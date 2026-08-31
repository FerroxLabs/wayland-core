//! RED ARM — #923 (session lost on a provider 400) and #1109b (a turn that
//! ends silently).
//!
//! Every positive assertion in this file is paired with a negative control that
//! exercises the SAME machinery on a case where today's behaviour is correct.
//! Without the pair, a harness that quietly stopped working would let the
//! positive claim pass vacuously.
//!
//! MEASURED, and it corrects the ticket: the session is NOT lost. On every
//! persisted configuration probed (durable journal bound, and the degraded
//! no-recovery-key path), the on-disk mirror after a provider 400 still holds
//! every message the run produced, tool results included. What IS lost is the
//! provider's own error — see `a_provider_400_reaches_the_caller_as_a_provider_400`.
//!
//! Scope: `wcore-agent/src/engine.rs` only.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tokio::sync::mpsc;
use wcore_agent::engine::AgentEngine;
use wcore_agent::output::OutputSink;
use wcore_egress::{AllowAllPolicy, EgressClient};
use wcore_providers::retry::{builder_send_with_retry, scope_max_retries};
use wcore_providers::{LlmProvider, ProviderError};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage};

use common::{
    RECOVERY_TEST_KEY, configure_persisted_test_session, physical_attempt_server, test_config,
};

// ---------------------------------------------------------------------------
// The two 400 bodies under test.
//
// ORPHAN_400 is the body Anthropic actually returns for the orphaned-tool-pair
// shape #923 is about. AUTH_400 is a 400 that a repair-and-retry must NEVER
// touch — it is the negative control that stops the fix becoming a blind retry
// of every client error.
// ---------------------------------------------------------------------------
const ORPHAN_400: &str = "messages.1: `tool_use` ids were found without `tool_result` \
                          blocks immediately after: toolu_01ABC. Each `tool_use` block must \
                          have a corresponding `tool_result` block in the next message.";
const AUTH_400: &str = "invalid x-api-key";

const USER_MARKER: &str = "the-irreplaceable-user-instruction";

