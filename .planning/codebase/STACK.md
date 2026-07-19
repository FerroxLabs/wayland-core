# Technology Stack

**Analysis Date:** 2026-07-18

## Languages

**Primary:**
- Rust 2024 edition, minimum Rust 1.95 — all production engine, protocol, provider, tool, plugin, channel, browser, CUA, sandbox, eval, and CLI code lives under `crates/*/src/**/*.rs`; workspace policy is defined in `Cargo.toml` and the compiler pin in `rust-toolchain.toml`.

**Secondary:**
- Shell/Bash — developer automation, release/evaluation harnesses, and CI helpers in `justfile` and `scripts/*.sh`.
- PowerShell — native Windows soak/evaluation automation in `scripts/wayland-e2e-windows-soak.ps1`.
- Python 3 — proof and evaluation utilities such as `scripts/f11-proof.py`.
- JavaScript/Node.js — npm release-tree generation in `npm/generate.mjs`.
- TypeScript — repository maintenance scripts such as `scripts/engine-rebrand.ts`.
- RON, TOML, YAML, JSON, WIT, and Liquid are configuration/schema/template formats, not application runtimes; representative paths are `crates/wcore-agent/src/orchestration/workflow/`, `crates/wcore-config/src/data/providers.toml`, `.github/workflows/ci.yml`, `crates/wcore-plugin-wasm/wit/`, and `templates/`.

## Runtime

**Environment:**
- Native command-line process built as `wayland-core` from `crates/wcore-cli/src/main.rs`; asynchronous work runs on Tokio 1 with the full feature set declared in `Cargo.toml`.
- The engine is provider-neutral and host-embeddable: `crates/wcore-agent/` owns orchestration, `crates/wcore-protocol/` owns the JSON-lines host protocol, and `crates/wcore-acp/` provides Agent Client Protocol server/client transports.
- Supported native release targets are Linux x86_64/aarch64, macOS x86_64/aarch64, and Windows x86_64/aarch64, as encoded in `.github/workflows/release.yml`.

**Package Manager:**
- Cargo workspace resolver 2 with 55 members in `Cargo.toml`; resolved dependency state is committed in `Cargo.lock`.
- Rust and Just are installed/pinned through vx (`rust = 1.95.0`, `just = 1.48.1`) in `vx.toml`; the compiler component pin is duplicated in `rust-toolchain.toml`.
- npm is used only for binary-wrapper distribution; package-tree metadata and generation live under `npm/` and the publish workflow in `.github/workflows/release.yml`.

## Frameworks

**Core:**
- Tokio 1 — async runtime, task orchestration, process I/O, networking, and synchronization across the workspace (`Cargo.toml`).
- Clap 4 — CLI argument parsing for the `wayland-core` executable (`Cargo.toml`, `crates/wcore-cli/src/main.rs`).
- Ratatui 0.30 + Crossterm 0.29 — terminal UI and input/output surface (`Cargo.toml`, `crates/wcore-cli/src/tui/`).
- Reqwest 0.12 with rustls — HTTP transport. Production outbound HTTP is centralized through `wcore_egress::EgressClient` in `crates/wcore-egress/src/`; `clippy.toml` prohibits raw client construction elsewhere.
- Axum 0.7 — ACP HTTP/SSE serving and the inbound channel webhook host (`crates/wcore-acp/Cargo.toml`, `crates/wcore-agent/src/inbound_webhook.rs`).
- Tokio Tungstenite 0.24 — ACP WebSocket transport (`crates/wcore-acp/Cargo.toml`, `crates/wcore-acp/src/transport/`).
- Serde/Serde JSON/TOML/RON — protocol, configuration, plugin manifests, and workflow DSL serialization (`Cargo.toml`).
- Wasmtime 36 + WASI Component Model — sandboxed WASM plugin host (`crates/wcore-plugin-wasm/Cargo.toml`, `crates/wcore-plugin-wasm/src/`).
- Inventory 0.3 — compile-time native plugin discovery (`Cargo.toml`, `crates/wcore-agent/src/plugins/`).
- Rusqlite 0.32 bundled + sqlite-vec — local long-term memory persistence and vector search (`Cargo.toml`, `crates/wcore-memory/src/db.rs`).

