//! W10B addition: evolution_event golden + gepa_enabled default-off invariant.
//!
//! Locks the new `ProtocolEvent::EvolutionEvent` variant added in W10B and
//! the dedicated `Capabilities.gepa_enabled` flag. The v0.1.21 baseline golden
//! in `golden_v0_1_21.rs` stays untouched; this file evolves alongside W10B+
//! protocol additions.

use serde_json::json;
use wcore_protocol::events::{Capabilities, ProtocolEvent};

#[test]
fn golden_evolution_event_w10b() {
    // W10B: ProtocolEvent::EvolutionEvent shape locked. This variant is
    // EMITTED only when capabilities.gepa_enabled is true (NOT
    // structured_traces). The W0 host decoder contract handles the
    // unknown-type drop for older hosts.
    let event = ProtocolEvent::EvolutionEvent {
        run_id: "run-001".into(),
        generation: 2,
        parent_id: "skill-refactor-imports".into(),
        child_id: "run-001/2/3".into(),
        mutation_kind: "Reorder".into(),
        score: 0.83,
        retained: true,
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(
        got,
        json!({
            "type": "evolution_event",
            "run_id": "run-001",
            "generation": 2,
            "parent_id": "skill-refactor-imports",
            "child_id": "run-001/2/3",
            "mutation_kind": "Reorder",
            "score": 0.83,
            "retained": true,
        })
    );
}

#[test]
fn golden_capabilities_default_omits_gepa_enabled() {
    // W0 forward-additive invariant: default-off Capabilities must NOT
    // serialize the new `gepa_enabled` key (skip_serializing_if = is_false).
    // Same discipline as every other W0 flag.
    let caps = Capabilities::default();
    let json = serde_json::to_value(&caps).unwrap();
    assert!(
        json.get("gepa_enabled").is_none(),
        "default-off Capabilities must omit gepa_enabled key; got {json}"
    );
}

#[test]
fn golden_capabilities_with_gepa_enabled_on_serializes_the_key() {
    let caps = Capabilities {
        gepa_enabled: true,
        ..Default::default()
    };
    let json = serde_json::to_value(&caps).unwrap();
    assert_eq!(json["gepa_enabled"], true);
}
