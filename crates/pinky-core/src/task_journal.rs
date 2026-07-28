use std::sync::{Arc, Mutex};

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{Database, ObjectStore, ObjectStoreError, TaskEvent, TaskState};

const LOG_SCHEMA_MAJOR: u16 = 1;
const LOG_SCHEMA_MINOR: u16 = 0;

#[derive(Debug, Error)]
pub enum TaskJournalError {
    #[error("task journal database lock is poisoned")]
    DatabaseLock,
    #[error("task journal database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("task journal object error: {0}")]
    Object(#[from] ObjectStoreError),
    #[error("task journal serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("task journal is corrupt: {0}")]
    Corrupt(String),
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskEventLog {
    schema_major: u16,
    schema_minor: u16,
    task_id: Uuid,
    events: Vec<TaskEvent>,
}

#[derive(Clone)]
pub struct TaskJournal {
    database: Arc<Mutex<Database>>,
    objects: ObjectStore,
}

impl TaskJournal {
    pub fn new(database: Arc<Mutex<Database>>, objects: ObjectStore) -> Self {
        Self { database, objects }
    }

    pub fn append(&self, event: &TaskEvent) -> Result<String, TaskJournalError> {
        let database = self
            .database
            .lock()
            .map_err(|_| TaskJournalError::DatabaseLock)?;
        let previous_hash: Option<String> = database
            .connection()
            .query_row(
                "SELECT event_log_hash FROM tasks WHERE id = ?1",
                [event.task_id.to_string()],
                |row| row.get(0),
            )
            .optional()?
            .flatten();

        let mut log = match previous_hash.as_deref() {
            Some(hash) => self.read_log(hash, event.task_id)?,
            None => TaskEventLog {
                schema_major: LOG_SCHEMA_MAJOR,
                schema_minor: LOG_SCHEMA_MINOR,
                task_id: event.task_id,
                events: Vec::new(),
            },
        };
        if log
            .events
            .last()
            .is_some_and(|previous| previous.sequence >= event.sequence)
        {
            return Err(TaskJournalError::Corrupt(format!(
                "task {} received non-monotonic event sequence {}",
                event.task_id, event.sequence
            )));
        }
        log.events.push(event.clone());

        let bytes = serde_json::to_vec(&log)?;
        let stored = self.objects.put(&bytes, "application/json")?;
        let transaction = database.connection().unchecked_transaction()?;
        transaction.execute(
            "INSERT INTO objects (
                sha256, uncompressed_length, compressed_length, mime_type, compression_level
             ) VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(sha256) DO NOTHING",
            params![
                stored.metadata.sha256,
                stored.metadata.uncompressed_length,
                stored.metadata.compressed_length,
                stored.metadata.mime_type,
                stored.metadata.compression_level,
            ],
        )?;
        transaction.execute(
            "UPDATE objects SET reference_count = reference_count + 1 WHERE sha256 = ?1",
            [&stored.metadata.sha256],
        )?;
        if let Some(previous_hash) = previous_hash.as_deref() {
            if previous_hash != stored.metadata.sha256 {
                transaction.execute(
                    "UPDATE objects
                     SET reference_count = reference_count - 1
                     WHERE sha256 = ?1 AND reference_count > 0",
                    [previous_hash],
                )?;
            }
        }

        let kind = log
            .events
            .first()
            .map(|first| first.phase.name.as_str())
            .unwrap_or(event.phase.name.as_str());
        let permission_scope = serde_json::to_string(&event.permission_state)?;
        let research_budget = serde_json::to_string(&event.budget_state)?;
        transaction.execute(
            "INSERT INTO tasks (
                id, parent_id, kind, state, phase, progress, permission_scope_json,
                research_budget_json, cancellation_requested, event_log_hash, error_code,
                recoverable, created_at, updated_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)
             ON CONFLICT(id) DO UPDATE SET
                state = excluded.state,
                phase = excluded.phase,
                progress = excluded.progress,
                permission_scope_json = excluded.permission_scope_json,
                research_budget_json = excluded.research_budget_json,
                cancellation_requested = excluded.cancellation_requested,
                event_log_hash = excluded.event_log_hash,
                error_code = excluded.error_code,
                recoverable = excluded.recoverable,
                updated_at = excluded.updated_at",
            params![
                event.task_id.to_string(),
                event.parent_id.map(|id| id.to_string()),
                kind,
                state_name(event.state),
                event.phase.name,
                event.phase.progress.map(f64::from),
                permission_scope,
                research_budget,
                matches!(event.state, TaskState::Cancelling | TaskState::Cancelled),
                stored.metadata.sha256,
                event.error.as_ref().map(|error| error.code.as_str()),
                event.error.as_ref().is_some_and(|error| error.recoverable),
                event.timestamp.to_rfc3339(),
            ],
        )?;
        transaction.commit()?;
        Ok(stored.metadata.sha256)
    }

