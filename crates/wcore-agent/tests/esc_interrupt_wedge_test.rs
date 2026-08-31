//! RED ARM — the Esc interrupt wedge.
//!
//! The in-turn keybar advertises `Esc interrupt`. Pressing Esc while a turn is
//! streaming routes `/cancel` to `TuiEngine::cancel`, which fires the per-turn
//! `CancellationToken` (`engine_bridge.rs`, stage 1). Live on macOS, 12 of 12
//! runs, the stream then stops cleanly and the session becomes permanently
//! unusable: every subsequent message is refused with
//!
//! ```text
//! session has an interrupted turn at journal cursor Some(N);
//! resume, reconcile, or cancel it before starting a new message
//! ```
//!
//! and every advertised way out (`/recover`, `continue`, `reconcile`, `cancel`,
//! quit + `--resume`) refuses in a closed loop.
//!
//! MECHANISM, read off the source rather than guessed. `ProviderAttemptStarted`
//! puts the attempt in `ExternalEffectState::Unknown` the instant the request is
//! dispatched (`session_journal/reducer.rs`), which is the NORMAL in-flight
//! state. On cancellation `run_journaled_turn` asks
//! `close_not_started_descendants_for_cancellation`; that function sees the
//! Unknown provider attempt, returns `ReconciliationRequired`, and
//! `run_journaled_turn` returns EARLY — before appending any terminal event
//! (`engine.rs`, the `matches!(result, Err(AgentError::UserAborted))` guard
//! above the `terminal` match). The turn therefore has no `TurnCommitted`,
//! no `TurnCancelled`, no `TurnFailed`. `RecoveryPlan::derive` then reaches
//! branch 4 and reports `Blocked { ProviderOutcomeUnknown }`, and
//! `run_with_content`'s pre-turn gate refuses every later message.
//!
//! WHAT THIS FILE ASSERTS, and what it deliberately does not. The red arm
//! asserts the OBSERVABLE END STATE — the next message is accepted and answered
//! — not any log line, error code, or refusal wording. A reword cannot green it.
//!
//! The refusal is CORRECT IN INTENT: the product genuinely does not know
//! whether the interrupted attempt produced effects. Control (c) is here to
//! stop the next agent "fixing" this by opening the fail-closed guard: an
//! operator must still be unable to assert an outcome the journal cannot
//! support. A fix must add an honest fourth disposition (abandon the turn with
//! its outcome recorded as genuinely unknown), never let a false outcome be
//! asserted and never fail open.

mod common;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use wcore_agent::engine::{AgentEngine, AgentError};
use wcore_agent::output::OutputSink;
use wcore_agent::session::SessionManager;
use wcore_agent::session_lifecycle::{
    OperatorResolution, ReconcileKind, SessionLifecycleError, reconcile_list, reconcile_resolve,
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

const FIRST_PROMPT: &str = "Write a very long 1500 word essay on double-entry bookkeeping.";
const SECOND_PROMPT: &str = "Reply with exactly one word: plum";
const SECOND_ANSWER: &str = "plum";

// ---------------------------------------------------------------------------
// Provider
// ---------------------------------------------------------------------------

/// What one `stream()` call does.
enum Script {
    /// Emit the events and end the stream.
    Complete(Vec<LlmEvent>),
    /// Emit some deltas, then hold the sender open forever. This is a turn
    /// mid-generation: the physical attempt has been accepted and durable
    /// stream batches exist, but no terminal provider receipt ever arrives.
    /// It is the exact state Esc lands in.
    StallMidStream,
}

struct ScriptedStreamProvider {
    script: Mutex<std::collections::VecDeque<Script>>,
    physical_url: String,
}

impl ScriptedStreamProvider {
    fn new(script: Vec<Script>, physical_url: String) -> Arc<Self> {
        Arc::new(Self {
            script: Mutex::new(script.into_iter().collect()),
            physical_url,
        })
    }
}

#[async_trait]
impl LlmProvider for ScriptedStreamProvider {
    async fn stream(&self, _r: &LlmRequest) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
        // The durable path records the accepted PHYSICAL attempt before any
        // scripted event becomes visible, so a purely in-memory provider cannot
        // reach the state under test. Cross a real local HTTP boundary first,
        // exactly as `common::MockLlmProvider::with_physical_url` does.
        let client = EgressClient::new().with_policy(Arc::new(AllowAllPolicy));
        let response =
            scope_max_retries(0, builder_send_with_retry(client.get(&self.physical_url))).await?;
        if !response.status().is_success() {
            return Err(ProviderError::Api {
                status: response.status().as_u16(),
                message: "fixture response".into(),
            });
        }

        let next = self.script.lock().unwrap().pop_front();
        let (tx, rx) = mpsc::channel(64);
        match next {
            Some(Script::StallMidStream) => {
                tokio::spawn(async move {
                    for _ in 0..3 {
                        if tx.send(LlmEvent::TextDelta("essay ".into())).await.is_err() {
                            return;
                        }
                    }
                    let _hold_the_stream_open = tx;
                    std::future::pending::<()>().await;
                });
            }
            Some(Script::Complete(events)) => {
                tokio::spawn(async move {
                    for event in events {
                        if tx.send(event).await.is_err() {
                            return;
                        }
                    }
                });
            }
            // Script exhausted: end cleanly so a wedge never shows up as a hang.
            None => {
                tokio::spawn(async move {
                    for event in end_turn("script exhausted") {
                        let _ = tx.send(event).await;
                    }
                });
            }
        }
        Ok(rx)
    }
}

