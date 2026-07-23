<!-- refreshed: 2026-07-23 -->
# Architecture

**Analysis Date:** 2026-07-23

## System Overview

```text
┌─────────────────────────────────────────────────────────────────────┐
│  wcore-cli — CLI binary (`main.rs`), TUI, slash commands, anvil verb │
│  `crates/wcore-cli/src`                                              │
└───────────────────────────────┬──────────────────────────────────────┘
                                 │
┌───────────────────────────────▼──────────────────────────────────────┐
│  wcore-agent — Engine, Session, Spawner, Orchestration                │
│  `crates/wcore-agent/src/engine.rs` (1.2MB), `session.rs`,           │
│  `spawner.rs`, `bootstrap.rs`, `orchestration/`                      │
│  Hosts: Anvil (gated forge), Council, Workflow (ForgeFlows/DSL),     │
│  SessionJournal, ChildTransaction, DurableSpawner                    │
└──┬──────────┬───────────┬────────────┬────────────┬──────────────────┘
   │          │           │            │            │
   ▼          ▼           ▼            ▼            ▼
┌──────┐ ┌─────────┐ ┌─────────┐ ┌───────────┐ ┌──────────────┐
│wcore-│ │wcore-   │ │wcore-   │ │wcore-swarm│ │wcore-sandbox │
│tools │ │providers│ │mcp/     │ │(worktree, │ │(bwrap/       │
│      │ │(Llm-    │ │skills/  │ │ fleet,    │ │ sandbox-exec/│
│      │ │Provider)│ │memory   │ │ consensus)│ │ AppContainer)│
└──┬───┘ └────┬────┘ └────┬────┘ └─────┬─────┘ └──────┬───────┘
   │          │           │            │              │
   └──────────┴─────┬─────┴────────────┴──────────────┘
                     ▼
┌─────────────────────────────────────────────────────────────┐
│  wcore-config / wcore-protocol / wcore-egress / wcore-budget │
│  `crates/wcore-config`, `crates/wcore-protocol`               │
└───────────────────────────────┬───────────────────────────────┘
                                 ▼
┌─────────────────────────────────────────────────────────────┐
│  wcore-types — provider-neutral data types (zero internal deps)│
│  `crates/wcore-types/src`                                     │
└─────────────────────────────────────────────────────────────┘
```

## Component Responsibilities

| Component | Responsibility | File |
|-----------|----------------|------|
| Engine | Turn loop, streaming, tool dispatch, hook execution | `crates/wcore-agent/src/engine.rs` |
| Session | Session lifecycle, message history, state persistence | `crates/wcore-agent/src/session.rs` |
| Spawner | Sub-agent / child process spawning abstraction (`AgentSpawner` trait) used by Anvil, Council, Swarm | `crates/wcore-agent/src/spawner.rs`, `crates/wcore-agent/src/spawner/` |
| DurableSpawner | Crash-safe spawn with journal-backed resume | `crates/wcore-agent/src/durable_spawner.rs` |
| SessionJournal | Append-only durable event log; source of truth for turn/tool/budget/hook state, resumability | `crates/wcore-agent/src/session_journal.rs`, `crates/wcore-agent/src/session_journal/` |
| ChildTransaction | Transactional wrapper around a spawned child's mutation lifecycle | `crates/wcore-agent/src/child_transaction.rs`, `crates/wcore-agent/src/child_transaction/` |
| Anvil (gated forge) | Native "climb" loop that forges a verified/self-checked candidate against a real executable gate | `crates/wcore-agent/src/orchestration/anvil/` |
| Council | Multi-advisor proposal/aggregation orchestration (sibling of Anvil) | `crates/wcore-agent/src/orchestration/council/` |
| Workflow (ForgeFlows) | Declarative RON workflow DSL lowered to `GraphConfig` IR, executed via `WorkflowRunner` | `crates/wcore-agent/src/orchestration/workflow/` |
| ExecutionGraph | Per-turn graph walker (legacy/test scaffolding path, distinct from Workflow runner) | `crates/wcore-agent/src/orchestration/graph.rs` |
| wcore-swarm | Worktree isolation, fleet dispatch, consensus/debate, `CandidateSeal` sealing of candidates | `crates/wcore-swarm/src/` |
| wcore-sandbox | Cross-platform process sandbox backends + `HardContainmentAuthority` minting | `crates/wcore-sandbox/src/lib.rs`, `crates/wcore-sandbox/src/backends/` |
| wcore-providers | `LlmProvider` trait + Anthropic/OpenAI/Bedrock/Vertex implementations | `crates/wcore-providers/src/` |
| wcore-tools | Built-in tool trait (`Tool`) + implementations (Read, Write, Edit, Bash, Grep, Glob, Spawn) | `crates/wcore-tools/src/` |
| wcore-config | Config cascade, `ProviderCompat`, auth, hooks, shell helpers | `crates/wcore-config/src/` |
| wcore-protocol | JSON stream protocol (events/commands/approval) for host integration (Wayland desktop) | `crates/wcore-protocol/src/` |

