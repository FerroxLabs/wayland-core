//! v0.9.3 W4 — agent list / nav surface.
//!
//! Full-screen sub-agent navigator. Layout:
//!   row 0   header  : `Sub-Agents   <topology>   <N> agents · <M> running · <D> done · <F> failed`
//!   rows 1+ body    : flat list (≤10 agents) OR collapsible groups (Running/Done/Failed)
//!   row N-1 footer  : keybind hint (`↑↓ select   ⏎ open   ⎋ workspace`)
//!
//! Selection is stored by `agent_id` (FROZEN SubAgentView::id), NOT row index,
//! so a status change that moves an agent across groups keeps the highlight
//! anchored to the same agent (SPEC §1C).

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::app::{App, SubAgentStatus, SubAgentView};
use crate::tui::surfaces::{Surface, SurfaceAction, SurfaceId, SurfaceStackEntry};
use crate::tui::theme::Theme;

/// Max characters for a rendered name before truncation. SPEC §1C — 24 chars
/// + `…`. The truncated form keeps row widths predictable for grouping math.
const NAME_MAX: usize = 24;

/// Threshold at which the filter hint appears + Done auto-collapses (SPEC §1C).
const FILTER_HINT_THRESHOLD: usize = 10;

/// Group overflow row threshold — `… N more` collapse indicator (SPEC §1C).
const GROUP_OVERFLOW_THRESHOLD: usize = 6;

/// Total-agent threshold for viewport virtualization (SPEC §1C, Fleet=100).
pub const VIRTUALIZE_THRESHOLD: usize = 50;

/// 30-second done-glow TTL (matches GlowFader / Theme::orange_muted band).
const DONE_GLOW_SECS: u64 = 30;

/// AgentNav full-screen surface. State lives here, not on App:
/// `selected_id`, `filter`, group-collapsed map, scroll offset.
#[derive(Default)]
pub struct AgentNavSurface {
    /// The agent currently highlighted, by FROZEN `SubAgentView::id`.
    /// Resolved against the live filtered/grouped list every render so
    /// status transitions that move an agent between groups don't break
    /// the highlight (W4.3).
    pub selected_id: Option<String>,
    /// Filter substring (lowercased once at edit time). `None` = inactive.
    pub filter: Option<String>,
    /// True while `/` filter editor is intercepting text keys.
    pub filter_editing: bool,
    /// Collapse state per group; absent = use default (Failed/Running expanded,
    /// Done collapsed when count > 10).
    pub collapsed: GroupCollapse,
    /// Scroll offset for virtualization at >50 agents. `u16` matches
    /// `WorkspaceSurface::transcript_scroll` precedent (`workspace.rs:122`).
    pub scroll_offset: u16,
}

/// Per-group collapse override. `None` = use default rule; `Some(true)` =
/// force collapsed; `Some(false)` = force expanded.
#[derive(Debug, Clone, Copy, Default)]
pub struct GroupCollapse {
    pub running: Option<bool>,
    pub done: Option<bool>,
    pub failed: Option<bool>,
}

impl GroupCollapse {
    fn get(self, g: Group) -> Option<bool> {
        match g {
            Group::Running => self.running,
            Group::Done => self.done,
            Group::Failed => self.failed,
        }
    }

    fn toggle(&mut self, g: Group, default_collapsed: bool) {
        let slot = match g {
            Group::Running => &mut self.running,
            Group::Done => &mut self.done,
            Group::Failed => &mut self.failed,
        };
        let now = slot.unwrap_or(default_collapsed);
        *slot = Some(!now);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Group {
    Running,
    Done,
    Failed,
}

impl Group {
    fn label(self) -> &'static str {
        match self {
            Group::Running => "Running",
            Group::Done => "Done",
            Group::Failed => "Failed",
        }
    }

    /// Group ordering in the grouped view: Running, Done, Failed (Failed last
    /// so it stays visible at the bottom of the viewport).
    const ORDER: [Group; 3] = [Group::Running, Group::Done, Group::Failed];

    fn of(s: SubAgentStatus) -> Group {
        match s {
            SubAgentStatus::Running => Group::Running,
            SubAgentStatus::Done => Group::Done,
            SubAgentStatus::Failed => Group::Failed,
        }
    }

    fn default_collapsed(self, count: usize) -> bool {
        match self {
            Group::Done => count > FILTER_HINT_THRESHOLD,
            // Running + Failed default-expanded (Failed always per SPEC §1C).
            _ => false,
        }
    }
}

/// One row to render — either a group header (grouped view only) or an agent.
#[derive(Debug, Clone)]
enum Row<'a> {
    /// Grouped-view header row: `▼ Running (12)` or `▶ Done (4)`.
    GroupHeader {
        group: Group,
        count: usize,
        collapsed: bool,
    },
    /// One agent row.
    Agent { agent: &'a SubAgentView },
    /// `… N more` overflow indicator inside an expanded group.
    Overflow { remaining: usize },
}

// ─────────────────────────────────────────────────────────────────────────
// Public surface API — pure builders used by render + tests
// ─────────────────────────────────────────────────────────────────────────

impl AgentNavSurface {
    /// True when the grouped view should be used (>10 agents triggers groups;
    /// at ≤10 the flat list is canonical per SPEC §1C).
    pub fn use_grouped(&self, total: usize) -> bool {
        total > FILTER_HINT_THRESHOLD
    }

