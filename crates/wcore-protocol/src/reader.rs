use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, BufReader};
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

/// Reads JSON Lines from stdin in a background task.
/// Returns a channel receiver for parsed commands.
///
/// Wave RA — `unbounded_channel` is intentional here. The producer side
/// is stdin from the host (Electron / CLI front-end); the rate is
/// human-input or host-script throughput, never a tight loop. Bounding
/// the channel could DROP a user command (e.g. an Approve / Cancel)
/// under transient consumer backpressure, which is materially worse
/// than the memory cost of one extra in-flight ProtocolCommand. The
/// documented exception is recorded inline.
pub fn spawn_stdin_reader() -> mpsc::UnboundedReceiver<ProtocolCommand> {
    let (tx, rx) = mpsc::unbounded_channel();

    tokio::spawn(async move {
        let stdin = tokio::io::stdin();
        let reader = BufReader::new(stdin);
        read_commands(reader, tx).await;
    });

    rx
}

/// Drive one capped line read per iteration, parse it, and forward parsed
/// commands to `tx`. Returns when the reader hits EOF, a read error, or the
/// receiver is dropped.
///
/// Generic over the reader so the byte-cap behavior is unit-testable without
/// touching the real stdin.
async fn read_commands<R: AsyncBufRead + Unpin>(
    mut reader: R,
    tx: mpsc::UnboundedSender<ProtocolCommand>,
) {
    // Capped line reader. `read_until` on a `take(MAX_LINE_BYTES)` limiter
    // stops at the byte cap even if no newline arrives, so an endless
    // newline-free stream can't grow the buffer unbounded. Overflow is
    // detected as "filled the cap without a terminating newline".
    let mut raw: Vec<u8> = Vec::new();
    loop {
        raw.clear();
        let read = match (&mut reader)
            .take(MAX_LINE_BYTES)
            .read_until(b'\n', &mut raw)
            .await
        {
            Ok(n) => n,
            Err(e) => {
                eprintln!("[protocol] stdin read error: {e}");
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
            eprintln!(
                "[protocol] Line exceeded {MAX_LINE_BYTES} byte cap — \
                 discarding oversized input and resuming"
            );
            if !discard_to_newline(&mut reader).await {
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
        match serde_json::from_str::<ProtocolCommand>(trimmed) {
            Ok(cmd) => {
                if tx.send(cmd).is_err() {
                    break;
                }
            }
            Err(e) => {
                // F-074: include the expected JSON shape in the
                // error message so developers debugging
                // integration issues can identify the problem
                // without reading protocol docs. Example of the
                // minimal required shape is shown in the hint.
                eprintln!(
                    "[protocol] Invalid command: {e} \
                     (expected JSON with a \"type\" field, e.g. \
                     {{\"type\":\"message\",\"msg_id\":\"1\",\"content\":\"hello\"}})"
                );
            }
        }
    }
}

/// Drain bytes from `reader` until (and including) the next newline, so the
/// remainder of an oversized line is consumed without buffering it. Reads in
/// bounded chunks via `fill_buf`/`consume` — never accumulates the discarded
/// bytes. Returns `false` on EOF or read error (caller should stop).
async fn discard_to_newline<R: AsyncBufRead + Unpin>(reader: &mut R) -> bool {
    loop {
        let buf = match reader.fill_buf().await {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[protocol] stdin read error while discarding: {e}");
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
    #[tokio::test]
    async fn oversized_line_is_skipped_then_next_line_parses() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        // One oversized newline-free run (cap + slack), a newline, then a
        // valid command. The oversized run must not be buffered whole.
        let oversized = vec![b'a'; MAX_LINE_BYTES as usize + 1024];
        let mut input = oversized;
        input.push(b'\n');
        input.extend_from_slice(br#"{"type":"ping"}"#);
        input.push(b'\n');

        let reader = BufReader::new(std::io::Cursor::new(input));
        read_commands(reader, tx).await;

        // Only the valid command comes through; the oversized line yields
        // no ProtocolCommand.
        let first = rx.recv().await;
        assert_eq!(first, Some(ProtocolCommand::Ping));
        assert!(rx.recv().await.is_none(), "no extra commands expected");
    }

    /// A normal line parses, and an oversized line in the middle of a stream
    /// does not corrupt the lines around it.
    #[tokio::test]
    async fn valid_line_before_and_after_oversized_line() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        let mut input = Vec::new();
        input.extend_from_slice(br#"{"type":"ping"}"#);
        input.push(b'\n');
        input.extend(std::iter::repeat_n(b'b', MAX_LINE_BYTES as usize + 1));
        input.push(b'\n');
        input.extend_from_slice(br#"{"type":"ping"}"#);
        input.push(b'\n');

        let reader = BufReader::new(std::io::Cursor::new(input));
        read_commands(reader, tx).await;

        assert_eq!(rx.recv().await, Some(ProtocolCommand::Ping));
        assert_eq!(rx.recv().await, Some(ProtocolCommand::Ping));
        assert!(rx.recv().await.is_none(), "only two valid pings expected");
    }

    /// A line exactly at the cap that IS newline-terminated is valid input,
    /// not an overflow — boundary check so we don't reject legitimate large
    /// (but bounded) commands.
    #[tokio::test]
    async fn line_at_cap_with_newline_is_not_treated_as_overflow() {
        let (tx, mut rx) = mpsc::unbounded_channel();

        // A valid command padded with trailing JSON whitespace up to just
        // under the cap, then a newline. `read_until` reads cap-or-fewer
        // bytes including the newline, so this stays within the limiter.
        let cmd = br#"{"type":"ping"}"#;
        let mut input = cmd.to_vec();
        let pad = MAX_LINE_BYTES as usize - cmd.len() - 1;
        input.extend(std::iter::repeat_n(b' ', pad));
        input.push(b'\n');

        let reader = BufReader::new(std::io::Cursor::new(input));
        read_commands(reader, tx).await;

        assert_eq!(rx.recv().await, Some(ProtocolCommand::Ping));
        assert!(rx.recv().await.is_none());
    }
}
