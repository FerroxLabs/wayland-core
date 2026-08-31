//! Regression guard for the per-byte cost of one turn — FerroxLabs/wayland-core#395 c3
//! and FerroxLabs/wayland#1235 c4.
//!
//! ## What was measured, and why this file exists
//!
//! `AgentEngine::run()` costs a fixed few milliseconds plus a term that is
//! LINEAR in the size of the tool result carried through the turn. Measured on
//! hetzner-dsm (96 cores, host load 42-88 recorded per reading), payload shed
//! OFF so every size takes one identical code path, four sizes 60,000 ->
//! 480,000 chars:
//!
//! | profile | exponent (log-log fit) | R^2    | per-byte slope |
//! |---------|------------------------|--------|----------------|
//! | release | 0.986                  | 0.9987 | 6.49 s/MB      |
//! | debug   | 1.022                  | 0.9996 | 97.18 s/MB     |
//!
//! So the term SURVIVES `--release` — it is a product cost a real user pays on
//! `Read` of a large file or a verbose `Bash`, not a test-profile artifact —
//! and it is linear, not superlinear.
//!
//! Bisecting instrumentation inside the turn named the carrier by measurement,
//! not by reading: `wcore_safety::PIIScrubber::scrub` is 99.8% of `run()`
//! (3.004s of 3.018s at 480,000 chars, release), and inside it
//! `decoded_contains_secret` is 99.1% of the whole turn (46.106s of 46.530s,
//! debug). That function base64-decodes the ENTIRE candidate run under four
//! alphabets and re-scans each decode. The engine scrubs each tool result
//! TWICE per turn, so a turn costs ~2 whole-payload scrub passes.
//!
//! Ruled out by the same measurement, so nobody re-derives them:
//! `estimate_tokens_from_messages` is 0.0000s over 6-14 calls, the #636 shed is
//! 0.0013s, `scrub_direct` is 0.0015s, and fixture construction and the
//! read-back through the jail are 0.001s each.
//!
//! ## What this guard asserts, and why it is a RATIO
//!
//! An absolute wall-clock bound cannot work here: the same term is 15x larger
//! in debug than release, and this box is shared, so any constant would either
//! flake or be so loose it catches nothing. Instead the guard measures, in the
//! SAME process seconds apart, both
//!
//!   * the engine's own per-byte term, as `t(2N) - t(N)` over the production
//!     `AgentEngine::run()` path, and
//!   * the cost of ONE `PIIScrubber::scrub` pass over N bytes of the same
//!     payload,
//!
//! and asserts their ratio. `N` cancels, so the ratio is the number of
//! whole-payload scrub passes one turn costs — 2.0 today. Machine speed, build
//! profile and load cancel with it.
//!
//! It therefore fails if the turn grows a third whole-payload pass over the
//! tool result (ratio -> 3+), and its LOWER bound fails if the scrub is
//! dropped from the turn to make it fast (ratio -> 0), which is the failure a
//! pure "make it faster" fix would otherwise ship silently.
//! `the_turn_still_redacts_a_secret_and_leaves_ordinary_output_alone` closes
//! the same hole from the other side.

mod common;

use std::sync::Arc;
use std::time::Instant;

use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::terminal::TerminalSink;
use wcore_safety::PIIScrubber;
use wcore_tools::registry::ToolRegistry;
use wcore_tools::vfs::{RealFs, SandboxedFs};
use wcore_tools::workspace_policy::WorkspacePolicy;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{ContentBlock, StopReason, TokenUsage};

use common::{MockLlmProvider, MockTool, test_config};

/// Small enough that the whole guard runs in a few seconds even in a debug
/// build (the profile CI uses), large enough that the per-byte term dominates
/// the fixed cost of a turn. `SMALL` and `2 * SMALL` are both multiples of 4,
/// which keeps the payload a valid base64 candidate at both sizes so the two
/// arms take the identical code path.
const SMALL: usize = 8_192;

fn silent_output() -> Arc<dyn OutputSink> {
    Arc::new(TerminalSink::new(true))
}

/// The payload shape the cost is worst for and that a real tool hits: one
/// unbroken run of base64-alphabet bytes, which is what a minified file, a
/// hash dump or a base64 blob returned by `Read` looks like.
fn payload(len: usize) -> String {
    "x".repeat(len)
}

