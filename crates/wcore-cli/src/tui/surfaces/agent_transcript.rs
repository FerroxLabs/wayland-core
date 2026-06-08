//! v0.9.3 W5 — per-agent transcript surface (read-only feed render).
//!
//! Reads `App::active_agent_transcript_id` (set by AgentNav before Switch)
//! to discover which agent to render. The surface is intentionally
//! composer-less while `status == Running` — the header signal
//! `Running — read-only feed` is the explicit UX-H9 cue.
//!
//! Layout:
//! ```text
//! ┌──────────────────────────────────────────────────────────────────┐
//! │ ‹ Sub-Agents   <name>   <glyph> <status-label>   N turns · K tok │  (header, 1 line)
//! ├──────────────────────────────────────────────────────────────────┤
//! │ <markdown-rendered feed>                                          │  (body, fills rest)
//! │   ...                                                             │
//! └──────────────────────────────────────────────────────────────────┘
//! ```
//!
//! Keybinds (W5.4):
//! - `↑/↓` — scroll by 1 visual line.
//! - `PgUp/PgDn` — scroll by viewport height.
//! - `Home/End` — jump to top / bottom.
//! - `Esc` — `SurfaceAction::Pop` (Router handles Esc-Esc chord at SPEC §3.5D).

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use crate::tui::app::{App, SubAgentStatus, SubAgentView};
use crate::tui::surfaces::{Surface, SurfaceAction, SurfaceId};
use crate::tui::theme::Theme;

/// Hard ceiling on rendered feed lines. Anything older is pruned with a
/// `… (older lines pruned)` header so the renderer cannot OOM on a
/// runaway sub-agent. Matches PLAN §W5.2.
const FEED_CAP: usize = 5000;

/// AgentTranscriptSurface — per-agent read-only feed viewer.
#[derive(Default)]
pub struct AgentTranscriptSurface {
    /// Visual-row scroll offset from the top of the rendered feed. `u16`
    /// to match the `transcript_scroll` precedent at `workspace.rs:122`.
    pub scroll_offset: u16,
    /// Sticky-bottom flag. When true, the next render snaps the offset to
    /// the bottom. Flipped off by any user-initiated upward scroll, back
    /// on when the user manually returns to the bottom (End / boundary).
    pub is_at_bottom: bool,
    /// Cache-validity sentinel. When `agent.feed.len() != last_feed_len`,
    /// the cached `rendered_lines` is stale and must be rebuilt.
    pub last_feed_len: usize,
    /// Cached markdown-rendered lines. Reused frame-to-frame while the
    /// feed length is unchanged.
    cached_lines: Vec<Line<'static>>,
    /// Cached agent id whose feed populated `cached_lines`. Switching
    /// agents (rare, but possible if AgentNav re-pushes mid-session)
    /// invalidates the cache too.
    cached_agent_id: Option<String>,
    /// Last rendered viewport height — used by the keybind handlers
    /// (PgUp/PgDn) which need a page step but don't have a `&Frame`.
    /// Updated on every `render`. 0 until the first frame.
    last_viewport_height: u16,
    /// Last computed total visual-line count from the previous render.
    /// Used to clamp scroll offsets in `handle_key` without re-running
    /// the markdown renderer.
    last_total_lines: u16,
}

impl AgentTranscriptSurface {
    /// True when the surface has never rendered a feed for this agent
    /// (initial cold-boot), so `on_enter` should snap to the bottom.
    fn cache_is_cold(&self, agent_id: &str) -> bool {
        match &self.cached_agent_id {
            Some(id) => id != agent_id,
            None => true,
        }
    }

