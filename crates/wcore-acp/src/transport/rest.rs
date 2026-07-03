//! REST + OpenAPI transport for the agent engine.
//!
//! A versioned (`/v1`) REST surface over the SAME [`HttpHandler`] engine
//! bridge that backs the ACP HTTP/SSE transport ([`super::http`]). It does
//! NOT introduce a second engine binding: both transports hold an
//! `Arc<H: HttpHandler>` over one `AcpServer`, so the engine wire-up
//! (`EngineTurnEngine`, installed by `wcore-cli`) is inherited for free.
//!
//! Routes (see [`ApiDoc`] for the generated OpenAPI document):
//!   POST   /v1/sessions              create_session
//!   GET    /v1/sessions              list_sessions
//!   GET    /v1/sessions/{id}         get_session
//!   DELETE /v1/sessions/{id}         delete_session
//!   POST   /v1/sessions/{id}/prompt  send_message  (SSE: text/event-stream)
//!   GET    /v1/tools                 list_tools
//!   GET    /v1/health                liveness
//!   GET    /openapi.json             the OpenAPI document (unauthenticated)
//!   GET    /doc                      embedded spec viewer, HTML (unauthenticated)
//!
//! Auth + CORS mirror [`super::http`] exactly (F-017): optional `X-API-Key`
//! verifier installed by `wcore-cli`; `/openapi.json` + `/doc` are a public
//! carve-out (spec discovery is not sensitive), every `/v1/*` route is gated
//! when a verifier is present.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use axum::extract::{Path, Request, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::stream::{Stream, StreamExt};
use serde::Serialize;
use utoipa::OpenApi;

use crate::auth::Verifier;
use crate::error::AcpError;
use crate::protocol::{
    ErrorCode, JsonRpcError, MessageEvent, MessageSendRequest, SessionCreateRequest,
    SessionCreateResponse, SessionGetResponse, SessionListResponse, ToolDefinition,
};
// Reuse the engine bridge and the HTTP error→status mapping from the ACP
// transport — no second engine binding, no duplicated `status_for`/`code_for`.
use crate::transport::http::{AcpHttpError, HttpHandler};

// ── Extra response bodies unique to REST (not in the ACP protocol) ────────

/// `GET /v1/health` body.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct HealthResponse {
    /// Always `"ok"` when the process is serving.
    pub status: String,
    /// Crate version (`env!("CARGO_PKG_VERSION")`).
    pub version: String,
}

/// `GET /v1/tools` body — the tools the engine advertises.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ToolsResponse {
    pub tools: Vec<ToolDefinition>,
}

/// Request body for `POST /v1/sessions/{id}/prompt`. Mirrors the ACP
/// `MessageBody` shape (text + optional tool overrides) but documented for
/// OpenAPI. The `session_id` comes from the path, not the body.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
pub struct PromptBody {
    pub text: String,
    #[serde(default)]
    pub tools: Vec<ToolDefinition>,
}

/// Request body for `POST /v1/sessions/{id}/approvals/{call_id}/resolve`
/// (Blocker #2). Answers an `ApprovalRequired` gate the host saw on the
/// prompt SSE stream. `session_id` + `call_id` come from the path.
///
/// `scope` (default `once`) selects auto-approval persistence:
///   * `once` — approve only this call;
///   * `always` — approve and auto-approve this tool name from now on;
///   * `always_prefix` — approve and persist a prefix rule (`prefix` required).
///
/// `scope` is ignored when `approved` is `false`.
#[derive(Debug, Clone, serde::Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ApprovalResolveRequest {
    /// `true` to approve the gated tool call, `false` to deny it.
    pub approved: bool,
    /// Auto-approval persistence scope. Defaults to `once`.
    #[serde(default)]
    pub scope: ApprovalScopeDto,
    /// Prefix for `always_prefix` scope (required only then; ignored
    /// otherwise).
    #[serde(default)]
    pub prefix: Option<String>,
    /// Optional answer payload (e.g. an AskUserQuestion choice) threaded back
    /// through the approval channel.
    #[serde(default)]
    pub answer: Option<String>,
    /// GHSA-8r7g M2 (wayland#568) — the SECRET `resume_token` copied from the
    /// matching `ApprovalRequired` frame. REQUIRED to resolve a BRIDGE-backed
    /// gate (Crucible council / egress consent); OMIT for a manager-gated tool
    /// (ordinary approve/deny, resolved by the path `call_id`). A stale or
    /// unknown token falls through to the manager path, then 404s if that also
    /// misses — the endpoint stays idempotent.
    #[serde(default)]
    pub resume_token: Option<String>,
}

