//! Authenticated encryption for durable confidential payloads.
//!
//! The binary envelope is deliberately small and strict:
//! `magic(4) || version(1) || algorithm(1) || nonce_len(1) || tag_len(1) ||
//! ciphertext_len(8) || nonce(24) || ciphertext+tag`.
//!
//! Callers supply a non-secret purpose and binding. They are encoded with
//! length prefixes and authenticated together with the exact envelope header,
//! preventing cross-purpose replay and ambiguous concatenation.

use base64::Engine;
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, AeadCore, KeyInit, OsRng, Payload},
};
use rand::RngCore;
use std::{fs::OpenOptions, io::ErrorKind, path::Path};
use thiserror::Error;
use zeroize::Zeroizing;

use crate::credentials::{ConfidentialCredentialsStore, CredentialsStore};

const MAGIC: &[u8; 4] = b"WCBL";
const VERSION: u8 = 1;
const ALGORITHM_XCHACHA20_POLY1305: u8 = 1;
const NONCE_LEN: usize = 24;
const TAG_LEN: usize = 16;
const KEY_LEN: usize = 32;
const HEADER_LEN: usize = 16;
const MAX_PLAINTEXT_LEN: usize = 64 * 1024 * 1024;
const MAX_PURPOSE_LEN: usize = 255;
const MAX_BINDING_LEN: usize = 16 * 1024;
const AAD_DOMAIN: &[u8] = b"wayland-core/confidential-blob/aad/v1\0";

/// A 256-bit confidential-blob key whose owned bytes are zeroized on drop.
pub struct ConfidentialBlobKey(Zeroizing<[u8; KEY_LEN]>);

impl ConfidentialBlobKey {
    /// Generate a key using the operating system random source.
    pub fn generate() -> Self {
        let mut bytes = Zeroizing::new([0_u8; KEY_LEN]);
        OsRng.fill_bytes(bytes.as_mut());
        Self(bytes)
    }

    /// Copy an exact 32-byte key into zeroizing storage.
    pub fn from_slice(bytes: &[u8]) -> Result<Self, ConfidentialBlobError> {
        if bytes.len() != KEY_LEN {
            return Err(ConfidentialBlobError::InvalidKeyLength);
        }
        let mut key = Zeroizing::new([0_u8; KEY_LEN]);
        key.copy_from_slice(bytes);
        Ok(Self(key))
    }

    fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

/// Confidential-key persistence failures. Store and decoding details are
/// deliberately suppressed so error chains cannot disclose key material.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ConfidentialKeyStoreError {
    #[error("confidential key reference is invalid")]
    InvalidReference,
    #[error("confidential key store read failed")]
    ReadFailed,
    #[error("stored confidential key is missing")]
    MissingStoredKey,
    #[error("confidential key store write failed")]
    WriteFailed,
    #[error("confidential key creation lock failed")]
    LockFailed,
    #[error("stored confidential key is malformed")]
    MalformedStoredKey,
}

/// Load an existing confidential-blob key or create and persist a new one.
///
/// The stored representation is canonical URL-safe base64 without padding.
/// Malformed or non-canonical existing values fail closed and are never
/// overwritten. `lock_path` serializes creation across processes because
/// [`CredentialsStore`] intentionally has no compare-and-set API.
pub fn load_or_create_confidential_blob_key(
    store: &ConfidentialCredentialsStore,
    key_ref: &str,
) -> Result<ConfidentialBlobKey, ConfidentialKeyStoreError> {
    load_or_create_confidential_blob_key_with_lock(store, key_ref, store.key_creation_lock_path())
}

/// Load an existing confidential-blob key without creating or mutating it.
///
/// Recovery uses this read-only path so a missing key cannot be replaced by a
/// newly generated key that merely makes authentication fail later.
pub fn load_confidential_blob_key(
    store: &ConfidentialCredentialsStore,
    key_ref: &str,
) -> Result<ConfidentialBlobKey, ConfidentialKeyStoreError> {
    load_confidential_blob_key_from_store(store, key_ref)
}

