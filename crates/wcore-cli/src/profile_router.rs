//! `CliProfileRouter` — the wcore-cli implementation of
//! [`wcore_acp::router::ProfileRouter`] (persona-profiles PR-7).
//!
//! An in-process persona overlay (PR-4') and a `profile:<name>` agent are two
//! DIFFERENT mechanisms. A persona shares this process's single credential/home
//! identity; a profile is a SEPARATE PROCESS (`wcore acp serve --profile
//! <name>`) with its OWN `WAYLAND_HOME` ⇒ own keys/.env/memory/SOUL. One
//! profile per process is the ONLY safe topology — N profiles in one address
//! space is the credential-bleed the red-team rejected (shared
//! `WAYLAND_HOME`/`*_API_KEY`/egress singletons). This router therefore never
//! resolves a profile to an in-process overlay; it spawns/routes to that
//! profile's dedicated child and forwards JSON-RPC over a localhost loopback.
//!
//! # Invariants carried from the design (06-supervisor-seams) + review hardening
//!   * **One child = one profile = one identity.** Never multiplex profiles.
//!   * **No inherited credentials.** A child is spawned with a CLEARED
//!     environment (only a credential-free allowlist is passed through), so its
//!     `*_API_KEY`/`.env` identity comes SOLELY from its own `WAYLAND_HOME` — a
//!     `*_API_KEY` present in the supervisor's own env can't shadow it.
//!   * **Fail closed.** An invalid/absent profile ([`profile_dir`] error or a
//!     missing home dir) returns [`AcpError::Agent`] (⇒ `AgentNotFound`) — the
//!     child is NEVER spawned and we NEVER fall through to the default home.
//!   * **No half-up children.** A spawned child that fails its health-check is
//!     killed + reaped before the error returns.
//!   * **Per-child key.** Each child gets a fresh random `X-API-Key` injected
//!     via `WAYLAND_ACP_SERVER_KEY`; one operator key is never shared.
//!   * **Signal-safe reaping.** Live children are held in a process-global
//!     registry so the signal handler (which `std::process::exit`s, bypassing
//!     `Drop`) can reap every child — no orphaned credential-bearing processes.
//!   * **Bounded.** Concurrent children are capped.

use std::collections::HashMap;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child as ProcChild, Command, Stdio};
use std::sync::Mutex as StdMutex;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, Stream, StreamExt};
use tokio::sync::Mutex;

use wcore_acp::client::AcpClient;
use wcore_acp::error::AcpError;
use wcore_acp::protocol::{
    ErrorCode, JsonRpcError, MessageEvent, MessageSendRequest, SessionCreateRequest,
    SessionGetResponse,
};
use wcore_acp::router::ProfileRouter;

/// Max concurrent per-profile children. A ceiling so a hostile/buggy client
/// cannot fork-bomb the host by opening unbounded distinct profiles.
const DEFAULT_MAX_CHILDREN: usize = 16;

/// Health-check poll interval for a freshly spawned child.
const HEALTH_POLL_INTERVAL: Duration = Duration::from_millis(100);
/// Health-check attempts before giving up (⇒ ~5s budget) and reaping the child.
const HEALTH_POLL_ATTEMPTS: usize = 50;

/// Environment variables passed through to a spawned child. This is an
/// ALLOWLIST: the child is spawned with a cleared environment and only these
/// (credential-FREE) vars are re-added. Any `*_API_KEY`/token in the
/// supervisor's own environment is therefore NEVER inherited — a child's
/// provider credentials come solely from its own `WAYLAND_HOME`. Adding a var
/// here must be a deliberate, non-secret choice.
#[cfg(unix)]
const ENV_PASSTHROUGH: &[&str] = &[
    // process basics + external-tool resolution
    "PATH",
    "HOME",
    "USER",
    "LOGNAME",
    "SHELL",
    "TERM",
    "TMPDIR",
    "TZ",
    // locale (avoid encoding breakage)
    "LANG",
    "LANGUAGE",
    "LC_ALL",
    "LC_CTYPE",
    // XDG base dirs (the `dirs` crate consults these on Linux)
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_RUNTIME_DIR",
    // TLS trust roots (HTTPS to providers on distros that set these)
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    // diagnostics
    "RUST_BACKTRACE",
    "RUST_LOG",
    // REQUIRED: the child's profile_dir(name) must resolve to the SAME root we
    // pass as WAYLAND_HOME, or its resolve_profile_home guard refuses to start.
    "WAYLAND_PROFILES_ROOT",
];
#[cfg(windows)]
const ENV_PASSTHROUGH: &[&str] = &[
    "PATH",
    "PATHEXT",
    "SystemRoot",
    "SystemDrive",
    "windir",
    "ComSpec",
    "USERPROFILE",
    "HOMEDRIVE",
    "HOMEPATH",
    "TEMP",
    "TMP",
    "APPDATA",
    "LOCALAPPDATA",
    "PROGRAMDATA",
    "NUMBER_OF_PROCESSORS",
    "PROCESSOR_ARCHITECTURE",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "RUST_BACKTRACE",
    "RUST_LOG",
    "WAYLAND_PROFILES_ROOT",
];

