use std::{
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use thiserror::Error;

const REQUIRED_DIRECTORIES: &[&str] = &[
    "objects",
    "database",
    "indexes",
    "logs",
    "snapshots",
    "trash",
];

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("vault path is not an absolute path: {0}")]
    RelativePath(PathBuf),
    #[error("vault path does not exist or is not a directory: {0}")]
    Missing(PathBuf),
    #[error("vault is not a verified gocryptfs mount: {0}")]
    NotEncryptedMount(PathBuf),
    #[error("vault layout entry escapes the encrypted mount: {0}")]
    UnsafeLayout(PathBuf),
    #[error("vault I/O error: {0}")]
    Io(#[from] std::io::Error),
}

/// The mount check is injectable so the boundary can be tested without mounting
/// FUSE inside a test runner. Production always uses [`ProcMountVerifier`].
pub trait MountVerifier: Send + Sync {
    fn is_gocryptfs_mount(&self, path: &Path) -> Result<bool, std::io::Error>;
}

#[derive(Debug, Default)]
pub struct ProcMountVerifier;

impl MountVerifier for ProcMountVerifier {
    fn is_gocryptfs_mount(&self, path: &Path) -> Result<bool, std::io::Error> {
        let canonical = fs::canonicalize(path)?;
        let mounts = fs::read_to_string("/proc/self/mountinfo")?;

        Ok(mounts.lines().any(|line| {
            let Some((before, after)) = line.split_once(" - ") else {
                return false;
            };
            let mount_point = before.split_whitespace().nth(4).unwrap_or_default();
            let fs_type = after.split_whitespace().next().unwrap_or_default();
            Path::new(mount_point) == canonical
                && matches!(fs_type, "fuse.gocryptfs" | "fuse.gocryptfs-reverse")
        }))
    }
}

/// Capability proving that the encrypted vault is currently mounted.
#[derive(Clone)]
pub struct Vault {
    root: Arc<PathBuf>,
    verifier: Arc<dyn MountVerifier>,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Vault")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

impl Vault {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, VaultError> {
        Self::open_with(path, ProcMountVerifier)
    }

    pub fn open_with<V: MountVerifier + 'static>(
        path: impl AsRef<Path>,
        verifier: V,
    ) -> Result<Self, VaultError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(VaultError::RelativePath(path.to_owned()));
        }
        if !path.is_dir() {
            return Err(VaultError::Missing(path.to_owned()));
        }
        let root = fs::canonicalize(path)?;
        if !verifier.is_gocryptfs_mount(&root)? {
            return Err(VaultError::NotEncryptedMount(root));
        }

        for directory in REQUIRED_DIRECTORIES {
            let child = root.join(directory);
            if fs::symlink_metadata(&child)
                .map(|metadata| metadata.file_type().is_symlink())
                .unwrap_or(false)
            {
                return Err(VaultError::UnsafeLayout(child));
            }
            fs::create_dir_all(&child)?;
            if !fs::canonicalize(&child)?.starts_with(&root) {
                return Err(VaultError::UnsafeLayout(child));
            }
            set_owner_only(&child)?;
        }

        Ok(Self {
            root: Arc::new(root),
            verifier: Arc::new(verifier),
        })
    }

    pub fn root(&self) -> &Path {
        self.root.as_path()
    }

    /// Revalidate at each persistence boundary so an unmounted vault cannot
    /// silently degrade into plaintext writes to its former mountpoint.
    pub fn ensure_mounted(&self) -> Result<(), VaultError> {
        if self.verifier.is_gocryptfs_mount(self.root())? {
            Ok(())
        } else {
            Err(VaultError::NotEncryptedMount(self.root().to_owned()))
        }
    }

    pub(crate) fn resolve_internal(&self, relative: impl AsRef<Path>) -> PathBuf {
        debug_assert!(!relative.as_ref().is_absolute());
        self.root.join(relative)
    }
}

#[cfg(unix)]
fn set_owner_only(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Answer(bool);
    impl MountVerifier for Answer {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(self.0)
        }
    }

    #[test]
    fn refuses_unencrypted_directories() {
        let root = tempfile::tempdir().unwrap();
        let error = Vault::open_with(root.path(), Answer(false)).unwrap_err();
        assert!(matches!(error, VaultError::NotEncryptedMount(_)));
    }

    #[test]
    fn initializes_layout_only_after_mount_verification() {
        let root = tempfile::tempdir().unwrap();
        let vault = Vault::open_with(root.path(), Answer(true)).unwrap();
        for child in REQUIRED_DIRECTORIES {
            assert!(vault.root().join(child).is_dir());
        }
    }

    #[cfg(unix)]
    #[test]
    fn refuses_layout_symlink_escapes() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        symlink(outside.path(), root.path().join("objects")).unwrap();
        assert!(matches!(
            Vault::open_with(root.path(), Answer(true)),
            Err(VaultError::UnsafeLayout(_))
        ));
    }
}
