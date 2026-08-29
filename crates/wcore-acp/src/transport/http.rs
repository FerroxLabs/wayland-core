//! HTTP/SSE transport for ACP.
//!
//! Maps the ACP session/message surface onto a REST-ish HTTP API:
//!
//! - `POST   /sessions`              → `session/create`
//! - `GET    /sessions`              → `session/list`
//! - `GET    /sessions/:id`          → `session/get`
//! - `DELETE /sessions/:id`          → `session/delete`
//! - `POST   /sessions/:id/messages` → `message/send` (SSE stream of [`MessageEvent`])
//! - `GET    /initialize`            → capability handshake (persona-profiles R2)
//! - `GET    /agents`                → `agents/list` (persona-profiles roster)
//!
//! The transport is decoupled from the server implementation via the
//! [`HttpHandler`] trait; the actual ACP server (lands in 1.A.6) plugs in by
//! implementing this trait.
//!
//! **Security (F-017)**: CORS is strict by default (no
//! `Access-Control-Allow-Origin: *`). `CorsLayer::permissive()` has been
//! removed — production deployments that genuinely need cross-origin access
//! must explicitly opt in via a wrapper layer. Auth middleware is installed
//! when a [`crate::auth::Verifier`] is supplied via
//! [`HttpSseTransport::with_verifier`]; `serve` in `wcore-cli/src/acp.rs`
//! always installs one (generated one-time key, printed to stderr on first
//! start).

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::{Stream, StreamExt};

use crate::auth::{Principal, Verifier};
use crate::cursor::{Cursor, CursorError, ResumeError, ResumeResponse};
use crate::error::AcpError;
use crate::protocol::{
    ACP_PROTOCOL_VERSION, AgentsListResponse, ErrorCode, InitializeResponse, JsonRpcError,
    MessageEvent, MessageSendRequest, ServerCapabilities, SessionCreateRequest,
    SessionCreateResponse, SessionGetResponse, SessionListResponse,
};

/// Header carrying a command's idempotency identity.
pub const IDEMPOTENCY_HEADER: &str = "idempotency-key";
/// Header a client uses to advertise the protocol version it speaks.
pub const PROTOCOL_VERSION_HEADER: &str = "x-acp-protocol-version";
/// Header carrying the version the handshake settled on.
pub const NEGOTIATED_VERSION_HEADER: &str = "x-acp-negotiated-version";
/// Header telling a newer client it was met at the server's version.
pub const CLIENT_IS_NEWER_HEADER: &str = "x-acp-client-is-newer";

/// The ACP method name a route+verb pair stands for.
///
/// # An unrecognised route resolves to its own path, and that is the point
///
/// The role table classifies METHODS, and its documented default for a method
/// it does not know is `Admin` — the loud failure. Returning the raw path for
/// an unmatched route feeds that default rather than defeating it: a route
/// added without being classified stops working for ordinary principals,
/// somebody notices, and the table gets an entry. Returning something like
/// `"unknown"` and special-casing it would be the quiet version of the same
/// omission.
pub(crate) fn acp_method_for(path: &str, verb: &axum::http::Method) -> String {
    use axum::http::Method;
    let segments: Vec<&str> = path.trim_matches('/').split('/').collect();
    match (verb, segments.as_slice()) {
        (&Method::GET, ["initialize"]) => "initialize".to_string(),
        (&Method::GET, ["agents"]) => "agents/list".to_string(),
        (&Method::POST, ["sessions"]) => "session/create".to_string(),
        (&Method::GET, ["sessions"]) => "session/list".to_string(),
        (&Method::GET, ["sessions", _]) => "session/get".to_string(),
        (&Method::DELETE, ["sessions", _]) => "session/delete".to_string(),
        (&Method::POST, ["sessions", _, "messages"]) => "message/send".to_string(),
        (&Method::GET, ["sessions", _, "events"]) => "session/events".to_string(),
        _ => path.to_string(),
    }
}