// ---------------------------------------------------------------------------
// Scripted provider: one scripted outcome per `stream()` call.
// ---------------------------------------------------------------------------
struct ScriptedProvider {
    script: Mutex<std::collections::VecDeque<Result<Vec<LlmEvent>, ProviderError>>>,
    calls: Arc<AtomicUsize>,
    /// Persisted-session runs refuse a purely in-memory provider: the durable
    /// path demands an accepted PHYSICAL attempt identity before scripted
    /// events become visible. Every scripted outcome therefore crosses a real
    /// local HTTP boundary first, exactly as `common::MockLlmProvider`
    /// does via `with_physical_url`.
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
        // Cross the physical boundary for EVERY outcome, including the failing
        // ones — a real 400 is served after a real send.
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
            // Script exhausted: keep the run alive with a clean end turn so a
            // test that measures CALL COUNT never also trips on a hang.
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

fn api_400(message: &str) -> ProviderError {
    ProviderError::Api {
        status: 400,
        message: message.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Capturing sink.
// ---------------------------------------------------------------------------
#[derive(Default)]
struct CapSink {
    errors: Mutex<Vec<String>>,
    infos: Mutex<Vec<String>>,
    text: Mutex<String>,
}

impl CapSink {
    fn errors(&self) -> Vec<String> {
        self.errors.lock().unwrap().clone()
    }
    fn text(&self) -> String {
        self.text.lock().unwrap().clone()
    }
}

impl OutputSink for CapSink {
    fn emit_text_delta(&self, text: &str, _: &str) {
        self.text.lock().unwrap().push_str(text);
    }
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(
        &self,
        message: &str,
        _: bool,
        _category: wcore_protocol::events::FailureCategory,
    ) {
        self.errors.lock().unwrap().push(message.to_string());
    }
    fn emit_info(&self, message: &str) {
        self.infos.lock().unwrap().push(message.to_string());
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------
struct Harness {
    engine: AgentEngine,
    sink: Arc<CapSink>,
    calls: Arc<AtomicUsize>,
    session_dir: PathBuf,
    _root: tempfile::TempDir,
    _server: wiremock::MockServer,
}

async fn harness(script: Vec<Result<Vec<LlmEvent>, ProviderError>>) -> Harness {
    let root = tempfile::tempdir().expect("tempdir");
    let server = physical_attempt_server().await;
    let (provider, calls) = ScriptedProvider::new(script, server.uri());
    let mut config = test_config();
    configure_persisted_test_session(&mut config, root.path());
    let session_dir = PathBuf::from(&config.session.directory);
    let sink = Arc::new(CapSink::default());

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        sink.clone() as Arc<dyn OutputSink>,
    );
    engine
        .init_session("test-provider", &root.path().to_string_lossy(), None)
        .expect("init_session");
    engine.use_recovery_test_key(&RECOVERY_TEST_KEY);

    Harness {
        engine,
        sink,
        calls,
        session_dir,
        _root: root,
        _server: server,
    }
}

/// A second harness with session persistence OFF. Same provider, same sink,
/// same error — the ONLY difference is the durable-session path. It is the
/// paired control for every claim below about what the persisted path does to
/// a provider error.
async fn harness_ephemeral(script: Vec<Result<Vec<LlmEvent>, ProviderError>>) -> Harness {
    let root = tempfile::tempdir().expect("tempdir");
    let server = physical_attempt_server().await;
    let (provider, calls) = ScriptedProvider::new(script, server.uri());
    let config = test_config(); // session.enabled == false
    let session_dir = PathBuf::from(&config.session.directory);
    let sink = Arc::new(CapSink::default());
    let engine = AgentEngine::new_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        sink.clone() as Arc<dyn OutputSink>,
    );
    Harness {
        engine,
        sink,
        calls,
        session_dir,
        _root: root,
        _server: server,
    }
}

/// Every regular file under `dir`, relative to it, sorted.
fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(next) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&next) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(dir) {
                out.push(rel.to_path_buf());
            }
        }
    }
    out.sort();
    out
}

/// Does ANY file under `dir` contain `needle`? Returns the matching paths.
fn files_containing(dir: &Path, needle: &str) -> Vec<PathBuf> {
    files_under(dir)
        .into_iter()
        .filter(|rel| {
            std::fs::read(dir.join(rel))
                .map(|bytes| String::from_utf8_lossy(&bytes).contains(needle))
                .unwrap_or(false)
        })
        .collect()
}

// ===========================================================================
// #923 — A. the session is lost on a provider 400
// ===========================================================================

/// RED. A provider 400 must reach the user AS a provider 400.
///
/// MEASURED on `0ccaa90b`: it does not. `engine.rs:12285` returns without
/// settling the journal's provider attempt, so the turn can no longer be
/// closed; `require_turn_descendants_terminal`
/// (`session_journal/reducer.rs:1823`) then rejects it and its
/// `InvalidTransition` becomes the error the caller sees. The provider's own
/// words — the one artifact four investigations needed — are destroyed on the
/// way out.
///
/// The script holds the SAME 400 twice on purpose. Section C requires one
/// repair-and-retry for exactly this shape, so a turn whose second send would
/// succeed is a turn that legitimately ends `Ok` — and a one-entry script ends
/// in the harness's clean end-turn fallback. Two identical refusals is the case
/// this test is actually about: the provider refuses, the repair changes
/// nothing because there is nothing to repair, and the caller must be told what
/// the PROVIDER said. Pre-fix this script is indistinguishable from a
/// one-entry one — the arm returned on the first `Err` and never issued a
/// second send — so the assertion below is red against `0ccaa90b` either way.
#[tokio::test]
async fn a_provider_400_reaches_the_caller_as_a_provider_400() {
    let mut h = harness(vec![Err(api_400(ORPHAN_400)), Err(api_400(ORPHAN_400))]).await;
    let error = h
        .engine
        .run(USER_MARKER, "")
        .await
        .expect_err("a 400 must fail the turn")
        .to_string();
    assert!(
        error.contains("tool_use"),
        "#923: the provider's 400 was replaced on the way out. Caller got:\n           {error}\nProvider actually said:\n  {ORPHAN_400}"
    );
}

