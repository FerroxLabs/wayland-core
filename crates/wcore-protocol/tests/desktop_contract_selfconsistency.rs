//! Self-consistency gates for the published Desktop contract (C2).
//!
//! `desktop_contract_corpus.rs` compares JSON fixtures to JSON fixtures. It never
//! points the *published schema* at the *published corpus*, and it never asks
//! whether a `ProtocolEvent` variant has anywhere to land. Five contract
//! regenerations passed while `core-event.schema.json` REJECTED
//! `events/goal_snapshot.json`, and while seven variants that production emits had
//! no payload schema in any published artifact.
//!
//! These gates close both holes. Every one of them is proved to be able to FAIL and
//! able to PASS — a gate stuck in either state measures nothing (LANE-BRIEF §3b-iii)
//! — by running the detector over a hand-built positive and a hand-built negative in
//! the same test.
//!
//! Note the gates read the **generator's in-memory output**, not the committed bytes.
//! `checked_corpus_matches_real_serializers_byte_for_byte` already owns the
//! bytes-on-disk question, and duplicating it here would just mean two tests report
//! one regeneration debt.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};
use wcore_protocol::contract::generated_artifacts;

/// Ask a schema whether it accepts an instance.
///
/// Deliberately a *separate* implementation from `desktop_contract_corpus.rs`'s
/// `schema_accepts`: this one is only used to detect self-inconsistency, and the two
/// disagreeing would itself be a finding. Cross-checked against Python
/// `jsonschema` 4.23.0 `Draft202012Validator` over all 52 event fixtures against both
/// published schemas — identical verdicts, including the single `goal_snapshot`
/// rejection. Capture: `.planning/evidence/core-contract-defects/`.
fn accepts(schema: &Value, instance: &Value) -> bool {
    if let Some(expected) = schema.get("const")
        && expected != instance
    {
        return false;
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array)
        && !values.iter().any(|expected| expected == instance)
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
    if let Some(object) = instance.as_object() {
        let properties = schema.get("properties").and_then(Value::as_object);
        if schema.get("additionalProperties") == Some(&Value::Bool(false))
            && object
                .keys()
                .any(|field| !properties.is_some_and(|properties| properties.contains_key(field)))
        {
            return false;
        }
        if let Some(properties) = properties {
            for (field, field_schema) in properties {
                if let Some(value) = object.get(field)
                    && !accepts(field_schema, value)
                {
                    return false;
                }
            }
        }
    }
    if let (Some(items), Some(values)) = (schema.get("items"), instance.as_array())
        && values.iter().any(|value| !accepts(items, value))
    {
        return false;
    }
    if let Some(any_of) = schema.get("anyOf").and_then(Value::as_array)
        && !any_of.iter().any(|branch| accepts(branch, instance))
    {
        return false;
    }
    if let Some(all_of) = schema.get("allOf").and_then(Value::as_array)
        && all_of.iter().any(|branch| !accepts(branch, instance))
    {
        return false;
    }
    if let Some(one_of) = schema.get("oneOf").and_then(Value::as_array)
        && one_of
            .iter()
            .filter(|branch| accepts(branch, instance))
            .count()
            != 1
    {
        return false;
    }
    if let Some(forbidden) = schema.get("not")
        && accepts(forbidden, instance)
    {
        return false;
    }
    true
}

fn artifacts() -> BTreeMap<String, Value> {
    generated_artifacts()
        .expect("the generator must produce the contract corpus in memory")
        .into_iter()
        .filter(|(relative, _)| relative.ends_with(".json"))
        .map(|(relative, bytes)| {
            let value = serde_json::from_slice(&bytes)
                .unwrap_or_else(|error| panic!("{relative} is not JSON: {error}"));
            (relative, value)
        })
        .collect()
}

fn event_fixtures(artifacts: &BTreeMap<String, Value>) -> Vec<(&str, &Value)> {
    artifacts
        .iter()
        .filter(|(relative, _)| relative.starts_with("events/"))
        .map(|(relative, value)| (relative.as_str(), value))
        .collect()
}

