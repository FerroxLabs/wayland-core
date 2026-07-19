# Codebase Concerns

**Analysis Date:** 2026-07-18

## Tech Debt

**[Proven] Production modules exceed the repository's size limit:**
- Issue: The project requires modules to stay below 1,000 lines, but a repository-wide scan finds 99 non-test production Rust files above that threshold. The largest are `crates/wcore-agent/src/engine.rs` (28,511 lines), `crates/wcore-cli/src/tui/surfaces/workspace.rs` (8,586), `crates/wcore-config/src/config.rs` (8,189), `crates/wcore-cli/src/main.rs` (7,640), and `crates/wcore-cli/src/tui/surfaces/mod.rs` (7,122).
- Files: `AGENTS.md`, `crates/wcore-agent/src/engine.rs`, `crates/wcore-cli/src/tui/surfaces/workspace.rs`, `crates/wcore-config/src/config.rs`, `crates/wcore-cli/src/main.rs`, `crates/wcore-cli/src/tui/surfaces/mod.rs`
- Impact: Core request execution, configuration, CLI startup, and TUI routing have large change surfaces; reviewers cannot isolate responsibilities easily, and unrelated behavior is likely to share one compilation unit and test module.
- Fix approach: Extract responsibility-aligned submodules behind existing private APIs, starting with `crates/wcore-agent/src/engine.rs` and the TUI surface files. Preserve public types and verify each extraction on the authoritative non-Mac Cargo harness.

**[Proven] Plugin configuration is defined but not loaded by production bootstrap:**
- Issue: `PluginsConfig::from_toml_str` exists, but its only callers are unit tests. `AgentBootstrap::build` constructs `PluginsConfig::default()` and passes it directly to `PluginLoader::discover`, so operator enable/disable entries and `trusted_plugin_keys` from `plugins.toml` never reach discovery even though the documentation says they are read at engine boot.
- Files: `crates/wcore-config/src/plugins_config.rs`, `crates/wcore-agent/src/bootstrap.rs`, `crates/wcore-agent/src/plugins/loader.rs`, `docs/plugin-authors.md`, `docs/providers.md`
- Impact: Operators cannot control compiled-in plugins through the documented file, and config-provided trust anchors are ineffective. Filesystem trust keys and the explicit unsigned-development environment override remain the only effective dynamic-plugin controls.
- Fix approach: Add one canonical `plugins.toml` load path in `wcore-config`, inject the resolved `PluginsConfig` into `AgentBootstrap`, call the fail-closed `PluginLoader::try_discover`, and add a boot-level test proving disable flags and config trust keys alter discovery.

**[Proven] Provider capability metadata is too coarse for model-specific behavior:**
- Issue: `ProviderCompat::anthropic_defaults` sets `supports_thinking = true` for the whole provider while its own TODO records that at least one Anthropic model does not support extended thinking.
- Files: `crates/wcore-config/src/compat.rs`, `crates/wcore-providers/src/anthropic.rs`
- Impact: A model can inherit an unsupported request feature, producing provider-side request rejection instead of a local capability decision.
- Fix approach: Move thinking/effort support into the existing model-capability resolution layer while keeping provider request construction driven by `ProviderCompat`; add per-model request-body tests for supported and unsupported models.

**[Proven uncertainty; rate correctness unverified] An active pricing row carries an unclosed audit TODO:**
- Issue: `deepseek.deepseek-v4-pro` is used by the bundled pricing catalog and Flux-pinned cost estimation, but the catalog comment says its exact SKU mapping/rate still needs confirmation.
- Files: `crates/wcore-pricing/pricing.toml`, `crates/wcore-pricing/src/lib.rs`
- Impact: If the provisional mapping is wrong, budget enforcement and cost-aware routing can use a systematically wrong cost for `flux-pinned-deepseek-v4-pro`. The source scan proves the uncertainty, not that the listed rate is wrong.
- Fix approach: Verify the SKU against an authoritative provider source, record provenance and retrieval date next to the row, then add a catalog fixture that pins the confirmed model-to-rate mapping.

## Known Bugs

