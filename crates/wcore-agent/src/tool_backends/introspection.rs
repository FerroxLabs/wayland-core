//! v0.9.0 Wave-1 B7 — in-process backend for `wayland_status` +
//! `wayland_telemetry_query`.
//!
//! The introspection tools are READ-ONLY observers of the engine's
//! own runtime state. There is no network call, no API key, and no
//! external service. This backend reads through an
//! `Arc<dyn SessionStateReader>` and shapes the JSON payloads the
//! tools surface to the model.
//!
//! ## Telemetry-query safety (S-H6)
//!
//! `wayland_telemetry_query` lives in the default `tools.allow_list`
//! and is auto-approved. To prevent the auto-approval gate from
//! becoming an injection surface, query input is a **typed enum**:
//!
//! ```text
//!   { "kind": "top_n_tools", "n": 5, "by": "calls" }
//!   { "kind": "tool_usage_over_time", "window_secs": 3600 }
//!   { "kind": "errors_by_provider", "provider": "anthropic" }
//!   { "kind": "session_stats" }
//! ```
//!
//! There is no `custom { sql: "…" }` escape hatch. `serde` rejects
//! unknown `kind` values before the value reaches dispatch, and the
//! tool's `input_schema` advertises only the allowed variants so the
//! model is guided to legal shapes.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};

use wcore_tools::wayland_introspection::{
    IntrospectionOutcome, StatusQuery, TelemetryQuery as ToolTelemetryQuery,
    WaylandIntrospectionBackend,
};

use crate::session_state::{ProviderHealthStatus, SessionStateReader};

/// Choice of ordering for `top_n_tools`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TopNMetric {
    Calls,
    /// Tokens-per-tool tracking is engine-side work that is not yet
    /// wired (Wave-1 only adds the read path). Accept the variant so
    /// the schema stays stable, but dispatch falls back to `Calls`
    /// with a `degraded=true` flag in the payload until the
    /// per-tool-token writer lands.
    Tokens,
    /// Same caveat as `Tokens`.
    Duration,
}

/// Typed shape for `wayland_telemetry_query.query`. JSON shape is
/// `{ "kind": "...", … }` via serde tagging — `serde` rejects any
/// `kind` not in this list before it can reach the backend, which
/// closes the auto-approval injection surface (S-H6).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TelemetryQuery {
    ToolUsageOverTime { window_secs: u64 },
    TopNTools { n: u32, by: TopNMetric },
    ErrorsByProvider { provider: Option<String> },
    SessionStats,
}

impl TelemetryQuery {
    /// Try to extract a typed `TelemetryQuery` from a tool input value.
    /// Accepts either `{ "query": <enum> }` (preferred) or the bare
    /// enum at the top level. Returns the structured serde error
    /// untouched so callers can surface it as a tool-level error.
    pub fn from_tool_input(input: &Value) -> Result<Self, String> {
        let raw = input.get("query").unwrap_or(input);
        serde_json::from_value(raw.clone()).map_err(|e| format!("invalid telemetry query: {e}"))
    }
}

/// Local, in-process backend. Holds no I/O resources — every call is
/// an O(1)/O(n) read against `SessionStateReader`.
pub struct LocalIntrospectionBackend {
    state: Arc<dyn SessionStateReader>,
}

impl LocalIntrospectionBackend {
    pub fn new(state: Arc<dyn SessionStateReader>) -> Self {
        Self { state }
    }

    fn session_stats_payload(&self) -> Value {
        let started = self.state.session_started_at();
        let elapsed = (Utc::now() - started).num_seconds().max(0);
        let tool_calls = self.state.tool_call_count();
        let total_tool_calls: u64 = tool_calls.values().sum();
        json!({
            "active_model": self.state.active_model(),
            "session_started_at": started.to_rfc3339(),
            "session_duration_secs": elapsed,
            "token_count_input": self.state.token_count_input(),
            "token_count_output": self.state.token_count_output(),
            "total_tool_calls": total_tool_calls,
        })
    }
}

