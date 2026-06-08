//! Plugins surface — the `/plugins` marketplace panel (installed vs
//! available).
//!
//! Implements T1.7. Design: `ux-krug-sutherland.md` §3c.
//!
//! The panel has two sections — INSTALLED and AVAILABLE — rendered as one
//! navigable list. Krug's "the affordance *is* the state": an installed
//! row carries a `✓` and removes on `⏎`; an available row carries a `+`
//! and installs on `⏎`. The footer states reversibility plainly
//! ("removable anytime") to kill install/remove anxiety.
//!
//! Backend boundary: this surface is read-only against
//! `crate::plugin::{install,registry}`. `list_installed()` and
//! `Registry::load_default()` populate the lists; install/remove never
//! execute here — they emit a `SurfaceAction::Command` (`/plugins install
//! <name>` / `/plugins remove <name>`) for the Wave-2 dispatcher to run.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph, Wrap};

use std::path::PathBuf;

use crate::plugin::install::{self, install_from_registry, list_installed};
use crate::plugin::manifest::PluginManifest;
use crate::plugin::registry::Registry;
use crate::tui::app::App;
use crate::tui::surfaces::{Surface, SurfaceAction, SurfaceId};
use crate::tui::theme::Theme;
use crate::tui::widgets::panel;

/// Which section a list row belongs to — drives the verb and the glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    /// An installed plugin — `✓` glyph, removes on `⏎`.
    Installed,
    /// A registry plugin not yet installed — `+` glyph, installs on `⏎`.
    Available,
}

/// One navigable row: a plugin manifest tagged with its section.
struct PluginRow {
    /// The plugin's manifest (name, version, description).
    manifest: PluginManifest,
    /// The section this row belongs to.
    section: Section,
}

impl PluginRow {
    /// The slash-command this row's `⏎` action emits for Wave-2 dispatch.
    fn command(&self) -> String {
        match self.section {
            Section::Installed => format!("/plugins remove {}", self.manifest.name),
            Section::Available => format!("/plugins install {}", self.manifest.name),
        }
    }
}

/// The `/plugins` marketplace surface.
///
/// All state here is surface-local: the loaded plugin rows, the cursor,
/// the details-view flag, and a one-off load-error message. Nothing is
/// stored on `App` — the lists are re-read from the read-only backend on
/// `on_enter`.
pub struct PluginsSurface {
    /// Installed rows first, then available rows. Empty until `on_enter`
    /// (or `reload`) populates it.
    rows: Vec<PluginRow>,
    /// Cursor into `rows`. Always `< rows.len()` while `rows` is
    /// non-empty; `0` and inert when `rows` is empty.
    selected: usize,
    /// `true` while the details view for the selected row is open (`i`
    /// toggles it; `esc`/`i` close it).
    details_open: bool,
    /// A backend load failure to surface instead of the lists. `None` on
    /// a clean load.
    load_error: Option<String>,
}

impl PluginsSurface {
    /// Construct an empty plugins surface. The lists are loaded lazily on
    /// `on_enter` so a freshly-built surface never touches the
    /// filesystem.
    pub fn new() -> Self {
        Self {
            rows: Vec::new(),
            selected: 0,
            details_open: false,
            load_error: None,
        }
    }

