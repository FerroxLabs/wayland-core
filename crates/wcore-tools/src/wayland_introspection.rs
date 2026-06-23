//! T3-3.8 — Wayland self-introspection toolset (`wayland_status` +
//! `wayland_telemetry_query`).
//!
//! Ported from the prior Wayland Python engine
//! (PRD-1 §4.2 + §4.3). The Python original registers two read-only
//! tools as a single `wayland_introspection` toolset so the model can
//! reflectively answer "what's the runtime state right now?" and "what
//! tools did I just call?".
//!
//! Both tools are READ-ONLY and have no side effects, so they categorize
//! as [`ToolCategory::Info`] and are always concurrency-safe.
//!
//! ## Backend boundary (audit F2)
//!
//! The Python handlers import `agent.status.wayland_status` and
//! `agent.telemetry.wayland_telemetry_query` at dispatch time — those
//! functions inspect a process-wide singleton (model/profile, daemon
//! socket, cost tracker, telemetry ring buffer). The engine MUST NOT
//! own that runtime state inside `wcore-tools`. Instead this module
//! exposes a [`WaylandIntrospectionBackend`] trait the host wires to a
//! real status snapshotter + telemetry store at construction time.
//!
//! Without a backend bound, both tools return a structured `{"error":
//! ...}` envelope (matching the Python `except Exception` path) rather
//! than a silent stub — NO-STUBS contract.
//!
//! ## Convenience filter folding
//!
//! The Python `_wayland_telemetry_query_handler` accepts EITHER a
//! structured `filter` object OR convenience top-level keys
//! (`event_type`, `tool_name`, `success`); when `filter` is absent the
//! convenience keys are folded into a synthesized filter dict. This
//! port reproduces that behaviour bit-for-bit in [`TelemetryQuery::parse`]
//! so prompts written against either shape stay valid.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Logical toolset name — matches the Python `toolset=` argument so
/// host disable lists (`platform_toolsets.cli_disabled: [...]`)
/// continue to work across the port boundary.
pub const WAYLAND_INTROSPECTION_TOOLSET: &str = "wayland_introspection";

/// Tool name for the runtime-snapshot tool.
pub const WAYLAND_STATUS_TOOL_NAME: &str = "wayland_status";

/// Tool name for the telemetry-query tool.
pub const WAYLAND_TELEMETRY_QUERY_TOOL_NAME: &str = "wayland_telemetry_query";

/// Default `since` window when the model omits it (matches Python).
pub const TELEMETRY_DEFAULT_SINCE: &str = "1h";
/// Default `limit` when the model omits it (matches Python).
pub const TELEMETRY_DEFAULT_LIMIT: u32 = 100;
/// Upper bound on `limit` (matches the Python schema 1..1000).
pub const TELEMETRY_MAX_LIMIT: u32 = 1000;

/// Parsed input for the `wayland_status` tool.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatusQuery {
    /// Optional session id to scope the snapshot. Empty string = global view.
    pub session_id: String,
}

impl StatusQuery {
    pub fn parse(input: &Value) -> Self {
        let session_id = input
            .get("session_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        Self { session_id }
    }
}

/// Parsed input for the `wayland_telemetry_query` tool. Mirrors the
/// Python `_wayland_telemetry_query_handler`: `filter` precedence over
/// convenience args, default since/limit, clamped limit.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TelemetryQuery {
    pub since: String,
    /// `None` when the caller passed no `filter` AND no convenience args.
    /// `Some(map)` when either a structured `filter` was supplied or
    /// convenience keys were folded in.
    pub filter: Option<Map<String, Value>>,
    pub limit: u32,
}

impl TelemetryQuery {
    pub fn parse(input: &Value) -> Self {
        let since = input
            .get("since")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .unwrap_or(TELEMETRY_DEFAULT_SINCE)
            .to_string();

        // Filter precedence: explicit `filter` object wins. Otherwise
        // fold convenience args (event_type / tool_name / success) into
        // a synthetic filter map; empty fold → None.
        let filter = match input.get("filter") {
            Some(Value::Object(m)) => Some(m.clone()),
            _ => {
                let mut fold = Map::new();
                for key in ["event_type", "tool_name", "success"] {
                    if let Some(v) = input.get(key)
                        && !v.is_null()
                    {
                        fold.insert(key.to_string(), v.clone());
                    }
                }
                if fold.is_empty() { None } else { Some(fold) }
            }
        };

        let raw_limit = input
            .get("limit")
            .and_then(Value::as_u64)
            .unwrap_or(u64::from(TELEMETRY_DEFAULT_LIMIT));
        let limit = raw_limit
            .clamp(1, u64::from(TELEMETRY_MAX_LIMIT))
            .min(u64::from(u32::MAX)) as u32;

        Self {
            since,
            filter,
            limit,
        }
    }
}