    /// Build the header line — `Sub-Agents   <topology>   <N> agents · …`.
    /// Counts always reflect the **unfiltered** total so the user can see
    /// the effect of their filter against the global.
    pub fn build_header_text(&self, agents: &[SubAgentView]) -> String {
        let total = agents.len();
        let mut running = 0usize;
        let mut done = 0usize;
        let mut failed = 0usize;
        for a in agents {
            match a.status {
                SubAgentStatus::Running => running += 1,
                SubAgentStatus::Done => done += 1,
                SubAgentStatus::Failed => failed += 1,
            }
        }
        let topology = topology_label(total);

        let mut parts: Vec<String> = Vec::new();
        parts.push(format!("{total} agents"));
        if running > 0 {
            parts.push(format!("{running} running"));
        }
        if done > 0 {
            parts.push(format!("{done} done"));
        }
        if failed > 0 {
            parts.push(format!("{failed} failed"));
        }
        let counts = parts.join(" · ");
        format!("Sub-Agents   {topology}   {counts}")
    }

    /// Build the footer keybind hint.
    pub fn build_footer_text(&self, total: usize) -> String {
        if total > FILTER_HINT_THRESHOLD {
            "↑↓ select   / filter   ⏎ open   ⎋ workspace".into()
        } else {
            "↑↓ select   ⏎ open   ⎋ workspace".into()
        }
    }

    /// Apply the current filter to a slice of agents — substring match
    /// against `agent.name`, case-insensitive. No filter = pass-through.
    fn filtered<'a>(&self, agents: &'a [SubAgentView]) -> Vec<&'a SubAgentView> {
        match &self.filter {
            Some(f) if !f.is_empty() => {
                let needle = f.to_lowercase();
                agents
                    .iter()
                    .filter(|a| a.name.to_lowercase().contains(&needle))
                    .collect()
            }
            _ => agents.iter().collect(),
        }
    }

    /// Build the ordered row list — the single source-of-truth the renderer
    /// + keyboard navigator both consume.
    fn build_rows<'a>(&self, agents: &'a [SubAgentView]) -> Vec<Row<'a>> {
        let filtered = self.filtered(agents);
        let total_after_filter = filtered.len();
        let mut rows: Vec<Row<'a>> = Vec::new();

        if !self.use_grouped(agents.len()) {
            // Flat list — selected_id resolves directly to one of these rows.
            for a in filtered {
                rows.push(Row::Agent { agent: a });
            }
            return rows;
        }

        for g in Group::ORDER {
            let mut in_group: Vec<&SubAgentView> = filtered
                .iter()
                .copied()
                .filter(|a| Group::of(a.status) == g)
                .collect();
            // SPEC §1C: empty groups OMITTED entirely.
            if in_group.is_empty() {
                continue;
            }
            // Failed always default-expanded per SPEC §1C (override stored as Some(true)).
            let default_collapsed = if g == Group::Failed {
                false
            } else {
                g.default_collapsed(in_group.len())
            };
            let collapsed = self.collapsed.get(g).unwrap_or(default_collapsed);
            let count = in_group.len();
            rows.push(Row::GroupHeader {
                group: g,
                count,
                collapsed,
            });
            if collapsed {
                continue;
            }
            // Expanded — drop in up to GROUP_OVERFLOW_THRESHOLD agents, then `… N more`.
            let visible = in_group.len().min(GROUP_OVERFLOW_THRESHOLD);
            let remaining = in_group.len().saturating_sub(visible);
            for a in in_group.drain(..visible) {
                rows.push(Row::Agent { agent: a });
            }
            if remaining > 0 {
                rows.push(Row::Overflow { remaining });
            }
        }
        let _ = total_after_filter; // currently informational; kept for symmetry.
        rows
    }

    /// Indexes into `rows` that are selectable (agents only — never group
    /// headers, never overflow markers). Used by ↑/↓ key handling.
    fn selectable_row_indexes(rows: &[Row<'_>]) -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter_map(|(i, r)| matches!(r, Row::Agent { .. }).then_some(i))
            .collect()
    }

    /// Find the row index for the current `selected_id`, or `None`.
    fn selected_row_index(&self, rows: &[Row<'_>]) -> Option<usize> {
        let id = self.selected_id.as_ref()?;
        rows.iter().position(|r| match r {
            Row::Agent { agent } => &agent.id == id,
            _ => false,
        })
    }

    /// Ensure `selected_id` points at an agent in the current row list.
    /// Falls back to the first selectable row if the previous id is no
    /// longer present (e.g. cleared by `clear()`).
    fn reconcile_selection(&mut self, rows: &[Row<'_>]) {
        if let Some(id) = &self.selected_id {
            let still_present = rows
                .iter()
                .any(|r| matches!(r, Row::Agent { agent } if &agent.id == id));
            if still_present {
                return;
            }
        }
        // No selection or stale — pick first agent row.
        let first_agent = rows.iter().find_map(|r| match r {
            Row::Agent { agent } => Some(agent.id.clone()),
            _ => None,
        });
        self.selected_id = first_agent;
    }
}

/// SPEC §1B/§1C topology label by total count.
fn topology_label(total: usize) -> &'static str {
    match total {
        0..=10 => "Spawn",
        11..=50 => "Mesh",
        _ => "Fleet",
    }
}

/// SPEC §1C status glyph per status + glow window.
fn status_glyph(status: SubAgentStatus) -> &'static str {
    match status {
        SubAgentStatus::Running => "●",
        SubAgentStatus::Done => "✓",
        SubAgentStatus::Failed => "✗",
    }
}

