use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::commands::ProtocolCommand;

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
        let mut reader = BufReader::new(stdin);
        let mut line = String::new();

        loop {
            line.clear();
            match reader.read_line(&mut line).await {
                Ok(0) => break, // EOF - client closed stdin
                Ok(_) => {
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
                Err(e) => {
                    eprintln!("[protocol] stdin read error: {e}");
                    break;
                }
            }
        }
    });

    rx
}