fn load_or_create_confidential_blob_key_with_lock(
    store: &dyn CredentialsStore,
    key_ref: &str,
    lock_path: &Path,
) -> Result<ConfidentialBlobKey, ConfidentialKeyStoreError> {
    validate_key_ref(key_ref)?;
    if let Some(parent) = lock_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|_| ConfidentialKeyStoreError::LockFailed)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|_| ConfidentialKeyStoreError::LockFailed)?;
    let mut lock = fd_lock::RwLock::new(file);
    let _guard = loop {
        match lock.write() {
            Ok(guard) => break guard,
            Err(error) if error.kind() == ErrorKind::Interrupted => continue,
            Err(_) => return Err(ConfidentialKeyStoreError::LockFailed),
        }
    };
    load_or_create_confidential_blob_key_from_store(store, key_ref)
}

fn load_or_create_confidential_blob_key_from_store(
    store: &dyn CredentialsStore,
    key_ref: &str,
) -> Result<ConfidentialBlobKey, ConfidentialKeyStoreError> {
    validate_key_ref(key_ref)?;
    if let Some(encoded) = store
        .get(key_ref)
        .map_err(|_| ConfidentialKeyStoreError::ReadFailed)?
    {
        let encoded = Zeroizing::new(encoded);
        return decode_stored_key(encoded.as_str());
    }

    let key = ConfidentialBlobKey::generate();
    let encoded =
        Zeroizing::new(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key.as_bytes()));
    store
        .put(key_ref, encoded.as_str())
        .map_err(|_| ConfidentialKeyStoreError::WriteFailed)?;
    Ok(key)
}

fn load_confidential_blob_key_from_store(
    store: &dyn CredentialsStore,
    key_ref: &str,
) -> Result<ConfidentialBlobKey, ConfidentialKeyStoreError> {
    validate_key_ref(key_ref)?;
    let encoded = Zeroizing::new(
        store
            .get(key_ref)
            .map_err(|_| ConfidentialKeyStoreError::ReadFailed)?
            .ok_or(ConfidentialKeyStoreError::MissingStoredKey)?,
    );
    decode_stored_key(encoded.as_str())
}

fn validate_key_ref(key_ref: &str) -> Result<(), ConfidentialKeyStoreError> {
    let mut chars = key_ref.chars();
    let Some(first) = chars.next() else {
        return Err(ConfidentialKeyStoreError::InvalidReference);
    };
    if key_ref.len() > 255
        || !first.is_ascii_alphanumeric()
        || !chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_'))
    {
        return Err(ConfidentialKeyStoreError::InvalidReference);
    }
    Ok(())
}

fn decode_stored_key(encoded: &str) -> Result<ConfidentialBlobKey, ConfidentialKeyStoreError> {
    let decoded = Zeroizing::new(
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(encoded)
            .map_err(|_| ConfidentialKeyStoreError::MalformedStoredKey)?,
    );
    if base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(decoded.as_slice()) != encoded {
        return Err(ConfidentialKeyStoreError::MalformedStoredKey);
    }
    ConfidentialBlobKey::from_slice(decoded.as_slice())
        .map_err(|_| ConfidentialKeyStoreError::MalformedStoredKey)
}

/// Non-secret context authenticated with a confidential blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfidentialBlobAad {
    purpose: String,
    binding: Vec<u8>,
}

impl ConfidentialBlobAad {
    /// Create AAD that binds a blob to one purpose and durable identity.
    ///
    /// `purpose` and `binding` are stored alongside neither the ciphertext nor
    /// the key. Callers must reproduce them exactly when opening the blob.
    pub fn new(purpose: impl Into<String>, binding: impl Into<Vec<u8>>) -> Self {
        Self {
            purpose: purpose.into(),
            binding: binding.into(),
        }
    }

    fn canonical_bytes(&self, header: &[u8; HEADER_LEN]) -> Result<Vec<u8>, ConfidentialBlobError> {
        let purpose = self.purpose.as_bytes();
        if purpose.is_empty() || purpose.len() > MAX_PURPOSE_LEN {
            return Err(ConfidentialBlobError::InvalidAad);
        }
        if self.binding.is_empty() || self.binding.len() > MAX_BINDING_LEN {
            return Err(ConfidentialBlobError::InvalidAad);
        }

        let mut out = Vec::with_capacity(
            AAD_DOMAIN.len() + 4 + purpose.len() + 4 + self.binding.len() + HEADER_LEN,
        );
        out.extend_from_slice(AAD_DOMAIN);
        out.extend_from_slice(&(purpose.len() as u32).to_be_bytes());
        out.extend_from_slice(purpose);
        out.extend_from_slice(&(self.binding.len() as u32).to_be_bytes());
        out.extend_from_slice(&self.binding);
        out.extend_from_slice(header);
        Ok(out)
    }
}

