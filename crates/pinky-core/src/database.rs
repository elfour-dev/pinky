use std::{fs, path::Path};

use rusqlite::Connection;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{Vault, VaultError};

const SCHEMA_VERSION: i64 = 4;
const MIGRATION_001: &str = include_str!("../migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("../migrations/002_hybrid_configuration.sql");
const MIGRATION_003: &str = include_str!("../migrations/003_asset_indexes.sql");
const MIGRATION_004: &str = include_str!("../migrations/004_ollama_configuration.sql");

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("vault unavailable: {0}")]
    Vault(#[from] VaultError),
    #[error("SQLCipher is unavailable; refusing to create an unencrypted database")]
    SqlCipherUnavailable,
    #[error("the vault database key must contain exactly 256 bits")]
    InvalidKeyLength,
    #[error("unsupported database schema {found}; this build supports {supported}")]
    UnsupportedSchema { found: i64, supported: i64 },
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("database I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// An encrypted metadata database. The key is consumed and zeroized by the
/// caller-visible wrapper after SQLCipher has copied it into the connection.
pub struct Database {
    connection: Connection,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HybridConfiguration {
    pub qdrant_executable: String,
    pub embedding_endpoint: String,
    pub embedding_model: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OllamaConfiguration {
    pub endpoint: String,
    pub model: String,
}

impl Database {
    pub fn open(vault: &Vault, key: Zeroizing<Vec<u8>>) -> Result<Self, DatabaseError> {
        if key.len() != 32 {
            return Err(DatabaseError::InvalidKeyLength);
        }
        vault.ensure_mounted()?;
        let path = vault.root().join("database/pinky.sqlite3");
        let is_new = !path.exists();
        let connection = Connection::open(&path)?;
        connection.pragma_update(None, "key", format!("x'{}'", hex::encode(key.as_slice())))?;
        let cipher_version: String = connection
            .query_row("PRAGMA cipher_version", [], |row| row.get(0))
            .map_err(|_| DatabaseError::SqlCipherUnavailable)?;
        if cipher_version.trim().is_empty() {
            return Err(DatabaseError::SqlCipherUnavailable);
        }
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.pragma_update(None, "secure_delete", "ON")?;
        connection.busy_timeout(std::time::Duration::from_secs(5))?;

        let version: i64 = connection.pragma_query_value(None, "user_version", |row| row.get(0))?;
        if version > SCHEMA_VERSION {
            return Err(DatabaseError::UnsupportedSchema {
                found: version,
                supported: SCHEMA_VERSION,
            });
        }
        if is_new || version == 0 {
            connection.execute_batch(MIGRATION_001)?;
            connection.execute_batch(MIGRATION_002)?;
            connection.execute_batch(MIGRATION_003)?;
            connection.execute_batch(MIGRATION_004)?;
        } else {
            if version < 2 {
                connection.execute_batch(MIGRATION_002)?;
            }
            if version < 3 {
                connection.execute_batch(MIGRATION_003)?;
            }
            if version < 4 {
                connection.execute_batch(MIGRATION_004)?;
            }
        }
        connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        sync_parent(&path)?;
        Ok(Self { connection })
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn hybrid_configuration(&self) -> Result<Option<HybridConfiguration>, DatabaseError> {
        let result = self.connection.query_row(
            "SELECT qdrant_executable, embedding_endpoint, embedding_model
                 FROM hybrid_configuration WHERE id = 1",
            [],
            |row| {
                Ok(HybridConfiguration {
                    qdrant_executable: row.get(0)?,
                    embedding_endpoint: row.get(1)?,
                    embedding_model: row.get(2)?,
                })
            },
        );
        match result {
            Ok(configuration) => Ok(Some(configuration)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(DatabaseError::Sql(error)),
        }
    }

    pub fn set_hybrid_configuration(
        &self,
        configuration: &HybridConfiguration,
    ) -> Result<(), DatabaseError> {
        self.connection.execute(
            "INSERT INTO hybrid_configuration
                (id, qdrant_executable, embedding_endpoint, embedding_model, configured_at, updated_at)
             VALUES (1, ?1, ?2, ?3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
             ON CONFLICT(id) DO UPDATE SET
                qdrant_executable = excluded.qdrant_executable,
                embedding_endpoint = excluded.embedding_endpoint,
                embedding_model = excluded.embedding_model,
                updated_at = CURRENT_TIMESTAMP",
            rusqlite::params![
                configuration.qdrant_executable,
                configuration.embedding_endpoint,
                configuration.embedding_model,
            ],
        )?;
        Ok(())
    }

    pub fn clear_hybrid_configuration(&self) -> Result<(), DatabaseError> {
        self.connection
            .execute("DELETE FROM hybrid_configuration WHERE id = 1", [])?;
        Ok(())
    }

    pub fn ollama_configuration(&self) -> Result<Option<OllamaConfiguration>, DatabaseError> {
        let result = self.connection.query_row(
            "SELECT endpoint, model FROM ollama_configuration WHERE id = 1",
            [],
            |row| {
                Ok(OllamaConfiguration {
                    endpoint: row.get(0)?,
                    model: row.get(1)?,
                })
            },
        );
        match result {
            Ok(configuration) => Ok(Some(configuration)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(DatabaseError::Sql(error)),
        }
    }

    pub fn set_ollama_configuration(
        &self,
        configuration: &OllamaConfiguration,
    ) -> Result<(), DatabaseError> {
        self.connection.execute(
            "INSERT INTO ollama_configuration
                (id, endpoint, model, configured_at, updated_at)
             VALUES (1, ?1, ?2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
             ON CONFLICT(id) DO UPDATE SET
                endpoint = excluded.endpoint,
                model = excluded.model,
                updated_at = CURRENT_TIMESTAMP",
            rusqlite::params![configuration.endpoint, configuration.model],
        )?;
        Ok(())
    }

    pub fn clear_ollama_configuration(&self) -> Result<(), DatabaseError> {
        self.connection
            .execute("DELETE FROM ollama_configuration WHERE id = 1", [])?;
        Ok(())
    }
}

fn sync_parent(path: &Path) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MountVerifier;
    use std::path::Path;

    struct Mounted;
    impl MountVerifier for Mounted {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(true)
        }
    }

    #[test]
    fn creates_an_encrypted_versioned_schema() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let database = Database::open(&vault, Zeroizing::new(vec![0x5a; 32])).unwrap();
        let version: i64 = database
            .connection()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        let source_table: String = database
            .connection()
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'sources'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let hybrid_table: String = database
            .connection()
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'hybrid_configuration'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(source_table, "sources");
        assert_eq!(hybrid_table, "hybrid_configuration");
        let ollama_table: String = database
            .connection()
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'ollama_configuration'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(ollama_table, "ollama_configuration");
        database
            .set_hybrid_configuration(&HybridConfiguration {
                qdrant_executable: "/usr/bin/qdrant".into(),
                embedding_endpoint: "http://127.0.0.1:11434".into(),
                embedding_model: "nomic-embed-text".into(),
            })
            .unwrap();
        assert_eq!(
            database.hybrid_configuration().unwrap(),
            Some(HybridConfiguration {
                qdrant_executable: "/usr/bin/qdrant".into(),
                embedding_endpoint: "http://127.0.0.1:11434".into(),
                embedding_model: "nomic-embed-text".into(),
            })
        );
        database.clear_hybrid_configuration().unwrap();
        assert!(database.hybrid_configuration().unwrap().is_none());
        database
            .set_ollama_configuration(&OllamaConfiguration {
                endpoint: "http://127.0.0.1:11435".into(),
                model: "qwen3.5:4b".into(),
            })
            .unwrap();
        assert_eq!(
            database.ollama_configuration().unwrap(),
            Some(OllamaConfiguration {
                endpoint: "http://127.0.0.1:11435".into(),
                model: "qwen3.5:4b".into(),
            })
        );
        database.clear_ollama_configuration().unwrap();
        assert!(database.ollama_configuration().unwrap().is_none());
        drop(database);

        let raw = fs::read(vault.root().join("database/pinky.sqlite3")).unwrap();
        assert!(!raw.starts_with(b"SQLite format 3"));
        assert!(!raw
            .windows(b"CREATE TABLE".len())
            .any(|window| window == b"CREATE TABLE"));
    }

    #[test]
    fn rejects_non_256_bit_keys() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        assert!(matches!(
            Database::open(&vault, Zeroizing::new(vec![0; 16])),
            Err(DatabaseError::InvalidKeyLength)
        ));
    }

    #[test]
    fn migrates_schema_one_to_two_without_losing_metadata() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let database = Database::open(&vault, Zeroizing::new(vec![0x2a; 32])).unwrap();
        database
            .connection()
            .execute("DROP TABLE hybrid_configuration", [])
            .unwrap();
        database
            .connection()
            .pragma_update(None, "user_version", 1_i64)
            .unwrap();
        drop(database);

        let reopened = Database::open(&vault, Zeroizing::new(vec![0x2a; 32])).unwrap();
        let version: i64 = reopened
            .connection()
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(reopened.hybrid_configuration().unwrap().is_none());
        let source_table: String = reopened
            .connection()
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'sources'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(source_table, "sources");
    }
}
