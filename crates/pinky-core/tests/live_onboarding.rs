use pinky_core::{
    create_registered_vault, read_registration, unlock_registered_vault, MountVerifier,
    ObjectStore, ProcMountVerifier, SystemVaultPlatform, VaultPaths, VaultPlatform,
};
use uuid::Uuid;

struct SecretCleanup(Option<Uuid>);

impl Drop for SecretCleanup {
    fn drop(&mut self) {
        if let Some(vault_id) = self.0 {
            SystemVaultPlatform.clear_root_key(vault_id);
        }
    }
}

#[test]
#[ignore = "requires a desktop Secret Service session and permission to mount /dev/fuse"]
fn creates_unmounts_and_reopens_a_real_encrypted_vault() {
    let root = tempfile::tempdir().unwrap();
    let paths = VaultPaths {
        cipher_dir: root.path().join("cipher"),
        mount_dir: root.path().join("mounted"),
    };
    let registration_path = root.path().join("vault-registration.json");
    let mut secret_cleanup = SecretCleanup(None);

    let created = create_registered_vault(
        &SystemVaultPlatform,
        paths.clone(),
        "live acceptance recovery passphrase",
        &registration_path,
    )
    .unwrap();
    secret_cleanup.0 = Some(created.id);
    assert!(created.vault.ensure_mounted().is_ok());
    let object = ObjectStore::new(created.vault.clone())
        .put(b"live encrypted round trip", "text/plain")
        .unwrap();
    let object_hash = object.metadata.sha256;
    let vault_id = created.id;
    drop(created);

    assert!(!ProcMountVerifier
        .is_gocryptfs_mount(&paths.mount_dir)
        .unwrap());
    let registration = read_registration(&registration_path).unwrap();
    let reopened = unlock_registered_vault(&SystemVaultPlatform, &registration).unwrap();
    assert_eq!(reopened.id, vault_id);
    assert_eq!(
        ObjectStore::new(reopened.vault.clone())
            .read_verified(&object_hash)
            .unwrap(),
        b"live encrypted round trip"
    );
    drop(reopened);
    assert!(!ProcMountVerifier
        .is_gocryptfs_mount(&paths.mount_dir)
        .unwrap());
}
