//! Confidential persistence for exact provider requests used by recovery.

use std::sync::Mutex;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use wcore_config::confidential_blob::{
    ConfidentialBlobAad, ConfidentialBlobKey, ConfidentialKeyStoreError,
    load_confidential_blob_key, load_or_create_confidential_blob_key, open_confidential_blob,
    seal_confidential_blob,
};
use wcore_config::config::Config;

/// The single source of this identifier is `wcore_config`, so the profile-delete
/// purge (`purge_profile_confidential_keys`) deletes exactly what this writes.
/// Two independent spellings is how a key ends up with a writer and no deleter.
const KEY_REF: &str = wcore_config::credentials::RECOVERY_PREPARED_REQUEST_KEY_REF;
const PURPOSE: &str = "recovery.prepared-provider-request.v1";
const ENVELOPE_VERSION: u8 = 1;
const ALGORITHM: &str = "xchacha20-poly1305";

/// Versioned encrypted request carried by a recovery checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SealedPreparedRequest {
    pub(crate) envelope_version: u8,
    pub(crate) algorithm: String,
    pub(crate) ciphertext: String,
}

impl SealedPreparedRequest {
    pub(crate) fn validate(&self) -> Result<(), RecoveryConfidentialError> {
        if self.envelope_version != ENVELOPE_VERSION || self.algorithm != ALGORITHM {
            return Err(RecoveryConfidentialError::Invalid);
        }
        let blob = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&self.ciphertext)
            .map_err(|_| RecoveryConfidentialError::Invalid)?;
        if blob.is_empty()
            || base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&blob) != self.ciphertext
        {
            return Err(RecoveryConfidentialError::Invalid);
        }
        Ok(())
    }
}

/// Durable identities authenticated with one exact prepared request.
#[derive(Debug, Clone)]
pub(crate) struct PreparedRequestBinding<'a> {
    pub(crate) session_id: &'a str,
    pub(crate) turn_id: &'a str,
    pub(crate) checkpoint_id: &'a str,
    pub(crate) checkpoint_version: u64,
    pub(crate) dispatch_id: &'a str,
    pub(crate) conversation_id: &'a str,
    pub(crate) conversation_digest: &'a str,
    pub(crate) message_count: u64,
    pub(crate) request_digest: &'a str,
    pub(crate) turn_index: u64,
    pub(crate) stream_attempt: u32,
    pub(crate) overflow_retried: bool,
    pub(crate) length_wedge_retried: bool,
    pub(crate) posture_authority_digest: &'a str,
}

/// Confidential request failures omit key material, payload, ciphertext and
/// associated-data details.
///
/// They do NOT omit which *configured* backend was refused. That value is
/// written in the operator's own cleartext config file, so repeating it
/// discloses nothing — while collapsing it produced the live UAT defect D3:
/// three unrelated causes rendered as one string that told a user to configure
/// a credentials backend they had already configured.
///
/// # Whether these are fatal is a decided question — read the ADR before changing it
///
/// `NoSecureBackendAvailable` in particular has been decided **twice, in opposite
/// directions**: refuse the turn (2026-07-16, `906287e1`, "fail closed instead of
/// replaying ambiguous effects") and then degrade durable sessions off and run
/// (2026-07-30, `c73ac417`, a release blocker on every keyring-less Linux host).
/// The second decision was taken by a cross-audit panel that was never shown the
/// first, and it turned the first one's test red.
///
/// Both decisions, the measured causation between them, the refutation of the
/// second one's reasoning, and what a future revisit must have in front of it are
/// merged into `docs/decisions/0003-durable-sessions-without-a-secure-store.md`.
///
/// If you arrived here from a red assertion in
/// `crates/wcore-cli/tests/f14_sigkill_recovery.rs`, that test is **not stale** —
/// it encodes the 2026-07-16 side of a live disagreement. Read ADR 0003 §7 before
/// re-pointing or deleting it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum RecoveryConfidentialError {
    #[error(
        "storage.credentials.backend is set to \"plaintext\", which cannot hold the confidential \
         key that durable session recovery requires. Unlock an encrypted vault by setting \
         WAYLAND_VAULT_PASSPHRASE_FD (a passphrase file descriptor — preferred) or \
         WAYLAND_VAULT_PASSPHRASE, or set [storage.credentials] backend = \"keyring\", or turn \
         durable sessions off with [session] enabled = false"
    )]
    PlaintextBackendRejected,
    #[error(
        "secure recovery storage is unavailable: no OS keyring was usable and no encrypted \
         credentials vault is unlocked. On a headless host set WAYLAND_VAULT_PASSPHRASE_FD (a \
         passphrase file descriptor — preferred) or WAYLAND_VAULT_PASSPHRASE to unlock the \
         encrypted vault, or turn durable sessions off with [session] enabled = false"
    )]
    NoSecureBackendAvailable,
    #[error(
        "secure recovery storage could not be read: the configured store rejected this profile's \
         recovery key. An encrypted vault opened with the wrong unlock passphrase reads this way \
         — re-check the passphrase for this profile"
    )]
    SecureStoreUnreadable,
    #[error(
        "this profile has no stored recovery key, so a sealed request cannot be opened. The key \
         is created when a new turn starts on a confidential-capable backend"
    )]
    MissingRecoveryKey,
    #[error("secure recovery storage is unavailable")]
    Unavailable,
    #[error("recovery confidential request is invalid")]
    Invalid,
}

