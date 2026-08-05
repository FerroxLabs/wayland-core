//! JSON-RPC 2.0 transport for the WhatsApp bridge subprocess.
//!
//! One JSON value per line over the child's stdin/stdout — the framing
//! `bridge.js` defines. This module owns the wire and nothing above it: the
//! backend enum, the readiness gates and the [`Channel`](wcore_channels::Channel)
//! implementation live in [`super`].
//!
//! Split out of `bridge.rs` to keep both files inside the project's
//! 1000-line-per-module limit.

use std::collections::{HashMap, VecDeque};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, oneshot, watch};

use serde::Deserialize;
use wcore_channels::ChannelError;
use wcore_channels::event::{ChannelEvent, ChatType, ConnectionState, IncomingMessage};

use super::{BridgeLaunch, BridgeUnavailable, PLATFORM, WhatsappBackend};

// ---------------------------------------------------------------------------

/// One line of the bridge's stdout. Responses carry `id`; notifications carry
/// `method` and no `id`.
#[derive(Debug, Deserialize)]
struct Frame {
    #[serde(default)]
    id: Option<serde_json::Value>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<serde_json::Value>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<RpcError>,
}

/// A JSON-RPC error object as the bridge emits it.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
struct RpcError {
    code: i64,
    #[serde(default)]
    message: String,
}

/// `inbound.message` notification params. Field names are identical across
/// both bridged backends (verified against `backends/baileys.js` and
/// `backends/whatsapp-web.js`); `senderRawJid` and `replyToMessageId` are
/// Baileys-only, hence optional.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InboundMessageParams {
    #[serde(default)]
    message_id: Option<String>,
    #[serde(default)]
    chat_id: Option<String>,
    #[serde(default)]
    sender_id: Option<String>,
    #[serde(default)]
    sender_raw_jid: Option<String>,
    #[serde(default)]
    sender_name: Option<String>,
    #[serde(default)]
    is_group: bool,
    #[serde(default)]
    from_me: bool,
    #[serde(default)]
    body: String,
    #[serde(default)]
    reply_to_message_id: Option<String>,
    #[serde(default)]
    timestamp: Option<f64>,
}

/// `connection.status` notification params.
///
/// The bridge also puts a `backend` field here, deliberately not decoded: the
/// authoritative backend read-back is the `health` handshake in
/// [`BridgeSession::over`], which happens once and REFUSES the session on a
/// mismatch. A second, advisory copy of the same fact arriving on every status
/// change would be a check nobody acts on.
#[derive(Debug, Clone, Deserialize)]
struct ConnectionStatusParams {
    #[serde(default)]
    state: Option<String>,
}

/// The `health` reply. The `backend` field is the whole point — it is what the
/// bridge says it loaded, not what we asked for.
#[derive(Debug, Clone, Deserialize, PartialEq)]
struct HealthReply {
    #[serde(default)]
    backend: Option<String>,
    #[serde(default)]
    connected: bool,
}

/// Failures of the bridge transport itself.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BridgeError {
    /// Preflight said the bridge cannot run.
    #[error(transparent)]
    Unavailable(#[from] BridgeUnavailable),
    /// The subprocess could not be spawned.
    #[error("spawn bridge: {0}")]
    Spawn(String),
    /// I/O against the subprocess failed.
    #[error("bridge io: {0}")]
    Io(String),
    /// The bridge replied with a JSON-RPC error.
    #[error("bridge rpc error {code}: {message}")]
    Rpc {
        /// JSON-RPC error code. `-32601` is the bridge's method allowlist.
        code: i64,
        /// Bridge-supplied message.
        message: String,
    },
    /// No reply within the configured timeout.
    #[error("bridge did not answer {method} within {secs}s")]
    Timeout {
        /// The RPC that went unanswered.
        method: String,
        /// The timeout that elapsed.
        secs: u64,
    },
    /// The bridge exited or closed stdout while a call was in flight.
    #[error("bridge closed the connection")]
    Closed,
    /// The bridge reported loading a different backend than we requested.
    ///
    /// Not a warning. `bridge.js` falls back to `baileys` on an unrecognised
    /// `--backend`, so this is the check that stops a config typo from putting
    /// a personal WhatsApp number on the wire.
    #[error(
        "bridge loaded backend {got:?} but {want:?} was requested — refusing the session; \
         check that bridge_path points at a bridge that understands --backend {want:?}"
    )]
    BackendMismatch {
        /// What we asked for.
        want: String,
        /// What the bridge said it loaded.
        got: String,
    },
}

