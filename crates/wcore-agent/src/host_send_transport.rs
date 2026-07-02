//! #537/#141 — `HostDelegatedTransport`: host-send-transport hook for the
//! `send_message` tool.
//!
//! Under the desktop the engine's channel table is empty (the desktop writes
//! no channel `.toml` into `$WAYLAND_HOME/channels`), so every agent
//! `send_message` failed with "unknown channel: email". Per the Overwatch
//! decision on wayland#537 (Option 1, host-send-transport-hook variant A),
//! when the host spawns the engine with `WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1`
//! the tool keeps its schema but the transport routes every send to the HOST:
//! it emits a `host_send_message_request` protocol event and awaits the
//! host's `host_send_message_result` command, correlated by `call_id`. The
//! host fulfils the send through its own outbound channel plugins — one SMTP
//! path, the engine owns no channel credentials.
//!
//! Without the env var nothing changes: `ChannelManagerTransport` stays the
//! transport and standalone/CLI engines with hand-authored channel toml are
//! byte-identical to before.
//!
//! **SECURITY (wayland#543 audit finding 4):** the desktop performs the send
//! WITHOUT re-gating, trusting that the engine already gated the tool call.
//! That holds structurally: this transport only runs inside
//! `SendMessageTool::execute`, which orchestration's approval gate fronts
//! (`send_message` is `Exec`-category, absent from the default allow-list,
//! and never mode-auto-approved outside Force). The integration tests in
//! `tests/host_send_delegation.rs` pin all three properties.
//!
//! **Race discipline (BridgeApprover precedent):** the waiter is registered
//! on the [`HostSendBridge`] BEFORE the request event is emitted, so a host
//! that replies instantly can never race a not-yet-registered `call_id`.
//!
//! **Never hang:** the await is bounded by [`HOST_SEND_TIMEOUT`] (mirroring
//! the bounded approval-bridge waits); a timeout is a loud `SendOutcome::Err`
//! surfaced to the model, and the pending entry is cleaned up.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::oneshot;

use wcore_tools::send_message::{MessageTransport, ParsedTarget, SendOutcome};

/// How long a delegated send waits for the host's
/// `host_send_message_result` before failing loudly. 30s per the #537 Core
/// spec — an SMTP handshake fits comfortably; a host that never answers must
/// not park the tool call anywhere near the 5-minute approval TTL.
pub const HOST_SEND_TIMEOUT: Duration = Duration::from_secs(30);

/// Activation check for host-delegated `send_message`. The desktop sets
/// `WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1` in the engine child's environment
/// (`envBuilder.buildEngineSpawnEnv`); anything other than exactly `"1"`
/// leaves the engine's behavior byte-identical to today.
pub fn host_delegated_send_enabled() -> bool {
    std::env::var("WAYLAND_SEND_MESSAGE_HOST_DELEGATE").as_deref() == Ok("1")
}

/// #141 audit item 3 — upper bound on host-supplied result strings
/// (`error`, `message_id`). Both land in the tool result and thus in the
/// model context; an unbounded string from a hostile/buggy host would be a
/// context-stuffing vector. 4096 chars carries any real SMTP diagnostic.
pub const MAX_HOST_RESULT_FIELD_CHARS: usize = 4096;

fn clamp_host_field(s: Option<String>) -> Option<String> {
    s.map(|v| {
        if v.chars().count() > MAX_HOST_RESULT_FIELD_CHARS {
            v.chars().take(MAX_HOST_RESULT_FIELD_CHARS).collect()
        } else {
            v
        }
    })
}

/// The host's answer to one `host_send_message_request`, as carried by the
/// `host_send_message_result` protocol command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSendResult {
    pub ok: bool,
    pub message_id: Option<String>,
    pub error: Option<String>,
}

/// Correlation bridge between the awaiting transport and the CLI command
/// loop. `register` (transport side) parks a oneshot under the `call_id`;
/// `resolve` (command-loop side, on `host_send_message_result`) completes it.
///
/// Locking: a synchronous `Mutex` with no await inside any critical section,
/// so the transport's cancel-safety guard can clean up from a `Drop` impl.
/// Unlike the `ApprovalBridge` there is no TTL reaper — the ONLY requester is
/// the transport itself, which always removes its entry on timeout/completion
/// (and via the drop guard on turn cancellation), so entries cannot outlive
/// their send.
#[derive(Default)]
pub struct HostSendBridge {
    pending: Mutex<HashMap<String, oneshot::Sender<HostSendResult>>>,
}