/// The statically decidable half of the confidential-storage requirement.
///
/// `credentials.backend = "plaintext"` can never satisfy it — that refusal is
/// deliberate security design and is unchanged here. What changes is *when* the
/// operator hears about it: this is a pure function of config with no side
/// effects, so a persisted session can refuse to open instead of accepting the
/// session and failing every turn afterwards.
pub(crate) fn reject_backend_without_confidential_storage(
    config: &Config,
) -> Result<(), RecoveryConfidentialError> {
    if config
        .storage
        .credentials
        .backend
        .supports_confidential_material()
    {
        Ok(())
    } else {
        Err(RecoveryConfidentialError::PlaintextBackendRejected)
    }
}

/// Lazily caches a successfully loaded key for one engine. Backend failures
/// are not cached, so unlocking the configured store can make a later retry
/// succeed without restarting Core.
#[derive(Default)]
pub(crate) struct RecoveryRequestProtector {
    key: Mutex<Option<ConfidentialBlobKey>>,
}

pub(crate) trait RecoveryRequestProtection: Send + Sync {
    /// Prove that crash-durable request protection is available before a
    /// journaled turn is accepted. This may create the profile's sealing key,
    /// but it never writes request content.
    fn preflight(&self, config: &Config) -> Result<(), RecoveryConfidentialError>;

    fn seal(
        &self,
        config: &Config,
        binding: &PreparedRequestBinding<'_>,
        request: &serde_json::Value,
    ) -> Result<SealedPreparedRequest, RecoveryConfidentialError>;

    fn open(
        &self,
        config: &Config,
        binding: &PreparedRequestBinding<'_>,
        sealed: &SealedPreparedRequest,
    ) -> Result<serde_json::Value, RecoveryConfidentialError>;
}

impl RecoveryRequestProtection for RecoveryRequestProtector {
    fn preflight(&self, config: &Config) -> Result<(), RecoveryConfidentialError> {
        self.with_key(config, true, |_| Ok(()))
    }

    fn seal(
        &self,
        config: &Config,
        binding: &PreparedRequestBinding<'_>,
        request: &serde_json::Value,
    ) -> Result<SealedPreparedRequest, RecoveryConfidentialError> {
        self.with_key(config, true, |key| seal_with_key(key, binding, request))
    }

    fn open(
        &self,
        config: &Config,
        binding: &PreparedRequestBinding<'_>,
        sealed: &SealedPreparedRequest,
    ) -> Result<serde_json::Value, RecoveryConfidentialError> {
        self.with_key(config, false, |key| open_with_key(key, binding, sealed))
    }
}

impl RecoveryRequestProtector {
    #[cfg(any(test, feature = "test-utils"))]
    pub(crate) fn with_test_key(bytes: &[u8; 32]) -> Self {
        Self {
            key: Mutex::new(Some(
                ConfidentialBlobKey::from_slice(bytes).expect("fixed recovery test key"),
            )),
        }
    }

