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
use wcore_providers::retry::capture_provider_attempts;
use wcore_types::llm::LlmRequest;
use wcore_types::message::{ContentBlock, Message, Role};

fn make_request() -> LlmRequest {
    LlmRequest {
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

/// A port nothing is listening on: every connect is refused.
fn dead_port_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
    let addr = listener.local_addr().expect("local addr");
    drop(listener);
    format!("http://{addr}")
}

fn physical_attempts(evidence: &[wcore_providers::retry::ProviderAttemptEvidence]) -> usize {
    evidence.iter().filter(|e| e.physical).count()
}

/// RED before the fix: the reset is classified `ProviderError::Http`, which is
/// not retryable, so the provider makes exactly one attempt and the job dies.
#[tokio::test]
async fn connection_reset_mid_request_is_retried() {
    let connections = Arc::new(AtomicUsize::new(0));
    let base_url = spawn_resetting_provider(Arc::clone(&connections));
    let provider = provider(&base_url);

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
    assert_eq!(
        sockets, 7,
        "the reset must be ridden out for BROKEN_CONNECTION_MAX_RETRIES: expected \
         7 attempts (1 initial + 6 retries), the server saw {sockets}"
    );
}

/// Control: a host that cannot be reached at all is NOT the measured case and
/// keeps `DEFAULT_MAX_RETRIES`, so a configured fallback is still tried within
/// a second or so rather than after the long broken-connection window.
#[tokio::test]
async fn connect_refused_keeps_the_default_ceiling() {
    let provider = provider(&dead_port_url());

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
    assert!(
        elapsed < Duration::from_secs(5),
        "failover must not wait out the broken-connection window on a dead host \
         (took {elapsed:?})"
    );
}
