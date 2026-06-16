//! Arrow-key pickers for `/model` and `/provider` (overlay surfaces).
//!
//! Both replace the old type-the-id text listing with an interactive,
//! instantly-opening overlay: `↑↓` move the selection (skipping group
//! headings), `⏎` selects, `esc` closes. They follow the manual `selected:
//! usize` + flattened `Vec<Row>` convention established by
//! [`PaletteSurface`](super::palette) — there is no `ListState` pattern in
//! this codebase.
//!
//! ## Static catalog, no async
//!
//! The model picker is built from the **static** alias catalog
//! ([`known_providers`] × [`models_for_provider`]) so it opens instantly —
//! it deliberately does NOT use the async `engine.list_models()` path (which
//! arrives later as an `Info` turn). The bare `/model <id>` shortcut keeps
//! using the live fetch.
//!
//! ## Selection routes through the existing command dispatch
//!
//! A `Surface` cannot reach `Router::apply_provider_swap` directly — it only
//! returns a [`SurfaceAction`]. So both pickers emit
//! [`SurfaceAction::Command`] lines that the router's `dispatch_command`
//! already handles:
//! - provider picker → `/provider <name>` (live swap, carries the OAuth
//!   precheck inside `apply_provider_swap`).
//! - model picker, same provider → `/model <role>` (live model set).
//! - model picker, different provider → `/model <provider> <role>` — the
//!   two-arg form the `/model` arm routes through `apply_provider_swap`
//!   FIRST (OAuth precheck; if not signed in it surfaces the login hint and
//!   leaves the engine untouched) and only then sets the model.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use wcore_types::model_aliases::{known_providers, models_for_provider};

use crate::tui::app::App;
use crate::tui::surfaces::{Surface, SurfaceAction, SurfaceId};
use crate::tui::theme::Theme;

/// A centered overlay rectangle — mirrors `palette::centered_rect` so the two
/// overlays share the same footprint and small-terminal clamping.
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

// ════════════════════════════════════════════════════════════════════════
// /model picker
// ════════════════════════════════════════════════════════════════════════

/// One renderable line in the model picker — a provider heading or a
/// selectable model row. Only `Model` rows are selectable.
enum ModelRow {
    /// A provider section heading (e.g. `anthropic`). Not selectable.
    Heading(&'static str),
    /// A selectable model row: `(provider, role, resolved_id)`.
    Model {
        provider: &'static str,
        /// The human role handle (the part after `provider:` in the short
        /// form, e.g. `opus`).
        role: &'static str,
        /// The resolved model id the request carries.
        resolved: &'static str,
    },
}

/// Arrow-key `/model` picker overlay. Lists every known provider's static
/// model catalog grouped by provider; the active model is marked `●`.
pub struct ModelPickerSurface {
    rows: Vec<ModelRow>,
    /// Index into `rows` of the highlighted model. Always points at a
    /// `Model` row when one exists; `0` when empty.
    selected: usize,
}

impl ModelPickerSurface {
    /// Build the picker from the static catalog. The selection lands on the
    /// active model when it is present, else the first model row.
    pub fn new(active_provider: &str, active_model: &str) -> Self {
        let rows = Self::build_rows();
        let mut surface = Self { rows, selected: 0 };
        surface.selected = surface
            .index_of_active(active_provider, active_model)
            .or_else(|| surface.first_model_index())
            .unwrap_or(0);
        surface
    }

    /// Flatten `known_providers() × models_for_provider()` into a
    /// heading-interleaved row list, in the catalog's display order.
    fn build_rows() -> Vec<ModelRow> {
        let mut rows = Vec::new();
        for provider in known_providers() {
            let models = models_for_provider(provider);
            if models.is_empty() {
                continue;
            }
            rows.push(ModelRow::Heading(provider));
            for (short, resolved) in models {
                let role = short.split_once(':').map(|x| x.1).unwrap_or(short);
                rows.push(ModelRow::Model {
                    provider,
                    role,
                    resolved,
                });
            }
        }
        rows
    }

