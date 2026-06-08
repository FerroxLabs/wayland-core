//! v0.9.3 W3 — the ambient agent strip rendered above the workspace composer.
//!
//! Owns the 1-row "ambient awareness" widget per SPEC §1B: count by status,
//! `last:` tail, and the right-aligned `⌥A list` hint. Five render states:
//!
//! 1. Steady-state: `<topology>   <N> running [· <M> done] [· <S> stale]   last: <name> <glyph> <duration>          ⌥A list`
//! 2. K>0 failed: `⚠ <K> failed` promoted to first position (`Theme::error`).
//! 3. Spawning: `<topology>   spawning <N>…` (count in `Theme::orange`,
//!    pulsing at `AnimId::Spinner` cadence per UX-H3).
//! 4. All-done-within-30s-TTL: `<topology>   <N> done   completed in <dur>`.
//! 5. First-ever spawn (within 5s of `onboarding_state.first_spawn_seen`):
//!    expanded hint `⌥A — open agent list · ⏎ open · ⎋ back`.
//!
//! Staleness is computed on-demand from `App::agent_last_event` per H2 closure
//! — there is NO `SubAgentView::stale` field. The staleness window is 10min.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::tui::agents::glow::GlowFader;
use crate::tui::anim::AnimationClock;
use crate::tui::app::{App, SubAgentStatus, SubAgentView};
use crate::tui::theme::Theme;

/// Staleness threshold: a Running agent whose last event was longer ago than
/// this counts as stale (per SPEC §1B + Sec-H2). 10 minutes.
const STALE_THRESHOLD: Duration = Duration::from_secs(10 * 60);

/// 30-second TTL for the "all done" state (Sutherland: the disappearing
/// strip IS the signal — but only after a brief glow window per UX-H7).
const ALL_DONE_TTL: Duration = Duration::from_secs(30);

/// First-spawn expanded-hint window per SPEC §1B (UX-B2 fix).
const FIRST_SPAWN_HINT_WINDOW: Duration = Duration::from_secs(5);

/// Failed transition flash (UX-H2 option b): 250ms tinted background on
/// the 0→1 failed transition.
const FAILED_FLASH_WINDOW: Duration = Duration::from_millis(250);

/// Counts derived from a `SubAgentView` slice + the staleness map. Running
/// EXCLUDES stale agents per SPEC §1B — they're surfaced as `stale` only,
/// never double-counted.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct StripCounts {
    /// Fresh Running agents (last event within `STALE_THRESHOLD`).
    pub running: usize,
    /// Done agents.
    pub done: usize,
    /// Failed agents.
    pub failed: usize,
    /// Stale Running agents (last event older than `STALE_THRESHOLD`).
    pub stale: usize,
}

/// The agent strip. State carried frame-to-frame: per-tick `should_render`
/// cache, last seen failed-count (for the 0→1 flash), and the instant of the
/// last 0→1 failed transition (for the 250ms flash window).
#[derive(Default)]
pub struct AgentStrip {
    /// v1.1 cache — the last tick we evaluated `should_render` on. The render
    /// loop calls `should_render` once per tick on every layout pass, so the
    /// cache saves a redundant `.iter().any()` per pass.
    pub last_should_render_tick: u64,
    /// The cached value paired with `last_should_render_tick`.
    pub last_should_render_value: bool,
    /// Previous failed count — drives the 0→1 transition flash (UX-H2b).
    failed_count: usize,
    /// When the most recent 0→1 failed transition fired. `None` until the
    /// first failure. The flash renders for `FAILED_FLASH_WINDOW` after this.
    failed_flash_at: Option<Instant>,
}

impl AgentStrip {
    /// Per-tick cached eligibility for mounting the strip. Returns `true` if
    /// the strip should consume a 1-row slot above the composer this frame.
    ///
    /// Eligibility:
    /// - Any sub-agent in `app.session.sub_agents`, OR
    /// - Any agent still inside the 30s done-glow window (the strip lingers
    ///   for the all-done TTL even after the SubAgentView vector drains —
    ///   bridge does not currently drain it, but cap the rule against
    ///   future drain so the SPEC §2B "strip TTL unmount" rule is well-defined).
    pub fn should_render(&mut self, app: &App, tick: u64) -> bool {
        if tick == self.last_should_render_tick && tick != 0 {
            return self.last_should_render_value;
        }
        let v = !app.session.sub_agents.is_empty() || app.agent_glow.any_active(Instant::now());
        self.last_should_render_tick = tick;
        self.last_should_render_value = v;
        v
    }

