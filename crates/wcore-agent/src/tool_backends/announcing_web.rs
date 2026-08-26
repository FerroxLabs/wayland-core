//! Delivers backend-selection notices to the USER, on the first search.
//!
//! Two notices are produced while the backend is being chosen at boot: the
//! keyless Parallel.ai privacy disclosure, and a warning that
//! `WAYLAND_WEB_BACKEND` was set to a value nothing recognises. Both used to be
//! written with `eprintln!` from `build_web_search_backend`, which is called
//! from `Bootstrap::build()` — and `main.rs` enters the TUI alt-screen BEFORE
//! it calls `build()`. So in the product's default mode the notice was painted
//! onto a buffer the splash immediately overwrote and `LeaveAlternateScreen`
//! then discarded. The privacy disclosure additionally burned its
//! once-per-user marker on that unseen write, permanently suppressing itself
//! for every later headless run where stderr would have worked.
//!
//! So the emission is moved off the boot path and onto the tool result, which
//! every mode renders: the TUI tool card, headless print output, and the JSON
//! stream protocol alike. The notice therefore rides the FIRST search rather
//! than preceding it — the user learns at their first query instead of never,
//! and can act before the second — and the marker is written only once the
//! notice has actually been attached to something the user reads.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use wcore_tools::web_tools::{CrawlRequest, ExtractRequest, WebBackend, WebOutcome};

/// One pending user-facing notice about how web search was configured.
pub struct WebNotice {
    pub text: String,
    /// Written only once `text` has been delivered. `None` means "show once
    /// per process"; `Some(path)` means "show once per user, and this file is
    /// the record". Nothing is written on a notice that was never shown.
    pub marker_on_delivery: Option<PathBuf>,
}

/// Wraps the selected backend and attaches any pending notices to the first
/// search result. Every later call is untouched.
pub struct AnnouncingWebBackend {
    inner: Arc<dyn WebBackend>,
    pending: Mutex<Vec<WebNotice>>,
}

impl AnnouncingWebBackend {
    pub fn new(inner: Arc<dyn WebBackend>, notices: Vec<WebNotice>) -> Self {
        Self {
            inner,
            pending: Mutex::new(notices),
        }
    }

    /// Wrap only when there is something to say — an unwrapped backend is one
    /// less layer on the hot path and one less thing to reason about.
    pub fn wrap(inner: Arc<dyn WebBackend>, notices: Vec<WebNotice>) -> Arc<dyn WebBackend> {
        if notices.is_empty() {
            return inner;
        }
        Arc::new(Self::new(inner, notices))
    }

    /// Take the pending notices, if any. Draining under the lock is what makes
    /// "first search only" true when several searches run concurrently.
    fn take(&self) -> Vec<WebNotice> {
        std::mem::take(&mut *self.pending.lock())
    }
}

