// M5.4: lib target for the `wcore-cli` crate. The binary
// (`src/main.rs`) still owns the runtime entry point; this lib exists
// so the plugin marketplace module — and now the ratatui TUI — are
// reachable from integration tests under `tests/`.

pub mod plugin;

// v0.7.0 Task 1.A.10: `acp` subcommand — production caller for the
// `wcore-acp` crate (methodology #27). Lives in the lib so the e2e
// `serve + request` round-trip test runs under `cargo test -p wcore-cli
// --lib`.
pub mod acp;

// ACP/A2A engine bridge: the `TurnEngine` + engine-backed `A2aHandler`
// impls that wire `wcore-acp`'s engine-free seams to the real
// `AgentEngine`. Lives in the lib (alongside `acp`) so the bridge's
// projection + relay logic is unit-testable under `cargo test -p wcore-cli
// --lib`.
pub mod acp_engine;

// v0.7.0 Task 3.B.2: `agent` subcommand — five flag-driven CRUD ops
// (create / list / show / edit / delete) wrapping the
// `wcore_agents_pack::factory` user-agent surface. Lives in the lib so
// the unit tests can inject a tempdir via `run_with_base`.
pub mod agent_cmd;

// T1-E2: dirty-death crash sentinel. Lives here so its unit tests run as
// part of `cargo test -p wcore-cli --lib` instead of being trapped inside
// the binary crate.
pub mod crash_sentinel;

// v0.7.0 Task 1.B.2: `wayland-core init` scaffolds `.wayland/config.toml`
// + a `WAYLAND.md` template in the current project. Non-interactive;
// idempotent unless `--force` is set.
pub mod init;

// T3-6: deterministic prompt-vagueness check (ported from ijfw
// mcp-server/src/prompt-check.js). Pure-regex heuristic; CLI pre-dispatch
// hooks and MCP tool handlers can call `prompt_check::check_prompt`.
pub mod prompt_check;

// v0.6.4 Task 2.4: `mcp-serve` subcommand — exposes the engine's
// `ToolRegistry` as a real MCP server (stdio or SSE). Owns the
// `ToolRegistry → Vec<ServerToolSpec>` adapter (`default_tool_set()` in
// `wcore-mcp` returns empty; this adapter is what actually populates the
// server).
pub mod mcp_serve;

// v0.6.4 Task 2.5: `PolicyGateAdapter` bridges `wcore_mcp::PolicyCheck`
// to the workspace `PolicyGate`. Lives in `wcore-cli` because `wcore-mcp`
// cannot depend on `wcore-agent` without a cycle.
pub mod policy_gate_adapter;

// v0.6.4 Task 2.6: `swarm` subcommand wiring `wcore-swarm` into the
// user-facing CLI. Module lives in the lib so the argv-to-brief mapping
// is unit-testable without spawning a real worker swarm.
pub mod swarm;

// Dynamic Workflows B2: `workflow` subcommand (validate / list / run)
// wrapping the public `wcore_agent::orchestration::workflow` API. Module
// lives in the lib so the file-discovery + validate logic is unit-testable
// against tempdir-backed `.wayland/workflows/` trees without a provider.
pub mod workflow;

// v0.8.1 U7: `cron` subcommand wiring `wcore-cron` into the user-facing
// CLI. Five flag-driven CRUD ops (add / list / remove / enable /
// disable). Module lives in the lib so add-target dispatch is
// unit-testable against a tempdir-backed `FileCronStore` without
// touching the user's home dir.
pub mod cron;

// Crucible (Mixture-of-Providers): `wayland-core crucible "<task>"` runs the
// cross-provider council — N pinned-provider proposers fused by a fenced,
// read-only aggregator. Self-contained one-shot handler.
pub mod crucible;
// v0.8.1 U9: `wayland-core self-update` — pulls the latest signed
// release artifact from GitHub Releases, verifies the .sig against the
// pinned marketplace pubkey, and atomically swaps the running binary.
// Module lives in the lib so the ed25519 verify + mockito-backed
// release-fetch round-trip run under `cargo test -p wcore-cli --lib`.
pub mod self_update;

// The `wayland-core --doctor` system-dependency probe. Lives in the lib
// (not the binary) so the TUI diagnostics surface can call
// `doctor::collect()` for its `/doctor` screen; `main.rs` calls
// `doctor::run()` through the lib for the `--doctor` CLI flag.
pub mod doctor;

// Wave 0 (CLI/TUI redesign): the ratatui terminal UI. `tui::run()` is the
// entry point; the `main.rs` default-mode dispatch into it is deferred to
// T2.3 (the binary is intentionally untouched in Wave 0).
pub mod tui;

// CLI surface: the shared provider-key recognizer + live key-validation.
// Extracted from `tui::surfaces::onboarding` so the onboarding surface and
// the `auth` subcommand share ONE recognizer (prefix table, env-var map,
// per-provider validation endpoints).
pub mod provider_keys;

// CLI surface: `wayland-core auth` — add / list / remove provider API
// keys directly in the global `config.toml` without the full onboarding
// flow. Lives in the lib so the TOML CRUD is unit-testable against a
// tempdir-backed config path.
pub mod auth;

// CLI surface: `wayland-core profile` — create / use / list / show / rename /
// delete / export / import isolated profiles. Lives in the lib so every verb is
// unit-testable against a tempdir-backed `WAYLAND_PROFILES_ROOT`. All
// active-pointer access stays in `wcore_config::profile` (D2 single-reader lint).
pub mod profile;

// CLI surface: `wayland-core image` — FluxRouter image generation
// (`POST /v1/images/generations`). Lives in the lib so credential
// resolution + path numbering are unit-testable.
pub mod image;

// CLI surface: `wayland-core fetch` — FluxRouter web_fetch
// (`POST /v1/fetch`). Lives in the lib so credential resolution is
// unit-testable; reuses the same Flux key/base resolution as `image`.
pub mod fetch;