## Pattern Overview

**Overall:** Layered Cargo workspace with strict downward dependency flow (bottom crates have zero/minimal internal deps; `wcore-agent` is the top engine that composes everything below it into a session/turn loop). Cross-cutting subsystems (sandboxing, provider abstraction, tool dispatch) are implemented as trait objects (`SandboxBackend`, `LlmProvider`, `Tool`, `AgentSpawner`) so `wcore-agent` never hardcodes a specific provider/platform/backend.

**Key Characteristics:**
- Strict downward crate dependency graph enforced by convention + `cargo metadata` review (see AGENTS.md crate map); plugin crates (`wayland-*`) go through `wcore-plugin-api` mirror types to avoid leaking `wcore-browser`/`wcore-cua`/`wcore-sandbox` deps into plugins (audit F2 isolation).
- Provider differences are resolved through the `ProviderCompat` config layer (`wcore-config`), never hardcoded `if base_url.contains(...)` conditionals in provider code.
- Fail-closed security posture: sandbox dispatch refuses execution (`FailClosedBackend`) unless a real backend is available or the operator opts into `WAYLAND_ALLOW_NO_SANDBOX=1`.
- Transactional delegated mutation: child/candidate work happens in an isolated checkout (worktree), is sealed (`CandidateSeal`), gated (real executable gate, not self-report), and only then landed into the parent's integration checkout via a CAS-guarded ref update with rollback on failure.
- Everything durable routes through `SessionJournal` (append-only, JSON-envelope) so crashes recover via idempotent resume rather than re-derivation.

## Layers

**`wcore-types` (bottom):**
- Purpose: shared provider-neutral types (`LlmRequest`, `LlmEvent`, `Message`, `ContentBlock`, execution policy types like `DangerousSessionGrant`)
- Location: `crates/wcore-types/src`
- Depends on: nothing internal
- Used by: virtually every other crate

**`wcore-config` / `wcore-protocol` / `wcore-egress` / `wcore-budget` (mid, foundational):**
- Purpose: configuration cascade + `ProviderCompat` + auth + cross-platform shell helpers (`wcore-config`); JSON-stream protocol events/commands/approval manager for host integration (`wcore-protocol`); single outbound-HTTP chokepoint (`wcore-egress`, `EgressClient`); budget caps + telemetry (`wcore-budget`)
- Location: `crates/wcore-config`, `crates/wcore-protocol`, `crates/wcore-egress`, `crates/wcore-budget`
- Depends on: `wcore-types`
- Used by: providers, tools, agent

**`wcore-providers` / `wcore-tools` / `wcore-mcp` / `wcore-skills` / `wcore-memory` / `wcore-sandbox` / `wcore-browser` / `wcore-cua` / `wcore-swarm` (mid, capability crates):**
- Purpose: pluggable capabilities the engine composes — LLM providers, built-in tools, MCP client, skills, long-term memory, process sandboxing, browser/computer-use tool families, worktree-isolated multi-agent dispatch
- Location: `crates/wcore-providers`, `crates/wcore-tools`, `crates/wcore-mcp`, `crates/wcore-skills`, `crates/wcore-memory`, `crates/wcore-sandbox`, `crates/wcore-browser`, `crates/wcore-cua`, `crates/wcore-swarm`
- Depends on: `wcore-types`, `wcore-config`, `wcore-protocol` (varies per crate)
- Used by: `wcore-agent`

