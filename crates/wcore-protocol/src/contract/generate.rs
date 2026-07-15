use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::commands::ProtocolCommand;

use super::ContractResult;
use super::canonical::{canonical_json, digest_named_bytes};
use super::observation::{ContractCapabilityStatus, ContractDescriptor};
use super::spec::{
    COMMAND_SPECS, EVENT_SPECS, PRODUCER_COMMAND_TYPES, PRODUCER_EVENT_TYPES, SOURCE_INPUTS,
    WireSpec, command_fixture_values, compatibility_event_values, event_fixture_values,
};

pub const CONTRACT_NAME: &str = "wayland-desktop-core";
pub const GENERATOR_VERSION: &str = "wcore-desktop-contract-gen/1";
pub const CONTRACT_ROOT: &str = "contracts/desktop/v1";

const DEFERRED: &str = r#"# Deferred Desktop contract adversarial cases

This v1.0 corpus records the current producer wire. Contract negotiation,
unknown-critical rejection, and unknown-noncritical dropping are live and
proved by serialized replay through the reference host observer.

- `ordering_duplicate_terminal_reducer`: deferred because ordinary current
  events have no producer event ID or monotonic sequence.
- `effective_execution_policy_revisions`: deferred; the current event is a
  launch snapshot without revision/change semantics.
- `workflow_node_child_lifecycle`: deferred; current workflow IDs collide on
  repeated runs and there is no node event, run ID, event ID, or sequence.
- `anvil_origin_replay_mutation_staleness`: deferred and unavailable. The
  legacy receipt is not promoted by this corpus.

Malformed command fixtures and the current unknown-type behavior are proved by
`desktop_contract_adversarial.rs`. Browser, CUA, and plugin event fixtures are
shape-only because no production emitter is proven at this source baseline.
"#;

fn schema_for(specs: &[WireSpec], title: &str) -> Value {
    let one_of = specs
        .iter()
        .map(|spec| {
            json!({
                "additionalProperties": true,
                "properties": {
                    "type": {"const": spec.wire_type}
                },
                "required": spec.required,
                "type": "object"
            })
        })
        .collect::<Vec<_>>();
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "oneOf": one_of,
        "title": title
    })
}

fn producer_complete_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "oneOf": [
            {
                "additionalProperties": true,
                "properties": {"type": {"enum": PRODUCER_COMMAND_TYPES}},
                "required": ["type"],
                "title": "Core ProtocolCommand",
                "type": "object"
            },
            {
                "additionalProperties": true,
                "properties": {"type": {"enum": PRODUCER_EVENT_TYPES}},
                "required": ["type"],
                "title": "Core ProtocolEvent",
                "type": "object"
            }
        ],
        "title": "Complete current Core producer inventory"
    })
}

fn contract_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTRACT_ROOT)
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("wcore-protocol must remain inside the workspace crates directory")
        .to_path_buf()
}

fn source_digest() -> ContractResult<String> {
    let root = workspace_root();
    let mut sources = Vec::with_capacity(SOURCE_INPUTS.len());
    for relative in SOURCE_INPUTS {
        let bytes = fs::read(root.join(relative))?;
        sources.push((*relative, bytes));
    }
    Ok(digest_named_bytes(
        sources
            .iter()
            .map(|(path, bytes)| (*path, bytes.as_slice())),
    ))
}

fn specs_manifest(specs: &[WireSpec]) -> Vec<Value> {
    specs
        .iter()
        .map(|spec| {
            json!({
                "capability": spec.capability,
                "correlation": spec.correlation,
                "criticality": spec.criticality,
                "path": spec.path,
                "type": spec.wire_type
            })
        })
        .collect()
}

fn fixtures_digest(artifacts: &BTreeMap<String, Vec<u8>>) -> ContractResult<String> {
    let mut normalized = Vec::new();
    for (path, bytes) in artifacts {
        let included = ["commands/", "events/", "compat/", "adversarial/"]
            .iter()
            .any(|prefix| path.starts_with(prefix));
        if !included {
            continue;
        }
        let mut bytes = bytes.clone();
        if path == "events/ready.json"
            || path == "adversarial/events/version-mismatch.jsonl"
            || path == "adversarial/events/schema-mismatch.jsonl"
            || path == "adversarial/events/fixture-mismatch.jsonl"
        {
            let mut value: Value = serde_json::from_slice(&bytes)?;
            if let Some(fixture_digest) = value
                .get_mut("contract")
                .and_then(Value::as_object_mut)
                .and_then(|contract| contract.get_mut("fixture_digest"))
            {
                *fixture_digest = Value::String(format!("sha256:{}", "0".repeat(64)));
                bytes = canonical_json(&value)?;
            }
        }
        normalized.push((path.as_str(), bytes));
    }
    Ok(digest_named_bytes(
        normalized
            .iter()
            .map(|(path, bytes)| (*path, bytes.as_slice())),
    ))
}

