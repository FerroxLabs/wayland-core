//! wayland#562 — a config MCP server that connects AFTER boot must still
//! reach the two session surfaces bootstrap wired at boot: the skill catalog
//! (and the prompt listing the model reads it from), and the plugin hook
//! dispatcher.
//!
//! HOW THESE FAIL IF THE DEFECT RETURNS: delete the `late_binder` call in
//! `integrate_deferred_mcp` (crates/wcore-cli/src/main.rs) — or, equivalently,
//! any single limb of `LateMcpBinder::bind` — and the session is back to what
//! wayland#551's deferral left behind: a json-stream chat whose MCP-served
//! skills are invisible to the model while a TUI on the same config sees them.
//! Each assertion below names one of those user-visible losses, and each of
//! the three limbs (catalog, prompt, hook dispatcher) is asserted separately
//! so a PARTIAL regression cannot pass.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use wcore_agent::bootstrap::{AgentBootstrap, load_session_skill_refs};
use wcore_agent::context::skills_reminder_section;
use wcore_agent::late_mcp::LateMcpBinder;
use wcore_config::config::Config;
use wcore_mcp::manager::McpManager;
use wcore_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolDef};
use wcore_mcp::transport::{McpError, McpTransport};
use wcore_skills::refs::SkillCatalog;

/// Answers `resources/list` then `resources/read` from a canned queue — the
/// exact two calls `wcore_skills::mcp::load_mcp_skills` makes per server.
struct ResourceTransport {
    responses: Mutex<Vec<serde_json::Value>>,
}

impl ResourceTransport {
    fn serving(uri: &str, body: &str) -> Box<dyn McpTransport> {
        Box::new(Self {
            responses: Mutex::new(vec![
                serde_json::json!({ "resources": [{ "uri": uri }] }),
                serde_json::json!({
                    "contents": [{ "uri": uri, "mimeType": "text/plain", "text": body }]
                }),
            ]),
        })
    }
}

#[async_trait]
impl McpTransport for ResourceTransport {
    async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let mut guard = self.responses.lock().unwrap();
        let value = if guard.is_empty() {
            serde_json::json!(null)
        } else {
            guard.remove(0)
        };
        Ok(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(1),
            result: Some(value),
            error: None,
        })
    }

    async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
        Ok(())
    }

    async fn close(&self) -> Result<(), McpError> {
        Ok(())
    }
}

struct SilentTransport;

#[async_trait]
impl McpTransport for SilentTransport {
    async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        Err(McpError::Transport("not used in this test".into()))
    }
    async fn notify(&self, _req: &JsonRpcRequest) -> Result<(), McpError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), McpError> {
        Ok(())
    }
}

const SKILL_BODY: &str = "---\nname: helper\ndescription: Formats a release note\n---\n\nBody.\n";

/// The name `uri_to_skill_name` derives for `skill://helper` on `notes-mcp`.
const MCP_SKILL: &str = "notes-mcp:helper";

fn tool(name: &str) -> McpToolDef {
    McpToolDef {
        name: name.to_string(),
        description: Some("hook tool".to_string()),
        input_schema: serde_json::json!({"type": "object"}),
    }
}

/// Boot a session exactly as a deferred json-stream boot leaves it: catalog
/// and prompt built with `mcp_manager = None`.
async fn deferred_boot(
    cwd: &std::path::Path,
) -> (wcore_agent::engine::AgentEngine, Arc<SkillCatalog>) {
    let bundled = wcore_skills::bundled::init_bundled_skills();
    let refs = load_session_skill_refs(cwd, &[], None, &bundled, None).await;
    let section = skills_reminder_section(&refs, None);

    let config = Config {
        system_prompt: Some(format!("BASE PROMPT\n\n{section}")),
        ..Default::default()
    };
    let (mut engine, _sink) = AgentBootstrap::build_for_test(config, vec![]);

    let catalog = Arc::new(SkillCatalog::from_refs(refs));
    engine.set_skill_catalog(Arc::clone(&catalog));
    engine.set_skill_listing_section(section);
    (engine, catalog)
}

fn binder_for(cwd: &std::path::Path, hooks: HashMap<String, Vec<String>>) -> LateMcpBinder {
    LateMcpBinder::new(
        cwd.to_path_buf(),
        vec![],
        Arc::new(wcore_skills::bundled::init_bundled_skills()),
        None,
        hooks,
        vec![],
        true,
    )
}

#[tokio::test]
async fn deferred_config_mcp_skill_reaches_catalog_and_prompt() {
    let cwd = tempfile::tempdir().unwrap();
    let (mut engine, catalog) = deferred_boot(cwd.path()).await;

    // The defect, stated: with the connect deferred, the MCP-served skill is
    // in neither the catalog nor the listing the model reads.
    assert!(
        catalog.find(MCP_SKILL).is_none(),
        "fixture is vacuous: the MCP skill must be absent before the late bind"
    );
    assert!(
        !engine.system_prompt().contains(MCP_SKILL),
        "fixture is vacuous: the prompt must not already list the MCP skill"
    );

    let manager = Arc::new(McpManager::new_for_test(vec![(
        "notes-mcp",
        true,
        ResourceTransport::serving("skill://helper", SKILL_BODY),
    )]));
    let outcome = binder_for(cwd.path(), HashMap::new())
        .bind(&mut engine, manager)
        .await;

    assert_eq!(
        outcome.skills_added,
        vec![MCP_SKILL.to_string()],
        "the late bind must report the skill the deferred server serves"
    );
    // Limb 1: the shared catalog the SkillTool and the /skill router read.
    assert!(
        catalog.find(MCP_SKILL).is_some(),
        "an MCP-served skill is unreachable in a json-stream session: '{MCP_SKILL}' \
         never entered the live skill catalog after the deferred connect"
    );
    // Limb 2: the listing the model discovers skills from. A catalog entry the
    // prompt never mentions is a skill the model cannot choose.
    assert!(
        engine.system_prompt().contains(MCP_SKILL),
        "an MCP-served skill is invisible to the model: '{MCP_SKILL}' is missing \
         from the system prompt's available-skills listing"
    );
    // Limb 3: a listed skill the SkillTool cannot activate is worse than an
    // unlisted one — the model picks it and the turn errors.
    let body = catalog
        .resolve_for_model(MCP_SKILL)
        .await
        .expect("a late-bound MCP skill must be activatable, not just listed");
    assert!(
        body.content.contains("Body."),
        "the resolved MCP skill must carry the body the server served"
    );
    assert!(
        engine.system_prompt().starts_with("BASE PROMPT"),
        "the rebind must replace only the skills block, not the prompt"
    );
    assert_eq!(
        engine
            .system_prompt()
            .matches("The following skills are available")
            .count(),
        1,
        "the rebind must swap the skills block, not append a second one"
    );
}

