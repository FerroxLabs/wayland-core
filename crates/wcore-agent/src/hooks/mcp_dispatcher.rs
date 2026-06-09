//! Host-side concrete `HookDispatcher` (C1 / Task A1).
//!
//! `HookEngine` (see `crate::hooks`) knows only the framework-blind
//! `HookDispatcher` trait — it never reaches an `McpManager`. This module
//! supplies the concrete bridge wired at bootstrap: a plugin lifecycle hook
//! NAME (e.g. a `SessionStart` hook) is resolved to an MCP tool of the same
//! name on the plugin's MCP server, and the tool's textual result becomes the
//! hook's contribution.
//!
//! Confirmed signatures (Step 1, read this session):
//! - `wcore_mcp::manager::McpManager::call_tool(&self, server: &str, tool: &str,
//!   args: serde_json::Value) -> Result<String, McpError>` — `&self` shared, no
//!   guard held across the await, so calling through an `Arc<McpManager>` is
//!   safe.
//! - `McpManager::server_names(&self) -> Vec<String>` and
//!   `McpManager::server_is_alive(&self, &str) -> bool`.
//! - `HookDispatcher::dispatch(&self, plugin: &str, hook_name: &str,
//!   phase: HookPhase) -> Option<String>` (crate::hooks).
//! - `HookPhase` lives at `wcore_plugin_api::registry::hooks::HookPhase`.
//!
//! Framework-blind: this file names no provider. The `plugin -> mcp server`
//! association is data passed in at construction (built from registry state by
//! bootstrap), never a hardcoded plugin string.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use wcore_mcp::manager::McpManager;
use wcore_plugin_api::registry::hooks::HookPhase;

use crate::hooks::HookDispatcher;

/// Minimal injectable seam over "call MCP tool `tool` on server `server`".
/// Lets the dispatcher be tested without a live MCP server, and keeps
/// `McpManager` out of the unit-test path.
#[async_trait]
pub trait McpToolCaller: Send + Sync {
    /// Invoke `tool` on `server`. `Err` carries a human-readable reason; the
    /// dispatcher treats every error as "no contribution" (tolerant).
    async fn call(&self, server: &str, tool: &str) -> Result<String, String>;
}

/// Concrete `HookDispatcher` that bridges a plugin-hook name to an MCP tool.
///
/// `server_for_plugin` maps an originating plugin name to the MCP server key
/// that hosts its lifecycle-hook tools. A plugin with no entry contributes
/// nothing (returns `None`), so non-MCP plugins and unmapped plugins are
/// silently inert.
pub struct McpHookDispatcher {
    caller: Arc<dyn McpToolCaller>,
    server_for_plugin: HashMap<String, String>,
}

impl McpHookDispatcher {
    /// Construct from an injected caller and a `plugin -> mcp server` map.
    pub fn new(caller: Arc<dyn McpToolCaller>, server_for_plugin: HashMap<String, String>) -> Self {
        Self {
            caller,
            server_for_plugin,
        }
    }
}

#[async_trait]
impl HookDispatcher for McpHookDispatcher {
    async fn dispatch(&self, plugin: &str, hook_name: &str, _phase: HookPhase) -> Option<String> {
        let server = self.server_for_plugin.get(plugin)?;
        match self.caller.call(server, hook_name).await {
            Ok(text) if !text.trim().is_empty() => Some(text),
            Ok(_) => None,
            Err(e) => {
                tracing::warn!(
                    target: "wcore_agent::hooks",
                    plugin,
                    hook = hook_name,
                    error = %e,
                    "hook MCP dispatch failed; proceeding without injection"
                );
                None
            }
        }
    }
}

/// Production `McpToolCaller` backed by the host's connected MCP managers.
///
/// Holds the `Vec<Arc<McpManager>>` bootstrap already assembles (config-file
/// servers + plugin servers). On each call it finds the first manager whose
/// `server_names()` contains the target server and routes `call_tool` there;
/// `call_tool` itself fast-fails on a dead transport. No lock guard is held
/// across the await — `McpManager::call_tool` takes `&self`.
pub struct McpManagerCaller {
    managers: Vec<Arc<McpManager>>,
}

impl McpManagerCaller {
    pub fn new(managers: Vec<Arc<McpManager>>) -> Self {
        Self { managers }
    }