**Testing:**
- cargo-nextest is the preferred workspace test runner; `just test` and `just test-ci` route to it in `justfile`, and `.github/workflows/ci.yml` installs it explicitly.
- Built-in Rust test harness plus Tokio Test 0.4 for unit/integration async tests (`Cargo.toml`, `crates/*/tests/`).
- Wiremock 0.6 for isolated HTTP contracts, Mockall 0.14 for trait mocks, Rstest 0.26 for parameterized fixtures, Serial Test 3 for process-global state, and Tempfile 3 for filesystem isolation (`Cargo.toml`).
- Criterion 0.5 provides benchmarks under crate `benches/` directories; regression automation is in `.github/workflows/bench-regression.yml`.
- `wcore-eval`, `wcore-eval-scenarios`, `wcore-fixture-harness`, and `wcore-replay` provide acceptance gates, packaged-binary scenarios, fixture replay, and trace replay (`crates/wcore-eval/`, `crates/wcore-eval-scenarios/`, `crates/wcore-fixture-harness/`, `crates/wcore-replay/`).

**Build/Development:**
- Just is the canonical command layer (`justfile`); vx supplies pinned Rust/Just versions (`vx.toml`).
- rustfmt and Clippy are required components (`rust-toolchain.toml`); workspace lint policy and disallowed methods are in `Cargo.toml` and `clippy.toml`.
- Cargo Hakari consolidates feature unification in `workspace-hack/`; recipes `hakari-generate` and `hakari-verify` live in `justfile`.
- Security gates use cargo-audit, cargo-deny, and OSV Scanner through `justfile`, `deny.toml`, `.cargo/audit.toml`, and `.github/workflows/osv-scan.yml`.
- Coverage is collected with cargo-llvm-cov through `justfile`; mutation tests are scheduled by `.github/workflows/mutants-nightly.yml`.
- Cross-compilation uses `cross` and `Cross.toml` for `aarch64-unknown-linux-gnu`; GitHub Actions performs the authoritative multi-OS builds in `.github/workflows/ci.yml` and `.github/workflows/release.yml`.

## Key Dependencies

**Critical:**
- `wcore-types` is the provider-neutral bottom layer; request, event, message, and tool contracts originate in `crates/wcore-types/src/`.
- `wcore-config` owns cascading TOML configuration, profiles, provider compatibility, auth selection, hooks, and centralized shell/platform behavior in `crates/wcore-config/src/`.
- `wcore-providers` implements Anthropic, OpenAI-compatible, Bedrock, Vertex, Gemini, Azure OpenAI, and other model transports behind the common `LlmProvider` trait in `crates/wcore-providers/src/`.
- `wcore-tools` supplies Read/Write/Edit/Bash/Grep/Glob/Spawn and API/media tools; host-side network/process implementations are bound in `crates/wcore-agent/src/tool_backends/`.
- `wcore-agent` is the top-level execution engine, session manager, plugin host, memory coordinator, and multi-agent/workflow orchestrator in `crates/wcore-agent/src/`.
- `wcore-cli` composes the production binary, TUI, JSON-stream mode, ACP commands, diagnostics, and runtime logging in `crates/wcore-cli/src/`.
- `wcore-egress` is the sole raw Reqwest construction boundary and enforces outbound policy in `crates/wcore-egress/src/`.

**Infrastructure:**
- AWS SDK crates (`aws-config`, `aws-sdk-sts`, `aws-sigv4`) implement Bedrock credential discovery and signing in `crates/wcore-providers/src/bedrock.rs`.
- Rustls 0.23, tokio-postgres 0.7, and tokio-postgres-rustls 0.13 provide managed PostgreSQL schema inspection without native OpenSSL in `crates/wcore-agent/src/tool_backends/postgres_schema.rs`.
- Keyring 3.6.3 plus Argon2 0.5.3 and ChaCha20-Poly1305 0.10.1 support OS-backed and encrypted local credential storage; dependencies are declared in `Cargo.toml` and `crates/wcore-config/Cargo.toml`. The implementation file `crates/wcore-config/src/credentials.rs` exists but was not inspected because credential files were excluded from this mapping pass.
- Ed25519/Dalek and SHA-2 support signed plugin verification in `crates/wcore-agent/src/plugins/sig_verifier.rs`.
- Optional Candle 0.9, Tokenizers 0.21, and hf-hub 0.4 provide local BGE embeddings under the `bge-local` feature in `crates/wcore-memory/Cargo.toml`.
- Camoufox sidecar HTTP is the browser default; Chromiumoxide 0.7 and Browserbase are opt-in features in `crates/wcore-browser/Cargo.toml`.
- Bollard 0.17 enables optional Docker isolation (`live-docker`); Landlock and libseccomp provide Linux-native sandbox controls in `crates/wcore-sandbox/Cargo.toml` and `crates/wcore-sandbox/src/backends/`.

