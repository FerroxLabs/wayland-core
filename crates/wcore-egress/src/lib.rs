//! # wcore-egress — the single outbound-HTTP chokepoint
//!
//! Every outbound HTTP request in the workspace flows through one
//! [`EgressClient`]. This is the structural foundation of the injection /
//! exfiltration defense (SPEC Layer 1, build step B1): a transport-level gate,
//! not a hand-maintained per-tool allowlist.
//!
//! ## Why a chokepoint
//!
//! The data-exfiltration boundary cannot be enforced tool-by-tool — there are
//! ~30 independent HTTP clients across the channels, tool backends, cloud CLIs,
//! MCP transports, and providers. A single client type, plus a clippy
//! `disallowed-methods` lint that bans raw `reqwest::Client::new`/`builder`
//! outside this crate, makes it impossible to add an off-gate network call: a
//! missed migration site fails the lint and the build.
//!
//! ## B1 scope
//!
//! B1 establishes the **type and the seam**, with a pass-through
//! [`AllowAllPolicy`] default so behavior is byte-identical to today. The real
//! policy (empty-default allowlist, GET-with-data exfil class, taint-gated
//! `ask`-with-memory) lands in B2 by swapping the default policy — no call site
//! changes again.
//!
//! ## Usage
//!
//! ```no_run
//! use wcore_egress::EgressClient;
//! # async fn demo() -> Result<(), wcore_egress::EgressError> {
//! let client = EgressClient::tool(); // hardened timeouts + no redirects
//! let body = client
//!     .post("https://api.example.com/v1/thing")
//!     .body(r#"{"k":"v"}"#)
//!     .send()
//!     .await?
//!     .text()
//!     .await?;
//! # let _ = body;
//! # Ok(())
//! # }
//! ```

mod client;
mod error;
mod observer;
mod policy;
mod request;
mod url_allow;

pub use client::{
    CONNECT_TIMEOUT, EgressClient, EgressClientBuilder, READ_TIMEOUT, TOOL_REQUEST_TIMEOUT,
};
pub use error::{BeforeDispatchError, EgressError};
pub use observer::{
    BoundedEgressRecorder, EgressDestination, EgressEvent, EgressObserver, EgressOutcome,
    EgressRecorderSnapshot, EgressTransportErrorClass, GlobalDefaultObserver, NoopEgressObserver,
    SharedEgressObserver, global_observer_installed, install_global_observer,
};
pub use policy::{
    AllowAllPolicy, EgressDecision, EgressPolicy, GlobalDefaultPolicy, SharedPolicy,
    default_policy, global_policy_installed, install_global_policy, with_default_policy,
    with_default_policy_sync,
};
pub use request::EgressRequestBuilder;
pub use url_allow::host_in_allowlist;

// Re-export the reqwest surface that migrated call sites still need to name
// directly, so they do not have to keep a separate `reqwest` dependency just
// for these types.
pub use reqwest::{self, Body, Method, Response, Url, header, multipart, redirect};

