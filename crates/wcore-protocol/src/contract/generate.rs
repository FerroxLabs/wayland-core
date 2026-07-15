use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::commands::ProtocolCommand;

use super::ContractResult;
use super::canonical::{canonical_json, digest_named_bytes};
use super::spec::{
    COMMAND_SPECS, EVENT_SPECS, PRODUCER_COMMAND_TYPES, PRODUCER_EVENT_TYPES, SOURCE_INPUTS,
    WireSpec, command_fixture_values, compatibility_event_values, event_fixture_values,
};

pub const CONTRACT_NAME: &str = "wayland-desktop-core";
pub const GENERATOR_VERSION: &str = "wcore-desktop-contract-gen/1";
pub const CONTRACT_ROOT: &str = "contracts/desktop/v1";

const DEFERRED: &str = r#"# Deferred Desktop contract adversarial cases

This v1.0 corpus foundation records the current 11-command and 33-event wire.
It does not invent authority that the current protocol does not carry.

- `unknown_critical_fail_closed`: deferred until events carry top-level
  `critical` and `contract_version` and Desktop has a contract-aware decoder.
- `version_mismatch_handshake`: deferred until `ready` advertises a versioned
  contract descriptor and schema digest.
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

fn fixtures_digest(artifacts: &BTreeMap<String, Vec<u8>>) -> String {
    digest_named_bytes(artifacts.iter().filter_map(|(path, bytes)| {
        let included = ["commands/", "events/", "compat/", "adversarial/"]
            .iter()
            .any(|prefix| path.starts_with(prefix));
        included.then_some((path.as_str(), bytes.as_slice()))
    }))
}

fn schemas_digest(artifacts: &BTreeMap<String, Vec<u8>>) -> String {
    digest_named_bytes(artifacts.iter().filter_map(|(path, bytes)| {
        path.starts_with("schema/")
            .then_some((path.as_str(), bytes.as_slice()))
    }))
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
        "adversarial/events/unknown-critical.deferred.jsonl".into(),
        b"{\"contract_version\":\"1.0\",\"critical\":true,\"type\":\"future_authority\"}\n"
            .to_vec(),
    );
    artifacts.insert(
        "adversarial/events/unknown-noncritical.jsonl".into(),
        b"{\"payload\":{},\"type\":\"future_observation\"}\n".to_vec(),
    );
    artifacts.insert(
        "adversarial/events/version-mismatch.deferred.jsonl".into(),
        b"{\"contract\":{\"generator\":\"wcore-desktop-contract-gen/1\",\"major\":2,\"minor\":0,\"name\":\"wayland-desktop-core\",\"schema_digest\":\"sha256:unsupported\"},\"type\":\"ready\"}\n".to_vec(),
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

    let fixture_digest = fixtures_digest(&artifacts);
    let schema_digest = schemas_digest(&artifacts);
    let source_inputs_digest = source_digest()?;
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
        "capabilities": {
            "anvil_receipts": "unavailable",
            "browser_events": "shape_only",
            "contract_negotiation": "unavailable",
            "cua_events": "shape_only",
            "effective_execution_policy_revisions": "unavailable",
            "host_delegated_delivery": "available",
            "plugin_events": "shape_only",
            "workflow_lifecycle_v1": "unavailable"
        },
        "commands": specs_manifest(COMMAND_SPECS),
        "contract": {"major": 1, "minor": 0, "name": CONTRACT_NAME},
        "deferred_adversarial": [
            "unknown_critical_fail_closed",
            "version_mismatch_handshake",
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
        fixtures_digest(&artifacts),
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
