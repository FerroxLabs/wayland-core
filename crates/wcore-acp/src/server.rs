//! Minimal ACP server backing the [`HttpHandler`] trait.
//!
//! In-memory session storage. Session create/list/get/delete are real and
//! round-trip. `message/send` drives a real turn through an injected
//! [`crate::turn::TurnEngine`] (installed from the CLI layer via
//! [`AcpServer::with_turn_engine`], keeping this mid-layer crate engine-free).
//! When no engine is installed, `send_message` returns a one-event stream
//! carrying an honest `Error { "no turn engine installed" }` frame rather than
//! a misleading empty `Done`.
//!
//! `HttpHandler` is implemented on [`AcpServer`] so the same server instance
//! plugs into [`crate::transport::HttpSseTransport`] (and, once wired, the
//! stdio/WS transports too).

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use futures::stream::{self, Stream, StreamExt};
use tokio::sync::RwLock;

use crate::auth::Principal;
use crate::cursor::{Cursor, EventLog, ResumeError, ResumeResponse};
use crate::error::AcpError;
use crate::idempotency::{CommandLedger, LedgerOutcome};
use crate::protocol::{
    ACP_PROTOCOL_VERSION, AgentsListResponse, ErrorCode, InitializeResponse, JsonRpcError,
    MessageEvent, MessageSendRequest, ServerCapabilities, SessionCreateRequest,
    SessionCreateResponse, SessionGetResponse, SessionListResponse, SessionMetadata,
    ToolDefinition,
};
use crate::roles::RolePolicy;
use crate::roster::AgentRoster;
use crate::transport::HttpHandler;

/// What an idempotency identity is bound to.
///
/// A canonical serialization of the method and its parameters, NOT the
/// parameters alone: `session/delete` of `s1` and a hypothetical
/// `session/archive` of `s1` must never look like the same command merely
/// because their payloads coincide.
type CommandFingerprint = String;

/// The receipt replayed when an idempotency identity repeats.
///
/// Held as the already-formed response rather than as inputs to re-derive it,
/// so a replay cannot produce a DIFFERENT answer from the one the first caller
/// acted on — re-deriving would mint a fresh session id on every replay, which
/// is the exact failure idempotency exists to prevent.
#[derive(Debug, Clone)]
pub(crate) enum CommandReceipt {
    SessionCreated(SessionCreateResponse),
    SessionDeleted,
}

/// Internal session record. Wraps [`SessionMetadata`] with the create-time
/// configuration a real turn must honour.
///
/// `system_prompt` and `tools` were previously dropped from
/// [`SessionCreateRequest`]; storing them here lets the injected
/// [`crate::turn::TurnEngine`] read the session's configured allowlist when a
/// per-message request omits its own.
#[derive(Debug, Clone)]
struct SessionRecord {
    metadata: SessionMetadata,
    /// Per-session system-prompt override supplied at create-time. Stored so
    /// it is not silently dropped; applying it to the engine build is a
    /// documented follow-up (the engine's configured prompt is the default).
    #[allow(dead_code)]
    system_prompt: Option<String>,
    /// The session's configured tool allowlist. Used as the fallback when a
    /// `message/send` request body carries no per-call tools.
    tools: Vec<ToolDefinition>,
    /// persona-profiles Phase A: the AUTHORIZED persona-agent id this session
    /// was created with, if any. Recorded (and readable via
    /// [`AcpServer::session_agent`]) so a later per-session persona binding
    /// (PR-4) can resolve the overlay. In PR-2 it is stored + validated only —
    /// no persona overlay is applied to the engine yet, so selecting an agent
    /// does NOT change turn behaviour or cross any credential boundary.
    agent: Option<String>,
    /// #998 — the per-tool MCP switches supplied at `session/create`.
    ///
    /// Stored on the record for the same reason `agent` is: it is bound at
    /// create and read from HERE on every turn, so a per-message body can
    /// neither introduce a selection the session was not created with nor widen
    /// one it was. Empty = no selection.
    mcp_servers: Vec<crate::protocol::McpToolSelection>,
}

/// Minimal ACP server with in-memory session storage.
///
/// All session state is held in an `Arc<RwLock<HashMap<_, _>>>`; the
/// server is `Clone`-friendly via the inner `Arc`. Construct one and
/// hand it to [`HttpSseTransport::new`] (and friends) to wire the wire
/// transports to the same backing state.
#[derive(Clone)]
pub struct AcpServer {
    /// Identity of THIS process's run, minted once at construction and mixed
    /// into every stream id. A restarted server therefore issues stream ids no
    /// pre-restart cursor can match, so a stale cursor gets a named
    /// `StreamMismatch` instead of being silently served positions of a
    /// different stream — the failure `cursor.rs` was built to make impossible,
    /// which only becomes impossible once something actually mints the id.
    instance_id: String,
    sessions: Arc<RwLock<HashMap<String, SessionRecord>>>,
    /// Per-session ordered event log. Every event `message/send` emits is
    /// appended here as it leaves the engine, INDEPENDENTLY of whether the
    /// client that asked for it is still connected — a client that disconnects
    /// mid-turn is precisely the client that needs to resume, so logging only
    /// what was successfully delivered would retain everything except the
    /// events that matter.
    events: Arc<RwLock<HashMap<String, EventLog<MessageEvent>>>>,
    /// Retained events per session stream. See [`Self::with_event_retention`].
    event_retention: usize,
    /// Bounded ledger backing the `Idempotency-Key` header on the mutating
    /// session commands.
    commands: Arc<RwLock<CommandLedger<CommandFingerprint, CommandReceipt>>>,
    /// Server-side role assignment. `None` means role gating is NOT configured
    /// — every authenticated principal reaches every method, which is the
    /// pre-role behaviour. Reported by [`Self::has_role_policy`] so an operator
    /// surface can state it rather than implying enforcement that is not
    /// happening.
    role_policy: Option<Arc<RolePolicy>>,
    /// v0.8.1 U12 — optional A2A handler. When `Some`, the server
    /// dispatches `a2a/*` methods to it. When `None`, those methods
    /// return a "no handler installed" protocol error (the typed
    /// equivalent of JSON-RPC -32601 "Method not found").
    a2a_handler: Option<Arc<dyn crate::a2a::A2aHandler>>,
    /// Engine bridge for `message/send`. When `Some`, `send_message` drives
    /// a real turn through it; when `None`, it returns an honest `Error`
    /// frame ("no turn engine installed"). Injected from the CLI layer
    /// exactly like `a2a_handler` so `wcore-acp` stays engine-free.
    turn_engine: Option<Arc<dyn crate::turn::TurnEngine>>,
    /// persona-profiles Phase A — optional persona-agent roster. When `Some`,
    /// `agents/list` returns the authorized catalog and a `session/create`
    /// `agent` selector is validated against it. When `None` (the default,
    /// feature-OFF), `agents/list` is `[]` and any selector is
    /// `AgentNotFound` — byte-identical to the pre-extension server for
    /// selector-free clients. Injected from the CLI layer (PR-3's
    /// `CliAgentRoster`) exactly like `a2a_handler`, keeping `wcore-acp`
    /// dependency-free.
    roster: Option<Arc<dyn AgentRoster>>,
    /// persona-profiles PR-7 — optional profile SUPERVISOR/ROUTER. When `Some`,
    /// a session whose create-time `agent` is a `profile:<name>` selector is
    /// routed to a per-profile child process (one process per profile — its own
    /// `WAYLAND_HOME`/keys/memory) instead of the in-process `turn_engine`. When
    /// `None` (the default, feature-OFF), the roster enumerates no profiles, so
    /// `session/create` never authorizes one and nothing reaches this path —
    /// byte-identical to the pre-extension server. Injected from the CLI layer
    /// (where process spawn + the ACP client pool live), keeping `wcore-acp`
    /// process-machinery-free.
    router: Option<Arc<dyn crate::router::ProfileRouter>>,
}

