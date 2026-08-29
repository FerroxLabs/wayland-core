//! ACP HTTP client.
//!
//! Talks to the [`crate::transport::HttpSseTransport`] endpoints
//! defined in 1.A.4. Stdio + WS clients land in follow-up tasks; HTTP
//! is the first client because the request/response framing is the
//! simplest — no muxing-by-id required, the transport pairs them
//! naturally over HTTP.
//!
//! Auth is bearer-token only here (matches the 1.A.8 auth layer's
//! `BearerVerifier`); add an API-key or OAuth path when the server
//! grows them.

use std::pin::Pin;
use std::time::Duration;

use futures::StreamExt;
use futures::stream::Stream;
use wcore_egress::EgressClient as HttpClient;

use crate::cursor::{Cursor, CursorError, ResumeResponse};
use crate::error::AcpError;
use crate::protocol::{
    ErrorCode, JsonRpcError, MessageEvent, MessageSendRequest, SessionCreateRequest,
    SessionCreateResponse, SessionGetResponse, SessionListResponse,
};

/// A resume the server declined to serve.
///
/// Carries the structured [`CursorError`] when the server named one, because
/// the client's correct next action differs per case and a sentence is not
/// something a program can branch on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeRefused {
    pub status: u16,
    pub message: String,
    pub cursor: Option<CursorError>,
}

/// The answer to a resume request.
///
/// A refusal is an ANSWER, not a communication failure — hence `Ok(Refused)`
/// rather than `Err`. Collapsing the two would make "the server told me my
/// cursor is from a dead stream" indistinguishable from "I could not reach the
/// server", and only one of those is worth retrying.
#[derive(Debug, Clone)]
pub enum ResumeOutcome {
    Served(ResumeResponse),
    Refused(ResumeRefused),
}

/// Default request timeout. Streaming endpoints (message/send) override
/// this with their own infinite-ish timeout since events arrive over
/// the open SSE connection.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Hard cap on the SSE reassembly buffer in [`parse_sse_events`]. The streaming
/// endpoint uses an effectively-infinite (24h) read timeout, so a hostile
/// server could stream delimiter-free bytes forever; bound the accumulator so
/// it fails the stream instead of OOMing. 4 MiB matches the MCP streamable-HTTP
/// SSE cap and exceeds any legitimate single ACP event frame.
const MAX_ACP_SSE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// ACP client over HTTP/SSE. Construct one per base URL + bearer
/// token; reuse it across requests so connection pooling kicks in.
#[derive(Debug, Clone)]
pub struct AcpClient {
    http: HttpClient,
    base_url: String,
    bearer: Option<String>,
    /// persona-profiles PR-7 — `X-API-Key` sent on every request. This is the
    /// auth an `acp serve` server enforces (`SimpleKeyVerifier`), so the profile
    /// SUPERVISOR uses it to talk to a child it spawned with a known key.
    api_key: Option<String>,
}

