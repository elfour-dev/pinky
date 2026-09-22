use std::{
    collections::HashMap,
    fs,
    sync::{Arc, Mutex},
};

use base64::{engine::general_purpose::STANDARD, Engine as _};
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use tantivy::{
    collector::TopDocs,
    query::QueryParser,
    schema::{Field, Schema, TantivyDocument, Value, STORED, STRING, TEXT},
    Index, IndexReader, ReloadPolicy, TantivyError, Term,
};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use crate::{
    embedding::{EmbeddingError, EmbeddingProvider},
    hybrid::{reciprocal_rank_fusion, RankedChunk},
    qdrant::{QdrantClient, QdrantError, VectorMatch},
    reranking::{rerank_hits, MAX_RERANK_CANDIDATES},
    Database, ObjectStore, ObjectStoreError, VaultError,
};

const WRITER_MEMORY_BYTES: usize = 15_000_000;
pub const HYBRID_CANDIDATE_LIMIT: usize = 50;

#[derive(Debug, Error)]
pub enum RetrievalError {
    #[error("vault unavailable: {0}")]
    Vault(#[from] VaultError),
    #[error("retrieval index error: {0}")]
    Index(#[from] TantivyError),
    #[error("retrieval database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("retrieval object error: {0}")]
    Object(#[from] ObjectStoreError),
    #[error("retrieval I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("retrieval database lock is poisoned")]
    DatabaseLock,
    #[error("invalid citation URI")]
    InvalidCitation,
    #[error("citation does not refer to a retained chunk")]
    CitationNotFound,
    #[error("retained chunk is not valid UTF-8")]
    InvalidChunkText,
    #[error("retained image is too large to display safely")]
    ImageTooLarge,
    #[error("citation does not refer to a retained image")]
    NotAnImage,
}

#[derive(Debug, Error)]
pub enum HybridRetrievalError {
    #[error("hybrid retrieval was cancelled")]
    Cancelled,
    #[error("lexical retrieval failed: {0}")]
    Retrieval(#[from] RetrievalError),
    #[error("query embedding failed: {0}")]
    Embedding(#[from] EmbeddingError),
    #[error("vector retrieval failed: {0}")]
    Qdrant(#[from] QdrantError),
}

#[derive(Debug, Clone)]
pub struct IndexedChunk {
    pub chunk_id: Uuid,
    pub source_id: Uuid,
    pub version_id: Uuid,
    pub ordinal: u64,
    pub display_name: String,
    pub heading: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SearchHit {
    pub score: f32,
    pub citation_uri: String,
    pub source_id: Uuid,
    pub version_id: Uuid,
    pub chunk_id: Uuid,
    pub ordinal: u64,
    pub display_name: String,
    pub heading: Option<String>,
    pub passage: String,
    pub coordinates: serde_json::Value,
    pub retrieved_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CitationPassage {
    pub citation_uri: String,
    pub source_id: Uuid,
    pub version_id: Uuid,
    pub chunk_id: Uuid,
    pub ordinal: u64,
    pub display_name: String,
    pub canonical_uri: String,
    pub mime_type: String,
    pub passage: String,
    pub heading: Option<String>,
    pub coordinates: serde_json::Value,
    pub retrieved_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetainedImage {
    pub source_id: Uuid,
    pub version_id: Uuid,
    pub display_name: String,
    pub mime_type: String,
    pub byte_size: usize,
    pub bytes_base64: String,
}

#[derive(Clone)]
pub struct RetrievalService {
    database: Arc<Mutex<Database>>,
    objects: ObjectStore,
}

impl RetrievalService {
    pub fn new(database: Arc<Mutex<Database>>, objects: ObjectStore) -> Self {
        Self { database, objects }
    }

    pub fn index_version(
        &self,
        version_id: Uuid,
        chunks: &[IndexedChunk],
    ) -> Result<(), RetrievalError> {
        let lexical = LexicalIndex::open(&self.objects)?;
        lexical.replace_version(version_id, chunks)
    }

    pub fn search(&self, query: &str, limit: usize) -> Result<Vec<SearchHit>, RetrievalError> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let lexical = LexicalIndex::open(&self.objects)?;
        self.rebuild_if_needed(&lexical)?;
        let candidates = lexical.search(query, limit.saturating_mul(10).max(50))?;
        let database = self
            .database
            .lock()
            .map_err(|_| RetrievalError::DatabaseLock)?;
        let mut hits = Vec::new();
        for candidate in candidates {
            let row = database
                .connection()
                .query_row(
                    "SELECT s.id, v.id, c.id, c.ordinal, s.display_name, c.heading_path,
                        c.extracted_text_hash, c.coordinates_json, v.retrieved_at
                 FROM chunks c
                 JOIN source_versions v ON v.id = c.source_version_id
                 JOIN sources s ON s.id = v.source_id
                 WHERE c.id = ?1 AND s.current_version_id = v.id
                   AND s.state NOT IN ('deleted', 'unsupported')",
                    [&candidate.chunk_id],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, i64>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, Option<String>>(5)?,
                            row.get::<_, String>(6)?,
                            row.get::<_, Option<String>>(7)?,
                            row.get::<_, String>(8)?,
                        ))
                    },
                )
                .optional()?;
            let Some((
                source_id,
                version_id,
                chunk_id,
                ordinal,
                display_name,
                heading,
                hash,
                coordinates,
                retrieved_at,
            )) = row
            else {
                continue;
            };
            let passage = String::from_utf8(self.objects.read_verified(&hash)?)
                .map_err(|_| RetrievalError::InvalidChunkText)?;
            let source_id = parse_uuid(&source_id)?;
            let version_id = parse_uuid(&version_id)?;
            hits.push(SearchHit {
                score: candidate.score,
                citation_uri: citation_uri(source_id, version_id, ordinal as u64),
                source_id,
                version_id,
                chunk_id: parse_uuid(&chunk_id)?,
                ordinal: ordinal as u64,
                display_name,
                heading,
                passage,
                coordinates: parse_coordinates(coordinates.as_deref()),
                retrieved_at,
            });
            if hits.len() == limit {
                break;
            }
        }
        Ok(hits)
    }

    pub fn current_chunks(&self) -> Result<Vec<IndexedChunk>, RetrievalError> {
        self.load_current_chunks(None)
    }

    pub fn current_chunks_needing_embedding(
        &self,
        embedding_identity: &str,
    ) -> Result<Vec<IndexedChunk>, RetrievalError> {
        self.load_current_chunks(Some(embedding_identity))
    }

    pub fn mark_chunks_embedded(
        &self,
        chunk_ids: &[Uuid],
        embedding_identity: &str,
    ) -> Result<usize, RetrievalError> {
        if chunk_ids.is_empty() || embedding_identity.is_empty() {
            return Ok(0);
        }
        let database = self
            .database
            .lock()
            .map_err(|_| RetrievalError::DatabaseLock)?;
        let mut statement = database
            .connection()
            .prepare("UPDATE chunks SET embedding_id = ?1 WHERE id = ?2")?;
        let mut marked = 0;
        for chunk_id in chunk_ids {
            marked += statement.execute(params![embedding_identity, chunk_id.to_string()])?;
        }
        Ok(marked)
    }

    fn load_current_chunks(
        &self,
        embedding_identity: Option<&str>,
    ) -> Result<Vec<IndexedChunk>, RetrievalError> {
        let database = self
            .database
            .lock()
            .map_err(|_| RetrievalError::DatabaseLock)?;
        let mut statement = database.connection().prepare(
            "SELECT c.id, s.id, v.id, c.ordinal, s.display_name, c.heading_path,
                    c.extracted_text_hash
             FROM chunks c
             JOIN source_versions v ON v.id = c.source_version_id
             JOIN sources s ON s.id = v.source_id
             WHERE s.current_version_id = v.id
               AND s.state NOT IN ('deleted', 'unsupported')
               AND (?1 IS NULL OR c.embedding_id IS NULL OR c.embedding_id != ?1)
             ORDER BY v.id, c.ordinal",
        )?;
        let rows = statement
            .query_map([embedding_identity], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        rows.into_iter()
            .map(|row| {
                Ok(IndexedChunk {
                    chunk_id: parse_uuid(&row.0)?,
                    source_id: parse_uuid(&row.1)?,
                    version_id: parse_uuid(&row.2)?,
                    ordinal: row.3 as u64,
                    display_name: row.4,
                    heading: row.5,
                    body: String::from_utf8(self.objects.read_verified(&row.6)?)
                        .map_err(|_| RetrievalError::InvalidChunkText)?,
                })
            })
            .collect()
    }

    pub fn merge_hybrid_hits(
        &self,
        lexical: &[SearchHit],
        vector: &[VectorMatch],
        limit: usize,
    ) -> Result<Vec<SearchHit>, RetrievalError> {
        self.merge_hybrid_hits_for_query("", lexical, vector, limit)
    }

    pub fn merge_hybrid_hits_for_query(
        &self,
        query: &str,
        lexical: &[SearchHit],
        vector: &[VectorMatch],
        limit: usize,
    ) -> Result<Vec<SearchHit>, RetrievalError> {
        if limit == 0 {
            return Ok(Vec::new());
        }
        let lexical_ranked = lexical
            .iter()
            .map(|hit| RankedChunk {
                chunk_id: hit.chunk_id,
                source_version_id: hit.version_id,
                score: hit.score,
            })
            .collect::<Vec<_>>();
        let vector_ranked = vector
            .iter()
            .map(|hit| RankedChunk {
                chunk_id: hit.id,
                source_version_id: hit.source_version_id,
                score: hit.score,
            })
            .collect::<Vec<_>>();
        let mut hits_by_id = lexical
            .iter()
            .cloned()
            .map(|hit| (hit.chunk_id, hit))
            .collect::<HashMap<_, _>>();
        for candidate in vector {
            if hits_by_id.contains_key(&candidate.id) {
                continue;
            }
            if let Some(hit) = self.load_current_hit(candidate.id)? {
                hits_by_id.insert(candidate.id, hit);
            }
        }
        let fused = reciprocal_rank_fusion(&lexical_ranked, &vector_ranked, MAX_RERANK_CANDIDATES)
            .into_iter()
            .filter_map(|candidate| {
                hits_by_id.remove(&candidate.chunk_id).map(|mut hit| {
                    hit.score = candidate.fused_score;
                    hit
                })
            })
            .collect::<Vec<_>>();
        Ok(rerank_hits(query, &fused, limit))
    }

    pub async fn search_hybrid<P: EmbeddingProvider + ?Sized>(
        &self,
        query: &str,
        limit: usize,
        provider: &P,
        qdrant: &QdrantClient,
        cancellation: &CancellationToken,
    ) -> Result<Vec<SearchHit>, HybridRetrievalError> {
        self.search_hybrid_with_identity(query, limit, provider, qdrant, None, cancellation)
            .await
    }

    pub async fn search_hybrid_with_identity<P: EmbeddingProvider + ?Sized>(
        &self,
        query: &str,
        limit: usize,
        provider: &P,
        qdrant: &QdrantClient,
        embedding_identity: Option<&str>,
        cancellation: &CancellationToken,
    ) -> Result<Vec<SearchHit>, HybridRetrievalError> {
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        if cancellation.is_cancelled() {
            return Err(HybridRetrievalError::Cancelled);
        }

        let lexical = self.search(query, HYBRID_CANDIDATE_LIMIT)?;
        let response = provider.embed(&[query.to_owned()], cancellation).await?;
        let query_vector = response
            .vectors
            .into_iter()
            .next()
            .ok_or(EmbeddingError::InvalidVectors)?;

        if cancellation.is_cancelled() {
            return Err(HybridRetrievalError::Cancelled);
        }
        let vector_future = async {
            match embedding_identity {
                Some(identity) => {
                    qdrant
                        .query_for_embedding(&query_vector, HYBRID_CANDIDATE_LIMIT, identity)
                        .await
                }
                None => qdrant.query(&query_vector, HYBRID_CANDIDATE_LIMIT).await,
            }
        };
        let vector = tokio::select! {
            _ = cancellation.cancelled() => return Err(HybridRetrievalError::Cancelled),
            result = vector_future => result?,
        };
        if cancellation.is_cancelled() {
            return Err(HybridRetrievalError::Cancelled);
        }
        Ok(self.merge_hybrid_hits_for_query(query, &lexical, &vector, limit)?)
    }

    pub fn open_citation(&self, uri: &str) -> Result<CitationPassage, RetrievalError> {
        let (source_id, version_id, ordinal) = parse_citation_uri(uri)?;
        let database = self
            .database
            .lock()
            .map_err(|_| RetrievalError::DatabaseLock)?;
        let row = database
            .connection()
            .query_row(
                "SELECT c.id, s.display_name, s.canonical_uri, v.mime_type, c.extracted_text_hash,
                    c.heading_path, c.coordinates_json, v.retrieved_at
             FROM chunks c
             JOIN source_versions v ON v.id = c.source_version_id
             JOIN sources s ON s.id = v.source_id
             WHERE s.id = ?1 AND v.id = ?2 AND c.ordinal = ?3 AND s.state != 'deleted'",
                params![
                    source_id.to_string(),
                    version_id.to_string(),
                    ordinal as i64
                ],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, Option<String>>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                },
            )
            .optional()?
            .ok_or(RetrievalError::CitationNotFound)?;
        let passage = String::from_utf8(self.objects.read_verified(&row.4)?)
            .map_err(|_| RetrievalError::InvalidChunkText)?;
        Ok(CitationPassage {
            citation_uri: uri.to_owned(),
            source_id,
            version_id,
            chunk_id: parse_uuid(&row.0)?,
            ordinal,
            display_name: row.1,
            canonical_uri: row.2,
            mime_type: row.3,
            passage,
            heading: row.5,
            coordinates: parse_coordinates(row.6.as_deref()),
            retrieved_at: row.7,
        })
    }

    /// Return the exact retained image bytes for a citation. This is an
    /// explicit second request so opening a text/metadata citation never
    /// eagerly copies image pixels through IPC.
    pub fn open_retained_image(&self, uri: &str) -> Result<RetainedImage, RetrievalError> {
        let (source_id, version_id, ordinal) = parse_citation_uri(uri)?;
        let database = self
            .database
            .lock()
            .map_err(|_| RetrievalError::DatabaseLock)?;
        let row: Option<(String, String, String, i64)> = database
            .connection()
            .query_row(
                "SELECT s.display_name, v.mime_type, v.original_object_hash, v.byte_size
                 FROM source_versions v
                 JOIN sources s ON s.id = v.source_id
                 JOIN chunks c ON c.source_version_id = v.id
                 WHERE s.id = ?1 AND v.id = ?2 AND c.ordinal = ?3 AND s.state != 'deleted'",
                params![
                    source_id.to_string(),
                    version_id.to_string(),
                    ordinal as i64
                ],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let Some((display_name, mime_type, hash, byte_size)) = row else {
            return Err(RetrievalError::CitationNotFound);
        };
        if !matches!(
            mime_type.as_str(),
            "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/tiff"
        ) {
            return Err(RetrievalError::NotAnImage);
        }
        if byte_size < 0 || byte_size as usize > crate::MAX_IMAGE_METADATA_BYTES {
            return Err(RetrievalError::ImageTooLarge);
        }
        let bytes = self.objects.read_verified(&hash)?;
        if bytes.len() > crate::MAX_IMAGE_METADATA_BYTES {
            return Err(RetrievalError::ImageTooLarge);
        }
        Ok(RetainedImage {
            source_id,
            version_id,
            display_name,
            mime_type,
            byte_size: bytes.len(),
            bytes_base64: STANDARD.encode(bytes),
        })
    }

    fn rebuild_if_needed(&self, lexical: &LexicalIndex) -> Result<(), RetrievalError> {
        let database = self
            .database
            .lock()
            .map_err(|_| RetrievalError::DatabaseLock)?;
        let expected: u64 =
            database
                .connection()
                .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))?;
        if lexical.num_docs()? == expected {
            return Ok(());
        }
        let mut statement = database.connection().prepare(
            "SELECT c.id, s.id, v.id, c.ordinal, s.display_name, c.heading_path, c.extracted_text_hash
             FROM chunks c JOIN source_versions v ON v.id = c.source_version_id
             JOIN sources s ON s.id = v.source_id ORDER BY v.id, c.ordinal"
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, String>(6)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let mut chunks = Vec::with_capacity(rows.len());
        for row in rows {
            chunks.push(IndexedChunk {
                chunk_id: parse_uuid(&row.0)?,
                source_id: parse_uuid(&row.1)?,
                version_id: parse_uuid(&row.2)?,
                ordinal: row.3 as u64,
                display_name: row.4,
                heading: row.5,
                body: String::from_utf8(self.objects.read_verified(&row.6)?)
                    .map_err(|_| RetrievalError::InvalidChunkText)?,
            });
        }
        lexical.rebuild(&chunks)
    }

    fn load_current_hit(&self, chunk_id: Uuid) -> Result<Option<SearchHit>, RetrievalError> {
        let database = self
            .database
            .lock()
            .map_err(|_| RetrievalError::DatabaseLock)?;
        let row = database
            .connection()
            .query_row(
                "SELECT s.id, v.id, c.id, c.ordinal, s.display_name, c.heading_path,
                        c.extracted_text_hash, c.coordinates_json, v.retrieved_at
                 FROM chunks c
                 JOIN source_versions v ON v.id = c.source_version_id
                 JOIN sources s ON s.id = v.source_id
                 WHERE c.id = ?1 AND s.current_version_id = v.id
                   AND s.state NOT IN ('deleted', 'unsupported')",
                [chunk_id.to_string()],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<String>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, Option<String>>(7)?,
                        row.get::<_, String>(8)?,
                    ))
                },
            )
            .optional()?;
        let Some((
            source_id,
            version_id,
            chunk_id,
            ordinal,
            display_name,
            heading,
            hash,
            coordinates,
            retrieved_at,
        )) = row
        else {
            return Ok(None);
        };
        let source_id = parse_uuid(&source_id)?;
        let version_id = parse_uuid(&version_id)?;
        let chunk_id = parse_uuid(&chunk_id)?;
        Ok(Some(SearchHit {
            score: 0.0,
            citation_uri: citation_uri(source_id, version_id, ordinal as u64),
            source_id,
            version_id,
            chunk_id,
            ordinal: ordinal as u64,
            display_name,
            heading,
            passage: String::from_utf8(self.objects.read_verified(&hash)?)
                .map_err(|_| RetrievalError::InvalidChunkText)?,
            coordinates: parse_coordinates(coordinates.as_deref()),
            retrieved_at,
        }))
    }
}

struct Candidate {
    score: f32,
    chunk_id: String,
}

struct LexicalFields {
    chunk_id: Field,
    source_id: Field,
    version_id: Field,
    ordinal: Field,
    display_name: Field,
    heading: Field,
    body: Field,
}

struct LexicalIndex {
    index: Index,
    reader: IndexReader,
    fields: LexicalFields,
}

impl LexicalIndex {
    fn open(objects: &ObjectStore) -> Result<Self, RetrievalError> {
        let vault = objects.vault();
        vault.ensure_mounted()?;
        let path = vault.resolve_internal("indexes/tantivy");
        let schema = lexical_schema();
        let index = if path.join("meta.json").exists() {
            match Index::open_in_dir(&path) {
                Ok(index) if index.schema() == schema => index,
                _ => {
                    let quarantine = vault
                        .resolve_internal(format!("trash/tantivy-incompatible-{}", Uuid::new_v4()));
                    fs::rename(&path, quarantine)?;
                    fs::create_dir_all(&path)?;
                    Index::create_in_dir(&path, schema.clone())?
                }
            }
        } else {
            fs::create_dir_all(&path)?;
            Index::create_in_dir(&path, schema.clone())?
        };
        let fields = LexicalFields::from_schema(&schema)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::OnCommitWithDelay)
            .try_into()?;
        Ok(Self {
            index,
            reader,
            fields,
        })
    }

    fn num_docs(&self) -> Result<u64, RetrievalError> {
        self.reader.reload()?;
        Ok(self.reader.searcher().num_docs())
    }

    fn replace_version(
        &self,
        version_id: Uuid,
        chunks: &[IndexedChunk],
    ) -> Result<(), RetrievalError> {
        let mut writer = self.index.writer(WRITER_MEMORY_BYTES)?;
        writer.delete_term(Term::from_field_text(
            self.fields.version_id,
            &version_id.to_string(),
        ));
        for chunk in chunks {
            writer.add_document(self.document(chunk))?;
        }
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    fn rebuild(&self, chunks: &[IndexedChunk]) -> Result<(), RetrievalError> {
        let mut writer = self.index.writer(WRITER_MEMORY_BYTES)?;
        writer.delete_all_documents()?;
        for chunk in chunks {
            writer.add_document(self.document(chunk))?;
        }
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    fn document(&self, chunk: &IndexedChunk) -> TantivyDocument {
        let mut doc = TantivyDocument::default();
        doc.add_text(self.fields.chunk_id, chunk.chunk_id.to_string());
        doc.add_text(self.fields.source_id, chunk.source_id.to_string());
        doc.add_text(self.fields.version_id, chunk.version_id.to_string());
        doc.add_u64(self.fields.ordinal, chunk.ordinal);
        doc.add_text(self.fields.display_name, &chunk.display_name);
        if let Some(heading) = &chunk.heading {
            doc.add_text(self.fields.heading, heading);
        }
        doc.add_text(self.fields.body, &chunk.body);
        doc
    }

    fn search(&self, query: &str, limit: usize) -> Result<Vec<Candidate>, RetrievalError> {
        self.reader.reload()?;
        let searcher = self.reader.searcher();
        let parser = QueryParser::for_index(
            &self.index,
            vec![
                self.fields.display_name,
                self.fields.heading,
                self.fields.body,
            ],
        );
        let (query, _) = parser.parse_query_lenient(query);
        let docs = searcher.search(&query, &TopDocs::with_limit(limit).order_by_score())?;
        docs.into_iter()
            .map(|(score, address)| {
                let doc: TantivyDocument = searcher.doc(address)?;
                let chunk_id = doc
                    .get_first(self.fields.chunk_id)
                    .and_then(|value| value.as_value().as_str())
                    .ok_or(TantivyError::InvalidArgument(
                        "indexed chunk has no identifier".into(),
                    ))?;
                Ok(Candidate {
                    score,
                    chunk_id: chunk_id.to_owned(),
                })
            })
            .collect::<Result<Vec<_>, TantivyError>>()
            .map_err(Into::into)
    }
}

impl LexicalFields {
    fn from_schema(schema: &Schema) -> Result<Self, TantivyError> {
        Ok(Self {
            chunk_id: schema.get_field("chunk_id")?,
            source_id: schema.get_field("source_id")?,
            version_id: schema.get_field("version_id")?,
            ordinal: schema.get_field("ordinal")?,
            display_name: schema.get_field("display_name")?,
            heading: schema.get_field("heading")?,
            body: schema.get_field("body")?,
        })
    }
}

fn lexical_schema() -> Schema {
    let mut builder = Schema::builder();
    builder.add_text_field("chunk_id", STRING | STORED);
    builder.add_text_field("source_id", STRING | STORED);
    builder.add_text_field("version_id", STRING | STORED);
    builder.add_u64_field("ordinal", STORED);
    builder.add_text_field("display_name", TEXT | STORED);
    builder.add_text_field("heading", TEXT | STORED);
    builder.add_text_field("body", TEXT);
    builder.build()
}

fn citation_uri(source_id: Uuid, version_id: Uuid, ordinal: u64) -> String {
    format!("pinky://source/{source_id}/version/{version_id}#chunk-{ordinal}")
}

fn parse_citation_uri(uri: &str) -> Result<(Uuid, Uuid, u64), RetrievalError> {
    let suffix = uri
        .strip_prefix("pinky://source/")
        .ok_or(RetrievalError::InvalidCitation)?;
    let (path, fragment) = suffix
        .split_once('#')
        .ok_or(RetrievalError::InvalidCitation)?;
    let (source, version) = path
        .split_once("/version/")
        .ok_or(RetrievalError::InvalidCitation)?;
    let ordinal = fragment
        .strip_prefix("chunk-")
        .ok_or(RetrievalError::InvalidCitation)?
        .parse()
        .map_err(|_| RetrievalError::InvalidCitation)?;
    Ok((
        Uuid::parse_str(source).map_err(|_| RetrievalError::InvalidCitation)?,
        Uuid::parse_str(version).map_err(|_| RetrievalError::InvalidCitation)?,
        ordinal,
    ))
}

fn parse_uuid(value: &str) -> Result<Uuid, RetrievalError> {
    Uuid::parse_str(value).map_err(|_| RetrievalError::Database(rusqlite::Error::InvalidQuery))
}

fn parse_coordinates(value: Option<&str>) -> serde_json::Value {
    value
        .and_then(|json| serde_json::from_str(json).ok())
        .unwrap_or(serde_json::Value::Null)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MountVerifier, Vault};
    use std::path::Path;
    use zeroize::Zeroizing;

    struct Mounted;
    impl MountVerifier for Mounted {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(true)
        }
    }

    #[test]
    fn citation_uri_round_trips() {
        let source = Uuid::new_v4();
        let version = Uuid::new_v4();
        let uri = citation_uri(source, version, 17);
        assert_eq!(parse_citation_uri(&uri).unwrap(), (source, version, 17));
        assert!(parse_citation_uri("https://example.com").is_err());
    }

    #[test]
    fn indexes_searches_and_reopens_exact_retained_passage() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, Zeroizing::new(vec![0x42; 32])).unwrap(),
        ));
        let objects = ObjectStore::new(vault);
        let approved = tempfile::tempdir().unwrap();
        let path = approved.path().join("facts.txt");
        fs::write(
            &path,
            "Pinky stores exact retained passages about lunar geology.",
        )
        .unwrap();
        let ingestor = crate::LocalIngestor::new(database.clone(), objects.clone());
        ingestor.ingest(approved.path(), &path).unwrap();
        let retrieval = RetrievalService::new(database, objects);
        let retained = retrieval.current_chunks().unwrap();
        assert_eq!(retained.len(), 1);
        assert_eq!(
            retained[0].body,
            "Pinky stores exact retained passages about lunar geology."
        );
        let hits = retrieval.search("lunar geology", 10).unwrap();
        assert_eq!(hits.len(), 1);
        let passage = retrieval.open_citation(&hits[0].citation_uri).unwrap();
        assert_eq!(
            passage.passage,
            "Pinky stores exact retained passages about lunar geology."
        );
        assert_eq!(passage.version_id, hits[0].version_id);
    }

    #[test]
    fn embedding_backfill_marks_only_current_chunks_for_the_selected_identity() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, Zeroizing::new(vec![0x43; 32])).unwrap(),
        ));
        let objects = ObjectStore::new(vault);
        let approved = tempfile::tempdir().unwrap();
        let path = approved.path().join("facts.txt");
        fs::write(&path, "A retained passage for embedding backfill.").unwrap();
        let ingestor = crate::LocalIngestor::new(database.clone(), objects.clone());
        ingestor.ingest(approved.path(), &path).unwrap();
        let retrieval = RetrievalService::new(database, objects);
        let chunk = retrieval.current_chunks().unwrap().pop().unwrap();

        assert_eq!(
            retrieval
                .current_chunks_needing_embedding("ollama:test-model")
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            retrieval
                .mark_chunks_embedded(&[chunk.chunk_id], "ollama:test-model")
                .unwrap(),
            1
        );
        assert!(retrieval
            .current_chunks_needing_embedding("ollama:test-model")
            .unwrap()
            .is_empty());
        assert_eq!(
            retrieval
                .current_chunks_needing_embedding("ollama:other-model")
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn opens_exact_retained_image_bytes_for_an_image_citation() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, Zeroizing::new(vec![0x42; 32])).unwrap(),
        ));
        let objects = ObjectStore::new(vault);
        let approved = tempfile::tempdir().unwrap();
        let path = approved.path().join("diagram.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend_from_slice(&[0, 0, 1, 0, 0, 0, 1, 0, 8, 6, 0, 0, 0, 0]);
        fs::write(&path, &bytes).unwrap();
        let ingestor = crate::LocalIngestor::new(database.clone(), objects.clone());
        let retained = ingestor.ingest(approved.path(), &path).unwrap();
        let retrieval = RetrievalService::new(database, objects);
        let citation = retrieval.search("PNG", 1).unwrap().remove(0).citation_uri;
        let image = retrieval.open_retained_image(&citation).unwrap();
        assert_eq!(image.version_id, retained.version_id);
        assert_eq!(image.mime_type, "image/png");
        assert_eq!(image.byte_size, bytes.len());
        assert_eq!(STANDARD.decode(image.bytes_base64).unwrap(), bytes);
        let invalid = citation.replace("#chunk-0", "#chunk-99");
        assert!(matches!(
            retrieval.open_retained_image(&invalid),
            Err(RetrievalError::CitationNotFound)
        ));
    }

    #[test]
    fn merges_lexical_and_vector_hits_without_losing_citations() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let database = Arc::new(Mutex::new(
            Database::open(&vault, Zeroizing::new(vec![0x42; 32])).unwrap(),
        ));
        let retrieval = RetrievalService::new(database, ObjectStore::new(vault));
        let source_id = Uuid::new_v4();
        let version_id = Uuid::new_v4();
        let chunk_id = Uuid::new_v4();
        let citation_uri = citation_uri(source_id, version_id, 2);
        let merged = retrieval
            .merge_hybrid_hits(
                &[SearchHit {
                    score: 2.0,
                    citation_uri: citation_uri.clone(),
                    source_id,
                    version_id,
                    chunk_id,
                    ordinal: 2,
                    display_name: "notes.md".into(),
                    heading: Some("Heading".into()),
                    passage: "Retained passage".into(),
                    coordinates: serde_json::json!({"line_start": 3}),
                    retrieved_at: "2026-09-15T00:00:00Z".into(),
                }],
                &[VectorMatch {
                    id: chunk_id,
                    source_version_id: version_id,
                    score: 0.98,
                }],
                1,
            )
            .unwrap();
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].citation_uri, citation_uri);
        assert!(merged[0].score > 0.0);
    }

    #[test]
    #[ignore = "target-host acceptance: builds one million chunks and measures warm lexical p95"]
    fn one_million_chunk_warm_lexical_p95_stays_below_half_second() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Mounted).unwrap();
        let objects = ObjectStore::new(vault);
        let lexical = LexicalIndex::open(&objects).unwrap();
        let source = Uuid::new_v4();
        let version = Uuid::new_v4();
        let chunks = (0..1_000_000_u64)
            .map(|ordinal| IndexedChunk {
                chunk_id: Uuid::new_v4(),
                source_id: source,
                version_id: version,
                ordinal,
                display_name: format!("source-{ordinal}.md"),
                heading: Some("retrieval benchmark".into()),
                body: format!(
                    "Retained benchmark passage {ordinal} contains a stable searchable token."
                ),
            })
            .collect::<Vec<_>>();
        lexical.rebuild(&chunks).unwrap();
        let mut timings = Vec::with_capacity(100);
        for _ in 0..100 {
            let started = std::time::Instant::now();
            let results = lexical.search("stable searchable token", 50).unwrap();
            assert_eq!(results.len(), 50);
            timings.push(started.elapsed());
        }
        timings.sort_unstable();
        let p95 = timings[timings.len() * 95 / 100];
        assert!(
            p95 < std::time::Duration::from_millis(500),
            "warm lexical p95 was {p95:?}"
        );
    }
}
