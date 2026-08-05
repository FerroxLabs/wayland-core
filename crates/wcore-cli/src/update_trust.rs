//! The release-lifecycle half of `wayland-core self-update`: an ordered update
//! decision, a bundled release trust root that fails closed on a placeholder,
//! persisted freeze protection, and revocation enforcement.
//!
//! Declared from `self_update.rs` (see its `#[path]` module declaration) so
//! `lib.rs` — a file every concurrent lane shares — needs no edit.
//!
//! ## Why this exists next to the attestation check rather than instead of it
//!
//! `self_update::verify_provenance` runs `gh attestation verify` against the
//! pinned source repo and fails CLOSED. That is a real defence and everything
//! here is ANDed with it, never substituted for it. But a keyless Sigstore
//! attestation states one thing — *this archive was built by that workflow from
//! that repository* — and it goes on stating it forever. It structurally cannot
//! say:
//!
//! - that this version is NEWER than the one running (a genuine older release
//!   carries genuine provenance, so a rollback attack passes attestation
//!   cleanly — finding F29-CEN-11);
//! - that the view you were handed is the CURRENT one rather than a correctly
//!   signed but stale view a hostile mirror has frozen you on;
//! - that the version was later REVOKED;
//! - that the key which vouched for it has since been retired.
//!
//! Those four are lifecycle facts. They need a signed, sequenced,
//! revocation-carrying manifest and a trust root that knows about key
//! lifetimes, which is what this module verifies.
//!
//! ## Three rules this module does not bend
//!
//! 1. **Verify only.** There is no way to mint a release-manifest signature
//!    here. Construction lives in `wcore-eval-scenarios`, a DEV-dependency of
//!    this crate, so it never enters the shipped artifact. A release binary
//!    that could mint a manifest signature would be a key-custody problem
//!    delivered to every user.
//! 2. **No update-source redirect, ever.** No environment variable, no config
//!    key, no flag repoints where updates come from. Anything that could set
//!    one could then serve any binary past every check in this file. This
//!    module performs zero environment reads other than the
//!    `WAYLAND_HOME`-honouring config-root resolver used for its own persisted
//!    state. Testability comes from the decision being a PURE function the
//!    tests drive directly.
//! 3. **Fail closed on a placeholder trust root.** [`ReleaseVerifier::bundled`]
//!    refuses an empty key set and refuses the all-zeros Ed25519 identity
//!    point, whose signatures can be forged with no secret — exactly as
//!    `plugin::index::IndexVerifier::bundled` refuses its own placeholder
//!    (finding F-021). [`RELEASE_TRUST_ROOT_JSON`] shipped EMPTY until
//!    2026-07-29 and the binary therefore installed nothing; it now carries the
//!    real FerroxLabs release-acceptance key, and the refusal is what still
//!    stands between a regressed constant and a forgeable install.

use std::cmp::Ordering;
use std::path::{Path, PathBuf};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// The bundled release trust root.
///
/// **Substituted 2026-07-29** with the real FerroxLabs root, minted by
/// `wayland-release trust-root-init`. It shipped with an empty key set until
/// then, and [`ReleaseVerifier::bundled`] refused to construct from it, so the
/// binary failed closed rather than trusting a placeholder.
///
/// PUBLIC halves only — the seeds were written to owner-only files on the
/// minting machine and never entered this tree. Only the `release_acceptance`
/// key is bundled: [`RELEASE_MANIFEST_ROLE`] is the sole role the updater will
/// accept, so the other three (`packaging`, `deployment_preparation`,
/// `rollback_rehearsal`) could never authorise an install and would add trust
/// surface for no function. They stay in the minting machine's root, which is
/// what makes the four-state ledger's separation meaningful.
///
/// `valid_from: 0` vouches for every release including the first, as required —
/// a later value would refuse the release it was minted for.
pub const RELEASE_TRUST_ROOT_JSON: &str = r#"{"schema":"wayland.release.trust-root","schema_version":1,"keys":[{"key_id":"release-acceptance-key","public_key_base64":"ycwkW1xZnCxruh59zJnQiuoN5xuXYkMurhquhHMBXXY=","role":"release_acceptance","valid_from":0,"retired_at":null}]}"#;

/// A release manifest that authorises an INSTALL must be signed by a key bound
/// to the final release state. Reaching packaging is not reaching acceptance.
pub const RELEASE_MANIFEST_ROLE: &str = "release_acceptance";

/// The manifest signature's domain separator, byte for byte as
/// `wcore_eval_scenarios::release_integrity` mints it. Distinct from the
/// evaluation-receipt domain and from every per-release-state domain, so no
/// signature minted over one object can be replayed onto another.
const MANIFEST_SIGNATURE_DOMAIN: &[u8] = b"wayland.release.manifest.v1\0";

const MANIFEST_SCHEMA: &str = "wayland.release.manifest";
const MANIFEST_SCHEMA_VERSION: u32 = 1;
const TRUST_ROOT_SCHEMA: &str = "wayland.release.trust-root";
const TRUST_ROOT_SCHEMA_VERSION: u32 = 1;

/// How old a release manifest may be before it is treated as a frozen view.
///
/// This is a POLICY number, not a measurement. Ninety days is chosen to be
/// comfortably longer than any plausible release gap while still catching a
/// mirror that has stopped moving. It is the only freeze protection available
/// on a first run, before a high-water mark exists. Revisit it once the
/// project's release cadence is established — recorded as a known unknown in
/// `29-03-UPDATE-TRUST-RESULTS.md`.
pub const DEFAULT_MAX_MANIFEST_AGE_SECS: u64 = 90 * 24 * 60 * 60;

