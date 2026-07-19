# External Integrations

**Analysis Date:** 2026-07-18

## APIs & External Services

**LLM Providers:**
- Anthropic Messages API — native streaming provider in `crates/wcore-providers/src/anthropic.rs`; authenticates with `ANTHROPIC_API_KEY` (or generic configured `API_KEY`) through `crates/wcore-config/src/config.rs`.
- OpenAI Responses/Chat-compatible APIs — native provider and compatibility layer in `crates/wcore-providers/src/openai.rs`, `crates/wcore-config/src/compat.rs`, and `crates/wcore-config/src/data/providers.toml`; `OPENAI_API_KEY` is the standard credential, while compatible services use their provider-specific keys.
- AWS Bedrock — AWS SDK credential chain, STS, and SigV4 in `crates/wcore-providers/src/bedrock.rs`; uses standard AWS environment/profile/container/web-identity credentials and `AWS_REGION`/`AWS_DEFAULT_REGION` from `crates/wcore-config/src/config.rs`.
- Google Vertex AI and Gemini — implementations in `crates/wcore-providers/src/vertex.rs` and `crates/wcore-providers/src/gemini.rs`; Vertex uses Google Application Default Credentials/project/region, while Gemini accepts `GEMINI_API_KEY` or `GOOGLE_API_KEY` as resolved in `crates/wcore-config/src/config.rs`.
- Azure OpenAI — OpenAI-wire provider in `crates/wcore-providers/src/azure_openai.rs`; authenticates with `AZURE_OPENAI_API_KEY` and configured Azure resource/deployment/base URL through `crates/wcore-config/src/config.rs`.
- Hosted OpenAI-compatible providers — Together, Fireworks, NVIDIA, Perplexity, Cerebras, OpenRouter, Flux Router, DeepSeek, xAI, Groq, Moonshot, Alibaba/Qwen, Mistral, Cohere, MiniMax, and Sakana are exposed by `crates/wcore-config/src/config.rs` and modules under `crates/wcore-providers/src/`; conventional keys are `TOGETHER_API_KEY`, `FIREWORKS_API_KEY`, `NVIDIA_API_KEY`, `PERPLEXITY_API_KEY`, `CEREBRAS_API_KEY`, `OPENROUTER_API_KEY`, `FLUX_API_KEY`, `DEEPSEEK_API_KEY`, `XAI_API_KEY`, `GROQ_API_KEY`, `MOONSHOT_API_KEY`, `DASHSCOPE_API_KEY`/`ALIBABA_API_KEY`, `MISTRAL_API_KEY`, `COHERE_API_KEY`, `MINIMAX_API_KEY`, and `SAKANA_API_KEY`.
- Additional OpenAI-wire services are data-driven: 104 endpoints are catalogued in `crates/wcore-config/src/data/providers.toml` and loaded through `crates/wcore-config/src/catalog.rs`; API-key environment names and base URLs come from each catalog entry.
- ChatGPT Codex OAuth and xAI OAuth — local OAuth handlers and token refresh live in `crates/wcore-agent/src/oauth/`; profile-scoped token paths are resolved by `crates/wcore-config/src/config.rs`.
- Ollama local inference — reference plugin at `crates/wayland-ollama/`; defaults to `http://localhost:11434/api/chat` with model `llama3`, overridden by `OLLAMA_BASE_URL`/`OLLAMA_MODEL` or plugin config in `crates/wayland-ollama/src/plugin.rs`.

