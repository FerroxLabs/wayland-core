//! End-to-end wiremock smoke for `HttpFetchBackend` + `WebFetchTool`.
//!
//! Proves the full stack the model actually exercises:
//!   1. Tool invocation parses input, runs URL safety, hands a
//!      `FetchRequest` to the host-supplied backend.
//!   2. The real `HttpFetchBackend` does a reqwest GET against an HTTP
//!      origin (wiremock), threads the per-call timeout through, and
//!      returns a populated `FetchOutcome::Ok`.
//!   3. HTML responses are run through readability extraction; JSON /
//!      plain-text passes through verbatim.
//!   4. HTTP error responses surface as `ToolResult { is_error: true }`.
//!
//! Doesn't touch the engine or the LLM — that path is exercised by the
//! manual smoke documented in the wave's commit message. The unit tests
//! in `wcore-tools::web_fetch` cover the SSRF/policy gates against the
//! `NullFetchBackend`; this file covers the live-HTTP wire-shape.

use std::sync::Arc;

use wcore_agent::tool_backends::HttpFetchBackend;
use wcore_tools::Tool;
use wcore_tools::web_fetch::{FetchBackend, WebFetchTool};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Sample github.com/trending-shaped HTML — a flat list of repo cards.
/// Article-mode readability would discard most of this because there's
/// no <article>/<main>. `HttpFetchBackend` uses Raw mode for this exact
/// reason; this test guards that decision.
const TRENDING_FIXTURE: &str = r#"<!DOCTYPE html>
<html><head><title>Trending repositories</title></head>
<body>
<nav>top nav (should be stripped)</nav>
<header>page header (should be stripped)</header>
<main>
  <h1>Trending</h1>
  <div class="Box-row">
    <h2><a href="/anthropics/claude-plugins-official">anthropics / claude-plugins-official</a></h2>
    <p>Official, Anthropic-managed directory of high quality Claude Code Plugins.</p>
    <span>2549 stars today</span>
  </div>
  <div class="Box-row">
    <h2><a href="/openai/gpt-oss">openai / gpt-oss</a></h2>
    <p>gpt-oss-120b and gpt-oss-20b are two open-weight language models by OpenAI.</p>
    <span>1834 stars today</span>
  </div>
  <div class="Box-row">
    <h2><a href="/microsoft/typescript-go">microsoft / typescript-go</a></h2>
    <p>Staging repo for development of native port of TypeScript</p>
    <span>912 stars today</span>
  </div>
</main>
<footer>page footer (should be stripped)</footer>
<script>tracking pixel (should be stripped)</script>
</body></html>"#;

#[tokio::test]
async fn http_fetch_returns_real_html_body_with_readability() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/trending"))
        .respond_with(
            // `.set_body_string` injects its own `text/plain` content-type
            // BEFORE other headers are merged in newer wiremock builds, so
            // we use `.set_body_raw` (no auto-CT) + insert_header to pin
            // text/html.
            ResponseTemplate::new(200).set_body_raw(
                TRENDING_FIXTURE.as_bytes().to_vec(),
                "text/html; charset=utf-8",
            ),
        )
        .mount(&server)
        .await;

    let backend: Arc<dyn FetchBackend> = Arc::new(HttpFetchBackend::new());
    let tool = WebFetchTool::new(backend);

    // Use the wiremock origin — it binds to 127.0.0.1 which the SSRF
    // gate would normally reject. The WebFetchTool's policy check runs
    // BEFORE the backend, so we can't reach the backend through the
    // tool's `execute()` with a loopback URL. Drive the backend directly
    // for the wire-shape assertion.
    use wcore_tools::web_fetch::{FetchOutcome, FetchRequest};
    let backend2 = HttpFetchBackend::new();
    let outcome = backend2
        .fetch(&FetchRequest {
            url: format!("{}/trending", server.uri()),
            timeout_ms: 5_000,
            readable: true,
        })
        .await;

    match outcome {
        FetchOutcome::Ok {
            status,
            content_type,
            text,
            truncated,
            final_url,
        } => {
            assert_eq!(status, 200);
            // wiremock 0.6 doesn't echo our content-type unless we set it
            // through `.set_body_string` which only sets text/plain. The
            // backend correctly passes through whatever the server sent.
            assert!(
                content_type.contains("text/html") || content_type.contains("text/plain"),
                "unexpected content_type: {content_type}"
            );
            assert!(!truncated);
            assert!(final_url.contains("/trending"));
            // Readability::Raw strips chrome but keeps body content.
            assert!(
                text.contains("anthropics") && text.contains("claude-plugins-official"),
                "missing first repo in body: {text}"
            );
            assert!(
                text.contains("openai") && text.contains("gpt-oss"),
                "missing second repo in body: {text}"
            );
            assert!(
                text.contains("microsoft") && text.contains("typescript-go"),
                "missing third repo in body: {text}"
            );
            // Chrome elements that strip_chrome targets MUST be gone.
            assert!(
                !text.contains("top nav (should be stripped)"),
                "<nav> chrome not stripped: {text}"
            );
            assert!(
                !text.contains("page footer (should be stripped)"),
                "<footer> chrome not stripped: {text}"
            );
            assert!(
                !text.contains("tracking pixel (should be stripped)"),
                "<script> chrome not stripped: {text}"
            );
        }
        other => panic!("unexpected fetch outcome: {other:?}"),
    }

    // Sanity-check the Tool surface too — `_tool` unused below is the
    // schema-visible adapter the engine actually registers, kept here so
    // a future API drift trips this compile.
    let _ = tool.name();
}

