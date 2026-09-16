use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::fd::AsRawFd,
    os::unix::{ffi::OsStringExt, fs::MetadataExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use chrono::Utc;
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    extract_image_metadata, Database, ImageOcrError, ImageOcrWorker, IndexedChunk, ObjectStore,
    ObjectStoreError, RetrievalError, RetrievalService, StoredObject, MAX_IMAGE_METADATA_BYTES,
    MAX_OCR_TEXT_BYTES,
};

const EXTRACTION_VERSION: &str = "pinky-text-v1";
const IMAGE_EXTRACTION_VERSION: &str = "pinky-image-metadata-v1";
const TARGET_TOKENS: usize = 500;
const OVERLAP_TOKENS: usize = 75;
const MAX_TEXT_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum IngestionError {
    #[error("cancelled")]
    Cancelled,
    #[error("approved root must be an existing directory: {0}")]
    InvalidApprovedRoot(PathBuf),
    #[error("source path is outside the approved root")]
    OutsideApprovedRoot,
    #[error("source must be a regular file: {0}")]
    NotRegularFile(PathBuf),
    #[error("source path has no displayable file name")]
    MissingFileName,
    #[error("ingestion database lock is poisoned")]
    DatabaseLock,
    #[error("ingestion I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ingestion object error: {0}")]
    Object(#[from] ObjectStoreError),
    #[error("ingestion database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("ingestion serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("ingestion retrieval index error: {0}")]
    Retrieval(#[from] RetrievalError),
    #[error("image OCR failed: {0}")]
    Ocr(#[from] ImageOcrError),
    #[error("the requested image version is no longer current")]
    StaleImageVersion,
    #[error("OCR returned no text")]
    EmptyOcrOutput,
    #[error("OCR has already been attached to this image version")]
    OcrAlreadyAttached,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IngestedSource {
    pub source_id: Uuid,
    pub version_id: Uuid,
    pub display_name: String,
    pub canonical_uri: String,
    pub mime_type: String,
    pub byte_size: u64,
    pub chunk_count: usize,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceSummary {
    pub source_id: Uuid,
    pub version_id: Uuid,
    pub display_name: String,
    pub canonical_uri: String,
    pub mime_type: String,
    pub byte_size: u64,
    pub chunk_count: usize,
    pub state: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalFileFingerprint {
    pub device: u64,
    pub inode: u64,
    pub byte_size: u64,
    pub modified_seconds: i64,
    pub modified_nanoseconds: i64,
}

impl LocalFileFingerprint {
    fn from_metadata(metadata: &fs::Metadata) -> Self {
        Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            byte_size: metadata.len(),
            modified_seconds: metadata.mtime(),
            modified_nanoseconds: metadata.mtime_nsec(),
        }
    }

    pub fn read(path: &Path) -> Result<Self, std::io::Error> {
        Ok(Self::from_metadata(&fs::metadata(path)?))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocalWatchTarget {
    pub source_id: Uuid,
    pub source_path: PathBuf,
    pub approved_root: PathBuf,
    pub fingerprint: Option<LocalFileFingerprint>,
    pub state: String,
}

#[derive(Clone)]
pub struct LocalIngestor {
    database: Arc<Mutex<Database>>,
    objects: ObjectStore,
}

impl LocalIngestor {
    pub fn new(database: Arc<Mutex<Database>>, objects: ObjectStore) -> Self {
        Self { database, objects }
    }

    pub fn ingest(
        &self,
        approved_root: impl AsRef<Path>,
        source_path: impl AsRef<Path>,
    ) -> Result<IngestedSource, IngestionError> {
        self.ingest_cancellable(approved_root, source_path, CancellationToken::new())
    }

    pub fn ingest_cancellable(
        &self,
        approved_root: impl AsRef<Path>,
        source_path: impl AsRef<Path>,
        cancellation: CancellationToken,
    ) -> Result<IngestedSource, IngestionError> {
        check_cancelled(&cancellation)?;
        let approved_root = fs::canonicalize(approved_root.as_ref())?;
        if !fs::metadata(&approved_root)?.is_dir() {
            return Err(IngestionError::InvalidApprovedRoot(approved_root));
        }
        let source_path = fs::canonicalize(source_path.as_ref())?;
        if !source_path.starts_with(&approved_root) {
            return Err(IngestionError::OutsideApprovedRoot);
        }
        let mut source_file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&source_path)?;
        let opened_path = fs::canonicalize(format!("/proc/self/fd/{}", source_file.as_raw_fd()))?;
        if opened_path != source_path || !opened_path.starts_with(&approved_root) {
            return Err(IngestionError::OutsideApprovedRoot);
        }
        let source_path = opened_path;
        let metadata = source_file.metadata()?;
        if !metadata.is_file() {
            return Err(IngestionError::NotRegularFile(source_path));
        }

        let display_name = source_path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .filter(|name| !name.is_empty())
            .ok_or(IngestionError::MissingFileName)?;
        let canonical_uri = file_uri(&source_path);
        let fingerprint = LocalFileFingerprint::from_metadata(&metadata);
        let mut prefix = [0_u8; 512];
        let prefix_length = source_file.read(&mut prefix)?;
        source_file.seek(SeekFrom::Start(0))?;
        let mime_type = detect_mime(&source_path, &prefix[..prefix_length]);
        let original_result = self.objects.put_reader(
            CancellableReader::new(source_file, cancellation.clone()),
            &mime_type,
        );
        check_cancelled(&cancellation)?;
        let original = original_result?;
        let byte_size = original.metadata.uncompressed_length;

        let extraction = if is_supported_text(&mime_type) && byte_size <= MAX_TEXT_BYTES {
            let bytes = self.objects.read_verified(&original.metadata.sha256)?;
            String::from_utf8(bytes)
                .ok()
                .map(|text| normalize_text(&text))
        } else if is_supported_image(&mime_type) && byte_size <= MAX_IMAGE_METADATA_BYTES as u64 {
            let bytes = self.objects.read_verified(&original.metadata.sha256)?;
            extract_image_metadata(&mime_type, &bytes)
                .ok()
                .and_then(|metadata| metadata.to_retained_json().ok())
        } else {
            None
        };
        let image_metadata_error = if is_supported_image(&mime_type)
            && byte_size <= MAX_IMAGE_METADATA_BYTES as u64
            && extraction.is_none()
        {
            let bytes = self.objects.read_verified(&original.metadata.sha256)?;
            extract_image_metadata(&mime_type, &bytes)
                .err()
                .map(|error| error.to_string())
        } else if is_supported_image(&mime_type) && byte_size > MAX_IMAGE_METADATA_BYTES as u64 {
            Some(format!(
                "image metadata limit exceeded ({byte_size} bytes; maximum {MAX_IMAGE_METADATA_BYTES})"
            ))
        } else {
            None
        };
        let state = if extraction.is_some() {
            "active"
        } else {
            "unsupported"
        };
        let processing_state = if extraction.is_some() {
            "ready"
        } else {
            "unsupported"
        };
        let extraction_error = if extraction.is_some() {
            None
        } else if byte_size > MAX_TEXT_BYTES && is_supported_text(&mime_type) {
            Some(format!(
                "text extraction limit exceeded ({byte_size} bytes; maximum {MAX_TEXT_BYTES})"
            ))
        } else if let Some(error) = image_metadata_error {
            Some(error)
        } else {
            Some("format is archived but not yet extractable".to_owned())
        };

        let extracted_mime = if is_supported_image(&mime_type) {
            "application/json"
        } else {
            "text/plain"
        };
        let extracted = extraction
            .as_ref()
            .map(|text| self.objects.put(text.as_bytes(), extracted_mime))
            .transpose()?;
        let mut chunks = Vec::new();
        if let Some(text) = extraction.as_deref() {
            for chunk in chunk_text(text) {
                check_cancelled(&cancellation)?;
                let stored = self.objects.put(chunk.text.as_bytes(), "text/plain")?;
                chunks.push((chunk, stored));
            }
        }

        check_cancelled(&cancellation)?;
        let source_id = Uuid::new_v4();
        let version_id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let database = self
            .database
            .lock()
            .map_err(|_| IngestionError::DatabaseLock)?;
        let transaction = database.connection().unchecked_transaction()?;
        record_object(&transaction, &original, 1)?;
        if let Some(extracted) = extracted.as_ref() {
            record_object(&transaction, extracted, 1)?;
        }
        for (_, stored) in &chunks {
            check_cancelled(&cancellation)?;
            record_object(&transaction, stored, 1)?;
        }

        let existing: Option<(String, Option<String>)> = transaction
            .query_row(
                "SELECT id, current_version_id FROM sources WHERE canonical_uri = ?1",
                [&canonical_uri],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let (source_id, previous_version) = match existing {
            Some((id, current)) => (
                Uuid::parse_str(&id).map_err(|_| rusqlite::Error::InvalidQuery)?,
                current,
            ),
            None => (source_id, None),
        };
        transaction.execute(
            "INSERT INTO sources (
                id, kind, canonical_uri, display_name, approval_scope, current_version_id,
                refresh_policy, authority_tier, created_at, updated_at, last_checked_at, state
             ) VALUES (?1, 'local_file', ?2, ?3, ?4, NULL, 'filesystem_event', 1, ?5, ?5, ?5, ?6)
             ON CONFLICT(canonical_uri) DO UPDATE SET
                display_name = excluded.display_name,
                approval_scope = excluded.approval_scope,
                updated_at = excluded.updated_at,
                last_checked_at = excluded.last_checked_at",
            params![
                source_id.to_string(),
                canonical_uri,
                display_name,
                approved_root.to_string_lossy(),
                now,
                state,
            ],
        )?;
        transaction.execute(
            "INSERT INTO source_versions (
                id, source_id, original_object_hash, extracted_object_hash, mime_type,
                detected_language, byte_size, extraction_method, extraction_version,
                retrieved_at, selected_headers_json, superseded_version_id, processing_state,
                error, citation_map_json
             ) VALUES (?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                version_id.to_string(),
                source_id.to_string(),
                original.metadata.sha256,
                extracted
                    .as_ref()
                    .map(|object| object.metadata.sha256.as_str()),
                mime_type,
                byte_size,
                extraction.as_ref().map(|_| {
                    if is_supported_image(&mime_type) {
                        "builtin_image_metadata"
                    } else {
                        "builtin_text"
                    }
                }),
                extraction.as_ref().map(|_| {
                    if is_supported_image(&mime_type) {
                        IMAGE_EXTRACTION_VERSION
                    } else {
                        EXTRACTION_VERSION
                    }
                }),
                now,
                serde_json::to_string(&fingerprint)?,
                previous_version,
                processing_state,
                extraction_error,
                "{}",
            ],
        )?;
        let mut indexed_chunks = Vec::with_capacity(chunks.len());
        for (ordinal, (chunk, stored)) in chunks.iter().enumerate() {
            check_cancelled(&cancellation)?;
            let chunk_id = Uuid::new_v4();
            transaction.execute(
                "INSERT INTO chunks (
                    id, source_version_id, ordinal, heading_path, character_start,
                    character_end, byte_start, byte_end, coordinates_json, token_count,
                    extracted_text_hash, embedding_id
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL)",
                params![
                    chunk_id.to_string(),
                    version_id.to_string(),
                    ordinal as i64,
                    chunk.heading,
                    chunk.character_start as i64,
                    chunk.character_end as i64,
                    chunk.byte_start as i64,
                    chunk.byte_end as i64,
                    serde_json::json!({
                        "line_start": chunk.line_start,
                        "line_end": chunk.line_end,
                    })
                    .to_string(),
                    chunk.token_count as i64,
                    stored.metadata.sha256,
                ],
            )?;
            indexed_chunks.push(IndexedChunk {
                chunk_id,
                source_id,
                version_id,
                ordinal: ordinal as u64,
                display_name: display_name.clone(),
                heading: chunk.heading.clone(),
                body: chunk.text.clone(),
            });
        }
        check_cancelled(&cancellation)?;
        RetrievalService::new(self.database.clone(), self.objects.clone())
            .index_version(version_id, &indexed_chunks)?;
        check_cancelled(&cancellation)?;
        transaction.execute(
            "UPDATE sources
             SET current_version_id = ?1, state = ?2, updated_at = ?3, last_checked_at = ?3
             WHERE id = ?4",
            params![version_id.to_string(), state, now, source_id.to_string()],
        )?;
        transaction.commit()?;

        Ok(IngestedSource {
            source_id,
            version_id,
            display_name,
            canonical_uri,
            mime_type,
            byte_size,
            chunk_count: chunks.len(),
            state: state.to_owned(),
        })
    }

    pub fn list_sources(&self) -> Result<Vec<SourceSummary>, IngestionError> {
        let database = self
            .database
            .lock()
            .map_err(|_| IngestionError::DatabaseLock)?;
        let mut statement = database.connection().prepare(
            "SELECT s.id, v.id, s.display_name, s.canonical_uri, v.mime_type, v.byte_size,
                    (SELECT COUNT(*) FROM chunks c WHERE c.source_version_id = v.id),
                    s.state, s.updated_at
             FROM sources s
             JOIN source_versions v ON v.id = s.current_version_id
             WHERE s.kind = 'local_file' AND s.state != 'deleted'
             ORDER BY s.updated_at DESC, s.display_name",
        )?;
        let rows = statement.query_map([], |row| {
            let source_id: String = row.get(0)?;
            let version_id: String = row.get(1)?;
            Ok(SourceSummary {
                source_id: Uuid::parse_str(&source_id)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                version_id: Uuid::parse_str(&version_id)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                display_name: row.get(2)?,
                canonical_uri: row.get(3)?,
                mime_type: row.get(4)?,
                byte_size: row.get::<_, i64>(5)? as u64,
                chunk_count: row.get::<_, i64>(6)? as usize,
                state: row.get(7)?,
                updated_at: row.get(8)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Run the supervised OCR worker for the current retained image version and
    /// attach its bounded text as additional searchable chunks. The original
    /// image and metadata remain unchanged; a stale version cannot be mutated.
    pub async fn ocr_current_image(
        &self,
        source_id: Uuid,
        version_id: Uuid,
        worker: &ImageOcrWorker,
        cancellation: &CancellationToken,
    ) -> Result<usize, IngestionError> {
        check_cancelled(cancellation)?;
        let (current_version, original_hash, mime_type, extracted_hash, extraction_method) = {
            let database = self
                .database
                .lock()
                .map_err(|_| IngestionError::DatabaseLock)?;
            database
                .connection()
                .query_row(
                    "SELECT s.current_version_id, v.original_object_hash, v.mime_type,
                            v.extracted_object_hash, v.extraction_method
                     FROM sources s
                     JOIN source_versions v ON v.id = s.current_version_id
                     WHERE s.id = ?1 AND s.state NOT IN ('deleted', 'missing')",
                    [source_id.to_string()],
                    |row| {
                        Ok((
                            row.get::<_, Option<String>>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, Option<String>>(3)?,
                            row.get::<_, Option<String>>(4)?,
                        ))
                    },
                )
                .optional()?
                .ok_or(IngestionError::StaleImageVersion)?
        };
        if current_version.as_deref() != Some(&version_id.to_string()) {
            return Err(IngestionError::StaleImageVersion);
        }
        if !is_supported_image(&mime_type) {
            return Err(IngestionError::Ocr(ImageOcrError::UnsupportedMime));
        }
        if extraction_method
            .as_deref()
            .is_some_and(|method| method.contains("+ocr"))
        {
            return Err(IngestionError::OcrAlreadyAttached);
        }
        let text = worker
            .recognize(&self.objects, &original_hash, &mime_type, cancellation)
            .await?;
        check_cancelled(cancellation)?;
        let text = normalize_text(&text);
        if text.trim().is_empty() {
            return Err(IngestionError::EmptyOcrOutput);
        }
        if text.len() > MAX_OCR_TEXT_BYTES {
            return Err(IngestionError::Ocr(ImageOcrError::OutputTooLarge));
        }

        let metadata = extracted_hash
            .as_deref()
            .map(|hash| self.objects.read_verified(hash))
            .transpose()?;
        let mut retained = metadata
            .as_deref()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(bytes).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        retained["ocr_text"] = serde_json::Value::String(text.clone());
        retained["ocr_version"] = serde_json::Value::String("pinky-ocr-v1".to_owned());
        let extracted = self.objects.put(
            serde_json::to_string_pretty(&retained)?.as_bytes(),
            "application/json",
        )?;
        let mut chunks = Vec::new();
        for chunk in chunk_text(&text) {
            check_cancelled(cancellation)?;
            let stored = self.objects.put(chunk.text.as_bytes(), "text/plain")?;
            chunks.push((chunk, stored));
        }
        if chunks.is_empty() {
            return Err(IngestionError::EmptyOcrOutput);
        }

        check_cancelled(cancellation)?;
        let now = Utc::now().to_rfc3339();
        let database = self
            .database
            .lock()
            .map_err(|_| IngestionError::DatabaseLock)?;
        let transaction = database.connection().unchecked_transaction()?;
        let current: Option<(String, Option<String>, Option<String>)> = transaction
            .query_row(
                "SELECT s.current_version_id, v.extracted_object_hash, v.extraction_method
                 FROM sources s JOIN source_versions v ON v.id = s.current_version_id
                 WHERE s.id = ?1 AND s.state NOT IN ('deleted', 'missing')",
                [source_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((current_id, previous_extracted, current_method)) = current else {
            return Err(IngestionError::StaleImageVersion);
        };
        if current_id != version_id.to_string() {
            return Err(IngestionError::StaleImageVersion);
        }
        if current_method
            .as_deref()
            .is_some_and(|method| method.contains("+ocr"))
        {
            return Err(IngestionError::OcrAlreadyAttached);
        }
        record_object(&transaction, &extracted, 1)?;
        for (_, stored) in &chunks {
            check_cancelled(cancellation)?;
            record_object(&transaction, stored, 1)?;
        }
        if let Some(previous) = previous_extracted {
            transaction.execute(
                "UPDATE objects SET reference_count = reference_count - 1
                 WHERE sha256 = ?1 AND reference_count > 0",
                [previous],
            )?;
        }
        let ordinal_start: i64 = transaction.query_row(
            "SELECT COALESCE(MAX(ordinal) + 1, 0) FROM chunks WHERE source_version_id = ?1",
            [version_id.to_string()],
            |row| row.get(0),
        )?;
        for (offset, (chunk, stored)) in chunks.iter().enumerate() {
            let chunk_id = Uuid::new_v4();
            transaction.execute(
                "INSERT INTO chunks (
                    id, source_version_id, ordinal, heading_path, character_start,
                    character_end, byte_start, byte_end, coordinates_json, token_count,
                    extracted_text_hash, embedding_id
                 ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7, ?8, ?9, ?10, NULL)",
                params![
                    chunk_id.to_string(),
                    version_id.to_string(),
                    ordinal_start + offset as i64,
                    chunk.character_start as i64,
                    chunk.character_end as i64,
                    chunk.byte_start as i64,
                    chunk.byte_end as i64,
                    serde_json::json!({
                        "ocr": true,
                        "line_start": chunk.line_start,
                        "line_end": chunk.line_end,
                    })
                    .to_string(),
                    chunk.token_count as i64,
                    stored.metadata.sha256,
                ],
            )?;
        }
        transaction.execute(
            "UPDATE source_versions
             SET extracted_object_hash = ?1, extraction_method = 'builtin_image_metadata+ocr',
                 extraction_version = 'pinky-image-metadata-v1+ocr-v1', processing_state = 'ready',
                 error = NULL, retrieved_at = ?2
             WHERE id = ?3",
            params![extracted.metadata.sha256, now, version_id.to_string()],
        )?;
        transaction.execute(
            "UPDATE sources SET updated_at = ?1, last_checked_at = ?1 WHERE id = ?2",
            params![now, source_id.to_string()],
        )?;
        check_cancelled(cancellation)?;
        transaction.commit()?;
        drop(database);

        let indexed = RetrievalService::new(self.database.clone(), self.objects.clone())
            .current_chunks()?
            .into_iter()
            .filter(|chunk| chunk.version_id == version_id)
            .collect::<Vec<_>>();
        RetrievalService::new(self.database.clone(), self.objects.clone())
            .index_version(version_id, &indexed)?;
        Ok(chunks.len())
    }

    pub fn watch_targets(&self) -> Result<Vec<LocalWatchTarget>, IngestionError> {
        let database = self
            .database
            .lock()
            .map_err(|_| IngestionError::DatabaseLock)?;
        let mut statement = database.connection().prepare(
            "SELECT s.id, s.canonical_uri, s.approval_scope, v.selected_headers_json, s.state
             FROM sources s
             JOIN source_versions v ON v.id = s.current_version_id
             WHERE s.kind = 'local_file' AND s.state != 'deleted'
             ORDER BY s.id",
        )?;
        let rows = statement.query_map([], |row| {
            let source_id: String = row.get(0)?;
            let canonical_uri: String = row.get(1)?;
            let fingerprint_json: Option<String> = row.get(3)?;
            let fingerprint = fingerprint_json
                .as_deref()
                .map(serde_json::from_str)
                .transpose()
                .map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?;
            Ok(LocalWatchTarget {
                source_id: Uuid::parse_str(&source_id)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                source_path: path_from_file_uri(&canonical_uri)
                    .map_err(|_| rusqlite::Error::InvalidQuery)?,
                approved_root: PathBuf::from(row.get::<_, String>(2)?),
                fingerprint,
                state: row.get(4)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn mark_missing(&self, source_id: Uuid) -> Result<bool, IngestionError> {
        let database = self
            .database
            .lock()
            .map_err(|_| IngestionError::DatabaseLock)?;
        Ok(database.connection().execute(
            "UPDATE sources
             SET state = 'missing', last_checked_at = ?1, updated_at = ?1
             WHERE id = ?2 AND state != 'deleted' AND state != 'missing'",
            params![Utc::now().to_rfc3339(), source_id.to_string()],
        )? > 0)
    }
}

fn check_cancelled(cancellation: &CancellationToken) -> Result<(), IngestionError> {
    if cancellation.is_cancelled() {
        Err(IngestionError::Cancelled)
    } else {
        Ok(())
    }
}

struct CancellableReader<R> {
    inner: R,
    cancellation: CancellationToken,
}

impl<R> CancellableReader<R> {
    fn new(inner: R, cancellation: CancellationToken) -> Self {
        Self {
            inner,
            cancellation,
        }
    }
}

impl<R: Read> Read for CancellableReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, std::io::Error> {
        if self.cancellation.is_cancelled() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "ingestion cancelled",
            ));
        }
        self.inner.read(buffer)
    }
}

fn record_object(
    transaction: &Transaction<'_>,
    object: &StoredObject,
    references: i64,
) -> Result<(), rusqlite::Error> {
    transaction.execute(
        "INSERT INTO objects (
            sha256, uncompressed_length, compressed_length, mime_type, compression_level
         ) VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(sha256) DO NOTHING",
        params![
            object.metadata.sha256,
            object.metadata.uncompressed_length,
            object.metadata.compressed_length,
            object.metadata.mime_type,
            object.metadata.compression_level,
        ],
    )?;
    transaction.execute(
        "UPDATE objects SET reference_count = reference_count + ?1 WHERE sha256 = ?2",
        params![references, object.metadata.sha256],
    )?;
    Ok(())
}

fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;

    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            uri.push(*byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

fn path_from_file_uri(uri: &str) -> Result<PathBuf, IngestionError> {
    let encoded = uri
        .strip_prefix("file://")
        .ok_or(IngestionError::OutsideApprovedRoot)?;
    let bytes = encoded.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let pair = bytes
                .get(index + 1..index + 3)
                .ok_or(IngestionError::OutsideApprovedRoot)?;
            let pair =
                std::str::from_utf8(pair).map_err(|_| IngestionError::OutsideApprovedRoot)?;
            decoded.push(
                u8::from_str_radix(pair, 16).map_err(|_| IngestionError::OutsideApprovedRoot)?,
            );
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    Ok(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
}

fn detect_mime(path: &Path, prefix: &[u8]) -> String {
    let signature = if prefix.starts_with(b"%PDF-") {
        Some("application/pdf")
    } else if prefix.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if prefix.starts_with(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if prefix.starts_with(b"GIF87a") || prefix.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if prefix.starts_with(b"RIFF") && prefix.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else if prefix.starts_with(b"II*\0") || prefix.starts_with(b"MM\0*") {
        Some("image/tiff")
    } else if prefix.starts_with(b"PK\x03\x04") {
        Some("application/zip")
    } else {
        None
    };
    if let Some(signature) = signature {
        return signature.to_owned();
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "md" | "markdown" => "text/markdown",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "yaml" | "yml" => "application/yaml",
        "xml" => "application/xml",
        "csv" => "text/csv",
        "rs" => "text/x-rust",
        "js" | "mjs" | "cjs" | "ts" | "tsx" | "jsx" => "text/javascript",
        "py" => "text/x-python",
        "sh" | "bash" => "text/x-shellscript",
        "txt" | "log" => "text/plain",
        _ if !prefix.contains(&0) && std::str::from_utf8(prefix).is_ok() => "text/plain",
        _ => "application/octet-stream",
    }
    .to_owned()
}

fn is_supported_text(mime_type: &str) -> bool {
    mime_type.starts_with("text/")
        || matches!(
            mime_type,
            "application/json" | "application/yaml" | "application/xml"
        )
}

fn is_supported_image(mime_type: &str) -> bool {
    matches!(
        mime_type,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/tiff"
    )
}

fn normalize_text(text: &str) -> String {
    text.strip_prefix('\u{feff}')
        .unwrap_or(text)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

struct TextChunk {
    text: String,
    heading: Option<String>,
    character_start: usize,
    character_end: usize,
    byte_start: usize,
    byte_end: usize,
    line_start: usize,
    line_end: usize,
    token_count: usize,
}

fn chunk_text(text: &str) -> Vec<TextChunk> {
    let mut tokens = Vec::new();
    let mut start = None;
    for (index, character) in text.char_indices() {
        if character.is_whitespace() {
            if let Some(token_start) = start.take() {
                tokens.push((token_start, index));
            }
        } else if start.is_none() {
            start = Some(index);
        }
    }
    if let Some(token_start) = start {
        tokens.push((token_start, text.len()));
    }
    if tokens.is_empty() {
        return Vec::new();
    }

    let mut chunks = Vec::new();
    let mut token_start = 0;
    while token_start < tokens.len() {
        let token_end = (token_start + TARGET_TOKENS).min(tokens.len());
        let byte_start = tokens[token_start].0;
        let byte_end = tokens[token_end - 1].1;
        let preceding = &text[..byte_start];
        let heading = preceding
            .lines()
            .rev()
            .find(|line| line.trim_start().starts_with('#'))
            .map(|line| line.trim().trim_start_matches('#').trim().to_owned())
            .filter(|heading| !heading.is_empty());
        chunks.push(TextChunk {
            text: text[byte_start..byte_end].to_owned(),
            heading,
            character_start: text[..byte_start].chars().count(),
            character_end: text[..byte_end].chars().count(),
            byte_start,
            byte_end,
            line_start: preceding.bytes().filter(|byte| *byte == b'\n').count() + 1,
            line_end: text[..byte_end]
                .bytes()
                .filter(|byte| *byte == b'\n')
                .count()
                + 1,
            token_count: token_end - token_start,
        });
        if token_end == tokens.len() {
            break;
        }
        token_start = token_end - OVERLAP_TOKENS;
    }
    chunks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ImageOcrWorker, MountVerifier, Vault};
    use std::fs::File;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    struct Mounted;
    impl MountVerifier for Mounted {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(true)
        }
    }

    fn ingestor() -> (tempfile::TempDir, tempfile::TempDir, LocalIngestor) {
        let vault_root = tempfile::tempdir().unwrap();
        let approved_root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, zeroize::Zeroizing::new(vec![0x51; 32])).unwrap(),
        ));
        let ingestor = LocalIngestor::new(database, ObjectStore::new(vault));
        (vault_root, approved_root, ingestor)
    }

    #[test]
    fn archives_extracts_chunks_and_versions_an_approved_text_file() {
        let (_vault, approved, ingestor) = ingestor();
        let source = approved.path().join("notes.md");
        let mut file = File::create(&source).unwrap();
        writeln!(file, "# Evidence\n\nPinky retains this statement.").unwrap();

        let first = ingestor.ingest(approved.path(), &source).unwrap();
        assert_eq!(first.state, "active");
        assert_eq!(first.chunk_count, 1);
        assert_eq!(ingestor.list_sources().unwrap().len(), 1);

        fs::write(&source, "# Evidence\n\nPinky retains a newer statement.\n").unwrap();
        let second = ingestor.ingest(approved.path(), &source).unwrap();
        assert_eq!(second.source_id, first.source_id);
        assert_ne!(second.version_id, first.version_id);
        let database = ingestor.database.lock().unwrap();
        let versions: i64 = database
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM source_versions WHERE source_id = ?1",
                [first.source_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(versions, 2);
    }

    #[test]
    fn rejects_a_symlink_escape_from_the_approved_root() {
        let (_vault, approved, ingestor) = ingestor();
        let outside = tempfile::NamedTempFile::new().unwrap();
        std::os::unix::fs::symlink(outside.path(), approved.path().join("escape.txt")).unwrap();
        assert!(matches!(
            ingestor.ingest(approved.path(), approved.path().join("escape.txt")),
            Err(IngestionError::OutsideApprovedRoot)
        ));
    }

    #[test]
    fn archives_unsupported_binary_sources_without_chunks() {
        let (_vault, approved, ingestor) = ingestor();
        let source = approved.path().join("sample.pdf");
        fs::write(&source, b"%PDF-1.7\0fixture").unwrap();
        let ingested = ingestor.ingest(approved.path(), &source).unwrap();
        assert_eq!(ingested.state, "unsupported");
        assert_eq!(ingested.chunk_count, 0);
        assert_eq!(ingestor.list_sources().unwrap()[0].state, "unsupported");
    }

    #[test]
    fn retains_image_metadata_as_encrypted_searchable_extraction() {
        let (_vault, approved, ingestor) = ingestor();
        let source = approved.path().join("diagram.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&[0, 0, 4, 0, 0, 0, 3, 0, 8, 6, 0, 0, 0, 0]);
        fs::write(&source, bytes).unwrap();

        let ingested = ingestor.ingest(approved.path(), &source).unwrap();
        assert_eq!(ingested.mime_type, "image/png");
        assert_eq!(ingested.state, "active");
        assert_eq!(ingested.chunk_count, 1);

        let database = ingestor.database.lock().unwrap();
        let extracted_hash: String = database
            .connection()
            .query_row(
                "SELECT extracted_object_hash FROM source_versions WHERE id = ?1",
                [ingested.version_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        drop(database);
        let retained =
            String::from_utf8(ingestor.objects.read_verified(&extracted_hash).unwrap()).unwrap();
        assert!(retained.contains("\"format\": \"PNG\""));
        assert!(retained.contains("\"width\": 1024"));
        assert!(retained.contains("\"height\": 768"));
        assert_eq!(ingestor.list_sources().unwrap()[0].state, "active");
        let retrieval = RetrievalService::new(ingestor.database.clone(), ingestor.objects.clone());
        let hits = retrieval.search("1024", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert!(hits[0].passage.contains("\"format\": \"PNG\""));
    }

    #[tokio::test]
    async fn attaches_bounded_ocr_as_searchable_current_version_chunks() {
        let (vault_root, approved, ingestor) = ingestor();
        let source = approved.path().join("scan.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&[0, 0, 2, 0, 0, 0, 2, 0, 8, 6, 0, 0, 0, 0]);
        fs::write(&source, bytes).unwrap();
        let retained = ingestor.ingest(approved.path(), &source).unwrap();

        let executable = vault_root.path().join("fake-ocr.sh");
        fs::write(
            &executable,
            "#!/bin/sh\nprintf 'invoice total 42\\n' > \"$2.txt\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let worker = ImageOcrWorker::new(&executable, "eng").unwrap();
        let chunks = ingestor
            .ocr_current_image(
                retained.source_id,
                retained.version_id,
                &worker,
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        assert_eq!(chunks, 1);

        let retrieval = RetrievalService::new(ingestor.database.clone(), ingestor.objects.clone());
        let hits = retrieval.search("invoice", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].coordinates["ocr"].as_bool(), Some(true));
        assert!(hits[0]
            .citation_uri
            .contains(&retained.version_id.to_string()));
        let duplicate = ingestor
            .ocr_current_image(
                retained.source_id,
                retained.version_id,
                &worker,
                &CancellationToken::new(),
            )
            .await;
        assert!(matches!(duplicate, Err(IngestionError::OcrAlreadyAttached)));
    }

    #[test]
    fn deduplicates_retained_bytes_across_distinct_sources() {
        let (_vault, approved, ingestor) = ingestor();
        fs::write(approved.path().join("first.txt"), "shared evidence").unwrap();
        fs::write(approved.path().join("second.txt"), "shared evidence").unwrap();
        ingestor
            .ingest(approved.path(), approved.path().join("first.txt"))
            .unwrap();
        ingestor
            .ingest(approved.path(), approved.path().join("second.txt"))
            .unwrap();

        let database = ingestor.database.lock().unwrap();
        let (objects, references): (i64, i64) = database
            .connection()
            .query_row(
                "SELECT COUNT(*), SUM(reference_count) FROM objects",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(objects, 1);
        assert_eq!(references, 6);
    }

    #[test]
    fn chunks_at_five_hundred_tokens_with_seventy_five_token_overlap() {
        let text = (0..1_100)
            .map(|index| format!("token{index}"))
            .collect::<Vec<_>>()
            .join(" ");
        let chunks = chunk_text(&text);
        assert_eq!(
            chunks
                .iter()
                .map(|chunk| chunk.token_count)
                .collect::<Vec<_>>(),
            [500, 500, 250]
        );
        let first_tokens = chunks[0].text.split_whitespace().collect::<Vec<_>>();
        let second_tokens = chunks[1].text.split_whitespace().collect::<Vec<_>>();
        assert_eq!(&first_tokens[425..], &second_tokens[..75]);
    }

    #[test]
    fn cancellation_refuses_to_create_source_metadata() {
        let (_vault, approved, ingestor) = ingestor();
        let source = approved.path().join("cancelled.txt");
        fs::write(&source, "this must not become a retained source").unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert!(matches!(
            ingestor.ingest_cancellable(approved.path(), &source, cancellation),
            Err(IngestionError::Cancelled)
        ));
        assert!(ingestor.list_sources().unwrap().is_empty());
    }

    #[test]
    fn cancelled_replacement_preserves_the_previous_current_version() {
        let (_vault, approved, ingestor) = ingestor();
        let source = approved.path().join("durable.txt");
        fs::write(&source, "known good version").unwrap();
        let first = ingestor.ingest(approved.path(), &source).unwrap();
        fs::write(&source, "replacement that must not become current").unwrap();
        let cancellation = CancellationToken::new();
        cancellation.cancel();

        assert!(matches!(
            ingestor.ingest_cancellable(approved.path(), &source, cancellation),
            Err(IngestionError::Cancelled)
        ));
        let summary = ingestor.list_sources().unwrap().remove(0);
        assert_eq!(summary.version_id, first.version_id);
        assert_eq!(summary.state, "active");
    }

    #[test]
    fn persists_watch_fingerprints_and_marks_deleted_sources_missing() {
        let (_vault, approved, ingestor) = ingestor();
        let source = approved.path().join("watched notes.txt");
        fs::write(&source, "first retained version").unwrap();
        let retained = ingestor.ingest(approved.path(), &source).unwrap();

        let targets = ingestor.watch_targets().unwrap();
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].source_path, source);
        assert_eq!(targets[0].approved_root, approved.path());
        assert_eq!(
            targets[0].fingerprint,
            Some(LocalFileFingerprint::read(&source).unwrap())
        );

        fs::remove_file(&source).unwrap();
        assert!(ingestor.mark_missing(retained.source_id).unwrap());
        let summary = ingestor.list_sources().unwrap().remove(0);
        assert_eq!(summary.state, "missing");
        assert_eq!(summary.version_id, retained.version_id);
        assert!(!ingestor.mark_missing(retained.source_id).unwrap());
    }
}
