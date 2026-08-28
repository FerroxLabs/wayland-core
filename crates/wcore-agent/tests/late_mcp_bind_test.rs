//! wayland#562 — the deferred config-MCP path must LATE-BIND, not just
//! late-register tools.
//!
//! `AgentBootstrap::defer_config_mcp(true)` (json-stream, wayland#551) skips
//! the config MCP connect inside `build()`. Tool registration into the live
//! engine was already handled. These tests cover the two consumers that were
//! not, and that this issue exists to close:
//!
//! 1. `skill://` resources served by that MCP server — never loaded, because
//!    `load_catalog(.., mcp_manager)` runs at boot with `None`.
//! 2. The plugin hook dispatcher — resolved once at boot, so a hook that
//!    should bind to the config server never bound, and the F5/F6 ambiguity
//!    gate evaluated without that server present.
//!
//! HOW THESE FAIL IF THE DEFECT RETURNS: gut `LateMcpBinder::bind` back to a
//! no-op (`LateBindReport::default()`) and every assertion below fails by
//! NAME — "late-bound MCP skill missing from the live catalog", "system
//! prompt never told the model", "hook dispatcher was never installed" — not
//! by a compile error.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use wcore_agent::late_mcp::LateMcpBinder;
use wcore_agent::plugins::runner::PluginHook;
use wcore_mcp::manager::McpManager;
use wcore_mcp::protocol::{JsonRpcRequest, JsonRpcResponse, McpToolDef};
use wcore_mcp::transport::{McpError, McpTransport};
use wcore_plugin_api::registry::hooks::HookPhase;
use wcore_skills::refs::{SkillCatalog, SkillRef};
use wcore_skills::types::{LoadedFrom, SkillSource};

/// Replays a fixed JSON-RPC script: first `resources/list`, then one
/// `resources/read` per listed skill. Mirrors the mock in
/// `wcore_skills::mcp`'s own tests.
struct ScriptedTransport {
    responses: Mutex<Vec<serde_json::Value>>,
}

impl ScriptedTransport {
    fn serving_skill(uri: &str, body: &str) -> Self {
        Self {
            responses: Mutex::new(vec![
                serde_json::json!({ "resources": [{ "uri": uri }] }),
                serde_json::json!({
                    "contents": [{ "uri": uri, "mimeType": "text/plain", "text": body }]
                }),
            ]),
        }
    }

    fn serving_nothing() -> Self {
        Self {
            responses: Mutex::new(vec![serde_json::json!({ "resources": [] })]),
        }
    }
}

