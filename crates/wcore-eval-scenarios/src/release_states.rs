//! The closed four-state release ledger.
//!
//! An agent on this program, having hit a wall, invented an extra termination
//! state to route around an artifact it wrongly believed unobtainable. Prose is
//! exactly what that agent walked around, so the separation of the four release
//! states is expressed here as a data structure rather than a convention:
//!
//! - the state name is a closed enum ([`crate::release_integrity::ReleaseState`])
//!   that refuses an unknown value at DESERIALIZATION, before any logic runs;
//! - a chain longer than four records is refused by count;
//! - each record must sit at its canonical ordinal, in canonical order;
//! - each record is signed under a key whose trust-root role EQUALS that
//!   record's state, and the states' signature domains differ, so a signature
//!   minted for one state cannot be replayed into another;
//! - the four records must be signed by four DISTINCT key ids;
//! - each record binds the manifest digest and the previous record's digest, so
//!   a record cannot be lifted from one chain into another;
//! - each state's evidence set is non-empty and disjoint by digest from every
//!   earlier state's, so relabelling packaging evidence as acceptance evidence
//!   is refused;
//! - and a MISSING record is not an error. Verification stops and reports the
//!   highest contiguously reached state, because that is the honest answer when
//!   a state's key is legitimately unavailable.
//!
//! That last rule is what makes the withheld-acceptance-key case report
//! `RollbackRehearsal` rather than failing ambiguously — and it is why release
//! acceptance is structurally unreachable without the key that Sean holds.

use std::collections::{BTreeSet, HashSet};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use ed25519_dalek::{Signer, SigningKey, Verifier};
use serde::{Deserialize, Serialize};

use crate::receipt::Evidence;
use crate::release_integrity::{
    CANONICAL_RELEASE_STATES, ReleaseAuthorityClaimV1, ReleaseIntegrityError, ReleaseManifestV1,
    ReleaseState, ReleaseTrustRootV1, decode_signature, domain_separated_message,
    hash_serializable, require_hex, require_nonempty,
};

pub const RELEASE_STATE_RECORD_SCHEMA: &str = "wayland.release.state-record";
pub const RELEASE_STATE_RECORD_SCHEMA_VERSION: u32 = 1;

