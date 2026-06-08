//! M5.bootstrap-wiring — end-to-end smoke for the three coupled fixes:
//!
//! 1. `Config { session_cap: Some(BudgetConfig { .. }), ..Default::default() }`
//!    proves `impl Default for Config` + the new `session_cap` field
//!    compose under spread syntax.
//!
//! 2. `AgentBootstrap::with_span_sink(...)` plumbs an `Arc<dyn SpanSink>`
//!    into the boot pipeline; the `ObservabilityBudgetEventBridge` then
//!    forwards `BudgetEvent::Charge` into the JSON span channel — the
//!    M3.3-style bridge that previously had no production install
//!    point.
//!
//! 3. The installed `BudgetTracker` enforces the configured cap:
//!    `charge` past the per-session USD limit returns
//!    `BudgetError::CapExceeded`. A separate test asserts the
//!    backward-compat path (no `session_cap` ⇒ `engine.budget_tracker()`
//!    stays `None`).

use std::sync::{Arc, Mutex};

use serde_json::Value;
use tempfile::TempDir;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::OutputSink;
use wcore_agent::output::null_sink::NullSink;
use wcore_budget::{BudgetConfig, BudgetError};
use wcore_config::config::Config;
use wcore_observability::sink::SpanSink;

/// SpanSink that captures every emitted JSON value into a shared buffer.
/// Distinct from `wcore_observability::sink::InMemorySink` only in that
/// we want the buffer handle separately addressable from the trait
/// object so the test can assert on collected events without re-cloning
/// the sink.
struct CollectingSink {
    events: Arc<Mutex<Vec<Value>>>,
}

impl SpanSink for CollectingSink {
    fn emit(&self, trace: &Value) {
        if let Ok(mut g) = self.events.lock() {
            g.push(trace.clone());
        }
    }
}

fn null_output() -> Arc<dyn OutputSink> {
    Arc::new(NullSink)
}

#[tokio::test]
async fn bootstrap_with_session_cap_installs_tracker_and_emits_charge_event() {
    let tmp = TempDir::new().expect("workdir");
    let workspace = tmp.path().to_str().expect("workdir utf-8").to_string();

    let buffer = Arc::new(Mutex::new(Vec::<Value>::new()));
    let sink: Arc<dyn SpanSink> = Arc::new(CollectingSink {
        events: Arc::clone(&buffer),
    });

    // Acceptance criterion 1: spread syntax over `Default` with the new
    // `session_cap` field carries through cleanly. If `Config` ever loses
    // `Default` or grows another mandatory field, this stops compiling
    // — that's the regression guard.
    let cfg = Config {
        session_cap: Some(BudgetConfig {
            max_cost_usd: Some(0.10),
            max_tokens_in: Some(500),
            max_tokens_out: Some(500),
            ..Default::default()
        }),
        ..Default::default()
    };

    let result = AgentBootstrap::new(cfg, workspace, null_output())
        .with_span_sink(Arc::clone(&sink))
        .build()
        .await
        .expect("bootstrap should succeed");

    // Acceptance criterion 2: the tracker actually installed.
    let tracker = result
        .engine
        .budget_tracker()
        .cloned()
        .expect("session_cap was Some, so budget_tracker must be Some");

    // Acceptance criterion 3a: a successful charge fires a
    // `BudgetEvent::Charge` JSON value through the SpanSink, proving the
    // ObservabilityBudgetEventBridge is wired end-to-end.
    tracker
        .lock()
        .charge("session-A", 100, 0.05)
        .expect("charge under cap");

    // Acceptance criterion 3b: charging past the cap returns CapExceeded.
    // The per-session USD cap is $0.10 and we already charged $0.05, so
    // an additional $0.10 trips the cap.
    let err = tracker
        .lock()
        .charge("session-A", 0, 0.10)
        .expect_err("post-cap charge must reject");
    assert!(
        matches!(err, BudgetError::CapExceeded { ref kind, .. } if kind == "per_session_usd"),
        "expected per_session_usd CapExceeded, got {err:?}"
    );

    // Inspect the captured sink events: must contain one Charge and one
    // CapBlock (the rejected charge fires CapBlock per BudgetTracker
    // semantics in `tracker.rs`).
    let events = buffer.lock().expect("buffer lock");
    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|v| v.get("kind").and_then(|k| k.as_str()))
        .collect();
    assert!(
        kinds.contains(&"charge"),
        "expected a BudgetEvent::Charge in the captured span events, got {kinds:?}"
    );
    assert!(
        kinds.contains(&"cap_block"),
        "expected a BudgetEvent::CapBlock for the rejected charge, got {kinds:?}"
    );
}

#[tokio::test]
async fn bootstrap_without_session_cap_leaves_budget_tracker_unset() {
    let tmp = TempDir::new().expect("workdir");
    let workspace = tmp.path().to_str().expect("workdir utf-8").to_string();

    // No session_cap and no span sink — pre-M5.3 wire shape.
    let cfg = Config::default();
    let result = AgentBootstrap::new(cfg, workspace, null_output())
        .build()
        .await
        .expect("bootstrap should succeed");

    assert!(
        result.engine.budget_tracker().is_none(),
        "without Config.session_cap, the engine's BudgetTracker must stay None"
    );
}