impl AcpClient {
    /// Build a client pointed at `base_url` (e.g. `http://127.0.0.1:8080`).
    /// No trailing slash; the request methods append the route segments.
    pub fn new(base_url: impl Into<String>) -> Result<Self, AcpError> {
        let http = HttpClient::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|e| AcpError::Transport(format!("build http client: {e}")))?;
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            bearer: None,
            api_key: None,
        })
    }

    /// Attach a bearer token applied to every subsequent request. Builder.
    pub fn with_bearer(mut self, token: impl Into<String>) -> Self {
        self.bearer = Some(token.into());
        self
    }

    /// persona-profiles PR-7 — attach an ACP server API key, sent as the
    /// `X-API-Key` header the `acp serve` `SimpleKeyVerifier` enforces. Builder.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    fn maybe_auth(
        &self,
        req: wcore_egress::EgressRequestBuilder,
    ) -> wcore_egress::EgressRequestBuilder {
        // X-API-Key (server auth) and bearer are independent; apply whichever is
        // configured. The supervisor uses X-API-Key to reach a child server.
        let req = match &self.api_key {
            Some(k) => req.header("X-API-Key", k),
            None => req,
        };
        match &self.bearer {
            Some(t) => req.bearer_auth(t),
            None => req,
        }
    }

    /// `POST /sessions` — create a new session.
    pub async fn create_session(
        &self,
        req: SessionCreateRequest,
    ) -> Result<SessionCreateResponse, AcpError> {
        let url = format!("{}/sessions", self.base_url);
        let resp = self
            .maybe_auth(self.http.post(url).json(&req))
            .send()
            .await
            .map_err(|e| AcpError::Transport(format!("create_session: {e}")))?;
        let resp = check(resp).await?;
        resp.json::<SessionCreateResponse>()
            .await
            .map_err(|e| AcpError::Transport(format!("create_session decode: {e}")))
    }

    /// `POST /sessions` under an idempotency identity.
    ///
    /// Retrying with the SAME key returns the SAME session rather than
    /// creating a second one. Separate from [`Self::create_session`] on
    /// purpose: sending the header is a promise the server must be able to
    /// keep, and a server that cannot refuses rather than silently creating
    /// two sessions for a caller that believed it was protected.
    pub async fn create_session_idempotent(
        &self,
        idempotency_key: &str,
        req: SessionCreateRequest,
    ) -> Result<SessionCreateResponse, AcpError> {
        let url = format!("{}/sessions", self.base_url);
        let resp = self
            .maybe_auth(
                self.http
                    .post(url)
                    .header(crate::transport::http::IDEMPOTENCY_HEADER, idempotency_key)
                    .json(&req),
            )
            .send()
            .await
            .map_err(|e| AcpError::Transport(format!("create_session: {e}")))?;
        let resp = check(resp).await?;
        resp.json::<SessionCreateResponse>()
            .await
            .map_err(|e| AcpError::Transport(format!("create_session decode: {e}")))
    }

    /// `DELETE /sessions/:id` under an idempotency identity. A repeat is a
    /// success, not the spurious not-found a bare retry would produce.
    pub async fn delete_session_idempotent(
        &self,
        idempotency_key: &str,
        session_id: &str,
    ) -> Result<(), AcpError> {
        let url = format!("{}/sessions/{session_id}", self.base_url);
        let resp = self
            .maybe_auth(
                self.http
                    .delete(url)
                    .header(crate::transport::http::IDEMPOTENCY_HEADER, idempotency_key),
            )
            .send()
            .await
            .map_err(|e| AcpError::Transport(format!("delete_session: {e}")))?;
        check(resp).await?;
        Ok(())
    }

    /// `GET /sessions` — list all sessions.
    pub async fn list_sessions(&self) -> Result<SessionListResponse, AcpError> {
        let url = format!("{}/sessions", self.base_url);
        let resp = self
            .maybe_auth(self.http.get(url))
            .send()
            .await
            .map_err(|e| AcpError::Transport(format!("list_sessions: {e}")))?;
        let resp = check(resp).await?;
        resp.json::<SessionListResponse>()
            .await
            .map_err(|e| AcpError::Transport(format!("list_sessions decode: {e}")))
    }

    /// `GET /sessions/:id` — fetch a single session.
    pub async fn get_session(&self, session_id: &str) -> Result<SessionGetResponse, AcpError> {
        let url = format!("{}/sessions/{session_id}", self.base_url);
        let resp = self
            .maybe_auth(self.http.get(url))
            .send()
            .await
            .map_err(|e| AcpError::Transport(format!("get_session: {e}")))?;
        let resp = check(resp).await?;
        resp.json::<SessionGetResponse>()
            .await
            .map_err(|e| AcpError::Transport(format!("get_session decode: {e}")))
    }

    /// `DELETE /sessions/:id` — delete a session.
    pub async fn delete_session(&self, session_id: &str) -> Result<(), AcpError> {
        let url = format!("{}/sessions/{session_id}", self.base_url);
        let resp = self
            .maybe_auth(self.http.delete(url))
            .send()
            .await
            .map_err(|e| AcpError::Transport(format!("delete_session: {e}")))?;
        check(resp).await?;
        Ok(())
    }

    /// `GET /sessions/:id/events` — ask what this cursor has not seen.
    ///
    /// This is the client half of the recovery contract. Without it the cursor
    /// module is a structure with no caller and a disconnected client has no
    /// way to find out what it missed — which is the difference between a
    /// contract that COULD let clients recover gaps and one where a client
    /// actually does.
    pub async fn resume_events(
        &self,
        session_id: &str,
        cursor: &Cursor,
    ) -> Result<ResumeOutcome, AcpError> {
        let url = format!("{}/sessions/{session_id}/events", self.base_url);
        let resp = self
            .maybe_auth(self.http.get(url).query(&[
                ("stream_id", cursor.stream_id.as_str()),
                ("position", &cursor.position.to_string()),
            ]))
            .send()
            .await
            .map_err(|e| AcpError::Transport(format!("resume_events: {e}")))?;
        let status = resp.status().as_u16();
        if resp.status().is_success() {
            return resp
                .json::<ResumeResponse>()
                .await
                .map(ResumeOutcome::Served)
                .map_err(|e| AcpError::Transport(format!("resume_events decode: {e}")));
        }
        let body = resp
            .json::<JsonRpcError>()
            .await
            .map_err(|e| AcpError::Transport(format!("resume_events error decode: {e}")))?;
        let cursor = body
            .data
            .clone()
            .and_then(|d| serde_json::from_value::<CursorError>(d).ok());
        Ok(ResumeOutcome::Refused(ResumeRefused {
            status,
            message: body.message,
            cursor,
        }))
    }

    /// `POST /sessions/:id/messages` — send a message; returns an SSE
    /// stream of [`MessageEvent`]s. Streaming requests bypass the
    /// default timeout (the connection stays open while the server
    /// emits events).
    pub async fn send_message(
        &self,
        req: MessageSendRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<MessageEvent, AcpError>> + Send>>, AcpError> {
        let url = format!("{}/sessions/{}/messages", self.base_url, req.session_id);
        let resp = self
            .maybe_auth(self.http.post(url).json(&req))
            .timeout(Duration::from_secs(60 * 60 * 24))
            .send()
            .await
            .map_err(|e| AcpError::Transport(format!("send_message: {e}")))?;
        let resp = check(resp).await?;

        // Parse the SSE byte stream into `MessageEvent` frames. Each
        // SSE event has the form `event: <name>\ndata: <json>\n\n`;
        // we read line-by-line and assemble. Box+pin so the returned
        // stream is `Unpin` for callers using `StreamExt::next()`.
        let byte_stream = resp.bytes_stream();
        Ok(Box::pin(parse_sse_events(byte_stream)))
    }
}