**Search, Fetch, and Media:**
- Web search backends are selected by `WAYLAND_WEB_BACKEND` or credential discovery in `crates/wcore-agent/src/tool_backends/mod.rs`: Firecrawl (`FIRECRAWL_API_KEY`, optional `FIRECRAWL_API_URL`), Parallel (`PARALLEL_API_KEY`), Tavily (`TAVILY_API_KEY`), Exa (`EXA_API_KEY`), SearXNG (`SEARXNG_URL`), Brave (`BRAVE_SEARCH_API_KEY`), and unauthenticated DuckDuckGo HTML.
- Generic WebFetch and the API tools use SSRF-resistant redirects and the central egress client in `crates/wcore-agent/src/tool_backends/http_fetch.rs` and `crates/wcore-agent/src/tool_backends/mod.rs`.
- Vision uses the active Anthropic/OpenAI/Gemini credentials in `crates/wcore-agent/src/tool_backends/anthropic_vision.rs`, `openai_vision.rs`, and `gemini_vision.rs`.
- Audio transcription selects Groq Whisper (`GROQ_API_KEY`) then OpenAI Whisper (`OPENAI_API_KEY`) in `crates/wcore-agent/src/tool_backends/mod.rs` and `openai_compat_whisper.rs`.
- Image generation resolves the active provider, then FAL (`FAL_API_KEY`), Gemini (`GEMINI_API_KEY`), Hugging Face (`HF_API_KEY`), or Pollinations fallback in `crates/wcore-agent/src/tool_backends/image_gen.rs`.
- Text-to-speech resolves the active OpenAI-wire provider, OpenAI (`OPENAI_API_KEY`), ElevenLabs (`ELEVENLABS_API_KEY`), or local Piper in `crates/wcore-agent/src/tool_backends/tts.rs` and `piper.rs`.

**Developer/Product APIs:**
- GitHub REST API — typed operations in `crates/wcore-tools/src/github_tool.rs`, real transport in `crates/wcore-agent/src/tool_backends/http_github.rs`; accepts tool argument auth or `GITHUB_TOKEN` and sends Bearer auth to `https://api.github.com`.
- GitLab REST API v4 — typed operations in `crates/wcore-tools/src/gitlab_tool.rs`, transport in `crates/wcore-agent/src/tool_backends/http_gitlab.rs`; the tool type supports a host-supplied token and self-hosted base URL via `PRIVATE-TOKEN`, but production bootstrap currently passes `None` for both in `crates/wcore-agent/src/bootstrap.rs`. `GITLAB_TOKEN` appears in TUI diagnostics at `crates/wcore-cli/src/tui/surfaces/diagnostics.rs` but is not consumed by the live tool registration path.
- Linear GraphQL API — `https://api.linear.app/graphql`, typed in `crates/wcore-tools/src/linear_tool.rs` and transported by `crates/wcore-agent/src/tool_backends/http_linear.rs`; accepts `LINEAR_API_KEY` or a tool argument as the Authorization header value.
- Notion REST API — `https://api.notion.com/v1`, typed in `crates/wcore-tools/src/notion_tool.rs` and transported by `crates/wcore-agent/src/tool_backends/http_notion.rs`; accepts `NOTION_TOKEN` or a tool argument as Bearer auth and pins Notion API version `2022-06-28`.
- PostgreSQL schema inspection — connects with `DATABASE_URL`, `POSTGRES_URL`, or `PG_CONN_STRING` via tokio-postgres/rustls in `crates/wcore-agent/src/tool_backends/postgres_schema.rs`; this is an agent tool, not the product's persistence backend.
- Google Meet API — Google OAuth client credentials (`GOOGLE_CLIENT_ID`, optional `GOOGLE_CLIENT_SECRET`) and Meet v2 REST calls in `crates/wcore-agent/src/tool_backends/google_meet.rs`.
- Home Assistant REST API — requires `HASS_URL` and `HASS_TOKEN`; implementation and SSRF boundary are in `crates/wcore-agent/src/tool_backends/homeassistant.rs`.
- Discord bot tool — requires `DISCORD_BOT_TOKEN` and uses Discord REST in `crates/wcore-agent/src/tool_backends/discord.rs`; this is separate from the persistent Discord channel adapter.
- Honcho user-model service — trait adapter in `crates/wcore-honcho-adapter/` and plugin in `crates/wayland-honcho/`; live HTTP is feature-gated by `live-honcho` in `crates/wayland-honcho/Cargo.toml`.

