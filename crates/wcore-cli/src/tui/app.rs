//! Central view-state model for the ratatui TUI.
//!
//! `App` is the single source of truth the TUI renders from. It is mutated
//! ONLY by two writers — the protocol bridge (T0.5, draining engine events)
//! and the surface router (T0.3, applying `SurfaceAction`s). Surfaces read
//! `&App` to render and receive `&mut App` in `handle_key`; they never spawn
//! their own writers.
//!
//! All public types in this file are FROZEN Wave-0 contracts: Wave-1 surface
//! agents build against these signatures and must not change them. Fields are
//! deliberately minimal-but-real — grounded in the mockup surfaces and the
//! `ProtocolEvent` payloads — so later waves extend by adding, not reshaping.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::time::Instant;

use crate::tui::anim::AnimationClock;
use crate::tui::checkpoint::CheckpointStore;
use crate::tui::render::ReasoningFilter;
use crate::tui::surfaces::SurfaceId;
use crate::tui::turn_element::TurnElement;

/// The whole TUI view state. FROZEN Wave-0 contract.
///
/// Why the live engine must be rebound from freshly-written disk. A typed
/// one-shot (L1: not a bare bool) so every producer declares intent: the single
/// consumer in `Router::tick_active` is unambiguous, a future caller cannot
/// silently repurpose a generic flag for an unrelated reason, and the reason is
/// available for logging. `is_pending()` gates the consume site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RebindRequest {
    /// No rebind pending.
    #[default]
    None,
    /// A Tier-1 settings save (approval / plan-first / stop-after / compaction
    /// / long-term memory).
    Tier1Save,
    /// A provider credential (API key) write via the Providers modal.
    Credential,
}

impl RebindRequest {
    /// True when a rebind is queued and not yet consumed.
    pub fn is_pending(self) -> bool {
        !matches!(self, RebindRequest::None)
    }
}