fn schemas_digest(artifacts: &BTreeMap<String, Vec<u8>>) -> String {
    digest_named_bytes(artifacts.iter().filter_map(|(path, bytes)| {
        path.starts_with("schema/")
            .then_some((path.as_str(), bytes.as_slice()))
    }))
}

fn contract_capabilities() -> BTreeMap<String, ContractCapabilityStatus> {
    BTreeMap::from([
        (
            "anvil_receipts".into(),
            ContractCapabilityStatus::Unavailable,
        ),
        ("browser_events".into(), ContractCapabilityStatus::ShapeOnly),
        (
            "contract_negotiation".into(),
            ContractCapabilityStatus::Available,
        ),
        ("cua_events".into(), ContractCapabilityStatus::ShapeOnly),
        (
            "effective_execution_policy_revisions".into(),
            ContractCapabilityStatus::Unavailable,
        ),
        (
            "host_delegated_delivery".into(),
            ContractCapabilityStatus::Available,
        ),
        ("plugin_events".into(), ContractCapabilityStatus::ShapeOnly),
        (
            "workflow_lifecycle_v1".into(),
            ContractCapabilityStatus::Unavailable,
        ),
    ])
}

fn descriptor(
    fixture_digest: String,
    schema_digest: String,
    source_inputs_digest: String,
    capabilities: BTreeMap<String, ContractCapabilityStatus>,
) -> ContractDescriptor {
    ContractDescriptor {
        name: CONTRACT_NAME.into(),
        major: 1,
        minor: 0,
        generator: GENERATOR_VERSION.into(),
        fixture_digest,
        schema_digest,
        source_inputs_digest,
        capabilities,
    }
}

fn insert_negotiation_fixtures(
    artifacts: &mut BTreeMap<String, Vec<u8>>,
    descriptor: &ContractDescriptor,
) -> ContractResult<()> {
    let ready = artifacts
        .get("events/ready.json")
        .ok_or_else(|| std::io::Error::other("canonical Ready fixture is missing"))?;
    let mut ready: Value = serde_json::from_slice(ready)?;
    ready["contract"] = serde_json::to_value(descriptor)?;
    artifacts.insert("events/ready.json".into(), canonical_json(&ready)?);

    let mut unsupported_major = descriptor.clone();
    unsupported_major.major += 1;
    artifacts.insert(
        "adversarial/events/version-mismatch.jsonl".into(),
        canonical_json(&json!({"contract": unsupported_major, "type": "ready"}))?,
    );

    let mut schema_mismatch = descriptor.clone();
    schema_mismatch.schema_digest = format!("sha256:{}", "f".repeat(64));
    artifacts.insert(
        "adversarial/events/schema-mismatch.jsonl".into(),
        canonical_json(&json!({"contract": schema_mismatch, "type": "ready"}))?,
    );

    let mut fixture_mismatch = descriptor.clone();
    fixture_mismatch.fixture_digest = format!("sha256:{}", "f".repeat(64));
    artifacts.insert(
        "adversarial/events/fixture-mismatch.jsonl".into(),
        canonical_json(&json!({"contract": fixture_mismatch, "type": "ready"}))?,
    );
    Ok(())
}

