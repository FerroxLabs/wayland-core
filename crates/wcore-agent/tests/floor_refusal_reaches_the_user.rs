//! wayland-core#355 — a command-floor refusal must reach the USER, not make
//! the model improvise.
//!
//! The reported incident: the floor refused a command, the model read the
//! refusal as one more failed shell call, and did what a model reasonably does
//! with a transient failure — it tried another route. It staged the work under
//! a temporary directory, wrote at the destination, and answered. The user was
//! never told a policy had fired.
//!
//! Two halves are needed and only both together close it:
//!
//! * The PAYLOAD the model reads must say this is policy and must say stop.
//!   That is [`POLICY_REFUSAL_MARKER`] plus the disclosure directive, and it
//!   has to be in the payload — a `warn!` cannot do this job, because
//!   `RUST_LOG` is unset on a default install (only `ERROR` reaches stderr)
//!   and the model does not read the log under any setting. Three features
//!   have already died on that mistake.
//! * A notice the user sees WITHOUT the model's cooperation. A payload
//!   instruction is only ever as good as compliance, and non-compliance is
//!   precisely the reported incident.
//!
//! These tests drive the REAL `BashTool` through the REAL `AgentEngine` over
//! the REAL `ProtocolSink`, and assert on the JSON Lines a host renders. The
//! model is scripted to improvise exactly as it did in the report, so a tree
//! without the fix reproduces the incident rather than merely failing an
//! assertion.
//!
//! Why the relayed tool result does not settle it on its own. Asserting the
//! refusal string is present in the tool result is what the shipped tree
//! already did, and it is what failed to prevent the incident. That text is
//! the model's own input echoed back for display — `ProtocolSink` relays it as
//! `[Bash error] <payload>` — and it renders as one more red error line among
//! the red error lines of an ordinary build-fix loop: truncated at 500
//! characters by the terminal formatter, folded into a collapsed card in the
//! TUI, and then contradicted by the assistant's own confident closing answer.
//! So [`WireRecorder::user_visible`] holds it apart, and c3 is graded on the
//! session's own account of what happened: the notices and the assistant's
//! words, in the channel the final answer arrives on.

mod common;

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::writer::ProtocolEmitter;
use wcore_tools::bash::BashTool;
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::LlmEvent;
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::{MockLlmProvider, test_config};

/// The policy marker, spelled locally ON PURPOSE.
///
/// This test has to COMPILE against a tree without the fix, or its red arm is
/// a build error instead of a reproduction of the incident. Importing
/// `wcore_config::command_floor::POLICY_REFUSAL_MARKER` would make that
/// impossible. The two spellings are pinned together by
/// `the_policy_marker_spelling_is_pinned_to_its_e2e_test` in `command_floor`.
const POLICY_REFUSAL_MARKER: &str = "[POLICY-REFUSAL: command-floor]";

/// The improvisation, verbatim in shape from the report: the model announces
/// success after routing around the refusal.
const IMPROVISED_ANSWER: &str = "I staged the skill under /tmp, wrote it to the destination and \
                                 cd'd in. The brief is set up.";

/// Records the exact JSON Lines `ProtocolWriter` would have written.
#[derive(Default)]
struct WireRecorder {
    lines: Mutex<Vec<String>>,
}

impl ProtocolEmitter for WireRecorder {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        let line = String::from_utf8(serde_json::to_vec(event).expect("serialize")).expect("utf8");
        self.lines.lock().unwrap().push(line);
        Ok(())
    }
}

impl WireRecorder {
    fn infos(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter(|value| value["type"] == "info")
            .filter_map(|value| value["message"].as_str().map(str::to_owned))
            .collect()
    }