**[Proven, latent until used] `plugin.deferred = true` does not defer initialization:**
- Symptoms: `PluginInfo` promises initialization on first use and `DeferredPluginRegistry::split_off` implements storage, but no production caller constructs or invokes that registry. Bootstrap sends every discovered inventory plugin directly to `PluginRunner::initialize_all`.
- Files: `crates/wcore-plugin-api/src/manifest.rs`, `crates/wcore-agent/src/plugins/deferred.rs`, `crates/wcore-agent/src/plugins/mod.rs`, `crates/wcore-agent/src/bootstrap.rs`, `crates/wcore-agent/src/plugins/runner.rs`
- Trigger: A compiled-in plugin sets `deferred = true` in its manifest.
- Workaround: Do not rely on plugin-level deferred initialization; accept eager initialization or keep the plugin out of the compiled inventory until the first-use wake path is wired.

**[Proven] `PreCompact` plugin contributions are discarded:**
- Symptoms: The engine calls `run_pre_compact`, but the hook implementation only records trace lines and returns a redacted empty outcome; it never dispatches or folds a plugin contribution into compaction input.
- Files: `crates/wcore-agent/src/hooks/mod.rs`, `crates/wcore-agent/src/engine.rs`
- Trigger: A plugin registers a `PreCompact` hook that returns content intended to influence compaction.
- Workaround: Use a contribution-capable phase such as `PrePrompt`; `PreCompact` is currently log-only.

## Security Considerations

**[Proven] Learned sub-agent tool policy is not enforced in production:**
- Risk: `CallActor` and `LearnedPolicy` are public building blocks, but production constructs only `CallActor::Root` with `learned_policy = None`; the learned-policy pre-filter was removed from `dispatch_once`, and all five integration tests are ignored. Persisted allow/deny decisions therefore cannot constrain delegated-agent tool calls through this path.
- Files: `crates/wcore-permissions/src/actor.rs`, `crates/wcore-permissions/src/learning.rs`, `crates/wcore-agent/src/orchestration/node_executor.rs`, `crates/wcore-agent/tests/actor_acl_test.rs`
- Current mitigation: The independent `PolicyGate` remains an unconditional pre-dispatch floor when configured, and the normal host/terminal approval path still applies. The missing layer is specifically actor-attributed learned policy, not all tool authorization.
- Recommendations: Thread the real source agent through spawn/orchestration, construct `CallActor::SubAgent`, load the procedural `LearnedPolicy`, restore the pre-filter, and unignore the deny-before-dispatch tests.

**[Proven availability/configuration gap, not an unsigned-load bypass] Plugin trust configuration cannot come from `plugins.toml`:**
- Risk: Bootstrap's hardcoded default omits config-provided trust keys and plugin enablement policy. This can make signed plugins unexpectedly unavailable and makes the documented policy file non-authoritative.
- Files: `crates/wcore-config/src/plugins_config.rs`, `crates/wcore-agent/src/bootstrap.rs`, `crates/wcore-agent/src/plugins/loader.rs`, `docs/plugin-authors.md`
- Current mitigation: Signature verification defaults to enabled; missing keys fail closed for path-based plugin artifacts. `WAYLAND_PLUGIN_TRUST_UNSIGNED=1` is explicit and logged as a development-only escape hatch.
- Recommendations: Load and validate `plugins.toml` before discovery, surface `try_discover` configuration errors at boot, and retain the current fail-closed default.

## Performance Bottlenecks

**[Proven] Channel session engines are unbounded and expensive to create:**
- Problem: `ChannelTurnDispatcher` retains one `AgentEngine` per distinct hashed conversation in an unbounded `HashMap`; every cache miss runs the full `AgentBootstrap`, including MCP, plugins, and skills initialization.
- Files: `crates/wcore-agent/src/channel_dispatch.rs`, `crates/wcore-agent/src/bootstrap.rs`
- Cause: The pool has neither an LRU/idle eviction policy nor a shared initialized-subsystem layer. Concurrent misses may also build duplicate engines before the entry race is resolved, discarding all but the first.
- Improvement path: Add bounded idle/LRU eviction with explicit session persistence semantics, then factor immutable expensive bootstrap products into shared session-safe state. Add flood and concurrent-same-session tests before enabling eviction.

**[Proven] Semantic and legacy episodic memory retrieval can scan and sort every row:**
- Problem: `facts_cosine_pass` loads all live embedded facts for a tier and sorts them in memory. Episodic search falls back to `legacy_cosine_pass`, which does the same for all active legacy episodes whenever the sqlite-vec KNN table returns no hits.
- Files: `crates/wcore-memory/src/retrieve.rs`, `crates/wcore-memory/src/partition/episodic.rs`
- Cause: Semantic facts have no vector-index fast path, and older rows written without `record_with_embedding` are absent from the sqlite-vec mirror.
- Improvement path: Backfill legacy episode vectors into the dimension-specific vec table, add indexed semantic-fact retrieval, and retain an explicitly capped migration fallback rather than an unbounded full scan.

