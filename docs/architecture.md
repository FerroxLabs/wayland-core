# Wayland-Core Architecture

> Single source of truth for the engine's layered architecture, cross-crate
> invariants, and substrate boundaries. Read alongside
> [AGENTS.md](../AGENTS.md) (conventions and rules for agents working on
> the code) and the per-crate `README.md` / module docs.

## 1. Layer map

Wayland-Core is a Cargo workspace of ~49 internal `wcore-*` crates plus 5
`wayland-*` plugin crates (54 total). Dependencies flow **downward** in the
diagram below — never introduce circular or upward references. The full
crate table with one-line responsibilities lives in
[AGENTS.md §Crate Map](../AGENTS.md#crate-map).

> The diagram below shows only the **load-bearing spine** plus the substrate
> band — it is not an exhaustive crate listing. For the live, complete set run
> `cargo metadata` or read [AGENTS.md §10](../AGENTS.md#10-project-context--wayland-core).

```
                           ┌──────────────────┐
                           │   wcore-cli      │  (binary entry point)
                           └────────┬─────────┘
                                    ▼
                           ┌──────────────────┐
                           │   wcore-agent    │  (session orchestration)
                           └────────┬─────────┘
                                    ▼
   ┌──────────────┬──────────────┬──────────────┬──────────────┬──────────────┐
   │ wcore-       │ wcore-       │ wcore-       │ wcore-       │ wcore-       │
   │ providers    │ tools        │ mcp          │ skills       │ memory       │
   ├──────────────┼──────────────┼──────────────┼──────────────┼──────────────┤
   │ wcore-       │ wcore-       │ wcore-       │ wcore-       │ wcore-       │
   │ permissions  │ observability│ browser      │ cua          │ eval/evolve  │
   └──────────────┴──────┬───────┴──────────────┴──────────────┴──────────────┘
                         ▼
              ┌──────────────────────────────────┐
              │ wcore-config (cross-platform     │
              │ shell, ProviderCompat, auth)     │
              └────────────┬─────────────────────┘
                           ▼
              ┌──────────────────────────────────┐
              │ wcore-protocol (JSON stream      │
              │ envelope) + wcore-plugin-api     │
              │ (mirror types, isolation)        │
              └────────────┬─────────────────────┘
                           ▼
              ┌──────────────────────────────────┐
              │ wcore-types + wcore-compact      │
              │ (provider-neutral data types,    │
              │  zero internal deps)             │
              └──────────────────────────────────┘

  Substrate band (peers of the mid tier, grouped by concern — not every
  crate is drawn on the spine above):
    security/sandbox  : wcore-sandbox, wcore-egress, wcore-safety,
                        wcore-permissions
    economics         : wcore-budget, wcore-pricing
    messaging/channels: wcore-channels, wcore-channels-registry, and the
                        wcore-channel-* connectors (discord, email,
                        imessage, matrix, msteams, signal, slack, sms,
                        telegram, whatsapp)
    plugin runtimes   : wcore-plugin-subprocess, wcore-plugin-wasm
    host integration  : wcore-acp, wcore-dispatch, wcore-swarm
    misc              : wcore-cron, wcore-replay, wcore-user-model,
                        wcore-eval-scenarios, wcore-fixture-harness,
                        wcore-honcho-adapter, wcore-agents-pack

  wcore-repomap is a deliberate island — NO internal `wcore-*` deps.

  Plugins (wayland-ollama, wayland-browser, wayland-cua, wayland-ijfw,
  wayland-honcho) sit OFF the main spine. They depend only on
  wcore-plugin-api + wcore-protocol — never on wcore-browser, wcore-cua,
  wcore-mcp, wcore-memory, or wcore-skills directly (audit F2).
```

The mid-tier crates fan out from `wcore-agent` and converge on
`wcore-config` / `wcore-protocol` / `wcore-types` at the bottom. New
functionality must land in the **lowest crate where it semantically
belongs** — see [§4 Where to put new code](#4-where-to-put-new-code).

`wcore-agent` also hosts the **ForgeFlows (Dynamic Workflows)** engine
(`crates/wcore-agent/src/orchestration/workflow/`): declarative RON lowers
to the existing `GraphConfig` IR and executes via `WorkflowRunner` over the
spawner path — see [docs/workflows.md](workflows.md).

## 2. Cross-crate invariants

These rules are load-bearing. Every PR is reviewed against them; several
are mechanically enforced.

| Rule | What it means | Enforced by |
|------|---------------|-------------|
| **Audit F2** (plugin isolation) | Plugin crates (`wayland-*`) MUST NOT depend on `wcore-browser`, `wcore-cua`, `wcore-mcp`, `wcore-memory`, or `wcore-skills`. They use mirror types from `wcore-plugin-api` instead. `wcore-agent` hosts `HostBrowserRegistrar` / `HostCuaRegistrar` to bind the mirror specs to the real backends without leaking the dep. | `build.rs` lint in `wcore-plugin-api`; Cargo manifest review |
| **`ProviderCompat` discipline** | No hardcoded provider conditionals (`if base_url.contains("api.openai.com")`). Provider differences live in `ProviderCompat` config fields with provider-specific defaults (`openai_defaults()`, `anthropic_defaults()`, etc.). | AGENTS.md §Architecture Principles; clippy + review |
| **Cross-platform shell** | All process spawning goes through `wcore_config::shell` (argv mode for LLM-supplied data, shell-string mode only when shell semantics are the contract). Never `Command::new("sh"/"bash"/"cmd")` directly. | AGENTS.md §Cross-Platform; clippy lint |
| **Observability decoupling** | `wcore-observability` sits between `wcore-types`/`wcore-config` and `wcore-agent`. `wcore-protocol` stays decoupled via opaque `serde_json::Value` payloads — the protocol crate never imports trace types directly. | dep graph; manual review |
| **`wcore-repomap` isolation** | Has NO internal `wcore-*` deps. Re-usable as a standalone library; the Aider-style symbol extractor is intentionally self-contained. | Cargo manifest review |
| **Top-down deps** | New code goes in the LOWEST crate where it semantically belongs. Never copy-paste across crates — extract to the right layer instead. | AGENTS.md §No Duplicate Code |
| **No interpolated shell strings** | In shell-string mode, never `format!`-interpolate LLM-supplied data into the command string. Every such site is a shell injection (the class closed during Wave SA). | Review + the existing audit log |

If you find code that violates an invariant, treat it as a bug. Don't add
a sibling violation to be consistent — fix the original.

## 3. Substrate boundaries

Wayland-Core has **four substrate systems** that overlap conceptually but
remain peers, not subsets. This boundary discipline is locked per the
project rule `feedback-dont-overextend-locked-decisions`: "Memory
substrate = IJFW" applies to **STORAGE only**. Skill curation, GEPA
evolution, and Honcho user-modeling are peer substrates, not subsets of
IJFW or of `wcore-memory`.

### 3.1 IJFW — unified STORAGE substrate

**Scope:** key-value persistence, vector storage, BM25 indexing,
cross-session recall under one consistent API. IJFW is the **bottom-most**
substrate — every other system can use it for durable bytes, but none of
them defer their *domain logic* to it.

**Where it lives:** `.ijfw/` at project root (per-project state) and
`~/.claude/projects/<id>/memory/` (cross-session memory files). The MCP
server `mcp__plugin_ijfw_ijfw-memory__*` exposes the read/write surface.
The `wayland-ijfw` plugin crate is the engine's in-tree anchor — it
exercises every `register_*` surface (tools + hooks + agents + skills +
rules + MCP server) through `wcore-plugin-api` mirror types.

**What IJFW does NOT own:**
- The engine's smart memory tier logic (gate, audit, decay).
- Skills curation, conditional activation, and learning.
- GEPA prompt evolution and child scoring.
- Honcho user-model inference.

Each of those is a peer substrate. IJFW gives them durable bytes; it
does not give them their domain model.

**Anti-pattern:** "since IJFW has vector + BM25 + KV, route every
read/write through `mcp__plugin_ijfw_ijfw-memory__memory_search`." That
collapses the smart layer above into a thin client and loses partition
routing, the gate, the audit log, and the decay scheduler. The engine's
domain types stay in the engine.

### 3.2 wcore-memory — smart MEMORY substrate (above IJFW)

**Scope:** the engine's 5-partition × 3-tier memory model, gate, audit
log, CDC, embedder, dream-cycle consolidation, decay scheduler.

**Relationship to IJFW:** `wcore-memory` MAY use IJFW as a storage
backend for episodic and semantic tiers. It does NOT inherit IJFW's API
surface — the engine's domain types (`MemoryItem`, `Tier`, `Partition`)
are defined in `crates/wcore-memory` and remain stable across storage
backends. Swap IJFW for a different KV/vector store and the
`wcore-memory` public API does not change.

**Anti-pattern:** "since IJFW is the substrate, delete `MemoryItem` and
return IJFW records directly from `wcore-memory`." That couples every
caller to a single backend's serialization. Keep the boundary.

### 3.3 wcore-skills + wcore-evolve + wcore-eval — LEARNING substrate

**Scope:** the engine's procedural intelligence — skill discovery,
curation, conditional activation, GEPA prompt evolution (child
generation, paraphrase mutation, retention), and the eval scoring gate
(precision / recall thresholds).

**Relationship to IJFW and `wcore-memory`:**
- Skill **artifacts** (text bodies, frontmatter, `!shell:` directives)
  live in their own `skills/` directories on disk — bundled in
  `crates/wcore-skills/src/bundled/` and user-installed under
  `~/.wayland/skills/`.
- Skill **telemetry** (usage counts, last-used timestamps, success
  rates) lands in `wcore-memory`'s procedural tier — the memory crate
  is the right home for usage state.
- Evolved prompt **variants** persist in a `prompt_store` owned by
  `wcore-evolve` — sibling-crate to `wcore-memory`, not a subset.

**Anti-pattern:** "fold skills into `wcore-memory` because it also
persists state." Skills have their own lifecycle (load → discover →
condition → execute) that does not match the memory partition model.
Different concerns; different crates.

### 3.4 wayland-honcho — USER-MODELING substrate

**Scope:** Honcho-backed user-model inference. Captures preferences,
history, and inferred attributes ABOUT the user across sessions.

**Relationship to `wcore-memory`:** `wcore-memory` already owns
`partition/p5_user_model` (see
`crates/wcore-memory/tests/p5_user_model_inference.rs`). The Honcho
client populates that partition; it does NOT replace it. Honcho is a
*backend* for user-model facts; the `p5` partition is the engine's
domain model of "things the engine knows about its user," with stable
types that survive a Honcho swap.

**Anti-pattern:** "since Honcho is the user model, delete the p5
partition and call Honcho directly from `wcore-agent`." That collapses
the engine's user-model abstraction into a single vendor's API and
makes the engine unrunnable when Honcho is offline.

### 3.5 Summary diagram

```
┌─────────────────────────────────────────────────────────────┐
│                    wcore-agent (session)                    │
└────┬──────────┬───────────┬──────────────┬──────────────────┘
     ▼          ▼           ▼              ▼
  ┌────────┐ ┌────────┐ ┌────────────┐ ┌──────────────┐
  │ wcore- │ │ wcore- │ │  wcore-    │ │  wayland-    │
  │ memory │ │ skills │ │  evolve    │ │  honcho      │
  │ (smart │ │ +eval  │ │  (GEPA     │ │  (user-model │
  │ tier)  │ │(learn) │ │  prompts)  │ │   backend)   │
  └───┬────┘ └────┬───┘ └─────┬──────┘ └──────┬───────┘
      │           │           │               │
      └───────────┴────┬──────┴───────────────┘
                       ▼
              ┌──────────────────────────────────┐
              │  IJFW (storage substrate)        │
              │  - KV  - vector  - BM25          │
              │  - cross-session memory files    │
              └──────────────────────────────────┘
```

Each of the four substrates (`wcore-memory`, `wcore-skills` +
`wcore-evolve` + `wcore-eval`, `wayland-honcho`, and IJFW) has its own
API, its own tests, and its own domain types. They **compose**; they do
not subsume each other.

## 4. Where to put new code

| You're adding... | Goes in... |
|------------------|------------|
| A new provider (LLM API surface) | `wcore-providers` + a new `ProviderCompat` preset in `wcore-config` |
| A new built-in tool | `wcore-tools` (registered via `register_tools` in agent bootstrap) |
| A new memory partition or tier | `wcore-memory/src/partition/` or `wcore-memory/src/tier.rs` |
| A new skill | `crates/wcore-skills/src/bundled/` (bundled) or user's `~/.wayland/skills/` |
| A new MCP server consumer | `wcore-mcp` |
| A new plugin (3rd-party LLM, alt tool family, custom hook) | New `wayland-<name>` crate using `wcore-plugin-api` mirror types — no `wcore-browser` / `wcore-cua` / `wcore-mcp` / `wcore-memory` / `wcore-skills` deps |
| A new evaluation metric | `wcore-eval` |
| A new prompt mutator (GEPA) | `wcore-evolve/src/mutator/` |
| A new permission rule | `wcore-permissions` |
| A new channel connector | `wcore-channel-<name>` (register via `wcore-channels-registry`) |
| A new shell sandbox backend | `wcore-sandbox` |
| A new egress / network rule | `wcore-egress` |
| A budget cap or pricing entry | `wcore-budget` / `wcore-pricing` |
| A new observability span / sink | `wcore-observability` |
| A new shell helper or platform-specific path | `wcore-config` (centralize, then call from anywhere) |
| A new JSON stream event type | `wcore-protocol` (and mirror in `wcore-plugin-api` if plugins need it) |
| A new provider-neutral data type | `wcore-types` (only if it's truly shared by 2+ mid-tier crates) |

When in doubt, prefer the lower layer. Functionality at the wrong layer
is the most common source of subsequent refactors.

## 5. Further reading

- [AGENTS.md](../AGENTS.md) — full conventions, build commands, code style, crate map
- [docs/getting-started.md](getting-started.md) — installation, CLI usage, config cascading precedence
- [docs/providers.md](providers.md) — provider setup, auth, `ProviderCompat`, aliases, profiles
- [docs/tools.md](tools.md) — built-in tool reference and execution flow
- [docs/skills.md](skills.md) — writing skills, front matter, shell expansion, conditional activation
- [docs/mcp.md](mcp.md) — MCP server integration, transport types, deferred loading
- [docs/advanced.md](advanced.md) — sub-agents, hooks, memory, plan mode, context compression
- [docs/json-stream-protocol.md](json-stream-protocol.md) — JSON Lines protocol for host integration (e.g. the Wayland desktop app)
- [docs/workflows.md](workflows.md) — ForgeFlows (Dynamic Workflows): RON → `GraphConfig` IR → `WorkflowRunner`
- [docs/wcore-evolve.md](wcore-evolve.md) — GEPA evolution loop deep dive
- [docs/troubleshooting.md](troubleshooting.md) — common errors and solutions
