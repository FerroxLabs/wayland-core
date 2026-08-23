//! FerroxLabs/wayland#1109 — a turn that ends silently on an empty provider
//! stream, driven end to end on a real terminal.
//!
//! ## The symptom, and why it is measured HERE
//!
//! #1109 reports a turn that produced nothing: no answer, no tool call, no
//! error the user could read. v0.13.5 closed the two REPORTING halves of that
//! — the TUI now renders a banner when a turn finishes abnormally
//! (`tui/protocol_bridge.rs`), and the engine's empty-turn guard now fires for
//! shapes it used to let through (`wcore-agent/src/engine.rs`). Neither of
//! those stops a turn from ending early; they describe it after it has.
//!
//! This file measures a CAUSE. Anthropic's wire format carries the terminal
//! `stop_reason` on a `message_delta`. A `message_delta` that carries only
//! `usage` is a metadata update on a message still being generated — and the
//! SSE parser used to emit `LlmEvent::Done` for EVERY `message_delta`. One
//! arriving before the first text block therefore ended the turn, with
//! `FinishReason::Error`, holding nothing. The answer the provider went on to
//! send arrived after `streaming_active` had been cleared and was discarded.
//!
//! ## Why a PTY and not a unit test
//!
//! `wcore-providers` already grades the parser directly
//! (`anthropic_shared::tests::red_1109_usage_only_message_delta_ends_the_turn`).
//! That proves the event is not emitted; it cannot prove the USER gets their
//! answer, which is what #1109 is about — the whole chain from SSE frame to
//! rendered cell has to hold. So this drives the shipped binary on a real
//! terminal and asserts on what is on the screen.
//!
//! ## Why `#![cfg(unix)]`
//!
//! Same as every other PTY smoke in this crate: `portable_pty`'s ConPTY
//! backend on a headless Windows runner does not surface the child's stdout to
//! the master end, so the vt100 grid stays empty and every wait times out.

#![cfg(unix)]

use std::time::Duration;

use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;

use support::pty::{Pty, write_config};

/// The answer the provider sends AFTER the usage-only `message_delta`. It can
/// only reach the screen if that delta did not end the turn.
const ANSWER_TOKEN: &str = "WAYLAND_1109_ANSWER_REACHED_THE_SCREEN";

/// An Anthropic SSE body for one text turn, with a `usage`-only
/// `message_delta` optionally spliced in before the text block.
///
/// Every frame outside the splice is byte-for-byte the shape the typed builder
/// emits, so the two arms of this file differ in exactly ONE event.
fn text_turn_with_optional_usage_delta(text: &str, splice_usage_delta: bool) -> String {
    let message_start = serde_json::json!({
        "type": "message_start",
        "message": {
            "id": "msg_mock_1109",
            "type": "message",
            "role": "assistant",
            "content": [],
            "model": "claude-mock",
            "stop_reason": serde_json::Value::Null,
            "stop_sequence": serde_json::Value::Null,
            "usage": { "input_tokens": 3, "output_tokens": 0 }
        }
    });
    let block_start = serde_json::json!({
        "type": "content_block_start",
        "index": 0,
        "content_block": { "type": "text", "text": "" }
    });
    let delta = serde_json::json!({
        "type": "content_block_delta",
        "index": 0,
        "delta": { "type": "text_delta", "text": text }
    });
    let block_stop = serde_json::json!({ "type": "content_block_stop", "index": 0 });
    // THE EVENT UNDER TEST: `usage` and no `stop_reason`.
    let usage_delta = serde_json::json!({
        "type": "message_delta",
        "delta": {},
        "usage": { "output_tokens": 1 }
    });
    let terminal_delta = serde_json::json!({
        "type": "message_delta",
        "delta": { "stop_reason": "end_turn", "stop_sequence": serde_json::Value::Null },
        "usage": { "output_tokens": 9 }
    });
    let message_stop = serde_json::json!({ "type": "message_stop" });

    let mut body = format!("event: message_start\ndata: {message_start}\n\n");
    if splice_usage_delta {
        body.push_str(&format!("event: message_delta\ndata: {usage_delta}\n\n"));
    }
    body.push_str(&format!(
        "event: content_block_start\ndata: {block_start}\n\n"
    ));
    body.push_str(&format!(
        "event: content_block_delta\ndata: {delta}\n\n\
         event: content_block_stop\ndata: {block_stop}\n\n\
         event: message_delta\ndata: {terminal_delta}\n\n\
         event: message_stop\ndata: {message_stop}\n\n"
    ));
    body
}

