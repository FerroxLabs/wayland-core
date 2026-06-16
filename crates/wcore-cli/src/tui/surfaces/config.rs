//! Config surface (surface 07) — the 3-tier progressive-disclosure
//! settings screen.
//!
//! This surface presents a single full-screen settings view
//! where every setting is framed by its *consequence*, never the TOML key
//! behind it. The depth is folded into three tiers:
//!
//! - **Tier 1 — overview.** The eight high-value settings a normal user
//!   actually touches, grouped into four intent sections ("How Wayland
//!   acts", "Memory & context", ...). Always visible.
//! - **Tier 2 — section detail.** `⏎` on a section opens a per-section
//!   detail pane; for radio settings that pane is where the choice is
//!   made.
//! - **Tier 3 — expert.** `x` opens the expert pane: the 19
//!   `wcore_config::ProviderCompat` fields, each glossed in one line of
//!   plain language so the raw key is never the only label. The four
//!   Pricing (cost-per-token) fields are editable in place — `⏎` opens a
//!   buffer, `⏎` commits (persisted to `[providers.<active>].compat`),
//!   `esc` cancels; an empty buffer clears the override. The remaining
//!   message-format / routing / capability fields stay read-only.
//!
//! ## State ownership
//!
//! All settings state is *surface-local* — it lives on `ConfigSurface`,
//! not on `App`. The four connection fields are seeded from the
//! `ConfigView` snapshot on `on_enter`. Edits mutate the local
//! `SettingsModel`; `esc` reverts the whole session's unsaved edits back
//! to that seeded baseline (Krug: cheap reversibility beats a confirm
//! dialog). `save()` persists the cleanly-mappable settings (turn-cap,
//! compaction, long-term memory) to the global `config.toml` via
//! `wcore_config::config::patch_global_config`, the partial merge writer
//! that preserves every other key. Approval mode and plan-first join the
//! persisted set once their config homes land (the remaining slices).

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use crate::tui::app::App;
use crate::tui::surfaces::{Surface, SurfaceAction, SurfaceId};
use crate::tui::theme::Theme;
use crate::tui::widgets::panel;

// ─────────────────────────────────────────────────────────────────────────
// Settings model — the surface-local view of the resolved config
// ─────────────────────────────────────────────────────────────────────────

/// The three approval modes (`ux-krug-sutherland.md`: Default / Auto-edit
/// / Force). Mirrors `wcore_protocol::commands::SessionMode` but is a
/// local copy so the surface owns its own pre-save state without touching
/// `App::mode` until a save lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApprovalMode {
    /// Asks before it writes or runs anything.
    Default,
    /// Applies edits automatically; still asks before running commands.
    AutoEdit,
    /// Never asks — applies and runs everything.
    Force,
}

impl ApprovalMode {
    /// The radio options in display order.
    const ALL: [ApprovalMode; 3] = [
        ApprovalMode::Default,
        ApprovalMode::AutoEdit,
        ApprovalMode::Force,
    ];

    /// The short label shown in the radio row.
    fn label(self) -> &'static str {
        match self {
            ApprovalMode::Default => "Default",
            ApprovalMode::AutoEdit => "Auto-edit",
            ApprovalMode::Force => "Force",
        }
    }

    /// The one-line consequence gloss shown beneath the radios.
    fn consequence(self) -> &'static str {
        match self {
            ApprovalMode::Default => "Asks before it writes or runs anything.",
            ApprovalMode::AutoEdit => {
                "Applies edits on its own — still asks before it runs commands."
            }
            ApprovalMode::Force => "Never asks — applies and runs everything. Use with care.",
        }
    }

    /// Strictly parse a mode wire string into an `ApprovalMode`, returning
    /// `None` for anything unrecognised (so the caller decides what an unknown
    /// value means instead of silently downgrading to `Default`).
    ///
    /// Accepts every canonical spelling plus the documented aliases that the
    /// other wire surfaces emit, so a value never silently loses its posture
    /// on a round-trip (D033):
    /// - `default` (canonical)
    /// - `auto-edit` (config/kebab canonical) and `auto_edit`
    ///   (`SessionMode`/snake form emitted by `current_mode()`)
    /// - `force` (canonical) and `yolo` (foreign-agent alias the `/mode`
    ///   parser and protocol both accept for `Force`)
    fn parse_view_str(s: &str) -> Option<ApprovalMode> {
        match s {
            "default" => Some(ApprovalMode::Default),
            "auto-edit" | "auto_edit" => Some(ApprovalMode::AutoEdit),
            "force" | "yolo" => Some(ApprovalMode::Force),
            _ => None,
        }
    }

    /// Seed the surface from the `ConfigView` wire string. A recognised value
    /// (any canonical spelling or documented alias) maps to its mode; an empty
    /// or genuinely unknown string falls back to `Default` — the documented
    /// boot default when no approval mode is configured. This is NOT a silent
    /// downgrade of a known mode: every valid alias is accepted by
    /// `parse_view_str` above, so only an absent/garbage value reaches the
    /// fallback (D033).
    fn from_view_str(s: &str) -> ApprovalMode {
        Self::parse_view_str(s).unwrap_or(ApprovalMode::Default)
    }

    /// Map to the persisted config posture (`[default] approval_mode`).
    fn to_config(self) -> wcore_config::config::ApprovalMode {
        match self {
            ApprovalMode::Default => wcore_config::config::ApprovalMode::Default,
            ApprovalMode::AutoEdit => wcore_config::config::ApprovalMode::AutoEdit,
            ApprovalMode::Force => wcore_config::config::ApprovalMode::Force,
        }
    }
}

/// The three compaction levels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Compaction {
    /// No automatic compaction.
    Off,
    /// Folds old turns, keeps decisions.
    Safe,
    /// Aggressively summarizes the whole history.
    Full,
}

impl Compaction {
    /// The radio options in display order.
    const ALL: [Compaction; 3] = [Compaction::Off, Compaction::Safe, Compaction::Full];

    /// The short label shown in the radio row.
    fn label(self) -> &'static str {
        match self {
            Compaction::Off => "Off",
            Compaction::Safe => "Safe",
            Compaction::Full => "Full",
        }
    }

    /// The one-line consequence gloss shown beneath the radios.
    fn consequence(self) -> &'static str {
        match self {
            Compaction::Off => "Off — keeps every turn until the window fills, then stalls.",
            Compaction::Safe => "Safe — folds old turns, keeps decisions.",
            Compaction::Full => "Full — summarizes aggressively; oldest detail is lost.",
        }
    }

    /// Map to the persisted engine level (`compact.compaction`).
    fn to_level(self) -> wcore_compact::CompactionLevel {
        match self {
            Compaction::Off => wcore_compact::CompactionLevel::Off,
            Compaction::Safe => wcore_compact::CompactionLevel::Safe,
            Compaction::Full => wcore_compact::CompactionLevel::Full,
        }
    }

    /// Parse the lowercase `ConfigView` string (`off`/`safe`/`full`). An
    /// unknown or empty string falls back to `Safe` (the engine default).
    fn from_view_str(s: &str) -> Compaction {
        match s {
            "off" => Compaction::Off,
            "full" => Compaction::Full,
            _ => Compaction::Safe,
        }
    }
}

/// The eight Tier-1 settings plus their grouping. This is the surface's
/// editable state; `ConfigSurface` keeps two copies — `current` (live,
/// edited) and `baseline` (the seeded snapshot `esc` reverts to).
#[derive(Debug, Clone, PartialEq)]
struct SettingsModel {
    // CONNECTION ----------------------------------------------------------
    /// The active provider's display label.
    provider: String,
    /// The active model identifier.
    model: String,
    /// Whether a provider API key is set.
    key_set: bool,
    // HOW WAYLAND ACTS ----------------------------------------------------
    /// The approval mode radio.
    approval: ApprovalMode,
    /// Whether plan-first is enabled for big changes.
    plan_first: bool,
    /// The runaway-guard turn ceiling.
    stop_after_turns: u32,
    // MEMORY & CONTEXT ----------------------------------------------------
    /// The compaction-level radio.
    compaction: Compaction,
    /// Whether long-term cross-session memory is on.
    long_term_memory: bool,
    // EXPERT (provider tuning) --------------------------------------------
    /// The four editable `ProviderCompat` cost-per-token overrides for the
    /// active provider (Expert tier). Each `None` = "no override set".
    compat_costs: crate::tui::app::CompatCosts,
}