#[tokio::test]
async fn late_bind_is_inert_when_the_server_serves_no_skills() {
    let cwd = tempfile::tempdir().unwrap();
    let (mut engine, _catalog) = deferred_boot(cwd.path()).await;
    let before = engine.system_prompt().to_string();

    let manager = Arc::new(McpManager::new_for_test(vec![(
        "tools-only",
        false,
        Box::new(SilentTransport) as Box<dyn McpTransport>,
    )]));
    let outcome = binder_for(cwd.path(), HashMap::new())
        .bind(&mut engine, manager)
        .await;

    assert!(outcome.is_empty(), "nothing was served, nothing may change");
    assert_eq!(
        engine.system_prompt(),
        before,
        "an unchanged catalog must leave the cached prompt prefix byte-identical"
    );
}

#[tokio::test]
async fn deferred_config_mcp_binds_the_plugin_hook_dispatcher() {
    let cwd = tempfile::tempdir().unwrap();
    let (mut engine, _catalog) = deferred_boot(cwd.path()).await;

    assert!(
        engine.hook_dispatcher_identity().is_none(),
        "fixture is vacuous: no dispatcher may be installed before the late bind"
    );

    let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
        "notes-mcp",
        false,
        Box::new(SilentTransport) as Box<dyn McpTransport>,
        vec![tool("on_session_start")],
    )]));
    let hooks = HashMap::from([("notes".to_string(), vec!["on_session_start".to_string()])]);
    let outcome = binder_for(cwd.path(), hooks)
        .bind(&mut engine, manager)
        .await;

    assert_eq!(
        outcome.hook_plugins_bound,
        vec!["notes".to_string()],
        "a plugin hook whose tool the deferred server advertises must bind"
    );
    assert_eq!(
        engine.hook_dispatcher_identity(),
        Some("wcore_agent::hooks::mcp_dispatcher::McpHookDispatcher"),
        "the plugin's lifecycle hooks stay log-only: no MCP hook dispatcher was \
         installed on the engine after the deferred config server connected"
    );
}

/// F5/F6 — the ambiguity gate must decide over the FULL manager set. Resolving
/// against the boot managers alone (the deferral defect) would bind `notes`
/// to the plugin server and never see the collision.
#[tokio::test]
async fn late_bind_respects_the_two_server_ambiguity_gate() {
    let cwd = tempfile::tempdir().unwrap();
    let (mut engine, _catalog) = deferred_boot(cwd.path()).await;

    let boot_manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
        "plugin-mcp",
        false,
        Box::new(SilentTransport) as Box<dyn McpTransport>,
        vec![tool("on_session_start")],
    )]));
    let late_manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
        "config-mcp",
        false,
        Box::new(SilentTransport) as Box<dyn McpTransport>,
        vec![tool("on_session_start")],
    )]));

    let binder = LateMcpBinder::new(
        cwd.path().to_path_buf(),
        vec![],
        Arc::new(wcore_skills::bundled::init_bundled_skills()),
        None,
        HashMap::from([("notes".to_string(), vec!["on_session_start".to_string()])]),
        vec![boot_manager],
        true,
    );
    let outcome = binder.bind(&mut engine, late_manager).await;

    assert!(
        outcome.hook_plugins_bound.is_empty(),
        "two servers advertise the hook tool: the binding is hijackable and must \
         be refused, not resolved against the boot manager set alone"
    );
    assert!(
        engine.hook_dispatcher_identity().is_none(),
        "an ambiguous binding must leave plugin hooks log-only"
    );
}

/// The operator switch still governs the late path: `hooks.dispatch_enabled =
/// false` must not acquire a dispatcher just because a server arrived late.
#[tokio::test]
async fn late_bind_honours_the_hook_dispatch_switch() {
    let cwd = tempfile::tempdir().unwrap();
    let (mut engine, _catalog) = deferred_boot(cwd.path()).await;

    let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
        "notes-mcp",
        false,
        Box::new(SilentTransport) as Box<dyn McpTransport>,
        vec![tool("on_session_start")],
    )]));
    let binder = LateMcpBinder::new(
        cwd.path().to_path_buf(),
        vec![],
        Arc::new(wcore_skills::bundled::init_bundled_skills()),
        None,
        HashMap::from([("notes".to_string(), vec!["on_session_start".to_string()])]),
        vec![],
        false,
    );
    let outcome = binder.bind(&mut engine, manager).await;

    assert!(outcome.hook_plugins_bound.is_empty());
    assert!(
        engine.hook_dispatcher_identity().is_none(),
        "dispatch_enabled = false must hold on the late path too"
    );
}
