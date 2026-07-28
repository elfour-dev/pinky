//! Security-sensitive orchestration primitives for Pinky.
//!
//! Core APIs require a verified, mounted vault handle. This makes it difficult
//! for callers to accidentally persist source material outside the encrypted
//! boundary.

pub mod database;
pub mod object_store;
pub mod task;
pub mod vault;

pub use database::{Database, DatabaseError};
pub use object_store::{CompressionClass, ObjectMetadata, ObjectStore, StoredObject};
pub use task::{TaskEvent, TaskManager, TaskPhase, TaskState};
pub use vault::{MountVerifier, ProcMountVerifier, Vault, VaultError};