    /// Re-read installed + available plugins from the read-only backend.
    ///
    /// Installed plugins come from `install::list_installed()` against the
    /// default install root (`dirs::data_dir()/wayland-core/plugins`).
    /// Available plugins come from the embedded default `Registry`, minus
    /// any name already installed (an installed plugin is never also
    /// offered for install). A backend failure is captured in
    /// `load_error` rather than panicking — the surface stays renderable.
    fn reload(&mut self) {
        self.rows.clear();
        self.load_error = None;

        let installed = match installed_manifests() {
            Ok(list) => list,
            Err(e) => {
                self.load_error = Some(format!("could not read installed plugins: {e}"));
                Vec::new()
            }
        };
        let installed_names: Vec<String> = installed.iter().map(|m| m.name.clone()).collect();

        for manifest in installed {
            self.rows.push(PluginRow {
                manifest,
                section: Section::Installed,
            });
        }

        match Registry::load_default() {
            Ok(reg) => {
                for manifest in reg.list_available() {
                    if installed_names.iter().any(|n| n == &manifest.name) {
                        continue;
                    }
                    self.rows.push(PluginRow {
                        manifest: manifest.clone(),
                        section: Section::Available,
                    });
                }
            }
            Err(e) => {
                // A registry failure only loses the AVAILABLE section;
                // keep the INSTALLED rows that already loaded. Only set
                // the error banner if nothing at all loaded.
                if self.rows.is_empty() {
                    self.load_error = Some(format!("could not read plugin registry: {e}"));
                }
            }
        }

        // Clamp the cursor — the row set just changed under it.
        if self.selected >= self.rows.len() {
            self.selected = self.rows.len().saturating_sub(1);
        }
    }

    /// Move the cursor down one row, stopping at the last row.
    fn move_down(&mut self) {
        if !self.rows.is_empty() && self.selected + 1 < self.rows.len() {
            self.selected += 1;
        }
    }