fn end_turn(text: &str) -> Vec<LlmEvent> {
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
                ..Default::default()
            },
        },
    ]
}

// ---------------------------------------------------------------------------
// Sink — counts deltas so the test can cancel at a point where the attempt is
// provably in flight rather than after an arbitrary sleep.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct DeltaCountingSink {
    deltas: AtomicUsize,
    text: Mutex<String>,
}

impl DeltaCountingSink {
    fn deltas(&self) -> usize {
        self.deltas.load(Ordering::SeqCst)
    }
}

impl OutputSink for DeltaCountingSink {
    fn emit_text_delta(&self, text: &str, _: &str) {
        self.deltas.fetch_add(1, Ordering::SeqCst);
        self.text.lock().unwrap().push_str(text);
    }
    fn emit_thinking(&self, _: &str, _: &str) {}
    fn emit_tool_call(&self, _: &str, _: &str) {}
    fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
    fn emit_stream_start(&self, _: &str) {}
    fn emit_stream_end(&self, _: &str, _: usize, _: u64, _: u64, _: u64, _: u64, _: FinishReason) {}
    fn emit_error(&self, _: &str, _: bool, _: wcore_protocol::events::FailureCategory) {}
    fn emit_info(&self, _: &str) {}
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

struct Harness {
    engine: AgentEngine,
    sink: Arc<DeltaCountingSink>,
    session_dir: PathBuf,
    session_id: String,
    _root: tempfile::TempDir,
    _server: wiremock::MockServer,
}

async fn harness(script: Vec<Script>) -> Harness {
    let root = tempfile::tempdir().expect("tempdir");
    let server = physical_attempt_server().await;
    let provider = ScriptedStreamProvider::new(script, server.uri());
    let mut config = test_config();
    configure_persisted_test_session(&mut config, root.path());
    let session_dir = PathBuf::from(&config.session.directory);
    let sink = Arc::new(DeltaCountingSink::default());

    let mut engine = AgentEngine::new_with_provider(
        provider,
        config,
        ToolRegistry::new(),
        sink.clone() as Arc<dyn OutputSink>,
    );
    engine
        .init_session("test-provider", &root.path().to_string_lossy(), None)
        .expect("init_session must bind a durable journal");
    engine.use_recovery_test_key(&RECOVERY_TEST_KEY);
    let session_id = engine
        .current_session_id()
        .expect("a durable session must exist");

    Harness {
        engine,
        sink,
        session_dir,
        session_id,
        _root: root,
        _server: server,
    }
}

/// Install a fresh per-turn cancellation token and hand back the host's clone,
/// exactly as `TuiEngine::submit` does before every turn
/// (`turn_cancel = CancellationToken::new(); guard.set_cancel_token(..)`).
fn arm_turn(engine: &mut AgentEngine) -> CancellationToken {
    let token = CancellationToken::new();
    engine.set_cancel_token(token.clone());
    token
}

async fn wait_for_stream_in_flight(sink: &DeltaCountingSink) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while sink.deltas() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the scripted provider must reach the streaming path");
}

/// Drive a session into the exact post-Esc state: a turn interrupted while its
/// provider attempt was in flight. Returns with the engine still live, which is
/// the state the TUI is in when the user types their next message.
async fn wedge_with_esc(harness: &mut Harness) {
    let token = arm_turn(&mut harness.engine);
    let sink = harness.sink.clone();

    let interrupt = tokio::spawn(async move {
        wait_for_stream_in_flight(&sink).await;
        // This is `TuiEngine::cancel` stage 1 — the Esc keystroke.
        token.cancel();
    });

    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        harness.engine.run(FIRST_PROMPT, ""),
    )
    .await
    .expect("Esc must stop the in-flight turn, not hang it");
    interrupt.await.expect("interrupt task must join");

    assert!(
        matches!(outcome, Err(AgentError::UserAborted)),
        "the interrupted turn must surface as UserAborted — if it does not, this \
         harness is no longer reproducing an Esc interrupt and every assertion \
         below is vacuous. Got: {outcome:?}"
    );
}