impl std::fmt::Debug for AcpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpServer")
            .field("sessions", &self.sessions)
            .field(
                "a2a_handler",
                &self.a2a_handler.as_ref().map(|_| "<dyn A2aHandler>"),
            )
            .field(
                "turn_engine",
                &self.turn_engine.as_ref().map(|_| "<dyn TurnEngine>"),
            )
            .field("roster", &self.roster.as_ref().map(|_| "<dyn AgentRoster>"))
            .field(
                "router",
                &self.router.as_ref().map(|_| "<dyn ProfileRouter>"),
            )
            .finish()
    }
}

impl Default for AcpServer {
    fn default() -> Self {
        Self::new()
    }
}

impl AcpServer {
    /// Construct an empty server.
    ///
    /// NOT `Self::default()` any more: the instance identity must be minted
    /// here, and a derived `Default` would hand every server the empty string
    /// — making every restart look like the same stream and re-opening exactly
    /// the silent-resume hole the cursor contract closes.
    pub fn new() -> Self {
        Self {
            instance_id: uuid::Uuid::new_v4().to_string(),
            sessions: Arc::new(RwLock::new(HashMap::new())),
            events: Arc::new(RwLock::new(HashMap::new())),
            event_retention: crate::cursor::DEFAULT_RETENTION,
            commands: Arc::new(RwLock::new(CommandLedger::new())),
            role_policy: None,
            a2a_handler: None,
            turn_engine: None,
            roster: None,
            router: None,
        }
    }

    /// This process's run identity — the suffix of every stream id it mints.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    /// How many events each session's stream retains for resumption.
    ///
    /// Retained history is memory a DISCONNECTED client makes the server hold
    /// (T-24-03-05), so it is finite and the bound is the operator's to choose.
    /// Lowering it does not lose events silently: a cursor that falls outside
    /// the window gets [`crate::cursor::CursorError::TooOld`] naming the oldest
    /// position still servable, so the client resynchronises deliberately.
    pub fn with_event_retention(mut self, events: usize) -> Self {
        self.event_retention = events.max(1);
        self
    }

    /// The stream identity `session_id` has in THIS process run.
    ///
    /// Public because a client cannot verify the resume contract without it:
    /// the property that matters is that a DIFFERENT run of this server names
    /// the same session's stream differently, and that is only assertable if
    /// the naming is observable. See
    /// `tests/typed_client_recovery.rs::a_cursor_from_another_stream_is_refused…`,
    /// which mints its stale cursor from a second server instance rather than
    /// from a hand-written string — a hand-written foreign id is refused even
    /// by a server whose stream ids carry no run identity at all, so it proves
    /// the CHECK and not the IDENTITY.
    pub fn stream_id_for(&self, session_id: &str) -> String {
        format!("{session_id}@{}", self.instance_id)
    }

    /// Install the server-side role policy. When present, every request the
    /// transport routes through [`HttpHandler::authorize_method`] is decided
    /// against it before dispatch. When absent, no role gating happens —
    /// see [`Self::has_role_policy`].
    pub fn with_role_policy(mut self, policy: RolePolicy) -> Self {
        self.role_policy = Some(Arc::new(policy));
        self
    }

    /// Whether a role policy is installed.
    ///
    /// `false` does not mean "everything is denied" and does not mean
    /// "everything is fine". It means role gating is NOT CONFIGURED, and any
    /// surface reporting on authorization must say that rather than printing a
    /// zero-refusals number that reads like enforcement.
    pub fn has_role_policy(&self) -> bool {
        self.role_policy.is_some()
    }

    /// Current session count — useful for tests + observability.
    pub async fn session_count(&self) -> usize {
        self.sessions.read().await.len()
    }

    /// v0.8.1 U12 — install an A2A handler. When present, the server
    /// dispatches `a2a/*` methods to it. When absent, those methods
    /// return a `Protocol("no handler installed")` error (the typed
    /// equivalent of JSON-RPC -32601 "Method not found").
    pub fn with_a2a_handler(mut self, handler: Arc<dyn crate::a2a::A2aHandler>) -> Self {
        self.a2a_handler = Some(handler);
        self
    }

    /// Whether an A2A handler is installed.
    pub fn has_a2a_handler(&self) -> bool {
        self.a2a_handler.is_some()
    }

    /// Install the engine bridge used by `message/send`. When present,
    /// `send_message` drives a real turn through it; when absent, it returns
    /// an honest `Error` frame. Mirrors [`Self::with_a2a_handler`].
    pub fn with_turn_engine(mut self, engine: Arc<dyn crate::turn::TurnEngine>) -> Self {
        self.turn_engine = Some(engine);
        self
    }

    /// Whether a turn engine is installed.
    pub fn has_turn_engine(&self) -> bool {
        self.turn_engine.is_some()
    }

