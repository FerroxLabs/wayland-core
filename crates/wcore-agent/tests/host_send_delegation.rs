//! #537/#141 — HARD SECURITY GATE tests for host-delegated `send_message`
//! (wayland#543 audit finding 4).
//!
//! The desktop performs the actual delivery for `host_send_message_request`
//! WITHOUT re-gating — it trusts that the engine's tool-approval flow
//! already ran. These tests pin the three properties that make that trust
//! sound:
//!
//! 1. `send_message` is `Exec`-category (never the auto-approvable `Info`)
//!    and absent from the default allow-list, so a default-mode agent turn
//!    can NOT send a message without an explicit approval.
//! 2. A DENIED approval produces a tool-error result and emits ZERO
//!    `host_send_message_request` frames — the host is never asked to send
//!    anything the user refused.
//! 3. AutoEdit mode (which auto-approves `info` + `edit` categories) still
//!    gates `send_message`.
//!
//! Plus the positive path: an APPROVED call emits exactly one request frame
//! and the host's `host_send_message_result` (same `call_id`) resolves the
//! parked tool call — the mid-turn unblocking semantics the CLI's
//! `HostSendMessageResult` arm relies on.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use wcore_agent::host_send_transport::{HostDelegatedTransport, HostSendBridge, HostSendResult};
use wcore_agent::orchestration::execute_tool_calls_with_approval;
use wcore_protocol::commands::{ApprovalScope, SessionMode};
use wcore_protocol::events::{ProtocolEvent, ToolCategory};
use wcore_protocol::{ToolApprovalManager, ToolApprovalResult};
use wcore_tools::registry::ToolRegistry;
use wcore_tools::send_message::SendMessageTool;
use wcore_types::message::ContentBlock;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// ProtocolEmitter that records every emitted event (tool_request /
/// tool_cancelled / tool_result / ...).
#[derive(Default)]
struct CaptureEmitter {
    events: Mutex<Vec<ProtocolEvent>>,
}

impl wcore_protocol::writer::ProtocolEmitter for CaptureEmitter {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        if let Ok(mut v) = self.events.lock() {
            v.push(event.clone());
        }
        Ok(())
    }
}

