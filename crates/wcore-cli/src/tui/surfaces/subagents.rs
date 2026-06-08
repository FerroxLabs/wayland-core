//! Sub-agents surface (surface 04) — the honest live monitor of an
//! in-flight `Spawn` call.
//!
//! `Spawn` is fire-and-collect: the main agent spawns up to 5 named
//! sub-agents that run in parallel with NO shared state. The audit
//! (C9–C12) is explicit about what does NOT exist and must therefore not
//! be drawn: there is no completion signal, so no progress percentages;
//! `Spawn` forbids coordination, so no "blocked" / dependency state; the
//! sub-agents are LLM-invoked, so no "+spawn" button; and results are
//! string-concatenated back to the parent, so no auto-merge.
//!
//! What IS real — and all this surface shows — is, per sub-agent: a name,
//! a `SubAgentStatus` (Running / Done / Failed), a turn count, a token
//! count, and a live feed (the `ChannelSink` relay). One sub-agent at a
//! time can be expanded to read its full feed.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use crate::tui::app::{App, SubAgentStatus, SubAgentView};
use crate::tui::surfaces::{Surface, SurfaceAction, SurfaceId};
use crate::tui::theme::Theme;

/// The sub-agent live-monitor surface. Implements the frozen `Surface`
/// trait.
///
/// All state here is surface-local view state — the selection cursor and
/// which card is expanded. The sub-agents themselves live on
/// `App::session::sub_agents`, written only by the protocol bridge.
pub struct SubAgentsSurface {
    /// Index of the highlighted sub-agent card. Clamped to the live list
    /// length at render time so a stale cursor never points past the end.
    selected: usize,
    /// Index of the card whose live feed is expanded, if any. `Enter`
    /// toggles it; only one card expands at a time.
    expanded: Option<usize>,
}

impl SubAgentsSurface {
    /// Construct the surface with the first card selected and none
    /// expanded.
    pub fn new() -> Self {
        Self {
            selected: 0,
            expanded: None,
        }
    }

    /// The selection index clamped into `0..len` (or `0` when the list is
    /// empty). The cursor is stored unclamped and resolved here so a list
    /// that shrinks between frames can never produce an out-of-bounds
    /// index.
    fn clamped_selection(&self, len: usize) -> usize {
        if len == 0 {
            0
        } else {
            self.selected.min(len - 1)
        }
    }
}

impl Default for SubAgentsSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl Surface for SubAgentsSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::SubAgents
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let agents = &app.session.sub_agents;

        // Paint the surface background so the panel reads as one screen.
        frame.render_widget(Block::default().style(Style::default().bg(theme.bg)), area);

        // Vertical layout: a one-line summary bar, the card list, and a
        // one-line hint footer.
        let [bar_area, grid_area, hint_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .areas(area);

        render_summary_bar(frame, bar_area, agents, theme);

        if agents.is_empty() {
            render_empty(frame, grid_area, theme);
        } else {
            render_grid(frame, grid_area, agents, self, theme);
        }

        render_hint(frame, hint_area, theme);
    }

    fn handle_key(&mut self, key: KeyEvent, app: &mut App) -> SurfaceAction {
        let len = app.session.sub_agents.len();

        match key.code {
            // Move the selection cursor within the card list.
            KeyCode::Down | KeyCode::Char('j') => {
                if len > 0 {
                    let cur = self.clamped_selection(len);
                    self.selected = (cur + 1) % len;
                }
                SurfaceAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if len > 0 {
                    let cur = self.clamped_selection(len);
                    self.selected = (cur + len - 1) % len;
                }
                SurfaceAction::None
            }
            // Toggle the selected card's expanded feed.
            KeyCode::Enter => {
                if len > 0 {
                    let cur = self.clamped_selection(len);
                    self.expanded = if self.expanded == Some(cur) {
                        None
                    } else {
                        Some(cur)
                    };
                }
                SurfaceAction::None
            }
            // `Esc` collapses an expanded feed first; with nothing
            // expanded it falls through to the router as `CloseOverlay`
            // (a no-op when this surface is not an overlay, but a sub-
            // agent surface is a primary tab, so this is the safe default).
            KeyCode::Esc => {
                if self.expanded.is_some() {
                    self.expanded = None;
                    SurfaceAction::None
                } else {
                    SurfaceAction::CloseOverlay
                }
            }
            KeyCode::Char('q') => SurfaceAction::Quit,
            KeyCode::Char('p') => SurfaceAction::OpenOverlay(SurfaceId::Palette),
            // Tab chrome — mirror the StubSurface cycling contract so the
            // router stays navigable until Wave 2 wires the keymap.
            KeyCode::Tab => SurfaceAction::Switch(next_tab(SurfaceId::SubAgents)),
            KeyCode::BackTab => SurfaceAction::Switch(prev_tab(SurfaceId::SubAgents)),
            _ => SurfaceAction::None,
        }
    }
}

