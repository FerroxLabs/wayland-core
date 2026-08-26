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

/// Record on a fallback result that the primary was tried and failed.
///
/// Without this the degradation is invisible: the caller receives an ordinary
/// success from DuckDuckGo and has no way to learn that the backend the user
/// actually configured never ran. gh#1068 — a user whose `EXA_API_KEY` was
/// being ignored was advised to set a Brave key, because the only backend
/// anyone could see was the fallback.
///
/// The consumer is `wcore-cli`'s `tui::tool_formatters::web::WebFormatter`,
/// which renders it on the tool card. Adding the field without a renderer left
/// the user-visible symptom of gh#1068 exactly as it was: the note reached the
/// model inside the tool JSON and nobody else.
///
/// Additive only. `payload` is splice-merged into the final result object, so
/// a new key cannot disturb the `web` / `results` shapes callers match on. A
/// non-object payload is passed through untouched rather than coerced.
fn note_degraded(outcome: WebOutcome, primary: &str, fallback: &str, reason: &str) -> WebOutcome {
    match outcome {
        WebOutcome::Ok { mut payload } => {
            if let Some(map) = payload.as_object_mut() {
                map.insert(
                    "degraded_from".to_string(),
                    serde_json::json!({
                        "backend": primary,
                        "served_by": fallback,
                        "reason": reason,
                    }),
                );
            }
            WebOutcome::Ok { payload }
        }
        // Both failed - the dead end. This message is the ONLY thing the user
        // sees, so it has to carry all three: what was tried, what happened to
        // each, and the one next step. The fallback's own message is the one
        // worth leading with, but it is misleading on its own (it names
        // DuckDuckGo for a failure that began with the user's configured
        // backend), so the primary's reason travels with it. The remedy is
        // appended only if the inner message did not already carry it -
        // repeating it twice in one error reads as noise and gets skipped.
        WebOutcome::Err { message } => {
            let mut out = format!(
                "web search failed on every backend. Fallback '{fallback}': {message} \
                         (primary '{primary}' also failed: {reason})"
            );
            if !out.contains(crate::tool_backends::shared::WEB_SEARCH_KEY_REMEDY) {
                out.push(' ');
                out.push_str(crate::tool_backends::shared::WEB_SEARCH_KEY_REMEDY);
            }
            WebOutcome::Err { message: out }
        }
    }
}

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
                // WARN, not DEBUG: with `RUST_LOG` unset a debug! record is not
                // written anywhere at all, so the single most useful line for
                // diagnosing "web search is broken" did not survive the run.
                tracing::warn!(
                    "web search: primary '{}' failed ({message}); falling back to '{}'",
                    self.primary.backend_id(),
                    self.fallback.backend_id()
                );
                let out = self.fallback.search(query, limit).await;
                note_degraded(
                    out,
                    self.primary.backend_id(),
                    self.fallback.backend_id(),
                    &message,
                )
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
    async fn fallback_result_records_which_backend_was_skipped_and_why() {
        // gh#1068. The whole harm was that this degradation was invisible: the
        // caller saw a clean DuckDuckGo success and could not tell that the
        // configured backend had been refused, so users were diagnosed against
        // a backend that was never involved.
        let fb = Arc::new(CapturingWebBackend::new().with_search_payload(json!({
            "web": [{"title": "ddg", "url": "https://x/", "snippet": "ok"}]
        })));
        let chain = ChainedWebBackend::new(Arc::new(ErrBackend), fb);
        let WebOutcome::Ok { payload } = chain.search("q", 5).await else {
            panic!("fallback should have served this");
        };
        let note = payload
            .get("degraded_from")
            .expect("a served-by-fallback result must say so");
        assert_eq!(note.get("backend").and_then(|v| v.as_str()), Some("err"));
        assert_eq!(
            note.get("served_by").and_then(|v| v.as_str()),
            Some("capturing")
        );
        assert!(
            note.get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .contains("boom"),
            "the primary's actual failure reason must survive, not just the fact of it"
        );
        // The note is additive: the payload callers match on is untouched.
        assert!(payload.get("web").is_some(), "results must be unchanged");
    }

    #[tokio::test]
    async fn a_successful_primary_is_not_marked_degraded() {
        // Guards the obvious way to "pass" the test above — annotating always.
        let primary = Arc::new(CapturingWebBackend::new().with_search_payload(json!({
            "web": [{"title": "p", "url": "https://p/", "snippet": "ok"}]
        })));
        let chain = ChainedWebBackend::new(primary, Arc::new(CapturingWebBackend::new()));
        let WebOutcome::Ok { payload } = chain.search("q", 5).await else {
            panic!("primary should have served this");
        };
        assert!(
            payload.get("degraded_from").is_none(),
            "a result the primary served is not degraded"
        );
    }

    #[tokio::test]
    async fn when_both_fail_the_primary_reason_is_not_lost() {
        // The fallback's own error alone is actively misleading — it names
        // DuckDuckGo for a failure that started with the user's configured
        // backend being refused.
        let chain = ChainedWebBackend::new(Arc::new(ErrBackend), Arc::new(ErrBackend));
        let WebOutcome::Err { message } = chain.search("q", 5).await else {
            panic!("both arms fail, so this must be an Err");
        };
        assert!(
            message.contains("primary 'err'"),
            "the primary that failed first must be named: {message}"
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
