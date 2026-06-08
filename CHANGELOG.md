# Changelog

## [0.9.6-rc.2] - 2026-06-07

Release-candidate cut closing the defect-remediation campaign. The canonical
version is the workspace package version in `Cargo.toml`, which
`wayland-core --version` reports; this CHANGELOG and the README track it.

This re-cuts rc.2 from the finished campaign HEAD; the rc.2 tag of 2026-06-06
was never published. It lands the full deduped defect ledger plus two
binary-integration holdouts and a cross-audit follow-up, on top of the ACP/A2A
engine wire-up, the REST/OpenAPI surface, and the 104-provider catalog. The work
was gated per change and closed with a campaign-wide adversarial audit (a
10-unit, four-lens review with independent skeptic verification) plus an external
cross-audit; the audit found no high or critical issues in the campaign diff.

### Fixed

* **Onboarding and config saves reach the live engine (no restart).** Completing
  onboarding or saving a credential / config rebinds the running engine in
  place, so the very next turn uses the new provider, model, and system prompt.
* **`/model` is a live library.** The picker fetches the active provider's real
  model list (`GET /v1/models` for OpenAI-compatible providers, the Anthropic
  models endpoint) and falls back to the bundled aliases only on error;
  switching provider re-fetches.
* **Phantom slash verbs now act.** `/provider`, `/profile`, `/resume`,
  `/skills`, `/mcp add`, `/auth`, and `/rewind` perform their advertised action
  instead of printing placeholder copy.
* **Keyless launch recovers in-app.** A configured-but-keyless provider (for
  example a catalog provider with no credential) opens the onboarding recovery
  instead of crashing to stderr; a corrupt config still aborts visibly. A
  catalog provider's own environment variable (for example `NOVITA_API_KEY`) is
  honored as a credential.
* **Input stays responsive after a turn.** The TUI no longer stops accepting
  keystrokes once a turn completes; a dropped input-poll future had been
  orphaning a blocking read that consumed the next keystroke.
* **`/rewind` scratch directory hardened.** Checkpoints stage in a `0700`,
  unguessable per-session directory rather than a predictable `/tmp` path.

### Changed — Plugin signing (BREAKING)

* **Plugin signing is unified.** The detached signature file is now
  `<plugin_dir>/wayland-plugin.sig` (raw 64-byte ed25519 over the entry
  binary). The legacy `<plugin_path>.sig` sidecar is removed. Operators
  with config-side keys keep them in `plugins.toml::trusted_plugin_keys`
  (base64 ed25519 public keys); filesystem-side keys go in
  `~/.wayland/trusted-keys/*.pub` (raw 32-byte ed25519 public keys,
  override the directory with `WAYLAND_TRUSTED_KEYS_DIR`). Both sources
  are unioned at verify time — the loader accepts on first match.
  Static plugins (no artifact path) always skip signing; the engine
  binary is their trust anchor. Set
  `WAYLAND_PLUGIN_TRUST_UNSIGNED=1` to opt out for development.
  Pre-existing deployments with `plugin_signature_verification = true`
  and `<binary>.sig` sidecars MUST migrate the sidecar to
  `<plugin_dir>/wayland-plugin.sig`.

## [0.6.3] - 2026-05-20

The "Wire The Scaffolding" release. Every primitive that v0.6.2 shipped as
scaffolded is now wired into the production path; a real cross-platform
sandbox perimeter is added; and the provider/tool catalog grows by 23
entries. 44 planned items delivered across sandbox (11), wiring (10),
providers (8), and tools (15), plus catalog integration. Three audit
rounds (7 agents) found 1 CRITICAL + 7 HIGH + ~20 MEDIUM/LOW — all
addressed. 4-gate green (fmt + check + clippy + test); 4482 tests pass /
0 fail (+316 vs v0.6.2).

### Added — Sandbox (new security perimeter)

* **`SandboxBackend` trait + `SandboxManifest` schema** — fs read/write
  allowlists, network policy, syscall policy, resource limits, env.
* **Linux** — bubblewrap backend (`--new-session`, `--tmpfs /tmp`),
  Landlock 5.13+ fs rulesets, and seccomp-bpf filtering applied via
  `bwrap --seccomp <fd>`. `seccomp` + `landlock` are cargo features,
  enabled by default on Linux targets.
* **macOS** — `sandbox-exec` backend with a Tahoe-aware profile
  (`hw.*` sysctl allowance), file-based profile load, startup probe,
  and `env -i` isolation. SBPL profile paths are escaped.
* **Windows** — AppContainer + Job Objects backend (full implementation).
* **Docker** — opt-in via `WAYLAND_SANDBOX=docker`; lazy client, cheap
  socket-probe `is_available()`.
* **NoSandbox** — warn-once at registry construction; `SandboxError::NotRequired`.
* **BashTool** — all four execute paths routed through the sandbox;
  `SandboxBackend::execute_streaming()` added. Sandboxed children receive
  a curated env allowlist — provider API keys and `WAYLAND_VAULT_PASSPHRASE`
  are never broadcast.
