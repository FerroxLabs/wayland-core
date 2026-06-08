//! `CuaTool` — `wcore_tools::Tool` impl that dispatches `CuaOp`s to the
//! current platform backend.
//!
//! Tool input shape (JSON):
//!
//! ```json
//! { "sub_agent": "writer", "op": { "kind": "left_click", "x": 100, "y": 200 } }
//! ```
//!
//! Per-sub-agent isolation: each sub-agent gets its own `CuaSession`
//! so per-op state (held modifiers) doesn't bleed across agents. The
//! tool maintains an `Arc<Mutex<HashMap<sub_agent, session_id>>>` map.
//!
//! Cancellation: `execute_with_ctx` races backend dispatch against
//! `ctx.cancel.cancelled()`. Backends never observe a cancel token
//! directly — the `select!` wrapper preempts them. 500ms max latency
//! per the W8a A.3 S2 contract.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::select;

use wcore_protocol::events::ToolCategory;
use wcore_tools::{Tool, context::ToolContext};
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::backend::{ComputerUseBackend, CuaSession, Platform};
use crate::error::{CuaError, CuaResult};
use crate::op::{CuaOp, CuaOpResult};
use crate::policy::{CuaPolicy, CuaPolicyOutcome};

pub struct CuaTool {
    backend: Arc<dyn ComputerUseBackend>,
    policy: CuaPolicy,
    sessions: Arc<Mutex<std::collections::HashMap<String, String>>>,
    /// Tool namespace the host registered this under. Defaults to "Cua".
    namespace: String,
}

fn err(content: impl Into<String>) -> ToolResult {
    ToolResult {
        content: content.into(),
        is_error: true,
    }
}

fn ok(content: impl Into<String>) -> ToolResult {
    ToolResult {
        content: content.into(),
        is_error: false,
    }
}

impl CuaTool {
    pub fn new(backend: Arc<dyn ComputerUseBackend>, policy: CuaPolicy) -> Self {
        Self {
            backend,
            policy,
            sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            namespace: "Cua".into(),
        }
    }

    pub fn with_namespace(mut self, ns: impl Into<String>) -> Self {
        self.namespace = ns.into();
        self
    }

    pub fn backend(&self) -> &Arc<dyn ComputerUseBackend> {
        &self.backend
    }

    pub fn platform(&self) -> Platform {
        self.backend.platform()
    }

    pub fn policy(&self) -> &CuaPolicy {
        &self.policy
    }

    fn ensure_session(&self, sub_agent: Option<&str>) -> CuaSession {
        let key = sub_agent.unwrap_or("").to_string();
        let mut map = self.sessions.lock();
        let session_id = map
            .entry(key.clone())
            .or_insert_with(|| {
                // Cheap counter-style id — sub-agent name pinned in so
                // logs stay readable.
                if key.is_empty() {
                    "cua-main".to_string()
                } else {
                    format!("cua-{key}")
                }
            })
            .clone();
        CuaSession::new(
            session_id,
            sub_agent.filter(|s| !s.is_empty()).map(String::from),
        )
    }

    async fn dispatch_inner(
        &self,
        session: CuaSession,
        op: CuaOp,
        cancel: tokio_util::sync::CancellationToken,
    ) -> CuaResult<CuaOpResult> {
        select! {
            _ = cancel.cancelled() => Err(CuaError::Cancelled),
            r = self.backend.dispatch(&session, op) => r,
        }
    }

    /// Public dispatch entry — used by tests + adapter callers. Mirrors
    /// the `execute_with_ctx` JSON path but takes typed args.
    ///
    /// **First-time-per-app gate wiring.** After a successful backend
    /// dispatch on a non-empty frontmost-app id, calls
    /// `policy.mark_app_seen(app)` so subsequent ops on the same
    /// `(plugin_id, app_id)` pair skip the `Suspend` prompt. The mark
    /// is persisted to disk via the policy's `seen_apps_path` (set by
    /// the host at registration time). The mark only fires on Ok — a
    /// failed op leaves the gate intact so the LLM can't smuggle an
    /// approval through a backend that returns an error.
    pub async fn dispatch(
        &self,
        session: CuaSession,
        op: CuaOp,
        cancel: tokio_util::sync::CancellationToken,
    ) -> CuaResult<CuaOpResult> {
        // Pull frontmost-app for the policy check; backends without a
        // real probe return `None` (treated as no-app-match).
        let app = self
            .backend
            .frontmost_app()
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        match self.policy.check_op(&op, &app) {
            CuaPolicyOutcome::Allow => {}
            CuaPolicyOutcome::Reject { reason } => return Err(CuaError::PolicyDenied { reason }),
            CuaPolicyOutcome::Suspend { reason } => {
                return Err(CuaError::PolicySuspended { reason });
            }
        }
        let result = self.dispatch_inner(session, op, cancel).await?;
        // Wire SECURITY MAJOR fix: record the (plugin_id, app_id) pair
        // in the persistent seen-apps store after a successful op.
        // The Allow branch above already passed the first-time gate
        // (the host approved either via require_approval flow or by
        // explicitly marking via the bridge), so recording here is the
        // post-condition that prevents re-prompting on the next op.
        if !app.is_empty() {
            self.policy.mark_app_seen(&app);
        }
        Ok(result)
    }
}

#[async_trait]
impl Tool for CuaTool {
    fn name(&self) -> &str {
        &self.namespace
    }