/// The next tab after `id`, wrapping. `SubAgents` is always a tab, so the
/// `position` lookup never falls back.
fn next_tab(id: SurfaceId) -> SurfaceId {
    let idx = SurfaceId::TABS.iter().position(|&s| s == id).unwrap_or(0);
    SurfaceId::TABS[(idx + 1) % SurfaceId::TABS.len()]
}

/// The previous tab before `id`, wrapping.
fn prev_tab(id: SurfaceId) -> SurfaceId {
    let len = SurfaceId::TABS.len();
    let idx = SurfaceId::TABS.iter().position(|&s| s == id).unwrap_or(0);
    SurfaceId::TABS[(idx + len - 1) % len]
}

/// Render the one-line summary bar: total count, running tally, done
/// tally — the honest mockup `.mgr-bar .sum` (no progress aggregate).
fn render_summary_bar(frame: &mut Frame, area: Rect, agents: &[SubAgentView], theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let running = agents
        .iter()
        .filter(|a| a.status == SubAgentStatus::Running)
        .count();
    let done = agents
        .iter()
        .filter(|a| a.status == SubAgentStatus::Done)
        .count();
    let failed = agents
        .iter()
        .filter(|a| a.status == SubAgentStatus::Failed)
        .count();

    let bg = Style::default().bg(theme.surface);
    let mut spans = vec![
        Span::styled(
            " SUB-AGENTS · LIVE  ",
            bg.fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{} spawned   ", agents.len()),
            bg.fg(theme.text_dim),
        ),
        Span::styled(format!("● {running} running"), bg.fg(theme.orange)),
        Span::styled("   ", bg),
        Span::styled(format!("● {done} done"), bg.fg(theme.success)),
    ];
    if failed > 0 {
        spans.push(Span::styled("   ", bg));
        spans.push(Span::styled(
            format!("● {failed} failed"),
            bg.fg(theme.error),
        ));
    }

    let para = Paragraph::new(Line::from(spans)).style(bg);
    frame.render_widget(para, area);
}

/// Render the empty state — no spawn is in flight.
fn render_empty(frame: &mut Frame, area: Rect, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let para = Paragraph::new(vec![
        Line::from(""),
        Line::from(Span::styled(
            "  No sub-agents running. This tab is a read-only monitor.",
            Style::default().bg(theme.bg).fg(theme.text_dim),
        )),
        Line::from(Span::styled(
            "  To run agents in parallel, ask the main agent to spawn sub-agents \
             (e.g. \"spawn 3 agents to...\"). They appear here live while they run.",
            Style::default().bg(theme.bg).fg(theme.text_muted),
        )),
    ])
    .style(Style::default().bg(theme.bg));
    frame.render_widget(para, area);
}

/// Render the one-line footer hint.
fn render_hint(frame: &mut Frame, area: Rect, theme: &Theme) {
    if area.height == 0 {
        return;
    }
    let bg = Style::default().bg(theme.surface);
    let spans = vec![
        Span::styled(" ↑↓ ", bg.fg(theme.orange)),
        Span::styled("select   ", bg.fg(theme.text_muted)),
        Span::styled("⏎ ", bg.fg(theme.orange)),
        Span::styled("expand feed   ", bg.fg(theme.text_muted)),
        Span::styled("Esc ", bg.fg(theme.orange)),
        Span::styled("collapse", bg.fg(theme.text_muted)),
    ];
    frame.render_widget(Paragraph::new(Line::from(spans)).style(bg), area);
}