impl From<BridgeError> for ChannelError {
    fn from(e: BridgeError) -> Self {
        match e {
            BridgeError::Unavailable(u) => ChannelError::Config(u.operator_message),
            BridgeError::Rpc { code, message } => {
                ChannelError::Rejected(format!("bridge rpc {code}: {message}"))
            }
            other => ChannelError::Transport(other.to_string()),
        }
    }
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<serde_json::Value, BridgeError>>>>>;
pub(super) type Inbox = Arc<Mutex<VecDeque<ChannelEvent>>>;

/// The half of a launched bridge the session talks over. Separated from the
/// spawn so tests can drive the exact same reader/writer over an in-memory
/// duplex without a Node process.
pub(super) struct Wire {
    stdin: Box<dyn AsyncWrite + Unpin + Send>,
    stdout: Box<dyn AsyncBufRead + Unpin + Send>,
    child: Option<tokio::process::Child>,
}

/// A live bridge subprocess plus its reader task.
pub(super) struct BridgeSession {
    stdin: Mutex<Box<dyn AsyncWrite + Unpin + Send>>,
    pending: Pending,
    next_id: AtomicU64,
    child: Mutex<Option<tokio::process::Child>>,
    shutdown: watch::Sender<bool>,
    reader: Mutex<Option<tokio::task::JoinHandle<()>>>,
    rpc_timeout: Duration,
}

impl BridgeSession {
    /// Spawn the bridge and complete the `health` handshake.
    pub(super) async fn open(
        launch: &BridgeLaunch,
        inbox: Inbox,
        handshake_timeout: Duration,
        rpc_timeout: Duration,
    ) -> Result<Self, BridgeError> {
        let wire = spawn_bridge(launch)?;
        Self::over(wire, inbox, launch.backend, handshake_timeout, rpc_timeout).await
    }

    /// Attach to an already-open wire, run the reader, and handshake.
    async fn over(
        wire: Wire,
        inbox: Inbox,
        want: WhatsappBackend,
        handshake_timeout: Duration,
        rpc_timeout: Duration,
    ) -> Result<Self, BridgeError> {
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        let (shutdown, shutdown_rx) = watch::channel(false);

        let reader = tokio::spawn(reader_loop(ReaderArgs {
            stdout: wire.stdout,
            inbox,
            pending: Arc::clone(&pending),
            shutdown: shutdown_rx,
        }));

        let session = Self {
            stdin: Mutex::new(wire.stdin),
            pending,
            next_id: AtomicU64::new(1),
            child: Mutex::new(wire.child),
            shutdown,
            reader: Mutex::new(Some(reader)),
            rpc_timeout,
        };

        // The handshake. `health` is the only RPC bridge.js answers before it
        // loads a backend, which makes it both a liveness probe and a dialect
        // check. Read the backend BACK — never infer it from what we passed.
        let raw = session
            .call_with_timeout("health", serde_json::json!({}), handshake_timeout)
            .await?;
        let health: HealthReply = serde_json::from_value(raw)
            .map_err(|e| BridgeError::Io(format!("malformed health reply: {e}")))?;

        let got = health.backend.unwrap_or_default();
        if got != want.wire_name() {
            // Reap before returning — a refused session must not leave a Node
            // process holding a WhatsApp socket open.
            session.close().await;
            return Err(BridgeError::BackendMismatch {
                want: want.wire_name().to_string(),
                got,
            });
        }

        Ok(session)
    }

    pub(super) async fn call(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, BridgeError> {
        self.call_with_timeout(method, params, self.rpc_timeout)
            .await
    }

    async fn call_with_timeout(
        &self,
        method: &str,
        params: serde_json::Value,
        timeout: Duration,
    ) -> Result<serde_json::Value, BridgeError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);