**Browser and Agent Protocols:**
- Camoufox — default local browser sidecar selected in `crates/wcore-browser/src/selection.rs`; client/backend code is in `crates/wcore-browser/src/backends/camoufox.rs`, with `CAMOFOX_ACCESS_KEY` and sidecar executable/endpoint configuration consumed under `crates/wcore-browser/`.
- Chromium — optional local CDP backend using Chromiumoxide 0.7 behind the `chromium` feature in `crates/wcore-browser/Cargo.toml`.
- Browserbase — optional cloud browser backend behind the `browserbase` feature; requires `BROWSERBASE_API_KEY` and `BROWSERBASE_PROJECT_ID` in `crates/wcore-browser/src/backends/browserbase.rs`.
- Model Context Protocol — client supports stdio, SSE, and Streamable HTTP transports under `crates/wcore-mcp/src/transport/`; server definitions, commands, environment, and headers are configured by `[mcp.servers.*]` in `crates/wcore-config/src/config.rs` and documented in `docs/mcp.md`.
- Agent Client Protocol — stdio, HTTP/SSE, WebSocket, and REST/OpenAPI surfaces live in `crates/wcore-acp/src/transport/`, `crates/wcore-acp/src/server.rs`, and `crates/wcore-acp/src/transport/rest.rs`; HTTP uses Axum and WebSockets use Tokio Tungstenite per `crates/wcore-acp/Cargo.toml`.
- Plugins — native inventory loading, signed WASM components, subprocess JSON-RPC, and marketplace source lowering are implemented in `crates/wcore-agent/src/plugins/`, `crates/wcore-plugin-wasm/`, `crates/wcore-plugin-subprocess/`, and `crates/wcore-pluginsrc/`.

**Messaging Channels:**
- Slack — Web API outbound and Events API webhook inbound with signing-secret verification in `crates/wcore-channel-slack/`.
- Telegram — Bot API outbound plus `getUpdates` long polling in `crates/wcore-channel-telegram/`.
- Discord — REST outbound plus Gateway v10 WebSocket inbound in `crates/wcore-channel-discord/`.
- Email — SMTP outbound via Lettre and IMAP inbound using native TLS in `crates/wcore-channel-email/`.
- Twilio SMS — REST outbound plus HMAC-SHA1 signed webhooks in `crates/wcore-channel-sms/`.
- WhatsApp Cloud — Meta Graph REST outbound plus verification/signature-checked webhooks in `crates/wcore-channel-whatsapp/`.
- Signal — `signal-cli` JSON-RPC subprocess transport in `crates/wcore-channel-signal/`; Signal credentials remain in signal-cli's own store.
- Matrix — configurable homeserver REST sending in `crates/wcore-channel-matrix/`.
- Microsoft Teams — Bot Framework OAuth2, Connector REST, and inbound token validation code in `crates/wcore-channel-msteams/`.
- iMessage — macOS-only AppleScript/Messages bridge in `crates/wcore-channel-imessage/`.
- Channel configuration auto-registration scans `$WAYLAND_HOME/channels/*.toml` and resolves credential handles through the credential-store abstraction in `crates/wcore-channels-registry/src/lib.rs`.

## Data Storage

**Databases:**
- SQLite is the local application database for long-term memory; bundled Rusqlite and sqlite-vec schemas/migrations are implemented in `crates/wcore-memory/src/db.rs`.
- Memory database locations are resolved in `crates/wcore-memory/src/paths.rs`: global memory under the configured memory base, per-session databases below `memory/sessions/`, project memory at `<project>/.wayland-core/memory/memory.db`, and audit data in `memory/audit.db`.
- `WCORE_MEMORY_DIR` overrides the memory base; legacy `AIONRS_MEMORY_DIR` is the fallback override before the platform-aware Wayland config directory (`crates/wcore-memory/src/paths.rs`).
- PostgreSQL is accessed only when the schema-inspection tool is configured; no PostgreSQL server is required for normal engine state (`crates/wcore-agent/src/tool_backends/postgres_schema.rs`).