/// Read a response body into memory with a hard byte cap, streaming chunk by
/// chunk so a server that lies about (or omits) `Content-Length` cannot OOM
/// the process. Rejects early when the declared `Content-Length` already
/// exceeds `max_bytes`, and aborts mid-stream the moment the accumulated
/// bytes would exceed it.
///
/// Use this for any fetch of attacker-influenced or unbounded-size media
/// (channel attachment downloads, etc.) instead of [`Response::bytes`], whose
/// unbounded buffering is an OOM-DoS vector on a chunked response with no
/// `Content-Length`.
pub async fn read_body_capped(
    mut resp: Response,
    max_bytes: usize,
) -> Result<Vec<u8>, EgressError> {
    if let Some(declared) = resp.content_length()
        && declared > max_bytes as u64
    {
        return Err(EgressError::BodyTooLarge { limit: max_bytes });
    }
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if buf.len().saturating_add(chunk.len()) > max_bytes {
            return Err(EgressError::BodyTooLarge { limit: max_bytes });
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    /// A policy that refuses everything — stand-in for B2's deny path, used to
    /// prove the gate short-circuits the network.
    #[derive(Debug)]
    struct DenyAll;
    #[async_trait::async_trait]
    impl EgressPolicy for DenyAll {
        async fn check(&self, _request: &reqwest::Request) -> EgressDecision {
            EgressDecision::Deny {
                reason: "denied by test policy".into(),
            }
        }
    }

    #[derive(Debug)]
    struct PendingPolicy {
        entered: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl EgressPolicy for PendingPolicy {
        async fn check(&self, _request: &reqwest::Request) -> EgressDecision {
            self.entered.notify_one();
            std::future::pending().await
        }
    }

    #[derive(Debug)]
    struct PanickingObserver;

    impl EgressObserver for PanickingObserver {
        fn observe(&self, _event: EgressEvent) {
            panic!("observer failure must not affect egress");
        }
    }

    #[test]
    fn presets_construct_without_panicking() {
        // The TLS backend initializes for every preset.
        let _ = EgressClient::new();
        let _ = EgressClient::streaming();
        let _ = EgressClient::tool();
    }

    #[tokio::test]
    async fn default_policy_is_allow_until_a_global_is_installed() {
        let client = EgressClient::tool();
        // The default client carries the global-proxy policy, which allows
        // until a real policy is installed (B1 behavior preserved).
        let url = "http://127.0.0.1:1/".parse::<reqwest::Url>().unwrap();
        let req = reqwest::Request::new(reqwest::Method::GET, url);
        assert!(matches!(
            client.policy().check(&req).await,
            EgressDecision::Allow
        ));
    }

    #[tokio::test]
    async fn streaming_client_does_not_follow_redirects() {
        // Parity with the old `http_client::build()` behavior: a 302 must be
        // surfaced, not followed (credential re-attach exfil vector).
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let resp = "HTTP/1.1 302 Found\r\nLocation: http://240.0.0.1:9/\r\nContent-Length: 0\r\n\r\n";
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });

        let client = EgressClient::streaming();
        let resp = client
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("request completes");
        assert_eq!(
            resp.status().as_u16(),
            302,
            "the client must surface the 302, not follow it"
        );
        server.abort();
    }

    #[tokio::test]
    async fn read_body_capped_rejects_oversize_and_accepts_within_cap() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        // Serve a 100-byte body (declared Content-Length) to each connection.
        let server = tokio::spawn(async move {
            loop {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let body = "x".repeat(100);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.flush().await;
                }
            }
        });

        let client = EgressClient::tool();

        // Declared length (100) over the cap (50) → early reject, body never buffered.
        let resp = client.get(format!("http://{addr}/")).send().await.unwrap();
        let err = read_body_capped(resp, 50)
            .await
            .expect_err("an oversize body must be rejected");
        assert!(
            matches!(err, EgressError::BodyTooLarge { limit: 50 }),
            "expected BodyTooLarge, got {err}"
        );

        // Within the cap → full body streamed back through the chunk loop.
        let resp = client.get(format!("http://{addr}/")).send().await.unwrap();
        let bytes = read_body_capped(resp, 200).await.expect("body within cap");
        assert_eq!(bytes.len(), 100);

        server.abort();
    }

    #[tokio::test]
    async fn tool_client_request_times_out_on_slow_drip() {
        // Parity with `http_client::build_tool_client()`: a request-level
        // timeout backstops a server that accepts then never replies.
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                std::future::pending::<()>().await;
            }
        });

        // Same construction path as `tool()`, with a fast TTL for the test.
        let recorder = Arc::new(BoundedEgressRecorder::new(2));
        let client = EgressClient::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .timeout(Duration::from_millis(200))
            .observer(recorder.clone())
            .build()
            .expect("client builds");

        let result = client.get(format!("http://{addr}/")).send().await;
        let err = result.expect_err("a slow-drip server must trip the timeout");
        assert!(
            err.is_timeout(),
            "the failure must be a timeout, got: {err}"
        );
        assert_eq!(
            recorder.snapshot().events[0].outcome,
            EgressOutcome::TransportError {
                class: EgressTransportErrorClass::Timeout,
            }
        );
        server.abort();
    }

    #[tokio::test]
    async fn deny_policy_blocks_before_the_request_is_sent() {
        // The gate's core guarantee: a Deny decision returns `Denied` and the
        // listener never sees a connection. We bind a listener, install
        // DenyAll, fire a request at it, and assert (a) the call returns
        // `Denied` and (b) no connection arrived within a short window.
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let dispatches = Arc::new(AtomicUsize::new(0));

        let client = EgressClient::tool().with_policy(Arc::new(DenyAll));
        let result = client
            .get(format!("http://{addr}/"))
            .before_dispatch({
                let dispatches = Arc::clone(&dispatches);
                move || {
                    let dispatches = Arc::clone(&dispatches);
                    async move {
                        dispatches.fetch_add(1, Ordering::SeqCst);
                        Ok::<(), &'static str>(())
                    }
                }
            })
            .send()
            .await;

        let err = result.expect_err("DenyAll must stop the request");
        assert!(err.is_denied(), "must be a policy denial, got: {err}");
        assert_eq!(dispatches.load(Ordering::SeqCst), 0);

        // No connection should have reached the listener — assert accept() does
        // not fire within a generous window.
        let accepted = tokio::time::timeout(Duration::from_millis(150), listener.accept()).await;
        assert!(
            accepted.is_err(),
            "a denied request must never reach the network"
        );
    }

    #[tokio::test]
    async fn before_dispatch_runs_once_after_allow_and_before_network_io() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        #[derive(Debug)]
        struct OrderedAllow {
            stage: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl EgressPolicy for OrderedAllow {
            async fn check(&self, _request: &reqwest::Request) -> EgressDecision {
                assert_eq!(
                    self.stage
                        .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst),
                    Ok(0),
                    "policy admission must precede the callback"
                );
                EgressDecision::Allow
            }
        }

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let stage = Arc::new(AtomicUsize::new(0));
        let server_stage = Arc::clone(&stage);
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("request reaches listener");
            assert_eq!(
                server_stage.load(Ordering::SeqCst),
                2,
                "callback must finish before physical dispatch"
            );
            let mut buffer = [0u8; 1024];
            let _ = socket.read(&mut buffer).await;
            socket
                .write_all(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n")
                .await
                .unwrap();
        });

        let response = EgressClient::tool()
            .with_policy(Arc::new(OrderedAllow {
                stage: Arc::clone(&stage),
            }))
            .get(format!("http://{addr}/"))
            .before_dispatch({
                let stage = Arc::clone(&stage);
                move || {
                    let stage = Arc::clone(&stage);
                    async move {
                        assert_eq!(
                            stage.compare_exchange(1, 2, Ordering::SeqCst, Ordering::SeqCst),
                            Ok(1),
                            "callback must run exactly once after admission"
                        );
                        Ok::<(), &'static str>(())
                    }
                }
            })
            .send()
            .await
            .expect("admitted request completes");

        assert_eq!(response.status().as_u16(), 204);
        assert_eq!(stage.load(Ordering::SeqCst), 2);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn before_dispatch_failure_prevents_network_io() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let invocations = Arc::new(AtomicUsize::new(0));
        let recorder = Arc::new(BoundedEgressRecorder::new(1));

        let error = EgressClient::tool()
            .with_observer(recorder.clone())
            .get(format!("http://{addr}/"))
            .before_dispatch({
                let invocations = Arc::clone(&invocations);
                move || {
                    let invocations = Arc::clone(&invocations);
                    async move {
                        invocations.fetch_add(1, Ordering::SeqCst);
                        Err::<(), _>("journal start was not durable")
                    }
                }
            })
            .send()
            .await
            .expect_err("callback failure must stop dispatch");

        assert!(error.is_before_dispatch());
        assert_eq!(invocations.load(Ordering::SeqCst), 1);
        assert!(
            tokio::time::timeout(Duration::from_millis(150), listener.accept())
                .await
                .is_err(),
            "callback failure must prevent network I/O"
        );
        assert_eq!(
            recorder.snapshot().events[0].outcome,
            EgressOutcome::BeforeDispatchFailed
        );
    }

    #[tokio::test]
    async fn allowed_request_reaches_a_real_server() {
        // Positive path: the default Allow policy lets a request through and the
        // response body round-trips.
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let resp = "HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });

        let client = EgressClient::tool();
        let body = client
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("request completes")
            .text()
            .await
            .expect("body decodes");
        assert_eq!(body, "hello");
        server.abort();
    }

    #[tokio::test]
    async fn observer_records_one_normalized_event_for_an_allowed_request() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        const SECRET: &str = "WCORE-CANARY-allowed-7a2d";
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 2048];
                let _ = sock.read(&mut buf).await;
                let resp = "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n";
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });

        let recorder = Arc::new(BoundedEgressRecorder::new(4));
        let client = EgressClient::tool().with_observer(recorder.clone());
        let response = client
            .post(format!("http://{addr}/private/{SECRET}?token={SECRET}"))
            .bearer_auth(SECRET)
            .body(SECRET)
            .send()
            .await
            .expect("request completes");
        assert_eq!(response.status().as_u16(), 204);

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.dropped_events, 0);
        assert_eq!(snapshot.events.len(), 1);
        let event = &snapshot.events[0];
        assert_eq!(event.attempt_id, 1);
        assert_eq!(event.method, "POST");
        assert_eq!(event.destination.scheme, "http");
        assert_eq!(event.destination.host, "127.0.0.1");
        assert_eq!(event.destination.effective_port, Some(addr.port()));
        assert_eq!(event.destination.path_query_sha256.len(), 64);
        assert_eq!(event.outcome, EgressOutcome::HttpResponse { status: 204 });
        assert!(!format!("{event:?}").contains(SECRET));
        server.abort();
    }

    #[tokio::test]
    async fn denied_request_records_one_event_without_network_io() {
        use tokio::net::TcpListener;

        const SECRET: &str = "WCORE-CANARY-denied-b61e";
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let recorder = Arc::new(BoundedEgressRecorder::new(4));
        let client = EgressClient::tool()
            .with_policy(Arc::new(DenyAll))
            .with_observer(recorder.clone());

        let error = client
            .get(format!("http://{addr}/{SECRET}?secret={SECRET}"))
            .send()
            .await
            .expect_err("DenyAll must stop the request");
        assert!(error.is_denied());
        let accepted = tokio::time::timeout(Duration::from_millis(150), listener.accept()).await;
        assert!(accepted.is_err(), "denial must precede network I/O");

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.dropped_events, 0);
        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(snapshot.events[0].outcome, EgressOutcome::Denied);
        assert!(!format!("{:?}", snapshot.events[0]).contains(SECRET));
        assert!(!format!("{:?}", snapshot.events[0]).contains("denied by test policy"));
    }

    #[tokio::test]
    async fn transport_failure_records_one_stable_error_class() {
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let recorder = Arc::new(BoundedEgressRecorder::new(4));
        let client = EgressClient::builder()
            .connect_timeout(Duration::from_secs(1))
            .timeout(Duration::from_secs(2))
            .observer(recorder.clone())
            .build()
            .expect("client builds");
        let error = client
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect_err("closed listener must refuse the connection");
        assert!(matches!(error, EgressError::Transport(_)));

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(
            snapshot.events[0].outcome,
            EgressOutcome::TransportError {
                class: EgressTransportErrorClass::Connect,
            }
        );
    }

    #[tokio::test]
    async fn client_and_builder_clones_share_observer_and_attempt_ids() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                if let Ok((mut sock, _)) = listener.accept().await {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.flush().await;
                }
            }
        });

        let recorder = Arc::new(BoundedEgressRecorder::new(4));
        let client = EgressClient::tool().with_observer(recorder.clone());
        let cloned_client = client.clone();
        let dispatches = Arc::new(AtomicUsize::new(0));
        let first = client.get(format!("http://{addr}/clone")).before_dispatch({
            let dispatches = Arc::clone(&dispatches);
            move || {
                let dispatches = Arc::clone(&dispatches);
                async move {
                    dispatches.fetch_add(1, Ordering::SeqCst);
                    Ok::<(), &'static str>(())
                }
            }
        });
        let retry = first.try_clone().expect("empty request body is cloneable");
        first.send().await.expect("first request completes");
        retry.send().await.expect("cloned request completes");
        cloned_client
            .get(format!("http://{addr}/client-clone"))
            .send()
            .await
            .expect("request from cloned client completes");

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.events.len(), 3);
        assert_eq!(snapshot.events[0].attempt_id, 1);
        assert_eq!(snapshot.events[1].attempt_id, 2);
        assert_eq!(snapshot.events[2].attempt_id, 3);
        assert_eq!(dispatches.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn cancellation_while_policy_is_pending_records_before_decision() {
        let entered = Arc::new(tokio::sync::Notify::new());
        let recorder = Arc::new(BoundedEgressRecorder::new(4));
        let client = EgressClient::tool()
            .with_policy(Arc::new(PendingPolicy {
                entered: entered.clone(),
            }))
            .with_observer(recorder.clone());

        let request = tokio::spawn(client.get("http://127.0.0.1:1/pending").send());
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .expect("policy check starts");
        request.abort();
        let _ = request.await;

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(
            snapshot.events[0].outcome,
            EgressOutcome::AbandonedBeforeDecision
        );
    }

    #[tokio::test]
    async fn cancellation_after_allow_records_after_allow() {
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(tokio::sync::Notify::new());
        let server_accepted = accepted.clone();
        let server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                server_accepted.notify_one();
                std::future::pending::<()>().await;
            }
        });

        let recorder = Arc::new(BoundedEgressRecorder::new(4));
        let client = EgressClient::tool().with_observer(recorder.clone());
        let request = tokio::spawn(client.get(format!("http://{addr}/pending")).send());
        tokio::time::timeout(Duration::from_secs(2), accepted.notified())
            .await
            .expect("server accepts request");
        request.abort();
        let _ = request.await;

        let snapshot = recorder.snapshot();
        assert_eq!(snapshot.events.len(), 1);
        assert_eq!(
            snapshot.events[0].outcome,
            EgressOutcome::AbandonedAfterAllow
        );
        server.abort();
    }

    #[tokio::test]
    async fn panicking_observer_does_not_change_successful_request() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let resp = "HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.flush().await;
            }
        });

        let response = EgressClient::tool()
            .with_observer(Arc::new(PanickingObserver))
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("observer panic must be fail-open");
        assert_eq!(response.status().as_u16(), 200);
        server.abort();
    }
}
