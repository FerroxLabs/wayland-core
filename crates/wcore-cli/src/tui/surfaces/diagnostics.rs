//! Diagnostics surfaces — `/doctor`, `/cost`, and `/memory`.
//!
//! Three compact diagnostic screens implemented as one [`Surface`] with a
//! mode switch:
//!
//! - **Doctor** — system dependency + environment health. Runs the real
//!   [`crate::doctor::collect`] probe on `on_enter` and the `r` re-run
//!   key; the same data that `wayland-core --doctor` prints.
//! - **Cost** — session token usage and spend, read live from
//!   [`App::cost`] (populated by the protocol bridge from
//!   [`ProtocolEvent::SessionCost`] events).
//!   ([`wcore_protocol::events::ProtocolEvent::SessionCost`])
//! - **Memory** — what long-term memory holds, scanned from the project
//!   memory directory via `wcore_memory::store`, with a real delete.
//!
//! ## Live wiring (Wave 3)
//!
//! - **Doctor** — `on_enter` and the `r` key call [`crate::doctor::collect`]
//!   on a short-lived worker thread (the probe is async; the surface
//!   hooks are sync), then convert its [`crate::doctor::CheckResult`] rows
//!   into [`HealthCheck`] rows.
//! - **Cost** — `render` reads [`App::cost`] every frame, so cost events
//!   that stream in while the screen is open are reflected immediately.
//! - **Memory** — `on_enter` scans `auto_memory_dir(cwd)` with
//!   `scan_memory_files`; the `d` key deletes the selected entry with
//!   `delete_memory` and re-scans.
//!
//! The OSC-9 desktop-notification helper ([`osc9_notification`]) and the
//! terminal-bell helper ([`terminal_bell`]) are pure string functions so
//! they are unit-testable without a terminal.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};
use wcore_agent::health::{self, HealthStatus, ProviderHealth as AgentProviderHealth};

use crate::doctor;
use crate::tui::app::{App, TurnRole};
use crate::tui::surfaces::{Surface, SurfaceAction, SurfaceId};
use crate::tui::theme::Theme;
use crate::tui::turn_element::TurnElement;
use crate::tui::widgets::panel;

// ─────────────────────────────────────────────────────────────────────────
// OSC-9 / terminal-bell notification helpers
// ─────────────────────────────────────────────────────────────────────────

/// The reason a desktop notification is being posted.
///
/// The TUI fires a notification only when the terminal is unfocused —
/// either an action is needed from the user, or a long task has just
/// finished — so the message copy is keyed to one of these two cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKind {
    /// A tool call is waiting on the user's approval.
    ApprovalNeeded,
    /// A long-running task finished while the user was away.
    TaskFinished,
}

impl NotifyKind {
    /// The human-readable notification body for this kind.
    fn message(self) -> &'static str {
        match self {
            NotifyKind::ApprovalNeeded => "wayland-core: approval needed",
            NotifyKind::TaskFinished => "wayland-core: task finished",
        }
    }
}

/// Build an [OSC-9] desktop-notification escape sequence.
///
/// OSC-9 is the widely supported "post a desktop notification" control
/// string: `ESC ] 9 ; <text> BEL`. iTerm2, kitty, WezTerm, and Ghostty
/// all honor it; terminals that don't simply ignore the sequence (it is
/// inert text to them), so emitting it is always safe.
///
/// The returned string is meant to be written straight to the terminal.
/// Callers should only emit it when the terminal is unfocused — see
/// [`maybe_notify`].
///
/// [OSC-9]: https://iterm2.com/documentation-escape-codes.html
pub fn osc9_notification(kind: NotifyKind) -> String {
    // ESC ] 9 ; <message> BEL
    format!("\x1b]9;{}\x07", kind.message())
}

/// The terminal-bell control character (`BEL`, `0x07`).
///
/// A bare bell is the lowest-common-denominator attention signal for
/// terminals with no OSC-9 support. It is paired with the OSC-9 sequence
/// by [`maybe_notify`] so a notification lands on every terminal.
pub fn terminal_bell() -> &'static str {
    "\x07"
}

/// Build the full attention signal for an unfocused terminal: an OSC-9
/// desktop notification followed by a terminal bell.
///
/// Returns `None` when `focused` is `true` — a focused user is already
/// looking at the TUI and does not need a desktop interrupt. This is the
/// single decision point the TUI event loop calls; keeping the
/// focused-check here means no call site can forget it.
pub fn maybe_notify(kind: NotifyKind, focused: bool) -> Option<String> {
    if focused {
        return None;
    }
    Some(format!("{}{}", osc9_notification(kind), terminal_bell()))
}

// ─────────────────────────────────────────────────────────────────────────
// Local view models — populated by Wave 2 (see module docs)
// ─────────────────────────────────────────────────────────────────────────

/// Outcome of one `/doctor` health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthState {
    /// The check passed.
    Ok,
    /// The check failed in a non-fatal way (an optional dependency).
    Warn,
    /// The check failed and the dependency is required.
    Fail,
}

impl HealthState {
    /// The status glyph painted in the left column.
    fn glyph(self) -> &'static str {
        match self {
            HealthState::Ok => "●",
            HealthState::Warn => "▲",
            HealthState::Fail => "✕",
        }
    }

    /// The theme color for this state.
    fn color(self, t: &Theme) -> ratatui::style::Color {
        match self {
            HealthState::Ok => t.success,
            HealthState::Warn => t.warning,
            HealthState::Fail => t.error,
        }
    }
}

/// One row of the `/doctor` report — a named check and its outcome.
#[derive(Debug, Clone)]
pub struct HealthCheck {
    /// Human-readable check label (e.g. `Anthropic API key`).
    pub label: String,
    /// The check outcome.
    pub state: HealthState,
    /// A short detail string (the discovered value or the failure hint).
    pub detail: String,
}

impl HealthCheck {
    /// Construct a health check row.
    pub fn new(label: impl Into<String>, state: HealthState, detail: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            state,
            detail: detail.into(),
        }
    }
}

/// The `/doctor` report: system dependency + environment health.
///
/// Built from a live [`crate::doctor::collect`] run by [`spawn_health_probe`];
/// `Default` is the empty report shown while the first probe is in flight.
#[derive(Debug, Clone, Default)]
pub struct DoctorReport {
    /// The individual check rows, in display order.
    pub checks: Vec<HealthCheck>,
}

// ─────────────────────────────────────────────────────────────────────────
// v0.9.0 Wave-4 E2 — extended `/doctor` sections
// ─────────────────────────────────────────────────────────────────────────

/// One well-known external-service tool and the env var that gates it.
///
/// The agent's `ToolRegistry::register` silently skips any tool whose
/// `is_available()` returns false (the audit pattern for `Null*Backend`
/// defaults), so the host's running registry shows only enabled tools.
/// The `/doctor` surface lists the *full* set of host-known tools — both
/// enabled and not — so a user can see which env var would upgrade a
/// disabled tool. This table is the static catalog of those gates.
///
/// The list is intentionally curated, not auto-discovered: the only
/// thing a user needs to know from `/doctor` is "set X to enable Y."
struct ToolGate {
    /// The tool's host-facing name (matches `Tool::name()`).
    name: &'static str,
    /// The env var the host probes to construct a real backend (e.g.
    /// `DISCORD_BOT_TOKEN`). When set, the tool is enabled.
    env_var: &'static str,
}

/// The curated list of external-service tools and their gating env vars.
/// Source of truth: each tool's `tool_backends::build_*` in `wcore-agent`.
const TOOL_GATES: &[ToolGate] = &[
    ToolGate {
        name: "github",
        env_var: "GITHUB_TOKEN",
    },
    ToolGate {
        name: "gitlab",
        env_var: "GITLAB_TOKEN",
    },
    ToolGate {
        name: "linear",
        env_var: "LINEAR_API_KEY",
    },
    ToolGate {
        name: "notion",
        env_var: "NOTION_TOKEN",
    },
    ToolGate {
        name: "discord",
        env_var: "DISCORD_BOT_TOKEN",
    },
    ToolGate {
        name: "spotify",
        env_var: "SPOTIFY_REFRESH_TOKEN",
    },
    ToolGate {
        name: "google_meet",
        env_var: "GOOGLE_OAUTH_CLIENT_ID",
    },
    ToolGate {
        name: "web_search",
        env_var: "BRAVE_API_KEY",
    },
    ToolGate {
        name: "web_fetch",
        env_var: "WAYLAND_FETCH_ALLOWED",
    },
    ToolGate {
        name: "transcribe_audio",
        env_var: "OPENAI_API_KEY",
    },
    ToolGate {
        name: "vision_analyze",
        env_var: "OPENAI_API_KEY",
    },
    ToolGate {
        name: "tts",
        env_var: "OPENAI_API_KEY",
    },
];

/// One row of the per-tool backend-status section.
#[derive(Debug, Clone)]
pub struct ToolStatusRow {
    /// The tool's host-facing name.
    pub name: String,
    /// Whether the env var that gates the tool is set (non-empty).
    pub enabled: bool,
    /// The gating env var the user would set to enable the tool.
    pub env_var: String,
}

/// Snapshot of one recent engine error for the `/doctor` errors panel.
#[derive(Debug, Clone)]
pub struct RecentErrorRow {
    /// Index of the system turn in `app.session.turns` this came from
    /// (debug aid — never rendered).
    pub turn_index: usize,
    /// The error message (already pre-formatted by `protocol_bridge`).
    pub message: String,
}

/// Snapshot of the token budget for the active model.
#[derive(Debug, Clone, Default)]
pub struct TokenBudgetView {
    /// The active model identifier (e.g. `claude-sonnet-4-6`).
    pub model: String,
    /// Tokens currently occupying the context window.
    pub used_tokens: u64,
    /// Total context-window size.
    pub window_size: u64,
    /// Output tokens for the current/last turn.
    pub last_turn_output: u64,
}

/// How long the surface waits for the provider-health probe before it stops
/// showing "probing…" and reports a timeout. Set above the underlying 5s
/// per-request cap (plus slack for the concurrent set) so a healthy-but-slow
/// network still lands normally; only a genuinely stalled probe trips it.
const HEALTH_PROBE_UI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(8);

/// One result from the off-thread `/doctor` probe. The two probes are
/// **independent** — the fast system-dependency scan (`which` subprocesses)
/// is delivered the instant it finishes, without waiting for the slow live
/// provider-health HTTP probes — so SYSTEM/DISCOVERED fill in immediately
/// while PROVIDERS is still in flight.
enum ProbeMsg {
    /// The converted system-dependency report (`which` checks).
    Doctor(DoctorReport),
    /// The live provider-health rows (real HTTP probes).
    Health(Vec<AgentProviderHealth>),
}

/// Start the two slow `/doctor` probes on a **detached** worker thread and
/// return a receiver that delivers each result as it lands.
///
/// This is the fix for the `on_enter` UI-thread freeze: `doctor::collect`
/// (which spawns subprocesses) and `health::provider_health_check_all`
/// (live HTTP) are async, but the surface's `on_enter`/`handle_key` hooks
/// are synchronous and run inside the TUI's tokio runtime. The previous
/// implementation drove them on a worker thread but then **joined** it,
/// blocking the render loop — and when the HTTP probes stalled (a
/// restrictive egress layer, or many connected providers exceeding the
/// per-probe cap), the entire TUI froze and the Diagnostics surface never
/// painted. Here the thread is left running; the surface renders a
/// "probing…" state immediately and `tick`→[`DiagnosticsSurface::poll_health`]
/// fills in SYSTEM/PROVIDERS/DISCOVERED as each result lands (the same
/// async-poll pattern as the paste-detect modal). The two probes are
/// dispatched concurrently and each sends on its own, so the fast system
/// scan is never held hostage by the slow provider HTTP probe.
fn spawn_health_probe() -> std::sync::mpsc::Receiver<ProbeMsg> {
    let (tx, rx) = std::sync::mpsc::channel();
    // If the thread fails to spawn, both senders drop and the receiver
    // resolves to `Disconnected` — the surface treats that as "no result"
    // and stops waiting rather than hanging on a perpetual "probing…".
    let _ = std::thread::Builder::new()
        .name("wld-doctor-probe".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            rt.block_on(async {
                let tx_doctor = tx.clone();
                let doctor_task = async move {
                    let raw = doctor::collect().await;
                    let report = DoctorReport {
                        checks: raw.checks.iter().map(health_check_from).collect(),
                    };
                    // A dropped receiver (surface left the screen) is an
                    // expected no-op, not an error.
                    let _ = tx_doctor.send(ProbeMsg::Doctor(report));
                };
                let health_task = async move {
                    let health = health::provider_health_check_all().await;
                    let _ = tx.send(ProbeMsg::Health(health));
                };
                tokio::join!(doctor_task, health_task);
            });
        });
    rx
}

/// Build the per-tool backend-status snapshot by walking [`TOOL_GATES`]
/// and probing each tool's gating env var. A tool is "enabled" only
/// when its env var is set and non-empty (matching the
/// `build_*_backend` resolver's empty-string-is-no-key semantic).
fn scan_tool_status() -> Vec<ToolStatusRow> {
    TOOL_GATES
        .iter()
        .map(|gate| ToolStatusRow {
            name: gate.name.to_string(),
            enabled: env_set_nonempty(gate.env_var),
            env_var: gate.env_var.to_string(),
        })
        .collect()
}

