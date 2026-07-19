<!-- Refreshed: 2026-07-18 -->
# Codebase Structure

**Analysis Date:** 2026-07-18

## Directory Layout

```text
waylandcore/
├── .cargo/                         # Cargo configuration and audit policy
├── .config/                        # Tool configuration, including nextest
├── .planning/                      # GSD-generated planning and codebase maps
├── .superpowers/                   # Local workflow state; ignored by Git
├── crates/                         # Rust workspace crates
│   ├── wcore-types/                # Provider-neutral shared data types
│   ├── wcore-compact/              # Context folding and token management
│   ├── wcore-config/               # Configuration, compatibility, shell helpers
│   ├── wcore-protocol/             # JSON stream commands, events, and I/O
│   ├── wcore-plugin-api/           # Stable plugin-facing contracts
│   ├── wcore-providers/            # Built-in LLM provider implementations
│   ├── wcore-tools/                # Built-in agent tools and registry
│   ├── wcore-mcp/                  # Model Context Protocol client
│   ├── wcore-skills/               # Skill discovery, activation, and execution
│   ├── wcore-memory/               # Persistent cross-session memory
│   ├── wcore-agent/                # Agent engine and orchestration
│   ├── wcore-cli/                  # Main CLI, TUI, and JSON-stream host
│   ├── wcore-channel-*/            # Individual external channel adapters
│   ├── wcore-*/                    # Other policy, runtime, and verification crates
│   └── wayland-*/                  # Loadable first-party plugins
├── docs/                           # User, architecture, protocol, and security docs
├── examples/                       # Standalone example projects, excluded from workspace
├── npm/                            # npm packaging/generation support
├── scripts/                        # Release, CI, packaging, and maintenance scripts
├── templates/                      # Project and service templates, excluded from workspace
├── tests/                          # Workspace-level integration and policy tests
├── workspace-hack/                 # cargo-hakari feature-unification crate
├── AGENTS.md                       # Repository operating and architecture rules
├── Cargo.toml                      # Workspace membership and shared dependencies
├── Cargo.lock                      # Locked Rust dependency graph
├── Cross.toml                      # Cross-compilation configuration
├── clippy.toml                     # Rust lint configuration
├── deny.toml                       # Dependency policy configuration
├── justfile                        # Deterministic development and release tasks
├── rust-toolchain.toml             # Pinned Rust toolchain
├── vx.toml                         # Pinned tool launcher configuration
└── vx.lock                         # Locked vx tool versions
```

## Directory Purposes

**`crates/`:** Cargo workspace implementation. Each child is independently packaged and normally contains `Cargo.toml`, `src/`, and optional `tests/`, `benches/`, `data/`, `contracts/`, or `fixtures/` directories.

The foundation and policy layer contains:

- `crates/wcore-types/` — provider-neutral messages, requests, events, tools, usage, and shared identifiers. It is the lowest common dependency.
- `crates/wcore-compact/` — context sanitization, folding, token estimation, and compression algorithms.
- `crates/wcore-config/` — configuration resolution, provider compatibility presets, authentication configuration, and centralized cross-platform shell helpers.
- `crates/wcore-protocol/` — JSON Lines host protocol types, bounded input, output emission, approval coordination, and checked-in protocol contracts.
- `crates/wcore-plugin-api/` — plugin traits, registration mirrors, and the dependency-isolation boundary used by first-party and third-party plugins.
- `crates/wcore-egress/`, `crates/wcore-permissions/`, `crates/wcore-safety/`, and `crates/wcore-sandbox/` — outbound-network, authorization, safety, and process-containment policy.

The runtime and capability layer contains:

- `crates/wcore-agent/` — the composition root, agent loop, session lifecycle, execution graphs, workflows, fleets, delegation, and runtime policy enforcement.
- `crates/wcore-providers/` — Anthropic, OpenAI-compatible, Bedrock, Vertex, and related provider adapters behind `LlmProvider`.
- `crates/wcore-tools/` — tool trait, registry, and built-in Read, Write, Edit, Bash, Grep, Glob, Spawn, Git, and supporting tools.
- `crates/wcore-mcp/` — MCP transports, server discovery, tool bridging, and lifecycle.
- `crates/wcore-skills/` — skill loading, conditional activation, permissions, hooks, and shell expansion.
- `crates/wcore-memory/` — persistent memory stores, extraction, retrieval, consolidation, and project/user context.
- `crates/wcore-observability/` — trace schema, span sinks, OTLP export, and prompt-cache observability.
- `crates/wcore-repomap/` — isolated repository symbol extraction and indexing with no internal `wcore-*` dependency.
- `crates/wcore-browser/` and `crates/wcore-cua/` — browser automation and cross-platform computer-use backends.
- `crates/wcore-budget/`, `crates/wcore-pricing/`, and `crates/wcore-user-model/` — cost controls, pricing data, and user-model behavior.

