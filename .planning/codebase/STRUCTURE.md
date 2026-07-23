# Codebase Structure

**Analysis Date:** 2026-07-23

## Directory Layout

```
waylandcore-ferrox/
├── crates/                    # Cargo workspace members (see AGENTS.md crate map)
│   ├── wcore-types/           # Bottom: provider-neutral shared types, zero internal deps
│   ├── wcore-compact/         # Context compression (folding, sanitization, tokenization)
│   ├── wcore-config/          # Config cascade, ProviderCompat, auth, hooks, shell helpers
│   ├── wcore-protocol/        # JSON stream protocol (events/commands/approval)
│   ├── wcore-egress/          # Single outbound-HTTP chokepoint (EgressClient)
│   ├── wcore-plugin-api/      # Plugin trait, PluginContext, scoped registries — isolation boundary
│   ├── wcore-pluginsrc/       # Marketplace: install-time acquisition + lowering of foreign plugin formats
│   ├── wcore-plugin-wasm/     # WASM Component Model plugin host (wasmtime + WASI)
│   ├── wcore-plugin-subprocess/ # Subprocess plugin host (JSON-RPC over stdio)
│   ├── wcore-providers/       # LlmProvider trait + Anthropic/OpenAI/Bedrock/Vertex impls
│   ├── wcore-pricing/         # Pricing-as-data catalog (provider x model token rates)
│   ├── wcore-tools/           # Built-in agent tools (Read, Write, Edit, Bash, Grep, Glob, Spawn)
│   ├── wcore-user-model/      # User-model backend abstraction (preferences, expertise, style)
│   ├── wcore-honcho-adapter/  # Honcho adapter for the user-model backend trait
│   ├── wcore-acp/             # Agent Client Protocol support types
│   ├── wcore-mcp/             # MCP (Model Context Protocol) client
│   ├── wcore-skills/          # Skills system (prompt snippets, hooks, permissions)
│   ├── wcore-eval/            # Acceptance/evaluation gate runner
│   ├── wcore-eval-scenarios/  # Scenario-level eval harness against real binary + LLM APIs
│   ├── wcore-evolve/          # GEPA evolution loop (child generation, scoring, retention)
│   ├── wcore-replay/          # Session-trace replay + diff for debugging
│   ├── wcore-budget/          # Budget caps + telemetry
│   ├── wcore-sandbox/         # Container/OS-isolated tool execution (bwrap/sandbox-exec/AppContainer/Docker)
│   ├── wcore-memory/          # Long-term cross-session memory
│   ├── wcore-permissions/     # Multi-actor ACL + bearer token (Team Mode foundation)
│   ├── wcore-observability/   # Trace schema, span sinks, OTLP exporter, prompt-cache discipline
│   ├── wcore-repomap/         # Aider-style symbol extractor + codebase index (NO internal wcore-* deps)
│   ├── wcore-agent/           # TOP: engine, session, orchestration (Anvil/Council/Workflow)
│   ├── wcore-agents-pack/     # Built-in agent pack (embedded TOML manifests for --agent=<name>)
│   ├── wcore-dispatch/        # Generic DecisionRouter trait + Thompson-sampling scorer
│   ├── wcore-channels/        # Channel runtime — trait + lifecycle + config loader
│   ├── wcore-channel-slack/   # Production Slack adapter
│   ├── wcore-channel-telegram/# Production Telegram Bot API adapter
│   ├── wcore-channel-discord/ # Production Discord adapter (REST + Gateway WebSocket)
│   ├── wcore-channel-sms/     # Twilio SMS adapter
│   ├── wcore-channel-email/   # SMTP/IMAP email adapter
│   ├── wcore-channel-whatsapp/# WhatsApp Cloud API adapter
│   ├── wcore-channel-signal/  # signal-cli subprocess adapter
│   ├── wcore-channel-imessage/# macOS-only AppleScript/osascript adapter
│   ├── wcore-channel-matrix/  # Matrix send-only MVP adapter
│   ├── wcore-channel-msteams/ # MS Teams Bot Framework adapter
│   ├── wcore-channels-registry/# Per-platform factory dispatch + on-disk auto-registration
│   ├── wcore-cli/             # TOP: CLI binary entry point
│   ├── wcore-browser/         # Multi-backend browser tool family (Camoufox/chromiumoxide/Browserbase)
│   ├── wcore-cua/             # Multi-platform computer-use tool family
│   ├── wcore-swarm/           # Worktree-isolated multi-agent dispatch (fleet, consensus, mesh)
│   ├── wcore-cron/            # Memory-resident scheduled triggers (slash/channel/skill targets)
│   ├── wcore-safety/          # Output validator + PII scrubber primitives
│   ├── wcore-fixture-harness/ # Customer-fixture catalog + replay harness
│   ├── wayland-ollama/        # PLUGIN: Ollama local-inference provider
│   ├── wayland-browser/       # PLUGIN: BrowserToolSpec mirror (no wcore-browser dep)
│   ├── wayland-cua/           # PLUGIN: CuaToolSpec mirror (no wcore-cua dep)
│   ├── wayland-ijfw/          # PLUGIN: anchor plugin exercising every register_* surface
│   ├── wayland-honcho/        # PLUGIN: Honcho user-model plugin shell
│   └── workspace-hack/        # cargo-hakari generated feature-unification crate
├── docs/                      # Design docs, subsystem references (providers.md, tools.md, workflows.md, ...)
├── examples/                  # Standalone plugin examples
├── templates/                 # `cargo generate` scaffolds (.liquid files)
├── scripts/                   # Dev/CI helper scripts
├── tests/                     # Workspace-level integration/e2e tests
├── npm/                       # npm packaging for distribution
├── .planning/                 # Ferrox planning artifacts (this document lives here)
├── .github/                   # CI workflows
├── Cargo.toml                 # Workspace manifest (member list + workspace deps)
├── Cargo.lock
├── justfile                   # `just push` = lint-fix → fmt → auto-commit-fixes → test → git push
├── vx.toml / vx.lock          # Pinned toolchain versions (Rust + just) via `vx`
├── clippy.toml / deny.toml    # Lint config, dependency audit config
├── rust-toolchain.toml        # Rust toolchain pin
└── AGENTS.md / CLAUDE.md       # Operating instructions (AGENTS.md canonical, CLAUDE.md imports it)
```

