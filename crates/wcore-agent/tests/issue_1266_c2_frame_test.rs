//! FerroxLabs/wayland#1266 c2 — a test PER CATEGORY asserting an in-band
//! **frame**, plus the control.
//!
//! # Why this file exists next to `issue_1266_in_band_category_test.rs`
//!
//! That file grades the in-band seam through a real `AgentEngine::run`, which
//! is the right shape, but it stops one layer short of c2's own wording in two
//! ways, and both were refuted rather than argued away:
//!
//! 1. **`LocalWayland` had ZERO in-band coverage.** `FailureCategory` has four
//!    variants; that file covers `ContextLimit`, `ToolRuntime` and the
//!    `Unknown` control. `LocalWayland` is the LARGEST group the classification
//!    commit touched (every budget and spend-guard refusal, session-persistence
//!    faults, the mid-flight monitor, the CLI's refused/malformed host commands
//!    and its startup failure) and nothing drove one in band. The only
//!    `LocalWayland` assertions in the tree are #1237's, which call
//!    `AgentError::failure_category()` on a TERMINAL exit — the exact shape
//!    #1266's own test-file header says it refuses as evidence.
//!
//! 2. **Nothing asserted a FRAME.** Every assertion there lands on a
//!    `(message, category)` tuple recorded by `CatSink`, a test double defined
//!    in the test file. A category that only ever exists as a Rust enum inside
//!    a test's own sink is not a category any host can branch on. c2 says
//!    "asserting an in-band frame", and a tuple is not a frame.
//!
//! # The instrument
//!
//! A real `AgentEngine::run` over a scripted provider, wired to the real
//! production `ProtocolSink` — the sink that builds the host's `error` frame —
//! over a recording [`ProtocolEmitter`]. Every assertion below is made on
//! `serde_json::Value` parsed back out of the **bytes** the emitter received,
//! never on a Rust value.
//!
//! [`FrameRecorder::emit`] is byte-for-byte the body of the production
//! `ProtocolWriter::emit` (`serde_json::to_vec(event)` + `b'\n'`), which is
//! also why the two are compared in
//! [`the_recorder_encodes_a_frame_the_way_the_production_writer_does`]. What
//! this file does NOT cover is the transport below that call —
//! `OutputPump`'s write to stdout, which never inspects the payload. That last
//! hop is covered for real, out of a spawned process's stdout, by
//! `crates/wcore-cli/tests/issue_1266_c1_host_frame_e2e.rs`.
//!
//! # Red arms (recorded, re-runnable)
//!
//! Each is a one-line mutation of the production site in
//! `crates/wcore-agent/src/engine.rs`, `touch`ed and rebuilt after the edit:
//!
//! * `FailureCategory::ContextLimit` → `Unknown` at the unworkable-window
//!   refusal reddens [`the_context_ceiling_refusal_reaches_the_host_frame_as_context_limit`].
//! * `FailureCategory::ToolRuntime` → `Unknown` at the tool-failure breaker
//!   reddens [`the_tool_breaker_reaches_the_host_frame_as_tool_runtime`].
//! * `FailureCategory::LocalWayland` → `Unknown` at the budget-reservation
//!   refusal reddens [`a_budget_refusal_reaches_the_host_frame_as_local_wayland`].
//!
//! Each mutation also reddens [`the_in_band_frames_carry_more_than_one_category`],
//! which is the guard against a fix that satisfies one arm with a constant.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use parking_lot::Mutex as PMutex;
use serde_json::Value;
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_budget::{BudgetCap, BudgetTracker};
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::writer::ProtocolEmitter;
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::test_config;

// ---------------------------------------------------------------------------
// The recorder. Encodes exactly as `ProtocolWriter::emit` does, and keeps the
// BYTES rather than the event, so nothing downstream of this file can assert
// against an in-process enum by accident.
// ---------------------------------------------------------------------------
#[derive(Default)]
struct FrameRecorder {
    bytes: Mutex<Vec<u8>>,
}

impl ProtocolEmitter for FrameRecorder {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        let mut encoded = serde_json::to_vec(event)
            .map_err(|e| std::io::Error::other(format!("failed to serialize: {e}")))?;
        encoded.push(b'\n');
        self.bytes.lock().unwrap().extend_from_slice(&encoded);
        Ok(())
    }
}