/// Return true iff the named env var is set AND non-empty. Empty
/// strings are treated as "unset" — matching the agent's tool-backend
/// resolvers (e.g. `build_discord_backend` returns `None` for both
/// `Err` and an empty value).
fn env_set_nonempty(name: &str) -> bool {
    matches!(std::env::var(name), Ok(v) if !v.is_empty())
}

/// S8: build the **config-posture** health rows from the resolved
/// [`ConfigView`](crate::tui::app::ConfigView) snapshot on `App`.
///
/// These are coherence/safety checks on the user's *configuration* — distinct
/// from the SYSTEM (binaries), PROVIDERS (live HTTP), and TOOLS (env gates)
/// sections, which probe the runtime environment. Every row reads real config
/// values plumbed by S5–S7 (egress allowlist, credential backend, spend cap,
/// failover chain, tool-approval posture) plus one cheap filesystem resolve
/// for the memory directory. A valid-but-permissive state is surfaced as
/// `Warn` (never a fake `Fail`); a benign "off" state is an honest `Ok`.
fn scan_config_health(app: &App) -> Vec<HealthCheck> {
    let c = &app.config;
    let mut rows = Vec::new();

    // Egress guard — off means outbound calls are not gated.
    rows.push(if c.security_egress_enabled {
        let n = c.egress_allow.len();
        HealthCheck::new(
            "egress guard",
            HealthState::Ok,
            format!(
                "on · {n} extra allowlist entr{}",
                if n == 1 { "y" } else { "ies" }
            ),
        )
    } else {
        HealthCheck::new(
            "egress guard",
            HealthState::Warn,
            "off — outbound network calls are not restricted",
        )
    });

    // Credential storage backend — plaintext is the permissive default.
    rows.push(match c.storage_backend.as_str() {
        "keyring" => HealthCheck::new("credential store", HealthState::Ok, "OS keyring"),
        "encrypted-file" => HealthCheck::new("credential store", HealthState::Ok, "encrypted file"),
        _ => HealthCheck::new(
            "credential store",
            HealthState::Warn,
            "plaintext file (0600) — Advanced → keyring to harden",
        ),
    });

    // Tool approval posture — auto-approve runs every tool call unprompted.
    rows.push(if c.tools_auto_approve {
        HealthCheck::new(
            "tool approval",
            HealthState::Warn,
            "auto-approve — every tool call runs without a prompt",
        )
    } else {
        HealthCheck::new(
            "tool approval",
            HealthState::Ok,
            format!("ask each · {} pre-approved", c.tools_allow_list.len()),
        )
    });

    // Provider failover chain (S7) — on with no chain does nothing.
    rows.push(if c.failover_enabled {
        if c.fallback_models.is_empty() {
            HealthCheck::new(
                "provider failover",
                HealthState::Warn,
                "on but no fallback models configured",
            )
        } else {
            let n = c.fallback_models.len();
            HealthCheck::new(
                "provider failover",
                HealthState::Ok,
                format!("on · {n} fallback model{}", if n == 1 { "" } else { "s" }),
            )
        }
    } else {
        HealthCheck::new(
            "provider failover",
            HealthState::Ok,
            "off — single provider",
        )
    });

    // Spend cap (S5) — informational; "no cap" is a valid choice.
    rows.push(match c.budget_max_cost_usd {
        Some(cap) => HealthCheck::new(
            "spend cap",
            HealthState::Ok,
            format!("${cap:.2} per session"),
        ),
        None => HealthCheck::new("spend cap", HealthState::Ok, "no cap set"),
    });

    // Long-term memory — when on, the store directory must resolve.
    rows.push(if c.memory_enabled {
        match std::env::current_dir()
            .ok()
            .and_then(|cwd| wcore_memory::paths::auto_memory_dir(&cwd))
        {
            Some(dir) => HealthCheck::new(
                "long-term memory",
                HealthState::Ok,
                format!("on · {}", dir.display()),
            ),
            None => HealthCheck::new(
                "long-term memory",
                HealthState::Warn,
                "on · could not resolve the memory directory",
            ),
        }
    } else {
        HealthCheck::new("long-term memory", HealthState::Ok, "off")
    });

    rows
}

/// S11: one "here's what I found" discovery row — a latent capability on this
/// machine the user could wire up (ambient cloud creds, a local Ollama daemon,
/// an existing Claude Desktop MCP config). Read-only: detection only, never
/// mutates config.
#[derive(Debug, Clone, PartialEq)]
pub struct Discovery {
    /// Short capability label (e.g. `AWS / Bedrock`).
    pub label: String,
    /// Whether the capability was detected on this host.
    pub available: bool,
    /// A one-line detail: what was found, or how to enable it.
    pub detail: String,
}

/// The Claude Desktop config path on this platform
/// (`<config-dir>/Claude/claude_desktop_config.json`). Uses the real OS config
/// dir — Claude Desktop is an external app, so this deliberately does NOT honor
/// `WAYLAND_HOME` (we are discovering the actual machine).
fn claude_desktop_config_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_default()
        .join("Claude")
        .join("claude_desktop_config.json")
}

/// Count the MCP servers declared in a Claude Desktop config file. Returns
/// `None` if the file is absent/unreadable/unparseable or has no `mcpServers`
/// object — the discovery row reads that as "not detected".
fn count_mcp_servers_in(path: &std::path::Path) -> Option<usize> {
    let body = std::fs::read_to_string(path).ok()?;
    let json: serde_json::Value = serde_json::from_str(&body).ok()?;
    Some(json.get("mcpServers")?.as_object()?.len())
}

/// S11: probe the machine for latent capabilities Wayland could use. Reuses the
/// single-source-of-truth ambient/OAuth detection in
/// [`wcore_config::config::provider_connected`] and the already-collected
/// Ollama doctor signal; the only fresh probe is the Claude Desktop config scan.
fn scan_environment(ollama_available: bool) -> Vec<Discovery> {
    use wcore_config::config::{ProviderType, provider_connected};

    let mut rows = Vec::new();

    rows.push(Discovery {
        label: "Ollama (local models)".to_string(),
        available: ollama_available,
        detail: if ollama_available {
            "detected — route local models with `ollama:<model>`".to_string()
        } else {
            "not found — install ollama or set OLLAMA_BASE_URL".to_string()
        },
    });

    let aws = provider_connected(ProviderType::Bedrock);
    rows.push(Discovery {
        label: "AWS / Bedrock".to_string(),
        available: aws,
        detail: if aws {
            "ambient AWS credentials detected — Bedrock is ready".to_string()
        } else {
            "no AWS credentials (env / ~/.aws / role)".to_string()
        },
    });

    let gcp = provider_connected(ProviderType::Vertex);
    rows.push(Discovery {
        label: "GCP / Vertex".to_string(),
        available: gcp,
        detail: if gcp {
            "ambient GCP credentials detected — Vertex is ready".to_string()
        } else {
            "no GCP credentials (GOOGLE_APPLICATION_CREDENTIALS / ADC)".to_string()
        },
    });

    let chatgpt = provider_connected(ProviderType::OpenAIChatGpt);
    rows.push(Discovery {
        label: "ChatGPT (OAuth)".to_string(),
        available: chatgpt,
        detail: if chatgpt {
            "stored ChatGPT login detected".to_string()
        } else {
            "not signed in — run `wayland auth login chatgpt`".to_string()
        },
    });

    let mcp = count_mcp_servers_in(&claude_desktop_config_path());
    rows.push(Discovery {
        label: "Claude Desktop MCP".to_string(),
        available: mcp.is_some_and(|n| n > 0),
        detail: match mcp {
            Some(n) if n > 0 => format!(
                "{n} MCP server{} configured — importable",
                if n == 1 { "" } else { "s" }
            ),
            _ => "no Claude Desktop MCP config found".to_string(),
        },
    });

    // Slice 3 — Forge-suite local MCP servers advertised in the cross-app
    // discovery file (e.g. Agent Vault). These are hints, not liveness — the
    // live probe + grant happen on `/mcp connect`, so this row just surfaces
    // that one is available to link. One row per discovered server.
    for s in wcore_config::forge_discovery::read_discovered_servers() {
        rows.push(Discovery {
            label: format!("Forge MCP: {}", s.label()),
            available: true,
            detail: format!("discovered at {} — run /mcp connect to link", s.url),
        });
    }

    rows
}

/// Extract the last 10 engine errors from the session transcript.
/// The protocol bridge pushes each `ProtocolEvent::Error` as a
/// `TurnRole::System` turn whose first markdown element starts with
/// one of the marker prefixes this detector matches.
///
/// v0.9.1.1 H3: previously matched on `Error [` (the pre-fix shape).
/// After the H3 fix, the bridge maps known internal classes
/// (`engine_error`, `engine_panic`, `rate_limit`) to user-facing
/// labels (`Error:`, `Turn ended unexpectedly:`, `Rate limit:`).
/// Unknown classes fall back to the `Error [<code>]:` chip shape so
/// new failure categories still surface here.
fn collect_recent_errors(app: &App) -> Vec<RecentErrorRow> {
    const MAX: usize = 10;
    let mut rows: Vec<RecentErrorRow> = Vec::new();
    for (i, turn) in app.session.turns.iter().enumerate().rev() {
        if turn.role != TurnRole::System {
            continue;
        }
        // Walk the typed elements (Markdown only — Thinking/Sources are
        // not error carriers). Match any of the known error-line
        // shapes the protocol bridge emits.
        for element in &turn.elements {
            if let TurnElement::Markdown(text) = element
                && (text.starts_with("Error [")
                    || text.starts_with("Error:")
                    || text.starts_with("Turn ended unexpectedly")
                    || text.starts_with("Rate limit")
                    || text.starts_with("Budget exceeded "))
            {
                rows.push(RecentErrorRow {
                    turn_index: i,
                    message: text.clone(),
                });
                break;
            }
        }
        if rows.len() >= MAX {
            break;
        }
    }
    rows
}

/// Build the token-budget snapshot from `App` state.
fn token_budget_view(app: &App) -> TokenBudgetView {
    TokenBudgetView {
        model: app.config.model.clone(),
        used_tokens: app.context.used_tokens,
        window_size: app.context.window_size,
        last_turn_output: app.session.tokens_out,
    }
}

/// Convert a [`HealthStatus`] into a [`HealthState`] so the doctor row
/// helpers (glyph + theme color) work uniformly across the four
/// sections.
fn health_state_from_status(status: HealthStatus) -> HealthState {
    match status {
        HealthStatus::Green => HealthState::Ok,
        HealthStatus::Yellow => HealthState::Warn,
        HealthStatus::Red => HealthState::Fail,
    }
}

/// Convert one [`crate::doctor::CheckResult`] into a [`HealthCheck`] row.
///
/// `Pass` → `Ok`; `Fail` → `Fail`; `Warn`/`Skip`/`Manual` → `Warn`
/// (none of those flip the doctor exit code, so they are non-fatal in the
/// surface's three-state model). The detail string mirrors what
/// `--doctor` prints for that row.
fn health_check_from(c: &doctor::CheckResult) -> HealthCheck {
    use doctor::Outcome;
    let (state, detail) = match &c.outcome {
        Outcome::Pass { detail } => (HealthState::Ok, detail.clone()),
        Outcome::Fail { hints } => (
            HealthState::Fail,
            match hints.first() {
                Some(h) => format!("not found — {h}"),
                None => "not found".to_string(),
            },
        ),
        Outcome::Warn { detail, .. } => (HealthState::Warn, detail.clone()),
        Outcome::Skip { reason } => (HealthState::Warn, format!("skipped ({reason})")),
        Outcome::Manual { hint } => (HealthState::Warn, hint.clone()),
    };
    HealthCheck::new(c.label, state, detail)
}

/// One long-term memory entry shown in the `/memory` report.
#[derive(Debug, Clone)]
pub struct MemoryItem {
    /// A stable id shown in the UI — the entry filename (e.g.
    /// `user_role.md`).
    pub id: String,
    /// The full on-disk path to the entry, used by the delete affordance.
    pub path: std::path::PathBuf,
    /// The memory category (`user`, `feedback`, `project`, `reference`).
    pub category: String,
    /// A one-line summary of what the entry holds.
    pub summary: String,
}

/// The `/memory` report: what long-term memory currently holds.
#[derive(Debug, Clone, Default)]
pub struct MemoryReport {
    /// The stored memory entries, newest first.
    pub items: Vec<MemoryItem>,
}

// ─────────────────────────────────────────────────────────────────────────
// DiagnosticsSurface
// ─────────────────────────────────────────────────────────────────────────

/// Which diagnostic screen is in view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagMode {
    /// `/doctor` — provider / key / MCP health.
    Doctor,
    /// `/cost` — session token usage and spend.
    Cost,
    /// `/memory` — long-term memory contents + delete.
    Memory,
    /// `/effective` — the resolved effective config, redacted (S9).
    Effective,
}

