use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::{broadcast, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

const EVENT_SCHEMA_MAJOR: u16 = 1;
const EVENT_SCHEMA_MINOR: u16 = 0;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    Queued,
    Running,
    WaitingForUser,
    Cancelling,
    Cancelled,
    Completed,
    Failed,
    FailedInterrupted,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskPhase {
    pub name: String,
    pub progress: Option<f32>,
    pub activity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StructuredError {
    pub code: String,
    pub message: String,
    pub recoverable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TaskEvent {
    pub schema_major: u16,
    pub schema_minor: u16,
    pub sequence: u64,
    pub task_id: Uuid,
    pub parent_id: Option<Uuid>,
    pub timestamp: DateTime<Utc>,
    pub state: TaskState,
    pub phase: TaskPhase,
    pub tool_name: Option<String>,
    pub resource_uri: Option<String>,
    pub permission_state: String,
    pub budget_state: String,
    pub cancellable: bool,
    pub error: Option<StructuredError>,
}

struct TaskControl {
    cancellation: CancellationToken,
    pause: watch::Sender<bool>,
    handle: JoinHandle<()>,
    last: TaskEvent,
}

#[derive(Clone)]
pub struct TaskContext {
    id: Uuid,
    cancellation: CancellationToken,
    pause: watch::Receiver<bool>,
    manager: TaskManager,
}

impl TaskContext {
    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    pub async fn checkpoint(&mut self) -> Result<(), Cancelled> {
        if self.cancellation.is_cancelled() {
            return Err(Cancelled);
        }
        while *self.pause.borrow() {
            tokio::select! {
                _ = self.cancellation.cancelled() => return Err(Cancelled),
                changed = self.pause.changed() => if changed.is_err() { return Err(Cancelled) },
            }
        }
        Ok(())
    }

    pub fn progress(
        &self,
        name: impl Into<String>,
        progress: Option<f32>,
        activity: impl Into<String>,
    ) {
        self.manager.transition(
            self.id,
            TaskState::Running,
            name,
            progress,
            activity,
            true,
            None,
        );
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Cancelled;

#[derive(Clone)]
pub struct TaskManager {
    inner: Arc<Mutex<Inner>>,
    events: broadcast::Sender<TaskEvent>,
}

struct Inner {
    sequence: u64,
    tasks: HashMap<Uuid, TaskControl>,
}

impl Default for TaskManager {
    fn default() -> Self {
        Self::new()
    }
}

impl TaskManager {
    pub fn new() -> Self {
        let (events, _) = broadcast::channel(512);
        Self {
            inner: Arc::new(Mutex::new(Inner {
                sequence: 0,
                tasks: HashMap::new(),
            })),
            events,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<TaskEvent> {
        self.events.subscribe()
    }

    pub fn spawn<F, Fut>(&self, kind: impl Into<String>, parent_id: Option<Uuid>, work: F) -> Uuid
    where
        F: FnOnce(TaskContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        let id = Uuid::new_v4();
        let kind = kind.into();
        let cancellation = CancellationToken::new();
        let control_cancellation = cancellation.clone();
        let (pause, pause_rx) = watch::channel(false);
        let manager = self.clone();
        let context = TaskContext {
            id,
            cancellation: cancellation.clone(),
            pause: pause_rx,
            manager: manager.clone(),
        };
        let (start_tx, start_rx) = oneshot::channel();

        let queued = self.make_event(
            id,
            parent_id,
            TaskState::Queued,
            &kind,
            Some(0.0),
            "Queued",
            true,
            None,
        );
        let handle = tokio::spawn(async move {
            let _ = start_rx.await;
            if !cancellation.is_cancelled() {
                manager.transition(
                    id,
                    TaskState::Running,
                    kind.clone(),
                    Some(0.0),
                    "Started",
                    true,
                    None,
                );
            }
            let result = work(context).await;
            if cancellation.is_cancelled() {
                manager.transition(
                    id,
                    TaskState::Cancelled,
                    kind,
                    None,
                    "Cancelled",
                    false,
                    None,
                );
            } else if let Err(message) = result {
                manager.transition(
                    id,
                    TaskState::Failed,
                    kind,
                    None,
                    "Failed",
                    false,
                    Some(StructuredError {
                        code: "worker_failed".into(),
                        message,
                        recoverable: true,
                    }),
                );
            } else {
                manager.transition(
                    id,
                    TaskState::Completed,
                    kind,
                    Some(1.0),
                    "Completed",
                    false,
                    None,
                );
            }
        });

        self.inner.lock().unwrap().tasks.insert(
            id,
            TaskControl {
                cancellation: control_cancellation,
                pause,
                handle,
                last: queued.clone(),
            },
        );
        let _ = self.events.send(queued);
        let _ = start_tx.send(());
        id
    }

    pub fn cancel(&self, id: Uuid) -> bool {
        let token = {
            let mut inner = self.inner.lock().unwrap();
            let Some(control) = inner.tasks.get_mut(&id) else {
                return false;
            };
            control.cancellation.clone()
        };
        self.transition(
            id,
            TaskState::Cancelling,
            "cancellation",
            None,
            "Stopping work",
            false,
            None,
        );
        token.cancel();
        true
    }

    pub fn pause(&self, id: Uuid, paused: bool) -> bool {
        let last = {
            let inner = self.inner.lock().unwrap();
            let Some(control) = inner.tasks.get(&id) else {
                return false;
            };
            if control.pause.send(paused).is_err() {
                return false;
            }
            control.last.clone()
        };
        self.transition(
            id,
            TaskState::Running,
            last.phase.name,
            last.phase.progress,
            if paused { "Paused" } else { "Resuming" },
            true,
            None,
        );
        true
    }

    pub fn cancel_all(&self) -> usize {
        let ids: Vec<_> = self
            .inner
            .lock()
            .unwrap()
            .tasks
            .iter()
            .filter(|(_, control)| control.last.cancellable)
            .map(|(id, _)| *id)
            .collect();
        ids.iter().filter(|id| self.cancel(**id)).count()
    }

    pub async fn enforce_cancel_deadlines(&self, id: Uuid) {
        tokio::time::sleep(Duration::from_secs(10)).await;
        let inner = self.inner.lock().unwrap();
        if let Some(control) = inner.tasks.get(&id) {
            if !control.handle.is_finished() {
                control.handle.abort();
            }
        }
    }

    pub fn snapshot(&self) -> Vec<TaskEvent> {
        self.inner
            .lock()
            .unwrap()
            .tasks
            .values()
            .map(|control| control.last.clone())
            .collect()
    }

    fn transition(
        &self,
        id: Uuid,
        state: TaskState,
        name: impl Into<String>,
        progress: Option<f32>,
        activity: impl Into<String>,
        cancellable: bool,
        error: Option<StructuredError>,
    ) {
        let parent_id = self
            .inner
            .lock()
            .unwrap()
            .tasks
            .get(&id)
            .and_then(|control| control.last.parent_id);
        let event = self.make_event(
            id,
            parent_id,
            state,
            name,
            progress,
            activity,
            cancellable,
            error,
        );
        if let Some(control) = self.inner.lock().unwrap().tasks.get_mut(&id) {
            control.last = event.clone();
        }
        let _ = self.events.send(event);
    }

    fn make_event(
        &self,
        task_id: Uuid,
        parent_id: Option<Uuid>,
        state: TaskState,
        name: impl Into<String>,
        progress: Option<f32>,
        activity: impl Into<String>,
        cancellable: bool,
        error: Option<StructuredError>,
    ) -> TaskEvent {
        let sequence = {
            let mut inner = self.inner.lock().unwrap();
            inner.sequence += 1;
            inner.sequence
        };
        TaskEvent {
            schema_major: EVENT_SCHEMA_MAJOR,
            schema_minor: EVENT_SCHEMA_MINOR,
            sequence,
            task_id,
            parent_id,
            timestamp: Utc::now(),
            state,
            phase: TaskPhase {
                name: name.into(),
                progress: progress.map(|value| value.clamp(0.0, 1.0)),
                activity: activity.into(),
            },
            tool_name: None,
            resource_uri: None,
            permission_state: "approved".into(),
            budget_state: "within_budget".into(),
            cancellable,
            error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn emits_monotonic_events_and_completes() {
        let manager = TaskManager::new();
        let mut events = manager.subscribe();
        manager.spawn("fixture", None, |_context| async { Ok(()) });
        let first = events.recv().await.unwrap();
        let second = events.recv().await.unwrap();
        assert!(second.sequence > first.sequence);
    }

    #[tokio::test]
    async fn cooperative_worker_observes_cancellation() {
        let manager = TaskManager::new();
        let observed = Arc::new(AtomicBool::new(false));
        let flag = observed.clone();
        let id = manager.spawn("fixture", None, move |context| async move {
            context.cancellation_token().cancelled().await;
            flag.store(true, Ordering::SeqCst);
            Ok(())
        });
        assert!(manager.cancel(id));
        tokio::time::timeout(Duration::from_millis(250), async {
            while !observed.load(Ordering::SeqCst) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
