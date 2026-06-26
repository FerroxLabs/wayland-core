use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;
use wcore_config::circuit_breaker::{BreakerState, CircuitBreaker, CircuitBreakerConfig};
use wcore_types::tool::{ToolDef, ToolResult};

use crate::Tool;
use crate::dispatcher::ToolDispatcher;

/// Per-tool circuit-breaker defaults.
///
/// 3 failures in a 30-second window trips the breaker; it stays Open
/// for 60 seconds before allowing a single trial (HalfOpen).
fn default_breaker_cfg() -> CircuitBreakerConfig {
    CircuitBreakerConfig::default()
}

pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
    /// One circuit breaker per registered tool name. `Arc<RwLock<…>>`
    /// so the registry can be shared across async call sites without
    /// requiring `&mut self` at dispatch time.
    breakers: Arc<RwLock<HashMap<String, CircuitBreaker>>>,
    /// Optional filesystem the orchestration dispatcher routes every
    /// tool's `ToolContext` through. `None` (the default) means the
    /// dispatcher uses an unconfined `RealFs` — the local-CLI behaviour.
    /// A channel-originated engine in `Workspace` posture sets this to a
    /// `SandboxedFs` rooted at its workspace so `Read`/`Grep`/`Glob`
    /// (which honour `ctx.vfs`) cannot escape the jail. Carried on the
    /// registry — which is already threaded into every orchestration
    /// `execute_*` call — to avoid plumbing a new parameter through the
    /// whole dispatch stack.
    tool_vfs: Option<Arc<dyn crate::vfs::VirtualFs>>,

    /// Session workspace policy, installed at bootstrap (`Trusted`) or by the
    /// `Workspace` posture (`Contained`). Threaded onto every dispatched
    /// `ToolContext` so BashTool can root its OS sandbox at the workspace.
    workspace_policy: Option<Arc<crate::workspace_policy::WorkspacePolicy>>,
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}
impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: Vec::new(),
            breakers: Arc::new(RwLock::new(HashMap::new())),
            tool_vfs: None,
            workspace_policy: None,
        }
    }

    /// Set the filesystem every dispatched tool's `ToolContext` is built
    /// with. See the [`tool_vfs`](Self::tool_vfs) field. Used by the
    /// channel `Workspace` posture to install a `SandboxedFs` jail.
    pub fn set_tool_vfs(&mut self, vfs: Arc<dyn crate::vfs::VirtualFs>) {
        self.tool_vfs = Some(vfs);
    }

    /// The filesystem the dispatcher should build tool contexts with, if
    /// one was installed. `None` means use the default `RealFs`.
    pub fn tool_vfs(&self) -> Option<Arc<dyn crate::vfs::VirtualFs>> {
        self.tool_vfs.clone()
    }

    pub fn set_workspace_policy(&mut self, policy: Arc<crate::workspace_policy::WorkspacePolicy>) {
        self.workspace_policy = Some(policy);
    }

    pub fn workspace_policy(&self) -> Option<Arc<crate::workspace_policy::WorkspacePolicy>> {
        self.workspace_policy.clone()
    }

    /// Drop every registered tool for which `keep` returns `false`.
    ///
    /// Applied once, AFTER the full tool set is registered, to enforce a
    /// reduced toolset on a restricted engine (e.g. a channel-originated
    /// engine that must not expose host filesystem/shell tools to a remote
    /// sender). Filtering at the registry — rather than only omitting tools
    /// from the LLM schema — means a dropped tool is also un-dispatchable:
    /// `get()` returns `None`, so even a hallucinated call cannot reach it.
    /// The matching circuit-breaker entries are pruned too.
    pub fn retain<F>(&mut self, keep: F)
    where
        F: Fn(&dyn Tool) -> bool,
    {
        let mut kept_names: Vec<String> = Vec::with_capacity(self.tools.len());
        self.tools.retain(|t| {
            let keep_it = keep(t.as_ref());
            if keep_it {
                kept_names.push(t.name().to_string());
            }
            keep_it
        });
        self.breakers
            .write()
            .retain(|name, _| kept_names.contains(name));
    }

    pub fn register(&mut self, tool: Box<dyn Tool>) {
        // External-service tools (web, vision, transcription, gitlab,
        // notion, discord, …) ship a `Null*Backend` default and override
        // `is_available()` to return false until the host wires a real
        // backend. Silently skipping unavailable tools here keeps the
        // model from ever seeing a tool it cannot successfully call —
        // which used to manifest as "running forever" in the TUI because
        // the tool sat in AwaitingApproval while the agent burned turns
        // retrying a call that always errored.
        if !tool.is_available() {
            tracing::info!(
                tool = %tool.name(),
                "skipping registration of tool whose backend is not configured"
            );
            return;
        }
        self.breakers
            .write()
            .entry(tool.name().to_string())
            .or_insert_with(|| CircuitBreaker::new(default_breaker_cfg()));
        self.tools.push(tool);
    }

    /// Replace any previously-registered tool with the same `name()` and
    /// install the new one. Preserves the existing circuit-breaker state
    /// (the breaker is per-name and persists across re-registration).
    ///
    /// Use this for the boot-time `Null*Transport` → real-transport
    /// upgrade pattern (audit 2026-05-24 fix): the host registers a
    /// schema-visible default at the registry-construction site, then
    /// later upgrades the implementation once host-side resources
    /// (channel managers, async runtimes) are available.
    pub fn replace_by_name(&mut self, tool: Box<dyn Tool>) {
        let name = tool.name().to_string();
        self.tools.retain(|t| t.name() != name);
        self.breakers
            .write()
            .entry(name)
            .or_insert_with(|| CircuitBreaker::new(default_breaker_cfg()));
        self.tools.push(tool);
    }

    /// Find a tool by name
    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|t| t.name() == name)
            .map(|t| t.as_ref())
    }

    /// AUDIT B-4 — is the named tool's circuit breaker currently open?
    ///
    /// The breaker (3 failures / 30s trips it; 60s open) was previously
    /// reachable ONLY through `ToolDispatcher::dispatch[_with_ctx]`,
    /// which the agent's main tool loop bypasses (it calls `get()` +
    /// `execute_with_ctx()` directly). This inherent method lets the
    /// orchestration dispatch path consult the breaker without routing
    /// through the full `ToolDispatcher` trait. Returns `false` for an
    /// unregistered tool (nothing to short-circuit).
    pub fn breaker_is_open(&self, name: &str) -> bool {
        self.breakers
            .read()
            .get(name)
            .map(|b| b.is_open())
            .unwrap_or(false)
    }

    /// AUDIT B-4 — record a dispatch outcome against the named tool's
    /// circuit breaker. `is_error == true` records a failure (a timeout
    /// or panic counts here too); `false` records a success, which
    /// resets the failure window. No-op for an unregistered tool.
    pub fn record_breaker_outcome(&self, name: &str, is_error: bool) {
        if let Some(breaker) = self.breakers.read().get(name) {
            if is_error {
                breaker.record_failure();
            } else {
                breaker.record_success();
            }
        }
    }

    /// Get all registered tool names
    pub fn tool_names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    /// Generate API tool definitions for all registered tools
    pub fn to_tool_defs(&self) -> Vec<ToolDef> {
        self.tools
            .iter()
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
                deferred: t.is_deferred(),
                server: t.mcp_server().map(str::to_string),
            })
            .collect()
    }

    /// Generate API tool definitions for tools matching a predicate.
    ///
    /// Used by plan mode to restrict the tool set sent to the LLM.
    pub fn to_tool_defs_filtered<F>(&self, filter: F) -> Vec<ToolDef>
    where
        F: Fn(&dyn Tool) -> bool,
    {
        self.tools
            .iter()
            .filter(|t| filter(t.as_ref()))
            .map(|t| ToolDef {
                name: t.name().to_string(),
                description: t.description().to_string(),
                input_schema: t.input_schema(),
                deferred: t.is_deferred(),
                server: t.mcp_server().map(str::to_string),
            })
            .collect()
    }
}

