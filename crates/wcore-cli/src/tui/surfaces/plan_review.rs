//! Plan review surface (surface 06) — the read-only plan-mode review
//! screen.
//!
//! Plan mode is a real engine feature: the model calls the `EnterPlanMode`
//! tool and is restricted to read-only tools until it presents a change
//! set; `ExitPlanMode` ends the restriction. This surface renders that
//! presented change set — a plan-mode banner, the file-scoped plan, a
//! safety panel, and three options — and lets the user decide what to do
//! with it. Entry into this surface is driven by the engine's
//! `EnterPlanMode` tool, NOT a keybind; Wave 2 wires that trigger. This
//! surface only renders the plan and handles its keys.
//!
//! The surface is read-only with respect to `App` — the plan content
//! itself lives on `PlanReviewSurface` (Wave 2 will populate it from the
//! `EnterPlanMode` tool payload). Shared status-bar state (provider,
//! model, context meter) is read from `&App`.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

use crate::tui::app::App;
use crate::tui::surfaces::{Surface, SurfaceAction, SurfaceId};
use crate::tui::theme::Theme;
use crate::tui::widgets::panel;

/// One step of a presented plan — a description plus the files it touches.
///
/// Surface-local: the plan is not part of the frozen `App` contract.
/// Wave 2 builds these from the `EnterPlanMode` tool payload; until then
/// the surface renders a representative plan (see the `#[cfg(test)]`
/// `sample_plan` helper and `PlanModel::default`).
#[derive(Debug, Clone)]
pub struct PlanStep {
    /// Human-readable description of what the step does.
    pub description: String,
    /// The kind of change this step is (`edit`, `new`, ...), shown as a
    /// short verb prefix on the file line.
    pub verb: String,
    /// The file paths this step touches.
    pub files: Vec<String>,
}

/// A presented plan: a title, an intro paragraph, ordered steps, and the
/// safety guarantees the agent attaches to it. Surface-local state.
#[derive(Debug, Clone)]
pub struct PlanModel {
    /// The plan's one-line title (e.g. "Migrate auth module").
    pub title: String,
    /// The agent's prose preamble shown above the plan body.
    pub intro: String,
    /// The ordered steps that make up the plan.
    pub steps: Vec<PlanStep>,
    /// Safety guarantees the agent vouches for (rendered as checks).
    pub safety: Vec<String>,
}

impl Default for PlanModel {
    /// An empty placeholder plan — used before `EnterPlanMode` populates a
    /// real one (Wave 2). Rendered as an explicit empty state, never as a
    /// blank screen.
    fn default() -> Self {
        Self {
            title: String::new(),
            intro: String::new(),
            steps: Vec::new(),
            safety: Vec::new(),
        }
    }
}

impl PlanModel {
    /// Total count of distinct files touched across all steps.
    fn file_count(&self) -> usize {
        let mut all: Vec<&str> = self
            .steps
            .iter()
            .flat_map(|s| s.files.iter().map(String::as_str))
            .collect();
        all.sort_unstable();
        all.dedup();
        all.len()
    }

    /// True when no plan has been presented yet.
    fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

/// The three choices the user can make about a presented plan. The
/// selected one is highlighted; `Up`/`Down` move the selection and the
/// hotkeys (`A` / `R` / `Esc`) act directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PlanChoice {
    /// Approve and run — exit plan mode and execute the plan.
    Run,
    /// Keep planning — stay in plan mode and tell the agent what to adjust.
    KeepPlanning,
    /// Discard — exit plan mode, write nothing, return to the workspace.
    Discard,
}

impl PlanChoice {
    /// The three choices in display order.
    const ALL: [PlanChoice; 3] = [
        PlanChoice::Run,
        PlanChoice::KeepPlanning,
        PlanChoice::Discard,
    ];

    /// This choice's index within `ALL`.
    fn index(self) -> usize {
        Self::ALL.iter().position(|&c| c == self).unwrap_or(0)
    }
}

/// The plan-mode review surface (surface 06).
///
/// Read-only with respect to `App`. The plan content and the currently
/// highlighted option are surface-local state.
pub struct PlanReviewSurface {
    /// The plan being reviewed. Surface-local — Wave 2 populates this from
    /// the `EnterPlanMode` tool payload.
    plan: PlanModel,
    /// The option the user currently has highlighted.
    selected: PlanChoice,
}

