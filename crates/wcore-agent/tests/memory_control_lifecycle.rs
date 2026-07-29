//! 23B Criterion 3 — the acceptance mechanism F23-03 is built around, and the
//! one thing Phase 23B never built.
//!
//! # Why this file exists
//!
//! `REQUIREMENTS.md:120-121` turns F23-03 on forgetting being proved by
//! **absence from the actual outbound provider request body**. The 23B-PHASE
//! verdict measured that no such proof existed anywhere
//! (`received_requests` in `crates/wcore-memory` + `crates/wcore-agent/src/slash`
//! → 0 files; known-positive in `crates/wcore-providers` → 7). What did exist
//! proved a deleted SQLite row — which `23B-02-PLAN` names *by hand* as the
//! engineered green to avoid, because a deleted row and an absent prompt are
//! different claims.
//!
//! They turned out to be different in this repo, and building the proof is what
//! showed it: `MemoryControls::forget_episode` hardcodes `Partition::Episodic`,
//! while `AgentEngine::recall_relevant_facts` keeps **only**
//! `Partition::Semantic` hits when it builds the `<system-reminder>` it injects
//! into the prompt. Forgetting deleted a row from the partition that never
//! reached the provider, and left untouched the partition that always did. The
//! row-level test passed throughout.
//!
//! # What "outbound provider request body" means here
//!
//! The real `wcore_providers::AnthropicProvider` POSTs to
//! `{base_url}/v1/messages`. Pointing `base_url` at a `wiremock` server and
//! reading `server.received_requests()` gives the literal bytes that would have
//! gone to Anthropic. This is the wire, not the engine's internal message list:
//! nothing between the assertion and the socket is mocked.
//!
//! # Instrument discipline (lane brief §3b-i)
//!
//! "The value is absent" is the single easiest assertion to pass without doing
//! any work — a missing artifact, an empty body, a typo'd field and a dead
//! instrument all produce it for free. So every absence assertion below is
//! paired, **in the same artifact**, with:
//!
//! 1. a length assertion on `received_requests` (the artifact exists at all);
//! 2. a non-empty assertion on the body bytes;
//! 3. a known-positive substring that IS in that same body (the user's own
//!    message text), proving the search would have found the nonce had it been
//!    there;
//!
//! and the file additionally carries [`the_probe_can_fail`], a control that runs
//! the identical two-turn shape **without** the forget and asserts the nonce
//! IS still in the second body. Without that control, a probe that silently
//! stopped injecting anything would pass every test here.

mod common;

use std::sync::Arc;

use common::{RECOVERY_TEST_KEY, configure_persisted_test_session};
use serde_json::json;
use wcore_agent::bootstrap::AgentBootstrap;
use wcore_agent::output::null_sink::NullSink;
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{Config, ProviderType};
use wcore_config::debug::DebugConfig;
use wcore_memory::MemoryApi;
use wcore_memory::v2_types::{AccessToken, Fact, FactId, Partition, Tier};
use wcore_providers::LlmProvider;
use wcore_providers::anthropic::AnthropicProvider;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The question every turn in this file asks. It is deliberately close in
/// wording to the planted fact so the cosine pass ranks the fact first, and it
/// doubles as the known-positive substring in each captured body.
const PROBE_QUESTION: &str = "what is my recorded deployment region";

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn anthropic_test_config(base_url: &str) -> Config {
    Config {
        provider_label: "anthropic".into(),
        provider: ProviderType::Anthropic,
        api_key: "sk-ant-not-a-real-key".into(),
        base_url: base_url.into(),
        model: "claude-mock".into(),
        max_tokens: 256,
        max_turns: Some(1),
        compat: ProviderCompat::anthropic_defaults(),
        ..Default::default()
    }
}

/// One complete Anthropic SSE text turn. Carries the terminal
/// `message_delta`+`message_stop` so the real parser never classifies the
/// stream as truncated and the engine does not retry (a retry would double the
/// request count and break the `requests.len()` assertions).
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
                "id": "msg_c3_mock",
                "type": "message",
                "role": "assistant",
                "content": [],
                "model": "claude-mock",
                "stop_reason": serde_json::Value::Null,
                "stop_sequence": serde_json::Value::Null,
                "usage": { "input_tokens": 10, "output_tokens": 1 }
            }
        }),
        block_start = json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" }
        }),
        delta = json!({
            "type": "content_block_delta",
            "index": 0,
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
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(anthropic_text_sse("ack"), "text/event-stream"),
        )
        .mount(&server)
        .await;
    server
}

