use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderValue};
use tokio::sync::Mutex;

use super::{McpError, McpTransport};
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Cap on the SSE reassembly buffer for streamable-HTTP responses
/// (audit mcp-40). A server can answer with `text/event-stream` and stream
/// bytes without a `\n\n` delimiter; the accumulator would otherwise grow
/// until the host OOMs. 4 MiB bounds memory while exceeding any legitimate
/// single JSON-RPC response frame.
const MAX_SSE_BUFFER_BYTES: usize = 4 * 1024 * 1024;

/// Streamable HTTP transport: uses HTTP POST for both requests and responses
/// Supports optional SSE streaming for server responses
#[derive(Debug)]
pub struct StreamableHttpTransport {
    client: wcore_egress::EgressClient,
    url: String,
    headers: HeaderMap,
    session_id: Mutex<Option<String>>,
    next_id: AtomicU64,
}

impl StreamableHttpTransport {
    /// Create a new Streamable HTTP transport
    pub async fn connect(url: &str, headers: &HashMap<String, String>) -> Result<Self, McpError> {
        // M-13 (SSRF) — validate the configured URL before attaching any
        // secret-bearing header. `is_safe_url` fails closed on
        // private/internal/metadata targets and on DNS failure.
        if !wcore_tools::url_safety::is_safe_url(url) {
            return Err(McpError::Transport(format!(
                "Streamable-HTTP MCP url rejected — resolves to a private or \
                 internal network address (SSRF guard): {url}"
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

        // Wave RA RELIABILITY BLOCKER #2 — pool_idle_timeout (see sse.rs
        // for rationale). A cancelled-mid-flight request must close its
        // underlying connection promptly, not loiter in the pool.
        //
        // Audit C5 — `pool_idle_timeout` only governs *idle* pooled
        // connections; it does nothing for an *active in-flight* request.
        // A streamable-HTTP MCP endpoint that connects then stalls (or
        // streams an SSE body forever without a valid JSON-RPC frame)
        // hangs `request()` permanently. `.timeout()` bounds the whole
        // request including the response body (so the SSE-streaming path
        // in `parse_sse_response` is covered too); `.connect_timeout()`
        // bounds the TCP/TLS handshake against an unreachable host.
        //
        // M-13 (SSRF) — install `ssrf_safe_redirect_policy` so every
        // redirect hop is re-validated. The default 10-hop follow would
        // otherwise chase a `302 → http://169.254.169.254/...` with the
        // configured auth headers attached.
        //
        // H-1-broad — the redirect policy re-checks the hop URL but reqwest
        // re-resolves the host at connect time; `SsrfSafeResolver` makes
        // reqwest dial only validated public IPs (no check→connect rebind
        // window) for the initial request and every redirect hop.
        let client = wcore_egress::EgressClient::builder()
            .pool_idle_timeout(std::time::Duration::from_secs(5))
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(120))
            .redirect(wcore_tools::url_safety::ssrf_safe_redirect_policy())
            .dns_resolver(std::sync::Arc::new(
                wcore_tools::url_safety::SsrfSafeResolver,
            ))
            .build()
            .unwrap_or_else(|_| wcore_egress::EgressClient::new());

        Ok(Self {
            client,
            url: url.to_string(),
            headers: header_map,
            session_id: Mutex::new(None),
            next_id: AtomicU64::new(1),
        })
    }

    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Build request with session ID header if available
    async fn build_request(&self, body: &str) -> wcore_egress::EgressRequestBuilder {
        let mut req = self
            .client
            .post(&self.url)
            .headers(self.headers.clone())
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream");

        if let Some(sid) = self.session_id.lock().await.as_ref() {
            req = req.header("Mcp-Session-Id", sid.as_str());
        }

        req.body(body.to_string())
    }

    /// Parse response based on content type
    async fn parse_response(
        &self,
        response: reqwest::Response,
    ) -> Result<JsonRpcResponse, McpError> {
        // Capture session ID from response headers
        if let Some(sid) = response.headers().get("mcp-session-id")
            && let Ok(sid_str) = sid.to_str()
        {
            *self.session_id.lock().await = Some(sid_str.to_string());
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        if content_type.contains("text/event-stream") {
            // SSE response: parse events to find the JSON-RPC response
            self.parse_sse_response(response).await
        } else {
            // Direct JSON response
            let text = response
                .text()
                .await
                .map_err(|e| McpError::Transport(format!("Read response body failed: {}", e)))?;
            serde_json::from_str(&text).map_err(|e| {
                McpError::Transport(format!("Parse JSON response failed: {} — raw: {}", e, text))
            })
        }
    }

    /// Parse an SSE stream response to extract JSON-RPC response
    async fn parse_sse_response(
        &self,
        response: reqwest::Response,
    ) -> Result<JsonRpcResponse, McpError> {
        use futures::StreamExt;

        let mut stream = response.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| McpError::Transport(format!("SSE read error: {}", e)))?;
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            // mcp-40 — bound the reassembly buffer so a server streaming an
            // endless newline-free body cannot OOM the host.
            if buffer.len() > MAX_SSE_BUFFER_BYTES {
                return Err(McpError::Transport(format!(
                    "SSE response buffer exceeded {MAX_SSE_BUFFER_BYTES} bytes \
                     without a complete JSON-RPC frame — server is misbehaving"
                )));
            }

            // Parse SSE events
            while let Some(event_end) = buffer.find("\n\n") {
                let event_block = buffer[..event_end].to_string();
                buffer = buffer[event_end + 2..].to_string();

                // Extract data lines
                let mut data_lines = Vec::new();
                for line in event_block.lines() {
                    if let Some(value) = line.strip_prefix("data:") {
                        data_lines.push(value.trim().to_string());
                    }
                }

                let data = data_lines.join("\n");
                if !data.is_empty()
                    && let Ok(rpc_response) = serde_json::from_str::<JsonRpcResponse>(&data)
                {
                    return Ok(rpc_response);
                }
            }
        }

        Err(McpError::Transport(
            "SSE stream ended without JSON-RPC response".into(),
        ))
    }
}

#[async_trait]
impl McpTransport for StreamableHttpTransport {
    async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let body = serde_json::to_string(req)
            .map_err(|e| McpError::Transport(format!("JSON serialize error: {}", e)))?;

        let http_req = self.build_request(&body).await;
        let response = http_req
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("HTTP request failed: {}", e)))?;