/// Splice the notices into whatever the backend returned.
///
/// On success the text becomes a `notice` key beside the results (additive:
/// the `web` / `results` shapes callers match on are untouched). On failure it
/// is appended to the error message, because a failed search's message is the
/// only thing the user sees at all.
fn attach(outcome: WebOutcome, notices: &[WebNotice]) -> WebOutcome {
    let joined = notices
        .iter()
        .map(|n| n.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    match outcome {
        WebOutcome::Ok { mut payload } => {
            if let Some(map) = payload.as_object_mut() {
                map.insert("notice".to_string(), serde_json::Value::String(joined));
            }
            WebOutcome::Ok { payload }
        }
        WebOutcome::Err { message } => WebOutcome::Err {
            message: format!("{message}\n\n{joined}"),
        },
    }
}

#[async_trait]
impl WebBackend for AnnouncingWebBackend {
    async fn search(&self, query: &str, limit: u32) -> WebOutcome {
        let notices = self.take();
        let out = self.inner.search(query, limit).await;
        if notices.is_empty() {
            return out;
        }
        let out = attach(out, &notices);
        // Delivered — only now is it honest to record that the user was told.
        for n in &notices {
            let Some(marker) = n.marker_on_delivery.as_ref() else {
                continue;
            };
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(marker, b"1");
        }
        out
    }

    async fn extract(&self, req: ExtractRequest) -> WebOutcome {
        self.inner.extract(req).await
    }

    async fn crawl(&self, req: CrawlRequest) -> WebOutcome {
        self.inner.crawl(req).await
    }

    fn backend_id(&self) -> &str {
        self.inner.backend_id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wcore_tools::web_tools::CapturingWebBackend;

    struct ErrBackend;
    #[async_trait]
    impl WebBackend for ErrBackend {
        async fn search(&self, _q: &str, _l: u32) -> WebOutcome {
            WebOutcome::Err {
                message: "everything failed".into(),
            }
        }
        async fn extract(&self, _r: ExtractRequest) -> WebOutcome {
            WebOutcome::Err {
                message: "everything failed".into(),
            }
        }
        async fn crawl(&self, _r: CrawlRequest) -> WebOutcome {
            WebOutcome::Err {
                message: "everything failed".into(),
            }
        }
        fn backend_id(&self) -> &str {
            "err"
        }
    }

    fn notice(text: &str, marker: Option<PathBuf>) -> WebNotice {
        WebNotice {
            text: text.to_string(),
            marker_on_delivery: marker,
        }
    }

    fn ok_backend() -> Arc<dyn WebBackend> {
        Arc::new(CapturingWebBackend::new().with_search_payload(json!({
            "web": [{"title": "a", "url": "https://a/", "snippet": "s"}]
        })))
    }

    #[tokio::test]
    async fn the_notice_reaches_a_successful_result() {
        let b = AnnouncingWebBackend::new(ok_backend(), vec![notice("queries go to x", None)]);
        let WebOutcome::Ok { payload } = b.search("q", 3).await else {
            panic!("expected Ok");
        };
        assert_eq!(
            payload.get("notice").and_then(|v| v.as_str()),
            Some("queries go to x")
        );
        assert!(payload.get("web").is_some(), "results must be untouched");
    }

    /// A failed search is the case where the user has nothing else to read, so
    /// the notice must survive there too.
    #[tokio::test]
    async fn the_notice_reaches_a_failed_result() {
        let b = AnnouncingWebBackend::new(
            Arc::new(ErrBackend),
            vec![notice("set TAVILY_API_KEY", None)],
        );
        let WebOutcome::Err { message } = b.search("q", 3).await else {
            panic!("expected Err");
        };
        assert!(message.contains("everything failed"));
        assert!(message.contains("set TAVILY_API_KEY"), "got: {message}");
    }

    #[tokio::test]
    async fn it_is_said_once_not_on_every_search() {
        let b = AnnouncingWebBackend::new(ok_backend(), vec![notice("once", None)]);
        let first = b.search("q", 3).await;
        let second = b.search("q", 3).await;
        let (WebOutcome::Ok { payload: p1 }, WebOutcome::Ok { payload: p2 }) = (first, second)
        else {
            panic!("expected two Ok results");
        };
        assert!(p1.get("notice").is_some(), "first search carries it");
        assert!(p2.get("notice").is_none(), "second search must not repeat");
    }

    /// The defect that made the privacy disclosure permanently invisible: the
    /// marker was written next to an `eprintln!` that nobody could read, so the
    /// once-per-user budget was spent on a notice that was never shown.
    #[tokio::test]
    async fn the_marker_is_written_only_after_the_notice_is_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("nested").join(".shown");
        let b = AnnouncingWebBackend::new(
            ok_backend(),
            vec![notice("disclosure", Some(marker.clone()))],
        );
        assert!(!marker.exists(), "control: nothing written at construction");
        let WebOutcome::Ok { payload } = b.search("q", 3).await else {
            panic!("expected Ok");
        };
        assert!(payload.get("notice").is_some(), "control: it was delivered");
        assert!(marker.exists(), "delivery must be recorded");
    }

    #[tokio::test]
    async fn no_notices_means_no_wrapper_and_no_payload_change() {
        let inner = ok_backend();
        let wrapped = AnnouncingWebBackend::wrap(inner, Vec::new());
        let WebOutcome::Ok { payload } = wrapped.search("q", 3).await else {
            panic!("expected Ok");
        };
        assert!(payload.get("notice").is_none());
        assert_eq!(wrapped.backend_id(), "capturing");
    }
}
