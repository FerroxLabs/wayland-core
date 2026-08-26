//! RED ARM — #923, second half: the RETRY-EXHAUSTED provider exit.
//!
//! #923's shipped fix settles the failed turn's provider attempts at exactly
//! TWO call sites, `engine.rs:12500` and `engine.rs:12524`, and BOTH are
//! inside the non-retryable dispatch arm. `ProviderError::is_retryable()`
//! (`wcore-providers/src/lib.rs:258`) routes 5xx, `Connection` and
//! `RateLimited` PAST that arm at `engine.rs:12381` and into the retry loop,
//! whose exhausted exit — `engine.rs:13263` — returns with no settle at all.
//! That is the path every outage, timeout and 500 actually takes.
//!
//! Graded on the OBSERVABLE END STATE, never on a log line: the reduced
//! journal's provider attempts (`ExternalEffectState`) and the turn's own
//! `TurnCompletion`, both read back from the live journal authority after the
//! run, plus the error the caller is actually handed.
//!
//! Every positive claim is paired with a negative control on the SAME harness
//! exercising the already-fixed non-retryable 400 path, so a harness that
//! quietly stopped recording provider attempts cannot let the red arm pass
//! vacuously.
//!
//! Scope: `wcore-agent/src/engine.rs`.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::engine::{AgentEngine, AgentError};
use wcore_agent::output::OutputSink;
use wcore_agent::session_journal::{
    CompletionOutcome, ExternalEffectState, ReducedSessionState, TurnCompletion,
};
use wcore_egress::{AllowAllPolicy, EgressClient};
use wcore_providers::retry::{builder_send_with_retry, scope_max_retries};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{FinishReason, StopReason, TokenUsage};

use common::{
    RECOVERY_TEST_KEY, configure_persisted_test_session, physical_attempt_server, test_config,
};

/// A SERVED 5xx. `is_retryable()` is true for it
/// (`is_retryable_http_status(500)` && no permanent reason in the body), and
/// `provider_failure_code` renders `http_500`, which
/// `is_unserved_request_failure` does NOT match — so the run takes the small
/// fixed count budget rather than the wall-clock outage window, and the test
/// finishes in the two served backoffs (500 ms + 1000 ms).
const SERVED_5XX_BODY: &str = "upstream server error";

/// The already-fixed control. A 400 is non-retryable, so it lands in the
/// dispatch arm that #923 already settles. `invalid x-api-key` is deliberately
/// NOT the orphaned-tool-pair shape, so `is_orphaned_tool_pair_rejection` keeps
/// it off the repair-and-retry gate and it exercises the plain settle-and-return
/// site at `engine.rs:12524`.
const AUTH_400_BODY: &str = "invalid x-api-key";

const USER_MARKER: &str = "the-irreplaceable-user-instruction";

// ---------------------------------------------------------------------------
// Scripted provider — one scripted outcome per `stream()` call.
//
// Every outcome, failing ones included, crosses a REAL local HTTP boundary
// first. That send is what registers the physical provider attempt with the
// journal lifecycle; a purely in-memory provider records no attempt at all and
// the end state under test could never exist. This is the same construction
// `issue_923_1109b_red_test.rs` uses, and the reason it uses it.
// ---------------------------------------------------------------------------
struct ScriptedProvider {
    script: Mutex<std::collections::VecDeque<Result<Vec<LlmEvent>, ProviderError>>>,
    calls: Arc<AtomicUsize>,
    physical_url: String,
}

impl ScriptedProvider {
    fn new(
        script: Vec<Result<Vec<LlmEvent>, ProviderError>>,
        physical_url: String,
    ) -> (Arc<Self>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        (
            Arc::new(Self {
                script: Mutex::new(script.into_iter().collect()),
                calls: calls.clone(),
                physical_url,
            }),
            calls,
        )
    }
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
            // Script exhausted: end cleanly so a test that measures call COUNT
            // never also trips on a hang.
            None => end_turn_text("script exhausted"),
        };
        let (tx, rx) = mpsc::channel(64);
        for event in events {
            let _ = tx.send(event).await;
        }
        Ok(rx)
    }
}

fn usage() -> TokenUsage {
    TokenUsage {
        input_tokens: 10,
        output_tokens: 5,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        ..Default::default()
    }
}