/// Render the stack of sub-agent cards. Each card is a fixed-height block;
/// the selected card carries an accent border, and the expanded card
/// grows to also show its live feed.
fn render_grid(
    frame: &mut Frame,
    area: Rect,
    agents: &[SubAgentView],
    surface: &SubAgentsSurface,
    theme: &Theme,
) {
    if area.height == 0 {
        return;
    }
    let selected = surface.clamped_selection(agents.len());

    // Per-card height: a collapsed card is 4 rows (border + 2 content +
    // border); an expanded card adds up to 6 feed rows.
    const COLLAPSED: u16 = 4;
    const FEED_ROWS: u16 = 6;

    let constraints: Vec<Constraint> = agents
        .iter()
        .enumerate()
        .map(|(i, _)| {
            if surface.expanded == Some(i) {
                Constraint::Length(COLLAPSED + FEED_ROWS)
            } else {
                Constraint::Length(COLLAPSED)
            }
        })
        .collect();

    let rows = Layout::vertical(constraints).split(area);
    for (i, agent) in agents.iter().enumerate() {
        let card_area = rows[i];
        if card_area.height == 0 {
            continue;
        }
        render_card(
            frame,
            card_area,
            agent,
            i == selected,
            surface.expanded == Some(i),
            theme,
        );
    }
}