#[async_trait]
impl McpTransport for ScriptedTransport {
    async fn request(&self, _req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let mut guard = self.responses.lock().expect("scripted transport lock");
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

fn tool(name: &str) -> McpToolDef {
    McpToolDef {
        name: name.to_string(),
        description: Some(format!("{name} tool")),
        input_schema: serde_json::json!({ "type": "object" }),
    }
}

const SKILL_BODY: &str =
    "---\nname: remote-helper\ndescription: RESOURCE_SERVED_SKILL\n---\n\nbody\n";

/// #1150: a description long enough that an 80-character skills budget must
/// visibly shorten the rendered block, while a 40,000-character one keeps it.
const LONG_SKILL_BODY: &str = "---\nname: remote-helper\ndescription: \
LATE_BOUND_DESCRIPTION_MARKER a deliberately long description whose only job is \
to be far wider than the eighty characters a two-thousand token context window \
buys, so the two budget arms cannot render the same bytes\n---\n\nbody\n";

fn local_ref(name: &str) -> SkillRef {
    SkillRef {
        name: name.to_string(),
        display_name: None,
        description: format!("{name} description"),
        when_to_use: None,
        paths: Vec::new(),
        source: SkillSource::Project,
        loaded_from: LoadedFrom::Skills,
        file_path: std::path::PathBuf::from(format!("<virtual:{name}>")),
        skill_root: None,
        content_length_hint: 0,
        user_invocable: true,
        disable_model_invocation: false,
        has_artifacts: false,
        inline_content: Some(format!(
            "---\nname: {name}\ndescription: local\n---\nbody\n"
        )),
    }
}

fn engine_with_catalog(
    catalog: Arc<SkillCatalog>,
) -> (
    wcore_agent::engine::AgentEngine,
    wcore_agent::test_utils::TestSinkHandle,
) {
    let (mut engine, sink) = wcore_agent::bootstrap::AgentBootstrap::build_for_test(
        wcore_config::config::Config::default(),
        vec![],
    );
    engine.set_skill_catalog(catalog);
    (engine, sink)
}

/// Gap 1. A config MCP server that finishes connecting AFTER boot must have
/// its `skill://` resources land in the LIVE catalog and in the system prompt
/// the model reads. Before wayland#562 a json-stream session lost them for
/// the whole session while the (non-deferred) TUI kept them.
#[tokio::test]
async fn late_config_mcp_skills_reach_the_live_catalog_and_prompt() {
    let catalog = Arc::new(SkillCatalog::from_refs(vec![local_ref("local-only")]));
    let (mut engine, _sink) = engine_with_catalog(Arc::clone(&catalog));

    // Boot state: no MCP manager at all — exactly what deferral produces.
    let mut binder = LateMcpBinder::new(Arc::clone(&catalog), &[], Vec::new(), true);

    assert!(
        catalog.find("late-srv:remote-helper").is_none(),
        "precondition: the MCP skill must be absent before the late connect"
    );
    let prompt_before = engine.system_prompt().to_string();
    assert!(
        !prompt_before.contains("RESOURCE_SERVED_SKILL"),
        "precondition: the boot prompt cannot already mention the MCP skill"
    );

    let mgr = Arc::new(McpManager::new_for_test(vec![(
        "late-srv",
        true,
        Box::new(ScriptedTransport::serving_skill(
            "skill://remote-helper",
            SKILL_BODY,
        )) as Box<dyn McpTransport>,
    )]));

    let refs = LateMcpBinder::skill_refs_for(&mgr).await;
    assert_eq!(
        refs.len(),
        1,
        "the scripted server serves exactly one skill:// resource; got {:?}",
        refs.iter().map(|r| r.name.clone()).collect::<Vec<_>>()
    );

    let report = binder.bind(&mut engine, mgr, refs);

    assert_eq!(
        report.skills_added,
        vec!["late-srv:remote-helper".to_string()],
        "late-bound MCP skill missing from the live catalog"
    );
    assert!(
        catalog.find("late-srv:remote-helper").is_some(),
        "late-bound MCP skill missing from the live catalog (the Arc the engine, \
         SkillTool and the skill router all share)"
    );
    assert!(
        catalog.find("local-only").is_some(),
        "merging must not drop skills that were already in the catalog"
    );
    assert!(
        report.prompt_updated,
        "system prompt never told the model about the late-bound skill"
    );
    let prompt_after = engine.system_prompt();
    assert!(
        prompt_after.contains("late-srv:remote-helper")
            && prompt_after.contains("RESOURCE_SERVED_SKILL"),
        "system prompt never told the model about the late-bound skill; prompt = {prompt_after}"
    );
}

/// A late server must never displace a skill the session already listed. The
/// model has already been told what that name means; silently re-pointing it
/// mid-session is worse than the late server losing the collision.
#[tokio::test]
async fn late_bind_never_displaces_an_existing_skill_name() {
    let existing = local_ref("late-srv:remote-helper");
    let catalog = Arc::new(SkillCatalog::from_refs(vec![existing]));
    let (mut engine, _sink) = engine_with_catalog(Arc::clone(&catalog));
    let mut binder = LateMcpBinder::new(Arc::clone(&catalog), &[], Vec::new(), true);

    let mgr = Arc::new(McpManager::new_for_test(vec![(
        "late-srv",
        true,
        Box::new(ScriptedTransport::serving_skill(
            "skill://remote-helper",
            SKILL_BODY,
        )) as Box<dyn McpTransport>,
    )]));

    let refs = LateMcpBinder::skill_refs_for(&mgr).await;
    let report = binder.bind(&mut engine, mgr, refs);

    assert!(
        report.skills_added.is_empty(),
        "a colliding late skill must be skipped, got {:?}",
        report.skills_added
    );
    assert_eq!(
        catalog.len(),
        1,
        "the collision must not duplicate the name in the catalog"
    );
    assert_eq!(
        catalog
            .find("late-srv:remote-helper")
            .expect("name still present")
            .description,
        "late-srv:remote-helper description",
        "the pre-existing skill must survive the collision unchanged"
    );
    assert!(
        !report.prompt_updated,
        "nothing was added, so nothing may be appended to the system prompt"
    );
}

/// Gap 2. A plugin hook whose tool is advertised by a DEFERRED config server
/// must bind once that server connects. With deferral the boot-time resolve
/// saw no managers at all, so the dispatcher was never installed.
#[tokio::test]
async fn late_config_mcp_binds_the_plugin_hook_dispatcher() {
    let catalog = Arc::new(SkillCatalog::from_refs(Vec::new()));
    let (mut engine, _sink) = engine_with_catalog(Arc::clone(&catalog));
    let hooks = vec![PluginHook {
        plugin: "demo-plugin".to_string(),
        phase: HookPhase::SessionStart,
        name: "demo_contribution".to_string(),
    }];
    engine.register_plugin_hooks(hooks.clone());

    assert!(
        !engine
            .hook_engine()
            .expect("bootstrap installs a HookEngine")
            .has_dispatcher(),
        "precondition: deferral leaves the boot resolve with no managers, so no dispatcher"
    );

    let mut binder = LateMcpBinder::new(Arc::clone(&catalog), &hooks, Vec::new(), true);
    let mgr = Arc::new(McpManager::new_for_test_with_tools(vec![(
        "late-srv",
        false,
        Box::new(ScriptedTransport::serving_nothing()) as Box<dyn McpTransport>,
        vec![tool("demo_contribution")],
    )]));

    let report = binder.bind(&mut engine, mgr, Vec::new());

    assert!(report.hooks_rewired, "hook resolution must have re-run");
    assert_eq!(
        report.hook_bindings.get("demo-plugin").map(String::as_str),
        Some("late-srv"),
        "the plugin must bind to the late config server; got {:?}",
        report.hook_bindings
    );
    assert!(
        engine.hook_engine().expect("HookEngine").has_dispatcher(),
        "hook dispatcher was never installed on the engine after the late connect"
    );
}

/// The F5/F6 ambiguity gate must be re-evaluated WITH the late server. A
/// binding that was unique at boot and becomes two-server ambiguous once the
/// deferred server arrives has to be UNBOUND, not left pointing at the
/// boot-time answer.
#[tokio::test]
async fn late_config_mcp_unbinds_a_newly_ambiguous_hook() {
    let catalog = Arc::new(SkillCatalog::from_refs(Vec::new()));
    let (mut engine, _sink) = engine_with_catalog(Arc::clone(&catalog));
    let hooks = vec![PluginHook {
        plugin: "demo-plugin".to_string(),
        phase: HookPhase::SessionStart,
        name: "demo_contribution".to_string(),
    }];
    engine.register_plugin_hooks(hooks.clone());

    // Boot: a plugin MCP server uniquely advertises the hook tool, so boot
    // bound it and installed a dispatcher.
    let plugin_mgr = Arc::new(McpManager::new_for_test_with_tools(vec![(
        "plugin-srv",
        false,
        Box::new(ScriptedTransport::serving_nothing()) as Box<dyn McpTransport>,
        vec![tool("demo_contribution")],
    )]));
    let mut binder = LateMcpBinder::new(
        Arc::clone(&catalog),
        &hooks,
        vec![Arc::clone(&plugin_mgr)],
        true,
    );
    engine.set_hook_dispatcher(Arc::new(wcore_agent::hooks::McpHookDispatcher::new(
        Arc::new(wcore_agent::hooks::McpManagerCaller::new(vec![Arc::clone(
            &plugin_mgr,
        )])),
        std::collections::HashMap::from([("demo-plugin".to_string(), "plugin-srv".to_string())]),
    )));
    assert!(
        engine.hook_engine().expect("HookEngine").has_dispatcher(),
        "precondition: boot bound the plugin to its own MCP server"
    );

    // The deferred config server advertises a tool with the SAME name.
    let config_mgr = Arc::new(McpManager::new_for_test_with_tools(vec![(
        "config-srv",
        false,
        Box::new(ScriptedTransport::serving_nothing()) as Box<dyn McpTransport>,
        vec![tool("demo_contribution")],
    )]));

    let report = binder.bind(&mut engine, config_mgr, Vec::new());

    assert!(
        report.hook_bindings.is_empty(),
        "two servers advertise the hook tool, so the gate must refuse to bind; got {:?}",
        report.hook_bindings
    );
    assert!(
        !engine.hook_engine().expect("HookEngine").has_dispatcher(),
        "the newly ambiguous binding must be REMOVED, not left on the boot-time answer"
    );
}

/// #1150 — the late-bind listing must be sized against the ACTIVE model's
/// window, exactly like the boot listing bootstrap renders.
///
/// `get_char_budget` is 1% of the window in characters, so a 2,000-token window
/// buys 80 characters of listing and a 1,000,000-token window buys 40,000. Two
/// otherwise-identical sessions differing only in that number must therefore
/// append different blocks. Under the defect this call site passed a hardcoded
/// `None` — matching the bootstrap call site, which also passed `None` — so
/// both sessions got the flat 8,000-character `DEFAULT_CHAR_BUDGET` and the two
/// blocks came out identical.
///
/// HOW THIS FAILS IF THE DEFECT RETURNS: put `None` back in
/// `late_mcp.rs`'s `format_skills_section` call. It compiles clean and this is
/// the only test that goes red.
#[tokio::test]
async fn late_bound_listing_is_sized_by_the_active_models_window() {
    async fn block_for(context_window: usize) -> String {
        let catalog = Arc::new(SkillCatalog::from_refs(Vec::new()));
        let mut config = wcore_config::config::Config::default();
        config.compact.context_window = Some(context_window);
        let (mut engine, _sink) =
            wcore_agent::bootstrap::AgentBootstrap::build_for_test(config, vec![]);
        engine.set_skill_catalog(Arc::clone(&catalog));
        let before = engine.system_prompt().len();

        let mut binder = LateMcpBinder::new(Arc::clone(&catalog), &[], Vec::new(), true);
        let mgr = Arc::new(McpManager::new_for_test(vec![(
            "late-srv",
            true,
            Box::new(ScriptedTransport::serving_skill(
                "skill://remote-helper",
                LONG_SKILL_BODY,
            )) as Box<dyn McpTransport>,
        )]));
        let refs = LateMcpBinder::skill_refs_for(&mgr).await;
        let report = binder.bind(&mut engine, mgr, refs);
        assert!(
            report.prompt_updated,
            "precondition: the late skill must reach the prompt for its budget to matter"
        );
        engine.system_prompt()[before..].to_string()
    }

    let roomy = block_for(1_000_000).await;
    let tight = block_for(2_000).await;

    assert!(
        roomy.contains("LATE_BOUND_DESCRIPTION_MARKER"),
        "precondition: a 40,000-char budget must keep the description in full: {roomy}"
    );
    assert!(
        tight.len() < roomy.len(),
        "an 80-char budget must render a SHORTER block than a 40,000-char one; equal \
         lengths mean this call site is still passing `None` (tight = {} bytes, roomy = \
         {} bytes)",
        tight.len(),
        roomy.len()
    );
}