    /// Count agents into `StripCounts`. Running excludes stale; stale is
    /// derived on-demand from `agent_last_event` per H2 closure.
    ///
    /// `glow` is reserved for future "done-within-TTL" gating but is not
    /// currently consulted (counts are status-pure; the glow drives render
    /// color, not the count). Kept in the signature to match the PLAN test.
    pub fn count_by_status(
        agents: &[SubAgentView],
        agent_last_event: &HashMap<String, Instant>,
        now: Instant,
        _glow: &GlowFader,
    ) -> StripCounts {
        let mut c = StripCounts::default();
        for a in agents {
            match a.status {
                SubAgentStatus::Running => {
                    let is_stale = agent_last_event
                        .get(&a.id)
                        .map(|t| now.duration_since(*t) > STALE_THRESHOLD)
                        .unwrap_or(false);
                    if is_stale {
                        c.stale += 1;
                    } else {
                        c.running += 1;
                    }
                }
                SubAgentStatus::Done => c.done += 1,
                SubAgentStatus::Failed => c.failed += 1,
            }
        }
        c
    }

    /// The most-recently-finished agent (Done or Failed). Used for the
    /// `last: <name> <glyph> <duration>` tail in the steady-state render.
    /// Returns the agent + the moment its last event was recorded (so the
    /// caller can compute the wall-clock since-completion duration).
    pub fn last_terminal_agent<'a>(
        agents: &'a [SubAgentView],
        agent_last_event: &HashMap<String, Instant>,
    ) -> Option<(&'a SubAgentView, Instant)> {
        agents
            .iter()
            .filter(|a| matches!(a.status, SubAgentStatus::Done | SubAgentStatus::Failed))
            .filter_map(|a| agent_last_event.get(&a.id).map(|t| (a, *t)))
            .max_by_key(|(_, t)| *t)
    }

    /// Render the strip into `area` (1 row tall). Five-state machine per
    /// SPEC §1B. A 0-tall area renders nothing (defensive: the workspace
    /// reserves Length(0) when `should_render` returns false).
    pub fn render(
        &mut self,
        frame: &mut Frame,
        area: Rect,
        app: &App,
        theme: &Theme,
        clock: &AnimationClock,
    ) {
        if area.width == 0 || area.height == 0 {
            return;
        }
        let now = Instant::now();
        let counts = Self::count_by_status(
            &app.session.sub_agents,
            &app.agent_last_event,
            now,
            &app.agent_glow,
        );

        // Failed-flash tracking: on 0→1 transition, record the instant; the
        // render path reads `failed_flash_at` to decide if the background
        // tint is still inside the 250ms window. Increases beyond 1 do NOT
        // re-fire the flash (per SPEC §2B — "0→1 transition").
        if counts.failed > 0 && self.failed_count == 0 {
            self.failed_flash_at = Some(now);
        }
        self.failed_count = counts.failed;
        let flash_active = self
            .failed_flash_at
            .map(|t| now.duration_since(t) < FAILED_FLASH_WINDOW)
            .unwrap_or(false);

        let topology = topology_label(app.session.sub_agents.len());
        let total = app.session.sub_agents.len();
        let all_terminal = total > 0 && counts.running == 0 && counts.stale == 0;

        // ── State selector ────────────────────────────────────────────
        // 1. First-ever spawn (within 5s of first_spawn_seen) → expanded hint
        //    on top of the spawning layout.
        // 2. Spawning (any Running + zero Done + zero Failed) → "spawning N…"
        //    with the count pulsing at Spinner cadence (UX-H3).
        // 3. All-done-within-TTL (every agent terminal) → "N done · completed
        //    in <dur>" with the count glowing for 30s.
        // 4. Steady-state otherwise.
        let first_spawn_active = app
            .onboarding_state
            .first_spawn_seen
            .map(|t| now.duration_since(t) < FIRST_SPAWN_HINT_WINDOW)
            .unwrap_or(false);
        let is_spawning = total > 0 && counts.done == 0 && counts.failed == 0 && counts.stale == 0;

        // `right_hint` is the right-aligned hint segment, returned as Spans.
        let right_hint = if first_spawn_active {
            expanded_hint_spans(theme)
        } else {
            compact_hint_spans(theme)
        };

        // Build the left-half content per state.
        let mut left: Vec<Span<'static>> = Vec::new();

        if is_spawning {
            // Hero spawning state.
            left.push(Span::styled(
                topology.to_string(),
                Style::default().fg(theme.orange),
            ));
            left.push(Span::raw("   "));
            left.push(Span::styled(
                "spawning ".to_string(),
                Style::default().fg(theme.text_muted),
            ));
            // Pulse the count at Spinner cadence — even tick = bright, odd =
            // muted. (The animation clock advances ~30fps; this reads as a
            // soft pulse, not a flicker.)
            let pulsing = clock.wants_tick() && app.frame_tick.is_multiple_of(2);
            let count_color = if pulsing {
                theme.orange
            } else {
                theme.orange_muted
            };
            left.push(Span::styled(
                format!("{}", total),
                Style::default()
                    .fg(count_color)
                    .add_modifier(Modifier::BOLD),
            ));
            left.push(Span::styled(
                "…".to_string(),
                Style::default().fg(theme.text_muted),
            ));
        } else if all_terminal {
            // "completed in <dur>" state.
            left.push(Span::styled(
                topology.to_string(),
                Style::default().fg(theme.orange),
            ));
            left.push(Span::raw("   "));
            if counts.failed > 0 {
                left.push(Span::styled(
                    format!("⚠ {} failed", counts.failed),
                    Style::default().fg(theme.error),
                ));
                if counts.done > 0 {
                    left.push(Span::styled(
                        " · ".to_string(),
                        Style::default().fg(theme.text_muted),
                    ));
                }
            }
            if counts.done > 0 {
                left.push(Span::styled(
                    format!("{} done", counts.done),
                    Style::default()
                        .fg(theme.orange_muted)
                        .add_modifier(Modifier::BOLD),
                ));
            }
            // Compute the wall-clock duration from the OLDEST first-seen to
            // the LATEST terminal event. Best-effort: agent_last_event tracks
            // the LAST event per agent, so the longest "since" amongst all
            // terminal agents is the closest stand-in for the start time.
            if let Some(longest) = app
                .session
                .sub_agents
                .iter()
                .filter_map(|a| app.agent_last_event.get(&a.id))
                .min()
                .copied()
            {
                let dur = now.duration_since(longest);
                left.push(Span::styled(
                    format!("   completed in {}", format_duration(dur)),
                    Style::default().fg(theme.text_muted),
                ));
            }
        } else {
            // Steady-state.
            left.push(Span::styled(
                topology.to_string(),
                Style::default().fg(theme.orange),
            ));
            left.push(Span::raw("   "));

            // Failure first when K>0 (UX-H2).
            let mut first_segment = true;
            if counts.failed > 0 {
                left.push(Span::styled(
                    format!("⚠ {} failed", counts.failed),
                    Style::default().fg(theme.error),
                ));
                first_segment = false;
            }
            if counts.running > 0 {
                if !first_segment {
                    left.push(Span::styled(
                        " · ".to_string(),
                        Style::default().fg(theme.text_muted),
                    ));
                }
                left.push(Span::styled(
                    format!("{} running", counts.running),
                    Style::default().fg(theme.text_muted),
                ));
                first_segment = false;
            }
            if counts.done > 0 {
                if !first_segment {
                    left.push(Span::styled(
                        " · ".to_string(),
                        Style::default().fg(theme.text_muted),
                    ));
                }
                left.push(Span::styled(
                    format!("{} done", counts.done),
                    Style::default().fg(theme.text_muted),
                ));
                first_segment = false;
            }
            if counts.stale > 0 {
                if !first_segment {
                    left.push(Span::styled(
                        " · ".to_string(),
                        Style::default().fg(theme.text_muted),
                    ));
                }
                left.push(Span::styled(
                    format!("{} stale", counts.stale),
                    Style::default().fg(theme.text_muted),
                ));
            }

            // `last: <name> <glyph> <duration>` tail.
            if let Some((agent, terminal_at)) =
                Self::last_terminal_agent(&app.session.sub_agents, &app.agent_last_event)
            {
                let dur = now.duration_since(terminal_at);
                let glyph = match agent.status {
                    SubAgentStatus::Done => "✓",
                    SubAgentStatus::Failed => "✗",
                    SubAgentStatus::Running => "◐",
                };
                let glyph_color = match agent.status {
                    SubAgentStatus::Done => {
                        // ≤30s glow: orange_muted; >30s: text_dim.
                        if dur < ALL_DONE_TTL {
                            theme.orange_muted
                        } else {
                            theme.text_dim
                        }
                    }
                    SubAgentStatus::Failed => theme.error,
                    SubAgentStatus::Running => theme.text_running,
                };
                left.push(Span::styled(
                    "   last: ".to_string(),
                    Style::default().fg(theme.text_muted),
                ));
                let name = truncate_name(&agent.name, 24);
                left.push(Span::styled(name, Style::default().fg(theme.text_muted)));
                left.push(Span::raw(" "));
                left.push(Span::styled(
                    glyph.to_string(),
                    Style::default().fg(glyph_color),
                ));
                left.push(Span::raw(" "));
                left.push(Span::styled(
                    format_duration(dur),
                    Style::default().fg(theme.text_muted),
                ));
            }
        }

        // ── Composition ───────────────────────────────────────────────
        // The right hint sits flush-right inside the area; the left content
        // sits flush-left. The width of the right hint is known (its visible
        // text); the left takes the remaining space.
        let right_text_width: u16 = right_hint
            .iter()
            .map(|s| s.content.chars().count() as u16)
            .sum();
        let right_width = right_text_width.min(area.width);
        let left_width = area.width.saturating_sub(right_width);

        let [left_area, right_area] = Layout::horizontal([
            Constraint::Length(left_width),
            Constraint::Length(right_width),
        ])
        .areas(area);

        // The 250ms failed-flash tints the whole strip background with
        // `Theme::error` at low alpha. ratatui doesn't expose alpha, so we
        // approximate with a SET background to error; the eye reads this
        // as the flash regardless of duration.
        let base_style = if flash_active {
            Style::default().bg(theme.error)
        } else {
            Style::default()
        };

        // A leading 2-space gutter mirrors the SPEC mockup's "  <topology>"
        // alignment. We render the left content with that gutter prepended
        // when there's room; on a tight area we drop the gutter.
        let mut left_with_gutter: Vec<Span<'static>> = Vec::with_capacity(left.len() + 1);
        if left_width > 2 {
            left_with_gutter.push(Span::raw("  "));
        }
        left_with_gutter.extend(left);

        frame.render_widget(
            Paragraph::new(Line::from(left_with_gutter)).style(base_style),
            left_area,
        );
        frame.render_widget(
            Paragraph::new(Line::from(right_hint)).style(base_style),
            right_area,
        );
    }
}

