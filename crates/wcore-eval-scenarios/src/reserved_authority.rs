//! Phase 30 reserved authority (F30-05) — the actions that are Sean's alone.
//!
//! **What this module is defending against, stated plainly.** An agent on this program
//! invented an extra termination state to dodge an artifact it wrongly believed unobtainable.
//! The Phase 30 shape of that move is to invent a reserved action, a principal, or a verdict
//! that routes around Sean's approval. It is always the same move: add an enum member and the
//! wall is gone.
//!
//! So the defence here is not a rule anybody has to remember at three in the morning. It is a
//! set of CLOSED types that refuse the invention at DESERIALIZATION, before any logic runs:
//!
//! - [`ReservedActionV1`] has exactly nine members, no catch-all, no default and no untagged
//!   fallback. An unrecognised action name does not map to anything — it fails to parse.
//! - [`PrincipalV1`] has exactly ONE member, and it is not the agent. There is no agent
//!   principal, no self-approval principal and no system principal, so an agent recording its
//!   own approval is not a policy violation to detect afterwards; it is a value that cannot be
//!   written down.
//! - Each reserved action carries its OWN signature domain, so an approval minted for a
//!   documentation push is not replayable as an approval for a release, and an approval for a
//!   release is not replayable as an approval for frontier positioning.
//! - Every approval binds the digest of the subject it approves, so an approval cannot be
//!   moved onto a different artifact.
//! - Authority arrives from OUTSIDE the record. Nothing inside an approval establishes its own
//!   authority; the trust root is supplied independently by the caller.
//!
//! **The bundled trust root is an all-zeros placeholder that fails closed**, exactly as
//! `crates/wcore-cli/src/plugin/index.rs` already does with `INDEX_PUBKEY_HEX` and
//! `IndexVerifier::bundled()` (F-021). That proven shape is COPIED rather than improvised on.
//! The consequence is deliberate and is the point of the module: **every reserved action,
//! including frontier positioning, is structurally unreachable from this repository.** Sean's
//! real approval public key replacing [`APPROVAL_ROOT_PUBKEY_HEX`] is the one substitution
//! that changes it, and the refusal error says so.
//!
//! **The mechanism is nonetheless proved to WORK.** [`ApprovalTrustRootV1::generate_throwaway`]
//! mints a root at run time whose approvals verify. Without that positive control the whole
//! module would be passed by a verifier that refuses everything, which would say nothing about
//! whether an approval can ever be honoured. A throwaway root declares itself
//! [`RootKindV1::ThrowawayGeneratedAtRunTime`] and the kind is carried into every accepted
//! result, so a clean-room acceptance can never be quoted as Sean's approval.
//!
//! No secret is read from argv, printed, or logged by anything in this module.

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand::RngCore as _;
use serde::{Deserialize, Serialize};

pub const RESERVED_APPROVAL_SCHEMA: &str = "wayland.reserved.approval";
pub const RESERVED_APPROVAL_SCHEMA_VERSION: u32 = 1;
pub const RESERVED_ROOT_SCHEMA: &str = "wayland.reserved.root";
pub const RESERVED_ROOT_SCHEMA_VERSION: u32 = 1;

/// The bundled approval trust root's public key, as committed.
///
/// **This is the all-zeros placeholder and it authorises nothing.** It is the same shape
/// `INDEX_PUBKEY_HEX` uses in `crates/wcore-cli/src/plugin/index.rs`, and for the same reason:
/// a bundled root that trusts itself is not a trust root. Verification refuses any approval
/// checked against a root declaring an all-zeros key, with an explicit F-030 error naming this
/// constant as the substitution point.
pub const APPROVAL_ROOT_PUBKEY_HEX: &str =
    "0000000000000000000000000000000000000000000000000000000000000000";

/// The key id the bundled placeholder root declares.
pub const APPROVAL_ROOT_KEY_ID: &str = "sean-reserved-approval-root";

