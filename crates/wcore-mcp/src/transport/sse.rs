use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue};
use tokio::sync::{Mutex, oneshot};

use super::{McpError, McpTransport};
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Per-request timeout for SSE JSON-RPC calls (audit C6). Matches the
/// stdio transport's bound so a tool call has consistent semantics
/// regardless of transport.
const SSE_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Cap on the SSE reassembly buffer (audit mcp-40). A hostile or buggy
/// MCP SSE server can stream bytes without a `\n\n` event delimiter; the
/// `buffer`/`buf` accumulators would otherwise grow until the host OOMs.
/// 4 MiB is far larger than any legitimate single SSE event yet small
/// enough to bound memory. On overflow the connection is treated as dead.
const MAX_SSE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// SSE transport: connects to an SSE endpoint for server→client events,
/// sends requests via POST to the endpoint URL received from the SSE stream
pub struct SseTransport {
    client: wcore_egress::EgressClient,
    /// The POST endpoint URL (received from the SSE stream's "endpoint" event)
    post_url: String,
    headers: HeaderMap,
    /// Pending request-response channels, keyed by JSON-RPC id
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    next_id: AtomicU64,
    /// Handle to the background SSE listener task
    _listener: tokio::task::JoinHandle<()>,
    /// Per-request timeout for the response `oneshot` (audit C6).
    request_timeout: std::time::Duration,
}

impl SseTransport {
    /// Connect to an SSE MCP server.
    pub async fn connect(
        url: &str,
        headers: &HashMap<String, String>,
        allow_local: bool,
    ) -> Result<Self, McpError> {
        Self::connect_with_timeout(url, headers, SSE_REQUEST_TIMEOUT, allow_local).await
    }

    /// [`connect`](Self::connect) with an explicit per-request timeout.
    /// Test seam (audit C6): a short bound lets a test verify the response
    /// wait is bounded without waiting the production 120s budget.
    pub async fn connect_with_timeout(
        url: &str,
        headers: &HashMap<String, String>,
        request_timeout: std::time::Duration,
        allow_local: bool,
    ) -> Result<Self, McpError> {
        // M-13 (SSRF) — validate the configured URL before we attach any
        // secret-bearing header and open a connection to it. `is_safe_url`
        // fails closed on private/internal/metadata targets and on DNS
        // failure, so a server configured at `http://169.254.169.254/...`
        // (or a hostname that resolves there) is rejected at connect time
        // rather than becoming an SSRF primitive.
        //
        // `allow_local` (per-server config) relaxes the LOOPBACK block only,
        // for trusted user-configured local MCP servers. Every other private/
        // internal/metadata range stays blocked.
        let url_ok = if allow_local {
            wcore_tools::url_safety::is_safe_url_allow_loopback(url)
        } else {
            wcore_tools::url_safety::is_safe_url(url)
        };
        if !url_ok {
            return Err(McpError::Transport(format!(
                "SSE MCP url rejected — resolves to a private or internal \
                 network address (SSRF guard): {url}"
            )));
        }

        let mut header_map = HeaderMap::new();
        for (k, v) in headers {
            let name = reqwest::header::HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| McpError::Transport(format!("Invalid header name '{}': {}", k, e)))?;
            let value = HeaderValue::from_str(v)
                .map_err(|e| McpError::Transport(format!("Invalid header value '{}': {}", v, e)))?;
            header_map.insert(name, value);
        }