impl SettingsModel {
    /// Seed a model from the `ConfigView` snapshot carried on `App`.
    ///
    /// Every editable setting — connection (provider/model), turn-cap,
    /// compaction, long-term memory, approval posture and plan-first — is
    /// seeded from the resolved config so the surface shows, and persists,
    /// real values rather than placeholders.
    fn from_config_view(cv: &crate::tui::app::ConfigView) -> Self {
        Self {
            provider: if cv.provider.is_empty() {
                "not connected".to_string()
            } else {
                cv.provider.clone()
            },
            model: if cv.model.is_empty() {
                "—".to_string()
            } else {
                cv.model.clone()
            },
            key_set: !cv.provider.is_empty(),
            approval: ApprovalMode::from_view_str(&cv.approval),
            plan_first: cv.plan_first,
            // `max_turns = None` means "no configured cap" — show the mockup's
            // display default of 25 (also what the engine falls back to).
            stop_after_turns: cv.max_turns.map(|n| n as u32).unwrap_or(25),
            compaction: Compaction::from_view_str(&cv.compaction),
            long_term_memory: cv.memory_enabled,
            compat_costs: cv.compat_costs,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Tier-1 row addressing — one flat focus index over the visible rows
// ─────────────────────────────────────────────────────────────────────────

/// Every focusable Tier-1 row, in top-to-bottom display order. `↑↓` walk
/// this list; the index of the focused entry is `ConfigSurface::focus`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Row {
    /// CONNECTION · Provider.
    Provider,
    /// CONNECTION · Model.
    Model,
    /// HOW WAYLAND ACTS · Approval mode.
    Approval,
    /// HOW WAYLAND ACTS · Plan first.
    PlanFirst,
    /// HOW WAYLAND ACTS · Stop after N turns.
    StopAfter,
    /// MEMORY & CONTEXT · Compaction level.
    Compaction,
    /// MEMORY & CONTEXT · Long-term memory.
    LongTerm,
    /// The expert-mode entry row below the rule.
    Expert,
}

impl Row {
    /// Every row in display order — the full list the overview renders.
    /// Provider and Model render here as read-only connection read-outs;
    /// they are not in `FOCUSABLE` (you change them with `/provider` and
    /// `/model`), so `↑↓` skip past them and `⏎` never lands on them.
    const ALL: [Row; 8] = [
        Row::Provider,
        Row::Model,
        Row::Approval,
        Row::PlanFirst,
        Row::StopAfter,
        Row::Compaction,
        Row::LongTerm,
        Row::Expert,
    ];

    /// The rows the focus ring walks, in display order. Provider and Model
    /// are deliberately excluded: they are read-outs, not editors, so
    /// keeping them out of the ring means `⏎` is never inert on them.
    const FOCUSABLE: [Row; 6] = [
        Row::Approval,
        Row::PlanFirst,
        Row::StopAfter,
        Row::Compaction,
        Row::LongTerm,
        Row::Expert,
    ];

    /// The section heading this row belongs under, or `None` for `Expert`
    /// (which sits below the section rule).
    fn section(self) -> Option<&'static str> {
        match self {
            Row::Provider | Row::Model => Some("CONNECTION"),
            Row::Approval | Row::PlanFirst | Row::StopAfter => Some("HOW WAYLAND ACTS"),
            Row::Compaction | Row::LongTerm => Some("MEMORY & CONTEXT"),
            Row::Expert => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// The surface
// ─────────────────────────────────────────────────────────────────────────

/// Which depth tier the surface is currently showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tier {
    /// Tier 1 — the overview list of eight settings.
    Overview,
    /// Tier 2 — the detail pane for the focused row.
    Detail,
    /// Tier 3 — the expert pane (the 24 `ProviderCompat` fields).
    Expert,
    /// Tier P — Tools & Providers list (v0.9.0 W4 E1 Part A).
    Providers,
}

/// The config / settings surface — surface 07.
///
/// Holds all settings state locally: `current` is the live (possibly
/// edited) model, `baseline` is the snapshot `esc` reverts to. `focus`
/// indexes `Row::ALL` in Tier 1; `expert_focus` indexes the expert
/// fields. A text edit on the `StopAfter` row routes keystrokes through
/// `editor` (a `tui-input` `Input`) until `⏎`/`esc` commits or cancels.
pub struct ConfigSurface {
    /// The live, possibly-edited settings.
    current: SettingsModel,
    /// The last-saved baseline `esc` reverts to.
    baseline: SettingsModel,
    /// The focused Tier-1 row.
    focus: usize,
    /// The current depth tier.
    tier: Tier,
    /// The focused expert field index (Tier 3 only).
    expert_focus: usize,
    /// `Some` while a text field is being edited; the edited row plus its
    /// `tui-input` buffer. `None` when no field edit is in flight.
    editor: Option<(Row, Input)>,
    /// `Some` while an Expert-tier cost field is being edited in place: the
    /// `CostField` under edit plus its `tui-input` buffer. Mirrors the
    /// `StopAfter` `editor` machinery (same `Input` buffer, `⏎` commits via
    /// `patch_global_config`, `esc` cancels) but is keyed by the focused
    /// expert field rather than a Tier-1 `Row`. `None` when no expert edit
    /// is in flight.
    expert_editor: Option<(CostField, Input)>,
    /// True once a save has landed this session — drives the `✓ saved`
    /// indicator in the context line.
    save_pending: bool,
    /// The last save failure, if any. `Some` shows `⚠ save failed: …` in the
    /// context line; cleared on the next edit or a successful save.
    save_error: Option<String>,
    /// The focused Tools & Providers list index (Tier P only).
    providers_focus: usize,
    /// `Some` while the Tools & Providers credentials modal is open.
    credentials_modal: Option<CredentialsModal>,
    /// H1: one-shot signal that a credentials-modal save just wrote a
    /// provider key to `~/.wayland/.env`. The credentials write happens deep
    /// inside `handle_providers_key` with no `App` access; this flag lets
    /// `handle_key` (which holds `&mut App`) raise `App::rebind_request` so the
    /// router rebinds the live engine — `Config::resolve` re-reads the `.env`
    /// file, so the freshly entered key reaches the wire via `create_provider`
    /// without a restart. Consumed (reset) by `handle_key` after dispatch.
    credential_saved: bool,
}

impl Default for ConfigSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigSurface {
    /// Construct the config surface with empty-default settings. The real
    /// values are seeded from `App` in `on_enter`.
    pub fn new() -> Self {
        let model = SettingsModel::from_config_view(&crate::tui::app::ConfigView::default());
        Self {
            current: model.clone(),
            baseline: model,
            focus: 0,
            tier: Tier::Overview,
            expert_focus: 0,
            editor: None,
            expert_editor: None,
            save_pending: false,
            save_error: None,
            providers_focus: 0,
            credentials_modal: None,
            credential_saved: false,
        }
    }

    /// True if `current` has un-saved edits relative to `baseline`.
    fn is_dirty(&self) -> bool {
        self.current != self.baseline
    }

    /// The currently focused Tier-1 row. `focus` indexes `Row::FOCUSABLE`,
    /// so Provider/Model (read-out rows outside the ring) are never focused.
    fn focused_row(&self) -> Row {
        Row::FOCUSABLE[self.focus.min(Row::FOCUSABLE.len() - 1)]
    }

    /// Revert every unsaved edit back to the baseline and drop any
    /// in-flight text edit. Returns `true` if anything actually changed.
    fn revert(&mut self) -> bool {
        let was_dirty = self.is_dirty() || self.editor.is_some() || self.expert_editor.is_some();
        self.current = self.baseline.clone();
        self.editor = None;
        self.expert_editor = None;
        was_dirty
    }

    /// Persist the current edits to the global `config.toml`, then promote
    /// `current` into `baseline` (so `esc` no longer reverts them). On a
    /// write failure the edits stay dirty and `save_error` carries the
    /// reason for the context line — nothing is silently dropped.
    fn save(&mut self) {
        if !self.is_dirty() {
            return;
        }
        match self.persist_to_disk() {
            Ok(()) => {
                self.baseline = self.current.clone();
                self.save_pending = true;
                self.save_error = None;
            }
            Err(e) => self.save_error = Some(e),
        }
    }

    /// Write the editable settings into the global `config.toml` via the
    /// partial merge writer (every other key — providers, MCP, hooks — is
    /// preserved). Returns a display error string on failure.
    fn persist_to_disk(&self) -> Result<(), String> {
        let max_turns = Some(self.current.stop_after_turns as usize);
        let compaction = self.current.compaction.to_level();
        let memory_enabled = self.current.long_term_memory;
        let approval_mode = self.current.approval.to_config();
        let plan_first = self.current.plan_first;
        wcore_config::config::patch_global_config(|f| {
            f.default.max_turns = max_turns;
            f.default.approval_mode = approval_mode;
            f.compact.compaction = compaction;
            // `ConfigFile.memory` is `Option` (F2: presence-aware merge). Saving
            // the toggle from the Config tab is an explicit opt-in/out, so
            // materialize the `[memory]` table (defaults) and set `enabled`.
            f.memory
                .get_or_insert_with(wcore_config::config::MemoryConfig::default)
                .enabled = memory_enabled;
            f.plan.plan_first = plan_first;
        })
        .map(|_| ())
        .map_err(|e| format!("{e:#}"))
    }

    // ── Tier-1 input ────────────────────────────────────────────────────

    /// Move focus up/down the focus ring (`Row::FOCUSABLE`), wrapping at the
    /// ends. Provider/Model are not in the ring, so `↑↓` step past them.
    fn move_focus(&mut self, delta: isize) {
        let len = Row::FOCUSABLE.len() as isize;
        let next = (self.focus as isize + delta).rem_euclid(len);
        self.focus = next as usize;
    }

    /// `space`/`↓`/`j` on a radio/toggle row: cycle to the next choice. Text
    /// rows and the navigation rows ignore it.
    fn toggle_focused(&mut self) {
        self.cycle_focused(1);
    }

    /// `↑`/`k` on a radio/toggle row: cycle to the previous choice. The footer
    /// advertises "↑↓ choose", so `↑` must move backward through the 3-state
    /// radios rather than mirror `↓`.
    fn toggle_focused_back(&mut self) {
        self.cycle_focused(-1);
    }

    /// Cycle the focused radio/toggle row by `delta` choices (`+1` forward,
    /// `-1` backward), wrapping at both ends. 2-state bool rows flip either
    /// way. Text and navigation rows are inert.
    fn cycle_focused(&mut self, delta: isize) {
        // A fresh edit clears any stale save outcome from the context line.
        self.save_error = None;
        self.save_pending = false;
        match self.focused_row() {
            Row::Approval => {
                let len = ApprovalMode::ALL.len();
                let idx = ApprovalMode::ALL
                    .iter()
                    .position(|&m| m == self.current.approval)
                    .unwrap_or(0);
                let next = (idx as isize + delta).rem_euclid(len as isize) as usize;
                self.current.approval = ApprovalMode::ALL[next];
            }
            Row::PlanFirst => self.current.plan_first = !self.current.plan_first,
            Row::Compaction => {
                let len = Compaction::ALL.len();
                let idx = Compaction::ALL
                    .iter()
                    .position(|&c| c == self.current.compaction)
                    .unwrap_or(0);
                let next = (idx as isize + delta).rem_euclid(len as isize) as usize;
                self.current.compaction = Compaction::ALL[next];
            }
            Row::LongTerm => self.current.long_term_memory = !self.current.long_term_memory,
            // Text / navigation rows: cycling is inert. Provider/Model are
            // not in the focus ring, so they never reach this match.
            Row::Provider | Row::Model | Row::StopAfter | Row::Expert => {}
        }
    }

    /// `⏎` on the focused row: open Tier 2, the expert tier, or begin a
    /// text edit, depending on the row.
    fn enter_focused(&mut self) {
        match self.focused_row() {
            Row::Expert => self.tier = Tier::Expert,
            Row::StopAfter => {
                // Begin an in-place numeric text edit.
                let input = Input::new(self.current.stop_after_turns.to_string());
                self.editor = Some((Row::StopAfter, input));
            }
            // Provider/Model are read-out rows, not editors — you change
            // them with `/provider` and `/model`. They are not in the focus
            // ring, so `⏎` never reaches them; this arm only guards against a
            // future ring change re-admitting them.
            Row::Provider | Row::Model => {}
            // Every other row opens its section-detail pane.
            _ => self.tier = Tier::Detail,
        }
    }

    /// Commit the in-flight text edit (`⏎`). Parses the buffer; an
    /// unparseable / zero value is rejected and the edit is dropped
    /// without changing the setting.
    fn commit_edit(&mut self) {
        if let Some((Row::StopAfter, input)) = self.editor.take()
            && let Ok(n) = input.value().trim().parse::<u32>()
            && n > 0
        {
            self.current.stop_after_turns = n;
        }
    }

    // ── Tier-3 (expert) input ───────────────────────────────────────────

    /// Move the expert-field selection up/down, wrapping.
    fn move_expert(&mut self, delta: isize) {
        let len = EXPERT_FIELDS.len() as isize;
        let next = (self.expert_focus as isize + delta).rem_euclid(len);
        self.expert_focus = next as usize;
    }

    /// Begin an in-place edit of the focused Expert field, if it is one of
    /// the editable cost fields. Seeds the buffer with the current value (or
    /// empty when the override is unset) so `⏎` either keeps or replaces it.
    /// Non-cost fields are inert (read-only), matching the footer promise.
    fn enter_expert(&mut self) {
        if let Some(field) = CostField::for_index(self.expert_focus) {
            // A fresh edit clears any stale save outcome from the context line.
            self.save_error = None;
            self.save_pending = false;
            let seed = field
                .value(&self.current.compat_costs)
                .map(format_cost)
                .unwrap_or_default();
            self.expert_editor = Some((field, Input::new(seed)));
        }
    }

    /// Commit the in-flight Expert cost edit (`⏎`). An empty buffer clears
    /// the override (`None`); a parseable non-negative number sets it. An
    /// unparseable / negative value is rejected and the edit is dropped
    /// without changing the setting. On a real change the new value is
    /// persisted to `[providers.<active>].compat` via `patch_global_config`.
    fn commit_expert_edit(&mut self) {
        let Some((field, input)) = self.expert_editor.take() else {
            return;
        };
        let trimmed = input.value().trim().to_string();
        let new_value = if trimmed.is_empty() {
            Some(None)
        } else {
            match trimmed.parse::<f64>() {
                Ok(n) if n >= 0.0 && n.is_finite() => Some(Some(n)),
                // Unparseable / negative / non-finite: reject, leave unchanged.
                _ => None,
            }
        };
        if let Some(value) = new_value
            && field.value(&self.current.compat_costs) != value
        {
            // Only persist on a real change — an unchanged commit (e.g. an
            // empty buffer over an already-unset field) writes nothing and
            // raises no save indicator, mirroring the Tier-1 `is_dirty` guard.
            field.set(&mut self.current.compat_costs, value);
            self.save_expert();
        }
    }

    /// Persist the current Expert cost overrides to the active provider's
    /// `[providers.<provider>].compat` table via the partial-merge writer.
    /// On success advances the baseline (so the D007 rebind seam fires) and
    /// flips the `✓ saved` indicator; on failure records the error string.
    fn save_expert(&mut self) {
        match self.persist_expert_to_disk() {
            Ok(()) => {
                self.baseline = self.current.clone();
                self.save_pending = true;
                self.save_error = None;
            }
            Err(e) => self.save_error = Some(e),
        }
    }

    /// Write the four cost overrides into `[providers.<active>].compat` in
    /// the global `config.toml`. Every other key (and every other compat
    /// field) is preserved by the partial-merge writer. Returns a display
    /// error string on failure.
    fn persist_expert_to_disk(&self) -> Result<(), String> {
        let provider = self.current.provider.clone();
        let costs = self.current.compat_costs;
        wcore_config::config::patch_global_config(|f| {
            let entry = f.providers.entry(provider).or_default();
            let compat = entry
                .compat
                .get_or_insert_with(wcore_config::compat::ProviderCompat::default);
            compat.cost_per_input_token = costs.input;
            compat.cost_per_output_token = costs.output;
            compat.cost_per_cache_read_token = costs.cache_read;
            compat.cost_per_cache_write_token = costs.cache_write;
        })
        .map(|_| ())
        .map_err(|e| format!("{e:#}"))
    }
}

/// Format a cost-per-token value for display / edit-buffer seeding. Uses
/// `{}` so an exact value like `0.000003` round-trips through the buffer
/// rather than being truncated by a fixed precision.
fn format_cost(v: f64) -> String {
    format!("{v}")
}

/// The four editable `ProviderCompat` cost-per-token fields surfaced in the
/// Expert tier, identified by their index into [`EXPERT_FIELDS`]. Each maps
/// to one `Option<f64>` on [`crate::tui::app::CompatCosts`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CostField {
    /// `cost_per_input_token` — `EXPERT_FIELDS[15]`.
    Input,
    /// `cost_per_output_token` — `EXPERT_FIELDS[16]`.
    Output,
    /// `cost_per_cache_read_token` — `EXPERT_FIELDS[17]`.
    CacheRead,
    /// `cost_per_cache_write_token` — `EXPERT_FIELDS[18]`.
    CacheWrite,
}

impl CostField {
    /// The editable cost field at this `EXPERT_FIELDS` index, or `None` if
    /// the index names a read-only field (message-format / routing /
    /// capability). The four Pricing rows are the trailing entries 15..=18.
    fn for_index(idx: usize) -> Option<CostField> {
        match idx {
            15 => Some(CostField::Input),
            16 => Some(CostField::Output),
            17 => Some(CostField::CacheRead),
            18 => Some(CostField::CacheWrite),
            _ => None,
        }
    }

    /// Read this field's current value off the cost overrides.
    fn value(self, costs: &crate::tui::app::CompatCosts) -> Option<f64> {
        match self {
            CostField::Input => costs.input,
            CostField::Output => costs.output,
            CostField::CacheRead => costs.cache_read,
            CostField::CacheWrite => costs.cache_write,
        }
    }

    /// Write `value` into this field on the cost overrides.
    fn set(self, costs: &mut crate::tui::app::CompatCosts, value: Option<f64>) {
        match self {
            CostField::Input => costs.input = value,
            CostField::Output => costs.output = value,
            CostField::CacheRead => costs.cache_read = value,
            CostField::CacheWrite => costs.cache_write = value,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Expert tier — the 24 ProviderCompat fields, each glossed
// ─────────────────────────────────────────────────────────────────────────

/// One expert (`ProviderCompat`) field: its raw key, a plain-language
/// gloss, and the group it belongs to. The gloss — never the raw key
/// alone — is what the user reads (`ux-krug-sutherland.md`: consequence,
/// not mechanism).
struct ExpertField {
    /// The `ProviderCompat` group heading.
    group: &'static str,
    /// The raw config key (shown dimmed, secondary to the gloss).
    key: &'static str,
    /// The one-line plain-language gloss.
    gloss: &'static str,
}

/// The 19 real `wcore_config::ProviderCompat` fields, grouped, each glossed
/// in plain language. Order matches the struct in `crates/wcore-config/
/// src/compat.rs` — every key here is a field that actually exists on
/// `ProviderCompat`. Groups follow `ux-krug-sutherland.md` §Task 2's expert
/// sketch: "Message format", "Pricing", "Capabilities", plus "Routing".
const EXPERT_FIELDS: [ExpertField; 19] = [
    // ── Message format ──────────────────────────────────────────────────
    ExpertField {
        group: "Message format",
        key: "merge_assistant_messages",
        gloss: "Combine back-to-back AI messages — required by OpenAI.",
    },
    ExpertField {
        group: "Message format",
        key: "clean_orphan_tool_calls",
        gloss: "Drop tool calls that never got a result — keeps OpenAI happy.",
    },
    ExpertField {
        group: "Message format",
        key: "dedup_tool_results",
        gloss: "Keep only the last result when a tool reports twice.",
    },
    ExpertField {
        group: "Message format",
        key: "ensure_alternation",
        gloss: "Force user/AI turns to alternate — Anthropic requires it.",
    },
    ExpertField {
        group: "Message format",
        key: "merge_same_role",
        gloss: "Fuse adjacent same-speaker messages into one.",
    },
    ExpertField {
        group: "Message format",
        key: "auto_tool_id",
        gloss: "Invent a tool-call ID when the model omits one.",
    },
    ExpertField {
        group: "Message format",
        key: "strip_patterns",
        gloss: "Text snippets scrubbed from history before it is sent.",
    },
    ExpertField {
        group: "Message format",
        key: "sanitize_schema",
        gloss: "Simplify tool schemas for strict providers like Bedrock.",
    },
    // ── Routing ─────────────────────────────────────────────────────────
    ExpertField {
        group: "Routing",
        key: "max_tokens_field",
        gloss: "Which request field carries the token cap.",
    },
    ExpertField {
        group: "Routing",
        key: "api_path",
        gloss: "URL path appended to the base URL for chat calls.",
    },
    ExpertField {
        group: "Routing",
        key: "provider_type",
        gloss: "Provider identity used for cost and trace attribution.",
    },
    // ── Capabilities ────────────────────────────────────────────────────
    ExpertField {
        group: "Capabilities",
        key: "supports_thinking",
        gloss: "Allow extended reasoning blocks (Anthropic-style).",
    },
    ExpertField {
        group: "Capabilities",
        key: "supports_effort",
        gloss: "Allow a reasoning-effort dial (OpenAI-style).",
    },
    ExpertField {
        group: "Capabilities",
        key: "effort_levels",
        gloss: "The effort steps offered when effort is supported.",
    },
    ExpertField {
        group: "Capabilities",
        key: "cache_message_breakpoints",
        gloss: "Place an extra prompt-cache marker to raise hit rate.",
    },
    // ── Pricing ─────────────────────────────────────────────────────────
    ExpertField {
        group: "Pricing",
        key: "cost_per_input_token",
        gloss: "USD charged per input token — used for the cost meter.",
    },
    ExpertField {
        group: "Pricing",
        key: "cost_per_output_token",
        gloss: "USD charged per output token.",
    },
    ExpertField {
        group: "Pricing",
        key: "cost_per_cache_read_token",
        gloss: "USD per token read from the prompt cache (cheaper).",
    },
    ExpertField {
        group: "Pricing",
        key: "cost_per_cache_write_token",
        gloss: "USD per token written into the prompt cache.",
    },
];

// ─────────────────────────────────────────────────────────────────────────
// Providers tier — Tools & Providers list, status badges, credentials
// modal. v0.9.0 W4 E1 Parts A + B.
// ─────────────────────────────────────────────────────────────────────────

/// One entry in the Tools & Providers list.
///
/// `name` is the user-facing tool / provider label (e.g. `web`,
/// `vision_analyze`, `google_meet`). `category` is the section heading
/// the row is filed under. `current_backend` is the live resolver
/// output — what backend the engine would pick right now (e.g.
/// `duckduckgo (free)`, `not-configured`, `connected`). `env_vars` is
/// the list of env vars that control this entry; the credentials modal
/// edits the first one, the rest are shown as alternatives.
#[derive(Debug, Clone)]
pub(crate) struct ProviderEntry {
    /// User-facing tool / provider label.
    pub name: &'static str,
    /// Section heading: "Search", "Vision", "Audio", "Provider keys", etc.
    pub category: &'static str,
    /// Plain-language description of what this provider does.
    pub description: &'static str,
    /// The env vars that control this entry. The first one is what
    /// the credentials modal edits when "Add credentials" is chosen.
    pub env_vars: &'static [&'static str],
    /// Signup / docs URL shown in the modal.
    pub signup_url: &'static str,
    /// Whether this entry is gated as "Deferred — not yet available".
    pub deferred: bool,
}

/// Status of a provider entry, resolved at render time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderStatus {
    /// Configured + ready (env var set).
    Connected,
    /// Not configured (env var missing / empty).
    NotConfigured,
    /// OAuth tokens stored + valid (Google Meet only).
    OAuthConnected,
    /// OAuth tokens stored but expired (Google Meet only).
    OAuthExpired,
    /// Device available (voice_mode cpal probe).
    DeviceAvailable,
    /// Device unavailable (voice_mode cpal probe failed).
    DeviceUnavailable,
    /// Device not yet probed — we cannot claim it is ready without a live
    /// cpal probe, and that probe would pull `wcore-tools` into the config
    /// surface. Until the probe exists we report this honest "unknown"
    /// rather than a false "device ready" (D028).
    DeviceUnprobed,
    /// Deferred to a future version.
    Deferred,
}

impl ProviderStatus {
    /// The status badge glyph + label, in plain text.
    pub fn label(self) -> &'static str {
        match self {
            ProviderStatus::Connected => "✓ connected",
            ProviderStatus::NotConfigured => "⚠ not configured",
            ProviderStatus::OAuthConnected => "✓ oauth connected",
            ProviderStatus::OAuthExpired => "⚠ oauth token expired",
            ProviderStatus::DeviceAvailable => "✓ device ready",
            ProviderStatus::DeviceUnavailable => "⚠ no audio device",
            ProviderStatus::DeviceUnprobed => "· device not probed",
            ProviderStatus::Deferred => "· not yet available",
        }
    }

    /// True when the status indicates the entry is ready to use.
    pub fn is_ok(self) -> bool {
        matches!(
            self,
            ProviderStatus::Connected
                | ProviderStatus::OAuthConnected
                | ProviderStatus::DeviceAvailable
        )
    }
}

/// The full Tools & Providers catalog. Order matches the W4 E1 brief:
/// search → vision → audio → image → tts → channels → home → db →
/// meet → voice → provider keys.
///
/// Pure-data — every consumer (render, status resolver, modal) reads
/// from this. The `config_lists_every_env_var_keyed_provider` test
/// asserts every env var named in `tool_backends/*` is also surfaced
/// here.
pub(crate) const PROVIDER_CATALOG: &[ProviderEntry] = &[
    // ── Search ───────────────────────────────────────────────────────
    ProviderEntry {
        name: "web (tavily)",
        category: "Search",
        description: "Tavily search backend — premium, better factual recall.",
        env_vars: &["TAVILY_API_KEY"],
        signup_url: "https://tavily.com/",
        deferred: false,
    },
    ProviderEntry {
        name: "web (brave)",
        category: "Search",
        description: "Brave search backend — independent index, ~free tier.",
        env_vars: &["BRAVE_SEARCH_API_KEY"],
        signup_url: "https://brave.com/search/api/",
        deferred: false,
    },
    // ── Vision ───────────────────────────────────────────────────────
    ProviderEntry {
        name: "vision_analyze",
        category: "Vision",
        description: "Describe and analyse images. Picks Anthropic → OpenAI → Gemini.",
        env_vars: &["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GEMINI_API_KEY"],
        signup_url: "https://console.anthropic.com/",
        deferred: false,
    },
    // ── Audio ────────────────────────────────────────────────────────
    ProviderEntry {
        name: "transcribe_audio",
        category: "Audio",
        description: "Speech-to-text. Picks Groq Whisper → OpenAI Whisper.",
        env_vars: &["GROQ_API_KEY", "OPENAI_API_KEY"],
        signup_url: "https://console.groq.com/",
        deferred: false,
    },
    // ── Image generation ─────────────────────────────────────────────
    ProviderEntry {
        name: "image_generate",
        category: "Image",
        description: "Generate images. Picks OpenAI DALL-E → fal.ai → Gemini → HF → Pollinations.",
        env_vars: &[
            "OPENAI_API_KEY",
            "FAL_API_KEY",
            "GEMINI_API_KEY",
            "HF_API_KEY",
        ],
        signup_url: "https://platform.openai.com/",
        deferred: false,
    },
    // ── TTS ──────────────────────────────────────────────────────────
    ProviderEntry {
        name: "tts_speak",
        category: "Audio",
        description: "Text-to-speech. Picks OpenAI TTS → ElevenLabs.",
        env_vars: &["OPENAI_API_KEY", "ELEVENLABS_API_KEY"],
        signup_url: "https://platform.openai.com/",
        deferred: false,
    },
    // ── Channels ─────────────────────────────────────────────────────
    ProviderEntry {
        name: "discord",
        category: "Channels",
        description: "Discord bot channel — post and read messages.",
        env_vars: &["DISCORD_BOT_TOKEN"],
        signup_url: "https://discord.com/developers/applications",
        deferred: false,
    },
    // ── Home & devices ───────────────────────────────────────────────
    ProviderEntry {
        name: "homeassistant",
        category: "Home & devices",
        description: "Home Assistant REST — control IoT devices.",
        env_vars: &["HASS_URL", "HASS_TOKEN"],
        signup_url: "https://www.home-assistant.io/integrations/http/",
        deferred: false,
    },
    // ── Database ─────────────────────────────────────────────────────
    ProviderEntry {
        name: "postgres_schema",
        category: "Database",
        description: "Postgres schema inspector. Picks DATABASE_URL → POSTGRES_URL → PG_CONN_STRING.",
        env_vars: &["DATABASE_URL", "POSTGRES_URL", "PG_CONN_STRING"],
        signup_url: "https://www.postgresql.org/docs/current/libpq-connect.html#LIBPQ-CONNSTRING",
        deferred: false,
    },
    // ── Meet / OAuth ─────────────────────────────────────────────────
    ProviderEntry {
        name: "google_meet",
        category: "Meet & OAuth",
        description: "Google Meet (OAuth). `/auth google-meet` starts the flow.",
        env_vars: &["GOOGLE_CLIENT_ID", "GOOGLE_CLIENT_SECRET"],
        signup_url: "https://console.cloud.google.com/apis/credentials",
        deferred: false,
    },
    ProviderEntry {
        name: "openai-chatgpt",
        category: "Meet & OAuth",
        description: "Sign in with ChatGPT (OAuth). `auth login chatgpt` starts the flow.",
        // OAuth-backed: status comes from the stored token, not an env var.
        env_vars: &[],
        signup_url: "https://chatgpt.com/",
        deferred: false,
    },
    // ── Voice ────────────────────────────────────────────────────────
    ProviderEntry {
        name: "voice_mode",
        category: "Audio",
        description: "Local microphone capture via cpal. No env var needed.",
        env_vars: &[],
        signup_url: "",
        deferred: false,
    },
    // ── Provider keys (LLM) ──────────────────────────────────────────
    ProviderEntry {
        name: "Anthropic",
        category: "Provider keys",
        description: "Anthropic — Claude models. Primary LLM provider.",
        env_vars: &["ANTHROPIC_API_KEY"],
        signup_url: "https://console.anthropic.com/",
        deferred: false,
    },
    ProviderEntry {
        name: "OpenAI",
        category: "Provider keys",
        description: "OpenAI — GPT models, DALL-E, Whisper, TTS.",
        env_vars: &["OPENAI_API_KEY"],
        signup_url: "https://platform.openai.com/api-keys",
        deferred: false,
    },
    ProviderEntry {
        name: "Gemini",
        category: "Provider keys",
        description: "Google Gemini models (text + vision + image).",
        env_vars: &["GEMINI_API_KEY"],
        signup_url: "https://aistudio.google.com/app/apikey",
        deferred: false,
    },
    ProviderEntry {
        name: "Groq",
        category: "Provider keys",
        description: "Groq — fast Whisper / LLama inference.",
        env_vars: &["GROQ_API_KEY"],
        signup_url: "https://console.groq.com/keys",
        deferred: false,
    },
    ProviderEntry {
        name: "Tavily",
        category: "Provider keys",
        description: "Tavily search API.",
        env_vars: &["TAVILY_API_KEY"],
        signup_url: "https://tavily.com/",
        deferred: false,
    },
    ProviderEntry {
        name: "Brave",
        category: "Provider keys",
        description: "Brave search API.",
        env_vars: &["BRAVE_SEARCH_API_KEY"],
        signup_url: "https://brave.com/search/api/",
        deferred: false,
    },
    ProviderEntry {
        name: "fal.ai",
        category: "Provider keys",
        description: "fal.ai image generation.",
        env_vars: &["FAL_API_KEY"],
        signup_url: "https://fal.ai/dashboard/keys",
        deferred: false,
    },
    ProviderEntry {
        name: "Hugging Face",
        category: "Provider keys",
        description: "Hugging Face inference (image, embedding).",
        env_vars: &["HF_API_KEY"],
        signup_url: "https://huggingface.co/settings/tokens",
        deferred: false,
    },
    ProviderEntry {
        name: "ElevenLabs",
        category: "Provider keys",
        description: "ElevenLabs voice synthesis.",
        env_vars: &["ELEVENLABS_API_KEY"],
        signup_url: "https://elevenlabs.io/app/settings/api-keys",
        deferred: false,
    },
    ProviderEntry {
        name: "Discord",
        category: "Provider keys",
        description: "Discord bot token (same as the discord channel above).",
        env_vars: &["DISCORD_BOT_TOKEN"],
        signup_url: "https://discord.com/developers/applications",
        deferred: false,
    },
    ProviderEntry {
        name: "Home Assistant",
        category: "Provider keys",
        description: "Home Assistant URL + long-lived token.",
        env_vars: &["HASS_URL", "HASS_TOKEN"],
        signup_url: "https://www.home-assistant.io/docs/authentication/",
        deferred: false,
    },
    // ── Deferred ─────────────────────────────────────────────────────
    ProviderEntry {
        name: "spotify",
        category: "Meet & OAuth",
        description: "Spotify OAuth — not yet available.",
        env_vars: &["SPOTIFY_CLIENT_ID", "SPOTIFY_CLIENT_SECRET"],
        signup_url: "https://developer.spotify.com/dashboard",
        deferred: true,
    },
];

/// Outcome of inspecting the stored Google Meet OAuth token file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GoogleMeetTokenStatus {
    /// No token file on disk (or unreadable).
    Absent,
    /// Token present and not past its expiry.
    Valid,
    /// Token present but `expires_at_unix_secs` is in the past.
    Expired,
}

/// Decode the stored Google Meet token file's expiry without depending on
/// `wcore-agent`. The file is the serialised `OAuthTokens` struct; we only
/// read `expires_at_unix_secs` (unix epoch seconds, `Option<u64>`).
///
/// - File missing / unreadable / unparsable → `Absent`.
/// - No `expires_at_unix_secs` field (provider returned no `expires_in`) →
///   `Valid` (mirrors the engine's `token_is_fresh`, which treats a missing
///   expiry as fresh).
/// - Expiry in the past relative to wall-clock → `Expired`; otherwise `Valid`.
pub(crate) fn google_meet_token_status(path: &std::path::Path) -> GoogleMeetTokenStatus {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return GoogleMeetTokenStatus::Absent;
    };
    let Ok(json) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return GoogleMeetTokenStatus::Absent;
    };
    match json.get("expires_at_unix_secs").and_then(|v| v.as_u64()) {
        None => GoogleMeetTokenStatus::Valid,
        Some(exp) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if exp <= now {
                GoogleMeetTokenStatus::Expired
            } else {
                GoogleMeetTokenStatus::Valid
            }
        }
    }
}