/// `App` deliberately derives nothing: it lives behind an
/// `Arc<Mutex<App>>` shared between the render loop and the protocol
/// bridge, so it is neither cloned nor copied. `mode` holds the real
/// `wcore_protocol::commands::SessionMode`, which is not `Clone`.
pub struct App {
    /// The currently active full-screen surface.
    pub surface: SurfaceId,
    /// An optional overlay surface drawn on top of `surface` (e.g. the
    /// command palette). `None` when no overlay is open.
    pub overlay: Option<SurfaceId>,
    /// The live conversation: turns, streaming buffer, tool cards, sub-agents.
    pub session: SessionView,
    /// A small snapshot of the resolved engine `Config` (not the whole thing).
    pub config: ConfigView,
    /// The session approval mode. Constructed only as `SessionMode::Default`
    /// in Wave 0; mode cycling is Wave 2.
    pub mode: wcore_protocol::commands::SessionMode,
    /// Context-window usage for the status-bar meter.
    pub context: ContextView,
    /// Set true to break the render loop and exit cleanly.
    pub quit: bool,
    /// The right-rail path map — files the agent has touched this
    /// session. Host-derived: the protocol bridge populates this from
    /// tool activity (there is no engine event for a path map). Wave-2
    /// additive field.
    pub path_map: TreeModel,
    /// The plan presented by the engine's `EnterPlanMode` tool, if any.
    /// `Some` while the session is in plan mode; the router switches to
    /// the plan-review surface when this becomes `Some`. Wave-2 additive
    /// field.
    pub plan: Option<PlanView>,
    /// Session token usage + spend, from the latest
    /// `ProtocolEvent::SessionCost`. `None` until the first cost event
    /// arrives (the engine emits it only when `cost_attribution` is on).
    /// The diagnostics `/cost` screen renders this. Wave-3 additive field.
    pub cost: Option<SessionCostView>,
    /// Whether the workspace right rail (path map · tools · activity) is
    /// shown. `Ctrl+B` toggles it; when `false` the transcript takes the
    /// full body width. Defaults to `true`.
    pub rail_visible: bool,
    /// Accidental-exit guard for the `Ctrl+C` quit chord. The first
    /// `Ctrl+C` arms this and shows a "press again to exit" hint instead
    /// of quitting; a second `Ctrl+C` while armed quits. Any other key
    /// disarms it. A stray single `Ctrl+C` must never kill the session.
    pub quit_armed: bool,
    /// Monotonic frame counter, advanced once per render tick by the
    /// render loop (`step`). Surfaces read it to animate the "working"
    /// spinner so a live slow turn is visually distinct from a hung one
    /// (AUDIT-D D8). Wraps cleanly — only `% FRAMES.len()` is ever used.
    pub frame_tick: u64,
    /// A user message typed while a turn was still streaming, held until
    /// the current turn ends and then submitted (AUDIT-D D3 — the real
    /// "queue a message" the composer hint advertises). `None` when no
    /// message is queued. Only ever one is held: a second `Enter` while
    /// streaming overwrites it, matching a single-slot type-ahead.
    pub queued_message: Option<String>,
    /// D007 keystone: one-shot signal that a `/config` Tier-1 save just
    /// landed on disk and the LIVE engine must be rebound to it. The
    /// Typed one-shot from a `ConfigSurface` / credential save asking the
    /// router to rebind the live engine from freshly-written disk. The router
    /// consumes it once per tick (`tick_active`), calls `engine.rebind()`, and
    /// resets it to `RebindRequest::None`. A `RebindRequest` rather than a
    /// `SurfaceAction` keeps the FROZEN Wave-0 `SurfaceAction` enum untouched
    /// and lets the save (deep in a per-tier key handler with no router access)
    /// signal the router without a new action variant.
    pub rebind_request: RebindRequest,
    /// M1/M2: one-shot honesty flag set by the router when a `config_saved`
    /// rebind's SYNCHRONOUS `Config::resolve` + `create_provider` failed —
    /// the engine kept its prior binding and the live apply was skipped.
    /// `ConfigSurface::render` reads this so the context line shows "saved to
    /// disk · live apply skipped" instead of a false "saved · now live".
    /// Cleared by the router on the next successful rebind. `false` at boot
    /// (no save pending) and on every successful apply.
    pub config_apply_failed: bool,
    /// v0.9.1 W1-B: MCP server readiness, keyed by server name. The
    /// protocol bridge writes a `Ready { tool_count }` entry on every
    /// `McpReady` event so the diagnostics `/doctor` screen and the
    /// right-rail Activity panel can surface MCP status without a
    /// transcript-spamming system turn per server.
    pub mcp_status: HashMap<String, McpServerStatus>,
    /// Mouse capture toggle for transcript scroll vs native text selection.
    /// `true` (DEFAULT, 2026-05-31) — `EnableMouseCapture` is active, so the
    /// scroll wheel drives transcript scrollback out of the box. This reverts
    /// the v0.9.1.3 F13 off-default: users overwhelmingly expect the wheel to
    /// scroll the transcript, and "the wheel scrolls my terminal's old output,
    /// not the app" reads as broken. To drag-select/copy text, hold **Shift**
    /// (the standard bypass that hands the drag to the host terminal).
    /// `false` (via `F4`) — `DisableMouseCapture`; the host terminal owns all
    /// mouse events so plain drag-select copy works everywhere (the trade for
    /// terminals without a Shift bypass, e.g. Apple Terminal).
    /// `F4` toggles either direction; the status hint reflects the live state.
    /// PgUp/PgDn/Home/End keyboard scroll works in both modes.
    pub mouse_capture_enabled: bool,
    /// v0.9.1.2 F14: One-shot trigger that asks the next render pass to
    /// scroll the topmost `AwaitingApproval` tool card into view. The
    /// protocol bridge sets it `true` on `ApprovalRequired`; the
    /// transcript renderer consumes it (resets to `false`) after the
    /// scroll offset has been clamped to the card's row. The lock that
    /// prevents auto-scroll-to-bottom while ANY card is pending is a
    /// separate concern derived from `session.tool_cards` directly — this
    /// flag fires only on the transition into the awaiting state.
    pub force_scroll_to_pending_approval: bool,
    /// v0.9.1.2 F8: Bash-style command history — the most recent user
    /// prompts, oldest first. `ArrowUp` on an empty composer walks backward
    /// through this and pastes the prompt into the composer; `ArrowDown`
    /// walks forward toward the present and clears the composer when it
    /// runs past the newest entry. The cap is 50 (a session-local ring,
    /// not a persistent shell history) — older prompts roll off the front.
    pub recent_user_prompts: VecDeque<String>,
    /// v0.9.1.2 F8: Position within `recent_user_prompts` while the user
    /// is actively walking history. `None` means "not currently navigating
    /// history" — the next `ArrowUp` lands on the most recent prompt.
    /// `Some(i)` means the composer currently displays
    /// `recent_user_prompts[i]`. Reset to `None` on any non-history key
    /// (typing, submit) so the user is never stuck in history mode.
    pub history_cursor: Option<usize>,
    /// v0.9.2 W1 (SPEC §1A): the animation clock. Owns per-`AnimId`
    /// subscriptions + a paused flag; the render loop reads `wants_tick()`
    /// to decide whether to schedule another tick and routes `frame_tick`
    /// through `advance()`. S0 scaffold installs the default; W1 lands the
    /// real subscribe/advance logic behind this stable field.
    pub anim: AnimationClock,
    /// v0.9.2 W5 (SPEC §3 S7): a transient toast string for demoted status
    /// events (e.g. `McpReady` → toast, not a transcript turn). `None` when
    /// nothing is showing. The protocol bridge sets it; the render path
    /// auto-dismisses after a short dwell (see `toast_at`). W5 wires the
    /// demotion + dismiss; S0 scaffolds the field.
    pub toast: Option<String>,
    /// v0.9.2 W5: when the current `toast` was set, for auto-dismiss. `None`
    /// when no toast is showing. W5 reads this against the clock to clear an
    /// expired toast; S0 scaffolds the field.
    pub toast_at: Option<std::time::Instant>,
    /// v0.9.2 W7 (SPEC §3 S21): collapsed/expanded state of per-turn
    /// reasoning blocks, keyed by turn index. Absent or `false` = collapsed
    /// (the default `▶ Thought: …` one-liner); `true` = expanded body. W7
    /// toggles entries on `Tab`+`Enter`; S0 scaffolds the empty map.
    pub reasoning_expanded: std::collections::HashMap<usize, bool>,
    /// v0.9.2 W10 (SPEC §1B): the transient status-bar slice behind a
    /// [`Store`](crate::tui::state::Store) — cost + mcp-status + context +
    /// toast. The protocol bridge routes its writes through
    /// [`set_transient`](App::set_transient) so an identical-value write is a
    /// no-op (the `Object.is` guard) and the render loop's redraw-skip can
    /// consult [`transient_revision`](App::transient_revision). The four
    /// public fields above (`cost`, `mcp_status`, `context`, `toast`,
    /// `toast_at`) stay the canonical render-read surface — every existing
    /// reader is untouched (the §10 risk-2 conservative migration; the bulk
    /// `App`/`SessionView` migration is v0.9.4). A `Store::set` subscriber
    /// bumps `transient_revision`, the cheap u64 the loop diffs each tick.
    pub transient: crate::tui::state::Store<crate::tui::state::TransientSlice>,
    /// v0.9.3 — 30s done-glow fader.
    pub agent_glow: crate::tui::agents::glow::GlowFader,
    /// v0.9.3 — push/pop surface stack for AgentNav/AgentTranscript.
    pub surface_stack: Vec<crate::tui::surfaces::SurfaceStackEntry>,
    /// v0.9.3 — per-agent last-event-at for stale-watchdog (Sec-H2 + H2 closure).
    /// Doubles as the off-band stale flag: `is_stale = (now - last_event) > 10min`
    /// is computed on demand by StaleWatchdog + render path (keeps SubAgentView
    /// FROZEN per app.rs:582-584).
    pub agent_last_event: std::collections::HashMap<String, std::time::Instant>,
    /// v0.9.3 — active agent_id while AgentTranscript surface is open.
    /// Set by AgentNav handler BEFORE returning Switch; cleared on Pop.
    pub active_agent_transcript_id: Option<String>,
    /// v0.9.3 — one-shot onboarding state for first-spawn hint.
    pub onboarding_state: crate::tui::onboarding::OnboardingState,
    /// v0.9.2 W10: monotonic revision bumped once per value-changing
    /// `transient` write (a no-op `set` does NOT bump it). A `Store`
    /// subscriber installed in [`App::with_initial_surface`] increments this
    /// atomic on every change; the render loop reads it via
    /// [`transient_revision`](App::transient_revision) and compares against
    /// the value it last drew to decide whether the transient row needs a
    /// fresh paint at idle — the redraw-skip lever that ties into W1's
    /// `wants_tick`. `Arc<AtomicU64>` (not a plain `u64`) so the subscriber
    /// closure can own a handle while `App` stays `Send` for the
    /// `Arc<Mutex<App>>` shared with the bridge task.
    transient_rev: std::sync::Arc<std::sync::atomic::AtomicU64>,
    /// D019 — the workspace checkpoint store behind `/rewind`. `None` until
    /// the first turn captures a snapshot; constructed lazily by
    /// [`checkpoint_store`](App::checkpoint_store) under a per-session scratch
    /// directory the first time it is needed. Held as `Option` so a session
    /// that never touches a file (or never ends a turn) pays no setup cost and
    /// allocates no scratch dir.
    checkpoints: Option<CheckpointStore>,
    /// D019 — owns the per-session scratch [`tempfile::TempDir`] backing the
    /// checkpoint store so it is CLEANED UP when the `App` drops (process
    /// exit), instead of leaking a `wayland-rewind-*` directory under the
    /// system temp dir on every run. `None` until the store is first built.
    checkpoint_tempdir: Option<tempfile::TempDir>,
    /// D019 — absolute on-disk paths the agent has touched this session, in
    /// first-seen order. Populated alongside the right-rail `path_map` by the
    /// protocol bridge (the path_map keeps project-relative display nodes;
    /// this keeps the real paths the checkpoint store snapshots and restores).
    /// Deduped on insert so repeated edits to one file appear once.
    touched_files: Vec<PathBuf>,
    /// D037: the `touched_files` count at the start of the current turn, so the
    /// bridge can slice exactly THIS turn's touched files at turn end for the
    /// post-turn "Files changed" card.
    touched_files_watermark: usize,
    /// B1b — touched paths staged at `ToolRequest` (pre-approval), keyed by
    /// `call_id`, NOT yet recorded for `/rewind`. A file-touching tool's path
    /// is parked here until the tool is actually approved and starts running
    /// (`ToolRunning` promotes it into `touched_files`); a denied or cancelled
    /// tool's entry is dropped without recording, so a DENIED tool never has
    /// its path snapshotted at turn end. The display `path_map` still shows the
    /// requested file at request time — only the on-disk checkpoint record is
    /// gated on approval.
    pending_touch: HashMap<String, PathBuf>,
    /// ForgeFlows-Live Phase 2 — workflows inferred from the
    /// `"workflow:<node_id>"` `parent_call_id` prefix on relayed sub-agent
    /// events. Populated by the protocol bridge ALONGSIDE
    /// `session.sub_agents` (the SubAgents tab is unchanged); read only by
    /// the Workflows surface. Empty until a workflow runs.
    pub workflows: Vec<WorkflowView>,
}

impl App {
    /// Construct the initial app state on the onboarding surface, no
    /// overlay, empty session, default mode.
    ///
    /// This is the first-run default. The live TUI uses
    /// [`App::with_initial_surface`] instead so a returning user (one
    /// with a config already on disk) starts on the Workspace rather than
    /// being walked through onboarding again.
    pub fn new() -> Self {
        Self::with_initial_surface(SurfaceId::Onboarding)
    }

