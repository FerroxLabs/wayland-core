# DEFECT CLUSTER 5 — hydrated MCP tools are never callable (tvcontrol, 101 tools, RC wayland-core 0.12.25)

**Confidence (self-reported):** probable

## Root cause

The admission half of the design is correct — I traced curation → cap → deferral and could not break it. The break is on the RECORDING half, and it is an interface mismatch nobody's test covers. `ToolSearchTool` returns a pretty-printed JSON array; `AgentEngine::record_hydrated_tools` (engine.rs:15327) parses that array with a single `serde_json::from_str` and `else { return; }`. But it is NOT handed the tool's return value. In production the body first goes through the orchestration result pipeline at `orchestration/mod.rs:2416-2426`: `redact -> truncate_result(&error_content, tool.max_result_size()) -> compact_output -> redact`. `ToolSearchTool` does not override `max_result_size()`, so it inherits the trait default of 50_000 (wcore-tools/src/lib.rs:488), and `truncate_result` (orchestration/mod.rs:3380) does not shorten from the end — it splices `\n\n... [truncated N chars] ...\n\n` into the MIDDLE of the payload. Any ToolSearch answer over 50 kB therefore arrives at the recorder as invalid JSON, `from_str` returns `Err`, the function returns early, `hydrated_tool_names` stays empty, and there is NO log line of any kind. On the next turn `apply_mcp_curation`/`apply_provider_tool_cap`/`apply_tool_deferral` see an empty hydrated set, `apply_cold_deferral` re-marks the tool deferred, `fold_deferred_into_catalog` removes it from `tools[]`, the model cannot call it, searches again, and the no-progress guard kills the run at 10 — exactly the observed failure. 101 tvcontrol tools with real descriptions and schemas put a broad query (`tv`, `chart`, `symbol`, `tv_tool` — every tool name starts with `tv_`, and ToolSearch matches on name OR description substring) far over 50 kB. SEPARATELY, and this is what made Sean's config probes uninformative: `[builtin_tools.defer_cold] enabled = false` is NOT honoured — `ConfigFile` (config.rs:373) has no `builtin_tools` field at all, and `Config::resolve` hardcodes `builtin_tools: BuiltinToolsConfig::default()` (config.rs:2594), so serde silently drops the whole table. `[mcp.servers.X] deferred = false` IS parsed and IS honoured into `McpToolProxy::is_deferred()` (tool_proxy.rs:235), but `apply_tool_deferral` unconditionally calls `apply_cold_deferral` which re-marks every tool not on the 8-name hot allowlist as deferred, so it is honoured-then-overridden. `[mcp.curation] kind = "off"` is honoured. That separates "config ignored" (one knob, provably) from "hydration broken" (the real defect).

## Evidence