/// Topology label by agent count (SPEC §1B).
fn topology_label(n: usize) -> &'static str {
    match n {
        0..=5 => "Spawn",
        6..=20 => "Swarm",
        21..=50 => "Mesh",
        _ => "Fleet",
    }
}

/// Format an elapsed duration per UX-L4: `<60s → Ns`, `60-3599s → NmNs`,
/// `≥3600s → NhNm`.
fn format_duration(d: Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{}s", secs)
    } else if secs < 3600 {
        format!("{}m{}s", secs / 60, secs % 60)
    } else {
        format!("{}h{}m", secs / 3600, (secs % 3600) / 60)
    }
}

/// Truncate `name` to a max of `max_chars` characters, appending `…` if cut.
fn truncate_name(name: &str, max_chars: usize) -> String {
    let count = name.chars().count();
    if count <= max_chars {
        name.to_string()
    } else {
        let cut: String = name.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{}…", cut)
    }
}

/// Compact right-hint: `⌥A list` (⌥A orange, ` list` muted).
fn compact_hint_spans(theme: &Theme) -> Vec<Span<'static>> {
    vec![
        Span::styled("⌥A".to_string(), Style::default().fg(theme.orange)),
        Span::styled(" list".to_string(), Style::default().fg(theme.text_muted)),
    ]
}

