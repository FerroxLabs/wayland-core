//! FerroxLabs/wayland#1098 — the SHIPPED contract for `render_artifact`.
//!
//! Everything here reads the checked-in corpus from DISK, never
//! `generated_artifacts()`. That is the whole point: a test that regenerates
//! the corpus and then validates against what it just generated cannot catch a
//! stale shipped file, and a stale shipped file is exactly how
//! `host-command.schema.json` came to FORBID the `always_path` scope that the
//! same release advertised as available. A capability the schema forbids is not
//! a capability, so the file a Desktop host actually validates against is the
//! file these tests open.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use wcore_protocol::contract::CONTRACT_ROOT;
use wcore_protocol::events::RenderMime;

fn corpus() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(CONTRACT_ROOT)
}

fn shipped(relative: &str) -> Value {
    let path = corpus().join(relative);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("the corpus must ship {}: {e}", path.display()));
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{} must be JSON: {e}", path.display()))
}

/// Deliberately NARROWER than the corpus test's full walker: it covers the
/// keywords this branch actually uses (`const`, `enum`, `type`, `required`,
/// `properties`, `additionalProperties`, `maxLength`, `not`) plus `oneOf`
/// exactly-one selection, which is what makes "the schema accepts the frame"
/// mean "accepts it through the render branch and no other".
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
    if let Some(maximum) = schema.get("maxLength").and_then(Value::as_u64)
        && instance
            .as_str()
            .is_some_and(|value| value.chars().count() > maximum as usize)
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
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array)
        && !any_of.iter().any(|branch| schema_accepts(branch, instance))
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

fn render_branch() -> Value {
    let schema = shipped("schema/core-event.schema.json");
    schema["oneOf"]
        .as_array()
        .expect("core-event.schema.json must be a oneOf union")
        .iter()
        .find(|branch| branch["properties"]["type"]["const"] == json!("render_artifact"))
        .cloned()
        .expect(
            "the SHIPPED core-event schema must carry a render_artifact branch — a host that \
             validates incoming frames against this file would otherwise reject the event we \
             advertise as available",
        )
}

/// The published schema the Desktop host validates against must accept the
/// published fixture, through the render branch and no other.
#[test]
fn the_shipped_core_event_schema_accepts_the_render_frame() {
    let schema = shipped("schema/core-event.schema.json");
    for fixture in [
        "events/render_artifact.json",
        "compat/events/render_artifact.truncated.json",
    ] {
        let frame = shipped(fixture);
        assert!(
            schema_accepts(&schema, &frame),
            "{fixture} is rejected by the shipped core-event schema"
        );
    }
}

/// NEGATIVE CONTROL for the closed vocabulary. Without this the acceptance
/// test above would pass against a schema whose `mime` was plain `string`, and
/// would be measuring nothing about the vocabulary being closed.
#[test]
fn the_shipped_schema_rejects_an_undeclared_mime() {
    let branch = render_branch();
    let mut frame = shipped("events/render_artifact.json");
    frame["mime"] = json!("application/x-shellscript");
    assert!(
        !schema_accepts(&branch, &frame),
        "the shipped schema must close the mime vocabulary"
    );
}

/// NEGATIVE CONTROL for the criticality pin. A producer that emitted
/// `critical: true` would tell an older host to disconnect rather than ignore
/// — the schema refuses to describe that frame as valid.
#[test]
fn the_shipped_schema_pins_the_criticality_to_false() {
    let branch = render_branch();
    let mut frame = shipped("events/render_artifact.json");
    frame["critical"] = json!(true);
    assert!(
        !schema_accepts(&branch, &frame),
        "critical must be pinned to the literal false"
    );
    frame["critical"] = json!(false);
    assert!(schema_accepts(&branch, &frame));
}

