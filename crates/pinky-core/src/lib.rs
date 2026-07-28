//! Security-sensitive orchestration primitives for Pinky.
//!
//! Core APIs require a verified, mounted vault handle. This makes it difficult
//! for callers to accidentally persist source material outside the encrypted
//! boundary.

pub mod database;
pub mod object_store;
pub mod onboarding;
pub mod recovery;
pub mod task;
pub mod task_journal;
pub mod vault;

pub use database::{Database, DatabaseError};
pub use object_store::{
    CompressionClass, ObjectMetadata, ObjectStore, ObjectStoreError, StoredObject,
};
pub use onboarding::{
    create_registered_vault, create_vault, read_registration, unlock_registered_vault,
    write_registration, GocryptfsMount, OnboardedVault, OnboardingError, SystemVaultPlatform,
    VaultPaths, VaultPlatform, VaultRegistration,
};
pub use recovery::{RecoveryEnvelope, RecoveryError, VaultKey, VaultSubkeys};
pub use task::{TaskEvent, TaskManager, TaskPhase, TaskState};
pub use task_journal::{TaskJournal, TaskJournalError};
pub use vault::{MountVerifier, ProcMountVerifier, Vault, VaultError};
