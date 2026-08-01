//! The learned user model reaches the provider under the SAME identity the
//! write path uses — proved at the outbound request body.
//!
//! # The defect
//!
//! `bootstrap.rs` rendered the user-context block under a hardcoded
//! `user_id = "default"`, while every write path resolved
//! `engine::resolve_user_model_user_id()`, which reads `WAYLAND_USER_ID`.
//! `engine.rs` said so outright in a doc comment: *"those two are not the same
//! value"*. On any host that sets the variable — the entire reason it exists —
//! the model was written under the real id and read back from `"default"`, so
//! nothing the layer inferred ever reached the prompt.
//!
//! The worse half is that `"default"` is a bucket OTHER processes write. With
//! the render pinned there, a process running with `WAYLAND_USER_ID` unset
//! wrote its inferred brief into the one bucket every *named* user's prompt
//! rendered from — one user's inferred traits landing in another user's system
//! prompt, and onward to the provider. That is what
//! [`a_named_user_does_not_render_the_default_buckets_model`] pins.
//!
//! # Why the assertion is at the wire and not at the store
//!
//! Same reason as `user_model_correction_wire.rs`: a store-level assertion
//! passes throughout this defect. The value was always *in* a store. It was in
//! the wrong one, and only the bytes leaving for the provider can tell the
//! difference. `wiremock`'s `received_requests()` yields those literal bytes;
//! nothing between the assertion and the socket is mocked.
//!
//! # Instrument discipline (lane brief §3b-i and §6b-ii)
//!
//! The three-assertion shape, all three present:
//!
//! 1. **known-positive** — [`the_resolved_user_ids_model_reaches_the_wire`]
//!    seeds the resolved bucket and finds its style line on the wire;
//! 2. **known-negative** — [`a_named_user_does_not_render_the_default_buckets_model`]
//!    seeds `"default"` with a DIFFERENT, equally-renderable style and requires
//!    it to be absent. Both worlds render a block, so "the block vanished
//!    entirely" cannot pass this;
//! 3. **the old shape would have missed it** —
//!    [`the_prefix_render_expression_reads_an_empty_bucket`] evaluates the
//!    pre-fix expression (`brief("default")`) against the same seeded store and
//!    shows it yields the default brief, which renders no block at all.
//!    Without this the file would pass unchanged against the code it fixes.
//!
//! # `WAYLAND_USER_ID` is process-global — how that is contained here
//!
//! This repo has an active flake family caused by tests toggling process
//! environment. This file adds nothing to it:
//!
//! * it lives in its own integration-test binary, so the variable it sets
//!   cannot be seen by any other test binary (`cargo` runs each in its own
//!   process);
//! * it sets the variable EXACTLY ONCE, via [`std::sync::Once`], and never
//!   unsets or changes it. Every test here wants the same value, so there is no
//!   toggle to race on — the discrimination comes from which *bucket* is
//!   seeded, not from changing the environment between tests;
//! * every test is `#[serial]`, so the one `set_var` cannot overlap another
//!   test's threads reading it.
//!
//! `user_model_correction_wire.rs` deliberately does NOT set the variable, so
//! it continues to resolve `"default"` and its `BOOTSTRAP_USER_ID` constant
//! stays correct.

mod common;

use std::path::Path;
use std::sync::{Arc, Once};