/// Confidential-blob failures. Messages intentionally contain no payload,
/// key, AAD, nonce, or backend details.
#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum ConfidentialBlobError {
    #[error("confidential blob key must be 32 bytes")]
    InvalidKeyLength,
    #[error("confidential blob associated data is invalid")]
    InvalidAad,
    #[error("confidential blob plaintext is too large")]
    PlaintextTooLarge,
    #[error("confidential blob header is truncated")]
    TruncatedHeader,
    #[error("confidential blob magic is invalid")]
    InvalidMagic,
    #[error("confidential blob version is unsupported")]
    UnsupportedVersion,
    #[error("confidential blob algorithm is unsupported")]
    UnsupportedAlgorithm,
    #[error("confidential blob nonce length is invalid")]
    InvalidNonceLength,
    #[error("confidential blob tag length is invalid")]
    InvalidTagLength,
    #[error("confidential blob ciphertext length is invalid")]
    InvalidCiphertextLength,
    #[error("confidential blob authentication failed")]
    AuthenticationFailed,
}

/// Seal plaintext into a versioned XChaCha20-Poly1305 envelope.
pub fn seal_confidential_blob(
    key: &ConfidentialBlobKey,
    aad: &ConfidentialBlobAad,
    plaintext: &[u8],
) -> Result<Vec<u8>, ConfidentialBlobError> {
    if plaintext.len() > MAX_PLAINTEXT_LEN {
        return Err(ConfidentialBlobError::PlaintextTooLarge);
    }

    let ciphertext_len = plaintext
        .len()
        .checked_add(TAG_LEN)
        .ok_or(ConfidentialBlobError::PlaintextTooLarge)?;
    let header = encode_header(ciphertext_len as u64);
    let canonical_aad = aad.canonical_bytes(&header)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_bytes())
        .map_err(|_| ConfidentialBlobError::InvalidKeyLength)?;
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(
            &nonce,
            Payload {
                msg: plaintext,
                aad: &canonical_aad,
            },
        )
        .map_err(|_| ConfidentialBlobError::AuthenticationFailed)?;

    let mut blob = Vec::with_capacity(HEADER_LEN + NONCE_LEN + ciphertext.len());
    blob.extend_from_slice(&header);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);
    Ok(blob)
}

/// Authenticate and open a versioned XChaCha20-Poly1305 envelope.
pub fn open_confidential_blob(
    key: &ConfidentialBlobKey,
    aad: &ConfidentialBlobAad,
    blob: &[u8],
) -> Result<Vec<u8>, ConfidentialBlobError> {
    let (header, nonce, ciphertext) = parse_envelope(blob)?;
    let canonical_aad = aad.canonical_bytes(&header)?;
    let cipher = XChaCha20Poly1305::new_from_slice(key.as_bytes())
        .map_err(|_| ConfidentialBlobError::InvalidKeyLength)?;
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: ciphertext,
                aad: &canonical_aad,
            },
        )
        .map_err(|_| ConfidentialBlobError::AuthenticationFailed)
}

fn encode_header(ciphertext_len: u64) -> [u8; HEADER_LEN] {
    let mut header = [0_u8; HEADER_LEN];
    header[..4].copy_from_slice(MAGIC);
    header[4] = VERSION;
    header[5] = ALGORITHM_XCHACHA20_POLY1305;
    header[6] = NONCE_LEN as u8;
    header[7] = TAG_LEN as u8;
    header[8..].copy_from_slice(&ciphertext_len.to_be_bytes());
    header
}

type ParsedEnvelope<'a> = ([u8; HEADER_LEN], &'a [u8], &'a [u8]);

