//! Command palette surface (surface 05) — the fuzzy command-palette
//! overlay.
//!
//! T1.4. A centered overlay drawn on top of the active surface. It hosts
//! a fuzzy filter over the [`CommandRegistry`](crate::tui::commands)
//! built-ins: the user types, results narrow with `nucleo`, and the
//! survivors render grouped by intent. Within each group, rows are
//! ordered by the persisted [`FrecencyStore`](crate::tui::frecency) —
//! recently/frequently used commands float to the top of their group.
//! The ordering is *felt*, never announced: per `ux-krug-sutherland.md`
//! finding #14, the word "frecency" appears nowhere in the UI.
//!
//! `⏎` on a selected row emits [`SurfaceAction::Command`] with the
//! command's name; `Esc` emits [`SurfaceAction::CloseOverlay`]. All other
//! state — the query, the selection index, the filtered rows — is local
//! to [`PaletteSurface`]; the surface reads nothing it does not own
//! except the immutable theme.

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use crate::tui::app::App;
use crate::tui::commands::{Command, CommandRegistry, IntentGroup};
use crate::tui::frecency::FrecencyStore;
use crate::tui::surfaces::{Surface, SurfaceAction, SurfaceId};
use crate::tui::theme::Theme;

/// One renderable line in the palette's result list — either a group
/// heading or a command row. The list is flattened to rows so a single
/// selection index and a single scroll calculation cover the whole view;
/// only `Command` rows are selectable.
enum Row {
    /// An intent-group section heading (e.g. `SESSION`). Not selectable.
    Heading(IntentGroup),
    /// A selectable command row, carrying a clone of the matched command.
    Command(Command),
}

/// The fuzzy command-palette overlay. Implements the frozen [`Surface`]
/// trait for [`SurfaceId::Palette`].
pub struct PaletteSurface {
    /// The command source. Owned by the surface so the palette is
    /// self-contained; the integrator may later seed it from the live
    /// registry, but Wave 1 builds it from the grounded built-ins.
    registry: CommandRegistry,
    /// Frecency ranking, loaded from disk on construction. A missing or
    /// corrupt store degrades to an empty one — never an error.
    frecency: FrecencyStore,
    /// The fuzzy matcher, reused across keystrokes (constructing one per
    /// frame is the documented `nucleo` anti-pattern).
    matcher: Matcher,
    /// The current search query (the text after the leading `/`).
    query: String,
    /// The flattened, filtered result list — headings interleaved with
    /// command rows. Rebuilt by `refilter` on every query change.
    rows: Vec<Row>,
    /// Index into `rows` of the highlighted command. Always points at a
    /// `Row::Command` when one exists; `0` when the list is empty.
    selected: usize,
}

impl PaletteSurface {
    /// Build the palette: load the built-in command registry and the
    /// frecency store, then compute the initial (unfiltered) row list.
    pub fn new() -> Self {
        let mut surface = Self {
            registry: CommandRegistry::with_builtins(),
            frecency: FrecencyStore::load().unwrap_or_default(),
            matcher: Matcher::new(Config::DEFAULT),
            query: String::new(),
            rows: Vec::new(),
            selected: 0,
        };
        surface.refilter();
        surface
    }

    /// Recompute `rows` from the current `query` and reset the selection
    /// to the first command row.
    ///
    /// Pipeline: fuzzy-filter the registry by `query`, keep the survivors,
    /// regroup them by [`IntentGroup`], order each group (by frecency for the
    /// unfiltered catalog, or by fuzzy score while a query is active), then
    /// flatten to a heading-interleaved row list. An empty query keeps
    /// every command (the `nucleo` pattern returns all items for an empty
    /// needle), so the palette opens showing the full grouped catalog.
    fn refilter(&mut self) {
        let survivors = self.fuzzy_filter();
        // The best-scoring survivor (prefix boost applied) is what a blind
        // Enter should run. `fuzzy_filter` returns best-first; with an active
        // query `group_and_flatten` now PRESERVES that score order within each
        // group (frecency only orders the unfiltered catalog), so the
        // prefix-boosted best match leads its group's rows. We still resolve
        // the selection by name here so it lands on the exact top survivor
        // even when an earlier intent group also has matches. Empty query
        // keeps the natural first-command selection.
        let top = (!self.query.is_empty())
            .then(|| survivors.first().map(|c| c.name.clone()))
            .flatten();
        self.rows = self.group_and_flatten(&survivors);
        self.selected = top
            .and_then(|name| {
                self.rows
                    .iter()
                    .position(|r| matches!(r, Row::Command(c) if c.name == name))
            })
            .or_else(|| self.first_command_index())
            .unwrap_or(0);
    }