impl PlanReviewSurface {
    /// Construct the surface with an empty placeholder plan. Wave 2's
    /// `EnterPlanMode` wiring replaces the plan via `set_plan` before the
    /// surface becomes visible.
    pub fn new() -> Self {
        Self {
            plan: PlanModel::default(),
            selected: PlanChoice::Run,
        }
    }

    /// Install the plan to review. Called by Wave 2 when the engine's
    /// `EnterPlanMode` tool presents a change set; resets the selection to
    /// the default (`Run`).
    pub fn set_plan(&mut self, plan: PlanModel) {
        self.plan = plan;
        self.selected = PlanChoice::Run;
    }

    /// Translate the highlighted choice into a `SurfaceAction`.
    ///
    /// The frozen `SurfaceAction` contract has no plan-specific variant,
    /// so each choice maps to the closest existing one:
    ///
    /// * `Run` → `Command("/exit-plan-mode")` — an engine-facing command
    ///   line. Approving a plan means leaving plan mode and executing;
    ///   the engine exposes that as the `ExitPlanMode` tool, and a
    ///   command line is the contract's channel for engine-facing verbs
    ///   (Wave 2 routes `Command` to the engine bridge). `Command` is
    ///   chosen over `SendMessage` because this is a control verb, not a
    ///   conversational message.
    /// * `KeepPlanning` → `None` — staying in plan mode is the absence of
    ///   a routing effect; the user keeps refining via the composer,
    ///   which stays in plan mode. No surface switch, no engine command.
    /// * `Discard` → `Switch(Workspace)` — discarding leaves plan mode
    ///   and returns to the main workspace, writing nothing.
    fn action_for(&self, choice: PlanChoice) -> SurfaceAction {
        match choice {
            PlanChoice::Run => SurfaceAction::Command("/exit-plan-mode".to_string()),
            PlanChoice::KeepPlanning => SurfaceAction::None,
            PlanChoice::Discard => SurfaceAction::Switch(SurfaceId::Workspace),
        }
    }

    /// Commit a highlighted/hotkey choice. T0-2: `Discard` must clear the
    /// live `app.plan` before returning `Switch(Workspace)` — otherwise the
    /// router's `sync_plan_mode` (called every tick) sees `app.plan` still
    /// `Some`, decides we're "in plan mode", and immediately switches the
    /// user straight back to PlanReview. The bare `Switch` alone traps them.
    fn commit(&self, choice: PlanChoice, app: &mut App) -> SurfaceAction {
        if choice == PlanChoice::Discard {
            app.plan = None;
        }
        self.action_for(choice)
    }
}

impl Default for PlanReviewSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl Surface for PlanReviewSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::PlanReview
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _app: &App, theme: &Theme) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        // Vertical split: the plan-mode banner (1 row), the body (plan +
        // rail), then the option footer. The footer shrinks before the
        // body so a short terminal keeps the plan visible.
        let footer_h = 5u16.min(area.height.saturating_sub(2));
        let [banner_area, body_area, footer_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(footer_h),
        ])
        .areas(area);

        render_banner(frame, banner_area, theme);

        // Body split: the plan transcript on the left, the scope/safety
        // rail on the right (matches the mockup's 2-column layout). On a
        // narrow terminal the rail is dropped entirely — the plan is the
        // essential pane and must never be starved below readability.
        let rail_width = if body_area.width >= 74 { 34 } else { 0 };
        let [plan_area, rail_area] =
            Layout::horizontal([Constraint::Min(40), Constraint::Length(rail_width)])
                .areas(body_area);

        self.render_plan(frame, plan_area, theme);
        if rail_area.width > 0 {
            self.render_rail(frame, rail_area, theme);
        }
        self.render_footer(frame, footer_area, theme);
    }

    fn handle_key(&mut self, key: KeyEvent, app: &mut App) -> SurfaceAction {
        match key.code {
            // Direct hotkeys — act regardless of the highlighted option.
            KeyCode::Char('a') | KeyCode::Char('A') => self.commit(PlanChoice::Run, app),
            KeyCode::Char('r') | KeyCode::Char('R') => self.commit(PlanChoice::KeepPlanning, app),
            KeyCode::Esc => self.commit(PlanChoice::Discard, app),
            // Move the highlight; no routing effect until the user commits.
            KeyCode::Up | KeyCode::Char('k') => {
                let idx = self.selected.index();
                let len = PlanChoice::ALL.len();
                self.selected = PlanChoice::ALL[(idx + len - 1) % len];
                SurfaceAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let idx = self.selected.index();
                self.selected = PlanChoice::ALL[(idx + 1) % PlanChoice::ALL.len()];
                SurfaceAction::None
            }
            // Commit the highlighted option.
            KeyCode::Enter => self.commit(self.selected, app),
            _ => SurfaceAction::None,
        }
    }

    /// On entry, populate the plan from `App::plan` — set by the protocol
    /// bridge when the engine's `EnterPlanMode` tool fires (Wave 2).
    /// Absent a plan the surface keeps its explicit empty state.
    fn on_enter(&mut self, app: &mut App) {
        if let Some(plan) = app.plan.as_ref() {
            self.set_plan(plan_model_from(plan));
        }
    }
}

