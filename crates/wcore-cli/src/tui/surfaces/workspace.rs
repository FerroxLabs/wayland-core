//! Workspace surface (surfaces 02 + 03) — the 3-pane conversation
//! workspace, idle and agent-live.
//!
//! The workspace is the screen users look at all day: a transcript of
//! user/assistant/system turns plus the in-flight stream and tool-call
//! cards, a right rail (path map, tools panel, activity feed), and a
//! composer at the bottom. It implements the FROZEN `Surface` trait —
//! local UI state (`composer`, `approval_sel`) lives on `WorkspaceSurface`;
//! all shared/conversation state is read from `&App`.
//!
//! Layout follows `mockup.html` surfaces 02 (idle) and 03 (agent-live):
//! a `transcript | rail` body and the composer below. The chrome
//! redesign removed the workspace's own near-top status line — the one
//! bottom status bar the router renders carries all live stats now, so
//! the workspace owns only the transcript, rail, and composer. The
//! Hearth Palette is applied via `Theme`; flat color only.

use ratatui::Frame;
use ratatui::crossterm::event::{
    Event as CrosstermEvent, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind,
};
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState,
};
use tui_input::Input;
use tui_input::backend::crossterm::to_input_request;

use crate::tui::app::{App, ToolCardModel, ToolCardStatus, TurnRole, TurnView};
use crate::tui::permission::components::shell_common::infer_shell_prefix;
use crate::tui::render::markdown::{render_markdown, render_markdown_with_width};
use crate::tui::render::safe_split::last_safe_split_point;
use crate::tui::surfaces::{Surface, SurfaceAction, SurfaceId};
use crate::tui::theme::Theme;
use crate::tui::turn_element::TurnElement;
use crate::tui::widgets::{panel, render_sources, render_streaming_status, wayland_banner};

/// Width of the right rail, in columns (mockup `.rail` is 312px ≈ 34
/// cells; clamped against the area so a narrow terminal still renders).
const RAIL_WIDTH: u16 = 34;

/// Below this body width the right rail auto-hides — a narrow terminal
/// gives the transcript the whole width rather than a cramped rail. This
/// is an *effective* visibility computed alongside `App::rail_visible`;
/// it never overwrites the user's `Ctrl+B` preference flag.
const RAIL_RESPONSIVE_MIN_WIDTH: u16 = 100;

/// Minimum height of the composer block (bordered, 1 hint row, 1 input row,
/// 1 top border, 1 bottom border = 4). The input section grows up to
/// `COMPOSER_INPUT_MAX_ROWS` when the buffer holds multi-line content (F-042).
const COMPOSER_HEIGHT_MIN: u16 = 4;

/// Maximum number of input rows the composer expands to when multi-line
/// content is pasted (F-042). Growth is `min(line_count, MAX) + chrome`.
const COMPOSER_INPUT_MAX_ROWS: u16 = 6;

/// D010 (huge-paste stall): hard cap on the composer buffer, in BYTES. A
/// multi-hundred-KB paste used to be absorbed whole, then re-measured
/// (`lines().count()`) and re-allocated on every keystroke AND every frame,
/// stalling the whole loop. Capping the buffer bounds both costs: a paste
/// that would push the buffer over the cap is truncated (on a UTF-8 char
/// boundary) and a toast tells the user it was clipped. 128 KiB is far above
/// any real prompt a human types yet small enough that a per-frame re-measure
/// of the capped buffer is trivial. The agent receives the (clipped) text on
/// submit; nothing silently grows without bound.
const MAX_COMPOSER_BYTES: usize = 128 * 1024;

/// How many render ticks to show the mode-cycle flash after `⇧Tab` (F-082).
/// At ~30fps, 15 ticks ≈ 0.5 s — long enough to read, short enough not to
/// feel stuck.
const MODE_FLASH_TICKS: u8 = 15;

/// Render ticks per spinner frame. The render loop ticks every ~33ms;
/// advancing the 8-frame braille cycle once every 3 ticks gives a ~10
/// frame/s spin — fast enough to read as "alive", slow enough not to
/// strobe (AUDIT-D D8).
const SPINNER_TICKS_PER_FRAME: u64 = 3;

/// v0.9.1 W1 A — batch-approval queue marker. When the user presses
/// capital `A` (approve all) or `N` (deny all) on a multi-card pending
/// approval, the surface stores one of these and the router drains the
/// remaining cards one per tick. One-action-per-tick is the frozen
/// `SurfaceAction` contract; this enum is the in-surface state that
/// lets the queue drain without reshaping the contract.
#[derive(Debug, Clone, Copy)]
enum ApprovalBatch {
    /// Approve every remaining `AwaitingApproval` card with `Once` scope.
    ApproveAll,
    /// Deny every remaining `AwaitingApproval` card with the user-batch
    /// reason.
    DenyAll,
}

/// The `@`-reference autocomplete popup state — the candidates for the
/// `@…` token currently being typed in the composer and the highlighted
/// row. `None` when the active word is not an `@`-reference.
struct AtCompletion {
    /// The partial `@…` token the candidates were computed for.
    partial: String,
    /// The completion candidates (file/dir/static-keyword rows).
    candidates: Vec<crate::tui::commands::at_refs::Completion>,
    /// The highlighted candidate index.
    selected: usize,
}

/// The main 3-pane conversation workspace. Implements the FROZEN
/// `Surface` trait for `SurfaceId::Workspace`.
pub struct WorkspaceSurface {
    /// The composer text field (surface-local; `tui-input` owns the
    /// buffer + cursor). Shared conversation state stays on `App`.
    composer: Input,
    /// The currently highlighted choice index for an interactive
    /// AskUserQuestion card. Moved by Up/Down/j/k in `handle_approval_key`
    /// and rendered as the live selection marker. Reset to 0 once the
    /// question is answered or dismissed. Unused by generic y/a/n cards.
    approval_sel: usize,
    /// Whether the head approval card's clamped body is expanded (Ctrl+F).
    /// Drives `PermissionContext.expanded` so a large Edit/Write diff or a
    /// big args blob can be read in full before the user approves/denies it.
    /// Reset to false once the card is resolved so the next card starts
    /// collapsed.
    approval_expanded: bool,
    /// The `@`-reference autocomplete popup, open while the active
    /// composer word is an `@…` token (Wave-2 wiring).
    at_completion: Option<AtCompletion>,
    /// How many lines the transcript has been scrolled up from the
    /// bottom. 0 = show the most recent content (default); N = show
    /// content N lines above the bottom (PageUp/PageDown, F-043).
    transcript_scroll: u16,
    /// v0.9.0 W3 D2 (added pre-D1 to unblock the shared build): sticky-
    /// scroll flag — `true` once the user has scrolled up off the
    /// bottom. While `true`, incoming new turns do NOT auto-scroll.
    user_has_scrolled_up: bool,
    /// v0.9.0 W3 D2: most recent transcript body height observed in
    /// `render_transcript`. Used by mouse-wheel + page-jump handlers
    /// that run outside the render path. 0 means "no area known yet".
    last_text_area_height: u16,
    /// v0.9.0 W3 D2: most recent total transcript line count observed
    /// in render. Gates Scrollbar visibility + clamps upward scroll.
    last_total_lines: u16,
    /// v0.9.1.3 F19: most recent rect (x, y, w, h) of the "↓ jump to
    /// latest" hint, in absolute terminal cells. `None` when the hint is
    /// not currently painted (user is at the bottom). `handle_mouse`
    /// hit-tests `MouseEventKind::Down(Left)` against this rect so the
    /// hint becomes a clickable affordance — not just a visual cue.
    last_jump_hint_rect: Option<Rect>,
    /// Countdown ticks remaining for the ⇧Tab mode-cycle flash (F-082).
    /// Set to `MODE_FLASH_TICKS` on each mode cycle; decremented each
    /// render tick; the flash is visible while > 0.
    mode_flash_ticks: u8,
    /// The discriminant of the session mode observed on the previous render
    /// tick. We use `std::mem::discriminant` because `SessionMode` is not
    /// `Copy`/`Clone`. When `app.mode`'s discriminant differs from this, the
    /// workspace fires the mode-cycle flash (F-082).
    prev_mode_disc: std::mem::Discriminant<wcore_protocol::commands::SessionMode>,
    /// True when the user just attempted to submit a message with no model
    /// configured. The banner is shown in the composer area until the next
    /// key press that isn't Enter.
    no_model_banner: bool,
    /// v0.9.1 W1 A: armed batch-approval drain. `Some(ApproveAll)` after
    /// the user pressed capital `A` on a multi-card pending approval,
    /// `Some(DenyAll)` after `N`. `None` (the default) means no batch is
    /// pending. The router drains one card per tick via
    /// [`WorkspaceSurface::pending_batch_decision`].
    approval_batch: Option<ApprovalBatch>,
    /// v0.9.2 W2 (SPEC §1C / F14 regression guard): the `call_id` of the
    /// most recently observed HEAD-of-queue `AwaitingApproval` card. The
    /// single-surface model renders one approval card at a time; when the
    /// head changes (first card surfaces, or a card is approved/denied and
    /// the next pending card becomes head), `tick` re-arms
    /// `App::force_scroll_to_pending_approval` so each new head — which may
    /// land below the fold after a long stream — is pulled into view.
    /// `None` when no card is pending.
    last_head_approval: Option<String>,
    /// v0.9.2 W3 (SPEC §2 Bash / §1D): the active prefix-edit sub-mode.
    /// `Some` while the user is editing the always-allow prefix after
    /// pressing `a` on a Bash/PowerShell card; `None` otherwise. While
    /// `Some`, the head approval card renders the editable buffer (via
    /// `PermissionContext.editable_prefix`) and the keyboard edits the
    /// prefix instead of taking a yes/no decision. Committing (Enter) sends
    /// `ApprovalScope::AlwaysPrefix`; `Esc` backs out to the card.
    prefix_edit: Option<PrefixEdit>,
    /// v0.9.3 — the ambient sub-agent strip rendered above the composer when
    /// any sub-agents are active. S0 stub returns `should_render = false` so the
    /// layout reserves zero rows; W3 fills the strip with real content.
    pub agent_strip: crate::tui::agents::strip::AgentStrip,
    /// D010 (huge-paste stall): cached newline count of the composer buffer.
    /// `render_composer` and the dynamic composer-height calc used to call
    /// `self.composer.value().lines().count()` EVERY frame — O(n) over the
    /// whole buffer per draw. With a huge paste that re-scan stalled the loop.
    /// The count is recomputed ONLY when the buffer actually changes (every
    /// edit goes through [`WorkspaceSurface::set_composer`]), so the hot render
    /// path just reads this `u16`. Stored as line COUNT (>= 1), saturating at
    /// `u16::MAX` (only the `min(_, COMPOSER_INPUT_MAX_ROWS)` clamp reads it).
    composer_lines: u16,
    /// D010: one-shot flag set when the most recent paste was truncated at
    /// [`MAX_COMPOSER_BYTES`]. The user-facing signal is a status-bar toast
    /// raised in `handle_paste`; this internal flag records the same fact for
    /// tests and any future composer indicator. Cleared on the next edit/
    /// keystroke/reset.
    paste_was_capped: bool,
    /// D009 (render-livelock): memoized POST-WRAP visual lines for the
    /// transcript. Building the logical lines (`push_turn` walks every turn,
    /// re-rendering markdown) AND wrapping them into visual rows are both
    /// O(transcript) costs that used to be paid on EVERY frame — the
    /// input-starving part of the livelock. The wrapped lines only change when
    /// the rendered content, the wrap width, or the theme changes, so they are
    /// cached against a cheap [`TranscriptSig`] signature and reused on the idle
    /// frames between content changes (e.g. while the user types a composer
    /// keystroke that does not touch the transcript). On a cache hit the render
    /// path skips the rebuild + re-wrap entirely and materializes ONLY the
    /// viewport-sized window of these lines, so per-frame cost is O(viewport),
    /// not O(transcript), regardless of how large the transcript grows.
    /// `None` until the first render.
    transcript_layout: Option<CachedTranscriptLayout>,
}

/// D009: a cheap signature of everything that can change the transcript's
/// wrapped visual lines. Compared by value to decide whether the cached
/// wrapped lines can be reused instead of re-running the O(transcript) rebuild
/// and re-wrap. It avoids hashing the turn BODIES — the lengths +
/// counts below flip on every real content change (a new turn, more streamed
/// text, a new tool card, a width/theme change), which is exactly when a
/// re-wrap is actually required.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TranscriptSig {
    turns: usize,
    last_turn_elements: usize,
    streaming_len: usize,
    thinking_len: usize,
    tool_cards: usize,
    width: u16,
    /// D009: the animation tick, included ONLY on the UNBOUNDED render path
    /// (streaming while the user has scrolled up). The cache then holds the
    /// FULL transcript, so the tick forces a rebuild each frame to keep the
    /// inline "working" spinner (`render_streaming_status`) moving. On the
    /// bottom-anchored windowed path the live tail (incl. the spinner) is
    /// rebuilt per frame OUTSIDE the cache, so this stays `0` and the SETTLED
    /// cache remains a stable HIT across every streaming frame — the live-turn
    /// half of the livelock fix (a huge turn no longer re-wraps per frame).
    /// `0` for a settled/idle transcript too, so a keystroke windows in
    /// O(viewport).
    streaming_tick: u64,
    /// A cheap rolling fingerprint over every tool card's LIVE identity —
    /// `(call_id, status, output length)` folded into one `u64`. `tool_cards`
    /// (the count) only flips when a card is added, but a `ToolResult` mutates
    /// an EXISTING card's `status` (Running→Ok/Err) and `output` IN PLACE
    /// without changing the count or the turn's element count. Without this
    /// fingerprint that transition leaves the whole settled signature unchanged,
    /// so the settled cache HITs and the completed card stays frozen on
    /// "running…" with no result body. Folding status + output length here
    /// makes the cache invalidate on the status/output transition while staying
    /// O(cards) (cards are few) — no full-buffer hashing. Computed by
    /// [`tool_cards_fingerprint`].
    tool_cards_fp: u64,
    /// Theme identity proxy — the background color stands in for the active
    /// palette so a `/theme` swap that re-styles (and could re-wrap) the
    /// transcript invalidates the cache. `Color` is `Copy + Eq`.
    theme_bg: ratatui::style::Color,
}

/// Fold every tool card's live identity into one cheap `u64` for
/// [`TranscriptSig::tool_cards_fp`]. The fold mixes each card's `call_id`,
/// `status` discriminant, and output BYTE length — the three things that change
/// across a tool's `Running → Ok/Err` lifecycle and its streamed output growth.
/// O(cards) and allocation-free (an FNV-1a-style mix), so it stays cheap to
/// recompute every frame even though it walks every card. It is a change
/// DETECTOR, not a cryptographic hash: a collision would at worst miss a
/// repaint, but status+len changing on a real transition virtually always
/// flips the fold.
fn tool_cards_fingerprint(cards: &[crate::tui::app::ToolCardModel]) -> u64 {
    // FNV-1a 64-bit constants.
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    #[inline]
    fn mix(hash: &mut u64, b: u8) {
        *hash ^= b as u64;
        *hash = hash.wrapping_mul(PRIME);
    }
    let mut hash = OFFSET;
    for card in cards {
        for b in card.call_id.as_bytes() {
            mix(&mut hash, *b);
        }
        mix(&mut hash, card.status as u8);
        // Output presence + length: a `None → Some("…")` transition and any
        // length change (streamed `ToolChunk` growth) both flip the fold. The
        // length is enough — re-wrapping is keyed on content existing, and the
        // status change already covers the common Running→Ok flip.
        let out_len = card.output.as_ref().map(|s| s.len()).unwrap_or(0);
        for b in out_len.to_le_bytes() {
            mix(&mut hash, b);
        }
    }
    hash
}

/// D009: the cached, wrapped transcript visual lines keyed on the
/// [`TranscriptSig`] they were computed for. `lines` is the materialized
/// POST-WRAP row list — the render windows a viewport-sized slice out of it
/// without re-walking the turns. `wrapped_total` is `lines.len()` cached as a
/// `u16` for the scroll math; the two are always kept in sync.
///
/// On the steady-state / unbounded paths this holds the FULL transcript. On the
/// bottom-anchored live-stream path it holds only the SETTLED turns (the live
/// tail is wrapped per frame and concatenated below at render time), so a turn
/// streaming a huge body never lands its growing text in this cache.
#[derive(Debug, Clone)]
struct CachedTranscriptLayout {
    sig: TranscriptSig,
    wrapped_total: u16,
    /// The pre-wrapped visual rows. Rendered with wrapping DISABLED (they are
    /// already wrapped to `sig.width`), so this layout — not a ratatui re-wrap
    /// — is the single source of truth for the visual line count and scroll.
    lines: Vec<Line<'static>>,
}

/// v0.9.2 W3: the live state of the Bash/PowerShell always-allow
/// prefix-edit sub-mode. Entered when the user presses `a` on a shell
/// card; the buffer is prefilled with [`infer_shell_prefix`] of the
/// command. Committing sends `ApprovalScope::AlwaysPrefix { prefix }` for
/// `call_id`; backing out (`Esc`) drops this and returns to the card.
struct PrefixEdit {
    /// The pending shell card this prefix scopes — the `Approve` target.
    call_id: String,
    /// The editable prefix buffer (prefilled from `infer_shell_prefix`).
    /// `tui-input` owns the text + caret so editing matches the composer.
    buffer: Input,
}

/// Whether a tool name is a shell-command tool that gets the editable
/// always-allow prefix sub-mode (SPEC §2 Bash / PowerShell). Only these
/// route through `ApprovalScope::AlwaysPrefix`; every other tool keeps the
/// plain `Always` affordance.
fn is_shell_tool(tool_name: &str) -> bool {
    matches!(tool_name, "Bash" | "PowerShell")
}

impl Default for WorkspaceSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceSurface {
    /// Construct an empty workspace: a blank composer, the first
    /// approval option preselected.
    pub fn new() -> Self {
        Self {
            composer: Input::default(),
            approval_sel: 0,
            approval_expanded: false,
            at_completion: None,
            transcript_scroll: 0,
            user_has_scrolled_up: false,
            last_text_area_height: 0,
            last_total_lines: 0,
            last_jump_hint_rect: None,
            mode_flash_ticks: 0,
            prev_mode_disc: std::mem::discriminant(&wcore_protocol::commands::SessionMode::Default),
            no_model_banner: false,
            approval_batch: None,
            last_head_approval: None,
            prefix_edit: None,
            // v0.9.3 S0.8 — strip is a no-op stub; W3 fills it.
            agent_strip: crate::tui::agents::strip::AgentStrip::default(),
            // D010 — an empty composer is one (empty) line; nothing capped yet.
            composer_lines: 1,
            paste_was_capped: false,
            // D009 — no transcript wrapped yet; first render fills the cache.
            transcript_layout: None,
        }
    }

    /// D010 (huge-paste stall): the single seam through which the composer
    /// buffer is replaced wholesale (paste, history-recall, @-completion,
    /// reset). It refreshes the cached [`WorkspaceSurface::composer_lines`]
    /// count ONCE here so the per-frame render never re-scans the buffer.
    /// `recount_composer` covers the incremental-edit path (`tui-input`
    /// mutates the buffer in place, so we recount after handing it the key).
    fn set_composer(&mut self, input: Input) {
        self.composer = input;
        self.recount_composer();
    }

    /// D010: recompute the cached composer line count from the current buffer.
    /// Called after any mutation of `self.composer` (both the wholesale
    /// [`set_composer`](WorkspaceSurface::set_composer) path and the in-place
    /// `tui-input` keystroke path). O(n) once per edit, NOT once per frame.
    fn recount_composer(&mut self) {
        let n = self.composer.value().lines().count().max(1);
        self.composer_lines = n.min(u16::MAX as usize) as u16;
    }

    /// The whitespace-delimited word the composer caret currently sits
    /// in — used to detect an in-progress `@`-reference.
    fn active_word(&self) -> &str {
        self.composer
            .value()
            .split_whitespace()
            .next_back()
            .unwrap_or("")
    }

    /// Recompute the `@`-completion popup against the current composer
    /// buffer. Opens the popup when the active word starts with `@` and
    /// has at least one candidate; closes it otherwise.
    fn refresh_at_completion(&mut self) {
        let word = self.active_word().to_string();
        if !word.starts_with('@') {
            self.at_completion = None;
            return;
        }
        // Completions are resolved against the current working dir — the
        // workspace the agent operates in.
        let root = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        let candidates = crate::tui::commands::at_refs::complete(&word, &root);
        if candidates.is_empty() {
            self.at_completion = None;
        } else {
            self.at_completion = Some(AtCompletion {
                partial: word,
                candidates,
                selected: 0,
            });
        }
    }

    /// Handle a key while the `@`-completion popup is open. Returns
    /// `Some(action)` if the key was consumed by the popup, `None` to let
    /// the normal composer path handle it.
    fn handle_at_completion_key(&mut self, key: KeyEvent) -> Option<SurfaceAction> {
        let comp = self.at_completion.as_mut()?;
        match key.code {
            KeyCode::Up => {
                comp.selected = comp.selected.saturating_sub(1);
                Some(SurfaceAction::None)
            }
            KeyCode::Down => {
                comp.selected = (comp.selected + 1).min(comp.candidates.len() - 1);
                Some(SurfaceAction::None)
            }
            // Tab accepts the highlighted candidate: replace the active
            // `@…` word with the candidate's insert text.
            KeyCode::Tab => {
                let chosen = comp.candidates.get(comp.selected)?.insert.clone();
                let partial = comp.partial.clone();
                self.accept_at_completion(&partial, &chosen);
                self.at_completion = None;
                Some(SurfaceAction::None)
            }
            KeyCode::Esc => {
                self.at_completion = None;
                Some(SurfaceAction::None)
            }
            _ => None,
        }
    }

    /// Replace the trailing `@…` partial in the composer with the chosen
    /// completion text.
    fn accept_at_completion(&mut self, partial: &str, chosen: &str) {
        let value = self.composer.value();
        // The partial is the last word — swap it for the chosen text.
        if let Some(stripped) = value.strip_suffix(partial) {
            let replaced = format!("{stripped}{chosen}");
            // D010: route through the seam so the cached line count refreshes.
            self.set_composer(Input::new(replaced));
        }
    }

    /// The first tool card awaiting the user's approval decision, if any.
    /// The approval prompt + diff-as-approval card render only when this
    /// is `Some`.
    fn pending_approval(app: &App) -> Option<&ToolCardModel> {
        app.session
            .tool_cards
            .iter()
            .find(|c| c.status == ToolCardStatus::AwaitingApproval)
    }
}

impl Surface for WorkspaceSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::Workspace
    }

    /// v0.9.1 W2 cycle-2 HIGH 1: drain one card off any armed batch-
    /// approval queue per tick. When the user presses `A` / `N` on a
    /// multi-card pending approval, the first card is approved/denied
    /// synchronously via the key handler and `approval_batch` is armed;
    /// each subsequent tick this method fires one more `Approve`/`Deny`
    /// until the queue is drained. The one-action-per-tick cadence gives
    /// the user a visible progress beat as each card processes.
    fn tick(&mut self, app: &mut App) -> SurfaceAction {
        // v0.9.2 W2 (SPEC §1C / F14 regression guard): single-surface
        // one-card queue. Re-arm the force-scroll trigger on EACH new
        // head-of-queue `AwaitingApproval` card — not just the first. The
        // protocol bridge already sets the flag on the initial
        // `ApprovalRequired`; this catches the dequeue case (a card is
        // approved/denied and the NEXT pending card becomes head), so the
        // newly-surfaced head — which may have landed below the fold after
        // a long markdown stream — is pulled into view. The deleted sticky
        // strip used to provide this visibility; per-card scroll-to-pending
        // now does.
        let head = app
            .session
            .tool_cards
            .iter()
            .find(|c| c.status == ToolCardStatus::AwaitingApproval)
            .map(|c| c.call_id.clone());
        if head != self.last_head_approval {
            if head.is_some() {
                app.force_scroll_to_pending_approval = true;
            }
            // v0.9.2 W3: the head card changed (resolved, or a new card
            // surfaced) — any prefix-edit sub-mode was scoped to the OLD
            // head, so drop it. Without this a stale `prefix_edit` would
            // keep editing a card the user can no longer see.
            if self
                .prefix_edit
                .as_ref()
                .is_some_and(|e| head.as_deref() != Some(e.call_id.as_str()))
            {
                self.prefix_edit = None;
            }
            self.last_head_approval = head;
        }

        // v0.9.1.2 F14: consume the one-shot force-scroll trigger. We snap
        // to the bottom anchor so the awaiting card lands in view; the
        // sticky-up lock in `render_transcript` (held while any card is
        // `AwaitingApproval`) then keeps the position so subsequent
        // streaming text cannot push it off-screen.
        if app.force_scroll_to_pending_approval {
            self.user_has_scrolled_up = false;
            self.transcript_scroll = 0;
            app.force_scroll_to_pending_approval = false;
        }
        self.pending_batch_decision(app)
    }

    fn handle_paste(&mut self, text: String, app: &mut App) {
        // F-041: insert the full paste blob into the composer verbatim so
        // embedded newlines never trigger a submit. The cursor lands at the
        // end of the insertion (matching the tui-input `Input::new` contract).
        let current = self.composer.value().to_string();
        // Normalise CRLF → LF so the composer stores a uniform string.
        let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut combined = format!("{current}{normalised}");
        // D010 (huge-paste stall): cap the buffer so a multi-hundred-KB paste
        // cannot be absorbed whole (which then re-scanned + re-allocated on
        // every keystroke/frame and stalled the loop). Truncate on a UTF-8
        // char boundary at or below the byte cap; surface the clip as a toast
        // (the status bar renders it for a short dwell) so it is never a silent
        // drop, and flag it internally too.
        if combined.len() > MAX_COMPOSER_BYTES {
            let mut cut = MAX_COMPOSER_BYTES;
            while cut > 0 && !combined.is_char_boundary(cut) {
                cut -= 1;
            }
            combined.truncate(cut);
            self.paste_was_capped = true;
            // The status bar reads `app.toast` (auto-dismissed via `toast_at`).
            app.toast = Some(format!(
                "Paste was too large and was clipped to {} KB",
                MAX_COMPOSER_BYTES / 1024
            ));
            app.toast_at = Some(std::time::Instant::now());
        } else {
            self.paste_was_capped = false;
        }
        // Route through `set_composer` so the cached line count refreshes once
        // here instead of being re-scanned per frame (D010).
        self.set_composer(Input::new(combined));
        self.at_completion = None; // a paste closes any open @-completion popup
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Detect a mode change from the previous tick (F-082). The router
        // intercepts ⇧Tab before the workspace sees the key, so we diff
        // `app.mode`'s discriminant against the stored one to know when to
        // fire the flash.
        let curr_disc = std::mem::discriminant(&app.mode);
        if curr_disc != self.prev_mode_disc {
            self.prev_mode_disc = curr_disc;
            self.mode_flash_ticks = MODE_FLASH_TICKS;
        }
        // Decrement the mode-flash countdown each render tick so the
        // ⇧Tab feedback times out automatically (F-082).
        self.mode_flash_ticks = self.mode_flash_ticks.saturating_sub(1);

        // Compute the composer height dynamically so multi-line pasted
        // content gets up to COMPOSER_INPUT_MAX_ROWS input rows (F-042).
        // chrome = 1 top border + 1 hint row = 2; the minimum is always
        // met so a one-line composer never collapses.
        let composer_height = {
            // D010: read the CACHED line count (refreshed on edit) instead of
            // re-scanning the whole buffer every frame.
            let content_lines = self.composer_lines.max(1);
            let input_rows = content_lines.min(COMPOSER_INPUT_MAX_ROWS);
            // 1 top border + N input rows + 1 hint row.
            (input_rows + 2).max(COMPOSER_HEIGHT_MIN)
        };

        // The body (transcript | rail) takes the slack; the composer
        // sits below it. The status bar is the router's job — it renders
        // one bottom bar below this whole surface, so the workspace no
        // longer paints its own near-top status line.
        //
        // v0.9.2 W2 (SPEC §0 #1): SINGLE-SURFACE. The v0.9.1.2 sticky
        // APPROVAL STRIP between the body and the composer is DELETED —
        // there is exactly one approval surface, the inline card in the
        // transcript. The strip's only job was visibility; that is now
        // done by per-card scroll-to-pending (see `tick`, which re-arms
        // `App::force_scroll_to_pending_approval` on each new head-of-queue
        // card so a card below the fold is pulled into view).
        // v0.9.3 S0.8 — reserve a 1-row strip slot above the composer when the
        // strip wants to render. `should_render` is a no-op stub through S0 (it
        // returns `false`), so this is a behaviour-preserving change for v0.9.2.
        // W3 fills the strip; the layout already routes a 1-row slice when the
        // stub flips to `true`. Note: `app.anim` (NOT `app.anim_clock`) is the
        // canonical field name (B1 — verified at `app.rs:125`).
        let want_strip = self.agent_strip.should_render(app, app.frame_tick);
        let strip_height = if want_strip { 1 } else { 0 };
        let [body_area, strip_area, composer_area] = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(strip_height),
            Constraint::Length(composer_height),
        ])
        .areas(area);

        self.render_body(frame, body_area, app, theme);
        self.agent_strip
            .render(frame, strip_area, app, theme, &app.anim);
        self.render_composer(frame, composer_area, app, theme);
        // The `@`-completion popup overlays the bottom of the body, just
        // above the composer, so the candidates are visible while typing.
        if self.at_completion.is_some() {
            self.render_at_completion(frame, body_area, theme);
        }
    }

    fn handle_key(&mut self, key: KeyEvent, app: &mut App) -> SurfaceAction {
        // Any key press (including a repeat Enter) dismisses the no-model
        // banner so it doesn't linger after the user takes corrective action.
        self.no_model_banner = false;

        // While a tool call is awaiting approval the approval prompt owns
        // the keyboard — it is the one decision the user must make before
        // the turn can proceed.
        if let Some(card) = WorkspaceSurface::pending_approval(app) {
            let call_id = card.call_id.clone();
            // v0.9.2 W3: the `a`-then-edit-prefix sub-mode (SPEC §2 Bash)
            // needs the tool kind + command to decide whether to open the
            // editable always-allow prefix; capture both off the head card
            // before handing the key to the approval handler.
            let tool_name = card.tool_name.clone();
            let command = card.summary.clone();
            // v0.9.2 W11-integ (SPEC §2 #10): AskUserQuestion is a Q&A, not
            // a yes/no — the handler needs the raw args to read the choice
            // list off `input_pretty` (same source the component renders).
            let input_pretty = card.input_pretty.clone();
            let action =
                self.handle_approval_key(key, call_id, &tool_name, &command, &input_pretty);
            // Once a card is resolved (approved/denied), reset the expand
            // toggle so the next pending card starts collapsed.
            if matches!(
                action,
                SurfaceAction::Approve { .. } | SurfaceAction::Deny { .. }
            ) {
                self.approval_expanded = false;
            }
            return action;
        }

        // While the `@`-completion popup is open it owns Tab / arrows /
        // Esc so the user can pick a reference.
        if self.at_completion.is_some()
            && let Some(action) = self.handle_at_completion_key(key)
        {
            return action;
        }

        // FIX-7 — a bare Tab here falls through to the Router's global
        // tab-switch (the documented "Tab next tab"). The `@`-completion
        // Tab-accept was handled above; the old reasoning-rail Tab-stepping was
        // removed (it hijacked Tab whenever any reasoning turn existed).

        // `Ctrl+B` toggles the right rail (path map · tools · activity).
        // Hiding it gives the transcript the full body width; a chord, so
        // it never collides with composer typing.
        if key.code == KeyCode::Char('b') && key.modifiers.contains(KeyModifiers::CONTROL) {
            app.rail_visible = !app.rail_visible;
            return SurfaceAction::None;
        }

        // `Ctrl+E` toggles compact-vs-full tool-card output for every
        // card on screen at once (v0.9.0 W3 D1, action
        // `toolcard.toggle_compact`). Global per-session, not per-card —
        // the user opens up everything or closes it down in one chord.
        if key.code == KeyCode::Char('e') && key.modifiers.contains(KeyModifiers::CONTROL) {
            app.session.compact_tool_output = !app.session.compact_tool_output;
            return SurfaceAction::None;
        }

        // v0.9.3 W7.1 — Open the agent list (AgentNav surface).
        //
        // Three keybind paths so the chord works on every terminal:
        //  * `Alt+a` — what iTerm2 / Kitty / Wezterm send.
        //  * `Ctrl+]` — universal fallback for terminals that swallow
        //    Alt+letter (Windows Terminal / tmux default profile / etc).
        //  * `'å'` (U+00E5) — what macOS Terminal.app sends for Option+A
        //    by default (the composed-key path; users who enabled "Use
        //    Option as Meta" get the Alt+a path above instead).
        //
        // Pushes the workspace onto `App::surface_stack` with the current
        // `transcript_scroll`, so an `Esc` on the AgentNav surface pops
        // back here with the same view (S0.6 contract). Per-surface
        // selection / scroll-mode state is not captured because F4 +
        // mouse_capture are global flags on `app`, not per-surface.
        let is_alt_a = key.code == KeyCode::Char('a') && key.modifiers.contains(KeyModifiers::ALT);
        let is_ctrl_bracket =
            key.code == KeyCode::Char(']') && key.modifiers.contains(KeyModifiers::CONTROL);
        let is_composed_ring = key.code == KeyCode::Char('å');
        if is_alt_a || is_ctrl_bracket || is_composed_ring {
            app.surface_stack
                .push(crate::tui::surfaces::SurfaceStackEntry {
                    id: SurfaceId::Workspace,
                    scroll_offset: self.transcript_scroll,
                });
            return SurfaceAction::Switch(SurfaceId::AgentNav);
        }

        // v0.9.2 W11-integ (SPEC §3 S21): toggle the collapsed/expanded
        // reasoning projection. W7 built the projection
        // (`reasoning_collapsed_lines_themed` keyed by turn index, reading
        // `App::reasoning_expanded`) but left the toggle unwired. There is
        // no per-element focus model in this surface (the transcript is a
        // flat line buffer; F4 is the mouse-capture toggle, not selection),
        // so the documented minimal-viable binding flips the MOST-RECENT
        // assistant turn's entry — the `Ctrl+R` chord mirrors `Ctrl+E`
        // (tool cards) / `Ctrl+B` (rail) and cannot collide with composer
        // typing. When a render site lands per-turn reasoning + focus, the
        // same map flip moves to the focused turn index with no contract
        // change (the map stays keyed by turn index).
        if key.code == KeyCode::Char('r') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if let Some(turn_idx) = Self::most_recent_assistant_turn(app) {
                let entry = app.reasoning_expanded.entry(turn_idx).or_insert(false);
                *entry = !*entry;
            }
            return SurfaceAction::None;
        }

        // v0.9.0 W1 B10 (P-B1 closure): Ctrl+Space → toggle voice
        // capture. Routes via `/voice` so it hits the same dispatch
        // path as every other tool (and so the TUI surface stays
        // presentational — the voice_mode tool itself owns the
        // start/stop state). The TUI never holds a direct ref to the
        // tool registry.
        if key.code == KeyCode::Char(' ') && key.modifiers.contains(KeyModifiers::CONTROL) {
            return SurfaceAction::Command("/voice".to_string());
        }

        // `Ctrl+D` on an empty composer — the universal terminal "I'm done"
        // chord (F-057). On a non-empty composer it is a no-op (matches
        // bash/zsh: EOT only fires when the line is empty).
        if key.code == KeyCode::Char('d') && key.modifiers.contains(KeyModifiers::CONTROL) {
            if self.composer.value().is_empty() {
                return SurfaceAction::Quit;
            }
            return SurfaceAction::None;
        }

        // Transcript scrollback (F-043 + D2/v0.9.0):
        //  * `PageUp` / `PageDown` jump `area.height - 2` lines (2 rows of
        //    overlap so the user keeps context). Falls back to 5 when no
        //    area has been observed yet (pre-first-render).
        //  * `Shift+Up` / `Shift+Down` move ONE line — fine-grained scroll
        //    that does NOT collide with the composer's own cursor movement.
        //  * `End` / `Ctrl+End` jump to the bottom and re-arm autoscroll.
        let page_jump: u16 = if self.last_text_area_height > 2 {
            self.last_text_area_height - 2
        } else {
            5
        };
        match key.code {
            KeyCode::PageUp => {
                self.scroll_up_by(page_jump);
                return SurfaceAction::None;
            }
            KeyCode::PageDown => {
                self.scroll_down_by(page_jump);
                return SurfaceAction::None;
            }
            KeyCode::Up if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.scroll_up_by(1);
                return SurfaceAction::None;
            }
            KeyCode::Down if key.modifiers.contains(KeyModifiers::SHIFT) => {
                self.scroll_down_by(1);
                return SurfaceAction::None;
            }
            // v0.9.1.2 F8: bare Up on an empty composer is bash-style
            // history recall — the most recent submitted prompt jumps
            // into the composer, and repeat presses walk further back.
            // Down walks forward toward the present and clears the
            // composer back to empty when it falls off the newest entry
            // (so the user can always escape history by walking past
            // the end). Slash commands ARE included in history because
            // re-running `/cost` or `/doctor` is a real workflow.
            //
            // Falls through to single-line transcript scroll (the v0.9.1
            // W1 A affordance below) when there is no history yet
            // (cold-start before any prompt has been submitted), so the
            // "browse scrollback with the arrows" hint still works on a
            // brand-new session with nothing to recall.
            KeyCode::Up
                if key.modifiers.is_empty()
                    && self.composer.value().is_empty()
                    && !app.recent_user_prompts.is_empty() =>
            {
                let next = match app.history_cursor {
                    None => app.recent_user_prompts.len() - 1,
                    Some(0) => 0, // already at the oldest entry — stay put
                    Some(i) => i - 1,
                };
                if let Some(prompt) = app.recent_user_prompts.get(next) {
                    self.set_composer(Input::new(prompt.clone()));
                    app.history_cursor = Some(next);
                }
                return SurfaceAction::None;
            }
            KeyCode::Down if key.modifiers.is_empty() && app.history_cursor.is_some() => {
                let cur = app
                    .history_cursor
                    .expect("history_cursor guarded Some by match arm");
                if cur + 1 >= app.recent_user_prompts.len() {
                    // Walked past the newest entry — exit history mode
                    // and return the composer to a fresh empty state so
                    // the next Up lands back on the most recent prompt.
                    self.set_composer(Input::default());
                    app.history_cursor = None;
                } else if let Some(prompt) = app.recent_user_prompts.get(cur + 1) {
                    self.set_composer(Input::new(prompt.clone()));
                    app.history_cursor = Some(cur + 1);
                }
                return SurfaceAction::None;
            }
            // v0.9.1 W1 A (B1 fix): bare Up/Down scroll the transcript
            // ONE line when the composer is empty — "browse scrollback
            // with the arrows" is the discoverable affordance. When the
            // composer has typed content, arrows pass through to the
            // composer's tui-input handler for cursor movement. The F8
            // history-recall arm above takes precedence as soon as the
            // history ring has any entries; this arm only fires on the
            // cold-start case where no prompt has been submitted yet.
            KeyCode::Up if key.modifiers.is_empty() && self.composer.value().is_empty() => {
                self.scroll_up_by(1);
                return SurfaceAction::None;
            }
            KeyCode::Down if key.modifiers.is_empty() && self.composer.value().is_empty() => {
                self.scroll_down_by(1);
                return SurfaceAction::None;
            }
            KeyCode::End => {
                self.jump_to_bottom();
                return SurfaceAction::None;
            }
            // v0.9.1.3 F19: `Home` jumps to the very top of the
            // transcript (oldest content). Symmetric with `End` (jump
            // to the bottom). Discoverable via the status hint when
            // the user has scrolled up. Only fires when the composer
            // is empty — typed text needs `Home` for cursor-to-start.
            KeyCode::Home if self.composer.value().is_empty() => {
                self.jump_to_top();
                return SurfaceAction::None;
            }
            _ => {}
        }

        match key.code {
            // Submit the composer. A `/…` line is a slash command
            // (routed to the registry); anything else is a message.
            KeyCode::Enter => {
                let text = self.composer.value().trim().to_string();
                if text.is_empty() {
                    SurfaceAction::None
                } else if !text.starts_with('/') && app.config.model.is_empty() {
                    // No model configured — block the submit and surface a
                    // clear error so the user is not left with an opaque
                    // "builder error" from the provider HTTP layer. Slash
                    // commands are still allowed (the `!starts_with('/')`
                    // guard above), so the D002 in-app recovery — `/model`,
                    // `/setup`, `/config` — works without quitting.
                    self.no_model_banner = true;
                    SurfaceAction::None
                } else {
                    self.composer.reset();
                    // D010: a reset empties the buffer — refresh the cached
                    // line count + clear the paste-clip flag.
                    self.recount_composer();
                    self.paste_was_capped = false;
                    self.at_completion = None;
                    // Reset transcript scroll so the user's own message
                    // and the incoming reply are immediately visible.
                    // Submitting is an explicit "show the next turn"
                    // signal — also clear the sticky-up flag so autoscroll
                    // resumes for the incoming reply.
                    self.transcript_scroll = 0;
                    self.user_has_scrolled_up = false;
                    if text.starts_with('/') {
                        // Slash commands route immediately even mid-turn
                        // (`/cancel` must work while streaming).
                        SurfaceAction::Command(text)
                    } else if app.session.streaming_active {
                        // A turn is in flight — do NOT drop the message
                        // (AUDIT-D D3). Queue it: the router flushes the
                        // queued message to the engine when the current
                        // turn ends. This is the real "queue a message"
                        // the composer hint advertises.
                        SurfaceAction::QueueMessage(text)
                    } else {
                        SurfaceAction::SendMessage(text)
                    }
                }
            }
            // `/` on an empty composer opens the command palette overlay
            // — the discoverable home for slash commands. Typed mid-line
            // it is just a literal character (a path can contain `/`).
            KeyCode::Char('/') if self.composer.value().is_empty() => {
                SurfaceAction::OpenOverlay(SurfaceId::Palette)
            }
            // v0.9.1.1 H5: `?` on an empty composer runs `/help` —
            // the global help binding declared in `keybind.rs` was
            // being eaten by the composer's `_ => composer.handle(req)`
            // fallback. Typed mid-line `?` is just a literal character
            // (a user might be asking a question in prose); only the
            // empty-composer case escalates to help. Sub-Agents /
            // Plugins / Diagnostics / etc. have no text field so the
            // router itself can layer the same binding later if needed.
            KeyCode::Char('?') if self.composer.value().is_empty() => {
                SurfaceAction::Command("/help".to_string())
            }
            // `Esc` while streaming is the cancel affordance — emit the
            // engine cancel verb (the router maps it to `TuiEngine::cancel`
            // via the `/cancel` command? no — cancellation routes via the
            // dedicated path below).
            KeyCode::Esc if app.session.streaming_active => {
                // Cancellation is routed as a command the router
                // recognises; the workspace itself stays presentational.
                SurfaceAction::Command("/cancel".to_string())
            }
            // Otherwise feed the key to the composer, then refresh the
            // `@`-completion popup against the new buffer.
            _ => {
                if let Some(req) = to_input_request(&CrosstermEvent::Key(key)) {
                    // v0.9.1.2 F8: any typing exits history-recall mode
                    // so the next bare Up lands back on the most recent
                    // entry (instead of stepping older from wherever the
                    // cursor was sitting). Cleared BEFORE we mutate the
                    // composer so the recall arm sees a fresh `None` on
                    // the next keystroke.
                    app.history_cursor = None;
                    self.composer.handle(req);
                    // D010: the buffer changed in place — refresh the cached
                    // line count so the per-frame render reads it instead of
                    // re-scanning. A normal keystroke clears any stale
                    // paste-clip note.
                    self.recount_composer();
                    self.paste_was_capped = false;
                }
                self.refresh_at_completion();
                SurfaceAction::None
            }
        }
    }

    /// Handle a mouse event — D2/v0.9.0 wires scroll-wheel ticks to
    /// transcript scrollback. Each wheel tick scrolls 3 lines (the
    /// convention modern terminal apps inherit from browser wheel-tick
    /// semantics).
    ///
    /// v0.9.1.3 F19: a left-button click is hit-tested against the
    /// "↓ jump to latest" hint rect stored on the last render. The
    /// hint is a visible cue that the user is scrolled up; making it
    /// clickable closes the loop (users who reach for the mouse expect
    /// to click an obvious bottom-right "go to latest" affordance, like
    /// every modern chat UI). Motion / drag events are still ignored.
    fn handle_mouse(&mut self, mouse: MouseEvent, _app: &mut App) -> SurfaceAction {
        use ratatui::crossterm::event::MouseButton;
        match mouse.kind {
            MouseEventKind::ScrollUp => self.scroll_up_by(3),
            MouseEventKind::ScrollDown => self.scroll_down_by(3),
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(rect) = self.last_jump_hint_rect
                    && mouse.column >= rect.x
                    && mouse.column < rect.x + rect.width
                    && mouse.row >= rect.y
                    && mouse.row < rect.y + rect.height
                {
                    self.jump_to_bottom();
                }
            }
            _ => {}
        }
        SurfaceAction::None
    }

    /// v0.9.3 W8 H3-integration: restore the transcript scroll captured at
    /// push time. Mirrors `AgentTranscriptSurface::restore_scroll`
    /// (`agent_transcript.rs:258`). Without this impl, the `scroll_offset`
    /// captured in `SurfaceStackEntry` at `workspace.rs:529` was semantic
    /// dead weight — the default no-op `Surface::restore_scroll` did nothing
    /// and behaviour was correct only incidentally via the H6 `SurfaceCache`
    /// preserving the whole `WorkspaceSurface` struct (incl. `transcript_scroll`).
    /// If the cache ever drops a slot for memory reasons, the Pop restoration
    /// would silently lose the scroll. This impl makes the contract explicit.
    ///
    /// The user-driven `user_has_scrolled_up` flag is set when the restored
    /// offset is non-zero so the sticky-to-bottom auto-scroll doesn't snap
    /// the user back to the latest line — they explicitly returned to a
    /// known scroll position via Pop, not to "live tail".
    fn restore_scroll(&mut self, offset: u16) {
        self.transcript_scroll = offset;
        self.user_has_scrolled_up = offset > 0;
    }

    /// D039 — the Workspace owns a bare Tab only when an open `@`-completion
    /// popup claims it (Tab accepts the highlighted candidate). Otherwise Tab
    /// falls through to the Router's global tab-switch — the documented
    /// "Tab next tab" hint, which must always hold (FIX-2/FIX-7). The earlier
    /// reasoning-rail Tab-stepping was removed: it claimed Tab whenever ANY
    /// reasoning turn existed, silently breaking tab-switching after the first
    /// agent "thinking" turn, and it had no visual focus indicator.
    fn owns_tab(&self, _app: &App) -> bool {
        self.at_completion.is_some()
    }

    /// D038 — the Workspace always owns `?`: its composer either types `?` as
    /// literal prose or, on an empty composer, escalates to `/help`. Either
    /// way the surface, not the Router's help overlay, owns the key.
    fn consumes_help_key(&self, _app: &App) -> bool {
        true
    }
}

