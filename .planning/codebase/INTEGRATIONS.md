# External Integrations

**Analysis Date:** 2026-07-23

## APIs & External Services

**LLM Providers** (`crates/wcore-providers/src/`):
- Anthropic (`anthropic.rs`, `anthropic_shared.rs`) — primary provider, largest implementation
- OpenAI (`openai.rs` — 232.9K, largest file in the crate; `openai_responses.rs`, `openai_chatgpt.rs`, `openai_compat.rs`, `openai_compatible.rs`)
- AWS Bedrock (`bedrock.rs`, 102.2K) — auth via `aws-sigv4` (exact-pinned) + `aws-config` + `aws-sdk-sts`
- Google Vertex AI (`vertex.rs`) — auth via `jsonwebtoken` (exact-pinned) service-account JWT
- Google Gemini (`gemini.rs`, 93.7K, direct API — separate from Vertex)
- Azure OpenAI (`azure_openai.rs`)
- Additional OpenAI-compatible / aggregator providers: `cerebras.rs`, `cohere.rs`, `deepseek.rs`, `fireworks.rs`, `groq.rs`, `mistral.rs`, `moonshot.rs`, `nvidia.rs`, `openrouter.rs`, `perplexity.rs`, `qwen.rs`, `sakana.rs`, `together.rs`, `xai.rs`
- Local inference: Ollama (`ollama_probe.rs` in `wcore-providers`; full provider registration lives in the `wayland-ollama` plugin crate)
- Flux image generation: `flux_fetch.rs`, `flux_image.rs`, `flux_router.rs`
- Provider-neutral surface: all providers implement a common `LlmProvider` trait; the engine only ever sees `LlmRequest`/`LlmEvent`/`Message`/`ContentBlock`. Provider quirks are NEVER hardcoded — they flow through the `ProviderCompat` config layer (set per-provider in preset functions like `openai_defaults()`), per AGENTS.md §"No Hardcoded Provider Quirks"
- Resilience layer: `failover.rs`, `failover_policy.rs`, `retry.rs` (54.3K), `resilient.rs` (57.0K), `cooldown.rs`, `chain.rs`, `routing.rs`, `key_rotation.rs`, `key_validation.rs` — multi-key rotation, cooldown-based failover chains, classification-driven retry
- `wcore-pricing` crate — provider × model token-rate catalog (pricing-as-data)

## MCP (Model Context Protocol)

**Client:** `crates/wcore-mcp/`
- Transports: stdio (`transport/stdio.rs`, 65.2K — largest, handles PATHEXT shim resolution for `.cmd`/`.bat` on Windows), SSE (`transport/sse.rs`, 40.6K), streamable-HTTP (`transport/streamable_http.rs`)
- `manager.rs` (68.1K) — MCP server lifecycle, deferred loading
- `forge_grant.rs`, `tool_proxy.rs`, `protocol.rs`, `server.rs` — capability grants and tool proxying for MCP-provided tools
- See `docs/mcp.md` for transport-type reference

## Plugin System

**Runtime hosts:**
- WASM Component Model: `crates/wcore-plugin-wasm` — `wasmtime` 30 + WASI sandboxed plugin execution
- Subprocess: `crates/wcore-plugin-subprocess` — JSON-RPC over stdio, reuses `wcore-mcp::protocol` framing
- Marketplace ingestion: `crates/wcore-pluginsrc` — install-time lowering of foreign plugin formats (Claude Code, raw MCP) into the native plugin model

**Isolation boundary:** `wcore-plugin-api` — plugins never depend directly on `wcore-browser`/`wcore-cua`/`wcore-skills`/`wcore-mcp`/`wcore-memory`; instead the host (`wcore-agent`) registers real backends behind mirror trait types (`HostBrowserRegistrar`, `HostCuaRegistrar`) so plugin crates like `wayland-browser`, `wayland-cua`, `wayland-ijfw`, `wayland-honcho` only see `wcore-plugin-api` — enforced by a `build.rs` lint (audit finding F2)

## Data Storage

