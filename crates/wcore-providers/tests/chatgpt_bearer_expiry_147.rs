//! #147 instrument — where `API error 404: {"detail":"Not Found"}` on a long
//! ChatGPT/Codex turn can and cannot come from, and what an OAuth bearer that
//! dies while the SSE body is still arriving actually does.
//!
//! Three prior triages of #147 asserted "an OAuth token expiring during a long
//! stream" without ever measuring it. These arms measure it against a mock
//! Codex backend whose token expiry the test controls:
//!
//! * the reported string is produced ONLY by the pre-stream status check in
//!   `OpenAIChatGptProvider::stream`; an in-band SSE failure frame renders as
//!   a bare `LlmEvent::Error`, so a genuinely mid-stream failure can never
//!   look like the report;
//! * a bearer that expires while the accepted stream is still trickling does
//!   not disturb that stream — HTTP authenticates once, at request receipt;
//! * the bearer is resolved exactly once per `stream()` and never again, so
//!   there is no mid-stream refresh seam to fix;
//! * a 401 at the front door is a login nudge, not a 404.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use wcore_config::compat::ProviderCompat;
use wcore_config::debug::DebugConfig;
use wcore_providers::openai_chatgpt::{AsyncBearerSource, BearerCreds};
use wcore_providers::{LlmProvider, OpenAIChatGptProvider, ProviderError};
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, Message, Role};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The exact body the reporter saw behind the 404.
const NOT_FOUND_BODY: &str = r#"{"detail":"Not Found"}"#;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A 3-segment JWT carrying `exp` plus the Codex account claim, so the mock
/// backend can authenticate it exactly as a real server would.
fn jwt_expiring_at(exp: u64) -> String {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let payload = serde_json::json!({
        "exp": exp,
        "https://api.openai.com/auth": { "chatgpt_account_id": "acct_147" }
    });
    let seg = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload).unwrap());
    format!("hdr.{seg}.sig")
}

fn exp_of(jwt: &str) -> Option<u64> {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let seg = jwt.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(seg).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("exp").and_then(|x| x.as_u64())
}

/// A bearer source that hands out `token` and counts how many times the
/// provider asked for it.
fn counting_bearer(token: &str, calls: Arc<AtomicUsize>) -> AsyncBearerSource {
    let creds = BearerCreds {
        access_token: token.to_string(),
        account_id: "acct_147".to_string(),
    };
    Arc::new(move || {
        let creds = creds.clone();
        let calls = calls.clone();
        Box::pin(async move {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(creds)
        })
    })
}

fn make_request() -> LlmRequest {
    LlmRequest {
        model: "gpt-5.2-codex".to_string(),
        system: "You are a test assistant.".to_string(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "a very long prompt".to_string(),
            }],
        )],
        max_tokens: 512,
        ..Default::default()
    }
}

async fn collect_events(mut rx: tokio::sync::mpsc::Receiver<LlmEvent>) -> Vec<LlmEvent> {
    let mut events = Vec::new();
    while let Some(event) = rx.recv().await {
        events.push(event);
    }
    events
}

#[derive(Default)]
struct MockLog {
    requests: AtomicUsize,
    refused_expired: AtomicUsize,
}

