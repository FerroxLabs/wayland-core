//! Top chrome row — the brand wordmark plus the inline tab strip.
//!
//! The hybrid-branding decision: the full WAYLAND ASCII banner is the
//! hero on the onboarding intro and the idle workspace, but once the user
//! is working that art would waste a sixth of the screen. This header is
//! its compact counterpart — a single row, painted as the top row of
//! every surface.
//!
//! The chrome redesign collapsed what used to be two stacked rows (a
//! stats strip + a tab row) into ONE row: the `◆ WAYLAND` wordmark on the
//! left, the surface tabs inline after it. Live stats (provider·model,
//! ctx, cost, cpu, ram) moved OUT of the header — they live ONLY in the
//! bottom [`status_bar`](super::status_bar), so nothing is duplicated.
//!
//! ## System sampling
//!
//! [`SystemSampler`] / [`SystemSample`] live in this module as the
//! shared CPU/RAM probe; the bottom status bar owns a `SystemSampler` and
//! reads a [`SystemSample`] from it each frame. The sampler refreshes at
//! most once per second and caches the last sample, so a ~30fps render
//! loop pays the `sysinfo` probe cost only ~1×/s.

use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use sysinfo::{MINIMUM_CPU_UPDATE_INTERVAL, System};

use crate::tui::surfaces::SurfaceId;
use crate::tui::theme::Theme;

/// Minimum wall-clock gap between two `sysinfo` refreshes. The render
/// loop ticks at ~30fps; sampling at 1Hz keeps the probe cost negligible
/// and is also the floor `sysinfo` documents for a meaningful CPU delta.
const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);

/// One cached snapshot of system load — what the header paints.
#[derive(Debug, Clone, Copy)]
pub struct SystemSample {
    /// Whole-system CPU usage as a percentage in `0.0..=100.0`.
    pub cpu_pct: f32,
    /// Resident memory in use, in bytes.
    pub used_mem: u64,
    /// Total physical memory, in bytes.
    pub total_mem: u64,
}

impl SystemSample {
    /// Memory usage as a percentage in `0.0..=100.0`. Zero when the total
    /// is unknown (avoids a divide-by-zero on an unusual host).
    fn mem_pct(&self) -> f32 {
        if self.total_mem == 0 {
            0.0
        } else {
            (self.used_mem as f32 / self.total_mem as f32) * 100.0
        }
    }
}

/// An interval-throttled sampler over `sysinfo`.
///
/// The render loop holds one `SystemSampler`. Every frame it calls
/// [`sample`](Self::sample), which returns the cached [`SystemSample`]
/// untouched unless [`SAMPLE_INTERVAL`] has elapsed — only then does it
/// pay the `sysinfo` refresh cost. This keeps a 30fps loop from probing
/// the OS 30×/s for a value that changes meaningfully ~1×/s.
pub struct SystemSampler {
    /// The `sysinfo` handle. Only its CPU + memory views are refreshed.
    system: System,
    /// When the cached sample was last refreshed.
    last_refresh: Instant,
    /// The most recent sample — returned as-is until the interval lapses.
    cached: SystemSample,
}

impl Default for SystemSampler {
    fn default() -> Self {
        Self::new()
    }
}

impl SystemSampler {
    /// Build a sampler and take an initial reading.
    ///
    /// CPU usage is a delta between two refreshes, so the constructor
    /// refreshes once, waits the documented minimum interval, then
    /// refreshes again — the first `sample()` call therefore returns a
    /// real CPU figure rather than a `0.0` placeholder.
    pub fn new() -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();
        // `sysinfo` needs a gap between the two CPU reads it diffs; its
        // own constant is the documented floor.
        std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
        system.refresh_cpu_usage();
        system.refresh_memory();
        let cached = read_sample(&system);
        Self {
            system,
            last_refresh: Instant::now(),
            cached,
        }
    }

    /// Return the current system sample.
    ///
    /// Cheap on the common path: it hands back the cached value and does
    /// nothing else. Only when [`SAMPLE_INTERVAL`] has elapsed since the
    /// last refresh does it re-probe `sysinfo` and update the cache.
    pub fn sample(&mut self) -> SystemSample {
        if self.last_refresh.elapsed() >= SAMPLE_INTERVAL {
            self.system.refresh_cpu_usage();
            self.system.refresh_memory();
            self.cached = read_sample(&self.system);
            self.last_refresh = Instant::now();
        }
        self.cached
    }
}

/// Read a [`SystemSample`] out of an already-refreshed `System`.
fn read_sample(system: &System) -> SystemSample {
    SystemSample {
        cpu_pct: system.global_cpu_usage(),
        used_mem: system.used_memory(),
        total_mem: system.total_memory(),
    }
}