/// Resolve the `voice_mode` device status.
///
/// With the `voice` feature OFF the cpal backend isn't even linked, so we
/// cannot claim anything about audio devices and report the honest "not
/// probed" state (which is explicitly NOT an `is_ok()` value).
///
/// With the `voice` feature ON we run the same probe the tool itself uses to
/// decide whether to expose itself — `CpalAudioRecorder::try_default()`, which
/// returns `Some` iff the default host has a default INPUT device. A present
/// device → `DeviceAvailable`; none (CI / container / SSH / headless host) →
/// `DeviceUnavailable`. This mirrors `Tool::is_available()` so the badge can
/// no longer be a permanent "device not probed" (D028).
///
/// `cpal` is pulled transitively through `wcore-agent/voice`; the cli's `voice`
/// feature re-exports it. We call the probe through
/// `wcore_agent::tool_backends::voice_mode` rather than depending on `cpal`
/// directly (which would be a cross-crate dependency change).
fn resolve_voice_mode_status() -> ProviderStatus {
    #[cfg(feature = "voice")]
    {
        match wcore_agent::tool_backends::voice_mode::CpalAudioRecorder::try_default() {
            Some(_) => ProviderStatus::DeviceAvailable,
            None => ProviderStatus::DeviceUnavailable,
        }
    }
    #[cfg(not(feature = "voice"))]
    {
        ProviderStatus::DeviceUnprobed
    }
}

/// Resolve the `openai-chatgpt` OAuth status from the stored ChatGPT token.
///
/// Uses [`wcore_agent::oauth::chatgpt_login_status`] — the same sync,
/// network-free reader the CLI `auth status` command and the `/provider` swap
/// precheck use — so the `/config` badge can't disagree with them. No token →
/// `NotConfigured`; a stored token with a past expiry → `OAuthExpired`;
/// otherwise `OAuthConnected` (a token with no recorded expiry is treated as
/// valid, mirroring the google-meet row's missing-expiry handling).
fn resolve_chatgpt_status() -> ProviderStatus {
    let status = wcore_agent::oauth::OAuthStorage::from_home()
        .ok()
        .and_then(|s| wcore_agent::oauth::chatgpt_login_status(&s).ok().flatten());
    let Some(status) = status else {
        return ProviderStatus::NotConfigured;
    };
    match status.expires_at_unix_secs {
        Some(exp) => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if exp <= now {
                ProviderStatus::OAuthExpired
            } else {
                ProviderStatus::OAuthConnected
            }
        }
        None => ProviderStatus::OAuthConnected,
    }
}

/// Resolve a provider entry's status by inspecting the env. The voice /
/// google-meet / chatgpt cases need extra probes; everything else is the
/// simple "any of these env vars is set?" check.
pub(crate) fn resolve_provider_status(entry: &ProviderEntry) -> ProviderStatus {
    if entry.deferred {
        return ProviderStatus::Deferred;
    }
    if entry.name == "voice_mode" {
        return resolve_voice_mode_status();
    }
    if entry.name == "google_meet" {
        // OAuth status is driven by the stored token file, not the client
        // env vars: a stored token is what actually authenticates the call.
        // We decode the token's `expires_at_unix_secs` (unix epoch seconds,
        // as serialised by `wcore_agent::oauth::OAuthTokens`) so an expired
        // token reaches `OAuthExpired` instead of falsely rendering "oauth
        // connected" (D030). Decoding is a plain serde_json read of a single
        // field — no `wcore-agent` dependency is needed.
        let tokens_path =
            dirs::home_dir().map(|h| h.join(".wayland").join("oauth").join("google_meet.json"));
        let token_status = tokens_path
            .as_ref()
            .map(|p| google_meet_token_status(p))
            .unwrap_or(GoogleMeetTokenStatus::Absent);
        return match token_status {
            GoogleMeetTokenStatus::Valid => ProviderStatus::OAuthConnected,
            GoogleMeetTokenStatus::Expired => ProviderStatus::OAuthExpired,
            GoogleMeetTokenStatus::Absent => ProviderStatus::NotConfigured,
        };
    }
    if entry.name == "openai-chatgpt" {
        // OAuth-backed: status is driven by the stored ChatGPT token, decoded
        // via the single-source `chatgpt_login_status` helper (sync, no
        // network/refresh). Not signed in → NotConfigured; signed in with a
        // past expiry → OAuthExpired (the next use silently refreshes, but the
        // badge should be honest); otherwise OAuthConnected.
        return resolve_chatgpt_status();
    }
    // Default: configured if every env var is present + non-empty
    // (for multi-var entries like home-assistant) OR any of them
    // (for "alternative backends" entries like vision_analyze).
    if entry.env_vars.is_empty() {
        return ProviderStatus::Connected;
    }
    // Single-var entries (provider keys, discord, tavily): set ↔ ok.
    // Multi-var "all required" entries (homeassistant, postgres needs
    // any-one, vision picks-one): we treat any-one as configured.
    let any_set = entry.env_vars.iter().any(|k| {
        std::env::var(k)
            .map(|v| !v.trim().is_empty())
            .unwrap_or(false)
    });
    // Home Assistant requires BOTH URL + token — special-case here.
    if entry.name == "homeassistant" {
        let all_set = entry.env_vars.iter().all(|k| {
            std::env::var(k)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false)
        });
        return if all_set {
            ProviderStatus::Connected
        } else {
            ProviderStatus::NotConfigured
        };
    }
    if any_set {
        ProviderStatus::Connected
    } else {
        ProviderStatus::NotConfigured
    }
}