## Directory Purposes

**`crates/wcore-agent/src/`:**
- Purpose: the engine — turn loop, session management, spawning, orchestration
- Contains: `engine.rs` (1.2MB, turn-loop core — largest file in the repo), `session.rs`, `spawner.rs`/`spawner/`, `session_journal.rs`/`session_journal/` (durable event log), `child_transaction.rs`/`child_transaction/` (transactional child mutation lifecycle), `bootstrap.rs`, `hooks/`, `plan/`, `plugins/`, `oauth/`, `slash/`, `output/`, `tool_backends/`, `compact/`, `agents/`, `auto_skill/`
- Key files: `crates/wcore-agent/src/engine.rs`, `crates/wcore-agent/src/session_journal.rs`, `crates/wcore-agent/src/spawner.rs`, `crates/wcore-agent/src/orchestration/mod.rs`

**`crates/wcore-agent/src/orchestration/`:**
- Purpose: hosts the three orchestration subsystems that coordinate multi-step/multi-agent work
- Contains:
  - `anvil/` — the native gated-forge engine: `climb.rs` (decision core), `detect.rs` (gate auto-detection), `engine.rs` (climb loop over injected seams), `forge.rs` (production wiring: sandbox gate + spawn builder + `drive_climb_full`), `gate_authorization.rs` (Anvil gate → parent-owned gate authorization translation), `gates.rs` (gate closure pinning, probe, injection fencing, flake policy), `journal.rs` (append-only climb journal), `landing.rs` (production landing orchestrator: winner → open → accept → land), `lease.rs` (per-workspace climb lease), `ledger.rs` (per-task cost ledger), `seat.rs` (driver-seat materialization), `tool.rs` (session-level Forge tool)
  - `council/` — multi-advisor proposal orchestration: `advisor.rs`, `aggregator.rs`, `assembler.rs`/`assembler_log.rs`, `driver.rs`, `gate.rs`, `plan_card.rs`, `proposal.rs`, `resolver.rs`, `roster.rs`, `run.rs`, `spend.rs`
  - `workflow/` — ForgeFlows (Dynamic Workflows) declarative RON engine: `dsl.rs`, `schema.rs`, `pipeline.rs`, `runner.rs`, `limits.rs`, `estimate.rs`, `meta.rs`, `error.rs`
  - `graph.rs` — the legacy/test-only per-turn `ExecutionGraph` walker (distinct from `WorkflowRunner`)
  - `intent.rs`, `monitor.rs`, `node_executor.rs`, `template_routing.rs`, `templates.rs`, `f13_durability_tests.rs`