**File Storage:**
- Configuration, profiles, OAuth state, channel TOML, plugins, logs, cron data, evolved artifacts, and trust anchors are local files rooted through `WAYLAND_HOME`; canonical resolution is in `crates/wcore-config/src/config.rs` and `crates/wcore-config/src/profile.rs`.
- Sessions and protocol artifacts are local JSON/JSONL/Markdown files managed under `crates/wcore-agent/src/session.rs`, `crates/wcore-replay/`, and `crates/wcore-eval-scenarios/`.
- Cron schedules and history use `$WAYLAND_HOME/cron/jobs.json` and `history.jsonl` in `crates/wcore-cron/src/store.rs`.
- Local BGE embeddings download model weights through hf-hub and use the Hugging Face cache when the `bge-local` feature is enabled (`crates/wcore-memory/src/embed/bge_local.rs`, `crates/wcore-memory/Cargo.toml`).
- No S3/GCS/Azure Blob product-storage adapter is present; cloud SDK use is for LLM/auth integrations rather than object persistence.

**Caching:**
- In-process LRU caches use the `lru` crate declared in `Cargo.toml`; file-state and provider/model caches are maintained by the owning crates under `crates/wcore-agent/`, `crates/wcore-config/`, and `crates/wcore-browser/`.
- SQLite/vector indexes cache durable memory retrieval state in `crates/wcore-memory/`; local BGE model artifacts rely on the Hugging Face filesystem cache.
- No Redis, Memcached, or external cache service is integrated.

## Authentication & Identity

**Application Credentials:**
- Provider authentication supports explicit config/API key, conventional environment variables, OAuth, and cloud ambient credential chains selected in `crates/wcore-config/src/config.rs` and consumed by `crates/wcore-providers/`.
- Profile isolation is anchored by `WAYLAND_HOME`; profile activation refuses conflicting homes for ACP service processes in `crates/wcore-config/src/profile.rs` and `crates/wcore-cli/src/acp.rs`.
- The credential store supports OS keyring and encrypted local-vault dependencies declared in `Cargo.toml` and `crates/wcore-config/Cargo.toml`. `crates/wcore-config/src/credentials.rs` exists but was intentionally not read under the mapping secret-file exclusion.
- Channel TOML stores credential handles rather than raw tokens; adapters resolve them through `CredentialsStore` in `crates/wcore-channels-registry/src/lib.rs`.
- Plugin trust uses Ed25519 signatures and per-profile trusted-key roots in `crates/wcore-agent/src/plugins/sig_verifier.rs`.

**OAuth and Service Identity:**
- ChatGPT and xAI OAuth authorize model providers; Google OAuth authorizes Meet access. Implementations and refresh flows are under `crates/wcore-agent/src/oauth/` and `crates/wcore-agent/src/tool_backends/google_meet.rs`.
- AWS Bedrock uses the standard AWS SDK credential chain rather than custom token storage (`crates/wcore-providers/src/bedrock.rs`).
- Vertex AI uses Google Application Default Credentials (`crates/wcore-providers/src/vertex.rs`).
- Microsoft Teams obtains Bot Framework OAuth2 access tokens in `crates/wcore-channel-msteams/src/token.rs` and validates inbound identity in `crates/wcore-channel-msteams/src/auth.rs`.
- Team-mode bearer tokens and multi-actor ACL checks live in `crates/wcore-permissions/`; ACP authentication hooks are in `crates/wcore-acp/src/auth.rs`.

## Monitoring & Observability

**Logs:**
- `tracing`, `tracing-subscriber`, and `tracing-appender` provide structured runtime logging; `RUST_LOG` controls filtering in `crates/wcore-cli/src/main.rs`.
- TUI mode writes a non-blocking log below `$WAYLAND_HOME/logs/`; non-TUI/headless execution logs to stderr (`crates/wcore-cli/src/main.rs`, `crates/wcore-tools/src/debug_helpers.rs`).
- Channel/webhook denials and egress errors are content-minimized at their boundaries, including `crates/wcore-agent/src/inbound_webhook.rs` and `crates/wcore-egress/src/`.

