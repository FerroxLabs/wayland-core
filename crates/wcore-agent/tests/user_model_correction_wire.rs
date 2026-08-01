//! 23B Criterion 3, **user-model half** — correction precedence proved at the
//! outbound provider request body.
//!
//! # Why this file exists, and why it does not test what the verdict named
//!
//! The 23B verdict's G3c says: `update_user_model` is `SystemToken`-only and
//! `UserModelInferencer::infer` overwrites at every session end, so a user
//! correction would be reverted. Both clauses are true. But
//! `MemoryApi::update_user_model` writes the `wcore-memory` P5 `user_model`
//! partition, and **that partition has no production read into any prompt** —
//! every non-test reader is a display surface (`/memory show`, the CLI dump).
//! A correction there could survive forever and still never reach the model.
//!
//! The user model that actually reaches the provider is a different object:
//! `wcore-user-model`'s `UserBrief` + `Preferences`, rendered by
//! `crate::user_context::render_user_context_block*` and appended to the system
//! prompt at `bootstrap.rs`. That is the surface under test here.
//!
//! This is the same trap the memory half of C3 fell into — controls acting on
//! `Partition::Episodic` while only `Partition::Semantic` reached the prompt —
//! and the reason it went unnoticed there was the absence of exactly this kind
//! of proof. A row-level (or here, a store-level) test passes throughout.
//!
//! # What "outbound provider request body" means here
//!
//! `wcore_providers::AnthropicProvider` POSTs to `{base_url}/v1/messages`.
//! Pointing `base_url` at a `wiremock` server and reading
//! `server.received_requests()` yields the literal bytes that would have gone
//! to Anthropic. Nothing between the assertion and the socket is mocked.
//!
//! # Instrument discipline (lane brief §3b-i)
//!
//! Every absence assertion below is preceded, in the same test, by:
//!
//! 1. a length assertion on the captured requests (the artifact exists);
//! 2. a known-positive substring in that same body (the user's own message),
//!    proving a search over it would have found the missing string; and
//! 3. a *prior turn* in which the value now claimed absent was proved PRESENT
//!    on the wire.
//!
//! and the file carries [`the_control_proves_the_probe_can_fail`], which runs
//! the identical two-session shape with **no correction** and asserts the
//! inferred value IS still on the wire. Without it, a harness that quietly
//! stopped rendering the user-context block at all would pass every other test
//! here.

mod common;

use std::path::Path;
use std::sync::Arc;

use common::{RECOVERY_TEST_KEY, configure_persisted_test_session};
use serde_json::json;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::anthropic::AnthropicProvider;
use wcore_user_model::{CorrectionStore, LocalBackend, Observation, UserModelBackend};
use wiremock::matchers::{method, path as wm_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The user message each turn sends. Doubles as the known-positive substring
/// proving a captured body is a real outbound prompt.
const PROBE_QUESTION: &str = "summarise the deployment plan";

/// The style fingerprint seeded through the real `LocalBackend::observe`.
/// High on every axis so the folded value clears the 0.05 render threshold.
const SEED_FINGERPRINT: [f32; 4] = [0.8, 0.6, 0.6, 0.2];

/// The inferred-style line's structural prefix — the thing a correction must
/// REMOVE from the prompt.
///
/// This is deliberately NOT an exact number such as `formality=0.40`. The
/// engine's per-turn `observe_user_turn` folds a fresh style fingerprint on
/// every turn, so the rendered value **drifts between sessions**. An exact-value
/// marker would therefore go absent in session 2 on its own, and the headline
/// test's "the correction removed it" assertion would have passed without the
/// correction doing anything — a self-passing gate.
///
/// Found by this file's own forget test failing for exactly that reason before
/// the marker was structural; the earlier exact-value form is why the headline
/// assertion needed the paired control in
/// [`the_control_proves_the_probe_can_fail`], which now asserts this same
/// string IS present when no correction is made.
const INFERRED_STYLE_MARKER: &str = "- style: formality=";

/// The user's own words. A nonce so a match cannot come from anywhere else.
const CORRECTION_VALUE: &str = "blunt, no preamble QK7UM3NONCE";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn anthropic_test_config(base_url: &str) -> Config {
    let mut config = Config {
        provider_label: "anthropic".into(),
        provider: ProviderType::Anthropic,
        api_key: "sk-ant-not-a-real-key".into(),
        base_url: base_url.into(),
        model: "claude-mock".into(),
        max_tokens: 256,
        max_turns: Some(1),
        compat: ProviderCompat::anthropic_defaults(),
        ..Default::default()
    };
    // The user-model backend is built only when `want_memory` is true
    // (`bootstrap.rs`: `config.memory.enabled || skills_lifecycle`). Without
    // this the block under test is never rendered and every assertion below
    // would be measuring an empty prompt.
    config.memory.enabled = true;
    config
}

fn anthropic_text_sse(text: &str) -> String {
    format!(
        "event: message_start\ndata: {message_start}\n\n\
         event: content_block_start\ndata: {block_start}\n\n\
         event: content_block_delta\ndata: {delta}\n\n\
         event: content_block_stop\ndata: {block_stop}\n\n\
         event: message_delta\ndata: {message_delta}\n\n\
         event: message_stop\ndata: {message_stop}\n\n",
        message_start = json!({
            "type": "message_start",
            "message": {
                "id": "msg_um_mock", "type": "message", "role": "assistant",
                "content": [], "model": "claude-mock",
                "stop_reason": serde_json::Value::Null,
                "stop_sequence": serde_json::Value::Null,
                "usage": { "input_tokens": 10, "output_tokens": 1 }
            }
        }),
        block_start = json!({
            "type": "content_block_start", "index": 0,
            "content_block": { "type": "text", "text": "" }
        }),
        delta = json!({
            "type": "content_block_delta", "index": 0,
            "delta": { "type": "text_delta", "text": text }
        }),
        block_stop = json!({ "type": "content_block_stop", "index": 0 }),
        message_delta = json!({
            "type": "message_delta",
            "delta": { "stop_reason": "end_turn", "stop_sequence": serde_json::Value::Null },
            "usage": { "output_tokens": 1 }
        }),
        message_stop = json!({ "type": "message_stop" }),
    )
}

async fn start_mock_anthropic() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wm_path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(anthropic_text_sse("ok")),
        )
        .mount(&server)
        .await;
    server
}

