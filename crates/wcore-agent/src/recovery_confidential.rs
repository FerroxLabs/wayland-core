//! Confidential persistence for exact provider requests used by recovery.

use std::sync::Mutex;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use wcore_config::confidential_blob::{
    ConfidentialBlobAad, ConfidentialBlobKey, load_confidential_blob_key,
    load_or_create_confidential_blob_key, open_confidential_blob, seal_confidential_blob,
};
use wcore_config::config::Config;

const KEY_REF: &str = "wayland-core.recovery.prepared-request.v1";
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

/// Confidential request failures intentionally omit backend, key, payload,
/// ciphertext, and associated-data details.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum RecoveryConfidentialError {
    #[error(
        "secure recovery storage is unavailable; configure an OS keyring or encrypted credentials vault"
    )]
    Unavailable,
    #[error("recovery confidential request is invalid")]
    Invalid,
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
            let store = config
                .open_confidential_credentials_store()
                .map_err(|_| RecoveryConfidentialError::Unavailable)?;
            let loaded = if create {
                load_or_create_confidential_blob_key(&store, KEY_REF)
            } else {
                load_confidential_blob_key(&store, KEY_REF)
            };
            *key = Some(loaded.map_err(|_| RecoveryConfidentialError::Unavailable)?);
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

    #[test]
    fn preflight_fails_with_actionable_guidance_before_request_persistence() {
        let mut config = Config::default();
        config.storage.credentials = CredentialsStorageConfig {
            backend: CredentialsBackend::Plaintext,
            service_name: None,
        };

        let error = RecoveryRequestProtector::default()
            .preflight(&config)
            .unwrap_err()
            .to_string();

        assert!(error.contains("secure recovery storage is unavailable"));
        assert!(error.contains("OS keyring or encrypted credentials vault"));
    }
}
