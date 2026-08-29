//! #998 — the ACP request's tool selection must CONSTRAIN the engine.
//!
//! `EngineTurnEngine::run_turn` used to open with `let _ = &req.tools;`. The
//! ACP server does the whole job of resolving the selection (per-call `tools`
//! when the body carries them, otherwise the list stored at `session/create`)
//! and then handed it to a bridge that dropped it on the floor. A host that
//! switched a tool off therefore got a control that lied: the tool was still
//! offered to the model and still dispatchable.
//!
//! These cases assert the WIRING, not just the helper functions: they read the
//! `tools[]` array off the request that actually reached the provider, which is
//! the only place "what the model was offered" is observable.

#[path = "support/mod.rs"]
mod support;

use std::sync::Arc;

use support::mock_llm::{MockLlm, received_requests};

use wcore_acp::protocol::ToolDefinition;
use wcore_acp::turn::{TurnEngine, TurnRequest};
use wcore_cli::acp_engine::{EngineTurnEngine, narrow_tool_allowlist, requested_tool_names};
use wcore_config::compat::ProviderCompat;
use wcore_config::config::{CliArgs, Config};
use wcore_config::debug::DebugConfig;
use wcore_providers::LlmProvider;
use wcore_providers::anthropic::AnthropicProvider;

use futures::stream::StreamExt;

fn test_config() -> Config {
    let mut config = Config::resolve(&CliArgs {
        provider: Some("anthropic".to_string()),
        api_key: Some("sk-ant-harness-not-real-key".to_string()),
        base_url: None,
        model: Some("claude-mock".to_string()),
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: true,
        project_dir: None,
    })
    .expect("resolve a default config");
    // Hermetic: `Config::resolve` reads the INVOKING ACCOUNT's real
    // config.toml, and a host whose ambient profile sets
    // `[storage.credentials] backend = "plaintext"` fails `init_session`
    // outright (durable recovery needs a confidential store). That has already
    // been mis-triaged once as a product defect. Durable sessions are
    // irrelevant to what these cases measure, so turn them off rather than
    // depend on the developer's profile.
    config.session.enabled = false;
    config
}

fn provider_against(base_url: &str) -> Arc<dyn LlmProvider> {
    Arc::new(
        AnthropicProvider::new(
            "sk-ant-harness-not-real-key",
            base_url,
            ProviderCompat::anthropic_defaults(),
            DebugConfig::default(),
        )
        .with_cache(false),
    )
}

fn tool_def(name: &str) -> ToolDefinition {
    ToolDefinition {
        name: name.to_string(),
        description: format!("{name} (selection carrier)"),
        input_schema: serde_json::json!({"type": "object"}),
    }
}