/// Pick the row fg color per SPEC §1C palette:
/// - Selected → orange
/// - Running → text_running
/// - Done within 30s of last event → orange_muted (glow window)
/// - Done > 30s → text_dim
/// - Failed → error
fn row_fg(
    agent: &SubAgentView,
    selected: bool,
    last_event: Option<Instant>,
    now: Instant,
    theme: &Theme,
) -> ratatui::style::Color {
    if selected {
        return theme.orange;
    }
    match agent.status {
        SubAgentStatus::Running => theme.text_running,
        SubAgentStatus::Done => {
            let glowing = match last_event {
                Some(at) => now.duration_since(at) <= Duration::from_secs(DONE_GLOW_SECS),
                None => false,
            };
            if glowing {
                theme.orange_muted
            } else {
                theme.text_dim
            }
        }
        SubAgentStatus::Failed => theme.error,
    }
}

/// Truncate `name` to ≤ NAME_MAX chars, appending `…` on overflow.
fn truncate_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= NAME_MAX {
        return name.to_string();
    }
    let mut out: String = chars.iter().take(NAME_MAX).collect();
    out.push('…');
    out
}

/// Build the per-agent row text body (status glyph rendered separately so it
/// can be coloured). Returns `(glyph_str, body_str)`.
fn build_agent_row_text(agent: &SubAgentView) -> (String, String) {
    let glyph = status_glyph(agent.status).to_string();
    let name = truncate_name(&agent.name);
    let tail = agent.feed.last().cloned().unwrap_or_default();
    // Compact pad: 24-char-name slot keeps columns aligned in the flat list.
    let body = if tail.is_empty() {
        format!(
            " {name:<24}   {turns} turns · {tokens} tok",
            turns = agent.turns,
            tokens = agent.tokens
        )
    } else {
        format!(
            " {name:<24}   {turns} turns · {tokens} tok · last {tail}",
            turns = agent.turns,
            tokens = agent.tokens,
        )
    };
    (glyph, body)
}

// ─────────────────────────────────────────────────────────────────────────
// Surface trait impl
// ─────────────────────────────────────────────────────────────────────────

