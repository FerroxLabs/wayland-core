pub mod null_sink;
pub mod permission_prompt;
pub mod protocol_sink;
pub mod slash_render;
pub mod terminal;

use crossterm::execute;
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use std::io::{self, Write};
use wcore_types::message::FinishReason;

/// Abstraction over output channels (terminal vs JSON stream protocol)
pub trait OutputSink: Send + Sync {
    /// Stream text delta from LLM
    fn emit_text_delta(&self, text: &str, msg_id: &str);
    /// Stream thinking content from LLM
    fn emit_thinking(&self, text: &str, msg_id: &str);
    /// #318 — emit a per-turn thinking SUBJECT: a short opaque display label
    /// for the in-flight reasoning block (e.g. Flux `reasoning_summary`).
    /// Default no-op so terminal/null/test sinks need no change; only
    /// `ProtocolSink` overrides to emit a `Thinking` event carrying
    /// `subject: Some(..)` on the same `msg_id`/turn as the reasoning text
    /// that follows.
    fn emit_thinking_subject(&self, _subject: &str, _msg_id: &str) {}
    /// Announce a tool call
    fn emit_tool_call(&self, name: &str, input: &str);
    /// Display tool result
    fn emit_tool_result(&self, name: &str, is_error: bool, content: &str);
    /// Signal start of a new message stream
    fn emit_stream_start(&self, msg_id: &str);
    /// Signal end of a message stream with usage stats and finish reason.
    ///
    /// `finish_reason` is required so the JSON stream protocol always
    /// advertises a value (closes the Gemini Pro reasoning-token bug at
    /// the protocol layer). Callers that don't have a real value (e.g.
    /// abrupt error paths) should pass `FinishReason::Error`.
    #[allow(clippy::too_many_arguments)]
    fn emit_stream_end(
        &self,
        msg_id: &str,
        turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        finish_reason: FinishReason,
    );
    /// #279(a)+(c): enriched stream-end carrying the engine-computed gauge
    /// and run-correlation id. Default delegates to emit_stream_end (dropping
    /// the extras) so existing sinks/mocks need no change; ProtocolSink
    /// overrides to populate Usage.active_window_percent + StreamEnd.agent_run_id.
    #[allow(clippy::too_many_arguments)]
    fn emit_stream_end_full(
        &self,
        msg_id: &str,
        turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        finish_reason: FinishReason,
        _active_window_percent: Option<u32>,
        _agent_run_id: Option<&str>,
    ) {
        self.emit_stream_end(
            msg_id,
            turns,
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
            finish_reason,
        );
    }
    /// Display error.
    ///
    /// `retryable` is the protocol contract's honest signal to the host: `true`
    /// for a transient class where a fresh attempt has a real chance (5xx,
    /// network drop, truncated stream); `false` for a hard failure where
    /// retrying re-sends the same doomed request (4xx, context-ceiling, budget
    /// cap, IO/persistence). It is a REQUIRED argument so no error can silently
    /// claim a wrong value — the `ProtocolSink` impl previously hardcoded
    /// `retryable: false` for every error regardless of truth.
    fn emit_error(&self, msg: &str, retryable: bool);
    /// Display informational message
    fn emit_info(&self, msg: &str);
    /// W1: F9 trace emission. Implementations that don't structure-trace
    /// (e.g. `TerminalSink`, `NullSink`) leave the default no-op body. The
    /// `ProtocolSink` impl emits `ProtocolEvent::TraceEvent` ONLY when the
    /// sink was configured with `with_structured_traces(true)` so hosts that
    /// haven't opted in via `Capabilities.structured_traces` never see the
    /// new variant.
    fn emit_trace(&self, _msg_id: &str, _trace_json: &serde_json::Value) {
        // Default: no-op. ProtocolSink overrides; terminal and null sinks
        // intentionally do nothing.
    }

    /// W6 F7 — emit the end-of-session cost aggregate. Default no-op;
    /// `ProtocolSink` overrides and gates on
    /// `AdvertisedCapabilitiesConfig.cost_attribution` (single authority,
    /// audit rev-2 finding 5).
    ///
    /// `cost_payload` is a JSON object shaped like
    /// `{ "total_cost_usd": f64, "per_turn": [TurnCost, ...] }`; the sink
    /// re-wraps it into the typed `ProtocolEvent::SessionCost` variant.
    fn emit_session_cost(&self, _session_id: &str, _cost_payload: &serde_json::Value) {}