- Key files: `crates/wcore-agent/src/orchestration/anvil/forge.rs`, `crates/wcore-agent/src/orchestration/anvil/landing.rs`, `crates/wcore-agent/src/orchestration/workflow/runner.rs`

**`crates/wcore-sandbox/src/`:**
- Purpose: cross-platform process-isolated tool execution + containment authority
- Contains: `lib.rs` (registry, `HardContainmentAuthority` mint/consume), `backends/` (per-platform: `bwrap.rs`/`bwrap_landlock.rs`/`bwrap_seccomp.rs` for Linux, `sandbox_exec.rs` for macOS, `appcontainer.rs`/`appcontainer/` for Windows including `acl_lease/` and `windows_impl/`, `docker.rs` opt-in, `no_sandbox.rs`, `process_tree.rs`), `directory_authority.rs`/`directory_authority_file.rs`/`directory_authority_archive.rs`/`directory_authority_windows.rs` (filesystem authority abstraction), `manifest.rs` (`SandboxManifest`, policies), `process_capture.rs`, `error.rs`, `bin/il_probe.rs`
- Key files: `crates/wcore-sandbox/src/lib.rs`, `crates/wcore-sandbox/src/backends/mod.rs`

**`crates/wcore-swarm/src/`:**
- Purpose: worktree-isolated multi-agent dispatch — the substrate Anvil/Council candidates execute inside
- Contains: `worktree.rs`/`worktree/` (`candidate.rs` — `CandidateSeal`; `parent.rs` — parent checkout authority), `worktree_manager.rs`, `worktree_cleanup.rs`, `worktree_security.rs`, `fleet.rs` (fleet dispatch), `dispatch.rs`, `consensus.rs`, `debate.rs`, `mesh.rs`, `topology.rs`, `bridge.rs`, `collect.rs`, `reduce.rs`, `scorer.rs`, `heartbeat.rs`, `audit.rs`
- Key files: `crates/wcore-swarm/src/worktree/candidate.rs`, `crates/wcore-swarm/src/worktree/parent.rs`, `crates/wcore-swarm/src/fleet.rs`

**`crates/wcore-cli/src/`:**
- Purpose: CLI binary entry point and all user-facing subcommands
- Contains: `main.rs` (arg parsing + wiring, 318.7K), `lib.rs`, `anvil.rs` (Anvil CLI verb), `swarm.rs`, `cron.rs`, `workflow.rs`, `crucible.rs`, `acp.rs`/`acp_engine.rs`/`acp_roster.rs` (Agent Client Protocol), `auth.rs`, `provider_keys.rs`, `profile.rs`/`profile_router.rs`, `self_update.rs`, `runtime_diagnostics.rs`, `crash_sentinel.rs`, `mcp_serve.rs`, `init.rs`, `agent_cmd.rs`, `budget_grants.rs`, `attachments.rs`, `fetch.rs`, `image.rs`, subdirectories `doctor/`, `migrate/`, `plugin/`, `tui/`
- Key files: `crates/wcore-cli/src/main.rs`, `crates/wcore-cli/src/anvil.rs`

