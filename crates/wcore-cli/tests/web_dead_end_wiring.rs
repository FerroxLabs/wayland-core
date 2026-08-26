//! End-to-end: does a degraded / dead-ended web search actually reach the
//! surface the user reads?
//!
//! Both halves of this were already unit-tested and both were still invisible
//! in the product, because nothing tested the SEAM. `ChainedWebBackend`
//! produced a correct `degraded_from` note that no renderer consumed, and the
//! keyless privacy disclosure was written to a terminal the TUI had already
//! taken over. So this drives the real `WebTool` over the real backend chain
//! and feeds its real output to the real TUI formatter — the path a user's
//! query takes — rather than checking each piece in isolation.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_agent::tool_backends::{AnnouncingWebBackend, ChainedWebBackend, WebNotice};
use wcore_cli::tui::theme::Theme;
use wcore_cli::tui::tool_formatters::formatter_for;
use wcore_tools::Tool;
use wcore_tools::web_tools::{
    CapturingWebBackend, CrawlRequest, ExtractRequest, WebBackend, WebOutcome, WebTool,
};

/// A primary that always fails — a wrong key, an expired plan, a dead host.
struct DeadPrimary;

#[async_trait]
impl WebBackend for DeadPrimary {
    async fn search(&self, _q: &str, _l: u32) -> WebOutcome {
        WebOutcome::Err {
            message: "exa returned HTTP 401 (invalid API key)".into(),
        }
    }
    async fn extract(&self, _r: ExtractRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "exa returned HTTP 401 (invalid API key)".into(),
        }
    }
    async fn crawl(&self, _r: CrawlRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "exa returned HTTP 401 (invalid API key)".into(),
        }
    }
    fn backend_id(&self) -> &str {
        "exa"
    }
}

/// A fallback that is itself locked out — the measured DuckDuckGo state after
/// two queries from one IP.
struct BlockedFallback;

#[async_trait]
impl WebBackend for BlockedFallback {
    async fn search(&self, _q: &str, _l: u32) -> WebOutcome {
        WebOutcome::Err {
            message: "duckduckgo refused this query as automated traffic (HTTP 202)".into(),
        }
    }
    async fn extract(&self, _r: ExtractRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "duckduckgo refused this query as automated traffic (HTTP 202)".into(),
        }
    }
    async fn crawl(&self, _r: CrawlRequest) -> WebOutcome {
        WebOutcome::Err {
            message: "duckduckgo refused this query as automated traffic (HTTP 202)".into(),
        }
    }
    fn backend_id(&self) -> &str {
        "duckduckgo"
    }
}

fn ddg_serving() -> Arc<dyn WebBackend> {
    Arc::new(CapturingWebBackend::new().with_search_payload(json!({
        "web": [{ "title": "A result", "url": "https://example.com/a", "snippet": "text" }]
    })))
}

async fn run_search(backend: Arc<dyn WebBackend>) -> wcore_types::tool::ToolResult {
    WebTool::new(backend)
        .execute(json!({ "operation": "search", "query": "rust ownership" }))
        .await
}

fn rendered_card(result: &wcore_types::tool::ToolResult) -> String {
    let payload: Value = serde_json::from_str(&result.content).expect("tool output must be JSON");
    let f = formatter_for("web");
    let theme = Theme::no_color();
    let mut out = f.summary_line(&payload, Duration::ZERO);
    for line in f.detail_lines(&payload, &theme) {
        out.push('\n');
        out.push_str(
            &line
                .spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>(),
        );
    }
    out
}

/// gh#1068. The configured backend was refused and DuckDuckGo answered
/// instead. The user must be able to see that from the card alone — otherwise
/// they debug the backend that actually answered rather than the one that
/// failed, which is exactly what happened to the user whose `EXA_API_KEY` was
/// being ignored.
#[tokio::test]
async fn a_fallback_served_search_names_the_skipped_backend_on_the_card() {
    let chain: Arc<dyn WebBackend> =
        Arc::new(ChainedWebBackend::new(Arc::new(DeadPrimary), ddg_serving()));
    let result = run_search(chain).await;

    assert!(
        !result.is_error,
        "the fallback served results, so this is Ok"
    );
    let card = rendered_card(&result);
    assert!(
        card.contains("exa"),
        "the backend that was skipped must be named: {card}"
    );
    assert!(
        card.contains("HTTP 401"),
        "the reason it was skipped must reach the card, or the user fixes the \
         wrong thing: {card}"
    );
    // The results themselves survive untouched.
    assert!(card.contains("A result"), "{card}");
}

/// Control for the test above: a search the configured backend served must not
/// grow a degradation line. A renderer that annotates unconditionally would
/// pass the assertion above while telling the user something false.
#[tokio::test]
async fn a_normally_served_search_is_not_labelled_degraded() {
    let result = run_search(ddg_serving()).await;
    let card = rendered_card(&result);
    assert!(!card.contains("did not answer"), "{card}");
    assert!(!card.contains("served by"), "{card}");
    assert!(
        card.contains("A result"),
        "control: results rendered: {card}"
    );
}

/// The dead end. Every backend failed, so the error message is the ONLY thing
/// the user gets — it has to carry what was tried, what happened, and the one
/// next step, on the surface they read rather than in a log file.
#[tokio::test]
async fn a_total_failure_names_both_backends_and_one_concrete_next_step() {
    let chain: Arc<dyn WebBackend> = Arc::new(ChainedWebBackend::new(
        Arc::new(DeadPrimary),
        Arc::new(BlockedFallback),
    ));
    let result = run_search(chain).await;
    assert!(result.is_error, "nothing was searched, so this is an error");

    let payload: Value = serde_json::from_str(&result.content).unwrap();
    let msg = payload["error"]
        .as_str()
        .expect("an error string")
        .to_string();

    assert!(msg.contains("exa"), "what was configured: {msg}");
    assert!(msg.contains("HTTP 401"), "why it failed: {msg}");
    assert!(msg.contains("duckduckgo"), "what was tried next: {msg}");
    assert!(msg.contains("HTTP 202"), "why that failed too: {msg}");
    assert!(
        msg.contains("https://app.tavily.com") && msg.contains("TAVILY_API_KEY"),
        "the single next step, named concretely: {msg}"
    );
}

/// The keyless privacy disclosure reaches the user through the tool result,
/// which every mode renders — instead of an `eprintln!` at boot, which in the
/// default TUI mode lands on an alt-screen buffer that is painted over and
/// then discarded.
#[tokio::test]
async fn the_keyless_disclosure_reaches_the_card_of_the_first_search() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join(".parallel-disclosure-shown");
    let backend = AnnouncingWebBackend::wrap(
        ddg_serving(),
        vec![WebNotice {
            text: "web search: your search queries are sent to parallel.ai. \
                   Set WAYLAND_WEB_BACKEND=off to disable."
                .to_string(),
            marker_on_delivery: Some(marker.clone()),
        }],
    );
    assert!(
        !marker.exists(),
        "control: nothing recorded before the search"
    );

    let card = rendered_card(&run_search(Arc::clone(&backend)).await);
    assert!(
        card.contains("parallel.ai"),
        "the destination must be on the card: {card}"
    );
    assert!(
        card.contains("WAYLAND_WEB_BACKEND=off"),
        "so must the way out: {card}"
    );
    assert!(
        marker.exists(),
        "and only a delivered notice may spend the once-per-user budget"
    );

    // Said once, not on every search.
    let second = rendered_card(&run_search(backend).await);
    assert!(!second.contains("parallel.ai"), "{second}");
}
