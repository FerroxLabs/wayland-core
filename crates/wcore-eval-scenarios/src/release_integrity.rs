//! Signed release manifests and the role-scoped release trust root.
//!
//! Three separations are load-bearing here, and each exists because this
//! repository has already been burned by its absence.
//!
//! 1. **Authority is never read from the document.** A manifest carries a key
//!    id and a detached signature; it never carries a boolean saying it is
//!    trusted. [`ReleaseTrustRootV1`] must be supplied independently, exactly
//!    as [`crate::receipt::ReceiptVerifier::trust_ci_key`] requires its key
//!    from outside the receipt.
//!
//! 2. **Every signed object is domain-separated.** `receipt.rs` separates its
//!    signature message; `wcore-cli`'s `plugin/index.rs` does not — it verifies
//!    Ed25519 directly over `serde_json::to_vec(&body)`. This module follows
//!    the former. A receipt signature can never verify as a manifest signature,
//!    and a signature minted for one release state can never be replayed into
//!    another, because the state's own name is inside the signed message.
//!
//! 3. **Roles are the four release states and nothing else.** [`ReleaseState`]
//!    is a closed enum with no catch-all and no default, so an unrecognised
//!    state name fails at deserialization rather than mapping to something
//!    permissive. The state ledger built on it lives in
//!    [`crate::release_states`].
//!
//! Nothing in this module invents a receipt field. [`CertificationBindingV1`]
//! names only fields that exist in [`crate::receipt`] today, and it is carried
//! as [`Evidence<CertificationBindingV1>`] so its absence is explicit rather
//! than an empty success.

use std::collections::BTreeSet;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::receipt::Evidence;

pub const RELEASE_MANIFEST_SCHEMA: &str = "wayland.release.manifest";
pub const RELEASE_MANIFEST_SCHEMA_VERSION: u32 = 1;
pub const RELEASE_TRUST_ROOT_SCHEMA: &str = "wayland.release.trust-root";
pub const RELEASE_TRUST_ROOT_SCHEMA_VERSION: u32 = 1;

/// The release manifest's own signature domain. Distinct from
/// `receipt.rs`'s `wayland.eval.receipt.v1\0` and from every per-state domain
/// in [`ReleaseState::signature_domain`], so no signature minted over one
/// object can be replayed onto another.
const MANIFEST_SIGNATURE_DOMAIN: &[u8] = b"wayland.release.manifest.v1\0";

// ---------------------------------------------------------------------------
// The four release states — a closed enum, used both as a ledger state and as
// a trust-root role.
// ---------------------------------------------------------------------------

/// The four release states, in canonical order.
///
/// There is no catch-all variant, no `#[serde(other)]`, and no `Default`. An
/// unrecognised state name is a deserialization failure, not a fallback. That
/// is deliberate: a prose separation is exactly what an agent under time
/// pressure walked around on this program, so the separation is expressed as a
/// type that refuses before any logic runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseState {
    Packaging,
    DeploymentPreparation,
    RollbackRehearsal,
    ReleaseAcceptance,
}

/// The canonical order. A verified chain must match this prefix exactly.
pub const CANONICAL_RELEASE_STATES: [ReleaseState; 4] = [
    ReleaseState::Packaging,
    ReleaseState::DeploymentPreparation,
    ReleaseState::RollbackRehearsal,
    ReleaseState::ReleaseAcceptance,
];

impl ReleaseState {
    /// Position in [`CANONICAL_RELEASE_STATES`].
    pub fn ordinal(self) -> u8 {
        match self {
            Self::Packaging => 0,
            Self::DeploymentPreparation => 1,
            Self::RollbackRehearsal => 2,
            Self::ReleaseAcceptance => 3,
        }
    }

    /// This state's own signature domain. The state name is inside the signed
    /// message, so a signature minted for one state cannot be replayed into
    /// another even when the same key produced it.
    pub fn signature_domain(self) -> &'static [u8] {
        match self {
            Self::Packaging => b"wayland.release.state.packaging.v1\0",
            Self::DeploymentPreparation => b"wayland.release.state.deployment-preparation.v1\0",
            Self::RollbackRehearsal => b"wayland.release.state.rollback-rehearsal.v1\0",
            Self::ReleaseAcceptance => b"wayland.release.state.release-acceptance.v1\0",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Packaging => "packaging",
            Self::DeploymentPreparation => "deployment_preparation",
            Self::RollbackRehearsal => "rollback_rehearsal",
            Self::ReleaseAcceptance => "release_acceptance",
        }
    }
}

