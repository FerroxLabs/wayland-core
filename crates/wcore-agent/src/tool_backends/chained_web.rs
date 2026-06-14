//! Generic primary → fallback wrapper for `WebBackend`.
//!
//! The factory wires every selected search backend (Firecrawl, Parallel,
//! Tavily, Exa, SearXNG, Brave) as `ChainedWebBackend { primary, DuckDuckGo }`
//! so search never hard-fails: if the primary returns a structured `Err`
//! (transport failure, non-2xx, unparseable / zombie payload, or a
//! validation-rejected empty set), the call falls through to DuckDuckGo.
//!
//! A *successful* primary response is final — including a legitimately empty
//! one would never reach here, because every backend returns `Err` (not
//! `Ok{web:[]}`) when it has no valid results, matching the existing
//! DuckDuckGo convention. So `Ok` always carries real results.

use std::sync::Arc;

use async_trait::async_trait;
use wcore_tools::web_tools::{CrawlRequest, ExtractRequest, WebBackend, WebOutcome};

/// Wraps a primary backend with a fallback (always DuckDuckGo in practice).
/// On any primary `Err`, the same operation is retried on the fallback.
pub struct ChainedWebBackend {
    primary: Arc<dyn WebBackend>,
    fallback: Arc<dyn WebBackend>,
}

impl ChainedWebBackend {
    pub fn new(primary: Arc<dyn WebBackend>, fallback: Arc<dyn WebBackend>) -> Self {
        Self { primary, fallback }
    }
}

#[async_trait]
impl WebBackend for ChainedWebBackend {
    async fn search(&self, query: &str, limit: u32) -> WebOutcome {
        match self.primary.search(query, limit).await {
            WebOutcome::Ok { payload } => WebOutcome::Ok { payload },
            WebOutcome::Err { message } => {
                tracing::debug!(
                    "web search: primary '{}' failed ({message}); falling back to '{}'",
                    self.primary.backend_id(),
                    self.fallback.backend_id()
                );
                self.fallback.search(query, limit).await
            }
        }
    }

    async fn extract(&self, req: ExtractRequest) -> WebOutcome {
        // Primary first (Firecrawl can extract); only fall back on Err.
        match self.primary.extract(req.clone()).await {
            WebOutcome::Ok { payload } => WebOutcome::Ok { payload },
            WebOutcome::Err { .. } => self.fallback.extract(req).await,
        }
    }

    async fn crawl(&self, req: CrawlRequest) -> WebOutcome {
        match self.primary.crawl(req.clone()).await {
            WebOutcome::Ok { payload } => WebOutcome::Ok { payload },
            WebOutcome::Err { .. } => self.fallback.crawl(req).await,
        }
    }

    fn backend_id(&self) -> &str {
        "chained"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wcore_tools::web_tools::CapturingWebBackend;

    /// Tiny double that always errors — stands in for an unreachable primary.
    struct ErrBackend;
    #[async_trait]
    impl WebBackend for ErrBackend {
        async fn search(&self, _q: &str, _l: u32) -> WebOutcome {
            WebOutcome::Err {
                message: "boom".into(),
            }
        }
        async fn extract(&self, _r: ExtractRequest) -> WebOutcome {
            WebOutcome::Err {
                message: "boom".into(),
            }
        }
        async fn crawl(&self, _r: CrawlRequest) -> WebOutcome {
            WebOutcome::Err {
                message: "boom".into(),
            }
        }
        fn backend_id(&self) -> &str {
            "err"
        }
    }

    #[tokio::test]
    async fn falls_back_to_fallback_on_primary_error() {
        let fb = Arc::new(CapturingWebBackend::new().with_search_payload(json!({
            "web": [{"title": "ddg", "url": "https://x/", "snippet": "ok"}]
        })));
        let chain = ChainedWebBackend::new(Arc::new(ErrBackend), fb.clone());
        let out = chain.search("q", 5).await;
        assert!(
            matches!(out, WebOutcome::Ok { .. }),
            "should serve fallback result"
        );
        assert_eq!(
            fb.snapshot().len(),
            1,
            "fallback must be invoked exactly once"
        );
    }

    #[tokio::test]
    async fn does_not_fall_back_when_primary_succeeds() {
        let primary = Arc::new(CapturingWebBackend::new().with_search_payload(json!({
            "web": [{"title": "p", "url": "https://p/", "snippet": "ok"}]
        })));
        let fb = Arc::new(CapturingWebBackend::new());
        let chain = ChainedWebBackend::new(primary, fb.clone());
        let out = chain.search("q", 5).await;
        assert!(matches!(out, WebOutcome::Ok { .. }));
        assert_eq!(
            fb.snapshot().len(),
            0,
            "fallback must NOT be touched on primary success"
        );
    }

    #[tokio::test]
    async fn extract_prefers_primary_then_falls_back() {
        let fb = Arc::new(CapturingWebBackend::new().with_extract_payload(json!({"results": []})));
        let chain = ChainedWebBackend::new(Arc::new(ErrBackend), fb.clone());
        let req = ExtractRequest {
            urls: vec!["https://x/".into()],
            format: None,
            use_llm_processing: false,
        };
        let out = chain.extract(req).await;
        assert!(matches!(out, WebOutcome::Ok { .. }));
        assert_eq!(fb.snapshot().len(), 1);
    }
}