/// NEGATIVE CONTROL. The SAME provider error through an engine with session
/// persistence OFF. If this fails, the 400 body never existed and the test
/// above proves nothing; if it passes, the message is real and the persisted
/// path is what destroys it.
#[tokio::test]
async fn a_control_ephemeral_engine_keeps_the_provider_400_text() {
    let mut h = harness_ephemeral(vec![Err(api_400(ORPHAN_400)), Err(api_400(ORPHAN_400))]).await;
    let error = h
        .engine
        .run(USER_MARKER, "")
        .await
        .expect_err("a 400 must fail the turn")
        .to_string();
    assert!(
        error.contains("tool_use"),
        "CONTROL BROKEN: even without persistence the 400 body is absent, so \
         the assertion above cannot attribute the loss to anything. Got:\n  {error}"
    );
}

// ===========================================================================
// #923 — B. the failing request array is thrown away
// ===========================================================================

/// RED. The 400 path is holding the exact `messages` array the provider
/// rejected, plus the provider's explanation of what was wrong with it. Both
/// are dropped. Write them down next to the session they belong to.
#[tokio::test]
async fn b_provider_400_captures_the_failing_request_to_the_session_dir() {
    let mut h = harness(vec![Err(api_400(ORPHAN_400)), Err(api_400(ORPHAN_400))]).await;
    let _ = h.engine.run(USER_MARKER, "").await;

    // The capture ARTIFACT, not "the reason appears somewhere under the session
    // dir". MEASURED: with `capture_rejected_provider_request` stubbed to return
    // `None` — no capture written at all — a dir-wide substring scan for the
    // provider's reason STILL passes, because #923(2)'s honest journal receipt
    // (`CompletionOutcome::Failed { error }`) carries that same sentence into the
    // journal file. Such a scan grades the settle fix, not this one. What #923(1)
    // is for is the REQUEST ARRAY that earned the refusal, in a file a reporter
    // can attach — so that is what is pinned here.
    let captures: Vec<PathBuf> = files_under(&h.session_dir)
        .into_iter()
        .filter(|rel| rel.starts_with("rejected-requests"))
        .collect();
    assert!(
        !captures.is_empty(),
        "#923(1): no rejected-request capture was written under {}. Files \
         present: {:?}",
        h.session_dir.display(),
        files_under(&h.session_dir)
    );
    let bodies: Vec<String> = captures
        .iter()
        .map(|rel| {
            String::from_utf8_lossy(&std::fs::read(h.session_dir.join(rel)).expect("read capture"))
                .into_owned()
        })
        .collect();
    assert!(
        bodies
            .iter()
            .any(|body| body.contains("were found without")),
        "#923(1): the capture does not record WHY the provider refused: {captures:?}"
    );
    assert!(
        bodies.iter().any(|body| body.contains(USER_MARKER)),
        "#923(1): the capture does not hold the messages array that was refused \
         — the one artifact four investigations needed: {captures:?}"
    );
}

/// KNOWN-POSITIVE CONTROL for the scan above: the same scan, for a string the
/// session dir demonstrably DOES contain after a run. A silently-broken scan
/// returns nothing, and nothing reads as "absent".
#[tokio::test]
async fn b_control_the_session_dir_scan_finds_what_is_really_there() {
    let mut h = harness(vec![Err(api_400(ORPHAN_400)), Err(api_400(ORPHAN_400))]).await;
    let _ = h.engine.run(USER_MARKER, "").await;

    let hits = files_containing(&h.session_dir, USER_MARKER);
    assert!(
        !hits.is_empty(),
        "CONTROL BROKEN: the scan cannot even find the user's own instruction, \
         so the absence above is the scanner's, not the product's. Files: {:?}",
        files_under(&h.session_dir)
    );
}