**[Unverified] TUI producer bursts may outpace unbounded internal channels:**
- Problem: The input reader and multiple engine/recovery/session-switch paths use `tokio::sync::mpsc::unbounded_channel`; no queue-depth telemetry or burst benchmark proves whether producers can outpace the render/consumer loop.
- Files: `crates/wcore-cli/src/tui/mod.rs`, `crates/wcore-cli/src/tui/engine_bridge.rs`
- Cause: Human input is normally low-rate, but bracketed paste, protocol event bursts, and background actions share channels without backpressure.
- Improvement path: Measure peak queue depth and consumer latency under paste and event-burst fixtures first. Only if growth is observed, use bounded channels or coalesce replaceable redraw/status events.

## Fragile Areas

**Agent engine and bootstrap composition root:**
- Files: `crates/wcore-agent/src/engine.rs`, `crates/wcore-agent/src/bootstrap.rs`, `crates/wcore-agent/src/orchestration/mod.rs`, `crates/wcore-agent/src/spawner.rs`
- Why fragile: These files combine provider streaming, recovery, compaction, tool dispatch, plugins, MCP, session state, budgets, hooks, and child-agent orchestration. Multiple TODOs describe partially connected cross-cutting state such as live session budget identity and skill reload.
- Safe modification: Change one seam at a time, preserve typed policy/compat layers, add a focused regression at that seam, and run committed-HEAD proof on the allowed remote harness. Avoid provider-name conditionals and direct shell spawning.
- Test coverage: Unit and integration coverage is extensive, but ignored actor-policy tests and deferred-hook/plugin paths leave important composition behavior unproved.

**Plugin discovery-to-runtime pipeline:**
- Files: `crates/wcore-plugin-api/src/manifest.rs`, `crates/wcore-config/src/plugins_config.rs`, `crates/wcore-agent/src/plugins/loader.rs`, `crates/wcore-agent/src/plugins/runner.rs`, `crates/wcore-agent/src/plugins/apply.rs`, `crates/wcore-agent/src/bootstrap.rs`
- Why fragile: Manifest promises, host policy, signature verification, runtime dispatch, capability capture, and final tool/hook registration span multiple crates. Current drift already leaves config loading and deferred initialization disconnected.
- Safe modification: Treat manifest parsing, policy resolution, artifact verification, initialize timing, and capability application as separate gates with end-to-end receipts/tests for each runtime type.
- Test coverage: Loader and runner units exist, but there is no boot-level proof that a real `plugins.toml` controls discovery or that a deferred plugin wakes exactly once.

**TUI routing and surfaces:**
- Files: `crates/wcore-cli/src/tui/surfaces/mod.rs`, `crates/wcore-cli/src/tui/surfaces/workspace.rs`, `crates/wcore-cli/src/tui/engine_bridge.rs`, `crates/wcore-cli/tests/smoke_p0.rs`
- Why fragile: Keyboard routing, rendering, approvals, protocol events, recovery, and session switching live in very large shared modules; many behavior checks require PTY state and platform-specific event handling.
- Safe modification: Assert rendered output and routed action together, keep input ownership cancel-safe, and preserve Windows/macOS/Linux branches when splitting modules.
- Test coverage: Several ordered smoke checks are absent or ignored, and some PTY/live tests require dedicated environments rather than the default suite.

## Scaling Limits

**Channel conversations:**
- Current capacity: No configured maximum; one retained engine is allocated per distinct channel session key.
- Limit: Memory and bootstrap work grow with the number of distinct conversations for the lifetime of the dispatcher.
- Scaling path: Bound the pool, evict idle engines after durable state is flushed, and expose active/evicted session metrics.

**Long-term memory rows:**
- Current capacity: No row cap is enforced by `facts_cosine_pass` or `legacy_cosine_pass`; each fallback query materializes all matching embedded rows before sorting.
- Limit: Query CPU, allocation, and mutex hold time grow linearly with stored facts/legacy episodes, plus `O(n log n)` sorting.
- Scaling path: Complete vec-index coverage and backfill, paginate or cap fallback scans, and benchmark latency against realistic per-tier row counts.

## Dependencies at Risk

