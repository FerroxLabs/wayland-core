<!-- refreshed: 2026-07-18 -->
# Architecture

**Analysis Date:** 2026-07-18

## System Overview

```text
┌───────────────────────────────────────────────────────────────────────────┐
│                              Host Surfaces                                │
├──────────────────────┬───────────────────────┬────────────────────────────┤
│ CLI / TUI / REPL     │ JSON-stream / ACP     │ Channels / scheduled work  │
│ `crates/wcore-cli/`  │ `crates/wcore-        │ `crates/wcore-channels/`   │
│                      │ protocol/`, `wcore-acp`│ `crates/wcore-cron/`       │
└──────────┬───────────┴───────────┬───────────┴─────────────┬──────────────┘
           │                       │                         │
           └───────────────────────┼─────────────────────────┘
                                   ▼
┌───────────────────────────────────────────────────────────────────────────┐
│                     Agent and Orchestration Runtime                       │
│ `crates/wcore-agent/src/bootstrap.rs` → `engine.rs` → `orchestration/`    │
└──────────┬─────────────────┬─────────────────┬────────────────────────────┘
           │                 │                 │
           ▼                 ▼                 ▼
┌──────────────────┐ ┌──────────────────┐ ┌────────────────────────────────┐
│ Model capability │ │ Action capability│ │ Stateful capability            │
│ `wcore-providers`│ │ `wcore-tools`    │ │ `wcore-memory`, `wcore-skills`,│
│                  │ │ `wcore-mcp`      │ │ `wcore-evolve`, `wcore-budget` │
└─────────┬────────┘ └─────────┬────────┘ └───────────────┬────────────────┘
          └────────────────────┼───────────────────────────┘
                               ▼
┌───────────────────────────────────────────────────────────────────────────┐
│                  Shared Contracts and Policy Boundaries                   │
│ `wcore-config` · `wcore-protocol` · `wcore-plugin-api` · `wcore-types`   │
│ `wcore-egress` · `wcore-permissions` · `wcore-sandbox`                   │
└──────────┬────────────────────────────────────────────────────────────────┘
           ▼
┌───────────────────────────────────────────────────────────────────────────┐
│ Filesystem state, provider APIs, MCP servers, OS sandboxes, channel APIs  │
│ Session files: `crates/wcore-agent/src/session.rs` and `session_journal.rs`│
└───────────────────────────────────────────────────────────────────────────┘
```

## Component Responsibilities

| Component | Responsibility | File |
|-----------|----------------|------|
| Process host | Parses commands, resolves launch mode, owns Tokio runtime, and selects TUI, REPL, one-shot, or JSON-stream execution | `crates/wcore-cli/src/main.rs` |
| CLI library | Implements host-facing subcommands and bridges that cannot live in engine-free crates | `crates/wcore-cli/src/lib.rs` |
| Bootstrap | Builds the session-scoped provider, tool registry, plugins, MCP managers, memory, hooks, channels, budgets, and policy handles | `crates/wcore-agent/src/bootstrap.rs` |
| Agent engine | Owns conversation state, provider streaming, tool turns, compaction, budgets, cancellation, durable recovery, and output emission | `crates/wcore-agent/src/engine.rs` |
| Orchestration | Executes tool calls and agent graphs, dynamic workflows, council runs, and Anvil flows over shared spawner seams | `crates/wcore-agent/src/orchestration/` |
| Provider layer | Converts provider-neutral requests into Anthropic, OpenAI, Bedrock, Vertex, Gemini, Flux, and compatible wire formats | `crates/wcore-providers/src/lib.rs` |
| Tool layer | Defines the `Tool` contract, registry, workspace policy, built-in actions, and tool result shaping | `crates/wcore-tools/src/lib.rs` |
| Configuration | Resolves global/project/CLI configuration and centralizes provider compatibility, credentials, shell, and platform behavior | `crates/wcore-config/src/config.rs` |
| Host protocol | Defines host commands/events, bounded stdin ingestion, approval state, and JSONL emission | `crates/wcore-protocol/src/lib.rs` |
| Plugin boundary | Exposes mirror types and scoped registrars without importing host implementation crates | `crates/wcore-plugin-api/src/lib.rs` |
| Session persistence | Stores session mirrors, write-ahead messages, indexes, exclusive run leases, and recovery journals | `crates/wcore-agent/src/session.rs` |
| Memory and learning | Provides long-term memory, skill discovery/routing, evaluation, and prompt evolution as peer substrates | `crates/wcore-memory/src/lib.rs` |
| Channels | Defines the transport-neutral channel lifecycle and registers concrete platform adapters | `crates/wcore-channels/src/lib.rs` |
| Security boundary | Centralizes outbound HTTP policy, approval policy, workspace containment, and platform sandbox execution | `crates/wcore-egress/src/lib.rs` |