/// Errors from reserved-authority verification.
///
/// `thiserror` per AGENTS.md, and every refusal NAMES what caused it — a refusal a reader
/// cannot locate is only marginally better than a silent pass.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ReservedAuthorityError {
    #[error(
        "reserved action `{action}` is unreachable: the approval trust root declares key \
         `{key_id}` as the all-zeros placeholder, which authorises nothing. Substitution \
         point: replace APPROVAL_ROOT_PUBKEY_HEX in \
         crates/wcore-eval-scenarios/src/reserved_authority.rs with Sean's real Ed25519 \
         approval public key. Until that substitution is made, every reserved action \
         including frontier positioning is structurally unreachable from this repository. \
         (F-030)"
    )]
    PlaceholderRoot { action: String, key_id: String },

    #[error("approval names key id `{key_id}`, which this trust root does not declare")]
    UntrustedKey { key_id: String },

    #[error(
        "approval for `{action}` is outside the trust root's validity window \
         [{not_before}, {not_after}] at {now}"
    )]
    OutsideValidityWindow {
        action: String,
        not_before: String,
        not_after: String,
        now: String,
    },

    #[error(
        "signature does not verify for action `{action}` over subject `{subject_sha256}` \
         under key `{key_id}`"
    )]
    InvalidSignature {
        action: String,
        subject_sha256: String,
        key_id: String,
    },

    #[error("malformed key `{key_id}`: {detail}")]
    MalformedKey { key_id: String, detail: String },

    #[error("malformed signature: {detail}")]
    MalformedSignature { detail: String },

    #[error("unexpected schema `{schema}` v{version}; expected `{expected}` v{expected_version}")]
    WrongSchema {
        schema: String,
        version: u32,
        expected: String,
        expected_version: u32,
    },

    #[error("subject digest must be 64 lowercase hex characters, got `{subject_sha256}`")]
    MalformedSubject { subject_sha256: String },
}

// ---------------------------------------------------------------------------
// The closed nine-member reserved-action enum
// ---------------------------------------------------------------------------

/// The nine actions this program reserves to Sean.
///
/// **CLOSED.** No catch-all, no `#[serde(other)]`, no default. An action name nobody declared
/// fails to deserialize rather than mapping to anything, which is the whole defence: adding a
/// tenth member is a visible, reviewable code change rather than a string an agent can write
/// into a document.
///
/// Tokens are spelled out per variant rather than derived by a `rename_all`, so the wire
/// vocabulary is greppable in this file and cannot drift silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ReservedActionV1 {
    #[serde(rename = "source_push")]
    SourcePush,
    #[serde(rename = "main_merge")]
    MainMerge,
    #[serde(rename = "pull_request")]
    PullRequest,
    #[serde(rename = "tag")]
    Tag,
    #[serde(rename = "release")]
    Release,
    #[serde(rename = "deployment")]
    Deployment,
    #[serde(rename = "issue_closure")]
    IssueClosure,
    #[serde(rename = "retained_evidence_ref_deletion")]
    RetainedEvidenceRefDeletion,
    #[serde(rename = "frontier_positioning")]
    FrontierPositioning,
}

/// Exhaustive list of the reserved actions.
///
/// The COMPILER checks the count via the array length, so a variant added without updating
/// this constant fails to build rather than relying on a reviewer counting variants by eye.
pub const ALL_RESERVED_ACTIONS: [ReservedActionV1; 9] = [
    ReservedActionV1::SourcePush,
    ReservedActionV1::MainMerge,
    ReservedActionV1::PullRequest,
    ReservedActionV1::Tag,
    ReservedActionV1::Release,
    ReservedActionV1::Deployment,
    ReservedActionV1::IssueClosure,
    ReservedActionV1::RetainedEvidenceRefDeletion,
    ReservedActionV1::FrontierPositioning,
];

