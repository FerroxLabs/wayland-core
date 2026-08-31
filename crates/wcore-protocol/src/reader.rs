use std::io::{BufRead, BufReader, Read};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;

use crate::commands::ProtocolCommand;
use crate::events::{ErrorInfo, ProtocolEvent};
use crate::writer::ProtocolEmitter;

/// Maximum bytes accepted for a single protocol line before it is rejected.
///
/// Audit DoS — a compromised/buggy host can send a long, newline-free run on
/// stdin. A bare `read_line`/`read_until` has no byte cap, so that run grows
/// the line buffer until the process OOMs. 8 MiB is far larger than any
/// legitimate protocol command yet bounds the worst case. Matches the MCP
/// stdio transport's `MAX_LINE_BYTES` (see `wcore-mcp` transport/stdio.rs).
const MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;

/// Commands are never dropped: the dedicated reader thread blocks when this
/// queue is full, applying backpressure to the host pipe instead of growing
/// process memory without bound.
const STDIN_COMMAND_CAPACITY: usize = 64;

/// Bound on host-supplied text quoted back inside a rejection message.
///
/// FerroxLabs/wayland#1070 — a rejection names the offending `type` and the
/// deserializer's reason so the host can fix the frame, but both are
/// host-controlled: an unknown variant name (or a `missing field` list) can be
/// megabytes long, and the rejection travels to a host UI. Both are truncated.
const MAX_REJECTION_ECHO_BYTES: usize = 120;

/// Receiver for JSON-stream commands read from process stdin.
///
/// Dropping this value cooperatively closes command admission. A reader thread
/// already blocked in an operating-system stdin read cannot be interrupted
/// portably, so that thread is intentionally detached from Tokio: it owns only
/// stdin, the bounded sender, and this cancellation flag, and is terminated by
/// the operating system when the process exits. It therefore cannot delay
/// Tokio runtime shutdown or retain cleanup-critical resources.
pub struct StdinReader {
    receiver: mpsc::Receiver<ProtocolCommand>,
    cancelled: Arc<AtomicBool>,
}

impl StdinReader {
    pub async fn recv(&mut self) -> Option<ProtocolCommand> {
        self.receiver.recv().await
    }

    /// Close command admission while preserving the receiver's buffered
    /// commands, matching Tokio receiver semantics.
    pub fn close(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        self.receiver.close();
    }
}

impl Deref for StdinReader {
    type Target = mpsc::Receiver<ProtocolCommand>;

    fn deref(&self) -> &Self::Target {
        &self.receiver
    }
}

impl DerefMut for StdinReader {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.receiver
    }
}

impl Drop for StdinReader {
    fn drop(&mut self) {
        self.close();
    }
}

/// Reads JSON Lines from stdin on a dedicated operating-system thread.
/// Returns a bounded, backpressured command receiver.
///
/// `errors` receives one `error` event per line this reader refuses. Before
/// FerroxLabs/wayland#1070 a refused line was logged and dropped, so a host
/// that sent a slightly wrong command saw nothing at all and could not tell a
/// rejection from a wedged engine.
pub fn spawn_stdin_reader(errors: Arc<dyn ProtocolEmitter>) -> StdinReader {
    let (tx, receiver) = mpsc::channel(STDIN_COMMAND_CAPACITY);
    let cancelled = Arc::new(AtomicBool::new(false));
    let thread_cancelled = Arc::clone(&cancelled);

    let spawned = std::thread::Builder::new()
        .name("wcore-json-stdin".to_string())
        .spawn(move || {
            let stdin = std::io::stdin();
            read_commands(
                BufReader::new(stdin.lock()),
                tx,
                &thread_cancelled,
                errors.as_ref(),
            );
        });
    if let Err(error) = spawned {
        tracing::error!(%error, "could not start protocol stdin reader");
    }

    StdinReader {
        receiver,
        cancelled,
    }
}

