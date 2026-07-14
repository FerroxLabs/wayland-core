//! The wayland-core ratatui terminal UI.
//!
//! `run()` is the single entry point: it sets up the terminal (raw mode +
//! alternate screen), installs a panic hook that restores the terminal
//! before any panic message prints, runs the draw/poll loop at ~30fps,
//! and tears the terminal back down on exit.
//!
//! The crossterm lifecycle (`run`) is a thin shell verified manually. The
//! per-frame logic — draw the router + handle one input event — is
//! factored into `step`, which is pure with respect to the terminal and
//! fully unit-testable with a ratatui `TestBackend` (no real TTY).
//!
//! Wave 0 ships the foundation: an empty themed shell with tab chrome,
//! routing across `StubSurface`s, and a clean panic-safe lifecycle. Live
//! engine wiring is T2.1; the `main.rs` dispatch into `run()` is T2.3.

// The `tui` module publishes its full integration boundary — the
// `Theme`/`App`/widget/`Surface`/command contracts. Some of that surface
// (widget free fns, contract variants) is only consumed by snapshot
// tests, so a module-wide `dead_code` allow keeps the build quiet
// without scattering per-item `#[allow]`s.
#![allow(dead_code)]

// v0.9.2 S0: module decls for the v0.9.2 redesign waves. Declared up front
// in the S0 scaffold so no later wave re-edits this decl region; each wave
// fills in its own module body (see the per-file stub headers).
pub mod agents; // v0.9.3 — multi-agent navigation (strip + glow + stale)
pub mod anim; // W1 — animation clock
pub mod app;
// v0.9.0 W4 E1 — `/auth google-meet` slash-command handler.
mod auth;
// D019 — workspace checkpoint store backing `/rewind` (capture/list/restore).
mod checkpoint;
mod commands;
mod engine_bridge;
mod event;
#[cfg(test)]
pub mod fixtures;
mod frecency;
mod keybind;
pub mod onboarding; // v0.9.3 — one-shot onboarding hints
pub mod permission; // W2 — per-tool approval components
mod protocol_bridge;
// v0.9.0 TUI-V1 W2 C5: streaming-safe markdown split-point helper.
// Sibling renderers (markdown, reasoning-tag filter, tool-card formatters)
// land alongside in W2.
pub mod render;
pub mod state; // W10 — state store + transient/toast slice
pub mod statusline; // W5 — status-line background sampler
pub mod streaming; // W6 — verb pool + single-pick mechanics
pub mod surfaces;
// v0.9.0 Wave-1 B0 (R-H8): RAII guard that restores the terminal on
// every non-panic exit path. The companion panic hook lives in this
// file (`install_panic_hook`) and covers the unwind path.
mod terminal_guard;
pub mod theme;
pub mod theme_detect; // W8 — terminal background detection
mod tool_formatters;
mod turn_element;
mod widgets;

use std::io::{Stdout, stdout};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use tokio::sync::mpsc::UnboundedReceiver;
use wcore_protocol::events::ProtocolEvent;

use self::event::InputEvent;

use self::app::App;
// Re-export the integration types `main.rs` needs to build a `TuiSession`.
pub use self::app::{ConfigView, ContextView};
pub use self::engine_bridge::{
    ChannelEmitter, ChannelSink, DedupeSet, EngineInventory, HookInfo, McpServerInfo, SkillInfo,
    TuiEngine, approval_mode_to_session, build_rebind_system_prompt,
};
use self::protocol_bridge::spawn_bridge;
// Resume repaint: the host (`main.rs::run_tui_mode`, a separate bin crate)
// rebuilds a restored session's transcript via this before constructing the
// `TuiSession`, so the re-export is `pub` (not `pub(crate)`).
pub use self::protocol_bridge::hydrate_history;
use self::surfaces::{Router, SurfaceId};
use self::theme::Theme;
// Re-export `TurnElement` so callers (and tests) can construct turn
// elements via `crate::tui::TurnElement` rather than the internal module.
pub use self::turn_element::TurnElement;
// v0.9.0 TUI-V1 W2 C1: re-export the markdown renderer so the transcript
// surface (and tests) call `crate::tui::render_markdown` directly.
pub use self::render::markdown::render_markdown;

/// Target frame interval — ~30fps. Also the input-poll timeout while the
/// animation clock wants ticks, so the loop wakes promptly on a key press
/// and otherwise redraws on the tick.
const TICK: Duration = Duration::from_millis(33);

/// v0.9.2 W1 (SPEC §1A): the input-poll slice the idle path uses when the
/// animation clock does NOT want ticks. Short on purpose — the blocking
/// poll thread re-checks `wants_tick()` (i.e. whether the bridge has
/// subscribed the clock on a fresh `StreamStart`) at most this often, so a
/// first streamed token can never be stuck behind a long blocking poll.
/// The `IDLE_DWELL` `select!` arm only bounds the absolute idle wake.
const IDLE_SLICE: Duration = Duration::from_millis(200);

/// v0.9.2 W1: the long fallback dwell at idle. With no input and no engine
/// activity the loop wakes at most once per `IDLE_DWELL` to redraw — the
/// rest of the time it is parked, which is the <2%-idle-CPU lever. An
/// inbound stream is observed within `IDLE_SLICE` (see above), not after a
/// full `IDLE_DWELL`, because the poll slice re-checks `wants_tick()`.
const IDLE_DWELL: Duration = Duration::from_millis(1000);

/// The blocking-poll slice the dedicated input reader thread (see `run`) uses
/// between checks for a dropped receiver. Short enough that the thread exits
/// promptly on shutdown; an actual key / paste / resize returns from
/// `event::poll` the instant it is pending, so this is NOT input latency.
const INPUT_READ_SLICE: Duration = Duration::from_millis(50);

/// The input-poll timeout for one loop iteration. `TICK` (33ms, ~30fps)
/// while the clock wants ticks; the short `IDLE_SLICE` otherwise so the
/// idle `select!`'s blocking poll re-checks `wants_tick()` promptly. Pure
/// and unit-tested so the dwell policy is verified without a TTY.
fn dwell_timeout(wants_tick: bool) -> Duration {
    if wants_tick { TICK } else { IDLE_SLICE }
}

/// v0.9.2 W10 (SPEC §1B): the redraw-skip decision.
///
/// Redraw when the animation clock wants a tick (something is animating, so
/// the frame must keep repainting at 30fps) OR the transient slice changed
/// since the last draw (a cost / mcp / context / toast update needs to
/// paint). At a fully-idle prompt — `wants_tick` false AND the transient
/// slice unchanged — the loop's idle-dwell wake would otherwise repaint a
/// byte-identical frame; this returns `false` so it skips that wasted draw,
/// the redraw-skip lever that rides on W1's `wants_tick`. Pure + unit-tested
/// so the policy is verified without a TTY.
fn should_redraw(slice_changed: bool, wants_tick: bool) -> bool {
    wants_tick || slice_changed
}