/// Turn a non-success response into the typed error the SERVER named.
///
/// # Why this reads the body's error CODE rather than the HTTP status
///
/// The status is a coarse projection the transport chose; the code is the
/// server's own classification. Deriving meaning from the status alone throws
/// that away, and F24-04 measured two places where it mattered: a role refusal
/// arrived as a generic transport failure, so a typed client's only available
/// reaction was to retry a refusal that can never succeed; and an idempotency
/// CONFLICT — "this key is bound to a different command" — arrived as an
/// anonymous `HTTP 400`, indistinguishable from a malformed body. Both are
/// structured on the server and both were being flattened here.
///
/// A body that is not a `JsonRpcError` at all falls back to the status, which
/// is then genuinely all the information there is.
async fn check(resp: reqwest::Response) -> Result<reqwest::Response, AcpError> {
    if resp.status().is_success() {
        return Ok(resp);
    }
    let status = resp.status();
    let fallback = || match status {
        reqwest::StatusCode::UNAUTHORIZED => AcpError::Auth(format!("HTTP {status}")),
        reqwest::StatusCode::FORBIDDEN => AcpError::Forbidden(format!("HTTP {status}")),
        reqwest::StatusCode::NOT_FOUND => AcpError::Session(format!("HTTP {status}")),
        _ => AcpError::Transport(format!("HTTP {status} from server")),
    };
    match resp.json::<JsonRpcError>().await {
        Ok(body) => {
            let msg = body.message;
            Err(match body.code {
                c if c == ErrorCode::AuthRequired.code() => AcpError::Auth(msg),
                c if c == ErrorCode::Forbidden.code() => AcpError::Forbidden(msg),
                c if c == ErrorCode::SessionNotFound.code() => AcpError::Session(msg),
                c if c == ErrorCode::AgentNotFound.code() => AcpError::Agent(msg),
                c if c == ErrorCode::InvalidRequest.code() => AcpError::Protocol(msg),
                _ => AcpError::Transport(format!("HTTP {status}: {msg}")),
            })
        }
        Err(_) => Err(fallback()),
    }
}