/// RED. The capture is worthless if nobody is told where it is.
///
/// Two identical refusals, for the same reason section A scripts them: with
/// #923(3) in place a single orphan-shaped 400 is a RECOVERED turn (the retry
/// lands on the harness's clean end-turn fallback), and a turn that recovered
/// must NOT emit a hard error. This test is about the turn that genuinely
/// fails.
#[tokio::test]
async fn b_provider_400_surfaces_an_error_to_the_user() {
    let mut h = harness(vec![Err(api_400(ORPHAN_400)), Err(api_400(ORPHAN_400))]).await;
    let _ = h.engine.run(USER_MARKER, "").await;
    let errors = h.sink.errors();
    assert!(
        !errors.is_empty(),
        "#923(1): a provider 400 emitted NOTHING on the error channel — the \
         only user-visible signal is the process exit code"
    );
}

/// KNOWN-POSITIVE CONTROL for the sink wiring used above. `engine.rs`'s #86
/// empty-turn guard calls `emit_error` on an ordinary (non-error) run, so a
/// non-empty result here proves `CapSink::emit_error` is genuinely reached by
/// the engine — and therefore that the empty result in the test above is a
/// real absence, not a dead sink.
#[tokio::test]
async fn b_control_sink_receives_engine_errors() {
    let mut h = harness(vec![Ok(vec![LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        finish_reason: FinishReason::Stop,
        usage: usage(),
    }])])
    .await;
    let _ = h.engine.run(USER_MARKER, "").await;
    assert!(
        !h.sink.errors().is_empty(),
        "CONTROL BROKEN: the engine never reaches CapSink::emit_error at all, \
         so the #923 emit_error absence proves nothing"
    );
}

/// NEGATIVE CONTROL for the capture. `MissingApiKey` is non-retryable and lands
/// in the SAME dispatch arm as a 400, but nothing was ever sent for it — the
/// budget reservation is released on that exact ground. A capture filed for it
/// would be a record of a request that does not exist, and "Provider rejected
/// this request" a claim about a provider that was never asked.
#[tokio::test]
async fn b_control_an_unsent_call_files_no_capture_and_claims_no_refusal() {
    let mut h = harness(vec![
        Err(ProviderError::MissingApiKey),
        Err(ProviderError::MissingApiKey),
    ])
    .await;
    let result = h.engine.run(USER_MARKER, "").await;
    assert!(result.is_err(), "a missing credential must fail the turn");

    let captures: Vec<PathBuf> = files_under(&h.session_dir)
        .into_iter()
        .filter(|rel| rel.starts_with("rejected-requests"))
        .collect();
    assert!(
        captures.is_empty(),
        "#923(1): a capture was filed for a request that was never sent: {captures:?}"
    );
    let errors = h.sink.errors();
    assert!(
        !errors.is_empty(),
        "the user must still be told the call failed"
    );
    assert!(
        !errors
            .iter()
            .any(|error| error.contains("Provider rejected this request")),
        "#923(1): an unsent call was reported as a provider refusal: {errors:?}"
    );
}

// ===========================================================================
// #923 — C. one-shot repair-and-retry, narrowly gated
// ===========================================================================