impl FrameRecorder {
    /// Parse the recorded stream the way the host does: JSON Lines.
    fn frames(&self) -> Vec<Value> {
        let bytes = self.bytes.lock().unwrap().clone();
        String::from_utf8(bytes)
            .expect("the host reads UTF-8 JSON Lines")
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str::<Value>(line)
                    .unwrap_or_else(|e| panic!("host cannot parse frame as JSON: {e}\n{line}"))
            })
            .collect()
    }

    /// Every `error` frame's `category` STRING, in emission order.
    ///
    /// Read out of the JSON, and `<absent>` rather than a default when the key
    /// is missing: a frame that omits the field entirely is a different defect
    /// from one that carries `"unknown"`, and collapsing them would hide it.
    fn error_categories(&self) -> Vec<String> {
        self.error_frames()
            .iter()
            .map(|error| {
                error
                    .get("category")
                    .and_then(Value::as_str)
                    .unwrap_or("<absent>")
                    .to_string()
            })
            .collect()
    }

    fn error_frames(&self) -> Vec<Value> {
        self.frames()
            .into_iter()
            .filter(|frame| frame.get("type").and_then(Value::as_str) == Some("error"))
            .filter_map(|frame| frame.get("error").cloned())
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Scripted provider.
// ---------------------------------------------------------------------------
type Outcome = Result<Vec<LlmEvent>, ProviderError>;

struct ScriptedProvider {
    script: Mutex<std::collections::VecDeque<Outcome>>,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(&self, _r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        let next = self
            .script
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| Ok(failing_tool_call_turn()));
        let events = next?;
        let (tx, rx) = mpsc::channel(64);
        tokio::spawn(async move {
            for event in events {
                if tx.send(event).await.is_err() {
                    break;
                }
            }
        });
        Ok(rx)
    }
}

fn usage() -> TokenUsage {
    TokenUsage {
        input_tokens: 10,
        output_tokens: 5,
        ..Default::default()
    }
}

fn end_turn_text(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ]
}

/// A tool-call turn naming a tool that does not exist, so the call always
/// fails. Enough of these in a row trip the consecutive-failure breaker.
fn failing_tool_call_turn() -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: "call-1".into(),
            name: "no_such_tool".to_string(),
            input: serde_json::json!({}),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ]
}

/// Run one real turn loop into a real `ProtocolSink`, and hand back the frames
/// the host would have received.
///
/// `mutate_config` arms the exit under test; `mutate_engine` is the hook the
/// budget arm needs, because a tracker is installed on the engine by
/// `spawner.rs`'s production child launch rather than read out of `Config`.
async fn frames_from_a_real_run(
    script: Vec<Outcome>,
    mutate_config: impl FnOnce(&mut wcore_config::config::Config),
    mutate_engine: impl FnOnce(&mut AgentEngine),
) -> Arc<FrameRecorder> {
    let provider = Arc::new(ScriptedProvider {
        script: Mutex::new(script.into_iter().collect()),
    });
    let mut config = test_config();
    mutate_config(&mut config);

    let recorder = Arc::new(FrameRecorder::default());
    let sink = Arc::new(ProtocolSink::with_emitter(
        Arc::clone(&recorder) as Arc<dyn ProtocolEmitter>
    ));
    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        sink as Arc<dyn OutputSink>,
    );
    mutate_engine(&mut engine);
    let _ = engine.run("hello", "msg-1266-c2").await;
    recorder
}

/// The exactly-one-error-frame reader every arm uses, with the vacuity control
/// built in: an arm that stopped exercising its branch fails HERE, loudly,
/// rather than passing on an empty set.
fn the_single_error_frame(recorder: &FrameRecorder, case: &str) -> Value {
    let errors = recorder.error_frames();
    assert!(
        !errors.is_empty(),
        "{case}: control — the in-band error must reach the host as an `error` \
         frame at all. If this fires, the harness stopped exercising the branch \
         and every category assertion would have passed vacuously. Frames \
         seen: {:?}",
        recorder.frames()
    );
    errors[0].clone()
}

// ---------------------------------------------------------------------------
// The four categories, each asserted on the FRAME.
// ---------------------------------------------------------------------------