// ── Terminal compatibility matrix ─────────────────────────────────────────
//
// The TUI uses only the crossterm feature set that is portable across the
// terminals below. Each row is the behaviour relied on and how it degrades
// where the terminal lacks it — verified by reasoning against each
// emulator's documented capabilities, not by a live grid (CI has no TTY).
//
// | Terminal          | Alt-screen | Raw mode | Bracketed paste | Resize event | Notes |
// |-------------------|-----------|----------|-----------------|--------------|-------|
// | tmux              | yes       | yes      | yes (passthrough)| yes         | inner TERM may be `screen-256color`; colours degrade, layout fine |
// | iTerm2 (macOS)    | yes       | yes      | yes             | yes          | full support |
// | Apple Terminal    | yes       | yes      | yes             | yes          | 256-colour, no truecolor — Theme RGB is quantised by the terminal |
// | Windows Terminal  | yes       | yes      | yes             | yes          | emits `KeyEventKind::Release`; `poll_input` filters non-Press |
// | kitty             | yes       | yes      | yes             | yes          | kitty keyboard protocol unused — plain keys keep it portable |
// | Ghostty           | yes       | yes      | yes             | yes          | full support |
// | TERM=dumb / pipe  | n/a       | n/a      | n/a             | n/a          | not a TTY — `main.rs` falls back to the readline path (T2.3) |
//
// Degradation contract:
//  * Bracketed paste: if a terminal silently ignores `EnableBracketedPaste`,
//    a paste still arrives as a burst of individual key events.
//    `poll_input` drains the whole burst in one tick (`MAX_DRAIN`), so a
//    paste is never one-keystroke-per-frame slow even without the feature.
//  * Resize: a terminal that does not emit `Event::Resize` still redraws
//    correctly — every frame re-reads `frame.area()`, so the next ~33ms
//    tick adopts the new size. The resize event only makes that adoption
//    immediate rather than tick-delayed.
//  * Colour: `NO_COLOR` / a non-truecolor terminal is handled by the
//    `Theme` layer; this module emits no raw escape sequences of its own.

/// Everything `main.rs` builds before handing control to the TUI: the
/// live engine controller, the engine→TUI event channel, and the
/// resolved-config snapshot for the status bar.
///
/// `main.rs` builds the `AgentEngine`, wires the channel-backed
/// `ProtocolEmitter`/`OutputSink`, and constructs this bundle; the TUI
/// then owns the render loop. Keeping the engine bootstrap in `main.rs`
/// — where the plugin force-links and the provider router already live —
/// avoids a circular crate dependency and keeps `tui::run` a thin host.
pub struct TuiSession {
    /// The engine controller the router drives.
    pub engine: TuiEngine,
    /// The receiver half of the engine→TUI event channel. The bridge
    /// task drains this into `App`.
    pub events: UnboundedReceiver<ProtocolEvent>,
    /// The resolved-config snapshot shown in the status bar.
    pub config: app::ConfigView,
    /// Typed posture that was installed in the shared approval manager at
    /// launch. The status bar must display this exact authority, including
    /// config-selected AutoEdit/Bypass and legacy auto-approve resolution.
    pub initial_mode: wcore_protocol::commands::SessionMode,
    /// The context-window size for the status meter.
    pub context: app::ContextView,
    /// True when no global config file exists yet — a true first run.
    /// The TUI starts on the Onboarding surface when set, and on the
    /// Workspace surface when a config is already present.
    pub first_run: bool,
    /// Force the TUI to start on the Onboarding surface even when a
    /// config already exists — the `wayland-core setup` re-entry point.
    /// When `false` the `first_run` gate decides the initial surface.
    pub force_onboarding: bool,
    /// On a `--resume` / `--continue` boot, the prior conversation rebuilt
    /// into view models (`TurnView`s + `ToolCardModel`s) by
    /// [`protocol_bridge::hydrate_history`]. Seeded into the initial `App`'s
    /// transcript before the render loop starts so the user sees their
    /// history, not a blank screen. Empty for a fresh session.
    pub restored_turns: Vec<app::TurnView>,
    /// The tool cards correlated to `restored_turns` (referenced by id from
    /// the turns' `ToolCard` elements). Seeded alongside `restored_turns`.
    pub restored_tool_cards: Vec<app::ToolCardModel>,
}

/// Build the status-bar [`ConfigView`](app::ConfigView) snapshot from a
/// resolved engine `Config`. Called by `main.rs` before the `Config` is
/// moved into the engine bootstrap.
pub fn config_view_from(config: &wcore_config::config::Config) -> app::ConfigView {
    let smart_policy = config.smart_approval_policy();
    app::ConfigView {
        provider: config.provider_label.clone(),
        model: config.model.clone(),
        prompt_caching: config.prompt_caching,
        memory_enabled: config.memory.enabled,
        max_turns: config.max_turns,
        compaction: config.compact.compaction.to_string(),
        approval: match smart_policy {
            wcore_types::execution_policy::ApprovalPolicy::Prompt => "default",
            wcore_types::execution_policy::ApprovalPolicy::AutoEdit => "auto-edit",
            wcore_types::execution_policy::ApprovalPolicy::Bypass => "force",
        }
        .to_string(),
        plan_first: config.plan.plan_first,
        // The TUI host (`main.rs::run_tui_mode`) sets this launch-authority
        // flag from `cli.force` after this snapshot is taken. Active posture
        // lives in `App::mode`, so the default here is `false`.
        force: false,
        // The active provider's resolved `ProviderCompat` cost overrides,
        // seeded so the Expert tier shows + persists the real pricing.
        compat_costs: app::CompatCosts {
            input: config.compat.cost_per_input_token,
            output: config.compat.cost_per_output_token,
            cache_read: config.compat.cost_per_cache_read_token,
            cache_write: config.compat.cost_per_cache_write_token,
        },
        // S5 Essentials: tools posture + budget cap, read straight from the
        // resolved config so the home shows the live values.
        tools_auto_approve: matches!(
            smart_policy,
            wcore_types::execution_policy::ApprovalPolicy::Bypass
        ),
        tools_allow_list: config.tools.allow_list.clone(),
        tools_verify_edits: config.tools.verify_edits,
        budget_max_cost_usd: config.budget.max_cost_usd,
        budget_max_wall_secs: config.budget.max_wall_time_secs,
        // S6 Advanced: observability toggles, storage backend tag, egress guard.
        obs_structured_traces: config.observability.structured_traces,
        obs_online_evolution: config.observability.online_evolution,
        obs_workflow_live: config.observability.workflow_live_mode,
        storage_backend: match &config.storage.credentials.backend {
            wcore_config::credentials::CredentialsBackend::Auto => "auto",
            wcore_config::credentials::CredentialsBackend::Plaintext => "plaintext",
            wcore_config::credentials::CredentialsBackend::Keyring => "keyring",
            wcore_config::credentials::CredentialsBackend::EncryptedFile { .. } => "encrypted-file",
        }
        .to_string(),
        security_egress_enabled: config.security.enabled,
        // S7 collection editors: the egress allowlist and the provider
        // failover chain, read straight from the resolved config.
        egress_allow: config.security.egress_allow.clone(),
        failover_enabled: config.provider_chain.enabled,
        fallback_models: config.provider_chain.fallback_models.clone(),
    }
}

/// Build the status-bar [`ContextView`](app::ContextView) snapshot from a
/// resolved engine `Config`. The window size is the compaction
/// `context_window`; `used_tokens` starts at zero and is updated live by
/// the protocol bridge as the session runs.
pub fn context_view_from(config: &wcore_config::config::Config) -> app::ContextView {
    app::ContextView {
        used_tokens: 0,
        window_size: config.compact.context_window as u64,
    }
}

/// Run the TUI: set up the terminal, run the async render loop, restore
/// on exit.
///
/// `session` is `Some` for the normal path (a live engine attached) and
/// `None` for a degraded launch (no engine — e.g. config resolution
/// failed but the user still wants to browse the UI). Returns `Ok(())`
/// on a clean quit. The terminal is restored on every exit path — clean
/// return, error, or panic.
pub async fn run(session: Option<TuiSession>) -> Result<()> {
    let (terminal, guard) = enter()?;
    run_attached(terminal, guard, session).await
}

