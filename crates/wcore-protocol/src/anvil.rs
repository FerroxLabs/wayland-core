//! Versioned Anvil receipt wire contract and host-side authority reducer.
//!
//! Only the two top-level event variants in [`AnvilAuthorityEvent`] can alter
//! receipt authority. Receipt-shaped text, plugin payloads, and nested child
//! events are deliberately inert.

use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

pub const ANVIL_RECEIPT_CONTRACT_VERSION: &str = "1.0";
pub const ANVIL_RECEIPT_ORIGIN: &str = "core/anvil";
pub const ANVIL_DIGEST_ALGORITHM: &str = "sha256";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnvilReceipt {
    pub receipt_id: String,
    pub event_id: String,
    pub origin: String,
    pub contract_version: String,
    /// Extension identifiers a consumer MUST understand before applying this
    /// event. Unknown ordinary fields remain forward-additive; an unknown
    /// entry here fails closed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_extensions: Vec<String>,
    pub session_id: String,
    pub run_id: String,
    pub task_id: String,
    pub sequence: u64,
    pub issued_at_unix_ms: u64,
    pub digest_algorithm: String,
    pub artifact_scope: String,
    pub artifact_digest: String,
    pub gate_closure_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supersedes_receipt_id: Option<String>,
    pub terminal_state: String,
    pub stamp: String,
    pub checks_passed: u32,
    pub checks_total: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub coverage: Option<String>,
    pub iterations: u32,
    #[serde(default)]
    pub valve_fires: u32,
    pub cost_microcents: u64,
    pub priced: bool,
    pub engine_version: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnvilInvalidationReason {
    ArtifactMutated,
    GateRevoked,
    Superseded,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnvilReceiptInvalidation {
    pub event_id: String,
    pub origin: String,
    pub contract_version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_extensions: Vec<String>,
    pub receipt_id: String,
    pub session_id: String,
    pub run_id: String,
    pub task_id: String,
    pub sequence: u64,
    pub issued_at_unix_ms: u64,
    pub reason: AnvilInvalidationReason,
    pub prior_artifact_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_artifact_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AnvilAuthorityEvent {
    AnvilReceipt {
        #[serde(flatten)]
        receipt: AnvilReceipt,
    },
    AnvilReceiptInvalidated {
        #[serde(flatten)]
        invalidation: AnvilReceiptInvalidation,
    },
}

impl AnvilAuthorityEvent {
    fn event_id(&self) -> &str {
        match self {
            Self::AnvilReceipt { receipt } => &receipt.event_id,
            Self::AnvilReceiptInvalidated { invalidation } => &invalidation.event_id,
        }
    }

    fn session_id(&self) -> &str {
        match self {
            Self::AnvilReceipt { receipt } => &receipt.session_id,
            Self::AnvilReceiptInvalidated { invalidation } => &invalidation.session_id,
        }
    }

    fn sequence(&self) -> u64 {
        match self {
            Self::AnvilReceipt { receipt } => receipt.sequence,
            Self::AnvilReceiptInvalidated { invalidation } => invalidation.sequence,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnvilReceiptStatus {
    Active,
    Invalidated,
    Superseded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnvilApplyOutcome {
    Applied,
    Duplicate,
    Inert,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnvilReceiptError {
    Malformed(String),
    VersionMismatch(String),
    InvalidOrigin(String),
    InvalidField(&'static str),
    SequenceGap { expected: u64, observed: u64 },
    OutOfOrder { expected: u64, observed: u64 },
    EventConflict(String),
    ReceiptConflict(String),
    UnknownReceipt(String),
    CorrelationMismatch(String),
    UnknownCriticalExtension(String),
}

impl fmt::Display for AnvilReceiptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Malformed(message) => write!(f, "malformed Anvil authority event: {message}"),
            Self::VersionMismatch(version) => {
                write!(f, "unsupported Anvil receipt contract version: {version}")
            }
            Self::InvalidOrigin(origin) => write!(f, "invalid Anvil receipt origin: {origin}"),
            Self::InvalidField(field) => write!(f, "invalid Anvil receipt field: {field}"),
            Self::SequenceGap { expected, observed } => {
                write!(
                    f,
                    "Anvil receipt sequence gap: expected {expected}, observed {observed}"
                )
            }
            Self::OutOfOrder { expected, observed } => write!(
                f,
                "out-of-order Anvil receipt: expected {expected}, observed {observed}"
            ),
            Self::EventConflict(id) => write!(f, "conflicting Anvil event id: {id}"),
            Self::ReceiptConflict(id) => write!(f, "conflicting Anvil receipt id: {id}"),
            Self::UnknownReceipt(id) => write!(f, "unknown Anvil receipt id: {id}"),
            Self::CorrelationMismatch(id) => {
                write!(
                    f,
                    "Anvil invalidation correlation mismatch for receipt: {id}"
                )
            }
            Self::UnknownCriticalExtension(extension) => {
                write!(f, "unknown critical Anvil receipt extension: {extension}")
            }
        }
    }
}

impl std::error::Error for AnvilReceiptError {}

#[derive(Debug, Clone)]
struct StoredReceipt {
    receipt: AnvilReceipt,
    canonical: Vec<u8>,
    status: AnvilReceiptStatus,
}

/// Deterministic authority reducer for serialized top-level Anvil events.
#[derive(Debug, Default)]
pub struct AnvilReceiptReducer {
    next_sequence: HashMap<String, u64>,
    event_bytes: HashMap<String, Vec<u8>>,
    receipts: HashMap<String, StoredReceipt>,
}

impl AnvilReceiptReducer {
    pub fn apply_json_line(&mut self, line: &str) -> Result<AnvilApplyOutcome, AnvilReceiptError> {
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|e| AnvilReceiptError::Malformed(e.to_string()))?;
        let Some(object) = value.as_object() else {
            return Err(AnvilReceiptError::Malformed(
                "top-level value is not an object".to_string(),
            ));
        };
        let Some(event_type) = object.get("type").and_then(serde_json::Value::as_str) else {
            return Err(AnvilReceiptError::Malformed(
                "missing string field `type`".to_string(),
            ));
        };
        if !matches!(event_type, "anvil_receipt" | "anvil_receipt_invalidated") {
            return Ok(AnvilApplyOutcome::Inert);
        }
        let event: AnvilAuthorityEvent = serde_json::from_value(value)
            .map_err(|e| AnvilReceiptError::Malformed(e.to_string()))?;
        self.apply(event)
    }

    pub fn apply(
        &mut self,
        event: AnvilAuthorityEvent,
    ) -> Result<AnvilApplyOutcome, AnvilReceiptError> {
        validate_event(&event)?;
        let canonical =
            serde_json::to_vec(&event).map_err(|e| AnvilReceiptError::Malformed(e.to_string()))?;
        if let Some(previous) = self.event_bytes.get(event.event_id()) {
            return if previous == &canonical {
                Ok(AnvilApplyOutcome::Duplicate)
            } else {
                Err(AnvilReceiptError::EventConflict(
                    event.event_id().to_string(),
                ))
            };
        }

        let expected = self
            .next_sequence
            .get(event.session_id())
            .copied()
            .unwrap_or(0);
        let observed = event.sequence();
        if observed > expected {
            return Err(AnvilReceiptError::SequenceGap { expected, observed });
        }
        if observed < expected {
            return Err(AnvilReceiptError::OutOfOrder { expected, observed });
        }

        match &event {
            AnvilAuthorityEvent::AnvilReceipt { receipt } => {
                if let Some(previous) = self.receipts.get(&receipt.receipt_id) {
                    if previous.canonical != canonical {
                        return Err(AnvilReceiptError::ReceiptConflict(
                            receipt.receipt_id.clone(),
                        ));
                    }
                    return Ok(AnvilApplyOutcome::Duplicate);
                }
                if let Some(superseded_id) = &receipt.supersedes_receipt_id {
                    let Some(superseded) = self.receipts.get_mut(superseded_id) else {
                        return Err(AnvilReceiptError::UnknownReceipt(superseded_id.clone()));
                    };
                    if superseded.receipt.session_id != receipt.session_id
                        || superseded.receipt.task_id != receipt.task_id
                    {
                        return Err(AnvilReceiptError::CorrelationMismatch(
                            superseded_id.clone(),
                        ));
                    }
                    superseded.status = AnvilReceiptStatus::Superseded;
                }
                self.receipts.insert(
                    receipt.receipt_id.clone(),
                    StoredReceipt {
                        receipt: receipt.clone(),
                        canonical: canonical.clone(),
                        status: AnvilReceiptStatus::Active,
                    },
                );
            }
            AnvilAuthorityEvent::AnvilReceiptInvalidated { invalidation } => {
                let Some(stored) = self.receipts.get_mut(&invalidation.receipt_id) else {
                    return Err(AnvilReceiptError::UnknownReceipt(
                        invalidation.receipt_id.clone(),
                    ));
                };
                if stored.receipt.session_id != invalidation.session_id
                    || stored.receipt.run_id != invalidation.run_id
                    || stored.receipt.task_id != invalidation.task_id
                    || stored.receipt.artifact_digest != invalidation.prior_artifact_digest
                {
                    return Err(AnvilReceiptError::CorrelationMismatch(
                        invalidation.receipt_id.clone(),
                    ));
                }
                stored.status = match invalidation.reason {
                    AnvilInvalidationReason::Superseded => AnvilReceiptStatus::Superseded,
                    AnvilInvalidationReason::ArtifactMutated
                    | AnvilInvalidationReason::GateRevoked => AnvilReceiptStatus::Invalidated,
                };
            }
        }

        self.event_bytes
            .insert(event.event_id().to_string(), canonical);
        self.next_sequence
            .insert(event.session_id().to_string(), expected + 1);
        Ok(AnvilApplyOutcome::Applied)
    }

    #[must_use]
    pub fn status(&self, receipt_id: &str) -> Option<AnvilReceiptStatus> {
        self.receipts.get(receipt_id).map(|stored| stored.status)
    }

    /// Next durable sequence expected for `session_id` after replaying a
    /// receipt journal. Producers use this value before appending a new event.
    #[must_use]
    pub fn next_sequence(&self, session_id: &str) -> u64 {
        self.next_sequence.get(session_id).copied().unwrap_or(0)
    }
}

fn validate_event(event: &AnvilAuthorityEvent) -> Result<(), AnvilReceiptError> {
    let (origin, version) = match event {
        AnvilAuthorityEvent::AnvilReceipt { receipt } => {
            reject_unknown_extensions(&receipt.required_extensions)?;
            validate_nonempty("receipt_id", &receipt.receipt_id)?;
            validate_nonempty("event_id", &receipt.event_id)?;
            validate_nonempty("session_id", &receipt.session_id)?;
            validate_nonempty("run_id", &receipt.run_id)?;
            validate_nonempty("task_id", &receipt.task_id)?;
            validate_nonempty("artifact_scope", &receipt.artifact_scope)?;
            validate_nonempty("terminal_state", &receipt.terminal_state)?;
            validate_nonempty("stamp", &receipt.stamp)?;
            validate_nonempty("engine_version", &receipt.engine_version)?;
            if receipt.digest_algorithm != ANVIL_DIGEST_ALGORITHM {
                return Err(AnvilReceiptError::InvalidField("digest_algorithm"));
            }
            if receipt.checks_passed > receipt.checks_total {
                return Err(AnvilReceiptError::InvalidField("checks_passed"));
            }
            for digest in [&receipt.artifact_digest, &receipt.gate_closure_digest] {
                if !valid_sha256(digest) {
                    return Err(AnvilReceiptError::InvalidField("digest"));
                }
            }
            (&receipt.origin, &receipt.contract_version)
        }
        AnvilAuthorityEvent::AnvilReceiptInvalidated { invalidation } => {
            reject_unknown_extensions(&invalidation.required_extensions)?;
            validate_nonempty("receipt_id", &invalidation.receipt_id)?;
            validate_nonempty("event_id", &invalidation.event_id)?;
            validate_nonempty("session_id", &invalidation.session_id)?;
            validate_nonempty("run_id", &invalidation.run_id)?;
            validate_nonempty("task_id", &invalidation.task_id)?;
            if !valid_sha256(&invalidation.prior_artifact_digest)
                || invalidation
                    .observed_artifact_digest
                    .as_deref()
                    .is_some_and(|digest| !valid_sha256(digest))
            {
                return Err(AnvilReceiptError::InvalidField("digest"));
            }
            (&invalidation.origin, &invalidation.contract_version)
        }
    };
    if origin != ANVIL_RECEIPT_ORIGIN {
        return Err(AnvilReceiptError::InvalidOrigin(origin.to_string()));
    }
    let Some((major, minor)) = version.split_once('.') else {
        return Err(AnvilReceiptError::VersionMismatch(version.to_string()));
    };
    if major != "1" || minor.parse::<u64>().is_err() {
        return Err(AnvilReceiptError::VersionMismatch(version.to_string()));
    }
    Ok(())
}

fn validate_nonempty(field: &'static str, value: &str) -> Result<(), AnvilReceiptError> {
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err(AnvilReceiptError::InvalidField(field));
    }
    Ok(())
}

fn reject_unknown_extensions(extensions: &[String]) -> Result<(), AnvilReceiptError> {
    if let Some(extension) = extensions.first() {
        return Err(AnvilReceiptError::UnknownCriticalExtension(
            extension.clone(),
        ));
    }
    Ok(())
}

fn valid_sha256(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn digest(byte: char) -> String {
        format!("sha256:{}", byte.to_string().repeat(64))
    }

    fn receipt(sequence: u64) -> AnvilReceipt {
        AnvilReceipt {
            receipt_id: "receipt-1".into(),
            event_id: "event-1".into(),
            origin: ANVIL_RECEIPT_ORIGIN.into(),
            contract_version: ANVIL_RECEIPT_CONTRACT_VERSION.into(),
            required_extensions: Vec::new(),
            session_id: "session-1".into(),
            run_id: "run-1".into(),
            task_id: "task-1".into(),
            sequence,
            issued_at_unix_ms: 1,
            digest_algorithm: ANVIL_DIGEST_ALGORITHM.into(),
            artifact_scope: "git:tracked+untracked-excluding-ignored@.".into(),
            artifact_digest: digest('a'),
            gate_closure_digest: digest('b'),
            supersedes_receipt_id: None,
            terminal_state: "verified".into(),
            stamp: "verified".into(),
            checks_passed: 1,
            checks_total: 1,
            coverage: None,
            iterations: 1,
            valve_fires: 0,
            cost_microcents: 0,
            priced: false,
            engine_version: "0.12.25".into(),
        }
    }

    fn receipt_event(sequence: u64) -> AnvilAuthorityEvent {
        AnvilAuthorityEvent::AnvilReceipt {
            receipt: receipt(sequence),
        }
    }

    #[test]
    fn exact_replay_is_idempotent_and_conflicting_duplicate_fails_closed() {
        let mut reducer = AnvilReceiptReducer::default();
        let event = receipt_event(0);
        assert_eq!(
            reducer.apply(event.clone()).unwrap(),
            AnvilApplyOutcome::Applied
        );
        assert_eq!(reducer.apply(event).unwrap(), AnvilApplyOutcome::Duplicate);

        let mut conflict = receipt(0);
        conflict.artifact_digest = digest('c');
        assert!(matches!(
            reducer.apply(AnvilAuthorityEvent::AnvilReceipt { receipt: conflict }),
            Err(AnvilReceiptError::EventConflict(_))
        ));
    }

    #[test]
    fn sequence_gap_and_out_of_order_events_fail_closed() {
        let mut reducer = AnvilReceiptReducer::default();
        assert!(matches!(
            reducer.apply(receipt_event(2)),
            Err(AnvilReceiptError::SequenceGap {
                expected: 0,
                observed: 2
            })
        ));
        reducer.apply(receipt_event(0)).unwrap();
        let mut older = receipt(0);
        older.receipt_id = "receipt-2".into();
        older.event_id = "event-2".into();
        assert!(matches!(
            reducer.apply(AnvilAuthorityEvent::AnvilReceipt { receipt: older }),
            Err(AnvilReceiptError::OutOfOrder {
                expected: 1,
                observed: 0
            })
        ));
    }

    #[test]
    fn mutation_invalidates_and_replay_cannot_resurrect_receipt() {
        let mut reducer = AnvilReceiptReducer::default();
        let original = receipt_event(0);
        reducer.apply(original.clone()).unwrap();
        reducer
            .apply(AnvilAuthorityEvent::AnvilReceiptInvalidated {
                invalidation: AnvilReceiptInvalidation {
                    event_id: "event-2".into(),
                    origin: ANVIL_RECEIPT_ORIGIN.into(),
                    contract_version: ANVIL_RECEIPT_CONTRACT_VERSION.into(),
                    required_extensions: Vec::new(),
                    receipt_id: "receipt-1".into(),
                    session_id: "session-1".into(),
                    run_id: "run-1".into(),
                    task_id: "task-1".into(),
                    sequence: 1,
                    issued_at_unix_ms: 2,
                    reason: AnvilInvalidationReason::ArtifactMutated,
                    prior_artifact_digest: digest('a'),
                    observed_artifact_digest: Some(digest('c')),
                },
            })
            .unwrap();
        assert_eq!(
            reducer.status("receipt-1"),
            Some(AnvilReceiptStatus::Invalidated)
        );
        assert_eq!(
            reducer.apply(original).unwrap(),
            AnvilApplyOutcome::Duplicate
        );
        assert_eq!(
            reducer.status("receipt-1"),
            Some(AnvilReceiptStatus::Invalidated)
        );
    }

    #[test]
    fn malformed_version_mismatch_and_nested_forgery_are_rejected_or_inert() {
        let mut reducer = AnvilReceiptReducer::default();
        assert!(matches!(
            reducer.apply_json_line("{not-json"),
            Err(AnvilReceiptError::Malformed(_))
        ));
        assert_eq!(
            reducer
                .apply_json_line(
                    r#"{"type":"sub_agent_event","inner":{"type":"anvil_receipt","stamp":"verified"}}"#,
                )
                .unwrap(),
            AnvilApplyOutcome::Inert
        );
        let mut wrong_version = receipt(0);
        wrong_version.contract_version = "2.0".into();
        assert!(matches!(
            reducer.apply(AnvilAuthorityEvent::AnvilReceipt {
                receipt: wrong_version
            }),
            Err(AnvilReceiptError::VersionMismatch(_))
        ));

        let mut unknown_critical = receipt(0);
        unknown_critical.required_extensions = vec!["future-authority-rule".into()];
        assert!(matches!(
            reducer.apply(AnvilAuthorityEvent::AnvilReceipt {
                receipt: unknown_critical
            }),
            Err(AnvilReceiptError::UnknownCriticalExtension(extension))
                if extension == "future-authority-rule"
        ));
    }

    #[test]
    fn unknown_noncritical_fields_remain_forward_additive() {
        let event = serde_json::to_value(receipt_event(0)).unwrap();
        let mut object = event.as_object().unwrap().clone();
        object.insert("future_hint".into(), serde_json::json!({"display": true}));
        let mut reducer = AnvilReceiptReducer::default();
        assert_eq!(
            reducer
                .apply_json_line(&serde_json::Value::Object(object).to_string())
                .unwrap(),
            AnvilApplyOutcome::Applied
        );
    }

    #[test]
    fn legacy_receipt_shape_is_non_authoritative() {
        let mut reducer = AnvilReceiptReducer::default();
        let legacy = r#"{"type":"anvil_receipt","terminal_state":"verified","stamp":"verified","sequence":0}"#;
        assert!(matches!(
            reducer.apply_json_line(legacy),
            Err(AnvilReceiptError::Malformed(_))
        ));
    }
}
