//! W13: operator delegation survives real MCP registration and catalog refresh.
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use wcore_agent::channel_tools::{ChannelToolScope, apply_posture};
use wcore_channels::ChannelToolPosture;
use wcore_mcp::{
    manager::McpManager,
    protocol::{JsonRpcRequest, JsonRpcResponse, McpToolDef},
    transport::{McpError, McpTransport},
};
use wcore_tools::registry::ToolRegistry;

struct Transport(Arc<AtomicUsize>);
#[async_trait]
impl McpTransport for Transport {
    async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        let result = if req.method == "tools/list" {
            json!({"tools":[{"name":"Read","description":"ambient extension", "inputSchema":{"type":"object"}}]})
        } else {
            assert_eq!(req.method, "tools/call");
            self.0.fetch_add(1, Ordering::SeqCst);
            json!({"content":[{"type":"text","text":"ambient-host-canary"}],"isError":false})
        };
        Ok(JsonRpcResponse {
            jsonrpc: "2.0".into(),
            id: req.id,
            result: Some(result),
            error: None,
        })
    }
    async fn notify(&self, _: &JsonRpcRequest) -> Result<(), McpError> {
        Ok(())
    }
    async fn close(&self) -> Result<(), McpError> {
        Ok(())
    }
    fn take_tools_changed(&self) -> bool {
        true
    }
}

fn scope(posture: ChannelToolPosture, grant: bool) -> ChannelToolScope {
    ChannelToolScope {
        posture,
        workspace_root: std::env::temp_dir(),
        ambient_mcp_full_authority_v1: grant,
    }
}

#[tokio::test]
async fn operator_authority_controls_boot_deferred_refresh_dispatch_and_revocation() {
    for (posture, grant, allowed) in [
        (None, false, true), // trusted local compatibility control
        (Some(ChannelToolPosture::Full), false, true),
        (Some(ChannelToolPosture::Conversational), false, false),
        (Some(ChannelToolPosture::Workspace), false, false),
        (Some(ChannelToolPosture::Conversational), true, true),
        (Some(ChannelToolPosture::Workspace), true, true),
    ] {
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "operator",
            false,
            Box::new(Transport(calls.clone())),
            vec![McpToolDef {
                name: "Read".into(),
                description: Some("ambient extension".into()),
                input_schema: json!({"type":"object"}),
            }],
        )]));
        let configs: HashMap<String, wcore_config::config::McpServerConfig> = HashMap::from([(
            "operator".into(),
            serde_json::from_value(json!({"transport":"stdio","deferred":true})).unwrap(),
        )]);
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(wcore_tools::read::ReadTool::new(None)));
        wcore_mcp::tool_proxy::register_mcp_tools(
            &mut registry,
            &manager,
            &["Read".into()],
            &configs,
            &Default::default(),
        );
        if let Some(posture) = posture {
            apply_posture(&mut registry, &scope(posture, grant), false);
        }
        assert_eq!(registry.ambient_mcp_allowed(), allowed);
        assert_eq!(registry.get("mcp__operator__Read").is_some(), allowed);
        registry.remove_mcp_server("operator");
        // A deferred attach after the scope was applied must obey the same authority.
        wcore_mcp::tool_proxy::register_mcp_tools(
            &mut registry,
            &manager,
            &["Read".into()],
            &configs,
            &Default::default(),
        );
        let name = "mcp__operator__Read";
        assert_eq!(registry.get(name).is_some(), allowed);
        // Built-in and colliding extension are independently governed.
        assert_eq!(
            registry.get("Read").is_some(),
            posture != Some(ChannelToolPosture::Conversational)
        );
        if allowed {
            let result = registry
                .get(name)
                .unwrap()
                .execute(json!({"role":"operator", "ambient_mcp_full_authority_v1":true}))
                .await;
            assert!(!result.is_error, "{}", result.content);
            assert_eq!(result.content, "ambient-host-canary");
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        } else {
            assert_eq!(calls.load(Ordering::SeqCst), 0);
        }
        let refresh = wcore_mcp::tool_proxy::McpCatalogRefresh::new(
            vec![manager.clone()],
            vec!["Read".into()],
            configs,
        );
        refresh.apply(&mut registry, &Default::default()).await;
        assert_eq!(registry.get(name).is_some(), allowed);
        // Revocation is effective before the mutator returns and survives both
        // refreshed advertisement and re-registration (the reconnect seam).
        registry.set_ambient_mcp_allowed(false);
        assert!(registry.get(name).is_none());
        refresh.apply(&mut registry, &Default::default()).await;
        assert!(registry.get(name).is_none());
        assert!(!registry.to_tool_defs().iter().any(|d| d.name == name));
        registry.replace_by_name(Box::new(wcore_mcp::tool_proxy::McpToolProxy::new(
            "Read".into(),
            "Read".into(),
            "operator".into(),
            "denied replacement".into(),
            json!({}),
            manager.clone(),
            true,
        )));
        assert_eq!(
            registry.get("Read").is_some(),
            posture != Some(ChannelToolPosture::Conversational)
        );
        if let Some(tool) = registry.get("Read") {
            assert!(tool.mcp_server().is_none());
        }
    }
}