/// Release-asset filename suffix carrying the signed manifest.
pub const RELEASE_MANIFEST_ASSET_SUFFIX: &str = "-release-manifest.json";

const FREEZE_STATE_FILE: &str = "release-freeze-state.json";
const FREEZE_STATE_SCHEMA: &str = "wayland.release.freeze-state";
const FREEZE_STATE_SCHEMA_VERSION: u32 = 1;

/// What a user can always do instead, and it is itself provenance-backed.
const NPM_FALLBACK: &str = "npm install -g @ferroxlabs/wayland-core@latest";

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UpdateTrustError {
    #[error(
        "the bundled release trust root is a placeholder and is refused: {0}. \
         Replace RELEASE_TRUST_ROOT_JSON in crates/wcore-cli/src/update_trust.rs with the \
         real FerroxLabs release trust root before release-manifest verification can work."
    )]
    PlaceholderTrustRoot(String),
    #[error("release trust root is malformed: {0}")]
    MalformedTrustRoot(String),
    #[error("release manifest is not valid JSON: {0}")]
    InvalidJson(String),
    #[error("unsupported release manifest schema {schema} version {version}")]
    UnsupportedSchema { schema: String, version: u32 },
    #[error(
        "release manifest body does not match the digest that was signed — the body was \
         altered after signing"
    )]
    BodyDigestMismatch,
    #[error("release manifest signing key id is not in the trust root: {0}")]
    UnknownKeyId(String),
    #[error("release key {key_id} is bound to role {bound} but role {required} is required")]
    RoleMismatch {
        key_id: String,
        bound: String,
        required: String,
    },
    #[error("release key {0} is not yet valid")]
    KeyNotYetValid(String),
    #[error("release key {0} is retired")]
    RetiredKey(String),
    #[error("a trusted release public key is not a 32-byte base64 Ed25519 key")]
    MalformedPublicKey,
    #[error("release manifest signature is malformed")]
    MalformedSignature,
    #[error("release manifest signature does not verify")]
    InvalidSignature,
    #[error("version string cannot be ordered: {value:?} ({detail})")]
    UnorderableVersion { value: String, detail: String },
    #[error(
        "the signed release manifest does not name the artifact {0} that was downloaded for \
         this host"
    )]
    ArtifactNotInManifest(String),
    #[error(
        "downloaded artifact {name} does not match the signed manifest: expected sha256 \
         {expected} ({expected_bytes} bytes), got {actual} ({actual_bytes} bytes)"
    )]
    ArtifactDigestMismatch {
        name: String,
        expected: String,
        actual: String,
        expected_bytes: u64,
        actual_bytes: u64,
    },
}

// ---------------------------------------------------------------------------
// Ordered versions
// ---------------------------------------------------------------------------

/// A version that can be ORDERED, which is the whole of the rollback defence.
///
/// The updater compared versions for equality only until this landed
/// (F29-CEN-11): anything merely *different* took the install path, including
/// anything lower.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseVersion {
    core: [u64; 3],
    /// Pre-release identifiers, already split on `.`. Empty means a final
    /// release, which orders ABOVE any pre-release of the same core.
    pre: Vec<String>,
}

impl ReleaseVersion {
    pub fn is_prerelease(&self) -> bool {
        !self.pre.is_empty()
    }
}

impl Ord for ReleaseVersion {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.core.cmp(&other.core) {
            Ordering::Equal => {}
            ordered => return ordered,
        }
        // SemVer §11.3: a pre-release orders BELOW its own release.
        match (self.pre.is_empty(), other.pre.is_empty()) {
            (true, true) => return Ordering::Equal,
            (true, false) => return Ordering::Greater,
            (false, true) => return Ordering::Less,
            (false, false) => {}
        }
        // SemVer §11.4, identifier by identifier.
        for (left, right) in self.pre.iter().zip(other.pre.iter()) {
            let ordered = match (left.parse::<u64>(), right.parse::<u64>()) {
                (Ok(l), Ok(r)) => l.cmp(&r),
                // A numeric identifier always has lower precedence than an
                // alphanumeric one.
                (Ok(_), Err(_)) => Ordering::Less,
                (Err(_), Ok(_)) => Ordering::Greater,
                (Err(_), Err(_)) => left.cmp(right),
            };
            if ordered != Ordering::Equal {
                return ordered;
            }
        }
        self.pre.len().cmp(&other.pre.len())
    }
}