* **EncryptedFile credentials vault** — real Argon2id + XChaCha20-Poly1305
  implementation.

### Added — Wiring (12 scaffolded primitives → wired)

* Smart-routing hint produced and consumed on `LlmRequest` (W1).
* Knowledge-graph + staleness schema initialized in `AgentBootstrap` (W2, W4).
* Auto-memorize session-end trigger (W3); fact-extractor → KG ingest (W5).
* Cross-project memory consulted during skill resolution (W6).
* Pricing catalog → budget charge wired with correct microcents→USD
  conversion (W7).
* `CacheTier` moved to `wcore-types`; `cache_tier` on `LlmRequest`
  produced by `pick_cache_tier` and consumed by the Anthropic cache
  zones, enabling the 1h prompt-cache tier (W8).
* `WAYLAND_TRACE_RESULT_SNIPPETS` env gate wired to the real capture
  site (W9); dead failover-flag docs removed (W10).

### Added — Providers (8 new — 17 → 25)

* Azure OpenAI (deployment-name routing, api-key auth), Together AI,
  Fireworks AI, NVIDIA NIM, Perplexity (Sonar), Cerebras, Bedrock
  Mistral, Bedrock Cohere. All selectable via `ProviderType` /
  `parse_builtin_provider`; cost attribution keyed to the real provider.

### Added — Tools (15 new — 65 → 80)

* SQL query, Postgres schema introspection, GitHub API, GitLab API,
  Linear API, Notion API, archive (zip/tar, zip-slip guarded), Markdown
  table format/lint, JSON Lines streaming, PDF text extraction, image
  metadata inspection, email (.eml/.mbox) parsing, and read-only
  `kubectl` / `gcloud` / `aws` CLI wrappers (verb allowlists, sandboxed).
  All 15 registered in `AgentBootstrap`; the 4 API tools bound to real
  HTTP backends.

### Changed

* `LlmRequest` derives `Default`; `cache_tier` + `routing_hint` fields added.
* `CacheTier` / `CacheTierConfig` / `pick_cache_tier` relocated to
  `wcore-types` (re-exported from `wcore-providers` for compatibility).

### Fixed

* Reasoning-token / Docker-alignment / merge-integration fixes across
  the audit cycle; the 6 new providers no longer charged at GPT-4o
  rates; the `landlock` feature compile break (E0283) resolved.

### Known limitations (working features; enhancements tracked for v0.6.4)

* Mistral/Cohere on Bedrock use the non-streaming `invoke` endpoint
  (buffered, then emitted) — native event-stream parsing is a v0.6.4
  enhancement.
* Azure OpenAI ships API-key auth; AAD/OAuth bearer mode is v0.6.4.
* The Linux `seccomp,landlock` feature combination requires a Linux CI
  job to compile-verify (the dev host is macOS).

## [0.6.2] - 2026-05-19

The Tier-1 / Tier-2 / Tier-3 lift release. 27 lifts shipped (12 Tier 1 +
15 Tier 2) covering provider reliability, semantic compaction, knowledge-
graph schema, MCP server, and smart-routing primitives; 65+ Tier 3 tool
ports plus `cargo-deny` supply-chain audit; 10 new OpenAI-compatible
provider adapters (mistral, xai, openrouter, openai-compatible, litellm,
lmstudio, ollama, vllm, deepseek, groq). ~9000 LOC added, 270+ tests.
Two cross-audit rounds; 38 findings, all addressed. 4-gate green (fmt +
check + clippy + test); 4166 tests pass / 0 fail.

### Added

* **Provider reliability** — `FailoverReason` taxonomy with
  `ContextOverflow` recovery path, 3-tier failover classifier
  (`status × body × sdk_code`), `CooldownTracker` state machine,
  `ProviderRegistry` trait, `Retry-After` nested extraction, API key
  rotation pool.
* **Provider adapters (10 new)** — Mistral, xAI, OpenRouter, OpenAI-
  compatible (generic), LiteLLM, LM Studio, Ollama, vLLM, DeepSeek,
  Groq. All OpenAI-API-compatible adapters routed through
  `OpenAIProvider` with `ProviderCompat` differences encoded as data,
  not hardcoded conditionals.
* **Pricing** — `wcore-pricing` crate with 15 providers / 21 model
  rows + self-healing 24h TTL OpenRouter HTTP refresh. Override the
  bundled catalog via `WAYLAND_PRICING_PATH`; disable live refresh via
  `WAYLAND_PRICING_AUTO_REFRESH=off`. (Crate ships standalone; wiring
  into the budget pipeline / `ProviderRegistry` deferred to v0.6.3.)
* **Cache tiering** — `CacheTier::Ephemeral5m` end-to-end +
  `CacheRetention` / `InvalidationCause` / `PromptCacheObservation`
  taxonomy. (`CacheTier::Ephemeral1h` scaffolded — wiring through
  `LlmRequest` deferred to v0.6.3.)