/// Parse an SSE byte stream into `MessageEvent` frames. The HTTP/SSE
/// transport emits one JSON-encoded `MessageEvent` per SSE event; we
/// don't care about event names, only `data:` lines carrying JSON.
fn parse_sse_events<S>(bytes: S) -> impl Stream<Item = Result<MessageEvent, AcpError>>
where
    S: Stream<Item = Result<bytes::Bytes, reqwest::Error>> + Send + 'static,
{
    use futures::stream;
    // Drive line-by-line parse via unfold; emit events as we find them.
    stream::unfold(
        (Box::pin(bytes), Vec::<u8>::new()),
        |(mut stream, mut buf)| async move {
            loop {
                // Try to extract a full event ("data: ...\n\n") from buf.
                if let Some(idx) = find_double_newline(&buf) {
                    let chunk = buf[..idx].to_vec();
                    let rest = buf[idx + 2..].to_vec();
                    let _ = std::mem::replace(&mut buf, rest);
                    if let Some(event) = parse_one_sse_block(&chunk) {
                        return Some((event, (stream, buf)));
                    }
                    // Empty heartbeat — keep looping.
                    continue;
                }
                // Bound the reassembly buffer. The HTTP/SSE endpoint sets a 24h
                // read timeout, so a hostile server streaming delimiter-free
                // bytes would otherwise accumulate without bound until the host
                // OOMs. Once the buffer exceeds the cap with no `\n\n` event
                // boundary in sight, fail the stream. Matches the MCP
                // streamable-HTTP SSE cap (`MAX_SSE_BUFFER_BYTES`, 4 MiB).
                if buf.len() > MAX_ACP_SSE_BUFFER_BYTES {
                    return Some((
                        Err(AcpError::Transport(format!(
                            "SSE reassembly buffer exceeded {MAX_ACP_SSE_BUFFER_BYTES} bytes \
                             without an event boundary — server is misbehaving"
                        ))),
                        (stream, buf),
                    ));
                }
                match stream.next().await {
                    Some(Ok(b)) => buf.extend_from_slice(&b),
                    Some(Err(e)) => {
                        return Some((
                            Err(AcpError::Transport(format!("sse read: {e}"))),
                            (stream, buf),
                        ));
                    }
                    None => return None,
                }
            }
        },
    )
}

fn find_double_newline(buf: &[u8]) -> Option<usize> {
    buf.windows(2).position(|w| w == b"\n\n")
}

