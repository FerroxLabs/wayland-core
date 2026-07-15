use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::events::Capabilities;

use super::spec::PRODUCER_EVENT_TYPES;

/// Availability of one producer-contract capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractCapabilityStatus {
    Available,
    PublicationBound,
    ShapeOnly,
    Unavailable,
}

/// Versioned producer contract advertised by the Core `ready` event.
///
/// This is descriptive output. Deserializing it never grants authority or
/// changes the live Core policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContractDescriptor {
    pub name: String,
    pub major: u64,
    pub minor: u64,
    pub generator: String,
    pub fixture_digest: String,
    pub schema_digest: String,
    pub source_inputs_digest: String,
    pub capabilities: BTreeMap<String, ContractCapabilityStatus>,
}

impl ContractDescriptor {
    fn validate(&self) -> Result<(), HostObservationError> {
        if self.name.is_empty() || self.generator.is_empty() || self.major == 0 {
            return Err(HostObservationError::InvalidContractDescriptor);
        }
        for digest in [
            &self.fixture_digest,
            &self.schema_digest,
            &self.source_inputs_digest,
        ] {
            let Some(hex) = digest.strip_prefix("sha256:") else {
                return Err(HostObservationError::InvalidContractDescriptor);
            };
            if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
                return Err(HostObservationError::InvalidContractDescriptor);
            }
        }
        if self.capabilities.is_empty() {
            return Err(HostObservationError::InvalidContractDescriptor);
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct EmbeddedContractVersion {
    name: String,
    major: u64,
    minor: u64,
}

#[derive(Deserialize)]
struct EmbeddedContractManifest {
    contract: EmbeddedContractVersion,
    generator: String,
    fixture_digest: String,
    schema_digest: String,
    source_inputs_digest: String,
    capabilities: BTreeMap<String, ContractCapabilityStatus>,
}

/// Descriptive-only view of the initial execution policy. This deliberately
/// does not deserialize into Core's authority-bearing policy types.
#[derive(Deserialize)]
struct ObservedExecutionPolicy {
    critical: bool,
    contract_version: String,
    revision: u64,
    reason: String,
    #[serde(rename = "effective_at_unix_ms")]
    _effective_at_unix_ms: u64,
    policy: ObservedEffectivePolicy,
}

#[derive(Deserialize)]
struct ObservedEffectivePolicy {
    posture: String,
    approvals: String,
    sandbox: String,
    source: String,
    managed_floor_active: bool,
    #[serde(default)]
    dangerous_activation_id: Option<String>,
    #[serde(default)]
    dangerous_expires_at_unix_ms: Option<u64>,
}

impl ObservedExecutionPolicy {
    fn validate(&self) -> Result<(), HostObservationError> {
        if !self.critical
            || self.revision != 0
            || !matches!(self.reason.as_str(), "launch" | "resume")
            || crate::execution_policy::validate_execution_policy_contract_version(
                &self.contract_version,
            )
            .is_err()
        {
            return Err(HostObservationError::InvalidReadyField {
                field: "execution_policy",
            });
        }

        let policy = &self.policy;
        if !matches!(policy.posture.as_str(), "smart" | "managed" | "dangerous")
            || !matches!(policy.approvals.as_str(), "prompt" | "auto_edit" | "bypass")
            || !matches!(policy.sandbox.as_str(), "required" | "bypass")
            || policy.source.is_empty()
        {
            return Err(HostObservationError::InvalidReadyField {
                field: "execution_policy",
            });
        }

        let dangerous = policy.posture == "dangerous";
        let has_dangerous_identity = policy
            .dangerous_activation_id
            .as_deref()
            .is_some_and(|value| !value.is_empty())
            && policy.dangerous_expires_at_unix_ms.is_some();
        let policy_is_consistent = if dangerous {
            policy.approvals == "bypass" && policy.sandbox == "bypass" && has_dangerous_identity
        } else {
            policy.sandbox == "required"
                && policy.dangerous_activation_id.is_none()
                && policy.dangerous_expires_at_unix_ms.is_none()
                && (policy.posture != "managed" || policy.managed_floor_active)
        };
        if !policy_is_consistent {
            return Err(HostObservationError::InvalidReadyField {
                field: "execution_policy",
            });
        }
        Ok(())
    }
}

/// Return the contract descriptor compiled into this producer.
///
/// The manifest is generated and byte-checked by `wcore-contract check` in
/// CI. Parsing it here is therefore a build invariant, not untrusted input.
pub fn producer_contract_descriptor() -> ContractDescriptor {
    let manifest: EmbeddedContractManifest =
        serde_json::from_str(include_str!("../../contracts/desktop/v1/manifest.json"))
            .expect("embedded Desktop contract manifest must pass the generated corpus gate");
    let descriptor = ContractDescriptor {
        name: manifest.contract.name,
        major: manifest.contract.major,
        minor: manifest.contract.minor,
        generator: manifest.generator,
        fixture_digest: manifest.fixture_digest,
        schema_digest: manifest.schema_digest,
        source_inputs_digest: manifest.source_inputs_digest,
        capabilities: manifest.capabilities,
    };
    descriptor
        .validate()
        .expect("embedded Desktop contract descriptor must be structurally valid");
    descriptor
}

/// A non-authoritative result from observing one producer JSON line.
#[derive(Debug, Clone, PartialEq)]
pub enum HostObservation {
    Negotiated(ContractDescriptor),
    Event(Value),
    DroppedUnknownNonCritical { event_type: String },
}

/// Fail-closed errors from the reference host-side contract observer.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum HostObservationError {
    #[error("producer line is not valid JSON")]
    MalformedJson,
    #[error("producer line must be a JSON object")]
    NonObject,
    #[error("producer line is missing a type field")]
    MissingType,
    #[error("producer line type must be a string")]
    NonStringType,
    #[error("ready must negotiate a contract before other events")]
    ReadyRequired,
    #[error("ready is missing its contract descriptor")]
    MissingContractDescriptor,
    #[error("ready is missing required field {field}")]
    MissingReadyField { field: &'static str },
    #[error("ready field {field} is malformed or inconsistent")]
    InvalidReadyField { field: &'static str },
    #[error("ready contract descriptor is malformed")]
    InvalidContractDescriptor,
    #[error("producer contract name is unsupported: {actual}")]
    UnsupportedContractName { actual: String },
    #[error("producer contract major is unsupported: {actual}")]
    UnsupportedContractMajor { actual: u64 },
    #[error("producer contract minor does not match the pinned host contract: {actual}")]
    ContractMinorMismatch { actual: u64 },
    #[error("producer contract generator does not match the pinned host contract")]
    GeneratorMismatch,
    #[error("producer schema digest does not match the pinned host schema")]
    SchemaDigestMismatch,
    #[error("producer fixture digest does not match the pinned host corpus")]
    FixtureDigestMismatch,
    #[error("producer source-input digest does not match the pinned host contract")]
    SourceInputsDigestMismatch,
    #[error("producer capability statuses do not match the pinned host contract")]
    CapabilityStatusMismatch,
    #[error("ready contract was negotiated more than once")]
    DuplicateReady,
    #[error("unknown event is contract-critical: {event_type}")]
    UnknownCriticalEvent { event_type: String },
    #[error("unknown event has no explicit noncritical classification: {event_type}")]
    UnknownCriticality { event_type: String },
}

/// Reference decoder for a host pinned to one generated producer contract.
///
/// It returns generic JSON observations only. It cannot construct commands,
/// execution policy, approvals, or any other Core authority type.
#[derive(Debug, Clone)]
pub struct HostContractObserver {
    expected: ContractDescriptor,
    negotiated: bool,
}

impl HostContractObserver {
    pub fn new(expected: ContractDescriptor) -> Self {
        Self {
            expected,
            negotiated: false,
        }
    }

    pub fn negotiated(&self) -> bool {
        self.negotiated
    }

    pub fn observe_json_line(
        &mut self,
        line: &[u8],
    ) -> Result<HostObservation, HostObservationError> {
        let value: Value =
            serde_json::from_slice(line).map_err(|_| HostObservationError::MalformedJson)?;
        let object = value.as_object().ok_or(HostObservationError::NonObject)?;
        let event_type = object
            .get("type")
            .ok_or(HostObservationError::MissingType)?
            .as_str()
            .ok_or(HostObservationError::NonStringType)?;

        if event_type == "ready" {
            if self.negotiated {
                return Err(HostObservationError::DuplicateReady);
            }
            match object.get("version") {
                None => {
                    return Err(HostObservationError::MissingReadyField { field: "version" });
                }
                Some(Value::String(version)) if !version.is_empty() => {}
                Some(_) => {
                    return Err(HostObservationError::InvalidReadyField { field: "version" });
                }
            }

            let raw_capabilities =
                object
                    .get("capabilities")
                    .ok_or(HostObservationError::MissingReadyField {
                        field: "capabilities",
                    })?;
            let capabilities: Capabilities = serde_json::from_value(raw_capabilities.clone())
                .map_err(|_| HostObservationError::InvalidReadyField {
                    field: "capabilities",
                })?;
            if !capabilities
                .modes
                .iter()
                .any(|mode| mode == &capabilities.current_mode)
            {
                return Err(HostObservationError::InvalidReadyField {
                    field: "capabilities",
                });
            }

            let raw_descriptor = object
                .get("contract")
                .ok_or(HostObservationError::MissingContractDescriptor)?;
            let descriptor: ContractDescriptor = serde_json::from_value(raw_descriptor.clone())
                .map_err(|_| HostObservationError::InvalidContractDescriptor)?;
            descriptor.validate()?;

            let execution_policy: ObservedExecutionPolicy = serde_json::from_value(
                object
                    .get("execution_policy")
                    .ok_or(HostObservationError::MissingReadyField {
                        field: "execution_policy",
                    })?
                    .clone(),
            )
            .map_err(|_| HostObservationError::InvalidReadyField {
                field: "execution_policy",
            })?;
            execution_policy.validate()?;
            if descriptor.name != self.expected.name {
                return Err(HostObservationError::UnsupportedContractName {
                    actual: descriptor.name,
                });
            }
            if descriptor.major != self.expected.major {
                return Err(HostObservationError::UnsupportedContractMajor {
                    actual: descriptor.major,
                });
            }
            if descriptor.minor != self.expected.minor {
                return Err(HostObservationError::ContractMinorMismatch {
                    actual: descriptor.minor,
                });
            }
            if descriptor.generator != self.expected.generator {
                return Err(HostObservationError::GeneratorMismatch);
            }
            if descriptor.schema_digest != self.expected.schema_digest {
                return Err(HostObservationError::SchemaDigestMismatch);
            }
            if descriptor.fixture_digest != self.expected.fixture_digest {
                return Err(HostObservationError::FixtureDigestMismatch);
            }
            if descriptor.source_inputs_digest != self.expected.source_inputs_digest {
                return Err(HostObservationError::SourceInputsDigestMismatch);
            }
            if descriptor.capabilities != self.expected.capabilities {
                return Err(HostObservationError::CapabilityStatusMismatch);
            }
            self.negotiated = true;
            return Ok(HostObservation::Negotiated(descriptor));
        }

        if !self.negotiated {
            return Err(HostObservationError::ReadyRequired);
        }
        if PRODUCER_EVENT_TYPES.contains(&event_type) {
            return Ok(HostObservation::Event(value));
        }

        match object.get("critical").and_then(Value::as_bool) {
            Some(false) => Ok(HostObservation::DroppedUnknownNonCritical {
                event_type: event_type.to_owned(),
            }),
            Some(true) => Err(HostObservationError::UnknownCriticalEvent {
                event_type: event_type.to_owned(),
            }),
            None => Err(HostObservationError::UnknownCriticality {
                event_type: event_type.to_owned(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn descriptor() -> ContractDescriptor {
        ContractDescriptor {
            name: "wayland-desktop-core".into(),
            major: 1,
            minor: 0,
            generator: "test-generator/1".into(),
            fixture_digest: format!("sha256:{}", "1".repeat(64)),
            schema_digest: format!("sha256:{}", "2".repeat(64)),
            source_inputs_digest: format!("sha256:{}", "3".repeat(64)),
            capabilities: BTreeMap::from([(
                "contract_negotiation".into(),
                ContractCapabilityStatus::Available,
            )]),
        }
    }

    fn ready(descriptor: &ContractDescriptor) -> Vec<u8> {
        serde_json::to_vec(&ready_value(descriptor)).unwrap()
    }

    fn ready_value(descriptor: &ContractDescriptor) -> Value {
        json!({
            "capabilities": {
                "current_mode": "default",
                "effort": true,
                "effort_levels": ["low", "medium", "high"],
                "mcp": true,
                "modes": ["default", "auto_edit", "force"],
                "thinking": true,
                "tool_approval": true
            },
            "contract": descriptor,
            "execution_policy": {
                "contract_version": "1.0",
                "critical": true,
                "effective_at_unix_ms": 1,
                "policy": {
                    "approvals": "prompt",
                    "managed_floor_active": false,
                    "posture": "smart",
                    "sandbox": "required",
                    "source": "desktop_local_launch"
                },
                "reason": "launch",
                "revision": 0
            },
            "type": "ready",
            "version": "0.12.25"
        })
    }

    #[test]
    fn serialized_ready_must_match_the_pinned_descriptor() {
        let expected = descriptor();
        let mut observer = HostContractObserver::new(expected.clone());
        assert_eq!(
            observer.observe_json_line(&ready(&expected)),
            Ok(HostObservation::Negotiated(expected))
        );
        assert!(observer.negotiated());
    }

    #[test]
    fn unsupported_major_fails_before_negotiation() {
        let expected = descriptor();
        let mut incompatible = expected.clone();
        incompatible.major = 2;
        let mut observer = HostContractObserver::new(expected);
        assert_eq!(
            observer.observe_json_line(&ready(&incompatible)),
            Err(HostObservationError::UnsupportedContractMajor { actual: 2 })
        );
        assert!(!observer.negotiated());
    }

    #[test]
    fn ready_missing_each_required_field_fails_before_negotiation() {
        let expected = descriptor();
        for (field, error) in [
            (
                "version",
                HostObservationError::MissingReadyField { field: "version" },
            ),
            (
                "capabilities",
                HostObservationError::MissingReadyField {
                    field: "capabilities",
                },
            ),
            ("contract", HostObservationError::MissingContractDescriptor),
            (
                "execution_policy",
                HostObservationError::MissingReadyField {
                    field: "execution_policy",
                },
            ),
        ] {
            let mut value = ready_value(&expected);
            value.as_object_mut().unwrap().remove(field);
            let mut observer = HostContractObserver::new(expected.clone());
            assert_eq!(
                observer.observe_json_line(&serde_json::to_vec(&value).unwrap()),
                Err(error),
                "missing {field}"
            );
            assert!(!observer.negotiated(), "missing {field}");
        }
    }

    #[test]
    fn ready_malformed_required_shapes_fail_before_negotiation() {
        let expected = descriptor();
        for (field, malformed, error) in [
            (
                "version",
                json!(12),
                HostObservationError::InvalidReadyField { field: "version" },
            ),
            (
                "capabilities",
                json!([]),
                HostObservationError::InvalidReadyField {
                    field: "capabilities",
                },
            ),
            (
                "contract",
                json!([]),
                HostObservationError::InvalidContractDescriptor,
            ),
            (
                "execution_policy",
                json!({"critical": true}),
                HostObservationError::InvalidReadyField {
                    field: "execution_policy",
                },
            ),
        ] {
            let mut value = ready_value(&expected);
            value[field] = malformed;
            let mut observer = HostContractObserver::new(expected.clone());
            assert_eq!(
                observer.observe_json_line(&serde_json::to_vec(&value).unwrap()),
                Err(error),
                "malformed {field}"
            );
            assert!(!observer.negotiated(), "malformed {field}");
        }
    }

    #[test]
    fn ready_inconsistent_capabilities_and_policy_fail_before_negotiation() {
        let expected = descriptor();
        let mut wrong_mode = ready_value(&expected);
        wrong_mode["capabilities"]["current_mode"] = json!("force");

        let mut unsafe_smart_policy = ready_value(&expected);
        unsafe_smart_policy["execution_policy"]["policy"]["sandbox"] = json!("bypass");

        let mut noncritical_policy = ready_value(&expected);
        noncritical_policy["execution_policy"]["critical"] = json!(false);

        let mut noninitial_policy = ready_value(&expected);
        noninitial_policy["execution_policy"]["revision"] = json!(1);

        for (name, value, field) in [
            ("current mode", wrong_mode, "capabilities"),
            ("smart sandbox", unsafe_smart_policy, "execution_policy"),
            ("criticality", noncritical_policy, "execution_policy"),
            ("initial revision", noninitial_policy, "execution_policy"),
        ] {
            let mut observer = HostContractObserver::new(expected.clone());
            assert_eq!(
                observer.observe_json_line(&serde_json::to_vec(&value).unwrap()),
                Err(HostObservationError::InvalidReadyField { field }),
                "{name}"
            );
            assert!(!observer.negotiated(), "{name}");
        }
    }

    #[test]
    fn unknown_events_require_explicit_noncritical_classification() {
        let expected = descriptor();
        let mut observer = HostContractObserver::new(expected.clone());
        observer.observe_json_line(&ready(&expected)).unwrap();

        assert!(matches!(
            observer.observe_json_line(br#"{"critical":false,"type":"future_info"}"#),
            Ok(HostObservation::DroppedUnknownNonCritical { .. })
        ));
        assert!(matches!(
            observer.observe_json_line(br#"{"critical":true,"type":"future_authority"}"#),
            Err(HostObservationError::UnknownCriticalEvent { .. })
        ));
        assert!(matches!(
            observer.observe_json_line(br#"{"type":"future_unclassified"}"#),
            Err(HostObservationError::UnknownCriticality { .. })
        ));
    }
}
