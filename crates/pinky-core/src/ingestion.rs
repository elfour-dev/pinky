use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom},
    os::fd::AsRawFd,
    os::unix::{ffi::OsStringExt, fs::MetadataExt, fs::OpenOptionsExt},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use chrono::Utc;
use rusqlite::{params, params_from_iter, types::Value, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    extract_image_metadata, Database, ImageOcrError, ImageOcrWorker, IndexedChunk, ObjectStore,
    ObjectStoreError, PdfError, PdfTextExtractor, RetrievalError, RetrievalService, StoredObject,
    MAX_IMAGE_METADATA_BYTES, MAX_OCR_TEXT_BYTES, PDF_EXTRACTION_VERSION,
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
    #[error("PDF extraction failed: {0}")]
    Pdf(#[from] PdfError),
    #[error("the requested image version is no longer current")]
    StaleImageVersion,
    #[error("OCR returned no text")]
    EmptyOcrOutput,
    #[error("retained image metadata is not a JSON object")]
    InvalidImageMetadata,
    #[error("the requested OCR detection no longer exists")]
    OcrChunkNotFound,
    #[error("retained OCR chunk is not valid UTF-8")]
    InvalidOcrChunkText,
    #[error("invalid asset {field}: {value}")]
    InvalidAssetFilter { field: &'static str, value: String },
    #[error("invalid asset cursor")]
    InvalidAssetCursor,
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
    pub kind: String,
    pub display_name: String,
    pub canonical_uri: String,
    pub mime_type: String,
    pub byte_size: u64,
    pub chunk_count: usize,
    pub state: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetListQuery {
    pub search: Option<String>,
    pub kind: Option<String>,
    pub state: Option<String>,
    pub sort: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AssetListResponse {
    pub items: Vec<SourceSummary>,
    pub total: usize,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AssetCursor {
    sort: String,
    primary: String,
    secondary: String,
    source_id: String,
}

const MAX_ASSET_SEARCH_BYTES: usize = 256;
const MAX_ASSET_PAGE_SIZE: usize = 100;

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
    pdf_extractor: Option<Arc<PdfTextExtractor>>,
}

impl LocalIngestor {
    pub fn new(database: Arc<Mutex<Database>>, objects: ObjectStore) -> Self {
        Self {
            database,
            objects,
            pdf_extractor: None,
        }
    }

    pub fn with_pdf_extractor(mut self, extractor: PdfTextExtractor) -> Self {
        self.pdf_extractor = Some(Arc::new(extractor));
        self
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

        let mut pdf_extraction_error = None;
        let mut pdf_ocr_pages = Vec::new();
        let extraction = if mime_type == "application/pdf" {
            match self.pdf_extractor.as_ref() {
                Some(extractor) => match extractor.extract_blocking_detailed(
                    &self.objects,
                    &original.metadata.sha256,
                    &cancellation,
                ) {
                    Ok(details) if !details.text.trim().is_empty() => {
                        pdf_ocr_pages = details.ocr_pages;
                        Some(details.text)
                    }
                    Ok(_) => None,
                    Err(PdfError::Cancelled) => return Err(IngestionError::Cancelled),
                    Err(error) => {
                        pdf_extraction_error = Some(error.to_string());
                        None
                    }
                },
                None => None,
            }
        } else if is_supported_text(&mime_type) && byte_size <= MAX_TEXT_BYTES {
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
        } else if let Some(error) = pdf_extraction_error {
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
            let text_chunks = if mime_type == "application/pdf" {
                chunk_pdf_text(text)
            } else {
                chunk_text(text)
            };
            for chunk in text_chunks {
                check_cancelled(&cancellation)?;
                let stored = self.objects.put(chunk.text.as_bytes(), "text/plain")?;
                chunks.push((chunk, stored));
            }
        }

        check_cancelled(&cancellation)?;
        let source_id = Uuid::new_v4();
        let version_id = Uuid::new_v4();
        let now = Utc::now().to_rfc3339();
        let extraction_method = extraction.as_ref().map(|_| {
            if is_supported_image(&mime_type) {
                "builtin_image_metadata"
            } else if mime_type == "application/pdf" {
                if pdf_ocr_pages.is_empty() {
                    "builtin_pdf_text"
                } else {
                    "builtin_pdf_text_ocr"
                }
            } else {
                "builtin_text"
            }
        });
        let extraction_version = extraction.as_ref().map(|_| {
            if is_supported_image(&mime_type) {
                IMAGE_EXTRACTION_VERSION
            } else if mime_type == "application/pdf" {
                PDF_EXTRACTION_VERSION
            } else {
                EXTRACTION_VERSION
            }
        });
        let citation_map_json = if !pdf_ocr_pages.is_empty() {
            serde_json::json!({"ocr_pages": pdf_ocr_pages}).to_string()
        } else {
            "{}".to_owned()
        };
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
                extraction_method,
                extraction_version,
                now,
                serde_json::to_string(&fingerprint)?,
                previous_version.as_ref().filter(|_| extraction.is_some()),
                processing_state,
                extraction_error,
                citation_map_json,
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
                        "image_metadata": is_supported_image(&mime_type),
                        "page": chunk.page,
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
        if extraction.is_some() || previous_version.is_none() {
            transaction.execute(
                "UPDATE sources
                 SET current_version_id = ?1, state = ?2, updated_at = ?3, last_checked_at = ?3
                 WHERE id = ?4",
                params![version_id.to_string(), state, now, source_id.to_string()],
            )?;
        }
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
            "SELECT s.id, v.id, s.kind, s.display_name, s.canonical_uri, v.mime_type, v.byte_size,
                    (SELECT COUNT(*) FROM chunks c WHERE c.source_version_id = v.id),
                    s.state, s.updated_at
             FROM sources s
             JOIN source_versions v ON v.id = s.current_version_id
             WHERE s.kind = 'local_file' AND s.state != 'deleted'
             ORDER BY s.updated_at DESC, s.display_name",
        )?;
        let rows = statement.query_map([], source_summary_from_row)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Return a bounded, filterable asset page. The cursor is opaque to the
    /// UI and keeps the query on a stable keyset rather than using OFFSET.
    pub fn list_assets(&self, query: AssetListQuery) -> Result<AssetListResponse, IngestionError> {
        let search = query.search.unwrap_or_default();
        if search.len() > MAX_ASSET_SEARCH_BYTES {
            return Err(IngestionError::InvalidAssetFilter {
                field: "search",
                value: "search text is too long".to_owned(),
            });
        }
        let kind = validate_asset_filter(
            "kind",
            query.kind,
            &["local_file", "web_page", "search_result", "generated"],
        )?;
        let state = validate_asset_filter(
            "state",
            query.state,
            &["active", "missing", "blocked", "unsupported"],
        )?;
        let sort = query.sort.as_deref().unwrap_or("updated");
        if !["updated", "name", "size", "type"].contains(&sort) {
            return Err(IngestionError::InvalidAssetFilter {
                field: "sort",
                value: sort.to_owned(),
            });
        }
        let cursor = query
            .cursor
            .as_deref()
            .map(decode_asset_cursor)
            .transpose()?;
        if cursor.as_ref().is_some_and(|cursor| cursor.sort != sort) {
            return Err(IngestionError::InvalidAssetCursor);
        }
        let limit = query.limit.unwrap_or(40).clamp(1, MAX_ASSET_PAGE_SIZE);

        let mut filters = vec!["s.state != 'deleted'".to_owned()];
        let mut base_params = Vec::new();
        if !search.is_empty() {
            let pattern = format!("%{}%", escape_like_query(&search));
            filters.push(
                "(s.display_name LIKE ? ESCAPE '\\' OR s.canonical_uri LIKE ? ESCAPE '\\')"
                    .to_owned(),
            );
            base_params.push(Value::Text(pattern.clone()));
            base_params.push(Value::Text(pattern));
        }
        if let Some(kind) = &kind {
            filters.push("s.kind = ?".to_owned());
            base_params.push(Value::Text(kind.clone()));
        }
        if let Some(state) = &state {
            filters.push("s.state = ?".to_owned());
            base_params.push(Value::Text(state.clone()));
        }
        let where_sql = filters.join(" AND ");
        let database = self
            .database
            .lock()
            .map_err(|_| IngestionError::DatabaseLock)?;
        let count_sql = format!(
            "SELECT COUNT(*) FROM sources s
             JOIN source_versions v ON v.id = s.current_version_id
             WHERE {where_sql}"
        );
        let total: i64 = database.connection().query_row(
            &count_sql,
            params_from_iter(base_params.clone()),
            |row| row.get(0),
        )?;

        let mut page_filters = filters;
        let mut page_params = base_params;
        let order_by = match sort {
            "updated" => {
                if let Some(cursor) = &cursor {
                    page_filters
                        .push("(s.updated_at < ? OR (s.updated_at = ? AND s.id < ?))".to_owned());
                    page_params.push(Value::Text(cursor.primary.clone()));
                    page_params.push(Value::Text(cursor.primary.clone()));
                    page_params.push(Value::Text(cursor.source_id.clone()));
                }
                "s.updated_at DESC, s.id DESC"
            }
            "name" => {
                if let Some(cursor) = &cursor {
                    page_filters.push(
                        "(s.display_name COLLATE NOCASE > ? OR
                          (s.display_name COLLATE NOCASE = ? AND s.id > ?))"
                            .to_owned(),
                    );
                    page_params.push(Value::Text(cursor.primary.clone()));
                    page_params.push(Value::Text(cursor.primary.clone()));
                    page_params.push(Value::Text(cursor.source_id.clone()));
                }
                "s.display_name COLLATE NOCASE ASC, s.id ASC"
            }
            "size" => {
                let cursor_size = cursor
                    .as_ref()
                    .map(|cursor| {
                        cursor
                            .primary
                            .parse::<i64>()
                            .map_err(|_| IngestionError::InvalidAssetCursor)
                    })
                    .transpose()?;
                if let Some(cursor_size) = cursor_size {
                    page_filters
                        .push("(v.byte_size < ? OR (v.byte_size = ? AND s.id > ?))".to_owned());
                    page_params.push(Value::Integer(cursor_size));
                    page_params.push(Value::Integer(cursor_size));
                    page_params.push(Value::Text(
                        cursor
                            .as_ref()
                            .expect("cursor checked above")
                            .source_id
                            .clone(),
                    ));
                }
                "v.byte_size DESC, s.id ASC"
            }
            "type" => {
                if let Some(cursor) = &cursor {
                    page_filters.push(
                        "(v.mime_type COLLATE NOCASE > ? OR
                          (v.mime_type COLLATE NOCASE = ? AND
                           (s.display_name COLLATE NOCASE > ? OR
                            (s.display_name COLLATE NOCASE = ? AND s.id > ?))))"
                            .to_owned(),
                    );
                    page_params.push(Value::Text(cursor.primary.clone()));
                    page_params.push(Value::Text(cursor.primary.clone()));
                    page_params.push(Value::Text(cursor.secondary.clone()));
                    page_params.push(Value::Text(cursor.secondary.clone()));
                    page_params.push(Value::Text(cursor.source_id.clone()));
                }
                "v.mime_type COLLATE NOCASE ASC, s.display_name COLLATE NOCASE ASC, s.id ASC"
            }
            _ => unreachable!("asset sort validated above"),
        };
        let page_where_sql = page_filters.join(" AND ");
        let page_sql = format!(
            "SELECT s.id, v.id, s.kind, s.display_name, s.canonical_uri, v.mime_type,
                    v.byte_size,
                    (SELECT COUNT(*) FROM chunks c WHERE c.source_version_id = v.id),
                    s.state, s.updated_at
             FROM sources s
             JOIN source_versions v ON v.id = s.current_version_id
             WHERE {page_where_sql}
             ORDER BY {order_by}
             LIMIT ?"
        );
        page_params.push(Value::Integer((limit + 1) as i64));
        let mut statement = database.connection().prepare(&page_sql)?;
        let rows = statement.query_map(params_from_iter(page_params), source_summary_from_row)?;
        let mut items = rows.collect::<Result<Vec<_>, _>>()?;
        let has_more = items.len() > limit;
        if has_more {
            items.truncate(limit);
        }
        let next_cursor = has_more
            .then(|| items.last())
            .flatten()
            .map(|item| encode_asset_cursor(item, sort));
        Ok(AssetListResponse {
            items,
            total: total.max(0) as usize,
            next_cursor,
        })
    }

    /// Run the supervised OCR worker for the current retained image version and
    /// attach its bounded text as additional searchable chunks. Repeated runs
    /// append a new OCR result so previous citation chunks remain valid; the
    /// original image bytes are never changed and a stale version cannot be
    /// mutated.
    pub async fn ocr_current_image(
        &self,
        source_id: Uuid,
        version_id: Uuid,
        worker: &ImageOcrWorker,
        cancellation: &CancellationToken,
    ) -> Result<usize, IngestionError> {
        check_cancelled(cancellation)?;
        let (current_version, original_hash, mime_type, extracted_hash) = {
            let database = self
                .database
                .lock()
                .map_err(|_| IngestionError::DatabaseLock)?;
            database
                .connection()
                .query_row(
                    "SELECT s.current_version_id, v.original_object_hash, v.mime_type,
                            v.extracted_object_hash
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
        let current: Option<(String, Option<String>)> = transaction
            .query_row(
                "SELECT s.current_version_id, v.extracted_object_hash
                 FROM sources s JOIN source_versions v ON v.id = s.current_version_id
                 WHERE s.id = ?1 AND s.state NOT IN ('deleted', 'missing')",
                [source_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((current_id, previous_extracted)) = current else {
            return Err(IngestionError::StaleImageVersion);
        };
        if current_id != version_id.to_string() {
            return Err(IngestionError::StaleImageVersion);
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

    /// Remove OCR-derived chunks from the current image version while keeping
    /// the original pixels and technical image metadata. This is an explicit
    /// destructive operation; citations to the removed OCR chunks no longer
    /// resolve, while image metadata citations remain valid.
    pub fn delete_ocr_current_image(
        &self,
        source_id: Uuid,
        version_id: Uuid,
    ) -> Result<usize, IngestionError> {
        self.delete_ocr_chunks(source_id, version_id, None)
    }

    /// Remove one OCR-derived chunk from the current image version. The
    /// original pixels, image metadata, and all other OCR detections remain
    /// retained; only the selected detection and its searchable object are
    /// removed.
    pub fn delete_ocr_chunk_current_image(
        &self,
        source_id: Uuid,
        version_id: Uuid,
        chunk_id: Uuid,
    ) -> Result<usize, IngestionError> {
        self.delete_ocr_chunks(source_id, version_id, Some(chunk_id))
    }

    fn delete_ocr_chunks(
        &self,
        source_id: Uuid,
        version_id: Uuid,
        target_chunk_id: Option<Uuid>,
    ) -> Result<usize, IngestionError> {
        let database = self
            .database
            .lock()
            .map_err(|_| IngestionError::DatabaseLock)?;
        let transaction = database.connection().unchecked_transaction()?;
        let current: Option<(String, String, Option<String>)> = transaction
            .query_row(
                "SELECT s.current_version_id, v.mime_type, v.extracted_object_hash
                 FROM sources s JOIN source_versions v ON v.id = s.current_version_id
                 WHERE s.id = ?1 AND s.state NOT IN ('deleted', 'missing')",
                [source_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        let Some((current_id, mime_type, previous_extracted)) = current else {
            return Err(IngestionError::StaleImageVersion);
        };
        if current_id != version_id.to_string() {
            return Err(IngestionError::StaleImageVersion);
        }
        if !is_supported_image(&mime_type) {
            return Err(IngestionError::Ocr(ImageOcrError::UnsupportedMime));
        }

        let mut ocr_chunks = Vec::new();
        {
            let mut statement = transaction.prepare(
                "SELECT id, extracted_text_hash FROM chunks
                 WHERE source_version_id = ?1 AND coordinates_json LIKE '%\"ocr\":true%'",
            )?;
            let rows = statement.query_map([version_id.to_string()], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows {
                ocr_chunks.push(row?);
            }
        }
        let target_chunk_id = target_chunk_id.map(|chunk_id| chunk_id.to_string());
        let removed_chunks = ocr_chunks
            .iter()
            .filter(|(chunk_id, _)| {
                target_chunk_id
                    .as_deref()
                    .is_none_or(|target| target == chunk_id)
            })
            .cloned()
            .collect::<Vec<_>>();
        if removed_chunks.is_empty() {
            return if target_chunk_id.is_some() {
                Err(IngestionError::OcrChunkNotFound)
            } else {
                Ok(0)
            };
        }
        let remaining_chunks = ocr_chunks
            .iter()
            .filter(|(chunk_id, _)| {
                !removed_chunks
                    .iter()
                    .any(|(removed_id, _)| removed_id == chunk_id)
            })
            .collect::<Vec<_>>();

        let previous_extracted = previous_extracted.ok_or(IngestionError::InvalidImageMetadata)?;
        let metadata_bytes = self.objects.read_verified(&previous_extracted)?;
        let mut metadata: serde_json::Value = serde_json::from_slice(&metadata_bytes)?;
        let Some(metadata) = metadata.as_object_mut() else {
            return Err(IngestionError::InvalidImageMetadata);
        };
        if remaining_chunks.is_empty() {
            metadata.remove("ocr_text");
            metadata.remove("ocr_version");
            metadata.remove("ocr_runs");
        } else {
            let remaining_text = remaining_chunks
                .iter()
                .map(|(_, hash)| {
                    String::from_utf8(self.objects.read_verified(hash)?)
                        .map_err(|_| IngestionError::InvalidOcrChunkText)
                })
                .collect::<Result<Vec<_>, _>>()?
                .join("\n\n");
            metadata["ocr_text"] = serde_json::Value::String(remaining_text);
            metadata["ocr_version"] = serde_json::Value::String("pinky-ocr-v1".to_owned());
        }
        let replacement = self.objects.put(
            serde_json::to_string_pretty(&serde_json::Value::Object(metadata.clone()))?.as_bytes(),
            "application/json",
        )?;
        record_object(&transaction, &replacement, 1)?;
        transaction.execute(
            "UPDATE objects SET reference_count = reference_count - 1
            WHERE sha256 = ?1 AND reference_count > 0",
            [previous_extracted],
        )?;
        for (_, hash) in &removed_chunks {
            transaction.execute(
                "UPDATE objects SET reference_count = reference_count - 1
                 WHERE sha256 = ?1 AND reference_count > 0",
                [hash],
            )?;
        }
        if let Some(target_chunk_id) = target_chunk_id.as_deref() {
            transaction.execute(
                "DELETE FROM chunks
                 WHERE source_version_id = ?1 AND id = ?2
                   AND coordinates_json LIKE '%\"ocr\":true%'",
                params![version_id.to_string(), target_chunk_id],
            )?;
        } else {
            transaction.execute(
                "DELETE FROM chunks
                 WHERE source_version_id = ?1 AND coordinates_json LIKE '%\"ocr\":true%'",
                [version_id.to_string()],
            )?;
        }
        let extraction_method = if remaining_chunks.is_empty() {
            "builtin_image_metadata"
        } else {
            "builtin_image_metadata+ocr"
        };
        let extraction_version = if remaining_chunks.is_empty() {
            IMAGE_EXTRACTION_VERSION
        } else {
            "pinky-image-metadata-v1+ocr-v1"
        };
        let now = Utc::now().to_rfc3339();
        transaction.execute(
            "UPDATE source_versions
             SET extracted_object_hash = ?1, extraction_method = ?2,
                 extraction_version = ?3, processing_state = 'ready', error = NULL,
                 retrieved_at = ?4
             WHERE id = ?5",
            params![
                replacement.metadata.sha256,
                extraction_method,
                extraction_version,
                now,
                version_id.to_string()
            ],
        )?;
        transaction.execute(
            "UPDATE sources SET updated_at = ?1, last_checked_at = ?1 WHERE id = ?2",
            params![now, source_id.to_string()],
        )?;
        transaction.commit()?;
        drop(database);

        let indexed = RetrievalService::new(self.database.clone(), self.objects.clone())
            .current_chunks()?
            .into_iter()
            .filter(|chunk| chunk.version_id == version_id)
            .collect::<Vec<_>>();
        RetrievalService::new(self.database.clone(), self.objects.clone())
            .index_version(version_id, &indexed)?;
        Ok(removed_chunks.len())
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

fn source_summary_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<SourceSummary> {
    let source_id: String = row.get(0)?;
    let version_id: String = row.get(1)?;
    Ok(SourceSummary {
        source_id: Uuid::parse_str(&source_id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        version_id: Uuid::parse_str(&version_id).map_err(|_| rusqlite::Error::InvalidQuery)?,
        kind: row.get(2)?,
        display_name: row.get(3)?,
        canonical_uri: row.get(4)?,
        mime_type: row.get(5)?,
        byte_size: row.get::<_, i64>(6)? as u64,
        chunk_count: row.get::<_, i64>(7)? as usize,
        state: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn validate_asset_filter(
    field: &'static str,
    value: Option<String>,
    allowed: &[&str],
) -> Result<Option<String>, IngestionError> {
    value
        .map(|value| {
            if allowed.contains(&value.as_str()) {
                Ok(value)
            } else {
                Err(IngestionError::InvalidAssetFilter { field, value })
            }
        })
        .transpose()
}

fn escape_like_query(query: &str) -> String {
    query
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn encode_asset_cursor(item: &SourceSummary, sort: &str) -> String {
    let (primary, secondary) = match sort {
        "updated" => (item.updated_at.clone(), String::new()),
        "name" => (item.display_name.clone(), String::new()),
        "size" => (item.byte_size.to_string(), String::new()),
        "type" => (item.mime_type.clone(), item.display_name.clone()),
        _ => unreachable!("asset sort validated before encoding"),
    };
    let cursor = AssetCursor {
        sort: sort.to_owned(),
        primary,
        secondary,
        source_id: item.source_id.to_string(),
    };
    URL_SAFE_NO_PAD.encode(serde_json::to_vec(&cursor).expect("asset cursor is serializable"))
}

fn decode_asset_cursor(encoded: &str) -> Result<AssetCursor, IngestionError> {
    if encoded.is_empty() || encoded.len() > 4096 {
        return Err(IngestionError::InvalidAssetCursor);
    }
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded.as_bytes())
        .map_err(|_| IngestionError::InvalidAssetCursor)?;
    let cursor: AssetCursor =
        serde_json::from_slice(&bytes).map_err(|_| IngestionError::InvalidAssetCursor)?;
    Uuid::parse_str(&cursor.source_id).map_err(|_| IngestionError::InvalidAssetCursor)?;
    Ok(cursor)
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
    page: Option<usize>,
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
            page: None,
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

fn chunk_pdf_text(text: &str) -> Vec<TextChunk> {
    text.split('\u{000c}')
        .enumerate()
        .flat_map(|(page_index, page_text)| {
            chunk_text(page_text).into_iter().map(move |mut chunk| {
                chunk.page = Some(page_index + 1);
                chunk
            })
        })
        .collect()
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
    fn asset_listing_is_filterable_and_keyset_paginated() {
        let (_vault, approved, ingestor) = ingestor();
        for name in ["alpha.txt", "bravo.md", "charlie.txt"] {
            fs::write(approved.path().join(name), format!("content for {name}")).unwrap();
            ingestor
                .ingest(approved.path(), approved.path().join(name))
                .unwrap();
        }

        let first = ingestor
            .list_assets(AssetListQuery {
                sort: Some("name".to_owned()),
                limit: Some(2),
                ..AssetListQuery::default()
            })
            .unwrap();
        assert_eq!(first.total, 3);
        assert_eq!(first.items.len(), 2);
        assert_eq!(first.items[0].display_name, "alpha.txt");
        assert_eq!(first.items[1].display_name, "bravo.md");
        assert!(first.next_cursor.is_some());

        let second = ingestor
            .list_assets(AssetListQuery {
                sort: Some("name".to_owned()),
                cursor: first.next_cursor,
                limit: Some(2),
                ..AssetListQuery::default()
            })
            .unwrap();
        assert_eq!(second.items.len(), 1);
        assert_eq!(second.items[0].display_name, "charlie.txt");
        assert!(second.next_cursor.is_none());

        let filtered = ingestor
            .list_assets(AssetListQuery {
                search: Some("bravo".to_owned()),
                kind: Some("local_file".to_owned()),
                ..AssetListQuery::default()
            })
            .unwrap();
        assert_eq!(filtered.total, 1);
        assert_eq!(filtered.items[0].kind, "local_file");
        assert!(matches!(
            ingestor.list_assets(AssetListQuery {
                sort: Some("unknown".to_owned()),
                ..AssetListQuery::default()
            }),
            Err(IngestionError::InvalidAssetFilter { field: "sort", .. })
        ));
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
    fn extracts_pdf_text_into_page_aware_searchable_chunks() {
        let (_vault, approved, ingestor) = ingestor();
        let executable = approved.path().join("pdftotext-fixture.sh");
        fs::write(
            &executable,
            "#!/bin/sh\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\nprintf 'Page one\\fPage two\\n' > \"$last\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let ingestor = ingestor.with_pdf_extractor(PdfTextExtractor::new(&executable).unwrap());
        let source = approved.path().join("sample.pdf");
        fs::write(&source, b"%PDF-1.7\0fixture").unwrap();

        let ingested = ingestor.ingest(approved.path(), &source).unwrap();
        assert_eq!(ingested.state, "active");
        assert_eq!(ingested.chunk_count, 2);
        let retrieval = RetrievalService::new(ingestor.database.clone(), ingestor.objects.clone());
        let first = retrieval.search("Page one", 5).unwrap();
        let second = retrieval.search("Page two", 5).unwrap();
        assert_eq!(first[0].coordinates["page"], 1);
        assert_eq!(second[0].coordinates["page"], 2);
        let database = ingestor.database.lock().unwrap();
        let method: String = database
            .connection()
            .query_row(
                "SELECT extraction_method FROM source_versions WHERE id = ?1",
                [ingested.version_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(method, "builtin_pdf_text");
    }

    #[test]
    fn failed_pdf_replacement_keeps_the_previous_version_searchable() {
        let (_vault, approved, ingestor) = ingestor();
        let executable = approved.path().join("pdftotext-replacement-fixture.sh");
        fs::write(
            &executable,
            "#!/bin/sh\nif grep -q fail \"$4\"; then exit 17; fi\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\nprintf 'stable PDF evidence\\n' > \"$last\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let ingestor = ingestor.with_pdf_extractor(PdfTextExtractor::new(&executable).unwrap());
        let source = approved.path().join("replacement.pdf");
        fs::write(&source, b"%PDF-1.7 stable").unwrap();
        let first = ingestor.ingest(approved.path(), &source).unwrap();
        assert_eq!(first.state, "active");
        assert_eq!(
            ingestor.list_sources().unwrap()[0].version_id,
            first.version_id
        );

        fs::write(&source, b"%PDF-1.7 fail").unwrap();
        let failed_attempt = ingestor.ingest(approved.path(), &source).unwrap();
        assert_eq!(failed_attempt.state, "unsupported");

        let current = ingestor.list_sources().unwrap();
        assert_eq!(current.len(), 1);
        assert_eq!(current[0].state, "active");
        assert_eq!(current[0].version_id, first.version_id);
        let retrieval = RetrievalService::new(ingestor.database.clone(), ingestor.objects.clone());
        let hits = retrieval.search("stable evidence", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].version_id, first.version_id);
        let database = ingestor.database.lock().unwrap();
        let failed_count: i64 = database
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM source_versions WHERE source_id = ?1 AND processing_state = 'unsupported'",
                [first.source_id.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(failed_count, 1);
    }

    #[test]
    fn records_pdf_page_ocr_provenance_in_the_retained_version() {
        let (_vault, approved, ingestor) = ingestor();
        let extractor = approved.path().join("pdftotext-provenance-fixture.sh");
        fs::write(
            &extractor,
            "#!/bin/sh\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\nprintf 'embedded page\\f\\f' > \"$last\"\n",
        )
        .unwrap();
        fs::set_permissions(&extractor, fs::Permissions::from_mode(0o700)).unwrap();

        let rendered = approved.path().join("rendered.png");
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        png.extend_from_slice(&[0, 0, 2, 0, 0, 0, 2, 0, 8, 6, 0, 0, 0, 0]);
        fs::write(&rendered, png).unwrap();
        let renderer = approved.path().join("pdftoppm-provenance-fixture.sh");
        fs::write(
            &renderer,
            format!(
                "#!/bin/sh\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\ncp '{}' \"$last.png\"\n",
                rendered.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&renderer, fs::Permissions::from_mode(0o700)).unwrap();
        let ocr = approved.path().join("tesseract-provenance-fixture.sh");
        fs::write(
            &ocr,
            "#!/bin/sh\nprintf 'ocr page evidence\\n' > \"$2.txt\"\n",
        )
        .unwrap();
        fs::set_permissions(&ocr, fs::Permissions::from_mode(0o700)).unwrap();

        let extractor = PdfTextExtractor::new(&extractor)
            .unwrap()
            .with_renderer(&renderer)
            .unwrap()
            .with_page_ocr(ImageOcrWorker::new(&ocr, "eng").unwrap());
        let ingestor = ingestor.with_pdf_extractor(extractor);
        let source = approved.path().join("provenance.pdf");
        fs::write(&source, b"%PDF-1.7 fixture").unwrap();
        let ingested = ingestor.ingest(approved.path(), &source).unwrap();

        let database = ingestor.database.lock().unwrap();
        let (method, citation_map): (String, String) = database
            .connection()
            .query_row(
                "SELECT extraction_method, citation_map_json FROM source_versions WHERE id = ?1",
                [ingested.version_id.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(method, "builtin_pdf_text_ocr");
        let citation_map: serde_json::Value = serde_json::from_str(&citation_map).unwrap();
        assert_eq!(citation_map["ocr_pages"], serde_json::json!([2]));
        drop(database);

        let retrieval = RetrievalService::new(ingestor.database.clone(), ingestor.objects.clone());
        let hits = retrieval.search("ocr evidence", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].coordinates["page"], 2);
    }

    #[test]
    fn reopens_a_retained_pdf_after_a_clean_process_restart() {
        let (vault_root, approved, ingestor) = ingestor();
        let executable = approved.path().join("pdftotext-restart-fixture.sh");
        fs::write(
            &executable,
            "#!/bin/sh\nlast=\nfor arg in \"$@\"; do last=\"$arg\"; done\nprintf 'restart-safe PDF text\\n' > \"$last\"\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let extractor = PdfTextExtractor::new(&executable).unwrap();
        let ingestor = ingestor.with_pdf_extractor(extractor.clone());
        let source = approved.path().join("restart.pdf");
        fs::write(&source, b"%PDF-1.7 fixture").unwrap();
        let retained = ingestor.ingest(approved.path(), &source).unwrap();
        drop(ingestor);

        let vault = Vault::open_with(vault_root.path(), Mounted).unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, zeroize::Zeroizing::new(vec![0x51; 32])).unwrap(),
        ));
        let reopened =
            LocalIngestor::new(database, ObjectStore::new(vault)).with_pdf_extractor(extractor);
        let sources = reopened.list_sources().unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].version_id, retained.version_id);
        let retrieval = RetrievalService::new(reopened.database.clone(), reopened.objects.clone());
        let hits = retrieval.search("restart-safe", 5).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].version_id, retained.version_id);
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
        assert_eq!(hits[0].coordinates["image_metadata"], true);
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
        let rerun_chunks = ingestor
            .ocr_current_image(
                retained.source_id,
                retained.version_id,
                &worker,
                &CancellationToken::new(),
            )
            .await;
        assert_eq!(rerun_chunks.unwrap(), 1);
        let hits = retrieval.search("invoice", 5).unwrap();
        assert_eq!(hits.len(), 2);
        assert_ne!(hits[0].citation_uri, hits[1].citation_uri);
        assert!(retrieval.open_citation(&hits[0].citation_uri).is_ok());
        assert!(retrieval.open_citation(&hits[1].citation_uri).is_ok());

        let deleted_chunk_id = hits[0].chunk_id;
        assert_eq!(
            ingestor
                .delete_ocr_chunk_current_image(
                    retained.source_id,
                    retained.version_id,
                    deleted_chunk_id,
                )
                .unwrap(),
            1
        );
        let remaining_hits = retrieval.search("invoice", 5).unwrap();
        assert_eq!(remaining_hits.len(), 1);
        assert!(retrieval
            .open_citation(&remaining_hits[0].citation_uri)
            .is_ok());
        assert_eq!(
            ingestor
                .delete_ocr_current_image(retained.source_id, retained.version_id)
                .unwrap(),
            1
        );
        assert!(retrieval.search("invoice", 5).unwrap().is_empty());
        assert_eq!(ingestor.list_sources().unwrap()[0].chunk_count, 1);
        let metadata_hits = retrieval.search("PNG", 5).unwrap();
        assert_eq!(metadata_hits.len(), 1);
        assert!(retrieval
            .open_citation(&metadata_hits[0].citation_uri)
            .is_ok());
        assert_eq!(
            ingestor
                .delete_ocr_current_image(retained.source_id, retained.version_id)
                .unwrap(),
            0
        );
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