use common::{RECOVERY_TEST_KEY, configure_persisted_test_session};
use serde_json::json;
use serial_test::serial;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::anthropic::AnthropicProvider;
use wcore_user_model::{LocalBackend, Observation, UserModelBackend};
use wiremock::matchers::{method, path as wm_path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Identities and markers
// ---------------------------------------------------------------------------

/// The `WAYLAND_USER_ID` this binary runs under. A nonce so a match in a
/// captured body cannot have come from anywhere else.
const NAMED_USER_ID: &str = "alice-7QW2M9NONCE";

/// The bucket the pre-fix render site was hardcoded to, and the bucket a
/// process with `WAYLAND_USER_ID` unset still writes.
const DEFAULT_BUCKET: &str = "default";

/// The user message each turn sends. Doubles as the known-positive substring
/// proving a captured body is a real outbound prompt.
const PROBE_QUESTION: &str = "summarise the deployment plan";

/// Style fingerprints chosen so the two buckets render **disjoint** style
/// lines, and so both are non-default (an all-zero style renders nothing).
///
/// `LocalBackend::fold_style` is an EMA with `alpha = 1/(obs_count+1)`, so the
/// FIRST observation into a fresh bucket lands at exactly `fp / 2`. One
/// observation each therefore gives deterministic, exactly-formatted lines
/// rather than a drifting value — the fragility
/// `user_model_correction_wire.rs` documents having been bitten by.
const NAMED_FINGERPRINT: [f32; 4] = [1.0, 0.0, 0.0, 0.0];
const DEFAULT_FINGERPRINT: [f32; 4] = [0.0, 1.0, 0.0, 0.0];

/// What `render_user_context_block_with_corrections` emits for each of the
/// above, after the `/ 2` fold. Asserted verbatim rather than by prefix so a
/// renderer change surfaces here instead of silently weakening the test.
const NAMED_STYLE_LINE: &str =
    "- style: formality=0.50, energy=0.00, terseness=0.00, emoji_use=0.00";
const DEFAULT_STYLE_LINE: &str =
    "- style: formality=0.00, energy=0.50, terseness=0.00, emoji_use=0.00";

const USER_CONTEXT_HEADING: &str = "User context (from rolling profile)";

/// Set `WAYLAND_USER_ID` once for this binary. See the module docs for why this
/// is the containment strategy rather than a per-test guard.
fn install_named_user_id() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: called under `#[serial]`, exactly once per process, before
        // this binary starts any work that reads the environment. The value is
        // never changed or removed afterwards, so no reader can observe a
        // torn or shifting value.
        unsafe { std::env::set_var("WAYLAND_USER_ID", NAMED_USER_ID) };
    });
    // Not a tautology on the line above: it fails loudly if some other part of
    // the process has since overwritten the variable, which would silently
    // turn every test here into a test of the `"default"` bucket.
    assert_eq!(
        std::env::var("WAYLAND_USER_ID").as_deref(),
        Ok(NAMED_USER_ID),
        "WAYLAND_USER_ID is not the value this binary requires; every assertion \
         below would be measuring the wrong bucket"
    );
}

// ---------------------------------------------------------------------------
// Harness — deliberately the same shape as `user_model_correction_wire.rs`
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
    // The user-model backend is built only when `want_memory` is true. Without
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
                "id": "msg_uid_mock", "type": "message", "role": "assistant",
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

/// The user-model store path, computed exactly as `bootstrap.rs` computes it.
/// Duplicating the expression rather than hardcoding a path is deliberate: if
/// bootstrap's path moves, this helper moves with it and the test fails
/// honestly instead of seeding a file nobody reads.
fn user_model_path(cwd: &Path) -> std::path::PathBuf {
    wcore_memory::paths::auto_memory_dir(cwd)
        .map(|d| d.join("user-model.json"))
        .unwrap_or_else(|| cwd.join(".wayland").join("user-model.json"))
}