/// Build the surface's richer `PlanModel` from the bridge's `PlanView`.
///
/// The engine's `EnterPlanMode` payload is free-form prose, so each
/// non-empty body line becomes one `PlanStep`. No per-step file list is
/// parsed (the engine does not structure it); the safety panel carries
/// the plan-mode guarantee that applies to every plan.
fn plan_model_from(plan: &crate::tui::app::PlanView) -> PlanModel {
    let steps: Vec<PlanStep> = plan
        .body
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|line| PlanStep {
            description: line.to_string(),
            verb: "step".to_string(),
            files: Vec::new(),
        })
        .collect();
    PlanModel {
        title: plan.title.clone(),
        intro: "The agent proposed the following plan before making changes.".to_string(),
        steps,
        safety: vec!["Plan mode is read-only — no files change until you approve.".to_string()],
    }
}

impl PlanReviewSurface {
    /// Draw the left column: the agent's intro, the numbered plan steps,
    /// and the "Proceed?" question.
    fn render_plan(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = panel(" Plan ", theme);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        if self.plan.is_empty() {
            let empty = Paragraph::new(Line::from(Span::styled(
                "No plan presented yet — waiting for the agent to enter plan mode.",
                Style::default().fg(theme.text_muted),
            )))
            .wrap(Wrap { trim: true });
            frame.render_widget(empty, inner);
            return;
        }

        let mut lines: Vec<Line> = Vec::new();

        // Title + metadata header.
        lines.push(Line::from(vec![
            Span::styled(
                &self.plan.title,
                Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                format!(
                    "{} steps · {} files",
                    self.plan.steps.len(),
                    self.plan.file_count()
                ),
                Style::default().fg(theme.text_muted),
            ),
        ]));
        lines.push(Line::from(""));

        // The agent's prose preamble.
        for chunk in wrap_text(&self.plan.intro, inner.width.max(1) as usize) {
            lines.push(Line::from(Span::styled(
                chunk,
                Style::default().fg(theme.text_dim),
            )));
        }
        lines.push(Line::from(""));

        // Numbered steps.
        for (i, step) in self.plan.steps.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} ", i + 1),
                    Style::default()
                        .fg(theme.orange)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(&step.description, Style::default().fg(theme.text)),
            ]));
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{} · ", step.verb),
                    Style::default().fg(theme.text_muted),
                ),
                Span::styled(
                    step.files.join(", "),
                    Style::default().fg(theme.orange_light),
                ),
            ]));
        }

        let body = Paragraph::new(lines).wrap(Wrap { trim: false });
        frame.render_widget(body, inner);
    }

    /// Draw the right rail: the plan-scope file list, the estimate, and
    /// the safety guarantees.
    fn render_rail(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let [scope_area, safety_area] =
            Layout::vertical([Constraint::Min(4), Constraint::Length(8)]).areas(area);

        // Plan scope — every distinct file the plan touches.
        let scope_block = panel(" Plan scope ", theme);
        let scope_inner = scope_block.inner(scope_area);
        frame.render_widget(scope_block, scope_area);

        let mut files: Vec<&str> = self
            .plan
            .steps
            .iter()
            .flat_map(|s| s.files.iter().map(String::as_str))
            .collect();
        files.sort_unstable();
        files.dedup();
        let scope_lines: Vec<Line> = if files.is_empty() {
            vec![Line::from(Span::styled(
                "—",
                Style::default().fg(theme.text_muted),
            ))]
        } else {
            files
                .iter()
                .map(|f| {
                    Line::from(vec![
                        Span::styled("• ", Style::default().fg(theme.orange)),
                        Span::styled(*f, Style::default().fg(theme.text_dim)),
                    ])
                })
                .collect()
        };
        frame.render_widget(Paragraph::new(scope_lines), scope_inner);

        // Safety — the agent's guarantees, rendered as success checks.
        let safety_block = panel(" Safety ", theme);
        let safety_inner = safety_block.inner(safety_area);
        frame.render_widget(safety_block, safety_area);

        let safety_lines: Vec<Line> = if self.plan.safety.is_empty() {
            vec![Line::from(Span::styled(
                "no guarantees stated",
                Style::default().fg(theme.text_muted),
            ))]
        } else {
            self.plan
                .safety
                .iter()
                .map(|s| {
                    Line::from(vec![
                        Span::styled("✓ ", Style::default().fg(theme.success)),
                        Span::styled(s.as_str(), Style::default().fg(theme.text_dim)),
                    ])
                })
                .collect()
        };
        frame.render_widget(
            Paragraph::new(safety_lines).wrap(Wrap { trim: true }),
            safety_inner,
        );
    }

    /// Draw the option footer: the "Proceed?" question and the three
    /// choices, with the highlighted one accented.
    fn render_footer(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let block = Block::default()
            .borders(Borders::TOP)
            .border_style(Style::default().fg(theme.border))
            .style(Style::default().bg(theme.surface));
        let inner = block.inner(area);
        frame.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();
        lines.push(Line::from(Span::styled(
            format!(
                "This plan touches {} files. Proceed?",
                self.plan.file_count()
            ),
            Style::default().fg(theme.text_dim),
        )));

        for choice in PlanChoice::ALL {
            lines.push(self.option_line(choice, theme));
        }

        frame.render_widget(Paragraph::new(lines), inner);
    }

    /// Render one option row, accented when it is the highlighted choice.
    fn option_line(&self, choice: PlanChoice, theme: &Theme) -> Line<'static> {
        let selected = choice == self.selected;
        // Single-key hints are lowercase per ux finding #17 — they must
        // match the lowercase keymap labels the `?` overlay renders.
        let (label, hotkey) = match choice {
            PlanChoice::Run => ("Approve & run — exit plan mode, execute the plan", "a"),
            PlanChoice::KeepPlanning => ("Keep planning — tell Wayland what to adjust", "r"),
            PlanChoice::Discard => ("Discard — exit plan mode, write nothing", "esc"),
        };
        let marker = if selected { "▸ " } else { "  " };
        let label_style = if selected {
            Style::default()
                .fg(theme.orange)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        Line::from(vec![
            Span::styled(marker, Style::default().fg(theme.orange)),
            Span::styled(label.to_string(), label_style),
            Span::raw("  "),
            Span::styled(format!("[{hotkey}]"), Style::default().fg(theme.text_muted)),
        ])
    }
}