The host and orchestration layer contains:

- `crates/wcore-cli/` — executable entry point, argument parsing, interactive TUI, one-shot operation, JSON-stream server, and host command dispatch.
- `crates/wcore-acp/` — Agent Client Protocol integration.
- `crates/wcore-agents-pack/`, `crates/wcore-dispatch/`, `crates/wcore-swarm/`, and `crates/wcore-cron/` — packaged agents, dispatch, multi-agent execution, and scheduled work.
- `crates/wcore-channels/` and `crates/wcore-channels-registry/` — common channel contracts and runtime registration.
- `crates/wcore-channel-discord/`, `crates/wcore-channel-email/`, `crates/wcore-channel-imessage/`, `crates/wcore-channel-matrix/`, `crates/wcore-channel-msteams/`, `crates/wcore-channel-signal/`, `crates/wcore-channel-slack/`, `crates/wcore-channel-sms/`, `crates/wcore-channel-telegram/`, and `crates/wcore-channel-whatsapp/` — external transport adapters.

Plugin implementations are separated from the core spine:

- `crates/wayland-browser/` and `crates/wayland-cua/` — plugin packages that expose browser and CUA capabilities through mirror types in `wcore-plugin-api`.
- `crates/wayland-ollama/` — Ollama local-inference provider plugin.
- `crates/wayland-ijfw/` — IJFW anchor plugin exercising plugin registration surfaces.
- `crates/wayland-honcho/` and `crates/wcore-honcho-adapter/` — Honcho plugin and adapter integration.
- `crates/wcore-plugin-wasm/`, `crates/wcore-plugin-subprocess/`, and `crates/wcore-pluginsrc/` — alternate plugin runtimes and plugin-source management.

Verification and evolution crates are kept distinct from the production engine:

- `crates/wcore-eval/` and `crates/wcore-eval-scenarios/` — acceptance gates, scenario runners, evidence, and threshold enforcement.
- `crates/wcore-evolve/` — GEPA generation, scoring, mutation, and retention loop.
- `crates/wcore-replay/` and `crates/wcore-fixture-harness/` — deterministic replay and fixture-driven verification support.

**`docs/`:** Checked-in documentation. Top-level references such as `docs/architecture.md`, `docs/providers.md`, `docs/tools.md`, `docs/skills.md`, `docs/mcp.md`, `docs/advanced.md`, and `docs/json-stream-protocol.md` describe the public system. Design, audit, security, and migration material lives in named subdirectories or topic files under this directory.

**`examples/`:** Standalone example crates and configurations. Root `Cargo.toml` excludes `examples/**`, so examples do not automatically participate in workspace builds.

**`templates/`:** Checked-in project/service templates. Root `Cargo.toml` excludes `templates/**`; treat their manifests and generated source independently from workspace members.

**`tests/`:** Cross-crate tests for workspace policies and end-to-end behavior that do not belong to one crate's public integration-test surface.

**`scripts/`:** Operational automation for CI, release, packaging, installation, and maintenance. Inspect the script and its callers before changing it because these paths often encode platform and distribution assumptions.

**`npm/`:** Source used to generate or package the npm distribution layer; it is not a Rust workspace crate.

**`workspace-hack/`:** `cargo-hakari`-managed feature-unification crate. Its manifest and source explicitly require regeneration via `cargo hakari generate` rather than manual edits.

**`.planning/`:** Generated GSD project state and codebase maps. It is ignored by Git in the current repository and is not runtime source.

**`.cargo/` and `.config/`:** Tool-specific configuration that applies across crates, including Cargo and nextest behavior.

## Key File Locations

**Entry Points:**

