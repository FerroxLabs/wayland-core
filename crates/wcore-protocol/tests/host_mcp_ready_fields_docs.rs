//! Every field `mcp_ready` puts on the wire must have a row in the §1.13 field
//! table a host integrator reads.
//!
//! # Why this file exists
//!
//! wayland#605 asked for one thing: make a SKIPPED `add_mcp_server` re-add
//! distinguishable from a real reconnect. The producer answers it with
//! `mcp_ready.already_connected`, which is `skip_serializing_if`-omitted when
//! false — so the only way a host learns the field exists at all is by being
//! told. `docs/json-stream-protocol.md` §1.13 carries an explicit field table,
//! and shipping the field while leaving that table listing only `name` and
//! `tools` would have closed the ticket on the wire and left it open for the
//! only audience it was for.
//!
//! # What is asserted
//!
//! The field set is taken from the WIRE, not from a source grep and not from a
//! hardcoded list: the test serializes a real [`ProtocolEvent::McpReady`] with
//! every optional field populated and reads the resulting JSON object's keys.
//! A future field added to the variant therefore reddens this gate until §1.13
//! documents it, which is the failure wayland#605 actually hit.
//!
//! # It runs in BOTH directions
//!
//! [`undocumented_fields`] is a pure function of two strings, so the tests feed
//! it doctored inputs: [`the_gate_rejects_a_field_table_missing_a_row`] deletes
//! the `already_connected` row and asserts it is reported, and
//! [`the_gate_passes_for_a_renamed_field_that_is_documented`] renames the field
//! in both the payload and the document and asserts green — so the pass state
//! is reachable under a changed fact rather than pinned to today's literal.
//! Each doctoring step asserts it actually changed its input; a `str::replace`
//! that matches nothing returns its argument and would make the control vacuous.

use std::path::{Path, PathBuf};

use serde_json::Value;
use wcore_protocol::events::ProtocolEvent;

const PROTOCOL_DOC: &str = "docs/json-stream-protocol.md";
const SECTION_START: &str = "### 1.13 `mcp_ready`";
const SECTION_END: &str = "### 1.14 ";

fn repo_root() -> PathBuf {
    // crates/wcore-protocol → crates → repo root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("wcore-protocol lives two levels below the repo root")
        .to_path_buf()
}

fn read_doc() -> String {
    let path = repo_root().join(PROTOCOL_DOC);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

/// The keys `mcp_ready` actually puts on the wire, minus the `type` tag.
///
/// Built by serializing the real event with every optional field populated, so
/// the gate tracks the variant rather than a list someone has to remember to
/// update.
fn wire_fields() -> Vec<String> {
    let event = ProtocolEvent::McpReady {
        name: "my-tools".into(),
        tools: vec!["tool_a".into()],
        already_connected: true,
    };
    let Value::Object(map) = serde_json::to_value(&event).expect("mcp_ready serializes") else {
        panic!("mcp_ready must serialize to a JSON object");
    };
    let fields: Vec<String> = map.keys().filter(|k| *k != "type").cloned().collect();
    assert!(
        !fields.is_empty(),
        "extracted no fields from a serialized mcp_ready — the rest of this gate would pass \
         vacuously"
    );
    fields
}

/// The §1.13 body, bounded so a row in a neighbouring section cannot satisfy
/// this gate. Panics rather than returning empty: a renamed heading must redden.
fn section(doc: &str) -> &str {
    let start = doc
        .find(SECTION_START)
        .unwrap_or_else(|| panic!("{PROTOCOL_DOC} has no {SECTION_START} heading"));
    let rest = &doc[start..];
    let end = rest
        .find(SECTION_END)
        .unwrap_or_else(|| panic!("{PROTOCOL_DOC} §1.13 is not terminated by {SECTION_END}"));
    &rest[..end]
}

/// Returns one problem per wire field with no row in the §1.13 field table.
/// Empty means the document and the wire agree.
fn undocumented_fields(doc: &str, fields: &[String]) -> Vec<String> {
    let body = section(doc);
    fields
        .iter()
        .filter(|field| !body.contains(&format!("| `{field}` |")))
        .map(|field| {
            format!(
                "{PROTOCOL_DOC} §1.13 `mcp_ready` puts `{field}` on the wire but its field table \
                 has no row for it — a host integrator reading the spec cannot learn the field \
                 exists"
            )
        })
        .collect()
}

/// Asserts a doctoring step actually changed the input.
fn doctored(original: &str, from: &str, to: &str) -> String {
    let out = original.replace(from, to);
    assert_ne!(
        out, original,
        "doctoring step matched nothing: {from:?} is not present, so the control below would be \
         vacuous"
    );
    out
}

#[test]
fn the_wire_carries_the_skip_annotation_this_gate_exists_for() {
    let fields = wire_fields();
    assert!(
        fields.iter().any(|f| f == "already_connected"),
        "mcp_ready no longer serializes `already_connected`; wayland#605's host-facing answer is \
         gone and the documentation gate below would be grading nothing. Got: {fields:?}"
    );
}

#[test]
fn every_mcp_ready_wire_field_has_a_row_in_the_protocol_spec() {
    let problems = undocumented_fields(&read_doc(), &wire_fields());
    assert!(
        problems.is_empty(),
        "mcp_ready fields are undocumented:\n- {}",
        problems.join("\n- ")
    );
}

#[test]
fn the_gate_rejects_a_field_table_missing_a_row() {
    let doc = read_doc();
    let stripped = doctored(&doc, "| `already_connected` |", "| `something_else` |");
    let problems = undocumented_fields(&stripped, &wire_fields());
    assert_eq!(
        problems.len(),
        1,
        "deleting the `already_connected` row must be reported exactly once; got {problems:?}"
    );
    assert!(problems[0].contains("already_connected"));
}

#[test]
fn the_gate_passes_for_a_renamed_field_that_is_documented() {
    let doc = doctored(&read_doc(), "already_connected", "skipped_readd");
    let fields: Vec<String> = wire_fields()
        .into_iter()
        .map(|f| {
            if f == "already_connected" {
                "skipped_readd".to_string()
            } else {
                f
            }
        })
        .collect();
    assert!(
        undocumented_fields(&doc, &fields).is_empty(),
        "a consistently renamed world must be green — otherwise this gate pins today's field \
         name rather than tracking the criterion"
    );
}

#[test]
fn the_gate_is_bounded_to_section_1_13() {
    let doc = read_doc();
    let body = section(&doc);
    assert!(
        body.starts_with(SECTION_START),
        "the extracted section must begin at the 1.13 heading"
    );
    assert!(
        !body.contains(SECTION_END),
        "the extracted section must stop before 1.14, or a row in a later section could satisfy \
         this gate"
    );
    assert!(
        body.contains("| `tools` |"),
        "the extracted section must contain the existing field table; if it does not, the bounds \
         are wrong and every assertion above is meaningless"
    );
}