**`docs/`:**
- Purpose: hand-maintained subsystem reference docs
- Contains: `getting-started.md`, `providers.md`, `tools.md`, `skills.md`, `mcp.md`, `advanced.md`, `json-stream-protocol.md`, `troubleshooting.md`, `workflows.md`, `design/` (dated design docs, e.g. `2026-07-12-anvil-native-gated-forge-design.md`)

## Key File Locations

**Entry Points:**
- `crates/wcore-cli/src/main.rs`: CLI binary entry point
- `crates/wcore-cli/src/anvil.rs`: Anvil CLI verb entry
- `crates/wcore-agent/src/orchestration/anvil/tool.rs`: in-session Forge tool entry
- `crates/wcore-cli/src/acp.rs`: Agent Client Protocol entry surface

**Configuration:**
- `Cargo.toml`: workspace member list (with inline dated comments explaining why each crate exists — treat these comments as authoritative provenance)
- `crates/wcore-config/src/`: runtime config cascade, `ProviderCompat`
- `rust-toolchain.toml`, `vx.toml`/`vx.lock`: toolchain pins

**Core Logic:**
- `crates/wcore-agent/src/engine.rs`: turn loop core
- `crates/wcore-agent/src/session_journal.rs`: durable event log
- `crates/wcore-agent/src/orchestration/anvil/forge.rs`: gated-forge production wiring, `drive_climb_full`
- `crates/wcore-agent/src/orchestration/anvil/landing.rs`: parent CAS landing + rollback
- `crates/wcore-sandbox/src/lib.rs`: containment authority mint/consume

**Testing:**
- Inline `#[cfg(test)]` modules per file (e.g. `crates/wcore-sandbox/src/backends/appcontainer/acl_lease/tests.rs`, `windows_impl/tests.rs`, `docker_tests.rs`)
- `crates/<crate>/tests/`: integration tests (e.g. `crates/wcore-agent/tests/orchestration_test.rs`, `workflow_runner_test.rs`, `dangerous_lease_e2e_test.rs`, `hooks_test.rs`, `json_stream_approval_test.rs`)
- `crates/wcore-swarm/src/worktree_tests.rs`/`worktree_tests/linux.rs`: platform-conditional worktree tests
- `crates/wcore-fixture-harness/`: customer-fixture catalog + replay harness
- `crates/wcore-eval-scenarios/`: scenario-level eval harness against the real binary + real LLM APIs

## Naming Conventions

**Crates:**
- `wcore-*` prefix for internal library crates (bottom/mid/top layers)
- `wayland-*` prefix for plugin crates that go through `wcore-plugin-api` mirror types
- `workspace-hack` is the sole exception (cargo-hakari generated)

**Files:**
- Snake_case module files matching their primary type/concern (e.g. `session_journal.rs`, `child_transaction.rs`)
- When a single file exceeds ~1000 lines it grows a same-named directory (e.g. `session_journal.rs` + `session_journal/`, `spawner.rs` + `spawner/`, `child_transaction.rs` + `child_transaction/`) with submodules split by responsibility (e.g. `session_journal/model.rs`, `session_journal/lease.rs`)
- Test files: `*_test.rs` or `*_tests.rs` suffix in `tests/`; inline `#[cfg(test)] mod tests` or a dedicated `tests.rs` sibling within `src/`

**Directories:**
- Organized by domain responsibility (e.g. `orchestration/anvil/`, `orchestration/council/`, `orchestration/workflow/`), not by type (no generic `models/`/`utils/` catch-alls at the orchestration level)
- Per-platform backend code lives under `backends/` with one file per platform/mechanism (e.g. `bwrap.rs`, `sandbox_exec.rs`, `appcontainer.rs`, `docker.rs`)