    /// W7 F2: emit a sub-agent's event wrapped as a SubAgentEvent.
    /// Default no-op; only `ProtocolSink` configured with
    /// `with_sub_agent_traces(true)` emits these.
    fn emit_sub_agent_event(
        &self,
        _parent_call_id: &str,
        _agent_name: &str,
        _inner: &serde_json::Value,
    ) {
    }

    /// ForgeFlows-Live: a workflow run started. Default no-op; only
    /// `ProtocolSink` configured with `with_sub_agent_traces(true)` emits
    /// the `WorkflowStarted` variant (same gate as `emit_sub_agent_event`).
    fn emit_workflow_started(&self, _workflow_id: &str, _name: &str, _node_count: usize) {}

    /// ForgeFlows-Live: a workflow run finished. Default no-op; only
    /// `ProtocolSink` configured with `with_sub_agent_traces(true)` emits
    /// the `WorkflowFinished` variant (same gate as `emit_sub_agent_event`).
    fn emit_workflow_finished(&self, _workflow_id: &str, _succeeded: bool) {}

    /// W7 F4: emit a streaming chunk from a long-running tool. Default
    /// no-op; only `ProtocolSink` configured with
    /// `with_streaming_tools(true)` emits these.
    fn emit_tool_chunk(&self, _msg_id: &str, _call_id: &str, _tool_name: &str, _chunk: &str) {}

    /// W7 F4 audit fix M5: surface the streaming-tools advertise gate
    /// directly on the trait so the engine dispatcher can branch without
    /// downcasting to a concrete sink. Default false; `ProtocolSink`
    /// overrides to return its builder-set flag.
    fn streaming_tools_advertised(&self) -> bool {
        false
    }

    /// W7 F8: emit a provider circuit-breaker transition. NOT gated by
    /// a capability flag (audit rev-2 F4); failure-mode visibility is
    /// always-on like `Error`. Default no-op for non-protocol sinks.
    fn emit_provider_circuit_event(
        &self,
        _primary: &str,
        _fallback: Option<&str>,
        _state: &str,
        _error: Option<&str>,
    ) {
    }

    /// W7 S4: emit ApprovalRequired (host renders modal). Default
    /// no-op; `ProtocolSink` overrides and gates on
    /// `with_hitl_suspend(true)`.
    fn emit_approval_required(
        &self,
        _call_id: &str,
        _resume_token: &str,
        _reason: &str,
        _context: &str,
    ) {
    }

    /// W7 S4: emit Suspend (session-level state pill). Default no-op.
    fn emit_suspend(&self, _reason: &str, _resume_token: &str) {}

    /// W7 S4: emit ApprovalResume (echo of resolved outcome). Default
    /// no-op.
    fn emit_approval_resume(&self, _resume_token: &str, _approved: bool) {}

    /// #537/#141: emit `host_send_message_request` — an APPROVED
    /// `send_message` tool call running host-delegated
    /// (`WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1`) asks the host to perform
    /// the actual delivery; the host replies with the
    /// `host_send_message_result` command, correlated by `call_id`.
    /// Always-on additive event (no capability flag) per the W0
    /// forward-additive baseline. Default no-op for non-protocol sinks —
    /// a delegated send under such a sink times out into a loud tool
    /// error rather than reaching a host that doesn't exist.
    #[allow(clippy::too_many_arguments)]
    fn emit_host_send_message_request(
        &self,
        _call_id: &str,
        _platform: &str,
        _chat_id: Option<&str>,
        _thread_id: Option<&str>,
        _body: &str,
        _subject: Option<&str>,
        _conversation_id: Option<&str>,
    ) {
    }

    /// W8a A.7: emit `budget_exceeded` (singular per session, fires
    /// once when the first ExecutionBudget cap trips). Always-emitted
    /// host-tolerated additive variant per audit F5 — no capability
    /// flag, hosts that don't know the `type` drop the line silently
    /// per the W0 host decoder contract. Default no-op for non-protocol
    /// sinks.
    fn emit_budget_exceeded(&self, _reason: &str, _observed: &str, _limit: &str) {}

    /// Wave RB RELIABILITY MAJOR: a tool's `execute_with_ctx` future
    /// panicked. The orchestration dispatcher caught the panic via
    /// `FutureExt::catch_unwind` and synthesised a normal "tool error"
    /// `ToolResult` so the LLM context sees a structured failure and
    /// the session continues. This event is the typed diagnostic the
    /// host renders alongside the synthetic error result. Always-on
    /// per W0 forward-additive baseline (no capability flag). Default
    /// no-op for non-protocol sinks.
    fn emit_tool_panicked(
        &self,
        _msg_id: &str,
        _call_id: &str,
        _tool_name: &str,
        _panic_message: &str,
    ) {
    }