/// c2 — `context_limit`, on the frame.
///
/// `[compact] context_window` below the baseline turn trips the engine's
/// unworkable-window refusal, an IN-BAND `emit_error`: the run returns
/// normally and the host is told out of band of any terminal exit.
#[tokio::test]
async fn the_context_ceiling_refusal_reaches_the_host_frame_as_context_limit() {
    let recorder = frames_from_a_real_run(
        vec![Ok(end_turn_text("unreachable"))],
        // Far below `minimum_workable_window()`: the refusal fires before any
        // provider call, so this cannot be flaky on the network.
        |config| config.compact.context_window = Some(1_024),
        |_| {},
    )
    .await;

    let error = the_single_error_frame(&recorder, "context-ceiling");
    assert_eq!(
        error.get("category").and_then(Value::as_str),
        Some("context_limit"),
        "wayland#1266 c2: the engine refused precisely BECAUSE the window is \
         unworkable, and the host frame must say so. Frame was: {error}"
    );
    // The prose half is an addition, not a replacement — a "fix" that carried
    // the category by rewriting the message would still be a regression.
    assert!(
        error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|m| m.contains("cannot operate in a window that small")),
        "the category is an ADDITION to the prose: {error}"
    );
}

/// c2 — `tool_runtime`, on the frame. The tool-failure breaker is the example
/// #1266's own body gives for "an engine error the engine itself classified".
#[tokio::test]
async fn the_tool_breaker_reaches_the_host_frame_as_tool_runtime() {
    // The consecutive-failure breaker trips at 10 by default and the
    // no-progress guard trips earlier on identical repeats; both are
    // `tool_runtime`. `max_turns` is well above either so the run cannot end
    // on the turn cap before a breaker fires — and if it did, the `expect`
    // below says so rather than passing.
    let script: Vec<Outcome> = (0..24).map(|_| Ok(failing_tool_call_turn())).collect();
    let recorder =
        frames_from_a_real_run(script, |config| config.max_turns = Some(40), |_| {}).await;

    let errors = recorder.error_frames();
    let breaker = errors
        .iter()
        .find(|error| {
            error
                .get("message")
                .and_then(Value::as_str)
                .is_some_and(|m| m.contains("tool calls failed") || m.contains("no-progress loop"))
        })
        .unwrap_or_else(|| {
            panic!(
                "control: no breaker frame was captured, so this test would \
                 grade nothing. Error frames seen: {errors:?}"
            )
        });
    assert_eq!(
        breaker.get("category").and_then(Value::as_str),
        Some("tool_runtime"),
        "wayland#1266 c2: a tool breaker firing is #388's `tool/runtime \
         failure` and the engine knows it at the call site. Frame was: {breaker}"
    );
}

/// c2 — `local_wayland`, on the frame. THE ARM THAT DID NOT EXIST.
///
/// A session-output-token ceiling makes the engine's pre-flight admission
/// reservation fail, and the engine refuses to start the provider call. That
/// is #388's "local Wayland error" in its purest form: nothing upstream is
/// implicated, the local process declined on its own account. It is also the
/// single largest family the classification commit touched, and until this
/// test nothing drove one of them in band.
///
/// The tracker is installed through `AgentEngine::set_budget_tracker`, which
/// is the same call `spawner.rs` makes on the production child-launch path —
/// not a test-only door.
#[tokio::test]
async fn a_budget_refusal_reaches_the_host_frame_as_local_wayland() {
    let recorder = frames_from_a_real_run(
        vec![Ok(end_turn_text("unreachable"))],
        |_| {},
        |engine| {
            // One output token for the whole session, against a request that
            // reserves `max_tokens` (4096) worth: the reservation cannot be
            // admitted, so the refusal fires before any provider call and
            // cannot be flaky on the network.
            let cap = BudgetCap::builder().per_session_output_tokens(1).build();
            engine.set_budget_tracker(Arc::new(PMutex::new(BudgetTracker::new(cap))));
        },
    )
    .await;

    let error = the_single_error_frame(&recorder, "budget-refusal");
    assert!(
        error
            .get("message")
            .and_then(Value::as_str)
            .is_some_and(|m| m.contains("budget cap")),
        "control: the frame under test must be the budget refusal, not some \
         other error that happened to arrive first: {error}"
    );
    assert_eq!(
        error.get("category").and_then(Value::as_str),
        Some("local_wayland"),
        "wayland#1266 c2: a spend-guard refusal is a LOCAL refusal — the local \
         process declined on its own account and nothing upstream is \
         implicated. Frame was: {error}"
    );
}