/// Drive a session into a CRASHED mid-stream state: the turn's future is
/// dropped while its provider attempt is in flight, so nothing unwinds and no
/// terminal event is ever written.
///
/// This is the state Esc used to leave behind, and the one it no longer does.
/// A crash still produces it, and must: no live engine witnessed the end, so
/// the attempt's outcome is outstanding in the strict sense — nobody is in a
/// position to write a receipt for it. That is exactly the subject control (c)
/// grades the fail-closed operator guard on.
async fn wedge_with_crash(harness: &mut Harness) {
    let sink = harness.sink.clone();
    tokio::select! {
        _ = harness.engine.run(FIRST_PROMPT, "") => {
            panic!("the stalled turn must not complete on its own")
        }
        // A batch is made durable BEFORE its delta is forwarded, so a delta
        // proves the journal already holds provider bytes for this attempt.
        () = wait_for_stream_in_flight(&sink) => {}
    }
    // The run future is dropped here: no unwind, no terminal event, no
    // cancellation. The process is simply gone as far as the journal knows.
}

// ---------------------------------------------------------------------------
// RED ARM
// ---------------------------------------------------------------------------

/// FAILS TODAY. Esc is advertised as "interrupt", not "end the session".
/// After an interrupt the user must be able to send another message.
///
/// The assertion is on the observable end state — the next message is accepted
/// and answered — so no rewording of the refusal can green it.
#[tokio::test]
async fn an_esc_interrupted_turn_leaves_the_session_able_to_accept_the_next_message() {
    let mut harness = harness(vec![
        Script::StallMidStream,
        Script::Complete(end_turn(SECOND_ANSWER)),
    ])
    .await;

    wedge_with_esc(&mut harness).await;

    let _ = arm_turn(&mut harness.engine);
    let second = tokio::time::timeout(
        Duration::from_secs(30),
        harness.engine.run(SECOND_PROMPT, ""),
    )
    .await
    .expect("the message after an interrupt must not hang");

    let answered = second.expect(
        "after Esc interrupts a turn the session must still accept the next \
         message. Today it never does: the interrupted turn reached no terminal \
         journal event, so RecoveryPlan reports Blocked{ProviderOutcomeUnknown} \
         and run_with_content's pre-turn gate refuses every later message, with \
         no advertised recovery verb able to clear it",
    );
    assert!(
        answered.text.contains(SECOND_ANSWER),
        "the next message must actually be ANSWERED, not merely accepted — a fix \
         that returns Ok without running the turn is not a fix. Got: {:?}",
        answered.text
    );
}

// ---------------------------------------------------------------------------
// CONTROLS — all three must PASS today.
// ---------------------------------------------------------------------------

/// CONTROL (a). Esc with no turn in flight is harmless.
///
/// This isolates the cancellation token from the journal state: firing the same
/// token the Esc path fires, at a moment where no provider attempt is
/// outstanding, must leave the session fully usable. If this ever fails, the
/// red arm above is measuring token plumbing rather than the durable wedge.
#[tokio::test]
async fn esc_on_an_idle_session_leaves_the_next_message_acceptable() {
    let mut harness = harness(vec![Script::Complete(end_turn(SECOND_ANSWER))]).await;

    // Esc while idle: the token fires with nothing in flight.
    let idle_token = arm_turn(&mut harness.engine);
    idle_token.cancel();

    let _ = arm_turn(&mut harness.engine);
    let outcome = tokio::time::timeout(
        Duration::from_secs(30),
        harness.engine.run(SECOND_PROMPT, ""),
    )
    .await
    .expect("an idle-session Esc must not hang the next message")
    .expect("an idle-session Esc must leave the next message acceptable");
    assert!(outcome.text.contains(SECOND_ANSWER));
}