    /// Construct the initial app state on a chosen surface.
    ///
    /// The TUI host picks the surface with the first-run gate: Onboarding
    /// when no config exists yet, Workspace when one does.
    pub fn with_initial_surface(surface: SurfaceId) -> Self {
        // v0.9.2 W10: the transient slice store + a subscriber that bumps a
        // shared revision counter on every value-changing write (the no-op
        // guard means an identical-value write fires nothing, so the counter
        // only moves on a real change). The render loop diffs this counter to
        // skip redrawing the transient row when nothing changed.
        use std::sync::Arc;
        use std::sync::atomic::{AtomicU64, Ordering};
        let transient_rev = Arc::new(AtomicU64::new(0));
        let rev_for_sub = transient_rev.clone();
        let mut transient =
            crate::tui::state::Store::new(crate::tui::state::TransientSlice::default(), None);
        transient.subscribe(move |_next| {
            rev_for_sub.fetch_add(1, Ordering::Relaxed);
        });
        Self {
            surface,
            overlay: None,
            session: SessionView::default(),
            config: ConfigView::default(),
            mode: wcore_protocol::commands::SessionMode::Default,
            context: ContextView::default(),
            quit: false,
            path_map: TreeModel::default(),
            plan: None,
            cost: None,
            rail_visible: true,
            quit_armed: false,
            frame_tick: 0,
            queued_message: None,
            // D007: no config save pending at boot — set by the ConfigSurface
            // save path, consumed once by the router's per-tick rebind.
            rebind_request: RebindRequest::None,
            // M1/M2: no rebind has run at boot, so the live apply has not
            // failed — `/config` shows the honest non-degraded copy.
            config_apply_failed: false,
            mcp_status: HashMap::new(),
            // v0.9.1.3 F13: mouse capture defaults OFF so native terminal
            // 2026-05-31: capture ON by default so the scroll wheel drives
            // the transcript out of the box (the off-default read as "I can't
            // scroll"). Shift+drag still selects/copies; F4 toggles capture
            // OFF for terminals without a Shift bypass.
            mouse_capture_enabled: true,
            // v0.9.1.2 F14: one-shot force-scroll trigger; idle by default
            // (no approval is pending at boot).
            force_scroll_to_pending_approval: false,
            // v0.9.1.2 F8: empty history at session start — first ArrowUp
            // is a no-op until the user has actually submitted a prompt.
            recent_user_prompts: VecDeque::new(),
            history_cursor: None,
            // v0.9.2 S0 scaffold defaults (W1/W5/W7 fill in behavior).
            anim: AnimationClock::default(),
            toast: None,
            toast_at: None,
            reasoning_expanded: HashMap::new(),
            transient,
            // v0.9.3 S0.3 — multi-agent navigation scaffold defaults.
            agent_glow: crate::tui::agents::glow::GlowFader::default(),
            surface_stack: Vec::new(),
            agent_last_event: HashMap::new(),
            active_agent_transcript_id: None,
            onboarding_state: crate::tui::onboarding::OnboardingState::default(),
            transient_rev,
            // D019: no snapshots taken yet — the store is built lazily on the
            // first turn that ends with touched files.
            checkpoints: None,
            checkpoint_tempdir: None,
            touched_files: Vec::new(),
            touched_files_watermark: 0,
            // B1b: no tool requests in flight at boot.
            pending_touch: HashMap::new(),
            // ForgeFlows-Live Phase 2: no workflows running at boot.
            workflows: Vec::new(),
        }
    }

    /// D019 — record an absolute path the agent touched, for `/rewind`.
    ///
    /// Deduped (a file edited twice this session lists once). The protocol
    /// bridge calls this from the same `ToolRequest` arm that folds the path
    /// into the right-rail `path_map`, so the checkpoint store snapshots the
    /// real on-disk paths while the rail keeps its project-relative nodes.
    pub fn record_touched_file(&mut self, path: PathBuf) {
        if !self.touched_files.contains(&path) {
            self.touched_files.push(path);
        }
    }

    /// D019 — the files this session has touched, the snapshot set `/rewind`
    /// captures.
    pub fn touched_files(&self) -> &[PathBuf] {
        &self.touched_files
    }

    /// B1b — stage a tool's touched path at `ToolRequest` (pre-approval).
    ///
    /// The path is parked under `call_id` and is NOT yet visible to `/rewind`.
    /// It is promoted into `touched_files` only when the tool actually runs
    /// ([`promote_pending_touch`](App::promote_pending_touch)), and dropped
    /// without recording if the tool is denied/cancelled
    /// ([`drop_pending_touch`](App::drop_pending_touch)). This keeps a denied
    /// tool's path out of the turn-end checkpoint.
    pub fn stash_pending_touch(&mut self, call_id: String, path: PathBuf) {
        self.pending_touch.insert(call_id, path);
    }

    /// B1b — promote a staged path into the recorded touched-files set once the
    /// tool has been approved and started running. No-op if nothing was staged
    /// for `call_id` (a tool that touches no single file).
    pub fn promote_pending_touch(&mut self, call_id: &str) {
        if let Some(path) = self.pending_touch.remove(call_id) {
            self.record_touched_file(path);
        }
    }

    /// B1b — discard a staged path without recording it, for a tool whose
    /// approval was denied or that was cancelled before it ran. No-op if
    /// nothing was staged for `call_id`.
    pub fn drop_pending_touch(&mut self, call_id: &str) {
        self.pending_touch.remove(call_id);
    }

    /// D037: snapshot the touched-file count at turn start, so the per-turn
    /// delta can be sliced at turn end for the "Files changed" card.
    pub fn mark_touched_files_watermark(&mut self) {
        self.touched_files_watermark = self.touched_files.len();
    }

    /// D037: the touched-file count recorded at the start of the current turn.
    pub fn touched_files_watermark(&self) -> usize {
        self.touched_files_watermark
    }

    /// D019 — borrow the per-session checkpoint store, constructing it lazily
    /// under a scratch directory on first use.
    ///
    /// The scratch root is created via `tempfile` as a `0700`, random-named
    /// directory owned by the current user. We deliberately do NOT use a
    /// predictable `wayland-rewind-<pid>` path under a world-writable `/tmp`:
    /// a guessable name in shared `/tmp` lets another local user pre-create the
    /// directory or plant a crafted checkpoint, and on a `/rewind` restore the
    /// store writes blob contents back to the file paths recorded in the
    /// checkpoint metadata -- so an attacker-controlled store could redirect
    /// writes to the victim's files. An unguessable `0700` directory closes
    /// that local race. If the secure create fails we fall back to the legacy
    /// per-pid path so `/rewind` still functions rather than panicking.
    pub fn checkpoint_store(&mut self) -> &CheckpointStore {
        if self.checkpoints.is_none() {
            // Create a 0700, random-named scratch dir via tempfile and OWN the
            // handle on `App` so it is removed when the process exits, instead
            // of `.keep()`-leaking a directory per run. If the secure create
            // fails we fall back to a per-user cache dir (NOT the predictable,
            // world-writable `/tmp/wayland-rewind-<pid>` path the secure path
            // exists to avoid — that name is guessable and pre-creatable by
            // another local user, which restore would then trust).
            let td = tempfile::Builder::new()
                .prefix("wayland-rewind-")
                .tempdir()
                .ok();
            let scratch = match &td {
                Some(d) => d.path().to_path_buf(),
                None => dirs::cache_dir()
                    .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".cache"))
                    .join("wayland-core")
                    .join("rewind"),
            };
            self.checkpoint_tempdir = td;
            // The workspace boundary restore/capture confine to: the directory
            // the TUI (and therefore the agent's file tools) runs in. Files
            // outside it are never snapshotted or written back by /rewind.
            let workspace_root =
                std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            self.checkpoints = Some(CheckpointStore::new(scratch, workspace_root));
        }
        self.checkpoints
            .as_ref()
            .expect("checkpoint store just initialized above")
    }

    /// Test seam: install a checkpoint store with an explicit scratch dir and
    /// workspace root. Production derives the root from `current_dir()`, but
    /// tests must capture/restore inside a `tempdir` (mutating the process cwd
    /// would race the parallel test runner), so they inject the root here. The
    /// confinement behaviour under test is identical — only the root differs.
    #[cfg(test)]
    pub(crate) fn init_checkpoint_store_for_test(
        &mut self,
        scratch: std::path::PathBuf,
        workspace_root: std::path::PathBuf,
    ) {
        self.checkpoints = Some(CheckpointStore::new(scratch, workspace_root));
    }

    /// v0.9.2 W10: route a transient-slice mutation through the [`Store`].
    ///
    /// `updater` receives the current [`TransientSlice`] and returns the
    /// next one. The store's `Object.is` no-op guard means an
    /// identical-value write fires no subscriber and does not bump
    /// [`transient_revision`](App::transient_revision); a real change bumps
    /// it. On a real change, mirror the new slice onto the four canonical
    /// public fields (`cost`, `mcp_status`, `context`, `toast`, `toast_at`)
    /// so every existing reader keeps working unchanged — the conservative
    /// migration: the store is the dirty-tracking + demotion home, the
    /// fields stay the render-read surface.
    pub fn set_transient(
        &mut self,
        updater: impl FnOnce(&crate::tui::state::TransientSlice) -> crate::tui::state::TransientSlice,
    ) {
        // Snapshot `prev` from the CANONICAL public fields, not the store's
        // stored value. Other code paths (the status-bar toast auto-dismiss,
        // existing tests) write `app.toast` / `app.cost` directly without the
        // store; reading them here means those direct edits are honoured and
        // a dismissed toast is never resurrected by a later `..prev.clone()`.
        let canonical = crate::tui::state::TransientSlice {
            cost: self.cost.clone(),
            mcp_status: self.mcp_status.clone(),
            context: self.context,
            toast: self.toast.clone(),
            toast_at: self.toast_at,
        };
        let next = updater(&canonical);
        // v0.9.2 audit M3: realign the store's internal state with canonical
        // BEFORE the real `set`. Direct field writes (`app.cost = …`, the
        // toast auto-dismiss) bypass the store, so its internal `state` drifts
        // from canonical; the `set` no-op guard then compares `next` against a
        // STALE store value, and a `next` that happens to equal the stale
        // state but differs from canonical would short-circuit — failing to
        // bump the revision even though the canonical fields really changed
        // (a stale render). `reseed` silently overwrites the store's internal
        // state (no subscriber fires, so no spurious bump), so the subsequent
        // `set` compares `next == canonical`: a real change bumps the revision
        // exactly once, an identical one never does.
        self.transient.reseed(canonical);
        // Drive the store with the precomputed `next` so its `Object.is`
        // no-op guard runs (and the revision-bumping subscriber fires) only
        // on a real change. The closure ignores the store's own state.
        self.transient.set(|_stored| next.clone());
        // Mirror the result onto the canonical fields the renderers read.
        self.cost = next.cost;
        self.mcp_status = next.mcp_status;
        self.context = next.context;
        self.toast = next.toast;
        self.toast_at = next.toast_at;
    }

    /// v0.9.2 W10: the current transient-slice revision — bumped once per
    /// value-changing [`set_transient`](App::set_transient). The render loop
    /// compares this against the value it last drew; an unchanged revision at
    /// idle (`!wants_tick`) lets it skip the redraw (`should_redraw`).
    pub fn transient_revision(&self) -> u64 {
        self.transient_rev
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// v0.9.4 — reset all multi-agent / transcript / reasoning UI state on /new.
    ///
    /// Clears the six agent/nav/reasoning fields that accumulate per-session
    /// state. Intentionally does NOT reset `onboarding_state` — that hint fires
    /// only once per session (first-spawn) regardless of how many `/new` resets
    /// the user runs.
    pub fn reset_agents(&mut self) {
        self.agent_last_event.clear();
        self.agent_glow = crate::tui::agents::glow::GlowFader::default();
        self.surface_stack.clear();
        self.active_agent_transcript_id = None;
        self.reasoning_expanded.clear();
        // ForgeFlows-Live Phase 2: drop inferred workflows on /new alongside
        // the sub-agent state they are grouped from.
        self.workflows.clear();
        // onboarding_state: intentionally NOT reset (once-per-session first-spawn hint).
    }

    /// v0.9.2 audit M2: clear a toast once it has outlived `dwell`, THROUGH
    /// the store (`set_transient`) so the revision bumps and the redraw-skip
    /// repaints promptly. Previously the status-bar widget simply stopped
    /// drawing the toast after its dwell (`&App` can't mutate), leaving
    /// `app.toast`/`toast_at` set; at a fully-idle prompt the stale frame
    /// lingered up to ~1s (until the heartbeat) because nothing bumped the
    /// revision. Clearing it here (the loop holds `&mut App`) bumps the
    /// revision so the very next frame drops the toast. No-op when no toast
    /// is showing or the dwell has not yet elapsed. Returns `true` when a
    /// toast was actually cleared.
    pub fn dismiss_expired_toast(&mut self, dwell: std::time::Duration) -> bool {
        let expired = matches!(self.toast_at, Some(at) if at.elapsed() >= dwell);
        if !expired {
            return false;
        }
        self.set_transient(|prev| crate::tui::state::TransientSlice {
            toast: None,
            toast_at: None,
            ..prev.clone()
        });
        true
    }
}