impl ReservedActionV1 {
    /// The wire token for this action.
    pub fn token(self) -> &'static str {
        match self {
            Self::SourcePush => "source_push",
            Self::MainMerge => "main_merge",
            Self::PullRequest => "pull_request",
            Self::Tag => "tag",
            Self::Release => "release",
            Self::Deployment => "deployment",
            Self::IssueClosure => "issue_closure",
            Self::RetainedEvidenceRefDeletion => "retained_evidence_ref_deletion",
            Self::FrontierPositioning => "frontier_positioning",
        }
    }

    /// This action's OWN signature domain. Nine actions, nine domains, none shared.
    ///
    /// This is the one place `crates/wcore-cli/src/plugin/index.rs` is a counter-example
    /// rather than a precedent: it verifies over a serialised body with no domain separator
    /// at all. `receipt.rs` gets it right with a single domain; nine reserved actions need
    /// nine, or approving a documentation push is approving a release.
    pub fn signature_domain(self) -> &'static [u8] {
        match self {
            Self::SourcePush => b"wayland.reserved.source_push.v1\0",
            Self::MainMerge => b"wayland.reserved.main_merge.v1\0",
            Self::PullRequest => b"wayland.reserved.pull_request.v1\0",
            Self::Tag => b"wayland.reserved.tag.v1\0",
            Self::Release => b"wayland.reserved.release.v1\0",
            Self::Deployment => b"wayland.reserved.deployment.v1\0",
            Self::IssueClosure => b"wayland.reserved.issue_closure.v1\0",
            Self::RetainedEvidenceRefDeletion => {
                b"wayland.reserved.retained_evidence_ref_deletion.v1\0"
            }
            Self::FrontierPositioning => b"wayland.reserved.frontier_positioning.v1\0",
        }
    }
}

// ---------------------------------------------------------------------------
// The single-member principal enum
// ---------------------------------------------------------------------------

/// Who an approval is attributed to.
///
/// **Exactly ONE member, and it is not the agent.** The point of a single-member enum is that
/// growing it is a visible, reviewable code change. There is deliberately no agent member, no
/// automation member and no system member, so an agent recording its own approval for a
/// reserved action is a value that cannot be deserialized rather than a breach to detect after
/// it has already happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum PrincipalV1 {
    #[serde(rename = "sean")]
    Sean,
}

/// Exhaustive list of principals. Compiler-checked at one.
pub const ALL_PRINCIPALS: [PrincipalV1; 1] = [PrincipalV1::Sean];

impl PrincipalV1 {
    pub fn token(self) -> &'static str {
        match self {
            Self::Sean => "sean",
        }
    }
}

// ---------------------------------------------------------------------------
// The approval record
// ---------------------------------------------------------------------------

/// A detached signature and the key id that produced it.
///
/// Possession of this value never makes an approval authoritative: a verifier must trust
/// `key_id` out of band, from a root supplied independently of the record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalAuthorityV1 {
    pub key_id: String,
    pub signature_base64: String,
}

/// A signed record that Sean approved one reserved action over one subject.
///
/// `deny_unknown_fields` because a field that was silently ignored reads exactly like a field
/// that was honoured.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalRecordV1 {
    pub schema: String,
    pub schema_version: u32,
    pub action: ReservedActionV1,
    pub principal: PrincipalV1,
    /// The digest of the artifact being approved. Binding it is what stops an approval of one
    /// artifact being moved onto a different one.
    pub subject_sha256: String,
    pub authority: ApprovalAuthorityV1,
}

/// What a verifier learned. Carries the root kind forward deliberately: an acceptance that
/// does not say WHICH root honoured it is exactly how a clean-room proof gets quoted as
/// authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedApprovalV1 {
    pub action: ReservedActionV1,
    pub principal: PrincipalV1,
    pub subject_sha256: String,
    pub key_id: String,
    pub root_kind: RootKindV1,
}

// ---------------------------------------------------------------------------
// The trust root
// ---------------------------------------------------------------------------

/// What kind of root honoured (or refused) an approval.
///
/// CLOSED. The distinction is load-bearing rather than cosmetic: a throwaway root generated
/// inside a test or a proof run must never be confusable with an operator-supplied one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RootKindV1 {
    #[serde(rename = "bundled_placeholder")]
    BundledPlaceholder,
    #[serde(rename = "throwaway_generated_at_run_time")]
    ThrowawayGeneratedAtRunTime,
    #[serde(rename = "operator_supplied")]
    OperatorSupplied,
}

impl RootKindV1 {
    pub fn token(self) -> &'static str {
        match self {
            Self::BundledPlaceholder => "bundled_placeholder",
            Self::ThrowawayGeneratedAtRunTime => "throwaway_generated_at_run_time",
            Self::OperatorSupplied => "operator_supplied",
        }
    }
}