**[Unverified] Current lockfile advisory status:**
- Risk: `Cargo.lock` exists, but this mapping did not run Cargo or an advisory scanner on the Mac. Static manifests show explicit mitigations for `RUSTSEC-2024-0320`, `RUSTSEC-2026-0098/0099/0104`, `RUSTSEC-2026-0187`, and the RSA timing issue; they do not prove the whole lockfile is currently clean.
- Impact: No current vulnerable dependency is proven by this scan, but advisory status remains uncertified until the lockfile is checked in the allowed environment.
- Migration plan: Run `cargo audit --locked` on the authoritative Hetzner harness and update exact security-critical pins only with cross-platform build/test proof.

## Missing Critical Features

**Plugin-level deferred initialization:**
- Problem: The manifest flag and registry data structure exist, but no first-use wake-up path is connected to discovery, bootstrap, or tool dispatch.
- Blocks: Plugins cannot rely on lazy startup to avoid boot cost or defer unavailable optional infrastructure.

**In-session skill hot reload:**
- Problem: `SkillWatcher` detects catalog version changes, but the spawned task only logs the version and keeps the catalog alive; it does not call `AgentEngine::set_skill_catalog` during the current session.
- Blocks: Added or changed skills are not usable until a new session starts despite the watcher being active.

**Ollama tool calling:**
- Problem: `OllamaProvider` rejects `ToolUse` and `ToolResult` content because `/api/chat` tool serialization is not implemented.
- Blocks: Local Ollama sessions cannot complete normal agent tool loops; they are limited to text-only interactions.

**One-hour Anthropic prompt-cache tier:**
- Problem: `CacheTier::Ephemeral1h` and its picker are scaffolded, but `LlmRequest` cannot carry the selection and production request construction always uses the five-minute tier.
- Blocks: Long-reuse prompts cannot opt into the one-hour cache policy through the provider-neutral request path.

## Test Coverage Gaps

**P0 ordered smoke behavior:**
- What's not tested: Checks `#7`, `#8`, `#9`, `#11`, `#14`, `#18`, and `#19` have no test; interactive checks `#15`, `#22`, and `#23` remain ignored/scaffolded. The smoke script gates only four implemented checks and reports other gaps without failing the release.
- Files: `scripts/smoke.sh`, `crates/wcore-cli/tests/smoke_p0.rs`
- Risk: A release can pass the smoke gate while ordered first-use and interactive behavior remains unverified.
- Priority: High

**Sub-agent learned-policy enforcement:**
- What's not tested: All five actor/learned-policy integration tests are ignored because the production pre-filter is absent.
- Files: `crates/wcore-agent/tests/actor_acl_test.rs`, `crates/wcore-agent/src/orchestration/node_executor.rs`
- Risk: Future wiring can accidentally execute a denied tool before the policy result is applied.
- Priority: High

**Provider HTTP error classification:**
- What's not tested: Azure OpenAI, Cohere, and Vertex lack wiremock coverage for `400`, `401`, `403`, `429`, and `500` classification even though the status handling code exists.
- Files: `crates/wcore-providers/src/azure_openai.rs`, `crates/wcore-providers/src/cohere.rs`, `crates/wcore-providers/src/vertex.rs`
- Risk: Retryability, authentication, rate-limit, and server-failure semantics can drift without provider-specific regression detection.
- Priority: Medium

**Real WASM component execution:**
- What's not tested: The real `tool.execute` round trip is ignored unless a built `plugin_wasm_hello.wasm` fixture is present; the default tests cover malformed/minimal component paths instead.
- Files: `crates/wcore-plugin-wasm/tests/wasm_real_execute.rs`, `crates/wcore-plugin-wasm/tests/wasm_e2e.rs`, `examples/plugin-wasm-hello/Cargo.toml`
- Risk: Component linking, host imports, and real guest execution can break while negative-path tests remain green.
- Priority: Medium

**Deferred plugin and `PreCompact` contribution contracts:**
- What's not tested: No boot/dispatch test proves `plugin.deferred` changes initialization timing, and no test proves a `PreCompact` contribution reaches compaction input.
- Files: `crates/wcore-agent/src/plugins/deferred.rs`, `crates/wcore-agent/src/bootstrap.rs`, `crates/wcore-agent/src/hooks/mod.rs`, `crates/wcore-agent/src/engine.rs`
- Risk: Public extension contracts remain silently inert.
- Priority: High

---

*Concerns audit: 2026-07-18*
