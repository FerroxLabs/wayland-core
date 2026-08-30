//! B-2 — a provider that destroys the connection mid-request must be retried.
//!
//! Measured by the job corpus (row B-2, case `fault-reset`): the fixture's
//! fault proxy accepts the request and then kills the socket with a TCP RST.
//! The product sent exactly ONE request, surfaced
//! `Provider error: HTTP error: error sending request`, and exited 1 with the
//! month-end report unwritten. A reset is the most ordinary transient failure
//! a network path produces; it must cost a retry, not the job.
//!
//! The second test is the control that keeps the first one honest: a host that
//! cannot be reached at all keeps the short ceiling, so a provider chain with a
//! fallback still fails over promptly instead of waiting out the long window.

use std::io::Read;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::openai::OpenAIProvider;
use wcore_providers::retry::{
    BROKEN_CONNECTION_RETRY_WINDOW, DEFAULT_MAX_RETRIES, capture_provider_attempts,
};
use wcore_types::llm::LlmRequest;
use wcore_types::message::{ContentBlock, Message, Role};

fn make_request() -> LlmRequest {
    LlmRequest {
        flux_loop_intent: None,
        flux_turn_nonce: None,
        model: "gpt-4o".to_string(),
        system: "You are a test assistant.".to_string(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "Hello".to_string(),
            }],
        )],
        tools: vec![],
        max_tokens: 512,
        thinking: None,
        reasoning_effort: None,
        cache_tier: None,
        routing_hint: None,
        stop_sequences: Vec::new(),
        web_search: false,
        conversation_id: None,
        client_context_tokens: None,
        temperature: None,
        omit_max_tokens: false,
        routed_model_hint: None,
        replay_reasoning_content: false,
    }
}

fn provider(base_url: &str) -> OpenAIProvider {
    OpenAIProvider::new(
        "test-key",
        base_url,
        ProviderCompat::openai_defaults(),
        DebugConfig::default(),
    )
}

/// Accept every connection, consume the head of the request, then close the
/// socket with the tail still unread so the kernel answers with RST instead of
/// an orderly FIN. This is the wire behaviour the B-2 fault proxy produces.
fn spawn_resetting_provider(connections: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            connections.fetch_add(1, Ordering::SeqCst);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(200)));
            let mut head = [0u8; 16];
            let mut handle = &stream;
            let _ = handle.read(&mut head);
            drop(stream);
        }
    });
    format!("http://{addr}")
}

/// Accept every connection, read the whole request, hang briefly, then close
/// the socket cleanly with no response at all. This is the `fault-timeout`
/// shape from the B-2 fixture: no RST, no io error — hyper reports
/// "connection closed before message completed", which carries no
/// `io::Error` cause and so is invisible to any classifier that walks the
/// source chain looking for one.
fn spawn_hanging_provider(connections: Arc<AtomicUsize>) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            connections.fetch_add(1, Ordering::SeqCst);
            let _ = stream.set_read_timeout(Some(Duration::from_millis(150)));
            // Drain the request so nothing is left unread and the close is an
            // orderly FIN rather than a reset.
            let mut sink = [0u8; 8192];
            let mut handle = &stream;
            while let Ok(n) = handle.read(&mut sink) {
                if n == 0 {
                    break;
                }
            }
            drop(stream);
        }
    });
    format!("http://{addr}")
}

/// A port nothing is listening on: every connect is refused.
///
/// The guard is returned with the URL and must be kept alive by the caller.
/// Binding and DROPPING a listener does not give a dead port on a busy host —
/// it gives a port that was dead a moment ago, which another process may
/// already be listening on by the time the connect happens.
fn dead_port_url() -> (wcore_egress::refused_port::RefusedPort, String) {
    let refused = wcore_egress::refused_port::RefusedPort::reserve().expect("bind loopback");
    let url = format!("http://{}", refused.addr());
    (refused, url)
}

fn physical_attempts(evidence: &[wcore_providers::retry::ProviderAttemptEvidence]) -> usize {
    evidence.iter().filter(|e| e.physical).count()
}

/// How many sends a peer that fails instantly can see inside
/// `BROKEN_CONNECTION_RETRY_WINDOW`, walking the backoff schedule the ring
/// actually runs. Derived from the constants rather than written down, so the
/// test follows the window instead of pinning a literal nobody can re-derive
/// — and so that changing the window or the curve cannot leave a green test
/// asserting the old shape.
///
/// It calls `wcore_providers::backoff::base_backoff`, the same function the
/// ring calls, rather than restating the schedule. The restated form this
/// replaces (250 ms, then x4 capped at 4 s) would have survived the move to
/// the shared curve as a green predictor of ~10 sends for a ring that now
/// makes 7 — the exact failure the paragraph above claims to prevent.
fn sends_within_the_window() -> usize {
    let mut elapsed = Duration::ZERO;
    let mut sends = 1usize;
    while elapsed < BROKEN_CONNECTION_RETRY_WINDOW {
        elapsed += wcore_providers::backoff::base_backoff(sends as u32);
        sends += 1;
    }
    sends
}

