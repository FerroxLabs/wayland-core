// Core agent infrastructure: engine, session, orchestration, output sinks.

pub mod agents;
pub mod agents_md;
pub mod approval;
// v0.8.1 U6 — autonomous skill creation. Records turn trajectories,
// buckets them by normalized task signature, and drafts a candidate
// skill + records into GEPA's PromptStore once a signature accumulates
// N consecutive successes. Closes the substrate's self-improvement loop:
// runtime observation → draft → PromptStore → U1 SkillRouter seed.
pub mod auto_skill;
pub mod bootstrap;
// W8a A.2: ExecutionBudget + ExecutionBudgetView (S2 foundation).
pub mod budget;
pub mod cache_diagnostics;
// W8a A.2: cooperative cancellation primitives (re-export of tokio-util).
pub mod cancel;
// Inbound channel consumer: subscribes to the ChannelManager broadcast,
// runs the pure dispatch kernel (wcore_channels::evaluate), and on admit
// drives an agent turn through the TurnDispatcher seam, then sends the
// reply back. Completes the inbound path that was structurally missing.
pub mod channel_inbound;
// Channel tool posture: maps a per-channel `ChannelToolPosture` onto a
// reduced/jailed toolset for channel-originated engines (closes remote
// host-secret exfiltration). Consumed by `bootstrap` + `channel_dispatch`.
pub mod channel_tools;
// Phase 1B-2 — the real engine-backed `TurnDispatcher`. Builds one
// per-session `AgentEngine` (via AgentBootstrap, `.without_channels(true)`
// to avoid channel recursion) and drives an agent turn from each admitted
// inbound message, returning the reply text the subscriber sends back.
pub mod channel_dispatch;
// Inbound media enrichment: resolves channel attachments (image/audio) to
// derived text (description/transcript) via the host-wired vision /
// transcription tools before the turn prompt is built. Inert when no
// backend is configured. Consumed by `channel_dispatch` + `bootstrap`.
pub mod channel_media;
// FleetDispatcher-class fix (audit 2026-05-24): bridges SendMessageTool's
// `MessageTransport` boundary to the host's `ChannelManager` so the LLM
// can drive Telegram/Discord/Slack/etc. through user-configured channels.
pub mod channel_send_transport;
pub mod compact;
pub mod confirm;
pub mod context;
// v0.8.1 U7 — production wire-up for `wcore-cron`. `bootstrap.rs`
// spawns a `CronRunner` with the `EngineJobHandler` defined here.
pub mod cron;
// B2: the real egress policy (allowlist + ask-with-memory + exfil-class)
// installed into the B1 wcore-egress chokepoint at bootstrap.
pub mod egress;
pub mod engine;
// W8b C.6: FileHistory snapshot store for Rollback (root-level RealFs).
pub mod file_history;
// v0.9.0 Wave-4 E2 — provider health probes (HTTP HEAD/GET against
// `/v1/models` with 5s cap) for the `/doctor` TUI diagnostics surface.
pub mod health;
pub mod hooks;
// #537/#141 — host-delegated `send_message` transport: when the host
// spawns the engine with `WAYLAND_SEND_MESSAGE_HOST_DELEGATE=1`, sends are
// fulfilled by the host over the json-stream protocol
// (`host_send_message_request` event / `host_send_message_result` command)
// instead of the engine's own channel table.
pub mod host_send_transport;
// Inbound webhook HTTP host — routes platform webhook POSTs (Slack /
// WhatsApp / Twilio SMS) to each channel's signature-verifying
// `Channel::ingest_webhook` via the `ChannelManager`.
pub mod inbound_webhook;
pub mod mcp_curator;
pub mod orchestration;
pub mod output;
pub mod plan;
pub mod plugins;
// v0.7.0 Task 1.C.1 — WAYLAND.md / AGENTS.md / .wayland/context.md /
// CLAUDE.md auto-detection.
pub mod project_context;
pub mod user_context;
// v0.6.1 hardening (CRIT-1) — opt-in wcore-permissions gate at tool
// dispatch boundary; without this, the M5.8 ACL shipped in v0.6.0 was
// orphan code.
pub mod policy_gate;
pub mod resilient_reporter;
// W8b C.7: RollbackTool — consumes FileHistory to restore prior states.
pub mod rollback_tool;
pub mod session;
// v0.9.0 W1 B7 — in-process live state surfaces for wayland_status +
// wayland_telemetry_query tools (introspection backend reads from this).
pub mod session_state;
pub mod skill_tool;
// v0.7.0 Task 3.C.1 — slash-command parser + dispatcher (stub handlers
// for the 8 built-ins; 3.C.2 swaps stubs for real implementations).
pub mod slash;
pub mod spawn_tool;
pub mod spawner;
pub mod style_detector;
// B1 — `WorkflowTool`: LLM-facing surface for the dynamic-workflow engine
// (parses inline RON via `WorkflowPlan`, runs it via `WorkflowRunner`).
pub mod workflow_tool;
// B7 — natural-language → RON workflow synthesis (one-shot LLM call, validates
// via `WorkflowPlan::parse`, re-prompts once on invalid RON then aborts).
pub mod workflow_synth;
// v0.9.0 Wave-1 B0 — shared OAuth subsystem. Provides `OAuthFlow`,
// PKCE-S256 by default, encrypted token storage, and single-flight
// refresh. Wired into providers (google_meet et al.) by Wave-1 B9.
pub mod oauth;
// v0.6.3 D.0: real HTTP backends for the API-seam catalog tools
// (github/gitlab/linear/notion) over wcore-providers::http_client.
// v0.9.0 Wave-1 B0 (2026-05-27): split into `tool_backends/<file>.rs`
// per backend so parallel Wave-1 sub-agents add new backends without
// colliding on shared lines (R-B1 structural fix).
pub mod tool_backends;
// W8b — per-tool ExecutionBudget tracking (call counts + runtime aggregation).
pub mod tool_budget;
pub mod vcr;
// W8b D.2: filesystem watcher (notify-rs) for external-edit detection.
pub mod watch;
// W8b.2.A: adapter that lets FileWatcher serve as a wcore-tools
// FileWriteNotifier without inverting the dep edge.
pub mod file_watcher_notifier;

// W7 Pre-flight 0.0d: test-driver helpers (ScriptedProvider, TestSink,
// run_synthetic_turn, build_for_test). Gated behind `test-utils` so it
// never ships in release binaries; consumers enable the feature in their
// `dev-dependencies`.
#[cfg(any(test, feature = "test-utils"))]
pub mod test_utils;

// Re-export the skills crate so existing callers (wcore-cli, tests) can use
// `wcore_agent::skills::` without changing their import paths.
pub use wcore_skills as skills;