* **Credentials** — `CredentialsBackend::EncryptedFile` variant
  (Argon2id + XChaCha20-Poly1305 + zeroize). NOTE: variant accepts the
  config but the runtime impl returns `BackendUnavailable` until v0.6.3
  wires the vault; use `WAYLAND_VAULT=plaintext` for v0.6.2.
* **Crash sentinel** — `~/.wayland/.crash-sentinel` flag for dirty-
  death detection on startup.
* **Knowledge graph** — schema + BFS + node/edge upsert (scaffolded;
  `WAYLAND_KG` flag awaits production init wiring in v0.6.3).
* **Semantic compaction** — transcript rewrite, identifier policy,
  scope mode.
* **MCP server** — stdio + SSE transports.
* **Smart routing primitives** — routing tier / decision / heuristics
  (scaffolded; dispatch wiring deferred to v0.6.3).
* **Hallucination guard** — claim extraction + cascade severity.
* **Knowledge schemas** — fact extractor, auto-memorize, cross-project
  memory, staleness (all scaffolded; wiring deferred to v0.6.3).
* **MoA (Mixture of Agents) tool** — Wang 2024 proposer fan-out +
  aggregator synth.
* **65+ Tier 3 tool ports** — file safety, path validation, URL
  safety, OSV malware check, schema sanitizer, browser / voice /
  transcription tools, Discord / Google Meet / HomeAssistant / Spotify
  tools, etc.
* **Plugin SDK extensions** + cargo-deny supply-chain audit.

### Changed

* **`tirith_security` default flipped to `fail_open: false`.**
  Previously the optional `tirith` binary missing silently allowed
  every command (the common case on machines without tirith). Set
  `TIRITH_FAIL_OPEN=true` to restore old behavior. Each fail-open
  return site now emits `tracing::warn`.
* **`classify_by_provider_error`** — `ProviderError::PromptTooLong`
  now maps to `FailoverReason::ContextOverflow` (was `Format`).
  Downstream policy compacts / routes to a larger-context model rather
  than swapping provider.