/// The document mapping key ids to public keys, with a validity window.
///
/// Supplied to a verifier INDEPENDENTLY of any approval. Nothing inside an approval
/// establishes its own authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApprovalTrustRootV1 {
    pub schema: String,
    pub schema_version: u32,
    pub root_kind: RootKindV1,
    /// RFC3339 UTC, `YYYY-MM-DDTHH:MM:SSZ`. Fixed-width, so lexicographic comparison is
    /// chronological comparison.
    pub not_before: String,
    pub not_after: String,
    /// key id → 64-character lowercase hex Ed25519 public key.
    pub keys: BTreeMap<String, String>,
}

/// A throwaway root and the seed that mints approvals under it.
///
/// The seed lives only as long as this value and is wiped on drop. It is never printed and
/// never reaches an argv.
pub struct ThrowawayRoot {
    pub root: ApprovalTrustRootV1,
    pub key_id: String,
    seed: [u8; 32],
}

impl ThrowawayRoot {
    /// The signing seed. Deliberately a method rather than a public field so every call site
    /// is greppable.
    pub fn seed(&self) -> &[u8; 32] {
        &self.seed
    }
}

impl Drop for ThrowawayRoot {
    fn drop(&mut self) {
        for byte in self.seed.iter_mut() {
            // SAFETY: `byte` is a valid unique reference for this write. Volatile prevents the
            // compiler eliding a security-sensitive wipe.
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
        std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
    }
}

impl ApprovalTrustRootV1 {
    /// The root as COMMITTED — an all-zeros placeholder that refuses every approval.
    ///
    /// This copies the shape `IndexVerifier::bundled()` already proves in
    /// `crates/wcore-cli/src/plugin/index.rs` (F-021) rather than improvising a new one. It is
    /// what makes every reserved action, frontier positioning included, structurally
    /// unreachable from this repository.
    pub fn bundled() -> Self {
        let mut keys = BTreeMap::new();
        keys.insert(
            APPROVAL_ROOT_KEY_ID.to_string(),
            APPROVAL_ROOT_PUBKEY_HEX.to_string(),
        );
        Self {
            schema: RESERVED_ROOT_SCHEMA.to_string(),
            schema_version: RESERVED_ROOT_SCHEMA_VERSION,
            root_kind: RootKindV1::BundledPlaceholder,
            not_before: "1970-01-01T00:00:00Z".to_string(),
            not_after: "9999-12-31T23:59:59Z".to_string(),
            keys,
        }
    }

    /// Generate a root at run time whose approvals actually verify.
    ///
    /// **This is the positive control and it is not optional.** A module that only ever
    /// refuses would be passed in full by a verifier that refuses unconditionally, proving
    /// nothing about whether an approval can ever be honoured. The generated root declares
    /// itself [`RootKindV1::ThrowawayGeneratedAtRunTime`] and that kind is carried into every
    /// accepted result, so its acceptance can never be read as Sean's approval.
    pub fn generate_throwaway() -> ThrowawayRoot {
        let mut seed = [0u8; 32];
        rand::rngs::OsRng.fill_bytes(&mut seed);
        let signing = SigningKey::from_bytes(&seed);
        let public_hex = hex_encode(signing.verifying_key().as_bytes());
        // The id names what it is, in the id itself, so a stray approval file found on disk
        // announces its own provenance without anyone having to open the root.
        let key_id = format!("throwaway-not-seans-key-{}", &public_hex[..16]);

        let mut keys = BTreeMap::new();
        keys.insert(key_id.clone(), public_hex);

        ThrowawayRoot {
            root: Self {
                schema: RESERVED_ROOT_SCHEMA.to_string(),
                schema_version: RESERVED_ROOT_SCHEMA_VERSION,
                root_kind: RootKindV1::ThrowawayGeneratedAtRunTime,
                not_before: "1970-01-01T00:00:00Z".to_string(),
                not_after: "9999-12-31T23:59:59Z".to_string(),
                keys,
            },
            key_id,
            seed,
        }
    }