**Databases:**
- SQLite (`rusqlite` 0.32, bundled + `sqlite-vec` 0.1) — long-term memory subsystem (`crates/wcore-memory`), vector search via sqlite-vec
- Postgres (`tokio-postgres` 0.7 + `tokio-postgres-rustls` 0.13) — read-only schema introspection tool backend (`postgres_schema_tool` in `crates/wcore-agent/src/tool_backends/`); TLS enforced (rustls, no native-tls/openssl); SSRF-hardened via private-range host rejection at connection-parse time; public CA trust store via `webpki-roots` (covers Supabase/RDS/Neon)

**File Storage:**
- Local filesystem only — no cloud object storage integration detected
- Skills, config, credentials vault all resolve through `dirs` (XDG/platform config dirs)

**Caching:**
- `lru` 0.18 — in-process file-state tracking cache
- Provider prompt-cache discipline handled in `wcore-observability` (trace schema tracks cache hits/misses for cost accounting)

**Local ML inference:**
- `candle-core`/`candle-nn`/`candle-transformers` — bge-small-en-v1.5 embedding model, fetched from HuggingFace Hub on first use, cached at `~/.cache/huggingface` (gated behind `wcore-memory`'s `bge-local` feature)

## Authentication & Identity

**Provider auth:**
- API keys — OS keychain (`keyring`, exact-pinned) or encrypted-file vault (Argon2id + XChaCha20-Poly1305, exact-pinned) as fallback
- AWS Bedrock — SigV4 request signing (`aws-sigv4`, exact-pinned) + `aws-config`/`aws-sdk-sts` for credential resolution
- Google Vertex AI — GCP service-account JWT (`jsonwebtoken`, exact-pinned)
- OAuth (Google Meet and similar): localhost redirect-URI flow via `hyper`/`hyper-util`, dual-stack `[::]` bind so both `localhost`→`::1` and `127.0.0.1` resolutions work; CSRF `state` token compared in constant time via `subtle`

**Plugin signing:**
- `ed25519-dalek` (exact-pinned) — optional plugin package signature verification (default off)

## Monitoring & Observability

**Tracing:**
- `crates/wcore-observability` — trace schema, span sinks, OTLP exporter; sits between `wcore-types`/`wcore-config` and `wcore-agent`; stays decoupled from `wcore-protocol` via opaque `serde_json::Value` payloads (avoids upward coupling)
- `tracing` + `tracing-subscriber` (`env-filter`, `RUST_LOG`-controlled) + `tracing-appender` (non-blocking file writer for TUI mode)

**Error Tracking:** Not detected (no Sentry/Bugsnag-style external error-tracking SDK in the dependency tree)

**Vulnerability scanning:**
- `cargo-audit` (via `taiki-e/install-action` in CI) + OSV scan (`.github/workflows/osv-scan.yml`)

## CI/CD & Deployment

**CI Pipeline:** GitHub Actions, `.github/workflows/`:
- `ci.yml` (23.1K) — main build/test/lint matrix (macOS, Linux, Windows)
- `e2e.yml` — live-API E2E tests (requires `ANTHROPIC_API_KEY`/`OPENAI_API_KEY`)
- `nightly-windows-soak.yml` — extended Windows soak testing
- `mutants-nightly.yml` — mutation testing
- `bench-regression.yml` — `criterion` benchmark regression gate
- `marketplace-drift.yml` — plugin marketplace contract drift check
- `osv-scan.yml` — dependency vulnerability scan
- `release.yml` + `release-please.yml` — automated release/changelog pipeline

**Hosting:** Distributed as a Rust CLI binary (not a hosted service); no PaaS/cloud hosting integration detected for the core engine itself

## Chat/Communication Channels

**Channel adapters** (`crates/wcore-channel-*`, dispatched via `wcore-channels` + `wcore-channels-registry` which auto-scans `~/.wayland/channels/*.toml` at boot):
- Slack (`wcore-channel-slack`) — Web API outbound + Events API webhook inbound
- Telegram (`wcore-channel-telegram`) — `sendMessage` outbound + `getUpdates` long-poll inbound, bot token in keychain
- Discord (`wcore-channel-discord`) — REST `channels/<id>/messages` outbound + Gateway WebSocket v10 inbound (HELLO/HEARTBEAT_ACK lifecycle, `GUILD_MESSAGES` + `MESSAGE_CONTENT` intents), bot token in keychain
- SMS (`wcore-channel-sms`) — Twilio REST outbound + webhook inbound with HMAC-SHA1 signature verification
- Email (`wcore-channel-email`) — SMTP outbound via `lettre`, IMAP poll inbound on `spawn_blocking`, credentials in keychain
- WhatsApp (`wcore-channel-whatsapp`) — Cloud API REST outbound to `graph.facebook.com` + webhook inbound with `X-Hub-Signature-256` verification
- Signal (`wcore-channel-signal`) — `signal-cli` subprocess adapter, JSON-RPC over stdio
- iMessage (`wcore-channel-imessage`) — macOS-only, AppleScript/`osascript`
- Matrix (`wcore-channel-matrix`) — send-only MVP REST (poll/sync deferred)
- MS Teams (`wcore-channel-msteams`) — Bot Framework OAuth2 + Connector REST send (webhook inbound deferred)

## Browser & Computer-Use (CUA) Backends

**Browser** (`crates/wcore-browser/src/backends/`):
- Camoufox (`camoufox.rs`, 34.2K) — primary backend
- Chromium via `chromiumoxide` (`chromium.rs`) — fallback
- Browserbase (`browserbase.rs`, 19.7K) — cloud backend
- ARIA-tree-first surface (`aria.rs`, `readability.rs`); network egress bounded by `BrowserPolicy` (`policy.rs`, 28.5K); lifecycle managed by `BrowserSupervisor` (`supervisor.rs`, 35.7K)
- Plugin mirror: `wayland-browser` exposes `BrowserToolSpec` through `wcore-plugin-api` without a direct `wcore-browser` dependency

**Computer Use (CUA)** (`crates/wcore-cua/src/backends/`):
- Platform-specific: `macos.rs` (29.3K), `linux_x11.rs` (22.4K), `linux_wayland.rs` (15.6K), `windows.rs` (23.4K), `unsupported.rs` (fallback stub)
- Gated by `CuaPolicy`; background-mode invariant enforced
- Plugin mirror: `wayland-cua` exposes `CuaToolSpec` through `wcore-plugin-api` without a direct `wcore-cua` dependency

## Sandbox Backends

`crates/wcore-sandbox/src/backends/`:
- Linux: `bwrap` (`bwrap.rs`, 38.1K) with landlock (`bwrap_landlock.rs`) + seccomp (`bwrap_seccomp.rs`) hardening
- macOS: `sandbox-exec` (`sandbox_exec.rs`, 29.1K)
- Windows: AppContainer + Job Object (`appcontainer/`, `appcontainer.rs`, 21.6K)
- Docker (`docker.rs`, 33.8K) — optional, behind `live-docker` Cargo feature
- `no_sandbox.rs` — explicit bypass fallback
- `process_tree.rs` (36.0K) — process-tree ownership/reaping across all backends
- All backends probe real spawn behavior rather than trusting shallow API/capability checks (per crate design note)

## User-Model / Memory Persistence

- `crates/wcore-honcho-adapter` — bridges `wcore-user-model::UserModelBackend` trait to `wayland-honcho::HonchoClient`, enabling persistent-across-devices user preferences/expertise/style/brief storage
- `wayland-honcho` plugin — mock-by-default, real HTTP calls gated behind the `live-honcho` Cargo feature (keeps plugin isolation intact per audit F2)

## Webhooks & Callbacks

**Incoming:**
- Slack Events API, Discord Gateway WebSocket, WhatsApp Cloud API webhook (`X-Hub-Signature-256`-verified), Twilio SMS webhook (HMAC-SHA1-verified), Telegram long-poll (not push), OAuth localhost redirect callback (Google Meet etc.)

**Outgoing:**
- Slack Web API, Discord REST, Twilio REST, WhatsApp Graph API, MS Teams Connector REST, SMTP (email), `signal-cli` JSON-RPC, AppleScript (iMessage)

---

*Integration audit: 2026-07-23*