impl PartialOrd for ReleaseVersion {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Strip the release-tag decoration the GitHub API hands back so a tag and a
/// `CARGO_PKG_VERSION` are directly comparable.
///
/// One implementation, used both by `Release::version` and by every ordering
/// comparison here, so the two can never disagree about what a tag means.
pub fn normalize_version_tag(tag: &str) -> &str {
    tag.trim_start_matches('v')
        .trim_end_matches("-wayland-base")
}

/// Parse an orderable version, or refuse. A guess here installs something.
pub fn parse_release_version(value: &str) -> Result<ReleaseVersion, UpdateTrustError> {
    let refuse = |detail: &str| UpdateTrustError::UnorderableVersion {
        value: value.to_string(),
        detail: detail.to_string(),
    };

    let normalized = normalize_version_tag(value.trim());
    if normalized.is_empty() {
        return Err(refuse("empty"));
    }
    // Build metadata is ignored for precedence (SemVer §10) but must not be
    // mistaken for a core component.
    let without_build = normalized.split('+').next().unwrap_or(normalized);
    let (core_text, pre_text) = match without_build.split_once('-') {
        Some((core, pre)) => (core, Some(pre)),
        None => (without_build, None),
    };

    let mut parts = core_text.split('.');
    let mut core = [0u64; 3];
    for slot in core.iter_mut() {
        let part = parts.next().ok_or_else(|| {
            refuse("expected three dot-separated numeric components, e.g. 0.13.0")
        })?;
        if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
            return Err(refuse("a core version component is not a decimal number"));
        }
        *slot = part
            .parse::<u64>()
            .map_err(|_| refuse("a core version component does not fit in 64 bits"))?;
    }
    if parts.next().is_some() {
        return Err(refuse("more than three core version components"));
    }

    let pre = match pre_text {
        None => Vec::new(),
        Some(text) => {
            if text.is_empty() {
                return Err(refuse("empty pre-release identifier"));
            }
            let identifiers: Vec<String> = text.split('.').map(str::to_string).collect();
            if identifiers.iter().any(String::is_empty) {
                return Err(refuse("empty pre-release identifier"));
            }
            identifiers
        }
    };

    Ok(ReleaseVersion { core, pre })
}

