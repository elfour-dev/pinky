use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

use crate::{
    Database, DatabaseError, MountVerifier, ProcMountVerifier, RecoveryEnvelope, RecoveryError,
    Vault, VaultError, VaultKey,
};

const RECOVERY_FILE: &str = ".pinky-recovery.json";
const REGISTRATION_VERSION: u16 = 1;
const MAX_REGISTRATION_BYTES: u64 = 64 * 1024;
const MOUNT_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VaultPaths {
    pub cipher_dir: PathBuf,
    pub mount_dir: PathBuf,
}

#[derive(Debug, Error)]
pub enum OnboardingError {
    #[error("vault paths must be absolute, normalized, distinct directories")]
    InvalidPaths,
    #[error("vault setup requires an empty destination: {0}")]
    DestinationNotEmpty(PathBuf),
    #[error("required program is unavailable: {0}")]
    MissingPrerequisite(&'static str),
    #[error("vault registration is invalid: {0}")]
    InvalidRegistration(String),
    #[error("{operation} failed: {message}")]
    Platform {
        operation: &'static str,
        message: String,
    },
    #[error("recovery setup failed: {0}")]
    Recovery(#[from] RecoveryError),
    #[error("vault verification failed: {0}")]
    Vault(#[from] VaultError),
    #[error("encrypted database setup failed: {0}")]
    Database(#[from] DatabaseError),
    #[error("vault setup I/O failed: {0}")]
    Io(#[from] std::io::Error),
}

pub trait VaultPlatform {
    type Mount: Send + 'static;
    type Verifier: MountVerifier + 'static;

    fn check_prerequisites(&self) -> Result<(), OnboardingError>;
    fn initialize_gocryptfs(
        &self,
        cipher_dir: &Path,
        password: &str,
    ) -> Result<(), OnboardingError>;
    fn store_root_key(&self, vault_id: Uuid, root_key: &[u8; 32]) -> Result<(), OnboardingError>;
    fn load_root_key(&self, vault_id: Uuid) -> Result<VaultKey, OnboardingError>;
    fn clear_root_key(&self, vault_id: Uuid);
    fn mount_gocryptfs(
        &self,
        paths: &VaultPaths,
        password: &str,
    ) -> Result<Self::Mount, OnboardingError>;
    fn verifier(&self) -> Self::Verifier;
}

pub struct OnboardedVault<M> {
    pub id: Uuid,
    pub vault: Vault,
    pub database: Arc<Mutex<Database>>,
    pub recovery_path: PathBuf,
    pub mount: M,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VaultRegistration {
    pub version: u16,
    pub vault_id: Uuid,
    pub paths: VaultPaths,
    pub recovery_path: PathBuf,
}

impl<M> OnboardedVault<M> {
    pub fn registration(&self, paths: VaultPaths) -> VaultRegistration {
        VaultRegistration {
            version: REGISTRATION_VERSION,
            vault_id: self.id,
            paths,
            recovery_path: self.recovery_path.clone(),
        }
    }
}

impl VaultRegistration {
    pub fn validate(&self) -> Result<(), OnboardingError> {
        if self.version != REGISTRATION_VERSION {
            return Err(OnboardingError::InvalidRegistration(format!(
                "unsupported version {}",
                self.version
            )));
        }
        self.paths.validate()?;
        if self.recovery_path != self.paths.cipher_dir.join(RECOVERY_FILE) {
            return Err(OnboardingError::InvalidRegistration(
                "recovery path does not belong to the registered vault".into(),
            ));
        }
        Ok(())
    }
}

impl VaultPaths {
    pub fn validate(&self) -> Result<(), OnboardingError> {
        validate_path(&self.cipher_dir)?;
        validate_path(&self.mount_dir)?;
        if canonical_destination(&self.cipher_dir)? != self.cipher_dir
            || canonical_destination(&self.mount_dir)? != self.mount_dir
        {
            return Err(OnboardingError::InvalidPaths);
        }
        if self.cipher_dir == self.mount_dir
            || self.cipher_dir.starts_with(&self.mount_dir)
            || self.mount_dir.starts_with(&self.cipher_dir)
        {
            return Err(OnboardingError::InvalidPaths);
        }
        Ok(())
    }
}

pub fn create_vault<P: VaultPlatform>(
    platform: &P,
    paths: VaultPaths,
    recovery_passphrase: &str,
) -> Result<OnboardedVault<P::Mount>, OnboardingError> {
    create_vault_inner(platform, paths, recovery_passphrase, None)
}

pub fn create_registered_vault<P: VaultPlatform>(
    platform: &P,
    paths: VaultPaths,
    recovery_passphrase: &str,
    registration_path: &Path,
) -> Result<OnboardedVault<P::Mount>, OnboardingError> {
    validate_path(registration_path)?;
    create_vault_inner(
        platform,
        paths,
        recovery_passphrase,
        Some(registration_path),
    )
}

fn create_vault_inner<P: VaultPlatform>(
    platform: &P,
    paths: VaultPaths,
    recovery_passphrase: &str,
    registration_path: Option<&Path>,
) -> Result<OnboardedVault<P::Mount>, OnboardingError> {
    paths.validate()?;
    platform.check_prerequisites()?;
    ensure_empty_or_missing(&paths.cipher_dir)?;
    ensure_empty_or_missing(&paths.mount_dir)?;

    let vault_id = Uuid::new_v4();
    let root_key = VaultKey::generate();
    let subkeys = root_key.derive_subkeys()?;
    let recovery = RecoveryEnvelope::wrap(vault_id, &root_key, recovery_passphrase)?;
    let gocryptfs_password = subkeys.gocryptfs_password();
    let database_key = subkeys.database_key();

    let mut rollback = SetupRollback::new(platform, vault_id, paths.clone());
    rollback.cipher_created = create_private_directory(&paths.cipher_dir)?;
    rollback.mount_created = create_private_directory(&paths.mount_dir)?;

    platform.initialize_gocryptfs(&paths.cipher_dir, &gocryptfs_password)?;
    let recovery_path = paths.cipher_dir.join(RECOVERY_FILE);
    atomic_write_json(&recovery_path, &recovery)?;
    platform.store_root_key(vault_id, root_key.expose())?;
    rollback.secret_stored = true;

    let mount = platform.mount_gocryptfs(&paths, &gocryptfs_password)?;
    let vault = Vault::open_with(&paths.mount_dir, platform.verifier())?;
    let database = Database::open(&vault, database_key)?;
    if let Some(registration_path) = registration_path {
        let registration = VaultRegistration {
            version: REGISTRATION_VERSION,
            vault_id,
            paths: paths.clone(),
            recovery_path: recovery_path.clone(),
        };
        write_registration(registration_path, &registration)?;
        rollback.registration_path = Some(registration_path.to_owned());
    }
    rollback.complete = true;

    Ok(OnboardedVault {
        id: vault_id,
        vault,
        database: Arc::new(Mutex::new(database)),
        recovery_path,
        mount,
    })
}

pub fn write_registration(
    path: &Path,
    registration: &VaultRegistration,
) -> Result<(), OnboardingError> {
    validate_path(path)?;
    registration.validate()?;
    if path.exists() {
        return Err(OnboardingError::InvalidRegistration(
            "registration already exists".into(),
        ));
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if canonical_destination(path)? != path {
        return Err(OnboardingError::InvalidPaths);
    }
    if let Some(parent) = path.parent() {
        set_owner_only(parent, true)?;
    }
    let result = atomic_write_json(path, registration);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

pub fn read_registration(path: &Path) -> Result<VaultRegistration, OnboardingError> {
    validate_path(path)?;
    if canonical_destination(path)? != path {
        return Err(OnboardingError::InvalidPaths);
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_REGISTRATION_BYTES
    {
        return Err(OnboardingError::InvalidRegistration(
            "registration is not a bounded regular file".into(),
        ));
    }
    let bytes = fs::read(path)?;
    let registration: VaultRegistration = serde_json::from_slice(&bytes).map_err(|error| {
        OnboardingError::InvalidRegistration(format!("malformed JSON: {error}"))
    })?;
    registration.validate()?;
    validate_recovery_identity(&registration)?;
    Ok(registration)
}

pub fn unlock_registered_vault<P: VaultPlatform>(
    platform: &P,
    registration: &VaultRegistration,
) -> Result<OnboardedVault<P::Mount>, OnboardingError> {
    registration.validate()?;
    validate_recovery_identity(registration)?;
    platform.check_prerequisites()?;

    let cipher_metadata = fs::symlink_metadata(&registration.paths.cipher_dir)?;
    if cipher_metadata.file_type().is_symlink() || !cipher_metadata.is_dir() {
        return Err(OnboardingError::InvalidRegistration(
            "cipher directory is unavailable".into(),
        ));
    }
    if platform
        .verifier()
        .is_gocryptfs_mount(&registration.paths.mount_dir)?
    {
        return Err(OnboardingError::InvalidRegistration(
            "registered mount directory is already mounted by another process".into(),
        ));
    }
    ensure_empty_or_missing(&registration.paths.mount_dir)?;
    let mount_created = create_private_directory(&registration.paths.mount_dir)?;

    let result = (|| {
        let root_key = platform.load_root_key(registration.vault_id)?;
        let subkeys = root_key.derive_subkeys()?;
        let password = subkeys.gocryptfs_password();
        let database_key = subkeys.database_key();
        let mount = platform.mount_gocryptfs(&registration.paths, &password)?;
        let vault = Vault::open_with(&registration.paths.mount_dir, platform.verifier())?;
        let database = Database::open(&vault, database_key)?;
        Ok(OnboardedVault {
            id: registration.vault_id,
            vault,
            database: Arc::new(Mutex::new(database)),
            recovery_path: registration.recovery_path.clone(),
            mount,
        })
    })();

    if result.is_err() && mount_created {
        let _ = fs::remove_dir(&registration.paths.mount_dir);
    }
    result
}

fn validate_recovery_identity(registration: &VaultRegistration) -> Result<(), OnboardingError> {
    let metadata = fs::symlink_metadata(&registration.recovery_path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_REGISTRATION_BYTES
    {
        return Err(OnboardingError::InvalidRegistration(
            "recovery envelope is not a bounded regular file".into(),
        ));
    }
    let recovery: RecoveryEnvelope =
        serde_json::from_slice(&fs::read(&registration.recovery_path)?).map_err(|error| {
            OnboardingError::InvalidRegistration(format!("recovery envelope is malformed: {error}"))
        })?;
    if recovery.vault_id != registration.vault_id {
        return Err(OnboardingError::InvalidRegistration(
            "vault and recovery identifiers do not match".into(),
        ));
    }
    Ok(())
}

struct SetupRollback<'a, P: VaultPlatform> {
    platform: &'a P,
    vault_id: Uuid,
    paths: VaultPaths,
    secret_stored: bool,
    cipher_created: bool,
    mount_created: bool,
    registration_path: Option<PathBuf>,
    complete: bool,
}

impl<'a, P: VaultPlatform> SetupRollback<'a, P> {
    fn new(platform: &'a P, vault_id: Uuid, paths: VaultPaths) -> Self {
        Self {
            platform,
            vault_id,
            paths,
            secret_stored: false,
            cipher_created: false,
            mount_created: false,
            registration_path: None,
            complete: false,
        }
    }
}

impl<P: VaultPlatform> Drop for SetupRollback<'_, P> {
    fn drop(&mut self) {
        if self.complete {
            return;
        }
        if self.secret_stored {
            self.platform.clear_root_key(self.vault_id);
        }
        if let Some(path) = &self.registration_path {
            let _ = fs::remove_file(path);
        }
        if self.mount_created {
            let _ = fs::remove_dir(&self.paths.mount_dir);
        }
        if self.cipher_created {
            // This directory was created by this failed transaction and cannot
            // contain user data. Never apply this cleanup to a pre-existing path.
            let _ = fs::remove_dir_all(&self.paths.cipher_dir);
        }
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemVaultPlatform;

impl VaultPlatform for SystemVaultPlatform {
    type Mount = GocryptfsMount;
    type Verifier = ProcMountVerifier;

    fn check_prerequisites(&self) -> Result<(), OnboardingError> {
        for program in ["gocryptfs", "secret-tool"] {
            if !executable_on_path(program) {
                return Err(OnboardingError::MissingPrerequisite(program));
            }
        }
        Ok(())
    }

    fn initialize_gocryptfs(
        &self,
        cipher_dir: &Path,
        password: &str,
    ) -> Result<(), OnboardingError> {
        run_with_secret_stdin(
            "gocryptfs",
            ["-q", "-init", "-passfile", "/dev/stdin"],
            Some(cipher_dir),
            password.as_bytes(),
            "gocryptfs initialization",
        )
    }

    fn store_root_key(&self, vault_id: Uuid, root_key: &[u8; 32]) -> Result<(), OnboardingError> {
        let secret = Zeroizing::new(STANDARD_NO_PAD.encode(root_key));
        let label = format!("Pinky vault {vault_id}");
        let status = Command::new("secret-tool")
            .arg("store")
            .arg(format!("--label={label}"))
            .args(["application", "pinky", "vault-id", &vault_id.to_string()])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .and_then(|mut child| {
                child
                    .stdin
                    .take()
                    .ok_or_else(|| std::io::Error::other("secret stdin unavailable"))?
                    .write_all(secret.as_bytes())?;
                child.wait_with_output()
            })?;
        status_to_result(status, "Secret Service storage")
    }

    fn load_root_key(&self, vault_id: Uuid) -> Result<VaultKey, OnboardingError> {
        load_root_key_from_secret_service(vault_id)
    }

    fn clear_root_key(&self, vault_id: Uuid) {
        let _ = Command::new("secret-tool")
            .args([
                "clear",
                "application",
                "pinky",
                "vault-id",
                &vault_id.to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    fn mount_gocryptfs(
        &self,
        paths: &VaultPaths,
        password: &str,
    ) -> Result<Self::Mount, OnboardingError> {
        let mut child = Command::new("gocryptfs")
            .args(["-fg", "-q", "-nosyslog", "-passfile", "/dev/stdin"])
            .arg(&paths.cipher_dir)
            .arg(&paths.mount_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()?;
        child
            .stdin
            .take()
            .ok_or_else(|| std::io::Error::other("gocryptfs stdin unavailable"))?
            .write_all(password.as_bytes())?;

        let started = Instant::now();
        let verifier = ProcMountVerifier;
        while started.elapsed() < MOUNT_TIMEOUT {
            if verifier.is_gocryptfs_mount(&paths.mount_dir)? {
                return Ok(GocryptfsMount {
                    child: Some(child),
                    mount_dir: paths.mount_dir.clone(),
                    mounted: true,
                });
            }
            if let Some(status) = child.try_wait()? {
                return Err(OnboardingError::Platform {
                    operation: "gocryptfs mount",
                    message: format!("process exited with {status}"),
                });
            }
            thread::sleep(Duration::from_millis(50));
        }
        let _ = child.kill();
        Err(OnboardingError::Platform {
            operation: "gocryptfs mount",
            message: "mount verification timed out".into(),
        })
    }

    fn verifier(&self) -> Self::Verifier {
        ProcMountVerifier
    }
}

pub struct GocryptfsMount {
    child: Option<Child>,
    mount_dir: PathBuf,
    mounted: bool,
}

impl std::fmt::Debug for GocryptfsMount {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("GocryptfsMount")
            .field("mount_dir", &self.mount_dir)
            .finish_non_exhaustive()
    }
}

impl GocryptfsMount {
    pub fn unmount(mut self) -> Result<(), OnboardingError> {
        unmount_path(&self.mount_dir)?;
        self.mounted = false;
        if let Some(mut child) = self.child.take() {
            let _ = child.wait();
        }
        Ok(())
    }
}

impl Drop for GocryptfsMount {
    fn drop(&mut self) {
        if self.mounted {
            let _ = unmount_path(&self.mount_dir);
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn unmount_path(path: &Path) -> Result<(), OnboardingError> {
    let program = if executable_on_path("fusermount3") {
        "fusermount3"
    } else {
        "fusermount"
    };
    let output = Command::new(program)
        .args(["-u", "--"])
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    status_to_result(output, "vault unmount")
}

fn run_with_secret_stdin<'a>(
    program: &str,
    arguments: impl IntoIterator<Item = &'a str>,
    trailing_path: Option<&Path>,
    secret: &[u8],
    operation: &'static str,
) -> Result<(), OnboardingError> {
    let mut command = Command::new(program);
    command.args(arguments);
    if let Some(path) = trailing_path {
        command.arg(path);
    }
    let output = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child
                .stdin
                .take()
                .ok_or_else(|| std::io::Error::other("secret stdin unavailable"))?
                .write_all(secret)?;
            child.wait_with_output()
        })?;
    status_to_result(output, operation)
}

fn status_to_result(
    output: std::process::Output,
    operation: &'static str,
) -> Result<(), OnboardingError> {
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr);
    Err(OnboardingError::Platform {
        operation,
        message: message.trim().chars().take(500).collect(),
    })
}

fn validate_path(path: &Path) -> Result<(), OnboardingError> {
    if !path.is_absolute()
        || path == Path::new("/")
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::CurDir | Component::Prefix(_)
            )
        })
    {
        return Err(OnboardingError::InvalidPaths);
    }
    Ok(())
}

fn canonical_destination(path: &Path) -> Result<PathBuf, OnboardingError> {
    let mut ancestor = path;
    let mut missing = Vec::new();
    while !ancestor.exists() {
        let name = ancestor.file_name().ok_or(OnboardingError::InvalidPaths)?;
        missing.push(name.to_owned());
        ancestor = ancestor.parent().ok_or(OnboardingError::InvalidPaths)?;
    }
    let mut resolved = fs::canonicalize(ancestor)?;
    for component in missing.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn ensure_empty_or_missing(path: &Path) -> Result<(), OnboardingError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            Err(OnboardingError::InvalidPaths)
        }
        Ok(_) if fs::read_dir(path)?.next().is_some() => {
            Err(OnboardingError::DestinationNotEmpty(path.to_owned()))
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn create_private_directory(path: &Path) -> Result<bool, OnboardingError> {
    let parent = path.parent().ok_or(OnboardingError::InvalidPaths)?;
    fs::create_dir_all(parent)?;
    let created = match fs::create_dir(path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => false,
        Err(error) => return Err(error.into()),
    };
    if fs::canonicalize(path)? != path {
        if created {
            let _ = fs::remove_dir(path);
        }
        return Err(OnboardingError::InvalidPaths);
    }
    set_owner_only(path, true)?;
    Ok(created)
}

fn atomic_write_json(path: &Path, value: &impl Serialize) -> Result<(), OnboardingError> {
    let temporary = path.with_extension("json.partial");
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)?;
        set_owner_only(&temporary, false)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(path.parent().ok_or(OnboardingError::InvalidPaths)?)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
fn set_owner_only(path: &Path, directory: bool) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    let mode = if directory { 0o700 } else { 0o600 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_owner_only(_path: &Path, _directory: bool) -> Result<(), std::io::Error> {
    Ok(())
}

fn executable_on_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|directory| {
                fs::metadata(directory.join(program))
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

/// Retrieve a root key during a later unlock without exposing it in argv or the
/// environment. The returned buffer is decoded and zeroized immediately.
pub fn load_root_key_from_secret_service(vault_id: Uuid) -> Result<VaultKey, OnboardingError> {
    let output = Command::new("secret-tool")
        .args([
            "lookup",
            "application",
            "pinky",
            "vault-id",
            &vault_id.to_string(),
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output()?;
    if !output.status.success() {
        return status_to_result(output, "Secret Service lookup").map(|_| unreachable!());
    }
    let mut encoded = Zeroizing::new(output.stdout);
    while encoded
        .last()
        .is_some_and(|byte| byte.is_ascii_whitespace())
    {
        encoded.pop();
    }
    let mut decoded = Zeroizing::new(STANDARD_NO_PAD.decode(encoded.as_slice()).map_err(|_| {
        OnboardingError::Platform {
            operation: "Secret Service lookup",
            message: "stored key has invalid encoding".into(),
        }
    })?);
    encoded.zeroize();
    if decoded.len() != 32 {
        return Err(OnboardingError::Platform {
            operation: "Secret Service lookup",
            message: "stored key has invalid length".into(),
        });
    }
    let mut root = [0_u8; 32];
    root.copy_from_slice(&decoded);
    decoded.zeroize();
    Ok(VaultKey::from_bytes(root))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    };

    #[derive(Clone)]
    struct FakeVerifier(Arc<AtomicBool>);
    impl MountVerifier for FakeVerifier {
        fn is_gocryptfs_mount(&self, _: &Path) -> Result<bool, std::io::Error> {
            Ok(self.0.load(Ordering::SeqCst))
        }
    }

    struct FakeMount(Arc<AtomicBool>);
    impl Drop for FakeMount {
        fn drop(&mut self) {
            self.0.store(false, Ordering::SeqCst);
        }
    }

    #[derive(Default)]
    struct FakePlatform {
        calls: Arc<Mutex<Vec<&'static str>>>,
        fail_store: bool,
        mounted: Arc<AtomicBool>,
        root_key: Arc<Mutex<Option<[u8; 32]>>>,
    }

    impl VaultPlatform for FakePlatform {
        type Mount = FakeMount;
        type Verifier = FakeVerifier;

        fn check_prerequisites(&self) -> Result<(), OnboardingError> {
            self.calls.lock().unwrap().push("prerequisites");
            Ok(())
        }

        fn initialize_gocryptfs(&self, _: &Path, _: &str) -> Result<(), OnboardingError> {
            self.calls.lock().unwrap().push("initialize");
            Ok(())
        }

        fn store_root_key(&self, _: Uuid, root_key: &[u8; 32]) -> Result<(), OnboardingError> {
            self.calls.lock().unwrap().push("store");
            if self.fail_store {
                Err(OnboardingError::Platform {
                    operation: "test store",
                    message: "injected failure".into(),
                })
            } else {
                *self.root_key.lock().unwrap() = Some(*root_key);
                Ok(())
            }
        }

        fn load_root_key(&self, _: Uuid) -> Result<VaultKey, OnboardingError> {
            self.calls.lock().unwrap().push("load");
            self.root_key
                .lock()
                .unwrap()
                .map(VaultKey::from_bytes)
                .ok_or_else(|| OnboardingError::Platform {
                    operation: "test load",
                    message: "missing key".into(),
                })
        }

        fn clear_root_key(&self, _: Uuid) {
            self.calls.lock().unwrap().push("clear");
            *self.root_key.lock().unwrap() = None;
        }

        fn mount_gocryptfs(&self, _: &VaultPaths, _: &str) -> Result<Self::Mount, OnboardingError> {
            self.calls.lock().unwrap().push("mount");
            self.mounted.store(true, Ordering::SeqCst);
            Ok(FakeMount(self.mounted.clone()))
        }

        fn verifier(&self) -> Self::Verifier {
            FakeVerifier(self.mounted.clone())
        }
    }

    #[test]
    fn creates_recovery_metadata_and_encrypted_database() {
        let root = tempfile::tempdir().unwrap();
        let paths = VaultPaths {
            cipher_dir: root.path().join("cipher"),
            mount_dir: root.path().join("mount"),
        };
        let platform = FakePlatform::default();
        let onboarded =
            create_vault(&platform, paths.clone(), "a durable recovery phrase").unwrap();
        assert!(onboarded.recovery_path.is_file());
        assert!(paths.mount_dir.join("database/pinky.sqlite3").is_file());
        assert_eq!(
            *platform.calls.lock().unwrap(),
            ["prerequisites", "initialize", "store", "mount"]
        );
    }

    #[test]
    fn rolls_back_directories_after_platform_failure() {
        let root = tempfile::tempdir().unwrap();
        let paths = VaultPaths {
            cipher_dir: root.path().join("cipher"),
            mount_dir: root.path().join("mount"),
        };
        let platform = FakePlatform {
            fail_store: true,
            ..FakePlatform::default()
        };
        assert!(create_vault(&platform, paths.clone(), "a durable recovery phrase").is_err());
        assert!(!paths.cipher_dir.exists());
        assert!(!paths.mount_dir.exists());
    }

    #[test]
    fn rejects_nested_relative_and_nonempty_paths() {
        let root = tempfile::tempdir().unwrap();
        let nested = VaultPaths {
            cipher_dir: root.path().join("vault"),
            mount_dir: root.path().join("vault/mount"),
        };
        assert!(nested.validate().is_err());
        let relative = VaultPaths {
            cipher_dir: PathBuf::from("cipher"),
            mount_dir: PathBuf::from("mount"),
        };
        assert!(relative.validate().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_destinations_reached_through_symlinked_parents() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let actual = root.path().join("actual");
        fs::create_dir(&actual).unwrap();
        let alias = root.path().join("alias");
        symlink(&actual, &alias).unwrap();
        let paths = VaultPaths {
            cipher_dir: alias.join("cipher"),
            mount_dir: root.path().join("mount"),
        };
        assert!(paths.validate().is_err());
    }

    #[test]
    fn registration_unlocks_the_same_vault_after_a_restart() {
        let root = tempfile::tempdir().unwrap();
        let paths = VaultPaths {
            cipher_dir: root.path().join("cipher"),
            mount_dir: root.path().join("mount"),
        };
        let registration_path = root.path().join("config/vault.json");
        let platform = FakePlatform::default();
        let first = create_registered_vault(
            &platform,
            paths.clone(),
            "a durable recovery phrase",
            &registration_path,
        )
        .unwrap();
        let first_id = first.id;
        drop(first);
        fs::remove_dir_all(&paths.mount_dir).unwrap();

        let registration = read_registration(&registration_path).unwrap();
        let reopened = unlock_registered_vault(&platform, &registration).unwrap();
        assert_eq!(reopened.id, first_id);
        assert!(reopened.vault.ensure_mounted().is_ok());
        assert!(platform
            .calls
            .lock()
            .unwrap()
            .ends_with(&["prerequisites", "load", "mount"]));
    }

    #[test]
    fn registration_rejects_a_mismatched_recovery_envelope() {
        let root = tempfile::tempdir().unwrap();
        let paths = VaultPaths {
            cipher_dir: root.path().join("cipher"),
            mount_dir: root.path().join("mount"),
        };
        let registration_path = root.path().join("vault.json");
        let platform = FakePlatform::default();
        let onboarded = create_registered_vault(
            &platform,
            paths,
            "a durable recovery phrase",
            &registration_path,
        )
        .unwrap();
        let recovery_path = onboarded.recovery_path.clone();
        drop(onboarded);
        let mut recovery: RecoveryEnvelope =
            serde_json::from_slice(&fs::read(&recovery_path).unwrap()).unwrap();
        recovery.vault_id = Uuid::new_v4();
        fs::write(&recovery_path, serde_json::to_vec(&recovery).unwrap()).unwrap();
        assert!(matches!(
            read_registration(&registration_path),
            Err(OnboardingError::InvalidRegistration(_))
        ));
    }

    #[cfg(unix)]
    #[test]
    fn registration_is_owner_readable_only() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let paths = VaultPaths {
            cipher_dir: root.path().join("cipher"),
            mount_dir: root.path().join("mount"),
        };
        let registration_path = root.path().join("config/vault.json");
        let platform = FakePlatform::default();
        let _onboarded = create_registered_vault(
            &platform,
            paths,
            "a durable recovery phrase",
            &registration_path,
        )
        .unwrap();
        assert_eq!(
            fs::metadata(registration_path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
}