#[async_trait]
impl ToolDispatcher for ToolRegistry {
    async fn dispatch(&self, tool: &str, input: serde_json::Value) -> ToolResult {
        // Check circuit breaker before executing.
        if let Some(breaker) = self.breakers.read().get(tool)
            && breaker.is_open()
        {
            return ToolResult {
                content: format!(
                    "tool '{tool}' circuit open: too many recent failures, try again later"
                ),
                is_error: true,
            };
        }

        let result = match self.get(tool) {
            Some(t) => t.execute(input).await,
            None => {
                return ToolResult {
                    content: format!("tool '{tool}' not in registry"),
                    is_error: true,
                };
            }
        };

        // Record outcome.
        if let Some(breaker) = self.breakers.read().get(tool) {
            if result.is_error {
                breaker.record_failure();
            } else {
                breaker.record_success();
            }
        }

        result
    }

    /// W8b.2.A — propagate the caller's `ToolContext` to the resolved
    /// tool's `execute_with_ctx`. Lets `ScriptTool` thread its parent
    /// context (vfs, cancel, file_write_notifier) into every sub-step.
    async fn dispatch_with_ctx(
        &self,
        tool: &str,
        input: serde_json::Value,
        ctx: &crate::context::ToolContext,
    ) -> ToolResult {
        // Check circuit breaker before executing.
        if let Some(breaker) = self.breakers.read().get(tool)
            && breaker.is_open()
        {
            return ToolResult {
                content: format!(
                    "tool '{tool}' circuit open: too many recent failures, try again later"
                ),
                is_error: true,
            };
        }

        let result = match self.get(tool) {
            Some(t) => t.execute_with_ctx(input, ctx).await,
            None => {
                return ToolResult {
                    content: format!("tool '{tool}' not in registry"),
                    is_error: true,
                };
            }
        };

        // Record outcome.
        if let Some(breaker) = self.breakers.read().get(tool) {
            if result.is_error {
                breaker.record_failure();
            } else {
                breaker.record_success();
            }
        }

        result
    }

