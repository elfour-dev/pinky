use std::{
    collections::HashMap,
    future::Future,
    sync::{Arc, Mutex},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::{
    process::Command,
    sync::{broadcast, oneshot, watch},
    task::JoinHandle,
};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{ProcessError, ProcessOutcome, ProcessSupervisor, TaskJournal, TaskJournalError};

const EVENT_SCHEMA_MAJOR: u16 = 1;
const EVENT_SCHEMA_MINOR: u16 = 0;
const FORCE_ABORT_DEADLINE: Duration = Duration::from_secs(12);

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
    handle: Option<JoinHandle<()>>,
    last: TaskEvent,
}

#[derive(Clone)]
pub struct TaskContext {
    id: Uuid,
    cancellation: CancellationToken,
    pause: watch::Receiver<bool>,
    manager: TaskManager,
    cancellable: bool,
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
            self.cancellable,
            None,
        );
    }

    pub async fn run_process(&self, command: Command) -> Result<ProcessOutcome, ProcessError> {
        ProcessSupervisor::new()
            .run(command, self.cancellation.clone())
            .await
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
    journal: Option<TaskJournal>,
    journal_error: Option<String>,
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
                journal: None,
                journal_error: None,
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
        self.spawn_internal(kind, parent_id, true, work)
    }

    pub fn spawn_uncancellable<F, Fut>(
        &self,
        kind: impl Into<String>,
        parent_id: Option<Uuid>,
        work: F,
    ) -> Uuid
    where
        F: FnOnce(TaskContext) -> Fut + Send + 'static,
        Fut: Future<Output = Result<(), String>> + Send + 'static,
    {
        self.spawn_internal(kind, parent_id, false, work)
    }

    fn spawn_internal<F, Fut>(
        &self,
        kind: impl Into<String>,
        parent_id: Option<Uuid>,
        cancellable: bool,
        work: F,
    ) -> Uuid
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
            cancellable,
        };
        let (start_tx, start_rx) = oneshot::channel();

        let queued = self.make_event(
            id,
            parent_id,
            TaskState::Queued,
            &kind,
            Some(0.0),
            "Queued",
            cancellable,
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
                    cancellable,
                    None,
                );
            }
            let result = work(context).await;
            match (cancellation.is_cancelled(), result) {
                (true, Err(message)) if message != "cancelled" => manager.transition(
                    id,
                    TaskState::Failed,
                    kind,
                    None,
                    "Cancellation failed",
                    false,
                    Some(StructuredError {
                        code: "cancellation_failed".into(),
                        message,
                        recoverable: false,
                    }),
                ),
                (true, _) => manager.transition(
                    id,
                    TaskState::Cancelled,
                    kind,
                    None,
                    "Cancelled",
                    false,
                    None,
                ),
                (false, Err(message)) => manager.transition(
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
                ),
                (false, Ok(())) => manager.transition(
                    id,
                    TaskState::Completed,
                    kind,
                    Some(1.0),
                    "Completed",
                    false,
                    None,
                ),
            }
        });

        self.inner.lock().unwrap().tasks.insert(
            id,
            TaskControl {
                cancellation: control_cancellation,
                pause,
                handle: Some(handle),
                last: queued.clone(),
            },
        );
        self.publish(queued);
        let _ = start_tx.send(());
        id
    }

    pub fn cancel(&self, id: Uuid) -> bool {
        let token = {
            let mut inner = self.inner.lock().unwrap();
            let Some(control) = inner.tasks.get_mut(&id) else {
                return false;
            };
            if !control.last.cancellable {
                return false;
            }
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
            if !control.last.cancellable {
                return false;
            }
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
        tokio::time::sleep(FORCE_ABORT_DEADLINE).await;
        let inner = self.inner.lock().unwrap();
        if let Some(control) = inner.tasks.get(&id) {
            if let Some(handle) = &control.handle {
                if !handle.is_finished() {
                    handle.abort();
                }
            }
        }
    }

    pub fn attach_journal(&self, journal: TaskJournal) -> Result<Vec<Uuid>, TaskJournalError> {
        let maximum_sequence = journal.maximum_sequence()?;
        let interrupted = journal.interrupted()?;
        {
            let mut inner = self.inner.lock().unwrap();
            if inner.journal.is_some() {
                return Ok(Vec::new());
            }
            inner.sequence = inner.sequence.max(maximum_sequence);
            inner.journal = Some(journal);
            inner.journal_error = None;
            for event in &interrupted {
                let (pause, _) = watch::channel(false);
                inner
                    .tasks
                    .entry(event.task_id)
                    .or_insert_with(|| TaskControl {
                        cancellation: CancellationToken::new(),
                        pause,
                        handle: None,
                        last: event.clone(),
                    });
            }
        }

        let mut recovered = Vec::with_capacity(interrupted.len());
        for event in interrupted {
            recovered.push(event.task_id);
            self.transition(
                event.task_id,
                TaskState::FailedInterrupted,
                event.phase.name,
                event.phase.progress,
                "Interrupted by application restart",
                false,
                Some(StructuredError {
                    code: "failed_interrupted".into(),
                    message:
                        "The application stopped before this task reached a durable terminal state"
                            .into(),
                    recoverable: true,
                }),
            );
        }
        Ok(recovered)
    }

    pub fn journal_error(&self) -> Option<String> {
        self.inner.lock().unwrap().journal_error.clone()
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
        self.publish(event);
    }

    fn publish(&self, event: TaskEvent) {
        let journal = self.inner.lock().unwrap().journal.clone();
        if let Some(journal) = journal {
            if let Err(error) = journal.append(&event) {
                self.inner.lock().unwrap().journal_error = Some(error.to_string());
            }
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

    #[tokio::test]
    async fn uncancellable_tasks_reject_stop_and_pause_requests() {
        let manager = TaskManager::new();
        let id = manager.spawn_uncancellable("vault setup", None, |_context| async {
            tokio::time::sleep(Duration::from_millis(20)).await;
            Ok(())
        });
        assert!(!manager.cancel(id));
        assert!(!manager.pause(id, true));
        assert!(
            !manager
                .snapshot()
                .into_iter()
                .find(|event| event.task_id == id)
                .unwrap()
                .cancellable
        );
    }

    #[tokio::test]
    async fn task_cancellation_reaches_a_supervised_process() {
        let manager = TaskManager::new();
        let id = manager.spawn("process fixture", None, |context| async move {
            let mut command = Command::new("/usr/bin/tail");
            command.args(["-f", "/dev/null"]);
            context
                .run_process(command)
                .await
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
        assert!(manager.cancel(id));
        tokio::time::timeout(Duration::from_secs(4), async {
            loop {
                let state = manager
                    .snapshot()
                    .into_iter()
                    .find(|event| event.task_id == id)
                    .unwrap()
                    .state;
                if state == TaskState::Cancelled {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn cancellation_failure_is_not_reported_as_cancelled() {
        let manager = TaskManager::new();
        let id = manager.spawn("failure fixture", None, |context| async move {
            context.cancellation_token().cancelled().await;
            Err("process remained in uninterruptible sleep".into())
        });
        assert!(manager.cancel(id));
        tokio::time::timeout(Duration::from_millis(250), async {
            loop {
                let event = manager
                    .snapshot()
                    .into_iter()
                    .find(|event| event.task_id == id)
                    .unwrap();
                if event.state == TaskState::Failed {
                    assert_eq!(event.error.unwrap().code, "cancellation_failed");
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
}