/// v0.9.1.2 F8: Upper bound on retained user prompts. Bash defaults to
/// 500 but Bash persists; this is session-local and the ring is walked
/// linearly by an `ArrowUp` cursor, so 50 is plenty for the "edit my
/// last prompt" workflow without making the cursor walk noisy.
pub const PROMPT_HISTORY_CAP: usize = 50;

/// D009 (render-livelock): hard cap on retained completed transcript turns.
///
/// The transcript renderer (`render_turns`) rebuilds and re-wraps EVERY
/// retained turn on every frame under the App mutex. An unbounded
/// `session.turns` therefore makes the per-frame wrap cost — and the input
/// latency that queues behind it — grow without bound on a long/chatty run.
/// Capping the retained turns bounds that cost: oldest turns roll off the
/// front (they have already scrolled out of any realistic viewport), the
/// newest `TURN_HISTORY_CAP` always stay. The cap is generous enough that a
/// normal session never hits it, but a runaway spawn cannot pin the loop.
pub const TURN_HISTORY_CAP: usize = 500;

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// ─────────────────────────────────────────────────────────────────────────
// SessionView — the conversation
// ─────────────────────────────────────────────────────────────────────────

/// The live conversation state. FROZEN Wave-0 contract.
#[derive(Debug)]
pub struct SessionView {
    /// Completed conversation turns, oldest first.
    pub turns: Vec<TurnView>,
    /// Text accumulated for the assistant turn currently streaming. Empty
    /// when no stream is in flight; flushed into a `TurnView` on `StreamEnd`.
    pub streaming: String,
    /// Reasoning/thinking text accumulated for the in-flight turn, if any.
    pub thinking: String,
    /// Tool-call cards for the current turn, keyed by appearance order.
    pub tool_cards: Vec<ToolCardModel>,
    /// Sub-agents spawned by the current turn (`Spawn` tool / `SubAgentEvent`).
    pub sub_agents: Vec<SubAgentView>,
    /// True while an assistant stream is in flight (between `StreamStart`
    /// and `StreamEnd`). Drives the spinner + cancel affordance.
    pub streaming_active: bool,
    /// Per-session streaming filter that strips `<think>`/`<reasoning>`/
    /// `<thinking>` reasoning tags (incl. tags split across token-chunk
    /// boundaries) from `TextDelta` text before it reaches `streaming`.
    /// Reset on each `StreamStart` so a runaway unclosed block from one
    /// turn cannot bleed into the next (W2 C4).
    pub reasoning_filter: ReasoningFilter,
    /// W3 D3: Current streaming phase — drives the animated status widget
    /// that replaced the static "working" line. The phase is derived from
    /// protocol events (StreamStart→Thinking, TextDelta→Drafting, ToolRequest
    /// →CallingTool, etc.) so the user sees what the model is actually doing,
    /// not a single spinner that reads as hung for long turns.
    pub phase: StreamingPhase,
    /// W3 D3: When the current `phase` was entered. The streaming-status
    /// widget gates tip rotation against this — tips only appear after the
    /// phase has held for >15s, so short turns stay quiet.
    pub phase_started_at: Instant,
    /// W3 D3: When the current turn started (`StreamStart`). Drives the
    /// elapsed-time clock + deterministic verb rotation in the status widget.
    pub turn_started_at: Instant,
    /// W3 D3: When the last `TextDelta` arrived. The tick loop transitions
    /// `Drafting → WrappingUp` when this has been silent for >15s, so a
    /// model that stalls mid-stream surfaces an explicit wrapping-up label.
    pub last_delta_at: Instant,
    /// W3 D3: Running output-token count for the current turn, summed from
    /// the `usage.output_tokens` carried on `StreamEnd`. Displayed inline in
    /// the status widget as `↑ <n> tokens`. Reset to 0 on each `StreamStart`.
    pub tokens_out: u64,
    /// W3 D1: Compact-vs-full mode for the transcript's tool cards.
    /// `true` (the default) collapses each card to a single un-bordered
    /// line of `<icon> <name>(<args>) · <summary>`; `false` expands each
    /// to a bordered box with header + detail lines + footer. `Ctrl+E`
    /// toggles this for every card on screen at once — per-session
    /// global, not per-card, on purpose: noisy turns get one keystroke
    /// to read everything, not N keystrokes per card.
    pub compact_tool_output: bool,
    /// v0.9.1.2 F12: index into [`SessionView::turns`] of the in-flight
    /// assistant turn, if one is currently being built. Opened lazily by
    /// the protocol bridge on the first piece of assistant content
    /// (`TextDelta` or `ToolRequest`); cleared on `StreamEnd` (and on
    /// `Error`). `None` between turns. The bridge appends
    /// [`TurnElement::Markdown`] / [`TurnElement::ToolCard`] to the
    /// indexed turn in document order so the renderer can interleave
    /// tool cards with assistant text instead of piling them up at the
    /// end of the transcript.
    pub in_flight_turn_idx: Option<usize>,
    /// v0.9.2 W6 (SPEC §4): per-turn nonce that seeds the single verb pick
    /// for the streaming-status widget — the `useState(|| sample(pool))`
    /// equivalent. Set once at `StreamStart` so the verb is stable for the
    /// whole turn (replaces the old time-based rotation). `0` between turns;
    /// W6 sets it from the protocol bridge.
    pub turn_verb_seed: u64,
}

