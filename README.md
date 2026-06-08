<!-- ───────────────────────────────────────────────────────────────────────────
     HERO HEADER — drop your branded hero image / banner here.
     (Sean fills this on GitHub with the Wayland Core hero in the house style.)
     Suggested: full-width banner + orbit mark, Hearth palette.
─────────────────────────────────────────────────────────────────────────── -->

<div align="center">

# Wayland Core

**The open-source Rust engine for autonomous LLM agents — in your terminal, or embedded in your app.**

Multi-provider. Tool-using. MCP-native. Built to run unattended and not lose your data.

[![npm](https://img.shields.io/npm/v/@ferroxlabs/wayland-core?label=npm&color=e85d2a)](https://www.npmjs.com/package/@ferroxlabs/wayland-core)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue)](LICENSE)
[![platforms](https://img.shields.io/badge/platforms-macOS%20%C2%B7%20Linux%20%C2%B7%20Windows-555)](#install)
[![built with Rust](https://img.shields.io/badge/built%20with-Rust-dea584)](https://www.rust-lang.org/)

</div>

---

Wayland Core connects to any major LLM provider, autonomously invokes local tools (files, shell, search, sub-agents), talks to [MCP](https://modelcontextprotocol.io/) servers, and drives a task end-to-end — from a single command, an interactive TUI, or as a headless engine behind a host app. It's the engine that powers **[Wayland](https://getwayland.com)** (the desktop app); this repo is the core, on its own, open.

> **Wayland Core** = the CLI + Rust engine (this repo). **Wayland Desktop** = the full GUI product built on it.

## Screenshots

<!-- TUI captures live in docs/img/ — generated from the real interface. -->

<div align="center">

| Interactive TUI | Agent at work |
|:---:|:---:|
| ![Wayland Core workspace](docs/img/tui-workspace.png) | ![Wayland Core running a task](docs/img/tui-session.png) |

</div>

## Install

**npm (recommended — grabs the right prebuilt binary for your platform):**

```bash
npm install -g @ferroxlabs/wayland-core
wayland-core --version
```

```bash
# or run without installing
npx @ferroxlabs/wayland-core "summarize the TODOs in this repo"
```

**Prebuilt binaries:** download for macOS (arm64/x64), Linux (arm64/x64), or Windows (arm64/x64) from [Releases](https://github.com/FerroxLabs/wayland-core/releases).

**From source (Rust 1.95+):**

```bash
cargo install --git https://github.com/FerroxLabs/wayland-core wcore-cli
# or, in a clone:
cargo build --release    # → target/release/wayland-core
```

## Quick start

```bash
# 1. Generate a config, then add an API key (any provider)
wayland-core --init-config
wayland-core --config-path        # shows where it lives

# 2. One-shot
wayland-core "Read Cargo.toml and explain the dependencies"

# 3. Interactive TUI (just run it)
wayland-core

# 4. Everything else
wayland-core --help
```

Point it at Anthropic, OpenAI (or any OpenAI-compatible endpoint — DeepSeek, Qwen, Ollama, Gemini, vLLM), AWS Bedrock, or Google Vertex AI. No code changes — provider differences are handled by configuration, not hardcoded branches.

## Why Wayland Core

- **Multi-provider, zero quirks in your code** — Anthropic, OpenAI + compatibles, Bedrock, Vertex. Every provider difference (token-field names, message-merge rules, schema sanitization) lives in a `ProviderCompat` config layer, never in `if base_url.contains(...)`.
- **Real tool use** — built-in **Read, Write, Edit, Bash, Grep, Glob, Spawn** (sub-agents), each with streaming output and an approval gate.
- **MCP-native** — connect any Model Context Protocol server over stdio / SSE / streamable-HTTP. Hosts can inject MCP servers at runtime over the [JSON stream protocol](docs/json-stream-protocol.md).
- **Skills** — named, reusable prompt snippets with variable substitution, shell expansion, conditional activation, and per-skill model/permission overrides. [→ docs/skills.md](docs/skills.md)
- **Hooks** — event-driven automation on the tool lifecycle (auto-format, lint, audit, block).
- **Sub-agent spawning** — fan out parallel work through the `Spawn` tool.
- **Persistent memory** — per-project memory, auto-indexed and recalled across sessions.
- **Scheduled tasks (cron)** — schedule recurring agent runs; integrity-checked job store.
- **Session persistence** — save and resume full conversation history.
- **Plan mode** — read-only exploration to design an implementation before touching anything.
- **Context compression** — three-tier automatic compaction (micro / auto / emergency) so long sessions don't fall off a cliff.
- **Prompt caching** — Anthropic `cache_control` for up to ~90% cost reduction.
- **Security on by default** — an egress gate blocks exfil-shaped traffic to non-allowlisted hosts; tool sandboxing (bwrap / sandbox-exec / AppContainer); shell-injection-safe argv execution.
- **Embeddable** — a clean JSON-Lines protocol (`--json-stream`) makes it the engine behind a host app (it's how Wayland Desktop drives it).

## Providers

| Provider | Auth | Notes |
|----------|------|-------|
| **Anthropic** | API key / OAuth (Claude.ai) | Prompt caching, streaming, vision |
| **OpenAI** | API key | Reasoning models (`o1`/`o3`); compatible with DeepSeek, Qwen, Ollama, Gemini, vLLM |
| **AWS Bedrock** | SigV4 | Regional endpoints, AWS credential chain, schema sanitization |
| **Google Vertex AI** | GCP OAuth2 / service account | Metadata-server auto-detection |

Named **profiles** with `extends` let you switch provider/model in one flag. OAuth means you can use a Claude.ai subscription directly — no API key required. [→ docs/providers.md](docs/providers.md)

## ProviderCompat — the no-hardcoded-quirks rule

Provider-specific behavior is data, not code:

```toml
[providers.my-openai.compat]
max_tokens_field = "max_completion_tokens"   # field name for max tokens
merge_assistant_messages = true              # merge consecutive assistant messages
clean_orphan_tool_calls  = true              # drop tool_use without a tool_result
sanitize_schema          = false             # Bedrock-style schema sanitization
strip_patterns           = ["<think>", "</think>"]
api_path                 = "/v1/chat/completions"
```

Sensible defaults per provider type; override any field. Adding support for a new OpenAI-compatible endpoint is usually a config block, not a patch.

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                  wayland-core  (CLI · TUI · JSON-stream)      │
├──────────────────┬───────────────────────┬───────────────────┤
│  Config          │  Engine (agent loop)  │  Session Manager  │
│  (3-level merge) │  streaming + tools    │  save / resume    │
├──────────────────┼───────────────────────┼───────────────────┤
│  Providers       │  Tool Registry        │  Hook Executor    │
│  ├ Anthropic     │  ├ Built-in (7)       │  ├ pre_tool_use   │
│  ├ OpenAI(+compat)│  ├ MCP tools (N)     │  ├ post_tool_use  │
│  ├ Bedrock       │  └ Plan-mode tools    │  └ stop           │
│  └ Vertex AI     │                       │                   │
│  ProviderCompat  │  MCP Client           │  Memory (per-proj)│
│  Compact Engine  │  (stdio/SSE/HTTP)     │  Sub-agent Spawner│
│  (micro/auto/em) │  Egress security gate │  Cron scheduler   │
└──────────────────┴───────────────────────┴───────────────────┘
```

A workspace of focused crates (`wcore-types`, `wcore-providers`, `wcore-tools`, `wcore-mcp`, `wcore-skills`, `wcore-memory`, `wcore-agent`, `wcore-cli`, …); dependencies flow strictly downward.

## Embedding it (host integration)

Run it headless and drive it over JSON Lines:

```bash
wayland-core --json-stream
```

The host sends `Message` / `SetConfig` / `SetMode` / `Stop` commands and receives a typed event stream (`text_delta`, `tool_request`, `tool_result`, `config_changed`, `stream_end`, …) — including an honest `retryable` flag on errors and a mid-turn `Stop` that cleanly terminates the turn. This is exactly how Wayland Desktop embeds the engine. [→ docs/json-stream-protocol.md](docs/json-stream-protocol.md)

## Documentation

| Document | Covers |
|----------|--------|
| [Getting Started](docs/getting-started.md) | Install, CLI reference, config & cascading precedence |
| [Providers & Auth](docs/providers.md) | Multi-provider setup, ProviderCompat, profiles, OAuth |
| [Built-in Tools](docs/tools.md) | The seven tools and the execution flow |
| [Skills](docs/skills.md) | Front matter, shell expansion, conditional activation |
| [MCP Integration](docs/mcp.md) | Transport types, deferred loading, runtime injection |
| [Advanced](docs/advanced.md) | Sub-agents, hooks, memory, plan mode, compaction |
| [JSON Stream Protocol](docs/json-stream-protocol.md) | Host integration protocol spec |
| [Troubleshooting](docs/troubleshooting.md) | Common errors and fixes |

## Contributing

Issues and PRs welcome. Before a PR: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo nextest run`. Keep changes surgical and provider differences in `ProviderCompat`, never hardcoded.

## License

[Apache-2.0](LICENSE). Wayland Core is a derivative work; see [NOTICE](NOTICE) for upstream attribution.

<div align="center">
<sub>Part of the Forge Suite · <a href="https://getwayland.com">getwayland.com</a></sub>
</div>
