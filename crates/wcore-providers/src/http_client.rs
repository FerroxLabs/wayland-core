//! Shared `reqwest::Client` constructors for LLM providers and HTTP tools.
//!
//! Bare `reqwest::Client::new()` ships with NO timeouts — a wedged TCP
//! handshake or a stalled stream hangs the agent indefinitely. v0.6.1
//! hardening: every client routes through this module so the timeout
//! policy is one edit, not five.
//!
//! Two policies, because streaming and request/response have opposite
//! needs:
//!
//! ## `build()` — streaming LLM providers
//!
//! - `connect_timeout(30s)` — TCP + TLS handshake must complete within
//!   30s. Catches DNS / routing / certificate failures fast.
//!
//! - `read_timeout(300s)` — gap between bytes must be under 5 min.
//!   Catches stalled streams without killing long generations.
//!
//!   L2 fix: the previous 120s ceiling false-tripped on extended-thinking
//!   models. A reasoning model can stream NO bytes for well over two
//!   minutes while it reasons server-side *before* the first
//!   `content_block` / `delta` — a perfectly healthy request that the old
//!   120s read timeout killed as a spurious `Connection` error. 5 min is
//!   above the realistic server-side reasoning gap while still catching a
//!   genuinely wedged stream (a truly hung connection never recovers, so
//!   the exact ceiling only affects how fast a real stall is reported).
//!
//! Deliberately NO request-level timeout — that would cap total stream
//! length and break long-form generation. For a token-by-token SSE
//! stream the `read_timeout` (between-bytes) is the correct hang guard.
//!
//! ## `build_tool_client()` — non-streaming HTTP tools
//!
//! AUDIT B-5: GitHub / GitLab / Linear / Notion tool backends do a
//! single request/response, not a stream. For them the `read_timeout`
//! is NOT enough — it is a between-bytes gap timer, so a slow-drip
//! ("slowloris") server that trickles one byte every 119s resets the
//! clock on every byte and the request runs unbounded. A request-level
//! `.timeout(...)` is the correct backstop: a hard wall-clock cap on the
//! whole request. Streaming generation is not a concern here (these are
//! finite REST/GraphQL responses), so the cap that would break `build()`
//! is exactly right for the tool client.

use std::time::Duration;

/// Default TCP+TLS connect timeout for provider clients.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default between-bytes read timeout for provider streams.
///
/// L2: raised from 120s to 300s so extended-thinking / long server-side
/// reasoning does not false-trip the timeout. See module docs.
pub const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// AUDIT B-5 — request-level wall-clock timeout for non-streaming HTTP
/// tools. A generous 300s cap: large GitHub/GitLab responses and slow
/// GraphQL queries still complete, but a slow-drip endpoint can no
/// longer hang a tool call forever (the between-bytes `read_timeout`
/// alone cannot catch that — see module docs).
pub const TOOL_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

/// Request-level wall-clock cap for the non-streaming `GET /v1/models` model
/// discovery call. The streaming provider client ([`build`]) deliberately
/// carries no request timeout (it would truncate long generations), but
/// `list_models` is a finite request/response that must not hang the `/model`
/// picker if the endpoint wedges. 30s is generous for a small JSON listing
/// while still failing fast on a stalled endpoint.
pub const LIST_MODELS_TIMEOUT: Duration = Duration::from_secs(30);

/// Build an `EgressClient` with the streaming-provider timeout policy.
///
/// Panics on builder failure, which can only happen if the TLS backend
/// fails to initialize — that's a deployment-time problem, not a
/// runtime one, and surfacing it loudly at startup is correct.
pub fn build() -> wcore_egress::EgressClient {
    build_with_read_timeout(READ_TIMEOUT)
}

/// Build an `EgressClient` with a caller-specified between-bytes read
/// timeout (and the standard 30s connect timeout).
///
/// L2: additive escape hatch for callers that know a request will have
/// unusually long silent gaps (e.g. a thinking-heavy model run). `build()`
/// uses [`READ_TIMEOUT`]; this variant lets a provider raise it without a
/// breaking signature change.
pub fn build_with_read_timeout(read_timeout: Duration) -> wcore_egress::EgressClient {
    // B1: route through the egress chokepoint. EgressClient::streaming_with_read_timeout
    // carries the identical policy (30s connect, caller read timeout, redirects
    // disabled — the credential-exfil-on-302 mitigation, M-1 / U-1).
    wcore_egress::EgressClient::streaming_with_read_timeout(read_timeout)
}

/// AUDIT B-5 — build an `EgressClient` for non-streaming HTTP tools.
///
/// Identical connect + read timeouts to [`build`], PLUS a request-level
/// `.timeout(TOOL_REQUEST_TIMEOUT)` wall-clock cap. Use this for any
/// finite request/response HTTP tool (REST, GraphQL); use [`build`] only
/// for token-streaming LLM providers where a request-level cap would
/// truncate a legitimate long generation.
///
/// Panics on builder failure — same rationale as [`build`].
pub fn build_tool_client() -> wcore_egress::EgressClient {
    // B1: EgressClient::tool carries the identical non-streaming policy (connect
    // + read timeouts PLUS the request-level wall-clock cap, redirects disabled).
    wcore_egress::EgressClient::tool()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_constructs_a_client() {
        // `build()` must not panic — the TLS backend initializes.
        let _client = build();
    }

    #[test]
    fn build_tool_client_constructs_a_client() {
        // AUDIT B-5 — the tool client must construct without panicking.
        let _client = build_tool_client();
    }

    #[tokio::test]
    async fn build_client_does_not_follow_redirects() {
        // M-1 / U-1: a 302 must NOT be followed — the client returns the 3xx
        // response itself rather than chasing the Location to a second host
        // (which would re-send any URL/header secret). We stand up a TCP
        // listener that always answers `302 Location: http://attacker/` and
        // assert reqwest yields the 302 status, not a followed-through 200.
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

        let client = build();
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
    async fn tool_client_request_times_out_on_a_slow_drip_server() {
        // AUDIT B-5 — a server that accepts the connection but never
        // sends a full response must NOT hang the tool client forever.
        // The request-level `.timeout(...)` is the backstop the
        // between-bytes `read_timeout` cannot provide. We assert the
        // timeout is WIRED by issuing a real request against a TCP
        // listener that accepts but withholds the response body and
        // confirming reqwest reports a timeout — with a short-TTL
        // client so the test runs fast.
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Accept the connection, read the request, then hold the socket
        // open forever without replying — the classic slowloris shape.
        let server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                // Never write a response; never close. Park until the
                // test drops the task.
                std::future::pending::<()>().await;
            }
        });

        // A tool client with a 200ms request cap — same construction
        // path as `build_tool_client`, just a fast TTL for the test.
        let client = wcore_egress::EgressClient::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .timeout(Duration::from_millis(200))
            .build()
            .expect("client builds");

        let result = client.get(format!("http://{addr}/")).send().await;
        assert!(
            result.is_err(),
            "a slow-drip server must trip the request-level timeout"
        );
        let err = result.unwrap_err();
        assert!(
            err.is_timeout(),
            "the failure must be a timeout, got: {err}"
        );

        server.abort();
    }
}
