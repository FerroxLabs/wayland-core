use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use serde_json::Value;
use wcore_protocol::commands::ProtocolCommand;
use wcore_protocol::contract::{
    COMMAND_SPECS, CONTRACT_ROOT, ContractCriticality, EVENT_SPECS, GENERATOR_VERSION,
    canonical_json, check_contract, generated_artifacts,
};

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTRACT_ROOT)
}

fn schema_accepts(schema: &Value, instance: &Value) -> bool {
    if schema
        .get("const")
        .is_some_and(|expected| expected != instance)
    {
        return false;
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.iter().any(|expected| expected == instance))
    {
        return false;
    }
    if let Some(expected) = schema.get("type").and_then(Value::as_str) {
        let matches = match expected {
            "null" => instance.is_null(),
            "boolean" => instance.is_boolean(),
            "integer" => instance.as_i64().is_some() || instance.as_u64().is_some(),
            "number" => instance.is_number(),
            "string" => instance.is_string(),
            "array" => instance.is_array(),
            "object" => instance.is_object(),
            _ => false,
        };
        if !matches {
            return false;
        }
    }
    if let Some(minimum) = schema.get("minLength").and_then(Value::as_u64)
        && instance
            .as_str()
            .is_some_and(|value| value.chars().count() < minimum as usize)
    {
        return false;
    }
    if let Some(minimum) = schema.get("minimum").and_then(Value::as_f64)
        && instance.as_f64().is_none_or(|value| value < minimum)
    {
        return false;
    }
    if let Some(maximum) = schema.get("maximum").and_then(Value::as_f64)
        && instance.as_f64().is_none_or(|value| value > maximum)
    {
        return false;
    }
    if schema.get("pattern").and_then(Value::as_str) == Some("^sha256:[0-9a-f]{64}$")
        && !instance.as_str().is_some_and(|value| {
            value.strip_prefix("sha256:").is_some_and(|hex| {
                hex.len() == 64
                    && hex
                        .bytes()
                        .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
            })
        })
    {
        return false;
    }
    if schema.get("pattern").and_then(Value::as_str) == Some("^[0-9a-f]{64}$")
        && !instance.as_str().is_some_and(|value| {
            value.len() == 64
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        })
    {
        return false;
    }
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        let Some(object) = instance.as_object() else {
            return false;
        };
        if required
            .iter()
            .filter_map(Value::as_str)
            .any(|field| !object.contains_key(field))
        {
            return false;
        }
    }
    if let (Some(properties), Some(object)) = (
        schema.get("properties").and_then(Value::as_object),
        instance.as_object(),
    ) {
        if schema.get("additionalProperties") == Some(&Value::Bool(false))
            && object.keys().any(|field| !properties.contains_key(field))
        {
            return false;
        }
        for (field, field_schema) in properties {
            if let Some(value) = object.get(field)
                && !schema_accepts(field_schema, value)
            {
                return false;
            }
        }
    }
    if let (Some(items), Some(values)) = (schema.get("items"), instance.as_array())
        && values.iter().any(|value| !schema_accepts(items, value))
    {
        return false;
    }
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array)
        && !any_of.iter().any(|branch| schema_accepts(branch, instance))
    {
        return false;
    }
    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array)
        && all_of
            .iter()
            .any(|branch| !schema_accepts(branch, instance))
    {
        return false;
    }
    if let Some(condition) = schema.get("if")
        && schema_accepts(condition, instance)
        && schema
            .get("then")
            .is_some_and(|consequence| !schema_accepts(consequence, instance))
    {
        return false;
    }
    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array)
        && one_of
            .iter()
            .filter(|branch| schema_accepts(branch, instance))
            .count()
            != 1
    {
        return false;
    }
    if schema
        .get("not")
        .is_some_and(|forbidden| schema_accepts(forbidden, instance))
    {
        return false;
    }
    true
}