/// Drive one capped line read per iteration, parse it, and forward parsed
/// commands to `tx`. Returns when the reader hits EOF, a read error, or the
/// receiver is dropped.
///
/// Generic over the reader so the byte-cap behavior is unit-testable without
/// touching the real stdin.
fn read_commands<R: BufRead>(
    mut reader: R,
    tx: mpsc::Sender<ProtocolCommand>,
    cancelled: &AtomicBool,
    errors: &dyn ProtocolEmitter,
) {
    // Capped line reader. `read_until` on a `take(MAX_LINE_BYTES)` limiter
    // stops at the byte cap even if no newline arrives, so an endless
    // newline-free stream can't grow the buffer unbounded. Overflow is
    // detected as "filled the cap without a terminating newline".
    let mut raw: Vec<u8> = Vec::new();
    loop {
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        raw.clear();
        let read = match (&mut reader)
            .take(MAX_LINE_BYTES)
            .read_until(b'\n', &mut raw)
        {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!(error = %e, "protocol stdin read failed");
                break;
            }
        };
        if read == 0 {
            break; // EOF - client closed stdin
        }

        // Overflow: hit the byte cap with no line terminator. A legitimate
        // protocol command is newline-delimited and far under the cap, so
        // this is a misbehaving/hostile host. Surface a structured error,
        // discard the rest of the oversized line up to the next newline so
        // its tail is not mis-parsed as a fresh command, then resume.
        if read as u64 >= MAX_LINE_BYTES && raw.last() != Some(&b'\n') {
            tracing::warn!(
                max_line_bytes = MAX_LINE_BYTES,
                "protocol line exceeded byte cap; discarding oversized input and resuming"
            );
            reject(
                errors,
                format!(
                    "invalid protocol command: line exceeded the {MAX_LINE_BYTES}-byte limit and \
                     was discarded"
                ),
            );
            if !discard_to_newline(&mut reader, cancelled) {
                break; // EOF or error while discarding — stop the reader
            }
            // `clear()` retains the multi-MiB capacity; reallocate so one
            // oversized line does not permanently inflate RSS.
            raw = Vec::new();
            continue;
        }

        let line = String::from_utf8_lossy(&raw);
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if cancelled.load(Ordering::Acquire) {
            break;
        }
        match serde_json::from_str::<ProtocolCommand>(trimmed) {
            Ok(cmd) => {
                if tx.blocking_send(cmd).is_err() {
                    break;
                }
            }
            Err(e) => {
                // F-074: include the expected JSON shape in the
                // error message so developers debugging
                // integration issues can identify the problem
                // without reading protocol docs. Example of the
                // minimal required shape is shown in the hint.
                tracing::warn!(
                    error = %e,
                    "invalid protocol command; expected JSON with a type field"
                );
                // FerroxLabs/wayland#1070: and tell the HOST, not just the
                // log. A dropped line used to be indistinguishable from a
                // hung engine.
                reject(errors, rejection_message(trimmed, &e));
            }
        }
    }
}

/// Emit one host-facing `error` frame for a line the reader refused.
///
/// Delivery failure is deliberately ignored: the emitter's own failure handler
/// already reports a broken stdout, and a reader that stopped reading stdin
/// because stdout broke would be a worse failure than the one being reported.
fn reject(errors: &dyn ProtocolEmitter, message: String) {
    let _ = errors.emit(&ProtocolEvent::Error {
        msg_id: None,
        error: ErrorInfo {
            // `engine_error` is the protocol's documented default code. A
            // rejection deliberately introduces NO new code: the host-facing
            // vocabulary stays exactly as `docs/json-stream-protocol.md`
            // catalogues it, and the detail rides the message.
            code: "engine_error".to_string(),
            message,
            retryable: false,
            // wayland#1237: the LOCAL reader refused a malformed line. The
            // vocabulary of `code` deliberately does not widen (above); the
            // category is where the machine-readable answer goes.
            category: crate::events::FailureCategory::LocalWayland,
        },
    });
}

/// Truncate host-supplied text to [`MAX_REJECTION_ECHO_BYTES`] on a character
/// boundary, marking the cut so a reader knows the text was elided.
fn truncate_echo(text: &str) -> String {
    if text.len() <= MAX_REJECTION_ECHO_BYTES {
        return text.to_string();
    }
    let mut end = MAX_REJECTION_ECHO_BYTES;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &text[..end])
}