/// CONTROL (b). A turn that completes normally leaves the session usable.
///
/// Without this, the red arm would also fail if the durable session path were
/// broken outright — which would be a different defect with a different fix.
#[tokio::test]
async fn a_normally_completed_turn_leaves_the_next_message_acceptable() {
    let mut harness = harness(vec![
        Script::Complete(end_turn("first answer")),
        Script::Complete(end_turn(SECOND_ANSWER)),
    ])
    .await;

    let _ = arm_turn(&mut harness.engine);
    let first = tokio::time::timeout(
        Duration::from_secs(30),
        harness.engine.run(FIRST_PROMPT, ""),
    )
    .await
    .expect("an uninterrupted turn must not hang")
    .expect("an uninterrupted turn must succeed");
    assert!(first.text.contains("first answer"));

    let _ = arm_turn(&mut harness.engine);
    let second = tokio::time::timeout(
        Duration::from_secs(30),
        harness.engine.run(SECOND_PROMPT, ""),
    )
    .await
    .expect("the second message must not hang")
    .expect("a completed turn must leave the session accepting the next message");
    assert!(second.text.contains(SECOND_ANSWER));
}

/// CONTROL (d). THE ABANDONMENT MUST BE LEGIBLE, NOT A GAP.
///
/// Letting the user carry on is only half the job. Whatever the session did
/// with the interrupted request has to be READABLE afterwards by someone who
/// was not there — otherwise "recovered" and "quietly dropped on the floor"
/// look identical in the journal, and the next person to ask what that turn
/// cost has nothing to read.
///
/// So this asserts the durable record says, in order: the request went out, we
/// captured exactly these bytes, we never saw it end, and the turn was
/// cancelled. Note what is deliberately NOT asserted and NOT written — no
/// success, no `not-started`, and no finished stream. The stream stays open
/// forever because it never received a terminal event, and that open stream is
/// itself part of the record.
#[tokio::test]
async fn an_abandoned_turn_records_that_its_outcome_was_never_observed() {
    use wcore_agent::session_journal::{CompletionOutcome, ExternalEffectState, SessionJournal};

    let mut harness = harness(vec![Script::StallMidStream]).await;
    wedge_with_esc(&mut harness).await;

    let session_id = harness.session_id.clone();
    let session_dir = harness.session_dir.clone();
    let Harness {
        engine,
        _root,
        _server,
        ..
    } = harness;
    drop(engine);

    // Read the journal exactly as an outside reader would: from the file on
    // disk, with no engine and no privileged handle.
    let state = SessionJournal::recovered_state(session_dir.join(format!("{session_id}.journal")))
        .expect("a later reader must be able to replay the journal");

    let (attempt_id, attempt) = state
        .provider_attempts
        .iter()
        .next()
        .expect("the dispatched request must still be in the record");

    // 1. The outcome is recorded as unobserved — not claimed either way.
    let ExternalEffectState::Completed {
        outcome: CompletionOutcome::Failed { error },
    } = &attempt.effect
    else {
        panic!(
            "an abandoned attempt must carry a terminal receipt, not be left \
             nonterminal for a reader to guess at. Got: {:?}",
            attempt.effect
        )
    };
    assert_eq!(
        error,
        wcore_agent::recovery::PROVIDER_OUTCOME_ABANDONED_UNOBSERVED,
        "the receipt must say IN WORDS that the provider may have served and \
         charged for this request; a bare status code is not a record anyone \
         can act on"
    );

    // 2. The bytes actually captured are pinned, so the record is checkable
    //    against the stream rather than merely asserted beside it.
    assert!(
        attempt.response_digest.is_some(),
        "a partial capture must pin what it captured"
    );

    // 3. The stream is NOT marked finished: it never received a terminal
    //    event, and saying otherwise would claim a complete reply arrived.
    let stream = state
        .streams
        .values()
        .find(|stream| stream.attempt_id == *attempt_id)
        .expect("the durable stream must survive for a later reader");
    assert!(
        !stream.finished,
        "a stream cut off mid-reply must stay open — a finished stream is the \
         claim that the whole reply arrived"
    );
    assert!(
        !stream.batches.is_empty(),
        "the bytes that DID arrive must still be readable"
    );

    // 4. The turn itself reached a terminal boundary, which is what lets the
    //    session carry on at all.
    let turn = state
        .turns
        .values()
        .next()
        .expect("the interrupted turn must still be in the record");
    assert!(
        turn.completion.is_some(),
        "the abandoned turn must be terminal; a nonterminal turn is the wedge"
    );

    drop(_root);
    drop(_server);
}