/// Drive one prompt against a scripted raw SSE body and return the screen
/// after the wait — satisfied or not. Returns `Err(screen)` on timeout so the
/// caller can assert on the SYMPTOM rather than dying inside the harness.
fn answer_reaches_the_screen(body: String) -> Result<String, String> {
    let home = TempDir::new().expect("tempdir");
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let server = rt.block_on(support::mock_llm::MockLlm::new().raw_sse(body).start());
    write_config(
        home.path(),
        "anthropic",
        Some("claude-sonnet-4-20250514"),
        Some(&server.uri()),
    );

    let mut pty = Pty::spawn_with_env(home.path(), 40, 200, &[] as &[(&str, &str)]);
    pty.wait_for(
        |s| s.contains("WAYLAND") && s.contains("Workspace"),
        Duration::from_secs(60),
        "TUI to render the chrome wordmark and Workspace tab",
    );
    pty.send(b"say the thing\r");

    // Deliberately NOT `wait_for`: a timeout here is the finding, not a
    // harness failure, and both arms of this file need to report the screen.
    let deadline = std::time::Instant::now() + Duration::from_secs(25);
    let mut screen = String::new();
    while std::time::Instant::now() < deadline {
        screen = pty.screen_text();
        if screen.contains(ANSWER_TOKEN) {
            pty.quit();
            return Ok(screen);
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    pty.quit();
    Err(screen)
}

/// THE PROOF. A usage-only `message_delta` ahead of the text must not end the
/// turn: the answer that follows it has to reach the user.
///
/// On `addb4f48` (v0.13.5) this FAILS — the parser reads that delta as the end
/// of the turn, the TUI clears `streaming_active`, and every later text delta
/// is discarded. What the user gets instead of their answer is the
/// abnormal-finish banner v0.13.5 added, which is an honest description of a
/// turn that should never have ended.
#[test]
fn a_usage_only_message_delta_does_not_swallow_the_answer() {
    let body = text_turn_with_optional_usage_delta(ANSWER_TOKEN, true);
    match answer_reaches_the_screen(body) {
        Ok(screen) => println!("--- answer rendered ---\n{screen}\n--- end ---"),
        Err(screen) => panic!(
            "the provider sent the answer AFTER a usage-only message_delta and it never \
             reached the screen — the turn ended on a metadata event.\n\
             --- last screen ---\n{screen}\n--- end ---"
        ),
    }
}

/// CONTROL. The identical stream WITHOUT the spliced event must render the
/// same answer.
///
/// Without this, a green above could equally mean the harness cannot fail:
/// this arm shares every line of the drive path and differs only in the one
/// event, so a break in the mock, the config, the PTY or the prompt shows up
/// here too.
#[test]
fn a_control_the_same_stream_without_the_usage_delta_renders_the_answer() {
    let body = text_turn_with_optional_usage_delta(ANSWER_TOKEN, false);
    match answer_reaches_the_screen(body) {
        Ok(screen) => println!("--- control answer rendered ---\n{screen}\n--- end ---"),
        Err(screen) => panic!(
            "the CONTROL stream carries no usage-only delta and must render its answer; \
             this failing means the harness itself is broken, not the parser.\n\
             --- last screen ---\n{screen}\n--- end ---"
        ),
    }
}