fn end_turn_text(text: &str) -> Vec<LlmEvent> {
    vec![
        LlmEvent::TextDelta(text.to_string()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ]
}

fn api_error(status: u16, message: &str) -> ProviderError {
    ProviderError::Api {
        status,
        message: message.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Sink
// ---------------------------------------------------------------------------
#[derive(Default)]
struct CapSink {
    errors: Mutex<Vec<String>>,
}

impl OutputSink for CapSink {
    fn emit_text_delta(&self, _: &str, _: &str) {}
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, message: &str, _: bool) {
        self.errors.lock().unwrap().push(message.to_string());
    }
    fn emit_info(&self, _: &str) {}
}

// ---------------------------------------------------------------------------
// Harness — a PERSISTED session, because the end state under test is journal
// state and only the persisted path has a journal.
// ---------------------------------------------------------------------------
struct Harness {
    engine: AgentEngine,
    calls: Arc<AtomicUsize>,
    _root: tempfile::TempDir,
    _server: wiremock::MockServer,
}

impl Harness {
    /// The live journal authority's reduced state. This is the same
    /// `Arc`-shared writer the reducer validates against, not a second source
    /// of truth.
    fn journal_state(&self) -> ReducedSessionState {
        self.engine
            .session_journal()
            .expect(
                "HARNESS BROKEN: the persisted run has no journal, so no claim \
                 about journal end state below can mean anything",
            )
            .state()
            .expect("reduced journal state")
    }

    fn nonterminal_attempts(&self) -> Vec<String> {
        self.journal_state()
            .provider_attempts
            .iter()
            .filter(|(_, attempt)| {
                matches!(
                    attempt.effect,
                    ExternalEffectState::Prepared | ExternalEffectState::Unknown
                )
            })
            .map(|(id, attempt)| format!("{id} => {:?}", attempt.effect))
            .collect()
    }

    fn unclosed_turns(&self) -> Vec<String> {
        self.journal_state()
            .turns
            .iter()
            .filter(|(_, turn)| turn.completion.is_none())
            .map(|(id, _)| id.clone())
            .collect()
    }
}

async fn harness(script: Vec<Result<Vec<LlmEvent>, ProviderError>>) -> Harness {
    let root = tempfile::tempdir().expect("tempdir");
    let server = physical_attempt_server().await;
    let (provider, calls) = ScriptedProvider::new(script, server.uri());
    let mut config = test_config();
    configure_persisted_test_session(&mut config, root.path());

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        Arc::new(CapSink::default()) as Arc<dyn OutputSink>,
    );
    engine
        .init_session("test-provider", &root.path().to_string_lossy(), None)
        .expect("init_session");
    engine.use_recovery_test_key(&RECOVERY_TEST_KEY);

    Harness {
        engine,
        calls,
        _root: root,
        _server: server,
    }
}

// ===========================================================================
// RED — the retry-exhausted exit
// ===========================================================================

/// RED. A provider failure that exhausts the retry budget must leave the turn
/// and its provider attempts in a terminal state.
///
/// The script holds four identical served 5xx so the budget is genuinely
/// exhausted rather than recovering on a later send. `calls > 1` is what pins
/// this test to the RETRY-EXHAUSTED exit (`engine.rs:13263`) rather than the
/// already-settled non-retryable dispatch arm (`engine.rs:12524`): only the
/// retry loop's `continue 'stream` can produce a second physical send.
///
/// MEASURED at `94856f31`: the attempt stays `Unknown`, the turn never takes a
/// completion, and `require_turn_descendants_terminal`
/// (`session_journal/reducer.rs:1816`) then rejects the `TurnFailed` receipt
/// that `run_with_content` appends — so the caller is handed the reducer's
/// `InvalidTransition` instead of the provider's own words.
#[tokio::test]
async fn an_exhausted_retry_budget_settles_the_turns_provider_attempts() {
    // Budget PINNED, not inherited. This test drives a provider that fails
    // every attempt; the shipped default is 10 retries on the shared backoff
    // curve (127.5 s of scheduled sleep), and what is under test here is the
    // failure OUTCOME, not the size of the budget.
    let _retry_budget = wcore_agent::test_utils::PinnedRetryBudget::pin(2);
    let mut h = harness(vec![
        Err(api_error(500, SERVED_5XX_BODY)),
        Err(api_error(500, SERVED_5XX_BODY)),
        Err(api_error(500, SERVED_5XX_BODY)),
        Err(api_error(500, SERVED_5XX_BODY)),
    ])
    .await;
    let error = h
        .engine
        .run(USER_MARKER, "")
        .await
        .expect_err("an exhausted retry budget must fail the turn");

    let sends = h.calls.load(Ordering::SeqCst);
    assert!(
        sends > 1,
        "ARM NOT REACHED: {sends} physical send(s) means the run never entered \
         the retry loop, so this test graded the dispatch arm, not the \
         retry-exhausted exit it is about"
    );

    assert!(
        h.nonterminal_attempts().is_empty(),
        "#923: the retry-exhausted exit (engine.rs:13263) returned without \
         settling. Provider attempts left nonterminal after {sends} sends: \
         {:?}\nThe caller was handed: {error:?}",
        h.nonterminal_attempts()
    );
    assert!(
        h.unclosed_turns().is_empty(),
        "#923: the turn could not take a terminal receipt because it still \
         holds a nonterminal provider attempt. Unclosed turns: {:?}",
        h.unclosed_turns()
    );
    assert!(
        matches!(error, AgentError::ApiError(_)),
        "#923: the reducer's authority error replaced the provider's. Caller \
         got: {error:?}"
    );
    assert!(
        error.to_string().contains(SERVED_5XX_BODY),
        "#923: the provider's own words did not survive the exit. Caller got:\n  \
         {error}\nProvider actually said:\n  {SERVED_5XX_BODY}"
    );
}

// ===========================================================================
// NEGATIVE CONTROL — the already-fixed non-retryable 400 path
// ===========================================================================

/// NEGATIVE CONTROL. The SAME harness, the SAME journal assertions, on the path
/// #923's first half already fixed. A 400 is non-retryable, so it lands in the
/// dispatch arm at `engine.rs:12524`, which settles before returning.
///
/// If this fails, the harness never records a provider attempt at all and the
/// red arm above is measuring nothing. If it passes, the attempt bookkeeping is
/// real and the red arm's finding is the product's.
///
/// It also pins that the settle does NOT double-settle: exactly one terminal
/// receipt lands per attempt, and it carries the PROVIDER's error rather than a
/// second write over a receipt the stream-forwarding task owned.
#[tokio::test]
async fn a_control_non_retryable_400_still_settles_exactly_once() {
    let mut h = harness(vec![
        Err(api_error(400, AUTH_400_BODY)),
        Err(api_error(400, AUTH_400_BODY)),
    ])
    .await;
    let error = h
        .engine
        .run(USER_MARKER, "")
        .await
        .expect_err("a 400 must fail the turn");

    let state = h.journal_state();
    assert!(
        !state.provider_attempts.is_empty(),
        "CONTROL BROKEN: no provider attempt was recorded at all, so neither \
         this test nor the red arm above can observe a settle"
    );
    assert!(
        h.nonterminal_attempts().is_empty(),
        "CONTROL BROKEN: the already-shipped dispatch-arm settle did not run. \
         Nonterminal: {:?}",
        h.nonterminal_attempts()
    );
    assert!(
        h.unclosed_turns().is_empty(),
        "CONTROL BROKEN: the turn is unclosed on the path #923 already fixed. \
         Unclosed: {:?}",
        h.unclosed_turns()
    );

    // No double-settle. `require_unknown` (reducer.rs:1183) admits exactly one
    // terminal transition per attempt under the journal's single writer lock,
    // so a second settle cannot land — what it CAN do is land the wrong one
    // first. Pin the surviving receipt: every settled attempt must carry the
    // provider's own refusal, not a generic overwrite.
    let receipts: Vec<String> = state
        .provider_attempts
        .values()
        .map(|attempt| match &attempt.effect {
            ExternalEffectState::Completed {
                outcome: CompletionOutcome::Failed { error },
            } => error.clone(),
            other => format!("{other:?}"),
        })
        .collect();
    assert!(
        receipts.iter().any(|r| r.contains(AUTH_400_BODY)),
        "CONTROL BROKEN: the settled receipt does not carry the provider's own \
         refusal, so a settle on the retry path could not be graded against it \
         either. Receipts: {receipts:?}"
    );
    // The dispatch arm returns `Err(e.into())`, so the provider error reaches
    // the caller as `AgentError::Provider` — un-rewritten, which is the whole
    // point of #923(2). Anything else means the reducer masked it here too.
    assert!(
        matches!(error, AgentError::Provider(_)),
        "CONTROL BROKEN: even the fixed path loses the provider error: {error:?}"
    );

    // The turn's own receipt must be the failure, not a fabricated commit.
    let completions: Vec<TurnCompletion> = state
        .turns
        .values()
        .filter_map(|turn| turn.completion.clone())
        .collect();
    assert!(
        completions
            .iter()
            .any(|c| matches!(c, TurnCompletion::Failed { .. })),
        "CONTROL BROKEN: the failed turn took a non-failure receipt: {completions:?}"
    );
}
// ===========================================================================
// THE RECOVERED RETRY — the exit the first pass at this lane did not grade
// ===========================================================================

/// A retryable failure whose RETRY SUCCEEDS must still be able to commit its
/// turn.
///
/// `require_turn_descendants_terminal` gates `TurnCommitted` with the SAME
/// predicate it gates `TurnFailed` with (`reducer.rs`), so an attempt left
/// nonterminal by the retryable dispatch arm breaks the SUCCESS path too, not
/// only the retry-exhausted one. Settling at the exhausted exit alone left
/// this open: measured on that shape, a 500 followed by a clean answer handed
/// the caller
///
///     SessionAuthority("invalid journal state transition: turn turn-... has
///     nonterminal provider attempt ...")
///
/// and discarded the answer the provider had already produced. `sends > 1`
/// pins the test to the retry loop; `result.is_ok()` is the claim the
/// exhausted-exit test cannot make.
#[tokio::test]
async fn a_recovered_retry_can_still_commit_its_turn() {
    let mut h = harness(vec![
        Err(api_error(500, SERVED_5XX_BODY)),
        Ok(end_turn_text("recovered")),
    ])
    .await;
    let result = h.engine.run(USER_MARKER, "").await;

    let sends = h.calls.load(Ordering::SeqCst);
    assert!(
        sends > 1,
        "ARM NOT REACHED: {sends} physical send(s) means the run never entered \
         the retry loop, so this test graded the dispatch arm"
    );
    assert!(
        result.is_ok(),
        "#923: a run that RECOVERED on retry was failed anyway after {sends} \
         sends. Nonterminal attempts: {:?}. Unclosed turns: {:?}. Caller got: \
         {:?}",
        h.nonterminal_attempts(),
        h.unclosed_turns(),
        result.as_ref().err()
    );
    assert!(
        h.nonterminal_attempts().is_empty(),
        "#923: the failed first attempt was never settled even though the \
         retry succeeded: {:?}",
        h.nonterminal_attempts()
    );
    assert!(
        h.unclosed_turns().is_empty(),
        "#923: the recovered turn took no terminal receipt: {:?}",
        h.unclosed_turns()
    );
}
/// The same defect on the sibling retry arm: `ProviderError::ContextOverflow`
/// compacts and re-sends ONCE (`engine.rs`, the `!overflow_retried` arm).
/// That arm left its overflowing attempt nonterminal across the `continue`,
/// so a compaction that WORKED still could not commit its turn.
///
/// Measured before the fix: the caller was handed
/// `SessionAuthority("... has nonterminal provider attempt ...")` after 2
/// sends with the second one a clean `Done`.
#[tokio::test]
async fn a_compacted_overflow_retry_can_still_commit_its_turn() {
    let mut h = harness(vec![
        Err(ProviderError::ContextOverflow {
            required_tokens: 900_000,
            model_window: 200_000,
            routed_model: "probe-model".to_string(),
            message: "context overflow".to_string(),
        }),
        Ok(end_turn_text("recovered after compaction")),
    ])
    .await;
    let result = h.engine.run(USER_MARKER, "").await;

    let sends = h.calls.load(Ordering::SeqCst);
    assert!(
        sends > 1,
        "ARM NOT REACHED: {sends} physical send(s) means the overflow arm never \
         re-sent, so this test graded nothing"
    );
    assert!(
        result.is_ok(),
        "#923: a compacted-and-retried overflow was failed anyway after {sends} \
         sends. Nonterminal attempts: {:?}. Unclosed turns: {:?}. Caller got: \
         {:?}",
        h.nonterminal_attempts(),
        h.unclosed_turns(),
        result.as_ref().err()
    );
    assert!(
        h.nonterminal_attempts().is_empty(),
        "#923: the overflowing attempt was never settled: {:?}",
        h.nonterminal_attempts()
    );
}