**`wcore-agent` (top engine):**
- Purpose: turn loop, session/spawner lifecycle, orchestration (Anvil/Council/Workflow), transactional delegated mutation
- Location: `crates/wcore-agent/src`
- Contains: `engine.rs` (turn loop core, largest file in the workspace at 1.2MB), `session.rs`, `spawner.rs`/`spawner/`, `session_journal.rs`/`session_journal/`, `child_transaction.rs`/`child_transaction/`, `orchestration/`
- Depends on: every mid-layer crate
- Used by: `wcore-cli`, plugin crates (`wayland-*`) via `wcore-plugin-api` mirrors, host integrations via `wcore-protocol`

**`wcore-cli` (top, entry binary):**
- Purpose: CLI parsing, TUI, slash commands, doctor/migrate/plugin subcommands, the `anvil` verb
- Location: `crates/wcore-cli/src`
- Contains: `main.rs` (318.7K — CLI entry + arg wiring), `acp.rs`/`acp_engine.rs` (Agent Client Protocol), `anvil.rs`, `swarm.rs`, `crucible.rs`, `tui/`
- Depends on: `wcore-agent` and lower layers
- Used by: end users (binary), Wayland desktop app (via JSON stream protocol, not this crate directly)

## Data Flow

### Primary Turn/Request Path

1. `wcore-cli/src/main.rs` parses args, constructs the engine/session, invokes `wcore-agent::engine` for a turn.
2. `Engine` streams from an `LlmProvider` (`crates/wcore-providers/src/lib.rs:135` trait) — provider quirks resolved via `ProviderCompat`, never hardcoded.
3. Tool calls dispatch through `ToolDispatcher` (`crates/wcore-tools/src/dispatcher.rs:22`) to `Tool` implementations (`crates/wcore-tools/src/lib.rs:319`); shell-executing tools route through `wcore_config::shell` (argv mode for untrusted data, shell-string mode only where semantics require it, e.g. `BashTool`).
4. Sandboxed tool execution goes through `wcore-sandbox::SandboxBackend::execute` against a per-platform backend (bwrap/Landlock+seccomp on Linux, sandbox-exec on macOS, AppContainer+Job Object on Windows, Docker opt-in); fails closed with `FailClosedBackend` if no real backend is available.
5. Turn/tool/budget/hook state is durably recorded to `SessionJournal` (`crates/wcore-agent/src/session_journal.rs`) as the turn progresses, enabling idempotent resume after a crash.

### Anvil (Gated Forge) Delegated-Mutation Flow

Entry point: `drive_climb_full` (`crates/wcore-agent/src/orchestration/anvil/forge.rs:628`), the production wiring of the climb loop (probe → gate → surgical → terminal), invoked via the `Forge` tool (`crates/wcore-agent/src/orchestration/anvil/tool.rs`) or the CLI `anvil` verb (`crates/wcore-cli/src/anvil.rs`). Rides the DRIVER rail (mirrors `council::drive_council`), NOT the test-only `GraphConfig`/`ExecutionGraph` walker.

