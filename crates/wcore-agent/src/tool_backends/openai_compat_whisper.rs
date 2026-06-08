//! Moved from monolith `tool_backends.rs` during v0.9.0 Wave-1 prep
//! (Sub-agent B0). The R-B1 fix: each backend lives in its own file so
//! parallel Wave-1 sub-agents can add new backend files without
//! colliding on `tool_backends.rs`.

use async_trait::async_trait;
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use wcore_tools::transcription_tools::{TranscriptionBackend, TranscriptionOutcome};

/// OpenAI-compatible Whisper backend. Drives both Groq's
/// `whisper-large-v3-turbo` and OpenAI's `whisper-1` since they share
/// the same multipart-form `/audio/transcriptions` API shape.
pub struct OpenAiCompatWhisperBackend {
    client: Client,
    api_key: String,
    endpoint: String,
    model: String,
    backend_id: &'static str,
}

impl OpenAiCompatWhisperBackend {
    pub fn new(api_key: String, endpoint: String, model: String, backend_id: &'static str) -> Self {
        Self {
            client: build_ssrf_safe_tool_client(),
            api_key,
            endpoint,
            model,
            backend_id,
        }
    }
}

#[async_trait]
impl TranscriptionBackend for OpenAiCompatWhisperBackend {
    async fn transcribe(
        &self,
        mime: &'static str,
        bytes: &[u8],
        language: Option<&str>,
    ) -> TranscriptionOutcome {
        let filename = match mime {
            "audio/mpeg" => "audio.mp3",
            "audio/mp4" => "audio.m4a",
            "audio/aac" => "audio.aac",
            "audio/wav" | "audio/x-wav" | "audio/wave" => "audio.wav",
            "audio/ogg" => "audio.ogg",
            "audio/webm" => "audio.webm",
            "audio/flac" => "audio.flac",
            _ => "audio.bin",
        };
        // Multipart form: file, model, optional language, request_json
        // response_format = verbose_json so we get language + segments.
        let file_part = reqwest::multipart::Part::bytes(bytes.to_vec())
            .file_name(filename.to_string())
            .mime_str(mime)
            .unwrap_or_else(|_| reqwest::multipart::Part::bytes(bytes.to_vec()));
        let mut form = reqwest::multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "verbose_json")
            .part("file", file_part);
        if let Some(lang) = language {
            form = form.text("language", lang.to_string());
        }
        let resp = match self
            .client
            .post(&self.endpoint)
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.api_key),
            )
            .timeout(std::time::Duration::from_secs(120))
            .multipart(form)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                return TranscriptionOutcome::Err {
                    message: format!("{} transcription request failed: {e}", self.backend_id),
                };
            }
        };
        let status = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return TranscriptionOutcome::Err {
                message: format!(
                    "{} transcription returned HTTP {}: {}",
                    self.backend_id,
                    status.as_u16(),
                    txt.chars().take(400).collect::<String>()
                ),
            };
        }
        let parsed: serde_json::Value = match serde_json::from_str(&txt) {
            Ok(v) => v,
            Err(e) => {
                return TranscriptionOutcome::Err {
                    message: format!("{} transcription JSON parse failed: {e}", self.backend_id),
                };
            }
        };
        let transcript = parsed
            .get("text")
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .unwrap_or_default();
        let language = parsed
            .get("language")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        // `segments` (whisper verbose_json) → our `TranscriptSegment`s.
        let segments = parsed
            .get("segments")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .map(|seg| wcore_tools::transcription_tools::TranscriptSegment {
                        start_seconds: seg.get("start").and_then(|v| v.as_f64()).unwrap_or(0.0)
                            as f32,
                        end_seconds: seg.get("end").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32,
                        text: seg
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        if transcript.is_empty() {
            return TranscriptionOutcome::Err {
                message: format!("{} transcription returned empty text", self.backend_id),
            };
        }
        TranscriptionOutcome::Ok {
            transcript,
            language,
            segments,
        }
    }
}