/// Drive ONE cold turn through a freshly-bootstrapped engine bound to `memory`
/// and to the mock provider.
///
/// A fresh engine per turn is load-bearing, not incidental:
/// `AgentEngine::should_attempt_recall` only injects on the first user turn of
/// a session (`engine.rs:13311`), so a second turn on the same engine would
/// send no memory at all and every absence assertion below would pass for the
/// wrong reason.
async fn drive_cold_turn(server: &MockServer, memory: &Arc<dyn MemoryApi>, msg_id: &str) {
    let provider: Arc<dyn LlmProvider> = Arc::new(AnthropicProvider::new(
        "sk-ant-not-a-real-key",
        &server.uri(),
        ProviderCompat::anthropic_defaults(),
        DebugConfig::default(),
    ));
    let mut config = anthropic_test_config(&server.uri());
    let workdir = tempfile::TempDir::new().expect("workdir");
    configure_persisted_test_session(&mut config, workdir.path());
    let mut built = AgentBootstrap::new(config, workdir.path().to_str().unwrap(), null_output())
        .provider(provider)
        .build()
        .await
        .expect("bootstrap against the mock Anthropic endpoint");
    built.engine.set_memory_api(memory.clone());
    built
        .engine
        .init_session("c3", workdir.path().to_str().unwrap(), None)
        .expect("persisted session must bind the production budget authority");
    built.engine.use_recovery_test_key(&RECOVERY_TEST_KEY);
    let _ = built.engine.run(PROBE_QUESTION, msg_id).await;
}

fn null_output() -> Arc<dyn wcore_agent::output::OutputSink> {
    Arc::new(NullSink)
}

/// Plant a durable fact whose object is `nonce`, at the tier the engine's
/// session-start recall searches (`Tier::Project`).
async fn plant_fact(memory: &Arc<dyn MemoryApi>, nonce: &str) -> FactId {
    memory
        .assert_fact(
            Fact {
                id: FactId(uuid::Uuid::new_v4()),
                tier: Tier::Project,
                ts: chrono::Utc::now().timestamp(),
                subject: "the user".into(),
                predicate: "recorded deployment region is".into(),
                object: nonce.into(),
                confidence: 1.0,
                source_episode: None,
                superseded_by: None,
            },
            AccessToken::System,
        )
        .await
        .expect("planting a project-tier fact")
}

/// Read every captured request body as UTF-8. Fails loudly rather than
/// lossily: a body that is not valid UTF-8 would silently lose the nonce and
/// hand a free pass to an absence assertion.
async fn captured_bodies(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .expect("wiremock records requests")
        .into_iter()
        .map(|r| String::from_utf8(r.body).expect("request body is UTF-8"))
        .collect()
}

/// The paired assertion §3b-i demands: the artifact exists, is non-empty, and
/// demonstrably contains something we know is there — checked BEFORE anything
/// is claimed about what it does not contain.
fn assert_body_is_a_live_instrument(bodies: &[String], index: usize, label: &str) {
    assert!(
        bodies.len() > index,
        "{label}: expected at least {} captured request(s), got {}. \
         An absent artifact makes every 'value is gone' assertion pass for free.",
        index + 1,
        bodies.len()
    );
    let body = &bodies[index];
    assert!(
        !body.is_empty(),
        "{label}: captured request body {index} is EMPTY; nothing can be proved absent from it"
    );
    assert!(
        body.contains(PROBE_QUESTION),
        "{label}: captured request body {index} does not contain the user's own message \
         ({PROBE_QUESTION:?}), so the probe is not reading a real outbound prompt and a \
         nonce search over it proves nothing. Body was: {body}"
    );
}

// ---------------------------------------------------------------------------
// The proofs
// ---------------------------------------------------------------------------

