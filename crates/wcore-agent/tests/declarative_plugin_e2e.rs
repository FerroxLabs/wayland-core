//! Path B step 1 — declarative on-disk plugin end-to-end.
//!
//! Proves the full data flow the blueprint specifies, WITHOUT touching the C1
//! dispatcher / apply / runner internals:
//!
//!   declarative plugin.toml
//!     → `PluginLoader::discover_on_disk` → `LoadedRuntimeHandle::Declarative`
//!     → merged into `InitializeOutcome.hooks` / `.mcp_servers`
//!     → `apply_initialize_outcome` → `applied.plugin_hooks` / `.plugin_mcp_servers`
//!     → `resolve_server_for_plugin` binds plugin→server
//!     → `McpHookDispatcher` fires the hook → returns the MCP tool's text.

use serial_test::serial;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, OnceLock};

use async_trait::async_trait;
use wcore_agent::hooks::HookDispatcher;
use wcore_agent::hooks::mcp_dispatcher::{
    McpHookDispatcher, McpToolCaller, resolve_server_for_plugin,
};
use wcore_agent::plugins::loader::PluginLoader;
use wcore_agent::plugins::runner::PluginRunner;
use wcore_agent::plugins::{InitializeOutcome, LoadedRuntimeHandle, apply_initialize_outcome};
use wcore_config::plugins_config::PluginsConfig;
use wcore_plugin_api::PluginAccessGate;
use wcore_plugin_api::registry::hooks::HookPhase;
use wcore_tools::registry::ToolRegistry;

// --- env-mutation serialization (mirrors plugin_on_disk_discovery.rs) -------

fn env_mutex() -> &'static Mutex<()> {
    static M: OnceLock<Mutex<()>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(()))
}

struct EnvGuard {
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl EnvGuard {
    fn set(dir: &Path) -> Self {
        let lock = env_mutex().lock().unwrap_or_else(|p| p.into_inner());
        unsafe {
            std::env::set_var("WAYLAND_PLUGINS_DIR", dir);
            std::env::set_var("WAYLAND_PLUGIN_TRUST_UNSIGNED", "1");
        }
        Self { _lock: lock }
    }
}
impl Drop for EnvGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var("WAYLAND_PLUGINS_DIR");
            std::env::remove_var("WAYLAND_PLUGIN_TRUST_UNSIGNED");
        }
    }
}

fn make_plugin_dir(root: &Path, name: &str, manifest_toml: &str) -> std::path::PathBuf {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let manifest = dir.join("plugin.toml");
    std::fs::write(&manifest, manifest_toml).unwrap();
    manifest
}

fn config() -> PluginsConfig {
    PluginsConfig {
        plugin_signature_verification: false,
        ..Default::default()
    }
}

/// Run on-disk discovery against the given plugins root and merge the
/// declarative handle into a fresh `InitializeOutcome` exactly as bootstrap
/// does, then apply it. Returns the applied outcome's hooks + mcp servers.
async fn discover_and_apply(
    root: &Path,
) -> (
    Vec<wcore_agent::plugins::runner::PluginHook>,
    Vec<wcore_plugin_api::McpServerSpec>,
) {
    let _guard = EnvGuard::set(root);
    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);
    loader.discover_on_disk(&runner, None, gate).await;

    let mut outcome = InitializeOutcome::default();
    for record in loader.take_on_disk_dispatches() {
        if record.load_result.is_err() {
            continue;
        }
        if let LoadedRuntimeHandle::Declarative { hooks, mcp_server } = record.handle {
            outcome.hooks.extend(hooks);
            if let Some(spec) = mcp_server {
                outcome.mcp_servers.push(spec);
            }
        }
    }

    let mut registry = ToolRegistry::new();
    let applied = apply_initialize_outcome(
        outcome,
        &mut registry,
        wcore_agent::plugins::adapters::browser_adapter::HostBrowserRegistrar::default(),
        wcore_agent::plugins::adapters::cua_adapter::HostCuaRegistrar::default(),
    );
    (applied.plugin_hooks, applied.plugin_mcp_servers)
}

/// T12 — integration: a declarative plugin's hooks + mcp_server reach the
/// applied outcome through the real on-disk discovery + apply pipeline.
#[serial]
#[tokio::test]
async fn declarative_plugin_flows_into_applied_outcome() {
    let tmp = tempfile::TempDir::new().unwrap();
    let manifest_toml = r#"
[plugin]
name = "memory-plug"
version = "0.0.0"
description = "declarative memory plugin"
license = "Apache-2.0"

[permissions]
register_hooks = true
register_mcp_server = true

[runtime]
kind = "declarative"

[[hooks]]
phase = "session_start"
tool = "memory_prelude"

[mcp_server]
name = "memory-plug-server"

[mcp_server.transport]
kind = "sse"
url = "https://example.invalid/sse"
"#;
    make_plugin_dir(tmp.path(), "memory-plug", manifest_toml);

    let (hooks, servers) = discover_and_apply(tmp.path()).await;

    assert_eq!(
        hooks.len(),
        1,
        "declared hook must reach applied.plugin_hooks"
    );
    assert_eq!(hooks[0].plugin, "memory-plug");
    assert_eq!(hooks[0].phase, HookPhase::SessionStart);
    assert_eq!(hooks[0].name, "memory_prelude");

    assert_eq!(
        servers.len(),
        1,
        "declared mcp_server must reach applied.plugin_mcp_servers"
    );
    assert_eq!(servers[0].name, "memory-plug-server");
}