/// Draw the plan-mode banner — the read-only-mode notice that must always
/// be present on this surface (per the mockup, surface 06).
fn render_banner(frame: &mut Frame, area: Rect, theme: &Theme) {
    let banner = Paragraph::new(Line::from(vec![
        Span::styled(
            " Plan mode ",
            Style::default()
                .fg(theme.bg)
                .bg(theme.orange)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            "read-only — Wayland writes nothing until you approve",
            Style::default().fg(theme.text_dim),
        ),
        Span::raw("  "),
        Span::styled(
            "(EnterPlanMode / ExitPlanMode)",
            Style::default().fg(theme.text_muted),
        ),
    ]))
    .style(Style::default().bg(theme.surface));
    frame.render_widget(banner, area);
}

/// Naive word-wrap to `width` columns — used for the prose intro so it
/// reflows inside the plan panel without a heavyweight dependency.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 || text.is_empty() {
        return vec![text.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        if current.is_empty() {
            current.push_str(word);
        } else if current.len() + 1 + word.len() <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyEvent, KeyModifiers};

    /// A representative plan — the T0.5 fixture set has no plan fixture
    /// (plan mode produces no distinct `ProtocolEvent` stream the bridge
    /// decodes), so the surface tests render from this local helper. It
    /// mirrors the mockup's surface-06 example.
    fn sample_plan() -> PlanModel {
        PlanModel {
            title: "Migrate auth module → ProviderCompat".to_string(),
            intro: "I read all four provider implementations. Here is a four-step \
                    migration that keeps behavior identical and lands clippy-clean. \
                    Nothing is written until you approve."
                .to_string(),
            steps: vec![
                PlanStep {
                    description: "Add resolve_field() to ProviderCompat.".to_string(),
                    verb: "edit".to_string(),
                    files: vec!["crates/wcore-config/src/compat.rs".to_string()],
                },
                PlanStep {
                    description: "Route Anthropic + OpenAI through the helper.".to_string(),
                    verb: "edit".to_string(),
                    files: vec![
                        "src/auth/anthropic.rs".to_string(),
                        "src/auth/openai.rs".to_string(),
                    ],
                },
                PlanStep {
                    description: "Backfill Bedrock + Vertex through the same path.".to_string(),
                    verb: "edit".to_string(),
                    files: vec![
                        "src/auth/bedrock.rs".to_string(),
                        "src/auth/vertex.rs".to_string(),
                    ],
                },
                PlanStep {
                    description: "Add a parity test for every provider.".to_string(),
                    verb: "new".to_string(),
                    files: vec!["crates/wcore-config/tests/compat_parity.rs".to_string()],
                },
            ],
            safety: vec![
                "no public API changes".to_string(),
                "parity test gates the change".to_string(),
                "atomic commits · file rollback safe".to_string(),
            ],
        }
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Render the surface into a `TestBackend` and return the flattened
    /// buffer text — used by the render-snapshot assertions.
    fn render_to_string(surface: &mut PlanReviewSurface, w: u16, h: u16) -> String {
        let app = App::new();
        let theme = Theme::no_color();
        let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, f.area(), &app, &theme))
            .expect("render plan review");
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

    fn surface_with_plan() -> PlanReviewSurface {
        let mut s = PlanReviewSurface::new();
        s.set_plan(sample_plan());
        s
    }

    #[test]
    fn id_is_plan_review() {
        assert_eq!(PlanReviewSurface::new().id(), SurfaceId::PlanReview);
    }

    #[test]
    fn file_count_dedups_across_steps() {
        // Two steps share no files here, but a file referenced twice must
        // count once. compat.rs appears once; total distinct = 6.
        assert_eq!(sample_plan().file_count(), 6);
    }

    #[test]
    fn render_shows_the_plan_mode_banner() {
        let mut s = surface_with_plan();
        let out = render_to_string(&mut s, 100, 32);
        assert!(out.contains("Plan mode"), "banner missing:\n{out}");
        assert!(
            out.contains("read-only"),
            "read-only notice missing:\n{out}"
        );
    }

    #[test]
    fn render_shows_the_plan_title_and_steps() {
        let mut s = surface_with_plan();
        let out = render_to_string(&mut s, 100, 36);
        assert!(out.contains("ProviderCompat"), "plan title missing:\n{out}");
        assert!(
            out.contains("resolve_field"),
            "step 1 description missing:\n{out}"
        );
        assert!(
            out.contains("parity test"),
            "step 4 description missing:\n{out}"
        );
    }

    #[test]
    fn render_shows_the_safety_panel() {
        let mut s = surface_with_plan();
        let out = render_to_string(&mut s, 100, 36);
        assert!(out.contains("Safety"), "safety panel title missing:\n{out}");
        assert!(
            out.contains("no public API changes"),
            "safety item missing:\n{out}"
        );
    }

    #[test]
    fn render_shows_all_three_options() {
        let mut s = surface_with_plan();
        let out = render_to_string(&mut s, 100, 36);
        assert!(out.contains("Approve & run"), "Run option missing:\n{out}");
        assert!(
            out.contains("Keep planning"),
            "Keep planning option missing:\n{out}"
        );
        assert!(out.contains("Discard"), "Discard option missing:\n{out}");
    }

    #[test]
    fn renders_on_a_narrow_terminal_without_panicking() {
        // Below 74 columns the rail is dropped; the plan pane must still
        // render. A 1×1 frame must not panic any layout split.
        let mut s = surface_with_plan();
        let narrow = render_to_string(&mut s, 50, 24);
        assert!(narrow.contains("ProviderCompat"), "plan lost when narrow");
        // The rail's "Plan scope" panel is gone on a narrow terminal.
        assert!(
            !narrow.contains("Plan scope"),
            "rail should be hidden on a narrow terminal"
        );
        let _ = render_to_string(&mut s, 1, 1);
        let _ = render_to_string(&mut s, 8, 3);
    }

    #[test]
    fn empty_plan_renders_a_designed_empty_state() {
        // A surface with no plan must not show a blank panel.
        let mut s = PlanReviewSurface::new();
        let out = render_to_string(&mut s, 100, 32);
        assert!(
            out.contains("No plan presented yet"),
            "empty state missing:\n{out}"
        );
    }

    #[test]
    fn run_hotkey_emits_exit_plan_mode_command() {
        // `A` approves the plan — maps to the engine-facing command line
        // that exits plan mode and runs.
        let mut s = surface_with_plan();
        let mut app = App::new();
        match s.handle_key(key(KeyCode::Char('a')), &mut app) {
            SurfaceAction::Command(line) => assert_eq!(line, "/exit-plan-mode"),
            _ => panic!("expected Command from the Run hotkey"),
        }
    }

    #[test]
    fn keep_planning_hotkey_emits_none() {
        // `R` keeps planning — staying in plan mode is the absence of a
        // routing effect.
        let mut s = surface_with_plan();
        let mut app = App::new();
        assert!(matches!(
            s.handle_key(key(KeyCode::Char('r')), &mut app),
            SurfaceAction::None
        ));
    }

    #[test]
    fn discard_hotkey_switches_to_workspace() {
        // `Esc` discards — leaves plan mode for the main workspace.
        let mut s = surface_with_plan();
        let mut app = App::new();
        assert!(matches!(
            s.handle_key(key(KeyCode::Esc), &mut app),
            SurfaceAction::Switch(SurfaceId::Workspace)
        ));
    }

    #[test]
    fn enter_commits_the_highlighted_option() {
        // The default highlight is Run; Enter on it emits the Run action.
        let mut s = surface_with_plan();
        let mut app = App::new();
        match s.handle_key(key(KeyCode::Enter), &mut app) {
            SurfaceAction::Command(line) => assert_eq!(line, "/exit-plan-mode"),
            _ => panic!("expected Command from the default selection"),
        }
    }

    #[test]
    fn arrow_keys_move_the_selection_and_enter_follows_it() {
        // Down moves Run → KeepPlanning; Enter then emits KeepPlanning's
        // action (`None`). Down again → Discard → Enter → Switch.
        let mut s = surface_with_plan();
        let mut app = App::new();

        assert!(matches!(
            s.handle_key(key(KeyCode::Down), &mut app),
            SurfaceAction::None
        ));
        assert_eq!(s.selected, PlanChoice::KeepPlanning);
        assert!(matches!(
            s.handle_key(key(KeyCode::Enter), &mut app),
            SurfaceAction::None
        ));

        s.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(s.selected, PlanChoice::Discard);
        assert!(matches!(
            s.handle_key(key(KeyCode::Enter), &mut app),
            SurfaceAction::Switch(SurfaceId::Workspace)
        ));
    }

    #[test]
    fn selection_wraps_at_both_ends() {
        let mut s = surface_with_plan();
        let mut app = App::new();
        // Up from the first option (Run) wraps to the last (Discard).
        s.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(s.selected, PlanChoice::Discard);
        // Down from the last wraps back to the first.
        s.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(s.selected, PlanChoice::Run);
    }

    #[test]
    fn unmapped_key_is_inert() {
        let mut s = surface_with_plan();
        let mut app = App::new();
        assert!(matches!(
            s.handle_key(key(KeyCode::Char('z')), &mut app),
            SurfaceAction::None
        ));
        // The selection is untouched by an unmapped key.
        assert_eq!(s.selected, PlanChoice::Run);
    }

    #[test]
    fn set_plan_resets_the_selection() {
        let mut s = surface_with_plan();
        let mut app = App::new();
        s.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(s.selected, PlanChoice::KeepPlanning);
        // Re-presenting a plan returns the highlight to the safe default.
        s.set_plan(sample_plan());
        assert_eq!(s.selected, PlanChoice::Run);
    }

    #[test]
    fn wrap_text_breaks_on_word_boundaries() {
        let wrapped = wrap_text("alpha beta gamma delta", 11);
        // "alpha beta" = 10 cols fits; "+ gamma" would be 16, so it breaks.
        assert_eq!(wrapped[0], "alpha beta");
        assert!(wrapped.len() > 1);
        // Every produced line stays within the width budget.
        for line in &wrapped {
            assert!(line.len() <= 11, "line over budget: {line:?}");
        }
    }
}
