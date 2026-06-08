// M4.7 — Voyage AI embeddings backend.
//
// Calls Voyage's `/v1/embeddings` endpoint over HTTPS and adapts the
// response to the `Embedder` trait's cross-backend contract:
//   * L2-normalized output (Voyage normalizes by default, but we
//     re-normalize defensively so callers can't tell backends apart by
//     vector magnitude).
//   * Stable `dim()` derived from the configured model.
//   * `name()` is a `&'static str` cached via `String::leak` at
//     construction so the trait stays object-safe.
//
// The file compiles unconditionally — only the live HTTP integration
// test (`tests/embedder_voyage_live.rs`) sits behind the
// `live-voyage` Cargo feature. That keeps clippy + the default
// nextest path covering this file without spending an API key.

use serde::Deserialize;

use super::Embedder;
use crate::error::{MemoryError, Result};

/// Default model when `EmbedderConfig::model` is `None`.
pub const DEFAULT_MODEL: &str = "voyage-2";

/// Voyage's published embedding endpoint.
const VOYAGE_ENDPOINT: &str = "https://api.voyageai.com/v1/embeddings";

/// Voyage AI embeddings backend.
///
/// Cheap to clone — wraps a shared `reqwest::Client` plus a few
/// immutable strings. Construct via `VoyageEmbedder::new` with an API
/// key + optional model override.
#[derive(Clone)]
pub struct VoyageEmbedder {
    http: wcore_egress::EgressClient,
    api_key: String,
    model: String,
    dim: usize,
    /// Cached "voyage/<model>/<dim>" identifier. Leaked at construction
    /// so the trait's `&'static str` return stays cheap and stable.
    cached_name: &'static str,
}

// Manual Debug — the auto-derived impl would leak `api_key` into any
// `{:?}` formatting (panic messages, tracing spans). Print only the
// non-sensitive surface.
impl std::fmt::Debug for VoyageEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VoyageEmbedder")
            .field("model", &self.model)
            .field("dim", &self.dim)
            .field("api_key", &"<redacted>")
            .finish()
    }
}

impl VoyageEmbedder {
    /// Construct a Voyage embedder. `model = None` selects the default
    /// `voyage-2` (1024-dim) backend. Errors if the model is unknown —
    /// we hard-code dims for the published Voyage models so callers
    /// don't have to round-trip the API just to size a sqlite-vec
    /// schema (M4.8).
    pub async fn new(api_key: impl Into<String>, model: Option<String>) -> Result<Self> {
        let api_key = api_key.into();
        if api_key.is_empty() {
            return Err(MemoryError::Embedding(
                "VoyageEmbedder: empty API key".into(),
            ));
        }
        let model = model.unwrap_or_else(|| DEFAULT_MODEL.to_string());
        let dim = dim_for_model(&model)?;
        let cached_name: &'static str = Box::leak(format!("voyage/{model}/{dim}").into_boxed_str());

        let http = wcore_egress::EgressClient::builder()
            .build()
            .map_err(|e| MemoryError::Embedding(format!("VoyageEmbedder: build client: {e}")))?;

        Ok(Self {
            http,
            api_key,
            model,
            dim,
            cached_name,
        })
    }
}

/// Hard-coded dim table for published Voyage models. Keeping this
/// local (instead of probing the API) keeps `dim()` synchronous and
/// schema migrations deterministic.
fn dim_for_model(model: &str) -> Result<usize> {
    match model {
        "voyage-2" => Ok(1024),
        "voyage-large-2" => Ok(1536),
        "voyage-code-2" => Ok(1536),
        other => Err(MemoryError::Embedding(format!(
            "VoyageEmbedder: unknown model `{other}` — add it to dim_for_model() with its published dimensionality"
        ))),
    }
}

#[derive(Deserialize)]
struct VoyageResponse {
    data: Vec<VoyageEmbedding>,
}

#[derive(Deserialize)]
struct VoyageEmbedding {
    embedding: Vec<f32>,
}