    /// The commands that survive the fuzzy filter for the current query.
    ///
    /// Each command is matched on `"<name> <description>"` so a query can
    /// hit either the slash name or a word from the consequence text. The
    /// survivors are returned best-score-first; grouping (which discards
    /// this order) happens downstream — the score order only decides
    /// which commands *pass*. An empty query passes every command.
    ///
    /// v0.9.1.1 MED: prefix matches on the command NAME outrank
    /// description fuzzy matches. The bug-hunter observed `cost` routing
    /// to `/doctor` because nucleo's character-overlap heuristic scored
    /// `/doctor`'s description token (`"costs"` or similar) above the
    /// exact prefix match on `/cost`. We now boost any command whose
    /// name starts with the query (case-insensitively, with or without
    /// the leading slash) by a large additive bonus so exact-prefix
    /// always wins.
    fn fuzzy_filter(&mut self) -> Vec<Command> {
        let commands = self.registry.all();
        if self.query.is_empty() {
            return commands.to_vec();
        }
        let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);
        // Normalised query for the prefix check: strip a leading `/`,
        // lowercase. Both `/cost` and `cost` should hit `/cost`.
        let query_norm = self.query.trim_start_matches('/').to_ascii_lowercase();
        // Score each command against its `name + description` haystack.
        // `Utf32Str::new` picks the ASCII or Unicode representation; the
        // scratch buffer is reused across every candidate.
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, Command)> = commands
            .iter()
            .filter_map(|c| {
                let haystack = format!("{} {}", c.name, c.description);
                let needle = Utf32Str::new(&haystack, &mut buf);
                pattern.score(needle, &mut self.matcher).map(|score| {
                    // Prefix boost: when the command NAME (after the
                    // slash, lowercased) starts with the query, add
                    // a large bonus so exact-prefix outranks
                    // description-substring matches. Equality wins
                    // over prefix wins over substring.
                    let name_norm = c.name.trim_start_matches('/').to_ascii_lowercase();
                    let boost = if name_norm == query_norm {
                        1_000_000
                    } else if name_norm.starts_with(&query_norm) {
                        500_000
                    } else {
                        0
                    };
                    (score.saturating_add(boost), c.clone())
                })
            })
            .collect();
        // Best score first.
        scored.sort_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored.into_iter().map(|(_, c)| c).collect()
    }

    /// Regroup `commands` by intent and flatten to a row list: one
    /// [`Row::Heading`] per non-empty group (in [`IntentGroup::ORDER`]),
    /// followed by that group's commands.
    ///
    /// Within a group, ordering depends on whether a query is active:
    /// - **Empty query** (the full catalog): order by frecency, so the
    ///   user's daily-driver commands float to the top of their section.
    /// - **Active query**: preserve the order of `commands`, which
    ///   `fuzzy_filter` already sorted best-score-first *with the
    ///   name-prefix boost applied*. Re-ranking by frecency here would
    ///   discard that boost — that is the v0.9.1.1 bug where `cost`
    ///   surfaced `/repomap` above `/cost` because both share the
    ///   ContextMemory group and frecency fell back to registry order.
    ///   An exact name-prefix match must outrank a description-only
    ///   fuzzy match, so the prefix-boosted survivor order wins.
    fn group_and_flatten(&self, commands: &[Command]) -> Vec<Row> {
        let querying = !self.query.is_empty();
        let mut rows = Vec::new();
        for group in IntentGroup::ORDER {
            let mut members: Vec<&Command> = commands.iter().filter(|c| c.group == group).collect();
            if members.is_empty() {
                continue;
            }
            if !querying {
                // Order within the group by frecency. `rank` works on owned
                // strings, so rank the names then reorder `members` to match.
                let names: Vec<String> = members.iter().map(|c| c.name.clone()).collect();
                let ranked = self.frecency.rank(&names);
                members.sort_by_key(|c| {
                    ranked
                        .iter()
                        .position(|n| n == &c.name)
                        .unwrap_or(usize::MAX)
                });
            }
            // When querying, `members` keeps its `commands` (score) order
            // — the prefix-boosted best match leads its group.
            rows.push(Row::Heading(group));
            for c in members {
                rows.push(Row::Command(c.clone()));
            }
        }
        rows
    }

    /// Index of the first selectable command row, if any.
    fn first_command_index(&self) -> Option<usize> {
        self.rows.iter().position(|r| matches!(r, Row::Command(_)))
    }

    /// Move the selection to the next selectable command row, skipping
    /// headings. A no-op when there is nothing selectable below.
    fn select_next(&mut self) {
        let mut i = self.selected + 1;
        while i < self.rows.len() {
            if matches!(self.rows[i], Row::Command(_)) {
                self.selected = i;
                return;
            }
            i += 1;
        }
    }

    /// Move the selection to the previous selectable command row,
    /// skipping headings. A no-op when there is nothing selectable above.
    fn select_prev(&mut self) {
        let mut i = self.selected;
        while i > 0 {
            i -= 1;
            if matches!(self.rows[i], Row::Command(_)) {
                self.selected = i;
                return;
            }
        }
    }

    /// The currently highlighted command, if the selection points at one.
    fn selected_command(&self) -> Option<&Command> {
        match self.rows.get(self.selected) {
            Some(Row::Command(c)) => Some(c),
            _ => None,
        }
    }

    /// Run the highlighted command: record it for frecency, persist the
    /// store, and emit the [`SurfaceAction::Command`] line. The router
    /// also closes the overlay when a `Command` action is dispatched.
    fn run_selected(&mut self) -> SurfaceAction {
        match self.selected_command() {
            Some(command) => {
                let name = command.name.clone();
                self.frecency.record(&name);
                // Persisting is best-effort: a frecency cache that fails
                // to write must never block running the command.
                let _ = self.frecency.save();
                SurfaceAction::Command(name)
            }
            None => SurfaceAction::None,
        }
    }
}