/// Outcome of a backend call — either a JSON payload (rendered verbatim
/// to the model) or an error string (wrapped in the same
/// `{"error": ...}` envelope the Python handlers produce).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntrospectionOutcome {
    Ok { payload: Value },
    Err { message: String },
}

/// Host-supplied backend. The engine never reads runtime state directly
/// — the host implements this trait (typically wrapping a status
/// snapshotter + telemetry ring buffer) and binds it at tool
/// construction time.
#[async_trait]
pub trait WaylandIntrospectionBackend: Send + Sync {
    /// Produce the runtime snapshot JSON (model, profile, gateway URL,
    /// daemon health, cost totals, telemetry counts).
    async fn status(&self, query: &StatusQuery) -> IntrospectionOutcome;

    /// Query the structured telemetry event log.
    async fn telemetry_query(&self, query: &TelemetryQuery) -> IntrospectionOutcome;
}

/// Fail-loud backend returned when nothing is wired. Every call returns
/// a structured error — no silent stub.
pub struct NullWaylandIntrospectionBackend;

#[async_trait]
impl WaylandIntrospectionBackend for NullWaylandIntrospectionBackend {
    async fn status(&self, _query: &StatusQuery) -> IntrospectionOutcome {
        IntrospectionOutcome::Err {
            message: "No wayland_introspection backend configured for wayland_status. \
                      Wire a WaylandIntrospectionBackend implementation when constructing \
                      the tools."
                .to_string(),
        }
    }

    async fn telemetry_query(&self, _query: &TelemetryQuery) -> IntrospectionOutcome {
        IntrospectionOutcome::Err {
            message: "No wayland_introspection backend configured for \
                      wayland_telemetry_query. Wire a WaylandIntrospectionBackend \
                      implementation when constructing the tools."
                .to_string(),
        }
    }
}

/// Recorded call into the introspection backend — preserved separately
/// per method so tests can assert dispatch shape precisely.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CapturedCalls {
    pub status: Vec<StatusQuery>,
    pub telemetry: Vec<TelemetryQuery>,
}

/// In-memory backend that records every dispatch and returns
/// deterministic synthetic JSON shaped to mirror the real runtime
/// snapshots, so tests can exercise parse → dispatch → render.
#[derive(Default)]
pub struct CapturingIntrospectionBackend {
    pub captured: parking_lot::Mutex<CapturedCalls>,
}

impl CapturingIntrospectionBackend {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> CapturedCalls {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl WaylandIntrospectionBackend for CapturingIntrospectionBackend {
    async fn status(&self, query: &StatusQuery) -> IntrospectionOutcome {
        self.captured.lock().status.push(query.clone());
        IntrospectionOutcome::Ok {
            payload: json!({
                "session_id": query.session_id,
                "model": "synthetic-model",
                "profile": "synthetic-profile",
                "gateway_url": "",
                "daemon_health": "ok",
                "cost_totals": {"input_tokens": 0, "output_tokens": 0, "usd": 0.0},
                "telemetry_counts": {"events": 0},
            }),
        }
    }

    async fn telemetry_query(&self, query: &TelemetryQuery) -> IntrospectionOutcome {
        self.captured.lock().telemetry.push(query.clone());
        IntrospectionOutcome::Ok {
            payload: json!({
                "since": query.since,
                "filter": query.filter,
                "limit": query.limit,
                "events": [],
                "count": 0,
            }),
        }
    }
}

fn render(outcome: IntrospectionOutcome) -> ToolResult {
    match outcome {
        IntrospectionOutcome::Ok { payload } => ToolResult {
            content: payload.to_string(),
            is_error: false,
        },
        IntrospectionOutcome::Err { message } => ToolResult {
            content: json!({"error": message}).to_string(),
            is_error: true,
        },
    }
}

fn status_description() -> &'static str {
    "Return a JSON snapshot of the Wayland runtime — model, profile, \
     gateway URL, daemon health, cost totals, and recent telemetry \
     counts (PRD-1 §4.3). Read-only; safe to call any time."
}

fn telemetry_description() -> &'static str {
    "Query the Wayland structured telemetry event log (PRD-1 §4.2). \
     Read-only. Returns up to `limit` events newer than `since`, \
     filtered by optional event_type / tool_name / success."
}

