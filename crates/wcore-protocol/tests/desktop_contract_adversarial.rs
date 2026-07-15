use std::fs;
use std::path::Path;

use serde_json::Value;
use wcore_protocol::commands::ProtocolCommand;
use wcore_protocol::contract::CONTRACT_ROOT;

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTRACT_ROOT)
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
fn current_host_contract_drops_unknown_noncritical_event() {
    let value: Value = serde_json::from_slice(
        &fs::read(root().join("adversarial/events/unknown-noncritical.jsonl")).unwrap(),
    )
    .unwrap();
    assert_eq!(value["type"], "future_observation");
    assert!(value.get("critical").is_none());
}

#[test]
fn unknown_critical_and_version_mismatch_are_explicitly_deferred() {
    let critical: Value = serde_json::from_slice(
        &fs::read(root().join("adversarial/events/unknown-critical.deferred.jsonl")).unwrap(),
    )
    .unwrap();
    assert_eq!(critical["critical"], true);
    assert_eq!(critical["contract_version"], "1.0");

    let mismatch: Value = serde_json::from_slice(
        &fs::read(root().join("adversarial/events/version-mismatch.deferred.jsonl")).unwrap(),
    )
    .unwrap();
    assert_eq!(mismatch["contract"]["major"], 2);

    let deferred = fs::read_to_string(root().join("DEFERRED.md")).unwrap();
    assert!(deferred.contains("unknown_critical_fail_closed"));
    assert!(deferred.contains("version_mismatch_handshake"));
    assert!(deferred.contains("ordering_duplicate_terminal_reducer"));
    assert!(deferred.contains("anvil_origin_replay_mutation_staleness"));
}

#[test]
fn canonical_ready_does_not_fabricate_unimplemented_negotiation() {
    let ready: Value =
        serde_json::from_slice(&fs::read(root().join("events/ready.json")).unwrap()).unwrap();
    assert!(ready.get("contract").is_none());
}