    /// Index of the row matching the active provider+model, if present. Matches
    /// on the resolved id OR the role so a config carrying either form lands.
    fn index_of_active(&self, active_provider: &str, active_model: &str) -> Option<usize> {
        self.rows.iter().position(|r| {
            matches!(
                r,
                ModelRow::Model { provider, role, resolved }
                    if *provider == active_provider
                        && (*resolved == active_model || *role == active_model)
            )
        })
    }

    /// Index of the first selectable model row, if any.
    fn first_model_index(&self) -> Option<usize> {
        self.rows
            .iter()
            .position(|r| matches!(r, ModelRow::Model { .. }))
    }

    /// Move the selection to the next model row, skipping headings.
    fn select_next(&mut self) {
        let mut i = self.selected + 1;
        while i < self.rows.len() {
            if matches!(self.rows[i], ModelRow::Model { .. }) {
                self.selected = i;
                return;
            }
            i += 1;
        }
    }

    /// Move the selection to the previous model row, skipping headings.
    fn select_prev(&mut self) {
        let mut i = self.selected;
        while i > 0 {
            i -= 1;
            if matches!(self.rows[i], ModelRow::Model { .. }) {
                self.selected = i;
                return;
            }
        }
    }

    /// The highlighted model row, if the selection points at one.
    fn selected_model(&self) -> Option<(&'static str, &'static str)> {
        match self.rows.get(self.selected) {
            Some(ModelRow::Model { provider, role, .. }) => Some((*provider, *role)),
            _ => None,
        }
    }

    /// Build the `SurfaceAction` for the current selection.
    ///
    /// Same provider → `/model <role>` (the existing live model set). A
    /// different provider → `/model <provider> <role>`, the two-arg form the
    /// `/model` dispatch arm routes through `apply_provider_swap` first (OAuth
    /// precheck) and then the model set. Nothing selectable → `None`.
    fn select_action(&self, active_provider: &str) -> SurfaceAction {
        match self.selected_model() {
            Some((provider, role)) if provider == active_provider => {
                SurfaceAction::Command(format!("/model {role}"))
            }
            Some((provider, role)) => SurfaceAction::Command(format!("/model {provider} {role}")),
            None => SurfaceAction::None,
        }
    }
}

impl Surface for ModelPickerSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::ModelPicker
    }

    /// Seed the selection from the live config (make_surface has no `App`, so
    /// the initial selection is resolved here when the overlay opens).
    fn on_enter(&mut self, app: &mut App) {
        self.selected = self
            .index_of_active(&app.config.provider, &app.config.model)
            .or_else(|| self.first_model_index())
            .unwrap_or(0);
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        let popup = centered_rect(area);
        frame.render_widget(Clear, popup);
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border))
            .style(Style::default().bg(theme.surface_elevated))
            .title(Span::styled(
                " model ",
                Style::default().fg(theme.text_muted),
            ));
        let inner = outer.inner(popup);
        frame.render_widget(outer, popup);

        let [list_area, foot_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);

        render_rows(
            frame,
            list_area,
            theme,
            self.rows.iter().enumerate().map(|(i, row)| {
                let selected = i == self.selected;
                match row {
                    ModelRow::Heading(p) => RowView::Heading((*p).to_string()),
                    ModelRow::Model {
                        provider,
                        role,
                        resolved,
                    } => {
                        let active = *provider == app.config.provider.as_str()
                            && (*resolved == app.config.model.as_str()
                                || *role == app.config.model.as_str());
                        RowView::Item {
                            selected,
                            active,
                            label: (*role).to_string(),
                            detail: (*resolved).to_string(),
                        }
                    }
                }
            }),
            self.selected,
        );
        render_footer(frame, foot_area, theme, "↑↓ move · ⏎ select · esc close");
    }

    fn handle_key(&mut self, key: KeyEvent, app: &mut App) -> SurfaceAction {
        match key.code {
            KeyCode::Esc => SurfaceAction::CloseOverlay,
            KeyCode::Enter => self.select_action(&app.config.provider),
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_prev();
                SurfaceAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
                SurfaceAction::None
            }
            _ => SurfaceAction::None,
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
// /provider picker
// ════════════════════════════════════════════════════════════════════════

/// Arrow-key `/provider` picker overlay. Lists the known providers; the
/// active one is marked `●`. On Enter emits `/provider <name>`, which the
/// router live-swaps through `apply_provider_swap` (keeping the OAuth
/// precheck + live rebind).
pub struct ProviderPickerSurface {
    providers: &'static [&'static str],
    selected: usize,
}