/// Wire spelling of the approval scope. Flat string enum so the OpenAPI
/// schema is a simple `enum`, decoupled from the un-`Serialize`-able
/// `wcore-protocol` `ApprovalScope`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalScopeDto {
    #[default]
    Once,
    Always,
    AlwaysPrefix,
}

/// Response body for the approval-resolve endpoint.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ApprovalResolveResponse {
    /// Always `true` on a 200 (a non-resolvable id returns 404, not
    /// `resolved: false`).
    pub resolved: bool,
    /// Echoes the resolved `call_id`.
    pub call_id: String,
    /// `"approved"` or `"denied"`.
    pub decision: String,
}

/// Optional REST extension to the engine bridge: advertise tools.
///
/// Default impl returns an empty list so existing [`HttpHandler`]
/// implementors (e.g. `AcpServer`) need no change until the engine exposes
/// its tool catalog. When that lands, `AcpServer` overrides this to project
/// the real tool registry — the route stays honest (`[]`, not a lie) in the
/// meantime.
#[async_trait]
pub trait RestExt: HttpHandler {
    async fn list_tools(&self) -> Result<ToolsResponse, AcpError> {
        Ok(ToolsResponse { tools: Vec::new() })
    }
}

// Blanket impl: every `HttpHandler` gets the default tool list for free.
#[async_trait]
impl<H: HttpHandler> RestExt for H {}

// ── Transport ─────────────────────────────────────────────────────────────

/// REST/OpenAPI transport. Twin of `HttpSseTransport` over the same
/// `Arc<H: HttpHandler>` engine bridge.
pub struct RestTransport<H: HttpHandler> {
    handler: Arc<H>,
    verifier: Option<Arc<dyn Verifier>>,
}

impl<H: HttpHandler> RestTransport<H> {
    pub fn new(handler: Arc<H>) -> Self {
        Self {
            handler,
            verifier: None,
        }
    }

    /// Install an auth verifier (F-017). Same contract as
    /// [`super::http::HttpSseTransport::with_verifier`].
    pub fn with_verifier(mut self, v: Arc<dyn Verifier>) -> Self {
        self.verifier = Some(v);
        self
    }

    /// Build the REST `Router`. `/openapi.json` and `/doc` are served
    /// UNAUTHENTICATED (spec discovery is not sensitive); all `/v1/*` routes
    /// go through the auth middleware when a verifier is present.
    pub fn router(&self) -> Router {
        // Authenticated API surface.
        let api = Router::new()
            .route(
                "/v1/sessions",
                post(create_session::<H>).get(list_sessions::<H>),
            )
            .route(
                "/v1/sessions/:id",
                get(get_session::<H>).delete(delete_session::<H>),
            )
            .route("/v1/sessions/:id/prompt", post(prompt::<H>))
            .route(
                "/v1/sessions/:id/approvals/:call_id/resolve",
                post(resolve_approval::<H>),
            )
            .route("/v1/tools", get(list_tools::<H>))
            .route("/v1/health", get(health))
            .with_state(self.handler.clone());

        let api = if let Some(v) = self.verifier.clone() {
            api.layer(middleware::from_fn(move |req: Request, next: Next| {
                let v = Arc::clone(&v);
                async move { auth_middleware(v, req, next).await }
            }))
        } else {
            api
        };

        // Spec routes are stateless + unauthenticated; merge them in.
        Router::new()
            .route("/openapi.json", get(openapi_json))
            .route("/doc", get(doc_ui))
            .merge(api)
    }
}