/// Compose the rejection for a line serde refused, naming the offending
/// command `type` when the line at least parsed as a JSON object carrying one.
///
/// The deserializer's own reason is quoted because it is the field-level
/// detail the host needs ("missing field `content`", "unknown variant `nope`");
/// both it and the echoed `type` are length-bounded.
fn rejection_message(line: &str, error: &serde_json::Error) -> String {
    let named_type = serde_json::from_str::<serde_json::Value>(line)
        .ok()
        .and_then(|value| {
            value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .map(truncate_echo)
        });
    let reason = truncate_echo(&error.to_string());
    match named_type {
        Some(command_type) => {
            format!("invalid protocol command of type \"{command_type}\": {reason}")
        }
        None => format!(
            "invalid protocol command: {reason} (expected one JSON object per line with a \
             string \"type\" field)"
        ),
    }
}

/// Drain bytes from `reader` until (and including) the next newline, so the
/// remainder of an oversized line is consumed without buffering it. Reads in
/// bounded chunks via `fill_buf`/`consume` — never accumulates the discarded
/// bytes. Returns `false` on EOF or read error (caller should stop).
fn discard_to_newline<R: BufRead>(reader: &mut R, cancelled: &AtomicBool) -> bool {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return false;
        }
        let buf = match reader.fill_buf() {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "protocol stdin read failed while discarding oversized input"
                );
                return false;
            }
        };
        if buf.is_empty() {
            return false; // EOF before a newline
        }
        match buf.iter().position(|&b| b == b'\n') {
            Some(pos) => {
                reader.consume(pos + 1);
                return true;
            }
            None => {
                let len = buf.len();
                reader.consume(len);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Captures emitted frames as their wire JSON, so an assertion reads the
    /// shape a host actually receives rather than a Rust enum.
    #[derive(Default)]
    struct RecordingEmitter {
        emitted: std::sync::Mutex<Vec<serde_json::Value>>,
    }

    impl RecordingEmitter {
        /// The `message` of every `error` frame, in order.
        fn error_messages(&self) -> Vec<String> {
            self.emitted
                .lock()
                .unwrap()
                .iter()
                .filter(|value| value["type"] == "error")
                .map(|value| value["error"]["message"].as_str().unwrap().to_string())
                .collect()
        }
    }

    impl ProtocolEmitter for RecordingEmitter {
        fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
            self.emitted
                .lock()
                .unwrap()
                .push(serde_json::to_value(event).unwrap());
            Ok(())
        }
    }

    /// Drive the reader over `input` and return (forwarded commands, emitter).
    fn drive(input: &[u8]) -> (Vec<ProtocolCommand>, RecordingEmitter) {
        let (tx, mut rx) = mpsc::channel(8);
        let cancelled = AtomicBool::new(false);
        let errors = RecordingEmitter::default();
        read_commands(
            BufReader::new(std::io::Cursor::new(input.to_vec())),
            tx,
            &cancelled,
            &errors,
        );
        let mut commands = Vec::new();
        while let Some(cmd) = rx.blocking_recv() {
            commands.push(cmd);
        }
        (commands, errors)
    }

    /// FerroxLabs/wayland#1070 (a) — a command whose `type` this build does
    /// not know is answered on the wire, naming the offending type. The
    /// pre-fix reader logged and dropped it, which a host cannot distinguish
    /// from a hang.
    #[test]
    fn unknown_command_type_is_rejected_on_the_wire() {
        let (commands, errors) = drive(b"{\"type\":\"teleport\"}\n");

        assert!(
            commands.is_empty(),
            "an unknown type must not be dispatched"
        );
        let messages = errors.error_messages();
        assert_eq!(messages.len(), 1, "expected exactly one rejection");
        assert!(
            messages[0].contains("teleport"),
            "the rejection must name the offending type; got {:?}",
            messages[0]
        );
    }

    /// #1070 (a) — a raw non-JSON line is answered too, and the rejection
    /// says what the reader expected instead.
    #[test]
    fn non_json_line_is_rejected_on_the_wire() {
        let (commands, errors) = drive(b"this is not json\n");

        assert!(commands.is_empty());
        let messages = errors.error_messages();
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].contains("invalid protocol command")
                && messages[0].contains("\"type\" field"),
            "a non-JSON line must be told what shape was expected; got {:?}",
            messages[0]
        );
    }

    /// #1070 (a) — a known `type` missing a required field names the FIELD,
    /// which is the detail a host integrator needs to fix the frame.
    #[test]
    fn message_missing_a_required_field_is_rejected_naming_the_field() {
        let (commands, errors) = drive(b"{\"type\":\"message\",\"msg_id\":\"m1\"}\n");

        assert!(commands.is_empty());
        let messages = errors.error_messages();
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].contains("message") && messages[0].contains("content"),
            "the rejection must name the type and the missing field; got {:?}",
            messages[0]
        );
    }

    /// CONTROL for the three tests above: a well-formed command must still be
    /// dispatched and must emit NO error. Without this the rejection path
    /// could be firing on every line and the tests above would still pass.
    #[test]
    fn well_formed_commands_are_dispatched_and_never_rejected() {
        let (commands, errors) = drive(
            b"{\"type\":\"ping\"}\n{\"type\":\"message\",\"msg_id\":\"m1\",\"content\":\"hi\"}\n",
        );

        assert_eq!(
            commands,
            vec![
                ProtocolCommand::Ping,
                ProtocolCommand::Message {
                    msg_id: "m1".to_string(),
                    content: "hi".to_string(),
                    files: Vec::new(),
                },
            ]
        );
        assert!(
            errors.error_messages().is_empty(),
            "a valid command must not produce an error frame; got {:?}",
            errors.error_messages()
        );
    }

    /// A rejection must not stop the stream: the reader answers the bad line
    /// and keeps dispatching the good ones around it.
    #[test]
    fn a_rejected_line_does_not_stop_the_commands_around_it() {
        let (commands, errors) =
            drive(b"{\"type\":\"ping\"}\n{\"type\":\"teleport\"}\n{\"type\":\"ping\"}\n");

        assert_eq!(commands, vec![ProtocolCommand::Ping, ProtocolCommand::Ping]);
        assert_eq!(errors.error_messages().len(), 1);
    }

    /// The echoed type and reason are host-controlled, so the rejection is
    /// length-bounded — a megabyte-long `type` must not become a megabyte-long
    /// event on a host UI.
    #[test]
    fn a_rejection_bounds_the_host_supplied_text_it_echoes() {
        let huge = "z".repeat(64 * 1024);
        let line = format!("{{\"type\":\"{huge}\"}}\n");

        let (_, errors) = drive(line.as_bytes());

        let messages = errors.error_messages();
        assert_eq!(messages.len(), 1);
        assert!(
            messages[0].len() < 1024,
            "rejection grew with the host's input: {} bytes",
            messages[0].len()
        );
        assert!(
            messages[0].contains("zzz") && messages[0].contains("..."),
            "the echo must be truncated, not dropped; got {:?}",
            messages[0]
        );
    }

    /// A line exceeding `MAX_LINE_BYTES` is rejected (never forwarded) and a
    /// following valid line still parses — proving the reader resumes at the
    /// next newline rather than OOMing or mis-parsing the oversized tail.
    #[test]
    fn oversized_line_is_skipped_then_next_line_parses() {
        let (tx, mut rx) = mpsc::channel(4);
        let cancelled = AtomicBool::new(false);

        // One oversized newline-free run (cap + slack), a newline, then a
        // valid command. The oversized run must not be buffered whole.
        let oversized = vec![b'a'; MAX_LINE_BYTES as usize + 1024];
        let mut input = oversized;
        input.push(b'\n');
        input.extend_from_slice(br#"{"type":"ping"}"#);
        input.push(b'\n');

        let reader = BufReader::new(std::io::Cursor::new(input));
        let errors = RecordingEmitter::default();
        read_commands(reader, tx, &cancelled, &errors);

        // Only the valid command comes through; the oversized line yields
        // no ProtocolCommand.
        let first = rx.blocking_recv();
        assert_eq!(first, Some(ProtocolCommand::Ping));
        assert!(rx.blocking_recv().is_none(), "no extra commands expected");

        // #1070: the discard is reported to the host rather than only logged.
        let messages = errors.error_messages();
        assert_eq!(
            messages.len(),
            1,
            "expected one rejection for the oversized line"
        );
        assert!(
            messages[0].contains("exceeded"),
            "the rejection must say the line was too long; got {:?}",
            messages[0]
        );
    }

    /// A normal line parses, and an oversized line in the middle of a stream
    /// does not corrupt the lines around it.
    #[test]
    fn valid_line_before_and_after_oversized_line() {
        let (tx, mut rx) = mpsc::channel(4);
        let cancelled = AtomicBool::new(false);

        let mut input = Vec::new();
        input.extend_from_slice(br#"{"type":"ping"}"#);
        input.push(b'\n');
        input.extend(std::iter::repeat_n(b'b', MAX_LINE_BYTES as usize + 1));
        input.push(b'\n');
        input.extend_from_slice(br#"{"type":"ping"}"#);
        input.push(b'\n');

        let reader = BufReader::new(std::io::Cursor::new(input));
        let errors = RecordingEmitter::default();
        read_commands(reader, tx, &cancelled, &errors);

        assert_eq!(rx.blocking_recv(), Some(ProtocolCommand::Ping));
        assert_eq!(rx.blocking_recv(), Some(ProtocolCommand::Ping));
        assert!(
            rx.blocking_recv().is_none(),
            "only two valid pings expected"
        );
        assert_eq!(errors.error_messages().len(), 1);
    }

    /// A line exactly at the cap that IS newline-terminated is valid input,
    /// not an overflow — boundary check so we don't reject legitimate large
    /// (but bounded) commands.
    #[test]
    fn line_at_cap_with_newline_is_not_treated_as_overflow() {
        let (tx, mut rx) = mpsc::channel(4);
        let cancelled = AtomicBool::new(false);

        // A valid command padded with trailing JSON whitespace up to just
        // under the cap, then a newline. `read_until` reads cap-or-fewer
        // bytes including the newline, so this stays within the limiter.
        let cmd = br#"{"type":"ping"}"#;
        let mut input = cmd.to_vec();
        let pad = MAX_LINE_BYTES as usize - cmd.len() - 1;
        input.extend(std::iter::repeat_n(b' ', pad));
        input.push(b'\n');

        let reader = BufReader::new(std::io::Cursor::new(input));
        let errors = RecordingEmitter::default();
        read_commands(reader, tx, &cancelled, &errors);

        assert_eq!(rx.blocking_recv(), Some(ProtocolCommand::Ping));
        assert!(rx.blocking_recv().is_none());
        // Control for the overflow rejection: a legitimate at-the-cap line is
        // accepted silently, so the rejection is not firing on size alone.
        assert!(errors.error_messages().is_empty());
    }

    #[test]
    fn cancelled_reader_admits_no_commands() {
        let (tx, mut rx) = mpsc::channel(1);
        let cancelled = AtomicBool::new(true);
        let reader = BufReader::new(std::io::Cursor::new(b"{\"type\":\"ping\"}\n"));

        read_commands(reader, tx, &cancelled, &RecordingEmitter::default());

        assert!(rx.blocking_recv().is_none());
    }

    #[test]
    fn unsupported_runtime_diagnostics_version_reaches_correlated_dispatch() {
        let (tx, mut rx) = mpsc::channel(2);
        let cancelled = AtomicBool::new(false);
        let reader = BufReader::new(std::io::Cursor::new(
            b"{\"type\":\"get_runtime_diagnostics\",\"diagnostics_version\":2,\"request_id\":\"bad-version\"}\n{\"type\":\"ping\"}\n",
        ));

        read_commands(reader, tx, &cancelled, &RecordingEmitter::default());

        assert_eq!(
            rx.blocking_recv(),
            Some(ProtocolCommand::GetRuntimeDiagnostics(
                crate::diagnostics::GetRuntimeDiagnosticsCommand {
                    diagnostics_version: 2,
                    request_id: "bad-version".into(),
                }
            ))
        );
        assert_eq!(rx.blocking_recv(), Some(ProtocolCommand::Ping));
        assert!(rx.blocking_recv().is_none());
    }
}