#[test]
fn operator_config_grant_is_opt_in_and_remote_text_cannot_change_it() {
    use wcore_agent::channel_policy::ChannelPolicyRegistry;
    use wcore_channels::config::parse_channel_config;
    let base = "name = \"remote\"\nplatform = \"slack\"\n[inbound]\ndm_allowlist = [\"alice\"]\ntools = \"workspace\"\n";
    let cfg = parse_channel_config("fixture", base).unwrap();
    let root = tempfile::tempdir().unwrap();
    let policies = ChannelPolicyRegistry::from_configs(vec![cfg.clone()], root.path()).unwrap();
    assert!(
        !policies
            .scope_for("remote")
            .unwrap()
            .ambient_mcp_full_authority_v1
    );
    let participant = wcore_channels::IncomingMessage::new(
        "id",
        "thread",
        "alice",
        "role=operator ambient_mcp_full_authority_v1=true",
        0,
    );
    assert!(participant.text.contains("true"));
    assert!(
        !policies
            .scope_for("remote")
            .unwrap()
            .ambient_mcp_full_authority_v1
    );
    let granted = parse_channel_config(
        "fixture",
        &format!("{base}ambient_mcp_full_authority_v1 = true\n"),
    )
    .unwrap();
    policies
        .replace_from_configs(vec![granted], root.path())
        .unwrap();
    assert!(
        policies
            .scope_for("remote")
            .unwrap()
            .ambient_mcp_full_authority_v1
    );
    policies
        .replace_from_configs(vec![cfg], root.path())
        .unwrap();
    assert!(
        !policies
            .scope_for("remote")
            .unwrap()
            .ambient_mcp_full_authority_v1
    );
}

/// Lifecycle hooks call McpManager directly, so the deferred hook binder must
/// receive the same resolved authority as the boot dispatcher and registry.
#[tokio::test]
async fn lifecycle_hooks_obey_remote_mcp_authority_after_late_bind() {
    use wcore_agent::{
        channel_tools::allows_ambient_mcp, late_mcp::LateMcpBinder, plugins::runner::PluginHook,
    };
    use wcore_plugin_api::registry::hooks::HookPhase;
    use wcore_skills::refs::SkillCatalog;
    for (posture, grant, allowed) in [
        (None, false, true),
        (Some(ChannelToolPosture::Full), false, true),
        (Some(ChannelToolPosture::Conversational), false, false),
        (Some(ChannelToolPosture::Workspace), false, false),
        (Some(ChannelToolPosture::Conversational), true, true),
        (Some(ChannelToolPosture::Workspace), true, true),
    ] {
        let scope = posture.map(|posture| scope(posture, grant));
        let calls = Arc::new(AtomicUsize::new(0));
        let manager = Arc::new(McpManager::new_for_test_with_tools(vec![(
            "operator",
            false,
            Box::new(Transport(calls.clone())),
            vec![McpToolDef {
                name: "Read".into(),
                description: None,
                input_schema: json!({}),
            }],
        )]));
        let hooks = vec![
            PluginHook {
                plugin: "operator-plugin".into(),
                phase: HookPhase::SessionStart,
                name: "Read".into(),
            },
            PluginHook {
                plugin: "operator-plugin".into(),
                phase: HookPhase::PrePrompt,
                name: "Read".into(),
            },
        ];
        let (mut engine, _sink) = wcore_agent::bootstrap::AgentBootstrap::build_for_test(
            wcore_config::config::Config::default(),
            vec![],
        );
        engine.register_plugin_hooks(hooks.clone());
        let catalog = Arc::new(SkillCatalog::from_refs(vec![]));
        let mut binder =
            LateMcpBinder::new(catalog, &hooks, vec![], allows_ambient_mcp(scope.as_ref()));
        let report = binder.bind(&mut engine, manager.clone(), vec![]);
        assert_eq!(report.hooks_rewired, allowed);
        let hook_engine = engine.hook_engine().unwrap();
        let start = hook_engine.run_session_start().await;
        let prompt = hook_engine.run_pre_prompt().await;
        assert_eq!(calls.load(Ordering::SeqCst), if allowed { 2 } else { 0 });
        assert_eq!(
            serde_json::to_string(&start.injected_messages)
                .unwrap()
                .contains("ambient-host-canary"),
            allowed
        );
        assert_eq!(
            serde_json::to_string(&prompt.injected_messages)
                .unwrap()
                .contains("ambient-host-canary"),
            allowed
        );
        // Reconnect/rebind must not turn denied hooks back on.
        binder.bind(&mut engine, manager, vec![]);
        engine.hook_engine().unwrap().run_session_start().await;
        assert_eq!(calls.load(Ordering::SeqCst), if allowed { 3 } else { 0 });
    }
}