/// Trait implemented by the ACP server to back the HTTP/SSE transport.
#[async_trait]
pub trait HttpHandler: Send + Sync + 'static {
    async fn create_session(
        &self,
        req: SessionCreateRequest,
    ) -> Result<SessionCreateResponse, AcpError>;

    async fn list_sessions(&self) -> Result<SessionListResponse, AcpError>;

    async fn get_session(&self, session_id: String) -> Result<SessionGetResponse, AcpError>;

    async fn delete_session(&self, session_id: String) -> Result<(), AcpError>;

    async fn send_message(
        &self,
        req: MessageSendRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError>;

    /// Resolve a pending tool-approval gate (Blocker #2). Backs
    /// `POST /v1/sessions/{id}/approvals/{call_id}/resolve` on the REST
    /// transport. Default impl reports "not supported" so handlers that do
    /// not host an approval-capable engine (e.g. test mocks) compile
    /// unchanged; `AcpServer` overrides it to delegate to its `TurnEngine`.
    async fn resolve_approval(
        &self,
        _session_id: String,
        _call_id: String,
        _decision: crate::turn::ApprovalDecision,
    ) -> Result<(), AcpError> {
        Err(AcpError::Protocol(
            "approval resolution not supported".to_string(),
        ))
    }

    /// #305 c2 — the operator's project allowlist (`GET /v1/approvals/projects`).
    ///
    /// Defaults to an EMPTY list rather than a "not supported" error: an
    /// allowlist with nothing in it is the honest description of a handler that
    /// does not host one, and it is also the fail-closed state, so a host can
    /// read the posture off one route without special-casing the answer.
    async fn list_projects(&self) -> Result<crate::allowlist::ProjectListResponse, AcpError> {
        Ok(crate::allowlist::ProjectListResponse {
            projects: Vec::new(),
        })
    }

    /// #305 c2 — add a project root, or flip an existing one's `enabled`.
    /// Default refuses: granting authority is never something a handler should
    /// appear to do by inheriting a default.
    async fn upsert_project(
        &self,
        _path: String,
        _enabled: bool,
    ) -> Result<crate::allowlist::ProjectEntry, AcpError> {
        Err(AcpError::Protocol(
            "project allowlist not supported by this handler".to_string(),
        ))
    }

    /// #305 c2 — remove one project root by id.
    async fn delete_project(&self, _id: String) -> Result<(), AcpError> {
        Err(AcpError::Protocol(
            "project allowlist not supported by this handler".to_string(),
        ))
    }

    /// persona-profiles Phase A — the persona-agent roster (`agents/list`).
    /// Default returns an empty roster so existing handlers + mocks compile
    /// unchanged and the route is backward-compatible (feature default-OFF).
    /// `AcpServer` overrides this to consult an installed `AgentRoster`, which
    /// returns only the AUTHORIZED agents (R3), each id/label-only (R4).
    async fn list_agents(&self) -> Result<AgentsListResponse, AcpError> {
        Ok(AgentsListResponse { agents: Vec::new() })
    }

    /// Decide `method` for the principal the transport just verified.
    ///
    /// The default performs no role gating, which is the behaviour of every
    /// handler that existed before roles did. `AcpServer` overrides it to
    /// consult an installed [`crate::roles::RolePolicy`]. A handler returning
    /// `Ok` here has not necessarily CHECKED anything — the transport does not
    /// and cannot know which, so nothing downstream may report a successful
    /// request as an authorization that passed.
    async fn authorize_method(
        &self,
        _principal: &Principal,
        _method: &str,
    ) -> Result<(), AcpError> {
        Ok(())
    }

    /// `session/create` under an optional idempotency identity.
    ///
    /// The default REFUSES a request that carries a key rather than ignoring
    /// it. Ignoring it is the dangerous option: the client believes its retry
    /// is protected, retries, and gets a second session. A handler that cannot
    /// honour the guarantee says so.
    async fn create_session_idempotent(
        &self,
        key: Option<&str>,
        req: SessionCreateRequest,
    ) -> Result<SessionCreateResponse, AcpError> {
        match key {
            None => self.create_session(req).await,
            Some(_) => Err(AcpError::Protocol(
                "this handler does not implement command idempotency; honouring the \
                 Idempotency-Key header would be a promise it cannot keep"
                    .to_string(),
            )),
        }
    }

    /// `session/delete` under an optional idempotency identity. Same default
    /// refusal, and for the same reason, as [`Self::create_session_idempotent`].
    async fn delete_session_idempotent(
        &self,
        key: Option<&str>,
        session_id: String,
    ) -> Result<(), AcpError> {
        match key {
            None => self.delete_session(session_id).await,
            Some(_) => Err(AcpError::Protocol(
                "this handler does not implement command idempotency; honouring the \
                 Idempotency-Key header would be a promise it cannot keep"
                    .to_string(),
            )),
        }
    }

    /// Serve a resume — everything after `cursor` on the session's stream.
    ///
    /// The default is [`ResumeError::Unsupported`], NOT an empty list. A
    /// handler with no event log answering "you missed nothing" is a surface
    /// that was not looking reporting a zero.
    async fn resume_events(
        &self,
        _session_id: String,
        _cursor: Cursor,
    ) -> Result<ResumeResponse, ResumeError> {
        Err(ResumeError::Unsupported)
    }

    /// persona-profiles Phase A — the capability handshake (`initialize`, R2).
    /// Default advertises NO extension capabilities (conservative), so an
    /// arbitrary handler does not over-claim. `AcpServer` overrides this to
    /// advertise `agent_selection`, telling clients this build understands the
    /// optional `agent` selector + `agents/list` before any selector is used.
    async fn initialize(&self) -> Result<InitializeResponse, AcpError> {
        Ok(InitializeResponse {
            protocol_version: ACP_PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities::default(),
        })
    }
}

