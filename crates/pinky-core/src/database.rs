use std::{fs, path::Path};

use rusqlite::Connection;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{Vault, VaultError};

const SCHEMA_VERSION: i64 = 1;
const MIGRATION_001: &str = include_str!("../migrations/001_initial.sql");

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
            connection.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        sync_parent(&path)?;
        Ok(Self { connection })
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
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
        assert_eq!(version, SCHEMA_VERSION);
        assert_eq!(source_table, "sources");
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
}