impl ProviderPickerSurface {
    pub fn new(active_provider: &str) -> Self {
        let providers = known_providers();
        let selected = providers
            .iter()
            .position(|p| *p == active_provider)
            .unwrap_or(0);
        Self {
            providers,
            selected,
        }
    }

    fn select_next(&mut self) {
        if self.selected + 1 < self.providers.len() {
            self.selected += 1;
        }
    }

    fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }
}

impl Surface for ProviderPickerSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::ProviderPicker
    }

    /// Seed the selection to the active provider when the overlay opens.
    fn on_enter(&mut self, app: &mut App) {
        self.selected = self
            .providers
            .iter()
            .position(|p| *p == app.config.provider)
            .unwrap_or(0);
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, app: &App, theme: &Theme) {
        let popup = centered_rect(area);
        frame.render_widget(Clear, popup);
        let outer = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border))
            .style(Style::default().bg(theme.surface_elevated))
            .title(Span::styled(
                " provider ",
                Style::default().fg(theme.text_muted),
            ));
        let inner = outer.inner(popup);
        frame.render_widget(outer, popup);

        let [list_area, foot_area] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);

        render_rows(
            frame,
            list_area,
            theme,
            self.providers.iter().enumerate().map(|(i, name)| {
                let selected = i == self.selected;
                let active = *name == app.config.provider.as_str();
                // Show the sign-in status for OAuth providers (cheap, sync).
                // `name` is `&&str`; deref-coerces to the `&str` the helper takes.
                let detail = match super::oauth_provider_signed_in(name) {
                    Some(true) => "signed in".to_string(),
                    Some(false) => "not signed in".to_string(),
                    None => String::new(),
                };
                RowView::Item {
                    selected,
                    active,
                    label: (*name).to_string(),
                    detail,
                }
            }),
            self.selected,
        );
        render_footer(frame, foot_area, theme, "↑↓ move · ⏎ select · esc close");
    }

    fn handle_key(&mut self, key: KeyEvent, _app: &mut App) -> SurfaceAction {
        match key.code {
            KeyCode::Esc => SurfaceAction::CloseOverlay,
            KeyCode::Enter => match self.providers.get(self.selected) {
                Some(name) => SurfaceAction::Command(format!("/provider {name}")),
                None => SurfaceAction::None,
            },
            KeyCode::Up | KeyCode::Char('k') => {
                self.select_prev();
                SurfaceAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.select_next();
                SurfaceAction::None
            }
            _ => SurfaceAction::None,
        }
    }
}

// ════════════════════════════════════════════════════════════════════════
// shared rendering
// ════════════════════════════════════════════════════════════════════════

/// A view-model for one rendered row, shared by both pickers.
enum RowView {
    Heading(String),
    Item {
        selected: bool,
        active: bool,
        label: String,
        detail: String,
    },
}

/// Draw a heading-interleaved row list with a scroll window keeping the
/// selected row visible. Mirrors `palette::render_list`.
fn render_rows(
    frame: &mut Frame,
    area: Rect,
    theme: &Theme,
    rows: impl Iterator<Item = RowView>,
    selected: usize,
) {
    let height = area.height as usize;
    let start = selected.saturating_sub(height.saturating_sub(1));
    let lines: Vec<Line> = rows
        .skip(start)
        .take(height.max(1))
        .map(|row| render_row(&row, theme))
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

fn render_row(row: &RowView, theme: &Theme) -> Line<'static> {
    match row {
        RowView::Heading(title) => Line::from(Span::styled(
            title.clone(),
            Style::default()
                .fg(theme.text_muted)
                .add_modifier(Modifier::BOLD),
        )),
        RowView::Item {
            selected,
            active,
            label,
            detail,
        } => {
            let (label_color, detail_color, prefix) = if *selected {
                (theme.orange, theme.text_dim, "› ")
            } else {
                (theme.text, theme.text_muted, "  ")
            };
            let mark = if *active { "● " } else { "○ " };
            let mut spans = vec![
                Span::styled(prefix, Style::default().fg(theme.orange)),
                Span::styled(
                    mark,
                    Style::default().fg(if *active {
                        theme.orange
                    } else {
                        theme.text_muted
                    }),
                ),
                Span::styled(
                    format!("{label:<18}"),
                    Style::default()
                        .fg(label_color)
                        .add_modifier(Modifier::BOLD),
                ),
            ];
            if !detail.is_empty() {
                spans.push(Span::styled(
                    detail.clone(),
                    Style::default().fg(detail_color),
                ));
            }
            Line::from(spans)
        }
    }
}