impl Surface for AgentNavSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::AgentNav
    }

    fn on_enter(&mut self, app: &mut App) {
        // Ensure selection is valid against the live agent list.
        let rows = self.build_rows(&app.session.sub_agents);
        self.reconcile_selection(&rows);
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        if area.height < 2 || area.width == 0 {
            return;
        }
        let now = Instant::now();
        let agents = &app.session.sub_agents;

        // header (1) · body (min) · footer (1)
        let [header_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);

        // ── Header ──
        let header_line = Line::from(Span::styled(
            self.build_header_text(agents),
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
        ));
        frame.render_widget(Paragraph::new(header_line), header_area);

        // ── Body ──
        let rows = self.build_rows(agents);
        let selectable = Self::selectable_row_indexes(&rows);

        // Resolve selection against the current rows; fall back to first selectable.
        let mut effective_selected_id = self.selected_id.clone();
        if let Some(id) = &effective_selected_id {
            let present = rows
                .iter()
                .any(|r| matches!(r, Row::Agent { agent } if &agent.id == id));
            if !present {
                effective_selected_id = rows.iter().find_map(|r| match r {
                    Row::Agent { agent } => Some(agent.id.clone()),
                    _ => None,
                });
            }
        } else {
            effective_selected_id = rows.iter().find_map(|r| match r {
                Row::Agent { agent } => Some(agent.id.clone()),
                _ => None,
            });
        }

        // Virtualization at >50 agents: render only rows in viewport.
        let viewport_h = body_area.height as usize;
        let total = agents.len();
        let (start, end) = if total > VIRTUALIZE_THRESHOLD {
            let start = self.scroll_offset as usize;
            let end = (start + viewport_h).min(rows.len());
            (start.min(rows.len()), end)
        } else {
            (0usize, rows.len().min(viewport_h))
        };

        let mut lines: Vec<Line<'static>> = Vec::with_capacity(end.saturating_sub(start));
        for r in rows.iter().take(end).skip(start) {
            match r {
                Row::GroupHeader {
                    group,
                    count,
                    collapsed,
                } => {
                    let chevron = if *collapsed { "▶" } else { "▼" };
                    let text = format!("{chevron} {} ({})", group.label(), count);
                    lines.push(Line::from(Span::styled(
                        text,
                        Style::default()
                            .fg(theme.text_muted)
                            .add_modifier(Modifier::BOLD),
                    )));
                }
                Row::Agent { agent } => {
                    let selected = effective_selected_id
                        .as_ref()
                        .map(|s| s == &agent.id)
                        .unwrap_or(false);
                    let last_event = app.agent_last_event.get(&agent.id).copied();
                    let fg = row_fg(agent, selected, last_event, now, theme);
                    let (glyph, body) = build_agent_row_text(agent);
                    let glyph_style = if selected {
                        Style::default()
                            .fg(theme.orange)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(fg)
                    };
                    let body_style = Style::default().fg(fg);
                    lines.push(Line::from(vec![
                        Span::styled(glyph, glyph_style),
                        Span::styled(body, body_style),
                    ]));
                }
                Row::Overflow { remaining } => {
                    lines.push(Line::from(Span::styled(
                        format!("  … {remaining} more"),
                        Style::default().fg(theme.text_muted),
                    )));
                }
            }
        }
        let _ = selectable; // selectable is consumed by keybinds, not render.
        frame.render_widget(Paragraph::new(lines), body_area);

        // ── Footer ──
        let footer_text = if self.filter_editing {
            let buf = self.filter.clone().unwrap_or_default();
            // Trailing caret signals the field is actively capturing input
            // (without it the buffer reads as a static label).
            format!("/filter: {buf}▏")
        } else {
            self.build_footer_text(total)
        };
        let footer = Paragraph::new(Line::from(Span::styled(
            footer_text,
            Style::default().fg(theme.text_muted),
        )));
        frame.render_widget(footer, footer_area);
    }

    fn handle_paste(&mut self, text: String, _app: &mut App) {
        // Bracketed paste only lands in the `/` filter field while it is
        // being edited (everywhere else the navigator is a key-driven list).
        // Without this override the default no-op silently dropped a paste —
        // the same class of bug as the onboarding key-field paste regression.
        if !self.filter_editing {
            return;
        }
        let cleaned: String = text.replace(['\r', '\n'], "");
        if cleaned.is_empty() {
            return;
        }
        self.filter
            .get_or_insert_with(String::new)
            .push_str(&cleaned);
    }

    fn handle_key(&mut self, key: KeyEvent, app: &mut App) -> SurfaceAction {
        // ── Filter-editing mode intercepts all text keys first ──
        if self.filter_editing {
            match key.code {
                KeyCode::Esc => {
                    // Clear filter buffer + exit filter mode, stay on surface.
                    self.filter = None;
                    self.filter_editing = false;
                    return SurfaceAction::None;
                }
                KeyCode::Enter => {
                    // Exit filter mode, keep buffer active for result list.
                    self.filter_editing = false;
                    return SurfaceAction::None;
                }
                KeyCode::Backspace => {
                    if let Some(f) = self.filter.as_mut() {
                        f.pop();
                    }
                    return SurfaceAction::None;
                }
                KeyCode::Char(c) => {
                    self.filter.get_or_insert_with(String::new).push(c);
                    return SurfaceAction::None;
                }
                _ => return SurfaceAction::None,
            }
        }

        // ── Open filter ──
        if let KeyCode::Char('/') = key.code {
            // Only meaningful at >10 agents, but accept always — filter still
            // narrows below threshold without changing the layout.
            self.filter_editing = true;
            self.filter.get_or_insert_with(String::new);
            return SurfaceAction::None;
        }

        // Resolve current rows + selection for keyboard nav.
        let rows = self.build_rows(&app.session.sub_agents);
        let selectable = Self::selectable_row_indexes(&rows);

        match key.code {
            KeyCode::Esc => {
                // Filter NOT open (intercepted above) → Pop to workspace.
                SurfaceAction::Pop
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1, &rows, &selectable);
                SurfaceAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1, &rows, &selectable);
                SurfaceAction::None
            }
            KeyCode::Tab => {
                self.jump_to_next_group(&rows);
                SurfaceAction::None
            }
            KeyCode::Char(' ') => {
                // Space on the row under the cursor toggles its group's collapse.
                // Resolve the cursor's group against the FULL agent list (not the
                // currently-visible rows), so a collapsed group can still be
                // re-expanded when the selected agent has been hidden by the
                // first collapse press.
                let cursor_group = self.selected_id.as_ref().and_then(|id| {
                    app.session
                        .sub_agents
                        .iter()
                        .find(|a| &a.id == id)
                        .map(|a| Group::of(a.status))
                });
                if let Some(g) = cursor_group {
                    let in_group_count = app
                        .session
                        .sub_agents
                        .iter()
                        .filter(|a| Group::of(a.status) == g)
                        .count();
                    let default_collapsed = if g == Group::Failed {
                        false
                    } else {
                        g.default_collapsed(in_group_count)
                    };
                    self.collapsed.toggle(g, default_collapsed);
                }
                SurfaceAction::None
            }
            KeyCode::Enter => {
                // Enter on selected agent → push current state + open transcript.
                if let Some(id) = self.selected_id.clone() {
                    // Confirm the id refers to a real agent in the current list.
                    let exists = app.session.sub_agents.iter().any(|a| a.id == id);
                    if exists {
                        // Push current AgentNav scroll onto surface_stack so Pop restores it.
                        app.surface_stack.push(SurfaceStackEntry {
                            id: SurfaceId::AgentNav,
                            scroll_offset: self.scroll_offset,
                        });
                        app.active_agent_transcript_id = Some(id);
                        return SurfaceAction::Switch(SurfaceId::AgentTranscript);
                    }
                }
                SurfaceAction::None
            }
            _ => SurfaceAction::None,
        }
    }

    fn restore_scroll(&mut self, offset: u16) {
        self.scroll_offset = offset;
    }
}

