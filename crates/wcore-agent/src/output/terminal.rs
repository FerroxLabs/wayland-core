use super::{OutputFormatter, OutputSink};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::{cursor, execute, terminal};
use std::io::{self, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use wcore_types::message::FinishReason;

/// Spec §3.2 — 10-frame Braille spinner @ 10 fps, DarkGrey, " Thinking…".
const THINKING_FRAMES_UNICODE: &[&str] = &[
    "\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283C}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280F}",
];
const THINKING_FRAMES_ASCII: &[&str] = &["|", "/", "-", "\\"];
const THINKING_SUFFIX: &str = " Thinking…";
const SPINNER_TICK_MS: u64 = 100;

/// Terminal output sink - wraps the existing OutputFormatter for human-readable output
pub struct TerminalSink {
    formatter: OutputFormatter,
    color_enabled: bool,
    /// Spec §3.2: assistant marker fires once per turn on the first non-empty
    /// `emit_text_delta`. Reset by `emit_stream_start`.
    first_delta_pending: AtomicBool,
    /// Spec §3.3 (Task 4.3): set when a tool block has been rendered without
    /// a following text delta. Consumed by the next `assistant_marker` path
    /// in `emit_text_delta` to inject a single blank line before `⏺ ` so
    /// tool blocks don't merge into the next assistant text. Reset by
    /// `emit_stream_start` / `emit_stream_end` / `emit_error`.
    in_tool_block: AtomicBool,
    /// Owns the thinking-spinner tick task. Started by `emit_stream_start`,
    /// torn down by the first `emit_text_delta` / `emit_tool_call` /
    /// `emit_error`, and on Drop to avoid orphaned tasks.
    ///
    /// §3.4 (per-tool `>2s` spinner) — DEFERRED to v0.6.5. Implementing
    /// the per-tool spinner requires plumbing `call_id` into the
    /// `OutputSink::emit_tool_call` trait method (currently
    /// `(name, input)`); that's an additive but cross-cutting change
    /// touching all `OutputSink` impls + engine dispatch sites + tests.
    /// Spec §3.4 explicitly authorises deferral ("Default to (a) — defer.
    /// Trait surface changes are a separate decision.").
    spinner: Mutex<Option<SpinnerHandle>>,
}

struct SpinnerHandle {
    handle: JoinHandle<()>,
    stop_tx: oneshot::Sender<()>,
}

impl TerminalSink {
    pub fn new(no_color: bool) -> Self {
        let formatter = OutputFormatter::new(no_color);
        // Re-derive the same gate the formatter uses so spinner output stays
        // consistent with formatter colour decisions.
        let color_enabled = !no_color
            && std::env::var("NO_COLOR").is_err()
            && is_terminal::is_terminal(io::stderr());
        Self {
            formatter,
            color_enabled,
            first_delta_pending: AtomicBool::new(false),
            in_tool_block: AtomicBool::new(false),
            spinner: Mutex::new(None),
        }
    }

    /// Access the underlying formatter for terminal-specific operations (repl_prompt, session_info)
    pub fn formatter(&self) -> &OutputFormatter {
        &self.formatter
    }

    fn start_thinking_spinner(&self) {
        // Only spin when stderr is a TTY (and we're in colour mode). In
        // non-TTY / NO_COLOR contexts a redraw loop pollutes piped output.
        if !self.color_enabled {
            return;
        }
        // Spawning requires a tokio runtime; if we're not on one, skip.
        if tokio::runtime::Handle::try_current().is_err() {
            return;
        }
        let mut guard = self.spinner.lock().unwrap();
        if guard.is_some() {
            return;
        }
        let (stop_tx, mut stop_rx) = oneshot::channel::<()>();
        let frames: &'static [&'static str] = THINKING_FRAMES_UNICODE;
        let handle = tokio::spawn(async move {
            let mut idx: usize = 0;
            let mut ticker =
                tokio::time::interval(std::time::Duration::from_millis(SPINNER_TICK_MS));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    biased;
                    _ = &mut stop_rx => break,
                    _ = ticker.tick() => {
                        let frame = frames[idx % frames.len()];
                        idx = idx.wrapping_add(1);
                        let mut stderr = io::stderr();
                        // \r to overwrite, frame, suffix, then clear-to-EOL.
                        let _ = execute!(
                            stderr,
                            Print("\r"),
                            SetForegroundColor(Color::DarkGrey),
                            Print(frame),
                            Print(THINKING_SUFFIX),
                            ResetColor,
                            terminal::Clear(terminal::ClearType::UntilNewLine),
                        );
                        let _ = stderr.flush();
                    }
                }
            }
            // On stop, clear the spinner line so the next write starts clean.
            let mut stderr = io::stderr();
            let _ = execute!(
                stderr,
                Print("\r"),
                terminal::Clear(terminal::ClearType::UntilNewLine),
                cursor::MoveToColumn(0),
            );
            let _ = stderr.flush();
        });
        *guard = Some(SpinnerHandle { handle, stop_tx });
    }

    fn stop_thinking_spinner(&self) {
        let mut guard = self.spinner.lock().unwrap();
        if let Some(SpinnerHandle { handle, stop_tx }) = guard.take() {
            // If the receiver was dropped (task already finished) the send
            // returns Err — that's fine, the task is gone.
            let _ = stop_tx.send(());
            handle.abort();
        }
    }
}