/// Seed an inferred style into `bucket`, through the real production writer
/// (`LocalBackend::observe`), at the path bootstrap reads from.
async fn seed_bucket(cwd: &Path, bucket: &str, fingerprint: [f32; 4]) {
    let backend =
        LocalBackend::with_persistence(user_model_path(cwd)).expect("open user-model store");
    backend
        .observe(
            bucket,
            Observation {
                style_fingerprint: Some(fingerprint),
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
/// assembled once during `build()`, so re-running on an existing engine would
/// re-send a prompt built before the seeding.
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

// ---------------------------------------------------------------------------
// 1. KNOWN-POSITIVE — the resolved identity's model is on the wire
// ---------------------------------------------------------------------------

/// **The headline proof.** With `WAYLAND_USER_ID` set, a model learned under
/// that id reaches the provider.
///
/// Before the fix this failed: the render site read `"default"`, which this
/// test never seeds, so the block was absent entirely.
#[tokio::test]
#[serial]
async fn the_resolved_user_ids_model_reaches_the_wire() {
    install_named_user_id();
    let server = start_mock_anthropic().await;
    let workdir = tempfile::TempDir::new().expect("workdir");
    let cwd_owned = common::bootstrap_workspace(workdir.path());
    let cwd = cwd_owned.as_path();

    // Seed ONLY the named bucket — the one the write path uses.
    seed_bucket(cwd, NAMED_USER_ID, NAMED_FINGERPRINT).await;

    // The store really carries it, so a miss on the wire below is the render
    // site's fault and not a seeding failure. This is the control whose
    // absence would let a broken seeder read as a broken renderer.
    let backend =
        LocalBackend::with_persistence(user_model_path(cwd)).expect("open user-model store");
    let seeded = backend.brief(NAMED_USER_ID).await.expect("brief");
    assert!(
        (seeded.style.formality - 0.5).abs() < 1e-6,
        "the seed did not land in the named bucket; got {seeded:?}"
    );

    drive_cold_session(&server, cwd, "msg-uid-1").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 0, "named-user session");

    assert!(
        bodies[0].contains(USER_CONTEXT_HEADING),
        "the user-context block is not in the outbound body at all. Body was: {}",
        bodies[0]
    );
    assert!(
        bodies[0].contains(NAMED_STYLE_LINE),
        "the model learned under WAYLAND_USER_ID={NAMED_USER_ID} did not reach the \
         outbound provider body. Expected {NAMED_STYLE_LINE:?}. A learned model that \
         does not reach the wire is not a learned model. Body was: {}",
        bodies[0]
    );
}

// ---------------------------------------------------------------------------
// 2. KNOWN-NEGATIVE — the `"default"` bucket is NOT rendered for a named user
// ---------------------------------------------------------------------------

/// **The cross-user assertion, and the known-negative for the test above.**
///
/// Only the `"default"` bucket is seeded — which is exactly what a sibling
/// process running with `WAYLAND_USER_ID` unset writes. A named user's prompt
/// must not contain it.
///
/// Before the fix this bucket was the *only* one the render site read, so this
/// test failed with the other user's style line present on the wire. That is
/// the leak, stated as a test rather than as a paragraph.
///
/// Note both worlds render a style line of identical shape, so this cannot pass
/// by the block disappearing — the second assertion requires the block to be
/// absent *because the bucket is empty*, and the third pins that the named
/// bucket's own line is what is missing rather than the whole feature.
#[tokio::test]
#[serial]
async fn a_named_user_does_not_render_the_default_buckets_model() {
    install_named_user_id();
    let server = start_mock_anthropic().await;
    let workdir = tempfile::TempDir::new().expect("workdir");
    let cwd_owned = common::bootstrap_workspace(workdir.path());
    let cwd = cwd_owned.as_path();

    // Seed ONLY `"default"` — another user's bucket, from this session's view.
    seed_bucket(cwd, DEFAULT_BUCKET, DEFAULT_FINGERPRINT).await;

    // POSITIVE CONTROL on the seed: the value really is in the `"default"`
    // bucket and really is renderable. Without this, a seeding failure would
    // produce the same "absent from the wire" result as a correct fix, and the
    // assertion below would pass for free.
    let backend =
        LocalBackend::with_persistence(user_model_path(cwd)).expect("open user-model store");
    let other = backend.brief(DEFAULT_BUCKET).await.expect("brief");
    assert!(
        (other.style.energy - 0.5).abs() < 1e-6,
        "the seed did not land in the \"default\" bucket, so this test proves nothing; \
         got {other:?}"
    );
    let rendered_other = wcore_agent::user_context::render_user_context_block(
        &other,
        &backend.preferences(DEFAULT_BUCKET).await.expect("prefs"),
    )
    .expect(
        "the \"default\" bucket's brief MUST be renderable, or its absence downstream \
             says nothing about which bucket was read",
    );
    assert!(
        rendered_other.contains(DEFAULT_STYLE_LINE),
        "the renderer does not emit {DEFAULT_STYLE_LINE:?} for the seeded default bucket, \
         so searching the wire for that string proves nothing. Rendered: {rendered_other}"
    );

    drive_cold_session(&server, cwd, "msg-uid-2").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 0, "named-user session, default bucket seeded");

    // THE CLAIM: another bucket's learned model is not in this user's prompt.
    assert!(
        !bodies[0].contains(DEFAULT_STYLE_LINE),
        "the \"default\" bucket's learned style reached a session running as \
         WAYLAND_USER_ID={NAMED_USER_ID}. That is one user's inferred traits in another \
         user's system prompt, on its way to the provider. Body was: {}",
        bodies[0]
    );
    // And the named bucket is empty, so nothing of its own is there either —
    // pinning that the render site read the named bucket and found it bare,
    // rather than reading nothing at all.
    assert!(
        !bodies[0].contains(NAMED_STYLE_LINE),
        "the named bucket was never seeded in this test, yet its style line is present. \
         The nonce values are not disjoint and every assertion here is meaningless. \
         Body was: {}",
        bodies[0]
    );
}

// ---------------------------------------------------------------------------
// 3. THE OLD SHAPE WOULD HAVE MISSED IT (lane brief §6b-ii, third assertion)
// ---------------------------------------------------------------------------

/// Without this, the two tests above would pass against the broken code as
/// easily as against the fix — so it is the only one that proves the repair
/// does anything.
///
/// The pre-fix render site was, verbatim:
///
/// ```ignore
/// let user_id = "default";
/// let brief = b.brief(user_id).await.unwrap_or_default();
/// let prefs = b.preferences(user_id).await.unwrap_or_default();
/// ```
///
/// Evaluated here against a store seeded the way production seeds it — under
/// the resolved id — it yields the DEFAULT brief, which renders no block at
/// all. The fixed expression, against the identical store, yields the seeded
/// one.
#[tokio::test]
#[serial]
async fn the_prefix_render_expression_reads_an_empty_bucket() {
    install_named_user_id();
    let workdir = tempfile::TempDir::new().expect("workdir");
    let cwd_owned = common::bootstrap_workspace(workdir.path());
    let cwd = cwd_owned.as_path();

    // Production seeding: the WRITE path keys by the resolved id.
    seed_bucket(cwd, NAMED_USER_ID, NAMED_FINGERPRINT).await;

    let backend =
        LocalBackend::with_persistence(user_model_path(cwd)).expect("open user-model store");

    // --- the OLD read, replayed verbatim ---------------------------------
    let old_brief = backend.brief(DEFAULT_BUCKET).await.unwrap_or_default();
    let old_prefs = backend
        .preferences(DEFAULT_BUCKET)
        .await
        .unwrap_or_default();
    let old_block = wcore_agent::user_context::render_user_context_block(&old_brief, &old_prefs);
    assert_eq!(
        old_block, None,
        "the pre-fix render site produced a block from the \"default\" bucket, which this \
         test never seeded. If that bucket is non-empty the demonstration is invalid."
    );

    // --- the NEW read, over the same store -------------------------------
    let resolved = std::env::var("WAYLAND_USER_ID").expect("set by install_named_user_id");
    let new_brief = backend.brief(&resolved).await.unwrap_or_default();
    let new_prefs = backend.preferences(&resolved).await.unwrap_or_default();
    let new_block = wcore_agent::user_context::render_user_context_block(&new_brief, &new_prefs)
        .expect("the fixed read site must produce a block from the bucket the writer used");
    assert!(
        new_block.contains(NAMED_STYLE_LINE),
        "the fixed read site produced a block without the learned style. Block was: \
         {new_block}"
    );

    // The two disagree, and that disagreement IS the defect. A test that could
    // not show this would pass identically on the broken code.
    assert_ne!(
        old_block.as_deref(),
        Some(new_block.as_str()),
        "the old and new read sites agree, so this file demonstrates nothing"
    );
}