    /// The configured tool allowlist for `session_id`, if the session exists.
    /// The engine bridge reads this when a `message/send` body omits tools.
    pub async fn session_tools(&self, session_id: &str) -> Option<Vec<ToolDefinition>> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .map(|r| r.tools.clone())
    }

    /// persona-profiles Phase A — install a persona-agent roster. When present,
    /// `agents/list` returns its authorized catalog and a `session/create`
    /// `agent` selector is validated against it (`AgentNotFound` on miss).
    /// When absent, the server is backward-compatible: empty roster, and any
    /// selector is rejected. Mirrors [`Self::with_a2a_handler`] /
    /// [`Self::with_turn_engine`].
    pub fn with_roster(mut self, roster: Arc<dyn AgentRoster>) -> Self {
        self.roster = Some(roster);
        self
    }

    /// Whether a persona-agent roster is installed.
    pub fn has_roster(&self) -> bool {
        self.roster.is_some()
    }

    /// persona-profiles PR-7 — install the profile supervisor/router. When
    /// present, a session created with a `profile:<name>` agent is routed to a
    /// per-profile child process instead of the in-process turn engine. When
    /// absent, no profile is enumerable/authorizable, so nothing reaches the
    /// router. Mirrors [`Self::with_turn_engine`] / [`Self::with_roster`].
    pub fn with_profile_router(mut self, router: Arc<dyn crate::router::ProfileRouter>) -> Self {
        self.router = Some(router);
        self
    }

    /// Whether a profile supervisor/router is installed.
    pub fn has_profile_router(&self) -> bool {
        self.router.is_some()
    }

    /// persona-profiles PR-7 — whether an agent selector names a per-PROFILE
    /// agent (routed to its own child process) rather than an in-process
    /// persona overlay. `profile:` is the wire prefix of a profile-agent id.
    fn is_profile_agent(agent: Option<&str>) -> bool {
        agent.is_some_and(|a| a.starts_with("profile:"))
    }

    /// The AUTHORIZED persona-agent id bound to `session_id` at create-time, if
    /// the session exists and selected one. Parallels [`Self::session_tools`].
    /// A later per-session persona binding (PR-4) reads this to resolve the
    /// overlay; in PR-2 it is a read-only record of the validated selector.
    /// #998 — the per-tool MCP switches bound to `session_id` at create-time.
    ///
    /// Empty for a session that made no selection, and for an unknown session:
    /// "no narrowing" is the only safe reading of "no record", and an unknown
    /// session is refused by the caller before a turn can run anyway.
    pub async fn session_mcp_servers(&self, session_id: &str) -> Vec<crate::protocol::McpToolSelection> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .map(|r| r.mcp_servers.clone())
            .unwrap_or_default()
    }

    pub async fn session_agent(&self, session_id: &str) -> Option<String> {
        self.sessions
            .read()
            .await
            .get(session_id)
            .and_then(|r| r.agent.clone())
    }

    /// v0.8.1 U12 — dispatch `a2a/handshake`.
    pub async fn a2a_handshake(
        &self,
        h: crate::a2a::A2aHandshake,
    ) -> Result<crate::a2a::A2aHandshake, AcpError> {
        let handler = self
            .a2a_handler
            .as_ref()
            .ok_or_else(|| AcpError::Protocol("a2a/handshake: no handler installed".to_string()))?;
        handler
            .on_handshake(h)
            .await
            .map_err(|e| AcpError::Protocol(e.to_string()))
    }

    /// v0.8.1 U12 — dispatch `a2a/message/send`.
    pub async fn a2a_message_send(
        &self,
        m: crate::a2a::A2aMessage,
    ) -> Result<crate::a2a::A2aMessage, AcpError> {
        let handler = self.a2a_handler.as_ref().ok_or_else(|| {
            AcpError::Protocol("a2a/message/send: no handler installed".to_string())
        })?;
        handler
            .on_message(m)
            .await
            .map_err(|e| AcpError::Protocol(e.to_string()))
    }

    /// v0.8.1 U12 — dispatch `a2a/capabilities`.
    pub async fn a2a_capabilities(&self) -> Result<crate::a2a::A2aCapabilities, AcpError> {
        let handler = self.a2a_handler.as_ref().ok_or_else(|| {
            AcpError::Protocol("a2a/capabilities: no handler installed".to_string())
        })?;
        handler
            .capabilities()
            .await
            .map_err(|e| AcpError::Protocol(e.to_string()))
    }

    // ── The event plane: what makes a cursor resumable in practice ─────────

    /// Drain `upstream` to completion in its own task, appending every event to
    /// the session's log, and hand the caller a stream fed from that drain.
    ///
    /// The tee is the whole point. Returning the engine's stream directly makes
    /// the log a function of what the CLIENT consumed: axum drops the response
    /// stream the moment the peer disconnects, the engine's remaining events
    /// are never polled, and they are never recorded. The client then resumes
    /// and is told — correctly, and uselessly — that there is nothing to
    /// resume. Draining in a task decouples recording from delivery, so a
    /// severed connection loses the DELIVERY and keeps the RECORD.
    fn tee_into_log(
        &self,
        session_id: &str,
        upstream: Pin<Box<dyn Stream<Item = MessageEvent> + Send>>,
    ) -> Pin<Box<dyn Stream<Item = MessageEvent> + Send>> {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<MessageEvent>();
        let events = Arc::clone(&self.events);
        let session_id = session_id.to_string();
        tokio::spawn(async move {
            let mut upstream = upstream;
            while let Some(ev) = upstream.next().await {
                {
                    let mut guard = events.write().await;
                    if let Some(log) = guard.get_mut(&session_id) {
                        log.append(ev.clone());
                    }
                }
                // A send error means the client is gone. That is not a reason
                // to stop draining: the events after the disconnection are the
                // ones the resume exists to deliver. The channel drops what it
                // is holding when the receiver goes, so nothing accumulates.
                let _ = tx.send(ev);
            }
        });
        Box::pin(futures::stream::unfold(rx, |mut rx| async move {
            rx.recv().await.map(|ev| (ev, rx))
        }))
    }

    /// The tip cursor for a session's stream — what a live subscriber holds.
    pub async fn event_tip(&self, session_id: &str) -> Option<Cursor> {
        self.events.read().await.get(session_id).map(|l| l.tip())
    }

    /// Serve a resume: everything the cursor has not seen, or a NAMED refusal.
    ///
    /// A session this server does not hold is [`ResumeError::NoSuchSession`],
    /// never an empty list. "I have nothing for you" and "I have never heard
    /// of you" are different answers and a client acts differently on each.
    pub async fn events_since(
        &self,
        session_id: &str,
        cursor: &Cursor,
    ) -> Result<ResumeResponse, ResumeError> {
        let guard = self.events.read().await;
        let Some(log) = guard.get(session_id) else {
            return Err(ResumeError::NoSuchSession {
                session_id: session_id.to_string(),
            });
        };
        let events = log.since(cursor).map_err(ResumeError::Cursor)?;
        Ok(ResumeResponse {
            stream_id: log.stream_id().to_string(),
            next_position: log.next_position(),
            oldest_available: log.oldest_available(),
            events,
        })
    }

    // ── Command idempotency on the request path ───────────────────────────

    /// Classify a command identity, returning a receipt to replay when the
    /// identity has been used before with the same command.
    async fn classify_command(
        &self,
        identity: &str,
        fingerprint: &CommandFingerprint,
    ) -> Result<Option<CommandReceipt>, AcpError> {
        match self.commands.read().await.classify(identity, fingerprint) {
            LedgerOutcome::Fresh => Ok(None),
            LedgerOutcome::Replay(receipt) => Ok(Some(receipt)),
            LedgerOutcome::Conflict => Err(AcpError::Protocol(format!(
                "idempotency key {identity:?} is already bound to a different command; \
                 reusing it would either perform a second effect or return another \
                 caller's receipt"
            ))),
            LedgerOutcome::InvalidIdentity => Err(AcpError::Protocol(
                "idempotency key is empty or longer than the accepted bound".to_string(),
            )),
            LedgerOutcome::Full => Err(AcpError::Protocol(
                "the idempotency ledger is at capacity; this command is REFUSED rather \
                 than admitted by discarding an older exactly-once guarantee"
                    .to_string(),
            )),
        }
    }

    async fn record_command(
        &self,
        identity: &str,
        fingerprint: &CommandFingerprint,
        receipt: &CommandReceipt,
    ) {
        self.commands
            .write()
            .await
            .record(identity, fingerprint, receipt);
    }
}