/// **F23-03's acceptance mechanism.** Plant, prove present in the outbound
/// body, forget through the real `MemoryApi` control, prove absent from the
/// next outbound body.
#[tokio::test]
async fn forgetting_a_fact_removes_it_from_the_outbound_provider_request_body() {
    let server = start_mock_anthropic().await;
    let memory: Arc<dyn MemoryApi> = Arc::new(
        wcore_memory::open_for_test(&std::env::temp_dir())
            .await
            .expect("in-memory memory backend"),
    );
    let nonce = "eu-central-QK7ZC3NONCE";
    let fact_id = plant_fact(&memory, nonce).await;

    // --- turn 1: the nonce must REACH the wire, or nothing after this means
    // anything. This is the known-positive that keeps the whole test honest.
    drive_cold_turn(&server, &memory, "msg-c3-1").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 0, "turn 1");
    assert!(
        bodies[0].contains(nonce),
        "turn 1: the planted fact never reached the outbound provider body, so this test \
         cannot prove a later absence means anything. Body was: {}",
        bodies[0]
    );

    // --- forget, through the user-facing control path.
    let receipt = memory
        .forget_recalled(
            Tier::Project,
            &fact_id.0.to_string(),
            "operator",
            AccessToken::MainAgent,
        )
        .await
        .expect("forgetting a planted fact must succeed");
    assert_eq!(
        receipt.partition,
        Partition::Semantic,
        "the receipt must name the partition the item was really found in — a receipt that \
         always says 'episodic' is what let a fact survive a forget unnoticed"
    );
    assert!(
        receipt.in_changelog,
        "a forget must reach the CDC changelog"
    );

    // --- turn 2: the same question, a fresh cold session, the same store.
    drive_cold_turn(&server, &memory, "msg-c3-2").await;
    let bodies = captured_bodies(&server).await;
    assert_eq!(
        bodies.len(),
        2,
        "expected exactly two outbound requests (one per cold turn); got {}",
        bodies.len()
    );
    assert_body_is_a_live_instrument(&bodies, 1, "turn 2");
    assert!(
        !bodies[1].contains(nonce),
        "FORGET DID NOT REACH THE PROMPT: the forgotten value is still in the outbound \
         provider request body. Body was: {}",
        bodies[1]
    );
}

/// **The control that proves the probe can fail.** Identical shape, with the
/// forget removed. If this passes, the previous test's absence is a real
/// consequence of forgetting rather than of the probe having gone dead.
#[tokio::test]
async fn the_probe_can_fail() {
    let server = start_mock_anthropic().await;
    let memory: Arc<dyn MemoryApi> = Arc::new(
        wcore_memory::open_for_test(&std::env::temp_dir())
            .await
            .expect("in-memory memory backend"),
    );
    let nonce = "ap-south-QK7ZC3CONTROL";
    let _ = plant_fact(&memory, nonce).await;

    drive_cold_turn(&server, &memory, "msg-ctl-1").await;
    drive_cold_turn(&server, &memory, "msg-ctl-2").await;

    let bodies = captured_bodies(&server).await;
    assert_eq!(bodies.len(), 2, "expected two outbound requests");
    assert_body_is_a_live_instrument(&bodies, 1, "control turn 2");
    assert!(
        bodies[1].contains(nonce),
        "CONTROL FAILED: with no forget performed, the planted value should STILL be in the \
         second outbound body. It is not — so the absence asserted by the forget test above \
         would have passed with or without the forget, and proves nothing. Body was: {}",
        bodies[1]
    );
}

/// **Privacy scope must reach the wire too.** Before 23B-C3 the single
/// enforcement site (`retrieve.rs`) read `read_privacy_scope(db,
/// Partition::Episodic, tier)`, so `/memory privacy semantic <reason>` was
/// accepted, audited, and changed nothing about what was sent. That is a
/// control reporting success while doing nothing — the exact failure the
/// F23-03 controls exist to rule out.
#[tokio::test]
async fn a_semantic_privacy_scope_removes_facts_from_the_outbound_body() {
    let server = start_mock_anthropic().await;
    let memory: Arc<dyn MemoryApi> = Arc::new(
        wcore_memory::open_for_test(&std::env::temp_dir())
            .await
            .expect("in-memory memory backend"),
    );
    let nonce = "us-west-QK7ZC3PRIVACY";
    let _ = plant_fact(&memory, nonce).await;

    drive_cold_turn(&server, &memory, "msg-priv-1").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 0, "privacy turn 1");
    assert!(
        bodies[0].contains(nonce),
        "privacy turn 1: the planted fact must reach the wire first. Body was: {}",
        bodies[0]
    );

    let controls = memory
        .controls()
        .expect("the real backend exposes operator controls");
    controls
        .set_privacy_scope(
            &AccessToken::MainAgent,
            Partition::Semantic,
            Tier::Project,
            "user asked for durable facts to stop being sent",
            "operator",
        )
        .expect("scoping the semantic partition");

    drive_cold_turn(&server, &memory, "msg-priv-2").await;
    let bodies = captured_bodies(&server).await;
    assert_eq!(bodies.len(), 2, "expected two outbound requests");
    assert_body_is_a_live_instrument(&bodies, 1, "privacy turn 2");
    assert!(
        !bodies[1].contains(nonce),
        "PRIVACY SCOPE DID NOT REACH THE PROMPT: the scoped value is still in the outbound \
         provider request body. Body was: {}",
        bodies[1]
    );
}