impl Default for SessionView {
    fn default() -> Self {
        let now = Instant::now();
        Self {
            turns: Vec::new(),
            streaming: String::new(),
            thinking: String::new(),
            tool_cards: Vec::new(),
            sub_agents: Vec::new(),
            streaming_active: false,
            reasoning_filter: ReasoningFilter::default(),
            phase: StreamingPhase::Idle,
            phase_started_at: now,
            turn_started_at: now,
            last_delta_at: now,
            tokens_out: 0,
            // W3 D1: compact-by-default — new sessions land on the dense
            // view; the user opts into full only via Ctrl+E.
            compact_tool_output: true,
            // v0.9.1.2 F12: no in-flight assistant turn between sessions.
            in_flight_turn_idx: None,
            // v0.9.2 W6: no verb seed between turns; set at StreamStart.
            turn_verb_seed: 0,
        }
    }
}

impl SessionView {
    /// v0.9.1.2 F12: drop every per-conversation collection. Used by the
    /// "start fresh" session-reset path (`/new`, `/clear`); collections
    /// the renderer reads MUST clear together — if `turns` drops while
    /// `tool_cards` keeps a card a stale [`TurnElement::ToolCard`] on a
    /// later turn could resolve to the wrong row. Streaming buffers
    /// are also wiped so an in-flight reset cancels cleanly.
    pub fn clear(&mut self) {
        self.turns.clear();
        self.tool_cards.clear();
        self.sub_agents.clear();
        self.streaming.clear();
        self.thinking.clear();
        self.streaming_active = false;
        self.in_flight_turn_idx = None;
    }

    /// D009 (render-livelock): roll completed turns off the FRONT once the
    /// transcript exceeds [`TURN_HISTORY_CAP`], so the per-frame re-wrap cost
    /// the render loop pays under the App mutex stays bounded as a long run
    /// grows. Returns the number of turns dropped (0 when under the cap).
    ///
    /// Safety of front-trimming:
    /// * Only runs when NO assistant turn is in flight (`in_flight_turn_idx`
    ///   is `None`). The in-flight index is a position INTO `turns`; dropping
    ///   from the front would shift it. Deferring the trim until the turn
    ///   completes keeps that index valid without a fragile rebase.
    /// * After trimming, any `ToolCardModel` whose `call_id` is no longer
    ///   referenced by a `TurnElement::ToolCard` on a retained turn is
    ///   dropped, so the `tool_cards` lookup Vec cannot grow unbounded behind
    ///   the trimmed transcript (it would otherwise leak orphaned cards).
    pub fn trim_history(&mut self) -> usize {
        if self.in_flight_turn_idx.is_some() {
            return 0;
        }
        if self.turns.len() <= TURN_HISTORY_CAP {
            return 0;
        }
        let dropped = self.turns.len() - TURN_HISTORY_CAP;
        self.turns.drain(0..dropped);

        // Prune tool cards no longer referenced by any retained turn. Collect
        // the referenced ids as owned `String`s first so the `tool_cards`
        // mutation below borrows nothing from `turns`.
        let referenced: std::collections::HashSet<String> = self
            .turns
            .iter()
            .flat_map(|turn| turn.elements.iter())
            .filter_map(|element| match element {
                TurnElement::ToolCard(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        self.tool_cards
            .retain(|card| referenced.contains(&card.call_id));
        dropped
    }
}

/// Streaming phase for the in-flight turn. LOCKED v0.9.0 W3 D3 contract —
/// no variants may be added without an orchestrator approval, per PLAN.md
/// P-H7. The status widget renders [`Self::display_label`] verbatim.
///
/// Transitions are event-driven from `protocol_bridge::apply_event`, except
/// `Drafting → WrappingUp`, which is timer-driven by the render-loop tick
/// (after >15s without a fresh `TextDelta`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamingPhase {
    /// No stream is in flight (between turns).
    Idle,
    /// `StreamStart` or `Thinking` event firing — the model is reasoning.
    Thinking,
    /// `TextDelta` is firing — the model is producing visible output.
    Drafting,
    /// `ToolRequest` emitted; payload is the tool name awaiting/queued.
    CallingTool(String),
    /// Tool execution in flight; payload is the running tool's name.
    RunningTool(String),
    /// >15s elapsed since the last `TextDelta` but no `StreamEnd` yet.
    WrappingUp,
    /// v0.9.1.2 F14: a tool call is parked waiting for the user's
    /// approval. The status widget must STOP saying "calling/running"
    /// (which implies work is happening) and tell the user that input is
    /// required. `tool` is the tool name; `pending_count` is the total
    /// number of tool cards in `AwaitingApproval` (>= 1) so the widget
    /// can show a `(+N more pending)` tail when a batch has stacked up.
    AwaitingApproval {
        /// Name of the tool whose call is parked.
        tool: String,
        /// Total tool cards currently awaiting approval (>= 1).
        pending_count: usize,
    },
}

impl StreamingPhase {
    /// The short label the status widget renders for the current phase.
    /// Stable strings — assertions in the regression test pin them.
    pub fn display_label(&self) -> String {
        match self {
            Self::Idle => "idle".into(),
            Self::Thinking => "thinking".into(),
            Self::Drafting => "drafting reply".into(),
            Self::CallingTool(name) => format!("calling {name}"),
            Self::RunningTool(name) => format!("running {name}"),
            Self::WrappingUp => "wrapping up".into(),
            Self::AwaitingApproval { tool, .. } => format!("awaiting approval: {tool}"),
        }
    }
}

impl SessionView {
    /// Timer-driven phase tick — called once per render frame by the TUI
    /// loop. Transitions `Drafting → WrappingUp` after the configured
    /// silence threshold (15s) so a stream that stalls mid-reply surfaces
    /// an explicit "wrapping up" hint instead of an indefinite "drafting
    /// reply" line. The reverse transition (back to Drafting) happens
    /// event-driven when the next `TextDelta` fires.
    pub fn tick_streaming_phase(&mut self, now: Instant) {
        const WRAP_UP_AFTER: std::time::Duration = std::time::Duration::from_secs(15);
        if self.phase == StreamingPhase::Drafting
            && now.duration_since(self.last_delta_at) > WRAP_UP_AFTER
        {
            self.phase = StreamingPhase::WrappingUp;
            self.phase_started_at = now;
        }
    }
}

/// One completed conversation turn.
///
/// Wave-0 stored the turn body as a flat `text: String`. v0.9.0 TUI-V1 W1
/// replaces that with `elements: Vec<TurnElement>` so W2 can render
/// markdown and so a turn can carry typed pieces (thinking, sources) the
/// renderer walks per element. The `text()` accessor preserves the old
/// flat-string semantics as a compile bridge for readers that have not
/// migrated to per-element rendering yet.
#[derive(Debug, Clone)]
pub struct TurnView {
    /// Who produced this turn.
    pub role: TurnRole,
    /// The turn's typed body, walked in order by the renderer.
    pub elements: Vec<TurnElement>,
}

impl TurnView {
    /// Build a new, empty turn with the given role. Elements are appended
    /// by the protocol bridge as the stream produces them (W2 wires this).
    pub fn new(role: TurnRole) -> Self {
        Self {
            role,
            elements: Vec::new(),
        }
    }

    /// Compile bridge for unmigrated readers — concatenate all
    /// `Markdown` elements into a single string, joined by `\n` between
    /// elements (so two `Markdown` pieces are visibly separated, matching
    /// the streamed-then-flushed semantics of the old `turn.text`).
    ///
    /// `Thinking` and `Sources` are intentionally omitted — they were
    /// never part of the old `text` field, and a reader that wants them
    /// must walk `elements` directly. W2+ readers walk `elements` per
    /// variant; this accessor exists only so the W1 commit compiles
    /// without rewriting every reader at once.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for e in &self.elements {
            if let TurnElement::Markdown(s) = e {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(s);
            }
        }
        out
    }
}

/// The author of a conversation turn. FROZEN Wave-0 contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnRole {
    /// A message the user sent.
    User,
    /// A message the assistant produced.
    Assistant,
    /// A system / engine notice (errors, info banners).
    System,
}