// ---------------------------------------------------------------------------
// Authority
// ---------------------------------------------------------------------------

/// A detached signature plus the id of the key that produced it.
///
/// Never a boolean. Possession of this claim establishes nothing; the key id
/// must resolve in an independently supplied [`ReleaseTrustRootV1`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseAuthorityClaimV1 {
    pub key_id: String,
    pub signature_base64: String,
}

// ---------------------------------------------------------------------------
// Trust root
// ---------------------------------------------------------------------------

/// The only thing this system believes without proof. It must arrive
/// independently of every document it validates.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTrustRootV1 {
    pub schema: String,
    pub schema_version: u32,
    pub keys: Vec<TrustedKeyV1>,
}

/// One key bound to exactly one role. A key retired at or before the
/// evaluation instant is refused even when its signature is cryptographically
/// valid.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct TrustedKeyV1 {
    pub key_id: String,
    pub public_key_base64: String,
    pub role: ReleaseState,
    pub valid_from: u64,
    pub retired_at: Option<u64>,
}

impl ReleaseTrustRootV1 {
    pub fn new(keys: Vec<TrustedKeyV1>) -> Self {
        Self {
            schema: RELEASE_TRUST_ROOT_SCHEMA.to_string(),
            schema_version: RELEASE_TRUST_ROOT_SCHEMA_VERSION,
            keys,
        }
    }

    /// Resolve `key_id` to a usable verifying key, enforcing schema, role and
    /// validity window. Each refusal is a distinct typed error so a caller
    /// never matches on a string.
    pub fn resolve(
        &self,
        key_id: &str,
        required_role: ReleaseState,
        now: u64,
    ) -> Result<VerifyingKey, ReleaseIntegrityError> {
        if self.schema != RELEASE_TRUST_ROOT_SCHEMA
            || self.schema_version != RELEASE_TRUST_ROOT_SCHEMA_VERSION
        {
            return Err(ReleaseIntegrityError::UnsupportedSchema {
                schema: self.schema.clone(),
                version: self.schema_version,
            });
        }
        let entry = self
            .keys
            .iter()
            .find(|candidate| candidate.key_id == key_id)
            .ok_or_else(|| ReleaseIntegrityError::UnknownKeyId(key_id.to_string()))?;
        if entry.role != required_role {
            return Err(ReleaseIntegrityError::RoleMismatch {
                key_id: key_id.to_string(),
                required: required_role,
                bound: entry.role,
            });
        }
        if now < entry.valid_from {
            return Err(ReleaseIntegrityError::KeyNotYetValid(key_id.to_string()));
        }
        if entry.retired_at.is_some_and(|retired| now >= retired) {
            return Err(ReleaseIntegrityError::RetiredKey(key_id.to_string()));
        }
        decode_public_key(&entry.public_key_base64)
    }
}

// ---------------------------------------------------------------------------
// The release manifest
// ---------------------------------------------------------------------------