/// Regenerate every tracked contract artifact in memory.
pub fn generated_artifacts() -> ContractResult<BTreeMap<String, Vec<u8>>> {
    let mut artifacts = BTreeMap::new();

    for (path, value) in command_fixture_values() {
        let _: ProtocolCommand = serde_json::from_value(value.clone())?;
        artifacts.insert(path, canonical_json(&value)?);
    }
    for (path, event) in event_fixture_values()
        .into_iter()
        .chain(compatibility_event_values())
    {
        artifacts.insert(path, canonical_json(&serde_json::to_value(event)?)?);
    }

    artifacts.insert(
        "adversarial/commands/invalid-json.jsonl".into(),
        b"{not-json}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/missing-type.jsonl".into(),
        b"{\"msg_id\":\"msg-001\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/non-object.jsonl".into(),
        b"[]\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/non-string-type.jsonl".into(),
        b"{\"type\":1}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/unknown-type.jsonl".into(),
        b"{\"type\":\"future_command\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/commands/wrong-required-field.jsonl".into(),
        b"{\"content\":\"hello\",\"msg_id\":7,\"type\":\"message\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/events/unknown-critical.jsonl".into(),
        b"{\"critical\":true,\"type\":\"future_authority\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/events/unknown-noncritical.jsonl".into(),
        b"{\"critical\":false,\"payload\":{},\"type\":\"future_observation\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/events/unknown-criticality.jsonl".into(),
        b"{\"payload\":{},\"type\":\"future_unclassified\"}\n".to_vec(),
    );

    artifacts.insert(
        "schema/host-command.schema.json".into(),
        canonical_json(&schema_for(
            COMMAND_SPECS,
            "Desktop-consumed HostCommand v1",
        ))?,
    );
    artifacts.insert(
        "schema/core-event.schema.json".into(),
        canonical_json(&schema_for(EVENT_SPECS, "Desktop-consumed CoreEvent v1"))?,
    );
    artifacts.insert(
        "schema/producer-complete.schema.json".into(),
        canonical_json(&producer_complete_schema())?,
    );
    artifacts.insert("DEFERRED.md".into(), DEFERRED.as_bytes().to_vec());

    let schema_digest = schemas_digest(&artifacts);
    let source_inputs_digest = source_digest()?;
    let capabilities = contract_capabilities();
    let provisional = descriptor(
        format!("sha256:{}", "0".repeat(64)),
        schema_digest.clone(),
        source_inputs_digest.clone(),
        capabilities.clone(),
    );
    insert_negotiation_fixtures(&mut artifacts, &provisional)?;
    let fixture_digest = fixtures_digest(&artifacts)?;
    let final_descriptor = descriptor(
        fixture_digest.clone(),
        schema_digest.clone(),
        source_inputs_digest.clone(),
        capabilities.clone(),
    );
    insert_negotiation_fixtures(&mut artifacts, &final_descriptor)?;
    debug_assert_eq!(fixture_digest, fixtures_digest(&artifacts)?);
    let fixture_inventory = artifacts
        .keys()
        .filter(|path| {
            ["commands/", "events/", "compat/", "adversarial/"]
                .iter()
                .any(|prefix| path.starts_with(prefix))
        })
        .cloned()
        .collect::<Vec<_>>();
    let manifest = json!({
        "capabilities": capabilities,
        "commands": specs_manifest(COMMAND_SPECS),
        "contract": {"major": 1, "minor": 0, "name": CONTRACT_NAME},
        "deferred_adversarial": [
            "ordering_duplicate_terminal_reducer",
            "effective_execution_policy_revisions",
            "workflow_node_child_lifecycle",
            "anvil_origin_replay_mutation_staleness"
        ],
        "events": specs_manifest(EVENT_SPECS),
        "fixture_digest": fixture_digest,
        "fixture_inventory": fixture_inventory,
        "generator": GENERATOR_VERSION,
        "schema_digest": schema_digest,
        "source_inputs": SOURCE_INPUTS,
        "source_inputs_digest": source_inputs_digest
    });
    artifacts.insert("manifest.json".into(), canonical_json(&manifest)?);

    Ok(artifacts)
}

pub fn write_contract() -> ContractResult<()> {
    let root = contract_root();
    let artifacts = generated_artifacts()?;
    let expected = artifacts.keys().cloned().collect::<BTreeSet<_>>();

    if root.exists() {
        for path in all_relative_files(&root)? {
            if !expected.contains(&path) {
                fs::remove_file(root.join(path))?;
            }
        }
    }
    for (relative, bytes) in artifacts {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)?;
    }
    Ok(())
}

pub fn manifest_digests() -> ContractResult<(String, String, String)> {
    let artifacts = generated_artifacts()?;
    Ok((
        fixtures_digest(&artifacts)?,
        schemas_digest(&artifacts),
        source_digest()?,
    ))
}

pub(crate) fn contract_path() -> PathBuf {
    contract_root()
}

pub(crate) fn all_relative_files(root: &Path) -> ContractResult<BTreeSet<String>> {
    fn visit(root: &Path, current: &Path, files: &mut BTreeSet<String>) -> ContractResult<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, files)?;
            } else if path.is_file() {
                files.insert(
                    path.strip_prefix(root)?
                        .to_string_lossy()
                        .replace('\\', "/"),
                );
            }
        }
        Ok(())
    }

    let mut files = BTreeSet::new();
    if root.exists() {
        visit(root, root, &mut files)?;
    }
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contract::{HostContractObserver, HostObservation, HostObservationError};

    #[test]
    fn generated_negotiation_fixtures_replay_without_digest_recursion() {
        let artifacts = generated_artifacts().unwrap();
        let ready = artifacts.get("events/ready.json").unwrap();
        let ready_value: Value = serde_json::from_slice(ready).unwrap();
        let expected: ContractDescriptor =
            serde_json::from_value(ready_value["contract"].clone()).unwrap();
        let mut observer = HostContractObserver::new(expected.clone());
        assert_eq!(
            observer.observe_json_line(ready),
            Ok(HostObservation::Negotiated(expected.clone()))
        );

        assert!(matches!(
            observer.observe_json_line(
                artifacts
                    .get("adversarial/events/unknown-noncritical.jsonl")
                    .unwrap()
            ),
            Ok(HostObservation::DroppedUnknownNonCritical { .. })
        ));
        assert!(matches!(
            observer.observe_json_line(
                artifacts
                    .get("adversarial/events/unknown-critical.jsonl")
                    .unwrap()
            ),
            Err(HostObservationError::UnknownCriticalEvent { .. })
        ));

        let manifest: Value =
            serde_json::from_slice(artifacts.get("manifest.json").unwrap()).unwrap();
        assert_eq!(manifest["fixture_digest"], expected.fixture_digest);
        assert_eq!(
            manifest["capabilities"]["contract_negotiation"],
            "available"
        );
        assert_eq!(
            fixtures_digest(&artifacts).unwrap(),
            expected.fixture_digest
        );
    }
}
