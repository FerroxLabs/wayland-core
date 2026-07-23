# Technology Stack

**Analysis Date:** 2026-07-23

## Languages

**Primary:**
- Rust (edition 2024, `rust-version = "1.95"`) — entire workspace (`Cargo.toml`), ~70 crates under `crates/`

**Secondary:**
- RON (Rusty Object Notation) — declarative workflow DSL front-end, lowered to `GraphConfig` IR (`crates/wcore-agent/src/orchestration/workflow/`)
- TOML — config files, skill front matter, agent manifests
- Shell (sh/pwsh) — `justfile` recipes, centralized via `wcore_config::shell` helpers

## Runtime

**Environment:**
- Rust stable toolchain, pinned to `1.95.0` via `rust-toolchain.toml` (`channel = "1.95.0"`, components `clippy` + `rustfmt`) and mirrored in `vx.toml` (`[tools] rust = "1.95.0"`)
- Async runtime: `tokio` 1.x, `features = ["full"]` (workspace dep, `Cargo.toml:212`)

**Package Manager:**
- Cargo workspace, `resolver = "2"`, ~70 members under `crates/*` plus `workspace-hack` (managed by `cargo-hakari` for build-speed dependency unification)
- Lockfile: `Cargo.lock` present and committed
- Tool version pinning: `vx` (loonghao/vx) pins `just = "1.48.1"` and `rust = "1.95.0"` in `vx.toml`; all `justfile` recipes route through `vx cargo ...` / `vx just ...` so local dev and CI use identical toolchain versions

## Frameworks

**Core:**
- Tokio async runtime — full workspace async foundation
- `reqwest` 0.12 (`json`, `stream`, `multipart`, `rustls-tls`, `default-features = false`) — HTTP client, rustls instead of native OpenSSL for clean cross-compilation; ALL outbound HTTP must route through the single chokepoint in `crates/wcore-egress` (a clippy `disallowed-methods` lint bans raw `reqwest::Client::new`/`builder` elsewhere)
- `clap` 4 (`derive`, `env`) — CLI argument parsing (`wcore-cli`)
- `ratatui` 0.30 (`unstable-rendered-line-info`) + `crossterm` 0.29 + `tui-input`, `tui-tree-widget`, `nucleo`, `pulldown-cmark`, `textwrap`, `syntect` — the TUI (terminal UI), `wcore-cli`

**Testing:**
- `cargo nextest` — primary test runner (`just test`, `just test-ci`, `just test-e2e`), multiple profiles: `default`, `ci` (`--no-fail-fast`), `e2e` (sequential, long timeout, no retry)
- `wiremock` 0.6, `tokio-test`, `tempfile`, `mockall` 0.14, `rstest` 0.26, `serial_test` 3, `criterion` 0.5 (benchmarks with HTML reports)
- `wcore-fixture-harness` crate — customer-fixture catalog + replay harness for E2E testing without live API calls

**Build/Dev:**
- `just` (via `vx`) — task runner (`justfile`); `just push` = lint-fix → fmt → auto-commit-fixes → test → `git push`
- `cargo-hakari` — workspace-hack crate generation/verification (`just hakari-generate`, `just hakari-verify`)
- `cargo clippy` (must pass with zero warnings) + `cargo fmt` (CI-enforced, no diffs)
- `cargo-audit` / `osv-scan.yml` GitHub Action — dependency vulnerability scanning
- `wasmtime` 30 + WASI — WASM Component Model plugin host (`crates/wcore-plugin-wasm`)

## Key Dependencies

**Critical:**
- `serde` 1 (`derive`) / `serde_json` 1 / `serde_yaml` 0.9 / `toml` 1.0 — serialization backbone across config, protocol, skills
- `thiserror` 2 (public API error types) / `anyhow` 1 (internal/application error propagation) — per AGENTS.md error-handling convention
- `tracing` 0.1 + `tracing-subscriber` 0.3 (`env-filter`) + `tracing-appender` 0.2 — structured logging, non-blocking file writer for TUI mode
- `jsonwebtoken` (exact-pinned `=10.3.0`) — Vertex AI GCP service-account JWT auth (security-critical, per CONTRIBUTING.md pinning policy)
- `aws-sigv4` (exact-pinned `=1.4.3`), `aws-credential-types`, `aws-config`, `aws-sdk-sts` — AWS SigV4 signing for Bedrock provider
- `keyring` (exact-pinned `=3.6.3`, features `apple-native`/`windows-native`/`sync-secret-service`) — OS keychain credential backend
- `argon2` (`=0.5.3`) + `chacha20poly1305` (`=0.10.1`) + `zeroize` (`=1.8.2`) — encrypted-file credentials backend (Argon2id KDF + XChaCha20-Poly1305 AEAD), Rust reimplementation of Forge's vault.ts pattern
- `ed25519-dalek` (`=2.2.0`) — opt-in plugin signature verification
- Note: all crypto/auth-boundary deps above use **exact version pins** (`=x.y.z`) per repo policy (CONTRIBUTING.md §Security-Critical Pinning) — never relax to a range without deliberate review