        let frame = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params,
        });
        // One JSON value per line — the bridge's framing.
        let mut line = serde_json::to_string(&frame)
            .map_err(|e| BridgeError::Io(format!("serialize {method}: {e}")))?;
        line.push('\n');

        {
            let mut stdin = self.stdin.lock().await;
            if let Err(e) = stdin.write_all(line.as_bytes()).await {
                self.pending.lock().await.remove(&id);
                return Err(BridgeError::Io(format!("write {method}: {e}")));
            }
            if let Err(e) = stdin.flush().await {
                self.pending.lock().await.remove(&id);
                return Err(BridgeError::Io(format!("flush {method}: {e}")));
            }
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(res)) => res,
            // Sender dropped: the reader loop exited (EOF / io error) without
            // draining us, or the map was cleared.
            Ok(Err(_)) => Err(BridgeError::Closed),
            Err(_) => {
                self.pending.lock().await.remove(&id);
                Err(BridgeError::Timeout {
                    method: method.to_string(),
                    secs: timeout.as_secs(),
                })
            }
        }
    }

    /// Stop the reader and reap the child. Idempotent.
    pub(super) async fn close(&self) {
        let _ = self.shutdown.send(true);
        if let Some(handle) = self.reader.lock().await.take() {
            handle.abort();
        }
        if let Some(mut child) = self.child.lock().await.take() {
            // `kill_on_drop` covers the panic path; this is the orderly one.
            let _ = child.kill().await;
        }
        // Nothing will answer these now.
        for (_, tx) in self.pending.lock().await.drain() {
            let _ = tx.send(Err(BridgeError::Closed));
        }
    }
}

/// Spawn `node <bridge.js> --backend <name> [--session <dir>]`.
///
/// Argv mode via [`wcore_config::shell::shell_command_argv`]: the backend name
/// and every path are separate argv entries, so no shell interprets them. The
/// backend name is a closed enum, but the session directory and bridge path are
/// operator-supplied and must never reach a shell string.
fn spawn_bridge(launch: &BridgeLaunch) -> Result<Wire, BridgeError> {
    let node = launch.node.to_str().ok_or_else(|| {
        BridgeError::Spawn(format!(
            "node path {} is not valid UTF-8",
            launch.node.display()
        ))
    })?;
    let script = launch.script.to_str().ok_or_else(|| {
        BridgeError::Spawn(format!(
            "bridge_path {} is not valid UTF-8",
            launch.script.display()
        ))
    })?;
    let session = match launch.session_dir.as_deref() {
        Some(d) => Some(d.to_str().ok_or_else(|| {
            BridgeError::Spawn(format!("session_dir {} is not valid UTF-8", d.display()))
        })?),
        None => None,
    };

    let mut args: Vec<&str> = vec![script, "--backend", launch.backend.wire_name()];
    if let Some(dir) = session {
        args.push("--session");
        args.push(dir);
    }

    let mut child = wcore_config::shell::shell_command_argv(node, &args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| BridgeError::Spawn(format!("{node} {script}: {e}")))?;

    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| BridgeError::Spawn("child stdin not captured".to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| BridgeError::Spawn("child stdout not captured".to_string()))?;

    // stderr is the bridge's human log channel, never RPC. Drain it or the
    // pipe fills and the child blocks.
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(target: "wcore_channel_whatsapp::bridge", stderr = %line);
            }
        });
    }

    Ok(Wire {
        stdin: Box::new(stdin),
        stdout: Box::new(BufReader::new(stdout)),
        child: Some(child),
    })
}

struct ReaderArgs {
    stdout: Box<dyn AsyncBufRead + Unpin + Send>,
    inbox: Inbox,
    pending: Pending,
    shutdown: watch::Receiver<bool>,
}

/// Read one line at a time; route responses to their caller and notifications
/// to the inbox. Exits on shutdown, EOF or I/O error, draining pending callers
/// so nobody waits on a process that is gone.
async fn reader_loop(mut args: ReaderArgs) {
    let mut buf = String::new();
    loop {
        buf.clear();
        tokio::select! {
            biased;
            _ = args.shutdown.changed() => {
                if *args.shutdown.borrow() {
                    break;
                }
            }
            res = args.stdout.read_line(&mut buf) => match res {
                Ok(0) => {
                    drain_pending(&args.pending, BridgeError::Closed).await;
                    break;
                }
                Ok(_) => {
                    let line = buf.trim();
                    if !line.is_empty() {
                        dispatch_line(line, &args.inbox, &args.pending).await;
                    }
                }
                Err(e) => {
                    drain_pending(&args.pending, BridgeError::Io(e.to_string())).await;
                    break;
                }
            },
        }
    }
}

async fn drain_pending(pending: &Pending, err: BridgeError) {
    for (_, tx) in pending.lock().await.drain() {
        let _ = tx.send(Err(err.clone()));
    }
}

