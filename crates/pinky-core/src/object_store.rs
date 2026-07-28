use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{Vault, VaultError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionClass {
    Text,
    Binary,
    AlreadyCompressed,
}

impl CompressionClass {
    pub fn for_mime(mime: &str) -> Self {
        if matches!(
            mime,
            "image/png" | "image/jpeg" | "image/webp" | "application/zip" | "application/pdf"
        ) || mime.starts_with("application/vnd.openxmlformats-officedocument")
        {
            Self::AlreadyCompressed
        } else if mime.starts_with("text/")
            || matches!(
                mime,
                "application/json" | "application/xml" | "application/yaml"
            )
        {
            Self::Text
        } else {
            Self::Binary
        }
    }

    fn level(self) -> i32 {
        match self {
            Self::Text => 6,
            Self::Binary => 3,
            Self::AlreadyCompressed => 1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectMetadata {
    pub sha256: String,
    pub mime_type: String,
    pub uncompressed_length: u64,
    pub compressed_length: u64,
    pub compression_level: i32,
}

#[derive(Debug, Clone)]
pub struct StoredObject {
    pub path: PathBuf,
    pub metadata: ObjectMetadata,
    pub deduplicated: bool,
}

#[derive(Debug, Error)]
pub enum ObjectStoreError {
    #[error("vault unavailable: {0}")]
    Vault(#[from] VaultError),
    #[error("object store I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("object metadata error: {0}")]
    Metadata(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct ObjectStore {
    vault: Vault,
}

impl ObjectStore {
    pub fn new(vault: Vault) -> Self {
        Self { vault }
    }

    pub(crate) fn vault(&self) -> &Vault {
        &self.vault
    }

    pub fn put(&self, bytes: &[u8], mime_type: &str) -> Result<StoredObject, ObjectStoreError> {
        self.put_reader(bytes, mime_type)
    }

    pub fn put_reader(
        &self,
        mut reader: impl Read,
        mime_type: &str,
    ) -> Result<StoredObject, ObjectStoreError> {
        self.vault.ensure_mounted()?;
        let temporary_dir = self.vault.resolve_internal("objects/.tmp");
        fs::create_dir_all(&temporary_dir)?;
        let temporary_path = temporary_dir.join(format!("{}.partial", uuid::Uuid::new_v4()));
        let temporary_file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary_path)?;
        let class = CompressionClass::for_mime(mime_type);
        let mut encoder = zstd::Encoder::new(temporary_file, class.level())?;
        let mut hasher = Sha256::new();
        let mut length = 0_u64;
        let mut buffer = [0_u8; 64 * 1024];

        let result = (|| -> Result<(), std::io::Error> {
            loop {
                let count = reader.read(&mut buffer)?;
                if count == 0 {
                    break;
                }
                hasher.update(&buffer[..count]);
                encoder.write_all(&buffer[..count])?;
                length += count as u64;
            }
            Ok(())
        })();
        if let Err(error) = result {
            drop(encoder);
            let _ = fs::remove_file(&temporary_path);
            return Err(error.into());
        }

        let output = encoder.finish()?;
        output.sync_all()?;
        let hash = hex::encode(hasher.finalize());
        self.vault.ensure_mounted()?;
        let directory = self
            .vault
            .resolve_internal(format!("objects/{}", &hash[..2]));
        fs::create_dir_all(&directory)?;
        let path = directory.join(format!("{hash}.zst"));
        let deduplicated = path.exists();
        if deduplicated {
            fs::remove_file(&temporary_path)?;
        } else {
            fs::rename(&temporary_path, &path)?;
        }

        let compressed_length = fs::metadata(&path)?.len();
        let metadata = ObjectMetadata {
            sha256: hash.clone(),
            mime_type: mime_type.to_owned(),
            uncompressed_length: length,
            compressed_length,
            compression_level: class.level(),
        };
        let metadata_path = directory.join(format!("{hash}.meta.json"));
        if !metadata_path.exists() {
            let bytes = serde_json::to_vec(&metadata)?;
            atomic_write(&metadata_path, &bytes)?;
        }
        Ok(StoredObject {
            path,
            metadata,
            deduplicated,
        })
    }

    pub fn read_verified(&self, hash: &str) -> Result<Vec<u8>, ObjectStoreError> {
        self.vault.ensure_mounted()?;
        validate_hash(hash)?;
        let path = self
            .vault
            .resolve_internal(format!("objects/{}/{}.zst", &hash[..2], hash));
        let mut decoder = zstd::Decoder::new(File::open(path)?)?;
        let mut bytes = Vec::new();
        decoder.read_to_end(&mut bytes)?;
        if hex::encode(Sha256::digest(&bytes)) != hash {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "object checksum mismatch",
            )
            .into());
        }
        Ok(bytes)
    }
}

fn validate_hash(hash: &str) -> Result<(), ObjectStoreError> {
    if hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Ok(());
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::InvalidInput,
        "invalid SHA-256 object identifier",
    )
    .into())
}

fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let temporary = path.with_extension("json.partial");
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::MountVerifier;
    use std::path::Path;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    struct Mounted;
    impl MountVerifier for Mounted {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(true)
        }
    }

    #[test]
    fn content_addressing_deduplicates_and_round_trips() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let store = ObjectStore::new(vault);
        let first = store.put(b"retained evidence", "text/plain").unwrap();
        let second = store.put(b"retained evidence", "text/plain").unwrap();
        assert!(!first.deduplicated);
        assert!(second.deduplicated);
        assert_eq!(
            store.read_verified(&first.metadata.sha256).unwrap(),
            b"retained evidence"
        );
        assert_eq!(first.metadata.compression_level, 6);
    }

    struct Switch(Arc<AtomicBool>);
    impl MountVerifier for Switch {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }

    #[test]
    fn refuses_plaintext_fallback_after_unmount() {
        let mounted = Arc::new(AtomicBool::new(true));
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Switch(mounted.clone())).unwrap();
        let store = ObjectStore::new(vault);
        mounted.store(false, Ordering::SeqCst);
        assert!(matches!(
            store.put(b"must not leak", "text/plain"),
            Err(ObjectStoreError::Vault(VaultError::NotEncryptedMount(_)))
        ));
    }
}