    /// Wave RB STABILITY MINOR #10: a plugin registration step failed
    /// with an error other than the expected "access denied because the
    /// manifest didn't request the surface" sentinel. The plugin still
    /// loads but the host can render a diagnostic so a missing surface
    /// has a visible cause. Always-on per W0 forward-additive baseline
    /// (no capability flag). Default no-op for non-protocol sinks.
    fn emit_plugin_registration_failed(
        &self,
        _plugin_name: &str,
        _surface: &str,
        _error_kind: &str,
        _message: &str,
    ) {
    }

    /// #279(d) + #280: a context compaction occurred. Default no-op;
    /// ProtocolSink overrides and gates on with_non_destructive_compact(true).
    /// tokens_freed 0 when unmeasurable; active_window_percent is the
    /// post-compaction fill from ContextWindow::percent() (None when unknown).
    fn emit_compaction(
        &self,
        _msg_id: &str,
        _reason: &str,
        _tokens_freed: u64,
        _active_window_percent: Option<u32>,
    ) {
    }
}

pub struct OutputFormatter {
    color_enabled: bool,
}

impl OutputFormatter {
    pub fn new(no_color: bool) -> Self {
        // Also check NO_COLOR env var (standard: https://no-color.org/)
        let color_enabled = !no_color
            && std::env::var("NO_COLOR").is_err()
            && is_terminal::is_terminal(io::stderr());
        Self { color_enabled }
    }

    /// Print LLM text delta (streaming, no newline)
    pub fn text_delta(&self, text: &str) {
        print!("{}", text);
        let _ = io::stdout().flush();
    }