## Configuration

**Environment:**
- `WAYLAND_HOME` is the canonical hermetic profile/config/data root and `WAYLAND_CONFIG_PATH` selects an explicit config file; resolution is implemented in `crates/wcore-config/src/config.rs` and profile activation in `crates/wcore-config/src/profile.rs`.
- Primary operator configuration is TOML: global/profile state uses `config.toml`, while project state resolves `.wayland-core.toml` or `.wayland-core/config.toml`; global, project, profile, CLI, and environment layers are merged in `crates/wcore-config/src/config.rs`.
- A local `$WAYLAND_HOME/.env` may be loaded by `crates/wcore-config/src/env_file.rs`; no checked-in runtime `.env` file exists in the repository.
- Provider differences must be represented as `ProviderCompat` data in `crates/wcore-config/src/compat.rs`; the 104-entry compatible-provider catalog is `crates/wcore-config/src/data/providers.toml` and is loaded by `crates/wcore-config/src/catalog.rs`.
- `RUST_LOG` controls tracing filters; TUI logs resolve below `$WAYLAND_HOME/logs/` while headless logs use stderr, configured in `crates/wcore-cli/src/main.rs`.

**Build:**
- Workspace versions, feature declarations, dependency versions, and release profile (`thin` LTO, one codegen unit, stripped debuginfo, overflow checks) are in `Cargo.toml`.
- Rust/Just tool versions are pinned in `rust-toolchain.toml` and `vx.toml`; cross target system dependencies are in `Cross.toml`.
- Default CLI features are `remote-registry`, `workflow`, `monitor`, and `review_artifact` in `crates/wcore-cli/Cargo.toml`; browser cloud/CDP, local embeddings, Docker sandbox, OTLP, and live integration tests remain feature-gated in their crate manifests.

## Platform Requirements

**Development:**
- Rust 1.95.0, Just 1.48.1, and vx are required by `rust-toolchain.toml`, `vx.toml`, and `justfile`; cargo-nextest and cargo-audit are installed explicitly in `.github/workflows/ci.yml`.
- Linux native builds require DBus, libseccomp, ALSA, and pkg-config development packages; the release workflow documents `libdbus-1-dev`, `libseccomp-dev`, `libasound2-dev`, and `pkg-config` in `.github/workflows/release.yml`.
- Linux aarch64 cross-build images additionally install arm64 DBus, SSL, seccomp, and ALSA headers as configured in `Cross.toml`.
- macOS development can format and inspect the repository, but this workspace's `AGENTS.md` requires compilation and Cargo proof on the `hetzner-dsm` Linux harness rather than on the Mac.

**Production:**
- Release artifacts are self-contained archives for six native targets, generated and smoke-tested by `.github/workflows/release.yml`; npm packages wrap those verified binaries under `npm/`.
- OS-specific capabilities require their host facilities: macOS AppleScript/Messages for iMessage (`crates/wcore-channel-imessage/`), X11/Wayland tools for Linux CUA (`crates/wcore-cua/`), and AppContainer/Job Object support on Windows (`crates/wcore-sandbox/src/backends/`).
- Optional external executables are capability-specific: Camoufox for the default browser backend, `signal-cli` for Signal, Piper voice assets for local TTS, and Docker for `live-docker`; their launch/selection code lives in `crates/wcore-browser/`, `crates/wcore-channel-signal/`, `crates/wcore-agent/src/tool_backends/piper.rs`, and `crates/wcore-sandbox/`.
- Public channel webhooks require a TLS-terminating reverse proxy in front of the default loopback Axum listener and an exact `public_base_url` for URL-signed Twilio requests (`crates/wcore-agent/src/inbound_webhook.rs`).

---

*Stack analysis: 2026-07-18*
