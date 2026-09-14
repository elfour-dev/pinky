//! Security-sensitive orchestration primitives for Pinky.
//!
//! Core APIs require a verified, mounted vault handle. This makes it difficult
//! for callers to accidentally persist source material outside the encrypted
//! boundary.

pub mod database;
pub mod hybrid;
pub mod ingestion;
pub mod object_store;
pub mod onboarding;
pub mod process;
pub mod qdrant;
pub mod recovery;
pub mod retrieval;
pub mod task;
pub mod task_journal;
pub mod vault;

pub use database::{Database, DatabaseError};
pub use hybrid::{
    reciprocal_rank_fusion, FusedChunk, RankedChunk, MAX_CHUNKS_PER_SOURCE_VERSION, RRF_K,
};
pub use ingestion::{
    IngestedSource, IngestionError, LocalFileFingerprint, LocalIngestor, LocalWatchTarget,
    SourceSummary,
};
pub use object_store::{
    CompressionClass, ObjectMetadata, ObjectStore, ObjectStoreError, StoredObject,
};
pub use onboarding::{
    create_registered_vault, create_vault, read_registration, unlock_registered_vault,
    write_registration, GocryptfsMount, OnboardedVault, OnboardingError, SystemVaultPlatform,
    VaultPaths, VaultPlatform, VaultRegistration,
};
pub use process::{ProcessError, ProcessOutcome, ProcessSupervisor, ProcessTermination};
pub use qdrant::{
    QdrantClient, QdrantError, QdrantLaunchConfig, QdrantSidecar, VectorMatch, VectorPoint,
};
pub use recovery::{RecoveryEnvelope, RecoveryError, VaultKey, VaultSubkeys};
pub use retrieval::{CitationPassage, IndexedChunk, RetrievalError, RetrievalService, SearchHit};
pub use task::{TaskEvent, TaskManager, TaskPhase, TaskState};
pub use task_journal::{TaskJournal, TaskJournalError};
pub use vault::{MountVerifier, ProcMountVerifier, Vault, VaultError};