## Pattern Overview

**Overall:** Layered Cargo workspace with ports-and-adapters boundaries and a session-scoped async runtime.

**Key Characteristics:**
- Dependencies flow from host crates through `wcore-agent` into capability crates and shared contracts; add shared behavior at the lowest semantically correct layer in `Cargo.toml`.
- Provider, tool, memory, channel, output, plugin, and spawning behavior is expressed through traits and registries rather than referenced as concrete implementations by the engine hot path.
- `AgentBootstrap` is the composition root. Runtime code consumes already-resolved policy and capability handles instead of reconstructing them per turn.
- JSON-stream contracts remain separate from observability internals; protocol trace payloads use opaque serialized values in `crates/wcore-protocol/src/events.rs`.
- Durable sessions use an event journal as recovery authority and a session JSON file as the user-facing mirror in `crates/wcore-agent/src/session_journal.rs` and `crates/wcore-agent/src/session.rs`.

## Layers

**Host and Transport Layer:**
- Purpose: Translate terminal, desktop, ACP, channel, cron, and subprocess interactions into engine turns and render engine output.
- Location: `crates/wcore-cli/`, `crates/wcore-acp/`, `crates/wcore-channels/`, `crates/wcore-channel-*/`, `crates/wcore-cron/`
- Contains: Process entry points, TUI state/rendering, JSON-stream command loop, ACP transports, channel connectors, and CLI subcommands.
- Depends on: `wcore-agent`, transport-neutral protocol types, configuration, and capability crates needed by explicit bridges.
- Used by: End users, the Wayland Desktop host, ACP clients, channel platforms, and schedulers.

**Agent Runtime Layer:**
- Purpose: Compose a session and run crash-recoverable model/tool turns.
- Location: `crates/wcore-agent/src/`
- Contains: Bootstrap, engine, output sinks, session journal, hooks, orchestration graphs, spawners, channel dispatch, recovery, and tool backends.
- Depends on: Capability crates and shared contracts below it.
- Used by: `wcore-cli` and engine-backed host bridges.

**Capability Layer:**
- Purpose: Implement independently testable model, action, context, learning, communication, and observability capabilities.
- Location: `crates/wcore-providers/`, `crates/wcore-tools/`, `crates/wcore-mcp/`, `crates/wcore-memory/`, `crates/wcore-skills/`, `crates/wcore-browser/`, `crates/wcore-cua/`, `crates/wcore-observability/`
- Contains: Provider adapters, built-in tools, MCP client/server transports, memory stores, skill catalogs, browser/CUA backends, and trace sinks.
- Depends on: `wcore-config`, `wcore-protocol`, `wcore-types`, and narrowly scoped peer crates declared in each manifest.
- Used by: `wcore-agent`, `wcore-cli`, evaluation harnesses, and plugin host adapters.

**Policy and Shared Contract Layer:**
- Purpose: Define provider-neutral data, configuration, protocol envelopes, permission decisions, egress controls, and plugin-facing mirror types.
- Location: `crates/wcore-types/`, `crates/wcore-config/`, `crates/wcore-protocol/`, `crates/wcore-plugin-api/`, `crates/wcore-egress/`, `crates/wcore-permissions/`, `crates/wcore-sandbox/`
- Contains: `LlmRequest`, `LlmEvent`, messages, execution policy, `ProviderCompat`, protocol commands/events, plugin registrars, and sandbox manifests.
- Depends on: External Rust libraries and only lower/peer internal crates allowed by the workspace graph.
- Used by: Every higher runtime and capability layer.