/// **Retention must reach the wire too**, and by the same single-enforcement-
/// site argument. A one-day bound over a fact timestamped a week ago must
/// remove it from the prompt, and must do so by REPORTING it expired rather
/// than deleting it — so relaxing the bound brings it back.
#[tokio::test]
async fn a_semantic_retention_bound_removes_expired_facts_from_the_outbound_body() {
    let server = start_mock_anthropic().await;
    let memory: Arc<dyn MemoryApi> = Arc::new(
        wcore_memory::open_for_test(&std::env::temp_dir())
            .await
            .expect("in-memory memory backend"),
    );
    let nonce = "sa-east-QK7ZC3RETENTION";
    let week_ago = chrono::Utc::now().timestamp() - 7 * 86_400;
    memory
        .assert_fact(
            Fact {
                id: FactId(uuid::Uuid::new_v4()),
                tier: Tier::Project,
                ts: week_ago,
                subject: "the user".into(),
                predicate: "recorded deployment region is".into(),
                object: nonce.into(),
                confidence: 1.0,
                source_episode: None,
                superseded_by: None,
            },
            AccessToken::System,
        )
        .await
        .expect("planting an old project-tier fact");

    drive_cold_turn(&server, &memory, "msg-ret-1").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 0, "retention turn 1");
    assert!(
        bodies[0].contains(nonce),
        "retention turn 1: a week-old fact with no bound set must still reach the wire. \
         Body was: {}",
        bodies[0]
    );

    let controls = memory
        .controls()
        .expect("the real backend exposes operator controls");
    controls
        .set_retention(
            &AccessToken::MainAgent,
            Partition::Semantic,
            Tier::Project,
            86_400,
            "operator",
        )
        .expect("bounding semantic retention to one day");

    drive_cold_turn(&server, &memory, "msg-ret-2").await;
    let bodies = captured_bodies(&server).await;
    assert_eq!(bodies.len(), 2, "expected two outbound requests");
    assert_body_is_a_live_instrument(&bodies, 1, "retention turn 2");
    assert!(
        !bodies[1].contains(nonce),
        "RETENTION BOUND DID NOT REACH THE PROMPT: an expired value is still in the outbound \
         provider request body. Body was: {}",
        bodies[1]
    );

    // The row survives — retention excludes, it does not delete. Relaxing the
    // bound must bring the fact back, which is the only thing that makes
    // "reported expired" different from "silently destroyed".
    controls
        .set_retention(
            &AccessToken::MainAgent,
            Partition::Semantic,
            Tier::Project,
            365 * 86_400,
            "operator",
        )
        .expect("relaxing the bound");
    drive_cold_turn(&server, &memory, "msg-ret-3").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 2, "retention turn 3");
    assert!(
        bodies[2].contains(nonce),
        "retention must EXCLUDE, not delete: relaxing the bound should return the fact to \
         the prompt. Body was: {}",
        bodies[2]
    );
}