/// Process-global registry of live per-profile child processes, keyed by pid.
///
/// This is the SINGLE owner of the OS `Child` handles so BOTH the async router
/// (normal per-session reap) AND the synchronous process signal handler can reap
/// them. The signal handler (`main.rs`) calls `std::process::exit(0)`, which
/// bypasses every `Drop`; without a sync-reachable registry, a SIGTERM/SIGINT/
/// SIGHUP would orphan every credential-bearing child. A plain `std::sync::Mutex`
/// (not the tokio one) so it is lockable from the non-async signal path.
static LIVE_CHILDREN: StdMutex<Option<HashMap<u32, ProcChild>>> = StdMutex::new(None);

/// Move a spawned child into the global registry; returns its pid.
fn register_child(child: ProcChild) -> u32 {
    let pid = child.id();
    let mut g = LIVE_CHILDREN.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(HashMap::new).insert(pid, child);
    pid
}

/// Whether the registered child for `pid` has already exited (fast-fail during
/// health-check). Treats an unregistered pid as gone.
fn child_exited(pid: u32) -> bool {
    let mut g = LIVE_CHILDREN.lock().unwrap_or_else(|e| e.into_inner());
    match g.as_mut().and_then(|m| m.get_mut(&pid)) {
        Some(child) => matches!(child.try_wait(), Ok(Some(_))),
        None => true,
    }
}