fn render_footer(frame: &mut Frame, area: Rect, theme: &Theme, hint: &str) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            hint.to_string(),
            Style::default().fg(theme.text_muted),
        ))),
        area,
    );
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

    /// The model rows as `(provider, role)` pairs, in display order.
    fn model_rows(p: &ModelPickerSurface) -> Vec<(&'static str, &'static str)> {
        p.rows
            .iter()
            .filter_map(|r| match r {
                ModelRow::Model { provider, role, .. } => Some((*provider, *role)),
                ModelRow::Heading(_) => None,
            })
            .collect()
    }

    // ── model picker: row construction ─────────────────────────────────

    #[test]
    fn model_rows_are_grouped_by_provider_with_headings() {
        let p = ModelPickerSurface::new("anthropic", "");
        // Every known provider with a catalog yields a heading followed by
        // its models, and every model row sits under its provider heading.
        let mut current: Option<&str> = None;
        let mut headings = Vec::new();
        for row in &p.rows {
            match row {
                ModelRow::Heading(g) => {
                    current = Some(g);
                    headings.push(*g);
                }
                ModelRow::Model { provider, .. } => {
                    assert_eq!(Some(*provider), current, "model under wrong heading");
                }
            }
        }
        // The grouping covers the catalog providers in known order.
        let expected: Vec<&str> = known_providers()
            .iter()
            .filter(|p| !models_for_provider(p).is_empty())
            .copied()
            .collect();
        assert_eq!(headings, expected);
        // At least anthropic:opus and openai-chatgpt:5.5 are present.
        let pairs = model_rows(&p);
        assert!(pairs.contains(&("anthropic", "opus")));
        assert!(pairs.contains(&("openai-chatgpt", "5.5")));
    }

    #[test]
    fn model_picker_marks_the_active_model_as_selected() {
        // Seeding with an active provider+model lands the selection on that row.
        let p = ModelPickerSurface::new("anthropic", "opus");
        let (provider, role) = p.selected_model().expect("a model must be selected");
        assert_eq!((provider, role), ("anthropic", "opus"));
    }

    // ── model picker: Enter routing ────────────────────────────────────

    #[test]
    fn enter_on_same_provider_emits_bare_model_command() {
        // Active provider == the selected model's provider → `/model <role>`
        // (the existing live model-set path, no provider swap).
        let mut app = App::new();
        app.config.provider = "anthropic".into();
        app.config.model = "opus".into();
        let mut p = ModelPickerSurface::new("anthropic", "opus");
        // Move to a different anthropic model (still same provider).
        p.handle_key(key(KeyCode::Down), &mut app);
        let (provider, role) = p.selected_model().unwrap();
        assert_eq!(provider, "anthropic");
        match p.handle_key(key(KeyCode::Enter), &mut app) {
            SurfaceAction::Command(line) => assert_eq!(line, format!("/model {role}")),
            other => panic!("expected a bare /model command, got {other:?}"),
        }
    }

    #[test]
    fn enter_on_different_provider_emits_qualified_command() {
        // Active provider differs from the selected model's provider → the
        // two-arg `/model <provider> <role>` form so the dispatch routes the
        // swap through apply_provider_swap (OAuth precheck) before the set.
        let mut app = App::new();
        app.config.provider = "anthropic".into();
        app.config.model = "opus".into();
        // Build the picker, then point the selection at an openai-chatgpt row.
        let mut p = ModelPickerSurface::new("anthropic", "opus");
        let target = p
            .rows
            .iter()
            .position(|r| matches!(r, ModelRow::Model { provider, role, .. } if *provider == "openai-chatgpt" && *role == "5.5"))
            .expect("openai-chatgpt:5.5 row must exist");
        p.selected = target;
        match p.handle_key(key(KeyCode::Enter), &mut app) {
            SurfaceAction::Command(line) => {
                assert_eq!(line, "/model openai-chatgpt 5.5");
            }
            other => panic!("expected a qualified /model command, got {other:?}"),
        }
    }

    // ── navigation skips headings + clamps ─────────────────────────────

    #[test]
    fn model_navigation_skips_headings_and_clamps() {
        let mut app = App::new();
        let mut p = ModelPickerSurface::new("anthropic", "opus");
        // Up to the top: clamps on the first model row.
        for _ in 0..p.rows.len() {
            p.handle_key(key(KeyCode::Up), &mut app);
        }
        assert!(p.selected_model().is_some());
        // Down past the end clamps on the last model row.
        for _ in 0..(p.rows.len() * 2) {
            p.handle_key(key(KeyCode::Down), &mut app);
        }
        let last = p.selected;
        p.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(p.selected, last);
        assert!(p.selected_model().is_some());
    }

    #[test]
    fn model_esc_closes_overlay() {
        let mut app = App::new();
        let mut p = ModelPickerSurface::new("anthropic", "opus");
        assert!(matches!(
            p.handle_key(key(KeyCode::Esc), &mut app),
            SurfaceAction::CloseOverlay
        ));
    }

    // ── provider picker ────────────────────────────────────────────────

    #[test]
    fn provider_picker_marks_active_and_selects_it() {
        let p = ProviderPickerSurface::new("openai");
        assert_eq!(p.providers[p.selected], "openai");
    }

    #[test]
    fn provider_enter_emits_provider_swap_command() {
        let mut app = App::new();
        app.config.provider = "anthropic".into();
        let mut p = ProviderPickerSurface::new("anthropic");
        // Move down one to a different provider and select it.
        p.handle_key(key(KeyCode::Down), &mut app);
        let name = p.providers[p.selected];
        match p.handle_key(key(KeyCode::Enter), &mut app) {
            SurfaceAction::Command(line) => assert_eq!(line, format!("/provider {name}")),
            other => panic!("expected a /provider command, got {other:?}"),
        }
    }

    #[test]
    fn provider_navigation_clamps_at_both_ends() {
        let mut app = App::new();
        let mut p = ProviderPickerSurface::new(known_providers()[0]);
        // Up from index 0 clamps.
        p.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(p.selected, 0);
        // Down past the end clamps on the last provider.
        for _ in 0..(p.providers.len() * 2) {
            p.handle_key(key(KeyCode::Down), &mut app);
        }
        assert_eq!(p.selected, p.providers.len() - 1);
    }

    #[test]
    fn provider_esc_closes_overlay() {
        let mut app = App::new();
        let mut p = ProviderPickerSurface::new("anthropic");
        assert!(matches!(
            p.handle_key(key(KeyCode::Esc), &mut app),
            SurfaceAction::CloseOverlay
        ));
    }

    // ── render smoke ───────────────────────────────────────────────────

    #[test]
    fn pickers_render_without_panicking() {
        let mut app = App::new();
        app.config.provider = "anthropic".into();
        app.config.model = "opus".into();
        let theme = Theme::no_color();
        let mut model = ModelPickerSurface::new("anthropic", "opus");
        let mut provider = ProviderPickerSurface::new("anthropic");
        for (w, h) in [(80, 24), (1, 1), (10, 4)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).expect("terminal");
            term.draw(|f| model.render(f, f.area(), &app, &theme))
                .expect("render model picker");
            let mut term2 = Terminal::new(TestBackend::new(w, h)).expect("terminal");
            term2
                .draw(|f| provider.render(f, f.area(), &app, &theme))
                .expect("render provider picker");
        }
        // The active model marker reaches the rendered model picker.
        let mut term = Terminal::new(TestBackend::new(80, 24)).expect("terminal");
        term.draw(|f| model.render(f, f.area(), &app, &theme))
            .expect("render");
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("anthropic"), "provider heading must render");
        assert!(text.contains('●'), "active marker must render");
    }
}
