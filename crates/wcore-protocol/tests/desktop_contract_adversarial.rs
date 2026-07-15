use std::fs;
use std::path::Path;

use serde_json::Value;
use wcore_protocol::commands::ProtocolCommand;
use wcore_protocol::contract::{
    CONTRACT_ROOT, ContractCapabilityStatus, HostContractObserver, HostObservation,
    HostObservationError, producer_contract_descriptor,
};

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTRACT_ROOT)
}

fn observer() -> HostContractObserver {
    HostContractObserver::new(producer_contract_descriptor())
}

fn negotiate(observer: &mut HostContractObserver) {
    let ready = fs::read(root().join("events/ready.json")).unwrap();
    assert!(matches!(
        observer.observe_json_line(&ready),
        Ok(HostObservation::Negotiated(_))
    ));
}

#[test]
fn malformed_and_unknown_commands_never_deserialize_for_dispatch() {
    for name in [
        "invalid-json.jsonl",
        "missing-type.jsonl",
        "non-object.jsonl",
        "non-string-type.jsonl",
        "unknown-type.jsonl",
        "wrong-required-field.jsonl",
    ] {
        let bytes = fs::read(root().join("adversarial/commands").join(name)).unwrap();
        assert!(
            serde_json::from_slice::<ProtocolCommand>(&bytes).is_err(),
            "{name} unexpectedly produced a dispatchable command"
        );
    }
}

#[test]
fn negotiated_host_drops_serialized_unknown_noncritical_event() {
    let mut observer = observer();
    negotiate(&mut observer);
    let line = fs::read(root().join("adversarial/events/unknown-noncritical.jsonl")).unwrap();
    assert_eq!(
        observer.observe_json_line(&line),
        Ok(HostObservation::DroppedUnknownNonCritical {
            event_type: "future_observation".into()
        })
    );
}

#[test]
fn negotiated_host_rejects_unknown_critical_and_unclassified_events() {
    for (name, expected) in [
        (
            "unknown-critical.jsonl",
            HostObservationError::UnknownCriticalEvent {
                event_type: "future_authority".into(),
            },
        ),
        (
            "unknown-criticality.jsonl",
            HostObservationError::UnknownCriticality {
                event_type: "future_unclassified".into(),
            },
        ),
    ] {
        let mut observer = observer();
        negotiate(&mut observer);
        let line = fs::read(root().join("adversarial/events").join(name)).unwrap();
        assert_eq!(observer.observe_json_line(&line), Err(expected), "{name}");
    }
}

#[test]
fn ready_replay_fails_closed_on_major_schema_and_fixture_mismatch() {
    for (name, expected) in [
        (
            "version-mismatch.jsonl",
            HostObservationError::UnsupportedContractMajor { actual: 2 },
        ),
        (
            "schema-mismatch.jsonl",
            HostObservationError::SchemaDigestMismatch,
        ),
        (
            "fixture-mismatch.jsonl",
            HostObservationError::FixtureDigestMismatch,
        ),
    ] {
        let line = fs::read(root().join("adversarial/events").join(name)).unwrap();
        assert_eq!(observer().observe_json_line(&line), Err(expected), "{name}");
    }
}

#[test]
fn remaining_deferrals_exclude_live_negotiation_guarantees() {
    let deferred = fs::read_to_string(root().join("DEFERRED.md")).unwrap();
    assert!(!deferred.contains("unknown_critical_fail_closed"));
    assert!(!deferred.contains("version_mismatch_handshake"));
    assert!(deferred.contains("ordering_duplicate_terminal_reducer"));
    assert!(deferred.contains("anvil_origin_replay_mutation_staleness"));
}

#[test]
fn canonical_ready_advertises_the_embedded_generated_contract() {
    let ready: Value =
        serde_json::from_slice(&fs::read(root().join("events/ready.json")).unwrap()).unwrap();
    let expected = producer_contract_descriptor();
    assert_eq!(ready["contract"], serde_json::to_value(&expected).unwrap());
    assert_eq!(
        expected.capabilities.get("contract_negotiation"),
        Some(&ContractCapabilityStatus::Available)
    );
}