/// Kill + wait the registered child with `pid` (normal per-session reap or
/// half-up cleanup). Best-effort, idempotent.
fn reap_child(pid: u32) {
    let mut g = LIVE_CHILDREN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = g.as_mut()
        && let Some(mut child) = map.remove(&pid)
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Kill + wait EVERY live profile child. Signal-safe (synchronous, `std` mutex).
///
/// Call this from the process signal handler BEFORE `std::process::exit(0)` so a
/// normal shutdown (SIGTERM/SIGINT/SIGHUP, Windows Ctrl+C) never orphans a
/// credential-bearing `acp serve --profile` child. Idempotent.
pub fn reap_all_children_blocking() {
    let mut g = LIVE_CHILDREN.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(map) = g.as_mut() {
        for (_pid, mut child) in map.drain() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Supervisor that fans `profile:<name>` ACP sessions out to a dedicated child
/// process per profile. Installed on an
/// [`wcore_acp::server::AcpServer`](wcore_acp::server::AcpServer) via
/// `with_profile_router`.
pub struct CliProfileRouter {
    /// This binary — re-invoked as `acp serve --profile <name>` per child.
    exe: PathBuf,
    /// Concurrent-child ceiling.
    max_children: usize,
    /// Router bookkeeping. The tokio mutex is held across spawn + health-check
    /// (the double-spawn guard) but RELEASED before per-session network round
    /// trips (`create_session`/`delete_session`) so one profile's I/O can't
    /// starve another's session open/close.
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    /// profile name -> its child's bookkeeping.
    children: HashMap<String, ChildMeta>,
    /// parent session id -> the child it routes to.
    routes: HashMap<String, Route>,
}

/// Where a parent session is mapped inside a child.
struct Route {
    profile: String,
    child_session_id: String,
}

/// Per-profile child bookkeeping. The OS `Child` handle itself lives in
/// [`LIVE_CHILDREN`]; here we keep only the pid + client + counters.
struct ChildMeta {
    pid: u32,
    /// Client bound to the child's loopback port + injected key.
    client: AcpClient,
    /// Parent sessions currently routed here.
    sessions: usize,
    /// In-flight `open()` calls that have committed to this child but not yet
    /// recorded a session (they released the lock to await `create_session`).
    /// A child is reaped only when `sessions == 0 && opening == 0`, so a
    /// concurrent open can't have its just-spawned child reaped out from under it.
    opening: usize,
}

impl Drop for Inner {
    fn drop(&mut self) {
        // Fallback reap for a clean shutdown that DOES run destructors. The
        // signal path can't rely on this (std::process::exit skips Drop) — it
        // calls reap_all_children_blocking() instead.
        for (_name, meta) in self.children.drain() {
            reap_child(meta.pid);
        }
    }
}

impl CliProfileRouter {
    /// Build a router that re-invokes the current binary for each child.
    pub fn new() -> Result<Self, AcpError> {
        let exe = std::env::current_exe()
            .map_err(|e| AcpError::Transport(format!("cannot resolve current binary: {e}")))?;
        Ok(Self {
            exe,
            max_children: DEFAULT_MAX_CHILDREN,
            inner: Mutex::new(Inner::default()),
        })
    }

    /// Override the concurrent-child cap (min 1). Builder.
    pub fn with_max_children(mut self, n: usize) -> Self {
        self.max_children = n.max(1);
        self
    }

    /// Spawn a fresh `acp serve --profile <name>` child bound to a loopback
    /// ephemeral port with a per-child injected key and a CLEARED environment.
    /// Registers the child in [`LIVE_CHILDREN`] and returns its bookkeeping.
    /// Does NOT health-check or register in `Inner` — the caller does.
    fn spawn_child(&self, name: &str, dir: &Path) -> Result<ChildMeta, AcpError> {
        // Free port: bind :0, read the port, then DROP the listener before the
        // child binds it (avoids parsing the child's stderr for its port).
        let port = {
            let l = TcpListener::bind("127.0.0.1:0")
                .map_err(|e| AcpError::Transport(format!("alloc loopback port: {e}")))?;
            l.local_addr()
                .map_err(|e| AcpError::Transport(format!("read loopback port: {e}")))?
                .port()
        };
        // Per-child API key (never shared across identities): 64 hex chars.
        let key = random_hex_key();
        // Per-child append log under the temp dir, keyed by the unique port.
        let log_path = std::env::temp_dir().join(format!("wayland-acp-profile-{name}-{port}.log"));
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .map_err(|e| {
                AcpError::Transport(format!("open child log {}: {e}", log_path.display()))
            })?;
        let log2 = log
            .try_clone()
            .map_err(|e| AcpError::Transport(format!("clone child log handle: {e}")))?;

        let bind = format!("127.0.0.1:{port}");
        let mut cmd = Command::new(&self.exe);
        // CRITICAL (credential isolation): clear the environment and re-add ONLY
        // the credential-free allowlist. A `*_API_KEY` in the supervisor's own
        // env (loaded from its home's `.env` at startup, or exported in the
        // launching shell) must NOT be inherited — the child's `.env` is
        // load-if-absent, so an inherited key would silently shadow the profile's
        // own identity. Clearing makes the child's WAYLAND_HOME the sole source.
        cmd.env_clear();
        for k in ENV_PASSTHROUGH {
            if let Some(v) = std::env::var_os(k) {
                cmd.env(k, v);
            }
        }
        // Pass BOTH --profile and WAYLAND_HOME=profile_dir(name): they AGREE
        // (same dir the child derives), and the child's resolve_profile_home
        // guard refuses to start if they ever disagree — a second fail-closed line.
        cmd.args(["acp", "serve", "--profile", name, "--bind", &bind])
            .env("WAYLAND_ACP_SERVER_KEY", &key)
            .env("WAYLAND_HOME", dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log2));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt as _;
            // process_group(0) calls setsid() in the child — its own group,
            // detached from our controlling terminal.
            cmd.process_group(0);
        }
        let proc = cmd
            .spawn()
            .map_err(|e| AcpError::Transport(format!("spawn profile child {name:?}: {e}")))?;

        let base = format!("http://127.0.0.1:{port}");
        let client = AcpClient::new(base)?.with_api_key(key);
        let pid = register_child(proc);
        Ok(ChildMeta {
            pid,
            client,
            sessions: 0,
            opening: 0,
        })
    }

    /// Poll a freshly spawned child until its ACP server answers (also verifies
    /// the injected key handshake) or the budget expires. On expiry the caller
    /// reaps the child — we never leave a half-up child registered.
    async fn health_check(pid: u32, client: &AcpClient, name: &str) -> Result<(), AcpError> {
        for _ in 0..HEALTH_POLL_ATTEMPTS {
            if child_exited(pid) {
                return Err(AcpError::Transport(format!(
                    "profile child {name:?} exited during startup"
                )));
            }
            if client.list_sessions().await.is_ok() {
                return Ok(());
            }
            tokio::time::sleep(HEALTH_POLL_INTERVAL).await;
        }
        Err(AcpError::Transport(format!(
            "profile child {name:?} did not become healthy within budget"
        )))
    }
}

#[async_trait]
impl ProfileRouter for CliProfileRouter {
    async fn open(
        &self,
        session_id: &str,
        agent: &str,
        req: &SessionCreateRequest,
    ) -> Result<(), AcpError> {
        // A non-`profile:` selector reaching the router is a server-side bug;
        // fail closed (AgentNotFound) rather than guess an identity.
        let name = agent
            .strip_prefix("profile:")
            .ok_or_else(|| AcpError::Agent(format!("not a profile selector: {agent}")))?;

        // FAIL CLOSED: an invalid name or a missing home dir is AgentNotFound —
        // the child is never spawned and we never touch the default home.
        let dir = wcore_config::profile::profile_dir(name)
            .map_err(|e| AcpError::Agent(format!("agent not found: profile:{name} ({e})")))?;
        if !dir.is_dir() {
            return Err(AcpError::Agent(format!("agent not found: profile:{name}")));
        }

        // Phase 1 (LOCKED): ensure a healthy child exists and claim an in-flight
        // `open` on it. The double-spawn guard holds the lock across spawn +
        // health-check so two concurrent opens for the SAME new profile can't
        // both spawn (the second blocks, then reuses).
        let client = {
            let mut inner = self.inner.lock().await;
            if !inner.children.contains_key(name) {
                if inner.children.len() >= self.max_children {
                    return Err(AcpError::Transport(format!(
                        "profile-router child cap ({}) reached; refusing to spawn {name:?}",
                        self.max_children
                    )));
                }
                let meta = self.spawn_child(name, &dir)?;
                let pid = meta.pid;
                let client = meta.client.clone();
                if let Err(e) = Self::health_check(pid, &client, name).await {
                    reap_child(pid); // never leave a half-up child
                    return Err(e);
                }
                inner.children.insert(name.to_string(), meta);
            }
            let meta = inner
                .children
                .get_mut(name)
                .expect("child present after get-or-spawn");
            meta.opening += 1; // pin the child against reap while we create below
            meta.client.clone()
        };

        // Phase 2 (UNLOCKED): open the child session over the network. `agent:
        // None` to the child — the child IS the profile, it does not re-route.
        let created = client
            .create_session(SessionCreateRequest {
                model: req.model.clone(),
                tools: req.tools.clone(),
                system_prompt: req.system_prompt.clone(),
                agent: None,
            })
            .await;

        // Phase 3 (LOCKED): release the open pin, then record the route (success)
        // or reap the child if it is now wholly unused (failure).
        let mut inner = self.inner.lock().await;
        let child_session_id = match created {
            Ok(resp) => resp.session_id,
            Err(e) => {
                let mut reap = None;
                if let Some(meta) = inner.children.get_mut(name) {
                    meta.opening = meta.opening.saturating_sub(1);
                    if meta.sessions == 0 && meta.opening == 0 {
                        reap = Some(meta.pid);
                    }
                }
                if let Some(pid) = reap {
                    inner.children.remove(name);
                    reap_child(pid);
                }
                return Err(e);
            }
        };
        match inner.children.get_mut(name) {
            Some(meta) => {
                meta.opening = meta.opening.saturating_sub(1);
                meta.sessions += 1;
            }
            None => {
                // The child vanished between phase 1 and here — tear down the
                // orphaned child session we just opened rather than leak it.
                let _ = client.delete_session(&child_session_id).await;
                return Err(AcpError::Transport(format!(
                    "profile child {name:?} vanished during open"
                )));
            }
        }
        inner.routes.insert(
            session_id.to_string(),
            Route {
                profile: name.to_string(),
                child_session_id,
            },
        );
        Ok(())
    }

    async fn send(
        &self,
        req: MessageSendRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        // Resolve the route + clone the child client, then RELEASE the lock
        // before streaming — a long-lived turn must not hold the router mutex.
        let (client, child_session_id) = {
            let inner = self.inner.lock().await;
            let route = inner.routes.get(&req.session_id).ok_or_else(|| {
                AcpError::Session(format!("no profile route for session {}", req.session_id))
            })?;
            let child = inner.children.get(&route.profile).ok_or_else(|| {
                AcpError::Session(format!("profile child gone for session {}", req.session_id))
            })?;
            (child.client.clone(), route.child_session_id.clone())
        };

        let upstream = client
            .send_message(MessageSendRequest {
                session_id: child_session_id,
                text: req.text,
                tools: req.tools,
            })
            .await?;

        // Adapt `Result<MessageEvent, AcpError>` -> `MessageEvent`. The trait
        // requires EXACTLY ONE terminal frame (`Done`|`Error`): we pass the
        // child's frames through and FUSE right after its first terminal; a
        // mid-stream error, OR the child's stream ending WITHOUT a terminal,
        // both synthesize one terminal `Error` (never a silent drop that would
        // hang the caller waiting for completion).
        // State is `(inner_stream, done)`. `done` is set true after we yield a
        // terminal frame (the child's own, or a synthesized one), so the next
        // poll returns `None` and the adapted stream ends with exactly one
        // terminal — never a silent drop, never a re-poll of an exhausted stream.
        let adapted = stream::unfold((upstream, false), |(mut s, done)| async move {
            if done {
                return None;
            }
            match s.next().await {
                Some(Ok(ev)) => {
                    let terminal =
                        matches!(ev, MessageEvent::Done { .. } | MessageEvent::Error { .. });
                    Some((ev, (s, terminal)))
                }
                Some(Err(e)) => Some((
                    terminal_error(format!("profile child stream error: {e}")),
                    (s, true),
                )),
                None => Some((
                    terminal_error("profile child stream ended without a terminal frame".into()),
                    (s, true),
                )),
            }
        });
        Ok(Box::pin(adapted))
    }

    async fn get(&self, session_id: &str) -> Result<SessionGetResponse, AcpError> {
        let (client, child_session_id) = {
            let inner = self.inner.lock().await;
            let route = inner.routes.get(session_id).ok_or_else(|| {
                AcpError::Session(format!("no profile route for session {session_id}"))
            })?;
            let child = inner.children.get(&route.profile).ok_or_else(|| {
                AcpError::Session(format!("profile child gone for session {session_id}"))
            })?;
            (child.client.clone(), route.child_session_id.clone())
        };
        client.get_session(&child_session_id).await
    }

    async fn delete(&self, session_id: &str) -> Result<(), AcpError> {
        // Phase 1 (LOCKED): drop the route, capture the child client + target.
        let (client, child_session_id, profile) = {
            let mut inner = self.inner.lock().await;
            // Idempotent: no route ⇒ nothing to tear down (server already removed
            // its own record). Not an error.
            let Some(route) = inner.routes.remove(session_id) else {
                return Ok(());
            };
            match inner.children.get(&route.profile) {
                Some(meta) => (
                    meta.client.clone(),
                    route.child_session_id.clone(),
                    route.profile,
                ),
                None => return Ok(()), // child already gone
            }
        };

        // Phase 2 (UNLOCKED): delete the child session over the network.
        let _ = client.delete_session(&child_session_id).await;

        // Phase 3 (LOCKED): decrement and reap when this profile's child has no
        // sessions AND no in-flight opens.
        let mut inner = self.inner.lock().await;
        let mut reap = None;
        if let Some(meta) = inner.children.get_mut(&profile) {
            meta.sessions = meta.sessions.saturating_sub(1);
            if meta.sessions == 0 && meta.opening == 0 {
                reap = Some(meta.pid);
            }
        }
        if let Some(pid) = reap {
            inner.children.remove(&profile);
            reap_child(pid);
        }
        Ok(())
    }
}

/// One terminal `Error` frame for the send adapter (empty turn_id — a
/// transport-level failure has no forwarded turn context).
fn terminal_error(message: String) -> MessageEvent {
    MessageEvent::Error {
        error: JsonRpcError {
            code: ErrorCode::InternalError.code(),
            message,
            data: None,
        },
        turn_id: String::new(),
    }
}

/// 32 random bytes (two v4 UUIDs) hex-encoded to 64 chars — same shape as the
/// `acp serve` first-run key so a child accepts it as its `X-API-Key`.
fn random_hex_key() -> String {
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let mut buf = [0u8; 32];
    buf[..16].copy_from_slice(a.as_bytes());
    buf[16..].copy_from_slice(b.as_bytes());
    buf.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> SessionCreateRequest {
        SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
        }
    }

    /// FAIL CLOSED: an unknown profile (no home dir) errors as AgentNotFound and
    /// spawns NO child — never falls through to a default identity.
    #[tokio::test]
    async fn open_unknown_profile_fails_closed_no_child() {
        let router = CliProfileRouter::new().unwrap();
        let err = router
            .open(
                "sess-1",
                "profile:definitely-not-a-real-profile-xyz",
                &req(),
            )
            .await
            .expect_err("unknown profile must fail");
        assert!(matches!(err, AcpError::Agent(_)), "got {err:?}");
        assert_eq!(
            router.inner.lock().await.children.len(),
            0,
            "no child may be spawned for an unknown profile"
        );
    }

    /// A non-`profile:` selector reaching the router is rejected (server bug —
    /// fail closed rather than guess an identity).
    #[tokio::test]
    async fn open_non_profile_selector_is_rejected() {
        let router = CliProfileRouter::new().unwrap();
        let err = router
            .open("s", "persona:researcher", &req())
            .await
            .expect_err("non-profile selector must fail");
        assert!(matches!(err, AcpError::Agent(_)), "got {err:?}");
    }

    /// Sending to an unmapped session errors (no silent default routing).
    #[tokio::test]
    async fn send_unknown_session_errors() {
        let router = CliProfileRouter::new().unwrap();
        // The Ok arm is a boxed `dyn Stream` (no Debug), so match rather than
        // `expect_err` (which would need Debug on the Ok type).
        match router
            .send(MessageSendRequest {
                session_id: "nope".into(),
                text: "hi".into(),
                tools: Vec::new(),
            })
            .await
        {
            Err(AcpError::Session(_)) => {}
            Err(other) => panic!("expected a Session error, got {other:?}"),
            Ok(_) => panic!("expected an error for an unknown session, got a stream"),
        }
    }

    /// Deleting an unmapped session is a no-op success (idempotent teardown).
    #[tokio::test]
    async fn delete_unknown_session_is_ok() {
        let router = CliProfileRouter::new().unwrap();
        assert!(router.delete("nope").await.is_ok());
    }

    /// Two distinct keys never collide and are the right shape (64 lc hex).
    #[test]
    fn random_hex_key_is_64_hex_and_unique() {
        let a = random_hex_key();
        let b = random_hex_key();
        assert_eq!(a.len(), 64);
        assert!(
            a.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_ne!(a, b);
    }

    /// The env allowlist must never carry a credential-shaped variable — that
    /// would reintroduce the cross-identity bleed `env_clear` exists to close.
    #[test]
    fn env_passthrough_carries_no_credentials() {
        for k in ENV_PASSTHROUGH {
            let up = k.to_ascii_uppercase();
            assert!(
                !up.contains("API_KEY")
                    && !up.contains("TOKEN")
                    && !up.contains("SECRET")
                    && !up.contains("PASSWORD"),
                "credential-shaped var {k:?} must not be in the passthrough allowlist"
            );
        }
    }
}