/// Render one sub-agent card.
fn render_card(
    frame: &mut Frame,
    area: Rect,
    agent: &SubAgentView,
    is_selected: bool,
    is_expanded: bool,
    theme: &Theme,
) {
    let accent = status_color(agent.status, theme);

    // The selected card carries the status-tinted border; the rest keep
    // the chrome border so unselected cards recede (mockup `.acard`).
    let border_color = if is_selected { accent } else { theme.border };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .style(Style::default().bg(theme.surface));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let bg = Style::default().bg(theme.surface);

    // Row 1: ● name   STATUS   turn N · M tokens
    let header = Line::from(vec![
        Span::styled("● ", bg.fg(accent)),
        Span::styled(
            agent.name.clone(),
            bg.fg(theme.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled("   ", bg),
        Span::styled(status_label(agent.status), bg.fg(accent)),
        Span::styled("   ", bg),
        Span::styled(
            format!("turn {} · {}", agent.turns, fmt_tokens(agent.tokens)),
            bg.fg(theme.text_muted),
        ),
    ]);

    // Row 2: the latest feed line, or a placeholder when the feed is
    // empty. The expanded card shows the full feed below instead.
    let latest = agent
        .feed
        .last()
        .map(String::as_str)
        .unwrap_or("waiting for first action…");
    let subline = Line::from(Span::styled(format!("  {latest}"), bg.fg(theme.text_dim)));

    let mut lines = vec![header, subline];

    if is_expanded {
        lines.push(Line::from(Span::styled(
            "  LIVE FEED · ChannelSink",
            bg.fg(theme.text_muted).add_modifier(Modifier::BOLD),
        )));
        if agent.feed.is_empty() {
            lines.push(Line::from(Span::styled(
                "  (no feed lines yet)",
                bg.fg(theme.text_muted),
            )));
        } else {
            // Show the most recent feed lines, oldest of the window first,
            // bounded by the rows the expanded card reserves.
            let budget = inner.height.saturating_sub(3) as usize;
            let start = agent.feed.len().saturating_sub(budget.max(1));
            for line in &agent.feed[start..] {
                lines.push(Line::from(Span::styled(
                    format!("  {line}"),
                    bg.fg(theme.text_dim),
                )));
            }
        }
    }

    let para = Paragraph::new(lines).style(bg);
    frame.render_widget(para, inner);
}

/// The accent color for a sub-agent's lifecycle status (Hearth Palette).
fn status_color(status: SubAgentStatus, theme: &Theme) -> ratatui::style::Color {
    match status {
        SubAgentStatus::Running => theme.orange,
        SubAgentStatus::Done => theme.success,
        SubAgentStatus::Failed => theme.error,
    }
}

/// A short, upper-cased status label for a card header.
fn status_label(status: SubAgentStatus) -> &'static str {
    match status {
        SubAgentStatus::Running => "RUNNING",
        SubAgentStatus::Done => "DONE",
        SubAgentStatus::Failed => "FAILED",
    }
}

/// Format a token count compactly: `9.8k` past a thousand, the raw count
/// otherwise. Mirrors the mockup's `tokens 31.2k` rendering.
fn fmt_tokens(tokens: u64) -> String {
    if tokens >= 1000 {
        format!("{:.1}k tokens", tokens as f64 / 1000.0)
    } else {
        format!("{tokens} tokens")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::app::App;
    use crate::tui::fixtures;
    use crate::tui::protocol_bridge::apply_event;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

    /// Build an `App` whose `sub_agents` holds the given views.
    fn app_with(agents: Vec<SubAgentView>) -> App {
        let mut app = App::new();
        app.session.sub_agents = agents;
        app
    }

    /// A `SubAgentView` with the given name + status, a few feed lines.
    fn agent(name: &str, status: SubAgentStatus, turns: usize, tokens: u64) -> SubAgentView {
        SubAgentView {
            id: format!("spawn:{name}"),
            name: name.into(),
            status,
            turns,
            tokens,
            feed: vec![
                format!("{name} started"),
                format!("{name} reading files"),
                format!("{name} latest action"),
            ],
        }
    }

    /// A canonical 3-running / 1-done state — the acceptance fixture.
    fn three_running_one_done() -> Vec<SubAgentView> {
        vec![
            agent("scout", SubAgentStatus::Running, 6, 31_200),
            agent("mason", SubAgentStatus::Running, 9, 27_400),
            agent("probe", SubAgentStatus::Running, 3, 9_800),
            agent("scribe", SubAgentStatus::Done, 8, 12_000),
        ]
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Render the surface into a `TestBackend` and return the whole frame
    /// as a single string.
    fn render(surface: &mut SubAgentsSurface, app: &App, w: u16, h: u16) -> String {
        let theme = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), app, &theme))
            .expect("render sub-agents surface");
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

    #[test]
    fn id_is_sub_agents() {
        assert_eq!(SubAgentsSurface::new().id(), SurfaceId::SubAgents);
    }

    #[test]
    fn renders_three_running_one_done_summary() {
        let app = app_with(three_running_one_done());
        let mut surface = SubAgentsSurface::new();
        let out = render(&mut surface, &app, 100, 30);
        // The honest summary: total, running tally, done tally.
        assert!(out.contains("4 spawned"), "total missing:\n{out}");
        assert!(out.contains("3 running"), "running tally missing:\n{out}");
        assert!(out.contains("1 done"), "done tally missing:\n{out}");
    }

    #[test]
    fn renders_every_sub_agent_name_and_status() {
        let app = app_with(three_running_one_done());
        let mut surface = SubAgentsSurface::new();
        let out = render(&mut surface, &app, 100, 30);
        for name in ["scout", "mason", "probe", "scribe"] {
            assert!(out.contains(name), "card for `{name}` missing:\n{out}");
        }
        assert!(out.contains("RUNNING"), "running status missing:\n{out}");
        assert!(out.contains("DONE"), "done status missing:\n{out}");
    }

    #[test]
    fn renders_turn_and_token_counts() {
        let app = app_with(three_running_one_done());
        let mut surface = SubAgentsSurface::new();
        let out = render(&mut surface, &app, 100, 30);
        // Real Spawn signals — turns and tokens — are shown; nothing else.
        assert!(out.contains("turn 6"), "turn count missing:\n{out}");
        assert!(out.contains("31.2k tokens"), "token count missing:\n{out}");
    }

    #[test]
    fn honest_surface_shows_no_invented_affordances() {
        // Audit C9–C12: no progress %, no blocked state, no +spawn button,
        // no auto-merge. None of those words may appear in the rendering.
        let app = app_with(three_running_one_done());
        let mut surface = SubAgentsSurface::new();
        let out = render(&mut surface, &app, 100, 30).to_lowercase();
        for forbidden in [
            "%",
            "blocked",
            "+spawn",
            "spawn agent",
            "auto-merge",
            "merge",
        ] {
            assert!(
                !out.contains(forbidden),
                "forbidden affordance `{forbidden}` rendered:\n{out}"
            );
        }
    }

    #[test]
    fn enter_expands_the_selected_feed() {
        let app = app_with(three_running_one_done());
        let mut surface = SubAgentsSurface::new();

        // Collapsed: the feed header is not shown.
        let collapsed = render(&mut surface, &app, 100, 30);
        assert!(
            !collapsed.contains("LIVE FEED"),
            "feed shown before expand:\n{collapsed}"
        );

        // Enter expands the first (selected) card's feed.
        let mut app = app;
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        let expanded = render(&mut surface, &app, 100, 30);
        assert!(
            expanded.contains("LIVE FEED"),
            "feed not shown after expand:\n{expanded}"
        );
        assert!(
            expanded.contains("scout latest action"),
            "feed line missing after expand:\n{expanded}"
        );
    }

    #[test]
    fn enter_toggles_the_feed_back_closed() {
        let mut app = app_with(three_running_one_done());
        let mut surface = SubAgentsSurface::new();
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(surface.expanded, Some(0));
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(surface.expanded, None);
    }

    #[test]
    fn arrows_move_the_selection_and_wrap() {
        let mut app = app_with(three_running_one_done());
        let mut surface = SubAgentsSurface::new();
        assert_eq!(surface.selected, 0);

        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.selected, 1);
        surface.handle_key(key(KeyCode::Down), &mut app);
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.selected, 3);
        // Down past the last card wraps to the first.
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.selected, 0);
        // Up from the first wraps to the last.
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(surface.selected, 3);
    }

    #[test]
    fn esc_collapses_an_expanded_feed_before_closing() {
        let mut app = app_with(three_running_one_done());
        let mut surface = SubAgentsSurface::new();
        surface.handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(surface.expanded, Some(0));

        // First Esc only collapses the feed — no routing effect.
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(matches!(action, SurfaceAction::None));
        assert_eq!(surface.expanded, None);

        // A second Esc, with nothing expanded, asks the router to close.
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(matches!(action, SurfaceAction::CloseOverlay));
    }

    #[test]
    fn quit_and_palette_keys_return_their_actions() {
        let mut app = app_with(three_running_one_done());
        let mut surface = SubAgentsSurface::new();
        assert!(matches!(
            surface.handle_key(key(KeyCode::Char('q')), &mut app),
            SurfaceAction::Quit
        ));
        assert!(matches!(
            surface.handle_key(key(KeyCode::Char('p')), &mut app),
            SurfaceAction::OpenOverlay(SurfaceId::Palette)
        ));
    }

    #[test]
    fn tab_keys_switch_to_adjacent_surfaces() {
        let mut app = app_with(three_running_one_done());
        let mut surface = SubAgentsSurface::new();
        // SubAgents is TABS[2]; Tab -> TABS[3] (PlanReview).
        assert!(matches!(
            surface.handle_key(key(KeyCode::Tab), &mut app),
            SurfaceAction::Switch(SurfaceId::PlanReview)
        ));
        // BackTab -> TABS[1] (Workspace).
        assert!(matches!(
            surface.handle_key(key(KeyCode::BackTab), &mut app),
            SurfaceAction::Switch(SurfaceId::Workspace)
        ));
    }

    #[test]
    fn empty_state_renders_without_panicking() {
        let app = App::new();
        let mut surface = SubAgentsSurface::new();
        let out = render(&mut surface, &app, 80, 20);
        assert!(
            out.contains("No sub-agents running"),
            "empty copy missing:\n{out}"
        );
        assert!(out.contains("0 running"), "empty summary missing:\n{out}");
    }

    #[test]
    fn empty_state_is_honest_about_being_a_read_only_monitor_d035() {
        // D035: the Sub-Agents tab is a read-only dashboard — the Spawn tool is
        // LLM-invoked, there is no user-facing "+spawn" trigger here. The empty
        // state must say so AND point the user at the real way to start
        // parallelism (ask the agent to spawn), rather than implying this tab is
        // interactive. Drive the REAL render so the copy is a RENDERED assertion.
        let app = App::new();
        let mut surface = SubAgentsSurface::new();
        let out = render(&mut surface, &app, 100, 20);
        assert!(
            out.contains("read-only monitor"),
            "empty state must flag the tab as read-only:\n{out}"
        );
        assert!(
            out.contains("spawn sub-agents"),
            "empty state must point the user at how to start parallelism:\n{out}"
        );
    }

    #[test]
    fn keys_on_an_empty_list_are_inert_and_safe() {
        let mut app = App::new();
        let mut surface = SubAgentsSurface::new();
        // No cards: cursor moves, Enter expansion, all stay no-ops without
        // panicking on the empty `sub_agents` vec.
        assert!(matches!(
            surface.handle_key(key(KeyCode::Down), &mut app),
            SurfaceAction::None
        ));
        assert_eq!(surface.selected, 0);
        assert!(matches!(
            surface.handle_key(key(KeyCode::Enter), &mut app),
            SurfaceAction::None
        ));
        assert_eq!(surface.expanded, None);
    }

    #[test]
    fn renders_tiny_area_without_panicking() {
        let app = app_with(three_running_one_done());
        let mut surface = SubAgentsSurface::new();
        // A 1x1 frame must not panic any of the layout splits.
        let _ = render(&mut surface, &app, 1, 1);
        let _ = render(&mut surface, &app, 5, 3);
    }

    #[test]
    fn failed_sub_agent_is_counted_and_labelled() {
        let app = app_with(vec![
            agent("scout", SubAgentStatus::Running, 2, 500),
            agent("probe", SubAgentStatus::Failed, 1, 200),
        ]);
        let mut surface = SubAgentsSurface::new();
        let out = render(&mut surface, &app, 100, 20);
        assert!(out.contains("1 failed"), "failed tally missing:\n{out}");
        assert!(out.contains("FAILED"), "failed label missing:\n{out}");
    }

    /// Drives the real T0.5 fixture stream through the protocol bridge and
    /// renders the resulting `App` — the surface must show the one `Done`
    /// sub-agent the fixture produces, with its feed expandable.
    #[test]
    fn renders_the_sub_agent_spawn_fixture() {
        let mut app = App::new();
        for ev in fixtures::sub_agent_spawn() {
            apply_event(&mut app, ev);
        }
        assert_eq!(app.session.sub_agents.len(), 1);
        assert_eq!(app.session.sub_agents[0].status, SubAgentStatus::Done);

        let mut surface = SubAgentsSurface::new();
        let collapsed = render(&mut surface, &app, 100, 24);
        assert!(
            collapsed.contains("reviewer"),
            "fixture agent missing:\n{collapsed}"
        );
        assert!(
            collapsed.contains("DONE"),
            "fixture status missing:\n{collapsed}"
        );
        assert!(
            collapsed.contains("1 done"),
            "fixture summary missing:\n{collapsed}"
        );

        // Expanding shows the fixture's live-feed lines.
        surface.handle_key(key(KeyCode::Enter), &mut app);
        let expanded = render(&mut surface, &app, 100, 24);
        assert!(
            expanded.contains("LIVE FEED"),
            "feed header missing:\n{expanded}"
        );
        assert!(
            expanded.contains("Reviewing the diff"),
            "fixture feed line missing:\n{expanded}"
        );
    }
}