    /// Move the cursor up one row, stopping at the first row.
    fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// The number of installed plugins currently loaded.
    fn installed_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.section == Section::Installed)
            .count()
    }

    /// The number of available (not-yet-installed) plugins loaded.
    fn available_count(&self) -> usize {
        self.rows
            .iter()
            .filter(|r| r.section == Section::Available)
            .count()
    }

    /// Render the two-section plugin list into `area`.
    fn render_list(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut lines: Vec<Line> = Vec::new();

        if let Some(err) = &self.load_error {
            lines.push(Line::from(Span::styled(
                err.clone(),
                Style::default().fg(theme.error),
            )));
            let para = Paragraph::new(lines)
                .wrap(Wrap { trim: true })
                .style(Style::default().bg(theme.surface));
            frame.render_widget(para, area);
            return;
        }

        // INSTALLED header.
        lines.push(section_header(
            "INSTALLED",
            &self.installed_count().to_string(),
            theme,
        ));
        if self.installed_count() == 0 {
            lines.push(empty_hint("no plugins installed yet", theme));
        }

        let mut rendered_available_header = false;
        for (idx, row) in self.rows.iter().enumerate() {
            if row.section == Section::Available && !rendered_available_header {
                lines.push(Line::from(""));
                lines.push(section_header("AVAILABLE", "from registry", theme));
                if self.available_count() == 0 {
                    lines.push(empty_hint("nothing else in the registry", theme));
                }
                rendered_available_header = true;
            }
            lines.push(self.render_row(row, idx == self.selected, theme));
        }
        // The AVAILABLE header still renders even if there are zero
        // available rows (so the section is never silently missing).
        if !rendered_available_header {
            lines.push(Line::from(""));
            lines.push(section_header("AVAILABLE", "from registry", theme));
            lines.push(empty_hint("nothing else in the registry", theme));
        }

        let para = Paragraph::new(lines).style(Style::default().bg(theme.surface));
        frame.render_widget(para, area);
    }

    /// Render one plugin row: glyph, name, version, description. The
    /// selected row is highlighted on the `surface_hover` background.
    fn render_row(&self, row: &PluginRow, selected: bool, theme: &Theme) -> Line<'static> {
        let (glyph, glyph_color) = match row.section {
            Section::Installed => ("✓", theme.success),
            Section::Available => ("+", theme.orange),
        };
        let name_style = if selected {
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text)
        };
        let row_bg = if selected {
            Style::default().bg(theme.surface_hover)
        } else {
            Style::default().bg(theme.surface)
        };

        let cursor = if selected { "▸ " } else { "  " };
        let mut spans = vec![
            Span::styled(cursor, Style::default().fg(theme.orange)),
            Span::styled(format!("{glyph} "), Style::default().fg(glyph_color)),
            Span::styled(format!("{:<22}", row.manifest.name), name_style),
            Span::styled(
                format!("{:<9}", row.manifest.version),
                Style::default().fg(theme.text_dim),
            ),
            Span::styled(
                row.manifest.description.clone(),
                Style::default().fg(theme.text_dim),
            ),
        ];
        if row.manifest.requires_sandbox {
            spans.push(Span::styled(
                "  ⚠ sandbox",
                Style::default().fg(theme.warning),
            ));
        }
        Line::from(spans).style(row_bg)
    }

    /// Render the footer: the per-row verb hint and the reversibility
    /// statement. The verb adapts to the selected row's section.
    fn render_footer(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let verb = match self.selected_row().map(|r| r.section) {
            Some(Section::Installed) => "⏎ remove",
            Some(Section::Available) => "⏎ install",
            None => "⏎ install/remove",
        };
        let hint = Line::from(vec![
            Span::styled(format!(" {verb}   "), Style::default().fg(theme.text)),
            Span::styled(
                "↑↓ move   i details   esc close",
                Style::default().fg(theme.text_dim),
            ),
        ]);
        // Reversibility — Krug error-tolerance: state it plainly.
        let reversible = Line::from(Span::styled(
            " installs to ~/…/wayland-core/plugins · you can remove any plugin anytime",
            Style::default().fg(theme.text_muted),
        ));
        let para = Paragraph::new(vec![hint, reversible]).style(Style::default().bg(theme.surface));
        frame.render_widget(para, area);
    }

    /// Render the details overlay for the selected row — what the plugin
    /// is and what it touches, shown *before* the user commits
    /// (Sutherland: earn the right to ask).
    fn render_details(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let Some(row) = self.selected_row() else {
            return;
        };
        let mf = &row.manifest;

        // A centered card over the list.
        let card = centered_rect(area, 64, 14);
        frame.render_widget(Clear, card);
        let block = panel(&format!(" {} — details ", mf.name), theme);
        let inner = block.inner(card);
        frame.render_widget(block, card);

        let state_line = match row.section {
            Section::Installed => Line::from(Span::styled(
                "installed — ⏎ removes it (reversible: reinstall anytime)",
                Style::default().fg(theme.success),
            )),
            Section::Available => Line::from(Span::styled(
                "not installed — ⏎ installs it (reversible: remove anytime)",
                Style::default().fg(theme.orange),
            )),
        };

        let mut lines: Vec<Line> = vec![
            kv("name", &mf.name, theme),
            kv("version", &mf.version, theme),
            kv("does", &mf.description, theme),
            kv(
                "sandbox",
                if mf.requires_sandbox {
                    "yes — runs under the sandboxed-tools surface"
                } else {
                    "no — runs in-process"
                },
                theme,
            ),
        ];
        if mf.dependencies.is_empty() {
            lines.push(kv("depends on", "nothing", theme));
        } else {
            lines.push(kv("depends on", &mf.dependencies.join(", "), theme));
        }
        lines.push(Line::from(""));
        lines.push(state_line);
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "i / esc — close details",
            Style::default().fg(theme.text_muted),
        )));

        let para = Paragraph::new(lines)
            .wrap(Wrap { trim: true })
            .style(Style::default().bg(theme.surface_elevated));
        frame.render_widget(para, inner);
    }

    /// The currently-selected row, or `None` when the list is empty.
    fn selected_row(&self) -> Option<&PluginRow> {
        self.rows.get(self.selected)
    }
}

impl Default for PluginsSurface {
    fn default() -> Self {
        Self::new()
    }
}

impl Surface for PluginsSurface {
    fn id(&self) -> SurfaceId {
        SurfaceId::Plugins
    }