/// Render the top chrome row into `area` — a single row.
///
/// `area` must be exactly one row tall; the caller (`Router::render`)
/// allocates it as a `Length(1)` strip at the top of the screen.
/// Contents, left to right: the `◆ WAYLAND` wordmark, then the surface
/// tabs inline. No live stats — provider·model / ctx / cost / cpu / ram
/// all live in the bottom [`status_bar`](super::status_bar) instead, so
/// nothing is duplicated.
///
/// `selected` is the index of the active tab within [`SurfaceId::TABS`];
/// it is clamped against the tab count so an out-of-range index (e.g. a
/// non-tab surface) never panics.
pub fn top_chrome(f: &mut Frame, area: Rect, t: &Theme, selected: usize) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    // The whole row sits on the same `surface` background as the bottom
    // status bar, so the two frame the working area symmetrically.
    let bar_style = Style::default().bg(t.bg).fg(t.text_dim);

    let mut spans: Vec<Span> = Vec::new();

    // ── Wordmark ──────────────────────────────────────────────────────
    // A leading space, the diamond + wordmark, then a trailing pad so the
    // brand never butts the first tab.
    //
    // v0.9.1.3 J: demoted from `t.orange` to `t.text` (bold) per test
    // agent 8's accent-inflation finding — the wordmark was 1 of 10+
    // orange surfaces vs. recon §1.4 budget of 2. Bold text on the bar
    // bg keeps the brand identity readable; the orange budget is now
    // reserved for the active-tab underline and the user-turn `▌`
    // (load-bearing accents only).
    spans.push(Span::styled(
        "  ◆ WAYLAND   ",
        Style::default()
            .bg(t.bg)
            .fg(t.text)
            .add_modifier(Modifier::BOLD),
    ));

    // ── Inline tabs ───────────────────────────────────────────────────
    // The active tab is the brand accent + bold + UNDERLINED so it reads
    // at a glance — color alone wasn't enough emphasis on a dim terminal.
    // Three spaces of gutter between tabs keeps the row from feeling
    // cramped.
    //
    // v0.9.1.1 MED: at narrow widths, the longest label ("Diagnostics")
    // used to be hard-truncated mid-word by the row clip ("Diagno"). We
    // now soft-truncate each label with an explicit `…` so the user
    // sees the labels are abbreviated, never just chopped.
    let active = selected.min(SurfaceId::TABS.len() - 1);
    // Budget: wordmark uses ~16 cols ("  ◆ WAYLAND   ") + 3-col gutters
    // between the 6 tabs (= 5 gutters · 3 cols = 15 cols). Whatever is
    // left is the tab-label budget. Floor at the minimum useful per-tab
    // width of 2 cols so we always emit at least one glyph + the
    // ellipsis on absurdly narrow terminals.
    const WORDMARK_COLS: u16 = 16;
    const GUTTER_COLS: u16 = 3;
    let tabs_count = SurfaceId::TABS.len() as u16;
    let gutter_total = GUTTER_COLS.saturating_mul(tabs_count.saturating_sub(1));
    let label_budget_total = area
        .width
        .saturating_sub(WORDMARK_COLS)
        .saturating_sub(gutter_total);
    let per_label_budget = (label_budget_total / tabs_count.max(1)) as usize;
    for (i, surface) in SurfaceId::TABS.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled("   ", bar_style));
        }
        let style = if i == active {
            Style::default()
                .bg(t.bg)
                .fg(t.orange)
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            Style::default().bg(t.bg).fg(t.text_dim)
        };
        let label = truncate_with_ellipsis(surface.title(), per_label_budget);
        spans.push(Span::styled(label, style));
    }

    let bar = Paragraph::new(Line::from(spans)).style(bar_style);
    f.render_widget(bar, area);
}