/// A spawned sub-agent's live state. FROZEN Wave-0 contract.
#[derive(Debug, Clone)]
pub struct SubAgentView {
    /// Stable identifier for the sub-agent task.
    pub id: String,
    /// Human-readable name (e.g. the sub-agent's role/label).
    pub name: String,
    /// Whether the sub-agent is still running or finished.
    pub status: SubAgentStatus,
    /// Turns completed by this sub-agent so far.
    pub turns: usize,
    /// Tokens consumed by this sub-agent so far.
    pub tokens: u64,
    /// Recent live-feed lines from the sub-agent (most recent last).
    pub feed: Vec<String>,
}

/// Lifecycle state of a sub-agent. FROZEN Wave-0 contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubAgentStatus {
    /// The sub-agent is actively working.
    #[default]
    Running,
    /// The sub-agent finished successfully.
    Done,
    /// The sub-agent terminated with an error.
    Failed,
}

/// ForgeFlows-Live Phase 2 — a running workflow grouped from the
/// `"workflow:<node_id>"` `parent_call_id` prefix the child agents relay
/// (shipped in Phase 1). This is the drill-in unit of the Workflows tab:
/// one `WorkflowView` lists the nodes a workflow ran. Keyed by the run's
/// `workflow_id` (one view per run); node events carry only the
/// `"workflow:"` prefix (no id) and attach to the current — i.e. last
/// unfinished — run. Sequential runs in one session each get their own
/// view; a node arriving before its `WorkflowStarted` lands on a "pending"
/// view that the started event then adopts.
#[derive(Debug, Clone, Default)]
pub struct WorkflowView {
    /// Grouping key — the run's `workflow_id`, or the pending sentinel
    /// (`PENDING_WORKFLOW_KEY`) for a view created by a node event that
    /// arrived before its `WorkflowStarted`.
    pub key: String,
    /// Display name; set from the `WorkflowStarted` event when one arrives,
    /// otherwise defaults to `"Workflow"`.
    pub name: String,
    /// The workflow's nodes, in first-seen order.
    pub nodes: Vec<WorkflowNodeView>,
    /// Run-level outcome from the `WorkflowFinished` event: `None` while
    /// running, `Some(true)` done, `Some(false)` failed. Distinct from the
    /// per-node tally — a run can finish failed even if every node it
    /// dispatched reported Done (e.g. a missing `over:` pipeline key).
    pub finished: Option<bool>,
}

/// One node within a [`WorkflowView`] — a single child agent the workflow
/// ran, identified by the suffix after `"workflow:"`.
#[derive(Debug, Clone, Default)]
pub struct WorkflowNodeView {
    /// The node id — the suffix after the `"workflow:"` prefix.
    pub node_id: String,
    /// The child agent's name (the relayed `agent_name`).
    pub agent_name: String,
    /// Whether the node is still running, done, or failed. Reuses the
    /// sub-agent lifecycle enum so the bridge fold stays identical.
    pub status: SubAgentStatus,
    /// Recent live-feed lines from the node (most recent last). Unbounded,
    /// matching [`SubAgentView::feed`] — neither the bridge nor the surface
    /// caps it today.
    pub feed: Vec<String>,
    /// Tokens consumed by the node so far.
    pub tokens: u64,
}

// ─────────────────────────────────────────────────────────────────────────
// ToolCardModel — one tool call rendered as a card
// ─────────────────────────────────────────────────────────────────────────

/// A tool call rendered as a card. FROZEN Wave-0 contract.
///
/// Derived from a `ProtocolEvent::ToolRequest` (then updated by
/// `ToolRunning` / `ToolResult` / `ToolCancelled`).
#[derive(Debug, Clone)]
pub struct ToolCardModel {
    /// The engine `call_id` correlating request → result → approval.
    pub call_id: String,
    /// The tool's name (e.g. `Read`, `Edit`, `Bash`).
    pub tool_name: String,
    /// A short, human-readable preview of the tool's arguments.
    pub summary: String,
    /// The card's current lifecycle status.
    pub status: ToolCardStatus,
    /// The tool's output once it has produced one; `None` while running.
    pub output: Option<String>,
    /// An edit preview for `Edit`/`Write` tools, used to render an inline
    /// diff-as-approval card. `None` for non-edit tools.
    pub edit_preview: Option<DiffModel>,
    /// The tool's full input args, pretty-printed JSON. The approval
    /// modal renders this so the user sees exactly what is being asked.
    /// Empty until a `ToolRequest` populates it.
    pub input_pretty: String,
    /// The reason carried on the matching `ApprovalRequired`, populated
    /// when approval is required. Shown in the modal's subtitle.
    pub approval_reason: String,
    /// v0.9.2 W4 (SPEC §2 #8): the plan body captured at card creation for
    /// `ExitPlanMode`. The live `app.plan` is cleared by the same event
    /// that creates this card (`protocol_bridge.rs`), so the body MUST be
    /// snapshotted onto the card and rendered from here, not read live.
    /// `None` for every non-`ExitPlanMode` tool. W4 populates it; S0
    /// scaffolds the field.
    pub plan_body: Option<String>,
    /// Crucible Stage 4: the typed proposal card (Some only for a Crucible
    /// council approval). Rendered by CrucibleComponent; None for all other tools.
    pub crucible_plan: Option<wcore_types::crucible::CruciblePlan>,
}

/// Lifecycle status of a tool-call card. FROZEN Wave-0 contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolCardStatus {
    /// The tool call is awaiting approval.
    AwaitingApproval,
    /// The tool is executing.
    Running,
    /// The tool finished successfully.
    Ok,
    /// The tool finished with an error.
    Err,
    /// The tool call was cancelled (by the user or the engine).
    Cancelled,
}

// ─────────────────────────────────────────────────────────────────────────
// DiffModel — an Edit/Write preview
// ─────────────────────────────────────────────────────────────────────────

/// An edit preview: a path and its old/new file content. FROZEN Wave-0
/// contract. The `diff_view` widget (T0.4) computes the line diff and
/// applies syntax highlighting from this model.
#[derive(Debug, Clone)]
pub struct DiffModel {
    /// The file the edit targets.
    pub path: String,
    /// The pre-edit content (the `old_string` / prior file body).
    pub old: String,
    /// The post-edit content (the `new_string` / resulting file body).
    pub new: String,
}

// ─────────────────────────────────────────────────────────────────────────
// TreeModel — the right-rail path map
// ─────────────────────────────────────────────────────────────────────────

/// A path-map tree for the workspace right rail. FROZEN Wave-0 contract.
#[derive(Debug, Clone, Default)]
pub struct TreeModel {
    /// The root-level nodes of the tree.
    pub roots: Vec<TreeNode>,
}

/// One node in a `TreeModel` — a file or directory. FROZEN Wave-0 contract.
#[derive(Debug, Clone)]
pub struct TreeNode {
    /// The node's display label (the file or directory name).
    pub name: String,
    /// True if this node is a directory (may have children).
    pub is_dir: bool,
    /// Child nodes; always empty for files.
    pub children: Vec<TreeNode>,
}