/// **Correction must reach the wire, and must not become a silent forget.**
/// `facts_cosine_pass` skips rows whose `embedding` is NULL, so the two lazy
/// implementations of "correct a fact" are both lies: keep the old vector and
/// the corrected fact is recalled by the query that matched the wrong text;
/// null the vector and `correct` silently performs a `forget`. This asserts
/// BOTH halves — the old value gone AND the new value present.
#[tokio::test]
async fn correcting_a_fact_replaces_it_in_the_outbound_body_rather_than_dropping_it() {
    let server = start_mock_anthropic().await;
    let memory: Arc<dyn MemoryApi> = Arc::new(
        wcore_memory::open_for_test(&std::env::temp_dir())
            .await
            .expect("in-memory memory backend"),
    );
    let wrong = "eu-west-QK7ZC3WRONG";
    let right = "eu-north-QK7ZC3RIGHT";
    let fact_id = plant_fact(&memory, wrong).await;

    drive_cold_turn(&server, &memory, "msg-cor-1").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 0, "correction turn 1");
    assert!(
        bodies[0].contains(wrong),
        "correction turn 1: the wrong value must reach the wire first. Body was: {}",
        bodies[0]
    );

    let receipt = memory
        .correct_recalled(
            Tier::Project,
            &fact_id.0.to_string(),
            right,
            "operator",
            AccessToken::MainAgent,
        )
        .await
        .expect("correcting a planted fact must succeed");
    assert_eq!(receipt.partition, Partition::Semantic);

    drive_cold_turn(&server, &memory, "msg-cor-2").await;
    let bodies = captured_bodies(&server).await;
    assert_eq!(bodies.len(), 2, "expected two outbound requests");
    assert_body_is_a_live_instrument(&bodies, 1, "correction turn 2");
    assert!(
        bodies[1].contains(right),
        "CORRECTION BECAME A SILENT FORGET: the corrected value is not in the outbound body. \
         A correction that drops the item is a forget wearing a correction's receipt. \
         Body was: {}",
        bodies[1]
    );
    assert!(
        !bodies[1].contains(wrong),
        "CORRECTION DID NOT REACH THE PROMPT: the pre-correction value is still in the \
         outbound body. Body was: {}",
        bodies[1]
    );
}

/// **`/memory why` must be able to answer for the content that is actually in
/// the prompt.** The dispatcher's `search_with_provenance` used to report
/// episodic hits only, by deliberate choice, while the engine injected
/// semantic hits only — so the command that exists to answer "why is this in
/// my context window" was silent about every item that was.
#[tokio::test]
async fn provenance_reports_the_partition_that_actually_reaches_the_prompt() {
    let memory: Arc<dyn MemoryApi> = Arc::new(
        wcore_memory::open_for_test(&std::env::temp_dir())
            .await
            .expect("in-memory memory backend"),
    );
    let nonce = "me-south-QK7ZC3WHY";
    let fact_id = plant_fact(&memory, nonce).await;

    let query = wcore_memory::v2_types::Query {
        text: PROBE_QUESTION.to_string(),
        tier: Tier::Project,
        ..Default::default()
    };
    let (hits, report) = memory
        .search_with_provenance(query.clone(), AccessToken::MainAgent)
        .await
        .expect("provenance search");

    // Known-positive first: the hit list itself must contain the fact, so a
    // later claim about the provenance list is a claim about a real recall.
    assert!(
        hits.iter().any(|h| h.id == fact_id.0.to_string()),
        "the planted fact is not even in the hit list; provenance cannot be asserted over it"
    );
    let entry = report
        .provenance
        .iter()
        .find(|p| p.id == fact_id.0.to_string())
        .unwrap_or_else(|| {
            panic!(
                "no provenance entry for the semantic fact that IS in the prompt. \
                 Entries reported: {:?}",
                report.provenance.iter().map(|p| &p.id).collect::<Vec<_>>()
            )
        });
    assert_eq!(entry.partition, Partition::Semantic);
    assert_eq!(
        entry.modality_label(),
        "vector",
        "the semantic pass is a single cosine ranking; reporting a 'fused' provenance for it \
         would be a fabrication"
    );

    // And the exclusion half: a scoped cell must be REPORTED as excluded, not
    // merely be missing. "Missing" and "withheld" are different answers to
    // "why is this not in my context", and only one of them is honest.
    let controls = memory.controls().expect("controls");
    controls
        .set_privacy_scope(
            &AccessToken::MainAgent,
            Partition::Semantic,
            Tier::Project,
            "scoped for the provenance test",
            "operator",
        )
        .expect("scoping");
    let (_hits, report) = memory
        .search_with_provenance(query, AccessToken::MainAgent)
        .await
        .expect("provenance search after scoping");
    assert!(
        report.exclusions.iter().any(|x| {
            x.partition == Partition::Semantic
                && matches!(x.cause, wcore_memory::ExclusionCause::PrivacyScope { .. })
        }),
        "a semantic privacy scope must be reported as an exclusion, not be silent. \
         Exclusions reported: {:?}",
        report.exclusions
    );
}