/// A `oneOf` branch set that NO instance can ever satisfy, because more than one
/// branch is an unconstrained object: `additionalProperties` not `false` and no
/// `required`. Any object matching one such branch matches them all, so the
/// "exactly one" rule can never hold.
fn one_of_is_unsatisfiable(branches: &[Value]) -> bool {
    let permissive = branches
        .iter()
        .filter(|branch| {
            branch.get("type").and_then(Value::as_str) == Some("object")
                && branch.get("additionalProperties") != Some(&Value::Bool(false))
                && branch
                    .get("required")
                    .and_then(Value::as_array)
                    .is_none_or(|required| required.is_empty())
        })
        .count();
    permissive > 1
}

fn walk_one_of_sites(node: &Value, path: &str, found: &mut Vec<(String, Vec<Value>)>) {
    match node {
        Value::Object(object) => {
            if !path.is_empty()
                && let Some(branches) = object.get("oneOf").and_then(Value::as_array)
            {
                found.push((path.to_string(), branches.clone()));
            }
            for (key, value) in object {
                walk_one_of_sites(value, &format!("{path}/{key}"), found);
            }
        }
        Value::Array(values) => {
            for (index, value) in values.iter().enumerate() {
                walk_one_of_sites(value, &format!("{path}/{index}"), found);
            }
        }
        _ => {}
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// C5 — the published schema must accept the published corpus
// ─────────────────────────────────────────────────────────────────────────────

#[test]
fn published_schemas_accept_every_event_fixture_they_describe() {
    let artifacts = artifacts();
    let core = &artifacts["schema/core-event.schema.json"];
    let producer = &artifacts["schema/producer-complete.schema.json"];
    let fixtures = event_fixtures(&artifacts);

    // BOTH DIRECTIONS, in-test, before the gate is trusted.
    //
    // Can it PASS? A schema that describes a fixture must accept it. If this
    // assertion ever fails the detector is stuck red and proves nothing, which is
    // exactly how `22-C3` was misgraded.
    let (_, first) = fixtures[0];
    let permissive = json!({"type": "object", "required": ["type"]});
    assert!(
        accepts(&permissive, first),
        "detector cannot reach a PASS state: a trivially-satisfiable schema rejected \
         a real fixture"
    );
    // Can it FAIL? A schema demanding an absent field must reject.
    let impossible = json!({"type": "object", "required": ["__no_such_field__"]});
    assert!(
        !accepts(&impossible, first),
        "detector cannot reach a FAIL state: an unsatisfiable schema accepted a fixture"
    );
    assert!(
        fixtures.len() >= 52,
        "a corpus sweep over {} fixtures is not a sweep — an empty or filtered set \
         passes this test for free (LANE-BRIEF §3.2)",
        fixtures.len()
    );

    let mut rejected = Vec::new();
    for (relative, fixture) in &fixtures {
        if !accepts(core, fixture) {
            rejected.push(format!(
                "{relative} rejected by schema/core-event.schema.json"
            ));
        }
        if !accepts(producer, fixture) {
            rejected.push(format!(
                "{relative} rejected by schema/producer-complete.schema.json"
            ));
        }
    }

    assert!(
        rejected.is_empty(),
        "the published Desktop contract REJECTS {} of its own {} event fixtures, so a \
         host validating against it rejects a valid Core frame:\n  {}",
        rejected.len(),
        fixtures.len(),
        rejected.join("\n  ")
    );
}

#[test]
fn no_published_one_of_is_unsatisfiable_by_construction() {
    let artifacts = artifacts();

    // Both directions on the detector itself.
    let bad = vec![
        json!({"type": "object", "additionalProperties": true, "properties": {"a": {}}}),
        json!({"type": "object", "additionalProperties": true, "properties": {"b": {}}}),
    ];
    assert!(
        one_of_is_unsatisfiable(&bad),
        "detector cannot FAIL: two permissive object branches were called satisfiable"
    );
    let good = vec![
        json!({"type": "object", "additionalProperties": false, "required": ["Ok"],
               "properties": {"Ok": {"type": "null"}}}),
        json!({"type": "object", "additionalProperties": false, "required": ["Err"],
               "properties": {"Err": {"type": "string"}}}),
    ];
    assert!(
        !one_of_is_unsatisfiable(&good),
        "detector cannot PASS: a genuinely disjoint Ok/Err union was called unsatisfiable"
    );
    let nullable = vec![json!({"type": "string"}), json!({"type": "null"})];
    assert!(
        !one_of_is_unsatisfiable(&nullable),
        "detector cannot PASS: a value-or-null union was called unsatisfiable"
    );

    let mut sites = Vec::new();
    for name in [
        "schema/core-event.schema.json",
        "schema/producer-complete.schema.json",
        "schema/host-command.schema.json",
    ] {
        walk_one_of_sites(&artifacts[name], "", &mut sites);
    }
    assert!(
        sites.len() >= 10,
        "only {} nested oneOf sites found across the three published schemas — the \
         walker is not reaching them, so a clean result here is vacuous",
        sites.len()
    );

    let broken = sites
        .iter()
        .filter(|(_, branches)| one_of_is_unsatisfiable(branches))
        .map(|(path, branches)| format!("{path} ({} branches)", branches.len()))
        .collect::<Vec<_>>();
    assert!(
        broken.is_empty(),
        "{} published `oneOf` site(s) can never be satisfied by ANY instance, because \
         more than one branch is an unconstrained object. Use `anyOf` for inferred \
         (descriptive) unions; keep `oneOf` only where branches pin `required` and \
         `additionalProperties: false`:\n  {}",
        broken.len(),
        broken.join("\n  ")
    );
}

// ─────────────────────────────────────────────────────────────────────────────
// C4 — every emitted variant must be modelled, or declared deferred
// ─────────────────────────────────────────────────────────────────────────────

/// Wire tags of every `ProtocolEvent` variant, read out of `events.rs` itself.
///
/// A hand-maintained list would go stale the first time someone adds a variant,
/// which is the failure mode this gate exists to prevent — so it parses the real
/// source at compile time via `include_str!`. Honours `#[serde(rename = "...")]`
/// and otherwise applies the enum's `rename_all = "snake_case"`.
fn protocol_event_wire_types() -> BTreeSet<String> {
    const SOURCE: &str = include_str!("../src/events.rs");

    let anchor = SOURCE
        .find("pub enum ProtocolEvent {")
        .expect("ProtocolEvent must remain declared in events.rs");
    let body = &SOURCE[SOURCE[anchor..].find('{').unwrap() + anchor + 1..];

    let mut wire_types = BTreeSet::new();
    let mut rename: Option<String> = None;
    let mut depth = 0usize;
    let mut index = 0usize;

    // NOTE: `index` is a BYTE offset into `body`, but `body` is not ASCII — the doc
    // comments carry em-dashes and other multi-byte characters. The first cut of this
    // loop advanced `index` by 1 unconditionally and panicked with "byte index 3618 is
    // not a char boundary; it is inside '—'". Every step below therefore advances by a
    // whole character (`len_utf8`) or by an offset returned from a search for an ASCII
    // needle, both of which are guaranteed char-aligned.
    while index < body.len() {
        let rest = &body[index..];
        let character = rest.chars().next().expect("index is a char boundary");
        if depth == 0 && character == '}' {
            break;
        }
        if depth == 0 && rest.starts_with("//") {
            index += rest.find('\n').map_or(rest.len(), |offset| offset + 1);
            continue;
        }
        if depth == 0 && rest.starts_with("#[") {
            let end = rest.find(']').map_or(rest.len(), |offset| offset + 1);
            let attribute = &rest[..end];
            if !attribute.contains("rename_all")
                && let Some(start) = attribute.find("rename = \"")
            {
                let tail = &attribute[start + "rename = \"".len()..];
                rename = tail.find('"').map(|end| tail[..end].to_string());
            }
            index += end;
            continue;
        }
        if depth == 0 && character.is_ascii_uppercase() {
            let end = rest
                .find(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .unwrap_or(rest.len());
            let identifier = &rest[..end];
            let follows = rest[end..].trim_start().chars().next();
            if matches!(follows, Some('{') | Some('(') | Some(',')) {
                wire_types.insert(rename.take().unwrap_or_else(|| snake_case(identifier)));
            }
            index += end;
            continue;
        }
        match character {
            '{' | '(' | '[' => depth += 1,
            '}' | ')' | ']' => depth = depth.saturating_sub(1),
            _ => {}
        }
        index += character.len_utf8();
    }
    wire_types
}

fn snake_case(identifier: &str) -> String {
    let mut out = String::with_capacity(identifier.len() + 4);
    for (index, character) in identifier.char_indices() {
        if character.is_ascii_uppercase() {
            if index != 0 {
                out.push('_');
            }
            out.push(character.to_ascii_lowercase());
        } else {
            out.push(character);
        }
    }
    out
}

/// Discriminators a branch pins, paired with whether it models a real payload
/// (i.e. declares any property other than `type`).
fn discriminators(branches: &[Value]) -> BTreeMap<String, bool> {
    let mut out = BTreeMap::new();
    for branch in branches {
        let Some(properties) = branch.get("properties").and_then(Value::as_object) else {
            continue;
        };
        let models_payload = properties.keys().any(|key| key != "type");
        let Some(discriminator) = properties.get("type") else {
            continue;
        };
        let mut names = Vec::new();
        if let Some(Value::String(name)) = discriminator.get("const") {
            names.push(name.clone());
        }
        if let Some(values) = discriminator.get("enum").and_then(Value::as_array) {
            names.extend(values.iter().filter_map(Value::as_str).map(str::to_string));
        }
        for name in names {
            let entry = out.entry(name).or_insert(false);
            *entry = *entry || models_payload;
        }
    }
    out
}

#[test]
fn protocol_event_source_parser_is_alive_in_both_directions() {
    let wire_types = protocol_event_wire_types();

    // Can it PASS: variants that certainly exist are found, including a
    // `#[serde(rename)]`d one and a multi-word one.
    for expected in [
        "ready",
        "sub_agent_event",
        "execution_policy",
        "workspace_policy",
    ] {
        assert!(
            wire_types.contains(expected),
            "parser missed the known-present variant `{expected}` — every absence this \
             gate reports would be free (LANE-BRIEF §3b-i)"
        );
    }
    // Can it FAIL: a tag that does not exist must not be reported present.
    assert!(
        !wire_types.contains("__variant_that_does_not_exist__"),
        "parser invents variants, so its inventory cannot be trusted"
    );
    // Third assertion — the one that proves the UTF-8 repair is load-bearing rather
    // than cosmetic (LANE-BRIEF §6b-ii). The first cut of this parser advanced by one
    // BYTE at a time and panicked on the em-dash in `Ready`'s doc comment, ~3.6 KB
    // into the enum body, so it never reached anything declared after it. Assert both
    // that the hazard is genuinely present in the source and that parsing survives it.
    const SOURCE: &str = include_str!("../src/events.rs");
    let enum_body = &SOURCE[SOURCE.find("pub enum ProtocolEvent {").unwrap()..];
    assert!(
        enum_body.chars().any(|character| !character.is_ascii()),
        "no non-ASCII character remains in the ProtocolEvent body, so this assertion \
         no longer exercises the multi-byte hazard it was written for — replace it \
         rather than letting it pass vacuously"
    );
    let first_non_ascii = enum_body
        .char_indices()
        .find(|(_, character)| !character.is_ascii())
        .map(|(offset, _)| offset)
        .unwrap();
    let declared_after_hazard = enum_body[first_non_ascii..].contains("CompactOffload");
    assert!(
        declared_after_hazard && wire_types.contains("compact_offload"),
        "`CompactOffload` is declared after the first multi-byte character yet the \
         parser did not report it — the byte-at-a-time scan is back"
    );
    assert!(
        wire_types.len() >= 52,
        "parsed only {} ProtocolEvent wire types; the enum has strictly more than the \
         52 Desktop events, so the parser is truncating",
        wire_types.len()
    );
    assert_eq!(
        snake_case("MidFlightMonitorDecision"),
        "mid_flight_monitor_decision"
    );
}

#[test]
fn every_protocol_event_variant_is_modelled_or_declared_deferred() {
    let artifacts = artifacts();
    let manifest = &artifacts["manifest.json"];
    let core = discriminators(
        artifacts["schema/core-event.schema.json"]["oneOf"]
            .as_array()
            .unwrap(),
    );
    let producer = discriminators(
        artifacts["schema/producer-complete.schema.json"]["anyOf"]
            .as_array()
            .unwrap(),
    );

    let deferred = String::from_utf8(generated_artifacts().unwrap()["DEFERRED.md"].clone())
        .expect("DEFERRED.md must be UTF-8");
    // A DECLARATION is a Markdown list item, not any mention. Mutation testing caught
    // this: deleting `- ``workspace_policy``` from DEFERRED left the gate GREEN,
    // because the surrounding prose happens to say "``workspace_policy`` is the one to
    // model first" and a substring search accepted that as a declaration. Prose drifts
    // and gets reworded; a list item is a deliberate entry. Requiring the list form is
    // what makes deleting an entry actually redden this gate.
    let declared = deferred
        .lines()
        .filter_map(|line| {
            line.trim_start()
                .strip_prefix("- ")
                .and_then(|item| item.trim().strip_prefix('`'))
                .and_then(|item| item.strip_suffix('`'))
                .filter(|item| !item.contains('`'))
                .map(str::to_string)
        })
        .collect::<BTreeSet<_>>();

    // Both directions on the list-item extractor itself, before anything trusts it.
    assert!(
        declared.contains("ordinary_turn_tool_replay_reducer"),
        "the DEFERRED list-item extractor found none of the pre-existing entries, so \
         every `declared` lookup below would be a free FAIL"
    );
    assert!(
        !declared.contains("ready"),
        "the extractor reports `ready` as deferred, so every lookup below would be a \
         free PASS"
    );

    let desktop = manifest["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["type"].as_str().unwrap().to_string())
        .collect::<BTreeSet<_>>();

    let mut unmodelled = Vec::new();
    for wire_type in protocol_event_wire_types() {
        if desktop.contains(&wire_type) {
            continue;
        }
        // Not a Desktop event. It is acceptable ONLY if some published schema
        // models its payload, or the contract declares the gap in DEFERRED.md.
        let modelled = core.get(&wire_type).copied().unwrap_or(false)
            || producer.get(&wire_type).copied().unwrap_or(false);
        if modelled {
            continue;
        }
        if declared.contains(&wire_type) {
            continue;
        }
        unmodelled.push(wire_type);
    }

    assert!(
        unmodelled.is_empty(),
        "{} ProtocolEvent variant(s) reach the JSON stream with NO payload schema in \
         any published artifact and NO entry in DEFERRED.md. A host can neither \
         validate nor legitimately consume them:\n  {}\nEither model the payload, or \
         declare the gap in `DEFERRED` in contract/generate.rs.",
        unmodelled.len(),
        unmodelled.join("\n  ")
    );
}

#[test]
fn every_desktop_manifest_event_has_a_real_protocol_event_variant() {
    // The reverse direction, and the gate in this file that is GREEN at base — proof
    // the suite is not simply a wall of red.
    let artifacts = artifacts();
    let variants = protocol_event_wire_types();
    let orphans = artifacts["manifest.json"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .filter(|wire_type| !variants.contains(*wire_type))
        .collect::<Vec<_>>();
    assert!(
        orphans.is_empty(),
        "the Desktop manifest promises event(s) that no ProtocolEvent variant can \
         ever emit: {orphans:?}"
    );
}