/// Tier-2 modal — the credentials editor.
///
/// Opens when the user presses Enter on a provider row. Shows the env
/// var name, signup URL, and a `tui-input` field. `Enter` saves via
/// `wcore_config::env_file::write_env_var`; `Esc` cancels without write.
#[derive(Debug)]
pub(crate) struct CredentialsModal {
    /// Index into PROVIDER_CATALOG for the entry being edited.
    pub entry_idx: usize,
    /// Which env_vars index inside the entry we're editing (0-based).
    pub var_idx: usize,
    /// Live input buffer.
    pub input: Input,
    /// Status banner — empty until a save attempt produces a result.
    pub status: String,
    /// True when the last write succeeded (drives the success colour).
    pub last_ok: bool,
}

impl CredentialsModal {
    /// Create a fresh modal for `entry_idx`'s first env var.
    pub fn new(entry_idx: usize) -> Self {
        Self {
            entry_idx,
            var_idx: 0,
            input: Input::default(),
            status: String::new(),
            last_ok: false,
        }
    }

    /// The entry being edited.
    pub fn entry(&self) -> &'static ProviderEntry {
        &PROVIDER_CATALOG[self.entry_idx]
    }

    /// The currently-targeted env var name.
    pub fn var_name(&self) -> Option<&'static str> {
        self.entry().env_vars.get(self.var_idx).copied()
    }

    /// Attempt to save the buffer to `~/.wayland/.env`. Sets `status`
    /// + `last_ok` for the render path. Returns whether a write happened.
    pub fn save(&mut self) -> bool {
        let Some(key) = self.var_name() else {
            self.status = "This provider has no env-var-based credentials.".into();
            self.last_ok = false;
            return false;
        };
        let value = self.input.value().to_string();
        if value.trim().is_empty() {
            self.status = "Value is empty — type a credential or press esc to cancel.".into();
            self.last_ok = false;
            return false;
        }
        let env_path = match dirs::home_dir() {
            Some(h) => h.join(".wayland").join(".env"),
            None => {
                self.status = "Cannot find home directory; aborting save.".into();
                self.last_ok = false;
                return false;
            }
        };
        match wcore_config::env_file::write_env_var(&env_path, key, &value) {
            Ok(()) => {
                self.status = format!("Saved {key} to ~/.wayland/.env · applying to this session");
                self.last_ok = true;
                true
            }
            Err(e) => {
                self.status = format!("Save failed: {e}");
                self.last_ok = false;
                false
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Surface impl
// ─────────────────────────────────────────────────────────────────────────

impl Surface for ConfigSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::Config
    }

    fn on_enter(&mut self, app: &mut App) {
        // Seed both copies from the live config snapshot so the surface
        // opens reflecting the resolved engine config, and a fresh `esc`
        // baseline matches it.
        let seeded = SettingsModel::from_config_view(&app.config);
        self.current = seeded.clone();
        self.baseline = seeded;
        self.focus = 0;
        self.tier = Tier::Overview;
        self.expert_focus = 0;
        self.editor = None;
        self.expert_editor = None;
        self.providers_focus = 0;
        self.credentials_modal = None;
        self.credential_saved = false;
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        // M1/M2: the router raises `config_apply_failed` when a save's live
        // rebind could not resolve/build — the disk write landed but the
        // engine kept its prior binding. The overview context line reads this
        // to show "live apply skipped" instead of a false "now live".
        let apply_failed = app.config_apply_failed;
        match self.tier {
            Tier::Overview => self.render_overview(frame, area, theme, apply_failed),
            Tier::Detail => {
                self.render_overview(frame, area, theme, apply_failed);
                self.render_detail(frame, area, theme);
            }
            Tier::Expert => self.render_expert(frame, area, theme),
            Tier::Providers => {
                self.render_providers(frame, area, theme);
                if self.credentials_modal.is_some() {
                    self.render_credentials_modal(frame, area, theme);
                }
            }
        }
    }

    fn handle_key(&mut self, key: KeyEvent, app: &mut App) -> SurfaceAction {
        // A text edit in flight captures every key until it commits or
        // cancels — the focus state machine.
        if self.editor.is_some() {
            match key.code {
                KeyCode::Enter => self.commit_edit(),
                KeyCode::Esc => {
                    self.editor = None;
                }
                _ => {
                    if let Some((_, input)) = self.editor.as_mut() {
                        input.handle_event(&ratatui::crossterm::event::Event::Key(key));
                    }
                }
            }
            return SurfaceAction::None;
        }

        // An Expert-tier cost edit captures every key the same way. `⏎`
        // commits (and persists, which advances `baseline` — the rebind
        // check below then fires the live engine rebind); `esc` cancels.
        if self.expert_editor.is_some() {
            let baseline_before = self.baseline.clone();
            match key.code {
                KeyCode::Enter => self.commit_expert_edit(),
                KeyCode::Esc => {
                    self.expert_editor = None;
                }
                _ => {
                    if let Some((_, input)) = self.expert_editor.as_mut() {
                        input.handle_event(&ratatui::crossterm::event::Event::Key(key));
                    }
                }
            }
            if self.baseline != baseline_before && !self.is_dirty() {
                app.rebind_request = crate::tui::app::RebindRequest::Tier1Save;
            }
            return SurfaceAction::None;
        }

        // D007: a Tier-1 save lands deep inside a per-tier key handler with
        // no router/engine access. Snapshot the baseline before dispatch;
        // if the dispatch persisted a change (baseline advanced to current
        // AND nothing is left dirty), raise the typed one-shot `rebind_request`
        // signal so the router rebinds the LIVE engine on the next tick.
        let baseline_before = self.baseline.clone();
        let action = match self.tier {
            Tier::Overview => self.handle_overview_key(key),
            Tier::Detail => self.handle_detail_key(key),
            Tier::Expert => self.handle_expert_key(key),
            Tier::Providers => self.handle_providers_key(key),
        };
        if self.baseline != baseline_before && !self.is_dirty() {
            app.rebind_request = crate::tui::app::RebindRequest::Tier1Save;
        }
        // H1: a credentials-modal write also requires a rebind so the new
        // provider key reaches the wire live. The write happens inside
        // `handle_providers_key` (no `App` access); consume its one-shot flag
        // here where `&mut App` is in scope.
        if self.credential_saved {
            self.credential_saved = false;
            app.rebind_request = crate::tui::app::RebindRequest::Credential;
        }
        action
    }
}

impl ConfigSurface {
    // ── Per-tier key handling ───────────────────────────────────────────

    /// Tier-1 keys: navigate the rows, toggle, descend, or save & close.
    fn handle_overview_key(&mut self, key: KeyEvent) -> SurfaceAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_focus(-1);
                SurfaceAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_focus(1);
                SurfaceAction::None
            }
            KeyCode::Char(' ') => {
                self.toggle_focused();
                SurfaceAction::None
            }
            KeyCode::Enter => {
                self.enter_focused();
                SurfaceAction::None
            }
            KeyCode::Char('x') => {
                self.tier = Tier::Expert;
                SurfaceAction::None
            }
            KeyCode::Char('p') => {
                // v0.9.0 W4 E1 — open the Tools & Providers tier.
                self.tier = Tier::Providers;
                self.providers_focus = 0;
                self.credentials_modal = None;
                SurfaceAction::None
            }
            KeyCode::Esc => {
                // `esc` saves & closes — the footer's contract. A dirty edit
                // (e.g. toggling long-term memory) is persisted: `save`
                // advances `baseline`, which the `handle_key` seam detects to
                // raise the Tier1Save rebind so the change applies live. We
                // stay on the surface so the saved/now-live affordance shows.
                // With nothing dirty, `esc` closes back to the workspace.
                if self.is_dirty() {
                    self.save();
                    SurfaceAction::None
                } else {
                    SurfaceAction::Switch(SurfaceId::Workspace)
                }
            }
            _ => SurfaceAction::None,
        }
    }

    /// Tier-2 keys: a radio detail pane makes its choice with `space` /
    /// `↑↓`; `⏎` saves the pane's selection and `esc` reverts back to
    /// Tier 1.
    fn handle_detail_key(&mut self, key: KeyEvent) -> SurfaceAction {
        match key.code {
            KeyCode::Char(' ') | KeyCode::Down | KeyCode::Char('j') => {
                // Forward through the radio: `space`/`↓`/`j` advance the
                // focused setting to the next choice.
                self.toggle_focused();
                SurfaceAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                // Backward through the radio: `↑`/`k` step to the previous
                // choice, matching the footer's "↑↓ choose" promise.
                self.toggle_focused_back();
                SurfaceAction::None
            }
            KeyCode::Enter => {
                // `⏎` accepts the detail pane and saves the change.
                self.save();
                self.tier = Tier::Overview;
                SurfaceAction::None
            }
            KeyCode::Esc => {
                // Revert the unsaved change made inside the detail pane,
                // then return to the overview.
                self.revert();
                self.tier = Tier::Overview;
                SurfaceAction::None
            }
            _ => SurfaceAction::None,
        }
    }

    /// Tier-3 keys: scroll the expert-field list; `⏎` begins an in-place
    /// edit of the focused Pricing (cost) field (inert on read-only fields);
    /// `esc` returns to the overview. The in-flight edit itself is captured
    /// earlier in `handle_key` (the `expert_editor` state machine).
    fn handle_expert_key(&mut self, key: KeyEvent) -> SurfaceAction {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_expert(-1);
                SurfaceAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_expert(1);
                SurfaceAction::None
            }
            KeyCode::Enter => {
                // `⏎` on an editable (Pricing) field begins an in-place edit;
                // on a read-only field it is inert.
                self.enter_expert();
                SurfaceAction::None
            }
            KeyCode::Esc => {
                self.tier = Tier::Overview;
                SurfaceAction::None
            }
            _ => SurfaceAction::None,
        }
    }

    // ── Rendering: Tier 1 (overview) ────────────────────────────────────

    /// Draw the Tier-1 overview: the four sections, the eight settings,
    /// the expert-entry row, and the footer hint line.
    fn render_overview(&self, frame: &mut Frame, area: Rect, t: &Theme, apply_failed: bool) {
        let block = panel("Wayland · Settings", t);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height < 3 || inner.width < 10 {
            return;
        }

        // Split: a context line, the body, then the two-line footer.
        let [ctx_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(2),
        ])
        .areas(inner);

        // Context line: scope of the config write + the live save outcome.
        // D017: these Tier-1 rows persist to the GLOBAL config only (the writer
        // is `patch_global_config`), matching the footer promise below. The
        // old "global + project · merged" label overstated the write scope and
        // contradicted that footer. With the engine-rebind seam wired (D007),
        // a save is applied to the running session immediately — so the honest
        // status is "saved · now live", not "restart to apply": the router
        // rebinds the live engine to the new disk config on the next tick.
        //
        // M1/M2: that "now live" claim is only honest when the rebind's
        // synchronous resolve/build succeeded. When `apply_failed` is set the
        // disk write landed but the live engine kept its prior binding, so we
        // show the degraded "live apply skipped" copy instead of overclaiming.
        let (scope, scope_style) = if let Some(err) = &self.save_error {
            (
                format!("global config · ⚠ save failed: {err}"),
                Style::default().fg(t.error),
            )
        } else if self.is_dirty() {
            (
                "global config   ● unsaved changes".to_string(),
                Style::default().fg(t.warning),
            )
        } else if self.save_pending && apply_failed {
            (
                "global config   ✓ saved to disk · live apply skipped - reopen /config or restart"
                    .to_string(),
                Style::default().fg(t.warning),
            )
        } else if self.save_pending {
            (
                "global config   ✓ saved · now live".to_string(),
                Style::default().fg(t.success),
            )
        } else {
            (
                "global config".to_string(),
                Style::default().fg(t.text_muted),
            )
        };
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(scope, scope_style))),
            ctx_area,
        );

        // Body — every section + setting row. Provider/Model render here as
        // read-outs but are outside the focus ring, so highlight is keyed on
        // the focused row, not the display index.
        let focused = self.focused_row();
        let mut lines: Vec<Line> = Vec::new();
        let mut last_section: Option<&'static str> = None;
        for &row in Row::ALL.iter() {
            // Emit a section heading when the section changes.
            match row.section() {
                Some(sec) if Some(sec) != last_section => {
                    if last_section.is_some() {
                        lines.push(Line::from(""));
                    }
                    lines.push(Line::from(Span::styled(
                        sec,
                        Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD),
                    )));
                    last_section = Some(sec);
                }
                None => {
                    // The expert row sits below a horizontal rule.
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        "─".repeat(body_area.width.saturating_sub(1) as usize),
                        Style::default().fg(t.border),
                    )));
                }
                _ => {}
            }
            lines.push(self.row_line(row, row == focused, t));
            // The Approval / Compaction rows carry a consequence gloss.
            if let Some(gloss) = self.row_gloss(row) {
                lines.push(Line::from(Span::styled(
                    format!("    {gloss}"),
                    Style::default().fg(t.text_muted),
                )));
            }
        }
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), body_area);

        // Footer — the keymap + the save/undo promise.
        let hints = Line::from(vec![
            Span::styled(" ↑↓ ", Style::default().fg(t.orange)),
            Span::styled("move  ", Style::default().fg(t.text_dim)),
            Span::styled("⏎ ", Style::default().fg(t.orange)),
            Span::styled("open  ", Style::default().fg(t.text_dim)),
            Span::styled("space ", Style::default().fg(t.orange)),
            Span::styled("toggle  ", Style::default().fg(t.text_dim)),
            Span::styled("x ", Style::default().fg(t.orange)),
            Span::styled("expert  ", Style::default().fg(t.text_dim)),
            Span::styled("p ", Style::default().fg(t.orange)),
            Span::styled("providers  ", Style::default().fg(t.text_dim)),
            Span::styled("esc ", Style::default().fg(t.orange)),
            Span::styled("save & close", Style::default().fg(t.text_dim)),
        ]);
        let promise = Line::from(Span::styled(
            " esc saves & closes · changes save to your global config.toml and apply live",
            Style::default().fg(t.text_muted),
        ));
        frame.render_widget(Paragraph::new(vec![hints, promise]), footer_area);
    }

    /// Build the value line for one Tier-1 row, highlighting it if focused.
    fn row_line(&self, row: Row, focused: bool, t: &Theme) -> Line<'static> {
        let marker = if focused { "▸ " } else { "  " };
        let label_style = if focused {
            Style::default().fg(t.orange).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text)
        };
        let label = row_label(row);

        let mut spans = vec![
            Span::styled(marker.to_string(), Style::default().fg(t.orange)),
            Span::styled(format!("{label:<14}"), label_style),
        ];
        spans.extend(self.row_value_spans(row, focused, t));
        Line::from(spans)
    }

    /// The value-side spans for a row: a radio strip, a toggle, a text
    /// value, or the navigation affordance.
    fn row_value_spans(&self, row: Row, focused: bool, t: &Theme) -> Vec<Span<'static>> {
        match row {
            Row::Provider => {
                let mut v = vec![Span::styled(
                    self.current.provider.clone(),
                    Style::default().fg(t.text),
                )];
                if self.current.key_set {
                    v.push(Span::styled("   ✓ key set", Style::default().fg(t.success)));
                } else {
                    v.push(Span::styled("   ⚠ no key", Style::default().fg(t.warning)));
                }
                v.push(Span::styled(
                    "   Change with /provider",
                    Style::default().fg(t.text_muted),
                ));
                v
            }
            Row::Model => vec![
                Span::styled(self.current.model.clone(), Style::default().fg(t.text)),
                Span::styled("   Change with /model", Style::default().fg(t.text_muted)),
            ],
            Row::Approval => radio_strip(
                &ApprovalMode::ALL.map(|m| m.label()),
                ApprovalMode::ALL
                    .iter()
                    .position(|&m| m == self.current.approval)
                    .unwrap_or(0),
                t,
            ),
            Row::PlanFirst => toggle_strip(self.current.plan_first, "off", "on for big changes", t),
            Row::StopAfter => {
                // While editing, render the live `tui-input` buffer.
                if let Some((Row::StopAfter, input)) = &self.editor {
                    vec![
                        Span::styled(
                            format!("{}_", input.value()),
                            Style::default().fg(t.orange).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            "  turns  (⏎ save · esc cancel)",
                            Style::default().fg(t.text_muted),
                        ),
                    ]
                } else {
                    vec![
                        Span::styled(
                            format!("{} turns", self.current.stop_after_turns),
                            Style::default().fg(t.text),
                        ),
                        Span::styled("   ▸ edit  (runaway guard)", disclosure(focused, t)),
                    ]
                }
            }
            Row::Compaction => radio_strip(
                &Compaction::ALL.map(|c| c.label()),
                Compaction::ALL
                    .iter()
                    .position(|&c| c == self.current.compaction)
                    .unwrap_or(0),
                t,
            ),
            Row::LongTerm => toggle_strip(
                self.current.long_term_memory,
                "off",
                "remembers across sessions",
                t,
            ),
            Row::Expert => vec![Span::styled(
                "x  expert settings (provider tuning, budgets, traces)",
                if focused {
                    Style::default().fg(t.orange).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(t.text_dim)
                },
            )],
        }
    }

    /// The one-line consequence gloss shown beneath a radio row, if any.
    fn row_gloss(&self, row: Row) -> Option<&'static str> {
        match row {
            Row::Approval => Some(self.current.approval.consequence()),
            Row::Compaction => Some(self.current.compaction.consequence()),
            _ => None,
        }
    }

    // ── Rendering: Tier 2 (detail) ──────────────────────────────────────

    /// Draw the Tier-2 detail pane centred over the dimmed overview.
    fn render_detail(&self, frame: &mut Frame, area: Rect, t: &Theme) {
        let pane = centered(area, 60, 11);
        frame.render_widget(Clear, pane);
        let row = self.focused_row();
        let block = panel(&format!("Settings · {}", row_label(row)), t);
        let inner = block.inner(pane);
        frame.render_widget(block, pane);
        if inner.height < 3 {
            return;
        }

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            detail_intro(row),
            Style::default().fg(t.text_dim),
        )));
        lines.push(Line::from(""));

        // Radio rows show every choice + its gloss; other rows show the
        // value and the where-to-edit hint.
        match row {
            Row::Approval => {
                for m in ApprovalMode::ALL {
                    let on = m == self.current.approval;
                    lines.push(detail_choice(m.label(), m.consequence(), on, t));
                }
            }
            Row::Compaction => {
                for c in Compaction::ALL {
                    let on = c == self.current.compaction;
                    lines.push(detail_choice(c.label(), c.consequence(), on, t));
                }
            }
            Row::PlanFirst => {
                lines.push(detail_choice(
                    "on for big changes",
                    "Wayland drafts a plan and waits for review first.",
                    self.current.plan_first,
                    t,
                ));
                lines.push(detail_choice(
                    "off",
                    "Wayland acts immediately, no plan step.",
                    !self.current.plan_first,
                    t,
                ));
            }
            Row::LongTerm => {
                lines.push(detail_choice(
                    "on",
                    "Remembers your preferences across sessions.",
                    self.current.long_term_memory,
                    t,
                ));
                lines.push(detail_choice(
                    "off",
                    "Each session starts with a blank memory.",
                    !self.current.long_term_memory,
                    t,
                ));
            }
            Row::Provider => {
                lines.push(Line::from(Span::styled(
                    "  Change with /provider.",
                    Style::default().fg(t.text_muted),
                )));
            }
            Row::Model => {
                lines.push(Line::from(Span::styled(
                    "  Change with /model.",
                    Style::default().fg(t.text_muted),
                )));
            }
            Row::StopAfter | Row::Expert => {
                lines.push(Line::from(Span::styled(
                    "  Edit this from the overview.",
                    Style::default().fg(t.text_muted),
                )));
            }
        }

        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" space/↑↓ ", Style::default().fg(t.orange)),
            Span::styled("choose   ", Style::default().fg(t.text_dim)),
            Span::styled("⏎ ", Style::default().fg(t.orange)),
            Span::styled("save   ", Style::default().fg(t.text_dim)),
            Span::styled("esc ", Style::default().fg(t.orange)),
            Span::styled("revert", Style::default().fg(t.text_dim)),
        ]));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }

    // ── Rendering: Tier 3 (expert) ──────────────────────────────────────

    /// Draw the Tier-3 expert pane: every `ProviderCompat` field, grouped,
    /// each glossed in plain language.
    fn render_expert(&self, frame: &mut Frame, area: Rect, t: &Theme) {
        let block = panel("Settings · Expert", t);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height < 3 {
            return;
        }

        let [head_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "Provider tuning · 19 fields — each shown in plain language.",
                    Style::default().fg(t.text_dim),
                )),
                Line::from(Span::styled(
                    "These rarely need changing; defaults are correct per provider.",
                    Style::default().fg(t.text_muted),
                )),
            ]),
            head_area,
        );

        let mut lines: Vec<Line> = Vec::new();
        let mut last_group: Option<&'static str> = None;
        // The body-line index of the focused field's gloss row, so the body
        // can scroll it into view (the list is taller than the viewport).
        let mut focus_line = 0usize;
        for (idx, field) in EXPERT_FIELDS.iter().enumerate() {
            if Some(field.group) != last_group {
                if last_group.is_some() {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(
                    field.group,
                    Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD),
                )));
                last_group = Some(field.group);
            }
            let focused = idx == self.expert_focus;
            if focused {
                // Anchor the scroll on the focused field's gloss row (the
                // next push); its key/value/editor line follows.
                focus_line = lines.len();
            }
            let marker = if focused { "▸ " } else { "  " };
            let gloss_style = if focused {
                Style::default().fg(t.orange).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.text)
            };
            lines.push(Line::from(vec![
                Span::styled(marker.to_string(), Style::default().fg(t.orange)),
                Span::styled(field.gloss.to_string(), gloss_style),
            ]));
            // The raw key, dimmed — the gloss leads, the mechanism follows.
            // Editable (Pricing) fields also show their live value plus an
            // edit affordance; while one is under edit the live `tui-input`
            // buffer renders with a trailing cursor.
            let key_line = match CostField::for_index(idx) {
                Some(cost) => {
                    if let Some((editing, input)) = &self.expert_editor
                        && *editing == cost
                    {
                        Line::from(vec![
                            Span::styled(
                                format!("    {}  ", field.key),
                                Style::default().fg(t.text_muted),
                            ),
                            Span::styled(
                                format!("{}_", input.value()),
                                Style::default().fg(t.orange).add_modifier(Modifier::BOLD),
                            ),
                            Span::styled(
                                "  (⏎ save · esc cancel · empty clears)",
                                Style::default().fg(t.text_muted),
                            ),
                        ])
                    } else {
                        let shown = cost
                            .value(&self.current.compat_costs)
                            .map(format_cost)
                            .unwrap_or_else(|| "provider default".to_string());
                        let edit_style = if focused {
                            Style::default().fg(t.orange)
                        } else {
                            Style::default().fg(t.text_muted)
                        };
                        Line::from(vec![
                            Span::styled(
                                format!("    {}  ", field.key),
                                Style::default().fg(t.text_muted),
                            ),
                            Span::styled(shown, Style::default().fg(t.text)),
                            Span::styled("   ▸ edit", edit_style),
                        ])
                    }
                }
                None => Line::from(Span::styled(
                    format!("    {}", field.key),
                    Style::default().fg(t.text_muted),
                )),
            };
            lines.push(key_line);
        }
        // Scroll so the focused field (gloss + its key/value/editor line) stays
        // visible — the 19-field list is taller than the viewport. Mirrors the
        // Providers tier (Issue #16): no scroll near the top, clamped so the
        // last page never scrolls past the final row.
        let visible = body_area.height as usize;
        let total = lines.len();
        let scroll_y = (focus_line + 2)
            .saturating_sub(visible)
            .min(total.saturating_sub(visible)) as u16;
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((scroll_y, 0)),
            body_area,
        );

        // The footer advertises the edit affordance so it is not a phantom:
        // `⏎ edit` is live for the Pricing fields, inert elsewhere.
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ↑↓ ", Style::default().fg(t.orange)),
                Span::styled("move   ", Style::default().fg(t.text_dim)),
                Span::styled("⏎ ", Style::default().fg(t.orange)),
                Span::styled("edit cost   ", Style::default().fg(t.text_dim)),
                Span::styled("esc ", Style::default().fg(t.orange)),
                Span::styled("back to settings", Style::default().fg(t.text_dim)),
            ])),
            footer_area,
        );
    }

    // ── Rendering: Tier P (Tools & Providers) ───────────────────────────

    /// Handle keys in the Tools & Providers tier. When the credentials
    /// modal is open it captures every key until `Enter`/`Esc`.
    fn handle_providers_key(&mut self, key: KeyEvent) -> SurfaceAction {
        // Modal-open path: capture everything for the input field.
        if self.credentials_modal.is_some() {
            match key.code {
                KeyCode::Enter => {
                    if let Some(modal) = self.credentials_modal.as_mut() {
                        // H1: a successful credential write must rebind the
                        // live engine (so the new key takes effect without a
                        // restart). Raise the one-shot flag `handle_key`
                        // consumes into `App::rebind_request`.
                        if modal.save() {
                            self.credential_saved = true;
                        }
                    }
                    // Stay on the modal so the user can see the status
                    // message. Esc closes.
                    SurfaceAction::None
                }
                KeyCode::Esc => {
                    self.credentials_modal = None;
                    SurfaceAction::None
                }
                _ => {
                    if let Some(modal) = self.credentials_modal.as_mut() {
                        modal
                            .input
                            .handle_event(&ratatui::crossterm::event::Event::Key(key));
                    }
                    SurfaceAction::None
                }
            }
        } else {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.move_providers_focus(-1);
                    SurfaceAction::None
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.move_providers_focus(1);
                    SurfaceAction::None
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    // Open the credentials modal for the focused entry,
                    // unless it has no env-var-based credentials (e.g.
                    // voice_mode) or is deferred.
                    let entry = &PROVIDER_CATALOG[self.providers_focus];
                    if entry.deferred {
                        // Deferred: no-op. Status line shows the notice.
                    } else if !entry.env_vars.is_empty() {
                        self.credentials_modal = Some(CredentialsModal::new(self.providers_focus));
                    }
                    SurfaceAction::None
                }
                KeyCode::Esc => {
                    self.tier = Tier::Overview;
                    SurfaceAction::None
                }
                _ => SurfaceAction::None,
            }
        }
    }

    /// Move the Tools & Providers focus up/down, wrapping at the ends.
    fn move_providers_focus(&mut self, delta: isize) {
        let len = PROVIDER_CATALOG.len() as isize;
        if len == 0 {
            self.providers_focus = 0;
            return;
        }
        let next = (self.providers_focus as isize + delta).rem_euclid(len);
        self.providers_focus = next as usize;
    }

    /// Draw the Tools & Providers tier (Tier P).
    fn render_providers(&self, frame: &mut Frame, area: Rect, t: &Theme) {
        let block = panel("Settings · Tools & Providers", t);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height < 3 {
            return;
        }

        let [head_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        frame.render_widget(
            Paragraph::new(vec![
                Line::from(Span::styled(
                    "Every tool and provider Wayland can use, with its current status.",
                    Style::default().fg(t.text_dim),
                )),
                Line::from(Span::styled(
                    "Enter on a row → set credentials. `esc` returns to settings.",
                    Style::default().fg(t.text_muted),
                )),
            ]),
            head_area,
        );

        let mut lines: Vec<Line> = Vec::new();
        let mut last_category: Option<&'static str> = None;
        // Issue #16: track the line index of the focused row so the body can
        // scroll to keep it on screen. The catalog (23+ entries, two lines
        // each, plus category headers) overflows a short terminal; without a
        // scroll offset every row past `body_area.height` was unreachable.
        let mut focus_line = 0usize;
        for (idx, entry) in PROVIDER_CATALOG.iter().enumerate() {
            if Some(entry.category) != last_category {
                if last_category.is_some() {
                    lines.push(Line::from(""));
                }
                lines.push(Line::from(Span::styled(
                    entry.category,
                    Style::default().fg(t.text_dim).add_modifier(Modifier::BOLD),
                )));
                last_category = Some(entry.category);
            }
            let status = resolve_provider_status(entry);
            let focused = idx == self.providers_focus;
            if focused {
                // The name line is the next push; its hint follows on the line
                // after, so we anchor the scroll on this index.
                focus_line = lines.len();
            }
            let marker = if focused { "▸ " } else { "  " };
            let name_style = if focused {
                Style::default().fg(t.orange).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(t.text)
            };
            let status_style = if status.is_ok() {
                Style::default().fg(t.success)
            } else if matches!(status, ProviderStatus::Deferred) {
                Style::default().fg(t.text_muted)
            } else {
                Style::default().fg(t.warning)
            };
            lines.push(Line::from(vec![
                Span::styled(marker.to_string(), Style::default().fg(t.orange)),
                Span::styled(format!("{:<22}", entry.name), name_style),
                Span::styled(status.label().to_string(), status_style),
            ]));
            // The env-var hint, dimmed.
            let var_hint = if entry.env_vars.is_empty() {
                "    (no env var — auto-detected)".to_string()
            } else {
                format!("    env: {}", entry.env_vars.join(" | "))
            };
            lines.push(Line::from(Span::styled(
                var_hint,
                Style::default().fg(t.text_muted),
            )));
        }
        // Issue #16: scroll the body so the focused row (and its hint line)
        // stay visible. With focus near the top this is 0 (no scroll); as
        // focus moves below the fold the offset advances, clamped so the last
        // page never scrolls past the final row.
        let visible = body_area.height as usize;
        let total = lines.len();
        let scroll_y = (focus_line + 2)
            .saturating_sub(visible)
            .min(total.saturating_sub(visible)) as u16;
        frame.render_widget(
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .scroll((scroll_y, 0)),
            body_area,
        );

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" ↑↓ ", Style::default().fg(t.orange)),
                Span::styled("move   ", Style::default().fg(t.text_dim)),
                Span::styled("⏎ ", Style::default().fg(t.orange)),
                Span::styled("set credentials   ", Style::default().fg(t.text_dim)),
                Span::styled("esc ", Style::default().fg(t.orange)),
                Span::styled("back to settings", Style::default().fg(t.text_dim)),
            ])),
            footer_area,
        );
    }

    /// Draw the credentials modal — a centred panel showing the env var
    /// name, signup URL, the input buffer, and a status banner.
    fn render_credentials_modal(&self, frame: &mut Frame, area: Rect, t: &Theme) {
        let Some(modal) = self.credentials_modal.as_ref() else {
            return;
        };
        let entry = modal.entry();
        let pane = centered(area, 70, 12);
        frame.render_widget(Clear, pane);
        let block = panel(&format!("Set credentials · {}", entry.name), t);
        let inner = block.inner(pane);
        frame.render_widget(block, pane);
        if inner.height < 4 {
            return;
        }

        let var_name = modal.var_name().unwrap_or("");
        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            entry.description.to_string(),
            Style::default().fg(t.text_dim),
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("  env var:  ", Style::default().fg(t.text_dim)),
            Span::styled(
                var_name.to_string(),
                Style::default().fg(t.orange).add_modifier(Modifier::BOLD),
            ),
        ]));
        if !entry.signup_url.is_empty() {
            lines.push(Line::from(vec![
                Span::styled("  get a key: ", Style::default().fg(t.text_dim)),
                Span::styled(entry.signup_url.to_string(), Style::default().fg(t.text)),
            ]));
        }
        lines.push(Line::from(""));
        // Input field — render with a cursor placeholder. Mask the
        // value (show `*`s) so a glance at the screen never leaks it.
        let masked: String = "*".repeat(modal.input.value().len());
        lines.push(Line::from(vec![
            Span::styled("  value:    ", Style::default().fg(t.text_dim)),
            Span::styled(
                format!("{masked}_"),
                Style::default().fg(t.orange).add_modifier(Modifier::BOLD),
            ),
        ]));
        if !modal.status.is_empty() {
            let style = if modal.last_ok {
                Style::default().fg(t.success)
            } else {
                Style::default().fg(t.warning)
            };
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(modal.status.clone(), style)));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled(" ⏎ ", Style::default().fg(t.orange)),
            Span::styled("save   ", Style::default().fg(t.text_dim)),
            Span::styled("esc ", Style::default().fg(t.orange)),
            Span::styled("cancel", Style::default().fg(t.text_dim)),
        ]));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Rendering helpers — free fns, no surface state