    /// Print tool call announcement
    pub fn tool_call(&self, name: &str, input: &str) {
        if self.color_enabled {
            let mut stderr = io::stderr();
            let _ = execute!(
                stderr,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold),
                Print(format!("\n[tool] {}", name)),
                ResetColor,
                SetForegroundColor(Color::DarkGrey),
                Print(format!("({})\n", truncate_display(input, 200))),
                ResetColor,
            );
        } else {
            eprintln!("\n[tool] {}({})", name, truncate_display(input, 200));
        }
    }

    /// Print tool result
    pub fn tool_result(&self, name: &str, is_error: bool, content: &str) {
        if self.color_enabled {
            let color = if is_error { Color::Red } else { Color::Green };
            let attr = if is_error {
                Attribute::Bold
            } else {
                Attribute::Dim
            };
            let mut stderr = io::stderr();
            let _ = execute!(
                stderr,
                SetForegroundColor(color),
                SetAttribute(attr),
                Print(format!("[{}] {}\n", name, truncate_display(content, 500))),
                ResetColor,
            );
        } else {
            let prefix = if is_error { "ERROR" } else { "OK" };
            eprintln!("[{} {}] {}", name, prefix, truncate_display(content, 500));
        }
    }

    /// Print thinking content
    pub fn thinking(&self, text: &str) {
        if self.color_enabled {
            let mut stderr = io::stderr();
            let _ = execute!(
                stderr,
                SetForegroundColor(Color::DarkGrey),
                SetAttribute(Attribute::Italic),
                Print(text),
                ResetColor,
            );
        }
        // Silent in no-color mode (thinking is optional display)
    }

    /// Print turn summary stats
    pub fn turn_stats(
        &self,
        turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
    ) {
        let cache_info = if cache_creation_tokens > 0 || cache_read_tokens > 0 {
            format!(
                " | cache: {} created, {} read",
                cache_creation_tokens, cache_read_tokens
            )
        } else {
            String::new()
        };

        let cached_suffix = if cache_read_tokens > 0 {
            format!(" ({} cached)", cache_read_tokens)
        } else {
            String::new()
        };

        if self.color_enabled {
            let mut stderr = io::stderr();
            let _ = execute!(
                stderr,
                SetForegroundColor(Color::Yellow),
                SetAttribute(Attribute::Dim),
                Print(format!(
                    "\n[turns: {} | tokens: {} in{} / {} out{}]\n",
                    turns, input_tokens, cached_suffix, output_tokens, cache_info
                )),
                ResetColor,
            );
        } else {
            eprintln!(
                "\n[turns: {} | tokens: {} in{} / {} out{}]",
                turns, input_tokens, cached_suffix, output_tokens, cache_info
            );
        }
    }

    /// Print REPL prompt — spec §3.1: one blank line, then Green Bold "> ".
    pub fn repl_prompt(&self) {
        if self.color_enabled {
            let mut stdout = io::stdout();
            let _ = execute!(
                stdout,
                Print("\n"),
                SetForegroundColor(Color::Green),
                SetAttribute(Attribute::Bold),
                Print("> "),
                ResetColor,
                SetAttribute(Attribute::Reset),
            );
            let _ = stdout.flush();
        } else {
            print!("\n> ");
            let _ = io::stdout().flush();
        }
    }

    /// Spec §3.2 — assistant turn marker, emitted once per turn on the first
    /// non-empty delta. Magenta Bold `⏺ ` in color mode, plain `* ` otherwise.
    /// Lands on stdout because the marker visually leads assistant body text.
    pub fn assistant_marker(&self) {
        if self.color_enabled {
            let mut stdout = io::stdout();
            let _ = execute!(
                stdout,
                SetForegroundColor(Color::Magenta),
                SetAttribute(Attribute::Bold),
                Print("\u{23FA} "),
                ResetColor,
                SetAttribute(Attribute::Reset),
            );
            let _ = stdout.flush();
        } else {
            print!("* ");
            let _ = io::stdout().flush();
        }
    }

    /// Spec §3.3 — tool "running" lifecycle line. Replaces `tool_call` for
    /// the new pretty path. `⏵ name(params)` in color mode (Cyan Bold name +
    /// DarkGrey params); ASCII fallback `> name(params)`.
    pub fn tool_call_running(&self, name: &str, input: &str) {
        let params = truncate_display(input, 200);
        if self.color_enabled {
            let mut stderr = io::stderr();
            let _ = execute!(
                stderr,
                SetForegroundColor(Color::Cyan),
                SetAttribute(Attribute::Bold),
                Print(format!("\u{23F5} {}", name)),
                ResetColor,
                SetAttribute(Attribute::Reset),
                SetForegroundColor(Color::DarkGrey),
                Print(format!("({})\n", params)),
                ResetColor,
            );
        } else {
            eprintln!("> {}({})", name, params);
        }
    }

    /// Spec §3.3 — tool "done" lifecycle line, two-space indented under the
    /// running line. `  ↳ summary` Green Dim glyph + DarkGrey summary; ASCII
    /// fallback `  └> summary`. Caller passes already-formatted content.
    pub fn tool_result_ok(&self, content: &str) {
        let summary = truncate_display(content, 500);
        if self.color_enabled {
            let mut stderr = io::stderr();
            let _ = execute!(
                stderr,
                SetForegroundColor(Color::Green),
                SetAttribute(Attribute::Dim),
                Print("  \u{21B3} "),
                ResetColor,
                SetAttribute(Attribute::Reset),
                SetForegroundColor(Color::DarkGrey),
                Print(format!("{}\n", summary)),
                ResetColor,
            );
        } else {
            eprintln!("  └> {}", summary);
        }
    }

    /// Spec §3.3 — tool "error" lifecycle line. `  ✗ msg` Red Bold glyph +
    /// Red msg; ASCII fallback `  X msg`.
    pub fn tool_result_err(&self, content: &str) {
        let summary = truncate_display(content, 500);
        if self.color_enabled {
            let mut stderr = io::stderr();
            let _ = execute!(
                stderr,
                SetForegroundColor(Color::Red),
                SetAttribute(Attribute::Bold),
                Print("  \u{2717} "),
                ResetColor,
                SetAttribute(Attribute::Reset),
                SetForegroundColor(Color::Red),
                Print(format!("{}\n", summary)),
                ResetColor,
            );
        } else {
            eprintln!("  X {}", summary);
        }
    }

    /// Spec §3.5 — error rendering with anyhow `Caused by:` chain handling.
    ///
    /// First line: `✗ Error: {summary}` Red Bold glyph + Red summary.
    /// Continuation lines: two-space indent, DarkGrey; `Caused by:` prefix
    /// stays Red Dim. Plain-mode fallback: `error: {msg}` lowercase.
    pub fn error(&self, msg: &str) {
        let mut lines = msg.split('\n');
        let head = lines.next().unwrap_or("");
        if self.color_enabled {
            let mut stderr = io::stderr();
            let _ = execute!(
                stderr,
                SetForegroundColor(Color::Red),
                SetAttribute(Attribute::Bold),
                Print("\u{2717} Error: "),
                ResetColor,
                SetAttribute(Attribute::Reset),
                SetForegroundColor(Color::Red),
                Print(format!("{}\n", head)),
                ResetColor,
            );
            for line in lines {
                let trimmed = line.trim_start();
                if let Some(rest) = trimmed.strip_prefix("Caused by:") {
                    let _ = execute!(
                        stderr,
                        Print("  "),
                        SetForegroundColor(Color::Red),
                        SetAttribute(Attribute::Dim),
                        Print("Caused by:"),
                        ResetColor,
                        SetAttribute(Attribute::Reset),
                        SetForegroundColor(Color::DarkGrey),
                        Print(format!("{}\n", rest)),
                        ResetColor,
                    );
                } else {
                    let _ = execute!(
                        stderr,
                        SetForegroundColor(Color::DarkGrey),
                        Print(format!("  {}\n", trimmed)),
                        ResetColor,
                    );
                }
            }
        } else {
            eprintln!("error: {}", head);
            for line in lines {
                eprintln!("{}", line.trim_start());
            }
        }
    }

    /// Print session info
    pub fn session_info(&self, msg: &str) {
        if self.color_enabled {
            let mut stderr = io::stderr();
            let _ = execute!(
                stderr,
                SetForegroundColor(Color::Blue),
                SetAttribute(Attribute::Dim),
                Print(format!("{}\n", msg)),
                ResetColor,
            );
        } else {
            eprintln!("{}", msg);
        }
    }
}