/// **The nudge bound must be reachable and must actually bound.** `NudgeBudget`
/// shipped in F23-03 with no caller outside its own unit tests, so the
/// criterion's "nudges" clause named a control no user could see. This drives
/// it through the `MemoryApi` accessor `/memory nudge` reads.
#[tokio::test]
async fn the_nudge_bound_is_reachable_through_the_memory_api_and_refuses_past_its_cap() {
    let memory: Arc<dyn MemoryApi> = Arc::new(
        wcore_memory::open_for_test(&std::env::temp_dir())
            .await
            .expect("in-memory memory backend"),
    );
    let budget = memory
        .nudge_budget()
        .expect("the real backend must expose a nudge bound");

    budget.set_cap(2);
    assert_eq!(budget.cap(), 2);
    assert!(budget.enabled());
    assert!(budget.request().is_ok(), "claim 1 of 2");
    assert!(budget.request().is_ok(), "claim 2 of 2");
    assert!(
        matches!(
            budget.request(),
            Err(wcore_memory::NudgeRefusal::CapReached { cap: 2 })
        ),
        "the third claim must be refused, and must say why"
    );

    // The off switch is a control, not a constant: it must be settable and it
    // must refuse the FIRST claim after a reset.
    budget.reset();
    let was_enabled = budget.set_enabled(false);
    assert!(was_enabled, "set_enabled must report the previous state");
    assert!(matches!(
        budget.request(),
        Err(wcore_memory::NudgeRefusal::Disabled)
    ));

    // And `NullMemory` must report having no bound rather than inventing one.
    let null: Arc<dyn MemoryApi> = Arc::new(wcore_memory::null::NullMemory);
    assert!(
        null.nudge_budget().is_none(),
        "a backend with no nudge path must say so rather than hand out a bound it cannot enforce"
    );
}

/// A control that reaches nothing must refuse, not report success. Guards the
/// `NullMemory` path for the two new `*_recalled` verbs.
#[tokio::test]
async fn controls_refuse_out_loud_on_a_backend_with_no_store() {
    let null: Arc<dyn MemoryApi> = Arc::new(wcore_memory::null::NullMemory);
    let err = null
        .forget_recalled(
            Tier::Project,
            "anything",
            "operator",
            AccessToken::MainAgent,
        )
        .await
        .expect_err("NullMemory must refuse a forget rather than report one");
    assert!(
        err.to_string().contains("no operator controls"),
        "the refusal must say why: {err}"
    );
    let err = null
        .correct_recalled(
            Tier::Project,
            "anything",
            "text",
            "operator",
            AccessToken::MainAgent,
        )
        .await
        .expect_err("NullMemory must refuse a correction rather than report one");
    assert!(
        err.to_string().contains("no operator controls"),
        "the refusal must say why: {err}"
    );
}

/// **Activation: the record must describe what really went into the prompt.**
/// Criterion 3's first clause had no surface at all — the engine injected
/// durable memory into the outbound body on the first turn of every session
/// and reported it to nobody but a `tracing::debug!` line. This asserts the
/// record and the wire agree, item for item, in the same run: anything the log
/// names must be findable in the captured body, and the captured body must
/// carry the nonce the log claims it placed.
#[tokio::test]
async fn the_activation_record_matches_what_reached_the_outbound_body() {
    let server = start_mock_anthropic().await;
    let memory: Arc<dyn MemoryApi> = Arc::new(
        wcore_memory::open_for_test(&std::env::temp_dir())
            .await
            .expect("in-memory memory backend"),
    );
    let log = memory
        .activation_log()
        .expect("the real backend must expose an activation log");
    assert!(
        log.last().is_none(),
        "before any turn there must be no activation record; an empty record here would make \
         'nothing was injected' indistinguishable from 'no recall has run'"
    );

    let nonce = "af-south-QK7ZC3ACTIVATION";
    let _ = plant_fact(&memory, nonce).await;
    drive_cold_turn(&server, &memory, "msg-act-1").await;

    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 0, "activation turn 1");
    let record = log.last().expect("a recall ran, so a record must exist");
    assert!(record.enabled);
    assert!(
        !record.injected.is_empty(),
        "the activation record claims nothing was injected, but the wire says otherwise. \
         Body was: {}",
        bodies[0]
    );
    // Every item the record names must actually be in the bytes that were sent.
    // A record that over-claims is worse than no record: it tells a user their
    // prompt contains something it does not, so they forget the wrong thing.
    for item in &record.injected {
        assert!(
            bodies[0].contains(&item.preview),
            "activation record names {:?} as injected, but it is NOT in the outbound body",
            item.preview
        );
    }
    assert!(
        record.injected.iter().any(|i| i.preview.contains(nonce)),
        "the planted nonce reached the wire but the activation record does not name it"
    );
}

