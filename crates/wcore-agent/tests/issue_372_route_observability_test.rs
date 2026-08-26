//! wayland#372 — RUN-ROUTE OBSERVABILITY on the wire.
//!
//! The report is a Wayland Desktop user who could not tell whether a stalled
//! run was model quality, backend routing, a failed tool call, hidden retries
//! or a stale route, and asked for the run metadata that would separate them:
//! "whether the step is local or cloud", "endpoint URL when local", "retry
//! count".
//!
//! Graded on the WIRE, never on a log line. `emit_info`'s
//! "retrying in 1.0s (attempt 1/2)" already reaches a CLI user, but the
//! desktop host reads `ProtocolEvent`s and cannot parse a prose sentence into
//! a progress indicator; and with `RUST_LOG` unset the `tracing::warn!` in the
//! provider retry ring reaches nobody at all. So every assertion here reads
//! the serialized JSON of the event the host actually decodes, produced by the
//! real `ProtocolSink` over a real emitter.
//!
//! Scope: `wcore-protocol/src/events.rs`, `wcore-providers/src/retry.rs`,
//! `wcore-egress/src/request.rs`, `wcore-agent/src/engine.rs`.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_agent::output::protocol_sink::ProtocolSink;
use wcore_egress::{AllowAllPolicy, EgressClient};
use wcore_protocol::events::ProtocolEvent;
use wcore_protocol::writer::ProtocolEmitter;
use wcore_providers::retry::{builder_send_with_retry, scope_max_retries};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::{physical_attempt_server, test_config};

const USER_MARKER: &str = "route-observability-probe";

// ---------------------------------------------------------------------------
// Provider — every outcome, failing ones included, crosses a REAL local HTTP
// boundary first. That send is the physical provider attempt whose route this
// test is about; a purely in-memory provider records no attempt and no
// endpoint could exist to assert on.
// ---------------------------------------------------------------------------
struct ScriptedProvider {
    script: Mutex<std::collections::VecDeque<Result<Vec<LlmEvent>, ProviderError>>>,
    calls: Arc<AtomicUsize>,
    physical_url: String,
}

#[async_trait]
impl LlmProvider for ScriptedProvider {
    async fn stream(&self, _r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let next = self.script.lock().unwrap().pop_front();
        let client = EgressClient::new().with_policy(Arc::new(AllowAllPolicy));
        let response =
            scope_max_retries(0, builder_send_with_retry(client.get(&self.physical_url))).await?;
        if !response.status().is_success() {
            return Err(ProviderError::Api {
                status: response.status().as_u16(),
                message: "fixture response".into(),
            });
        }
        let events = match next {
            Some(Ok(events)) => events,
            Some(Err(e)) => return Err(e),
            None => end_turn_text("script exhausted"),
        };
        let (tx, rx) = mpsc::channel(64);
        for event in events {
            let _ = tx.send(event).await;
        }
        Ok(rx)
    }
}

fn end_turn_text(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage {
                input_tokens: 10,
                output_tokens: 5,
                cache_creation_tokens: 0,
                cache_read_tokens: 0,
            },
        },
    ]
}

// ---------------------------------------------------------------------------
// The wire.
// ---------------------------------------------------------------------------
#[derive(Default)]
struct WireRecorder(Mutex<Vec<serde_json::Value>>);

impl ProtocolEmitter for WireRecorder {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        self.0
            .lock()
            .unwrap()
            .push(serde_json::to_value(event).expect("every protocol event serializes"));
        Ok(())
    }
}

impl WireRecorder {
    fn of_type(&self, kind: &str) -> Vec<serde_json::Value> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter(|frame| frame.get("type").and_then(|t| t.as_str()) == Some(kind))
            .cloned()
            .collect()
    }
}

struct Harness {
    engine: AgentEngine,
    wire: Arc<WireRecorder>,
    calls: Arc<AtomicUsize>,
    origin: String,
    _root: tempfile::TempDir,
    _server: wiremock::MockServer,
}

async fn harness(script: Vec<Result<Vec<LlmEvent>, ProviderError>>) -> Harness {
    let root = tempfile::tempdir().expect("tempdir");
    let server = physical_attempt_server().await;
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = Arc::new(ScriptedProvider {
        script: Mutex::new(script.into_iter().collect()),
        calls: Arc::clone(&calls),
        physical_url: server.uri(),
    });

    let wire = Arc::new(WireRecorder::default());
    let sink: Arc<dyn OutputSink> = Arc::new(ProtocolSink::with_emitter(
        Arc::clone(&wire) as Arc<dyn ProtocolEmitter>
    ));

    let mut engine =
        AgentEngine::new_with_provider(provider, test_config(), ToolRegistry::new(), sink);
    engine
        .init_session("test-provider", &root.path().to_string_lossy(), None)
        .expect("init_session");

    // `MockServer::uri()` is already `http://127.0.0.1:PORT` with no path.
    let origin = server.uri().trim_end_matches('/').to_string();
    Harness {
        engine,
        wire,
        calls,
        origin,
        _root: root,
        _server: server,
    }
}

