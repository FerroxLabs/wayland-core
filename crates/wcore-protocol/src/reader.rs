use std::io::{BufRead, BufReader, Read};
use std::ops::{Deref, DerefMut};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::mpsc;

use crate::commands::ProtocolCommand;

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
pub fn spawn_stdin_reader() -> StdinReader {
    let (tx, receiver) = mpsc::channel(STDIN_COMMAND_CAPACITY);
    let cancelled = Arc::new(AtomicBool::new(false));
    let thread_cancelled = Arc::clone(&cancelled);

    let spawned = std::thread::Builder::new()
        .name("wcore-json-stdin".to_string())
        .spawn(move || {
            let stdin = std::io::stdin();
            read_commands(BufReader::new(stdin.lock()), tx, &thread_cancelled);
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
            }
        }
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
        read_commands(reader, tx, &cancelled);

        // Only the valid command comes through; the oversized line yields
        // no ProtocolCommand.
        let first = rx.blocking_recv();
        assert_eq!(first, Some(ProtocolCommand::Ping));
        assert!(rx.blocking_recv().is_none(), "no extra commands expected");
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
        read_commands(reader, tx, &cancelled);

        assert_eq!(rx.blocking_recv(), Some(ProtocolCommand::Ping));
        assert_eq!(rx.blocking_recv(), Some(ProtocolCommand::Ping));
        assert!(
            rx.blocking_recv().is_none(),
            "only two valid pings expected"
        );
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
        read_commands(reader, tx, &cancelled);

        assert_eq!(rx.blocking_recv(), Some(ProtocolCommand::Ping));
        assert!(rx.blocking_recv().is_none());
    }

    #[test]
    fn cancelled_reader_admits_no_commands() {
        let (tx, mut rx) = mpsc::channel(1);
        let cancelled = AtomicBool::new(true);
        let reader = BufReader::new(std::io::Cursor::new(b"{\"type\":\"ping\"}\n"));

        read_commands(reader, tx, &cancelled);

        assert!(rx.blocking_recv().is_none());
    }

    #[test]
    fn unsupported_runtime_diagnostics_version_reaches_correlated_dispatch() {
        let (tx, mut rx) = mpsc::channel(2);
        let cancelled = AtomicBool::new(false);
        let reader = BufReader::new(std::io::Cursor::new(
            b"{\"type\":\"get_runtime_diagnostics\",\"diagnostics_version\":2,\"request_id\":\"bad-version\"}\n{\"type\":\"ping\"}\n",
        ));

        read_commands(reader, tx, &cancelled);

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
