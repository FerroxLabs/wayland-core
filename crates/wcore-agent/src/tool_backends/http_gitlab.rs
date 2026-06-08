//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use serde_json::Value;
use wcore_egress::EgressClient as Client;

use super::{build_ssrf_safe_tool_client, parse_json_or_raw};
use wcore_tools::gitlab_tool::{
    GitLabBackend, GitLabOutcome, GitLabRequest, HttpMethod as GlMethod,
};

// ---------------------------------------------------------------------
// GitLab.
// ---------------------------------------------------------------------

/// Real GitLab REST backend over `reqwest`.
pub struct HttpGitLabBackend {
    client: Client,
}

impl HttpGitLabBackend {
    /// New backend with the non-streaming HTTP timeout policy (AUDIT B-5)
    /// plus the SSRF-resistant redirect policy (#279) — see
    /// [`build_ssrf_safe_tool_client`].
    pub fn new() -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
        }
    }
}

impl Default for HttpGitLabBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl GitLabBackend for HttpGitLabBackend {
    async fn dispatch(&self, request: &GitLabRequest) -> GitLabOutcome {
        let mut builder = match request.method {
            GlMethod::Get => self.client.get(&request.url),
            GlMethod::Post => self.client.post(&request.url),
        };
        // GitLabRequest exposes auth via `.headers()` (PRIVATE-TOKEN +
        // Content-Type when a body is present), not an embedded vec.
        for (name, value) in request.headers() {
            builder = builder.header(name, value);
        }
        if let Some(body) = &request.body {
            builder = builder.json(body);
        }
        let response = match builder.send().await {
            Ok(r) => r,
            Err(e) => {
                return GitLabOutcome::Err {
                    message: format!("GitLab request transport error: {e}"),
                    status_code: None,
                };
            }
        };
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if status.is_success() {
            // `get_file` hits the "raw" endpoint and returns the file
            // body verbatim — wrap it in a JSON object per the tool's
            // documented contract. Every other action returns JSON.
            if request.action == "get_file" {
                GitLabOutcome::Ok {
                    payload: serde_json::json!({ "content": text }),
                }
            } else {
                GitLabOutcome::Ok {
                    payload: parse_json_or_raw(&text),
                }
            }
        } else {
            let payload = parse_json_or_raw(&text);
            // GitLab errors use either `message` or `error` fields.
            let msg = payload
                .get("message")
                .and_then(Value::as_str)
                .or_else(|| payload.get("error").and_then(Value::as_str))
                .map(str::to_string)
                .unwrap_or_else(|| format!("GitLab returned HTTP {}", status.as_u16()));
            GitLabOutcome::Err {
                message: msg,
                status_code: Some(status.as_u16()),
            }
        }
    }
}
