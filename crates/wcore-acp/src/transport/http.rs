//! HTTP/SSE transport for ACP.
//!
//! Maps the ACP session/message surface onto a REST-ish HTTP API:
//!
//! - `POST   /sessions`              → `session/create`
//! - `GET    /sessions`              → `session/list`
//! - `GET    /sessions/:id`          → `session/get`
//! - `DELETE /sessions/:id`          → `session/delete`
//! - `POST   /sessions/:id/messages` → `message/send` (SSE stream of [`MessageEvent`])
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

use crate::auth::Verifier;
use crate::error::AcpError;
use crate::protocol::{
    ErrorCode, JsonRpcError, MessageEvent, MessageSendRequest, SessionCreateRequest,
    SessionCreateResponse, SessionGetResponse, SessionListResponse,
};

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
            .with_state(self.handler.clone());

        // F-017: wrap with auth middleware when a verifier is present.
        if let Some(v) = self.verifier.clone() {
            base.layer(middleware::from_fn(move |req: Request, next: Next| {
                let v = Arc::clone(&v);
                async move { auth_middleware(v, req, next).await }
            }))
        } else {
            base
        }
    }
}

/// Axum middleware: extract headers, run the verifier, 401 on failure.
/// (F-017)
async fn auth_middleware(verifier: Arc<dyn Verifier>, req: Request, next: Next) -> Response {
    let headers: Vec<(String, String)> = req
        .headers()
        .iter()
        .filter_map(|(k, v)| {
            let name = k.as_str().to_string();
            v.to_str().ok().map(|val| (name, val.to_string()))
        })
        .collect();

    match verifier.verify(&headers) {
        Ok(_) => next.run(req).await,
        Err(e) => {
            let body = JsonRpcError {
                code: ErrorCode::AuthRequired.code(),
                message: e.to_string(),
                data: None,
            };
            (StatusCode::UNAUTHORIZED, Json(body)).into_response()
        }
    }
}

// ── Error mapping ───────────────────────────────────────────────────────

fn status_for(err: &AcpError) -> StatusCode {
    match err {
        AcpError::Auth(_) => StatusCode::UNAUTHORIZED,
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

async fn create_session<H: HttpHandler>(
    State(handler): State<Arc<H>>,
    Json(req): Json<SessionCreateRequest>,
) -> Result<Json<SessionCreateResponse>, AcpHttpError> {
    Ok(Json(handler.create_session(req).await?))
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
) -> Result<StatusCode, AcpHttpError> {
    handler.delete_session(id).await?;
    Ok(StatusCode::NO_CONTENT)
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