impl WorkspaceSurface {
    /// v0.9.2 W11-integ (SPEC §3 S21): index of the most-recent assistant
    /// turn, used as the toggle target for the reasoning-collapse chord
    /// (`Ctrl+R`). `reasoning_expanded` is keyed by turn index, so this is
    /// the key the chord flips. Returns `None` on an empty transcript or
    /// when no assistant turn exists yet (cold-start / user-only history).
    fn most_recent_assistant_turn(app: &App) -> Option<usize> {
        app.session
            .turns
            .iter()
            .enumerate()
            .rev()
            .find(|(_, t)| t.role == crate::tui::app::TurnRole::Assistant)
            .map(|(idx, _)| idx)
    }

    /// v0.9.2 W11-integ (SPEC §2 #10): parse the AskUserQuestion choice
    /// labels out of the card's `input_pretty` JSON. Mirrors the shapes the
    /// `AskUserQuestionComponent` renders (the component owns the display;
    /// this owns the answer index → label mapping for arrow-nav): top-level
    /// `choices`/`options`, or CC-style `questions[0].options`; each element
    /// is a bare string or an object preferring `label`/`header`/`value`.
    /// Returns labels in render order so `approval_sel` indexes directly.
    fn parse_ask_user_choices(input_pretty: &str) -> Vec<String> {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(input_pretty.trim()) else {
            return Vec::new();
        };
        let scope = value
            .get("questions")
            .and_then(serde_json::Value::as_array)
            .and_then(|q| q.first())
            .unwrap_or(&value);
        scope
            .get("choices")
            .or_else(|| scope.get("options"))
            .and_then(serde_json::Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|item| {
                        if let Some(s) = item.as_str() {
                            let s = s.trim();
                            return (!s.is_empty()).then(|| s.to_string());
                        }
                        item.as_object().and_then(|obj| {
                            let label = obj
                                .get("label")
                                .and_then(serde_json::Value::as_str)
                                .or_else(|| obj.get("header").and_then(serde_json::Value::as_str))
                                .or_else(|| obj.get("value").and_then(serde_json::Value::as_str))
                                .unwrap_or("")
                                .trim();
                            (!label.is_empty()).then(|| label.to_string())
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// v0.9.0 W3 D2 stub (added by D1 to unblock the shared build):
    /// scroll transcript UP by `n` lines. D2 owns the real impl with
    /// the sticky flag + clamp on `last_total_lines - last_text_area_height`.
    fn scroll_up_by(&mut self, n: u16) {
        let max_up = self
            .last_total_lines
            .saturating_sub(self.last_text_area_height);
        let raw = self.transcript_scroll.saturating_add(n);
        self.transcript_scroll = if max_up == 0 { raw } else { raw.min(max_up) };
        if self.transcript_scroll > 0 {
            self.user_has_scrolled_up = true;
        }
    }

    /// v0.9.0 W3 D2 stub: scroll transcript DOWN by `n` lines, clearing
    /// the sticky flag when the bottom is reached.
    fn scroll_down_by(&mut self, n: u16) {
        self.transcript_scroll = self.transcript_scroll.saturating_sub(n);
        if self.transcript_scroll == 0 {
            self.user_has_scrolled_up = false;
        }
    }

    /// v0.9.0 W3 D2 stub: jump to the bottom of the transcript and
    /// re-arm autoscroll.
    fn jump_to_bottom(&mut self) {
        self.transcript_scroll = 0;
        self.user_has_scrolled_up = false;
    }

    /// v0.9.1.3 F19: jump to the top of the transcript (oldest content
    /// visible). The clamp on `scroll_up_by` caps at the maximum legal
    /// offset (`last_total_lines - last_text_area_height`), so we just
    /// ask for `u16::MAX` and let `scroll_up_by` do the math. Sets the
    /// sticky flag so an arriving turn does not snap-yank the reading
    /// position away (same contract as `scroll_up_by`).
    fn jump_to_top(&mut self) {
        self.scroll_up_by(u16::MAX);
    }

    /// Dispatch a key while a tool call is awaiting approval.
    ///
    /// v0.9.1 W1 A rebinds the approval keys to match the inline-card
    /// design (HTML mockup §6.1):
    ///
    /// * `y` / `Enter`  — Approve { Once }
    /// * `a` (lowercase) — Approve { Always } for this tool
    /// * `n` / `Esc`    — Deny
    /// * `A` (capital)  — Approve all pending — fires Approve for the
    ///   NEXT card; remaining cards are drained one per
    ///   tick via [`Self::pending_batch_decision`] until
    ///   the queue is empty. (One-action-per-tick is
    ///   the frozen SurfaceAction contract; the batch
    ///   queue lives on the surface so the contract
    ///   isn't reshaped just for the batch UX.)
    /// * `N` (capital)  — Deny all pending (same drain strategy).
    /// * `Up`/`Down`/`j`/`k`/`Enter` keep the existing highlight-and-
    ///   commit dance for users without the hotkeys.
    ///
    /// v0.9.2 W3 (SPEC §2 Bash / §1D) — the `a`-then-edit-prefix sub-mode.
    /// On a Bash/PowerShell card, `a` does NOT immediately send
    /// `ApprovalScope::Always` (which would auto-approve EVERY shell
    /// command — the audit BLOCKER). It opens an inline prefix-edit input
    /// prefilled with [`infer_shell_prefix`] of the command. While editing,
    /// the keyboard mutates that buffer; `Enter` commits the prefix as
    /// `ApprovalScope::AlwaysPrefix { prefix }` (the W0 prefix-scoped
    /// variant), `Esc` backs out to the card. For every non-shell tool `a`
    /// keeps sending `ApprovalScope::Always` exactly as before.
    fn handle_approval_key(
        &mut self,
        key: KeyEvent,
        call_id: String,
        tool_name: &str,
        command: &str,
        input_pretty: &str,
    ) -> SurfaceAction {
        use wcore_protocol::commands::ApprovalScope;

        // v0.9.2 W11-integ (SPEC §2 #10): AskUserQuestion is interactive
        // Q&A, NOT a yes/no approval. Scope arrow-nav + answer to ONLY this
        // tool so a real approval card is unaffected. `↑`/`↓` move the
        // selected choice index (stored on `approval_sel`, clamped to the
        // parsed choice count); `Enter` picks a choice; `Esc` cancels.
        //
        // v0.9.2: AskUser answer routing needs an engine answer channel
        // (v0.9.3); for now Enter approves rather than error-denies. The
        // previous path sent the chosen label through `Deny { reason }`,
        // which the engine turns into `ToolResult { is_error: true,
        // "Tool denied: <answer>" }` and SKIPS execution — picking an answer
        // ERROR-DENIED the tool. There is no `AskUserQuestion` engine tool
        // and no answer channel on `ToolApprovalResult` (only `Approved` /
        // `Denied { reason }`), so true answer-routing is out of scope here.
        // Harm-reduction: Enter sends `Approve { Once }` so the tool proceeds
        // rather than error-denies; `Esc` still denies (an honest cancel).
        if tool_name == "AskUserQuestion" {
            use wcore_protocol::commands::ApprovalScope;
            let choices = Self::parse_ask_user_choices(input_pretty);
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.approval_sel = self.approval_sel.saturating_sub(1);
                    return SurfaceAction::None;
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    let max = choices.len().saturating_sub(1);
                    self.approval_sel = (self.approval_sel + 1).min(max);
                    return SurfaceAction::None;
                }
                KeyCode::Enter => {
                    let answer = choices.get(self.approval_sel).cloned();
                    // Reset selection so the next AskUser card starts at the
                    // top, matching the component's static first-row default.
                    self.approval_sel = 0;
                    return match answer {
                        // v0.9.3 W8 B1: route the chosen label through the
                        // approval channel as `Approved { answer: Some(label) }`.
                        // Orchestration's synth arm (mod.rs:911) — guarded on
                        // `name == "AskUserQuestion"` per W8 H1-reliability —
                        // turns this into the tool's result content directly,
                        // bypassing `AskUserQuestionTool::execute()`'s
                        // loud-defensive `is_error: true` fallback.
                        Some(label) => SurfaceAction::Approve {
                            call_id,
                            scope: ApprovalScope::Once,
                            answer: Some(label),
                        },
                        // No structured choices parsed — the card showed the
                        // free-text affordance; Enter with nothing selected is
                        // a no-op (the user has no composer in the modal yet).
                        None => SurfaceAction::None,
                    };
                }
                KeyCode::Esc => {
                    self.approval_sel = 0;
                    return SurfaceAction::Deny {
                        call_id,
                        reason: "User dismissed the question without answering".to_string(),
                    };
                }
                _ => return SurfaceAction::None,
            }
        }

        // v0.9.2 W3: while the prefix-edit sub-mode is active it owns the
        // keyboard — Enter commits the edited prefix, Esc backs out, every
        // other key edits the buffer. This branch runs before the normal
        // yes/no/always decision keys so typing `a`, `n`, etc. into the
        // prefix is not misread as a fresh approval decision.
        if let Some(edit) = self.prefix_edit.as_mut() {
            match key.code {
                KeyCode::Enter => {
                    // Commit: send the W0 prefix-scoped allow. Trailing
                    // whitespace is significant to the matcher (head-scope),
                    // so commit the buffer verbatim; only guard the empty
                    // case (an emptied prefix would scope to nothing).
                    let prefix = edit.buffer.value().to_string();
                    let target = edit.call_id.clone();
                    self.prefix_edit = None;
                    if prefix.trim().is_empty() {
                        // Nothing to scope — fall back to a single approve
                        // rather than registering an all-matching prefix.
                        return SurfaceAction::Approve {
                            call_id: target,
                            scope: ApprovalScope::Once,
                            answer: None,
                        };
                    }
                    return SurfaceAction::Approve {
                        call_id: target,
                        scope: ApprovalScope::AlwaysPrefix { prefix },
                        answer: None,
                    };
                }
                KeyCode::Esc => {
                    // Back out to the card — no engine action.
                    self.prefix_edit = None;
                    return SurfaceAction::None;
                }
                _ => {
                    if let Some(req) = to_input_request(&CrosstermEvent::Key(key)) {
                        edit.buffer.handle(req);
                    }
                    return SurfaceAction::None;
                }
            }
        }

        match key.code {
            // y / Enter → Approve { Once } the NEXT card.
            KeyCode::Char('y') | KeyCode::Char('Y') => SurfaceAction::Approve {
                call_id,
                scope: ApprovalScope::Once,
                answer: None,
            },
            // a (lowercase) → on a shell tool, ENTER prefix-edit mode
            // (SPEC §2 Bash); otherwise Approve { Always } for this tool.
            KeyCode::Char('a') => {
                if is_shell_tool(tool_name) {
                    let prefix = infer_shell_prefix(command);
                    self.prefix_edit = Some(PrefixEdit {
                        call_id,
                        buffer: Input::new(prefix),
                    });
                    SurfaceAction::None
                } else {
                    SurfaceAction::Approve {
                        call_id,
                        scope: ApprovalScope::Always,
                        answer: None,
                    }
                }
            }
            // A (capital) → Approve ALL pending (drain queue).
            KeyCode::Char('A') => {
                self.approval_batch = Some(ApprovalBatch::ApproveAll);
                SurfaceAction::Approve {
                    call_id,
                    scope: ApprovalScope::Once,
                    answer: None,
                }
            }
            // n / Esc → Deny.
            KeyCode::Esc | KeyCode::Char('n') => SurfaceAction::Deny {
                call_id,
                reason: "User declined — explain what to change instead".to_string(),
            },
            // N (capital) → Deny ALL pending (drain queue).
            KeyCode::Char('N') => {
                self.approval_batch = Some(ApprovalBatch::DenyAll);
                SurfaceAction::Deny {
                    call_id,
                    reason: "User declined — explain what to change instead".to_string(),
                }
            }
            // Note: generic approval cards (Bash/Edit/Write/…) render a
            // static y/a/n key row with NO selectable list — there is no
            // arrow affordance to honour, so Up/Down/j/k are intentionally
            // unbound here. (Arrow-nav exists ONLY for the AskUserQuestion
            // card, handled in the early-return block above, where the
            // moving marker is actually rendered.) Removing the old dead
            // arrow arms also stops `approval_sel` drifting on a generic
            // card and leaking a stale index into the next AskUser card.

            // Ctrl+F toggles the expanded view of a clamped card body so the
            // user can read a large Edit/Write diff or args blob in full
            // before approving/denying it. The `[ctrl+f] expand` affordance
            // the file/notebook/fallback cards advertise was previously
            // unhandled (the v0.9.6 "expand does nothing" fix).
            KeyCode::Char('f') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.approval_expanded = !self.approval_expanded;
                SurfaceAction::None
            }
            KeyCode::Enter => SurfaceAction::Approve {
                call_id,
                scope: ApprovalScope::Once,
                answer: None,
            },
            _ => SurfaceAction::None,
        }
    }

    /// Drain one card off the batch-approval queue, if a previous `A`/`N`
    /// keypress armed one. Returns the next `Approve`/`Deny` action and
    /// clears the batch once no more pending cards remain.
    ///
    /// Called by the router each tick (parallel to `sync_plan_mode`) so
    /// the batch drains without the user re-pressing the key. Returns
    /// `SurfaceAction::None` when no batch is armed or no cards remain.
    pub fn pending_batch_decision(&mut self, app: &App) -> SurfaceAction {
        use wcore_protocol::commands::ApprovalScope;
        let Some(batch) = self.approval_batch.as_ref().copied() else {
            return SurfaceAction::None;
        };
        let next = app
            .session
            .tool_cards
            .iter()
            .find(|c| c.status == ToolCardStatus::AwaitingApproval)
            .map(|c| c.call_id.clone());
        match next {
            Some(call_id) => match batch {
                ApprovalBatch::ApproveAll => SurfaceAction::Approve {
                    call_id,
                    scope: ApprovalScope::Once,
                    answer: None,
                },
                ApprovalBatch::DenyAll => SurfaceAction::Deny {
                    call_id,
                    reason: "User declined batch — explain what to change instead".to_string(),
                },
            },
            None => {
                // Queue drained — clear the batch.
                self.approval_batch = None;
                SurfaceAction::None
            }
        }
    }

    /// Draw the body: the transcript pane on the left, the right rail
    /// on the right (mockup `.body` → `.transcript` + `.rail`).
    ///
    /// The body keeps a one-column outer margin from the screen edges so
    /// content never butts the frame, and — when the rail is shown — a
    /// one-column gutter between the transcript and the rail. `Ctrl+B`
    /// (`App::rail_visible`) hides the rail; on a narrow terminal it also
    /// auto-hides (see [`rail_effectively_visible`]). Either way the
    /// transcript then takes the full margined width.
    fn render_body(&mut self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // A one-column margin on each side gives the body air. On a
        // terminal too narrow to spare it, `Layout` simply yields a
        // zero-width margin rather than panicking.
        let inner = Layout::horizontal([Constraint::Min(1)])
            .horizontal_margin(1)
            .split(area)[0];
        if inner.width == 0 || inner.height == 0 {
            return;
        }

        if !rail_effectively_visible(app, area.width) || rail_is_empty(app) {
            // Rail hidden — by the user's `Ctrl+B`, by a narrow
            // terminal, or because every panel is empty. The transcript
            // takes the full margined width.
            self.render_transcript(frame, inner, app, theme);
            return;
        }

        // The rail keeps a fixed width but never starves the transcript:
        // on a narrow terminal it shrinks (and may vanish entirely). A
        // one-column gutter (`Layout::spacing`) separates the two panes.
        let rail_width = RAIL_WIDTH.min(inner.width.saturating_sub(20));
        let [transcript_area, rail_area] =
            Layout::horizontal([Constraint::Min(20), Constraint::Length(rail_width)])
                .spacing(1)
                .areas(inner);

        self.render_transcript(frame, transcript_area, app, theme);
        if rail_area.width > 0 {
            render_rail(frame, rail_area, app, theme);
        }
    }

    /// Draw the transcript: completed turns, the in-flight stream and
    /// thinking, and tool-call cards. Tool cards (including any awaiting-
    /// approval inline prompt) render inside the transcript flow via
    /// [`build_settled_transcript_lines`] / [`push_tool_card_lines`] — there is no
    /// separate cards strip or approval strip anymore. An empty transcript
    /// renders the idle hero.
    ///
    /// v0.9.1 W2 cycle-2: the legacy per-frame `cards_area` + `diff_area` +
    /// `approval_area` layout split was deleted. It caused every tool card
    /// to render twice (once inline from W1 A's rewrite, once in the strip)
    /// and every awaiting-approval card to render its approval UI twice
    /// (once inline via `render_approval_inline`, once via the
    /// now-deleted `render_approval` strip).
    fn render_transcript(&mut self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        let block = Block::default().style(Style::default().bg(theme.bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let session = &app.session;
        let is_idle = session.turns.is_empty()
            && session.streaming.is_empty()
            && session.thinking.is_empty()
            && session.tool_cards.is_empty();

        if is_idle {
            render_idle_hero(frame, inner, theme);
            return;
        }

        // The transcript paragraph owns the full inner area. Tool cards,
        // approval prompts, and any associated body text are emitted as
        // lines inside `render_turns` (see `push_tool_card_lines`).
        let text_area = inner;

        // D2/v0.9.0 sticky-at-bottom: if a new turn arrived while the user
        // had scrolled up, bump `transcript_scroll` by the lines that grew
        // at the bottom so their reading position stays pinned. `render_turns`
        // returns the total line count; we react after the first draw and
        // re-render once with the corrected scroll. No frame is presented
        // mid-paint so the brief overdraw is invisible.
        //
        // v0.9.1 W1 A (B13 fix): the bumper also requires
        // `transcript_scroll > 0` at the START of the render — gating on
        // just the sticky flag races a wheel-tick that arrives in the
        // same frame as a new turn (the flag flips true mid-tick from the
        // wheel handler, then the bumper over-adds the new-turn delta on
        // top of the wheel-tick's 3-line bump). Requiring a non-zero
        // pre-render scroll means the bumper only fires when there is an
        // already-anchored reading position to preserve.
        //
        // v0.9.1.1 F3: belt-and-suspenders snap-to-bottom — whenever the
        // user is NOT sticky-scrolled-up, force `transcript_scroll = 0`
        // before the render. `render_turns` already bottom-anchors the
        // paragraph at `scroll_offset == 0`, but several state-machine
        // paths (a partial-wheel-tick scroll that didn't reach the bottom,
        // a synthesized scroll from an overlay opening, a turn-replay on
        // surface switch) could leave a small positive `transcript_scroll`
        // with `user_has_scrolled_up == false`. The defensive clamp here
        // means "no reading-position to preserve → always show the newest
        // content at the bottom of the pane" — matching the mockup §6
        // sticky-at-bottom behaviour. Live e2e found assistant responses
        // landing below the visible area after a fresh submit; this is
        // the closure.
        // v0.9.1.2 F14: while ANY tool card is awaiting approval, lock
        // the sticky-up flag so the auto-snap-to-bottom can't push the
        // pending card off-screen while subsequent streaming text grows
        // the transcript. The lock releases as soon as the user decides
        // (which transitions every card off `AwaitingApproval`).
        let any_awaiting_approval = app
            .session
            .tool_cards
            .iter()
            .any(|c| matches!(c.status, ToolCardStatus::AwaitingApproval));
        // v0.9.1.2 F14c: F14b used to snap `transcript_scroll = 0` here
        // whenever `force_scroll_to_pending_approval` was set, on the
        // theory that bottom-anchoring would bring the pending card
        // into view inside the same frame. It did not — `scroll = 0`
        // is the bottom anchor of `render_turns`, not the card's row,
        // so on a long streaming markdown response the inline card
        // still landed below the visible viewport. Worse, the in-frame
        // snap yanked the user away from their scroll position every
        // tick a pending approval existed (the user lost the ability
        // to browse the transcript for context while approving).
        //
        // The sticky APPROVAL STRIP (rendered above the composer by
        // `render`) makes the snap unnecessary — the prompt is always
        // visible in its own pinned row regardless of scroll position,
        // so the user is free to scroll the transcript without losing
        // the approval affordance. `tick()` still consumes the
        // `force_scroll_to_pending_approval` flag idempotently for any
        // other path that may rely on it, but render no longer mutates
        // `transcript_scroll` on the flag.
        if !self.user_has_scrolled_up && !any_awaiting_approval {
            self.transcript_scroll = 0;
        }
        let scroll_before_render = self.transcript_scroll;
        // D009 (render-livelock): consult the layout cache. The cache stores the
        // wrapped SETTLED transcript (completed turns + inline cards) keyed on a
        // signature that EXCLUDES the live stream and its animation tick — so a
        // turn streaming a 108KB body, which used to flip the signature every
        // frame and re-wrap the whole turn, now leaves the settled cache a
        // stable HIT. The live tail (thinking + streamed text + status spinner)
        // is rebuilt every frame, but BOUNDED: on the bottom-anchored path we
        // wrap only the visible tail of the streaming buffer, so a live stream
        // costs O(viewport) per frame, not O(turn). The cache MISS path still
        // builds the settled logical lines once and wraps them once.
        //
        // The user is bottom-anchored unless they have scrolled up. While
        // streaming AND scrolled up (a rare interactive case, NOT the livelock),
        // we fall back to the full unbounded build so the scrolled-up reader
        // sees the entire in-flight turn at any offset.
        let streaming = session.streaming_active || !session.streaming.is_empty();
        let bottom_anchored = !self.user_has_scrolled_up;
        let window_live_stream = streaming && bottom_anchored;

        let settled_sig = TranscriptSig {
            turns: session.turns.len(),
            last_turn_elements: session.turns.last().map(|t| t.elements.len()).unwrap_or(0),
            // The settled cache never holds the live stream/thinking; those go
            // into the per-frame live tail. When NOT windowing the live stream
            // (idle, or streaming-while-scrolled-up) the cache holds the FULL
            // transcript, so the stream lengths belong in the signature.
            streaming_len: if window_live_stream {
                0
            } else {
                session.streaming.len()
            },
            thinking_len: if window_live_stream {
                0
            } else {
                session.thinking.len()
            },
            tool_cards: session.tool_cards.len(),
            width: text_area.width,
            // The animation tick only forces a rebuild on the unbounded path
            // (streaming-while-scrolled-up) so the spinner keeps moving there.
            // On the windowed bottom-anchored path the tick lives in the
            // per-frame live tail instead, so the settled cache stays a HIT.
            streaming_tick: if streaming && !window_live_stream {
                app.frame_tick
            } else {
                0
            },
            // Fold each tool card's status + output length so a `ToolResult`
            // that flips a card Running→Ok/Err (and sets its output) in place —
            // without changing `tool_cards` (the count) or the turn's element
            // count — still invalidates the settled cache and repaints the card.
            // Without this the completed card stays frozen on "running…".
            tool_cards_fp: tool_cards_fingerprint(&session.tool_cards),
            theme_bg: theme.bg,
        };
        let cache_hit = self
            .transcript_layout
            .as_ref()
            .is_some_and(|c| c.sig == settled_sig);

        if !cache_hit {
            // MISS: rebuild + re-wrap the settled half (and, on the unbounded
            // path, the live tail too). `editable_prefix` borrows `self`
            // immutably; scope it to the build so it never overlaps the cache
            // write below.
            let wrapped = {
                let editable_prefix = self.prefix_edit.as_ref().map(|e| e.buffer.value());
                let mut logical = build_settled_transcript_lines(
                    app,
                    theme,
                    text_area.width,
                    editable_prefix,
                    self.approval_sel,
                    self.approval_expanded,
                );
                // On the unbounded path the cache holds the full transcript, so
                // append the live tail over the FULL streaming buffer here. On
                // the windowed path the live tail is built per frame below.
                if !window_live_stream {
                    build_live_tail_lines(&mut logical, app, theme, &session.streaming);
                }
                wrap_lines_to_width(logical, text_area.width)
            };
            let wrapped_total = wrapped.len().min(u16::MAX as usize) as u16;
            self.transcript_layout = Some(CachedTranscriptLayout {
                sig: settled_sig,
                wrapped_total,
                lines: wrapped,
            });
        }

        // The cached settled wrapped count. On the unbounded path this IS the
        // full transcript total; on the windowed path the live tail adds to it
        // (computed below).
        let settled_total = self
            .transcript_layout
            .as_ref()
            .map_or(0, |c| c.wrapped_total);

        // D009 stream windowing (Option A): on the bottom-anchored live path,
        // build + wrap ONLY the visible tail of the live turn this frame, then
        // render the bottom window of [settled tail ++ live tail]. The live tail
        // is bounded to the viewport budget by `streaming_visible_tail_offset`,
        // so this is O(viewport) regardless of the streaming turn's size — the
        // remaining half of the livelock fix.
        if window_live_stream {
            let tail_off = streaming_visible_tail_offset(
                &session.streaming,
                text_area.width,
                text_area.height,
            );
            let mut live_logical: Vec<Line<'static>> = Vec::new();
            build_live_tail_lines(
                &mut live_logical,
                app,
                theme,
                &session.streaming[tail_off..],
            );
            let live_wrapped = wrap_lines_to_width(live_logical, text_area.width);

            // Total visual rows = settled + live tail. The live tail is bounded,
            // so this sum is cheap; bottom-anchored render needs only the last
            // `area.height` rows of the concatenation.
            let total = (settled_total as usize + live_wrapped.len()).min(u16::MAX as usize) as u16;

            // Build only the bottom-window slice: take the tail of the live
            // rows, then back-fill from the settled cache if the live tail is
            // shorter than the viewport. This materializes <= viewport+overscan
            // rows, never the whole transcript.
            self.render_bottom_window_with_live_tail(frame, text_area, theme, &live_wrapped);

            self.last_text_area_height = text_area.height;
            self.last_total_lines = total;

            // Scrollbar / jump hint reflect the combined total below; share the
            // `total` value via the outer scope.
            self.render_transcript_scroll_chrome(frame, text_area, theme, total);
            return;
        }

        // From here the cache is guaranteed populated for `settled_sig` and
        // holds the FULL transcript. Read the wrapped total for the scroll math;
        // the window render borrows the cached lines immutably.
        let total = settled_total;

        // D2/v0.9.0 sticky-at-bottom bump: if a new turn grew the transcript
        // while the user had scrolled up, bump `transcript_scroll` by the
        // delta so their reading position stays pinned. A new turn is also a
        // cache MISS, so this only runs on the rebuild frame.
        if self.user_has_scrolled_up && scroll_before_render > 0 && total > self.last_total_lines {
            let delta = total - self.last_total_lines;
            self.transcript_scroll = self.transcript_scroll.saturating_add(delta);
        }

        // Render only the visible window. O(viewport), independent of
        // transcript size.
        if let Some(cached) = self.transcript_layout.as_ref() {
            render_transcript_window(
                frame,
                text_area,
                theme,
                &cached.lines,
                self.transcript_scroll,
            );
        }
        self.last_text_area_height = text_area.height;
        self.last_total_lines = total;

        self.render_transcript_scroll_chrome(frame, text_area, theme, total);
    }

    /// D009: render the bottom-anchored window of `[settled tail ++ live tail]`
    /// for a live stream WITHOUT materializing the whole transcript. `live` is
    /// the already-wrapped, viewport-bounded live tail (thinking + streamed text
    /// and status). We take the bottom `area.height + overscan` rows of the
    /// concatenation: the live tail fills from the bottom, and if it is shorter
    /// than the viewport we back-fill the remaining rows from the END of the
    /// cached settled layout. The materialized slice is therefore bounded to the
    /// viewport size, never the transcript size — the O(viewport) guarantee that
    /// keeps input responsive while a huge turn streams.
    fn render_bottom_window_with_live_tail(
        &self,
        frame: &mut Frame,
        area: Rect,
        theme: &Theme,
        live: &[Line<'static>],
    ) {
        if area.height == 0 || area.width == 0 {
            return;
        }
        const OVERSCAN: usize = 2;
        let budget = area.height as usize + OVERSCAN;

        // Take the bottom `budget` rows of the live tail.
        let live_take = live.len().min(budget);
        let mut window: Vec<Line<'static>> = Vec::with_capacity(budget);
        // Back-fill from the settled cache tail if the live tail underfills the
        // viewport, so a short stream still shows the prior turns above it.
        let need = budget - live_take;
        if let Some(cached) = self.transcript_layout.as_ref().filter(|_| need > 0) {
            let settled = &cached.lines;
            let start = settled.len().saturating_sub(need);
            window.extend(settled[start..].iter().cloned());
        }
        window.extend(live[live.len() - live_take..].iter().cloned());

        // Bottom-anchor: if the window is taller than the viewport (overscan),
        // scroll the Paragraph down so the newest row sits on the last visual
        // row. The rows are already wrapped, so wrapping stays disabled.
        let inner_scroll = (window.len().saturating_sub(area.height as usize)) as u16;
        let para = Paragraph::new(window).style(Style::default().bg(theme.bg));
        frame.render_widget(para.scroll((inner_scroll, 0)), area);
    }

    /// Render the transcript's scrollbar + "jump to latest" hint chrome. Shared
    /// by the steady-state windowed path and the live-stream windowed path so
    /// both paint identical overflow affordances against the combined `total`.
    fn render_transcript_scroll_chrome(
        &mut self,
        frame: &mut Frame,
        text_area: Rect,
        theme: &Theme,
        total: u16,
    ) {
        // Vertical scrollbar — only when content overflows. `VerticalRight`
        // paints the track in the rightmost column of the area handed in,
        // so passing `text_area` lines it up with the transcript pane.
        if total > text_area.height {
            let max_scroll = total.saturating_sub(text_area.height);
            let position = (max_scroll.saturating_sub(self.transcript_scroll)) as usize;
            let mut scrollbar_state = ScrollbarState::new(max_scroll as usize).position(position);
            let scrollbar = Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .style(Style::default().fg(theme.text_muted).bg(theme.bg));
            frame.render_stateful_widget(scrollbar, text_area, &mut scrollbar_state);
        }

        // "↓ jump to latest" hint — ALWAYS pinned bottom-right of the
        // transcript pane (Gmail "scroll to bottom" pattern). Dimmed
        // `text_muted` when the user is already at the bottom (no
        // signal); promoted to `orange + BOLD` when the user has
        // scrolled up (new content below — clickable affordance).
        //
        // v0.9.1.3 F19: stash the painted rect on the surface so
        // `handle_mouse` can hit-test a left click against it.
        // v0.9.1.3 K: hint is always rendered (per recon §6.4 polish
        // bar — was only visible while scrolled-up under F19). When
        // `user_has_scrolled_up` the orange+bold treatment IS one of
        // the two retained orange surfaces (alongside the user-turn
        // `▌`) — signal that there's content the user hasn't reached.
        if text_area.height > 0 && text_area.width >= 20 {
            let label = "↓ jump to latest";
            let label_w = label.chars().count() as u16;
            // Stop short of the scrollbar track when the bar is shown.
            let right_inset: u16 = if total > text_area.height { 2 } else { 1 };
            let x = text_area.x + text_area.width.saturating_sub(label_w + right_inset);
            let y = text_area.y + text_area.height - 1;
            let hint_area = Rect::new(x, y, label_w, 1);
            let style = if self.user_has_scrolled_up {
                Style::default()
                    .fg(theme.orange)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
                    .fg(theme.text_muted)
                    .add_modifier(Modifier::DIM)
            };
            let hint =
                Paragraph::new(Line::from(Span::styled(label, style))).alignment(Alignment::Right);
            frame.render_widget(hint, hint_area);
            self.last_jump_hint_rect = Some(hint_area);
        } else {
            self.last_jump_hint_rect = None;
        }
    }

    /// Draw the composer (mockup `.composer`): a bordered input row plus
    /// a hint row. While the agent is streaming the input row is replaced
    /// by a spinner + cancel affordance.
    ///
    /// The top border is painted in `theme.border` (neutral dim grey) so
    /// it reads as a visible break between scrollback and the input zone
    /// without burning brand accent on chrome.
    ///
    /// v0.9.1.3 J: demoted from `theme.orange` per the accent-inflation
    /// audit (10+ orange surfaces vs. recon §1.4 budget of 2). The
    /// active-tab underline and the user-turn `▌` are the only surfaces
    /// that keep orange — chrome rails are structural.
    fn render_composer(&self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(theme.border))
            .style(Style::default().bg(theme.bg));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        // The input section takes up everything except the hint row. When
        // multi-line content is present the inner height may be > 1 (up to
        // COMPOSER_INPUT_MAX_ROWS), letting the user see the full pasted
        // text (F-042). The hint row always gets exactly 1 row.
        let input_rows = inner.height.saturating_sub(1).max(1);
        let [input_area, hint_area] =
            Layout::vertical([Constraint::Length(input_rows), Constraint::Min(0)]).areas(inner);

        // No-model guard: when the resolved config carries no model, block
        // the input area and show a fix-it panel. The hint row below still
        // renders normally so the user can see how to open the palette.
        if app.config.model.is_empty() {
            // D002: this state is reached most often when a catalog provider
            // (Groq / OpenRouter / DeepSeek / Novita / …) is chosen at
            // onboarding — those hosts carry no single default model, so the
            // resolved config has an empty model. The banner must offer an
            // IN-APP recovery (`/model`, reachable right here — slash commands
            // bypass the no-model submit block) instead of a quit-and-hand-edit
            // dead-end. `wayland-core setup` stays as a secondary option.
            // The first line always carries the literal "No model configured"
            // string (the no-model state's stable fingerprint). When the
            // provider is known we name it on the next line so the user knows
            // which provider needs a model picked.
            let mut lines = vec![Line::from(vec![Span::styled(
                "No model configured.",
                Style::default()
                    // v0.9.1.3 J: demoted from `theme.orange` to
                    // `theme.warning` per the accent-inflation
                    // audit. A missing-model state is a warn, not
                    // a brand accent — yellow carries the right
                    // semantic and frees orange for active-tab
                    // underline + user-turn `▌`.
                    .fg(theme.warning)
                    .add_modifier(ratatui::style::Modifier::BOLD),
            )])];
            if !app.config.provider.is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    format!("{} has no default model.", app.config.provider),
                    Style::default().fg(theme.text_dim),
                )]));
            }
            lines.push(Line::from(vec![Span::styled("", Style::default())]));
            lines.push(Line::from(vec![Span::styled(
                "Type  /model  to pick one now, no restart needed.",
                Style::default().fg(theme.text_dim),
            )]));
            lines.push(Line::from(vec![Span::styled(
                "Or run  wayland-core setup  to change provider.",
                Style::default().fg(theme.text_muted),
            )]));
            let banner = Paragraph::new(lines)
                .style(Style::default().bg(theme.bg))
                .alignment(ratatui::layout::Alignment::Left);
            frame.render_widget(banner, input_area);
            // Still render the hint row so the user can see /setup etc.
            if hint_area.height > 0 {
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        "/model pick a model    / open palette    ⌃C quit",
                        Style::default().fg(theme.text_muted),
                    )))
                    .style(Style::default().bg(theme.bg)),
                    hint_area,
                );
            }
            return;
        }

        {
            // 2026-05-31: the animated "working" status line moved INLINE,
            // directly under the streaming text (see `render_turns`), so the
            // motion sits where the user is reading instead of detached at the
            // bottom. The composer input therefore renders even while streaming
            // — the user can see and queue their next message live.
            let value = self.composer.value();
            // D010: use the CACHED line count instead of re-scanning the buffer
            // each frame.
            let line_count = self.composer_lines.max(1) as usize;

            if line_count > 1 && input_rows > 1 {
                // Multi-line paste (F-042): render the text with wrapping
                // so all lines are visible in the expanded input area. The
                // cursor is placed at the end (paste always ends there).
                //
                // D010: only build the LAST `input_rows` lines — that is all
                // that fits in the (capped-height) input area. Building every
                // line of a large buffer per frame was part of the per-frame
                // stall. The cursor sits at the end of a paste, so showing the
                // tail keeps it visible.
                let visible = input_rows as usize;
                let total = line_count;
                let skip = total.saturating_sub(visible);
                let mut lines: Vec<Line> = Vec::new();
                for (i, line) in value.lines().skip(skip).enumerate() {
                    // The first GLOBAL line gets the `›` prompt; continuation
                    // lines get the 2-space gutter. When the head is scrolled
                    // out of view (`skip > 0`) every visible line is a
                    // continuation, so it always gets the gutter.
                    let glyph = if skip == 0 && i == 0 {
                        Span::styled("› ", Style::default().fg(theme.text_dim))
                    } else {
                        Span::styled("  ", Style::default().fg(theme.text_dim))
                    };
                    lines.push(Line::from(vec![
                        glyph,
                        Span::styled(line.to_string(), Style::default().fg(theme.text)),
                    ]));
                }
                // Paint a chip if the text is longer than what fits. The
                // clipped-paste signal is a status-bar TOAST (set in
                // `handle_paste`), not a composer row — the dynamic composer
                // height fills the input area exactly with content, leaving no
                // room for an extra note row here.
                if line_count as u16 > COMPOSER_INPUT_MAX_ROWS {
                    lines.push(Line::from(Span::styled(
                        format!("  ({} chars — press ⏎ to send)", value.len()),
                        Style::default().fg(theme.text_muted),
                    )));
                }
                frame.render_widget(
                    Paragraph::new(lines).style(Style::default().bg(theme.bg)),
                    input_area,
                );
            } else {
                // Single-line: horizontal scroll so cursor stays visible.
                let width = input_area.width.saturating_sub(2) as usize;
                let scroll = self.composer.visual_scroll(width.max(1));
                let line = if value.is_empty() {
                    // Ghost placeholder: when the composer is empty show a
                    // dim "type / for commands" hint right where typing
                    // would appear, so the discovery hint sits inside the
                    // input zone (not floating below the banner). The
                    // hint vanishes as soon as the user types.
                    // v0.9.1.3 J: composer `›` prompt demoted to
                    // `theme.text_dim` per the accent-inflation audit.
                    Line::from(vec![
                        Span::styled("› ", Style::default().fg(theme.text_dim)),
                        Span::styled(
                            "type / for commands",
                            Style::default()
                                .fg(theme.text_muted)
                                .add_modifier(Modifier::DIM),
                        ),
                    ])
                } else {
                    Line::from(vec![
                        Span::styled("› ", Style::default().fg(theme.text_dim)),
                        Span::styled(value.to_string(), Style::default().fg(theme.text)),
                    ])
                };
                frame.render_widget(
                    Paragraph::new(line)
                        .style(Style::default().bg(theme.bg))
                        .scroll((0, scroll as u16)),
                    input_area,
                );
                // Place the terminal cursor at the composer caret. With
                // the placeholder showing the visual_cursor is 0 so the
                // caret sits at column 2 (right after the `› ` prompt).
                let cursor_x = input_area.x
                    + 2
                    + (self.composer.visual_cursor().saturating_sub(scroll)) as u16;
                if cursor_x < input_area.x + input_area.width {
                    frame.set_cursor_position((cursor_x, input_area.y));
                }
            }
        }

        if hint_area.height > 0 {
            // The rail-toggle hint reflects the current state so the
            // user knows what `Ctrl+B` will do.
            let rail_hint = if app.rail_visible {
                "⌃B hide rail"
            } else {
                "⌃B show rail"
            };

            // A turn blocked on an approval card keeps `streaming_active`
            // true; when a card is awaiting approval the footer must show the
            // y/a/n/esc affordances (the `else` arm below), NOT the generic
            // "Esc interrupt" streaming hint (the v0.9.6 footer fix — the
            // "Esc interrupt" line is exactly what misled the user into
            // pressing Esc and cancelling the whole turn).
            let awaiting_approval_now = app
                .session
                .tool_cards
                .iter()
                .any(|c| matches!(c.status, ToolCardStatus::AwaitingApproval));
            if app.session.streaming_active && !awaiting_approval_now {
                // While streaming, `Enter` queues the typed message —
                // it is held and sent when the turn ends (AUDIT-D D3).
                let hint = if app.queued_message.is_some() {
                    "Esc interrupt    ⏎ replace queued message".to_string()
                } else {
                    "Esc interrupt    ⏎ queue a message for the next turn".to_string()
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(
                        hint,
                        Style::default().fg(theme.text_muted),
                    )))
                    .style(Style::default().bg(theme.bg)),
                    hint_area,
                );
            } else if self.mode_flash_ticks > 0 {
                // F-082: briefly highlight the new mode after ⇧Tab so the
                // user gets visible confirmation that the cycle fired.
                use wcore_protocol::commands::SessionMode;
                let mode_name = match &app.mode {
                    SessionMode::Default => "Default",
                    SessionMode::AutoEdit => "Auto-edit",
                    SessionMode::Force => "Force",
                };
                frame.render_widget(
                    Paragraph::new(Line::from(vec![
                        Span::styled(
                            " ⇧Tab → ",
                            Style::default().fg(theme.text_muted).bg(theme.bg),
                        ),
                        // v0.9.1.3 J: mode-flash chip bg demoted from
                        // `theme.orange` to `theme.surface_hover` per
                        // the accent-inflation audit. The chip is a
                        // brief confirmation (mode_flash_ticks), not a
                        // persistent brand surface — bold text on a
                        // hover-tint bg reads as "new state" without
                        // burning brand accent.
                        Span::styled(
                            format!(" {mode_name} "),
                            Style::default()
                                .fg(theme.text)
                                .bg(theme.surface_hover)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(
                            "  mode active",
                            Style::default().fg(theme.text_muted).bg(theme.bg),
                        ),
                    ]))
                    .style(Style::default().bg(theme.bg)),
                    hint_area,
                );
            } else {
                // v0.9.1.2 F14: when ANY tool card is awaiting approval,
                // the hint must tell the user what keys to press. The
                // mouse-capture and selection-mode hints are the FALLBACK
                // when no approval is pending — approval discoverability
                // wins because a stuck "Brewing… 4m 31s" with no
                // y/n affordance is the bug we are closing here.
                //
                // 2026-05-31: status hint reflects the live mouse-capture
                // mode. The default is ON — the scroll wheel drives the
                // transcript out of the box. F4 toggles capture OFF for
                // native drag-select/copy (Shift+drag also works while
                // captured); the hint then advertises how to get scroll back.
                // D040: the AskUserQuestion card is a pick-a-choice list, not a
                // yes/no tool approval — y/a/n do NOTHING on it (only the
                // arrow-nav / Enter / Esc arms in `handle_approval_key` fire).
                // Advertising [y][a][n] there was a lie. Detect the HEAD awaiting
                // card's kind and show the keys that actually work for it; a
                // normal tool-approval card keeps its real y/a/n triplet.
                let head_awaiting = app
                    .session
                    .tool_cards
                    .iter()
                    .find(|c| matches!(c.status, ToolCardStatus::AwaitingApproval));
                let head_is_ask_user =
                    head_awaiting.is_some_and(|c| c.tool_name == "AskUserQuestion");
                let (hint, hint_style) = if head_is_ask_user {
                    (
                        "⊘ ↑/↓ move · ⏎ select · esc cancel".to_string(),
                        Style::default()
                            .fg(theme.warning)
                            .add_modifier(Modifier::BOLD),
                    )
                } else if head_awaiting.is_some() {
                    (
                        "⊘ press [y] approve · [a] always · [n] deny · [esc] cancel".to_string(),
                        Style::default()
                            .fg(theme.warning)
                            .add_modifier(Modifier::BOLD),
                    )
                } else if !app.mouse_capture_enabled {
                    // Capture toggled OFF via F4: native drag-select/copy
                    // works, but the wheel now scrolls the host terminal, not
                    // the transcript. Tell the user how to get wheel scroll
                    // back (and that keyboard scroll still works meanwhile).
                    (
                        "🖱 Select mode — drag to copy · F4 for wheel scroll · PgUp/PgDn scroll"
                            .to_string(),
                        Style::default().fg(theme.text_muted),
                    )
                } else if self.user_has_scrolled_up {
                    // v0.9.1.3 F19: when the user has scrolled up the
                    // discoverable next move is "jump to latest" — make
                    // both End AND the click affordance findable. The
                    // ⇧Tab / rail / history hints are buried during
                    // scroll-back because there is no room for both;
                    // the user gets them back as soon as they hit End.
                    (
                        "PgUp/PgDn scroll · Home top · End jump to latest · click ↓ to jump"
                            .to_string(),
                        Style::default().fg(theme.text_muted),
                    )
                } else {
                    // 2026-05-31: default state — capture is ON so the wheel
                    // scrolls the transcript. PgUp/PgDn/Home/End also scroll.
                    // F4 drops to native select/copy (Shift+drag works too).
                    // D038: the `? keys` hint is now truthful — `?` on an
                    // empty composer opens the live help (here it routes to
                    // /help; on every other surface the Router opens the `?`
                    // overlay). FIX-7: Tab always switches tabs now (the
                    // reasoning-rail Tab-stepping was removed), so the hint is
                    // unconditionally honest.
                    (
                        format!(
                            "Tab next tab  ⇧Tab mode  {rail_hint}  PgUp/PgDn scroll  End ↓latest  F4 copy/select  ? keys"
                        ),
                        Style::default().fg(theme.text_muted),
                    )
                };
                frame.render_widget(
                    Paragraph::new(Line::from(Span::styled(hint, hint_style)))
                        .style(Style::default().bg(theme.bg)),
                    hint_area,
                );
            }
        }
    }
}

