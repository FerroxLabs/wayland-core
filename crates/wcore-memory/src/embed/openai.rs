// M4.6 — OpenAI embeddings backend.
//
// Calls POST https://api.openai.com/v1/embeddings against the configured
// model (default `text-embedding-3-small`, 1536-dim) and L2-normalizes the
// response so the cross-backend post-condition declared on `Embedder`
// holds. OpenAI's API does NOT pre-normalize, so we do it here once per
// call — `embed::cosine` assumes normalized inputs.
//
// Errors flow through `MemoryError::Embedding(String)` per
// PLAN-AMENDMENTS §C2 (no new error variants). Every failure mode —
// network drop, non-200 status, malformed JSON, empty `data` array,
// dimension mismatch — surfaces with a descriptive prefix so operators
// can correlate against the engine log.
//
// The module compiles unconditionally; the live integration test that
// actually hits the OpenAI endpoint lives under `tests/` behind the
// `live-openai` Cargo feature.

use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use super::Embedder;
use crate::error::{MemoryError, Result};

/// Default model when `EmbedderConfig::model` is `None`. 1536-dim,
/// cheapest of the v3 series, and the one we benchmark against in the
/// dream-cycle eval suite.
pub const DEFAULT_MODEL: &str = "text-embedding-3-small";

/// Documented embedding dimensionality for the OpenAI models we
/// recognize. Looking up the dim from a string keeps `dim()` cheap (no
/// startup API call) and consistent with the sqlite-vec migration check
/// in M4.8 — the embedder's `dim()` is queried *before* the first embed.
fn known_dim(model: &str) -> Option<usize> {
    match model {
        "text-embedding-3-small" | "text-embedding-ada-002" => Some(1536),
        "text-embedding-3-large" => Some(3072),
        _ => None,
    }
}

/// OpenAI embeddings backend. Construct via [`OpenAiEmbedder::new`] which
/// validates the API key and resolves the model dimensionality up front.
///
/// Cheap to clone if a caller needs to fan out — the inner `reqwest::Client`
/// already pools connections internally.
///
/// `Debug` is implemented by hand so the API key never appears in panic
/// payloads or trace logs.
pub struct OpenAiEmbedder {
    client: wcore_egress::EgressClient,
    api_key: String,
    model: String,
    dim: usize,
    /// Cached `"openai/{model}/{dim}"` leaked to `'static` once at
    /// construction so `name()` can return `&'static str` per the trait
    /// contract without re-allocating on every call.
    name: &'static str,
    /// Override for the API base URL. Production defaults to the public
    /// OpenAI endpoint; tests can point this at a wiremock instance.
    base_url: String,
}

impl std::fmt::Debug for OpenAiEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OpenAiEmbedder")
            .field("model", &self.model)
            .field("dim", &self.dim)
            .field("name", &self.name)
            .field("base_url", &self.base_url)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl OpenAiEmbedder {
    /// Construct a backend hitting the public OpenAI endpoint.
    ///
    /// `api_key` is the raw token (the config-layer caller is responsible
    /// for resolving the env var named in `EmbedderConfig::api_key_env`).
    /// `model` of `None` defaults to [`DEFAULT_MODEL`]. Unknown models are
    /// rejected up front rather than at first-embed time so a misconfigured
    /// `wcore.toml` fails fast.
    pub fn new(api_key: impl Into<String>, model: Option<&str>) -> Result<Self> {
        Self::with_base_url(api_key, model, "https://api.openai.com")
    }

    /// Variant used by integration tests to redirect to a mock server.
    /// Production code goes through [`Self::new`].
    pub fn with_base_url(
        api_key: impl Into<String>,
        model: Option<&str>,
        base_url: impl Into<String>,
    ) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(MemoryError::Embedding(
                "openai embedder: api_key is empty".into(),
            ));
        }

        let model = model.unwrap_or(DEFAULT_MODEL).to_string();
        let dim = known_dim(&model).ok_or_else(|| {
            MemoryError::Embedding(format!(
                "openai embedder: unknown model {model:?} — known: \
                 text-embedding-3-small (1536), text-embedding-3-large (3072), \
                 text-embedding-ada-002 (1536)"
            ))
        })?;

        // Leak once at construction to satisfy the `&'static str` trait
        // contract on `name()`. One leak per embedder instance — cheap
        // compared to the alternative of returning `String` and forcing
        // every caller to clone or sprinkle `Box::leak` themselves.
        let name: &'static str = Box::leak(format!("openai/{model}/{dim}").into_boxed_str());

        Ok(Self {
            // B1: route through the egress chokepoint like every other
            // network client. A bare default (no extra timeout policy) keeps
            // prior behavior; the egress policy is enforced in EgressClient.
            client: wcore_egress::EgressClient::new(),
            api_key,
            model,
            dim,
            name,
            base_url: base_url.into(),
        })
    }
}

/// Wire shape of `POST /v1/embeddings` for the fields we consume.
/// `usage` and `object` are intentionally ignored — we only need the
/// embedding vector itself.
#[derive(Debug, Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