impl DiagMode {
    /// The modes in tab order.
    const ALL: [DiagMode; 4] = [
        DiagMode::Doctor,
        DiagMode::Cost,
        DiagMode::Memory,
        DiagMode::Effective,
    ];

    /// The slash-command label for this mode.
    fn label(self) -> &'static str {
        match self {
            DiagMode::Doctor => "/doctor",
            DiagMode::Cost => "/cost",
            DiagMode::Memory => "/memory",
            DiagMode::Effective => "/effective",
        }
    }
}

/// The `/doctor` `/cost` `/memory` diagnostics surface.
///
/// All three screens live here because they are small, related, and
/// share the same chrome. Local UI state (the active mode, the `/memory`
/// selection cursor, the doctor + memory report data) lives on this
/// struct; the `/cost` data is read live from [`App::cost`] at render
/// time, so cost events that arrive while the screen is open show
/// immediately.
pub struct DiagnosticsSurface {
    /// Which screen is currently shown.
    mode: DiagMode,
    /// The `/doctor` report — refreshed by [`run_doctor`] on `on_enter`
    /// and the `r` re-run key.
    doctor: DoctorReport,
    /// The `/memory` report — scanned from the project memory dir on
    /// `on_enter` and after a delete.
    memory: MemoryReport,
    /// The selected row in the `/memory` list, for the delete affordance.
    memory_cursor: usize,
    /// Two-step delete confirmation arm. `Some(idx)` means a first `d` was
    /// pressed on row `idx` and a second `d` on that same row will perform
    /// the irreversible disk delete. Any cursor move or other key clears it
    /// back to `None`, so the destructive action always needs an explicit,
    /// in-place second confirmation. (D043)
    delete_armed: Option<usize>,
    /// Whether the `/doctor` report has been collected at least once.
    /// `false` for the single frame between construction and the first
    /// `on_enter`, when the report is still empty.
    doctor_collected: bool,
    /// v0.9.0 W4 E2 — live provider health rows (Anthropic / OpenAI /
    /// Gemini / Groq), refreshed on `on_enter` + the `r` key.
    provider_health: Vec<AgentProviderHealth>,
    /// v0.9.0 W4 E2 — per-tool backend status rows derived from
    /// `TOOL_GATES` + a live env-var probe. Cheap to recompute, but
    /// caching it keeps the render path free of `std::env::var` calls.
    tool_status: Vec<ToolStatusRow>,
    /// S8 — config-posture health rows built from the resolved `ConfigView`
    /// on `App` (egress / credentials / tool approval / failover / spend cap /
    /// memory). Refreshed alongside the doctor probe on `on_enter` + `r`.
    config_checks: Vec<HealthCheck>,
    /// S9 — the rendered effective-config TOML (redacted) for the `/effective`
    /// mode, built on `on_enter` via `wcore_config::config::effective_config_toml`.
    /// Holds an error message string if the render failed.
    effective_toml: String,
    /// Vertical scroll offset for the `/effective` view (the TOML can exceed
    /// the viewport). Clamped on `Up`/`Down`/`PageUp`/`PageDown`.
    effective_scroll: u16,
    /// S10 — secret-free summaries of the on-disk channel configs (the
    /// "Integrations ghost" made visible). Scanned from the canonical
    /// `channels_dir()` on `on_enter` + the `r` key, rendered as the CHANNELS
    /// section of `/doctor`.
    channels: Vec<wcore_channels_registry::ChannelSummary>,
    /// S11 — "here's what I found" environment discovery: latent capabilities
    /// on this machine (ambient cloud creds, local Ollama, Claude Desktop MCP)
    /// the user could wire up. Rendered as the DISCOVERED section of `/doctor`.
    discovered: Vec<Discovery>,
    /// In-flight handle for the async system + provider-health probe. `Some`
    /// while either probe is running (started by `on_enter` / the `r` key);
    /// `tick` polls it and clears it back to `None` once both results land.
    /// Keeping the probe off the synchronous `on_enter` path is what stops the
    /// live HTTP probes from freezing the whole TUI (see [`spawn_health_probe`]).
    health_pending: Option<std::sync::mpsc::Receiver<ProbeMsg>>,
    /// Whether the live provider-health rows have landed for the current probe.
    /// Distinct from `doctor_collected` (the system scan) because the two
    /// probes resolve independently — the fast system scan should not gate the
    /// PROVIDERS "probing…" state on the slow HTTP probe, and vice-versa.
    health_collected: bool,
    /// When the current probe was started, for the UI-side health timeout.
    probe_started: Option<std::time::Instant>,
    /// Set when the provider-health probe exceeded [`HEALTH_PROBE_UI_TIMEOUT`]
    /// without landing. The HTTP probe carries its own per-request cap, but a
    /// hostile egress layer can stall it beyond that; rather than show
    /// "probing…" forever we give up waiting and say so honestly.
    health_timed_out: bool,
}

impl Default for DiagnosticsSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl DiagnosticsSurface {
    /// Construct the surface on the `/doctor` screen. The doctor + memory
    /// reports are populated by `on_enter` (the doctor probe is async and
    /// the memory scan touches the filesystem — neither belongs in a
    /// constructor).
    pub fn new() -> Self {
        Self {
            mode: DiagMode::Doctor,
            doctor: DoctorReport::default(),
            memory: MemoryReport::default(),
            memory_cursor: 0,
            delete_armed: None,
            doctor_collected: false,
            provider_health: Vec::new(),
            tool_status: Vec::new(),
            config_checks: Vec::new(),
            effective_toml: String::new(),
            effective_scroll: 0,
            channels: Vec::new(),
            discovered: Vec::new(),
            health_pending: None,
            health_collected: false,
            probe_started: None,
            health_timed_out: false,
        }
    }

    /// The currently displayed diagnostic mode.
    pub fn mode(&self) -> DiagMode {
        self.mode
    }

    /// Refresh the `/doctor` data. The cheap, local scans (tool gates, config
    /// posture, channel configs) run synchronously here; the two slow live
    /// probes (system `which` checks + provider HTTP health) are started on a
    /// detached worker thread and filled in later by `tick`→[`Self::poll_health`].
    ///
    /// Splitting the work this way is the fix for the `on_enter` freeze: the
    /// surface paints immediately with the local sections (CONFIG / TOOLS /
    /// CHANNELS) and a "probing…" placeholder for SYSTEM / PROVIDERS /
    /// DISCOVERED, instead of blocking the render loop on uncapped HTTP probes.
    fn refresh_doctor(&mut self, app: &App) {
        // Instant, local, non-blocking scans — safe on the UI thread.
        self.tool_status = scan_tool_status();
        self.config_checks = scan_config_health(app);
        // S10: surface the on-disk channel configs (read-only, secret-free).
        self.channels = wcore_channels_registry::scan_user_channels();
        // Slow live probes run off-thread; DISCOVERED (S11) is computed in
        // `poll_health` once the doctor report (its Ollama signal) lands.
        self.start_health_probe();
    }

    /// Start the async system + provider-health probe (idempotent: replacing
    /// any in-flight handle). Marks both results not-yet-collected so the
    /// render shows the "probing…" state until [`Self::poll_health`] applies
    /// each. The prior rows are kept until then, so a re-run (`r`) updates in
    /// place rather than flashing to empty.
    fn start_health_probe(&mut self) {
        self.doctor_collected = false;
        self.health_collected = false;
        self.health_timed_out = false;
        self.probe_started = Some(std::time::Instant::now());
        self.health_pending = Some(spawn_health_probe());
    }

    /// Poll the in-flight health probe; apply any results that have landed.
    /// Returns `true` when something was applied (so the caller knows a
    /// repaint is due). Drains every queued message per call so a fast and a
    /// slow result that arrive together are both picked up. The receiver is
    /// cleared once both probes have resolved (or the thread dropped without a
    /// result), so the surface never hangs on a perpetual "probing…". Called
    /// from `tick` every loop iteration.
    fn poll_health(&mut self) -> bool {
        let Some(rx) = self.health_pending.as_ref() else {
            return false;
        };
        let mut changed = false;
        loop {
            match rx.try_recv() {
                Ok(ProbeMsg::Doctor(report)) => {
                    self.doctor = report;
                    // S11: "here's what I found" — reuse the just-landed Ollama
                    // doctor signal rather than re-probing it.
                    let ollama_available = self
                        .doctor
                        .checks
                        .iter()
                        .any(|c| c.label == "ollama" && c.state == HealthState::Ok);
                    self.discovered = scan_environment(ollama_available);
                    self.doctor_collected = true;
                    changed = true;
                }
                Ok(ProbeMsg::Health(rows)) => {
                    self.provider_health = rows;
                    self.health_collected = true;
                    changed = true;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // The probe thread is gone — stop waiting on anything that
                    // never arrived and render whatever we have.
                    self.doctor_collected = true;
                    self.health_collected = true;
                    changed = true;
                    break;
                }
            }
        }
        // UI-side timeout: a stalled provider-health probe (egress layer
        // holding the connection past its own cap) must not show "probing…"
        // forever. Give up waiting on it and report a timeout. The detached
        // probe thread is left to finish on its own; a re-run (`r`) starts a
        // fresh one.
        if !self.health_collected
            && self
                .probe_started
                .is_some_and(|t| t.elapsed() >= HEALTH_PROBE_UI_TIMEOUT)
        {
            self.health_collected = true;
            self.health_timed_out = true;
            changed = true;
        }
        // Stop polling once both probes have settled.
        if self.doctor_collected && self.health_collected {
            self.health_pending = None;
        }
        changed
    }

    /// S9: render the redacted effective config into `effective_toml` and reset
    /// the scroll. Uses default CLI args — the file-merged config (global ←
    /// project ← profile is not threaded yet; a follow-up). A render failure is
    /// stored as a readable message rather than panicking the read-only screen.
    fn refresh_effective(&mut self) {
        self.effective_toml = match wcore_config::config::effective_config_toml(
            &wcore_config::config::CliArgs::default(),
        ) {
            Ok(toml) => toml,
            Err(e) => format!("could not render the effective config:\n{e:#}"),
        };
        self.effective_scroll = 0;
    }

    /// Re-scan the project long-term memory directory into the `/memory`
    /// report, clamping the selection cursor so it never dangles past the
    /// new (possibly shorter) list.
    fn refresh_memory(&mut self) {
        self.memory = scan_memory();
        let last = self.memory.items.len().saturating_sub(1);
        self.memory_cursor = self.memory_cursor.min(last);
        // A re-scan changes the list under the cursor; never carry a stale
        // delete arm across it.
        self.delete_armed = None;
    }

    /// Move the `/memory` selection cursor by `delta`, clamped to the
    /// list bounds.
    fn move_memory_cursor(&mut self, delta: isize) {
        if self.memory.items.is_empty() {
            return;
        }
        let last = self.memory.items.len() as isize - 1;
        let next = (self.memory_cursor as isize + delta).clamp(0, last);
        self.memory_cursor = next as usize;
        // Moving the selection cancels any pending delete confirmation so
        // the armed `d` can never fire against a different row.
        self.delete_armed = None;
    }

    /// Handle the `/memory` delete key (`d` / `Delete`) with a two-step,
    /// in-place confirmation so a single keystroke can never erase a fact
    /// from disk (D043).
    ///
    /// - First press on a row arms the delete: it records the row index and
    ///   returns `None` (no disk write, no notice). The render path shows a
    ///   "press d again to delete - cannot be undone" warning while armed.
    /// - Second press, while still armed on the **same** row, performs the
    ///   irreversible delete and returns the result notice.
    ///
    /// Any cursor move or other key clears the arm (handled at those call
    /// sites), so the destructive second `d` is always explicit and local.
    fn handle_delete_key(&mut self) -> Option<String> {
        // Nothing selectable - cannot arm or delete.
        if self.memory.items.is_empty() {
            self.delete_armed = None;
            return None;
        }
        match self.delete_armed {
            // Armed on the row still under the cursor: confirm + delete.
            Some(idx) if idx == self.memory_cursor => {
                self.delete_armed = None;
                self.delete_selected_memory()
            }
            // Not armed (or armed on a now-stale row): arm this row only.
            // The first keypress must never delete.
            _ => {
                self.delete_armed = Some(self.memory_cursor);
                None
            }
        }
    }

    /// Delete the currently selected memory entry from disk and re-scan.
    /// Returns a human-readable result line (success or failure) for the
    /// caller to surface as a system notice, or `None` if the list is
    /// empty (nothing to delete).
    ///
    /// This is the unconditional disk-write primitive; the two-step
    /// confirmation gate lives in [`Self::handle_delete_key`].
    fn delete_selected_memory(&mut self) -> Option<String> {
        let item = self.memory.items.get(self.memory_cursor)?;
        let path = item.path.clone();
        let id = item.id.clone();
        let result = match wcore_memory::store::delete_memory(&path) {
            Ok(()) => format!("Deleted memory entry `{id}`"),
            Err(e) => format!("Failed to delete memory entry `{id}`: {e}"),
        };
        self.refresh_memory();
        Some(result)
    }
}