impl WorkspaceSurface {
    /// Draw the `@`-completion popup — a small bordered list anchored to
    /// the bottom of `body_area`, just above the composer. Each row shows
    /// the candidate's insert text and its "surfaced as" label; the
    /// highlighted row is accent-styled.
    fn render_at_completion(&self, frame: &mut Frame, body_area: Rect, theme: &Theme) {
        let Some(comp) = self.at_completion.as_ref() else {
            return;
        };
        if body_area.height < 3 || body_area.width < 10 {
            return;
        }
        // Up to 6 candidate rows + the border.
        let rows = comp.candidates.len().min(6) as u16;
        let popup_h = rows + 2;
        let popup_w = body_area.width.min(56);
        let popup = Rect::new(
            body_area.x,
            body_area.y + body_area.height.saturating_sub(popup_h),
            popup_w,
            popup_h,
        );
        frame.render_widget(ratatui::widgets::Clear, popup);
        // v0.9.1.3 J: popup border demoted from `theme.orange` to
        // `theme.border` per the accent-inflation audit. Modal chrome
        // is structural; the selected-row bold + marker is the actual
        // affordance.
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border))
            .style(Style::default().bg(theme.surface_elevated))
            .title(Span::styled(
                " @ reference ",
                Style::default().fg(theme.text_muted),
            ))
            // D039: name the Tab the user gets while the popup is open — here
            // Tab accepts the highlighted candidate (it does NOT switch tabs).
            .title_bottom(Span::styled(
                " Tab accept · ↑/↓ move · Esc close ",
                Style::default().fg(theme.text_dim),
            ));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let mut lines: Vec<Line> = Vec::new();
        for (i, cand) in comp.candidates.iter().take(rows as usize).enumerate() {
            let selected = i == comp.selected;
            let marker = if selected { "▸ " } else { "  " };
            let name_style = if selected {
                Style::default()
                    .fg(theme.text)
                    .add_modifier(Modifier::BOLD)
                    .bg(theme.surface_elevated)
            } else {
                Style::default()
                    .fg(theme.text_dim)
                    .bg(theme.surface_elevated)
            };
            lines.push(Line::from(vec![
                // v0.9.1.3 J: `▸` marker demoted to `theme.text_dim`
                // per the accent-inflation audit. Selected-row bold +
                // brighter fg already signals selection; the marker is
                // structural.
                Span::styled(marker, Style::default().fg(theme.text_dim)),
                Span::styled(cand.insert.clone(), name_style),
                Span::styled(
                    format!("  {}", cand.label),
                    Style::default()
                        .fg(theme.text_muted)
                        .bg(theme.surface_elevated),
                ),
            ]));
        }
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().bg(theme.surface_elevated)),
            inner,
        );
    }
}

/// Draw the idle hero — the full WAYLAND banner.
///
/// An empty transcript was previously blank space with a one-line prompt;
/// the hybrid-branding decision makes it the home for the full WAYLAND
/// ASCII banner (the same wordmark the onboarding intro shows). The
/// [`wayland_banner`] widget paints the wordmark, the "the autonomous AI
/// agent" tagline, and the "type / for commands" hint, centered, and
/// degrades to the tagline + hint alone on a terminal too small for the
/// art. The transcript background is painted under it for cohesion.
/// The idle/empty Workspace hero: the WAYLAND banner plus a factual one-line
/// "what is this" subtitle and 2-3 concrete example prompts. Rendered ONLY when
/// the transcript is empty (the sole caller is the idle branch of
/// [`WorkspaceSurface::render_transcript`]).
///
/// D044: the bare banner (wordmark + tagline) left the first-keystroke-after-
/// onboarding user staring at a blank canvas with no model of what to type. The
/// hero block answers "what now?" directly: a one-line description of what
/// Wayland is, then three copy-pasteable starter prompts. The banner keeps the
/// top of the pane; the hero copy sits in a fixed slot at the bottom so it never
/// collides with the centered wordmark on a tall terminal and gracefully drops
/// when the pane is too short for both.
fn render_idle_hero(frame: &mut Frame, area: Rect, theme: &Theme) {
    frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);

    // FIX-5: the boot screen is the first-run concierge moment. Alongside the
    // starter prompts it surfaces (a) the paste-to-connect door (`/connect`,
    // the easiest way to add a provider — previously undiscoverable), and (b)
    // any ambient cloud credentials already on this machine, so the user sees
    // "you can use these right now" instead of dead space.
    let detected = detect_boot_providers();
    let forge = detect_forge_servers();

    // Rows: subtitle + blank + [detected?] + [forge?] + 3 prompts + blank-lead.
    // Reserve them only when the pane can spare the rows after the banner's
    // headroom; on a short pane the banner takes the whole area and the copy is
    // dropped.
    let hero_rows: u16 = 6 + u16::from(!detected.is_empty()) + u16::from(!forge.is_empty());
    let show_hero = area.height >= hero_rows + 8;
    if !show_hero {
        wayland_banner(frame, area, theme);
        return;
    }

    let [banner_area, hero_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(hero_rows)]).areas(area);
    wayland_banner(frame, banner_area, theme);

    // No em-dashes in user-facing copy (project rule) — the bullet is a middle
    // dot.
    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            "A terminal AI agent that reads, writes, and runs code in your project.",
            Style::default().fg(theme.text_dim),
        )),
        Line::from(""),
    ];
    if !detected.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Detected: ", Style::default().fg(theme.success)),
            Span::styled(detected.join(" · "), Style::default().fg(theme.text)),
            Span::styled(
                "  ·  /provider to use",
                Style::default().fg(theme.text_muted),
            ),
        ]));
    }
    // Slice 3b — a discovered Forge MCP server (e.g. Agent Vault) is one
    // command from connected; surface it next to the provider line.
    if !forge.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Forge MCP: ", Style::default().fg(theme.success)),
            Span::styled(forge.join(" · "), Style::default().fg(theme.text)),
            Span::styled(
                "  ·  /mcp connect to link",
                Style::default().fg(theme.text_muted),
            ),
        ]));
    }
    lines.push(Line::from(Span::styled(
        "Try: explain this codebase",
        Style::default().fg(theme.text_muted),
    )));
    lines.push(Line::from(vec![
        Span::styled("/connect", Style::default().fg(theme.orange)),
        Span::styled(
            " to paste an API key and add a provider",
            Style::default().fg(theme.text_muted),
        ),
    ]));
    lines.push(Line::from(Span::styled(
        "Try: /model to pick a model",
        Style::default().fg(theme.text_muted),
    )));

    let para = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .style(Style::default().bg(theme.bg));
    frame.render_widget(para, hero_area);
}

/// FIX-5: ambient LLM-provider credentials already present on this machine —
/// the ones a user can route to immediately without entering a key. Reuses the
/// single source of truth `provider_connected` (sync, no network): AWS for
/// Bedrock, GCP ADC for Vertex, and a stored ChatGPT OAuth login. Returns the
/// human labels of the connected ones, in a stable order.
fn detect_forge_servers() -> Vec<String> {
    wcore_config::forge_discovery::read_discovered_servers()
        .into_iter()
        .map(|s| s.label().to_string())
        .collect()
}

fn detect_boot_providers() -> Vec<&'static str> {
    use wcore_config::config::{ProviderType, provider_connected};
    let mut found = Vec::new();
    if provider_connected(ProviderType::Bedrock) {
        found.push("AWS Bedrock");
    }
    if provider_connected(ProviderType::Vertex) {
        found.push("Google Vertex");
    }
    if provider_connected(ProviderType::OpenAIChatGpt) {
        found.push("ChatGPT login");
    }
    found
}

/// D009: the SETTLED half of the transcript — the completed turns and their
/// inline tool cards, NOT the in-flight stream. This is the O(transcript) walk
/// (it re-renders markdown + cards) but it only changes when a turn completes,
/// a card lands, or width/theme flips — never on a per-frame animation tick.
/// The render path caches its wrapped result keyed on a signature that
/// excludes the streaming tick, so it stays a stable cache HIT across every
/// frame of a live stream.
fn build_settled_transcript_lines(
    app: &App,
    theme: &Theme,
    content_width: u16,
    editable_prefix: Option<&str>,
    selected_choice: usize,
    expanded: bool,
) -> Vec<Line<'static>> {
    let session = &app.session;
    let mut lines: Vec<Line<'static>> = Vec::new();

    for (turn_idx, turn) in session.turns.iter().enumerate() {
        push_turn(
            &mut lines,
            turn,
            theme,
            &session.tool_cards,
            app,
            content_width,
            editable_prefix,
            selected_choice,
            expanded,
            turn_idx,
        );
        lines.push(Line::from(""));
    }

    // v0.9.1.2 F12: tool cards no longer render in a trailing block at
    // the end of the transcript — that piled prior turns' cards under
    // the latest response and left assistant text floating ABOVE the
    // tool it introduced. `push_turn` now walks `turn.elements` in
    // document order and looks up the matching `ToolCardModel` for each
    // `TurnElement::ToolCard(id)` in `session.tool_cards`, so cards
    // interleave inline. The previous trailer is gone.

    lines
}

/// D009: the LIVE half of the transcript — thinking, the in-flight streamed
/// text, and the animated "working" status line. This is the part that
/// animates every frame; the render path rebuilds ONLY this (over a bounded
/// tail of the streaming buffer) so a live stream costs O(viewport) per frame
/// instead of re-wrapping the whole turn.
///
/// `stream` is the streaming buffer to render — normally `session.streaming`,
/// but the stream-windowing path passes only its visible TAIL so the wrap stays
/// viewport-bounded under a huge live turn.
fn build_live_tail_lines(lines: &mut Vec<Line<'static>>, app: &App, theme: &Theme, stream: &str) {
    let session = &app.session;

    // The in-flight assistant turn: thinking first (dimmed), then the
    // streamed text. v0.9.1 W1 A drops the explicit `wayland` role
    // label from the streaming block (the 2-space gutter is the role
    // signal; the streaming-status widget in the composer carries the
    // personality) and renders the partial buffer through markdown via
    // `safe_split` so a mid-fence chunk doesn't flicker as half-rendered
    // raw text.
    if !session.thinking.is_empty() {
        lines.push(Line::from(Span::styled(
            "  thinking…",
            Style::default()
                .fg(theme.text_muted)
                .add_modifier(Modifier::ITALIC),
        )));
        for line in session.thinking.lines() {
            lines.push(Line::from(Span::styled(
                format!("  {line}"),
                Style::default().fg(theme.text_muted),
            )));
        }
    }
    if !stream.is_empty() {
        // v0.9.1.2 F17: stream the live tail at the same 2-space indent
        // as the completed assistant text — no gutter spinner glyph.
        push_streaming_preview(lines, stream, theme);
    }

    // 2026-05-31: render the animated "working" line INLINE, directly under
    // the in-flight text, whenever a turn is live. The user's feedback: with
    // the indicator pinned to the bottom composer and the (often short)
    // streamed text sitting at the TOP of a tall pane, the motion felt
    // detached — "it feels like it's stumbling". Co-locating it with the text
    // is where the eye is. It is indented to the 2-space assistant gutter so
    // it reads as a continuation of the response, not the orphan column-0
    // glyph the F17 note warned about. The composer no longer renders the
    // duplicate. `render_streaming_status` shows the awaiting-approval label
    // itself, so it stays correct across every streaming phase.
    if session.streaming_active {
        let status = render_streaming_status(session, app.frame_tick, theme);
        lines.push(indent_line(status, 2));
    }
}

/// D009 (stream windowing): the byte offset in `buffer` at which the visible
/// TAIL begins — enough trailing content to fill a `height`-row viewport at
/// `width` columns plus overscan, snapped to a line boundary so markdown
/// renders whole lines. Returns `0` when the whole buffer already fits in the
/// budget. The cap is `(height + overscan) * width` VISIBLE chars: even if
/// every char hard-wraps to its own row, that many chars cannot produce more
/// than the budgeted rows, so wrapping only this tail is O(viewport) regardless
/// of the total turn size — the live-turn half of the render livelock fix.
///
/// The earlier settled content is NOT dropped from the user's view: the render
/// concatenates this tail BELOW the (cached) settled turns and bottom-anchors
/// the window, so the rows above the streamed tail come from the settled cache.
/// Windowing the live buffer is only used on the bottom-anchored path (the user
/// is reading the newest text as it arrives); a scrolled-up reader takes the
/// full unbounded build.
fn streaming_visible_tail_offset(buffer: &str, width: u16, height: u16) -> usize {
    let width = (width as usize).max(1);
    // Budget = viewport height + a few rows of overscan so a partial top row
    // never starves the window.
    let budget_rows = height as usize + 4;

    // Two independent caps, both O(viewport):
    //
    //   line cap: the last `budget_rows` LOGICAL lines. For prose (one short
    //   line per row) this alone bounds the wrapped result to ~budget_rows.
    //
    //   char cap: the last `budget_rows * width` chars. For a pathological
    //   single over-wide line (no newlines) the line cap would keep the whole
    //   buffer, so this hard-caps the char count — even if every char hard-wraps
    //   to its own row, that many chars cannot exceed budget_rows.
    //
    // Take the LATER (larger byte offset) of the two starts, i.e. the SMALLER
    // tail, so whichever cap is tighter wins. The result wraps to at most
    // ~budget_rows visual rows regardless of buffer size — the O(viewport)
    // guarantee. Both scans walk only the tail, never the whole buffer.
    let line_cap_start = nth_line_start_from_end(buffer, budget_rows);

    let char_cap_start = {
        let char_budget = budget_rows.saturating_mul(width);
        // Walk BACKWARD from the end, counting at most `char_budget` chars, and
        // stop the instant the budget is met. This keeps the last `char_budget`
        // chars (the visible tail) in O(viewport) — the prior implementation did
        // `buffer.chars().count()` + `char_indices().nth(skip)`, both O(buffer),
        // scanning the WHOLE accumulated streaming turn every frame (audit
        // findings #17 / #20). The visible OUTPUT is identical: this lands on the
        // same byte offset the forward `nth(total - budget)` produced — the char
        // that is `char_budget` chars from the end, or 0 when the buffer holds
        // at most `char_budget` chars.
        let mut start = buffer.len();
        let mut counted = 0usize;
        for (i, _) in buffer.char_indices().rev() {
            start = i;
            counted += 1;
            if counted == char_budget {
                break;
            }
        }
        // `counted < char_budget` ⇒ the whole buffer fits the budget ⇒ no
        // windowing (the forward path returned 0 here too).
        if counted < char_budget {
            0
        } else {
            // Snap forward to a line boundary so markdown sees whole lines: find
            // the newline immediately PRECEDING `start` and begin just after it.
            // Bound the backward search to a `char_budget`-byte window below
            // `start` instead of scanning `[..start]` — a buffer with NO newline
            // at all (the pathological single over-wide line the char cap
            // defends) would otherwise make `rfind` scan O(buffer), the very
            // cost this rewrite removes. The bound preserves O(viewport); it
            // matches the forward `nth`-based original on every normal buffer,
            // because the line containing `start` begins within the kept tail
            // (any newline farther back than `char_budget` bytes is outside the
            // window we render anyway, so snapping to it would only re-introduce
            // unbounded content).
            let mut snap_floor = start.saturating_sub(char_budget);
            while snap_floor > 0 && !buffer.is_char_boundary(snap_floor) {
                snap_floor -= 1;
            }
            match buffer[snap_floor..start].rfind('\n') {
                Some(rel) => snap_floor + rel + 1,
                None => start,
            }
        }
    };

    line_cap_start.max(char_cap_start)
}

/// D009 helper: the byte offset at which the last `n` LINES of `buffer` begin
/// (the start of the line that is `n` newlines back from the end). `0` when the
/// buffer has at most `n` lines. Walks backward from the end, so it touches only
/// the trailing `n` lines — O(viewport), not O(buffer).
fn nth_line_start_from_end(buffer: &str, n: usize) -> usize {
    if n == 0 {
        return buffer.len();
    }
    let mut seen = 0usize;
    // Iterate newline byte positions from the end. The (n)th newline from the
    // end marks the boundary just before the last `n` lines.
    for (i, b) in buffer.bytes().enumerate().rev() {
        if b == b'\n' {
            seen += 1;
            if seen == n {
                return i + 1;
            }
        }
    }
    0
}

/// D009: wrap a list of LOGICAL lines into POST-WRAP visual rows at `width`,
/// preserving per-span styling. This replaces ratatui's internal
/// `Wrap { trim: false }` so the windowed render can slice the visual rows
/// directly — making THIS function (not a ratatui re-wrap) the single source
/// of truth for both the visual line count and the scroll math.
///
/// The wrap is a greedy word wrap matching `Wrap { trim: false }` semantics:
/// leading whitespace on a logical line is preserved, words are kept whole
/// when they fit, and a single word wider than `width` is hard-split at the
/// `width` boundary. Spans are split at the wrap point so styling carries
/// across the break. An empty logical line stays one (empty) visual row.
fn wrap_lines_to_width(logical: Vec<Line<'static>>, width: u16) -> Vec<Line<'static>> {
    if width == 0 {
        return logical;
    }
    let width = width as usize;
    let mut out: Vec<Line<'static>> = Vec::with_capacity(logical.len());
    for line in logical {
        wrap_one_line(line, width, &mut out);
    }
    out
}

/// Wrap a single logical [`Line`] into one or more visual rows, appending
/// them to `out`. Walks the line as a flat sequence of styled chars and
/// breaks at word boundaries, falling back to a hard split for an over-wide
/// single word. Width is measured in Unicode scalar values (char count) for
/// VISIBLE chars, matching the existing crate wrappers
/// (`osc8::split_segments`, `plan_review::wrap_text`).
///
/// OSC 8 hyperlink escape spans (emitted by the markdown/link path as their
/// OWN spans, content starting `\x1b]8`) carry ZERO display width — the same
/// intent the osc8 module documents so width accounting "sees only the
/// visible text". Counting them would split a fitting line early and corrupt
/// a link mid-escape, so escape-span chars contribute 0 to the row width and
/// are never break points.
fn wrap_one_line(line: Line<'static>, width: usize, out: &mut Vec<Line<'static>>) {
    // Flatten to (char, style, char_width). `Line::style` is the line-level
    // base; per-span styles layer on top, so each emitted row inherits the
    // line style and carries the span styles.
    let line_style = line.style;
    let line_alignment = line.alignment;
    let mut chars: Vec<(char, Style, usize)> = Vec::new();
    let mut total_w = 0usize;
    for span in &line.spans {
        // A pure OSC 8 escape span contributes no visible width.
        let zero_width = span.content.contains('\x1b');
        for ch in span.content.chars() {
            let w = if zero_width { 0 } else { 1 };
            total_w += w;
            chars.push((ch, span.style, w));
        }
    }

    // Fast path: the whole line already fits (by VISIBLE width).
    if total_w <= width || chars.is_empty() {
        out.push(line);
        return;
    }

    // Greedy wrap. `cur` accumulates the current visual row; `cur_w` its
    // visible width. `last_break` records the index in `cur` just after the
    // last whitespace, so we can rewind a too-long word to a clean boundary.
    let mut cur: Vec<(char, Style, usize)> = Vec::new();
    let mut cur_w = 0usize;
    let mut last_break: Option<usize> = None;

    let flush = |cur: &mut Vec<(char, Style, usize)>, out: &mut Vec<Line<'static>>| {
        out.push(build_styled_row(cur, line_style, line_alignment));
        cur.clear();
    };

    for (ch, style, w) in chars {
        if cur_w + w > width && !cur.is_empty() {
            // The row is full. Prefer breaking at the last word boundary.
            match last_break {
                Some(bp) if bp > 0 && bp < cur.len() => {
                    let tail: Vec<(char, Style, usize)> = cur.split_off(bp);
                    flush(&mut cur, out);
                    cur = tail;
                    cur_w = cur.iter().map(|(_, _, w)| *w).sum();
                }
                _ => {
                    // No usable break (one over-wide word): hard split here.
                    flush(&mut cur, out);
                    cur_w = 0;
                }
            }
            last_break = None;
        }
        if ch == ' ' {
            // Record the break point AFTER this space so the trailing space
            // stays with the current row (matches `trim: false`).
            cur.push((ch, style, w));
            cur_w += w;
            last_break = Some(cur.len());
        } else {
            cur.push((ch, style, w));
            cur_w += w;
        }
    }
    if !cur.is_empty() {
        flush(&mut cur, out);
    }
}

/// Rebuild a visual row from `(char, style)` pairs, coalescing runs of the
/// same style back into [`Span`]s so the row carries the original styling.
fn build_styled_row(
    chars: &[(char, Style, usize)],
    line_style: Style,
    alignment: Option<Alignment>,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut run = String::new();
    let mut run_style: Option<Style> = None;
    for (ch, style, _w) in chars {
        match run_style {
            Some(s) if s == *style => run.push(*ch),
            _ => {
                if let Some(s) = run_style.take() {
                    spans.push(Span::styled(std::mem::take(&mut run), s));
                }
                run.push(*ch);
                run_style = Some(*style);
            }
        }
    }
    if let Some(s) = run_style {
        spans.push(Span::styled(run, s));
    }
    let mut row = Line::from(spans);
    row.style = line_style;
    row.alignment = alignment;
    row
}

/// D009: render ONLY the viewport-sized window of the pre-wrapped transcript
/// rows. This is the per-frame hot path; its cost is O(viewport), independent
/// of the total transcript size — the windowing fix for the render livelock.
///
/// `wrapped` is the full list of POST-WRAP visual rows. `scroll_offset` is the
/// number of rows to scroll upward from the bottom (0 = bottom-anchored). We
/// compute the bottom anchor against `wrapped.len()` (the exact visual count,
/// since these rows ARE the rendered layout), materialize the visible slice
/// plus a small overscan, and render it with wrapping DISABLED (the rows are
/// already wrapped).
fn render_transcript_window(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    wrapped: &[Line<'static>],
    scroll_offset: u16,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    // A few rows of overscan above the window absorb off-by-one wrap rounding
    // and keep partial top rows clean; the render still clips to `area`.
    const OVERSCAN: usize = 2;

    let total = wrapped.len().min(u16::MAX as usize) as u16;
    let bottom_anchor = total.saturating_sub(area.height);
    // Clamp the upward offset so we can never scroll past the top.
    let upward = scroll_offset.min(bottom_anchor);
    let top_row = bottom_anchor.saturating_sub(upward) as usize;

    // Window = [start, end) over the wrapped rows, with overscan on top so the
    // Paragraph's own `scroll` can shave the overscan back off. This bounds
    // the materialized line count to `area.height + OVERSCAN`, regardless of
    // how many rows the full transcript holds.
    let start = top_row.saturating_sub(OVERSCAN);
    let end = (top_row + area.height as usize).min(wrapped.len());
    let window: Vec<Line<'static>> = wrapped[start..end].to_vec();
    // Rows inside the window that sit above the viewport top (the overscan).
    let inner_scroll = (top_row - start) as u16;

    let para = Paragraph::new(window).style(Style::default().bg(theme.bg));
    frame.render_widget(para.scroll((inner_scroll, 0)), area);
}

/// D037 — the maximum number of file rows the "Files changed this turn"
/// card prints. Past this point the card would crowd the transcript, so the
/// overflow is collapsed into a single `… and N more` summary row (the card
/// is a reviewable hint, not an exhaustive manifest).
const FILES_CHANGED_MAX_ROWS: usize = 6;

/// D037 — render a compact "Files changed this turn" summary card.
///
/// After an assistant turn that edited files there must be a single,
/// reviewable "files changed" unit in the transcript so the deliverable is
/// not reconstructed from prose. The bridge attaches the touched paths to
/// the turn as a [`TurnElement::FilesChanged`]; this widget turns them into a
/// small footer block:
///
/// ```text
/// Files changed (3):
///   crates/wcore-cli/src/main.rs
///   crates/wcore-cli/src/tui/app.rs
///   … and 1 more
/// ```
///
/// The header carries the count so a glance reads how many files the turn
/// touched. Rows cap at [`FILES_CHANGED_MAX_ROWS`]; the overflow collapses to
/// one `… and N more` row rather than scrolling the whole list. An empty path
/// list returns zero lines so the caller can append unconditionally without
/// producing a stray header. The whole block paints in `theme.text_muted` so
/// it reads as a footer affordance distinct from body text.
pub fn render_files_changed(paths: &[String], theme: &Theme) -> Vec<Line<'static>> {
    if paths.is_empty() {
        return Vec::new();
    }
    let style = Style::default().fg(theme.text_muted);
    let mut lines: Vec<Line<'static>> =
        Vec::with_capacity(paths.len().min(FILES_CHANGED_MAX_ROWS) + 2);
    lines.push(Line::from(Span::styled(
        format!("Files changed ({}):", paths.len()),
        style,
    )));
    for path in paths.iter().take(FILES_CHANGED_MAX_ROWS) {
        lines.push(Line::from(Span::styled(format!("  {path}"), style)));
    }
    let overflow = paths.len().saturating_sub(FILES_CHANGED_MAX_ROWS);
    if overflow > 0 {
        lines.push(Line::from(Span::styled(
            format!("  … and {overflow} more"),
            style,
        )));
    }
    lines
}