/// Enter the full-screen TUI: raw mode, the alt-screen (+ bracketed paste,
/// focus reporting, mouse capture), the panic-restore hook, and the RAII
/// terminal guard. Returns the live `Terminal` and its guard so the caller
/// can render a boot splash on it *before* a session exists, then hand both
/// to [`run_attached`]. Entering the alt-screen exactly once here is what lets
/// the splash and the main loop share one terminal — entering it twice
/// corrupts the screen.
pub fn enter() -> Result<(
    Terminal<CrosstermBackend<Stdout>>,
    terminal_guard::TerminalGuard,
)> {
    enable_raw_mode().context("failed to enable terminal raw mode")?;
    // Enter the alt-screen and ask the terminal to bracket pastes. With
    // bracketed paste on, a paste arrives as one `Event::Paste` blob the
    // input layer decomposes in a single tick — without it the same paste
    // would dribble in one keystroke per frame. A terminal that does not
    // support the escape simply ignores it (see the matrix above).
    //
    // 2026-05-31: mouse capture is ON by default at boot so the scroll wheel
    // drives the transcript out of the box. The prior F13 off-default (for
    // native drag-copy) read as "I can't scroll — the wheel moves my terminal,
    // not the app", which is the stronger expectation to honour. Drag-select
    // copy still works via Shift+drag (the standard host-terminal bypass); F4
    // toggles capture OFF (`toggle_mouse_capture()` issues `DisableMouseCapture`
    // at runtime) for terminals without a Shift bypass, e.g. Apple Terminal.
    // `DisableMouseCapture` is also emitted on shutdown (see `restore_terminal`)
    // as a safety net. Keep this in sync with `App::mouse_capture_enabled`'s
    // `true` default — both must agree or the flag and the terminal desync.
    // v0.9.2 W1 (SPEC §1A): ask the terminal to report focus changes so the
    // AnimationClock can pause animation ticks while the window is blurred.
    // A terminal that ignores the escape simply emits no focus events and the
    // clock never pauses on blur — the idle-dwell CPU win still holds.
    execute!(
        stdout(),
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableFocusChange,
        EnableMouseCapture,
    )
    .context("failed to enter alternate screen")?;
    install_panic_hook();

    // v0.9.0 Wave-1 B0 (R-H8): RAII guard restores the terminal on
    // every non-panic exit path, including the `?`-bubble below if
    // `Terminal::new` fails. The panic path is covered by
    // `install_panic_hook` above.
    let guard = terminal_guard::TerminalGuard::new();

    let backend = CrosstermBackend::new(stdout());
    let terminal = Terminal::new(backend).context("failed to initialize terminal")?;
    Ok((terminal, guard))
}

/// Run the render/poll loop on an already-entered terminal (see [`enter`]),
/// restoring it on every exit path. Split from [`run`] so the boot path can
/// render a splash on the same terminal while the engine builds.
pub async fn run_attached(
    mut terminal: Terminal<CrosstermBackend<Stdout>>,
    guard: terminal_guard::TerminalGuard,
    session: Option<TuiSession>,
) -> Result<()> {
    // Run the loop, capturing its result so the terminal is always restored
    // before returning regardless of how the loop ended. `guard`'s Drop also
    // calls `restore_terminal()` (idempotent).
    let loop_result = run_loop(&mut terminal, session).await;
    drop(guard);
    loop_result
}

/// Render a branded boot splash on `terminal` while `fut` (the engine build)
/// runs, returning `fut`'s output the instant it completes. The splash is
/// driven by a `select!` against a frame ticker — `fut` is polled in place
/// (no `tokio::spawn`, so it needs no `'static`/`Send`), and the spinner
/// animates at ~11fps so a multi-second MCP connect shows live progress
/// instead of a blank terminal.
pub async fn splash_while<F: std::future::Future>(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    mcp_count: usize,
    fut: F,
) -> F::Output {
    let theme = theme::Theme::detect();
    tokio::pin!(fut);
    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(90));
    let mut frame: u64 = 0;
    loop {
        tokio::select! {
            out = &mut fut => return out,
            _ = ticker.tick() => {
                let _ = terminal.draw(|f| draw_splash(f, &theme, mcp_count, frame));
                frame = frame.wrapping_add(1);
            }
        }
    }
}

