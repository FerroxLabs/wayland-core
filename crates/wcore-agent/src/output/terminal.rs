use super::{OutputFormatter, OutputSink};
use crossterm::style::{Color, Print, ResetColor, SetForegroundColor};
use crossterm::{cursor, execute, terminal};
use std::io::{self, Write};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use wcore_types::message::FinishReason;
use wcore_types::reasoning_filter::ReasoningFilter;

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
    /// One-shot mode: a prompt was supplied on argv, so this process emits
    /// exactly ONE assistant answer and exits. Set via [`TerminalSink::one_shot`].
    ///
    /// Two behaviours change, both measured against UAT-TUI-WINDOWS F4/F5 where
    /// `wayland-core --no-tui '17 x 23'` wrote **five bytes** — `2a 20 33 39 31`,
    /// i.e. `* 391` with no terminator:
    ///
    /// 1. The §3.2 assistant turn marker (`⏺ ` / `* `) is suppressed. The marker
    ///    answers "which speaker is this?", which carries no information when
    ///    there is exactly one speaker, no prompt echo and no following turn —
    ///    and it makes the stream non-machine-consumable without stripping two
    ///    bytes. The REPL and TUI paths are untouched, where the marker does
    ///    separate turns.
    /// 2. stdout is terminated with a newline if the answer did not already end
    ///    in one, so the next shell prompt (or the next line of a log) does not
    ///    run into the answer.
    one_shot: bool,
    /// Whether any assistant text has been written to stdout this turn, and
    /// whether the last byte written was `\n`. Both are needed to decide the
    /// one-shot terminator: an empty turn must not gain a stray blank line, and
    /// an answer that already ends in a newline must not gain a second one.
    wrote_text: AtomicBool,
    last_byte_newline: AtomicBool,
    /// Set once [`OutputSink::emit_durability_degraded`] has printed. See that
    /// method's override below — the fact is a process property, not a turn
    /// property, so the human hears it once.
    durability_degrade_announced: AtomicBool,
    /// The last error text this sink printed, for the restatement guard in
    /// [`OutputSink::emit_error`].
    last_error: Mutex<Option<String>>,
    /// #908 — inline-reasoning split for this sink's visible text lane.
    ///
    /// `ProtocolSink` and `ChannelSink` both strip `<think>`-class tags out of
    /// `text_delta` and re-emit the body on the thinking lane. This sink did
    /// not, so the plain terminal (`--no-tui`, one-shot, REPL) printed a
    /// literal `<think>…</think>` — and a stray `</thought>` — straight to
    /// stdout. Its own filter, not a shared one: a filter's pending buffer is
    /// per-consumer state and two sinks may be attached to one engine.
    ///
    /// Stateful across chunks by design (a tag straddles `<thi` | `nk>`);
    /// reset on `emit_stream_start`, drained after every chunk.
    reasoning: Mutex<ReasoningFilter>,
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
            one_shot: false,
            wrote_text: AtomicBool::new(false),
            last_byte_newline: AtomicBool::new(false),
            durability_degrade_announced: AtomicBool::new(false),
            last_error: Mutex::new(None),
            reasoning: Mutex::new(ReasoningFilter::new()),
        }
    }

    /// Mark this sink as serving a one-shot `wayland-core "<prompt>"` run.
    ///
    /// Consuming builder rather than a `new` parameter so the ~20 existing
    /// `TerminalSink::new(bool)` call sites keep compiling unchanged and keep
    /// the interactive behaviour they were written against.
    #[must_use]
    pub fn one_shot(mut self) -> Self {
        self.one_shot = true;
        self
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

impl TerminalSink {
    /// #908 — run one chunk through this sink's reasoning filter and return
    /// `(visible text, reasoning drained by this chunk)`.
    ///
    /// Split out of [`OutputSink::emit_text_delta`] so the filtering is
    /// assertable: the emit path writes to the process's real stdout, which a
    /// unit test cannot capture, but this is where the decision is made.
    fn split_reasoning(&self, text: &str) -> (String, String) {
        match self.reasoning.lock() {
            Ok(mut filter) => {
                let visible = filter.process(text);
                (visible, filter.take_captured_delta())
            }
            // A poisoned filter mutex means another thread panicked mid-chunk.
            // Showing the raw chunk is the honest fallback: hiding output on a
            // lock fault would be a worse failure than an unfiltered tag.
            Err(_) => (text.to_string(), String::new()),
        }
    }

    /// Put already-filtered text on the visible lane, with the marker and
    /// spinner bookkeeping that goes with "the user saw assistant text".
    ///
    /// Split out of [`OutputSink::emit_text_delta`] for #1242: the
    /// end-of-stream drain has text to show that has ALREADY been through the
    /// filter, and putting it back through `process` would re-parse the very
    /// tag the drain just recovered and swallow it a second time.
    fn show(&self, visible: &str) {
        // A chunk the filter consumed WHOLE (pure reasoning, or the leading
        // half of a tag straddling the boundary) is not assistant text: it
        // must not fire the turn marker or set `wrote_text`, or a
        // reasoning-only turn would gain a phantom `⏺ ` and a stray newline.
        if visible.is_empty() {
            return;
        }
        let text = visible;
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
            // One-shot answers carry no speaker marker — see the `one_shot`
            // field docs. Interactive REPL turns still get `⏺ ` / `* `.
            if !self.one_shot {
                self.formatter.assistant_marker();
            }
        }
        self.formatter.text_delta(text);
        self.wrote_text.store(true, Ordering::Release);
        self.last_byte_newline
            .store(text.ends_with('\n'), Ordering::Release);
    }
}