// ===========================================================================
// The route of every physical attempt
// ===========================================================================

/// #372: "selected model route for each step ... whether the step is local or
/// cloud ... endpoint URL when local".
///
/// The host is told a physical attempt happened (`provider_attempt`) and, on
/// failure, which class it failed with — but nothing about WHERE it went. A
/// user whose local Ollama route is stalling and whose cloud route is not
/// cannot see, from anything Core emits, which of the two a given step used.
#[tokio::test]
async fn a_physical_attempt_names_its_endpoint_and_whether_it_is_local() {
    let h = harness(vec![Ok(end_turn_text("done"))]).await;
    let mut engine = h.engine;
    engine
        .run(USER_MARKER, "")
        .await
        .expect("the run must succeed");

    let attempts = h.wire.of_type("provider_attempt");

    // CONTROL. No physical attempt on the wire means the harness never
    // dispatched and every claim below is vacuous.
    assert!(
        !attempts.is_empty(),
        "control failed: no provider_attempt frame reached the wire after \
         {} provider call(s), so this test graded nothing",
        h.calls.load(Ordering::SeqCst)
    );

    let routed = attempts
        .iter()
        .find(|frame| frame.get("endpoint").is_some())
        .unwrap_or_else(|| {
            panic!(
                "wayland#372: no provider_attempt frame carries an `endpoint`. The host \
                 is told an attempt happened and cannot tell which route it used. \
                 Frames were: {attempts:#?}"
            )
        });

    assert_eq!(
        routed.get("endpoint").and_then(|v| v.as_str()),
        Some(h.origin.as_str()),
        "the endpoint must be the origin actually dispatched to"
    );
    assert_eq!(
        routed.get("is_local").and_then(serde_json::Value::as_bool),
        Some(true),
        "wayland#372: a loopback endpoint must be reported as local — that is \
         the local-vs-cloud distinction the report asks for. Frame: {routed:#?}"
    );

    // The origin and nothing else: a query string is where an API key hides.
    let endpoint = routed.get("endpoint").and_then(|v| v.as_str()).unwrap();
    assert!(
        !endpoint.contains('?') && !endpoint.contains('@'),
        "the endpoint must carry no credentials: {endpoint}"
    );
}

// ===========================================================================
// The retry count
// ===========================================================================

/// #372: "retry count ... preserve the original run timer and show retry count
/// separately".
///
/// `provider_retry` tells the host that Core scheduled another attempt and
/// nothing else — not which attempt, not out of how many. The CLI's own
/// `emit_info` line already says "attempt 1/2"; the desktop host, which reads
/// events rather than prose, is left with a bare signal it cannot turn into a
/// progress indicator. That is exactly the "repeated hidden retries" the
/// reporter could not distinguish from a restarted planning loop.
#[tokio::test]
async fn a_scheduled_retry_names_the_attempt_and_the_budget() {
    // Budget PINNED so the test costs two backoffs, not the shipped curve.
    let _retry_budget = wcore_agent::test_utils::PinnedRetryBudget::pin(2);
    let h = harness(vec![
        Err(ProviderError::Api {
            status: 500,
            message: "upstream server error".into(),
        }),
        Ok(end_turn_text("recovered")),
    ])
    .await;
    let mut engine = h.engine;
    engine
        .run(USER_MARKER, "")
        .await
        .expect("the run must recover on the second attempt");

    let sends = h.calls.load(Ordering::SeqCst);
    // CONTROL. One send means the retry loop was never entered and the
    // assertions below would grade an event that was never emitted.
    assert!(
        sends > 1,
        "control failed: {sends} provider call(s) means no retry was scheduled"
    );

    let retries = h.wire.of_type("provider_retry");
    assert!(
        !retries.is_empty(),
        "control failed: no provider_retry frame reached the wire after {sends} sends"
    );

    let counted = retries
        .iter()
        .find(|frame| frame.get("attempt").is_some())
        .unwrap_or_else(|| {
            panic!(
                "wayland#372: no provider_retry frame carries an `attempt`. The host is \
                 told a retry was scheduled and cannot say which one, so it cannot show \
                 a retry count separately from the run timer. Frames were: {retries:#?}"
            )
        });

    assert_eq!(
        counted.get("attempt").and_then(serde_json::Value::as_u64),
        Some(1),
        "the first scheduled retry is attempt 1: {counted:#?}"
    );
    assert!(
        counted
            .get("max_attempts")
            .and_then(serde_json::Value::as_u64)
            .is_some_and(|max| max >= 1),
        "wayland#372: the retry must name the budget IN FORCE, not only its \
         ordinal — \"attempt 1\" alone cannot be rendered as progress. \
         Frame: {counted:#?}"
    );
}