impl TreeModel {
    /// Fold a touched file path into the tree, creating any missing
    /// directory nodes along the way. A path already present is a no-op,
    /// so repeated tool calls on the same file do not duplicate it.
    ///
    /// The path is split on `/` (the bridge normalizes `\` to `/`
    /// upstream); empty and `.`/`..` components are skipped so a path
    /// like `./src/lib.rs` folds the same as `src/lib.rs`.
    pub fn insert_path(&mut self, path: &str) {
        let components: Vec<&str> = path
            .split('/')
            .filter(|c| !c.is_empty() && *c != "." && *c != "..")
            .collect();
        if components.is_empty() {
            return;
        }
        let mut level = &mut self.roots;
        for (i, comp) in components.iter().enumerate() {
            let is_dir = i < components.len() - 1;
            // Find or create the node for this component.
            let pos = level.iter().position(|n| n.name == *comp);
            let idx = match pos {
                Some(p) => p,
                None => {
                    level.push(TreeNode {
                        name: (*comp).to_string(),
                        is_dir,
                        children: Vec::new(),
                    });
                    level.len() - 1
                }
            };
            level = &mut level[idx].children;
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// PlanView — a presented plan-mode change set
// ─────────────────────────────────────────────────────────────────────────

/// A plan presented by the engine's `EnterPlanMode` tool. Wave-2 additive
/// type. Carries the title and the raw plan body as the engine described
/// it; the plan-review surface renders this into its richer `PlanModel`.
#[derive(Debug, Clone, Default)]
pub struct PlanView {
    /// The plan's one-line title.
    pub title: String,
    /// The plan body — the engine's prose plan, one logical step per line.
    pub body: String,
}

// ─────────────────────────────────────────────────────────────────────────
// SessionCostView — session token usage + spend
// ─────────────────────────────────────────────────────────────────────────

/// Session token usage + spend, mirroring the [`ProtocolEvent::SessionCost`]
/// payload (`session_id`, `total_cost_usd`, `per_turn`). Wave-3 additive
/// type. The protocol bridge replaces this wholesale on each
/// `SessionCost` event; the diagnostics `/cost` screen renders it.
///
/// [`ProtocolEvent::SessionCost`]: wcore_protocol::events::ProtocolEvent::SessionCost
// v0.9.2 W10: `PartialEq` is additive — it lets the `TransientSlice`
// `Store` (state/mod.rs) run its `Object.is` no-op guard on the cost
// payload. `f64` is `PartialEq` (not `Eq`), so we derive `PartialEq` only.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionCostView {
    /// The session id this cost belongs to.
    pub session_id: String,
    /// Aggregate session spend in USD.
    pub total_cost_usd: f64,
    /// Per-turn cost rows, oldest first.
    pub per_turn: Vec<TurnCostView>,
}

/// One per-turn cost row in a [`SessionCostView`]. Mirrors
/// `wcore_protocol::events::TurnCost`.
// v0.9.2 W10: additive `PartialEq` so `SessionCostView` (which holds a
// `Vec<TurnCostView>`) can derive it for the transient-slice no-op guard.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnCostView {
    /// The 1-based turn index.
    pub turn: usize,
    /// The model that produced this turn.
    pub model: String,
    /// The provider id (e.g. `anthropic`).
    pub provider: String,
    /// The turn's cost in USD.
    pub cost_usd: f64,
}

// ─────────────────────────────────────────────────────────────────────────
// McpServerStatus — MCP server readiness for /doctor + right-rail Activity
// ─────────────────────────────────────────────────────────────────────────

/// MCP server lifecycle status, written by the protocol bridge on every
/// [`wcore_protocol::events::ProtocolEvent::McpReady`] event and read by
/// the diagnostics `/doctor` screen and the right-rail Activity panel.
///
/// v0.9.1 W1-B: replaces the per-event system-turn that was spamming the
/// transcript with "MCP server X ready — N tool(s)" lines. The status
/// belongs in the diagnostics layer, not the conversation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum McpServerStatus {
    /// The server registered successfully and is exposing `tool_count`
    /// tools to the engine.
    Ready {
        /// Number of tools the server advertised in its `McpReady` event.
        tool_count: usize,
    },
    /// The server's connect attempt failed cleanly (transport / handshake);
    /// `reason` is the preserved cause, surfaced in `/doctor`.
    Failed {
        /// Human-readable failure cause from the connect attempt.
        reason: String,
    },
    /// The server's connect attempt exceeded its per-server budget.
    TimedOut,
    /// The server was skipped by a pre-connect gate (e.g. unreachable
    /// command); reason explains why. Rendered as a skipped (⊘) row.
    Skipped { reason: String },
}

// ─────────────────────────────────────────────────────────────────────────
// ConfigView — a small snapshot of the resolved engine Config
// ─────────────────────────────────────────────────────────────────────────

/// A minimal snapshot of the resolved engine `Config`. FROZEN Wave-0
/// contract.
///
/// Deliberately NOT a re-model of `wcore_config::config::Config` — it
/// carries only the few fields the status bar and config surface need.
/// Strings (not the real enum types) keep this view decoupled from the
/// config crate's internal type evolution.
#[derive(Debug, Default, Clone)]
pub struct ConfigView {
    /// The active provider's display label (e.g. `anthropic`).
    pub provider: String,
    /// The active model identifier.
    pub model: String,
    /// Whether prompt caching is enabled.
    pub prompt_caching: bool,
    /// Whether long-term memory is enabled.
    pub memory_enabled: bool,
    /// The runaway-guard turn ceiling (`[default] max_turns`). `None` = no
    /// configured cap; the config surface shows its display default. Carried
    /// so `/config` seeds + persists the real value instead of a placeholder.
    pub max_turns: Option<usize>,
    /// The compaction level as a lowercase string (`off` / `safe` / `full`),
    /// kept as a String so this view stays decoupled from `wcore_compact`.
    pub compaction: String,
    /// The default approval posture as a wire string (`default` /
    /// `auto-edit` / `force`), kept as a String so the view stays decoupled
    /// from the config/protocol enum types.
    pub approval: String,
    /// Whether plan-first is enabled (`[plan] plan_first`) — the agent is
    /// nudged to plan before large/risky changes.
    pub plan_first: bool,
    /// `--force` (`--yolo`, `--dangerously-skip-permissions`) is active:
    /// every tool call is auto-approved by the engine's approval manager
    /// (`SessionMode::Force`). The status bar appends a `· FORCE` badge
    /// so the mode is impossible to miss.
    pub force: bool,
    /// The active provider's resolved `ProviderCompat` cost-per-token
    /// values (`cost_per_input_token`, `cost_per_output_token`,
    /// `cost_per_cache_read_token`, `cost_per_cache_write_token`). `None`
    /// for each field means "no override set" — the cost meter falls back
    /// to the provider preset. Carried so the Config surface's Expert tier
    /// seeds + persists the real pricing values instead of placeholders.
    pub compat_costs: CompatCosts,
    /// `[tools] auto_approve` — every tool call is auto-approved without a
    /// per-call prompt. Surfaced by the Essentials Tools row.
    pub tools_auto_approve: bool,
    /// `[tools] allow_list` — the pre-approved tools. The Essentials Tools
    /// row shows the count (`.len()`); the Advanced list editor (S7) edits the
    /// entries directly.
    pub tools_allow_list: Vec<String>,
    /// `[tools] verify_edits` — re-read files after Write/Edit and feed a
    /// verification-failed note back into the next turn.
    pub tools_verify_edits: bool,
    /// `[budget] max_cost_usd` — the per-session spend cap, or `None` for no
    /// cap. The Essentials Wallet row shows + edits this; real session spend
    /// comes from `App::cost` (never fabricated).
    pub budget_max_cost_usd: Option<f64>,
    /// `[budget] max_wall_time_secs` — the runaway wall-clock guard, or
    /// `None`. Shown on the Safety row alongside the turn ceiling.
    pub budget_max_wall_secs: Option<u64>,
    /// `[observability] structured_traces` — emit structured trace spans.
    /// Advanced-tier toggle.
    pub obs_structured_traces: bool,
    /// `[observability] online_evolution` — the GEPA online-evolution loop.
    pub obs_online_evolution: bool,
    /// `[observability] workflow_live_mode` — live workflow drill-in.
    pub obs_workflow_live: bool,
    /// `[storage.credentials] backend` as a lowercase tag (`plaintext` /
    /// `keyring` / `encrypted-file`). The Advanced radio cycles plaintext↔
    /// keyring; `encrypted-file` (two configured paths) is shown read-only so
    /// the radio never clobbers the path layout.
    pub storage_backend: String,
    /// `[security] enabled` — the egress network guard. Advanced-tier toggle.
    pub security_egress_enabled: bool,
    /// `[security] egress_allow` — operator-curated extra egress allowlist
    /// entries. The Advanced list editor (S7) adds/edits/removes domains.
    pub egress_allow: Vec<String>,
    /// `[provider_chain] enabled` — wrap the primary provider in the resilient
    /// circuit-breaker + fallback chain. Advanced-tier toggle (S7).
    pub failover_enabled: bool,
    /// `[provider_chain] fallback_models` — ordered fallback model ids tried
    /// when the primary's circuit opens. The Advanced list editor (S7) edits
    /// the chain.
    pub fallback_models: Vec<String>,
}

/// The four `ProviderCompat` cost-per-token overrides surfaced by the
/// Config Expert tier. Each is `Option<f64>`: `None` = no override (the
/// provider preset stands), `Some(x)` = an explicit per-token USD price.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct CompatCosts {
    /// USD charged per input token.
    pub input: Option<f64>,
    /// USD charged per output token.
    pub output: Option<f64>,
    /// USD per token read from the prompt cache.
    pub cache_read: Option<f64>,
    /// USD per token written into the prompt cache.
    pub cache_write: Option<f64>,
}

// ─────────────────────────────────────────────────────────────────────────
// ContextView — context-window usage for the status meter
// ─────────────────────────────────────────────────────────────────────────

/// Context-window usage for the status-bar meter. FROZEN Wave-0 contract.
// v0.9.2 W10: additive `PartialEq` for the transient-slice no-op guard.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ContextView {
    /// Tokens currently occupying the context window.
    pub used_tokens: u64,
    /// The total context-window size in tokens.
    pub window_size: u64,
}