impl Default for PaletteSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl Surface for PaletteSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::Palette
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _app: &App, theme: &Theme) {
        let popup = centered_rect(area);
        // Clear the cells behind the overlay so the surface underneath
        // does not bleed through the palette body.
        frame.render_widget(Clear, popup);

        let outer = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border))
            .style(Style::default().bg(theme.surface_elevated))
            .title(Span::styled(
                " /  command ",
                Style::default().fg(theme.text_muted),
            ));
        let inner = outer.inner(popup);
        frame.render_widget(outer, popup);

        // Three stacked regions: the search line, the result list, the
        // key-hint footer.
        let [search_area, list_area, foot_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .areas(inner);

        self.render_search(frame, search_area, theme);
        self.render_list(frame, list_area, theme);
        self.render_footer(frame, foot_area, theme);
    }

    fn handle_key(&mut self, key: KeyEvent, _app: &mut App) -> SurfaceAction {
        match key.code {
            KeyCode::Esc => SurfaceAction::CloseOverlay,
            KeyCode::Enter => self.run_selected(),
            KeyCode::Up => {
                self.select_prev();
                SurfaceAction::None
            }
            KeyCode::Down => {
                self.select_next();
                SurfaceAction::None
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.refilter();
                SurfaceAction::None
            }
            KeyCode::Char(c) => {
                // v0.9.1.2 polish 1C: a typed char that cannot be part of
                // any slash-command name (whitespace, a second `/`, `:`,
                // punctuation) is a strong signal the user did NOT mean
                // to open the palette — they typed `/` at the start of a
                // path or prose line (e.g. `Output to /tmp/v0912-test-4/
                // express/:`). Gracefully dismiss the overlay and forward
                // the accumulated typing back to the composer so the user
                // keeps moving. The restored prefix is `/<query><c>`
                // because the opening `/` was consumed by the workspace
                // when the overlay opened. A `/` as the FIRST char of the
                // query is tolerated — some users retype the slash, and
                // the prefix-boost in `fuzzy_filter` already strips it.
                let allow_leading_slash = c == '/' && self.query.is_empty();
                if !is_command_name_char(c) && !allow_leading_slash {
                    let restored = format!("/{}{}", self.query, c);
                    return SurfaceAction::CloseOverlayAndPasteToActive(restored);
                }
                self.query.push(c);
                self.refilter();
                SurfaceAction::None
            }
            _ => SurfaceAction::None,
        }
    }
}