1. **Isolated-checkout substrate:** work happens in a `wcore-swarm` worktree (`crates/wcore-swarm/src/worktree.rs`, `worktree/candidate.rs`, `worktree/parent.rs`), giving each climb candidate its own filesystem isolation from the parent integration checkout.
2. **Production spawner routing:** the climb loop drives an `AgentSpawner` (`crates/wcore-agent/src/spawner.rs`) via `SpawnBuilder`/`SpawnValve` (`forge.rs:361`, `forge.rs:506`) to run the actual candidate-generation work.
3. **Gate execution:** each candidate is checked against a real executable gate (`SandboxGate`, `forge.rs:159`) — never a self-report — with gate closure pinning and injection fencing enforced in `crates/wcore-agent/src/orchestration/anvil/gates.rs`.
4. **CandidateSeal:** on success, the candidate's mutation guard is sealed via `CandidateSeal` (`wcore_swarm::worktree::CandidateSeal`, consumed in `engine.rs:75` `into_landing_authority` and `forge.rs:326`) — the ONLY path that hands a candidate's guard to the landing authority.
5. **Durable receipt / AcceptedCandidate:** the outcome is stamped with a `TerminalState` (`crates/wcore-agent/src/orchestration/anvil/mod.rs`) — `Verified` (real Tier-1 gate), `CriteriaChecked` (Tier-2), `SelfChecked` (Tier-3), or a non-success terminal (`NeedsEscalation`, `Blocked`, `Cancelled`, `TimedOut`, `PermissionDenied`, `CrashedRecovered`, `Superseded`) — persisted through `crates/wcore-agent/src/orchestration/anvil/journal.rs` for crash recovery.
6. **Parent landing / CAS + rollback:** `land_selected_winner` (`crates/wcore-agent/src/orchestration/anvil/landing.rs:123`) takes a `WinnerLandingRequest` (`landing.rs:65`) and lands the sealed winner into the Wayland-owned integration checkout via the parent's compare-and-swap ref update (`landing.rs:159` comment: "Land into the Wayland-owned integration checkout via the parent CAS"); failure returns `WinnerLandingError` (`landing.rs:94`) and the CAS refusal rolls back cleanly rather than partially landing.
7. **session_journal integration:** the journal handle is threaded through the forge path (`forge.rs:905`, `spawner.session_journal()`) so the whole climb is crash-recoverable and idempotently resumable.

**Containment enforcement:** independent of the climb's own gate, any hard-contained execution the climb spawns is authorized by minting a one-use `HardContainmentAuthority` (`crates/wcore-sandbox/src/lib.rs:473` `mint`, consumed at `lib.rs:503`) which binds to and is verified against the specific containment context it was minted for — an Anvil gate result is translated to this parent-owned authorization in `crates/wcore-agent/src/orchestration/anvil/gate_authorization.rs`.

### ForgeFlows (Dynamic Workflows)

1. Declarative RON workflow definitions are parsed via `crates/wcore-agent/src/orchestration/workflow/dsl.rs` / `schema.rs`.
2. Lowered to the existing `GraphConfig` IR (shared with `orchestration/graph.rs`).
3. Executed by `WorkflowRunner` (`crates/wcore-agent/src/orchestration/workflow/runner.rs`) driven over the `AgentSpawner`/fleet dispatcher spawner path — explicitly NOT the per-turn `ExecutionGraph` walker. Limits/estimation/pipeline concerns live in `limits.rs`, `estimate.rs`, `pipeline.rs`. See `docs/workflows.md`.

**State Management:**
- Turn/session state: `SessionJournal` (append-only envelopes, `crates/wcore-agent/src/session_journal.rs`).
- Climb state: `crates/wcore-agent/src/orchestration/anvil/journal.rs` (append-only climb journal, idempotent resume) plus `ledger.rs` (per-task cost ledger, atomic reservation-before-dispatch) and `lease.rs` (per-workspace climb lease preventing interleaved climbs/user edits).
- Worktree/candidate state: `wcore-swarm` (`worktree_manager.rs`, `worktree_cleanup.rs`, `worktree_security.rs`).

## Key Abstractions

**`LlmProvider` (provider abstraction):**
- Purpose: represents a single LLM backend's streaming request/response surface
- Examples: `crates/wcore-providers/src/lib.rs:135` (trait def), provider impls under `crates/wcore-providers/src/`
- Pattern: object-safe trait (`Send + Sync`); provider quirks resolved through `ProviderCompat` fields set in per-provider default functions (e.g. `openai_defaults()`), never hardcoded conditionals — this is the single most important architectural rule per `AGENTS.md`