/// **Activation is a CONTROL, not just a report.** With it switched off, no
/// durable memory may reach the outbound body at all — and the record must say
/// the off switch is why, not that nothing matched.
#[tokio::test]
async fn switching_activation_off_stops_memory_reaching_the_outbound_body() {
    let server = start_mock_anthropic().await;
    let memory: Arc<dyn MemoryApi> = Arc::new(
        wcore_memory::open_for_test(&std::env::temp_dir())
            .await
            .expect("in-memory memory backend"),
    );
    let nonce = "ca-central-QK7ZC3ACTOFF";
    let _ = plant_fact(&memory, nonce).await;
    let log = memory.activation_log().expect("activation log");

    // Known-positive first: with the switch ON the value must reach the wire,
    // or the absence asserted below would be free.
    drive_cold_turn(&server, &memory, "msg-actoff-1").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 0, "activation-off turn 1");
    assert!(
        bodies[0].contains(nonce),
        "with activation ON the fact must reach the wire. Body was: {}",
        bodies[0]
    );

    assert!(
        log.set_enabled(false),
        "set_enabled must report the previous state"
    );
    drive_cold_turn(&server, &memory, "msg-actoff-2").await;
    let bodies = captured_bodies(&server).await;
    assert_eq!(bodies.len(), 2, "expected two outbound requests");
    assert_body_is_a_live_instrument(&bodies, 1, "activation-off turn 2");
    assert!(
        !bodies[1].contains(nonce),
        "ACTIVATION OFF DID NOT REACH THE PROMPT: memory is still being injected. Body was: {}",
        bodies[1]
    );
    let record = log.last().expect("the disabled turn must still record");
    assert!(
        !record.enabled,
        "the record must say the off switch is why nothing was injected — 'nothing matched' \
         and 'you turned this off' are different answers"
    );
}

/// **A withheld item must be reported as withheld.** With a privacy scope set,
/// the activation record must name the exclusion rather than simply showing an
/// empty injection list — "missing" and "withheld" are different answers to
/// "why is this not in my prompt".
#[tokio::test]
async fn activation_reports_what_a_privacy_scope_withheld() {
    let server = start_mock_anthropic().await;
    let memory: Arc<dyn MemoryApi> = Arc::new(
        wcore_memory::open_for_test(&std::env::temp_dir())
            .await
            .expect("in-memory memory backend"),
    );
    let nonce = "eu-south-QK7ZC3WITHHELD";
    let _ = plant_fact(&memory, nonce).await;
    memory
        .controls()
        .expect("controls")
        .set_privacy_scope(
            &AccessToken::MainAgent,
            Partition::Semantic,
            Tier::Project,
            "withheld for the activation test",
            "operator",
        )
        .expect("scoping");

    drive_cold_turn(&server, &memory, "msg-withheld-1").await;
    let bodies = captured_bodies(&server).await;
    assert_body_is_a_live_instrument(&bodies, 0, "withheld turn 1");
    assert!(
        !bodies[0].contains(nonce),
        "a scoped fact must not reach the wire. Body was: {}",
        bodies[0]
    );

    let record = log_of(&memory);
    assert!(
        record.injected.is_empty(),
        "nothing should have been injected"
    );
    assert!(
        record
            .excluded
            .iter()
            .any(|x| x.partition == Partition::Semantic
                && matches!(x.cause, wcore_memory::ExclusionCause::PrivacyScope { .. })),
        "the activation record must NAME the privacy scope as the reason. Excluded: {:?}",
        record.excluded
    );
}

fn log_of(memory: &Arc<dyn MemoryApi>) -> wcore_memory::RecallActivation {
    memory
        .activation_log()
        .expect("activation log")
        .last()
        .expect("a recall ran, so a record must exist")
}