**Tracing and Metrics:**
- Structured turn, memory, budget, and tool spans are defined in `crates/wcore-observability/src/trace.rs`; sinks include in-memory buffering and JSON stdout in `crates/wcore-observability/src/sink.rs`.
- `PiiScrubbingSink` defensively scrubs trace values before forwarding in `crates/wcore-observability/src/sink.rs`.
- OTLP export is available only when the `otlp` Cargo feature is enabled; OpenTelemetry 0.27 dependencies and the HTTP exporter are in `crates/wcore-observability/Cargo.toml` and `crates/wcore-observability/src/sink.rs`.
- JSON-stream hosts receive opaque trace payloads through `crates/wcore-protocol/`; `[observability]` controls structured traces in `crates/wcore-config/src/config.rs`.
- No hosted error-tracking or APM vendor SDK is present in the workspace manifests.

## CI/CD & Deployment

**CI Pipeline:**
- GitHub Actions is the only CI/CD platform. Pull requests and main pushes run formatting, Clippy, nextest, release-binary smoke, protocol-contract drift, packaged eval, acceptance eval, and cargo-audit gates in `.github/workflows/ci.yml`.
- CI covers macOS, a self-hosted Windows x64 runner, and containerized Linux jobs; Rust/Just setup is pinned via `loonghao/vx` and `vx.toml` (`.github/workflows/ci.yml`).
- End-to-end, OSV, benchmark-regression, marketplace-drift, mutation, and Windows-soak workflows live in `.github/workflows/e2e.yml`, `osv-scan.yml`, `bench-regression.yml`, `marketplace-drift.yml`, `mutants-nightly.yml`, and `nightly-windows-soak.yml`.
- Release Please configuration exists in `.github/workflows/release-please.yml`; its workflow trigger/state is defined there rather than inferred from release tags.

**Release/Deployment:**
- Tags matching `v*-wayland-*` trigger `.github/workflows/release.yml`, which builds six platform archives, uploads artifacts, creates/verifies GitHub Release assets, and runs downloaded-binary smoke checks.
- GitHub build provenance uses keyless Sigstore-backed `actions/attest-build-provenance`, and SHA-256 checksums are published alongside archives (`.github/workflows/release.yml`).
- Smoke-verified archives are repackaged into npm platform packages plus the `@ferroxlabs/wayland-core` wrapper under `npm/`; publish uses public access and npm provenance when `NPM_TOKEN` is available (`.github/workflows/release.yml`, `npm/generate.mjs`).
- This is a distributed native CLI, not a continuously deployed server. ACP/webhook listeners and optional services are started by operators from the shipped binary.

## Environment Configuration