/// The shipped MIME enum must be the code's enum. A schema that drifted from
/// `RenderMime` would either reject our own frames or admit a value nothing
/// can render.
#[test]
fn the_shipped_schema_enum_is_the_code_enum() {
    let branch = render_branch();
    let published = branch["properties"]["mime"]["enum"]
        .as_array()
        .expect("mime must publish an enum")
        .iter()
        .map(|v| v.as_str().expect("enum values are strings"))
        .collect::<Vec<_>>();
    assert_eq!(published.as_slice(), RenderMime::all());
}

/// A host feature-detects from `ready.contract.capabilities`, not from a file
/// it never receives — so both the manifest and the `ready` fixture are
/// asserted, not just the manifest.
#[test]
fn the_shipped_manifest_and_ready_fixture_advertise_render_artifact_v1() {
    let manifest = shipped("manifest.json");
    assert_eq!(
        manifest["capabilities"]["render_artifact_v1"],
        json!("available"),
        "manifest.json must advertise the capability"
    );
    assert!(
        manifest["events"]
            .as_array()
            .expect("manifest must list events")
            .iter()
            .any(|entry| entry["type"] == json!("render_artifact")),
        "manifest.json must declare the event, or a corpus-only host never learns it"
    );

    let ready = shipped("events/ready.json");
    assert_eq!(
        ready["contract"]["capabilities"]["render_artifact_v1"],
        json!("available"),
        "ready.contract.capabilities is what a host actually reads at handshake"
    );
    assert_eq!(
        ready["contract"]["major"],
        json!(1),
        "an additive event must not move the major"
    );
    assert_eq!(ready["contract"]["minor"], json!(16));
}

/// The three outcomes a corpus-only host classifies a frame into. Mirrors
/// `desktop_contract_corpus_only_host.rs`, which is the consumer-side
/// implementation of the documented W0 rule.
#[derive(Debug, PartialEq, Eq)]
enum HostOutcome {
    Accepted,
    DroppedUnknownNonCritical,
    HardError(&'static str),
}

fn observe(known: &[String], frame: &Value) -> HostOutcome {
    let event_type = frame["type"].as_str().expect("a frame carries a type");
    if known.iter().any(|k| k == event_type) {
        return HostOutcome::Accepted;
    }
    match frame.get("critical").and_then(Value::as_bool) {
        Some(false) => HostOutcome::DroppedUnknownNonCritical,
        Some(true) => HostOutcome::HardError("unknown_critical_event"),
        None => HostOutcome::HardError("unknown_criticality"),
    }
}

/// The DoD clause: a host that does not know the event ignores it safely.
///
/// GUARD: the `critical: NonCritical` field on the variant. Remove it and the
/// outcome becomes `HardError("unknown_criticality")` — an older host does not
/// skip the render, it tears the connection down.
#[test]
fn a_host_pinned_to_the_previous_corpus_drops_render_artifact_instead_of_hard_erroring() {
    let manifest = shipped("manifest.json");
    let pinned: Vec<String> = manifest["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["type"].as_str().unwrap().to_owned())
        .filter(|t| t != "render_artifact")
        .collect();
    assert!(
        !pinned.is_empty(),
        "a host that knows nothing would trivially drop everything"
    );

    assert_eq!(
        observe(&pinned, &shipped("events/render_artifact.json")),
        HostOutcome::DroppedUnknownNonCritical
    );
    assert_eq!(
        observe(
            &pinned,
            &shipped("compat/events/render_artifact.truncated.json")
        ),
        HostOutcome::DroppedUnknownNonCritical
    );
}

/// NEGATIVE CONTROL. Without it the drop test above degenerates into "accept
/// everything" and measures nothing.
#[test]
fn a_genuinely_unknown_type_still_hard_errors() {
    let pinned = vec!["ready".to_string()];
    assert_eq!(
        observe(
            &pinned,
            &json!({"type": "future_authority", "critical": true})
        ),
        HostOutcome::HardError("unknown_critical_event")
    );
    assert_eq!(
        observe(&pinned, &json!({"type": "future_observation"})),
        HostOutcome::HardError("unknown_criticality")
    );
}