        // Wave RA RELIABILITY BLOCKER #2 — `pool_idle_timeout` so a
        // request that the caller cancels mid-flight doesn't loiter in
        // the reqwest connection pool. 5s is short enough to avoid
        // connection-pool exhaustion under cancel/retry storms and long
        // enough to retain healthy keep-alive for back-to-back tool
        // calls in normal use. Builder failure is a configuration bug;
        // fall back to the default client so the transport stays usable.
        //
        // Audit C5 — `.connect_timeout()` bounds the TCP/TLS handshake
        // against an unreachable host. We deliberately do NOT set a
        // request-wide `.timeout()` here: this client also drives the
        // long-lived SSE GET event stream, and a body timeout would kill
        // that stream after the bound elapses. The per-request bound for
        // SSE lives on the response `oneshot` in `request()` (audit C6).
        //
        // M-13 (SSRF) — install `ssrf_safe_redirect_policy` so every
        // redirect hop is re-validated. Without it reqwest follows up to
        // 10 hops blindly and a `302 → http://169.254.169.254/...` would
        // be followed with the configured auth headers attached.
        //
        // H-1-broad — the redirect policy re-checks the hop URL but reqwest
        // re-resolves the host at connect time; `SsrfSafeResolver` makes
        // reqwest dial only validated public IPs (no check→connect rebind
        // window) for the initial GET and every redirect hop.
        // allow_local swaps in the loopback-permitting redirect policy +
        // resolver so reqwest can dial 127.0.0.1/::1 for a trusted local SSE
        // server; both variants keep every non-loopback SSRF protection.
        let builder = wcore_egress::EgressClient::builder()
            .pool_idle_timeout(std::time::Duration::from_secs(5))
            .connect_timeout(std::time::Duration::from_secs(15));
        let builder = if allow_local {
            builder
                .redirect(wcore_tools::url_safety::ssrf_safe_redirect_policy_allow_loopback())
                .dns_resolver(std::sync::Arc::new(
                    wcore_tools::url_safety::LoopbackOkResolver,
                ))
        } else {
            builder
                .redirect(wcore_tools::url_safety::ssrf_safe_redirect_policy())
                .dns_resolver(std::sync::Arc::new(
                    wcore_tools::url_safety::SsrfSafeResolver,
                ))
        };
        let client = builder
            .build()
            .unwrap_or_else(|_| wcore_egress::EgressClient::new());

        // GET the SSE endpoint to establish the event stream
        let response = client
            .get(url)
            .headers(header_map.clone())
            .header("Accept", "text/event-stream")
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("SSE connection failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(McpError::Transport(format!(
                "SSE connection returned status: {}",
                response.status()
            )));
        }

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        // Parse the SSE stream to find the endpoint URL
        // The server sends an "endpoint" event with the POST URL
        let mut bytes_stream = response.bytes_stream();
        let mut buffer = String::new();
        // Decode incrementally so a codepoint split across chunks is not
        // corrupted into U+FFFD; the decoder is moved into the listener task
        // below so a split at the handshake/listener boundary is handled too.
        let mut utf8 = wcore_types::utf8_stream::Utf8StreamDecoder::new();
        let mut post_url: Option<String> = None;

        use futures::StreamExt;
        // Read initial events to get the endpoint URL
        while let Some(chunk) = bytes_stream.next().await {
            let chunk = chunk.map_err(|e| McpError::Transport(format!("SSE read error: {}", e)))?;
            buffer.push_str(&utf8.push(&chunk));

            // mcp-40 — bound the handshake reassembly buffer. A server that
            // streams bytes without an event delimiter must not OOM us.
            if buffer.len() > MAX_SSE_BUFFER_BYTES {
                return Err(McpError::Transport(format!(
                    "SSE handshake buffer exceeded {MAX_SSE_BUFFER_BYTES} bytes \
                     without an endpoint event — server is misbehaving"
                )));
            }

            // Parse SSE events from buffer
            while let Some(event_end) = buffer.find("\n\n") {
                let event_block = buffer[..event_end].to_string();
                buffer = buffer[event_end + 2..].to_string();

                let (event_type, event_data) = parse_sse_event(&event_block);

                if event_type == "endpoint" {
                    // H-5 — the server-chosen endpoint controls where we
                    // POST the configured (secret-bearing) auth headers. A
                    // malicious server can emit an absolute URL pointing at
                    // its own collector; resolving it verbatim would leak
                    // the bearer token + request bodies cross-origin.
                    //
                    // Constrain it: the endpoint is resolved via WHATWG
                    // `Url::join` against the configured url, then accepted
                    // ONLY when its origin (scheme+host+port) matches the
                    // configured url's origin — for absolute and relative
                    // payloads alike. The resolved URL is then re-checked
                    // through the SSRF guard before we ever attach headers.
                    let endpoint = resolve_endpoint(url, &event_data)?;
                    post_url = Some(endpoint);
                    break;
                }
            }

            if post_url.is_some() {
                break;
            }
        }

        let post_url = post_url
            .ok_or_else(|| McpError::Transport("No endpoint event received from SSE".into()))?;

        // Spawn background task to listen for SSE responses
        let pending_clone = pending.clone();
        let listener = tokio::spawn(async move {
            let mut buf = buffer; // carry over remaining buffer
            let mut utf8 = utf8; // carry the UTF-8 decoder tail across loops
            while let Some(chunk) = bytes_stream.next().await {
                let Ok(chunk) = chunk else { break };
                buf.push_str(&utf8.push(&chunk));

                // mcp-40 — bound the listener reassembly buffer. On overflow
                // we drop the stream (break): pending requests then time out
                // via the per-request oneshot bound rather than OOMing.
                if buf.len() > MAX_SSE_BUFFER_BYTES {
                    break;
                }

                while let Some(event_end) = buf.find("\n\n") {
                    let event_block = buf[..event_end].to_string();
                    buf = buf[event_end + 2..].to_string();

                    let (event_type, event_data) = parse_sse_event(&event_block);

                    if (event_type == "message" || event_type.is_empty())
                        && let Ok(response) = serde_json::from_str::<JsonRpcResponse>(&event_data)
                        && let Some(id) = response.id
                    {
                        let mut map: tokio::sync::MutexGuard<
                            '_,
                            HashMap<u64, oneshot::Sender<JsonRpcResponse>>,
                        > = pending_clone.lock().await;
                        if let Some(sender) = map.remove(&id) {
                            let _ = sender.send(response);
                        }
                    }
                }
            }

            // Rank 26 — the listener exited (stream EOF, chunk error, or the
            // mcp-40 buffer-overflow break). Any request still parked in
            // `pending` will never receive its `message` event now. Drain the
            // map and drop every sender so each waiting `request()` wakes
            // immediately via its `Ok(Err(_))` arm ("Response channel closed
            // unexpectedly") instead of waiting out the full `request_timeout`.
            pending_clone.lock().await.clear();
        });

        Ok(Self {
            client,
            post_url,
            headers: header_map,
            pending,
            next_id: AtomicU64::new(1),
            _listener: listener,
            request_timeout,
        })
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }
}