**`Tool` / `ToolDispatcher` (tool execution abstraction):**
- Purpose: uniform interface for built-in and plugin tools; dispatcher decouples the engine from concrete tool implementations
- Examples: `crates/wcore-tools/src/lib.rs:319` (`Tool` trait), `crates/wcore-tools/src/dispatcher.rs:22` (`ToolDispatcher` trait), `ToolOutputSink` (`lib.rs:288`)
- Pattern: trait objects registered into a dispatcher; sandboxed execution delegated to `wcore-sandbox`

**`SandboxBackend` / `HardContainmentAuthority` (containment abstraction):**
- Purpose: uniform per-platform process isolation with a fail-closed default and a one-use, context-bound authority token for hard-contained execution
- Examples: `crates/wcore-sandbox/src/backends/mod.rs` (backend trait + registry), `crates/wcore-sandbox/src/lib.rs:473/503/536` (`mint`/consume/`HardContainmentAuthority` struct)
- Pattern: `SandboxRegistry` selects `default_for_platform` by `cfg`; `HardContainmentAuthority` is minted once and verified as still bound to the exact context it was minted for before being consumed

**`AgentSpawner` (spawn abstraction):**
- Purpose: uniform interface for spawning sub-agents/children, used by Anvil, Council, and Swarm/ForgeFlows alike so orchestration logic doesn't special-case the spawn mechanism
- Examples: `crates/wcore-agent/src/spawner.rs`, `crates/wcore-agent/src/durable_spawner.rs` (crash-safe variant)
- Pattern: trait object threaded through `SpawnBuilder`/`SpawnValve` (Anvil) and the fleet dispatcher (Swarm/Workflow)

**`CandidateSeal` / transactional delegated mutation:**
- Purpose: represents a sealed, gate-verified candidate's mutation guard — the single hand-off point between "candidate produced work in isolation" and "authorized to land into the parent checkout"
- Examples: `wcore_swarm::worktree::CandidateSeal`, consumed in `crates/wcore-agent/src/orchestration/anvil/engine.rs:75`, `forge.rs:326`; landed via `crates/wcore-agent/src/orchestration/anvil/landing.rs:123`
- Pattern: guard + seal pair prevents any code path from landing an unsealed (i.e., ungated) candidate

## Entry Points

**CLI binary (`main.rs`):**
- Location: `crates/wcore-cli/src/main.rs`
- Triggers: user invocation of the `wayland-core` / `wl`-style binary
- Responsibilities: arg parsing, subcommand dispatch (`agent_cmd.rs`, `anvil.rs`, `swarm.rs`, `cron.rs`, `workflow.rs`, `crucible.rs`, etc.), engine/session bootstrap

**Anvil CLI verb:**
- Location: `crates/wcore-cli/src/anvil.rs`
- Triggers: `wayland-core anvil ...` invocation
- Responsibilities: materializes a driver seat (`crates/wcore-agent/src/orchestration/anvil/seat.rs`) and calls `drive_climb_full`

**Forge tool (in-session):**
- Location: `crates/wcore-agent/src/orchestration/anvil/tool.rs`
- Triggers: model-issued tool call during a live agent session
- Responsibilities: natural language in, receipt out — same underlying `drive_climb_full` path as the CLI verb

**JSON stream protocol (host integration):**
- Location: `crates/wcore-protocol/src/`, consumed by `wcore-agent` output/session layers
- Triggers: host process (e.g. Wayland desktop Electron app) driving the engine over JSON Lines
- Responsibilities: structured events/commands/approval flow instead of a TTY

**ACP (Agent Client Protocol):**
- Location: `crates/wcore-cli/src/acp.rs`, `acp_engine.rs`, `acp_roster.rs`
- Triggers: ACP-speaking host client
- Responsibilities: alternate host-integration surface parallel to the JSON stream protocol

## Architectural Constraints