/// Append one completed turn to `lines`, styled by its role.
///
/// v0.9.1 W1 A: Assistant turns no longer render a `"wayland"` role
/// label — per the HTML mockup §5.2 the 2-space gutter (user has a `›`
/// glyph, assistant has plain indentation) is the only role signal we
/// need, and the explicit label conflicted with the personality the
/// streaming-status widget already carries (the `Leavening / Brewing /
/// Steeping` verb rotation). Assistant body text is rendered through
/// [`render_markdown`] so bold/italic/lists/code-fences/headings/links
/// land styled instead of raw.
// 8 args is one above clippy's default 7. The extra `turn_index` is needed
// by the v0.9.3 W1.4 Thinking projection to key the per-turn expand state in
// `App::reasoning_expanded`; a refactor to a builder would obscure the
// hot-path render flow without a real benefit.
#[allow(clippy::too_many_arguments)]
fn push_turn(
    lines: &mut Vec<Line<'static>>,
    turn: &TurnView,
    theme: &Theme,
    tool_cards: &[ToolCardModel],
    app: &App,
    content_width: u16,
    editable_prefix: Option<&str>,
    selected_choice: usize,
    // Whether the head approval card body is expanded (Ctrl+F) — drives the
    // permission card's full-vs-clamped render.
    expanded: bool,
    turn_index: usize,
) {
    match turn.role {
        TurnRole::User => {
            // v0.9.1.2 F16: OpenClaw-style user-message highlight. Every
            // wrapped line of the user message gets a column-0 `▌`
            // (U+258C LEFT HALF BLOCK) in the brand accent plus a
            // slightly raised background tint (`surface_hover`, #262626)
            // on the message body, so the user can skim "what I said
            // vs. what the agent said back" at a glance. The previous
            // `›` first-line marker is gone — the full-height bar is
            // the clearer signal and avoids confusing the leading-line
            // marker with composer-prompt or banner glyphs.
            //
            // v0.9.1.2 F16-followup: extend the surface_hover tint to the
            // full transcript width by appending a trailing pad span of
            // bg-styled whitespace, so each line reads as a contiguous
            // "card" filling the row instead of stopping at end-of-text.
            // The body span uses ratatui's `Span::width()` (unicode-width
            // aware) so multi-byte chars / emoji measure correctly.
            let body_style = Style::default()
                .fg(theme.text)
                .bg(theme.surface_hover)
                .add_modifier(Modifier::BOLD);
            let pad_style = Style::default().bg(theme.surface_hover);
            for line in turn.text().lines() {
                let bar = Span::styled(
                    "▌",
                    Style::default()
                        .fg(theme.orange)
                        .add_modifier(Modifier::BOLD),
                );
                let body = Span::styled(format!(" {line}"), body_style);
                // Bar is 1 column wide (▌ has display width 1); body span
                // already includes the leading space. Total prefix width
                // is `bar.width() + body.width()`. Pad to content_width
                // with bg-styled spaces; saturate to 0 if the body
                // overflows (defensive — wrap is upstream) or if
                // content_width is 0 (cold-boot before first render).
                let used = (bar.width() + body.width()) as u16;
                let pad_len = content_width.saturating_sub(used) as usize;
                let mut spans = vec![bar, body];
                if pad_len > 0 {
                    spans.push(Span::styled(" ".repeat(pad_len), pad_style));
                }
                lines.push(Line::from(spans));
            }
        }
        TurnRole::Assistant => {
            // v0.9.1 W1 A: walk per element with the real renderers.
            // `Markdown` goes through [`render_markdown`] (W2 C1) so the
            // body lands styled; `Sources` is rendered with the widget;
            // `Thinking` is skipped (ephemeral, never in transcript per
            // the A2 contract). Each Markdown line gets a 2-space gutter
            // so the visual indent matches the spec mockup.
            //
            // v0.9.1.2 F12: `ToolCard(call_id)` looks the matching
            // `ToolCardModel` up in `tool_cards` and renders the card at
            // THIS position in the element flow, so the user reads
            // `text → tool call → result → next text` in document order
            // instead of seeing every card pile up below the response.
            // A `ToolCard` whose `call_id` no longer resolves is a
            // graceful no-op (stale reference from a cleared session).
            for element in &turn.elements {
                match element {
                    TurnElement::Markdown(s) => {
                        // v0.9.1.2 F11-followup: pass the viewport budget
                        // minus the 2-space assistant gutter so wide
                        // tables fall back to a bullet list instead of
                        // rendering as misaligned columns with wrapped
                        // pipes (Sean's screenshot bug).
                        let md_width = content_width.saturating_sub(2);
                        let (md_lines, _urls) = render_markdown_with_width(s, theme, md_width);
                        for line in md_lines {
                            lines.push(indent_line(line, 2));
                        }
                    }
                    TurnElement::Sources(urls) => {
                        // The widget returns zero lines for an empty URL
                        // list, so an accidental empty Sources element
                        // produces no stray header.
                        for line in render_sources(urls, theme) {
                            lines.push(line);
                        }
                    }
                    TurnElement::FilesChanged(paths) => {
                        // D037: a reviewable "files changed this turn" card.
                        // A blank gutter row above it separates the card from
                        // the preceding body so it reads as a distinct footer
                        // unit. The widget returns zero lines for an empty
                        // path list, so an accidental empty element produces
                        // no stray header.
                        let rows = render_files_changed(paths, theme);
                        if !rows.is_empty() {
                            lines.push(Line::from(""));
                            for line in rows {
                                lines.push(line);
                            }
                        }
                    }
                    TurnElement::Thinking { body, secs, tokens } => {
                        // v0.9.3 W1.4 — render the collapsed reasoning
                        // projection. Per-turn expand state lives in
                        // `App::reasoning_expanded` keyed by turn index;
                        // absent or `false` ⇒ one-line `▶ Thought: <title>`,
                        // `true` ⇒ `▼ Thought: <title>` + wrapped body.
                        let expanded = app
                            .reasoning_expanded
                            .get(&turn_index)
                            .copied()
                            .unwrap_or(false);
                        for line in
                            crate::tui::render::reasoning_filter::reasoning_collapsed_lines_themed(
                                body, *secs, *tokens, expanded, theme,
                            )
                        {
                            lines.push(line);
                        }
                    }
                    TurnElement::ToolCard(call_id) => {
                        if let Some(card) = tool_cards.iter().find(|c| &c.call_id == call_id) {
                            // Blank gutter row above the card keeps it
                            // visually separated from the preceding
                            // markdown block.
                            lines.push(Line::from(""));
                            push_tool_card_lines(
                                lines,
                                card,
                                theme,
                                app.session.compact_tool_output,
                                app,
                                content_width,
                                editable_prefix,
                                selected_choice,
                                expanded,
                            );
                        }
                        // A missing card is a stale reference — render
                        // nothing rather than panic.
                    }
                }
            }
        }
        TurnRole::System => {
            for line in turn.text().lines() {
                lines.push(Line::from(Span::styled(
                    format!("⚙ {line}"),
                    Style::default().fg(theme.text_dim),
                )));
            }
        }
    }
}

/// Prepend `n` spaces to a [`Line`], preserving existing spans + style.
/// Used to indent rendered-markdown lines into the assistant gutter
/// without round-tripping through a string (which would drop styling).
fn indent_line(line: Line<'static>, n: usize) -> Line<'static> {
    if n == 0 {
        return line;
    }
    let indent = " ".repeat(n);
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len() + 1);
    spans.push(Span::raw(indent));
    spans.extend(line.spans);
    let mut out = Line::from(spans);
    out.style = line.style;
    out.alignment = line.alignment;
    out
}

/// Render the in-flight streaming buffer into `lines` using markdown
/// rendering up to the last safe split point.
///
/// Why split before render: pulldown-cmark on a partial buffer ending
/// mid-fence emits half-rendered output that flickers as the chunk
/// completes. [`last_safe_split_point`] walks the buffer and returns
/// the byte offset where every markdown construct is closed. Up to
/// that offset we render through `render_markdown`; the unsafe tail
/// (still arriving) renders as plain text so the user sees the live
/// typing tail without the flicker. If the WHOLE buffer is unsafe
/// (one open fence is the textbook case), we render nothing — the next
/// chunk will land a safe split soon.
///
/// v0.9.1.2 F17: the live tail is indented to match the completed
/// assistant text (2-space gutter, no glyph). The `render_streaming_status`
/// widget in the composer already owns the "wayland is working" affordance
/// (the rotating verb + elapsed + token counter), so a duplicate gutter
/// spinner in the transcript was redundant — and the prior implementation
/// painted it at column 0 below the indented assistant text, which read
/// as an orphan glyph on a stray line.
fn push_streaming_preview(lines: &mut Vec<Line<'static>>, buffer: &str, theme: &Theme) {
    let split = last_safe_split_point(buffer);
    let (safe, tail) = buffer.split_at(split);

    if !safe.is_empty() {
        let (md_lines, _urls) = render_markdown(safe, theme);
        for line in md_lines {
            lines.push(indent_line(line, 2));
        }
    }
    if tail.is_empty() {
        return;
    }
    for line in tail.lines() {
        lines.push(Line::from(vec![
            Span::raw("  "),
            Span::styled(line.to_string(), Style::default().fg(theme.text)),
        ]));
    }
}

/// Append a tool-call card's lines for the inline transcript flow.
///
/// One row in compact mode (the W3 D1 default), expanded body in full
/// mode (Ctrl+E or for an error card). The card's status drives the
/// gutter glyph + colour so a glance reads pending/running/done/error/
/// rejected without parsing the chip text.
#[allow(clippy::too_many_arguments)]
fn push_tool_card_lines(
    lines: &mut Vec<Line<'static>>,
    card: &ToolCardModel,
    theme: &Theme,
    compact: bool,
    app: &App,
    width: u16,
    editable_prefix: Option<&str>,
    // The live AskUserQuestion choice index, used to highlight the picked
    // row in the inline approval card (mirrors `WorkspaceSurface::approval_sel`).
    selected_choice: usize,
    // Whether the head approval card body is expanded (Ctrl+F) — drives the
    // permission card's full-vs-clamped render.
    expanded: bool,
) {
    match card.status {
        ToolCardStatus::Running => {
            // Spinner row — `  <glyph> <tool_name>(<args>) · running… <elapsed>s`.
            // Args are the input_pretty collapsed (the existing args_summary
            // helper lives in toolcard.rs as private; replicate the contract
            // inline for the one-line summary here).
            let glyph_idx = (app.frame_tick / SPINNER_TICKS_PER_FRAME) as usize % 4;
            let glyph = ["◐", "◓", "◑", "◒"][glyph_idx];
            let args = compact_args_preview(&card.input_pretty);
            lines.push(Line::from(vec![
                // v0.9.1.3 J: running-spinner glyph demoted from
                // `theme.orange` to `theme.text_dim` per the accent-
                // inflation audit. The rotating glyph animation
                // already signals "in flight"; orange was redundant
                // and consumed brand budget. The user-turn `▌` and
                // active-tab underline are the only retained orange
                // surfaces.
                Span::styled(
                    format!("  {glyph} "),
                    Style::default()
                        .fg(theme.text_dim)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    card.tool_name.clone(),
                    Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("({args})"), Style::default().fg(theme.text_dim)),
                Span::styled(" · ", Style::default().fg(theme.text_muted)),
                Span::styled("running…", Style::default().fg(theme.text_muted)),
            ]));
        }
        ToolCardStatus::AwaitingApproval => {
            // v0.9.2 W2 (SPEC §0 #1, §1C): SINGLE-SURFACE one-card queue.
            // Only the FIRST pending card renders the inline approval
            // dialog; any other `AwaitingApproval` card later in document
            // order renders nothing (the user decides one at a time, CC
            // dequeue model). The head card's title carries a
            // `(+N more pending)` tail so the queue depth is visible.
            let mut pending = app
                .session
                .tool_cards
                .iter()
                .filter(|c| c.status == ToolCardStatus::AwaitingApproval);
            let is_head = pending
                .next()
                .is_some_and(|head| head.call_id == card.call_id);
            if !is_head {
                return;
            }
            let pending_count = 1 + app
                .session
                .tool_cards
                .iter()
                .filter(|c| {
                    c.status == ToolCardStatus::AwaitingApproval && c.call_id != card.call_id
                })
                .count();

            // v0.9.2 W3: build the permission context with the live
            // interaction state — the editable prefix (Bash/PowerShell
            // always-allow edit) AND the selected choice index (the
            // AskUserQuestion arrow-nav). Both must be threaded here, not
            // defaulted, or the rendered card cannot reflect the keys that
            // move it (the v0.9.6 phantom-affordance fix: arrows moved
            // `approval_sel` but the marker was frozen on choice 0).
            let ctx = crate::tui::permission::PermissionContext {
                card,
                theme,
                width,
                always_allow_available: true,
                editable_prefix,
                selected_choice,
                expanded,
            };
            let mut dialog = crate::tui::permission::render(card, &ctx);
            // The dialog header is the 3rd line (`[blank, rule, header,
            // …body, blank, keys]`). Append the queue-depth tail there.
            if pending_count > 1 && dialog.len() > 2 {
                dialog[2].spans.push(Span::styled(
                    format!("  (+{} more pending)", pending_count - 1),
                    Style::default().fg(theme.text_dim),
                ));
            }
            for line in dialog {
                lines.push(line);
            }
        }
        ToolCardStatus::Ok => {
            // Header: `  ● tool_name(args) · done`. Then a per-tool
            // formatter summary line below (v0.9.1.1 bonus fix — the
            // formatters were dead code from this render path; the body
            // now actually formats).
            push_tool_card_oneliner(lines, card, theme, "done", theme.success);
            push_tool_card_formatter_body(lines, card, theme, compact);
        }
        ToolCardStatus::Err => {
            push_tool_card_oneliner(lines, card, theme, "error", theme.error);
            // Error continuation — always expanded (compact=false) so
            // the error never hides behind a truncated summary. The
            // formatter's summary is preferred over a raw dump; if the
            // formatter declines (empty summary) we fall back to the
            // raw output's first lines so a non-JSON error still shows.
            push_tool_card_formatter_body(lines, card, theme, /* compact = */ false);
        }
        ToolCardStatus::Cancelled => {
            // Rejected/cancelled — muted strikethrough.
            let args = compact_args_preview(&card.input_pretty);
            lines.push(Line::from(vec![
                Span::styled("  ⊘ ", Style::default().fg(theme.text_muted)),
                Span::styled(
                    card.tool_name.clone(),
                    Style::default()
                        .fg(theme.text_muted)
                        .add_modifier(Modifier::CROSSED_OUT),
                ),
                Span::styled(
                    format!("({args})"),
                    Style::default()
                        .fg(theme.text_muted)
                        .add_modifier(Modifier::CROSSED_OUT),
                ),
                Span::styled(" · ", Style::default().fg(theme.text_muted)),
                Span::styled("rejected by user", Style::default().fg(theme.text_muted)),
            ]));
        }
    }
}

/// One-line summary row for a tool card — `  <icon> <name>(<args>) · <chip>`.
fn push_tool_card_oneliner(
    lines: &mut Vec<Line<'static>>,
    card: &ToolCardModel,
    theme: &Theme,
    chip: &str,
    chip_color: ratatui::style::Color,
) {
    // v0.9.2 W11-integ (S20): use the SAME glyph map W7 locked in the
    // standalone `widgets/toolcard.rs::status_icon` so the live inline
    // path matches the full-card variant — `◐` running · `●` done ·
    // `○` cancelled · `⊘` awaiting-approval · `✗` error. (The `◑`
    // stalled glyph is not reachable here; the inline card carries no
    // stall flag.)
    let icon = match card.status {
        ToolCardStatus::Ok => "●",
        ToolCardStatus::Err => "✗",
        ToolCardStatus::Running => "◐",
        ToolCardStatus::AwaitingApproval => "⊘",
        ToolCardStatus::Cancelled => "○",
    };
    let args = compact_args_preview(&card.input_pretty);
    lines.push(Line::from(vec![
        Span::styled(
            format!("  {icon} "),
            Style::default().fg(chip_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            card.tool_name.clone(),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!("({args})"), Style::default().fg(theme.text_dim)),
        Span::styled(" · ", Style::default().fg(theme.text_muted)),
        Span::styled(chip.to_string(), Style::default().fg(chip_color)),
    ]));
}

/// v0.9.1.1 bonus fix: render the per-tool formatter's body lines below
/// a tool card's one-liner header.
///
/// Before this, the inline render path emitted only the `<icon>
/// tool_name(<args>) · done` header and (for errors) `output.lines()
/// .take(4)` raw — every formatter under `tool_formatters/` was dead
/// code from this path. Now:
///
/// * **Compact mode** (`compact=true`, default): emit only the
///   formatter's `summary_line` as a single dimmed continuation row.
///   This is the C2/C3 spec's compact card body.
/// * **Expanded mode** (`compact=false`, Ctrl+E toggled or any error
///   card): emit `summary_line` PLUS up to 6 `detail_lines` so the
///   user can see the full breakdown without a separate full-screen
///   widget render.
///
/// Errors always force expanded mode regardless of the session
/// preference (errors are rare and load-bearing).
///
/// The body is only rendered when the formatter produces a non-empty
/// summary; that lets the generic fallback decline gracefully on a
/// payload it can't make sense of and keeps the transcript from
/// printing `completed in 0.0s` filler for every successful Edit.
/// Per-thread cap for the tool-card formatter memo. Tool-heavy sessions can
/// accumulate hundreds of completed cards; 2048 distinct renders bounds memory
/// while covering very long sessions. FIFO-evicted entries just re-format once.
const TOOL_CARD_CACHE_CAP: usize = 2048;

thread_local! {
    /// Memo of [`push_tool_card_formatter_body`] output, keyed on everything the
    /// render depends on (output, tool, compact, error-status, theme). A
    /// completed card's output is immutable, so after the first frame its body
    /// is served as a clone instead of re-parsing the output JSON + re-running
    /// the formatter every frame — the 2026-05-31 P3 sink, worst for tool-heavy
    /// sessions. The memo cannot go stale: every render input is part of the key.
    static TOOL_CARD_CACHE: std::cell::RefCell<ToolCardMemo> =
        std::cell::RefCell::new(ToolCardMemo::new());
}

/// FIFO-bounded map from a tool-card render key to its rendered body lines.
/// Mirrors `render::markdown::MarkdownMemo`.
struct ToolCardMemo {
    map: std::collections::HashMap<u64, Vec<Line<'static>>>,
    order: std::collections::VecDeque<u64>,
}

impl ToolCardMemo {
    fn new() -> Self {
        Self {
            map: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
        }
    }

    fn get(&self, key: u64) -> Option<Vec<Line<'static>>> {
        self.map.get(&key).cloned()
    }

    fn insert(&mut self, key: u64, value: Vec<Line<'static>>) {
        if self.map.insert(key, value).is_none() {
            self.order.push_back(key);
            if self.order.len() > TOOL_CARD_CACHE_CAP
                && let Some(evicted) = self.order.pop_front()
            {
                self.map.remove(&evicted);
            }
        }
    }
}

/// Hash everything the formatter body render depends on into the memo key. The
/// output (parsed into the payload), the tool name (picks the formatter), the
/// compact flag (gates detail lines), the error status (picks the body color),
/// and the theme palette. `Duration::ZERO` is passed to `summary_line`
/// unconditionally, so time is NOT an input — the render is a pure function of
/// these, which is what makes it safe to memoize.
fn tool_card_cache_key(card: &ToolCardModel, theme: &Theme, compact: bool) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    card.tool_name.hash(&mut h);
    card.output.hash(&mut h);
    compact.hash(&mut h);
    // Only the Err discriminant changes the render (body color); Running/Done
    // both use `text_dim`.
    matches!(card.status, ToolCardStatus::Err).hash(&mut h);
    crate::tui::render::markdown::hash_theme(theme, &mut h);
    h.finish()
}

/// Test-only probes for the tool-card memo: number of cached entries and a
/// reset. Mirror `render::markdown::memo_len` — let tests assert hit/miss
/// without exposing the thread-local.
#[cfg(test)]
fn tool_card_cache_len() -> usize {
    TOOL_CARD_CACHE.with(|c| c.borrow().map.len())
}

#[cfg(test)]
fn tool_card_cache_clear() {
    TOOL_CARD_CACHE.with(|c| {
        let mut m = c.borrow_mut();
        m.map.clear();
        m.order.clear();
    });
}

fn push_tool_card_formatter_body(
    lines: &mut Vec<Line<'static>>,
    card: &ToolCardModel,
    theme: &Theme,
    compact: bool,
) {
    // P3 memo: serve a previously-rendered body (including an empty body for a
    // filler card) without re-parsing the output JSON or re-running the
    // formatter. The empty-body case is cached too, so a filler card stops
    // re-parsing after its first frame.
    let key = tool_card_cache_key(card, theme, compact);
    if let Some(cached) = TOOL_CARD_CACHE.with(|c| c.borrow().get(key)) {
        lines.extend(cached);
        return;
    }
    let mut produced: Vec<Line<'static>> = Vec::new();
    format_tool_card_formatter_body(&mut produced, card, theme, compact);
    TOOL_CARD_CACHE.with(|c| c.borrow_mut().insert(key, produced.clone()));
    lines.extend(produced);
}

/// The uncached formatter-body render. [`push_tool_card_formatter_body`] is the
/// cached entry point; this does the actual parse + format work.
fn format_tool_card_formatter_body(
    lines: &mut Vec<Line<'static>>,
    card: &ToolCardModel,
    theme: &Theme,
    compact: bool,
) {
    use std::time::Duration;
    let payload = parse_card_payload(card);
    let formatter = crate::tui::tool_formatters::formatter_for(&card.tool_name);
    let summary = formatter.summary_line(&payload, Duration::ZERO);

    // The formatter's generic fallback emits "completed in 0.0s" for an
    // empty payload — that's no signal at all, so filter it out so
    // the typical successful Bash/Read/Write doesn't print
    // line-noise.
    let summary_trim = summary.trim();
    let is_filler = summary_trim.is_empty()
        || summary_trim == "completed in 0.0s"
        || summary_trim == "completed";
    if is_filler {
        // Compact path: a non-helpful summary means we say nothing.
        // Expanded path: still skip the body if the formatter has
        // nothing useful to offer; the header line already tells the
        // user the tool completed.
        return;
    }

    let body_color = match card.status {
        ToolCardStatus::Err => theme.error,
        _ => theme.text_dim,
    };
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled(summary_trim.to_string(), Style::default().fg(body_color)),
    ]));

    if !compact {
        // Expanded: detail lines (e.g. numbered web results, stdout
        // preview, edit-target path). Cap at 6 to keep the inline
        // render bounded.
        let detail = formatter.detail_lines(&payload, theme);
        for line in detail.into_iter().take(6) {
            // Indent the detail line so it visually nests under the
            // header. Detail Lines already carry their own styling.
            let mut spans = vec![Span::raw("      ")];
            spans.extend(line.spans);
            lines.push(Line::from(spans));
        }
    }
}

/// Best-effort parse of `card.output` into a `serde_json::Value` — same
/// contract as `widgets/toolcard.rs::parse_payload` (private there).
/// Empty / `None` output collapses to `Value::Null`; a non-JSON output
/// becomes a `Value::String` so formatters always receive *some*
/// value to read.
fn parse_card_payload(card: &ToolCardModel) -> serde_json::Value {
    use serde_json::Value;
    match card.output.as_deref() {
        None | Some("") => Value::Null,
        Some(s) => serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string())),
    }
}

/// Collapse a JSON-pretty `input_pretty` string into a single-line
/// args preview, truncated to a readable width.
///
/// Mirrors `widgets/toolcard.rs::args_summary` (private there). The
/// duplication is intentional — the inline path needs the same shape
/// without crossing the widget's frame-bound API.
fn compact_args_preview(input_pretty: &str) -> String {
    let trimmed = input_pretty.trim();
    if trimmed.is_empty() || trimmed == "{}" || trimmed == "null" {
        return String::new();
    }
    let mut collapsed = String::with_capacity(trimmed.len());
    let mut in_space = false;
    for ch in trimmed.chars() {
        if ch.is_whitespace() {
            if !in_space {
                collapsed.push(' ');
                in_space = true;
            }
        } else {
            collapsed.push(ch);
            in_space = false;
        }
    }
    const MAX: usize = 60;
    if collapsed.chars().count() <= MAX {
        collapsed
    } else {
        let preview: String = collapsed.chars().take(MAX.saturating_sub(1)).collect();
        format!("{preview}…")
    }
}

/// The *effective* right-rail visibility: the user's `Ctrl+B` preference
/// (`App::rail_visible`) AND a terminal wide enough to spare the rail.
///
/// Below [`RAIL_RESPONSIVE_MIN_WIDTH`] columns the rail auto-hides so the
/// transcript is not crammed into the leftover width. This is computed
/// per-frame and never written back to `App::rail_visible` — widening the
/// terminal restores the rail without the user having to press `Ctrl+B`
/// again.
fn rail_effectively_visible(app: &App, body_width: u16) -> bool {
    app.rail_visible && body_width >= RAIL_RESPONSIVE_MIN_WIDTH
}

/// True when the Path map panel has nothing to show.
fn path_map_is_empty(app: &App) -> bool {
    app.path_map.roots.is_empty()
}

/// True when the Tools panel has nothing to show.
fn tools_panel_is_empty(app: &App) -> bool {
    app.session.tool_cards.is_empty()
}

/// True when the Activity panel has nothing to show. Activity surfaces
/// system-turn notices plus *active* tool work (awaiting approval or
/// currently running) — a session whose tool calls have all finished
/// with no system notices reads as empty here even though `tool_cards`
/// is non-empty (the completed cards already render inline in the
/// transcript; v0.9.1.2 W8 dropped the parallel rail mirror).
fn activity_panel_is_empty(app: &App) -> bool {
    let no_system_notices = !app.session.turns.iter().any(|t| t.role == TurnRole::System);
    let no_active_tools = !app.session.tool_cards.iter().any(|c| {
        matches!(
            c.status,
            ToolCardStatus::Running | ToolCardStatus::AwaitingApproval
        )
    });
    no_system_notices && no_active_tools
}

/// True when every right-rail panel is empty — used to skip the rail
/// allocation entirely so the transcript reclaims the column width
/// (Sean's UX callout 2026-05-27: "if there's no fucking activity then
/// hide that shit").
///
/// v0.9.1.2 F15: the Path map panel was removed from the rail. The
/// `app.path_map` data field is still populated by the bridge (kept for
/// future widgets) but no longer factors into rail visibility.
///
/// v0.9.1.2 W8: the Tools panel was removed from the rail — tool cards
/// render inline in the transcript after F12 so the rail mirror was
/// pure duplication (recon §4.7 parallel-panel anti-pattern). Activity
/// is the only remaining rail panel and the only signal for "should
/// this rail allocate column width at all?"
fn rail_is_empty(app: &App) -> bool {
    activity_panel_is_empty(app)
}

/// Render the right rail (mockup `.rail`): the activity feed, in a
/// bordered `panel`. The panel renders ONLY when its data source is
/// non-empty — an empty panel is omitted entirely so the user never
/// sees a "no activity yet" placeholder. When the feed is empty the
/// caller (`render_body`) skips this function so the transcript
/// expands to take the freed column.
///
/// v0.9.1.2 F15: the Path map panel was removed. For 1-N files a
/// directory-tree visualization is wasted space; for many/deep files it
/// becomes unreadable.
///
/// v0.9.1.2 W8: the Tools panel was removed. After F12 the tool card
/// renders inline in the transcript at the position of the tool_use
/// block, so a parallel "Tools · N call(s)" rail summary was pure
/// duplication. Recon §4.7 explicitly calls this out as an
/// anti-pattern. Activity stays because it carries a different signal
/// (recent system notices + currently-in-flight work) — see
/// [`activity_panel_is_empty`] and [`render_activity_feed`].
fn render_rail(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    // Paint the rail bg so a previous frame never bleeds through.
    let bg = Block::default().style(Style::default().bg(theme.bg));
    frame.render_widget(bg, area);

    if activity_panel_is_empty(app) {
        return;
    }

    // Activity is the sole rail tenant; it takes the whole column.
    render_activity_feed(frame, area, app, theme);
}

/// Render the activity feed — the most recent system/tool notices,
/// newest last (mockup `.feed`).
fn render_activity_feed(frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let block = panel("Activity", theme);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    // The honest activity feed derives from what the session already
    // holds: in-flight tool work and system notices. Completed/cancelled
    // tool calls are reflected in the Tools panel above; including them
    // here too would double-count.
    let mut entries: Vec<String> = Vec::new();
    for card in &app.session.tool_cards {
        let verb = match card.status {
            ToolCardStatus::Running => "running",
            ToolCardStatus::AwaitingApproval => "awaiting",
            ToolCardStatus::Ok | ToolCardStatus::Err | ToolCardStatus::Cancelled => continue,
        };
        // v0.9.1.1 F9: `card.summary` for an unknown tool falls back
        // to a compact JSON pretty-print (see `summarize_args` in
        // protocol_bridge.rs). Strip the JSON envelope so the rail
        // never shows `tool · {"x":...`.
        entries.push(format!(
            "{verb} {} · {}",
            card.tool_name,
            sanitize_activity_line(&card.summary)
        ));
    }
    for turn in &app.session.turns {
        if turn.role == TurnRole::System {
            entries.push(sanitize_activity_line(&turn.text()));
        }
    }

    // v0.9.1.2 F14: when any tool card is awaiting approval, pin a
    // warn-yellow `⊘ Pending(N)  press y` pill as the FIRST row of the
    // rail so the pending decision is impossible to miss even on a
    // long-scrolling turn. Sits ABOVE every other entry; the rest of
    // the feed clips below it if the rail is short.
    let pending_count = app
        .session
        .tool_cards
        .iter()
        .filter(|c| matches!(c.status, ToolCardStatus::AwaitingApproval))
        .count();

    // This panel is only rendered when at least one of those sources is
    // non-empty (see `activity_panel_is_empty` / `render_rail`).
    let take = inner.height as usize;
    // v0.9.1.1 F9: cap each line to the panel width minus a 4-char
    // safety margin so a long path or stray JSON tail can never
    // overflow the rail's column.
    let max_len = inner.width.saturating_sub(4) as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(take);
    if pending_count > 0 {
        let pill = clip_to_width(&format!("⊘ Pending({pending_count})  press y"), max_len);
        lines.push(Line::from(Span::styled(
            pill,
            Style::default()
                .fg(theme.warning)
                .bg(theme.surface)
                .add_modifier(Modifier::BOLD),
        )));
    }
    // Reserve the pending pill's row when the rail is short — clip the
    // tail of the rolling feed so the pill always stays on screen.
    let feed_take = take.saturating_sub(lines.len());
    let feed_lines: Vec<Line> = entries
        .iter()
        .rev()
        .take(feed_take)
        .rev()
        .map(|e| {
            let clipped = clip_to_width(e, max_len);
            Line::from(Span::styled(
                clipped,
                Style::default().fg(theme.text_dim).bg(theme.surface),
            ))
        })
        .collect();
    lines.extend(feed_lines);
    frame.render_widget(
        Paragraph::new(lines).style(Style::default().bg(theme.surface)),
        inner,
    );
}

/// v0.9.1.1 F9: convert a possibly-JSON event line into a short,
/// human-readable summary suitable for the right-rail Activity panel.
///
/// The live drive 2026-05-27 (Sean direct feedback) showed lines like
/// `cancelled text_to_speech · {"t...` — the trailing `· {"t...` is
/// the start of a JSON envelope being clipped mid-character. The
/// fix:
///
/// * If the input contains a JSON object that looks like a tool-
///   result envelope, parse it and render a `tool · status` or
///   `tool · error: …` summary from the salient fields.
/// * Otherwise, drop the JSON envelope (`{…` or `[…`) entirely and
///   keep the human-readable prefix with a trailing `…`.
/// * If the line has no JSON shape at all, return it unchanged.
fn sanitize_activity_line(line: &str) -> String {
    let trimmed = line.trim();
    // Locate the first JSON-ish opener. Either character starts the
    // raw-envelope region we want to strip.
    let brace = trimmed.find('{');
    let bracket = trimmed.find('[');
    let json_start = match (brace, bracket) {
        (Some(a), Some(b)) => Some((a.min(b), if a <= b { '{' } else { '[' })),
        (Some(a), None) => Some((a, '{')),
        (None, Some(b)) => Some((b, '[')),
        (None, None) => None,
    };

    let Some((idx, opener)) = json_start else {
        return line.to_string();
    };

    let prefix = trimmed[..idx].trim_end_matches([' ', '·', '-', ':']).trim();
    let json_blob = &trimmed[idx..];

    // Try a friendly summary by parsing the JSON.
    if opener == '{'
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(json_blob)
        && let Some(summary) = friendly_summary_from_json(&value)
    {
        if prefix.is_empty() {
            return summary;
        }
        return format!("{prefix} · {summary}");
    }

    // Fallback: drop the JSON envelope entirely with a "…" tail so
    // the user sees we trimmed something.
    if prefix.is_empty() {
        "…".to_string()
    } else {
        format!("{prefix} …")
    }
}

/// v0.9.1.1 F9: extract a short summary from a JSON value, looking
/// for the keys a tool-result envelope typically carries.
fn friendly_summary_from_json(value: &serde_json::Value) -> Option<String> {
    let obj = value.as_object()?;
    let str_field = |k: &str| obj.get(k).and_then(|v| v.as_str());

    let tool = str_field("tool").or_else(|| str_field("name"));
    let status = str_field("status").or_else(|| str_field("outcome"));
    let error = str_field("error")
        .or_else(|| str_field("err"))
        .or_else(|| str_field("message"));

    match (tool, status, error) {
        (Some(t), Some(s), _) => Some(format!("{t} · {s}")),
        (Some(t), None, Some(e)) => Some(format!("{t} · error: {}", clip_to_width(e, 40))),
        (None, Some(s), _) => Some(s.to_string()),
        (Some(t), None, None) => Some(t.to_string()),
        _ => None,
    }
}