impl Drop for TerminalSink {
    fn drop(&mut self) {
        // Spec §3.2: spinner MUST tear down on Drop to avoid orphan tasks.
        if let Ok(mut guard) = self.spinner.lock()
            && let Some(SpinnerHandle { handle, stop_tx }) = guard.take()
        {
            let _ = stop_tx.send(());
            handle.abort();
        }
    }
}

// Reference ASCII spinner so the constant is exercised (plain-mode fallback
// is documented in the spec; the active spinner path is gated to TTY-only).
#[allow(dead_code)]
const _ASCII_SPINNER_REFERENCE: &[&str] = THINKING_FRAMES_ASCII;

impl OutputSink for TerminalSink {
    fn emit_text_delta(&self, text: &str, _msg_id: &str) {
        if text.is_empty() {
            return;
        }
        // First delta of the turn: tear down spinner + emit assistant marker.
        if self.first_delta_pending.swap(false, Ordering::AcqRel) {
            self.stop_thinking_spinner();
            // Spec §3.3 (Task 4.3): inject a blank line between a tool block
            // and the following assistant marker so they don't merge. Lands
            // on stdout to align with the assistant marker stream (the
            // marker itself writes to stdout in `assistant_marker`).
            if self.in_tool_block.swap(false, Ordering::AcqRel) {
                let mut stdout = io::stdout();
                let _ = writeln!(stdout);
                let _ = stdout.flush();
            }
            self.formatter.assistant_marker();
        }
        self.formatter.text_delta(text);
    }

    fn emit_thinking(&self, text: &str, _msg_id: &str) {
        self.formatter.thinking(text);
    }

    fn emit_tool_call(&self, name: &str, input: &str) {
        // Spec §3.2: tool call also tears down the thinking spinner. The
        // tool-call line implicitly opens a new visual block so we suppress
        // the assistant marker until the next text delta arrives.
        self.stop_thinking_spinner();
        // A tool call mid-turn must NOT emit `⏺ ` for the in-progress text
        // block — but a fresh delta after the tool result still wants a
        // marker. Re-arm the marker latch.
        self.first_delta_pending.store(true, Ordering::Release);
        // Spec §3.3 (Task 4.3): mark that we're rendering a tool block so
        // the next assistant marker injects a leading blank line.
        self.in_tool_block.store(true, Ordering::Release);
        self.formatter.tool_call_running(name, input);
    }

    fn emit_tool_result(&self, _name: &str, is_error: bool, content: &str) {
        if is_error {
            self.formatter.tool_result_err(content);
        } else {
            self.formatter.tool_result_ok(content);
        }
    }

    fn emit_stream_start(&self, _msg_id: &str) {
        // Arm the assistant-marker latch and start the thinking spinner.
        self.first_delta_pending.store(true, Ordering::Release);
        // Spec §3.3 (Task 4.3): new turn begins — reset tool-block flag so
        // a stale flag from a prior turn can't inject a phantom blank line.
        self.in_tool_block.store(false, Ordering::Release);
        self.start_thinking_spinner();
    }

    fn emit_stream_end(
        &self,
        _msg_id: &str,
        turns: usize,
        input_tokens: u64,
        output_tokens: u64,
        cache_creation_tokens: u64,
        cache_read_tokens: u64,
        finish_reason: FinishReason,
    ) {
        // If the stream ended without any text (e.g. tool-only turn or
        // immediate error), ensure the spinner is down before printing stats.
        self.stop_thinking_spinner();
        self.first_delta_pending.store(false, Ordering::Release);
        self.in_tool_block.store(false, Ordering::Release);
        self.formatter.turn_stats(
            turns,
            input_tokens,
            output_tokens,
            cache_creation_tokens,
            cache_read_tokens,
        );
        // Make truncation visible to terminal users — Gemini Pro reasoning
        // models exhaust the thinking-token budget silently; surfacing
        // `length` here closes that gap for CLI sessions.
        if finish_reason == FinishReason::Length {
            self.formatter.session_info(
                "[truncated] response stopped at the max_tokens budget — visible output may be incomplete",
            );
        }
    }

    fn emit_error(&self, msg: &str, _retryable: bool) {
        // Spec §3.2: error also tears down the thinking spinner.
        self.stop_thinking_spinner();
        self.first_delta_pending.store(false, Ordering::Release);
        self.in_tool_block.store(false, Ordering::Release);
        self.formatter.error(msg);
    }