- **Threading:** Async/await (Tokio) throughout; `AgentSpawner`/`DurableSpawner` spawn child processes/tasks, not raw OS threads, for orchestration fan-out; sandbox backends spawn and reap real OS process trees (`crates/wcore-sandbox/src/backends/process_tree.rs`).
- **Global state:** `SessionJournal` is the closest thing to a durable shared-state singleton, but it is instance-scoped per session/spawner rather than a process-global; no other module-level mutable singletons were observed in the explored subsystems.
- **Circular imports:** None expected by design — the crate map in `AGENTS.md` mandates strictly downward dependencies; `wcore-plugin-api` is a deliberate isolation boundary (enforced by a `build.rs` lint) so plugin crates (`wayland-*`) cannot pull in `wcore-browser`/`wcore-cua`/`wcore-sandbox` directly.
- **Isolation boundary:** `wcore-repomap` deliberately has NO internal `wcore-*` deps; `wcore-plugin-api` is capped to `wcore-types`/`wcore-protocol` beyond its own surface.
- **Fail-closed default:** sandbox execution refuses (does not silently degrade) when no real platform backend is available, unless explicitly overridden with `WAYLAND_ALLOW_NO_SANDBOX=1`.

## Anti-Patterns

### Hardcoded provider quirks

**What happens:** Provider-specific behavior detected via string matching on `base_url` or provider name and branched inline in provider code.
**Why it's wrong:** Scatters provider knowledge across the codebase, breaks when a provider changes its base URL or a new compatible provider appears, and bypasses the single audited configuration surface.
**Do this instead:** Add an `Option<T>` field to `ProviderCompat` (`wcore-config`), set its default in the relevant `*_defaults()` function, and read it via `self.compat.field_name` in provider code. See `AGENTS.md` §"No Hardcoded Provider Quirks" for the canonical wrong/right example.

### Raw shell spawning / string-interpolated shell commands

**What happens:** Calling `Command::new("sh"/"bash"/"cmd")` directly, or building a shell-string command via `format!` with LLM-supplied data interpolated into it.
**Why it's wrong:** Bypasses the centralized, audited `wcore_config::shell` helpers, is platform-specific, and — when LLM data is interpolated into a shell string — is a shell injection vulnerability (the exact class closed in Wave SA per `SECURITY-v0.2.0.md` BLOCKER #1).
**Do this instead:** Use `shell_command_argv(program, &[args])` for any command whose arguments include LLM-supplied data (no shell interpreter involved, metacharacters are inert); reserve `shell_command`/`shell_command_builder` (real `sh -c`/`cmd /C`) only for cases that genuinely need shell semantics (e.g. `BashTool`, MCP stdio program-launch, skill `!shell:` directives), and never interpolate untrusted data into that string.

## Error Handling

**Strategy:** `thiserror` for public API error types (structured, matchable — e.g. `SandboxError`, `ForgeError`, `WinnerLandingError`, `JournalError`), `anyhow` for internal/application-level propagation. `unwrap()` is forbidden in production code unless the invariant is proven and commented.

**Patterns:**
- Terminal-state enums (e.g. Anvil's `TerminalState`) enumerate every possible exit explicitly rather than allowing a "silent fourth exit" — errors are a first-class outcome, not just a `Result::Err`.
- Fail-closed defaults for security-sensitive subsystems (sandbox execution, containment authority) rather than fail-open/degrade.

## Cross-Cutting Concerns

**Logging:** Structured via `wcore-observability` (trace schema, span sinks, OTLP exporter) — sits between `wcore-types`/`wcore-config` and `wcore-agent`; the protocol crate stays decoupled from it via opaque `serde_json::Value` payloads.
**Validation/Safety:** `wcore-safety` provides output validator + PII scrubber primitives, independent of the sandbox layer.
**Authentication:** Handled in `wcore-config` (auth, hooks) at the mid layer; per-session/channel auth (e.g. OAuth) lives under `crates/wcore-agent/src/oauth/`.
**Budgeting:** `wcore-budget` (caps/telemetry) at the mid layer plus `crates/wcore-agent/src/budget_authority.rs` and `tool_budget.rs` at the engine layer; Anvil additionally has its own per-task cost ledger (`orchestration/anvil/ledger.rs`).

---

*Architecture analysis: 2026-07-23*