**Plugin Layer:**
- Purpose: Package optional providers and tool families without making plugin crates depend on their host implementations.
- Location: `crates/wayland-*/`, `crates/wcore-plugin-wasm/`, `crates/wcore-plugin-subprocess/`, `crates/wcore-pluginsrc/`
- Contains: Static inventory plugins, plugin manifests, mirror specs, WASM/subprocess runtimes, and marketplace-source lowering.
- Depends on: Primarily `wcore-plugin-api` and protocol/types allowed by the isolation contract.
- Used by: `PluginLoader` and host adapters in `crates/wcore-agent/src/plugins/`.

**Verification and Evolution Layer:**
- Purpose: Score behavior, replay traces, exercise real scenarios, evolve prompts, and validate packaged outcomes.
- Location: `crates/wcore-eval/`, `crates/wcore-eval-scenarios/`, `crates/wcore-evolve/`, `crates/wcore-replay/`, `crates/wcore-fixture-harness/`
- Contains: Eval gates, scenario drivers, receipts, prompt mutation/retention, trace replay, and customer-shaped fixtures.
- Depends on: Public capability and protocol surfaces rather than private host state where possible.
- Used by: CI, release proof, benchmark flows, and developers.

## Data Flow

### Primary Request Path

1. `main` activates the selected profile, loads process environment, constructs a multithreaded Tokio runtime, and enters `run` (`crates/wcore-cli/src/main.rs:886`).
2. `run` resolves config/provenance and launch policy, then selects JSON-stream, TUI, REPL, or one-shot mode (`crates/wcore-cli/src/main.rs:962`, `crates/wcore-config/src/config.rs:2015`).
3. The chosen host creates an `OutputSink` and calls `AgentBootstrap::build`; bootstrap assembles plugins, built-ins, MCP tools, memory, channels, policies, and provider (`crates/wcore-agent/src/bootstrap.rs:552`).
4. A host message reaches `AgentEngine::run_with_content`; the engine opens a durable turn, checks recovery and budget authority, appends the user message, and enters `run_inner` (`crates/wcore-agent/src/engine.rs:5978`, `crates/wcore-agent/src/engine.rs:8532`).
5. `run_inner` builds a provider-neutral `LlmRequest` from the conversation, selected tools, compatibility settings, cache boundaries, and transient context (`crates/wcore-agent/src/engine.rs:9109`).
6. The selected `LlmProvider` streams `LlmEvent` values; text/thinking is emitted immediately and tool-use blocks are collected (`crates/wcore-agent/src/engine.rs:10131`, `crates/wcore-agent/src/engine.rs:10353`).
7. Tool calls execute through an `AgentNodeExecutor` and `ExecutionGraph`; approval, budgets, hooks, cancellation, effect receipts, and registry dispatch are applied at this boundary (`crates/wcore-agent/src/engine.rs:11360`, `crates/wcore-agent/src/engine.rs:11420`).
8. Tool results are appended to conversation state and the provider loop continues until a terminal model event, budget/loop guard, cancellation, or error commits the durable turn and emits stream end (`crates/wcore-agent/src/engine.rs:6093`, `crates/wcore-cli/src/main.rs:4900`).

### JSON-Stream Host Flow

1. `run_json_stream_mode` creates a `ProtocolWriter`, `ProtocolSink`, and shared `ToolApprovalManager`, then bootstraps a primary long-running engine (`crates/wcore-cli/src/main.rs:4045`).
2. A bounded dedicated stdin thread parses newline-delimited `ProtocolCommand` values into a Tokio channel (`crates/wcore-protocol/src/reader.rs:72`).
3. `ProtocolCommand::Message` validates composer attachments and races `engine.run_with_content` against approvals, stop, config, MCP, diagnostics, and host-send commands (`crates/wcore-cli/src/main.rs:4790`).
4. `ProtocolSink` converts engine callbacks into `ProtocolEvent`; `ProtocolWriter` serializes them through the output pump to stdout (`crates/wcore-agent/src/output/protocol_sink.rs`, `crates/wcore-protocol/src/writer.rs`).

