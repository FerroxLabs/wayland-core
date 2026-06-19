<div align="center">

![Wayland Core — Forged to run. Hardened to last. Built to evolve.](docs/img/hero.png)

### The open-source Rust engine for autonomous LLM agents.

**Terminal-first. Multi-provider. MCP-native. Embeddable. Apache-2.0.**

[![npm](https://img.shields.io/npm/v/@ferroxlabs/wayland-core?style=for-the-badge&logo=npm&logoColor=white&label=npm&color=e85d2a)](https://www.npmjs.com/package/@ferroxlabs/wayland-core)
[![CI](https://img.shields.io/github/actions/workflow/status/FerroxLabs/wayland-core/ci.yml?style=for-the-badge&logo=githubactions&logoColor=white&label=CI&branch=main)](https://github.com/FerroxLabs/wayland-core/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-Apache--2.0-3b3b3b?style=for-the-badge)](LICENSE)
[![Rust](https://img.shields.io/badge/built_with-Rust-dea584?style=for-the-badge&logo=rust&logoColor=black)](https://www.rust-lang.org/)
[![platforms](https://img.shields.io/badge/macOS_·_Linux_·_Windows-2b2b2b?style=for-the-badge)](#install)
[![status](https://img.shields.io/badge/status-public_beta-e85d2a?style=for-the-badge)](#built-to-endure)

[Install](#install) · [Quick start](#quick-start) · [Providers](#providers--model-routing) · [Orchestration](#orchestration--swarms) · [Security](#security-by-default) · [Endurance](#built-to-endure) · [Embedding](#embedding-it) · [Docs](#documentation)

</div>

---

Wayland Core is a domain-agnostic autonomous-agent engine written in Rust. It connects to any major LLM provider, invokes real local tools inside an OS-native sandbox, fans out parallel agent swarms, speaks [MCP](https://modelcontextprotocol.io/) in both directions, and drives a task end to end. It runs three ways from one binary: a one-shot command, a full-screen interactive TUI, or a headless engine embedded behind your own app.

> **Wayland Core** is the engine, on its own, open (this repo, Apache-2.0). **[Wayland Desktop](https://getwayland.com)** is the GUI product built on it. Core is the engine; Desktop is one application that embeds it.

## The 30-second proof

```bash
npx @ferroxlabs/wayland-core "read Cargo.toml, list the workspace crates, and explain the dependency layering"
```

One command. The agent reads the file, runs `grep`/`glob` across the tree, reasons, and answers, with every tool call gated and streamed. Or run `wayland-core` with no arguments and it detects your provider keys and drops you into the TUI:

<div align="center">

![Wayland Core — connect a provider](docs/img/screenshot-onboarding.png)

</div>

**Paste a key, get a provider.** Paste an API key (or run `/connect` in the TUI) and the engine fingerprints the provider from the key's shape, validates it live, and stores it in your OS keyring. From there, `/config` exposes Essentials and Advanced editors, `/doctor` shows provider, key, and MCP health, and `/effective` prints the resolved config with secrets redacted.

## What it is

- **A standalone engine.** The engine is the product, not a feature bolted onto an editor and not a wrapper around one vendor's API.
- **Terminal-first.** A one-shot command, an interactive TUI, or a headless stream. The terminal is the primary home, not an afterthought.
- **Embeddable.** Drive it from your own app over a typed JSON-Lines protocol. It is exactly how Wayland Desktop uses it.
- **Apache-2.0.** Permissive. Build on it commercially without an AGPL obligation.

## Install

**npm** (recommended, pulls the right prebuilt binary for your platform):

```bash
npm install -g @ferroxlabs/wayland-core
wayland-core --version
```

```bash
# or run it once, no install
npx @ferroxlabs/wayland-core "summarize the TODOs in this repo and draft a triage plan"
```

**Prebuilt binaries** for macOS (arm64/x64), Linux (arm64/x64), and Windows (arm64/x64) are on the [Releases](https://github.com/FerroxLabs/wayland-core/releases) page, each verifiable against `wayland-core-checksums.txt`.

**From source** (Rust 1.95+):

```bash
cargo install --git https://github.com/FerroxLabs/wayland-core wcore-cli
```

## Quick start

```bash
# 1. Generate a config, then add an API key for any provider
wayland-core --init-config
wayland-core --config-path        # shows where the config lives

# 2. One-shot: the agent reads files and uses tools to answer
wayland-core "Read Cargo.toml and explain the dependencies"

# 3. Interactive TUI (just run it)
wayland-core

# 4. Everything else
wayland-core --help
```

---

## Providers & model routing

Most terminal agents are married to one vendor. Wayland Core is built provider-neutral from the foundation up: the engine only ever sees provider-neutral types, and every vendor's quirks are pushed into a configuration layer instead of the code.

- **~20 first-class provider integrations**, written in-tree: Anthropic, OpenAI, Google Gemini, Google Vertex AI, AWS Bedrock (real SigV4 signing and event-stream framing), Cohere, Azure OpenAI, plus Groq, DeepSeek, Mistral, Together, Fireworks, xAI, Qwen, Moonshot, MiniMax, Nvidia, Cerebras, Perplexity, OpenRouter, and Ollama for local models.
- **Sign in with ChatGPT** (OAuth, no API key): `wayland-core auth login chatgpt`, then run with `--provider openai-chatgpt`.
- **A 104-entry [models.dev](https://models.dev) catalog** on top of those, so 100+ more endpoints are selectable by id with no code change.
- **Live model discovery** for connected providers — Bedrock `ListFoundationModels`, Gemini, and OpenAI-compatible `list_models` — cached on disk for 24h and surfaced in the arrow-key `/model` and `/provider` pickers.
- **Switching vendors is a config change, not a fork.** There is no `if base_url.contains("openai.com")` anywhere in the codebase.

**ProviderCompat** is the rule that makes that true. Point `base_url` at any OpenAI-compatible endpoint and describe its differences as data:

```toml
[providers.my-openai.compat]
max_tokens_field = "max_completion_tokens"   # field name for max tokens
merge_assistant_messages = true              # merge consecutive assistant messages
clean_orphan_tool_calls  = true              # drop tool_use without a tool_result
sanitize_schema          = false             # Bedrock-style schema sanitization
strip_patterns           = ["<think>", "</think>"]
```

Resilience is on by default. Every provider call is wrapped in a circuit breaker, with automatic retry, mid-stream reconnect (a TLS drop after headers is retried, not treated as fatal), multi-key rotation with cooldown demotion, and cross-provider failover when you configure a fallback chain.

<div align="center">

![Providers and model routing — Wayland Core vs OpenClaw, Hermes, opencode, aider](docs/img/compare-providers.png)

</div>

## Orchestration & swarms

A single agent is the floor, not the ceiling. Wayland Core fans work out across many agents and brings the results back, with real isolation between workers.

- **Sub-agents (`Spawn`)** fan parallel work out from one tool call, each with its own turn loop. Concurrency caps scale by tier.
- **Worktree swarm** runs N workers as subprocesses, each in its own fresh git worktree on its own branch, with a dirty-checkout guard that refuses to dispatch on an uncommitted tree, per-worker timeouts, and idempotent cleanup. Process isolation, not threads, so one bad worker cannot corrupt another.
- **ForgeFlows** are declarative RON workflows that lower onto the engine's own execution graph, so stages are real sub-agents rather than a separate interpreter. Stages are schema-validated and self-retrying: a stage whose output fails its schema is re-dispatched with the error, and completed stages are never discarded on a later failure.
- **Selectable reducers.** Roll worker results up with `wayland swarm --reduce mesh|fleet|consensus|debate` — majority consensus, multi-round debate, or a plain fan-in.

<div align="center">

![Fleet spawn fan-out — one orchestrator, isolated workers, merged result](docs/img/diagram-swarm.png)

</div>

<div align="center">

![Orchestration, compared](docs/img/compare-orchestration.png)

</div>

## Security by default

Security is a default, not a setting, and it is built to hold up when someone reads the source.

- **Fail-closed OS-native sandbox.** Shell runs inside bubblewrap (Linux), `sandbox-exec` (macOS), or AppContainer (Windows). With no working sandbox and no explicit opt-out, model-driven commands are refused, not run unsandboxed.
- **Egress chokepoint enforced by CI.** Every outbound request goes through one client; a lint bans constructing a raw HTTP client, so a missed migration fails the build. An exfil-shape classifier hard-denies suspicious POSTs and high-entropy paths to non-allowlisted hosts, and shared multi-tenant suffixes can never be apex-allowlisted.
- **SSRF and metadata floor, always on.** Cloud-metadata endpoints and non-standard IP encodings are rejected, and the resolved IP is re-validated at connect time to close DNS-rebinding races.
- **Injection-safe shell.** Tool arguments are passed in argv mode, so shell metacharacters reach the child as literal bytes, never interpreted.
- **Crash-safe.** Your prompt is journaled to a write-ahead log before the model sees it, so a `SIGKILL` mid-turn does not lose it. File edits are checkpointed for rollback. Every tool call is approval-gated, with scoped allow/deny.

<div align="center">

![Security: fail-closed — sandbox plus one CI-enforced egress gate](docs/img/diagram-security.png)

</div>

<div align="center">

![Security and sandboxing, compared](docs/img/compare-security.png)

</div>

## Built to endure

Most agent demos prove an agent can finish a task. The harder question is whether one can run unattended for a long time, on its own codebase, surviving crashes and injected faults without ever drifting or corrupting its state. We are putting Wayland Core through exactly that, as an open endurance trial.

<div align="center">

![Resilience under fire — WAL, retry, and checkpoint recovering through deliberate kills](docs/img/diagram-resilience.png)

</div>

**Measured so far** (one continuous 12-hour unattended run):

- 322 maintenance iterations, 229 accepted and committed (~71%), each gated on a real compile + lint pass before entering history.
- Survived a `SIGKILL` injected mid-build: it restarted and resumed on its own, with zero duplicate commits, zero lost commits, and a clean tree.
- No degradation across the window. Acceptance held a stable equilibrium; memory, disk, and cache stayed flat.
- Single-digit USD for the 12 hours, measured from the provider's raw usage records, not a self-reported number.
- A separate fault-injection suite kills the process inside every sensitive window of the commit path: **80 of 80 recovered with zero duplicate commits.**

**What we do not claim.** This is not recursive self-improvement; the build is pinned and nothing rewrites itself. Uptime is not the metric, we count gate-passing commits. One clean run is not proof of a week. A continuous week, then a month, are roadmap goals, labeled as goals. [→ docs/resilience.md](docs/resilience.md)

## Memory & sessions

Per-project long-term memory, indexed with full-text search and local vector embeddings, recalled across sessions with a consolidation lifecycle and decay so old context fades instead of piling up. Full session save and resume. A read-only **plan mode** to design an approach before touching anything. Automatic context compaction so long sessions do not fall off a cliff. Prompt caching for up to ~90% cost reduction on supported providers.

## Self-evolution (GEPA)

Wayland Core ships a scored evolutionary optimizer: it generates variant prompts and skills, scores them against your own reference cases, and keeps the winners. It is behind explicit trust boundaries, and it is one of the few capabilities no sibling engine ships.

## Extensibility

- **MCP, both directions.** Connect to many MCP servers concurrently over stdio, SSE, or streamable-HTTP (a wedged server is skipped, not fatal), and inject servers at runtime mid-session. Wayland Core also **runs as an MCP server that advertises and executes its own built-in tools**, so another agent can drive it over MCP.
- **~70 built-in tools.** `Read`, `Write`, `Edit`, `Bash`, `Grep`, `Glob`, `Spawn` are the headline; the catalog also covers git, GitHub, Kubernetes, Postgres, PDF, and more. `Bash` runs network-denied with secrets scrubbed from its environment by default.
- **Skills.** Markdown plus YAML front matter, with path-glob conditional activation, forked context, a per-skill model pin, a per-skill tool allowlist, and shell-expansion directives.
- **Hooks.** Shell or native hooks on `pre_tool_use` / `post_tool_use` / `stop`; a pre-hook can block a tool call.
- **Plugins.** Register tools, hooks, agents, skills, rules, and MCP servers through a stable plugin API.

[→ docs/tools.md](docs/tools.md) · [→ docs/skills.md](docs/skills.md) · [→ docs/mcp.md](docs/mcp.md)

<div align="center">

![Built-in tools, compared — Wayland Core vs OpenClaw, Hermes, opencode, aider](docs/img/compare-tools.png)

</div>

<div align="center">

![Extensibility, compared](docs/img/compare-extensibility.png)

</div>

## Embedding it

Run it headless and drive it over JSON Lines:

```bash
wayland-core --json-stream
```

The host sends `Message` / `SetConfig` / `SetMode` / `Stop` commands and receives a typed event stream (`text_delta`, `tool_request`, `tool_result`, `config_changed`, `stream_end`), including an honest `retryable` flag on errors and a mid-turn `Stop` that cleanly ends the turn. This is exactly how Wayland Desktop embeds the engine. [→ docs/json-stream-protocol.md](docs/json-stream-protocol.md)

## Architecture

A workspace of focused crates. Dependencies flow strictly downward; the engine only ever sees provider-neutral types, and format conversion lives inside each provider.

<div align="center">

![One engine, many surfaces — CLI, TUI, JSON stream, and an embedded host all driving the same core](docs/img/diagram-architecture.png)

</div>

| Layer | Crates | Responsibility |
|-------|--------|----------------|
| Foundation | `wcore-types`, `wcore-compact` | Provider-neutral data types; context compression |
| Core services | `wcore-config`, `wcore-providers`, `wcore-tools`, `wcore-mcp` | Config + ProviderCompat, LLM providers, built-in tools, MCP |
| Capabilities | `wcore-skills`, `wcore-memory`, `wcore-sandbox`, `wcore-swarm`, `wcore-browser` | Skills, memory, sandbox, swarm, browser + computer use |
| Engine | `wcore-agent` | Agent loop, sessions, orchestration, workflows |
| Surface | `wcore-cli` | CLI / TUI / JSON-stream binary |

[→ AGENTS.md](AGENTS.md)

## How it compares

We ran a file-level audit of the open-source agent CLIs and a docs-level orientation against the closed ones. Where we lose, we say so (git auto-commit loops, for instance, belong to opencode and aider).

<!-- HEADLINE COMPARISON GRAPHIC: docs/img/compare-capabilities.png
![How Wayland Core compares](docs/img/compare-capabilities.png)
-->

<div align="center">

![Landscape comparison — Wayland Core vs opencode, aider, Claude Code, Codex CLI](docs/img/compare-capabilities.png)

</div>

Closed-source tools (Claude Code, Codex CLI) are a docs-based orientation, not a code audit. Where we lose, we say so: git auto-commit/undo belongs to opencode and aider.

## Documentation

| Document | Covers |
|----------|--------|
| [Getting Started](docs/getting-started.md) | Install, CLI reference, config and cascading precedence |
| [Providers & Auth](docs/providers.md) | Multi-provider setup, ProviderCompat, profiles |
| [Built-in Tools](docs/tools.md) | The tool catalog and execution flow |
| [Skills](docs/skills.md) | Front matter, shell expansion, conditional activation |
| [MCP Integration](docs/mcp.md) | Transport types, deferred loading, runtime injection |
| [Advanced](docs/advanced.md) | Sub-agents, hooks, memory, plan mode, compaction |
| [Resilience](docs/resilience.md) | The endurance trial: method, measurements, and honesty bounds |
| [JSON Stream Protocol](docs/json-stream-protocol.md) | Host integration protocol spec |

## Contributing

Issues and PRs welcome. Before a PR: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo nextest run`. Keep changes surgical, and keep provider differences in `ProviderCompat`, never hardcoded. [→ AGENTS.md](AGENTS.md)

## License

[Apache-2.0](LICENSE). Wayland Core is a derivative work; see [NOTICE](NOTICE) for upstream attribution.

<div align="center">
<sub>Part of the Forge Suite · <a href="https://getwayland.com">getwayland.com</a></sub>
</div>