fn truncate_display(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // Find a char boundary to avoid panicking on multi-byte characters
        let end = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
        format!("{}...", &s[..end])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_formatter_no_color_mode() {
        // Verify construction with no_color=true does not panic
        let _formatter = OutputFormatter::new(true);
    }

    #[test]
    fn test_text_truncation_short_string_unchanged() {
        let result = truncate_display("hello", 10);
        assert_eq!(result, "hello");
    }

    #[test]
    fn test_text_truncation_exact_length_unchanged() {
        let result = truncate_display("helloworld", 10);
        assert_eq!(result, "helloworld");
    }

    #[test]
    fn test_text_truncation_long_string_truncated() {
        let result = truncate_display("hello world this is long", 10);
        assert_eq!(result, "hello worl...");
    }

    #[test]
    fn test_text_truncation_empty_string() {
        let result = truncate_display("", 10);
        assert_eq!(result, "");
    }

    #[test]
    fn test_turn_stats_no_panic() {
        let formatter = OutputFormatter::new(true);
        // Verify turn_stats does not panic with various inputs
        formatter.turn_stats(1, 100, 50, 0, 0);
        formatter.turn_stats(5, 1000, 500, 200, 300);
        formatter.turn_stats(0, 0, 0, 0, 0);
    }

    #[test]
    fn test_text_truncation_cjk_does_not_panic() {
        // Each CJK char is 3 bytes; byte-based slicing at max=200 would land
        // mid-character and panic without the char_indices fix.
        let cjk: String = "你好世界测试".chars().cycle().take(200).collect();
        let result = truncate_display(&cjk, 50);
        assert!(result.ends_with("..."));
    }

    #[test]
    fn test_text_truncation_mixed_cjk_ascii_does_not_panic() {
        let mixed = "abc你好def世界ghi测试".repeat(20);
        let result = truncate_display(&mixed, 30);
        assert!(result.ends_with("..."));
    }

    // Task 4.2 — every new render method must be ANSI-free under no_color.
    // We exercise the constructors directly; the render methods themselves
    // write to io::stdout/stderr which we can't easily capture in unit tests,
    // but we *can* assert the formatter does not panic and that no_color
    // disables color_enabled.

    #[test]
    fn test_new_methods_no_color_no_panic() {
        let f = OutputFormatter::new(true);
        assert!(!f.color_enabled);
        f.repl_prompt();
        f.assistant_marker();
        f.tool_call_running("read_file", r#"{"path":"/nope"}"#);
        f.tool_result_ok(r#"{"ok":true}"#);
        f.tool_result_err("No such file or directory (os error 2)");
        f.error("tool failed: read_file\nCaused by: io error\nCaused by: permission denied");
    }

    #[test]
    fn test_error_chain_single_line() {
        let f = OutputFormatter::new(true);
        // Plain single-line error must not panic and must not require chain.
        f.error("simple failure");
    }

    #[test]
    fn test_tool_lifecycle_truncates_long_input() {
        let f = OutputFormatter::new(true);
        let huge = "x".repeat(5000);
        f.tool_call_running("big_tool", &huge);
        f.tool_result_ok(&huge);
        f.tool_result_err(&huge);
    }
}