### Plugin and MCP Bootstrap Flow

1. `PluginLoader` discovers statically linked inventory plugins and on-disk manifests; `PluginRunner` captures declared capabilities (`crates/wcore-agent/src/plugins/loader.rs`, `crates/wcore-agent/src/plugins/runner.rs`).
2. Host registrars translate plugin-api mirror specs into concrete tools, providers, hooks, skills, rules, agents, user models, browser tools, and CUA tools (`crates/wcore-agent/src/plugins/apply.rs`).
3. Config-declared and plugin-declared MCP servers connect through `McpManager`; advertised MCP tools join the same `ToolRegistry` with collision handling (`crates/wcore-agent/src/bootstrap.rs`, `crates/wcore-mcp/src/tool_proxy.rs`).
4. The registry refreshes `ToolSearch` after every boot-time capability is present, so the provider sees one authoritative catalog (`crates/wcore-tools/src/registry.rs`).

**State Management:**
- Mutable conversation and per-turn state stays inside one `AgentEngine`; shared registries and policies use `Arc`, locks, atomics, Tokio channels, and cancellation tokens only where concurrent host/tool tasks require them.
- Persistent sessions use `SessionManager` JSON/index/WAL files and an exclusive `SessionJournal` writer lease; incomplete effects fail closed into recovery/reconciliation (`crates/wcore-agent/src/session.rs`, `crates/wcore-agent/src/session_journal.rs`).
- Long-term semantic/procedural/user context is accessed through `MemoryApi`; `NullMemory` preserves the same engine contract when memory is disabled (`crates/wcore-memory/src/api.rs`).
- Host approval state is centralized in one shared `ToolApprovalManager` so CLI/TUI/protocol commands and tool dispatch observe the same mode and pending decisions (`crates/wcore-protocol/src/lib.rs`).

## Key Abstractions

**`LlmProvider`:**
- Purpose: Stream provider-neutral `LlmEvent` values for an `LlmRequest`.
- Examples: `crates/wcore-providers/src/anthropic.rs`, `crates/wcore-providers/src/openai.rs`, `crates/wcore-providers/src/bedrock.rs`
- Pattern: Async trait adapter selected by config and wrapped for retry/failover/journaling.

**`Tool` and `ToolRegistry`:**
- Purpose: Publish schemas, availability, effect contracts, execution class, and async execution for built-in and adapted tools.
- Examples: `crates/wcore-tools/src/lib.rs`, `crates/wcore-tools/src/registry.rs`, `crates/wcore-agent/src/plugins/adapters/plugin_tool_adapter.rs`
- Pattern: Command registry with collision control, deferred discovery, policy context, and circuit breakers.

**`OutputSink` and `ProtocolEmitter`:**
- Purpose: Keep engine output independent from terminal, JSON-stream, null, or channel presentation.
- Examples: `crates/wcore-agent/src/output/mod.rs`, `crates/wcore-agent/src/output/terminal.rs`, `crates/wcore-agent/src/output/protocol_sink.rs`, `crates/wcore-protocol/src/writer.rs`
- Pattern: Ports-and-adapters output boundary.

**`Config` and `ProviderCompat`:**
- Purpose: Carry fully resolved launch policy and data-driven provider differences into runtime code.
- Examples: `crates/wcore-config/src/config.rs`, `crates/wcore-config/src/compat.rs`, `crates/wcore-config/src/resolution_provenance.rs`
- Pattern: Cascading configuration plus typed compatibility presets; project input may narrow protected settings but cannot silently widen trust.

**Plugin mirror registries:**
- Purpose: Let plugins declare capabilities without importing engine/tool/browser/CUA/MCP implementations.
- Examples: `crates/wcore-plugin-api/src/registry/`, `crates/wcore-agent/src/plugins/adapters/`, `crates/wayland-browser/src/lib.rs`
- Pattern: Stable mirror DTOs translated by host-side adapters at the isolation boundary.