async fn dispatch_line(line: &str, inbox: &Inbox, pending: &Pending) {
    let frame: Frame = match serde_json::from_str(line) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(
                target: "wcore_channel_whatsapp::bridge",
                error = %e,
                "skipping malformed line from bridge"
            );
            return;
        }
    };

    // Response path.
    if let Some(id) = frame.id.as_ref().and_then(serde_json::Value::as_u64)
        && let Some(tx) = pending.lock().await.remove(&id)
    {
        let payload = match frame.error {
            Some(err) => Err(BridgeError::Rpc {
                code: err.code,
                message: err.message,
            }),
            None => Ok(frame.result.unwrap_or(serde_json::Value::Null)),
        };
        let _ = tx.send(payload);
        return;
    }

    let Some(method) = frame.method.as_deref() else {
        return;
    };
    let params = frame.params.unwrap_or(serde_json::Value::Null);

    match method {
        "inbound.message" => match serde_json::from_value::<InboundMessageParams>(params) {
            Ok(p) => {
                if let Some(msg) = build_incoming(p) {
                    let mut guard = inbox.lock().await;
                    wcore_channels::push_bounded(&mut guard, ChannelEvent::MessageReceived { msg });
                }
            }
            Err(e) => tracing::warn!(
                target: "wcore_channel_whatsapp::bridge",
                error = %e,
                "malformed inbound.message params"
            ),
        },
        "connection.status" => {
            let p: ConnectionStatusParams =
                serde_json::from_value(params).unwrap_or(ConnectionStatusParams { state: None });
            let state = p.state.unwrap_or_default();
            let mut guard = inbox.lock().await;
            match state.as_str() {
                "connected" => wcore_channels::push_bounded(
                    &mut guard,
                    ChannelEvent::ConnectionStateChanged {
                        state: ConnectionState::Connected,
                    },
                ),
                "connecting" | "starting" => wcore_channels::push_bounded(
                    &mut guard,
                    ChannelEvent::ConnectionStateChanged {
                        state: ConnectionState::Connecting,
                    },
                ),
                "disconnected" => wcore_channels::push_bounded(
                    &mut guard,
                    ChannelEvent::ConnectionStateChanged {
                        state: ConnectionState::Disconnected,
                    },
                ),
                // `logged_out` is not a transport state — the operator must
                // re-pair. Surface it as a warning as well as a disconnect so
                // it is not mistaken for a retryable drop.
                "logged_out" => {
                    wcore_channels::push_bounded(
                        &mut guard,
                        ChannelEvent::PlatformWarning {
                            message: "whatsapp bridge reports logged_out — the pairing was \
                                      revoked; re-pair by scanning a new QR code"
                                .to_string(),
                        },
                    );
                    wcore_channels::push_bounded(
                        &mut guard,
                        ChannelEvent::ConnectionStateChanged {
                            state: ConnectionState::Disconnected,
                        },
                    );
                }
                other => tracing::debug!(
                    target: "wcore_channel_whatsapp::bridge",
                    state = %other,
                    "unhandled connection.status state"
                ),
            }
        }
        // QR pairing is a human step. Surfacing it as a warning is what makes
        // it visible at all in a headless Core — the alternative is a channel
        // that sits in `connecting` forever with no explanation.
        "qr.update" => {
            let mut guard = inbox.lock().await;
            wcore_channels::push_bounded(
                &mut guard,
                ChannelEvent::PlatformWarning {
                    message: "whatsapp bridge requires QR pairing — scan the code the bridge \
                              printed on its stderr with WhatsApp > Linked devices"
                        .to_string(),
                },
            );
        }
        "error" => {
            let kind = params
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown");
            let message = params
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let mut guard = inbox.lock().await;
            wcore_channels::push_bounded(
                &mut guard,
                ChannelEvent::PlatformWarning {
                    message: format!("whatsapp bridge error [{kind}]: {message}"),
                },
            );
        }
        other => tracing::trace!(
            target: "wcore_channel_whatsapp::bridge",
            method = %other,
            "ignoring unhandled bridge notification"
        ),
    }
}