// ─────────────────────────────────────────────────────────────────────────

/// The fixed label for a Tier-1 row.
fn row_label(row: Row) -> &'static str {
    match row {
        Row::Provider => "Provider",
        Row::Model => "Model",
        Row::Approval => "Approval",
        Row::PlanFirst => "Plan first",
        Row::StopAfter => "Stop after",
        Row::Compaction => "Compaction",
        Row::LongTerm => "Long-term",
        Row::Expert => "",
    }
}

/// The one-line intro shown at the top of a row's Tier-2 detail pane.
fn detail_intro(row: Row) -> &'static str {
    match row {
        Row::Approval => "How much Wayland does before it asks you:",
        Row::Compaction => "How Wayland keeps the conversation inside the context window:",
        Row::PlanFirst => "Whether Wayland plans before it touches your code:",
        Row::LongTerm => "Whether Wayland remembers you between sessions:",
        Row::Provider => "The LLM provider Wayland connects to:",
        Row::Model => "The model Wayland uses for this provider:",
        Row::StopAfter => "The runaway guard — how many turns before Wayland halts:",
        Row::Expert => "",
    }
}

/// A `▸ more` disclosure span, accented when its row is focused.
fn disclosure(focused: bool, t: &Theme) -> Style {
    if focused {
        Style::default().fg(t.orange)
    } else {
        Style::default().fg(t.text_dim)
    }
}

