use argon2::{Algorithm, Argon2, Params, Version};
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use chacha20poly1305::{
    aead::{Aead, Payload},
    KeyInit, XChaCha20Poly1305, XNonce,
};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

const ENVELOPE_VERSION: u16 = 1;
const DEFAULT_MEMORY_KIB: u32 = 64 * 1024;
const DEFAULT_TIME_COST: u32 = 3;
const DEFAULT_LANES: u32 = 1;
const MIN_PASSPHRASE_CHARS: usize = 12;

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("the recovery passphrase must contain at least {MIN_PASSPHRASE_CHARS} characters")]
    WeakPassphrase,
    #[error("unsupported recovery-envelope version {0}")]
    UnsupportedVersion(u16),
    #[error("invalid or unsafe recovery-envelope parameters")]
    InvalidParameters,
    #[error("invalid recovery-envelope encoding")]
    InvalidEncoding,
    #[error("the recovery passphrase is incorrect or the envelope was altered")]
    AuthenticationFailed,
    #[error("key derivation failed")]
    KeyDerivation,
}

/// The random root secret from which purpose-specific vault keys are derived.
/// Its memory is cleared when the value is dropped.
#[derive(Zeroize)]
#[zeroize(drop)]
pub struct VaultKey([u8; 32]);

impl VaultKey {
    pub fn generate() -> Self {
        let mut bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub fn expose(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn derive_subkeys(&self) -> Result<VaultSubkeys, RecoveryError> {
        let hkdf = Hkdf::<Sha256>::new(Some(b"pinky-vault-root-v1"), &self.0);
        let mut gocryptfs = Zeroizing::new([0_u8; 32]);
        let mut database = Zeroizing::new([0_u8; 32]);
        hkdf.expand(b"gocryptfs-password", gocryptfs.as_mut())
            .map_err(|_| RecoveryError::KeyDerivation)?;
        hkdf.expand(b"sqlcipher-database", database.as_mut())
            .map_err(|_| RecoveryError::KeyDerivation)?;
        Ok(VaultSubkeys {
            gocryptfs,
            database,
        })
    }
}

pub struct VaultSubkeys {
    gocryptfs: Zeroizing<[u8; 32]>,
    database: Zeroizing<[u8; 32]>,
}

impl VaultSubkeys {
    /// A newline-free gocryptfs password suitable for a passfile on stdin.
    pub fn gocryptfs_password(&self) -> Zeroizing<String> {
        Zeroizing::new(STANDARD_NO_PAD.encode(self.gocryptfs.as_slice()))
    }

    pub fn database_key(&self) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(self.database.to_vec())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecoveryEnvelope {
    pub version: u16,
    pub vault_id: Uuid,
    pub kdf: RecoveryKdf,
    pub cipher: RecoveryCipher,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecoveryKdf {
    pub algorithm: String,
    pub memory_kib: u32,
    pub time_cost: u32,
    pub lanes: u32,
    pub salt_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecoveryCipher {
    pub algorithm: String,
    pub nonce_base64: String,
    pub ciphertext_base64: String,
}

impl RecoveryEnvelope {
    pub fn wrap(
        vault_id: Uuid,
        root_key: &VaultKey,
        passphrase: &str,
    ) -> Result<Self, RecoveryError> {
        Self::wrap_with_parameters(
            vault_id,
            root_key,
            passphrase,
            DEFAULT_MEMORY_KIB,
            DEFAULT_TIME_COST,
            DEFAULT_LANES,
        )
    }

    fn wrap_with_parameters(
        vault_id: Uuid,
        root_key: &VaultKey,
        passphrase: &str,
        memory_kib: u32,
        time_cost: u32,
        lanes: u32,
    ) -> Result<Self, RecoveryError> {
        validate_passphrase(passphrase)?;
        validate_parameters(memory_kib, time_cost, lanes)?;
        let mut salt = [0_u8; 16];
        let mut nonce = [0_u8; 24];
        OsRng.fill_bytes(&mut salt);
        OsRng.fill_bytes(&mut nonce);
        let mut wrapping_key = Zeroizing::new([0_u8; 32]);
        derive_wrapping_key(
            passphrase,
            &salt,
            memory_kib,
            time_cost,
            lanes,
            wrapping_key.as_mut(),
        )?;
        let cipher = XChaCha20Poly1305::new(wrapping_key.as_ref().into());
        let ciphertext = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: root_key.expose(),
                    aad: aad(vault_id).as_bytes(),
                },
            )
            .map_err(|_| RecoveryError::AuthenticationFailed)?;

        Ok(Self {
            version: ENVELOPE_VERSION,
            vault_id,
            kdf: RecoveryKdf {
                algorithm: "argon2id-1.3".into(),
                memory_kib,
                time_cost,
                lanes,
                salt_base64: STANDARD_NO_PAD.encode(salt),
            },
            cipher: RecoveryCipher {
                algorithm: "xchacha20poly1305".into(),
                nonce_base64: STANDARD_NO_PAD.encode(nonce),
                ciphertext_base64: STANDARD_NO_PAD.encode(ciphertext),
            },
        })
    }

    pub fn recover(&self, passphrase: &str) -> Result<VaultKey, RecoveryError> {
        validate_passphrase(passphrase)?;
        if self.version != ENVELOPE_VERSION {
            return Err(RecoveryError::UnsupportedVersion(self.version));
        }
        if self.kdf.algorithm != "argon2id-1.3" || self.cipher.algorithm != "xchacha20poly1305" {
            return Err(RecoveryError::InvalidParameters);
        }
        validate_parameters(self.kdf.memory_kib, self.kdf.time_cost, self.kdf.lanes)?;
        let salt = decode_exact::<16>(&self.kdf.salt_base64)?;
        let nonce = decode_exact::<24>(&self.cipher.nonce_base64)?;
        let ciphertext = STANDARD_NO_PAD
            .decode(&self.cipher.ciphertext_base64)
            .map_err(|_| RecoveryError::InvalidEncoding)?;
        let mut wrapping_key = Zeroizing::new([0_u8; 32]);
        derive_wrapping_key(
            passphrase,
            &salt,
            self.kdf.memory_kib,
            self.kdf.time_cost,
            self.kdf.lanes,
            wrapping_key.as_mut(),
        )?;
        let cipher = XChaCha20Poly1305::new(wrapping_key.as_ref().into());
        let mut plaintext = Zeroizing::new(
            cipher
                .decrypt(
                    XNonce::from_slice(&nonce),
                    Payload {
                        msg: &ciphertext,
                        aad: aad(self.vault_id).as_bytes(),
                    },
                )
                .map_err(|_| RecoveryError::AuthenticationFailed)?,
        );
        if plaintext.len() != 32 {
            return Err(RecoveryError::AuthenticationFailed);
        }
        let mut root = [0_u8; 32];
        root.copy_from_slice(&plaintext);
        plaintext.zeroize();
        Ok(VaultKey::from_bytes(root))
    }

    #[cfg(test)]
    fn wrap_for_test(
        vault_id: Uuid,
        root_key: &VaultKey,
        passphrase: &str,
    ) -> Result<Self, RecoveryError> {
        Self::wrap_with_parameters(vault_id, root_key, passphrase, 8 * 1024, 1, 1)
    }
}

fn validate_passphrase(passphrase: &str) -> Result<(), RecoveryError> {
    if passphrase.chars().count() < MIN_PASSPHRASE_CHARS {
        Err(RecoveryError::WeakPassphrase)
    } else {
        Ok(())
    }
}

fn validate_parameters(memory_kib: u32, time_cost: u32, lanes: u32) -> Result<(), RecoveryError> {
    if !(8 * 1024..=256 * 1024).contains(&memory_kib)
        || !(1..=10).contains(&time_cost)
        || !(1..=8).contains(&lanes)
    {
        return Err(RecoveryError::InvalidParameters);
    }
    Ok(())
}

fn derive_wrapping_key(
    passphrase: &str,
    salt: &[u8],
    memory_kib: u32,
    time_cost: u32,
    lanes: u32,
    output: &mut [u8],
) -> Result<(), RecoveryError> {
    let params = Params::new(memory_kib, time_cost, lanes, Some(output.len()))
        .map_err(|_| RecoveryError::InvalidParameters)?;
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, output)
        .map_err(|_| RecoveryError::KeyDerivation)
}

fn decode_exact<const N: usize>(encoded: &str) -> Result<[u8; N], RecoveryError> {
    let decoded = STANDARD_NO_PAD
        .decode(encoded)
        .map_err(|_| RecoveryError::InvalidEncoding)?;
    decoded
        .try_into()
        .map_err(|_| RecoveryError::InvalidEncoding)
}

fn aad(vault_id: Uuid) -> String {
    format!("pinky-recovery:v{ENVELOPE_VERSION}:{vault_id}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passphrase_round_trip_restores_the_root_key() {
        let vault_id = Uuid::new_v4();
        let root = VaultKey::generate();
        let envelope =
            RecoveryEnvelope::wrap_for_test(vault_id, &root, "a long recovery passphrase").unwrap();
        let recovered = envelope.recover("a long recovery passphrase").unwrap();
        assert_eq!(root.expose(), recovered.expose());
    }

    #[test]
    fn incorrect_passphrases_and_tampering_are_rejected() {
        let vault_id = Uuid::new_v4();
        let root = VaultKey::generate();
        let mut envelope =
            RecoveryEnvelope::wrap_for_test(vault_id, &root, "a long recovery passphrase").unwrap();
        assert!(matches!(
            envelope.recover("another wrong passphrase"),
            Err(RecoveryError::AuthenticationFailed)
        ));
        envelope.vault_id = Uuid::new_v4();
        assert!(matches!(
            envelope.recover("a long recovery passphrase"),
            Err(RecoveryError::AuthenticationFailed)
        ));
    }

    #[test]
    fn subkeys_are_domain_separated_and_stable() {
        let root = VaultKey::from_bytes([7; 32]);
        let first = root.derive_subkeys().unwrap();
        let second = root.derive_subkeys().unwrap();
        assert_eq!(first.gocryptfs.as_slice(), second.gocryptfs.as_slice());
        assert_ne!(first.gocryptfs.as_slice(), first.database.as_slice());
    }

    #[test]
    fn weak_passphrases_are_rejected() {
        assert!(matches!(
            RecoveryEnvelope::wrap_for_test(Uuid::new_v4(), &VaultKey::generate(), "too short"),
            Err(RecoveryError::WeakPassphrase)
        ));
    }
}
