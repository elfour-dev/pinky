use std::sync::{Arc, Mutex};

use chrono::Utc;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{Database, ObjectStore, ObjectStoreError};

const MAX_TITLE_BYTES: usize = 256;
const MAX_CONTENT_BYTES: usize = 256 * 1024;
const MAX_CITATIONS: usize = 64;

#[derive(Debug, Error)]
pub enum ConversationError {
    #[error("conversation database lock is poisoned")]
    DatabaseLock,
    #[error("conversation database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("conversation object error: {0}")]
    Object(#[from] ObjectStoreError),
    #[error("conversation serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("conversation was not found")]
    NotFound,
    #[error("conversation title must contain between 1 and {MAX_TITLE_BYTES} bytes")]
    InvalidTitle,
    #[error("message content must contain between 1 and {MAX_CONTENT_BYTES} bytes")]
    InvalidContent,
    #[error("message role must be user or assistant")]
    InvalidRole,
    #[error("a message may contain at most {MAX_CITATIONS} citations")]
    TooManyCitations,
    #[error("message contains an invalid task UUID")]
    InvalidTask,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationSummary {
    pub id: Uuid,
    pub title: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationMessage {
    pub id: Uuid,
    pub conversation_id: Uuid,
    pub ordinal: u64,
    pub role: String,
    pub content: String,
    pub model: Option<String>,
    pub citations: Vec<String>,
    pub task_id: Option<Uuid>,
    pub replaces_message_id: Option<Uuid>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConversationDetail {
    pub summary: ConversationSummary,
    pub messages: Vec<ConversationMessage>,
}

pub struct MessageDraft<'a> {
    pub conversation_id: Uuid,
    pub role: &'a str,
    pub content: &'a str,
    pub model: Option<&'a str>,
    pub citations: &'a [String],
    pub task_id: Option<Uuid>,
    pub replaces_message_id: Option<Uuid>,
}

#[derive(Clone)]
pub struct ConversationService {
    database: Arc<Mutex<Database>>,
    objects: ObjectStore,
}

impl ConversationService {
    pub fn new(database: Arc<Mutex<Database>>, objects: ObjectStore) -> Self {
        Self { database, objects }
    }

    pub fn create(&self, title: &str) -> Result<ConversationSummary, ConversationError> {
        let title = validate_title(title)?;
        let id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let database = self
            .database
            .lock()
            .map_err(|_| ConversationError::DatabaseLock)?;
        database.connection().execute(
            "INSERT INTO conversations (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id.to_string(), title, now],
        )?;
        Ok(ConversationSummary {
            id,
            title,
            created_at: now.clone(),
            updated_at: now,
        })
    }

    pub fn list(&self) -> Result<Vec<ConversationSummary>, ConversationError> {
        let database = self
            .database
            .lock()
            .map_err(|_| ConversationError::DatabaseLock)?;
        let mut statement = database.connection().prepare(
            "SELECT id, title, created_at, updated_at
             FROM conversations ORDER BY updated_at DESC, id DESC",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        rows.map(|row| {
            let (id, title, created_at, updated_at) = row?;
            Ok(ConversationSummary {
                id: parse_uuid(&id)?,
                title,
                created_at,
                updated_at,
            })
        })
        .collect()
    }

    pub fn rename(&self, id: Uuid, title: &str) -> Result<ConversationSummary, ConversationError> {
        let title = validate_title(title)?;
        let now = Utc::now().to_rfc3339();
        let database = self
            .database
            .lock()
            .map_err(|_| ConversationError::DatabaseLock)?;
        let changed = database.connection().execute(
            "UPDATE conversations SET title = ?2, updated_at = ?3 WHERE id = ?1",
            params![id.to_string(), title, now],
        )?;
        if changed == 0 {
            return Err(ConversationError::NotFound);
        }
        let (created_at,): (String,) = database.connection().query_row(
            "SELECT created_at FROM conversations WHERE id = ?1",
            [id.to_string()],
            |row| Ok((row.get(0)?,)),
        )?;
        Ok(ConversationSummary {
            id,
            title,
            created_at,
            updated_at: now,
        })
    }

    pub fn delete(&self, id: Uuid) -> Result<(), ConversationError> {
        let database = self
            .database
            .lock()
            .map_err(|_| ConversationError::DatabaseLock)?;
        let transaction = database.connection().unchecked_transaction()?;
        let exists: Option<String> = transaction
            .query_row(
                "SELECT id FROM conversations WHERE id = ?1",
                [id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(ConversationError::NotFound);
        }
        let hashes = {
            let mut statement = transaction
                .prepare("SELECT content_object_hash FROM messages WHERE conversation_id = ?1")?;
            let rows = statement.query_map([id.to_string()], |row| row.get::<_, String>(0))?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        transaction.execute(
            "DELETE FROM messages WHERE conversation_id = ?1",
            [id.to_string()],
        )?;
        for hash in hashes {
            transaction.execute(
                "UPDATE objects SET reference_count = reference_count - 1
                 WHERE sha256 = ?1 AND reference_count > 0",
                [hash],
            )?;
        }
        transaction.execute("DELETE FROM conversations WHERE id = ?1", [id.to_string()])?;
        transaction.commit()?;
        Ok(())
    }

    pub fn get(&self, id: Uuid) -> Result<ConversationDetail, ConversationError> {
        let summary = {
            let database = self
                .database
                .lock()
                .map_err(|_| ConversationError::DatabaseLock)?;
            database
                .connection()
                .query_row(
                    "SELECT title, created_at, updated_at FROM conversations WHERE id = ?1",
                    [id.to_string()],
                    |row| {
                        Ok(ConversationSummary {
                            id,
                            title: row.get(0)?,
                            created_at: row.get(1)?,
                            updated_at: row.get(2)?,
                        })
                    },
                )
                .optional()?
                .ok_or(ConversationError::NotFound)?
        };
        let rows = {
            let database = self
                .database
                .lock()
                .map_err(|_| ConversationError::DatabaseLock)?;
            let mut statement = database.connection().prepare(
                "SELECT id, ordinal, role, content_object_hash, model, citations_json,
                        task_id, replaces_message_id, created_at
                 FROM messages WHERE conversation_id = ?1 ORDER BY ordinal ASC",
            )?;
            let rows = statement.query_map([id.to_string()], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, String>(8)?,
                ))
            })?;
            rows.collect::<Result<Vec<_>, _>>()?
        };
        let messages = rows
            .into_iter()
            .map(
                |(
                    message_id,
                    ordinal,
                    role,
                    content_hash,
                    model,
                    citations_json,
                    task_id,
                    replaces_message_id,
                    created_at,
                )| {
                    let citations = serde_json::from_str(&citations_json)?;
                    Ok(ConversationMessage {
                        id: parse_uuid(&message_id)?,
                        conversation_id: id,
                        ordinal: u64::try_from(ordinal).map_err(|_| {
                            ConversationError::Database(rusqlite::Error::InvalidQuery)
                        })?,
                        role,
                        content: String::from_utf8(self.objects.read_verified(&content_hash)?)
                            .map_err(|_| {
                                ConversationError::Object(ObjectStoreError::Io(
                                    std::io::Error::new(
                                        std::io::ErrorKind::InvalidData,
                                        "message object is not valid UTF-8",
                                    ),
                                ))
                            })?,
                        model,
                        citations,
                        task_id: task_id.as_deref().map(parse_uuid).transpose()?,
                        replaces_message_id: replaces_message_id
                            .as_deref()
                            .map(parse_uuid)
                            .transpose()?,
                        created_at,
                    })
                },
            )
            .collect::<Result<Vec<_>, ConversationError>>()?;
        Ok(ConversationDetail { summary, messages })
    }

    pub fn append_message(
        &self,
        draft: &MessageDraft<'_>,
    ) -> Result<ConversationMessage, ConversationError> {
        validate_content(draft.content)?;
        if !matches!(draft.role, "user" | "assistant") {
            return Err(ConversationError::InvalidRole);
        }
        if draft.citations.len() > MAX_CITATIONS {
            return Err(ConversationError::TooManyCitations);
        }
        let citations_json = serde_json::to_string(draft.citations)?;
        let stored = self.objects.put(draft.content.as_bytes(), "text/plain")?;
        let message_id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let database = self
            .database
            .lock()
            .map_err(|_| ConversationError::DatabaseLock)?;
        let transaction = database.connection().unchecked_transaction()?;
        let exists: Option<String> = transaction
            .query_row(
                "SELECT id FROM conversations WHERE id = ?1",
                [draft.conversation_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(ConversationError::NotFound);
        }
        let ordinal: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(ordinal) + 1, 0) FROM messages WHERE conversation_id = ?1",
            [draft.conversation_id.to_string()],
            |row| row.get(0),
        )?;
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
        transaction.execute(
            "INSERT INTO messages (
                id, conversation_id, ordinal, role, content_object_hash, model,
                citations_json, task_id, replaces_message_id, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                message_id.to_string(),
                draft.conversation_id.to_string(),
                ordinal,
                draft.role,
                stored.metadata.sha256,
                draft.model,
                citations_json,
                draft.task_id.map(|id| id.to_string()),
                draft.replaces_message_id.map(|id| id.to_string()),
                now,
            ],
        )?;
        transaction.execute(
            "UPDATE conversations SET updated_at = ?2 WHERE id = ?1",
            params![draft.conversation_id.to_string(), now],
        )?;
        transaction.commit()?;
        Ok(ConversationMessage {
            id: message_id,
            conversation_id: draft.conversation_id,
            ordinal: u64::try_from(ordinal)
                .map_err(|_| ConversationError::Database(rusqlite::Error::InvalidQuery))?,
            role: draft.role.to_owned(),
            content: draft.content.to_owned(),
            model: draft.model.map(str::to_owned),
            citations: draft.citations.to_owned(),
            task_id: draft.task_id,
            replaces_message_id: draft.replaces_message_id,
            created_at: now,
        })
    }
}