/// An orphaned-tool-pair 400 is exactly the error the pre-send repair exists
/// to prevent; seeing one means the repair missed a shape. Repair once and
/// re-send once.
///
/// CHANGED after 0.13.5, and the reason matters. This test used to run against
/// the engine's default EMPTY history — no tool blocks anywhere — and still
/// asserted a repair-and-retry. That could only ever pass vacuously: the
/// pre-send repairs had already run against that exact array and are
/// idempotent, so the "repaired" second send was byte-for-byte the first and
/// re-earned the same 400. The retry was a guaranteed second bill that could
/// not change the outcome, and this test was what made that look correct.
///
/// The retry now happens only when the repair actually changes the array, so
/// the history seeded here carries a tool pair for the escalation to act on.
/// `control_c_a_toolless_conversation_is_not_resent` in
/// `issue_923_recurrence_test.rs` pins the other half: no tool blocks, no
/// second send.
#[tokio::test]
async fn c_orphan_shaped_400_is_repaired_and_retried_once() {
    let mut h = harness(vec![
        Err(api_400(ORPHAN_400)),
        Ok(end_turn_text("recovered")),
    ])
    .await;
    h.engine.load_conversation(vec![
        Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "read the config".to_string(),
            }],
        ),
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "toolu_01ABC".to_string(),
                name: "Read".to_string(),
                input: serde_json::json!({"path": "config.toml"}),
                extra: None,
            }],
        ),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "toolu_01ABC".to_string(),
                content: "port = 8080".to_string(),
                is_error: false,
            }],
        ),
    ]);
    let result = h.engine.run(USER_MARKER, "").await;
    let calls = h.calls.load(Ordering::SeqCst);
    assert!(
        result.is_ok(),
        "#923(3): an orphan-shaped 400 killed the turn instead of being \
         repaired and retried once (provider sends: {calls})"
    );
    assert_eq!(
        calls, 2,
        "#923(3): expected exactly one repair-and-retry (2 sends), got {calls}"
    );
    // A turn that RECOVERED must not also report a failure. `emit_error` is the
    // host's hard-error channel and the signal the SkillRouter/auto-skill
    // observers record a FAILURE from; emitting it for a refusal that is about
    // to be retried marks a successful turn as failed.
    assert!(
        h.sink.errors().is_empty(),
        "#923(3): the recovered turn still emitted a hard error: {:?}",
        h.sink.errors()
    );
}

/// NEGATIVE CONTROL — the one that stops (3) becoming a blind retry of every
/// client error. An auth 400 must be terminal on the FIRST send. This test
/// passes today and MUST still pass after the fix.
#[tokio::test]
async fn c_control_auth_400_is_never_retried() {
    let mut h = harness(vec![
        Err(api_400(AUTH_400)),
        Ok(end_turn_text("must not happen")),
    ])
    .await;
    let result = h.engine.run(USER_MARKER, "").await;
    let calls = h.calls.load(Ordering::SeqCst);
    assert!(result.is_err(), "an auth 400 must fail the turn");
    assert_eq!(
        calls, 1,
        "#923(3): an auth 400 was re-sent — the repair gate is not narrow \
         enough and every client error is now billed twice (sends: {calls})"
    );
}

// ===========================================================================
// #1109b — an empty turn that ends silently
// ===========================================================================

/// RED. `finish_reason == Error` means the PROVIDER signalled a failure (or
/// sent a stop signal the mapper could not classify). Telling that user the
/// endpoint "may be incompatible" sends them to verify a wire format that is
/// working. Only the genuinely unexplained empty turn earns that diagnosis —
/// the same argument that already carved out `Length`.
#[tokio::test]
async fn d_empty_turn_with_finish_reason_error_is_not_blamed_on_the_endpoint() {
    let mut h = harness(vec![Ok(vec![LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        finish_reason: FinishReason::Error,
        usage: usage(),
    }])])
    .await;
    let _ = h.engine.run(USER_MARKER, "").await;
    let errors = h.sink.errors();
    assert!(!errors.is_empty(), "an empty turn must say something");
    let joined = errors.join("\n");
    assert!(
        !joined.contains("may be incompatible"),
        "#1109b: an empty turn whose finish_reason is Error was diagnosed as an \
         endpoint/model incompatibility. The provider reported an error stop \
         signal; that is the fact to report. Got:\n{joined}"
    );
}