    pub fn maximum_sequence(&self) -> Result<u64, TaskJournalError> {
        Ok(self
            .load_rows(None)?
            .into_iter()
            .map(|event| event.sequence)
            .max()
            .unwrap_or(0))
    }

    pub fn interrupted(&self) -> Result<Vec<TaskEvent>, TaskJournalError> {
        self.load_rows(Some(&["running", "cancelling"]))
    }

    fn load_rows(&self, states: Option<&[&str]>) -> Result<Vec<TaskEvent>, TaskJournalError> {
        let rows = {
            let database = self
                .database
                .lock()
                .map_err(|_| TaskJournalError::DatabaseLock)?;
            let mut statement = database.connection().prepare(
                "SELECT id, state, event_log_hash
                 FROM tasks
                 WHERE event_log_hash IS NOT NULL
                 ORDER BY created_at, id",
            )?;
            let mapped = statement.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            mapped.collect::<Result<Vec<_>, _>>()?
        };

        rows.into_iter()
            .filter(|(_, state, _)| states.is_none_or(|allowed| allowed.contains(&state.as_str())))
            .map(|(id, state, hash)| {
                let task_id = Uuid::parse_str(&id).map_err(|_| {
                    TaskJournalError::Corrupt(format!("task row has invalid UUID {id}"))
                })?;
                let log = self.read_log(&hash, task_id)?;
                let last = log.events.last().cloned().ok_or_else(|| {
                    TaskJournalError::Corrupt(format!("task {task_id} has an empty event log"))
                })?;
                if state_name(last.state) != state {
                    return Err(TaskJournalError::Corrupt(format!(
                        "task {task_id} state does not match its event log"
                    )));
                }
                Ok(last)
            })
            .collect()
    }

    fn read_log(&self, hash: &str, task_id: Uuid) -> Result<TaskEventLog, TaskJournalError> {
        let log: TaskEventLog = serde_json::from_slice(&self.objects.read_verified(hash)?)?;
        if log.schema_major != LOG_SCHEMA_MAJOR
            || log.schema_minor > LOG_SCHEMA_MINOR
            || log.task_id != task_id
        {
            return Err(TaskJournalError::Corrupt(format!(
                "unsupported or mismatched event log for task {task_id}"
            )));
        }
        Ok(log)
    }
}

fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Queued => "queued",
        TaskState::Running => "running",
        TaskState::WaitingForUser => "waiting_for_user",
        TaskState::Cancelling => "cancelling",
        TaskState::Cancelled => "cancelled",
        TaskState::Completed => "completed",
        TaskState::Failed => "failed",
        TaskState::FailedInterrupted => "failed_interrupted",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{future::pending, path::Path, time::Duration};

    use crate::{MountVerifier, TaskManager, Vault};
    use zeroize::Zeroizing;

    struct Mounted;

    impl MountVerifier for Mounted {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(true)
        }
    }

    struct Fixture {
        _directory: tempfile::TempDir,
        vault: Vault,
        database: Arc<Mutex<Database>>,
        objects: ObjectStore,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().unwrap();
            let vault = Vault::open_with(directory.path(), Mounted).unwrap();
            let database = Arc::new(Mutex::new(
                Database::open(&vault, Zeroizing::new(vec![0x6b; 32])).unwrap(),
            ));
            let objects = ObjectStore::new(vault.clone());
            Self {
                _directory: directory,
                vault,
                database,
                objects,
            }
        }

        fn journal(&self) -> TaskJournal {
            TaskJournal::new(self.database.clone(), self.objects.clone())
        }

        async fn wait_for_state(&self, task_id: Uuid, state: &str) {
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    let current: Option<String> = self
                        .database
                        .lock()
                        .unwrap()
                        .connection()
                        .query_row(
                            "SELECT state FROM tasks WHERE id = ?1",
                            [task_id.to_string()],
                            |row| row.get(0),
                        )
                        .optional()
                        .unwrap();
                    if current.as_deref() == Some(state) {
                        break;
                    }
                    tokio::task::yield_now().await;
                }
            })
            .await
            .unwrap();
        }

        fn current_log(&self, task_id: Uuid) -> TaskEventLog {
            let hash: String = self
                .database
                .lock()
                .unwrap()
                .connection()
                .query_row(
                    "SELECT event_log_hash FROM tasks WHERE id = ?1",
                    [task_id.to_string()],
                    |row| row.get(0),
                )
                .unwrap();
            self.journal().read_log(&hash, task_id).unwrap()
        }
    }

    #[tokio::test]
    async fn stores_complete_compressed_event_history_and_references_latest_object() {
        let fixture = Fixture::new();
        let manager = TaskManager::new();
        assert!(manager
            .attach_journal(fixture.journal())
            .unwrap()
            .is_empty());
        let task_id = manager.spawn("durability fixture", None, |context| async move {
            context.progress(
                "durability fixture",
                Some(0.5),
                "Writing durable checkpoint",
            );
            Ok(())
        });
        fixture.wait_for_state(task_id, "completed").await;

        let log = fixture.current_log(task_id);
        assert!(log.events.len() >= 4);
        assert_eq!(log.events.first().unwrap().state, TaskState::Queued);
        assert_eq!(log.events.last().unwrap().state, TaskState::Completed);
        assert!(log
            .events
            .windows(2)
            .all(|events| events[0].sequence < events[1].sequence));

        let (references, mime): (i64, String) = fixture
            .database
            .lock()
            .unwrap()
            .connection()
            .query_row(
                "SELECT reference_count, mime_type
                 FROM objects
                 WHERE sha256 = (SELECT event_log_hash FROM tasks WHERE id = ?1)",
                [task_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(references, 1);
        assert_eq!(mime, "application/json");
    }

    #[tokio::test]
    async fn restart_marks_running_task_failed_interrupted_at_a_durable_checkpoint() {
        let fixture = Fixture::new();
        let first_manager = TaskManager::new();
        first_manager.attach_journal(fixture.journal()).unwrap();
        let task_id = first_manager.spawn("long fixture", None, |_context| async move {
            pending::<()>().await;
            Ok(())
        });
        fixture.wait_for_state(task_id, "running").await;

        let restarted_manager = TaskManager::new();
        assert_eq!(
            restarted_manager.attach_journal(fixture.journal()).unwrap(),
            vec![task_id]
        );
        let recovered = restarted_manager
            .snapshot()
            .into_iter()
            .find(|event| event.task_id == task_id)
            .unwrap();
        assert_eq!(recovered.state, TaskState::FailedInterrupted);
        assert_eq!(recovered.error.unwrap().code, "failed_interrupted");

        let (state, error, recoverable): (String, String, bool) = fixture
            .database
            .lock()
            .unwrap()
            .connection()
            .query_row(
                "SELECT state, error_code, recoverable FROM tasks WHERE id = ?1",
                [task_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(state, "failed_interrupted");
        assert_eq!(error, "failed_interrupted");
        assert!(recoverable);
    }

    #[tokio::test]
    async fn refuses_to_recover_a_corrupt_event_log() {
        let fixture = Fixture::new();
        let manager = TaskManager::new();
        manager.attach_journal(fixture.journal()).unwrap();
        let task_id = manager.spawn("corruption fixture", None, |_context| async move {
            pending::<()>().await;
            Ok(())
        });
        fixture.wait_for_state(task_id, "running").await;
        let hash: String = fixture
            .database
            .lock()
            .unwrap()
            .connection()
            .query_row(
                "SELECT event_log_hash FROM tasks WHERE id = ?1",
                [task_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        std::fs::write(
            fixture
                .vault
                .root()
                .join(format!("objects/{}/{}.zst", &hash[..2], hash)),
            b"corrupt",
        )
        .unwrap();

        let restarted_manager = TaskManager::new();
        assert!(matches!(
            restarted_manager.attach_journal(fixture.journal()),
            Err(TaskJournalError::Object(_))
        ));
    }
}