        if !response.status().is_success() {
            return Err(McpError::Transport(format!(
                "HTTP request returned status: {}",
                response.status()
            )));
        }

        let rpc_response = self.parse_response(response).await?;

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

        let http_req = self.build_request(&body).await;
        http_req
            .send()
            .await
            .map_err(|e| McpError::Transport(format!("Notification request failed: {}", e)))?;

        Ok(())
    }

    async fn close(&self) -> Result<(), McpError> {
        // No persistent connection to close for HTTP
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M-13 — a configured URL pointing at the cloud metadata IP must be
    /// rejected at connect time, before any auth header is attached.
    #[tokio::test]
    async fn connect_rejects_metadata_ip_url() {
        let headers = HashMap::new();
        let err = StreamableHttpTransport::connect("http://169.254.169.254/mcp", &headers)
            .await
            .expect_err("metadata-IP url must be rejected at connect");
        let msg = err.to_string();
        assert!(
            msg.contains("private or internal") || msg.contains("SSRF"),
            "expected an SSRF rejection, got: {msg}"
        );
    }

    /// M-13 — a loopback URL is likewise rejected (SSRF to localhost
    /// services).
    #[tokio::test]
    async fn connect_rejects_loopback_url() {
        let headers = HashMap::new();
        let err = StreamableHttpTransport::connect("http://127.0.0.1:8080/mcp", &headers)
            .await
            .expect_err("loopback url must be rejected at connect");
        assert!(
            err.to_string().contains("private or internal") || err.to_string().contains("SSRF")
        );
    }
}