#[async_trait]
impl McpTransport for SseTransport {
    async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let req_id = req
            .id
            .ok_or_else(|| McpError::Transport("Request must have an id".into()))?;

        // Set up response channel before sending
        let (tx, rx) = oneshot::channel::<JsonRpcResponse>();
        {
            let mut map: tokio::sync::MutexGuard<
                '_,
                HashMap<u64, oneshot::Sender<JsonRpcResponse>>,
            > = self.pending.lock().await;
            map.insert(req_id, tx);
        }

        // POST the request
        let body = serde_json::to_string(req)
            .map_err(|e| McpError::Transport(format!("JSON serialize error: {}", e)))?;

        // Rank 25 — bound the POST send itself. `connect_timeout` (15s) only
        // covers the TCP/TLS handshake and the response `oneshot` wait below
        // only starts once the POST returns; neither bounds a server that
        // accepts the connection but never reads the request body, which would
        // otherwise park `send().await` forever. `.timeout()` bounds the whole
        // request (send + headers) using the same per-request budget.
        let response = self
            .client
            .post(&self.post_url)
            .timeout(self.request_timeout)
            .headers(self.headers.clone())
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("POST request failed: {}", e)))?;

        if !response.status().is_success() {
            // Clean up pending
            self.pending.lock().await.remove(&req_id);
            return Err(McpError::Transport(format!(
                "POST returned status: {}",
                response.status()
            )));
        }

        // Wait for response from SSE stream.
        //
        // Audit C6 — bound the wait. The listener task can be alive yet the
        // server may never emit the `message` event for this id (server bug,
        // dropped event, id mismatch); the connection stays open, the
        // listener never breaks, this `tx` is never used or dropped, and an
        // unbounded `rx.await` parks forever. On timeout we MUST remove the
        // stale `pending` entry, otherwise every lost response leaks a dead
        // oneshot in the map.
        let rpc_response = match tokio::time::timeout(self.request_timeout, rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => {
                // Listener dropped the sender (stream ended / errored).
                return Err(McpError::Transport(
                    "Response channel closed unexpectedly".into(),
                ));
            }
            Err(_) => {
                self.pending.lock().await.remove(&req_id);
                return Err(McpError::Transport(format!(
                    "MCP SSE request timed out after {:?}",
                    self.request_timeout
                )));
            }
        };

        if let Some(err) = &rpc_response.error {
            return Err(McpError::JsonRpc {
                code: err.code,
                message: err.message.clone(),
            });
        }

        Ok(rpc_response)
    }

    async fn notify(&self, req: &JsonRpcRequest) -> Result<(), McpError> {
        let body = serde_json::to_string(req)
            .map_err(|e| McpError::Transport(format!("JSON serialize error: {}", e)))?;

        self.client
            .post(&self.post_url)
            .headers(self.headers.clone())
            .header("Content-Type", "application/json")
            .body(body)
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("Notification POST failed: {}", e)))?;

        Ok(())
    }

    async fn close(&self) -> Result<(), McpError> {
        self._listener.abort();
        Ok(())
    }
}