    fn description(&self) -> &str {
        "Computer-use tool: synthesized mouse + keyboard + screenshot + a11y \
         tree against the host desktop. Background-clean: does not move the \
         user cursor or steal focus. Per-sub-agent session isolation."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "object",
                    "description": "CUA operation tagged by `kind` (see wcore_cua::op::CuaOp)."
                },
                "sub_agent": {
                    "type": ["string", "null"],
                    "description": "Optional sub-agent name; isolates modifier state."
                }
            },
            "required": ["op"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Synthesised input mutates global desktop state — never safe.
        false
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    async fn execute(&self, input: Value) -> ToolResult {
        self.execute_with_ctx(input, &ToolContext::test_default())
            .await
    }

    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let Some(op_val) = input.get("op") else {
            return err("cua: missing required field `op`");
        };
        let op: CuaOp = match serde_json::from_value(op_val.clone()) {
            Ok(o) => o,
            Err(e) => return err(format!("cua: invalid op: {e}")),
        };

        let sub = input.get("sub_agent").and_then(|s| s.as_str());
        let session = self.ensure_session(sub);

        match self.dispatch(session, op, ctx.cancel.clone()).await {
            Ok(out) => {
                let s = serde_json::to_string(&out).unwrap_or_else(|_| "{}".into());
                ok(s)
            }
            Err(CuaError::Cancelled) => err("cua op cancelled"),
            Err(e) => err(format!("cua: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{KeyMods, MouseButton};
    use crate::backends::unsupported::UnsupportedBackend;

    fn tool_with_unsupported() -> CuaTool {
        CuaTool::new(
            Arc::new(UnsupportedBackend::new(Platform::Unsupported)),
            CuaPolicy::permissive(),
        )
    }

    #[tokio::test]
    async fn missing_op_returns_error() {
        let t = tool_with_unsupported();
        let r = t.execute(json!({})).await;
        assert!(r.is_error);
        assert!(r.content.contains("`op`"));
    }

    #[tokio::test]
    async fn wait_op_succeeds_on_unsupported_backend() {
        // Wait is platform-neutral on the unsupported backend.
        let t = tool_with_unsupported();
        let r = t
            .execute(json!({ "op": { "kind": "wait", "duration_ms": 1 } }))
            .await;
        assert!(!r.is_error, "wait should succeed: {}", r.content);
    }

    #[tokio::test]
    async fn click_returns_unsupported_on_fallback() {
        let t = tool_with_unsupported();
        let r = t
            .execute(json!({
                "op": { "kind": "left_click", "x": 10, "y": 20 }
            }))
            .await;
        assert!(r.is_error);
        assert!(r.content.to_lowercase().contains("platform"));
    }

    #[tokio::test]
    async fn cancellation_aborts_dispatch() {
        let t = tool_with_unsupported();
        let ctx = ToolContext::test_default();
        ctx.cancel.cancel();
        let r = t
            .execute_with_ctx(
                json!({ "op": { "kind": "wait", "duration_ms": 60000 } }),
                &ctx,
            )
            .await;
        assert!(r.is_error);
        assert!(r.content.to_lowercase().contains("cancel"));
    }

    #[tokio::test]
    async fn policy_reject_surfaces_as_error() {
        let mut policy = CuaPolicy::permissive();
        policy.forbidden_key_combos = vec!["ctrl+alt+del".into()];
        policy.first_time_per_app_approval = false;
        let t = CuaTool::new(
            Arc::new(UnsupportedBackend::new(Platform::Unsupported)),
            policy,
        );
        let r = t
            .execute(json!({
                "op": { "kind": "key", "keys": "ctrl+alt+del" }
            }))
            .await;
        assert!(r.is_error);
        assert!(
            r.content.to_lowercase().contains("forbidden")
                || r.content.to_lowercase().contains("policy")
        );
    }

    #[tokio::test]
    async fn typed_dispatch_returns_op_result() {
        let t = tool_with_unsupported();
        let r = t
            .dispatch(
                CuaSession::for_test("d"),
                CuaOp::Wait { duration_ms: 1 },
                tokio_util::sync::CancellationToken::new(),
            )
            .await
            .unwrap();
        assert!(matches!(r, CuaOpResult::Ok));
    }

    #[tokio::test]
    async fn per_sub_agent_sessions_are_isolated() {
        let t = tool_with_unsupported();
        let s1 = t.ensure_session(Some("writer"));
        let s2 = t.ensure_session(Some("reviewer"));
        let s1_again = t.ensure_session(Some("writer"));
        assert_ne!(s1.session_id, s2.session_id);
        assert_eq!(s1.session_id, s1_again.session_id);
    }

    /// Click is a no-op on the unsupported backend, but verifying that
    /// it does NOT silently succeed when the platform can't drive it is
    /// the whole reason the typed `UnsupportedPlatform` exists.
    #[tokio::test]
    async fn invalid_input_returns_error() {
        let t = tool_with_unsupported();
        let r = t
            .execute(json!({ "op": { "kind": "not_a_real_op" } }))
            .await;
        assert!(r.is_error);
        assert!(r.content.to_lowercase().contains("invalid"));
    }

    /// Keep the input-schema contract stable so the JSON tool router
    /// keeps decoding our envelope.
    #[test]
    fn input_schema_has_op_required() {
        let t = tool_with_unsupported();
        let schema = t.input_schema();
        let required = &schema["required"];
        assert!(required.as_array().unwrap().iter().any(|v| v == "op"));
    }

    /// `LeftClick` with non-default mods serde-roundtrips (regression for
    /// the modifier-mask field defaults).
    #[test]
    fn left_click_with_mods_roundtrip() {
        let op = CuaOp::LeftClick {
            x: 10,
            y: 20,
            button: MouseButton::Right,
            mods: KeyMods {
                shift: true,
                ctrl: true,
                ..KeyMods::default()
            },
        };
        let s = serde_json::to_string(&op).unwrap();
        let back: CuaOp = serde_json::from_str(&s).unwrap();
        assert_eq!(op, back);
    }
}