/// One state's signed record. Same shape as the manifest — a body, a digest
/// over that body, and an authority claim — so there is one idiom in this
/// crate and not two.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseStateRecordV1 {
    pub schema: String,
    pub schema_version: u32,
    pub body_sha256: String,
    pub body: ReleaseStateBodyV1,
    pub authority: ReleaseAuthorityClaimV1,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseStateBodyV1 {
    pub state: ReleaseState,
    pub ordinal: u8,
    pub manifest_sha256: String,
    /// `None` only for the first record in a chain.
    pub previous_record_sha256: Option<String>,
    /// Non-empty, and disjoint by digest from every earlier state's.
    pub evidence: Vec<StateEvidenceV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct StateEvidenceV1 {
    pub name: String,
    pub sha256: String,
}

impl ReleaseStateRecordV1 {
    /// Build an unsigned record for `state`. The ordinal is derived from the
    /// state rather than supplied, so the two can only disagree if a record is
    /// hand-edited after the fact — which verification then refuses.
    pub fn unsigned(
        state: ReleaseState,
        manifest_sha256: impl Into<String>,
        previous_record_sha256: Option<String>,
        evidence: Vec<StateEvidenceV1>,
    ) -> Result<Self, ReleaseIntegrityError> {
        let body = ReleaseStateBodyV1 {
            state,
            ordinal: state.ordinal(),
            manifest_sha256: manifest_sha256.into(),
            previous_record_sha256,
            evidence,
        };
        validate_state_body(&body)?;
        Ok(Self {
            schema: RELEASE_STATE_RECORD_SCHEMA.to_string(),
            schema_version: RELEASE_STATE_RECORD_SCHEMA_VERSION,
            body_sha256: hash_serializable(&body)?,
            body,
            authority: ReleaseAuthorityClaimV1 {
                key_id: String::new(),
                signature_base64: String::new(),
            },
        })
    }

    /// Sign under this record's OWN state domain.
    pub fn sign(mut self, key_id: impl Into<String>, key: &SigningKey) -> Self {
        let signature = key.sign(&self.signature_message());
        self.authority = ReleaseAuthorityClaimV1 {
            key_id: key_id.into(),
            signature_base64: BASE64.encode(signature.to_bytes()),
        };
        self
    }

    /// The bytes this record's signature covers: the state's own domain
    /// separator followed by the body digest.
    pub fn signature_message(&self) -> Vec<u8> {
        domain_separated_message(self.body.state.signature_domain(), &self.body_sha256)
    }
}

fn validate_state_body(body: &ReleaseStateBodyV1) -> Result<(), ReleaseIntegrityError> {
    require_hex("manifest_sha256", &body.manifest_sha256, 64)?;
    if let Some(previous) = &body.previous_record_sha256 {
        require_hex("previous_record_sha256", previous, 64)?;
    }
    if body.evidence.is_empty() {
        return Err(ReleaseIntegrityError::EmptyEvidence { state: body.state });
    }
    let mut seen = BTreeSet::new();
    for entry in &body.evidence {
        require_nonempty("evidence.name", &entry.name)?;
        require_hex("evidence.sha256", &entry.sha256, 64)?;
        if !seen.insert(entry.sha256.as_str()) {
            return Err(ReleaseIntegrityError::InvalidBody(format!(
                "duplicate evidence digest {} within one record",
                entry.sha256
            )));
        }
    }
    Ok(())
}

/// The outcome of verifying a chain: how far the release contiguously got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChainProgress {
    /// `None` when the chain is empty — no state has been reached.
    pub highest_state: Option<ReleaseState>,
    pub records_verified: usize,
}

impl ChainProgress {
    /// True only when all four states verified. This is the single predicate a
    /// release gate should consult, and it cannot be satisfied without a
    /// signature from the release-acceptance key.
    pub fn is_accepted(&self) -> bool {
        self.highest_state == Some(ReleaseState::ReleaseAcceptance)
    }
}

