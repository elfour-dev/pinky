use std::{fs, path::Path};

use rusqlite::Connection;
use thiserror::Error;
use zeroize::Zeroizing;

use crate::{ArtifactKind, ArtifactManifestV1, Vault, VaultError};

const SCHEMA_VERSION: i64 = 6;
const MIGRATION_001: &str = include_str!("../migrations/001_initial.sql");
const MIGRATION_002: &str = include_str!("../migrations/002_hybrid_configuration.sql");
const MIGRATION_003: &str = include_str!("../migrations/003_asset_indexes.sql");
const MIGRATION_004: &str = include_str!("../migrations/004_ollama_configuration.sql");
const MIGRATION_005: &str = include_str!("../migrations/005_verified_artifacts.sql");
const MIGRATION_006: &str = include_str!("../migrations/006_source_watch_controls.sql");

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
    #[error("verified artifact metadata is invalid: {0}")]
    InvalidVerifiedArtifact(String),
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedArtifact {
    pub key_id: String,
    pub manifest: ArtifactManifestV1,
    pub installed_path: String,
    pub signed_manifest_json: String,
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
            connection.execute_batch(MIGRATION_005)?;
            connection.execute_batch(MIGRATION_006)?;
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
            if version < 5 {
                connection.execute_batch(MIGRATION_005)?;
            }
            if version < 6 {
                connection.execute_batch(MIGRATION_006)?;
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

    pub fn record_verified_artifact(
        &self,
        artifact: &VerifiedArtifact,
    ) -> Result<(), DatabaseError> {
        artifact
            .manifest
            .validate()
            .map_err(|error| DatabaseError::InvalidVerifiedArtifact(error.to_string()))?;
        if artifact.key_id.trim().is_empty()
            || artifact.key_id.chars().any(char::is_whitespace)
            || artifact.installed_path.trim().is_empty()
            || artifact.signed_manifest_json.trim().is_empty()
        {
            return Err(DatabaseError::InvalidVerifiedArtifact(
                "key ID, installed path, and signed manifest are required".into(),
            ));
        }
        self.connection.execute(
            "INSERT INTO verified_artifacts
                (artifact_id, key_id, kind, capability, version, url, sha256, byte_size,
                 license_url, runtime_version, context_length, minimum_ram_bytes,
                 installed_path, signed_manifest_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)
             ON CONFLICT(artifact_id) DO UPDATE SET
                key_id = excluded.key_id, kind = excluded.kind, capability = excluded.capability,
                version = excluded.version, url = excluded.url, sha256 = excluded.sha256,
                byte_size = excluded.byte_size, license_url = excluded.license_url,
                runtime_version = excluded.runtime_version,
                context_length = excluded.context_length,
                minimum_ram_bytes = excluded.minimum_ram_bytes,
                installed_path = excluded.installed_path,
                signed_manifest_json = excluded.signed_manifest_json,
                installed_at = CURRENT_TIMESTAMP",
            rusqlite::params![
                artifact.manifest.artifact_id,
                artifact.key_id,
                artifact_kind_name(artifact.manifest.kind),
                artifact.manifest.capability,
                artifact.manifest.version,
                artifact.manifest.url,
                artifact.manifest.sha256,
                artifact.manifest.byte_size,
                artifact.manifest.license_url,
                artifact.manifest.runtime_version,
                artifact.manifest.context_length,
                artifact.manifest.minimum_ram_bytes,
                artifact.installed_path,
                artifact.signed_manifest_json,
            ],
        )?;
        Ok(())
    }

    pub fn verified_artifact(
        &self,
        artifact_id: &str,
    ) -> Result<Option<VerifiedArtifact>, DatabaseError> {
        let result = self.connection.query_row(
            "SELECT key_id, kind, capability, version, url, sha256, byte_size, license_url,
                    runtime_version, context_length, minimum_ram_bytes, installed_path,
                    signed_manifest_json
             FROM verified_artifacts WHERE artifact_id = ?1",
            [artifact_id],
            |row| {
                let kind: String = row.get(1)?;
                let kind = artifact_kind_from_name(&kind).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(VerifiedArtifact {
                    key_id: row.get(0)?,
                    manifest: ArtifactManifestV1 {
                        schema_version: crate::ARTIFACT_MANIFEST_SCHEMA_VERSION,
                        artifact_id: artifact_id.to_owned(),
                        kind,
                        capability: row.get(2)?,
                        version: row.get(3)?,
                        url: row.get(4)?,
                        sha256: row.get(5)?,
                        byte_size: row.get(6)?,
                        license_url: row.get(7)?,
                        runtime_version: row.get(8)?,
                        context_length: row.get(9)?,
                        minimum_ram_bytes: row.get(10)?,
                    },
                    installed_path: row.get(11)?,
                    signed_manifest_json: row.get(12)?,
                })
            },
        );
        match result {
            Ok(artifact) => Ok(Some(artifact)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(DatabaseError::Sql(error)),
        }
    }

    pub fn verified_artifact_by_installed_path(
        &self,
        installed_path: &str,
    ) -> Result<Option<VerifiedArtifact>, DatabaseError> {
        let result = self.connection.query_row(
            "SELECT artifact_id, key_id, kind, capability, version, url, sha256, byte_size,
                    license_url, runtime_version, context_length, minimum_ram_bytes,
                    signed_manifest_json
             FROM verified_artifacts WHERE installed_path = ?1",
            [installed_path],
            |row| {
                let kind: String = row.get(2)?;
                let kind = artifact_kind_from_name(&kind).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
                Ok(VerifiedArtifact {
                    key_id: row.get(1)?,
                    manifest: ArtifactManifestV1 {
                        schema_version: crate::ARTIFACT_MANIFEST_SCHEMA_VERSION,
                        artifact_id: row.get(0)?,
                        kind,
                        capability: row.get(3)?,
                        version: row.get(4)?,
                        url: row.get(5)?,
                        sha256: row.get(6)?,
                        byte_size: row.get(7)?,
                        license_url: row.get(8)?,
                        runtime_version: row.get(9)?,
                        context_length: row.get(10)?,
                        minimum_ram_bytes: row.get(11)?,
                    },
                    installed_path: installed_path.to_owned(),
                    signed_manifest_json: row.get(12)?,
                })
            },
        );
        match result {
            Ok(artifact) => Ok(Some(artifact)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(error) => Err(DatabaseError::Sql(error)),
        }
    }
}

fn artifact_kind_name(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Model => "model",
        ArtifactKind::Executable => "executable",
    }
}

fn artifact_kind_from_name(value: &str) -> Result<ArtifactKind, DatabaseError> {
    match value {
        "model" => Ok(ArtifactKind::Model),
        "executable" => Ok(ArtifactKind::Executable),
        _ => Err(DatabaseError::InvalidVerifiedArtifact(format!(
            "unknown artifact kind `{value}`"
        ))),
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
        let verified_artifact_table: String = database
            .connection()
            .query_row(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'verified_artifacts'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(verified_artifact_table, "verified_artifacts");
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
        let artifact = VerifiedArtifact {
            key_id: "release-key-2026-01".into(),
            manifest: ArtifactManifestV1 {
                schema_version: crate::ARTIFACT_MANIFEST_SCHEMA_VERSION,
                artifact_id: "qdrant-linux-x86_64".into(),
                kind: ArtifactKind::Executable,
                capability: "vector_database".into(),
                version: "1.19.1".into(),
                url: "https://releases.example.test/qdrant".into(),
                sha256: "a".repeat(64),
                byte_size: 123_456,
                license_url: "https://releases.example.test/license".into(),
                runtime_version: "linux-x86_64".into(),
                context_length: None,
                minimum_ram_bytes: 1_073_741_824,
            },
            installed_path: "/absolute/path/qdrant".into(),
            signed_manifest_json: "{\"key_id\":\"release-key-2026-01\"}".into(),
        };
        database.record_verified_artifact(&artifact).unwrap();
        assert_eq!(
            database.verified_artifact("qdrant-linux-x86_64").unwrap(),
            Some(artifact)
        );
        assert_eq!(
            database
                .verified_artifact_by_installed_path("/absolute/path/qdrant")
                .unwrap(),
            database.verified_artifact("qdrant-linux-x86_64").unwrap()
        );
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