impl OutputSink for TerminalSink {
    fn emit_text_delta(&self, text: &str, _msg_id: &str) {
        if text.is_empty() {
            return;
        }
        // #908 — split inline reasoning out of the visible lane before any of
        // the marker/spinner bookkeeping below, which is keyed on "the user
        // saw assistant text". The withheld body is rendered as thinking
        // rather than deleted, so the local reader loses nothing.
        let (visible, captured) = self.split_reasoning(text);
        if !captured.is_empty() {
            self.formatter.thinking(&captured);
        }
        self.show(&visible);
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
        // Per-turn, not per-process: an agentic one-shot run streams several
        // assistant blocks (text → tool → text). Each block is terminated on
        // its own `emit_stream_end`, which is also what keeps consecutive
        // blocks from running together now that the marker is suppressed.
        self.wrote_text.store(false, Ordering::Release);
        self.last_byte_newline.store(false, Ordering::Release);
        // #908 — a leftover pending tag prefix from a cancelled turn must not
        // eat the first characters of this one.
        if let Ok(mut filter) = self.reasoning.lock() {
            filter.reset();
        }
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
        // #1242 — drain whatever the reasoning filter is still holding and
        // SHOW it. `process` is a lossy view of the stream: an undecided
        // `<`-prefix and an unclosed reasoning block are both withheld while
        // the stream runs, and used to be dropped when it ended. The engine's
        // history-side twin has drained since #1222, so without this the
        // terminal and the stored turn disagree about the same answer.
        //
        // Before the spinner teardown and the stats line, so recovered text
        // lands where the rest of the answer did rather than after the footer.
        let recovered = match self.reasoning.lock() {
            Ok(mut filter) => filter.finish(),
            Err(_) => String::new(),
        };
        self.show(&recovered);
        // If the stream ended without any text (e.g. tool-only turn or
        // immediate error), ensure the spinner is down before printing stats.
        self.stop_thinking_spinner();
        self.first_delta_pending.store(false, Ordering::Release);
        self.in_tool_block.store(false, Ordering::Release);
        // One-shot: terminate stdout so the next writer starts on its own line.
        // UAT-TUI-WINDOWS F4 measured `live1.stdout` at exactly 5 bytes with no
        // terminator; UAT-TUI-UNIX F8 is the same defect seen from the other
        // side, where a stderr log line rendered onto the answer's line. Guarded
        // on `wrote_text` so a tool-only or errored turn gains no blank line,
        // and on `last_byte_newline` so an answer that already ends in `\n`
        // gains no second one. Emitted BEFORE `turn_stats` (which writes to
        // stderr) so stdout is complete before anything else is printed.
        if self.one_shot
            && self.wrote_text.load(Ordering::Acquire)
            && !self.last_byte_newline.load(Ordering::Acquire)
        {
            let mut stdout = io::stdout();
            let _ = writeln!(stdout);
            let _ = stdout.flush();
            self.last_byte_newline.store(true, Ordering::Release);
        }
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

    /// Print the error, unless the user has just read it.
    ///
    /// One provider fault reaches this sink twice on the CLI path: the engine
    /// reports the failure itself, then returns an `AgentError` and
    /// `wcore_cli`'s `SlashOrRun::Engine(Err(e))` arm renders that too. Both
    /// calls are correct in isolation — the engine's carries the retryable
    /// flag a protocol host consumes, and the CLI's is what makes an error
    /// from anywhere else visible at all — so neither can simply be deleted.
    /// What a human reads is two `error:` lines for one fault, the second
    /// stuttering ("API error: API error 500: …") and carrying no fact the
    /// first did not.
    ///
    /// The rule is containment in EITHER direction, not equality: the second
    /// render wraps the first in the `AgentError` Display prefix
    /// (`"API error: " + text`), so the texts are never equal, and which of
    /// the two is the longer depends on which layer added the prefix. One
    /// error text wholly containing the other means the fault is already on
    /// screen and the extra render adds only a wrapper. Only the immediately
    /// preceding error is remembered — two identical faults separated by other
    /// output are two events the user should see twice.
    ///
    /// Terminal-only, deliberately. `ProtocolSink` emits one frame per call
    /// and a host correlates them by `msg_id`; suppressing a frame there would
    /// change the protocol.
    fn emit_error(&self, msg: &str, _retryable: bool) {
        {
            let mut last = self.last_error.lock().unwrap();
            if last
                .as_deref()
                .is_some_and(|prev| prev.contains(msg) || msg.contains(prev))
            {
                // Keep the text the user actually read as the reference point.
                return;
            }
            *last = Some(msg.to_string());
        }
        // Spec §3.2: error also tears down the thinking spinner.
        self.stop_thinking_spinner();
        self.first_delta_pending.store(false, Ordering::Release);
        self.in_tool_block.store(false, Ordering::Release);
        self.formatter.error(msg);
    }

    fn emit_info(&self, msg: &str) {
        self.formatter.session_info(msg);
    }

    /// Once per sink, which for a CLI/REPL/TUI run is once per process.
    ///
    /// The condition is `durable_sessions_disabled_by_host()` — a host fact
    /// resolved at startup that cannot change while the process lives — so
    /// every repeat carries zero new information. The trait default (per turn)
    /// stays in force for `ProtocolSink`, where the frame is machine-consumed
    /// and correlated to a `msg_id`.
    fn emit_durability_degraded(&self, msg: &str) {
        // Zero times, not once, when config resolution already said it. That
        // startup notice and this one report the SAME immutable host fact in
        // two different wordings, and it reached the same stderr moments
        // earlier — measured at 1,333 of a trivial run's 2,019 stderr bytes.
        // The latch below still stands on its own for the paths that reach a
        // turn without a resolution notice.
        if wcore_config::config::replay_protection_notice_printed() {
            return;
        }
        if self
            .durability_degrade_announced
            .swap(true, Ordering::Relaxed)
        {
            return;
        }
        self.formatter.session_info(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #908 — the plain terminal (`--no-tui`, one-shot, REPL) printed inline
    /// reasoning verbatim: `ProtocolSink` and `ChannelSink` both filtered
    /// `text_delta`, this sink did not.
    ///
    /// Asserted on the seam rather than on captured output because the
    /// formatter writes to the process's real stdout; `split_reasoning` is
    /// where the visible/withheld decision is actually made.
    #[test]
    fn inline_reasoning_is_split_out_of_the_visible_terminal_lane() {
        let sink = TerminalSink::new(true);

        // Straddles the chunk boundary, so a stateless filter would miss it.
        let (v1, c1) = sink.split_reasoning("a<thi");
        let (v2, c2) = sink.split_reasoning("nk>hidden</think>b");
        assert_eq!(format!("{v1}{v2}"), "ab", "reasoning leaked to stdout");
        assert_eq!(
            format!("{c1}{c2}"),
            "hidden",
            "the withheld body must be rendered as thinking, not deleted"
        );
    }

    #[test]
    fn a_stray_reasoning_close_never_reaches_the_terminal() {
        let sink = TerminalSink::new(true);
        let (visible, captured) = sink.split_reasoning("The answer is 42.</thought>");
        assert_eq!(visible, "The answer is 42.");
        assert_eq!(captured, "", "a close with no opener has no body");
    }

    #[test]
    fn the_reasoning_filter_is_reset_between_turns() {
        // An unclosed block eats to end of stream by design; without the
        // per-turn reset it would go on eating the NEXT turn's answer.
        let sink = TerminalSink::new(true);
        assert_eq!(sink.split_reasoning("<think>runaway").0, "");
        sink.emit_stream_start("m2");
        assert_eq!(sink.split_reasoning("fresh answer").0, "fresh answer");
    }

    #[test]
    fn test_terminal_sink_construct_no_color() {
        let sink = TerminalSink::new(true);
        assert!(!sink.color_enabled);
        // first_delta_pending defaults to false; emit_text_delta without a
        // stream_start must not panic and must not call the marker.
        sink.emit_text_delta("hi", "m1");
    }

    /// One provider fault, two renders: the engine reports it, then the CLI
    /// renders the returned `AgentError`, whose Display wraps the same text.
    /// Measured on 0.12.26 as two `error:` lines for one expired key.
    ///
    /// Asserted on `last_error` rather than on captured output because the
    /// formatter writes to the process's real stderr; the latch is what
    /// decides, so it is what is checked. The end-to-end byte count is proved
    /// against the built binary.
    #[test]
    fn a_wrapped_restatement_of_the_last_error_is_not_printed_again() {
        let sink = TerminalSink::new(true);

        OutputSink::emit_error(&sink, "boom: the key was rejected", false);
        assert_eq!(
            sink.last_error.lock().unwrap().as_deref(),
            Some("boom: the key was rejected"),
            "the first render is the one the user reads"
        );

        // The CLI's `{e:#}` render of the same fault: same payload, Display
        // prefix in front.
        OutputSink::emit_error(&sink, "API error: boom: the key was rejected", false);
        assert_eq!(
            sink.last_error.lock().unwrap().as_deref(),
            Some("boom: the key was rejected"),
            "a wrapped restatement must be suppressed, leaving the text the \
             user actually read as the reference point"
        );

        // A genuinely different fault still gets through — without this the
        // guard would be indistinguishable from "print at most one error".
        OutputSink::emit_error(&sink, "a different failure entirely", false);
        assert_eq!(
            sink.last_error.lock().unwrap().as_deref(),
            Some("a different failure entirely"),
            "an unrelated error must still be printed"
        );
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
