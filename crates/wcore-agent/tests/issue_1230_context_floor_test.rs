//! FerroxLabs/wayland#1230 c1 + c2 -- the un-compactable floor of a turn,
//! DERIVED from a real bootstrapped engine rather than assumed, and a guard
//! that reds when the snapshot constant drifts away from it.
//!
//! # Why the floor cannot be a constant
//!
//! `wcore_config::compact::BASELINE_TURN_TOKENS` is a MEASURED SNAPSHOT: 3,118
//! real prompt tokens, read off the `usage` block a real `qwen3:8b` returned to
//! #1172 through a logging proxy. Every small-window decision in the product
//! divides by it. It also moves whenever the system prompt is edited, a
//! built-in tool is added, an MCP server registers, or a skill lands in the
//! index -- and until this file existed, nothing regraded it. #1230 measured it
//! 2.8% low against the same rig.
//!
//! # What is graded here
//!
//! The floor is computed from a REAL `AgentBootstrap` engine -- the production
//! system prompt (constitution, tool-usage guidance, date block, skills index)
//! and the production tool registry -- through the same
//! `uncompactable_floor_tokens` the turn loop calls. A test that built its own
//! prompt and its own two-tool array would red on nothing that matters.
//!
//! # Live figures, hetzner-dsm, 2026-08-31
//!
//! Against a private Ollama on port 21434 (NOT the ambient service on 11434)
//! serving `qwen3:8b` at `CONTEXT 4096`, driven through a logging proxy:
//!
//! | quantity | value |
//! |---|---|
//! | real `usage.prompt_tokens`, turn 1, before the user types | 3,207 |
//! | `BASELINE_TURN_TOKENS` | 3,118 |
//! | `uncompactable_floor_tokens` on the same turn | 3,619 |
//! | `input_ceiling_for_window(4_096)` | 2,527 |
//!
//! The estimator runs high, which is the correct direction for a floor: it
//! refuses slightly early rather than slightly late.

use std::sync::Arc;

use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::compact::estimate::{estimate_request_tokens, uncompactable_floor_tokens};
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compact::{BASELINE_TURN_TOKENS, CompactConfig};
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};

/// The tolerance the drift guard allows between the hardcoded snapshot and the
/// floor this tree actually produces.
///
/// 25% is not a comfort band, it is a TRIPWIRE width. The failure this guards
/// against is the constant silently ceasing to describe the product -- a
/// second built-in tool family, an always-on MCP server, a doubled
/// constitution. Those move the floor by tens of percent, not by three. A
/// tighter band would red on ordinary prompt copy-editing and be disabled
/// within a week, which is how the constant got stale in the first place.
const DRIFT_TOLERANCE: f64 = 0.25;

fn stock_config() -> Config {
    Config {
        provider_label: "openai".into(),
        provider: ProviderType::OpenAI,
        api_key: "sk-test".into(),
        base_url: "http://localhost:0".into(),
        model: "gpt-test-model".into(),
        max_tokens: 1024,
        max_turns: Some(1),
        compat: ProviderCompat::openai_defaults(),
        ..Default::default()
    }
}

fn null_output() -> Arc<dyn wcore_agent::output::OutputSink> {
    Arc::new(NullSink)
}

/// #1230 c1 -- the floor is COMPUTED from the assembled request.
///
/// Three properties, each a different way for a "floor" to be fake:
///
///   1. it is non-zero on a real engine (a floor of 0 would make the gate
///      that consumes it vacuous, and every window would pass);
///   2. it is exactly the system prompt plus the tool schemas -- adding
///      messages does not change it, which is what "un-compactable" means,
///      since the degradation rungs only ever shrink messages;
///   3. it is a LOWER BOUND on the full request estimate, including a request
///      carrying the 26,054-character tool result #1230 measured being
///      discarded.
#[tokio::test]
async fn the_floor_is_derived_from_a_real_assembled_request() {
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(
        stock_config(),
        workdir.path().to_str().expect("utf-8 workdir"),
        null_output(),
    )
    .build()
    .await
    .expect("bootstrap");

    let system = result.engine.system_prompt().to_string();
    let tools = result.engine.tools().to_tool_defs();
    let floor = uncompactable_floor_tokens(&system, &tools);

    assert!(
        !tools.is_empty(),
        "fixture guard: a stock engine must register tools, or the floor this \
         test derives is not the product's floor"
    );
    assert!(
        floor > 0,
        "a real system prompt plus {} tool schemas cannot cost nothing",
        tools.len()
    );

    // (2) the floor ignores messages, and (3) bounds the full estimate.
    let big = vec![wcore_types::message::Message::new(
        wcore_types::message::Role::User,
        vec![wcore_types::message::ContentBlock::ToolResult {
            tool_use_id: "t1".to_string(),
            content: "x".repeat(26_054),
            is_error: false,
        }],
    )];
    let with_messages = estimate_request_tokens(&big, &system, &tools);
    assert_eq!(
        uncompactable_floor_tokens(&system, &tools),
        floor,
        "the floor moved when messages were added -- then it is not the \
         un-compactable part"
    );
    assert!(
        with_messages > floor,
        "a 26,054-character tool result did not raise the request above the \
         floor ({with_messages} vs {floor}) -- the estimator is not seeing it"
    );

    // The live figure this test is quoted beside, restated as an executable
    // claim: this floor does not fit under the input ceiling of the 4,096-slot
    // #1230 measured. If it ever does, #1230 is fixed by other means and this
    // whole lane can be reconsidered.
    let ceiling = CompactConfig::default().input_ceiling_for_window(4_096) as u64;
    assert!(
        ceiling < floor,
        "the derived floor {floor} now fits under the 4,096-slot ceiling \
         {ceiling}; #1230's premise no longer holds and the gate should be \
         re-derived"
    );

    eprintln!(
        "#1230 c1: derived floor = {floor} tokens over {} tool schemas \
         ({} chars of system prompt); BASELINE_TURN_TOKENS = {BASELINE_TURN_TOKENS}",
        tools.len(),
        system.len()
    );
}

/// #1230 c2 -- the drift guard.
///
/// `BASELINE_TURN_TOKENS` gates every small-window decision and is a snapshot
/// of a tree that has moved. This reds when the floor this tree actually
/// produces walks away from it, which is the alternative c2 explicitly allows
/// to deriving the constant at runtime. Deriving it is not open: the constant
/// lives in `wcore-config`, which sits BELOW `wcore-agent` in the crate graph
/// and cannot see a system prompt or a tool registry. Moving it up would
/// invert the dependency; regrading it from above is what this does.
#[tokio::test]
async fn the_baseline_constant_still_describes_this_tree() {
    let workdir = tempfile::TempDir::new().expect("workdir");
    let result = AgentBootstrap::new(
        stock_config(),
        workdir.path().to_str().expect("utf-8 workdir"),
        null_output(),
    )
    .build()
    .await
    .expect("bootstrap");

    let floor = uncompactable_floor_tokens(
        result.engine.system_prompt(),
        &result.engine.tools().to_tool_defs(),
    ) as f64;
    let snapshot = BASELINE_TURN_TOKENS as f64;
    let drift = (floor - snapshot).abs() / snapshot;

    assert!(
        drift <= DRIFT_TOLERANCE,
        "BASELINE_TURN_TOKENS = {BASELINE_TURN_TOKENS} but this tree's real \
         un-compactable floor is {floor:.0} tokens, a drift of {:.1}%. The \
         constant gates every small-window decision in the product; regrade it \
         against a live measurement and update it, do not widen this \
         tolerance.",
        drift * 100.0
    );
}