    fn with_key<T>(
        &self,
        config: &Config,
        create: bool,
        operation: impl FnOnce(&ConfidentialBlobKey) -> Result<T, RecoveryConfidentialError>,
    ) -> Result<T, RecoveryConfidentialError> {
        let mut key = self
            .key
            .lock()
            .map_err(|_| RecoveryConfidentialError::Unavailable)?;
        if key.is_none() {
            // Decide the config-determined cause before touching any store, so
            // a plaintext backend is never reported as an environment problem.
            reject_backend_without_confidential_storage(config)?;
            let store = config
                .open_confidential_credentials_store()
                .map_err(|_| RecoveryConfidentialError::NoSecureBackendAvailable)?;
            let loaded = if create {
                load_or_create_confidential_blob_key(&store, KEY_REF)
            } else {
                load_confidential_blob_key(&store, KEY_REF)
            };
            // The store opened, so the backend exists; a failure past this
            // point is about the key itself, not about availability.
            *key = Some(loaded.map_err(|error| match error {
                ConfidentialKeyStoreError::ReadFailed
                | ConfidentialKeyStoreError::MalformedStoredKey => {
                    RecoveryConfidentialError::SecureStoreUnreadable
                }
                ConfidentialKeyStoreError::MissingStoredKey => {
                    RecoveryConfidentialError::MissingRecoveryKey
                }
                _ => RecoveryConfidentialError::Unavailable,
            })?);
        }
        operation(key.as_ref().ok_or(RecoveryConfidentialError::Unavailable)?)
    }
}

fn seal_with_key(
    key: &ConfidentialBlobKey,
    binding: &PreparedRequestBinding<'_>,
    request: &serde_json::Value,
) -> Result<SealedPreparedRequest, RecoveryConfidentialError> {
    let plaintext = serde_json::to_vec(request).map_err(|_| RecoveryConfidentialError::Invalid)?;
    let aad = request_aad(binding)?;
    let blob = seal_confidential_blob(key, &aad, &plaintext)
        .map_err(|_| RecoveryConfidentialError::Invalid)?;
    Ok(SealedPreparedRequest {
        envelope_version: ENVELOPE_VERSION,
        algorithm: ALGORITHM.to_owned(),
        ciphertext: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(blob),
    })
}

fn open_with_key(
    key: &ConfidentialBlobKey,
    binding: &PreparedRequestBinding<'_>,
    sealed: &SealedPreparedRequest,
) -> Result<serde_json::Value, RecoveryConfidentialError> {
    sealed.validate()?;
    let blob = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(&sealed.ciphertext)
        .map_err(|_| RecoveryConfidentialError::Invalid)?;
    if base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&blob) != sealed.ciphertext {
        return Err(RecoveryConfidentialError::Invalid);
    }
    let aad = request_aad(binding)?;
    let plaintext =
        open_confidential_blob(key, &aad, &blob).map_err(|_| RecoveryConfidentialError::Invalid)?;
    serde_json::from_slice(&plaintext).map_err(|_| RecoveryConfidentialError::Invalid)
}