/// Expanded first-spawn hint: `⌥A — open agent list · ⏎ open · ⎋ back`.
fn expanded_hint_spans(theme: &Theme) -> Vec<Span<'static>> {
    vec![
        Span::styled("⌥A".to_string(), Style::default().fg(theme.orange)),
        Span::styled(
            " — open agent list · ".to_string(),
            Style::default().fg(theme.text_muted),
        ),
        Span::styled("⏎".to_string(), Style::default().fg(theme.orange)),
        Span::styled(
            " open · ".to_string(),
            Style::default().fg(theme.text_muted),
        ),
        Span::styled("⎋".to_string(), Style::default().fg(theme.orange)),
        Span::styled(" back".to_string(), Style::default().fg(theme.text_muted)),
    ]
}

// ─────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

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

    // ── W3.1 — count + cache ─────────────────────────────────────────────

    #[test]
    fn strip_counts_by_status_excludes_stale_from_running_v093() {
        let agents = vec![
            sub("a", SubAgentStatus::Running),
            sub("b", SubAgentStatus::Running), // stale
            sub("c", SubAgentStatus::Done),
            sub("d", SubAgentStatus::Failed),
        ];
        let now = Instant::now();
        let mut last_event = HashMap::new();
        last_event.insert("a".to_string(), now);
        last_event.insert("b".to_string(), now - Duration::from_secs(11 * 60));
        last_event.insert("c".to_string(), now);
        last_event.insert("d".to_string(), now);
        let glow = GlowFader::default();
        let counts = AgentStrip::count_by_status(&agents, &last_event, now, &glow);
        assert_eq!(counts.running, 1, "stale running excluded");
        assert_eq!(counts.stale, 1);
        assert_eq!(counts.done, 1);
        assert_eq!(counts.failed, 1);
    }

    #[test]
    fn should_render_caches_per_tick_v093() {
        let mut strip = AgentStrip::default();
        let mut app = App::default();
        assert!(!strip.should_render(&app, 1));
        // Adding an agent in next tick should re-evaluate.
        app.session
            .sub_agents
            .push(sub("a", SubAgentStatus::Running));
        assert!(strip.should_render(&app, 2));
        // Same tick = cache hit (no recompute).
        // Mutate the field that should_render would consult; if cache
        // is honored, the value won't change.
        app.session.sub_agents.clear();
        assert!(strip.should_render(&app, 2), "tick=2 must hit cache");
        // Next tick — re-evaluate, now false.
        assert!(!strip.should_render(&app, 3));
    }

    #[test]
    fn count_by_status_with_no_event_treats_running_as_fresh() {
        // Defensive: an agent with no entry in the map (e.g. a freshly-pushed
        // SubAgentView before the bridge gets its first event in) is treated
        // as fresh, not stale.
        let agents = vec![sub("a", SubAgentStatus::Running)];
        let now = Instant::now();
        let last_event = HashMap::new();
        let glow = GlowFader::default();
        let counts = AgentStrip::count_by_status(&agents, &last_event, now, &glow);
        assert_eq!(counts.running, 1);
        assert_eq!(counts.stale, 0);
    }

    // ── W3.2 — render ────────────────────────────────────────────────────

    fn render_strip_to_string(app: &App, w: u16) -> String {
        let theme = Theme::no_color();
        let mut strip = AgentStrip::default();
        let mut terminal = Terminal::new(TestBackend::new(w, 1)).expect("test terminal");
        terminal
            .draw(|f| {
                let area = f.area();
                strip.render(f, area, app, &theme, &app.anim);
            })
            .expect("render strip");
        let buf = terminal.backend().buffer();
        // Flatten row 0.
        (0..w)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn render_steady_state_shows_topology_and_running_count() {
        let mut app = App::default();
        let now = Instant::now();
        app.session
            .sub_agents
            .push(sub("alpha", SubAgentStatus::Running));
        app.session
            .sub_agents
            .push(sub("beta", SubAgentStatus::Done));
        app.agent_last_event.insert("alpha".to_string(), now);
        app.agent_last_event.insert("beta".to_string(), now);
        // Push first_spawn_seen far enough in the past that the expanded
        // hint window is closed.
        app.onboarding_state.first_spawn_seen = Some(now - Duration::from_secs(60));
        let text = render_strip_to_string(&app, 100);
        assert!(text.contains("Spawn"), "topology label missing: {:?}", text);
        assert!(
            text.contains("1 running"),
            "running count missing: {:?}",
            text
        );
        assert!(text.contains("1 done"), "done count missing: {:?}", text);
        assert!(text.contains("last:"), "last tail missing: {:?}", text);
        assert!(text.contains("⌥A"), "right hint missing: {:?}", text);
    }

    #[test]
    fn render_promotes_failed_count_to_first_position() {
        let mut app = App::default();
        let now = Instant::now();
        app.session
            .sub_agents
            .push(sub("alpha", SubAgentStatus::Running));
        app.session
            .sub_agents
            .push(sub("beta", SubAgentStatus::Failed));
        app.agent_last_event.insert("alpha".to_string(), now);
        app.agent_last_event.insert("beta".to_string(), now);
        app.onboarding_state.first_spawn_seen = Some(now - Duration::from_secs(60));
        let text = render_strip_to_string(&app, 100);
        let failed_idx = text.find("failed").expect("'failed' substring");
        let running_idx = text.find("running").expect("'running' substring");
        assert!(
            failed_idx < running_idx,
            "failed must precede running: text={:?}",
            text
        );
        assert!(text.contains("⚠"), "warning glyph missing: {:?}", text);
    }

    #[test]
    fn render_spawning_state_shows_spawning_label() {
        let mut app = App::default();
        let now = Instant::now();
        app.session
            .sub_agents
            .push(sub("alpha", SubAgentStatus::Running));
        app.session
            .sub_agents
            .push(sub("beta", SubAgentStatus::Running));
        app.agent_last_event.insert("alpha".to_string(), now);
        app.agent_last_event.insert("beta".to_string(), now);
        // first_spawn_seen well in the past so we get the compact hint,
        // not the expanded hint.
        app.onboarding_state.first_spawn_seen = Some(now - Duration::from_secs(60));
        let text = render_strip_to_string(&app, 100);
        // The "spawning" state activates because no Done + no Failed yet.
        assert!(
            text.contains("spawning"),
            "spawning label missing: {:?}",
            text
        );
        assert!(text.contains("2"), "count missing: {:?}", text);
        assert!(text.contains("…"), "ellipsis missing: {:?}", text);
        // Compact hint, not expanded.
        assert!(text.contains("⌥A"), "compact hint missing: {:?}", text);
        assert!(
            !text.contains("open agent list"),
            "expanded hint should be closed: {:?}",
            text
        );
    }

    #[test]
    fn render_all_done_state_shows_completed_in_duration() {
        let mut app = App::default();
        let now = Instant::now();
        app.session
            .sub_agents
            .push(sub("alpha", SubAgentStatus::Done));
        app.session
            .sub_agents
            .push(sub("beta", SubAgentStatus::Done));
        // Both terminal 5 seconds ago.
        app.agent_last_event
            .insert("alpha".to_string(), now - Duration::from_secs(5));
        app.agent_last_event
            .insert("beta".to_string(), now - Duration::from_secs(5));
        app.onboarding_state.first_spawn_seen = Some(now - Duration::from_secs(60));
        let text = render_strip_to_string(&app, 100);
        assert!(text.contains("2 done"), "done count missing: {:?}", text);
        assert!(
            text.contains("completed in"),
            "completion label missing: {:?}",
            text
        );
        assert!(text.contains("5s"), "duration missing: {:?}", text);
    }

    #[test]
    fn render_first_spawn_shows_expanded_hint() {
        let mut app = App::default();
        let now = Instant::now();
        app.session
            .sub_agents
            .push(sub("alpha", SubAgentStatus::Running));
        app.agent_last_event.insert("alpha".to_string(), now);
        // first_spawn_seen was JUST now → inside the 5s window.
        app.onboarding_state.first_spawn_seen = Some(now);
        let text = render_strip_to_string(&app, 100);
        assert!(
            text.contains("open agent list"),
            "expanded hint missing: {:?}",
            text
        );
        assert!(text.contains("⏎"), "enter glyph missing: {:?}", text);
        assert!(text.contains("⎋"), "esc glyph missing: {:?}", text);
    }

    #[test]
    fn render_zero_area_is_a_no_op() {
        // Defensive: workspace reserves Length(0) when should_render is
        // false; render(area-with-0-rows) must not panic.
        let app = App::default();
        let theme = Theme::no_color();
        let mut strip = AgentStrip::default();
        let mut terminal = Terminal::new(TestBackend::new(80, 1)).expect("test terminal");
        terminal
            .draw(|f| {
                let zero = Rect::new(0, 0, 80, 0);
                strip.render(f, zero, &app, &theme, &app.anim);
            })
            .expect("zero-area render must not panic");
    }

    #[test]
    fn topology_label_buckets_correctly() {
        assert_eq!(topology_label(0), "Spawn");
        assert_eq!(topology_label(5), "Spawn");
        assert_eq!(topology_label(6), "Swarm");
        assert_eq!(topology_label(20), "Swarm");
        assert_eq!(topology_label(21), "Mesh");
        assert_eq!(topology_label(50), "Mesh");
        assert_eq!(topology_label(51), "Fleet");
        assert_eq!(topology_label(100), "Fleet");
    }

    #[test]
    fn format_duration_matches_ux_l4() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
        assert_eq!(format_duration(Duration::from_secs(59)), "59s");
        assert_eq!(format_duration(Duration::from_secs(60)), "1m0s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m5s");
        assert_eq!(format_duration(Duration::from_secs(3599)), "59m59s");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h0m");
        assert_eq!(format_duration(Duration::from_secs(7325)), "2h2m");
    }

    #[test]
    fn truncate_name_truncates_long_names_with_ellipsis() {
        assert_eq!(truncate_name("short", 24), "short");
        assert_eq!(
            truncate_name("this-name-is-way-too-long-for-a-row", 24),
            "this-name-is-way-too-lo…"
        );
        // Boundary: exactly max chars is untouched.
        assert_eq!(
            truncate_name("aaaaaaaaaaaaaaaaaaaaaaaa", 24),
            "aaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn last_terminal_agent_returns_the_most_recent_terminal() {
        let agents = vec![
            sub("a", SubAgentStatus::Done),
            sub("b", SubAgentStatus::Failed),
            sub("c", SubAgentStatus::Running),
        ];
        let now = Instant::now();
        let mut last_event = HashMap::new();
        last_event.insert("a".to_string(), now - Duration::from_secs(10));
        last_event.insert("b".to_string(), now - Duration::from_secs(3));
        last_event.insert("c".to_string(), now);
        let (agent, _) =
            AgentStrip::last_terminal_agent(&agents, &last_event).expect("a terminal exists");
        assert_eq!(agent.id, "b", "most recent terminal is 'b'");
    }

    #[test]
    fn failed_flash_fires_only_on_zero_to_one_transition() {
        // Render once with K=0 (steady-state), then again with K=1 — the
        // strip should record a failed_flash_at on the second pass.
        let mut app = App::default();
        let now = Instant::now();
        app.session
            .sub_agents
            .push(sub("alpha", SubAgentStatus::Running));
        app.agent_last_event.insert("alpha".to_string(), now);
        app.onboarding_state.first_spawn_seen = Some(now - Duration::from_secs(60));

        let theme = Theme::no_color();
        let mut strip = AgentStrip::default();
        let mut terminal = Terminal::new(TestBackend::new(80, 1)).expect("test terminal");

        // Pass 1: no failures.
        terminal
            .draw(|f| strip.render(f, f.area(), &app, &theme, &app.anim))
            .expect("render");
        assert!(
            strip.failed_flash_at.is_none(),
            "no flash before any failure"
        );

        // Pass 2: introduce a failure.
        app.session
            .sub_agents
            .push(sub("beta", SubAgentStatus::Failed));
        app.agent_last_event
            .insert("beta".to_string(), Instant::now());
        terminal
            .draw(|f| strip.render(f, f.area(), &app, &theme, &app.anim))
            .expect("render");
        assert!(
            strip.failed_flash_at.is_some(),
            "0→1 transition must arm the flash"
        );
        let first_flash = strip.failed_flash_at;

        // Pass 3: still 1 failure — flash_at should NOT advance (it's the
        // 0→1 transition that fires, not every render with K>0).
        terminal
            .draw(|f| strip.render(f, f.area(), &app, &theme, &app.anim))
            .expect("render");
        assert_eq!(
            strip.failed_flash_at, first_flash,
            "K=1→K=1 must not re-arm the flash"
        );
    }
}