/// Parse a single SSE event block into (event_type, data)
fn parse_sse_event(block: &str) -> (String, String) {
    let mut event_type = String::new();
    let mut data_lines = Vec::new();

    for line in block.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            event_type = value.trim().to_string();
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim().to_string());
        }
    }

    (event_type, data_lines.join("\n"))
}

/// Extract the WHATWG tuple origin (scheme, host, port) from a URL using the
/// SAME parser reqwest dials with (`reqwest::Url` is `url::Url`).
///
/// Returns `None` (fail closed) for anything that does not parse as a
/// hierarchical URL with a real host — i.e. opaque/unparseable origins are
/// rejected. Using the WHATWG parser here is the whole point of H-5: a
/// hand-rolled authority split treats `\` literally, but reqwest (per WHATWG)
/// maps `\` to `/`, so `https://attacker.example\@vendor.example/...` is
/// dialed against `attacker.example`. Comparing `Origin`s parsed this way
/// closes that divergence.
fn whatwg_tuple_origin(url: &str) -> Option<reqwest::Url> {
    let parsed = reqwest::Url::parse(url).ok()?;
    // Reject opaque/non-tuple origins (no host) — fail closed. A tuple origin
    // is the WHATWG (scheme, host, port) triple; an opaque origin is never
    // equal to any other origin, so this also guarantees the comparison is
    // meaningful.
    parsed.origin().is_tuple().then_some(parsed)
}