/// The user-model persistence paths, computed exactly as `bootstrap.rs` does.
/// Duplicating the expression rather than hardcoding a path is deliberate: if
/// bootstrap's path ever moves, this helper moves with it and the test starts
/// failing honestly instead of seeding a file nobody reads.
fn user_model_paths(cwd: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let dir = wcore_memory::paths::auto_memory_dir(cwd);
    let brief = dir
        .clone()
        .map(|d| d.join("user-model.json"))
        .unwrap_or_else(|| cwd.join(".wayland").join("user-model.json"));
    let corrections = dir
        .map(|d| d.join("user-corrections.json"))
        .unwrap_or_else(|| cwd.join(".wayland").join("user-corrections.json"));
    (brief, corrections)
}

/// The user-id `bootstrap.rs` renders the user-context block under.
const BOOTSTRAP_USER_ID: &str = "default";

/// Seed an *inferred* belief about the user through the real production writer
/// (`LocalBackend::observe`), at the path bootstrap reads from.
async fn seed_inferred_style(cwd: &Path) {
    let (brief_path, _) = user_model_paths(cwd);
    let backend = LocalBackend::with_persistence(&brief_path).expect("open user-model store");
    backend
        .observe(
            BOOTSTRAP_USER_ID,
            Observation {
                style_fingerprint: Some(SEED_FINGERPRINT),
                ts_secs: 1_700_000_000,
                ..Observation::default()
            },
        )
        .await
        .expect("seed an observation");
}