    /// Verify an approval against THIS root.
    ///
    /// Refusal order is deliberate: the placeholder check comes first so the bundled root
    /// refuses every action with the same explicit, substitution-point-naming error rather
    /// than with an incidental "unknown key id" that a reader would misdiagnose.
    pub fn verify(
        &self,
        approval: &ApprovalRecordV1,
    ) -> Result<VerifiedApprovalV1, ReservedAuthorityError> {
        if self.schema != RESERVED_ROOT_SCHEMA
            || self.schema_version != RESERVED_ROOT_SCHEMA_VERSION
        {
            return Err(ReservedAuthorityError::WrongSchema {
                schema: self.schema.clone(),
                version: self.schema_version,
                expected: RESERVED_ROOT_SCHEMA.to_string(),
                expected_version: RESERVED_ROOT_SCHEMA_VERSION,
            });
        }
        if approval.schema != RESERVED_APPROVAL_SCHEMA
            || approval.schema_version != RESERVED_APPROVAL_SCHEMA_VERSION
        {
            return Err(ReservedAuthorityError::WrongSchema {
                schema: approval.schema.clone(),
                version: approval.schema_version,
                expected: RESERVED_APPROVAL_SCHEMA.to_string(),
                expected_version: RESERVED_APPROVAL_SCHEMA_VERSION,
            });
        }

        // FAIL CLOSED ON THE PLACEHOLDER, before anything else. A root declaring an all-zeros
        // key trusts nothing, and says which single substitution would change that.
        for (key_id, public_hex) in &self.keys {
            if public_hex.bytes().all(|b| b == b'0') {
                return Err(ReservedAuthorityError::PlaceholderRoot {
                    action: approval.action.token().to_string(),
                    key_id: key_id.clone(),
                });
            }
        }

        let now = now_rfc3339();
        if now < self.not_before || now > self.not_after {
            return Err(ReservedAuthorityError::OutsideValidityWindow {
                action: approval.action.token().to_string(),
                not_before: self.not_before.clone(),
                not_after: self.not_after.clone(),
                now,
            });
        }

        let key_id = &approval.authority.key_id;
        let public_hex =
            self.keys
                .get(key_id)
                .ok_or_else(|| ReservedAuthorityError::UntrustedKey {
                    key_id: key_id.clone(),
                })?;

        if !is_sha256_hex(&approval.subject_sha256) {
            return Err(ReservedAuthorityError::MalformedSubject {
                subject_sha256: approval.subject_sha256.clone(),
            });
        }

        let key_bytes =
            hex_decode_32(public_hex).ok_or_else(|| ReservedAuthorityError::MalformedKey {
                key_id: key_id.clone(),
                detail: "not 64 hex characters".to_string(),
            })?;
        let verifying = VerifyingKey::from_bytes(&key_bytes).map_err(|error| {
            ReservedAuthorityError::MalformedKey {
                key_id: key_id.clone(),
                detail: error.to_string(),
            }
        })?;

        let signature_bytes = BASE64
            .decode(approval.authority.signature_base64.as_bytes())
            .map_err(|error| ReservedAuthorityError::MalformedSignature {
                detail: error.to_string(),
            })?;
        let signature = Signature::from_slice(&signature_bytes).map_err(|error| {
            ReservedAuthorityError::MalformedSignature {
                detail: error.to_string(),
            }
        })?;

        let message = signature_message(
            approval.action,
            approval.principal,
            &approval.subject_sha256,
        );
        verifying.verify(&message, &signature).map_err(|_| {
            ReservedAuthorityError::InvalidSignature {
                action: approval.action.token().to_string(),
                subject_sha256: approval.subject_sha256.clone(),
                key_id: key_id.clone(),
            }
        })?;

        Ok(VerifiedApprovalV1 {
            action: approval.action,
            principal: approval.principal,
            subject_sha256: approval.subject_sha256.clone(),
            key_id: key_id.clone(),
            root_kind: self.root_kind,
        })
    }
}

/// Mint an approval for one action over one subject.
///
/// The seed is borrowed, used, and never copied into a longer-lived value. It is never
/// printed. There is no code path in this crate that accepts a seed on argv.
pub fn mint_approval(
    action: ReservedActionV1,
    subject_sha256: &str,
    key_id: &str,
    seed: &[u8; 32],
) -> Result<ApprovalRecordV1, ReservedAuthorityError> {
    if !is_sha256_hex(subject_sha256) {
        return Err(ReservedAuthorityError::MalformedSubject {
            subject_sha256: subject_sha256.to_string(),
        });
    }
    let signing = SigningKey::from_bytes(seed);
    let message = signature_message(action, PrincipalV1::Sean, subject_sha256);
    let signature = signing.sign(&message);
    Ok(ApprovalRecordV1 {
        schema: RESERVED_APPROVAL_SCHEMA.to_string(),
        schema_version: RESERVED_APPROVAL_SCHEMA_VERSION,
        action,
        principal: PrincipalV1::Sean,
        subject_sha256: subject_sha256.to_string(),
        authority: ApprovalAuthorityV1 {
            key_id: key_id.to_string(),
            signature_base64: BASE64.encode(signature.to_bytes()),
        },
    })
}

/// The bytes actually signed: the action's OWN domain, then the principal, then the subject.
///
/// The domain comes first and is NUL-terminated so no concatenation of a later field can
/// forge a different domain prefix.
fn signature_message(
    action: ReservedActionV1,
    principal: PrincipalV1,
    subject_sha256: &str,
) -> Vec<u8> {
    let domain = action.signature_domain();
    let principal_token = principal.token().as_bytes();
    let mut message =
        Vec::with_capacity(domain.len() + principal_token.len() + 1 + subject_sha256.len());
    message.extend_from_slice(domain);
    message.extend_from_slice(principal_token);
    message.push(0);
    message.extend_from_slice(subject_sha256.as_bytes());
    message
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

fn hex_encode(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

fn hex_decode_32(hex: &str) -> Option<[u8; 32]> {
    if hex.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, slot) in out.iter_mut().enumerate() {
        *slot = u8::from_str_radix(hex.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

/// Current UTC instant as fixed-width RFC3339, so lexicographic comparison is chronological.
///
/// Implemented locally rather than by adding a date dependency: this plan adds none, and
/// `wcore-eval-scenarios` does not already depend on `chrono`. The civil-from-days conversion
/// is Howard Hinnant's, valid for every date this program will ever see.
fn now_rfc3339() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's `civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_token_round_trips_through_serde() {
        for action in ALL_RESERVED_ACTIONS {
            let json = serde_json::to_string(&action).expect("encode");
            assert_eq!(json, format!("\"{}\"", action.token()));
            let back: ReservedActionV1 = serde_json::from_str(&json).expect("decode");
            assert_eq!(back, action);
        }
    }

    #[test]
    fn the_principal_enum_has_exactly_one_member_and_it_is_not_the_agent() {
        assert_eq!(ALL_PRINCIPALS.len(), 1);
        assert_eq!(ALL_PRINCIPALS[0].token(), "sean");
    }

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(11_016), (2000, 3, 1));
    }

    #[test]
    fn now_is_fixed_width_and_sorts_between_the_bundled_bounds() {
        let now = now_rfc3339();
        assert_eq!(
            now.len(),
            20,
            "fixed width is what makes lexicographic comparison sound"
        );
        let root = ApprovalTrustRootV1::bundled();
        assert!(now > root.not_before && now < root.not_after);
    }

    #[test]
    fn hex_round_trips_and_refuses_a_wrong_length() {
        let bytes = [7u8; 32];
        assert_eq!(hex_decode_32(&hex_encode(&bytes)), Some(bytes));
        assert_eq!(hex_decode_32("00"), None);
        assert_eq!(hex_decode_32(&"z".repeat(64)), None);
    }

    #[test]
    fn a_subject_that_is_not_a_sha256_digest_is_refused_at_mint_time() {
        let throwaway = ApprovalTrustRootV1::generate_throwaway();
        let err = mint_approval(
            ReservedActionV1::Release,
            "not-a-digest",
            &throwaway.key_id,
            throwaway.seed(),
        )
        .expect_err("a malformed subject must be refused");
        assert!(matches!(
            err,
            ReservedAuthorityError::MalformedSubject { .. }
        ));
    }
}