impl AgentNavSurface {
    /// Move selection by `delta` (±1) among selectable rows, wrapping at
    /// boundaries. Also nudges scroll_offset to keep selection within 3
    /// rows of viewport edge when the list is virtualized.
    fn move_selection(&mut self, delta: i32, rows: &[Row<'_>], selectable: &[usize]) {
        if selectable.is_empty() {
            self.selected_id = None;
            return;
        }
        // Find current position among selectable; default to first.
        let cur_row = self.selected_row_index(rows);
        let cur_pos = cur_row
            .and_then(|r| selectable.iter().position(|&s| s == r))
            .unwrap_or(0);
        let n = selectable.len() as i32;
        let mut next = (cur_pos as i32 + delta) % n;
        if next < 0 {
            next += n;
        }
        let next_row = selectable[next as usize];
        if let Row::Agent { agent } = &rows[next_row] {
            self.selected_id = Some(agent.id.clone());
        }
        self.maybe_scroll_into_view(next_row);
    }

    /// Nudge `scroll_offset` so `target_row` stays at least 3 rows from
    /// the top/bottom edge of the viewport. SPEC §1C.
    fn maybe_scroll_into_view(&mut self, target_row: usize) {
        // Viewport height is not known here without &Frame; the threshold
        // rule (3 rows from edge) is enforced lazily by render's clamp.
        // We adjust scroll_offset by ±1 toward the selection if it is
        // outside the assumed window. Conservative: bring offset to within
        // a few rows of target.
        let target = target_row as u16;
        if target < self.scroll_offset.saturating_add(3) {
            self.scroll_offset = self.scroll_offset.saturating_sub(1);
        } else if target > self.scroll_offset.saturating_add(20) {
            self.scroll_offset = self.scroll_offset.saturating_add(1);
        }
    }

    /// Move selection to the first agent of the next group, wrapping.
    /// No-op at flat scale (no group headers present).
    fn jump_to_next_group(&mut self, rows: &[Row<'_>]) {
        let group_starts: Vec<(usize, Group)> = rows
            .iter()
            .enumerate()
            .filter_map(|(i, r)| match r {
                Row::GroupHeader { group, .. } => Some((i, *group)),
                _ => None,
            })
            .collect();
        if group_starts.is_empty() {
            return; // flat list — no-op.
        }
        let cur_row = self.selected_row_index(rows).unwrap_or(0);
        // Find current group (the last header at or above cur_row).
        let cur_group_idx = group_starts
            .iter()
            .rposition(|&(i, _)| i <= cur_row)
            .unwrap_or(0);
        let next_group_idx = (cur_group_idx + 1) % group_starts.len();
        let next_header_row = group_starts[next_group_idx].0;
        // First Agent row AFTER the header.
        for (i, r) in rows.iter().enumerate().skip(next_header_row + 1) {
            if let Row::Agent { agent } = r {
                self.selected_id = Some(agent.id.clone());
                self.maybe_scroll_into_view(i);
                return;
            }
            // Stop scanning when we hit the next group header.
            if matches!(r, Row::GroupHeader { .. }) {
                break;
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;
    use crate::tui::app::App;

    // ── helpers ──

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn sub(name: &str, status: SubAgentStatus) -> SubAgentView {
        SubAgentView {
            id: name.into(),
            name: name.into(),
            status,
            turns: 0,
            tokens: 0,
            feed: Vec::new(),
        }
    }

    fn mk_app() -> App {
        App::new()
    }

    /// Render `surface` into an 80×24 `TestBackend` and return the buffer
    /// as a single flattened string for substring assertions.
    fn render_to_string(surface: &mut AgentNavSurface, app: &App) -> String {
        let theme = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), app, &theme))
            .expect("render agent_nav");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..24 {
            for x in 0..120 {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    // ── W4.1: header + flat list at Spawn=5 scale ──

    #[test]
    fn w41_header_shows_topology_and_counts_at_spawn_scale() {
        let surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = vec![
            sub("a", SubAgentStatus::Running),
            sub("b", SubAgentStatus::Running),
            sub("c", SubAgentStatus::Running),
            sub("d", SubAgentStatus::Running),
            sub("e", SubAgentStatus::Done),
        ];
        let header = surface.build_header_text(&app.session.sub_agents);
        assert!(header.starts_with("Sub-Agents"), "got {header:?}");
        assert!(header.contains("Spawn"), "got {header:?}");
        assert!(header.contains("5 agents"), "got {header:?}");
        assert!(header.contains("4 running"), "got {header:?}");
        assert!(header.contains("1 done"), "got {header:?}");
    }

    #[test]
    fn w41_flat_list_renders_five_rows_with_glyphs_and_columns() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = vec![
            sub("alpha", SubAgentStatus::Running),
            sub("bravo", SubAgentStatus::Running),
            sub("charlie", SubAgentStatus::Running),
            sub("delta", SubAgentStatus::Running),
            sub("echo", SubAgentStatus::Done),
        ];
        surface.on_enter(&mut app);
        let out = render_to_string(&mut surface, &app);
        // Header line present.
        assert!(out.contains("Sub-Agents"), "header missing\n{out}");
        // All five names rendered.
        for name in ["alpha", "bravo", "charlie", "delta", "echo"] {
            assert!(out.contains(name), "missing {name}\n{out}");
        }
        // Running glyph (●) present.
        assert!(out.contains('●'), "missing running glyph\n{out}");
        // Done glyph (✓) present.
        assert!(out.contains('✓'), "missing done glyph\n{out}");
        // Footer hint present.
        assert!(out.contains("⏎ open"), "footer missing\n{out}");
    }

    #[test]
    fn w41_name_truncates_at_24_chars_with_ellipsis() {
        let long = "a".repeat(40);
        let got = truncate_name(&long);
        assert_eq!(got.chars().count(), 25, "expected 24 + …, got {got}");
        assert!(got.ends_with('…'), "got {got}");
    }

    #[test]
    fn w41_no_filter_hint_at_le_10_agents() {
        let surface = AgentNavSurface::default();
        let footer = surface.build_footer_text(5);
        assert!(!footer.contains("/ filter"), "got {footer}");
        assert!(footer.contains("⎋ workspace"), "got {footer}");
    }

    // ── W4.2: grouped body + virtualization ──

    #[test]
    fn w42_grouped_view_kicks_in_above_10_agents() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = (0..12)
            .map(|i| sub(&format!("r{i}"), SubAgentStatus::Running))
            .collect();
        surface.on_enter(&mut app);
        assert!(surface.use_grouped(12));
        let rows = surface.build_rows(&app.session.sub_agents);
        assert!(matches!(
            rows[0],
            Row::GroupHeader {
                group: Group::Running,
                ..
            }
        ));
        let footer = surface.build_footer_text(12);
        assert!(footer.contains("/ filter"), "got {footer}");
    }

    #[test]
    fn w42_empty_groups_are_omitted() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        // 11 running, 0 done, 0 failed → grouped view, but only Running shows.
        app.session.sub_agents = (0..11)
            .map(|i| sub(&format!("r{i}"), SubAgentStatus::Running))
            .collect();
        let rows = surface.build_rows(&app.session.sub_agents);
        for r in &rows {
            if let Row::GroupHeader { group, .. } = r {
                assert_eq!(
                    *group,
                    Group::Running,
                    "non-running group rendered: {group:?}"
                );
            }
        }
        surface.on_enter(&mut app);
        let _ = surface;
    }

    #[test]
    fn w42_done_collapsed_by_default_when_count_gt_10() {
        let mut surface = AgentNavSurface::default();
        let mut agents: Vec<SubAgentView> = (0..5)
            .map(|i| sub(&format!("r{i}"), SubAgentStatus::Running))
            .collect();
        agents.extend((0..12).map(|i| sub(&format!("d{i}"), SubAgentStatus::Done)));
        let rows = surface.build_rows(&agents);
        // Find Done header and assert it's collapsed.
        let done_collapsed = rows.iter().find_map(|r| match r {
            Row::GroupHeader {
                group: Group::Done,
                collapsed,
                ..
            } => Some(*collapsed),
            _ => None,
        });
        assert_eq!(done_collapsed, Some(true), "done should be collapsed");
        // No Done agent rows should render.
        let done_agent_rows = rows
            .iter()
            .filter(|r| matches!(r, Row::Agent { agent } if agent.status == SubAgentStatus::Done))
            .count();
        assert_eq!(done_agent_rows, 0);
        surface.on_enter(&mut App::new());
    }

    #[test]
    fn w42_failed_always_default_expanded() {
        let surface = AgentNavSurface::default();
        let mut agents: Vec<SubAgentView> = (0..11)
            .map(|i| sub(&format!("r{i}"), SubAgentStatus::Running))
            .collect();
        agents.extend((0..15).map(|i| sub(&format!("f{i}"), SubAgentStatus::Failed)));
        let rows = surface.build_rows(&agents);
        let failed_header_collapsed = rows.iter().find_map(|r| match r {
            Row::GroupHeader {
                group: Group::Failed,
                collapsed,
                ..
            } => Some(*collapsed),
            _ => None,
        });
        assert_eq!(
            failed_header_collapsed,
            Some(false),
            "failed group must be default-expanded"
        );
        let _ = surface;
    }

    #[test]
    fn w42_overflow_row_appears_when_group_exceeds_threshold() {
        let surface = AgentNavSurface::default();
        // 11 running → grouped view, group is expanded by default, 6 visible + 5 more.
        let agents: Vec<SubAgentView> = (0..11)
            .map(|i| sub(&format!("r{i}"), SubAgentStatus::Running))
            .collect();
        let rows = surface.build_rows(&agents);
        let overflow = rows.iter().find_map(|r| match r {
            Row::Overflow { remaining } => Some(*remaining),
            _ => None,
        });
        assert_eq!(overflow, Some(5), "expected 5-more overflow row");
        let _ = surface;
    }

    // ── W4.3: selection follows agent_id across status change ──

    #[test]
    fn w43_selection_follows_agent_across_group_change() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = (0..11)
            .map(|i| sub(&format!("r{i}"), SubAgentStatus::Running))
            .collect();
        surface.on_enter(&mut app);
        // Select agent r5 explicitly.
        surface.selected_id = Some("r5".into());
        // r5 transitions Done → it moves out of Running group, into Done.
        app.session.sub_agents[5].status = SubAgentStatus::Done;
        // Force the Done group expanded so r5 renders as an agent row.
        surface.collapsed.done = Some(false);
        let rows = surface.build_rows(&app.session.sub_agents);
        let sel_row = surface.selected_row_index(&rows);
        assert!(
            sel_row.is_some(),
            "r5 should still be selectable after status change"
        );
        // And it should be inside the Done group, not Running.
        if let Some(i) = sel_row {
            // Walk backward to the nearest GroupHeader.
            let group = rows[..i].iter().rev().find_map(|r| match r {
                Row::GroupHeader { group, .. } => Some(*group),
                _ => None,
            });
            assert_eq!(group, Some(Group::Done), "selected row should be in Done");
        }
    }

    #[test]
    fn w43_reconcile_selection_picks_first_when_id_missing() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = vec![
            sub("a", SubAgentStatus::Running),
            sub("b", SubAgentStatus::Running),
        ];
        surface.selected_id = Some("nonexistent".into());
        let rows = surface.build_rows(&app.session.sub_agents);
        surface.reconcile_selection(&rows);
        assert_eq!(surface.selected_id.as_deref(), Some("a"));
    }