    fn on_enter(&mut self, _app: &mut App) {
        // Re-read the lists every time the surface becomes active so an
        // install/remove run by Wave-2 dispatch is reflected on return.
        self.reload();
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _app: &App, theme: &Theme) {
        let block = panel(" /plugins ", theme);
        let inner = block.inner(area);
        frame.render_widget(block, area);

        // List body fills the panel; a 2-row footer is pinned to the
        // bottom. `Length(2)` for the footer; `Min(0)` for the list.
        let [list_area, footer_area] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(2)]).areas(inner);

        self.render_list(frame, list_area, theme);
        self.render_footer(frame, footer_area, theme);

        if self.details_open {
            self.render_details(frame, inner, theme);
        }
    }

    fn handle_key(&mut self, key: KeyEvent, _app: &mut App) -> SurfaceAction {
        // When the details overlay is open, only `i`/`esc` (close) and
        // navigation are meaningful; everything else is swallowed.
        if self.details_open {
            match key.code {
                KeyCode::Char('i') | KeyCode::Esc => {
                    self.details_open = false;
                }
                KeyCode::Down | KeyCode::Char('j') => self.move_down(),
                KeyCode::Up | KeyCode::Char('k') => self.move_up(),
                _ => {}
            }
            return SurfaceAction::None;
        }

        match key.code {
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_down();
                SurfaceAction::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_up();
                SurfaceAction::None
            }
            KeyCode::Char('i') => {
                // Details view only opens when there is a row to detail.
                if self.selected_row().is_some() {
                    self.details_open = true;
                }
                SurfaceAction::None
            }
            KeyCode::Enter => {
                // The one verb per row: install an available plugin /
                // remove an installed one. The surface NEVER runs the
                // install itself — it emits the command for Wave-2
                // dispatch.
                match self.selected_row() {
                    Some(row) => SurfaceAction::Command(row.command()),
                    None => SurfaceAction::None,
                }
            }
            KeyCode::Char('r') => {
                // Manual refresh — re-read both lists from the backend.
                self.reload();
                SurfaceAction::None
            }
            KeyCode::Esc => SurfaceAction::CloseOverlay,
            _ => SurfaceAction::None,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Backend helpers
// ─────────────────────────────────────────────────────────────────────────

/// The default plugin install root: `dirs::data_dir()/wayland-core/plugins`.
/// Mirrors the path logic in `crate::plugin::run` so the TUI install/remove
/// verbs target the same directory the `wayland-core plugin` subcommand does.
pub(crate) fn plugin_install_root() -> anyhow::Result<PathBuf> {
    let base = dirs::data_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine the data directory"))?;
    Ok(base.join("wayland-core").join("plugins"))
}

/// Read installed plugin manifests from the default install root.
///
/// `list_installed` already returns an empty Vec when that directory does
/// not exist (first-run case), so the only error path is a real filesystem
/// fault.
fn installed_manifests() -> anyhow::Result<Vec<PluginManifest>> {
    Ok(list_installed(&plugin_install_root()?)?)
}

/// Execute a `/plugins install <name>` / `/plugins remove <name>` verb
/// parsed from the full command line, returning a human-facing result line
/// for the system transcript. A bare `/plugins` (no verb) returns an empty
/// string — the caller opens the marketplace panel instead.
///
/// Registry installs are local and network-free (`install_from_registry`
/// copies a manifest from the embedded default registry and writes a JSON
/// install record); `remove` deletes that record. Both surface a real error
/// rather than silently succeeding, closing the v0.9.x "install is a no-op"
/// gap where the `<name>` argument was dropped on the floor.
pub(crate) fn run_plugins_verb(line: &str) -> String {
    let mut toks = line.split_whitespace();
    let _cmd = toks.next(); // "/plugins"
    let verb = toks.next().unwrap_or("");
    let name = toks.next().unwrap_or("").trim();

    match verb {
        "" => String::new(),
        "install" | "add" => {
            if name.is_empty() {
                return "usage: /plugins install <name>".to_string();
            }
            let root = match plugin_install_root() {
                Ok(r) => r,
                Err(e) => return format!("could not locate the plugin directory: {e}"),
            };
            let registry = match Registry::load_default() {
                Ok(r) => r,
                Err(e) => return format!("could not load the plugin registry: {e}"),
            };
            match install_from_registry(&registry, name, &root) {
                Ok(()) => format!("Installed plugin `{name}`."),
                Err(e) => format!("Could not install `{name}`: {e}"),
            }
        }
        "remove" | "rm" | "uninstall" => {
            if name.is_empty() {
                return "usage: /plugins remove <name>".to_string();
            }
            let root = match plugin_install_root() {
                Ok(r) => r,
                Err(e) => return format!("could not locate the plugin directory: {e}"),
            };
            match install::remove(&root, name) {
                Ok(()) => format!("Removed plugin `{name}`."),
                Err(e) => format!("Could not remove `{name}`: {e}"),
            }
        }
        other => {
            format!("unknown /plugins subcommand `{other}` — use install|remove <name>")
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────
// Render helpers
// ─────────────────────────────────────────────────────────────────────────

/// A section header line: a bold label plus a dim count/qualifier.
fn section_header(label: &str, qualifier: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!(" {label}  "),
            Style::default()
                .fg(theme.orange)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(qualifier.to_string(), Style::default().fg(theme.text_muted)),
    ])
}

/// A muted "this section is empty" hint line.
fn empty_hint(text: &str, theme: &Theme) -> Line<'static> {
    Line::from(Span::styled(
        format!("   {text}"),
        Style::default().fg(theme.text_muted),
    ))
}

/// A `key: value` line for the details overlay — dim key, normal value.
fn kv(key: &str, value: &str, theme: &Theme) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<13}"), Style::default().fg(theme.text_muted)),
        Span::styled(value.to_string(), Style::default().fg(theme.text)),
    ])
}