/// What a release IS: its source, its packaged artifacts, its SBOM, the
/// dependency-policy outcome, the reproducibility verdict, and the binding to
/// the evaluation receipt that certified it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifestV1 {
    pub schema: String,
    pub schema_version: u32,
    pub body_sha256: String,
    pub body: ReleaseManifestBodyV1,
    pub authority: ReleaseAuthorityClaimV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManifestBodyV1 {
    pub release_id: String,
    pub source_commit: String,
    /// Non-empty. Each distributed artifact bound by name and digest.
    pub artifacts: Vec<PackagedArtifactV1>,
    pub sbom: Evidence<SbomReferenceV1>,
    pub dependency_policy: Evidence<DependencyPolicyOutcomeV1>,
    pub reproducibility: ReproducibilityVerdictV1,
    /// The Phase 28 seam. Representable as unavailable so this manifest is
    /// buildable and verifiable before Phase 28's receipt extensions land.
    pub certification: Evidence<CertificationBindingV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PackagedArtifactV1 {
    pub name: String,
    pub sha256: String,
    pub byte_length: u64,
    pub kind: ArtifactKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactKind {
    Archive,
    Checksums,
    Sbom,
    Attestation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SbomReferenceV1 {
    pub name: String,
    pub sha256: String,
    pub format: SbomFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SbomFormat {
    CycloneDxJson,
    SpdxJson,
}

/// The tool that produced the outcome, the digest of the configuration it ran
/// against, and its result. The policy digest is what makes "it passed"
/// meaningful — a pass against an empty policy is not a pass.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DependencyPolicyOutcomeV1 {
    pub tool: String,
    pub policy_sha256: String,
    pub result: PolicyResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PolicyResult {
    Pass,
    Fail,
}

/// Either the build reproduced, or it did not and the variance is documented.
/// There is no third option and no silent "unknown" that reads as success.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum ReproducibilityVerdictV1 {
    Reproduced,
    Variance {
        class: VarianceClass,
        evidence_sha256: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VarianceClass {
    Timestamp,
    PathPrefix,
    BuildId,
    Unclassified,
}

/// The join between a Phase 28 evaluation receipt and this release.
///
/// Every field named here exists in [`crate::receipt`] today: `body_sha256`,
/// `schema`, `schema_version`, the `AuthorityClaimV1::Ci` key id, and
/// `identity.source_commit`, `identity.binary_sha256`, `target.os`,
/// `target.architecture`. Phase 29 invents no receipt field; what Phase 28
/// must ADD is stated as requirements R28-A..R28-F in
/// `29-01-RECEIPT-INTERFACE.md`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CertificationBindingV1 {
    pub receipt_body_sha256: String,
    pub receipt_schema: String,
    pub receipt_schema_version: u32,
    pub receipt_signing_key_id: String,
    pub source_commit: String,
    pub binary_sha256: String,
    pub target_os: String,
    pub target_architecture: String,
}

// ---------------------------------------------------------------------------
// Errors — one variant per cause, so no caller matches on a string.
// ---------------------------------------------------------------------------

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ReleaseIntegrityError {
    #[error("invalid JSON: {0}")]
    InvalidJson(String),
    #[error("unsupported schema {schema} version {version}")]
    UnsupportedSchema { schema: String, version: u32 },
    #[error("body digest mismatch")]
    DigestMismatch,
    #[error("invalid body: {0}")]
    InvalidBody(String),
    #[error("key id is not in the trust root: {0}")]
    UnknownKeyId(String),
    #[error("key {key_id} is bound to role {bound:?} but role {required:?} is required")]
    RoleMismatch {
        key_id: String,
        required: ReleaseState,
        bound: ReleaseState,
    },
    #[error("key is not yet valid: {0}")]
    KeyNotYetValid(String),
    #[error("key is retired: {0}")]
    RetiredKey(String),
    #[error("trusted public key is not a 32-byte base64 Ed25519 key")]
    InvalidPublicKey,
    #[error("signing key is not a 32-byte base64 Ed25519 seed")]
    InvalidSigningKey,
    #[error("signature is malformed")]
    MalformedSignature,
    #[error("signature verification failed")]
    InvalidSignature,
    // --- state ledger ---
    #[error("record at position {position} declares state {declared:?} but ordinal {ordinal}")]
    OrdinalMismatch {
        position: usize,
        declared: ReleaseState,
        ordinal: u8,
    },
    #[error("record at position {position} is {found:?} but canonical order requires {expected:?}")]
    NonCanonicalOrder {
        position: usize,
        expected: ReleaseState,
        found: ReleaseState,
    },
    #[error("chain holds {count} records but there are only 4 release states")]
    TooManyRecords { count: usize },
    #[error("record at position {position} binds a different release manifest")]
    ManifestMismatch { position: usize },
    #[error("record at position {position} has a broken previous-record digest")]
    PreviousDigestMismatch { position: usize },
    #[error("key id {key_id} signs more than one state in the same chain")]
    DuplicateKeyId { key_id: String },
    #[error("state {state:?} has an empty evidence set")]
    EmptyEvidence { state: ReleaseState },
    #[error("state {state:?} reuses evidence digest {sha256} from an earlier state")]
    EvidenceReuse { state: ReleaseState, sha256: String },
    #[error("release acceptance requires an observed certification binding")]
    UnavailableCertificationAtAcceptance,
}

// ---------------------------------------------------------------------------
// Manifest construction, signing and verification
// ---------------------------------------------------------------------------

impl ReleaseManifestV1 {
    /// Build an unsigned manifest, content-addressed by a digest over its body
    /// computed the way `receipt.rs` computes its own.
    ///
    /// The returned manifest carries a structurally-valid but empty authority
    /// claim; it cannot verify until [`ReleaseManifestV1::sign`] is called, and
    /// verification always requires an external trust root regardless.
    pub fn unsigned(body: ReleaseManifestBodyV1) -> Result<Self, ReleaseIntegrityError> {
        validate_manifest_body(&body)?;
        Ok(Self {
            schema: RELEASE_MANIFEST_SCHEMA.to_string(),
            schema_version: RELEASE_MANIFEST_SCHEMA_VERSION,
            body_sha256: hash_serializable(&body)?,
            body,
            authority: ReleaseAuthorityClaimV1 {
                key_id: String::new(),
                signature_base64: String::new(),
            },
        })
    }

    /// Attach a detached signature over the domain-separated message. Holding
    /// this object never makes it authoritative.
    pub fn sign(mut self, key_id: impl Into<String>, key: &SigningKey) -> Self {
        let signature = key.sign(&manifest_signature_message(&self.body_sha256));
        self.authority = ReleaseAuthorityClaimV1 {
            key_id: key_id.into(),
            signature_base64: BASE64.encode(signature.to_bytes()),
        };
        self
    }

    /// The bytes this manifest's signature covers. Exposed so a caller can
    /// prove cross-domain separation without reimplementing the construction.
    pub fn signature_message(&self) -> Vec<u8> {
        manifest_signature_message(&self.body_sha256)
    }
}

fn manifest_signature_message(body_sha256: &str) -> Vec<u8> {
    domain_separated_message(MANIFEST_SIGNATURE_DOMAIN, body_sha256)
}

/// Domain separator followed by the body digest. The shape `receipt.rs` uses.
pub(crate) fn domain_separated_message(domain: &[u8], body_sha256: &str) -> Vec<u8> {
    let mut message = Vec::with_capacity(domain.len() + body_sha256.len());
    message.extend_from_slice(domain);
    message.extend_from_slice(body_sha256.as_bytes());
    message
}

/// Verify a manifest against an independently supplied trust root.
///
/// Refuses on: an unsupported schema, a body digest that does not match, an
/// unknown key id, a key bound to the wrong role, a key outside its validity
/// window, a malformed signature, and a signature that does not verify. Each
/// is a distinct typed error.
pub fn verify_manifest(
    manifest: &ReleaseManifestV1,
    trust_root: &ReleaseTrustRootV1,
    required_role: ReleaseState,
    now: u64,
) -> Result<(), ReleaseIntegrityError> {
    if manifest.schema != RELEASE_MANIFEST_SCHEMA
        || manifest.schema_version != RELEASE_MANIFEST_SCHEMA_VERSION
    {
        return Err(ReleaseIntegrityError::UnsupportedSchema {
            schema: manifest.schema.clone(),
            version: manifest.schema_version,
        });
    }
    validate_manifest_body(&manifest.body)?;
    if hash_serializable(&manifest.body)? != manifest.body_sha256 {
        return Err(ReleaseIntegrityError::DigestMismatch);
    }
    let key = trust_root.resolve(&manifest.authority.key_id, required_role, now)?;
    let signature = decode_signature(&manifest.authority.signature_base64)?;
    key.verify(
        &manifest_signature_message(&manifest.body_sha256),
        &signature,
    )
    .map_err(|_| ReleaseIntegrityError::InvalidSignature)
}

/// Parse and verify in one step. Unknown fields anywhere in the document are
/// refused by `deny_unknown_fields` during this parse, before any logic runs.
pub fn parse_and_verify_manifest(
    manifest_json: &[u8],
    trust_root: &ReleaseTrustRootV1,
    required_role: ReleaseState,
    now: u64,
) -> Result<ReleaseManifestV1, ReleaseIntegrityError> {
    let manifest: ReleaseManifestV1 = serde_json::from_slice(manifest_json)
        .map_err(|error| ReleaseIntegrityError::InvalidJson(error.to_string()))?;
    verify_manifest(&manifest, trust_root, required_role, now)?;
    Ok(manifest)
}

fn validate_manifest_body(body: &ReleaseManifestBodyV1) -> Result<(), ReleaseIntegrityError> {
    require_nonempty("release_id", &body.release_id)?;
    require_hex("source_commit", &body.source_commit, 40)?;
    if body.artifacts.is_empty() {
        return Err(ReleaseIntegrityError::InvalidBody(
            "artifacts must not be empty".to_string(),
        ));
    }
    let mut seen = BTreeSet::new();
    for artifact in &body.artifacts {
        require_nonempty("artifact.name", &artifact.name)?;
        require_hex("artifact.sha256", &artifact.sha256, 64)?;
        if artifact.byte_length == 0 {
            return Err(ReleaseIntegrityError::InvalidBody(format!(
                "artifact {} has zero length",
                artifact.name
            )));
        }
        if !seen.insert(artifact.name.as_str()) {
            return Err(ReleaseIntegrityError::InvalidBody(format!(
                "duplicate artifact name {}",
                artifact.name
            )));
        }
    }
    if let Evidence::Observed { value } = &body.sbom {
        require_nonempty("sbom.name", &value.name)?;
        require_hex("sbom.sha256", &value.sha256, 64)?;
    }
    if let Evidence::Observed { value } = &body.dependency_policy {
        require_nonempty("dependency_policy.tool", &value.tool)?;
        require_hex("dependency_policy.policy_sha256", &value.policy_sha256, 64)?;
    }
    if let ReproducibilityVerdictV1::Variance {
        evidence_sha256, ..
    } = &body.reproducibility
    {
        require_hex("reproducibility.evidence_sha256", evidence_sha256, 64)?;
    }
    if let Evidence::Observed { value } = &body.certification {
        require_hex(
            "certification.receipt_body_sha256",
            &value.receipt_body_sha256,
            64,
        )?;
        require_hex("certification.binary_sha256", &value.binary_sha256, 64)?;
        require_hex("certification.source_commit", &value.source_commit, 40)?;
        require_nonempty(
            "certification.receipt_signing_key_id",
            &value.receipt_signing_key_id,
        )?;
        require_nonempty("certification.receipt_schema", &value.receipt_schema)?;
        require_nonempty("certification.target_os", &value.target_os)?;
        require_nonempty(
            "certification.target_architecture",
            &value.target_architecture,
        )?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

pub(crate) fn hash_serializable(value: &impl Serialize) -> Result<String, ReleaseIntegrityError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| ReleaseIntegrityError::InvalidBody(format!("canonical JSON: {error}")))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

pub(crate) fn decode_public_key(
    public_key_base64: &str,
) -> Result<VerifyingKey, ReleaseIntegrityError> {
    let decoded = BASE64
        .decode(public_key_base64.trim())
        .map_err(|_| ReleaseIntegrityError::InvalidPublicKey)?;
    let bytes: [u8; 32] = decoded
        .try_into()
        .map_err(|_| ReleaseIntegrityError::InvalidPublicKey)?;
    VerifyingKey::from_bytes(&bytes).map_err(|_| ReleaseIntegrityError::InvalidPublicKey)
}

pub(crate) fn decode_signature(signature_base64: &str) -> Result<Signature, ReleaseIntegrityError> {
    let decoded = BASE64
        .decode(signature_base64.trim())
        .map_err(|_| ReleaseIntegrityError::MalformedSignature)?;
    let bytes: [u8; 64] = decoded
        .try_into()
        .map_err(|_| ReleaseIntegrityError::MalformedSignature)?;
    Ok(Signature::from_bytes(&bytes))
}

/// Decode a 32-byte Ed25519 seed supplied as base64 by the caller. The caller
/// owns the buffer and is responsible for wiping it; this function copies the
/// decoded bytes into a `SigningKey` and wipes its own intermediate.
pub fn signing_key_from_seed_base64(
    seed_base64: &[u8],
) -> Result<SigningKey, ReleaseIntegrityError> {
    let trimmed = trim_ascii(seed_base64);
    let mut decoded = BASE64
        .decode(trimmed)
        .map_err(|_| ReleaseIntegrityError::InvalidSigningKey)?;
    let result = <[u8; 32]>::try_from(decoded.as_slice())
        .map(|bytes| SigningKey::from_bytes(&bytes))
        .map_err(|_| ReleaseIntegrityError::InvalidSigningKey);
    wipe(&mut decoded);
    result
}

/// Volatile zeroing wipe. Shared with the `wayland-release` binary so there is
/// one wipe implementation and not two.
pub fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        // SAFETY: `byte` is a valid unique reference for this write. Volatile
        // prevents the compiler from eliding this security-sensitive wipe.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

fn trim_ascii(mut value: &[u8]) -> &[u8] {
    while value.first().is_some_and(u8::is_ascii_whitespace) {
        value = &value[1..];
    }
    while value.last().is_some_and(u8::is_ascii_whitespace) {
        value = &value[..value.len() - 1];
    }
    value
}

pub(crate) fn require_nonempty(field: &str, value: &str) -> Result<(), ReleaseIntegrityError> {
    if value.trim().is_empty() {
        return Err(ReleaseIntegrityError::InvalidBody(format!(
            "{field} must not be empty"
        )));
    }
    Ok(())
}

pub(crate) fn require_hex(
    field: &str,
    value: &str,
    length: usize,
) -> Result<(), ReleaseIntegrityError> {
    if value.len() != length
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ReleaseIntegrityError::InvalidBody(format!(
            "{field} must be {length} lowercase hexadecimal characters"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(ch: char, n: usize) -> String {
        std::iter::repeat_n(ch, n).collect()
    }

    #[test]
    fn ordinals_match_canonical_positions() {
        for (index, state) in CANONICAL_RELEASE_STATES.iter().enumerate() {
            assert_eq!(state.ordinal() as usize, index);
        }
    }

    #[test]
    fn every_state_has_a_distinct_signature_domain() {
        let domains: BTreeSet<&[u8]> = CANONICAL_RELEASE_STATES
            .iter()
            .map(|state| state.signature_domain())
            .collect();
        assert_eq!(domains.len(), 4, "state domains must all differ");
        assert!(
            !domains.contains(&MANIFEST_SIGNATURE_DOMAIN),
            "no state domain may equal the manifest domain"
        );
    }

    #[test]
    fn an_unrecognised_role_name_fails_to_deserialize() {
        let json = r#"{"key_id":"k","public_key_base64":"AA==","role":"notarization","valid_from":0,"retired_at":null}"#;
        assert!(serde_json::from_str::<TrustedKeyV1>(json).is_err());
        // Pristine control: a real role name parses.
        let ok = r#"{"key_id":"k","public_key_base64":"AA==","role":"packaging","valid_from":0,"retired_at":null}"#;
        assert!(serde_json::from_str::<TrustedKeyV1>(ok).is_ok());
    }

    #[test]
    fn body_validation_rejects_an_empty_artifact_list() {
        let body = ReleaseManifestBodyV1 {
            release_id: "r".to_string(),
            source_commit: h('a', 40),
            artifacts: Vec::new(),
            sbom: Evidence::Unavailable {
                code: "absent".to_string(),
            },
            dependency_policy: Evidence::Unavailable {
                code: "absent".to_string(),
            },
            reproducibility: ReproducibilityVerdictV1::Reproduced,
            certification: Evidence::Unavailable {
                code: "absent".to_string(),
            },
        };
        assert!(matches!(
            ReleaseManifestV1::unsigned(body),
            Err(ReleaseIntegrityError::InvalidBody(_))
        ));
    }
}