- crates/wcore-agent/src/engine.rs:15327-15338 — the whole recorder: `let Ok(serde_json::Value::Array(matches)) = serde_json::from_str::<serde_json::Value>(content) else { return; };` — one strict parse, silent early return, no log, no fallback.
- crates/wcore-agent/src/orchestration/mod.rs:2419-2426 — the pipeline the body actually travels: `let redacted_content = crate::output_redaction::redact_tool_output(&r.content); ... let content = truncate_result(&error_content, max_size); let content = wcore_compact::compact_output(&content, compaction_level); let content = if toon_enabled { ... };` (max_size from `tool.max_result_size()` at mod.rs:1712).
- crates/wcore-agent/src/orchestration/mod.rs:3380-3405 — `truncate_result` splices, it does not tail-trim: `format!("{}\n\n... [truncated {} chars] ...\n\n{}", head, content.len() - max_chars, tail)`. Applied to a JSON array this yields un-parseable JSON.
- crates/wcore-tools/src/lib.rs:488-490 — `fn max_result_size(&self) -> usize { 50_000 }` is the trait default; `rtk proxy grep -rn 'max_result_size' crates/wcore-tools/src/` shows grep/edit/pdf/read/write/jsonl/tts/doc/kubectl/image/video override it and tool_search.rs does NOT.
- crates/wcore-agent/src/engine.rs:16746-16761 — `hydrate_via_tool_search`, the ONLY end-to-end hydration test helper, bypasses the pipeline: `let result = search.execute(...).await; ... engine.record_hydrated_tools(&result.content);`. The defect is invisible to the suite by construction. `rtk proxy grep -rln 'hydrat' crates/wcore-agent/tests/` returns nothing — there is no integration coverage at all.
- crates/wcore-tools/src/tool_search.rs:117-120 — output is `serde_json::to_string_pretty(&matches)` with no size bound of any kind; every match carries `name` + full `description` + full `input_schema`.
- crates/wcore-config/src/config.rs:373-470 — `pub struct ConfigFile` field list: default, security, execution, providers, profiles, tools, session, inbound_webhook, compact, plan, file_cache, hooks, bedrock, vertex, mcp, debug, observability, provider_chain, provider_policy, budget, storage, memory, browser… there is NO `builtin_tools`.
- crates/wcore-config/src/config.rs:2594 — inside `Config::resolve`: `builtin_tools: crate::tools::BuiltinToolsConfig::default(),` — hardcoded. `rtk proxy grep -rn 'builtin_tools' crates/wcore-config/src/` returns only 5 hits: the field decl, the Debug impl, a doc comment, and two `::default()` constructions. Nothing ever reads it from TOML.
- crates/wcore-mcp/src/tool_proxy.rs:234-237 — `let deferred = server_configs.get(*server_name).and_then(|c| c.deferred).unwrap_or(true);` — the per-server knob IS read; crates/wcore-tools/src/registry.rs:408-417 `apply_cold_deferral` then sets `def.deferred = true` for everything off the hot allowlist, which engine.rs:15446 calls unconditionally. Honoured, then overridden.
- crates/wcore-config/src/tools.rs:59-90 — `DeferColdConfig::default()` = `enabled: true, hot_allowlist: [Read, Edit, Write, Bash, Grep, Glob, ToolSearch, Forge], catalog: true` — byte-for-byte the 8 names the live model enumerated, confirming the resolved config was the hardcoded default.
- git: `git log -S record_hydrated_tools` → 38736654; `git merge-base --is-ancestor 38736654 9007c2c6` → true, so the hydration code IS in the RC binary Sean ran.
- tvcontrol scale check (gh api repos/FerroxLabs/tvcontrol/git/trees/HEAD): src/tools/*.js total ~50 kB of source for 101 tools; sampled src/tools/health.js shows ~120-char descriptions plus small schemas. Pretty-printed as ToolSearch matches that is roughly 600-900 bytes per tool, i.e. 60-90 kB for a query matching all 101 — over the 50 kB cut. THIS IS AN ESTIMATE, not a measurement.

## How to verify

PATCH APPLIES CLEANLY — verified with `git apply --check --verbose` against integration head 7accc0c1 in the read-only worktree (all three files, no offsets, no --recount needed). Nothing else was run: no cargo on the Mac, and I was not authorised to build on hetzner.

Build/test (hetzner `/root/orch-gate`, `export PATH=$HOME/.cargo/bin:$PATH`):
  cargo nextest run -p wcore-tools tool_search
  cargo nextest run -p wcore-agent --test tool_search_hydration_e2e
  cargo nextest run -p wcore-agent --lib hydrat
  cargo clippy --workspace --all-targets   # CI denies warnings
  cargo fmt --all --check                  # cargo fmt is safe on the Mac

The distinguishing observable, in one line: in
`tool_hydrated_through_the_real_result_pipeline_is_declared_next_turn`, the SECOND captured
LlmRequest's `tools` array either contains `tv_tool_000` with `deferred == false` (fixed) or does
not contain it at all (broken). The companion test asserts the ToolSearch body reaching the
provider contains no `[truncated` marker.

LIVE re-verification, and the one measurement that would upgrade this from "probable" to
"confident": re-run the tvcontrol session and grep the ToolSearch tool_result the model received
for the literal string `... [truncated`. Present => this root cause is confirmed outright.
Absent => the truncation branch did NOT fire in Sean's run, the patch is still correct but
incomplete, and the next step is to log `content.len()` and the `serde_json::from_str` error at
engine.rs:15327 (the new tier-3 `tracing::warn!` does exactly this) and re-run. Also drive the
Anthropic path — it was never exercised and has no 128-tool provider cap, so it may behave
differently.

CONFIG SEPARATION (this half is proven from source, no run needed):
  [builtin_tools.defer_cold] enabled=false  -> NOT HONOURED. `ConfigFile` has no `builtin_tools`
      field (config.rs:373-470) and `Config::resolve` hardcodes the default (config.rs:2594);
      serde silently discards the table. Same for [builtin_tools.script] and
      [builtin_tools.repomap]. Deliberately NOT fixed in this patch — it is a separate defect
      needing a ConfigFile field + a merge rule, and I would not ship an unbuilt config-merge
      change alongside the hydration fix. File it; it is the operator's only escape hatch.
  [mcp.servers.X] deferred=false            -> HONOURED (tool_proxy.rs:235) then unconditionally
      OVERRIDDEN by apply_cold_deferral (engine.rs:15446 -> registry.rs:408). "No observable
      change" is the correct behaviour of the current code, not a config bug.
  [mcp.curation] kind="off"                 -> HONOURED (config.rs:244 -> engine.rs:3251).

## Mutant

Three independent mutants, each stated on the test that catches it.

1. `tool_hydrated_through_the_real_result_pipeline_is_declared_next_turn` (the product gate).
   Revert `record_hydrated_tools` to the single strict parse:
       let Ok(serde_json::Value::Array(matches)) = serde_json::from_str(content) else { return; };
   OR delete the `if !matches.is_empty() && used_bytes + cost > MAX_MATCH_BYTES` guard in
   tool_search.rs. OR delete `ToolSearchTool::max_result_size`. Any ONE restores the >50 kB
   splice-into-invalid-JSON path, `hydrated_tool_names` stays empty, and the test panics with
   "hydrated tool absent from the next turn's tools[]" listing the declared names.

2. `truncated_tool_search_body_still_hydrates_only_real_tools` (engine unit).
   Delete the tier-3 branch. First assertion fails: `mcp__srv__alpha` is not hydrated.
   Inverse mutant for the security half: drop the `registry_checked` filter in
   `push_hydrated_from_matches` and the second assertion fails — `mcp__srv__ghost`, a name lifted
   from untrusted text, gets hydrated.

3. `wide_match_set_stays_inside_the_declared_result_size` (tool unit).
   Delete the byte-budget guard: 200 fat matches serialize to ~180 kB, far past
   `max_result_size() == 64_000`, and the length assertion fails with both numbers printed.

Non-vacuity guard inside gate 1: it first asserts `tv_tool_000` is ABSENT from turn 1's tools[].
If catalog folding ever stopped hiding MCP tools, the turn-2 assertion would pass for free — that
baseline assertion fails instead, so the gate cannot silently become tautological.

## Unknowns

- NOT MEASURED: whether Sean's specific run actually crossed the 50 kB truncation threshold. My size estimate for tvcontrol (60-90 kB for a broad query) comes from `gh api` file sizes for src/tools/*.js plus one sampled file — it is arithmetic on a proxy, not the real tools/list payload. If the model queried an exact name like `tv_health_check` the result would be ~1 kB and would NOT truncate, and then this root cause is wrong. The `[truncated` grep in how_to_verify settles it in one command. I did not run the binary and I did not observe the fix working.
- I could not build, test, lint or format the patch. It is verified only to APPLY cleanly. Compile risks I flagged and mitigated but cannot confirm: the tier-2 let-chain (`if let (Some,Some) = ... && start < end && let Ok(...)`) needs edition-2024 let-chains, which the file already uses elsewhere; clippy::collapsible_if was the reason I wrote it as a chain rather than nested ifs, and I cannot confirm which form clippy prefers here.
- The 2x margin between MAX_MATCH_BYTES (32_000) and max_result_size() (64_000) is reasoned, not measured. It assumes `to_string_pretty` of a standalone entry under-counts the in-array form by at most 2 spaces per line. A pathological MCP schema (very many very short lines) could in principle eat that margin; it cannot exceed 2x for any realistic schema, but I did not compute a worst case.
- Anthropic path entirely untested, by Sean and by me. It has no 128-tool provider cap, so `apply_provider_tool_cap` early-returns and `prune_stale_hydrated_tools` never runs on that path. The hydration recording defect is provider-independent, but I cannot rule out a second, Anthropic-only failure.
- I did not fix the `[builtin_tools]` config gap, only proved it. Wiring `builtin_tools` into ConfigFile + merge_config_files needs a merge-precedence decision (global vs project) and a build, and I would not ship an unbuilt config-merge change in the same patch as the hydration fix.
- Not investigated: whether `mcp_curation_cache`'s append-only union is ever truncated to top_k. It grows monotonically with no cap (unlike mcp_cap_cache). Not implicated in this defect, but it is unbounded growth in a long session with a large MCP inventory.
- `wcore-compact`'s CompactionLevel::Full path (`fold_repeated_lines` + `compact_json`) would ALSO mangle a pretty-printed JSON array. Default is Safe so it does not fire out of the box, but anyone who sets `[compact] compaction = "full"` re-enters the same class of failure. The tier-2/tier-3 recovery covers it; I did not test that combination.

## Proposed patch (NOT APPLIED, NOT COMPILED)

```diff
diff --git a/crates/wcore-agent/src/engine.rs b/crates/wcore-agent/src/engine.rs
index d2c8a97..cf99ae2 100644
--- a/crates/wcore-agent/src/engine.rs
+++ b/crates/wcore-agent/src/engine.rs
@@ -15325,13 +15325,102 @@ impl AgentEngine {
     /// callable. A no-match result is a plain string, not JSON — it parses
     /// to nothing and records nothing.
     fn record_hydrated_tools(&mut self, content: &str) {
-        let Ok(serde_json::Value::Array(matches)) =
+        // Tier 1 — the exact ToolSearch contract: the whole body is the JSON
+        // array of matches.
+        if let Ok(serde_json::Value::Array(matches)) =
             serde_json::from_str::<serde_json::Value>(content)
-        else {
+        {
+            self.push_hydrated_from_matches(matches, false);
             return;
-        };
+        }
+
+        // Tier 2 — array plus a trailing note. ToolSearch appends one when it
+        // had to omit matches to stay inside its byte budget; re-parse the
+        // outermost `[` .. `]` slice, which is still exactly the contract.
+        if let (Some(start), Some(end)) = (content.find('['), content.rfind(']'))
+            && start < end
+            && let Ok(serde_json::Value::Array(matches)) =
+                serde_json::from_str::<serde_json::Value>(&content[start..=end])
+        {
+            self.push_hydrated_from_matches(matches, false);
+            return;
+        }
+
+        // Tier 3 — the body no longer parses at all, because something
+        // outside this tool rewrote it (historically: the orchestration
+        // truncator splicing `... [truncated N chars] ...` into the middle of
+        // a >50 kB result from an MCP server with ~100 tools). Dropping the
+        // hydration here is what made every such tool permanently uncallable,
+        // and it did so with no log line at all. Recover what the body still
+        // carries, keep ONLY names that are real tools in the LIVE registry —
+        // a name lifted out of a mangled body is untrusted text — and say so.
+        let recovered = Self::recover_hydrated_names(content);
+        if recovered.is_empty() {
+            return;
+        }
+        tracing::warn!(
+            target: "wcore_agent::engine",
+            "ToolSearch result ({} bytes) did not parse as JSON; recovered {} candidate tool \
+             name(s) by scan so the hydration is not silently lost",
+            content.len(),
+            recovered.len()
+        );
+        self.push_hydrated_from_matches(
+            recovered
+                .into_iter()
+                .map(|n| serde_json::json!({ "name": n }))
+                .collect(),
+            true,
+        );
+    }
+
+    /// Last-resort recovery of `"name": "<value>"` pairs from a ToolSearch
+    /// body that no longer parses as JSON. Deliberately dumb, allocation- and
+    /// count-bounded ([`HYDRATED_TOOLS_CAP`]); every candidate it returns is
+    /// validated against the LIVE tool registry by the caller before it is
+    /// trusted, so a name embedded in some other tool's description text can
+    /// never inject a hydration.
+    fn recover_hydrated_names(content: &str) -> Vec<String> {
+        const KEY: &str = "\"name\":";
+        let mut out: Vec<String> = Vec::new();
+        let mut rest = content;
+        while let Some(pos) = rest.find(KEY) {
+            rest = &rest[pos + KEY.len()..];
+            let after = rest.trim_start();
+            let Some(body) = after.strip_prefix('"') else {
+                continue;
+            };
+            let Some(end) = body.find('"') else {
+                break;
+            };
+            let candidate = &body[..end];
+            if !candidate.is_empty() && !out.iter().any(|n| n == candidate) {
+                out.push(candidate.to_string());
+            }
+            if out.len() >= HYDRATED_TOOLS_CAP {
+                break;
+            }
+        }
+        out
+    }
+
+    /// Shared tail of [`Self::record_hydrated_tools`]. `registry_checked`
+    /// restricts admission to names that exist in the LIVE tool registry;
+    /// it is set only for the untrusted tier-3 recovery scan, so the
+    /// well-formed tiers keep their existing (snapshot-tolerant) behaviour
+    /// and `prune_stale_hydrated_tools` stays the thing that removes a
+    /// hydration whose MCP server has since disconnected.
+    fn push_hydrated_from_matches(
+        &mut self,
+        matches: Vec<serde_json::Value>,
+        registry_checked: bool,
+    ) {
+        let registry = Arc::clone(&self.tools);
         for m in matches {
             if let Some(name) = m.get("name").and_then(|v| v.as_str()) {
+                if registry_checked && registry.get(name).is_none() {
+                    continue;
+                }
                 self.push_hydrated_name(name);
             }
         }
@@ -17185,6 +17274,50 @@ mod set_config_tests {
         assert_eq!(kept.len(), 3, "cap still enforced");
     }
 
+    /// Cluster-5 regression (tier-3 recovery): production does NOT hand this
+    /// recorder the ToolSearch tool's return value — it hands it the value
+    /// after `truncate_result` / compaction / redaction. A body the truncator
+    /// spliced is no longer JSON, and the old recorder returned early and
+    /// recorded NOTHING, with no log line, which left the hydrated tool
+    /// permanently undeclared. Recovery must still hydrate, and must admit
+    /// ONLY names that are real tools in the live registry.
+    ///
+    /// Mutant: delete the tier-3 branch in `record_hydrated_tools` (restore
+    /// the bare `else { return; }`) and the first assertion fails.
+    #[test]
+    fn truncated_tool_search_body_still_hydrates_only_real_tools() {
+        let mut engine = make_engine("m");
+        engine.tools = Arc::new(hydration_registry(&["mcp__srv__alpha"]));
+        let cut = concat!(
+            "[\n  {\n    \"name\": \"mcp__srv__alpha\",\n",
+            "    \"description\": \"a\"\n\n",
+            "... [truncated 40000 chars] ...\n\n",
+            "\"name\": \"mcp__srv__ghost\", \"parameters\": {}"
+        );
+        assert!(
+            serde_json::from_str::<serde_json::Value>(cut).is_err(),
+            "the fixture must genuinely be invalid JSON"
+        );
+
+        engine.record_hydrated_tools(cut);
+
+        assert!(
+            engine
+                .hydrated_tool_names
+                .iter()
+                .any(|n| n == "mcp__srv__alpha"),
+            "a real tool named in a mangled body must still hydrate, got {:?}",
+            engine.hydrated_tool_names
+        );
+        assert!(
+            !engine
+                .hydrated_tool_names
+                .iter()
+                .any(|n| n == "mcp__srv__ghost"),
+            "a name recovered from untrusted text must be registry-checked"
+        );
+    }
+
     /// #359 regression — a BARE-named MCP tool is subject to curation. The
     /// curator must rank/trim it like a prefixed MCP tool; a built-in with the
     /// same snake_case shape (server: None) must never be curated away.
diff --git a/crates/wcore-agent/tests/tool_search_hydration_e2e.rs b/crates/wcore-agent/tests/tool_search_hydration_e2e.rs
new file mode 100644
index 0000000..b865085
--- /dev/null
+++ b/crates/wcore-agent/tests/tool_search_hydration_e2e.rs
@@ -0,0 +1,264 @@
+//! Cluster-5 regression: a tool the model HYDRATES through `ToolSearch` must
+//! be genuinely callable on the next turn — declared in the outbound
+//! `tools[]` with `deferred == false`.
+//!
+//! Why this lives here and not next to the other hydration tests: the
+//! in-crate unit tests feed `ToolSearchTool`'s return value STRAIGHT into
+//! `AgentEngine::record_hydrated_tools`. Production does not. Production
+//! routes the body through the orchestration result pipeline
+//! (`redact -> truncate_result(max_result_size) -> compact -> redact`), and
+//! `truncate_result` splices `... [truncated N chars] ...` into the MIDDLE of
+//! anything over the tool's `max_result_size`. That splice makes the body
+//! un-parseable, `record_hydrated_tools` returned early, and NOTHING was
+//! recorded — with no log line. Measured live against a 101-tool MCP server
+//! (tvcontrol, 2026-08-04): the model searched, got the schema, the tool was
+//! still absent from `tools[]`, and the run died in the no-progress guard.
+//!
+//! This test drives a REAL `AgentEngine` turn over a scripted provider, so it
+//! exercises that whole pipeline. No live LLM.
+
+mod common;
+
+use std::sync::{Arc, Mutex};
+
+use async_trait::async_trait;
+use serde_json::{Value, json};
+use tokio::sync::mpsc;
+
+use common::{MockLlmProvider, test_config};
+use wcore_agent::engine::AgentEngine;
+use wcore_agent::output::OutputSink;
+use wcore_agent::output::null_sink::NullSink;
+use wcore_protocol::events::ToolCategory;
+use wcore_providers::{LlmProvider, ProviderError};
+use wcore_tools::Tool;
+use wcore_tools::registry::ToolRegistry;
+use wcore_types::llm::{LlmEvent, LlmRequest};
+use wcore_types::message::{FinishReason, StopReason, TokenUsage};
+use wcore_types::tool::ToolResult;
+
+/// Number of MCP-provenance fixture tools. Sized with `DESC_BYTES` so the
+/// unbounded ToolSearch answer to a query matching all of them lands well
+/// over the 50 kB default `max_result_size` — i.e. inside the regime the
+/// live tvcontrol run hit.
+const MCP_TOOL_COUNT: usize = 64;
+const DESC_BYTES: usize = 700;
+
+/// A deferred tool with real MCP provenance — the shape `McpToolProxy`
+/// presents to the registry (`is_deferred() == true`, `mcp_server() == Some`).
+struct FixtureMcpTool {
+    name: String,
+    description: String,
+}
+
+#[async_trait]
+impl Tool for FixtureMcpTool {
+    fn name(&self) -> &str {
+        &self.name
+    }
+    fn description(&self) -> &str {
+        &self.description
+    }
+    fn input_schema(&self) -> Value {
+        json!({
+            "type": "object",
+            "properties": {"symbol": {"type": "string"}}
+        })
+    }
+    fn is_concurrency_safe(&self, _input: &Value) -> bool {
+        false
+    }
+    fn is_deferred(&self) -> bool {
+        true
+    }
+    fn mcp_server(&self) -> Option<&str> {
+        Some("tvfixture")
+    }
+    async fn execute(&self, _input: Value) -> ToolResult {
+        ToolResult {
+            content: "ok".to_string(),
+            is_error: false,
+        }
+    }
+    fn category(&self) -> ToolCategory {
+        ToolCategory::Info
+    }
+}
+
+struct CapturingProvider {
+    inner: MockLlmProvider,
+    captured: Arc<Mutex<Vec<LlmRequest>>>,
+}
+
+#[async_trait]
+impl LlmProvider for CapturingProvider {
+    async fn stream(
+        &self,
+        request: &LlmRequest,
+    ) -> Result<mpsc::Receiver<LlmEvent>, ProviderError> {
+        self.captured.lock().unwrap().push(request.clone());
+        self.inner.stream(request).await
+    }
+}
+
+fn done(stop: StopReason) -> LlmEvent {
+    LlmEvent::Done {
+        stop_reason: stop,
+        finish_reason: FinishReason::from_stop_reason(stop),
+        usage: TokenUsage::default(),
+    }
+}
+
+fn mcp_registry() -> ToolRegistry {
+    let mut registry = ToolRegistry::new();
+    let filler = "d".repeat(DESC_BYTES);
+    for i in 0..MCP_TOOL_COUNT {
+        registry.register(Box::new(FixtureMcpTool {
+            name: format!("tv_tool_{i:03}"),
+            description: filler.clone(),
+        }));
+    }
+    // Install the REAL ToolSearch over a real bootstrap-style snapshot of
+    // this registry — the same seam bootstrap and `/mcp add` use.
+    registry.refresh_tool_search_catalog(&wcore_config::tools::DeferColdConfig::default());
+    registry
+}
+
+/// THE GATE. Turn 1: the model runs a broad `ToolSearch`. Turn 2: the tool it
+/// learned must be DECLARED, with its full schema, in the outbound `tools[]`.
+///
+/// Fails (pre-fix) because the >50 kB ToolSearch body is truncated by the
+/// orchestration pipeline into invalid JSON, `record_hydrated_tools` bails,
+/// and `tv_tool_000` never reaches `tools[]`.
+///
+/// MUTANT (must make this fail again):
+///   - revert `record_hydrated_tools` to the single strict
+///     `serde_json::from_str` + `else { return; }`, **or**
+///   - delete the `MAX_MATCH_BYTES` guard in
+///     `wcore-tools/src/tool_search.rs::execute_with_ctx`, **or**
+///   - drop `ToolSearchTool::max_result_size`.
+/// Each alone restores the silent-loss path and this assertion trips.
+#[tokio::test]
+async fn tool_hydrated_through_the_real_result_pipeline_is_declared_next_turn() {
+    let captured: Arc<Mutex<Vec<LlmRequest>>> = Arc::new(Mutex::new(Vec::new()));
+    let provider = CapturingProvider {
+        inner: MockLlmProvider::with_turns(vec![
+            vec![
+                LlmEvent::ToolUse {
+                    id: "ts-1".to_string(),
+                    name: "ToolSearch".to_string(),
+                    input: json!({"query": "tv_tool_"}),
+                    extra: None,
+                },
+                done(StopReason::ToolUse),
+            ],
+            vec![
+                LlmEvent::TextDelta("done".to_string()),
+                done(StopReason::EndTurn),
+            ],
+        ]),
+        captured: Arc::clone(&captured),
+    };
+
+    let config = test_config();
+    let output: Arc<dyn OutputSink> = Arc::new(NullSink);
+    let mut engine =
+        AgentEngine::new_with_provider(Arc::new(provider), config, mcp_registry(), output);
+
+    engine
+        .run("find and use the tv tools", "")
+        .await
+        .expect("engine.run should succeed");
+
+    let requests = captured.lock().unwrap();
+    assert!(
+        requests.len() >= 2,
+        "expected an initial request plus the post-ToolSearch follow-up, got {}",
+        requests.len()
+    );
+
+    // Turn 1 baseline: the MCP tools are folded out (catalog mode), so the
+    // gate below is not passing for free.
+    assert!(
+        !requests[0].tools.iter().any(|t| t.name == "tv_tool_000"),
+        "turn 1 must NOT already declare the tool — otherwise this test proves nothing"
+    );
+
+    let admitted = requests[1]
+        .tools
+        .iter()
+        .find(|t| t.name == "tv_tool_000")
+        .unwrap_or_else(|| {
+            panic!(
+                "hydrated tool absent from the next turn's tools[] — declared: {:?}",
+                requests[1]
+                    .tools
+                    .iter()
+                    .map(|t| t.name.as_str())
+                    .collect::<Vec<_>>()
+            )
+        });
+    assert!(
+        !admitted.deferred,
+        "a hydrated tool must ship its FULL schema; a stub is not callable"
+    );
+    assert_eq!(
+        admitted.input_schema["properties"]["symbol"]["type"], "string",
+        "the declared schema must be the real one"
+    );
+}
+
+/// The other half of the same defect: the ToolSearch body the model is shown
+/// must never be cut by the orchestration truncator, because that cut is what
+/// destroyed the hydration record. Asserts on the transcript the provider
+/// actually received.
+#[tokio::test]
+async fn tool_search_body_is_never_spliced_by_the_result_truncator() {
+    let captured: Arc<Mutex<Vec<LlmRequest>>> = Arc::new(Mutex::new(Vec::new()));
+    let provider = CapturingProvider {
+        inner: MockLlmProvider::with_turns(vec![
+            vec![
+                LlmEvent::ToolUse {
+                    id: "ts-2".to_string(),
+                    name: "ToolSearch".to_string(),
+                    input: json!({"query": "tv_tool_"}),
+                    extra: None,
+                },
+                done(StopReason::ToolUse),
+            ],
+            vec![
+                LlmEvent::TextDelta("done".to_string()),
+                done(StopReason::EndTurn),
+            ],
+        ]),
+        captured: Arc::clone(&captured),
+    };
+
+    let output: Arc<dyn OutputSink> = Arc::new(NullSink);
+    let mut engine = AgentEngine::new_with_provider(
+        Arc::new(provider),
+        test_config(),
+        mcp_registry(),
+        output,
+    );
+    engine.run("search the tv tools", "").await.expect("run");
+
+    let requests = captured.lock().unwrap();
+    let body = requests
+        .get(1)
+        .into_iter()
+        .flat_map(|r| &r.messages)
+        .flat_map(|m| &m.content)
+        .find_map(|b| match b {
+            wcore_types::message::ContentBlock::ToolResult { content, .. } => {
+                Some(content.as_str())
+            }
+            _ => None,
+        })
+        .expect("the follow-up request carries the ToolSearch result");
+    assert!(
+        !body.contains("[truncated"),
+        "ToolSearch body was spliced by the result truncator: {}",
+        &body[..body.len().min(400)]
+    );
+}
diff --git a/crates/wcore-tools/src/tool_search.rs b/crates/wcore-tools/src/tool_search.rs
index 4e36d67..6e88377 100644
--- a/crates/wcore-tools/src/tool_search.rs
+++ b/crates/wcore-tools/src/tool_search.rs
@@ -13,6 +13,27 @@ use crate::context::ToolContext;
 /// overhead of repeated `is_cancelled()` calls on a `CancellationToken`.
 const CANCEL_CHECK_INTERVAL: usize = 100;
 
+/// HARD budget on the JSON-array portion of a `ToolSearch` result, in bytes.
+///
+/// This exists to keep the result INSIDE [`ToolSearchTool::max_result_size`]
+/// so the orchestration truncator (`truncate_result`, which splices
+/// `... [truncated N chars] ...` into the MIDDLE of the payload) can never
+/// run on a ToolSearch body. That splice does not merely shorten the text —
+/// it destroys the JSON, and the engine's hydration recorder parses this body
+/// to learn which tools the model just loaded. A cut body therefore recorded
+/// NOTHING, silently, and the hydrated tool stayed out of `tools[]` forever.
+/// Reachable in practice: one MCP server with ~100 tools answers a broad
+/// query with well over 50 kB.
+///
+/// Overflow is reported to the model (see `OVERFLOW_NOTE_PREFIX`) instead of
+/// being hidden, so a query that is too broad is a visible, actionable
+/// condition rather than a silently short list.
+const MAX_MATCH_BYTES: usize = 32_000;
+
+/// Leading text of the trailing overflow note. Kept OUTSIDE the JSON array so
+/// the array itself stays a valid, parseable JSON document.
+const OVERFLOW_NOTE_PREFIX: &str = "\n\nNOTE: ";
+
 /// Built-in tool that searches for deferred tools and loads their full schema.
 /// Core tool (never deferred itself) — always available to the LLM.
 pub struct ToolSearchTool {
@@ -76,6 +97,8 @@ impl Tool for ToolSearchTool {
 
         let query_lower = query.to_lowercase();
         let mut matches: Vec<Value> = Vec::new();
+        let mut used_bytes: usize = 2; // the enclosing `[` + `]`
+        let mut omitted: usize = 0;
 
         for (idx, def) in self.tool_defs.iter().enumerate() {
             if idx % CANCEL_CHECK_INTERVAL == 0 && ctx.cancel.is_cancelled() {
@@ -90,11 +113,23 @@ impl Tool for ToolSearchTool {
             let name_l = def.name.to_lowercase();
             let desc_l = def.description.to_lowercase();
             if name_l.contains(&query_lower) || desc_l.contains(&query_lower) {
-                matches.push(json!({
+                let entry = json!({
                     "name": def.name,
                     "description": def.description,
                     "parameters": def.input_schema,
-                }));
+                });
+                // Charge the entry against the budget BEFORE admitting it.
+                // `to_string_pretty` of the standalone entry under-counts the
+                // in-array form only by the two extra indent spaces per line,
+                // and `max_result_size()` leaves a 2x margin over this budget,
+                // so the emitted body is bounded even with that slack.
+                let cost = serde_json::to_string_pretty(&entry).map_or(0, |s| s.len()) + 8;
+                if !matches.is_empty() && used_bytes + cost > MAX_MATCH_BYTES {
+                    omitted += 1;
+                    continue;
+                }
+                used_bytes += cost;
+                matches.push(entry);
             }
         }
 
@@ -114,12 +149,30 @@ impl Tool for ToolSearchTool {
             };
         }
 
+        let body = serde_json::to_string_pretty(&matches).unwrap_or_default();
+        let content = if omitted == 0 {
+            body
+        } else {
+            format!(
+                "{body}{OVERFLOW_NOTE_PREFIX}{omitted} further match(es) were omitted to keep \
+                 this result within its size budget. Narrow the query (for example to an exact \
+                 tool name) to see them."
+            )
+        };
+
         ToolResult {
-            content: serde_json::to_string_pretty(&matches).unwrap_or_default(),
+            content,
             is_error: false,
         }
     }
 
+    /// Twice [`MAX_MATCH_BYTES`] plus room for the overflow note. The tool
+    /// bounds its OWN output above; this only has to be provably larger than
+    /// that bound so the orchestration truncator is unreachable here.
+    fn max_result_size(&self) -> usize {
+        MAX_MATCH_BYTES * 2
+    }
+
     fn category(&self) -> ToolCategory {
         ToolCategory::Info
     }
@@ -206,4 +259,46 @@ mod tests {
         let result = tool.execute(json!({"query": ""})).await;
         assert!(result.is_error);
     }
+
+    /// The result body must stay inside the tool's own declared
+    /// `max_result_size()`, because the orchestration truncator splices a
+    /// marker into the MIDDLE of anything larger and that destroys the JSON
+    /// the engine's hydration recorder parses.
+    ///
+    /// Mutant: delete the `used_bytes + cost > MAX_MATCH_BYTES` guard in
+    /// `execute_with_ctx` and this fails — the body grows past
+    /// `max_result_size()`.
+    #[tokio::test]
+    async fn wide_match_set_stays_inside_the_declared_result_size() {
+        let filler = "d".repeat(700);
+        let defs: Vec<ToolDef> = (0..200)
+            .map(|i| ToolDef {
+                name: format!("srv_tool_{i:03}"),
+                description: filler.clone(),
+                input_schema: json!({
+                    "type": "object",
+                    "properties": {"symbol": {"type": "string"}}
+                }),
+                deferred: true,
+                server: Some("srv".into()),
+            })
+            .collect();
+        let tool = ToolSearchTool::new(defs);
+        let result = tool.execute(json!({"query": "srv_tool_"})).await;
+        assert!(!result.is_error);
+        assert!(
+            result.content.len() <= tool.max_result_size(),
+            "body {} bytes exceeds the declared max_result_size {}",
+            result.content.len(),
+            tool.max_result_size()
+        );
+        // The first match is always present, and the overflow is REPORTED.
+        assert!(result.content.contains("srv_tool_000"));
+        assert!(result.content.contains("were omitted"));
+        // The JSON array prefix is still a parseable array.
+        let end = result.content.rfind(']').expect("array terminator");
+        let parsed: Value =
+            serde_json::from_str(&result.content[..=end]).expect("array prefix must parse");
+        assert!(parsed.as_array().is_some_and(|a| !a.is_empty()));
+    }
 }

```