    // ── W4.4: filter input ──

    #[test]
    fn w44_slash_opens_filter_editing() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = vec![sub("a", SubAgentStatus::Running)];
        let action = surface.handle_key(key(KeyCode::Char('/')), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        assert!(surface.filter_editing);
    }

    #[test]
    fn w44_filter_narrows_results_substring() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = vec![
            sub("alpha", SubAgentStatus::Running),
            sub("alphabet", SubAgentStatus::Running),
            sub("beta", SubAgentStatus::Running),
        ];
        surface.handle_key(key(KeyCode::Char('/')), &mut app);
        surface.handle_key(key(KeyCode::Char('a')), &mut app);
        surface.handle_key(key(KeyCode::Char('l')), &mut app);
        surface.handle_key(key(KeyCode::Char('p')), &mut app);
        let rows = surface.build_rows(&app.session.sub_agents);
        let names: Vec<&str> = rows
            .iter()
            .filter_map(|r| match r {
                Row::Agent { agent } => Some(agent.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(names, vec!["alpha", "alphabet"]);
    }

    #[test]
    fn w44_esc_clears_filter_without_exiting_surface() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = vec![sub("a", SubAgentStatus::Running)];
        surface.handle_key(key(KeyCode::Char('/')), &mut app);
        surface.handle_key(key(KeyCode::Char('x')), &mut app);
        assert_eq!(surface.filter.as_deref(), Some("x"));
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "Esc inside filter must not pop"
        );
        assert!(surface.filter.is_none());
        assert!(!surface.filter_editing);
    }