**Session authority:**
- Purpose: Separate user-facing session mirrors from crash-recovery truth and enforce one writer for a live persisted session.
- Examples: `crates/wcore-agent/src/session.rs`, `crates/wcore-agent/src/session_journal.rs`, `crates/wcore-agent/src/recovery.rs`
- Pattern: Write-ahead/event journal plus replay and explicit reconciliation.

**Spawner and graph IR:**
- Purpose: Run direct turns, sub-agents, councils, and declarative workflows through shared budget/cancellation/durability controls.
- Examples: `crates/wcore-agent/src/spawner.rs`, `crates/wcore-agent/src/orchestration/graph.rs`, `crates/wcore-agent/src/orchestration/workflow/runner.rs`
- Pattern: Strategy-backed spawner plus graph intermediate representation.

**`Channel`:**
- Purpose: Normalize platform lifecycle, inbound polling/webhooks, media, reactions, typing, and outbound messages.
- Examples: `crates/wcore-channels/src/lib.rs`, `crates/wcore-channels/src/manager.rs`, `crates/wcore-channels-registry/src/lib.rs`
- Pattern: Adapter trait with platform factories and supervised background tasks.

## Entry Points

**`wayland-core` binary:**
- Location: `crates/wcore-cli/src/main.rs`
- Triggers: Terminal invocation, desktop child-process launch, scripts, or tests.
- Responsibilities: Process setup, config and execution-policy resolution, subcommand dispatch, runtime lifecycle, and host-mode selection.

**CLI library subcommands:**
- Location: `crates/wcore-cli/src/lib.rs`
- Triggers: `acp`, `agent`, `auth`, `profile`, `migrate`, `workflow`, `forge`, `crucible`, `cron`, `image`, `fetch`, and related command variants.
- Responsibilities: Bridge user-facing commands to lower crates without introducing upward dependencies.

**Evaluation binaries:**
- Location: `crates/wcore-eval/src/bin/`, `crates/wcore-eval-scenarios/src/bin/`, `crates/wcore-evolve/src/bin/`
- Triggers: Eval, receipt, benchmark, fixture, and evolution commands declared in their `Cargo.toml` files.
- Responsibilities: Drive public runtime/protocol surfaces and produce scored or signed evidence.

**MCP server surface:**
- Location: `crates/wcore-cli/src/mcp_serve.rs`, `crates/wcore-mcp/src/server.rs`
- Triggers: `wayland-core mcp-serve` over stdio or SSE.
- Responsibilities: Expose the engine tool set through MCP with an injected policy gate.

**Standalone plugin examples:**
- Location: `examples/plugin-wasm-hello/`, `examples/plugin-subprocess-mcp/`
- Triggers: Independent Cargo builds outside workspace membership.
- Responsibilities: Demonstrate WASM component and subprocess/MCP plugin packaging.

## Architectural Constraints

- **Threading:** `main` builds a multithreaded Tokio runtime on a dedicated 32 MiB stack thread; JSON stdin uses one bounded blocking reader thread, async components use Tokio tasks/channels, and blocking libraries such as IMAP use `spawn_blocking` (`crates/wcore-cli/src/main.rs:886`, `crates/wcore-protocol/src/reader.rs`).
- **Global state:** Resolve active profile/environment before spawning threads; use session-scoped egress, cancellation, approval, workspace policy, and registry handles thereafter. Static plugin discovery uses `inventory`, while runtime plugin handles are retained by the engine (`crates/wcore-agent/src/bootstrap.rs`).
- **Circular imports:** No known circular internal dependency chain is permitted. Keep `wcore-cli` and `wcore-agent` at the top, plugin implementations behind `wcore-plugin-api`, and shared types in the lowest viable crate (`Cargo.toml`, `crates/wcore-plugin-api/build.rs`).
- **Provider compatibility:** Put wire quirks in `ProviderCompat` fields/defaults and consume them inside provider request builders (`crates/wcore-config/src/compat.rs`).
- **Platform behavior:** Centralize shell and platform differences in `wcore-config::shell`; pass attacker-controlled arguments through argv mode (`crates/wcore-config/src/shell.rs`).
- **Network boundary:** Route in-process outbound HTTP through `wcore-egress`; pass the session policy into late-created clients (`crates/wcore-egress/src/lib.rs`).
- **Filesystem boundary:** Install one `WorkspacePolicy` in the tool registry and derive file-tool and Bash sandbox behavior from it (`crates/wcore-tools/src/workspace_policy.rs`).
- **Plugin isolation:** Plugin crates use plugin-api mirror types and must not depend directly on host browser, CUA, MCP, memory, or skills implementations (`crates/wcore-plugin-api/src/lib.rs`).
- **Cross-platform verification:** Code targets macOS, Linux, and Windows; platform-specific backends live behind centralized APIs and `cfg`-gated modules (`crates/wcore-sandbox/src/backends/`).