**Types:**
- PascalCase struct/enum/trait names describing the domain concept directly (e.g. `HardContainmentAuthority`, `CandidateSeal`, `TerminalState`, `WinnerLandingRequest`)
- Error enums named `<Domain>Error` (e.g. `ForgeError`, `WinnerLandingError`, `SandboxError`, `JournalError`)

## Where to Add New Code

**New orchestration subsystem (sibling of Anvil/Council/Workflow):**
- Add a new module under `crates/wcore-agent/src/orchestration/<name>/` with its own `mod.rs`; wire it into `crates/wcore-agent/src/orchestration/mod.rs`
- Tests: `crates/wcore-agent/tests/<name>_test.rs` for integration, inline `#[cfg(test)]` for unit tests

**New Anvil piece (gate type, landing behavior, etc.):**
- Extend the relevant existing file under `crates/wcore-agent/src/orchestration/anvil/` (`gates.rs` for gate logic, `landing.rs` for landing/CAS behavior, `forge.rs` for spawn wiring) rather than creating a new crate
- Any new sandbox containment primitive belongs in `crates/wcore-sandbox`, not in `wcore-agent`

**New LLM provider:**
- Add an implementation crate module under `crates/wcore-providers/src/` implementing `LlmProvider` (`crates/wcore-providers/src/lib.rs:135`)
- Provider-specific quirks go into `ProviderCompat` (`wcore-config`), never hardcoded in the provider file — see AGENTS.md §"No Hardcoded Provider Quirks"

**New built-in tool:**
- Add to `crates/wcore-tools/src/` implementing the `Tool` trait (`crates/wcore-tools/src/lib.rs:319`); register with the `ToolDispatcher`
- If the tool needs process isolation, delegate execution to `wcore-sandbox::SandboxBackend` rather than spawning directly

**New chat channel adapter:**
- Add a new `crates/wcore-channel-<platform>/` crate implementing the `Channel` trait from `wcore-channels`; register via `wcore-channels-registry`'s on-disk auto-registration

**New sandbox backend (new platform/mechanism):**
- Add under `crates/wcore-sandbox/src/backends/<name>.rs` implementing `SandboxBackend`; wire selection into `default_for_platform` (`crates/wcore-sandbox/src/backends/mod.rs`) behind the appropriate `cfg`

**Plugin-facing capability:**
- Extend `crates/wcore-plugin-api` mirror types first, then implement the real capability in the corresponding `wcore-*` crate and bind it in `wcore-agent` (see `HostBrowserRegistrar`/`HostCuaRegistrar` pattern) — never add a direct `wcore-browser`/`wcore-cua`/`wcore-sandbox` dependency to a `wayland-*` plugin crate (audit F2)

**Utilities:**
- Shared cross-platform helpers (paths, shell execution) belong in `wcore-config` (already hosts `wcore_config::shell`)
- If functionality is needed by 2+ crates, extract to the lowest existing crate in the dependency graph where it semantically belongs — never duplicate across crates, never create a new crate for one shared function

## Special Directories

**`workspace-hack/`:**
- Purpose: cargo-hakari generated crate that unifies feature flags across the workspace to speed up builds
- Generated: Yes (regenerated by `cargo hakari generate`)
- Committed: Yes

**`.planning/`:**
- Purpose: Ferrox planning artifacts (phase plans, codebase maps — this document's home)
- Generated: Partially (docs like this one are generated by mapper agents; phase plans are human/agent-authored)
- Committed: Yes

**`templates/`:**
- Purpose: `cargo generate` scaffolds (`.liquid` files that intentionally do not parse as Rust)
- Generated: No (hand-authored templates)
- Committed: Yes

**`examples/`:**
- Purpose: standalone plugin examples, kept out of the main crate graph
- Generated: No
- Committed: Yes

**`.github/`:**
- Purpose: CI workflow definitions
- Generated: No
- Committed: Yes

---

*Structure analysis: 2026-07-23*