/// Render a radio strip — `● selected   ○ other   ○ other` — for an
/// `n`-option setting with `selected` filled.
fn radio_strip(labels: &[&str], selected: usize, t: &Theme) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let on = i == selected;
        let glyph = if on { "● " } else { "○ " };
        let style = if on {
            Style::default().fg(t.orange).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text_dim)
        };
        spans.push(Span::styled(glyph.to_string(), style));
        spans.push(Span::styled(format!("{label}   "), style));
    }
    spans
}

/// Render a two-state toggle strip — `○ off   ● on-label` or the inverse.
fn toggle_strip(on: bool, off_label: &str, on_label: &str, t: &Theme) -> Vec<Span<'static>> {
    let off_glyph = if on { "○ " } else { "● " };
    let on_glyph = if on { "● " } else { "○ " };
    let off_style = if on {
        Style::default().fg(t.text_dim)
    } else {
        Style::default().fg(t.orange).add_modifier(Modifier::BOLD)
    };
    let on_style = if on {
        Style::default().fg(t.orange).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.text_dim)
    };
    vec![
        Span::styled(off_glyph.to_string(), off_style),
        Span::styled(format!("{off_label}   "), off_style),
        Span::styled(on_glyph.to_string(), on_style),
        Span::styled(on_label.to_string(), on_style),
    ]
}

/// One choice row inside a Tier-2 detail pane: `● label` + a gloss line.
fn detail_choice(label: &str, gloss: &str, on: bool, t: &Theme) -> Line<'static> {
    let glyph = if on { "  ● " } else { "  ○ " };
    let style = if on {
        Style::default().fg(t.orange).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(t.text_dim)
    };
    Line::from(vec![
        Span::styled(glyph.to_string(), style),
        Span::styled(format!("{label}  "), style),
        Span::styled(format!("— {gloss}"), Style::default().fg(t.text_muted)),
    ])
}