/// Canonical fingerprint of a command: its method name plus a serialization of
/// its parameters.
///
/// The method name is part of the fingerprint on purpose. Without it two
/// different commands whose payloads happen to serialize identically would be
/// indistinguishable to the ledger, and the second would be answered with the
/// first one's receipt.
fn fingerprint_of<T: serde::Serialize>(
    method: &str,
    params: &T,
) -> Result<CommandFingerprint, AcpError> {
    let body = serde_json::to_string(params)?;
    Ok(format!("{method}\u{1e}{body}"))
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[async_trait]
impl HttpHandler for AcpServer {
    async fn create_session(
        &self,
        req: SessionCreateRequest,
    ) -> Result<SessionCreateResponse, AcpError> {
        // persona-profiles Phase A (R3): if the client selected a persona-agent,
        // AUTHORIZE it against the installed roster BEFORE creating the session.
        // The roster returns only agents the principal may use, so a miss (or no
        // roster at all) is `AgentNotFound` — which doubles as "not authorized"
        // without leaking existence. A selector-free create is untouched, keeping
        // the pre-extension wire byte-identical (compat regression proof).
        //
        // NOTE this only VALIDATES + RECORDS the id; it applies NO persona
        // overlay to the engine (system_prompt/model/tools stay as configured).
        // Binding the persona is PR-4 — deliberately not done here.
        if let Some(agent_id) = req.agent.as_deref() {
            let authorized = match &self.roster {
                Some(roster) => roster.contains(agent_id).await,
                None => false,
            };
            if !authorized {
                return Err(AcpError::Agent(format!("agent not found: {agent_id}")));
            }
        }

        let id = uuid::Uuid::new_v4().to_string();
        let now = now_secs();
        let metadata = SessionMetadata {
            session_id: id.clone(),
            model: req.model.clone(),
            created_at: now,
            last_activity: now,
            message_count: 0,
        };
        let record = SessionRecord {
            metadata: metadata.clone(),
            system_prompt: req.system_prompt.clone(),
            tools: req.tools.clone(),
            agent: req.agent.clone(),
            mcp_servers: req.mcp_servers.clone(),
        };
        self.sessions.write().await.insert(id.clone(), record);
        // Open the session's event stream at create, not lazily at first send.
        // Lazily would mean a resume issued between create and the first
        // message could not tell "no events yet" from "no such session".
        self.events.write().await.insert(
            id.clone(),
            EventLog::with_capacity(self.stream_id_for(&id), self.event_retention),
        );

        // persona-profiles PR-7: a `profile:<name>` agent routes to a per-PROFILE
        // CHILD process (its own WAYLAND_HOME/identity). Spawn/open it now and map
        // this parent session to the child. FAIL CLOSED — on any error discard the
        // just-inserted parent record, so a failed bind never leaves a session
        // that could later fall through to this process's default identity.
        if Self::is_profile_agent(req.agent.as_deref()) {
            let agent_id = req.agent.as_deref().unwrap_or_default();
            let opened = match &self.router {
                Some(router) => router.open(&id, agent_id, &req).await,
                // Authorized as a profile agent but no supervisor is installed —
                // a misconfiguration. Fail closed rather than silently serving
                // the default identity under the profile's name.
                None => Err(AcpError::Agent(format!("agent not found: {agent_id}"))),
            };
            if let Err(e) = opened {
                self.sessions.write().await.remove(&id);
                self.events.write().await.remove(&id);
                return Err(e);
            }
        }

        Ok(SessionCreateResponse {
            session_id: id,
            model: req.model,
        })
    }

    async fn list_agents(&self) -> Result<AgentsListResponse, AcpError> {
        // Feature default-OFF: no roster installed ⇒ empty catalog (`[]`),
        // backward-compatible. When installed, the roster returns ONLY the
        // agents the calling principal is authorized to see (R3), each exposing
        // just id/label/description (R4).
        match &self.roster {
            Some(roster) => Ok(AgentsListResponse {
                agents: roster.list().await?,
            }),
            None => Ok(AgentsListResponse { agents: Vec::new() }),
        }
    }

    async fn initialize(&self) -> Result<InitializeResponse, AcpError> {
        // Capability handshake (R2): advertise `agent_selection` so a client
        // knows THIS build understands the optional `agent` selector +
        // `agents/list` before it risks sending version-gated fields to a
        // possibly-older peer. This is a compile-time property of the server
        // (always `true` here) — it is advertised even when no roster is
        // installed, and grants nothing: `agents/list` is still `[]` and any
        // selector still yields `AgentNotFound`.
        Ok(InitializeResponse {
            protocol_version: ACP_PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities {
                agent_selection: true,
                // #998: a compile-time property of this build, exactly like
                // `agent_selection`. It tells a client the `mcp_servers` key
                // will be parsed and applied rather than hard-rejected; it
                // grants nothing, since a selection can only narrow.
                mcp_tool_selection: true,
            },
        })
    }

    async fn list_sessions(&self) -> Result<SessionListResponse, AcpError> {
        let guard = self.sessions.read().await;
        let mut sessions: Vec<SessionMetadata> =
            guard.values().map(|r| r.metadata.clone()).collect();
        // Stable order: newest first by created_at.
        sessions.sort_by_key(|s| std::cmp::Reverse(s.created_at));
        Ok(SessionListResponse { sessions })
    }

    async fn get_session(&self, session_id: String) -> Result<SessionGetResponse, AcpError> {
        let guard = self.sessions.read().await;
        match guard.get(&session_id) {
            Some(record) => Ok(SessionGetResponse {
                session: record.metadata.clone(),
            }),
            None => Err(AcpError::Session(format!(
                "session not found: {session_id}"
            ))),
        }
    }

    async fn delete_session(&self, session_id: String) -> Result<(), AcpError> {
        let removed = self.sessions.write().await.remove(&session_id);
        let Some(record) = removed else {
            return Err(AcpError::Session(format!(
                "session not found: {session_id}"
            )));
        };
        self.events.write().await.remove(&session_id);
        // persona-profiles PR-7: reap the per-profile child session mapped to
        // this session (the router tears the child process down when its last
        // session goes away). Non-profile sessions have no child — nothing to do.
        if Self::is_profile_agent(record.agent.as_deref())
            && let Some(router) = &self.router
        {
            router.delete(&session_id).await?;
        }
        Ok(())
    }

    async fn send_message(
        &self,
        req: MessageSendRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
        // Verify session exists + bump activity.
        {
            let mut guard = self.sessions.write().await;
            let Some(record) = guard.get_mut(&req.session_id) else {
                return Err(AcpError::Session(format!(
                    "session not found: {}",
                    req.session_id
                )));
            };
            record.metadata.last_activity = now_secs();
            record.metadata.message_count = record.metadata.message_count.saturating_add(1);
        }

        // Per-call tools override the session allowlist; an empty body falls
        // back to the tools stored at create-time.
        let tools = if req.tools.is_empty() {
            self.session_tools(&req.session_id)
                .await
                .unwrap_or_default()
        } else {
            req.tools
        };

        // persona-profiles PR-4': carry the session's AUTHORIZED persona-agent id
        // into the turn so the engine bridge can apply that persona's overlay.
        // Read from the session record (NOT from the request body) — a per-message
        // body can never smuggle in a persona that was not authorized at create.
        let agent = self.session_agent(&req.session_id).await;

        // #998: likewise read the session's per-tool MCP switches from the
        // RECORD. A per-message body carries no MCP field at all, so there is
        // no path by which a later message can widen what `session/create`
        // narrowed.
        let mcp_servers = self.session_mcp_servers(&req.session_id).await;

        // persona-profiles PR-7: a `profile:<name>` session is served by its own
        // child process — forward the message to that child instead of the
        // in-process turn engine. The persona/tools overlay is the CHILD's
        // concern (it runs under the profile's own home/config).
        // F24-04: BOTH dispatch paths converge on one `upstream` and one tee, so
        // there is no branch through which events can reach a client without
        // reaching the event log. An early `return` in either arm would be a
        // path whose events are unresumable, and it would look identical from
        // the outside until somebody disconnected and counted.
        let session_id = req.session_id.clone();
        let upstream: Pin<Box<dyn Stream<Item = MessageEvent> + Send>> =
            if Self::is_profile_agent(agent.as_deref()) {
                match &self.router {
                    Some(router) => {
                        router
                            .send(MessageSendRequest {
                                session_id: req.session_id,
                                text: req.text,
                                tools,
                            })
                            .await?
                    }
                    None => {
                        return Err(AcpError::Session(format!(
                            "session {} is bound to a profile agent but no supervisor is \
                             installed",
                            req.session_id
                        )));
                    }
                }
            } else {
                match &self.turn_engine {
                    Some(engine) => {
                        engine
                            .run_turn(crate::turn::TurnRequest {
                                session_id: req.session_id,
                                text: req.text,
                                tools,
                                agent,
                                mcp_servers,
                            })
                            .await?
                    }
                    None => {
                        // No engine installed: emit a typed, honest signal rather
                        // than a misleading `Done{not_implemented}` (which is not a
                        // valid StopReason and looks like a successful empty turn).
                        let ev = MessageEvent::Error {
                            error: JsonRpcError {
                                code: ErrorCode::InternalError.code(),
                                message: "no turn engine installed".to_string(),
                                data: None,
                            },
                            // #787: a server-level frame with no turn context — there is
                            // no per-turn id to stamp (no engine ran).
                            turn_id: String::new(),
                        };
                        stream::iter(vec![ev]).boxed()
                    }
                }
            };
        Ok(self.tee_into_log(&session_id, upstream))
    }

    /// The server's authorization decision, taken from the principal the
    /// TRANSPORT verified and the method the transport resolved from the route.
    ///
    /// With no policy installed this is `Ok` for everything — the pre-role
    /// behaviour, kept deliberately so that shipping roles cannot lock an
    /// operator out of a gateway they configured before roles existed.
    /// [`Self::has_role_policy`] is how a caller learns which of the two states
    /// it is in; a bare `Ok` must never be read as "the role check passed".
    async fn authorize_method(&self, principal: &Principal, method: &str) -> Result<(), AcpError> {
        match &self.role_policy {
            Some(policy) => policy.authorize(principal, method),
            None => Ok(()),
        }
    }

    async fn create_session_idempotent(
        &self,
        key: Option<&str>,
        req: SessionCreateRequest,
    ) -> Result<SessionCreateResponse, AcpError> {
        let Some(key) = key else {
            return self.create_session(req).await;
        };
        let fingerprint = fingerprint_of("session/create", &req)?;
        if let Some(CommandReceipt::SessionCreated(resp)) =
            self.classify_command(key, &fingerprint).await?
        {
            return Ok(resp);
        }
        let resp = self.create_session(req).await?;
        self.record_command(
            key,
            &fingerprint,
            &CommandReceipt::SessionCreated(resp.clone()),
        )
        .await;
        Ok(resp)
    }

    async fn delete_session_idempotent(
        &self,
        key: Option<&str>,
        session_id: String,
    ) -> Result<(), AcpError> {
        let Some(key) = key else {
            return self.delete_session(session_id).await;
        };
        let fingerprint = fingerprint_of("session/delete", &session_id)?;
        if let Some(CommandReceipt::SessionDeleted) =
            self.classify_command(key, &fingerprint).await?
        {
            // The delete already happened. Re-issuing it would now report
            // "session not found" — turning a successful retry into a spurious
            // failure, which is the precise reason the caller sent a key.
            return Ok(());
        }
        self.delete_session(session_id).await?;
        self.record_command(key, &fingerprint, &CommandReceipt::SessionDeleted)
            .await;
        Ok(())
    }

    async fn resume_events(
        &self,
        session_id: String,
        cursor: Cursor,
    ) -> Result<ResumeResponse, ResumeError> {
        self.events_since(&session_id, &cursor).await
    }

    async fn resolve_approval(
        &self,
        session_id: String,
        call_id: String,
        decision: crate::turn::ApprovalDecision,
    ) -> Result<(), AcpError> {
        // The pending-approval state lives in the engine's per-session
        // approval manager (the `AcpServer` record map only tracks metadata),
        // so resolution delegates straight to the installed `TurnEngine` —
        // the same engine that emitted the `ApprovalRequired` gate. Mirrors
        // the `send_message` "no engine installed" arm.
        match &self.turn_engine {
            Some(engine) => {
                engine
                    .resolve_approval(&session_id, &call_id, decision)
                    .await
            }
            None => Err(AcpError::Protocol("no turn engine installed".to_string())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::turn::{TurnEngine, TurnRequest};

    /// A `TurnEngine` that records the [`TurnRequest`] it received and
    /// replays a fixed event script. Lets server tests assert that
    /// `send_message` proxies a stream verbatim and forwards the right tools.
    struct MockTurnEngine {
        script: Vec<MessageEvent>,
        last_req: std::sync::Mutex<Option<TurnRequest>>,
    }

    impl MockTurnEngine {
        fn new(script: Vec<MessageEvent>) -> Self {
            Self {
                script,
                last_req: std::sync::Mutex::new(None),
            }
        }
    }

    #[async_trait]
    impl TurnEngine for MockTurnEngine {
        async fn run_turn(
            &self,
            req: TurnRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
            *self.last_req.lock().unwrap() = Some(req);
            Ok(stream::iter(self.script.clone()).boxed())
        }
    }

    fn empty_create() -> SessionCreateRequest {
        SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: None,
            mcp_servers: Vec::new(),
        }
    }

    #[tokio::test]
    async fn create_then_get_roundtrips() {
        let server = AcpServer::new();
        let resp = server
            .create_session(SessionCreateRequest {
                model: Some("claude-opus-4-7".to_string()),
                tools: Vec::new(),
                system_prompt: None,
                agent: None,
                mcp_servers: Vec::new(),
            })
            .await
            .unwrap();
        assert!(!resp.session_id.is_empty());
        assert_eq!(resp.model.as_deref(), Some("claude-opus-4-7"));

        let got = server.get_session(resp.session_id.clone()).await.unwrap();
        assert_eq!(got.session.session_id, resp.session_id);
        assert_eq!(got.session.message_count, 0);
    }

    #[tokio::test]
    async fn list_returns_newest_first() {
        let server = AcpServer::new();
        let a = server.create_session(empty_create()).await.unwrap();
        // Force a different created_at by sleeping 1s — coarse but
        // matches the 1-second resolution of `now_secs`.
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let b = server.create_session(empty_create()).await.unwrap();

        let list = server.list_sessions().await.unwrap();
        assert_eq!(list.sessions.len(), 2);
        // Newest first.
        assert_eq!(list.sessions[0].session_id, b.session_id);
        assert_eq!(list.sessions[1].session_id, a.session_id);
    }

    #[tokio::test]
    async fn delete_then_get_errors() {
        let server = AcpServer::new();
        let resp = server.create_session(empty_create()).await.unwrap();
        server
            .delete_session(resp.session_id.clone())
            .await
            .unwrap();
        let err = server
            .get_session(resp.session_id.clone())
            .await
            .expect_err("expected session-not-found");
        assert!(matches!(err, AcpError::Session(_)));
    }

    #[tokio::test]
    async fn delete_missing_errors() {
        let server = AcpServer::new();
        let err = server
            .delete_session("nope".to_string())
            .await
            .expect_err("expected session-not-found");
        assert!(matches!(err, AcpError::Session(_)));
    }

    // T-A2: with NO engine installed, `send_message` yields exactly one
    // honest `Error{message:"no turn engine installed"}` frame (replacing the
    // old misleading `Done{not_implemented}`), and still bumps activity.
    #[tokio::test]
    async fn send_message_without_engine_returns_error_event() {
        let server = AcpServer::new();
        assert!(!server.has_turn_engine());
        let resp = server.create_session(empty_create()).await.unwrap();
        let mut s = server
            .send_message(MessageSendRequest {
                session_id: resp.session_id.clone(),
                text: "hello".to_string(),
                tools: Vec::new(),
            })
            .await
            .unwrap();
        let first = s.next().await.expect("one event");
        match first {
            MessageEvent::Error { error, .. } => {
                assert_eq!(error.message, "no turn engine installed");
                assert_eq!(error.code, ErrorCode::InternalError.code());
            }
            other => panic!("expected Error, got {other:?}"),
        }
        assert!(s.next().await.is_none(), "stream should end after Error");

        // last_activity + message_count should have advanced regardless.
        let got = server.get_session(resp.session_id).await.unwrap();
        assert_eq!(got.session.message_count, 1);
    }

    // T-A3: with an engine installed, `send_message` proxies the engine's
    // stream verbatim; a missing session still errors BEFORE the engine runs.
    #[tokio::test]
    async fn send_message_with_engine_proxies_stream() {
        let engine = Arc::new(MockTurnEngine::new(vec![
            MessageEvent::TextDelta {
                text: "hi".to_string(),
            },
            MessageEvent::Done {
                stop_reason: "end_turn".to_string(),
                turn_id: String::new(),
            },
        ]));
        let server = AcpServer::new().with_turn_engine(engine.clone());
        assert!(server.has_turn_engine());
        let resp = server.create_session(empty_create()).await.unwrap();

        let mut s = server
            .send_message(MessageSendRequest {
                session_id: resp.session_id.clone(),
                text: "go".to_string(),
                tools: Vec::new(),
            })
            .await
            .unwrap();
        match s.next().await.expect("first") {
            MessageEvent::TextDelta { text } => assert_eq!(text, "hi"),
            other => panic!("expected TextDelta, got {other:?}"),
        }
        match s.next().await.expect("terminal") {
            MessageEvent::Done { stop_reason, .. } => assert_eq!(stop_reason, "end_turn"),
            other => panic!("expected Done, got {other:?}"),
        }
        assert!(s.next().await.is_none());

        // Missing session errors before the engine is reached.
        match server
            .send_message(MessageSendRequest {
                session_id: "nope".to_string(),
                text: "x".to_string(),
                tools: Vec::new(),
            })
            .await
        {
            Err(AcpError::Session(_)) => {}
            Err(other) => panic!("expected Session error, got {other:?}"),
            Ok(_) => panic!("expected session-not-found error"),
        }
    }

    // T-A4: create with system_prompt + tools, then verify they are stored
    // (previously dropped) and that an empty-body send falls back to the
    // stored allowlist.
    #[tokio::test]
    async fn create_stores_tools_and_send_falls_back_to_them() {
        let tools = vec![ToolDefinition {
            name: "Read".to_string(),
            description: "read".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
        }];
        let engine = Arc::new(MockTurnEngine::new(vec![MessageEvent::Done {
            stop_reason: "end_turn".to_string(),
            turn_id: String::new(),
        }]));
        let server = AcpServer::new().with_turn_engine(engine.clone());
        let resp = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: tools.clone(),
                system_prompt: Some("be terse".to_string()),
                agent: None,
                mcp_servers: Vec::new(),
            })
            .await
            .unwrap();

        // Store-extension proof: the tools survived create.
        let stored = server
            .session_tools(&resp.session_id)
            .await
            .expect("session exists");
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, "Read");

        // Empty-body send falls back to the stored allowlist; assert the
        // engine saw it.
        let _ = server
            .send_message(MessageSendRequest {
                session_id: resp.session_id.clone(),
                text: "go".to_string(),
                tools: Vec::new(),
            })
            .await
            .unwrap();
        let seen = engine.last_req.lock().unwrap().clone();
        let seen = seen.expect("engine was called");
        assert_eq!(seen.tools.len(), 1);
        assert_eq!(seen.tools[0].name, "Read");
    }

    // Per-call tools override the stored allowlist.
    #[tokio::test]
    async fn send_message_per_call_tools_override_stored() {
        let stored_tool = ToolDefinition {
            name: "Read".to_string(),
            description: "read".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
        };
        let call_tool = ToolDefinition {
            name: "Bash".to_string(),
            description: "shell".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
        };
        let engine = Arc::new(MockTurnEngine::new(vec![MessageEvent::Done {
            stop_reason: "end_turn".to_string(),
            turn_id: String::new(),
        }]));
        let server = AcpServer::new().with_turn_engine(engine.clone());
        let resp = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: vec![stored_tool],
                system_prompt: None,
                agent: None,
                mcp_servers: Vec::new(),
            })
            .await
            .unwrap();
        let _ = server
            .send_message(MessageSendRequest {
                session_id: resp.session_id,
                text: "go".to_string(),
                tools: vec![call_tool],
            })
            .await
            .unwrap();
        let seen = engine.last_req.lock().unwrap().clone().unwrap();
        assert_eq!(seen.tools.len(), 1);
        assert_eq!(seen.tools[0].name, "Bash", "per-call tools win");
    }

    /// #998 c6 — the per-tool MCP switches supplied at `session/create` reach
    /// the turn engine, and they come from the SESSION RECORD.
    ///
    /// `MessageSendRequest` carries no MCP field at all, so this is also the
    /// proof that a per-message body has no way to introduce a selection the
    /// session was not created with, or to widen one it was.
    #[tokio::test]
    async fn session_create_mcp_switches_reach_the_turn_engine() {
        let engine = Arc::new(MockTurnEngine::new(vec![MessageEvent::Done {
            stop_reason: "end_turn".to_string(),
            turn_id: String::new(),
        }]));
        let server = AcpServer::new().with_turn_engine(engine.clone());
        let selection = crate::protocol::McpToolSelection {
            server: "library".to_string(),
            allowed_tools: Some(vec!["safe_read".to_string()]),
        };
        let resp = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: Vec::new(),
                system_prompt: None,
                agent: None,
                mcp_servers: vec![selection.clone()],
            })
            .await
            .unwrap();
        assert_eq!(
            server.session_mcp_servers(&resp.session_id).await,
            vec![selection.clone()],
            "the selection is bound to the session record at create"
        );

        let _ = server
            .send_message(MessageSendRequest {
                session_id: resp.session_id,
                text: "go".to_string(),
                tools: Vec::new(),
            })
            .await
            .unwrap();
        let seen = engine.last_req.lock().unwrap().clone().unwrap();
        assert_eq!(
            seen.mcp_servers,
            vec![selection],
            "the turn must carry the session's switches to the engine"
        );
    }

    /// CONTROL: a session created with no selection carries none, so the
    /// pre-#998 behaviour of every existing ACP session is unchanged.
    #[tokio::test]
    async fn a_session_with_no_mcp_switches_carries_none() {
        let engine = Arc::new(MockTurnEngine::new(vec![MessageEvent::Done {
            stop_reason: "end_turn".to_string(),
            turn_id: String::new(),
        }]));
        let server = AcpServer::new().with_turn_engine(engine.clone());
        let resp = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: Vec::new(),
                system_prompt: None,
                agent: None,
                mcp_servers: Vec::new(),
            })
            .await
            .unwrap();
        let _ = server
            .send_message(MessageSendRequest {
                session_id: resp.session_id,
                text: "go".to_string(),
                tools: Vec::new(),
            })
            .await
            .unwrap();
        assert!(
            engine
                .last_req
                .lock()
                .unwrap()
                .clone()
                .unwrap()
                .mcp_servers
                .is_empty()
        );
    }

    #[tokio::test]
    async fn send_message_missing_session_errors() {
        let server = AcpServer::new();
        // The Ok variant `Pin<Box<dyn Stream>>` is not Debug, so
        // `expect_err` won't compile — match instead.
        match server
            .send_message(MessageSendRequest {
                session_id: "nope".to_string(),
                text: "x".to_string(),
                tools: Vec::new(),
            })
            .await
        {
            Err(AcpError::Session(_)) => {}
            Err(other) => panic!("expected Session error, got {other:?}"),
            Ok(_) => panic!("expected session-not-found error"),
        }
    }

    // v0.8.1 U12 — A2A integration tests. These exercise the production
    // call-site shape: AcpServer::new().with_a2a_handler(Arc::new(...))
    // followed by a2a_* dispatch methods.

    #[tokio::test]
    async fn a2a_handshake_no_handler_returns_protocol_error() {
        let server = AcpServer::new();
        assert!(!server.has_a2a_handler());
        let incoming = crate::a2a::A2aHandshake {
            agent_id: "peer".to_string(),
            agent_kind: "other".to_string(),
            version: "0.0.1".to_string(),
            capabilities: crate::a2a::A2aCapabilities::default(),
        };
        let err = server
            .a2a_handshake(incoming)
            .await
            .expect_err("no handler");
        assert!(matches!(err, AcpError::Protocol(_)));
    }

    #[tokio::test]
    async fn a2a_handshake_with_handler_returns_self_identity() {
        let handler = Arc::new(crate::a2a::DefaultA2aHandler::new("server-agent"));
        let server = AcpServer::new().with_a2a_handler(handler);
        assert!(server.has_a2a_handler());
        let incoming = crate::a2a::A2aHandshake {
            agent_id: "peer".to_string(),
            agent_kind: "other".to_string(),
            version: "0.0.1".to_string(),
            capabilities: crate::a2a::A2aCapabilities::default(),
        };
        let reply = server.a2a_handshake(incoming).await.unwrap();
        assert_eq!(reply.agent_kind, "wayland-core");
        assert_eq!(reply.agent_id, "server-agent");
    }

    #[tokio::test]
    async fn a2a_message_send_with_handler_echoes() {
        let handler = Arc::new(crate::a2a::DefaultA2aHandler::new("server-agent"));
        let server = AcpServer::new().with_a2a_handler(handler);
        let msg = crate::a2a::A2aMessage {
            from: "peer".to_string(),
            to: "server-agent".to_string(),
            text: "ping".to_string(),
            attachments: vec![],
            correlation_id: Some("c1".to_string()),
        };
        let reply = server.a2a_message_send(msg).await.unwrap();
        assert_eq!(reply.text, "ack: ping");
        assert_eq!(reply.from, "server-agent");
        assert_eq!(reply.to, "peer");
        assert_eq!(reply.correlation_id, Some("c1".to_string()));
    }

    #[tokio::test]
    async fn a2a_capabilities_with_handler_returns_set_caps() {
        let handler = Arc::new(crate::a2a::DefaultA2aHandler::new("server-agent"));
        let mut caps = crate::a2a::A2aCapabilities::default();
        caps.skills.push("plan".to_string());
        caps.tools.push("read".to_string());
        caps.streaming_supported = false;
        handler.set_capabilities(caps);
        let server = AcpServer::new().with_a2a_handler(handler);
        let got = server.a2a_capabilities().await.unwrap();
        assert_eq!(got.skills, vec!["plan"]);
        assert_eq!(got.tools, vec!["read"]);
    }

    // ── persona-profiles Phase A: roster wiring + capability handshake ──────

    use crate::protocol::AgentInfo;
    use crate::roster::AgentRoster;

    /// Fixed in-memory roster for server tests. Returns a canned authorized
    /// set — the same fixed-script mock style as `MockTurnEngine`.
    struct MockRoster {
        agents: Vec<AgentInfo>,
    }

    #[async_trait]
    impl AgentRoster for MockRoster {
        async fn list(&self) -> Result<Vec<AgentInfo>, AcpError> {
            Ok(self.agents.clone())
        }
    }

    fn agent_info(id: &str) -> AgentInfo {
        AgentInfo {
            id: id.to_string(),
            label: id.to_string(),
            description: None,
        }
    }

    // Feature default-OFF: no roster ⇒ `agents/list` is empty, and the server
    // reports it has no roster.
    #[tokio::test]
    async fn list_agents_empty_without_roster() {
        let server = AcpServer::new();
        assert!(!server.has_roster());
        let resp = server.list_agents().await.unwrap();
        assert!(resp.agents.is_empty());
    }

    // With a roster installed, `agents/list` returns its authorized catalog.
    #[tokio::test]
    async fn list_agents_returns_roster_catalog() {
        let roster = Arc::new(MockRoster {
            agents: vec![agent_info("architect"), agent_info("researcher")],
        });
        let server = AcpServer::new().with_roster(roster);
        assert!(server.has_roster());
        let resp = server.list_agents().await.unwrap();
        let ids: Vec<&str> = resp.agents.iter().map(|a| a.id.as_str()).collect();
        assert_eq!(ids, ["architect", "researcher"]);
    }

    // R3: a `session/create` selecting an AUTHORIZED agent succeeds and the id
    // is recorded (readable via `session_agent`) — but no persona overlay is
    // applied (PR-2 records only).
    #[tokio::test]
    async fn create_with_authorized_agent_records_selector() {
        let roster = Arc::new(MockRoster {
            agents: vec![agent_info("architect")],
        });
        let server = AcpServer::new().with_roster(roster);
        let resp = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: Vec::new(),
                system_prompt: None,
                agent: Some("architect".to_string()),
                mcp_servers: Vec::new(),
            })
            .await
            .unwrap();
        assert_eq!(
            server.session_agent(&resp.session_id).await.as_deref(),
            Some("architect")
        );
    }

    // R3: selecting an agent NOT in the authorized roster is rejected with the
    // agent-not-found signal (maps to ErrorCode::AgentNotFound at the transport).
    #[tokio::test]
    async fn create_with_unauthorized_agent_is_agent_error() {
        let roster = Arc::new(MockRoster {
            agents: vec![agent_info("architect")],
        });
        let server = AcpServer::new().with_roster(roster);
        let err = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: Vec::new(),
                system_prompt: None,
                agent: Some("root".to_string()),
                mcp_servers: Vec::new(),
            })
            .await
            .expect_err("unauthorized agent must be rejected");
        assert!(matches!(err, AcpError::Agent(_)), "got {err:?}");
    }

    // ── persona-profiles PR-7: profile supervisor/router dispatch ───────────

    use crate::router::ProfileRouter;

    /// A mock supervisor that records routed calls WITHOUT spawning a real
    /// child — proves `AcpServer` dispatches a `profile:` session to the router
    /// (and a non-profile session to the engine), and fails closed on open error.
    #[derive(Default)]
    struct MockProfileRouter {
        opened: std::sync::Mutex<Vec<(String, String)>>,
        sent: std::sync::Mutex<Vec<String>>,
        deleted: std::sync::Mutex<Vec<String>>,
        fail_open: bool,
    }

    #[async_trait]
    impl ProfileRouter for MockProfileRouter {
        async fn open(
            &self,
            session_id: &str,
            agent: &str,
            _req: &SessionCreateRequest,
        ) -> Result<(), AcpError> {
            if self.fail_open {
                return Err(AcpError::Agent(format!("cannot open {agent}")));
            }
            self.opened
                .lock()
                .unwrap()
                .push((session_id.to_string(), agent.to_string()));
            Ok(())
        }
        async fn send(
            &self,
            req: MessageSendRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
            self.sent.lock().unwrap().push(req.session_id.clone());
            let ev = MessageEvent::Done {
                stop_reason: "end_turn".to_string(),
                turn_id: req.session_id,
            };
            Ok(stream::iter(vec![ev]).boxed())
        }
        async fn get(&self, session_id: &str) -> Result<SessionGetResponse, AcpError> {
            Err(AcpError::Session(format!("no child for {session_id}")))
        }
        async fn delete(&self, session_id: &str) -> Result<(), AcpError> {
            self.deleted.lock().unwrap().push(session_id.to_string());
            Ok(())
        }
    }

    // A `profile:<name>` session is opened on the router at create, forwarded to
    // it on send, and reaped on delete — NEVER the in-process engine.
    #[tokio::test]
    async fn profile_agent_routes_to_supervisor_not_engine() {
        let roster = Arc::new(MockRoster {
            agents: vec![agent_info("profile:work")],
        });
        let router = Arc::new(MockProfileRouter::default());
        // An engine is installed but must NOT run for a profile session — its
        // script is a marker we assert never reaches the caller.
        let engine = Arc::new(MockTurnEngine::new(vec![MessageEvent::Error {
            error: JsonRpcError {
                code: -1,
                message: "ENGINE-MUST-NOT-RUN".to_string(),
                data: None,
            },
            turn_id: String::new(),
        }]));
        let server = AcpServer::new()
            .with_roster(roster)
            .with_turn_engine(engine)
            .with_profile_router(router.clone());

        let resp = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: Vec::new(),
                system_prompt: None,
                agent: Some("profile:work".to_string()),
                mcp_servers: Vec::new(),
            })
            .await
            .unwrap();
        assert_eq!(
            router.opened.lock().unwrap().as_slice(),
            &[(resp.session_id.clone(), "profile:work".to_string())],
            "create must open the child on the router"
        );

        let s = server
            .send_message(MessageSendRequest {
                session_id: resp.session_id.clone(),
                text: "hi".to_string(),
                tools: Vec::new(),
            })
            .await
            .unwrap();
        let frames: Vec<MessageEvent> = s.collect().await;
        // Routed to the child (a clean Done), NOT the engine's error marker.
        assert!(
            matches!(frames.last(), Some(MessageEvent::Done { .. })),
            "got {frames:?}"
        );
        assert_eq!(
            router.sent.lock().unwrap().as_slice(),
            std::slice::from_ref(&resp.session_id)
        );

        server
            .delete_session(resp.session_id.clone())
            .await
            .unwrap();
        assert_eq!(
            router.deleted.lock().unwrap().as_slice(),
            &[resp.session_id]
        );
    }

    // FAIL CLOSED: a child that cannot open fails the create AND leaves no
    // dangling parent session (which could later fall through to the default).
    #[tokio::test]
    async fn profile_open_failure_fails_closed_with_no_dangling_session() {
        let roster = Arc::new(MockRoster {
            agents: vec![agent_info("profile:ghost")],
        });
        let router = Arc::new(MockProfileRouter {
            fail_open: true,
            ..Default::default()
        });
        let server = AcpServer::new()
            .with_roster(roster)
            .with_profile_router(router);
        let err = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: Vec::new(),
                system_prompt: None,
                agent: Some("profile:ghost".to_string()),
                mcp_servers: Vec::new(),
            })
            .await
            .expect_err("a child that cannot open must fail the create");
        assert!(matches!(err, AcpError::Agent(_)), "got {err:?}");
        assert_eq!(
            server.session_count().await,
            0,
            "no dangling session after a failed profile bind"
        );
    }

    // FAIL CLOSED: a profile agent authorized but NO supervisor installed must
    // error — never serve this process's default identity under the profile name.
    #[tokio::test]
    async fn profile_agent_without_router_fails_closed() {
        let roster = Arc::new(MockRoster {
            agents: vec![agent_info("profile:work")],
        });
        let server = AcpServer::new().with_roster(roster);
        let err = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: Vec::new(),
                system_prompt: None,
                agent: Some("profile:work".to_string()),
                mcp_servers: Vec::new(),
            })
            .await
            .expect_err("profile agent with no supervisor must fail closed");
        assert!(matches!(err, AcpError::Agent(_)), "got {err:?}");
        assert_eq!(server.session_count().await, 0);
    }

    // Feature default-OFF: selecting any agent when NO roster is installed is
    // rejected (cannot authorize without a roster) — fail closed.
    #[tokio::test]
    async fn create_with_agent_but_no_roster_is_agent_error() {
        let server = AcpServer::new();
        let err = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: Vec::new(),
                system_prompt: None,
                agent: Some("architect".to_string()),
                mcp_servers: Vec::new(),
            })
            .await
            .expect_err("no roster ⇒ cannot authorize any selector");
        assert!(matches!(err, AcpError::Agent(_)), "got {err:?}");
    }

    // Compat (R2): a selector-free create is unaffected and records no agent.
    #[tokio::test]
    async fn create_without_agent_records_none() {
        let server = AcpServer::new();
        let resp = server.create_session(empty_create()).await.unwrap();
        assert_eq!(server.session_agent(&resp.session_id).await, None);
    }

    // R2: AcpServer advertises the `agent_selection` capability in `initialize`.
    #[tokio::test]
    async fn initialize_advertises_agent_selection() {
        let server = AcpServer::new();
        let resp = server.initialize().await.unwrap();
        assert_eq!(resp.protocol_version, ACP_PROTOCOL_VERSION);
        assert!(
            resp.capabilities.agent_selection,
            "AcpServer must advertise agent_selection (R2)"
        );
        // Advertised even without a roster (capability = protocol understanding,
        // not availability).
        assert!(!server.has_roster());
    }

    #[tokio::test]
    async fn tool_definitions_accepted_in_create() {
        let server = AcpServer::new();
        let tools = vec![ToolDefinition {
            name: "Read".to_string(),
            description: "read".to_string(),
            input_schema: serde_json::json!({"type":"object"}),
        }];
        let resp = server
            .create_session(SessionCreateRequest {
                model: None,
                tools,
                system_prompt: None,
                agent: None,
                mcp_servers: Vec::new(),
            })
            .await
            .unwrap();
        assert!(!resp.session_id.is_empty());
    }
}