/// Drive one cold session against the mock endpoint, from `cwd`.
///
/// A fresh `AgentBootstrap` per call is load-bearing: the user-context block is
/// assembled once during `build()`, so re-running on the same engine would
/// re-send the prompt built before the correction and every assertion would be
/// measuring the wrong session.
async fn drive_cold_session(server: &MockServer, cwd: &Path, msg_id: &str) {
    let provider: Arc<dyn LlmProvider> = Arc::new(AnthropicProvider::new(
        "sk-ant-not-a-real-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    ));
    let mut config = anthropic_test_config(&server.uri());
    configure_persisted_test_session(&mut config, cwd);
    let mut built = AgentBootstrap::new(config, cwd.to_str().unwrap(), null_output())
        .provider(provider)
        .build()
        .await
        .expect("bootstrap against the mock Anthropic endpoint");
    built
        .engine
        .init_session(msg_id, cwd.to_str().unwrap(), None)
        .expect("persisted session must bind the production budget authority");
    built.engine.use_recovery_test_key(&RECOVERY_TEST_KEY);
    // Surface a run failure instead of swallowing it. A turn that errors before
    // dispatch captures ZERO requests, and a silent `let _ =` turns that into
    // "the value is absent from the outbound body" — a free pass for every
    // absence assertion in this file.
    if let Err(e) = built.engine.run(PROBE_QUESTION, msg_id).await {
        panic!("turn {msg_id} failed before reaching the provider: {e}");
    }
}

fn null_output() -> Arc<dyn wcore_agent::output::OutputSink> {
    Arc::new(NullSink)
}

async fn captured_bodies(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("wiremock records requests")
        .into_iter()
        .map(|r| String::from_utf8(r.body).expect("request body is UTF-8"))
        .collect()
}

/// §3b-i: prove the artifact is a live instrument BEFORE claiming anything is
/// absent from it.
fn assert_body_is_a_live_instrument(bodies: &[String], index: usize, label: &str) {
    assert!(
        bodies.len() > index,
        "{label}: expected at least {} captured request(s), got {}. An absent artifact \
         makes every 'value is gone' assertion pass for free.",
        index + 1,
        bodies.len()
    );
    assert!(
        !bodies[index].is_empty(),
        "{label}: captured body {index} is EMPTY; nothing can be proved absent from it"
    );
    assert!(
        bodies[index].contains(PROBE_QUESTION),
        "{label}: captured body {index} does not contain the user's own message \
         ({PROBE_QUESTION:?}), so this is not a real outbound prompt and a search over it \
         proves nothing. Body was: {}",
        bodies[index]
    );
}

/// Everything that happens between two sessions in production: the session-end
/// P5 inference, plus further per-turn style observations. Both are the writers
/// G3c says would clobber a correction.
async fn run_everything_that_would_clobber_a_correction(cwd: &Path) {
    // (a) the per-turn inference fold, many times over.
    let (brief_path, _) = user_model_paths(cwd);
    let backend = LocalBackend::with_persistence(&brief_path).expect("open user-model store");
    for i in 0..25 {
        backend
            .observe(
                BOOTSTRAP_USER_ID,
                Observation {
                    style_fingerprint: Some([0.0, 0.0, 0.0, 0.0]),
                    ts_secs: 1_700_000_100 + i,
                    ..Observation::default()
                },
            )
            .await
            .expect("observe");
    }
    // (b) the session-end P5 user-model inference named by G3c. It writes
    // through `MemoryApi::update_user_model` with a System token and
    // overwrites its whole key set — the exact mechanism the verdict flagged.
    let memory: Arc<dyn wcore_memory::MemoryApi> = Arc::new(
        wcore_memory::open_for_test(&std::env::temp_dir())
            .await
            .expect("memory backend"),
    );
    let inferencer = wcore_memory::partition::UserModelInferencer::new(memory);
    inferencer
        .infer_and_persist(&[])
        .await
        .expect("session-end inference must run");
}

// ---------------------------------------------------------------------------
// The proofs
// ---------------------------------------------------------------------------

/// **The headline proof.** Correct the user model, end the session, start a new
/// one, and show the corrected value is what reaches the model — and that the
/// inference it replaces does not.
#[tokio::test]
async fn a_user_correction_survives_session_end_and_reaches_the_provider() {
    let server = start_mock_anthropic().await;
    let workdir = tempfile::TempDir::new().expect("workdir");
    let cwd_owned = common::bootstrap_workspace(workdir.path());
    let cwd = cwd_owned.as_path();

    seed_inferred_style(cwd).await;

    // --- session 1: the INFERRED value must reach the wire. Without this the
    // absence asserted after the correction would prove nothing at all.
    drive_cold_session(&server, cwd, "msg-um-1").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 0, "session 1");
    assert!(
        bodies[0].contains(INFERRED_STYLE_MARKER),
        "session 1: the inferred style never reached the outbound body, so this test cannot \
         prove a correction removed it. Body was: {}",
        bodies[0]
    );
    assert!(
        !bodies[0].contains(CORRECTION_VALUE),
        "session 1: the correction nonce is present before any correction was made — the \
         nonce is not unique and every later assertion is meaningless"
    );

    // --- the correction, through the real store the slash command writes to.
    let (_, corrections_path) = user_model_paths(cwd);
    let store = CorrectionStore::with_persistence(&corrections_path).expect("open corrections");
    store
        .correct(BOOTSTRAP_USER_ID, "style", CORRECTION_VALUE, 1_700_000_200)
        .await
        .expect("a correction must be storable");

    // --- everything that G3c says would revert it.
    run_everything_that_would_clobber_a_correction(cwd).await;

    // --- session 2: a brand-new cold session reading from disk.
    drive_cold_session(&server, cwd, "msg-um-2").await;
    let bodies = captured_bodies(&server).await;
    assert_eq!(
        bodies.len(),
        2,
        "expected exactly two outbound requests, one per cold session; got {}",
        bodies.len()
    );
    assert_body_is_a_live_instrument(&bodies, 1, "session 2");

    // The correction SURVIVED and is on the wire.
    assert!(
        bodies[1].contains(CORRECTION_VALUE),
        "session 2: the user's correction did not reach the outbound provider body. A \
         correction that does not reach the model is not a correction. Body was: {}",
        bodies[1]
    );
    // And it is marked as authoritative rather than as one more signal.
    assert!(
        bodies[1].contains("Corrected by the user"),
        "session 2: the correction reached the prompt without the precedence framing, so \
         the model cannot tell it outranks the inferences above it. Body was: {}",
        bodies[1]
    );
    // The inference it replaces is GONE — this is the assertion the old code
    // fails, and the reason precedence had to be subtractive.
    assert!(
        !bodies[1].contains(INFERRED_STYLE_MARKER),
        "session 2: the corrected-away inference is STILL in the outbound body alongside \
         the correction. The model is being handed a contradiction and left to choose, \
         which is not user control. Body was: {}",
        bodies[1]
    );
}

/// **The control that makes the absence above mean something.** Identical
/// two-session shape, no correction. The inferred value must STILL be on the
/// wire in session 2.
///
/// If the harness ever stops rendering the user-context block — a config
/// regression, a path change, a bootstrap reorder — this test goes red while
/// the headline test above would go *green for the wrong reason*.
#[tokio::test]
async fn the_control_proves_the_probe_can_fail() {
    let server = start_mock_anthropic().await;
    let workdir = tempfile::TempDir::new().expect("workdir");
    let cwd_owned = common::bootstrap_workspace(workdir.path());
    let cwd = cwd_owned.as_path();

    seed_inferred_style(cwd).await;
    drive_cold_session(&server, cwd, "msg-ctl-1").await;

    // Same clobber machinery, but NO correction.
    run_everything_that_would_clobber_a_correction(cwd).await;

    drive_cold_session(&server, cwd, "msg-ctl-2").await;
    let bodies = captured_bodies(&server).await;
    assert_eq!(bodies.len(), 2, "expected two outbound requests");
    assert_body_is_a_live_instrument(&bodies, 1, "control session 2");
    assert!(
        bodies[1].contains("User context (from rolling profile)"),
        "control: the user-context block is not being rendered at all, so the headline \
         test's absence assertion would pass for the wrong reason. Body was: {}",
        bodies[1]
    );
    // The load-bearing assertion. The headline test claims this exact string is
    // absent from session 2 *because of the correction*. Here — same shape, same
    // clobber machinery, no correction — it must be PRESENT. Without this, EMA
    // drift, a path change or a bootstrap reorder could remove the string on its
    // own and the headline test would pass having proved nothing.
    assert!(
        bodies[1].contains(INFERRED_STYLE_MARKER),
        "control: the inferred style line is absent from session 2 even though NO \
         correction was made. Something other than a correction removes it, so the \
         headline test's absence assertion proves nothing. Body was: {}",
        bodies[1]
    );
    assert!(
        !bodies[1].contains("Corrected by the user"),
        "control: a corrections section appeared without any correction being made"
    );
}

/// Forgetting a correction returns that subject to the inference — the
/// `forgetting` verb, on the user-model half, proved at the wire.
#[tokio::test]
async fn forgetting_a_correction_returns_the_subject_to_inference_on_the_wire() {
    let server = start_mock_anthropic().await;
    let workdir = tempfile::TempDir::new().expect("workdir");
    let cwd_owned = common::bootstrap_workspace(workdir.path());
    let cwd = cwd_owned.as_path();

    seed_inferred_style(cwd).await;
    let (_, corrections_path) = user_model_paths(cwd);
    let store = CorrectionStore::with_persistence(&corrections_path).expect("open corrections");
    store
        .correct(BOOTSTRAP_USER_ID, "style", CORRECTION_VALUE, 1_700_000_200)
        .await
        .expect("correct");

    // Session 1: correction present, inference suppressed.
    drive_cold_session(&server, cwd, "msg-fg-1").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 0, "forget session 1");
    assert!(
        bodies[0].contains(CORRECTION_VALUE),
        "correction on the wire"
    );
    assert!(
        !bodies[0].contains(INFERRED_STYLE_MARKER),
        "inference suppressed while corrected"
    );

    // Drop the correction.
    let removed = store
        .forget(BOOTSTRAP_USER_ID, "style")
        .await
        .expect("forget must not error")
        .expect("forget must report what it removed, not a bare ok");
    assert_eq!(removed.value, CORRECTION_VALUE);

    // Session 2: correction gone, inference returns.
    drive_cold_session(&server, cwd, "msg-fg-2").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 1, "forget session 2");
    assert!(
        !bodies[1].contains(CORRECTION_VALUE),
        "session 2: a forgotten correction is STILL on the wire. Body was: {}",
        bodies[1]
    );
    assert!(
        bodies[1].contains(INFERRED_STYLE_MARKER),
        "session 2: forgetting a correction must return the subject to whatever the agent \
         infers, not erase the subject entirely. Body was: {}",
        bodies[1]
    );
}
