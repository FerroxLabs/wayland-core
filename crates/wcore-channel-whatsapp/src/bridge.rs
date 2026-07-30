//! `bridge` — OPT-IN client for the Wayland Desktop WhatsApp bridge.
//!
//! The adapter in [`crate`] speaks the Meta Business Cloud API and nothing
//! else. Wayland Desktop additionally drives two unofficial WhatsApp Web
//! backends through a Node subprocess that speaks JSON-RPC 2.0 over stdio.
//! This module lets Core **talk to that subprocess**. It does not reimplement
//! the WhatsApp Web protocol, and it must not: Baileys is a reverse-engineered
//! client including a Signal-protocol E2E stack.
//!
//! # `meta-business` remains the default, and Node is never required
//!
//! [`WhatsappBackend`] defaults to [`WhatsappBackend::MetaBusiness`], which
//! routes to the Cloud API adapter and touches none of this module. A Core
//! install with no Node, no bridge and no `backend` key configured behaves
//! exactly as it did before this module existed. Selecting `baileys` or
//! `whatsapp-web` is an explicit operator act.
//!
//! # Distribution: the operator supplies the bridge
//!
//! Core does not vendor, download or install the bridge. [`WhatsappBridgeConfig::bridge_path`]
//! names an existing `bridge.js` on disk. Three reasons, in order of weight:
//!
//! 1. **A single-binary install must keep working.** Vendoring the bridge's
//!    dependency tree (122 MB at the time of writing) into a Rust release, or
//!    running a package install on first use, makes a Node toolchain a de facto
//!    build/runtime dependency of every Core install — including the ones that
//!    will never send a WhatsApp message.
//! 2. **Fetching executable code at runtime is a supply-chain surface** Core
//!    does not otherwise have, and would be the worst option on that axis.
//! 3. **Redistribution carries obligations that not-redistributing does not.**
//!    The bridge's own sources carry attribution the Desktop project wrote for
//!    upstream MIT work, and one backend wraps an Apache-2.0 library. Executing
//!    an operator's own copy is not redistribution. See `docs/whatsapp-bridge.md`.
//!
//! # Fail closed, and never advertise what is not reachable
//!
//! [`preflight`] answers "can this actually run" from the filesystem, and
//! [`Channel::probe`] reports its verdict through
//! [`ProbeOutcome`](wcore_channels::probe::ProbeOutcome), whose
//! `is_ready()` is true only for `Ok`. A missing Node runtime or a missing
//! bridge script yields `Incomplete` naming exactly what is absent — never a
//! green, and never a `Connected` state.
//!
//! # The backend is read back from the bridge, not assumed
//!
//! `bridge.js` defaults to `baileys` when `--backend` is absent or unparsable.
//! An operator who typo'd a backend name must not silently have their personal
//! WhatsApp number put on the wire. So [`BridgeSession::open`] performs a
//! `health` handshake — the one RPC `bridge.js` answers before loading any
//! backend — and **refuses the session** unless the backend the bridge reports
//! is the backend we asked for. This also detects a bridge too old or too new
//! to speak this dialect, which a path-based config cannot otherwise know.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex, oneshot, watch};

use wcore_channels::event::{
    ChannelEvent, ChatType, ConnectionState, IncomingMessage, MessageReceipt,
};
use wcore_channels::outgoing::OutgoingMessage;
use wcore_channels::probe::ProbeReport;
use wcore_channels::{Channel, ChannelError};

/// Platform tag shared with the Cloud API adapter — the backend differs, the
/// platform does not.
pub const PLATFORM: &str = "whatsapp";

/// Default seconds to wait for the `health` handshake before giving up on a
/// spawned bridge. Generous: Node cold start on a loaded host is not fast.
pub const DEFAULT_HANDSHAKE_TIMEOUT_SECS: u64 = 20;

/// Default seconds to wait for any other RPC reply.
pub const DEFAULT_RPC_TIMEOUT_SECS: u64 = 60;

// ---------------------------------------------------------------------------
// Backend selection
// ---------------------------------------------------------------------------

/// Which WhatsApp backend a channel instance drives.
///
/// The default is [`Self::MetaBusiness`] — the official Cloud API — and that is
/// deliberate, not incidental. The two bridged variants drive unofficial
/// reverse-engineered clients against a personal WhatsApp account; see
/// [`Self::is_unofficial`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhatsappBackend {
    /// Official Meta WhatsApp Business Cloud API. Handled entirely by
    /// [`crate::WhatsappChannel`]; no subprocess, no Node.
    #[default]
    MetaBusiness,
    /// WhatsApp Web protocol via `@whiskeysockets/baileys`, through the bridge.
    Baileys,
    /// WhatsApp Web protocol via `whatsapp-web.js` + Chromium, through the bridge.
    WhatsappWeb,
}