/// v0.9.1.1 MED: truncate a tab label to `max_cols` columns, appending
/// `…` when the full label would not fit. A `max_cols` of 0 or 1 falls
/// back to the raw label (no useful preview is possible at that width;
/// the row clip then takes over and the label is hard-truncated, which
/// is the only sensible behaviour for sub-2-column budgets).
fn truncate_with_ellipsis(label: &str, max_cols: usize) -> String {
    if max_cols < 2 {
        return label.to_string();
    }
    let chars: Vec<char> = label.chars().collect();
    if chars.len() <= max_cols {
        return label.to_string();
    }
    let mut out: String = chars.iter().take(max_cols.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use super::*;
    use crate::tui::theme::Theme;

    /// Render the top chrome row and flatten the single row to a string.
    fn render(t: &Theme, w: u16, selected: usize) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, 1)).expect("test terminal");
        terminal
            .draw(|f| top_chrome(f, f.area(), t, selected))
            .expect("render top chrome");
        let buf = terminal.backend().buffer();
        (0..w).map(|x| buf[(x, 0)].symbol()).collect()
    }

    #[test]
    fn top_chrome_shows_the_wordmark_and_every_tab() {
        let line = render(&Theme::hearth(), 120, 0);
        assert!(line.contains("WAYLAND"), "wordmark missing: {line:?}");
        // Every tab label is painted inline on the one row.
        for surface in SurfaceId::TABS {
            assert!(
                line.contains(surface.title()),
                "tab `{}` missing: {line:?}",
                surface.title()
            );
        }
    }

    #[test]
    fn top_chrome_carries_no_live_stats() {
        // Live stats moved to the bottom status bar — the top row must
        // not duplicate provider·model / ctx / cost / cpu / ram.
        let line = render(&Theme::hearth(), 120, 0);
        assert!(
            !line.contains("ctx"),
            "ctx leaked into the top row: {line:?}"
        );
        assert!(
            !line.contains("cpu"),
            "cpu leaked into the top row: {line:?}"
        );
        assert!(
            !line.contains('$'),
            "cost leaked into the top row: {line:?}"
        );
    }

    #[test]
    fn top_chrome_highlights_the_selected_tab() {
        // The active tab is painted in the brand accent; with the hearth
        // theme that is a distinct color from the dimmed inactive tabs.
        let t = Theme::hearth();
        let mut terminal = Terminal::new(TestBackend::new(120, 1)).expect("test terminal");
        terminal
            .draw(|f| top_chrome(f, f.area(), &t, 2))
            .expect("render top chrome");
        let buf = terminal.backend().buffer();
        // The third tab is "Plan" — its glyphs carry the orange accent.
        let has_accent_plan = (0..120).any(|x| {
            let cell = &buf[(x, 0)];
            cell.symbol() == "P" && cell.fg == t.orange
        });
        assert!(has_accent_plan, "selected tab not accent-highlighted");
    }

    #[test]
    fn top_chrome_renders_with_the_no_color_theme() {
        let line = render(&Theme::no_color(), 120, 0);
        assert!(
            line.contains("WAYLAND"),
            "uncolored chrome broken: {line:?}"
        );
    }

    #[test]
    fn top_chrome_does_not_panic_on_a_narrow_row() {
        let _ = render(&Theme::hearth(), 10, 0);
        let _ = render(&Theme::hearth(), 1, 0);
    }

    #[test]
    fn top_chrome_clamps_an_out_of_range_selection() {
        // An index past the tab count must clamp, not panic.
        let _ = render(&Theme::hearth(), 120, 99);
    }

    #[test]
    fn tab_label_truncation_appends_ellipsis_v0911() {
        // MED: at narrow widths the longest label ("Diagnostics") used
        // to be hard-clipped to "Diagno". After the fix it must end in
        // `…` so the user knows the label is abbreviated.
        let line = render(&Theme::hearth(), 70, 0);
        // At width 70 the per-label budget is too small to fit
        // "Diagnostics" (11 chars). The rendered row must contain `…`.
        assert!(
            line.contains('…'),
            "narrow row must show an ellipsis on truncated tab labels: {line:?}"
        );
        // And the hard "Diagno" mid-word clip must NOT appear standalone
        // — when "Diagnostics" is truncated we see "Diagn…" or shorter,
        // never "Diagno" with no ellipsis adjacent.
        if let Some(idx) = line.find("Diagno") {
            let next = line[idx + "Diagno".len()..].chars().next().unwrap_or(' ');
            assert!(
                next == 's' || next == '…',
                "Diagno appears without an ellipsis follow-up: {line:?}"
            );
        }
    }

    #[test]
    fn tab_label_truncation_helper_appends_ellipsis() {
        assert_eq!(truncate_with_ellipsis("Diagnostics", 6), "Diagn…");
        // No truncation when the label fits.
        assert_eq!(truncate_with_ellipsis("Plan", 6), "Plan");
        // Exactly fits — no ellipsis appended.
        assert_eq!(truncate_with_ellipsis("Config", 6), "Config");
        // Sub-2-column budget falls back to the raw label (the row
        // clip then takes over).
        assert_eq!(truncate_with_ellipsis("Diagnostics", 1), "Diagnostics");
    }

    #[test]
    fn sampler_caches_within_the_interval() {
        // Two back-to-back `sample()` calls fall inside one interval, so
        // the sampler must hand back the very same cached snapshot — the
        // proof it is not re-probing `sysinfo` every frame.
        let mut sampler = SystemSampler::new();
        let first = sampler.sample();
        let second = sampler.sample();
        assert_eq!(first.total_mem, second.total_mem);
        assert!(
            (first.cpu_pct - second.cpu_pct).abs() < f32::EPSILON,
            "cached sample changed within the interval"
        );
    }
}
