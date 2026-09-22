//! Security-sensitive orchestration primitives for Pinky.
//!
//! Core APIs require a verified, mounted vault handle. This makes it difficult
//! for callers to accidentally persist source material outside the encrypted
//! boundary.

pub mod artifact;
pub mod conversation;
pub mod database;
pub mod embedding;
pub mod hybrid;
pub mod image;
pub mod inference;
pub mod ingestion;
pub mod llama;
pub mod object_store;
pub mod ollama;
pub mod onboarding;
pub mod pdf;
pub mod process;
pub mod qa;
pub mod qdrant;
pub mod recovery;
pub mod reranking;
pub mod research;
pub mod retrieval;
pub mod task;
pub mod task_journal;
pub mod vault;

pub use artifact::{
    ArtifactDigest, ArtifactError, ArtifactKind, ArtifactManifestV1, SignedArtifactManifestV1,
    ARTIFACT_MANIFEST_SCHEMA_VERSION,
};
pub use conversation::{
    ConversationDetail, ConversationError, ConversationMessage, ConversationService,
    ConversationSummary, MessageDraft,
};
pub use database::{Database, DatabaseError, HybridConfiguration, OllamaConfiguration};
pub use embedding::{
    EmbeddingError, EmbeddingFuture, EmbeddingIndexError, EmbeddingIndexer, EmbeddingProvider,
    EmbeddingResponse, DEFAULT_EMBEDDING_BATCH_SIZE, MAX_EMBEDDING_ERROR_BYTES,
    MAX_EMBEDDING_INPUTS, MAX_EMBEDDING_INPUT_BYTES, MAX_EMBEDDING_RESPONSE_BYTES,
};
pub use hybrid::{
    reciprocal_rank_fusion, FusedChunk, RankedChunk, MAX_CHUNKS_PER_SOURCE_VERSION, RRF_K,
};
pub use image::{
    extract_image_metadata, ImageMetadata, ImageMetadataError, ImageOcrError, ImageOcrWorker,
    AUTO_OCR_CONFIDENCE_THRESHOLDS, AUTO_OCR_PAGE_SEGMENTATION_MODES, DEFAULT_OCR_MIN_CONFIDENCE,
    DEFAULT_OCR_PAGE_SEGMENTATION_MODE, IMAGE_METADATA_VERSION, MAX_IMAGE_METADATA_BYTES,
    MAX_OCR_TEXT_BYTES, OCR_WORKER_FILE_BYTES, OCR_WORKER_MEMORY_BYTES,
};
pub use inference::{
    InferenceError, InferenceFuture, InferenceMetrics, InferenceProvider, InferenceResponse,
    StructuredGenerationRequest, MAX_INFERENCE_REQUEST_BYTES, MAX_INFERENCE_RESPONSE_BYTES,
    MAX_OUTPUT_TOKENS, MAX_PROMPT_BYTES, MAX_SCHEMA_BYTES, MAX_SYSTEM_BYTES,
};
pub use ingestion::{
    AssetListQuery, AssetListResponse, IngestedSource, IngestionError, LocalFileFingerprint,
    LocalIngestor, LocalWatchTarget, SourceSummary,
};
pub use llama::{LlamaClient, LlamaError, LlamaHealth, LlamaRuntimeInfo, MIN_CHAT_CONTEXT};
pub use object_store::{
    CompressionClass, ObjectMetadata, ObjectStore, ObjectStoreError, StoredObject,
};
pub use ollama::{OllamaClient, OllamaEmbeddingRuntimeInfo, OllamaError, OllamaRuntimeInfo};
pub use onboarding::{
    create_registered_vault, create_vault, read_registration, unlock_registered_vault,
    write_registration, GocryptfsMount, OnboardedVault, OnboardingError, SystemVaultPlatform,
    VaultPaths, VaultPlatform, VaultRegistration,
};
pub use pdf::{
    PdfError, PdfExtraction, PdfTextExtractor, MAX_PDF_INPUT_BYTES, MAX_PDF_PAGES,
    MAX_PDF_RENDERED_IMAGE_BYTES, MAX_PDF_RENDERED_PIXELS, MAX_PDF_TEXT_BYTES,
    PDF_EXTRACTION_VERSION, PDF_WORKER_FILE_BYTES, PDF_WORKER_MEMORY_BYTES,
};
pub use process::{ProcessError, ProcessOutcome, ProcessSupervisor, ProcessTermination};
pub use qa::{
    answer_question, answer_question_with_history, select_evidence, validate_answer, AnswerClaimV1,
    AnswerEnvelopeV1, ClaimSupportV1, ConversationTurnV1, EvidenceV1, ModelAnswerV1, ModelClaimV1,
    QaError, QuestionLimitsV1, QuestionRequestV1, MAX_ANSWER_CLAIMS,
    MAX_CHUNKS_PER_EVIDENCE_VERSION, MAX_CONVERSATION_BYTES, MAX_CONVERSATION_MESSAGES,
    MAX_EVIDENCE_CHUNKS, MAX_EVIDENCE_TOKENS, QA_SCHEMA_VERSION,
};
pub use qdrant::{
    QdrantClient, QdrantError, QdrantLaunchConfig, QdrantSidecar, VectorMatch, VectorPoint,
};
pub use recovery::{RecoveryEnvelope, RecoveryError, VaultKey, VaultSubkeys};
pub use reranking::{rerank_hits, MAX_RERANK_CANDIDATES, MAX_RERANK_RESULTS};
pub use research::{
    build_public_client, fetch_public_body, normalize_search_queries, PublicSearchResult,
    PublicUrl, PublicWebPolicy, ResearchBudgetV1, ResearchError, ResolvedPublicUrl, SearxngClient,
    DEFAULT_CRAWL_DEPTH, DEFAULT_MAX_CONCURRENT_REQUESTS, DEFAULT_MAX_DOCUMENTS,
    DEFAULT_MAX_DOWNLOAD_BYTES, DEFAULT_MAX_DURATION_SECONDS, DEFAULT_MAX_PAGES_PER_ORIGIN,
    DEFAULT_MIN_ORIGIN_INTERVAL, MAX_SEARCH_QUERIES, MAX_SEARCH_QUERY_BYTES,
    MAX_SEARCH_RESPONSE_BYTES, MAX_SEARCH_RESULTS, MAX_SEARCH_TEXT_BYTES, RESEARCH_SCHEMA_VERSION,
};
pub use retrieval::{
    CitationPassage, HybridRetrievalError, IndexedChunk, RetainedImage, RetrievalError,
    RetrievalService, SearchHit, HYBRID_CANDIDATE_LIMIT,
};
pub use task::{TaskContext, TaskEvent, TaskManager, TaskPhase, TaskState};
pub use task_journal::{TaskJournal, TaskJournalError};
pub use vault::{MountVerifier, ProcMountVerifier, Vault, VaultError};