#[async_trait]
impl WaylandIntrospectionBackend for LocalIntrospectionBackend {
    async fn status(&self, query: &StatusQuery) -> IntrospectionOutcome {
        let recent = self.state.recent_errors(3);
        let recent_json: Vec<Value> = recent
            .iter()
            .map(|e| {
                json!({
                    "timestamp": e.timestamp.to_rfc3339(),
                    "message": e.message,
                })
            })
            .collect();
        let providers: Vec<Value> = self
            .state
            .provider_health()
            .iter()
            .map(|(name, h)| {
                json!({
                    "provider": name,
                    "status": provider_status_label(h.status),
                    "last_check": h.last_check.to_rfc3339(),
                })
            })
            .collect();
        let stats = self.session_stats_payload();
        IntrospectionOutcome::Ok {
            payload: json!({
                "session_id": query.session_id,
                "model": self.state.active_model(),
                "active_model": self.state.active_model(),
                "session_started_at": stats["session_started_at"],
                "session_duration_secs": stats["session_duration_secs"],
                "token_count_input": self.state.token_count_input(),
                "token_count_output": self.state.token_count_output(),
                "tool_calls": self.state.tool_call_count(),
                "recent_errors": recent_json,
                "provider_health": providers,
            }),
        }
    }

    async fn telemetry_query(&self, query: &ToolTelemetryQuery) -> IntrospectionOutcome {
        // The tool's `filter` slot carries the typed query in v0.9.0
        // (the legacy `event_type`/`tool_name` convenience args are
        // ignored here — the typed enum is the only public surface).
        // The tool wrapper synthesises a `Value` and hands it to us;
        // we reparse to the typed enum so unknown variants fail loud.
        let filter_value = match &query.filter {
            Some(map) => Value::Object(map.clone()),
            None => {
                // Default to session_stats when the model passes no
                // structured query — matches the "what's my session
                // doing right now?" intent and avoids the auto-approval
                // path silently returning an empty event list.
                return IntrospectionOutcome::Ok {
                    payload: self.session_stats_payload(),
                };
            }
        };
        let parsed = match TelemetryQuery::from_tool_input(&filter_value) {
            Ok(q) => q,
            Err(message) => return IntrospectionOutcome::Err { message },
        };

        match parsed {
            TelemetryQuery::SessionStats => IntrospectionOutcome::Ok {
                payload: self.session_stats_payload(),
            },
            TelemetryQuery::TopNTools { n, by } => {
                let mut entries: Vec<(String, u64)> =
                    self.state.tool_call_count().into_iter().collect();
                // Today only `Calls` is wired end-to-end (see TopNMetric
                // docstring). Tokens/Duration sort by call count as a
                // graceful degradation so the schema stays callable.
                entries.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                let take = (n as usize).min(entries.len());
                let top: Vec<Value> = entries
                    .into_iter()
                    .take(take)
                    .map(|(name, count)| json!({"tool": name, "count": count}))
                    .collect();
                let mut payload = json!({
                    "kind": "top_n_tools",
                    "by": top_n_metric_label(by),
                    "results": top,
                });
                if !matches!(by, TopNMetric::Calls) {
                    // The results are call-count-sorted, not token/duration
                    // sorted. Report the sort key honestly as `calls` so the
                    // model does not read these as tokens/duration-ranked;
                    // `degraded`/`degraded_reason` carry the requested metric.
                    payload
                        .as_object_mut()
                        .expect("payload is an object")
                        .insert("by".to_string(), Value::String("calls".to_string()));
                    payload
                        .as_object_mut()
                        .expect("payload is an object")
                        .insert("degraded".to_string(), Value::Bool(true));
                    payload
                        .as_object_mut()
                        .expect("payload is an object")
                        .insert(
                            "degraded_reason".to_string(),
                            Value::String(
                                "per-tool tokens/duration are not yet wired; results sorted by \
                                 calls as a graceful degradation"
                                    .to_string(),
                            ),
                        );
                }
                IntrospectionOutcome::Ok { payload }
            }
            TelemetryQuery::ToolUsageOverTime { window_secs } => {
                // Time-bucketed history would require per-call
                // timestamps that the writer side does not yet record.
                // Surface the current cumulative counts so the model
                // sees a useful answer; mark `degraded` so callers
                // can detect the partial implementation.
                let entries: HashMap<String, u64> = self.state.tool_call_count();
                IntrospectionOutcome::Ok {
                    payload: json!({
                        "kind": "tool_usage_over_time",
                        "window_secs": window_secs,
                        "totals": entries,
                        "degraded": true,
                        "degraded_reason":
                            "per-call timestamps not recorded; returning cumulative totals",
                    }),
                }
            }
            TelemetryQuery::ErrorsByProvider { provider } => {
                let errors = self.state.recent_errors(RECENT_ERRORS_FOR_QUERY);
                let filtered: Vec<Value> = errors
                    .into_iter()
                    .filter(|e| match &provider {
                        Some(p) => e.message.starts_with(&format!("{p}:")),
                        None => true,
                    })
                    .map(|e| {
                        json!({
                            "timestamp": e.timestamp.to_rfc3339(),
                            "message": e.message,
                        })
                    })
                    .collect();
                IntrospectionOutcome::Ok {
                    payload: json!({
                        "kind": "errors_by_provider",
                        "provider": provider,
                        "errors": filtered,
                    }),
                }
            }
        }
    }
}