/// HTTP/SSE transport. Build a router with [`Self::router`] and serve it on
/// whatever `axum::serve` or hyper binding suits the host.
///
/// Call [`Self::with_verifier`] before [`Self::router`] to add auth
/// middleware. Without a verifier the router still works (useful for unit
/// tests against the in-process handler), but production callers in
/// `wcore-cli/src/acp.rs` always install the generated one-time API key
/// verifier (F-017).
pub struct HttpSseTransport<H: HttpHandler> {
    handler: Arc<H>,
    /// Optional auth verifier. When `Some`, every request must supply a
    /// valid `X-API-Key` header (or `Authorization: ApiKey <key>`). When
    /// `None`, no auth check is applied — only acceptable for unit tests
    /// that talk to the handler directly without a network boundary. (F-017)
    verifier: Option<Arc<dyn Verifier>>,
}

impl<H: HttpHandler> HttpSseTransport<H> {
    pub fn new(handler: Arc<H>) -> Self {
        Self {
            handler,
            verifier: None,
        }
    }

    /// Install an auth verifier. Returns `Self` for chaining.
    ///
    /// Production callers (see `wcore-cli/src/acp.rs`) pass an
    /// [`crate::auth::ApiKeyVerifier`] backed by a generated one-time key.
    /// (F-017)
    pub fn with_verifier(mut self, v: Arc<dyn Verifier>) -> Self {
        self.verifier = Some(v);
        self
    }

    /// Build an `axum::Router` wired to the handler.
    ///
    /// CORS: strict by default — no `Access-Control-Allow-Origin: *`.
    /// Auth: if a verifier is installed, an axum middleware layer enforces
    /// it on every route, returning 401 on missing or wrong credentials.
    /// (F-017)
    pub fn router(&self) -> Router {
        let base = Router::new()
            .route(
                "/sessions",
                post(create_session::<H>).get(list_sessions::<H>),
            )
            .route(
                "/sessions/:id",
                get(get_session::<H>).delete(delete_session::<H>),
            )
            .route("/sessions/:id/messages", post(send_message::<H>))
            // F24-04: the resume route. This is the transport half of the
            // cursor contract — without it the event log is a structure no
            // client can reach.
            .route("/sessions/:id/events", get(resume_events::<H>))
            // persona-profiles Phase A: capability handshake + agent roster.
            // Both default-safe (empty roster / advertised capability only).
            .route("/initialize", get(initialize::<H>))
            .route("/agents", get(get_agents::<H>))
            .with_state(self.handler.clone());

        // F-017: wrap with auth middleware when a verifier is present.
        if let Some(v) = self.verifier.clone() {
            let handler = self.handler.clone();
            base.layer(middleware::from_fn(move |req: Request, next: Next| {
                let v = Arc::clone(&v);
                let h = Arc::clone(&handler);
                async move { auth_middleware(v, h, req, next).await }
            }))
        } else {
            base
        }
    }
}