    /// Returns the current `BreakerState` for a tool, or `None` if
    /// the tool is not registered. Used by tests and observability hooks.
    fn breaker_state(&self, tool: &str) -> Option<BreakerState> {
        self.breakers.read().get(tool).map(|b| b.state())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Tool;
    use async_trait::async_trait;
    use wcore_protocol::events::ToolCategory;
    use wcore_types::tool::ToolResult;

    #[test]
    fn workspace_policy_defaults_none_and_sets() {
        use crate::workspace_policy::WorkspacePolicy;
        use std::sync::Arc;
        let mut reg = ToolRegistry::new();
        assert!(reg.workspace_policy().is_none());
        let dir = tempfile::tempdir().unwrap();
        let policy = Arc::new(WorkspacePolicy::trusted_local(dir.path()));
        reg.set_workspace_policy(Arc::clone(&policy));
        assert_eq!(reg.workspace_policy().unwrap().root(), policy.root());
    }

    /// A minimal Tool implementation used only in tests
    struct MockTool {
        tool_name: String,
        tool_description: String,
        tool_category: ToolCategory,
    }

    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn description(&self) -> &str {
            &self.tool_description
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }

        async fn execute(&self, _input: serde_json::Value) -> ToolResult {
            ToolResult {
                content: "ok".to_string(),
                is_error: false,
            }
        }

        fn category(&self) -> ToolCategory {
            self.tool_category
        }
    }

    /// Helper to create a MockTool with the given name and description
    fn make_tool(name: &str, description: &str) -> Box<MockTool> {
        Box::new(MockTool {
            tool_name: name.to_string(),
            tool_description: description.to_string(),
            tool_category: ToolCategory::Info,
        })
    }

    fn make_tool_with_category(
        name: &str,
        description: &str,
        category: ToolCategory,
    ) -> Box<MockTool> {
        Box::new(MockTool {
            tool_name: name.to_string(),
            tool_description: description.to_string(),
            tool_category: category,
        })
    }