- `crates/wcore-cli/src/main.rs` — primary `wayland` executable; selects one-shot, TUI, JSON stream, bootstrap, and utility subcommands.
- `crates/wcore-cli/src/lib.rs` — reusable CLI-facing modules exposed to the executable and tests.
- `crates/wcore-eval/src/bin/` — evaluation command entry points.
- `crates/wcore-eval-scenarios/src/bin/` — scenario-runner entry points.
- `crates/wcore-evolve/src/bin/` — evolution-loop command entry points.
- `crates/wayland-*/src/lib.rs` — first-party plugin library entry points.

**Configuration:**

- `Cargo.toml` — authoritative workspace member list, dependency versions, profiles, lints, and package metadata.
- `Cargo.lock` — resolved Rust dependencies; update only through Cargo workflows.
- `rust-toolchain.toml` — pinned compiler/toolchain channel.
- `vx.toml` and `vx.lock` — deterministic tool launcher declarations and lock state.
- `justfile` — repository-standard lint, format, test, release, and push tasks.
- `.cargo/config.toml` — shared Cargo configuration.
- `.config/nextest.toml` — nextest execution profiles.
- `Cross.toml` — cross-compilation target configuration.
- `clippy.toml` and `deny.toml` — lint and dependency-policy settings.
- `crates/wcore-config/src/config.rs` — runtime configuration model and resolution logic.
- `crates/wcore-config/src/compat.rs` — provider-specific behavior expressed as `ProviderCompat` data instead of provider-name conditionals.
- `AGENTS.md` — repository constraints, crate map, verification rules, and platform boundaries.

**Core Logic:**

- `crates/wcore-agent/src/bootstrap.rs` — runtime composition root that binds providers, tools, plugins, MCP, skills, memory, channels, policies, and the engine.
- `crates/wcore-agent/src/engine.rs` — primary agent loop, provider streaming, tool-use handling, session updates, and graph execution.
- `crates/wcore-agent/src/orchestration/` — workflows, execution graphs, fleets, spawning, and higher-level orchestration.
- `crates/wcore-agent/src/session.rs` — active session and session-manager lifecycle.
- `crates/wcore-agent/src/session_journal.rs` — durable session journal models, append/recovery behavior, leases, snapshots, and reduction.
- `crates/wcore-tools/src/lib.rs` and `crates/wcore-tools/src/registry.rs` — built-in tool surface and lookup/dispatch registry.
- `crates/wcore-providers/src/lib.rs` — `LlmProvider` abstraction and provider construction.
- `crates/wcore-protocol/src/commands.rs` and `crates/wcore-protocol/src/events.rs` — host-to-core commands and core-to-host events.
- `crates/wcore-protocol/src/reader.rs` and `crates/wcore-protocol/src/writer.rs` — bounded JSON input and serialized event output.
- `crates/wcore-plugin-api/src/lib.rs` — plugin-facing API boundary and exported registration contracts.
- `crates/wcore-config/src/shell.rs` — centralized cross-platform process construction; use argv mode for attacker-controlled arguments.

**Testing:**

- `crates/<crate>/tests/` — public integration tests for an individual crate.
- Inline `#[cfg(test)]` modules under `crates/<crate>/src/` — unit tests for module internals.
- `tests/` — workspace-wide integration, policy, and compatibility tests.
- `crates/wcore-protocol/contracts/` — checked-in protocol contract fixtures that protect host compatibility.
- `crates/wcore-eval/data/` and `crates/wcore-eval-scenarios/` — evaluation definitions, expected evidence, and scenario verification.
- `crates/wcore-fixture-harness/` and `crates/wcore-replay/` — reusable deterministic test infrastructure.
- `crates/*/benches/` — benchmarks owned by the crate containing the measured code.

## Naming Conventions

**Files:**

- Rust modules use `snake_case.rs`; multi-file modules use a matching `snake_case/` directory with `mod.rs` or an explicitly declared root module.
- Library and binary roots use Cargo conventions: `src/lib.rs`, `src/main.rs`, and `src/bin/<name>.rs`.
- Integration tests use descriptive `snake_case.rs` names under `tests/`; several suites use a `_test.rs` suffix where that is already the local pattern.
- Public Rust types and traits use `PascalCase`; functions, methods, fields, variables, and modules use `snake_case`; constants use `SCREAMING_SNAKE_CASE`.
- Documentation uses descriptive lowercase kebab-case filenames such as `json-stream-protocol.md`; established uppercase audit/release artifacts retain their existing convention.

**Directories:**