/// Axum middleware: extract headers, run the verifier, then put the verified
/// principal through the handler's role decision. 401 on a failed
/// verification, 403 on a refused role.
///
/// # The two refusals stay apart all the way to the wire
///
/// A 401 invites the client to re-authenticate. For a role refusal that is a
/// retry loop that can never succeed, and it sends an operator to rotate a
/// credential that was never the problem. The role decision therefore produces
/// [`AcpError::Forbidden`], which [`status_for`] maps to 403 and [`code_for`]
/// maps to [`ErrorCode::Forbidden`] — asserted over a real socket in
/// `tests/roles_and_idempotency.rs`, not just at this function's boundary.
///
/// Authorization runs ONLY where a verifier is installed, because without one
/// there is no verified principal to decide about, and deciding from an
/// unverified identity is worse than not deciding. (F-017)
async fn auth_middleware(
    verifier: Arc<dyn Verifier>,
    handler: Arc<dyn AuthorizesMethods>,
    req: Request,
    next: Next,
) -> Response {
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            let name = k.as_str().to_string();
            v.to_str().ok().map(|val| (name, val.to_string()))
        })
        .collect();

    let principal = match verifier.verify(&headers) {
        Ok(p) => p,
        Err(e) => {
            let body = JsonRpcError {
                code: ErrorCode::AuthRequired.code(),
                message: e.to_string(),
                data: None,
            };
            return (StatusCode::UNAUTHORIZED, Json(body)).into_response();
        }
    };

    let method = acp_method_for(req.uri().path(), req.method());
    if let Err(e) = handler.authorize_method_dyn(&principal, &method).await {
        return AcpHttpError(e).into_response();
    }
    next.run(req).await
}

/// Object-safe view of [`HttpHandler::authorize_method`], so the middleware can
/// hold the handler behind a trait object without making `HttpHandler` itself
/// object-safe (it is not — `send_message` returns an `impl`-shaped stream).
#[async_trait]
pub(crate) trait AuthorizesMethods: Send + Sync + 'static {
    async fn authorize_method_dyn(
        &self,
        principal: &Principal,
        method: &str,
    ) -> Result<(), AcpError>;
}

#[async_trait]
impl<H: HttpHandler> AuthorizesMethods for H {
    async fn authorize_method_dyn(
        &self,
        principal: &Principal,
        method: &str,
    ) -> Result<(), AcpError> {
        self.authorize_method(principal, method).await
    }
}

// ── Error mapping ───────────────────────────────────────────────────────

