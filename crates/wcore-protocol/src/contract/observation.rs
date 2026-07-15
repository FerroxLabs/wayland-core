use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use super::spec::PRODUCER_EVENT_TYPES;

/// Availability of one producer-contract capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContractCapabilityStatus {
    Available,
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
            let raw_descriptor = object
                .get("contract")
                .ok_or(HostObservationError::MissingContractDescriptor)?;
            let descriptor: ContractDescriptor = serde_json::from_value(raw_descriptor.clone())
                .map_err(|_| HostObservationError::InvalidContractDescriptor)?;
            descriptor.validate()?;
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
        serde_json::to_vec(&json!({"contract": descriptor, "type": "ready"})).unwrap()
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
