// M5.4: lib target for the `wcore-cli` crate. The binary
// (`src/main.rs`) still owns the runtime entry point; this lib exists
// so the plugin marketplace module — and now the ratatui TUI — are
// reachable from integration tests under `tests/`.

pub mod plugin;

// Shared packaged-process execution-policy and static-plugin link seam.
pub mod packaged_runtime;

/// At-most-once host budget-grant ledger shared by the JSON-stream runtime
/// and its focused acceptance tests.
pub mod budget_grants;

/// Secure lowering of JSON-stream composer files into inline image blocks.
pub mod attachments;

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

// persona-profiles PR-3': the CLI-layer `AgentRoster` impl. `wcore-acp` owns
// the transport-neutral roster seam but must not depend on the identity
// sources, so enumeration (AgentPack + the operator's global agent YAML —
// never project-supplied manifests, never isolated profiles) lives here,
// alongside the other injected bridges.
pub mod acp_roster;

// persona-profiles PR-7: the CLI-layer `ProfileRouter` impl — the profile
// SUPERVISOR. `wcore-acp` owns the transport-neutral router seam but must not
// depend on process/spawn machinery or the profile store, so the one-process-
// per-profile spawn/health-check/route/reap lifecycle lives here.
pub mod profile_router;

// FerroxLabs/wayland#1156: the parent-death channel a supervisor wires into
// each `acp serve --profile` child. Lives beside the router (its only
// producer) and `acp` (its only consumer), in the lib so its park loop is
// unit-tested without spawning a supervisor.
pub mod parent_channel;

// v0.7.0 Task 3.B.2: `agent` subcommand — five flag-driven CRUD ops
// (create / list / show / edit / delete) wrapping the
// `wcore_agents_pack::factory` user-agent surface. Lives in the lib so
// the unit tests can inject a tempdir via `run_with_base`.
pub mod agent_cmd;

// T1-E2: dirty-death crash sentinel. Lives here so its unit tests run as
// part of `cargo test -p wcore-cli --lib` instead of being trapped inside
// the binary crate.
pub mod crash_sentinel;

// B3: the process exit-code contract. Lives in the lib so both the binary and
// the end-to-end `tests/exit_code_contract.rs` name the SAME constants — a
// contract only one side can see is not a contract.
pub mod exit_code;

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

// Anvil (gated forge): `wayland-core forge "<task>"` — kill-switched engine.
pub mod anvil;
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

// Secret-safe live state projection for the JSON-stream diagnostics contract.
pub mod runtime_diagnostics;

/// wayland#896 — JSON-stream bridge for the quiesced snapshot lease. The one
/// place `wcore_protocol::quiescence` meets `wcore_config::quiesce`.
pub mod quiesce_control;

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

// CLI surface: `wayland-core migrate` — import Hermes/OpenClaw setups (#228).
pub mod migrate;

// F26-03/F26-04: CLI surface `wayland-core backup` — archive / verify / restore /
// recover a Wayland home, with a write-ahead operation journal whose recovery
// pass undoes an interrupted operation to the exact pre-operation tree. Lives in
// the lib so the journal, remap and rollback logic are testable under
// `cargo test -p wcore-cli --lib` against tempdir-backed synthetic homes.
pub mod backup;

// F23-02 (Phase 23B) — `wayland-core session`: the operator surface for
// Success Criterion 2's verbs (list, search, show, checkpoint, rewind, retry,
// fork, export, retain, reconcile, cancel). Lives in the lib so the
// integration suite can drive it without the binary.
pub mod session_cmd;

// F23-06 (Phase 23B) — `wayland-core index`: build / status / search / verify
// over `wcore-repomap`'s persistent index. Lives in the lib for the same
// reason `session_cmd` does, and because it is the instrument the phase's
// perf and retrieval-quality gates are measured through.
pub mod index_cmd;

// F23-04 (Phase 23B) — `wayland-core cache`: report / list / show / verify over
// the engine's cache + compaction ledger. Lives in the lib for the same reason
// `index_cmd` does — it is the instrument Success Criterion 4 is measured
// through, so its output format is asserted on by the integration suite.
pub mod cache_cmd;

// CLI surface: `wayland-core image` — FluxRouter image generation
// (`POST /v1/images/generations`). Lives in the lib so credential
// resolution + path numbering are unit-testable.
pub mod image;

// CLI surface: `wayland-core fetch` — FluxRouter web_fetch
// (`POST /v1/fetch`). Lives in the lib so credential resolution is
// unit-testable; reuses the same Flux key/base resolution as `image`.
pub mod fetch;

// F25-01: CLI surface `wayland-core backend` — the execution-backend operator
// surface (list / probe / run / cancel / orphans / receipt verify / diff).
pub mod backend;

// F24-B: CLI surface `wayland-core gateway` — the persistent-runtime operator
// surface (install / uninstall / start / stop / restart / status / drain) plus
// `run`, the long-lived runtime every generated service unit invokes. Lives in
// the lib so the lifecycle projection and the unit/verb agreement are testable
// under `cargo test -p wcore-cli --lib`.
pub mod gateway;
// F25-03: CLI surface `wayland-core node` — the node/device operator surface
// (identity / pair / list / show / probe / revoke / submit / attribution).
pub mod node;

// F22-04: CLI surface `wayland-core goal` — the user-reachable surface over the
// durable Goal kernel and its Fleet task ledger (open / task / run / status /
// exec-task). Lives in the lib so the idempotency gate at the effect boundary is
// testable under `cargo test -p wcore-cli --lib`, and so the kill/restart proof
// runs against the shipped binary rather than an `examples/` instrument.
pub mod goal_cmd;

// F24-03: CLI surface `wayland-core channel` — the channel operator surface
// (list / probe / health / reload). Lives in the lib so the observation
// boundary (health is only ever reported from a LIVE gateway, never
// fabricated by the reporting process) is testable under
// `cargo test -p wcore-cli --lib`.
pub mod channel;

// F28 (F-28-02-001): CLI surface `wayland-core sandbox` — the platform
// containment operator surface (status / exec). `exec` dispatches through
// `BashTool::execute_with_ctx`, the agent's OWN shell tool, so the containment
// an operator observes is the containment the agent applies — the evidence is
// transitive rather than parallel. Lives in the lib so the selector's
// bypass refusal and the contained-profile context are testable under
// `cargo test -p wcore-cli --lib`.
pub mod sandbox_cmd;

// The single chokepoint that guarantees a `--json-stream` host is told WHY the
// engine refused to start. Before this, three of four startup refusal paths
// exited with ZERO protocol frames and put the reason on stderr, which the
// protocol consumer does not read. Lives in the lib so the decision rule is
// testable under `cargo test -p wcore-cli --lib`, while the end-to-end proof
// drives the real binary and reads its stdout as the host does.
pub mod startup_error;

// 23A-C1: governed skill promotion, revocation and rollback, on the binary the
// release actually ships. The capability existed in `wcore-skills` and in a
// `wcore-skill-govern` helper that is packaged by nothing, so no installed copy of
// the product could reach it.
pub mod skill_govern;

// Size-bounded rotation for `$WAYLAND_HOME/logs/wayland-core.log`. Lives in the
// lib so the rotation invariants — that a rotation happens AND that the bytes
// it keeps are the newest — are testable under `cargo test -p wcore-cli --lib`,
// while the fallback-when-the-log-cannot-be-opened path is proven against the
// real binary in `tests/log_rotation.rs`.
pub mod log_rotate;