// ── OpenAPI document (utoipa, generated at compile time) ──────────────────

#[derive(OpenApi)]
#[openapi(
    info(
        title = "wayland-core agent REST API",
        description = "REST + SSE surface over the wayland-core agent engine."
    ),
    paths(
        create_session,
        list_sessions,
        get_session,
        delete_session,
        prompt,
        resolve_approval,
        list_tools,
        health,
    ),
    components(schemas(
        SessionCreateRequest,
        SessionCreateResponse,
        SessionListResponse,
        SessionGetResponse,
        MessageSendRequest,
        MessageEvent,
        ToolDefinition,
        ToolsResponse,
        HealthResponse,
        PromptBody,
        ApprovalResolveRequest,
        ApprovalResolveResponse,
        ApprovalScopeDto,
        JsonRpcError,
        crate::protocol::SessionMetadata,
        crate::protocol::ToolCall,
        crate::protocol::ToolResult,
    )),
    tags((name = "sessions", description = "Session lifecycle + prompting"))
)]
pub struct ApiDoc;

/// `GET /openapi.json` — the OpenAPI document as JSON.
///
/// utoipa 4.x emits OpenAPI 3.0.3. The mainstream SDK generators
/// (`openapi-generator`, Speakeasy, `openapi-typescript`) consume it as-is;
/// a strict 3.1 consumer requires the repo-wide axum-0.8 + utoipa-5 bump,
/// tracked as a separate decision.
async fn openapi_json() -> impl IntoResponse {
    Json(ApiDoc::openapi())
}

/// `GET /doc` — a zero-dependency, fully hermetic spec viewer.
///
/// It fetches `/openapi.json` at runtime and renders a readable summary of
/// every documented path with vanilla inline JS — NO external CDN script, NO
/// Subresource-Integrity hashes to keep current, NO vendored multi-MB asset.
/// This honours the offline-capable-CLI requirement: the page works with zero
/// network beyond the server it is served from.
async fn doc_ui() -> impl IntoResponse {
    Html(DOC_HTML)
}

const DOC_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>wayland-core agent REST API</title>
<style>
  :root { color-scheme: light dark; }
  body { font: 14px/1.5 system-ui, sans-serif; margin: 0; padding: 2rem;
         max-width: 60rem; margin-inline: auto; }
  h1 { font-size: 1.4rem; margin-bottom: .25rem; }
  .sub { opacity: .7; margin-bottom: 1.5rem; }
  .op { border: 1px solid currentColor; border-radius: 6px; padding: .75rem 1rem;
        margin-bottom: .75rem; opacity: .92; }
  .method { display: inline-block; font-weight: 700; font-family: ui-monospace, monospace;
            padding: .1rem .5rem; border-radius: 4px; margin-right: .5rem; }
  .path { font-family: ui-monospace, monospace; }
  .desc { opacity: .75; margin-top: .35rem; }
  a { color: inherit; }
  code { font-family: ui-monospace, monospace; }
</style>
</head>
<body>
<h1>wayland-core agent REST API</h1>
<div class="sub">Raw OpenAPI document: <a href="/openapi.json">/openapi.json</a></div>
<div id="ops">Loading spec&hellip;</div>
<script>
(async () => {
  const root = document.getElementById('ops');
  try {
    const res = await fetch('/openapi.json');
    const spec = await res.json();
    const paths = spec.paths || {};
    const frag = document.createDocumentFragment();
    for (const [p, methods] of Object.entries(paths)) {
      for (const [m, op] of Object.entries(methods)) {
        const div = document.createElement('div');
        div.className = 'op';
        const meth = document.createElement('span');
        meth.className = 'method';
        meth.textContent = m.toUpperCase();
        const path = document.createElement('span');
        path.className = 'path';
        path.textContent = p;
        div.append(meth, path);
        const summary = op.summary || op.description || '';
        if (summary) {
          const d = document.createElement('div');
          d.className = 'desc';
          d.textContent = summary;
          div.append(d);
        }
        frag.append(div);
      }
    }
    root.replaceChildren(frag);
  } catch (e) {
    root.textContent = 'Failed to load /openapi.json: ' + e;
  }
})();
</script>
</body>
</html>"##;