* **`path_validation`** added `~/.ssh/authorized_keys`,
  `~/.ssh/known_hosts`, `~/.ssh/id_dsa` to the read-path deny list
  (file_safety.rs blocked writes; path_validation didn't block reads).

### Fixed

* **MoA panic propagation** — `JoinSet` `.expect()` replaced with
  `MoaError::ProposerFailed` propagation; aggregator preserves
  `provider_id` on re-wrap (R2 fix A1).
* **MCP SSE Content-Length parse** — explicit invalid/missing 400s
  instead of silent fallback to 0.
* **MCP server `tools/list`** — removed stub-tool advertisements that
  returned `NOT_IMPLEMENTED` (R2 fix A3 — MCP protocol compliance).
  Defense-in-depth: known stub names still answered with
  `NOT_IMPLEMENTED` via `tools/call`, not `METHOD_NOT_FOUND`.
* **`classify_by_body`** — `"not allowed"` arm moved after billing +
  rate-limit checks (billing messages often phrase as "not allowed on
  your plan").
* **`halluci_guard`** file-path regex tightened — version numbers like
  `v0.6.2/foo` no longer match as paths.
* **`cross_project::discover_projects`** — symlinks no longer followed
  (could load same DB under multiple `project_id`s).
* **`file_state` mutex poison recovery** — 9 sites unified on a single
  warn-once recovery path (was: 8 silent + 2 panicking) (R2 fix A4).
* **`skills_lifecycle_cmd` test isolation** — added `#[serial(env)]`
  (test races with other crates that set `WCORE_MEMORY_DIR`).
* **`debug_helpers` test isolation** — added `#[serial(env)]` (R2 fix
  A2).
* **Crash sentinel double-arm** — `main.rs` no longer double-writes
  the flag on dirty-death path; new `check_dirty()` probe is side-
  effect-free (R2 fix A5).
* **`osv_check` SSRF block log** — `tracing::debug!` →
  `tracing::warn!` (operators monitoring at warn level now see SSRF
  attempts).

### Deferred to v0.6.3+

The following primitives are scaffolded, tested in isolation, and
exported from public APIs, but their production wiring is deferred:

* `wcore-providers::routing` — types + functions ready; no dispatch
  caller in v0.6.2.
* `wcore-memory::kg` — schema / BFS work; `init_kg` not called by
  `AgentBootstrap`.
* `wcore-memory::staleness` — helpers ready; not called by Memory
  write paths.
* `WAYLAND_KG`, `WAYLAND_STALENESS`, `WAYLAND_TRACE_RESULT_SNIPPETS`,
  `WAYLAND_FAILOVER_ENVELOPE`, `WAYLAND_LEGACY_PROVIDER_ENUM` —
  rollback flags documented but have no operational effect in v0.6.2.
* `CacheTier::Ephemeral1h` — variant defined; `LlmRequest` lacks the
  tier field.
* `CredentialsBackend::EncryptedFile` — variant accepted by config;
  runtime returns `BackendUnavailable`.
* `wcore-pricing` — catalog + 24h refresh primitives ready; no
  consumer crate wires it into provider dispatch or budget tracking
  in v0.6.2.

These will be wired in v0.6.3+; the underlying primitives ship now so
downstream callers do not face a re-port later. Module-level docs on
each scaffolded item flag the gap in source.

### Verification

* **4-gate** (fmt + check + clippy + test): all GREEN.
* **4166 tests pass / 0 fail.**
* **Cross-audit**: 2 rounds, 38 findings, all addressed.
* Tier 1 + 2 + 3 integrated cleanly into the trunk — only additive
  `pub mod` conflicts at integration time.

## [0.6.1] - 2026-05-19

Hardening release closing the v0.6.0 audit findings. Every change is
either a defensive fix or a test that proves the defence works; no new
features.

### Reliability

* **CRIT-2 — HTTP timeouts on every provider.** New `http_client::build()`
  helper applies `connect_timeout(30s)` + `read_timeout(120s)` to all 5
  LLM provider `reqwest::Client` constructors (Anthropic, OpenAI,
  Bedrock, Gemini, Vertex). A wedged upstream can no longer hang the
  agent indefinitely.
* **R4 — `ProviderChain` sequential fallback.** New primitive in
  `wcore-providers::chain` lets callers compose an ordered fallback list
  (e.g. `anthropic → openai → bedrock`); 5xx / 429 / connection errors
  move to the next slot, 4xx / parse errors terminate. Not yet wired
  into agent dispatch — building block only.
* **R5 — `CircuitBreaker` lifted to `wcore-config`.** Reusable primitive
  with rolling-window failure counting + half-open recovery. Wired into
  `wcore-tools::registry` (per-tool keying) so a misbehaving tool
  short-circuits subsequent invocations during the cooldown window.
  Consolidates the prior `resilient.rs` private breaker into the same
  type.
* **R6 — Retry framework wired into all 5 provider streams.** Default
  policy: 3 attempts, 250ms → 1s → 4s exponential with jitter. 429
  responses now honour the server's `retry-after` hint (capped at 60s).
  `is_request()` reqwest errors (invalid URL / header) are no longer
  retried — they were always going to fail.

### Stability

* **CRIT-4 / S1 — `atomic_write` for durable state.** New helper in
  `wcore-config::atomic_io` writes to a sibling tempfile, `fsync`s, then
  renames atomically. Applied to 13 production sites that previously
  used raw `fs::write`: OAuth credentials, plaintext credential store,
  memory store + index, session state + index, plan persistence, skills
  artifacts, plugin install records, evolve graveyard, replay traces,
  swarm heartbeats, VCR cassettes, skills audit, user-facing
  Edit/Write tools, agent VFS layer. A crash mid-write can no longer
  leave any of these files half-written.
* **S2 — `wcore-memory` writes now wrapped in DB transactions.** Three
  multi-step write paths (`EpisodicPartition::record_with_embedding`,
  `SemanticPartition::assert`, `ProceduralPartition::transition`)
  previously could leave orphan rows on a crash between INSERTs. All
  three now run inside `Connection::unchecked_transaction()`.
* **S6 — `OAuthCredentials` versioned.** Adds `version: u32` field
  (current = 1). Files without the field default to version 1 for
  backward compatibility; files with an unknown future version are
  rejected with a clear error rather than silently mis-parsed.

### Security

* **CRIT-1 / Sec1 — `wcore-permissions` wired into tool dispatch.** The
  M5.8 / M5.9 policy machinery shipped in v0.6.0 was orphan code with no
  production caller. v0.6.1 adds an optional `PolicyGate` field to
  `AgentEngine` + `AgentExecutorConfig`; when set, `dispatch_once`
  routes every tool call through the gate BEFORE the approval / budget
  pipeline. Fails closed (denied calls never reach hooks, sandbox, or
  any side-effecting machinery). Defaults to disabled for byte-identical
  v0.6.0 behaviour; opt in via `AgentEngine::set_policy_gate()`.
* **Sec3 — `BashTool` denylist expanded.** Adds patterns covering
  encoding-based credential exfil (`base64`, `xxd`, `od`, `hexdump`,
  `uuencode`) and `printf`/`awk` variants. Dedicated integration test
  proves denial of the credential-exfil class.
* **Sec5 — `OutputValidator` + `PIIScrubber` wired into observability.**
  New `wcore-safety` crate ships `RefusalDetector`,
  `CredentialLeakDetector`, `FormatValidator`, and a `PIIScrubber` with
  6 regex patterns (AWS keys, OpenAI `sk-` keys, Anthropic `sk-ant-`
  keys, JWTs, Bearer tokens, AWS secret-key context). `PIIScrubber` is
  wired into the `SpanSink` chain via a `PiiScrubbingSink` wrapper so
  secrets that leak into trace / log payloads are redacted before they
  reach disk or an OTLP exporter.
* **Sec6 — Opt-in ed25519 plugin signature verification.** Plugins may
  ship a `.sig` sidecar; when `plugin_signature_verification = true` in
  engine config, the loader rejects plugins with missing / invalid /
  mismatched signatures. Empty `trusted_plugin_keys` list with the flag
  on now errors out rather than silently bypassing verification.
  Default-off preserves existing behaviour.

### Test coverage

* **E1 — `wcore-compact`**: 18 integration tests across 3 scenario
  files (round-trip, ANSI handling, sanitisation, edge cases). Was 0
  tests before.
* **E2 — `wcore-mcp`**: 15 integration tests across 8 scenarios
  (handshake, request/response, concurrent requests, server disconnect,
  malformed messages, tool list, resource fetch).
* **E3 — Threat coverage**: T3 (sandbox ACL) and T4 (bearer-token
  budget claim) un-`#[ignore]`d and passing. T1 remains ignored — its
  precondition (`PluginManifest.actor` field + `install_guard`) is a
  separate feature, not a v0.6.1 deliverable.
* **E5 — 5 end-to-end scenarios**: swarm failover, provider fallback
  (uses R4's `ProviderChain`), compact end-to-end, memory concurrency
  (uses S2's transactions), MCP ↔ browser round-trip.

### Tooling

* **Sp2 — Criterion bench harness** in `wcore-agent`,
  `wcore-providers`, `wcore-tools`. No CI gate yet; benches are
  baseline-only so future PRs can detect regressions locally.

### Bugs surfaced during hardening

* **AF5 — Safe compaction byte-drift on ANSI-free input.** The Safe
  compaction level was stripping a trailing newline from inputs that
  contained no ANSI escape codes, mutating data it had no business
  touching. Sanitiser now preserves the input byte-for-byte when no
  escape sequences are present.

### Reclassified as false-alarms

The original audit flagged several findings that turned out to be
miscategorised. We document them here so future audits don't re-raise:

* **R2 / R3 — `openai.rs:808, 965, 966, 1069` unwraps.** All inside a
  `#[cfg(test)] mod tests` starting at line 756 — test code, not
  production.
* **R7 magnitude — "1422 production unwraps".** Raw grep without test
  filtering. Real production count is ~15, of which 13 already have
  SAFETY comments and 2 received one in this release.
* **S3 — `HashMap` in 3 ordered contexts.** `engine.rs:1751` is
  point-lookup only; `approval.rs:88::reap_expired` collects keys before
  removal so iteration order doesn't matter; `partition/core_inference.rs`
  doesn't exist.
* **S4 — Sleep-based flaky tests.** The two flagged worst-offenders
  (`orchestration_graph_test.rs:128-136`, `bootstrap_budget_test.rs:77`)
  use sleeps that ARE the workload-under-test or model a poll interval;
  replacing them would invalidate the test.
* **S5 — `engine.rs:454 SystemTime`.** Cross-process Unix-epoch age
  math; `Instant` is the wrong tool because it can't be persisted or
  compared across processes.

### Deferred to v0.6.2

Tracked as follow-ups (non-blocking): T1 threat-test un-ignore (needs
`PluginManifest.actor`); Sp1 parallel tool dispatch; E4 property-test
infrastructure; O1–O4 cost optimisation.

### Rebrand

* Internal source rebrand from `aionrs` to `wayland-core`. All 11 crates renamed (`aion-*` → `wcore-*`), workspace dependencies updated, binary now built as `wayland-core`. Default config file `.aionrs.toml` → `.wcore.toml`, user config dir `~/.aionrs` → `~/.wcore`. Apache-2.0 copyright headers preserved.
* Added: `WCORE_*` env vars and template tokens as primary names — `WCORE_MEMORY_DIR` env var (wcore-memory), `${WCORE_SKILL_DIR}` and `${WCORE_SESSION_ID}` template tokens (wcore-skills). Docs and test fixtures advertise the new names.
* Compat: `AIONRS_MEMORY_DIR`, `${AIONRS_SKILL_DIR}`, and `${AIONRS_SESSION_ID}` remain as backward-compat aliases. They resolve to the same paths/values; pre-rebrand user skills and shell configs continue to work without changes. When both forms are set, the `WCORE_*` form wins.

## [0.1.21](https://github.com/iOfficeAI/aionrs/compare/v0.1.20...v0.1.21) (2026-05-09)


### Bug Fixes

* **deps:** resolve rustls-webpki security vulnerabilities ([#90](https://github.com/iOfficeAI/aionrs/issues/90)) ([b2f46b3](https://github.com/iOfficeAI/aionrs/commit/b2f46b3c7d3463499381b75ac82c5cbb53fb44e9))

## [0.1.20](https://github.com/iOfficeAI/aionrs/compare/v0.1.19...v0.1.20) (2026-05-08)


### Features

* **config:** add project_dir to CliArgs for non-CWD config loading ([#87](https://github.com/iOfficeAI/aionrs/issues/87)) ([f0a5fd7](https://github.com/iOfficeAI/aionrs/commit/f0a5fd7be8582675357ab7994fce96ff4c472004))

## [0.1.19](https://github.com/iOfficeAI/aionrs/compare/v0.1.18...v0.1.19) (2026-05-07)


### Bug Fixes

* **compact:** autocompact token watermark for prefix-caching providers ([#84](https://github.com/iOfficeAI/aionrs/issues/84)) ([581f11d](https://github.com/iOfficeAI/aionrs/commit/581f11d0c7e72c04bfd93dac711ded6e7daf89dc))

## [0.1.18](https://github.com/iOfficeAI/aionrs/compare/v0.1.17...v0.1.18) (2026-04-30)


### Bug Fixes

* **openai:** preserve reasoning_content in multi-turn conversations ([#80](https://github.com/iOfficeAI/aionrs/issues/80)) ([88bdf06](https://github.com/iOfficeAI/aionrs/commit/88bdf061883043a50a21d25e241a4e6eee9623da))

## [0.1.17](https://github.com/iOfficeAI/aionrs/compare/v0.1.16...v0.1.17) (2026-04-29)


### Code Refactoring

* extract ProtocolEmitter trait for backend integration ([#75](https://github.com/iOfficeAI/aionrs/issues/75)) ([b792d74](https://github.com/iOfficeAI/aionrs/commit/b792d74a0171708de4f6c2019f1b3f3864375b0b))

## [0.1.16](https://github.com/iOfficeAI/aionrs/compare/v0.1.15...v0.1.16) (2026-04-26)


### Features

* add AgentBootstrap builder for consistent engine initialization ([#73](https://github.com/iOfficeAI/aionrs/issues/73)) ([a9392ba](https://github.com/iOfficeAI/aionrs/commit/a9392ba353d664c0c8429ea1e7a754e493e9ff29))


### Bug Fixes

* cross-platform shell execution for Windows support ([#70](https://github.com/iOfficeAI/aionrs/issues/70)) ([402d4ff](https://github.com/iOfficeAI/aionrs/commit/402d4ff7311ec47892733dfb79dcd9e83fbfce9c))

## [0.1.15](https://github.com/iOfficeAI/aionrs/compare/v0.1.14...v0.1.15) (2026-04-24)


### Features

* add ping/pong heartbeat protocol support ([#68](https://github.com/iOfficeAI/aionrs/issues/68)) ([20e760b](https://github.com/iOfficeAI/aionrs/commit/20e760b5020525260c5fc10f7211390d96a1be01))

## [0.1.14](https://github.com/iOfficeAI/aionrs/compare/v0.1.13...v0.1.14) (2026-04-23)


### Features

* align maxTurns logic with Claude Code ([#66](https://github.com/iOfficeAI/aionrs/issues/66)) ([d640d88](https://github.com/iOfficeAI/aionrs/commit/d640d88380c7c2e64be1c644e1cf424a1699b8a1))


### Bug Fixes

* UTF-8 panic in tool describe + autocompact skip logging ([#63](https://github.com/iOfficeAI/aionrs/issues/63)) ([c00222d](https://github.com/iOfficeAI/aionrs/commit/c00222d5363c681398dfd1333108ea38fc9eae69))

## [0.1.13](https://github.com/iOfficeAI/aionrs/compare/v0.1.12...v0.1.13) (2026-04-21)


### Features

* hierarchical AGENTS.md loading with [@include](https://github.com/include) support ([#59](https://github.com/iOfficeAI/aionrs/issues/59)) ([3992d52](https://github.com/iOfficeAI/aionrs/commit/3992d5211b87069f11420fc8c7eaa4e8dc0b8214))


### Bug Fixes

* **orchestration:** guide LLM to ToolSearch when deferred tool fails ([#60](https://github.com/iOfficeAI/aionrs/issues/60)) ([a62c8c2](https://github.com/iOfficeAI/aionrs/commit/a62c8c249e45bc56e5bc74bf74f29d43311ede2c))

## [0.1.12](https://github.com/iOfficeAI/aionrs/compare/v0.1.11...v0.1.12) (2026-04-20)


### Features

* add output compaction for tool results ([#54](https://github.com/iOfficeAI/aionrs/issues/54)) ([63130c7](https://github.com/iOfficeAI/aionrs/commit/63130c70ead6dc30fb5244e49515bddc767c3c66))


### Documentation

* sync documentation with v0.1.8–v0.1.12 code changes ([#56](https://github.com/iOfficeAI/aionrs/issues/56)) ([436b09b](https://github.com/iOfficeAI/aionrs/commit/436b09b5a44997b0fb3b679b31ffe608b9f2ebf9))

## [0.1.11](https://github.com/iOfficeAI/aionrs/compare/v0.1.10...v0.1.11) (2026-04-17)


### Features

* **cli:** add team mode support with dynamic MCP server injection ([#50](https://github.com/iOfficeAI/aionrs/issues/50)) ([a16c9ee](https://github.com/iOfficeAI/aionrs/commit/a16c9eed2d64e679f23f18347fc02c423532298b))

## [0.1.10](https://github.com/iOfficeAI/aionrs/compare/v0.1.9...v0.1.10) (2026-04-16)


### Bug Fixes

* **openai:** handle empty function name in SSE deltas + add response dump ([#43](https://github.com/iOfficeAI/aionrs/issues/43)) ([d7ba0fa](https://github.com/iOfficeAI/aionrs/commit/d7ba0fabe48ea2a19cff42d8764ca5ccc1a3d608))

## [0.1.9](https://github.com/iOfficeAI/aionrs/compare/v0.1.8...v0.1.9) (2026-04-15)


### Features

* input token optimization — deferred tools, description truncation, prompt caching ([#41](https://github.com/iOfficeAI/aionrs/issues/41)) ([b20ce58](https://github.com/iOfficeAI/aionrs/commit/b20ce5813c94fa8c6a682ae8d88c7f17f42ec05a))

## [0.1.8](https://github.com/iOfficeAI/aionrs/compare/v0.1.7...v0.1.8) (2026-04-14)


### Features

* agent evolution - memory, compaction, plan mode, tool enhancement & file cache ([#32](https://github.com/iOfficeAI/aionrs/issues/32)) ([0b2a486](https://github.com/iOfficeAI/aionrs/commit/0b2a486e4e921d3b005307675c102a68d4b8f7ed))
* runtime config and capability discovery ([#36](https://github.com/iOfficeAI/aionrs/issues/36)) ([9539b54](https://github.com/iOfficeAI/aionrs/commit/9539b540c64f30ce6afcbfef65a078ab88913f50))


### Bug Fixes

* isolate sub-agent stdout to prevent JSON stream corruption ([#34](https://github.com/iOfficeAI/aionrs/issues/34)) ([6a7584a](https://github.com/iOfficeAI/aionrs/commit/6a7584abe9d0f5c85c36c01858d503cc72d9facd))


### Code Refactoring

* centralize platform-specific paths via app_config_dir() ([ad87748](https://github.com/iOfficeAI/aionrs/commit/ad87748edb299ff488c839630f065ccafc6e28dc))


### Documentation

* refactor AGENTS.md to focus on rules and conventions ([0f81cbc](https://github.com/iOfficeAI/aionrs/commit/0f81cbc644f8c9ed3cbe6af690bc23434feb6c0a))
* update file paths to reflect multi-crate workspace structure ([51b6cc7](https://github.com/iOfficeAI/aionrs/commit/51b6cc7bd29a51af7ad57aa8f87901e85005da42))

## [0.1.7](https://github.com/iOfficeAI/aionrs/compare/v0.1.6...v0.1.7) (2026-04-09)


### Bug Fixes

* **ci:** handle scoped release commit message in release-please workflow ([#22](https://github.com/iOfficeAI/aionrs/issues/22)) ([7222806](https://github.com/iOfficeAI/aionrs/commit/72228064a58d9a8ee410d37ad2380c8f84361cc9))

## [0.1.6](https://github.com/iOfficeAI/aionrs/compare/v0.1.5...v0.1.6) (2026-04-09)


### Bug Fixes

* **ci:** fix release_created typo and update Cargo.lock ([#18](https://github.com/iOfficeAI/aionrs/issues/18)) ([3964963](https://github.com/iOfficeAI/aionrs/commit/3964963f2d45849985c93e5f005cf59e6615573e))

## [0.1.5](https://github.com/iOfficeAI/aionrs/compare/v0.1.4...v0.1.5) (2026-04-09)


### Bug Fixes

* **ci:** fix release workflow to correctly build and upload GitHub Release assets ([#12](https://github.com/iOfficeAI/aionrs/issues/12)) ([997ec18](https://github.com/iOfficeAI/aionrs/commit/997ec18cbbd21ea2ef8eb19ff4cbf6280376a80c))

## [0.1.4](https://github.com/iOfficeAI/aionrs/compare/v0.1.3...v0.1.4) (2026-04-09)


### Bug Fixes

* **ci:** fix action versions and install cargo-audit ([#10](https://github.com/iOfficeAI/aionrs/issues/10)) ([8512765](https://github.com/iOfficeAI/aionrs/commit/85127654ce7afc4ec04b7e6a325d8470e0770175))

## [0.1.3](https://github.com/iOfficeAI/aionrs/compare/v0.1.2...v0.1.3) (2026-04-09)


### Features

* accept optional session ID in SessionManager::create and AgentEngine::init_session ([b5e50e8](https://github.com/iOfficeAI/aionrs/commit/b5e50e82ad8420cf603b3689ea4faba47df988b9))
* add --config-path flag and warn on config parse failure ([2f67ed8](https://github.com/iOfficeAI/aionrs/commit/2f67ed8bff7d1b585a74e112c39f86ebb9a7fba8))
* add --session-id flag and --resume support in json-stream mode ([6ecfa09](https://github.com/iOfficeAI/aionrs/commit/6ecfa094042c2cf1966758a105c7fd76db167516))
* add --version flag support for AionUi integration ([0d32f1f](https://github.com/iOfficeAI/aionrs/commit/0d32f1f21e72ac219eab638c4ca9e2391dd9f42b))
* add ProviderCompat configuration layer (Phase 0.1) ([cc4a315](https://github.com/iOfficeAI/aionrs/commit/cc4a31547283c9cfae1d7c279784e4a2a2e4ffd5))
* add session_id field to Ready protocol event ([f1025b5](https://github.com/iOfficeAI/aionrs/commit/f1025b567dddfd8d92e1afa40ab31fd438a85485))
* Bedrock schema sanitization via compat config (Phase 1.4) ([7802f19](https://github.com/iOfficeAI/aionrs/commit/7802f19709f015f1851a14943a1c2b71d1771f07))
* compat-driven message alternation, merging, and auto tool ID (Phase 1.1, 1.8) ([9bd5b3c](https://github.com/iOfficeAI/aionrs/commit/9bd5b3c692bafdd284a8179b6b29647c2bea381a))
* **compat:** add configurable api_path for chat completions endpoint ([ad8b6e9](https://github.com/iOfficeAI/aionrs/commit/ad8b6e949da7393dfa0d5cecf75449edeba98dbf))
* enhanced Bedrock error messages with actionable hints (Phase 2.1) ([a80d0ff](https://github.com/iOfficeAI/aionrs/commit/a80d0ff03e7c3a3559dc446d21a401b815460e89))
* initial commit of aionrs ([f8f3249](https://github.com/iOfficeAI/aionrs/commit/f8f3249acfcf595a2634d4ba37ae14993d365246))
* integrate ProviderCompat into config system (Phase 0.2) ([c0a4753](https://github.com/iOfficeAI/aionrs/commit/c0a47539eab824aea6c823455df3eafbef6f7016))
* OpenAI compat features - max_tokens field, message merging, orphan cleanup, dedup, strip patterns (Phase 1.2, 1.3, 1.5, 1.6, 1.7) ([c61896d](https://github.com/iOfficeAI/aionrs/commit/c61896d367abbfc31c64a5ae827599a5eee4e558))
* OpenAI reasoning model support (Phase 3.1) ([106108a](https://github.com/iOfficeAI/aionrs/commit/106108a46c06398c3f68d803a8528fad3b43b8d0))
* pass ProviderCompat to all providers (Phase 0.3) ([d9c6e1b](https://github.com/iOfficeAI/aionrs/commit/d9c6e1b0901732666b4fa96b747fe5af53929a99))
* session ID and resume support for JSON stream mode ([d36df5e](https://github.com/iOfficeAI/aionrs/commit/d36df5ed80855ba2fa2fc900a6bee2deda856869))
* skills system - named prompt snippets with tool orchestration ([#5](https://github.com/iOfficeAI/aionrs/issues/5)) ([4a5183f](https://github.com/iOfficeAI/aionrs/commit/4a5183fc7657ad756e751986f8c5c471346642cb))
* support custom provider aliases in configuration ([#2](https://github.com/iOfficeAI/aionrs/issues/2)) ([9fde728](https://github.com/iOfficeAI/aionrs/commit/9fde728f588ae0233179038984551c016d50919d))
* wire up skills system in main.rs and fix symlink traversal ([f93303c](https://github.com/iOfficeAI/aionrs/commit/f93303c9978fce867ee5361b97ee5fb4a4e2e31f))


### Bug Fixes

* **ci:** fix invalid workflow files (matrix.if + YAML syntax) ([#6](https://github.com/iOfficeAI/aionrs/issues/6)) ([4fd6de4](https://github.com/iOfficeAI/aionrs/commit/4fd6de49ad063d5e747b9fcff05d0f29b5535df3))
* **release:** bootstrap release-please for Cargo workspace ([#8](https://github.com/iOfficeAI/aionrs/issues/8)) ([18dd3e3](https://github.com/iOfficeAI/aionrs/commit/18dd3e32213ffabe022f05eaa9b16ec89ad04a76))


### Code Refactoring

* remove Claude branding, use AGENTS.md and AIONRS_* variables ([97dc25c](https://github.com/iOfficeAI/aionrs/commit/97dc25cabd3f4bc15e34d3a42da5dd42120e3bb2))
* split into Cargo workspace with fine-grained crates + CI/E2E ([#3](https://github.com/iOfficeAI/aionrs/issues/3)) ([a4537d9](https://github.com/iOfficeAI/aionrs/commit/a4537d944b3f3643ecb7db58c569f583edea7f97))


### Documentation

* add AGENTS.md with architecture principles and CLAUDE.md reference ([5dfeb89](https://github.com/iOfficeAI/aionrs/commit/5dfeb8990f6ba59363ad8c51eb3ba7738546f56f))
* document --session-id flag and session_id in Ready event ([510c141](https://github.com/iOfficeAI/aionrs/commit/510c141bf93b7a39ba6dda9cef9d9b8a2791f900))
* replace hardcoded ~/.config/aionrs paths with --config-path ([d94d518](https://github.com/iOfficeAI/aionrs/commit/d94d518ea12b0360a9f27e440fb7c0e08b0ffe93))
* update README with ProviderCompat layer and reasoning model support ([c831e21](https://github.com/iOfficeAI/aionrs/commit/c831e2119804f0e5bb2a080f9bef8c5df093dff3))

## Changelog

All notable changes to this project will be documented in this file.

See [Conventional Commits](https://conventionalcommits.org) for commit guidelines.