/// The names the outgoing provider request actually offered the model.
async fn offered_tool_names(server: &wiremock::MockServer) -> Vec<String> {
    let requests = received_requests(server).await;
    let first = requests
        .first()
        .expect("the turn must have reached the provider");
    first
        .body
        .get("tools")
        .and_then(|t| t.as_array())
        .map(|tools| {
            tools
                .iter()
                .filter_map(|t| t.get("name").and_then(|n| n.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// THE wiring proof: a selection naming only `Read` must leave `Bash` out of
/// what the model is offered — and out of the registry, so a hallucinated call
/// cannot reach it either.
#[tokio::test]
async fn a_selected_tool_set_narrows_what_the_engine_offers() {
    let mock = MockLlm::new().text("ok");
    let server = mock.start().await;
    let provider = provider_against(&server.uri());
    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let engine = EngineTurnEngine::with_provider(test_config(), cwd, provider);

    let stream = engine
        .run_turn(TurnRequest {
            session_id: "99999999-1111-2222-3333-aaaaaaaaaaaa".to_string(),
            text: "hi".to_string(),
            tools: vec![tool_def("Read")],
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await
        .expect("run_turn establishes a stream");
    let _: Vec<_> = stream.collect().await;

    let offered = offered_tool_names(&server).await;
    assert!(
        offered.iter().any(|n| n == "Read"),
        "the selected tool must still be offered; offered: {offered:?}"
    );
    assert!(
        !offered.iter().any(|n| n == "Bash"),
        "a tool the request did NOT select must not be offered; offered: {offered:?}"
    );
    assert!(
        !offered.iter().any(|n| n == "Write"),
        "nor any other unselected tool; offered: {offered:?}"
    );
}

/// BACK-COMPAT CONTROL. An empty `tools` is "no selection", so the engine keeps
/// its full registry exactly as it did before #998. Without this the test above
/// would also pass against an engine that simply offers nothing.
#[tokio::test]
async fn an_empty_selection_leaves_the_full_registry_intact() {
    let mock = MockLlm::new().text("ok");
    let server = mock.start().await;
    let provider = provider_against(&server.uri());
    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let engine = EngineTurnEngine::with_provider(test_config(), cwd, provider);

    let stream = engine
        .run_turn(TurnRequest {
            session_id: "99999999-1111-2222-3333-bbbbbbbbbbbb".to_string(),
            text: "hi".to_string(),
            tools: Vec::new(),
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await
        .expect("run_turn establishes a stream");
    let _: Vec<_> = stream.collect().await;

    let offered = offered_tool_names(&server).await;
    assert!(
        offered.iter().any(|n| n == "Read") && offered.iter().any(|n| n == "Bash"),
        "with no selection the engine must offer its whole registry; offered: {offered:?}"
    );
}

/// A session's toolset is bound once, at build time, because `retain` is a
/// one-way door and the engine is pooled per session id. A later turn that asks
/// for a DIFFERENT set is refused rather than silently ignored — silently
/// ignoring it is the exact defect this issue names.
#[tokio::test]
async fn a_later_turn_cannot_silently_change_the_selection() {
    let mock = MockLlm::new().text("ok");
    let server = mock.start().await;
    let provider = provider_against(&server.uri());
    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let engine = EngineTurnEngine::with_provider(test_config(), cwd, provider);
    let session_id = "99999999-1111-2222-3333-cccccccccccc".to_string();

    let stream = engine
        .run_turn(TurnRequest {
            session_id: session_id.clone(),
            text: "hi".to_string(),
            tools: vec![tool_def("Read")],
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await
        .expect("the first turn binds the selection");
    let _: Vec<_> = stream.collect().await;

    // Re-sending the SAME selection is fine — it is already enforced.
    assert!(
        engine
            .run_turn(TurnRequest {
                session_id: session_id.clone(),
                text: "again".to_string(),
                tools: vec![tool_def("Read")],
                agent: None,
                mcp_servers: Vec::new(),
            })
            .await
            .is_ok(),
        "an unchanged selection must not be refused"
    );

    // A different one cannot be honoured, so it must be reported.
    let outcome = engine
        .run_turn(TurnRequest {
            session_id,
            text: "now with bash".to_string(),
            tools: vec![tool_def("Bash")],
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await;
    let message = match outcome {
        Err(err) => err.to_string(),
        Ok(_) => panic!("a changed selection must be refused, not ignored"),
    };
    assert!(
        message.contains("tool selection"),
        "the refusal must name the reason; got {message}"
    );
}

/// A string that can only appear in a tool RESULT if `Bash` actually ran.
const RAN_MARKER: &str = "WHARDEN-RAN-9Z";

/// Every `tool_result` block the conversation carried back to the provider, as
/// `(is_error, content)`. This is where "was the tool dispatched" is
/// observable: the outbound `tools[]` array only says what the model was
/// OFFERED, and a model is free to call a tool it was never offered.
async fn tool_results_on_the_wire(server: &wiremock::MockServer) -> Vec<(bool, String)> {
    let mut out = Vec::new();
    for request in received_requests(server).await {
        let Some(messages) = request.body.get("messages").and_then(|m| m.as_array()) else {
            continue;
        };
        for message in messages {
            let Some(blocks) = message.get("content").and_then(|c| c.as_array()) else {
                continue;
            };
            for block in blocks {
                if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                    out.push((
                        block
                            .get("is_error")
                            .and_then(|e| e.as_bool())
                            .unwrap_or(false),
                        block
                            .get("content")
                            .and_then(|c| c.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    ));
                }
            }
        }
    }
    out
}

/// Drive one turn whose model hallucinates a `Bash` call, under `selection`,
/// and return the `tool_result` blocks that came back.
async fn hallucinated_bash_under(
    selection: Vec<ToolDefinition>,
    session_id: &str,
) -> Vec<(bool, String)> {
    let mock = MockLlm::new()
        .tool_use(
            "Bash",
            serde_json::json!({ "command": format!("echo {RAN_MARKER}") }),
        )
        .text("ok");
    let server = mock.start().await;
    let provider = provider_against(&server.uri());
    let cwd = std::env::current_dir()
        .unwrap()
        .to_string_lossy()
        .to_string();
    let engine = EngineTurnEngine::with_provider(test_config(), cwd, provider);

    let stream = engine
        .run_turn(TurnRequest {
            session_id: session_id.to_string(),
            text: "hi".to_string(),
            tools: selection,
            agent: None,
            mcp_servers: Vec::new(),
        })
        .await
        .expect("run_turn establishes a stream");
    let _: Vec<_> = stream.collect().await;

    tool_results_on_the_wire(&server).await
}

/// THE AUTHORITY ASSERTION. A deselected tool must be UNDISPATCHABLE, not
/// merely unadvertised.
///
/// # Why the offered-list tests above are not enough
///
/// They read `tools[]` off the outbound request, which is what the model was
/// OFFERED. A model is free to emit a call for a tool it was never offered —
/// that is what a hallucinated call IS — and the operator's switch has to
/// survive one. **Measured:** with `ToolRegistry::retain` changed to hide the
/// dropped tools from `to_tool_defs()` while leaving them registered, so the
/// selection filtered only the outgoing schema, every one of the cases above
/// passed and the hallucinated `Bash` below executed and returned its output.
///
/// So this drives a real turn in which the model calls `Bash` under a
/// selection of `["Read"]`, and grades the `tool_result` that comes back.
#[tokio::test]
async fn a_deselected_tool_refuses_a_hallucinated_call() {
    let results = hallucinated_bash_under(
        vec![tool_def("Read")],
        "99999999-1111-2222-3333-dddddddddddd",
    )
    .await;

    let (is_error, content) = results.first().expect(
        "the hallucinated call must be ANSWERED - no tool_result means the turn never \
                 got as far as dispatch and nothing below is being measured",
    );
    assert!(
        is_error,
        "a call to a deselected tool must come back as an ERROR result; got: {content:?}"
    );
    assert!(
        content.contains("Unknown tool"),
        "the refusal must be dispatch not finding the tool at all (it was dropped from the \
         registry), not some later failure that a re-widened registry could stop producing; \
         got: {content:?}"
    );
    assert!(
        !content.contains(RAN_MARKER),
        "the deselected tool RAN: the selection filtered only what the model was offered, so \
         the operator's switch is a control that lies. Result was: {content:?}"
    );
}

/// THE KNOWN-POSITIVE for the refusal above. The same hallucinated call, with
/// no selection declared, must NOT be refused as unknown — the tool is in the
/// registry and dispatch reaches it.
///
/// Without this, `is_error && "Unknown tool"` is equally well explained by a
/// harness whose mock never produced a reachable call at all, or by an engine
/// that refuses every tool.
///
/// The assertion is deliberately about the REFUSAL, not about Bash succeeding:
/// whether a shell command completes depends on the host's sandbox and shell,
/// but "Unknown tool" is the registry's answer and is the same everywhere.
#[tokio::test]
async fn the_same_call_is_dispatched_when_nothing_is_deselected() {
    let results = hallucinated_bash_under(Vec::new(), "99999999-1111-2222-3333-eeeeeeeeeeee").await;

    let (_, content) = results
        .first()
        .expect("the call must be answered here too, or the control proves nothing");
    assert!(
        !content.contains("Unknown tool"),
        "with no selection declared, Bash is in the registry and dispatch must REACH it - if \
         it is unknown here, the refusal in the test above is not caused by the selection; \
         got: {content:?}"
    );
}
// ── The pure composition rules, exercised directly ───────────────────────

#[test]
fn a_selection_is_normalized_to_a_set() {
    let names = requested_tool_names(&[tool_def("Read"), tool_def("Bash"), tool_def("Read")]);
    assert_eq!(names, vec!["Bash".to_string(), "Read".to_string()]);
}

#[test]
fn an_empty_selection_leaves_the_persona_allowlist_alone() {
    let persona = vec!["Read".to_string(), "Delegate".to_string()];
    assert_eq!(narrow_tool_allowlist(persona.clone(), &[]), persona);
}

#[test]
fn a_selection_without_a_persona_becomes_the_allowlist() {
    let requested = vec!["Read".to_string()];
    assert_eq!(
        narrow_tool_allowlist(Vec::new(), &requested),
        vec!["Read".to_string()]
    );
}

#[test]
fn a_selection_can_only_narrow_a_persona_never_widen_it() {
    let persona = vec!["Read".to_string()];
    // The request asks for Bash too; the persona never granted it.
    let composed = narrow_tool_allowlist(persona, &["Bash".to_string(), "Read".to_string()]);
    assert_eq!(
        composed,
        vec!["Read".to_string()],
        "a request must not hand back an authority the persona withheld"
    );
}

/// A selection that intersects the persona in NOTHING must not collapse to an
/// empty list: downstream, empty means "no restriction declared", which would
/// silently hand the session the persona's whole toolset back.
#[test]
fn a_disjoint_selection_falls_back_to_the_persona_not_to_allow_all() {
    let persona = vec!["Read".to_string()];
    assert_eq!(
        narrow_tool_allowlist(persona.clone(), &["Bash".to_string()]),
        persona
    );
}
