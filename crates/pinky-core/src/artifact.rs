use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use ring::signature::{self, UnparsedPublicKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

pub const ARTIFACT_MANIFEST_SCHEMA_VERSION: u16 = 1;
const SIGNING_CONTEXT: &[u8] = b"pinky-artifact-manifest-v1\0";
const COPY_BUFFER_BYTES: usize = 64 * 1024;

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("artifact manifest JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("artifact manifest schema version {0} is unsupported")]
    UnsupportedSchema(u16),
    #[error("artifact manifest field `{0}` must not be empty")]
    EmptyField(&'static str),
    #[error("artifact manifest field `{0}` contains whitespace")]
    WhitespaceField(&'static str),
    #[error("artifact manifest URL `{field}` must be an HTTPS URL")]
    InsecureUrl { field: &'static str },
    #[error("artifact manifest URL `{field}` is invalid")]
    InvalidUrl { field: &'static str },
    #[error("artifact manifest SHA-256 must be 64 lowercase hexadecimal characters")]
    InvalidSha256,
    #[error("artifact manifest byte size must be greater than zero")]
    InvalidByteSize,
    #[error("artifact manifest context length must be greater than zero")]
    InvalidContextLength,
    #[error("artifact manifest minimum RAM must be greater than zero")]
    InvalidMinimumRam,
    #[error("artifact manifest signature is not valid base64")]
    InvalidSignatureEncoding,
    #[error("artifact manifest signature could not be verified")]
    InvalidSignature,
    #[error("artifact file is not a regular file: {0}")]
    NotRegularFile(PathBuf),
    #[error("artifact file size is {found} bytes, expected {expected}")]
    SizeMismatch { found: u64, expected: u64 },
    #[error("artifact file SHA-256 is {found}, expected {expected}")]
    DigestMismatch { found: String, expected: String },
    #[error("artifact destination must be an absolute path: {0}")]
    InvalidDestination(PathBuf),
    #[error("artifact destination already exists: {0}")]
    DestinationExists(PathBuf),
    #[error("artifact I/O error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Model,
    Executable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ArtifactManifestV1 {
    pub schema_version: u16,
    pub artifact_id: String,
    pub kind: ArtifactKind,
    pub capability: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub byte_size: u64,
    pub license_url: String,
    pub runtime_version: String,
    pub context_length: Option<u64>,
    pub minimum_ram_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SignedArtifactManifestV1 {
    pub key_id: String,
    pub manifest: ArtifactManifestV1,
    pub signature: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactDigest {
    pub sha256: String,
    pub byte_size: u64,
}

impl ArtifactManifestV1 {
    pub fn validate(&self) -> Result<(), ArtifactError> {
        if self.schema_version != ARTIFACT_MANIFEST_SCHEMA_VERSION {
            return Err(ArtifactError::UnsupportedSchema(self.schema_version));
        }
        non_empty_without_whitespace(&self.artifact_id, "artifact_id")?;
        non_empty_without_whitespace(&self.capability, "capability")?;
        non_empty_without_whitespace(&self.version, "version")?;
        non_empty_without_whitespace(&self.runtime_version, "runtime_version")?;
        validate_https_url(&self.url, "url")?;
        validate_https_url(&self.license_url, "license_url")?;
        if self.sha256.len() != 64
            || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || self.sha256.bytes().any(|byte| byte.is_ascii_uppercase())
        {
            return Err(ArtifactError::InvalidSha256);
        }
        if self.byte_size == 0 {
            return Err(ArtifactError::InvalidByteSize);
        }
        if self.context_length.is_some_and(|length| length == 0) {
            return Err(ArtifactError::InvalidContextLength);
        }
        if self.minimum_ram_bytes == 0 {
            return Err(ArtifactError::InvalidMinimumRam);
        }
        Ok(())
    }

    pub fn verify_bytes(&self, bytes: &[u8]) -> Result<ArtifactDigest, ArtifactError> {
        self.validate()?;
        let digest = digest_bytes(bytes);
        self.verify_digest(&digest)
    }

    pub fn verify_file(&self, path: impl AsRef<Path>) -> Result<ArtifactDigest, ArtifactError> {
        self.validate()?;
        let path = path.as_ref();
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.file_type().is_file() {
            return Err(ArtifactError::NotRegularFile(path.to_owned()));
        }
        let mut file = File::open(path)?;
        let digest = digest_reader(&mut file, Some(self.byte_size))?;
        self.verify_digest(&digest)
    }

    pub fn install_verified_file(
        &self,
        source: impl AsRef<Path>,
        destination: impl AsRef<Path>,
    ) -> Result<ArtifactDigest, ArtifactError> {
        self.validate()?;
        let source = source.as_ref();
        let destination = destination.as_ref();
        if !destination.is_absolute() {
            return Err(ArtifactError::InvalidDestination(destination.to_owned()));
        }
        if fs::symlink_metadata(destination).is_ok() {
            return Err(ArtifactError::DestinationExists(destination.to_owned()));
        }
        let parent = destination
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or_else(|| ArtifactError::InvalidDestination(destination.to_owned()))?;
        let parent_metadata = fs::symlink_metadata(parent)?;
        if !parent_metadata.is_dir() || parent_metadata.file_type().is_symlink() {
            return Err(ArtifactError::InvalidDestination(parent.to_owned()));
        }
        let source_metadata = fs::symlink_metadata(source)?;
        if !source_metadata.file_type().is_file() {
            return Err(ArtifactError::NotRegularFile(source.to_owned()));
        }

        let temporary = parent.join(format!(".pinky-artifact-{}.partial", Uuid::new_v4()));
        let result = (|| {
            let mut input = File::open(source)?;
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            copy_bounded(&mut input, &mut output, self.byte_size)?;
            output.sync_all()?;
            drop(output);

            let digest = self.verify_file(&temporary)?;
            if fs::symlink_metadata(destination).is_ok() {
                return Err(ArtifactError::DestinationExists(destination.to_owned()));
            }
            fs::rename(&temporary, destination)?;
            if let Ok(directory) = File::open(parent) {
                let _ = directory.sync_all();
            }
            Ok(digest)
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    fn verify_digest(&self, digest: &ArtifactDigest) -> Result<ArtifactDigest, ArtifactError> {
        if digest.byte_size != self.byte_size {
            return Err(ArtifactError::SizeMismatch {
                found: digest.byte_size,
                expected: self.byte_size,
            });
        }
        if digest.sha256 != self.sha256 {
            return Err(ArtifactError::DigestMismatch {
                found: digest.sha256.clone(),
                expected: self.sha256.clone(),
            });
        }
        Ok(digest.clone())
    }
}

impl SignedArtifactManifestV1 {
    pub fn from_json(bytes: &[u8]) -> Result<Self, ArtifactError> {
        let signed: Self = serde_json::from_slice(bytes)?;
        signed.validate()?;
        Ok(signed)
    }

    pub fn validate(&self) -> Result<(), ArtifactError> {
        non_empty_without_whitespace(&self.key_id, "key_id")?;
        if self.signature.trim().is_empty() {
            return Err(ArtifactError::EmptyField("signature"));
        }
        self.manifest.validate()
    }

    pub fn verify(&self, trusted_public_key: &[u8]) -> Result<(), ArtifactError> {
        self.validate()?;
        let signature = STANDARD
            .decode(self.signature.as_bytes())
            .map_err(|_| ArtifactError::InvalidSignatureEncoding)?;
        let payload = self.canonical_payload()?;
        UnparsedPublicKey::new(&signature::ED25519, trusted_public_key)
            .verify(&payload, &signature)
            .map_err(|_| ArtifactError::InvalidSignature)
    }

    pub fn canonical_payload(&self) -> Result<Vec<u8>, ArtifactError> {
        self.manifest.validate()?;
        let mut payload = SIGNING_CONTEXT.to_vec();
        payload.extend(serde_json::to_vec(&self.manifest)?);
        Ok(payload)
    }
}

fn non_empty_without_whitespace(value: &str, field: &'static str) -> Result<(), ArtifactError> {
    if value.is_empty() {
        return Err(ArtifactError::EmptyField(field));
    }
    if value.chars().any(char::is_whitespace) {
        return Err(ArtifactError::WhitespaceField(field));
    }
    Ok(())
}

fn validate_https_url(value: &str, field: &'static str) -> Result<(), ArtifactError> {
    let url = Url::parse(value).map_err(|_| ArtifactError::InvalidUrl { field })?;
    if url.scheme() != "https" {
        return Err(ArtifactError::InsecureUrl { field });
    }
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || value.chars().any(char::is_whitespace)
    {
        return Err(ArtifactError::InvalidUrl { field });
    }
    Ok(())
}

fn digest_bytes(bytes: &[u8]) -> ArtifactDigest {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    ArtifactDigest {
        sha256: hex::encode(hasher.finalize()),
        byte_size: bytes.len() as u64,
    }
}

fn digest_reader(
    reader: &mut File,
    maximum_size: Option<u64>,
) -> Result<ArtifactDigest, ArtifactError> {
    let mut hasher = Sha256::new();
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        bytes = bytes.saturating_add(read as u64);
        if maximum_size.is_some_and(|limit| bytes > limit) {
            return Err(ArtifactError::SizeMismatch {
                found: bytes,
                expected: maximum_size.unwrap_or_default(),
            });
        }
        hasher.update(&buffer[..read]);
    }
    Ok(ArtifactDigest {
        sha256: hex::encode(hasher.finalize()),
        byte_size: bytes,
    })
}

fn copy_bounded(
    reader: &mut File,
    writer: &mut File,
    maximum_size: u64,
) -> Result<(), ArtifactError> {
    let mut bytes = 0_u64;
    let mut buffer = [0_u8; COPY_BUFFER_BYTES];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            return Ok(());
        }
        bytes = bytes.saturating_add(read as u64);
        if bytes > maximum_size {
            return Err(ArtifactError::SizeMismatch {
                found: bytes,
                expected: maximum_size,
            });
        }
        writer.write_all(&buffer[..read])?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{Ed25519KeyPair, KeyPair};

    fn manifest(bytes: &[u8]) -> ArtifactManifestV1 {
        let digest = digest_bytes(bytes);
        ArtifactManifestV1 {
            schema_version: ARTIFACT_MANIFEST_SCHEMA_VERSION,
            artifact_id: "nomic-embed-text-q8".into(),
            kind: ArtifactKind::Model,
            capability: "embeddings".into(),
            version: "1.5.0".into(),
            url: "https://models.example.test/nomic-embed-text.gguf".into(),
            sha256: digest.sha256,
            byte_size: digest.byte_size,
            license_url: "https://models.example.test/license".into(),
            runtime_version: "ollama-0.9".into(),
            context_length: Some(8192),
            minimum_ram_bytes: 1024 * 1024 * 1024,
        }
    }

    fn signed(manifest: ArtifactManifestV1, key_pair: &Ed25519KeyPair) -> SignedArtifactManifestV1 {
        let mut signed = SignedArtifactManifestV1 {
            key_id: "pinky-test-key".into(),
            manifest,
            signature: String::new(),
        };
        signed.signature = STANDARD.encode(key_pair.sign(&signed.canonical_payload().unwrap()));
        signed
    }

    #[test]
    fn verifies_a_signed_manifest_and_rejects_tampering() {
        let key_pair = Ed25519KeyPair::from_seed_unchecked(&[7_u8; 32]).unwrap();
        let signed = signed(manifest(b"model bytes"), &key_pair);
        signed.verify(key_pair.public_key().as_ref()).unwrap();

        let mut tampered = signed.clone();
        tampered.manifest.version = "9.9.9".into();
        assert!(matches!(
            tampered.verify(key_pair.public_key().as_ref()),
            Err(ArtifactError::InvalidSignature)
        ));
    }

    #[test]
    fn rejects_invalid_metadata_before_signature_work() {
        let mut manifest = manifest(b"model bytes");
        manifest.url = "http://models.example.test/model.gguf".into();
        assert!(matches!(
            manifest.validate(),
            Err(ArtifactError::InsecureUrl { field: "url" })
        ));
        manifest.url = "https://models.example.test/model.gguf".into();
        manifest.sha256 = "A".repeat(64);
        assert!(matches!(
            manifest.validate(),
            Err(ArtifactError::InvalidSha256)
        ));
    }

    #[test]
    fn verifies_bytes_and_installs_atomically() {
        let bytes = b"verified qdrant executable";
        let manifest = manifest(bytes);
        assert_eq!(
            manifest.verify_bytes(bytes).unwrap().byte_size,
            bytes.len() as u64
        );
        assert!(matches!(
            manifest.verify_bytes(b"tampered"),
            Err(ArtifactError::SizeMismatch { .. }) | Err(ArtifactError::DigestMismatch { .. })
        ));

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("download.partial");
        let destination = root.path().join("model.gguf");
        fs::write(&source, bytes).unwrap();
        manifest
            .install_verified_file(&source, &destination)
            .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), bytes);
        assert!(fs::read_dir(root.path()).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            !(name.starts_with('.') && name.ends_with(".partial"))
        }));
        assert!(matches!(
            manifest.install_verified_file(&source, &destination),
            Err(ArtifactError::DestinationExists(_))
        ));
    }

    #[test]
    fn failed_install_removes_partial_output() {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("download.partial");
        let destination = root.path().join("model.gguf");
        fs::write(&source, b"wrong bytes").unwrap();
        let manifest = manifest(b"expected bytes");
        assert!(manifest
            .install_verified_file(&source, &destination)
            .is_err());
        assert!(!destination.exists());
        assert!(fs::read_dir(root.path()).unwrap().all(|entry| {
            let name = entry.unwrap().file_name();
            let name = name.to_string_lossy();
            !(name.starts_with('.') && name.ends_with(".partial"))
        }));
    }
}
