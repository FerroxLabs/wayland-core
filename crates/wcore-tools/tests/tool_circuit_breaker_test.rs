//! H2-R5: Integration tests for the per-tool circuit breaker in `ToolRegistry`.
//!
//! Covers:
//! - Below threshold: stays closed, calls succeed.
//! - At threshold: opens, blocks subsequent calls.
//! - After cooldown: half-open, one trial allowed; success → closed.
//! - After cooldown: half-open trial fails → re-opens.
//! - Failures outside window don't count toward threshold.
//! - Unknown tool still returns is_error without tripping a breaker.

use std::time::Duration;

use async_trait::async_trait;
use wcore_config::circuit_breaker::BreakerState;
use wcore_protocol::events::ToolCategory;
use wcore_tools::Tool;
use wcore_tools::dispatcher::ToolDispatcher;
use wcore_tools::registry::ToolRegistry;
use wcore_types::tool::ToolResult;

// ── Test doubles ────────────────────────────────────────────────────────────

/// A tool that always returns success.
struct OkTool;

#[async_trait]
impl Tool for OkTool {
    fn name(&self) -> &str {
        "ok_tool"
    }
    fn description(&self) -> &str {
        "always succeeds"
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
        true
    }
    async fn execute(&self, _: serde_json::Value) -> ToolResult {
        ToolResult {
            content: "ok".into(),
            is_error: false,
        }
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }
}

/// A tool that always returns an error.
struct ErrTool;

#[async_trait]
impl Tool for ErrTool {
    fn name(&self) -> &str {
        "err_tool"
    }
    fn description(&self) -> &str {
        "always fails"
    }
    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({"type": "object"})
    }
    fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
        true
    }
    async fn execute(&self, _: serde_json::Value) -> ToolResult {
        ToolResult {
            content: "tool error".into(),
            is_error: true,
        }
    }
    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }
}

fn input() -> serde_json::Value {
    serde_json::json!({})
}

// ── Tests ────────────────────────────────────────────────────────────────────

/// Successful calls keep the breaker closed and return the tool result.
#[tokio::test]
async fn closed_below_threshold_success_calls_pass_through() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(OkTool));

    for _ in 0..10 {
        let r = reg.dispatch("ok_tool", input()).await;
        assert!(!r.is_error, "ok_tool must succeed");
    }
    assert_eq!(reg.breaker_state("ok_tool"), Some(BreakerState::Closed));
}

/// Two failures (< threshold of 3) must not open the breaker.
#[tokio::test]
async fn stays_closed_below_threshold() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(ErrTool));

    reg.dispatch("err_tool", input()).await;
    reg.dispatch("err_tool", input()).await;

    assert_eq!(
        reg.breaker_state("err_tool"),
        Some(BreakerState::Closed),
        "two failures must not trip the breaker (threshold is 3)"
    );
}

/// At the 3rd failure in the window the breaker opens; 4th call is blocked
/// and returns a circuit-open error.
#[tokio::test]
async fn opens_at_threshold_and_blocks_calls() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(ErrTool));

    // 3 failures → trips.
    for _ in 0..3 {
        reg.dispatch("err_tool", input()).await;
    }
    assert_eq!(reg.breaker_state("err_tool"), Some(BreakerState::Open));

    // 4th call must be blocked by the open breaker.
    let blocked = reg.dispatch("err_tool", input()).await;
    assert!(blocked.is_error, "blocked call must return is_error");
    assert!(
        blocked.content.contains("circuit open"),
        "error message must mention circuit open; got: {}",
        blocked.content
    );
}