fn request_aad(
    binding: &PreparedRequestBinding<'_>,
) -> Result<ConfidentialBlobAad, RecoveryConfidentialError> {
    if binding.session_id.is_empty()
        || binding.turn_id.is_empty()
        || binding.checkpoint_id.is_empty()
        || binding.dispatch_id.is_empty()
        || binding.conversation_id.is_empty()
        || binding.conversation_digest.is_empty()
        || binding.request_digest.is_empty()
        || binding.posture_authority_digest.is_empty()
    {
        return Err(RecoveryConfidentialError::Invalid);
    }
    let mut canonical = Vec::new();
    for field in [
        binding.session_id,
        binding.turn_id,
        binding.checkpoint_id,
        binding.dispatch_id,
        binding.conversation_id,
        binding.conversation_digest,
        binding.request_digest,
        binding.posture_authority_digest,
    ] {
        let length = u32::try_from(field.len()).map_err(|_| RecoveryConfidentialError::Invalid)?;
        canonical.extend_from_slice(&length.to_be_bytes());
        canonical.extend_from_slice(field.as_bytes());
    }
    canonical.extend_from_slice(&binding.checkpoint_version.to_be_bytes());
    canonical.extend_from_slice(&binding.message_count.to_be_bytes());
    canonical.extend_from_slice(&binding.turn_index.to_be_bytes());
    canonical.extend_from_slice(&binding.stream_attempt.to_be_bytes());
    canonical.push(u8::from(binding.overflow_retried));
    canonical.push(u8::from(binding.length_wedge_retried));
    Ok(ConfidentialBlobAad::new(PURPOSE, canonical))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_config::credentials::{CredentialsBackend, CredentialsStorageConfig};

    fn binding<'a>() -> PreparedRequestBinding<'a> {
        PreparedRequestBinding {
            session_id: "session-a",
            turn_id: "turn-a",
            checkpoint_id: "checkpoint-a",
            checkpoint_version: 3,
            dispatch_id: "dispatch-a",
            conversation_id: "conversation-a",
            conversation_digest: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            message_count: 2,
            request_digest: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
            turn_index: 1,
            stream_attempt: 0,
            overflow_retried: false,
            length_wedge_retried: false,
            posture_authority_digest: "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
        }
    }

    #[test]
    fn sealed_request_roundtrips_without_plaintext() {
        let key = ConfidentialBlobKey::generate();
        let request = serde_json::json!({"secret": "F14-UNIQUE-PLAINTEXT-SENTINEL"});

        let sealed = seal_with_key(&key, &binding(), &request).unwrap();

        assert!(!sealed.ciphertext.contains("F14-UNIQUE-PLAINTEXT-SENTINEL"));
        assert_eq!(open_with_key(&key, &binding(), &sealed).unwrap(), request);
    }

    #[test]
    fn wrong_binding_key_or_ciphertext_fails_closed() {
        let key = ConfidentialBlobKey::generate();
        let request = serde_json::json!({"request": "exact"});
        let sealed = seal_with_key(&key, &binding(), &request).unwrap();

        let mut wrong_binding = binding();
        wrong_binding.dispatch_id = "dispatch-b";
        assert_eq!(
            open_with_key(&key, &wrong_binding, &sealed),
            Err(RecoveryConfidentialError::Invalid)
        );
        assert_eq!(
            open_with_key(&ConfidentialBlobKey::generate(), &binding(), &sealed),
            Err(RecoveryConfidentialError::Invalid)
        );

        let mut tampered = sealed;
        let last = tampered.ciphertext.pop().unwrap();
        tampered
            .ciphertext
            .push(if last == 'A' { 'B' } else { 'A' });
        assert_eq!(
            open_with_key(&key, &binding(), &tampered),
            Err(RecoveryConfidentialError::Invalid)
        );
    }

    #[test]
    fn every_durable_binding_field_is_authenticated() {
        let key = ConfidentialBlobKey::generate();
        let request = serde_json::json!({"request": "exact"});
        let original = binding();
        let sealed = seal_with_key(&key, &original, &request).unwrap();
        let mut changed = Vec::new();

        macro_rules! changed_binding {
            ($field:ident, $value:expr) => {{
                let mut binding = original.clone();
                binding.$field = $value;
                changed.push(binding);
            }};
        }
        changed_binding!(session_id, "session-b");
        changed_binding!(turn_id, "turn-b");
        changed_binding!(checkpoint_id, "checkpoint-b");
        changed_binding!(checkpoint_version, 4);
        changed_binding!(dispatch_id, "dispatch-b");
        changed_binding!(conversation_id, "conversation-b");
        changed_binding!(
            conversation_digest,
            "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"
        );
        changed_binding!(message_count, 3);
        changed_binding!(
            request_digest,
            "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"
        );
        changed_binding!(turn_index, 2);
        changed_binding!(stream_attempt, 1);
        changed_binding!(overflow_retried, true);
        changed_binding!(length_wedge_retried, true);
        changed_binding!(
            posture_authority_digest,
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
        );

        for changed_binding in changed {
            assert_eq!(
                open_with_key(&key, &changed_binding, &sealed),
                Err(RecoveryConfidentialError::Invalid)
            );
        }
    }

    #[test]
    fn noncanonical_or_unknown_envelope_fails_closed() {
        let key = ConfidentialBlobKey::generate();
        let request = serde_json::json!({"request": "exact"});
        let mut sealed = seal_with_key(&key, &binding(), &request).unwrap();

        sealed.ciphertext.push('=');
        assert_eq!(
            open_with_key(&key, &binding(), &sealed),
            Err(RecoveryConfidentialError::Invalid)
        );

        let mut sealed = seal_with_key(&key, &binding(), &request).unwrap();
        sealed.algorithm = "unknown".to_owned();
        assert_eq!(
            open_with_key(&key, &binding(), &sealed),
            Err(RecoveryConfidentialError::Invalid)
        );
    }

    #[test]
    fn errors_never_render_request_or_binding_material() {
        let key = ConfidentialBlobKey::generate();
        let request_secret = "F14-REQUEST-SECRET";
        let binding_secret = "F14-BINDING-SECRET";
        let request = serde_json::json!({"secret": request_secret});
        let mut bound = binding();
        bound.turn_id = binding_secret;
        let mut sealed = seal_with_key(&key, &bound, &request).unwrap();
        sealed.ciphertext.push('=');

        let rendered = open_with_key(&key, &bound, &sealed)
            .unwrap_err()
            .to_string();
        assert!(!rendered.contains(request_secret));
        assert!(!rendered.contains(binding_secret));
    }

    fn config_with_backend(backend: CredentialsBackend) -> Config {
        let mut config = Config::default();
        config.storage.credentials = CredentialsStorageConfig {
            backend,
            service_name: None,
        };
        config
    }

    /// D3: the plaintext backend used to be reported as "secure recovery
    /// storage is unavailable; configure an OS keyring or encrypted credentials
    /// vault" — guidance for a user who has configured nothing, given to a user
    /// who has configured exactly the one value that is fatal. The failure must
    /// name itself and name the setting to change.
    #[test]
    fn preflight_fails_with_actionable_guidance_before_request_persistence() {
        let error = RecoveryRequestProtector::default()
            .preflight(&config_with_backend(CredentialsBackend::Plaintext))
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("plaintext"),
            "the cause must be named: {error}"
        );
        assert!(
            error.contains("credentials.backend"),
            "the setting to change must be named: {error}"
        );
        assert!(
            error.contains("session"),
            "the user must be told which capability requires it: {error}"
        );
    }

    /// Every `backend = "<value>"` a remediation string tells an operator to
    /// write must actually be accepted by the config parser it will be written
    /// into.
    ///
    /// This exists because it was NOT true. Both messages advertised
    /// `credentials.backend = "encrypted-file"`, and all three of its parts were
    /// wrong at once, measured live on a keyring-less host:
    ///   * `[credentials]` is not a section — the loader logs "ignoring unknown
    ///     or mis-sectioned config key `credentials` … it has no effect" and
    ///     then re-emits this identical error. A closed loop.
    ///   * at the real section `[storage.credentials]`, `"encrypted-file"` is
    ///     rejected: `unknown variant, expected one of auto, plaintext,
    ///     keyring, encrypted_file` — the config no longer loads AT ALL, so
    ///     following the advice is strictly worse than ignoring it.
    ///   * even `"encrypted_file"` fails, because the variant is
    ///     `EncryptedFile { cipher_path, key_params_path }` — a struct variant
    ///     that can never be a bare string in any spelling.
    ///
    /// The gate can fail: it re-parses whatever the messages say through the
    /// real `CredentialsStorageConfig`, so re-introducing any unrepresentable
    /// value reds it. Verified red against the pre-fix strings.
    #[test]
    fn every_backend_value_the_messages_advertise_actually_parses() {
        let messages = [
            RecoveryConfidentialError::PlaintextBackendRejected.to_string(),
            RecoveryConfidentialError::NoSecureBackendAvailable.to_string(),
            RecoveryConfidentialError::SecureStoreUnreadable.to_string(),
            RecoveryConfidentialError::MissingRecoveryKey.to_string(),
        ];

        let mut checked = 0usize;
        for message in &messages {
            // Pull every `backend = "value"` / `backend to "value"` the text
            // offers, however it is phrased around the quotes.
            for (index, _) in message.match_indices("backend") {
                let tail = &message[index..];
                let Some(open) = tail.find('"') else { continue };
                let Some(len) = tail[open + 1..].find('"') else {
                    continue;
                };
                let value = &tail[open + 1..open + 1 + len];
                // Only a bare word can be a backend value; skip prose quotes.
                if value.is_empty() || value.contains(' ') {
                    continue;
                }
                let toml = format!("backend = \"{value}\"");
                assert!(
                    toml::from_str::<CredentialsStorageConfig>(&toml).is_ok(),
                    "message advertises backend = \"{value}\", which \
                     [storage.credentials] rejects. An operator who follows this \
                     text literally ends up with a config that will not load.\n\
                     message: {message}"
                );
                checked += 1;
            }
        }
        assert!(
            checked > 0,
            "no backend value was extracted from any message — the gate would \
             pass vacuously; fix the extraction, do not delete the assert"
        );
    }

    /// The messages must point at a remedy that a headless operator can reach
    /// with product-supplied information alone.
    ///
    /// Measured: `WAYLAND_VAULT_PASSPHRASE` appears in ZERO files under `docs/`,
    /// in ZERO bytes of `--help`, and (before this fix) in no error message —
    /// yet setting it, with no config change whatsoever, is the ONE thing that
    /// makes a default install complete a turn on a host with no OS keyring.
    #[test]
    fn the_unavailable_message_names_a_remedy_an_operator_can_actually_perform() {
        let message = RecoveryConfidentialError::NoSecureBackendAvailable.to_string();

        assert!(
            message.contains("WAYLAND_VAULT_PASSPHRASE"),
            "the vault unlock transport is the only remedy that works without a \
             config change, and it is documented nowhere else: {message}"
        );

        // The other advertised escape must be spelled exactly as the config
        // schema accepts it, not described in prose.
        assert!(
            message.contains("[session] enabled = false"),
            "the persistence-off escape must be given as a writable config key: {message}"
        );
        assert!(
            toml::from_str::<wcore_config::config::SessionConfig>("enabled = false").is_ok(),
            "the key this message advertises must exist in SessionConfig"
        );
    }

    /// D3/D8: the plaintext backend and an unavailable secure backend are
    /// different problems with different fixes, so they must not render as one
    /// indistinguishable string.
    #[test]
    fn distinct_confidential_failures_do_not_share_one_message() {
        let plaintext = RecoveryConfidentialError::PlaintextBackendRejected.to_string();
        let unavailable = RecoveryConfidentialError::NoSecureBackendAvailable.to_string();
        let unreadable = RecoveryConfidentialError::SecureStoreUnreadable.to_string();

        assert_ne!(plaintext, unavailable);
        assert_ne!(plaintext, unreadable);
        assert_ne!(unavailable, unreadable);
    }

    /// The static rule the session-open check uses. Refusing plaintext for
    /// confidential material is the security property being preserved, not
    /// relaxed.
    #[test]
    fn only_plaintext_is_statically_rejected() {
        assert_eq!(
            reject_backend_without_confidential_storage(&config_with_backend(
                CredentialsBackend::Plaintext
            )),
            Err(RecoveryConfidentialError::PlaintextBackendRejected)
        );
        assert_eq!(
            reject_backend_without_confidential_storage(&config_with_backend(
                CredentialsBackend::Auto
            )),
            Ok(())
        );
        assert_eq!(
            reject_backend_without_confidential_storage(&config_with_backend(
                CredentialsBackend::Keyring
            )),
            Ok(())
        );
    }

    /// Naming the configured backend is not a disclosure — the value is written
    /// in the user's own cleartext config. Key material, ciphertext and AAD
    /// still must never appear.
    #[test]
    fn cause_specific_messages_still_render_no_secret_material() {
        for error in [
            RecoveryConfidentialError::PlaintextBackendRejected,
            RecoveryConfidentialError::NoSecureBackendAvailable,
            RecoveryConfidentialError::SecureStoreUnreadable,
            RecoveryConfidentialError::MissingRecoveryKey,
            RecoveryConfidentialError::Unavailable,
            RecoveryConfidentialError::Invalid,
        ] {
            let rendered = error.to_string();
            assert!(!rendered.contains(KEY_REF), "key ref leaked: {rendered}");
            assert!(
                !rendered.contains(PURPOSE),
                "AAD purpose leaked: {rendered}"
            );
            assert!(
                !rendered.contains(ALGORITHM),
                "cipher detail leaked: {rendered}"
            );
        }
    }
}