## Anti-Patterns

### Hardcoded Provider Detection

**What happens:** Provider behavior is selected from a URL/model string inside request code instead of from resolved compatibility data.
**Why it's wrong:** Compatible endpoints and aliases share wire shapes, so string detection couples runtime behavior to one vendor URL and bypasses profile inheritance.
**Do this instead:** Add/merge a field in `crates/wcore-config/src/compat.rs`, set provider defaults there, and read it in `crates/wcore-providers/src/`.

### Plugin-to-Host Dependency Leakage

**What happens:** A `wayland-*` plugin imports a concrete host crate such as `wcore-browser`, `wcore-cua`, `wcore-mcp`, `wcore-memory`, or `wcore-skills`.
**Why it's wrong:** It reverses the plugin isolation boundary and makes plugin loading create upward/circular dependency pressure.
**Do this instead:** Define/use mirror data in `crates/wcore-plugin-api/src/` and translate it in `crates/wcore-agent/src/plugins/adapters/`.

## Error Handling

**Strategy:** Use typed, matchable errors at crate/public boundaries and contextual propagation in composition/application code; fail closed at security, recovery, and effect-uncertainty boundaries while allowing non-critical optional plugins/channels to log and skip.

**Patterns:**
- Public crates define `thiserror` enums such as provider, memory, MCP, channel, plugin, sandbox, and protocol errors; callers can classify retryability or policy denial by variant (`crates/wcore-providers/src/lib.rs`, `crates/wcore-memory/src/error.rs`).
- `wcore-cli` and bootstrap use `anyhow` for multi-layer context and process-level failure reporting (`crates/wcore-cli/src/main.rs`, `crates/wcore-agent/src/bootstrap.rs`).
- Provider failures stream as `ProviderError`; terminal host failures become `ProtocolEvent::Error` or `OutputSink::emit_error` without discarding durable session state (`crates/wcore-protocol/src/events.rs`).
- Optional plugin/MCP/channel discovery logs errors and continues only when the failed component is not required for session authority (`crates/wcore-agent/src/bootstrap.rs`).
- Unknown/running durable tool effects require explicit reconciliation instead of automatic replay (`crates/wcore-agent/src/engine.rs`).

## Cross-Cutting Concerns

**Logging:** Use `tracing` throughout; `wcore-cli` installs the subscriber, sends headless logs to stderr, and sends TUI logs through a non-blocking file writer (`crates/wcore-cli/src/main.rs:962`, `crates/wcore-observability/src/lib.rs`).
**Validation:** Deserialize into typed configs/protocol enums, validate paths and workspace trust before execution, sanitize provider tool schemas, and re-check effect/recovery digests at durable boundaries (`crates/wcore-config/src/config.rs`, `crates/wcore-tools/src/path_validation.rs`, `crates/wcore-agent/src/session_journal.rs`).
**Authentication:** Resolve provider/channel/MCP credential references through `wcore-config` credential/keychain abstractions; keep token values out of long-lived config and protocol payloads (`crates/wcore-config/src/credentials.rs`, `crates/wcore-config/src/keychain.rs`, `crates/wcore-config/src/mcp_cred_refs.rs`).

---

*Architecture analysis: 2026-07-18*
