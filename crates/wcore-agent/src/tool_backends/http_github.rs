//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use super::{build_ssrf_safe_tool_client, error_message, parse_json_or_raw};
use wcore_tools::github_tool::{
    GitHubBackend, GitHubOutcome, GitHubRequest, HttpMethod as GhMethod,
};

// ---------------------------------------------------------------------
// GitHub.
// ---------------------------------------------------------------------

/// Real GitHub REST backend over `reqwest`.
pub struct HttpGitHubBackend {
    client: Client,
}

impl HttpGitHubBackend {
    /// New backend with the non-streaming HTTP timeout policy (AUDIT B-5)
    /// plus the SSRF-resistant redirect policy (#279) — see
    /// [`build_ssrf_safe_tool_client`].
    pub fn new() -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
        }
    }
}

impl Default for HttpGitHubBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl GitHubBackend for HttpGitHubBackend {
    async fn dispatch(&self, request: &GitHubRequest) -> GitHubOutcome {
        let mut builder = match request.method {
            GhMethod::Get => self.client.get(&request.url),
            GhMethod::Post => self.client.post(&request.url),
            GhMethod::Put => self.client.put(&request.url),
        };
        for (name, value) in &request.headers {
            builder = builder.header(name, value);
        }
        if let Some(body) = &request.body {
            builder = builder.json(body);
        }
        let response = match builder.send().await {
            Ok(r) => r,
            Err(e) => {
                return GitHubOutcome::Err {
                    message: format!("GitHub request transport error: {e}"),
                };
            }
        };
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        let payload = parse_json_or_raw(&text);
        if status.is_success() {
            GitHubOutcome::Ok { payload }
        } else {
            GitHubOutcome::HttpError {
                status: status.as_u16(),
                message: error_message(
                    &payload,
                    &format!("GitHub returned HTTP {}", status.as_u16()),
                ),
            }
        }
    }
}