- Workspace crates use kebab-case Cargo package names. Core crates start with `wcore-`; first-party loadable plugins start with `wayland-`.
- Rust module directories use `snake_case` to match module names.
- Each crate owns its tests and fixtures when they validate that crate; only truly cross-workspace checks belong in root `tests/`.
- New provider, channel, or plugin code should follow the existing neighboring crate/module naming rather than introducing a parallel taxonomy.

## Where to Add New Code

**New Feature:**

- Add provider-neutral data first in `crates/wcore-types/` only when multiple higher layers need it; otherwise keep the type in the lowest owning crate.
- Add a built-in LLM provider or request/response translation in `crates/wcore-providers/`. Express provider differences in `crates/wcore-config/src/compat.rs`, then cover both the preset and serialized request behavior with tests.
- Add a built-in tool in `crates/wcore-tools/src/`, register it through the existing registry/bootstrap path, and place public behavior tests in `crates/wcore-tools/tests/`.
- Add agent-loop, delegation, graph, fleet, or workflow behavior in `crates/wcore-agent/src/` under the matching domain module. Keep provider-format conversion out of this crate.
- Add a host command or event in `crates/wcore-protocol/src/commands.rs` or `events.rs`, update the relevant checked-in contracts, then wire dispatch/emission in `crates/wcore-cli/src/main.rs` and host adapters that consume it.
- Add runtime configuration in `crates/wcore-config/`; centralize any new operating-system behavior there rather than scattering `cfg` or raw shell selection across call sites.
- Add memory, skill, MCP, browser, CUA, sandbox, pricing, or observability behavior to its existing dedicated `wcore-*` crate and integrate it in `wcore-agent` bootstrap only where composition is required.

**New Component/Module:**

- Prefer a focused module inside the existing owning crate. Create a new workspace crate only when the component has an independent dependency boundary, public contract, and more than a single shared helper.
- Add a new external channel as `crates/wcore-channel-<service>/`, implement the contracts from `crates/wcore-channels/`, and register it through `crates/wcore-channels-registry/` and the agent bootstrap path.
- Add a first-party plugin as `crates/wayland-<name>/`. Depend on mirror contracts in `wcore-plugin-api`; do not make plugin crates reach upward into browser, CUA, agent, or CLI internals.
- Add plugin host bindings in `crates/wcore-agent/` only when a mirror contract must be connected to a concrete core backend without breaking plugin isolation.
- Add verification binaries, scenarios, or acceptance gates to `wcore-eval`, `wcore-eval-scenarios`, `wcore-replay`, or `wcore-fixture-harness` according to whether the artifact is a gate, scenario, replay, or fixture.

**Utilities:**

- Put cross-platform paths, process launching, permissions, and environment handling in `crates/wcore-config/` when they are configuration/platform concerns.
- Put provider-neutral shared model types in `crates/wcore-types/` and context-compression helpers in `crates/wcore-compact/`.
- Keep a utility local to its single caller's crate until a second real consumer establishes a shared responsibility; do not create a crate or public abstraction for speculative reuse.
- Put operational automation in `scripts/` and deterministic developer task composition in `justfile`; do not embed release operations in product code.

## Special Directories

**`.planning/`:** Generated by GSD workflows. It is ignored by Git and contains planning state rather than product code. Generated: Yes. Committed: No.

**`.superpowers/`:** Local workflow/runtime state. It is ignored by Git. Generated: Yes. Committed: No.

**`target/`:** Cargo build output when builds are run. It is ignored and must never be treated as source evidence. Generated: Yes. Committed: No.

**`workspace-hack/`:** Feature-unification crate managed by `cargo-hakari`. Regenerate its manifest/source through `cargo hakari generate`; do not edit generated dependency content manually. Generated: Yes. Committed: Yes.

**`crates/wcore-protocol/contracts/`:** Versioned compatibility contracts for the JSON-stream surface. Update deliberately with protocol changes. Generated: No. Committed: Yes.

**`crates/wcore-eval/data/` and scenario fixture directories:** Versioned evaluation inputs and expected evidence. Treat them as executable acceptance artifacts, not scratch output. Generated: No. Committed: Yes.

**`templates/` and `examples/`:** Checked-in source artifacts intentionally excluded from the root Cargo workspace. Changes require their own validation because normal workspace commands do not prove them. Generated: No. Committed: Yes.

**`docs/`:** Checked-in public and internal documentation. Architecture claims should be reconciled with current code and manifests before being repeated. Generated: No. Committed: Yes.

---

*Structure analysis: 2026-07-18*