/// Build an [`IncomingMessage`] from an `inbound.message` notification.
///
/// Returns `None` for our own echoes (`fromMe`) and for bodies with nothing in
/// them, so a receipt does not surface as a message.
fn build_incoming(p: InboundMessageParams) -> Option<IncomingMessage> {
    if p.from_me {
        return None;
    }
    let body = p.body;
    if body.is_empty() {
        return None;
    }

    let conversation_id = p.chat_id.unwrap_or_default();
    let sender_id = p.sender_id.unwrap_or_default();
    let ts_secs = p.timestamp.unwrap_or(0.0) as i64;

    Some(IncomingMessage {
        id: p.message_id.unwrap_or_default(),
        conversation_id,
        // `senderId` is normalized to bare digits by the bridge; the full JID
        // is what an outbound `sendText({chatId})` needs, so it rides in
        // `sender_alt_id` rather than being discarded.
        author: sender_id.clone(),
        text: body,
        ts_secs,
        // The bridge writes media to a local path (`mediaPath`) rather than
        // exposing a URL. `Channel::fetch_media` has no local-path contract
        // here, so media is deliberately NOT claimed — see `native_actions`.
        attachments: Vec::new(),
        sender_id,
        sender_display: p.sender_name,
        sender_handle: None,
        sender_alt_id: p.sender_raw_jid,
        is_bot: false,
        is_self: false,
        chat_type: if p.is_group {
            ChatType::Group
        } else {
            ChatType::Direct
        },
        chat_name: None,
        space_id: None,
        thread_id: None,
        parent_chat_id: None,
        account_id: None,
        platform: Some(PLATFORM.to_string()),
        was_mentioned: false,
        mention_kind: None,
        reply_to_message_id: p.reply_to_message_id,
        reply_to_text: None,
    })
}

#[cfg(test)]
pub(crate) mod testing {
    use super::*;

    /// Stand up a scripted peer that answers `health` with `backend`, then
    /// echoes a fixed reply to everything else.
    pub(crate) async fn scripted_peer(
        report_backend: &'static str,
    ) -> (Wire, tokio::task::JoinHandle<Vec<String>>) {
        let (ours, theirs) = tokio::io::duplex(64 * 1024);
        let (their_read, mut their_write) = tokio::io::split(theirs);
        let (our_read, our_write) = tokio::io::split(ours);

        let peer = tokio::spawn(async move {
            let mut seen = Vec::new();
            let mut lines = BufReader::new(their_read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                seen.push(line.clone());
                let v: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                let id = v.get("id").cloned().unwrap_or(serde_json::Value::Null);
                let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("");
                let result = if method == "health" {
                    serde_json::json!({"backend": report_backend, "connected": false})
                } else {
                    serde_json::json!({"ok": true, "messageId": "wamid.FAKE"})
                };
                let reply =
                    serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}).to_string();
                if their_write
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
            }
            seen
        });

        (
            Wire {
                stdin: Box::new(our_write),
                stdout: Box::new(BufReader::new(our_read)),
                child: None,
            },
            peer,
        )
    }

    pub(crate) async fn test_session(
        backend: WhatsappBackend,
    ) -> (Arc<BridgeSession>, tokio::task::JoinHandle<Vec<String>>) {
        let (wire, peer) = scripted_peer(backend.wire_name()).await;
        let inbox: Inbox = Arc::new(Mutex::new(VecDeque::new()));
        let session = BridgeSession::over(
            wire,
            inbox,
            backend,
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .await
        .expect("handshake must succeed when the peer reports the requested backend");
        (Arc::new(session), peer)
    }
}

#[cfg(test)]
mod tests {
    use super::testing::*;
    use super::*;

    #[tokio::test]
    async fn handshake_succeeds_when_the_bridge_reports_the_requested_backend() {
        // CAN PASS.
        let (session, _peer) = test_session(WhatsappBackend::Baileys).await;
        let reply = session
            .call("sendText", serde_json::json!({"chatId": "x", "text": "y"}))
            .await
            .expect("a handshaken session must carry calls");
        assert_eq!(
            reply.get("messageId").and_then(|v| v.as_str()),
            Some("wamid.FAKE")
        );
        session.close().await;
    }