/// Whether `c` can appear inside a slash-command name. Built-in commands
/// use `[a-z0-9-]`; underscore is allowed defensively so future commands
/// may extend the alphabet without re-tuning the dismiss predicate. Every
/// other byte (whitespace, `/`, `:`, punctuation, accented letters) is a
/// signal the user is no longer typing a command name and the palette
/// should dismiss itself. v0.9.1.2 polish 1C.
fn is_command_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-' || c == '_'
}

impl PaletteSurface {
    /// Draw the search line: an orange `/` prompt followed by the live
    /// query text.
    fn render_search(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let line = Line::from(vec![
            Span::styled(
                "/ ",
                Style::default()
                    .fg(theme.orange)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(self.query.clone(), Style::default().fg(theme.text)),
        ]);
        frame.render_widget(Paragraph::new(line), area);
    }

    /// Draw the grouped result list. Headings render as dim section
    /// labels; command rows show name + consequence description, and the
    /// selected row is accent-highlighted. Destructive commands carry a
    /// visible `↩` tag so a file-discarding command never looks like a
    /// read-only one (finding #15).
    fn render_list(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        if self.rows.is_empty() {
            let empty = Paragraph::new(Line::from(Span::styled(
                "no commands match",
                Style::default().fg(theme.text_muted),
            )));
            frame.render_widget(empty, area);
            return;
        }

        // Scroll so the selected row stays visible in a small viewport.
        let height = area.height as usize;
        let start = self.selected.saturating_sub(height.saturating_sub(1));
        let lines: Vec<Line> = self
            .rows
            .iter()
            .enumerate()
            .skip(start)
            .take(height)
            .map(|(i, row)| self.render_row(row, i == self.selected, theme))
            .collect();
        frame.render_widget(Paragraph::new(lines), area);
    }

    /// Render one result row to a styled [`Line`].
    fn render_row(&self, row: &Row, selected: bool, theme: &Theme) -> Line<'static> {
        match row {
            Row::Heading(group) => Line::from(Span::styled(
                group.title().to_string(),
                Style::default()
                    .fg(theme.text_muted)
                    .add_modifier(Modifier::BOLD),
            )),
            Row::Command(c) => {
                let (name_color, desc_color, prefix) = if selected {
                    (theme.orange, theme.text_dim, "› ")
                } else {
                    (theme.text, theme.text_muted, "  ")
                };
                let mut spans = vec![
                    Span::styled(prefix, Style::default().fg(theme.orange)),
                    Span::styled(
                        format!("{:<11}", c.name),
                        Style::default().fg(name_color).add_modifier(Modifier::BOLD),
                    ),
                ];
                // Destructive tag — an accent `↩` glyph (finding #15).
                if c.destructive {
                    spans.push(Span::styled(
                        "↩ ",
                        Style::default()
                            .fg(theme.warning)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                spans.push(Span::styled(
                    c.description.clone(),
                    Style::default().fg(desc_color),
                ));
                Line::from(spans)
            }
        }
    }

    /// Draw the key-hint footer. Per `ux-krug-sutherland.md` finding #14
    /// the footer is exactly `↑↓ move · ⏎ run · esc close` — no
    /// "frecency", no item count.
    fn render_footer(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let hint = |k: &'static str, label: &'static str| {
            vec![
                Span::styled(k, Style::default().fg(theme.orange)),
                Span::styled(format!(" {label}"), Style::default().fg(theme.text_muted)),
            ]
        };
        let mut spans = Vec::new();
        spans.extend(hint("↑↓", "move"));
        spans.push(Span::styled("   ", Style::default()));
        spans.extend(hint("⏎", "run"));
        spans.push(Span::styled("   ", Style::default()));
        spans.extend(hint("esc", "close"));
        frame.render_widget(Paragraph::new(Line::from(spans)), area);
    }
}

/// A centered rectangle for the palette overlay — bounded width and
/// height so the popup never spans the whole terminal but still shrinks
/// gracefully on a small screen.
///
/// The result is always clamped to fit inside `area`: on a terminal too
/// small for the popup's preferred minimum (a 1- or 2-row frame) the
/// popup collapses to the available space rather than overflowing the
/// buffer.
fn centered_rect(area: Rect) -> Rect {
    let width = area.width.saturating_sub(8).clamp(1, 72).min(area.width);
    let height = area.height.saturating_sub(4).clamp(3, 20).min(area.height);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    Rect {
        x,
        y,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Type a string into the palette one `Char` key at a time.
    fn type_query(p: &mut PaletteSurface, app: &mut App, text: &str) {
        for ch in text.chars() {
            p.handle_key(key(KeyCode::Char(ch)), app);
        }
    }

    /// The command names currently in the row list, in display order.
    fn visible_commands(p: &PaletteSurface) -> Vec<String> {
        p.rows
            .iter()
            .filter_map(|r| match r {
                Row::Command(c) => Some(c.name.clone()),
                Row::Heading(_) => None,
            })
            .collect()
    }

    // ── construction ───────────────────────────────────────────────────

    #[test]
    fn new_palette_shows_every_builtin_command() {
        let palette = PaletteSurface::new();
        assert_eq!(
            visible_commands(&palette).len(),
            CommandRegistry::with_builtins().len()
        );
        assert_eq!(palette.id(), SurfaceId::Palette);
    }

    #[test]
    fn new_palette_selects_the_first_command_not_a_heading() {
        let palette = PaletteSurface::new();
        // Row 0 is a heading; the selection must land on the first
        // selectable command row beneath it.
        assert!(matches!(palette.rows[0], Row::Heading(_)));
        assert!(palette.selected_command().is_some());
    }

    // ── fuzzy filter ───────────────────────────────────────────────────

    #[test]
    fn typing_narrows_the_list_to_fuzzy_matches() {
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        type_query(&mut palette, &mut app, "repo");
        let visible = visible_commands(&palette);
        assert!(visible.contains(&"/repomap".to_string()));
        assert!(!visible.contains(&"/quit".to_string()));
    }

    #[test]
    fn typing_a_command_word_selects_that_command() {
        // Discoverability: typing a command's own word must put THAT command
        // under the selection cursor, so a blind Enter runs what the user
        // typed — not a fuzzy neighbour. (The 2026-06-01 stub-wiring E2E hit
        // `mcp` selecting `/compact`.)
        for (word, expected) in [
            ("skills", "/skills"),
            ("mcp", "/mcp"),
            ("hooks", "/hooks"),
            ("resume", "/resume"),
            ("profile", "/profile"),
            ("provider", "/provider"),
            ("replay", "/replay"),
            ("rewind", "/rewind"),
            ("repomap", "/repomap"),
        ] {
            let mut app = App::new();
            let mut palette = PaletteSurface::new();
            type_query(&mut palette, &mut app, word);
            assert_eq!(
                palette.selected_command().map(|c| c.name.as_str()),
                Some(expected),
                "typing `{word}` must select `{expected}`, not a fuzzy neighbour"
            );
        }
    }

    #[test]
    fn fuzzy_filter_matches_on_the_description_not_only_the_name() {
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        // "snapshot" appears only in /rewind's consequence description.
        type_query(&mut palette, &mut app, "snapshot");
        assert!(visible_commands(&palette).contains(&"/rewind".to_string()));
    }

    #[test]
    fn backspace_widens_the_filter_again() {
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        type_query(&mut palette, &mut app, "repo");
        let narrowed = visible_commands(&palette).len();
        for _ in 0.."repo".len() {
            palette.handle_key(key(KeyCode::Backspace), &mut app);
        }
        assert!(visible_commands(&palette).len() > narrowed);
        assert_eq!(
            visible_commands(&palette).len(),
            CommandRegistry::with_builtins().len()
        );
    }

    #[test]
    fn no_match_yields_an_empty_row_list() {
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        type_query(&mut palette, &mut app, "zzzznotacommand");
        assert!(palette.rows.is_empty());
        // Running with nothing selected is inert, not a panic.
        assert!(matches!(
            palette.handle_key(key(KeyCode::Enter), &mut app),
            SurfaceAction::None
        ));
    }

    // ── grouping ───────────────────────────────────────────────────────

    #[test]
    fn rows_are_grouped_by_intent_in_canonical_order() {
        let palette = PaletteSurface::new();
        let headings: Vec<IntentGroup> = palette
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Heading(g) => Some(*g),
                Row::Command(_) => None,
            })
            .collect();
        assert_eq!(headings, IntentGroup::ORDER.to_vec());
    }

    #[test]
    fn every_command_row_follows_a_heading_of_its_own_group() {
        let palette = PaletteSurface::new();
        let mut current: Option<IntentGroup> = None;
        for row in &palette.rows {
            match row {
                Row::Heading(g) => current = Some(*g),
                Row::Command(c) => {
                    assert_eq!(Some(c.group), current, "{} under wrong group", c.name);
                }
            }
        }
    }

    // ── selection + navigation ─────────────────────────────────────────

    #[test]
    fn down_and_up_move_the_selection_skipping_headings() {
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        let first = palette.selected;
        palette.handle_key(key(KeyCode::Down), &mut app);
        assert_ne!(palette.selected, first);
        // The selection always lands on a command, never a heading.
        assert!(palette.selected_command().is_some());
        palette.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(palette.selected, first);
    }

    #[test]
    fn navigation_clamps_at_the_list_ends() {
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        // Up from the first command is a no-op.
        let first = palette.selected;
        palette.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(palette.selected, first);
        // Down past the last command clamps on the last command.
        for _ in 0..200 {
            palette.handle_key(key(KeyCode::Down), &mut app);
        }
        assert!(palette.selected_command().is_some());
        let last = palette.selected;
        palette.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(palette.selected, last);
    }

    // ── emitted SurfaceActions ─────────────────────────────────────────

    #[test]
    fn enter_emits_a_command_action_for_the_selection() {
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        type_query(&mut palette, &mut app, "repomap");
        let action = palette.handle_key(key(KeyCode::Enter), &mut app);
        // `SurfaceAction` is a frozen contract without `Debug`, so the
        // payload is extracted by pattern and asserted directly.
        match action {
            SurfaceAction::Command(line) => assert_eq!(line, "/repomap"),
            _ => panic!("expected SurfaceAction::Command from Enter"),
        }
    }

    #[test]
    fn esc_emits_close_overlay() {
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        assert!(matches!(
            palette.handle_key(key(KeyCode::Esc), &mut app),
            SurfaceAction::CloseOverlay
        ));
    }

    // ── render snapshot ────────────────────────────────────────────────

    #[test]
    fn render_draws_the_palette_chrome_and_a_command() {
        let app = App::new();
        let theme = Theme::no_color();
        let mut palette = PaletteSurface::new();
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).expect("test terminal");
        terminal
            .draw(|f| palette.render(f, f.area(), &app, &theme))
            .expect("render palette");
        let buf = terminal.backend().buffer();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        // The palette title, a group heading, a known command, and the
        // finding-#14 footer are all on screen.
        assert!(text.contains("/  command"), "title missing");
        assert!(text.contains("SESSION"), "group heading missing");
        assert!(text.contains("/rewind"), "command row missing");
        assert!(text.contains("move"), "footer hint missing");
        // Finding #14 — the word "frecency" never reaches the UI.
        assert!(
            !text.to_lowercase().contains("frecency"),
            "the UI leaked the word 'frecency'"
        );
    }

    #[test]
    fn renders_on_a_tiny_terminal_without_panicking() {
        // The centered-overlay math and the three-region split must clamp,
        // never overflow, when the terminal is smaller than the popup.
        let app = App::new();
        let theme = Theme::no_color();
        let mut palette = PaletteSurface::new();
        for (w, h) in [(1, 1), (4, 3), (10, 4)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
            terminal
                .draw(|f| palette.render(f, f.area(), &app, &theme))
                .expect("render palette on a tiny terminal");
        }
    }

    #[test]
    fn render_tags_a_destructive_command() {
        let app = App::new();
        let theme = Theme::no_color();
        let mut palette = PaletteSurface::new();
        // Filter to just /rewind so its row is guaranteed on screen.
        let mut app_mut = App::new();
        type_query(&mut palette, &mut app_mut, "rewind");
        let mut terminal = Terminal::new(TestBackend::new(80, 12)).expect("test terminal");
        terminal
            .draw(|f| palette.render(f, f.area(), &app, &theme))
            .expect("render palette");
        let text: String = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        // The destructive glyph marks /rewind apart from read-only rows.
        assert!(text.contains("↩"), "destructive tag missing for /rewind");
    }

    // ── v0.9.1.1 MED: slash picker prefers exact prefix match ──────

    #[test]
    fn slash_picker_prefers_exact_prefix_match_v0911() {
        // The hunt observed `cost` routing to `/doctor` because the
        // fuzzy scorer favoured description-token overlap. After the
        // fix, an exact name prefix outranks any description match —
        // so `cost` must land on `/cost` first, not `/doctor`.
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        type_query(&mut palette, &mut app, "cost");
        let visible = visible_commands(&palette);
        assert!(
            visible.contains(&"/cost".to_string()),
            "fuzzy did not surface /cost: {:?}",
            visible
        );

        // The first selectable command in the result rows must be /cost
        // (proving the prefix boost rerouted it above /doctor or any
        // other description-overlap candidate). The selection always
        // lands on `first_command_index`, so we read THAT row.
        let first_command_name = palette
            .rows
            .iter()
            .find_map(|r| match r {
                Row::Command(c) => Some(c.name.as_str()),
                _ => None,
            })
            .expect("no command rows after typing 'cost'");
        assert_eq!(
            first_command_name, "/cost",
            "first match for `cost` should be `/cost`, got `{}`",
            first_command_name
        );
    }

    // ── v0.9.1.2 polish 1C: palette dismisses on non-command-name chars ──

    #[test]
    fn slash_picker_dismisses_on_path_separator_v0913() {
        // Reproduction of the v0.9.1.2 test-4 audit finding: the user
        // typed `/tmp/...` at the start of a prose line, the workspace
        // consumed the leading `/` and opened the palette, then every
        // subsequent char (`t`, `m`, `p`) became the palette query.
        // When the user hit the SECOND `/` (path separator) the palette
        // must dismiss itself and return the typed text to the composer
        // so the user can keep typing without losing input.
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        type_query(&mut palette, &mut app, "tmp");
        // Up to here we are still in the palette — typed chars are
        // valid name chars and the query is `tmp`.
        assert_eq!(palette.query, "tmp");
        // The fourth keystroke is `/` — that is the path-separator
        // signal. The palette must close AND paste `/tmp/` to the
        // active surface so the composer can absorb the whole typed
        // prefix (including the `/` the workspace originally consumed).
        let action = palette.handle_key(key(KeyCode::Char('/')), &mut app);
        match action {
            SurfaceAction::CloseOverlayAndPasteToActive(text) => {
                assert_eq!(
                    text, "/tmp/",
                    "must restore the consumed `/` plus the typed query"
                );
            }
            other => panic!("expected CloseOverlayAndPasteToActive, got {:?}", other),
        }
    }

    #[test]
    fn slash_picker_dismisses_on_colon_after_path_v0913() {
        // The audit specifically flagged `/tmp/foo:` — the trailing
        // colon is what got the test agent stuck. With the polish-1C
        // fix the path-separator `/` already dismisses the palette
        // before the colon ever arrives. This test covers the
        // standalone case: colon after a name-only query also dismisses
        // the palette (a colon cannot appear in any slash command name).
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        type_query(&mut palette, &mut app, "doctor");
        assert_eq!(palette.query, "doctor");
        let action = palette.handle_key(key(KeyCode::Char(':')), &mut app);
        match action {
            SurfaceAction::CloseOverlayAndPasteToActive(text) => {
                assert_eq!(text, "/doctor:");
            }
            other => panic!("expected CloseOverlayAndPasteToActive, got {:?}", other),
        }
    }

    #[test]
    fn slash_picker_dismisses_on_whitespace_v0913() {
        // Whitespace cannot appear in any slash command name.  Typing a
        // space mid-query is a strong signal the user is writing prose;
        // dismiss the palette and forward the buffer to the composer so
        // the space lands where the user expected it.
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        type_query(&mut palette, &mut app, "tmp");
        let action = palette.handle_key(key(KeyCode::Char(' ')), &mut app);
        match action {
            SurfaceAction::CloseOverlayAndPasteToActive(text) => {
                assert_eq!(text, "/tmp ");
            }
            other => panic!("expected CloseOverlayAndPasteToActive, got {:?}", other),
        }
    }

    #[test]
    fn slash_picker_tolerates_leading_slash_in_query_v0913() {
        // The dismiss predicate must not regress the leading-`/` query
        // path: a user who artificially types `/` as the first query
        // char (or whose terminal layer replays it) still gets the
        // command list, not an instant dismissal.  Once the query has
        // any chars, a further `/` IS path-separator semantics and DOES
        // dismiss (covered by `slash_picker_dismisses_on_path_separator`).
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        let action = palette.handle_key(key(KeyCode::Char('/')), &mut app);
        assert!(
            matches!(action, SurfaceAction::None),
            "leading `/` must be tolerated as a query char, got {:?}",
            action
        );
        assert_eq!(palette.query, "/");
    }

    #[test]
    fn slash_picker_keeps_typing_through_valid_name_chars_v0913() {
        // The dismiss predicate must not fire for ANY char that can
        // appear in a slash-command name. Sanity-check the full lower
        // ASCII alpha + digit + `-` + `_` alphabet.
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        for c in "abcdefghijklmnopqrstuvwxyz0123456789-_".chars() {
            let action = palette.handle_key(key(KeyCode::Char(c)), &mut app);
            assert!(
                matches!(action, SurfaceAction::None),
                "name-char `{}` should not dismiss the palette, got {:?}",
                c,
                action
            );
        }
        assert_eq!(palette.query, "abcdefghijklmnopqrstuvwxyz0123456789-_");
    }

    #[test]
    fn slash_picker_prefix_match_works_with_leading_slash_v0911() {
        // Typing `/cost` (with the leading slash) must also score
        // `/cost` first — the prefix boost normalises the slash.
        let mut app = App::new();
        let mut palette = PaletteSurface::new();
        type_query(&mut palette, &mut app, "/cost");
        let first_command_name = palette
            .rows
            .iter()
            .find_map(|r| match r {
                Row::Command(c) => Some(c.name.as_str()),
                _ => None,
            })
            .expect("no command rows after typing '/cost'");
        assert_eq!(
            first_command_name, "/cost",
            "first match for `/cost` should be `/cost`, got `{}`",
            first_command_name
        );
    }
}