/// c2's CONTROL, on the frame — a genuinely unclassifiable in-band error is
/// still `unknown`, not given a plausible-looking value.
///
/// An opaque non-2xx is exactly the #1184 rate-limit-versus-router split that
/// is not decidable from inside this repo, and #1237 c4 forbids guessing it.
/// If a later change starts "helpfully" classifying these, this reddens.
#[tokio::test]
async fn an_opaque_upstream_failure_stays_unknown_on_the_frame() {
    // A 400 rather than a 503: a client error is not retried, so this reaches
    // the same opaque-upstream exit without waiting out the retry backoff.
    // What makes it the right control is not the status but that core cannot
    // see past it.
    let script: Vec<Outcome> = (0..4)
        .map(|_| {
            Err(ProviderError::Api {
                status: 400,
                message: "{\"error\":{\"message\":\"upstream said no\"}}".to_string(),
            })
        })
        .collect();
    let recorder = frames_from_a_real_run(script, |_| {}, |_| {}).await;

    let categories = recorder.error_categories();
    assert!(
        !categories.is_empty(),
        "control: the exhausted-provider path must reach the host at all"
    );
    for category in &categories {
        assert_eq!(
            category, "unknown",
            "wayland#1266 c2 control: core cannot tell a provider rate limit \
             from a router failure (#1184) and must SAY so rather than pick \
             one. Categories on the wire: {categories:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Anti-constant guard and encoder fidelity.
// ---------------------------------------------------------------------------

/// The three tests above each assert ONE value, so a seam wired to emit that
/// value everywhere would satisfy each of them in isolation. This asserts the
/// three in-band exits put three DIFFERENT strings on the wire, which no
/// single-constant implementation can pass.
#[tokio::test]
async fn the_in_band_frames_carry_more_than_one_category() {
    let ceiling = frames_from_a_real_run(
        vec![Ok(end_turn_text("x"))],
        |config| config.compact.context_window = Some(1_024),
        |_| {},
    )
    .await;
    let budget = frames_from_a_real_run(
        vec![Ok(end_turn_text("x"))],
        |_| {},
        |engine| {
            let cap = BudgetCap::builder().per_session_output_tokens(1).build();
            engine.set_budget_tracker(Arc::new(PMutex::new(BudgetTracker::new(cap))));
        },
    )
    .await;
    let opaque = frames_from_a_real_run(
        (0..4)
            .map(|_| {
                Err(ProviderError::Api {
                    status: 400,
                    message: "opaque".to_string(),
                })
            })
            .collect(),
        |_| {},
        |_| {},
    )
    .await;

    let mut seen: Vec<String> = vec![
        ceiling
            .error_categories()
            .first()
            .cloned()
            .unwrap_or_default(),
        budget
            .error_categories()
            .first()
            .cloned()
            .unwrap_or_default(),
        opaque
            .error_categories()
            .first()
            .cloned()
            .unwrap_or_default(),
    ];
    assert!(
        seen.iter().all(|c| !c.is_empty()),
        "control: all three runs must have produced an error frame: {seen:?}"
    );
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen.len(),
        3,
        "three different in-band exits put fewer than three distinct categories \
         on the wire, so the seam is carrying a constant rather than a \
         classification: {seen:?}"
    );
}

/// The recorder is only evidence if it encodes a frame the way production
/// does. This asserts the bytes it stores are byte-identical to
/// `serde_json::to_vec` of the same event plus a newline — the entire body of
/// `ProtocolWriter::emit` — so "what the recorder saw" and "what the host
/// would have read" are the same string.
#[test]
fn the_recorder_encodes_a_frame_the_way_the_production_writer_does() {
    let event = ProtocolEvent::Error {
        msg_id: None,
        error: wcore_protocol::events::ErrorInfo {
            code: "engine_error".to_string(),
            message: "probe".to_string(),
            retryable: false,
            category: wcore_protocol::events::FailureCategory::LocalWayland,
        },
    };
    let recorder = FrameRecorder::default();
    recorder.emit(&event).expect("recorder accepts the frame");

    let mut expected = serde_json::to_vec(&event).expect("production encoding");
    expected.push(b'\n');
    assert_eq!(
        recorder.bytes.lock().unwrap().as_slice(),
        expected.as_slice(),
        "the recorder must store the bytes ProtocolWriter::emit would write"
    );
    // And the field the whole file turns on is really a snake_case STRING on
    // the wire, not a number or a nested object.
    assert_eq!(
        recorder.error_categories(),
        vec!["local_wayland".to_string()],
        "the host reads `category` as a snake_case string"
    );
}