    fn emit_info(&self, msg: &str) {
        self.formatter.session_info(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_sink_construct_no_color() {
        let sink = TerminalSink::new(true);
        assert!(!sink.color_enabled);
        // first_delta_pending defaults to false; emit_text_delta without a
        // stream_start must not panic and must not call the marker.
        sink.emit_text_delta("hi", "m1");
    }

    #[test]
    fn test_first_delta_latch_arms_on_stream_start() {
        let sink = TerminalSink::new(true);
        // No tokio runtime here, so start_thinking_spinner short-circuits.
        sink.emit_stream_start("m1");
        assert!(sink.first_delta_pending.load(Ordering::Acquire));
        sink.emit_text_delta("hello", "m1");
        assert!(!sink.first_delta_pending.load(Ordering::Acquire));
    }

    #[test]
    fn test_tool_call_rearms_marker_latch() {
        let sink = TerminalSink::new(true);
        sink.emit_stream_start("m1");
        sink.emit_text_delta("partial", "m1");
        // After first delta, latch is consumed.
        assert!(!sink.first_delta_pending.load(Ordering::Acquire));
        // Tool call should re-arm so the next text delta paints a marker.
        sink.emit_tool_call("read_file", r#"{"path":"x"}"#);
        assert!(sink.first_delta_pending.load(Ordering::Acquire));
    }

    #[test]
    fn test_emit_error_clears_latch() {
        let sink = TerminalSink::new(true);
        sink.emit_stream_start("m1");
        assert!(sink.first_delta_pending.load(Ordering::Acquire));
        sink.emit_error("boom", false);
        assert!(!sink.first_delta_pending.load(Ordering::Acquire));
    }

    #[test]
    fn test_stream_end_clears_latch_and_spinner() {
        let sink = TerminalSink::new(true);
        sink.emit_stream_start("m1");
        sink.emit_stream_end("m1", 1, 10, 5, 0, 0, FinishReason::Stop);
        assert!(!sink.first_delta_pending.load(Ordering::Acquire));
        assert!(sink.spinner.lock().unwrap().is_none());
    }

    /// Spec §3.3 (Task 4.3): a tool block followed by a fresh text delta
    /// must set the in_tool_block latch so the assistant marker is preceded
    /// by a blank line. Verifies the flag transitions only — actual byte
    /// output goes to stdout/stderr and isn't easily captured here.
    #[test]
    fn test_tool_block_flag_set_by_tool_call_and_cleared_by_text_delta() {
        let sink = TerminalSink::new(true);
        sink.emit_stream_start("m1");
        assert!(!sink.in_tool_block.load(Ordering::Acquire));
        sink.emit_tool_call("read_file", r#"{"path":"x"}"#);
        assert!(
            sink.in_tool_block.load(Ordering::Acquire),
            "tool call must set in_tool_block"
        );
        sink.emit_tool_result("read_file", false, "ok");
        // tool_result doesn't change the flag; the next assistant text delta
        // consumes it.
        assert!(sink.in_tool_block.load(Ordering::Acquire));
        sink.emit_text_delta("here is the result", "m1");
        assert!(
            !sink.in_tool_block.load(Ordering::Acquire),
            "first delta after tool block must clear in_tool_block"
        );
    }

    /// Spec §3.3 (Task 4.3): stream_start resets in_tool_block so a stale
    /// flag from a prior turn doesn't inject a phantom blank line.
    #[test]
    fn test_stream_start_resets_in_tool_block() {
        let sink = TerminalSink::new(true);
        sink.emit_stream_start("m1");
        sink.emit_tool_call("read_file", r#"{"path":"x"}"#);
        assert!(sink.in_tool_block.load(Ordering::Acquire));
        // Simulate a new turn: stream_start must clear the latch.
        sink.emit_stream_start("m2");
        assert!(
            !sink.in_tool_block.load(Ordering::Acquire),
            "stream_start must reset in_tool_block"
        );
    }

    /// Spec §3.3 (Task 4.3): error/stream_end paths also clear the flag so
    /// a tool block followed by an error doesn't leave the latch armed.
    #[test]
    fn test_error_and_stream_end_clear_in_tool_block() {
        let sink = TerminalSink::new(true);
        sink.emit_stream_start("m1");
        sink.emit_tool_call("read_file", r#"{"path":"x"}"#);
        assert!(sink.in_tool_block.load(Ordering::Acquire));
        sink.emit_error("boom", false);
        assert!(!sink.in_tool_block.load(Ordering::Acquire));

        sink.emit_stream_start("m2");
        sink.emit_tool_call("read_file", r#"{"path":"x"}"#);
        sink.emit_stream_end("m2", 1, 10, 5, 0, 0, FinishReason::Stop);
        assert!(!sink.in_tool_block.load(Ordering::Acquire));
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn test_spinner_lifecycle_in_runtime() {
        // Force color_enabled=true to exercise the spinner branch even on
        // CI where stderr is not a TTY. We construct the sink with the
        // public API then poke the internal flag for this test only.
        let mut sink = TerminalSink::new(true);
        sink.color_enabled = true;
        sink.emit_stream_start("m1");
        // Spinner should have started.
        assert!(sink.spinner.lock().unwrap().is_some());
        // Advance virtual time past one tick to prove the loop runs.
        tokio::time::advance(std::time::Duration::from_millis(150)).await;
        sink.emit_text_delta("first", "m1");
        // First-delta latch consumed and spinner torn down.
        assert!(!sink.first_delta_pending.load(Ordering::Acquire));
        assert!(sink.spinner.lock().unwrap().is_none());
    }
}
