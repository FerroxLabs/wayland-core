//! A provider turn that carries no content must not become conversation.
//!
//! An OpenAI-compatible endpoint can answer HTTP 200 with a well-formed stream
//! that reaches `[DONE]` having emitted no text, no thinking and no tool call —
//! a model cut off at its output cap, or a response shape the SSE parser found
//! nothing in. Core surfaces that dead-end as an error. It must ALSO refuse to
//! remember it: an assistant turn with zero content blocks, once committed,
//! lands in the session file and is rebuilt into the `messages` array of every
//! later request as an empty message body. Strict endpoints reject that
//! outright; tolerant proxies repair it in place, and a proxy that repairs it
//! announces the repair in the response stream — so the repair text arrives as
//! assistant speech, is printed to the user, is journaled as real history, and
//! is replayed upstream on every subsequent turn.
//!
//! These tests drive the packaged `wayland-core` binary against a scripted
//! loopback provider and grade two pieces of world state: the `messages` array
//! the binary actually put on the wire, and the session file it actually wrote.

use std::path::Path;
use std::time::Duration;

use wcore_eval_scenarios::fixtures::openai::{
    OpenAiFixtureObservation, OpenAiFixtureScript, OpenAiStep,
};
use wcore_eval_scenarios::providers::{ProviderConfig, ProviderId};
use wcore_eval_scenarios::runner::{ScenarioResult, run_with_binary_in_environment};
use wcore_eval_scenarios::scenario::{ApprovalPolicy, Category, Scenario, Turn};
use wcore_eval_scenarios::tempenv::{self, TempEnv, TempEnvOptions};

/// Roles/positions of every persisted message that carries no content, across
/// every session file the run left behind.
fn content_free_messages_in_sessions(sessions_dir: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(sessions_dir) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(session) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        let Some(messages) = session.get("messages").and_then(|m| m.as_array()) else {
            continue;
        };
        for (index, message) in messages.iter().enumerate() {
            let blank = match message.get("content") {
                None | Some(serde_json::Value::Null) => true,
                Some(serde_json::Value::Array(blocks)) => blocks.is_empty(),
                Some(serde_json::Value::String(text)) => text.trim().is_empty(),
                Some(_) => false,
            };
            if blank {
                let role = message
                    .get("role")
                    .and_then(|role| role.as_str())
                    .unwrap_or("?");
                found.push(format!(
                    "{}#{index}:{role}",
                    path.file_name().unwrap_or_default().to_string_lossy()
                ));
            }
        }
    }
    found
}

async fn run(
    name: &'static str,
    steps: Vec<OpenAiStep>,
) -> (ScenarioResult, OpenAiFixtureObservation, TempEnv) {
    let fixture = OpenAiFixtureScript::new(steps)
        .start()
        .await
        .expect("start OpenAI fixture");
    let provider = ProviderConfig::new(ProviderId::OpenAI, "fixture-chat-v1")
        .with_api_key("fixture-local-token")
        .with_known_free_cost()
        .with_base_url(fixture.base_url());
    let env = tempenv::build_with(&provider, &TempEnvOptions::default())
        .expect("build hermetic environment");
    let scenario = Scenario::new(name, Category::Hardening)
        .max_total_time(Duration::from_secs(60))
        .approval(ApprovalPolicy::Yolo)
        .turn(Turn::new("Run the fixture script.").max_time(Duration::from_secs(30)));

    let result = run_with_binary_in_environment(
        &scenario,
        &provider,
        Path::new(env!("CARGO_BIN_EXE_wayland-core")),
        &env,
    )
    .await
    .expect("packaged Core run");
    let observation = fixture.shutdown().await.expect("fixture shutdown");
    (result, observation, env)
}

/// The defect, driven end to end: a tool round (so there is a real multi-request
/// conversation to poison) followed by a provider turn that carries nothing.
#[tokio::test]
async fn a_content_free_provider_turn_is_never_committed_to_the_conversation() {
    let (result, observation, env) = run(
        "content_free_turn_not_committed",
        vec![
            OpenAiStep::tool_call(
                "call_probe",
                "Bash",
                serde_json::json!({"command": "echo empty-turn-probe"}),
            ),
            OpenAiStep::empty_response(),
        ],
    )
    .await;

    // Both scripted steps must have been served, or every assertion below
    // passes vacuously.
    assert_eq!(
        observation.requests.len(),
        2,
        "the tool round and the content-free turn must both reach the provider; \
         observation: {observation:?}"
    );

    // (1) Nothing with an empty message body may leave Core — graded on the
    // `messages` array of the bytes the binary actually sent.
    let empty_on_the_wire: Vec<_> = observation
        .requests
        .iter()
        .filter(|request| !request.empty_content_messages.is_empty())
        .map(|request| (request.sequence, request.empty_content_messages.clone()))
        .collect();
    assert!(
        empty_on_the_wire.is_empty(),
        "Core put a message with an empty body on the wire: {empty_on_the_wire:?}"
    );

    // (2) …and nothing with an empty body may be persisted, because a session
    // file is replayed verbatim into the next request. This is the arm the
    // unfixed engine fails: it commits `{"role":"assistant","content":[]}`.
    let persisted = content_free_messages_in_sessions(env.sessions_dir());
    assert!(
        persisted.is_empty(),
        "the session file kept a content-free message: {persisted:?}"
    );

    // (3) Dropping the turn must not silence it. The dead-end is still an
    // error the user sees — a fix that just stops reporting would pass (1) and
    // (2) and leave the user staring at nothing.
    let reported = format!("{:?}", result.failures);
    assert!(
        reported.contains("empty response"),
        "the content-free turn must still be reported; failures={reported}"
    );
}

/// Same invariant with no tool round: the plain single-turn shape.
#[tokio::test]
async fn a_content_free_first_turn_leaves_no_empty_message_behind() {
    let (_result, observation, env) = run(
        "content_free_first_turn",
        vec![OpenAiStep::empty_response()],
    )
    .await;

    assert_eq!(
        observation.requests.len(),
        1,
        "observation: {observation:?}"
    );
    assert!(
        observation
            .requests
            .iter()
            .all(|request| request.empty_content_messages.is_empty()),
        "Core put a message with an empty body on the wire: {observation:?}"
    );
    let persisted = content_free_messages_in_sessions(env.sessions_dir());
    assert!(
        persisted.is_empty(),
        "the session file kept a content-free message: {persisted:?}"
    );
}