/// After cooldown elapses, breaker enters HalfOpen and allows one trial.
/// A successful trial closes the breaker.
#[tokio::test]
async fn half_open_trial_success_closes_breaker() {
    // We need a very short cooldown to avoid a slow test. Use the shared
    // CircuitBreakerConfig directly on a stand-alone breaker, then verify
    // the registry wiring via `breaker_state`.
    //
    // The registry's default config has a 60-second cooldown, which we
    // can't wait for in a unit test. Instead, we drive the state machine
    // on the underlying `CircuitBreaker` type directly and confirm the
    // API contract holds — the registry test above already proved
    // `dispatch` gates on `is_open()`.
    use wcore_config::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};

    let b = CircuitBreaker::new(CircuitBreakerConfig {
        fail_threshold: 1,
        window: Duration::from_secs(30),
        cooldown: Duration::from_millis(2),
    });

    b.record_failure();
    assert_eq!(b.state(), BreakerState::Open);

    std::thread::sleep(Duration::from_millis(10));

    // is_open() transitions to HalfOpen and returns false.
    assert!(!b.is_open());
    assert_eq!(b.state(), BreakerState::HalfOpen);

    b.record_success();
    assert_eq!(b.state(), BreakerState::Closed);
    assert!(!b.is_open());
}

/// A failed HalfOpen trial immediately re-opens the breaker.
#[tokio::test]
async fn half_open_trial_failure_reopens() {
    use wcore_config::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};

    let b = CircuitBreaker::new(CircuitBreakerConfig {
        fail_threshold: 1,
        window: Duration::from_secs(30),
        cooldown: Duration::from_millis(2),
    });

    b.record_failure();
    std::thread::sleep(Duration::from_millis(10));
    assert!(!b.is_open()); // → HalfOpen

    let t = b.record_failure();
    assert_eq!(t, Some(BreakerState::Open));
    assert!(b.is_open());
}

/// Failures that fall outside the rolling window must not count toward
/// the threshold.
#[test]
fn failures_outside_window_do_not_count() {
    use wcore_config::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};

    let b = CircuitBreaker::new(CircuitBreakerConfig {
        fail_threshold: 2,
        window: Duration::from_millis(0), // zero window → every prior failure is stale
        cooldown: Duration::from_secs(60),
    });

    b.record_failure();
    std::thread::sleep(Duration::from_millis(1));
    let t = b.record_failure(); // prior failure evicted; only 1 in window
    assert!(
        t.is_none(),
        "stale failure must not count; breaker must stay closed"
    );
    assert_eq!(b.state(), BreakerState::Closed);
}

/// An unknown tool name returns is_error but does not panic or create a breaker.
#[tokio::test]
async fn unknown_tool_returns_error_no_breaker() {
    let reg = ToolRegistry::new();
    let r = reg.dispatch("ghost", input()).await;
    assert!(r.is_error);
    assert!(r.content.contains("ghost"));
    assert_eq!(reg.breaker_state("ghost"), None);
}

/// A success on a tool that previously had failures resets the breaker.
#[tokio::test]
async fn success_after_failures_resets_breaker() {
    // Register both a failing and an ok variant under the same registry
    // so we can test the success path without waiting for cooldown.
    // Drive the underlying breaker directly.
    use wcore_config::circuit_breaker::{CircuitBreaker, CircuitBreakerConfig};

    let b = CircuitBreaker::new(CircuitBreakerConfig::default());
    b.record_failure();
    b.record_failure();
    b.record_success(); // clears failures
    assert_eq!(b.state(), BreakerState::Closed);

    // Now two more failures should be needed before opening again.
    b.record_failure();
    b.record_failure();
    assert_eq!(b.state(), BreakerState::Closed);
}

/// #403: reset_all_breakers() clears an opened breaker so a new user turn
/// starts clean instead of staying short-circuited for the whole session.
#[tokio::test]
async fn reset_all_breakers_clears_open_breaker() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(ErrTool));

    for _ in 0..3 {
        reg.dispatch("err_tool", input()).await;
    }
    assert_eq!(reg.breaker_state("err_tool"), Some(BreakerState::Open));

    // Simulate the start of a new user turn.
    reg.reset_all_breakers();
    assert_eq!(
        reg.breaker_state("err_tool"),
        Some(BreakerState::Closed),
        "reset must close a previously-open breaker"
    );

    // The breaker must be functional again (not wedged): a fresh full
    // threshold of failures is needed before it re-opens.
    reg.dispatch("err_tool", input()).await;
    reg.dispatch("err_tool", input()).await;
    assert_eq!(reg.breaker_state("err_tool"), Some(BreakerState::Closed));
}
