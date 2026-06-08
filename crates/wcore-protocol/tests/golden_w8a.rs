//! W8a additions: golden snapshots for variants added by the W8a wave.
//! v0.1.21 (`golden_v0_1_21.rs`), W1 (`golden_w1.rs`), W7 (`golden_w7.rs`)
//! goldens stay untouched. This file evolves with W8a+.

use serde_json::json;
use wcore_protocol::events::ProtocolEvent;

/// W8a A.7 — `budget_exceeded`. Singular per-session event emitted when
/// the first ExecutionBudget cap trips. Host-tolerated additive variant
/// (no dedicated capability flag); v0.1.21 hosts drop it silently per
/// the W0 forward-compat baseline (verified in
/// `host_decoder_contract::host_drops_budget_exceeded_silently_per_w8a_a7`).
#[test]
fn golden_budget_exceeded_wall_time_w8a() {
    let event = ProtocolEvent::BudgetExceeded {
        reason: "max_wall_time".into(),
        observed: "62.0s".into(),
        limit: "60.0s".into(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(
        got,
        json!({
            "type": "budget_exceeded",
            "reason": "max_wall_time",
            "observed": "62.0s",
            "limit": "60.0s",
        })
    );
}

#[test]
fn golden_budget_exceeded_max_cost_usd_w8a() {
    let event = ProtocolEvent::BudgetExceeded {
        reason: "max_cost_usd".into(),
        observed: "$1.5234".into(),
        limit: "$1.5000".into(),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(got["type"], "budget_exceeded");
    assert_eq!(got["reason"], "max_cost_usd");
    assert_eq!(got["observed"], "$1.5234");
    assert_eq!(got["limit"], "$1.5000");
}

/// W8a H.1 — `plugin_event` — opaque plugin-emitted event. Gated by
/// the W0 `capabilities.plugins` flag at emission time; hosts that
/// don't recognise the variant drop it silently per the W0 host
/// decoder contract.
#[test]
fn golden_plugin_event_w8a() {
    let event = ProtocolEvent::PluginEvent {
        plugin_name: "wayland-ijfw".into(),
        event_type: "memory_capture".into(),
        payload: json!({ "key": "abc", "tier": "P2" }),
    };
    let got = serde_json::to_value(&event).unwrap();
    assert_eq!(
        got,
        json!({
            "type": "plugin_event",
            "plugin_name": "wayland-ijfw",
            "event_type": "memory_capture",
            "payload": { "key": "abc", "tier": "P2" },
        })
    );
}
