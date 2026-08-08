//! A red shell exit is not a Bash fault.
//!
//! `ToolResult` carries exactly one failure bit, `is_error`, and it is
//! overloaded: it means both "this tool malfunctioned" and "this tool worked
//! perfectly and is faithfully reporting that the child exited non-zero".
//! `orchestration::execute_*` fed that bit straight to the per-tool circuit
//! breaker, so three ordinary red commands inside the 30 s window (three
//! failing test runs, three failed builds) opened the breaker and Bash was
//! short-circuited for the rest of the run. `reset_all_breakers()` fires only
//! at the top of a USER turn, and a headless `-p` run is ONE user turn, so the
//! 60 s cooldown admits roughly one trial Bash call per minute for the rest of
//! the run — and a red trial immediately re-opens it. That is precisely the
//! red -> fix -> rerun loop the product exists to drive.
//!
//! Both directions are pinned here. A "fix" that made Bash report non-zero
//! exits as success would silence the breaker AND lie to the model about a red
//! suite; the `is_error` / `Exit code: 7` assertions on the RED calls exist to
//! fail that one.

use std::sync::{Arc, Mutex};

use serde_json::json;
use wcore_agent::confirm::ToolConfirmer;
use wcore_agent::orchestration::{StreamingContext, execute_tool_calls_with_streaming};
use wcore_agent::output::OutputSink;
use wcore_config::circuit_breaker::BreakerState;
use wcore_tools::dispatcher::ToolDispatcher;
use wcore_types::message::{ContentBlock, FinishReason};

/// Distinct canaries. Reusing one string for the red calls and the rerun would
/// make the suite vacuous: the canary is already present in results 1-3 on
/// unfixed code, so any assertion against an aggregate would pass today.
const RED_CANARY: &str = "WLC_BREAKER_RED_4F91B7";
const GREEN_CANARY: &str = "WLC_BREAKER_GREEN_2C08D3";

/// `ToolRegistry::new()` installs `FailClosedBackend`, and orchestration builds
/// every `ToolContext` from `registry.sandbox_runtime()`. Without a usable
/// backend EVERY Bash call returns
/// "Failed to execute command: sandbox UNAVAILABLE ..." — which the fix
/// correctly still classes as a genuine tool fault, so the breaker would open
/// either way and the test would prove nothing. Precedent:
/// `tool_chunk_streaming.rs`, `tool_result_fidelity_test.rs`.
fn make_registry() -> wcore_tools::registry::ToolRegistry {
    let mut reg = wcore_tools::registry::ToolRegistry::new();
    reg.set_sandbox_runtime(Arc::new(wcore_sandbox::SandboxRegistry::new(Arc::new(
        wcore_sandbox::backends::no_sandbox::NoSandboxBackend::new(),
    ))));
    reg.register(Box::new(wcore_tools::bash::BashTool));
    assert!(
        reg.get("Bash").is_some(),
        "Bash must register or the test is vacuous"
    );
    reg
}

fn bash_call(id: &str, command: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: "Bash".into(),
        input: json!({ "command": command }),
        extra: None,
    }
}

/// `sh -c` and `cmd /C` both honour `&&` and `exit N`.
fn red_command() -> String {
    format!("echo {RED_CANARY} && exit 7")
}

fn green_command() -> String {
    format!("echo {GREEN_CANARY}")
}

/// A sink that advertises streaming, so orchestration takes the
/// `execute_streaming_with_ctx` branch — the one production actually runs,
/// since `BashTool::supports_streaming()` is true and the live CLI advertises
/// streaming. Without this leg the shipped path stays unproven.
#[derive(Default)]
struct StreamingSink;

impl OutputSink for StreamingSink {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool) {}
    fn emit_info(&self, _: &str) {}
    fn emit_tool_chunk(&self, _: &str, _: &str, _: &str, _: &str) {}
    fn streaming_tools_advertised(&self) -> bool {
        true
    }
}