fn header_value<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.trim().eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// A raw-TCP stand-in for `chatgpt.com/backend-api/codex`.
///
/// It authenticates the presented bearer AT REQUEST RECEIPT — the one moment a
/// real HTTP server checks it — and, on acceptance, trickles `ticks` SSE
/// deltas `gap` apart before the terminal frame, so the accepted stream can be
/// made to outlive the token that opened it. An expired bearer is refused the
/// way the reporter's backend refused: 404 with `{"detail":"Not Found"}`.
async fn spawn_codex_mock(gap: Duration, ticks: usize) -> (String, Arc<MockLog>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let log = Arc::new(MockLog::default());
    let log_task = log.clone();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            let log = log_task.clone();
            tokio::spawn(async move {
                let mut buf: Vec<u8> = Vec::new();
                let mut tmp = [0u8; 8192];
                let mut head_end = None;
                // Read the full request (headers + declared body) so the
                // client's write side always completes.
                loop {
                    let n = match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    if head_end.is_none() {
                        head_end = buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4);
                    }
                    if let Some(he) = head_end {
                        let head = String::from_utf8_lossy(&buf[..he]).to_string();
                        let want: usize = header_value(&head, "content-length")
                            .and_then(|v| v.parse().ok())
                            .unwrap_or(0);
                        if buf.len() >= he + want {
                            break;
                        }
                    }
                }
                let Some(he) = head_end else { return };
                log.requests.fetch_add(1, Ordering::SeqCst);
                let head = String::from_utf8_lossy(&buf[..he]).to_string();
                let exp = header_value(&head, "authorization")
                    .and_then(|v| v.strip_prefix("Bearer "))
                    .and_then(exp_of);

                let live = exp.is_some_and(|e| e > now_secs());
                if !live {
                    log.refused_expired.fetch_add(1, Ordering::SeqCst);
                    let resp = format!(
                        "HTTP/1.1 404 Not Found\r\ncontent-type: application/json\r\n\
                         content-length: {}\r\nconnection: close\r\n\r\n{NOT_FOUND_BODY}",
                        NOT_FOUND_BODY.len()
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                    let _ = sock.flush().await;
                    let _ = sock.shutdown().await;
                    return;
                }

                // Accepted. Body length is delimited by connection close.
                let _ = sock
                    .write_all(
                        b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                          connection: close\r\n\r\n",
                    )
                    .await;
                let _ = sock.flush().await;
                for i in 0..ticks {
                    let frame = format!(
                        "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"tick{i}\"}}\n\n"
                    );
                    if sock.write_all(frame.as_bytes()).await.is_err() {
                        return;
                    }
                    let _ = sock.flush().await;
                    tokio::time::sleep(gap).await;
                }
                let completed = "data: {\"type\":\"response.completed\",\"response\":\
                                 {\"id\":\"resp_147\",\"status\":\"completed\",\
                                 \"usage\":{\"input_tokens\":1,\"output_tokens\":1}}}\n\n";
                let _ = sock.write_all(completed.as_bytes()).await;
                let _ = sock.flush().await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (format!("http://{addr}"), log)
}

fn provider_for(base_url: &str, bearer: AsyncBearerSource) -> OpenAIChatGptProvider {
    OpenAIChatGptProvider::new(bearer, ProviderCompat::default(), DebugConfig::default())
        .with_base_url(base_url)
}

/// MEASUREMENT (#147, claim 2 and 4). A bearer that is live when the request
/// is received but DEAD long before the stream finishes does not disturb that
/// stream: the backend authenticated once, at receipt, and the accepted
/// response runs to `response.completed`. Token expiry cannot break a stream
/// that is already open.
///
/// This arm is also the file's must-pass control — it fails if the mock, the
/// trickle, or the provider wiring is broken.
#[tokio::test]
async fn a_bearer_that_dies_midstream_does_not_disturb_the_accepted_stream() {
    // 4 ticks 700ms apart: the stream is still arriving ~2.8s after open,
    // well past the token's 1s of remaining life.
    let (base, log) = spawn_codex_mock(Duration::from_millis(700), 4).await;
    let token = jwt_expiring_at(now_secs() + 1);
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = provider_for(&base, counting_bearer(&token, calls.clone()));

    let opened = std::time::Instant::now();
    let rx = provider
        .stream(&make_request())
        .await
        .expect("stream opens");
    let events = collect_events(rx).await;
    let elapsed = opened.elapsed();

    assert!(
        elapsed >= Duration::from_secs(2),
        "stream must outlive the token by design; took {elapsed:?}"
    );
    assert_eq!(log.refused_expired.load(Ordering::SeqCst), 0);
    assert_eq!(log.requests.load(Ordering::SeqCst), 1);
    assert!(
        !events.iter().any(|e| matches!(e, LlmEvent::Error(_))),
        "an expired-mid-flight token must not produce an error: {events:?}"
    );
    assert!(
        matches!(events.last(), Some(LlmEvent::Done { .. })),
        "events: {events:?}"
    );
    // Claim 4: nothing re-resolves the bearer once the stream is open.
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "the bearer is resolved once per stream() and never during it"
    );
}

/// RED ARM. The same mock, the same code path, a bearer that is already dead
/// when the request is RECEIVED: the backend refuses it, and the engine
/// surfaces the reporter's string verbatim. Without this arm the test above
/// would pass against a mock that never refuses anything.
#[tokio::test]
async fn a_bearer_dead_at_receipt_renders_the_exact_reported_404() {
    let (base, log) = spawn_codex_mock(Duration::from_millis(10), 1).await;
    let token = jwt_expiring_at(now_secs().saturating_sub(1));
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = provider_for(&base, counting_bearer(&token, calls.clone()));

    let err = provider
        .stream(&make_request())
        .await
        .expect_err("dead bearer must be refused");

    assert_eq!(log.refused_expired.load(Ordering::SeqCst), 1);
    match &err {
        ProviderError::Api { status, message } => {
            assert_eq!(*status, 404);
            assert!(message.starts_with(NOT_FOUND_BODY), "message: {message}");
        }
        other => panic!("expected Api{{404}}, got {other:?}"),
    }
    // The reporter's text still leads, character for character, so a user
    // reporting this error is still matchable against issue #147.
    let rendered = err.to_string();
    assert!(
        rendered.starts_with(r#"API error 404: {"detail":"Not Found"}"#),
        "the reported string must survive as the prefix: {rendered}"
    );
    // ...and it is now attributed. Only provable facts: which endpoint
    // answered, that it was refused there, that it is not retried.
    assert!(rendered.contains("ChatGPT Codex backend"), "{rendered}");
    assert!(rendered.contains("/responses"), "{rendered}");
    assert!(rendered.contains("does not retry"), "{rendered}");
    assert!(
        rendered.contains("wayland auth login chatgpt"),
        "{rendered}"
    );
    // No cause is asserted. These are the words a diagnosis would need.
    for forbidden in ["expired", "token expiry", "your session", "because"] {
        assert!(
            !rendered.contains(forbidden),
            "the message must not assert a cause ({forbidden:?}): {rendered}"
        );
    }
    // A 404 is not retried: exactly one physical request reached the backend.
    assert_eq!(
        log.requests.load(Ordering::SeqCst),
        1,
        "404 must not be retried"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

/// MEASUREMENT (#147, claim 2). A failure that is genuinely mid-stream — an
/// in-band `response.failed` frame after real deltas — surfaces as a bare
/// `LlmEvent::Error` carrying the upstream message. It carries no status and
/// can never render as `API error 404: …`. The reported string therefore
/// pins the failure to the pre-stream status check, i.e. to a request that
/// was refused at its front door, not to a stream that died in flight.
#[tokio::test]
async fn a_midstream_failure_cannot_render_as_the_reported_404() {
    let server = MockServer::start().await;
    let body = concat!(
        "data: {\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}\n\n",
        "data: {\"type\":\"response.failed\",\"response\":{\"error\":\
         {\"code\":\"server_error\",\"message\":\"Not Found\"}}}\n\n",
    );
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let calls = Arc::new(AtomicUsize::new(0));
    let provider = provider_for(
        &server.uri(),
        counting_bearer(&jwt_expiring_at(now_secs() + 3600), calls.clone()),
    );
    let events = collect_events(provider.stream(&make_request()).await.expect("opens")).await;

    let errors: Vec<&String> = events
        .iter()
        .filter_map(|e| match e {
            LlmEvent::Error(m) => Some(m),
            _ => None,
        })
        .collect();
    assert_eq!(errors.len(), 1, "events: {events:?}");
    // Positive control on the assertion below: the message IS carried through.
    assert!(errors[0].contains("Not Found"), "message: {}", errors[0]);
    assert!(
        !errors[0].contains("API error"),
        "a mid-stream failure must not render with an HTTP status prefix: {}",
        errors[0]
    );
    assert!(
        events.iter().any(|e| matches!(e, LlmEvent::TextDelta(_))),
        "the deltas before the failure must still reach the caller: {events:?}"
    );
}

/// MEASUREMENT (#147, claim 2, 401 leg). A 401 at the front door is mapped to
/// `MissingApiKey` — a re-login nudge — and never renders with a 404. So if
/// the reporter's backend had answered an expired bearer with 401, the report
/// would not have said 404.
#[tokio::test]
async fn a_401_at_the_front_door_is_a_login_nudge_not_a_404() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(401).set_body_string(r#"{"detail":"Unauthorized"}"#))
        .mount(&server)
        .await;

    let calls = Arc::new(AtomicUsize::new(0));
    let provider = provider_for(
        &server.uri(),
        counting_bearer(&jwt_expiring_at(now_secs() + 3600), calls.clone()),
    );
    let err = provider
        .stream(&make_request())
        .await
        .expect_err("401 must fail the turn");

    assert!(matches!(err, ProviderError::MissingApiKey), "err: {err:?}");
    let rendered = err.to_string();
    assert!(
        !rendered.contains("404"),
        "a 401 must never render as a 404: {rendered}"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

/// RED ARM for the 404 attribution. The SAME body at a DIFFERENT status must
/// pass through untouched: the arm is gated on 404, not on the body, and every
/// other status keeps its existing behaviour. Without this, the assertions
/// above would pass on a provider that decorated every error alike.
#[tokio::test]
async fn a_403_with_the_same_body_is_not_attributed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(403).set_body_string(NOT_FOUND_BODY))
        .mount(&server)
        .await;

    let calls = Arc::new(AtomicUsize::new(0));
    let provider = provider_for(
        &server.uri(),
        counting_bearer(&jwt_expiring_at(now_secs() + 3600), calls.clone()),
    );
    let err = provider
        .stream(&make_request())
        .await
        .expect_err("403 must fail the turn");

    match &err {
        ProviderError::Api { status, message } => {
            assert_eq!(*status, 403);
            assert_eq!(
                message, NOT_FOUND_BODY,
                "a non-404 body must pass through unchanged"
            );
        }
        other => panic!("expected Api{{403}}, got {other:?}"),
    }
    assert!(
        !err.to_string().contains("ChatGPT Codex backend"),
        "only 404 is attributed: {err}"
    );
}
