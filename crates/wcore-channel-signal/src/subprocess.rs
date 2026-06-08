//! Subprocess plumbing: launcher trait + real `tokio::process::Command`
//! impl + the stdout reader task that demuxes JSON-RPC frames into
//! pending-request responses and inbox notifications.
//!
//! The launcher trait exists so tests can substitute `tokio::io::duplex`
//! for a real signal-cli process — tests never need the binary
//! installed.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, BufReader};
use tokio::process::Command;
use tokio::sync::{Mutex, oneshot, watch};

use wcore_channels::event::{ChannelEvent, IncomingMessage};

use crate::error::SignalError;
use crate::jsonrpc::{Frame, ReceiveParams};

/// Shared map of in-flight JSON-RPC request id → response sender.
/// Aliased to keep callsites readable (clippy::type_complexity).
pub type PendingResponses =
    Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, SignalError>>>>>;

/// Handle returned by a [`SignalProcessLauncher`]. Carries the
/// half-duplex stdio + (optional) a child handle to kill on `stop()`.
pub struct SignalProcessHandle {
    pub stdin: Box<dyn AsyncWrite + Unpin + Send>,
    pub stdout: Box<dyn AsyncBufRead + Unpin + Send>,
    /// Real launcher returns Some; test launcher returns None.
    pub child: Option<tokio::process::Child>,
}

/// Swappable behind a trait so tests fabricate stdio with
/// `tokio::io::duplex` instead of spawning a real process.
pub trait SignalProcessLauncher: Send + Sync {
    fn launch(&self, cli_path: &Path, account: &str) -> Result<SignalProcessHandle, SignalError>;
}

/// Real launcher — spawns `signal-cli -a <account> jsonRpc`.
pub struct RealLauncher;

impl SignalProcessLauncher for RealLauncher {
    fn launch(&self, cli_path: &Path, account: &str) -> Result<SignalProcessHandle, SignalError> {
        let mut child = Command::new(cli_path)
            .arg("-a")
            .arg(account)
            .arg("jsonRpc")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| SignalError::Spawn(format!("{e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SignalError::Spawn("child stdin not captured".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SignalError::Spawn("child stdout not captured".into()))?;

        // Drain stderr in the background so signal-cli doesn't block.
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                use tokio::io::AsyncBufReadExt;
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(target: "wcore_channel_signal", stderr = %line);
                }
            });
        }

        Ok(SignalProcessHandle {
            stdin: Box::new(stdin),
            stdout: Box::new(BufReader::new(stdout)),
            child: Some(child),
        })
    }
}

/// Arguments to the reader task.
pub struct ReaderArgs {
    pub stdout: Box<dyn AsyncBufRead + Unpin + Send>,
    pub inbox: Arc<Mutex<VecDeque<ChannelEvent>>>,
    pub pending: PendingResponses,
    pub shutdown: watch::Receiver<bool>,
}

/// The reader task: read one line at a time, parse as JSON-RPC,
/// route to pending request or push as inbox event. Exits when
/// `shutdown` flips to true or stdout hits EOF.
pub async fn reader_loop(mut args: ReaderArgs) {
    let mut buf = String::new();
    loop {
        buf.clear();
        tokio::select! {
            biased;
            _ = args.shutdown.changed() => {
                if *args.shutdown.borrow() {
                    tracing::debug!(target: "wcore_channel_signal", "reader: shutdown signalled");
                    break;
                }
            }
            res = args.stdout.read_line(&mut buf) => {
                match res {
                    Ok(0) => {
                        tracing::debug!(target: "wcore_channel_signal", "reader: stdout EOF");
                        // Drain pending with SubprocessClosed so callers
                        // don't hang forever.
                        let mut pending = args.pending.lock().await;
                        for (_, tx) in pending.drain() {
                            let _ = tx.send(Err(SignalError::SubprocessClosed));
                        }
                        break;
                    }
                    Ok(_) => {
                        let trimmed = buf.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        dispatch_line(trimmed, &args.inbox, &args.pending).await;
                    }
                    Err(e) => {
                        tracing::warn!(
                            target: "wcore_channel_signal",
                            error = %e,
                            "reader: io error reading stdout"
                        );
                        let mut pending = args.pending.lock().await;
                        for (_, tx) in pending.drain() {
                            let _ = tx.send(Err(SignalError::Io(format!("{e}"))));
                        }
                        break;
                    }
                }
            }
        }
    }
}

async fn dispatch_line(
    line: &str,
    inbox: &Arc<Mutex<VecDeque<ChannelEvent>>>,
    pending: &PendingResponses,
) {
    let frame: Frame = match serde_json::from_str(line) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                target: "wcore_channel_signal",
                line = %line,
                error = %e,
                "reader: skipping malformed JSON line"
            );
            return;
        }
    };

    // Response path: id present + (result or error). Match id-as-u64.
    if let Some(id_val) = frame.id.as_ref()
        && let Some(id) = id_val.as_u64()
    {
        let mut pending_guard = pending.lock().await;
        if let Some(tx) = pending_guard.remove(&id) {
            let payload = if let Some(err) = frame.error {
                Err(SignalError::Rpc {
                    code: err.code,
                    message: err.message,
                })
            } else {
                Ok(frame.result.unwrap_or(serde_json::Value::Null))
            };
            let _ = tx.send(payload);
            return;
        }
    }

    // Notification path: method = "receive" → IncomingMessage.
    if let Some(method) = frame.method.as_deref() {
        if method == "receive" {
            let params = match frame.params {
                Some(p) => p,
                None => {
                    tracing::debug!(target: "wcore_channel_signal", "receive notification without params");
                    return;
                }
            };
            match serde_json::from_value::<ReceiveParams>(params) {
                Ok(parsed) => {
                    if let Some(msg) = build_incoming(&parsed) {
                        inbox
                            .lock()
                            .await
                            .push_back(ChannelEvent::MessageReceived { msg });
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        target: "wcore_channel_signal",
                        error = %e,
                        "reader: malformed `receive` params"
                    );
                }
            }
        } else {
            tracing::trace!(
                target: "wcore_channel_signal",
                method = %method,
                "reader: ignoring unhandled notification"
            );
        }
    }
}

/// Build an `IncomingMessage` from a parsed `receive` envelope.
/// Returns `None` for envelopes that don't carry a data message
/// (sync / receipt / typing events), so they're silently dropped.
fn build_incoming(parsed: &ReceiveParams) -> Option<IncomingMessage> {
    let envelope = &parsed.envelope;
    let data = envelope.data_message.as_ref()?;
    let text = data.message.clone().unwrap_or_default();
    if text.is_empty() && data.group_info.is_none() {
        // Empty receipt-style envelope — nothing useful to surface.
        return None;
    }

    // Prefer envelope.timestamp; fall back to dataMessage.timestamp.
    let ts_ms = envelope.timestamp.or(data.timestamp).unwrap_or(0);
    let ts_secs = ts_ms / 1000;
    let id = format!("{ts_ms}");

    // conversation_id: group id when present, otherwise the sender's
    // address (1:1 DMs are keyed by source).
    let conversation_id = data
        .group_info
        .as_ref()
        .and_then(|g| g.group_id.clone())
        .or_else(|| envelope.source.clone())
        .or_else(|| envelope.source_uuid.clone())
        .unwrap_or_default();

    let author = envelope
        .source
        .clone()
        .or_else(|| envelope.source_uuid.clone())
        .or_else(|| envelope.source_name.clone())
        .unwrap_or_default();

    Some(IncomingMessage {
        id,
        conversation_id,
        author,
        text,
        ts_secs,
        attachments: Vec::new(),
    })
}