/// Dispatch one call and return `(content, is_error)` for exactly that
/// `tool_use_id` — never an aggregate over every call made so far.
async fn dispatch_one(
    registry: &wcore_tools::registry::ToolRegistry,
    confirmer: &Arc<Mutex<ToolConfirmer>>,
    id: &str,
    command: &str,
    streaming: bool,
) -> (String, bool) {
    let calls = vec![bash_call(id, command)];
    let outcome = execute_tool_calls_with_streaming(
        registry,
        &calls,
        confirmer,
        None,
        wcore_compact::CompactionLevel::default(),
        false,
        streaming.then(|| StreamingContext {
            output: Arc::new(StreamingSink) as Arc<dyn OutputSink>,
            msg_id: "m-1".into(),
        }),
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .expect("dispatch must produce a tool result");

    outcome
        .results
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } if tool_use_id == id => Some((content.clone(), *is_error)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no result for {id}"))
}

async fn three_reds_then_a_rerun(streaming: bool) {
    let registry = make_registry();
    let confirmer = Arc::new(Mutex::new(ToolConfirmer::new(true, vec![])));

    for i in 0..3 {
        let id = format!("red-{i}");
        let (content, is_error) =
            dispatch_one(&registry, &confirmer, &id, &red_command(), streaming).await;

        // POSITIVE CONTROL: a real child ran on THIS call, so the test cannot
        // pass because Bash silently never executed.
        #[cfg(unix)]
        assert!(
            content.contains(RED_CANARY),
            "red {i} must have run a real child: {content}"
        );
        // ANTI-OVER-FIX: the model must still be told the command went red.
        assert!(is_error, "red {i} must still be is_error for the LLM");
        assert!(
            content.contains("Exit code: 7"),
            "red {i} must report the child's exit code: {content}"
        );
    }

    // The rerun the whole defect is about.
    let (content, is_error) =
        dispatch_one(&registry, &confirmer, "green", &green_command(), streaming).await;

    #[cfg(unix)]
    assert!(
        content.contains(GREEN_CANARY),
        "the rerun's child must actually run: {content}"
    );
    assert!(
        !content.contains("circuit open"),
        "Bash was short-circuited by the breaker after three ordinary red \
         commands: {content}"
    );
    assert!(!is_error, "the rerun succeeded: {content}");

    // Non-mutating state check. `breaker_is_open()` transitions
    // Open -> HalfOpen and consumes the single trial permit as a side effect,
    // so it is wrong as a predicate.
    assert_eq!(
        registry.breaker_state("Bash"),
        Some(BreakerState::Closed),
        "three red shell exits must not open the Bash breaker"
    );
}

/// Non-streaming dispatch (`execute_with_ctx`).
#[tokio::test]
async fn red_shell_exits_leave_bash_usable_for_the_rerun() {
    three_reds_then_a_rerun(false).await;
}

/// Streaming dispatch (`execute_streaming_with_ctx`) — the branch a live CLI
/// session actually takes.
#[tokio::test]
async fn red_shell_exits_leave_streaming_bash_usable_for_the_rerun() {
    three_reds_then_a_rerun(true).await;
}

/// The exemption must stay keyed on a marker the TOOL constructs, never on
/// "does this look like a shell result". A Bash result with no child status —
/// a backend error, a timeout, a cancellation, a credential-denylist refusal,
/// a missing parameter — is still a genuine fault and must still trip the
/// breaker after three. The credential-denylist case matters most: the breaker
/// is the incidental rate limit that stops a prompt-injected model grinding at
/// the Wave-SA exfiltration denylist.
#[tokio::test]
async fn genuine_bash_faults_still_open_the_breaker() {
    let mut reg = wcore_tools::registry::ToolRegistry::new();
    // Deliberately NO sandbox runtime: `ToolRegistry::new()` leaves
    // `FailClosedBackend` installed, so every call fails with
    // "Failed to execute command: sandbox UNAVAILABLE ..." — a result with no
    // `Exit code: ` head, i.e. a real tool fault.
    reg.register(Box::new(wcore_tools::bash::BashTool));
    let confirmer = Arc::new(Mutex::new(ToolConfirmer::new(true, vec![])));

    for i in 0..3 {
        let id = format!("fault-{i}");
        let (content, is_error) = dispatch_one(&reg, &confirmer, &id, "echo hi", false).await;
        assert!(is_error, "fault {i} must be an error: {content}");
        assert!(
            !content.starts_with("Exit code: "),
            "fault {i} must not carry a child status head: {content}"
        );
    }

    assert_ne!(
        reg.breaker_state("Bash"),
        Some(BreakerState::Closed),
        "three genuine Bash faults must still open the breaker"
    );
}