/// Verify a chain of state records against the manifest they describe and an
/// independently supplied trust root.
///
/// Returns the highest CONTIGUOUSLY verified state. A short chain is not an
/// error: it means the release has not progressed further. Every other
/// deviation is a distinct typed error.
pub fn verify_state_chain(
    records: &[ReleaseStateRecordV1],
    manifest: &ReleaseManifestV1,
    trust_root: &ReleaseTrustRootV1,
    now: u64,
) -> Result<ChainProgress, ReleaseIntegrityError> {
    if records.len() > CANONICAL_RELEASE_STATES.len() {
        return Err(ReleaseIntegrityError::TooManyRecords {
            count: records.len(),
        });
    }

    let mut key_ids: HashSet<&str> = HashSet::new();
    let mut evidence_seen: HashSet<&str> = HashSet::new();
    let mut previous_digest: Option<&str> = None;

    for (position, record) in records.iter().enumerate() {
        if record.schema != RELEASE_STATE_RECORD_SCHEMA
            || record.schema_version != RELEASE_STATE_RECORD_SCHEMA_VERSION
        {
            return Err(ReleaseIntegrityError::UnsupportedSchema {
                schema: record.schema.clone(),
                version: record.schema_version,
            });
        }

        // Canonical order and ordinal agreement.
        let expected = CANONICAL_RELEASE_STATES[position];
        if record.body.state != expected {
            return Err(ReleaseIntegrityError::NonCanonicalOrder {
                position,
                expected,
                found: record.body.state,
            });
        }
        if record.body.ordinal != record.body.state.ordinal() {
            return Err(ReleaseIntegrityError::OrdinalMismatch {
                position,
                declared: record.body.state,
                ordinal: record.body.ordinal,
            });
        }

        validate_state_body(&record.body)?;
        if hash_serializable(&record.body)? != record.body_sha256 {
            return Err(ReleaseIntegrityError::DigestMismatch);
        }

        // Bound to this manifest, and to the record before it.
        if record.body.manifest_sha256 != manifest.body_sha256 {
            return Err(ReleaseIntegrityError::ManifestMismatch { position });
        }
        if record.body.previous_record_sha256.as_deref() != previous_digest {
            return Err(ReleaseIntegrityError::PreviousDigestMismatch { position });
        }

        // Release acceptance may not be reached over an absent certification.
        if record.body.state == ReleaseState::ReleaseAcceptance
            && matches!(manifest.body.certification, Evidence::Unavailable { .. })
        {
            return Err(ReleaseIntegrityError::UnavailableCertificationAtAcceptance);
        }

        // Distinct key per state, and the key's ROLE must equal this state.
        if !key_ids.insert(record.authority.key_id.as_str()) {
            return Err(ReleaseIntegrityError::DuplicateKeyId {
                key_id: record.authority.key_id.clone(),
            });
        }
        let key = trust_root.resolve(&record.authority.key_id, record.body.state, now)?;
        let signature = decode_signature(&record.authority.signature_base64)?;
        key.verify(&record.signature_message(), &signature)
            .map_err(|_| ReleaseIntegrityError::InvalidSignature)?;

        // Evidence disjointness across states.
        for entry in &record.body.evidence {
            if !evidence_seen.insert(entry.sha256.as_str()) {
                return Err(ReleaseIntegrityError::EvidenceReuse {
                    state: record.body.state,
                    sha256: entry.sha256.clone(),
                });
            }
        }

        previous_digest = Some(&record.body_sha256);
    }

    Ok(ChainProgress {
        highest_state: records.last().map(|record| record.body.state),
        records_verified: records.len(),
    })
}

/// Parse a chain from JSON and verify it. Unknown fields — including an
/// invented state name — are refused during this parse.
pub fn parse_and_verify_state_chain(
    chain_json: &[u8],
    manifest: &ReleaseManifestV1,
    trust_root: &ReleaseTrustRootV1,
    now: u64,
) -> Result<(Vec<ReleaseStateRecordV1>, ChainProgress), ReleaseIntegrityError> {
    let records: Vec<ReleaseStateRecordV1> = serde_json::from_slice(chain_json)
        .map_err(|error| ReleaseIntegrityError::InvalidJson(error.to_string()))?;
    let progress = verify_state_chain(&records, manifest, trust_root, now)?;
    Ok((records, progress))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_state_name_fails_to_deserialize_before_any_logic_runs() {
        let json = r#"{"state":"quarantine","ordinal":0,"manifest_sha256":"aa","previous_record_sha256":null,"evidence":[]}"#;
        let parsed = serde_json::from_str::<ReleaseStateBodyV1>(json);
        assert!(
            parsed.is_err(),
            "an unknown state name must not deserialize"
        );
        // Pristine control: a canonical state name parses.
        let ok = r#"{"state":"packaging","ordinal":0,"manifest_sha256":"aa","previous_record_sha256":null,"evidence":[]}"#;
        assert!(serde_json::from_str::<ReleaseStateBodyV1>(ok).is_ok());
    }

    #[test]
    fn chain_progress_is_accepted_only_at_release_acceptance() {
        for state in CANONICAL_RELEASE_STATES {
            let progress = ChainProgress {
                highest_state: Some(state),
                records_verified: state.ordinal() as usize + 1,
            };
            assert_eq!(
                progress.is_accepted(),
                state == ReleaseState::ReleaseAcceptance
            );
        }
        assert!(
            !ChainProgress {
                highest_state: None,
                records_verified: 0
            }
            .is_accepted()
        );
    }
}
