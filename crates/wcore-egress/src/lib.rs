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
mod policy;
mod request;

pub use client::{
    CONNECT_TIMEOUT, EgressClient, EgressClientBuilder, READ_TIMEOUT, TOOL_REQUEST_TIMEOUT,
};
pub use error::EgressError;
pub use policy::{
    AllowAllPolicy, EgressDecision, EgressPolicy, GlobalDefaultPolicy, SharedPolicy,
    default_policy, global_policy_installed, install_global_policy,
};
pub use request::EgressRequestBuilder;

// Re-export the reqwest surface that migrated call sites still need to name
// directly, so they do not have to keep a separate `reqwest` dependency just
// for these types.
pub use reqwest::{self, Body, Method, Response, Url, header, multipart, redirect};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
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
        let client = EgressClient::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .timeout(Duration::from_millis(200))
            .build()
            .expect("client builds");

        let result = client.get(format!("http://{addr}/")).send().await;
        let err = result.expect_err("a slow-drip server must trip the timeout");
        assert!(
            err.is_timeout(),
            "the failure must be a timeout, got: {err}"
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

        let client = EgressClient::tool().with_policy(Arc::new(DenyAll));
        let result = client.get(format!("http://{addr}/")).send().await;

        let err = result.expect_err("DenyAll must stop the request");
        assert!(err.is_denied(), "must be a policy denial, got: {err}");

        // No connection should have reached the listener — assert accept() does
        // not fire within a generous window.
        let accepted = tokio::time::timeout(Duration::from_millis(150), listener.accept()).await;
        assert!(
            accepted.is_err(),
            "a denied request must never reach the network"
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
}