/// Scan the project long-term memory directory into a [`MemoryReport`].
///
/// The directory is `auto_memory_dir(cwd)` — the per-project flat-file
/// memory store. A missing directory (no memory yet) or an unresolvable
/// base dir yields an empty report; failures degrade to empty rather than
/// erroring, since the diagnostics screen is read-only.
fn scan_memory() -> MemoryReport {
    let Ok(cwd) = std::env::current_dir() else {
        return MemoryReport::default();
    };
    let Some(dir) = wcore_memory::paths::auto_memory_dir(&cwd) else {
        return MemoryReport::default();
    };
    let headers = wcore_memory::store::scan_memory_files(&dir).unwrap_or_default();
    MemoryReport {
        items: headers
            .into_iter()
            .map(|h| MemoryItem {
                id: h.filename,
                path: h.file_path,
                category: h
                    .memory_type
                    .map(|t| t.as_str().to_string())
                    .unwrap_or_else(|| "unknown".to_string()),
                summary: h
                    .description
                    .unwrap_or_else(|| "(no description)".to_string()),
            })
            .collect(),
    }
}

impl Surface for DiagnosticsSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::Diagnostics
    }

    /// Refresh both the doctor probe and the memory scan when the surface
    /// becomes active, so re-entering the screen always shows live data.
    fn on_enter(&mut self, app: &mut App) {
        self.refresh_doctor(app);
        self.refresh_memory();
        self.refresh_effective();
    }

    /// Per-tick poll for the async health probe started by `on_enter`/`r`.
    /// Applies the result when it lands so SYSTEM/PROVIDERS/DISCOVERED fill in
    /// without blocking the UI thread. Emits no action.
    fn tick(&mut self, _app: &mut App) -> SurfaceAction {
        self.poll_health();
        SurfaceAction::None
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // A mode-switcher header row, then the active screen's body.
        let [header_area, body_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

        render_mode_switcher(frame, header_area, self.mode, theme);

        match self.mode {
            DiagMode::Doctor => self.render_doctor(frame, body_area, app, theme),
            DiagMode::Cost => self.render_cost(frame, body_area, app, theme),
            DiagMode::Memory => self.render_memory(frame, body_area, theme),
            DiagMode::Effective => self.render_effective(frame, body_area, theme),
        }
    }

    fn handle_key(&mut self, key: KeyEvent, app: &mut App) -> SurfaceAction {
        // Any key other than the delete key disarms a pending delete
        // confirmation, so the destructive second `d` is only ever reachable
        // by pressing `d` twice in a row on the same row (D043). The delete
        // arm/fire decision itself is made in `handle_delete_key`.
        let is_delete_key = matches!(key.code, KeyCode::Char('d') | KeyCode::Delete);
        if !is_delete_key {
            self.delete_armed = None;
        }
        match key.code {
            // Mode switching: number keys + Tab cycle.
            KeyCode::Char('1') => {
                self.mode = DiagMode::Doctor;
                SurfaceAction::None
            }
            KeyCode::Char('2') => {
                self.mode = DiagMode::Cost;
                SurfaceAction::None
            }
            KeyCode::Char('3') => {
                self.mode = DiagMode::Memory;
                SurfaceAction::None
            }
            KeyCode::Char('4') => {
                self.mode = DiagMode::Effective;
                SurfaceAction::None
            }
            // `/effective` scroll — the redacted TOML can exceed the viewport.
            KeyCode::Up | KeyCode::Char('k') if self.mode == DiagMode::Effective => {
                self.effective_scroll = self.effective_scroll.saturating_sub(1);
                SurfaceAction::None
            }
            KeyCode::Down | KeyCode::Char('j') if self.mode == DiagMode::Effective => {
                self.effective_scroll = self.effective_scroll.saturating_add(1);
                SurfaceAction::None
            }
            KeyCode::PageUp if self.mode == DiagMode::Effective => {
                self.effective_scroll = self.effective_scroll.saturating_sub(10);
                SurfaceAction::None
            }
            KeyCode::PageDown if self.mode == DiagMode::Effective => {
                self.effective_scroll = self.effective_scroll.saturating_add(10);
                SurfaceAction::None
            }
            KeyCode::Tab => {
                let idx = DiagMode::ALL
                    .iter()
                    .position(|&m| m == self.mode)
                    .unwrap_or(0);
                self.mode = DiagMode::ALL[(idx + 1) % DiagMode::ALL.len()];
                SurfaceAction::None
            }
            KeyCode::BackTab => {
                let idx = DiagMode::ALL
                    .iter()
                    .position(|&m| m == self.mode)
                    .unwrap_or(0);
                let len = DiagMode::ALL.len();
                self.mode = DiagMode::ALL[(idx + len - 1) % len];
                SurfaceAction::None
            }
            // `/doctor` re-run — re-run the live probe in place.
            KeyCode::Char('r') if self.mode == DiagMode::Doctor => {
                self.refresh_doctor(app);
                SurfaceAction::None
            }
            // `/memory` list navigation.
            KeyCode::Up | KeyCode::Char('k') if self.mode == DiagMode::Memory => {
                self.move_memory_cursor(-1);
                SurfaceAction::None
            }
            KeyCode::Down | KeyCode::Char('j') if self.mode == DiagMode::Memory => {
                self.move_memory_cursor(1);
                SurfaceAction::None
            }
            // `/memory` delete — two-step confirmation. The first `d` arms
            // the delete and shows a warning in the render; only a second
            // `d` on the same row writes to disk + surfaces a notice (D043).
            KeyCode::Char('d') | KeyCode::Delete if self.mode == DiagMode::Memory => {
                if let Some(notice) = self.handle_delete_key() {
                    app.session.turns.push(crate::tui::app::TurnView {
                        role: crate::tui::app::TurnRole::System,
                        elements: vec![crate::tui::turn_element::TurnElement::Markdown(notice)],
                    });
                }
                SurfaceAction::None
            }
            _ => SurfaceAction::None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Rendering helpers — one per screen
// ─────────────────────────────────────────────────────────────────────────

/// Render the three-tab mode switcher header.
fn render_mode_switcher(frame: &mut Frame, area: Rect, active: DiagMode, t: &Theme) {
    let mut spans: Vec<Span> = Vec::new();
    for (i, mode) in DiagMode::ALL.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("  ", Style::default().bg(t.surface)));
        }
        let style = if *mode == active {
            Style::default()
                .bg(t.surface)
                .fg(t.orange)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().bg(t.surface).fg(t.text_muted)
        };
        spans.push(Span::styled(format!(" {} ", mode.label()), style));
    }
    let bar = Paragraph::new(Line::from(spans)).style(Style::default().bg(t.surface));
    frame.render_widget(bar, area);
}

/// Render a "no data yet" empty-state line into `area`.
fn render_empty(frame: &mut Frame, area: Rect, t: &Theme, msg: &str) {
    let para = Paragraph::new(Line::from(Span::styled(
        msg.to_string(),
        Style::default().fg(t.text_muted),
    )))
    .wrap(Wrap { trim: true });
    frame.render_widget(para, area);
}

/// Push a dim, bold section heading line onto `lines`. Used by the
/// extended `/doctor` view to delimit its five sections.
fn push_section_header(lines: &mut Vec<Line<'static>>, t: &Theme, label: &str) {
    lines.push(Line::from(Span::styled(
        label.to_string(),
        Style::default()
            .fg(t.text_muted)
            .add_modifier(Modifier::BOLD),
    )));
}

/// Build one tri-state status row: glyph + colored label + dim detail.
/// Shared between the system, provider, and tool sections of `/doctor`
/// so every row paints with the same shape and palette.
fn status_row(state: HealthState, label: &str, detail: &str, t: &Theme) -> Line<'static> {
    let color = state.color(t);
    Line::from(vec![
        Span::styled(
            format!(" {} ", state.glyph()),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("{:<22}", label), Style::default().fg(t.text)),
        Span::styled(detail.to_string(), Style::default().fg(t.text_dim)),
    ])
}