fn validate_title(title: &str) -> Result<String, ConversationError> {
    let title = title.trim();
    if title.is_empty() || title.len() > MAX_TITLE_BYTES {
        return Err(ConversationError::InvalidTitle);
    }
    Ok(title.to_owned())
}

fn validate_content(content: &str) -> Result<(), ConversationError> {
    if content.trim().is_empty() || content.len() > MAX_CONTENT_BYTES {
        return Err(ConversationError::InvalidContent);
    }
    Ok(())
}

fn parse_uuid(value: &str) -> Result<Uuid, ConversationError> {
    Uuid::parse_str(value).map_err(|_| ConversationError::Database(rusqlite::Error::InvalidQuery))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Database, MountVerifier, ObjectStore, Vault};
    use std::{path::Path, sync::Arc};
    use zeroize::Zeroizing;

    struct Mounted;
    impl MountVerifier for Mounted {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(true)
        }
    }

    fn fixture() -> (tempfile::TempDir, ConversationService) {
        let directory = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(directory.path(), Mounted).unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, Zeroizing::new(vec![0x45; 32])).unwrap(),
        ));
        (
            directory,
            ConversationService::new(database, ObjectStore::new(vault)),
        )
    }

    #[test]
    fn persists_immutable_ordered_messages_and_reopens_them() {
        let (directory, service) = fixture();
        let conversation = service.create("Incident notes").unwrap();
        service
            .append_message(&MessageDraft {
                conversation_id: conversation.id,
                role: "user",
                content: "What happened?",
                model: None,
                citations: &[],
                task_id: None,
                replaces_message_id: None,
            })
            .unwrap();
        let citations = vec!["pinky://source/a/version/b#chunk-0".to_owned()];
        service
            .append_message(&MessageDraft {
                conversation_id: conversation.id,
                role: "assistant",
                content: "The retained record reports a restart.",
                model: Some("qwen3.5:9b"),
                citations: &citations,
                task_id: None,
                replaces_message_id: None,
            })
            .unwrap();

        let reopened = service.get(conversation.id).unwrap();
        assert_eq!(reopened.summary.title, "Incident notes");
        assert_eq!(reopened.messages.len(), 2);
        assert_eq!(reopened.messages[0].ordinal, 0);
        assert_eq!(reopened.messages[1].ordinal, 1);
        assert_eq!(reopened.messages[1].citations, citations);
        assert_eq!(reopened.messages[1].task_id, None);

        drop(service);
        let reopened_vault = Vault::open_with(directory.path(), Mounted).unwrap();
        let reopened_database = Arc::new(Mutex::new(
            Database::open(&reopened_vault, Zeroizing::new(vec![0x45; 32])).unwrap(),
        ));
        let restarted =
            ConversationService::new(reopened_database, ObjectStore::new(reopened_vault));
        assert_eq!(restarted.get(conversation.id).unwrap().messages.len(), 2);
    }

    #[test]
    fn refuses_invalid_messages_without_creating_rows() {
        let (_directory, service) = fixture();
        let conversation = service.create("Notes").unwrap();
        assert!(matches!(
            service.append_message(&MessageDraft {
                conversation_id: conversation.id,
                role: "tool",
                content: "not allowed",
                model: None,
                citations: &[],
                task_id: None,
                replaces_message_id: None,
            }),
            Err(ConversationError::InvalidRole)
        ));
        assert!(service.get(conversation.id).unwrap().messages.is_empty());
    }

    #[test]
    fn renames_and_deletes_a_conversation_without_leaving_message_rows() {
        let (_directory, service) = fixture();
        let conversation = service.create("Old title").unwrap();
        service
            .append_message(&MessageDraft {
                conversation_id: conversation.id,
                role: "user",
                content: "Keep this encrypted.",
                model: None,
                citations: &[],
                task_id: None,
                replaces_message_id: None,
            })
            .unwrap();
        assert_eq!(
            service.rename(conversation.id, "New title").unwrap().title,
            "New title"
        );
        service.delete(conversation.id).unwrap();
        assert!(matches!(
            service.get(conversation.id),
            Err(ConversationError::NotFound)
        ));
    }
}
