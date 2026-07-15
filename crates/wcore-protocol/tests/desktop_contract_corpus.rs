use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;
use wcore_protocol::commands::ProtocolCommand;
use wcore_protocol::contract::{
    canonical_json, check_contract, generated_artifacts, COMMAND_SPECS, CONTRACT_ROOT, EVENT_SPECS,
    GENERATOR_VERSION,
};

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTRACT_ROOT)
}

#[test]
fn checked_corpus_matches_real_serializers_byte_for_byte() {
    check_contract().expect("checked-in Desktop contract corpus must match the generator");

    let artifacts = generated_artifacts().unwrap();
    for (relative, expected) in artifacts {
        assert_eq!(
            fs::read(root().join(&relative)).unwrap(),
            expected,
            "serialized fixture drift at {relative}"
        );
    }
}

#[test]
fn inventory_is_exactly_eleven_commands_and_thirty_nine_events() {
    assert_eq!(COMMAND_SPECS.len(), 11);
    assert_eq!(EVENT_SPECS.len(), 39);
    assert_eq!(
        COMMAND_SPECS
            .iter()
            .map(|spec| spec.wire_type)
            .collect::<BTreeSet<_>>()
            .len(),
        11
    );
    assert_eq!(
        EVENT_SPECS
            .iter()
            .map(|spec| spec.wire_type)
            .collect::<BTreeSet<_>>()
            .len(),
        39
    );
}

#[test]
fn every_command_fixture_deserializes_through_protocol_command() {
    for entry in fs::read_dir(root().join("commands")).unwrap() {
        let path = entry.unwrap().path();
        let bytes = fs::read(&path).unwrap();
        serde_json::from_slice::<ProtocolCommand>(&bytes)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
    for entry in fs::read_dir(root().join("compat/commands")).unwrap() {
        let path = entry.unwrap().path();
        let bytes = fs::read(&path).unwrap();
        serde_json::from_slice::<ProtocolCommand>(&bytes)
            .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
    }
}

#[test]
fn every_json_artifact_is_canonical_and_lf_terminated() {
    for (relative, bytes) in generated_artifacts().unwrap() {
        if !(relative.ends_with(".json") || relative.ends_with(".schema.json")) {
            continue;
        }
        assert_eq!(bytes.last(), Some(&b'\n'), "{relative} must end in LF");
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(canonical_json(&value).unwrap(), bytes, "{relative}");
    }
}

#[test]
fn manifest_pins_generator_and_all_three_digests() {
    let manifest: Value =
        serde_json::from_slice(&fs::read(root().join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["generator"], GENERATOR_VERSION);
    for key in ["fixture_digest", "schema_digest", "source_inputs_digest"] {
        assert!(
            manifest[key]
                .as_str()
                .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71),
            "manifest {key} must be a prefixed SHA-256 digest"
        );
    }
    assert_eq!(manifest["commands"].as_array().unwrap().len(), 11);
    assert_eq!(manifest["events"].as_array().unwrap().len(), 39);
    assert_eq!(manifest["counts"]["commands"], 11);
    assert_eq!(manifest["counts"]["events"], 39);
    assert_eq!(
        manifest["capabilities"]["contract_negotiation"],
        "available"
    );
    assert_eq!(
        manifest["capabilities"]["anvil_receipts"],
        "publication_bound"
    );
    assert_eq!(
        manifest["capabilities"]["workflow_lifecycle_v1"],
        "available"
    );
    assert_eq!(
        manifest["capabilities"]["effective_execution_policy_revisions"],
        "available"
    );
    let invalidation = manifest["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["type"] == "anvil_receipt_invalidated")
        .expect("authoritative invalidation must be in EVENT_SPECS");
    assert_eq!(invalidation["criticality"], "safety");
    assert_eq!(invalidation["capability"], "anvil_receipts");
}

#[test]
fn producer_complete_schema_keeps_non_desktop_variants_visible() {
    let schema: Value = serde_json::from_slice(
        &fs::read(root().join("schema/producer-complete.schema.json")).unwrap(),
    )
    .unwrap();
    let wire = serde_json::to_string(&schema).unwrap();
    for required in [
        "continue_with_budget",
        "grant_workspace_capability",
        "execution_policy",
        "workflow_started",
        "workflow_node_event",
        "workflow_finished",
        "anvil_receipt",
        "anvil_receipt_invalidated",
    ] {
        assert!(
            wire.contains(required),
            "producer schema omitted {required}"
        );
    }
}

#[test]
fn authority_fixtures_pin_correlated_current_shapes() {
    let ready: Value =
        serde_json::from_slice(&fs::read(root().join("events/ready.json")).unwrap()).unwrap();
    assert_eq!(ready["execution_policy"]["revision"], 0);
    assert_eq!(ready["execution_policy"]["critical"], true);
    assert_eq!(ready["execution_policy"]["contract_version"], "1.0");

    let workflow: Value =
        serde_json::from_slice(&fs::read(root().join("events/workflow_started.json")).unwrap())
            .unwrap();
    assert_eq!(workflow["sequence"], 0);
    assert!(workflow["run_id"].as_str().is_some_and(|id| !id.is_empty()));
    assert!(workflow["event_id"]
        .as_str()
        .is_some_and(|id| !id.is_empty()));

    let receipt: Value =
        serde_json::from_slice(&fs::read(root().join("events/anvil_receipt.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["origin"], "core/anvil");
    assert_eq!(receipt["sequence"], 0);
    assert_eq!(receipt["contract_version"], "1.0");
    assert!(receipt["receipt_body_digest"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71));

    let invalidation: Value = serde_json::from_slice(
        &fs::read(root().join("events/anvil_receipt_invalidated.json")).unwrap(),
    )
    .unwrap();
    assert!(invalidation["invalidation_body_digest"]
        .as_str()
        .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71));
}