fn status_schema() -> JsonSchema {
    json!({
        "type": "object",
        "properties": {
            "session_id": {
                "type": "string",
                "description": "Optional session id to scope the snapshot. Defaults to empty (process-global view).",
            },
        },
        "required": []
    })
}

fn telemetry_schema() -> JsonSchema {
    json!({
        "type": "object",
        "properties": {
            "since": {
                "type": "string",
                "description": "ISO-8601 timestamp or relative window (1h, 24h, 7d). Default: '1h'."
            },
            "event_type": {
                "type": "string",
                "description": "Filter by event type (e.g. 'tool_call', 'model_call')."
            },
            "tool_name": {
                "type": "string",
                "description": "Filter by tool name."
            },
            "success": {
                "type": "boolean",
                "description": "Filter by success flag."
            },
            "limit": {
                "type": "integer",
                "minimum": 1,
                "maximum": 1000,
                "description": "Max events to return (1..1000). Default 100."
            },
            "filter": {
                "type": "object",
                "description": "Structured filter object. If supplied, takes precedence over the convenience top-level keys.",
                "additionalProperties": true
            }
        },
        "required": []
    })
}

/// `wayland_status` tool — single-call snapshot of the Wayland runtime.
pub struct WaylandStatusTool {
    backend: Arc<dyn WaylandIntrospectionBackend>,
    /// v0.9.0 W1: defaults `false` so `Tool::is_available()` hides the
    /// tool when no real backend is wired. `new(backend)` flips it on.
    backend_configured: bool,
}

impl Default for WaylandStatusTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullWaylandIntrospectionBackend),
            backend_configured: false,
        }
    }
}

impl WaylandStatusTool {
    pub fn new(backend: Arc<dyn WaylandIntrospectionBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for WaylandStatusTool {
    fn name(&self) -> &str {
        WAYLAND_STATUS_TOOL_NAME
    }

    /// v0.9.0 W1: hidden when no real `WaylandIntrospectionBackend` is
    /// wired. `Default::default()` yields `backend_configured == false`,
    /// so `ToolRegistry::register` drops the tool before the model sees it.
    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        status_description()
    }

    fn input_schema(&self) -> JsonSchema {
        status_schema()
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Pure read-only snapshot.
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let query = StatusQuery::parse(&input);
        render(self.backend.status(&query).await)
    }
}

/// `wayland_telemetry_query` tool — structured event-log query.
pub struct WaylandTelemetryQueryTool {
    backend: Arc<dyn WaylandIntrospectionBackend>,
    /// v0.9.0 W1: defaults `false` so `Tool::is_available()` hides the
    /// tool when no real backend is wired. `new(backend)` flips it on.
    backend_configured: bool,
}

impl Default for WaylandTelemetryQueryTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullWaylandIntrospectionBackend),
            backend_configured: false,
        }
    }
}