**Required Variables:**
- No single environment variable is universally required: a local Ollama configuration can run without a cloud API key, while each cloud provider requires its own authentication (`crates/wcore-config/src/config.rs`, `crates/wayland-ollama/src/plugin.rs`).
- Core path/log controls: `WAYLAND_HOME`, `WAYLAND_CONFIG_PATH`, `WAYLAND_PROFILES_ROOT`, `RUST_LOG`, and memory overrides `WCORE_MEMORY_DIR`/`AIONRS_MEMORY_DIR` (`crates/wcore-config/src/config.rs`, `crates/wcore-config/src/profile.rs`, `crates/wcore-memory/src/paths.rs`).
- Provider credentials: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`/`GOOGLE_API_KEY`, `AZURE_OPENAI_API_KEY`, hosted-provider keys listed under LLM Providers, and standard AWS/GCP credential variables (`crates/wcore-config/src/config.rs`).
- Tool credentials/config: `WAYLAND_WEB_BACKEND`, `FIRECRAWL_API_KEY`, `FIRECRAWL_API_URL`, `PARALLEL_API_KEY`, `TAVILY_API_KEY`, `EXA_API_KEY`, `SEARXNG_URL`, `BRAVE_SEARCH_API_KEY`, `GITHUB_TOKEN`, `LINEAR_API_KEY`, `NOTION_TOKEN`, `DATABASE_URL`/`POSTGRES_URL`/`PG_CONN_STRING`, `GOOGLE_CLIENT_ID`/`GOOGLE_CLIENT_SECRET`, `HASS_URL`/`HASS_TOKEN`, and `DISCORD_BOT_TOKEN` (`crates/wcore-agent/src/tool_backends/`, `crates/wcore-tools/src/`). `GITLAB_TOKEN` is diagnostic-only until bootstrap passes it to `register_gitlab_tool` (`crates/wcore-agent/src/bootstrap.rs`).
- Browser credentials/config: `OLLAMA_BASE_URL`, `OLLAMA_MODEL`, `WAYLAND_CAMOUFOX_BIN`, `CAMOFOX_ACCESS_KEY`, `BROWSERBASE_API_KEY`, and `BROWSERBASE_PROJECT_ID` (`crates/wayland-ollama/src/plugin.rs`, `crates/wcore-browser/src/backends/`, `crates/wcore-browser/src/supervisor.rs`).
- Memory cloud embeddings default to `OPENAI_API_KEY` or `VOYAGE_API_KEY`; `[memory.embedder].api_key_env` can name a different environment variable (`crates/wcore-config/src/config.rs`, `crates/wcore-memory/src/memory.rs`).

**Secrets Location:**
- Do not commit secrets. `$WAYLAND_HOME/.env` is an optional local import surface handled by `crates/wcore-config/src/env_file.rs`; no runtime `.env` is checked in.
- Prefer credential handles backed by the OS keyring/encrypted local store for persistent channel secrets; channel loaders consume handles in `crates/wcore-channels-registry/src/lib.rs`.
- OAuth tokens and plugin trust roots are profile-scoped through paths resolved in `crates/wcore-config/src/config.rs` and `crates/wcore-agent/src/plugins/sig_verifier.rs`.
- GitHub Actions publishing uses repository secrets such as `NPM_TOKEN`, referenced only by workflow name in `.github/workflows/release.yml`.
- Secret/credential-focused source and test files detected during inventory were not opened: `crates/wcore-plugin-wasm/src/host_adapters/secrets.rs`, `crates/wcore-config/src/credentials.rs`, `crates/wcore-config/tests/credential_storage_test.rs`, `crates/wcore-config/tests/credential_isolation_test.rs`, `crates/wcore-tools/tests/bash_credential_exfil_test.rs`, and `crates/wcore-sandbox/tests/secret_read_deny.rs`.

## Webhooks & Callbacks

**Inbound:**
- The opt-in Axum host routes `GET|POST /webhooks/:channel` and `GET /healthz`; it binds `127.0.0.1:8787` by default in `crates/wcore-agent/src/inbound_webhook.rs` and `crates/wcore-config/src/config.rs`.
- Slack Events, Twilio SMS, and WhatsApp Cloud deliveries are forwarded to adapter-owned signature verification through `ChannelManager::ingest_webhook` in `crates/wcore-agent/src/inbound_webhook.rs`.
- `public_base_url` must match the external HTTPS origin for Twilio URL-signature verification; public exposure requires an operator-managed TLS reverse proxy (`crates/wcore-agent/src/inbound_webhook.rs`).
- Microsoft Teams contains inbound JWT validation code under `crates/wcore-channel-msteams/src/auth.rs`; adapter registration and delivery behavior are owned by `crates/wcore-channel-msteams/` and `crates/wcore-channels-registry/`.

**Outbound/Callbacks:**
- ChatGPT, xAI, and Google Meet OAuth flows use local callback/refresh logic under `crates/wcore-agent/src/oauth/` and `crates/wcore-agent/src/tool_backends/google_meet.rs`.
- ACP HTTP/SSE and WebSocket clients receive streaming protocol events from routes implemented under `crates/wcore-acp/src/transport/`.
- MCP SSE and Streamable HTTP servers are configured external callback/stream endpoints consumed by transports under `crates/wcore-mcp/src/transport/`.
- Slack, Discord, Telegram, email, Twilio, WhatsApp, Matrix, Teams, and Signal outbound delivery is initiated by their respective crates under `crates/wcore-channel-*`.

---

*Integration audit: 2026-07-18*