/// Paint one frame of the boot splash: the wordmark centred over a spinner
/// line. `mcp_count` is the *config-declared* server count known before the
/// connect runs (installed-plugin servers are discovered during build, so the
/// copy stays honest by not claiming a total).
fn draw_splash(f: &mut ratatui::Frame, theme: &theme::Theme, mcp_count: usize, frame: u64) {
    use ratatui::layout::{Alignment, Constraint, Layout};
    use ratatui::style::{Modifier, Style};
    use ratatui::text::{Line, Span};
    use ratatui::widgets::{Block, Paragraph};

    let area = f.area();
    let bg = Style::default().bg(theme.bg);
    f.render_widget(Block::default().style(bg), area);

    let spinner = widgets::spinner_frame(frame);
    let sub = if mcp_count > 0 {
        format!(
            "{spinner}  starting engine · connecting {mcp_count} MCP server{}…",
            if mcp_count == 1 { "" } else { "s" }
        )
    } else {
        format!("{spinner}  starting engine · connecting tools & MCP servers…")
    };
    let lines = vec![
        Line::from(Span::styled(
            "WAYLAND CORE",
            bg.fg(theme.orange).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(sub, bg.fg(theme.text_muted))),
    ];
    let [_, mid, _] = Layout::vertical([
        Constraint::Percentage(45),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).style(bg),
        mid,
    );
}

/// The async draw/poll loop. Hosts the engine event bridge alongside the
/// render loop: a `tokio` task drains the engine→TUI channel into the
/// shared `App`, while this loop draws every tick and polls input.
///
/// `App` lives behind one `Arc<Mutex<App>>` shared between the bridge
/// task and the render loop — the same single-writer-per-field discipline
/// the Wave-0 contract describes (the bridge writes engine-driven fields,
/// the router writes routing/composer fields). Each tick the loop takes
/// the lock only for the short, non-blocking `render` + `handle_key`
/// work; the blocking input poll happens off-thread with the lock
/// released, so the bridge task is never starved.
async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    session: Option<TuiSession>,
) -> Result<()> {
    // v0.9.2 WIRE-RUNTIME (§5 / Q1): the LIVE theme is no longer a fixed
    // run-loop local — the `Router` owns it (it boots on `Theme::detect()`,
    // the dark path that honors `NO_COLOR`, identical to the prior local).
    // Each frame the loop reads `router.theme()` (cloned — `Theme` is
    // `Copy`) so a `/theme <mode>` command, handled inside
    // `Router::dispatch_command`, re-resolves the theme in place and the
    // very next render repaints with the new palette. No restart.

    // First-run gate: start on Onboarding only when no config exists yet
    // (or the engine failed to attach at all). A returning user lands
    // straight on the Workspace — unless `force_onboarding` is set
    // (`wayland-core setup`), which always opens Onboarding.
    let initial_surface = match session.as_ref() {
        Some(s) if s.force_onboarding => SurfaceId::Onboarding,
        Some(s) if !s.first_run => SurfaceId::Workspace,
        _ => SurfaceId::Onboarding,
    };
    let app = Arc::new(Mutex::new(App::with_initial_surface(initial_surface)));

    // v0.9.2 W1 (audit H2 — idle-wake latency): the bridge→loop wake
    // signal. The render loop cannot `select!` on the engine channel
    // directly (the bridge task owns `engine_rx`), so the bridge calls
    // `notify_one()` after every applied event and the idle `select!`
    // below has a `notified()` arm. A bridge push at a fully-idle prompt
    // (first streamed token, error turn, budget toast) then wakes the loop
    // within ~one frame instead of waiting out the up-to-200ms `IDLE_SLICE`
    // input poll. `Notify` stores one permit, so a notify that races ahead
    // of the await is not lost.
    let wake = Arc::new(tokio::sync::Notify::new());

    // Attach the engine (if any): seed the config snapshot, spawn the
    // event bridge, and give the router its engine controller.
    let mut router = {
        let guard = app.lock().expect("fresh app lock");
        Router::new(&guard)
    };
    if let Some(s) = session {
        {
            let mut guard = app.lock().expect("fresh app lock");
            // Mirror the exact typed launch posture already installed in the
            // approval manager. Config-selected AutoEdit/Bypass must never run
            // behind a misleading Default badge.
            guard.mode = s.initial_mode;
            guard.config = s.config;
            guard.context = s.context;
            // Resume repaint: seed the rebuilt prior conversation into the
            // transcript so a `--resume` / `--continue` boot shows history
            // instead of a blank screen. Done before the bridge spawns so the
            // first frame already carries it; live events then append normally.
            if !s.restored_turns.is_empty() {
                guard.session.turns = s.restored_turns;
                guard.session.tool_cards = s.restored_tool_cards;
            }
            // Seed mcp_status from the boot health snapshot so `/doctor` shows
            // MCP server health the first time it opens. Boot-time MCP connect
            // does NOT emit McpReady/McpFailed to the TUI (only the inventory
            // snapshot carries it); live `/mcp add` events update these entries
            // afterward through the same map.
            for info in &s.engine.inventory().mcp_servers {
                let status = match &info.health {
                    wcore_mcp::manager::McpServerHealth::Ready { tool_count } => {
                        app::McpServerStatus::Ready {
                            tool_count: *tool_count,
                        }
                    }
                    wcore_mcp::manager::McpServerHealth::Failed { reason } => {
                        app::McpServerStatus::Failed {
                            reason: reason.clone(),
                        }
                    }
                    wcore_mcp::manager::McpServerHealth::TimedOut { .. } => {
                        app::McpServerStatus::TimedOut
                    }
                    wcore_mcp::manager::McpServerHealth::Skipped { reason } => {
                        app::McpServerStatus::Skipped {
                            reason: reason.clone(),
                        }
                    }
                };
                guard.mcp_status.insert(info.name.clone(), status);
            }
        }
        spawn_bridge(s.events, app.clone(), wake.clone());
        router = router.with_engine(s.engine);
    }

    // v0.9.2 WIRE-RUNTIME (W5): start the off-thread statusLine sampler once
    // at run-loop startup, inside the tokio runtime (the W5 module requires
    // this). `statusline::init` spawns the background task only when a
    // `statusLine.command` is set — and it is SETTINGS-FILE-ONLY (never
    // model-writable, see the W5 security note). No config source plumbs the
    // command through yet, so we pass the default (`command: None`): `init`
    // is then a no-op and the curated default bar renders. The status bar
    // (`widgets::status_bar`) already READS the cached line as plain data;
    // wiring the spawn here is what makes a user-set command actually run.
    statusline::init(&statusline::StatusLineConfig::default());

    // D009 input delivery: a DEDICATED reader thread owns the crossterm event
    // source. The idle loop below used to race `spawn_blocking(poll_input)`
    // inside a `select!` against `wake` / `sleep`; when `wake` won (a bridge
    // push after a turn settles leaves a pending `Notify` permit), the dropped
    // `poll_input` future left its blocking `event::read()` ORPHANED — and that
    // orphan then consumed, and discarded, the user's NEXT keystroke. The
    // symptom was a TUI that silently stopped accepting input after a turn
    // completed (gap D009, misfiled as a "render livelock" — the render is
    // O(viewport) and fast; the keystroke never reached the loop). A single
    // long-lived reader thread forwarding over a channel removes the race: the
    // loop consumes via a cancel-safe `recv()`, so dropping the recv future on
    // a `wake` / `sleep` win leaves queued events intact for the next iteration.
    let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<event::InputEvent>();
    std::thread::Builder::new()
        .name("wld-input".into())
        .spawn(move || {
            loop {
                match event::poll_input(INPUT_READ_SLICE) {
                    Ok(events) => {
                        for ev in events {
                            if input_tx.send(ev).is_err() {
                                return; // receiver dropped — the run loop has exited
                            }
                        }
                    }
                    // A persistent poll error ends the reader; the loop then sees a
                    // closed channel and degrades to a no-input UI rather than
                    // aborting the whole TUI on a transient terminal hiccup.
                    Err(_) => return,
                }
                if input_tx.is_closed() {
                    return;
                }
            }
        })
        .expect("spawn input reader thread");

    // v0.9.2 W10 (SPEC §1B): the redraw-skip bookkeeping. `last_transient_rev`
    // is the transient-slice revision we last drew; when it is unchanged AND
    // the clock does not want a tick, the per-iteration draw is skipped
    // (`should_redraw`). `force_redraw` is set after any input/resize so the
    // FOLLOWING iteration always repaints (a keystroke handled this iteration
    // is reflected in the next draw) — and `true` on the first iteration so
    // the initial frame always paints.
    let mut last_transient_rev = u64::MAX; // != any real revision → first draw always happens
    let mut force_redraw = true;
    // v0.9.2 W10: a heartbeat repaint bound. The redraw-skip kills the
    // ~5x/sec byte-identical idle redraws (the IDLE_SLICE poll wakes), but a
    // background bridge push (an error / budget system turn that does NOT
    // animate, so `wants_tick` stays false and the transient slice is
    // unchanged) must still paint promptly. Force at least one redraw per
    // `IDLE_DWELL` so such a push is visible within ≤1s — the same staleness
    // bound W1's idle wake already accepts — without the per-poll waste.
    let mut last_draw_at = std::time::Instant::now();

    loop {
        // Draw + plan-mode sync under a short lock; bail once `quit` set.
        // `wants_tick` is read from the animation clock inside this same
        // lock so the dwell decision below uses a coherent snapshot; the
        // lock is dropped before the `await`, never held across it.
        let clock_wants_tick;
        {
            // D015: recover a poisoned lock instead of `.expect()` aborting the
            // process (which would leave the terminal raw/bricked). A surface
            // panic is already contained by catch_unwind in the input dispatch,
            // so the guard is normally un-poisoned; this is belt-and-braces.
            let mut guard = app.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            if guard.quit {
                break;
            }
            router.sync_plan_mode(&mut guard);
            // v0.9.1 W1-B: the centered approval-modal auto-open/close
            // (sync_approval_modal) was removed. Inline approval cards
            // live in the transcript via `widgets::render_approval_inline`
            // and the right-rail Activity panel mirrors pending counts.
            // Flush a message queued mid-turn once the engine is idle
            // (AUDIT-D D3) — the type-ahead the composer hint advertises.
            router.flush_queued_message(&mut guard);
            // v0.9.1 W2 cycle-2 HIGH 1: drain a single card off any armed
            // batch-approval queue per tick (`A` / `N` keypress on a
            // multi-card pending approval). One action per tick keeps the
            // user's eyes anchored on the card flipping as it processes
            // and preserves the frozen one-action-per-event contract.
            router.tick_active(&mut guard);
            // v0.9.2 audit M2: clear an expired toast THROUGH the store so
            // the revision bumps and the redraw-skip below repaints promptly.
            // The status-bar widget only stops DRAWING the toast after its
            // dwell (it holds `&App`); without this the field stayed set and a
            // fully-idle prompt showed the stale toast frame up to ~1s (until
            // the heartbeat). Clearing it here (we hold `&mut App`) bumps the
            // revision so the very next frame drops the toast.
            guard.dismiss_expired_toast(widgets::TOAST_DWELL);
            // v0.9.2 W1 (SPEC §1A): the animation clock is the single
            // monotonic tick source now. `advance()` bumps it and the
            // result feeds `App::frame_tick` (the read surface the spinner /
            // streaming-status widgets animate against — they are untouched).
            // While animating, `wants_tick()` is true and the loop polls at
            // `TICK` (33ms), so `frame_tick` advances at ~30fps; at idle the
            // loop parks and `frame_tick` barely moves (nothing animates).
            clock_wants_tick = guard.anim.wants_tick();
            // v0.9.2 W10 (SPEC §1B): the redraw-skip. Compute whether the
            // transient slice changed since the last draw (a value-changing
            // `set_transient` bumped the revision), then decide via the pure
            // `should_redraw`. At a fully-idle prompt with an unchanged slice
            // this skips the byte-identical repaint the idle-dwell wake would
            // otherwise issue. `force_redraw` (first iteration + the frame
            // after any input/resize) overrides the skip so a keystroke is
            // never left unpainted.
            let cur_rev = guard.transient_revision();
            let slice_changed = cur_rev != last_transient_rev;
            let heartbeat_due = last_draw_at.elapsed() >= IDLE_DWELL;
            if force_redraw || heartbeat_due || should_redraw(slice_changed, clock_wants_tick) {
                // Advance the monotonic tick ONLY on a real draw so
                // `frame_tick` does not run ahead of painted frames at idle
                // (it is read only while animating, where we always draw).
                let tick = guard.anim.advance();
                // Snapshot the router's LIVE theme (Copy) before borrowing
                // the router mutably for `step` — so `/theme` swaps applied
                // last tick are picked up here on the next frame.
                let theme = *router.theme();
                step(terminal, &mut guard, &mut router, &theme, tick, None)?;
                last_transient_rev = cur_rev;
                last_draw_at = std::time::Instant::now();
                force_redraw = false;
            }
        }

        // Poll a whole batch of input off-thread — the lock is released,
        // so the bridge task keeps draining engine events while we wait.
        // `poll_input` returns every event already buffered, so a paste
        // (a fast key burst, or one bracketed-paste blob) is consumed in
        // this single tick instead of one keystroke per frame.
        //
        // v0.9.2 W1 (SPEC §1A, audit HIGH — idle-wake race): when the clock
        // wants ticks we poll at `TICK` (current 30fps behaviour). When it
        // does not, we `select!` a SHORT (`IDLE_SLICE`) blocking poll
        // against a long `IDLE_DWELL` timeout and park the rest of the time
        // — the <2%-idle-CPU lever. The poll slice is deliberately short so
        // that when the engine bridge subscribes the clock on a fresh
        // `StreamStart` (it owns `engine_rx`, so we cannot `select!` on the
        // channel directly here), the next loop iteration re-reads
        // `wants_tick()` within at most `IDLE_SLICE` and resumes 30fps —
        // the first streamed token can never sit behind a 1s blocking poll.
        let batch: Vec<event::InputEvent> = if clock_wants_tick {
            // Animating: collect whatever input arrived within ~one frame so
            // the 30fps pacing is preserved (an empty return redraws the next
            // animation tick), then drain any co-arrived burst in one go.
            match tokio::time::timeout(TICK, input_rx.recv()).await {
                Ok(Some(first)) => {
                    let mut v = vec![first];
                    while let Ok(ev) = input_rx.try_recv() {
                        v.push(ev);
                    }
                    v
                }
                // The reader thread ended (channel closed): the TUI has lost its
                // SOLE input source, so no key — including the quit chord — can
                // ever arrive again. Treat it as a fatal-but-clean shutdown: set
                // `quit` and break so the `TerminalGuard` restores the terminal
                // and the user is dropped back to a usable shell, rather than
                // spinning forever drawing a UI that accepts no input (the
                // reader-thread-death deadlock).
                Ok(None) => {
                    let mut guard = app.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    guard.quit = true;
                    break;
                }
                // Timed out this frame — no input, redraw on the next tick.
                Err(_) => Vec::new(),
            }
        } else {
            tokio::select! {
                // Cancel-safe: if `wake` / `sleep` wins, the dropped `recv`
                // future leaves any queued event in the channel for the next
                // iteration — no keystroke is consumed-and-discarded the way the
                // old `spawn_blocking(poll_input)` select arm orphaned a
                // blocking read after a turn settled (gap D009).
                first = input_rx.recv() => match first {
                    Some(first) => {
                        let mut v = vec![first];
                        while let Ok(ev) = input_rx.try_recv() {
                            v.push(ev);
                        }
                        v
                    }
                    // Reader thread gone — same fatal-but-clean shutdown as the
                    // animating branch above. Without this the loop would take
                    // this `None` branch every iteration with an empty `batch`,
                    // never reaching `apply_quit_chord`, so even Ctrl+C (a
                    // KeyEvent through the now-dead channel) could not quit:
                    // the entire TUI would be wedged until the process is killed
                    // externally. Quit cleanly instead.
                    None => {
                        let mut guard =
                            app.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                        guard.quit = true;
                        break;
                    }
                },
                // v0.9.2 W1 (audit H2): a bridge push (first streamed
                // token, error turn, budget toast) signals `wake`, which
                // resolves this arm immediately so the loop re-reads
                // `wants_tick()` / the transient slice on the next
                // iteration and paints within ~one frame — not after the
                // up-to-`IDLE_DWELL` park. Returns no input events;
                // the redraw is driven by the bridge's `App` mutation.
                _ = wake.notified() => Vec::new(),
                _ = tokio::time::sleep(IDLE_DWELL) => Vec::new(),
            }
        };

        if !batch.is_empty() {
            // v0.9.2 W10: any input/paste/mouse/focus event handled this
            // iteration must be reflected on screen. The draw happens at the
            // TOP of the next iteration, so force it past the redraw-skip —
            // a keystroke is never left unpainted by the idle optimization.
            force_redraw = true;
            // D015: recover a poisoned lock rather than aborting (see render lock).
            let mut guard = app.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let mut needs_resize = false;
            for ev in batch {
                match ev {
                    InputEvent::Key(key) => {
                        if apply_quit_chord(&mut guard, key) {
                            if guard.quit {
                                break;
                            }
                            // First press: armed, not quitting — the key
                            // is consumed, do not route it on.
                            continue;
                        }
                        router.handle_key(key, &mut guard);
                    }
                    // A bracketed-paste blob: route to the surface as a
                    // paste-insert action so the composer can absorb the
                    // full text in one shot, without emitting Enter per
                    // embedded newline (F-041, the wallet-drain fix).
                    InputEvent::PastedBlock(text) => {
                        router.handle_paste(text, &mut guard);
                    }
                    // A resize mid-stream just forces an immediate redraw;
                    // the actual size is re-read from `frame.area()`. The
                    // terminal autoresizes the ratatui buffer, so there is
                    // nothing to corrupt — drawing once with the new area
                    // is the whole fix. v0.9.2 W1 (SPEC §1A): a
                    // resize-to-zero is the offscreen proxy — pause the
                    // animation clock so we stop ticking while there are no
                    // visible cells; a non-zero resize unpauses.
                    InputEvent::Resize { cols, rows } => {
                        if cols == 0 || rows == 0 {
                            guard.anim.set_paused(true);
                        } else {
                            guard.anim.set_paused(false);
                            needs_resize = true;
                        }
                    }
                    // D2/v0.9.0: scroll-wheel ticks drive transcript
                    // scrollback in the workspace; every other surface's
                    // default `handle_mouse` drops the event.
                    InputEvent::Mouse(m) => {
                        router.handle_mouse(m, &mut guard);
                    }
                    // v0.9.2 W1 (SPEC §1A): focus drives the clock pause so
                    // animation ticks stop while the user is in another
                    // window and resume when they return. On a terminal that
                    // never emits focus events these arms simply never fire.
                    InputEvent::FocusGained => guard.anim.set_paused(false),
                    InputEvent::FocusLost => guard.anim.set_paused(true),
                }
            }
            if needs_resize && !guard.quit {
                terminal
                    .autoresize()
                    .context("failed to resize terminal buffer")?;
                router.sync_plan_mode(&mut guard);
                // v0.9.2 W1: this immediate-redraw path advances the clock
                // too so `frame_tick` stays monotonic across a resize-driven
                // extra draw (otherwise a held frame index could repeat).
                let tick = guard.anim.advance();
                // Snapshot the router's LIVE theme (Copy) — same reason as
                // the per-tick draw above (`/theme` live-switch).
                let theme = *router.theme();
                step(terminal, &mut guard, &mut router, &theme, tick, None)?;
            }
        }
    }
    // The render loop has exited — fire the config/plugin Stop hooks while the
    // router still owns the engine (the REPL/json-stream surfaces already do
    // this; the TUI could not until the engine controller exposed it).
    router.run_stop_hooks().await;
    Ok(())
}