impl HostSendBridge {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a waiter for `call_id`. MUST be called BEFORE the
    /// corresponding `host_send_message_request` is emitted (the
    /// register-before-emit rule) so an instant host reply always finds
    /// its entry.
    pub fn register(&self, call_id: String) -> oneshot::Receiver<HostSendResult> {
        let (tx, rx) = oneshot::channel();
        if let Ok(mut map) = self.pending.lock() {
            map.insert(call_id, tx);
        }
        rx
    }

    /// Resolve the waiter registered under `call_id`. Returns `false` when
    /// the id is unknown (stale, mistyped, or already timed out) — a wrong
    /// `call_id` never completes somebody else's send.
    ///
    /// #141 audit item 3: `message_id` / `error` are host-supplied wire
    /// strings headed for the tool result (model context); they are clamped
    /// to [`MAX_HOST_RESULT_FIELD_CHARS`] here — the single chokepoint every
    /// CLI arm routes through — so a hostile host cannot stuff the context.
    pub fn resolve(&self, call_id: &str, result: HostSendResult) -> bool {
        let result = HostSendResult {
            ok: result.ok,
            message_id: clamp_host_field(result.message_id),
            error: clamp_host_field(result.error),
        };
        let sender = match self.pending.lock() {
            Ok(mut map) => map.remove(call_id),
            Err(_) => None,
        };
        match sender {
            Some(tx) => tx.send(result).is_ok(),
            None => false,
        }
    }

    /// Drop the waiter registered under `call_id` without resolving it
    /// (timeout / cancellation cleanup).
    fn remove(&self, call_id: &str) {
        if let Ok(mut map) = self.pending.lock() {
            map.remove(call_id);
        }
    }

    /// Number of in-flight delegated sends. Test/diagnostic helper.
    pub fn pending_count(&self) -> usize {
        self.pending.lock().map(|m| m.len()).unwrap_or(0)
    }

    /// Snapshot of in-flight `call_id`s. Test/diagnostic helper (mirrors
    /// `ApprovalBridge::pending_tokens`); production resolution always uses
    /// the id carried by the wire command.
    pub fn pending_ids(&self) -> Vec<String> {
        self.pending
            .lock()
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// RAII cleanup: if the awaiting send future is dropped mid-flight (turn
/// cancelled via Esc/Stop), the pending entry is removed so a late host
/// reply resolves nothing and the map never leaks.
struct PendingGuard<'a> {
    bridge: &'a HostSendBridge,
    call_id: &'a str,
    armed: bool,
}

impl Drop for PendingGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.bridge.remove(self.call_id);
        }
    }
}

/// `MessageTransport` impl that delegates delivery to the host. Installed
/// as the PRIMARY transport (spec variant A — deterministic, table-state
/// independent) when [`host_delegated_send_enabled`] is true at bootstrap.
pub struct HostDelegatedTransport {
    bridge: Arc<HostSendBridge>,
    output: Arc<dyn crate::output::OutputSink>,
    timeout: Duration,
}

impl HostDelegatedTransport {
    pub fn new(bridge: Arc<HostSendBridge>, output: Arc<dyn crate::output::OutputSink>) -> Self {
        Self {
            bridge,
            output,
            timeout: HOST_SEND_TIMEOUT,
        }
    }