/// NEGATIVE CONTROL A — `Length` already gets its own honest message. Proves
/// the guard fires and that the branch machinery this test reads is live.
#[tokio::test]
async fn d_control_empty_turn_with_length_says_truncated() {
    let mut h = harness(vec![Ok(vec![LlmEvent::Done {
        stop_reason: StopReason::MaxTokens,
        finish_reason: FinishReason::Length,
        usage: usage(),
    }])])
    .await;
    let _ = h.engine.run(USER_MARKER, "").await;
    let joined = h.sink.errors().join("\n");
    assert!(
        joined.contains("TRUNCATED"),
        "CONTROL BROKEN: the empty-turn guard did not fire at all, so the \
         Error-branch assertion above proves nothing. Got:\n{joined}"
    );
}

/// NEGATIVE CONTROL B — a clean `Stop` with no content IS the unexplained
/// empty response, and must keep the incompatibility diagnosis.
#[tokio::test]
async fn d_control_empty_turn_with_clean_stop_keeps_the_endpoint_diagnosis() {
    let mut h = harness(vec![Ok(vec![LlmEvent::Done {
        stop_reason: StopReason::EndTurn,
        finish_reason: FinishReason::Stop,
        usage: usage(),
    }])])
    .await;
    let _ = h.engine.run(USER_MARKER, "").await;
    let joined = h.sink.errors().join("\n");
    assert!(
        joined.contains("may be incompatible"),
        "CONTROL BROKEN: the genuinely-unexplained empty turn lost its \
         diagnosis. Got:\n{joined}"
    );
}