// ── Auth middleware (mirrors super::http) ─────────────────────────────────

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

// ── Handlers ───────────────────────────────────────────────────────────────

#[utoipa::path(
    post, path = "/v1/sessions", tag = "sessions",
    request_body = SessionCreateRequest,
    responses((status = 200, description = "Session created", body = SessionCreateResponse))
)]
async fn create_session<H: HttpHandler>(
    State(h): State<Arc<H>>,
    Json(req): Json<SessionCreateRequest>,
) -> Result<Json<SessionCreateResponse>, AcpHttpError> {
    Ok(Json(h.create_session(req).await?))
}

#[utoipa::path(
    get, path = "/v1/sessions", tag = "sessions",
    responses((status = 200, description = "All sessions", body = SessionListResponse))
)]
async fn list_sessions<H: HttpHandler>(
    State(h): State<Arc<H>>,
) -> Result<Json<SessionListResponse>, AcpHttpError> {
    Ok(Json(h.list_sessions().await?))
}

#[utoipa::path(
    get, path = "/v1/sessions/{id}", tag = "sessions",
    params(("id" = String, Path, description = "Session id")),
    responses(
        (status = 200, description = "Session metadata", body = SessionGetResponse),
        (status = 404, description = "Not found", body = JsonRpcError)
    )
)]
async fn get_session<H: HttpHandler>(
    State(h): State<Arc<H>>,
    Path(id): Path<String>,
) -> Result<Json<SessionGetResponse>, AcpHttpError> {
    Ok(Json(h.get_session(id).await?))
}

#[utoipa::path(
    delete, path = "/v1/sessions/{id}", tag = "sessions",
    params(("id" = String, Path, description = "Session id")),
    responses(
        (status = 204, description = "Deleted"),
        (status = 404, description = "Not found", body = JsonRpcError)
    )
)]
async fn delete_session<H: HttpHandler>(
    State(h): State<Arc<H>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AcpHttpError> {
    h.delete_session(id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(
    post, path = "/v1/sessions/{id}/prompt", tag = "sessions",
    params(("id" = String, Path, description = "Session id")),
    request_body = PromptBody,
    responses((
        status = 200,
        description = "SSE stream (text/event-stream). Each event `data:` is a \
                       JSON-encoded MessageEvent; the event name is the `kind` \
                       (thinking|text_delta|tool_call|tool_result|done|error).",
        content_type = "text/event-stream",
        body = MessageEvent
    ))
)]
async fn prompt<H: HttpHandler>(
    State(h): State<Arc<H>>,
    Path(id): Path<String>,
    Json(body): Json<PromptBody>,
) -> Result<Sse<impl Stream<Item = Result<Event, std::convert::Infallible>>>, AcpHttpError> {
    let req = MessageSendRequest {
        session_id: id,
        text: body.text,
        tools: body.tools,
    };
    let stream = h.send_message(req).await?;
    // SSE encoder copied verbatim from `super::http::send_message` so it
    // type-checks identically against the same six `MessageEvent` variants.
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

#[utoipa::path(
    post, path = "/v1/sessions/{id}/approvals/{call_id}/resolve", tag = "sessions",
    params(
        ("id" = String, Path, description = "Session id"),
        ("call_id" = String, Path, description = "The ToolCall.id from the ApprovalRequired frame")
    ),
    request_body = ApprovalResolveRequest,
    responses(
        (status = 200, description = "Approval resolved", body = ApprovalResolveResponse),
        (status = 404, description = "Unknown session, or unknown/already-resolved/expired call_id", body = JsonRpcError)
    )
)]
async fn resolve_approval<H: HttpHandler>(
    State(h): State<Arc<H>>,
    Path((session_id, call_id)): Path<(String, String)>,
    Json(body): Json<ApprovalResolveRequest>,
) -> Result<Json<ApprovalResolveResponse>, AcpHttpError> {
    // Map the flat wire scope onto the transport-neutral engine decision. The
    // bridge crate has no wcore-protocol dep; the CLI layer maps this onto the
    // real ApprovalScope when it reaches the approval manager.
    let scope = match body.scope {
        ApprovalScopeDto::Once => crate::turn::ApprovalScopeWire::Once,
        ApprovalScopeDto::Always => crate::turn::ApprovalScopeWire::Always,
        ApprovalScopeDto::AlwaysPrefix => crate::turn::ApprovalScopeWire::AlwaysPrefix {
            prefix: body.prefix.unwrap_or_default(),
        },
    };
    let decision = crate::turn::ApprovalDecision {
        approved: body.approved,
        scope,
        answer: body.answer,
        resume_token: body.resume_token,
    };
    // Idempotency + 404: an unknown session or unknown/already-resolved/
    // expired call_id surfaces as `AcpError::Session("... not found ...")`,
    // which `status_for` maps to 404 — so a double resolve is a clean 404,
    // never a panic or a phantom 200.
    h.resolve_approval(session_id, call_id.clone(), decision)
        .await?;
    Ok(Json(ApprovalResolveResponse {
        resolved: true,
        call_id,
        decision: if body.approved { "approved" } else { "denied" }.to_string(),
    }))
}

#[utoipa::path(
    get, path = "/v1/tools", tag = "sessions",
    responses((status = 200, description = "Advertised tools", body = ToolsResponse))
)]
async fn list_tools<H: HttpHandler>(
    State(h): State<Arc<H>>,
) -> Result<Json<ToolsResponse>, AcpHttpError> {
    // The `RestExt` blanket impl gives every `HttpHandler` a default (empty)
    // tool list until the engine exposes its catalog.
    Ok(Json(h.list_tools().await?))
}