impl WhatsappBackend {
    /// The exact string `bridge.js --backend` expects, and the exact string
    /// its `health` reply reports back.
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::MetaBusiness => "meta-business",
            Self::Baileys => "baileys",
            Self::WhatsappWeb => "whatsapp-web",
        }
    }

    /// Whether this backend is driven through the Node bridge subprocess.
    ///
    /// `meta-business` is reachable through the bridge too, but Core already
    /// speaks the Cloud API natively over HTTPS, so routing it through a Node
    /// subprocess would add a dependency and buy nothing.
    pub const fn is_bridged(self) -> bool {
        matches!(self, Self::Baileys | Self::WhatsappWeb)
    }

    /// Whether this backend is an unofficial client of WhatsApp.
    ///
    /// Both bridged backends drive the WhatsApp Web protocol from a personal
    /// account. That is not a supported use of WhatsApp, and Meta bans accounts
    /// for it — see `docs/whatsapp-bridge.md`. Callers surface this; it is not
    /// this type's job to decide policy.
    pub const fn is_unofficial(self) -> bool {
        matches!(self, Self::Baileys | Self::WhatsappWeb)
    }

    /// The npm package this backend needs installed beside the bridge.
    ///
    /// `None` for `meta-business`, which the bridge serves with `axios` alone.
    /// Measured from `whatsapp-bridge/package.json`.
    pub const fn npm_package(self) -> Option<&'static str> {
        match self {
            Self::MetaBusiness => None,
            Self::Baileys => Some("@whiskeysockets/baileys"),
            Self::WhatsappWeb => Some("whatsapp-web.js"),
        }
    }

    /// Subdirectory of the bridge's session root this backend stores pairing
    /// material under, and the entry inside it that exists once paired.
    ///
    /// Measured from `backends/baileys.js` (`<session>/baileys/creds.json`, via
    /// `useMultiFileAuthState`) and `backends/whatsapp-web.js`
    /// (`<session>/whatsapp-web/session-wayland`, via `LocalAuth` with
    /// `clientId: 'wayland'`).
    const fn pairing_marker(self) -> Option<(&'static str, &'static str)> {
        match self {
            Self::MetaBusiness => None,
            Self::Baileys => Some(("baileys", "creds.json")),
            Self::WhatsappWeb => Some(("whatsapp-web", "session-wayland")),
        }
    }

    /// Every accepted wire name, for error messages and operator help.
    pub const ALL_WIRE_NAMES: [&'static str; 3] = ["meta-business", "baileys", "whatsapp-web"];
}

/// A `backend` value that names no known backend.
///
/// Returned rather than defaulted, and the reason is measured rather than
/// assumed. Driving the real `bridge.js` shows it treats an unrecognised
/// `--backend` value verbatim — `health` echoes `baileyz` back — and only
/// fails later, at backend load, with `-32000 Unknown backend: baileyz`. What
/// it DOES silently default to `baileys` is an **absent or valueless**
/// `--backend` flag. Either way a typo must be caught here, at parse time,
/// rather than after a Node process is holding a WhatsApp socket.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unknown whatsapp backend {got:?} — expected one of: {}", WhatsappBackend::ALL_WIRE_NAMES.join(", "))]
pub struct UnknownBackend {
    /// The rejected value, echoed so the operator can see their typo.
    pub got: String,
}

impl std::str::FromStr for WhatsappBackend {
    type Err = UnknownBackend;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "meta-business" => Ok(Self::MetaBusiness),
            "baileys" => Ok(Self::Baileys),
            "whatsapp-web" => Ok(Self::WhatsappWeb),
            other => Err(UnknownBackend {
                got: other.to_string(),
            }),
        }
    }
}

impl std::fmt::Display for WhatsappBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.wire_name())
    }
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// TOML schema for one bridged WhatsApp channel instance.
///
/// Carries no secrets: Baileys and whatsapp-web.js authenticate by QR pairing
/// and persist their own session material under [`Self::session_dir`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WhatsappBridgeConfig {
    /// Which bridged backend to drive. Must be `baileys` or `whatsapp-web`;
    /// `meta-business` is served by [`crate::WhatsappChannel`] instead.
    pub backend: WhatsappBackend,

    /// Absolute path to the operator's `bridge.js`. Core does not ship one.
    pub bridge_path: PathBuf,

    /// Explicit Node interpreter. When absent, `node` is resolved from `PATH`.
    #[serde(default)]
    pub node_path: Option<PathBuf>,

    /// Directory the bridge persists pairing/session state under. When absent,
    /// the bridge picks its own default beneath the user's home directory.
    #[serde(default)]
    pub session_dir: Option<PathBuf>,

    /// Human-readable label, logs only.
    #[serde(default)]
    pub workspace_name: String,

    /// Fallback chat id when an `OutgoingMessage` arrives with none.
    #[serde(default)]
    pub default_recipient: String,

    /// Seconds to wait for the `health` handshake.
    #[serde(default = "default_handshake_timeout_secs")]
    pub handshake_timeout_secs: u64,

    /// Seconds to wait for any other RPC reply.
    #[serde(default = "default_rpc_timeout_secs")]
    pub rpc_timeout_secs: u64,
}

fn default_handshake_timeout_secs() -> u64 {
    DEFAULT_HANDSHAKE_TIMEOUT_SECS
}

fn default_rpc_timeout_secs() -> u64 {
    DEFAULT_RPC_TIMEOUT_SECS
}

// ---------------------------------------------------------------------------
// Preflight
// ---------------------------------------------------------------------------

/// Why the bridge cannot be launched, in a form an operator can act on.
///
/// Every finding names an ITEM, never a value — this feeds
/// [`ProbeReport::findings`], which is contractually secret-free.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{operator_message}")]
pub struct BridgeUnavailable {
    /// Names of the missing items: `"node_runtime"`, `"bridge_path"`, `"backend"`.
    pub findings: Vec<String>,
    /// One actionable message naming exactly what is missing and how to supply it.
    pub operator_message: String,
}

