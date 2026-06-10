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

use wcore_channels::event::{ChannelEvent, ChatType, IncomingMessage};

use crate::error::SignalError;
use crate::jsonrpc::{Frame, ReceiveParams};

/// Shared map of in-flight JSON-RPC request id → response sender.
/// Aliased to keep callsites readable (clippy::type_complexity).
pub type PendingResponses =
    Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, SignalError>>>>>;

/// Shared, swappable stdin writer. `send_message` reads the current
/// writer from this slot; the supervisor swaps the inner writer on each
/// (re)spawn so sends always target the live `signal-cli` process. The
/// `Option` is `None` between a process death and the next respawn.
pub type SharedStdin = Arc<Mutex<Option<Box<dyn AsyncWrite + Unpin + Send>>>>;

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

    // sender_id: ACI/UUID is the stable Signal identity. Fall back to
    // phone number if no UUID is present (older clients / linked devices
    // may omit it), then to source_name as last resort.
    let sender_id = envelope
        .source_uuid
        .clone()
        .or_else(|| envelope.source.clone())
        .or_else(|| envelope.source_name.clone())
        .unwrap_or_default();

    // author: stable address label — phone, then UUID, then display name
    // as a last resort. The display name lives in `sender_display`.
    let author = envelope
        .source
        .clone()
        .or_else(|| envelope.source_uuid.clone())
        .or_else(|| envelope.source_name.clone())
        .unwrap_or_default();

    // sender_display: source_name when present.
    let sender_display = envelope.source_name.clone();

    // sender_handle: e164 phone number.
    let sender_handle = envelope.source.clone();

    // sender_alt_id: the OTHER half of the UUID/number union. When UUID
    // is the primary id (sender_id), put the phone number here; when UUID
    // is absent and phone is primary, there is no alt.
    let sender_alt_id = if envelope.source_uuid.is_some() {
        envelope.source.clone()
    } else {
        None
    };

    // chat_type: presence of groupInfo in the data message is the
    // definitive Signal indicator for a group context. Signal does not
    // have broadcast channels in the daemon JSON-RPC surface, so all
    // non-group messages are 1:1 Direct.
    let chat_type = if data.group_info.is_some() {
        ChatType::Group
    } else {
        ChatType::Direct
    };

    // account_id: the receiving Signal account number, if signal-cli
    // reported it in the outer params envelope.
    let account_id = parsed.account.clone();

    Some(IncomingMessage {
        id,
        conversation_id,
        author,
        text,
        ts_secs,
        // attachments: signal-cli's JSON-RPC `receive` notification does
        // not surface attachment references in the current ReceiveParams
        // schema; leave empty until the struct gains an attachments field.
        attachments: Vec::new(),
        sender_id,
        sender_display,
        sender_handle,
        sender_alt_id,
        // is_bot / is_self: signal-cli gives no bot flag and no self-send
        // indicator in the receive notification path.
        is_bot: false,
        is_self: false,
        chat_type,
        // chat_name: GroupInfo only carries a base64 group id, not a
        // human-readable name; leave None until a name-resolution layer
        // is added.
        chat_name: None,
        // space_id / thread_id / parent_chat_id: Signal has no workspace
        // or thread nesting concept exposed via JSON-RPC.
        space_id: None,
        thread_id: None,
        parent_chat_id: None,
        account_id,
        platform: Some("signal".into()),
        // was_mentioned / mention_kind: ReceiveParams carries no mentions
        // array; mention detection is deferred to a higher layer.
        was_mentioned: false,
        mention_kind: None,
        // reply_to_message_id / reply_to_text: DataMessage carries no
        // quote field in the current schema.
        reply_to_message_id: None,
        reply_to_text: None,
    })
}