#[utoipa::path(
    get, path = "/v1/health", tag = "sessions",
    responses((status = 200, description = "Liveness", body = HealthResponse))
)]
async fn health() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{Body, to_bytes};
    use axum::http::Request as HttpRequest;
    use futures::stream;
    use tower::ServiceExt;

    use crate::protocol::SessionMetadata;

    /// Mirrors the private `MockHandler` in `super::http`'s test module:
    /// streams `[TextDelta, Done]`, and can be told to fail `get_session`
    /// with a "not found" error to exercise the shared error→status table.
    struct MockHandler {
        fail_get_not_found: bool,
    }

    /// The call_id the `MockHandler` treats as a live pending gate; any other
    /// id resolves to a not-found (404) — mirroring the real engine's
    /// presence-based 200/404 contract.
    const PENDING_CALL_ID: &str = "c1";

    #[async_trait]
    impl HttpHandler for MockHandler {
        async fn create_session(
            &self,
            req: SessionCreateRequest,
        ) -> Result<SessionCreateResponse, AcpError> {
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
                    created_at: 1_700_000_000,
                    last_activity: 1_700_000_100,
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
                    created_at: 1_700_000_000,
                    last_activity: 1_700_000_000,
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
        ) -> Result<std::pin::Pin<Box<dyn Stream<Item = MessageEvent> + Send>>, AcpError> {
            let events = vec![
                MessageEvent::TextDelta { text: "hi".into() },
                MessageEvent::Done {
                    stop_reason: "end_turn".into(),
                },
            ];
            Ok(Box::pin(stream::iter(events)))
        }

        async fn resolve_approval(
            &self,
            _session_id: String,
            call_id: String,
            _decision: crate::turn::ApprovalDecision,
        ) -> Result<(), AcpError> {
            // Model the engine's presence contract: only the one known
            // pending id resolves; anything else is a not-found (→ 404),
            // which is also the idempotent second-resolve path.
            if call_id == PENDING_CALL_ID {
                Ok(())
            } else {
                Err(AcpError::Session(format!("approval not found: {call_id}")))
            }
        }
    }

    fn router(fail_get_not_found: bool) -> Router {
        RestTransport::new(Arc::new(MockHandler { fail_get_not_found })).router()
    }

    /// A trivial always-fail verifier to exercise the auth carve-out (T-C8).
    struct DenyVerifier;
    impl Verifier for DenyVerifier {
        fn verify(
            &self,
            _headers: &[(String, String)],
        ) -> Result<crate::auth::Principal, AcpError> {
            Err(AcpError::Auth("missing X-API-Key".into()))
        }
    }

    // ── T-C1 ───────────────────────────────────────────────────────────
    #[tokio::test]
    async fn post_v1_sessions_creates() {
        let app = router(false);
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/v1/sessions")
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

    // ── T-C2 ───────────────────────────────────────────────────────────
    #[tokio::test]
    async fn list_get_delete_sessions() {
        let app = router(false);
        let resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let listed: SessionListResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(listed.sessions.len(), 1);

        let resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/sessions/abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let got: SessionGetResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(got.session.session_id, "abc");

        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .method("DELETE")
                    .uri("/v1/sessions/abc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    // ── T-C3 (SSE round-trip) ──────────────────────────────────────────
    #[tokio::test]
    async fn post_prompt_returns_sse() {
        let app = router(false);
        let req = HttpRequest::builder()
            .method("POST")
            .uri("/v1/sessions/abc/prompt")
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

    // ── T-C4 ───────────────────────────────────────────────────────────
    #[tokio::test]
    async fn get_tools_returns_empty_default() {
        let app = router(false);
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["tools"], serde_json::json!([]));
    }

    // ── T-C5 ───────────────────────────────────────────────────────────
    #[tokio::test]
    async fn get_health_ok() {
        let app = router(false);
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["status"], "ok");
        assert!(parsed["version"].is_string());
    }

    // ── T-C6 (OpenAPI doc generates with all schemas resolvable) ───────
    #[tokio::test]
    async fn get_openapi_json_has_paths_and_resolves_schemas() {
        let app = router(false);
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 256 * 1024).await.unwrap();
        let doc: serde_json::Value = serde_json::from_slice(&body).unwrap();

        // Version + the two keystone paths are registered.
        assert!(
            doc["openapi"].as_str().unwrap().starts_with("3.0"),
            "openapi version: {}",
            doc["openapi"]
        );
        assert!(doc["paths"]["/v1/sessions"].is_object());
        assert!(doc["paths"]["/v1/sessions/{id}/prompt"].is_object());

        // No dangling $ref: every `#/components/schemas/<Name>` referenced
        // anywhere in the document must be defined under components.schemas.
        let schemas = doc["components"]["schemas"]
            .as_object()
            .expect("components.schemas present");
        let mut refs = Vec::new();
        collect_refs(&doc, &mut refs);
        for r in refs {
            if let Some(name) = r.strip_prefix("#/components/schemas/") {
                assert!(
                    schemas.contains_key(name),
                    "dangling $ref to undefined schema {name:?}; \
                     defined schemas: {:?}",
                    schemas.keys().collect::<Vec<_>>()
                );
            }
        }

        // SessionMetadata is reachable via SessionListResponse — it MUST be
        // present (the half-wired derive that had to be finished).
        assert!(
            schemas.contains_key("SessionMetadata"),
            "SessionMetadata schema missing"
        );
    }

    /// Recursively collect every `"$ref": "..."` string value in the doc.
    fn collect_refs(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::Object(map) => {
                for (k, val) in map {
                    if k == "$ref" {
                        if let Some(s) = val.as_str() {
                            out.push(s.to_string());
                        }
                    } else {
                        collect_refs(val, out);
                    }
                }
            }
            serde_json::Value::Array(arr) => {
                for item in arr {
                    collect_refs(item, out);
                }
            }
            _ => {}
        }
    }

    /// The same assertion driven directly off `ApiDoc::openapi()` (no HTTP),
    /// guarding a forgotten `#[utoipa::path]` registration.
    #[test]
    fn apidoc_serializes_with_all_documented_paths() {
        let doc = serde_json::to_value(ApiDoc::openapi()).unwrap();
        for p in [
            "/v1/sessions",
            "/v1/sessions/{id}",
            "/v1/sessions/{id}/prompt",
            "/v1/sessions/{id}/approvals/{call_id}/resolve",
            "/v1/tools",
            "/v1/health",
        ] {
            assert!(doc["paths"][p].is_object(), "missing path {p}");
        }
    }

    // ── T-C7 ───────────────────────────────────────────────────────────
    #[tokio::test]
    async fn get_doc_returns_html() {
        let app = router(false);
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/doc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let ct = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert!(ct.starts_with("text/html"), "expected HTML, got {ct}");
    }

    // ── T-C8 (auth carve-out) ──────────────────────────────────────────
    #[tokio::test]
    async fn auth_gates_v1_but_not_spec() {
        let app = RestTransport::new(Arc::new(MockHandler {
            fail_get_not_found: false,
        }))
        .with_verifier(Arc::new(DenyVerifier))
        .router();

        // /v1/* without a valid key → 401.
        let resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

        // /openapi.json without a key → 200 (public carve-out).
        let resp = app
            .clone()
            .oneshot(
                HttpRequest::builder()
                    .uri("/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);

        // /doc without a key → 200 (public carve-out).
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/doc")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── T-C9 (shared error→status table) ───────────────────────────────
    #[tokio::test]
    async fn get_session_not_found_maps_to_404() {
        let app = router(true);
        let resp = app
            .oneshot(
                HttpRequest::builder()
                    .uri("/v1/sessions/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── Blocker #2: approval-resolve endpoint ───────────────────────────

    fn resolve_req(call_id: &str, body: &str) -> HttpRequest<Body> {
        HttpRequest::builder()
            .method("POST")
            .uri(format!("/v1/sessions/sess-1/approvals/{call_id}/resolve"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    /// Approve a live pending gate → 200, resolved=true, decision="approved".
    #[tokio::test]
    async fn post_approval_resolve_approves_pending_call() {
        let app = router(false);
        let resp = app
            .oneshot(resolve_req(PENDING_CALL_ID, r#"{"approved":true}"#))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["resolved"], true);
        assert_eq!(parsed["call_id"], PENDING_CALL_ID);
        assert_eq!(parsed["decision"], "approved");
    }

    /// Deny a live pending gate → 200, decision="denied".
    #[tokio::test]
    async fn post_approval_resolve_deny_path() {
        let app = router(false);
        let resp = app
            .oneshot(resolve_req(
                PENDING_CALL_ID,
                r#"{"approved":false,"answer":"no thanks"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = to_bytes(resp.into_body(), 4096).await.unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(parsed["resolved"], true);
        assert_eq!(parsed["decision"], "denied");
    }

    /// Always scope is accepted (deserializes + maps without error) → 200.
    #[tokio::test]
    async fn post_approval_resolve_accepts_always_scope() {
        let app = router(false);
        let resp = app
            .oneshot(resolve_req(
                PENDING_CALL_ID,
                r#"{"approved":true,"scope":"always"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    /// Unknown / already-resolved / expired call_id → 404 (idempotency
    /// contract: a second resolve of the same id lands here, never panics).
    #[tokio::test]
    async fn post_approval_resolve_missing_call_returns_404() {
        let app = router(false);
        let resp = app
            .oneshot(resolve_req("does-not-exist", r#"{"approved":true}"#))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    /// The resolve route is behind the same X-API-Key auth as the rest of
    /// `/v1/*`: no key → 401, never reaching the handler.
    #[tokio::test]
    async fn post_approval_resolve_is_auth_gated() {
        let app = RestTransport::new(Arc::new(MockHandler {
            fail_get_not_found: false,
        }))
        .with_verifier(Arc::new(DenyVerifier))
        .router();
        let resp = app
            .oneshot(resolve_req(PENDING_CALL_ID, r#"{"approved":true}"#))
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    }
}