/// A launch the filesystem says is actually possible.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BridgeLaunch {
    /// Resolved Node interpreter.
    pub node: PathBuf,
    /// Resolved bridge entrypoint.
    pub script: PathBuf,
    /// Backend to request via `--backend`.
    pub backend: WhatsappBackend,
    /// Session directory to request via `--session`, when configured.
    pub session_dir: Option<PathBuf>,
}

/// Resolve a Node interpreter without running anything.
///
/// Platform difference is centralised here (AGENTS.md): Windows needs the
/// `PATHEXT` extensions appended, Unix does not.
fn resolve_node(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.is_file().then(|| p.to_path_buf());
    }

    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .collect()
    } else {
        vec![String::new()]
    };

    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        for ext in &extensions {
            let candidate = dir.join(format!("node{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Whether the backend's npm package is resolvable from the bridge's directory.
///
/// # Why this check exists
///
/// Driving the real `bridge.js` with no `node_modules` present shows that
/// `health` **still succeeds** — it is answered before any backend is loaded —
/// while the very next `connect` fails with
/// `-32000 Failed to load backend baileys: Cannot find module …`. A readiness
/// verdict taken from the handshake alone would therefore report a green for a
/// bridge that cannot send a single message. This is that gap closed.
///
/// Node resolves `node_modules` by walking up from the importing file, so this
/// walks the same ancestors rather than checking only the sibling directory —
/// a hoisted install must not read as missing.
fn backend_package_installed(bridge_path: &Path, backend: WhatsappBackend) -> bool {
    let Some(pkg) = backend.npm_package() else {
        return true;
    };
    let Some(dir) = bridge_path.parent() else {
        return false;
    };
    dir.ancestors()
        .any(|a| a.join("node_modules").join(pkg).is_dir())
}

/// Resolve the directory the bridge will keep pairing material in.
///
/// Mirrors the bridge's own default (`$HOME/.wayland/whatsapp`) so the answer
/// is the same one the subprocess will compute. Measured from
/// `backends/baileys.js` and `backends/whatsapp-web.js`.
fn pairing_dir(cfg: &WhatsappBridgeConfig) -> Option<PathBuf> {
    let (subdir, marker) = cfg.backend.pairing_marker()?;
    let root = match cfg.session_dir.as_ref() {
        Some(d) => d.clone(),
        None => dirs_home()?.join(".wayland").join("whatsapp"),
    };
    Some(root.join(subdir).join(marker))
}

/// Home directory, without taking a dependency for one lookup.
fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
}

/// Decide whether the bridge can be launched, from the filesystem alone.
///
/// Spawns nothing and sends nothing. Every missing item is collected, so an
/// operator who is missing two things is told about both rather than
/// discovering the second after fixing the first.
pub fn preflight(cfg: &WhatsappBridgeConfig) -> Result<BridgeLaunch, BridgeUnavailable> {
    let mut findings = Vec::new();
    let mut lines = Vec::new();

    if !cfg.backend.is_bridged() {
        findings.push("backend".to_string());
        lines.push(format!(
            "backend {:?} is not driven through the bridge — use the Cloud API adapter for it, \
             or set backend to one of: {}",
            cfg.backend.wire_name(),
            ["baileys", "whatsapp-web"].join(", ")
        ));
    }

    let node = resolve_node(cfg.node_path.as_deref());
    if node.is_none() {
        findings.push("node_runtime".to_string());
        lines.push(match cfg.node_path.as_deref() {
            Some(p) => format!(
                "node_path points at {} which is not a file — install Node 18+ or correct node_path",
                p.display()
            ),
            None => "no `node` on PATH — the bridge is a Node program; install Node 18+ or set \
                     node_path in the channel config"
                .to_string(),
        });
    }

    if !cfg.bridge_path.is_file() {
        findings.push("bridge_path".to_string());
        lines.push(format!(
            "bridge_path {} is not a file — wayland-core does not ship the bridge; point \
             bridge_path at the bridge.js of a Wayland Desktop install (or a checkout of it) \
             whose dependencies are installed",
            cfg.bridge_path.display()
        ));
    } else if !backend_package_installed(&cfg.bridge_path, cfg.backend) {
        // Only meaningful when the script exists — otherwise this would just
        // restate the finding above.
        findings.push("bridge_dependencies".to_string());
        lines.push(format!(
            "the bridge at {} has no resolvable {} — its `health` will still answer, but the \
             first connect fails with `Cannot find module`; run `npm install` (or `bun install`) \
             in the bridge's directory",
            cfg.bridge_path.display(),
            cfg.backend.npm_package().unwrap_or("backend package"),
        ));
    }

    if findings.is_empty() {
        // Both unwraps are guarded by the checks above.
        return Ok(BridgeLaunch {
            node: node.expect("node resolved: findings is empty"),
            script: cfg.bridge_path.clone(),
            backend: cfg.backend,
            session_dir: cfg.session_dir.clone(),
        });
    }

    Err(BridgeUnavailable {
        findings,
        operator_message: format!(
            "whatsapp backend {} is not available: {}",
            cfg.backend.wire_name(),
            lines.join("; ")
        ),
    })
}

// ---------------------------------------------------------------------------
// JSON-RPC framing
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
type Inbox = Arc<Mutex<VecDeque<ChannelEvent>>>;

/// The half of a launched bridge the session talks over. Separated from the
/// spawn so tests can drive the exact same reader/writer over an in-memory
/// duplex without a Node process.
struct Wire {
    stdin: Box<dyn AsyncWrite + Unpin + Send>,
    stdout: Box<dyn AsyncBufRead + Unpin + Send>,
    child: Option<tokio::process::Child>,
}

/// A live bridge subprocess plus its reader task.
struct BridgeSession {
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
    async fn open(
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

    async fn call(
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
    async fn close(&self) {
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

// ---------------------------------------------------------------------------
// The adapter
// ---------------------------------------------------------------------------

/// A WhatsApp channel driven through the Node bridge subprocess.
///
/// Construction never spawns anything and never fails — a Core install with a
/// bridged channel configured but no Node present still boots. The failure
/// surfaces at [`Channel::start`] and [`Channel::probe`], named.
pub struct WhatsappBridgeChannel {
    name: String,
    config: WhatsappBridgeConfig,
    state: ConnectionState,
    session: Option<Arc<BridgeSession>>,
    inbox: Inbox,
}

impl WhatsappBridgeChannel {
    /// Construct. Spawns nothing; see [`Channel::probe`] for readiness.
    pub fn new(name: impl Into<String>, config: WhatsappBridgeConfig) -> Self {
        Self {
            name: name.into(),
            config,
            state: ConnectionState::Disconnected,
            session: None,
            inbox: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// The backend this instance drives.
    pub fn backend(&self) -> WhatsappBackend {
        self.config.backend
    }

    /// Cached connection state.
    pub fn state(&self) -> ConnectionState {
        self.state
    }

    fn session(&self) -> Result<&Arc<BridgeSession>, ChannelError> {
        self.session.as_ref().ok_or(ChannelError::NotStarted)
    }
}

#[async_trait]
impl Channel for WhatsappBridgeChannel {
    fn name(&self) -> &str {
        &self.name
    }

    fn platform(&self) -> &str {
        PLATFORM
    }

    async fn start(&mut self) -> Result<(), ChannelError> {
        if self.state == ConnectionState::Connected {
            return Ok(());
        }

        // Fail closed BEFORE any state change: a channel that cannot launch
        // must not pass through `Connecting`, because a health projection that
        // samples mid-start would read that as progress.
        let launch = preflight(&self.config).map_err(BridgeError::from)?;

        self.state = ConnectionState::Connecting;

        let session = BridgeSession::open(
            &launch,
            Arc::clone(&self.inbox),
            Duration::from_secs(self.config.handshake_timeout_secs),
            Duration::from_secs(self.config.rpc_timeout_secs),
        )
        .await
        .inspect_err(|_| {
            self.state = ConnectionState::Disconnected;
        })?;

        let session = Arc::new(session);

        // `connect` returns as soon as the socket is created; pairing (QR) and
        // the transition to `connected` arrive as notifications afterwards.
        // So the adapter is Connecting here, NOT Connected — the bridge's own
        // `connection.status` promotes it. Declaring Connected now would be an
        // adapter attesting to a link it has not seen come up.
        if let Err(e) = session.call("connect", serde_json::json!({})).await {
            session.close().await;
            self.state = ConnectionState::Disconnected;
            return Err(e.into());
        }

        self.session = Some(session);
        self.inbox
            .lock()
            .await
            .push_back(ChannelEvent::ConnectionStateChanged {
                state: ConnectionState::Connecting,
            });
        Ok(())
    }

    async fn stop(&mut self) -> Result<(), ChannelError> {
        if let Some(session) = self.session.take() {
            let _ = session.call("disconnect", serde_json::json!({})).await;
            session.close().await;
        }
        if self.state != ConnectionState::Disconnected {
            self.state = ConnectionState::Disconnected;
            self.inbox
                .lock()
                .await
                .push_back(ChannelEvent::ConnectionStateChanged {
                    state: ConnectionState::Disconnected,
                });
        }
        Ok(())
    }

    async fn poll_events(&mut self) -> Result<Vec<ChannelEvent>, ChannelError> {
        let mut inbox = self.inbox.lock().await;
        if inbox.is_empty() && self.session.is_none() {
            return Err(ChannelError::NotStarted);
        }
        // The bridge owns the authoritative link state; mirror the last one we
        // saw so `state()` does not lie to a UI that polls it.
        let events: Vec<ChannelEvent> = inbox.drain(..).collect();
        for ev in &events {
            if let ChannelEvent::ConnectionStateChanged { state } = ev {
                self.state = *state;
            }
        }
        Ok(events)
    }

    async fn send_message(&mut self, msg: OutgoingMessage) -> Result<MessageReceipt, ChannelError> {
        let session = self.session()?;

        // Both bridged backends' `sendMedia` take a LOCAL filePath and reject
        // anything else (verified in backends/baileys.js and
        // backends/whatsapp-web.js) — the `mediaUrl` form is meta-business
        // only. `OutgoingMessage::attachments` carries URLs, so there is no
        // honest mapping. Refuse loudly rather than send the text and drop the
        // attachment silently.
        if !msg.attachments.is_empty() {
            return Err(ChannelError::Unsupported {
                op: "send_message with attachments".to_string(),
                platform: format!("whatsapp/{}", self.config.backend),
            });
        }

        let chat_id = if msg.conversation_id.is_empty() {
            if self.config.default_recipient.is_empty() {
                return Err(ChannelError::Rejected(
                    "no conversation_id and no default_recipient configured".to_string(),
                ));
            }
            self.config.default_recipient.clone()
        } else {
            msg.conversation_id.clone()
        };

        let result = session
            .call(
                "sendText",
                serde_json::json!({ "chatId": chat_id, "text": msg.text }),
            )
            .await?;

        let id = result
            .get("messageId")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();

        Ok(MessageReceipt {
            id,
            conversation_id: chat_id,
            ts_secs: chrono::Utc::now().timestamp(),
        })
    }

    async fn react(
        &self,
        conversation_id: &str,
        message_id: &str,
        emoji: &str,
    ) -> Result<(), ChannelError> {
        let session = self.session()?;
        session
            .call(
                "react",
                serde_json::json!({
                    "chatId": conversation_id,
                    "messageId": message_id,
                    "emoji": emoji,
                }),
            )
            .await?;
        Ok(())
    }

    async fn send_typing(&self, conversation_id: &str) -> Result<(), ChannelError> {
        let session = self.session()?;
        session
            .call(
                "setPresence",
                serde_json::json!({ "chatId": conversation_id, "presence": "composing" }),
            )
            .await?;
        Ok(())
    }

    /// What this adapter drives through the bridge, and what it does not.
    ///
    /// `edit`/`delete`: the bridge's method allowlist has no entry for either,
    /// so there is no RPC to call — this is a bridge-surface absence, recorded
    /// as `NotImplemented` (a wider bridge could add them) rather than
    /// `PlatformHasNoApi` (the Web protocol does have both).
    fn native_actions(&self) -> wcore_channels::NativeActions {
        use wcore_channels::ActionSupport::{Implemented, NotImplemented};
        wcore_channels::NativeActions::none()
            .edit(NotImplemented)
            .delete(NotImplemented)
            .react(Implemented)
            .typing(Implemented)
            .note(
                "driven through the Wayland Desktop WhatsApp bridge. edit/delete: the bridge's \
                 ALLOWED_RPC_METHODS has no edit or delete method, so no RPC exists to call. \
                 Attachments are refused: both bridged backends' sendMedia require a local \
                 filePath and OutgoingMessage carries URLs.",
            )
    }

    /// Readiness, answered from the filesystem and then from the bridge itself.
    ///
    /// This is the surface that keeps the capability from being advertised when
    /// it is not reachable, and every one of its three gates was put there by a
    /// measurement rather than by anticipation:
    ///
    /// 1. **[`preflight`]** — Node, the script, and the backend's npm package.
    ///    Without the third, a bridge with no `node_modules` answers `health`
    ///    happily and then fails every `connect`.
    /// 2. **The `health` handshake** — the bridge confirms, in its own words,
    ///    which backend it loaded.
    /// 3. **Pairing material on disk** — a reachable bridge that has never been
    ///    paired cannot send anything. `ProbeOutcome::Ok` sets
    ///    `authenticated: true`, so claiming it without evidence of a pairing
    ///    would be exactly the unearned green this type exists to prevent.
    async fn probe(&self) -> Result<ProbeReport, ChannelError> {
        let launch = match preflight(&self.config) {
            Ok(l) => l,
            // `Incomplete` is the right verdict: nothing has been asked of the
            // platform yet, the operator's own machine is missing a piece.
            Err(u) => return Ok(ProbeReport::incomplete(&self.name, PLATFORM, u.findings)),
        };

        // A throwaway inbox: a probe must not inject events into the live one.
        let scratch: Inbox = Arc::new(Mutex::new(VecDeque::new()));
        match BridgeSession::open(
            &launch,
            scratch,
            Duration::from_secs(self.config.handshake_timeout_secs),
            Duration::from_secs(self.config.rpc_timeout_secs),
        )
        .await
        {
            Ok(session) => {
                session.close().await;

                // Gate 3. Both bridged backends authenticate by QR pairing, and
                // a probe must send nothing — so pairing is read from the
                // material the backend persists, at the path the bridge itself
                // computes.
                match pairing_dir(&self.config) {
                    Some(marker) if marker.exists() => Ok(ProbeReport::ok(
                        &self.name,
                        PLATFORM,
                        format!("bridge/{}", self.config.backend),
                    )),
                    _ => Ok(ProbeReport::incomplete(
                        &self.name,
                        PLATFORM,
                        vec!["whatsapp_pairing".to_string()],
                    )),
                }
            }
            Err(BridgeError::BackendMismatch { want, got }) => Ok(ProbeReport::unauthenticated(
                &self.name,
                PLATFORM,
                format!("backend_mismatch: requested {want}, bridge loaded {got}"),
            )),
            Err(e) => Ok(ProbeReport::unreachable(
                &self.name,
                PLATFORM,
                e.to_string(),
            )),
        }
    }

    fn config_schema(&self) -> &str {
        include_str!("../schemas/whatsapp-bridge.json")
    }

    /// WhatsApp caps a single text body at 4096 characters on every backend.
    fn max_message_len(&self) -> Option<usize> {
        Some(4096)
    }

    // `supports_outbound_idempotency` is deliberately NOT overridden. The
    // bridge transmits no idempotency key and neither WhatsApp Web backend
    // accepts one, so the trait default of `false` is the true answer. Slack
    // and Discord both declared `true` here on mockito evidence and both
    // produced duplicates against the real API; a mock cannot witness what a
    // destination does with a key.
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    fn cfg(backend: WhatsappBackend, bridge_path: PathBuf) -> WhatsappBridgeConfig {
        WhatsappBridgeConfig {
            backend,
            bridge_path,
            node_path: None,
            session_dir: None,
            workspace_name: "test".to_string(),
            default_recipient: String::new(),
            handshake_timeout_secs: 5,
            rpc_timeout_secs: 5,
        }
    }

    // -- selector: both directions -----------------------------------------

    #[test]
    fn backend_default_is_meta_business_not_baileys() {
        // The load-bearing default. bridge.js defaults to `baileys`; Core must
        // not inherit that, or an install that says nothing about backends
        // would drive an unofficial client.
        assert_eq!(WhatsappBackend::default(), WhatsappBackend::MetaBusiness);
        assert!(!WhatsappBackend::default().is_bridged());
        assert!(!WhatsappBackend::default().is_unofficial());
    }

    #[test]
    fn backend_selector_accepts_exactly_the_three_known_names() {
        // CAN PASS.
        assert_eq!(
            WhatsappBackend::from_str("meta-business").unwrap(),
            WhatsappBackend::MetaBusiness
        );
        assert_eq!(
            WhatsappBackend::from_str("baileys").unwrap(),
            WhatsappBackend::Baileys
        );
        assert_eq!(
            WhatsappBackend::from_str("whatsapp-web").unwrap(),
            WhatsappBackend::WhatsappWeb
        );
    }

    #[test]
    fn backend_selector_rejects_an_unknown_name_rather_than_defaulting() {
        // CAN FAIL. A typo must not silently become baileys.
        for bad in ["baileyz", "", "META-BUSINESS", "whatsapp_web", "cloud"] {
            let err = WhatsappBackend::from_str(bad)
                .unwrap_err_or_panic(&format!("{bad:?} must be rejected"));
            assert_eq!(err.got, bad);
            assert!(
                err.to_string().contains("baileys"),
                "the rejection must name the valid options: {err}"
            );
        }
    }

    #[test]
    fn wire_names_round_trip_through_serde_and_from_str() {
        // The two parsers must not drift: `--backend` uses `wire_name`, the
        // TOML uses serde, and the health read-back compares against
        // `wire_name`. A mismatch would make the handshake permanently fail.
        for b in [
            WhatsappBackend::MetaBusiness,
            WhatsappBackend::Baileys,
            WhatsappBackend::WhatsappWeb,
        ] {
            let json = serde_json::to_string(&b).unwrap();
            let unquoted = json.trim_matches('"');
            assert_eq!(unquoted, b.wire_name(), "serde and wire_name disagree");
            assert_eq!(WhatsappBackend::from_str(unquoted).unwrap(), b);
        }
        assert_eq!(
            WhatsappBackend::ALL_WIRE_NAMES.len(),
            3,
            "ALL_WIRE_NAMES must list every variant"
        );
    }

    #[test]
    fn toml_rejects_an_unknown_backend_value() {
        // The registry parses TOML, so the rejection must survive that path too.
        let body = r#"
backend = "baileyz"
bridge_path = "/nonexistent/bridge.js"
"#;
        let err = toml::from_str::<WhatsappBridgeConfig>(body).unwrap_err();
        assert!(
            err.to_string().contains("baileyz") || err.to_string().contains("unknown variant"),
            "expected an unknown-variant rejection, got: {err}"
        );
    }

    #[test]
    fn toml_accepts_a_known_backend_value() {
        // Control for the test above: the same shape with a valid name parses,
        // proving the rejection came from the VALUE and not the schema.
        let body = r#"
backend = "baileys"
bridge_path = "/nonexistent/bridge.js"
"#;
        let cfg: WhatsappBridgeConfig = toml::from_str(body).unwrap();
        assert_eq!(cfg.backend, WhatsappBackend::Baileys);
        assert_eq!(cfg.handshake_timeout_secs, DEFAULT_HANDSHAKE_TIMEOUT_SECS);
    }

    // -- preflight: both directions ----------------------------------------

    #[test]
    fn preflight_fails_closed_naming_a_missing_bridge_script() {
        let c = cfg(
            WhatsappBackend::Baileys,
            PathBuf::from("/definitely/not/here/bridge.js"),
        );
        let err = preflight(&c).unwrap_err();
        assert!(
            err.findings.contains(&"bridge_path".to_string()),
            "findings must NAME the missing item, got {:?}",
            err.findings
        );
        assert!(
            err.operator_message.contains("does not ship the bridge"),
            "the message must tell the operator what to do: {}",
            err.operator_message
        );
    }

    #[test]
    fn preflight_fails_closed_naming_a_missing_node_runtime() {
        // node_path pointing at a nonexistent file is the deterministic way to
        // express "no Node" — emptying PATH would be process-global and would
        // race every other test in this binary. The bridge is a fully INSTALLED
        // one so that `node_runtime` is genuinely the only thing missing.
        let (_dir, script) = installed_bridge(WhatsappBackend::Baileys);
        let mut c = cfg(WhatsappBackend::Baileys, script);
        c.node_path = Some(PathBuf::from("/definitely/not/here/node"));

        let err = preflight(&c).unwrap_err();
        assert_eq!(
            err.findings,
            vec!["node_runtime".to_string()],
            "only Node is missing — the script and its dependencies exist"
        );
        assert!(err.operator_message.contains("install Node"));
    }

    #[test]
    fn preflight_names_every_missing_item_at_once() {
        let mut c = cfg(
            WhatsappBackend::Baileys,
            PathBuf::from("/definitely/not/here/bridge.js"),
        );
        c.node_path = Some(PathBuf::from("/definitely/not/here/node"));
        let err = preflight(&c).unwrap_err();
        assert!(err.findings.contains(&"node_runtime".to_string()));
        assert!(err.findings.contains(&"bridge_path".to_string()));
        assert_eq!(err.findings.len(), 2);
    }

    #[test]
    fn preflight_refuses_meta_business_because_it_is_not_a_bridged_backend() {
        // A config that asks the bridge for the Cloud API is a routing mistake,
        // and must be named rather than silently spawning Node to do something
        // Core already does natively over HTTPS.
        let script = tempfile::NamedTempFile::new().unwrap();
        let c = cfg(WhatsappBackend::MetaBusiness, script.path().to_path_buf());
        let err = preflight(&c).unwrap_err();
        assert!(err.findings.contains(&"backend".to_string()));
    }

    /// Lay out a directory that looks the way an installed bridge looks:
    /// `<root>/bridge.js` plus `<root>/node_modules/<pkg>/`. Returns the
    /// tempdir (kept alive by the caller) and the script path.
    fn installed_bridge(backend: WhatsappBackend) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("bridge.js");
        std::fs::write(&script, "// stand-in\n").unwrap();
        if let Some(pkg) = backend.npm_package() {
            std::fs::create_dir_all(dir.path().join("node_modules").join(pkg)).unwrap();
        }
        (dir, script)
    }

    #[test]
    fn preflight_passes_when_node_the_script_and_the_backend_package_all_exist() {
        // CAN PASS — the control proving the failures above are about missing
        // items and not about preflight being unable to succeed. A real file
        // stands in for Node: preflight checks the path, it runs nothing.
        let (_dir, script) = installed_bridge(WhatsappBackend::Baileys);
        let fake_node = tempfile::NamedTempFile::new().unwrap();
        let mut c = cfg(WhatsappBackend::Baileys, script.clone());
        c.node_path = Some(fake_node.path().to_path_buf());

        let launch = preflight(&c).expect("preflight must be able to succeed");
        assert_eq!(launch.backend, WhatsappBackend::Baileys);
        assert_eq!(launch.script, script);
        assert_eq!(launch.node, fake_node.path());
    }

    #[test]
    fn preflight_names_bridge_dependencies_when_the_backend_package_is_absent() {
        // The gate that closes the measured gap: the real bridge answers
        // `health` with no node_modules and only fails at the first `connect`,
        // so a handshake-only verdict would hand out an unearned green.
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("bridge.js");
        std::fs::write(&script, "// stand-in\n").unwrap();
        let fake_node = tempfile::NamedTempFile::new().unwrap();
        let mut c = cfg(WhatsappBackend::Baileys, script);
        c.node_path = Some(fake_node.path().to_path_buf());

        let err = preflight(&c).unwrap_err();
        assert_eq!(err.findings, vec!["bridge_dependencies".to_string()]);
        assert!(
            err.operator_message.contains("@whiskeysockets/baileys"),
            "the message must name the package to install: {}",
            err.operator_message
        );
    }

    #[test]
    fn a_hoisted_node_modules_above_the_bridge_counts_as_installed() {
        // Node resolves node_modules by walking up, so checking only the
        // sibling directory would report a false red on a hoisted install.
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(
            root.path()
                .join("node_modules")
                .join("@whiskeysockets/baileys"),
        )
        .unwrap();
        let nested = root.path().join("packages").join("bridge");
        std::fs::create_dir_all(&nested).unwrap();
        let script = nested.join("bridge.js");
        std::fs::write(&script, "// stand-in\n").unwrap();

        assert!(
            backend_package_installed(&script, WhatsappBackend::Baileys),
            "known-positive: a hoisted install must resolve"
        );
        assert!(
            !backend_package_installed(&script, WhatsappBackend::WhatsappWeb),
            "known-negative: a package that is NOT installed must not resolve"
        );
    }

    #[test]
    fn each_bridged_backend_checks_for_its_own_package() {
        // Guards against a check that passes for any backend once one package
        // happens to be installed.
        let (_d1, baileys_script) = installed_bridge(WhatsappBackend::Baileys);
        assert!(backend_package_installed(
            &baileys_script,
            WhatsappBackend::Baileys
        ));
        assert!(!backend_package_installed(
            &baileys_script,
            WhatsappBackend::WhatsappWeb
        ));

        let (_d2, www_script) = installed_bridge(WhatsappBackend::WhatsappWeb);
        assert!(backend_package_installed(
            &www_script,
            WhatsappBackend::WhatsappWeb
        ));
        assert!(!backend_package_installed(
            &www_script,
            WhatsappBackend::Baileys
        ));
    }

    #[test]
    fn pairing_marker_paths_match_the_layout_each_backend_actually_writes() {
        // Measured from backends/baileys.js (useMultiFileAuthState under
        // <session>/baileys, creds.json) and backends/whatsapp-web.js
        // (LocalAuth dataPath <session>/whatsapp-web, clientId "wayland").
        let session = PathBuf::from("/srv/wa");
        let mut c = cfg(WhatsappBackend::Baileys, PathBuf::from("/x/bridge.js"));
        c.session_dir = Some(session.clone());
        assert_eq!(
            pairing_dir(&c),
            Some(session.join("baileys").join("creds.json"))
        );

        c.backend = WhatsappBackend::WhatsappWeb;
        assert_eq!(
            pairing_dir(&c),
            Some(session.join("whatsapp-web").join("session-wayland"))
        );

        // meta-business has no bridge pairing at all.
        c.backend = WhatsappBackend::MetaBusiness;
        assert_eq!(pairing_dir(&c), None);
    }

    #[test]
    fn resolve_node_finds_a_real_file_and_rejects_a_missing_one() {
        // Instrument control for `resolve_node`'s explicit-path arm: a
        // known-positive and a known-negative in the same test, so a resolver
        // that always returned None could not pass.
        let real = tempfile::NamedTempFile::new().unwrap();
        assert_eq!(
            resolve_node(Some(real.path())).as_deref(),
            Some(real.path()),
            "known-positive: an existing file must resolve"
        );
        assert_eq!(
            resolve_node(Some(Path::new("/definitely/not/here/node"))),
            None,
            "known-negative: a missing file must not resolve"
        );
    }

    // -- adapter: fails closed without ever reporting health ----------------

    #[tokio::test]
    async fn construction_never_spawns_and_start_fails_closed_when_node_is_absent() {
        let mut c = cfg(
            WhatsappBackend::Baileys,
            PathBuf::from("/definitely/not/here/bridge.js"),
        );
        c.node_path = Some(PathBuf::from("/definitely/not/here/node"));

        // Construction must succeed — a Core install with this configured but
        // nothing installed still has to boot.
        let mut ch = WhatsappBridgeChannel::new("wa-personal", c);
        assert_eq!(ch.state(), ConnectionState::Disconnected);

        let err = ch.start().await.unwrap_err();
        match &err {
            ChannelError::Config(m) => {
                assert!(m.contains("node"), "must name node: {m}");
                assert!(m.contains("bridge_path"), "must name bridge_path: {m}");
            }
            other => panic!("expected Config with an actionable message, got {other:?}"),
        }
        assert_eq!(
            ch.state(),
            ConnectionState::Disconnected,
            "a channel that cannot launch must NOT sit in Connecting"
        );
    }

    #[tokio::test]
    async fn probe_reports_incomplete_and_not_ready_when_the_bridge_is_absent() {
        // The anti-false-advertising property: an unreachable bridge must never
        // read as ready anywhere.
        let mut c = cfg(
            WhatsappBackend::Baileys,
            PathBuf::from("/definitely/not/here/bridge.js"),
        );
        c.node_path = Some(PathBuf::from("/definitely/not/here/node"));

        let ch = WhatsappBridgeChannel::new("wa-personal", c);
        let report = ch.probe().await.unwrap();

        assert_eq!(
            report.outcome,
            wcore_channels::probe::ProbeOutcome::Incomplete
        );
        assert!(
            !report.outcome.is_ready(),
            "an absent bridge must not be advertised as ready"
        );
        assert!(!report.config_complete);
        assert!(!report.authenticated);
        assert!(report.findings.contains(&"node_runtime".to_string()));
        assert!(report.findings.contains(&"bridge_path".to_string()));
    }

    #[tokio::test]
    async fn a_bridged_channel_does_not_claim_outbound_idempotency() {
        // Slack and Discord both claimed this on mock evidence and both
        // duplicated against the real API. The bridge transmits no key.
        let ch =
            WhatsappBridgeChannel::new("wa", cfg(WhatsappBackend::Baileys, PathBuf::from("/nope")));
        assert!(!ch.supports_outbound_idempotency());
    }

    #[tokio::test]
    async fn attachments_are_refused_rather_than_silently_dropped() {
        // There is no honest mapping from a URL attachment to either bridged
        // backend's local-filePath sendMedia, so the send must fail visibly.
        // Driven over a fabricated wire so no Node is needed.
        let (session, _srv) = test_session(WhatsappBackend::Baileys).await;
        let mut ch =
            WhatsappBridgeChannel::new("wa", cfg(WhatsappBackend::Baileys, PathBuf::from("/nope")));
        ch.session = Some(session);

        let msg = OutgoingMessage {
            conversation_id: "123@s.whatsapp.net".to_string(),
            text: "see attached".to_string(),
            reply_to: None,
            attachments: vec!["https://cdn.example/pic.jpg".to_string()],
        };
        let err = ch.send_message(msg).await.unwrap_err();
        assert!(
            matches!(err, ChannelError::Unsupported { .. }),
            "got {err:?}"
        );
    }

    // -- handshake: both directions, over a fabricated wire ------------------
    //
    // These drive the REAL reader/writer against an in-memory duplex, so they
    // prove the framing and the read-back logic without Node. They do NOT
    // prove the real bridge.js behaves this way — that claim rests on the
    // live run recorded in the lane evidence directory, against the real
    // unmodified bridge.js under a real Node.

    /// Stand up a scripted peer that answers `health` with `backend`, then
    /// echoes a fixed reply to everything else.
    async fn scripted_peer(
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

    async fn test_session(
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

    /// Small helper so the rejection test reads as one assertion per case.
    trait UnwrapErrOrPanic<T, E> {
        fn unwrap_err_or_panic(self, msg: &str) -> E;
    }

    impl<T: std::fmt::Debug, E> UnwrapErrOrPanic<T, E> for Result<T, E> {
        fn unwrap_err_or_panic(self, msg: &str) -> E {
            match self {
                Ok(v) => panic!("{msg} — but it was accepted as {v:?}"),
                Err(e) => e,
            }
        }
    }
}