// --- T13 e2e: declarative plugin → C1 dispatch returns the MCP tool text ----

/// Fake `McpToolCaller`: returns canned text for one `(server, tool)` pair.
struct FakeCaller {
    server: String,
    tool: String,
    text: String,
}

#[async_trait]
impl McpToolCaller for FakeCaller {
    async fn call(&self, server: &str, tool: &str) -> Result<String, String> {
        if server == self.server && tool == self.tool {
            Ok(self.text.clone())
        } else {
            Err(format!("no such tool {server}/{tool}"))
        }
    }
}

#[serial]
#[tokio::test]
async fn declarative_plugin_hook_dispatches_through_c1() {
    let tmp = tempfile::TempDir::new().unwrap();
    let manifest_toml = r#"
[plugin]
name = "memory-plug"
version = "0.0.0"
description = "declarative memory plugin"
license = "Apache-2.0"

[permissions]
register_hooks = true
register_mcp_server = true

[runtime]
kind = "declarative"

[[hooks]]
phase = "session_start"
tool = "memory_prelude"

[mcp_server]
name = "memory-plug-server"

[mcp_server.transport]
kind = "sse"
url = "https://example.invalid/sse"
"#;
    make_plugin_dir(tmp.path(), "memory-plug", manifest_toml);

    let (hooks, _servers) = discover_and_apply(tmp.path()).await;

    // Build the `plugin -> [hook tool names]` view the C1 binder consumes.
    let mut hooks_by_plugin: HashMap<&str, Vec<&str>> = HashMap::new();
    for h in &hooks {
        hooks_by_plugin
            .entry(h.plugin.as_str())
            .or_default()
            .push(h.name.as_str());
    }

    // The connected MCP server advertises the same tool name as the hook.
    let servers: Vec<(&str, Vec<&str>)> = vec![("memory-plug-server", vec!["memory_prelude"])];
    let binding = resolve_server_for_plugin(&hooks_by_plugin, &servers);
    assert_eq!(
        binding.get("memory-plug").map(String::as_str),
        Some("memory-plug-server"),
        "C1 binder must bind the declarative plugin to its server"
    );

    // Drive the real C1 dispatcher with a fake caller; it must return the
    // server tool's text as the hook contribution.
    let caller = Arc::new(FakeCaller {
        server: "memory-plug-server".into(),
        tool: "memory_prelude".into(),
        text: "DECLARATIVE-PRELUDE".into(),
    });
    let dispatcher = McpHookDispatcher::new(caller, binding);
    let out = dispatcher
        .dispatch("memory-plug", "memory_prelude", HookPhase::SessionStart)
        .await;
    assert_eq!(out.as_deref(), Some("DECLARATIVE-PRELUDE"));
}

// --- T14 security: declarative plugin outside allowed roots → load Err ------

#[cfg(unix)]
#[serial]
#[tokio::test]
async fn declarative_plugin_outside_roots_via_symlink_skipped() {
    use std::os::unix::fs::symlink;

    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    // Real plugin dir lives OUTSIDE the plugins root.
    let outside = tempfile::TempDir::new().unwrap();
    let real_dir = outside.path().join("attacker-controlled");
    std::fs::create_dir_all(&real_dir).unwrap();
    let manifest_toml = r#"
[plugin]
name = "attacker"
version = "0.0.0"
description = "fixture"
license = "Apache-2.0"

[permissions]
register_hooks = true

[runtime]
kind = "declarative"

[[hooks]]
phase = "session_start"
tool = "steal"
"#;
    std::fs::write(real_dir.join("plugin.toml"), manifest_toml).unwrap();
    // Symlink under the plugins root pointing at the attacker dir.
    symlink(&real_dir, tmp.path().join("attacker")).unwrap();

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);
    loader.discover_on_disk(&runner, None, gate).await;

    let recs = loader.on_disk_dispatches();
    assert!(
        recs.is_empty(),
        "declarative plugin reached via a symlink out of the plugins root must be skipped, got: {recs:?}"
    );
}

// --- T15 security: [mcp_server] without register_mcp_server → load Err -------

#[serial]
#[tokio::test]
async fn declarative_mcp_server_without_permission_rejected() {
    let tmp = tempfile::TempDir::new().unwrap();
    let _guard = EnvGuard::set(tmp.path());

    let manifest_toml = r#"
[plugin]
name = "mcp-nogrant"
version = "0.0.0"
description = "fixture"
license = "Apache-2.0"

[runtime]
kind = "declarative"

[mcp_server]
name = "rogue"

[mcp_server.transport]
kind = "stdio"
command = "npx"
args = []
"#;
    make_plugin_dir(tmp.path(), "mcp-nogrant", manifest_toml);

    let config = config();
    let mut loader = PluginLoader::discover(&config);
    let runner = PluginRunner::new();
    let gate = Arc::new(PluginAccessGate);
    loader.discover_on_disk(&runner, None, gate).await;

    let recs = loader.on_disk_dispatches();
    assert_eq!(recs.len(), 1);
    let r = &recs[0];
    // The manifest is rejected at PARSE time (validate gate), before
    // `classify_runtime` runs — so the error record carries the default
    // `Static` dispatch. The load_result error is the real signal.
    let err = r
        .load_result
        .as_ref()
        .expect_err("mcp_server without register_mcp_server must be rejected");
    assert!(
        err.contains("parse manifest") || err.contains("register_mcp_server"),
        "expected manifest-validation rejection, got: {err}"
    );
}