/// One iteration of the render loop's pure work: draw the current frame,
/// then apply the key polled this tick (if any).
///
/// Generic over the ratatui `Backend` and fully exercisable with a
/// `TestBackend` — no real terminal or stdin required.
fn step<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    router: &mut Router,
    theme: &Theme,
    tick: u64,
    key: Option<ratatui::crossterm::event::KeyEvent>,
) -> Result<()>
where
    // ratatui 0.30 generalized `Backend::Error` from `io::Error` to an
    // associated type, so `Terminal::draw` now returns `Result<_, B::Error>`.
    // anyhow's `.context()` needs the error to be `Error + Send + Sync`.
    // `TestBackend`'s `Infallible` and `CrosstermBackend`'s `io::Error` both
    // satisfy this.
    B::Error: std::error::Error + Send + Sync + 'static,
{
    // Publish the loop tick onto `App` so surfaces can animate the
    // "working" spinner (AUDIT-D D8). The `Surface::render` trait is a
    // FROZEN Wave-0 contract, so the tick rides on `App` rather than a
    // new render parameter.
    app.frame_tick = tick;
    // W3 D3: timer-driven phase transition — `Drafting → WrappingUp` after
    // 15s of TextDelta silence. Cheap (one duration compare) and called
    // every tick because the bridge has no clock of its own.
    app.session.tick_streaming_phase(std::time::Instant::now());
    terminal
        .draw(|frame| router.render(frame, frame.area(), app, theme))
        .context("failed to draw frame")?;

    if let Some(key) = key {
        // A `Ctrl+C` is consumed by the quit-chord guard (first press
        // arms, second quits); any other key disarms it and routes on.
        if !apply_quit_chord(app, key) {
            router.handle_key(key, app);
        }
    }
    Ok(())
}