    /// Resolve the active agent. Returns `None` if either the active id
    /// is unset OR the id no longer matches any agent (e.g. session
    /// reset cleared `sub_agents`). Callers render an empty placeholder
    /// in that case — no panic.
    fn active_agent<'a>(&self, app: &'a App) -> Option<&'a SubAgentView> {
        let id = app.active_agent_transcript_id.as_deref()?;
        app.session.sub_agents.iter().find(|a| a.id == id)
    }

    /// Build the header line per SPEC §1D + UX-H9. Visible signal that
    /// no composer is present while running.
    fn build_header(agent: &SubAgentView, theme: &Theme) -> Line<'static> {
        let (glyph, label, status_fg) = match agent.status {
            SubAgentStatus::Running => ("●", "Running — read-only feed", theme.orange),
            SubAgentStatus::Done => ("✓", "Done", theme.success),
            SubAgentStatus::Failed => ("✗", "Failed", theme.error),
        };
        let counts = format!("{} turns · {} tok", agent.turns, agent.tokens);
        Line::from(vec![
            Span::styled("‹ Sub-Agents", Style::default().fg(theme.text_dim)),
            Span::raw("   "),
            Span::styled(
                agent.name.clone(),
                Style::default()
                    .fg(theme.orange)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("   "),
            Span::styled(format!("{glyph} "), Style::default().fg(status_fg)),
            Span::styled(label.to_string(), Style::default().fg(status_fg)),
            Span::raw("   "),
            Span::styled(counts, Style::default().fg(theme.text_dim)),
        ])
    }

    /// Rebuild `cached_lines` from `agent.feed`. Applies the 5000-line
    /// cap with a pruned-header prefix when exceeded. Width-aware so wide
    /// tables degrade to the bullet-list fallback (v0.9.1.2 F11-followup).
    /// H3-closed: destructures the `(Vec<Line>, Vec<String>)` tuple and
    /// discards the OSC8 link refs (the transcript is read-only —
    /// clickable links route through workspace's existing OSC8 path).
    fn rebuild_cache(&mut self, agent: &SubAgentView, theme: &Theme, area_width: u16) {
        let (header_line, lines_to_render): (Option<&str>, &[String]) =
            if agent.feed.len() > FEED_CAP {
                let start = agent.feed.len() - FEED_CAP;
                (Some("… (older lines pruned)"), &agent.feed[start..])
            } else {
                (None, &agent.feed[..])
            };
        let joined = lines_to_render.join("\n");
        // H3-closed tuple destructure. _link_refs is discarded — the
        // read-only feed does not route OSC8 clicks.
        let (mut rendered, _link_refs): (Vec<Line<'static>>, Vec<String>) =
            crate::tui::render::markdown::render_markdown_with_width(&joined, theme, area_width);
        if let Some(h) = header_line {
            rendered.insert(
                0,
                Line::from(vec![Span::styled(
                    h.to_string(),
                    Style::default().fg(theme.text_dim),
                )]),
            );
        }
        self.cached_lines = rendered;
        self.last_feed_len = agent.feed.len();
        self.cached_agent_id = Some(agent.id.clone());
    }

    /// Recompute the maximum scroll offset given the current viewport
    /// height + cached visual-row count. Saturating arithmetic so a
    /// short feed never produces a negative anchor.
    fn max_scroll(total_lines: u16, viewport_height: u16) -> u16 {
        total_lines.saturating_sub(viewport_height)
    }
}