    /// Test hook: shrink the host-reply timeout so timeout-path tests run
    /// in milliseconds. Production callers use [`Self::new`].
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

#[async_trait]
impl MessageTransport for HostDelegatedTransport {
    async fn send(&self, target: &ParsedTarget, message: &str) -> SendOutcome {
        let call_id = format!("hsm-{}", uuid::Uuid::new_v4());
        // Register BEFORE emit — an instant host reply must find the waiter.
        let rx = self.bridge.register(call_id.clone());
        let mut guard = PendingGuard {
            bridge: self.bridge.as_ref(),
            call_id: &call_id,
            armed: true,
        };
        self.output.emit_host_send_message_request(
            &call_id,
            target.platform.as_str(),
            target.chat_id.as_deref(),
            target.thread_id.as_deref(),
            message,
            None, // subject: no subject input on the send_message schema today
            None, // conversation_id: session id not threaded to transports
        );
        match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(result)) => {
                // Resolved normally — resolve() already removed the entry.
                guard.armed = false;
                if result.ok {
                    SendOutcome::Ok {
                        message_id: result.message_id,
                    }
                } else {
                    SendOutcome::Err {
                        message: result.error.unwrap_or_else(|| {
                            "host reported the send failed (no error detail)".to_string()
                        }),
                    }
                }
            }
            // Sender dropped without a result — only possible if the bridge
            // entry was removed out-of-band; fail loudly.
            Ok(Err(_)) => SendOutcome::Err {
                message: "host send channel closed before a result arrived".to_string(),
            },
            Err(_) => {
                // Timeout: the guard cleans the entry on drop; a late host
                // reply then resolves nothing (resolve returns false).
                //
                // #141 audit Fix B: the host has no send-timeout of its own —
                // a slow SMTP handshake can outlast this bound and STILL
                // deliver. The error text below instructs the model not to
                // auto-retry (a retry could double-send); keep the wording
                // model-readable and verbatim-ish.
                SendOutcome::Err {
                    message: format!(
                        "host did not answer the delegated send within {}s \
                         (no host_send_message_result); the message was NOT \
                         confirmed sent. The host may still complete this \
                         delivery — do NOT automatically retry; ask the user \
                         before re-sending.",
                        self.timeout.as_secs()
                    ),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use wcore_tools::send_message::MessagingPlatform;

    /// One captured request frame:
    /// `(call_id, platform, chat_id, thread_id, body)`.
    type CapturedRequest = (String, String, Option<String>, Option<String>, String);

    /// Capturing sink that records every host-send request frame.
    #[derive(Default)]
    struct CaptureSink {
        requests: Mutex<Vec<CapturedRequest>>,
    }

    impl crate::output::OutputSink for CaptureSink {
        fn emit_text_delta(&self, _text: &str, _msg_id: &str) {}
        fn emit_thinking(&self, _text: &str, _msg_id: &str) {}
        fn emit_tool_call(&self, _name: &str, _input: &str) {}
        fn emit_tool_result(&self, _name: &str, _is_error: bool, _content: &str) {}
        fn emit_stream_start(&self, _msg_id: &str) {}
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
            thread_id: Option<&str>,
            body: &str,
            _subject: Option<&str>,
            _conversation_id: Option<&str>,
        ) {
            if let Ok(mut v) = self.requests.lock() {
                v.push((
                    call_id.to_string(),
                    platform.to_string(),
                    chat_id.map(str::to_string),
                    thread_id.map(str::to_string),
                    body.to_string(),
                ));
            }
        }
    }

    /// Sink that resolves the bridge SYNCHRONOUSLY from inside `emit` —
    /// proves the register-before-emit rule: if the waiter were registered
    /// after emission this instant reply would be lost and the send would
    /// time out.
    struct InstantReplySink {
        bridge: Arc<HostSendBridge>,
        resolved_ok: AtomicUsize,
    }

    impl crate::output::OutputSink for InstantReplySink {
        fn emit_text_delta(&self, _text: &str, _msg_id: &str) {}
        fn emit_thinking(&self, _text: &str, _msg_id: &str) {}
        fn emit_tool_call(&self, _name: &str, _input: &str) {}
        fn emit_tool_result(&self, _name: &str, _is_error: bool, _content: &str) {}
        fn emit_stream_start(&self, _msg_id: &str) {}
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
            _platform: &str,
            _chat_id: Option<&str>,
            _thread_id: Option<&str>,
            _body: &str,
            _subject: Option<&str>,
            _conversation_id: Option<&str>,
        ) {
            let resolved = self.bridge.resolve(
                call_id,
                HostSendResult {
                    ok: true,
                    message_id: Some("instant-1".to_string()),
                    error: None,
                },
            );
            if resolved {
                self.resolved_ok.fetch_add(1, Ordering::SeqCst);
            }
        }
    }

    fn email_target() -> ParsedTarget {
        ParsedTarget {
            platform: MessagingPlatform::Email,
            chat_id: Some("mike@example.com".to_string()),
            thread_id: None,
        }
    }

    #[test]
    fn env_gate_requires_exactly_one() {
        // Uses a scoped set/remove; the var name is unique to this feature so
        // no other test in this crate races it.
        // SAFETY (Rust 2024 set_var): single-threaded test context for this
        // env var; no concurrent readers of this name.
        unsafe {
            std::env::remove_var("WAYLAND_SEND_MESSAGE_HOST_DELEGATE");
        }
        assert!(!host_delegated_send_enabled());
        unsafe {
            std::env::set_var("WAYLAND_SEND_MESSAGE_HOST_DELEGATE", "0");
        }
        assert!(!host_delegated_send_enabled());
        unsafe {
            std::env::set_var("WAYLAND_SEND_MESSAGE_HOST_DELEGATE", "1");
        }
        assert!(host_delegated_send_enabled());
        unsafe {
            std::env::remove_var("WAYLAND_SEND_MESSAGE_HOST_DELEGATE");
        }
    }

