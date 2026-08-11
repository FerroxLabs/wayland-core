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

// ── A-4: the caller's command failing is not the tool failing ───────────────

/// A real `BashTool`, dispatched through the real registry, running commands
/// that exit non-zero. The shell is healthy the whole time and must stay
/// available.
///
/// The measured failure this pins: three errored Bash results in thirty
/// seconds opened the Bash breaker for sixty seconds, so every later call —
/// including the correctly reshaped ones the agent pivoted to — came back
/// `circuit open`, the mid-flight monitor read that as the same error
/// repeating, and the run was terminated having delivered nothing.
///
/// Guarded against measuring nothing: the test asserts each call actually
/// reached a child (`Exit code: 7`). If the sandbox refuses or breaks in this
/// environment the assertion fails loudly rather than passing vacuously on a
/// breaker that was never exercised.
#[tokio::test]
async fn a_shell_reporting_a_failed_command_keeps_its_circuit_closed() {
    use wcore_tools::bash::BashTool;

    let mut reg = ToolRegistry::new();
    reg.register(Box::new(BashTool));

    let call = serde_json::json!({"command": "exit 7"});
    for attempt in 1..=5 {
        let r = reg.dispatch("Bash", call.clone()).await;
        assert!(
            !r.content.contains("circuit open"),
            "attempt {attempt}: the shell was taken away from the agent for \
             reporting that the command it was handed failed: {}",
            r.content
        );
        assert!(
            r.content.starts_with("Exit code: 7"),
            "attempt {attempt}: this test only means something if a child \
             really ran and really exited 7; got: {}",
            r.content
        );
        assert!(
            r.is_error,
            "attempt {attempt}: exit 7 is an error to the caller"
        );
    }

    assert_eq!(
        reg.breaker_state("Bash"),
        Some(BreakerState::Closed),
        "five failed commands are five failed commands, not a sick shell"
    );
}

/// The control for the test above, and the reason the classifier cannot just
/// return `false`: a tool whose errors are its OWN machinery failing still
/// trips at the threshold and is still short-circuited.
///
/// `ErrTool` returns content matching neither the completed-child nor the
/// refused prefix, so it is graded a tool fault exactly as before.
#[tokio::test]
async fn a_genuinely_failing_tool_still_opens_its_circuit() {
    let mut reg = ToolRegistry::new();
    reg.register(Box::new(ErrTool));

    for _ in 0..3 {
        let r = reg.dispatch("err_tool", input()).await;
        assert!(!r.content.contains("circuit open"));
    }
    assert_eq!(
        reg.breaker_state("err_tool"),
        Some(BreakerState::Open),
        "exempting the caller's failures must not disarm the breaker"
    );
    let blocked = reg.dispatch("err_tool", input()).await;
    assert!(
        blocked.content.contains("circuit open"),
        "a tool that keeps breaking must still be backed off; got: {}",
        blocked.content
    );
}

/// The path the agent's own tool loop actually uses.
///
/// `wcore_agent::orchestration` calls `get()` + `execute_with_ctx()` directly
/// and reports the outcome with `record_dispatch_outcome`, so a fix that only
/// covered `ToolDispatcher::dispatch` would not have reached the run that
/// failed. Both halves are pinned here.
#[tokio::test]
async fn the_agent_dispatch_recorder_ignores_the_callers_failed_command() {
    use wcore_tools::bash::BashTool;
    use wcore_types::tool::ToolResult;

    let mut reg = ToolRegistry::new();
    reg.register(Box::new(BashTool));

    let completed = ToolResult {
        content: "Exit code: 7\nSTDOUT:\n\nSTDERR:\n".to_string(),
        is_error: true,
    };
    let refused = ToolResult {
        content: "Command refused, nothing ran: a Windows `cmd /C` command \
                  line cannot carry a line break."
            .to_string(),
        is_error: true,
    };
    for _ in 0..3 {
        reg.record_dispatch_outcome("Bash", &completed);
        reg.record_dispatch_outcome("Bash", &refused);
    }
    assert_eq!(
        reg.breaker_state("Bash"),
        Some(BreakerState::Closed),
        "six of the caller's own failures must leave the shell available"
    );

    // Control: the failures the breaker exists for still reach it through the
    // same recorder, so this exemption cannot be hiding a dead guard.
    let wedged = ToolResult {
        content: "Command timed out after 120000ms".to_string(),
        is_error: true,
    };
    for _ in 0..3 {
        reg.record_dispatch_outcome("Bash", &wedged);
    }
    assert_eq!(
        reg.breaker_state("Bash"),
        Some(BreakerState::Open),
        "three wedged children must still open the breaker"
    );
}