impl ContextView {
    /// Fraction of the context window in use, in `0.0..=1.0`. Returns
    /// `0.0` when `window_size` is zero (avoids a divide-by-zero).
    pub fn pct(&self) -> f64 {
        if self.window_size == 0 {
            0.0
        } else {
            (self.used_tokens as f64 / self.window_size as f64).clamp(0.0, 1.0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_app_starts_on_onboarding_with_no_overlay() {
        let app = App::new();
        assert_eq!(app.surface, SurfaceId::Onboarding);
        assert!(app.overlay.is_none());
        assert!(!app.quit);
        assert_eq!(app.mode, wcore_protocol::commands::SessionMode::Default);
        // Wave-2 additive fields start empty.
        assert!(app.path_map.roots.is_empty());
        assert!(app.plan.is_none());
        // The right rail is shown by default.
        assert!(app.rail_visible);
        // The Ctrl+C quit guard starts disarmed.
        assert!(!app.quit_armed);
    }

    #[test]
    fn turn_view_default_has_no_elements() {
        let tv = TurnView::new(TurnRole::Assistant);
        assert!(tv.elements.is_empty());
    }

    #[test]
    fn turn_view_can_append_markdown_element() {
        let mut tv = TurnView::new(TurnRole::Assistant);
        tv.elements.push(TurnElement::Markdown("hi".to_string()));
        assert_eq!(tv.elements.len(), 1);
        assert_eq!(tv.text(), "hi");
    }

    #[test]
    fn tree_model_insert_path_builds_nested_nodes() {
        let mut tree = TreeModel::default();
        tree.insert_path("crates/wcore-cli/src/main.rs");
        // One root dir `crates`.
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].name, "crates");
        assert!(tree.roots[0].is_dir);
        // Leaf is a file.
        let main = &tree.roots[0].children[0].children[0].children[0];
        assert_eq!(main.name, "main.rs");
        assert!(!main.is_dir);
    }

    #[test]
    fn tree_model_insert_path_is_idempotent_and_merges_siblings() {
        let mut tree = TreeModel::default();
        tree.insert_path("src/a.rs");
        tree.insert_path("src/a.rs"); // duplicate — no new node
        tree.insert_path("src/b.rs"); // sibling — shares the `src` dir
        assert_eq!(tree.roots.len(), 1);
        assert_eq!(tree.roots[0].name, "src");
        assert_eq!(tree.roots[0].children.len(), 2);
    }

    #[test]
    fn tree_model_insert_path_skips_dot_components() {
        let mut tree = TreeModel::default();
        tree.insert_path("./src/lib.rs");
        assert_eq!(tree.roots[0].name, "src");
    }

    #[test]
    fn context_pct_handles_zero_window() {
        let cv = ContextView {
            used_tokens: 100,
            window_size: 0,
        };
        assert_eq!(cv.pct(), 0.0);
    }

    #[test]
    fn context_pct_clamps_and_computes() {
        let half = ContextView {
            used_tokens: 1000,
            window_size: 2000,
        };
        assert!((half.pct() - 0.5).abs() < f64::EPSILON);

        let over = ContextView {
            used_tokens: 5000,
            window_size: 2000,
        };
        assert_eq!(over.pct(), 1.0);
    }

    // ── W2 / v0.9.4 — reset_agents clears all six agent/nav/reasoning fields ──

    #[test]
    fn reset_agents_clears_all_v094() {
        use crate::tui::surfaces::{SurfaceId, SurfaceStackEntry};
        use std::time::Instant;

        let mut app = App::new();

        // Populate the fields that reset_agents must clear.
        app.agent_last_event
            .insert("spawn:test".to_string(), Instant::now());
        app.agent_glow
            .record_terminal("spawn:test".to_string(), Instant::now());
        app.surface_stack.push(SurfaceStackEntry {
            id: SurfaceId::AgentNav,
            scroll_offset: 0,
        });
        app.active_agent_transcript_id = Some("spawn:test".to_string());
        app.reasoning_expanded.insert(0, true);

        // Set a sentinel on onboarding_state to verify it is NOT reset.
        app.onboarding_state.first_spawn_seen = Some(Instant::now());

        app.reset_agents();

        assert!(
            app.agent_last_event.is_empty(),
            "reset_agents must clear agent_last_event"
        );
        assert!(
            app.agent_glow.terminals.is_empty(),
            "reset_agents must clear agent_glow"
        );
        assert!(
            app.surface_stack.is_empty(),
            "reset_agents must clear surface_stack"
        );
        assert!(
            app.active_agent_transcript_id.is_none(),
            "reset_agents must clear active_agent_transcript_id"
        );
        assert!(
            app.reasoning_expanded.is_empty(),
            "reset_agents must clear reasoning_expanded"
        );
        // onboarding_state must survive the reset (once-per-session hint).
        assert!(
            app.onboarding_state.first_spawn_seen.is_some(),
            "reset_agents must NOT clear onboarding_state"
        );
    }

    // ── D009 (render-livelock) — transcript trim bounds the retained turns ──

    fn assistant_turn(text: &str) -> TurnView {
        TurnView {
            role: TurnRole::Assistant,
            elements: vec![TurnElement::Markdown(text.to_string())],
        }
    }

    #[test]
    fn trim_history_is_a_noop_under_the_cap() {
        let mut s = SessionView::default();
        for i in 0..10 {
            s.turns.push(assistant_turn(&format!("turn {i}")));
        }
        assert_eq!(s.trim_history(), 0, "under the cap nothing is dropped");
        assert_eq!(s.turns.len(), 10);
    }

    #[test]
    fn trim_history_caps_the_transcript_and_keeps_the_newest() {
        let mut s = SessionView::default();
        let total = TURN_HISTORY_CAP + 25;
        for i in 0..total {
            s.turns.push(assistant_turn(&format!("turn {i}")));
        }
        let dropped = s.trim_history();
        assert_eq!(
            dropped, 25,
            "exactly the overflow is dropped from the front"
        );
        assert_eq!(s.turns.len(), TURN_HISTORY_CAP);
        // The newest turn must survive; the oldest must be gone.
        assert_eq!(
            s.turns.last().unwrap().text(),
            format!("turn {}", total - 1),
            "the newest turn must be retained after trimming"
        );
        assert_eq!(
            s.turns.first().unwrap().text(),
            format!("turn {}", total - TURN_HISTORY_CAP),
            "the front (oldest) turns must be dropped"
        );
    }

    #[test]
    fn trim_history_defers_while_a_turn_is_in_flight() {
        // While an assistant turn is being built, `in_flight_turn_idx` points
        // INTO `turns`; trimming the front would invalidate that index, so the
        // trim must defer until the turn completes.
        let mut s = SessionView::default();
        for i in 0..(TURN_HISTORY_CAP + 5) {
            s.turns.push(assistant_turn(&format!("turn {i}")));
        }
        s.in_flight_turn_idx = Some(s.turns.len() - 1);
        assert_eq!(
            s.trim_history(),
            0,
            "trim must not run while a turn is in flight"
        );
        assert_eq!(s.turns.len(), TURN_HISTORY_CAP + 5);
    }

    #[test]
    fn trim_history_drops_orphaned_tool_cards() {
        // A tool card referenced only by a trimmed-away turn must be pruned so
        // the tool_cards lookup Vec cannot grow unbounded behind the trim.
        let mut s = SessionView::default();
        // Oldest turn owns an orphan card; it will be trimmed away.
        let mut old = TurnView::new(TurnRole::Assistant);
        old.elements
            .push(TurnElement::ToolCard("orphan".to_string()));
        s.turns.push(old);
        s.tool_cards.push(ToolCardModel {
            call_id: "orphan".to_string(),
            tool_name: "Read".to_string(),
            summary: String::new(),
            status: ToolCardStatus::Ok,
            output: None,
            edit_preview: None,
            input_pretty: String::new(),
            approval_reason: String::new(),
            plan_body: None,
            crucible_plan: None,
        });
        // Fill past the cap with plain turns so the orphan-owning turn rolls off.
        for i in 0..(TURN_HISTORY_CAP + 5) {
            s.turns.push(assistant_turn(&format!("turn {i}")));
        }
        let dropped = s.trim_history();
        assert!(dropped >= 1);
        assert!(
            !s.tool_cards.iter().any(|c| c.call_id == "orphan"),
            "a tool card whose owning turn was trimmed must be pruned"
        );
    }
}
