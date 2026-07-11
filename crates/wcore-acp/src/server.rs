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

use crate::error::AcpError;
use crate::protocol::{
    ACP_PROTOCOL_VERSION, AgentsListResponse, ErrorCode, InitializeResponse, JsonRpcError,
    MessageEvent, MessageSendRequest, ServerCapabilities, SessionCreateRequest,
    SessionCreateResponse, SessionGetResponse, SessionListResponse, SessionMetadata,
    ToolDefinition,
};
use crate::roster::AgentRoster;
use crate::transport::HttpHandler;

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
}

/// Minimal ACP server with in-memory session storage.
///
/// All session state is held in an `Arc<RwLock<HashMap<_, _>>>`; the
/// server is `Clone`-friendly via the inner `Arc`. Construct one and
/// hand it to [`HttpSseTransport::new`] (and friends) to wire the wire
/// transports to the same backing state.
#[derive(Clone, Default)]
pub struct AcpServer {
    sessions: Arc<RwLock<HashMap<String, SessionRecord>>>,
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
            .finish()
    }
}

impl AcpServer {
    /// Construct an empty server.
    pub fn new() -> Self {
        Self::default()
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

    /// The AUTHORIZED persona-agent id bound to `session_id` at create-time, if
    /// the session exists and selected one. Parallels [`Self::session_tools`].
    /// A later per-session persona binding (PR-4) reads this to resolve the
    /// overlay; in PR-2 it is a read-only record of the validated selector.
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
        };
        self.sessions.write().await.insert(id.clone(), record);
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
        let mut guard = self.sessions.write().await;
        if guard.remove(&session_id).is_some() {
            Ok(())
        } else {
            Err(AcpError::Session(format!(
                "session not found: {session_id}"
            )))
        }
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

        match &self.turn_engine {
            Some(engine) => {
                engine
                    .run_turn(crate::turn::TurnRequest {
                        session_id: req.session_id,
                        text: req.text,
                        tools,
                    })
                    .await
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
                };
                Ok(stream::iter(vec![ev]).boxed())
            }
        }
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
            MessageEvent::Error { error } => {
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
            MessageEvent::Done { stop_reason } => assert_eq!(stop_reason, "end_turn"),
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
        }]));
        let server = AcpServer::new().with_turn_engine(engine.clone());
        let resp = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: tools.clone(),
                system_prompt: Some("be terse".to_string()),
                agent: None,
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
        }]));
        let server = AcpServer::new().with_turn_engine(engine.clone());
        let resp = server
            .create_session(SessionCreateRequest {
                model: None,
                tools: vec![stored_tool],
                system_prompt: None,
                agent: None,
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
            })
            .await
            .expect_err("unauthorized agent must be rejected");
        assert!(matches!(err, AcpError::Agent(_)), "got {err:?}");
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
            })
            .await
            .unwrap();
        assert!(!resp.session_id.is_empty());
    }
}