    #[test]
    fn w44_enter_exits_filter_mode_keeps_buffer() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = vec![sub("a", SubAgentStatus::Running)];
        surface.handle_key(key(KeyCode::Char('/')), &mut app);
        surface.handle_key(key(KeyCode::Char('a')), &mut app);
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        assert!(!surface.filter_editing);
        assert_eq!(surface.filter.as_deref(), Some("a"), "buffer must persist");
    }

    // ── W4.5: keybinds ──

    #[test]
    fn w45_up_down_walk_through_agents() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = vec![
            sub("a", SubAgentStatus::Running),
            sub("b", SubAgentStatus::Running),
            sub("c", SubAgentStatus::Running),
        ];
        surface.on_enter(&mut app);
        assert_eq!(surface.selected_id.as_deref(), Some("a"));
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.selected_id.as_deref(), Some("b"));
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.selected_id.as_deref(), Some("c"));
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.selected_id.as_deref(), Some("a"), "wraps to top");
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(surface.selected_id.as_deref(), Some("c"), "wraps to bottom");
    }

    #[test]
    fn w45_enter_on_agent_pushes_stack_sets_active_id_and_switches() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = vec![sub("a", SubAgentStatus::Running)];
        surface.on_enter(&mut app);
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(matches!(
            action,
            SurfaceAction::Switch(SurfaceId::AgentTranscript)
        ));
        assert_eq!(app.active_agent_transcript_id.as_deref(), Some("a"));
        assert_eq!(app.surface_stack.len(), 1);
        assert_eq!(app.surface_stack[0].id, SurfaceId::AgentNav);
    }

    #[test]
    fn w45_esc_with_no_filter_open_returns_pop() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        app.session.sub_agents = vec![sub("a", SubAgentStatus::Running)];
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(matches!(action, SurfaceAction::Pop));
    }

    #[test]
    fn w45_space_on_agent_row_toggles_its_group_collapse() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        // 11 running → grouped view, Running default-expanded.
        app.session.sub_agents = (0..11)
            .map(|i| sub(&format!("r{i}"), SubAgentStatus::Running))
            .collect();
        surface.on_enter(&mut app);
        // First selectable agent is r0 (Running).
        surface.handle_key(key(KeyCode::Char(' ')), &mut app);
        assert_eq!(
            surface.collapsed.running,
            Some(true),
            "Space toggled Running collapsed"
        );
        surface.handle_key(key(KeyCode::Char(' ')), &mut app);
        assert_eq!(
            surface.collapsed.running,
            Some(false),
            "Space toggled Running back to expanded"
        );
    }

    #[test]
    fn w45_tab_jumps_to_next_group() {
        let mut surface = AgentNavSurface::default();
        let mut app = mk_app();
        let mut agents: Vec<SubAgentView> = (0..11)
            .map(|i| sub(&format!("r{i}"), SubAgentStatus::Running))
            .collect();
        agents.push(sub("f0", SubAgentStatus::Failed));
        app.session.sub_agents = agents;
        surface.on_enter(&mut app);
        // First selection should land on r0 (Running group).
        assert_eq!(surface.selected_id.as_deref(), Some("r0"));
        surface.handle_key(key(KeyCode::Tab), &mut app);
        // Now should be on f0 (Failed group is next non-empty after Running;
        // Done has 0 agents so it's omitted entirely).
        assert_eq!(surface.selected_id.as_deref(), Some("f0"));
    }

    // ── Stale window — Done agents within 30s glow ──

    #[test]
    fn w41_done_agent_glow_window_uses_orange_muted() {
        let mut app = mk_app();
        let theme = Theme::hearth();
        let agent = sub("a", SubAgentStatus::Done);
        let now = Instant::now();
        // Just-completed → glow color.
        app.agent_last_event.insert("a".into(), now);
        let fg = row_fg(
            &agent,
            false,
            app.agent_last_event.get("a").copied(),
            now,
            &theme,
        );
        assert_eq!(fg, theme.orange_muted);
        // Old completion (> 30s) → dim color.
        let stale = now - Duration::from_secs(31);
        let fg2 = row_fg(&agent, false, Some(stale), now, &theme);
        assert_eq!(fg2, theme.text_dim);
    }

    // ── Restore scroll trait ──

    #[test]
    fn restore_scroll_round_trips_offset() {
        let mut surface = AgentNavSurface::default();
        surface.restore_scroll(7);
        assert_eq!(surface.scroll_offset, 7);
    }
}