impl DiagnosticsSurface {
    /// Render the `/doctor` screen — five stacked sections inside one
    /// panel:
    ///   1. System dependency rows (from `doctor::collect`)
    ///   2. Provider health rows (Anthropic / OpenAI / Gemini / Groq)
    ///   3. Per-tool backend status (from `TOOL_GATES` + env probe)
    ///   4. Recent engine errors (last 10 system error turns)
    ///   5. Token budget for the active model
    fn render_doctor(&self, frame: &mut Frame, area: Rect, app: &App, t: &Theme) {
        let block = panel(
            " /doctor — system · providers · config · tools · errors · tokens ",
            t,
        );
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height == 0 {
            return;
        }

        // The two live probes (SYSTEM + PROVIDERS) run off-thread and resolve
        // independently; until each lands its section (and DISCOVERED, derived
        // from SYSTEM) shows a "probing…" line. The local sections (CONFIG /
        // TOOLS / CHANNELS) always render immediately — the whole point of not
        // blocking `on_enter`.
        let system_probing = self.health_pending.is_some() && !self.doctor_collected;
        let providers_probing = self.health_pending.is_some() && !self.health_collected;
        let probing_line = || {
            Line::from(Span::styled(
                "  probing…",
                Style::default().fg(t.text_muted),
            ))
        };

        let mut lines: Vec<Line> = Vec::new();

        // ── 1. System dependency rows ───────────────────────────────
        push_section_header(&mut lines, t, "SYSTEM");
        if self.doctor.checks.is_empty() && system_probing {
            lines.push(probing_line());
        } else {
            for check in &self.doctor.checks {
                lines.push(status_row(check.state, &check.label, &check.detail, t));
            }
        }

        // ── 2. Provider health rows ─────────────────────────────────
        lines.push(Line::from(""));
        push_section_header(&mut lines, t, "PROVIDERS");
        if self.provider_health.is_empty() {
            lines.push(if providers_probing {
                probing_line()
            } else if self.health_timed_out {
                Line::from(Span::styled(
                    "  health probe timed out — check network / egress policy",
                    Style::default().fg(t.warning),
                ))
            } else {
                Line::from(Span::styled(
                    "  no provider probes yet",
                    Style::default().fg(t.text_muted),
                ))
            });
        } else {
            for ph in &self.provider_health {
                lines.push(status_row(
                    health_state_from_status(ph.status),
                    &ph.name,
                    &ph.detail,
                    t,
                ));
            }
        }

        // ── 3. Config posture rows (S8) ─────────────────────────────
        // Coherence/safety checks on the resolved config snapshot — the
        // answer to "is my configuration sane", distinct from the runtime
        // probes above.
        lines.push(Line::from(""));
        push_section_header(&mut lines, t, "CONFIG");
        for check in &self.config_checks {
            lines.push(status_row(check.state, &check.label, &check.detail, t));
        }

        // ── 4. Per-tool backend status ──────────────────────────────
        lines.push(Line::from(""));
        push_section_header(&mut lines, t, "TOOLS");
        for row in &self.tool_status {
            let (state, label) = if row.enabled {
                (HealthState::Ok, "yes")
            } else {
                (HealthState::Warn, "no")
            };
            let detail = if row.enabled {
                format!("{label} · env {}", row.env_var)
            } else {
                format!("{label} · set {} to enable", row.env_var)
            };
            lines.push(status_row(state, &row.name, &detail, t));
        }

        // ── 5. MCP servers ──────────────────────────────────────────
        // Seeded at boot from the connect-health snapshot (tui::run) and
        // updated live by McpReady / McpFailed events. A failed or timed-out
        // server is the answer to "why aren't my plugin's tools showing up".
        lines.push(Line::from(""));
        push_section_header(&mut lines, t, "MCP SERVERS");
        if app.mcp_status.is_empty() {
            lines.push(Line::from(Span::styled(
                "  none configured",
                Style::default().fg(t.text_muted),
            )));
        } else {
            let mut names: Vec<&String> = app.mcp_status.keys().collect();
            names.sort();
            for name in names {
                let (state, detail) = match &app.mcp_status[name] {
                    crate::tui::app::McpServerStatus::Ready { tool_count } => {
                        (HealthState::Ok, format!("ready · {tool_count} tools"))
                    }
                    crate::tui::app::McpServerStatus::Failed { reason } => {
                        (HealthState::Fail, format!("failed · {reason}"))
                    }
                    crate::tui::app::McpServerStatus::TimedOut => {
                        (HealthState::Fail, "timed out at connect".to_string())
                    }
                    crate::tui::app::McpServerStatus::Skipped { reason } => {
                        (HealthState::Warn, format!("⊘ skipped · {reason}"))
                    }
                };
                lines.push(status_row(state, name, &detail, t));
            }
        }

        // ── 6. Channels / integrations (S10) ────────────────────────
        // The on-disk channel configs (`~/.wayland/channels/*.toml`) are
        // otherwise invisible to the schema-driven `/config` TUI — this is the
        // "ghost subsystem" made visible. Secret-free: only key names show.
        lines.push(Line::from(""));
        push_section_header(&mut lines, t, "CHANNELS");
        if self.channels.is_empty() {
            lines.push(Line::from(Span::styled(
                "  none configured",
                Style::default().fg(t.text_muted),
            )));
        } else {
            for ch in &self.channels {
                let (state, detail) = if let Some(err) = &ch.parse_error {
                    (HealthState::Fail, format!("parse error · {err}"))
                } else if !ch.known_platform {
                    (
                        HealthState::Fail,
                        format!("unknown platform `{}` · won't load", ch.platform),
                    )
                } else if !ch.enabled {
                    (HealthState::Warn, format!("{} · disabled", ch.platform))
                } else {
                    let opts = if ch.option_keys.is_empty() {
                        String::new()
                    } else {
                        format!(" · opts: {}", ch.option_keys.join(", "))
                    };
                    let secrets = if ch.secret_keys.is_empty() {
                        String::new()
                    } else {
                        format!(" · secrets: {}", ch.secret_keys.join(", "))
                    };
                    (
                        HealthState::Ok,
                        format!("{} · enabled{opts}{secrets}", ch.platform),
                    )
                };
                lines.push(status_row(state, &ch.name, &detail, t));
            }
        }

        // ── 7. Discovered capabilities (S11) ────────────────────────
        // "Here's what I found" — latent capabilities on this machine the user
        // could wire up. A found capability paints green; an absent one is a
        // dim line with the how-to hint (absence is not a problem, so no Warn).
        lines.push(Line::from(""));
        push_section_header(&mut lines, t, "DISCOVERED");
        if self.discovered.is_empty() && system_probing {
            lines.push(probing_line());
        }
        for d in &self.discovered {
            let (glyph, glyph_color, label_color) = if d.available {
                ("● ", t.success, t.text)
            } else {
                ("○ ", t.text_dim, t.text_dim)
            };
            lines.push(Line::from(vec![
                Span::styled(glyph.to_string(), Style::default().fg(glyph_color)),
                Span::styled(format!("{:<22}", d.label), Style::default().fg(label_color)),
                Span::styled(d.detail.clone(), Style::default().fg(t.text_muted)),
            ]));
        }

        // ── 8. Recent engine errors ─────────────────────────────────
        lines.push(Line::from(""));
        push_section_header(&mut lines, t, "RECENT ERRORS");
        let errors = collect_recent_errors(app);
        if errors.is_empty() {
            lines.push(Line::from(Span::styled(
                "  none — clean session",
                Style::default().fg(t.text_muted),
            )));
        } else {
            for err in &errors {
                // The protocol bridge prefixes each line; we just show
                // the message as-is so the user sees what the engine
                // surfaced.
                lines.push(Line::from(vec![
                    Span::styled(" ✕ ", Style::default().fg(t.error)),
                    Span::styled(err.message.clone(), Style::default().fg(t.text_dim)),
                ]));
            }
        }

        // ── 9. Token budget ─────────────────────────────────────────
        lines.push(Line::from(""));
        push_section_header(&mut lines, t, "TOKEN BUDGET");
        let budget = token_budget_view(app);
        if budget.window_size == 0 && budget.used_tokens == 0 && budget.last_turn_output == 0 {
            lines.push(Line::from(Span::styled(
                "  no usage recorded yet",
                Style::default().fg(t.text_muted),
            )));
        } else {
            let model_label = if budget.model.is_empty() {
                "(unknown)".to_string()
            } else {
                budget.model.clone()
            };
            lines.push(Line::from(vec![
                Span::styled("  model     ", Style::default().fg(t.text_dim)),
                Span::styled(model_label, Style::default().fg(t.text)),
            ]));
            let remaining = budget.window_size.saturating_sub(budget.used_tokens);
            lines.push(Line::from(vec![
                Span::styled("  context   ", Style::default().fg(t.text_dim)),
                Span::styled(
                    format!(
                        "{} / {} ({} remaining)",
                        budget.used_tokens, budget.window_size, remaining
                    ),
                    Style::default().fg(t.text),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("  last turn ", Style::default().fg(t.text_dim)),
                Span::styled(
                    format!("↑ {} output tokens", budget.last_turn_output),
                    Style::default().fg(t.text),
                ),
            ]));
        }

        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  r re-run · 1 doctor · 2 cost · 3 memory · esc workspace",
            Style::default().fg(t.text_muted),
        )));

        let para = Paragraph::new(lines)
            .style(Style::default().bg(t.surface))
            .wrap(Wrap { trim: true });
        frame.render_widget(para, inner);
    }

    /// Render the `/cost` screen: total spend + a per-turn breakdown.
    ///
    /// Reads [`App::cost`] live, so a `SessionCost` event that arrives
    /// while this screen is open shows on the next frame.
    fn render_cost(&self, frame: &mut Frame, area: Rect, app: &App, t: &Theme) {
        let block = panel(" /cost — session token usage · spend ", t);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height == 0 {
            return;
        }

        // v0.9.1.1 H4: previously this gated on `!c.per_turn.is_empty()`,
        // which left the diagnostics screen showing "No cost recorded
        // yet" while the status bar (which reads only `total_cost_usd`)
        // showed the real session spend. Disagreement arose whenever
        // `total_cost_usd > 0` but `per_turn = []` — most commonly a
        // session that emitted `SessionCost` without any per-turn rows
        // pushed (e.g. an Esc-cancelled first turn). Treat any
        // `Some(c)` as a recorded cost; an empty per-turn breakdown is
        // rendered as the total alone, with a "(no per-turn rows
        // available)" footnote so the absence is explicit. Both
        // surfaces now share a single source of truth: `app.cost`.
        let cost = match app.cost.as_ref() {
            Some(c) => c,
            None => {
                render_empty(
                    frame,
                    inner,
                    t,
                    "No cost recorded yet — spend appears here once a session runs.",
                );
                return;
            }
        };

        let mut lines: Vec<Line> = Vec::new();

        // The session aggregate, accented.
        lines.push(Line::from(vec![
            Span::styled("  session  ", Style::default().fg(t.text_dim)),
            Span::styled(cost.session_id.clone(), Style::default().fg(t.text)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  total    ", Style::default().fg(t.text_dim)),
            Span::styled(
                format!("${:.4}", cost.total_cost_usd),
                Style::default().fg(t.orange).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!("   ({} turns)", cost.per_turn.len()),
                Style::default().fg(t.text_muted),
            ),
        ]));
        lines.push(Line::from(""));

        // Per-turn rows.
        if cost.per_turn.is_empty() {
            // v0.9.1.1 H4 — `total > 0` with no per-turn breakdown is a
            // valid state (the engine emitted `SessionCost` from a path
            // where no `per_turn_costs.push` ran, e.g. a cancelled first
            // turn). Show the total alone with a footnote so the
            // absence is explicit, rather than the previous "No cost
            // recorded yet" lie.
            lines.push(Line::from(Span::styled(
                "  (no per-turn breakdown available for this session)",
                Style::default().fg(t.text_muted),
            )));
        } else {
            for row in &cost.per_turn {
                lines.push(Line::from(vec![
                    Span::styled(
                        format!("  turn {:>3}  ", row.turn),
                        Style::default().fg(t.text_muted),
                    ),
                    Span::styled(
                        format!("{:<24}", format!("{} · {}", row.provider, row.model)),
                        Style::default().fg(t.text_dim),
                    ),
                    Span::styled(format!("${:.4}", row.cost_usd), Style::default().fg(t.text)),
                ]));
            }
        }

        let para = Paragraph::new(lines)
            .style(Style::default().bg(t.surface))
            .wrap(Wrap { trim: true });
        frame.render_widget(para, inner);
    }

    /// Render the `/memory` screen: the stored entries + a delete
    /// affordance on the selected row.
    fn render_memory(&self, frame: &mut Frame, area: Rect, t: &Theme) {
        let block = panel(" /memory — long-term memory contents ", t);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height == 0 {
            return;
        }

        if self.memory.items.is_empty() {
            render_empty(
                frame,
                inner,
                t,
                "Long-term memory is empty — entries appear here as they accumulate.",
            );
            return;
        }

        let mut lines: Vec<Line> = Vec::new();
        for (i, item) in self.memory.items.iter().enumerate() {
            let selected = i == self.memory_cursor;
            let marker = if selected { "›" } else { " " };
            let row_style = if selected {
                Style::default().bg(t.surface_hover)
            } else {
                Style::default().bg(t.surface)
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {marker} "), row_style.fg(t.orange)),
                Span::styled(
                    format!("[{:<9}] ", item.category),
                    row_style.fg(t.text_muted),
                ),
                Span::styled(item.summary.clone(), row_style.fg(t.text)),
            ]));
        }

        lines.push(Line::from(""));

        // When a delete is armed on the selected row, the footer becomes a
        // loud confirmation prompt instead of the normal hint. The first `d`
        // only arms; this warning makes the destructive second `d` explicit
        // (D043).
        if self.delete_armed == Some(self.memory_cursor) {
            let armed_id = self
                .memory
                .items
                .get(self.memory_cursor)
                .map(|item| item.id.as_str())
                .unwrap_or("this entry");
            lines.push(Line::from(Span::styled(
                format!("  press d again to delete {armed_id} · cannot be undone"),
                Style::default().fg(t.error).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(Span::styled(
                "  any other key cancels",
                Style::default().fg(t.text_muted),
            )));
        } else {
            lines.push(Line::from(Span::styled(
                "  ↑/↓ select · d delete · 1 doctor · 2 cost · 3 memory · esc workspace",
                Style::default().fg(t.text_muted),
            )));
        }

        let para = Paragraph::new(lines).style(Style::default().bg(t.surface));
        frame.render_widget(para, inner);
    }

    /// Render the `/effective` screen (S9): the redacted, merged effective
    /// config as scrollable TOML, with a header noting what is and isn't shown.
    fn render_effective(&self, frame: &mut Frame, area: Rect, t: &Theme) {
        let block = panel(" /effective — resolved config · redacted ", t);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height < 3 || inner.width < 10 {
            return;
        }
        let [note_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "Merged from global ← project config files. Secrets shown as ***.",
                    Style::default().fg(t.text_dim),
                )),
                Line::from(Span::styled(
                    "Live env-resolved API keys and session CLI flags are not shown.",
                    Style::default().fg(t.text_muted),
                )),
            ]),
            note_area,
        );

        // Clamp the scroll so the view can never run past the end into a blank
        // screen (the held offset may exceed the content; the display clamps).
        let total_lines = self.effective_toml.lines().count() as u16;
        let scroll = self.effective_scroll.min(total_lines.saturating_sub(1));
        frame.render_widget(
            Paragraph::new(self.effective_toml.clone())
                .style(Style::default().fg(t.text))
                .scroll((scroll, 0)),
            body_area,
        );

        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  ↑↓ scroll · 1 doctor · 2 cost · 3 memory · 4 effective · esc workspace",
                Style::default().fg(t.text_muted),
            ))),
            footer_area,
        );
    }
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::tui::app::App;
    use crate::tui::theme::Theme;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Render `surface` into an 80×24 `TestBackend` and return the buffer
    /// as a single flattened string for substring assertions.
    fn render_to_string(surface: &mut DiagnosticsSurface) -> String {
        let app = App::new();
        let theme = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), &app, &theme))
            .expect("render diagnostics");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..24 {
            for x in 0..80 {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// Drive the async health probe (started by `on_enter`/`r`) to full
    /// settlement by polling `tick`, the way the live render loop does. Both
    /// the system scan and the provider-health rows must land (the receiver is
    /// cleared only when both do). Bounded so a hung/slow probe fails the test
    /// rather than spinning forever. Returns whether it settled in budget.
    fn drive_health_probe(s: &mut DiagnosticsSurface) -> bool {
        let mut app = App::new();
        for _ in 0..400 {
            s.tick(&mut app);
            if s.health_pending.is_none() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        s.health_pending.is_none()
    }

    // -- OSC-9 / bell helpers ----------------------------------------------

    #[test]
    fn osc9_notification_wraps_message_in_the_control_string() {
        let s = osc9_notification(NotifyKind::ApprovalNeeded);
        // ESC ] 9 ; <text> BEL — exact framing.
        assert!(s.starts_with("\x1b]9;"), "missing OSC-9 prefix: {s:?}");
        assert!(s.ends_with('\x07'), "missing BEL terminator: {s:?}");
        assert!(
            s.contains("approval needed"),
            "approval copy missing: {s:?}"
        );
    }

    #[test]
    fn osc9_notification_copy_differs_per_kind() {
        let approval = osc9_notification(NotifyKind::ApprovalNeeded);
        let finished = osc9_notification(NotifyKind::TaskFinished);
        assert_ne!(approval, finished);
        assert!(finished.contains("task finished"));
    }

    #[test]
    fn terminal_bell_is_the_bel_byte() {
        assert_eq!(terminal_bell(), "\x07");
    }

    #[test]
    fn maybe_notify_is_silent_when_terminal_is_focused() {
        // A focused user is already looking at the TUI — no interrupt.
        assert!(maybe_notify(NotifyKind::ApprovalNeeded, true).is_none());
        assert!(maybe_notify(NotifyKind::TaskFinished, true).is_none());
    }

    #[test]
    fn maybe_notify_emits_osc9_plus_bell_when_unfocused() {
        let signal = maybe_notify(NotifyKind::TaskFinished, false)
            .expect("unfocused terminal should get a notification");
        // The OSC-9 sequence first, then the standalone bell.
        assert!(signal.starts_with("\x1b]9;"), "OSC-9 missing: {signal:?}");
        assert!(signal.contains("task finished"));
        // Two BEL bytes total: the OSC-9 terminator + the trailing bell.
        assert_eq!(signal.matches('\x07').count(), 2, "bell pairing wrong");
    }

    // -- mode switching ----------------------------------------------------

    #[test]
    fn surface_starts_on_doctor() {
        let s = DiagnosticsSurface::new();
        assert_eq!(s.mode(), DiagMode::Doctor);
        assert_eq!(s.id(), SurfaceId::Diagnostics);
    }

    #[test]
    fn number_keys_switch_modes() {
        let mut s = DiagnosticsSurface::new();
        let mut app = App::new();
        s.handle_key(key(KeyCode::Char('2')), &mut app);
        assert_eq!(s.mode(), DiagMode::Cost);
        s.handle_key(key(KeyCode::Char('3')), &mut app);
        assert_eq!(s.mode(), DiagMode::Memory);
        s.handle_key(key(KeyCode::Char('1')), &mut app);
        assert_eq!(s.mode(), DiagMode::Doctor);
    }

    #[test]
    fn tab_cycles_through_the_modes() {
        let mut s = DiagnosticsSurface::new();
        let mut app = App::new();
        s.handle_key(key(KeyCode::Tab), &mut app);
        assert_eq!(s.mode(), DiagMode::Cost);
        s.handle_key(key(KeyCode::Tab), &mut app);
        assert_eq!(s.mode(), DiagMode::Memory);
        s.handle_key(key(KeyCode::Tab), &mut app);
        assert_eq!(s.mode(), DiagMode::Effective);
        s.handle_key(key(KeyCode::Tab), &mut app);
        // Wraps back to the first mode.
        assert_eq!(s.mode(), DiagMode::Doctor);
        s.handle_key(key(KeyCode::BackTab), &mut app);
        assert_eq!(s.mode(), DiagMode::Effective);
    }

    #[test]
    fn effective_mode_renders_redaction_note_and_scrolls() {
        // S9: the `4` key selects the Effective tab; the redacted-config
        // preview renders with its caveat header and the scroll keys move the
        // offset. The redaction note is static UI text, so this assertion is
        // hermetic regardless of the box's real config contents.
        let mut s = DiagnosticsSurface::new();
        let mut app = App::new();
        s.on_enter(&mut app);
        s.handle_key(key(KeyCode::Char('4')), &mut app);
        assert_eq!(s.mode(), DiagMode::Effective);
        let out = render_to_string(&mut s);
        assert!(out.contains("/effective"), "tab label missing:\n{out}");
        assert!(
            out.contains("Secrets shown as"),
            "redaction note missing:\n{out}"
        );
        // Scroll down then up returns to the start; saturating at 0.
        let before = s.effective_scroll;
        s.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(s.effective_scroll, before + 1);
        s.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(s.effective_scroll, before);
        s.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(s.effective_scroll, 0, "scroll must saturate at the top");
    }

    // -- /doctor screen ----------------------------------------------------

    #[test]
    fn health_check_from_maps_outcomes_to_states() {
        use crate::doctor::{CheckResult, Outcome};
        // Pass → Ok.
        let pass = health_check_from(&CheckResult {
            label: "binary version",
            outcome: Outcome::Pass {
                detail: "v0.6.1".into(),
            },
        });
        assert_eq!(pass.state, HealthState::Ok);
        assert_eq!(pass.detail, "v0.6.1");

        // Fail → Fail, detail carries the first hint.
        let fail = health_check_from(&CheckResult {
            label: "chromium browser",
            outcome: Outcome::Fail {
                hints: vec!["apt install chromium-browser".into()],
            },
        });
        assert_eq!(fail.state, HealthState::Fail);
        assert!(fail.detail.contains("apt install chromium-browser"));

        // Warn / Skip / Manual all degrade to the non-fatal Warn state.
        for outcome in [
            Outcome::Warn {
                detail: "not set".into(),
                hints: vec![],
            },
            Outcome::Skip {
                reason: "Linux-only".into(),
            },
            Outcome::Manual {
                hint: "check System Settings".into(),
            },
        ] {
            let hc = health_check_from(&CheckResult {
                label: "x",
                outcome,
            });
            assert_eq!(hc.state, HealthState::Warn);
        }
    }

    #[test]
    fn doctor_renders_live_probe_rows_after_on_enter() {
        // `on_enter` starts the real `doctor::collect` probe off-thread; the
        // render loop's `tick` applies it once it lands. Every doctor run
        // includes the structural `binary version` row, so the collected
        // report must show it — and no placeholder banner.
        let mut s = DiagnosticsSurface::new();
        s.on_enter(&mut App::new());
        assert!(
            drive_health_probe(&mut s),
            "the async probe must collect the doctor report"
        );
        assert!(
            !s.doctor.checks.is_empty(),
            "live doctor report must have rows"
        );
        let out = render_to_string(&mut s);
        assert!(out.contains("/doctor"), "doctor header missing");
        assert!(out.contains("binary version"), "version check row missing");
        assert!(
            !out.contains("Wave 2") && !out.contains("representative report"),
            "live report must not show the placeholder banner"
        );
    }

    #[test]
    fn doctor_on_enter_does_not_block_and_shows_probing() {
        // The freeze fix: `on_enter` must NOT block on the live probes. It
        // returns immediately with a probe in flight, and the first render
        // shows the local sections plus a "probing…" placeholder for the
        // async ones — never a frozen/blank screen.
        let mut s = DiagnosticsSurface::new();
        s.on_enter(&mut App::new());
        assert!(
            s.health_pending.is_some(),
            "on_enter must start the probe without blocking"
        );
        assert!(
            !s.doctor_collected,
            "the report is still in flight right after on_enter"
        );
        let out = render_to_string(&mut s);
        // Local sections render instantly; the async sections show probing.
        assert!(out.contains("CONFIG"), "local CONFIG section must render");
        assert!(
            out.contains("probing"),
            "async sections must show a probing state"
        );
    }

    #[test]
    fn doctor_re_run_key_refreshes_in_place() {
        let mut s = DiagnosticsSurface::new();
        let mut app = App::new();
        // `r` starts the probe directly and consumes the key (no command).
        match s.handle_key(key(KeyCode::Char('r')), &mut app) {
            SurfaceAction::None => {}
            other => panic!("expected re-run to be inert, got {other:?}"),
        }
        assert!(
            s.health_pending.is_some(),
            "the `r` key must start the probe"
        );
        assert!(drive_health_probe(&mut s), "the `r` probe must resolve");
        assert!(!s.doctor.checks.is_empty(), "re-run must populate rows");
    }

    #[test]
    fn provider_health_probe_times_out_in_the_ui_after_the_cap() {
        // A stalled provider-health probe (egress layer holding the connection
        // past its own cap) must not show "probing…" forever. Once the UI
        // timeout elapses the surface gives up waiting and renders an honest
        // "timed out" line. Drive it with a never-resolving channel and a
        // start time pushed past the cap (no real network, fully hermetic).
        let mut s = DiagnosticsSurface::new();
        let (_tx, rx) = std::sync::mpsc::channel::<ProbeMsg>();
        s.health_pending = Some(rx); // _tx kept alive: not Disconnected, just Empty
        s.doctor_collected = true; // pretend the fast system scan already landed
        s.health_collected = false;
        s.probe_started = Some(
            std::time::Instant::now()
                - (HEALTH_PROBE_UI_TIMEOUT + std::time::Duration::from_secs(1)),
        );

        let mut app = App::new();
        s.tick(&mut app);

        assert!(s.health_timed_out, "the probe must be marked timed out");
        assert!(
            s.health_pending.is_none(),
            "a timed-out probe must stop being polled"
        );
        let out = render_to_string(&mut s);
        assert!(
            out.contains("timed out"),
            "PROVIDERS must render the timeout copy:\n{out}"
        );
    }

    // -- /cost screen ------------------------------------------------------

    #[test]
    fn cost_shows_an_empty_state_with_no_data() {
        let mut s = DiagnosticsSurface::new();
        s.handle_key(key(KeyCode::Char('2')), &mut App::new());
        let out = render_to_string(&mut s);
        assert!(out.contains("/cost"), "cost header missing");
        assert!(
            out.contains("No cost recorded yet"),
            "empty-state copy missing"
        );
    }

    #[test]
    fn cost_status_bar_and_doctor_view_agree_v0911() {
        // v0.9.1.1 H4 regression — the bug-hunter observed the status
        // bar at `$0.0178` while `/cost` showed "No cost recorded yet"
        // in the same session. Disagreement arose because `/cost`
        // gated on `!per_turn.is_empty()` while the status bar reads
        // only `total_cost_usd`. Both surfaces must now agree: when
        // `app.cost` is `Some(c)`, both render `c.total_cost_usd`.
        use crate::tui::app::SessionCostView;
        let mut s = DiagnosticsSurface::new();
        s.handle_key(key(KeyCode::Char('2')), &mut App::new());
        let mut app = App::new();
        // The exact bug-hunter state: total > 0, per_turn empty (a
        // cancelled first turn that never pushed a `per_turn_costs`
        // row but where `fire_on_session_end` still emitted
        // `SessionCost`).
        app.cost = Some(SessionCostView {
            session_id: "h4-regression".into(),
            total_cost_usd: 0.0178,
            per_turn: vec![],
        });
        let out = render_with_app(&mut s, &app);
        // The total appears verbatim — same field the status bar uses.
        assert!(
            out.contains("$0.0178"),
            "total spend missing — /cost disagrees with status bar: {out}"
        );
        // The misleading empty-state copy is gone.
        assert!(
            !out.contains("No cost recorded yet"),
            "stale empty-state copy fired on a populated cost view: {out}"
        );
        // The absence of per-turn rows is acknowledged explicitly.
        assert!(
            out.contains("no per-turn breakdown"),
            "missing footnote for empty per_turn: {out}"
        );
    }

    #[test]
    fn cost_renders_total_and_per_turn_rows_from_app_cost() {
        use crate::tui::app::{SessionCostView, TurnCostView};
        let mut s = DiagnosticsSurface::new();
        s.handle_key(key(KeyCode::Char('2')), &mut App::new());

        // The `/cost` screen reads `App::cost` live.
        let mut app = App::new();
        app.cost = Some(SessionCostView {
            session_id: "a3f8c2".into(),
            total_cost_usd: 0.0731,
            per_turn: vec![
                TurnCostView {
                    turn: 1,
                    model: "claude-sonnet-4-6".into(),
                    provider: "anthropic".into(),
                    cost_usd: 0.0412,
                },
                TurnCostView {
                    turn: 2,
                    model: "claude-sonnet-4-6".into(),
                    provider: "anthropic".into(),
                    cost_usd: 0.0319,
                },
            ],
        });
        let out = render_with_app(&mut s, &app);
        assert!(out.contains("a3f8c2"), "session id missing");
        assert!(out.contains("$0.0731"), "total spend missing");
        assert!(out.contains("turn   1"), "turn 1 row missing");
        assert!(out.contains("turn   2"), "turn 2 row missing");
        assert!(out.contains("anthropic"), "provider missing");
    }

    // -- /memory screen ----------------------------------------------------

    #[test]
    fn memory_shows_an_empty_state_with_no_entries() {
        let mut s = DiagnosticsSurface::new();
        s.handle_key(key(KeyCode::Char('3')), &mut App::new());
        let out = render_to_string(&mut s);
        assert!(out.contains("/memory"), "memory header missing");
        assert!(
            out.contains("Long-term memory is empty"),
            "empty-state copy missing"
        );
    }

    #[test]
    fn memory_lists_entries_and_marks_the_selection() {
        let mut s = sample_memory_surface();
        s.handle_key(key(KeyCode::Char('3')), &mut App::new());
        let out = render_to_string(&mut s);
        assert!(out.contains("user role"), "first entry missing");
        assert!(out.contains("project context"), "second entry missing");
        // The selection marker sits on the first row by default.
        assert!(out.contains("› [user"), "selection marker missing");
    }

    #[test]
    fn memory_cursor_moves_within_bounds() {
        let mut s = sample_memory_surface();
        s.handle_key(key(KeyCode::Char('3')), &mut App::new());
        // Down moves to the second entry.
        s.handle_key(key(KeyCode::Down), &mut App::new());
        assert_eq!(s.memory_cursor, 1);
        // Down again is clamped at the last entry (two items total).
        s.handle_key(key(KeyCode::Down), &mut App::new());
        assert_eq!(s.memory_cursor, 1);
        // Up returns to the first; Up again is clamped at 0.
        s.handle_key(key(KeyCode::Up), &mut App::new());
        assert_eq!(s.memory_cursor, 0);
        s.handle_key(key(KeyCode::Up), &mut App::new());
        assert_eq!(s.memory_cursor, 0);
    }

    #[test]
    fn memory_delete_removes_the_file_and_pushes_a_notice() {
        // `delete_selected_memory` performs a real `delete_memory` on the
        // entry's on-disk path. Back it with a tempfile so the delete is
        // observable.
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("user_role.md");
        std::fs::write(&path, "---\ntype: user\n---\nbody").expect("seed file");
        assert!(path.exists());

        let mut s = DiagnosticsSurface::new();
        s.memory = MemoryReport {
            items: vec![MemoryItem {
                id: "user_role.md".into(),
                path: path.clone(),
                category: "user".into(),
                summary: "user role".into(),
            }],
        };
        s.handle_key(key(KeyCode::Char('3')), &mut App::new());

        // First `d` only arms — the file must still exist and no notice
        // fires. The destructive write requires a second confirmation.
        let mut app = App::new();
        let armed = s.handle_key(key(KeyCode::Char('d')), &mut app);
        assert!(matches!(armed, SurfaceAction::None));
        assert!(path.exists(), "first d must NOT delete (arm only)");
        assert!(
            app.session.turns.is_empty(),
            "first d must not push a notice"
        );

        // Second `d` on the same row performs the irreversible delete.
        let action = s.handle_key(key(KeyCode::Char('d')), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        // The file is gone and a system notice was pushed.
        assert!(!path.exists(), "second d must remove the file from disk");
        assert_eq!(app.session.turns.len(), 1);
        assert!(
            app.session.turns[0].text().contains("Deleted memory entry"),
            "delete must surface a system notice"
        );
    }

    #[test]
    fn memory_delete_is_inert_when_the_list_is_empty() {
        let mut s = DiagnosticsSurface::new();
        s.handle_key(key(KeyCode::Char('3')), &mut App::new());
        let mut app = App::new();
        match s.handle_key(key(KeyCode::Char('d')), &mut app) {
            SurfaceAction::None => {}
            _ => panic!("empty-list delete must be inert (expected SurfaceAction::None)"),
        }
        assert!(
            app.session.turns.is_empty(),
            "empty-list delete must not push a notice"
        );
    }

    // -- D043: two-step delete confirmation (render-asserted) --------------

    /// Build a tempdir-backed surface on the `/memory` screen whose single
    /// entry points at a real on-disk file, so a delete is observable. The
    /// `tempfile::TempDir` is returned so the caller keeps it alive.
    fn armed_delete_surface() -> (DiagnosticsSurface, std::path::PathBuf, tempfile::TempDir) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("user_role.md");
        std::fs::write(&path, "---\ntype: user\n---\nbody").expect("seed file");
        let mut s = DiagnosticsSurface::new();
        s.memory = MemoryReport {
            items: vec![MemoryItem {
                id: "user_role.md".into(),
                path: path.clone(),
                category: "user".into(),
                summary: "user role".into(),
            }],
        };
        s.handle_key(key(KeyCode::Char('3')), &mut App::new());
        (s, path, tmp)
    }

    #[test]
    fn first_d_arms_and_renders_the_confirm_prompt_without_deleting() {
        let (mut s, path, _tmp) = armed_delete_surface();

        // The idle hint is shown before arming; the warning is not.
        let before = render_with_app(&mut s, &App::new());
        assert!(
            before.contains("d delete"),
            "idle hint must show the plain delete affordance"
        );
        assert!(
            !before.contains("press d again"),
            "no confirm prompt before the first d"
        );

        let mut app = App::new();
        s.handle_key(key(KeyCode::Char('d')), &mut app);

        // First d must NOT delete and must NOT push a notice.
        assert!(path.exists(), "first d must not delete the file");
        assert!(
            app.session.turns.is_empty(),
            "first d must not push a system notice"
        );

        // The RENDERED footer now carries the destructive-confirm warning.
        let after = render_with_app(&mut s, &app);
        assert!(
            after.contains("press d again to delete"),
            "first d must render the confirm prompt, got:\n{after}"
        );
        assert!(
            after.contains("cannot be undone"),
            "confirm prompt must warn the action is irreversible"
        );
    }

    #[test]
    fn second_d_confirms_and_deletes() {
        let (mut s, path, _tmp) = armed_delete_surface();
        let mut app = App::new();

        s.handle_key(key(KeyCode::Char('d')), &mut app); // arm
        assert!(path.exists(), "arming must not delete");

        s.handle_key(key(KeyCode::Char('d')), &mut app); // confirm
        assert!(!path.exists(), "second d must delete the file from disk");
        assert_eq!(app.session.turns.len(), 1, "delete must push one notice");
        assert!(
            app.session.turns[0].text().contains("Deleted memory entry"),
            "confirmed delete must surface a system notice"
        );

        // After the delete the list is empty, so the warning is gone.
        let after = render_with_app(&mut s, &app);
        assert!(
            !after.contains("press d again"),
            "confirm prompt must clear after the delete fires"
        );
    }

    #[test]
    fn a_different_key_disarms_the_delete() {
        let (mut s, path, _tmp) = armed_delete_surface();
        let mut app = App::new();

        s.handle_key(key(KeyCode::Char('d')), &mut app); // arm
        let armed = render_with_app(&mut s, &app);
        assert!(
            armed.contains("press d again to delete"),
            "first d must arm + render the prompt"
        );

        // A non-delete key (mode switch back to memory) must disarm.
        s.handle_key(key(KeyCode::Char('3')), &mut app);
        assert!(path.exists(), "disarming key must not delete");

        let disarmed = render_with_app(&mut s, &app);
        assert!(
            !disarmed.contains("press d again"),
            "a different key must clear the confirm prompt, got:\n{disarmed}"
        );

        // And a single `d` after disarming only re-arms (does NOT delete),
        // proving the arm was truly reset rather than carried over.
        s.handle_key(key(KeyCode::Char('d')), &mut app);
        assert!(
            path.exists(),
            "the re-armed first d must not delete after a disarm"
        );
    }

    #[test]
    fn moving_the_cursor_disarms_the_delete() {
        let mut s = sample_memory_surface();
        s.handle_key(key(KeyCode::Char('3')), &mut App::new());
        let mut app = App::new();

        s.handle_key(key(KeyCode::Char('d')), &mut app); // arm row 0
        assert_eq!(s.delete_armed, Some(0));

        // Navigating to another row must cancel the arm so the next `d`
        // cannot fire against the row that was originally selected.
        s.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(s.delete_armed, None, "cursor move must disarm");

        let rendered = render_with_app(&mut s, &app);
        assert!(
            !rendered.contains("press d again"),
            "moving the cursor must clear the confirm prompt"
        );
    }

    #[test]
    fn refresh_memory_clamps_a_stale_cursor() {
        // A re-scan that shrinks the list must not leave the cursor
        // dangling past the end. Drive it through a tempdir-backed scan
        // so `refresh_memory` exercises the real scan path.
        let mut s = sample_memory_surface();
        s.move_memory_cursor(1);
        assert_eq!(s.memory_cursor, 1);
        // Directly shrink + re-clamp the way `refresh_memory` does.
        s.memory = MemoryReport {
            items: vec![MemoryItem {
                id: "only.md".into(),
                path: std::path::PathBuf::from("only.md"),
                category: "user".into(),
                summary: "the only entry".into(),
            }],
        };
        let last = s.memory.items.len().saturating_sub(1);
        s.memory_cursor = s.memory_cursor.min(last);
        assert_eq!(s.memory_cursor, 0);
    }

    /// Render `surface` against an explicit `app` and flatten the buffer.
    fn render_with_app(surface: &mut DiagnosticsSurface, app: &App) -> String {
        let theme = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), app, &theme))
            .expect("render diagnostics");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..24 {
            for x in 0..80 {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    /// Render a diagnostics surface into a tall buffer so the extended
    /// `/doctor` view (5 sections) fits without overflow. The default
    /// 80×24 terminal cuts off the bottom three sections; E2 tests that
    /// assert on TOOLS / RECENT ERRORS / TOKEN BUDGET use this helper.
    fn render_tall(surface: &mut DiagnosticsSurface, app: &App) -> String {
        const W: u16 = 120;
        const H: u16 = 80;
        let theme = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(W, H)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), app, &theme))
            .expect("render diagnostics");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..H {
            for x in 0..W {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    // -- v0.9.0 W4 E2 — extended /doctor sections -------------------------

    /// Per-test batch of env-var mutations. Takes the diagnostics env
    /// lock on construction (so tests don't race on `std::env::set_var`)
    /// and restores every changed key on drop. `std::sync::Mutex` is
    /// not re-entrant, so each test takes one batch and calls
    /// `set`/`unset` repeatedly — not a per-key guard.
    struct EnvBatch {
        priors: Vec<(&'static str, Option<String>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    use std::sync::Mutex;
    static DIAG_ENV_LOCK: Mutex<()> = Mutex::new(());

    impl EnvBatch {
        fn new() -> Self {
            let lock = DIAG_ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            Self {
                priors: Vec::new(),
                _lock: lock,
            }
        }

        fn set(&mut self, key: &'static str, val: &str) {
            self.priors.push((key, std::env::var(key).ok()));
            // SAFETY: env mutations are serialized by `DIAG_ENV_LOCK`.
            unsafe { std::env::set_var(key, val) };
        }

        fn unset(&mut self, key: &'static str) {
            self.priors.push((key, std::env::var(key).ok()));
            // SAFETY: env mutations are serialized by `DIAG_ENV_LOCK`.
            unsafe { std::env::remove_var(key) };
        }
    }

    impl Drop for EnvBatch {
        fn drop(&mut self) {
            for (key, prior) in self.priors.iter().rev() {
                // SAFETY: env mutations are serialized by `DIAG_ENV_LOCK`.
                match prior {
                    Some(v) => unsafe { std::env::set_var(key, v) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    #[test]
    fn doctor_shows_an_mcp_section_with_ready_and_failed_servers() {
        // A4: the MCP SERVERS section answers "why aren't my plugin's tools
        // showing up" — a ready server shows its tool count, a failed one its
        // preserved cause. Tall terminal so the section is not below the fold.
        let mut s = DiagnosticsSurface::new();
        let mut app = App::new();
        app.mcp_status.insert(
            "notion".into(),
            crate::tui::app::McpServerStatus::Ready { tool_count: 6 },
        );
        app.mcp_status.insert(
            "claude-mem".into(),
            crate::tui::app::McpServerStatus::Failed {
                reason: "spawn node".into(),
            },
        );
        // A4c: a server dropped by the pre-connect reachability gate shows as a
        // distinct "skipped" row (Warn, not Fail — a skip is a decision).
        app.mcp_status.insert(
            "ghost-plugin".into(),
            crate::tui::app::McpServerStatus::Skipped {
                reason: "stdio command not launchable".into(),
            },
        );
        s.on_enter(&mut app);
        let out = render_tall(&mut s, &app);
        assert!(
            out.contains("MCP SERVERS"),
            "section header missing:\n{out}"
        );
        assert!(out.contains("notion"), "ready server missing:\n{out}");
        assert!(out.contains("ready"), "ready state missing:\n{out}");
        assert!(out.contains("claude-mem"), "failed server missing:\n{out}");
        assert!(out.contains("failed"), "failure state missing:\n{out}");
        assert!(
            out.contains("ghost-plugin"),
            "skipped server missing:\n{out}"
        );
        assert!(out.contains("skipped"), "skipped state missing:\n{out}");
    }

    #[test]
    fn doctor_mcp_section_reads_none_configured_when_empty() {
        let mut s = DiagnosticsSurface::new();
        let mut app = App::new(); // empty mcp_status
        s.on_enter(&mut app);
        let out = render_tall(&mut s, &app);
        assert!(out.contains("MCP SERVERS"));
        assert!(
            out.contains("none configured"),
            "empty state missing:\n{out}"
        );
    }

    #[test]
    fn doctor_shows_config_section_with_posture_rows() {
        // S8: the CONFIG section surfaces config-posture health. A permissive
        // config flags Warn rows the user can act on.
        let mut s = DiagnosticsSurface::new();
        let mut app = App::new();
        app.config.security_egress_enabled = false; // unrestricted egress
        app.config.tools_auto_approve = true; // every tool unprompted
        app.config.storage_backend = "plaintext".into();
        app.config.failover_enabled = true;
        app.config.fallback_models = Vec::new(); // on but empty
        s.on_enter(&mut app);
        let out = render_tall(&mut s, &app);
        assert!(
            out.contains("CONFIG"),
            "CONFIG section header missing:\n{out}"
        );
        assert!(out.contains("egress guard"), "egress row missing:\n{out}");
        assert!(
            out.contains("credential store"),
            "creds row missing:\n{out}"
        );
        assert!(
            out.contains("tool approval"),
            "tool-approval row missing:\n{out}"
        );
        assert!(
            out.contains("provider failover"),
            "failover row missing:\n{out}"
        );
    }

    #[test]
    fn config_health_flags_permissive_posture_as_warn() {
        // The risky-but-valid states must be Warn (actionable), never a fake
        // Fail and never a silent Ok.
        let mut app = App::new();
        app.config.security_egress_enabled = false;
        app.config.tools_auto_approve = true;
        app.config.storage_backend = "plaintext".into();
        app.config.failover_enabled = true;
        app.config.fallback_models = Vec::new();
        let rows = scan_config_health(&app);
        let warn = |label: &str| {
            rows.iter()
                .find(|r| r.label == label)
                .unwrap_or_else(|| panic!("missing row {label}"))
                .state
        };
        assert_eq!(warn("egress guard"), HealthState::Warn);
        assert_eq!(warn("tool approval"), HealthState::Warn);
        assert_eq!(warn("credential store"), HealthState::Warn);
        assert_eq!(warn("provider failover"), HealthState::Warn);
    }

    #[test]
    fn config_health_marks_hardened_posture_ok() {
        // A locked-down config reads clean: guard on, keyring, ask-each,
        // failover with a chain, a spend cap.
        let mut app = App::new();
        app.config.security_egress_enabled = true;
        app.config.egress_allow = vec!["example.com".into()];
        app.config.tools_auto_approve = false;
        app.config.tools_allow_list = vec!["Read".into(), "Grep".into()];
        app.config.storage_backend = "keyring".into();
        app.config.failover_enabled = true;
        app.config.fallback_models = vec!["anthropic:haiku".into()];
        app.config.budget_max_cost_usd = Some(5.0);
        let rows = scan_config_health(&app);
        assert!(
            rows.iter().all(|r| r.state == HealthState::Ok),
            "a hardened config must have no warnings, got: {:?}",
            rows.iter()
                .filter(|r| r.state != HealthState::Ok)
                .map(|r| (&r.label, &r.detail))
                .collect::<Vec<_>>()
        );
        // The egress row should report the allowlist count.
        let egress = rows.iter().find(|r| r.label == "egress guard").unwrap();
        assert!(
            egress.detail.contains('1'),
            "egress detail should count the allowlist entry: {}",
            egress.detail
        );
    }

    #[test]
    fn doctor_channels_section_surfaces_status_secret_free() {
        // S10: the CHANNELS section makes the on-disk channel configs visible —
        // an enabled known channel shows its option/secret KEY names, an unknown
        // platform reads "won't load". Injecting summaries keeps the test
        // hermetic (no FS / no env); `doctor_collected` is forced so the body
        // renders rather than the "running checks" splash.
        let mut s = DiagnosticsSurface::new();
        s.doctor_collected = true;
        s.channels = vec![
            wcore_channels_registry::ChannelSummary {
                name: "myslack".to_string(),
                platform: "slack".to_string(),
                enabled: true,
                known_platform: true,
                option_keys: vec!["channel".to_string()],
                secret_keys: vec!["bot_token".to_string()],
                parse_error: None,
            },
            wcore_channels_registry::ChannelSummary {
                name: "weird".to_string(),
                platform: "carrierpigeon".to_string(),
                enabled: true,
                known_platform: false,
                option_keys: vec![],
                secret_keys: vec![],
                parse_error: None,
            },
        ];
        let app = App::new();
        let out = render_tall(&mut s, &app);
        assert!(out.contains("CHANNELS"), "section header missing:\n{out}");
        assert!(
            out.contains("myslack") && out.contains("slack"),
            "known channel missing:\n{out}"
        );
        // KEY names are surfaced (never values).
        assert!(out.contains("bot_token"), "secret key name missing:\n{out}");
        assert!(
            out.contains("weird") && out.contains("unknown platform"),
            "unknown-platform channel must read 'won't load':\n{out}"
        );
    }

    #[test]
    fn count_mcp_servers_in_parses_claude_desktop_config() {
        // S11: the only greenfield probe — count `mcpServers` in a Claude
        // Desktop config; absent file / absent key / bad JSON all read None.
        let dir = tempfile::tempdir().expect("tempdir");
        let ok = dir.path().join("claude_desktop_config.json");
        std::fs::write(&ok, r#"{"mcpServers":{"notion":{},"github":{}}}"#).unwrap();
        assert_eq!(count_mcp_servers_in(&ok), Some(2));

        assert_eq!(
            count_mcp_servers_in(&dir.path().join("missing.json")),
            None,
            "absent file must read None"
        );

        let no_key = dir.path().join("empty.json");
        std::fs::write(&no_key, "{}").unwrap();
        assert_eq!(
            count_mcp_servers_in(&no_key),
            None,
            "config without mcpServers must read None"
        );
    }

    #[test]
    fn doctor_discovered_section_shows_found_and_absent_rows() {
        // S11: the DISCOVERED section paints found capabilities prominently and
        // absent ones as dim how-to hints. Injecting rows keeps the test
        // hermetic; `doctor_collected` forces the body to render.
        let mut s = DiagnosticsSurface::new();
        s.doctor_collected = true;
        s.discovered = vec![
            Discovery {
                label: "Ollama (local models)".to_string(),
                available: true,
                detail: "detected — route local models with `ollama:<model>`".to_string(),
            },
            Discovery {
                label: "AWS / Bedrock".to_string(),
                available: false,
                detail: "no AWS credentials (env / ~/.aws / role)".to_string(),
            },
        ];
        let app = App::new();
        let out = render_tall(&mut s, &app);
        assert!(out.contains("DISCOVERED"), "section header missing:\n{out}");
        assert!(
            out.contains("Ollama") && out.contains("detected"),
            "found capability missing:\n{out}"
        );
        assert!(
            out.contains("AWS / Bedrock") && out.contains("no AWS credentials"),
            "absent capability + hint missing:\n{out}"
        );
    }

    #[test]
    fn doctor_lists_all_tools_with_backend_status() {
        // The TOOLS section must include every entry in `TOOL_GATES`,
        // each annotated with its current `enabled` state and the env
        // var that gates it. A user reading /doctor must see "set X to
        // enable Y" for any disabled tool. Uses a tall terminal so the
        // tools section is not cut off below the system + provider rows.
        let mut s = DiagnosticsSurface::new();
        let mut app = App::new();
        s.on_enter(&mut app);
        let out = render_tall(&mut s, &app);
        assert!(out.contains("TOOLS"), "TOOLS section header missing");
        // Every gate must appear by name, paired with its env var.
        for gate in TOOL_GATES {
            assert!(
                out.contains(gate.name),
                "tool `{}` missing from /doctor output",
                gate.name
            );
            assert!(
                out.contains(gate.env_var),
                "env var `{}` (for `{}`) missing from /doctor output",
                gate.env_var,
                gate.name
            );
        }
    }

    #[test]
    fn doctor_shows_yellow_when_key_unset() {
        // With no provider keys set, every provider row must render as
        // Yellow with the "no key" detail string. The brand-yellow `▲`
        // glyph is what HealthState::Warn paints.
        let mut env = EnvBatch::new();
        env.unset("ANTHROPIC_API_KEY");
        env.unset("OPENAI_API_KEY");
        env.unset("GEMINI_API_KEY");
        env.unset("GROQ_API_KEY");

        let mut s = DiagnosticsSurface::new();
        s.on_enter(&mut App::new());
        assert!(drive_health_probe(&mut s), "probe must resolve");

        // All four probes must report Yellow.
        assert_eq!(s.provider_health.len(), 4);
        for ph in &s.provider_health {
            assert_eq!(
                ph.status,
                wcore_agent::health::HealthStatus::Yellow,
                "{} expected Yellow with no key, got {:?}",
                ph.name,
                ph.status
            );
            assert_eq!(ph.detail, "no key");
        }

        // The rendered surface must carry the `no key` detail copy on
        // every provider row.
        let out = render_to_string(&mut s);
        assert!(
            out.contains("PROVIDERS"),
            "PROVIDERS section header missing"
        );
        for name in ["Anthropic", "OpenAI", "Gemini", "Groq"] {
            assert!(
                out.contains(name),
                "provider `{name}` missing from /doctor output"
            );
        }
        assert!(
            out.contains("no key"),
            "yellow detail copy must reach the rendered surface"
        );
    }

    #[test]
    fn doctor_provider_health_times_out_at_5s() {
        // A wedged provider must NOT stall /doctor. We point
        // `ANTHROPIC_API_BASE` at a TCP listener that accepts and never
        // replies, set the api key so the probe is exercised (not skipped as
        // Yellow), then assert two things: (1) `on_enter` returns IMMEDIATELY
        // — the probe runs off-thread, so a wedged provider can never freeze
        // the UI (the on_enter-freeze fix); (2) once the probe is driven to
        // completion the Anthropic row is Red/unreachable, proving the
        // underlying health cap still fired.
        use std::time::Instant;
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        // Spawn the sink listener on a worker thread (the surface's
        // `on_enter` is sync; we need the listener live for the whole
        // duration of the probe).
        let listener = std::thread::spawn(|| {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("rt");
            rt.block_on(async {
                let l = TcpListener::bind("127.0.0.1:0").await.expect("bind");
                let addr = l.local_addr().expect("addr");
                tokio::spawn(async move {
                    loop {
                        if let Ok((mut sock, _)) = l.accept().await {
                            let mut buf = [0u8; 1024];
                            let _ = sock.read(&mut buf).await;
                            // Park forever — slowloris shape.
                            std::future::pending::<()>().await;
                        }
                    }
                });
                addr.port()
            })
        })
        .join()
        .expect("listener thread");

        let mut env = EnvBatch::new();
        env.set(
            "ANTHROPIC_API_BASE",
            &format!("http://127.0.0.1:{listener}"),
        );
        env.set("ANTHROPIC_API_KEY", "sk-ant-test");
        env.unset("OPENAI_API_KEY");
        env.unset("GEMINI_API_KEY");
        env.unset("GROQ_API_KEY");

        let mut s = DiagnosticsSurface::new();
        let started = Instant::now();
        s.on_enter(&mut App::new());
        let elapsed = started.elapsed();

        // on_enter must NOT block on the (wedged) probe — it only starts the
        // worker thread and returns. Generous slack for thread-spawn on slow CI.
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "/doctor on_enter must not block on the health probe (took {elapsed:?})"
        );

        // Drive the off-thread probe to completion (the wedged provider hits
        // the underlying 5s cap), then check the result.
        assert!(
            drive_health_probe(&mut s),
            "probe must resolve within budget"
        );

        // The Anthropic row must be Red with an unreachable detail.
        let anth = s
            .provider_health
            .iter()
            .find(|p| p.name == "Anthropic")
            .expect("Anthropic probe must run");
        assert_eq!(anth.status, wcore_agent::health::HealthStatus::Red);
        assert!(
            anth.detail.starts_with("unreachable:"),
            "anthropic detail must mark unreachable: {}",
            anth.detail
        );
    }

    #[test]
    fn doctor_recent_errors_pulls_from_session_turns() {
        use crate::tui::app::{TurnRole, TurnView};
        let mut s = DiagnosticsSurface::new();
        let mut app = App::new();
        // Seed two error turns + one non-error system turn (must be
        // ignored by the filter).
        app.session.turns.push(TurnView {
            role: TurnRole::System,
            elements: vec![TurnElement::Markdown("Error [E001]: rate limited".into())],
        });
        app.session.turns.push(TurnView {
            role: TurnRole::System,
            elements: vec![TurnElement::Markdown("Note: cache miss".into())],
        });
        app.session.turns.push(TurnView {
            role: TurnRole::System,
            elements: vec![TurnElement::Markdown("Error [E002]: invalid model".into())],
        });
        s.on_enter(&mut app);
        let out = render_tall(&mut s, &app);
        assert!(out.contains("RECENT ERRORS"), "errors header missing");
        assert!(out.contains("E001"), "first error missing");
        assert!(out.contains("E002"), "second error missing");
        // The non-error system turn must NOT appear in the panel.
        assert!(
            !out.contains("cache miss"),
            "non-error system turn leaked into errors panel"
        );
    }

    #[test]
    fn doctor_token_budget_renders_from_app_context() {
        let mut s = DiagnosticsSurface::new();
        let mut app = App::new();
        app.config.model = "claude-sonnet-4-6".into();
        app.context.used_tokens = 12_500;
        app.context.window_size = 200_000;
        app.session.tokens_out = 421;
        s.on_enter(&mut app);
        let out = render_tall(&mut s, &app);
        assert!(out.contains("TOKEN BUDGET"), "budget header missing");
        assert!(out.contains("claude-sonnet-4-6"), "model label missing");
        assert!(out.contains("12500"), "used tokens missing");
        assert!(out.contains("200000"), "window size missing");
        assert!(out.contains("187500"), "remaining tokens missing");
        assert!(out.contains("421"), "last-turn output tokens missing");
    }

    /// A diagnostics surface preloaded with two memory entries.
    fn sample_memory_surface() -> DiagnosticsSurface {
        let mut s = DiagnosticsSurface::new();
        s.memory = MemoryReport {
            items: vec![
                MemoryItem {
                    id: "user_role.md".into(),
                    path: std::path::PathBuf::from("user_role.md"),
                    category: "user".into(),
                    summary: "user role — Rust engineer on wayland-core".into(),
                },
                MemoryItem {
                    id: "project_context.md".into(),
                    path: std::path::PathBuf::from("project_context.md"),
                    category: "project".into(),
                    summary: "project context — CLI/TUI redesign in flight".into(),
                },
            ],
        };
        s
    }
}
