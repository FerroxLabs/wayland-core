//! Which command answers an `approval_required`, graded against the bytes a
//! host actually receives.
//!
//! # Why this file was rewritten (wayland#1088)
//!
//! It used to hold four serde round-trips of HAND-WRITTEN JSON. Each built an
//! `ApprovalRequired` by hand with a non-empty `resume_token` and
//! `correlation_id == resume_token`, serialised it, and asserted the fields
//! came back. No emitter was ever called, so the tests could not observe what
//! the engine emits — and what they hand-wrote was the OPPOSITE of it. The
//! engine sets `correlation_id = call_id` (never the token), and an ordinary
//! tool gate has NO bridge entry, so its `resume_token` is the empty string
//! (`crates/wcore-protocol/src/events.rs`, the `ApprovalRequired` doc; and
//! `docs/json-stream-protocol.md`, "Which command answers this").
//!
//! Those four tests, and a corpus row that agreed with them, are what taught a
//! host to answer every gate with `approval_resume` — which resolves nothing
//! for an ordinary gate, so the tool hangs until its TTL.
//!
//! So the oracle here is no longer a hand-written frame. It is:
//!
//!   * the SHIPPED corpus bytes under `contracts/desktop/v1/` (all a real
//!     integrator has), asserted against the documented rule, and
//!   * the real [`ToolApprovalManager`], driven to show which key actually
//!     resolves an ordinary gate.

use std::path::PathBuf;

use serde_json::{Value, json};
use wcore_protocol::commands::{ApprovalScope, ProtocolCommand};
use wcore_protocol::events::ToolCategory;
use wcore_protocol::{ToolApprovalManager, ToolApprovalResult};

/// The corpus as SHIPPED — the checked-in bytes, not a regeneration. A real
/// integrator has these and nothing else.
fn shipped(relative: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("contracts/desktop/v1")
        .join(relative);
    let bytes = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_slice(&bytes).unwrap_or_else(|e| panic!("{} is not JSON: {e}", path.display()))
}

/// `correlation_id` "always equals `call_id`" — the documented invariant, over
/// every shipped `approval_required` row rather than the canonical one alone.
///
/// Red on the shipped corpus: the row carried `correlation_id: "resume-001"`
/// against `call_id: "call-tool-001"`.
#[test]
fn every_shipped_approval_required_correlates_on_its_call_id() {
    let manifest = shipped("manifest.json");
    let rows = manifest["events"]
        .as_array()
        .expect("manifest.events must be an array")
        .iter()
        .filter(|entry| entry["type"] == "approval_required")
        .map(|entry| entry["path"].as_str().expect("path").to_string())
        .chain(["compat/events/approval_required.minimal.json".to_string()])
        .collect::<Vec<_>>();
    assert!(
        rows.len() >= 2,
        "instrument control: the corpus must publish the canonical row and its \
         minimal compat sibling; found {rows:?}"
    );

    for relative in rows {
        let row = shipped(&relative);
        let call_id = row["call_id"].as_str().expect("call_id must be a string");
        // `correlation_id` is omitted when empty (`skip_serializing_if`); a row
        // that carries it must carry the call_id.
        if let Some(correlation_id) = row.get("correlation_id").and_then(Value::as_str) {
            assert_eq!(
                correlation_id, call_id,
                "{relative}: correlation_id always equals call_id (events.rs, ApprovalRequired)"
            );
        }
    }
}

/// The canonical row is the ordinary tool gate — the case a host meets on
/// nearly every session — so its `resume_token` is the EMPTY string, and the
/// manifest must not tell a host to correlate it on that token.
///
/// Red on the shipped corpus twice over: the row carried `"resume-001"`, and
/// the manifest declared `"correlation": "resume_token"`.
#[test]
fn the_canonical_approval_required_row_is_an_ordinary_tool_gate() {
    let row = shipped("events/approval_required.json");
    assert!(
        row.get("plan").is_none(),
        "instrument control: the canonical row must not be a Crucible council card"
    );
    assert_eq!(
        row["resume_token"], "",
        "an ordinary tool gate has no bridge entry, so its resume_token is empty \
         (docs/json-stream-protocol.md, \"Which command answers this\")"
    );

    let manifest = shipped("manifest.json");
    let declared = manifest["events"]
        .as_array()
        .expect("manifest.events")
        .iter()
        .find(|entry| entry["type"] == "approval_required")
        .expect("manifest must declare approval_required");
    assert_eq!(
        declared["correlation"], "call_id",
        "the token is empty on the ordinary gate, so it cannot be the correlation key"
    );
}

/// What the host must send back, driven through the REAL approval manager
/// rather than a serde round-trip: the ordinary gate is resolved by `call_id`,
/// and the empty token the row carries resolves nothing.
#[test]
fn the_real_manager_resolves_an_ordinary_gate_by_call_id_and_never_by_its_empty_token() {
    let row = shipped("events/approval_required.json");
    let call_id = row["call_id"].as_str().expect("call_id").to_string();
    let token = row["resume_token"]
        .as_str()
        .expect("resume_token")
        .to_string();

    // The host answers with `tool_approve`, keyed by the call_id it read off
    // the gate frame — the command the corpus row's correlation key names.
    let command: ProtocolCommand =
        serde_json::from_value(json!({"type": "tool_approve", "call_id": call_id}))
            .expect("tool_approve must deserialize");
    let ProtocolCommand::ToolApprove { call_id: keyed, .. } = command else {
        panic!("expected ToolApprove");
    };

    let manager = ToolApprovalManager::new();
    let mut parked = manager.request_approval(&keyed, &ToolCategory::Exec, "Bash");

    // Echoing the (empty) resume_token back is the host bug this row taught:
    // it names no pending call, so the gate stays parked.
    manager.approve(&token, ApprovalScope::Once, None);
    assert!(
        matches!(
            parked.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ),
        "the empty resume_token must resolve nothing — that is why an ordinary \
         gate answered with approval_resume hangs until its TTL"
    );

    manager.approve(&keyed, ApprovalScope::Once, None);
    let outcome = parked
        .try_recv()
        .expect("approving by call_id must release the parked gate");
    assert!(
        matches!(outcome, ToolApprovalResult::Approved { .. }),
        "approving by call_id must release the ordinary gate"
    );
}