    /// Register-before-emit: a host that answers from INSIDE the emit call
    /// (the fastest possible reply) still resolves the send.
    #[tokio::test]
    async fn instant_host_reply_cannot_race_registration() {
        let bridge = Arc::new(HostSendBridge::new());
        let sink = Arc::new(InstantReplySink {
            bridge: bridge.clone(),
            resolved_ok: AtomicUsize::new(0),
        });
        let transport = HostDelegatedTransport::new(bridge.clone(), sink.clone());
        let outcome = transport.send(&email_target(), "hello").await;
        assert_eq!(sink.resolved_ok.load(Ordering::SeqCst), 1);
        match outcome {
            SendOutcome::Ok { message_id } => {
                assert_eq!(message_id.as_deref(), Some("instant-1"))
            }
            SendOutcome::Err { message } => panic!("expected Ok, got Err: {message}"),
        }
        assert_eq!(bridge.pending_count(), 0);
    }

    /// The request frame carries the ParsedTarget verbatim, and a host
    /// `ok=true` result resolves into `SendOutcome::Ok` with the receipt.
    #[tokio::test]
    async fn ok_result_resolves_send_with_receipt() {
        let bridge = Arc::new(HostSendBridge::new());
        let sink = Arc::new(CaptureSink::default());
        let transport = HostDelegatedTransport::new(bridge.clone(), sink.clone());

        let target = ParsedTarget {
            platform: MessagingPlatform::Email,
            chat_id: Some("mike@example.com".to_string()),
            thread_id: Some("t-1".to_string()),
        };
        let send = transport.send(&target, "body text");
        tokio::pin!(send);
        // Drive the send until the request frame is emitted.
        let outcome = tokio::select! {
            biased;
            out = &mut send => Some(out),
            _ = tokio::task::yield_now() => None,
        };
        assert!(outcome.is_none(), "send must park awaiting the host");

        let (call_id, platform, chat_id, thread_id, body) = {
            let reqs = sink.requests.lock().unwrap();
            assert_eq!(reqs.len(), 1, "exactly one request frame");
            reqs[0].clone()
        };
        assert!(call_id.starts_with("hsm-"), "engine-minted call_id");
        assert_eq!(platform, "email");
        assert_eq!(chat_id.as_deref(), Some("mike@example.com"));
        assert_eq!(thread_id.as_deref(), Some("t-1"));
        assert_eq!(body, "body text");

        assert!(bridge.resolve(
            &call_id,
            HostSendResult {
                ok: true,
                message_id: Some("smtp-250".to_string()),
                error: None,
            },
        ));
        match send.await {
            SendOutcome::Ok { message_id } => {
                assert_eq!(message_id.as_deref(), Some("smtp-250"))
            }
            SendOutcome::Err { message } => panic!("expected Ok, got Err: {message}"),
        }
    }

    /// A WRONG call_id must not resolve the waiter; the correct one still
    /// does afterwards.
    #[tokio::test]
    async fn wrong_call_id_does_not_resolve() {
        let bridge = Arc::new(HostSendBridge::new());
        let sink = Arc::new(CaptureSink::default());
        let transport = HostDelegatedTransport::new(bridge.clone(), sink.clone());

        let target = email_target();
        let send = transport.send(&target, "hi");
        tokio::pin!(send);
        tokio::select! {
            biased;
            _ = &mut send => panic!("send must still be pending"),
            _ = tokio::task::yield_now() => {}
        }
        let call_id = sink.requests.lock().unwrap()[0].0.clone();

        assert!(
            !bridge.resolve(
                "hsm-not-the-right-id",
                HostSendResult {
                    ok: true,
                    message_id: None,
                    error: None,
                },
            ),
            "unknown call_id must resolve nothing"
        );
        assert_eq!(bridge.pending_count(), 1, "real waiter still pending");

        assert!(bridge.resolve(
            &call_id,
            HostSendResult {
                ok: true,
                message_id: None,
                error: None,
            },
        ));
        assert!(matches!(send.await, SendOutcome::Ok { .. }));
    }