/// CONTROL (e). THE REFUSAL MUST NAME AN EXIT THE READER'S SURFACE HAS.
///
/// This string is not TUI text. `AgentEngine::run` hands it verbatim to every
/// surface, including the `--json-stream` host, whose recovery vocabulary is
/// `continue|reconcile|cancel` — measured on the shipped binary, an
/// `"action":"abandon"` there is refused with "unknown variant `abandon`",
/// while `"action":"cancel"` parses and reaches the cursor check. So a refusal
/// that names only `/recover abandon` rebuilds the closed loop for that reader
/// out of the message itself: every verb their surface HAS refuses, and the one
/// it names their surface does not have.
///
/// `--resume` is the exit that exists everywhere — the resume path settles the
/// interrupted turn before the first prompt — so the refusal must name it.
#[tokio::test]
async fn the_refusal_names_an_exit_that_exists_off_the_terminal_ui() {
    let mut harness = harness(vec![Script::StallMidStream]).await;
    wedge_with_crash(&mut harness).await;

    let refused = harness.engine.run(SECOND_PROMPT, "").await;
    let AgentError::SessionAuthority(message) =
        refused.expect_err("a crash-interrupted turn must still refuse the next message")
    else {
        panic!("the pre-turn gate must refuse with SessionAuthority")
    };

    // Vacuity: without this the assertions below could pass on some other
    // refusal that never mentions recovery at all.
    assert!(
        message.contains("interrupted turn at journal cursor"),
        "this control must be grading the pre-turn recovery gate, not some \
         unrelated refusal. Got: {message}"
    );
    assert!(
        message.contains("abandon"),
        "the refusal must name the one disposition that is honest here. Got: {message}"
    );
    assert!(
        message.contains("--resume"),
        "the refusal must name an exit that exists on surfaces without \
         `/recover` — the json-stream host rejects `abandon` as an unknown \
         action, so naming only the slash command leaves that reader wedged \
         by the message itself. Got: {message}"
    );
}

/// CONTROL (c). THE FAIL-CLOSED GUARD MUST STILL BITE.
///
/// An operator asserting an outcome (`--as-outcome succeeded|failed|not-started`)
/// on an attempt with durable stream events must still be REFUSED: only the
/// engine that dispatched the request can mint a receipt carrying what it
/// proved, and no operator can honestly claim what the provider did.
///
/// FIXTURE NOTE — read this before changing it. This control originally reached
/// that state through `wedge_with_esc`, and no longer can, because the fix
/// removed the state from the Esc path rather than opening any guard: the live
/// engine now writes the attempt's honest terminal receipt at interrupt time,
/// so after Esc there is no outstanding attempt left for anyone to assert
/// about. The guard itself (`session_lifecycle`'s `operator_resolvable`, and
/// `reconcile_resolve`) was not touched. So the control now grades it on the
/// state that DOES still contain an unobserved attempt — a crash, where no
/// engine survived to write anything. Same assertions, same guard, on a
/// subject that still exists.
///
/// The `panic!` below is the vacuity guard and must be left in place: if the
/// crash fixture ever stops producing an outstanding provider attempt, this
/// control proves nothing and says so instead of passing quietly.
///
/// If a later change lets an operator assert a false outcome, or fails open,
/// this control turns red.
#[tokio::test]
async fn an_operator_still_cannot_assert_an_outcome_the_journal_cannot_support() {
    let mut harness = harness(vec![Script::StallMidStream]).await;
    wedge_with_crash(&mut harness).await;

    let session_id = harness.session_id.clone();
    let session_dir = harness.session_dir.clone();
    // Release the engine's exclusive journal writer lease — the operator verbs
    // run out-of-process against a session no engine holds.
    let Harness {
        engine,
        _root,
        _server,
        ..
    } = harness;
    drop(engine);

    let manager = SessionManager::new(session_dir, 10);

    let outstanding = reconcile_list(&manager, &session_id).expect("reconcile_list must read");
    let attempt = outstanding
        .iter()
        .find(|item| item.kind == ReconcileKind::ProviderAttempt)
        .unwrap_or_else(|| {
            panic!(
                "the interrupted turn must leave an outstanding provider attempt — \
                 without one this control proves nothing. Got: {outstanding:?}"
            )
        });
    assert!(
        !attempt.operator_resolvable,
        "an attempt with durable stream events must not be operator-resolvable: \
         only the engine can mint a receipt carrying the dispatch it proved"
    );

    for outcome in [
        OperatorResolution::Succeeded,
        OperatorResolution::Failed,
        OperatorResolution::NotStarted,
    ] {
        let refused = reconcile_resolve(
            &manager,
            &session_id,
            &attempt.tool_execution_id,
            Some(outcome),
            "test-operator",
        );
        assert!(
            matches!(
                refused,
                Err(SessionLifecycleError::RefusedByAuthority { .. })
            ),
            "asserting {outcome:?} on an attempt the journal cannot support must \
             stay refused — trading a wedged session for a fabricated receipt is \
             worse than the wedge. Got: {refused:?}"
        );
    }

    drop(_root);
    drop(_server);
}
