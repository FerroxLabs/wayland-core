//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use base64::Engine as _;
use wcore_tools::vision_tools::{VisionBackend, VisionOutcome};

/// OpenAI vision backend (GPT-4o). Uses the chat-completions endpoint
/// with an `image_url` content block carrying a base64 `data:` URL.
pub struct OpenAiVisionBackend {
    client: Client,
    api_key: String,
    model: String,
}

impl OpenAiVisionBackend {
    pub fn new(api_key: String) -> Self {
        let model = std::env::var("WAYLAND_VISION_MODEL").unwrap_or_else(|_| "gpt-4o".to_string());
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            model,
        }
    }
}

#[async_trait]
impl VisionBackend for OpenAiVisionBackend {
    async fn analyze(&self, mime: &'static str, bytes: &[u8], prompt: &str) -> VisionOutcome {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        let data_url = format!("data:{mime};base64,{b64}");
        let body = serde_json::json!({
            "model": self.model,
            "max_tokens": 1024,
            "messages": [{
                "role": "user",
                "content": [
                    { "type": "image_url", "image_url": { "url": data_url } },
                    { "type": "text", "text": prompt }
                ]
            }]
        });
        let resp = match self
            .client
            .post("https://api.openai.com/v1/chat/completions")
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.api_key),
            )
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .timeout(std::time::Duration::from_secs(60))
            .body(body.to_string())
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return VisionOutcome::Err {
                    message: format!("openai vision request failed: {e}"),
                };
            }
        };
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return VisionOutcome::Err {
                message: format!(
                    "openai vision returned HTTP {}: {}",
                    status.as_u16(),
                    txt.chars().take(400).collect::<String>()
                ),
            };
        }
        let parsed: serde_json::Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                return VisionOutcome::Err {
                    message: format!("openai vision JSON parse failed: {e}"),
                };
            }
        };
        let analysis = parsed
            .pointer("/choices/0/message/content")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();
        if analysis.is_empty() {
            return VisionOutcome::Err {
                message: "openai vision returned no text content".to_string(),
            };
        }
        VisionOutcome::Ok { analysis }
    }
}