/// Resolve the server-sent SSE `endpoint` event into a POST URL that is
/// safe to attach the configured (secret-bearing) auth headers to.
///
/// H-5 — The endpoint is resolved with WHATWG [`Url::join`] against the
/// configured url and accepted ONLY when its origin (scheme+host+port)
/// matches the configured url's origin — for absolute AND relative payloads
/// alike. `join` resolves a real absolute URL to itself and treats an
/// authority-injecting payload (`@host`, `\host`, `//host`) as path data
/// relative to the base, so the same-origin gate always compares the host
/// reqwest will actually dial. (The earlier revision only origin-checked
/// inputs literally prefixed `http(s)://` and string-concatenated everything
/// else, so a relative `@attacker/x` smuggled the bearer header off-origin:
/// `https://vendor` + `@attacker/x` parses to host `attacker`.) The resolved
/// URL is then re-checked through the SSRF guard (M-13) before it is
/// returned.
fn resolve_endpoint(configured_url: &str, event_data: &str) -> Result<String, McpError> {
    // Parse the configured base with the SAME WHATWG parser reqwest dials
    // with (`reqwest::Url` is `url::Url`); fail closed on a hostless/opaque
    // configured url.
    let base = whatwg_tuple_origin(configured_url)
        .ok_or_else(|| McpError::Transport("SSE: configured url has no parseable origin".into()))?;

    // WHATWG join: absolute endpoints resolve to themselves; relative ones
    // resolve against the base path. An authority-injecting payload becomes
    // path data, not a new host.
    let resolved = base
        .join(event_data)
        .map_err(|e| McpError::Transport(format!("SSE: endpoint event is not a valid URL: {e}")))?;

    // Same-origin gate on the FULLY RESOLVED url, applied uniformly. A
    // cross-origin POST target would carry the configured secret-bearing
    // auth headers to a server-chosen collector.
    if resolved.origin() != base.origin() {
        return Err(McpError::Transport(format!(
            "SSE endpoint rejected — cross-origin POST target ({:?}) differs from \
             the configured origin ({:?}); refusing to forward auth headers off-origin",
            resolved.origin(),
            base.origin()
        )));
    }

    let endpoint = resolved.to_string();

    // Defense-in-depth: re-run the SSRF guard on the resolved POST URL.
    if !wcore_tools::url_safety::is_safe_url(&endpoint) {
        return Err(McpError::Transport(format!(
            "SSE endpoint rejected — resolves to a private or internal \
             network address (SSRF guard): {endpoint}"
        )));
    }

    Ok(endpoint)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Audit C6 — `SseTransport::request` must bound its wait on the response
    /// `oneshot`. The failure mode without the fix: the listener is alive, the
    /// connection stays open, but the server never emits the `message` event
    /// for the request id, so `rx.await` parks forever.
    ///
    /// This constructs the transport DIRECTLY rather than via
    /// `connect_with_timeout`: the M-13 SSRF guard on the connect path
    /// (correctly) rejects the loopback test server, and the production
    /// `SsrfSafeResolver` would refuse to dial loopback even if it didn't. By
    /// building the struct with a plain client and a no-op listener task that
    /// never fills `pending`, `request()` exercises ONLY the C6 timeout path —
    /// with zero change to the SSRF-hardened production connect path. A real
    /// TCP server accepts the POST and 200s it but never answers on the stream.
    #[tokio::test]
    async fn c6_request_times_out_and_cleans_up_pending_when_response_never_arrives() {
        use std::collections::HashMap;
        use std::sync::Arc;
        use std::sync::atomic::AtomicU64;
        use std::time::{Duration, Instant};

        use reqwest::header::HeaderMap;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::Mutex;

        use crate::protocol::JsonRpcRequest;
        use crate::transport::McpTransport;

        // Loopback server: accept the POST, 200 it, never send an SSE message.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let _ = socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    let _ = socket.flush().await;
                });
            }
        });

        let transport = SseTransport {
            // Plain client (no `SsrfSafeResolver`) so it can dial loopback.
            client: wcore_egress::EgressClient::new(),
            post_url: format!("http://{addr}/post"),
            headers: HeaderMap::new(),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            // No-op listener: never delivers a `message`, so the response
            // oneshot for this request can only be resolved by the timeout.
            _listener: tokio::spawn(async {}),
            request_timeout: Duration::from_millis(500),
        };

        let req = JsonRpcRequest::new(1, "tools/call", None);
        let start = Instant::now();
        let result = transport.request(&req).await;
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "a never-answered SSE request must error, not hang"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("timed out"),
            "expected a timeout error, got: {msg}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "SSE request timeout must fire promptly, took {elapsed:?}"
        );
        // The stale pending entry MUST be removed on timeout — otherwise every
        // lost response leaks a dead oneshot in the map (audit C6).
        assert!(
            transport.pending.lock().await.is_empty(),
            "timed-out request must remove its pending oneshot"
        );
    }

    /// Rank 25 — `SseTransport::request` must bound the POST `send()` itself,
    /// not just the response `oneshot` wait that follows it. The failure mode
    /// without the fix: a server that accepts the TCP connection but never
    /// reads the request body nor responds parks `send().await` forever —
    /// `connect_timeout` already elapsed (handshake completed) and the oneshot
    /// timeout never starts because the POST never returns.
    ///
    /// The loopback server here accepts the connection and then goes silent
    /// (never reading or replying), so only the `.timeout(request_timeout)` on
    /// the POST builder can unblock the call. As in the C6 test, the transport
    /// is constructed directly with a plain client so it can dial loopback
    /// without tripping the production SSRF resolver.
    #[tokio::test]
    async fn rank25_post_send_times_out_when_server_never_reads_body() {
        use std::collections::HashMap;
        use std::sync::Arc;
        use std::sync::atomic::AtomicU64;
        use std::time::{Duration, Instant};

        use reqwest::header::HeaderMap;
        use tokio::net::TcpListener;
        use tokio::sync::Mutex;

        use crate::protocol::JsonRpcRequest;
        use crate::transport::McpTransport;

        // Loopback server: accept the connection, then never read the body or
        // write a response. The accepted socket is held (not dropped) so the
        // client side stays connected with the POST in flight.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let mut held = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                // Park the socket open and silent.
                held.push(socket);
            }
        });

        let transport = SseTransport {
            // Plain client (no `SsrfSafeResolver`) so it can dial loopback.
            client: wcore_egress::EgressClient::new(),
            post_url: format!("http://{addr}/post"),
            headers: HeaderMap::new(),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            _listener: tokio::spawn(async {}),
            request_timeout: Duration::from_millis(500),
        };

        let req = JsonRpcRequest::new(1, "tools/call", None);
        let start = Instant::now();
        let result = transport.request(&req).await;
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "a POST to a server that never reads the body must error, not hang"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("POST request failed"),
            "expected the POST send to fail (timeout), got: {msg}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "the POST send timeout must fire promptly, took {elapsed:?}"
        );
    }

    /// Rank 26 — when the listener task exits (stream EOF / chunk error /
    /// buffer-overflow break) it must drain `pending` and drop every parked
    /// sender so an in-flight `request()` fails FAST with the channel-closed
    /// error, instead of waiting out the full `request_timeout`.
    ///
    /// We construct the transport directly (as the C6/Rank-25 tests do) with a
    /// long request timeout and a listener that shares the real `pending` map
    /// and exits immediately after draining it. A POST that 200s lets the
    /// request reach the `rx.await`; the drained sender then resolves it
    /// promptly via the `Ok(Err(_))` arm — well under the request timeout.
    #[tokio::test]
    async fn rank26_listener_exit_drains_pending_and_fails_fast() {
        use std::collections::HashMap;
        use std::sync::Arc;
        use std::sync::atomic::AtomicU64;
        use std::time::{Duration, Instant};

        use reqwest::header::HeaderMap;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        use tokio::sync::Mutex;

        use crate::protocol::JsonRpcRequest;
        use crate::transport::McpTransport;

        // Loopback server: accept the POST, 200 it, never send an SSE message.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = socket.read(&mut buf).await;
                    let _ = socket
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    let _ = socket.flush().await;
                });
            }
        });

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_clone = pending.clone();

        // Listener stand-in: wait until the request has registered its pending
        // oneshot, then drain the map (mirrors the production drain-on-exit) and
        // exit. Dropping the senders is what wakes the parked request.
        let listener_task = tokio::spawn(async move {
            loop {
                {
                    let mut map = pending_clone.lock().await;
                    if !map.is_empty() {
                        map.clear();
                        break;
                    }
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        let transport = SseTransport {
            client: wcore_egress::EgressClient::new(),
            post_url: format!("http://{addr}/post"),
            headers: HeaderMap::new(),
            pending,
            next_id: AtomicU64::new(1),
            _listener: listener_task,
            // A long request timeout: if the drain did NOT fail the request
            // fast, this test would hang for ~30s and the elapsed assert would
            // catch the regression.
            request_timeout: Duration::from_secs(30),
        };

        let req = JsonRpcRequest::new(1, "tools/call", None);
        let start = Instant::now();
        let result = transport.request(&req).await;
        let elapsed = start.elapsed();

        assert!(
            result.is_err(),
            "a request parked when the listener exits must error, not hang"
        );
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("Response channel closed"),
            "expected the channel-closed (fast-fail) error, got: {msg}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "listener-exit drain must fail the request fast, took {elapsed:?}"
        );
    }

    #[test]
    fn whatwg_tuple_origin_basic() {
        // Same origin regardless of path/userinfo, parsed by the WHATWG parser.
        assert_eq!(
            whatwg_tuple_origin("https://vendor.example/mcp/sse").map(|u| u.origin()),
            whatwg_tuple_origin("https://user:pass@vendor.example/messages").map(|u| u.origin())
        );
        // A distinct port is a distinct origin.
        assert_ne!(
            whatwg_tuple_origin("https://vendor.example/mcp").map(|u| u.origin()),
            whatwg_tuple_origin("https://vendor.example:8443/mcp").map(|u| u.origin())
        );
        // Non-hierarchical / hostless inputs fail closed.
        assert!(whatwg_tuple_origin("not-a-url").is_none());
        assert!(whatwg_tuple_origin("mailto:a@b.example").is_none());
    }

    /// H-5 — the exact bypass from the audit. A malicious SSE server emits an
    /// absolute endpoint with a single backslash before `@`. A hand-rolled
    /// authority split would `rsplit('@')` to `vendor.example` and pass the
    /// gate, but reqwest (WHATWG) maps `\` to `/`, so the real dialed host is
    /// `attacker.example`. The same-origin gate MUST reject it cross-origin.
    #[test]
    fn resolve_endpoint_backslash_at_smuggle_rejected() {
        let err = resolve_endpoint(
            "https://vendor.example/mcp",
            r"https://attacker.example\@vendor.example/collect",
        )
        .expect_err("backslash-@ host smuggle must be rejected cross-origin");
        let msg = err.to_string();
        assert!(
            msg.contains("cross-origin"),
            "expected a cross-origin rejection, got: {msg}"
        );
    }

    /// H-5 — a same-origin relative endpoint joins onto the base.
    #[test]
    fn resolve_endpoint_relative_same_origin_ok() {
        let resolved = resolve_endpoint("https://example.com/mcp/sse", "/messages")
            .expect("relative endpoint should resolve");
        assert_eq!(resolved, "https://example.com/messages");
    }

    /// H-5 — a same-origin absolute endpoint is honored.
    #[test]
    fn resolve_endpoint_absolute_same_origin_ok() {
        let resolved = resolve_endpoint(
            "https://example.com/mcp/sse",
            "https://example.com/messages/abc",
        )
        .expect("same-origin absolute endpoint should resolve");
        assert_eq!(resolved, "https://example.com/messages/abc");
    }

    /// H-5 — the core hole: a cross-origin absolute endpoint (the
    /// attacker collector) must be rejected so the bearer header is never
    /// POSTed off-origin.
    #[test]
    fn resolve_endpoint_cross_origin_rejected() {
        let err = resolve_endpoint(
            "https://vendor.example/mcp",
            "https://attacker.example/collect",
        )
        .expect_err("cross-origin endpoint must be rejected");
        let msg = err.to_string();
        assert!(
            msg.contains("cross-origin"),
            "expected a cross-origin rejection, got: {msg}"
        );
    }

    /// H-5 (residual bypass) — the `@`-userinfo RELATIVE-endpoint smuggle.
    /// A payload not literally prefixed with a scheme used to skip the
    /// same-origin gate entirely and string-concat onto the base, turning
    /// the configured host into userinfo:
    /// `https://vendor.example` + `@attacker/x` =
    /// `https://vendor.example@attacker/x` — which `url::Url`/reqwest dials
    /// against host `attacker`, carrying the bearer header off-origin.
    /// Resolving via WHATWG `join` keeps it on-origin (path data).
    #[test]
    fn resolve_endpoint_userinfo_relative_smuggle_stays_on_origin() {
        // Configured host is a literal public IP so the SSRF guard is not
        // what rejects — this isolates the join-vs-concat behavior. Under
        // the old concat resolver the host became `attacker.evil.test`; the
        // join resolver keeps it on `8.8.8.8`.
        let resolved = resolve_endpoint("http://8.8.8.8/mcp", "@attacker.evil.test/x")
            .expect("userinfo-relative payload must resolve on-origin, not smuggle a host");
        let host = reqwest::Url::parse(&resolved)
            .unwrap()
            .host_str()
            .unwrap()
            .to_string();
        assert_eq!(
            host, "8.8.8.8",
            "endpoint must stay on the configured origin; got {resolved}"
        );
    }

    /// H-5 + M-13 — a same-origin absolute endpoint whose host is the
    /// cloud metadata IP must still be rejected by the SSRF guard.
    #[test]
    fn resolve_endpoint_metadata_ip_rejected() {
        // Configured url and endpoint share origin (so the same-origin
        // gate passes) but the host is the metadata service — the SSRF
        // guard must reject it.
        let err = resolve_endpoint(
            "http://169.254.169.254/mcp",
            "http://169.254.169.254/latest/meta-data/",
        )
        .expect_err("metadata-IP endpoint must be rejected by the SSRF guard");
        let msg = err.to_string();
        assert!(
            msg.contains("private or internal") || msg.contains("SSRF"),
            "expected an SSRF rejection, got: {msg}"
        );
    }
}