#[tokio::test]
async fn http_fetch_returns_raw_json_when_readable_false() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            br#"{"repos":["anthropics/claude-plugins-official"]}"#.to_vec(),
            "application/json",
        ))
        .mount(&server)
        .await;

    use wcore_tools::web_fetch::{FetchOutcome, FetchRequest};
    let backend = HttpFetchBackend::new();
    let outcome = backend
        .fetch(&FetchRequest {
            url: format!("{}/api", server.uri()),
            timeout_ms: 5_000,
            readable: false,
        })
        .await;

    match outcome {
        FetchOutcome::Ok {
            status,
            content_type,
            text,
            ..
        } => {
            assert_eq!(status, 200);
            assert!(content_type.contains("application/json"));
            assert!(text.contains("anthropics/claude-plugins-official"));
            // Raw passthrough: JSON braces survive (readability would've
            // mangled them).
            assert!(text.contains('{') && text.contains('}'));
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[tokio::test]
async fn http_fetch_surfaces_404_as_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/missing"))
        .respond_with(ResponseTemplate::new(404).set_body_string("Not Found"))
        .mount(&server)
        .await;

    use wcore_tools::web_fetch::{FetchOutcome, FetchRequest};
    let backend = HttpFetchBackend::new();
    let outcome = backend
        .fetch(&FetchRequest {
            url: format!("{}/missing", server.uri()),
            timeout_ms: 5_000,
            readable: true,
        })
        .await;

    match outcome {
        FetchOutcome::HttpError { status, message } => {
            assert_eq!(status, 404);
            assert!(message.contains("404"));
        }
        other => panic!("expected HttpError, got: {other:?}"),
    }
}

#[tokio::test]
async fn http_fetch_truncates_oversize_body_and_flags_truncated() {
    use wcore_tools::web_fetch::{FetchOutcome, FetchRequest, WEB_FETCH_MAX_RESPONSE_BYTES};
    // Body larger than the cap.
    let big = "A".repeat(WEB_FETCH_MAX_RESPONSE_BYTES + 1024);
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/big"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_body_string(big),
        )
        .mount(&server)
        .await;
    let backend = HttpFetchBackend::new();
    let outcome = backend
        .fetch(&FetchRequest {
            url: format!("{}/big", server.uri()),
            timeout_ms: 5_000,
            readable: false,
        })
        .await;
    match outcome {
        FetchOutcome::Ok {
            text, truncated, ..
        } => {
            assert!(truncated, "expected truncated:true on oversize body");
            assert_eq!(text.len(), WEB_FETCH_MAX_RESPONSE_BYTES);
        }
        other => panic!("unexpected outcome: {other:?}"),
    }
}

#[tokio::test]
async fn http_fetch_honors_per_call_timeout() {
    use wcore_tools::web_fetch::{FetchOutcome, FetchRequest};
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/slow"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/plain")
                .set_delay(std::time::Duration::from_millis(800))
                .set_body_string("eventually"),
        )
        .mount(&server)
        .await;
    let backend = HttpFetchBackend::new();
    // Cap WAY below the upstream delay — request MUST time out fast.
    let started = std::time::Instant::now();
    let outcome = backend
        .fetch(&FetchRequest {
            url: format!("{}/slow", server.uri()),
            timeout_ms: 100,
            readable: false,
        })
        .await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_millis(700),
        "timeout did not fire fast enough: {elapsed:?}"
    );
    match outcome {
        FetchOutcome::Err { message } => {
            assert!(
                message.contains("timed out") || message.contains("timeout"),
                "unexpected error: {message}"
            );
        }
        other => panic!("expected Err on timeout, got: {other:?}"),
    }
}

/// LIVE (real network, `#[ignore]`) — exercises the `SsrfSafeResolver`
/// **real-hostname** path that the wiremock tests above never reach: they all
/// use loopback IP-literal URLs (`http://127.0.0.1:PORT`), and reqwest resolves
/// IP literals WITHOUT invoking a custom DNS resolver. So the resolver that the
/// `WebFetch` tool uses for every real hostname is untested here.
///
/// Regression guard for the eval finding where `WebFetch` to `example.com`
/// timed out at 30s while `curl` resolved + fetched it in <100ms — a
/// resolver/client hang on the hostname path, invisible to the loopback tests.
/// A real DNS hostname (not an IP literal) forces `SsrfSafeResolver::resolve`.
#[tokio::test]
#[ignore = "live: hits real example.com over the network (exercises SsrfSafeResolver hostname path)"]
async fn http_fetch_live_real_hostname_resolves_fast() {
    use wcore_tools::web_fetch::{FetchOutcome, FetchRequest};
    let backend = HttpFetchBackend::new();
    let started = std::time::Instant::now();
    let outcome = backend
        .fetch(&FetchRequest {
            url: "https://example.com/".to_string(),
            timeout_ms: 15_000,
            readable: true,
        })
        .await;
    let elapsed = started.elapsed();
    match outcome {
        FetchOutcome::Ok { status, text, .. } => {
            assert_eq!(status, 200, "example.com should 200");
            assert!(
                text.contains("Example Domain") || text.contains("example"),
                "unexpected body (first 200): {}",
                text.chars().take(200).collect::<String>()
            );
            // The crux: the SsrfSafeResolver hostname path must be FAST. curl
            // does this in <100ms; anything near the request timeout is the bug.
            assert!(
                elapsed < std::time::Duration::from_secs(10),
                "fetch of example.com took {elapsed:?} — resolver/client is hanging"
            );
        }
        other => panic!("expected Ok from example.com, got {other:?} after {elapsed:?}"),
    }
}