/// A `w`×`h` rectangle centred inside `area`, clamped to `area`'s bounds.
fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// A key event with no modifiers.
    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// A printable-char key event.
    fn ch(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
    }

    /// Render the surface into an 80×30 `TestBackend` and return the
    /// flattened buffer text.
    fn render_text(surface: &mut ConfigSurface, app: &App) -> String {
        let theme = Theme::no_color();
        let mut terminal = Terminal::new(TestBackend::new(80, 30)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), app, &theme))
            .expect("render config surface");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn surface_reports_config_id() {
        assert_eq!(ConfigSurface::new().id(), SurfaceId::Config);
    }

    #[test]
    fn on_enter_seeds_settings_from_config_view() {
        let mut app = App::new();
        app.config.provider = "anthropic".into();
        app.config.model = "claude-sonnet-4-6".into();
        app.config.memory_enabled = true;
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        assert_eq!(surface.current.provider, "anthropic");
        assert_eq!(surface.current.model, "claude-sonnet-4-6");
        assert!(surface.current.key_set);
        assert!(surface.current.long_term_memory);
        // A fresh seed is clean — nothing to revert.
        assert!(!surface.is_dirty());
    }

    #[test]
    fn seeds_turn_cap_and_compaction_from_real_config_not_placeholders_g1() {
        // Slice 2: the turn-cap and compaction rows must reflect the resolved
        // config, so a save persists the real value (not the old hardcoded
        // 25 / Safe placeholders).
        let mut app = App::new();
        app.config.max_turns = Some(7);
        app.config.compaction = "full".into();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        assert_eq!(surface.current.stop_after_turns, 7);
        assert_eq!(surface.current.compaction, Compaction::Full);
        assert!(!surface.is_dirty(), "a fresh seed must be clean");
    }

    #[test]
    fn missing_max_turns_falls_back_to_display_default_g1() {
        // `None` (no configured cap) shows the 25 display default.
        let app = App::new(); // ConfigView::default(): max_turns None, compaction ""
        let mut surface = ConfigSurface::new();
        surface.current = SettingsModel::from_config_view(&app.config);
        assert_eq!(surface.current.stop_after_turns, 25);
        // Empty compaction string falls back to Safe.
        assert_eq!(surface.current.compaction, Compaction::Safe);
    }

    #[test]
    fn seeds_approval_posture_from_real_config_g1() {
        // Slice 3: the approval radio reflects the resolved `[default]
        // approval_mode`, so a save round-trips it instead of always
        // writing Default.
        let mut app = App::new();
        app.config.approval = "force".into();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        assert_eq!(surface.current.approval, ApprovalMode::Force);
        assert!(!surface.is_dirty());
    }

    #[test]
    fn seeds_plan_first_from_real_config_g1() {
        // Slice 4: the plan-first toggle reflects [plan] plan_first, so a save
        // round-trips it (it was a dead `true` placeholder before).
        let mut app = App::new();
        app.config.plan_first = true;
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        assert!(surface.current.plan_first);
        assert!(!surface.is_dirty());

        // The config default (false) seeds as off, not the old hardcoded true.
        let mut app_off = App::new();
        let mut s_off = ConfigSurface::new();
        s_off.on_enter(&mut app_off);
        assert!(!s_off.current.plan_first);
    }

    #[test]
    fn approval_maps_round_trip_between_view_string_and_config_g1() {
        use wcore_config::config::ApprovalMode as Cfg;
        for (s, local, cfg) in [
            ("default", ApprovalMode::Default, Cfg::Default),
            ("auto-edit", ApprovalMode::AutoEdit, Cfg::AutoEdit),
            ("force", ApprovalMode::Force, Cfg::Force),
        ] {
            assert_eq!(ApprovalMode::from_view_str(s), local);
            assert_eq!(ApprovalMode::parse_view_str(s), Some(local));
            assert_eq!(local.to_config(), cfg);
            // The config enum's wire string must match the view string the
            // surface parses — the round-trip can't silently drift.
            assert_eq!(cfg.as_str(), s);
        }

        // D033: documented aliases must NOT silently downgrade. The snake-case
        // `auto_edit` (emitted by `SessionMode::current_mode()`) and the
        // foreign-agent `yolo` (Force) both round-trip to the right posture
        // instead of hitting a catch-all `Default`.
        assert_eq!(
            ApprovalMode::parse_view_str("auto_edit"),
            Some(ApprovalMode::AutoEdit)
        );
        assert_eq!(
            ApprovalMode::from_view_str("auto_edit"),
            ApprovalMode::AutoEdit
        );
        assert_eq!(
            ApprovalMode::parse_view_str("yolo"),
            Some(ApprovalMode::Force)
        );
        assert_eq!(ApprovalMode::from_view_str("yolo"), ApprovalMode::Force);

        // An unrecognised string is an explicit `None` from the strict parser
        // (caller's choice), not a silently-swallowed value. `from_view_str`'s
        // documented boot fallback then seeds `Default`.
        assert_eq!(ApprovalMode::parse_view_str("nonsense"), None);
        assert_eq!(
            ApprovalMode::from_view_str("nonsense"),
            ApprovalMode::Default
        );
    }

    #[test]
    fn compaction_maps_round_trip_between_view_string_and_engine_level_g1() {
        use wcore_compact::CompactionLevel;
        for (s, local, level) in [
            ("off", Compaction::Off, CompactionLevel::Off),
            ("safe", Compaction::Safe, CompactionLevel::Safe),
            ("full", Compaction::Full, CompactionLevel::Full),
        ] {
            assert_eq!(Compaction::from_view_str(s), local);
            assert_eq!(local.to_level(), level);
        }
        // An unknown string is coerced to Safe, never panics.
        assert_eq!(Compaction::from_view_str("garbage"), Compaction::Safe);
    }

    #[test]
    fn editing_clears_a_stale_save_outcome_g1() {
        // After a save shows ✓/⚠, the next edit must reset the indicator so
        // the context line never lies about the current dirty state.
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        surface.save_pending = true;
        surface.save_error = Some("disk full".into());
        // Focus the approval row and toggle it.
        surface.focus = Row::FOCUSABLE
            .iter()
            .position(|r| *r == Row::Approval)
            .unwrap();
        surface.toggle_focused();
        assert!(surface.save_error.is_none());
        assert!(!surface.save_pending);
        assert!(surface.is_dirty(), "the toggle is a real edit");
    }

    #[test]
    fn approval_radio_cycles_both_directions_g1() {
        // The footer advertises "↑↓ choose": `↓`/`j`/`space` must advance and
        // `↑`/`k` must step backward, wrapping at both ends. From the first
        // option, backward lands on the LAST and forward on the SECOND.
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        surface.focus = Row::FOCUSABLE
            .iter()
            .position(|r| *r == Row::Approval)
            .unwrap();

        let last = *ApprovalMode::ALL.last().unwrap();
        let second = ApprovalMode::ALL[1];

        // Backward from the first option wraps to the last.
        surface.current.approval = ApprovalMode::ALL[0];
        surface.toggle_focused_back();
        assert_eq!(surface.current.approval, last, "↑/k must wrap backward");

        // Forward from the first option lands on the second.
        surface.current.approval = ApprovalMode::ALL[0];
        surface.toggle_focused();
        assert_eq!(surface.current.approval, second, "↓/j must move forward");

        // Forward then backward is a no-op (true inverse).
        surface.current.approval = ApprovalMode::ALL[0];
        surface.toggle_focused();
        surface.toggle_focused_back();
        assert_eq!(surface.current.approval, ApprovalMode::ALL[0]);
    }

    // ── Tier-1 render snapshot ──────────────────────────────────────────

    #[test]
    fn tier1_renders_all_four_sections_and_eight_settings() {
        let mut app = App::new();
        app.config.provider = "anthropic".into();
        app.config.model = "claude-sonnet-4-6".into();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        let text = render_text(&mut surface, &app);
        // The four intent section headings.
        assert!(text.contains("CONNECTION"), "missing CONNECTION:\n{text}");
        assert!(
            text.contains("HOW WAYLAND ACTS"),
            "missing HOW WAYLAND ACTS:\n{text}"
        );
        assert!(
            text.contains("MEMORY & CONTEXT"),
            "missing MEMORY & CONTEXT:\n{text}"
        );
        // A consequence gloss, not a mechanism, for the approval radio.
        assert!(
            text.contains("Asks before it writes or runs anything"),
            "missing approval gloss:\n{text}"
        );
        // The expert-entry row.
        assert!(
            text.contains("expert settings"),
            "missing expert row:\n{text}"
        );
        // The save/undo footer promise.
        assert!(
            text.contains("config.toml"),
            "missing save promise:\n{text}"
        );
    }

    #[test]
    fn provider_and_model_rows_are_read_outs_with_change_hints_d025() {
        // D025: the Provider/Model rows must render as honest read-outs that
        // point the user at the real verb (`/provider`, `/model`) — never an
        // in-surface affordance like "▸ change"/"▸ pick", and never any
        // sprint/"Wave" jargon.
        let mut app = App::new();
        app.config.provider = "anthropic".into();
        app.config.model = "claude-sonnet-4-6".into();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        let text = render_text(&mut surface, &app);

        // The change-hints are present and name the real slash commands.
        assert!(
            text.contains("Change with /provider"),
            "Provider row must show the /provider change hint:\n{text}"
        );
        assert!(
            text.contains("Change with /model"),
            "Model row must show the /model change hint:\n{text}"
        );
        // No sprint/"Wave" jargon anywhere in the rendered surface.
        assert!(
            !text.contains("Wave"),
            "rendered config surface must contain no \"Wave\" jargon:\n{text}"
        );
        // The phantom in-surface affordances are gone.
        assert!(
            !text.contains("▸ change") && !text.contains("▸ pick"),
            "Provider/Model must not advertise an in-surface picker:\n{text}"
        );
    }

    // ── Tier-2 render snapshot ──────────────────────────────────────────

    #[test]
    fn tier2_detail_pane_shows_every_choice_with_a_gloss() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        // Focus the Approval row and open its detail pane.
        while surface.focused_row() != Row::Approval {
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(surface.tier, Tier::Detail);
        let text = render_text(&mut surface, &app);
        // All three approval choices and their glosses are listed.
        assert!(text.contains("Default"), "missing Default:\n{text}");
        assert!(text.contains("Auto-edit"), "missing Auto-edit:\n{text}");
        assert!(text.contains("Force"), "missing Force:\n{text}");
        assert!(text.contains("Never asks"), "missing Force gloss:\n{text}");
    }

    // ── Tier-3 render snapshot ──────────────────────────────────────────

    #[test]
    fn tier3_expert_pane_lists_19_glossed_provider_compat_fields() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        surface.handle_key(ch('x'), &mut app);
        assert_eq!(surface.tier, Tier::Expert);
        let text = render_text(&mut surface, &app);
        // The expert pane is titled and grouped.
        assert!(
            text.contains("Settings · Expert"),
            "missing expert title:\n{text}"
        );
        assert!(
            text.contains("Message format"),
            "missing group heading:\n{text}"
        );
        // A raw key is shown alongside its plain-language gloss.
        assert!(
            text.contains("merge_assistant_messages"),
            "missing raw key:\n{text}"
        );
        assert!(
            text.contains("Combine back-to-back AI messages"),
            "missing gloss:\n{text}"
        );
        // Exactly 19 expert fields are defined — one per real
        // `ProviderCompat` field, with no padding (D029).
        assert_eq!(EXPERT_FIELDS.len(), 19);
    }

    // ── D029: every expert key is a REAL ProviderCompat field ───────────

    /// Each `EXPERT_FIELDS` key must name a field that actually exists on
    /// `wcore_config::ProviderCompat`. The fix removed 5 fabricated keys
    /// (`per_model_input_override`, `per_model_output_override`,
    /// `wall_time_budget`, `token_budget`, `cost_budget`) that never existed
    /// on the struct. We derive the real field set from a serialized
    /// `ProviderCompat` (every Option field is emitted as a JSON key), so
    /// this test fails the moment a fabricated key is reintroduced or a real
    /// field is renamed.
    #[test]
    fn every_expert_key_is_a_real_provider_compat_field() {
        let real_value = serde_json::to_value(wcore_config::compat::ProviderCompat::default())
            .expect("ProviderCompat serializes");
        let real_keys: std::collections::BTreeSet<&str> = real_value
            .as_object()
            .expect("ProviderCompat serializes to a JSON object")
            .keys()
            .map(String::as_str)
            .collect();

        for field in EXPERT_FIELDS.iter() {
            assert!(
                real_keys.contains(field.key),
                "expert key `{}` is NOT a real ProviderCompat field (real fields: {:?})",
                field.key,
                real_keys
            );
        }

        // The 5 fabricated keys must never reappear.
        for fake in [
            "per_model_input_override",
            "per_model_output_override",
            "wall_time_budget",
            "token_budget",
            "cost_budget",
        ] {
            assert!(
                !EXPERT_FIELDS.iter().any(|f| f.key == fake),
                "fabricated expert key `{fake}` is back in EXPERT_FIELDS"
            );
        }
    }

    // ── D030: google_meet token expiry decode ───────────────────────────

    fn write_meet_token(dir: &std::path::Path, expires_at: Option<u64>) -> std::path::PathBuf {
        let path = dir.join("google_meet.json");
        let body = match expires_at {
            Some(exp) => format!(
                r#"{{"access_token":"tok","refresh_token":"r","expires_at_unix_secs":{exp},"token_type":"Bearer"}}"#
            ),
            None => {
                r#"{"access_token":"tok","refresh_token":"r","token_type":"Bearer"}"#.to_string()
            }
        };
        std::fs::write(&path, body).expect("write token file");
        path
    }

    #[test]
    fn google_meet_token_status_absent_when_file_missing() {
        let dir = tempfile::tempdir().expect("tempdir");
        let missing = dir.path().join("nope.json");
        assert_eq!(
            google_meet_token_status(&missing),
            GoogleMeetTokenStatus::Absent
        );
    }

    #[test]
    fn google_meet_token_status_expired_when_past_expiry() {
        let dir = tempfile::tempdir().expect("tempdir");
        // expiry of 1 (1970) is unambiguously in the past.
        let path = write_meet_token(dir.path(), Some(1));
        assert_eq!(
            google_meet_token_status(&path),
            GoogleMeetTokenStatus::Expired
        );
    }

    #[test]
    fn google_meet_token_status_valid_when_future_expiry() {
        let dir = tempfile::tempdir().expect("tempdir");
        let far_future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            + 3600;
        let path = write_meet_token(dir.path(), Some(far_future));
        assert_eq!(
            google_meet_token_status(&path),
            GoogleMeetTokenStatus::Valid
        );
    }

    // ── FIX 3: openai-chatgpt OAuth status row ───────────────────────────

    /// Seed (or omit) `$HOME/.wayland/oauth/chatgpt.json` under a tempdir HOME,
    /// run `resolve_chatgpt_status`, and restore HOME. The `token` arg:
    /// - `None` → write NO token file (not signed in).
    /// - `Some(None)` → write a token with no `expires_at_unix_secs` field.
    /// - `Some(Some(exp))` → write a token whose expiry is `exp`.
    #[cfg(unix)]
    fn chatgpt_status_with_home(token: Option<Option<u64>>) -> ProviderStatus {
        let tmp = tempfile::tempdir().expect("tempdir");
        if let Some(expires_at) = token {
            let oauth_dir = tmp.path().join(".wayland").join("oauth");
            std::fs::create_dir_all(&oauth_dir).expect("mkdir");
            // A JWT-less access_token is fine: the plan decode just yields None,
            // and the status row only reads expiry. The struct must round-trip
            // through `OAuthTokens`'s serde shape.
            let body = match expires_at {
                Some(exp) => format!(
                    r#"{{"access_token":"hdr.e30.sig","refresh_token":"r","expires_at_unix_secs":{exp},"token_type":"Bearer"}}"#
                ),
                None => {
                    r#"{"access_token":"hdr.e30.sig","refresh_token":"r","token_type":"Bearer"}"#
                        .to_string()
                }
            };
            std::fs::write(oauth_dir.join("chatgpt.json"), body).expect("write token");
        }
        let saved = std::env::var_os("HOME");
        // SAFETY: serial test; HOME reverted before return.
        unsafe { std::env::set_var("HOME", tmp.path()) };
        let status = resolve_chatgpt_status();
        match saved {
            Some(v) => unsafe { std::env::set_var("HOME", v) },
            None => unsafe { std::env::remove_var("HOME") },
        }
        status
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn chatgpt_status_not_configured_when_no_token() {
        assert_eq!(
            chatgpt_status_with_home(None),
            ProviderStatus::NotConfigured
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn chatgpt_status_connected_when_future_expiry() {
        let far_future = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            + 3600;
        assert_eq!(
            chatgpt_status_with_home(Some(Some(far_future))),
            ProviderStatus::OAuthConnected
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn chatgpt_status_expired_when_past_expiry() {
        assert_eq!(
            chatgpt_status_with_home(Some(Some(1))),
            ProviderStatus::OAuthExpired
        );
    }

    #[cfg(unix)]
    #[test]
    #[serial_test::serial]
    fn chatgpt_status_connected_when_no_expiry_field() {
        // A token with no recorded expiry is treated as valid (mirrors the
        // google-meet missing-expiry handling).
        assert_eq!(
            chatgpt_status_with_home(Some(None)),
            ProviderStatus::OAuthConnected
        );
    }

    #[test]
    fn google_meet_token_status_valid_when_no_expiry_field() {
        // No `expires_at_unix_secs` (provider returned no `expires_in`) is
        // treated as fresh — mirrors the engine's `token_is_fresh`.
        let dir = tempfile::tempdir().expect("tempdir");
        let path = write_meet_token(dir.path(), None);
        assert_eq!(
            google_meet_token_status(&path),
            GoogleMeetTokenStatus::Valid
        );
    }

    #[test]
    fn google_meet_token_status_absent_when_unparsable() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("google_meet.json");
        std::fs::write(&path, "{ not json").expect("write");
        assert_eq!(
            google_meet_token_status(&path),
            GoogleMeetTokenStatus::Absent
        );
    }

    // ── esc reverts an unsaved change ───────────────────────────────────

    #[test]
    fn esc_reverts_an_unsaved_toggle() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        // Focus the long-term memory toggle and flip it.
        while surface.focused_row() != Row::LongTerm {
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        let before = surface.current.long_term_memory;
        surface.handle_key(ch(' '), &mut app);
        assert_ne!(
            surface.current.long_term_memory, before,
            "space should flip the toggle"
        );
        assert!(surface.is_dirty(), "an unsaved edit should be dirty");
        // `esc` over a dirty model reverts it and stays on the surface.
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        assert_eq!(
            surface.current.long_term_memory, before,
            "esc must revert the unsaved toggle"
        );
        assert!(!surface.is_dirty(), "after revert the model is clean");
    }

    #[test]
    fn esc_on_a_clean_surface_closes_to_workspace() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        // No edits made — `esc` closes back to the workspace.
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(matches!(
            action,
            SurfaceAction::Switch(SurfaceId::Workspace)
        ));
    }

    // ── a toggle updates state ──────────────────────────────────────────

    #[test]
    fn space_cycles_the_approval_radio() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        while surface.focused_row() != Row::Approval {
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        assert_eq!(surface.current.approval, ApprovalMode::Default);
        surface.handle_key(ch(' '), &mut app);
        assert_eq!(surface.current.approval, ApprovalMode::AutoEdit);
        surface.handle_key(ch(' '), &mut app);
        assert_eq!(surface.current.approval, ApprovalMode::Force);
        // Cycles back around to Default.
        surface.handle_key(ch(' '), &mut app);
        assert_eq!(surface.current.approval, ApprovalMode::Default);
    }

    #[test]
    fn space_flips_the_plan_first_toggle() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        while surface.focused_row() != Row::PlanFirst {
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        let before = surface.current.plan_first;
        surface.handle_key(ch(' '), &mut app);
        assert_ne!(surface.current.plan_first, before);
    }

    // ── detail pane save promotes the baseline ──────────────────────────

    #[test]
    fn detail_pane_enter_saves_the_change() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        while surface.focused_row() != Row::Compaction {
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        surface.handle_key(key(KeyCode::Enter), &mut app); // open detail
        assert_eq!(surface.tier, Tier::Detail);
        let before = surface.current.compaction;
        surface.handle_key(ch(' '), &mut app); // cycle the radio
        assert_ne!(surface.current.compaction, before);
        surface.handle_key(key(KeyCode::Enter), &mut app); // save & close
        assert_eq!(surface.tier, Tier::Overview);
        // The change persisted — the baseline moved with it.
        assert!(!surface.is_dirty(), "a saved change is no longer dirty");
        assert!(surface.save_pending, "a save should be recorded");
    }

    #[test]
    fn detail_pane_esc_reverts_the_change() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        while surface.focused_row() != Row::Compaction {
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        surface.handle_key(key(KeyCode::Enter), &mut app);
        let before = surface.current.compaction;
        surface.handle_key(ch(' '), &mut app);
        assert_ne!(surface.current.compaction, before);
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert_eq!(surface.tier, Tier::Overview);
        assert_eq!(
            surface.current.compaction, before,
            "esc in the detail pane reverts"
        );
    }

    // ── the text-field focus state machine ──────────────────────────────

    #[test]
    fn stop_after_text_edit_commits_a_new_value() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        while surface.focused_row() != Row::StopAfter {
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        assert_eq!(surface.current.stop_after_turns, 25);
        // `⏎` begins the edit; the buffer captures keystrokes.
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(surface.editor.is_some(), "edit should be in flight");
        // Clear the seeded "25" and type a new value.
        surface.handle_key(key(KeyCode::Backspace), &mut app);
        surface.handle_key(key(KeyCode::Backspace), &mut app);
        surface.handle_key(ch('4'), &mut app);
        surface.handle_key(ch('0'), &mut app);
        surface.handle_key(key(KeyCode::Enter), &mut app); // commit
        assert!(surface.editor.is_none(), "edit should be committed");
        assert_eq!(surface.current.stop_after_turns, 40);
    }

    #[test]
    fn stop_after_text_edit_esc_cancels_without_change() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        while surface.focused_row() != Row::StopAfter {
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        surface.handle_key(key(KeyCode::Enter), &mut app);
        surface.handle_key(key(KeyCode::Backspace), &mut app);
        surface.handle_key(ch('9'), &mut app);
        // `esc` cancels — the setting is untouched.
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(surface.editor.is_none());
        assert_eq!(surface.current.stop_after_turns, 25);
    }

    #[test]
    fn stop_after_rejects_a_zero_value() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        while surface.focused_row() != Row::StopAfter {
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        surface.handle_key(key(KeyCode::Enter), &mut app);
        surface.handle_key(key(KeyCode::Backspace), &mut app);
        surface.handle_key(key(KeyCode::Backspace), &mut app);
        surface.handle_key(ch('0'), &mut app);
        surface.handle_key(key(KeyCode::Enter), &mut app);
        // A zero runaway-guard is rejected; the old value stands.
        assert_eq!(surface.current.stop_after_turns, 25);
    }

    // ── the Expert-tier (ProviderCompat cost) in-place editor ───────────
    //
    // The Expert tier was read-only before this slice — `handle_expert_key`
    // handled only `↑↓`/`esc`, the static `EXPERT_FIELDS` table carried no
    // live values, and the footer advertised no edit. These tests drive real
    // keys through the router and assert the RENDERED frame changes (cursor,
    // value, footer) — not just an internal flag — to guard against a
    // phantom-affordance regression.

    /// Process-global env guard: `WAYLAND_HOME` / `set_var` are process-wide,
    /// so the persisting test serialises through this lock (the same pattern
    /// the theme tests use) to stay hermetic under the concurrent runner.
    static EXPERT_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Open the Expert tier and step the selection to the first editable
    /// Pricing field (`cost_per_input_token`, `EXPERT_FIELDS[15]`).
    fn enter_expert_on_first_cost_field(surface: &mut ConfigSurface, app: &mut App) {
        surface.handle_key(ch('x'), app); // overview → Expert tier
        assert_eq!(surface.tier, Tier::Expert);
        for _ in 0..15 {
            surface.handle_key(key(KeyCode::Down), app);
        }
        assert_eq!(
            CostField::for_index(surface.expert_focus),
            Some(CostField::Input),
            "selection should rest on cost_per_input_token"
        );
    }

    #[test]
    fn expert_footer_advertises_the_edit_affordance() {
        // The footer must promise the edit so the affordance is not a phantom.
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        surface.handle_key(ch('x'), &mut app);
        let frame = render_text(&mut surface, &app);
        assert!(
            frame.contains("edit cost"),
            "expert footer must advertise the edit affordance, got:\n{frame}"
        );
    }

    #[test]
    fn expert_enter_renders_the_edit_cursor_on_a_cost_field() {
        // Pressing `⏎` on a Pricing field must show the live edit buffer +
        // cursor in the RENDERED frame, plus the save/cancel hint.
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        enter_expert_on_first_cost_field(&mut surface, &mut app);
        // Before edit: no buffer cursor, no save hint.
        let before = render_text(&mut surface, &app);
        assert!(
            !before.contains("⏎ save · esc cancel"),
            "no edit hint before ⏎, got:\n{before}"
        );
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(surface.expert_editor.is_some(), "edit should be in flight");
        let after = render_text(&mut surface, &app);
        assert!(
            after.contains('_'),
            "the edit buffer cursor must render, got:\n{after}"
        );
        assert!(
            after.contains("⏎ save · esc cancel"),
            "the in-place edit hint must render, got:\n{after}"
        );
    }

    #[test]
    fn expert_typing_updates_the_rendered_cost_value() {
        // Keystrokes must update the RENDERED buffer, not just the state var.
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        enter_expert_on_first_cost_field(&mut surface, &mut app);
        surface.handle_key(key(KeyCode::Enter), &mut app);
        // The default ConfigView seeds no override, so the buffer starts empty.
        surface.handle_key(ch('0'), &mut app);
        surface.handle_key(ch('.'), &mut app);
        surface.handle_key(ch('5'), &mut app);
        let frame = render_text(&mut surface, &app);
        assert!(
            frame.contains("0.5_"),
            "the typed value + cursor must render, got:\n{frame}"
        );
    }

    #[test]
    fn expert_commit_renders_new_value_and_persists() {
        // `⏎` commits: the new value must render (buffer gone) AND land in
        // `[providers.<active>].compat` on disk. Hermetic via WAYLAND_HOME.
        let _guard = EXPERT_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("WAYLAND_HOME");
        // SAFETY: process-global env mutation is serialised by EXPERT_ENV_LOCK;
        // the previous value is restored before the lock is released.
        unsafe { std::env::set_var("WAYLAND_HOME", dir.path()) };

        let mut app = App::new();
        app.config.provider = "anthropic".into();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        enter_expert_on_first_cost_field(&mut surface, &mut app);
        surface.handle_key(key(KeyCode::Enter), &mut app);
        surface.handle_key(ch('0'), &mut app);
        surface.handle_key(ch('.'), &mut app);
        surface.handle_key(ch('2'), &mut app);
        surface.handle_key(key(KeyCode::Enter), &mut app); // commit

        assert!(surface.expert_editor.is_none(), "edit should be committed");
        assert_eq!(surface.current.compat_costs.input, Some(0.2));
        // The committed value renders as a value, not a live buffer.
        let frame = render_text(&mut surface, &app);
        assert!(
            frame.contains("0.2") && !frame.contains("0.2_"),
            "the committed value must render without the edit cursor, got:\n{frame}"
        );
        // And it must have been written to the active provider's compat table.
        let written = std::fs::read_to_string(dir.path().join("config.toml"))
            .expect("config.toml should have been written");
        assert!(
            written.contains("cost_per_input_token"),
            "the override must persist to [providers.anthropic].compat, got:\n{written}"
        );

        // SAFETY: restore the prior env under the same lock.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }
    }

    #[test]
    fn expert_esc_cancels_without_change() {
        // `esc` drops the in-flight edit; the override stays unset and the
        // buffer no longer renders.
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        enter_expert_on_first_cost_field(&mut surface, &mut app);
        surface.handle_key(key(KeyCode::Enter), &mut app);
        surface.handle_key(ch('9'), &mut app);
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(surface.expert_editor.is_none());
        assert_eq!(surface.current.compat_costs.input, None);
        let frame = render_text(&mut surface, &app);
        assert!(
            !frame.contains("⏎ save · esc cancel"),
            "the edit hint must be gone after esc, got:\n{frame}"
        );
    }

    #[test]
    fn overview_esc_saves_pending_toggle_instead_of_reverting() {
        // Regression: toggling Long-term memory then pressing `esc` must SAVE
        // the edit (persist + advance baseline + signal a live rebind) and
        // stay on the surface — not silently revert it. Hermetic via
        // WAYLAND_HOME under the shared env lock (the save writes config.toml).
        let _guard = EXPERT_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().expect("tempdir");
        let prev = std::env::var_os("WAYLAND_HOME");
        // SAFETY: process-global env mutation is serialised by EXPERT_ENV_LOCK;
        // the previous value is restored before the lock is released.
        unsafe { std::env::set_var("WAYLAND_HOME", dir.path()) };

        let mut app = App::new();
        app.config.memory_enabled = false; // start with long-term memory off
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        assert!(!surface.current.long_term_memory);
        assert!(!surface.is_dirty(), "a fresh seed must be clean");

        // Focus the Long-term row (FOCUSABLE index 4) and toggle it on.
        for _ in 0..4 {
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        assert_eq!(surface.focused_row(), Row::LongTerm);
        surface.handle_key(key(KeyCode::Char(' ')), &mut app);
        assert!(surface.current.long_term_memory, "space must toggle it on");
        assert!(surface.is_dirty(), "the toggle must be a pending edit");

        // `esc` on a dirty overview must SAVE, not revert.
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "dirty esc must stay on the surface, not switch away"
        );
        assert!(
            surface.current.long_term_memory,
            "the toggle must survive esc — not be reverted"
        );
        assert!(
            !surface.is_dirty(),
            "after save the edit is no longer pending (baseline advanced)"
        );
        // The save must signal a live rebind so the engine sees the change.
        assert!(
            matches!(app.rebind_request, crate::tui::app::RebindRequest::Tier1Save),
            "saving a dirty overview must raise the Tier1Save rebind signal"
        );
        // And it must have been written to disk.
        let written = std::fs::read_to_string(dir.path().join("config.toml"))
            .expect("config.toml should have been written");
        assert!(
            written.contains("[memory]") && written.contains("enabled"),
            "the memory toggle must persist to [memory].enabled, got:\n{written}"
        );

        // SAFETY: restore the prior env under the same lock.
        unsafe {
            match prev {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }
    }

    #[test]
    fn overview_esc_closes_when_nothing_is_dirty() {
        // With no pending edits, `esc` closes back to the workspace.
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        assert!(!surface.is_dirty());
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(
            matches!(action, SurfaceAction::Switch(SurfaceId::Workspace)),
            "a clean esc must close to the workspace"
        );
    }

    #[test]
    fn expert_read_only_field_has_no_edit_affordance() {
        // `⏎` on a non-Pricing field (e.g. the first message-format toggle,
        // index 0) is inert — no editor opens — so the affordance stays
        // honest (read-only fields are not advertised as editable).
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        surface.handle_key(ch('x'), &mut app);
        assert_eq!(surface.expert_focus, 0);
        assert!(CostField::for_index(0).is_none());
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(
            surface.expert_editor.is_none(),
            "⏎ on a read-only field must not open an editor"
        );
    }

    // ── navigation ──────────────────────────────────────────────────────

    #[test]
    fn focus_wraps_at_both_ends() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        // Provider/Model are read-outs outside the focus ring, so the first
        // focusable row is Approval — focus never lands on Provider/Model.
        assert_eq!(surface.focused_row(), Row::Approval);
        // Up from the first focusable row wraps to the last (Expert).
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(surface.focused_row(), Row::Expert);
        // Down from the last row wraps back to the first focusable row.
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.focused_row(), Row::Approval);
    }

    #[test]
    fn focus_ring_never_lands_on_provider_or_model_d025() {
        // D025: Provider/Model are read-out rows, not editors. Walking the
        // full ring in both directions must never focus them, so `⏎` can
        // never be inert on a Provider/Model row.
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        // A full forward lap plus a full backward lap covers every reachable
        // focus position twice.
        for _ in 0..(Row::ALL.len() * 2) {
            assert_ne!(surface.focused_row(), Row::Provider);
            assert_ne!(surface.focused_row(), Row::Model);
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        for _ in 0..(Row::ALL.len() * 2) {
            assert_ne!(surface.focused_row(), Row::Provider);
            assert_ne!(surface.focused_row(), Row::Model);
            surface.handle_key(key(KeyCode::Up), &mut app);
        }
    }

    #[test]
    fn enter_on_expert_row_opens_the_expert_tier() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        while surface.focused_row() != Row::Expert {
            surface.handle_key(key(KeyCode::Down), &mut app);
        }
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(surface.tier, Tier::Expert);
        // `esc` from the expert tier returns to the overview.
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert_eq!(surface.tier, Tier::Overview);
    }

    #[test]
    fn renders_on_a_tiny_terminal_without_panicking() {
        // All three tiers must clamp on a terminal too small for their
        // layout splits — a 1×1 frame is the degenerate case.
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        let theme = Theme::no_color();
        for tier in [Tier::Overview, Tier::Detail, Tier::Expert, Tier::Providers] {
            surface.tier = tier;
            for (w, h) in [(1u16, 1u16), (8, 4), (20, 6)] {
                let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
                terminal
                    .draw(|f| surface.render(f, f.area(), &app, &theme))
                    .expect("render config on a tiny terminal");
            }
        }
    }

    #[test]
    fn expert_field_selection_scrolls_and_wraps() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        surface.handle_key(ch('x'), &mut app);
        assert_eq!(surface.expert_focus, 0);
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(
            surface.expert_focus,
            EXPERT_FIELDS.len() - 1,
            "up wraps to the last expert field"
        );
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.expert_focus, 0);
    }

    // ── Tools & Providers (v0.9.0 W4 E1 Part A + B) ─────────────────────

    /// Every env var named in a `tool_backends/*` resolver MUST appear
    /// somewhere in `PROVIDER_CATALOG` — otherwise a user has no UI to
    /// set credentials for that tool.
    #[test]
    fn config_lists_every_env_var_keyed_provider() {
        // Pulled from `crates/wcore-agent/src/tool_backends/*.rs`
        // `read_env_key(...)` call sites. Updating that list (adding a
        // new provider) requires updating this catalog — by design.
        let expected = [
            "TAVILY_API_KEY",
            "BRAVE_SEARCH_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENAI_API_KEY",
            "GEMINI_API_KEY",
            "GROQ_API_KEY",
            "FAL_API_KEY",
            "HF_API_KEY",
            "ELEVENLABS_API_KEY",
            "DISCORD_BOT_TOKEN",
            "HASS_URL",
            "HASS_TOKEN",
            "DATABASE_URL",
            "POSTGRES_URL",
            "PG_CONN_STRING",
            "GOOGLE_CLIENT_ID",
            "GOOGLE_CLIENT_SECRET",
        ];
        let surfaced: std::collections::HashSet<&'static str> = PROVIDER_CATALOG
            .iter()
            .flat_map(|e| e.env_vars.iter().copied())
            .collect();
        for k in expected {
            assert!(
                surfaced.contains(k),
                "PROVIDER_CATALOG is missing env var {k} — tool_backends/* resolves it but Config TUI doesn't surface it"
            );
        }
    }

    /// Every tool entry MUST show a "current backend" status — never
    /// just the raw env-var name. The resolver returns one of the
    /// non-`Deferred` `ProviderStatus` values for non-deferred entries.
    #[test]
    fn config_shows_current_backend_per_tool_resolver() {
        let mut tool_count = 0;
        for entry in PROVIDER_CATALOG {
            if entry.deferred {
                assert_eq!(
                    resolve_provider_status(entry),
                    ProviderStatus::Deferred,
                    "{} is deferred but resolver did not return Deferred",
                    entry.name
                );
                continue;
            }
            tool_count += 1;
            let status = resolve_provider_status(entry);
            // The non-deferred entries resolve to one of the live
            // status variants — never Deferred.
            assert!(
                !matches!(status, ProviderStatus::Deferred),
                "non-deferred entry {} resolved to Deferred",
                entry.name
            );
        }
        assert!(
            tool_count >= 10,
            "expected ≥10 non-deferred provider entries, got {tool_count}"
        );
    }

    /// With the `voice` feature OFF the cpal backend is not linked, so we
    /// cannot probe audio devices and MUST report the honest "not probed"
    /// state (explicitly not an `is_ok()` value). This is the only path that
    /// is deterministic regardless of build features (D028).
    #[cfg(not(feature = "voice"))]
    #[test]
    fn voice_mode_status_is_unprobed_when_feature_off() {
        let status = resolve_voice_mode_status();
        assert_eq!(
            status,
            ProviderStatus::DeviceUnprobed,
            "feature-off voice_mode must report DeviceUnprobed, not a false readiness claim"
        );
        assert!(!status.is_ok(), "DeviceUnprobed must never count as ready");
    }

    /// With the `voice` feature ON the resolver runs the same cpal probe the
    /// tool itself uses (`CpalAudioRecorder::try_default()`): a present default
    /// input device maps to `DeviceAvailable`, its absence (CI / container /
    /// headless host) to `DeviceUnavailable`. The host may or may not have a
    /// mic, so we assert the mapping matches the probe outcome — never the old
    /// permanent `DeviceUnprobed`.
    #[cfg(feature = "voice")]
    #[test]
    fn voice_mode_status_reflects_cpal_probe_when_feature_on() {
        use wcore_agent::tool_backends::voice_mode::CpalAudioRecorder;
        let status = resolve_voice_mode_status();
        let device_present = CpalAudioRecorder::try_default().is_some();
        let expected = if device_present {
            ProviderStatus::DeviceAvailable
        } else {
            ProviderStatus::DeviceUnavailable
        };
        assert_eq!(
            status, expected,
            "feature-on voice_mode status must mirror the cpal input-device probe"
        );
        assert_ne!(
            status,
            ProviderStatus::DeviceUnprobed,
            "feature-on voice_mode must not return the permanent 'not probed' badge"
        );
        assert_eq!(
            status.is_ok(),
            device_present,
            "is_ok() must track whether an input device was actually found"
        );
    }

    /// The credentials modal validates keys loosely: it relies on the
    /// `wcore_config::env_file::write_env_var` writer to reject invalid
    /// keys + values, but the modal itself does NOT shadow that check.
    /// Empty values are rejected modal-side as a usability guard.
    #[test]
    fn add_credentials_modal_validates_key_format_loosely() {
        // Find an entry whose first env_var is a well-formed key.
        let entry_idx = PROVIDER_CATALOG
            .iter()
            .position(|e| !e.env_vars.is_empty() && !e.deferred)
            .expect("at least one non-deferred entry with env vars");
        let mut modal = CredentialsModal::new(entry_idx);

        // Empty value is rejected without writing.
        let saved = modal.save();
        assert!(!saved, "empty value must not be saved");
        assert!(
            modal.status.to_lowercase().contains("empty"),
            "want 'empty' hint: {}",
            modal.status
        );
        assert!(!modal.last_ok);

        // The var_name resolves to a valid env-var-shaped key (so the
        // writer's strict regex would accept it). The modal trusts the
        // catalog rather than re-validating.
        let var = modal.var_name().expect("first env var");
        assert!(
            var.bytes().next().is_some_and(|b| b.is_ascii_uppercase()),
            "catalog env var name should be ALL_CAPS: {var}"
        );
        for b in var.bytes() {
            assert!(
                b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_',
                "catalog env var contains unexpected char in {var}"
            );
        }
    }

    /// Pressing `p` from the Tier-1 overview opens the providers tier.
    #[test]
    fn p_key_opens_providers_tier_from_overview() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        assert_eq!(surface.tier, Tier::Overview);
        surface.handle_key(ch('p'), &mut app);
        assert_eq!(surface.tier, Tier::Providers);
        // `esc` returns to the overview.
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert_eq!(surface.tier, Tier::Overview);
    }

    /// Issue #16: when the provider catalog overflows a short terminal, moving
    /// focus to the last entry must scroll it into view. Asserts on the
    /// RENDERED buffer (not just the focus index): the last provider's name is
    /// absent when focus is at the top (below the fold) and present once it is
    /// focused (scrolled in). Before the fix the offset was never applied, so
    /// the row stayed permanently off-screen and unreachable.
    #[test]
    fn providers_tier_scrolls_focused_row_into_view_issue_16() {
        fn render_at(surface: &mut ConfigSurface, app: &App, h: u16) -> String {
            let theme = Theme::no_color();
            let mut terminal = Terminal::new(TestBackend::new(80, h)).expect("test terminal");
            terminal
                .draw(|f| surface.render(f, f.area(), app, &theme))
                .expect("render config surface");
            let buf = terminal.backend().buffer();
            let mut out = String::new();
            for y in 0..buf.area.height {
                for x in 0..buf.area.width {
                    out.push_str(buf[(x, y)].symbol());
                }
                out.push('\n');
            }
            out
        }

        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        surface.tier = Tier::Providers;

        // The test only proves something if the catalog overflows the viewport.
        assert!(
            PROVIDER_CATALOG.len() > 10,
            "test assumes the provider catalog overflows a short terminal"
        );
        let last = PROVIDER_CATALOG.len() - 1;
        let last_name = PROVIDER_CATALOG[last].name;

        // Focus at the top: the last row sits below the fold and is not drawn.
        surface.providers_focus = 0;
        let top = render_at(&mut surface, &app, 14);
        assert!(
            !top.contains(last_name),
            "last provider ({last_name}) must be off-screen when focus is at the top:\n{top}"
        );

        // Focus on the last row: it must scroll into view.
        surface.providers_focus = last;
        let bottom = render_at(&mut surface, &app, 14);
        assert!(
            bottom.contains(last_name),
            "last provider ({last_name}) must scroll into view when focused:\n{bottom}"
        );
    }

    /// Pressing Enter on a non-deferred entry with env vars opens the
    /// credentials modal; pressing Esc closes it.
    #[test]
    fn enter_on_provider_row_opens_credentials_modal() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        surface.handle_key(ch('p'), &mut app);
        // The first entry in the catalog has env vars and is not
        // deferred — confirm at the assertion level.
        assert!(!PROVIDER_CATALOG[0].env_vars.is_empty());
        assert!(!PROVIDER_CATALOG[0].deferred);
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(surface.credentials_modal.is_some());
        // Esc closes it without saving.
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(surface.credentials_modal.is_none());
        // The providers tier stays open after a modal close.
        assert_eq!(surface.tier, Tier::Providers);
    }

    /// The providers tier renders the first batch of category headings
    /// (the rest scroll off the 80×30 test terminal but still belong to
    /// the same Paragraph — the catalog ordering is asserted via
    /// `PROVIDER_CATALOG` below).
    #[test]
    fn providers_tier_renders_categories_above_the_fold() {
        let mut app = App::new();
        let mut surface = ConfigSurface::new();
        surface.on_enter(&mut app);
        surface.handle_key(ch('p'), &mut app);
        let text = render_text(&mut surface, &app);
        for cat in ["Search", "Vision", "Audio"] {
            assert!(
                text.contains(cat),
                "missing category {cat} in providers tier:\n{text}"
            );
        }
        // The surface title also renders.
        assert!(
            text.contains("Tools & Providers"),
            "missing surface title in providers tier:\n{text}"
        );
    }

    /// The catalog has at least one entry per category we ship — every
    /// category mentioned in `ProviderEntry::category` shows up at least
    /// once. This pins the catalog wiring against silent drops.
    #[test]
    fn provider_catalog_has_every_category() {
        let cats: std::collections::HashSet<&'static str> =
            PROVIDER_CATALOG.iter().map(|e| e.category).collect();
        for cat in [
            "Search",
            "Vision",
            "Audio",
            "Image",
            "Channels",
            "Home & devices",
            "Database",
            "Meet & OAuth",
            "Provider keys",
        ] {
            assert!(
                cats.contains(cat),
                "PROVIDER_CATALOG is missing category {cat}"
            );
        }
    }
}