fn parse_envelope(blob: &[u8]) -> Result<ParsedEnvelope<'_>, ConfidentialBlobError> {
    if blob.len() < HEADER_LEN {
        return Err(ConfidentialBlobError::TruncatedHeader);
    }
    let mut header = [0_u8; HEADER_LEN];
    header.copy_from_slice(&blob[..HEADER_LEN]);

    if &header[..4] != MAGIC {
        return Err(ConfidentialBlobError::InvalidMagic);
    }
    if header[4] != VERSION {
        return Err(ConfidentialBlobError::UnsupportedVersion);
    }
    if header[5] != ALGORITHM_XCHACHA20_POLY1305 {
        return Err(ConfidentialBlobError::UnsupportedAlgorithm);
    }
    if header[6] as usize != NONCE_LEN {
        return Err(ConfidentialBlobError::InvalidNonceLength);
    }
    if header[7] as usize != TAG_LEN {
        return Err(ConfidentialBlobError::InvalidTagLength);
    }

    let declared_u64 = u64::from_be_bytes(
        header[8..]
            .try_into()
            .map_err(|_| ConfidentialBlobError::InvalidCiphertextLength)?,
    );
    let declared = usize::try_from(declared_u64)
        .map_err(|_| ConfidentialBlobError::InvalidCiphertextLength)?;
    if !(TAG_LEN..=MAX_PLAINTEXT_LEN + TAG_LEN).contains(&declared) {
        return Err(ConfidentialBlobError::InvalidCiphertextLength);
    }
    let expected_len = HEADER_LEN
        .checked_add(NONCE_LEN)
        .and_then(|prefix| prefix.checked_add(declared))
        .ok_or(ConfidentialBlobError::InvalidCiphertextLength)?;
    if blob.len() != expected_len {
        return Err(ConfidentialBlobError::InvalidCiphertextLength);
    }

    let nonce_start = HEADER_LEN;
    let ciphertext_start = nonce_start + NONCE_LEN;
    Ok((
        header,
        &blob[nonce_start..ciphertext_start],
        &blob[ciphertext_start..],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        collections::HashMap,
        sync::{Arc, Barrier, Mutex},
    };

    use crate::credentials::CredentialsError;

    #[derive(Default)]
    struct MemoryCredentialsStore {
        values: Mutex<HashMap<String, String>>,
    }

    impl CredentialsStore for MemoryCredentialsStore {
        fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
            Ok(self.values.lock().unwrap().get(key).cloned())
        }

        fn put(&self, key: &str, value: &str) -> Result<(), CredentialsError> {
            self.values
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }

        fn delete(&self, key: &str) -> Result<(), CredentialsError> {
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    fn test_key(byte: u8) -> ConfidentialBlobKey {
        ConfidentialBlobKey::from_slice(&[byte; KEY_LEN]).unwrap()
    }

    fn aad() -> ConfidentialBlobAad {
        ConfidentialBlobAad::new("recovery.prepared-request", b"session-1:cursor-9".to_vec())
    }

    #[test]
    fn seal_open_round_trips_exact_bytes() {
        let plaintext = b"\0exact prepared request\xff";
        let blob = seal_confidential_blob(&test_key(7), &aad(), plaintext).unwrap();
        assert_eq!(
            open_confidential_blob(&test_key(7), &aad(), &blob).unwrap(),
            plaintext
        );
    }

    #[test]
    fn fresh_nonce_produces_distinct_ciphertext() {
        let key = test_key(8);
        let aad = aad();
        let first = seal_confidential_blob(&key, &aad, b"same plaintext").unwrap();
        let second = seal_confidential_blob(&key, &aad, b"same plaintext").unwrap();
        assert_ne!(first, second);
        assert_eq!(
            open_confidential_blob(&key, &aad, &first).unwrap(),
            b"same plaintext"
        );
        assert_eq!(
            open_confidential_blob(&key, &aad, &second).unwrap(),
            b"same plaintext"
        );
    }

    #[test]
    fn wrong_key_aad_and_tamper_fail_closed() {
        let key = test_key(9);
        let blob = seal_confidential_blob(&key, &aad(), b"sensitive-value-123").unwrap();

        assert_eq!(
            open_confidential_blob(&test_key(10), &aad(), &blob),
            Err(ConfidentialBlobError::AuthenticationFailed)
        );
        assert_eq!(
            open_confidential_blob(
                &key,
                &ConfidentialBlobAad::new(
                    "recovery.prepared-request",
                    b"different-session".to_vec(),
                ),
                &blob,
            ),
            Err(ConfidentialBlobError::AuthenticationFailed)
        );

        let mut tampered = blob;
        *tampered.last_mut().unwrap() ^= 1;
        assert_eq!(
            open_confidential_blob(&key, &aad(), &tampered),
            Err(ConfidentialBlobError::AuthenticationFailed)
        );
    }

    #[test]
    fn unsupported_and_malformed_headers_fail_before_decryption() {
        let key = test_key(11);
        let original = seal_confidential_blob(&key, &aad(), b"payload").unwrap();

        assert_eq!(
            open_confidential_blob(&key, &aad(), &original[..HEADER_LEN - 1]),
            Err(ConfidentialBlobError::TruncatedHeader)
        );

        let mut bad_magic = original.clone();
        bad_magic[0] ^= 1;
        assert_eq!(
            open_confidential_blob(&key, &aad(), &bad_magic),
            Err(ConfidentialBlobError::InvalidMagic)
        );

        let mut unsupported_version = original.clone();
        unsupported_version[4] = VERSION + 1;
        assert_eq!(
            open_confidential_blob(&key, &aad(), &unsupported_version),
            Err(ConfidentialBlobError::UnsupportedVersion)
        );

        let mut unsupported_algorithm = original.clone();
        unsupported_algorithm[5] = ALGORITHM_XCHACHA20_POLY1305 + 1;
        assert_eq!(
            open_confidential_blob(&key, &aad(), &unsupported_algorithm),
            Err(ConfidentialBlobError::UnsupportedAlgorithm)
        );

        let mut bad_nonce_len = original.clone();
        bad_nonce_len[6] = (NONCE_LEN - 1) as u8;
        assert_eq!(
            open_confidential_blob(&key, &aad(), &bad_nonce_len),
            Err(ConfidentialBlobError::InvalidNonceLength)
        );

        let mut bad_tag_len = original.clone();
        bad_tag_len[7] = (TAG_LEN - 1) as u8;
        assert_eq!(
            open_confidential_blob(&key, &aad(), &bad_tag_len),
            Err(ConfidentialBlobError::InvalidTagLength)
        );

        let mut bad_ciphertext_len = original;
        bad_ciphertext_len[8..HEADER_LEN].copy_from_slice(&u64::MAX.to_be_bytes());
        assert_eq!(
            open_confidential_blob(&key, &aad(), &bad_ciphertext_len),
            Err(ConfidentialBlobError::InvalidCiphertextLength)
        );

        let mut trailing_bytes = seal_confidential_blob(&key, &aad(), b"payload").unwrap();
        trailing_bytes.push(0);
        assert_eq!(
            open_confidential_blob(&key, &aad(), &trailing_bytes),
            Err(ConfidentialBlobError::InvalidCiphertextLength)
        );
    }

    #[test]
    fn errors_never_include_key_aad_or_plaintext() {
        let key_text = "super-secret-key-material";
        let aad_text = "customer-acme-session-99";
        let plaintext = "plaintext-api-key-sk-live-123";
        let key = ConfidentialBlobKey::from_slice(&[42; KEY_LEN]).unwrap();
        let context = ConfidentialBlobAad::new("recovery", aad_text.as_bytes().to_vec());
        let mut blob = seal_confidential_blob(&key, &context, plaintext.as_bytes()).unwrap();
        *blob.last_mut().unwrap() ^= 1;
        let rendered = open_confidential_blob(&key, &context, &blob)
            .unwrap_err()
            .to_string();

        assert!(!rendered.contains(key_text));
        assert!(!rendered.contains(aad_text));
        assert!(!rendered.contains(plaintext));
    }

    #[test]
    fn key_load_or_create_persists_and_reloads_exact_key() {
        let store = MemoryCredentialsStore::default();
        let key_ref = "recovery.session-123.sealing-key";
        let created = load_or_create_confidential_blob_key_from_store(&store, key_ref).unwrap();
        let stored = store.get(key_ref).unwrap().unwrap();
        assert_eq!(stored.len(), 43, "32 bytes encode to 43 base64url chars");

        let loaded = load_or_create_confidential_blob_key_from_store(&store, key_ref).unwrap();
        let context = ConfidentialBlobAad::new("recovery", b"session-123".to_vec());
        let blob = seal_confidential_blob(&created, &context, b"durable payload").unwrap();
        assert_eq!(
            open_confidential_blob(&loaded, &context, &blob).unwrap(),
            b"durable payload"
        );
        assert_eq!(store.get(key_ref).unwrap().unwrap(), stored);
    }

    #[test]
    fn read_only_key_load_never_creates_or_overwrites() {
        let store = MemoryCredentialsStore::default();
        let key_ref = "recovery.session-read-only.sealing-key";

        assert!(matches!(
            load_confidential_blob_key_from_store(&store, key_ref),
            Err(ConfidentialKeyStoreError::MissingStoredKey)
        ));
        assert!(store.values.lock().unwrap().is_empty());

        store.put(key_ref, "malformed").unwrap();
        assert!(matches!(
            load_confidential_blob_key_from_store(&store, key_ref),
            Err(ConfidentialKeyStoreError::MalformedStoredKey)
        ));
        assert_eq!(store.get(key_ref).unwrap().as_deref(), Some("malformed"));
    }

    #[test]
    fn concurrent_key_creation_converges_on_one_decryptable_key() {
        const WORKERS: usize = 12;
        let dir = tempfile::tempdir().unwrap();
        let lock_path = Arc::new(dir.path().join("confidential-key.lock"));
        let store = Arc::new(MemoryCredentialsStore::default());
        let barrier = Arc::new(Barrier::new(WORKERS));
        let mut workers = Vec::with_capacity(WORKERS);

        for worker in 0..WORKERS {
            let lock_path = Arc::clone(&lock_path);
            let store = Arc::clone(&store);
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                let key = load_or_create_confidential_blob_key_with_lock(
                    store.as_ref(),
                    "recovery.session-race.sealing-key",
                    lock_path.as_ref(),
                )
                .unwrap();
                let context = ConfidentialBlobAad::new(
                    "recovery",
                    format!("session-race:{worker}").into_bytes(),
                );
                let blob = seal_confidential_blob(&key, &context, b"durable payload").unwrap();
                (context, blob)
            }));
        }

        let canonical = load_or_create_confidential_blob_key_with_lock(
            store.as_ref(),
            "recovery.session-race.sealing-key",
            lock_path.as_ref(),
        )
        .unwrap();
        for worker in workers {
            let (context, blob) = worker.join().unwrap();
            assert_eq!(
                open_confidential_blob(&canonical, &context, &blob).unwrap(),
                b"durable payload"
            );
        }
        assert_eq!(store.values.lock().unwrap().len(), 1);
    }

    #[test]
    fn malformed_stored_key_fails_closed_without_overwrite() {
        let store = MemoryCredentialsStore::default();
        let key_ref = "recovery.session-456.sealing-key";
        store.put(key_ref, "not-a-canonical-32-byte-key").unwrap();

        assert!(matches!(
            load_or_create_confidential_blob_key_from_store(&store, key_ref),
            Err(ConfidentialKeyStoreError::MalformedStoredKey)
        ));
        let rendered = match load_or_create_confidential_blob_key_from_store(&store, key_ref) {
            Ok(_) => panic!("malformed key must not be accepted"),
            Err(error) => error.to_string(),
        };
        assert!(!rendered.contains("not-a-canonical-32-byte-key"));
        assert_eq!(
            store.get(key_ref).unwrap().as_deref(),
            Some("not-a-canonical-32-byte-key")
        );
    }

    #[test]
    fn invalid_key_reference_is_rejected_without_store_access() {
        let store = MemoryCredentialsStore::default();
        for key_ref in ["", ".hidden", "recovery/session", "recovery secret"] {
            assert!(matches!(
                load_or_create_confidential_blob_key_from_store(&store, key_ref),
                Err(ConfidentialKeyStoreError::InvalidReference)
            ));
        }
        assert!(store.values.lock().unwrap().is_empty());
    }
}