impl CaptureEmitter {
    fn tool_request_names(&self) -> Vec<String> {
        self.events
            .lock()
            .map(|v| {
                v.iter()
                    .filter_map(|e| match e {
                        ProtocolEvent::ToolRequest { tool, .. } => Some(tool.name.clone()),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// One captured request frame: `(call_id, platform, chat_id, body)`.
type CapturedRequest = (String, String, Option<String>, String);

/// OutputSink that records every `host_send_message_request` frame the
/// transport emits. The gate assertion is on THIS capture: a denied call
/// must leave it empty.
#[derive(Default)]
struct HostRequestCapture {
    requests: Mutex<Vec<CapturedRequest>>,
}

impl HostRequestCapture {
    fn snapshot(&self) -> Vec<CapturedRequest> {
        self.requests.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

impl wcore_agent::output::OutputSink for HostRequestCapture {
    fn emit_text_delta(&self, _text: &str, _msg_id: &str) {}
    fn emit_thinking(&self, _text: &str, _msg_id: &str) {}
    fn emit_tool_call(&self, _name: &str, _input: &str) {}
    fn emit_tool_result(&self, _name: &str, _is_error: bool, _content: &str) {}
    fn emit_stream_start(&self, _msg_id: &str) {}
    #[allow(clippy::too_many_arguments)]
    fn emit_stream_end(
        &self,
        _msg_id: &str,
        _turns: usize,
        _input_tokens: u64,
        _output_tokens: u64,
        _cache_creation_tokens: u64,
        _cache_read_tokens: u64,
        _finish_reason: wcore_protocol::events::FinishReason,
    ) {
    }
    fn emit_error(&self, _msg: &str, _retryable: bool) {}
    fn emit_info(&self, _msg: &str) {}
    fn emit_host_send_message_request(
        &self,
        call_id: &str,
        platform: &str,
        chat_id: Option<&str>,
        _thread_id: Option<&str>,
        body: &str,
        _subject: Option<&str>,
        _conversation_id: Option<&str>,
    ) {
        if let Ok(mut v) = self.requests.lock() {
            v.push((
                call_id.to_string(),
                platform.to_string(),
                chat_id.map(str::to_string),
                body.to_string(),
            ));
        }
    }
}

struct Harness {
    registry: ToolRegistry,
    bridge: Arc<HostSendBridge>,
    sink: Arc<HostRequestCapture>,
    mgr: Arc<ToolApprovalManager>,
    emitter: Arc<CaptureEmitter>,
}

fn harness() -> Harness {
    let bridge = Arc::new(HostSendBridge::new());
    let sink = Arc::new(HostRequestCapture::default());
    let mut registry = ToolRegistry::new();
    registry.register(Box::new(SendMessageTool::new(Arc::new(
        HostDelegatedTransport::new(bridge.clone(), sink.clone()),
    ))));
    Harness {
        registry,
        bridge,
        sink,
        mgr: Arc::new(ToolApprovalManager::new()),
        emitter: Arc::new(CaptureEmitter::default()),
    }
}

fn send_message_call(call_id: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: call_id.into(),
        name: "send_message".into(),
        input: json!({
            "target": "email:mike@example.com",
            "message": "hello from the agent"
        }),
        extra: None,
    }
}

async fn run_turn(h: &Harness, call: ContentBlock) -> ContentBlock {
    let writer: Arc<dyn wcore_protocol::writer::ProtocolEmitter> = h.emitter.clone();
    let outcome = execute_tool_calls_with_approval(
        &h.registry,
        &[call],
        &h.mgr,
        &writer,
        "msg-1",
        false, // auto_approve OFF — the gate must fire
        &[],   // allow_list empty (mirrors: send_message not allow-listed)
        None,
        wcore_compact::CompactionLevel::Off,
        false,
        &tokio_util::sync::CancellationToken::new(),
        None,
    )
    .await
    .expect("should not return ExecutionControl");
    assert_eq!(outcome.results.len(), 1, "one result expected");
    outcome.results.into_iter().next().expect("one result")
}

// ---------------------------------------------------------------------------
// Gate property 1: categorization + defaults
// ---------------------------------------------------------------------------

/// `send_message` must be `Exec`, never the auto-approvable `Info` — the
/// exact miscategorization wayland#543's audit finding 4 warns about.
#[test]
fn send_message_category_is_exec_not_info() {
    use wcore_tools::Tool;
    let tool = SendMessageTool::default();
    assert_eq!(tool.category(), ToolCategory::Exec);
}

/// `send_message` must be absent from the config default allow-list (the
/// read-only auto-approve set) — presence there would skip the approval
/// gate entirely for every default install.
#[test]
fn send_message_absent_from_default_allow_list() {
    let defaults = wcore_config::config::ToolsConfig::default().allow_list;
    assert!(
        !defaults.iter().any(|t| t == "send_message"),
        "send_message must NOT be in the default allow-list, got: {defaults:?}"
    );
    // Belt-and-suspenders: nothing in the default list may be Exec-category
    // messaging by another spelling either.
    assert!(
        !defaults.iter().any(
            |t| t.eq_ignore_ascii_case("sendmessage") || t.eq_ignore_ascii_case("send_message")
        ),
        "no spelling variant of send_message may be auto-approved: {defaults:?}"
    );
}

/// The manager's mode-level auto-approval must not cover `exec` in any
/// non-Force mode: Default approves nothing, AutoEdit approves only
/// `info`/`edit`. This is what keeps an Exec-category send_message gated.
#[test]
fn no_default_or_autoedit_mode_auto_approves_exec() {
    let mgr = ToolApprovalManager::new();
    assert!(!mgr.is_auto_approved("exec"), "Default mode gates exec");
    mgr.set_mode(SessionMode::AutoEdit);
    assert!(
        mgr.is_auto_approved("info"),
        "sanity: AutoEdit auto-approves info"
    );
    assert!(
        !mgr.is_auto_approved("exec"),
        "AutoEdit must NOT auto-approve exec (send_message's category)"
    );
}

// ---------------------------------------------------------------------------
// Gate property 2: denial emits NO host_send_message_request
// ---------------------------------------------------------------------------

/// THE GATE: a denied `send_message` produces a tool-error result and the
/// transport never emits `host_send_message_request` — the host (which
/// sends without re-gating) is never asked to deliver a refused message.
#[tokio::test]
async fn denied_send_message_emits_no_host_request() {
    let h = harness();
    let call_id = "call-deny-1";

    let mgr = h.mgr.clone();
    let id = call_id.to_string();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        mgr.resolve(
            &id,
            ToolApprovalResult::Denied {
                reason: "user refused the send".into(),
            },
        );
    });

    let result = run_turn(&h, send_message_call(call_id)).await;
    let ContentBlock::ToolResult {
        content, is_error, ..
    } = result
    else {
        panic!("expected ToolResult");
    };
    assert!(is_error, "denied send must surface as a tool error");
    assert!(
        content.contains("denied"),
        "denial reason must reach the model, got: {content}"
    );
    assert!(
        h.sink.snapshot().is_empty(),
        "NO host_send_message_request may be emitted for a denied call"
    );
    // The gate DID fire: a tool_request for send_message was emitted first.
    assert_eq!(h.emitter.tool_request_names(), vec!["send_message"]);
}

/// Same gate under AutoEdit — the mode that auto-approves Info/Edit. The
/// Exec-category send_message must still park on the approval and a denial
/// must still suppress the host request.
#[tokio::test]
async fn autoedit_mode_still_gates_send_message() {
    let h = harness();
    h.mgr.set_mode(SessionMode::AutoEdit);
    let call_id = "call-deny-autoedit";

    let mgr = h.mgr.clone();
    let id = call_id.to_string();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        mgr.resolve(
            &id,
            ToolApprovalResult::Denied {
                reason: "refused".into(),
            },
        );
    });

    let result = run_turn(&h, send_message_call(call_id)).await;
    let ContentBlock::ToolResult { is_error, .. } = result else {
        panic!("expected ToolResult");
    };
    assert!(is_error, "AutoEdit must not auto-approve send_message");
    assert!(
        h.sink.snapshot().is_empty(),
        "AutoEdit + denial: no host request may be emitted"
    );
    assert_eq!(
        h.emitter.tool_request_names(),
        vec!["send_message"],
        "the approval gate must have fired under AutoEdit"
    );
}

// ---------------------------------------------------------------------------
// Positive path: approval → request frame → host result unblocks the tool
// ---------------------------------------------------------------------------

/// Approved call: exactly one `host_send_message_request` (emitted AFTER
/// the tool_request approval round-trip) carrying the ParsedTarget, and a
/// `host_send_message_result` with the SAME call_id resolves the parked
/// tool into a success result — the same unblocking the CLI's mid-turn
/// `HostSendMessageResult` arm performs on a live json-stream session.
#[tokio::test]
async fn approved_send_resolves_via_host_result_same_call_id() {
    let h = harness();
    let call_id = "call-approve-1";

    // Approve once the gate parks, then play host: wait for the request
    // frame and resolve the bridge under its call_id.
    let mgr = h.mgr.clone();
    let sink = h.sink.clone();
    let bridge = h.bridge.clone();
    let id = call_id.to_string();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        mgr.approve(&id, ApprovalScope::Once, None);
        for _ in 0..200 {
            if let Some((hsm_id, platform, chat_id, body)) = sink.snapshot().first().cloned() {
                assert_eq!(platform, "email");
                assert_eq!(chat_id.as_deref(), Some("mike@example.com"));
                assert_eq!(body, "hello from the agent");
                // A wrong call_id must NOT unblock the tool.
                assert!(!bridge.resolve(
                    "hsm-wrong-id",
                    HostSendResult {
                        ok: true,
                        message_id: None,
                        error: None,
                    },
                ));
                assert!(bridge.resolve(
                    &hsm_id,
                    HostSendResult {
                        ok: true,
                        message_id: Some("smtp-250-ok".into()),
                        error: None,
                    },
                ));
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("host_send_message_request never appeared");
    });

    let result = run_turn(&h, send_message_call(call_id)).await;
    let ContentBlock::ToolResult {
        content, is_error, ..
    } = result
    else {
        panic!("expected ToolResult");
    };
    assert!(!is_error, "approved + host ok must succeed, got: {content}");
    assert!(
        content.contains("smtp-250-ok"),
        "the host receipt must reach the model, got: {content}"
    );
    assert_eq!(
        h.sink.snapshot().len(),
        1,
        "exactly one request frame per send"
    );
    assert_eq!(h.emitter.tool_request_names(), vec!["send_message"]);
}

/// #141 audit Gap A pin: `ApprovalScope::Always` on the FIRST send_message
/// must NOT auto-approve the second — Always downgrades to Once for this
/// tool. The second call emits a fresh tool_request; denying it produces a
/// tool error and no second host_send_message_request frame. Without the
/// carve-out, one "Always allow" click would let a prompt-injected turn
/// message arbitrary recipients silently for the rest of the session.
#[tokio::test]
async fn always_scope_does_not_skip_approval_on_second_send() {
    let h = harness();

    // ── First send: approved with ApprovalScope::Always; host fulfils. ──
    let first_id = "call-always-1";
    {
        let mgr = h.mgr.clone();
        let sink = h.sink.clone();
        let bridge = h.bridge.clone();
        let id = first_id.to_string();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            mgr.approve(&id, ApprovalScope::Always, None);
            for _ in 0..200 {
                if let Some((hsm_id, ..)) = sink.snapshot().first().cloned() {
                    assert!(bridge.resolve(
                        &hsm_id,
                        HostSendResult {
                            ok: true,
                            message_id: Some("first-ok".into()),
                            error: None,
                        },
                    ));
                    return;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            panic!("first host_send_message_request never appeared");
        });
    }
    let first = run_turn(&h, send_message_call(first_id)).await;
    let ContentBlock::ToolResult { is_error, .. } = &first else {
        panic!("expected ToolResult");
    };
    assert!(!is_error, "first (approved) send must succeed");

    // ── Second send: MUST park on a fresh approval; deny it. ──
    let second_id = "call-always-2";
    {
        let mgr = h.mgr.clone();
        let id = second_id.to_string();
        tokio::spawn(async move {
            tokio::task::yield_now().await;
            mgr.resolve(
                &id,
                ToolApprovalResult::Denied {
                    reason: "second send refused".into(),
                },
            );
        });
    }
    let second = run_turn(&h, send_message_call(second_id)).await;
    let ContentBlock::ToolResult { is_error, .. } = &second else {
        panic!("expected ToolResult");
    };
    assert!(
        is_error,
        "second send must have required (and here been denied) approval — \
         Always on send_message must downgrade to Once"
    );
    assert_eq!(
        h.emitter.tool_request_names(),
        vec!["send_message", "send_message"],
        "BOTH sends must emit their own tool_request card"
    );
    assert_eq!(
        h.sink.snapshot().len(),
        1,
        "only the first (approved) send may reach the host"
    );
}

/// Approved call + host failure: the tool result is a REAL error carrying
/// the host's message (never a false success).
#[tokio::test]
async fn approved_send_with_host_failure_is_tool_error() {
    let h = harness();
    let call_id = "call-approve-fail";

    let mgr = h.mgr.clone();
    let sink = h.sink.clone();
    let bridge = h.bridge.clone();
    let id = call_id.to_string();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        mgr.approve(&id, ApprovalScope::Once, None);
        for _ in 0..200 {
            if let Some((hsm_id, ..)) = sink.snapshot().first().cloned() {
                assert!(bridge.resolve(
                    &hsm_id,
                    HostSendResult {
                        ok: false,
                        message_id: None,
                        error: Some("SMTP 550: mailbox unavailable".into()),
                    },
                ));
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("host_send_message_request never appeared");
    });

    let result = run_turn(&h, send_message_call(call_id)).await;
    let ContentBlock::ToolResult {
        content, is_error, ..
    } = result
    else {
        panic!("expected ToolResult");
    };
    assert!(is_error, "host failure must be a tool error");
    assert!(
        content.contains("SMTP 550"),
        "the host's error must reach the model, got: {content}"
    );
}