fn scripted_turns() -> Vec<Vec<LlmEvent>> {
    let usage = TokenUsage {
        input_tokens: 5_000,
        output_tokens: 100,
        ..Default::default()
    };
    vec![
        vec![
            LlmEvent::ToolUse {
                id: "big".to_string(),
                name: "mock_tool".to_string(),
                input: serde_json::json!({}),
                extra: None,
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                    StopReason::ToolUse,
                ),
                usage: usage.clone(),
            },
        ],
        vec![
            LlmEvent::TextDelta("done".to_string()),
            LlmEvent::Done {
                stop_reason: StopReason::EndTurn,
                finish_reason: wcore_types::message::FinishReason::from_stop_reason(
                    StopReason::EndTurn,
                ),
                usage,
            },
        ],
    ]
}

/// Drive ONE real turn through the production `AgentEngine::run()` with a tool
/// result of `len` bytes, and return the seconds `run()` took.
///
/// The context ceiling is raised far above the payload on purpose: the #636
/// shed is measured at 0.0013s and is NOT what this guard is about, and
/// leaving it off keeps both arms on one identical path.
async fn turn_seconds(len: usize) -> (f64, AgentEngine) {
    let workspace = tempfile::tempdir().expect("workspace");
    let policy = Arc::new(WorkspacePolicy::contained(workspace.path()));
    let provider = Arc::new(MockLlmProvider::with_turns(scripted_turns()));

    let mut config = test_config();
    config.compact.enabled = false;
    config.compact.context_window = Some(100_000_000);
    config.compact.output_reserve = 10_000;
    config.compact.emergency_buffer = 10_000;

    let mut registry = ToolRegistry::new();
    let huge = payload(len);
    registry.register(Box::new(
        MockTool::new("mock_tool", &huge, false).with_max_result_size(4_000_000),
    ));
    registry.set_tool_vfs(Arc::new(SandboxedFs::new(RealFs, workspace.path())));
    registry.set_workspace_policy(Arc::clone(&policy));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, silent_output());
    let start = Instant::now();
    engine.run("go", "msg-1").await.expect("run");
    let elapsed = start.elapsed().as_secs_f64();
    (elapsed, engine)
}

/// Seconds for ONE `PIIScrubber::scrub` pass over `len` bytes of the same
/// payload. This is the unit the engine's per-byte term is expressed in.
fn one_scrub_pass_seconds(len: usize) -> f64 {
    let text = payload(len);
    let start = Instant::now();
    let scrubbed = PIIScrubber.scrub(&text);
    let elapsed = start.elapsed().as_secs_f64();
    // Keep the result observable so nothing here can be optimised away.
    assert!(!scrubbed.is_empty());
    elapsed
}

/// Compile the scrubber's lazy `OnceLock` regex sets before anything is timed.
/// Measured: the FIRST scrub in a debug process paid ~4s of one-off regex
/// compilation, which is a fixed cost and would otherwise land entirely in
/// whichever arm ran first.
fn warm_up() {
    let _ = PIIScrubber.scrub(&payload(4_096));
}