// ---------------------------------------------------------------------------
// The wire format — an INDEPENDENT mirror, pinned by a cross-implementation
// test rather than by a shared implementation.
// ---------------------------------------------------------------------------
//
// AGENTS.md forbids copy-pasting shared functionality across crates. This is
// deliberately not that: the harness CONSTRUCTS and fully verifies, the shipped
// updater only VERIFIES, and the two meet at the wire format. Two independent
// implementations mean a bug in one does not silently pass both. What makes
// that safe rather than sloppy is `self_update_trust.rs`'s anti-drift guard: a
// manifest minted by the harness must verify here, and one the harness rejects
// must be rejected here too.
//
// The field ORDER below is load-bearing. `body_sha256` is a digest over the
// re-serialized body, not over the bytes as they arrived (the document travels
// pretty-printed), so this mirror must serialize to the same bytes the harness
// digests. `deny_unknown_fields` makes any divergence a hard refusal rather
// than a silently ignored field.

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireManifest {
    pub schema: String,
    pub schema_version: u32,
    pub body_sha256: String,
    pub body: WireBody,
    pub authority: WireAuthority,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireAuthority {
    pub key_id: String,
    pub signature_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireBody {
    pub release_id: String,
    pub source_commit: String,
    pub artifacts: Vec<WireArtifact>,
    pub sbom: WireEvidence<WireSbom>,
    pub dependency_policy: WireEvidence<WirePolicy>,
    pub reproducibility: WireReproducibility,
    pub certification: WireEvidence<WireCertification>,
    pub sequence: u64,
    pub issued_at: u64,
    pub revocations: Vec<WireRevocation>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WireEvidence<T> {
    Observed { value: T },
    Unavailable { code: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireArtifact {
    pub name: String,
    pub sha256: String,
    pub byte_length: u64,
    pub kind: WireArtifactKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireArtifactKind {
    Archive,
    Checksums,
    Sbom,
    Attestation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireSbom {
    pub name: String,
    pub sha256: String,
    pub format: WireSbomFormat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireSbomFormat {
    CycloneDxJson,
    SpdxJson,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WirePolicy {
    pub tool: String,
    pub policy_sha256: String,
    pub result: WirePolicyResult,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WirePolicyResult {
    Pass,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "verdict", rename_all = "snake_case")]
pub enum WireReproducibility {
    Reproduced,
    Variance {
        class: WireVarianceClass,
        evidence_sha256: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireVarianceClass {
    Timestamp,
    PathPrefix,
    BuildId,
    Unclassified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireCertification {
    pub receipt_body_sha256: String,
    pub receipt_schema: String,
    pub receipt_schema_version: u32,
    pub receipt_signing_key_id: String,
    pub source_commit: String,
    pub binary_sha256: String,
    pub target_os: String,
    pub target_architecture: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireRevocation {
    pub kind: WireRevocationKind,
    pub value: String,
    pub reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WireRevocationKind {
    Version,
    ArtifactSha256,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireTrustRoot {
    pub schema: String,
    pub schema_version: u32,
    pub keys: Vec<WireTrustedKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct WireTrustedKey {
    pub key_id: String,
    pub public_key_base64: String,
    /// Kept as a plain string rather than a mirrored enum: an unrecognised
    /// role simply fails to match [`RELEASE_MANIFEST_ROLE`], which is the
    /// refusal we want, and the shipped side gains no reason to track the
    /// harness's state vocabulary.
    pub role: String,
    pub valid_from: u64,
    pub retired_at: Option<u64>,
}

// ---------------------------------------------------------------------------
// The verifier — VERIFY ONLY. There is no construction path in this crate.
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct ReleaseVerifier {
    trust_root: WireTrustRoot,
}

impl ReleaseVerifier {
    /// Construct from the bundled trust root.
    ///
    /// Refuses when that root is still the shipped placeholder — an empty key
    /// set, or any key that is the all-zeros Ed25519 identity point whose
    /// signatures can be forged with no secret. This is the same discipline
    /// `plugin::index::IndexVerifier::bundled` applies for finding F-021, and
    /// it is why the shipped binary fails closed today rather than trusting
    /// anything.
    pub fn bundled() -> Result<Self, UpdateTrustError> {
        Self::with_trust_root_json(RELEASE_TRUST_ROOT_JSON.as_bytes())
    }

    /// Construct from an externally supplied trust root, so the whole path is
    /// exercisable against a key generated at run time. Mirrors the injection
    /// `IndexVerifier::with_pubkey` already provides. The placeholder refusal
    /// applies here too: an injected all-zeros root is no safer than a bundled
    /// one.
    pub fn with_trust_root_json(json: &[u8]) -> Result<Self, UpdateTrustError> {
        let trust_root: WireTrustRoot = serde_json::from_slice(json)
            .map_err(|error| UpdateTrustError::MalformedTrustRoot(error.to_string()))?;
        Self::with_trust_root(trust_root)
    }

    pub fn with_trust_root(trust_root: WireTrustRoot) -> Result<Self, UpdateTrustError> {
        if trust_root.schema != TRUST_ROOT_SCHEMA
            || trust_root.schema_version != TRUST_ROOT_SCHEMA_VERSION
        {
            return Err(UpdateTrustError::MalformedTrustRoot(format!(
                "unsupported trust root {} version {}",
                trust_root.schema, trust_root.schema_version
            )));
        }
        if trust_root.keys.is_empty() {
            return Err(UpdateTrustError::PlaceholderTrustRoot(
                "it holds no keys".to_string(),
            ));
        }
        for key in &trust_root.keys {
            let bytes = decode_public_key_bytes(&key.public_key_base64)?;
            if bytes.iter().all(|byte| *byte == 0) {
                return Err(UpdateTrustError::PlaceholderTrustRoot(format!(
                    "key {} is the all-zeros Ed25519 identity point, whose signatures can be \
                     forged without any secret",
                    key.key_id
                )));
            }
        }
        Ok(Self { trust_root })
    }

    /// Parse and verify a signed release manifest.
    ///
    /// Refuses, each with a distinct typed cause: an unsupported schema, a body
    /// that no longer matches the digest that was signed, an unknown key id, a
    /// key bound to the wrong role, a key outside its validity window, a
    /// malformed signature, and a signature that does not verify — including
    /// one minted over the same body under a different domain.
    pub fn verify_manifest_json(
        &self,
        json: &[u8],
        now_unix: u64,
    ) -> Result<VerifiedManifest, UpdateTrustError> {
        let manifest: WireManifest = serde_json::from_slice(json)
            .map_err(|error| UpdateTrustError::InvalidJson(error.to_string()))?;

        if manifest.schema != MANIFEST_SCHEMA || manifest.schema_version != MANIFEST_SCHEMA_VERSION
        {
            return Err(UpdateTrustError::UnsupportedSchema {
                schema: manifest.schema,
                version: manifest.schema_version,
            });
        }

        // Bind the body to the signature BEFORE reading a single field out of
        // it. Without this, sequence, age and revocations would be read from an
        // unauthenticated document that merely travelled next to a valid
        // signature.
        let recomputed = digest_of(&manifest.body)?;
        if recomputed != manifest.body_sha256 {
            return Err(UpdateTrustError::BodyDigestMismatch);
        }

        let key = self.resolve(&manifest.authority.key_id, now_unix)?;
        let signature = decode_signature(&manifest.authority.signature_base64)?;
        let mut message = MANIFEST_SIGNATURE_DOMAIN.to_vec();
        message.extend_from_slice(manifest.body_sha256.as_bytes());
        key.verify(&message, &signature)
            .map_err(|_| UpdateTrustError::InvalidSignature)?;

        Ok(VerifiedManifest {
            body_sha256: manifest.body_sha256,
            key_id: manifest.authority.key_id,
            body: manifest.body,
        })
    }

    fn resolve(&self, key_id: &str, now_unix: u64) -> Result<VerifyingKey, UpdateTrustError> {
        let entry = self
            .trust_root
            .keys
            .iter()
            .find(|candidate| candidate.key_id == key_id)
            .ok_or_else(|| UpdateTrustError::UnknownKeyId(key_id.to_string()))?;
        if entry.role != RELEASE_MANIFEST_ROLE {
            return Err(UpdateTrustError::RoleMismatch {
                key_id: key_id.to_string(),
                bound: entry.role.clone(),
                required: RELEASE_MANIFEST_ROLE.to_string(),
            });
        }
        if now_unix < entry.valid_from {
            return Err(UpdateTrustError::KeyNotYetValid(key_id.to_string()));
        }
        if entry.retired_at.is_some_and(|retired| now_unix >= retired) {
            return Err(UpdateTrustError::RetiredKey(key_id.to_string()));
        }
        let bytes = decode_public_key_bytes(&entry.public_key_base64)?;
        VerifyingKey::from_bytes(&bytes).map_err(|_| UpdateTrustError::MalformedPublicKey)
    }
}

/// A manifest whose signature has been checked against the trust root and
/// whose body has been bound to that signature. Constructible only through
/// [`ReleaseVerifier::verify_manifest_json`], so possession of one IS the
/// proof — no caller can fabricate an "already verified" manifest.
#[derive(Debug, Clone)]
pub struct VerifiedManifest {
    body_sha256: String,
    key_id: String,
    body: WireBody,
}

impl VerifiedManifest {
    pub fn body_sha256(&self) -> &str {
        &self.body_sha256
    }

    pub fn signing_key_id(&self) -> &str {
        &self.key_id
    }

    pub fn release_id(&self) -> &str {
        &self.body.release_id
    }

    /// The release this manifest describes, normalized the same way a GitHub
    /// release tag is.
    pub fn version_string(&self) -> &str {
        normalize_version_tag(&self.body.release_id)
    }

    pub fn sequence(&self) -> u64 {
        self.body.sequence
    }

    pub fn issued_at(&self) -> u64 {
        self.body.issued_at
    }

    pub fn artifacts(&self) -> &[WireArtifact] {
        &self.body.artifacts
    }

    pub fn revocations(&self) -> &[WireRevocation] {
        &self.body.revocations
    }

    /// The revocation covering `version`, if any. Compares normalized forms so
    /// a revocation written as a tag catches a bare version and vice versa.
    pub fn revocation_for_version(&self, version: &str) -> Option<&WireRevocation> {
        let wanted = normalize_version_tag(version.trim());
        self.body.revocations.iter().find(|revocation| {
            matches!(revocation.kind, WireRevocationKind::Version)
                && normalize_version_tag(revocation.value.trim()) == wanted
        })
    }

    /// Bind bytes actually downloaded to the artifact the signed manifest
    /// names. Without this the manifest's artifact digests are decorative: a
    /// correctly signed manifest would sit next to whatever archive the source
    /// chose to hand over.
    ///
    /// ANDed with — never a substitute for — the keyless attestation check,
    /// which independently establishes that the archive was built by the
    /// pinned repository's release workflow.
    pub fn check_archive(
        &self,
        archive_name: &str,
        actual_sha256: &str,
        actual_bytes: u64,
    ) -> Result<(), UpdateTrustError> {
        let artifact = self
            .body
            .artifacts
            .iter()
            .find(|artifact| artifact.name == archive_name)
            .ok_or_else(|| UpdateTrustError::ArtifactNotInManifest(archive_name.to_string()))?;
        if !artifact.sha256.eq_ignore_ascii_case(actual_sha256)
            || artifact.byte_length != actual_bytes
        {
            return Err(UpdateTrustError::ArtifactDigestMismatch {
                name: archive_name.to_string(),
                expected: artifact.sha256.clone(),
                actual: actual_sha256.to_string(),
                expected_bytes: artifact.byte_length,
                actual_bytes,
            });
        }
        Ok(())
    }

    /// The first artifact in this manifest whose digest has been revoked.
    pub fn revoked_artifact(&self) -> Option<(&WireArtifact, &WireRevocation)> {
        for revocation in &self.body.revocations {
            if !matches!(revocation.kind, WireRevocationKind::ArtifactSha256) {
                continue;
            }
            if let Some(artifact) = self.body.artifacts.iter().find(|artifact| {
                artifact
                    .sha256
                    .eq_ignore_ascii_case(revocation.value.trim())
            }) {
                return Some((artifact, revocation));
            }
        }
        None
    }
}

fn digest_of(body: &WireBody) -> Result<String, UpdateTrustError> {
    let bytes = serde_json::to_vec(body)
        .map_err(|error| UpdateTrustError::InvalidJson(error.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn decode_public_key_bytes(encoded: &str) -> Result<[u8; 32], UpdateTrustError> {
    let decoded = BASE64
        .decode(encoded.trim())
        .map_err(|_| UpdateTrustError::MalformedPublicKey)?;
    <[u8; 32]>::try_from(decoded.as_slice()).map_err(|_| UpdateTrustError::MalformedPublicKey)
}

fn decode_signature(encoded: &str) -> Result<Signature, UpdateTrustError> {
    let decoded = BASE64
        .decode(encoded.trim())
        .map_err(|_| UpdateTrustError::MalformedSignature)?;
    let bytes = <[u8; 64]>::try_from(decoded.as_slice())
        .map_err(|_| UpdateTrustError::MalformedSignature)?;
    Ok(Signature::from_bytes(&bytes))
}

// ---------------------------------------------------------------------------
// Persisted freeze state
// ---------------------------------------------------------------------------

/// The highest manifest sequence this installation has ever accepted.
///
/// This is the memory that makes a freeze detectable: without it, a mirror
/// serving a correctly signed but old view looks indistinguishable from a
/// project that has not shipped lately.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FreezeState {
    pub schema: String,
    pub schema_version: u32,
    pub highest_sequence: u64,
    pub last_seen_at: u64,
}

impl FreezeState {
    /// A machine that has never installed anything. Not an error state: a
    /// first run is normal, and it still enforces the maximum-age rule,
    /// because that is the only freeze protection available before a
    /// high-water mark exists.
    pub fn first_run() -> Self {
        Self {
            schema: FREEZE_STATE_SCHEMA.to_string(),
            schema_version: FREEZE_STATE_SCHEMA_VERSION,
            highest_sequence: 0,
            last_seen_at: 0,
        }
    }

    pub fn is_first_run(&self) -> bool {
        self.highest_sequence == 0
    }

    /// Resolved through `wcore_config`'s `WAYLAND_HOME`-honouring resolver, so
    /// a sandboxed or test run can never touch the developer's real
    /// installation and never reads a hand-rolled home path.
    pub fn default_path() -> PathBuf {
        wcore_config::config::wayland_config_dir().join(FREEZE_STATE_FILE)
    }

    pub fn load() -> Self {
        Self::load_from(&Self::default_path())
    }

    /// A missing, unreadable or malformed state file is a FIRST RUN, not an
    /// error — refusing to update because a cache file got corrupted would be
    /// a denial of service, and the age rule still applies.
    pub fn load_from(path: &Path) -> Self {
        let Ok(bytes) = std::fs::read(path) else {
            return Self::first_run();
        };
        match serde_json::from_slice::<Self>(&bytes) {
            Ok(state)
                if state.schema == FREEZE_STATE_SCHEMA
                    && state.schema_version == FREEZE_STATE_SCHEMA_VERSION =>
            {
                state
            }
            _ => Self::first_run(),
        }
    }

    pub fn record_install(sequence: u64, now_unix: u64) -> std::io::Result<Self> {
        Self::record_install_at(&Self::default_path(), sequence, now_unix)
    }

    /// Advance the high-water mark after a SUCCESSFUL install. Never called
    /// from the decision, which is pure. The mark only ever rises: a lower
    /// sequence cannot lower it, so a rolled-back view cannot reset the memory
    /// that would have caught it.
    pub fn record_install_at(path: &Path, sequence: u64, now_unix: u64) -> std::io::Result<Self> {
        let mut state = Self::load_from(path);
        state.highest_sequence = state.highest_sequence.max(sequence);
        state.last_seen_at = now_unix;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let encoded = serde_json::to_vec_pretty(&state)
            .map_err(|error| std::io::Error::other(error.to_string()))?;
        std::fs::write(path, encoded)?;
        Ok(state)
    }
}

// ---------------------------------------------------------------------------
// The update decision — a PURE function
// ---------------------------------------------------------------------------

/// Everything the choice depends on. No network, no filesystem, no clock
/// beyond the injected instant, and no environment: that is what makes the
/// decision testable by EXTRACTION rather than by an update-source redirect,
/// which would itself be a supply-chain attack surface.
pub struct UpdateOffer<'a> {
    pub running_version: &'a str,
    pub offered_version: &'a str,
    pub manifest: Option<&'a VerifiedManifest>,
    pub state: &'a FreezeState,
    pub now_unix: u64,
    pub max_manifest_age_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateDecision {
    Proceed {
        to_version: String,
        sequence: u64,
    },
    AlreadyUpToDate {
        version: String,
    },
    RefusedDowngrade {
        running: String,
        offered: String,
    },
    RefusedUnorderableVersion {
        value: String,
        detail: String,
    },
    RefusedMissingManifest {
        offered: String,
    },
    RefusedManifestDoesNotDescribeOffer {
        manifest_version: String,
        offered: String,
    },
    RefusedStaleSequence {
        offered_sequence: u64,
        high_water_mark: u64,
    },
    RefusedOverAgeManifest {
        issued_at: u64,
        age_secs: u64,
        max_age_secs: u64,
    },
    RefusedRevokedVersion {
        version: String,
        reason: String,
    },
    RefusedRevokedArtifact {
        artifact: String,
        sha256: String,
        reason: String,
    },
}

impl UpdateDecision {
    pub fn proceeds(&self) -> bool {
        matches!(self, Self::Proceed { .. })
    }

    /// The user-visible sentence. Every refusal names its CAUSE — and, for a
    /// downgrade, its DIRECTION — rather than a generic error, because a user
    /// who cannot tell why an update was refused cannot act on it.
    pub fn message(&self) -> String {
        match self {
            Self::Proceed { to_version, .. } => format!("installing v{to_version}"),
            Self::AlreadyUpToDate { version } => {
                format!("already up to date (v{version}).")
            }
            Self::RefusedDowngrade { running, offered } => format!(
                "REFUSED: the offered release v{offered} is OLDER than the running v{running}. \
                 An update must move forward; installing a downgrade would reintroduce every \
                 defect fixed since v{offered}. Nothing was installed."
            ),
            Self::RefusedUnorderableVersion { value, detail } => format!(
                "REFUSED: the version {value:?} cannot be ordered ({detail}), so whether it is \
                 newer or older is unknowable. Guessing here installs something. Nothing was \
                 installed."
            ),
            Self::RefusedMissingManifest { offered } => format!(
                "REFUSED: release v{offered} carries no verifiable signed release manifest, so \
                 rollback, freeze and revocation protection cannot be applied. This is a \
                 deliberate fail-closed posture. Update instead with: {NPM_FALLBACK}"
            ),
            Self::RefusedManifestDoesNotDescribeOffer {
                manifest_version,
                offered,
            } => format!(
                "REFUSED: the signed manifest describes release v{manifest_version} but the \
                 offer is v{offered}. A manifest for one release must not authorise another."
            ),
            Self::RefusedStaleSequence {
                offered_sequence,
                high_water_mark,
            } => format!(
                "REFUSED: this release view is stale — its manifest sequence {offered_sequence} \
                 is not newer than sequence {high_water_mark}, which this installation has \
                 already accepted. An update source that keeps serving an old but correctly \
                 signed view is freezing you on it."
            ),
            Self::RefusedOverAgeManifest {
                issued_at,
                age_secs,
                max_age_secs,
            } => format!(
                "REFUSED: the signed release manifest was issued at unix {issued_at}, \
                 {age_secs}s ago, which exceeds the maximum accepted age of {max_age_secs}s. \
                 An update source stuck this far in the past is not a current view."
            ),
            Self::RefusedRevokedVersion { version, reason } => format!(
                "REFUSED: release v{version} has been REVOKED: {reason}. Nothing was installed."
            ),
            Self::RefusedRevokedArtifact {
                artifact,
                sha256,
                reason,
            } => format!(
                "REFUSED: artifact {artifact} ({sha256}) has been REVOKED: {reason}. Nothing \
                 was installed."
            ),
        }
    }
}

/// Decide whether to install. Pure.
///
/// Order matters and is deliberate. The version comparison runs FIRST, so an
/// equal offer stays the clean no-op it has always been and a downgrade is
/// refused without any dependence on manifest availability — the rollback
/// defence must not be contingent on the freeze machinery working.
pub fn decide_update(offer: &UpdateOffer<'_>) -> UpdateDecision {
    let running = match parse_release_version(offer.running_version) {
        Ok(version) => version,
        Err(UpdateTrustError::UnorderableVersion { value, detail }) => {
            return UpdateDecision::RefusedUnorderableVersion { value, detail };
        }
        Err(other) => {
            return UpdateDecision::RefusedUnorderableVersion {
                value: offer.running_version.to_string(),
                detail: other.to_string(),
            };
        }
    };
    let offered = match parse_release_version(offer.offered_version) {
        Ok(version) => version,
        Err(UpdateTrustError::UnorderableVersion { value, detail }) => {
            return UpdateDecision::RefusedUnorderableVersion { value, detail };
        }
        Err(other) => {
            return UpdateDecision::RefusedUnorderableVersion {
                value: offer.offered_version.to_string(),
                detail: other.to_string(),
            };
        }
    };

    let normalized_offered = normalize_version_tag(offer.offered_version.trim()).to_string();
    match offered.cmp(&running) {
        Ordering::Less => {
            return UpdateDecision::RefusedDowngrade {
                running: normalize_version_tag(offer.running_version.trim()).to_string(),
                offered: normalized_offered,
            };
        }
        Ordering::Equal => {
            return UpdateDecision::AlreadyUpToDate {
                version: normalized_offered,
            };
        }
        Ordering::Greater => {}
    }

    // Forward moves must be authorised by a signed manifest. Fail closed.
    let Some(manifest) = offer.manifest else {
        return UpdateDecision::RefusedMissingManifest {
            offered: normalized_offered,
        };
    };

    if manifest.version_string() != normalized_offered {
        return UpdateDecision::RefusedManifestDoesNotDescribeOffer {
            manifest_version: manifest.version_string().to_string(),
            offered: normalized_offered,
        };
    }

    // Age first: it is the only freeze protection that works on a first run.
    let age = offer.now_unix.saturating_sub(manifest.issued_at());
    if age > offer.max_manifest_age_secs {
        return UpdateDecision::RefusedOverAgeManifest {
            issued_at: manifest.issued_at(),
            age_secs: age,
            max_age_secs: offer.max_manifest_age_secs,
        };
    }

    if manifest.sequence() <= offer.state.highest_sequence {
        return UpdateDecision::RefusedStaleSequence {
            offered_sequence: manifest.sequence(),
            high_water_mark: offer.state.highest_sequence,
        };
    }

    if let Some(revocation) = manifest.revocation_for_version(&normalized_offered) {
        return UpdateDecision::RefusedRevokedVersion {
            version: normalized_offered,
            reason: revocation.reason.clone(),
        };
    }
    if let Some((artifact, revocation)) = manifest.revoked_artifact() {
        return UpdateDecision::RefusedRevokedArtifact {
            artifact: artifact.name.clone(),
            sha256: artifact.sha256.clone(),
            reason: revocation.reason.clone(),
        };
    }

    UpdateDecision::Proceed {
        to_version: normalized_offered,
        sequence: manifest.sequence(),
    }
}

/// Lines a check-only run prints in addition to the decision.
///
/// A revocation the user never learns about protects nobody, so a revoked
/// RUNNING version is reported prominently even though a check-only run
/// installs nothing.
pub fn check_only_report(
    manifest: Option<&VerifiedManifest>,
    running_version: &str,
) -> Vec<String> {
    let mut lines = Vec::new();
    let Some(manifest) = manifest else {
        return lines;
    };
    if let Some(revocation) = manifest.revocation_for_version(running_version) {
        let running = normalize_version_tag(running_version.trim());
        lines.push(format!(
            "!! SECURITY: the running version v{running} has been REVOKED: {}",
            revocation.reason
        ));
        lines.push(format!(
            "!! Move to a newer release as soon as one is available, or reinstall with: \
             {NPM_FALLBACK}"
        ));
    }
    lines
}

// ---------------------------------------------------------------------------
// Unit tests. The hostile corpus lives in tests/self_update_trust.rs; these
// cover the internals that corpus reaches only indirectly.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled constant held the empty placeholder until 2026-07-29 and
    /// this test asserted `bundled()` REFUSED it. The real root was
    /// substituted, so the assertion is re-aimed rather than deleted: the
    /// refusal behaviour is proved against an INJECTED placeholder (which is
    /// what `bundled()` would become again if the constant regressed), and the
    /// bundled constant is separately proved to be real and to construct.
    #[test]
    fn a_placeholder_root_is_refused_and_the_bundled_root_is_not_one() {
        let empty = r#"{"schema":"wayland.release.trust-root","schema_version":1,"keys":[]}"#;
        assert!(matches!(
            ReleaseVerifier::with_trust_root_json(empty.as_bytes()),
            Err(UpdateTrustError::PlaceholderTrustRoot(_))
        ));

        let zeros = format!(
            r#"{{"schema":"wayland.release.trust-root","schema_version":1,"keys":[{{"key_id":"k","public_key_base64":"{}","role":"release_acceptance","valid_from":0,"retired_at":null}}]}}"#,
            BASE64.encode([0u8; 32])
        );
        assert!(matches!(
            ReleaseVerifier::with_trust_root_json(zeros.as_bytes()),
            Err(UpdateTrustError::PlaceholderTrustRoot(_))
        ));

        // The control. Without it the two refusals above pass against a
        // constructor that refuses everything, which would brick every install.
        assert!(
            !RELEASE_TRUST_ROOT_JSON.contains("\"keys\":[]"),
            "the bundled root regressed to the empty placeholder"
        );
        ReleaseVerifier::bundled().expect("the bundled trust root must construct");
    }

    /// Only the role the updater will act on belongs in the bundled root.
    /// `resolve` refuses every other role, so a `packaging`,
    /// `deployment_preparation` or `rollback_rehearsal` key bundled here could
    /// never authorise an install — it would be trust surface with no function,
    /// and it would weaken the four-state ledger's separation.
    #[test]
    fn the_bundled_root_carries_only_the_role_the_updater_accepts() {
        let root: WireTrustRoot =
            serde_json::from_slice(RELEASE_TRUST_ROOT_JSON.as_bytes()).expect("must parse");
        assert_eq!(root.keys.len(), 1, "exactly one key belongs here");
        assert_eq!(root.keys[0].role, RELEASE_MANIFEST_ROLE);
        assert_eq!(
            root.keys[0].valid_from, 0,
            "must vouch for the first release it signs"
        );
        assert!(
            root.keys[0].retired_at.is_none(),
            "a retired key installs nothing"
        );
        assert_eq!(
            decode_public_key_bytes(&root.keys[0].public_key_base64)
                .expect("must decode")
                .len(),
            32
        );

        // The refusal this bundling relies on, proved live rather than assumed:
        // a non-acceptance role does not resolve.
        let verifier = ReleaseVerifier::with_trust_root(WireTrustRoot {
            schema: TRUST_ROOT_SCHEMA.to_string(),
            schema_version: TRUST_ROOT_SCHEMA_VERSION,
            keys: vec![WireTrustedKey {
                role: "packaging".to_string(),
                ..root.keys[0].clone()
            }],
        })
        .expect("a well-formed root constructs regardless of role");
        assert!(matches!(
            verifier.resolve(&root.keys[0].key_id, 1_800_000_000),
            Err(UpdateTrustError::RoleMismatch { .. })
        ));
        // Control: the same key under its real role DOES resolve, so the
        // refusal above is about the role and not about the key.
        assert!(
            ReleaseVerifier::bundled()
                .expect("bundled must construct")
                .resolve(&root.keys[0].key_id, 1_800_000_000)
                .is_ok()
        );
    }

    #[test]
    fn version_ordering_matches_semver_precedence() {
        let v = |s: &str| parse_release_version(s).unwrap();
        assert!(v("0.9.0") < v("0.10.0"), "numeric, not lexicographic");
        assert!(v("0.12.25") < v("0.13.0"));
        assert!(v("1.0.0") > v("0.999.999"));
        assert!(v("1.2.3-rc.1") < v("1.2.3"));
        assert!(v("1.2.3-rc.1") < v("1.2.3-rc.2"));
        assert!(v("1.2.3-rc.2") < v("1.2.3-rc.10"), "numeric identifiers");
        assert!(v("1.2.3-alpha") < v("1.2.3-beta"));
        assert!(v("1.2.3-1") < v("1.2.3-alpha"), "numeric ranks below alnum");
        assert!(v("1.2.3-rc") < v("1.2.3-rc.1"), "more fields ranks higher");
        assert_eq!(v("1.2.3+a"), v("1.2.3+b"), "build metadata is ignored");
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert_eq!(v("v0.12.25-wayland-base"), v("0.12.25"));
    }

    #[test]
    fn normalize_agrees_with_the_release_tag_rule() {
        assert_eq!(normalize_version_tag("v0.8.1"), "0.8.1");
        assert_eq!(normalize_version_tag("v0.7.0-wayland-base"), "0.7.0");
        assert_eq!(normalize_version_tag("1.2.3"), "1.2.3");
    }

    #[test]
    fn a_trust_root_with_a_bad_schema_is_refused_before_any_key_is_read() {
        let json = r#"{"schema":"something.else","schema_version":1,"keys":[]}"#;
        assert!(matches!(
            ReleaseVerifier::with_trust_root_json(json.as_bytes()),
            Err(UpdateTrustError::MalformedTrustRoot(_))
        ));
    }

    #[test]
    fn a_freeze_state_with_a_foreign_schema_reads_as_a_first_run() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.json");
        std::fs::write(
            &path,
            br#"{"schema":"someone.elses.state","schema_version":1,"highest_sequence":99,"last_seen_at":1}"#,
        )
        .unwrap();
        let state = FreezeState::load_from(&path);
        assert!(
            state.is_first_run(),
            "a foreign document must not be believed as a high-water mark"
        );
    }
}
