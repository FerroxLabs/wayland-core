# Documentation

## Getting started

- **[Getting Started](getting-started.md)** — Installation, CLI parameters, configuration files, usage examples, session management, REPL mode
- **[Advanced Features](advanced.md)** — Sub-agent spawning, hook system, prompt caching, VCR recording/replay, AGENTS.md hierarchical loading, plan mode, context compression, file state cache, output compaction
- **[Troubleshooting](troubleshooting.md)** — Common errors, diagnostics, and solutions

## Providers & routing

- **[Providers & Auth](providers.md)** — Multi-provider configuration, profile inheritance, AWS Bedrock, Google Vertex AI, OAuth login

## Tools & MCP

- **[Built-in Tools](tools.md)** — Read, Write, Edit, Bash, Grep, Glob, Spawn — detailed reference and concurrency notes
- **[MCP Integration](mcp.md)** — Model Context Protocol client: stdio, SSE, and streamable-http transports

## Orchestration

- **[ForgeFlows (Dynamic Workflows)](workflows.md)** — Declarative multi-stage workflow engine: RON definitions, graph lowering, the WorkflowRunner execution path
- **[Crucible](crucible.md)** — Cross-provider Mixture-of-Providers council: fan-out, fusion, and per-tier model selection

## Profiles & cost

- **[Isolated Profiles](profiles.md)** — Independent `WAYLAND_HOME`-rooted environments with separate config, credentials, memory, and skills, managed by the `wayland-core profile` command family
- **[Cost & Token-Spend Governance](cost-governance.md)** — Three independent cost-control mechanisms: spend caps, token budgets, and per-tier accounting

## Memory

- **[Memory Model](memory.md)** — The 5-partition × 3-tier SQLite-backed long-term memory layer

## Security

- **[Channels](channels.md)** — Inbound security model for chat-platform messages (Telegram, Discord, and others)

## Extensibility

- **[Skills](skills.md)** — Named prompt snippets the agent can invoke on demand: front matter, shell expansion, conditional activation
- **[Plugin Authors Guide](plugin-authors.md)** — Stable plugin API for contributing tools, hooks, providers, skills, rules, MCP servers, and user-model backends without forking
- **[Marketplace Allowlist Format](marketplace-format.md)** — v1.0 index format backing `wayland-core plugin install`
- **[wcore-evolve](wcore-evolve.md)** — The W10B GEPA skill-evolution loop: child generation, scoring, and retention

## Embedding

- **[JSON Stream Protocol](json-stream-protocol.md)** — Host integration protocol specification (`--json-stream` mode)

## Architecture

- **[Architecture](architecture.md)** — Cargo workspace crate map and dependency-graph layering
- **[Endurance & Resilience Trial](resilience.md)** — Ongoing experiment in long-horizon, unattended autonomous operation
- **[E2E Infrastructure](e2e.md)** — End-to-end test runner on DigitalOcean ephemeral droplets