impl Surface for AgentTranscriptSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::AgentTranscript
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        // No active agent (e.g. session reset cleared sub_agents) → render
        // nothing. Explicit no-panic per SPEC §1D acceptance.
        let Some(agent) = self.active_agent(app) else {
            return;
        };

        // Header is one line; the body fills the rest.
        let chunks = Layout::default()
            .direction(ratatui::layout::Direction::Vertical)
            .constraints([Constraint::Length(1), Constraint::Min(0)])
            .split(area);
        let header_area = chunks[0];
        let body_area = chunks[1];

        // Header — always rebuilt because it is cheap (1 line, few spans).
        let header = Self::build_header(agent, theme);
        frame.render_widget(Paragraph::new(header), header_area);

        // Body cache invalidation: agent id changed OR feed length changed.
        let cache_stale = self.cache_is_cold(&agent.id) || agent.feed.len() != self.last_feed_len;
        if cache_stale {
            self.rebuild_cache(agent, theme, body_area.width);
        }

        // Wrap-aware total: `Paragraph::line_count` walks the same wrapper
        // ratatui will use to render. Matches workspace.rs:1824 H1 fix.
        let para = Paragraph::new(self.cached_lines.clone()).wrap(Wrap { trim: false });
        let total = para.line_count(body_area.width) as u16;

        self.last_viewport_height = body_area.height;
        self.last_total_lines = total;

        // Sticky-bottom: if we are anchored at the bottom, recompute the
        // offset every frame so newly-appended lines stay in view.
        let max = Self::max_scroll(total, body_area.height);
        if self.is_at_bottom {
            self.scroll_offset = max;
        } else {
            // Clamp in case the feed shrank (e.g. session reset).
            self.scroll_offset = self.scroll_offset.min(max);
        }

        frame.render_widget(para.scroll((self.scroll_offset, 0)), body_area);
    }

    fn handle_key(&mut self, key: KeyEvent, _app: &mut App) -> SurfaceAction {
        let max = Self::max_scroll(self.last_total_lines, self.last_viewport_height);
        let page = self.last_viewport_height.max(1);
        match key.code {
            KeyCode::Esc => SurfaceAction::Pop,
            KeyCode::Up => {
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                if self.scroll_offset < max {
                    self.is_at_bottom = false;
                }
                SurfaceAction::None
            }
            KeyCode::Down => {
                self.scroll_offset = (self.scroll_offset + 1).min(max);
                if self.scroll_offset >= max {
                    self.is_at_bottom = true;
                }
                SurfaceAction::None
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_sub(page);
                if self.scroll_offset < max {
                    self.is_at_bottom = false;
                }
                SurfaceAction::None
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_add(page).min(max);
                if self.scroll_offset >= max {
                    self.is_at_bottom = true;
                }
                SurfaceAction::None
            }
            KeyCode::Home => {
                self.scroll_offset = 0;
                self.is_at_bottom = self.scroll_offset >= max;
                SurfaceAction::None
            }
            KeyCode::End => {
                self.scroll_offset = max;
                self.is_at_bottom = true;
                SurfaceAction::None
            }
            _ => SurfaceAction::None,
        }
    }

    fn on_enter(&mut self, _app: &mut App) {
        // Fresh entry into the transcript snaps to the bottom (the most
        // recent activity) — the v0.9.1.1 F3 sticky pattern.
        self.is_at_bottom = true;
    }

    fn restore_scroll(&mut self, offset: u16) {
        // Pop restoration: prior offset wins. Unset sticky so we honour
        // exactly where the user was before pushing AgentNav.
        self.scroll_offset = offset;
        self.is_at_bottom = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    use crate::tui::app::{App, SubAgentStatus, SubAgentView};
    use crate::tui::theme::Theme;

    // ── helpers ─────────────────────────────────────────────────────────

    /// Build a `SubAgentView` with the given feed lines. The id is
    /// derived from the name.
    fn agent(
        name: &str,
        status: SubAgentStatus,
        turns: usize,
        tokens: u64,
        feed: Vec<String>,
    ) -> SubAgentView {
        SubAgentView {
            id: format!("spawn:{name}"),
            name: name.into(),
            status,
            turns,
            tokens,
            feed,
        }
    }

    /// Build an `App` with one active agent and the transcript surface's
    /// id pointer set to it.
    fn app_with(agent: SubAgentView) -> App {
        let mut app = App::new();
        app.active_agent_transcript_id = Some(agent.id.clone());
        app.session.sub_agents = vec![agent];
        app
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Render the surface to a string for assertion. `w x h` are the
    /// `TestBackend` dimensions.
    fn render_str(surface: &mut AgentTranscriptSurface, app: &App, w: u16, h: u16) -> String {
        let theme = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), app, &theme))
            .expect("render agent_transcript surface");
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

    // ── W5.1 — header ───────────────────────────────────────────────────

    #[test]
    fn id_is_agent_transcript() {
        let s = AgentTranscriptSurface::default();
        assert_eq!(s.id(), SurfaceId::AgentTranscript);
    }

    #[test]
    fn header_running_signals_read_only_feed() {
        let a = agent(
            "scout",
            SubAgentStatus::Running,
            6,
            31_200,
            vec!["hi".into()],
        );
        let app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        let out = render_str(&mut s, &app, 100, 10);
        // UX-H9 explicit signal — composer absence is announced in the header.
        assert!(
            out.contains("Running — read-only feed"),
            "missing UX-H9 signal:\n{out}"
        );
        // Back-affordance
        assert!(out.contains("‹ Sub-Agents"), "missing back glyph:\n{out}");
        // Agent name
        assert!(out.contains("scout"), "missing agent name:\n{out}");
        // Counts
        assert!(out.contains("6 turns"), "missing turn count:\n{out}");
        assert!(out.contains("31200 tok"), "missing token count:\n{out}");
    }

    #[test]
    fn header_done_uses_done_label() {
        let a = agent("scribe", SubAgentStatus::Done, 8, 12_000, vec!["x".into()]);
        let app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        let out = render_str(&mut s, &app, 100, 10);
        assert!(out.contains("Done"), "missing Done label:\n{out}");
        // Should NOT carry the read-only-feed signal (only Running does).
        assert!(
            !out.contains("read-only feed"),
            "Done surface must NOT carry read-only-feed signal (composer-less inferred from Running only):\n{out}"
        );
    }

    #[test]
    fn header_failed_uses_failed_label() {
        let a = agent("probe", SubAgentStatus::Failed, 1, 200, vec!["x".into()]);
        let app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        let out = render_str(&mut s, &app, 100, 10);
        assert!(out.contains("Failed"), "missing Failed label:\n{out}");
    }

    #[test]
    fn no_active_agent_renders_nothing_no_panic() {
        // active_agent_transcript_id = None → render is a no-op.
        let app = App::new();
        let mut s = AgentTranscriptSurface::default();
        let out = render_str(&mut s, &app, 60, 5);
        // The backend buffer is filled with spaces only.
        assert!(
            out.chars().all(|c| c == ' ' || c == '\n'),
            "expected empty buffer (all spaces), got:\n{out}"
        );
    }

    #[test]
    fn stale_active_id_renders_nothing_no_panic() {
        // active_agent_transcript_id set but no agent with that id exists.
        let mut app = App::new();
        app.active_agent_transcript_id = Some("spawn:ghost".into());
        let mut s = AgentTranscriptSurface::default();
        let out = render_str(&mut s, &app, 60, 5);
        assert!(
            out.chars().all(|c| c == ' ' || c == '\n'),
            "expected empty buffer for stale id, got:\n{out}"
        );
    }

    // ── W5.2 — feed render + 5000 cap ───────────────────────────────────

    #[test]
    fn feed_join_and_rerender_multi_line_code_block() {
        // Multi-line code block must be `join`-ed before being handed to
        // the markdown renderer — otherwise each ` ``` ` lives on its own
        // line and the fence is never closed.
        let feed = vec![
            "```rust".into(),
            "fn hello() {".into(),
            "    println!(\"hi\");".into(),
            "}".into(),
            "```".into(),
        ];
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        let out = render_str(&mut s, &app, 80, 20);
        // The `▎` orange bar is the markdown renderer's code-block prefix.
        assert!(
            out.contains('▎'),
            "code block ▎ bar missing — feed-join failed:\n{out}"
        );
    }

    #[test]
    fn feed_exactly_5000_renders_without_pruned_header() {
        let feed: Vec<String> = (0..5000).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        let _ = render_str(&mut s, &app, 80, 10);
        // After render, cache should hold no pruned-header (5000 == cap).
        let first_line_text: String = s.cached_lines[0]
            .spans
            .iter()
            .map(|sp| sp.content.as_ref())
            .collect();
        assert!(
            !first_line_text.contains("older lines pruned"),
            "5000-line feed should NOT prune; first line was: {first_line_text:?}"
        );
    }

    #[test]
    fn feed_5001_prunes_oldest_with_header() {
        let feed: Vec<String> = (0..5001).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        let _ = render_str(&mut s, &app, 80, 10);
        // The cache's first rendered line is the pruned-header banner.
        let first_line_text: String = s.cached_lines[0]
            .spans
            .iter()
            .map(|sp| sp.content.as_ref())
            .collect();
        assert!(
            first_line_text.contains("older lines pruned"),
            "expected pruned-header on 5001-line feed; got first line: {first_line_text:?}"
        );
    }

    #[test]
    fn feed_10000_keeps_only_5000_most_recent() {
        let feed: Vec<String> = (0..10_000).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        let _ = render_str(&mut s, &app, 80, 10);
        // last_feed_len is the FULL length (cache key); pruning happens in
        // the rendered output. Verify by checking the cache's body contains
        // the LAST kept line ("line 9999") and DOES NOT contain a very old
        // line ("line 0").
        let body: String = s
            .cached_lines
            .iter()
            .flat_map(|l| l.spans.iter().map(|sp| sp.content.as_ref()))
            .collect();
        assert!(
            body.contains("line 9999"),
            "most recent line missing from rendered cache"
        );
        assert!(
            !body.contains("line 0\n") && !body.contains("line 0 "),
            "oldest line should have been pruned"
        );
        assert!(
            body.contains("older lines pruned"),
            "pruned-header missing on 10000-line feed"
        );
    }

    #[test]
    fn cache_is_reused_when_feed_unchanged() {
        let feed: Vec<String> = (0..100).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        let _ = render_str(&mut s, &app, 80, 10);
        let snapshot_len = s.cached_lines.len();
        let snapshot_first: String = s.cached_lines[0]
            .spans
            .iter()
            .map(|sp| sp.content.as_ref())
            .collect();
        // Re-render with same feed — cache must not be rebuilt (we observe
        // this via `last_feed_len` being the only allowed mutation
        // sentinel; values must stay identical).
        let _ = render_str(&mut s, &app, 80, 10);
        assert_eq!(s.cached_lines.len(), snapshot_len);
        let again_first: String = s.cached_lines[0]
            .spans
            .iter()
            .map(|sp| sp.content.as_ref())
            .collect();
        assert_eq!(snapshot_first, again_first);
        assert_eq!(s.last_feed_len, 100);
    }

    // ── W5.3 — auto-scroll-to-bottom ───────────────────────────────────

    #[test]
    fn on_enter_snaps_to_bottom() {
        let feed: Vec<String> = (0..50).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let mut app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        s.on_enter(&mut app);
        assert!(s.is_at_bottom, "on_enter must set is_at_bottom");
    }

    #[test]
    fn auto_scroll_when_feed_grows_and_sticky_is_on() {
        let feed: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let mut app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        s.on_enter(&mut app);
        let _ = render_str(&mut s, &app, 80, 10);
        let off_before = s.scroll_offset;
        assert!(
            off_before > 0,
            "with 200 lines in a 10-row viewport, sticky bottom must produce a nonzero offset"
        );
        // Grow the feed and re-render — offset must follow the bottom.
        app.session.sub_agents[0]
            .feed
            .extend((200..300).map(|i| format!("line {i}")));
        let _ = render_str(&mut s, &app, 80, 10);
        let off_after = s.scroll_offset;
        assert!(
            off_after > off_before,
            "sticky-bottom failed: offset before {off_before} after {off_after}"
        );
    }

    #[test]
    fn scrolling_up_unsticks_and_new_lines_append_silently() {
        let feed: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let mut app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        s.on_enter(&mut app);
        let _ = render_str(&mut s, &app, 80, 10);
        // User scrolls up — sticky must flip off.
        let _ = s.handle_key(key(KeyCode::Up), &mut app);
        let _ = s.handle_key(key(KeyCode::Up), &mut app);
        let _ = s.handle_key(key(KeyCode::Up), &mut app);
        assert!(!s.is_at_bottom, "Up must unstick sticky-bottom");
        let off_user = s.scroll_offset;
        // Feed grows; render again — offset must NOT jump to bottom.
        app.session.sub_agents[0]
            .feed
            .extend((200..300).map(|i| format!("line {i}")));
        let _ = render_str(&mut s, &app, 80, 10);
        // We allow offset to be clamped if the feed length grew, but it
        // must remain BELOW the new bottom anchor.
        let max = AgentTranscriptSurface::max_scroll(s.last_total_lines, s.last_viewport_height);
        assert!(
            s.scroll_offset < max,
            "after Up + feed growth, offset {off_user} must remain below max {max} (got {})",
            s.scroll_offset
        );
    }

    #[test]
    fn end_key_restores_sticky_bottom() {
        let feed: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let mut app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        s.on_enter(&mut app);
        let _ = render_str(&mut s, &app, 80, 10);
        let _ = s.handle_key(key(KeyCode::Up), &mut app);
        let _ = s.handle_key(key(KeyCode::Up), &mut app);
        assert!(!s.is_at_bottom);
        let _ = s.handle_key(key(KeyCode::End), &mut app);
        assert!(s.is_at_bottom, "End must restore sticky-bottom");
    }

    // ── W5.4 — keybinds ─────────────────────────────────────────────────

    #[test]
    fn esc_returns_pop() {
        let a = agent("scout", SubAgentStatus::Running, 1, 10, vec!["hi".into()]);
        let mut app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        let action = s.handle_key(key(KeyCode::Esc), &mut app);
        assert!(
            matches!(action, SurfaceAction::Pop),
            "Esc must return Pop; got {action:?}"
        );
    }

    #[test]
    fn arrow_up_scrolls_one_line() {
        let feed: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let mut app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        s.on_enter(&mut app);
        let _ = render_str(&mut s, &app, 80, 10);
        let before = s.scroll_offset;
        let _ = s.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(
            s.scroll_offset,
            before.saturating_sub(1),
            "Up must scroll by 1 line"
        );
    }

    #[test]
    fn page_up_scrolls_by_viewport_height() {
        let feed: Vec<String> = (0..400).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let mut app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        s.on_enter(&mut app);
        let _ = render_str(&mut s, &app, 80, 10);
        let before = s.scroll_offset;
        let page = s.last_viewport_height;
        assert!(page > 0);
        let _ = s.handle_key(key(KeyCode::PageUp), &mut app);
        assert_eq!(
            s.scroll_offset,
            before.saturating_sub(page),
            "PageUp must scroll by viewport height ({page})"
        );
    }

    #[test]
    fn home_jumps_to_top() {
        let feed: Vec<String> = (0..200).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let mut app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        s.on_enter(&mut app);
        let _ = render_str(&mut s, &app, 80, 10);
        let _ = s.handle_key(key(KeyCode::Home), &mut app);
        assert_eq!(s.scroll_offset, 0, "Home must jump to offset 0");
    }

    #[test]
    fn no_composer_rendered_while_running() {
        // The composer's signature placeholder is `Ask Wayland …` (the
        // workspace's input prompt). The transcript surface must never
        // emit anything that looks like a composer hint or input row.
        let feed: Vec<String> = (0..10).map(|i| format!("line {i}")).collect();
        let a = agent("scout", SubAgentStatus::Running, 1, 10, feed);
        let app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        let out = render_str(&mut s, &app, 100, 20);
        assert!(
            !out.contains("Ask Wayland"),
            "composer prompt leaked into transcript:\n{out}"
        );
        assert!(
            !out.contains("Ctrl+Enter"),
            "composer hint leaked into transcript:\n{out}"
        );
    }

    #[test]
    fn other_keys_are_no_op() {
        let a = agent("scout", SubAgentStatus::Running, 1, 10, vec!["hi".into()]);
        let mut app = app_with(a);
        let mut s = AgentTranscriptSurface::default();
        let action = s.handle_key(key(KeyCode::Char('z')), &mut app);
        assert!(matches!(action, SurfaceAction::None));
    }

    #[test]
    fn restore_scroll_sets_offset_and_unsticks() {
        let mut s = AgentTranscriptSurface {
            is_at_bottom: true,
            ..Default::default()
        };
        s.restore_scroll(42);
        assert_eq!(s.scroll_offset, 42);
        assert!(
            !s.is_at_bottom,
            "restore_scroll must unset sticky (caller wins)"
        );
    }
}