#[tokio::test]
async fn one_turn_costs_about_two_whole_payload_scrub_passes() {
    warm_up();

    // Three interleaved ROUNDS. Each round measures the two turns and the
    // reference scrub adjacent in time, so a burst of co-tenant load lands on
    // all three and largely cancels inside that round's ratio; the median then
    // discards a round where it did not. Measured on hetzner-dsm with the box
    // deliberately oversubscribed (48 spinners on 96 cores, load 43->75), a
    // per-sample `min` estimator produced ratios of 2.04/2.30/2.07/0.98 — one
    // of which is a false red and one of which is close to a false green.
    let mut ratios: Vec<f64> = Vec::new();
    let (mut small, mut large, mut scrub) = (f64::MAX, f64::MAX, f64::MAX);
    for _ in 0..3 {
        let round_small = turn_seconds(SMALL).await.0;
        let round_large = turn_seconds(2 * SMALL).await.0;
        let round_scrub = one_scrub_pass_seconds(SMALL);
        ratios.push((round_large - round_small) / round_scrub);
        small = small.min(round_small);
        large = large.min(round_large);
        scrub = scrub.min(round_scrub);
    }
    ratios.sort_by(|a, b| a.partial_cmp(b).expect("no NaN timings"));

    let passes = ratios[1];
    eprintln!(
        "TURN-COST min_small={small:.4}s min_large={large:.4}s min_scrub={scrub:.4}s ratios={ratios:.2?} median_passes={passes:.2}"
    );

    assert!(
        passes < 2.5,
        "one turn now costs {passes:.2} whole-payload PIIScrubber passes per byte of tool \
         result, up from the 2.0 measured for wayland-core#395. Something added another \
         full scan of the tool result to the turn loop. small={small:.4}s \
         large={large:.4}s one_scrub={scrub:.4}s"
    );
    assert!(
        passes > 1.5,
        "one turn now costs only {passes:.2} whole-payload PIIScrubber passes per byte of \
         tool result, down from 2.0. That is not a free win: the most likely cause is that \
         tool output stopped being scrubbed on this path. Check \
         `the_turn_still_redacts_a_secret_and_leaves_ordinary_output_alone` before \
         relaxing this bound. small={small:.4}s large={large:.4}s one_scrub={scrub:.4}s"
    );

    // Shape, not just magnitude. The term is linear (exponent 0.986 release /
    // 1.022 debug), so doubling the payload must not much more than double the
    // turn. The fixed per-turn cost is included here and biases this ratio
    // DOWN, so the bound stays sound as an upper limit.
    assert!(
        large < 2.8 * small,
        "turn cost is no longer ~linear in tool-result size: {SMALL} bytes took {small:.4}s \
         and {} bytes took {large:.4}s ({:.2}x). wayland#1235 c2 measured exponent 0.986 \
         (release) / 1.022 (debug); a superlinear term has been reintroduced.",
        2 * SMALL,
        large / small
    );
}

/// The control for the lower bound above. A "fix" that makes the turn cheap by
/// not scrubbing tool output would satisfy a naive speed guard; it must not
/// satisfy this one. The second half is the wrong-refusal control: ordinary
/// tool output must still arrive intact, so scrubbing EVERYTHING is not a pass
/// either.
#[tokio::test]
async fn the_turn_still_redacts_a_secret_and_leaves_ordinary_output_alone() {
    let secret = format!("ghp_{}", "A".repeat(36));
    let benign = "total 12 drwxr-xr-x 2 root root 4096 Aug 31 11:44 build.log";
    let tool_output = format!("{benign}\nGITHUB_TOKEN={secret}\n{benign}");

    let workspace = tempfile::tempdir().expect("workspace");
    let policy = Arc::new(WorkspacePolicy::contained(workspace.path()));
    let provider = Arc::new(MockLlmProvider::with_turns(scripted_turns()));

    let mut config = test_config();
    config.compact.enabled = false;
    config.compact.context_window = Some(100_000_000);

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(
        MockTool::new("mock_tool", &tool_output, false).with_max_result_size(4_000_000),
    ));
    registry.set_tool_vfs(Arc::new(SandboxedFs::new(RealFs, workspace.path())));
    registry.set_workspace_policy(Arc::clone(&policy));

    let mut engine = AgentEngine::new_with_provider(provider, config, registry, silent_output());
    engine.run("go", "msg-1").await.expect("run");

    let result = engine
        .conversation_messages()
        .iter()
        .flat_map(|message| message.content.iter())
        .find_map(|block| match block {
            ContentBlock::ToolResult { content, .. } => Some(content.clone()),
            _ => None,
        })
        .expect("the turn must carry a tool result");

    assert!(
        !result.contains(&secret),
        "the tool result reached history with a live GitHub PAT in it — tool output is no \
         longer scrubbed on the turn path, which is also how the per-byte cost guard would \
         be made to pass by accident. Got: {result}"
    );
    assert!(
        result.contains("[REDACTED:"),
        "nothing in the tool result was redacted; expected a redaction marker. Got: {result}"
    );
    assert!(
        result.contains(benign),
        "ordinary tool output was destroyed by the scrub. A guard that refuses real traffic \
         is worse than the cost it saves. Got: {result}"
    );
}