    /// The tool result as the sink relays it for display:
    /// `ProtocolSink::emit_tool_result` writes `[<tool> <status>] <payload>`
    /// into the info channel.
    ///
    /// This frame is the model's own input echoed back, and it is exactly what
    /// the shipped tree already had. Held apart from [`Self::user_visible`] so
    /// c3 cannot be satisfied by the very artefact that failed to prevent the
    /// incident.
    fn tool_relay(&self) -> String {
        self.infos()
            .into_iter()
            .filter(|m| is_tool_relay(m))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// What the user reads as the session's own account of what happened: the
    /// notices, minus the relayed tool output, plus the assistant's words.
    fn user_visible(&self) -> String {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .filter_map(|line| serde_json::from_str::<Value>(line).ok())
            .filter_map(|value| match value["type"].as_str() {
                Some("info") => {
                    let message = value["message"].as_str().unwrap_or_default();
                    (!is_tool_relay(message)).then(|| format!("[notice] {message}"))
                }
                Some("text_delta") => Some(format!(
                    "[assistant] {}",
                    value["text"].as_str().unwrap_or_default()
                )),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// `[<tool> error] ...` / `[<tool> success] ...` — the relay shape written by
/// `ProtocolSink::emit_tool_result`.
fn is_tool_relay(message: &str) -> bool {
    message.starts_with("[Bash error] ") || message.starts_with("[Bash success] ")
}

fn bash_turn(command: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::ToolUse {
            id: "call-1".to_string(),
            name: "Bash".to_string(),
            input: json!({ "command": command }),
            extra: None,
        },
        LlmEvent::Done {
            stop_reason: StopReason::ToolUse,
            finish_reason: FinishReason::from_stop_reason(StopReason::ToolUse),
            usage: TokenUsage::default(),
        },
    ]
}

fn answer_turn(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: TokenUsage::default(),
        },
    ]
}

/// Drive the real engine over the real `BashTool` with a scripted model that
/// runs `command` and then answers `answer`.
async fn run_with_bash(command: &str, answer: &str) -> Arc<WireRecorder> {
    let recorder = Arc::new(WireRecorder::default());
    let sink = ProtocolSink::with_emitter(recorder.clone());
    let output: Arc<dyn OutputSink> = Arc::new(sink);

    let provider = Arc::new(MockLlmProvider::with_turns(vec![
        bash_turn(command),
        answer_turn(answer),
    ]));
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(BashTool));

    let mut engine = AgentEngine::new_with_provider(provider, test_config(), registry, output);
    engine
        .run("set the project up for me", "")
        .await
        .expect("the scripted run completes");

    recorder
}

/// c1 + c2 — the payload the model reads carries the policy marker and the
/// instruction to stop, not merely the name of the rule.
#[tokio::test]
async fn a_floor_refusal_payload_marks_itself_a_policy_decision_and_says_stop() {
    let wire = run_with_bash("cat .git/config", IMPROVISED_ANSWER).await;

    let payload = wire.tool_relay();
    assert!(
        !payload.is_empty(),
        "the Bash call produced no tool result at all"
    );

    assert!(
        payload.contains(POLICY_REFUSAL_MARKER),
        "the refusal the model reads must carry the policy marker `{POLICY_REFUSAL_MARKER}`, \
         otherwise it is indistinguishable from a missing binary or a flaky sandbox. Payload \
         was:\n{payload}"
    );
    assert!(
        payload.contains("policy decision, not a transient tool failure"),
        "the payload must SAY it is a policy decision. Payload was:\n{payload}"
    );
    assert!(
        payload.contains("Do NOT work around it"),
        "the payload must instruct the model to stop rather than route around the refusal — \
         naming the rule is what the shipped tree already did. Payload was:\n{payload}"
    );
    assert!(
        payload.contains("tell the user"),
        "the payload must instruct the model to surface the refusal to the user. Payload \
         was:\n{payload}"
    );
}

/// c3 — the USER-VISIBLE output names the refusal, driven end to end through a
/// real floor trip, and it does so even though the scripted model improvises
/// exactly as it did in the report.
#[tokio::test]
async fn a_floor_refusal_reaches_the_user_even_when_the_model_improvises() {
    let wire = run_with_bash("cat .git/config", IMPROVISED_ANSWER).await;

    let seen = wire.user_visible();
    assert!(
        seen.contains(IMPROVISED_ANSWER),
        "the scripted model must actually have improvised, or this test is not reproducing the \
         incident. User-visible output was:\n{seen}"
    );
    assert!(
        seen.contains("Blocked by policy"),
        "the user must be told a POLICY blocked the command, not left with the model's \
         improvised answer. User-visible output was:\n{seen}"
    );
    assert!(
        seen.contains("command floor"),
        "the user-visible notice must NAME the refusal that fired. User-visible output \
         was:\n{seen}"
    );
    assert!(
        seen.contains("repository control surface"),
        "the notice must name WHICH rule fired, so the user can decide what to do. \
         User-visible output was:\n{seen}"
    );
}

/// The control, and the whole meaning of c1: an ordinary failing command must
/// NOT be dressed as a policy refusal. Without this arm the assertions above
/// would be satisfied by a notice fired on every error, which distinguishes
/// nothing.
#[tokio::test]
async fn an_ordinary_command_failure_is_not_dressed_as_a_policy_refusal() {
    let wire = run_with_bash(
        "wayland_core_355_no_such_command",
        "That command is not installed here.",
    )
    .await;

    let payload = wire.tool_relay();
    assert!(
        !payload.is_empty(),
        "the Bash call produced no tool result at all"
    );
    assert!(
        !payload.contains(POLICY_REFUSAL_MARKER),
        "a command-not-found is not a policy refusal. Payload was:\n{payload}"
    );

    let seen = wire.user_visible();
    assert!(
        !seen.contains("Blocked by policy"),
        "a transient failure must raise no policy notice. User-visible output was:\n{seen}"
    );
}