/// Assert a window-bounded retry: strictly more than the short default
/// ceiling, no more than the schedule allows, and stopped by the clock.
///
/// The lower bound carries slack because every real attempt costs a few
/// milliseconds of its own; enough of them can push the last admission past
/// the deadline. The point being pinned is that the bound is the WINDOW —
/// exactness would be pinning the arithmetic instead.
fn assert_window_bounded(sends: usize, elapsed: Duration, shape: &str) {
    let ceiling = sends_within_the_window();
    assert!(
        sends > DEFAULT_MAX_RETRIES as usize + 1,
        "{shape}: a destroyed socket must outlast the default ceiling \
         ({} sends), saw {sends}",
        DEFAULT_MAX_RETRIES + 1
    );
    assert!(
        sends <= ceiling && sends + 2 >= ceiling,
        "{shape}: expected about {ceiling} sends inside \
         {BROKEN_CONNECTION_RETRY_WINDOW:?}, saw {sends}"
    );
    assert!(
        elapsed >= BROKEN_CONNECTION_RETRY_WINDOW,
        "{shape}: gave up after {elapsed:?}, inside the window"
    );
    assert!(
        elapsed <= BROKEN_CONNECTION_RETRY_WINDOW + Duration::from_secs(10),
        "{shape}: ran {elapsed:?}, past the window plus one backoff — the \
         deadline is not what stopped it"
    );
}

/// RED before the fix: the reset is classified `ProviderError::Http`, which is
/// not retryable, so the provider makes exactly one attempt and the job dies.
#[tokio::test]
async fn connection_reset_mid_request_is_retried() {
    let connections = Arc::new(AtomicUsize::new(0));
    let base_url = spawn_resetting_provider(Arc::clone(&connections));
    let provider = provider(&base_url);

    let started = std::time::Instant::now();
    let (result, evidence) =
        capture_provider_attempts(async { provider.stream(&make_request()).await }).await;
    let err = result.expect_err("a provider that resets every connection cannot succeed");

    assert!(
        err.is_retryable(),
        "a mid-request connection reset is transient and must be retryable, got: {err:?}"
    );
    let sockets = connections.load(Ordering::SeqCst);
    assert_eq!(
        sockets,
        physical_attempts(&evidence),
        "every physical attempt must be one socket the server actually saw"
    );
    assert_window_bounded(sockets, started.elapsed(), "reset");
}

/// Job corpus row B-2 `fault-timeout`: the provider hangs and then closes the
/// connection with no response. Measured on the sealed binary as the identical
/// `HTTP error: error sending request` the reset produced, with the identical
/// consequence — one attempt, then the job died.
#[tokio::test]
async fn orderly_close_before_a_response_is_retried() {
    let connections = Arc::new(AtomicUsize::new(0));
    let base_url = spawn_hanging_provider(Arc::clone(&connections));
    let provider = provider(&base_url);

    let started = std::time::Instant::now();
    let (result, evidence) =
        capture_provider_attempts(async { provider.stream(&make_request()).await }).await;
    let err = result.expect_err("a provider that answers nothing cannot succeed");

    assert!(
        err.is_retryable(),
        "a connection closed before the response is transient, got: {err:?}"
    );
    assert_eq!(
        connections.load(Ordering::SeqCst),
        physical_attempts(&evidence),
        "every physical attempt must be one socket the server actually saw"
    );
    assert_window_bounded(
        physical_attempts(&evidence),
        started.elapsed(),
        "orderly close",
    );
}

/// The premise the old classifier rested on, checked against the linked
/// reqwest rather than assumed: a malformed URL and a malformed header value
/// are BUILDER errors, so `is_request()` never fires for them and using it as
/// the transport signal cannot retry a permanent client-side mistake.
// A bare reqwest client is the point of this test: it asserts how reqwest
// itself classifies two malformed requests. Nothing is dispatched (both fail
// in the builder), so the egress chokepoint has nothing to police here.
#[allow(clippy::disallowed_methods)]
#[tokio::test]
async fn client_side_mistakes_are_builder_errors_not_request_errors() {
    let client = reqwest::Client::new();

    let bad_url = client
        .post("not a url")
        .send()
        .await
        .expect_err("a relative URL cannot be sent");
    assert!(bad_url.is_builder(), "bad URL must be a builder error");
    assert!(
        !bad_url.is_request(),
        "bad URL must not look like transport"
    );

    let bad_header = client
        .post("http://127.0.0.1:1/")
        .header("x-test", "bad\nvalue")
        .send()
        .await
        .expect_err("an invalid header value cannot be sent");
    assert!(
        bad_header.is_builder(),
        "invalid header must be a builder error"
    );
    assert!(
        !bad_header.is_request(),
        "invalid header must not look like transport"
    );
}

/// Control: a host that cannot be reached at all is NOT the measured case and
/// keeps `DEFAULT_MAX_RETRIES`, so a configured fallback is still tried within
/// a second or so rather than after the long broken-connection window.
#[tokio::test]
async fn connect_refused_keeps_the_default_ceiling() {
    // `_refused` is bound, not `_`: dropping the guard would free the port and
    // reopen the race this helper exists to close.
    let (_refused, url) = dead_port_url();
    let provider = provider(&url);

    let started = std::time::Instant::now();
    let (result, evidence) =
        capture_provider_attempts(async { provider.stream(&make_request()).await }).await;
    let elapsed = started.elapsed();

    assert!(
        result.is_err(),
        "a refused connection cannot produce a stream"
    );
    assert_eq!(
        physical_attempts(&evidence),
        3,
        "connect failures keep the 3-attempt ceiling"
    );
    // Bounded against the WINDOW, not against a hand-picked stopwatch value.
    // The claim is "a dead host does not get the long window", and the only
    // number that can express it is the window itself. A literal 5 s failed on
    // Windows, where three refused connects legitimately cost 7.4 s (measured,
    // SeanDesktop, 2026-08-11) — the ceiling was right there, the assertion was
    // not.
    assert!(
        elapsed < BROKEN_CONNECTION_RETRY_WINDOW / 2,
        "failover must not wait out the broken-connection window \
         ({BROKEN_CONNECTION_RETRY_WINDOW:?}) on a dead host (took {elapsed:?})"
    );
}