fn generated_json(relative: &str) -> Value {
    let artifacts = generated_artifacts().unwrap();
    serde_json::from_slice(
        artifacts
            .get(relative)
            .unwrap_or_else(|| panic!("missing generated artifact {relative}")),
    )
    .unwrap()
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
fn inventory_is_exactly_sixteen_commands_and_forty_seven_events() {
    assert_eq!(COMMAND_SPECS.len(), 16);
    assert_eq!(EVENT_SPECS.len(), 47);
    assert_eq!(
        COMMAND_SPECS
            .iter()
            .map(|spec| spec.wire_type)
            .collect::<BTreeSet<_>>()
            .len(),
        16
    );
    assert_eq!(
        EVENT_SPECS
            .iter()
            .map(|spec| spec.wire_type)
            .collect::<BTreeSet<_>>()
            .len(),
        47
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
    assert_eq!(manifest["contract"]["major"], 1);
    assert_eq!(manifest["contract"]["minor"], 5);
    assert_eq!(manifest["commands"].as_array().unwrap().len(), 16);
    assert_eq!(manifest["events"].as_array().unwrap().len(), 47);
    assert_eq!(manifest["counts"]["commands"], 16);
    assert_eq!(manifest["counts"]["events"], 47);
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
        manifest["capabilities"]["semantic_failover_receipts"],
        "available"
    );
    assert_eq!(
        manifest["capabilities"]["effective_execution_policy_revisions"],
        "available"
    );
    assert_eq!(manifest["capabilities"]["turn_recovery_v1"], "available");
    assert_eq!(
        manifest["capabilities"]["runtime_diagnostics_v1"],
        "available"
    );
    assert_eq!(manifest["subcontracts"]["runtime_diagnostics"], "1.0");
    assert_eq!(manifest["subcontracts"]["turn_recovery"], "1.0");
    let invalidation = manifest["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["type"] == "anvil_receipt_invalidated")
        .expect("authoritative invalidation must be in EVENT_SPECS");
    assert_eq!(invalidation["criticality"], "safety");
    assert_eq!(invalidation["capability"], "anvil_receipts");
    let child_event = manifest["events"]
        .as_array()
        .unwrap()
        .iter()
        .find(|event| event["type"] == "sub_agent_event")
        .expect("child terminal evidence must be in EVENT_SPECS");
    assert_eq!(child_event["criticality"], "safety");
}

#[test]
fn manifest_criticality_uses_only_the_normative_typed_vocabulary() {
    let manifest: Value =
        serde_json::from_slice(&fs::read(root().join("manifest.json")).unwrap()).unwrap();
    for entry in manifest["commands"]
        .as_array()
        .unwrap()
        .iter()
        .chain(manifest["events"].as_array().unwrap())
    {
        let criticality: ContractCriticality = serde_json::from_value(entry["criticality"].clone())
            .unwrap_or_else(|error| {
                panic!("{} has non-normative criticality: {error}", entry["type"])
            });
        assert!(matches!(
            criticality,
            ContractCriticality::Required
                | ContractCriticality::Safety
                | ContractCriticality::Observational
        ));
    }
}

#[test]
fn event_schema_distinguishes_correlated_and_legacy_child_shapes() {
    let schema: Value =
        serde_json::from_slice(&fs::read(root().join("schema/core-event.schema.json")).unwrap())
            .unwrap();
    let child_variants: Vec<&Value> = schema["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|branch| branch["properties"]["type"]["const"] == "sub_agent_event")
        .collect();
    assert_eq!(child_variants.len(), 2);

    let correlated = child_variants
        .iter()
        .find(|branch| {
            branch["required"]
                .as_array()
                .is_some_and(|required| required.iter().any(|field| field == "run_id"))
        })
        .expect("correlated child schema missing");
    assert!(correlated["required"].as_array().unwrap().len() > 4);

    let legacy = child_variants
        .iter()
        .find(|branch| branch.get("not").is_some())
        .expect("legacy compatibility schema missing");
    assert_eq!(
        legacy["required"],
        serde_json::json!(["type", "parent_call_id", "agent_name", "inner"])
    );
}

#[test]
fn generated_schemas_reject_malformed_authority_types_and_enums() {
    let event_schema = generated_json("schema/core-event.schema.json");
    let mut ready = generated_json("events/ready.json");
    assert!(schema_accepts(&event_schema, &ready));
    ready["contract"]["major"] = Value::String("one".into());
    assert!(!schema_accepts(&event_schema, &ready));
    ready["contract"]["major"] = Value::from(1_u64);
    ready["execution_policy"]["policy"]["sandbox"] = Value::String("optional".into());
    assert!(!schema_accepts(&event_schema, &ready));

    let mut policy = generated_json("events/execution_policy.json");
    assert!(schema_accepts(&event_schema, &policy));
    policy["policy"]["source"] = Value::String("desktop_claim".into());
    assert!(!schema_accepts(&event_schema, &policy));

    let mut finished = generated_json("events/workflow_finished.json");
    assert!(schema_accepts(&event_schema, &finished));

    finished["sequence"] = Value::String("three".into());
    assert!(!schema_accepts(&event_schema, &finished));
    finished["sequence"] = Value::from(4_u64);
    finished["terminal_state"] = Value::String("paused".into());
    assert!(!schema_accepts(&event_schema, &finished));
    finished["terminal_state"] = Value::String("failed".into());
    finished["succeeded"] = Value::Bool(false);
    finished["failure"] = serde_json::json!({
        "code": "stage_failed",
        "message": "failed",
        "retryable": "no"
    });
    assert!(!schema_accepts(&event_schema, &finished));

    let mut failover = generated_json("events/provider_failover_receipt.json");
    assert!(schema_accepts(&event_schema, &failover));
    failover["receipt"]["candidates"][0]["disposition"] =
        serde_json::json!({"Err": "tools_unsupported"});
    failover["receipt"]["candidates"][0]["failure_reason"] =
        Value::String("context_overflow".into());
    failover["receipt"]["candidates"][0]["cooldown_reason"] = Value::String("rate_limit".into());
    failover["receipt"]["candidates"][0]["pricing"]["age_seconds"] = Value::from(12_u64);
    failover["receipt"]["selected_provider"] = Value::Null;
    failover["receipt"]["selected_model"] = Value::Null;
    assert!(
        schema_accepts(&event_schema, &failover),
        "receipt schema must accept typed rejection and reason evidence"
    );
    failover["receipt"]["candidates"][0]["disposition"] =
        serde_json::json!({"Err": "silently_retry_anywhere"});
    assert!(
        !schema_accepts(&event_schema, &failover),
        "receipt schema must reject unknown candidate dispositions"
    );

    let command_schema = generated_json("schema/host-command.schema.json");
    let mut message = generated_json("commands/message.json");
    assert!(schema_accepts(&command_schema, &message));
    message["msg_id"] = Value::from(7_u64);
    assert!(!schema_accepts(&command_schema, &message));

    let mut resume = generated_json("commands/resume_turn.json");
    assert!(schema_accepts(&command_schema, &resume));
    resume["recovery_version"] = Value::from(2_u64);
    assert!(!schema_accepts(&command_schema, &resume));
    resume["recovery_version"] = Value::from(1_u64);
    resume["action"] = Value::String("claim_effect_succeeded".into());
    assert!(!schema_accepts(&command_schema, &resume));
    resume["action"] = Value::String("reconcile".into());
    resume["cursor"]["journal_digest"] = Value::String("sha256:not-a-digest".into());
    assert!(!schema_accepts(&command_schema, &resume));
    resume["cursor"]["journal_digest"] = Value::String("6".repeat(64));
    resume["future_authority"] = Value::Bool(true);
    assert!(
        !schema_accepts(&command_schema, &resume),
        "closed recovery command schemas must reject unknown properties"
    );

    let mut snapshot = generated_json("events/session_recovery_snapshot.json");
    assert!(schema_accepts(&event_schema, &snapshot));
    snapshot["lifecycle"] = Value::String("silently_restarted".into());
    assert!(!schema_accepts(&event_schema, &snapshot));
    snapshot["lifecycle"] = Value::String("reconciliation_required".into());
    snapshot["state_digest"] = Value::String(format!("sha256:{}", "a".repeat(64)));
    assert!(
        !schema_accepts(&event_schema, &snapshot),
        "recovery state digests are raw lowercase hex, not evidence digests"
    );

    let mut replay = generated_json("events/session_recovery_replay.json");
    assert!(schema_accepts(&event_schema, &replay));
    replay["items"][0]["kind"] = Value::String("provider_payload".into());
    assert!(!schema_accepts(&event_schema, &replay));

    let mut diagnostics = generated_json("events/runtime_diagnostics_snapshot.json");
    assert!(schema_accepts(&event_schema, &diagnostics));
    diagnostics["snapshot"]["config_sources"][0]["precedence"] = Value::from(-1);
    assert!(!schema_accepts(&event_schema, &diagnostics));
    diagnostics["snapshot"]["config_sources"][0]["precedence"] = Value::from(10_u64);
    diagnostics["snapshot"]["mcp_servers"][0]["tool_count"] = Value::from(4_294_967_296_u64);
    assert!(!schema_accepts(&event_schema, &diagnostics));

    let artifacts = generated_artifacts().unwrap();
    let lifecycle = std::str::from_utf8(
        artifacts
            .get("adversarial/workflow/valid-lifecycle.jsonl")
            .unwrap(),
    )
    .unwrap()
    .lines()
    .map(|line| serde_json::from_str::<Value>(line).unwrap())
    .collect::<Vec<_>>();
    let mut child_terminal = lifecycle
        .into_iter()
        .find(|event| event["terminal_state"] == "succeeded")
        .expect("canonical workflow must include a successful child terminal");
    assert!(schema_accepts(&event_schema, &child_terminal));
    child_terminal["inner"]
        .as_object_mut()
        .unwrap()
        .remove("msg_id");
    assert!(!schema_accepts(&event_schema, &child_terminal));
    child_terminal["inner"]["msg_id"] = Value::String(String::new());
    assert!(!schema_accepts(&event_schema, &child_terminal));
}

#[test]
fn producer_complete_schema_keeps_non_desktop_variants_visible() {
    let schema = generated_json("schema/producer-complete.schema.json");
    let wire = serde_json::to_string(&schema).unwrap();
    for required in [
        "continue_with_budget",
        "grant_workspace_capability",
        "session_resync",
        "resume_turn",
        "execution_policy",
        "workflow_started",
        "workflow_node_event",
        "workflow_finished",
        "anvil_receipt",
        "anvil_receipt_invalidated",
        "session_recovery_snapshot",
        "session_recovery_replay",
        "session_recovery_unavailable",
        "turn_recovery_lifecycle",
    ] {
        assert!(
            wire.contains(required),
            "producer schema omitted {required}"
        );
    }

    let command = generated_json("commands/approval_resume.json");
    let event = generated_json("events/approval_resume.json");
    assert!(schema_accepts(&schema, &command));
    assert!(schema_accepts(&schema, &event));
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
    assert!(
        workflow["event_id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );

    let receipt: Value =
        serde_json::from_slice(&fs::read(root().join("events/anvil_receipt.json")).unwrap())
            .unwrap();
    assert_eq!(receipt["origin"], "core/anvil");
    assert_eq!(receipt["sequence"], 0);
    assert_eq!(receipt["contract_version"], "1.0");
    assert!(
        receipt["receipt_body_digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71)
    );

    let invalidation: Value = serde_json::from_slice(
        &fs::read(root().join("events/anvil_receipt_invalidated.json")).unwrap(),
    )
    .unwrap();
    assert!(
        invalidation["invalidation_body_digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71)
    );
}