fn parse_one_sse_block(block: &[u8]) -> Option<Result<MessageEvent, AcpError>> {
    // Scan lines, take the value(s) of `data:` field; concatenate them
    // per SSE spec, then JSON-decode.
    let text = std::str::from_utf8(block).ok()?;
    let mut data = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        }
    }
    if data.is_empty() {
        return None;
    }
    Some(serde_json::from_str::<MessageEvent>(&data).map_err(AcpError::Serde))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest};
    use crate::server::AcpServer;
    use crate::transport::HttpSseTransport;
    use crate::turn::{TurnEngine, TurnRequest};
    use futures::StreamExt;
    use std::sync::Arc;
    use tokio::net::TcpListener;

    /// Minimal turn engine that emits a single `Done`, so the client's
    /// `send_message` streaming path can be exercised without a real engine.
    /// (The server's `send_message` now requires an installed turn engine;
    /// without one it honestly returns "no turn engine installed".)
    struct DoneTurnEngine;

    #[async_trait::async_trait]
    impl TurnEngine for DoneTurnEngine {
        async fn run_turn(
            &self,
            _req: TurnRequest,
        ) -> Result<std::pin::Pin<Box<dyn futures::Stream<Item = MessageEvent> + Send>>, AcpError>
        {
            Ok(futures::stream::iter(vec![MessageEvent::Done {
                stop_reason: "end_turn".to_string(),
                turn_id: String::new(),
            }])
            .boxed())
        }
    }

    /// Spin up a real server + HTTP transport on an ephemeral port and
    /// return the base URL.
    async fn serve_real() -> (String, tokio::task::JoinHandle<()>) {
        let server = Arc::new(AcpServer::new().with_turn_engine(Arc::new(DoneTurnEngine)));
        let transport = HttpSseTransport::new(Arc::clone(&server));
        let app = transport.router();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn create_then_get_roundtrips_over_http() {
        let (base, _h) = serve_real().await;
        let client = AcpClient::new(&base).unwrap();
        let resp = client
            .create_session(SessionCreateRequest {
                model: Some("opus".into()),
                tools: Vec::new(),
                system_prompt: None,
                agent: None,
                cwd: None,
            })
            .await
            .unwrap();
        assert_eq!(resp.model.as_deref(), Some("opus"));
        let got = client.get_session(&resp.session_id).await.unwrap();
        assert_eq!(got.session.session_id, resp.session_id);
    }

    #[tokio::test]
    async fn list_returns_created_sessions() {
        let (base, _h) = serve_real().await;
        let client = AcpClient::new(&base).unwrap();
        for _ in 0..3 {
            client
                .create_session(SessionCreateRequest {
                    model: None,
                    tools: Vec::new(),
                    system_prompt: None,
                    agent: None,
                    cwd: None,
                })
                .await
                .unwrap();
        }
        let list = client.list_sessions().await.unwrap();
        assert_eq!(list.sessions.len(), 3);
    }

    #[tokio::test]
    async fn delete_then_get_returns_session_error() {
        let (base, _h) = serve_real().await;
        let client = AcpClient::new(&base).unwrap();
        let resp = client
            .create_session(SessionCreateRequest {
                model: None,
                tools: Vec::new(),
                system_prompt: None,
                agent: None,
                cwd: None,
            })
            .await
            .unwrap();
        client.delete_session(&resp.session_id).await.unwrap();
        let err = client
            .get_session(&resp.session_id)
            .await
            .expect_err("expected session-not-found");
        assert!(matches!(err, AcpError::Session(_)), "got {err:?}");
    }

    #[tokio::test]
    async fn send_message_streams_done_event() {
        let (base, _h) = serve_real().await;
        let client = AcpClient::new(&base).unwrap();
        let resp = client
            .create_session(SessionCreateRequest {
                model: None,
                tools: Vec::new(),
                system_prompt: None,
                agent: None,
                cwd: None,
            })
            .await
            .unwrap();
        let mut stream = client
            .send_message(MessageSendRequest {
                session_id: resp.session_id.clone(),
                text: "hi".to_string(),
                tools: Vec::new(),
            })
            .await
            .unwrap();
        let first = stream
            .next()
            .await
            .expect("at least one event")
            .expect("event ok");
        assert!(matches!(first, MessageEvent::Done { .. }), "got {first:?}");
    }

    /// F8 — a server streaming delimiter-free bytes past the reassembly cap
    /// must fail the stream with a Transport error rather than accumulating
    /// without bound (24h read timeout would otherwise OOM the process).
    #[tokio::test]
    async fn sse_parser_fails_on_unbounded_delimiterless_stream() {
        // Each chunk is 64 KiB of non-`\n` bytes; feed enough to exceed the cap.
        let chunk = bytes::Bytes::from(vec![b'x'; 64 * 1024]);
        let n = (MAX_ACP_SSE_BUFFER_BYTES / chunk.len()) + 2;
        let upstream =
            futures::stream::iter(std::iter::repeat_with(move || Ok(chunk.clone())).take(n));

        let mut events = Box::pin(parse_sse_events(upstream));
        let item = events
            .next()
            .await
            .expect("the parser must yield a terminal error item");
        match item {
            Err(AcpError::Transport(msg)) => {
                assert!(
                    msg.contains("reassembly buffer exceeded"),
                    "expected buffer-cap error, got: {msg}"
                );
            }
            other => panic!("expected a Transport buffer-cap error, got {other:?}"),
        }
    }
}