    /// First manager that knows `server` (regardless of liveness — `call_tool`
    /// enforces the liveness fast-fail and yields a typed error we map to a
    /// tolerant `None` upstream).
    fn manager_for(&self, server: &str) -> Option<&Arc<McpManager>> {
        self.managers
            .iter()
            .find(|m| m.server_names().iter().any(|s| s == server))
    }
}

#[async_trait]
impl McpToolCaller for McpManagerCaller {
    async fn call(&self, server: &str, tool: &str) -> Result<String, String> {
        let manager = self
            .manager_for(server)
            .ok_or_else(|| format!("no connected MCP manager hosts server '{server}'"))?;
        manager
            .call_tool(server, tool, serde_json::json!({}))
            .await
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Fake caller: returns a canned result for one `(server, tool)` pair,
    /// errors for a configured tool, and records call count.
    struct FakeCaller {
        ok_server: String,
        ok_tool: String,
        ok_text: String,
        err_tool: Option<String>,
        calls: AtomicUsize,
    }

    impl FakeCaller {
        fn new(ok_server: &str, ok_tool: &str, ok_text: &str) -> Self {
            Self {
                ok_server: ok_server.to_string(),
                ok_tool: ok_tool.to_string(),
                ok_text: ok_text.to_string(),
                err_tool: None,
                calls: AtomicUsize::new(0),
            }
        }
    }

    #[async_trait]
    impl McpToolCaller for FakeCaller {
        async fn call(&self, server: &str, tool: &str) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            if self.err_tool.as_deref() == Some(tool) {
                return Err("backend exploded".to_string());
            }
            if server == self.ok_server && tool == self.ok_tool {
                Ok(self.ok_text.clone())
            } else {
                // Unknown tool on a known server: empty contribution.
                Ok(String::new())
            }
        }
    }

    fn map_one(plugin: &str, server: &str) -> HashMap<String, String> {
        let mut m = HashMap::new();
        m.insert(plugin.to_string(), server.to_string());
        m
    }

    // A mapped plugin whose hook tool yields text returns that text.
    #[tokio::test]
    async fn known_plugin_returns_tool_text() {
        let caller = Arc::new(FakeCaller::new("memory-server", "context_tool", "PRELUDE"));
        let dispatcher =
            McpHookDispatcher::new(caller.clone(), map_one("plugin-a", "memory-server"));
        let out = dispatcher
            .dispatch("plugin-a", "context_tool", HookPhase::SessionStart)
            .await;
        assert_eq!(out.as_deref(), Some("PRELUDE"));
        assert_eq!(caller.calls.load(Ordering::Relaxed), 1);
    }

    // An unmapped plugin never reaches the caller and returns None.
    #[tokio::test]
    async fn unknown_plugin_returns_none_without_calling() {
        let caller = Arc::new(FakeCaller::new("memory-server", "context_tool", "PRELUDE"));
        let dispatcher =
            McpHookDispatcher::new(caller.clone(), map_one("plugin-a", "memory-server"));
        let out = dispatcher
            .dispatch("some-other-plugin", "context_tool", HookPhase::SessionStart)
            .await;
        assert!(out.is_none());
        assert_eq!(
            caller.calls.load(Ordering::Relaxed),
            0,
            "unmapped plugin must short-circuit before the caller"
        );
    }

    // A caller error is tolerated: dispatch returns None, never propagates.
    #[tokio::test]
    async fn caller_error_returns_none() {
        let caller = Arc::new(FakeCaller {
            ok_server: "memory-server".to_string(),
            ok_tool: "context_tool".to_string(),
            ok_text: "PRELUDE".to_string(),
            err_tool: Some("context_tool".to_string()),
            calls: AtomicUsize::new(0),
        });
        let dispatcher = McpHookDispatcher::new(caller, map_one("plugin-a", "memory-server"));
        let out = dispatcher
            .dispatch("plugin-a", "context_tool", HookPhase::SessionStart)
            .await;
        assert!(out.is_none());
    }

    // An empty / whitespace-only result is "no contribution".
    #[tokio::test]
    async fn whitespace_result_returns_none() {
        let caller = Arc::new(FakeCaller::new("memory-server", "context_tool", "   \n"));
        let dispatcher = McpHookDispatcher::new(caller, map_one("plugin-a", "memory-server"));
        let out = dispatcher
            .dispatch("plugin-a", "context_tool", HookPhase::SessionStart)
            .await;
        assert!(out.is_none());
    }
}