#[derive(Debug, Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
}

fn l2_normalize(v: &mut [f32]) {
    // f64 accumulator to keep precision steady on 3072-dim vectors —
    // single-precision sum-of-squares can drift noticeably at that size.
    let norm = v
        .iter()
        .map(|x| (*x as f64) * (*x as f64))
        .sum::<f64>()
        .sqrt() as f32;
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    } else if !v.is_empty() {
        // Defensive: if OpenAI ever returns a zero vector (it shouldn't),
        // emit a sentinel unit so cosine stays well defined. Matches the
        // HashedEmbedder fallback so downstream code doesn't branch.
        v[0] = 1.0;
    }
}

#[async_trait]
impl Embedder for OpenAiEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/v1/embeddings", self.base_url);
        let body = json!({
            "input": text,
            "model": self.model,
        });

        let response = self
            .client
            .post(&url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                MemoryError::Embedding(format!("openai embeddings request failed: {e}"))
            })?;

        let status = response.status();
        if !status.is_success() {
            // Pull the body for diagnosability — OpenAI puts the actual
            // failure reason (auth, model, quota) in the JSON error
            // object. Truncate aggressively so a giant HTML 502 page from
            // an intermediate proxy doesn't blow up the log line.
            let body_text = response.text().await.unwrap_or_default();
            let truncated = if body_text.len() > 512 {
                format!("{}…", &body_text[..512])
            } else {
                body_text
            };
            return Err(MemoryError::Embedding(format!(
                "openai embeddings HTTP {}: {truncated}",
                status.as_u16()
            )));
        }

        let parsed: EmbeddingResponse = response.json().await.map_err(|e| {
            MemoryError::Embedding(format!("openai embeddings response decode failed: {e}"))
        })?;

        let mut vec = parsed
            .data
            .into_iter()
            .next()
            .ok_or_else(|| {
                MemoryError::Embedding("openai embeddings response had empty `data` array".into())
            })?
            .embedding;

        if vec.len() != self.dim {
            return Err(MemoryError::Embedding(format!(
                "openai embeddings returned {} dims, expected {} for model {}",
                vec.len(),
                self.dim,
                self.model
            )));
        }

        l2_normalize(&mut vec);

        if !vec.iter().all(|v| v.is_finite()) {
            return Err(MemoryError::Embedding(
                "openai embeddings: non-finite values after L2-normalization".into(),
            ));
        }

        Ok(vec)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn name(&self) -> &'static str {
        self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dim_recognizes_documented_models() {
        assert_eq!(known_dim("text-embedding-3-small"), Some(1536));
        assert_eq!(known_dim("text-embedding-3-large"), Some(3072));
        assert_eq!(known_dim("text-embedding-ada-002"), Some(1536));
        assert_eq!(known_dim("text-embedding-99-future"), None);
    }

    #[test]
    fn rejects_empty_api_key() {
        let err = OpenAiEmbedder::new("", None).unwrap_err();
        assert!(matches!(err, MemoryError::Embedding(_)));
        assert!(err.to_string().contains("api_key"));
    }

    #[test]
    fn rejects_unknown_model() {
        let err = OpenAiEmbedder::new("sk-test", Some("text-embedding-99-future")).unwrap_err();
        assert!(matches!(err, MemoryError::Embedding(_)));
        assert!(err.to_string().contains("unknown model"));
    }

    #[test]
    fn name_encodes_model_and_dim() {
        let e = OpenAiEmbedder::new("sk-test", None).unwrap();
        assert_eq!(e.name(), "openai/text-embedding-3-small/1536");
        assert_eq!(e.dim(), 1536);

        let e = OpenAiEmbedder::new("sk-test", Some("text-embedding-3-large")).unwrap();
        assert_eq!(e.name(), "openai/text-embedding-3-large/3072");
        assert_eq!(e.dim(), 3072);
    }

    #[test]
    fn l2_normalize_unit_input() {
        // Already a unit vector — must come out the same shape.
        let mut v = vec![1.0f32, 0.0, 0.0, 0.0];
        l2_normalize(&mut v);
        assert!((v[0] - 1.0).abs() < 1e-6);
        assert!(v[1..].iter().all(|x| x.abs() < 1e-6));
    }

    #[test]
    fn l2_normalize_arbitrary_input() {
        let mut v = vec![3.0f32, 4.0, 0.0]; // norm = 5
        l2_normalize(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-6,
            "post-norm should be 1.0, got {norm}"
        );
        assert!((v[0] - 0.6).abs() < 1e-6);
        assert!((v[1] - 0.8).abs() < 1e-6);
    }

    #[test]
    fn l2_normalize_zero_vector_falls_back_to_sentinel() {
        let mut v = vec![0.0f32; 4];
        l2_normalize(&mut v);
        // Sentinel keeps cosine well defined.
        assert_eq!(v[0], 1.0);
    }
}