    #[tokio::test]
    async fn handshake_refuses_a_session_whose_backend_does_not_match() {
        // CAN FAIL — and this is the check that stops bridge.js's own
        // fall-back-to-baileys from silently applying to a typo'd config.
        let (wire, _peer) = scripted_peer("baileys").await;
        let inbox: Inbox = Arc::new(Mutex::new(VecDeque::new()));
        // `BridgeSession` owns a child process handle and is deliberately not
        // `Debug`, so unwrap the Result by hand rather than via `expect_err`.
        let err = match BridgeSession::over(
            wire,
            inbox,
            WhatsappBackend::WhatsappWeb,
            Duration::from_secs(5),
            Duration::from_secs(5),
        )
        .await
        {
            Ok(session) => {
                session.close().await;
                panic!("a mismatched backend must be refused, but a session was opened");
            }
            Err(e) => e,
        };

        match err {
            BridgeError::BackendMismatch { want, got } => {
                assert_eq!(want, "whatsapp-web");
                assert_eq!(got, "baileys");
            }
            other => panic!("expected BackendMismatch, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_handshake_actually_puts_health_on_the_wire() {
        // Guards against a handshake that "passes" without sending anything —
        // an actor that never spoke is a dead instrument.
        let (session, peer) = test_session(WhatsappBackend::Baileys).await;
        session.close().await;
        drop(session);
        let seen = peer.await.unwrap();
        assert!(
            seen.iter().any(|l| l.contains("\"method\":\"health\"")),
            "no health frame reached the peer; frames seen: {seen:?}"
        );
    }

    // -- notification decoding ----------------------------------------------

    async fn dispatch_for_test(line: &str) -> Vec<ChannelEvent> {
        let inbox: Inbox = Arc::new(Mutex::new(VecDeque::new()));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));
        dispatch_line(line, &inbox, &pending).await;
        let mut guard = inbox.lock().await;
        guard.drain(..).collect()
    }

    #[tokio::test]
    async fn inbound_message_becomes_a_message_event() {
        let line = r#"{"jsonrpc":"2.0","method":"inbound.message","params":{"messageId":"3EB0","chatId":"1555@s.whatsapp.net","senderId":"1555","senderRawJid":"1555@s.whatsapp.net","senderName":"Ada","isGroup":false,"fromMe":false,"body":"hello","timestamp":1700000000}}"#;
        let evs = dispatch_for_test(line).await;
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            ChannelEvent::MessageReceived { msg } => {
                assert_eq!(msg.text, "hello");
                assert_eq!(msg.id, "3EB0");
                assert_eq!(msg.conversation_id, "1555@s.whatsapp.net");
                assert_eq!(msg.sender_display.as_deref(), Some("Ada"));
                // The full JID must survive — an agent echoing senderId back
                // into sendText needs the host part or Baileys rejects it.
                assert_eq!(msg.sender_alt_id.as_deref(), Some("1555@s.whatsapp.net"));
                assert_eq!(msg.chat_type, ChatType::Direct);
            }
            other => panic!("expected MessageReceived, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn our_own_echo_is_not_surfaced_as_an_inbound_message() {
        let line = r#"{"jsonrpc":"2.0","method":"inbound.message","params":{"messageId":"3EB1","chatId":"1555@s.whatsapp.net","senderId":"1555","fromMe":true,"body":"hello","timestamp":1700000000}}"#;
        assert!(dispatch_for_test(line).await.is_empty());
    }

    #[tokio::test]
    async fn qr_update_surfaces_a_pairing_instruction() {
        // Without this a headless Core sits in `connecting` forever with no
        // explanation of the human step it is waiting on.
        let line = r#"{"jsonrpc":"2.0","method":"qr.update","params":{"qr":"2@abc"}}"#;
        let evs = dispatch_for_test(line).await;
        assert_eq!(evs.len(), 1);
        match &evs[0] {
            ChannelEvent::PlatformWarning { message } => {
                assert!(message.contains("QR"), "got {message}");
                // The QR payload itself is pairing material — it must not be
                // echoed into an event stream that gets logged.
                assert!(!message.contains("2@abc"), "QR payload leaked: {message}");
            }
            other => panic!("expected PlatformWarning, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn logged_out_is_reported_as_a_warning_and_a_disconnect() {
        let line = r#"{"jsonrpc":"2.0","method":"connection.status","params":{"state":"logged_out","backend":"baileys"}}"#;
        let evs = dispatch_for_test(line).await;
        assert_eq!(evs.len(), 2, "logged_out must not read as a plain drop");
        assert!(matches!(evs[0], ChannelEvent::PlatformWarning { .. }));
        assert!(matches!(
            evs[1],
            ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Disconnected
            }
        ));
    }

    #[tokio::test]
    async fn connected_status_promotes_the_connection_state() {
        let line = r#"{"jsonrpc":"2.0","method":"connection.status","params":{"state":"connected","backend":"baileys"}}"#;
        let evs = dispatch_for_test(line).await;
        assert!(matches!(
            evs.as_slice(),
            [ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Connected
            }]
        ));
    }

    #[tokio::test]
    async fn a_malformed_line_is_skipped_without_taking_the_reader_down() {
        assert!(dispatch_for_test("not json at all").await.is_empty());
        assert!(dispatch_for_test("{}").await.is_empty());
    }
}