**Infrastructure:**
- `rusqlite` 0.32 (`bundled`, `blob`, `chrono`) + `sqlite-vec` 0.1 — SQLite for memory subsystem v2 (bundled build avoids system-lib drift on Windows)
- `tokio-postgres` 0.7 + `tokio-postgres-rustls` 0.13 + `rustls` 0.23 (`ring`, `std`, `tls12`) + `webpki-roots` 0.26 — Postgres schema tool backend, TLS-only, SSRF mitigation via private-range host rejection at connection-parse time
- `candle-core`/`candle-nn`/`candle-transformers` 0.9, `tokenizers` 0.21, `hf-hub` 0.4, `safetensors` 0.5 — local embedding inference (bge-small-en-v1.5), pure-Rust, models fetched from HuggingFace on first use (cached `~/.cache/huggingface`)
- `hyper` 1.5 + `hyper-util` + `open` + `urlencoding` + `subtle` — OAuth subsystem (Google Meet etc.), localhost redirect listener with constant-time CSRF token comparison
- `uuid` 1 (`v4`/`v5`/`v7`/`serde`), `parking_lot` 0.12, `lru` 0.18, `tokio-util` 0.7 (`rt`, cancellation tokens)
- `cpal` 0.15 + `hound` 3 — cross-platform audio capture (CoreAudio/WASAPI/ALSA) for voice-mode tool backend
- `pdf-extract` 0.12, `calamine` 0.26, `quick-xml` 0.39, `csv` 1.3, `image` 0.25, `kamadak-exif` 0.6 — document/media extraction tools, all pure-Rust (no native lib deps)
- `zip` 2, `tar` 0.4, `flate2` 1 — archive tool, pure-Rust
- `utoipa` 4.2 (`axum_extras`) — OpenAPI 3.0.3 doc generation for the REST transport (`wcore-acp`), pinned to 4.x for axum 0.7 compat

## Configuration

**Environment:**
- Credentials sourced via OS keychain (`keyring`) or encrypted-file vault (Argon2id + XChaCha20-Poly1305), with interactive `rpassword` (`=7.5.2`) TTY passphrase prompt when `WAYLAND_VAULT_PASSPHRASE` unset
- `~/.wayland/channels/*.toml` — chat-platform channel adapter configs, auto-scanned/registered at engine boot by `wcore-channels-registry`
- Config cascades — see `docs/getting-started.md` for full precedence rules

**Build:**
- `Cargo.toml` (workspace root) — dependency versions, workspace member list, `[profile.release]` (thin LTO, `codegen-units = 1`, debuginfo stripped, `overflow-checks` on in release for defense-in-depth)
- `vx.toml` — pins `rust` and `just` tool versions for `vx`-mediated invocation
- `rust-toolchain.toml` — Rust channel + components (`clippy`, `rustfmt`)
- `justfile` — all task recipes (build/test/lint/fmt/audit/hakari), cross-platform shell (`sh -cu` on Unix, `pwsh` on Windows)

## Platform Requirements

**Development:**
- macOS, Linux, and Windows all supported and CI-tested (`.github/workflows/ci.yml`, `nightly-windows-soak.yml`)
- One-time setup: install `vx` (pinned tool version manager) + `cargo install cargo-nextest --locked`
- Platform-specific code must route through centralized helpers: `wcore_config::shell` for process spawning, no raw `Command::new("sh"/"bash"/"cmd")`

**Production:**
- Distributed as a compiled CLI binary (`wcore-cli`)
- Sandbox backends are platform-specific: `bwrap` (Linux, with landlock + seccomp), `sandbox-exec` (macOS), AppContainer + Job Object (Windows), Docker (optional, behind `live-docker` feature)
- CUA (computer-use) backends are platform-specific: macOS, Linux X11, Linux Wayland, Windows (`crates/wcore-cua/src/backends/`)
- Release pipeline: `.github/workflows/release.yml`, `release-please.yml` (automated changelog/versioning)

---

*Stack analysis: 2026-07-23*