/// Cap how many errors we pull through the `errors_by_provider` query.
/// Keeps the LLM's context bounded.
const RECENT_ERRORS_FOR_QUERY: usize = 32;

fn provider_status_label(s: ProviderHealthStatus) -> &'static str {
    match s {
        ProviderHealthStatus::Ok => "ok",
        ProviderHealthStatus::Degraded => "degraded",
        ProviderHealthStatus::Down => "down",
    }
}

fn top_n_metric_label(m: TopNMetric) -> &'static str {
    match m {
        TopNMetric::Calls => "calls",
        TopNMetric::Tokens => "tokens",
        TopNMetric::Duration => "duration",
    }
}

/// Resolver. The introspection backend is always available because
/// the underlying state is in-process — no env keys, no API tokens.
/// Returns an `Arc<dyn WaylandIntrospectionBackend>` ready to hand to
/// `WaylandStatusTool::new` / `WaylandTelemetryQueryTool::new`.
pub fn build_introspection_backend(
    state: Arc<dyn SessionStateReader>,
) -> Arc<dyn WaylandIntrospectionBackend> {
    Arc::new(LocalIntrospectionBackend::new(state))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_state::InMemorySessionState;
    use serde_json::Map;

    fn run<F: std::future::Future<Output = IntrospectionOutcome>>(f: F) -> IntrospectionOutcome {
        futures::executor::block_on(f)
    }

    fn ok_payload(o: IntrospectionOutcome) -> Value {
        match o {
            IntrospectionOutcome::Ok { payload } => payload,
            IntrospectionOutcome::Err { message } => panic!("expected ok, got err: {message}"),
        }
    }

    fn err_message(o: IntrospectionOutcome) -> String {
        match o {
            IntrospectionOutcome::Err { message } => message,
            IntrospectionOutcome::Ok { payload } => panic!("expected err, got ok: {payload}"),
        }
    }

    #[test]
    fn introspection_returns_current_token_counts() {
        let state = Arc::new(InMemorySessionState::new("claude-opus"));
        state.add_token_usage(100, 200);
        state.add_token_usage(50, 75);
        let backend = LocalIntrospectionBackend::new(state.clone());

        let out = run(backend.status(&StatusQuery::default()));
        let v = ok_payload(out);
        assert_eq!(v["token_count_input"].as_u64(), Some(150));
        assert_eq!(v["token_count_output"].as_u64(), Some(275));
        assert_eq!(v["model"].as_str(), Some("claude-opus"));
    }

    #[test]
    fn introspection_tool_usage_reflects_recent_calls() {
        let state = Arc::new(InMemorySessionState::default());
        state.record_tool_call("read");
        state.record_tool_call("read");
        state.record_tool_call("bash");
        let backend = LocalIntrospectionBackend::new(state);

        let v = ok_payload(run(backend.status(&StatusQuery::default())));
        let calls = v["tool_calls"]
            .as_object()
            .expect("tool_calls must be an object");
        assert_eq!(calls.get("read").and_then(Value::as_u64), Some(2));
        assert_eq!(calls.get("bash").and_then(Value::as_u64), Some(1));
    }

    #[test]
    fn introspection_recent_errors_returns_last_n() {
        let state = Arc::new(InMemorySessionState::default());
        state.push_error("first");
        state.push_error("second");
        state.push_error("third");
        state.push_error("fourth");
        let backend = LocalIntrospectionBackend::new(state.clone());

        // status() surfaces last 3.
        let v = ok_payload(run(backend.status(&StatusQuery::default())));
        let errs = v["recent_errors"]
            .as_array()
            .expect("recent_errors must be an array");
        assert_eq!(errs.len(), 3);
        assert_eq!(errs[0]["message"].as_str(), Some("fourth"));
        assert_eq!(errs[2]["message"].as_str(), Some("second"));

        // SessionStateReader::recent_errors honours the `n` argument.
        let reader: Arc<dyn SessionStateReader> = state;
        assert_eq!(reader.recent_errors(2).len(), 2);
        assert_eq!(reader.recent_errors(2)[0].message, "fourth");
    }

    #[test]
    fn introspection_provider_health_reflects_recent_calls() {
        let state = Arc::new(InMemorySessionState::default());
        state.set_provider_health("anthropic", ProviderHealthStatus::Ok);
        state.set_provider_health("openai", ProviderHealthStatus::Degraded);
        state.set_provider_health("anthropic", ProviderHealthStatus::Down);
        let backend = LocalIntrospectionBackend::new(state);

        let v = ok_payload(run(backend.status(&StatusQuery::default())));
        let providers = v["provider_health"]
            .as_array()
            .expect("provider_health must be an array");
        assert_eq!(providers.len(), 2);
        let map: HashMap<&str, &str> = providers
            .iter()
            .map(|p| {
                (
                    p["provider"].as_str().unwrap(),
                    p["status"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(map.get("anthropic"), Some(&"down"));
        assert_eq!(map.get("openai"), Some(&"degraded"));
    }

    #[test]
    fn telemetry_query_top_n_orders_by_count() {
        let state = Arc::new(InMemorySessionState::default());
        for _ in 0..5 {
            state.record_tool_call("read");
        }
        for _ in 0..2 {
            state.record_tool_call("bash");
        }
        for _ in 0..7 {
            state.record_tool_call("edit");
        }
        let backend = LocalIntrospectionBackend::new(state);

        let mut filter = Map::new();
        filter.insert("kind".to_string(), Value::String("top_n_tools".into()));
        filter.insert("n".to_string(), json!(2));
        filter.insert("by".to_string(), Value::String("calls".into()));
        let q = ToolTelemetryQuery {
            since: "1h".to_string(),
            filter: Some(filter),
            limit: 100,
        };
        let v = ok_payload(run(backend.telemetry_query(&q)));
        let results = v["results"].as_array().expect("results array");
        assert_eq!(results.len(), 2);
        assert_eq!(results[0]["tool"].as_str(), Some("edit"));
        assert_eq!(results[0]["count"].as_u64(), Some(7));
        assert_eq!(results[1]["tool"].as_str(), Some("read"));
        assert_eq!(results[1]["count"].as_u64(), Some(5));
        // Pure-calls path is not degraded.
        assert!(v.get("degraded").is_none());
    }

    #[test]
    fn telemetry_query_top_n_tokens_marks_degraded() {
        let state = Arc::new(InMemorySessionState::default());
        state.record_tool_call("read");
        let backend = LocalIntrospectionBackend::new(state);

        let mut filter = Map::new();
        filter.insert("kind".to_string(), Value::String("top_n_tools".into()));
        filter.insert("n".to_string(), json!(5));
        filter.insert("by".to_string(), Value::String("tokens".into()));
        let q = ToolTelemetryQuery {
            since: "1h".to_string(),
            filter: Some(filter),
            limit: 100,
        };
        let v = ok_payload(run(backend.telemetry_query(&q)));
        assert_eq!(v["degraded"].as_bool(), Some(true));
        // When degrading to call-count sorting, `by` must report `calls`
        // honestly (not `tokens`) so the model does not misread the rank.
        assert_eq!(v["by"].as_str(), Some("calls"));
        assert!(
            v["degraded_reason"]
                .as_str()
                .is_some_and(|s| s.contains("tokens")),
            "the requested metric should still be recoverable from degraded_reason"
        );
    }

    #[test]
    fn telemetry_query_rejects_unknown_kind() {
        // Direct deserialization check — `serde` MUST reject the bogus
        // variant before it can reach dispatch, which closes the
        // auto-approval injection surface (S-H6).
        let bad =
            serde_json::from_str::<TelemetryQuery>("{\"kind\":\"custom\",\"sql\":\"DROP TABLE\"}");
        assert!(bad.is_err());
        let bad2 = serde_json::from_str::<TelemetryQuery>("{\"kind\":\"shell_exec\"}");
        assert!(bad2.is_err());

        // Backend path — same outcome via from_tool_input.
        let state = Arc::new(InMemorySessionState::default());
        let backend = LocalIntrospectionBackend::new(state);
        let mut filter = Map::new();
        filter.insert("kind".to_string(), Value::String("custom".into()));
        filter.insert("sql".to_string(), Value::String("SELECT *".into()));
        let q = ToolTelemetryQuery {
            since: "1h".into(),
            filter: Some(filter),
            limit: 100,
        };
        let err = err_message(run(backend.telemetry_query(&q)));
        assert!(
            err.contains("invalid telemetry query"),
            "expected typed-enum rejection, got: {err}"
        );
    }

    #[test]
    fn telemetry_query_typed_enum_does_not_allow_sql_field() {
        // Even if the caller passes an extra `sql` field alongside a
        // known variant, serde drops it silently (the enum has no `sql`
        // slot). That is the structural guarantee S-H6 relies on:
        // there is no escape hatch into a free-form payload.
        let q: TelemetryQuery =
            serde_json::from_str(r#"{"kind":"session_stats","sql":"DROP TABLE users"}"#).unwrap();
        assert!(matches!(q, TelemetryQuery::SessionStats));

        // And the *reverse* shape — a `sql` field at the top with no
        // `kind` — must NOT deserialize.
        let bad = serde_json::from_str::<TelemetryQuery>(r#"{"sql":"DROP TABLE"}"#);
        assert!(bad.is_err());
    }

    #[test]
    fn telemetry_query_session_stats_default_when_no_filter() {
        let state = Arc::new(InMemorySessionState::new("m"));
        state.add_token_usage(12, 34);
        let backend = LocalIntrospectionBackend::new(state);
        let q = ToolTelemetryQuery {
            since: "1h".into(),
            filter: None,
            limit: 100,
        };
        let v = ok_payload(run(backend.telemetry_query(&q)));
        assert_eq!(v["token_count_input"].as_u64(), Some(12));
        assert_eq!(v["token_count_output"].as_u64(), Some(34));
        assert_eq!(v["active_model"].as_str(), Some("m"));
    }

    #[test]
    fn telemetry_query_errors_by_provider_filters() {
        let state = Arc::new(InMemorySessionState::default());
        state.push_error("anthropic: 429 rate limit");
        state.push_error("openai: 500 server");
        state.push_error("anthropic: 503 down");
        let backend = LocalIntrospectionBackend::new(state);

        let mut filter = Map::new();
        filter.insert(
            "kind".to_string(),
            Value::String("errors_by_provider".into()),
        );
        filter.insert("provider".to_string(), Value::String("anthropic".into()));
        let q = ToolTelemetryQuery {
            since: "1h".into(),
            filter: Some(filter),
            limit: 100,
        };
        let v = ok_payload(run(backend.telemetry_query(&q)));
        let errs = v["errors"].as_array().expect("errors array");
        assert_eq!(errs.len(), 2);
        for e in errs {
            assert!(e["message"].as_str().unwrap().starts_with("anthropic:"));
        }
    }

    #[test]
    fn build_introspection_backend_returns_arc() {
        let state: Arc<dyn SessionStateReader> = Arc::new(InMemorySessionState::default());
        let backend = build_introspection_backend(state);
        // Smoke-call status to confirm the wiring is live.
        let v = ok_payload(run(backend.status(&StatusQuery::default())));
        assert!(v["model"].is_string());
    }
}