impl WaylandTelemetryQueryTool {
    pub fn new(backend: Arc<dyn WaylandIntrospectionBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for WaylandTelemetryQueryTool {
    fn name(&self) -> &str {
        WAYLAND_TELEMETRY_QUERY_TOOL_NAME
    }

    /// v0.9.0 W1: hidden when no real `WaylandIntrospectionBackend` is
    /// wired. `Default::default()` yields `backend_configured == false`,
    /// so `ToolRegistry::register` drops the tool before the model sees it.
    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        telemetry_description()
    }

    fn input_schema(&self) -> JsonSchema {
        telemetry_schema()
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Pure read-only query.
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let query = TelemetryQuery::parse(&input);
        render(self.backend.telemetry_query(&query).await)
    }
}

/// Construct both tools sharing a single backend — convenience for
/// hosts that want to register the toolset atomically (matching the
/// "they travel as a unit" contract in the Python module docstring).
pub fn build_toolset(
    backend: Arc<dyn WaylandIntrospectionBackend>,
) -> (WaylandStatusTool, WaylandTelemetryQueryTool) {
    (
        WaylandStatusTool::new(Arc::clone(&backend)),
        WaylandTelemetryQueryTool::new(backend),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run<T: Tool>(t: &T, args: Value) -> ToolResult {
        futures::executor::block_on(t.execute(args))
    }

    #[test]
    fn status_records_session_id_and_returns_payload() {
        let backend = Arc::new(CapturingIntrospectionBackend::new());
        let tool = WaylandStatusTool::new(backend.clone());
        let res = run(&tool, json!({"session_id": "sess-42"}));
        assert!(!res.is_error, "expected ok, got: {}", res.content);
        let snap = backend.snapshot();
        assert_eq!(snap.status.len(), 1);
        assert_eq!(snap.status[0].session_id, "sess-42");
        assert!(snap.telemetry.is_empty());
        assert!(res.content.contains("\"session_id\":\"sess-42\""));
        assert!(res.content.contains("\"model\""));
    }

    #[test]
    fn status_defaults_session_id_to_empty() {
        let backend = Arc::new(CapturingIntrospectionBackend::new());
        let tool = WaylandStatusTool::new(backend.clone());
        let res = run(&tool, json!({}));
        assert!(!res.is_error);
        let snap = backend.snapshot();
        assert_eq!(snap.status[0].session_id, "");
    }

    #[test]
    fn null_backend_status_fails_loud_no_silent_stub() {
        let tool = WaylandStatusTool::default();
        let res = run(&tool, json!({}));
        assert!(res.is_error);
        assert!(
            res.content
                .contains("No wayland_introspection backend configured"),
            "expected fail-loud, got: {}",
            res.content
        );
        assert!(res.content.contains("wayland_status"));
    }

    #[test]
    fn null_backend_telemetry_fails_loud_no_silent_stub() {
        let tool = WaylandTelemetryQueryTool::default();
        let res = run(&tool, json!({}));
        assert!(res.is_error);
        assert!(
            res.content
                .contains("No wayland_introspection backend configured")
        );
        assert!(res.content.contains("wayland_telemetry_query"));
    }

    #[test]
    fn telemetry_defaults_match_python_handler() {
        let backend = Arc::new(CapturingIntrospectionBackend::new());
        let tool = WaylandTelemetryQueryTool::new(backend.clone());
        let res = run(&tool, json!({}));
        assert!(!res.is_error);
        let snap = backend.snapshot();
        assert_eq!(snap.telemetry.len(), 1);
        let q = &snap.telemetry[0];
        assert_eq!(q.since, "1h");
        assert_eq!(q.limit, 100);
        assert!(q.filter.is_none(), "no filter args ⇒ None");
    }

    #[test]
    fn telemetry_folds_convenience_args_into_filter() {
        // No top-level `filter`, but event_type + tool_name + success
        // present — Python folds these into a synthetic filter dict.
        let backend = Arc::new(CapturingIntrospectionBackend::new());
        let tool = WaylandTelemetryQueryTool::new(backend.clone());
        let res = run(
            &tool,
            json!({
                "event_type": "tool_call",
                "tool_name": "Read",
                "success": true,
                "since": "24h",
                "limit": 50,
            }),
        );
        assert!(!res.is_error);
        let q = &backend.snapshot().telemetry[0];
        assert_eq!(q.since, "24h");
        assert_eq!(q.limit, 50);
        let f = q.filter.as_ref().expect("convenience args ⇒ Some(filter)");
        assert_eq!(
            f.get("event_type").and_then(Value::as_str),
            Some("tool_call")
        );
        assert_eq!(f.get("tool_name").and_then(Value::as_str), Some("Read"));
        assert_eq!(f.get("success").and_then(Value::as_bool), Some(true));
    }

    #[test]
    fn telemetry_explicit_filter_takes_precedence_over_convenience() {
        // Python: `flt = args.get("filter") if isinstance(...) else None`
        // then convenience folding ONLY runs when flt is None. So if
        // both shapes coexist, the explicit filter wins.
        let backend = Arc::new(CapturingIntrospectionBackend::new());
        let tool = WaylandTelemetryQueryTool::new(backend.clone());
        let res = run(
            &tool,
            json!({
                "filter": {"event_type": "model_call", "model": "claude-3-5"},
                "event_type": "tool_call",
                "tool_name": "Read",
            }),
        );
        assert!(!res.is_error);
        let q = &backend.snapshot().telemetry[0];
        let f = q.filter.as_ref().expect("explicit filter ⇒ Some");
        assert_eq!(
            f.get("event_type").and_then(Value::as_str),
            Some("model_call")
        );
        assert_eq!(f.get("model").and_then(Value::as_str), Some("claude-3-5"));
        // Convenience args MUST NOT be folded in alongside.
        assert!(!f.contains_key("tool_name"));
    }

    #[test]
    fn telemetry_limit_is_clamped_to_max() {
        let backend = Arc::new(CapturingIntrospectionBackend::new());
        let tool = WaylandTelemetryQueryTool::new(backend.clone());
        let _ = run(&tool, json!({"limit": 9999}));
        assert_eq!(backend.snapshot().telemetry[0].limit, TELEMETRY_MAX_LIMIT);
    }

    #[test]
    fn schemas_match_python_required_and_enum_shape() {
        let s = status_schema();
        assert_eq!(s["type"], "object");
        assert!(s["properties"]["session_id"].is_object());
        assert_eq!(s["required"].as_array().unwrap().len(), 0);

        let t = telemetry_schema();
        assert_eq!(t["type"], "object");
        for key in [
            "since",
            "event_type",
            "tool_name",
            "success",
            "limit",
            "filter",
        ] {
            assert!(t["properties"][key].is_object(), "missing property: {key}");
        }
        assert_eq!(t["properties"]["limit"]["minimum"], 1);
        assert_eq!(t["properties"]["limit"]["maximum"], 1000);
        assert_eq!(t["required"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn registry_metadata_matches_python_toolset_unit() {
        // Both tools share the same toolset name so hosts that disable
        // `wayland_introspection` flip both off together.
        assert_eq!(WAYLAND_INTROSPECTION_TOOLSET, "wayland_introspection");
        assert_eq!(WAYLAND_STATUS_TOOL_NAME, "wayland_status");
        assert_eq!(WAYLAND_TELEMETRY_QUERY_TOOL_NAME, "wayland_telemetry_query");

        let backend = Arc::new(CapturingIntrospectionBackend::new());
        let (status, tq) = build_toolset(backend);
        assert_eq!(status.name(), WAYLAND_STATUS_TOOL_NAME);
        assert_eq!(tq.name(), WAYLAND_TELEMETRY_QUERY_TOOL_NAME);
        // Both are read-only Info-category.
        assert!(matches!(status.category(), ToolCategory::Info));
        assert!(matches!(tq.category(), ToolCategory::Info));
        // Both are concurrency-safe regardless of input.
        assert!(status.is_concurrency_safe(&json!({"session_id": "x"})));
        assert!(tq.is_concurrency_safe(&json!({"event_type": "tool_call"})));
    }

    #[test]
    fn build_toolset_shares_single_backend_so_disable_travels_as_unit() {
        let backend = Arc::new(CapturingIntrospectionBackend::new());
        let (status, tq) = build_toolset(backend.clone());
        let _ = run(&status, json!({"session_id": "s1"}));
        let _ = run(&tq, json!({"event_type": "tool_call"}));
        let snap = backend.snapshot();
        assert_eq!(snap.status.len(), 1);
        assert_eq!(snap.telemetry.len(), 1);
    }

    #[test]
    fn telemetry_null_value_in_convenience_arg_is_dropped() {
        // The Python `if args[key] is not None` check excludes JSON nulls.
        let backend = Arc::new(CapturingIntrospectionBackend::new());
        let tool = WaylandTelemetryQueryTool::new(backend.clone());
        let _ = run(
            &tool,
            json!({
                "event_type": "tool_call",
                "tool_name": null,
                "success": null,
            }),
        );
        let q = &backend.snapshot().telemetry[0];
        let f = q.filter.as_ref().unwrap();
        assert_eq!(f.len(), 1);
        assert!(f.contains_key("event_type"));
    }

    // --- v0.9.0 W1 backend gate (per-tool, gated independently) ---

    #[test]
    fn status_default_is_hidden_when_no_backend_wired() {
        let tool = WaylandStatusTool::default();
        assert!(
            !tool.is_available(),
            "Default::default() must yield backend_configured == false"
        );
    }

    #[test]
    fn status_with_real_backend_is_available() {
        let tool = WaylandStatusTool::new(Arc::new(CapturingIntrospectionBackend::new()));
        assert!(
            tool.is_available(),
            "new(backend) must yield backend_configured == true"
        );
    }

    #[test]
    fn telemetry_default_is_hidden_when_no_backend_wired() {
        let tool = WaylandTelemetryQueryTool::default();
        assert!(
            !tool.is_available(),
            "Default::default() must yield backend_configured == false"
        );
    }

    #[test]
    fn telemetry_with_real_backend_is_available() {
        let tool = WaylandTelemetryQueryTool::new(Arc::new(CapturingIntrospectionBackend::new()));
        assert!(
            tool.is_available(),
            "new(backend) must yield backend_configured == true"
        );
    }
}
