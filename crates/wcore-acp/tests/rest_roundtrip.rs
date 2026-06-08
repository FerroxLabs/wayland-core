//! T-C10 — REST transport end-to-end over a live `tokio` listener.
//!
//! Spins `RestTransport` over a `MockHandler` (streams `[TextDelta, Done]`)
//! on an ephemeral port, then exercises create → prompt(SSE) → delete against
//! the real HTTP stack with `reqwest`, asserting each SSE `data:` line
//! deserializes to a `MessageEvent`. This is the REST-prompt SSE round-trip
//! proof the impl plan lists (separate from the in-process `oneshot` unit
//! tests in `transport::rest`).

use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, Stream};

use wcore_acp::error::AcpError;
use wcore_acp::protocol::{
    MessageEvent, MessageSendRequest, SessionCreateRequest, SessionCreateResponse,
    SessionGetResponse, SessionListResponse, SessionMetadata,
};
use wcore_acp::transport::RestTransport;
use wcore_acp::transport::http::HttpHandler;

/// In-memory handler: a single fixed session, prompt streams `[TextDelta,
/// Done]`. Enough to drive the REST surface end-to-end without an engine.
struct MockHandler;

#[async_trait]
impl HttpHandler for MockHandler {
    async fn create_session(
        &self,
        req: SessionCreateRequest,
    ) -> Result<SessionCreateResponse, AcpError> {
        Ok(SessionCreateResponse {
            session_id: "sess-live".into(),
            model: req.model,
        })
    }

    async fn list_sessions(&self) -> Result<SessionListResponse, AcpError> {
        Ok(SessionListResponse {
            sessions: vec![SessionMetadata {
                session_id: "sess-live".into(),
                model: None,
                created_at: 1_700_000_000,
                last_activity: 1_700_000_000,
                message_count: 0,
            }],
        })
    }

    async fn get_session(&self, session_id: String) -> Result<SessionGetResponse, AcpError> {
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

#[tokio::test]
async fn rest_create_prompt_sse_delete_roundtrip() {
    let transport = RestTransport::new(Arc::new(MockHandler));
    let app = transport.router();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _serve = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Give the listener a moment to start accepting.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let base = format!("http://{addr}");
    #[allow(clippy::disallowed_methods)] // localhost roundtrip test; no proxy/timeout policy needed
    let client = reqwest::Client::new();

    // create
    let resp = client
        .post(format!("{base}/v1/sessions"))
        .json(&serde_json::json!({"model": "opus"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let created: SessionCreateResponse = resp.json().await.unwrap();
    assert_eq!(created.session_id, "sess-live");
    let id = created.session_id;

    // prompt (SSE) — read the whole body and parse each `data:` line.
    let resp = client
        .post(format!("{base}/v1/sessions/{id}/prompt"))
        .json(&serde_json::json!({"text": "hello"}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    assert!(
        ct.starts_with("text/event-stream"),
        "expected SSE, got {ct}"
    );
    let body = resp.text().await.unwrap();

    let mut events: Vec<MessageEvent> = Vec::new();
    for line in body.lines() {
        if let Some(payload) = line.strip_prefix("data:") {
            let payload = payload.trim();
            if payload.is_empty() {
                continue;
            }
            let ev: MessageEvent = serde_json::from_str(payload)
                .unwrap_or_else(|e| panic!("data line not a MessageEvent: {payload:?} ({e})"));
            events.push(ev);
        }
    }
    assert!(
        matches!(events.first(), Some(MessageEvent::TextDelta { .. })),
        "first frame should be TextDelta, got {events:?}"
    );
    assert!(
        matches!(events.last(), Some(MessageEvent::Done { .. })),
        "last frame should be Done, got {events:?}"
    );

    // delete
    let resp = client
        .delete(format!("{base}/v1/sessions/{id}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
}

#[tokio::test]
async fn rest_openapi_doc_served_over_live_listener() {
    let transport = RestTransport::new(Arc::new(MockHandler));
    let app = transport.router();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let _serve = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let base = format!("http://{addr}");
    #[allow(clippy::disallowed_methods)] // localhost roundtrip test; no proxy/timeout policy needed
    let client = reqwest::Client::new();

    let doc: serde_json::Value = client
        .get(format!("{base}/openapi.json"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(doc["openapi"].as_str().unwrap().starts_with("3.0"));
    assert!(doc["paths"]["/v1/sessions"].is_object());
    assert!(
        doc["components"]["schemas"]["SessionMetadata"].is_object(),
        "SessionMetadata schema must resolve"
    );
}