    #[test]
    fn test_register_and_get() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("my_tool", "does something"));

        let found = registry.get("my_tool");
        assert!(
            found.is_some(),
            "registered tool should be retrievable by name"
        );
        assert_eq!(found.unwrap().name(), "my_tool");
    }

    #[test]
    fn test_get_nonexistent_returns_none() {
        let registry = ToolRegistry::new();

        let result = registry.get("ghost");
        assert!(
            result.is_none(),
            "looking up an unregistered name should return None"
        );
    }

    #[test]
    fn test_tool_names() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("alpha", "first tool"));
        registry.register(make_tool("beta", "second tool"));
        registry.register(make_tool("gamma", "third tool"));

        let mut names = registry.tool_names();
        names.sort(); // sort for a stable assertion order
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn test_to_tool_defs() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("tool_a", "description A"));
        registry.register(make_tool("tool_b", "description B"));

        let defs = registry.to_tool_defs();
        assert_eq!(
            defs.len(),
            2,
            "to_tool_defs should return one entry per registered tool"
        );

        // Collect (name, description) pairs for assertion independent of order
        let mut pairs: Vec<(&str, &str)> = defs
            .iter()
            .map(|d| (d.name.as_str(), d.description.as_str()))
            .collect();
        pairs.sort();

        assert_eq!(pairs[0], ("tool_a", "description A"));
        assert_eq!(pairs[1], ("tool_b", "description B"));

        // Verify the input_schema field is populated correctly
        let expected_schema = serde_json::json!({"type": "object"});
        for def in &defs {
            assert_eq!(def.input_schema, expected_schema);
        }
    }

    // --- retain / tool_vfs tests ---

    #[test]
    fn retain_drops_unmatched_tools_and_prunes_breakers() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool_with_category(
            "Read",
            "fs read",
            ToolCategory::Info,
        ));
        registry.register(make_tool_with_category("Bash", "shell", ToolCategory::Exec));
        registry.register(make_tool_with_category("web", "net", ToolCategory::Info));

        // Keep only "web".
        registry.retain(|t| t.name() == "web");

        let mut names = registry.tool_names();
        names.sort();
        assert_eq!(names, vec!["web"], "only the kept tool survives");
        // Dropped tools are un-dispatchable, not merely hidden from the schema.
        assert!(registry.get("Read").is_none());
        assert!(registry.get("Bash").is_none());
        // Breaker entries for dropped tools are pruned; the survivor keeps one.
        assert!(!registry.breaker_is_open("web"), "survivor breaker intact");
        assert!(registry.breakers.read().contains_key("web"));
        assert!(!registry.breakers.read().contains_key("Read"));
        assert!(!registry.breakers.read().contains_key("Bash"));
    }

    #[test]
    fn tool_vfs_defaults_none_and_round_trips() {
        let mut registry = ToolRegistry::new();
        assert!(
            registry.tool_vfs().is_none(),
            "default is unconfined RealFs"
        );
        registry.set_tool_vfs(Arc::new(crate::vfs::RealFs));
        assert!(registry.tool_vfs().is_some(), "installed vfs is observable");
    }

    // --- to_tool_defs_filtered tests ---

    #[test]
    fn filtered_by_category_returns_matching_tools() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool_with_category(
            "Read",
            "read files",
            ToolCategory::Info,
        ));
        registry.register(make_tool_with_category(
            "Write",
            "write files",
            ToolCategory::Edit,
        ));
        registry.register(make_tool_with_category(
            "Bash",
            "run commands",
            ToolCategory::Exec,
        ));
        registry.register(make_tool_with_category(
            "ExitPlanMode",
            "exit plan mode",
            ToolCategory::Info,
        ));

        let defs = registry.to_tool_defs_filtered(|t| t.category() == ToolCategory::Info);

        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert!(names.contains(&"Read"));
        assert!(names.contains(&"ExitPlanMode"));
        assert!(!names.contains(&"Write"));
        assert!(!names.contains(&"Bash"));
    }

    #[test]
    fn filtered_by_name_excludes_specific_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("alpha", "first"));
        registry.register(make_tool("beta", "second"));
        registry.register(make_tool("gamma", "third"));

        let defs = registry.to_tool_defs_filtered(|t| t.name() != "beta");

        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names.len(), 2);
        assert!(names.contains(&"alpha"));
        assert!(names.contains(&"gamma"));
        assert!(!names.contains(&"beta"));
    }

    #[test]
    fn filtered_accept_all_matches_to_tool_defs() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("a", "tool a"));
        registry.register(make_tool("b", "tool b"));

        let all = registry.to_tool_defs();
        let filtered = registry.to_tool_defs_filtered(|_| true);

        assert_eq!(all.len(), filtered.len());
        for (a, f) in all.iter().zip(filtered.iter()) {
            assert_eq!(a.name, f.name);
        }
    }

    #[test]
    fn filtered_reject_all_returns_empty() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("a", "tool a"));

        let defs = registry.to_tool_defs_filtered(|_| false);
        assert!(defs.is_empty());
    }

    #[test]
    fn filtered_empty_registry_returns_empty() {
        let registry = ToolRegistry::new();
        let defs = registry.to_tool_defs_filtered(|_| true);
        assert!(defs.is_empty());
    }

    // --- deferred flag tests ---

    /// A minimal Tool that overrides is_deferred() to return true
    struct DeferredMockTool {
        tool_name: String,
    }

    #[async_trait]
    impl Tool for DeferredMockTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn description(&self) -> &str {
            "a deferred tool"
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object", "properties": {"x": {"type": "string"}}})
        }

        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
            true
        }

        fn is_deferred(&self) -> bool {
            true
        }

        async fn execute(&self, _input: serde_json::Value) -> ToolResult {
            ToolResult {
                content: "ok".to_string(),
                is_error: false,
            }
        }

        fn category(&self) -> ToolCategory {
            ToolCategory::Info
        }
    }

    #[test]
    fn to_tool_defs_includes_deferred_flag() {
        let mut registry = ToolRegistry::new();
        registry.register(make_tool("core_tool", "a core tool"));
        let defs = registry.to_tool_defs();
        assert!(!defs[0].deferred, "default tools should not be deferred");
    }

    #[test]
    fn to_tool_defs_deferred_tool_flagged() {
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(DeferredMockTool {
            tool_name: "lazy_tool".to_string(),
        }));
        let defs = registry.to_tool_defs();
        assert!(defs[0].deferred, "deferred tool should have deferred=true");
    }

    /// v0.9.1.1 F8 — the catalog the LLM sees must use the exact
    /// string each backend reports from `Tool::name()`. A mismatch
    /// here means the model is taught the tool is called X, the
    /// dispatcher routes only on Y, and every call comes back as
    /// "tool 'X' not in registry" → which the live drive surfaced
    /// as `cancelled text_to_speech · API 400 …` errors.
    ///
    /// The current `to_tool_defs()` builds the catalog directly from
    /// `t.name()`, so this property holds by construction. The test
    /// pins it so a future refactor that, say, lower-cases the
    /// catalog or rewrites snake_case to PascalCase before sending
    /// to the LLM is caught immediately.
    #[test]
    fn tool_catalog_names_match_backend_names_v0911() {
        let mut registry = ToolRegistry::new();
        // The real-world set the architecture audit cited as the
        // dispatcher-mismatch surface — file ops in PascalCase,
        // multimodal/integration tools in snake_case, plus the two
        // names (`web`, `homeassistant`) that already matched.
        let names = [
            "Bash",
            "Read",
            "Write",
            "Edit",
            "Grep",
            "Glob",
            "web",
            "WebFetch",
            "vision_analyze",
            "transcribe_audio",
            "image_generate",
            "text_to_speech",
            "github_api",
            "discord_server",
            "homeassistant",
        ];
        for name in names {
            registry.register(make_tool(name, "fixture"));
        }
        let defs = registry.to_tool_defs();
        // Build a name-keyed map from both sides so we compare equal
        // sets regardless of registration order.
        let catalog_names: std::collections::HashSet<String> =
            defs.iter().map(|d| d.name.clone()).collect();
        let backend_names: std::collections::HashSet<String> =
            registry.tool_names().into_iter().collect();
        assert_eq!(
            catalog_names, backend_names,
            "tool catalog names sent to the LLM must equal the set returned by Tool::name() \
             (catalog={catalog_names:?}, backend={backend_names:?})"
        );
        // And no name was rewritten in transit.
        for d in &defs {
            assert!(
                backend_names.contains(&d.name),
                "catalog name `{}` not present in backend names {:?}",
                d.name,
                backend_names
            );
        }
    }
}