    /// A host-reported failure surfaces as a real `SendOutcome::Err`
    /// carrying the host's error string — never a false success.
    #[tokio::test]
    async fn host_failure_surfaces_as_error() {
        let bridge = Arc::new(HostSendBridge::new());
        let sink = Arc::new(CaptureSink::default());
        let transport = HostDelegatedTransport::new(bridge.clone(), sink.clone());

        let target = email_target();
        let send = transport.send(&target, "hi");
        tokio::pin!(send);
        tokio::select! {
            biased;
            _ = &mut send => panic!("send must still be pending"),
            _ = tokio::task::yield_now() => {}
        }
        let call_id = sink.requests.lock().unwrap()[0].0.clone();
        assert!(bridge.resolve(
            &call_id,
            HostSendResult {
                ok: false,
                message_id: None,
                error: Some("SMTP 550: mailbox unavailable".to_string()),
            },
        ));
        match send.await {
            SendOutcome::Err { message } => assert!(
                message.contains("SMTP 550"),
                "host error must reach the model, got: {message}"
            ),
            SendOutcome::Ok { .. } => panic!("host failure must not become success"),
        }
    }

    /// No host reply → bounded timeout → loud error, no hang, pending
    /// entry cleaned up (a late reply then resolves nothing).
    #[tokio::test]
    async fn timeout_is_a_loud_error_not_a_hang() {
        let bridge = Arc::new(HostSendBridge::new());
        let sink = Arc::new(CaptureSink::default());
        let transport = HostDelegatedTransport::new(bridge.clone(), sink.clone())
            .with_timeout(Duration::from_millis(50));

        let outcome = transport.send(&email_target(), "hi").await;
        match outcome {
            SendOutcome::Err { message } => assert!(
                message.contains("did not answer")
                    && message.contains("NOT confirmed sent")
                    // #141 audit Fix B: the double-send advisory must reach
                    // the model verbatim — the host may still deliver after
                    // our bound, so an auto-retry could double-send.
                    && message.contains("do NOT automatically retry")
                    && message.contains("ask the user before re-sending"),
                "timeout error must pin non-delivery + the no-retry advisory, got: {message}"
            ),
            SendOutcome::Ok { .. } => panic!("timeout must not be a success"),
        }
        assert_eq!(bridge.pending_count(), 0, "timed-out entry cleaned up");
        let call_id = sink.requests.lock().unwrap()[0].0.clone();
        assert!(
            !bridge.resolve(
                &call_id,
                HostSendResult {
                    ok: true,
                    message_id: None,
                    error: None,
                },
            ),
            "a late host reply after timeout must resolve nothing"
        );
    }

    /// #141 audit item 3: host-supplied `error` / `message_id` strings are
    /// clamped at the bridge chokepoint so a hostile host can't stuff the
    /// model context through the tool result.
    #[tokio::test]
    async fn oversized_host_result_strings_are_clamped() {
        let bridge = Arc::new(HostSendBridge::new());
        let rx = bridge.register("hsm-clamp".to_string());
        let huge = "x".repeat(MAX_HOST_RESULT_FIELD_CHARS + 5000);
        assert!(bridge.resolve(
            "hsm-clamp",
            HostSendResult {
                ok: false,
                message_id: Some(huge.clone()),
                error: Some(huge),
            },
        ));
        let result = rx.await.expect("resolved");
        assert_eq!(
            result.error.as_ref().map(|s| s.chars().count()),
            Some(MAX_HOST_RESULT_FIELD_CHARS),
            "error must be clamped to the cap"
        );
        assert_eq!(
            result.message_id.as_ref().map(|s| s.chars().count()),
            Some(MAX_HOST_RESULT_FIELD_CHARS),
            "message_id must be clamped to the cap"
        );
    }

    /// Cancel-safety: dropping the in-flight send future (turn cancelled)
    /// removes the pending entry via the drop guard.
    #[tokio::test]
    async fn dropped_send_future_cleans_pending_entry() {
        let bridge = Arc::new(HostSendBridge::new());
        let sink = Arc::new(CaptureSink::default());
        let transport = HostDelegatedTransport::new(bridge.clone(), sink.clone());

        {
            let target = email_target();
            let send = transport.send(&target, "hi");
            tokio::pin!(send);
            tokio::select! {
                biased;
                _ = &mut send => panic!("send must still be pending"),
                _ = tokio::task::yield_now() => {}
            }
            assert_eq!(bridge.pending_count(), 1);
            // `send` dropped here — simulates Esc/Stop cancelling the turn.
        }
        assert_eq!(bridge.pending_count(), 0, "drop guard must clean the entry");
    }
}