/// Compute a centered `width × height` rectangle inside `area`, clamped
/// so it never exceeds the available space.
fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use ratatui::layout::Rect;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    /// Build a surface with deterministic, fixture rows so tests don't
    /// depend on the host filesystem / install root.
    fn fixture_surface() -> PluginsSurface {
        let installed = PluginRow {
            manifest: PluginManifest {
                name: "wayland-ollama".into(),
                version: "0.6.1".into(),
                requires_sandbox: false,
                description: "local inference provider".into(),
                dependencies: Vec::new(),
            },
            section: Section::Installed,
        };
        let available = PluginRow {
            manifest: PluginManifest {
                name: "wayland-cua".into(),
                version: "0.6.1".into(),
                requires_sandbox: true,
                description: "computer use".into(),
                dependencies: vec!["wayland-browser".into()],
            },
            section: Section::Available,
        };
        PluginsSurface {
            rows: vec![installed, available],
            selected: 0,
            details_open: false,
            load_error: None,
        }
    }

    fn render_to_string(surface: &mut PluginsSurface) -> String {
        let app = App::new();
        let theme = Theme::no_color();
        let mut terminal = Terminal::new(TestBackend::new(80, 20)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, Rect::new(0, 0, 80, 20), &app, &theme))
            .expect("render plugins surface");
        let buf = terminal.backend().buffer();
        let mut out = String::new();
        for y in 0..20 {
            for x in 0..80 {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn surface_reports_its_identity() {
        assert_eq!(PluginsSurface::new().id(), SurfaceId::Plugins);
    }

    #[test]
    fn render_shows_both_sections_and_their_rows() {
        let mut surface = fixture_surface();
        let out = render_to_string(&mut surface);
        assert!(out.contains("INSTALLED"), "missing INSTALLED header");
        assert!(out.contains("AVAILABLE"), "missing AVAILABLE header");
        assert!(out.contains("wayland-ollama"), "missing installed row");
        assert!(out.contains("wayland-cua"), "missing available row");
        // The footer states reversibility plainly.
        assert!(
            out.contains("remove any plugin anytime"),
            "footer must state reversibility"
        );
    }

    #[test]
    fn installed_and_available_rows_carry_distinct_verb_glyphs() {
        let mut surface = fixture_surface();
        let out = render_to_string(&mut surface);
        // Installed: ✓ glyph. Available: + glyph. The affordance is the
        // state (Krug).
        assert!(out.contains("✓"), "installed row missing ✓ glyph");
        assert!(
            out.contains("+ wayland-cua"),
            "available row missing + glyph"
        );
    }

    #[test]
    fn enter_on_an_installed_row_emits_a_remove_command() {
        let mut surface = fixture_surface();
        let mut app = App::new();
        // selected == 0 is the installed row.
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        match action {
            SurfaceAction::Command(cmd) => {
                assert_eq!(cmd, "/plugins remove wayland-ollama");
            }
            _ => panic!("expected a Command action, got something else"),
        }
    }

    #[test]
    fn enter_on_an_available_row_emits_an_install_command() {
        let mut surface = fixture_surface();
        let mut app = App::new();
        // Move down onto the available row, then activate it.
        surface.handle_key(key(KeyCode::Down), &mut app);
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        match action {
            SurfaceAction::Command(cmd) => {
                assert_eq!(cmd, "/plugins install wayland-cua");
            }
            _ => panic!("expected a Command action, got something else"),
        }
    }

    #[test]
    fn enter_never_executes_an_install_only_emits_a_command() {
        // The surface must not run the backend itself — `handle_key`
        // returning a `Command` (not `None`) and the row set being
        // unchanged proves the install was delegated, not executed.
        let mut surface = fixture_surface();
        let mut app = App::new();
        let before = surface.rows.len();
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(matches!(action, SurfaceAction::Command(_)));
        assert_eq!(surface.rows.len(), before, "row set must not change");
        assert!(!surface.details_open);
    }

    #[test]
    fn cursor_moves_within_bounds_and_does_not_run_off_either_end() {
        let mut surface = fixture_surface();
        let mut app = App::new();
        // Up at the top is a no-op.
        surface.handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(surface.selected, 0);
        // Down advances; a second Down stops at the last row.
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.selected, 1);
        surface.handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(surface.selected, 1, "cursor must stop at the last row");
    }

    #[test]
    fn footer_verb_tracks_the_selected_rows_section() {
        let mut surface = fixture_surface();
        let mut app = App::new();
        // Installed row selected → footer reads "remove".
        let out = render_to_string(&mut surface);
        assert!(out.contains("⏎ remove"), "footer should read remove");
        // Move to the available row → footer reads "install".
        surface.handle_key(key(KeyCode::Down), &mut app);
        let out = render_to_string(&mut surface);
        assert!(out.contains("⏎ install"), "footer should read install");
    }

    #[test]
    fn i_key_opens_details_and_closes_it() {
        let mut surface = fixture_surface();
        let mut app = App::new();
        assert!(!surface.details_open);
        surface.handle_key(key(KeyCode::Char('i')), &mut app);
        assert!(surface.details_open, "i must open the details view");

        let out = render_to_string(&mut surface);
        assert!(out.contains("details"), "details overlay must render");
        assert!(
            out.contains("reversible"),
            "details must state reversibility"
        );

        // `i` again closes it; so does `esc`.
        surface.handle_key(key(KeyCode::Char('i')), &mut app);
        assert!(!surface.details_open, "i must toggle details closed");
        surface.handle_key(key(KeyCode::Char('i')), &mut app);
        surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(!surface.details_open, "esc must close details");
    }

    #[test]
    fn enter_is_inert_while_the_details_overlay_is_open() {
        // With details open, Enter must not emit an install/remove — the
        // overlay swallows it so a stray keypress can't commit.
        let mut surface = fixture_surface();
        let mut app = App::new();
        surface.handle_key(key(KeyCode::Char('i')), &mut app);
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(matches!(action, SurfaceAction::None));
    }

    #[test]
    fn esc_with_no_overlay_closes_the_surface() {
        let mut surface = fixture_surface();
        let mut app = App::new();
        let action = surface.handle_key(key(KeyCode::Esc), &mut app);
        assert!(matches!(action, SurfaceAction::CloseOverlay));
    }

    #[test]
    fn empty_surface_renders_both_section_headers_without_panicking() {
        // A surface with no rows (no plugins, empty registry) must still
        // show both sections so the layout is never silently missing.
        let mut surface = PluginsSurface::new();
        let out = render_to_string(&mut surface);
        assert!(out.contains("INSTALLED"));
        assert!(out.contains("AVAILABLE"));
        // Enter on an empty list is a safe no-op.
        let mut app = App::new();
        let action = surface.handle_key(key(KeyCode::Enter), &mut app);
        assert!(matches!(action, SurfaceAction::None));
    }

    #[test]
    fn a_load_error_is_rendered_instead_of_the_lists() {
        let mut surface = PluginsSurface::new();
        surface.load_error = Some("could not read plugin registry: boom".into());
        let out = render_to_string(&mut surface);
        assert!(out.contains("could not read plugin registry"));
    }

    #[test]
    fn renders_on_a_tiny_terminal_without_panicking() {
        // The panel split (list body + 2-row footer) and the centered
        // details card must clamp on a terminal smaller than either.
        let app = App::new();
        let theme = Theme::no_color();
        let mut surface = fixture_surface();
        for (w, h) in [(1u16, 1u16), (6, 3), (12, 5)] {
            let mut terminal = Terminal::new(TestBackend::new(w, h)).expect("test terminal");
            terminal
                .draw(|f| surface.render(f, Rect::new(0, 0, w, h), &app, &theme))
                .expect("render plugins on a tiny terminal");
        }
        // The details overlay must also survive a tiny frame.
        surface.details_open = true;
        let mut terminal = Terminal::new(TestBackend::new(8, 4)).expect("test terminal");
        terminal
            .draw(|f| surface.render(f, Rect::new(0, 0, 8, 4), &app, &theme))
            .expect("render plugins details on a tiny terminal");
    }

    #[test]
    fn details_view_shows_dependencies_and_sandbox_flag() {
        let mut surface = fixture_surface();
        let mut app = App::new();
        // The available fixture row requires a sandbox + has a dependency.
        surface.handle_key(key(KeyCode::Down), &mut app);
        surface.handle_key(key(KeyCode::Char('i')), &mut app);
        let out = render_to_string(&mut surface);
        assert!(out.contains("wayland-browser"), "dependency must show");
        assert!(out.contains("sandbox"), "sandbox field must show");
    }

    #[test]
    fn run_plugins_verb_parses_usage_and_unknown_subcommands_g1() {
        // G1: the `/plugins install|remove <name>` verbs are parsed from the
        // full line (the panel's row-⏎ emits them). These branches return
        // before any filesystem touch, so the test is side-effect-free.
        use super::run_plugins_verb;
        // Bare `/plugins` → empty (the caller opens the marketplace panel).
        assert_eq!(run_plugins_verb("/plugins"), "");
        // Missing name → usage guidance, no install attempt.
        assert_eq!(
            run_plugins_verb("/plugins install"),
            "usage: /plugins install <name>"
        );
        assert_eq!(
            run_plugins_verb("/plugins remove"),
            "usage: /plugins remove <name>"
        );
        // Unknown verb → guidance, never forwarded anywhere.
        assert!(
            run_plugins_verb("/plugins frobnicate foo").contains("unknown /plugins subcommand"),
            "unknown subcommand must be reported"
        );
    }

    #[test]
    fn run_plugins_verb_install_unregistered_name_fails_without_writing_g1() {
        // A syntactically-valid name that isn't in the embedded registry
        // fails cleanly with a real error message and writes NOTHING (the
        // registry lookup fails before the install record is written), so
        // this exercises the real install path with no filesystem side effect.
        use super::run_plugins_verb;
        let msg = run_plugins_verb("/plugins install nonexistent-plugin");
        assert!(
            msg.starts_with("Could not install"),
            "an unregistered plugin must report a real failure, got: {msg}"
        );
        assert!(msg.contains("nonexistent-plugin"));
    }
}