#[async_trait::async_trait]
impl Embedder for VoyageEmbedder {
    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        // Voyage's API requires `input` to be an array even for a
        // single text — passing a bare string is a 400. Wrap
        // unconditionally so callers don't have to know.
        let body = serde_json::json!({
            "input": [text],
            "model": self.model,
        });

        let resp = self
            .http
            .post(VOYAGE_ENDPOINT)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| MemoryError::Embedding(format!("VoyageEmbedder: send: {e}")))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp
                .text()
                .await
                .unwrap_or_else(|_| "<no body>".to_string());
            return Err(MemoryError::Embedding(format!(
                "VoyageEmbedder: HTTP {status}: {body}"
            )));
        }

        let parsed: VoyageResponse = resp
            .json()
            .await
            .map_err(|e| MemoryError::Embedding(format!("VoyageEmbedder: decode: {e}")))?;

        let mut vector = parsed
            .data
            .into_iter()
            .next()
            .ok_or_else(|| {
                MemoryError::Embedding("VoyageEmbedder: response had empty `data`".into())
            })?
            .embedding;

        if vector.len() != self.dim {
            return Err(MemoryError::Embedding(format!(
                "VoyageEmbedder: expected {} dims for {}, got {}",
                self.dim,
                self.model,
                vector.len()
            )));
        }

        // Defensive L2-normalize. Voyage normalizes by default but the
        // trait's cross-backend contract demands it, so callers don't
        // have to trust the upstream.
        l2_normalize(&mut vector);
        if !vector.iter().all(|v| v.is_finite()) {
            return Err(MemoryError::Embedding(
                "VoyageEmbedder: non-finite vector after normalize".into(),
            ));
        }
        Ok(vector)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn name(&self) -> &'static str {
        self.cached_name
    }
}

fn l2_normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| (x * x) as f64).sum::<f64>().sqrt() as f32;
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    } else if !v.is_empty() {
        // All-zero vector — emit a sentinel unit so cosine is defined.
        v[0] = 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dim_table_covers_published_models() {
        assert_eq!(dim_for_model("voyage-2").unwrap(), 1024);
        assert_eq!(dim_for_model("voyage-large-2").unwrap(), 1536);
        assert_eq!(dim_for_model("voyage-code-2").unwrap(), 1536);
        assert!(dim_for_model("voyage-imaginary-99").is_err());
    }

    #[tokio::test]
    async fn rejects_empty_api_key() {
        let err = VoyageEmbedder::new("", None).await.unwrap_err();
        match err {
            MemoryError::Embedding(msg) => assert!(msg.contains("empty API key")),
            other => panic!("expected Embedding error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn rejects_unknown_model() {
        let err = VoyageEmbedder::new("sk-fake", Some("voyage-unknown".into()))
            .await
            .unwrap_err();
        match err {
            MemoryError::Embedding(msg) => assert!(msg.contains("unknown model")),
            other => panic!("expected Embedding error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn metadata_matches_default_model() {
        let e = VoyageEmbedder::new("sk-fake", None).await.unwrap();
        assert_eq!(e.dim(), 1024);
        assert_eq!(e.name(), "voyage/voyage-2/1024");
    }

    #[tokio::test]
    async fn metadata_reflects_model_override() {
        let e = VoyageEmbedder::new("sk-fake", Some("voyage-large-2".into()))
            .await
            .unwrap();
        assert_eq!(e.dim(), 1536);
        assert_eq!(e.name(), "voyage/voyage-large-2/1536");
    }

    #[test]
    fn l2_normalize_zero_vector_falls_back_to_unit() {
        let mut v = vec![0.0f32; 4];
        l2_normalize(&mut v);
        assert_eq!(v, vec![1.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn l2_normalize_makes_unit_norm() {
        let mut v = vec![3.0f32, 4.0];
        l2_normalize(&mut v);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm = {norm}");
    }
}