/// True if `key` is the global quit chord (`Ctrl+C`). Plain `q` is
/// handled per-surface by the router (a surface with a text field must
/// not treat a literal `q` as quit); `Ctrl+C` is the one global chord.
fn is_quit_chord(key: ratatui::crossterm::event::KeyEvent) -> bool {
    use ratatui::crossterm::event::{KeyCode, KeyModifiers};
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

/// Apply the two-press `Ctrl+C` quit guard for one key event.
///
/// A single stray `Ctrl+C` must never kill the session, so the chord is
/// a confirm:
/// - First `Ctrl+C`: arm `App::quit_armed` (the status bar shows
///   "Press Ctrl+C again to exit"); the session keeps running.
/// - Second `Ctrl+C` while armed: set `App::quit` — the loop exits.
/// - Any other key while armed: disarm, clearing the hint.
///
/// Returns `true` when the key was a `Ctrl+C` and was consumed by the
/// guard (so the caller must NOT route it on to a surface); `false` when
/// the key is an ordinary key the caller should route normally.
fn apply_quit_chord(app: &mut App, key: ratatui::crossterm::event::KeyEvent) -> bool {
    if is_quit_chord(key) {
        if app.quit_armed {
            // Second press — confirmed. Quit.
            app.quit = true;
        } else {
            // First press — arm and show the hint, do not quit.
            app.quit_armed = true;
        }
        true
    } else {
        // Any other key disarms a pending quit, clearing the hint.
        app.quit_armed = false;
        false
    }
}

/// Restore the terminal to its pre-TUI state: disable raw mode and leave
/// the alternate screen.
///
/// Idempotent and TTY-independent — every step is a no-op-safe crossterm
/// call, so this may be called twice (loop exit + panic hook) or with no
/// TTY attached (tests) without error. Failures are swallowed: there is
/// nothing useful to do if cleanup itself fails, and surfacing it would
/// only obscure the real exit cause.
pub(crate) fn restore_terminal() {
    let _ = disable_raw_mode();
    // Mirror `run`'s setup in reverse: stop mouse capture + bracketed
    // paste + focus reporting, then leave the alt-screen. All are no-op-safe,
    // so a double call (loop exit + panic hook) or a call with no TTY is
    // harmless. `DisableMouseCapture` runs BEFORE `LeaveAlternateScreen`
    // so the host terminal is restored to a clean keyboard-only state
    // before we hand it back. `DisableFocusChange` (W1) stops the terminal
    // emitting focus escapes once we are back to the host shell.
    let _ = execute!(
        stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        DisableFocusChange,
        LeaveAlternateScreen,
    );
}

/// Install a panic hook that restores the terminal BEFORE the default
/// hook prints the panic message — otherwise the message would be drawn
/// into the alternate screen and lost, leaving the user in a corrupted
/// shell. The previous hook is chained so the panic still reports.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal();
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn restore_terminal_is_idempotent_without_a_tty() {
        // Calling restore repeatedly with no real terminal attached must
        // not panic — the panic hook + loop-exit path both call it.
        restore_terminal();
        restore_terminal();
        restore_terminal();
    }

    #[test]
    fn step_draws_without_a_real_terminal() {
        // `step` must render through a TestBackend with no TTY and no
        // stdin. With `None` for the polled key it is a pure draw.
        // The theme is built inline (not via `Theme::hearth()`, whose body
        // is a T0.4 stub) so this test exercises only Wave-0 logic.
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new();
        let mut router = Router::new(&app);
        let theme = test_theme();
        step(&mut terminal, &mut app, &mut router, &theme, 0, None).expect("step renders");
        assert!(!app.quit);
    }

    #[test]
    fn step_routes_a_polled_key_through_the_router() {
        // A polled key fed to `step` reaches the focused surface. The
        // onboarding surface's `s` shortcut picks the Skip path and
        // advances to its Ready step; a following confirm key
        // (`Enter`) then routes a `Switch(Workspace)` back through the
        // router. This proves keys flow step → router → surface →
        // router-apply end to end.
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new();
        let mut router = Router::new(&app);
        let theme = test_theme();
        let skip = KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE);
        step(&mut terminal, &mut app, &mut router, &theme, 0, Some(skip)).expect("step renders");
        let enter = KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE);
        step(&mut terminal, &mut app, &mut router, &theme, 1, Some(enter)).expect("step renders");
        assert_eq!(app.surface, surfaces::SurfaceId::Workspace);
        assert!(!app.quit);
    }

    #[test]
    fn first_run_gate_picks_the_initial_surface() {
        // The first-run gate: a fresh user (no config yet) starts on the
        // Onboarding surface; a returning user (config on disk) starts on
        // the Workspace. `App::with_initial_surface` is the seam the TUI
        // host uses to apply that decision.
        let first_run = App::with_initial_surface(surfaces::SurfaceId::Onboarding);
        assert_eq!(first_run.surface, surfaces::SurfaceId::Onboarding);
        let returning = App::with_initial_surface(surfaces::SurfaceId::Workspace);
        assert_eq!(returning.surface, surfaces::SurfaceId::Workspace);
        // The bare `App::new()` is the first-run default.
        assert_eq!(App::new().surface, surfaces::SurfaceId::Onboarding);
    }

    #[test]
    fn step_arms_then_quits_on_a_double_ctrl_c() {
        // `Ctrl+C` is the global quit chord, but it now confirms: the
        // first press through `step` only arms the guard; the second
        // sets `App::quit`. A stray single press never kills the session.
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new();
        let mut router = Router::new(&app);
        let theme = test_theme();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);

        // First press: armed, not quitting.
        step(
            &mut terminal,
            &mut app,
            &mut router,
            &theme,
            0,
            Some(ctrl_c),
        )
        .expect("step renders");
        assert!(app.quit_armed, "first Ctrl+C should arm the guard");
        assert!(!app.quit, "first Ctrl+C must not quit");

        // Second press while armed: quit.
        step(
            &mut terminal,
            &mut app,
            &mut router,
            &theme,
            1,
            Some(ctrl_c),
        )
        .expect("step renders");
        assert!(app.quit, "second Ctrl+C should quit");
    }

    /// An uncolored `Theme` for render tests — a render only needs *a*
    /// valid `Theme`, and `no_color()` is the simplest one.
    fn test_theme() -> Theme {
        Theme::no_color()
    }

    #[tokio::test]
    async fn statusline_sampler_spawn_is_a_noop_without_a_command() {
        // v0.9.2 WIRE-RUNTIME (W5): the run-loop calls `statusline::init`
        // once at startup. With the default config (no `statusLine.command`)
        // it must NOT spawn a background task and must NOT panic — the
        // curated default bar renders instead. Calling it inside a tokio
        // runtime (as the real run-loop does) and confirming the cache stays
        // empty proves the no-command path is inert.
        statusline::init(&statusline::StatusLineConfig::default());
        assert!(
            statusline::cached_line().is_none(),
            "no command set ⇒ nothing should be published into the cache"
        );
    }

    #[test]
    fn step_paints_the_routers_live_theme() {
        // v0.9.2 WIRE-RUNTIME (§5 / Q1): the run-loop reads `router.theme()`
        // each frame and hands it to `step`. After a `/theme light` dispatch
        // the router's live theme is the light palette, so the snapshot the
        // loop takes (`*router.theme()`) is the light theme and the frame is
        // painted with the light bg. This is the loop-side half of the
        // live-switch contract (the dispatch half is tested in `surfaces`).
        use surfaces::SurfaceAction;
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new();
        let mut router = Router::new(&app);

        // Boot theme is dark; flip it live.
        let dark_bg = router.theme().bg;
        router.apply(SurfaceAction::Command("/theme light".into()), &mut app);
        let live = *router.theme();
        assert_ne!(live.bg, dark_bg, "/theme light must change the live bg");

        // The loop's per-frame read path: snapshot then draw. No panic, and
        // the theme handed to `step` is the freshly-resolved light one.
        let snapshot = *router.theme();
        step(&mut terminal, &mut app, &mut router, &snapshot, 0, None).expect("step renders");
        assert_eq!(snapshot.bg, live.bg, "step paints the router's live theme");
    }

    #[test]
    fn step_redraws_cleanly_after_a_mid_stream_resize() {
        // A resize while content is on screen must not corrupt the
        // display: the next draw re-reads `frame.area()` and repaints the
        // whole buffer at the new size. Simulate it by shrinking the
        // backend between two `step`s and confirming the second draw fills
        // the new, smaller buffer without panic.
        let backend = TestBackend::new(100, 40);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new();
        let mut router = Router::new(&app);
        let theme = test_theme();

        step(&mut terminal, &mut app, &mut router, &theme, 0, None).expect("first draw");
        // Shrink the terminal mid-session, the way a window drag would.
        terminal.backend_mut().resize(48, 12);
        terminal.autoresize().expect("autoresize after shrink");
        step(&mut terminal, &mut app, &mut router, &theme, 1, None).expect("redraw at new size");

        // The buffer now matches the new geometry exactly — no stale rows
        // or columns from the larger layout survive.
        let area = terminal.backend().buffer().area;
        assert_eq!((area.width, area.height), (48, 12));
        assert!(!app.quit);
    }

    #[test]
    fn step_handles_a_large_burst_of_keys_in_one_call() {
        // A paste decomposes into many `Char` keys. Feeding a long run of
        // them through `step` one after another must stay fast and never
        // panic — this is the unit-level analogue of the loop draining a
        // whole paste batch in a single tick.
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new();
        let mut router = Router::new(&app);
        let theme = test_theme();
        let key = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        for i in 0..5_000u64 {
            step(&mut terminal, &mut app, &mut router, &theme, i, Some(key))
                .expect("burst key routes");
        }
        assert!(!app.quit);
    }

    #[test]
    fn ctrl_c_is_recognized_as_the_quit_chord() {
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(is_quit_chord(ctrl_c));

        let plain_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::NONE);
        assert!(!is_quit_chord(plain_c));

        let ctrl_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert!(!is_quit_chord(ctrl_x));
    }

    #[test]
    fn first_ctrl_c_arms_the_guard_without_quitting() {
        // A single stray `Ctrl+C` must not kill the session — it only
        // arms the guard so the status bar can prompt to press again.
        let mut app = App::new();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        let consumed = apply_quit_chord(&mut app, ctrl_c);
        assert!(consumed, "Ctrl+C is consumed by the guard, not routed on");
        assert!(app.quit_armed, "first press arms the guard");
        assert!(!app.quit, "first press must not quit");
    }

    #[test]
    fn second_ctrl_c_while_armed_quits() {
        // The confirm press: with the guard already armed, a second
        // `Ctrl+C` sets `App::quit`.
        let mut app = App::new();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        apply_quit_chord(&mut app, ctrl_c);
        assert!(app.quit_armed);
        let consumed = apply_quit_chord(&mut app, ctrl_c);
        assert!(consumed);
        assert!(app.quit, "second Ctrl+C while armed quits");
    }

    #[test]
    fn any_other_key_disarms_a_pending_quit() {
        // Once armed, pressing anything that is not `Ctrl+C` clears the
        // pending state (and the hint) — and a following `Ctrl+C` is
        // back to being a first press that only re-arms.
        let mut app = App::new();
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        apply_quit_chord(&mut app, ctrl_c);
        assert!(app.quit_armed);

        let plain_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE);
        let consumed = apply_quit_chord(&mut app, plain_x);
        assert!(!consumed, "an ordinary key is routed on, not consumed");
        assert!(!app.quit_armed, "any other key disarms the guard");
        assert!(!app.quit, "disarming never quits");

        // The next `Ctrl+C` is a fresh first press — arms, does not quit.
        apply_quit_chord(&mut app, ctrl_c);
        assert!(app.quit_armed);
        assert!(!app.quit);
    }

    // ── v0.9.2 W1: animation-clock dwell + focus/pause ───────────────────

    #[test]
    fn dwell_is_short_when_animating_and_sliced_at_idle() {
        // The dwell policy: poll at 33ms (TICK) while the clock wants ticks,
        // and at the short IDLE_SLICE otherwise so the idle select! re-checks
        // wants_tick() promptly (the first-token latency bound).
        assert_eq!(dwell_timeout(true), TICK);
        assert_eq!(dwell_timeout(false), IDLE_SLICE);
        // The idle slice must be short enough that a freshly-subscribed clock
        // is observed within one slice, well under the long fallback dwell.
        assert!(
            IDLE_SLICE < IDLE_DWELL,
            "the poll slice bounds first-token latency"
        );
    }

    // ── v0.9.2 W10: redraw-skip policy ───────────────────────────────────

    #[test]
    fn should_redraw_skips_only_when_idle_and_slice_unchanged() {
        // The §10 risk-2 acceptance: a no-op set on the transient slice at a
        // fully-idle prompt does NOT redraw; a changed slice does; anything
        // animating always redraws regardless of the slice.
        assert!(!should_redraw(false, false), "idle + unchanged → skip");
        assert!(should_redraw(true, false), "idle + slice changed → draw");
        assert!(should_redraw(false, true), "animating → always draw");
        assert!(should_redraw(true, true), "animating + changed → draw");
    }

    #[test]
    fn no_op_transient_set_does_not_bump_the_revision_so_no_redraw() {
        // The live redraw-skip reads `App::transient_revision()`. Prove the
        // store's no-op guard keeps it unchanged across an identical write
        // (so `slice_changed` is false and `should_redraw(false, false)` is
        // false), while a real change bumps it (forcing a redraw).
        let mut app = App::new();
        let rev0 = app.transient_revision();
        // Identical-value write — the Object.is no-op guard fires nothing.
        app.set_transient(|prev| prev.clone());
        assert_eq!(
            app.transient_revision(),
            rev0,
            "a no-op set must NOT bump the revision (→ should_redraw stays false)"
        );
        assert!(
            !should_redraw(app.transient_revision() != rev0, false),
            "idle + unchanged revision → no redraw"
        );
        // A real change bumps the revision → the loop will redraw.
        app.set_transient(|prev| crate::tui::state::TransientSlice {
            toast: Some("ready".into()),
            ..prev.clone()
        });
        assert_ne!(
            app.transient_revision(),
            rev0,
            "a value-changing set bumps the revision"
        );
        assert!(
            should_redraw(app.transient_revision() != rev0, false),
            "idle + changed revision → redraw"
        );
        // And the canonical field was mirrored for the renderers.
        assert_eq!(app.toast.as_deref(), Some("ready"));
    }

    #[test]
    fn expired_toast_dismiss_bumps_revision_and_clears_the_field() {
        // v0.9.2 audit M2: the loop clears an expired toast THROUGH the store
        // so the revision bumps → the next frame redraws and the toast drops.
        // A toast that has NOT outlived its dwell is left untouched (no bump,
        // no premature dismissal).
        let mut app = App::new();
        // Show a toast, then backdate `toast_at` past the dwell so it counts
        // as expired without sleeping.
        app.set_transient(|prev| crate::tui::state::TransientSlice {
            toast: Some("ready".into()),
            toast_at: Some(std::time::Instant::now()),
            ..prev.clone()
        });
        let rev_shown = app.transient_revision();
        assert_eq!(app.toast.as_deref(), Some("ready"));

        // Not yet expired (huge dwell) → no-op: field intact, revision flat.
        let cleared = app.dismiss_expired_toast(std::time::Duration::from_secs(3600));
        assert!(!cleared, "a non-expired toast must NOT be dismissed");
        assert_eq!(app.toast.as_deref(), Some("ready"));
        assert_eq!(
            app.transient_revision(),
            rev_shown,
            "a no-op dismiss must not bump the revision"
        );

        // Now treat it as expired (zero dwell) → cleared THROUGH the store:
        // field gone, revision bumped, so the redraw-skip repaints.
        let cleared = app.dismiss_expired_toast(std::time::Duration::ZERO);
        assert!(cleared, "an expired toast must be dismissed");
        assert!(app.toast.is_none(), "the toast field is cleared");
        assert!(app.toast_at.is_none(), "the toast_at field is cleared");
        let rev_after = app.transient_revision();
        assert_ne!(
            rev_after, rev_shown,
            "clearing the toast through the store bumps the revision"
        );
        assert!(
            should_redraw(rev_after != rev_shown, false),
            "a bumped revision at idle forces the redraw that drops the toast"
        );
    }

    #[test]
    fn a_fresh_app_clock_is_idle_then_a_subscription_makes_it_want_ticks() {
        // The run_loop reads `App::anim.wants_tick()` to choose its dwell.
        // A fresh app is idle (no subscribers); subscribing the spinner (as
        // the bridge does on StreamStart, W1 Task 1.5) flips it true so the
        // loop returns to 30fps and the first token paints within a slice.
        let mut app = App::new();
        assert!(!app.anim.wants_tick(), "fresh app must be idle");
        app.anim.subscribe(anim::AnimId::Spinner, false);
        assert!(app.anim.wants_tick(), "a subscribed clock wants ticks");
        app.anim.unsubscribe(anim::AnimId::Spinner);
        assert!(
            !app.anim.wants_tick(),
            "releasing the last subscriber idles it"
        );
    }

    #[test]
    fn focus_lost_pauses_the_clock_and_focus_gained_resumes_it() {
        // The run_loop maps InputEvent::FocusLost/Gained onto the clock
        // pause; verify the clock semantics the loop relies on. A paused
        // clock never wants a tick even while a subscriber is active.
        let mut app = App::new();
        app.anim.subscribe(anim::AnimId::Spinner, false);
        assert!(app.anim.wants_tick());
        app.anim.set_paused(true); // FocusLost
        assert!(!app.anim.wants_tick(), "blurred clock must not tick");
        app.anim.set_paused(false); // FocusGained
        assert!(app.anim.wants_tick(), "refocus resumes ticking");
    }

    #[test]
    fn step_feeds_the_advanced_tick_through_to_frame_tick() {
        // run_loop advances the clock and passes the result to `step`, which
        // publishes it onto App::frame_tick (the spinner read surface). Prove
        // the tick reaches frame_tick unchanged.
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).expect("test terminal");
        let mut app = App::new();
        let mut router = Router::new(&app);
        let theme = test_theme();
        let tick = app.anim.advance();
        step(&mut terminal, &mut app, &mut router, &theme, tick, None).expect("step renders");
        assert_eq!(app.frame_tick, tick, "frame_tick mirrors the clock tick");
    }
}