/// v0.9.1.1 F9: clamp a string to at most `max` chars, appending `…`
/// when truncation happens. Width-only — not a `unicode-width` aware
/// implementation; the rail's monospaced font means char count ≈
/// column count for the inputs we surface here (ASCII tool names,
/// short error strings).
fn clip_to_width(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let len = s.chars().count();
    if len <= max {
        return s.to_string();
    }
    let take = max.saturating_sub(1);
    let mut out: String = s.chars().take(take).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::tui::app::App;
    use crate::tui::fixtures;
    use crate::tui::protocol_bridge::apply_event;
    use crate::tui::theme::Theme;

    /// Build an `App` by feeding a fixture's `ProtocolEvent` stream
    /// through the real bridge — the same path the live engine drives.
    fn app_from_fixture(events: Vec<wcore_protocol::events::ProtocolEvent>) -> App {
        let mut app = App::new();
        for event in events {
            apply_event(&mut app, event);
        }
        app
    }

    /// Render the surface and flatten the `TestBackend` buffer to a
    /// single string for substring assertions.
    fn render_to_string(surface: &mut WorkspaceSurface, app: &App, w: u16, h: u16) -> String {
        let theme = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), app, &theme))
            .expect("render workspace");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..h {
            for x in 0..w {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::CONTROL)
    }

    #[test]
    fn id_is_workspace() {
        assert_eq!(WorkspaceSurface::new().id(), SurfaceId::Workspace);
    }

    #[test]
    fn ctrl_f_toggles_expand_on_a_pending_card_and_resets_on_resolve() {
        // The fixture is a Bash approval card (generic, non-AskUser), so
        // Ctrl+F lands in the generic approval branch and toggles the
        // expand flag the clamped components read via `PermissionContext`.
        let mut app = app_from_fixture(fixtures::tool_call_with_approval());
        // Force the head card into AwaitingApproval so the key router hands
        // the keypress to the approval handler (the fixture lands the card in
        // a non-approval state otherwise; sibling approval tests do the same).
        app.session.tool_cards[0].status = ToolCardStatus::AwaitingApproval;
        let mut surface = WorkspaceSurface::new();
        assert!(!surface.approval_expanded, "a fresh card starts collapsed");

        surface.handle_key(ctrl(KeyCode::Char('f')), &mut app);
        assert!(surface.approval_expanded, "Ctrl+F expands the card body");
        surface.handle_key(ctrl(KeyCode::Char('f')), &mut app);
        assert!(!surface.approval_expanded, "Ctrl+F again collapses it");

        // Expand, then resolve the card: the toggle resets so the next
        // pending card starts collapsed, not stuck expanded.
        surface.handle_key(ctrl(KeyCode::Char('f')), &mut app);
        assert!(surface.approval_expanded);
        let action = surface.handle_key(key(KeyCode::Char('y')), &mut app);
        assert!(
            matches!(action, SurfaceAction::Approve { .. }),
            "y approves the pending card"
        );
        assert!(
            !surface.approval_expanded,
            "resolving the card resets the expand toggle"
        );
    }

    #[test]
    fn idle_state_renders_the_wayland_banner() {
        // An empty session is the idle state (mockup surface 02). The
        // hybrid-branding decision makes it the home for the full WAYLAND
        // banner — the wordmark art, the tagline, and the `/` hint.
        let mut app = App::new();
        // B2 guard: set a model so the no-model banner doesn't override
        // the compositor area (this tests the normal idle render path).
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 100, 30);
        assert!(
            out.contains("the autonomous AI agent"),
            "banner tagline missing:\n{out}"
        );
        assert!(
            out.contains("type / for commands"),
            "banner hint missing:\n{out}"
        );
        // The composer prompt glyph is present in the idle state.
        assert!(out.contains('›'), "composer prompt missing:\n{out}");
    }

    #[test]
    fn idle_hero_shows_subtitle_and_example_prompts_d044() {
        // D044: the blank Workspace must answer "what now?" — a one-line
        // factual description of what Wayland is, plus 2-3 concrete starter
        // prompts. An empty session (no model-banner override) renders it.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        let mut surface = WorkspaceSurface::new();
        // Tall enough to fit the banner AND the hero copy slot.
        let out = render_to_string(&mut surface, &app, 100, 30);

        assert!(
            out.contains("A terminal AI agent that reads, writes, and runs code"),
            "idle hero subtitle missing:\n{out}"
        );
        assert!(
            out.contains("Try: explain this codebase"),
            "idle hero example prompt 1 missing:\n{out}"
        );
        // FIX-5: the boot screen advertises the paste-to-connect door.
        assert!(
            out.contains("/connect") && out.contains("paste an API key"),
            "idle hero must surface the /connect provider door:\n{out}"
        );
        assert!(
            out.contains("Try: /model"),
            "idle hero /model prompt missing:\n{out}"
        );
    }

    #[test]
    fn idle_hero_drops_to_banner_only_on_a_short_pane_d044() {
        // The hero copy slot is reserved ONLY when the pane has the rows to
        // spare; on a short pane the banner takes the whole area and the hero
        // copy is dropped rather than crammed (no panic, no clipped half-hero).
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        let mut surface = WorkspaceSurface::new();
        // body height ≈ 8 (composer takes 4): enough for the degraded banner
        // tagline, below the hero-copy reservation threshold.
        let out = render_to_string(&mut surface, &app, 100, 12);

        // The tagline (degraded banner) still renders…
        assert!(
            out.contains("the autonomous AI agent"),
            "banner tagline missing on a short pane:\n{out}"
        );
        // …but the hero copy is suppressed when there is no room for it.
        assert!(
            !out.contains("Try: explain this codebase"),
            "hero copy must be dropped on a short pane:\n{out}"
        );
    }

    #[test]
    fn mid_stream_state_shows_streaming_text_and_spinner() {
        // A full conversation fixture with the StreamEnd dropped leaves a
        // stream in flight — the agent-live state (mockup surface 03).
        let mut events = fixtures::full_conversation();
        events.pop(); // drop StreamEnd → still streaming
        let mut app = app_from_fixture(events);
        // B2 guard: set a model so the no-model banner doesn't override
        // the streaming composer area.
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        assert!(
            app.session.streaming_active,
            "fixture must leave a stream live"
        );

        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 100, 30);
        assert!(
            out.contains("Hello!") && out.contains("How can I help?"),
            "streamed text missing:\n{out}"
        );
        // v0.9.0 W3 D3: the composer renders the animated streaming-status
        // line (which includes `↑ N tokens`) plus the `interrupt` hint on
        // the row below. The substring `tokens` is unique to the new
        // widget; `interrupt` is unique to the hint row.
        assert!(
            out.contains("tokens") && out.contains("interrupt"),
            "streaming status + cancel affordance missing:\n{out}"
        );
    }

    #[test]
    fn completed_conversation_renders_the_assistant_turn() {
        let app = app_from_fixture(fixtures::full_conversation());
        assert!(!app.session.streaming_active);
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 100, 30);
        // v0.9.1 W1 A: the literal `"wayland"` role label is GONE — the
        // 2-space gutter is the only role signal. We assert the body
        // text lands (markdown-rendered, indented). The lowercase
        // `"wayland"` string used to mark each assistant turn header;
        // its absence is the regression guard for Part 3 of A's scope.
        assert!(
            out.contains("How can I help?"),
            "assistant turn text missing:\n{out}"
        );
        assert!(
            !out.contains("wayland"),
            "v0.9.1 W1 A removed the lowercase 'wayland' role label, but it leaked:\n{out}"
        );
    }

    /// The `tool_call_with_approval` fixture truncated to its first
    /// `n` events — `ProtocolEvent` is not `Clone`, so subset a fixture
    /// by consuming its `Vec` rather than cloning elements.
    fn tool_fixture_prefix(n: usize) -> Vec<wcore_protocol::events::ProtocolEvent> {
        fixtures::tool_call_with_approval()
            .into_iter()
            .take(n)
            .collect()
    }

    #[test]
    fn tool_call_running_renders_a_tool_card() {
        // The first two events (ToolRequest + ApprovalRequired) minus the
        // approval still register a running card from the request.
        let app = app_from_fixture(tool_fixture_prefix(1));
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 100, 30);
        assert!(out.contains("Bash"), "tool card name missing:\n{out}");
        assert!(
            out.contains("cargo test"),
            "tool card summary missing:\n{out}"
        );
    }

    #[test]
    fn awaiting_approval_renders_the_single_surface_dialog() {
        // ToolRequest + ApprovalRequired leaves the card AwaitingApproval.
        // v0.9.2 W2 (SPEC §0 #1, §1C): the single-surface permission
        // dialog renders inline — the routed component's title + body +
        // a key row carrying `approve` / `deny`. (The fixture tool routes
        // to FallbackComponent until W3/W4 add bespoke arms.) The legacy
        // 6-line "Approve this tool call?" card is gone.
        let app = app_from_fixture(tool_fixture_prefix(2));
        assert!(
            WorkspaceSurface::pending_approval(&app).is_some(),
            "fixture must leave a card awaiting approval"
        );
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 100, 30);
        // The single-surface dialog offers approve + deny affordances.
        assert!(
            out.contains("approve"),
            "approve affordance missing:\n{out}"
        );
        assert!(out.contains("deny"), "deny affordance missing:\n{out}");
        // Single-surface: the old bespoke `│`-bar card header is gone.
        assert!(
            !out.contains("Approve this tool call?"),
            "legacy 6-line approval card header must be gone:\n{out}"
        );
    }

    #[test]
    fn edit_tool_call_renders_the_single_surface_dialog() {
        // The edit fixture's ToolRequest carries old/new strings; force
        // the card into AwaitingApproval so the inline approval renders.
        //
        // v0.9.2 W2: the Edit tool routes through the single-surface
        // permission dialog. v0.9.2 SCAFFOLD registers the bespoke
        // FileEditComponent (currently a stub — title `Make this edit`,
        // empty body; the W3 component agent fills in the diff). So Edit no
        // longer degrades to FallbackComponent: assert the bespoke dialog
        // renders with an `approve` key row and no legacy header.
        let request = fixtures::edit_tool_call()
            .into_iter()
            .next()
            .expect("edit fixture must have a ToolRequest");
        let mut app = app_from_fixture(vec![request]);
        app.session.tool_cards[0].status = ToolCardStatus::AwaitingApproval;
        assert!(
            app.session.tool_cards[0].edit_preview.is_some(),
            "edit fixture must yield a DiffModel"
        );
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 110, 36);
        assert!(
            out.contains("Make this edit"),
            "bespoke FileEdit dialog title missing:\n{out}"
        );
        assert!(
            out.contains("approve"),
            "approve affordance missing:\n{out}"
        );
        assert!(
            !out.contains("Approve this tool call?"),
            "legacy 6-line approval card header must be gone:\n{out}"
        );
    }

    #[test]
    fn composer_accepts_typed_input() {
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        for c in "hi".chars() {
            let action = surface.handle_key(key(KeyCode::Char(c)), &mut app);
            assert!(matches!(action, SurfaceAction::None));
        }
        assert_eq!(surface.composer.value(), "hi");
    }

    #[test]
    fn enter_submits_the_composer_as_a_send_message() {
        let mut app = App::new();
        // B2 guard: set a model so the no-model guard doesn't intercept
        // the Enter key before it reaches the send-message path.
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        let mut surface = WorkspaceSurface::new();
        for c in "refactor auth".chars() {
            surface.handle_key(key(KeyCode::Char(c)), &mut app);
        }
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        match action {
            SurfaceAction::SendMessage(text) => assert_eq!(text, "refactor auth"),
            _ => panic!("expected SurfaceAction::SendMessage"),
        }
        // The composer is cleared after a submit.
        assert_eq!(surface.composer.value(), "");
    }

    #[test]
    fn enter_while_streaming_queues_the_message_instead_of_dropping_it() {
        // AUDIT-D D3: pressing `Enter` during a live turn must NOT fire
        // `SendMessage` (which `submit`'s `is_busy` gate would silently
        // drop). It emits `QueueMessage` so the router holds the text and
        // sends it when the turn ends.
        let mut events = fixtures::full_conversation();
        events.pop(); // drop StreamEnd → still streaming
        let mut app = app_from_fixture(events);
        // B2 guard: set a model so the no-model guard doesn't intercept.
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        assert!(
            app.session.streaming_active,
            "fixture must leave a stream live"
        );

        let mut surface = WorkspaceSurface::new();
        for c in "type ahead".chars() {
            surface.handle_key(key(KeyCode::Char(c)), &mut app);
        }
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        match action {
            SurfaceAction::QueueMessage(text) => assert_eq!(text, "type ahead"),
            other => panic!("expected QueueMessage while streaming, got {other:?}"),
        }
        // The composer is cleared — the text moved into the queue.
        assert_eq!(surface.composer.value(), "");
    }

    #[test]
    fn slash_command_still_routes_immediately_while_streaming() {
        // A `/…` line must route as a `Command` even mid-turn — `/cancel`
        // has to work while the stream is live. Only plain messages queue.
        use tui_input::Input;
        let mut events = fixtures::full_conversation();
        events.pop();
        let mut app = app_from_fixture(events);
        assert!(app.session.streaming_active);
        let mut surface = WorkspaceSurface::new();
        surface.composer = Input::new("/cancel".to_string());
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        match action {
            SurfaceAction::Command(cmd) => assert_eq!(cmd, "/cancel"),
            other => panic!("expected /cancel to route immediately, got {other:?}"),
        }
    }

    #[test]
    fn working_spinner_animates_with_the_frame_tick() {
        // AUDIT-D D8 + v0.9.0 W3 D3: the animated streaming-status symbol
        // must advance with the render tick so a live slow turn is
        // visually distinct from a hung one. Two renders at frame ticks
        // far enough apart must show different symbol glyphs.
        let mut events = fixtures::full_conversation();
        events.pop(); // still streaming → the streaming-status line renders
        let mut app = app_from_fixture(events);
        // B2 guard: set a model so the no-model banner doesn't replace the
        // streaming status area this test checks.
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        assert!(app.session.streaming_active);

        let mut surface = WorkspaceSurface::new();
        app.frame_tick = 0;
        let frame_a = render_to_string(&mut surface, &app, 100, 30);
        // Advance enough ticks to cross at least one symbol frame (the
        // W3 D3 widget uses 8 ticks per frame; advancing by 8 lands on
        // the next frame — a multiple of 32 (=8×4 frames) would wrap
        // back to the same one).
        app.frame_tick = 8;
        let frame_b = render_to_string(&mut surface, &app, 100, 30);

        // The status line still renders in both (`tokens` is unique to
        // the new widget, so it's a reliable substring marker)…
        assert!(frame_a.contains("tokens") && frame_b.contains("tokens"));
        // …but the symbol glyph differs — the animation advanced.
        let sym_a = first_status_symbol(&frame_a);
        let sym_b = first_status_symbol(&frame_b);
        assert_ne!(
            sym_a, sym_b,
            "the streaming-status symbol must animate across frame ticks (was frozen)"
        );
    }

    /// The first v0.9.0 W3 D3 streaming-status symbol glyph found in a
    /// rendered frame, used to assert the animated symbol advances.
    /// Returns `None` if no glyph is present.
    fn first_status_symbol(frame: &str) -> Option<char> {
        const SYMBOLS: [char; 4] = ['✻', '⋆', '✦', '✷'];
        frame.chars().find(|c| SYMBOLS.contains(c))
    }

    #[test]
    fn enter_on_an_empty_composer_does_nothing() {
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "empty submit must be inert"
        );
    }

    #[test]
    fn approval_key_y_emits_approve_once() {
        // v0.9.1 W1 A: `y` (lowercase) approves the next pending card
        // with `Once` scope. (Was: `a` → Once in v0.9.0; the lowercase
        // `a` now means "always for this tool", matching the inline
        // approval card hint row `[y] approve once   [a] always …`.)
        let mut app = app_from_fixture(tool_fixture_prefix(2));
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Char('y')), &mut app);
        match action {
            SurfaceAction::Approve {
                call_id,
                scope,
                answer,
            } => {
                assert_eq!(call_id, "call-1");
                assert_eq!(scope, wcore_protocol::commands::ApprovalScope::Once);
                assert!(answer.is_none(), "non-AskUser approve carries no answer");
            }
            _ => panic!("expected SurfaceAction::Approve with scope Once"),
        }
    }

    #[test]
    fn approval_key_a_lowercase_on_shell_enters_prefix_edit() {
        // v0.9.1 W1 A originally sent `Always` on `a` for every tool.
        // v0.9.2 W3 (SPEC §1C/§2 Bash) supersedes that for shell tools:
        // the fixture card is `Bash`, so `a` now opens the editable
        // always-allow prefix sub-mode instead of emitting bare `Always`
        // (which would auto-approve every shell command — the audit
        // BLOCKER). Non-shell `a` still emits `Always`
        // (`non_shell_a_still_emits_always_unchanged`).
        let mut app = app_from_fixture(tool_fixture_prefix(2));
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Char('a')), &mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "Bash `a` opens prefix-edit (no immediate action); got {action:?}"
        );
        let edit = surface
            .prefix_edit
            .as_ref()
            .expect("Bash `a` must arm the prefix-edit sub-mode");
        assert_eq!(edit.call_id, "call-1");
        assert_eq!(
            edit.buffer.value(),
            "cargo test ",
            "prefix prefilled from infer_shell_prefix(\"cargo test\")"
        );
    }

    #[test]
    fn approval_esc_emits_deny() {
        let mut app = app_from_fixture(tool_fixture_prefix(2));
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        match action {
            SurfaceAction::Deny { call_id, reason } => {
                assert_eq!(call_id, "call-1");
                assert!(!reason.is_empty(), "deny reason must be set");
            }
            _ => panic!("expected SurfaceAction::Deny"),
        }
    }

    #[test]
    fn approval_key_n_emits_deny() {
        // v0.9.1 W1 A: lowercase `n` denies the next pending card.
        let mut app = app_from_fixture(tool_fixture_prefix(2));
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Char('n')), &mut app);
        match action {
            SurfaceAction::Deny { call_id, reason } => {
                assert_eq!(call_id, "call-1");
                assert!(!reason.is_empty(), "deny reason must carry a reason");
            }
            _ => panic!("expected SurfaceAction::Deny"),
        }
    }

    #[test]
    fn approval_capital_a_approves_all_and_arms_batch_drain() {
        // v0.9.1 W1 A: capital `A` approves the NEXT card immediately
        // and arms the batch-drain so subsequent cards are approved one
        // per router tick (via `pending_batch_decision`). The frozen
        // SurfaceAction contract is one action per key event, so the
        // surface holds the batch state and the router drains it.
        let mut app = app_from_fixture(tool_fixture_prefix(2));
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Char('A')), &mut app);
        match action {
            SurfaceAction::Approve {
                call_id,
                scope,
                answer,
            } => {
                assert_eq!(call_id, "call-1");
                assert_eq!(scope, wcore_protocol::commands::ApprovalScope::Once);
                assert!(answer.is_none(), "capital-A approve carries no answer");
            }
            _ => panic!("expected SurfaceAction::Approve from capital A"),
        }
        assert!(
            surface.approval_batch.is_some(),
            "capital A must arm the batch-approval drain"
        );
    }

    #[test]
    fn approval_capital_n_denies_all_and_arms_batch_drain() {
        // v0.9.1 W1 A: capital `N` denies the NEXT card and arms the
        // batch-deny drain.
        let mut app = app_from_fixture(tool_fixture_prefix(2));
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Char('N')), &mut app);
        match action {
            SurfaceAction::Deny { call_id, .. } => {
                assert_eq!(call_id, "call-1");
            }
            _ => panic!("expected SurfaceAction::Deny from capital N"),
        }
        assert!(
            surface.approval_batch.is_some(),
            "capital N must arm the batch-deny drain"
        );
    }

    /// Build an `App` with one head `AwaitingApproval` card for `tool` with
    /// the given `command` as its summary (the field the Bash component +
    /// the prefix-edit sub-mode read the command from). Reuses the edit
    /// fixture's ToolRequest to seed a real card, then overwrites it.
    fn app_with_shell_approval(tool: &str, command: &str) -> App {
        let request = fixtures::edit_tool_call()
            .into_iter()
            .next()
            .expect("edit fixture must have a ToolRequest");
        let mut app = app_from_fixture(vec![request]);
        let mut card = app.session.tool_cards[0].clone();
        card.call_id = "shell-1".into();
        card.tool_name = tool.into();
        card.summary = command.into();
        card.status = ToolCardStatus::AwaitingApproval;
        app.session.tool_cards.clear();
        app.session.tool_cards.push(card);
        app
    }

    #[test]
    fn bash_a_enters_prefix_edit_mode_with_inferred_prefix() {
        // v0.9.2 W3 (SPEC §2 Bash): `a` on a Bash card does NOT send a
        // SurfaceAction immediately — it opens the prefix-edit sub-mode
        // prefilled with `infer_shell_prefix(command)`. The audit BLOCKER
        // is that `a` must NEVER reach bare `Always` for a shell tool.
        let mut app = app_with_shell_approval("Bash", "cargo test --lib");
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Char('a')), &mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "Bash `a` must consume the key into prefix-edit, not emit an action; got {action:?}"
        );
        let edit = surface
            .prefix_edit
            .as_ref()
            .expect("Bash `a` must arm the prefix-edit sub-mode");
        assert_eq!(edit.call_id, "shell-1");
        assert_eq!(
            edit.buffer.value(),
            "cargo test ",
            "prefix must be prefilled from infer_shell_prefix(command)"
        );
    }

    #[test]
    fn bash_prefix_edit_commit_emits_always_prefix() {
        // v0.9.2 W3 / §1D: committing the prefix-edit (Enter) sends the W0
        // prefix-scoped variant `AlwaysPrefix { prefix }` for the card —
        // never bare `Always`.
        use wcore_protocol::commands::ApprovalScope;
        let mut app = app_with_shell_approval("Bash", "cargo build --release");
        let mut surface = WorkspaceSurface::new();
        // Enter edit mode (prefix → "cargo build ").
        surface.handle_key(key(KeyCode::Char('a')), &mut app);
        // Commit.
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        match action {
            SurfaceAction::Approve {
                call_id,
                scope,
                answer,
            } => {
                assert_eq!(call_id, "shell-1");
                assert_eq!(
                    scope,
                    ApprovalScope::AlwaysPrefix {
                        prefix: "cargo build ".into()
                    }
                );
                assert!(answer.is_none(), "prefix-edit approve carries no answer");
            }
            _ => panic!("expected Approve with AlwaysPrefix; got {action:?}"),
        }
        assert!(
            surface.prefix_edit.is_none(),
            "committing the prefix must clear the sub-mode"
        );
    }

    #[test]
    fn bash_prefix_edit_esc_backs_out_without_action() {
        // v0.9.2 W3: `Esc` while editing the prefix backs out to the card
        // with no engine action — the user can still y/n/a fresh.
        let mut app = app_with_shell_approval("Bash", "npm run build");
        let mut surface = WorkspaceSurface::new();
        surface.handle_key(key(KeyCode::Char('a')), &mut app);
        assert!(surface.prefix_edit.is_some());
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "Esc out of prefix-edit must be inert; got {action:?}"
        );
        assert!(
            surface.prefix_edit.is_none(),
            "Esc must clear the prefix-edit sub-mode"
        );
    }

    #[test]
    fn non_shell_a_still_emits_always_unchanged() {
        // v0.9.2 W3 regression guard: `a` on a non-shell tool keeps the
        // pre-W3 behaviour — bare `ApprovalScope::Always`, no prefix-edit.
        use wcore_protocol::commands::ApprovalScope;
        let mut app = app_with_shell_approval("Edit", "src/main.rs");
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Char('a')), &mut app);
        match action {
            SurfaceAction::Approve {
                call_id,
                scope,
                answer,
            } => {
                assert_eq!(call_id, "shell-1");
                assert_eq!(scope, ApprovalScope::Always);
                assert!(
                    answer.is_none(),
                    "non-shell Always approve carries no answer"
                );
            }
            _ => panic!("expected Approve with Always for a non-shell tool; got {action:?}"),
        }
        assert!(
            surface.prefix_edit.is_none(),
            "a non-shell tool must NOT open the prefix-edit sub-mode"
        );
    }

    #[test]
    fn batch_a_keypress_then_two_ticks_processes_three_cards() {
        // v0.9.1 W2 cycle-2 HIGH 1: simulate a 3-card pending-approval
        // batch. Pressing capital `A` approves the first card AND arms
        // the batch-drain; each subsequent `tick()` returns the next
        // Approve action until the queue is empty. Exactly 3 Approve
        // actions are emitted across (1 keypress + 2 ticks).
        let request = fixtures::edit_tool_call()
            .into_iter()
            .next()
            .expect("edit fixture must have a ToolRequest");
        let mut app = app_from_fixture(vec![request]);
        // Force-build a 3-card pending state: clone the first card twice
        // so all three are AwaitingApproval with distinct call_ids.
        let template = app.session.tool_cards[0].clone();
        app.session.tool_cards.clear();
        for i in 1..=3 {
            let mut card = template.clone();
            card.call_id = format!("batch-{i}");
            card.status = ToolCardStatus::AwaitingApproval;
            app.session.tool_cards.push(card);
        }

        let mut surface = WorkspaceSurface::new();

        // Keypress: capital `A` → approve the first card + arm the batch.
        let action = surface.handle_key(key(KeyCode::Char('A')), &mut app);
        let first_id = match action {
            SurfaceAction::Approve { call_id, .. } => call_id,
            _ => panic!("expected Approve from capital A; got {action:?}"),
        };
        assert_eq!(first_id, "batch-1");
        assert!(
            surface.approval_batch.is_some(),
            "capital A must arm the batch-drain"
        );
        // The router would normally fire the engine approval here, which
        // flips the card off AwaitingApproval. The test fakes that
        // transition explicitly so the drain finds the next pending card.
        app.session.tool_cards[0].status = ToolCardStatus::Running;

        // Tick #1: the drain fires Approve for the second card.
        let action = surface.tick(&mut app);
        let second_id = match action {
            SurfaceAction::Approve { call_id, .. } => call_id,
            _ => panic!("expected Approve from tick #1; got {action:?}"),
        };
        assert_eq!(second_id, "batch-2");
        app.session.tool_cards[1].status = ToolCardStatus::Running;

        // Tick #2: drain fires Approve for the third card.
        let action = surface.tick(&mut app);
        let third_id = match action {
            SurfaceAction::Approve { call_id, .. } => call_id,
            _ => panic!("expected Approve from tick #2; got {action:?}"),
        };
        assert_eq!(third_id, "batch-3");
        app.session.tool_cards[2].status = ToolCardStatus::Running;

        // Tick #3: queue drained — batch state cleared, no more actions.
        let action = surface.tick(&mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "drained queue must emit None; got {action:?}"
        );
        assert!(
            surface.approval_batch.is_none(),
            "drained queue must clear the armed batch state"
        );
    }

    #[test]
    fn approval_enter_emits_approve_once() {
        // v0.9.1 W1 A: Enter is the unmodified "approve once" affordance
        // (matches the inline-card hint row). Was: "commit highlighted
        // option" from the 3-option modal.
        let mut app = app_from_fixture(tool_fixture_prefix(2));
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        match action {
            SurfaceAction::Approve {
                call_id,
                scope,
                answer,
            } => {
                assert_eq!(call_id, "call-1");
                assert_eq!(scope, wcore_protocol::commands::ApprovalScope::Once);
                assert!(answer.is_none(), "plain-Enter approve carries no answer");
            }
            _ => panic!("expected SurfaceAction::Approve from Enter"),
        }
    }

    #[test]
    fn approval_prompt_swallows_typed_input() {
        // While a card awaits approval, typed chars must not leak into
        // the composer — the decision owns the keyboard.
        let mut app = app_from_fixture(tool_fixture_prefix(2));
        let mut surface = WorkspaceSurface::new();
        // 'z' is not an approval hot-key — it is consumed, not typed.
        surface.handle_key(key(KeyCode::Char('z')), &mut app);
        assert_eq!(
            surface.composer.value(),
            "",
            "composer must not receive input during approval"
        );
    }

    #[test]
    fn esc_while_streaming_emits_the_cancel_command() {
        // `Esc` mid-stream is the cancel affordance. Wave 2 wires it to
        // the engine: it emits the `/cancel` command, which the router
        // routes to `TuiEngine::cancel`.
        let mut events = fixtures::full_conversation();
        events.pop(); // drop StreamEnd → still streaming
        let mut app = app_from_fixture(events);
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        match action {
            SurfaceAction::Command(cmd) => assert_eq!(cmd, "/cancel"),
            other => panic!("expected the /cancel command, got {other:?}"),
        }
    }

    #[test]
    fn slash_prefixed_submit_emits_a_command() {
        // A composer line starting with `/` submits as a slash command,
        // not a chat message. `/` on an *empty* composer opens the
        // palette, so a typed slash line is built by first entering a
        // leading char then editing — here the buffer is set directly to
        // exercise the submit branch.
        use tui_input::Input;
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        surface.composer = Input::new("/help".to_string());
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        match action {
            SurfaceAction::Command(cmd) => assert_eq!(cmd, "/help"),
            other => panic!("expected SurfaceAction::Command, got {other:?}"),
        }
    }

    #[test]
    fn slash_on_empty_composer_opens_the_palette() {
        // Typing `/` into an empty composer opens the command palette
        // overlay (the discoverable home for slash commands).
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Char('/')), &mut app);
        assert!(
            matches!(action, SurfaceAction::OpenOverlay(SurfaceId::Palette)),
            "`/` on an empty composer must open the palette"
        );
    }

    #[test]
    fn at_token_opens_the_completion_popup() {
        // Typing an `@…` token whose prefix matches a static keyword
        // (`@di` → `@diff`) opens the completion popup.
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        for c in "@di".chars() {
            surface.handle_key(key(KeyCode::Char(c)), &mut app);
        }
        assert!(
            surface.at_completion.is_some(),
            "an `@` token must open the completion popup"
        );
    }

    #[test]
    fn at_completion_tab_accepts_the_highlighted_candidate() {
        // `Tab` while the `@`-completion popup is open replaces the
        // partial `@…` word with the chosen candidate.
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        for c in "@di".chars() {
            surface.handle_key(key(KeyCode::Char(c)), &mut app);
        }
        surface.handle_key(key(KeyCode::Tab), &mut app);
        // `@diff` is the static-keyword completion for `@di`.
        assert_eq!(surface.composer.value(), "@diff");
        assert!(surface.at_completion.is_none(), "popup closes after accept");
    }

    #[test]
    fn renders_in_a_narrow_terminal_without_panicking() {
        // A terminal too narrow for the rail must still render the
        // transcript + composer without panicking.
        let app = app_from_fixture(fixtures::full_conversation());
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 24, 12);
        assert!(!out.is_empty());
    }

    #[test]
    fn ctrl_b_toggles_the_rail_visibility() {
        // `Ctrl+B` flips `App::rail_visible`; the chord is consumed
        // without a routing effect and never leaks into the composer.
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        assert!(app.rail_visible, "rail is shown by default");

        let action = surface.handle_key(ctrl(KeyCode::Char('b')), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        assert!(!app.rail_visible, "Ctrl+B hides the rail");

        surface.handle_key(ctrl(KeyCode::Char('b')), &mut app);
        assert!(app.rail_visible, "a second Ctrl+B shows it again");
        // The chord did not type a literal `b` into the composer.
        assert_eq!(surface.composer.value(), "");
    }

    #[test]
    fn ctrl_e_toggles_compact_globally() {
        // v0.9.0 W3 D1: `Ctrl+E` flips
        // `App::session.compact_tool_output` for every tool card on
        // screen at once. The chord is consumed (returns None) and
        // never leaks into the composer.
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        assert!(
            app.session.compact_tool_output,
            "compact tool output is the default"
        );

        let action = surface.handle_key(ctrl(KeyCode::Char('e')), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        assert!(
            !app.session.compact_tool_output,
            "Ctrl+E expands tool cards to full mode"
        );

        surface.handle_key(ctrl(KeyCode::Char('e')), &mut app);
        assert!(
            app.session.compact_tool_output,
            "a second Ctrl+E collapses tool cards back to compact"
        );
        // The chord did not type a literal `e` into the composer.
        assert_eq!(surface.composer.value(), "");
    }

    #[test]
    fn rail_renders_when_visible_and_vanishes_when_hidden() {
        // v0.9.1.2 W8: the Tools panel was removed; Activity is the
        // sole rail tenant. Forcing the fixture's tool card back to
        // `Running` keeps it in the Activity feed so the rail has
        // something to render — then `Ctrl+B` hides everything.
        let mut app = app_from_fixture(fixtures::tool_call_with_approval());
        if let Some(card) = app.session.tool_cards.first_mut() {
            card.status = ToolCardStatus::Running;
        }
        let mut surface = WorkspaceSurface::new();

        let shown = render_to_string(&mut surface, &app, 100, 30);
        assert!(
            shown.contains("Activity"),
            "Activity panel missing when visible:\n{shown}"
        );
        // W8 regression: the removed "Tools · N call(s)" rail title
        // must never reappear.
        assert!(
            !shown.contains("Tools ·"),
            "Tools rail panel must not render (W8):\n{shown}"
        );

        app.rail_visible = false;
        let hidden = render_to_string(&mut surface, &app, 100, 30);
        assert!(
            !hidden.contains("Tools ")
                && !hidden.contains("Path map")
                && !hidden.contains("Activity"),
            "rail still rendered when hidden:\n{hidden}"
        );
    }

    #[test]
    fn rail_fully_collapses_on_a_cold_session() {
        // Sean's UX callout 2026-05-27 ("if there's no fucking activity
        // then hide that shit") — on a session with no path-map data,
        // no tool calls and no system notices the entire rail collapses.
        // The user sees only the transcript and composer, no "no
        // activity yet" / "no files touched yet" / "no tools used yet"
        // placeholders, and no empty panel chrome.
        let app = app_from_fixture(fixtures::full_conversation());
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 110, 30);
        assert!(
            !out.contains("no activity yet"),
            "empty-state placeholder still rendered:\n{out}"
        );
        assert!(
            !out.contains("no files touched yet"),
            "empty path-map placeholder still rendered:\n{out}"
        );
        assert!(
            !out.contains("no tools used yet"),
            "empty tools placeholder still rendered:\n{out}"
        );
        assert!(
            !out.contains("Path map") && !out.contains("Tools "),
            "rail did not collapse on a cold session:\n{out}"
        );
        assert!(
            !out.contains("Activity"),
            "empty Activity panel still rendered:\n{out}"
        );
    }

    #[test]
    fn rail_auto_hides_on_a_narrow_terminal_without_touching_the_preference() {
        // Below the responsive threshold the rail auto-hides so the
        // transcript gets the full width — but `App::rail_visible` (the
        // user's `Ctrl+B` preference) is NOT overwritten, so widening the
        // terminal restores the rail with no key press.
        //
        // v0.9.1.2 W8: Activity is the only rail panel; force the
        // fixture's tool card back to `Running` so the feed has
        // something to render.
        let mut app = app_from_fixture(fixtures::tool_call_with_approval());
        if let Some(card) = app.session.tool_cards.first_mut() {
            card.status = ToolCardStatus::Running;
        }
        let mut surface = WorkspaceSurface::new();
        assert!(app.rail_visible, "preference is on by default");

        // Wide enough — the rail renders (Activity panel; Path map and
        // the W8-deleted Tools panel are both gone).
        let wide = render_to_string(&mut surface, &app, 110, 30);
        assert!(wide.contains("Activity"), "rail missing when wide:\n{wide}");

        // Narrow — the rail is gone, but the preference flag is untouched.
        let narrow = render_to_string(&mut surface, &app, 80, 30);
        assert!(
            !narrow.contains("Activity"),
            "rail not auto-hidden on a narrow terminal:\n{narrow}"
        );
        assert!(
            app.rail_visible,
            "responsive auto-hide must not overwrite the user preference"
        );
        // The transcript still renders in the reclaimed width — the tool
        // card's tool name is the load-bearing fingerprint here.
        assert!(
            narrow.contains("Bash") || narrow.contains("bash"),
            "transcript missing on a narrow terminal:\n{narrow}"
        );
    }

    #[test]
    fn rail_effective_visibility_combines_preference_and_width() {
        let mut app = App::new();
        // Preference on + wide → visible.
        assert!(rail_effectively_visible(&app, 120));
        // Preference on + narrow → hidden (responsive).
        assert!(!rail_effectively_visible(&app, 80));
        // Preference off → hidden regardless of width.
        app.rail_visible = false;
        assert!(!rail_effectively_visible(&app, 120));
    }

    /// B2 regression guard: when `app.config.model` is empty, Enter must
    /// not proceed to `SendMessage` — it must return `None` and set the
    /// no-model banner flag so the user sees a human-readable error instead
    /// of an opaque provider "builder error".
    #[test]
    fn enter_with_no_model_blocks_submit_and_sets_banner() {
        let mut app = App::new(); // config.model == "" by default
        assert!(app.config.model.is_empty(), "precondition: no model set");
        let mut surface = WorkspaceSurface::new();
        for c in "hello".chars() {
            surface.handle_key(key(KeyCode::Char(c)), &mut app);
        }
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "Enter with no model must return None, not SendMessage"
        );
        assert!(
            surface.no_model_banner,
            "no_model_banner must be set after blocked submit"
        );
        // The composer text must be preserved (not cleared on a blocked submit).
        assert_eq!(
            surface.composer.value(),
            "hello",
            "composer must not be cleared"
        );
    }

    /// B2 render guard: when no model is configured, the composer area shows
    /// the no-model error panel, not the normal prompt.
    #[test]
    fn no_model_banner_renders_when_model_is_empty() {
        let app = App::new(); // config.model == ""
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 100, 30);
        assert!(
            out.contains("No model configured"),
            "no-model banner missing from render:\n{out}"
        );
    }

    /// D002 (P0 onboarding dead-end): a catalog provider chosen at onboarding
    /// writes NO model, so the first prompt hits the no-model state. The banner
    /// must offer an IN-APP `/model` recovery (a one-keystroke slash command),
    /// NOT a quit-and-hand-edit-config.toml dead-end. We assert the RENDERED
    /// banner names `/model` AND that submitting `/model` from the no-model
    /// state is actually reachable (the no-model block lets slash commands
    /// through, returning `Command("/model")` instead of swallowing the submit).
    #[test]
    fn no_model_banner_offers_in_app_model_recovery() {
        use tui_input::Input;
        let mut app = App::new(); // config.model == ""
        app.config.provider = "novita-ai".to_string();
        let mut surface = WorkspaceSurface::new();

        // 1. RENDERED affordance: the banner must advertise in-app `/model`,
        //    not just a quit-and-hand-edit-config.toml dead-end.
        let out = render_to_string(&mut surface, &app, 100, 30);
        assert!(
            out.contains("/model"),
            "no-model banner must offer an in-app /model recovery affordance, \
             not only a quit/hand-edit dead-end:\n{out}"
        );

        // 2. REACHABLE — submitting `/model` from the no-model state routes the
        //    command (the no-model guard only blocks NON-slash submits, so the
        //    in-app recovery actually works). `/` on an empty composer opens
        //    the palette, so the typed slash line is set directly to exercise
        //    the submit branch (mirrors `slash_prefixed_submit_emits_a_command`).
        surface.composer = Input::new("/model".to_string());
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        match action {
            SurfaceAction::Command(cmd) => assert_eq!(
                cmd, "/model",
                "submitting /model from the no-model state must route the command"
            ),
            other => panic!("expected SurfaceAction::Command(\"/model\"), got {other:?}"),
        }

        // 3. DISCOVERABLE — `/` on the empty composer opens the palette (the
        //    discoverable home for /model), also reachable from this state.
        let mut surface2 = WorkspaceSurface::new();
        let open = surface2.handle_key(key(KeyCode::Char('/')), &mut app);
        assert!(
            matches!(open, SurfaceAction::OpenOverlay(SurfaceId::Palette)),
            "the command palette must be reachable from the no-model state"
        );
    }

    fn composer_hint_reflects_the_rail_toggle_state() {
        // The composer hint advertises `Ctrl+B` and tracks whether the
        // toggle will show or hide the rail.
        let mut app = App::new();
        // B2 guard: set a model so the no-model banner doesn't replace
        // the normal composer hint row this test checks.
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        let mut surface = WorkspaceSurface::new();

        let shown = render_to_string(&mut surface, &app, 100, 30);
        assert!(
            shown.contains("hide rail"),
            "hide-rail hint missing:\n{shown}"
        );

        app.rail_visible = false;
        let hidden = render_to_string(&mut surface, &app, 100, 30);
        assert!(
            hidden.contains("show rail"),
            "show-rail hint missing:\n{hidden}"
        );
    }

    #[test]
    fn status_hint_shows_select_mode_when_capture_off_2026() {
        // 2026-05-31: capture defaults ON (wheel scrolls the transcript), so
        // the DEFAULT hint advertises F4 copy/select. Toggling capture OFF
        // (F4) pivots the hint to the "Select mode" string that tells the
        // user how to get wheel scroll back. Esc is NOT a capture binding
        // anymore (it would hijack normal Esc), so the hint never mentions it.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        let mut surface = WorkspaceSurface::new();

        // Default state: capture ON, hint advertises the F4 copy/select path.
        assert!(app.mouse_capture_enabled, "default capture is ON");
        let on = render_to_string(&mut surface, &app, 120, 30);
        assert!(
            on.contains("F4 copy/select"),
            "default hint must advertise F4 copy/select when capture is on:\n{on}"
        );

        // Toggle capture OFF — the hint switches to "Select mode" and tells
        // the user F4 re-enables wheel scroll. It must NOT mention Esc.
        app.mouse_capture_enabled = false;
        let off = render_to_string(&mut surface, &app, 120, 30);
        assert!(
            off.contains("Select mode"),
            "select-mode hint missing when capture is off:\n{off}"
        );
        assert!(
            off.contains("F4 for wheel scroll"),
            "select-mode hint must advertise F4 to re-enable wheel scroll:\n{off}"
        );
        assert!(
            !off.contains("Esc"),
            "the hint must NOT advertise Esc as a capture binding (removed):\n{off}"
        );
    }

    // ── v0.9.1.2 F14 — approval discoverability via status hint + rail ──

    /// Push an `AwaitingApproval` card onto `app.session` for the given
    /// tool name and `call_id`. Used by F14 tests to seed the awaiting
    /// state without driving the full protocol-event sequence.
    fn push_awaiting_card(app: &mut App, call_id: &str, tool: &str) {
        app.session.tool_cards.push(ToolCardModel {
            call_id: call_id.into(),
            tool_name: tool.into(),
            summary: "test".into(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: None,
            input_pretty: String::new(),
            approval_reason: "test".into(),
            plan_body: None,
        });
    }

    #[test]
    fn tool_card_formatter_body_memo_caches_and_matches_uncached_p3() {
        // P3 perf memo: a completed card's formatter body must be served from
        // the cache on repeat frames (no re-parse + re-format) AND be identical
        // to the uncached render (correctness — the memo must never change what
        // the user sees).
        let theme = Theme::hearth();
        let card = ToolCardModel {
            call_id: "c-memo".into(),
            tool_name: "Bash".into(),
            summary: "ls".into(),
            status: ToolCardStatus::Ok,
            // A real JSON output the formatter parses each frame without the memo.
            output: Some(r#"{"stdout":"file-a.txt\nfile-b.txt","exit_code":0}"#.into()),
            edit_preview: None,
            input_pretty: String::new(),
            approval_reason: String::new(),
            plan_body: None,
        };

        tool_card_cache_clear();
        // The uncached render, for the correctness comparison.
        let mut uncached = Vec::new();
        format_tool_card_formatter_body(&mut uncached, &card, &theme, false);

        // First cached render populates the memo; a repeat HITS it (no re-insert).
        let mut first = Vec::new();
        push_tool_card_formatter_body(&mut first, &card, &theme, false);
        assert_eq!(
            tool_card_cache_len(),
            1,
            "first render must populate the memo"
        );
        let mut second = Vec::new();
        push_tool_card_formatter_body(&mut second, &card, &theme, false);
        assert_eq!(
            tool_card_cache_len(),
            1,
            "a repeat render must HIT the memo, not re-insert"
        );

        // Correctness: the memoized body equals the uncached render, byte-for-byte.
        assert_eq!(
            format!("{first:?}"),
            format!("{uncached:?}"),
            "the memo changed the rendered body"
        );
        assert_eq!(format!("{first:?}"), format!("{second:?}"));

        // A distinct render input (the compact flag) is a different key → a miss.
        let mut compact = Vec::new();
        push_tool_card_formatter_body(&mut compact, &card, &theme, true);
        assert_eq!(
            tool_card_cache_len(),
            2,
            "a different render key must miss and insert"
        );
    }

    #[test]
    fn status_hint_priority_approval_beats_mouse_v0912() {
        // v0.9.1.2 F14: when ANY card is awaiting approval, the hint
        // must show `[y]/[a]/[n]/[esc]` — even when the mouse hint would
        // otherwise fire. The approval triplet wins because the user has a
        // decision to make and the prior hint gave them no instructions.
        // 2026-05-31: the mouse special hint now fires in the capture-OFF
        // (select) state, so drop capture to exercise the priority path.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        assert!(app.mouse_capture_enabled, "default capture is ON");
        app.mouse_capture_enabled = false; // select mode shows the mouse hint
        push_awaiting_card(&mut app, "c-1", "Write");

        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 120, 30);

        assert!(
            out.contains("⊘ press [y] approve"),
            "approval hint missing — got:\n{out}"
        );
        assert!(out.contains("[n] deny"), "deny key missing — got:\n{out}");
        assert!(
            !out.contains("Select mode"),
            "mouse-capture hint must NOT win when approval is pending — got:\n{out}"
        );
    }

    // ── v0.9.2 W2 — single-surface inline approval (strip DELETED) ────────

    #[test]
    fn single_approval_card_renders_inline_when_pending_v092() {
        // v0.9.2 W2 (SPEC §0 #1): the single approval surface is the
        // inline transcript card — there is no separate sticky strip. The
        // dialog routes the tool (here → the bespoke FileWriteComponent,
        // SCAFFOLD-registered with title `Write a new file`) and offers
        // `approve` / `deny` affordances. Render via the card's ToolCard
        // turn element so we exercise the real inline path, not the
        // status-bar hint.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![crate::tui::turn_element::TurnElement::ToolCard(
                "c-1".into(),
            )],
        });
        push_awaiting_card(&mut app, "c-1", "Write");

        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 120, 30);

        assert!(
            out.contains("Write file"),
            "inline single-surface dialog title missing:\n{out}"
        );
        assert!(
            out.contains("approve"),
            "inline dialog approve affordance missing:\n{out}"
        );
        assert!(
            out.contains("deny"),
            "inline dialog deny affordance missing:\n{out}"
        );
        // The deleted v0.9.1.2 strip carried this exact label — must be gone.
        assert!(
            !out.contains("⊘ Approve"),
            "the deleted sticky strip must not render:\n{out}"
        );
    }

    #[test]
    fn no_approval_surface_when_no_pending_v092() {
        // v0.9.2 W2: zero AwaitingApproval cards → no approval card and no
        // (deleted) strip anywhere in the output.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();

        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 120, 30);

        assert!(
            !out.contains("⊘ Approve"),
            "no approval surface when nothing is pending:\n{out}"
        );
    }

    #[test]
    fn head_card_carries_more_pending_tail_v092() {
        // v0.9.2 W2 (SPEC §1C): one-card queue. Only the HEAD pending card
        // renders; its title carries the `(+N more pending)` tail so the
        // queue depth is visible. (The `(+N more pending)` text moved from
        // the deleted strip onto the head card's header.)
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        // Cards render inline only via their turn's ToolCard elements.
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![
                crate::tui::turn_element::TurnElement::ToolCard("c-1".into()),
                crate::tui::turn_element::TurnElement::ToolCard("c-2".into()),
                crate::tui::turn_element::TurnElement::ToolCard("c-3".into()),
            ],
        });
        push_awaiting_card(&mut app, "c-1", "Write");
        push_awaiting_card(&mut app, "c-2", "Edit");
        push_awaiting_card(&mut app, "c-3", "Bash");

        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 140, 30);

        assert!(
            out.contains("+2 more pending"),
            "head card must show `+2 more pending` tail for 3 cards:\n{out}"
        );
    }

    #[test]
    fn only_head_card_renders_dialog_v092() {
        // v0.9.2 W2 (SPEC §1C): one-card queue. With two simultaneously-
        // pending cards, ONLY the head renders its dialog. The head is c-1
        // (Write → the bespoke FileWriteComponent, SCAFFOLD title `Write a
        // new file`); the second card (c-2 Edit → FileEditComponent, title
        // `Make this edit`) must NOT render its own dialog.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        // Both cards must sit in the same assistant turn's element flow,
        // else `push_turn` never reaches the ToolCard arm. Build the turn.
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![
                crate::tui::turn_element::TurnElement::ToolCard("c-1".into()),
                crate::tui::turn_element::TurnElement::ToolCard("c-2".into()),
            ],
        });
        push_awaiting_card(&mut app, "c-1", "Write");
        push_awaiting_card(&mut app, "c-2", "Edit");

        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 120, 40);

        assert!(
            out.contains("Write file"),
            "head card dialog must render:\n{out}"
        );
        assert!(
            !out.contains("Make this edit"),
            "second pending card must NOT render its own dialog (one-card queue):\n{out}"
        );
        assert!(
            out.contains("+1 more pending"),
            "head card must show `+1 more pending` for the queued 2nd card:\n{out}"
        );
    }

    #[test]
    fn right_rail_pending_pill_v0912() {
        // v0.9.1.2 F14: the activity rail must surface a warn-yellow
        // `⊘ Pending(N)` pill so the user can't miss the pending
        // decision even on a fast-scrolling transcript. The pill must
        // be the FIRST row of the rail (it appears before any rolling
        // activity line).
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        // Two awaiting cards → the pill should read "Pending(2)".
        push_awaiting_card(&mut app, "c-a", "Write");
        push_awaiting_card(&mut app, "c-b", "Edit");

        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 120, 30);

        // Match `⊘ Pending(N)` with regex-light contains.
        assert!(
            out.contains("⊘ Pending(2)"),
            "rail must surface Pending(N) pill — got:\n{out}"
        );
        // The pill must appear before the body of the rail — assert the
        // first occurrence is in the upper half of the rendered output.
        let pending_idx = out
            .find("⊘ Pending")
            .expect("pill present in render output");
        let length_half = out.len() / 2;
        assert!(
            pending_idx < length_half,
            "pill must be in upper half of rail (idx={pending_idx}, half={length_half})"
        );
    }

    // ── D2/v0.9.0 scrollback + sticky-at-bottom + scrollbar ────────────────

    /// Build a `MouseEvent` of the given `kind` at (0, 0) with no modifiers.
    fn mouse(kind: ratatui::crossterm::event::MouseEventKind) -> MouseEvent {
        MouseEvent {
            kind,
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        }
    }

    /// Seed an `App` with a large transcript by pushing many `User`
    /// turns directly onto `App::session.turns`. The protocol bridge
    /// does not own user turns (the Router writes them on submit), so
    /// direct push is the correct test seam.
    fn app_with_long_transcript(turns: usize) -> App {
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        for i in 0..turns {
            app.session.turns.push(crate::tui::app::TurnView {
                role: TurnRole::User,
                elements: vec![crate::tui::turn_element::TurnElement::Markdown(format!(
                    "user message {i}"
                ))],
            });
        }
        app
    }

    #[test]
    fn mouse_scroll_up_sets_user_has_scrolled_up_flag() {
        // D2: a single wheel tick up off the bottom must mark the surface
        // as sticky-scrolled — so a new turn that arrives next render does
        // NOT auto-scroll over the user's reading position.
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        // Render once so `last_text_area_height` and `last_total_lines`
        // are observed; without this the scroll bump still works but the
        // flag latches on any non-zero scroll, which is the contract.
        let _ = render_to_string(&mut surface, &app, 100, 30);
        assert!(!surface.user_has_scrolled_up, "default must be false");

        surface.handle_mouse(
            mouse(ratatui::crossterm::event::MouseEventKind::ScrollUp),
            &mut app,
        );
        assert!(
            surface.user_has_scrolled_up,
            "ScrollUp must flip the sticky flag"
        );
        assert!(
            surface.transcript_scroll > 0,
            "ScrollUp must advance transcript_scroll"
        );
    }

    #[test]
    fn mouse_scroll_up_advances_transcript_scroll_by_three() {
        // D2: each wheel tick scrolls by 3 lines (browser convention).
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        let before = surface.transcript_scroll;
        surface.handle_mouse(
            mouse(ratatui::crossterm::event::MouseEventKind::ScrollUp),
            &mut app,
        );
        assert_eq!(
            surface.transcript_scroll,
            before + 3,
            "one wheel tick = 3 lines"
        );
    }

    #[test]
    fn scroll_to_bottom_clears_flag_and_resumes_autoscroll() {
        // D2: scrolling back all the way to the bottom must clear the
        // sticky flag — incoming turns autoscroll again.
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);

        surface.handle_mouse(
            mouse(ratatui::crossterm::event::MouseEventKind::ScrollUp),
            &mut app,
        );
        assert!(surface.user_has_scrolled_up);

        // Pump enough scroll-downs to land back at the bottom.
        for _ in 0..10 {
            surface.handle_mouse(
                mouse(ratatui::crossterm::event::MouseEventKind::ScrollDown),
                &mut app,
            );
        }
        assert_eq!(surface.transcript_scroll, 0, "must be at the bottom");
        assert!(
            !surface.user_has_scrolled_up,
            "reaching the bottom must clear the sticky flag"
        );
    }

    #[test]
    fn end_jumps_to_bottom_and_clears_sticky_flag() {
        // D2: End key is the explicit "jump to latest" path. It must work
        // even when the user is many pages above the bottom.
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        // Scroll far up.
        for _ in 0..20 {
            surface.handle_mouse(
                mouse(ratatui::crossterm::event::MouseEventKind::ScrollUp),
                &mut app,
            );
        }
        assert!(surface.transcript_scroll > 0);
        assert!(surface.user_has_scrolled_up);

        // End → snap to bottom.
        surface.handle_key(key(KeyCode::End), &mut app);
        assert_eq!(surface.transcript_scroll, 0);
        assert!(!surface.user_has_scrolled_up);
    }

    #[test]
    fn wrapped_long_lines_remain_visible_at_bottom_anchor_v0914() {
        // v0.9.1.4 H1: Sean's "Top-5 GitHub list cuts after item 2"
        // bug. When assistant turns contain long lines (URLs, repo
        // descriptions) that wrap to 2-3 visual rows, the previous
        // bottom-anchor math used `lines.len()` (logical) instead of
        // the post-wrap visual count. `Paragraph::scroll` operates in
        // VISUAL rows, so a transcript with `logical=20` lines that
        // wrap to `visual=35` rows in a `height=25` viewport got
        // `bottom_anchor = 20 - 25 = 0`, painting rows 0..25 of 35 —
        // the last 10 rows (where items 4/5 + Caveat + Sources live)
        // were never visible, and `End` / `jump_to_bottom`
        // (scroll=0) could not reach them.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        let mut body = String::new();
        for i in 1..=5 {
            body.push_str(&format!(
                "{i}. example-org/long-repo-name-number-{i} (https://github.com/example-org/long-repo-name-number-{i}) — {i}000 stars long description forcing this line to wrap to 2+ visual rows.\n",
            ));
        }
        body.push_str("END_OF_LIST_v0914\n");
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(body)],
        });
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 80, 25);
        assert!(
            out.contains("END_OF_LIST_v0914"),
            "the last logical line must be visible after the default \
             bottom-anchor; the viewport hid it pre-fix:\n{out}"
        );
        assert!(
            !surface.user_has_scrolled_up,
            "default render must not flip the sticky-up flag"
        );
    }

    #[test]
    fn end_key_scrolls_past_wrap_overflow_to_last_line_v0914() {
        // v0.9.1.4 H1 companion: after the user scrolls up and presses
        // End, the LAST logical line must become visible — even when
        // the turn's wrapped height exceeds the viewport. Pre-fix the
        // wrap-overflow tail was unreachable.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        let mut body = String::new();
        for i in 1..=8 {
            body.push_str(&format!(
                "{i}. example-org/long-repo-name-{i} (https://github.com/example-org/long-repo-name-{i}) — {i}00 stars description long enough to wrap past 80 cols.\n",
            ));
        }
        body.push_str("FINAL_v0914\n");
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(body)],
        });
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 80, 20);
        for _ in 0..5 {
            surface.handle_key(key(KeyCode::PageUp), &mut app);
        }
        assert!(
            surface.user_has_scrolled_up,
            "PageUp must arm sticky-up before End is tested"
        );
        surface.handle_key(key(KeyCode::End), &mut app);
        let out = render_to_string(&mut surface, &app, 80, 20);
        assert!(
            out.contains("FINAL_v0914"),
            "End must surface the final logical line; wrap-overflow \
             tail was unreachable pre-fix:\n{out}"
        );
        assert_eq!(surface.transcript_scroll, 0, "End → scroll=0");
        assert!(
            !surface.user_has_scrolled_up,
            "End must clear sticky-up so autoscroll resumes"
        );
    }

    #[test]
    fn last_total_lines_reports_wrapped_height_v0914() {
        // v0.9.1.4 H1 companion: `last_total_lines` is the source of
        // truth for `scroll_up_by`'s `max_up` clamp and for the
        // scrollbar overflow check. Pre-fix it held the LOGICAL line
        // count, so `max_up` under-clamped (PgUp could not scroll to
        // the true top once wrap overflow existed) and the scrollbar
        // mis-reported "no overflow" for a viewport that was actually
        // hiding rows. Post-fix it must report the POST-WRAP visual
        // count.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        let very_long = "x".repeat(200);
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(very_long)],
        });
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 40, 10);
        // The single 200-char body line wraps to at least 5 visual
        // rows at 40 cols. Pre-fix `last_total_lines` would have been
        // ~2-3 (the logical Line count); post-fix it must reflect the
        // wrapped reality.
        assert!(
            surface.last_total_lines >= 5,
            "last_total_lines must report wrapped visual rows, got {}",
            surface.last_total_lines
        );
    }

    #[test]
    fn new_turn_does_not_visually_jump_while_sticky_up() {
        // D2: while `user_has_scrolled_up == true`, a new turn arriving must
        // NOT visually scroll the transcript over the user's reading row.
        // We measure this via `transcript_scroll`: when the bottom anchor
        // grows by N lines, `transcript_scroll` must grow by the same N so
        // the same content stays anchored at the same screen row.
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);

        // Scroll up.
        surface.handle_mouse(
            mouse(ratatui::crossterm::event::MouseEventKind::ScrollUp),
            &mut app,
        );
        let scroll_before = surface.transcript_scroll;
        let total_before = surface.last_total_lines;
        assert!(surface.user_has_scrolled_up);

        // A new user turn appends `≥1` line to the transcript (direct
        // push — see `app_with_long_transcript` for why).
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::User,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(
                "fresh inbound".to_string(),
            )],
        });
        let _ = render_to_string(&mut surface, &app, 100, 30);
        let total_after = surface.last_total_lines;
        let delta = total_after - total_before;
        assert!(delta > 0, "the new turn must have grown the transcript");
        assert_eq!(
            surface.transcript_scroll,
            scroll_before + delta,
            "transcript_scroll must absorb the new lines while sticky-up"
        );
    }

    #[test]
    fn scrollbar_renders_only_when_content_overflows() {
        // D2: the scrollbar is decorative-when-not-needed. With a small
        // transcript (no overflow) no track is drawn. With a large one
        // (overflow) the rightmost column of the transcript pane has a
        // visible track. ratatui's Scrollbar paints "│" plus thumb glyphs
        // — we use the unicode line glyph as a marker.
        // Empty session → no overflow → no scrollbar.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        // Idle hero takes the pane, but we want the non-idle render path —
        // add ONE small system turn so the transcript renders normally.
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::System,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(
                "hi".to_string(),
            )],
        });
        let mut surface = WorkspaceSurface::new();
        let out_small = render_to_string(&mut surface, &app, 100, 30);
        let small_has_track = out_small.contains('│');
        // We can't strictly assert "no │ at all" (Block borders use box-
        // drawing chars in other widgets). Instead, prove the scrollbar
        // bookkeeping reports no overflow.
        assert!(
            surface.last_total_lines <= surface.last_text_area_height,
            "small transcript must not overflow (got total={} height={})",
            surface.last_total_lines,
            surface.last_text_area_height
        );

        // Now a long transcript → overflow → scrollbar.
        let big_app = app_with_long_transcript(80);
        let mut big_surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut big_surface, &big_app, 100, 30);
        assert!(
            big_surface.last_total_lines > big_surface.last_text_area_height,
            "long transcript must overflow (got total={} height={})",
            big_surface.last_total_lines,
            big_surface.last_text_area_height
        );
        // Sanity: small ≠ big — different content paths exercised.
        let _ = small_has_track;
    }

    #[test]
    fn page_up_jumps_area_height_minus_two() {
        // D2: PageUp uses `last_text_area_height - 2` once an area has been
        // observed. Falls back to 5 before the first render.
        let mut app = app_with_long_transcript(80);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        let h = surface.last_text_area_height;
        assert!(h > 2, "test setup must observe a real area: got {h}");

        let before = surface.transcript_scroll;
        surface.handle_key(key(KeyCode::PageUp), &mut app);
        let jumped = surface.transcript_scroll - before;
        assert_eq!(
            jumped,
            h - 2,
            "PageUp jump = area.height - 2 (got {jumped}, expected {})",
            h - 2
        );
    }

    #[test]
    fn shift_up_scrolls_one_line() {
        // D2: Shift+Up scrolls exactly one line — fine-grained scroll
        // without colliding with the composer's own cursor movement.
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        let before = surface.transcript_scroll;
        surface.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::SHIFT), &mut app);
        assert_eq!(
            surface.transcript_scroll,
            before + 1,
            "Shift+Up = 1 line scroll up"
        );
    }

    // ── v0.9.1 W1 A: new scroll regression tests ──────────────────────────

    #[test]
    fn programmatic_scroll_up_sets_user_has_scrolled_up_flag_v091() {
        // Direct-API variant of `mouse_scroll_up_sets_user_has_scrolled_up_flag`:
        // calling `scroll_up_by` from any non-mouse path (Up arrow, PageUp,
        // Shift+Up) also latches the sticky flag.
        let app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        assert!(!surface.user_has_scrolled_up);
        surface.scroll_up_by(3);
        assert!(surface.user_has_scrolled_up);
    }

    #[test]
    fn new_turn_does_not_auto_scroll_when_user_scrolled_up_v091() {
        // Sticky-bumper contract regression. Variant of the existing
        // `new_turn_does_not_visually_jump_while_sticky_up` using the
        // direct scroll API.
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);

        surface.scroll_up_by(3);
        let scroll_before = surface.transcript_scroll;
        let total_before = surface.last_total_lines;

        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::User,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(
                "fresh inbound".to_string(),
            )],
        });
        let _ = render_to_string(&mut surface, &app, 100, 30);
        let delta = surface.last_total_lines - total_before;
        assert!(delta > 0);
        assert_eq!(
            surface.transcript_scroll,
            scroll_before + delta,
            "transcript_scroll must absorb the new lines while sticky-up"
        );
    }

    #[test]
    fn scroll_to_bottom_clears_flag_via_direct_api_v091() {
        let app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);

        surface.scroll_up_by(5);
        assert!(surface.user_has_scrolled_up);
        for _ in 0..10 {
            surface.scroll_down_by(3);
        }
        assert_eq!(surface.transcript_scroll, 0);
        assert!(!surface.user_has_scrolled_up);
    }

    #[test]
    fn new_turn_after_bottom_scrolls_to_bottom_v0911() {
        // v0.9.1.1 F3 regression: live e2e found the viewport stuck on
        // previous content when the user typed a new question and the
        // response landed below the visible area. Root cause: state-
        // machine paths could leave a small positive `transcript_scroll`
        // with `user_has_scrolled_up == false` (the flag tracks the
        // sticky-up reading position; the scroll counter does NOT
        // automatically follow). The defensive clamp in the render path
        // now forces `transcript_scroll = 0` whenever the user is not
        // sticky-up, matching the mockup §6 snap-to-bottom behaviour.
        //
        // Simulate the broken state, render, and assert the snap.
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);

        // Synthesize the broken state: scrolled away from the bottom,
        // but the sticky-up flag is false (e.g. surface restored from a
        // session checkpoint, or a downstream code path forgot to clear
        // the counter).
        surface.transcript_scroll = 7;
        surface.user_has_scrolled_up = false;

        // A new assistant turn arrives — the most common live trigger
        // for the F3 symptom.
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(
                "fresh assistant reply that should be visible".to_string(),
            )],
        });
        let _ = render_to_string(&mut surface, &app, 100, 30);

        // The render must have snapped scroll to 0 (bottom) — the user
        // is not sticky-up, so there is no reading position to preserve.
        assert_eq!(
            surface.transcript_scroll, 0,
            "render must snap transcript_scroll to 0 when user is not sticky-up"
        );
        assert!(
            !surface.user_has_scrolled_up,
            "user_has_scrolled_up must remain false after the snap"
        );
    }

    #[test]
    fn scrollbar_overflow_bookkeeping_v091() {
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::System,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(
                "hi".to_string(),
            )],
        });
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        assert!(surface.last_total_lines <= surface.last_text_area_height);

        let big_app = app_with_long_transcript(80);
        let mut big_surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut big_surface, &big_app, 100, 30);
        assert!(big_surface.last_total_lines > big_surface.last_text_area_height);
    }

    #[test]
    fn mouse_scroll_up_increments_by_3_v091() {
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        let before = surface.transcript_scroll;
        surface.handle_mouse(
            mouse(ratatui::crossterm::event::MouseEventKind::ScrollUp),
            &mut app,
        );
        assert_eq!(surface.transcript_scroll, before + 3);
    }

    #[test]
    fn page_up_jumps_area_height_minus_two_v091() {
        let mut app = app_with_long_transcript(80);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        let h = surface.last_text_area_height;
        assert!(h > 2);
        let before = surface.transcript_scroll;
        surface.handle_key(key(KeyCode::PageUp), &mut app);
        assert_eq!(surface.transcript_scroll - before, h - 2);
    }

    #[test]
    fn arrow_up_when_composer_empty_scrolls_one_line_v091() {
        // B1 fix: bare Up scrolls the transcript ONE line when the
        // composer is empty. The discoverable arrow-to-scroll affordance.
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        assert_eq!(surface.composer.value(), "");
        let before = surface.transcript_scroll;
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(
            surface.transcript_scroll,
            before + 1,
            "bare Up on an empty composer must scroll one line"
        );
    }

    #[test]
    fn arrow_up_with_typed_text_does_not_scroll_v091() {
        // B1 contract negative: arrows pass through to tui-input when
        // the composer has typed content.
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        for c in "draft".chars() {
            surface.handle_key(key(KeyCode::Char(c)), &mut app);
        }
        let before = surface.transcript_scroll;
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(
            surface.transcript_scroll, before,
            "bare Up with composer content must NOT scroll the transcript"
        );
    }

    #[test]
    fn sticky_bumper_skipped_when_scroll_is_zero_at_render_start_v091() {
        // The B13 protection: a sticky flag with `transcript_scroll == 0`
        // at render start (legacy state) must NOT trigger the new-turn
        // delta absorption — there is no reading position to preserve.
        let mut app = app_with_long_transcript(50);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        surface.user_has_scrolled_up = true;
        surface.transcript_scroll = 0;
        let total_before = surface.last_total_lines;

        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::User,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(
                "incoming".to_string(),
            )],
        });
        let _ = render_to_string(&mut surface, &app, 100, 30);
        assert!(surface.last_total_lines > total_before);
        assert_eq!(
            surface.transcript_scroll, 0,
            "bumper must skip when scroll is 0 at render start (B13 fix)"
        );
    }

    // ── v0.9.1 W1 A: render dispatch regression tests ─────────────────────

    #[test]
    fn render_turns_calls_render_markdown_for_markdown_element_v091() {
        // The raw-text bug was: assistant `Markdown(s)` elements were
        // pushed as plain lines, so `**bold**` leaked as literal
        // asterisks. The fix calls `render_markdown(s, theme)` so the
        // asterisks are consumed.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(
                "Hello, **bold** world".to_string(),
            )],
        });
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 100, 30);
        assert!(out.contains("bold"), "body text missing:\n{out}");
        assert!(
            !out.contains("**bold**"),
            "raw markdown leaked — render_markdown not called:\n{out}"
        );
    }

    #[test]
    fn render_turns_appends_sources_block_v091() {
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![
                crate::tui::turn_element::TurnElement::Markdown("see refs".to_string()),
                crate::tui::turn_element::TurnElement::Sources(vec![
                    "https://example.com/a".to_string(),
                ]),
            ],
        });
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 110, 30);
        assert!(out.contains("see refs"));
        assert!(out.contains("Sources"));
        assert!(out.contains("example.com"));
    }

    #[test]
    fn render_turns_renders_tool_card_inline_v091() {
        // A Running tool card renders inline in the transcript flow
        // (between turn elements and any approval card) so the user
        // reads tool call → result → next as one stream.
        //
        // v0.9.1 W2 cycle-2 regression guard: the tool name must appear
        // EXACTLY ONCE in the TRANSCRIPT region. Before the BLOCKER 1 fix
        // tool cards rendered twice — once inline from W1 A's rewrite,
        // once via the legacy `render_tool_cards` strip — and the old
        // assertion (`contains("Bash")`) passed despite the double draw.
        //
        // The right rail's Tools + Activity panels also surface the tool
        // name (by design — that's the rail's whole job) so we hide the
        // rail with `app.rail_visible = false` to isolate the transcript
        // pane for this count assertion.
        let mut app = app_from_fixture(tool_fixture_prefix(1));
        app.rail_visible = false;
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 100, 30);
        let bash_count = out.matches("Bash").count();
        assert_eq!(
            bash_count, 1,
            "tool name must render EXACTLY ONCE in the transcript \
             (regression guard for the cards_area + push_tool_card_lines \
             double-render bug); got {bash_count} occurrences:\n{out}"
        );
        assert!(
            out.contains("running") || out.contains("cargo test"),
            "tool card body missing:\n{out}"
        );
    }

    // ── v0.9.3 W1.4 — Thinking element collapsed projection ──────────────

    #[test]
    fn render_turns_renders_thinking_collapsed_by_default_v093() {
        // A persisted Thinking element renders as the collapsed
        // `▶ Thought: <title>` line when `reasoning_expanded` has no
        // entry for this turn index.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![crate::tui::turn_element::TurnElement::Thinking {
                body: "Weighing the refactor trade-offs.".to_string(),
                secs: 3,
                tokens: 64,
            }],
        });
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 120, 30);
        assert!(
            out.contains("▶"),
            "collapsed Thought marker missing:\n{out}"
        );
        assert!(out.contains("Thought"), "Thought label missing:\n{out}");
        assert!(
            out.contains("Weighing the refactor trade-offs"),
            "Thought title missing:\n{out}"
        );
    }

    #[test]
    fn render_turns_renders_thinking_expanded_when_flag_set_v093() {
        // Toggling `reasoning_expanded[turn_idx] = true` flips the marker
        // to ▼ and appends the wrapped body lines under the header.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![crate::tui::turn_element::TurnElement::Thinking {
                body: "First line\nSecond line".to_string(),
                secs: 0,
                tokens: 0,
            }],
        });
        app.reasoning_expanded.insert(0, true);
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 120, 30);
        assert!(out.contains("▼"), "expanded Thought marker missing:\n{out}");
        assert!(
            out.contains("First line"),
            "expanded body line 1 missing:\n{out}"
        );
        assert!(
            out.contains("Second line"),
            "expanded body line 2 missing:\n{out}"
        );
    }

    // ── v0.9.1.1 F5 — hide-when-empty rail panels ────────────────────
    // Sean direct feedback during xterm.js drive 2026-05-27: "If there's
    // no fucking activity then hide that shit. What's the point in
    // having a name to your fucking window if there's nothing to show
    // in it?" — every right-rail panel must omit itself when its data
    // source is empty, and the entire rail must collapse when all three
    // are empty (transcript reclaims the column).

    #[test]
    fn right_rail_activity_panel_hidden_when_no_events() {
        // A finished tool call (status=Ok) populates the Tools panel
        // but leaves Activity empty (no in-flight work, no system
        // notice). Activity must not render.
        let app = app_from_fixture(fixtures::tool_call_with_approval());
        assert!(activity_panel_is_empty(&app));
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 110, 30);
        assert!(
            !out.contains("Activity"),
            "Activity panel rendered with no events:\n{out}"
        );
        assert!(
            !out.contains("no activity yet"),
            "stale empty-state placeholder still rendered:\n{out}"
        );
    }

    #[test]
    fn right_rail_path_map_hidden_when_no_files() {
        // The Bash fixture does not touch any path → path_map.roots is
        // empty → Path map panel must not render.
        let app = app_from_fixture(fixtures::tool_call_with_approval());
        assert!(path_map_is_empty(&app));
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 110, 30);
        assert!(
            !out.contains("Path map"),
            "Path map panel rendered with no files:\n{out}"
        );
        assert!(
            !out.contains("no files touched yet"),
            "stale empty-state placeholder still rendered:\n{out}"
        );
    }

    #[test]
    fn right_rail_tools_panel_hidden_when_no_tool_calls() {
        // A pure text fixture (no ToolRequest events) leaves
        // tool_cards empty → Tools panel must not render.
        let app = app_from_fixture(fixtures::full_conversation());
        assert!(tools_panel_is_empty(&app));
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 110, 30);
        assert!(
            !out.contains("Tools "),
            "Tools panel rendered with no tool calls:\n{out}"
        );
        assert!(
            !out.contains("no tools used yet"),
            "stale empty-state placeholder still rendered:\n{out}"
        );
    }

    #[test]
    fn right_rail_fully_collapses_when_all_panels_empty() {
        // An entirely cold session: no tool calls, no system notices,
        // no path-map data. Every panel is empty and the whole rail
        // collapses so the transcript reclaims the column width.
        let app = App::new();
        assert!(path_map_is_empty(&app));
        assert!(tools_panel_is_empty(&app));
        assert!(activity_panel_is_empty(&app));
        assert!(rail_is_empty(&app));
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 110, 30);
        assert!(
            !out.contains("Path map") && !out.contains("Tools ") && !out.contains("Activity"),
            "rail panel chrome still drawn on a fully-empty session:\n{out}"
        );
    }

    /// v0.9.1.2 W8 regression: the "Tools · N call(s)" rail panel was
    /// deleted because tool cards already render inline in the transcript
    /// after F12 (recon §4.7 parallel-panel anti-pattern). Activity
    /// remains as the rail's recent-events feed — different signal, not
    /// a mirror of the inline cards.
    ///
    /// Test: populate `tool_cards` with multiple cards in mixed states,
    /// then render the workspace and assert:
    ///   * the "Tools · " heading is NOT in the rail (regression guard);
    ///   * the inline tool-card chrome IS in the transcript (so we
    ///     didn't accidentally delete the inline render too);
    ///   * Activity DOES render when an in-flight card is present
    ///     (the rail is not unconditionally hidden).
    #[test]
    fn right_rail_no_longer_renders_tools_panel_v0913() {
        // Build a session with a running tool card so the Activity feed
        // has signal AND `tool_cards` is non-empty. Pre-W8 this would
        // have rendered BOTH a "Tools · 2 call(s)" rail panel AND an
        // Activity panel. After W8 only Activity renders.
        let mut app = app_from_fixture(fixtures::tool_call_with_approval());
        // Drop a second card in `Ok` state so the per-tool tally would
        // have been "Bash ×2" — making the W8 regression unambiguous.
        let mut extra = app.session.tool_cards[0].clone();
        extra.call_id = "call-2".into();
        extra.status = ToolCardStatus::Ok;
        app.session.tool_cards.push(extra);
        // Force the first card back to Running so Activity has a row.
        app.session.tool_cards[0].status = ToolCardStatus::Running;

        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 110, 30);

        assert!(
            !out.contains("Tools · "),
            "v0.9.1.2 W8: the Tools rail panel must not render:\n{out}"
        );
        assert!(
            !out.contains("Bash    ×"),
            "v0.9.1.2 W8: the per-tool tally must not render:\n{out}"
        );
        // Activity remains a rail tenant when in-flight work exists.
        assert!(
            out.contains("Activity"),
            "Activity panel must still render when in-flight tool work \
             is present:\n{out}"
        );
        // Sanity: the inline tool-card chrome is still emitted — the
        // running spinner glyph row carries the tool name in the
        // transcript flow.
        assert!(
            out.contains("Bash"),
            "inline tool card was accidentally removed:\n{out}"
        );
    }

    /// v0.9.1.1 bonus fix: the inline tool-card render path now calls
    /// `tool_formatters::formatter_for(...)` so the per-tool summary
    /// shows up under the header. Before this, a successful `WebFetch`
    /// rendered ONLY `● WebFetch(...) · done` and a failing call
    /// dumped 4 lines of raw JSON. The test synthesizes a WebFetch
    /// result, runs it through `push_tool_card_lines`, and asserts the
    /// formatter's summary appears AND raw JSON keys do NOT.
    #[test]
    fn tool_card_inline_render_uses_formatter_for_summary_v0911() {
        use crate::tui::app::{ToolCardModel, ToolCardStatus};
        // A canonical WebFetch result envelope as the engine produces
        // (`wcore-tool-web-fetch`): url + bytes + readability_score +
        // content. The formatter's job is to collapse it into a
        // compact `Fetched <host> · N bytes …` line. The raw output is
        // JSON; if the render path bypassed the formatter (the v0.9.1
        // bug), those JSON keys would leak verbatim.
        let card = ToolCardModel {
            call_id: "c1".into(),
            tool_name: "WebFetch".into(),
            summary: String::new(),
            status: ToolCardStatus::Ok,
            output: Some(
                r#"{"url":"https://example.com/page","bytes":42137,"readability_score":0.91,"content":"hello world"}"#
                    .into(),
            ),
            edit_preview: None,
            input_pretty: "{}".into(),
            approval_reason: String::new(),
            plan_body: None,
        };
        let theme = Theme::hearth();
        let mut app = App::new();
        // Compact mode (default) is what we want to test — even compact
        // mode should emit the formatter's summary as one body line.
        app.session.compact_tool_output = true;
        let mut lines: Vec<Line<'static>> = Vec::new();
        push_tool_card_lines(&mut lines, &card, &theme, true, &app, 80, None, 0, false);
        let flat: String = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    + "\n"
            })
            .collect();

        // WebFetch formatter idiom: `Fetched <host> · <bytes> bytes …`.
        // Before the wire-in, the body never emitted this string.
        assert!(
            flat.contains("Fetched"),
            "WebFetch formatter summary missing `Fetched` token \
             (formatter not wired into push_tool_card_lines). Body:\n{flat}"
        );
        assert!(
            flat.contains("example.com"),
            "WebFetch formatter summary missing host. Body:\n{flat}"
        );
        assert!(
            flat.contains("42137"),
            "WebFetch formatter summary missing byte count. Body:\n{flat}"
        );

        // Anti-leak: raw JSON syntax (quoted-key colon shape) must NOT
        // appear anywhere in the rendered output.
        assert!(
            !flat.contains("\"content\":"),
            "raw JSON key `content:` leaked into inline render. Body:\n{flat}"
        );
        assert!(
            !flat.contains("\"readability_score\":"),
            "raw JSON key `readability_score:` leaked. Body:\n{flat}"
        );

        // Now test the Err arm — it forces expanded mode and must
        // also route through the formatter rather than dumping raw
        // JSON.lines().take(4) (the pre-wire-in behaviour).
        let err_card = ToolCardModel {
            status: ToolCardStatus::Err,
            output: Some(
                r#"{"url":"https://example.com/api","bytes":0,"content":"upstream timeout"}"#
                    .into(),
            ),
            ..card.clone()
        };
        let mut err_lines: Vec<Line<'static>> = Vec::new();
        push_tool_card_lines(
            &mut err_lines,
            &err_card,
            &theme,
            true,
            &app,
            80,
            None,
            0,
            false,
        );
        let err_flat: String = err_lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        // Err arm forces expanded mode, so we expect both the summary
        // AND (when the formatter has them) detail lines. Either way,
        // raw JSON syntax must not leak.
        assert!(
            !err_flat.contains("\"url\":"),
            "raw JSON `url:` syntax leaked into Err render:\n{err_flat}"
        );
        assert!(
            !err_flat.contains("\"bytes\":"),
            "raw JSON `bytes:` syntax leaked into Err render:\n{err_flat}"
        );
        // Formatter summary should still be reachable from the Err
        // path — the audit's `take(4)` raw dump is replaced.
        assert!(
            err_flat.contains("Fetched") || err_flat.contains("example.com"),
            "Err arm bypassed the formatter (no host/Fetched token):\n{err_flat}"
        );
    }

    // ── v0.9.2 W11-integ S20 — live one-liner glyph matches toolcard.rs ──
    /// The inline one-liner row's status glyph must use the SAME S20 map
    /// W7 locked in `widgets/toolcard.rs::status_icon`: `●` done (NOT the
    /// old `✓`), `✗` error. This drives the live transcript path through
    /// `push_tool_card_lines`, so a regression that reverts to `✓`/`·`
    /// fails here.
    #[test]
    fn tool_card_oneliner_uses_s20_glyph_map_v0912() {
        use crate::tui::app::{ToolCardModel, ToolCardStatus};
        let theme = Theme::hearth();
        let app = App::new();

        let flatten = |lines: &[Line<'static>]| -> String {
            lines
                .iter()
                .map(|l| {
                    l.spans
                        .iter()
                        .map(|s| s.content.as_ref())
                        .collect::<String>()
                        + "\n"
                })
                .collect()
        };

        let ok_card = ToolCardModel {
            call_id: "c1".into(),
            tool_name: "Bash".into(),
            summary: String::new(),
            status: ToolCardStatus::Ok,
            output: None,
            edit_preview: None,
            input_pretty: "{}".into(),
            approval_reason: String::new(),
            plan_body: None,
        };
        let mut ok_lines: Vec<Line<'static>> = Vec::new();
        push_tool_card_lines(
            &mut ok_lines,
            &ok_card,
            &theme,
            true,
            &app,
            80,
            None,
            0,
            false,
        );
        let ok_flat = flatten(&ok_lines);
        assert!(
            ok_flat.contains('●'),
            "done one-liner must use the S20 `●` glyph. Body:\n{ok_flat}"
        );
        assert!(
            !ok_flat.contains('✓'),
            "done one-liner must NOT use the old `✓` glyph. Body:\n{ok_flat}"
        );

        let err_card = ToolCardModel {
            status: ToolCardStatus::Err,
            ..ok_card.clone()
        };
        let mut err_lines: Vec<Line<'static>> = Vec::new();
        push_tool_card_lines(
            &mut err_lines,
            &err_card,
            &theme,
            true,
            &app,
            80,
            None,
            0,
            false,
        );
        let err_flat = flatten(&err_lines);
        assert!(
            err_flat.contains('✗'),
            "error one-liner must keep the `✗` glyph. Body:\n{err_flat}"
        );
    }

    // ── v0.9.1.1 F9 — Activity rail JSON envelope strip ──────────────
    // Sean direct feedback during xterm.js drive 2026-05-27: the rail
    // showed `cancelled text_to_speech · {"t...` — the trailing
    // `· {"t...` is the start of a JSON envelope being clipped
    // mid-character. The rail must never show raw JSON.

    #[test]
    fn sanitize_activity_line_parses_friendly_json_v0911() {
        // Common tool-result envelope shape — the LLM cancellation
        // path emits `{"tool":"X","status":"done"}`.
        let out = sanitize_activity_line(r#"{"tool":"web","status":"done","body":"long"}"#);
        assert_eq!(out, "web · done");
    }

    #[test]
    fn sanitize_activity_line_strips_json_with_prefix_v0911() {
        // Real shape from the screenshot: an action verb followed by
        // a `· {...` JSON tail.
        let out = sanitize_activity_line(
            r#"cancelled text_to_speech · {"tool":"text_to_speech","error":"API 400"}"#,
        );
        assert!(
            !out.contains('{') && !out.contains('}'),
            "JSON envelope leaked: {out}"
        );
        assert!(out.contains("text_to_speech"), "tool name dropped: {out}");
    }

    #[test]
    fn sanitize_activity_line_truncates_unparseable_json_v0911() {
        // Mid-character clip (matches the live screenshot exactly).
        let out = sanitize_activity_line(r#"cancelled text_to_speech · {"t..."#);
        assert!(
            !out.contains('{') && !out.contains('['),
            "raw JSON opener leaked: {out}"
        );
        // The human-readable prefix survives.
        assert!(out.starts_with("cancelled text_to_speech"));
        // Tail signals that something was trimmed.
        assert!(out.ends_with('…'), "missing truncation marker: {out}");
    }

    #[test]
    fn sanitize_activity_line_passes_through_plain_text_v0911() {
        // No JSON, no change.
        let out = sanitize_activity_line("running web · search 'rust async'");
        assert_eq!(out, "running web · search 'rust async'");
    }

    #[test]
    fn clip_to_width_appends_ellipsis_only_when_clipped_v0911() {
        assert_eq!(clip_to_width("abc", 10), "abc");
        assert_eq!(clip_to_width("abcdef", 4), "abc…");
        // max=0 yields empty (defensive — rail width can't ever be
        // that small in practice).
        assert_eq!(clip_to_width("abc", 0), "");
        // Boundary: len == max → no ellipsis.
        assert_eq!(clip_to_width("abcd", 4), "abcd");
    }

    #[test]
    fn activity_rail_strips_json_envelopes_v0911() {
        // Reach into `render_activity_feed`'s entry-building logic
        // by feeding the sanitizer the exact strings the rail
        // composes from a Running card with a JSON-bodied summary.
        // (A full end-to-end render also exercises the transcript
        // tool-card body, which legitimately shows the input JSON;
        // this test stays focused on the sanitizer the rail uses.)
        let composed = format!(
            "running {} · {}",
            "unknown_tool_xyz", r#"{"target":"wss://x"}"#
        );
        let sanitized = sanitize_activity_line(&composed);
        assert!(
            !sanitized.contains('{'),
            "rail entry still contains `{{`: {sanitized}"
        );
        assert!(
            !sanitized.contains('['),
            "rail entry still contains `[`: {sanitized}"
        );
        // The sanitizer should produce a friendly summary preserving
        // the tool name — either via `friendly_summary_from_json`
        // (which extracts a `tool · status` form) or by stripping the
        // envelope and keeping the prefix.
        assert!(
            sanitized.contains("unknown_tool_xyz"),
            "tool name dropped during sanitize: {sanitized}"
        );
    }

    // ── v0.9.1.1 H5: `?` runs /help when composer is empty ───────────

    #[test]
    fn question_mark_opens_global_help_when_composer_empty_v0911() {
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        // Composer starts empty — `?` must NOT be typed; it must emit
        // the global help command instead.
        let action = surface.handle_key(key(KeyCode::Char('?')), &mut app);
        match action {
            SurfaceAction::Command(line) => {
                assert_eq!(line, "/help", "? did not route to /help");
            }
            other => panic!(
                "expected SurfaceAction::Command(\"/help\"), got {:?}",
                other
            ),
        }
        // The composer must remain empty (the key was consumed by the
        // global path, not typed).
        assert!(
            surface.composer.value().is_empty(),
            "? leaked into the composer despite the global-help fix"
        );
    }

    #[test]
    fn question_mark_types_into_composer_when_non_empty_v0911() {
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        // Prime the composer with prose — the user is typing a question.
        for c in "is this".chars() {
            surface.handle_key(key(KeyCode::Char(c)), &mut app);
        }
        assert_eq!(surface.composer.value(), "is this");
        // Now `?` must type as a literal, not fire /help.
        let action = surface.handle_key(key(KeyCode::Char('?')), &mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "? on a non-empty composer should not emit a command, got {:?}",
            action
        );
        assert_eq!(
            surface.composer.value(),
            "is this?",
            "? was not appended to the composer"
        );
    }

    // ── v0.9.1.1 B4-hunt: approval card uses formatter, not raw JSON ─

    #[test]
    fn approval_card_renders_friendly_args_not_raw_json_v0911() {
        // The B4-hunt symptom was the `text_to_speech` approval card
        // body showing `{"text":"..."}` raw. The fix routes args through
        // `formatter_for(tool).format_args(...)`; for `text_to_speech`
        // that returns a `"…"` quoted excerpt, not braces or quotes
        // around a key.
        use crate::tui::tool_formatters::formatter_for;
        let args = serde_json::json!({
            "text": "Here is a comprehensive guide to async Rust with code examples."
        });
        let preview = formatter_for("text_to_speech")
            .format_args(&args)
            .expect("text_to_speech formatter must format its args");
        assert!(
            !preview.contains('{'),
            "approval args still rendering as raw JSON: {preview}"
        );
        assert!(
            !preview.contains("\"text\""),
            "approval args still leak the JSON key: {preview}"
        );
        // The preview is a quoted excerpt of the text.
        assert!(
            preview.starts_with('"'),
            "expected a quoted excerpt, got: {preview}"
        );
        assert!(
            preview.contains("Here is"),
            "expected text excerpt, got: {preview}"
        );
    }

    // ── v0.9.1.2 F15 / F16 / F17 — Path map removal, user-turn highlight,
    // spinner alignment ───────────────────────────────────────────────────
    // Sean's 2026-05-27 live xterm.js drive feedback after the v0.9.1.1
    // ship:
    //   F15 "the path map is useless for 1-N files" — the panel is gone.
    //   F16 "I can't tell my prompt apart from the agent's reply" — user
    //   messages render with a column-0 `▌` accent bar plus a slight
    //   surface-alt background tint, OpenClaw style.
    //   F17 "what's that orphan `:` floating below the streaming text" —
    //   the transcript-gutter spinner was removed; the bottom status bar
    //   already shows "Considering… (Ns)" with the rotating verb.

    #[test]
    fn right_rail_no_longer_renders_path_map_v0912() {
        // F15: even with a populated `path_map` the rail must never show
        // the `Path map` panel — the widget is dead. Tools/Activity stay
        // gated on their own predicates. Use a fixture with real tool
        // activity (so the rail is otherwise non-empty) and synthesize a
        // path_map entry to prove the path-map data field is no longer
        // honored by the renderer.
        use crate::tui::app::TreeNode;
        let mut app = app_from_fixture(fixtures::tool_call_with_approval());
        app.path_map.roots.push(TreeNode {
            name: "src".into(),
            is_dir: true,
            children: vec![TreeNode {
                name: "main.rs".into(),
                is_dir: false,
                children: Vec::new(),
            }],
        });
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 110, 30);
        assert!(
            !out.contains("Path map"),
            "F15 regression: Path map panel still rendered despite F15 removing it:\n{out}"
        );
        assert!(
            !out.contains("main.rs"),
            "F15 regression: path-tree contents still rendered:\n{out}"
        );
    }

    #[test]
    fn user_turn_renders_left_bar_v0912() {
        // F16: every line of a user message starts with a `▌` (U+258C
        // LEFT HALF BLOCK) span styled in the brand accent. The span is
        // the load-bearing visual signal that distinguishes user vs.
        // assistant text at a glance.
        let mut lines: Vec<Line<'static>> = Vec::new();
        let turn = TurnView {
            role: TurnRole::User,
            elements: vec![TurnElement::Markdown("hello world".into())],
        };
        let theme = Theme::hearth();
        let app = App::new();
        push_turn(&mut lines, &turn, &theme, &[], &app, 60, None, 0, false, 0);
        let user_line = lines
            .iter()
            .find(|l| !l.spans.is_empty() && l.spans[0].content == "▌")
            .expect("F16: no user-turn line starts with the ▌ accent bar");
        let bar_span = &user_line.spans[0];
        assert_eq!(
            bar_span.style.fg,
            Some(theme.orange),
            "F16: ▌ bar must be styled with theme.orange (brand accent); got {:?}",
            bar_span.style.fg
        );
    }

    #[test]
    fn user_turn_uses_alt_background_v0912() {
        // F16: the user message body span must paint a non-default bg
        // (surface_hover / #262626) so the rectangle reads as distinct
        // from the surrounding `theme.bg` transcript. Without the bg
        // tint the `▌` bar floats alone and the visual contrast collapses
        // to "same color as everything else".
        let mut lines: Vec<Line<'static>> = Vec::new();
        let turn = TurnView {
            role: TurnRole::User,
            elements: vec![TurnElement::Markdown("a question".into())],
        };
        let theme = Theme::hearth();
        let app = App::new();
        push_turn(&mut lines, &turn, &theme, &[], &app, 60, None, 0, false, 0);
        let user_line = lines
            .iter()
            .find(|l| !l.spans.is_empty() && l.spans[0].content == "▌")
            .expect("F16: user-turn ▌ bar missing");
        // The body span is the one carrying the text content; it sits
        // between the `▌` bar and the trailing full-width pad span (see
        // user_turn_bg_extends_full_width_v0912).
        let body_span = user_line
            .spans
            .iter()
            .skip(1)
            .find(|s| s.content.contains("a question"))
            .expect("F16: user-turn body span missing");
        assert_eq!(
            body_span.style.bg,
            Some(theme.surface_hover),
            "F16: user-turn body must paint surface_hover bg; got {:?}",
            body_span.style.bg
        );
    }

    #[test]
    fn user_turn_bg_extends_full_width_v0912() {
        // F16-followup: the surface_hover tint must extend to the right
        // edge of the transcript pane, not just end-of-text. A trailing
        // pad span of bg-styled whitespace fills the row to
        // `content_width` total columns so the message reads as a
        // contiguous "card". Verifies: (1) the last span is whitespace,
        // (2) the last span carries the surface_hover bg, (3) total
        // visible width across all spans equals `content_width`.
        let mut lines: Vec<Line<'static>> = Vec::new();
        let turn = TurnView {
            role: TurnRole::User,
            // "short msg" is 9 visible columns; with " " prefix and ▌
            // that's 11 used columns; pad must be 49 to fill 60.
            elements: vec![TurnElement::Markdown("short msg".into())],
        };
        let theme = Theme::hearth();
        let app = App::new();
        let content_width: u16 = 60;
        push_turn(
            &mut lines,
            &turn,
            &theme,
            &[],
            &app,
            content_width,
            None,
            0,
            false,
            0,
        );
        let user_line = lines
            .iter()
            .find(|l| !l.spans.is_empty() && l.spans[0].content == "▌")
            .expect("F16-followup: user-turn ▌ bar missing");
        let last = user_line
            .spans
            .last()
            .expect("F16-followup: user-turn line has no spans");
        assert!(
            !last.content.is_empty() && last.content.chars().all(|c| c == ' '),
            "F16-followup: last span must be whitespace-only pad; got {:?}",
            last.content
        );
        assert_eq!(
            last.style.bg,
            Some(theme.surface_hover),
            "F16-followup: pad span must paint surface_hover bg; got {:?}",
            last.style.bg
        );
        let total_width: usize = user_line.spans.iter().map(|s| s.width()).sum();
        assert_eq!(
            total_width as u16, content_width,
            "F16-followup: sum of span widths must equal content_width; got {} vs {}",
            total_width, content_width
        );
    }

    /// v0.9.1.2 F14b / v0.9.2 W2 — drive TWO sequential tool calls through
    /// the real bridge and assert the SECOND tool's single-surface inline
    /// approval dialog lands in the rendered transcript. The first tool is
    /// fully resolved (Ok), the second is left `AwaitingApproval` — the
    /// exact multi-turn scenario Sean saw in the e2e drive where the bottom
    /// status + right rail signalled "pending" but the inline card never
    /// appeared in the scrollback.
    #[test]
    fn subsequent_tool_approval_card_renders_inline_v0912() {
        use serde_json::json;
        use wcore_protocol::events::{
            OutputType, ProtocolEvent, ToolCategory, ToolInfo, ToolStatus,
        };

        // Tool #1: full lifecycle — ToolRequest → ApprovalRequired →
        // ToolRunning → ToolResult(Ok). Card ends in `Ok` status.
        // Tool #2: ToolRequest → ApprovalRequired only. Card ends
        // `AwaitingApproval` — this is the card whose inline 6-line
        // prompt MUST render.
        let events = vec![
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "call-1".into(),
                tool: ToolInfo {
                    name: "Bash".into(),
                    category: ToolCategory::Exec,
                    args: json!({"command": "ls"}),
                    description: "List dir".into(),
                },
            },
            ProtocolEvent::ApprovalRequired {
                call_id: "call-1".into(),
                resume_token: "tok-1".into(),
                correlation_id: "tok-1".into(),
                reason: "exec".into(),
                context: "first call".into(),
            },
            ProtocolEvent::ToolRunning {
                msg_id: "m1".into(),
                call_id: "call-1".into(),
                tool_name: "Bash".into(),
            },
            ProtocolEvent::ToolResult {
                msg_id: "m1".into(),
                call_id: "call-1".into(),
                tool_name: "Bash".into(),
                status: ToolStatus::Success,
                output: "files listed".into(),
                output_type: OutputType::Text,
                metadata: None,
            },
            // No StreamEnd between the two tool calls — the engine
            // continues the SAME assistant turn (a tool-using assistant
            // routinely chains 2-5 tools in one turn). The 2nd ToolCard
            // element lands on the same in-flight TurnView as the 1st.
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "call-2".into(),
                tool: ToolInfo {
                    name: "Write".into(),
                    category: ToolCategory::Edit,
                    args: json!({"file_path": "/tmp/x.txt", "content": "y"}),
                    description: "Write file".into(),
                },
            },
            ProtocolEvent::ApprovalRequired {
                call_id: "call-2".into(),
                resume_token: "tok-2".into(),
                correlation_id: "tok-2".into(),
                reason: "writes a new file".into(),
                context: "second call".into(),
            },
        ];

        // Simulate the realistic e2e flow: user turn between two
        // assistant turns. (Sean's drive included real user submits
        // between agent calls.)
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        // First user submit lands as a User TurnView.
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::User,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(
                "first request".into(),
            )],
        });
        for event in events {
            crate::tui::protocol_bridge::apply_event(&mut app, event);
        }

        // Sanity: bridge state — first card Ok, second AwaitingApproval.
        assert_eq!(
            app.session.tool_cards.len(),
            2,
            "two tool cards must be present"
        );
        assert_eq!(app.session.tool_cards[0].status, ToolCardStatus::Ok);
        assert_eq!(
            app.session.tool_cards[1].status,
            ToolCardStatus::AwaitingApproval
        );

        // Render and inspect. Use a TALL viewport so nothing is clipped —
        // the bug is "the card isn't in the line stream", not "the card
        // is off-screen".
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 120, 80);

        // v0.9.2 W2 (SPEC §0 #1, §1C): the 2nd card (the head pending
        // card, since the 1st resolved Ok) renders the single-surface
        // permission dialog inline — approve + deny affordances. Only one
        // card is pending, so no `(+N more pending)` tail.
        assert!(
            out.contains("approve"),
            "F14b regression: 2nd tool approve affordance missing from render:\n{out}"
        );
        assert!(
            out.contains("deny"),
            "F14b regression: 2nd tool deny affordance missing from render:\n{out}"
        );
        // The legacy 6-line card header must be gone (single-surface).
        assert!(
            !out.contains("Approve this tool call?"),
            "legacy 6-line approval card header must be gone:\n{out}"
        );
        // The first card's status text must still render (the assertion
        // is "second card ADDED its approval prompt", not "first card was
        // removed"). The first card is Ok, so it carries a "done" pill.
        assert!(
            out.contains("Bash") && out.contains("done"),
            "first tool card must still render its `done` row:\n{out}"
        );
    }

    /// v0.9.1.2 F14b — the protocol bridge's `force_scroll_to_pending_approval`
    /// trigger must arm on EVERY `ApprovalRequired` event, not just the
    /// first one. The bug Sean caught: tick() consumes the flag on the
    /// first approval, but the second approval never re-arms it, leaving
    /// the pending card off-screen if the user scrolled up between tool
    /// calls. Drive two sequential approvals through the bridge and
    /// assert the flag re-arms after the second.
    #[test]
    fn force_scroll_fires_on_every_pending_approval_v0912() {
        use serde_json::json;
        use wcore_protocol::events::{ProtocolEvent, ToolCategory, ToolInfo};

        let mut app = App::new();

        // First approval cycle. Arms the trigger.
        crate::tui::protocol_bridge::apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "c-1".into(),
                tool: ToolInfo {
                    name: "Bash".into(),
                    category: ToolCategory::Exec,
                    args: json!({"command": "ls"}),
                    description: "ls".into(),
                },
            },
        );
        crate::tui::protocol_bridge::apply_event(
            &mut app,
            ProtocolEvent::ApprovalRequired {
                call_id: "c-1".into(),
                resume_token: "t-1".into(),
                correlation_id: "t-1".into(),
                reason: "exec".into(),
                context: String::new(),
            },
        );
        assert!(
            app.force_scroll_to_pending_approval,
            "first ApprovalRequired must arm the force-scroll trigger"
        );

        // Surface tick consumes the flag (Sean's symptom path).
        let mut surface = WorkspaceSurface::new();
        let _ = surface.tick(&mut app);
        assert!(
            !app.force_scroll_to_pending_approval,
            "tick must consume the flag after the first approval (existing v0912 contract)"
        );

        // Second tool, second approval. The flag MUST re-arm — otherwise
        // the user who scrolled away after card #1 won't get snapped
        // back to card #2 when the second approval pops.
        crate::tui::protocol_bridge::apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "c-2".into(),
                tool: ToolInfo {
                    name: "Write".into(),
                    category: ToolCategory::Edit,
                    args: json!({"file_path": "/tmp/x", "content": "y"}),
                    description: "write".into(),
                },
            },
        );
        crate::tui::protocol_bridge::apply_event(
            &mut app,
            ProtocolEvent::ApprovalRequired {
                call_id: "c-2".into(),
                resume_token: "t-2".into(),
                correlation_id: "t-2".into(),
                reason: "write".into(),
                context: String::new(),
            },
        );
        assert!(
            app.force_scroll_to_pending_approval,
            "F14b regression: second ApprovalRequired must re-arm the force-scroll trigger"
        );
    }

    /// v0.9.1.2 F14c — F14b's render-frame `transcript_scroll = 0` snap
    /// has been REMOVED. The sticky approval strip above the composer is
    /// the always-visible signal that an approval is pending, so the
    /// render path no longer fights the user's scroll position. This
    /// test pins the regression: render must NOT clear
    /// `user_has_scrolled_up` and must not call `transcript_scroll = 0`
    /// when an approval lands. The previous F14b test (which asserted
    /// the snap to 0) is replaced by this inversion.
    #[test]
    fn render_frame_no_longer_snaps_scroll_on_pending_v0912() {
        let mut app = app_with_long_transcript(40);
        let mut surface = WorkspaceSurface::new();
        // Seed the surface's `last_total_lines` by rendering once first
        // so the sticky-at-bottom bumper (which adds `total - last_total_lines`
        // when the user is scrolled up) does not skew the assertion on
        // the second render.
        let _ = render_to_string(&mut surface, &app, 80, 20);
        let scroll_before = 5_u16;
        surface.transcript_scroll = scroll_before;
        surface.user_has_scrolled_up = true;
        push_awaiting_card(&mut app, "c-2", "Write");
        // Bridge armed force-scroll on the just-arrived ApprovalRequired.
        // Under F14c the render path no longer reads/clears this flag —
        // `tick()` still consumes it for any other consumer.
        app.force_scroll_to_pending_approval = true;

        let scroll_pre = surface.transcript_scroll;
        let _ = render_to_string(&mut surface, &app, 80, 20);

        // The render path must NOT snap to 0 (the F14b bug). The exact
        // value may differ from `scroll_pre` by the sticky-at-bottom
        // bumper delta when a new card landed, but it must NOT be 0.
        assert_ne!(
            surface.transcript_scroll, 0,
            "F14c: render must NOT snap transcript_scroll to 0 when force-scroll \
             is armed (pre={scroll_pre}, got 0)"
        );
        assert!(
            surface.user_has_scrolled_up,
            "F14c: render must not silently clear user_has_scrolled_up when \
             an approval is pending"
        );
    }

    #[test]
    fn in_flight_spinner_removed_v0912() {
        // F17 (Option B): the live streaming tail used to push a spinner
        // glyph on its own line in the gutter (column 0/1) below the
        // indented assistant text — read as an orphan `:`-shaped braille
        // glyph in Sean's 2026-05-27 drive. The fix removes the gutter
        // spinner from the transcript entirely; the bottom status bar
        // (`render_streaming_status`) already shows the animated
        // "Considering… (Ns · ↑ N tokens)" indicator.
        let mut lines: Vec<Line<'static>> = Vec::new();
        let theme = Theme::hearth();
        // Buffer has a complete-enough prefix that the safe-split lands
        // a rendered line, so this exercises the post-F17 tail path.
        push_streaming_preview(&mut lines, "I'll search for the news.", &theme);
        for line in &lines {
            // No line may carry a braille spinner frame as its first
            // span — those were the legacy gutter glyphs. The 2-space
            // raw indent is fine; the assertion is on the leading span
            // content.
            if let Some(first) = line.spans.first() {
                let braille = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠇"];
                assert!(
                    !braille.iter().any(|g| first.content.starts_with(g)),
                    "F17 regression: orphan braille spinner glyph still leads a streaming line: {:?}",
                    first.content
                );
            }
        }
    }

    // ── v0.9.1.3 F19 — interactivity discoverability + jump-to-latest ─────

    #[test]
    fn pgup_pgdn_scroll_status_hint_advertised_v0913() {
        // F19: the bottom status hint must surface the scroll keybinds.
        // Sean's empirical complaint was "no PgUp/PgDn scroll keys" — the
        // handlers existed but the hint hid them behind `F4 select` only.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        let mut surface = WorkspaceSurface::new();
        let shown = render_to_string(&mut surface, &app, 120, 30);
        assert!(
            shown.contains("PgUp/PgDn"),
            "PgUp/PgDn affordance missing from default hint:\n{shown}"
        );
        assert!(
            shown.contains("End"),
            "End-key affordance missing from default hint:\n{shown}"
        );
    }

    #[test]
    fn status_hint_when_scrolled_up_advertises_click_v0913() {
        // F19: when the user has scrolled up the hint pivots to advertise
        // the jump-to-latest paths (click the ↓ hint, or press End). The
        // ⇧Tab/rail hints are buried during scrollback to make room.
        let mut app = app_with_long_transcript(80);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 120, 30);
        // Scroll up to arm the sticky flag.
        surface.handle_mouse(
            mouse(ratatui::crossterm::event::MouseEventKind::ScrollUp),
            &mut app,
        );
        assert!(surface.user_has_scrolled_up);
        let scrolled = render_to_string(&mut surface, &app, 120, 30);
        assert!(
            scrolled.contains("End jump to latest"),
            "scrolled-up hint must advertise End key — got:\n{scrolled}"
        );
        assert!(
            scrolled.contains("click"),
            "scrolled-up hint must advertise click affordance — got:\n{scrolled}"
        );
    }

    #[test]
    fn home_jumps_to_top_when_composer_empty_v0913() {
        // F19: Home on an empty composer is the explicit "jump to top of
        // transcript" — symmetric with End. The scroll offset clamps to
        // the maximum legal value (`last_total_lines - last_text_area_height`).
        let mut app = app_with_long_transcript(80);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        assert_eq!(surface.transcript_scroll, 0, "starts at bottom");
        surface.handle_key(key(KeyCode::Home), &mut app);
        let max = surface
            .last_total_lines
            .saturating_sub(surface.last_text_area_height);
        assert_eq!(
            surface.transcript_scroll, max,
            "Home must clamp to the maximum legal scroll offset"
        );
        assert!(
            surface.user_has_scrolled_up,
            "Home must arm the sticky-up flag so a new turn does not yank away"
        );
    }

    #[test]
    fn home_does_not_steal_when_composer_has_text_v0913() {
        // F19: Home with typed text passes through to the composer's
        // tui-input handler so cursor-to-start still works in the input
        // field. Only an empty composer escalates Home to transcript jump.
        let mut app = app_with_long_transcript(80);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 100, 30);
        // Type some text into the composer.
        surface.composer = tui_input::Input::new("hello".to_string());
        let before = surface.transcript_scroll;
        surface.handle_key(key(KeyCode::Home), &mut app);
        assert_eq!(
            surface.transcript_scroll, before,
            "Home with typed text must NOT scroll transcript"
        );
    }

    #[test]
    fn jump_hint_rect_stored_when_user_scrolled_up_v0913() {
        // F19 + v0.9.1.3 K: the jump-to-latest hint is now ALWAYS
        // rendered (Gmail "scroll to bottom" pattern). The rect is
        // captured at every render so the mouse handler can hit-test a
        // click against it whether the user is at the bottom (grayed)
        // or scrolled-up (orange + bold). Pre-K the rect was None at
        // the bottom; K makes it Some in both states.
        let mut app = app_with_long_transcript(80);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 120, 30);
        let rect_at_bottom = surface
            .last_jump_hint_rect
            .expect("K: hint must paint even at bottom (grayed)");
        assert!(rect_at_bottom.width > 0 && rect_at_bottom.height == 1);
        surface.handle_mouse(
            mouse(ratatui::crossterm::event::MouseEventKind::ScrollUp),
            &mut app,
        );
        let _ = render_to_string(&mut surface, &app, 120, 30);
        let rect = surface
            .last_jump_hint_rect
            .expect("rect must be set while sticky-up");
        assert!(rect.width > 0 && rect.height == 1);
        // Snap back to the bottom — the rect stays Some (K) so a
        // click on the grayed hint can still snap to bottom.
        surface.jump_to_bottom();
        let _ = render_to_string(&mut surface, &app, 120, 30);
        assert!(
            surface.last_jump_hint_rect.is_some(),
            "K: rect stays present at bottom (grayed) — hint is always-visible"
        );
    }

    #[test]
    fn click_on_jump_hint_snaps_to_bottom_v0913() {
        // F19: a left-mouse click inside the stored jump-to-latest rect
        // must call jump_to_bottom. The xterm.js / Apple Terminal
        // affordance the dim "↓ jump to latest" hint advertises is
        // becoming a real button.
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let mut app = app_with_long_transcript(80);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 120, 30);
        // Arm sticky-up so the hint paints.
        surface.handle_mouse(mouse(MouseEventKind::ScrollUp), &mut app);
        let _ = render_to_string(&mut surface, &app, 120, 30);
        let rect = surface.last_jump_hint_rect.expect("hint must paint");

        // Click in the centre of the hint cell.
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: rect.x + rect.width / 2,
            row: rect.y,
            modifiers: KeyModifiers::NONE,
        };
        surface.handle_mouse(click, &mut app);
        assert_eq!(
            surface.transcript_scroll, 0,
            "click on jump hint must snap to bottom"
        );
        assert!(
            !surface.user_has_scrolled_up,
            "click on jump hint must clear the sticky-up flag"
        );
    }

    #[test]
    fn click_outside_jump_hint_does_not_jump_v0913() {
        // F19: clicks anywhere OTHER than the hint rect must be a no-op
        // (the surface is non-interactive by design today; only the
        // dedicated affordance is clickable).
        use ratatui::crossterm::event::{MouseButton, MouseEventKind};
        let mut app = app_with_long_transcript(80);
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 120, 30);
        surface.handle_mouse(mouse(MouseEventKind::ScrollUp), &mut app);
        let _ = render_to_string(&mut surface, &app, 120, 30);
        let before_scroll = surface.transcript_scroll;
        // Click at column 0, row 0 — top-left, far from the hint.
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 0,
            row: 0,
            modifiers: KeyModifiers::NONE,
        };
        surface.handle_mouse(click, &mut app);
        assert_eq!(
            surface.transcript_scroll, before_scroll,
            "click outside the hint rect must NOT change scroll"
        );
        assert!(
            surface.user_has_scrolled_up,
            "click outside must NOT clear the sticky-up flag"
        );
    }

    // ── v0.9.1.3 J — orange accent demotion ─────────────────────────

    /// Walk every span on every line and return the FIRST span whose
    /// text content contains `needle`. Used by the accent-demotion
    /// tests to look up specific glyphs without depending on positional
    /// line indices that drift across unrelated edits.
    fn find_span_by_content<'a>(
        lines: &'a [Line<'static>],
        needle: &str,
    ) -> Option<&'a Span<'static>> {
        for line in lines {
            for span in &line.spans {
                if span.content.contains(needle) {
                    return Some(span);
                }
            }
        }
        None
    }

    #[test]
    fn user_turn_bar_still_orange_v0914() {
        // J: the user-turn `▌` bar is one of the TWO surfaces that
        // retains brand orange (the other is the active-tab underline
        // — verified in widgets/header.rs). This pins that the bar
        // does NOT get demoted along with the rest of the accent budget.
        let mut lines: Vec<Line<'static>> = Vec::new();
        let turn = TurnView {
            role: TurnRole::User,
            elements: vec![TurnElement::Markdown("hi".into())],
        };
        let theme = Theme::hearth();
        let app = App::new();
        push_turn(&mut lines, &turn, &theme, &[], &app, 40, None, 0, false, 0);
        let bar = find_span_by_content(&lines, "▌").expect("user-turn `▌` span must be present");
        assert_eq!(
            bar.style.fg,
            Some(theme.orange),
            "J: user-turn `▌` MUST stay orange (1 of 2 retained accent surfaces)"
        );
    }

    #[test]
    fn tool_card_running_spinner_no_longer_orange_v0914() {
        // J: the running-spinner glyph (`◐◓◑◒`) was demoted from
        // `theme.orange` to `theme.text_dim` per the accent-inflation
        // audit. The animation already signals in-flight; orange was
        // redundant and consumed brand budget.
        use crate::tui::app::ToolCardModel;
        let mut lines: Vec<Line<'static>> = Vec::new();
        let theme = Theme::hearth();
        let app = App::new();
        let card = ToolCardModel {
            call_id: "c1".into(),
            tool_name: "Bash".into(),
            input_pretty: String::new(),
            summary: String::new(),
            output: None,
            edit_preview: None,
            approval_reason: String::new(),
            status: ToolCardStatus::Running,
            plan_body: None,
        };
        push_tool_card_lines(&mut lines, &card, &theme, true, &app, 80, None, 0, false);
        // The spinner glyph is one of `◐◓◑◒`; find whichever frame
        // landed and assert its fg.
        let spinner_glyphs = ["◐", "◓", "◑", "◒"];
        let span = spinner_glyphs
            .iter()
            .find_map(|g| find_span_by_content(&lines, g))
            .expect("spinner glyph span must be present on a Running card");
        assert_ne!(
            span.style.fg,
            Some(theme.orange),
            "J: running spinner must NOT be orange (demoted to neutral)"
        );
        assert_eq!(
            span.style.fg,
            Some(theme.text_dim),
            "J: running spinner must be `theme.text_dim`"
        );
    }

    // ── v0.9.1.3 K — jump-to-latest hint is always-visible ──────────

    #[test]
    fn jump_to_latest_hint_always_present_v0914() {
        // K: the `↓ jump to latest` hint is ALWAYS painted (Gmail
        // pattern). Pre-K it was only visible while the user had
        // scrolled up; K makes it persistent so the affordance is
        // discoverable BEFORE the user scrolls.
        let app = app_with_long_transcript(80);
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 120, 30);
        assert!(
            out.contains("↓ jump to latest"),
            "K: hint must be visible at bottom (grayed): not found in render"
        );
        assert!(
            surface.last_jump_hint_rect.is_some(),
            "K: hint rect must be stored even at the bottom (grayed)"
        );
    }

    #[test]
    fn jump_to_latest_hint_grayed_when_at_bottom_v0914() {
        // K: at the bottom the hint paints in `theme.text_muted`
        // (grayed) — no urgent signal, just the affordance.
        let app = app_with_long_transcript(80);
        let mut surface = WorkspaceSurface::new();
        let theme = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), &app, &theme))
            .expect("render workspace");
        let buf = terminal.backend().buffer();
        let rect = surface.last_jump_hint_rect.expect("K: hint must paint");
        // First non-space cell inside the rect is the `↓` glyph.
        let cell = (0..rect.width)
            .map(|dx| &buf[(rect.x + dx, rect.y)])
            .find(|c| c.symbol() != " ")
            .expect("hint glyph cell must exist");
        assert_eq!(
            cell.fg, theme.text_muted,
            "K: hint must paint in `theme.text_muted` when at bottom"
        );
    }

    #[test]
    fn jump_to_latest_hint_accent_when_scrolled_up_v0914() {
        // K: when scrolled-up the hint promotes to `theme.orange` + BOLD
        // — this IS one of the two retained orange surfaces (alongside
        // the user-turn `▌`), signaling "there's content below you
        // haven't reached".
        use ratatui::crossterm::event::MouseEventKind;
        let mut app = app_with_long_transcript(80);
        let mut surface = WorkspaceSurface::new();
        let theme = Theme::hearth();
        let _ = render_to_string(&mut surface, &app, 120, 30);
        // Arm sticky-up.
        surface.handle_mouse(mouse(MouseEventKind::ScrollUp), &mut app);
        let mut terminal = Terminal::new(TestBackend::new(120, 30)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), &app, &theme))
            .expect("render workspace");
        let buf = terminal.backend().buffer();
        let rect = surface.last_jump_hint_rect.expect("K: hint must paint");
        let cell = (0..rect.width)
            .map(|dx| &buf[(rect.x + dx, rect.y)])
            .find(|c| c.symbol() != " ")
            .expect("hint glyph cell must exist");
        assert_eq!(
            cell.fg, theme.orange,
            "K: hint must paint in `theme.orange` when scrolled up"
        );
        assert!(
            cell.modifier.contains(Modifier::BOLD),
            "K: hint must be BOLD when scrolled up"
        );
    }

    #[test]
    fn new_user_turn_auto_scrolls_to_bottom_v0913() {
        // F19 regression-pin: when the user submits a new message AFTER
        // having scrolled up, the submit path must reset transcript_scroll
        // to 0 + clear the sticky-up flag so the new prompt + reply land
        // in the viewport. The workspace key handler does this in the
        // KeyCode::Enter arm at workspace.rs:508-509; this test pins
        // that contract against regression.
        let mut app = app_with_long_transcript(80);
        // Set a model so the no-model banner doesn't trip the submit
        // path's empty-model guard.
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 120, 30);
        // Scroll up off the bottom.
        surface.handle_mouse(
            mouse(ratatui::crossterm::event::MouseEventKind::ScrollUp),
            &mut app,
        );
        assert!(surface.user_has_scrolled_up);
        assert!(surface.transcript_scroll > 0);
        // Type a message and press Enter — the workspace's Enter branch
        // is the auto-scroll-on-new-turn site.
        surface.composer = tui_input::Input::new("test message".to_string());
        let _ = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(
            surface.transcript_scroll, 0,
            "submit must reset transcript_scroll to 0"
        );
        assert!(
            !surface.user_has_scrolled_up,
            "submit must clear the sticky-up flag so the reply autoscrolls"
        );
    }

    // ── v0.9.1.2 polish 1D: F8 ArrowUp history recall ───────────────────

    #[test]
    fn arrow_up_on_empty_composer_recalls_previous_prompt_v0913() {
        // v0.9.1.2 F8: bare Up on an empty composer must pull the most
        // recently submitted prompt back into the composer, not scroll
        // the transcript. The status hint already advertised this; the
        // feature was missing until now.
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        app.recent_user_prompts
            .push_back("first prompt".to_string());
        app.recent_user_prompts
            .push_back("second prompt".to_string());
        app.recent_user_prompts
            .push_back("third prompt".to_string());

        let action = surface.handle_key(key(KeyCode::Up), &mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "Up on empty composer should be consumed locally, not emit an action"
        );
        assert_eq!(
            surface.composer.value(),
            "third prompt",
            "first Up should recall the most recent prompt"
        );
        assert_eq!(app.history_cursor, Some(2), "cursor at newest index");
    }

    #[test]
    fn arrow_up_walks_further_back_through_history_v0913() {
        // Repeated Up walks older through the ring; the cursor stops at
        // the oldest entry (no wrap-around) so a long history can be
        // browsed linearly without surprises.
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        app.recent_user_prompts.push_back("oldest".to_string());
        app.recent_user_prompts.push_back("middle".to_string());
        app.recent_user_prompts.push_back("newest".to_string());

        surface.handle_key(key(KeyCode::Up), &mut app); // → "newest"
        surface.composer = Input::default();
        surface.handle_key(key(KeyCode::Up), &mut app); // → "middle"
        assert_eq!(surface.composer.value(), "middle");
        assert_eq!(app.history_cursor, Some(1));
        surface.composer = Input::default();
        surface.handle_key(key(KeyCode::Up), &mut app); // → "oldest"
        assert_eq!(surface.composer.value(), "oldest");
        assert_eq!(app.history_cursor, Some(0));
        surface.composer = Input::default();
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(
            surface.composer.value(),
            "oldest",
            "Up at the oldest entry must stay there, not wrap"
        );
        assert_eq!(app.history_cursor, Some(0));
    }

    #[test]
    fn arrow_up_then_down_returns_to_empty_v0913() {
        // Down walks forward and falls off the newest entry by clearing
        // the composer + resetting the cursor — a clean escape from
        // history mode without retyping.
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        app.recent_user_prompts.push_back("only entry".to_string());

        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(surface.composer.value(), "only entry");
        assert_eq!(app.history_cursor, Some(0));
        let action = surface.handle_key(key(KeyCode::Down), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        assert_eq!(
            surface.composer.value(),
            "",
            "Down past the newest entry must clear the composer"
        );
        assert_eq!(
            app.history_cursor, None,
            "Down past the newest entry must exit history mode"
        );
    }

    #[test]
    fn arrow_up_in_non_empty_composer_does_not_recall_v0913() {
        // History recall only fires on an empty composer. A composer
        // with typed text needs ArrowUp for in-buffer cursor navigation;
        // hijacking it would clobber the in-progress prompt.
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        app.recent_user_prompts
            .push_back("would-be-recalled".to_string());
        surface.handle_key(key(KeyCode::Char('a')), &mut app);
        surface.handle_key(key(KeyCode::Char('b')), &mut app);
        surface.handle_key(key(KeyCode::Char('c')), &mut app);
        assert_eq!(surface.composer.value(), "abc");
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(
            surface.composer.value(),
            "abc",
            "Up on a non-empty composer must NOT recall history"
        );
        assert_eq!(
            app.history_cursor, None,
            "history cursor must remain None when recall did not fire"
        );
    }

    #[test]
    fn typing_after_history_recall_resets_cursor_v0913() {
        // Any typing exits history-recall mode so the next bare Up lands
        // back on the most recent entry. Without this the user gets
        // stuck stepping through a stale path in history every time
        // they amend a recalled prompt.
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        app.recent_user_prompts.push_back("recalled".to_string());
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(app.history_cursor, Some(0));
        surface.handle_key(key(KeyCode::Char('x')), &mut app);
        assert_eq!(
            app.history_cursor, None,
            "typing after recall must clear the history cursor"
        );
    }

    #[test]
    fn arrow_up_with_empty_history_still_scrolls_transcript_v0913() {
        // Cold-start — no prompt has been submitted yet, so ArrowUp
        // falls through to the v0.9.1 W1 A single-line transcript scroll.
        // The "browse with arrows" affordance still works on a brand-new
        // session with nothing to recall.
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        surface.last_text_area_height = 20;
        surface.last_total_lines = 40;
        assert!(
            app.recent_user_prompts.is_empty(),
            "history must be empty for this case to fire"
        );
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert!(
            surface.composer.value().is_empty(),
            "no history → composer must not be filled"
        );
        assert_eq!(
            app.history_cursor, None,
            "no history → cursor must stay None"
        );
    }

    // ── v0.9.2 W11-integ S21 — reasoning collapse toggle ──────────────────

    /// `Ctrl+R` flips `reasoning_expanded` for the most-recent assistant
    /// turn — the documented minimal-viable binding (no per-element focus
    /// model exists in this surface). The map stays keyed by turn index, so
    /// this is the same flip a focused-element activation would perform.
    #[test]
    fn ctrl_r_toggles_reasoning_for_most_recent_assistant_turn_v092() {
        use crate::tui::app::{TurnRole, TurnView};
        let mut app = App::new();
        // Two turns: a user turn (idx 0) and an assistant turn (idx 1). The
        // toggle must target the assistant turn, not the user turn.
        app.session.turns.push(TurnView::new(TurnRole::User));
        app.session.turns.push(TurnView::new(TurnRole::Assistant));
        let mut surface = WorkspaceSurface::new();

        // Default: absent ⇒ collapsed.
        assert!(!app.reasoning_expanded.get(&1).copied().unwrap_or(false));

        surface.handle_key(ctrl(KeyCode::Char('r')), &mut app);
        assert_eq!(
            app.reasoning_expanded.get(&1).copied(),
            Some(true),
            "Ctrl+R must expand the most-recent assistant turn (idx 1)"
        );
        // The user turn (idx 0) is never the target.
        assert_eq!(app.reasoning_expanded.get(&0).copied(), None);

        // A second press collapses it again (idempotent toggle).
        surface.handle_key(ctrl(KeyCode::Char('r')), &mut app);
        assert_eq!(app.reasoning_expanded.get(&1).copied(), Some(false));
    }

    #[test]
    fn ctrl_r_is_a_noop_with_no_assistant_turn_v092() {
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        // Empty transcript — no assistant turn to target.
        surface.handle_key(ctrl(KeyCode::Char('r')), &mut app);
        assert!(
            app.reasoning_expanded.is_empty(),
            "Ctrl+R with no assistant turn must not mutate the map"
        );
    }

    // ── v0.9.4 W3b — Tab+Enter focused-reasoning chord ───────────────────────

    /// Build a turn with the given role and a Thinking element.
    fn reasoning_turn(role: crate::tui::app::TurnRole) -> crate::tui::app::TurnView {
        let mut t = crate::tui::app::TurnView::new(role);
        t.elements
            .push(crate::tui::turn_element::TurnElement::Thinking {
                body: "some thought".into(),
                secs: 1,
                tokens: 10,
            });
        t
    }

    /// Build a plain text turn (no Thinking element).
    fn plain_turn(role: crate::tui::app::TurnRole) -> crate::tui::app::TurnView {
        let mut t = crate::tui::app::TurnView::new(role);
        t.elements
            .push(crate::tui::turn_element::TurnElement::Markdown("hi".into()));
        t
    }

    /// FIX-7 — a reasoning turn in the transcript must NOT make the Workspace
    /// claim Tab. The old behavior hijacked Tab whenever any Thinking block
    /// existed (with no visual focus indicator), silently breaking the
    /// documented "Tab next tab" after the first agent turn. `owns_tab` must
    /// now be false so the Router's global tab-switch fires.
    #[test]
    fn tab_does_not_get_stolen_by_a_reasoning_turn_fix7() {
        use crate::tui::app::TurnRole;
        let mut app = App::new();
        app.session.turns.push(plain_turn(TurnRole::User));
        app.session.turns.push(reasoning_turn(TurnRole::Assistant));

        let surface = WorkspaceSurface::new();
        assert!(
            !surface.owns_tab(&app),
            "a reasoning turn must not make the Workspace claim Tab (FIX-7)"
        );
    }

    /// Ctrl+R still toggles the most-recent assistant turn's reasoning when
    /// no reasoning focus is active — regression guard for the existing chord.
    #[test]
    fn ctrl_r_still_flips_most_recent_v094() {
        use crate::tui::app::TurnRole;
        let mut app = App::new();
        app.session.turns.push(plain_turn(TurnRole::User));
        app.session.turns.push(reasoning_turn(TurnRole::Assistant));

        let mut surface = WorkspaceSurface::new();

        // Ctrl+R must flip the most-recent assistant turn (idx 1).
        surface.handle_key(ctrl(KeyCode::Char('r')), &mut app);
        assert_eq!(
            app.reasoning_expanded.get(&1).copied(),
            Some(true),
            "Ctrl+R must expand the most-recent assistant turn (idx 1)"
        );

        // Second Ctrl+R collapses it.
        surface.handle_key(ctrl(KeyCode::Char('r')), &mut app);
        assert_eq!(app.reasoning_expanded.get(&1).copied(), Some(false));
    }

    /// Tab while the `@`-completion popup is open accepts the highlighted
    /// candidate — the one in-surface state that still owns Tab (FIX-7).
    #[test]
    fn tab_still_accepts_slash_completion_v094() {
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();

        // Type `@di` to open the @-completion popup.
        for c in "@di".chars() {
            surface.handle_key(key(KeyCode::Char(c)), &mut app);
        }
        assert!(
            surface.at_completion.is_some(),
            "typing `@di` must open the @-completion popup"
        );
        assert!(
            surface.owns_tab(&app),
            "an open @-completion popup must claim Tab"
        );

        // Tab must accept the completion.
        surface.handle_key(key(KeyCode::Tab), &mut app);

        assert!(
            surface.at_completion.is_none(),
            "Tab must close the @-completion popup"
        );
        assert_eq!(
            surface.composer.value(),
            "@diff",
            "Tab must accept the highlighted candidate"
        );
    }

    // ── v0.9.2 W11-integ #10 — AskUserQuestion arrow-nav + answer ─────────

    /// Seed a pending `AskUserQuestion` card with the given args JSON.
    fn push_ask_user_card(app: &mut App, call_id: &str, input_pretty: &str) {
        app.session.tool_cards.push(ToolCardModel {
            call_id: call_id.into(),
            tool_name: "AskUserQuestion".into(),
            summary: String::new(),
            status: ToolCardStatus::AwaitingApproval,
            output: None,
            edit_preview: None,
            input_pretty: input_pretty.into(),
            approval_reason: String::new(),
            plan_body: None,
        });
    }

    #[test]
    fn ask_user_arrow_keys_move_selection_index_v092() {
        let mut app = App::new();
        push_ask_user_card(
            &mut app,
            "c1",
            r#"{ "question": "Which?", "choices": ["Alpha", "Beta", "Gamma"] }"#,
        );
        let mut surface = WorkspaceSurface::new();
        assert_eq!(surface.approval_sel, 0, "starts on the first choice");

        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.approval_sel, 1, "Down moves to the second choice");
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.approval_sel, 2, "Down moves to the third choice");
        // Clamp at the last choice — no overflow past the choice count.
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(
            surface.approval_sel, 2,
            "selection clamps at the last choice"
        );

        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(surface.approval_sel, 1, "Up moves back");
    }

    #[test]
    fn ask_user_enter_approves_selected_choice_v092() {
        use wcore_protocol::commands::ApprovalScope;
        let mut app = App::new();
        push_ask_user_card(
            &mut app,
            "c1",
            r#"{ "question": "Which DB?", "choices": ["Postgres", "SQLite"] }"#,
        );
        let mut surface = WorkspaceSurface::new();
        // Move to the second choice, then answer.
        surface.handle_key(key(KeyCode::Down), &mut app);
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        // v0.9.3 W8 B1: picking a choice APPROVES the tool (scope Once) AND
        // carries the chosen label as `answer: Some(label)`. Orchestration's
        // synth arm at mod.rs:911 — guarded on tool_name == "AskUserQuestion"
        // — turns this into the tool result content directly, bypassing
        // execute()'s loud-defensive `is_error: true` fallback.
        match action {
            SurfaceAction::Approve {
                call_id,
                scope,
                answer,
            } => {
                assert_eq!(call_id, "c1");
                assert_eq!(
                    scope,
                    ApprovalScope::Once,
                    "AskUser answer approves once (no error-deny)"
                );
                assert_eq!(
                    answer.as_deref(),
                    Some("SQLite"),
                    "AskUser Enter must route the SELECTED choice as `answer`"
                );
            }
            other => panic!("expected Approve (not Deny) for the chosen answer, got {other:?}"),
        }
        // Selection resets for the next card.
        assert_eq!(surface.approval_sel, 0);
    }

    #[test]
    fn ask_user_does_not_consume_approval_hotkeys_v092() {
        // On an AskUser card, `y`/`a` are NOT approve actions — they are
        // ignored (only ↑/↓/Enter/Esc are meaningful). This proves the
        // arrow-nav branch is scoped to AskUserQuestion and does not leak
        // the approve/deny dance.
        let mut app = App::new();
        push_ask_user_card(&mut app, "c1", r#"{ "question": "q", "choices": ["A"] }"#);
        let mut surface = WorkspaceSurface::new();
        let action = surface.handle_key(key(KeyCode::Char('y')), &mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "y must be inert on an AskUser card, got {action:?}"
        );
    }

    #[test]
    fn ask_user_footer_shows_working_keys_not_yan_d040() {
        // D040: the AskUserQuestion card footer used to advertise [y][a][n],
        // but on an AskUser card only ↑/↓/Enter/Esc actually do anything
        // (proved by `ask_user_does_not_consume_approval_hotkeys_v092`). The
        // footer must show the keys that WORK, not the dead y/a/n triplet.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        push_ask_user_card(
            &mut app,
            "c1",
            r#"{ "question": "Which?", "choices": ["A", "B"] }"#,
        );
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 120, 30);

        // The arrow-nav / select / cancel keys (the ones honored in
        // `handle_approval_key`'s AskUser arm) are advertised.
        assert!(
            out.contains("↑/↓ move") && out.contains("⏎ select"),
            "AskUser footer must advertise arrow-nav + Enter — got:\n{out}"
        );
        // The dead approval triplet must NOT appear on an AskUser card.
        assert!(
            !out.contains("[y] approve"),
            "AskUser footer must NOT advertise the dead [y] approve key — got:\n{out}"
        );
        assert!(
            !out.contains("[n] deny"),
            "AskUser footer must NOT advertise the dead [n] deny key — got:\n{out}"
        );
    }

    #[test]
    fn normal_approval_footer_keeps_yan_d040() {
        // D040 guard: a generic tool-approval card (Write) keeps its real
        // y/a/n triplet — the AskUser-aware branch must not regress it.
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        push_awaiting_card(&mut app, "c1", "Write");
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 120, 30);

        assert!(
            out.contains("[y] approve") && out.contains("[n] deny"),
            "a generic approval card must keep its y/a/n footer — got:\n{out}"
        );
        assert!(
            !out.contains("↑/↓ move"),
            "a generic approval card must NOT show the AskUser arrow footer — got:\n{out}"
        );
    }

    // ---------------------------------------------------------------
    // v0.9.3 W7.1: ⌥A / Ctrl+] / 'å' open the agent list (3 keybind
    // paths for cross-terminal compat — pure-Alt-A is dropped by
    // Terminal.app, which sends the composed character `å`; iTerm2 +
    // Kitty send Alt+a properly; Ctrl+] is the universal fallback).
    // Pushing the workspace onto `App::surface_stack` lets `Esc` on the
    // AgentNav surface pop straight back here with `transcript_scroll`
    // restored (per the S0.6 contract).
    // ---------------------------------------------------------------

    #[test]
    fn alt_a_pushes_workspace_and_switches_to_agent_nav_v093() {
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        // Pretend the user scrolled to row 7 in the workspace.
        surface.transcript_scroll = 7;
        let key_ev = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::ALT);
        let action = surface.handle_key(key_ev, &mut app);
        assert!(
            matches!(action, SurfaceAction::Switch(SurfaceId::AgentNav)),
            "Alt+A must switch to AgentNav, got {action:?}"
        );
        assert_eq!(app.surface_stack.len(), 1, "workspace must be pushed");
        let entry = &app.surface_stack[0];
        assert_eq!(entry.id, SurfaceId::Workspace);
        assert_eq!(entry.scroll_offset, 7);
    }

    #[test]
    fn ctrl_bracket_close_also_opens_agent_nav_v093() {
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        let key_ev = KeyEvent::new(KeyCode::Char(']'), KeyModifiers::CONTROL);
        let action = surface.handle_key(key_ev, &mut app);
        assert!(
            matches!(action, SurfaceAction::Switch(SurfaceId::AgentNav)),
            "Ctrl+] must switch to AgentNav, got {action:?}"
        );
        assert_eq!(app.surface_stack.len(), 1);
        assert_eq!(app.surface_stack[0].id, SurfaceId::Workspace);
    }

    #[test]
    fn alt_a_composed_a_ring_for_mac_terminal_v093() {
        // macOS Terminal.app sends 'å' (U+00E5) for Option+A by default.
        let mut app = App::new();
        let mut surface = WorkspaceSurface::new();
        let key_ev = KeyEvent::new(KeyCode::Char('å'), KeyModifiers::NONE);
        let action = surface.handle_key(key_ev, &mut app);
        assert!(
            matches!(action, SurfaceAction::Switch(SurfaceId::AgentNav)),
            "'å' must switch to AgentNav, got {action:?}"
        );
        assert_eq!(app.surface_stack.len(), 1);
    }

    #[test]
    fn workspace_restore_scroll_sets_offset_and_unsticks_v093_w8_h3() {
        // v0.9.3 W8 H3-integration: WorkspaceSurface now implements
        // Surface::restore_scroll so the scroll_offset captured into
        // SurfaceStackEntry on push (workspace.rs:529) is actually consumed
        // on Pop. Before this fix the default no-op trait impl was used;
        // behaviour was correct only because SurfaceCache parked the full
        // struct. If the cache ever drops a slot, Pop would silently lose
        // the scroll. This regression test pins the explicit contract.
        let mut surface = WorkspaceSurface::new();
        // Sticky-bottom flag starts unset for a new surface.
        assert!(!surface.user_has_scrolled_up);
        surface.transcript_scroll = 0;

        // Drive restore_scroll directly via the Surface trait.
        Surface::restore_scroll(&mut surface, 11);

        assert_eq!(
            surface.transcript_scroll, 11,
            "restore_scroll must overwrite transcript_scroll with the captured offset"
        );
        assert!(
            surface.user_has_scrolled_up,
            "non-zero restore_scroll must mark user_has_scrolled_up so sticky-bottom \
             auto-scroll doesn't snap the user back to the latest line"
        );
    }

    #[test]
    fn workspace_restore_scroll_zero_clears_sticky_flag_v093_w8_h3() {
        // Symmetric guard: restore_scroll(0) means "back at the top of the
        // transcript" — keep user_has_scrolled_up = false so live tail
        // resumes correctly. Without this branch, a Pop back to a fully
        // scrolled-up Workspace at offset 0 would set the sticky flag and
        // the user would have to explicitly press End to re-tail.
        let mut surface = WorkspaceSurface::new();
        surface.user_has_scrolled_up = true;
        surface.transcript_scroll = 9;

        Surface::restore_scroll(&mut surface, 0);

        assert_eq!(surface.transcript_scroll, 0);
        assert!(
            !surface.user_has_scrolled_up,
            "restore_scroll(0) means top-of-transcript; sticky-bottom flag must clear"
        );
    }

    // ── D010 (huge-paste stall) — composer cap + cached line count ────────

    #[test]
    fn huge_paste_is_truncated_at_the_cap_and_flagged() {
        // A paste larger than MAX_COMPOSER_BYTES must be truncated (not
        // absorbed whole) and the clip flagged so the cap is never silent.
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        let payload = "x".repeat(MAX_COMPOSER_BYTES + 50_000);
        surface.handle_paste(payload, &mut app);

        assert!(
            surface.composer.value().len() <= MAX_COMPOSER_BYTES,
            "composer buffer must be capped at MAX_COMPOSER_BYTES; got {} bytes",
            surface.composer.value().len()
        );
        assert!(
            surface.paste_was_capped,
            "an over-cap paste must set the paste_was_capped flag"
        );
    }

    #[test]
    fn huge_paste_truncates_on_a_utf8_char_boundary() {
        // The cap must never split a multi-byte codepoint — `value()` must
        // stay valid UTF-8 (it is `&str`, so an invalid cut would have
        // panicked inside handle_paste). Use 2-byte chars straddling the cap.
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        // 'é' is 2 bytes; this overshoots the cap so the boundary walk runs.
        let payload = "é".repeat(MAX_COMPOSER_BYTES);
        surface.handle_paste(payload, &mut app);
        assert!(surface.composer.value().len() <= MAX_COMPOSER_BYTES);
        assert!(surface.paste_was_capped);
    }

    #[test]
    fn under_cap_paste_is_kept_whole_and_not_flagged() {
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        surface.handle_paste("a small paste".to_string(), &mut app);
        assert_eq!(surface.composer.value(), "a small paste");
        assert!(
            !surface.paste_was_capped,
            "an under-cap paste must NOT set the clip flag"
        );
    }

    #[test]
    fn composer_line_count_cache_tracks_edits_without_per_frame_rescan() {
        // The cached count must equal a fresh scan after each mutation path:
        // keystroke, paste, and reset. This is the value the per-frame render
        // reads instead of re-scanning the buffer (D010).
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        assert_eq!(surface.composer_lines, 1, "empty composer is one line");

        // Keystroke path.
        surface.handle_key(key(KeyCode::Char('h')), &mut app);
        surface.handle_key(key(KeyCode::Char('i')), &mut app);
        assert_eq!(
            surface.composer_lines as usize,
            surface.composer.value().lines().count().max(1),
            "cached count must match a fresh scan after keystrokes"
        );

        // Paste path with embedded newlines.
        surface.handle_paste("\nline2\nline3".to_string(), &mut app);
        assert_eq!(
            surface.composer_lines as usize,
            surface.composer.value().lines().count().max(1),
            "cached count must match a fresh scan after a multi-line paste"
        );
        assert!(
            surface.composer_lines >= 3,
            "the cached count must reflect the pasted newlines"
        );
    }

    #[test]
    fn capped_paste_raises_a_clip_toast_not_a_silent_drop() {
        // An over-cap paste must surface a toast (the status bar renders
        // `app.toast` for a short dwell — its render is covered by the
        // status-bar tests) so the clip is never silent, and must NOT raise
        // one when the paste fits.
        let mut surface = WorkspaceSurface::new();
        let mut app = App::new();
        surface.handle_paste("x".repeat(MAX_COMPOSER_BYTES + 1), &mut app);
        let toast = app
            .toast
            .clone()
            .expect("an over-cap paste must set a toast");
        assert!(
            toast.to_lowercase().contains("clip"),
            "the toast must tell the user the paste was clipped; got {toast:?}"
        );
        assert!(
            app.toast_at.is_some(),
            "the toast must carry a timestamp for dwell"
        );

        // A fitting paste raises no toast.
        let mut surface2 = WorkspaceSurface::new();
        let mut app2 = App::new();
        surface2.handle_paste("small".to_string(), &mut app2);
        assert!(app2.toast.is_none(), "an under-cap paste must not toast");
    }

    // ── D009 (render-livelock) — transcript layout cache ──────────────────

    #[test]
    fn transcript_layout_cache_reuses_wrapped_count_when_content_unchanged() {
        // Two renders with NO content/width/theme change must reuse the cached
        // wrapped lines (the signature is unchanged), so the expensive
        // rebuild + re-wrap is skipped on the second frame.
        let mut app = app_from_fixture(fixtures::full_conversation());
        let mut surface = WorkspaceSurface::new();

        // Helper: snapshot the Copy parts of the cache (the wrapped `Vec` is
        // not `Copy`, so we read sig + count through a shared borrow).
        let snapshot = |s: &WorkspaceSurface, what: &str| -> (TranscriptSig, u16) {
            let c = s.transcript_layout.as_ref().expect(what);
            (c.sig, c.wrapped_total)
        };

        let _ = render_to_string(&mut surface, &app, 80, 24);
        let (first_sig, first_total) =
            snapshot(&surface, "first render must populate the layout cache");

        let _ = render_to_string(&mut surface, &app, 80, 24);
        let (second_sig, second_total) =
            snapshot(&surface, "second render keeps the cache populated");

        assert_eq!(
            first_sig, second_sig,
            "an unchanged transcript must keep the same cache signature"
        );
        assert_eq!(
            first_total, second_total,
            "the cached wrapped count must be reused, not recomputed differently"
        );

        // A width change must invalidate the signature (force a fresh wrap).
        let _ = render_to_string(&mut surface, &app, 40, 24);
        let (narrower_sig, _) = snapshot(&surface, "cache present after resize");
        assert_ne!(
            narrower_sig.width, first_sig.width,
            "a width change must change the cache signature so the wrap recomputes"
        );

        // A new turn must invalidate the signature too.
        apply_event(
            &mut app,
            wcore_protocol::events::ProtocolEvent::Info {
                msg_id: "test".to_string(),
                message: "another turn".to_string(),
            },
        );
        let _ = render_to_string(&mut surface, &app, 40, 24);
        let (after_turn_sig, _) = snapshot(&surface, "cache present after new turn");
        assert_ne!(
            after_turn_sig.turns, narrower_sig.turns,
            "a new turn must change the cache signature"
        );
    }

    // ── D009 (render-livelock) — viewport windowing ───────────────────────

    #[test]
    fn transcript_window_materializes_viewport_bounded_lines_for_a_huge_transcript() {
        // The windowing fix: a keystroke under a very large transcript must
        // window only the visible rows, NOT re-walk the whole transcript. We
        // assert the cached wrapped layout holds the FULL transcript (so
        // scroll math stays correct), while the per-frame WINDOW materialized
        // by `render_transcript_window` is bounded to ~viewport height — the
        // O(viewport) property that breaks the livelock.
        let width: u16 = 80;
        let height: u16 = 24;

        // Build a large single assistant turn (mirrors the smoke_p0 gap_d009
        // shape: one ~100KB markdown blob).
        let big = "lorem ipsum dolor sit amet ".repeat(4000);
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(big)],
        });

        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, width, height);

        let total = surface
            .transcript_layout
            .as_ref()
            .expect("a large transcript must populate the cache")
            .wrapped_total;
        assert!(
            total > height * 4,
            "the cached layout must hold the FULL wrapped transcript (got {total} rows) \
             so scroll math is correct"
        );

        // The window the render materializes must be viewport-bounded, NOT
        // O(transcript). Re-derive what `render_transcript_window` slices for
        // the bottom-anchored frame and assert its size is ~viewport.
        let wrapped = &surface.transcript_layout.as_ref().unwrap().lines;
        let window = window_slice_len(wrapped.len(), height, 0);
        assert!(
            window <= height as usize + 4,
            "per-frame window must be viewport-bounded (got {window} rows for a \
             {total}-row transcript)"
        );

        // Scroll must still reach arbitrary offsets across the full transcript.
        // The bottom anchor is `total - height`; a near-top scroll must land a
        // window near the START of the transcript.
        let near_top = total.saturating_sub(height);
        let top_window_start = window_slice_start(wrapped.len(), height, near_top);
        assert_eq!(
            top_window_start, 0,
            "scrolling to the maximum upward offset must window the very top \
             of the transcript"
        );
        // A mid scroll lands a window strictly between top and bottom.
        let mid = near_top / 2;
        let mid_start = window_slice_start(wrapped.len(), height, mid);
        assert!(
            mid_start > 0 && mid_start < wrapped.len().saturating_sub(height as usize),
            "a mid-range scroll must window the middle of the transcript \
             (start={mid_start}, total={total})"
        );
    }

    /// Test mirror of `render_transcript_window`'s window math: the START
    /// index of the materialized slice for a given scroll offset.
    fn window_slice_start(total_rows: usize, height: u16, scroll_offset: u16) -> usize {
        const OVERSCAN: usize = 2;
        let total = total_rows.min(u16::MAX as usize) as u16;
        let bottom_anchor = total.saturating_sub(height);
        let upward = scroll_offset.min(bottom_anchor);
        let top_row = bottom_anchor.saturating_sub(upward) as usize;
        top_row.saturating_sub(OVERSCAN)
    }

    /// Test mirror of `render_transcript_window`'s window math: the LENGTH of
    /// the materialized slice for a given scroll offset.
    fn window_slice_len(total_rows: usize, height: u16, scroll_offset: u16) -> usize {
        const OVERSCAN: usize = 2;
        let total = total_rows.min(u16::MAX as usize) as u16;
        let bottom_anchor = total.saturating_sub(height);
        let upward = scroll_offset.min(bottom_anchor);
        let top_row = bottom_anchor.saturating_sub(upward) as usize;
        let start = top_row.saturating_sub(OVERSCAN);
        let end = (top_row + height as usize).min(total_rows);
        end - start
    }

    #[test]
    fn transcript_window_keystroke_frame_is_a_cache_hit_not_a_rebuild() {
        // After a large transcript settles (no live stream), a composer
        // keystroke must NOT change the transcript signature — so the render
        // reuses the cached wrapped lines and only windows the viewport. This
        // is the exact path the smoke_p0 gap_d009 latency contract exercises.
        let big = "lorem ipsum dolor sit amet ".repeat(4000);
        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(big)],
        });

        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, 80, 24);
        let sig_before = surface.transcript_layout.as_ref().unwrap().sig;

        // Type a sentinel into the composer — a non-transcript edit.
        surface.handle_key(key(KeyCode::Char('Z')), &mut app);
        let _ = render_to_string(&mut surface, &app, 80, 24);
        let sig_after = surface.transcript_layout.as_ref().unwrap().sig;

        assert_eq!(
            sig_before, sig_after,
            "a composer keystroke must NOT invalidate the transcript cache \
             (otherwise the keystroke re-walks the whole transcript — the livelock)"
        );
    }

    // ── D009 (render-livelock) — LIVE-turn streaming windowing ────────────

    #[test]
    fn streaming_tail_offset_is_viewport_bounded_for_a_huge_live_buffer() {
        // The live-turn fix: while a turn is streaming a huge body, the per-frame
        // wrap must touch only the VISIBLE TAIL of the streaming buffer, not the
        // whole turn. `streaming_visible_tail_offset` returns where that tail
        // starts; the tail length (chars after the offset) must stay bounded to
        // the viewport budget regardless of how large the buffer grows.
        let width: u16 = 80;
        let height: u16 = 24;

        // ~108KB live buffer (mirrors smoke_p0 gap_d009).
        let big = "lorem ipsum dolor sit amet\n".repeat(4000);
        let off = streaming_visible_tail_offset(&big, width, height);
        let tail_chars = big[off..].chars().count();

        // Budget = (height + overscan) * width. The tail must not exceed it by
        // more than one line's worth (the line-boundary snap can add a partial
        // line back), and it must be VASTLY smaller than the full buffer.
        let budget = (height as usize + 4) * width as usize;
        assert!(
            tail_chars <= budget + width as usize,
            "live-turn tail must stay viewport-bounded (got {tail_chars} chars, \
             budget {budget}) so the per-frame wrap is O(viewport), not O(turn)"
        );
        assert!(
            tail_chars < big.chars().count() / 10,
            "the tail must be a small fraction of the {}-char live buffer, \
             not the whole turn (got {tail_chars})",
            big.chars().count()
        );

        // The snap lands on a line boundary so markdown renders whole lines:
        // the offset is either 0 or immediately follows a newline.
        assert!(
            off == 0 || big.as_bytes()[off - 1] == b'\n',
            "the tail offset must snap to a line boundary (got {off})"
        );
    }

    #[test]
    fn streaming_tail_offset_returns_zero_when_buffer_fits() {
        // A short live buffer fits the viewport: no windowing, offset 0, the
        // whole buffer renders (no content dropped for small streams).
        let width: u16 = 80;
        let height: u16 = 24;
        let small = "hello world\nthis is a short stream\n".to_string();
        assert_eq!(
            streaming_visible_tail_offset(&small, width, height),
            0,
            "a buffer that fits the viewport must not be windowed"
        );
    }

    #[test]
    fn live_turn_render_stays_a_settled_cache_hit_across_a_streaming_tick() {
        // The root-cause regression guard: while a turn streams a large body,
        // the SETTLED transcript (a prior completed turn) must remain a stable
        // cache HIT across animation ticks — the streaming tick must NOT flip
        // the settled signature and re-wrap the whole transcript. Only the
        // bounded live tail is rebuilt per frame.
        let width: u16 = 80;
        let height: u16 = 24;

        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        // A settled prior turn above the live stream.
        app.session.turns.push(crate::tui::app::TurnView {
            role: TurnRole::Assistant,
            elements: vec![crate::tui::turn_element::TurnElement::Markdown(
                "a settled prior answer".to_string(),
            )],
        });
        // A huge IN-FLIGHT streaming turn (not yet a settled turn).
        app.session.streaming = "lorem ipsum dolor sit amet\n".repeat(4000);
        app.session.streaming_active = true;

        let mut surface = WorkspaceSurface::new();
        let _ = render_to_string(&mut surface, &app, width, height);
        let sig_before = surface
            .transcript_layout
            .as_ref()
            .expect("first streaming frame must populate the settled cache")
            .sig;
        let total_before = surface.transcript_layout.as_ref().unwrap().wrapped_total;

        // Advance the animation tick AND grow the streaming buffer (a new chunk),
        // exactly as a live stream does every frame.
        app.frame_tick = app.frame_tick.wrapping_add(1);
        app.session
            .streaming
            .push_str("more streamed text arrives here\n");
        let _ = render_to_string(&mut surface, &app, width, height);
        let sig_after = surface.transcript_layout.as_ref().unwrap().sig;
        let total_after = surface.transcript_layout.as_ref().unwrap().wrapped_total;

        assert_eq!(
            sig_before, sig_after,
            "a streaming tick + chunk must NOT change the SETTLED cache signature \
             (otherwise every frame re-wraps the whole live turn — the livelock)"
        );
        assert_eq!(
            total_before, total_after,
            "the cached SETTLED wrapped count must be reused across a streaming \
             tick — the settled turns did not change"
        );

        // And the streaming tick must NOT have been baked into the settled sig.
        assert_eq!(
            sig_after.streaming_tick, 0,
            "the windowed live-stream path must keep the settled sig tick-free"
        );
        assert_eq!(
            sig_after.streaming_len, 0,
            "the settled cache must exclude the live streaming buffer length"
        );
    }

    #[test]
    fn live_turn_per_frame_materialized_lines_stay_viewport_bounded() {
        // The O(viewport) property for the LIVE turn: the per-frame work is the
        // wrap of the streaming buffer's visible TAIL plus the settled cache,
        // never the whole live turn. We assert the wrapped live tail the render
        // builds each frame is viewport-bounded regardless of the live turn's
        // size — the smoke_p0 gap_d009 contract at the unit level.
        let width: u16 = 80;
        let height: u16 = 24;

        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();
        app.session.streaming = "lorem ipsum dolor sit amet\n".repeat(4000); // ~108KB
        app.session.streaming_active = true;

        // Re-derive exactly what the render's windowed path wraps each frame.
        let theme = Theme::hearth();
        let tail_off = streaming_visible_tail_offset(&app.session.streaming, width, height);
        let mut live_logical: Vec<Line<'static>> = Vec::new();
        build_live_tail_lines(
            &mut live_logical,
            &app,
            &theme,
            &app.session.streaming[tail_off..],
        );
        let live_wrapped = wrap_lines_to_width(live_logical, width);

        // The full live turn wraps to thousands of rows; the per-frame live tail
        // must be a small multiple of the viewport, NOT the full turn.
        let full_turn_rows = app.session.streaming.lines().count();
        assert!(
            full_turn_rows > (height as usize) * 10,
            "sanity: the full live turn must dwarf the viewport (got {full_turn_rows} rows)"
        );
        assert!(
            live_wrapped.len() <= (height as usize + 4) * 2,
            "the per-frame live-tail wrap must be viewport-bounded (got {} rows for a \
             {full_turn_rows}-line live turn) — O(viewport), not O(turn)",
            live_wrapped.len()
        );

        // And a full end-to-end render of the same huge live stream must still
        // paint (no panic, no unbounded materialization) and show the tail text.
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, width, height);
        assert!(
            out.contains("lorem ipsum"),
            "the live stream's visible tail must render:\n{out}"
        );
    }

    /// D037 — after a turn that edited a file, the transcript must show a
    /// reviewable "Files changed" card listing the touched path, so the
    /// deliverable is a unit and not reconstructed from prose. Drives the full
    /// event stream (StreamStart → Edit tool → text → StreamEnd) through the
    /// REAL bridge, then asserts the RENDERED transcript via the surface.
    #[test]
    fn files_changed_card_lists_touched_paths_after_an_edit_turn_d037() {
        use wcore_protocol::events::{
            FinishReason, OutputType, ProtocolEvent, ToolCategory, ToolInfo, ToolStatus,
        };

        let events = vec![
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "call-edit".into(),
                tool: ToolInfo {
                    name: "Edit".into(),
                    category: ToolCategory::Edit,
                    args: serde_json::json!({
                        "file_path": "crates/wcore-cli/src/main.rs",
                        "old_string": "fn main() {}",
                        "new_string": "fn main() {\n    run();\n}",
                    }),
                    description: "Edit crates/wcore-cli/src/main.rs".into(),
                },
            },
            ProtocolEvent::ToolResult {
                msg_id: "m1".into(),
                call_id: "call-edit".into(),
                tool_name: "Edit".into(),
                status: ToolStatus::Success,
                output: "Edited crates/wcore-cli/src/main.rs".into(),
                output_type: OutputType::Diff,
                metadata: None,
            },
            ProtocolEvent::TextDelta {
                text: "Done — updated main.".into(),
                msg_id: "m1".into(),
            },
            ProtocolEvent::StreamEnd {
                msg_id: "m1".into(),
                finish_reason: FinishReason::Stop,
                usage: None,
                agent_run_id: None,
            },
        ];

        let app = app_from_fixture(events);
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 100, 30);

        assert!(
            out.contains("Files changed"),
            "the transcript must carry a 'Files changed' card after an edit turn:\n{out}"
        );
        assert!(
            out.contains("crates/wcore-cli/src/main.rs"),
            "the 'Files changed' card must list the touched path:\n{out}"
        );
    }

    /// D037 — a chat-only turn (no file-touching tool) must NOT render a
    /// "Files changed" card, so the affordance only appears when there is a
    /// real deliverable to review.
    #[test]
    fn no_files_changed_card_on_a_chat_only_turn_d037() {
        let app = app_from_fixture(fixtures::full_conversation());
        let mut surface = WorkspaceSurface::new();
        let out = render_to_string(&mut surface, &app, 100, 30);
        assert!(
            !out.contains("Files changed"),
            "a chat-only turn must not show a 'Files changed' card:\n{out}"
        );
    }

    /// Wave 6 B4 (audit #6) — the settled-cache invalidation contract. A
    /// `ToolResult` that flips a card Running→Ok and sets its output MUST be
    /// reflected in the RENDERED transcript on the very next frame, MID-STREAM
    /// (no StreamEnd in between). The render is driven through the REAL bridge
    /// (`apply_event`) and asserted on painted text — this is the
    /// phantom-affordance class the project tests for: before the
    /// `tool_cards_fp` fold, the settled signature was unchanged across the
    /// in-place status/output mutation, so the cache HIT and the card stayed
    /// frozen on "running…".
    #[test]
    fn tool_card_flips_running_to_done_in_the_rendered_transcript_mid_stream_w6b4() {
        use wcore_protocol::events::{
            OutputType, ProtocolEvent, ToolCategory, ToolInfo, ToolStatus,
        };

        let mut app = App::new();
        app.config.model = "anthropic/claude-opus-4-5".to_string();

        // A turn starts and requests a tool. The agent KEEPS streaming after the
        // request (streaming_active stays true), so the card lives in the settled
        // cache path — exactly the buggy window.
        apply_event(
            &mut app,
            ProtocolEvent::StreamStart {
                msg_id: "m1".into(),
            },
        );
        apply_event(
            &mut app,
            ProtocolEvent::ToolRequest {
                msg_id: "m1".into(),
                call_id: "call-grep".into(),
                tool: ToolInfo {
                    name: "Grep".into(),
                    category: ToolCategory::Info,
                    args: serde_json::json!({ "pattern": "needle" }),
                    description: "Grep for needle".into(),
                },
            },
        );

        // Frame 1: render with the card Running. This POPULATES the settled
        // cache with the card's `running…` spinner row. We assert on the
        // card-specific `running…` (the ellipsis form the CARD paints, line
        // ~3157) and the `· done` chip — NOT the bare verb the live
        // streaming-status widget animates ("running Bash" etc., no ellipsis),
        // which would otherwise pollute the match while the turn is still live.
        let mut surface = WorkspaceSurface::new();
        let out_running = render_to_string(&mut surface, &app, 100, 30);
        assert!(
            out_running.contains("Grep"),
            "frame 1 must render the tool card header:\n{out_running}"
        );
        assert!(
            out_running.contains("running…"),
            "frame 1 must show the tool card mid-flight as running…:\n{out_running}"
        );
        assert!(
            !out_running.contains("done"),
            "frame 1 must NOT yet show the card chip as done:\n{out_running}"
        );

        // The tool finishes — status flips Running→Ok and output is set IN PLACE
        // on the existing card. The turn is STILL streaming (no StreamEnd), so
        // `turns`, `last_turn_elements`, and `tool_cards` (the count) are all
        // unchanged; only the card's status + output changed. Pre-fix this left
        // the settled signature identical → cache HIT → frozen "running…".
        apply_event(
            &mut app,
            ProtocolEvent::ToolResult {
                msg_id: "m1".into(),
                call_id: "call-grep".into(),
                tool_name: "Grep".into(),
                status: ToolStatus::Success,
                output: "3 matches found".into(),
                output_type: OutputType::Text,
                metadata: None,
            },
        );

        // Frame 2: re-render WITHOUT any other event. The card must now paint as
        // done — the `tool_cards_fp` fold flipped the settled signature, so the
        // cache invalidated and the card repainted.
        let out_done = render_to_string(&mut surface, &app, 100, 30);
        // The `· done` chip is rendered ONLY by a completed tool card (the live
        // streaming-status verb pool is calling/running/wrapping up/thinking —
        // never "done"). Pre-fix, the settled cache HIT kept the old wrapped
        // lines with the `running…` spinner and never painted `done`, so this
        // assertion is the precise regression guard for the cache invalidation.
        assert!(
            out_done.contains("done"),
            "the completed tool card must flip to 'done' in the RENDERED transcript \
             on the next frame (settled-cache invalidation):\n{out_done}"
        );
        assert!(
            out_done.contains("Grep"),
            "the tool card must still render its header after the flip:\n{out_done}"
        );
    }

    /// Wave 6 B4 (audit #6) — direct unit proof that the `tool_cards_fp` fold
    /// flips when a card's status / output changes in place, and stays stable
    /// when nothing about the cards changes. This is the cheap O(cards)
    /// signal the render cache keys on.
    #[test]
    fn tool_cards_fingerprint_flips_on_status_and_output_change_w6b4() {
        let mut cards = vec![crate::tui::app::ToolCardModel {
            call_id: "c1".into(),
            tool_name: "Grep".into(),
            summary: String::new(),
            status: ToolCardStatus::Running,
            output: None,
            edit_preview: None,
            input_pretty: String::new(),
            approval_reason: String::new(),
            plan_body: None,
        }];
        let fp_running = tool_cards_fingerprint(&cards);

        // Recomputing over the same cards is stable (no spurious invalidation).
        assert_eq!(
            fp_running,
            tool_cards_fingerprint(&cards),
            "the fingerprint must be stable when the cards are unchanged"
        );

        // Status flip Running→Ok must change the fold.
        cards[0].status = ToolCardStatus::Ok;
        let fp_ok = tool_cards_fingerprint(&cards);
        assert_ne!(
            fp_running, fp_ok,
            "a Running→Ok status flip must change the fingerprint"
        );

        // Setting output (None→Some) must change the fold again.
        cards[0].output = Some("3 matches found".into());
        let fp_out = tool_cards_fingerprint(&cards);
        assert_ne!(
            fp_ok, fp_out,
            "setting a card's output must change the fingerprint"
        );
    }

    /// Wave 6 B4 (audit #17 / #20) — the rewritten char-cap must produce the
    /// SAME visible tail offset as the old forward `chars().count()` +
    /// `char_indices().nth(skip)` walk, including the line-boundary snap, while
    /// staying O(viewport). We assert the OUTPUT contract directly: the tail is
    /// bounded, snaps to a line boundary, and (for the no-newline pathological
    /// case the char cap defends) still windows down to the budget.
    #[test]
    fn streaming_tail_offset_matches_legacy_result_and_handles_no_newline_w6b4() {
        let width: u16 = 80;
        let height: u16 = 24;
        let budget = (height as usize + 4) * width as usize;

        // Newline-rich buffer: the offset must snap to a line boundary and keep
        // only the tail.
        let prose = "lorem ipsum dolor sit amet\n".repeat(4000);
        let off = streaming_visible_tail_offset(&prose, width, height);
        assert!(
            off == 0 || prose.as_bytes()[off - 1] == b'\n',
            "newline buffer tail must snap to a line boundary (got {off})"
        );
        assert!(
            prose[off..].chars().count() <= budget + width as usize,
            "newline buffer tail must stay viewport-bounded"
        );

        // Pathological single line, NO newline at all — the line cap keeps the
        // whole buffer, so the CHAR cap must window it down to ~budget chars.
        // Pre-rewrite this branch ran `buffer.chars().count()` over the whole
        // thing; the result tail must still be bounded.
        let one_line = "x".repeat(budget * 4);
        let off2 = streaming_visible_tail_offset(&one_line, width, height);
        let tail2 = one_line[off2..].chars().count();
        assert!(
            tail2 <= budget,
            "a no-newline over-wide line must be char-capped to the budget \
             (got {tail2} chars, budget {budget})"
        );
        assert!(
            off2 > 0,
            "a buffer far larger than the budget must be windowed (offset > 0)"
        );

        // A buffer that fits the budget is never windowed.
        let small = "hello\nworld\n".to_string();
        assert_eq!(
            streaming_visible_tail_offset(&small, width, height),
            0,
            "a buffer within the viewport budget must not be windowed"
        );
    }
}
