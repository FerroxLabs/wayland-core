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

/// F5/F6 — resolve the `plugin -> mcp server` binding from registry state,
/// applying the ambiguity guard.
///
/// `hooks_by_plugin` maps each plugin to its registered hook (tool) names.
/// `servers` lists every connected server with the tool names it advertises.
///
/// A plugin binds to a server iff EXACTLY ONE distinct server advertises a tool
/// matching one of that plugin's hook names. If two or more distinct servers
/// match, the binding is ambiguous (nondeterministic and hijackable — a
/// malicious plugin could advertise a tool named like another plugin's hook),
/// so the plugin is left UNBOUND (stays log-only) and a warning names the
/// conflict. A plugin with no match is simply absent from the result.
///
/// Pure and deterministic: no I/O, order-independent over `servers`. Extracted
/// from bootstrap so the binding policy is unit-testable.
pub fn resolve_server_for_plugin<'a>(
    hooks_by_plugin: &'a std::collections::HashMap<&'a str, Vec<&'a str>>,
    servers: &'a [(&'a str, Vec<&'a str>)],
) -> HashMap<String, String> {
    let mut out: HashMap<String, String> = HashMap::new();
    for (plugin, hook_names) in hooks_by_plugin {
        // Distinct servers advertising any of this plugin's hook tool names.
        let mut matching: Vec<&str> = servers
            .iter()
            .filter(|(_, tools)| tools.iter().any(|t| hook_names.iter().any(|hn| hn == t)))
            .map(|(s, _)| *s)
            .collect();
        matching.sort_unstable();
        matching.dedup();

        match matching.as_slice() {
            [] => {
                tracing::debug!(
                    target: "wcore_agent::hooks",
                    plugin = %plugin,
                    "no MCP server advertises this plugin's hook tools; hooks stay log-only"
                );
            }
            [server] => {
                out.insert((*plugin).to_string(), (*server).to_string());
            }
            many => {
                tracing::warn!(
                    target: "wcore_agent::hooks",
                    plugin = %plugin,
                    servers = ?many,
                    "ambiguous plugin->server binding: multiple servers advertise this \
                     plugin's hook tools; refusing to bind (hooks stay log-only)"
                );
            }
        }
    }
    out
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
    /// tolerant `None` upstream). F10: uses `hosts_server` (a no-alloc
    /// `contains_key`) rather than `server_names()` (which clones every key).
    fn manager_for(&self, server: &str) -> Option<&Arc<McpManager>> {
        self.managers.iter().find(|m| m.hosts_server(server))
    }
}

/// F4: hook-contribution-specific ceiling on a raw MCP tool response. The
/// transport already caps a single line at 8 MiB; this bounds the text that a
/// hook contribution propagates and clones downstream so one huge response
/// can't blow up per-turn memory.
const MAX_HOOK_RESPONSE_BYTES: usize = 64 * 1024;

/// Truncate `text` to at most [`MAX_HOOK_RESPONSE_BYTES`], rounded down to a
/// char boundary so multi-byte UTF-8 is never split. Returns the input
/// unchanged when already within the cap (no allocation in the common case).
fn cap_hook_response(text: String) -> String {
    if text.len() <= MAX_HOOK_RESPONSE_BYTES {
        return text;
    }
    let cut = text
        .char_indices()
        .map(|(i, _)| i)
        .take_while(|&i| i <= MAX_HOOK_RESPONSE_BYTES)
        .last()
        .unwrap_or(0);
    let mut t = text;
    t.truncate(cut);
    t
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
            // Hooks consume the text only; the #475 is_error flag is not
            // relevant to the hook-dispatch path.
            .map(|outcome| cap_hook_response(outcome.text))
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

    // F6: the extracted map-build binds an unambiguous match, leaves an
    // unmatched plugin unbound, and refuses an ambiguous (2-server) match.
    #[test]
    fn resolve_server_for_plugin_binding_policy() {
        let mut hooks: HashMap<&str, Vec<&str>> = HashMap::new();
        hooks.insert("mem-plugin", vec!["context_tool"]);
        hooks.insert("lonely-plugin", vec!["nobody_advertises_this"]);
        hooks.insert("contested-plugin", vec!["shared_tool"]);

        let servers: Vec<(&str, Vec<&str>)> = vec![
            ("memory-server", vec!["context_tool", "shared_tool"]),
            ("other-server", vec!["shared_tool"]),
        ];

        let map = resolve_server_for_plugin(&hooks, &servers);

        // Unambiguous: one server advertises context_tool.
        assert_eq!(
            map.get("mem-plugin").map(String::as_str),
            Some("memory-server")
        );
        // No match: unbound (log-only).
        assert!(!map.contains_key("lonely-plugin"));
        // Ambiguous: two distinct servers advertise shared_tool ⇒ refuse to bind.
        assert!(
            !map.contains_key("contested-plugin"),
            "ambiguous binding must be refused"
        );
    }

    // A single server advertising a contested tool name IS bound (the guard
    // only fires on >1 DISTINCT server).
    #[test]
    fn resolve_server_for_plugin_single_server_is_unambiguous() {
        let mut hooks: HashMap<&str, Vec<&str>> = HashMap::new();
        hooks.insert("p", vec!["t"]);
        let servers: Vec<(&str, Vec<&str>)> = vec![("only-server", vec!["t", "t2"])];
        let map = resolve_server_for_plugin(&hooks, &servers);
        assert_eq!(map.get("p").map(String::as_str), Some("only-server"));
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

    // F4: a response larger than the cap is truncated to <= cap, on a char
    // boundary; a response within the cap is returned unchanged.
    #[test]
    fn cap_hook_response_truncates_oversize_on_char_boundary() {
        let huge = "x".repeat(MAX_HOOK_RESPONSE_BYTES + 5_000);
        let capped = cap_hook_response(huge);
        assert!(
            capped.len() <= MAX_HOOK_RESPONSE_BYTES,
            "capped len {} must be <= {MAX_HOOK_RESPONSE_BYTES}",
            capped.len()
        );

        // Multi-byte content is never split mid-char.
        let multibyte = "€".repeat(MAX_HOOK_RESPONSE_BYTES); // 3 bytes each
        let capped_mb = cap_hook_response(multibyte);
        assert!(capped_mb.len() <= MAX_HOOK_RESPONSE_BYTES);
        assert!(
            capped_mb.is_char_boundary(capped_mb.len()),
            "truncation must land on a char boundary"
        );

        // Within-cap input is returned byte-identical.
        let small = "within cap".to_string();
        assert_eq!(cap_hook_response(small.clone()), small);
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