/// RED (candidate mechanism for #1109's silent turn). A turn carrying ONLY
/// reasoning — no text, no tool calls — escapes the empty-turn guard entirely,
/// because `assistant_content` is non-empty. The run returns `Ok` with an
/// empty answer and nothing on the error channel: literally nothing happens.
#[tokio::test]
async fn e_thinking_only_turn_does_not_end_silently() {
    let mut h = harness(vec![Ok(vec![
        LlmEvent::ThinkingDelta("I should answer this.".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ])])
    .await;
    let result = h.engine.run(USER_MARKER, "").await;
    let answer_text = result.as_ref().map(|r| r.text.clone()).unwrap_or_default();
    let surfaced = h.sink.text();
    assert!(
        !h.sink.errors().is_empty() || !answer_text.is_empty(),
        "#1109b: a reasoning-only turn returned Ok with an empty answer and \
         no error — the run produced literally nothing the user can see. \
         result.text={answer_text:?} emitted_text={surfaced:?}"
    );
}

/// NEGATIVE CONTROL — an ordinary text turn emits no error at all, so the
/// assertion above is not satisfied by some unrelated always-on error.
#[tokio::test]
async fn e_control_ordinary_turn_emits_no_error() {
    let mut h = harness(vec![Ok(end_turn_text("here is your answer"))]).await;
    let result = h.engine.run(USER_MARKER, "").await.expect("run");
    assert_eq!(result.text, "here is your answer");
    assert!(
        h.sink.errors().is_empty(),
        "CONTROL BROKEN: an ordinary successful turn emits errors, so the \
         silent-turn assertions above could pass vacuously. Got: {:?}",
        h.sink.errors()
    );
}

// ===========================================================================
// #908 — an empty turn caused by OUR filter, blamed on the endpoint
// ===========================================================================

/// RED (#908). The reporter's model wrapped its whole reply in inline
/// reasoning tags. `508405d4` stopped those tags leaking into the answer, which
/// was the right fix and is not in question here — but it turned the reporter's
/// second symptom ("`</thought>` before responses") into their first one ("not
/// producing any response at all"), and the message that fires for it is false.
///
/// `assistant_content` carries NATIVE thinking blocks. An open-weights model
/// (DeepSeek-R1, QwQ, the Ollama-class models this was reported against) emits
/// its reasoning as ordinary text deltas instead, so `assistant_content` is
/// empty, the reasoning branch is skipped, and the turn is diagnosed as
/// "the endpoint or model may be incompatible" — sending the user to verify a
/// wire format that just delivered a complete response.
#[tokio::test]
async fn f_a_turn_the_reasoning_filter_emptied_is_not_blamed_on_the_endpoint() {
    let mut h = harness(vec![Ok(vec![
        LlmEvent::TextDelta("<think>I should answer this.</think>".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ])])
    .await;
    let _ = h.engine.run(USER_MARKER, "").await;
    let errors = h.sink.errors();
    assert!(
        !errors.is_empty(),
        "#908: a turn whose whole reply was reasoning-tagged ended with nothing said at all"
    );
    let joined = errors.join("\n");
    assert!(
        !joined.contains("may be incompatible"),
        "#908: the provider streamed a complete response and OUR filter removed it, and the \
         user was told their endpoint may be incompatible. Got:\n{joined}"
    );
    assert!(
        joined.contains("reasoning tags"),
        "#908: the message must name the real cause — the reply was all reasoning — so the \
         user does not go and re-check a working endpoint. Got:\n{joined}"
    );
}

/// RED (#908), the reporter's literal wording: *"sometimes just `</thought>`
/// repeatedly, the most so far has been 5 times along one row without anything
/// else included"*.
///
/// A stray closing tag is dropped by the filter and captured as nothing, so
/// this shape reaches the guard with an empty visible lane AND an empty
/// reasoning lane. It is the same defect and it must get the same honest
/// answer, which is why the fix counts RAW characters rather than captured
/// reasoning.
#[tokio::test]
async fn f_a_turn_of_only_stray_closing_tags_is_not_blamed_on_the_endpoint() {
    let mut h = harness(vec![Ok(vec![
        LlmEvent::TextDelta("</thought></thought></thought></thought></thought>".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ])])
    .await;
    let _ = h.engine.run(USER_MARKER, "").await;
    let joined = h.sink.errors().join("\n");
    assert!(
        !joined.is_empty(),
        "#908: five stray closing tags ended the turn in silence"
    );
    assert!(
        !joined.contains("may be incompatible"),
        "#908: a turn of nothing but stray `</thought>` tags was blamed on the endpoint. \
         Got:\n{joined}"
    );
}

/// NEGATIVE CONTROL — the guard must keep its teeth. A provider that streams
/// NOTHING is the genuinely unexplained empty response and must still get the
/// endpoint diagnosis; if this reddens, the fix above has simply deleted the
/// message rather than narrowing it.
///
/// (`d_control_empty_turn_with_clean_stop_keeps_the_endpoint_diagnosis` asserts
/// the same thing from the other direction. Both are kept: that one guards the
/// `finish_reason` chain, this one guards the raw-character count.)
#[tokio::test]
async fn f_control_a_provider_that_streams_nothing_still_gets_the_endpoint_diagnosis() {
    let mut h = harness(vec![Ok(vec![
        // An EMPTY text delta: the provider spoke and said nothing, which must
        // not be mistaken for text the filter ate.
        LlmEvent::TextDelta(String::new()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ])])
    .await;
    let _ = h.engine.run(USER_MARKER, "").await;
    let joined = h.sink.errors().join("\n");
    assert!(
        joined.contains("may be incompatible"),
        "CONTROL BROKEN: the unexplained empty response lost its diagnosis, so the two \
         assertions above pass against a guard that says nothing to anybody. Got:\n{joined}"
    );
}

/// NEGATIVE CONTROL — an ordinary turn that also carries reasoning must be
/// silent. Without this, a fix that fired the new message on every turn with a
/// `<think>` block in it would still pass the tests above.
#[tokio::test]
async fn f_control_reasoning_followed_by_an_answer_emits_no_error() {
    let mut h = harness(vec![Ok(vec![
        LlmEvent::TextDelta("<think>weighing it up</think>here is your answer".into()),
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: usage(),
        },
    ])])
    .await;
    let result = h.engine.run(USER_MARKER, "").await.expect("run");
    assert_eq!(result.text, "here is your answer");
    assert!(
        h.sink.errors().is_empty(),
        "CONTROL BROKEN: a normal reasoning-then-answer turn emitted an error. Got: {:?}",
        h.sink.errors()
    );
}
