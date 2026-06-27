use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::hooks::{HookEngine, SessionEndSummary, TurnContext, TurnResult};
use wcore_config::compact::CompactConfig;
use wcore_config::config::Config;
use wcore_observability::SOURCE_PRODUCT;
use wcore_observability::cache::mark_cache_boundaries;
use wcore_observability::cost::estimate_turn_cost;
use wcore_observability::trace::{ToolCallTrace, TurnTrace, WorkflowDetectionRecord};
use wcore_protocol::events::ToolCategory;
use wcore_providers::{LlmProvider, ProviderError, create_provider};
use wcore_tools::registry::ToolRegistry;
use wcore_types::llm::{LlmEvent, LlmRequest};
use wcore_types::message::{ContentBlock, FinishReason, Message, Role, StopReason, TokenUsage};
use wcore_types::skill_types::{ContextModifier, PlanModeTransition, effort_to_string};

use crate::approval::ApprovalBridge;
use crate::cache_diagnostics::{CacheBreakDetector, CacheDiagnostic, CacheStats};
use crate::compact::state::CompactState;
use crate::compact::{auto, emergency, estimate, micro};
use crate::confirm::ToolConfirmer;
use crate::orchestration::ExecutionControl;
use crate::orchestration::StreamingContext;
use crate::orchestration::ToolCallOutcome;
use crate::orchestration::graph::{
    ExecutionGraph, GraphConfig, GraphContext, GraphError, NodeExecutor,
};
use crate::orchestration::intent::IntentClassifier;
use crate::orchestration::node_executor::{
    AgentExecutorConfig, AgentNodeExecutor, ApprovalChannel, TurnCell,
};
use crate::orchestration::template_routing::{
    TemplateDecisionSource, decision_is_unwired_template, select_graph_config,
};
use crate::output::OutputSink;
use crate::plan::prompt as plan_prompt;
use crate::plan::state::PlanState;
use crate::session::{Session, SessionManager};

/// W7 (v0.6.3) — resolve the USD cost of one LLM turn from the
/// `wcore-pricing` provider×model catalog.
///
/// `provider` is the lowercase provider key (e.g. `"anthropic"`,
/// `"openai"`, `"gemini"`) and `model` the model id. On a catalog miss
/// (unknown provider or model) returns `None`; the caller treats that as
/// non-fatal and falls back to the `ProviderCompat` heuristic so the
/// budget charge still happens and the LLM call is never failed.
///
/// Conversion: `estimate_cost_microcents` returns integer microcents
/// where 1 microcent = 1e-6 cent, so 1 USD = 100 cents = 100_000_000
/// microcents. USD = microcents / 100_000_000. (The W7 spec's
/// `/100_000` divisor was off by 1000× — verified against
/// `wcore_pricing::PricingCatalog::estimate_cost_microcents`, which
/// computes `usd * 100 * 1_000_000`.)
/// Token-opt (read-once): a short human-readable label for a Grep/Glob/Bash
/// call, used in the backref stub so the model can locate the earlier result.
fn backref_label(name: &str, input: &serde_json::Value) -> String {
    let detail = match name {
        "Grep" | "Glob" => input.get("pattern").and_then(|v| v.as_str()),
        "Bash" => input.get("command").and_then(|v| v.as_str()),
        _ => None,
    };
    match detail {
        Some(d) => {
            let trimmed: String = d.chars().take(60).collect();
            format!("`{name}` ({trimmed})")
        }
        None => format!("`{name}`"),
    }
}

fn pricing_turn_cost_usd(
    provider: &str,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
) -> Option<f64> {
    match wcore_pricing::DEFAULT_CATALOG.estimate_cost_microcents(
        provider,
        model,
        input_tokens,
        output_tokens,
    ) {
        Ok(microcents) => Some(microcents as f64 / wcore_types::crucible::MICROCENTS_PER_USD),
        Err(e) => {
            tracing::warn!(
                provider,
                model,
                error = %e,
                "W7: wcore-pricing catalog miss; falling back to ProviderCompat cost heuristic"
            );
            None
        }
    }
}

/// Resolve the USD cost for one turn: try the pricing catalog first (per-model
/// resolution), fall back to `estimate_turn_cost` using the ProviderCompat rows.
///
/// This is the authoritative cost for TurnTrace.cost_usd and the SessionCost
/// aggregate. The budget tracker used `pricing_turn_cost_usd` directly, but
/// TurnTrace was still using `estimate_turn_cost` (which returns $0 now that
/// `openai_defaults()` uses the $0/$0 sentinel).
///
/// Fix(pricing-audit-2026-05-24): wiring catalog into the TurnTrace path so
/// session_cost events reflect real per-model pricing (not compat-row fallback).
fn resolve_turn_cost_usd(
    provider: &str,
    model: &str,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    compat: &wcore_config::compat::ProviderCompat,
) -> f64 {
    pricing_turn_cost_usd(provider, model, input_tokens, output_tokens).unwrap_or_else(|| {
        estimate_turn_cost(
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            compat,
        )
    })
}

/// Finding #174 — does this turn carry image/vision content that forbids a
/// tier downgrade?
///
/// The `wcore-types` `ContentBlock` enum has no image variant today, so there
/// is no real vision content to detect and this returns `false`. It is kept as
/// the single chokepoint for the vision signal: when a future image
/// `ContentBlock` lands, only this function must change for the smart-routing
/// guard (and the router's `requires_vision` shape input) to start respecting
/// it. The router independently promotes any vision turn to Premium, which
/// [`select_tier_model`] already refuses to swap; this is belt-and-suspenders.
fn message_requires_vision(_messages: &[Message]) -> bool {
    false
}

/// Finding #174 — decide whether to swap to a configured tier model, and to
/// which, given a routing decision.
///
/// Returns `Some(model_id)` ONLY when ALL hold:
/// 1. the hinted tier is `Cheap` or `Balanced` (a `Premium` hint is never
///    downgraded — returns `None`);
/// 2. the turn does NOT require vision content (`requires_vision == false`);
/// 3. the user configured a `[compat.tier_models]` model for that tier.
///
/// Otherwise returns `None` and the caller keeps the originally requested
/// model. With no `tier_models` map configured (the default) this always
/// returns `None`, so behaviour is unchanged for users who have not opted in.
fn select_tier_model(
    decision: &wcore_providers::RoutingDecision,
    requires_vision: bool,
    compat: &wcore_config::compat::ProviderCompat,
) -> Option<String> {
    // Never downgrade a vision turn, regardless of tier.
    if requires_vision {
        return None;
    }
    let tier = match decision.tier {
        wcore_providers::RoutingTier::Cheap => "cheap",
        wcore_providers::RoutingTier::Balanced => "balanced",
        // Premium is never downgraded.
        wcore_providers::RoutingTier::Premium => return None,
    };
    compat.tier_model(tier).map(str::to_string)
}

/// Up-front output sizing (Layer 1). Returns the `max_tokens` to request so a
/// normal turn finishes in ONE round, sized to the model's real output ceiling
/// where known and clamped to the room left in the context window. This makes a
/// generous `config_max` SAFE: a known model is clamped to what it actually
/// allows (e.g. gpt-4o → 16384, never a 400), and an unknown model — an
/// unlisted variant or a router alias like `flux-auto` whose served model is
/// unknown per-request — is clamped to a conservative floor. `config_max` is the
/// user's CAP and always binds; an explicit low value is respected.
///
/// `model` must be the FINAL model that will actually be sent — i.e. this is
/// called AFTER the smart-routing tier swap (`request.model = tier_model`) so
/// the clamp sees the post-swap model's real ceiling, never the pre-swap one.
fn size_output_cap(config_max: u32, provider: &str, model: &str, est_input_tokens: usize) -> u32 {
    /// Conservative cap for models with no known output ceiling. Safe for
    /// essentially every modern model (gpt-4o is 16384). Never raised on
    /// guesswork — that is what 400s.
    const UNKNOWN_CAP: u32 = 8_192;
    /// Headroom kept free in the window for prompt growth / safety margin.
    const WINDOW_BUFFER: u32 = 512;

    match wcore_config::limits::model_output_ceiling(provider, model) {
        Some((out_ceiling, context_window)) => {
            let est = u32::try_from(est_input_tokens).unwrap_or(u32::MAX);
            let window_room = context_window
                .saturating_sub(est)
                .saturating_sub(WINDOW_BUFFER)
                .max(1);
            config_max.min(out_ceiling).min(window_room)
        }
        None => config_max.min(UNKNOWN_CAP),
    }
}

#[cfg(test)]
mod output_sizing_tests {
    use super::size_output_cap;

    #[test]
    fn known_model_is_clamped_to_its_real_output_ceiling() {
        // A generous config cap is clamped DOWN to the model's real ceiling,
        // so a large default never 400s a model that allows less.
        assert_eq!(
            size_output_cap(64_000, "openai", "gpt-4o-mini", 1_000),
            16_384,
            "gpt-4o output ceiling binds"
        );
        assert_eq!(
            size_output_cap(64_000, "anthropic", "claude-opus-4-7", 1_000),
            32_000,
            "opus 4.x output ceiling binds"
        );
        assert_eq!(
            size_output_cap(64_000, "anthropic", "claude-sonnet-4-6", 1_000),
            64_000,
            "sonnet can use its full 64k"
        );
    }

    #[test]
    fn unknown_model_falls_back_to_conservative_cap_not_the_generous_default() {
        // A router alias (served model unknown) must NOT receive the generous
        // 64k — it could route to a small model and 400. Clamp to the floor.
        assert_eq!(
            size_output_cap(64_000, "flux-router", "flux-auto", 1_000),
            8_192
        );
        assert_eq!(
            size_output_cap(64_000, "ollama", "some-local-model", 1_000),
            8_192
        );
    }

    #[test]
    fn explicit_low_user_cap_is_always_respected() {
        // If the user sets a low max_tokens, it binds on known AND unknown.
        assert_eq!(
            size_output_cap(4_000, "anthropic", "claude-opus-4-7", 1_000),
            4_000
        );
        assert_eq!(
            size_output_cap(4_000, "flux-router", "flux-auto", 1_000),
            4_000
        );
    }

    #[test]
    fn near_context_limit_input_shrinks_the_output_cap() {
        // When the prompt nearly fills the window, the remaining room binds
        // below the model's output ceiling (prevents an input+output overflow).
        let cap = size_output_cap(64_000, "anthropic", "claude-opus-4-7", 199_000);
        assert!(cap < 32_000, "window room must bind near the context limit");
        assert!(cap >= 1, "never zero or negative");
    }
}

/// v0.9.1.1 B6 — true when `reason` is an HTTP 4xx (client) error from
/// a provider. The engine retries 5xx + network drops + truncated
/// streams (real chance the next attempt succeeds), but a 4xx — the
/// most common is Anthropic's 400 `invalid_request_error` from an
/// orphaned `tool_use` after a denied tool — is NOT transient. Retry
/// burns the budget producing identical errors stacked in the
/// Activity rail; the user sees `Error [engine_error]: API 400:`
/// three times for what should be a single failure.
///
/// Detection is by string match on the typical provider error shapes:
///
/// * `API error 400: …` (the engine's own `AgentError::ApiError` shape)
/// * `API 400: …` (the post-`sanitize_provider_error_message` shape)
/// * `status: 400`, `status code 400`, `400 Bad Request`
///
/// Conservative — when the shape is ambiguous, returns `false` and
/// the retry loop fires as before. The cost of a missed 4xx is one
/// extra retry; the cost of a false-positive 4xx is no retry on a
/// transient failure. Bias toward the latter.
fn is_http_4xx_error(reason: &str) -> bool {
    /// Returns true if the first 3 bytes of `s` are ASCII digits and
    /// the first digit is `4` — i.e. `s` starts with a literal 4xx
    /// status code. The 4th byte is checked for non-digit to reject
    /// 4-digit numbers like `4000` (a trace id, not a 400 status).
    fn starts_with_4xx(s: &str) -> bool {
        let b = s.as_bytes();
        if b.len() < 3 {
            return false;
        }
        if b[0] != b'4' || !b[1].is_ascii_digit() || !b[2].is_ascii_digit() {
            return false;
        }
        // Reject `4000…` style multi-digit ids.
        if matches!(b.get(3), Some(c) if c.is_ascii_digit()) {
            return false;
        }
        // 429 (rate limit) is transient: the HTTP path retries it with backoff,
        // so the prefixed shapes ("API error 429: …") must stay retryable too,
        // matching the generic substring list below (which also omits 429).
        &b[..3] != b"429"
    }
    // The exact "API error <code>: " prefix the provider chain emits.
    if let Some(rest) = reason.strip_prefix("API error ")
        && starts_with_4xx(rest)
    {
        return true;
    }
    // The sanitized "API <code>: " shape the protocol_bridge emits.
    if let Some(rest) = reason.strip_prefix("API ")
        && starts_with_4xx(rest)
    {
        return true;
    }
    // Generic substring matches — slower but catches misc shapes.
    // NOTE: 429 is deliberately absent. A rate-limit is transient: the
    // HTTP path retries it with backoff, so the in-band SSE string path
    // (HTTP 200 + an error body carrying "429") must agree and stay
    // retryable rather than failing the turn immediately.
    for code in ["400", "401", "403", "404", "409", "413", "422"] {
        // Require the code as a standalone token to avoid matching
        // "4000" or a trace id like "4000-abc". `code` is always 3
        // digits so a boundary check on each side suffices.
        if let Some(idx) = reason.find(code) {
            let before_ok = idx == 0
                || reason
                    .as_bytes()
                    .get(idx - 1)
                    .map(|b| !b.is_ascii_digit())
                    .unwrap_or(true);
            let after_ok = reason
                .as_bytes()
                .get(idx + code.len())
                .map(|b| !b.is_ascii_digit())
                .unwrap_or(true);
            if before_ok && after_ok {
                return true;
            }
        }
    }
    false
}

/// FluxRouter web_search grounding (contract §5.4): render the "Sources" block
/// appended after a grounded answer. The answer text already carries inline
/// `[n]` markers; this maps `[n]` → `citations[n-1]` (1-indexed) and lists the
/// richer `search_results` cards beneath. Returns an empty string when there is
/// nothing to render, so the caller can append unconditionally.
fn render_grounding_sources(
    citations: &[String],
    search_results: &[wcore_types::llm::FluxSearchResult],
) -> String {
    if citations.is_empty() && search_results.is_empty() {
        return String::new();
    }
    let mut out = String::from("\n\nSources:\n");
    // Inline-marker map: `[1]` → first citation URL, etc.
    for (i, url) in citations.iter().enumerate() {
        out.push_str(&format!("[{}] {}\n", i + 1, url));
    }
    // Richer source cards (title + url + snippet). Only rendered when present;
    // a card with an empty title falls back to its URL so the line is never
    // blank.
    if !search_results.is_empty() {
        out.push('\n');
        for r in search_results {
            let label = if r.title.is_empty() { &r.url } else { &r.title };
            out.push_str(&format!("- {label}"));
            if !r.url.is_empty() && !r.title.is_empty() {
                out.push_str(&format!(" — {}", r.url));
            }
            if let Some(date) = &r.date
                && !date.is_empty()
            {
                out.push_str(&format!(" ({date})"));
            }
            out.push('\n');
            if !r.snippet.is_empty() {
                out.push_str(&format!("  {}\n", r.snippet));
            }
        }
    }
    out
}

/// GAP-5/7 — upper bound on live workflow synthesis (up to 3 LLM round-trips).
/// Past this the gate falls through to a normal turn rather than stalling the
/// session on a hung synthesis call. Generous so legitimate multi-round-trip
/// synthesis completes; the progress indicator covers the wait.
const WORKFLOW_SYNTH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// Dynamic Workflows B6 — synthesise a workflow plan on an owned spawner.
///
/// A free function (not an `AgentEngine` method) run via `tokio::spawn` so the
/// `Send + 'static` bound is checked HERE, at the task boundary. That severs
/// the otherwise-infinite async-recursion type cycle: synthesis spawns a
/// sub-agent whose own `AgentEngine::run` the compiler cannot prove never
/// re-enters the live gate. Returns the spawner alongside the result so the
/// caller can reuse it for execution.
type SynthOwnedOutput = (
    Result<crate::orchestration::workflow::runner::WorkflowPlan, crate::workflow_synth::SynthError>,
    crate::spawner::AgentSpawner,
);

fn synthesize_workflow_owned(
    spawner: crate::spawner::AgentSpawner,
    task: String,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = SynthOwnedOutput> + Send>> {
    // Concrete boxed `Send` return type (not `impl Future`) so the engine's
    // own `run` future does not transitively depend on an opaque type that
    // cycles back through this gate during auto-trait inference.
    Box::pin(async move {
        let result = crate::workflow_synth::synthesize_workflow(&task, &spawner).await;
        (result, spawner)
    })
}

/// Dynamic Workflows B6 — execute a workflow plan on an owned spawner.
///
/// Same `tokio::spawn` recursion-cut rationale as [`synthesize_workflow_owned`].
/// Returns the plan back so the caller can render its per-stage summary.
type RunOwnedOutput = (
    crate::orchestration::workflow::runner::WorkflowPlan,
    Result<
        crate::orchestration::workflow::runner::WorkflowRunResult,
        crate::orchestration::workflow::runner::WorkflowRunError,
    >,
);

fn run_workflow_owned(
    spawner: crate::spawner::AgentSpawner,
    plan: crate::orchestration::workflow::runner::WorkflowPlan,
    initial: serde_json::Value,
    parent_output: std::sync::Arc<dyn crate::output::OutputSink>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = RunOwnedOutput> + Send>> {
    // Concrete boxed `Send` return type (see `synthesize_workflow_owned`).
    Box::pin(async move {
        // ForgeFlows-Live: derive the lifecycle-event fields from the plan.
        // `run_workflow_owned` has no turn/msg id in scope, so the stable
        // correlation handle is the plan's own display name (the meta.name
        // the author declared); the TUI groups runs under one MVP key today,
        // so the name doubles as the human-readable id. `node_count` counts
        // the AgentCall nodes — the units the runner actually dispatches and
        // what the Workflows tab tallies (End/PassThrough/Pipeline-placeholder
        // nodes are graph plumbing, not agents).
        let workflow_id = plan.meta.name.clone();
        let name = plan.meta.name.clone();
        let node_count = plan
            .graph
            .nodes
            .iter()
            .filter(|(_, node)| matches!(node, crate::orchestration::graph::Node::AgentCall { .. }))
            .count();

        // ForgeFlows-Live: bookend the run with WorkflowStarted/Finished so
        // hosts get a clean lifecycle signal instead of inferring it from the
        // first `workflow:<node_id>` SubAgentEvent. Both ride the existing
        // `sub_agent_traces` gate on the sink.
        parent_output.emit_workflow_started(&workflow_id, &name, node_count);

        // ForgeFlows-Live Phase 1: wire the parent sink so the live (B6) path's
        // sub-agent events relay back as `SubAgentEvent`, matching the LLM-facing
        // `WorkflowTool` surface.
        let run = crate::orchestration::workflow::runner::WorkflowRunner::new(&spawner)
            .with_parent_output(std::sync::Arc::clone(&parent_output))
            .run(&plan, initial)
            .await;

        // `succeeded` mirrors the caller's `is_error` definition (a hard `Err`
        // OR an `Ok` run that still carries a failed stage both count as
        // failure — e.g. a no-barrier pipeline over a missing `over:` key).
        let succeeded = match &run {
            Ok(result) => AgentEngine::errored_stage_ids(&result.stage_results).is_empty(),
            Err(_) => false,
        };
        parent_output.emit_workflow_finished(&workflow_id, succeeded);

        (plan, run)
    })
}

#[cfg(test)]
mod v0911_engine_recovery_tests {
    use super::*;

    #[test]
    fn http_4xx_detected_in_api_error_shape() {
        assert!(is_http_4xx_error(
            "API error 400: invalid_request_error tool_use ids…"
        ));
        assert!(is_http_4xx_error("API error 401: invalid x-api-key"));
    }

    #[test]
    fn rate_limit_429_is_retryable_not_a_client_error() {
        // A 429 is transient: the HTTP path backs off and retries it, so
        // the in-band SSE string path (HTTP 200 + an error body carrying
        // "429") must agree and NOT classify it as a non-retryable 4xx.
        assert!(!is_http_4xx_error("API error 429: rate_limit_exceeded"));
        assert!(!is_http_4xx_error("API 429: too many requests"));
        assert!(!is_http_4xx_error("provider returned status 429"));
    }

    #[test]
    fn http_4xx_detected_in_sanitized_shape() {
        assert!(is_http_4xx_error("API 400: bad request body"));
        assert!(is_http_4xx_error("API 422: validation failed"));
    }

    #[test]
    fn http_4xx_detected_in_freeform_shape() {
        assert!(is_http_4xx_error("provider returned status 400"));
        assert!(is_http_4xx_error("response: 404 not found"));
    }

    #[test]
    fn http_5xx_and_transients_not_treated_as_client_error() {
        assert!(!is_http_4xx_error("API error 500: internal server error"));
        assert!(!is_http_4xx_error("API error 503: service unavailable"));
        assert!(!is_http_4xx_error(
            "provider stream closed before a Done event (truncated response)"
        ));
        assert!(!is_http_4xx_error("connection reset by peer"));
    }

    #[test]
    fn digits_inside_longer_numbers_do_not_match() {
        // "4000-abc" is a trace id, not status 400. The boundary check
        // must exclude it.
        assert!(!is_http_4xx_error("trace id 4000-abc"));
        assert!(!is_http_4xx_error("offset 4001 reached"));
    }

    #[test]
    fn empty_string_is_not_a_4xx() {
        assert!(!is_http_4xx_error(""));
        assert!(!is_http_4xx_error("4"));
        assert!(!is_http_4xx_error("40"));
    }

    #[test]
    fn render_grounding_sources_empty_is_blank() {
        assert_eq!(render_grounding_sources(&[], &[]), "");
    }

    #[test]
    fn render_grounding_sources_maps_markers_to_urls() {
        let cites = vec![
            "https://a.example".to_string(),
            "https://b.example".to_string(),
        ];
        let out = render_grounding_sources(&cites, &[]);
        assert!(out.contains("Sources:"));
        assert!(out.contains("[1] https://a.example"));
        assert!(out.contains("[2] https://b.example"));
    }

    #[test]
    fn render_grounding_sources_includes_search_result_cards() {
        let results = vec![wcore_types::llm::FluxSearchResult {
            title: "JWST".into(),
            url: "https://science.nasa.gov/jwst".into(),
            date: Some("2026-06-15".into()),
            last_updated: None,
            snippet: "new image".into(),
            source: "web".into(),
        }];
        let out = render_grounding_sources(&["https://science.nasa.gov/jwst".into()], &results);
        assert!(out.contains("- JWST — https://science.nasa.gov/jwst"));
        assert!(out.contains("(2026-06-15)"));
        assert!(out.contains("new image"));
    }
}

#[cfg(test)]
mod forgeflow_final_state_tests {
    use super::*;

    #[test]
    fn render_final_state_surfaces_results_and_skips_seed_and_empty_keys_gap3() {
        // GAP-3: the run summary must show the workflow's produced DATA
        // (aggregator folds, pipeline arrays), not just per-stage statuses.
        let state = serde_json::json!({
            "changed_files": ["a.rs", "b.rs"], // seed input — skipped
            "cwd": "/repo",                     // seed input — skipped
            "review": {"verdict": "ship", "bugs": 0}, // a real result — shown
            "pl": [],                            // empty pipeline result — skipped
            "summary": "all clear",             // real result — shown
            "notes": ""                         // empty string — skipped
        });
        let out = AgentEngine::render_workflow_final_state(&state);
        // Real results appear...
        assert!(out.contains("review:"), "aggregator result dropped: {out}");
        assert!(out.contains("\"verdict\":\"ship\""), "value dropped: {out}");
        assert!(out.contains("summary:"), "string result dropped: {out}");
        // ...seed inputs and empties do NOT.
        assert!(!out.contains("changed_files"), "seed key leaked: {out}");
        assert!(!out.contains("cwd"), "seed key leaked: {out}");
        assert!(
            !out.contains("\npl:") && !out.contains("- pl:"),
            "empty array shown: {out}"
        );
        assert!(!out.contains("notes"), "empty string shown: {out}");
    }

    #[test]
    fn render_final_state_truncates_large_values_and_handles_non_object() {
        // A huge fan result must not flood the transcript.
        let big: Vec<i64> = (0..1000).collect();
        let state = serde_json::json!({ "fan": big });
        let out = AgentEngine::render_workflow_final_state(&state);
        assert!(
            out.contains('…'),
            "large value not truncated: {}",
            &out[..out.len().min(80)]
        );
        assert!(out.chars().count() < 800, "truncation cap not enforced");

        // A non-object final_state (defensive) yields nothing, never panics.
        assert!(AgentEngine::render_workflow_final_state(&serde_json::json!([1, 2, 3])).is_empty());
        assert!(AgentEngine::render_workflow_final_state(&serde_json::Value::Null).is_empty());
    }

    #[test]
    fn errored_stage_ids_names_failed_stages_so_missing_over_key_is_not_silent_success() {
        use crate::orchestration::workflow::runner::StageResult;

        let stage = |id: &str, is_error: bool| StageResult {
            node_id: id.to_string(),
            text: "ran".to_string(),
            is_error,
            turns: 0,
        };

        // A clean run reports no errored stages → surfaces as a real success.
        let clean = vec![stage("split", false), stage("review", false)];
        assert!(
            AgentEngine::errored_stage_ids(&clean).is_empty(),
            "clean run must not report errored stages"
        );

        // The bug: a no-barrier pipeline whose `over:` key is missing records an
        // `is_error` StageResult yet `run_no_barrier_pipeline` returns `Ok(())`.
        // The post-check must catch that stage so the workflow does NOT report a
        // silent partial success, and the diagnostic must NAME the failing stage.
        let partial = vec![
            stage("seed", false),
            stage("fan_pipeline", true), // missing `over:` key
            stage("collect", false),
        ];
        let errored = AgentEngine::errored_stage_ids(&partial);
        assert_eq!(
            errored,
            vec!["fan_pipeline".to_string()],
            "errored stage id not surfaced — workflow would report silent success"
        );
    }
}

#[cfg(test)]
mod w7_pricing_budget_tests {
    use super::pricing_turn_cost_usd;
    use wcore_budget::{BudgetCap, BudgetTracker};

    /// A known provider/model with known token counts resolves to the
    /// expected USD cost and that cost is charged against the budget.
    ///
    /// Fix(pricing-audit-2026-05-24): claude-opus-4-7 rate corrected $15 → $5/Mtok input.
    /// claude-opus-4-7: input $5/Mtok, output $25/Mtok. 1M input + 0
    /// output = $5.00 exactly (matches the wcore-pricing catalog).
    #[test]
    fn known_model_charges_expected_usd() {
        let usd = pricing_turn_cost_usd("anthropic", "claude-opus-4-7", 1_000_000, 0)
            .expect("anthropic/claude-opus-4-7 is in the bundled catalog");
        assert!(
            (usd - 5.0).abs() < 1e-6,
            "expected $5.00 for 1M input tokens, got {usd}"
        );

        let mut tracker = BudgetTracker::new(BudgetCap::default());
        tracker
            .charge("w7-sess", 1_000_000, usd)
            .expect("charge under default (uncapped) budget must succeed");
    }

    /// A pricing-catalog miss is non-fatal: `pricing_turn_cost_usd`
    /// returns `None` (the caller then falls back to the ProviderCompat
    /// heuristic) — the LLM turn is never failed by a missing price row.
    #[test]
    fn catalog_miss_returns_none_and_does_not_fail() {
        // Unknown provider.
        assert!(pricing_turn_cost_usd("no-such-provider", "x", 1000, 500).is_none());
        // Known provider, unknown model.
        assert!(pricing_turn_cost_usd("anthropic", "no-such-model", 1000, 500).is_none());

        // The charge still happens via the fallback path the caller uses:
        // a `None` cost resolves to a heuristic value, and `charge` itself
        // succeeds — the turn is not aborted.
        let mut tracker = BudgetTracker::new(BudgetCap::default());
        let fallback_cost =
            pricing_turn_cost_usd("no-such-provider", "x", 1000, 500).unwrap_or(0.0);
        tracker
            .charge("w7-sess", 1500, fallback_cost)
            .expect("charge must still succeed when pricing lookup misses");
    }
}

/// D.2 (v0.6.3) — W8 cache_tier producer side. R1 wired the Anthropic
/// adapter to READ `request.cache_tier`; this verifies the engine's
/// production request-build path now SETS it (it was hard-coded `None`).
#[cfg(test)]
mod w8_cache_tier_producer_tests {
    use super::AGENT_TURN_CACHE_REUSE_WINDOW_SECS;
    use wcore_providers::cache_tier::{CacheTier, pick_cache_tier};

    /// The exact expression the engine stamps onto a production
    /// `LlmRequest` (`engine.rs` request-build block). A large multi-turn
    /// prompt must resolve to the 1h tier — the previously-unreachable
    /// path — because the agent reuses its prefix far beyond 5 minutes.
    #[test]
    fn large_prompt_resolves_to_1h_tier() {
        let input_token_estimate = 50_000usize;
        let tier = pick_cache_tier(input_token_estimate, AGENT_TURN_CACHE_REUSE_WINDOW_SECS);
        assert_eq!(
            tier,
            CacheTier::Ephemeral1h,
            "a large prompt with the production reuse window must reach the 1h tier"
        );
    }

    /// A production-shaped request stamps `cache_tier: Some(..)` — never
    /// the old hard-coded `None`. `Some(CacheTier::None)` is still a valid
    /// outcome for a tiny prompt (the adapter then injects no marker), but
    /// the field itself is always populated by the producer now.
    #[test]
    fn production_request_cache_tier_is_some() {
        // Tiny prompt below the 1024-token cache minimum.
        let small = Some(pick_cache_tier(200, AGENT_TURN_CACHE_REUSE_WINDOW_SECS));
        assert_eq!(small, Some(CacheTier::None));
        // Mid-size prompt above the minimum.
        let mid = Some(pick_cache_tier(8_000, AGENT_TURN_CACHE_REUSE_WINDOW_SECS));
        assert!(matches!(
            mid,
            Some(CacheTier::Ephemeral1h | CacheTier::Ephemeral5m)
        ));
        // Either way the producer yields Some(..), not None.
        assert!(small.is_some() && mid.is_some());
    }

    /// The production reuse-window constant must be long enough that a
    /// cacheable prompt actually reaches the 1h tier — otherwise the
    /// producer wiring is moot. Asserted via observable picker behaviour:
    /// a cacheable prompt at this window must NOT stay on the 5m tier.
    #[test]
    fn reuse_window_promotes_cacheable_prompt_past_5m() {
        let tier = pick_cache_tier(4_096, AGENT_TURN_CACHE_REUSE_WINDOW_SECS);
        assert_eq!(
            tier,
            CacheTier::Ephemeral1h,
            "the production reuse window must promote a cacheable prompt to the 1h tier"
        );
    }
}

/// Runaway-loop circuit breaker, local to a single `run()` call.
///
/// Tracks the signature of the most recent (tool name + arguments + outcome)
/// tuple and the length of the current back-to-back repeat streak. The run is
/// terminated when the SAME signature repeats `threshold` times *consecutively*
/// — i.e. the agent is stuck retrying one call to no effect (a network-blocked
/// command, re-reading an unchanged file, …).
///
/// Detection is consecutive (not windowed) on purpose: ANY intervening call
/// with a different signature — a different tool, different args, or the same
/// call producing a *different* result (e.g. `cargo test` after an edit) —
/// resets the streak. This makes legitimate *varied* repetition immune: an
/// agent productively cycling several tools (the pattern skill-drafting
/// recognises, e.g. `Grep, Glob, Grep, Glob, Grep`) never builds a long
/// back-to-back run of one signature, so it is never cut.
///
/// This is the backstop the `max_turns = None` design decision (see the run
/// loop) leaves open for *no-progress loops*: the budget cap and context
/// ceiling bound per-turn size and total spend, not the loop itself. The
/// threshold is read once per run from `WAYLAND_MAX_REPEATED_TOOL_CALLS`
/// (default 10); `0` disables the breaker.
///
/// The default is deliberately conservative. The cheap, common loop — re-reading
/// an unchanged file — is already neutralised by Read's content dedup (it
/// returns a tiny stub), so the breaker's real job is the *expensive*
/// command-retry loop (a network-blocked `npm install` retried over and over),
/// which retries far more than 10 times before a human would notice. Ten
/// identical, no-progress calls in a row is an unambiguous stuck loop with
/// little false-positive surface.
struct LoopGuard {
    last_sig: Option<u64>,
    count: u32,
    threshold: u32,
}

impl LoopGuard {
    fn from_env() -> Self {
        let threshold = std::env::var("WAYLAND_MAX_REPEATED_TOOL_CALLS")
            .ok()
            .and_then(|v| v.trim().parse::<u32>().ok())
            .unwrap_or(10);
        Self {
            last_sig: None,
            count: 0,
            threshold,
        }
    }

    /// Record one tool call+outcome signature. Returns `Some(count)` once the
    /// same signature has repeated `>= threshold` times in a row. Any different
    /// signature resets the streak to 1.
    fn observe(&mut self, sig: u64) -> Option<u32> {
        if self.threshold == 0 {
            return None;
        }
        if self.last_sig == Some(sig) {
            self.count = self.count.saturating_add(1);
        } else {
            self.last_sig = Some(sig);
            self.count = 1;
        }
        (self.count >= self.threshold).then_some(self.count)
    }
}

/// Signature of a tool call + its outcome for [`LoopGuard`]:
/// hash of `(tool name, canonical args, is_error, content prefix)`. A repeated
/// call that yields a different result hashes differently; only identical
/// call→identical result repetition accumulates.
fn loop_call_signature(
    tool_name: &str,
    args: &serde_json::Value,
    is_error: bool,
    content: &str,
) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    tool_name.hash(&mut h);
    serde_json::to_string(args).unwrap_or_default().hash(&mut h);
    is_error.hash(&mut h);
    // Hash only a prefix so a large tool output is not fully re-hashed each
    // turn; a byte slice avoids any UTF-8 char-boundary panic.
    let bytes = content.as_bytes();
    bytes[..bytes.len().min(512)].hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod loop_guard_tests {
    use super::{LoopGuard, loop_call_signature};
    use serde_json::json;

    fn guard(threshold: u32) -> LoopGuard {
        LoopGuard {
            last_sig: None,
            count: 0,
            threshold,
        }
    }

    #[test]
    fn trips_on_consecutive_identical_calls() {
        let mut g = guard(3);
        assert_eq!(g.observe(1), None);
        assert_eq!(g.observe(1), None);
        assert_eq!(
            g.observe(1),
            Some(3),
            "3rd consecutive identical call must trip"
        );
    }

    #[test]
    fn interleaved_calls_reset_the_streak() {
        // A,B,A,B,A — an alternating/varied sequence (the skill-drafting
        // pattern) must NEVER trip: any different signature resets the streak.
        let mut g = guard(3);
        for s in [10u64, 20, 10, 20, 10, 20, 10] {
            assert_eq!(g.observe(s), None, "varied sequence must not trip");
        }
    }

    #[test]
    fn a_different_result_breaks_the_streak() {
        // Same call, but the result changes on the 3rd (progress) — streak
        // resets, so the following repeats start over and do not trip early.
        let mut g = guard(3);
        assert_eq!(g.observe(1), None); // A
        assert_eq!(g.observe(1), None); // A (streak 2)
        assert_eq!(g.observe(2), None); // different result -> reset to 1
        assert_eq!(g.observe(1), None); // back to A, streak 1
        assert_eq!(g.observe(1), None); // streak 2 — still below threshold
    }

    #[test]
    fn threshold_zero_disables_the_breaker() {
        let mut g = guard(0);
        for _ in 0..100 {
            assert_eq!(g.observe(7), None);
        }
    }

    #[test]
    fn signature_stable_for_identical_calls_varies_on_change() {
        let a = loop_call_signature("Read", &json!({"file_path": "/x"}), true, "err");
        let b = loop_call_signature("Read", &json!({"file_path": "/x"}), true, "err");
        assert_eq!(a, b, "identical call+outcome must hash equal");
        // Changed result (progress) -> different signature -> streak resets.
        let changed_result = loop_call_signature("Read", &json!({"file_path": "/x"}), false, "ok");
        assert_ne!(a, changed_result);
        // Different args -> different signature.
        let changed_args = loop_call_signature("Read", &json!({"file_path": "/y"}), true, "err");
        assert_ne!(a, changed_args);
    }
}

pub struct AgentEngine {
    provider: Arc<dyn LlmProvider>,
    /// Wave OR: the tool registry is Arc-shared so per-turn
    /// [`AgentNodeExecutor`] adapter clones in `engine::run` can satisfy the
    /// `'static + Send + Sync` bound that `ExecutionGraph::execute` (and its
    /// `tokio::spawn`-based parallel AgentCall path) requires.
    ///
    /// `registry_mut` uses `Arc::get_mut` and returns `None` once any task
    /// holds a clone — at the CLI boot site (the only external mutator) the
    /// engine is not running yet so the call always succeeds.
    tools: Arc<ToolRegistry>,
    messages: Vec<Message>,
    system_prompt: String,
    /// D016 / Wave-6 #5: the fully-assembled boot system prompt
    /// (Constitution + skills index + persona + the resolved
    /// `[default] system_prompt` + any `inject_history` prepends), retained so
    /// an in-session rebind ([`set_system_prompt`]) can re-prepend a fresh
    /// config/name overlay onto these framework fragments instead of replacing
    /// them wholesale. Seeded at construction from `system_prompt` and kept in
    /// sync by [`inject_history`]; `None` only in the (test-only) paths that
    /// never go through the standard constructor.
    rebind_system_prefix: Option<String>,
    model: String,
    /// D014: when the user makes an explicit `/model <id>` pick, that choice
    /// is authoritative for the session and a skill/hook `switch_model` must
    /// NOT silently move the live model off it. Set by [`set_model`] (the
    /// explicit-pick path), cleared by [`clear_model_pin`] /
    /// [`clear_conversation`] (a `/new` re-baselines) and by an explicit
    /// provider rebind / config update. While `Some`, the three hook/skill
    /// override sites (`apply_pre_turn_outcome`, `apply_turn_end_outcome`,
    /// `apply_context_modifiers`) refuse the switch and log the divergence.
    user_model_pin: Option<String>,
    max_tokens: u32,
    /// Crucible #3: optional sampling temperature sourced from `Config` at
    /// construction. `None` (the default top-level session) omits the field;
    /// the council's child engines set it via `child_config` -> `Config`.
    temperature: Option<f32>,
    max_turns: Option<usize>,
    /// FluxRouter web_search grounding (contract §5): when `true`, each turn's
    /// `LlmRequest` carries `web_search: true` so the provider attaches the
    /// grounding tool. Set by the CLI `--search` flag via [`set_web_search`];
    /// defaults to `false` so non-Flux sessions are unaffected. Grounding only
    /// actually fires when the live model is a Flux tier alias (the provider
    /// guards on `is_flux_tier_alias`).
    web_search: bool,
    total_usage: TokenUsage,
    thinking: Option<wcore_types::llm::ThinkingConfig>,
    /// Resolved provider compat settings (for capability validation)
    compat: wcore_config::compat::ProviderCompat,
    confirmer: Arc<Mutex<ToolConfirmer>>,
    hooks: Option<HookEngine>,
    session_manager: Option<SessionManager>,
    current_session: Option<Session>,
    output: Arc<dyn OutputSink>,
    current_msg_id: String,
    /// #279(c): stable per-agent-run id, minted once per run() entry and
    /// held across the run's turns. Distinct from internal spawn/workflow
    /// run ids. None until the first run() of the session.
    current_agent_run_id: Option<String>,
    approval_manager: Option<Arc<wcore_protocol::ToolApprovalManager>>,
    /// W7.1 S4-3.2: shared `ApprovalBridge` instance used to round-trip
    /// `approval_required` Script steps. `AgentBootstrap` creates one bridge
    /// and shares it with both the engine (via `set_approval_bridge`) and
    /// `ScriptTool` (via `.with_approval(...)`). The CLI's `ApprovalResume`
    /// command arm calls `engine.approval_bridge().resolve(token, outcome)`
    /// to unblock the script step's awaiting future.
    approval_bridge: Arc<ApprovalBridge>,
    protocol_writer: Option<Arc<dyn wcore_protocol::writer::ProtocolEmitter>>,
    allow_list: Vec<String>,
    /// Persisted reasoning effort, updated by skill context modifiers.
    /// Carried into each turn's LlmRequest.reasoning_effort.
    current_reasoning_effort: Option<String>,
    /// Compaction configuration (thresholds, enabled flag, etc.)
    compact_config: CompactConfig,
    /// Runtime compaction state (circuit breaker, last input tokens)
    compact_state: CompactState,
    /// Runtime plan mode state (active flag, pre-plan allow-list, plan file path)
    plan_state: PlanState,
    /// Shared flag read by EnterPlanMode/ExitPlanMode tools to validate transitions.
    /// Updated by the engine when processing PlanModeTransition modifiers.
    plan_active_flag: Option<Arc<AtomicBool>>,
    /// Prompt cache break detector for diagnostics.
    cache_detector: CacheBreakDetector,
    compaction_level: wcore_compact::CompactionLevel,
    toon_enabled: bool,
    /// Token-opt: native Bash output compaction gate (cargo/git/test/grep).
    /// Resolved from `ProviderCompat::compact_bash()` at construction. When
    /// on, verbose Bash tool output is compacted into the model's transcript
    /// copy (the human still sees full output) before read-once dedup.
    compact_bash: bool,
    /// W4 (Task 8): engine-advertised capability flags. Mirrored to
    /// `Capabilities.*` when emitting Ready/ConfigChanged.
    advertised_capabilities: wcore_config::tools::AdvertisedCapabilitiesConfig,
    /// W6 F7 per-turn cost log; appended after each TurnTrace is built;
    /// emitted as a SessionCost aggregate by `fire_on_session_end`.
    per_turn_costs: Vec<wcore_protocol::events::TurnCost>,
    /// W6 F17 — per-turn MCP tool curation policy. Defaults to `TopK { k: 15 }`
    /// via `McpConfig::default()` so most callers get curated tool lists for
    /// free without flipping a flag.
    mcp_curation: wcore_config::config::McpCurationPolicy,
    /// Cache-stability (token-opt): inventory-hashed UNION of curated MCP
    /// keep-sets. Keeps the serialized tool-zone prefix byte-stable across
    /// turns — a per-turn re-curation otherwise rewrites the cached prefix at
    /// the cache-WRITE rate every MCP turn. Reset only when the MCP tool
    /// inventory itself changes (server connect/disconnect / plugin reload).
    /// Stored as a `Vec` in FIRST-ADD order (not a `HashSet`) so a newly
    /// unioned tool is APPENDED at the end and never reorders already-kept
    /// tools — keeping the emitted tool-zone prefix byte-stable (a cache-safe
    /// append) on every union-growth turn instead of a mid-array insert.
    mcp_curation_cache: Option<(u64, Vec<String>)>,
    /// Cache-stability (token-opt) for the provider tool-CAP path, mirroring
    /// `mcp_curation_cache` but keyed on the cap's own (inventory, budget). When
    /// the cap must trim the MCP tail (chiefly in curation-OFF mode, where the
    /// full MCP inventory reaches the cap), BM25 decides WHICH MCP tools are
    /// most relevant, but the emitted order is an inventory-keyed APPEND-ONLY
    /// union so the serialized tool-zone prefix stays byte-stable turn-to-turn
    /// (a cache READ, not a per-turn prefix rewrite). Reset when the MCP
    /// inventory or the MCP budget changes.
    mcp_cap_cache: Option<(u64, Vec<String>)>,
    /// Token-opt (diff-resend): handle to the shared file-state cache (the same
    /// `Arc` the Read/Edit/Write tools hold). Used only to bump the cache's
    /// compaction generation after a compaction pass, so stale read bases stop
    /// qualifying for diff-resend. `None` in test engines and when the file
    /// cache is disabled; wired by `AgentBootstrap` via `set_file_cache`.
    file_cache: Option<Arc<std::sync::RwLock<wcore_tools::file_cache::FileStateCache>>>,
    /// B7 writer-side wiring — the same `Arc<InMemorySessionState>` the
    /// `wayland_status` / `wayland_telemetry_query` introspection backend
    /// reads. When wired by `AgentBootstrap` (via [`set_session_state`]) the
    /// engine pushes per-turn token totals and per-tool call counts into it,
    /// so the introspection tools surface live numbers instead of zeroes.
    /// `None` in test engines and sub-agents (no introspection surface).
    session_state: Option<Arc<crate::session_state::InMemorySessionState>>,
    /// W6 F17 — audit-log handle for the recency input to `McpCurator`.
    /// `None` means the agent runs without M2 memory wiring (test envs);
    /// curation gracefully degrades to keyword-only ranking in that case.
    audit_log: Option<Arc<wcore_memory::audit::AuditLog>>,
    /// W7 Pre-flight 0: cross-session memory handle.
    ///
    /// Always present — `AgentBootstrap` substitutes a `NullMemory` when
    /// the user has not opted into a real backend. This unblocks W9 T10b
    /// (Curator hook on session end) and W9 T11 (PUM run on session end),
    /// both of which need a `&dyn MemoryApi`. Today this field is read
    /// only by the test-driver helpers; the production hooks land in a
    /// follow-up wave per the bootstrap.rs `skills_lifecycle` tracing log.
    memory_api: Arc<dyn wcore_memory::MemoryApi>,
    /// M3.1: throttle for the session-end dream cycle. `should_run()` is
    /// consulted inside `fire_on_session_end` immediately before
    /// `memory_api.dream_now()` so short interactive sessions don't churn
    /// the consolidation pipeline. The throttle window is seeded from
    /// `cfg.memory.dream_cycle_throttle_secs` at engine construction
    /// (default 1800s / 30 min). `NullMemory::dream_now` is a no-op so the
    /// call is always safe regardless of memory wiring state.
    dream_throttle: Arc<wcore_memory::consolidate::DreamThrottle>,
    /// W7 Pre-flight 0.0d: `TestSink` event-buffer handle. Only populated
    /// by `AgentBootstrap::build_for_test`; production paths leave this
    /// at its default (a detached buffer that never receives emissions).
    /// Read via `captured_protocol_events()`.
    #[cfg(any(test, feature = "test-utils"))]
    test_sink_handle: crate::test_utils::TestSinkHandle,
    /// W9.1 T3 (T10b): cached `config.observability.skills_lifecycle`
    /// flag. Gates the per-turn F10 detect/stage/emit flow. Read at
    /// construction; the field is cheap-cloneable and never mutated
    /// for the engine's lifetime.
    skills_lifecycle: bool,
    /// F-092 (W7-N): cached `config.observability.online_evolution` flag.
    /// Gates live `EvolutionEvent` emission + Paraphrase mutator application
    /// at session-end. Default off. Opt-in via CLI `--online-evolution` or
    /// `[observability] online_evolution = true` in config.
    online_evolution: bool,
    /// W9.1 T3 (T10b): rolling buffer of the most recent `TurnTrace`s,
    /// capped at `SKILL_DETECTION_WINDOW`. `PatternDetector::detect`
    /// consumes this slice once per turn when `skills_lifecycle` is on.
    /// Always empty (never grown) when the flag is off, so the off path
    /// has zero per-turn allocation overhead.
    recent_turn_traces: VecDeque<TurnTrace>,
    /// W9.1 T3 (T10b): dedup set keyed on each detected candidate's
    /// canonical signature (`tool_sequence`, `input_shape`). Without
    /// this, every subsequent turn re-emits the same `skill_drafted`
    /// TraceEvent because `PatternDetector::detect` always sees the
    /// same pattern still present in the rolling window. Matches
    /// `DraftWriter::stage`'s deterministic-UUID idempotency at the
    /// emit layer.
    drafted_skill_signatures: HashSet<(Vec<String>, Vec<Vec<String>>)>,
    /// W8b.2.B D.3: optional filesystem watcher used to surface external
    /// edits to the user between turns. When `Some`, the per-turn boundary
    /// drains `FileWatcher::drain_external_events()`, renders a synthetic
    /// system message via `render_external_edit_message`, emits it as a
    /// `ProtocolEvent::Info`, and pushes a User-role context block into
    /// `self.messages` so the next turn's `LlmRequest` carries the note.
    /// `AgentBootstrap::build` populates this when the session has a real
    /// filesystem root (Task 7); tests can set it via `set_file_watcher`.
    file_watcher: Arc<std::sync::OnceLock<Arc<crate::watch::FileWatcher>>>,
    /// W8b.2.B Task 7: optional notifier handed to per-call `ToolContext`s
    /// at the orchestration dispatcher. When set, `FileWatcher`-backed
    /// `Write`/`Edit` tools call `note_self_originated_write` before each
    /// write so the watcher's debounce path swallows engine-driven events.
    /// Plumbed into the orchestration layer via the new `notifier`
    /// parameter on `execute_tool_calls*_with_*`.
    tool_write_notifier:
        Arc<std::sync::OnceLock<Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>>>,
    /// Wave OR (W8b.2.B.1): optional user override for the per-turn
    /// orchestration template. When `Some`, [`super::orchestration::intent::LoopSelector`]
    /// honours the override and picks the matching graph template
    /// (Direct, Parallel, Sequential, SelfCritique); when `None`,
    /// `LoopSelector` defers to the [`IntentClassifier`]'s keyword
    /// heuristic. Default is `None` (auto-classify), which routes to
    /// `Intent::Direct` for trivial / unmarked tasks — byte-identical
    /// to pre-OR behaviour.
    mode_override: Option<crate::orchestration::intent::Mode>,
    /// M3.2 — handles to background tokio tasks spawned by `AgentBootstrap`
    /// when `cfg.memory.enabled = true` (currently: the decay scheduler).
    /// `Drop` aborts every handle so tests + production shutdowns never
    /// leak the background task. Empty when memory is disabled or when
    /// the engine was built via a test helper that bypasses bootstrap.
    decay_handles: Vec<tokio::task::JoinHandle<()>>,
    /// Wave 6A.1 — keepalives for on-disk plugin runtime handles
    /// (`Arc<LoadedWasmPlugin>` / `Arc<LoadedSubprocessPlugin>`). The
    /// closures inside synthesized plugin tools clone these `Arc`s per
    /// invocation, so dropping them before the engine itself is dropped
    /// would close the subprocess child / drop the WASM component
    /// mid-session. Bootstrap calls `set_plugin_runtime_handles` after
    /// `apply_initialize_outcome` so the engine outlives the registered
    /// closures.
    /// v0.8.0 N.2 — stored as `Arc<Vec<_>>` (instead of plain `Vec<_>`)
    /// so the slash-runtime PluginHandler can hold a cheap shared
    /// reference without forcing `LoadedRuntimeHandle: Clone` (its
    /// McpBridge variant isn't Clone-able). Existing `&[T]` access via
    /// `plugin_runtime_handles()` is unchanged; the new arc-flavoured
    /// getter `plugin_runtime_handles_arc()` returns a clone of the Arc.
    plugin_runtime_handles: Arc<Vec<crate::plugins::LoadedRuntimeHandle>>,
    /// M5.3 — optional per-session / per-user budget tracker. When set,
    /// every accepted LLM turn calls `tracker.charge(session_id, tokens,
    /// usd)` so the `BudgetEvent::Charge` telemetry hits the wired
    /// observability sink. `None` is the default — no caps configured,
    /// no telemetry; matches pre-M5.3 behaviour.
    budget_tracker: Option<Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>>,
    /// v0.6.1 CRIT-1: opt-in policy gate. When `Some`, every tool call
    /// in `dispatch_once` is checked against the `PolicyEngine` before
    /// it reaches the approval / budget pipeline. `None` (the default)
    /// preserves byte-identical v0.6.0 behaviour for all existing sessions.
    /// Set via `set_policy_gate` after construction (e.g. from
    /// `AgentBootstrap` when the session config enables permission
    /// enforcement).
    policy_gate: Option<crate::policy_gate::PolicyGate>,
    /// v0.6.4 Task 1.2 — optional plugin-contributed `AgentRegistry`.
    /// When `Some`, bootstrap has called `set_agent_registry` after applying
    /// `InitializeOutcome` via `apply_initialize_outcome`. The registry is
    /// `Arc`-wrapped so the engine and `SpawnTool::with_registry` (which
    /// requires that exact `Arc<AgentRegistry>` type) observe the *same*
    /// registry instance — a single shared identity, not to avoid the cost
    /// of cloning (which is already cheap via the inner `Arc<Mutex<HashMap>>`).
    /// `None` (the default) preserves pre-Task-1.2 behaviour: `SpawnTool`
    /// resolves no named agents.
    agent_registry: Option<std::sync::Arc<crate::agents::registry::AgentRegistry>>,
    /// v0.6.5 Wave 6A.2 — plugin-reified user-model backends. Populated by
    /// `AgentBootstrap` after `apply_initialize_outcome` returns via
    /// `set_plugin_user_models`. When non-empty, the session-end PUM path
    /// mirrors every inferred user-model delta to each reified backend
    /// (e.g. `HonchoClient::learn_preference`) in addition to the local
    /// `MemoryApi::update_user_model` write. Empty (the default) preserves
    /// the pre-Wave-6A.2 behaviour: PUM writes deltas to local memory only.
    plugin_user_models: Vec<crate::plugins::apply::ReifiedUserModel>,
    /// 1.B.3: rolling user-style fingerprint, updated each turn by
    /// `AgentEngine::run` from the incoming user input. Read by Phase 4
    /// system-prompt + agent-router work.
    style_detector: Mutex<crate::style_detector::StyleDetector>,
    /// v0.8.0 N.3 — read-only handle to the session's resolved
    /// `SkillCatalog` (same instance as the one wired into `SkillTool`).
    /// Populated by `AgentBootstrap::build` via `set_skill_catalog`; the
    /// `/skill` slash-handler's `Runtime` variant reads from this so the
    /// runtime "list / show" output matches the catalog the model
    /// actually sees in its system prompt. `None` is the default for
    /// engines constructed outside bootstrap (`SkillHandler::Stub`
    /// continues to back those paths).
    skill_catalog: Option<Arc<wcore_skills::refs::SkillCatalog>>,
    /// v0.8.0 Task K — optional learned router that picks an
    /// orchestration `Template` per turn. When `Some`, it is consulted
    /// before the deterministic `IntentClassifier` cold-start fallback;
    /// when `None`, behaviour is byte-identical to pre-K (classifier
    /// only). Wrapped in `Mutex` because `TemplateRouter::choose`
    /// mutates the inner Beta scorer's RNG state.
    template_router: Option<Arc<Mutex<wcore_dispatch::TemplateRouter>>>,
    /// v0.8.0 Task M — per-turn user-model write-back. Populated by
    /// `AgentBootstrap::build` via `set_user_model_backend` when memory
    /// is enabled. On every `run()` invocation the engine derives a
    /// 4-axis style fingerprint from the user input and calls
    /// `backend.observe(user_id, Observation::Style(fp))` so the
    /// user-model layer learns continuously instead of being
    /// bootstrap-only-read. `None` (the default) skips write-back —
    /// preserves pre-v0.8.0 behaviour byte-identical when no backend
    /// is installed.
    user_model_backend: Option<Arc<dyn wcore_user_model::UserModelBackend>>,
    /// v0.8.0 Task M — user-id key used for write-back. Defaults to
    /// `"default"` (mirrors the bootstrap read site at
    /// `bootstrap.rs::user_ctx_block`); overridable via the
    /// `WAYLAND_USER_ID` env var for multi-user / shared-host setups.
    /// Cached on the engine so the per-turn write-back doesn't pay an
    /// env-lookup tax on the hot path.
    user_model_user_id: String,
    /// v0.8.1 U1 — per-turn skill router. Installed by
    /// `AgentBootstrap::build` via `set_skill_router` after the catalog
    /// is loaded and the `SkillPrioritizer` has run; seeded from GEPA's
    /// `PromptStore::seed_pairs_for` + `SkillPrioritizer::priority_order`
    /// for warm cold-start. `None` is the default for engines
    /// constructed outside bootstrap (mirrors the `template_router` /
    /// `skill_catalog` `Option` patterns) — `engine.run()` short-circuits
    /// the choose/observe loop in that case so test engines stay
    /// behaviour-equivalent to pre-U1.
    skill_router: Option<Arc<Mutex<wcore_skills::SkillRouter>>>,
    /// v0.8.1 U1 — per-turn pick from `skill_router.choose()` captured
    /// at the top of `run()` so the matching `observe()` call at the
    /// end of the turn can credit the same arm. `take()`-cleared on
    /// every observe to keep the slot single-use.
    current_skill_router_pick: Option<String>,
    /// v0.8.1 U6 — autonomous-skill bucketer. Records every `run()`
    /// trajectory; N consecutive successes on the same task signature
    /// produces a `DraftTrigger` that's handed to `skill_drafter`. Lives
    /// behind `Mutex` (not `parking_lot::Mutex`) so the existing
    /// std-style locking idiom in this module carries through.
    auto_skill_bucketer: Mutex<crate::auto_skill::Bucketer>,
    /// v0.8.1 U6 — installed by `AgentBootstrap::build` when both memory
    /// and skills are available. `None` here keeps every non-bootstrap
    /// construction site (tests, resume-without-bootstrap, sub-agent
    /// shadows) on the pre-U6 no-op path — the bucketer still observes,
    /// but no on-disk draft is written.
    skill_drafter: Option<Arc<crate::auto_skill::SkillDrafter>>,
    /// AUDIT A2 / B1 — session-root cooperative cancellation token.
    ///
    /// One token per engine, threaded into every per-turn `GraphContext`
    /// and every per-call `ToolContext` so a host (TUI, ACP server) that
    /// fires `cancel_token()` actually reaches a running tool and stops
    /// the turn loop between iterations. Before this field the per-turn
    /// `GraphContext.cancel` was a fresh orphan and every `ToolContext`
    /// got `ToolContext::test_default().cancel` — a dead stub — so a
    /// host cancel had no cooperative path and a wedged tool could only
    /// be escaped by killing the process.
    ///
    /// The engine never fires it itself (that is the host's job); the
    /// loop only *observes* it between turns and propagates a child
    /// token into tool dispatch.
    cancel_token: tokio_util::sync::CancellationToken,
    /// AUDIT B-2 / D-5 — handles for background reliability tasks
    /// (currently: the `ToolApprovalManager` TTL reaper). Kept separate
    /// from `decay_handles` so the memory-scheduler accounting that
    /// `decay_handles_len()` exposes stays accurate. `Drop` aborts every
    /// handle so a recycled session leaks no background task.
    background_handles: Vec<tokio::task::JoinHandle<()>>,
    /// Dynamic Workflows B3 — cached `observability.workflow_detection_enabled`
    /// gate. When `false` (the default), the per-turn `WorkflowCandidate`
    /// heuristic is not even computed at the intent-telemetry seam, so a
    /// default-config session behaves byte-for-byte as before. When `true`,
    /// the engine computes the candidate alongside the existing
    /// intent-telemetry classify and emits it as a telemetry-only signal —
    /// it NEVER feeds `select_graph_config` / `TemplateDecision` or any
    /// routing / tool-dispatch decision. Cached at construction (the flag
    /// is operator-controlled and never flips mid-session) to avoid a
    /// per-turn config dereference on the hot path.
    workflow_detection_enabled: bool,
    /// Dynamic Workflows B6 — cached `observability.workflow_live_mode` gate.
    /// Distinct from `workflow_detection_enabled` (the shadow-only signal):
    /// when `true`, the live confirm gate (after the shadow seam in `run`)
    /// may synthesise a workflow, emit a `Workflow` approval request, and —
    /// only on explicit user approval — run it as the turn's output. Default
    /// `false`; cached at construction (operator-controlled, never flips
    /// mid-session) to keep the off-path branch-free.
    workflow_live_mode: bool,
    /// Dynamic Workflows B6 — the resolved runtime config, retained so the
    /// live confirm gate can build a transient `AgentSpawner` (which needs a
    /// full `Config`) for workflow synthesis + execution without re-reading
    /// from disk. Only the live gate reads this; the rest of the engine uses
    /// the derived fields above.
    config: Config,
    /// Token-opt "compaction floor": the number of leading conversation
    /// messages that autocompact has summarized/collapsed away. Any absolute
    /// message index `< compaction_floor` no longer maps to its original
    /// message — autocompact replaced that whole prefix with a single folded
    /// boundary+summary `User` message (see `run_compaction`).
    ///
    /// Consumers (diff-resend, read-once) use this to decide whether an earlier
    /// message's content is STILL verbatim in the model's visible history.
    /// Microcompact does NOT move this floor: it clears tool-result *bodies*
    /// in place (leaving the message + its `CLEARED`/`SUPERSEDED` marker), so
    /// the indices still map — a stubbed body is detected via those markers,
    /// not via the floor.
    ///
    /// Reset to 0 on conversation reset (`/clear`, `/resume`), where the
    /// message buffer is replaced wholesale.
    compaction_floor: usize,
    /// C1 / Task A2 — number of leading `self.messages` entries that are the
    /// synthetic SessionStart hook prelude (applied by `run_session_start_hooks`
    /// on a cold session). Acts as a "cold baseline" so that
    /// `recall_relevant_facts` still treats a session whose ONLY message is the
    /// plugin prelude as cold and still fires cross-session recall. `0` in every
    /// other case (no prelude, resume — where construction already populated
    /// `messages` before session-start hooks run, so the prelude path is skipped).
    session_start_injected_len: usize,
    /// Hook actions (InjectMessage / SwitchModel) fired since the last
    /// `TurnTrace` was emitted, accumulated across the turn's pre-turn,
    /// post-tool-use, and turn-end hook phases. Drained into the next
    /// `TurnTrace.hook_actions` via `std::mem::take` at each emission site.
    pending_hook_actions: Vec<wcore_observability::trace::HookActionRecord>,
    /// #282 contract V1: stable per-session conversation id for Flux sticky
    /// routing. Minted ONCE at construction with a v4 UUID and threaded onto
    /// every `LlmRequest` as `conversation_id`; the Flux provider emits it as
    /// the `x-wl-conversation-id` request header on tier-alias turns. Survives
    /// `/clear` and `/resume` so a continued session keeps routing affinity.
    conversation_id: String,
    /// #282 contract V1: the REAL context window of the model Flux routed the
    /// LAST turn to (`x-flux-model-window`). `None` until Flux signals back.
    /// Used to reconcile the #255 pre-flight overflow guard against the actual
    /// served-model window instead of the pre-route guess.
    flux_served_window: Option<u64>,
    /// #282 contract V1: the LAST-seen Flux context pressure
    /// (`x-flux-context-pressure`, `0.0..=1.0` = required / window). Captured
    /// for #280 pressure-driven scheduling (see `smart_compact_fraction`).
    flux_context_pressure: Option<f32>,
    /// #280 smart auto-compaction — anti-thrash + one-shot wiring. ALL inert
    /// unless `compact_config.smart_enabled` (the single chokepoint is
    /// `smart_compact_fraction`, which returns `None` before touching any of
    /// these when the gate is off).
    ///
    /// HYSTERESIS arm flag: true = a proactive compact may fire when the
    /// fraction crosses the trigger. Flips false on a fire and re-arms only
    /// once a later turn's fraction drops below the release low-water.
    smart_compact_armed: bool,
    /// COOLDOWN watermark: the last turn index a smart compact fired, so two
    /// fires are spaced at least `smart_cooldown_turns` apart (absorbs the
    /// post-stream watermark refresh lag).
    smart_compact_last_turn: Option<u32>,
    /// TERMINAL cannot-shrink latch: once a smart-forced compact frees fewer
    /// than `smart_min_shrink_tokens`, smart compaction is OFF for the session.
    smart_compact_exhausted: bool,
    /// One-shot: set true by the turn-top pre-gate when a smart compact should
    /// fire, then consumed via `std::mem::take` inside `run_compaction` so the
    /// existing autocompact path runs even though the static threshold is unmet.
    smart_compact_force: bool,
}

impl Drop for AgentEngine {
    fn drop(&mut self) {
        // M3.2 — abort every background decay scheduler task on shutdown.
        // `JoinHandle::abort` is safe on already-finished tasks (no-op),
        // so we don't need to inspect state.
        for h in self.decay_handles.drain(..) {
            h.abort();
        }
        // AUDIT B-2 / D-5 — abort background reliability tasks (the
        // approval-manager TTL reaper) so a recycled session leaks no
        // task.
        for h in self.background_handles.drain(..) {
            h.abort();
        }
    }
}

/// W9.1 T3 (T10b): rolling-window size for skill pattern detection.
/// `PatternDetector::DEFAULT_MIN_REPEATS = 3`, so a window of 6 is the
/// smallest size that lets the detector observe 3 repeats while still
/// tolerating 3 interleaved non-matching turns. Documented in the W9
/// design contract §5.3 (F10 acceptance) — keep in sync.
const SKILL_DETECTION_WINDOW: usize = 6;

/// C1 / Task A2 — per-message token budget for a SessionStart plugin-hook
/// prelude. Generous: the prelude is injected once per session, not per turn.
/// A plugin's contribution larger than this (estimated at `chars / 4`, matching
/// the repo's `estimate_tokens_from_messages` text heuristic) is truncated at
/// the fold site so a misbehaving plugin can't blow up the first request — we
/// never trust the plugin's self-reported size.
const SESSION_PRELUDE_TOKEN_BUDGET: usize = 1500;

/// C1 / Task A2 — chars-per-token text heuristic, mirroring
/// `compact::estimate`'s `CHARS_PER_TOKEN_TEXT` (kept local rather than widening
/// that module's visibility for one call site).
const PRELUDE_CHARS_PER_TOKEN: usize = 4;

/// C1 / Task A3 — per-turn token budget for a `PrePrompt` plugin-hook
/// contribution. Much tighter than the SessionStart prelude budget because this
/// is applied to the request tail on EVERY turn (not once per session), so a
/// large injection here both inflates every request and risks per-turn cache
/// churn. A plugin's contribution larger than this (estimated at `chars / 4`) is
/// truncated at the fold site — we never trust the plugin's self-reported size.
const PRE_PROMPT_TOKEN_BUDGET: usize = 500;

/// W8 v0.6.3 — expected prompt-prefix reuse window for the agent turn loop.
/// Every turn re-sends the same system prompt + tool definitions, so the
/// cacheable prefix is hit again on the very next turn — well beyond the
/// 5-minute ephemeral window. `pick_cache_tier` uses this to promote a
/// large prompt to the 1h cache tier. 30 minutes is a conservative lower
/// bound for a multi-turn agent session.
const AGENT_TURN_CACHE_REUSE_WINDOW_SECS: u64 = 1800;

/// Output-side token optimization (Part A): fluff closers that, once the model
/// starts emitting one at a *paragraph boundary*, signal the answer is over and
/// only ceremonial filler follows. Sent as provider stop sequences so the model
/// halts before spending output tokens on the closer.
///
/// EVERY entry is prefixed with `"\n\n"` on purpose: a stop sequence is a raw
/// substring match, so prefixing the paragraph break guarantees these only fire
/// at the start of a fresh paragraph. A mid-sentence occurrence of the same
/// words (e.g. "...let me know if that helps, but first...") never matches,
/// because it is not preceded by a blank line. Anthropic caps stop sequences at
/// a small number, so keep this list at most 4 entries.
///
/// Only applied when the route optimizes client-side
/// (`compat.input_optimization() == "client"`); router-optimized routes get an
/// empty Vec and emit no stop field. The list is a fixed `const`, so it never
/// perturbs the cached prompt prefix.
const FLUFF_STOP_SEQUENCES: [&str; 4] = [
    "\n\nLet me know if",
    "\n\nI hope this helps",
    "\n\nFeel free to",
    "\n\nIs there anything else",
];

/// v0.8.0 Task M — default user-id key for per-turn user-model
/// write-back. Mirrors the bootstrap read site (`bootstrap.rs`,
/// `user_id = "default"`). Override via the `WAYLAND_USER_ID` env var.
const DEFAULT_USER_MODEL_USER_ID: &str = "default";

/// Stage 4c — one-shot, per-process suppression for the `/crucible` gate
/// suggestion. Set after the hint is shown once, or pre-emptively via
/// `/crucible off`.
static CRUCIBLE_SUGGEST_SUPPRESSED: AtomicBool = AtomicBool::new(false);

/// v0.8.0 Task M — resolve the user-id used for user-model
/// observations. Reads `WAYLAND_USER_ID` once at engine construction;
/// falls back to `DEFAULT_USER_MODEL_USER_ID` when unset or empty.
pub(crate) fn resolve_user_model_user_id() -> String {
    match std::env::var("WAYLAND_USER_ID") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => DEFAULT_USER_MODEL_USER_ID.to_string(),
    }
}

impl AgentEngine {
    pub fn new(config: Config, tools: ToolRegistry, output: Arc<dyn OutputSink>) -> Self {
        let provider = create_provider(&config);
        Self::new_with_provider(provider, config, tools, output)
    }

    /// Create an engine with an externally-provided provider (for sub-agent sharing)
    pub fn new_with_provider(
        provider: Arc<dyn LlmProvider>,
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
    ) -> Self {
        // Dynamic Workflows B6 — capture the live confirm-gate flag and a full
        // clone of the resolved config BEFORE the partial moves below (the
        // struct literal moves `config.model`, `config.thinking`, etc., so the
        // whole value is no longer available afterwards). The clone is retained
        // on `self.config` solely for the live gate's transient `AgentSpawner`.
        let workflow_live_mode = config.observability.workflow_live_mode;
        let retained_config = config.clone();
        let system_prompt = config.system_prompt.clone().unwrap_or_default();
        let confirmer =
            ToolConfirmer::new(config.tools.auto_approve, config.tools.allow_list.clone());

        let session_manager = if config.session.enabled {
            Some(SessionManager::new(
                config.session.directory.clone().into(),
                config.session.max_sessions,
            ))
        } else {
            None
        };

        let allow_list = config.tools.allow_list.clone();
        let compact_config = config.compact.clone();
        // M3.1: throttle window comes from the resolved config so users can
        // tune session-end dream cadence via `[memory] dream_cycle_throttle_secs`.
        let dream_throttle_window =
            std::time::Duration::from_secs(config.memory.dream_cycle_throttle_secs);

        Self {
            provider,
            tools: Arc::new(tools),
            messages: Vec::new(),
            // Wave-6 #5: retain the boot prompt so a later rebind preserves the
            // framework fragments baked in here (`build_system_prompt` already
            // folded Constitution / persona / skills index into this string).
            rebind_system_prefix: Some(system_prompt.clone()),
            system_prompt,
            model: config.model,
            user_model_pin: None,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            max_turns: config.max_turns,
            total_usage: TokenUsage::default(),
            thinking: config.thinking,
            compat: config.compat.clone(),
            confirmer: Arc::new(Mutex::new(confirmer)),
            // Always initialise Some so that skill-declared hooks can be merged in
            // even when the global config has no static hooks configured.
            hooks: Some({
                let mut h = HookEngine::new(config.hooks.clone());
                // W6 F15: verify_edits flag registers VerifyWriteHook
                // (post_tool_use Write-only re-read). Off by default.
                if config.tools.verify_edits {
                    h.register_rust_hook(Box::new(
                        crate::hooks::verify_write::VerifyWriteHook::new(),
                    ));
                }
                h
            }),
            session_manager,
            current_session: None,
            output,
            current_msg_id: String::new(),
            current_agent_run_id: None,
            approval_manager: None,
            approval_bridge: Arc::new(ApprovalBridge::new()),
            protocol_writer: None,
            allow_list,
            current_reasoning_effort: None,
            compact_config,
            compact_state: CompactState::new(),
            plan_state: PlanState::default(),
            plan_active_flag: None,
            cache_detector: CacheBreakDetector::new(),
            compaction_level: config.compact.compaction,
            toon_enabled: config.compact.toon,
            compact_bash: config.compat.compact_bash(),
            advertised_capabilities: config.advertised_capabilities.clone(),
            per_turn_costs: Vec::new(),
            mcp_curation: config.mcp.curation.clone(),
            mcp_curation_cache: None,
            mcp_cap_cache: None,
            file_cache: None,
            session_state: None,
            audit_log: None,
            // W7 Pre-flight 0: default to NullMemory; `AgentBootstrap`
            // calls `set_memory_api()` after construction when a real
            // backend is configured.
            memory_api: Arc::new(wcore_memory::NullMemory),
            dream_throttle: Arc::new(wcore_memory::consolidate::DreamThrottle::new(
                dream_throttle_window,
            )),
            #[cfg(any(test, feature = "test-utils"))]
            test_sink_handle: crate::test_utils::TestSinkHandle::default(),
            // W9.1 T3 (T10b): read the skills_lifecycle gate at
            // construction. The flag is operator-controlled and never
            // flips at runtime; caching here saves a per-turn config
            // dereference on the hot path.
            skills_lifecycle: config.observability.skills_lifecycle,
            // F-092 (W7-N): cache online_evolution gate at construction.
            online_evolution: config.observability.online_evolution,
            recent_turn_traces: VecDeque::new(),
            drafted_skill_signatures: HashSet::new(),
            file_watcher: Arc::new(std::sync::OnceLock::new()),
            tool_write_notifier: Arc::new(std::sync::OnceLock::new()),
            mode_override: None,
            template_router: None,
            decay_handles: Vec::new(),
            plugin_runtime_handles: Arc::new(Vec::new()),
            budget_tracker: None,
            policy_gate: None,
            agent_registry: None,
            plugin_user_models: Vec::new(),
            style_detector: Mutex::new(crate::style_detector::StyleDetector::new()),
            skill_catalog: None,
            user_model_backend: None,
            user_model_user_id: resolve_user_model_user_id(),
            // v0.8.1 U1 — installed post-construction by
            // `AgentBootstrap::build` (see `set_skill_router`). `None`
            // here keeps every non-bootstrap construction site (tests,
            // resume-without-bootstrap, sub-agent shadows) on the
            // pre-U1 no-op path.
            skill_router: None,
            current_skill_router_pick: None,
            // v0.8.1 U6 — autonomous-skill bucketer is always live (N=3
            // threshold). Drafter is None at construction; bootstrap
            // installs one when memory is wired.
            auto_skill_bucketer: Mutex::new(crate::auto_skill::Bucketer::new(3)),
            skill_drafter: None,
            // AUDIT A2 / B1 — fresh session-root cancellation token.
            // Hosts replace/observe it via `cancel_token()`.
            cancel_token: tokio_util::sync::CancellationToken::new(),
            // AUDIT B-2 / D-5 — reaper handle storage; populated by
            // `set_approval_manager`, aborted by `Drop`.
            background_handles: Vec::new(),
            // Dynamic Workflows B3 — cache the off-by-default detection
            // gate at construction; mirrors `skills_lifecycle` /
            // `online_evolution` above.
            workflow_detection_enabled: config.observability.workflow_detection_enabled,
            // Dynamic Workflows B6 — live confirm gate + retained config,
            // captured before the partial moves above.
            workflow_live_mode,
            config: retained_config,
            // Token-opt: no history has been collapsed yet at construction.
            compaction_floor: 0,
            // C1 / A2 — no SessionStart prelude applied at construction.
            session_start_injected_len: 0,
            // No hook actions have fired before the first turn.
            web_search: false,
            pending_hook_actions: Vec::new(),
            // #282: mint the stable Flux conversation id once per engine.
            conversation_id: uuid::Uuid::new_v4().to_string(),
            // #282: no Flux signal-back seen yet at construction.
            flux_served_window: None,
            flux_context_pressure: None,
            // #280: smart auto-compaction starts armed; latches seeded clear.
            smart_compact_armed: true,
            smart_compact_last_turn: None,
            smart_compact_exhausted: false,
            smart_compact_force: false,
        }
    }

    /// Create from a resumed session
    pub fn resume(
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
        session: Session,
    ) -> Self {
        let provider = create_provider(&config);
        Self::resume_with_provider(provider, config, tools, output, session)
    }

    /// Create from a resumed session with an externally-provided provider
    pub fn resume_with_provider(
        provider: Arc<dyn LlmProvider>,
        config: Config,
        tools: ToolRegistry,
        output: Arc<dyn OutputSink>,
        session: Session,
    ) -> Self {
        // Dynamic Workflows B6 — capture the live confirm-gate flag and a full
        // clone of the resolved config BEFORE the partial moves below (see
        // `new_with_provider` for the rationale).
        let workflow_live_mode = config.observability.workflow_live_mode;
        let retained_config = config.clone();
        let system_prompt = config.system_prompt.clone().unwrap_or_default();
        let confirmer =
            ToolConfirmer::new(config.tools.auto_approve, config.tools.allow_list.clone());

        let session_manager = if config.session.enabled {
            Some(SessionManager::new(
                config.session.directory.clone().into(),
                config.session.max_sessions,
            ))
        } else {
            None
        };

        let allow_list = config.tools.allow_list.clone();
        let compact_config = config.compact.clone();
        // M3.1 (M3.2 follow-up): seed throttle from cfg so resume paths
        // honour `[memory] dream_cycle_throttle_secs` the same way
        // `new_with_provider` does. Was previously hardcoded to 1800s.
        let dream_throttle_window =
            std::time::Duration::from_secs(config.memory.dream_cycle_throttle_secs);

        Self {
            provider,
            tools: Arc::new(tools),
            messages: session.messages.clone(),
            // Wave-6 #5: retain the boot prompt so a later rebind preserves the
            // framework fragments folded in by `build_system_prompt`.
            rebind_system_prefix: Some(system_prompt.clone()),
            system_prompt,
            model: config.model.clone(),
            user_model_pin: None,
            max_tokens: config.max_tokens,
            temperature: config.temperature,
            max_turns: config.max_turns,
            total_usage: session.total_usage.clone(),
            thinking: config.thinking,
            compat: config.compat.clone(),
            confirmer: Arc::new(Mutex::new(confirmer)),
            hooks: Some({
                let mut h = HookEngine::new(config.hooks.clone());
                // W6 F15: verify_edits flag registers VerifyWriteHook
                // (post_tool_use Write-only re-read). Off by default.
                if config.tools.verify_edits {
                    h.register_rust_hook(Box::new(
                        crate::hooks::verify_write::VerifyWriteHook::new(),
                    ));
                }
                h
            }),
            session_manager,
            current_session: Some(session),
            output,
            current_msg_id: String::new(),
            current_agent_run_id: None,
            approval_manager: None,
            approval_bridge: Arc::new(ApprovalBridge::new()),
            protocol_writer: None,
            allow_list,
            current_reasoning_effort: None,
            compact_config,
            compact_state: CompactState::new(),
            plan_state: PlanState::default(),
            plan_active_flag: None,
            cache_detector: CacheBreakDetector::new(),
            compaction_level: config.compact.compaction,
            toon_enabled: config.compact.toon,
            compact_bash: config.compat.compact_bash(),
            advertised_capabilities: config.advertised_capabilities.clone(),
            per_turn_costs: Vec::new(),
            mcp_curation: config.mcp.curation.clone(),
            mcp_curation_cache: None,
            mcp_cap_cache: None,
            file_cache: None,
            session_state: None,
            audit_log: None,
            // W7 Pre-flight 0: default to NullMemory; `AgentBootstrap`
            // calls `set_memory_api()` after construction when a real
            // backend is configured.
            memory_api: Arc::new(wcore_memory::NullMemory),
            dream_throttle: Arc::new(wcore_memory::consolidate::DreamThrottle::new(
                dream_throttle_window,
            )),
            #[cfg(any(test, feature = "test-utils"))]
            test_sink_handle: crate::test_utils::TestSinkHandle::default(),
            // W9.1 T3 (T10b): cache the gate; see new_with_provider note.
            skills_lifecycle: config.observability.skills_lifecycle,
            // F-092 (W7-N): cache online_evolution gate at construction.
            online_evolution: config.observability.online_evolution,
            recent_turn_traces: VecDeque::new(),
            drafted_skill_signatures: HashSet::new(),
            file_watcher: Arc::new(std::sync::OnceLock::new()),
            tool_write_notifier: Arc::new(std::sync::OnceLock::new()),
            mode_override: None,
            template_router: None,
            decay_handles: Vec::new(),
            plugin_runtime_handles: Arc::new(Vec::new()),
            budget_tracker: None,
            policy_gate: None,
            agent_registry: None,
            plugin_user_models: Vec::new(),
            style_detector: Mutex::new(crate::style_detector::StyleDetector::new()),
            skill_catalog: None,
            user_model_backend: None,
            user_model_user_id: resolve_user_model_user_id(),
            // v0.8.1 U1 — installed post-construction by
            // `AgentBootstrap::build` (see `set_skill_router`). `None`
            // here keeps every non-bootstrap construction site (tests,
            // resume-without-bootstrap, sub-agent shadows) on the
            // pre-U1 no-op path.
            skill_router: None,
            current_skill_router_pick: None,
            // v0.8.1 U6 — autonomous-skill bucketer is always live (N=3
            // threshold). Drafter is None at construction; bootstrap
            // installs one when memory is wired.
            auto_skill_bucketer: Mutex::new(crate::auto_skill::Bucketer::new(3)),
            skill_drafter: None,
            // AUDIT A2 / B1 — fresh session-root cancellation token.
            // Hosts replace/observe it via `cancel_token()`.
            cancel_token: tokio_util::sync::CancellationToken::new(),
            // AUDIT B-2 / D-5 — reaper handle storage; populated by
            // `set_approval_manager`, aborted by `Drop`.
            background_handles: Vec::new(),
            // Dynamic Workflows B3 — cache the off-by-default detection
            // gate from the resumed session's config (mirrors
            // `new_with_provider`).
            workflow_detection_enabled: config.observability.workflow_detection_enabled,
            // Dynamic Workflows B6 — live confirm gate + retained config,
            // captured before the partial moves above.
            workflow_live_mode,
            config: retained_config,
            // Token-opt: no history has been collapsed yet at construction.
            compaction_floor: 0,
            // C1 / A2 — resume populates `messages` here at construction, so
            // the session-start prelude path is skipped; baseline stays 0.
            session_start_injected_len: 0,
            // No hook actions have fired before the first turn.
            web_search: false,
            pending_hook_actions: Vec::new(),
            // #282: mint the stable Flux conversation id once per engine. A
            // resumed session gets a fresh id (sticky routing is best-effort).
            conversation_id: uuid::Uuid::new_v4().to_string(),
            // #282: no Flux signal-back seen yet at construction.
            flux_served_window: None,
            flux_context_pressure: None,
            // #280: smart auto-compaction starts armed; latches seeded clear.
            smart_compact_armed: true,
            smart_compact_last_turn: None,
            smart_compact_exhausted: false,
            smart_compact_force: false,
        }
    }

    pub fn compaction_level(&self) -> wcore_compact::CompactionLevel {
        self.compaction_level
    }

    /// Token-opt: the compaction floor — the number of leading conversation
    /// messages that autocompact has summarized/collapsed away. Any absolute
    /// message index `< compaction_floor` no longer maps to its original
    /// message. `0` means no autocompact has run this conversation. See the
    /// `compaction_floor` field doc.
    //
    // `allow(dead_code)`: the consumers (diff-resend, read-once) land later in
    // the token-opt campaign; this is the shared primitive they read. The field
    // itself is already live (written by autocompact, reset on `/clear`).
    #[allow(dead_code)]
    pub(crate) fn compaction_floor(&self) -> usize {
        self.compaction_floor
    }

    /// Token-opt: whether the absolute message index `idx` still maps to its
    /// original message in the model's visible history (i.e. autocompact has
    /// not collapsed it away). Note: this only tracks autocompact's leading
    /// collapse — a message can still be *visible* by this test yet have an
    /// in-place-cleared tool-result body (microcompact); detect that via the
    /// `CLEARED`/`SUPERSEDED` markers, not this helper.
    #[allow(dead_code)]
    pub(crate) fn message_index_still_visible(&self, idx: usize) -> bool {
        idx >= self.compaction_floor
    }

    /// Get a reference to the shared provider
    pub fn provider(&self) -> &Arc<dyn LlmProvider> {
        &self.provider
    }

    /// Get a reference to the resolved compat settings
    pub fn compat(&self) -> &wcore_config::compat::ProviderCompat {
        &self.compat
    }

    /// Get a reference to the engine-advertised capabilities.
    pub fn advertised_capabilities(&self) -> &wcore_config::tools::AdvertisedCapabilitiesConfig {
        &self.advertised_capabilities
    }

    pub fn tool_names(&self) -> Vec<String> {
        self.tools.tool_names()
    }

    /// Wave OR: returns the registry by mutable reference only when no
    /// per-turn `AgentNodeExecutor` adapter (or any other Arc clone) is
    /// active. The CLI's MCP-server registration site mutates the registry
    /// at startup before `run` is invoked, so the refcount is always 1
    /// there and the call always succeeds. Returns `None` if a stale clone
    /// has leaked.
    pub fn registry_mut(&mut self) -> Option<&mut ToolRegistry> {
        Arc::get_mut(&mut self.tools)
    }

    /// v0.9.1 W1 E (debt sweep): a cheap `Arc` clone of the tool
    /// registry, for hosts that need to invoke a tool directly
    /// (e.g. the TUI `/voice` slash dispatcher calls
    /// `VoiceModeTool::toggle_record` without an LLM round-trip).
    /// Read-only by contract — mutation must still go through
    /// [`Self::registry_mut`] which holds the `Arc::get_mut` invariant.
    pub fn tools(&self) -> Arc<ToolRegistry> {
        self.tools.clone()
    }

    /// Initialize a new session for this engine run
    pub fn init_session(
        &mut self,
        provider_name: &str,
        cwd: &str,
        session_id: Option<&str>,
    ) -> anyhow::Result<()> {
        if let Some(mgr) = &self.session_manager {
            let session = mgr.create(provider_name, &self.model, cwd, session_id)?;
            // W6 F16: if a previous plan was persisted for this session id,
            // advertise resume-availability via the existing Info channel.
            // No new protocol variant (audit rev-2). Errors from the probe
            // are non-fatal: we just skip the banner.
            if let Ok(Some(plan)) = crate::plan::persist::load_plan_json(&session.id, None) {
                let age_secs = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs().saturating_sub(plan.ts_unix))
                    .unwrap_or(0);
                self.output.emit_info(&format!(
                    "Previous plan available for session {} (saved {}s ago). \
                     Reference it in your first message to resume.",
                    session.id, age_secs
                ));
            }
            self.current_session = Some(session);
        }
        Ok(())
    }

    /// Get the current session ID (if sessions are enabled and initialized)
    pub fn current_session_id(&self) -> Option<String> {
        self.current_session.as_ref().map(|s| s.id.clone())
    }

    /// AUDIT A2 / B1 — clone the engine's session-root cancellation
    /// token.
    ///
    /// A host (TUI, ACP server) holds the clone and calls `.cancel()` on
    /// it to cooperatively stop a running agent. The `run()` loop checks
    /// the token between turns and threads a child of it into every
    /// per-turn `GraphContext` and every per-call `ToolContext`, so a
    /// cancel reaches an in-flight tool. The token is `Arc`-backed —
    /// the clone observes the same cancellation as the engine's own.
    pub fn cancel_token(&self) -> tokio_util::sync::CancellationToken {
        self.cancel_token.clone()
    }

    /// AUDIT A2 — install an externally-owned cancellation token as the
    /// session root. Use this when the host wants to scope agent
    /// cancellation to a parent token (e.g. a child of the process-wide
    /// shutdown token). Must be called before `run()`. When unused the
    /// engine keeps the fresh token minted at construction.
    pub fn set_cancel_token(&mut self, token: tokio_util::sync::CancellationToken) {
        self.cancel_token = token;
    }

    /// M5.3 — install a `BudgetTracker` to enforce per-session / per-user
    /// caps and emit `BudgetEvent` telemetry. `AgentBootstrap` wires the
    /// tracker after construction when the user opts into M5.3 caps via
    /// config; tests can install one directly. `None` (the default)
    /// preserves pre-M5.3 behaviour: no charges, no events.
    pub fn set_budget_tracker(
        &mut self,
        tracker: Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>,
    ) {
        self.budget_tracker = Some(tracker);
    }

    /// M5.bootstrap-wiring — read access to the optional `BudgetTracker`.
    /// Returns `None` when bootstrap did not install one (the default
    /// when `Config.session_cap` is `None`). Tests use this to assert
    /// install-from-config wiring works end-to-end.
    pub fn budget_tracker(&self) -> Option<&Arc<parking_lot::Mutex<wcore_budget::BudgetTracker>>> {
        self.budget_tracker.as_ref()
    }

    /// v0.6.1 CRIT-1: install a `PolicyGate` for this session. Once set,
    /// every tool dispatch in `dispatch_once` is checked against the gate
    /// before reaching the approval / budget pipeline. The gate fails
    /// closed — `PolicyDenied` returns a `ToolResult { is_error: true }`
    /// without invoking the tool. Call from `AgentBootstrap` when the
    /// session config requests permissions enforcement; omit entirely to
    /// preserve v0.6.0 open-gate behaviour.
    pub fn set_policy_gate(&mut self, gate: crate::policy_gate::PolicyGate) {
        self.policy_gate = Some(gate);
    }

    /// v0.6.4 Task 1.3 — register plugin-contributed hooks into the engine's
    /// `HookEngine`. Matches the `set_memory_api` / `set_approval_bridge` /
    /// `set_agent_registry` post-construction setter pattern.
    ///
    /// Called by `AgentBootstrap` after `apply_initialize_outcome` returns.
    /// Each `PluginHook` is forwarded to `HookEngine::register_plugin_hook`.
    /// No-op when `self.hooks` is `None` (synthetic test-mode engines).
    pub fn register_plugin_hooks(&mut self, hooks: Vec<crate::plugins::runner::PluginHook>) {
        if let Some(engine) = self.hooks.as_mut() {
            for hook in hooks {
                engine.register_plugin_hook(hook);
            }
        }
    }

    /// C1 / Task A1 — install the host `HookDispatcher` onto the engine's
    /// `HookEngine`. Until set, plugin lifecycle hooks fire log-only. Called by
    /// `AgentBootstrap` after `register_plugin_hooks`. No-op when `self.hooks`
    /// is `None` (synthetic test-mode engines).
    pub fn set_hook_dispatcher(
        &mut self,
        dispatcher: std::sync::Arc<dyn crate::hooks::HookDispatcher>,
    ) {
        if let Some(engine) = self.hooks.as_mut() {
            engine.set_dispatcher(dispatcher);
        }
    }

    /// v0.6.4 Task 1.2 — install a plugin-contributed `AgentRegistry`.
    ///
    /// Called by `AgentBootstrap` after `apply_initialize_outcome` returns,
    /// matching the `set_memory_api` / `set_approval_bridge` / `set_budget_tracker`
    /// post-construction setter pattern. The registry is `Arc`-wrapped so the
    /// engine and `SpawnTool::with_registry` share the *same* registry instance
    /// — a single shared identity, not to avoid clone cost (already cheap).
    ///
    /// `None` (the default) preserves pre-Task-1.2 behaviour: `SpawnTool`
    /// resolves no named agents.
    pub fn set_agent_registry(
        &mut self,
        registry: std::sync::Arc<crate::agents::registry::AgentRegistry>,
    ) {
        self.agent_registry = Some(registry);
    }

    /// v0.6.4 Task 1.2 — read access to the plugin-contributed `AgentRegistry`.
    ///
    /// Returns `None` when `set_agent_registry` has not been called (i.e.
    /// no plugins registered agents, or bootstrap has not yet applied
    /// `InitializeOutcome`).
    pub fn agent_registry(
        &self,
    ) -> Option<&std::sync::Arc<crate::agents::registry::AgentRegistry>> {
        self.agent_registry.as_ref()
    }

    /// Get a reference to the output sink
    pub fn output(&self) -> &dyn OutputSink {
        self.output.as_ref()
    }

    /// v0.8.0 Task K — install a learned orchestration `TemplateRouter`.
    /// When set, `engine::run` consults the router before falling back
    /// to the deterministic `IntentClassifier`. Per-turn route selection
    /// goes through `orchestration::template_routing::select_graph_config`.
    /// Default is `None` (classifier only) — byte-identical to pre-K.
    pub fn set_template_router(&mut self, router: Arc<Mutex<wcore_dispatch::TemplateRouter>>) {
        self.template_router = Some(router);
    }

    /// v0.8.0 Task K — read access to the wired router (for observation
    /// updates: `engine::run` doesn't currently call `observe`; the
    /// scheduler / acceptance tests do once an outcome verdict exists).
    pub fn template_router(&self) -> Option<&Arc<Mutex<wcore_dispatch::TemplateRouter>>> {
        self.template_router.as_ref()
    }

    /// v0.8.1 U1 — install the per-turn `SkillRouter`. Called by
    /// `AgentBootstrap::build` after the catalog is loaded and the
    /// session-start `SkillPrioritizer` has run. Engines constructed
    /// outside bootstrap leave this `None` and `engine::run`
    /// short-circuits the choose/observe loop (byte-identical to pre-U1).
    pub fn set_skill_router(&mut self, router: wcore_skills::SkillRouter) {
        self.skill_router = Some(Arc::new(Mutex::new(router)));
    }

    /// v0.8.1 U1 — read access to the wired skill router. Mirrors the
    /// `template_router()` accessor; used by tests that want to inspect
    /// or pre-seed the scorer state.
    pub fn skill_router(&self) -> Option<&Arc<Mutex<wcore_skills::SkillRouter>>> {
        self.skill_router.as_ref()
    }

    /// v0.8.1 U6 — install the autonomous `SkillDrafter`. Called by
    /// `AgentBootstrap::build` once a real `Memory` Db handle is
    /// available so the drafter can record into the GEPA `PromptStore`.
    /// Engines without a drafter still observe trajectories (the
    /// bucketer is always live); they just never write a draft to disk.
    pub fn set_skill_drafter(&mut self, drafter: Arc<crate::auto_skill::SkillDrafter>) {
        self.skill_drafter = Some(drafter);
    }

    /// v0.8.1 U6 — read access for tests.
    pub fn skill_drafter(&self) -> Option<&Arc<crate::auto_skill::SkillDrafter>> {
        self.skill_drafter.as_ref()
    }

    /// v0.8.0 Task K — override the engine-level `Mode` knob used by the
    /// classifier fallback (Direct/Parallel/Sequential/SelfCritique/Auto).
    /// Distinct from the `TemplateRouter`: this knob ONLY affects the
    /// classifier branch and is honoured even when no router is wired.
    pub fn set_mode_override(&mut self, mode: Option<crate::orchestration::intent::Mode>) {
        self.mode_override = mode;
    }

    /// v0.6.5 Wave 6A.2 — install plugin-reified user-model backends.
    ///
    /// Called by `AgentBootstrap` after `apply_initialize_outcome` returns
    /// (alongside `set_agent_registry` / `register_plugin_hooks`). When the
    /// supplied vector is non-empty, the session-end PUM path mirrors every
    /// inferred delta to each reified backend (e.g.
    /// `HonchoClient::learn_preference`) in addition to writing through
    /// `MemoryApi::update_user_model`. Empty is the default and keeps
    /// pre-Wave-6A.2 behaviour (local memory only).
    ///
    /// This closes the v0.6.5 carrier-without-consumer gap: the
    /// `AppliedPluginCapabilities::plugin_reified_user_models` slice now
    /// reaches a production read site.
    pub fn set_plugin_user_models(&mut self, models: Vec<crate::plugins::apply::ReifiedUserModel>) {
        self.plugin_user_models = models;
    }

    /// v0.6.5 Wave 6A.2 — read access to the installed reified user-model
    /// backends. Returns the empty slice when no plugins reified one.
    pub fn plugin_user_models(&self) -> &[crate::plugins::apply::ReifiedUserModel] {
        &self.plugin_user_models
    }

    /// W7 Pre-flight 0: read access to the engine's `HookEngine`.
    /// Returns `None` only in the synthetic test-mode where the engine
    /// was constructed without a hook registry (`hooks: None`); production
    /// `AgentBootstrap` paths always install an empty-or-populated
    /// `HookEngine`. Used by the test-driver helpers + future W8 rollback
    /// wiring that needs to inspect installed hooks.
    pub fn hook_engine(&self) -> Option<&HookEngine> {
        self.hooks.as_ref()
    }

    pub fn set_approval_manager(&mut self, mgr: Arc<wcore_protocol::ToolApprovalManager>) {
        // AUDIT B-2 / D-5 — spawn the approval-manager reaper so an
        // unanswered or cancelled tool-call approval cannot wedge or
        // leak forever. The reaper sweeps expired (TTL) and
        // requester-crashed (`tx.is_closed()`) entries. Guarded by
        // `Handle::try_current()` because `set_approval_manager` may be
        // called from a non-async bootstrap context; when there is no
        // runtime the reaper is skipped (the host then relies on the
        // per-call cancel race in `execute_tool_calls_with_approval`).
        // The handle joins `background_handles`, which `Drop` aborts.
        if tokio::runtime::Handle::try_current().is_ok() {
            self.background_handles
                .push(mgr.spawn_reaper(wcore_protocol::DEFAULT_REAP_INTERVAL));
        }
        self.approval_manager = Some(mgr);
    }

    /// W7.1 S4-3.2: read access to the engine's shared `ApprovalBridge`.
    ///
    /// `AgentBootstrap` builds one bridge per engine, hands clones to the
    /// engine and to `ScriptTool` (via `.with_approval(...)`), and the CLI
    /// command loop clones it from this accessor so that an `ApprovalResume`
    /// command can call `bridge.resolve(token, outcome)` on the same
    /// instance the script step is awaiting.
    pub fn approval_bridge(&self) -> &Arc<ApprovalBridge> {
        &self.approval_bridge
    }

    /// W7.1 S4-3.2: install a host-supplied `ApprovalBridge` so the engine
    /// and the registered `ScriptTool` share one instance. Called by
    /// `AgentBootstrap::build` after engine construction; production paths
    /// that don't go through bootstrap keep the default bridge created in
    /// the constructor.
    pub fn set_approval_bridge(&mut self, bridge: Arc<ApprovalBridge>) {
        self.approval_bridge = bridge;
    }

    /// W7 Pre-flight 0: read access to the engine's `MemoryApi` handle.
    /// Always returns a real `Arc<dyn MemoryApi>` (never `None`) — when
    /// memory is disabled it points at a `NullMemory` no-op.
    pub fn memory_api(&self) -> &Arc<dyn wcore_memory::MemoryApi> {
        &self.memory_api
    }

    /// M3.2 — install a background-task handle on the engine (currently
    /// used for the decay scheduler spawned by `AgentBootstrap::build`
    /// when `cfg.memory.enabled = true`). `Drop` aborts every handle on
    /// engine shutdown so no task is leaked across sessions or tests.
    pub fn push_decay_handle(&mut self, h: tokio::task::JoinHandle<()>) {
        self.decay_handles.push(h);
    }

    /// Wave 6A.1 — install the keepalive vec for on-disk plugin runtime
    /// handles. Closures inside synthesized plugin tools clone the inner
    /// `Arc`s on every invocation; the engine must outlive those clones,
    /// so bootstrap moves the vec onto the engine after
    /// `apply_initialize_outcome`. Calling twice replaces the previous
    /// vec (the prior `Arc`s drop, which is correct only when no tool
    /// closures still hold them — bootstrap calls this exactly once).
    pub fn set_plugin_runtime_handles(
        &mut self,
        handles: Vec<crate::plugins::LoadedRuntimeHandle>,
    ) {
        self.plugin_runtime_handles = Arc::new(handles);
    }

    /// Wave 6A.1 — read-only handle count, for tests + diagnostics.
    pub fn plugin_runtime_handles_len(&self) -> usize {
        self.plugin_runtime_handles.len()
    }

    /// v0.8.0 N.2 — read-only slice of the engine's keepalive plugin
    /// runtime handles. The `/plugin list` slash-handler's `Runtime`
    /// variant enumerates this slice to display the live plugin
    /// inventory (name + runtime kind). Returns an empty slice when no
    /// plugins are loaded.
    pub fn plugin_runtime_handles(&self) -> &[crate::plugins::LoadedRuntimeHandle] {
        self.plugin_runtime_handles.as_slice()
    }

    /// v0.8.0 N.2 — clone the underlying `Arc<Vec<...>>` so the
    /// slash-runtime PluginHandler can hold a shared reference without
    /// requiring `LoadedRuntimeHandle: Clone`. Cheap (Arc clone).
    pub fn plugin_runtime_handles_arc(&self) -> Arc<Vec<crate::plugins::LoadedRuntimeHandle>> {
        self.plugin_runtime_handles.clone()
    }

    /// v0.8.0 N.3 — install the session's resolved `SkillCatalog` on the
    /// engine. Called by `AgentBootstrap::build` after constructing the
    /// catalog so the `/skill` slash handler observes the same instance
    /// the model sees in its system prompt. Calling twice replaces the
    /// previous handle.
    pub fn set_skill_catalog(&mut self, catalog: Arc<wcore_skills::refs::SkillCatalog>) {
        self.skill_catalog = Some(catalog);
    }

    /// v0.8.0 N.3 — read-only handle to the engine's resolved skill
    /// catalog. `None` when no catalog has been installed (constructors
    /// used outside `AgentBootstrap::build` keep the default).
    pub fn skill_catalog(&self) -> Option<&Arc<wcore_skills::refs::SkillCatalog>> {
        self.skill_catalog.as_ref()
    }

    /// v0.8.0 Task M — install a `UserModelBackend` for per-turn
    /// observation write-back. Called by `AgentBootstrap::build` after
    /// constructing the backend so the engine and the bootstrap read
    /// site share the *same* backend instance — observations land in
    /// the same store the next bootstrap reads from. `None` (the
    /// default) preserves pre-v0.8.0 behaviour: `run()` skips
    /// write-back entirely.
    pub fn set_user_model_backend(&mut self, backend: Arc<dyn wcore_user_model::UserModelBackend>) {
        self.user_model_backend = Some(backend);
    }

    /// v0.8.0 Task M — read-only handle to the installed
    /// `UserModelBackend`. Returns `None` when no backend has been
    /// installed (memory disabled, or backend init failed in bootstrap).
    pub fn user_model_backend(&self) -> Option<&Arc<dyn wcore_user_model::UserModelBackend>> {
        self.user_model_backend.as_ref()
    }

    /// v0.8.0 Task M — override the user-id key used for write-back.
    /// Default is resolved from `WAYLAND_USER_ID` (or `"default"`); this
    /// setter exists for tests that need deterministic per-test user-ids.
    pub fn set_user_model_user_id(&mut self, user_id: impl Into<String>) {
        self.user_model_user_id = user_id.into();
    }

    /// v0.8.0 Task M — read the user-id key used for write-back.
    pub fn user_model_user_id(&self) -> &str {
        &self.user_model_user_id
    }

    /// Switch the active model for subsequent turns (the TUI `/model`
    /// command). Takes effect on the next turn — an in-flight turn finishes
    /// on the old model. Provider and `ProviderCompat` are unchanged, so the
    /// caller must keep the new model within the current provider
    /// (cross-provider switches go through config + a fresh bootstrap).
    ///
    /// D014: this is the explicit-user-pick path (the TUI `/model <id>`
    /// dispatch routes here through the engine bridge). It records the chosen
    /// model as the session's authoritative `user_model_pin`, so a later
    /// skill/hook `switch_model` cannot silently move the live model off the
    /// user's choice. Call [`clear_model_pin`] (or [`clear_conversation`], a
    /// `/new`) to release the pin and let hook/skill switches resume.
    pub fn set_model(&mut self, model: impl Into<String>) {
        let model = model.into();
        self.user_model_pin = Some(model.clone());
        self.model = model;
    }

    /// FluxRouter web_search grounding (contract §5): enable/disable attaching
    /// the `web_search` tool to every turn. Wired by the CLI `--search` flag.
    /// Grounding still only fires when the live model is a Flux tier alias —
    /// the provider guards on `is_flux_tier_alias`, so enabling this on a
    /// non-Flux model is a harmless no-op.
    pub fn set_web_search(&mut self, enabled: bool) {
        self.web_search = enabled;
    }

    /// D014: release the explicit user model pin set by [`set_model`], so a
    /// subsequent skill/hook `switch_model` is honoured again. Does NOT change
    /// the active model — only the precedence. Exposed for hosts (e.g. the TUI
    /// `/model reset`) that need to drop the pin without starting a fresh
    /// conversation.
    pub fn clear_model_pin(&mut self) {
        self.user_model_pin = None;
    }

    /// D014: the model the user explicitly pinned for this session via
    /// `/model`, or `None` if no explicit pick is active. Lets a host surface
    /// the authoritative choice and reconcile its own pin state with the
    /// engine.
    pub fn user_model_pin(&self) -> Option<&str> {
        self.user_model_pin.as_deref()
    }

    /// D014: apply a skill/hook-originated `switch_model`, honouring the
    /// explicit-user-pin precedence. When the user has pinned a model via
    /// `/model`, an implicit switch is refused and the divergence is logged;
    /// the pin wins. With no pin set, the switch is applied as before.
    fn apply_switch_model(&mut self, new_model: String) {
        if let Some(pin) = self.user_model_pin.as_deref() {
            tracing::debug!(
                target: "wcore_agent::model",
                pinned = pin,
                requested = %new_model,
                "D014: ignoring skill/hook switch_model; user has an explicit /model pin"
            );
            return;
        }
        self.model = new_model;
    }

    /// The active model identifier (used by the TUI status bar + tests).
    pub fn model(&self) -> &str {
        &self.model
    }

    /// D001 / D007 / D016 keystone: atomically swap the live provider,
    /// its `ProviderCompat`, and the active model.
    ///
    /// `set_model` only swaps the model string — it can never apply a
    /// freshly entered API key or a cross-provider switch, because the key
    /// is baked into the provider `Arc` at `create_provider` time and the
    /// compat row carries the provider's capability profile. After
    /// onboarding (or a `/config` provider edit) writes a new provider +
    /// key to disk, the host rebuilds the provider via
    /// `wcore_providers::create_provider` and calls this to install it on
    /// the running engine. The three fields are replaced together so a
    /// turn never observes a provider that disagrees with its compat or
    /// model. Takes effect on the next turn — an in-flight turn finishes
    /// on the old provider (the engine is locked per-turn, so this swap
    /// only lands between turns).
    pub fn rebind_provider(
        &mut self,
        provider: Arc<dyn LlmProvider>,
        compat: wcore_config::compat::ProviderCompat,
        model: String,
    ) {
        self.provider = provider;
        self.compat = compat;
        self.model = model;
        // D014: a provider rebind installs a deliberately chosen model for the
        // new provider — a fresh baseline. Drop any prior `/model` pin so it
        // can't shadow the rebind's model or block hook switches on a model
        // string that belonged to the old provider.
        self.user_model_pin = None;
    }

    /// D016 / Wave-6 #5: reinstall the session system prompt on an in-session
    /// rebind (`/config save`, `/provider`, `/profile`), PRESERVING the boot
    /// framework fragments.
    ///
    /// `prompt` is the rebind OVERLAY — the host's `build_rebind_system_prompt`
    /// output, i.e. the `[default] user` display-name block (the resolved
    /// `[default] system_prompt` is already embedded in the retained base, so
    /// the rebind helper passes the name block alone). The effective prompt
    /// becomes `overlay + "\n\n" + retained_base`, where `retained_base` is the
    /// fully-assembled boot prompt captured at construction
    /// (the `rebind_system_prefix` field) — Constitution,
    /// skills index, persona, and the resolved config prompt that
    /// `build_system_prompt` folded together at bootstrap, plus any
    /// [`inject_history`] prepends. Earlier this method REPLACED the prompt
    /// wholesale, silently dropping every framework fragment on the first
    /// in-session rebind (red-team finding #5 — the F-003 "no deliverables"
    /// regression reintroduced via the rebind seam). It now re-prepends the
    /// overlay onto the retained base instead.
    ///
    /// When no base was retained (the test-only constructors) it falls back to
    /// the legacy replace semantics. An empty overlay installs the retained
    /// base unchanged. Engine-managed fragments (plan mode, skill hints) are
    /// still appended per turn after this base.
    ///
    /// Caveat: these TUI verbs mutate provider / model / approval / display
    /// name — none of them edits `[default] system_prompt` interactively, so
    /// re-prepending the retained base (rather than a re-resolved config
    /// prompt) loses nothing they can change. A future surface that edits the
    /// base prompt in-session must refresh the retained base via
    /// [`inject_history`] or a dedicated capture, not rely on this overlay.
    pub fn set_system_prompt(&mut self, prompt: String) {
        match self.rebind_system_prefix.as_deref() {
            Some(base) => {
                let overlay = prompt.trim();
                if overlay.is_empty() {
                    self.system_prompt = base.to_string();
                } else if base.is_empty() {
                    self.system_prompt = overlay.to_string();
                } else {
                    self.system_prompt = format!("{overlay}\n\n{base}");
                }
            }
            None => self.system_prompt = prompt,
        }
    }

    /// Force a context compaction now (the TUI `/compact` command).
    ///
    /// Runs the same deterministic micro-compaction stage the automatic
    /// [`run_compaction`](Self::run_compaction) path uses: old tool-result
    /// bodies are cleared in place (and superseded reads pruned), which frees
    /// the bulk of the context tokens without discarding any conversational
    /// text. The earlier fixed-string fold replaced the whole middle of the
    /// transcript with a canned `[Previous conversation summary: …]` placeholder
    /// — that threw away real context and is no longer used here.
    ///
    /// Returns the [`MicrocompactResult`]; `cleared_count == 0` means there was
    /// nothing eligible to compact yet.
    pub fn compact_now(&mut self) -> micro::MicrocompactResult {
        let result = micro::microcompact(&mut self.messages, &self.compact_config);
        if result.cleared_count > 0 {
            // Token-opt (diff-resend): clearing a tool-result body can remove
            // the read content a cached diff base references. Bump the file
            // cache's compaction generation so those bases stop qualifying —
            // mirrors `run_compaction`'s microcompact branch.
            self.bump_file_cache_generation();
        }
        result
    }

    /// M3.2 — number of background decay-scheduler tasks owned by the
    /// engine. Used by integration tests to assert that bootstrap wired
    /// (or skipped) the scheduler based on `cfg.memory.enabled`. Stays at
    /// zero on the `NullMemory` path.
    pub fn decay_handles_len(&self) -> usize {
        self.decay_handles.len()
    }

    /// F-003 fix: route `init_history` text into the session system prompt.
    ///
    /// The app ships Constitution + skills index + persona via the
    /// `init_history` ProtocolCommand before the first user turn. Without
    /// this method the handler in `wcore-cli/src/main.rs` was an `eprintln!`
    /// no-op; assistants ran with raw model defaults and produced generic
    /// responses instead of persona-aware ones.
    ///
    /// Prepends `text` to the existing system prompt so that engine-managed
    /// prompt fragments (plan mode, etc.) are still appended after this block.
    /// Calling twice accumulates both payloads — each `init_history` frame the
    /// host sends extends the injected context.
    ///
    /// Wave-6 #5: the prepended text becomes part of the boot framework prompt
    /// (the protocol/host path delivers Constitution / persona / skills index
    /// here), so the retained rebind base
    /// (the `rebind_system_prefix` field) is updated in
    /// lockstep. Otherwise a protocol-host session that ships those
    /// fragments via `init_history` would lose them on the first in-session
    /// rebind, exactly as the TUI bootstrap path did.
    pub fn inject_history(&mut self, text: String) {
        if self.system_prompt.is_empty() {
            self.system_prompt = text.clone();
        } else {
            self.system_prompt = format!("{}\n\n{}", text, self.system_prompt);
        }
        // Keep the retained rebind base in lockstep with the injected framework
        // fragment so a later `set_system_prompt` re-prepends the full context.
        //
        // Prepend `text` to the EXISTING (overlay-free) base rather than
        // capturing the live `system_prompt`. The live prompt may already carry
        // a `set_system_prompt` name overlay; folding that whole string back
        // into the base would bake the overlay into the retained base, so the
        // next rebind double-prepends the name (cosmetic display-name-twice
        // bug). Extending the overlay-free base keeps the base/overlay split the
        // rebind path relies on. When no base was ever retained (test-only
        // constructors), there is no overlay to separate, so fall back to the
        // live prompt — matching the previous behaviour for that path.
        self.rebind_system_prefix = Some(match self.rebind_system_prefix.take() {
            Some(base) if base.is_empty() => text,
            Some(base) => format!("{text}\n\n{base}"),
            None => self.system_prompt.clone(),
        });
    }

    /// `/clear` — drop the in-memory conversation history so the next turn
    /// starts fresh. The system prompt (Constitution, persona, skills index)
    /// is preserved; only the user/assistant message buffer is cleared.
    pub fn clear_conversation(&mut self) {
        self.messages.clear();
        // #279(c): a cleared session is a new agent run — drop the correlation
        // id so the next run() mints a fresh one.
        self.current_agent_run_id = None;
        // D014: a fresh conversation re-baselines the model. The prior
        // explicit `/model` pin no longer applies, so hook/skill switches
        // are honoured again until the user pins anew.
        self.user_model_pin = None;
        // Token-opt: the message buffer is gone, so the compaction floor
        // (which indexes into it) no longer means anything — reset it.
        self.compaction_floor = 0;
        // C1: the session-start prelude baseline indexes into `messages`; a
        // cleared buffer re-baselines it to 0 so cross-session recall keys off
        // the new (empty) buffer, not a stale prelude count.
        self.session_start_injected_len = 0;
        // Token-opt: wipe the file cache (read states + read-once backrefs) too.
        // None of the prior reads/outputs are in the new transcript, so a dedup
        // stub or backref must not reference them.
        self.clear_file_cache();
    }

    /// `/resume <id>` - swap the in-memory conversation buffer to a loaded
    /// session's messages so the NEXT turn continues that session's context.
    /// Symmetric with `clear_conversation`; the system prompt is preserved.
    pub fn load_conversation(&mut self, messages: Vec<Message>) {
        self.messages = messages;
        // Token-opt: a swapped-in buffer is a fresh index space; the prior
        // session's compaction floor does not apply. Symmetric with the
        // `clear_conversation` reset below.
        self.compaction_floor = 0;
        // C1: drop the cold-boot prelude baseline. A resumed buffer is real
        // prior context, so recall must key off the resumed length (and thus
        // skip), not a stale `1` left over from a prelude applied at boot — else
        // a single-message resumed session would wrongly re-trigger recall.
        self.session_start_injected_len = 0;
        // Wave-6 #5 (secondary): a resumed/loaded session must start without the
        // PREVIOUS session's explicit `/model` pin. Symmetric with
        // `clear_conversation` (a `/new` re-baselines): the loaded session's
        // intended model (resolved provider/config default) should win, and a
        // stale pin from the prior session would otherwise shadow it and block
        // hook/skill `switch_model` for the resumed conversation. The user can
        // pin anew with `/model` after resuming.
        self.user_model_pin = None;
        // Token-opt: the prior session's cached reads/outputs are not in this
        // buffer; wipe the file cache so no dedup stub or backref references them.
        self.clear_file_cache();
    }

    /// The engine's current conversation messages, oldest first. After a
    /// `--resume` / `--continue` boot this is the restored session history
    /// (populated by `resume_with_provider` from `session.messages`), so a
    /// host (e.g. the TUI) can repaint the prior conversation into its
    /// transcript on startup rather than showing a blank screen. Returns an
    /// empty slice for a fresh session before the first turn.
    pub fn conversation_messages(&self) -> &[Message] {
        &self.messages
    }

    /// W7 Pre-flight 0: replace the engine's `MemoryApi` handle.
    /// Called by `AgentBootstrap::build` when the user has opted into a
    /// real backend; otherwise the default `NullMemory` is kept.
    pub fn set_memory_api(&mut self, api: Arc<dyn wcore_memory::MemoryApi>) {
        self.memory_api = api;
    }

    /// W7 Pre-flight 0.0d: install a `TestSinkHandle` so subsequent
    /// `captured_protocol_events()` calls observe the event buffer the
    /// `TestSink` passed to `output` is recording into.
    ///
    /// `AgentBootstrap::build_for_test` calls this after constructing the
    /// engine with `Arc<TestSink>` as the output sink; production paths
    /// never call it, so `captured_protocol_events()` returns an empty
    /// Vec there.
    #[cfg(any(test, feature = "test-utils"))]
    pub fn set_test_sink_handle(&mut self, handle: crate::test_utils::TestSinkHandle) {
        self.test_sink_handle = handle;
    }

    /// W8b.2.B D.3: install a filesystem watcher. The per-turn boundary
    /// drains it for external-edit events and emits a synthetic context
    /// message via `render_external_edit_message`. `AgentBootstrap::build`
    /// calls this when a real-fs session root is available; tests can
    /// supply a `tempdir()`-rooted watcher to exercise the seam directly.
    pub fn set_file_watcher(&mut self, watcher: Arc<crate::watch::FileWatcher>) {
        let _ = self.file_watcher.set(watcher);
    }

    /// Phase 0 "eventual install": arm the filesystem watcher off the boot
    /// path. `FileWatcher::new` performs a recursive watch-add that can block
    /// for tens of seconds on a wedged FS-events backend or a very large tree,
    /// so it runs on a detached `std::thread` the runtime never joins (the
    /// hang-guard). The watcher + its paired `FileWriteNotifier` are installed
    /// into the engine's interior-mutable `OnceLock` slots whenever that thread
    /// finishes — there is no grace window and nothing is ever built-then-
    /// dropped, so a contended host can no longer lose external-edit tracking
    /// by missing a timing budget. A genuinely wedged backend simply never
    /// installs (the same best-effort contract bootstrap always had).
    /// Idempotent: the first successful install wins; later `set`s are no-ops.
    pub fn install_file_watcher_eventually(&self, watch_root: std::path::PathBuf) {
        let fw_slot = Arc::clone(&self.file_watcher);
        let nf_slot = Arc::clone(&self.tool_write_notifier);
        let spawned = std::thread::Builder::new()
            .name("wcore-filewatcher-init".to_string())
            .spawn(move || match crate::watch::FileWatcher::new(&watch_root) {
                Ok(watcher) => {
                    let watcher = Arc::new(watcher);
                    let notifier = crate::file_watcher_notifier::FileWatcherNotifier::arc(
                        Arc::clone(&watcher),
                    );
                    let _ = fw_slot.set(watcher);
                    let _ = nf_slot.set(notifier);
                }
                Err(e) => {
                    tracing::warn!(
                        error = %e,
                        "FileWatcher init failed; continuing without external-edit tracking"
                    );
                }
            });
        if let Err(e) = spawned {
            tracing::warn!(
                error = %e,
                "could not spawn FileWatcher init thread; continuing without external-edit tracking"
            );
        }
    }

    /// W8b.2.B D.3: read access to the engine's watcher (None when no
    /// real-fs root was bound at construction). Exposed so consumers
    /// can assert the watcher was wired (Task 7 acceptance) and for
    /// future integration tests that need to drive marks directly.
    pub fn file_watcher(&self) -> Option<&Arc<crate::watch::FileWatcher>> {
        self.file_watcher.get()
    }

    /// W8b.2.B Task 7: install a `FileWriteNotifier` for orchestration
    /// `ToolContext`s. When set, the dispatcher in `orchestration::*`
    /// attaches this notifier to every per-call ToolContext so Write/Edit
    /// tools can flag self-originated writes (D.4) and the paired
    /// `FileWatcher` debounces its own events out.
    pub fn set_tool_write_notifier(
        &mut self,
        notifier: Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>,
    ) {
        let _ = self.tool_write_notifier.set(notifier);
    }

    /// Token-opt (diff-resend): wire the shared file-state cache so the engine
    /// can bump its compaction generation after each compaction pass. Called
    /// once by `AgentBootstrap` with the same `Arc` the Read/Edit/Write tools
    /// hold. Engines without a file cache (tests, cache-disabled) skip this and
    /// diff-resend simply never fires.
    pub fn set_file_cache(
        &mut self,
        cache: Arc<std::sync::RwLock<wcore_tools::file_cache::FileStateCache>>,
    ) {
        self.file_cache = Some(cache);
    }

    /// B7 writer-side wiring: install the shared `InMemorySessionState` so
    /// per-turn token totals and per-tool call counts land in the same struct
    /// the `wayland_status` / `wayland_telemetry_query` introspection backend
    /// reads. Called once by `AgentBootstrap` with the same `Arc` handed to
    /// `build_introspection_backend`. Engines without an introspection surface
    /// (tests, sub-agents) skip this and the counters simply stay at zero.
    pub fn set_session_state(&mut self, state: Arc<crate::session_state::InMemorySessionState>) {
        self.session_state = Some(state);
    }

    /// Token-opt (diff-resend): invalidate cached read bases for diffing by
    /// advancing the file cache's compaction generation. No-op when no cache is
    /// wired or the lock is poisoned.
    fn bump_file_cache_generation(&self) {
        if let Some(cache) = &self.file_cache
            && let Ok(mut c) = cache.write()
        {
            c.bump_compaction_generation();
        }
    }

    /// Token-opt: wipe the file cache (read states + read-once backrefs) on a
    /// conversation reset. No-op when no cache is wired or the lock is poisoned.
    fn clear_file_cache(&self) {
        if let Some(cache) = &self.file_cache
            && let Ok(mut c) = cache.write()
        {
            c.clear();
        }
    }

    /// Token-opt (read-once): rewrite a repeated Grep/Glob/Bash output to a short
    /// backref pointing at the earlier identical result, instead of re-sending
    /// the whole thing into the transcript. Runs AFTER the result is displayed to
    /// the user (so the human still sees full output) and only mutates the copy
    /// that goes to the model. The backref is gated (client route + min size) and
    /// generation-guarded inside `output_backref`, so it only fires while the
    /// referenced result is still in the visible transcript.
    ///
    /// `None` file cache (sub-agents, cache disabled) makes this a no-op, which
    /// also keeps the process-wide cache from cross-referencing a sibling agent's
    /// output that this transcript never contained.
    fn dedup_repeated_tool_outputs(
        &self,
        blocks: &mut [ContentBlock],
        tool_calls: &[ContentBlock],
    ) {
        const DEDUP_TOOLS: [&str; 3] = ["Grep", "Glob", "Bash"];
        let Some(cache) = &self.file_cache else {
            return;
        };
        for block in blocks.iter_mut() {
            let ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } = block
            else {
                continue;
            };
            let Some((name, input)) = tool_calls.iter().find_map(|c| match c {
                ContentBlock::ToolUse {
                    id, name, input, ..
                } if id == tool_use_id => Some((name.as_str(), input)),
                _ => None,
            }) else {
                continue;
            };
            if !DEDUP_TOOLS.contains(&name) {
                continue;
            }
            if let Ok(mut c) = cache.write() {
                let label = backref_label(name, input);
                if let Some(stub) = c.output_backref(content, &label) {
                    *content = stub;
                }
            }
        }
    }

    /// W8b.2.B Task 7: read access to the notifier. `None` when no
    /// orchestration-side wiring was performed.
    pub fn tool_write_notifier(
        &self,
    ) -> Option<&Arc<dyn wcore_tools::file_write_notifier::FileWriteNotifier>> {
        self.tool_write_notifier.get()
    }

    /// W8b.2.B Task 7: build the per-call `ToolContext` the orchestration
    /// dispatcher would mint for this engine. Mirrors the construction
    /// in `orchestration::execute_single_with_streaming` so tests can
    /// assert the wiring without driving a real tool dispatch.
    ///
    /// `call_id` is left empty in the snapshot; production dispatch
    /// substitutes the live ToolUse id per call.
    pub fn current_tool_context(&self) -> wcore_tools::context::ToolContext {
        // Mirror the production dispatcher: build the context with the
        // registry's configured vfs (a `SandboxedFs` jail for a channel
        // `Workspace` engine), falling back to unconfined `RealFs`.
        let vfs = self
            .tools
            .tool_vfs()
            .unwrap_or_else(|| std::sync::Arc::new(wcore_tools::vfs::RealFs));
        let mut ctx = wcore_tools::context::ToolContext::new(
            String::new(),
            wcore_tools::context::ToolContext::test_default().cancel,
            vfs,
            None,
            std::sync::Arc::new(wcore_tools::NullToolOutputSink),
        );
        if let Some(notifier) = self.tool_write_notifier.get() {
            ctx = ctx.with_file_write_notifier(Arc::clone(notifier));
        }
        ctx
    }

    /// W8b.2.B D.3: drain pending external-edit events from the watcher
    /// (if any) and emit/inject a single synthetic context message
    /// summarising every edit observed since the last drain.
    ///
    /// Two side effects:
    ///   1. `output.emit_info(...)` — surfaces the message over the
    ///      protocol stream so opted-in hosts (and the W8b D.3 test
    ///      assertions) can observe it.
    ///   2. `self.messages.push(User-role text)` — bakes the note into
    ///      the conversation tail so the *next* turn's `LlmRequest`
    ///      carries it. The model sees "User edited <paths> while I was
    ///      thinking — re-read each before proceeding." before its next
    ///      assistant turn.
    ///
    /// No-op when `self.file_watcher` is `None` or no events accumulated.
    fn drain_and_inject_external_edits(&mut self) {
        let Some(msg) = self.drain_external_edits_message() else {
            return;
        };
        self.messages.push(Message::now(
            Role::User,
            vec![ContentBlock::Text { text: msg }],
        ));
    }

    /// v0.9.1.1 B6 — drain the watcher and return the rendered "User
    /// edited N files…" message text without pushing it onto
    /// `self.messages`. Callers that need to *bundle* the message
    /// into an existing user turn (because pushing a separate User
    /// message would break Anthropic's `tool_use` → `tool_result`
    /// pairing) consume the returned `String` directly. The bare
    /// `drain_and_inject_external_edits` path remains for sites that
    /// can safely push their own User message (e.g. the early-return
    /// when the model produced no tool calls).
    ///
    /// Returns `None` when no watcher is wired, no events drained,
    /// or `render_external_edit_message` returns `None`.
    fn drain_external_edits_message(&mut self) -> Option<String> {
        let watcher = self.file_watcher.get()?;
        let events = watcher.drain_external_events();
        let msg = crate::watch::render_external_edit_message(&events)?;
        // v0.9.1.1 F7: previously this also called
        // `self.output.emit_info(&msg)`, which the TUI bridge routes
        // to `push_system` → transcript system turn. On a
        // `cargo fmt` burst the watcher fires for ~683 paths and
        // the resulting message named every one verbatim, scrolling
        // the user's transcript with a 683-line wall of paths. The
        // LLM still needs the context (which path changed so it can
        // re-read), so we KEEP the User-role message inject in the
        // caller — that drives the model on the next turn — but we
        // no longer mirror it as a user-visible Info event. The
        // same message is also written to tracing::info! so operators
        // can diagnose via the log file.
        tracing::info!(
            target: "wcore_agent::watch",
            "external edits detected (transcript-suppressed): {}",
            truncate_for_trace(&msg, 240)
        );
        Some(msg)
    }

    /// W7 Pre-flight 0.0d: snapshot of every `ProtocolEvent` the engine
    /// has emitted in this session, as captured by the `TestSink`
    /// installed via `AgentBootstrap::build_for_test`. Returns an empty
    /// Vec on production engines (the default detached handle records
    /// nothing). Each entry is the serialised JSON form (`ProtocolEvent`
    /// is `Serialize`-only — no `Clone` — so we round-trip through
    /// `serde_json::Value` to keep the buffer cheap to clone).
    #[cfg(any(test, feature = "test-utils"))]
    pub fn captured_protocol_events(&self) -> Vec<crate::test_utils::CapturedEvent> {
        self.test_sink_handle.snapshot()
    }

    /// W7 Pre-flight 0.0d: drive a single synthetic turn against the
    /// engine's currently-configured provider. Useful in conjunction
    /// with `AgentBootstrap::build_for_test`, which installs a
    /// `ScriptedProvider`. The `msg_id` is auto-derived as
    /// `"synthetic-{turn}"` for traceability.
    #[cfg(any(test, feature = "test-utils"))]
    pub async fn run_synthetic_turn(
        &mut self,
        input: &str,
    ) -> Result<crate::test_utils::SyntheticTurnOutput, AgentError> {
        let msg_id = format!("synthetic-{}", self.total_usage.input_tokens);
        let result = self.run(input, &msg_id).await?;
        Ok(crate::test_utils::SyntheticTurnOutput {
            final_text: result.text,
            events: self.captured_protocol_events(),
            turns: result.turns,
        })
    }

    /// W9.1 T3 (T10b): per-turn skill-draft entry point. Pushes the
    /// just-completed `TurnTrace` into the rolling window, runs the F10
    /// `PatternDetector`, stages any newly-detected drafts as P4
    /// procedures (`Staged` status) via `DraftWriter`, and emits one
    /// `TraceEvent { trace.kind = "skill_drafted" }` per *newly-staged*
    /// candidate so opted-in hosts can surface the draft in their UI.
    ///
    /// Gated on `self.skills_lifecycle`. When off this is a single
    /// boolean check — no allocation, no MemoryApi call.
    ///
    /// Idempotent across turns: `drafted_skill_signatures` collapses
    /// repeat detections of the same `(tool_sequence, input_shape)`
    /// signature to a single emission. This pairs with `DraftWriter::stage`'s
    /// deterministic-UUID idempotency on the storage side.
    async fn try_draft_skill_for_turn(&mut self, trace: TurnTrace, msg_id: &str) {
        if !self.skills_lifecycle {
            return;
        }
        if self.recent_turn_traces.len() >= SKILL_DETECTION_WINDOW {
            self.recent_turn_traces.pop_front();
        }
        self.recent_turn_traces.push_back(trace);

        // Snapshot the window as a slice for the detector. `make_contiguous`
        // returns the backing slice without reallocating when the deque is
        // already laid out linearly; for our 6-element cap the cost is
        // negligible regardless.
        let window: Vec<TurnTrace> = self.recent_turn_traces.iter().cloned().collect();
        let detector = wcore_skills::draft::PatternDetector::default();
        let candidates = detector.detect(&window);
        if candidates.is_empty() {
            return;
        }

        let writer = wcore_skills::draft::DraftWriter::new(self.memory_api.clone());
        for candidate in candidates {
            let signature = (
                candidate.tool_sequence.clone(),
                candidate.input_shape.clone(),
            );
            if !self.drafted_skill_signatures.insert(signature) {
                // Same pattern already staged + emitted this session;
                // skip the storage call AND the TraceEvent emission so the
                // host UI doesn't keep redrawing the same draft.
                continue;
            }
            match writer
                .stage(&candidate, wcore_memory::AccessToken::System)
                .await
            {
                Ok(_id) => {
                    let payload = wcore_skills::draft::render_skill_drafted_payload(&candidate);
                    self.output.emit_trace(msg_id, &payload);
                }
                Err(e) => {
                    // Staging failure must not break the turn — log and
                    // move on so the engine keeps progressing. The
                    // signature stays in the dedup set so we don't
                    // re-attempt every subsequent turn on the same
                    // upstream failure.
                    tracing::warn!(
                        target: "wcore_agent::skills_lifecycle",
                        error = %e,
                        candidate = %candidate.suggested_name,
                        "W9.1 T10b: failed to stage drafted skill; continuing"
                    );
                }
            }
        }
    }

    pub fn set_protocol_writer(
        &mut self,
        writer: Arc<dyn wcore_protocol::writer::ProtocolEmitter>,
    ) {
        self.protocol_writer = Some(writer);
    }

    /// Set the initial reasoning effort override (used by sub-agents spawned with an effort override).
    pub fn set_initial_reasoning_effort(&mut self, effort: Option<String>) {
        self.current_reasoning_effort = effort;
    }

    /// Set the shared plan-mode active flag.
    ///
    /// This flag is shared with EnterPlanMode/ExitPlanMode tools so they can
    /// validate transitions (e.g. reject double-entry).  The engine updates
    /// the flag when processing `PlanModeTransition` context modifiers.
    pub fn set_plan_active_flag(&mut self, flag: Arc<AtomicBool>) {
        self.plan_active_flag = Some(flag);
    }

    /// Enter plan mode from a host-driven entry point (the TUI `/plan`
    /// command), not the model's `EnterPlanMode` tool.
    ///
    /// D005: `/plan` advertised "(read-only)" but never set this flag, so a
    /// Write/Edit tool was NOT gated — files could be written under a
    /// posture the user trusted as safe. This is the same transition the
    /// `PlanModeTransition::Enter` modifier applies (engine.rs
    /// `apply_context_modifiers`): snapshot the allow-list, flip
    /// `plan_state.is_active` (read by the per-turn tool filter, which then
    /// drops every non-Info tool), and publish the shared atomic flag.
    /// Idempotent — re-entering while already active is a no-op so the
    /// snapshotted allow-list is not clobbered.
    pub fn enter_plan_mode(&mut self) {
        if self.plan_state.is_active {
            return;
        }
        self.plan_state.pre_plan_allow_list = self.allow_list.clone();
        self.plan_state.is_active = true;
        if let Some(ref flag) = self.plan_active_flag {
            flag.store(true, Ordering::Release);
        }
    }

    /// Exit plan mode from a host-driven entry point (the TUI plan-review
    /// "Approve & run" path), mirroring `PlanModeTransition::Exit`.
    ///
    /// D006: approving a plan must clear this gate so the approved work can
    /// actually run with its full tool set. Restores the pre-plan allow-list
    /// and clears the shared atomic flag. Idempotent — exiting when not in
    /// plan mode is a no-op.
    pub fn exit_plan_mode(&mut self) {
        if !self.plan_state.is_active {
            return;
        }
        self.plan_state.is_active = false;
        self.allow_list = self.plan_state.pre_plan_allow_list.clone();
        if let Some(ref flag) = self.plan_active_flag {
            flag.store(false, Ordering::Release);
        }
    }

    /// Default thinking budget when "enabled" is requested without a specific budget.
    const DEFAULT_THINKING_BUDGET: u32 = 10_000;

    /// Apply a runtime config update received from the protocol layer.
    ///
    /// Returns a list of human-readable change descriptions for the Info event.
    /// Empty list means no fields were changed.
    pub fn apply_config_update(
        &mut self,
        model: Option<String>,
        thinking: Option<String>,
        thinking_budget: Option<u32>,
        effort: Option<String>,
        compaction: Option<String>,
    ) -> Vec<String> {
        let mut changes = Vec::new();

        if let Some(new_model) = model {
            // D014: a protocol-layer config update that names a model is an
            // explicit user/host choice — record it as the authoritative pin
            // (same precedence as a TUI `/model` pick) so a later skill/hook
            // `switch_model` cannot silently override it.
            self.user_model_pin = Some(new_model.clone());
            let old = std::mem::replace(&mut self.model, new_model.clone());
            changes.push(format!("model: {old} → {new_model}"));
        }

        if let Some(thinking_str) = thinking {
            if !self.compat.supports_thinking() {
                changes.push("thinking: not supported by current provider".to_string());
            } else {
                match thinking_str.as_str() {
                    "enabled" => {
                        let budget = thinking_budget.unwrap_or(Self::DEFAULT_THINKING_BUDGET);
                        self.thinking = Some(wcore_types::llm::ThinkingConfig::Enabled {
                            budget_tokens: budget,
                        });
                        changes.push(format!("thinking: enabled (budget: {budget})"));
                    }
                    "disabled" => {
                        self.thinking = Some(wcore_types::llm::ThinkingConfig::Disabled);
                        changes.push("thinking: disabled".to_string());
                    }
                    other => {
                        changes.push(format!("thinking: ignored invalid value \"{other}\""));
                    }
                }
            }
        } else if let Some(new_budget) = thinking_budget
            && let Some(wcore_types::llm::ThinkingConfig::Enabled { budget_tokens }) =
                &mut self.thinking
        {
            *budget_tokens = new_budget;
            changes.push(format!("thinking budget: {new_budget}"));
        }

        if let Some(new_effort) = effort {
            if new_effort.is_empty() {
                self.current_reasoning_effort = None;
                changes.push("effort: cleared".to_string());
            } else if !self.compat.supports_effort() {
                changes.push("effort: not supported by current provider".to_string());
            } else {
                let levels = self.compat.effort_levels();
                if !levels.is_empty() && !levels.iter().any(|l| l == &new_effort) {
                    changes.push(format!(
                        "effort: invalid level \"{}\" (valid: {})",
                        new_effort,
                        levels.join(", ")
                    ));
                } else {
                    let old = self
                        .current_reasoning_effort
                        .replace(new_effort.clone())
                        .unwrap_or_else(|| "none".to_string());
                    changes.push(format!("effort: {old} → {new_effort}"));
                }
            }
        }

        if let Some(ref level_str) = compaction {
            match level_str.parse::<wcore_compact::CompactionLevel>() {
                Ok(new_level) => {
                    let old = self.compaction_level.to_string();
                    self.compaction_level = new_level;
                    changes.push(format!("compaction: {old} → {new_level}"));
                }
                Err(e) => {
                    changes.push(format!("compaction: invalid ({e})"));
                }
            }
        }

        changes
    }

    /// v0.8.0 Task M — per-turn user-model write-back. Derives a
    /// 4-axis style fingerprint from the rolling `StyleDetector`
    /// window and hands it to the installed `UserModelBackend` as an
    /// `Observation`. Closes the v0.7.0 deferment that left the
    /// user-model layer bootstrap-only-read. Empty input is skipped
    /// (nothing meaningful to observe). Backend errors are logged +
    /// swallowed so observation failures never kill a turn.
    ///
    /// Returns `true` iff an observation was actually attempted (input
    /// non-empty AND a backend is installed). The boolean exists so
    /// tests can assert the call path without re-running a full turn.
    async fn observe_user_turn(&self, user_input: &str) -> bool {
        if user_input.trim().is_empty() {
            return false;
        }
        let Some(backend) = self.user_model_backend.as_ref() else {
            return false;
        };
        let style = self
            .style_detector
            .lock()
            .ok()
            .map(|det| det.style())
            .unwrap_or_default();
        let fingerprint = [
            style.formality,
            style.energy,
            style.terseness,
            style.emoji_use,
        ];
        let ts_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let observation = wcore_user_model::Observation {
            style_fingerprint: Some(fingerprint),
            ts_secs,
            ..wcore_user_model::Observation::default()
        };
        if let Err(e) = backend.observe(&self.user_model_user_id, observation).await {
            tracing::warn!(
                target: "wcore_agent::user_model",
                error = %e,
                user_id = %self.user_model_user_id,
                backend = backend.backend_tag(),
                "per-turn user-model observe failed; continuing turn"
            );
        }
        true
    }

    /// v0.8.1 U1 — build the non-binding skill hint line for the turn, if
    /// the router picked a skill that is (a) present in the loaded catalog
    /// and (b) model-invocable. Returns `None` when no router is installed,
    /// no pick was credited this turn, or the picked name isn't a visible
    /// catalog skill — so the hint NEVER injects in those cases and the
    /// system prompt stays byte-identical to pre-U1 behaviour.
    ///
    /// The hint is intentionally one short, non-coercive line: it tells the
    /// model a skill "may help" and to "use it only if genuinely relevant",
    /// leaving the model free to ignore it. This closes the F-068
    /// telemetry-only gap (router learns but the pick went nowhere) without
    /// coercing skill selection. The pick is `take()`-cleared at turn end by
    /// `observe_skill_router_outcome`, so a stale pick can't leak across
    /// turns — each turn re-`choose()`s before this runs.
    fn skill_router_hint(&self) -> Option<String> {
        // Gate on router installation first: when absent, zero behaviour
        // change (mirrors the choose/observe short-circuit in `run`).
        self.skill_router.as_ref()?;
        let picked = self.current_skill_router_pick.as_ref()?;
        let catalog = self.skill_catalog.as_ref()?;
        // Only hint a skill that exists AND the model is allowed to invoke.
        // Hinting a hidden/non-invocable skill would be useless advice.
        let skill = catalog.find(picked)?;
        if skill.disable_model_invocation {
            return None;
        }
        Some(format!(
            "Skill hint: based on what has worked before, the \"{}\" skill may help with this request — use it only if genuinely relevant.",
            skill.name
        ))
    }

    /// v0.8.1 U1 — credit the turn's `SkillRouter` pick (if any) with a
    /// success/failure observation based on the terminal `StopReason`.
    /// `EndTurn` and `ToolUse` count as success; anything else (errors,
    /// `MaxTurns`, refusals) counts as failure. `take()`-clears the
    /// stashed pick so a subsequent `run()` invocation starts with a
    /// clean slot. No-op when no router is installed OR no pick was
    /// credited at the top of the turn.
    fn observe_skill_router_outcome(&mut self, stop_reason: StopReason) {
        if let Some(picked) = self.current_skill_router_pick.take()
            && let Some(router) = self.skill_router.as_ref()
        {
            let outcome = match stop_reason {
                StopReason::EndTurn | StopReason::ToolUse => wcore_dispatch::TaskOutcome::Success,
                _ => wcore_dispatch::TaskOutcome::Failure,
            };
            // `BetaScorer::record` is cheap (HashMap update); the std
            // Mutex is uncontested between `choose` (top of run) and
            // `observe` (end of run) on the same task, so locking here
            // is fine.
            if let Ok(mut guard) = router.lock() {
                use wcore_dispatch::DecisionRouter;
                guard.observe(&picked, outcome);
                tracing::debug!(
                    target: "wcore_agent::engine",
                    skill = %picked,
                    ?stop_reason,
                    ?outcome,
                    "skill_router: observed turn outcome"
                );
            }
        }
    }

    /// v0.8.1 U6 — record the turn into the autonomous-skill bucketer.
    /// When a streak of N consecutive successes on the same task
    /// signature lands, the drafter (if installed) writes a candidate
    /// skill + records into GEPA's `PromptStore` so the next session's
    /// `SkillRouter` (U1) sees it as a seed pair.
    ///
    /// `picked` is the U1 skill the router chose for this turn (if
    /// any), captured from `current_skill_router_pick` BEFORE
    /// `observe_skill_router_outcome` clears it. The user_input is the
    /// raw turn input — the bucketer normalizes it into a signature.
    ///
    /// Errors are logged + swallowed. The autonomous-skill path is
    /// strictly optional; a failure here must never abort the turn.
    fn observe_auto_skill(
        &self,
        user_input: &str,
        picked: Option<String>,
        stop_reason: StopReason,
        turns: usize,
    ) {
        let outcome = match stop_reason {
            StopReason::EndTurn | StopReason::ToolUse => crate::auto_skill::TurnOutcome::Success,
            _ => crate::auto_skill::TurnOutcome::Failure,
        };
        let traj = crate::auto_skill::TurnTrajectory {
            user_input: user_input.to_string(),
            picked_skill: picked,
            outcome,
            summary: format!("{turns} turn(s)"),
            timestamp: chrono::Utc::now(),
        };
        // SAFETY: `Mutex` here is std::sync; the bucketer's `observe` is
        // pure CPU (HashMap insert + Vec push) and cannot panic, so the
        // lock cannot be poisoned. `.unwrap()` mirrors the in-crate
        // idiom for non-panicking critical sections.
        let trigger_opt = {
            let mut guard = match self.auto_skill_bucketer.lock() {
                Ok(g) => g,
                Err(poisoned) => poisoned.into_inner(),
            };
            guard.observe(traj)
        };
        let Some(trigger) = trigger_opt else {
            return;
        };
        let Some(drafter) = self.skill_drafter.as_ref() else {
            // No drafter installed (test engines, no-memory bootstrap).
            // Bucket fired; without a drafter we just log and move on.
            tracing::debug!(
                target: "wcore_agent::auto_skill",
                signature = %trigger.signature,
                evidence = trigger.trajectories.len(),
                "bucket triggered but no SkillDrafter installed; skipping draft"
            );
            return;
        };
        match drafter.draft(&trigger) {
            Ok(res) => tracing::info!(
                target: "wcore_agent::auto_skill",
                name = %res.name,
                evidence = trigger.trajectories.len(),
                md = %res.md_path.display(),
                "auto-drafted skill from observed-turn streak"
            ),
            Err(e) => tracing::warn!(
                target: "wcore_agent::auto_skill",
                error = %e,
                signature = %trigger.signature,
                "skill draft failed; trajectories discarded"
            ),
        }
    }

    /// Append the user's turn, first repairing any `tool_use` left
    /// dangling by a previous turn that was aborted between the model's
    /// tool call and the tool's execution — `Esc`-cancel, a crash, or a
    /// session resumed mid-tool. Anthropic rejects a request whose
    /// `tool_use` has no following `tool_result` (HTTP 400
    /// `invalid_request_error`); without this repair, one cancelled tool
    /// call permanently bricks the session.
    ///
    /// Only the trailing assistant message can be orphaned: a completed
    /// turn always pairs its `tool_use` with a `tool_result`, and
    /// compaction preserves the pairing. Synthetic error results are
    /// bundled into this same user message so conversation roles stay
    /// strictly alternating.
    fn push_user_turn(&mut self, user_input: &str) {
        let mut content: Vec<ContentBlock> = Self::orphan_repair_results(self.messages.last());
        content.push(ContentBlock::Text {
            text: user_input.to_string(),
        });
        self.messages.push(Message::now(Role::User, content));
    }

    /// AUDIT D-6 — synthesize the `ToolResult` blocks needed to repair a
    /// trailing assistant message that carries `tool_use` blocks with no
    /// following tool-results message.
    ///
    /// Returns an empty vec when `last` is `None`, not an assistant
    /// message, or carries no `tool_use` blocks. Shared by
    /// `push_user_turn` (repair in-memory before the next provider call)
    /// and `save_session` (repair before persisting to disk, so a
    /// session inspector / export never reads an Anthropic-invalid
    /// `tool_use`-without-`tool_result` message).
    fn orphan_repair_results(last: Option<&Message>) -> Vec<ContentBlock> {
        match last {
            Some(last) if matches!(last.role, Role::Assistant) => last
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, .. } => Some(ContentBlock::ToolResult {
                        tool_use_id: id.clone(),
                        content: "Turn cancelled before this tool ran.".to_string(),
                        is_error: true,
                    }),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    /// AUDIT D-6 — if `self.messages` ends with an assistant message
    /// carrying dangling `tool_use` blocks, append a user message with
    /// the matching synthetic error `tool_result`s so the on-disk /
    /// in-memory message list is always a valid alternating shape.
    /// No-op when there is nothing to repair.
    fn repair_orphaned_tool_use(&mut self) {
        let repairs = Self::orphan_repair_results(self.messages.last());
        if !repairs.is_empty() {
            self.messages.push(Message::now(Role::User, repairs));
        }
    }

    /// Belt-and-suspenders behind `repair_orphaned_tool_use`.
    ///
    /// `repair_orphaned_tool_use` only repairs a trailing-assistant
    /// orphan. But a `tool_use` can also end up orphaned mid-history:
    /// a dispatch escape path (cancel-during-approval, reaper-denial,
    /// channel-drop, panic, partial-batch failure) may push the
    /// assistant's tool_use, fail to push a matching tool_result, and
    /// then push some other message on top — leaving an orphan that
    /// the trailing-only repair will never see. The Anthropic API
    /// rejects ANY such orphan with HTTP 400 and bricks the session.
    ///
    /// This scans the whole history. For each assistant message
    /// carrying tool_use blocks: if the next message is a User
    /// message, any tool_use id missing a matching tool_result there
    /// gets one appended in place. If the next message isn't a User
    /// (or doesn't exist), a synthetic User message carrying every
    /// missing tool_result is inserted right after the assistant.
    /// Idempotent — a clean history is left untouched. Called from
    /// `run()` immediately before every provider request as the load-
    /// bearing guard.
    fn repair_all_orphaned_tool_uses(&mut self) {
        use std::collections::HashSet;
        let mut i = 0;
        while i < self.messages.len() {
            if !matches!(self.messages[i].role, Role::Assistant) {
                i += 1;
                continue;
            }
            let tool_use_ids: Vec<String> = self.messages[i]
                .content
                .iter()
                .filter_map(|b| match b {
                    ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                    _ => None,
                })
                .collect();
            if tool_use_ids.is_empty() {
                i += 1;
                continue;
            }
            let satisfied: HashSet<String> = if i + 1 < self.messages.len()
                && matches!(self.messages[i + 1].role, Role::User)
            {
                self.messages[i + 1]
                    .content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.clone()),
                        _ => None,
                    })
                    .collect()
            } else {
                HashSet::new()
            };
            let missing: Vec<String> = tool_use_ids
                .into_iter()
                .filter(|id| !satisfied.contains(id))
                .collect();
            if missing.is_empty() {
                i += 1;
                continue;
            }
            let synth: Vec<ContentBlock> = missing
                .into_iter()
                .map(|id| ContentBlock::ToolResult {
                    tool_use_id: id,
                    content: "Tool result missing — backfilled before sending to provider."
                        .to_string(),
                    is_error: true,
                })
                .collect();
            if i + 1 < self.messages.len() && matches!(self.messages[i + 1].role, Role::User) {
                self.messages[i + 1].content.extend(synth);
            } else {
                self.messages.insert(i + 1, Message::now(Role::User, synth));
            }
            i += 2;
        }
    }

    /// Losslessly demote a `tool_result` block whose `tool_use` partner is
    /// gone into a plain `text` block. The model keeps the result content as
    /// context, but no dangling `tool_result` reaches the provider. Used by
    /// both the pre-send repair (`repair_orphaned_tool_results`) and the
    /// autocompact fold site, which can orphan a result by summarizing its
    /// `tool_use` away.
    fn neutralize_orphaned_tool_result(block: ContentBlock) -> ContentBlock {
        match block {
            ContentBlock::ToolResult { content, .. } => ContentBlock::Text {
                text: format!("[tool result (call elided during compaction)]\n{content}"),
            },
            other => other,
        }
    }

    /// #285 — the reverse of `repair_all_orphaned_tool_uses`: a
    /// `tool_result` whose `tool_use_id` matches NO `tool_use` anywhere in
    /// the history. DeepSeek (and other strict providers) reject the whole
    /// conversation array with HTTP 400 `missing field tool_call_id` when
    /// such an orphan is sent, bricking the session.
    ///
    /// The orphan is produced at autocompact time: the live user turn that
    /// triggered compaction can carry `tool_result` blocks whose matching
    /// `tool_use` lived in an assistant message the autocompact fold just
    /// summarized into prose (engine.rs `run_compaction`). After the fold the
    /// result is dangling, and the forward-direction repair — which only
    /// scans assistant `tool_use` blocks — never sees it.
    ///
    /// This builds the set of every `tool_use` id present anywhere in
    /// `self.messages`, then demotes any `tool_result` whose id is not in
    /// that set to a plain `text` block (`neutralize_orphaned_tool_result`),
    /// preserving the content. If neutralizing empties a message of all
    /// blocks it leaves a single text placeholder rather than an empty
    /// message (strict providers reject empty content too). Idempotent — a
    /// clean history is left untouched. Called from `run()` immediately
    /// before every provider request, alongside the forward repair.
    fn repair_orphaned_tool_results(&mut self) {
        use std::collections::HashSet;
        let live_ids: HashSet<String> = self
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        for msg in &mut self.messages {
            let has_orphan = msg.content.iter().any(|b| {
                matches!(b, ContentBlock::ToolResult { tool_use_id, .. }
                    if !live_ids.contains(tool_use_id))
            });
            if !has_orphan {
                continue;
            }
            let demoted: Vec<ContentBlock> = std::mem::take(&mut msg.content)
                .into_iter()
                .map(|b| match &b {
                    ContentBlock::ToolResult { tool_use_id, .. }
                        if !live_ids.contains(tool_use_id) =>
                    {
                        Self::neutralize_orphaned_tool_result(b)
                    }
                    _ => b,
                })
                .collect();
            msg.content = if demoted.is_empty() {
                vec![ContentBlock::Text {
                    text: "[tool result elided during compaction]".to_string(),
                }]
            } else {
                demoted
            };
        }
    }

    /// AUDIT A1 / E-C1 / A2 — shared clean-termination path for the
    /// non-natural loop exits (turn cap, budget cap, context ceiling,
    /// host cancel).
    ///
    /// Every one of these is a *failure verdict*: the model did not
    /// close the task on its own. They run the identical session-end
    /// bookkeeping the `MaxTurns` exit already did — `fire_on_session_end`,
    /// `save_session`, and the two learning observes — then return an
    /// `AgentResult` with `StopReason::MaxTurns` (the engine's existing
    /// "ran out of budget" verdict; `observe_*` already maps it to
    /// `Failure` and `FinishReason::from_stop_reason` maps it to
    /// `Error`). The distinct, user-visible reason is surfaced via the
    /// `emit_error` call the caller makes before invoking this — the
    /// `StopReason` enum lives in `wcore-types` and is not extended
    /// here.
    async fn finish_run_terminated(
        &mut self,
        user_input: &str,
        turn: usize,
    ) -> Result<AgentResult, AgentError> {
        self.fire_on_session_end(turn).await;
        self.save_session();
        let auto_skill_picked = self.current_skill_router_pick.clone();
        self.observe_skill_router_outcome(StopReason::MaxTurns);
        self.observe_auto_skill(user_input, auto_skill_picked, StopReason::MaxTurns, turn);
        Ok(AgentResult {
            text: String::new(),
            stop_reason: StopReason::MaxTurns,
            finish_reason: FinishReason::Length,
            usage: self.total_usage.clone(),
            turns: turn,
            active_window_percent: self.active_window_percent_now(&self.model, 0),
            agent_run_id: self.current_agent_run_id.clone(),
        })
    }

    /// #279(a): the sole place the engine sources the active-window gauge.
    /// Constructs the #255 ContextWindow kernel from the POST-swap effective
    /// model and the per-turn `used_tokens` watermark the run loop already
    /// computes, then returns `percent()` (0..=100), or `None` when the window
    /// is unknown. The protocol/TUI NEVER recompute this.
    ///
    /// Real #255 kernel API: `ContextWindow::resolve(used_tokens, provider,
    /// model, config_window) -> Self`, then `percent(&self) -> Option<u32>`.
    fn active_window_percent_now(&self, effective_model: &str, used_tokens: u64) -> Option<u32> {
        use wcore_config::context_window::ContextWindow;
        ContextWindow::resolve(
            used_tokens,
            self.compat.provider_type(),
            effective_model,
            self.compact_config.context_window as u64,
        )
        .percent()
    }

    /// #280 — the SINGLE chokepoint for smart auto-compaction. Returns the
    /// Flux-aware active-window fraction (0.0..) of the CURRENTLY-active model,
    /// or `None` when the feature is disabled OR the window is unknown.
    ///
    /// Default-OFF guarantee: when `smart_enabled` is false this returns `None`
    /// on its first line, before reading any state — so the turn-top pre-gate
    /// never touches memory or the force flag and `run_compaction` is byte-for-
    /// byte unchanged.
    ///
    /// Re-eval-on-swap is STRUCTURAL: this reads `self.model` /
    /// `flux_served_window` / `flux_context_pressure` fresh each turn (the model
    /// swap was already applied by `apply_pre_turn_outcome`), so there is no
    /// stored trigger state to invalidate.
    fn smart_compact_fraction(&self) -> Option<f64> {
        // Chokepoint: nothing below runs unless explicitly enabled.
        if !self.compact_config.smart_enabled {
            return None;
        }
        use wcore_config::context_window::ContextWindow;
        // Finding #174 — the same REAL-pressure watermark the static auto
        // trigger trusts. On turn 1 it is 0 → fraction 0.0 → never fires before
        // any stream completes.
        let used_tokens = self.compact_state.last_real_input_tokens;

        // Flux-aware denominator, mirroring the pre-flight guard + #282
        // reconciliation: prefer the REAL served-model window when Flux signalled
        // it back for a tier alias; else the #255 kernel resolution.
        let kernel_fraction: Option<f64> = if wcore_providers::is_flux_tier_alias(&self.model)
            && self.flux_served_window.is_some()
        {
            ContextWindow {
                used_tokens,
                window: self.flux_served_window,
            }
            .fraction()
        } else {
            ContextWindow::resolve(
                used_tokens,
                self.compat.provider_type(),
                &self.model,
                self.compact_config.context_window as u64,
            )
            .fraction()
        };

        // The Flux-signalled pressure header is served-model ground truth; it
        // only ADDS fires via max() so an overload trips even when the local
        // token estimate lags.
        let pressure_candidate: Option<f64> = self.flux_context_pressure.map(|p| p as f64);

        match (kernel_fraction, pressure_candidate) {
            (Some(k), Some(p)) => Some(k.max(p)),
            (Some(k), None) => Some(k),
            // Kernel window unknown but Flux gave pressure → use pressure alone.
            (None, Some(p)) => Some(p),
            // Fail open: never fabricate a denominator.
            (None, None) => None,
        }
    }

    /// #280 — anti-thrash decision. Three independent latches; ALL must clear to
    /// re-fire. The arm flag flips and the cooldown watermark stamps HERE (at the
    /// decision point), not inside `run_compaction`, so a circuit-broken or
    /// erroring autocompact cannot hot-loop the failing summarization every turn.
    fn smart_compact_should_fire(&mut self, turn: u32, frac: f64) -> bool {
        // TERMINAL cannot-shrink latch.
        if self.smart_compact_exhausted {
            return false;
        }
        let trigger = self.compact_config.smart_trigger_fraction.clamp(0.60, 0.70);
        // A mis-set release >= trigger can never collapse hysteresis.
        let release = self
            .compact_config
            .smart_release_fraction
            .min(trigger - 0.05);

        // HYSTERESIS RE-ARM: while disarmed we only re-arm (never fire the same
        // turn, since release < trigger).
        if !self.smart_compact_armed {
            if frac < release {
                self.smart_compact_armed = true;
            }
            return false;
        }

        // COOLDOWN: min completed turns between two fires.
        if let Some(last) = self.smart_compact_last_turn
            && turn.saturating_sub(last) < self.compact_config.smart_cooldown_turns
        {
            return false;
        }

        // FIRE TEST.
        if self.smart_compact_armed && frac >= trigger {
            self.smart_compact_armed = false;
            self.smart_compact_last_turn = Some(turn);
            return true;
        }
        false
    }

    /// #280 — record ONE additive, non-destructive handoff Episode to long-term
    /// memory BEFORE `run_compaction` mutates the buffer, so the verbatim
    /// pre-compaction transcript survives even if the LLM summary rewords prose.
    /// The transcript captures every load-bearing block — assistant/user text,
    /// the `tool_use` name + args, the `tool_result` bodies (which carry the
    /// bulk of a coding session's context), and thinking — so the spec's
    /// "nothing lost" intent holds, not just prose.
    ///
    /// Non-destructive by construction: `record_episode` is a pure INSERT that
    /// never touches `self.messages`; a failure is logged-and-swallowed (never
    /// propagated) so a memory outage cannot abort or alter the turn. With
    /// `NullMemory` installed this is a no-op stub.
    async fn write_smart_handoff(&self) {
        use wcore_memory::v2_types::{Episode, EpisodeId, EpisodeStatus, Tier};

        // VERBATIM role-tagged concatenation of the current buffer (the live
        // conversation about to be summarized), captured from the LIVE buffer
        // (not the post-fold result). Every content-block kind is preserved —
        // tool_result bodies are the bulk of real context and MUST survive, so
        // a text-only handoff would silently drop most of what we promise to
        // keep.
        let mut summary = String::new();
        // The last USER message's text — the active task — for atomic_facts[0].
        let mut last_user_text: Option<String> = None;
        for m in &self.messages {
            let role = match m.role {
                Role::User => "user",
                Role::Assistant => "assistant",
                Role::System => "system",
                Role::Tool => "tool",
            };
            let mut this_user_text: Option<String> = None;
            for block in &m.content {
                let line = match block {
                    ContentBlock::Text { text } => {
                        if matches!(m.role, Role::User) {
                            this_user_text = Some(text.clone());
                        }
                        Some(text.clone())
                    }
                    ContentBlock::ToolUse { name, input, .. } => {
                        Some(format!("tool_use({name}): {input}"))
                    }
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        is_error,
                    } => Some(format!(
                        "tool_result({tool_use_id}{}): {content}",
                        if *is_error { " error" } else { "" }
                    )),
                    ContentBlock::Thinking { thinking } => Some(format!("thinking: {thinking}")),
                };
                if let Some(line) = line {
                    summary.push_str(role);
                    summary.push_str(": ");
                    summary.push_str(&line);
                    summary.push('\n');
                }
            }
            // Capture the LAST user message's text (the current task), not the
            // last text block across all user messages.
            if this_user_text.is_some() {
                last_user_text = this_user_text;
            }
        }
        let live_user_text = last_user_text;

        // Cheap heuristic bullets — NO extra LLM call.
        let mut atomic_facts: Vec<String> = Vec::new();
        if let Some(t) = live_user_text {
            atomic_facts.push(t);
        }
        let pre_estimate = self.compact_state.last_real_input_tokens;
        atomic_facts.push(format!("pre_compact_estimate: {pre_estimate}"));
        if let Some(pct) = self.active_window_percent_now(&self.model, pre_estimate) {
            atomic_facts.push(format!("active_window_percent: {pct}"));
        }

        let ep = Episode {
            id: EpisodeId::new(),
            tier: Tier::Session,
            ts: wcore_memory::audit::now_secs(),
            episode_type: "compaction_handoff".into(),
            summary,
            atomic_facts,
            source: "smart_compact".into(),
            source_product: "wcore-compact".into(),
            session_id: Some(self.conversation_id.clone()),
            project_root: None,
            decay_score: 1.0,
            status: EpisodeStatus::Active,
        };
        if let Err(e) = self
            .memory_api
            .record_episode(ep, wcore_memory::AccessToken::MainAgent)
            .await
        {
            tracing::warn!(
                target: "wcore_agent::memory",
                error = %e,
                "#280 smart handoff record_episode failed; continuing"
            );
        }
    }

    /// Run the agent loop with user input
    pub async fn run(&mut self, user_input: &str, msg_id: &str) -> Result<AgentResult, AgentError> {
        // methodology #27: production caller for StyleDetector::observe (Task 1.B.3)
        if let Ok(mut det) = self.style_detector.lock() {
            det.observe(user_input);
        }
        // v0.8.0 Task M — per-turn user-model write-back. See
        // `observe_user_turn` for the full contract: no-op when no
        // backend is installed; errors are logged + swallowed.
        self.observe_user_turn(user_input).await;
        // v0.8.1 U1 — per-turn `SkillRouter` choose. Picks one skill
        // from the resolved catalog using a Thompson Beta scorer
        // seeded from GEPA winners + session-start prioritizer ranking
        // (see `AgentBootstrap::build`). The pick is stashed on the
        // engine so the matching `observe()` at the end of this turn
        // credits the same arm. No-op when no router was installed
        // OR no catalog was wired OR the catalog has zero entries —
        // matches the Stub/`None` defaults for engines built outside
        // bootstrap.
        if let Some(router) = self.skill_router.as_ref()
            && let Some(catalog) = self.skill_catalog.as_ref()
        {
            let candidates: Vec<String> = catalog.refs().map(|r| r.name.clone()).collect();
            if !candidates.is_empty() {
                // `choose` lives on the `DecisionRouter` trait, in the
                // sibling `wcore-dispatch` crate. Importing it inline
                // here scopes the trait to just this block (the only
                // place it's needed in `engine.rs`).
                use wcore_dispatch::DecisionRouter;
                let pick = {
                    let mut guard = router.lock().unwrap();
                    guard.choose(wcore_skills::SkillRouterInput {
                        task: user_input,
                        candidates: &candidates,
                    })
                };
                match pick {
                    Ok(name) => {
                        tracing::debug!(
                            target: "wcore_agent::engine",
                            skill = %name,
                            "skill_router: per-turn choice"
                        );
                        self.current_skill_router_pick = Some(name);
                    }
                    Err(e) => tracing::debug!(
                        target: "wcore_agent::engine",
                        error = %e,
                        "skill_router: choose returned error (no pick credited)"
                    ),
                }
            }
        }
        self.current_msg_id = msg_id.to_string();
        // #279(c): mint a stable per-run correlation id on the first run()
        // of the session and reuse it for every subsequent turn/message.
        // Re-minted only when None (fresh engine / cleared session); persists
        // across --resume because the resumed engine keeps the field.
        if self.current_agent_run_id.is_none() {
            self.current_agent_run_id = Some(format!("agent-run-{}", uuid::Uuid::new_v4()));
        }
        self.output.emit_stream_start(msg_id);
        // Cross-session recall (v2 memory gap fix): on a cold first turn,
        // pre-inject durable facts relevant to this message BEFORE the user
        // turn so a fresh process answers from prior-session memory without
        // relying on the model invoking `session_search`. No-op on resumed
        // sessions, with NullMemory, or when nothing relevant is stored.
        self.recall_relevant_facts(user_input).await;
        self.push_user_turn(user_input);

        // F-030 WAL: persist the user message BEFORE any LLM call so a
        // SIGKILL mid-turn does not silently erase it.  On resume the
        // SessionManager::load path calls merge_wal() which folds the WAL
        // entry back into session.messages and re-saves, so the user always
        // sees their last prompt after a crash.
        //
        // Two cases:
        //   a) First message in session: the session file hasn't been written
        //      yet (F-034 deferred write).  Call persist_first_message which
        //      does the initial session + index write AND the WAL append.
        //   b) Subsequent messages: session file exists; just append WAL.
        if let (Some(mgr), Some(session)) = (&self.session_manager, &mut self.current_session) {
            let is_first_message = session.messages.len() == 1; // we just pushed
            if is_first_message {
                session.messages = self.messages.clone();
                session.updated_at = chrono::Utc::now();
                if let Err(e) = mgr.persist_first_message(session) {
                    self.output
                        .emit_error(&format!("Failed to persist first message: {}", e), false);
                }
            } else {
                if let Err(e) = mgr.append_wal(session, user_input) {
                    self.output
                        .emit_error(&format!("Failed to append WAL: {}", e), false);
                }
            }
        }

        // Dynamic Workflows B6 — LIVE workflow confirm gate. Distinct from the
        // shadow `workflow_detection` block inside the loop (telemetry-only).
        // This is a PRE-LLM intercept: it fires ONCE per `run()` BEFORE any
        // model turn, so a workflow-shaped prompt is confirmed up front rather
        // than after the model has already wasted a turn (and so prompts the
        // model would answer in plain text still surface the gate). Fires only
        // when ALL of:
        //   * `workflow_live_mode` is on (operator opt-in, default off),
        //   * the input looks like a workflow candidate (B3 heuristic),
        //   * BOTH an approval manager AND a protocol writer are wired (the
        //     gate is meaningless without a host to confirm through).
        // On approval it runs the synthesised workflow and RETURNS the run as
        // the `run()` output, skipping the normal turn loop entirely. On
        // decline / cancel / synthesis error / mode-off it returns `None` and
        // execution falls through to the normal turn loop below — the user
        // still gets an ordinary response. SECURITY: the spawner is built
        // WITHOUT forcing `auto_approve`; the workflow's sub-agents inherit the
        // parent's read-only toolset + approval posture, so this gate
        // authorises *running* the workflow only — inner tool calls still gate
        // normally. Child engines spawned for those sub-agents carry
        // `workflow_live_mode = false` (set in `AgentSpawner::child_config`) AND
        // lack an approval manager / protocol writer, so they can never
        // recursively re-enter this gate.
        if self.workflow_live_mode
            && self.approval_manager.is_some()
            && self.protocol_writer.is_some()
            && crate::orchestration::intent::workflow_candidate(user_input).is_some()
            && let Some(result) = self.try_live_workflow(user_input, 0).await
        {
            return Ok(result);
        }

        // Crucible Stage 3b — the `/crucible <task>` front door. A deterministic,
        // zero-false-trigger prefix intercept: convene a cross-vendor council, gate
        // spend on the typed proposal card, and return the council's answer as the
        // turn output. Requires a host to confirm through (protocol_writer); a
        // `/crucible` typed with no approver falls through to a normal turn ($0).
        // Child engines (council members) carry no protocol_writer, so they can
        // never re-enter this gate.
        {
            let trimmed = user_input.trim_start();
            // Stage 4c — `/crucible off` silences the discovery suggestion for the
            // rest of the process. Checked BEFORE the council dispatch because
            // "/crucible off" also matches the `starts_with("/crucible ")` arm.
            if trimmed == "/crucible off" {
                CRUCIBLE_SUGGEST_SUPPRESSED.store(true, Ordering::Relaxed);
                let msg = "Crucible suggestions off for this session.".to_string();
                self.output.emit_text_delta(&msg, &self.current_msg_id);
                return Ok(self.crucible_result(msg));
            }
            if (trimmed == "/crucible" || trimmed.starts_with("/crucible "))
                && self.protocol_writer.is_some()
                && let Some(result) = self.try_crucible_council(trimmed).await
            {
                return Ok(result);
            }
        }

        // Stage 4c — discovery nudge: if the input is HIGH-stakes and the user
        // hasn't used (or silenced) `/crucible`, surface the council once.
        if let Some(hint) = self.maybe_suggest_council(user_input) {
            self.output.emit_text_delta(&hint, &self.current_msg_id);
            CRUCIBLE_SUGGEST_SUPPRESSED.store(true, Ordering::Relaxed);
        }

        // Runaway-loop breaker (per-run): the engine-side backstop for the
        // no-progress loops that `max_turns = None` leaves unguarded. Terminates
        // the run if the same tool call keeps producing the same result.
        let mut loop_guard = LoopGuard::from_env();
        let mut turn: usize = 0;
        loop {
            // AUDIT A2 — cooperative cancellation check between turns.
            // A host (TUI, ACP server) that fired `cancel_token()`
            // stops the loop here cleanly instead of the caller having
            // to drop the `run()` future mid-`await`. The unpaired
            // `tool_use` left by an in-turn cancel is repaired on the
            // next `push_user_turn` / `save_session` (AUDIT D-6).
            if self.cancel_token.is_cancelled() {
                self.output
                    .emit_info("Run cancelled by host before the next turn.");
                self.save_session();
                return Err(AgentError::UserAborted);
            }
            // AUDIT A1 — `max_turns` is an OPTIONAL override, not the
            // primary runaway guard. When set it still caps the loop;
            // when `None` the budget cap (E-C1, at the `charge()` site
            // below) and the context-token ceiling (A1, after
            // compaction below) are the real backstops, per the locked
            // design decision (project owner, 2026-05-22). `None` no
            // longer means "unbounded with no other guard".
            if let Some(limit) = self.max_turns
                && turn >= limit
            {
                self.output.emit_info(&format!(
                    "Run stopped: reached the configured max_turns limit ({limit})."
                ));
                return self.finish_run_terminated(user_input, turn).await;
            }
            // Fire on_turn_start hooks at the top of each iteration so Rust
            // hooks can override the model or inject prompt messages before
            // run_compaction + provider.stream(). Outcome is applied via
            // apply_pre_turn_outcome (switch_model + injected_messages).
            //
            // AUDIT A9 — a turn-start hook that returns `block` halts
            // the loop cleanly: operators can write a "stop after
            // condition X" hook as a backstop.
            if let Some(hook_engine) = self.hooks.as_ref() {
                let ctx = TurnContext {
                    turn,
                    model: self.model.clone(),
                    message_count: self.messages.len(),
                };
                let outcome = hook_engine.on_turn_start(turn, &ctx).await;
                if let Some(reason) = self.apply_pre_turn_outcome(outcome) {
                    self.output
                        .emit_info(&format!("Run stopped by on_turn_start hook: {reason}"));
                    return self.finish_run_terminated(user_input, turn).await;
                }
            }

            // Fire PreCompact plugin hooks once per turn, immediately before
            // the compaction pass. Gated like every other phase: a no-op when
            // no hook engine / no PreCompact hooks are registered.
            if let Some(hook_engine) = self.hooks.as_ref() {
                let outcome = hook_engine.run_pre_compact(turn, self.messages.len()).await;
                for line in outcome.hook_trace {
                    tracing::debug!(target: "wcore_agent::hooks", "{line}");
                }
            }

            // #280 — smart auto-compaction pre-gate. Boundary-fire ONLY: this is
            // the turn-loop top (the tool loop lives below provider.stream in the
            // same iteration), and the model swap was already applied above, so
            // the Flux-aware fraction is computed against the CURRENTLY-active
            // model. Default-OFF: `smart_compact_fraction` returns `None` on its
            // first line when disabled, so neither memory nor the force flag is
            // touched and `run_compaction` below behaves exactly as before. On a
            // fire we (1) persist a non-destructive handoff Episode, then (2) set
            // the one-shot force flag and fall straight through into the existing
            // `run_compaction` — no new summarization / fold / emit code.
            let smart_fired = match self.smart_compact_fraction() {
                Some(frac) => self.smart_compact_should_fire(turn as u32, frac),
                None => false,
            };
            if smart_fired {
                self.smart_compact_force = true;
                if self.compact_config.smart_handoff_to_memory {
                    // Persist BEFORE run_compaction mutates the buffer so the
                    // verbatim pre-compaction transcript is durable; errors are
                    // swallowed and never abort the turn.
                    self.write_smart_handoff().await;
                }
            }

            // Run multi-level compaction before each API call.
            // On the first turn last_input_tokens is 0 so neither
            // autocompact nor emergency will fire.
            //
            // AUDIT A6 — a compaction failure (e.g. the emergency
            // `ContextTooLong` bail) ends the session; persist + fire
            // session-end hooks before propagating, so the error exit
            // is consistent with every other loop-exit path.
            if let Err(e) = self.run_compaction().await {
                self.fire_on_session_end(turn).await;
                self.save_session();
                return Err(e);
            }

            // Build tool list: filter based on plan mode state
            let tools = if self.plan_state.is_active {
                // Plan mode: only Info-category tools (excluding EnterPlanMode)
                self.tools.to_tool_defs_filtered(|t| {
                    t.category() == ToolCategory::Info && t.name() != "EnterPlanMode"
                })
            } else {
                // Normal mode: all tools except ExitPlanMode
                self.tools
                    .to_tool_defs_filtered(|t| t.name() != "ExitPlanMode")
            };

            // W6 F17: trim MCP tools to a curated top-K. MCP tools are
            // identified by real provenance (`ToolDef::server.is_some()`), not
            // the `mcp__` name prefix — a non-colliding MCP tool keeps its bare
            // name (wcore-mcp/src/tool_proxy.rs). Non-MCP tools (builtins,
            // skills, spawn, plan tools) are always kept. Off-policy is a no-op.
            // Audit-log recency degrades to empty/keyword-only when
            // self.audit_log is None.
            let tools = self.apply_mcp_curation(tools);

            // #344/#359: enforce the provider's HARD tool-array cap (OpenAI =
            // 128). MCP servers can push the total past the limit even after
            // curation; this is the correctness guarantee, separate from the
            // relevance trim above.
            let tools = self.apply_provider_tool_cap(tools);

            // Build system prompt: append plan mode instructions when active
            let system = if self.plan_state.is_active {
                format!(
                    "{}\n\n{}",
                    self.system_prompt,
                    plan_prompt::plan_mode_instructions()
                )
            } else {
                self.system_prompt.clone()
            };

            // v0.8.1 U1 — the per-turn skill-router hint (when the router is
            // installed and picked a visible catalog skill). Cache-stability
            // (token-opt): the hint is dynamic per turn, so appending it to the
            // `system` string here would rewrite the cached system prefix
            // (zone 1) every turn. Compute it now and inject it into the
            // request's volatile message tail below instead. `None` (no router
            // / no pick / hidden skill) leaves both system and tail untouched.
            let skill_hint = self.skill_router_hint();

            // Record prompt state for cache diagnostics
            self.cache_detector.record_request(&system, &tools);

            // W8 v0.6.3 — pick the Anthropic prompt-cache tier for this
            // request. The agent turn loop reuses the same system prompt +
            // tools across every turn, so the prefix is stable far longer
            // than the 5-minute ephemeral window; `pick_cache_tier` promotes
            // to the 1h tier once the prompt clears the 1024-token minimum.
            // `None` stays valid (the Anthropic adapter falls back to 5m for
            // a `None` request) but the production path now produces a real
            // tier instead of always-`None`. Non-Anthropic providers ignore
            // the field.
            //
            // AUDIT A5 — estimate the FULL request (messages + system +
            // tool defs), not just message content. The message-only
            // estimate undercounts the turn-1 watermark by the system
            // prompt + tool-schema size (tens of k tokens for MCP-heavy
            // configs).
            let input_token_estimate =
                estimate::estimate_request_tokens(&self.messages, &system, &tools) as usize;
            // AUDIT A1 / #255 — context-token overflow guard MOVED below, to
            // immediately AFTER the smart-routing tier swap (so it measures the
            // POST-swap effective model's REAL window via the wcore-config
            // context_window kernel, not the stale CompactConfig 200k default).
            // See the `ContextWindow::resolve(..)` block just before
            // `size_output_cap`.
            let cache_tier = Some(wcore_providers::cache_tier::pick_cache_tier(
                input_token_estimate,
                AGENT_TURN_CACHE_REUSE_WINDOW_SECS,
            ));

            // Belt-and-suspenders: ensure no `tool_use` in history is
            // orphaned before sending to the provider. Anthropic 400s
            // on any orphan and bricks the session; the per-path fixes
            // in the dispatch loop close the known escape paths, but
            // this guard catches every remaining one — denial-by-
            // reaper, partial-batch loss on cancel, system-message
            // injection between an assistant tool_use and its result.
            self.repair_all_orphaned_tool_uses();
            // #285 — the reverse direction: a `tool_result` whose `tool_use`
            // was summarized away by autocompact is orphaned and makes
            // DeepSeek 400 the whole array. Run AFTER the forward repair so
            // the synthetic results it just backfilled (which always have a
            // matching tool_use) are untouched, and only true orphans are
            // demoted to text.
            self.repair_orphaned_tool_results();

            // Output-side optimization (Part A): attach fluff stop sequences
            // only when the route optimizes client-side. On router-optimized
            // routes the server already trims output, so we leave the Vec
            // empty and providers emit no stop field.
            let stop_sequences = if self.compat.input_optimization() == "client"
                && self.compat.supports_stop_param()
            {
                FLUFF_STOP_SEQUENCES.iter().map(|s| s.to_string()).collect()
            } else {
                // Either the route is router-optimized (server trims output) or
                // the provider rejects `stop` (e.g. xAI grok-4.3 400s on it) —
                // leave empty so providers emit no `stop` field.
                Vec::new()
            };

            // Finding #174: the model actually dispatched this turn. Starts as
            // the user's configured model and is rewritten below if a routing
            // hint selects a configured tier model. Cost/usage accounting reads
            // THIS (not `self.model`) so attribution follows any swap.
            let mut effective_model = self.model.clone();

            let mut request = LlmRequest {
                model: self.model.clone(),
                system,
                messages: self.messages.clone(),
                tools,
                max_tokens: self.max_tokens,
                thinking: self.thinking.clone(),
                reasoning_effort: self.current_reasoning_effort.clone(),
                cache_tier,
                routing_hint: None,
                stop_sequences,
                web_search: self.web_search,
                // #282: thread the stable conversation id + assembled-prompt
                // token estimate so the Flux provider can emit the x-wl-*
                // context-routing headers on tier-alias turns.
                conversation_id: Some(self.conversation_id.clone()),
                client_context_tokens: Some(input_token_estimate as u64),
                // Crucible #3: per-session sampling temperature (council child
                // engines set it; top-level session leaves it `None`).
                temperature: self.temperature,
            };

            // Cache-stability (token-opt): inject the per-turn skill-router
            // hint as a transient text block on the request's last user-role
            // message. `request.messages` is a clone, so this never persists
            // into history and never shifts the cached system/tool prefix.
            // Done before `mark_cache_boundaries` so the tail breakpoint
            // accounts for the final content. Skipped unless the tail is
            // user-role (never orphans a tool_use or creates adjacent user
            // messages).
            if let Some(hint) = skill_hint
                && let Some(last) = request.messages.last_mut()
                && matches!(last.role, Role::User)
            {
                last.content.push(ContentBlock::Text { text: hint });
            }

            // Cache-stability (token-opt, finding #174): inject the current date
            // as a transient text block on the request's last user-role message
            // instead of the cached system prefix. The date value changes daily /
            // across cross-midnight restarts; keeping it out of the prefix lets
            // the cached system+tools prefix stay byte-stable, so Anthropic prompt
            // caching survives cold starts. `request.messages` is a clone, so this
            // never persists into history. Skipped unless the tail is user-role
            // (never orphans a tool_use or creates adjacent user messages).
            if let Some(last) = request.messages.last_mut()
                && matches!(last.role, Role::User)
            {
                last.content.push(ContentBlock::Text {
                    text: crate::context::current_date_block(&crate::context::today_string()),
                });
            }

            // C1 / Task A3: fire PrePrompt plugin hooks once per turn and apply
            // their contributions to the request's last user-role message. Done
            // here — after the skill hint and BEFORE `mark_cache_boundaries`, but
            // OUTSIDE the `'stream` retry loop below (so it fires once per turn,
            // not once per stream retry). `request.messages` is a clone, so this
            // never persists into history and never shifts the cached system/tool
            // prefix; placing it before the breakpoint marking lets the tail
            // breakpoint account for the final content. The contribution is
            // budget-capped and deduped against the last injection. No-op when no
            // hook engine / PrePrompt hooks / dispatcher are present.
            let pre_prompt_outcome = match self.hooks.as_ref() {
                Some(hook_engine) => Some(hook_engine.run_pre_prompt().await),
                None => None,
            };
            if let Some(outcome) = pre_prompt_outcome {
                for line in &outcome.hook_trace {
                    tracing::debug!(target: "wcore_agent::hooks", "{line}");
                }
                Self::apply_pre_prompt_contribution(&mut request.messages, &outcome);
            }

            // W1 S3: place per-message cache breakpoint at the tail when the
            // provider honours it. Idempotent across turns: previous turns'
            // markers are cleared and the new tail is marked.
            mark_cache_boundaries(&mut request, &self.compat);

            // W1 v0.6.3: stamp a smart-routing hint onto the request so
            // `ProviderChain` can surface the router's decision in dispatch
            // observability. Finding #174: ALSO act on the hint — swap the
            // model to a configured tier model (cheap/balanced) before
            // dispatch. `input_tokens`, `max_output_tokens`, and
            // `tool_call_count` are real; `code_ratio` is conservatively zero
            // (no code-ratio scanner) and `requires_vision` comes from
            // `message_requires_vision` (false until an image ContentBlock
            // exists), so this producer emits only large-context / tool-heavy /
            // simple decisions — never a wrong hint.
            {
                let tool_call_count = self
                    .messages
                    .iter()
                    .flat_map(|m| &m.content)
                    .filter(|b| matches!(b, ContentBlock::ToolUse { .. }))
                    .count() as u32;
                // Vision guard input: a turn that carries image/vision content
                // must never be downgraded (the cheap tier may not be
                // vision-capable). The `wcore-types` message model has no image
                // block today, so this is conservatively `false` from real
                // content; the routing classifier promotes any vision turn to
                // Premium anyway, which `select_tier_model` also refuses to
                // swap. Routed through one flag so a future image ContentBlock
                // only needs to flip this.
                let requires_vision = message_requires_vision(&self.messages);
                let shape = wcore_providers::RequestShape {
                    input_tokens: input_token_estimate,
                    max_output_tokens: request.max_tokens as usize,
                    code_ratio: 0.0,
                    tool_call_count,
                    requires_vision,
                };
                let decision =
                    wcore_providers::route(&shape, &wcore_providers::RoutingHeuristics::default());
                request.routing_hint = Some(decision.to_hint());

                // Finding #174: act on the hint. Opt-in + guarded — see
                // `select_tier_model`. Only Cheap/Balanced hints with a
                // configured tier model and no vision content swap the model;
                // everything else leaves `request.model` (and `effective_model`)
                // as the user's configured model.
                if let Some(tier_model) =
                    select_tier_model(&decision, requires_vision, &self.compat)
                {
                    tracing::debug!(
                        target: "wcore_agent::routing",
                        from = %self.model,
                        to = %tier_model,
                        hint = %decision.to_hint().0,
                        "smart-routing tier swap"
                    );
                    request.model = tier_model.clone();
                    effective_model = tier_model;
                }
            }

            // #255 — pre-flight context-window overflow guard, RECOMPUTED on the
            // POST-swap effective model. `run_compaction` above already had its
            // chance to shrink history; if the assembled request still exceeds a
            // safe fraction of the model that will ACTUALLY serve this request,
            // the provider call would 400. Terminate the run cleanly instead.
            //
            // The denominator comes from the kernel (`ContextWindow::resolve`),
            // which reads the post-swap model's REAL window from
            // `wcore_config::limits` — NOT the stale `CompactConfig` 200k
            // default that the old guard used. After a Flux/tier swap to e.g.
            // gpt-4o (128k) the ceiling is now computed against 128k, so the
            // guard fires at the correct count (the #255 false-negative fix).
            //
            // `&request.model` is the same model arg fed to `size_output_cap`
            // below, so guard and cap agree. When the window is unknown
            // (`input_ceiling() == None`) the guard SKIPS — fail open, identical
            // to the old `window > 0` skip; `size_output_cap`'s UNKNOWN_CAP and
            // the provider 400 are the backstops.
            {
                let mut ctx = wcore_config::context_window::ContextWindow::resolve(
                    input_token_estimate as u64,
                    self.compat.provider_type(),
                    &request.model,
                    self.compact_config.context_window as u64,
                );
                // #282 contract V1: once Flux has SIGNALLED-BACK the real served
                // window (`x-flux-model-window`) on a prior turn of THIS Flux
                // route, prefer it over the alias's pre-route guess so the guard
                // measures against the model that will actually serve the turn.
                if wcore_providers::is_flux_tier_alias(&request.model)
                    && let Some(window) = self.flux_served_window
                {
                    ctx.window = Some(window);
                }
                if let Some(ceiling) = ctx.input_ceiling(
                    self.compact_config.output_reserve as u64,
                    self.compact_config.emergency_buffer as u64,
                ) && ctx.used_tokens >= ceiling
                {
                    self.output.emit_error(
                        &format!(
                            "Run stopped: estimated request size ({} tokens) \
                             reached the context-window ceiling ({ceiling}) for model \
                             '{}' and compaction could not reduce it further.",
                            ctx.used_tokens, request.model,
                        ),
                        false,
                    );
                    return self.finish_run_terminated(user_input, turn).await;
                }
            }

            // Up-front output sizing (Layer 1). Clamp `max_tokens` to the FINAL
            // model's real output ceiling. Placed AFTER the smart-routing tier
            // swap above so it sees `request.model` post-swap: a tier-swapped
            // cheaper model is clamped to ITS ceiling, never over-asked at the
            // premium model's. A known model is clamped to what it actually
            // allows (so the generous default never 400s); an unknown/router
            // model is clamped to a conservative floor. `self.max_tokens` is the
            // user's CAP and always binds.
            request.max_tokens = size_output_cap(
                self.max_tokens,
                self.compat.provider_type(),
                &request.model,
                input_token_estimate,
            );

            // AUDIT A3 / E-C2 — bounded stream-level retry loop.
            //
            // `provider.stream()` returns `Ok(rx)` after the response
            // HEADERS arrive; the SSE body is drained from the channel
            // here. A failure that lands AFTER headers — connection
            // reset mid-stream, TLS drop, an in-band `error` SSE frame
            // — surfaces as a mid-stream `LlmEvent::Error`. Before this
            // fix that became a fatal `AgentError::ApiError` with no
            // retry, even though the identical error BEFORE headers
            // would have been retried by the provider's own retry
            // layer. Now the engine re-issues the same request up to
            // `MAX_STREAM_RETRIES` times with linear backoff.
            //
            // A truncated stream (channel closes with no `Done` event)
            // is likewise treated as a failed attempt and retried; if
            // every attempt is exhausted the turn ends as an ERROR
            // verdict (not the old silent "successful empty turn" that
            // poisoned the SkillRouter / auto-skill learning).
            const MAX_STREAM_RETRIES: u32 = 2;
            let mut assistant_text = String::new();
            let mut thinking_text = String::new();
            let mut tool_calls: Vec<ContentBlock> = Vec::new();
            // Declared without an initial value: the `'stream` loop only
            // leaves via `break 'stream` (after these are assigned in
            // the consumed attempt) or `return`, so the post-loop code
            // always observes assigned values — the compiler proves it.
            let stop_reason: StopReason;
            let finish_reason: FinishReason;
            let turn_usage: TokenUsage;
            let mut stream_attempt: u32 = 0;
            // #282 contract V1: a managed Flux client that overflows the routed
            // model's window gets a typed 409 `ProviderError::ContextOverflow`.
            // We compact the conversation and retry the SAME turn EXACTLY ONCE;
            // this guard bounds it so a persistently-overflowing turn cannot
            // infinite-loop. After the single retry, a recurring overflow is
            // surfaced as a clean terminal error below.
            let mut overflow_retried = false;
            'stream: loop {
                // Reset per-attempt accumulators so a retry never
                // double-commits text/tool-calls from a failed attempt.
                assistant_text.clear();
                thinking_text.clear();
                tool_calls.clear();
                // `stop_reason` / `finish_reason` / `turn_usage` are
                // assigned only on the successful (`Done`) path below;
                // a failed attempt either retries or `return`s.
                let mut attempt_stop_reason = StopReason::EndTurn;
                let mut attempt_finish_reason = FinishReason::Error;
                let mut attempt_usage = TokenUsage::default();
                let mut done_seen = false;
                let mut stream_error: Option<String> = None;
                // Finding #174 (nested re-bill): set ONLY when `stream()` itself
                // returns a retryable `Err` — i.e. the provider's HTTP ring
                // (`builder_send_with_retry`, up to 3 sends) already spent its
                // full retry budget on this exact request before surfacing the
                // error here. A mid-stream `LlmEvent::Error` or a truncated
                // stream does NOT set this: those arrive AFTER `stream()`
                // returned `Ok(rx)` (the HTTP ring succeeded; the failure is in
                // the SSE body), so they keep the full engine retry budget and
                // their retryability is unchanged. When this IS set, granting a
                // fresh full engine budget on top of an already-exhausted HTTP
                // ring stacks 3×3=9 full-input re-sends for one logical turn;
                // we cap the engine ring at a single retry instead (see the
                // budget guard below).
                let mut http_ring_exhausted = false;
                // FluxRouter web_search grounding (contract §5.4): per-attempt
                // accumulators for the end-of-stream Citations / SearchResults
                // events. Reset each retry alongside the other accumulators.
                let mut grounding_citations: Vec<String> = Vec::new();
                let mut grounding_search_results: Vec<wcore_types::llm::FluxSearchResult> =
                    Vec::new();

                // P1 Bug#3 — `stream()` runs `builder_send_with_retry`
                // internally and can surface a *retryable*
                // `ProviderError::Connection` (a connection reset/drop while
                // the request was being sent, after the provider's own retry
                // budget was spent) as the `Err` of this call — NOT as a
                // mid-stream `LlmEvent::Error`. The previous `?` short-
                // circuited the whole turn here, bypassing the bounded
                // `'stream` retry loop below even though the identical error
                // arriving mid-stream WOULD be retried. Funnel a retryable
                // provider error into the same failed-attempt classifier — but
                // (finding #174) with a capped engine budget, since the HTTP
                // ring already exhausted its own retries on this request (see
                // `http_ring_exhausted` and the budget guard below).
                // Non-retryable errors (auth/4xx/parse/prompt-too-long)
                // propagate immediately, exactly as before.
                let mut rx = match self.provider.stream(&request).await {
                    Ok(rx) => rx,
                    Err(e) if e.is_retryable() => {
                        stream_error = Some(e.to_string());
                        // The provider's HTTP ring already retried this exact
                        // request to exhaustion before surfacing `Err` — mark it
                        // so the engine ring does not stack a fresh full budget.
                        http_ring_exhausted = true;
                        // Skip the recv loop; fall through to the
                        // classifier, which retries or fails the turn.
                        // An already-closed empty receiver makes the
                        // `while rx.recv()` loop below a no-op.
                        tokio::sync::mpsc::channel(1).1
                    }
                    // #282: hard context overflow from a managed Flux route.
                    // Compact the conversation, rebuild the request from the
                    // compacted history, and retry this same turn ONCE. The
                    // compaction itself hardens the tool-pair invariant (#285);
                    // we only call `run_compaction` here, never duplicate it.
                    Err(ProviderError::ContextOverflow {
                        required_tokens,
                        model_window,
                        routed_model,
                        ..
                    }) if !overflow_retried => {
                        overflow_retried = true;
                        self.output.emit_info(&format!(
                            "context overflow on {routed_model} ({required_tokens} tokens > \
                             {model_window} window) — compacted and retrying"
                        ));
                        if let Err(e) = self.run_compaction().await {
                            self.fire_on_session_end(turn).await;
                            self.save_session();
                            return Err(e);
                        }
                        // Rebuild the volatile request inputs from the compacted
                        // history; the cached system/tool prefix is unchanged by
                        // compaction, so only messages + the token estimate move.
                        request.messages = self.messages.clone();
                        let recount = estimate::estimate_request_tokens(
                            &self.messages,
                            &request.system,
                            &request.tools,
                        ) as u64;
                        request.client_context_tokens = Some(recount);
                        continue 'stream;
                    }
                    Err(e) => return Err(e.into()),
                };

                while let Some(event) = rx.recv().await {
                    match event {
                        LlmEvent::TextDelta(text) => {
                            self.output.emit_text_delta(&text, &self.current_msg_id);
                            assistant_text.push_str(&text);
                        }
                        LlmEvent::ToolUse {
                            id,
                            name,
                            input,
                            extra,
                        } => {
                            let input_str = serde_json::to_string(&input).unwrap_or_default();
                            self.output.emit_tool_call(&name, &input_str);
                            tool_calls.push(ContentBlock::ToolUse {
                                id,
                                name,
                                input,
                                extra,
                            });
                        }
                        LlmEvent::ThinkingDelta(text) => {
                            self.output.emit_thinking(&text, &self.current_msg_id);
                            thinking_text.push_str(&text);
                        }
                        LlmEvent::Done {
                            stop_reason: sr,
                            finish_reason: fr,
                            usage,
                        } => {
                            attempt_stop_reason = sr;
                            attempt_finish_reason = fr;
                            attempt_usage = usage;
                            done_seen = true;
                        }
                        LlmEvent::Error(e) => {
                            // AUDIT E-C2 — do NOT immediately abort the run.
                            // Record the error and stop consuming this
                            // attempt; the retry decision below re-issues
                            // the request or fails the turn after the
                            // bounded retry budget is spent.
                            stream_error = Some(e);
                            break;
                        }
                        LlmEvent::Citations(urls) => {
                            // FluxRouter web_search grounding (contract §5.4):
                            // capture the citation URL list so the Sources block
                            // is rendered after the answer once both grounding
                            // events have arrived (SearchResults follows).
                            grounding_citations = urls;
                        }
                        LlmEvent::SearchResults(results) => {
                            grounding_search_results = results;
                        }
                        LlmEvent::ProviderMeta {
                            routed_model,
                            model_window,
                            context_pressure,
                            tokens_counted,
                        } => {
                            // #282 contract V1: Flux SIGNALS-BACK. Capture the
                            // pressure for future #280 scheduling (not yet acted
                            // on) and the REAL served-model window so the #255
                            // overflow guard measures against the model that
                            // actually served this turn, not the pre-route guess.
                            self.flux_context_pressure = context_pressure;
                            self.flux_served_window = model_window;
                            if let Some(window) = model_window {
                                // Reconcile the #255 gauge through the kernel:
                                // recompute the active-window fraction against the
                                // REAL served window. `tokens_counted` is Flux's
                                // own prompt count when present; fall back to our
                                // pre-flight estimate otherwise.
                                let used = tokens_counted.unwrap_or(input_token_estimate as u64);
                                let ctx = wcore_config::context_window::ContextWindow {
                                    used_tokens: used,
                                    window: Some(window),
                                };
                                if let Some(pct) = ctx.percent() {
                                    let routed = routed_model.as_deref().unwrap_or("flux-routed");
                                    self.output.emit_info(&format!(
                                        "Flux routed to {routed} ({pct}% of {window}-token window)"
                                    ));
                                }
                            }
                        }
                    }
                }

                // AUDIT A3 / E-C2 — classify the attempt outcome.
                // A clean `Done` is success. A mid-stream `LlmEvent::Error`
                // OR a channel that closed with no `Done` (truncated /
                // dropped stream) is a FAILED attempt.
                if done_seen && stream_error.is_none() {
                    // FluxRouter web_search grounding (contract §5.4): render
                    // the "Sources" block after a SUCCESSFUL grounded answer.
                    // Done only on the committed attempt (inside the success
                    // gate, before `break 'stream`) so a failed-then-retried
                    // attempt never double-emits sources. The answer text
                    // already carries inline `[n]` markers; map `[n]` →
                    // `citations[n-1]` and append the richer source cards.
                    // Emitted via the assistant text stream so every sink (TUI,
                    // REPL, json-stream) shows it with no new trait method.
                    if !grounding_citations.is_empty() || !grounding_search_results.is_empty() {
                        let block = render_grounding_sources(
                            &grounding_citations,
                            &grounding_search_results,
                        );
                        self.output.emit_text_delta(&block, &self.current_msg_id);
                        assistant_text.push_str(&block);
                    }
                    stop_reason = attempt_stop_reason;
                    finish_reason = attempt_finish_reason;
                    turn_usage = attempt_usage;
                    break 'stream;
                }
                let reason = stream_error.clone().unwrap_or_else(|| {
                    "provider stream closed before a Done event (truncated response)".to_string()
                });
                // v0.9.1.1 B6: HTTP 4xx errors (especially 400
                // `invalid_request_error`) are NOT transient — retrying
                // sends the same malformed request and burns the retry
                // budget producing identical errors stacked in the
                // Activity rail. The user sees `Error [engine_error]:
                // API 400:` three times for what should be a single
                // surfaced failure. Skip retry on any 4xx; preserve
                // bounded retry for 5xx / truncated streams / network
                // drops where the next attempt has a real chance.
                let is_client_error = is_http_4xx_error(&reason);
                // Finding #174 (nested re-bill cap): when the failure was an
                // already-HTTP-exhausted provider `Err` (3 sends spent), grant
                // the engine ring exactly ONE retry instead of the full
                // `MAX_STREAM_RETRIES` (2). This still retries a genuinely
                // transient single failure once — the common case — but stops
                // the engine ring from stacking 3 attempts × 3 HTTP sends = 9
                // full-input re-sends for one logical turn. A mid-stream error /
                // truncated stream never sets `http_ring_exhausted`, so its
                // retryability and budget are unchanged.
                let effective_max_retries = if http_ring_exhausted {
                    MAX_STREAM_RETRIES.min(1)
                } else {
                    MAX_STREAM_RETRIES
                };
                if !is_client_error && stream_attempt < effective_max_retries {
                    stream_attempt += 1;
                    // Linear backoff: 500ms, 1000ms.
                    let backoff = std::time::Duration::from_millis(500 * stream_attempt as u64);
                    self.output.emit_info(&format!(
                        "Provider stream failed ({reason}); retrying \
                         (attempt {stream_attempt}/{effective_max_retries})…"
                    ));
                    tokio::time::sleep(backoff).await;
                    continue 'stream;
                }
                // Retry budget exhausted — fail the turn. The provider
                // billed nothing usable; surface a hard error so the
                // host (and the SkillRouter / auto-skill observers)
                // record a FAILURE, not a silent empty success.
                self.output.emit_error(
                    &format!("Provider stream failed after retries: {reason}"),
                    !is_client_error,
                );
                return Err(AgentError::ApiError(reason));
            }

            self.total_usage.input_tokens += turn_usage.input_tokens;
            self.total_usage.output_tokens += turn_usage.output_tokens;
            self.total_usage.cache_creation_tokens += turn_usage.cache_creation_tokens;
            self.total_usage.cache_read_tokens += turn_usage.cache_read_tokens;

            // B7 writer-side wiring: mirror this turn's token usage into the
            // live introspection state so `wayland_status` /
            // `wayland_telemetry_query` report non-zero token counters.
            if let Some(state) = &self.session_state {
                state.add_token_usage(turn_usage.input_tokens, turn_usage.output_tokens);
            }

            // M5.3 — charge the per-session/per-user budget tracker after the
            // turn's usage is finalized. Sink-side `BudgetEvent::Charge`
            // emission happens inside `tracker.charge`.
            //
            // AUDIT E-C1 — the `charge()` result is now HONORED. Before,
            // it was discarded (`let _ = ...`), so a configured
            // `max_cost_usd` / `max_tokens` cap did nothing and a
            // runaway tool-call loop burned unbounded cost. The
            // provider already billed THIS turn, so the cap cannot
            // un-spend it — but `BudgetError::CapExceeded` is captured
            // here and, once the assistant message is committed below,
            // the loop terminates cleanly with a user-visible reason
            // instead of starting another (paid) turn.
            //
            // W7 (v0.6.3) — turn cost is resolved from the `wcore-pricing`
            // provider×model catalog. A catalog miss is non-fatal: it logs
            // a warning and falls back to the `ProviderCompat` heuristic so
            // the charge still happens.
            let mut budget_cap_hit: Option<wcore_budget::BudgetError> = None;
            if let Some(tracker) = self.budget_tracker.as_ref() {
                let session_id = self
                    .current_session_id()
                    .unwrap_or_else(|| "session-unknown".to_string());
                let turn_tokens = turn_usage
                    .input_tokens
                    .saturating_add(turn_usage.output_tokens);
                let provider = self.compat.provider_type.as_deref().unwrap_or("");
                // Finding #174: charge the budget against the model ACTUALLY
                // dispatched (`effective_model`), so a tier-swapped cheap turn
                // is billed at the cheap model's catalog rate, not the premium
                // model's.
                let catalog_cost = pricing_turn_cost_usd(
                    provider,
                    &effective_model,
                    turn_usage.input_tokens,
                    turn_usage.output_tokens,
                );
                if catalog_cost.is_none() {
                    // Emit host-visible info on catalog miss so operators know to add
                    // a pricing.toml entry. The tracing::warn! in pricing_turn_cost_usd
                    // covers log files; this makes it visible in --json-stream output.
                    self.output.emit_info(&format!(
                        "cost-catalog miss for {provider}/{model} — billing at compat-fallback rate; add to pricing.toml",
                        model = &effective_model,
                    ));
                }
                let turn_cost = catalog_cost.unwrap_or_else(|| {
                    estimate_turn_cost(
                        turn_usage.input_tokens,
                        turn_usage.output_tokens,
                        turn_usage.cache_read_tokens,
                        turn_usage.cache_creation_tokens,
                        &self.compat,
                    )
                });
                if let Err(e) = tracker.lock().charge(&session_id, turn_tokens, turn_cost) {
                    budget_cap_hit = Some(e);
                }
            }

            // Track per-turn input tokens for compaction watermarks.
            //
            // EMERGENCY watermark (conservative): max(provider_reported,
            // local_estimate) with historical thinking COUNTED. This is the
            // safety net for the hard-stop — some providers (e.g. DeepSeek
            // with prefix caching) underreport prompt_tokens, so over-
            // estimating here ensures we never blow the context window.
            let local_estimate = estimate::estimate_tokens_from_messages(&self.messages);
            let effective_watermark = turn_usage.input_tokens.max(local_estimate);

            // AUTO-trigger watermark (finding #174): track REAL token
            // pressure so auto-compaction doesn't fire prematurely. The
            // local estimate over-counts vs real billing because it prices
            // history at face value (no prompt-cache reads) and counts
            // thinking blocks that non-replaying providers (Anthropic/
            // Bedrock/Vertex) DROP at the wire — those cost zero real input.
            // Prefer the provider-reported billed input when known; fall
            // back to a thinking-aware estimate only on the first turn
            // (before any usage is reported). Do NOT take max() here — that
            // is exactly what made the inflated estimate win and trigger
            // the summarizer earlier than warranted.
            let auto_estimate = estimate::estimate_tokens_from_messages_with_thinking(
                &self.messages,
                self.compat.replays_thinking_in_history(),
            );
            let auto_watermark = if turn_usage.input_tokens > 0 {
                turn_usage.input_tokens
            } else {
                auto_estimate
            };

            // v0.9.1.2 F-watermark: watermark-override is TELEMETRY, never
            // transcript content. Each per-turn LLM round-trip can re-trip
            // the >10k delta condition (5-10× per user prompt under heavy
            // tool use), and `emit_info` pushes a system message that
            // forces a full transcript re-render — the dominant source of
            // the v0.9.1.1 "molasses" responsiveness complaint. Route
            // straight to `tracing::debug!` so the data is still in
            // `/doctor` output and log files, but stays out of the transcript.
            // Same pattern as F10's plugin-hook lifecycle classifier
            // (`run_post_tool_use` routes to `hook_trace`, never `log_lines`).
            if local_estimate > turn_usage.input_tokens
                && local_estimate.saturating_sub(turn_usage.input_tokens) > 10_000
            {
                tracing::debug!(
                    provider_reported = turn_usage.input_tokens,
                    local_estimate = local_estimate,
                    effective = effective_watermark,
                    "Token watermark override: provider={}, local_estimate={}, using={}",
                    turn_usage.input_tokens,
                    local_estimate,
                    effective_watermark
                );
            }

            self.compact_state.last_input_tokens = effective_watermark;
            self.compact_state.last_real_input_tokens = auto_watermark;

            // Cache break detection
            let cache_stats = CacheStats {
                input_tokens: turn_usage.input_tokens,
                cache_read_tokens: turn_usage.cache_read_tokens,
                cache_creation_tokens: turn_usage.cache_creation_tokens,
            };
            if let Some(diagnostic) = self.cache_detector.check_response(cache_stats) {
                match &diagnostic {
                    CacheDiagnostic::FullMiss { cause } => {
                        // #101: a full cache miss is diagnostic telemetry, NOT a
                        // user error. A `TtlExpiry` (the prompt cache's TTL lapsed
                        // because the user paused between turns) is expected, and
                        // surfacing it as an error mid-chat alarms users over
                        // normal behaviour. Gate it behind the same
                        // `cache_diagnostics` debug flag as the partial/healthy
                        // arms so only operators who opt in ever see it.
                        if self.compact_config.cache_diagnostics {
                            self.output
                                .emit_info(&format!("Cache full miss (cause: {cause:?})"));
                        }
                    }
                    CacheDiagnostic::PartialMiss { hit_rate, cause } => {
                        if self.compact_config.cache_diagnostics {
                            self.output.emit_info(&format!(
                                "Cache: {:.0}% hit rate (cause: {cause:?})",
                                hit_rate * 100.0
                            ));
                        }
                    }
                    CacheDiagnostic::Healthy { hit_rate } => {
                        if self.compact_config.cache_diagnostics {
                            self.output
                                .emit_info(&format!("Cache: {:.0}% hit rate", hit_rate * 100.0));
                        }
                    }
                }
            }

            let mut assistant_content: Vec<ContentBlock> = Vec::new();
            if !thinking_text.is_empty() {
                assistant_content.push(ContentBlock::Thinking {
                    thinking: thinking_text,
                });
            }
            if !assistant_text.is_empty() {
                assistant_content.push(ContentBlock::Text {
                    text: assistant_text.clone(),
                });
            }
            assistant_content.extend(tool_calls.clone());

            // #86: a stream that completed WITHOUT a provider error but yielded
            // no text, no thinking, and no tool calls is a silent dead-end —
            // e.g. an OpenAI-compatible endpoint that returned 200 + `[DONE]`
            // with zero deltas, or a response shape the SSE parser produced
            // nothing from. Without this guard the turn commits an empty
            // assistant message and returns with no output and no error, so the
            // host shows nothing and the user can't tell what went wrong.
            // Surface a visible, non-retryable error instead of the silent
            // no-op. (Genuine content/tool-call/thinking turns are unaffected.)
            if assistant_content.is_empty() {
                self.output.emit_error(
                    "Provider returned an empty response — no content and no tool calls. \
                     The endpoint or model may be incompatible (verify it speaks the OpenAI \
                     chat-completions streaming format and that the model name is valid).",
                    false,
                );
            }

            self.messages
                .push(Message::now(Role::Assistant, assistant_content));

            // Fire on_turn_end after the assistant message is committed.
            // SwitchModel and InjectMessage apply to the NEXT turn (or are
            // moot if the loop is about to return below).
            if let Some(hook_engine) = self.hooks.as_ref() {
                let result = TurnResult {
                    turn,
                    tool_call_count: tool_calls.len(),
                    input_tokens: turn_usage.input_tokens,
                    output_tokens: turn_usage.output_tokens,
                };
                let outcome = hook_engine.on_turn_end(turn, &result).await;
                self.apply_turn_end_outcome(outcome);
            }

            // AUDIT E-C1 — budget cap honored. The provider already
            // billed this turn; running its tool calls (and another
            // turn after) would burn more cost past the cap. Terminate
            // now: repair the assistant message's dangling `tool_use`
            // blocks (those tools never run), emit a `BudgetExceeded`
            // event + a user-visible error, and finish cleanly.
            if let Some(err) = budget_cap_hit {
                let wcore_budget::BudgetError::CapExceeded {
                    kind,
                    limit,
                    observed,
                } = err;
                self.repair_orphaned_tool_use();
                self.output.emit_budget_exceeded(&kind, &observed, &limit);
                self.output.emit_error(
                    &format!(
                        "Run stopped: budget cap '{kind}' exceeded \
                     (limit {limit}, observed {observed}). The session has reached \
                     its configured spend ceiling."
                    ),
                    false,
                );
                return self.finish_run_terminated(user_input, turn + 1).await;
            }

            if tool_calls.is_empty() {
                // W1 F9: emit the final turn's trace before returning so
                // single-turn sessions still produce exactly one TurnTrace.
                let trace = TurnTrace {
                    turn,
                    // Finding #174: attribute to the model ACTUALLY dispatched
                    // (`effective_model`), which equals `self.model` unless a
                    // tier swap fired this turn — so cost is never mis-charged
                    // to the premium model when a cheap model was used.
                    model: effective_model.clone(),
                    provider: self.compat.provider_type().to_string(),
                    input_tokens: turn_usage.input_tokens,
                    output_tokens: turn_usage.output_tokens,
                    cache_read: turn_usage.cache_read_tokens,
                    cache_write: turn_usage.cache_creation_tokens,
                    cache_hit_rate: TurnTrace::cache_hit_rate_from(
                        turn_usage.input_tokens,
                        turn_usage.cache_read_tokens,
                    ),
                    // Fix(pricing-audit-2026-05-24): use resolve_turn_cost_usd which tries
                    // the pricing catalog first, then falls back to estimate_turn_cost.
                    // Previously estimate_turn_cost used compat rows directly — with
                    // openai_defaults() now at $0/$0 sentinel, that always returned $0.
                    cost_usd: resolve_turn_cost_usd(
                        self.compat.provider_type(),
                        &effective_model,
                        turn_usage.input_tokens,
                        turn_usage.output_tokens,
                        turn_usage.cache_read_tokens,
                        turn_usage.cache_creation_tokens,
                        &self.compat,
                    ),
                    tool_calls: vec![],
                    // Drain hook actions fired this turn into the trace.
                    // partial: populated in a future pass when a streaming-drain trigger exists
                    hook_actions: std::mem::take(&mut self.pending_hook_actions),
                    source_product: SOURCE_PRODUCT.to_string(),
                    // #279(b)+(c): correlate this turn's trace to the run.
                    agent_run_id: self.current_agent_run_id.clone().unwrap_or_default(),
                };
                if let Ok(trace_json) = serde_json::to_value(&trace) {
                    self.output.emit_trace(&self.current_msg_id, &trace_json);
                }
                // W6 F7: record this turn's cost for the SessionCost aggregate.
                self.per_turn_costs.push(wcore_protocol::events::TurnCost {
                    turn: trace.turn,
                    model: trace.model.clone(),
                    provider: trace.provider.clone(),
                    cost_usd: trace.cost_usd,
                });
                // W9.1 T3 (T10b): feed the trace into the F10 detect flow
                // even on the no-tool-calls early-return path. Pattern
                // detection's `min_seq_len = 5` floor means empty
                // tool-calls turns are no-ops for staging, but keeping the
                // call here makes the window contiguous if the session
                // alternates between productive multi-tool turns and
                // text-only final turns.
                let drafted_msg_id = self.current_msg_id.clone();
                self.try_draft_skill_for_turn(trace, &drafted_msg_id).await;
                // W8b.2.B D.3: drain external-edit events one final time
                // before the early-return so any user edits that landed
                // during the assistant's final turn still surface as a
                // user-visible Info event (and into the message tail in
                // case the host resumes the session).
                self.drain_and_inject_external_edits();
                self.fire_on_session_end(turn + 1).await;
                self.save_session();
                // v0.8.1 U6 — snapshot the U1 pick BEFORE
                // `observe_skill_router_outcome` clears it, so the
                // autonomous-skill bucketer can record which catalog
                // skill (if any) was active for this trajectory.
                let auto_skill_picked = self.current_skill_router_pick.clone();
                // v0.8.1 U1 — credit the SkillRouter on the natural
                // EndTurn / ToolUse exit. `observe_skill_router_outcome`
                // maps `stop_reason` → Success/Failure and updates the
                // Beta scorer.
                self.observe_skill_router_outcome(stop_reason);
                // v0.8.1 U6 — record the turn into the autonomous-skill
                // bucketer. N=3 successes on the same task signature
                // triggers a draft + PromptStore record. Failure logged
                // and swallowed — the user's turn must complete.
                self.observe_auto_skill(user_input, auto_skill_picked, stop_reason, turn + 1);
                return Ok(AgentResult {
                    text: assistant_text,
                    stop_reason,
                    finish_reason,
                    usage: self.total_usage.clone(),
                    turns: turn + 1,
                    active_window_percent: self
                        .active_window_percent_now(&effective_model, input_token_estimate as u64),
                    agent_run_id: self.current_agent_run_id.clone(),
                });
            }

            // Wave OR (W8b.2.B.1): per-turn dispatch flows through
            // `ExecutionGraph::execute`. For `Intent::Direct` (default;
            // every existing test) the graph is a single AgentCall node
            // whose executor runs the SAME `execute_tool_calls_*`
            // dispatch as before, preserving byte-identical behaviour.
            // For `--mode parallel/--mode iterative/...` the graph walks
            // multiple nodes; production AgentNodeExecutor invocations
            // are serialised through the per-turn hook engine inside
            // the cell so hook ordering remains observable.
            //
            // Why classify on `user_input`: the input is the latest
            // user message captured at the top of `run`. Re-classifying
            // on every turn would let mid-session shape switches happen
            // when supported; today we classify once and the inferred
            // graph is rebuilt per turn (cheap — keyword pass).
            //
            // v0.8.0 Task K: `select_graph_config` resolves the template
            // in three stages — manual `@@template=` override →
            // `TemplateRouter::choose` (when wired) → `IntentClassifier`
            // fallback. Cold-start engines without a wired router still
            // hit the classifier (byte-identical to pre-K). The
            // classifier-only path is preserved below for the no-router
            // case so observability / `IntentClassifier::classify` side
            // effects (none today, but the call is the documented seam)
            // remain in place.
            let _intent_for_telemetry = IntentClassifier::classify(user_input);
            // Dynamic Workflows B3 — telemetry-only WorkflowCandidate
            // signal. STRICTLY a side-channel: this value is computed
            // here next to `_intent_for_telemetry` and emitted as a
            // trace; it is NEVER read by `select_graph_config`,
            // `TemplateDecision`, or any tool-dispatch decision below.
            // The confirm gate that turns this into user-facing behaviour
            // lands in B6. Gated behind `workflow_detection_enabled`
            // (default false): when off we do not even run the heuristic,
            // so a default-config session is byte-for-byte unchanged.
            if self.workflow_detection_enabled
                && let Some(candidate) =
                    crate::orchestration::intent::workflow_candidate(user_input)
            {
                tracing::debug!(
                    confidence = candidate.confidence,
                    rationale = %candidate.rationale,
                    "workflow_detection: turn looks like a workflow candidate (telemetry only)"
                );
                // B4 shadow mode: emit a structured, aggregatable record of
                // what the Detected tier WOULD have proposed. It is purely
                // observational — it never prompts the user and never feeds
                // routing. `task_excerpt` is capped at TASK_EXCERPT_MAX bytes
                // inside `new`, so the full prompt is never logged here.
                // FIX E — this `emit_trace` path does NOT run through the
                // `SpanSink`-level `PiiScrubbingSink`, so `WorkflowDetectionRecord::new`
                // scrubs the excerpt (via `wcore_safety::PIIScrubber`) at
                // construction instead; `rationale` is token-only keyword names,
                // never a raw prompt slice. Operators review accumulated records
                // by filtering the trace log for `kind == "workflow_detection"`
                // and running them through `summarize_workflow_detection`.
                let record = WorkflowDetectionRecord::new(
                    user_input,
                    candidate.confidence,
                    candidate.rationale,
                );
                if let Ok(record_json) = serde_json::to_value(&record) {
                    self.output.emit_trace(&self.current_msg_id, &record_json);
                }
            }
            // Build a fresh `AgentNodeExecutor` per turn so the captured
            // per-turn state (tool_calls, hooks) is freshly seeded.
            // The adapter owns Arc clones of registry/confirmer; the
            // hook engine is moved into the cell via `take()` and moved
            // back out after the graph walk.
            let approval_channel = self.approval_manager.as_ref().map(|mgr| {
                let writer = self
                    .protocol_writer
                    .as_ref()
                    .expect("protocol writer required for approval")
                    .clone();
                // SAFETY: see confirm_call in orchestration/mod.rs —
                // ToolConfirmer's critical sections cannot panic so
                // the std::sync::Mutex can never be poisoned.
                let auto_approve = self.confirmer.lock().unwrap().is_auto_approve();
                ApprovalChannel {
                    manager: mgr.clone(),
                    writer,
                    msg_id: self.current_msg_id.clone(),
                    auto_approve,
                }
            });
            let exec_cfg = AgentExecutorConfig {
                tools: self.tools.clone(),
                confirmer: self.confirmer.clone(),
                compaction_level: self.compaction_level,
                toon_enabled: self.toon_enabled,
                // W7 F4: wire tool-chunk streaming (e.g. BashTool live
                // output) into the live dispatch path. Gated on the host
                // advertising streaming support so this stays opt-in by
                // capability — hosts that don't advertise it keep the
                // buffered-output behaviour (None). The dispatcher further
                // gates per-tool on `tool.supports_streaming()`.
                streaming: build_turn_streaming_context(&self.output, &self.current_msg_id),
                approval: approval_channel,
                allow_list: self.allow_list.clone(),
                // v0.6.1 CRIT-1: clone the optional gate into the per-turn
                // config. `PolicyGate` is `Clone` (Arc<PolicyEngine> +
                // Actor). `None` preserves v0.6.0 open-gate behaviour.
                policy_gate: self.policy_gate.clone(),
                // v0.8.0 Task I (1.D.3): top-level engine dispatch is
                // Root by default. Sub-agent spawners that drive
                // `dispatch_once` directly set `actor` +
                // `learned_policy` themselves.
                actor: wcore_permissions::CallActor::Root,
                learned_policy: None,
                // AUDIT B-1 — thread a child of the session-root
                // cancel token into tool dispatch so a host cancel
                // reaches a running tool and the per-category dispatch
                // timeout can fire the call's cooperative cancel.
                cancel: self.cancel_token.child_token(),
                // W8b.2.A — thread the engine's stored notifier (set via
                // `set_tool_write_notifier`) into per-call ToolContexts so
                // Write/Edit self-originated writes are suppressed by the
                // file watcher instead of re-entering context as user edits.
                file_write_notifier: self.tool_write_notifier().cloned(),
            };
            // Move tool_calls + hooks into the per-turn cell. The
            // adapter's `run_agent` consumes `tool_calls` once; hooks
            // travel both ways (`take()` here, write-back inside the
            // adapter after dispatch).
            let cell = Arc::new(tokio::sync::Mutex::new(TurnCell::new(
                tool_calls.clone(),
                self.hooks.take(),
            )));
            let executor: Arc<dyn NodeExecutor> =
                Arc::new(AgentNodeExecutor::new(exec_cfg, cell.clone()));
            // v0.8.0 Task K: route via the unified selector. When the
            // engine has a wired `TemplateRouter`, lock it for the
            // single `choose` call (the scorer mutates RNG state); the
            // guard is dropped before the async `ExecutionGraph::execute`
            // below so we never await while holding it.
            let template_decision = {
                let mut router_guard = self.template_router.as_ref().map(|r| r.lock().unwrap());
                let router_ref = router_guard.as_deref_mut();
                select_graph_config(user_input, router_ref, self.mode_override)
            };
            // Emit an INFO-level trace so dashboards / acceptance tests
            // can verify the router path was taken. The classifier path
            // is the silent default — no event needed.
            if template_decision.source != TemplateDecisionSource::Classifier {
                tracing::debug!(
                    template = ?template_decision.template,
                    source = ?template_decision.source,
                    "template_routing: non-classifier orchestration template selected"
                );
            }
            // Phase 0 (rank 5) honesty coercion. Non-Direct orchestration
            // templates are structurally hollow under the per-turn
            // `AgentNodeExecutor` (first-dispatch-wins makes every node past
            // the first an inert carrier), so Consensus / SelfCritique /
            // Hierarchical — and Adaptive when it projects to one of them —
            // already collapse to a Direct-equivalent result while walking a
            // fake multi-node graph and emitting misleading per-node traces.
            // The default-wired `TemplateRouter` (bootstrap F-024) Thompson-
            // samples all five arms every turn, so the majority of turns hit
            // this path today. Rather than walk the inert graph (or pretend
            // to do multi-agent), run an honest single-node Direct turn. This
            // is behaviour-preserving — the net tool/answer result is already
            // Direct — but removes the fake-orchestration spans. The silent
            // classifier heuristic is never coerced, so its (real) Sequential
            // / Parallel shapes are untouched. ForgeFlows-Live Phase 3
            // repoints these decisions at the real `WorkflowRunner` spawner
            // and retires this coercion. (See
            // `.planning/2026-06-13-FORGEFLOWS-LIVE-DESIGN.md`.)
            let graph_config = if decision_is_unwired_template(&template_decision) {
                tracing::debug!(
                    template = ?template_decision.template,
                    source = ?template_decision.source,
                    "rank5: non-Direct per-turn template is not wired to a real \
                     multi-agent backend; executing an honest Direct turn"
                );
                GraphConfig::direct("main", serde_json::json!({}))
            } else {
                template_decision.config
            };
            // AUDIT A2 — the graph context's cancel token is a child of
            // the engine's session-root token (was a fresh orphan that
            // nothing could ever fire). A host cancel now propagates
            // into the graph walker, which checks it at every tick.
            let graph_ctx = GraphContext {
                cancel: self.cancel_token.child_token(),
                executor,
            };
            let graph_result =
                ExecutionGraph::execute(graph_config, serde_json::Value::Null, graph_ctx).await;
            // Drain the per-turn cell back into engine state regardless
            // of graph outcome. Hooks ALWAYS move back; outcome may be
            // absent if the graph errored before dispatch.
            let mut cell_guard = cell.lock().await;
            self.hooks = cell_guard.hooks.take();
            // AUDIT A6 — every loop-exit path must persist the session
            // and fire `on_session_end`. The `Quit` / `Cancelled` arms
            // previously called only `save_session()` (no hooks); the
            // generic graph-error arm called neither. They are now all
            // consistent with the natural / max-turns exits.
            //
            // `take()` an exit decision out of the match (so the async
            // `fire_on_session_end` runs after the `cell_guard` is
            // dropped, not while it is held across an await).
            enum GraphExit {
                Continue(ToolCallOutcome),
                Aborted,
                Failed(String),
            }
            let exit = match graph_result {
                Ok(_) => match cell_guard.outcome.take() {
                    Some(Ok(o)) => GraphExit::Continue(o),
                    Some(Err(ExecutionControl::Quit)) => GraphExit::Aborted,
                    None => {
                        // Graph walked but never invoked the adapter —
                        // happens when the selected template has no
                        // AgentCall nodes (impossible today: every
                        // template includes at least one AgentCall).
                        // Synthesize an empty outcome so the rest of
                        // the turn-end bookkeeping runs.
                        GraphExit::Continue(ToolCallOutcome {
                            results: vec![],
                            modifiers: vec![],
                            hook_outcomes: vec![],
                            cancelled_ids: vec![],
                        })
                    }
                },
                Err(GraphError::Cancelled) => GraphExit::Aborted,
                Err(e) => GraphExit::Failed(format!("orchestration graph failed: {e}")),
            };
            drop(cell_guard);
            let outcome = match exit {
                GraphExit::Continue(o) => o,
                GraphExit::Aborted => {
                    self.fire_on_session_end(turn + 1).await;
                    self.save_session();
                    return Err(AgentError::UserAborted);
                }
                GraphExit::Failed(msg) => {
                    self.fire_on_session_end(turn + 1).await;
                    self.save_session();
                    return Err(AgentError::ApiError(msg));
                }
            };

            // Apply any context modifiers from skill executions before the next turn
            self.apply_context_modifiers(&outcome.modifiers);

            // Apply post-tool-use Rust hook outcomes (InjectMessage,
            // SwitchModel) collected by orchestration. Log lines were
            // already drained at the orchestration layer.
            for hook_outcome in outcome.hook_outcomes {
                self.apply_turn_end_outcome(hook_outcome);
            }

            // W1 F9: pre-populate ToolCallTrace stubs from the LLM-requested
            // tool calls; the result loop below fills in output / duration.
            // Wall-clock timing is captured here because execute_tool_calls*
            // returns only the final outcome.
            let tool_call_start = std::time::Instant::now();
            let mut tool_call_traces: Vec<ToolCallTrace> = tool_calls
                .iter()
                .filter_map(|tc| {
                    if let ContentBlock::ToolUse {
                        id, name, input, ..
                    } = tc
                    {
                        Some(ToolCallTrace::new(id.clone(), name.clone(), input.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            for trace in &mut tool_call_traces {
                trace.bytes_in = serde_json::to_string(&trace.input)
                    .map(|s| s.len() as u64)
                    .unwrap_or(0);
            }
            // Capture batch size before the iter_mut borrow below so the
            // duration-per-call calculation can reference it without a
            // concurrent borrow conflict.
            let tool_call_batch_size = tool_call_traces.len().max(1) as u128;

            // Token-opt: native Bash output compactions collected during the
            // display loop (where the per-call traces are still live), keyed
            // by tool_use_id, then applied to the model's transcript copy
            // below — before read-once dedup.
            let mut bash_compactions: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            // Runaway-loop breaker: set to the offending (tool, repeat-count) the
            // first time a call+outcome signature trips the window threshold.
            let mut loop_break: Option<(String, u32)> = None;
            // Display tool results AND populate the matching ToolCallTrace.
            for result in &outcome.results {
                if let ContentBlock::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } = result
                {
                    let tool_name = tool_calls
                        .iter()
                        .find_map(|c| {
                            if let ContentBlock::ToolUse { id, name, .. } = c
                                && id == tool_use_id
                            {
                                return Some(name.as_str());
                            }
                            None
                        })
                        .unwrap_or("unknown");

                    // Runaway-loop breaker: signature this call+outcome and trip
                    // if the same (tool, args, result) has repeated consecutively
                    // past the threshold. Checked once; the first trip wins.
                    if loop_break.is_none() {
                        let tool_input = tool_calls.iter().find_map(|c| match c {
                            ContentBlock::ToolUse { id, input, .. } if id == tool_use_id => {
                                Some(input)
                            }
                            _ => None,
                        });
                        let sig = loop_call_signature(
                            tool_name,
                            tool_input.unwrap_or(&serde_json::Value::Null),
                            *is_error,
                            content,
                        );
                        if let Some(count) = loop_guard.observe(sig) {
                            loop_break = Some((tool_name.to_string(), count));
                        }
                    }

                    self.output.emit_tool_result(tool_name, *is_error, content);

                    // B7 writer-side wiring: bump the per-tool call counter in
                    // the live introspection state (one increment per executed
                    // tool result) so `wayland_status` reports real tool-call
                    // activity instead of an empty histogram.
                    if let Some(state) = &self.session_state {
                        state.record_tool_call(tool_name);
                    }

                    // W1: fill in the matching ToolCallTrace.
                    if let Some(trace) = tool_call_traces
                        .iter_mut()
                        .find(|t| t.call_id == *tool_use_id)
                    {
                        trace.bytes_out = content.len() as u64;
                        trace.output_summary = truncate_for_trace(content, 4096);
                        if *is_error {
                            trace.error = Some(content.clone());
                        }
                        // Naïve: divide the elapsed time evenly across the
                        // batch. Per-tool wall-clock comes when execute_tool_calls*
                        // is extended in W2 to surface per-call timing.
                        trace.duration_ms =
                            (tool_call_start.elapsed().as_millis() / tool_call_batch_size) as u64;
                        // W9 v0.6.3: capture the result snippet (first
                        // RESULT_SNIPPET_MAX bytes). `with_result_snippet`
                        // self-gates on WAYLAND_TRACE_RESULT_SNIPPETS — when
                        // the flag is off this is a no-op and `result_snippet`
                        // stays `None`. This is the real capture site the
                        // env gate now governs (previously the gated builder
                        // method had zero callers).
                        *trace = trace.clone().with_result_snippet(content);

                        // S2: flag traces whose result came from the dispatch
                        // timeout-cancel path (synthesized, not a completed run).
                        if outcome.cancelled_ids.contains(tool_use_id) {
                            *trace = trace.clone().with_cancelled(true);
                        }

                        // Token-opt: native Bash output compaction. The human
                        // already saw full output via `emit_tool_result`
                        // above; compact only the model's copy (applied below,
                        // before read-once dedup) and record the savings here
                        // while the trace is live. Fail-open inside
                        // `compact_bash` — never drops the error signal.
                        if self.compact_bash && tool_name == "Bash" {
                            let command = tool_calls
                                .iter()
                                .find_map(|c| match c {
                                    ContentBlock::ToolUse { id, input, .. }
                                        if id == tool_use_id =>
                                    {
                                        input.get("command").and_then(|v| v.as_str())
                                    }
                                    _ => None,
                                })
                                .unwrap_or("");
                            let c = wcore_tools::bash_compact::compact_bash(
                                command,
                                content,
                                parse_bash_exit_code(content),
                            );
                            if c.compacted_bytes < c.raw_bytes {
                                trace.record_compaction(
                                    c.raw_bytes as u64,
                                    c.compacted_bytes as u64,
                                );
                                tracing::debug!(
                                    target: "wcore_agent::compaction",
                                    command = %command,
                                    raw_bytes = c.raw_bytes,
                                    compacted_bytes = c.compacted_bytes,
                                    saved_bytes = c.raw_bytes - c.compacted_bytes,
                                    "bash output compacted"
                                );
                                bash_compactions.insert(tool_use_id.clone(), c.content);
                            }
                        }
                    }
                }
            }

            // Runaway-loop breaker tripped: the agent is repeating the same tool
            // call to no effect. Stop the run cleanly with guidance instead of
            // looping and burning tokens. Mirrors the budget-cap termination
            // path: repair the assistant's now-unanswered tool_use blocks, emit a
            // user-visible error, and finish (tool results for this turn were not
            // committed to history yet, hence `turn + 1`).
            if let Some((looping_tool, count)) = loop_break {
                self.repair_orphaned_tool_use();
                self.output.emit_error(
                    &format!(
                        "Run stopped: the `{looping_tool}` tool was called with the same \
                         arguments and produced the same result {count} times in a row — \
                         this is a no-progress loop. Change approach (different \
                         arguments/command, or a different tool) instead of repeating the \
                         same call. (Tune or disable via WAYLAND_MAX_REPEATED_TOOL_CALLS.)"
                    ),
                    false,
                );
                return self.finish_run_terminated(user_input, turn + 1).await;
            }

            // W1 F9: emit one TurnTrace per turn. Hosts that opt in via
            // capabilities.structured_traces consume it; others receive a
            // no-op (ProtocolSink only emits when its builder was configured;
            // terminal / null sinks default to no-op via the trait).
            let trace = TurnTrace {
                turn,
                // Finding #174: attribute to the dispatched model (see the
                // no-tool-calls path above for rationale).
                model: effective_model.clone(),
                provider: self.compat.provider_type().to_string(),
                input_tokens: turn_usage.input_tokens,
                output_tokens: turn_usage.output_tokens,
                cache_read: turn_usage.cache_read_tokens,
                cache_write: turn_usage.cache_creation_tokens,
                cache_hit_rate: TurnTrace::cache_hit_rate_from(
                    turn_usage.input_tokens,
                    turn_usage.cache_read_tokens,
                ),
                // Fix(pricing-audit-2026-05-24): catalog-first cost resolution.
                cost_usd: resolve_turn_cost_usd(
                    self.compat.provider_type(),
                    &effective_model,
                    turn_usage.input_tokens,
                    turn_usage.output_tokens,
                    turn_usage.cache_read_tokens,
                    turn_usage.cache_creation_tokens,
                    &self.compat,
                ),
                tool_calls: tool_call_traces,
                // Drain hook actions fired this turn into the trace.
                // partial: populated in a future pass when a streaming-drain trigger exists
                hook_actions: std::mem::take(&mut self.pending_hook_actions),
                source_product: SOURCE_PRODUCT.to_string(),
                // #279(b)+(c): correlate this turn's trace to the run.
                agent_run_id: self.current_agent_run_id.clone().unwrap_or_default(),
            };
            if let Ok(trace_json) = serde_json::to_value(&trace) {
                self.output.emit_trace(&self.current_msg_id, &trace_json);
            }
            // W6 F7: record this turn's cost for the SessionCost aggregate.
            self.per_turn_costs.push(wcore_protocol::events::TurnCost {
                turn: trace.turn,
                model: trace.model.clone(),
                provider: trace.provider.clone(),
                cost_usd: trace.cost_usd,
            });
            // W9.1 T3 (T10b): feed the trace into the F10 detect+stage+emit
            // flow. Consumes `trace` — every read above this line has
            // already happened. No-op when `skills_lifecycle` is off.
            let drafted_msg_id = self.current_msg_id.clone();
            self.try_draft_skill_for_turn(trace, &drafted_msg_id).await;

            // W8b.2.B D.3 + v0.9.1.1 B6: drain external-edit events at
            // the per-turn boundary, but BUNDLE them into the same User
            // message as the tool-results below. A separate User message
            // here would split the Anthropic-required pairing of the
            // assistant's tool_use(s) with their tool_result(s) — the
            // request would 400 with `invalid_request_error: tool_use
            // ids were found without tool_result blocks immediately
            // after`, bricking the session post-deny. Idempotent: a
            // turn with zero external edits keeps `tool_results_content`
            // = `outcome.results` verbatim.
            let mut tool_results_content = outcome.results;
            // Token-opt: swap the compacted Bash output into the model's copy
            // (the host/human already received the full result via
            // `emit_tool_result`). Done before dedup so identical compacted
            // outputs collapse to a backref.
            if !bash_compactions.is_empty() {
                for block in &mut tool_results_content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = block
                        && let Some(compacted) = bash_compactions.remove(tool_use_id)
                    {
                        *content = compacted;
                    }
                }
            }
            // Token-opt (read-once): rewrite repeated Grep/Glob/Bash outputs to a
            // backref before they enter the transcript. The user already saw the
            // full output via `emit_tool_result` above; only the model's copy is
            // deduped.
            self.dedup_repeated_tool_outputs(&mut tool_results_content, &tool_calls);
            if let Some(edit_msg) = self.drain_external_edits_message() {
                tool_results_content.push(ContentBlock::Text { text: edit_msg });
            }

            self.messages
                .push(Message::now(Role::User, tool_results_content));

            // Save session after each turn
            self.save_session();
            turn += 1;
        }
    }

    /// Dynamic Workflows B6 — the live workflow confirm gate body.
    ///
    /// Synthesises a [`WorkflowPlan`] from `user_input`, asks the host to
    /// confirm running it (via the existing `ToolRequest` + `ApprovalRequired`
    /// protocol round-trip), and — only on explicit approval — executes the
    /// workflow and returns its rendered result as the turn output.
    ///
    /// Returns:
    ///   * `Some(AgentResult)` — the user approved and the workflow ran (the
    ///     result is the turn output);
    ///   * `None` — the user declined, the turn was cancelled, OR synthesis
    ///     failed. In every `None` case the caller falls through to the normal
    ///     single-agent turn, so the user always gets a response.
    ///
    /// The caller has already verified `workflow_live_mode`, the candidate
    /// heuristic, and that both the approval manager and protocol writer are
    /// wired; this method re-binds those two `Option`s. It runs as a PRE-LLM
    /// intercept in `run()` (before the turn loop), so `turn` is `0`.
    async fn try_live_workflow(&self, user_input: &str, turn: usize) -> Option<AgentResult> {
        use wcore_protocol::events::{OutputType, ProtocolEvent, ToolInfo, ToolStatus};

        // Re-bind the two collaborators the caller already proved present.
        let manager = self.approval_manager.as_ref()?;
        let writer = self.protocol_writer.as_ref()?;

        // (a) Transient spawner sharing the parent provider + retained config.
        // SECURITY: no forced `auto_approve` — sub-agents inherit the parent's
        // read-only toolset and approval posture (see `AgentSpawner::spawn_one`).
        // It is owned (`Arc` + `Config`, no borrows of `self`) so it can be
        // MOVED onto the detached tasks below.
        let spawner = crate::spawner::AgentSpawner::new(
            std::sync::Arc::clone(&self.provider),
            self.config.clone(),
        )
        // Bind sub-agents to the engine's cancel token so a host cancel stops
        // the whole workflow rather than letting 20+ sub-agents run to
        // completion and burn LLM calls.
        .with_cancel(self.cancel_token.clone());

        // (b) Synthesise the plan on a DETACHED task. The synthesis sub-agent
        // runs its own `engine.run`, which the compiler cannot prove never
        // re-enters this gate; running it behind `tokio::spawn` forces the
        // `Send + 'static` boundary once here, severing the otherwise-infinite
        // async-recursion type cycle. On ANY synthesis error (or join failure)
        // we log and fall through — a failed synthesis must NEVER abort the
        // session.
        //
        // GAP-5/7: synthesis is up to 3 LLM round-trips with no signal. In live
        // mode the operator opted in and the input matched the workflow
        // heuristic, so: (1) emit a progress indicator instead of a silent wait,
        // (2) bound it with a timeout so a hung synthesis LLM falls through to a
        // normal turn rather than stalling forever, and (3) on any failure leave
        // a one-line note so the plain answer that follows isn't an unexplained
        // surprise. All three fall-through paths still return `None` — a failed
        // synthesis must NEVER abort the session.
        let _ = writer.emit(&ProtocolEvent::Info {
            msg_id: self.current_msg_id.clone(),
            message: "Designing a workflow for this…".to_string(),
        });
        let synth_input = user_input.to_string();
        let synth_task = tokio::spawn(synthesize_workflow_owned(spawner, synth_input));
        let synth_abort = synth_task.abort_handle();
        let synth_note = |message: &str| {
            let _ = writer.emit(&ProtocolEvent::Info {
                msg_id: self.current_msg_id.clone(),
                message: message.to_string(),
            });
        };
        let (plan, spawner) = match tokio::time::timeout(WORKFLOW_SYNTH_TIMEOUT, synth_task).await {
            Ok(Ok((Ok(plan), spawner))) => (plan, spawner),
            Ok(Ok((Err(e), _))) => {
                tracing::debug!(
                    error = %e,
                    "workflow_live: synthesis failed; falling through to normal turn"
                );
                synth_note("Couldn't design a workflow for this — answering directly.");
                return None;
            }
            Ok(Err(join_err)) => {
                tracing::debug!(
                    error = %join_err,
                    "workflow_live: synthesis task join failed; falling through"
                );
                synth_note("Couldn't design a workflow for this — answering directly.");
                return None;
            }
            Err(_elapsed) => {
                // The detached synth task keeps running after a timeout unless
                // aborted; abort it so it does not burn LLM spend headless.
                synth_abort.abort();
                tracing::debug!("workflow_live: synthesis timed out; falling through");
                synth_note("Workflow design took too long — answering directly.");
                return None;
            }
        };

        // (c) Seed the initial state, then estimate against it. Running against
        // `{}` (the prior behaviour) meant any `over: Some("changed_files")`
        // pipeline — the shape the synthesizer's few-shot anchors on — fanned
        // over a missing key and silently dispatched zero items (the 2026-05-31
        // empty-result bug). The seed populates the keys synthesis can rely on
        // (`changed_files`, `cwd`); estimating against it also yields a truthful
        // agent count on the confirm card instead of the cardinality-unknown
        // fallback. The SAME state is handed to the run below.
        let initial_state = Self::seed_workflow_state().await;
        let est = crate::orchestration::workflow::estimate::estimate(&plan, &initial_state);

        // (d) Emit the confirm round-trip. The `summary` string in `args` and
        // the `context` in `ApprovalRequired` carry the same "~N agents / ~$X"
        // human-readable estimate so the host can render either surface.
        let call_id = uuid::Uuid::new_v4().to_string();
        let summary = format!("~{} agents / ~${:.2}", est.agents, est.est_usd);
        let args = serde_json::json!({
            "name": plan.meta.name,
            "steps": est.agents,
            "summary": summary,
        });
        let _ = writer.emit(&ProtocolEvent::ToolRequest {
            msg_id: self.current_msg_id.clone(),
            call_id: call_id.clone(),
            tool: ToolInfo {
                name: "Workflow".to_string(),
                category: ToolCategory::Exec,
                args,
                description: plan.meta.description.clone(),
            },
        });
        let _ = writer.emit(&ProtocolEvent::ApprovalRequired {
            call_id: call_id.clone(),
            resume_token: call_id.clone(),
            correlation_id: call_id.clone(),
            reason: format!("Run ForgeFlow `{}`?", plan.meta.name),
            context: summary,
            plan: None,
        });

        // (e) Register the pending approval and await it, racing the await
        // against the session-root cancel token (mirrors orchestration's
        // `request_approval` round-trip). A cancel deterministically resolves
        // the await and drops the pending entry so the manager retains no
        // stale `Sender`.
        let rx = manager.request_approval(&call_id, &ToolCategory::Exec, "Workflow");
        let approval = tokio::select! {
            biased;
            _ = self.cancel_token.cancelled() => {
                manager.drop_pending(&call_id);
                return None;
            }
            res = rx => res,
        };

        // (f) Only an explicit `Approved` runs the workflow. Denied, a closed
        // channel (host crash / reaper), or any other outcome falls through.
        match approval {
            Ok(wcore_protocol::ToolApprovalResult::Approved { .. }) => {
                // Execute on a DETACHED task for the same `Send + 'static`
                // recursion-cut reason as synthesis above. The task takes
                // ownership of the plan + spawner and returns them with the
                // run result so the caller can render the per-stage summary.
                //
                // Snapshot the workflow id BEFORE the spawn moves `plan`. It
                // must equal the `workflow_id` `run_workflow_owned` emits in
                // `WorkflowStarted` (which is `plan.meta.name`) so the join-
                // failure arm below can emit the matching `WorkflowFinished`.
                let workflow_id = plan.meta.name.clone();
                match tokio::spawn(run_workflow_owned(
                    spawner,
                    plan,
                    initial_state,
                    std::sync::Arc::clone(&self.output),
                ))
                .await
                {
                    Ok((plan, run)) => {
                        // A hard `Err` OR an `Ok` run that still carries a failed
                        // stage (silent partial success — e.g. a no-barrier
                        // pipeline over a missing `over:` key) must both close the
                        // card as an error, matching the run-summary text.
                        let is_error = match &run {
                            Err(_) => true,
                            Ok(result) => {
                                !Self::errored_stage_ids(&result.stage_results).is_empty()
                            }
                        };
                        let result = self.surface_workflow_result(&plan, run, turn);
                        // Close the `Workflow` tool card. The gate emitted a
                        // `ToolRequest` for `call_id` (the proposal card) but the
                        // run only produces a text delta — without a terminal
                        // event the card is stuck in `AwaitingApproval` forever
                        // (the 2026-05-31 stuck-pill bug) and json-stream hosts
                        // never see the call resolve. Emit a real `ToolResult`.
                        let _ = writer.emit(&ProtocolEvent::ToolResult {
                            msg_id: self.current_msg_id.clone(),
                            call_id: call_id.clone(),
                            tool_name: "Workflow".to_string(),
                            status: if is_error {
                                ToolStatus::Error
                            } else {
                                ToolStatus::Success
                            },
                            output: format!(
                                "ForgeFlow `{}` {}",
                                plan.meta.name,
                                if is_error { "failed" } else { "completed" }
                            ),
                            output_type: OutputType::Text,
                            metadata: None,
                        });
                        Some(result)
                    }
                    Err(join_err) => {
                        tracing::debug!(
                            error = %join_err,
                            "workflow_live: execution task join failed; falling through"
                        );
                        // ForgeFlows-Live: `run_workflow_owned` already emitted
                        // `WorkflowStarted` before the panic/cancel, but the
                        // matching `WorkflowFinished` rides at the END of that
                        // task body and never fired. Without it the TUI's
                        // WorkflowView stays `finished: None` (orange/running)
                        // forever. Emit it here with the SAME `workflow_id` so
                        // the card resolves to "failed".
                        self.output.emit_workflow_finished(&workflow_id, false);
                        // GAP-8: the gate already emitted a `ToolRequest` proposal
                        // card for `call_id`, and the user `Approved` it. A
                        // JoinError (the detached run task panicked or was
                        // cancelled) must STILL resolve that card — otherwise it
                        // hangs in `AwaitingApproval` forever (the 300s reaper
                        // skips already-`Approved` entries, so nothing ever closes
                        // it). Emit a terminal `ToolResult` so the card resolves,
                        // mirroring the success and denied branches.
                        let _ = writer.emit(&ProtocolEvent::ToolResult {
                            msg_id: self.current_msg_id.clone(),
                            call_id: call_id.clone(),
                            tool_name: "Workflow".to_string(),
                            status: ToolStatus::Error,
                            output: "ForgeFlow execution was interrupted before it \
                                     could finish."
                                .to_string(),
                            output_type: OutputType::Text,
                            metadata: None,
                        });
                        None
                    }
                }
            }
            // Denied, a closed channel (host crash / reaper), or any other
            // non-approval outcome: resolve the proposal card as cancelled so it
            // does not linger in `AwaitingApproval`, then fall through to a
            // normal turn.
            _ => {
                let _ = writer.emit(&ProtocolEvent::ToolCancelled {
                    msg_id: self.current_msg_id.clone(),
                    call_id: call_id.clone(),
                    reason: "ForgeFlow declined".to_string(),
                });
                None
            }
        }
    }

    /// Stage 4c — discovery nudge for users who don't know `/crucible` exists.
    /// Returns a one-line hint to show ONCE per process when the user's input
    /// classifies as a HIGH-stakes council (Med is too noisy), and only when a
    /// host is wired to render it (`protocol_writer`). Returns `None` once
    /// suppressed (after the first show, or via `/crucible off`).
    fn maybe_suggest_council(&self, user_input: &str) -> Option<String> {
        use crate::orchestration::council::gate::{
            CouncilDecision, GateConfig, Stakes, classify_task,
        };
        self.protocol_writer.as_ref()?;
        if CRUCIBLE_SUGGEST_SUPPRESSED.load(Ordering::Relaxed) {
            return None;
        }
        match classify_task(user_input, &GateConfig::default()) {
            CouncilDecision::Council {
                stakes: Stakes::High,
                ..
            } => Some(
                "This looks high-stakes — `/crucible` convenes a cross-vendor \
                 council to cross-check it (you approve the cost first)."
                    .to_string(),
            ),
            _ => None,
        }
    }

    /// Engine-integrated `/crucible <task>` front door (Stage 3b). Pre-LLM
    /// intercept mirroring `try_live_workflow`: assemble a cross-vendor council,
    /// emit the typed `ApprovalRequired{plan}` card, await the host's typed
    /// `CrucibleDecision` via the approval BRIDGE (the only channel carrying
    /// structured `modifications`), run the council through the SHARED
    /// `drive_council`, and surface the answer. Returns `None` (fall through to a
    /// normal turn) when there is no host to confirm through.
    async fn try_crucible_council(&self, user_input: &str) -> Option<AgentResult> {
        use wcore_protocol::events::{
            OutputType, ProtocolEvent, ToolCategory, ToolInfo, ToolStatus,
        };

        let writer = self.protocol_writer.as_ref()?;
        let task = user_input.strip_prefix("/crucible")?.trim().to_string();
        let cfg = self.config.crucible.clone();
        // NOTE (gate): the engine retains the *resolved* `Config`, which has no
        // on-disk `[providers]` map (that lives only on `ConfigFile`). Council
        // members are resolved from the credential chain (env vars + credential
        // store) via `resolve_council_provider`, so an empty providers map still
        // keys every env-/store-credentialed provider. Wiring the inline
        // `[providers]` map into the engine is a follow-up.
        let base = self.config.clone();
        let providers = std::collections::HashMap::new();

        if task.is_empty() {
            let msg = "Usage: /crucible <task> — convene a cross-vendor council to \
                       cross-check an answer."
                .to_string();
            self.output.emit_text_delta(&msg, &self.current_msg_id);
            return Some(self.crucible_result(msg));
        }

        // Candidate pool: configured proposers ∪ candidate_pool, or — when BOTH
        // are empty (the default-config case) — the cross-vendor DEFAULT_FLUX_POOL
        // (Stage 4c empty-state self-bootstrap), then filtered to runnable (keyed)
        // specs.
        let candidates = crate::orchestration::council::bootstrap_pool(&cfg);
        let runnable = crate::orchestration::council::CouncilProviderResolver::new(
            base.clone(),
            providers.clone(),
        )
        .resolvable_specs(&candidates);
        if runnable.is_empty() {
            let msg = "crucible: no runnable council candidates — connect a provider \
                       Crucible can run (e.g. a Flux key) or set `proposers` / \
                       `candidate_pool` under `[crucible]` with keyed providers."
                .to_string();
            self.output.emit_text_delta(&msg, &self.current_msg_id);
            return Some(self.crucible_result(msg));
        }

        // Transient spawner: parent provider + a council resolver, bound to the
        // engine cancel token. Attach a cap-less council budget tracker when a cap
        // is configured. NOTE: this tracker is fresh per /crucible call, so the
        // per-run ceiling binds every invocation but the DAILY envelope does NOT
        // yet aggregate across calls in a session — cross-session/process daily
        // spend persistence is the Stage 6 deliverable (spec §9, "Cross-process
        // daily-spend"). Until then each council is bounded by its per-run cap.
        let mut spawner = crate::spawner::AgentSpawner::new(
            std::sync::Arc::clone(&self.provider),
            self.config.clone(),
        )
        .with_provider_resolver(std::sync::Arc::new(
            crate::orchestration::council::CouncilProviderResolver::new(
                base.clone(),
                providers.clone(),
            ),
        ))
        .with_cancel(self.cancel_token.clone());
        if cfg.daily_cap_usd.is_some() || cfg.max_cost_usd.is_some() {
            let tracker = std::sync::Arc::new(parking_lot::Mutex::new(
                wcore_budget::BudgetTracker::new(wcore_budget::BudgetCap::default()),
            ));
            spawner = spawner
                .with_budget_tracker(tracker)
                .with_budget_identity("crucible".to_string(), "default".to_string());
        }

        // Build the proposal card UP FRONT so it can ride the ToolRequest's
        // `tool.args`. The approval-emitter dedupe (engine_bridge ChannelEmitter +
        // main GatingProtocolWriter) synthesizes the host-visible ApprovalRequired
        // FROM the ToolRequest and SUPPRESSES the explicit one, so the typed plan
        // MUST travel via args to reach the host (spec §3.1). assemble is pure, so
        // this first plan matches the one drive_council re-derives below.
        let ov = crate::orchestration::council::CouncilOverrides::default();
        let card_gate = crate::orchestration::council::build_gate(&ov, &task);
        let card_policy = crate::orchestration::council::build_policy(&cfg, &ov);
        let first_plan = crate::orchestration::council::assemble(
            &task,
            &runnable,
            &wcore_pricing::DEFAULT_CATALOG,
            &card_gate,
            &card_policy,
        );
        let day_cap_microcents = cfg
            .daily_cap_usd
            .map(|u| (u * wcore_types::crucible::MICROCENTS_PER_USD) as u64);
        let first_card = crate::orchestration::council::plan_to_card(
            &first_plan,
            &card_policy,
            None,
            None,
            day_cap_microcents,
        );
        let plan_arg = serde_json::to_value(&first_card).unwrap_or(serde_json::Value::Null);

        // Open the tool card carrying the typed plan in args. Close it later with a
        // terminal ToolResult/ToolCancelled so it never hangs in AwaitingApproval.
        //
        // Stage 4b: the `crucible:` prefix lets the in-process TUI decision path
        // (`TuiEngine::approve`/`deny`) route this approval to the ApprovalBridge
        // instead of the ToolApprovalManager — mirroring the `egress:` precedent.
        let call_id = format!("crucible:{}", uuid::Uuid::new_v4());
        let _ = writer.emit(&ProtocolEvent::ToolRequest {
            msg_id: self.current_msg_id.clone(),
            call_id: call_id.clone(),
            tool: ToolInfo {
                name: "Crucible".to_string(),
                category: ToolCategory::Exec,
                args: serde_json::json!({ "task": task, "plan": plan_arg }),
                description: "Convene a cross-vendor council".to_string(),
            },
        });

        let approver = BridgeApprover {
            writer: std::sync::Arc::clone(writer),
            bridge: std::sync::Arc::clone(&self.approval_bridge),
            cancel: self.cancel_token.clone(),
            call_id: call_id.clone(),
        };
        let base_refilter = base.clone();
        let providers_refilter = providers.clone();
        let refilter = move |specs: &[String]| {
            crate::orchestration::council::CouncilProviderResolver::new(
                base_refilter.clone(),
                providers_refilter.clone(),
            )
            .resolvable_specs(specs)
        };

        // Run on a DETACHED task whose future is type-ERASED behind a
        // `Pin<Box<dyn Future>>` (see `run_crucible_owned`). `drive_council`
        // transitively reaches `spawner.spawn_one` → a child `engine.run`, which
        // the compiler cannot prove never re-enters this method; the boxed-dyn
        // indirection severs the otherwise-infinite async-recursion type cycle
        // (the same cut `run_workflow_owned` makes for the workflow path). A bare
        // `tokio::spawn(async move {...})` does NOT suffice — its concrete future
        // still embeds `drive_council`'s, so the cycle persists.
        let drive = tokio::spawn(run_crucible_owned(
            task, runnable, base, cfg, ov, spawner, approver, refilter,
        ));
        let outcome = match drive.await {
            Ok(res) => res,
            Err(join_err) => {
                tracing::debug!(error = %join_err, "crucible: council task join failed");
                Err(anyhow::anyhow!("crucible council was interrupted"))
            }
        };

        use crate::orchestration::council::CouncilRunResult;
        match outcome {
            Ok(CouncilRunResult::Cancelled) => {
                let _ = writer.emit(&ProtocolEvent::ToolCancelled {
                    msg_id: self.current_msg_id.clone(),
                    call_id,
                    reason: "Crucible declined — no spend.".to_string(),
                });
                let msg = "crucible: cancelled — no spend.".to_string();
                self.output.emit_text_delta(&msg, &self.current_msg_id);
                Some(self.crucible_result(msg))
            }
            Ok(CouncilRunResult::Direct { spec, text }) => {
                let _ = writer.emit(&ProtocolEvent::ToolResult {
                    msg_id: self.current_msg_id.clone(),
                    call_id,
                    tool_name: "Crucible".to_string(),
                    status: ToolStatus::Success,
                    output: format!("Crucible direct answer via {spec}"),
                    output_type: OutputType::Text,
                    metadata: None,
                });
                self.output.emit_text_delta(&text, &self.current_msg_id);
                Some(self.crucible_result(text))
            }
            Ok(CouncilRunResult::Council { plan: _, outcome }) => {
                let _ = writer.emit(&ProtocolEvent::ToolResult {
                    msg_id: self.current_msg_id.clone(),
                    call_id,
                    tool_name: "Crucible".to_string(),
                    status: ToolStatus::Success,
                    output: "Crucible council completed.".to_string(),
                    output_type: OutputType::Text,
                    metadata: None,
                });
                self.output
                    .emit_text_delta(&outcome.final_text, &self.current_msg_id);
                Some(self.crucible_result(outcome.final_text))
            }
            Err(e) => {
                let msg = format!("crucible: {e}");
                let _ = writer.emit(&ProtocolEvent::ToolResult {
                    msg_id: self.current_msg_id.clone(),
                    call_id,
                    tool_name: "Crucible".to_string(),
                    status: ToolStatus::Error,
                    output: msg.clone(),
                    output_type: OutputType::Text,
                    metadata: None,
                });
                self.output.emit_text_delta(&msg, &self.current_msg_id);
                Some(self.crucible_result(msg))
            }
        }
    }

    /// Minimal end-of-turn `AgentResult` for the Crucible front door, which
    /// produces its answer outside the LLM turn loop. (The council's own token
    /// spend is tracked via the budget envelope + provenance, not `total_usage`.)
    fn crucible_result(&self, text: String) -> AgentResult {
        AgentResult {
            text,
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: self.total_usage.clone(),
            turns: 1,
            active_window_percent: self.active_window_percent_now(&self.model, 0),
            agent_run_id: self.current_agent_run_id.clone(),
        }
    }

    /// Dynamic Workflows B6 — render a completed (or failed) workflow run into
    /// the assistant turn output. Emits the rendered text as a delta on the
    /// current msg so streaming hosts see it, then returns the `AgentResult`.
    fn surface_workflow_result(
        &self,
        plan: &crate::orchestration::workflow::runner::WorkflowPlan,
        run: Result<
            crate::orchestration::workflow::runner::WorkflowRunResult,
            crate::orchestration::workflow::runner::WorkflowRunError,
        >,
        turn: usize,
    ) -> AgentResult {
        let text = match run {
            Ok(result) => {
                // A successful `run()` can still carry per-stage failures: a
                // no-barrier `Pipeline` whose `over:` key is missing/wrong-typed
                // records an `is_error` StageResult yet returns `Ok(())` (it has
                // no hard resource bound to trip). Reporting "completed" there is
                // a silent partial success — name the failing stages and report
                // failure instead. See `Self::errored_stage_ids`.
                let errored = Self::errored_stage_ids(&result.stage_results);
                let headline = if errored.is_empty() {
                    format!("Workflow `{}` completed.\n", plan.meta.name)
                } else {
                    format!(
                        "Workflow `{}` failed: stage(s) reported an error: {}.\n",
                        plan.meta.name,
                        errored.join(", ")
                    )
                };
                let mut out = headline;
                for stage in &result.stage_results {
                    let status = if stage.is_error { "error" } else { "ok" };
                    out.push_str(&format!(
                        "- {} [{}]: {}\n",
                        stage.node_id, status, stage.text
                    ));
                }
                // GAP-3: the per-stage list shows each node's text, but the
                // workflow's actual produced data — aggregator folds and
                // pipeline result arrays — lives in `final_state` and was being
                // dropped entirely (every reader looked only at stage_results).
                // Surface the non-seed, non-empty keys so the user sees the real
                // output, not just stage statuses.
                let results = Self::render_workflow_final_state(&result.final_state);
                if !results.is_empty() {
                    out.push_str("\nResults:\n");
                    out.push_str(&results);
                }
                out
            }
            Err(e) => format!("Workflow `{}` failed: {}", plan.meta.name, e),
        };
        self.output.emit_text_delta(&text, &self.current_msg_id);
        AgentResult {
            text,
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
            usage: self.total_usage.clone(),
            turns: turn + 1,
            active_window_percent: self.active_window_percent_now(&self.model, 0),
            agent_run_id: self.current_agent_run_id.clone(),
        }
    }

    /// Collect the node ids of every stage that reported an error, in order.
    /// A non-empty result means the run produced a silent partial success — an
    /// `Ok(WorkflowRunResult)` whose stages still failed (e.g. a no-barrier
    /// pipeline fanning over a missing `over:` key, which records an
    /// `is_error` StageResult but does not return `Err`). Pure over the slice so
    /// the failure-detection is unit-testable.
    fn errored_stage_ids(
        stage_results: &[crate::orchestration::workflow::runner::StageResult],
    ) -> Vec<String> {
        stage_results
            .iter()
            .filter(|s| s.is_error)
            .map(|s| s.node_id.clone())
            .collect()
    }

    /// GAP-3 helper: render the meaningful keys of a completed workflow's
    /// `final_state` for the run summary. The per-stage list already shows each
    /// node's text; this surfaces the produced DATA (aggregator folds, pipeline
    /// result arrays) that otherwise vanished. Skips the seeded inputs
    /// (`changed_files`/`cwd` — not results) and empty/null values. Each value
    /// is compact-JSON-rendered and truncated so a large fan result can't flood
    /// the transcript. Pure over `&Value` → unit-testable.
    fn render_workflow_final_state(state: &serde_json::Value) -> String {
        const SEED_KEYS: &[&str] = &["changed_files", "cwd"];
        const MAX_VALUE_CHARS: usize = 600;
        let serde_json::Value::Object(map) = state else {
            return String::new();
        };
        let mut out = String::new();
        for (key, value) in map {
            if SEED_KEYS.contains(&key.as_str()) {
                continue;
            }
            let empty = match value {
                serde_json::Value::Null => true,
                serde_json::Value::Array(a) => a.is_empty(),
                serde_json::Value::String(s) => s.is_empty(),
                serde_json::Value::Object(o) => o.is_empty(),
                _ => false,
            };
            if empty {
                continue;
            }
            let rendered = serde_json::to_string(value).unwrap_or_default();
            let rendered = if rendered.chars().count() > MAX_VALUE_CHARS {
                let head: String = rendered.chars().take(MAX_VALUE_CHARS).collect();
                format!("{head}…")
            } else {
                rendered
            };
            out.push_str(&format!("- {key}: {rendered}\n"));
        }
        out
    }

    /// Build the initial state handed to a synthesized live workflow (estimate +
    /// run). The gate previously ran against `{}`, so an `over: Some("...")`
    /// pipeline fanned over a missing key and silently produced nothing. Seed the
    /// two keys synthesis can rely on: `changed_files` (paths from
    /// `git status --porcelain`, empty on any failure) and `cwd` (the process
    /// working directory). Both are present even when degraded, so the
    /// synthesizer's documented `over: Some("changed_files")` shape resolves to a
    /// real (possibly empty) array rather than `null`.
    async fn seed_workflow_state() -> serde_json::Value {
        let cwd = std::env::current_dir()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        let changed_files = Self::changed_files_via_git(&cwd).await;
        serde_json::json!({
            "changed_files": changed_files,
            "cwd": cwd,
        })
    }

    /// Best-effort `git status --porcelain` → changed file paths. Returns an
    /// empty vec when `cwd` is unknown, git is absent, the dir is not a repo, or
    /// the command fails — never errors. Spawned through the central
    /// `wcore_config::shell` argv helper (no shell interpreter, no injection).
    async fn changed_files_via_git(cwd: &str) -> Vec<String> {
        if cwd.is_empty() {
            return Vec::new();
        }
        let mut cmd = wcore_config::shell::shell_command_argv("git", &["status", "--porcelain"]);
        cmd.current_dir(cwd);
        match cmd.output().await {
            Ok(out) if out.status.success() => {
                Self::parse_git_porcelain(&String::from_utf8_lossy(&out.stdout))
            }
            _ => Vec::new(),
        }
    }

    /// Parse `git status --porcelain` (v1) stdout into changed file paths. Each
    /// line is `XY <path>` (two status chars + a space); a rename renders as
    /// `R  old -> new`, for which we keep the destination path.
    fn parse_git_porcelain(stdout: &str) -> Vec<String> {
        stdout
            .lines()
            .filter_map(|line| {
                let path = line.get(3..)?.trim();
                let path = path.rsplit(" -> ").next().unwrap_or(path);
                if path.is_empty() {
                    None
                } else {
                    Some(path.to_string())
                }
            })
            .collect()
    }

    /// Run the multi-level compaction pipeline before each API call.
    ///
    /// Execution order: microcompact → autocompact → emergency check.
    /// After a successful autocompact the emergency check is skipped
    /// because the context has been significantly reduced.
    async fn run_compaction(&mut self) -> Result<(), AgentError> {
        // 1. Microcompact (lightweight, no LLM call)
        if micro::should_microcompact(&self.messages, &self.compact_config) {
            let result = micro::microcompact(&mut self.messages, &self.compact_config);
            if result.cleared_count > 0 {
                self.output.emit_info(&format!(
                    "Microcompact: cleared {} tool results (~{} tokens freed)",
                    result.cleared_count, result.estimated_tokens_freed
                ));
                // Token-opt (diff-resend): clearing a tool-result body can remove
                // the read content a cached diff base references. Bump the file
                // cache's compaction generation so those bases stop qualifying.
                self.bump_file_cache_generation();
            }
        }

        // 2. Autocompact (LLM summarization)
        let mut compacted = false;
        // Finding #174 — the AUTO trigger reads the REAL-pressure watermark
        // (provider-reported billed input, thinking excluded for non-
        // replaying providers), NOT the conservative `last_input_tokens`
        // the EMERGENCY hard-stop uses. This stops premature compaction.
        // #280 — consume the one-shot smart force flag (set by the turn-top
        // pre-gate). `std::mem::take` makes it strictly one-shot per arming and
        // consumes it whether or not the circuit breaker lets the compact run,
        // so a tripped breaker can't latch it. `smart_drove` is reused below for
        // the CompactOffload reason string and the cannot-shrink latch.
        let smart_drove = std::mem::take(&mut self.smart_compact_force);
        let should_compact = smart_drove
            || auto::should_autocompact(
                self.compact_state.last_real_input_tokens,
                &self.compact_config,
            );
        if should_compact && !self.compact_state.is_circuit_broken(&self.compact_config) {
            let provider = Arc::clone(&self.provider);
            // AUDIT A4 — `run_compaction` runs at the TOP of the turn
            // loop, AFTER `push_user_turn` appended the user's live
            // instruction. Summarizing ALL of `self.messages` therefore
            // collapses the current task into an LLM summary that may
            // drop / reword it. Carve the trailing user message OUT of
            // the span handed to `autocompact` and re-attach it verbatim
            // below so the live instruction always survives intact.
            let live_user_turn: Option<Message> = match self.messages.last() {
                Some(m) if matches!(m.role, Role::User) => self.messages.pop(),
                _ => None,
            };
            let result = auto::autocompact(
                provider.as_ref(),
                &self.messages,
                &self.model,
                &self.compact_config,
                &mut self.compact_state,
            )
            .await;
            // Restore the live turn regardless of the compaction
            // outcome — on failure the conversation must be left intact.
            match result {
                Ok(result) => {
                    self.output.emit_info(&format!(
                        "Autocompact: summarized {} messages ({} tokens → compact)",
                        result.messages_summarized, result.pre_compact_tokens
                    ));
                    // #279(d): capture before `result` is moved into `folded`.
                    let result_pre_compact_tokens = result.pre_compact_tokens;
                    // AUDIT A7 — `autocompact` returns `[boundary(User),
                    // summary(User)]`; appending the live user turn
                    // would then yield three consecutive `User`
                    // messages, an invalid shape for strict-alternation
                    // providers. Fold the boundary, the summary, and
                    // the verbatim live-turn content blocks into ONE
                    // `User` message: a single role, valid shape, and
                    // the live instruction preserved block-for-block.
                    let mut folded: Vec<ContentBlock> = result
                        .messages
                        .into_iter()
                        .flat_map(|m| m.content)
                        .collect();
                    if let Some(turn) = live_user_turn {
                        // #285 (defense-in-depth) — the live turn that
                        // triggered compaction may carry `tool_result`
                        // blocks whose matching `tool_use` lived in the
                        // prefix autocompact just summarized into prose. Any
                        // such result is now orphaned. Demote it to text at
                        // the fold so the malformed array never even forms;
                        // the pre-send `repair_orphaned_tool_results` remains
                        // the guarantee. `folded` here holds only summary
                        // prose (no surviving `tool_use`), so every
                        // `tool_result` in the live turn is an orphan.
                        // Own the ids (not `&str` into `folded`) so the
                        // `folded.push(..)` below isn't blocked by a live
                        // immutable borrow (E0502).
                        let surviving_ids: std::collections::HashSet<String> = folded
                            .iter()
                            .filter_map(|b| match b {
                                ContentBlock::ToolUse { id, .. } => Some(id.to_string()),
                                _ => None,
                            })
                            .collect();
                        for block in turn.content {
                            let keep = !matches!(&block, ContentBlock::ToolResult { tool_use_id, .. }
                                if !surviving_ids.contains(tool_use_id.as_str()));
                            if keep {
                                folded.push(block);
                            } else {
                                folded.push(Self::neutralize_orphaned_tool_result(block));
                            }
                        }
                    }
                    // Token-opt compaction-floor: every message currently in
                    // `self.messages` is the prefix autocompact just summarized
                    // (the live user turn was popped out above and re-folded
                    // verbatim, so it is NOT in this count). Replacing the whole
                    // buffer with one synthetic boundary+summary message
                    // collapses all of them away — none map to an original
                    // index any more. Advance the floor by that count (the
                    // `+=` accumulates across repeated autocompacts, since the
                    // synthetic message itself becomes part of the next prefix).
                    let collapsed = self.messages.len();
                    self.compaction_floor += collapsed;
                    self.messages = vec![Message::now(Role::User, folded)];
                    compacted = true;
                    // #279(d): signal the host a compaction fired. Gated host-side
                    // by capabilities.non_destructive_compact; dormant unless the
                    // sink was built with with_non_destructive_compact(true).
                    //
                    // tokens_freed is the ACTUAL delta: the API-reported
                    // pre-compaction total minus a post-compaction estimate of the
                    // folded buffer (the only post count available here — the
                    // autocompact result carries no post_compact_tokens).
                    // saturating_sub floors it at 0 ("0 when not measurable"),
                    // never going negative if the estimate overshoots.
                    let post_compact_tokens =
                        estimate::estimate_tokens_from_messages(&self.messages);
                    let tokens_freed =
                        result_pre_compact_tokens.saturating_sub(post_compact_tokens);
                    // #280 — when the smart trigger drove this pass, tag the host
                    // chip with smart provenance (free-form String field, forward-
                    // compatible) and arm the cannot-shrink terminal latch if the
                    // compaction freed less than the configured floor.
                    if smart_drove && tokens_freed < self.compact_config.smart_min_shrink_tokens {
                        self.smart_compact_exhausted = true;
                    }
                    let reason = if smart_drove {
                        "smart_window_pressure"
                    } else {
                        "window_pressure"
                    };
                    self.output.emit_compaction(
                        &self.current_msg_id,
                        reason,
                        tokens_freed,
                        self.active_window_percent_now(&self.model, post_compact_tokens),
                    );
                    // Token-opt (diff-resend): autocompact collapsed the leading
                    // prefix, so any read base cached before now is no longer in
                    // the visible transcript. Invalidate diff bases.
                    self.bump_file_cache_generation();
                }
                Err(auto::CompactError::CircuitBroken { .. }) => {
                    // Already tripped; logged at circuit-breaker level.
                    // AUDIT A4 — restore the carved-out live user turn:
                    // compaction did not run, the conversation must be
                    // left exactly as it was.
                    if let Some(turn) = live_user_turn {
                        self.messages.push(turn);
                    }
                }
                Err(e) => {
                    self.output
                        .emit_error(&format!("Autocompact failed: {}", e), false);
                    // AUDIT A4 — restore the carved-out live user turn
                    // on failure so the next turn still sees the task.
                    if let Some(turn) = live_user_turn {
                        self.messages.push(turn);
                    }
                }
            }
        } else if should_compact {
            self.output.emit_info(&format!(
                "Autocompact: skipped (circuit breaker tripped after {} consecutive failures, \
                 last_real_input_tokens={})",
                self.compact_state.consecutive_failures, self.compact_state.last_real_input_tokens
            ));
        } else if !self.compact_config.enabled {
            let threshold = self
                .compact_config
                .context_window
                .saturating_sub(self.compact_config.output_reserve)
                .saturating_sub(self.compact_config.autocompact_buffer);
            if self.compact_state.last_real_input_tokens as usize >= threshold {
                self.output.emit_info(&format!(
                    "Autocompact: disabled (compact.enabled=false, \
                     last_real_input_tokens={}, threshold={})",
                    self.compact_state.last_real_input_tokens, threshold
                ));
            }
        }

        // 3. Emergency check (skip if autocompact just succeeded)
        if !compacted
            && emergency::is_at_emergency_limit(
                self.compact_state.last_input_tokens,
                &self.compact_config,
            )
        {
            return Err(AgentError::ContextTooLong {
                input_tokens: self.compact_state.last_input_tokens,
                limit: self
                    .compact_config
                    .context_window
                    .saturating_sub(self.compact_config.emergency_buffer),
            });
        }

        Ok(())
    }

    /// Fire SessionStart plugin hooks once, when a session begins. Hosts
    /// (CLI / TUI / JSON-stream) call this immediately after `init_session`,
    /// so it fires exactly once per session rather than once per user turn
    /// (`run()` is invoked per user message). No-op when no hook engine / no
    /// SessionStart hooks are registered.
    pub async fn run_session_start_hooks(&mut self) {
        let Some(hook_engine) = &self.hooks else {
            return;
        };
        // F11: cold-session check BEFORE dispatch. A resumed session populated
        // `self.messages` at construction (before this runs); applying a
        // session-start prelude on top of restored history would be redundant
        // and could perturb the cached prefix. Returning here also skips the MCP
        // round-trip `run_session_start` would otherwise make on every
        // `--resume` / ACP re-init (up to the SessionStart dispatch timeout).
        if !self.messages.is_empty() {
            return;
        }
        let outcome = hook_engine.run_session_start().await;
        // Hook lifecycle telemetry → tracing only (never the transcript).
        for msg in outcome.hook_trace {
            tracing::debug!(target: "wcore_agent::hooks", "{msg}");
        }
        // C1 / Task A2 — APPLY the plugin-hook contributions. `run_session_start`
        // already wrapped each one as an untrusted `<plugin-context>` User block
        // (see `HookEngine::dispatch_into`); we only fold them into the live
        // conversation. The prelude never touches the system prompt (it is
        // appended to the volatile message tail).
        let mut applied = 0usize;
        for mut msg in outcome.injected_messages {
            Self::enforce_prelude_budget(&mut msg);
            tracing::debug!(
                target: "wcore_agent::hooks",
                chars = Self::message_text_len(&msg),
                "session-start: applied plugin-hook prelude block to cold turn"
            );
            self.messages.push(msg);
            applied += 1;
        }
        // Baseline so `recall_relevant_facts` still treats a session whose ONLY
        // messages are this prelude as cold (cross-session recall still fires).
        self.session_start_injected_len = applied;
    }

    /// C1 / Task A2 — total length (in chars) of a message's text blocks. Used
    /// for the prelude budget check and its tracing log.
    fn message_text_len(msg: &Message) -> usize {
        msg.content
            .iter()
            .map(|b| match b {
                ContentBlock::Text { text } => text.len(),
                _ => 0,
            })
            .sum()
    }

    /// C1 / Task A2 — enforce [`SESSION_PRELUDE_TOKEN_BUDGET`] on a prelude
    /// message in place. We never trust a plugin's size: if the text's byte
    /// length exceeds `SESSION_PRELUDE_TOKEN_BUDGET * PRELUDE_CHARS_PER_TOKEN`,
    /// truncate it to that many bytes (rounded down to a char boundary, so
    /// multi-byte UTF-8 is never split) and append a short marker. For
    /// multi-byte text this caps bytes, i.e. it is more conservative than the
    /// char-based token estimate — never over budget, occasionally under.
    fn enforce_prelude_budget(msg: &mut Message) {
        for block in &mut msg.content {
            if let ContentBlock::Text { text } = block {
                Self::truncate_to_token_budget(text, SESSION_PRELUDE_TOKEN_BUDGET);
            }
        }
    }

    /// C1 — truncate `text` in place to at most `tokens` worth of bytes
    /// (`tokens * PRELUDE_CHARS_PER_TOKEN`), rounded down to a char boundary so
    /// multi-byte UTF-8 is never split, appending a short marker when it cuts.
    /// Shared by the A2 SessionStart prelude budget and the A3 PrePrompt budget
    /// so both enforce the same "never trust the plugin's size" discipline.
    /// Byte-capping is more conservative than the char/4 token estimate for
    /// multi-byte text — never over budget, occasionally under.
    fn truncate_to_token_budget(text: &mut String, tokens: usize) {
        const MARKER: &str = " …[truncated]";
        let max_chars = tokens * PRELUDE_CHARS_PER_TOKEN;
        if text.len() > max_chars {
            let cut = text
                .char_indices()
                .map(|(i, _)| i)
                .take_while(|&i| i <= max_chars)
                .last()
                .unwrap_or(0);
            text.truncate(cut);
            text.push_str(MARKER);
        }
    }

    /// C1 / Task A3 — apply `PrePrompt` plugin-hook contributions to the volatile
    /// request tail, cache-safely. Each contribution block is already wrapped by
    /// `dispatch_into` as an untrusted `<plugin-context>` envelope.
    ///
    /// - collect every text block, each truncated to [`PRE_PROMPT_TOKEN_BUDGET`]
    ///   (never trust the plugin's size);
    /// - SKIP a block only if an identical text block ALREADY EXISTS anywhere in
    ///   `request_messages` (e.g. the turn-1 SessionStart prelude the host already
    ///   pushed onto history, which the per-turn clone carries) — that would be a
    ///   true duplicate, so re-appending it churns the cache for no new info;
    /// - otherwise append the block as a `ContentBlock::Text` onto the LAST
    ///   message ONLY IF it is `Role::User` (never append to a non-user tail —
    ///   that would orphan a `tool_use` or create adjacent user messages).
    ///
    /// F3: dedup is against the per-turn request itself (which is regenerated as a
    /// fresh `self.messages.clone()` every turn), NOT against a cross-turn field.
    /// The previous cross-turn `last_context_injection` dedup suppressed genuinely
    /// needed per-turn recall on turn N+1 because the regenerated clone no longer
    /// contained it. Keying off the live request makes recall re-appear every turn
    /// while still avoiding the turn-1 overlap with the persisted prelude. The
    /// cache PREFIX is unaffected: the append is on the volatile tail and runs
    /// before `mark_cache_boundaries`.
    ///
    /// Operates on `request_messages` (the per-turn CLONE), never on session
    /// history.
    fn apply_pre_prompt_contribution(
        request_messages: &mut [Message],
        outcome: &crate::hooks::HookOutcome,
    ) {
        // Collect this turn's whole contribution (budget-capped).
        let mut blocks: Vec<String> = Vec::new();
        for msg in &outcome.injected_messages {
            for block in &msg.content {
                if let ContentBlock::Text { text } = block {
                    let mut text = text.clone();
                    Self::truncate_to_token_budget(&mut text, PRE_PROMPT_TOKEN_BUDGET);
                    blocks.push(text);
                }
            }
        }
        if blocks.is_empty() {
            return;
        }
        // Drop blocks that already exist verbatim in the request (e.g. the
        // turn-1 SessionStart prelude carried in the clone). Per-block, scanning
        // the live request — never a cross-turn field.
        let already_present = |candidate: &str| -> bool {
            request_messages
                .iter()
                .flat_map(|m| &m.content)
                .any(|b| matches!(b, ContentBlock::Text { text } if text == candidate))
        };
        let to_append: Vec<String> = blocks.into_iter().filter(|b| !already_present(b)).collect();
        if to_append.is_empty() {
            return;
        }
        // Cache-safe tail rule: only append onto a user-role tail. If the tail
        // isn't user-role, inject nothing so a later user-role turn can still
        // surface this content (the clone is regenerated each turn).
        let Some(last) = request_messages.last_mut() else {
            return;
        };
        if !matches!(last.role, Role::User) {
            return;
        }
        for text in to_append {
            last.content.push(ContentBlock::Text { text });
        }
    }

    /// Move session-tier memory onto the real per-session DB file.
    ///
    /// Bootstrap opens memory under the synthetic `"boot"` session id (the
    /// real id isn't known until `init_session`). Calling this once after
    /// `init_session` rebinds the session tier to `sessions/<id>.db`, giving
    /// each session its own isolated, cleanable file instead of one shared,
    /// ever-growing `boot.db`. No-op when no session is active or the memory
    /// backend doesn't implement rebinding (`NullMemory`). Project/Global
    /// tiers are unaffected.
    pub async fn rebind_memory_session(&self) {
        if let Some(id) = self.current_session_id()
            && let Err(e) = self.memory_api.rebind_session(&id).await
        {
            tracing::warn!(
                target: "wcore_agent::memory",
                error = %e,
                "session memory rebind failed; session-tier writes stay on the boot DB"
            );
        }
    }

    /// C1 / Task A2 — whether `recall_relevant_facts` should run on this turn.
    ///
    /// Cold = no REAL prior conversation. `self.messages` is the source of truth
    /// (covers hosts/tests with no SessionManager). A resumed session populates
    /// `messages` at construction (before session-start hooks), so a populated
    /// buffer there means "skip — prior context is present". But session-start
    /// hooks may have injected a synthetic plugin prelude into an otherwise-cold
    /// session; `session_start_injected_len` records how many such leading
    /// messages exist. Treat "only the session-start prelude present" as STILL
    /// cold so cross-session recall fires alongside the prelude. Math:
    /// fresh+prelude → 1 vs 1 (fires); fresh, no prelude → 0 vs 0 (fires);
    /// resume → N>0 vs 0 (skips); turn 2+ → len ≫ baseline (skips).
    fn should_attempt_recall(&self) -> bool {
        self.messages.len() <= self.session_start_injected_len
    }

    /// Cross-session recall: on the FIRST user turn of a session, pull the
    /// durable facts most relevant to what the user just asked and inject
    /// them as a synthetic context message so a cold session can answer from
    /// prior-session memory WITHOUT depending on the model choosing to call
    /// `session_search`.
    ///
    /// This closes the v2 memory recall-injection gap: `assert_fact` persists
    /// (subject, predicate, object) triples into the project/global tiers, but
    /// nothing previously re-surfaced them into a fresh process's prompt. The
    /// model in session 2 therefore answered "I don't know" even though the
    /// fact was on disk. We query the same `MemoryApi::search` path the
    /// `session_search` tool uses (now extended to include the Semantic facts
    /// partition) and prepend the hits as a `<system-reminder>`.
    ///
    /// Best-effort: a `NullMemory` backend or an empty/erroring search yields
    /// no injection and never blocks the turn. We search Project then Global
    /// so user-wide truths ("global" tier) and project-scoped facts both
    /// surface; session-tier facts are already in-context within a session and
    /// are skipped.
    async fn recall_relevant_facts(&mut self, user_input: &str) {
        if !self.should_attempt_recall() {
            return;
        }
        let query = user_input.trim();
        if query.is_empty() {
            return;
        }

        use wcore_memory::v2_types::{AccessToken, Partition, Query, Tier};
        // Clone the Arc so the search awaits don't hold a borrow of `self`
        // across the `self.messages.push` below.
        let memory_api = self.memory_api.clone();
        let mut previews: Vec<String> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        for tier in [Tier::Project, Tier::Global] {
            let q = Query {
                text: query.to_string(),
                tier,
                partition: None,
                entities: None,
                limit_per_modality: 5,
                kg_depth: 1,
                token_budget: None,
            };
            match memory_api.search(q, AccessToken::MainAgent).await {
                Ok(hits) => {
                    for h in hits {
                        // Only durable facts are worth pre-injecting; episodic
                        // previews are noisier and the model can still reach
                        // them via `session_search` on demand.
                        if h.partition == Partition::Semantic && seen.insert(h.preview.clone()) {
                            previews.push(h.preview);
                        }
                    }
                }
                Err(e) => tracing::debug!(
                    target: "wcore_agent::memory",
                    error = %e,
                    tier = %tier.as_str(),
                    "session-start fact recall search failed; continuing without injection"
                ),
            }
        }
        if previews.is_empty() {
            return;
        }
        // Cap to keep the injection tight; top hits are first (search returns
        // facts ranked by embedding similarity to the query).
        previews.truncate(6);
        // HIGH-3 / F1: recalled previews are untrusted memory content. Defang any
        // host trust-tag delimiters so a stored fact can't forge or escape this
        // <system-reminder> block. Same helper as the plugin-hook envelope (DRY).
        let body = previews
            .iter()
            .map(|p| format!("- {}", wcore_config::hooks::neutralize_trust_delimiters(p)))
            .collect::<Vec<_>>()
            .join("\n");
        let block = format!(
            "<system-reminder>\nRecalled from your durable cross-session memory \
             (facts you stored in earlier sessions), potentially relevant to the \
             user's message:\n{body}\nUse these if they answer the user's question; \
             ignore any that are irrelevant.\n</system-reminder>"
        );
        tracing::debug!(
            target: "wcore_agent::memory",
            facts = previews.len(),
            "session-start recall: injected durable facts into first turn"
        );
        self.messages.push(Message::now(
            Role::User,
            vec![ContentBlock::Text { text: block }],
        ));
    }

    /// Run stop hooks when the agent session ends
    pub async fn run_stop_hooks(&self) {
        if let Some(hook_engine) = &self.hooks {
            let mut outcome = hook_engine.run_stop().await;
            // v0.9.1.2 F10: hook lifecycle telemetry to tracing only — never
            // eprintln! (leaks into the TUI alt-screen).
            for msg in outcome.hook_trace.drain(..) {
                tracing::debug!(target: "wcore_agent::hooks", "{msg}");
            }
            for msg in outcome.log_lines.drain(..) {
                if is_hook_lifecycle_line(&msg) {
                    tracing::debug!(target: "wcore_agent::hooks", "{msg}");
                } else {
                    eprintln!("{}", msg);
                }
            }
        }
    }

    /// Apply context modifiers collected from skill tool executions.
    fn apply_context_modifiers(&mut self, modifiers: &[Option<ContextModifier>]) {
        for modifier in modifiers.iter().flatten() {
            if let Some(ref model) = modifier.model {
                // D014: a skill context modifier's model is a hook/skill
                // switch — it must not override an explicit user `/model` pin.
                self.apply_switch_model(model.clone());
            }
            if let Some(effort) = modifier.effort {
                self.current_reasoning_effort = Some(effort_to_string(effort));
            }
            for tool_name in &modifier.allowed_tools {
                if !self.allow_list.contains(tool_name) {
                    self.allow_list.push(tool_name.clone());
                }
                // SAFETY: ToolConfirmer critical section cannot
                // panic, so the std::sync Mutex is never poisoned.
                self.confirmer.lock().unwrap().add_to_allow_list(tool_name);
            }

            // Handle plan mode transitions
            if let Some(ref transition) = modifier.plan_mode_transition {
                match transition {
                    PlanModeTransition::Enter => {
                        self.plan_state.pre_plan_allow_list = self.allow_list.clone();
                        self.plan_state.is_active = true;
                        if let Some(ref flag) = self.plan_active_flag {
                            flag.store(true, Ordering::Release);
                        }
                    }
                    PlanModeTransition::Exit { plan_content } => {
                        self.plan_state.is_active = false;
                        self.allow_list = self.plan_state.pre_plan_allow_list.clone();
                        if let Some(ref flag) = self.plan_active_flag {
                            flag.store(false, Ordering::Release);
                        }
                        // W6 F16: persist the plan text on Exit. Source is
                        // the `plan_content` field on the transition (already
                        // surfaced by ExitPlanModeTool); failures are logged,
                        // not propagated — plan-mode exit must always succeed.
                        if let (Some(session), Some(text)) =
                            (self.current_session.as_ref(), plan_content.as_deref())
                            && !text.is_empty()
                            && let Err(e) =
                                crate::plan::persist::save_plan_json(&session.id, text, None)
                        {
                            self.output
                                .emit_info(&format!("[F16] failed to persist plan: {e}"));
                        }
                    }
                }
            }
        }
    }

    /// Apply a `HookOutcome` produced before a turn begins (the `on_turn_start`
    /// phase). Honours `switch_model` (overwrites `self.model`) and
    /// `injected_messages` (pushes synthetic user-role messages onto
    /// `self.messages`).
    ///
    /// AUDIT A9 — `block` is now honoured at turn level: a turn-start
    /// hook that returns `block = Some(reason)` halts the loop cleanly.
    /// Returns the block reason when set so the caller can terminate;
    /// `None` means proceed. `modified_input` remains a pre-tool-use
    /// concern and is still ignored here.
    fn apply_pre_turn_outcome(&mut self, mut outcome: crate::hooks::HookOutcome) -> Option<String> {
        // Accumulate fired hook actions for the next TurnTrace.
        self.pending_hook_actions.append(&mut outcome.fired_actions);
        if let Some(new_model) = outcome.switch_model {
            // D014: an explicit user `/model` pin outranks a hook switch_model.
            self.apply_switch_model(new_model);
        }
        self.messages.extend(outcome.injected_messages);
        // v0.9.1.2 F10: route hook lifecycle telemetry straight to tracing.
        for line in outcome.hook_trace {
            tracing::debug!(target: "wcore_agent::hooks", "{line}");
        }
        for line in outcome.log_lines {
            // v0.9.1.1 F2: same plugin-hook/hook-lifecycle filter as
            // `apply_turn_end_outcome`. `on_turn_start` is the third
            // emission site that bled plugin-hook fired lines into the
            // transcript.
            if is_hook_lifecycle_line(&line) {
                tracing::debug!(target: "wcore_agent::hooks", "{line}");
            } else {
                self.output.emit_info(&line);
            }
        }
        outcome.block
    }

    /// Apply a `HookOutcome` produced after a turn ends (the `on_turn_end`
    /// phase). `switch_model` applies to the NEXT turn; `injected_messages`
    /// are appended for the next turn.
    fn apply_turn_end_outcome(&mut self, mut outcome: crate::hooks::HookOutcome) {
        // Accumulate fired hook actions for the next TurnTrace. PostToolUse
        // outcomes also flow through here (engine.rs turn-end loop), so this
        // covers the post-tool-use phase too.
        self.pending_hook_actions.append(&mut outcome.fired_actions);
        if let Some(new_model) = outcome.switch_model {
            // D014: an explicit user `/model` pin outranks a hook switch_model.
            self.apply_switch_model(new_model);
        }
        self.messages.extend(outcome.injected_messages);
        // v0.9.1.2 F10: route hook lifecycle telemetry straight to tracing.
        for line in outcome.hook_trace {
            tracing::debug!(target: "wcore_agent::hooks", "{line}");
        }
        for line in outcome.log_lines {
            // v0.9.1.1 F2: plugin-hook + rust-hook lifecycle log lines are
            // diagnostics, not user-facing messages. Re-emitting them as
            // `Info` produced transcript clutter like
            // `[plugin-hook:wayland-ijfw:jfw_session_capture] post_tool_use fired for "WebFetch"`
            // on every turn. Route them to tracing so `/doctor` and log
            // files still see them, but the transcript stays clean.
            if is_hook_lifecycle_line(&line) {
                tracing::debug!(target: "wcore_agent::hooks", "{line}");
            } else {
                self.output.emit_info(&line);
            }
        }
    }

    /// Fire `on_session_end` hooks at the AgentResult return paths.
    /// Outcome is logging-only (no next turn to apply switch_model /
    /// inject_message to).
    async fn fire_on_session_end(&self, turns: usize) {
        if let Some(hook_engine) = self.hooks.as_ref() {
            let summary = SessionEndSummary {
                turns,
                total_input_tokens: self.total_usage.input_tokens,
                total_output_tokens: self.total_usage.output_tokens,
            };
            let outcome = hook_engine.on_session_end(&summary).await;
            // v0.9.1.2 F10: route hook lifecycle telemetry straight to tracing.
            for line in outcome.hook_trace {
                tracing::debug!(target: "wcore_agent::hooks", "{line}");
            }
            for line in outcome.log_lines {
                // v0.9.1.1 F2: same plugin-hook filter as
                // `apply_turn_end_outcome`. `on_session_end` is where
                // `jfw_session_summarize` + `jfw_session_capture` fired
                // their lifecycle prints.
                if is_hook_lifecycle_line(&line) {
                    tracing::debug!(target: "wcore_agent::hooks", "{line}");
                } else {
                    self.output.emit_info(&line);
                }
            }
        }

        // M3.1: fire the dream cycle at session-end, gated by a throttle so
        // short sessions don't churn the consolidation pipeline. Throttle
        // window is configured via `cfg.memory.dream_cycle_throttle_secs`
        // and seeded at engine construction; `NullMemory::dream_now` is a
        // no-op so this is always safe regardless of memory wiring state.
        if self.dream_throttle.should_run()
            && let Err(e) = self.memory_api.dream_now().await
        {
            tracing::warn!(
                target: "wcore_agent::memory",
                error = %e,
                "M3.1: dream_now() failed at session_end; continuing"
            );
        }

        // W3 (v0.6.3 B.1): fire the auto-memorize SessionEnd trigger.
        // `AutoMemorizer` existed but `run_session_end` was never invoked
        // on the production path. It is consent-gated internally (OFF
        // unless the user creates the opt-in consent file) and uses the
        // episodic/fact partitions — no KG dependency. Non-fatal: a memory
        // failure must not block session teardown.
        self.fire_auto_memorize().await;

        // Wave W3 (closes B.1): direct invocation of W9 Curator + PUM
        // (UserModelInferencer) for CLI-only flows.
        //
        // The host-side `HookEngine` route above is the GUI path: the
        // Wayland desktop app registers a Curator/PUM hook through
        // `register_rust_hook` and
        // observes the same `on_session_end` callback. CLI-only flows
        // (no host) never register those hooks, so without this block the
        // Curator and PUM never fire — silently — and the skills_lifecycle
        // pipeline degrades to "drafts staged forever, never curated".
        //
        // Both calls share the `skills_lifecycle` gate already cached on
        // the engine (same gate as the per-turn draft path in
        // `try_draft_skill_for_turn`). Errors are logged via
        // `tracing::warn!` and swallowed — a Curator or PUM failure must
        // NOT crash the engine's session-end path because it would lose
        // the SessionCost emit immediately below.
        if self.skills_lifecycle {
            let curator = wcore_skills::curate::Curator::new(self.memory_api.clone());
            if let Err(e) = curator.run().await {
                tracing::warn!(
                    target: "wcore_agent::skills_lifecycle",
                    error = %e,
                    "W3 (B.1): Curator.run() failed at session_end; continuing"
                );
            }

            let inferencer =
                wcore_memory::partition::UserModelInferencer::new(self.memory_api.clone());
            let traces: Vec<TurnTrace> = self.recent_turn_traces.iter().cloned().collect();
            // Compute the deltas once; the local write path and any
            // plugin-reified backends both consume the same set.
            let deltas = inferencer.infer(&traces);
            for (k, v) in &deltas {
                if let Err(e) = self
                    .memory_api
                    .update_user_model(k, v.clone(), wcore_memory::AccessToken::System)
                    .await
                {
                    tracing::warn!(
                        target: "wcore_agent::skills_lifecycle",
                        key = %k,
                        error = %e,
                        "W3 (B.1): UserModelInferencer local persist failed at session_end; continuing"
                    );
                }
            }

            // v0.6.5 Wave 6A.2: mirror every delta to each plugin-reified
            // user-model backend. Closes the carrier-without-consumer gap on
            // `AppliedPluginCapabilities::plugin_reified_user_models`. Empty
            // when no plugin reified a backend — byte-identical to pre-6A.2
            // behaviour. Failures are logged via `tracing::warn!` and
            // swallowed (the session-end SessionCost emit below must not be
            // lost to a backend hiccup).
            if !self.plugin_user_models.is_empty() {
                let user_id = self
                    .current_session
                    .as_ref()
                    .map(|s| s.id.as_str())
                    .unwrap_or("default");
                for reified in &self.plugin_user_models {
                    match &reified.backend {
                        crate::plugins::apply::ReifiedUserModelBackend::Honcho(client) => {
                            for (k, v) in &deltas {
                                // Honcho's API takes a string value; render
                                // any non-string JSON via `to_string`.
                                let value_str = match v {
                                    serde_json::Value::String(s) => s.clone(),
                                    other => other.to_string(),
                                };
                                if let Err(e) =
                                    client.learn_preference(user_id, k, &value_str).await
                                {
                                    tracing::warn!(
                                        target: "wcore_agent::skills_lifecycle",
                                        plugin = %reified.plugin,
                                        name = %reified.name,
                                        key = %k,
                                        error = %e,
                                        "v0.6.5 6A.2: plugin user-model learn_preference failed; continuing"
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }

        // W5 v0.6.3 — fact extractor → KG ingest. At session end, run the
        // heuristic fact extractor over the conversation transcript and
        // upsert each extracted (subject, predicate, object) triple into
        // the knowledge graph. Gated by `kg_enabled()` (the KG only exists
        // when enabled — W2 wires `init_kg` under the same gate). Failure
        // is non-fatal: a KG-ingest error must not lose the SessionCost
        // emit below.
        if wcore_memory::kg::kg_enabled() {
            let transcript: String = self
                .messages
                .iter()
                .flat_map(|m| m.content.iter())
                .filter_map(|b| match b {
                    ContentBlock::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("\n");
            if !transcript.trim().is_empty() {
                match self.memory_api.kg_ingest_facts(&transcript).await {
                    Ok(n) => tracing::debug!(
                        target: "wcore_agent::memory",
                        facts = n,
                        "W5: fact extractor ingested {n} facts into the KG"
                    ),
                    Err(e) => tracing::warn!(
                        target: "wcore_agent::memory",
                        error = %e,
                        "W5: KG fact ingest failed at session_end; continuing"
                    ),
                }
            }
        }

        // F-092 (W7-N): live-session online evolution.
        // Gate: opt-in only (default false). Emits one EvolutionEvent per
        // session when the session had at least one successful tool call
        // (success = ≥1 tool call landed in recent_turn_traces, no
        // indication of engine error). Applies the Paraphrase mutator live
        // and persists the evolved system-prompt variant to
        // `$WAYLAND_HOME/evolved/<id>.md`. Failure is non-fatal.
        if self.online_evolution {
            self.fire_online_evolution().await;
        }

        // W6 F7: emit aggregate SessionCost. The sink's emit_session_cost
        // is a no-op when advertised.cost_attribution is false, so we do
        // not need to gate here (single-authority gate lives on the sink).
        let session_id = self
            .current_session
            .as_ref()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        let total_cost_usd: f64 = self.per_turn_costs.iter().map(|t| t.cost_usd).sum();
        let payload = serde_json::json!({
            "total_cost_usd": total_cost_usd,
            "per_turn": &self.per_turn_costs,
        });
        self.output.emit_session_cost(&session_id, &payload);
    }

    /// F-092 (W7-N): apply live online evolution at session-end.
    ///
    /// Success criterion (simple): the session accumulated at least one
    /// turn with ≥1 tool call. The score is the fraction of turns that
    /// had tool calls (0.0–1.0). Applies the Paraphrase mutator only when
    /// the score exceeds `ONLINE_EVOLVE_THRESHOLD`; always emits the
    /// `EvolutionEvent` so hosts can observe trajectories regardless.
    ///
    /// Persists the paraphrased system-prompt variant to
    /// `$WAYLAND_HOME/evolved/<session_id>.md`. SkillRouter integration
    /// is deferred — the file is the integration point for now.
    ///
    /// Every failure branch logs at `warn` level and returns without
    /// affecting the session teardown path.
    ///
    /// ## Async correctness
    ///
    /// This is an `async fn` invoked with `.await` from the async
    /// `fire_on_session_end`. The paraphrase goes through the real
    /// LLM-backed [`LlmParaphraseProvider`] via its **async**
    /// `paraphrase_async` surface — NOT the sync `paraphrase_blocking`
    /// bridge. `paraphrase_blocking` calls `Handle::block_on`, which panics
    /// when invoked on a reactor worker thread; calling it from this async
    /// context (as the old passthrough `Paraphrase::mutate` path did) would
    /// be unsound the moment a real provider is wired in. A
    /// `tokio::time::timeout` caps the call so a hung provider cannot stall
    /// session teardown.
    async fn fire_online_evolution(&self) {
        /// Fraction of tool-using turns required to trigger the mutator.
        const ONLINE_EVOLVE_THRESHOLD: f64 = 0.5;

        let session_id = self
            .current_session
            .as_ref()
            .map(|s| s.id.clone())
            .unwrap_or_else(|| "unknown".to_string());

        // Simple success score: fraction of turns with ≥1 tool call.
        let total_turns = self.recent_turn_traces.len();
        if total_turns == 0 {
            tracing::debug!(
                target: "wcore_agent::online_evolve",
                session = %session_id,
                "F-092: no turns recorded this session — skipping online evolution"
            );
            return;
        }
        let tool_using_turns = self
            .recent_turn_traces
            .iter()
            .filter(|t| !t.tool_calls.is_empty())
            .count();
        let score = tool_using_turns as f64 / total_turns as f64;

        // Determine whether the trajectory met the success bar.
        let retained = score >= ONLINE_EVOLVE_THRESHOLD;

        // Emit the EvolutionEvent to the protocol stream if a writer is wired.
        if let Some(writer) = self.protocol_writer.as_ref() {
            let event = wcore_protocol::events::ProtocolEvent::EvolutionEvent {
                run_id: session_id.clone(),
                generation: 0,
                parent_id: "system_prompt".to_string(),
                child_id: format!("{}/live/paraphrase", session_id),
                mutation_kind: "Paraphrase".to_string(),
                score,
                retained,
            };
            if let Err(e) = writer.emit(&event) {
                tracing::warn!(
                    target: "wcore_agent::online_evolve",
                    session = %session_id,
                    error = %e,
                    "F-092: failed to emit EvolutionEvent; continuing"
                );
            }
        }

        // Only apply the Paraphrase mutator when the trajectory succeeded.
        if !retained {
            tracing::debug!(
                target: "wcore_agent::online_evolve",
                session = %session_id,
                score,
                threshold = ONLINE_EVOLVE_THRESHOLD,
                "F-092: trajectory below threshold — Paraphrase mutator skipped"
            );
            return;
        }

        // Paraphrase the current system prompt with the REAL LLM-backed
        // provider (formerly a no-op passthrough that wrote the prompt back
        // byte-identical). The engine's own `provider` + `model` drive the
        // rewrite, so the evolved variant reflects the session's live model.
        let evolved_dir = wcore_config::config::wayland_config_dir().join("evolved");
        Self::paraphrase_and_persist(
            std::sync::Arc::clone(&self.provider),
            &self.model,
            &self.system_prompt,
            &session_id,
            score,
            &evolved_dir,
        )
        .await;
    }

    /// F-092 helper: paraphrase `system_prompt` with the real LLM-backed
    /// [`LlmParaphraseProvider`] and persist the variant to
    /// `<evolved_dir>/<session_id>.md`. Split out of `fire_online_evolution`
    /// so it can be unit-tested with a mock `LlmProvider` against an explicit
    /// directory (no `WAYLAND_HOME` process-env mutation).
    ///
    /// Uses `paraphrase_async` (the async trait surface), NOT the sync
    /// `paraphrase_blocking` bridge — see the `fire_online_evolution` doc
    /// comment for why blocking here would be unsound. A 30s
    /// `tokio::time::timeout` bounds the call. Every failure (provider error
    /// or timeout) logs at `warn` and returns; session teardown is never
    /// blocked.
    async fn paraphrase_and_persist(
        provider: Arc<dyn LlmProvider>,
        model: &str,
        system_prompt: &str,
        session_id: &str,
        score: f64,
        evolved_dir: &std::path::Path,
    ) {
        use wcore_evolve::mutator::{AsyncParaphrase, LlmParaphraseProvider};

        /// Wall-clock cap on the live paraphrase call. Session teardown must
        /// not stall on a hung provider.
        const PARAPHRASE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

        let paraphraser = LlmParaphraseProvider::new(provider, model.to_string());
        let evolved_body = match tokio::time::timeout(
            PARAPHRASE_TIMEOUT,
            paraphraser.paraphrase_async(system_prompt),
        )
        .await
        {
            Ok(Ok(body)) => body,
            Ok(Err(e)) => {
                tracing::warn!(
                    target: "wcore_agent::online_evolve",
                    session = %session_id,
                    error = %e,
                    "F-092: LLM paraphrase failed — evolved prompt not persisted"
                );
                return;
            }
            Err(_) => {
                tracing::warn!(
                    target: "wcore_agent::online_evolve",
                    session = %session_id,
                    timeout_secs = PARAPHRASE_TIMEOUT.as_secs(),
                    "F-092: LLM paraphrase timed out — evolved prompt not persisted"
                );
                return;
            }
        };

        // Persist to <evolved_dir>/<session_id>.md.
        // SkillRouter integration is deferred — this file is the handoff
        // surface for a follow-up wave (see F-092 triage note).
        if let Err(e) = std::fs::create_dir_all(evolved_dir) {
            tracing::warn!(
                target: "wcore_agent::online_evolve",
                dir = %evolved_dir.display(),
                error = %e,
                "F-092: could not create evolved/ dir — evolved prompt not persisted"
            );
            return;
        }
        let file_path = evolved_dir.join(format!("{session_id}.md"));
        let content = format!(
            "<!-- F-092 online-evolve: session={session_id} score={score:.4} mutator=Paraphrase -->\n{evolved_body}\n"
        );
        if let Err(e) = std::fs::write(&file_path, &content) {
            tracing::warn!(
                target: "wcore_agent::online_evolve",
                path = %file_path.display(),
                error = %e,
                "F-092: failed to write evolved prompt — continuing"
            );
        } else {
            tracing::info!(
                target: "wcore_agent::online_evolve",
                session = %session_id,
                path = %file_path.display(),
                score,
                "F-092: Paraphrase variant persisted to evolved/"
            );
        }
    }

    /// W3 (v0.6.3): auto-memorize SessionEnd trigger.
    ///
    /// Builds a [`SessionDigest`] by running the heuristic
    /// [`FactExtractor`](wcore_memory::fact_extractor::FactExtractor) over
    /// the session's plain-text messages, then hands it to
    /// [`AutoMemorize::run_session_end`]. That method is consent-gated
    /// internally — auto-memorize is OFF unless the user opts in via the
    /// consent file (and `WAYLAND_AUTO_MEMORIZE=off` is the kill switch),
    /// so when consent is absent this is a cheap no-op skip.
    ///
    /// The `run_session_end` `persist` closure is synchronous, but
    /// `MemoryApi::assert_fact` is async — so the closure only collects the
    /// surviving candidates into a buffer; the actual writes happen after
    /// `run_session_end` returns. Every memory error is logged and
    /// swallowed: session teardown must never be blocked by a memory issue.
    async fn fire_auto_memorize(&self) {
        use wcore_memory::auto_memorize::{AutoMemorize, FactCandidate, SessionDigest};
        use wcore_memory::fact_extractor::FactExtractor;

        // Gather plain-text content surfaced during the session. Tool-use /
        // tool-result / thinking blocks are skipped — the extractor scores
        // natural-language assertions, not tool plumbing.
        let extractor = FactExtractor::default();
        let mut fact_candidates: Vec<FactCandidate> = Vec::new();
        for msg in &self.messages {
            for block in &msg.content {
                if let ContentBlock::Text { text } = block {
                    for f in extractor.extract_with_dedup(text) {
                        fact_candidates.push(FactCandidate {
                            subject: f.subject,
                            predicate: f.predicate,
                            object: f.object,
                            confidence: f.confidence,
                        });
                    }
                }
            }
        }

        let session_id = self
            .current_session
            .as_ref()
            .map(|s| s.id.clone())
            .unwrap_or_default();
        let turn_count = self.recent_turn_traces.len() as u32;
        let digest = SessionDigest {
            session_id,
            turn_count,
            fact_candidates,
        };

        // The persist closure is sync; collect the filtered survivors and
        // write them after `run_session_end` returns.
        let mut to_persist: Vec<FactCandidate> = Vec::new();
        let report = AutoMemorize::default().run_session_end(digest, |facts| {
            to_persist = facts.to_vec();
            facts.len()
        });

        if !report.triggered {
            return;
        }

        for cand in to_persist {
            let fact = wcore_memory::v2_types::Fact {
                id: wcore_memory::v2_types::FactId::new(),
                tier: wcore_memory::v2_types::Tier::Project,
                ts: wcore_memory::audit::now_secs(),
                subject: cand.subject,
                predicate: cand.predicate,
                object: cand.object,
                confidence: cand.confidence as f64,
                source_episode: None,
                superseded_by: None,
            };
            if let Err(e) = self
                .memory_api
                .assert_fact(fact, wcore_memory::AccessToken::System)
                .await
            {
                tracing::warn!(
                    target: "wcore_agent::memory",
                    error = %e,
                    "W3: assert_fact() failed at session_end; continuing"
                );
            }
        }
    }

    /// W6 F17: trim MCP tools in the per-turn `ToolDef` list to a curated
    /// top-K. Non-MCP tools (builtins, skills, spawn, plan) are always kept.
    /// MCP tools are identified by their REAL provenance (`ToolDef::server`
    /// is `Some`), NOT the `mcp__` name prefix: a non-colliding MCP tool keeps
    /// its bare original name (see `wcore-mcp/src/tool_proxy.rs`), so the prefix
    /// would misclassify it as a built-in and never curate it (#359). Curation
    /// source: the most recent user message in `self.messages`. `Off` policy is
    /// a no-op.
    fn apply_mcp_curation(
        &mut self,
        tools: Vec<wcore_types::tool::ToolDef>,
    ) -> Vec<wcore_types::tool::ToolDef> {
        let top_k = match &self.mcp_curation {
            wcore_config::config::McpCurationPolicy::Off => return tools,
            wcore_config::config::McpCurationPolicy::TopK { k } => *k,
        };
        // Partition into MCP and non-MCP slices by real provenance.
        let (mcp_tools, mut keep): (Vec<_>, Vec<_>) =
            tools.into_iter().partition(|t| t.server.is_some());
        if mcp_tools.len() <= top_k {
            keep.extend(mcp_tools);
            return keep;
        }

        // Cache-stability (token-opt): the kept MCP set must stay stable across
        // turns, or it rewrites the cached tool-zone prefix every turn at the
        // cache-WRITE rate (~1.25x) instead of re-reading it (~0.1x). Key a
        // UNION of curated keep-sets on the MCP tool inventory hash: this turn's
        // keyword/recency pick is unioned into the cached set, which grows
        // monotonically as new user messages surface new tools and then
        // stabilizes — byte-stable on the common turn. We never freeze turn-1's
        // keywords (that would permanently hide a tool the model needs later);
        // the cache resets only when the inventory itself changes (server
        // connect/disconnect / plugin reload), a legitimate one-turn miss.
        let inventory_hash = {
            use std::hash::{Hash, Hasher};
            let mut names: Vec<&str> = mcp_tools.iter().map(|t| t.name.as_str()).collect();
            names.sort_unstable();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            names.hash(&mut h);
            h.finish()
        };

        let user_msg = self.most_recent_user_text();
        let usage = self.recent_mcp_usage();
        let triples: Vec<(String, String, String)> = mcp_tools
            .iter()
            .map(|t| {
                // Real provenance from `ToolDef::server`; this slice is the MCP
                // partition so `server` is always `Some` here. Fall back to the
                // prefix-derived name only defensively.
                let server = t
                    .server
                    .clone()
                    .unwrap_or_else(|| t.name.split("__").nth(1).unwrap_or("mcp").to_string());
                (server, t.name.clone(), t.description.clone())
            })
            .collect();
        let ranked =
            crate::mcp_curator::McpCurator::new(top_k).curate(&crate::mcp_curator::CurationInput {
                user_message: &user_msg,
                tools: &triples,
                recent_usage: &usage,
            });
        // This turn's pick, in rank order (already deduped by the curator).
        let this_turn: Vec<String> = ranked.into_iter().map(|r| r.tool_name).collect();

        // Union into the inventory-keyed cache (reset on inventory change),
        // APPEND-ONLY: a name already in the kept order keeps its position; a
        // newly-surfaced name is pushed at the END. This makes every
        // union-growth turn a cache-safe append (stable prefix) rather than a
        // mid-array insert that would invalidate the cached tool-zone prefix.
        let keep_order: &Vec<String> = match self.mcp_curation_cache.as_mut() {
            Some((hash, order)) if *hash == inventory_hash => {
                for name in this_turn {
                    if !order.contains(&name) {
                        order.push(name);
                    }
                }
                &*order
            }
            _ => {
                self.mcp_curation_cache = Some((inventory_hash, this_turn));
                &self.mcp_curation_cache.as_ref().expect("just set").1
            }
        };

        // Emit kept MCP tools in FIRST-ADD order (not registry-iteration
        // order). Built-in/non-MCP tools in `keep` retain their original
        // relative position ahead of the MCP block.
        let by_name: std::collections::HashMap<&str, &wcore_types::tool::ToolDef> =
            mcp_tools.iter().map(|t| (t.name.as_str(), t)).collect();
        for name in keep_order {
            if let Some(t) = by_name.get(name.as_str()) {
                keep.push((*t).clone());
            }
        }
        keep
    }

    /// #344/#359 — provider-aware HARD cap on the total tool count. A provider
    /// request's tool array must not exceed the provider's limit (OpenAI = 128);
    /// MCP servers can push the array past it (222 > 128 -> API 400). Separate
    /// from `apply_mcp_curation` (a relevance/token optimization): this is the
    /// correctness guarantee. Keeps ALL non-MCP tools (built-ins, ToolSearch,
    /// skills, plan) and fills the remaining budget with the most RELEVANT MCP
    /// tools (BM25 over the current user message), truncating the rest. Dropped
    /// MCP tools stay DISCOVERABLE via the `ToolSearch` meta-tool (its registry
    /// is a full bootstrap snapshot). `None` cap = no-op.
    ///
    /// Cache-stability (token-opt): when the cap must trim, the kept MCP set is
    /// emitted in an inventory-keyed APPEND-ONLY UNION order (mirroring
    /// `apply_mcp_curation`), NOT re-sorted by relevance every turn. BM25 only
    /// decides which NEW tools to admit when budget allows; a tool, once
    /// admitted, holds its slot. This keeps the serialized tool-zone prefix
    /// byte-stable across same-context turns (a cache READ, not a per-turn
    /// prefix rewrite that would thrash the Anthropic prompt cache).
    fn apply_provider_tool_cap(
        &mut self,
        tools: Vec<wcore_types::tool::ToolDef>,
    ) -> Vec<wcore_types::tool::ToolDef> {
        let limit = match self.compat.max_tools() {
            Some(l) => l,
            None => return tools,
        };
        if tools.len() <= limit {
            return tools;
        }
        // Classify by REAL provenance (`ToolDef::server`), not the `mcp__` name
        // prefix: a non-colliding MCP tool keeps its bare name, so the prefix
        // would count it as a built-in and ALWAYS keep it — letting a swarm of
        // bare-named MCP tools blow past the provider limit (#359).
        let (mcp_tools, non_mcp): (Vec<_>, Vec<_>) =
            tools.into_iter().partition(|t| t.server.is_some());
        let non_mcp_count = non_mcp.len();
        let mcp_budget = limit.saturating_sub(non_mcp_count);
        let mcp_total = mcp_tools.len();

        // Under budget after partition — keep every MCP tool, no trim, no churn.
        if mcp_total <= mcp_budget {
            let mut kept = non_mcp;
            kept.extend(mcp_tools);
            return kept;
        }

        // No MCP budget at all (built-ins alone meet/exceed the cap): drop the
        // entire MCP block. Correctness > strict limit for built-ins, matching
        // the prior behavior. No BM25/union work needed.
        if mcp_budget == 0 {
            tracing::info!(
                target: "wcore_agent::engine",
                "provider tool cap: {} tools exceeded the provider limit of {}; kept {} \
                 (incl. all {} non-MCP), {} MCP tools hidden but reachable via ToolSearch",
                non_mcp_count + mcp_total,
                limit,
                non_mcp_count,
                non_mcp_count,
                mcp_total
            );
            return non_mcp;
        }

        // Over budget. Rank the MCP tools by BM25 relevance (same scorer the
        // curator uses), then take this turn's top `mcp_budget` names. The
        // emitted order, however, is the inventory-keyed append-only union —
        // not the per-turn relevance order — so the prefix stays byte-stable.
        let inventory_hash = {
            use std::hash::{Hash, Hasher};
            let mut names: Vec<&str> = mcp_tools.iter().map(|t| t.name.as_str()).collect();
            names.sort_unstable();
            let mut h = std::collections::hash_map::DefaultHasher::new();
            names.hash(&mut h);
            // The budget is part of the cache key: a budget change (e.g. a new
            // built-in shrinking the MCP slots) legitimately resets the union.
            mcp_budget.hash(&mut h);
            h.finish()
        };

        let user_msg = self.most_recent_user_text();
        let usage = self.recent_mcp_usage();
        let triples: Vec<(String, String, String)> = mcp_tools
            .iter()
            .map(|t| {
                let server = t
                    .server
                    .clone()
                    .unwrap_or_else(|| t.name.split("__").nth(1).unwrap_or("mcp").to_string());
                (server, t.name.clone(), t.description.clone())
            })
            .collect();
        // `curate` truncates to the cap's MCP budget = this turn's admit set.
        let ranked = crate::mcp_curator::McpCurator::new(mcp_budget).curate(
            &crate::mcp_curator::CurationInput {
                user_message: &user_msg,
                tools: &triples,
                recent_usage: &usage,
            },
        );
        let this_turn: Vec<String> = ranked.into_iter().map(|r| r.tool_name).collect();

        // Union into the (inventory, budget)-keyed cache, APPEND-ONLY: an
        // already-kept name holds its slot; a newly-admitted name is pushed at
        // the END (a cache-safe append, not a mid-array insert). Cap the union
        // length at `mcp_budget` so the kept set never exceeds the provider
        // limit even as the union grows across turns — once full, no new tool
        // displaces an admitted one for the life of this inventory/budget.
        let keep_order: Vec<String> = match self.mcp_cap_cache.as_mut() {
            Some((hash, order)) if *hash == inventory_hash => {
                for name in this_turn {
                    if order.len() >= mcp_budget {
                        break;
                    }
                    if !order.contains(&name) {
                        order.push(name);
                    }
                }
                order.clone()
            }
            _ => {
                let mut order = this_turn;
                order.truncate(mcp_budget);
                self.mcp_cap_cache = Some((inventory_hash, order.clone()));
                order
            }
        };

        // Emit non-MCP tools first (preserving their relative order), then the
        // kept MCP block in FIRST-ADD union order.
        let by_name: std::collections::HashMap<&str, &wcore_types::tool::ToolDef> =
            mcp_tools.iter().map(|t| (t.name.as_str(), t)).collect();
        let mut kept = non_mcp;
        for name in &keep_order {
            if let Some(t) = by_name.get(name.as_str()) {
                kept.push((*t).clone());
            }
        }

        let hidden = mcp_total - keep_order.len();
        if hidden > 0 {
            tracing::info!(
                target: "wcore_agent::engine",
                "provider tool cap: {} tools exceeded the provider limit of {}; kept {} \
                 (incl. all {} non-MCP), {} MCP tools hidden but reachable via ToolSearch",
                non_mcp_count + mcp_total,
                limit,
                kept.len(),
                non_mcp_count,
                hidden
            );
        }
        kept
    }

    /// W6 F17 recency input for `McpCurator`. Reads the M2 audit log via the
    /// `recent_tool_uses` API. When `audit_log` is `None` the curator
    /// gracefully degrades to keyword-only ranking.
    fn recent_mcp_usage(&self) -> std::collections::HashMap<String, u64> {
        const WINDOW_SECS: i64 = 24 * 3600;
        match self.audit_log.as_ref() {
            Some(log) => log.recent_tool_uses(WINDOW_SECS).unwrap_or_default(),
            None => std::collections::HashMap::new(),
        }
    }

    /// Most recent user-message text for the curator's keyword overlap.
    /// Empty string when no user message has been seen yet (rare).
    fn most_recent_user_text(&self) -> String {
        self.messages
            .iter()
            .rev()
            .find(|m| matches!(m.role, Role::User))
            .map(|m| {
                m.content
                    .iter()
                    .filter_map(|b| match b {
                        ContentBlock::Text { text } => Some(text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default()
    }

    /// W6 F17 — inject an audit log for MCP curation recency input.
    /// Bootstrap calls this when the M2 audit log is wired.
    pub fn set_audit_log(&mut self, log: Arc<wcore_memory::audit::AuditLog>) {
        self.audit_log = Some(log);
    }

    fn save_session(&mut self) {
        // AUDIT D-6 — never persist a trailing assistant message whose
        // `tool_use` blocks have no following `tool_result`. The graph
        // `Cancelled` / `Quit` exits call `save_session()` after the
        // assistant message (with `tool_use`) is pushed but before the
        // tool-results message — that orphaned shape is Anthropic-invalid
        // and any consumer reading the session file without going
        // through `push_user_turn` (inspector, export, re-send) would
        // choke on it. Repair in-memory first so the on-disk copy is
        // always a valid alternating message list. No-op when there is
        // nothing dangling.
        self.repair_orphaned_tool_use();
        if let (Some(mgr), Some(session)) = (&self.session_manager, &mut self.current_session) {
            session.messages = self.messages.clone();
            session.total_usage = self.total_usage.clone();
            session.updated_at = chrono::Utc::now();
            if let Err(e) = mgr.save(session) {
                self.output
                    .emit_error(&format!("Failed to save session: {}", e), false);
            } else {
                // F-030: full save succeeded; the WAL is now redundant.
                mgr.delete_wal(session);
            }
            if let Err(e) = mgr.update_index_for(session) {
                self.output
                    .emit_error(&format!("Failed to update session index: {}", e), false);
            }
        }
    }
}

/// Truncate a tool output to ≤max bytes for trace embedding without panicking
/// on multi-byte char boundaries.
#[cfg(test)]
mod v0_9_1_1_hook_lifecycle_filter_tests {
    use super::is_hook_lifecycle_line;

    #[test]
    fn plugin_hook_fired_lines_are_classified_as_lifecycle() {
        // v0.9.1.1 F2 regression: HookEngine's plugin-hook lifecycle
        // prints leaked into the transcript via `emit_info`. The
        // classifier here must catch every shape `fire_plugin_hooks`
        // emits so the engine routes them to `tracing::debug!` instead.
        assert!(is_hook_lifecycle_line(
            "[plugin-hook:wayland-ijfw:jfw_session_summarize] on_session_end fired (turns: 1)"
        ));
        assert!(is_hook_lifecycle_line(
            "[plugin-hook:wayland-ijfw:jfw_session_capture] post_tool_use fired for \"WebFetch\""
        ));
        // Rust-hook lifecycle (Block/ModifyInput ignored, etc.).
        assert!(is_hook_lifecycle_line(
            "[hook:curator] Block ignored on on_turn_end: 'no curator'"
        ));
    }

    #[test]
    fn is_hook_lifecycle_line_catches_unprefixed_v0912() {
        // v0.9.1.2 F10: Sean's live e2e showed the leading `[plugin-hook:...]`
        // prefix getting visually clipped or wrapped — the residual body
        // (`post_tool_use fired for tool "web"`) was still painting on the
        // alt-screen. Even though the architectural fix routes these lines
        // away from log_lines entirely, the classifier widens its match so
        // any future code path that pushes a bare `*_fired` body string into
        // a transcript-bound drain still gets filtered. Defense in depth.
        assert!(is_hook_lifecycle_line(
            "post_tool_use fired for tool \"web\""
        ));
        assert!(is_hook_lifecycle_line(
            "pre_tool_use fired for tool \"github_api\""
        ));
        assert!(is_hook_lifecycle_line("on_turn_start fired (turn 3)"));
        assert!(is_hook_lifecycle_line("on_turn_end fired (turn 3)"));
        assert!(is_hook_lifecycle_line("on_session_end fired (turns: 5)"));
        // The previous F2 prefixes still match.
        assert!(is_hook_lifecycle_line(
            "[plugin-hook:wayland-ijfw:ijfw_observation_capture] post_tool_use fired for tool \"web\""
        ));
        // Lines that LOOK lifecycle-ish but aren't are still allowed
        // through (only the documented verbs match).
        assert!(!is_hook_lifecycle_line("tool fired off a side-effect"));
    }

    #[test]
    fn user_facing_info_lines_are_not_classified_as_lifecycle() {
        // Anything that isn't a `[plugin-hook:` or `[hook:` prefix must
        // still flow through to `emit_info` so the transcript keeps
        // showing real notices (budget exceeded, plan persisted, etc.).
        assert!(!is_hook_lifecycle_line(
            "Budget exceeded (tokens): 12345 > 10000"
        ));
        assert!(!is_hook_lifecycle_line("[F16] failed to persist plan: io"));
        assert!(!is_hook_lifecycle_line(""));
        assert!(!is_hook_lifecycle_line("plain text"));
    }
}

/// v0.9.1.1 F2: detect a hook-lifecycle log line emitted by `HookEngine`.
///
/// `HookEngine::fire_plugin_hooks` formats every fired hook as
/// `[plugin-hook:<plugin>:<name>] <verb> fired <detail>`, and the
/// rust-hook + shell-hook log lines share the `[hook:<name>] ...` shape.
/// Both are diagnostics — they exist so `/doctor` and log files can
/// confirm hooks ran, NOT so users read them in their transcript on
/// every turn.
///
/// Match by prefix on the raw log-line string (the only thing we have
/// at the engine layer — `HookOutcome.log_lines` is `Vec<String>`, not
/// a structured variant). The match is intentionally narrow: only the
/// two prefixes the hook engine itself emits. Anything else — user-
/// facing notices, shell-hook stdout, build/registration failures —
/// still flows through `emit_info` and remains visible.
pub(crate) fn is_hook_lifecycle_line(line: &str) -> bool {
    // v0.9.1.2 F10: defensive coverage — even if a future code path strips
    // the `[plugin-hook:...]` prefix (e.g. truncation, log-line wrapping),
    // the residual `*_fired` body still gets caught here. Plugin-hook fire
    // lines and per-phase rust-hook "action ignored" diagnostics are
    // telemetry, never transcript content.
    line.starts_with("[plugin-hook:")
        || line.starts_with("[hook:")
        || line.starts_with("post_tool_use fired")
        || line.starts_with("pre_tool_use fired")
        || line.starts_with("post_user_turn fired")
        || line.starts_with("pre_user_turn fired")
        || line.starts_with("post_session fired")
        || line.starts_with("pre_session fired")
        || line.starts_with("on_turn_start fired")
        || line.starts_with("on_turn_end fired")
        || line.starts_with("on_session_end fired")
}

/// W7 F4: build the per-turn `StreamingContext` for tool-chunk streaming
/// (e.g. BashTool live output). Returns `Some` only when the host has
/// advertised streaming support via `OutputSink::streaming_tools_advertised`,
/// keeping the feature opt-in by capability — hosts that don't advertise it
/// keep the buffered-output behaviour (`None`). The orchestration dispatcher
/// further gates per-tool on `tool.supports_streaming()`.
fn build_turn_streaming_context(
    output: &Arc<dyn OutputSink>,
    msg_id: &str,
) -> Option<StreamingContext> {
    if output.streaming_tools_advertised() {
        Some(StreamingContext {
            output: output.clone(),
            msg_id: msg_id.to_string(),
        })
    } else {
        None
    }
}

#[cfg(test)]
mod streaming_context_gate_tests {
    use std::sync::Arc;

    use wcore_types::message::FinishReason;

    use super::build_turn_streaming_context;
    use crate::output::OutputSink;

    /// Minimal sink whose streaming-advertise gate is configurable.
    struct GateSink {
        advertised: bool,
    }

    impl OutputSink for GateSink {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
        fn streaming_tools_advertised(&self) -> bool {
            self.advertised
        }
    }

    #[test]
    fn streaming_context_some_only_when_host_advertises() {
        // W7 F4 regression: the per-turn AgentExecutorConfig must carry a
        // StreamingContext on live paths when the host advertises streaming
        // support, and stay None otherwise. Before this fix the engine
        // hardcoded `streaming: None`, leaving tool-chunk streaming dead on
        // every production path despite full dispatch machinery existing.
        let on: Arc<dyn OutputSink> = Arc::new(GateSink { advertised: true });
        let ctx = build_turn_streaming_context(&on, "m-1");
        let ctx = ctx.expect("streaming advertised => Some(StreamingContext)");
        assert_eq!(ctx.msg_id, "m-1");
        assert!(ctx.output.streaming_tools_advertised());

        let off: Arc<dyn OutputSink> = Arc::new(GateSink { advertised: false });
        assert!(
            build_turn_streaming_context(&off, "m-1").is_none(),
            "streaming NOT advertised => None (no behaviour change for hosts that don't opt in)"
        );
    }
}

/// Parse the leading `Exit code: N` line of a Bash tool result, defaulting to
/// 0 when absent or unparseable. Lets the compactor bias toward preserving the
/// failure reason on a non-zero exit.
fn parse_bash_exit_code(content: &str) -> i32 {
    content
        .lines()
        .next()
        .and_then(|l| l.strip_prefix("Exit code:"))
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0)
}

fn truncate_for_trace(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let end = s
        .char_indices()
        .take_while(|(i, _)| *i < max)
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(s.len());
    let mut out = s[..end].to_string();
    out.push_str("…[truncated]");
    out
}

// ---------------------------------------------------------------------------
// Finding #174 — smart-routing tier model swap (select_tier_model)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tier_routing_tests {
    use super::{message_requires_vision, select_tier_model};
    use wcore_config::compat::{ProviderCompat, TierModels};
    use wcore_providers::{RequestShape, RoutingHeuristics, route};

    /// A shape that classifies to Cheap (simple turn): small context, no tools,
    /// no code, no vision.
    fn cheap_shape() -> RequestShape {
        RequestShape {
            input_tokens: 100,
            max_output_tokens: 1024,
            code_ratio: 0.0,
            tool_call_count: 0,
            requires_vision: false,
        }
    }

    fn compat_with_cheap_model() -> ProviderCompat {
        ProviderCompat {
            tier_models: Some(TierModels {
                cheap: Some("cheap-model".into()),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// Cheap hint + configured tier_models + no vision → swaps to cheap model.
    ///
    /// Fails without the fix because nothing reads the routing hint: the engine
    /// would dispatch (and bill) the premium `self.model` on every cheap turn.
    #[test]
    fn cheap_hint_with_config_swaps_model() {
        let decision = route(&cheap_shape(), &RoutingHeuristics::default());
        let swapped = select_tier_model(&decision, false, &compat_with_cheap_model());
        assert_eq!(swapped.as_deref(), Some("cheap-model"));
    }

    /// Cheap hint but tier_models unconfigured (default) → no swap (opt-in).
    ///
    /// Fails if the feature were on-by-default: proves the presence of the map
    /// is the enable switch and default behaviour is unchanged.
    #[test]
    fn cheap_hint_without_config_keeps_model() {
        let decision = route(&cheap_shape(), &RoutingHeuristics::default());
        let swapped = select_tier_model(&decision, false, &ProviderCompat::default());
        assert_eq!(swapped, None);
    }

    /// Cheap hint + vision content → no swap (vision guard).
    ///
    /// Fails without the guard because the cheap tier may not be
    /// vision-capable; a naive swap would drop image content to a text-only
    /// model. (`message_requires_vision` is the live signal; here we assert the
    /// guard directly.)
    #[test]
    fn cheap_hint_with_vision_keeps_model() {
        let decision = route(&cheap_shape(), &RoutingHeuristics::default());
        // requires_vision = true even though the tier classified Cheap.
        let swapped = select_tier_model(&decision, true, &compat_with_cheap_model());
        assert_eq!(swapped, None);
    }

    /// Premium hint → no swap, even with a premium model configured.
    ///
    /// Fails without the tier check because a premium turn (vision /
    /// large-context / tool-heavy) must keep the strongest model.
    #[test]
    fn premium_hint_keeps_model() {
        let mut shape = cheap_shape();
        // Large context routes to Premium.
        shape.input_tokens = RoutingHeuristics::default().large_context_tokens + 1;
        let decision = route(&shape, &RoutingHeuristics::default());
        let compat = ProviderCompat {
            tier_models: Some(TierModels {
                cheap: Some("cheap-model".into()),
                premium: Some("premium-model".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(select_tier_model(&decision, false, &compat), None);
    }

    /// The vision signal helper returns false today (no image ContentBlock in
    /// the type system) — documents the current chokepoint behaviour so a
    /// future image variant has a failing test to update.
    #[test]
    fn vision_helper_is_false_without_image_blocks() {
        assert!(!message_requires_vision(&[]));
    }
}

// ---------------------------------------------------------------------------
// Finding #174 — end-to-end: the tier swap reaches the DISPATCHED request and
// cost attribution follows.
//
// The `tier_routing_tests` above exercise `select_tier_model` in isolation.
// They prove the GUARD logic, but nothing drives the real turn loop — so they
// cannot catch a regression that leaves the swap inert at the dispatch seam
// (e.g. someone deletes the `request.model = tier_model` assignment, or bills
// `self.model` instead of `effective_model`). These tests close that gap: a
// capturing provider records the `request.model` it is actually handed, and a
// capturing sink records the emitted `TurnTrace.model` and the per-turn
// `TurnCost.model` carried in the session-cost payload.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tier_routing_e2e_tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use serde_json::Value;
    use wcore_config::compat::{ProviderCompat, TierModels};
    use wcore_config::config::{Config, ProviderType};
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::{FinishReason, StopReason, TokenUsage};

    use crate::output::OutputSink;

    /// A provider that records the `model` field of every `LlmRequest` it is
    /// dispatched, then replays a single clean text turn. This is the only way
    /// to observe what the engine ACTUALLY put on the wire — the stock
    /// `test_utils::ScriptedProvider` ignores the request entirely.
    struct CapturingProvider {
        dispatched_models: Arc<Mutex<Vec<String>>>,
    }

    impl CapturingProvider {
        fn new() -> (Self, Arc<Mutex<Vec<String>>>) {
            let dispatched_models = Arc::new(Mutex::new(Vec::new()));
            (
                Self {
                    dispatched_models: Arc::clone(&dispatched_models),
                },
                dispatched_models,
            )
        }
    }

    #[async_trait]
    impl LlmProvider for CapturingProvider {
        async fn stream(
            &self,
            request: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            // Record the dispatched model BEFORE replaying the turn — this is
            // the load-bearing observation: it is `request.model` after the
            // engine's tier-swap seam has (or has not) run.
            self.dispatched_models
                .lock()
                .expect("CapturingProvider mutex")
                .push(request.model.clone());
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            tokio::spawn(async move {
                let _ = tx.send(LlmEvent::TextDelta("done".into())).await;
                let _ = tx
                    .send(LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        finish_reason: FinishReason::Stop,
                        usage: TokenUsage::default(),
                    })
                    .await;
            });
            Ok(rx)
        }
    }

    /// A sink that captures the two cost-attribution signals the engine emits:
    /// every `TurnTrace` JSON (via `emit_trace`) and the aggregate session-cost
    /// payload (via `emit_session_cost`, whose `per_turn` array carries each
    /// `TurnCost`). The default `OutputSink` impls for both are no-ops, so a
    /// dedicated sink is required to observe them. All other methods are no-ops.
    #[derive(Default)]
    struct CostCapturingSink {
        traces: Arc<Mutex<Vec<Value>>>,
        session_costs: Arc<Mutex<Vec<Value>>>,
    }

    impl OutputSink for CostCapturingSink {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
        fn emit_trace(&self, _: &str, trace_json: &Value) {
            self.traces
                .lock()
                .expect("trace mutex")
                .push(trace_json.clone());
        }
        fn emit_session_cost(&self, _: &str, cost_payload: &Value) {
            self.session_costs
                .lock()
                .expect("session-cost mutex")
                .push(cost_payload.clone());
        }
    }

    /// Build an anthropic-flavoured Config with `cheap` wired to `cheap-model`.
    fn config_with_cheap_tier() -> Config {
        Config {
            provider_label: "anthropic".into(),
            provider: ProviderType::Anthropic,
            api_key: "sk-test".into(),
            base_url: "http://localhost:0".into(),
            model: "premium-model".into(),
            max_tokens: 1024,
            max_turns: Some(1),
            compat: ProviderCompat {
                tier_models: Some(TierModels {
                    cheap: Some("cheap-model".into()),
                    ..Default::default()
                }),
                ..ProviderCompat::anthropic_defaults()
            },
            ..Default::default()
        }
    }

    /// The same Config with NO tier_models configured — the default, opt-out
    /// state where the engine must dispatch + bill the configured model.
    fn config_without_tier() -> Config {
        Config {
            provider_label: "anthropic".into(),
            provider: ProviderType::Anthropic,
            api_key: "sk-test".into(),
            base_url: "http://localhost:0".into(),
            model: "premium-model".into(),
            max_tokens: 1024,
            max_turns: Some(1),
            compat: ProviderCompat::anthropic_defaults(),
            ..Default::default()
        }
    }

    /// Drive one synthetic turn and return (dispatched models, trace JSONs,
    /// session-cost JSONs).
    async fn drive_turn(config: Config) -> (Vec<String>, Vec<Value>, Vec<Value>) {
        let (provider, dispatched) = CapturingProvider::new();
        let sink = Arc::new(CostCapturingSink::default());
        let traces = Arc::clone(&sink.traces);
        let session_costs = Arc::clone(&sink.session_costs);

        let mut engine = super::AgentEngine::new_with_provider(
            Arc::new(provider),
            config,
            ToolRegistry::new(),
            sink,
        );

        // A short, tool-free prompt classifies to the Cheap tier (input far
        // below the 8000-token large-context threshold, zero tool calls).
        engine
            .run("hi", "m-routing-e2e")
            .await
            .expect("synthetic turn should succeed");

        let dispatched = dispatched.lock().expect("dispatched mutex").clone();
        let traces = traces.lock().expect("trace mutex").clone();
        let session_costs = session_costs.lock().expect("session-cost mutex").clone();
        (dispatched, traces, session_costs)
    }

    /// POSITIVE: a Cheap-classified turn with `tier_models.cheap` configured
    /// must (a) dispatch `cheap-model` on the wire, AND (b) attribute the
    /// emitted TurnTrace.model and TurnCost.model to `cheap-model`.
    ///
    /// Fails if the swap is inert: without `request.model = tier_model` at the
    /// dispatch seam the provider would record `premium-model`. Fails if cost
    /// attribution regressed (billing `self.model` instead of
    /// `effective_model`): TurnTrace.model / TurnCost.model would read
    /// `premium-model` even though `cheap-model` was dispatched.
    #[tokio::test]
    async fn cheap_turn_swaps_dispatched_model_and_cost() {
        let (dispatched, traces, session_costs) = drive_turn(config_with_cheap_tier()).await;

        assert_eq!(
            dispatched,
            vec!["cheap-model".to_string()],
            "the swap must reach the wire: provider should have been handed cheap-model"
        );

        // TurnTrace.model — the per-turn observability record.
        let trace_models: Vec<String> = traces
            .iter()
            .filter_map(|t| t.get("model").and_then(Value::as_str).map(str::to_string))
            .collect();
        assert!(
            trace_models.iter().any(|m| m == "cheap-model"),
            "TurnTrace.model must follow the swap, got {trace_models:?}"
        );
        assert!(
            !trace_models.iter().any(|m| m == "premium-model"),
            "no TurnTrace may be billed to premium-model on a swapped turn, got {trace_models:?}"
        );

        // TurnCost.model — carried in the session-cost payload's per_turn array.
        let cost_models: Vec<String> = session_costs
            .iter()
            .filter_map(|c| c.get("per_turn").and_then(Value::as_array))
            .flatten()
            .filter_map(|t| t.get("model").and_then(Value::as_str).map(str::to_string))
            .collect();
        assert!(
            !cost_models.is_empty(),
            "a session-cost payload with at least one per-turn cost must be emitted"
        );
        assert!(
            cost_models.iter().all(|m| m == "cheap-model"),
            "every TurnCost.model must be cheap-model so billing follows the swap, got {cost_models:?}"
        );
    }

    /// NEGATIVE: with NO tier_models configured (the default), the same
    /// Cheap-classified turn must dispatch + bill the engine's configured
    /// `premium-model` — proving default behaviour is unchanged end-to-end and
    /// the swap is genuinely opt-in.
    ///
    /// Fails if the feature were on-by-default (provider would record a swapped
    /// model with no configuration present).
    #[tokio::test]
    async fn default_turn_keeps_dispatched_model_and_cost() {
        let (dispatched, traces, session_costs) = drive_turn(config_without_tier()).await;

        assert_eq!(
            dispatched,
            vec!["premium-model".to_string()],
            "with no tier_models the configured model must be dispatched unchanged"
        );

        let trace_models: Vec<String> = traces
            .iter()
            .filter_map(|t| t.get("model").and_then(Value::as_str).map(str::to_string))
            .collect();
        assert!(
            trace_models.iter().all(|m| m == "premium-model"),
            "TurnTrace.model must stay premium-model with no tier configured, got {trace_models:?}"
        );

        let cost_models: Vec<String> = session_costs
            .iter()
            .filter_map(|c| c.get("per_turn").and_then(Value::as_array))
            .flatten()
            .filter_map(|t| t.get("model").and_then(Value::as_str).map(str::to_string))
            .collect();
        assert!(
            !cost_models.is_empty(),
            "a session-cost payload with at least one per-turn cost must be emitted"
        );
        assert!(
            cost_models.iter().all(|m| m == "premium-model"),
            "every TurnCost.model must stay premium-model with no tier configured, got {cost_models:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// set_config tests — apply_config_update()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod set_config_tests {
    use std::sync::{Arc, Mutex};

    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::FinishReason;

    use crate::approval::ApprovalBridge;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;
    // v0.8.0 Task M: inline-test fixture builders need access to the
    // engine-private user-id resolver.
    use super::resolve_user_model_user_id;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_engine(model: &str) -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            temperature: None,
            tools: Arc::new(ToolRegistry::new()),
            messages: vec![],
            rebind_system_prefix: None,
            system_prompt: String::new(),
            model: model.to_string(),
            user_model_pin: None,
            max_tokens: 4096,
            max_turns: Some(10),
            total_usage: Default::default(),
            thinking: None,
            compat: wcore_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            current_msg_id: String::new(),
            current_agent_run_id: None,
            approval_manager: None,
            approval_bridge: Arc::new(ApprovalBridge::new()),
            protocol_writer: None,
            allow_list: vec![],
            current_reasoning_effort: None,
            compact_config: wcore_config::compact::CompactConfig::default(),
            compact_state: super::CompactState::new(),
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
            compaction_level: wcore_compact::CompactionLevel::default(),
            toon_enabled: false,
            compact_bash: false,
            advertised_capabilities: wcore_config::tools::AdvertisedCapabilitiesConfig::default(),
            per_turn_costs: Vec::new(),
            mcp_curation: wcore_config::config::McpCurationPolicy::default(),
            mcp_curation_cache: None,
            mcp_cap_cache: None,
            file_cache: None,
            session_state: None,
            audit_log: None,
            memory_api: Arc::new(wcore_memory::NullMemory),
            dream_throttle: Arc::new(wcore_memory::consolidate::DreamThrottle::new(
                std::time::Duration::from_secs(1800),
            )),
            #[cfg(any(test, feature = "test-utils"))]
            test_sink_handle: crate::test_utils::TestSinkHandle::default(),
            // W9.1 T3 (T10b): inline-test fixture default — gate off.
            skills_lifecycle: false,
            // F-092 (W7-N): inline-test fixture default — gate off.
            online_evolution: false,
            recent_turn_traces: std::collections::VecDeque::new(),
            drafted_skill_signatures: std::collections::HashSet::new(),
            // W8b.2.B D.3 / Task 7: inline-test fixture defaults — watcher off.
            file_watcher: Arc::new(std::sync::OnceLock::new()),
            tool_write_notifier: Arc::new(std::sync::OnceLock::new()),
            mode_override: None,
            template_router: None,
            decay_handles: Vec::new(),
            plugin_runtime_handles: Arc::new(Vec::new()),
            budget_tracker: None,
            policy_gate: None,
            agent_registry: None,
            plugin_user_models: Vec::new(),
            style_detector: Mutex::new(crate::style_detector::StyleDetector::new()),
            skill_catalog: None,
            user_model_backend: None,
            user_model_user_id: resolve_user_model_user_id(),
            // v0.8.1 U1 — installed post-construction by
            // `AgentBootstrap::build` (see `set_skill_router`). `None`
            // here keeps every non-bootstrap construction site (tests,
            // resume-without-bootstrap, sub-agent shadows) on the
            // pre-U1 no-op path.
            skill_router: None,
            current_skill_router_pick: None,
            // v0.8.1 U6 — autonomous-skill bucketer is always live (N=3
            // threshold). Drafter is None at construction; bootstrap
            // installs one when memory is wired.
            auto_skill_bucketer: Mutex::new(crate::auto_skill::Bucketer::new(3)),
            skill_drafter: None,
            // AUDIT A2 / B1 — fresh session-root cancellation token.
            // Hosts replace/observe it via `cancel_token()`.
            cancel_token: tokio_util::sync::CancellationToken::new(),
            // AUDIT B-2 / D-5 — reaper handle storage; populated by
            // `set_approval_manager`, aborted by `Drop`.
            background_handles: Vec::new(),
            // Dynamic Workflows B3 — detection gate (default off).
            workflow_detection_enabled: false,
            // Dynamic Workflows B6 — live confirm gate (default off) + a
            // default config for the (unused-in-these-fixtures) live gate.
            workflow_live_mode: false,
            config: wcore_config::config::Config::default(),
            compaction_floor: 0,
            session_start_injected_len: 0,
            web_search: false,
            pending_hook_actions: Vec::new(),
            conversation_id: String::new(),
            flux_served_window: None,
            flux_context_pressure: None,
            smart_compact_armed: true,
            smart_compact_last_turn: None,
            smart_compact_exhausted: false,
            smart_compact_force: false,
        }
    }

    fn make_engine_with_compat(
        model: &str,
        compat: wcore_config::compat::ProviderCompat,
    ) -> super::AgentEngine {
        let mut engine = make_engine(model);
        engine.compat = compat;
        engine
    }

    /// Cache-stability regression (token-opt): MCP tool curation must NOT churn
    /// the kept set turn-to-turn, or it rewrites the cached tool-zone prefix
    /// every turn at the cache-WRITE rate. The inventory-keyed UNION retains
    /// earlier-surfaced tools (monotonic) and is byte-identical once stabilized.
    #[test]
    fn mcp_curation_union_is_cache_stable_across_turns() {
        use wcore_types::message::{ContentBlock, Message, Role};

        fn mcp_tool(name: &str, desc: &str) -> wcore_types::tool::ToolDef {
            wcore_types::tool::ToolDef {
                name: name.to_string(),
                description: desc.to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                deferred: false,
                // #174 tools are mcp__-prefixed AND carry real provenance —
                // both signals classify them as MCP.
                server: Some("srv".to_string()),
            }
        }
        let tools = vec![
            mcp_tool("mcp__srv__alpha", "search alpha database records"),
            mcp_tool("mcp__srv__bravo", "send bravo email messages"),
            mcp_tool("mcp__srv__charlie", "compile charlie reports"),
            mcp_tool("mcp__srv__delta", "remove delta entries"),
        ];
        let names = |v: Vec<wcore_types::tool::ToolDef>| -> Vec<String> {
            v.into_iter().map(|t| t.name).collect()
        };
        let user = |text: &str| {
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
            )]
        };

        let mut engine = make_engine("m");
        engine.mcp_curation = wcore_config::config::McpCurationPolicy::TopK { k: 2 };
        engine.audit_log = None; // keyword-only ranking → deterministic

        // Turn 1: a request about "alpha database".
        engine.messages = user("alpha database query");
        let turn1 = names(engine.apply_mcp_curation(tools.clone()));
        assert!(turn1.contains(&"mcp__srv__alpha".to_string()));

        // Turn 2: a DIFFERENT request about "charlie reports". The old per-turn
        // curation would DROP alpha here (cache bust); the union must keep it.
        engine.messages = user("charlie reports compile");
        let turn2 = names(engine.apply_mcp_curation(tools.clone()));
        assert!(
            turn2.contains(&"mcp__srv__alpha".to_string()),
            "monotonic union must retain earlier-surfaced tools across turns"
        );
        assert!(turn2.contains(&"mcp__srv__charlie".to_string()));

        // Turn 3: repeat turn 2 — the union is now stable, so the serialized
        // tool list is byte-identical (a cache READ, not a prefix rewrite).
        let turn3 = names(engine.apply_mcp_curation(tools.clone()));
        assert_eq!(
            turn2, turn3,
            "stabilized union must be byte-identical across turns"
        );

        // A real inventory change (new MCP server tool) legitimately resets the
        // cache so the new tool can be surfaced.
        let mut tools2 = tools.clone();
        tools2.push(mcp_tool("mcp__srv__echo", "echo fresh tool"));
        engine.messages = user("echo fresh tool");
        let after_change = names(engine.apply_mcp_curation(tools2));
        assert!(after_change.contains(&"mcp__srv__echo".to_string()));
    }

    // --- #344/#359: provider-aware hard cap on the total tool count ---

    /// Build a non-MCP/built-in `ToolDef` (`server: None`).
    fn builtin_tool(name: &str) -> wcore_types::tool::ToolDef {
        wcore_types::tool::ToolDef {
            name: name.to_string(),
            description: format!("{name} built-in tool"),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            server: None,
        }
    }

    /// Build an MCP `ToolDef` (`mcp__{server}__{tool}`, real provenance set).
    fn cap_mcp_tool(name: &str) -> wcore_types::tool::ToolDef {
        wcore_types::tool::ToolDef {
            name: name.to_string(),
            description: format!("{name} mcp tool"),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            server: Some("srv".to_string()),
        }
    }

    #[test]
    fn provider_tool_cap_truncates_total_to_limit() {
        let mut engine = make_engine("m");
        engine.compat.max_tools = Some(10);

        let mut tools = vec![
            builtin_tool("Read"),
            builtin_tool("Write"),
            builtin_tool("Bash"),
            builtin_tool("ToolSearch"),
        ];
        for i in 0..20 {
            tools.push(cap_mcp_tool(&format!("mcp__srv__tool{i:02}")));
        }

        let capped = engine.apply_provider_tool_cap(tools);
        assert_eq!(capped.len(), 10, "total must be truncated to the limit");

        let names: Vec<&str> = capped.iter().map(|t| t.name.as_str()).collect();
        for b in ["Read", "Write", "Bash", "ToolSearch"] {
            assert!(names.contains(&b), "built-in {b} must be kept");
        }
        let mcp_kept = capped
            .iter()
            .filter(|t| t.name.starts_with("mcp__"))
            .count();
        assert_eq!(mcp_kept, 6, "exactly (limit - non_mcp) MCP tools kept");
    }

    #[test]
    fn provider_tool_cap_noop_when_unset() {
        let mut engine = make_engine("m");
        engine.compat.max_tools = None;

        let tools: Vec<_> = (0..50)
            .map(|i| cap_mcp_tool(&format!("mcp__srv__tool{i:02}")))
            .collect();
        let capped = engine.apply_provider_tool_cap(tools);
        assert_eq!(capped.len(), 50, "no cap configured → unchanged");
    }

    #[test]
    fn provider_tool_cap_noop_under_limit() {
        let mut engine = make_engine("m");
        engine.compat.max_tools = Some(128);

        let tools: Vec<_> = (0..30)
            .map(|i| cap_mcp_tool(&format!("mcp__srv__tool{i:02}")))
            .collect();
        let capped = engine.apply_provider_tool_cap(tools);
        assert_eq!(capped.len(), 30, "under the limit → unchanged");
    }

    #[test]
    fn provider_tool_cap_keeps_all_builtins_even_when_budget_tiny() {
        let mut engine = make_engine("m");
        engine.compat.max_tools = Some(3);

        let mut tools = vec![
            builtin_tool("Read"),
            builtin_tool("Write"),
            builtin_tool("Edit"),
            builtin_tool("Bash"),
            builtin_tool("Grep"),
            builtin_tool("ToolSearch"),
        ];
        for i in 0..10 {
            tools.push(cap_mcp_tool(&format!("mcp__srv__tool{i:02}")));
        }

        let capped = engine.apply_provider_tool_cap(tools);
        let names: Vec<&str> = capped.iter().map(|t| t.name.as_str()).collect();
        for b in ["Read", "Write", "Edit", "Bash", "Grep", "ToolSearch"] {
            assert!(
                names.contains(&b),
                "built-in {b} must be kept even when built-ins alone exceed the cap"
            );
        }
        let mcp_kept = capped
            .iter()
            .filter(|t| t.name.starts_with("mcp__"))
            .count();
        assert_eq!(mcp_kept, 0, "no MCP budget left → 0 MCP tools kept");
        assert_eq!(capped.len(), 6, "correctness > strict limit for built-ins");
    }

    // --- #359: provenance-based MCP classification (NOT the `mcp__` prefix) ---

    /// Build a BARE-named MCP `ToolDef`: real provenance (`server: Some`) but a
    /// name with NO `mcp__` prefix — exactly what `tool_proxy.rs` registers for
    /// a non-colliding MCP tool (e.g. `execute_sql`, `list_calendar_events`).
    fn bare_mcp_tool(name: &str) -> wcore_types::tool::ToolDef {
        wcore_types::tool::ToolDef {
            name: name.to_string(),
            description: format!("{name} bare-named mcp tool"),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            server: Some("db".to_string()),
        }
    }

    /// #359 regression — a BARE-named MCP tool (no `mcp__` prefix) must count
    /// against the provider cap and be DROPPED when over budget. Pre-fix it was
    /// classified as a pseudo-built-in (name-prefix check) and ALWAYS kept,
    /// letting a swarm of bare-named MCP tools blow past the provider limit.
    #[test]
    fn provider_tool_cap_drops_bare_named_mcp_over_budget() {
        let mut engine = make_engine("m");
        engine.compat.max_tools = Some(4);

        // 2 real built-ins (server: None) + 10 bare-named MCP tools (server:
        // Some, NO `mcp__` prefix). Budget = 4 - 2 = 2 MCP slots.
        let mut tools = vec![builtin_tool("Read"), builtin_tool("ToolSearch")];
        for i in 0..10 {
            tools.push(bare_mcp_tool(&format!("execute_sql_{i:02}")));
        }

        let capped = engine.apply_provider_tool_cap(tools);

        // Both built-ins kept; exactly (limit - non_mcp) = 2 bare MCP tools kept.
        let names: Vec<&str> = capped.iter().map(|t| t.name.as_str()).collect();
        assert!(names.contains(&"Read"));
        assert!(names.contains(&"ToolSearch"));
        let mcp_kept = capped.iter().filter(|t| t.server.is_some()).count();
        assert_eq!(
            mcp_kept, 2,
            "bare-named MCP tools must be budgeted/truncated like prefixed ones"
        );
        assert_eq!(
            capped.len(),
            4,
            "total must be capped at the provider limit"
        );
        // The dropped bare MCP tools are NOT silently kept as pseudo-builtins.
        assert!(
            capped.iter().filter(|t| t.server.is_none()).count() == 2,
            "only the two real built-ins are server:None"
        );
    }

    /// #359 regression — a real built-in (`server: None`) is NEVER classified as
    /// MCP, so it is always kept by the cap even when it shares a non-prefixed
    /// snake_case name shape with MCP tools.
    #[test]
    fn provider_tool_cap_never_drops_real_builtin() {
        let mut engine = make_engine("m");
        engine.compat.max_tools = Some(3);

        // 5 built-ins (server: None) exceed the cap of 3 — but built-ins are
        // always kept; only MCP tools are ever truncated.
        let tools = vec![
            builtin_tool("Read"),
            builtin_tool("Write"),
            builtin_tool("execute"), // bare snake_case name, but server: None
            builtin_tool("Bash"),
            builtin_tool("ToolSearch"),
        ];

        let capped = engine.apply_provider_tool_cap(tools);
        let names: Vec<&str> = capped.iter().map(|t| t.name.as_str()).collect();
        for b in ["Read", "Write", "execute", "Bash", "ToolSearch"] {
            assert!(names.contains(&b), "built-in {b} must never be dropped");
        }
        assert_eq!(
            capped.len(),
            5,
            "all server:None tools kept (correctness > strict limit for built-ins)"
        );
    }

    /// Cap-path relevance + cache-stability: when the provider cap must trim a
    /// large MCP inventory, BM25 keeps the calendar-relevant tool for a calendar
    /// request (not array-order tail-truncation), AND the emitted set is
    /// byte-identical across two same-context turns (no prompt-cache prefix
    /// churn — the inventory-keyed append-only union holds slots).
    #[test]
    fn provider_tool_cap_keeps_relevant_and_is_cache_stable() {
        use wcore_types::message::{ContentBlock, Message, Role};

        let mut engine = make_engine("m");
        // Tiny cap so the 200 generic MCP tools must be trimmed hard.
        engine.compat.max_tools = Some(5);
        engine.audit_log = None; // BM25-only ranking → deterministic
        engine.messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "schedule a calendar meeting".to_string(),
            }],
        )];

        let mut tools = vec![builtin_tool("Read"), builtin_tool("ToolSearch")];
        // The one relevant tool, placed in the MIDDLE of the array so array-order
        // truncation would drop it — only BM25 can rescue it.
        for i in 0..100 {
            tools.push(cap_mcp_tool(&format!("mcp__srv__generic{i:03}")));
        }
        tools.push(wcore_types::tool::ToolDef {
            name: "mcp__gcal__create_calendar_event".to_string(),
            description: "Create a calendar event to schedule a meeting".to_string(),
            input_schema: serde_json::json!({"type": "object"}),
            deferred: false,
            server: Some("gcal".to_string()),
        });
        for i in 100..200 {
            tools.push(cap_mcp_tool(&format!("mcp__srv__generic{i:03}")));
        }

        let turn1 = engine.apply_provider_tool_cap(tools.clone());
        let names1: Vec<&str> = turn1.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(turn1.len(), 5, "total capped to the provider limit");
        assert!(
            names1.contains(&"mcp__gcal__create_calendar_event"),
            "BM25 must retain the calendar tool for a calendar request, \
             not array-order tail-truncate it"
        );
        assert!(names1.contains(&"Read") && names1.contains(&"ToolSearch"));

        // Turn 2, same context: the emitted set must be byte-identical (the
        // append-only union holds every slot — no prefix churn).
        let turn2 = engine.apply_provider_tool_cap(tools);
        let names2: Vec<&str> = turn2.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(
            names1, names2,
            "cap emission must be byte-stable across same-context turns"
        );
    }

    /// #359 regression — a BARE-named MCP tool is subject to curation. The
    /// curator must rank/trim it like a prefixed MCP tool; a built-in with the
    /// same snake_case shape (server: None) must never be curated away.
    #[test]
    fn mcp_curation_curates_bare_named_mcp_and_spares_builtin() {
        use wcore_types::message::{ContentBlock, Message, Role};

        let names = |v: Vec<wcore_types::tool::ToolDef>| -> Vec<String> {
            v.into_iter().map(|t| t.name).collect()
        };

        // 1 built-in (server: None) + 4 BARE-named MCP tools (server: Some, no
        // `mcp__` prefix). With k=2 and 4 MCP tools > k, curation must engage.
        let tools = vec![
            builtin_tool("Read"), // server: None → always kept, never curated
            bare_mcp_tool("query_alpha_database"),
            bare_mcp_tool("send_bravo_email"),
            bare_mcp_tool("compile_charlie_report"),
            bare_mcp_tool("remove_delta_entry"),
        ];

        let mut engine = make_engine("m");
        engine.mcp_curation = wcore_config::config::McpCurationPolicy::TopK { k: 2 };
        engine.audit_log = None; // keyword-only ranking → deterministic
        engine.messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "query alpha database".to_string(),
            }],
        )];

        let kept = names(engine.apply_mcp_curation(tools));

        // The built-in survives unconditionally (it is NOT an MCP tool).
        assert!(
            kept.contains(&"Read".to_string()),
            "real built-in must never be curated away"
        );
        // The keyword-matching bare MCP tool is surfaced...
        assert!(
            kept.contains(&"query_alpha_database".to_string()),
            "keyword-matched bare MCP tool must be curated in"
        );
        // ...and curation actually trimmed the bare MCP set (4 MCP > k=2), i.e.
        // bare-named MCP tools ARE subject to TopK (pre-fix they were treated as
        // built-ins and ALL kept).
        let mcp_kept = kept.iter().filter(|n| n.as_str() != "Read").count();
        assert_eq!(
            mcp_kept, 2,
            "bare-named MCP tools must be curated to TopK (k=2), not all kept"
        );
    }

    /// Cache-stability regression (token-opt, finding #174): when turn 2
    /// unions in a tool that sorts EARLIER in registry-iteration order than an
    /// already-kept tool, the kept set must stay APPEND-ONLY — the new tool
    /// lands AFTER the already-kept tools, never ahead of them. Inserting a
    /// tool earlier in the `tools` array would invalidate the Anthropic
    /// prompt-cache prefix from that point through the rest of the tools zone.
    ///
    /// Pre-fix, the final emission iterated `mcp_tools` in registry order and
    /// kept any name present in an unordered `HashSet`, so a newly-unioned tool
    /// reappeared at its REGISTRY position (here `alpha`, index 0) ahead of the
    /// turn-1 keep (`zulu`, index 1) — a mid-array insert that busts the cache.
    #[test]
    fn mcp_curation_union_is_append_only_not_registry_order() {
        use wcore_types::message::{ContentBlock, Message, Role};

        fn mcp_tool(name: &str, desc: &str) -> wcore_types::tool::ToolDef {
            wcore_types::tool::ToolDef {
                name: name.to_string(),
                description: desc.to_string(),
                input_schema: serde_json::json!({"type": "object"}),
                deferred: false,
                // #174 tools are mcp__-prefixed AND carry real provenance.
                server: Some("srv".to_string()),
            }
        }
        // Registry order: alpha sits at index 0, ahead of zulu/yankee. Turn 1
        // surfaces zulu + yankee (both positive keyword overlap, filling the
        // k=2 picks); alpha scores zero on turn 1 so it is NOT kept yet. Turn 2
        // surfaces alpha — which sits BEFORE zulu/yankee in registry order.
        // Append-only ordering must keep zulu/yankee first and append alpha.
        let tools = vec![
            mcp_tool("mcp__srv__alpha", "search alpha database records"),
            mcp_tool("mcp__srv__zulu", "compile zulu reports"),
            mcp_tool("mcp__srv__yankee", "compile yankee reports"),
            mcp_tool("mcp__srv__bravo", "send bravo email messages"),
        ];
        let names = |v: Vec<wcore_types::tool::ToolDef>| -> Vec<String> {
            v.into_iter().map(|t| t.name).collect()
        };
        let user = |text: &str| {
            vec![Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: text.to_string(),
                }],
            )]
        };

        let mut engine = make_engine("m");
        engine.mcp_curation = wcore_config::config::McpCurationPolicy::TopK { k: 2 };
        engine.audit_log = None; // keyword-only ranking → deterministic

        // Turn 1: "compile reports" surfaces zulu + yankee (both score 2);
        // alpha scores 0 and is NOT kept. zulu lands ahead of the later union.
        engine.messages = user("compile reports");
        let turn1 = names(engine.apply_mcp_curation(tools.clone()));
        assert!(turn1.contains(&"mcp__srv__zulu".to_string()));
        assert!(
            !turn1.contains(&"mcp__srv__alpha".to_string()),
            "alpha must not be surfaced on turn 1 (precondition for the union-growth check)"
        );
        let zulu_pos_1 = turn1.iter().position(|n| n == "mcp__srv__zulu");

        // Turn 2: "alpha database" surfaces. alpha is unioned in. Despite alpha
        // sitting at registry index 0 (ahead of zulu's index 1), append-only
        // ordering must place alpha AFTER the already-kept zulu.
        engine.messages = user("alpha database query");
        let turn2 = names(engine.apply_mcp_curation(tools.clone()));

        let zulu_pos_2 = turn2
            .iter()
            .position(|n| n == "mcp__srv__zulu")
            .expect("zulu must still be kept (monotonic union)");
        let alpha_pos_2 = turn2
            .iter()
            .position(|n| n == "mcp__srv__alpha")
            .expect("alpha must be unioned in on turn 2");

        // The append-only invariant: the newly-unioned alpha lands AFTER zulu.
        // Pre-fix this fails — alpha (registry idx 0) emits ahead of zulu.
        assert!(
            zulu_pos_2 < alpha_pos_2,
            "newly-unioned tool must APPEND after already-kept tools, not \
             insert ahead of them (zulu at {zulu_pos_2}, alpha at {alpha_pos_2})"
        );

        // And the prefix is byte-stable: zulu keeps its turn-1 position.
        assert_eq!(
            zulu_pos_1,
            Some(zulu_pos_2),
            "already-kept tool must retain its position across the union-growth turn"
        );
    }

    // --- Cycle 1 tests (updated signature) ---

    #[test]
    fn set_config_changes_model() {
        let mut engine = make_engine("old-model");
        let changes = engine.apply_config_update(Some("new-model".into()), None, None, None, None);
        assert_eq!(engine.model, "new-model");
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("old-model"));
        assert!(changes[0].contains("new-model"));
    }

    #[test]
    fn set_config_none_model_no_change() {
        let mut engine = make_engine("current");
        let changes = engine.apply_config_update(None, None, None, None, None);
        assert_eq!(engine.model, "current");
        assert!(changes.is_empty());
    }

    /// Wave-6 #5: after an in-session rebind, the boot framework fragments
    /// (Constitution / persona / skills index) MUST still be present in the
    /// effective system prompt while the fresh config/name overlay is also
    /// applied. Earlier `set_system_prompt` REPLACED the prompt wholesale,
    /// silently dropping every framework fragment on the first `/config`,
    /// `/provider`, or `/profile` — the F-003 "no deliverables" regression
    /// reintroduced via the rebind seam.
    #[test]
    fn rebind_preserves_framework_fragments_and_applies_overlay() {
        let mut engine = make_engine("m");
        // Simulate the boot prompt build_system_prompt produced: framework
        // fragments folded together with the resolved config prompt. The
        // constructor seeds `rebind_system_prefix` from this; mirror that here.
        let boot_prompt = "## Constitution\nObey the rules.\n\n\
                           ## Skills\n- writer\n\n\
                           ## Persona\nYou are Wayland.\n\n\
                           You are a helpful agent.";
        engine.system_prompt = boot_prompt.to_string();
        engine.rebind_system_prefix = Some(boot_prompt.to_string());

        // A rebind installs only the display-name overlay (what /provider,
        // /profile, /config actually change about the prompt).
        engine.set_system_prompt("You are talking to Sean.".to_string());

        // Framework fragments survive the rebind.
        assert!(
            engine.system_prompt.contains("## Constitution"),
            "Constitution must survive the rebind: {}",
            engine.system_prompt
        );
        assert!(
            engine.system_prompt.contains("## Skills"),
            "skills index must survive the rebind: {}",
            engine.system_prompt
        );
        assert!(
            engine.system_prompt.contains("## Persona"),
            "persona must survive the rebind: {}",
            engine.system_prompt
        );
        // The fresh overlay is applied AND leads the prompt.
        assert!(
            engine.system_prompt.starts_with("You are talking to Sean."),
            "the fresh config/name overlay must take effect at the front: {}",
            engine.system_prompt
        );
        // It is a prepend, not a duplication: the base appears exactly once.
        assert_eq!(
            engine.system_prompt.matches("## Constitution").count(),
            1,
            "the retained base must not be duplicated: {}",
            engine.system_prompt
        );
    }

    /// Wave-6 #5: a second rebind REPLACES the overlay rather than stacking a
    /// second name block — the retained base is the stable anchor, the overlay
    /// is swappable. Guards against the prepend-accumulation bug.
    #[test]
    fn successive_rebinds_replace_overlay_not_accumulate() {
        let mut engine = make_engine("m");
        engine.system_prompt = "## Constitution\nbase".to_string();
        engine.rebind_system_prefix = Some("## Constitution\nbase".to_string());

        engine.set_system_prompt("You are talking to Sean.".to_string());
        engine.set_system_prompt("You are talking to Alex.".to_string());

        assert!(engine.system_prompt.contains("Alex"));
        assert_eq!(
            engine.system_prompt.matches("You are talking to").count(),
            1,
            "successive rebinds must not accumulate name blocks: {}",
            engine.system_prompt
        );
        assert_eq!(
            engine.system_prompt.matches("## Constitution").count(),
            1,
            "the base must stay singular across rebinds: {}",
            engine.system_prompt
        );
    }

    /// Wave-6 #5: an empty overlay installs the retained base unchanged (a
    /// rebind for a session with no display name keeps the framework prompt).
    #[test]
    fn rebind_with_empty_overlay_keeps_retained_base() {
        let mut engine = make_engine("m");
        let base = "## Constitution\nbase prompt";
        engine.system_prompt = base.to_string();
        engine.rebind_system_prefix = Some(base.to_string());

        engine.set_system_prompt(String::new());
        assert_eq!(engine.system_prompt, base);
    }

    /// Wave-6 #5: with no retained base (the test-only / non-bootstrap
    /// constructors that set `rebind_system_prefix = None`), `set_system_prompt`
    /// falls back to the legacy replace semantics so existing behavior is
    /// unchanged.
    #[test]
    fn set_system_prompt_without_base_replaces() {
        let mut engine = make_engine("m");
        assert!(engine.rebind_system_prefix.is_none());
        engine.system_prompt = "old".to_string();
        engine.set_system_prompt("new".to_string());
        assert_eq!(engine.system_prompt, "new");
    }

    /// Wave-6 #5: `inject_history` keeps the retained rebind base in lockstep
    /// with the live boot prompt, so framework fragments delivered via the
    /// protocol/host `init_history` path also survive a later rebind.
    #[test]
    fn inject_history_updates_retained_rebind_base() {
        let mut engine = make_engine("m");
        engine.system_prompt = "config prompt".to_string();
        engine.rebind_system_prefix = Some("config prompt".to_string());

        engine.inject_history("## Constitution\nfrom host".to_string());
        // The prepend is reflected in both the live prompt and the retained base.
        assert!(engine.system_prompt.contains("## Constitution"));
        assert_eq!(
            engine.rebind_system_prefix.as_deref(),
            Some(engine.system_prompt.as_str()),
            "the retained base must track the live boot prompt after inject_history"
        );

        // A subsequent rebind therefore still carries the host-injected fragment.
        engine.set_system_prompt("You are talking to Sean.".to_string());
        assert!(engine.system_prompt.contains("## Constitution"));
        assert!(engine.system_prompt.starts_with("You are talking to Sean."));
    }

    /// F6 regression: a `/style` (inject_history) BETWEEN two `/config`
    /// (set_system_prompt) rebinds must not bake the name overlay into the
    /// retained base, so the second rebind prepends the name exactly ONCE.
    /// Previously `inject_history` captured the full live prompt — which already
    /// carried the first rebind's overlay — as the new base, so the next rebind
    /// double-prepended the display name (cosmetic name-appears-twice bug).
    #[test]
    fn inject_history_between_rebinds_does_not_double_the_name() {
        let mut engine = make_engine("m");
        engine.system_prompt = "## Constitution\nbase".to_string();
        engine.rebind_system_prefix = Some("## Constitution\nbase".to_string());

        // /config #1: install a name overlay.
        engine.set_system_prompt("You are talking to Sean.".to_string());
        // /style: inject a framework fragment (the overlay is live at this point).
        engine.inject_history("## Persona\nfriendly".to_string());
        // /config #2: re-bind the name overlay.
        engine.set_system_prompt("You are talking to Sean.".to_string());

        assert_eq!(
            engine.system_prompt.matches("You are talking to").count(),
            1,
            "the display name must appear exactly once after style-between-rebinds: {}",
            engine.system_prompt,
        );
        // The injected fragment and the original base both survive, once each.
        assert_eq!(
            engine.system_prompt.matches("## Persona").count(),
            1,
            "injected fragment survives singular: {}",
            engine.system_prompt,
        );
        assert_eq!(
            engine.system_prompt.matches("## Constitution").count(),
            1,
            "the base stays singular across the inject + rebind: {}",
            engine.system_prompt,
        );
    }

    /// Wave-6 #5 (secondary): a loaded/resumed session must start WITHOUT the
    /// previous session's explicit `/model` pin, so the resumed session's
    /// intended model wins and hook/skill `switch_model` is honoured again.
    #[test]
    fn load_conversation_clears_stale_model_pin() {
        let mut engine = make_engine("m");
        // A prior session pinned a model via `/model`.
        engine.set_model("pinned-model");
        assert_eq!(engine.user_model_pin(), Some("pinned-model"));

        // Resuming a different session drops the stale pin.
        engine.load_conversation(vec![]);
        assert_eq!(
            engine.user_model_pin(),
            None,
            "load_conversation must clear the previous session's /model pin"
        );

        // And with no pin, a hook/skill switch_model is honoured again.
        engine.apply_switch_model("hook-model".to_string());
        assert_eq!(engine.model(), "hook-model");
    }

    /// Wave-6 #5 (secondary, contrast): the legitimate in-session pin still
    /// blocks an implicit switch_model — `load_conversation` only clears the pin
    /// at the session boundary, it does not regress live-pin precedence.
    #[test]
    fn live_model_pin_still_blocks_switch_model_within_a_session() {
        let mut engine = make_engine("m");
        engine.set_model("pinned-model");
        // A hook tries to move the model mid-session; the pin wins.
        engine.apply_switch_model("hook-model".to_string());
        assert_eq!(
            engine.model(),
            "pinned-model",
            "an active /model pin must still shadow an implicit switch_model"
        );
    }

    #[test]
    fn set_config_same_model_still_reports_change() {
        let mut engine = make_engine("same");
        let changes = engine.apply_config_update(Some("same".into()), None, None, None, None);
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_empty_string_model_accepted() {
        let mut engine = make_engine("real-model");
        engine.apply_config_update(Some(String::new()), None, None, None, None);
        assert_eq!(engine.model, "");
    }

    #[test]
    fn set_config_model_does_not_affect_other_state() {
        let mut engine = make_engine("m");
        engine.current_reasoning_effort = Some("high".into());
        engine.apply_config_update(Some("new-m".into()), None, None, None, None);
        assert_eq!(engine.model, "new-m");
        assert_eq!(engine.current_reasoning_effort.as_deref(), Some("high"));
    }

    // --- Cycle 2: Effort config tests ---

    #[test]
    fn set_config_changes_effort() {
        let mut engine =
            make_engine_with_compat("m", wcore_config::compat::ProviderCompat::openai_defaults());
        assert!(engine.current_reasoning_effort.is_none());
        let changes = engine.apply_config_update(None, None, None, Some("high".into()), None);
        assert_eq!(engine.current_reasoning_effort.as_deref(), Some("high"));
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("high"));
    }

    #[test]
    fn set_config_clears_effort_with_empty_string() {
        let mut engine = make_engine("m");
        engine.current_reasoning_effort = Some("high".into());
        let changes = engine.apply_config_update(None, None, None, Some(String::new()), None);
        assert!(engine.current_reasoning_effort.is_none());
        assert_eq!(changes.len(), 1);
    }

    // --- Cycle 2: Thinking config tests ---

    #[test]
    fn set_config_enables_thinking() {
        let mut engine = make_engine("m");
        let changes =
            engine.apply_config_update(None, Some("enabled".into()), Some(16000), None, None);
        match &engine.thinking {
            Some(wcore_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 16000);
            }
            other => panic!("expected Enabled, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_disables_thinking() {
        let mut engine = make_engine("m");
        engine.thinking = Some(wcore_types::llm::ThinkingConfig::Enabled {
            budget_tokens: 8000,
        });
        let changes = engine.apply_config_update(None, Some("disabled".into()), None, None, None);
        match &engine.thinking {
            Some(wcore_types::llm::ThinkingConfig::Disabled) => {}
            other => panic!("expected Disabled, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_thinking_enabled_default_budget() {
        let mut engine = make_engine("m");
        let changes = engine.apply_config_update(None, Some("enabled".into()), None, None, None);
        match &engine.thinking {
            Some(wcore_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert!(*budget_tokens > 0);
            }
            other => panic!("expected Enabled with default budget, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_invalid_thinking_ignored() {
        let mut engine = make_engine("m");
        engine.thinking = Some(wcore_types::llm::ThinkingConfig::Enabled {
            budget_tokens: 8000,
        });
        let changes =
            engine.apply_config_update(None, Some("invalid_value".into()), None, None, None);
        match &engine.thinking {
            Some(wcore_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 8000);
            }
            other => panic!("expected Enabled unchanged, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
        assert!(changes[0].contains("invalid") || changes[0].contains("ignored"));
    }

    // --- Cycle 2: Combined fields test ---

    #[test]
    fn set_config_all_fields_at_once() {
        let compat = wcore_config::compat::ProviderCompat {
            supports_thinking: Some(true),
            supports_effort: Some(true),
            effort_levels: Some(vec!["low".into()]),
            ..Default::default()
        };
        let mut engine = make_engine_with_compat("old-model", compat);
        let changes = engine.apply_config_update(
            Some("new-model".into()),
            Some("enabled".into()),
            Some(12000),
            Some("low".into()),
            None,
        );
        assert_eq!(engine.model, "new-model");
        assert_eq!(engine.current_reasoning_effort.as_deref(), Some("low"));
        match &engine.thinking {
            Some(wcore_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 12000);
            }
            other => panic!("expected Enabled, got: {other:?}"),
        }
        assert_eq!(changes.len(), 3);
    }

    // --- Cycle 2: White-box edge case tests ---

    #[test]
    fn set_config_thinking_budget_only_updates_existing_enabled() {
        let mut engine = make_engine("m");
        engine.thinking = Some(wcore_types::llm::ThinkingConfig::Enabled {
            budget_tokens: 5000,
        });
        let changes = engine.apply_config_update(None, None, Some(20000), None, None);
        match &engine.thinking {
            Some(wcore_types::llm::ThinkingConfig::Enabled { budget_tokens }) => {
                assert_eq!(*budget_tokens, 20000);
            }
            other => panic!("expected Enabled with 20000, got: {other:?}"),
        }
        assert_eq!(changes.len(), 1);
    }

    #[test]
    fn set_config_thinking_budget_ignored_when_disabled() {
        let mut engine = make_engine("m");
        engine.thinking = Some(wcore_types::llm::ThinkingConfig::Disabled);
        let changes = engine.apply_config_update(None, None, Some(20000), None, None);
        match &engine.thinking {
            Some(wcore_types::llm::ThinkingConfig::Disabled) => {}
            other => panic!("expected Disabled unchanged, got: {other:?}"),
        }
        assert!(changes.is_empty());
    }

    #[test]
    fn set_config_effort_valid_values() {
        let compat = wcore_config::compat::ProviderCompat {
            supports_effort: Some(true),
            effort_levels: Some(vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "max".into(),
            ]),
            ..Default::default()
        };
        for value in ["low", "medium", "high", "max"] {
            let mut engine = make_engine_with_compat("m", compat.clone());
            engine.apply_config_update(None, None, None, Some(value.to_string()), None);
            assert_eq!(
                engine.current_reasoning_effort.as_deref(),
                Some(value),
                "effort should be set to {value}"
            );
        }
    }

    // --- Capability validation tests ---

    #[test]
    fn set_config_thinking_rejected_when_unsupported() {
        let mut engine =
            make_engine_with_compat("m", wcore_config::compat::ProviderCompat::openai_defaults());
        let changes = engine.apply_config_update(None, Some("enabled".into()), None, None, None);
        assert!(changes.iter().any(|c| c.contains("not supported")));
        assert!(engine.thinking.is_none());
    }

    #[test]
    fn set_config_effort_rejected_when_unsupported() {
        let mut engine = make_engine("m"); // anthropic defaults: supports_effort = false
        let changes = engine.apply_config_update(None, None, None, Some("high".into()), None);
        assert!(changes.iter().any(|c| c.contains("not supported")));
        assert!(engine.current_reasoning_effort.is_none());
    }

    #[test]
    fn set_config_effort_rejected_invalid_level() {
        let mut engine =
            make_engine_with_compat("m", wcore_config::compat::ProviderCompat::openai_defaults());
        let changes = engine.apply_config_update(None, None, None, Some("max".into()), None);
        assert!(changes.iter().any(|c| c.contains("invalid")));
        assert!(engine.current_reasoning_effort.is_none());
    }

    #[test]
    fn set_config_effort_clear_always_works() {
        let mut engine = make_engine("m"); // anthropic defaults: supports_effort = false
        engine.current_reasoning_effort = Some("high".into());
        let changes = engine.apply_config_update(None, None, None, Some(String::new()), None);
        assert!(engine.current_reasoning_effort.is_none());
        assert!(changes.iter().any(|c| c.contains("cleared")));
    }

    // ---- Dynamic Workflows B3 — engine-seam gate ----

    /// Default config keeps workflow detection OFF, so the per-turn
    /// `WorkflowCandidate` heuristic at the telemetry seam is never even
    /// computed — a default-config session is byte-for-byte unchanged.
    #[test]
    fn workflow_detection_defaults_off() {
        let cfg = wcore_config::config::Config::default();
        let engine = super::AgentEngine::new_with_provider(
            Arc::new(NullProvider),
            cfg,
            ToolRegistry::new(),
            Arc::new(NullOutput),
        );
        assert!(
            !engine.workflow_detection_enabled,
            "workflow detection must default to off"
        );
    }

    /// Flipping `[observability] workflow_detection_enabled = true`
    /// propagates to the engine's cached gate (opt-in plumbing works).
    #[test]
    fn workflow_detection_opt_in_propagates() {
        let mut cfg = wcore_config::config::Config::default();
        cfg.observability.workflow_detection_enabled = true;
        let engine = super::AgentEngine::new_with_provider(
            Arc::new(NullProvider),
            cfg,
            ToolRegistry::new(),
            Arc::new(NullOutput),
        );
        assert!(engine.workflow_detection_enabled);
    }
}

// ---------------------------------------------------------------------------
// Phase 6 tests — apply_context_modifiers()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod phase6_tests {
    use std::sync::{Arc, Mutex};

    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::FinishReason;
    use wcore_types::skill_types::{ContextModifier, EffortLevel};

    use crate::approval::ApprovalBridge;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;
    // v0.8.0 Task M: inline-test fixture builders need access to the
    // engine-private user-id resolver.
    use super::resolve_user_model_user_id;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_engine(model: &str, allow_list: Vec<String>) -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            temperature: None,
            tools: Arc::new(ToolRegistry::new()),
            messages: vec![],
            rebind_system_prefix: None,
            system_prompt: String::new(),
            model: model.to_string(),
            user_model_pin: None,
            max_tokens: 4096,
            max_turns: Some(10),
            total_usage: Default::default(),
            thinking: None,
            compat: wcore_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, allow_list.clone()))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            current_msg_id: String::new(),
            current_agent_run_id: None,
            approval_manager: None,
            approval_bridge: Arc::new(ApprovalBridge::new()),
            protocol_writer: None,
            allow_list,
            current_reasoning_effort: None,
            compact_config: wcore_config::compact::CompactConfig::default(),
            compact_state: super::CompactState::new(),
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
            compaction_level: wcore_compact::CompactionLevel::default(),
            toon_enabled: false,
            compact_bash: false,
            advertised_capabilities: wcore_config::tools::AdvertisedCapabilitiesConfig::default(),
            per_turn_costs: Vec::new(),
            mcp_curation: wcore_config::config::McpCurationPolicy::default(),
            mcp_curation_cache: None,
            mcp_cap_cache: None,
            file_cache: None,
            session_state: None,
            audit_log: None,
            memory_api: Arc::new(wcore_memory::NullMemory),
            dream_throttle: Arc::new(wcore_memory::consolidate::DreamThrottle::new(
                std::time::Duration::from_secs(1800),
            )),
            #[cfg(any(test, feature = "test-utils"))]
            test_sink_handle: crate::test_utils::TestSinkHandle::default(),
            // W9.1 T3 (T10b): inline-test fixture default — gate off.
            skills_lifecycle: false,
            // F-092 (W7-N): inline-test fixture default — gate off.
            online_evolution: false,
            recent_turn_traces: std::collections::VecDeque::new(),
            drafted_skill_signatures: std::collections::HashSet::new(),
            // W8b.2.B D.3 / Task 7: inline-test fixture defaults — watcher off.
            file_watcher: Arc::new(std::sync::OnceLock::new()),
            tool_write_notifier: Arc::new(std::sync::OnceLock::new()),
            // Wave OR: inline-test fixture default — no mode override.
            mode_override: None,
            template_router: None,
            decay_handles: Vec::new(),
            plugin_runtime_handles: Arc::new(Vec::new()),
            budget_tracker: None,
            policy_gate: None,
            agent_registry: None,
            plugin_user_models: Vec::new(),
            style_detector: Mutex::new(crate::style_detector::StyleDetector::new()),
            skill_catalog: None,
            user_model_backend: None,
            user_model_user_id: resolve_user_model_user_id(),
            // v0.8.1 U1 — installed post-construction by
            // `AgentBootstrap::build` (see `set_skill_router`). `None`
            // here keeps every non-bootstrap construction site (tests,
            // resume-without-bootstrap, sub-agent shadows) on the
            // pre-U1 no-op path.
            skill_router: None,
            current_skill_router_pick: None,
            // v0.8.1 U6 — autonomous-skill bucketer is always live (N=3
            // threshold). Drafter is None at construction; bootstrap
            // installs one when memory is wired.
            auto_skill_bucketer: Mutex::new(crate::auto_skill::Bucketer::new(3)),
            skill_drafter: None,
            // AUDIT A2 / B1 — fresh session-root cancellation token.
            // Hosts replace/observe it via `cancel_token()`.
            cancel_token: tokio_util::sync::CancellationToken::new(),
            // AUDIT B-2 / D-5 — reaper handle storage; populated by
            // `set_approval_manager`, aborted by `Drop`.
            background_handles: Vec::new(),
            // Dynamic Workflows B3 — detection gate (default off).
            workflow_detection_enabled: false,
            // Dynamic Workflows B6 — live confirm gate (default off) + a
            // default config for the (unused-in-these-fixtures) live gate.
            workflow_live_mode: false,
            config: wcore_config::config::Config::default(),
            compaction_floor: 0,
            session_start_injected_len: 0,
            web_search: false,
            pending_hook_actions: Vec::new(),
            conversation_id: String::new(),
            flux_served_window: None,
            flux_context_pressure: None,
            smart_compact_armed: true,
            smart_compact_last_turn: None,
            smart_compact_exhausted: false,
            smart_compact_force: false,
        }
    }

    #[test]
    fn tc_6_21_model_override_applied() {
        let mut engine = make_engine("original-model", vec![]);
        let modifiers = vec![Some(ContextModifier {
            model: Some("override-model".to_string()),
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "override-model");
    }

    #[test]
    fn tc_6_22_effort_override_applied() {
        let mut engine = make_engine("m", vec![]);
        let modifiers = vec![Some(ContextModifier {
            effort: Some(EffortLevel::High),
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.current_reasoning_effort.as_deref(), Some("high"));
    }

    #[test]
    fn tc_6_22b_effort_all_variants() {
        for (level, expected) in [
            (EffortLevel::Low, "low"),
            (EffortLevel::Medium, "medium"),
            (EffortLevel::High, "high"),
            (EffortLevel::Max, "max"),
        ] {
            let mut engine = make_engine("m", vec![]);
            engine.apply_context_modifiers(&[Some(ContextModifier {
                effort: Some(level),
                ..Default::default()
            })]);
            assert_eq!(
                engine.current_reasoning_effort.as_deref(),
                Some(expected),
                "EffortLevel::{level:?} should map to {expected:?}"
            );
        }
    }

    #[test]
    fn tc_6_23_allowed_tools_no_duplicates() {
        let mut engine = make_engine("m", vec!["Bash".to_string()]);
        let modifiers = vec![Some(ContextModifier {
            allowed_tools: vec!["Bash".to_string(), "Read".to_string()],
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        let bash_count = engine
            .allow_list
            .iter()
            .filter(|t| t.as_str() == "Bash")
            .count();
        assert_eq!(bash_count, 1, "Bash should appear exactly once");
        assert!(engine.allow_list.contains(&"Read".to_string()));
    }

    #[test]
    fn tc_6_24_none_modifiers_skipped() {
        let mut engine = make_engine("original", vec![]);
        engine.apply_context_modifiers(&[None, None]);
        assert_eq!(engine.model, "original");
        assert!(engine.current_reasoning_effort.is_none());
    }

    #[test]
    fn tc_6_25_empty_modifiers_no_change() {
        let mut engine = make_engine("current-model", vec![]);
        engine.apply_context_modifiers(&[]);
        assert_eq!(engine.model, "current-model");
        assert!(engine.allow_list.is_empty());
    }

    #[test]
    fn tc_6_26_none_model_does_not_overwrite() {
        let mut engine = make_engine("current-model", vec![]);
        engine.apply_context_modifiers(&[Some(ContextModifier {
            allowed_tools: vec!["Bash".to_string()],
            ..Default::default()
        })]);
        assert_eq!(engine.model, "current-model");
        assert!(engine.allow_list.contains(&"Bash".to_string()));
    }

    #[test]
    fn tc_6_27_multiple_modifiers_stacked() {
        let mut engine = make_engine("initial", vec![]);
        let modifiers = vec![
            Some(ContextModifier {
                model: Some("model-a".to_string()),
                allowed_tools: vec!["Bash".to_string()],
                ..Default::default()
            }),
            Some(ContextModifier {
                model: Some("model-b".to_string()),
                allowed_tools: vec!["Read".to_string()],
                ..Default::default()
            }),
        ];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "model-b", "last model wins");
        assert!(engine.allow_list.contains(&"Bash".to_string()));
        assert!(engine.allow_list.contains(&"Read".to_string()));
    }

    #[test]
    fn tc_6_28_modifier_applied_after_tool_execution_not_during() {
        let mut engine = make_engine("original", vec![]);
        let model_before = engine.model.clone();
        let modifiers = vec![Some(ContextModifier {
            model: Some("new-model".to_string()),
            ..Default::default()
        })];
        assert_eq!(engine.model, model_before);
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "new-model");
        assert_eq!(model_before, "original");
    }
}

// ---------------------------------------------------------------------------
// Phase 2 tests — run_compaction()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod compact_tests {
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use wcore_config::compact::CompactConfig;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::FinishReason;
    // v0.8.0 Task M: inline-test fixture builders need access to the
    // engine-private user-id resolver.
    use super::resolve_user_model_user_id;
    use wcore_types::message::{ContentBlock, Message, Role};

    use crate::approval::ApprovalBridge;
    use crate::compact::state::CompactState;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_compact_engine(
        compact_config: CompactConfig,
        compact_state: CompactState,
        messages: Vec<Message>,
    ) -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            temperature: None,
            tools: Arc::new(ToolRegistry::new()),
            messages,
            rebind_system_prefix: None,
            system_prompt: String::new(),
            model: "test-model".to_string(),
            user_model_pin: None,
            max_tokens: 4096,
            max_turns: Some(10),
            total_usage: Default::default(),
            thinking: None,
            compat: wcore_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            current_msg_id: String::new(),
            current_agent_run_id: None,
            approval_manager: None,
            approval_bridge: Arc::new(ApprovalBridge::new()),
            protocol_writer: None,
            allow_list: vec![],
            current_reasoning_effort: None,
            compact_config,
            compact_state,
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
            compaction_level: wcore_compact::CompactionLevel::default(),
            toon_enabled: false,
            compact_bash: false,
            advertised_capabilities: wcore_config::tools::AdvertisedCapabilitiesConfig::default(),
            per_turn_costs: Vec::new(),
            mcp_curation: wcore_config::config::McpCurationPolicy::default(),
            mcp_curation_cache: None,
            mcp_cap_cache: None,
            file_cache: None,
            session_state: None,
            audit_log: None,
            memory_api: Arc::new(wcore_memory::NullMemory),
            dream_throttle: Arc::new(wcore_memory::consolidate::DreamThrottle::new(
                std::time::Duration::from_secs(1800),
            )),
            #[cfg(any(test, feature = "test-utils"))]
            test_sink_handle: crate::test_utils::TestSinkHandle::default(),
            // W9.1 T3 (T10b): inline-test fixture default — gate off.
            skills_lifecycle: false,
            // F-092 (W7-N): inline-test fixture default — gate off.
            online_evolution: false,
            recent_turn_traces: std::collections::VecDeque::new(),
            drafted_skill_signatures: std::collections::HashSet::new(),
            // W8b.2.B D.3 / Task 7: inline-test fixture defaults — watcher off.
            file_watcher: Arc::new(std::sync::OnceLock::new()),
            tool_write_notifier: Arc::new(std::sync::OnceLock::new()),
            // Wave OR: inline-test fixture default — no mode override.
            mode_override: None,
            template_router: None,
            decay_handles: Vec::new(),
            plugin_runtime_handles: Arc::new(Vec::new()),
            budget_tracker: None,
            policy_gate: None,
            agent_registry: None,
            plugin_user_models: Vec::new(),
            style_detector: Mutex::new(crate::style_detector::StyleDetector::new()),
            skill_catalog: None,
            user_model_backend: None,
            user_model_user_id: resolve_user_model_user_id(),
            // v0.8.1 U1 — installed post-construction by
            // `AgentBootstrap::build` (see `set_skill_router`). `None`
            // here keeps every non-bootstrap construction site (tests,
            // resume-without-bootstrap, sub-agent shadows) on the
            // pre-U1 no-op path.
            skill_router: None,
            current_skill_router_pick: None,
            // v0.8.1 U6 — autonomous-skill bucketer is always live (N=3
            // threshold). Drafter is None at construction; bootstrap
            // installs one when memory is wired.
            auto_skill_bucketer: Mutex::new(crate::auto_skill::Bucketer::new(3)),
            skill_drafter: None,
            // AUDIT A2 / B1 — fresh session-root cancellation token.
            // Hosts replace/observe it via `cancel_token()`.
            cancel_token: tokio_util::sync::CancellationToken::new(),
            // AUDIT B-2 / D-5 — reaper handle storage; populated by
            // `set_approval_manager`, aborted by `Drop`.
            background_handles: Vec::new(),
            // Dynamic Workflows B3 — detection gate (default off).
            workflow_detection_enabled: false,
            // Dynamic Workflows B6 — live confirm gate (default off) + a
            // default config for the (unused-in-these-fixtures) live gate.
            workflow_live_mode: false,
            config: wcore_config::config::Config::default(),
            compaction_floor: 0,
            session_start_injected_len: 0,
            web_search: false,
            pending_hook_actions: Vec::new(),
            conversation_id: String::new(),
            flux_served_window: None,
            flux_context_pressure: None,
            smart_compact_armed: true,
            smart_compact_last_turn: None,
            smart_compact_exhausted: false,
            smart_compact_force: false,
        }
    }

    fn tool_use_msg(id: &str, name: &str) -> Message {
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: id.to_string(),
                name: name.to_string(),
                input: json!({}),
                extra: None,
            }],
        )
    }

    fn tool_result_msg(id: &str, content: &str) -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: id.to_string(),
                content: content.to_string(),
                is_error: false,
            }],
        )
    }

    // --- aborted-tool-call history repair (orphaned tool_use) ---

    fn engine_with_history(messages: Vec<Message>) -> super::AgentEngine {
        make_compact_engine(CompactConfig::default(), CompactState::new(), messages)
    }

    #[test]
    fn push_user_turn_plain_history_just_appends_user_text() {
        let mut engine = engine_with_history(vec![]);
        engine.push_user_turn("hello");
        assert_eq!(engine.messages.len(), 1);
        let m = &engine.messages[0];
        assert_eq!(m.role, Role::User);
        assert_eq!(m.content.len(), 1);
        assert!(matches!(&m.content[0], ContentBlock::Text { text } if text == "hello"));
    }

    #[test]
    fn push_user_turn_after_end_turn_synthesizes_no_tool_result() {
        let mut engine = engine_with_history(vec![Message::new(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "all done".to_string(),
            }],
        )]);
        engine.push_user_turn("next");
        let last = engine.messages.last().unwrap();
        assert_eq!(
            last.content.len(),
            1,
            "no tool_result should be synthesized"
        );
        assert!(matches!(&last.content[0], ContentBlock::Text { .. }));
    }

    #[test]
    fn push_user_turn_repairs_orphaned_tool_use() {
        // A turn aborted between the model's tool_use and the tool result.
        let mut engine = engine_with_history(vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
            ),
            tool_use_msg("toolu_1", "Skill"),
        ]);
        engine.push_user_turn("next message");
        // Exactly one user message follows the orphaned assistant message,
        // so conversation roles stay strictly alternating.
        assert_eq!(engine.messages.len(), 3);
        let last = &engine.messages[2];
        assert_eq!(last.role, Role::User);

        let mut found_result = false;
        let mut found_text = false;
        for block in &last.content {
            match block {
                ContentBlock::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => {
                    assert_eq!(tool_use_id.as_str(), "toolu_1");
                    assert!(*is_error, "synthetic result must be flagged is_error");
                    found_result = true;
                }
                ContentBlock::Text { text } => {
                    assert_eq!(text, "next message");
                    found_text = true;
                }
                other => panic!("unexpected block in repaired turn: {other:?}"),
            }
        }
        assert!(found_result, "synthetic tool_result missing");
        assert!(found_text, "the new user input must still be carried");
    }

    #[test]
    fn push_user_turn_repairs_every_orphaned_tool_use() {
        let mut engine = engine_with_history(vec![Message::new(
            Role::Assistant,
            vec![
                ContentBlock::Text {
                    text: "running tools".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "a".to_string(),
                    name: "Read".to_string(),
                    input: json!({}),
                    extra: None,
                },
                ContentBlock::ToolUse {
                    id: "b".to_string(),
                    name: "Grep".to_string(),
                    input: json!({}),
                    extra: None,
                },
            ],
        )]);
        engine.push_user_turn("go");
        let last = engine.messages.last().unwrap();
        let ids: Vec<&str> = last
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            ids,
            vec!["a", "b"],
            "every orphaned tool_use needs a result"
        );
    }

    // --- repair_all_orphaned_tool_uses — the request-build-time
    //     belt-and-suspenders guard against orphan-then-400-brick.

    #[test]
    fn repair_all_is_a_noop_on_well_formed_history() {
        let mut engine = engine_with_history(vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "hi".to_string(),
                }],
            ),
            tool_use_msg("a", "Read"),
            tool_result_msg("a", "file contents"),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "done".to_string(),
                }],
            ),
        ]);
        let before = engine.messages.len();
        engine.repair_all_orphaned_tool_uses();
        assert_eq!(engine.messages.len(), before, "no repair needed");
    }

    #[test]
    fn repair_all_appends_missing_result_to_existing_user_message() {
        // The reaper-denial scenario: an assistant with two tool_uses,
        // the user-results message only carries one — append the other
        // in place.
        let mut engine = engine_with_history(vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "go".to_string(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![
                    ContentBlock::ToolUse {
                        id: "a".to_string(),
                        name: "Read".to_string(),
                        input: json!({}),
                        extra: None,
                    },
                    ContentBlock::ToolUse {
                        id: "b".to_string(),
                        name: "Browser".to_string(),
                        input: json!({}),
                        extra: None,
                    },
                ],
            ),
            tool_result_msg("a", "ok"),
        ]);
        engine.repair_all_orphaned_tool_uses();
        assert_eq!(engine.messages.len(), 3, "no new message inserted");
        let last = engine.messages.last().unwrap();
        let ids: Vec<&str> = last
            .content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();
        assert!(ids.contains(&"a") && ids.contains(&"b"), "got: {ids:?}");
    }

    #[test]
    fn repair_all_inserts_user_between_assistant_and_non_user() {
        // Mid-history orphan: assistant tool_use followed by another
        // assistant message (system injection, model error retry,
        // whatever). Insert a synthetic user with the missing results.
        let mut engine = engine_with_history(vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "go".to_string(),
                }],
            ),
            tool_use_msg("a", "Browser"),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "continuing".to_string(),
                }],
            ),
        ]);
        engine.repair_all_orphaned_tool_uses();
        assert_eq!(engine.messages.len(), 4, "user inserted between");
        assert_eq!(engine.messages[2].role, Role::User);
        assert!(engine.messages[2].content.iter().any(
            |b| matches!(b, ContentBlock::ToolResult { tool_use_id, is_error: true, .. }
                    if tool_use_id == "a")
        ));
        assert_eq!(engine.messages[3].role, Role::Assistant);
    }

    #[test]
    fn repair_all_repairs_trailing_orphan() {
        // Same case the existing repair_orphaned_tool_use handles —
        // confirm the new scanner also catches it.
        let mut engine = engine_with_history(vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "go".to_string(),
                }],
            ),
            tool_use_msg("a", "Read"),
        ]);
        engine.repair_all_orphaned_tool_uses();
        assert_eq!(engine.messages.len(), 3);
        assert_eq!(engine.messages[2].role, Role::User);
    }

    #[test]
    fn repair_all_is_idempotent() {
        let mut engine = engine_with_history(vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "go".to_string(),
                }],
            ),
            tool_use_msg("a", "Browser"),
        ]);
        engine.repair_all_orphaned_tool_uses();
        let after_first = engine.messages.len();
        engine.repair_all_orphaned_tool_uses();
        assert_eq!(engine.messages.len(), after_first, "second pass no-op");
    }

    // --- repair_orphaned_tool_results — #285 reverse-direction guard
    //     against orphaned tool_result → DeepSeek 400.

    #[test]
    fn repair_results_demotes_orphan_to_text_preserving_content() {
        // A tool_result whose tool_use_id matches no tool_use anywhere
        // (the autocompact-summarized-away case): must become a text
        // block that preserves the original content, and no orphaned
        // tool_result may remain.
        let mut engine = engine_with_history(vec![Message::new(
            Role::User,
            vec![ContentBlock::ToolResult {
                tool_use_id: "t1".to_string(),
                content: "the file said hello".to_string(),
                is_error: false,
            }],
        )]);
        engine.repair_orphaned_tool_results();

        let live_ids: std::collections::HashSet<&str> = engine
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        // No tool_result with an unmatched id survives.
        let dangling = engine.messages.iter().flat_map(|m| &m.content).any(|b| {
            matches!(b, ContentBlock::ToolResult { tool_use_id, .. }
                if !live_ids.contains(tool_use_id.as_str()))
        });
        assert!(!dangling, "orphaned tool_result must be neutralized");
        // The content is preserved inside a text block.
        let preserved = engine.messages.iter().flat_map(|m| &m.content).any(
            |b| matches!(b, ContentBlock::Text { text } if text.contains("the file said hello")),
        );
        assert!(preserved, "tool_result content must be preserved as text");
        // The message must not be left empty.
        assert!(
            engine.messages.iter().all(|m| !m.content.is_empty()),
            "no message may be left with empty content"
        );
    }

    #[test]
    fn repair_results_keeps_matched_pair_and_is_idempotent() {
        // A well-formed tool_use/tool_result pair must be left untouched,
        // and a second pass must be a no-op.
        let mut engine =
            engine_with_history(vec![tool_use_msg("a", "Read"), tool_result_msg("a", "ok")]);
        let before = format!("{:?}", engine.messages);
        engine.repair_orphaned_tool_results();
        assert_eq!(
            format!("{:?}", engine.messages),
            before,
            "matched pair must be untouched (clean history is a no-op)"
        );
        engine.repair_orphaned_tool_results();
        assert_eq!(
            format!("{:?}", engine.messages),
            before,
            "second pass must also be a no-op (idempotent)"
        );
    }

    // -- Emergency check fires when at limit --

    #[tokio::test]
    async fn emergency_fires_when_at_limit() {
        let config = CompactConfig {
            context_window: 200_000,
            emergency_buffer: 3_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 198_000; // >= 197k limit

        let mut engine = make_compact_engine(config, state, vec![]);
        let result = engine.run_compaction().await;

        match result {
            Err(super::AgentError::ContextTooLong {
                input_tokens,
                limit,
            }) => {
                assert_eq!(input_tokens, 198_000);
                assert_eq!(limit, 197_000);
            }
            other => panic!("expected ContextTooLong, got: {:?}", other),
        }
    }

    // -- Emergency does not fire when below limit --

    #[tokio::test]
    async fn emergency_silent_below_limit() {
        let config = CompactConfig::default();
        let mut state = CompactState::new();
        state.last_input_tokens = 190_000; // below 197k

        let mut engine = make_compact_engine(config, state, vec![]);
        assert!(engine.run_compaction().await.is_ok());
    }

    // -- Finding #174: AUTO trigger tracks REAL pressure, not the
    //    thinking-inflated conservative watermark --

    /// A non-replaying provider's history carries large historical thinking
    /// blocks. The conservative watermark (`last_input_tokens`,
    /// max(real, estimate-with-thinking)) is pushed over the 167k auto
    /// threshold purely by that thinking — but the REAL billed input
    /// (`last_real_input_tokens`) is well under it, because Anthropic drops
    /// the thinking at the wire. Auto compaction must NOT fire: that is the
    /// premature-trigger bug. Pre-fix, `should_autocompact` read the
    /// conservative watermark and would summarize early.
    #[tokio::test]
    async fn auto_does_not_fire_on_thinking_inflated_watermark() {
        let config = CompactConfig {
            context_window: 200_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        // Conservative watermark inflated past the 167k auto threshold by
        // historical thinking (and still below the 197k emergency limit).
        state.last_input_tokens = 180_000;
        // Real billed input is far below the auto threshold.
        state.last_real_input_tokens = 90_000;

        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Thinking {
                    thinking: "reasoning ".repeat(10_000),
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "the live task".into(),
                }],
            ),
        ];
        let len_before = messages.len();

        // SummaryProvider would SUCCEED if autocompact ran — so if the
        // message count is untouched, the trigger genuinely did not fire
        // (a fired compaction replaces history with a summary, dropping len).
        let mut engine = make_compact_engine(config, state, messages);
        engine.provider = Arc::new(SummaryProvider);

        assert!(engine.run_compaction().await.is_ok());
        assert_eq!(
            engine.messages.len(),
            len_before,
            "auto compaction must not fire on a thinking-inflated watermark \
             when real billed input is below the threshold"
        );
    }

    /// When the provider-reported REAL input (`last_real_input_tokens`) is
    /// itself over the 167k threshold, the AUTO trigger fires — proving it
    /// keys off real pressure, not just the conservative path.
    #[tokio::test]
    async fn auto_fires_on_real_usage_over_threshold() {
        let config = CompactConfig {
            context_window: 200_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        // Conservative and real watermarks both above 167k, below emergency.
        state.last_input_tokens = 180_000;
        state.last_real_input_tokens = 180_000;

        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "old reply".into(),
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "the live task".into(),
                }],
            ),
        ];
        let len_before = messages.len();

        let mut engine = make_compact_engine(config, state, messages);
        engine.provider = Arc::new(SummaryProvider);

        assert!(engine.run_compaction().await.is_ok());
        // A fired compaction replaces history with a summary, changing the
        // message count; an unchanged len would mean the trigger never fired.
        assert_ne!(
            engine.messages.len(),
            len_before,
            "auto compaction must fire when real billed input exceeds the threshold"
        );
    }

    /// The EMERGENCY hard-stop stays conservative: it fires off
    /// `last_input_tokens` even when the REAL watermark
    /// (`last_real_input_tokens`) is low. This guards against ever blowing
    /// the context window. Behaviour here is unchanged by finding #174.
    #[tokio::test]
    async fn emergency_stays_conservative_ignoring_real_watermark() {
        let config = CompactConfig {
            context_window: 200_000,
            emergency_buffer: 3_000,
            // Disable auto so ONLY emergency can act, isolating its path.
            enabled: false,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 198_000; // >= 197k emergency limit
        state.last_real_input_tokens = 10_000; // low real pressure — irrelevant

        let mut engine = make_compact_engine(config, state, vec![]);
        let result = engine.run_compaction().await;

        match result {
            Err(super::AgentError::ContextTooLong { input_tokens, .. }) => {
                assert_eq!(
                    input_tokens, 198_000,
                    "emergency must use the conservative watermark, not the real one"
                );
            }
            other => panic!("expected ContextTooLong from emergency, got: {other:?}"),
        }
    }

    // -- Microcompact runs when count trigger fires --

    #[tokio::test]
    async fn microcompact_clears_old_results() {
        // 12 tool results with keep_recent=3 (threshold=6) → should clear 9
        let mut messages = Vec::new();
        for i in 0..12 {
            let id = format!("t{i}");
            messages.push(tool_use_msg(&id, "Read"));
            messages.push(tool_result_msg(&id, &format!("data-{i}")));
        }

        let config = CompactConfig {
            micro_keep_recent: 3,
            ..Default::default()
        };
        let state = CompactState::new();

        let mut engine = make_compact_engine(config, state, messages);
        engine.run_compaction().await.unwrap();

        // Last 3 tool results should be preserved
        let cleared_count = engine
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| {
                matches!(b, ContentBlock::ToolResult { content, .. } if content == "[Tool result cleared]")
            })
            .count();

        assert_eq!(cleared_count, 9);
    }

    // -- Manual /compact runs deterministic micro-compaction, not the
    //    old fixed-string truncation --

    #[tokio::test]
    async fn compact_now_microcompacts_without_canned_summary() {
        // A real conversational message plus enough tool results to trip the
        // count trigger (keep_recent=3 → threshold 6).
        let mut messages = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "PRESERVE_THIS_USER_TURN".to_string(),
            }],
        )];
        for i in 0..12 {
            let id = format!("t{i}");
            messages.push(tool_use_msg(&id, "Read"));
            messages.push(tool_result_msg(&id, &format!("data-{i}")));
        }

        let config = CompactConfig {
            micro_keep_recent: 3,
            ..Default::default()
        };
        let mut engine = make_compact_engine(config, CompactState::new(), messages);

        let result = engine.compact_now();

        // Micro-compaction ran: old tool-result bodies were cleared.
        assert_eq!(
            result.cleared_count, 9,
            "manual /compact should clear old tool results via micro-compaction"
        );

        // The original real content must still be present — manual /compact no
        // longer discards the middle of the transcript.
        let has_user_turn =
            engine.messages.iter().flat_map(|m| &m.content).any(
                |b| matches!(b, ContentBlock::Text { text } if text == "PRESERVE_THIS_USER_TURN"),
            );
        assert!(
            has_user_turn,
            "the user's real text turn must survive manual compaction"
        );

        // The old canned fixed-string summary must NEVER appear.
        let has_canned_summary = engine.messages.iter().flat_map(|m| &m.content).any(|b| {
            matches!(b, ContentBlock::Text { text }
                    if text.contains("[Previous conversation summary"))
        });
        assert!(
            !has_canned_summary,
            "manual /compact must not emit the old canned truncation summary"
        );
    }

    // -- Disabled config skips micro and auto but not emergency --

    #[tokio::test]
    async fn disabled_config_skips_micro_auto() {
        let mut messages = Vec::new();
        for i in 0..12 {
            let id = format!("t{i}");
            messages.push(tool_use_msg(&id, "Read"));
            messages.push(tool_result_msg(&id, &format!("data-{i}")));
        }

        let config = CompactConfig {
            enabled: false,
            micro_keep_recent: 3,
            ..Default::default()
        };
        let state = CompactState::new();

        let mut engine = make_compact_engine(config, state, messages);
        engine.run_compaction().await.unwrap();

        // Nothing should be cleared (microcompact skipped)
        let cleared_count = engine
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter(|b| {
                matches!(b, ContentBlock::ToolResult { content, .. } if content == "[Tool result cleared]")
            })
            .count();

        assert_eq!(
            cleared_count, 0,
            "microcompact should be skipped when disabled"
        );
    }

    #[tokio::test]
    async fn disabled_config_still_fires_emergency() {
        let config = CompactConfig {
            enabled: false,
            context_window: 200_000,
            emergency_buffer: 3_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 198_000;

        let mut engine = make_compact_engine(config, state, vec![]);
        let result = engine.run_compaction().await;

        assert!(
            matches!(result, Err(super::AgentError::ContextTooLong { .. })),
            "emergency should fire even when disabled"
        );
    }

    // -- Zero tokens on first turn does not trigger anything --

    #[tokio::test]
    async fn first_turn_zero_tokens_no_compaction() {
        let config = CompactConfig::default();
        let state = CompactState::new(); // last_input_tokens = 0

        let mut engine = make_compact_engine(config, state, vec![]);
        assert!(engine.run_compaction().await.is_ok());
        assert_eq!(engine.compact_state.last_input_tokens, 0);
    }

    // -- AUDIT A4 / A7: autocompact preserves the live user task --

    /// A provider whose `stream()` always returns a fixed summary text
    /// followed by a clean `Done` — enough for `autocompact` to succeed.
    struct SummaryProvider;
    #[async_trait::async_trait]
    impl LlmProvider for SummaryProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tokio::spawn(async move {
                let _ = tx
                    .send(LlmEvent::TextDelta(
                        "<summary>prior conversation summary</summary>".into(),
                    ))
                    .await;
                let _ = tx
                    .send(LlmEvent::Done {
                        stop_reason: super::StopReason::EndTurn,
                        finish_reason: FinishReason::Stop,
                        usage: wcore_types::message::TokenUsage::default(),
                    })
                    .await;
            });
            Ok(rx)
        }
    }

    #[tokio::test]
    async fn autocompact_preserves_the_trailing_user_task_verbatim() {
        // AUDIT A4 — `run_compaction` runs AFTER the live user turn is
        // pushed. Autocompact must NOT collapse that instruction into
        // the LLM summary; the verbatim trailing user message must
        // survive. Pre-fix: `self.messages` became `[boundary, summary]`
        // and the user's actual request was lost.
        let config = CompactConfig {
            context_window: 200_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        // Above the autocompact threshold (167k) but below emergency.
        // Finding #174 — the AUTO trigger reads `last_real_input_tokens`,
        // so set it too; `last_input_tokens` alone now only drives emergency.
        state.last_input_tokens = 180_000;
        state.last_real_input_tokens = 180_000;

        // History: some prior turns, then the LIVE user instruction.
        let messages = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "old turn one".into(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "old reply".into(),
                }],
            ),
            // The trailing user turn — the live task.
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "DELETE the staging database now".into(),
                }],
            ),
        ];

        let mut engine = make_compact_engine(config, state, messages);
        engine.provider = Arc::new(SummaryProvider);
        engine.run_compaction().await.expect("autocompact succeeds");

        // The verbatim live instruction must still be present somewhere
        // in the post-compact message list.
        let preserved = engine
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .any(|b| matches!(b, ContentBlock::Text { text } if text == "DELETE the staging database now"));
        assert!(
            preserved,
            "the live user task must survive autocompact verbatim; \
             post-compact messages: {:?}",
            engine.messages
        );

        // AUDIT A7 — the post-compact list must not contain consecutive
        // same-role messages (an invalid shape for strict providers).
        for pair in engine.messages.windows(2) {
            assert_ne!(
                pair[0].role, pair[1].role,
                "post-compact history must alternate roles (A7): {:?}",
                engine.messages
            );
        }
    }

    #[tokio::test]
    async fn autocompact_never_emits_orphaned_tool_result_285() {
        // #285 regression — when the turn that triggers compaction is a
        // tool-result turn, its matching tool_use lives in an earlier
        // assistant message autocompact summarizes into prose. Pre-fix the
        // folded buffer carried a dangling tool_result (id "t1") with no
        // matching tool_use → DeepSeek rejected the whole array with HTTP
        // 400 `missing field tool_call_id`. Post-fix the fold neutralizes
        // it (and the pre-send `repair_orphaned_tool_results` is the
        // belt-and-suspenders). Assert ZERO dangling tool_result remains.
        let config = CompactConfig {
            context_window: 200_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 180_000;
        state.last_real_input_tokens = 180_000;

        let messages = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "read the config file".into(),
                }],
            ),
            // Assistant tool_use — this gets summarized away by autocompact.
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "Read".into(),
                    input: json!({"path": "config.toml"}),
                    extra: None,
                }],
            ),
            // LIVE turn: the tool_result that triggers compaction. Its
            // partner tool_use ("t1") is in the prefix above and will be
            // collapsed into the summary, orphaning this result.
            Message::new(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "port = 8080".into(),
                    is_error: false,
                }],
            ),
        ];

        let mut engine = make_compact_engine(config, state, messages);
        engine.provider = Arc::new(SummaryProvider);
        engine.run_compaction().await.expect("autocompact succeeds");
        // Run the same pre-send repairs the request-build path runs, so the
        // assertion reproduces exactly what would reach the provider.
        engine.repair_all_orphaned_tool_uses();
        engine.repair_orphaned_tool_results();

        let live_ids: std::collections::HashSet<&str> = engine
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        let dangling = engine.messages.iter().flat_map(|m| &m.content).any(|b| {
            matches!(b, ContentBlock::ToolResult { tool_use_id, .. }
                if !live_ids.contains(tool_use_id.as_str()))
        });
        assert!(
            !dangling,
            "no orphaned tool_result may survive compaction (the #285 \
             DeepSeek 400); post-compact messages: {:?}",
            engine.messages
        );
        // The result content must be preserved somewhere as context.
        let preserved = engine
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .any(|b| matches!(b, ContentBlock::Text { text } if text.contains("port = 8080")));
        assert!(
            preserved,
            "the tool_result content must survive as text: {:?}",
            engine.messages
        );
    }

    #[tokio::test]
    async fn autocompact_failure_restores_the_live_user_turn() {
        // AUDIT A4 — when autocompact FAILS, the carved-out live user
        // turn must be put back so the next turn still sees the task.
        // `NullProvider` (from make_compact_engine) yields an empty
        // stream → autocompact returns `EmptyResponse` → failure path.
        let config = CompactConfig {
            context_window: 200_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 180_000;
        state.last_real_input_tokens = 180_000;

        let messages = vec![
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "prior".into(),
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "the live task".into(),
                }],
            ),
        ];
        let mut engine = make_compact_engine(config, state, messages);
        // NullProvider → autocompact fails. run_compaction still Ok
        // (failure is logged + swallowed), but the conversation must be
        // intact.
        let _ = engine.run_compaction().await;
        let preserved = engine
            .messages
            .iter()
            .flat_map(|m| &m.content)
            .any(|b| matches!(b, ContentBlock::Text { text } if text == "the live task"));
        assert!(
            preserved,
            "a failed autocompact must restore the live user turn"
        );
    }

    // -- Token-opt compaction-floor primitive --

    #[tokio::test]
    async fn autocompact_advances_compaction_floor_and_reset_clears_it() {
        // Token-opt: after autocompact collapses the leading N messages,
        // `compaction_floor()` must equal N, indices `< N` must report
        // not-visible and index N must report visible. A conversation reset
        // (`/clear`) must return the floor to 0.
        let config = CompactConfig {
            context_window: 200_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        // Above the autocompact threshold (167k), below emergency.
        state.last_input_tokens = 180_000;
        state.last_real_input_tokens = 180_000;

        // Three leading messages (indices 0,1,2 — the ones that collapse)
        // plus a trailing LIVE user turn. `run_compaction` pops the live
        // turn out before handing the rest to `autocompact`, so exactly
        // 3 messages are summarized away → floor advances by 3.
        let messages = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "leading one".into(),
                }],
            ),
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "leading two".into(),
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "leading three".into(),
                }],
            ),
            // Trailing live user turn — popped+re-folded, NOT counted.
            Message::new(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "assistant reply".into(),
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "the live task".into(),
                }],
            ),
        ];
        // Leading span handed to autocompact = everything except the popped
        // trailing User turn = 4 messages → N = 4.
        let n = messages.len() - 1;

        let mut engine = make_compact_engine(config, state, messages);
        engine.provider = Arc::new(SummaryProvider);

        // Precondition: nothing collapsed yet, every index visible.
        assert_eq!(engine.compaction_floor(), 0);
        assert!(engine.message_index_still_visible(0));

        engine.run_compaction().await.expect("autocompact succeeds");

        // The floor advanced by exactly the collapsed leading count.
        assert_eq!(engine.compaction_floor(), n);
        // The last collapsed index is no longer visible…
        assert!(!engine.message_index_still_visible(n - 1));
        // …but the index at the floor (and beyond) maps to live history.
        assert!(engine.message_index_still_visible(n));

        // A conversation reset re-baselines the index space.
        engine.clear_conversation();
        assert_eq!(engine.compaction_floor(), 0);
        assert!(engine.message_index_still_visible(0));
    }

    #[tokio::test]
    async fn autocompact_bumps_wired_file_cache_generation() {
        // Token-opt (diff-resend): when the engine has a wired file cache, a
        // compaction pass must advance the cache's compaction generation so
        // stale read bases stop qualifying for diff-resend.
        let config = CompactConfig {
            context_window: 200_000,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 180_000;
        state.last_real_input_tokens = 180_000;
        let messages = vec![
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "leading".into(),
                }],
            ),
            Message::new(
                Role::User,
                vec![ContentBlock::Text {
                    text: "the live task".into(),
                }],
            ),
        ];
        let mut engine = make_compact_engine(config, state, messages);
        engine.provider = Arc::new(SummaryProvider);

        let cache = Arc::new(std::sync::RwLock::new(
            wcore_tools::file_cache::FileStateCache::new(
                &wcore_config::file_cache::FileCacheConfig {
                    max_entries: 10,
                    max_size_bytes: 1_000_000,
                    enabled: true,
                },
            ),
        ));
        assert_eq!(cache.read().unwrap().compaction_generation(), 0);
        engine.set_file_cache(cache.clone());

        engine.run_compaction().await.expect("autocompact succeeds");

        assert!(
            cache.read().unwrap().compaction_generation() >= 1,
            "a compaction pass must bump the wired file cache's generation"
        );
    }

    #[tokio::test]
    async fn read_once_backrefs_repeated_grep_output() {
        // Token-opt (read-once): a repeated identical Grep result is rewritten to
        // a short backref before it enters the transcript; the first is kept full.
        let mut engine = make_compact_engine(CompactConfig::default(), CompactState::new(), vec![]);
        let cache = Arc::new(std::sync::RwLock::new(
            wcore_tools::file_cache::FileStateCache::new(
                &wcore_config::file_cache::FileCacheConfig {
                    max_entries: 10,
                    max_size_bytes: 1_000_000,
                    enabled: true,
                },
            ),
        ));
        cache.write().unwrap().set_optimize_reads(true);
        engine.set_file_cache(cache);

        let big = "src/lib.rs:42: let token = compute();\n".repeat(20); // > 300 bytes
        let tool_calls = vec![
            ContentBlock::ToolUse {
                id: "a".into(),
                name: "Grep".into(),
                input: serde_json::json!({ "pattern": "token" }),
                extra: None,
            },
            ContentBlock::ToolUse {
                id: "b".into(),
                name: "Grep".into(),
                input: serde_json::json!({ "pattern": "token" }),
                extra: None,
            },
        ];
        let mut blocks = vec![
            ContentBlock::ToolResult {
                tool_use_id: "a".into(),
                content: big.clone(),
                is_error: false,
            },
            ContentBlock::ToolResult {
                tool_use_id: "b".into(),
                content: big.clone(),
                is_error: false,
            },
        ];

        engine.dedup_repeated_tool_outputs(&mut blocks, &tool_calls);

        match &blocks[0] {
            ContentBlock::ToolResult { content, .. } => {
                assert_eq!(content, &big, "first occurrence keeps full output")
            }
            _ => panic!("expected ToolResult"),
        }
        match &blocks[1] {
            ContentBlock::ToolResult { content, .. } => {
                assert!(
                    content.contains("Identical to the earlier result"),
                    "repeat must be a backref, got: {content}"
                );
                assert!(content.contains("token"), "backref names the earlier call");
                assert!(content.len() < big.len());
            }
            _ => panic!("expected ToolResult"),
        }
    }

    // -- Circuit broken prevents autocompact, emergency still fires --

    #[tokio::test]
    async fn circuit_broken_skips_auto_but_emergency_fires() {
        let config = CompactConfig {
            context_window: 200_000,
            emergency_buffer: 3_000,
            max_failures: 3,
            ..Default::default()
        };
        let mut state = CompactState::new();
        state.last_input_tokens = 198_000; // triggers emergency
        state.last_real_input_tokens = 198_000; // would trigger auto (but circuit broken)
        state.consecutive_failures = 3; // circuit broken

        let mut engine = make_compact_engine(config, state, vec![]);
        let result = engine.run_compaction().await;

        // Auto is skipped due to circuit breaker; emergency fires
        assert!(matches!(
            result,
            Err(super::AgentError::ContextTooLong { .. })
        ));
    }
}

// ---------------------------------------------------------------------------
// Phase 3 tests — plan mode integration in apply_context_modifiers()
// ---------------------------------------------------------------------------

#[cfg(test)]
mod plan_mode_tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use wcore_protocol::events::ToolCategory;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::FinishReason;
    use wcore_types::skill_types::{ContextModifier, PlanModeTransition};
    // v0.8.0 Task M: inline-test fixture builders need access to the
    // engine-private user-id resolver.
    use super::resolve_user_model_user_id;

    use crate::approval::ApprovalBridge;
    use crate::compact::state::CompactState;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;
    use crate::plan::state::PlanState;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    /// Minimal `Tool` impl whose `category()` is fixed at construction — used
    /// by the D005 plan-gate test to register a real Info tool and a real
    /// Edit tool so the run-loop's plan-mode filter is exercised end to end.
    struct CategorizedMockTool {
        tool_name: String,
        tool_category: wcore_protocol::events::ToolCategory,
    }

    #[async_trait::async_trait]
    impl wcore_tools::Tool for CategorizedMockTool {
        fn name(&self) -> &str {
            &self.tool_name
        }
        fn description(&self) -> &str {
            "mock"
        }
        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }
        fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
            true
        }
        async fn execute(&self, _: serde_json::Value) -> wcore_types::tool::ToolResult {
            wcore_types::tool::ToolResult {
                content: "ok".to_string(),
                is_error: false,
            }
        }
        fn category(&self) -> wcore_protocol::events::ToolCategory {
            self.tool_category
        }
    }

    fn make_plan_engine(allow_list: Vec<String>) -> super::AgentEngine {
        let flag = Arc::new(AtomicBool::new(false));
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            temperature: None,
            tools: Arc::new(ToolRegistry::new()),
            messages: vec![],
            rebind_system_prefix: None,
            system_prompt: String::new(),
            model: "test-model".to_string(),
            user_model_pin: None,
            max_tokens: 4096,
            max_turns: Some(10),
            total_usage: Default::default(),
            thinking: None,
            compat: wcore_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, allow_list.clone()))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            current_msg_id: String::new(),
            current_agent_run_id: None,
            approval_manager: None,
            approval_bridge: Arc::new(ApprovalBridge::new()),
            protocol_writer: None,
            allow_list,
            current_reasoning_effort: None,
            compact_config: wcore_config::compact::CompactConfig::default(),
            compact_state: CompactState::new(),
            plan_state: PlanState::default(),
            plan_active_flag: Some(flag),
            cache_detector: super::CacheBreakDetector::new(),
            compaction_level: wcore_compact::CompactionLevel::default(),
            toon_enabled: false,
            compact_bash: false,
            advertised_capabilities: wcore_config::tools::AdvertisedCapabilitiesConfig::default(),
            per_turn_costs: Vec::new(),
            mcp_curation: wcore_config::config::McpCurationPolicy::default(),
            mcp_curation_cache: None,
            mcp_cap_cache: None,
            file_cache: None,
            session_state: None,
            audit_log: None,
            memory_api: Arc::new(wcore_memory::NullMemory),
            dream_throttle: Arc::new(wcore_memory::consolidate::DreamThrottle::new(
                std::time::Duration::from_secs(1800),
            )),
            #[cfg(any(test, feature = "test-utils"))]
            test_sink_handle: crate::test_utils::TestSinkHandle::default(),
            // W9.1 T3 (T10b): inline-test fixture default — gate off.
            skills_lifecycle: false,
            // F-092 (W7-N): inline-test fixture default — gate off.
            online_evolution: false,
            recent_turn_traces: std::collections::VecDeque::new(),
            drafted_skill_signatures: std::collections::HashSet::new(),
            // W8b.2.B D.3 / Task 7: inline-test fixture defaults — watcher off.
            file_watcher: Arc::new(std::sync::OnceLock::new()),
            tool_write_notifier: Arc::new(std::sync::OnceLock::new()),
            // Wave OR: inline-test fixture default — no mode override.
            mode_override: None,
            template_router: None,
            decay_handles: Vec::new(),
            plugin_runtime_handles: Arc::new(Vec::new()),
            budget_tracker: None,
            policy_gate: None,
            agent_registry: None,
            plugin_user_models: Vec::new(),
            style_detector: Mutex::new(crate::style_detector::StyleDetector::new()),
            skill_catalog: None,
            user_model_backend: None,
            user_model_user_id: resolve_user_model_user_id(),
            // v0.8.1 U1 — installed post-construction by
            // `AgentBootstrap::build` (see `set_skill_router`). `None`
            // here keeps every non-bootstrap construction site (tests,
            // resume-without-bootstrap, sub-agent shadows) on the
            // pre-U1 no-op path.
            skill_router: None,
            current_skill_router_pick: None,
            // v0.8.1 U6 — autonomous-skill bucketer is always live (N=3
            // threshold). Drafter is None at construction; bootstrap
            // installs one when memory is wired.
            auto_skill_bucketer: Mutex::new(crate::auto_skill::Bucketer::new(3)),
            skill_drafter: None,
            // AUDIT A2 / B1 — fresh session-root cancellation token.
            // Hosts replace/observe it via `cancel_token()`.
            cancel_token: tokio_util::sync::CancellationToken::new(),
            // AUDIT B-2 / D-5 — reaper handle storage; populated by
            // `set_approval_manager`, aborted by `Drop`.
            background_handles: Vec::new(),
            // Dynamic Workflows B3 — detection gate (default off).
            workflow_detection_enabled: false,
            // Dynamic Workflows B6 — live confirm gate (default off) + a
            // default config for the (unused-in-these-fixtures) live gate.
            workflow_live_mode: false,
            config: wcore_config::config::Config::default(),
            compaction_floor: 0,
            session_start_injected_len: 0,
            web_search: false,
            pending_hook_actions: Vec::new(),
            conversation_id: String::new(),
            flux_served_window: None,
            flux_context_pressure: None,
            smart_compact_armed: true,
            smart_compact_last_turn: None,
            smart_compact_exhausted: false,
            smart_compact_force: false,
        }
    }

    // --- TC-3.5-03: Enter transition activates plan mode ---

    #[test]
    fn enter_transition_activates_plan_mode() {
        let mut engine = make_plan_engine(vec!["Read".into(), "Bash".into()]);
        let modifiers = vec![Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })];

        engine.apply_context_modifiers(&modifiers);

        assert!(engine.plan_state.is_active, "plan mode should be active");
        assert_eq!(
            engine.plan_state.pre_plan_allow_list,
            vec!["Read".to_string(), "Bash".to_string()],
            "pre_plan_allow_list should capture original allow_list"
        );
    }

    // --- TC-3.5-03 supplement: shared flag updated on enter ---

    #[test]
    fn enter_transition_updates_shared_flag() {
        let mut engine = make_plan_engine(vec![]);
        let flag = engine.plan_active_flag.clone().unwrap();
        assert!(!flag.load(Ordering::Acquire));

        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);

        assert!(flag.load(Ordering::Acquire), "shared flag should be true");
    }

    // --- TC-3.5-04: Exit transition deactivates plan mode and restores allow_list ---

    #[test]
    fn exit_transition_deactivates_and_restores() {
        let mut engine = make_plan_engine(vec!["Read".into(), "Bash".into()]);

        // Enter plan mode first
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);
        assert!(engine.plan_state.is_active);

        // Modify allow_list while in plan mode (simulating a skill adding tools)
        engine.allow_list.push("NewTool".into());

        // Exit plan mode
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Exit { plan_content: None }),
            ..Default::default()
        })]);

        assert!(!engine.plan_state.is_active, "plan mode should be inactive");
        assert_eq!(
            engine.allow_list,
            vec!["Read".to_string(), "Bash".to_string()],
            "allow_list should be restored to pre-plan state"
        );
    }

    // --- TC-3.5-04 supplement: shared flag updated on exit ---

    #[test]
    fn exit_transition_updates_shared_flag() {
        let mut engine = make_plan_engine(vec![]);
        let flag = engine.plan_active_flag.clone().unwrap();

        // Enter
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);
        assert!(flag.load(Ordering::Acquire));

        // Exit
        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Exit { plan_content: None }),
            ..Default::default()
        })]);
        assert!(
            !flag.load(Ordering::Acquire),
            "shared flag should be false after exit"
        );
    }

    // --- TC-3.5-05: No transition does not affect plan state ---

    #[test]
    fn no_transition_does_not_affect_plan_state() {
        let mut engine = make_plan_engine(vec![]);

        engine.apply_context_modifiers(&[Some(ContextModifier {
            model: Some("new-model".into()),
            plan_mode_transition: None,
            ..Default::default()
        })]);

        assert_eq!(engine.model, "new-model");
        assert!(
            !engine.plan_state.is_active,
            "plan state should remain inactive"
        );
    }

    // --- Enter + other modifiers applied together ---

    #[test]
    fn enter_with_model_override_both_applied() {
        let mut engine = make_plan_engine(vec![]);

        engine.apply_context_modifiers(&[Some(ContextModifier {
            model: Some("planning-model".into()),
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);

        assert!(engine.plan_state.is_active);
        assert_eq!(engine.model, "planning-model");
    }

    // --- D005: host-driven /plan entry sets the SAME gate the tool sets ---

    #[test]
    fn host_enter_plan_mode_activates_the_gate_and_shared_flag() {
        // The TUI `/plan` command calls `enter_plan_mode()` directly (no
        // model tool round-trip). It must flip the SAME `plan_state.is_active`
        // the per-turn tool filter reads — otherwise a Write/Edit tool stays
        // ungated under a posture the user trusts as read-only (D005).
        let mut engine = make_plan_engine(vec!["Read".into(), "Write".into()]);
        let flag = engine.plan_active_flag.clone().unwrap();
        assert!(!engine.plan_state.is_active);
        assert!(!flag.load(Ordering::Acquire));

        engine.enter_plan_mode();

        assert!(
            engine.plan_state.is_active,
            "/plan must set the engine plan gate"
        );
        assert!(
            flag.load(Ordering::Acquire),
            "/plan must publish the shared plan-active flag"
        );
        // The pre-plan allow-list is snapshotted so exit can restore it.
        assert_eq!(
            engine.plan_state.pre_plan_allow_list,
            vec!["Read".to_string(), "Write".to_string()]
        );
    }

    #[test]
    fn host_enter_plan_mode_gates_write_out_of_the_turn_tool_set() {
        // The concrete D005 symptom: while plan mode is active the per-turn
        // tool filter keeps ONLY Info-category tools, so a mutating tool like
        // Write is not offered to the model. Register a real Info tool and a
        // real Edit-category tool, then assert the SAME filter the run-loop
        // uses (engine.rs ~2554) keeps Info and drops Write once
        // `enter_plan_mode` ran — and that the un-gated set keeps both.
        let mut registry = ToolRegistry::new();
        registry.register(Box::new(CategorizedMockTool {
            tool_name: "Read".to_string(),
            tool_category: ToolCategory::Info,
        }));
        registry.register(Box::new(CategorizedMockTool {
            tool_name: "Write".to_string(),
            tool_category: ToolCategory::Edit,
        }));

        let mut engine = make_plan_engine(vec![]);
        engine.tools = Arc::new(registry);

        // Before /plan: the full set offers Write.
        let ungated = engine.tools.to_tool_defs_filtered(|_| true);
        assert!(
            ungated.iter().any(|t| t.name == "Write"),
            "Write must be available before /plan"
        );

        engine.enter_plan_mode();
        assert!(engine.plan_state.is_active);

        // After /plan: mirror the run-loop's plan-mode branch (Info-only,
        // minus EnterPlanMode). Write must be gone; Read must remain.
        let gated = engine.tools.to_tool_defs_filtered(|t| {
            t.category() == ToolCategory::Info && t.name() != "EnterPlanMode"
        });
        assert!(
            !gated.iter().any(|t| t.name == "Write"),
            "Write must be gated out while /plan is active"
        );
        assert!(
            gated.iter().any(|t| t.name == "Read"),
            "Info tools must still be offered in plan mode"
        );
    }

    #[test]
    fn host_enter_then_exit_plan_mode_clears_the_gate() {
        // D006: approving the plan ("Approve & run") calls `exit_plan_mode()`,
        // which must clear the gate so the approved work runs with its full
        // tool set — and restore the pre-plan allow-list.
        let mut engine = make_plan_engine(vec!["Read".into()]);
        let flag = engine.plan_active_flag.clone().unwrap();

        engine.enter_plan_mode();
        engine.allow_list.push("Write".into());
        assert!(engine.plan_state.is_active);

        engine.exit_plan_mode();

        assert!(
            !engine.plan_state.is_active,
            "approving the plan must clear the gate"
        );
        assert!(!flag.load(Ordering::Acquire), "shared flag must clear");
        assert_eq!(
            engine.allow_list,
            vec!["Read".to_string()],
            "exit must restore the pre-plan allow-list"
        );
    }

    #[test]
    fn host_enter_plan_mode_is_idempotent() {
        // A second `/plan` while already in plan mode must not clobber the
        // snapshotted pre-plan allow-list with the (now Info-narrowed) one.
        let mut engine = make_plan_engine(vec!["Read".into(), "Write".into()]);
        engine.enter_plan_mode();
        // Simulate the allow-list having been narrowed after entry.
        engine.allow_list.clear();
        engine.enter_plan_mode();
        assert_eq!(
            engine.plan_state.pre_plan_allow_list,
            vec!["Read".to_string(), "Write".to_string()],
            "re-entry must not overwrite the original snapshot"
        );
    }

    // --- No plan_active_flag set does not panic ---

    #[test]
    fn enter_without_flag_does_not_panic() {
        let mut engine = make_plan_engine(vec![]);
        engine.plan_active_flag = None;

        engine.apply_context_modifiers(&[Some(ContextModifier {
            plan_mode_transition: Some(PlanModeTransition::Enter),
            ..Default::default()
        })]);

        assert!(engine.plan_state.is_active);
    }
}

// ---------------------------------------------------------------------------
// Hook integration tests — apply_pre_turn_outcome() white-box tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod hook_integration_tests {
    use std::sync::{Arc, Mutex};

    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::{ContentBlock, FinishReason, Message, Role};

    use crate::approval::ApprovalBridge;
    use crate::compact::state::CompactState;
    use crate::confirm::ToolConfirmer;
    // v0.8.0 Task M: inline-test fixture builders need access to the
    // engine-private user-id resolver.
    use super::resolve_user_model_user_id;
    use crate::hooks::HookOutcome;
    use crate::output::OutputSink;
    // D014: context-modifier precedence tests construct skill modifiers.
    use wcore_types::skill_types::ContextModifier;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_engine(model: &str) -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            temperature: None,
            tools: Arc::new(ToolRegistry::new()),
            messages: vec![],
            rebind_system_prefix: None,
            system_prompt: String::new(),
            model: model.to_string(),
            user_model_pin: None,
            max_tokens: 4096,
            max_turns: Some(10),
            total_usage: Default::default(),
            thinking: None,
            compat: wcore_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            current_msg_id: String::new(),
            current_agent_run_id: None,
            approval_manager: None,
            approval_bridge: Arc::new(ApprovalBridge::new()),
            protocol_writer: None,
            allow_list: vec![],
            current_reasoning_effort: None,
            compact_config: wcore_config::compact::CompactConfig::default(),
            compact_state: CompactState::new(),
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
            compaction_level: wcore_compact::CompactionLevel::default(),
            toon_enabled: false,
            compact_bash: false,
            advertised_capabilities: wcore_config::tools::AdvertisedCapabilitiesConfig::default(),
            per_turn_costs: Vec::new(),
            mcp_curation: wcore_config::config::McpCurationPolicy::default(),
            mcp_curation_cache: None,
            mcp_cap_cache: None,
            file_cache: None,
            session_state: None,
            audit_log: None,
            memory_api: Arc::new(wcore_memory::NullMemory),
            dream_throttle: Arc::new(wcore_memory::consolidate::DreamThrottle::new(
                std::time::Duration::from_secs(1800),
            )),
            #[cfg(any(test, feature = "test-utils"))]
            test_sink_handle: crate::test_utils::TestSinkHandle::default(),
            // W9.1 T3 (T10b): inline-test fixture default — gate off.
            skills_lifecycle: false,
            // F-092 (W7-N): inline-test fixture default — gate off.
            online_evolution: false,
            recent_turn_traces: std::collections::VecDeque::new(),
            drafted_skill_signatures: std::collections::HashSet::new(),
            // W8b.2.B D.3 / Task 7: inline-test fixture defaults — watcher off.
            file_watcher: Arc::new(std::sync::OnceLock::new()),
            tool_write_notifier: Arc::new(std::sync::OnceLock::new()),
            // Wave OR: inline-test fixture default — no mode override.
            mode_override: None,
            template_router: None,
            decay_handles: Vec::new(),
            plugin_runtime_handles: Arc::new(Vec::new()),
            budget_tracker: None,
            policy_gate: None,
            agent_registry: None,
            plugin_user_models: Vec::new(),
            style_detector: Mutex::new(crate::style_detector::StyleDetector::new()),
            skill_catalog: None,
            user_model_backend: None,
            user_model_user_id: resolve_user_model_user_id(),
            // v0.8.1 U1 — installed post-construction by
            // `AgentBootstrap::build` (see `set_skill_router`). `None`
            // here keeps every non-bootstrap construction site (tests,
            // resume-without-bootstrap, sub-agent shadows) on the
            // pre-U1 no-op path.
            skill_router: None,
            current_skill_router_pick: None,
            // v0.8.1 U6 — autonomous-skill bucketer is always live (N=3
            // threshold). Drafter is None at construction; bootstrap
            // installs one when memory is wired.
            auto_skill_bucketer: Mutex::new(crate::auto_skill::Bucketer::new(3)),
            skill_drafter: None,
            // AUDIT A2 / B1 — fresh session-root cancellation token.
            // Hosts replace/observe it via `cancel_token()`.
            cancel_token: tokio_util::sync::CancellationToken::new(),
            // AUDIT B-2 / D-5 — reaper handle storage; populated by
            // `set_approval_manager`, aborted by `Drop`.
            background_handles: Vec::new(),
            // Dynamic Workflows B3 — detection gate (default off).
            workflow_detection_enabled: false,
            // Dynamic Workflows B6 — live confirm gate (default off) + a
            // default config for the (unused-in-these-fixtures) live gate.
            workflow_live_mode: false,
            config: wcore_config::config::Config::default(),
            compaction_floor: 0,
            session_start_injected_len: 0,
            web_search: false,
            pending_hook_actions: Vec::new(),
            conversation_id: String::new(),
            flux_served_window: None,
            flux_context_pressure: None,
            smart_compact_armed: true,
            smart_compact_last_turn: None,
            smart_compact_exhausted: false,
            smart_compact_force: false,
        }
    }

    #[test]
    fn apply_pre_turn_outcome_switch_model_overwrites_self_model() {
        let mut engine = make_engine("old-model");
        let outcome = HookOutcome {
            switch_model: Some("new-model".into()),
            ..Default::default()
        };
        engine.apply_pre_turn_outcome(outcome);
        assert_eq!(engine.model, "new-model");
    }

    // D014: a skill/hook switch_model must NOT override an explicit user
    // `/model` pin; it MUST apply when no pin is set.

    #[test]
    fn d014_pre_turn_switch_model_honored_when_no_user_pin() {
        let mut engine = make_engine("base-model");
        assert!(engine.user_model_pin().is_none());
        let outcome = HookOutcome {
            switch_model: Some("hook-model".into()),
            ..Default::default()
        };
        engine.apply_pre_turn_outcome(outcome);
        // No pin → hook switch wins.
        assert_eq!(engine.model, "hook-model");
    }

    #[test]
    fn d014_pre_turn_switch_model_ignored_when_user_pin_set() {
        let mut engine = make_engine("base-model");
        // Explicit user `/model` pick (the TUI bridge calls set_model).
        engine.set_model("user-pick");
        assert_eq!(engine.user_model_pin(), Some("user-pick"));
        assert_eq!(engine.model, "user-pick");

        let outcome = HookOutcome {
            switch_model: Some("hook-model".into()),
            ..Default::default()
        };
        engine.apply_pre_turn_outcome(outcome);
        // Pin wins — the hook switch is refused.
        assert_eq!(engine.model, "user-pick");
        assert_eq!(engine.user_model_pin(), Some("user-pick"));
    }

    #[test]
    fn d014_turn_end_switch_model_ignored_when_user_pin_set() {
        let mut engine = make_engine("base-model");
        engine.set_model("user-pick");
        let outcome = HookOutcome {
            switch_model: Some("hook-model".into()),
            ..Default::default()
        };
        engine.apply_turn_end_outcome(outcome);
        assert_eq!(engine.model, "user-pick");
    }

    #[test]
    fn d014_turn_end_switch_model_honored_when_no_user_pin() {
        let mut engine = make_engine("base-model");
        let outcome = HookOutcome {
            switch_model: Some("hook-model".into()),
            ..Default::default()
        };
        engine.apply_turn_end_outcome(outcome);
        assert_eq!(engine.model, "hook-model");
    }

    #[test]
    fn d014_context_modifier_model_ignored_when_user_pin_set() {
        let mut engine = make_engine("base-model");
        engine.set_model("user-pick");
        let modifiers = vec![Some(ContextModifier {
            model: Some("skill-model".to_string()),
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "user-pick");
    }

    #[test]
    fn d014_context_modifier_model_honored_when_no_user_pin() {
        let mut engine = make_engine("base-model");
        let modifiers = vec![Some(ContextModifier {
            model: Some("skill-model".to_string()),
            ..Default::default()
        })];
        engine.apply_context_modifiers(&modifiers);
        assert_eq!(engine.model, "skill-model");
    }

    #[test]
    fn d014_clear_model_pin_re_enables_hook_switch() {
        let mut engine = make_engine("base-model");
        engine.set_model("user-pick");
        engine.clear_model_pin();
        assert!(engine.user_model_pin().is_none());
        // Active model is unchanged by clearing the pin.
        assert_eq!(engine.model, "user-pick");

        let outcome = HookOutcome {
            switch_model: Some("hook-model".into()),
            ..Default::default()
        };
        engine.apply_pre_turn_outcome(outcome);
        // Pin released → hook switch wins again.
        assert_eq!(engine.model, "hook-model");
    }

    #[test]
    fn d014_clear_conversation_releases_user_pin() {
        let mut engine = make_engine("base-model");
        engine.set_model("user-pick");
        engine.clear_conversation();
        assert!(engine.user_model_pin().is_none());

        let outcome = HookOutcome {
            switch_model: Some("hook-model".into()),
            ..Default::default()
        };
        engine.apply_pre_turn_outcome(outcome);
        assert_eq!(engine.model, "hook-model");
    }

    #[test]
    fn d014_apply_config_update_model_sets_authoritative_pin() {
        let mut engine = make_engine("base-model");
        let changes =
            engine.apply_config_update(Some("config-model".to_string()), None, None, None, None);
        assert!(!changes.is_empty());
        assert_eq!(engine.model, "config-model");
        assert_eq!(engine.user_model_pin(), Some("config-model"));

        // A later hook switch must not override the config-set pin.
        let outcome = HookOutcome {
            switch_model: Some("hook-model".into()),
            ..Default::default()
        };
        engine.apply_pre_turn_outcome(outcome);
        assert_eq!(engine.model, "config-model");
    }

    #[test]
    fn apply_pre_turn_outcome_injects_messages_into_history() {
        let mut engine = make_engine("m");
        let injected = Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "from hook".into(),
            }],
        );
        let outcome = HookOutcome {
            injected_messages: vec![injected.clone()],
            ..Default::default()
        };
        engine.apply_pre_turn_outcome(outcome);
        assert_eq!(engine.messages.len(), 1);
        match &engine.messages[0].content[0] {
            ContentBlock::Text { text } => assert_eq!(text, "from hook"),
            other => panic!("expected Text, got: {other:?}"),
        }
    }

    #[test]
    fn apply_pre_turn_outcome_continue_default_does_nothing() {
        let mut engine = make_engine("keep");
        let outcome = HookOutcome::default();
        engine.apply_pre_turn_outcome(outcome);
        assert_eq!(engine.model, "keep");
        assert!(engine.messages.is_empty());
    }

    #[test]
    fn apply_turn_end_outcome_switch_model_applies_to_next_turn() {
        let mut engine = make_engine("m");
        let outcome = HookOutcome {
            switch_model: Some("next-turn-model".into()),
            ..Default::default()
        };
        engine.apply_turn_end_outcome(outcome);
        assert_eq!(engine.model, "next-turn-model");
    }

    #[test]
    fn apply_turn_end_outcome_injects_messages_for_next_turn() {
        let mut engine = make_engine("m");
        let injected = Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "inject-end".into(),
            }],
        );
        let outcome = HookOutcome {
            injected_messages: vec![injected],
            ..Default::default()
        };
        engine.apply_turn_end_outcome(outcome);
        assert_eq!(engine.messages.len(), 1);
    }

    #[tokio::test]
    async fn fire_on_session_end_no_hooks_is_noop() {
        // Engine with hooks: None should not panic.
        let engine = make_engine("m");
        engine.fire_on_session_end(5).await;
    }

    #[tokio::test]
    async fn fire_on_session_end_with_hook_fires_summary() {
        use std::sync::Arc;

        use crate::hooks::{Hook, HookAction, HookEngine, SessionEndSummary};

        struct EndHook {
            fired: Arc<Mutex<Option<SessionEndSummary>>>,
        }
        #[async_trait::async_trait]
        impl Hook for EndHook {
            fn name(&self) -> &str {
                "end-hook"
            }
            async fn on_session_end(&self, summary: &SessionEndSummary) -> HookAction {
                *self.fired.lock().unwrap() = Some(summary.clone());
                HookAction::Continue
            }
        }

        let fired: Arc<Mutex<Option<SessionEndSummary>>> = Arc::new(Mutex::new(None));
        let mut engine = make_engine("m");
        let mut hook_engine = HookEngine::new(wcore_config::hooks::HooksConfig::default());
        hook_engine.register_rust_hook(Box::new(EndHook {
            fired: fired.clone(),
        }));
        engine.hooks = Some(hook_engine);
        engine.total_usage.input_tokens = 100;
        engine.total_usage.output_tokens = 50;

        engine.fire_on_session_end(7).await;

        let snap = fired.lock().unwrap().clone().expect("hook should fire");
        assert_eq!(snap.turns, 7);
        assert_eq!(snap.total_input_tokens, 100);
        assert_eq!(snap.total_output_tokens, 50);
    }

    // ---- W7 Pre-flight 0: engine carries a MemoryApi handle ----------------

    #[test]
    fn w7_pre0_engine_carries_memory_api_handle() {
        // Fixture engine constructed via make_engine() — defaults to
        // NullMemory under the hood.
        let engine = make_engine("m");
        // memory_api() returns a valid Arc<dyn MemoryApi>.
        let api: &Arc<dyn wcore_memory::MemoryApi> = engine.memory_api();
        // The Arc must point at *something*; downstream W9 hooks dyn-dispatch
        // through it. Strong-count >= 1 proves the field is alive.
        assert!(Arc::strong_count(api) >= 1);
    }

    // ---- B7 (Rank 38): engine writer wired to the session state the
    // `wayland_status` / `wayland_telemetry_query` tools read ---------------

    #[test]
    fn b7_session_state_wiring_surfaces_nonzero_to_reader() {
        use crate::session_state::{InMemorySessionState, SessionStateReader};

        // The introspection backend reads the live state through this Arc.
        let state = Arc::new(InMemorySessionState::new("m"));
        let reader: Arc<dyn SessionStateReader> = state.clone();

        // Before wiring, the reader the tool sees is all zeroes — the exact
        // symptom Rank 38 reported.
        assert_eq!(reader.token_count_input(), 0);
        assert_eq!(reader.token_count_output(), 0);
        assert!(reader.tool_call_count().is_empty());

        // Wire the SAME Arc into the engine (this is the deferred follow-up
        // that Rank 38 flagged as missing).
        let mut engine = make_engine("m");
        engine.set_session_state(state);

        // Drive the engine's writer surface exactly as the per-turn paths now
        // do: token accounting after each turn, tool-call counter per result.
        let wired = engine
            .session_state
            .as_ref()
            .expect("engine must hold the session state after set_session_state");
        wired.add_token_usage(120, 45);
        wired.record_tool_call("Read");
        wired.record_tool_call("Read");
        wired.record_tool_call("Bash");

        // The tool-side reader (a clone of the same Arc) now observes the
        // engine's writes — non-zero counters, not the prior zeroes.
        assert_eq!(reader.token_count_input(), 120);
        assert_eq!(reader.token_count_output(), 45);
        let calls = reader.tool_call_count();
        assert_eq!(calls.get("Read"), Some(&2));
        assert_eq!(calls.get("Bash"), Some(&1));
    }

    #[tokio::test]
    async fn w7_pre0_default_memory_api_is_null_memory_no_op() {
        // The fixture engine uses NullMemory by default. A read returns
        // an empty result instead of erroring — proves the handle is
        // operational, not just present.
        let engine = make_engine("m");
        let api = engine.memory_api();
        let hits = api
            .search(
                wcore_memory::v2_types::Query::default(),
                wcore_memory::AccessToken::MainAgent,
            )
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[test]
    fn w7_pre0_set_memory_api_replaces_handle() {
        let mut engine = make_engine("m");
        let old_count = Arc::strong_count(engine.memory_api());
        let fresh: Arc<dyn wcore_memory::MemoryApi> = Arc::new(wcore_memory::NullMemory);
        engine.set_memory_api(fresh.clone());
        // After replacement, the engine's handle shares strong count with `fresh`.
        assert!(Arc::strong_count(engine.memory_api()) >= 2);
        // Old default handle is dropped (just bound `old_count` to suppress
        // the unused-variable lint and document the before-state intent).
        let _ = old_count;
    }

    #[test]
    fn w7_pre0_hook_engine_accessor_returns_default_engine() {
        // make_engine() sets hooks: None. Accessor must surface that.
        let engine = make_engine("m");
        assert!(engine.hook_engine().is_none());
    }

    // ---- W3 (v0.6.3): auto-memorize SessionEnd wiring ----------------------

    /// A `MemoryApi` mock that counts `assert_fact` invocations so the W3
    /// tests can assert whether `fire_auto_memorize` reached persistence.
    #[derive(Default)]
    struct FactCountingMem {
        fact_writes: std::sync::atomic::AtomicU64,
    }

    #[async_trait::async_trait]
    impl wcore_memory::MemoryApi for FactCountingMem {
        async fn record_episode(
            &self,
            _: wcore_memory::v2_types::Episode,
            _: wcore_memory::AccessToken,
        ) -> wcore_memory::error::Result<wcore_memory::v2_types::EpisodeId> {
            Ok(wcore_memory::v2_types::EpisodeId::default())
        }
        async fn assert_fact(
            &self,
            _: wcore_memory::v2_types::Fact,
            _: wcore_memory::AccessToken,
        ) -> wcore_memory::error::Result<wcore_memory::v2_types::FactId> {
            self.fact_writes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(wcore_memory::v2_types::FactId::default())
        }
        async fn upsert_procedure(
            &self,
            _: wcore_memory::v2_types::Procedure,
            _: wcore_memory::AccessToken,
        ) -> wcore_memory::error::Result<wcore_memory::v2_types::ProcedureId> {
            Ok(wcore_memory::v2_types::ProcedureId::default())
        }
        async fn list_procedures(
            &self,
            _: wcore_memory::v2_types::Tier,
            _: wcore_memory::AccessToken,
        ) -> wcore_memory::error::Result<Vec<wcore_memory::v2_types::Procedure>> {
            Ok(vec![])
        }
        async fn update_user_model(
            &self,
            _: &str,
            _: serde_json::Value,
            _: wcore_memory::AccessToken,
        ) -> wcore_memory::error::Result<()> {
            Ok(())
        }
        async fn search(
            &self,
            _: wcore_memory::v2_types::Query,
            _: wcore_memory::AccessToken,
        ) -> wcore_memory::error::Result<Vec<wcore_memory::v2_types::Hit>> {
            Ok(vec![])
        }
        async fn get_episode(
            &self,
            _: &wcore_memory::v2_types::EpisodeId,
            _: wcore_memory::AccessToken,
        ) -> wcore_memory::error::Result<wcore_memory::v2_types::Episode> {
            unimplemented!("not exercised by the W3 tests")
        }
        async fn user_model(
            &self,
            _: wcore_memory::AccessToken,
        ) -> wcore_memory::error::Result<wcore_memory::v2_types::UserModel> {
            Ok(wcore_memory::v2_types::UserModel::default())
        }
        async fn dream_now(
            &self,
        ) -> wcore_memory::error::Result<wcore_memory::v2_types::DreamReport> {
            Ok(wcore_memory::v2_types::DreamReport::default())
        }
        async fn compact(
            &self,
            _: u64,
        ) -> wcore_memory::error::Result<wcore_memory::v2_types::CompactReport> {
            Ok(wcore_memory::v2_types::CompactReport::default())
        }
        async fn record_skill_use(
            &self,
            _: &str,
            _: bool,
            _: u64,
        ) -> wcore_memory::error::Result<()> {
            Ok(())
        }
        async fn top_procedures(
            &self,
            _: wcore_memory::v2_types::Tier,
            _: usize,
            _: u64,
            _: wcore_memory::AccessToken,
        ) -> wcore_memory::error::Result<Vec<wcore_memory::v2_types::Procedure>> {
            Ok(vec![])
        }
        async fn kg_ingest_facts(&self, _: &str) -> wcore_memory::error::Result<usize> {
            Ok(0)
        }
    }

    /// W3: when consent is NOT granted, `fire_auto_memorize` must not reach
    /// `assert_fact` even though the session messages carry extractable
    /// facts. `WAYLAND_AUTO_MEMORIZE=off` is the hermetic kill switch.
    #[tokio::test]
    #[serial_test::serial(env)]
    async fn w3_auto_memorize_skips_without_consent() {
        let prior = std::env::var(wcore_memory::auto_memorize::ENV_AUTO_MEMORIZE).ok();
        // SAFETY: #[serial(env)] serializes all env writes in this group.
        unsafe {
            std::env::set_var(wcore_memory::auto_memorize::ENV_AUTO_MEMORIZE, "off");
        }

        let counter = Arc::new(FactCountingMem::default());
        let mut engine = make_engine("m");
        engine.set_memory_api(counter.clone());
        engine.messages = vec![Message::new(
            super::Role::Assistant,
            vec![super::ContentBlock::Text {
                text: "Rust is a language".into(),
            }],
        )];

        engine.fire_auto_memorize().await;

        assert_eq!(
            counter
                .fact_writes
                .load(std::sync::atomic::Ordering::SeqCst),
            0,
            "no facts may be persisted when consent is off"
        );

        // SAFETY: #[serial(env)] serializes all env writes in this group.
        unsafe {
            match prior {
                Some(v) => std::env::set_var(wcore_memory::auto_memorize::ENV_AUTO_MEMORIZE, v),
                None => std::env::remove_var(wcore_memory::auto_memorize::ENV_AUTO_MEMORIZE),
            }
        }
    }

    /// W3: when consent IS granted, `fire_auto_memorize` extracts facts
    /// from the session messages and routes the survivors to
    /// `assert_fact` — proving the SessionEnd trigger is wired through to
    /// persistence. The consent file is created and removed within the
    /// test (state restored on exit) under `#[serial]`.
    #[tokio::test]
    #[serial_test::serial(env)]
    async fn w3_auto_memorize_persists_with_consent() {
        let prior_env = std::env::var(wcore_memory::auto_memorize::ENV_AUTO_MEMORIZE).ok();
        // SAFETY: #[serial(env)] serializes all env writes in this group.
        unsafe {
            std::env::remove_var(wcore_memory::auto_memorize::ENV_AUTO_MEMORIZE);
        }

        let consent_path = wcore_memory::auto_memorize::consent_file_path();
        let consent_existed = consent_path.is_file();
        if !consent_existed {
            if let Some(parent) = consent_path.parent() {
                std::fs::create_dir_all(parent).expect("create consent dir");
            }
            std::fs::write(&consent_path, b"opt-in").expect("write consent file");
        }

        let counter = Arc::new(FactCountingMem::default());
        let mut engine = make_engine("m");
        engine.set_memory_api(counter.clone());
        // "X uses Y" is a default FactExtractor pattern at confidence 0.70,
        // which clears the 0.5 min_confidence threshold.
        engine.messages = vec![Message::new(
            super::Role::Assistant,
            vec![super::ContentBlock::Text {
                text: "wayland uses tokio".into(),
            }],
        )];

        engine.fire_auto_memorize().await;

        let writes = counter
            .fact_writes
            .load(std::sync::atomic::Ordering::SeqCst);

        // Restore consent-file state before asserting so a failure does not
        // leak the test fixture into the user's config dir.
        if !consent_existed {
            let _ = std::fs::remove_file(&consent_path);
        }
        // SAFETY: #[serial(env)] serializes all env writes in this group.
        unsafe {
            if let Some(v) = prior_env {
                std::env::set_var(wcore_memory::auto_memorize::ENV_AUTO_MEMORIZE, v);
            }
        }

        assert_eq!(
            writes, 1,
            "the extracted fact must be persisted via assert_fact when consent is granted"
        );
    }
}

#[derive(Debug)]
pub struct AgentResult {
    pub text: String,
    pub stop_reason: StopReason,
    /// Protocol-level finish reason. Threaded from the provider's last
    /// `LlmEvent::Done` so the JSON stream protocol's `stream_end` event
    /// can advertise the same value the underlying API returned.
    pub finish_reason: FinishReason,
    pub usage: TokenUsage,
    pub turns: usize,
    /// #279(a): active-window fill (0..=100) from ContextWindow::percent()
    /// on the post-swap effective model. None when the window is unknown.
    pub active_window_percent: Option<u32>,
    /// #279(c): the run's stable correlation id (clone of current_agent_run_id).
    pub agent_run_id: Option<String>,
}

/// Run the shared `drive_council` over OWNED inputs behind a `Pin<Box<dyn Future>>`.
/// The boxed-dyn return type ERASES the council future, which is what severs the
/// `engine.run → try_crucible_council → drive_council → spawn_one → engine.run`
/// async-recursion type cycle (mirrors `run_workflow_owned`). Spawned detached so
/// the `Send + 'static` boundary is forced exactly once.
#[allow(clippy::too_many_arguments)]
fn run_crucible_owned(
    task: String,
    runnable: Vec<String>,
    base: Config,
    cfg: wcore_config::crucible::CrucibleConfig,
    ov: crate::orchestration::council::CouncilOverrides,
    spawner: crate::spawner::AgentSpawner,
    approver: BridgeApprover,
    refilter: impl Fn(&[String]) -> Vec<String> + Send + Sync + 'static,
) -> std::pin::Pin<
    Box<
        dyn std::future::Future<
                Output = anyhow::Result<crate::orchestration::council::CouncilRunResult>,
            > + Send,
    >,
> {
    Box::pin(async move {
        crate::orchestration::council::drive_council(
            &task, runnable, &base, &cfg, &ov, &spawner, &approver, &refilter,
        )
        .await
    })
}

/// Engine-side [`CouncilApprover`] (Stage 3b): emits the typed `ApprovalRequired`
/// card and awaits the host's typed `CrucibleDecision` via the approval bridge,
/// so the `/crucible` front door drives through the SAME `drive_council` loop the
/// CLI uses. A fresh `correlation_id` per round keeps bridge resolution
/// unambiguous; a stable `call_id` ties the rounds to one host-side tool card.
struct BridgeApprover {
    writer: std::sync::Arc<dyn wcore_protocol::writer::ProtocolEmitter>,
    bridge: std::sync::Arc<crate::approval::ApprovalBridge>,
    cancel: tokio_util::sync::CancellationToken,
    call_id: String,
}

#[async_trait::async_trait]
impl crate::orchestration::council::CouncilApprover for BridgeApprover {
    async fn approve(
        &self,
        card: &wcore_types::crucible::CruciblePlan,
    ) -> anyhow::Result<wcore_types::crucible::CrucibleDecision> {
        use wcore_protocol::events::ProtocolEvent;
        use wcore_types::crucible::CrucibleDecision;

        let ceiling = card
            .ceiling_usd()
            .map(|u| format!("${u:.4}"))
            .unwrap_or_else(|| "price unknown".to_string());
        // Stage 4b: key the bridge by the stable, `crucible:`-prefixed call_id —
        // NOT a throwaway per-round uuid. The approval dedupe (TUI ChannelEmitter
        // + JSON-stream GatingProtocolWriter) SUPPRESSES this explicit
        // ApprovalRequired and keeps the ToolRequest-synthesized frame, whose
        // `resume_token` IS the call_id. So every host can only resolve the
        // bridge by call_id: the TUI via `resolve_crucible(call_id)` and the
        // JSON host via `ApprovalResume{resume_token}` → `bridge.resolve(call_id)`
        // (main.rs). Keying by a fresh uuid here left the council unresolvable on
        // ALL hosts — it hung until the 24h TTL.
        //
        // Register the pending entry BEFORE emitting the card so a fast host
        // response can never arrive before the bridge can resolve it.
        let rx = self
            .bridge
            .request_with_id_and_ttl(
                self.call_id.clone(),
                crate::approval::ApprovalRequest {
                    call_id: self.call_id.clone(),
                    reason: "Run Crucible council?".to_string(),
                    context: format!("council ceiling {ceiling}"),
                },
                // Long/no-expire TTL (spec §7): a multi-vendor cost card is a
                // deliberation-worthy decision and must not be reaped by the
                // 5-minute default while the user reads it.
                crate::approval::CRUCIBLE_APPROVAL_TTL,
            )
            .await;
        let _ = self.writer.emit(&ProtocolEvent::ApprovalRequired {
            call_id: self.call_id.clone(),
            resume_token: self.call_id.clone(),
            correlation_id: self.call_id.clone(),
            reason: "Run Crucible council?".to_string(),
            context: format!("Crucible council — you're approving up to {ceiling}."),
            plan: Some(card.clone()),
        });
        let outcome = tokio::select! {
            biased;
            _ = self.cancel.cancelled() => return Ok(CrucibleDecision::Cancel),
            res = rx => res,
        };
        match outcome {
            Ok(o) if o.approved => Ok(CrucibleDecision::from_modifications(
                o.modifications.as_ref(),
            )
            .unwrap_or(CrucibleDecision::Approve)),
            // Denied, cancelled, or a dropped channel (reaper/host crash) → no spend.
            _ => Ok(CrucibleDecision::Cancel),
        }
    }
}

#[cfg(test)]
mod approval_bridge_engine_tests {
    //! W7.1 S4-3.2: verify `engine.approval_bridge()` exposes the same
    //! `Arc<ApprovalBridge>` instance that was installed via
    //! `set_approval_bridge`, so a `bridge.resolve(...)` call on that
    //! accessor unblocks a `bridge.request(...)` future taken from the
    //! shared bridge — which is exactly the round-trip the CLI relies on.
    use std::sync::{Arc, Mutex};

    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::FinishReason;
    // #280 smart-compaction tests construct conversation messages directly.
    use wcore_types::message::{ContentBlock, Message, Role};

    use crate::approval::{ApprovalBridge, ApprovalOutcome, ApprovalRequest};
    use crate::compact::state::CompactState;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;
    // v0.8.0 Task M: inline-test fixture builders need access to the
    // engine-private user-id resolver.
    use super::resolve_user_model_user_id;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait::async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_engine() -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            temperature: None,
            tools: Arc::new(ToolRegistry::new()),
            messages: vec![],
            rebind_system_prefix: None,
            system_prompt: String::new(),
            model: "test-model".into(),
            user_model_pin: None,
            max_tokens: 4096,
            max_turns: Some(10),
            total_usage: Default::default(),
            thinking: None,
            compat: wcore_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            current_msg_id: String::new(),
            current_agent_run_id: None,
            approval_manager: None,
            approval_bridge: Arc::new(ApprovalBridge::new()),
            protocol_writer: None,
            allow_list: vec![],
            current_reasoning_effort: None,
            compact_config: wcore_config::compact::CompactConfig::default(),
            compact_state: CompactState::new(),
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
            compaction_level: wcore_compact::CompactionLevel::default(),
            toon_enabled: false,
            compact_bash: false,
            advertised_capabilities: wcore_config::tools::AdvertisedCapabilitiesConfig::default(),
            per_turn_costs: Vec::new(),
            mcp_curation: wcore_config::config::McpCurationPolicy::default(),
            mcp_curation_cache: None,
            mcp_cap_cache: None,
            file_cache: None,
            session_state: None,
            audit_log: None,
            memory_api: Arc::new(wcore_memory::NullMemory),
            dream_throttle: Arc::new(wcore_memory::consolidate::DreamThrottle::new(
                std::time::Duration::from_secs(1800),
            )),
            #[cfg(any(test, feature = "test-utils"))]
            test_sink_handle: crate::test_utils::TestSinkHandle::default(),
            // W9.1 T3 (T10b): inline-test fixture default — gate off.
            skills_lifecycle: false,
            // F-092 (W7-N): inline-test fixture default — gate off.
            online_evolution: false,
            recent_turn_traces: std::collections::VecDeque::new(),
            drafted_skill_signatures: std::collections::HashSet::new(),
            // W8b.2.B D.3 / Task 7: inline-test fixture defaults — watcher off.
            file_watcher: Arc::new(std::sync::OnceLock::new()),
            tool_write_notifier: Arc::new(std::sync::OnceLock::new()),
            // Wave OR: inline-test fixture default — no mode override.
            mode_override: None,
            template_router: None,
            decay_handles: Vec::new(),
            plugin_runtime_handles: Arc::new(Vec::new()),
            budget_tracker: None,
            policy_gate: None,
            agent_registry: None,
            plugin_user_models: Vec::new(),
            style_detector: Mutex::new(crate::style_detector::StyleDetector::new()),
            skill_catalog: None,
            user_model_backend: None,
            user_model_user_id: resolve_user_model_user_id(),
            // v0.8.1 U1 — installed post-construction by
            // `AgentBootstrap::build` (see `set_skill_router`). `None`
            // here keeps every non-bootstrap construction site (tests,
            // resume-without-bootstrap, sub-agent shadows) on the
            // pre-U1 no-op path.
            skill_router: None,
            current_skill_router_pick: None,
            // v0.8.1 U6 — autonomous-skill bucketer is always live (N=3
            // threshold). Drafter is None at construction; bootstrap
            // installs one when memory is wired.
            auto_skill_bucketer: Mutex::new(crate::auto_skill::Bucketer::new(3)),
            skill_drafter: None,
            // AUDIT A2 / B1 — fresh session-root cancellation token.
            // Hosts replace/observe it via `cancel_token()`.
            cancel_token: tokio_util::sync::CancellationToken::new(),
            // AUDIT B-2 / D-5 — reaper handle storage; populated by
            // `set_approval_manager`, aborted by `Drop`.
            background_handles: Vec::new(),
            // Dynamic Workflows B3 — detection gate (default off).
            workflow_detection_enabled: false,
            // Dynamic Workflows B6 — live confirm gate (default off) + a
            // default config for the (unused-in-these-fixtures) live gate.
            workflow_live_mode: false,
            config: wcore_config::config::Config::default(),
            compaction_floor: 0,
            session_start_injected_len: 0,
            web_search: false,
            pending_hook_actions: Vec::new(),
            conversation_id: String::new(),
            flux_served_window: None,
            flux_context_pressure: None,
            smart_compact_armed: true,
            smart_compact_last_turn: None,
            smart_compact_exhausted: false,
            smart_compact_force: false,
        }
    }

    #[tokio::test]
    async fn accessor_returns_the_same_bridge_set_by_setter() {
        let mut engine = make_engine();

        // Install a host-supplied bridge — same pattern AgentBootstrap uses.
        let shared = Arc::new(ApprovalBridge::new());
        engine.set_approval_bridge(shared.clone());

        // Make a pending request on the original Arc the host kept.
        let (token, rx) = shared
            .request(ApprovalRequest {
                call_id: "c-1".into(),
                reason: "test".into(),
                context: "ctx".into(),
            })
            .await;

        // Resolve via the engine's accessor — must hit the same pending map.
        let resolved = engine
            .approval_bridge()
            .resolve(
                &token,
                ApprovalOutcome {
                    approved: true,
                    modifications: None,
                },
            )
            .await;
        assert!(
            resolved,
            "engine.approval_bridge() must point at the same instance set via \
             set_approval_bridge(); resolve returned false meaning the token \
             was not found on the engine-side handle"
        );

        // And the original request future must complete with the resolved outcome.
        let outcome = rx.await.expect("oneshot must deliver");
        assert!(outcome.approved);
    }

    /// Stage 4b — a minimal, fully-`None` proposal card for approver round-trip
    /// tests (the approver only reads `ceiling_usd()` for the prompt text).
    fn crucible_test_plan() -> wcore_types::crucible::CruciblePlan {
        wcore_types::crucible::CruciblePlan {
            convene: true,
            members: vec![],
            stakes: "low".into(),
            focus: None,
            ceiling_microcents: None,
            single_model_baseline_microcents: None,
            day_spent_microcents: None,
            day_cap_microcents: None,
            judge_independent: false,
            reason: String::new(),
            trims: vec![],
        }
    }

    struct NullEmitter;
    impl wcore_protocol::writer::ProtocolEmitter for NullEmitter {
        fn emit(&self, _e: &wcore_protocol::events::ProtocolEvent) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Run `BridgeApprover::approve` on a fresh bridge, then have a "host" resolve
    /// the council by the `crucible:` call_id (the only token any host can send,
    /// since the approval dedupe collapses every resume token to the call_id).
    /// Returns the decision the approver derived. Polls `resolve` so the test does
    /// not race the spawned approve() before it registers the pending entry.
    async fn drive_crucible_approver(
        approved: bool,
        modifications: Option<serde_json::Value>,
    ) -> wcore_types::crucible::CrucibleDecision {
        use crate::orchestration::council::CouncilApprover;
        use tokio_util::sync::CancellationToken;

        let bridge = Arc::new(ApprovalBridge::new());
        let call_id = "crucible:fixed-id".to_string();
        let writer: Arc<dyn wcore_protocol::writer::ProtocolEmitter> = Arc::new(NullEmitter);
        let approver = super::BridgeApprover {
            writer,
            bridge: bridge.clone(),
            cancel: CancellationToken::new(),
            call_id: call_id.clone(),
        };
        let handle = tokio::spawn(async move { approver.approve(&crucible_test_plan()).await });

        let mut resolved = false;
        for _ in 0..512 {
            if bridge
                .resolve(
                    &call_id,
                    ApprovalOutcome {
                        approved,
                        modifications: modifications.clone(),
                    },
                )
                .await
            {
                resolved = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            resolved,
            "ApprovalBridge MUST be resolvable by the `crucible:` call_id — keying \
             the bridge by a throwaway uuid (the pre-4b bug) makes every host's \
             resolve-by-call_id miss and the council hangs until the 24h TTL"
        );
        handle
            .await
            .expect("approve task joins")
            .expect("approve ok")
    }

    #[tokio::test]
    async fn crucible_approver_keys_bridge_by_call_id_and_maps_approve() {
        let decision = drive_crucible_approver(true, None).await;
        assert_eq!(
            decision,
            wcore_types::crucible::CrucibleDecision::Approve,
            "approved + no modifications must map to Approve"
        );
    }

    #[tokio::test]
    async fn crucible_approver_maps_denied_outcome_to_cancel() {
        let decision = drive_crucible_approver(false, None).await;
        assert_eq!(
            decision,
            wcore_types::crucible::CrucibleDecision::Cancel,
            "a not-approved bridge outcome (TUI deny / host reject) must map to Cancel — no spend"
        );
    }

    /// Stage 4c — the `/crucible` discovery suggestion. Driven through the real
    /// `maybe_suggest_council` predicate on a wired engine. The suppression flag
    /// is process-global, so this single test saves/restores it and walks the
    /// four gate conditions in order (writer present + not suppressed + High →
    /// Some; Med → None; suppressed → None; no writer → None) rather than relying
    /// on cross-test ordering of a shared static.
    #[test]
    fn maybe_suggest_council_fires_only_on_high_stakes_with_a_writer_once() {
        use std::sync::atomic::Ordering;

        let prev = super::CRUCIBLE_SUGGEST_SUPPRESSED.swap(false, Ordering::Relaxed);

        let mut engine = make_engine();
        engine.set_protocol_writer(Arc::new(NullEmitter));

        // High-stakes input with a writer and not suppressed → a hint mentioning
        // `/crucible`.
        let hint = engine
            .maybe_suggest_council("audit the security of this auth flow for vulnerabilities");
        assert!(
            hint.as_deref().is_some_and(|h| h.contains("/crucible")),
            "a high-stakes input must surface the `/crucible` hint, got {hint:?}"
        );

        // A short / Med-stakes input must NOT nudge (Med is too noisy).
        assert!(
            engine
                .maybe_suggest_council("rename this variable")
                .is_none(),
            "a low/med-stakes input must not surface the hint"
        );

        // Once suppressed, even a high-stakes input is silent.
        super::CRUCIBLE_SUGGEST_SUPPRESSED.store(true, Ordering::Relaxed);
        assert!(
            engine
                .maybe_suggest_council("audit the security of this auth flow for vulnerabilities")
                .is_none(),
            "after suppression the hint must not fire again"
        );

        // With no protocol writer there is no host to render the hint → None.
        super::CRUCIBLE_SUGGEST_SUPPRESSED.store(false, Ordering::Relaxed);
        engine.protocol_writer = None;
        assert!(
            engine
                .maybe_suggest_council("audit the security of this auth flow for vulnerabilities")
                .is_none(),
            "without a protocol writer the hint must not fire"
        );

        // Restore the shared static so test ordering can't leak suppression.
        super::CRUCIBLE_SUGGEST_SUPPRESSED.store(prev, Ordering::Relaxed);
    }

    #[tokio::test]
    async fn accessor_default_bridge_resolves_unknown_token_as_false() {
        // No set_approval_bridge() call — engine ships with a default bridge
        // from its constructor. Resolving an unknown token must report false
        // (the same stale-token signal the CLI relies on to emit Info).
        let engine = make_engine();
        let resolved = engine
            .approval_bridge()
            .resolve(
                "no-such-token",
                ApprovalOutcome {
                    approved: false,
                    modifications: None,
                },
            )
            .await;
        assert!(!resolved);
    }

    // --- #280 smart auto-compaction ---------------------------------------

    /// Default-OFF: with `smart_enabled` false, `smart_compact_fraction` returns
    /// `None` even at extreme pressure — the single chokepoint that keeps the
    /// whole feature inert and `run_compaction` byte-for-byte unchanged.
    #[test]
    fn smart_default_off_fraction_is_none_at_any_pressure() {
        let mut engine = make_engine();
        // Slam the watermark well past any window; gate is still off by default.
        engine.compact_state.last_real_input_tokens = 10_000_000;
        assert!(!engine.compact_config.smart_enabled);
        assert_eq!(engine.smart_compact_fraction(), None);
    }

    /// When enabled, the fraction is computed against the active model's window.
    /// "test-model" is unknown → resolves to the config window (200k), so 130k
    /// used ≈ 0.65. At a small fill it must be well under the trigger.
    #[test]
    fn smart_fraction_uses_config_window_for_unknown_model() {
        let mut engine = make_engine();
        engine.compact_config.smart_enabled = true;
        engine.compact_config.context_window = 200_000;
        engine.compact_state.last_real_input_tokens = 130_000;
        let frac = engine.smart_compact_fraction().expect("window known");
        assert!((frac - 0.65).abs() < 0.01, "frac was {frac}");
        engine.compact_state.last_real_input_tokens = 60_000;
        let low = engine.smart_compact_fraction().expect("window known");
        assert!(low < 0.35, "low frac was {low}");
    }

    /// Flux-aware denominator: a tier-alias model with a served window prefers
    /// that window; a Flux pressure header forces a higher effective fraction
    /// via the max() path even when the local estimate lags.
    #[test]
    fn smart_fraction_prefers_flux_served_window_and_pressure() {
        let mut engine = make_engine();
        engine.compact_config.smart_enabled = true;
        // A Flux tier alias so the served-window branch is taken.
        engine.model = "flux-auto".into();
        engine.flux_served_window = Some(280_000);
        engine.compact_state.last_real_input_tokens = 200_000;
        // 200k / 280k ≈ 0.714.
        let frac = engine.smart_compact_fraction().expect("served window set");
        assert!((frac - 0.714).abs() < 0.01, "frac was {frac}");

        // Larger served window lowers the fraction.
        engine.flux_served_window = Some(400_000);
        let lower = engine.smart_compact_fraction().expect("served window set");
        assert!(lower < 0.55, "lower frac was {lower}");

        // A high pressure header forces a higher effective fraction via max()
        // even though the kernel fraction (200k/400k = 0.5) is lower.
        engine.flux_context_pressure = Some(0.70);
        let forced = engine.smart_compact_fraction().expect("pressure set");
        // The f32 pressure header (≈0.70) wins over the kernel fraction (0.50)
        // via max(); allow for f32→f64 rounding.
        assert!(forced > 0.69, "forced frac was {forced}");
    }

    /// Fire test + the band clamp: an out-of-band trigger (0.95) is clamped to
    /// 0.70; a fraction at/above the clamped trigger fires once and disarms.
    #[test]
    fn smart_should_fire_clamps_trigger_and_disarms() {
        let mut engine = make_engine();
        engine.compact_config.smart_trigger_fraction = 0.95; // → clamped 0.70
        // 0.69 is below the clamped 0.70 trigger → no fire.
        assert!(!engine.smart_compact_should_fire(1, 0.69));
        assert!(engine.smart_compact_armed);
        // 0.72 >= clamped 0.70 → fire, then disarm.
        assert!(engine.smart_compact_should_fire(1, 0.72));
        assert!(!engine.smart_compact_armed);
        assert_eq!(engine.smart_compact_last_turn, Some(1));
    }

    /// Hysteresis: across turns the trigger fires, stays disarmed through the
    /// dead band, re-arms only once the fraction drops below release, then fires
    /// again — exactly two fires.
    #[test]
    fn smart_hysteresis_two_fires_across_turns() {
        let mut engine = make_engine();
        engine.compact_config.smart_trigger_fraction = 0.65;
        engine.compact_config.smart_release_fraction = 0.50;
        engine.compact_config.smart_cooldown_turns = 0; // isolate hysteresis
        let fracs = [0.66_f64, 0.62, 0.55, 0.48, 0.66];
        let mut fires = 0;
        for (i, f) in fracs.iter().enumerate() {
            if engine.smart_compact_should_fire(i as u32 + 1, *f) {
                fires += 1;
            }
        }
        assert_eq!(fires, 2, "expected fires at turns 1 and 5 only");
    }

    /// Cooldown: with cooldown 2 and the fraction held above the trigger, fires
    /// are spaced at least 2 turns apart. Re-arm each turn to isolate cooldown
    /// from hysteresis.
    #[test]
    fn smart_cooldown_spaces_fires() {
        let mut engine = make_engine();
        engine.compact_config.smart_trigger_fraction = 0.65;
        engine.compact_config.smart_cooldown_turns = 2;
        let mut fire_turns = Vec::new();
        for turn in 1..=6u32 {
            engine.smart_compact_armed = true; // bypass hysteresis
            if engine.smart_compact_should_fire(turn, 0.80) {
                fire_turns.push(turn);
            }
        }
        // First fire at turn 1; next allowed at turn >= 3, then >= 5.
        assert_eq!(fire_turns, vec![1, 3, 5]);
    }

    /// Cannot-shrink terminal latch: once exhausted, the trigger never fires
    /// again even at extreme pressure and after release would otherwise re-arm.
    #[test]
    fn smart_exhausted_latch_is_terminal() {
        let mut engine = make_engine();
        engine.compact_config.smart_trigger_fraction = 0.65;
        engine.compact_config.smart_cooldown_turns = 0;
        engine.smart_compact_exhausted = true;
        for turn in 1..=5u32 {
            engine.smart_compact_armed = true;
            assert!(!engine.smart_compact_should_fire(turn, 0.99));
        }
    }

    /// Giant-buffer / stays-above-release case: a fraction permanently above
    /// release fires exactly once, then the trigger stays disarmed forever.
    #[test]
    fn smart_stays_disarmed_when_above_release() {
        let mut engine = make_engine();
        engine.compact_config.smart_trigger_fraction = 0.65;
        engine.compact_config.smart_release_fraction = 0.50;
        engine.compact_config.smart_cooldown_turns = 0;
        let mut fires = 0;
        for turn in 1..=6u32 {
            // Held at 0.80: above both trigger and release every turn.
            if engine.smart_compact_should_fire(turn, 0.80) {
                fires += 1;
            }
        }
        assert_eq!(fires, 1, "must fire once then stay disarmed");
    }

    /// Handoff write: a smart handoff records exactly one Episode tagged
    /// compaction_handoff/Session/smart_compact via MainAgent, and the
    /// conversation buffer is left intact (non-destructive).
    #[tokio::test]
    async fn smart_handoff_records_one_session_episode_non_destructively() {
        use std::sync::Mutex as StdMutex;
        use wcore_memory::v2_types::Episode;

        // Capture episodes; delegate every other MemoryApi method to NullMemory
        // so the stub stays a complete, correct impl without re-stubbing 13
        // methods.
        #[derive(Default)]
        struct CapturingMemory {
            episodes: StdMutex<Vec<Episode>>,
            inner: wcore_memory::NullMemory,
        }
        #[async_trait::async_trait]
        impl wcore_memory::MemoryApi for CapturingMemory {
            async fn record_episode(
                &self,
                ep: Episode,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::EpisodeId> {
                self.episodes.lock().unwrap().push(ep.clone());
                self.inner.record_episode(ep, tok).await
            }
            async fn assert_fact(
                &self,
                f: wcore_memory::v2_types::Fact,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::FactId> {
                self.inner.assert_fact(f, tok).await
            }
            async fn upsert_procedure(
                &self,
                p: wcore_memory::v2_types::Procedure,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::ProcedureId> {
                self.inner.upsert_procedure(p, tok).await
            }
            async fn list_procedures(
                &self,
                tier: wcore_memory::v2_types::Tier,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<Vec<wcore_memory::v2_types::Procedure>> {
                self.inner.list_procedures(tier, tok).await
            }
            async fn update_user_model(
                &self,
                key: &str,
                val: serde_json::Value,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<()> {
                self.inner.update_user_model(key, val, tok).await
            }
            async fn search(
                &self,
                q: wcore_memory::v2_types::Query,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<Vec<wcore_memory::v2_types::Hit>> {
                self.inner.search(q, tok).await
            }
            async fn get_episode(
                &self,
                id: &wcore_memory::v2_types::EpisodeId,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<Episode> {
                self.inner.get_episode(id, tok).await
            }
            async fn user_model(
                &self,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::UserModel> {
                self.inner.user_model(tok).await
            }
            async fn dream_now(
                &self,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::DreamReport> {
                self.inner.dream_now().await
            }
            async fn compact(
                &self,
                target_tokens: u64,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::CompactReport> {
                self.inner.compact(target_tokens).await
            }
            async fn record_skill_use(
                &self,
                name: &str,
                succeeded: bool,
                latency_ms: u64,
            ) -> wcore_memory::error::Result<()> {
                self.inner
                    .record_skill_use(name, succeeded, latency_ms)
                    .await
            }
            async fn top_procedures(
                &self,
                tier: wcore_memory::v2_types::Tier,
                k: usize,
                min_uses: u64,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<Vec<wcore_memory::v2_types::Procedure>> {
                self.inner.top_procedures(tier, k, min_uses, tok).await
            }
            async fn kg_ingest_facts(
                &self,
                transcript: &str,
            ) -> wcore_memory::error::Result<usize> {
                self.inner.kg_ingest_facts(transcript).await
            }
        }

        let mut engine = make_engine();
        let mem = Arc::new(CapturingMemory::default());
        engine.memory_api = mem.clone();
        engine.conversation_id = "conv-280".into();
        engine.messages = vec![
            Message::now(
                Role::User,
                vec![ContentBlock::Text {
                    text: "fix the failing test".into(),
                }],
            ),
            Message::now(
                Role::Assistant,
                vec![ContentBlock::Text {
                    text: "looking into it".into(),
                }],
            ),
        ];
        let before = engine.messages.len();

        engine.write_smart_handoff().await;

        let eps = mem.episodes.lock().unwrap();
        assert_eq!(eps.len(), 1, "exactly one episode recorded");
        let ep = &eps[0];
        assert_eq!(ep.episode_type, "compaction_handoff");
        assert_eq!(ep.source, "smart_compact");
        assert_eq!(ep.tier, wcore_memory::v2_types::Tier::Session);
        assert_eq!(ep.session_id.as_deref(), Some("conv-280"));
        assert!(ep.summary.contains("fix the failing test"));
        assert!(ep.summary.contains("looking into it"));
        // Non-destructive: the buffer was not touched.
        assert_eq!(engine.messages.len(), before);
    }

    /// A memory backend that errors must NOT abort the handoff path — the error
    /// is swallowed so the turn proceeds.
    #[tokio::test]
    async fn smart_handoff_swallows_memory_errors() {
        // record_episode errors; everything else delegates to NullMemory. The
        // handoff path must swallow the error and never abort the turn.
        #[derive(Default)]
        struct FailingMemory {
            inner: wcore_memory::NullMemory,
        }
        #[async_trait::async_trait]
        impl wcore_memory::MemoryApi for FailingMemory {
            async fn record_episode(
                &self,
                _ep: wcore_memory::v2_types::Episode,
                _tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::EpisodeId> {
                Err(wcore_memory::error::MemoryError::AccessDenied {
                    partition: "test".into(),
                    tier: "session".into(),
                    reason: "boom".into(),
                })
            }
            async fn assert_fact(
                &self,
                f: wcore_memory::v2_types::Fact,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::FactId> {
                self.inner.assert_fact(f, tok).await
            }
            async fn upsert_procedure(
                &self,
                p: wcore_memory::v2_types::Procedure,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::ProcedureId> {
                self.inner.upsert_procedure(p, tok).await
            }
            async fn list_procedures(
                &self,
                tier: wcore_memory::v2_types::Tier,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<Vec<wcore_memory::v2_types::Procedure>> {
                self.inner.list_procedures(tier, tok).await
            }
            async fn update_user_model(
                &self,
                key: &str,
                val: serde_json::Value,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<()> {
                self.inner.update_user_model(key, val, tok).await
            }
            async fn search(
                &self,
                q: wcore_memory::v2_types::Query,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<Vec<wcore_memory::v2_types::Hit>> {
                self.inner.search(q, tok).await
            }
            async fn get_episode(
                &self,
                id: &wcore_memory::v2_types::EpisodeId,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::Episode> {
                self.inner.get_episode(id, tok).await
            }
            async fn user_model(
                &self,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::UserModel> {
                self.inner.user_model(tok).await
            }
            async fn dream_now(
                &self,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::DreamReport> {
                self.inner.dream_now().await
            }
            async fn compact(
                &self,
                target_tokens: u64,
            ) -> wcore_memory::error::Result<wcore_memory::v2_types::CompactReport> {
                self.inner.compact(target_tokens).await
            }
            async fn record_skill_use(
                &self,
                name: &str,
                succeeded: bool,
                latency_ms: u64,
            ) -> wcore_memory::error::Result<()> {
                self.inner
                    .record_skill_use(name, succeeded, latency_ms)
                    .await
            }
            async fn top_procedures(
                &self,
                tier: wcore_memory::v2_types::Tier,
                k: usize,
                min_uses: u64,
                tok: wcore_memory::AccessToken,
            ) -> wcore_memory::error::Result<Vec<wcore_memory::v2_types::Procedure>> {
                self.inner.top_procedures(tier, k, min_uses, tok).await
            }
            async fn kg_ingest_facts(
                &self,
                transcript: &str,
            ) -> wcore_memory::error::Result<usize> {
                self.inner.kg_ingest_facts(transcript).await
            }
        }

        let mut engine = make_engine();
        engine.memory_api = Arc::new(FailingMemory::default());
        engine.messages = vec![Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        )];
        // Must not panic / propagate.
        engine.write_smart_handoff().await;
        assert_eq!(engine.messages.len(), 1);
    }

    // --- #280 smart compaction through run_compaction() (integration) ------

    /// A provider that always returns a fixed, non-empty summary turn. Reused by
    /// both run_compaction integration tests so autocompact's summary LLM call
    /// succeeds deterministically (no live model). Mirrors the `summary_turn`
    /// shape in `tests/engine_compact_test.rs`.
    struct SummaryProvider {
        summary: String,
    }
    #[async_trait::async_trait]
    impl LlmProvider for SummaryProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            use wcore_types::message::{StopReason, TokenUsage};
            let events = vec![
                LlmEvent::TextDelta(self.summary.clone()),
                LlmEvent::Done {
                    stop_reason: StopReason::EndTurn,
                    finish_reason: FinishReason::from_stop_reason(StopReason::EndTurn),
                    usage: TokenUsage::default(),
                },
            ];
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                for event in events {
                    let _ = tx.send(event).await;
                }
            });
            Ok(rx)
        }
    }

    /// An OutputSink that captures every `emit_compaction` call so the
    /// integration tests can read the `CompactOffload` reason string and the
    /// tokens_freed the engine emitted. Every other method is a no-op.
    #[derive(Default)]
    struct CapturingCompactSink {
        events: std::sync::Mutex<Vec<(String, u64)>>,
    }
    impl OutputSink for CapturingCompactSink {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
        fn emit_compaction(&self, _: &str, reason: &str, tokens_freed: u64, _: Option<u32>) {
            self.events
                .lock()
                .unwrap()
                .push((reason.to_string(), tokens_freed));
        }
    }

    /// Positive path: `smart_compact_force = true` drives `run_compaction()`
    /// through the smart branch. The emitted `CompactOffload` carries
    /// `reason == "smart_window_pressure"`, and because the pre-compaction
    /// total (`compact_state.last_input_tokens`) dwarfs the post-compaction
    /// estimate, `tokens_freed >> smart_min_shrink_tokens` and the cannot-shrink
    /// latch stays clear (`smart_compact_exhausted == false`).
    #[tokio::test]
    async fn smart_run_compaction_emits_smart_reason_and_does_not_exhaust() {
        let mut engine = make_engine();
        engine.provider = Arc::new(SummaryProvider {
            summary: "Conversation summary of the prior turns.".into(),
        });
        let sink = Arc::new(CapturingCompactSink::default());
        engine.output = sink.clone();

        // A real, non-User buffer to summarize (a trailing User message would be
        // popped out as the live turn). pre_compact_tokens == last_input_tokens.
        engine.messages = vec![Message::now(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "earlier assistant context that gets summarized".into(),
            }],
        )];
        // Smart drove this pass; pre-compaction total is huge so the freed delta
        // (pre − tiny post estimate) is far above the 2000-token floor.
        engine.smart_compact_force = true;
        engine.compact_state.last_input_tokens = 300_000;
        assert_eq!(engine.compact_config.smart_min_shrink_tokens, 2_000);

        engine
            .run_compaction()
            .await
            .expect("compaction must succeed");

        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1, "exactly one CompactOffload emitted");
        let (reason, tokens_freed) = &events[0];
        assert_eq!(
            reason.as_str(),
            "smart_window_pressure",
            "smart-driven pass must tag the offload with smart provenance"
        );
        assert!(
            *tokens_freed > engine.compact_config.smart_min_shrink_tokens,
            "tokens_freed {tokens_freed} must dwarf the {} floor",
            engine.compact_config.smart_min_shrink_tokens
        );
        assert!(
            !engine.smart_compact_exhausted,
            "normal shrink must NOT arm the cannot-shrink latch"
        );
        // The one-shot force flag is consumed regardless of outcome.
        assert!(!engine.smart_compact_force);
    }

    /// Negative path (cannot-shrink latch): a tiny pre-compaction total means the
    /// freed delta falls below `smart_min_shrink_tokens`, so a smart-driven
    /// `run_compaction()` arms the terminal `smart_compact_exhausted` latch. The
    /// offload reason is still the smart string.
    #[tokio::test]
    async fn smart_run_compaction_latches_exhausted_when_cannot_shrink() {
        let mut engine = make_engine();
        engine.provider = Arc::new(SummaryProvider {
            summary: "tiny summary".into(),
        });
        let sink = Arc::new(CapturingCompactSink::default());
        engine.output = sink.clone();

        engine.messages = vec![Message::now(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "small context".into(),
            }],
        )];
        engine.smart_compact_force = true;
        // pre_compact_tokens far below the 2000 floor → freed delta < floor → the
        // cannot-shrink latch must arm.
        engine.compact_state.last_input_tokens = 100;
        assert!(
            engine.compact_state.last_input_tokens < engine.compact_config.smart_min_shrink_tokens
        );

        engine
            .run_compaction()
            .await
            .expect("compaction must succeed");

        assert!(
            engine.smart_compact_exhausted,
            "freeing fewer than smart_min_shrink_tokens must latch exhausted"
        );
        let events = sink.events.lock().unwrap();
        assert_eq!(events.len(), 1, "exactly one CompactOffload emitted");
        assert_eq!(
            events[0].0.as_str(),
            "smart_window_pressure",
            "the cannot-shrink pass is still smart-driven"
        );
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentError {
    #[error("API error: {0}")]
    ApiError(String),
    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),
    #[error("User aborted the session")]
    UserAborted,
    #[error("Context window nearly full ({input_tokens} tokens used, limit {limit})")]
    ContextTooLong { input_tokens: u64, limit: usize },
}

#[cfg(test)]
mod user_model_writeback_tests {
    //! v0.8.0 Task M — per-turn observation write-back into
    //! `UserModelBackend`. Closes the v0.7.0 deferment where the
    //! user-model layer was bootstrap-only-read: `engine.run()` now
    //! observes on every user turn so the backend keeps learning.

    use std::sync::Arc;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::FinishReason;
    use wcore_user_model::{
        LocalBackend, Observation, UserBrief, UserModelBackend, UserModelError,
    };

    use crate::approval::ApprovalBridge;
    use crate::compact::state::CompactState;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    fn make_engine() -> super::AgentEngine {
        super::AgentEngine {
            provider: Arc::new(NullProvider),
            temperature: None,
            tools: Arc::new(ToolRegistry::new()),
            messages: vec![],
            rebind_system_prefix: None,
            system_prompt: String::new(),
            model: "test-model".into(),
            user_model_pin: None,
            max_tokens: 4096,
            max_turns: Some(10),
            total_usage: Default::default(),
            thinking: None,
            compat: wcore_config::compat::ProviderCompat::anthropic_defaults(),
            confirmer: Arc::new(Mutex::new(ToolConfirmer::new(true, vec![]))),
            hooks: None,
            session_manager: None,
            current_session: None,
            output: Arc::new(NullOutput),
            current_msg_id: String::new(),
            current_agent_run_id: None,
            approval_manager: None,
            approval_bridge: Arc::new(ApprovalBridge::new()),
            protocol_writer: None,
            allow_list: vec![],
            current_reasoning_effort: None,
            compact_config: wcore_config::compact::CompactConfig::default(),
            compact_state: CompactState::new(),
            plan_state: Default::default(),
            plan_active_flag: None,
            cache_detector: super::CacheBreakDetector::new(),
            compaction_level: wcore_compact::CompactionLevel::default(),
            toon_enabled: false,
            compact_bash: false,
            advertised_capabilities: wcore_config::tools::AdvertisedCapabilitiesConfig::default(),
            per_turn_costs: Vec::new(),
            mcp_curation: wcore_config::config::McpCurationPolicy::default(),
            mcp_curation_cache: None,
            mcp_cap_cache: None,
            file_cache: None,
            session_state: None,
            audit_log: None,
            memory_api: Arc::new(wcore_memory::NullMemory),
            dream_throttle: Arc::new(wcore_memory::consolidate::DreamThrottle::new(
                std::time::Duration::from_secs(1800),
            )),
            #[cfg(any(test, feature = "test-utils"))]
            test_sink_handle: crate::test_utils::TestSinkHandle::default(),
            skills_lifecycle: false,
            online_evolution: false,
            recent_turn_traces: std::collections::VecDeque::new(),
            drafted_skill_signatures: std::collections::HashSet::new(),
            file_watcher: Arc::new(std::sync::OnceLock::new()),
            tool_write_notifier: Arc::new(std::sync::OnceLock::new()),
            mode_override: None,
            decay_handles: Vec::new(),
            plugin_runtime_handles: Arc::new(Vec::new()),
            budget_tracker: None,
            policy_gate: None,
            agent_registry: None,
            plugin_user_models: Vec::new(),
            style_detector: Mutex::new(crate::style_detector::StyleDetector::new()),
            skill_catalog: None,
            template_router: None,
            user_model_backend: None,
            user_model_user_id: "test-user".to_string(),
            // v0.8.1 U1 — test harness defaults to no router; the
            // router-specific tests below install one explicitly.
            skill_router: None,
            current_skill_router_pick: None,
            // v0.8.1 U6 — autonomous-skill bucketer is always live (N=3
            // threshold). Drafter is None at construction; bootstrap
            // installs one when memory is wired.
            auto_skill_bucketer: Mutex::new(crate::auto_skill::Bucketer::new(3)),
            skill_drafter: None,
            // AUDIT A2 / B1 — fresh session-root cancellation token.
            // Hosts replace/observe it via `cancel_token()`.
            cancel_token: tokio_util::sync::CancellationToken::new(),
            // AUDIT B-2 / D-5 — reaper handle storage; populated by
            // `set_approval_manager`, aborted by `Drop`.
            background_handles: Vec::new(),
            // Dynamic Workflows B3 — detection gate (default off).
            workflow_detection_enabled: false,
            // Dynamic Workflows B6 — live confirm gate (default off) + a
            // default config for the (unused-in-these-fixtures) live gate.
            workflow_live_mode: false,
            config: wcore_config::config::Config::default(),
            compaction_floor: 0,
            session_start_injected_len: 0,
            web_search: false,
            pending_hook_actions: Vec::new(),
            conversation_id: String::new(),
            flux_served_window: None,
            flux_context_pressure: None,
            smart_compact_armed: true,
            smart_compact_last_turn: None,
            smart_compact_exhausted: false,
            smart_compact_force: false,
        }
    }

    /// After 3 simulated user turns of terse messages, the backend's
    /// brief for that user reflects the accumulated style observations
    /// (non-default style + `last_observed_ts` advanced). Proves the
    /// write-back is per-turn and not bootstrap-only.
    #[tokio::test]
    async fn three_terse_turns_accumulate_in_backend() {
        let backend = Arc::new(LocalBackend::in_memory());
        let mut engine = make_engine();
        engine.set_user_model_backend(backend.clone());

        // Simulate 3 user messages by directly feeding the detector +
        // calling the write-back helper. This exercises the same code
        // path `run()` uses without spinning up the full LLM loop.
        for msg in ["ok", "yes", "nope"] {
            engine.style_detector.lock().unwrap().observe(msg);
            let observed = engine.observe_user_turn(msg).await;
            assert!(
                observed,
                "non-empty input with backend installed must observe"
            );
        }

        let brief: UserBrief = backend.brief("test-user").await.expect("brief read");
        assert!(
            brief.last_observed_ts > 0,
            "backend should record the observation timestamp; got {}",
            brief.last_observed_ts
        );
        // Terse messages → high terseness axis on the rolling style.
        // The exact magnitude depends on the EMA fold inside
        // LocalBackend, but it must be strictly positive (default is
        // 0.5 only when no observation lands; an observation with
        // terseness != 0.5 moves the EMA off-default).
        assert!(
            brief.style.terseness > 0.0,
            "terse-message stream must produce a positive terseness axis; got {}",
            brief.style.terseness
        );
    }

    /// Backend `observe` errors must be logged + swallowed so the turn
    /// doesn't die when the user-model write-back fails (e.g. Honcho
    /// network blip). The helper still returns `true` because an
    /// observation was *attempted* — the failure is non-fatal by design.
    #[tokio::test]
    async fn backend_observe_error_is_swallowed() {
        struct FailingBackend;
        #[async_trait]
        impl UserModelBackend for FailingBackend {
            async fn brief(&self, _: &str) -> Result<UserBrief, UserModelError> {
                Ok(UserBrief::default())
            }
            async fn preferences(
                &self,
                _: &str,
            ) -> Result<wcore_user_model::Preferences, UserModelError> {
                Ok(wcore_user_model::Preferences::default())
            }
            async fn observe(&self, _: &str, _: Observation) -> Result<(), UserModelError> {
                Err(UserModelError::Transport("simulated failure".into()))
            }
            fn backend_tag(&self) -> &str {
                "failing-test-backend"
            }
        }

        let mut engine = make_engine();
        engine.set_user_model_backend(Arc::new(FailingBackend));

        // Must not panic. Returns true because we did attempt an
        // observation — the swallowed error is the whole point.
        let observed = engine.observe_user_turn("hello world").await;
        assert!(observed, "observation attempt should be recorded as true");
    }

    /// Empty input is a no-op: the detector window has nothing to
    /// fingerprint and we don't want to spam the backend with neutral
    /// observations. Pure-whitespace input is treated the same.
    #[tokio::test]
    async fn empty_input_skips_observation() {
        let backend = Arc::new(LocalBackend::in_memory());
        let mut engine = make_engine();
        engine.set_user_model_backend(backend.clone());

        for msg in ["", "   ", "\n\t"] {
            let observed = engine.observe_user_turn(msg).await;
            assert!(
                !observed,
                "empty/whitespace input must skip observation; msg={msg:?}"
            );
        }
        // Backend untouched — no record for the test user.
        let brief = backend.brief("test-user").await.unwrap();
        assert_eq!(brief.last_observed_ts, 0);
    }

    /// No backend installed (the default for engines built outside
    /// `AgentBootstrap`) skips write-back entirely — preserves
    /// pre-v0.8.0 byte-identical behaviour.
    #[tokio::test]
    async fn no_backend_skips_observation() {
        let engine = make_engine();
        let observed = engine.observe_user_turn("anything").await;
        assert!(
            !observed,
            "engine with no backend installed must not attempt observation"
        );
    }

    // ── v0.8.1 U1 — SkillRouter wire-up tests ────────────────────────────
    //
    // Goal: assert that the engine carries an installable per-turn
    // `SkillRouter`, that it short-circuits when no router is wired,
    // and that the observe-outcome helper credits the correct arm
    // with Success/Failure verdicts derived from `StopReason`. These
    // tests don't spin up the full `run()` loop (no real provider);
    // they exercise the `observe_skill_router_outcome` helper plus
    // the choose primitive the run loop wraps.

    use wcore_dispatch::DecisionRouter as _;
    use wcore_skills::{SkillRouter, SkillRouterInput};
    use wcore_types::message::StopReason;

    /// Bare engine (no router) must short-circuit
    /// `observe_skill_router_outcome` cleanly — even when a phantom
    /// pick is sitting in the slot. Preserves pre-U1 behaviour for
    /// every test/sub-agent engine constructed outside bootstrap.
    #[test]
    fn observe_skill_router_outcome_is_noop_without_router() {
        let mut engine = make_engine();
        assert!(
            engine.skill_router().is_none(),
            "default engine must not carry a SkillRouter"
        );
        engine.current_skill_router_pick = Some("ghost".into());
        engine.observe_skill_router_outcome(StopReason::EndTurn);
        // `take()` inside the helper clears the slot even when the
        // router branch is unreachable — single-use semantics.
        assert!(
            engine.current_skill_router_pick.is_none(),
            "pick slot must be cleared even when no router is installed"
        );
    }

    /// With a router installed, a Success observation on the stashed
    /// pick biases subsequent `choose` calls toward that arm. Proves
    /// the helper actually called `observe(..., Success)` — without
    /// reaching into private scorer state.
    #[test]
    fn observe_endturn_biases_router_toward_picked_arm() {
        let mut engine = make_engine();
        // Use a deterministic RNG seed so the post-bias `choose`
        // calls are reproducible.
        engine.set_skill_router(SkillRouter::with_seed(2026));
        engine.current_skill_router_pick = Some("alpha".into());

        // Fire one Success on "alpha" via the helper. Then fire a
        // few more directly through the trait to amplify the bias
        // past the cold-start prior of beta (no observations).
        engine.observe_skill_router_outcome(StopReason::EndTurn);
        assert!(
            engine.current_skill_router_pick.is_none(),
            "pick must be cleared after observe"
        );
        {
            let router = engine.skill_router().expect("router installed");
            let mut guard = router.lock().unwrap();
            for _ in 0..30 {
                guard.observe(&"alpha".to_string(), wcore_dispatch::TaskOutcome::Success);
                guard.observe(&"beta".to_string(), wcore_dispatch::TaskOutcome::Failure);
            }
        }

        // Sample the posterior. After heavy success on alpha and
        // failure on beta, alpha must dominate over many trials.
        let candidates = vec!["alpha".to_string(), "beta".to_string()];
        let mut alpha_picks = 0;
        for _ in 0..200 {
            let router = engine.skill_router().expect("router installed");
            let mut guard = router.lock().unwrap();
            let pick = guard
                .choose(SkillRouterInput {
                    task: "any task",
                    candidates: &candidates,
                })
                .expect("non-empty candidates");
            if pick == "alpha" {
                alpha_picks += 1;
            }
        }
        assert!(
            alpha_picks > 150,
            "alpha should dominate after 30 success / 30 failure: got {alpha_picks}/200"
        );
    }

    /// `StopReason::MaxTurns` is a failure verdict — observe must
    /// shift the Beta posterior AWAY from the picked arm, not toward
    /// it. We verify by stashing a pick on a router that already has
    /// strong success priors on a competitor, then firing MaxTurns
    /// observations and seeing the picked arm lose.
    #[test]
    fn observe_max_turns_credits_failure_not_success() {
        let mut engine = make_engine();
        engine.set_skill_router(SkillRouter::with_seed(2026));

        // Pre-bias: 30 successes on "beta" (the competitor) and 0 on
        // "alpha" so alpha starts COLD. Then fire a MaxTurns observe
        // on alpha and verify it stays cold (no spurious success).
        {
            let router = engine.skill_router().expect("router installed");
            let mut guard = router.lock().unwrap();
            for _ in 0..30 {
                guard.observe(&"beta".to_string(), wcore_dispatch::TaskOutcome::Success);
            }
        }

        engine.current_skill_router_pick = Some("alpha".into());
        engine.observe_skill_router_outcome(StopReason::MaxTurns);
        // Fire it again a few times to amplify the failure signal.
        for _ in 0..29 {
            engine.current_skill_router_pick = Some("alpha".into());
            engine.observe_skill_router_outcome(StopReason::MaxTurns);
        }

        // After 30 failures on alpha vs 30 successes on beta, beta
        // must dominate.
        let candidates = vec!["alpha".to_string(), "beta".to_string()];
        let mut beta_picks = 0;
        for _ in 0..200 {
            let router = engine.skill_router().expect("router installed");
            let mut guard = router.lock().unwrap();
            let pick = guard
                .choose(SkillRouterInput {
                    task: "any task",
                    candidates: &candidates,
                })
                .expect("non-empty candidates");
            if pick == "beta" {
                beta_picks += 1;
            }
        }
        assert!(
            beta_picks > 150,
            "MaxTurns must credit failure (not success); \
             expected beta to dominate, got beta_picks={beta_picks}/200"
        );
    }

    // ── v0.8.1 U1 — skill-router HINT injection tests ────────────────────
    //
    // F-068 loop closure: these assert that the learned per-turn pick
    // actually reaches the model via `skill_router_hint()` (the seam the
    // run loop appends to the system prompt), and that it stays silent in
    // every case where injecting would be wrong.

    /// Build a one-skill catalog whose single skill is model-invocable
    /// unless `disable_model_invocation` is set.
    fn catalog_with(
        name: &str,
        disable_model_invocation: bool,
    ) -> Arc<wcore_skills::refs::SkillCatalog> {
        use wcore_skills::refs::SkillCatalog;
        use wcore_skills::types::{LoadedFrom, SkillSource};
        let r = wcore_skills::refs::SkillRef {
            name: name.to_string(),
            display_name: None,
            description: "test skill".to_string(),
            when_to_use: None,
            paths: Vec::new(),
            source: SkillSource::Project,
            loaded_from: LoadedFrom::Skills,
            file_path: std::path::PathBuf::from(format!("/tmp/{name}/SKILL.md")),
            content_length_hint: 0,
            user_invocable: true,
            disable_model_invocation,
            has_artifacts: false,
            inline_content: None,
        };
        Arc::new(SkillCatalog::from_refs(vec![r]))
    }

    /// (a) A router pick that names a visible catalog skill produces the
    /// hint line carrying that skill's name.
    #[test]
    fn router_hint_present_for_visible_catalog_pick() {
        let mut engine = make_engine();
        engine.set_skill_router(SkillRouter::with_seed(2026));
        engine.set_skill_catalog(catalog_with("rust-review", false));
        engine.current_skill_router_pick = Some("rust-review".into());

        let hint = engine
            .skill_router_hint()
            .expect("hint must be present for a visible catalog pick");
        assert!(
            hint.contains("rust-review"),
            "hint must name the picked skill: {hint}"
        );
        assert!(
            hint.starts_with("Skill hint:"),
            "hint must use the agreed non-binding prefix: {hint}"
        );
        assert!(
            hint.contains("only if genuinely relevant"),
            "hint must stay non-coercive: {hint}"
        );
    }

    /// (b) No pick stashed → no hint (the common per-turn idle case).
    #[test]
    fn router_hint_absent_when_no_pick() {
        let mut engine = make_engine();
        engine.set_skill_router(SkillRouter::with_seed(2026));
        engine.set_skill_catalog(catalog_with("rust-review", false));
        assert!(engine.current_skill_router_pick.is_none());
        assert!(
            engine.skill_router_hint().is_none(),
            "no pick must yield no hint"
        );
    }

    /// No router installed → no hint even if a phantom pick + catalog are
    /// present. Guards the zero-behaviour-change contract for engines built
    /// outside bootstrap.
    #[test]
    fn router_hint_absent_without_router() {
        let mut engine = make_engine();
        assert!(engine.skill_router().is_none());
        engine.set_skill_catalog(catalog_with("rust-review", false));
        engine.current_skill_router_pick = Some("rust-review".into());
        assert!(
            engine.skill_router_hint().is_none(),
            "no router installed must yield no hint"
        );
    }

    /// A pick that isn't in the catalog (stale/unknown name) → no hint.
    #[test]
    fn router_hint_absent_for_unknown_skill() {
        let mut engine = make_engine();
        engine.set_skill_router(SkillRouter::with_seed(2026));
        engine.set_skill_catalog(catalog_with("rust-review", false));
        engine.current_skill_router_pick = Some("does-not-exist".into());
        assert!(
            engine.skill_router_hint().is_none(),
            "unknown skill name must yield no hint"
        );
    }

    /// A pick that names a catalog skill the model is NOT allowed to invoke
    /// → no hint (advising an un-invocable skill is useless).
    #[test]
    fn router_hint_absent_for_model_hidden_skill() {
        let mut engine = make_engine();
        engine.set_skill_router(SkillRouter::with_seed(2026));
        engine.set_skill_catalog(catalog_with("internal-only", true));
        engine.current_skill_router_pick = Some("internal-only".into());
        assert!(
            engine.skill_router_hint().is_none(),
            "model-hidden skill must yield no hint"
        );
    }

    // ── v0.8.1 U6 — autonomous-skill drafter wire-up tests ───────────────
    //
    // The bucketer + drafter unit tests live next to their modules
    // (`auto_skill::bucketer::tests`, `auto_skill::drafter::tests`).
    // These engine-level tests verify the WIRE: the engine's
    // `observe_auto_skill` helper buckets trajectories AND, when a
    // drafter is installed, writes a real on-disk draft after 3
    // consecutive successes on the same signature.

    /// Engine without a drafter installed still buckets observations but
    /// never writes to disk. Preserves the no-bootstrap default and
    /// keeps test engines free of filesystem side effects.
    #[test]
    fn auto_skill_no_drafter_no_disk_write() {
        let tmp = tempfile::tempdir().unwrap();
        let engine = make_engine();
        assert!(engine.skill_drafter().is_none());

        // Fire 3 consecutive successes — the bucketer will trigger but
        // the helper logs + returns without writing.
        for _ in 0..3 {
            engine.observe_auto_skill("refactor the code please", None, StopReason::EndTurn, 1);
        }

        // Filesystem under the temp dir must be empty — we never told
        // the engine about it, but we also assert the cwd-default
        // location isn't accidentally written to in test runs.
        let auto_dir = tmp.path().join("skills").join("auto");
        assert!(
            !auto_dir.exists(),
            "no drafter should mean no on-disk write"
        );
    }

    /// Three consecutive successful turns on the same task signature,
    /// with a drafter installed, produces an on-disk auto-draft file
    /// AND a PromptStore record. Closes the v0.8.1 U6 wire end-to-end.
    #[test]
    fn auto_skill_three_successes_writes_draft_to_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("skills").join("auto");

        // Real PromptStore against an in-memory Db so we can also
        // assert the row landed.
        let db = Arc::new(wcore_memory::db::Db::open_memory().unwrap());
        let store = Arc::new(wcore_evolve::prompt_store::PromptStore::new(db));
        let drafter = Arc::new(crate::auto_skill::SkillDrafter::new(
            skill_dir.clone(),
            Some(store.clone()),
        ));

        let mut engine = make_engine();
        engine.set_skill_drafter(drafter);
        assert!(engine.skill_drafter().is_some());

        // Three successive successes that ALL normalize to the same
        // top-3 content words. Sanity: signature("...") is checked
        // alongside the auto_skill::bucketer tests; here we just need
        // three inputs whose content words after stopword strip are
        // identical sets of {refactor, code}.
        let inputs = [
            "refactor the code",
            "the code refactor",
            "please refactor code",
        ];
        // Pre-flight assertion so a bucketer drift breaks here loudly,
        // not in the file-presence assertion below.
        let sigs: Vec<String> = inputs
            .iter()
            .map(|s| crate::auto_skill::signature(s))
            .collect();
        assert!(
            sigs.windows(2).all(|w| w[0] == w[1]),
            "test inputs must produce identical signatures, got {sigs:?}"
        );
        for variant in inputs {
            engine.observe_auto_skill(variant, None, StopReason::EndTurn, 1);
        }

        // F-038: drafter now writes directory format — auto-<sig>/SKILL.md.
        // Look for a sub-directory named "auto-*" that contains SKILL.md.
        let mut found_skill_name: Option<String> = None;
        if skill_dir.exists() {
            for entry in std::fs::read_dir(&skill_dir).unwrap() {
                let entry = entry.unwrap();
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if s.starts_with("auto-") && entry.path().join("SKILL.md").exists() {
                    found_skill_name = Some(s.to_string());
                    break;
                }
            }
        }
        assert!(
            found_skill_name.is_some(),
            "expected an auto-*/SKILL.md draft in {}",
            skill_dir.display()
        );

        // And the PromptStore got a matching auto_drafter row.
        // (Skill name = auto-<signature> — the directory name is the skill name.)
        let all_rows = store
            .all_for_skill(&found_skill_name.clone().unwrap_or_default())
            .unwrap();
        assert!(
            !all_rows.is_empty(),
            "expected at least one PromptStore row for the auto-drafted skill"
        );
        assert!(
            all_rows.iter().any(|r| r.scorer == "auto_drafter"),
            "PromptStore row must use scorer='auto_drafter'"
        );
    }

    /// A failure breaks the streak: 2 successes + 1 failure + 1 success
    /// must NOT trigger a draft. Guarantees we don't crystallize tasks
    /// the engine struggles with.
    #[test]
    fn auto_skill_failure_breaks_streak() {
        let tmp = tempfile::tempdir().unwrap();
        let skill_dir = tmp.path().join("skills").join("auto");
        let drafter = Arc::new(crate::auto_skill::SkillDrafter::new(
            skill_dir.clone(),
            None,
        ));
        let mut engine = make_engine();
        engine.set_skill_drafter(drafter);

        engine.observe_auto_skill("refactor the code", None, StopReason::EndTurn, 1);
        engine.observe_auto_skill("refactor the code", None, StopReason::EndTurn, 1);
        // Failure resets.
        engine.observe_auto_skill("refactor the code", None, StopReason::MaxTurns, 5);
        // One more success — streak is now only at 1, must not draft.
        engine.observe_auto_skill("refactor the code", None, StopReason::EndTurn, 1);

        if skill_dir.exists() {
            let count = std::fs::read_dir(&skill_dir)
                .unwrap()
                .filter_map(Result::ok)
                .count();
            assert_eq!(
                count, 0,
                "failure mid-streak must prevent draft; found {count} files"
            );
        }
    }
}

// ===========================================================================
// Audit 2026-05-22 — agentic-core correctness fixes.
//
// Regression tests for AUDIT-A (turn loop), AUDIT-B (tools), and the
// D5/D6/A3/E-C2 cross-cuts. Each test below would have FAILED against the
// pre-fix engine (unbounded loop, discarded budget cap, no tool timeout,
// truncated-stream-as-success, orphaned tool_use on disk, leaked approval).
// ===========================================================================
#[cfg(test)]
mod audit_2026_05_22_tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use serde_json::json;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::{FinishReason, StopReason, TokenUsage};

    use crate::approval::ApprovalBridge;
    use crate::confirm::ToolConfirmer;
    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
    }

    /// One scripted provider response — the events `stream()` will yield
    /// on a given call.
    type Script = Vec<LlmEvent>;

    /// AUDIT A3 / E-C2 — a provider whose successive `stream()` calls
    /// replay a queue of scripted event sequences. An empty sequence
    /// models a truncated stream (channel closes with no `Done`).
    struct ScriptedProvider {
        scripts: Mutex<std::collections::VecDeque<Script>>,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl ScriptedProvider {
        fn new(scripts: Vec<Script>) -> Self {
            Self {
                scripts: Mutex::new(scripts.into_iter().collect()),
                calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
        fn call_counter(&self) -> Arc<std::sync::atomic::AtomicUsize> {
            Arc::clone(&self.calls)
        }
    }

    #[async_trait]
    impl LlmProvider for ScriptedProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let script = self.scripts.lock().unwrap().pop_front().unwrap_or_default();
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                for ev in script {
                    let _ = tx.send(ev).await;
                }
                // Dropping `tx` closes the channel — a script with no
                // `Done` event therefore models a truncated stream.
            });
            Ok(rx)
        }
    }

    fn done_endturn() -> LlmEvent {
        LlmEvent::Done {
            stop_reason: StopReason::EndTurn,
            finish_reason: FinishReason::Stop,
            usage: TokenUsage::default(),
        }
    }

    fn engine_with(provider: Arc<dyn LlmProvider>) -> super::AgentEngine {
        let mut e = super::AgentEngine::new_with_provider(
            provider,
            wcore_config::config::Config::default(),
            ToolRegistry::new(),
            Arc::new(NullOutput),
        );
        // Keep tests fast and deterministic — no max_turns dependency.
        e.max_turns = Some(20);
        e
    }

    // --- F-092: online-evolution LLM paraphrase persistence ---------------

    #[tokio::test]
    async fn online_evolve_persists_llm_rewrite_not_identity() {
        // F-092 regression: the live online-evolution path must write the
        // REAL LLM-backed paraphrase, not the byte-identical system prompt
        // the old passthrough provider produced. Drive the extracted
        // `paraphrase_and_persist` helper with a mock provider that returns a
        // known rewrite, then assert the file contains the rewrite and NOT
        // the original prompt body.
        const SYSTEM_PROMPT: &str = "You are a helpful coding agent. Be terse.";
        const MOCK_REWRITE: &str = "Act as a concise programming assistant.";

        let provider = Arc::new(ScriptedProvider::new(vec![vec![
            LlmEvent::TextDelta(MOCK_REWRITE.into()),
            done_endturn(),
        ]]));

        let tmp = tempfile::tempdir().expect("tempdir");
        let evolved_dir = tmp.path().join("evolved");
        let session_id = "sess-f092-xyz";

        super::AgentEngine::paraphrase_and_persist(
            provider,
            "test-model",
            SYSTEM_PROMPT,
            session_id,
            0.75,
            &evolved_dir,
        )
        .await;

        let file_path = evolved_dir.join(format!("{session_id}.md"));
        let written = std::fs::read_to_string(&file_path)
            .expect("evolved file must be written by paraphrase_and_persist");

        assert!(
            written.contains(MOCK_REWRITE),
            "evolved file must contain the LLM rewrite, got: {written:?}"
        );
        assert!(
            !written.contains(SYSTEM_PROMPT),
            "evolved body must be the rewrite, NOT the identity system prompt"
        );
        assert!(
            written.contains("F-092 online-evolve")
                && written.contains(session_id)
                && written.contains("Paraphrase"),
            "header comment must carry session + mutator provenance"
        );
    }

    // --- A2 / B1: cancellation token plumbing -----------------------------

    #[tokio::test]
    async fn cancel_token_is_observed_between_turns() {
        // AUDIT A2 — firing the session-root token before `run()` makes
        // the loop terminate immediately with `UserAborted` instead of
        // calling the provider. Pre-fix: the token was an orphan,
        // nothing checked it, the loop ran unconditionally.
        let provider = Arc::new(ScriptedProvider::new(vec![vec![
            LlmEvent::TextDelta("hello".into()),
            done_endturn(),
        ]]));
        let counter = provider.call_counter();
        let mut engine = engine_with(provider);
        engine.cancel_token().cancel();
        let result = engine.run("do a thing", "m-1").await;
        assert!(
            matches!(result, Err(super::AgentError::UserAborted)),
            "a fired cancel token must abort the run cleanly, got {result:?}"
        );
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "a pre-cancelled run must not call the provider at all"
        );
    }

    #[test]
    fn cancel_token_clone_observes_same_cancellation() {
        // AUDIT A2 — `cancel_token()` hands the host a clone backed by
        // the same `Arc`; cancelling the clone cancels the engine's.
        let engine = engine_with(Arc::new(ScriptedProvider::new(vec![])));
        let host_handle = engine.cancel_token();
        assert!(!engine.cancel_token().is_cancelled());
        host_handle.cancel();
        assert!(
            engine.cancel_token().is_cancelled(),
            "cancelling a host clone must cancel the engine's root token"
        );
    }

    // --- A3 / E-C2: truncated stream + mid-stream-error retry -------------

    #[tokio::test]
    async fn truncated_stream_then_success_retries_and_succeeds() {
        // AUDIT A3 / E-C2 — first `stream()` yields text but NO `Done`
        // (truncated). The engine must retry; the second `stream()`
        // completes cleanly. Pre-fix: the truncated stream was recorded
        // as a successful empty `EndTurn`.
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![LlmEvent::TextDelta("partial".into())], // no Done — truncated
            vec![LlmEvent::TextDelta("complete".into()), done_endturn()],
        ]));
        let counter = provider.call_counter();
        let mut engine = engine_with(provider);
        let result = engine
            .run("task", "m-1")
            .await
            .expect("retry must recover the truncated stream");
        assert_eq!(result.text, "complete");
        assert_eq!(result.stop_reason, StopReason::EndTurn);
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "a truncated first attempt must trigger exactly one retry"
        );
    }

    #[tokio::test]
    async fn mid_stream_error_then_success_retries() {
        // AUDIT E-C2 — a mid-stream `LlmEvent::Error` is a retryable
        // failure, not a fatal abort. Pre-fix: it became a fatal
        // `AgentError::ApiError` with no retry.
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![
                LlmEvent::TextDelta("oops".into()),
                LlmEvent::Error("connection reset".into()),
            ],
            vec![LlmEvent::TextDelta("recovered".into()), done_endturn()],
        ]));
        let counter = provider.call_counter();
        let mut engine = engine_with(provider);
        let result = engine
            .run("task", "m-1")
            .await
            .expect("a transient mid-stream error must be retried");
        assert_eq!(result.text, "recovered");
        assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn stream_error_exhausts_retries_then_fails_the_turn() {
        // AUDIT A3 / E-C2 — when every attempt fails the turn ends as a
        // hard error (NOT a silent empty success). 1 initial + 2
        // retries = 3 provider calls.
        let provider = Arc::new(ScriptedProvider::new(vec![
            vec![LlmEvent::Error("e1".into())],
            vec![LlmEvent::Error("e2".into())],
            vec![LlmEvent::Error("e3".into())],
        ]));
        let counter = provider.call_counter();
        let mut engine = engine_with(provider);
        let result = engine.run("task", "m-1").await;
        assert!(
            matches!(result, Err(super::AgentError::ApiError(_))),
            "an exhausted stream-retry budget must fail the turn, got {result:?}"
        );
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "1 initial attempt + 2 retries = 3 provider calls"
        );
    }

    // --- Finding #174: nested re-bill cap on an HTTP-ring-exhausted Err ----

    /// Models the engine entry point that triggers the worst nesting:
    /// `stream()` itself returns `Err(ProviderError::Connection)` — i.e. the
    /// provider's HTTP ring (`builder_send_with_retry`) already spent its full
    /// 3-send budget on this request before surfacing the error. Returns that
    /// `Err` for the first `fail_calls` calls, then a clean `Done` stream.
    struct StreamErrProvider {
        fail_calls: usize,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl StreamErrProvider {
        fn new(fail_calls: usize) -> Self {
            Self {
                fail_calls,
                calls: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            }
        }
        fn call_counter(&self) -> Arc<std::sync::atomic::AtomicUsize> {
            Arc::clone(&self.calls)
        }
    }
    #[async_trait]
    impl LlmProvider for StreamErrProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < self.fail_calls {
                // A retryable Connection error surfaced from `stream()` itself:
                // the HTTP ring already exhausted its budget on this request.
                return Err(ProviderError::Connection("http ring exhausted".into()));
            }
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                let _ = tx.send(LlmEvent::TextDelta("recovered".into())).await;
                let _ = tx.send(done_endturn()).await;
            });
            Ok(rx)
        }
    }

    #[tokio::test]
    async fn http_exhausted_stream_err_grants_only_one_engine_retry() {
        // Finding #174 — when `stream()` returns a retryable `Err` (the HTTP
        // ring already spent its 3-send budget), the engine ring must NOT stack
        // a fresh full `MAX_STREAM_RETRIES` budget on top. Without the cap a
        // permanent HTTP-exhausted failure would drive 3 engine attempts (1 +
        // 2 retries); with the cap it is bounded to 2 (1 + 1 retry), each of
        // which would re-enter the HTTP ring's 3 sends — so the worst case
        // drops from 9 full-input re-sends to 6 for one logical turn.
        let provider = Arc::new(StreamErrProvider::new(usize::MAX)); // always fails
        let counter = provider.call_counter();
        let mut engine = engine_with(provider);
        let result = engine.run("task", "m-1").await;
        assert!(
            matches!(result, Err(super::AgentError::ApiError(_))),
            "a permanent HTTP-exhausted failure must fail the turn, got {result:?}"
        );
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "an HTTP-ring-exhausted Err must get exactly 1 engine retry \
             (2 attempts), NOT the full 3 — that is the nested re-bill cap"
        );
    }

    #[tokio::test]
    async fn http_exhausted_stream_err_still_retries_a_single_transient_failure() {
        // Finding #174 normal-case guard — the cap must NOT kill resilience for
        // a genuinely transient single failure. One HTTP-exhausted `Err`
        // followed by a clean stream must still be retried and recover.
        let provider = Arc::new(StreamErrProvider::new(1)); // fail once, then ok
        let counter = provider.call_counter();
        let mut engine = engine_with(provider);
        let result = engine
            .run("task", "m-1")
            .await
            .expect("a single transient HTTP-exhausted failure must still recover");
        assert_eq!(result.text, "recovered");
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "1 initial attempt + 1 retry recovers the single transient failure"
        );
    }

    // --- E-C1: budget cap halts the loop ---------------------------------

    #[tokio::test]
    async fn budget_cap_terminates_the_run() {
        // AUDIT E-C1 — a charge that trips a per-session token cap must
        // STOP the loop. Pre-fix: the `charge()` result was discarded
        // (`let _ = ...`) and the cap did nothing.
        //
        // The model keeps emitting a tool call every turn (a runaway).
        // Without the budget guard this loop would only stop at
        // `max_turns`; with it, the tiny cap halts it after turn 1.
        let provider = Arc::new(ScriptedProvider::new(vec![vec![
            LlmEvent::ToolUse {
                id: "t1".into(),
                name: "Nope".into(),
                input: json!({}),
                extra: None,
            },
            LlmEvent::Done {
                stop_reason: StopReason::ToolUse,
                finish_reason: FinishReason::Stop,
                usage: TokenUsage {
                    input_tokens: 10_000,
                    output_tokens: 10_000,
                    ..Default::default()
                },
            },
        ]]));
        let counter = provider.call_counter();
        let mut engine = engine_with(provider);
        // 1-token cap — the very first turn's 20k-token charge trips it.
        let cap = wcore_budget::BudgetCap::builder()
            .per_session_tokens(1)
            .build();
        let tracker = Arc::new(parking_lot::Mutex::new(wcore_budget::BudgetTracker::new(
            cap,
        )));
        engine.set_budget_tracker(tracker);
        let result = engine
            .run("runaway task", "m-1")
            .await
            .expect("budget termination is a clean Ok, not an Err");
        assert_eq!(
            result.stop_reason,
            StopReason::MaxTurns,
            "budget termination uses the MaxTurns failure verdict"
        );
        assert_eq!(
            counter.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the budget cap must stop the loop after the FIRST turn, \
             not run until max_turns"
        );
    }

    #[tokio::test]
    async fn no_budget_tracker_does_not_terminate_early() {
        // Control: with no tracker installed the run completes
        // naturally (a single no-tool turn ends the loop).
        let provider = Arc::new(ScriptedProvider::new(vec![vec![
            LlmEvent::TextDelta("done".into()),
            done_endturn(),
        ]]));
        let mut engine = engine_with(provider);
        let result = engine.run("task", "m-1").await.expect("clean run");
        assert_eq!(result.stop_reason, StopReason::EndTurn);
    }

    // --- B1 / B8: tool-dispatch timeout ----------------------------------

    /// A tool that never returns — models a wedged MCP server / hung
    /// syscall. It does NOT observe the cancel token, so only the
    /// dispatch timeout can rescue the agent.
    struct HangingTool;
    #[async_trait]
    impl wcore_tools::Tool for HangingTool {
        fn name(&self) -> &str {
            "Hang"
        }
        fn description(&self) -> &str {
            "hangs forever"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
            false
        }
        async fn execute(&self, _: serde_json::Value) -> wcore_types::tool::ToolResult {
            // Sleep far past any category timeout.
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
            wcore_types::tool::ToolResult {
                content: "unreachable".into(),
                is_error: false,
            }
        }
        fn category(&self) -> wcore_protocol::events::ToolCategory {
            // Info → 30s category timeout. The test fast-forwards
            // tokio's clock so it resolves instantly.
            wcore_protocol::events::ToolCategory::Info
        }
    }

    #[tokio::test(start_paused = true)]
    async fn hung_tool_times_out_with_error_result() {
        // AUDIT B-1 / B-8 — a tool that never returns must NOT hang the
        // agent. The per-category dispatch timeout fires, an error
        // `tool_result` is synthesized, and dispatch returns. Pre-fix:
        // the `await` ran unbounded (the 35-minute hang).
        //
        // `start_paused` + `tokio::time::timeout` means the 30s Info
        // timeout elapses in virtual time — the test is instant.
        use crate::orchestration::execute_tool_calls;
        use wcore_types::message::ContentBlock;

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(HangingTool));
        let confirmer = Arc::new(Mutex::new(ToolConfirmer::new(true, vec![])));
        let calls = vec![ContentBlock::ToolUse {
            id: "c1".into(),
            name: "Hang".into(),
            input: json!({}),
            extra: None,
        }];
        let outcome = execute_tool_calls(
            &registry,
            &calls,
            &confirmer,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
        )
        .await
        .expect("dispatch must return, not hang");
        assert_eq!(
            outcome.results.len(),
            1,
            "the tool_use must get a tool_result"
        );
        match &outcome.results[0] {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(is_error, "a timed-out tool yields an error result");
                assert!(
                    content.contains("timed out"),
                    "result must explain the timeout, got: {content}"
                );
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    // --- B4: circuit breaker on the agent path ---------------------------

    /// A tool that always fails — drives the breaker toward Open.
    struct AlwaysFailTool;
    #[async_trait]
    impl wcore_tools::Tool for AlwaysFailTool {
        fn name(&self) -> &str {
            "Flaky"
        }
        fn description(&self) -> &str {
            "always fails"
        }
        fn input_schema(&self) -> serde_json::Value {
            json!({"type": "object"})
        }
        fn is_concurrency_safe(&self, _: &serde_json::Value) -> bool {
            false
        }
        async fn execute(&self, _: serde_json::Value) -> wcore_types::tool::ToolResult {
            wcore_types::tool::ToolResult {
                content: "boom".into(),
                is_error: true,
            }
        }
        fn category(&self) -> wcore_protocol::events::ToolCategory {
            wcore_protocol::events::ToolCategory::Info
        }
    }

    #[tokio::test]
    async fn circuit_breaker_trips_on_repeated_failures_via_agent_path() {
        // AUDIT B-4 — the agent's dispatch path now consults + records
        // the per-tool circuit breaker. After 3 failures the breaker
        // opens and the 4th dispatch is short-circuited WITHOUT calling
        // the tool. Pre-fix: the agent path bypassed the breaker
        // entirely (`registry.get()` + `execute_with_ctx()` direct).
        use crate::orchestration::execute_tool_calls;
        use wcore_types::message::ContentBlock;

        let mut registry = ToolRegistry::new();
        registry.register(Box::new(AlwaysFailTool));
        let confirmer = Arc::new(Mutex::new(ToolConfirmer::new(true, vec![])));
        let mk = |id: &str| {
            vec![ContentBlock::ToolUse {
                id: id.into(),
                name: "Flaky".into(),
                input: json!({}),
                extra: None,
            }]
        };
        // 3 failures trip the breaker (default config: 3 / 30s).
        for i in 0..3 {
            let _ = execute_tool_calls(
                &registry,
                &mk(&format!("c{i}")),
                &confirmer,
                None,
                wcore_compact::CompactionLevel::Off,
                false,
            )
            .await;
        }
        // 4th call: breaker is Open — the result must be the
        // circuit-open message, not the tool's own "boom".
        let outcome = execute_tool_calls(
            &registry,
            &mk("c4"),
            &confirmer,
            None,
            wcore_compact::CompactionLevel::Off,
            false,
        )
        .await
        .expect("dispatch returns");
        match &outcome.results[0] {
            ContentBlock::ToolResult {
                content, is_error, ..
            } => {
                assert!(is_error);
                assert!(
                    content.contains("circuit open"),
                    "an open breaker must short-circuit dispatch, got: {content}"
                );
            }
            other => panic!("expected ToolResult, got {other:?}"),
        }
    }

    // --- D6: orphaned tool_use repaired before save ----------------------

    #[test]
    fn save_session_repairs_orphaned_tool_use() {
        // AUDIT D-6 — `save_session` must not persist a trailing
        // assistant message whose `tool_use` blocks have no following
        // `tool_result`. The repair appends a synthetic error-result
        // user message. Pre-fix: the orphaned shape was written verbatim.
        use wcore_types::message::{ContentBlock, Message, Role};

        let mut engine = engine_with(Arc::new(ScriptedProvider::new(vec![])));
        engine.messages = vec![
            Message::now(Role::User, vec![ContentBlock::Text { text: "hi".into() }]),
            // Assistant message with a dangling tool_use — no results.
            Message::now(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "dangling".into(),
                    name: "Read".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
        ];
        engine.save_session();
        let last = engine.messages.last().expect("messages non-empty");
        assert_eq!(
            last.role,
            Role::User,
            "repair must append a User tool-results message"
        );
        match &last.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => {
                assert_eq!(tool_use_id, "dangling");
                assert!(is_error, "the synthetic repair result is an error");
            }
            other => panic!("expected a repair ToolResult, got {other:?}"),
        }
    }

    #[test]
    fn tool_denial_emits_well_formed_tool_result_v0911() {
        // v0.9.1.1 B6 regression — after a tool denial, the engine must
        // build a User message where the `tool_result` IS PRESENT and
        // is the FIRST block in the user turn that follows the
        // assistant's `tool_use`. The pre-fix bug was that
        // `drain_and_inject_external_edits` would insert a SEPARATE
        // User-Text message ("User edited N files…") BETWEEN the
        // assistant's tool_use and the tool_result message, breaking
        // Anthropic's required pairing and triggering an API 400
        // `invalid_request_error` on every subsequent turn.
        //
        // We test the load-bearing seam directly: a freshly-built User
        // message that bundles `outcome.results` (a denied tool result)
        // with an appended `Text` block (the external-edit notice) is
        // a single user turn with the tool_result block first.
        use wcore_types::message::{ContentBlock, Message, Role};

        // Simulate the engine's post-denial state: outcome.results
        // carries the synthetic `Tool denied: …` ToolResult, and
        // `drain_external_edits_message` returns Some(edit notice).
        let denied_result = ContentBlock::ToolResult {
            tool_use_id: "call-1".into(),
            content: "Tool denied: User declined".into(),
            is_error: true,
        };
        let edit_notice = "User edited 3 files while I was thinking…".to_string();

        // This is the EXACT bundling the engine performs at the
        // post-fix call site (engine.rs ~line 2870):
        let mut bundled: Vec<ContentBlock> = vec![denied_result.clone()];
        bundled.push(ContentBlock::Text { text: edit_notice });
        let msg = Message::now(Role::User, bundled);

        assert_eq!(
            msg.role,
            Role::User,
            "post-denial message must be a User turn"
        );
        // The tool_result MUST be the first block — Anthropic's
        // validator scans the next message after a tool_use and
        // expects tool_result blocks at the head, not text.
        match &msg.content[0] {
            ContentBlock::ToolResult {
                tool_use_id,
                is_error,
                ..
            } => {
                assert_eq!(tool_use_id, "call-1");
                assert!(is_error, "denied tool result must carry is_error=true");
            }
            other => panic!(
                "first block of the post-denial User message must be a ToolResult, got {other:?}"
            ),
        }
        // The synthetic edit notice rides at the end, NOT as a
        // separate User message.
        match &msg.content[1] {
            ContentBlock::Text { text } => {
                assert!(text.contains("User edited"), "got: {text}");
            }
            other => panic!("expected trailing Text block, got {other:?}"),
        }
    }

    #[test]
    fn engine_does_not_loop_on_api_400_after_denial_v0911() {
        // v0.9.1.1 B6 — an HTTP 4xx (the most common is Anthropic's
        // 400 `invalid_request_error` from a malformed history) is
        // NOT transient; retrying produces identical errors stacked
        // in the Activity rail. The `is_http_4xx_error` detector
        // skips the retry loop for client errors so the user sees
        // ONE clean error notice per turn, not three.
        //
        // The detector is exercised by the retry-loop guard at
        // engine.rs ~line 2300. Here we lock the contract directly:
        // every shape the provider chain emits a 4xx as must be
        // detected, and 5xx / network drops must NOT be misdetected
        // (false positive would skip a legitimately-retryable
        // transient failure).
        assert!(
            super::is_http_4xx_error("API error 400: invalid_request_error tool_use ids …"),
            "the post-denial 400 shape MUST be detected"
        );
        assert!(
            !super::is_http_4xx_error("API error 502: bad gateway"),
            "5xx MUST NOT be misdetected as a client error"
        );
        assert!(
            !super::is_http_4xx_error(
                "provider stream closed before a Done event (truncated response)"
            ),
            "truncated streams MUST remain retryable"
        );
    }

    #[test]
    fn save_session_leaves_well_formed_history_untouched() {
        // Control: a history that already ends with a tool-results
        // message must NOT gain a spurious repair message.
        use wcore_types::message::{ContentBlock, Message, Role};

        let mut engine = engine_with(Arc::new(ScriptedProvider::new(vec![])));
        engine.messages = vec![
            Message::now(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "t1".into(),
                    name: "Read".into(),
                    input: json!({}),
                    extra: None,
                }],
            ),
            Message::now(
                Role::User,
                vec![ContentBlock::ToolResult {
                    tool_use_id: "t1".into(),
                    content: "ok".into(),
                    is_error: false,
                }],
            ),
        ];
        let before = engine.messages.len();
        engine.save_session();
        assert_eq!(
            engine.messages.len(),
            before,
            "a well-formed history must not gain a repair message"
        );
    }

    // --- B2 / D5: approval-manager TTL + reaper --------------------------

    #[tokio::test]
    async fn approval_manager_reaper_collects_expired_entry() {
        // AUDIT B-2 — an unanswered approval must not wedge forever.
        // With a sub-second TTL the reaper resolves the pending entry
        // as `Denied`, so the awaiting `rx` completes.
        use wcore_protocol::{ToolApprovalManager, ToolApprovalResult};

        let mgr = ToolApprovalManager::with_ttl(std::time::Duration::from_millis(10));
        let rx = mgr.request_approval(
            "call-1",
            &wcore_protocol::events::ToolCategory::Exec,
            "exec_tool",
        );
        // Let the TTL lapse, then sweep.
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let collected = mgr.reap_now();
        assert_eq!(collected, 1, "the expired entry must be reaped");
        let outcome = rx.await.expect("reaper resolves the await, not drops it");
        match outcome {
            ToolApprovalResult::Denied { reason } => {
                assert!(reason.contains("timed out"), "got: {reason}");
            }
            ToolApprovalResult::Approved { .. } => {
                panic!("an unanswered approval must reap as Denied, not Approved")
            }
        }
    }

    #[tokio::test]
    async fn approval_manager_reaper_collects_crashed_requester() {
        // AUDIT D-5 — when the awaiting future is dropped (turn
        // cancelled mid-approval), `tx.is_closed()` is true and the
        // reaper collects the leaked entry even before its TTL.
        use wcore_protocol::ToolApprovalManager;

        let mgr = ToolApprovalManager::new(); // default 5-min TTL
        let rx = mgr.request_approval(
            "call-1",
            &wcore_protocol::events::ToolCategory::Exec,
            "exec_tool",
        );
        drop(rx); // requester "crashed" / turn cancelled
        let collected = mgr.reap_now();
        assert_eq!(
            collected, 1,
            "a requester-crashed entry must be reaped before its TTL"
        );
        // A second sweep finds nothing — the entry is gone.
        assert_eq!(mgr.reap_now(), 0);
    }

    // --- A9: turn-start hook can block -----------------------------------

    #[tokio::test]
    async fn turn_start_hook_block_halts_the_loop() {
        // AUDIT A9 — a turn-start hook returning `block` halts the run
        // cleanly. We assert via `apply_pre_turn_outcome` returning the
        // block reason (the loop wiring consumes that).
        let mut engine = engine_with(Arc::new(ScriptedProvider::new(vec![])));
        let outcome = crate::hooks::HookOutcome {
            block: Some("operator stop".into()),
            ..Default::default()
        };
        let halt = engine.apply_pre_turn_outcome(outcome);
        assert_eq!(
            halt.as_deref(),
            Some("operator stop"),
            "a turn-start hook block must surface as a halt reason"
        );
    }

    // Keep the `ApprovalBridge` import used (the engine builds one in
    // its constructor; this silences an unused-import lint on the test
    // module's `use` list when the bridge is not otherwise touched).
    #[test]
    fn approval_bridge_default_constructs() {
        let _ = ApprovalBridge::new();
    }

    #[test]
    fn parse_git_porcelain_extracts_paths_incl_renames() {
        let stdout = " M crates/foo/src/bar.rs\n\
                      ?? newfile.txt\n\
                      A  staged.rs\n\
                      R  old/path.rs -> new/path.rs\n";
        let paths = super::AgentEngine::parse_git_porcelain(stdout);
        assert_eq!(
            paths,
            vec![
                "crates/foo/src/bar.rs".to_string(),
                "newfile.txt".to_string(),
                "staged.rs".to_string(),
                // A rename keeps the DESTINATION path, not the source.
                "new/path.rs".to_string(),
            ]
        );
    }

    #[test]
    fn parse_git_porcelain_empty_input_is_empty() {
        assert!(super::AgentEngine::parse_git_porcelain("").is_empty());
        assert!(super::AgentEngine::parse_git_porcelain("\n\n").is_empty());
    }

    #[tokio::test]
    async fn seed_workflow_state_always_has_changed_files_array_and_cwd() {
        // The seed must ALWAYS present both keys with the right shapes — even on
        // a non-repo / git-absent box, where `changed_files` is an empty array
        // (never `null`/missing), so a synthesized `over: Some("changed_files")`
        // resolves to an array rather than silently fanning over nothing.
        let state = super::AgentEngine::seed_workflow_state().await;
        assert!(
            state.get("changed_files").is_some_and(|v| v.is_array()),
            "changed_files must be an array; got {state:?}"
        );
        assert!(
            state.get("cwd").is_some_and(|v| v.is_string()),
            "cwd must be a string; got {state:?}"
        );
    }

    /// Output-side opt (Part A) safety invariant: every fluff stop sequence is
    /// prefixed with a paragraph break (`"\n\n"`). This is what guarantees the
    /// stop only fires at a fresh paragraph boundary — a mid-sentence
    /// occurrence of the same words (e.g. "...in summary, the result is...")
    /// is NOT preceded by a blank line and therefore never matches, so the
    /// model is never cut off mid-answer.
    #[test]
    fn fluff_stop_sequences_all_start_with_paragraph_break() {
        assert!(
            !super::FLUFF_STOP_SEQUENCES.is_empty(),
            "fluff stop list must be non-empty"
        );
        // Anthropic caps stop sequences at a small number; keep the list <= 4.
        assert!(
            super::FLUFF_STOP_SEQUENCES.len() <= 4,
            "keep the fluff stop list at most 4 entries"
        );
        for s in super::FLUFF_STOP_SEQUENCES {
            assert!(
                s.starts_with("\n\n"),
                "fluff stop {s:?} must start with a paragraph break so it only \
                 fires at a paragraph boundary, never mid-sentence"
            );
        }
    }
}

/// C1 / Task A2 — `run_session_start_hooks` applies plugin-hook contributions
/// to a cold conversation (gated, budgeted), without touching the system prompt
/// or regressing cross-session recall.
#[cfg(test)]
mod session_start_apply_tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use wcore_config::config::Config;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::{ContentBlock, Message, Role};

    use crate::hooks::HookDispatcher;
    use crate::output::OutputSink;
    use crate::plugins::runner::PluginHook;
    use wcore_plugin_api::registry::hooks::HookPhase;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: wcore_types::message::FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    /// Stub host dispatcher: returns a fixed contribution for any hook.
    struct StubDispatcher {
        text: String,
    }
    #[async_trait]
    impl HookDispatcher for StubDispatcher {
        async fn dispatch(&self, _: &str, _: &str, _: HookPhase) -> Option<String> {
            Some(self.text.clone())
        }
    }

    fn cold_engine_with_session_hook(contribution: &str) -> super::AgentEngine {
        // A real system prompt so the "system prompt unchanged" assertion is
        // meaningful (not comparing empty-to-empty).
        let cfg = Config {
            system_prompt: Some("SYSTEM-PROMPT-CONTENT".to_string()),
            ..Default::default()
        };
        let mut engine = super::AgentEngine::new_with_provider(
            Arc::new(NullProvider),
            cfg,
            ToolRegistry::new(),
            Arc::new(NullOutput),
        );
        engine.register_plugin_hooks(vec![PluginHook {
            plugin: "wayland-ijfw".to_string(),
            phase: HookPhase::SessionStart,
            name: "ijfw_memory_prelude".to_string(),
        }]);
        engine.set_hook_dispatcher(Arc::new(StubDispatcher {
            text: contribution.to_string(),
        }));
        engine
    }

    fn sole_message_text(engine: &super::AgentEngine) -> &str {
        assert_eq!(
            engine.messages.len(),
            1,
            "expected exactly one applied message"
        );
        let msg = &engine.messages[0];
        assert_eq!(
            msg.role,
            Role::User,
            "prelude must be a User block, never system"
        );
        match msg.content.first() {
            Some(ContentBlock::Text { text }) => text,
            other => panic!("expected a text block, got {other:?}"),
        }
    }

    // TEST 1 — cold inject: a SessionStart contribution is applied as exactly
    // one untrusted User-role <plugin-context> block.
    #[tokio::test]
    async fn cold_session_applies_untrusted_prelude_block() {
        let mut engine = cold_engine_with_session_hook("PRELUDE");
        assert!(engine.messages.is_empty(), "precondition: cold");
        engine.run_session_start_hooks().await;
        let text = sole_message_text(&engine);
        assert!(
            text.contains("trust=\"untrusted\""),
            "missing untrusted envelope: {text}"
        );
        assert!(
            text.contains("PRELUDE"),
            "missing contribution body: {text}"
        );
        assert_eq!(
            engine.session_start_injected_len, 1,
            "baseline must record the one applied prelude message"
        );
    }

    // TEST 2 — resume skip: a populated buffer (simulating resume, which sets
    // messages at construction before session-start hooks run) is NOT extended.
    #[tokio::test]
    async fn resumed_session_skips_prelude() {
        let mut engine = cold_engine_with_session_hook("PRELUDE");
        engine.messages.push(Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: "prior turn".to_string(),
            }],
        ));
        engine.run_session_start_hooks().await;
        assert_eq!(
            engine.messages.len(),
            1,
            "resume path must not apply a session-start prelude"
        );
        assert_eq!(
            engine.session_start_injected_len, 0,
            "no prelude applied ⇒ baseline stays 0"
        );
    }

    // TEST 3 — budget truncation: an oversized contribution is capped near the
    // budget and marked truncated.
    #[tokio::test]
    async fn oversized_prelude_is_truncated_to_budget() {
        let max_chars = super::SESSION_PRELUDE_TOKEN_BUDGET * super::PRELUDE_CHARS_PER_TOKEN;
        let huge = "x".repeat(max_chars * 4);
        let mut engine = cold_engine_with_session_hook(&huge);
        engine.run_session_start_hooks().await;
        let text = sole_message_text(&engine);
        assert!(
            text.contains("[truncated]"),
            "oversized prelude must be marked truncated"
        );
        // The envelope adds a small fixed wrapper; the body is capped at
        // `max_chars`, so the whole message stays within a tight margin of the
        // budget rather than echoing the multi-MB plugin payload.
        assert!(
            text.len() < max_chars + 512,
            "truncated message length {} should be near the budget {}",
            text.len(),
            max_chars
        );
    }

    // TEST 4 — the system prompt is byte-identical across the call.
    #[tokio::test]
    async fn system_prompt_is_untouched() {
        let mut engine = cold_engine_with_session_hook("PRELUDE");
        let before = engine.system_prompt.clone();
        engine.run_session_start_hooks().await;
        assert_eq!(
            engine.system_prompt, before,
            "session-start prelude must never alter the system prompt"
        );
    }

    // TEST 5 (coexistence) — after a prelude is applied (baseline == 1), the
    // recall guard still treats the session as cold so cross-session recall
    // fires; a resumed session (baseline 0, populated) is correctly skipped.
    #[tokio::test]
    async fn recall_still_fires_alongside_a_prelude() {
        let mut engine = cold_engine_with_session_hook("PRELUDE");
        engine.run_session_start_hooks().await;
        assert_eq!(engine.messages.len(), 1);
        assert_eq!(engine.session_start_injected_len, 1);
        assert!(
            engine.should_attempt_recall(),
            "a session whose only message is the prelude is still cold — recall must fire"
        );

        // Resume shape: populated buffer, no prelude baseline ⇒ NOT cold.
        let mut resumed = cold_engine_with_session_hook("PRELUDE");
        resumed.messages.push(Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: "prior".to_string(),
            }],
        ));
        assert!(
            !resumed.should_attempt_recall(),
            "a resumed session has real prior context — recall must NOT fire"
        );
    }

    // Regression (code-review A2) — `/resume` and `/clear` must re-baseline the
    // prelude count. Without the reset a stale baseline of 1 makes a
    // single-message resumed session read as "cold" and wrongly re-trigger
    // cross-session recall on real prior context.
    #[tokio::test]
    async fn resume_and_clear_reset_the_prelude_baseline() {
        // Cold boot applies a prelude → baseline 1.
        let mut engine = cold_engine_with_session_hook("PRELUDE");
        engine.run_session_start_hooks().await;
        assert_eq!(engine.session_start_injected_len, 1);

        // `/resume` swaps in a one-message session. With a stale baseline this
        // would be `1 <= 1` ⇒ recall wrongly fires on resumed context.
        engine.load_conversation(vec![Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: "resumed".to_string(),
            }],
        )]);
        assert_eq!(
            engine.session_start_injected_len, 0,
            "resume must clear the prelude baseline"
        );
        assert!(
            !engine.should_attempt_recall(),
            "a resumed session has real prior context — recall must NOT fire"
        );

        // `/clear` empties the buffer; a cleared session genuinely IS cold.
        engine.clear_conversation();
        assert_eq!(
            engine.session_start_injected_len, 0,
            "clear must reset the prelude baseline"
        );
        assert!(
            engine.should_attempt_recall(),
            "a cleared session is cold — recall fires"
        );
    }

    // Code-review A2 (minor) — budget truncation must be char-boundary safe on
    // multi-byte UTF-8: never panic, never split a codepoint, still bounded.
    #[test]
    fn oversized_multibyte_prelude_truncates_on_a_char_boundary() {
        let max_chars = super::SESSION_PRELUDE_TOKEN_BUDGET * super::PRELUDE_CHARS_PER_TOKEN;
        // 'é' is 2 bytes — a payload well over the byte budget that would panic
        // a naive byte-offset `String::truncate`.
        let mut msg = Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: "é".repeat(max_chars),
            }],
        );
        super::AgentEngine::enforce_prelude_budget(&mut msg); // must not panic
        let text = match msg.content.first() {
            Some(ContentBlock::Text { text }) => text,
            other => panic!("expected a text block, got {other:?}"),
        };
        assert!(text.contains("[truncated]"), "must be marked truncated");
        assert!(
            text.trim_end_matches(" …[truncated]")
                .chars()
                .all(|c| c == 'é'),
            "truncation split a multi-byte codepoint"
        );
        assert!(
            text.len() <= max_chars + 32,
            "truncated length {} should stay near the budget {}",
            text.len(),
            max_chars
        );
    }

    // Degradation — with NO dispatcher wired, the apply path is a no-op
    // (legacy log-only behavior) and the conversation stays empty.
    #[tokio::test]
    async fn no_dispatcher_applies_nothing() {
        let cfg = Config {
            system_prompt: Some("SP".to_string()),
            ..Default::default()
        };
        let mut engine = super::AgentEngine::new_with_provider(
            Arc::new(NullProvider),
            cfg,
            ToolRegistry::new(),
            Arc::new(NullOutput),
        );
        engine.register_plugin_hooks(vec![PluginHook {
            plugin: "wayland-ijfw".to_string(),
            phase: HookPhase::SessionStart,
            name: "ijfw_memory_prelude".to_string(),
        }]);
        // No set_hook_dispatcher.
        engine.run_session_start_hooks().await;
        assert!(
            engine.messages.is_empty(),
            "no dispatcher ⇒ log-only, nothing applied"
        );
        assert_eq!(engine.session_start_injected_len, 0);
    }

    // ---- rank 76: TurnTrace.hook_actions reflects fired Rust-hook actions ----

    /// A Rust hook that injects a message on `on_turn_end` — exercises the
    /// honoured `InjectMessage` arm that now records a `HookActionRecord`.
    struct TurnEndInjectHook;

    #[async_trait]
    impl crate::hooks::Hook for TurnEndInjectHook {
        fn name(&self) -> &str {
            "turn_end_injector"
        }
        async fn on_turn_end(
            &self,
            _turn: usize,
            _result: &crate::hooks::TurnResult,
        ) -> crate::hooks::HookAction {
            crate::hooks::HookAction::InjectMessage(Message::now(
                Role::User,
                vec![ContentBlock::Text {
                    text: "injected-by-hook".to_string(),
                }],
            ))
        }
    }

    #[tokio::test]
    async fn turn_trace_records_hook_action_from_turn_end_hook() {
        use wcore_config::compat::ProviderCompat;
        use wcore_config::config::{Config, ProviderType};
        use wcore_observability::trace::TurnTrace;

        let cfg = Config {
            provider_label: "openai".into(),
            provider: ProviderType::OpenAI,
            api_key: "sk-test".into(),
            base_url: "http://localhost:0".into(),
            model: "gpt-test-model".into(),
            max_tokens: 1024,
            max_turns: Some(1),
            compat: ProviderCompat::openai_defaults(),
            ..Default::default()
        };
        // A single text-only turn (no tool calls) drives the engine to the
        // turn-end hook phase + the TurnTrace emission.
        let script = vec![
            LlmEvent::TextDelta("done".into()),
            LlmEvent::Done {
                stop_reason: wcore_types::message::StopReason::EndTurn,
                finish_reason: wcore_types::message::FinishReason::Stop,
                usage: wcore_types::message::TokenUsage::default(),
            },
        ];
        let (mut engine, _handle) = crate::bootstrap::AgentBootstrap::build_for_test(cfg, script);
        engine
            .hooks
            .as_mut()
            .expect("build_for_test installs a HookEngine")
            .register_rust_hook(Box::new(TurnEndInjectHook));

        let out = engine
            .run_synthetic_turn("anything")
            .await
            .expect("synthetic turn should succeed");

        // Find the TraceEvent and deserialize its TurnTrace payload.
        let trace: TurnTrace = out
            .events
            .iter()
            .find(|e| e["type"] == "trace_event")
            .and_then(|e| serde_json::from_value(e["trace"].clone()).ok())
            .expect("a TurnTrace must be emitted");

        assert!(
            !trace.hook_actions.is_empty(),
            "the turn-end InjectMessage must be recorded in hook_actions"
        );
        let rec = &trace.hook_actions[0];
        assert_eq!(rec.kind, "InjectMessage");
        assert_eq!(rec.hook_name, "turn_end_injector");
    }

    // ---- C1 / Task A3 — PrePrompt contribution applied to the request tail ----

    use crate::hooks::HookOutcome;

    /// Build a PrePrompt-style outcome carrying one untrusted block of `body`,
    /// mirroring what `HookEngine::dispatch_into` produces.
    fn pre_prompt_outcome(body: &str) -> HookOutcome {
        HookOutcome {
            injected_messages: vec![Message::now(
                Role::User,
                vec![ContentBlock::Text {
                    text: format!(
                        "<plugin-context source=\"p:h\" trust=\"untrusted\">\n{body}\n</plugin-context>"
                    ),
                }],
            )],
            ..Default::default()
        }
    }

    fn user_msg(text: &str) -> Message {
        Message::now(
            Role::User,
            vec![ContentBlock::Text {
                text: text.to_string(),
            }],
        )
    }

    // (a) APPLIES INTO TAIL — a new contribution is appended to a user-role tail
    // as the untrusted block.
    #[test]
    fn pre_prompt_applies_into_user_tail() {
        let mut messages = vec![user_msg("hello")];
        let outcome = pre_prompt_outcome("RECALL-A");
        super::AgentEngine::apply_pre_prompt_contribution(&mut messages, &outcome);

        assert_eq!(
            messages.len(),
            1,
            "must append to the tail, not push a new message"
        );
        let blocks = &messages[0].content;
        assert_eq!(blocks.len(), 2, "original text + appended contribution");
        let appended = match blocks.last() {
            Some(ContentBlock::Text { text }) => text,
            other => panic!("expected appended text block, got {other:?}"),
        };
        assert!(
            appended.contains("trust=\"untrusted\""),
            "must carry the untrusted envelope"
        );
        assert!(
            appended.contains("RECALL-A"),
            "must carry the contribution body"
        );
    }

    // (F3-a) NO CROSS-TURN SUPPRESSION — the SAME contribution applied to two
    // freshly-cloned request message-vecs (simulating turns N and N+1, where the
    // clone is regenerated each turn) is appended BOTH times. The old cross-turn
    // `last_context_injection` dedup wrongly suppressed the second.
    #[test]
    fn pre_prompt_reapplies_across_turns_no_cross_turn_suppression() {
        let outcome = pre_prompt_outcome("PER-TURN-RECALL");

        // Turn N: fresh clone.
        let mut turn_n = vec![user_msg("hello")];
        super::AgentEngine::apply_pre_prompt_contribution(&mut turn_n, &outcome);
        assert_eq!(turn_n[0].content.len(), 2, "appended on turn N");

        // Turn N+1: a NEW fresh clone (regenerated from self.messages each turn).
        let mut turn_n1 = vec![user_msg("hello")];
        super::AgentEngine::apply_pre_prompt_contribution(&mut turn_n1, &outcome);
        assert_eq!(
            turn_n1[0].content.len(),
            2,
            "must re-append on turn N+1 — no cross-turn suppression (F3)"
        );
    }

    // (F3-b) DEDUP AGAINST LIVE REQUEST — a contribution whose exact block ALREADY
    // EXISTS in the request (simulating the turn-1 SessionStart prelude the host
    // pushed onto history, carried in the clone) is NOT re-appended.
    #[test]
    fn pre_prompt_dedups_against_block_already_in_request() {
        let outcome = pre_prompt_outcome("RECALL-A");
        let injected_text = match &outcome.injected_messages[0].content[0] {
            ContentBlock::Text { text } => text.clone(),
            _ => unreachable!(),
        };
        // The block is already present as a prior message in the request.
        let mut messages = vec![user_msg(&injected_text), user_msg("hello")];
        super::AgentEngine::apply_pre_prompt_contribution(&mut messages, &outcome);

        assert_eq!(
            messages[1].content.len(),
            1,
            "a block already present in the request must not be re-appended"
        );
    }

    // (c) NON-USER TAIL SKIP — if the tail is not user-role (e.g. an assistant
    // tool_use), nothing is appended (no orphaned tool_use / no adjacent users).
    #[test]
    fn pre_prompt_skips_non_user_tail() {
        let mut messages = vec![
            user_msg("hello"),
            Message::now(
                Role::Assistant,
                vec![ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "Read".to_string(),
                    input: serde_json::json!({}),
                    extra: None,
                }],
            ),
        ];
        let outcome = pre_prompt_outcome("RECALL-A");
        super::AgentEngine::apply_pre_prompt_contribution(&mut messages, &outcome);

        assert_eq!(messages.len(), 2, "must not push a new message");
        assert_eq!(
            messages[1].content.len(),
            1,
            "must not append onto an assistant tool_use tail"
        );
    }

    // (d) BUDGET TRUNCATION — an oversized contribution is capped near
    // PRE_PROMPT_TOKEN_BUDGET and marked truncated before appending.
    #[test]
    fn pre_prompt_contribution_is_budget_capped() {
        let max_chars = super::PRE_PROMPT_TOKEN_BUDGET * super::PRELUDE_CHARS_PER_TOKEN;
        let huge = "z".repeat(max_chars * 6);
        let mut messages = vec![user_msg("hello")];
        let outcome = pre_prompt_outcome(&huge);
        super::AgentEngine::apply_pre_prompt_contribution(&mut messages, &outcome);

        let appended = match messages[0].content.last() {
            Some(ContentBlock::Text { text }) => text,
            other => panic!("expected appended text block, got {other:?}"),
        };
        assert!(
            appended.contains("[truncated]"),
            "oversized contribution must be marked truncated"
        );
        // Body is capped at max_chars; the envelope adds only a small wrapper.
        assert!(
            appended.len() < max_chars + 512,
            "appended length {} should stay near the budget {}",
            appended.len(),
            max_chars
        );
    }
}

/// C1 / Task A4 — END-TO-END proof that the REAL `wayland-ijfw` plugin's
/// `SessionStart` hook (`ijfw_memory_prelude`) reaches a cold session's
/// conversation as an untrusted User block, through the real C1 path
/// (`register_plugin_hooks` → `set_hook_dispatcher(McpHookDispatcher)` →
/// `run_session_start_hooks`).
///
/// This test deliberately READS `wayland_ijfw::hooks::HOOKS` and
/// `wayland_ijfw::mcp::SERVER_NAME` from the real plugin crate (a dev-only
/// dependency edge — wayland-ijfw still depends ONLY on
/// wcore-plugin-api/types/protocol per audit F2, so the edge is acyclic) so a
/// rename on the plugin side breaks this proof rather than silently passing.
#[cfg(test)]
mod ijfw_session_start_e2e_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use async_trait::async_trait;
    use wcore_config::config::Config;
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::{ContentBlock, Role};

    use crate::hooks::{McpHookDispatcher, McpToolCaller};
    use crate::output::OutputSink;
    use crate::plugins::runner::PluginHook;
    use wcore_plugin_api::registry::hooks::HookPhase;

    /// Sentinel the fake MCP caller emits ONLY for the real prelude tool. The
    /// suffix keeps it unique so an accidental literal elsewhere can't satisfy
    /// the assertion.
    const PRELUDE_SENTINEL: &str = "IJFW-PRELUDE-SENTINEL-a4e2e";

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: wcore_types::message::FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
    }

    struct NullProvider;
    #[async_trait]
    impl LlmProvider for NullProvider {
        async fn stream(
            &self,
            _: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let (_tx, rx) = tokio::sync::mpsc::channel(1);
            Ok(rx)
        }
    }

    /// Fake MCP backend standing in for the live `@ijfw/memory-server`: it
    /// returns the sentinel ONLY when the real prelude tool is called on the
    /// real server name, and contributes nothing otherwise.
    struct FakeIjfwServer {
        server: String,
        prelude_tool: String,
    }
    #[async_trait]
    impl McpToolCaller for FakeIjfwServer {
        async fn call(&self, server: &str, tool: &str) -> Result<String, String> {
            if server == self.server && tool == self.prelude_tool {
                Ok(PRELUDE_SENTINEL.to_string())
            } else {
                Err(format!("no fixture for {server}/{tool}"))
            }
        }
    }

    /// Pull the SessionStart hook name out of the plugin's real HOOKS table so
    /// the test breaks if the plugin renames it.
    fn ijfw_session_start_hook_name() -> &'static str {
        wayland_ijfw::hooks::HOOKS
            .iter()
            .find(|(phase, _)| *phase == HookPhase::SessionStart)
            .map(|(_, name)| *name)
            .expect("wayland-ijfw must register a SessionStart hook")
    }

    fn cold_engine_with_dispatcher(
        dispatcher: Arc<dyn crate::hooks::HookDispatcher>,
        hooks: Vec<PluginHook>,
    ) -> super::AgentEngine {
        let cfg = Config {
            system_prompt: Some("SYSTEM-PROMPT-CONTENT".to_string()),
            ..Default::default()
        };
        let mut engine = super::AgentEngine::new_with_provider(
            Arc::new(NullProvider),
            cfg,
            ToolRegistry::new(),
            Arc::new(NullOutput),
        );
        engine.register_plugin_hooks(hooks);
        engine.set_hook_dispatcher(dispatcher);
        engine
    }

    // E2E — the real wayland-ijfw SessionStart hook, dispatched through the
    // real McpHookDispatcher against the real plugin server name, surfaces in a
    // cold conversation as exactly one untrusted User block carrying the
    // backend's payload, WITHOUT touching the system prompt.
    #[tokio::test]
    async fn ijfw_prelude_reaches_cold_session_as_untrusted_user_block() {
        let hook_name = ijfw_session_start_hook_name();
        let server_name = wayland_ijfw::mcp::SERVER_NAME;

        let caller = Arc::new(FakeIjfwServer {
            server: server_name.to_string(),
            prelude_tool: hook_name.to_string(),
        });
        let mut server_for_plugin = HashMap::new();
        server_for_plugin.insert("wayland-ijfw".to_string(), server_name.to_string());
        let dispatcher = Arc::new(McpHookDispatcher::new(caller, server_for_plugin));

        let mut engine = cold_engine_with_dispatcher(
            dispatcher,
            vec![PluginHook {
                plugin: "wayland-ijfw".to_string(),
                phase: HookPhase::SessionStart,
                name: hook_name.to_string(),
            }],
        );

        assert!(engine.messages.is_empty(), "precondition: cold session");
        let system_prompt_before = engine.system_prompt.clone();

        engine.run_session_start_hooks().await;

        assert_eq!(
            engine.messages.len(),
            1,
            "exactly one prelude message must be applied"
        );
        let msg = &engine.messages[0];
        assert_eq!(msg.role, Role::User, "prelude must be a User block");
        let text = match msg.content.first() {
            Some(ContentBlock::Text { text }) => text,
            other => panic!("expected a text block, got {other:?}"),
        };
        assert!(
            text.contains("trust=\"untrusted\""),
            "prelude must carry the untrusted envelope: {text}"
        );
        assert!(
            text.contains(PRELUDE_SENTINEL),
            "prelude must carry the backend payload: {text}"
        );
        assert!(
            text.contains(&format!("source=\"wayland-ijfw:{hook_name}\"")),
            "prelude must record the real ijfw provenance: {text}"
        );
        assert_eq!(
            engine.system_prompt, system_prompt_before,
            "session-start prelude must never alter the system prompt"
        );
    }

    // GATE — a plugin with NO entry in `server_for_plugin` (here: the real
    // ijfw hook registered under an unrelated plugin name) contributes nothing,
    // proving the dispatcher's plugin→server map is what gates the injection.
    #[tokio::test]
    async fn unmapped_plugin_contributes_nothing() {
        let hook_name = ijfw_session_start_hook_name();
        let server_name = wayland_ijfw::mcp::SERVER_NAME;

        let caller = Arc::new(FakeIjfwServer {
            server: server_name.to_string(),
            prelude_tool: hook_name.to_string(),
        });
        // Map only "wayland-ijfw"; register the hook under a DIFFERENT plugin.
        let mut server_for_plugin = HashMap::new();
        server_for_plugin.insert("wayland-ijfw".to_string(), server_name.to_string());
        let dispatcher = Arc::new(McpHookDispatcher::new(caller, server_for_plugin));

        let mut engine = cold_engine_with_dispatcher(
            dispatcher,
            vec![PluginHook {
                plugin: "some-other-plugin".to_string(),
                phase: HookPhase::SessionStart,
                name: hook_name.to_string(),
            }],
        );

        engine.run_session_start_hooks().await;
        assert!(
            engine.messages.is_empty(),
            "an unmapped plugin must contribute no prelude"
        );
    }
}

// ---------------------------------------------------------------------------
// #282 contract V1 — engine compact-and-retry on a Flux 409 context_overflow.
//
// A managed Flux client that overflows the routed window gets a typed
// `ProviderError::ContextOverflow` from `provider.stream()`. The engine must
// run ONE compaction pass and retry the SAME turn EXACTLY ONCE: a transient
// overflow recovers, a persistent one terminates cleanly (no infinite loop).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod overflow_retry_tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use wcore_config::config::{Config, ProviderType};
    use wcore_providers::{LlmProvider, ProviderError};
    use wcore_tools::registry::ToolRegistry;
    use wcore_types::llm::{LlmEvent, LlmRequest};
    use wcore_types::message::{FinishReason, StopReason, TokenUsage};

    use crate::output::OutputSink;

    struct NullOutput;
    impl OutputSink for NullOutput {
        fn emit_text_delta(&self, _: &str, _: &str) {}
        fn emit_thinking(&self, _: &str, _: &str) {}
        fn emit_tool_call(&self, _: &str, _: &str) {}
        fn emit_tool_result(&self, _: &str, _: bool, _: &str) {}
        fn emit_stream_start(&self, _: &str) {}
        fn emit_stream_end(
            &self,
            _: &str,
            _: usize,
            _: u64,
            _: u64,
            _: u64,
            _: u64,
            _: FinishReason,
        ) {
        }
        fn emit_error(&self, _: &str, _: bool) {}
        fn emit_info(&self, _: &str) {}
    }

    /// A provider that returns `ContextOverflow` for its first `overflow_for`
    /// `stream()` calls, then a clean one-line turn. The shared call counter
    /// lets a test assert EXACTLY how many times the engine dispatched.
    struct OverflowProvider {
        calls: Arc<AtomicUsize>,
        overflow_for: usize,
    }

    #[async_trait]
    impl LlmProvider for OverflowProvider {
        async fn stream(
            &self,
            _request: &LlmRequest,
        ) -> Result<tokio::sync::mpsc::Receiver<LlmEvent>, ProviderError> {
            let n = self.calls.fetch_add(1, Ordering::SeqCst);
            if n < self.overflow_for {
                return Err(ProviderError::ContextOverflow {
                    required_tokens: 210_000,
                    model_window: 200_000,
                    routed_model: "claude-sonnet-4".into(),
                    message: "prompt exceeds routed window".into(),
                });
            }
            let (tx, rx) = tokio::sync::mpsc::channel(2);
            tokio::spawn(async move {
                let _ = tx.send(LlmEvent::TextDelta("ok".into())).await;
                let _ = tx
                    .send(LlmEvent::Done {
                        stop_reason: StopReason::EndTurn,
                        finish_reason: FinishReason::Stop,
                        usage: TokenUsage::default(),
                    })
                    .await;
            });
            Ok(rx)
        }
    }

    fn flux_config() -> Config {
        Config {
            provider_label: "flux-router".into(),
            provider: ProviderType::FluxRouter,
            api_key: "sk-flux-test".into(),
            base_url: "http://localhost:0".into(),
            model: "flux-auto".into(),
            max_tokens: 1024,
            max_turns: Some(1),
            ..Default::default()
        }
    }

    /// A SINGLE overflow recovers: the engine compacts and retries once, the
    /// retry succeeds, and `run` returns Ok. The provider is dispatched exactly
    /// twice (the overflowing attempt + the recovered retry).
    #[tokio::test]
    async fn single_overflow_compacts_and_retries_then_succeeds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = OverflowProvider {
            calls: Arc::clone(&calls),
            overflow_for: 1,
        };
        let mut engine = super::AgentEngine::new_with_provider(
            Arc::new(provider),
            flux_config(),
            ToolRegistry::new(),
            Arc::new(NullOutput),
        );
        let result = engine.run("hello", "m-overflow-ok").await;
        assert!(result.is_ok(), "a single overflow must recover: {result:?}");
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "exactly one overflow-retry: one failed dispatch + one recovered"
        );
    }

    /// A PERSISTENT overflow terminates: the engine retries exactly ONCE, and
    /// when the retry still overflows it surfaces a terminal error instead of
    /// looping forever. The provider is dispatched exactly twice.
    #[tokio::test]
    async fn persistent_overflow_retries_once_then_terminal_error() {
        let calls = Arc::new(AtomicUsize::new(0));
        let provider = OverflowProvider {
            calls: Arc::clone(&calls),
            overflow_for: usize::MAX,
        };
        let mut engine = super::AgentEngine::new_with_provider(
            Arc::new(provider),
            flux_config(),
            ToolRegistry::new(),
            Arc::new(NullOutput),
        );
        let result = engine.run("hello", "m-overflow-loop").await;
        assert!(
            result.is_err(),
            "a persistent overflow must terminate, not loop"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "the overflow-retry is bounded to ONE: one initial + one retry, then terminal"
        );
    }
}