fn status_for(err: &AcpError) -> StatusCode {
    match err {
        AcpError::Auth(_) => StatusCode::UNAUTHORIZED,
        // 403, never 401. A 401 tells the client its credential was not
        // accepted and invites it to re-authenticate, which for a role refusal
        // is a retry loop that can never succeed.
        AcpError::Forbidden(_) => StatusCode::FORBIDDEN,
        // persona-profiles R3/R4: an unauthorized/unknown agent selector is a
        // 404 — it leaks no existence information (the roster only ever exposes
        // authorized agents, so "unknown" and "forbidden" are indistinguishable).
        AcpError::Agent(_) => StatusCode::NOT_FOUND,
        AcpError::Session(msg) if msg.to_lowercase().contains("not found") => StatusCode::NOT_FOUND,
        AcpError::Session(_) => StatusCode::BAD_REQUEST,
        AcpError::Protocol(_) => StatusCode::BAD_REQUEST,
        AcpError::Serde(_) => StatusCode::BAD_REQUEST,
        AcpError::Io(_) | AcpError::Transport(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn code_for(err: &AcpError) -> ErrorCode {
    match err {
        AcpError::Auth(_) => ErrorCode::AuthRequired,
        AcpError::Forbidden(_) => ErrorCode::Forbidden,
        AcpError::Agent(_) => ErrorCode::AgentNotFound,
        AcpError::Session(_) => ErrorCode::SessionNotFound,
        AcpError::Protocol(_) | AcpError::Serde(_) => ErrorCode::InvalidRequest,
        AcpError::Io(_) | AcpError::Transport(_) => ErrorCode::InternalError,
    }
}

pub(crate) struct AcpHttpError(AcpError);

impl IntoResponse for AcpHttpError {
    fn into_response(self) -> Response {
        let status = status_for(&self.0);
        let body = JsonRpcError {
            code: code_for(&self.0).code(),
            message: self.0.to_string(),
            data: None,
        };
        (status, Json(body)).into_response()
    }
}

impl From<AcpError> for AcpHttpError {
    fn from(e: AcpError) -> Self {
        Self(e)
    }
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// Read the idempotency identity off a request, if the client sent one.
///
/// An empty header value is passed through as `Some("")` rather than being
/// normalised to `None`: the ledger names it `InvalidIdentity` and the client
/// is told. Silently treating it as absent would answer a client that BELIEVES
/// it sent a key with unprotected behaviour.
fn idempotency_key(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(IDEMPOTENCY_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
}

async fn create_session<H: HttpHandler>(
    State(handler): State<Arc<H>>,
    headers: axum::http::HeaderMap,
    Json(req): Json<SessionCreateRequest>,
) -> Result<Json<SessionCreateResponse>, AcpHttpError> {
    let key = idempotency_key(&headers);
    Ok(Json(
        handler
            .create_session_idempotent(key.as_deref(), req)
            .await?,
    ))
}

async fn list_sessions<H: HttpHandler>(
    State(handler): State<Arc<H>>,
) -> Result<Json<SessionListResponse>, AcpHttpError> {
    Ok(Json(handler.list_sessions().await?))
}

async fn get_session<H: HttpHandler>(
    State(handler): State<Arc<H>>,
    Path(id): Path<String>,
) -> Result<Json<SessionGetResponse>, AcpHttpError> {
    Ok(Json(handler.get_session(id).await?))
}

async fn delete_session<H: HttpHandler>(
    State(handler): State<Arc<H>>,
    Path(id): Path<String>,
    headers: axum::http::HeaderMap,
) -> Result<StatusCode, AcpHttpError> {
    let key = idempotency_key(&headers);
    handler
        .delete_session_idempotent(key.as_deref(), id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Query parameters of a resume request.
///
/// Both are REQUIRED. A resume that omits the stream id is a bare position,
/// and a bare position is the thing `cursor.rs` exists to refuse — accepting
/// one here would reintroduce the silent wrong-stream resume at the transport
/// after the core had closed it.
#[derive(serde::Deserialize)]
struct ResumeQuery {
    stream_id: String,
    position: u64,
}

/// `GET /sessions/:id/events?stream_id=…&position=…` — serve a resume.
///
/// Each refusal gets its OWN status, because a client's correct next action
/// differs for each: re-subscribe from scratch (409, wrong stream), resynchronise
/// from `oldest_available` (410, evicted), or fix its own bookkeeping (400,
/// a position this stream never emitted).
async fn resume_events<H: HttpHandler>(
    State(handler): State<Arc<H>>,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<ResumeQuery>,
) -> Response {
    let cursor = Cursor {
        stream_id: q.stream_id,
        position: q.position,
    };
    match handler.resume_events(id, cursor).await {
        Ok(resp) => (StatusCode::OK, Json(resp)).into_response(),
        Err(e) => {
            let status = match &e {
                ResumeError::NoSuchSession { .. } => StatusCode::NOT_FOUND,
                ResumeError::Unsupported => StatusCode::NOT_IMPLEMENTED,
                ResumeError::Cursor(CursorError::StreamMismatch { .. }) => StatusCode::CONFLICT,
                ResumeError::Cursor(CursorError::TooOld { .. }) => StatusCode::GONE,
                ResumeError::Cursor(CursorError::Ahead { .. }) => StatusCode::BAD_REQUEST,
            };
            // The structured refusal travels in `data`, so a client can act on
            // the named case rather than parsing a sentence.
            let data = match &e {
                ResumeError::Cursor(c) => serde_json::to_value(c).ok(),
                _ => None,
            };
            let body = JsonRpcError {
                code: ErrorCode::InvalidRequest.code(),
                message: e.to_string(),
                data,
            };
            (status, Json(body)).into_response()
        }
    }
}

#[derive(serde::Deserialize)]
struct MessageBody {
    #[serde(default)]
    text: String,
    #[serde(default)]
    tools: Vec<crate::protocol::ToolDefinition>,
}

async fn send_message<H: HttpHandler>(
    State(handler): State<Arc<H>>,
    Path(id): Path<String>,
    Json(body): Json<MessageBody>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AcpHttpError> {
    let req = MessageSendRequest {
        session_id: id,
        text: body.text,
        tools: body.tools,
    };
    let stream = handler.send_message(req).await?;
    let sse_stream = stream.map(|ev| {
        let kind = match &ev {
            MessageEvent::Thinking { .. } => "thinking",
            MessageEvent::TextDelta { .. } => "text_delta",
            MessageEvent::ToolCall { .. } => "tool_call",
            MessageEvent::ApprovalRequired { .. } => "approval_required",
            MessageEvent::ToolResult { .. } => "tool_result",
            MessageEvent::Done { .. } => "done",
            MessageEvent::Error { .. } => "error",
        };
        let data = serde_json::to_string(&ev).unwrap_or_else(|e| {
            format!(
                r#"{{"kind":"error","error":{{"code":-32603,"message":"{}"}}}}"#,
                e
            )
        });
        Ok(Event::default().event(kind).data(data))
    });
    Ok(Sse::new(sse_stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15))))
}

/// `GET /initialize` — persona-profiles capability handshake (R2). Advertises
/// the server's [`crate::protocol::ServerCapabilities`] so clients can gate
/// version-sensitive features (e.g. the `agent` selector) before using them.
/// A client that advertises a version gets it NEGOTIATED; one that does not is
/// served exactly as before.
///
/// The negotiated answer travels in response HEADERS rather than in
/// [`InitializeResponse`], deliberately: the body is a published wire shape and
/// widening it to carry a negotiation outcome would move a contract for a
/// property that is per-connection, not per-server. A client below the floor is
/// REFUSED here — a silent downgrade would let it connect and fail later,
/// somewhere with no visible connection to a version.
async fn initialize<H: HttpHandler>(
    State(handler): State<Arc<H>>,
    headers: axum::http::HeaderMap,
) -> Response {
    let requested = headers
        .get(PROTOCOL_VERSION_HEADER)
        .and_then(|v| v.to_str().ok());

    let negotiated = match requested {
        None => None,
        Some(v) => match crate::negotiate::negotiate(v) {
            Ok(n) => Some(n),
            Err(e) => {
                let body = JsonRpcError {
                    code: ErrorCode::InvalidRequest.code(),
                    message: e.to_string(),
                    data: None,
                };
                return (StatusCode::BAD_REQUEST, Json(body)).into_response();
            }
        },
    };

    let resp = match handler.initialize().await {
        Ok(r) => r,
        Err(e) => return AcpHttpError(e).into_response(),
    };

    let mut out = Json(resp).into_response();
    if let Some(n) = negotiated {
        let h = out.headers_mut();
        if let Ok(v) = n.agreed.to_string().parse() {
            h.insert(NEGOTIATED_VERSION_HEADER, v);
        }
        if let Ok(v) = n.client_is_newer.to_string().parse() {
            h.insert(CLIENT_IS_NEWER_HEADER, v);
        }
    }
    out
}

/// `GET /agents` — persona-profiles agent roster (`agents/list`). Returns the
/// AUTHORIZED persona-agents (R3), each id/label-only (R4). Defaults to `[]`
/// when no roster is installed (feature default-OFF, backward-compatible).
async fn get_agents<H: HttpHandler>(
    State(handler): State<Arc<H>>,
) -> Result<Json<AgentsListResponse>, AcpHttpError> {
    Ok(Json(handler.list_agents().await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use futures::stream;
    use tower::ServiceExt;

    use crate::protocol::{SessionMetadata, ToolCall, ToolResult};

    struct MockHandler {
        fail_create: bool,
        fail_get_not_found: bool,
    }

    #[async_trait]
    impl HttpHandler for MockHandler {
        async fn create_session(
            &self,
            req: SessionCreateRequest,
        ) -> Result<SessionCreateResponse, AcpError> {
            if self.fail_create {
                return Err(AcpError::Auth("token expired".into()));
            }
            Ok(SessionCreateResponse {
                session_id: "sess-1".into(),
                model: req.model,
            })
        }

        async fn list_sessions(&self) -> Result<SessionListResponse, AcpError> {
            Ok(SessionListResponse {
                sessions: vec![SessionMetadata {
                    session_id: "sess-1".into(),
                    model: Some("claude-opus-4-7".into()),
                    created_at: 1700000000,
                    last_activity: 1700000100,
                    message_count: 3,
                }],
            })
        }

        async fn get_session(&self, session_id: String) -> Result<SessionGetResponse, AcpError> {
            if self.fail_get_not_found {
                return Err(AcpError::Session("session not found".into()));
            }
            Ok(SessionGetResponse {
                session: SessionMetadata {
                    session_id,
                    model: None,
                    created_at: 1700000000,
                    last_activity: 1700000000,
                    message_count: 0,
                },
            })
        }

        async fn delete_session(&self, _session_id: String) -> Result<(), AcpError> {
            Ok(())
        }

        async fn send_message(
            &self,
            _req: MessageSendRequest,
        ) -> Result<Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
            let events = vec![
                MessageEvent::TextDelta { text: "hi".into() },
                MessageEvent::Done {
                    stop_reason: "end_turn".into(),
                    turn_id: String::new(),
                },
            ];
            Ok(Box::pin(stream::iter(events)))
        }
    }

    fn router(fail_create: bool, fail_get_not_found: bool) -> Router {
        let handler = Arc::new(MockHandler {
            fail_create,
            fail_get_not_found,
        });
        HttpSseTransport::new(handler).router()
    }

    #[tokio::test]
    async fn post_sessions_creates() {
        let app = router(false, false);
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"model":"claude-opus-4-7"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: SessionCreateResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.session_id, "sess-1");
        assert_eq!(parsed.model.as_deref(), Some("claude-opus-4-7"));
    }

    #[tokio::test]
    async fn get_sessions_lists() {
        let app = router(false, false);
        let req = Request::builder()
            .method("GET")
            .uri("/sessions")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: SessionListResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.sessions.len(), 1);
        assert_eq!(parsed.sessions[0].session_id, "sess-1");
    }

    #[tokio::test]
    async fn get_session_by_id() {
        let app = router(false, false);
        let req = Request::builder()
            .method("GET")
            .uri("/sessions/abc")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: SessionGetResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.session.session_id, "abc");
    }

    #[tokio::test]
    async fn delete_session_returns_204() {
        let app = router(false, false);
        let req = Request::builder()
            .method("DELETE")
            .uri("/sessions/abc")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn post_messages_returns_sse() {
        let app = router(false, false);
        let req = Request::builder()
            .method("POST")
            .uri("/sessions/abc/messages")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"text":"hello"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(
            ct.starts_with("text/event-stream"),
            "expected SSE content-type, got {ct}"
        );
        let body = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert!(
            text.contains("event: text_delta"),
            "missing text_delta: {text}"
        );
        assert!(text.contains("event: done"), "missing done: {text}");
    }

    #[tokio::test]
    async fn auth_error_maps_to_401() {
        let app = router(true, false);
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from(r#"{}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: JsonRpcError = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.code, ErrorCode::AuthRequired.code());
    }

    #[tokio::test]
    async fn session_not_found_maps_to_404() {
        let app = router(false, true);
        let req = Request::builder()
            .method("GET")
            .uri("/sessions/missing")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn bad_path_is_404() {
        let app = router(false, false);
        let req = Request::builder()
            .method("GET")
            .uri("/nope")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn bad_json_body_is_400() {
        let app = router(false, false);
        let req = Request::builder()
            .method("POST")
            .uri("/sessions")
            .header("content-type", "application/json")
            .body(Body::from("not json"))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(
            resp.status() == StatusCode::BAD_REQUEST
                || resp.status() == StatusCode::UNPROCESSABLE_ENTITY,
            "expected 4xx, got {}",
            resp.status()
        );
    }

    // persona-profiles Phase A: the roster route defaults to `[]` for a
    // handler that does not install a roster (backward-compatible).
    #[tokio::test]
    async fn get_agents_defaults_to_empty() {
        let app = router(false, false);
        let req = Request::builder()
            .method("GET")
            .uri("/agents")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: AgentsListResponse = serde_json::from_slice(&body).unwrap();
        assert!(parsed.agents.is_empty(), "no roster ⇒ empty agents list");
    }

    // persona-profiles Phase A: the capability handshake is reachable and
    // returns a protocol version + capability set (R2).
    #[tokio::test]
    async fn get_initialize_returns_handshake() {
        let app = router(false, false);
        let req = Request::builder()
            .method("GET")
            .uri("/initialize")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: InitializeResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed.protocol_version, ACP_PROTOCOL_VERSION);
        // The bare MockHandler inherits the conservative default (no
        // capability); AcpServer overrides to advertise `agent_selection`
        // (asserted in server.rs tests).
        assert!(!parsed.capabilities.agent_selection);
    }

    // Silence unused-import warnings for protocol types referenced only
    // through trait impls in this test module.
    #[allow(dead_code)]
    fn _type_check() -> (ToolCall, ToolResult) {
        (
            ToolCall {
                id: "x".into(),
                name: "y".into(),
                input: serde_json::json!({}),
            },
            ToolResult {
                call_id: "x".into(),
                output: serde_json::json!({}),
                is_error: false,
            },
        )
    }
}
