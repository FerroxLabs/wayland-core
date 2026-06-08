//! Declarative context-aware keybinding layer + `?` help model.
//!
//! FROZEN Wave-0 public surface; STUB bodies (T0.6 fills them).
//!
//! Keybindings are declared per *context* — a global set plus one set per
//! surface. `Keymap::resolve` answers "what does this key do here", and
//! `Keymap::help` produces the rows the `?` overlay renders.

use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::tui::app::App;
use crate::tui::surfaces::{Surface, SurfaceAction, SurfaceId};
use crate::tui::theme::Theme;

/// The context a key is pressed in: the global layer or a specific
/// surface. FROZEN Wave-0 contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyContext {
    /// Bindings active on every surface.
    Global,
    /// Bindings active only on the named surface.
    Surface(SurfaceId),
}

/// A single declared binding: a key, the action it names, and a
/// human-readable description for the `?` help overlay. FROZEN Wave-0
/// contract.
#[derive(Debug, Clone)]
pub struct KeyBinding {
    /// The key that triggers this binding.
    pub key: KeyEvent,
    /// A stable identifier for the action the binding maps to.
    pub action: String,
    /// A one-line description shown in the help overlay.
    pub description: String,
}

/// One row of the `?` help overlay: a key label and what it does.
/// FROZEN Wave-0 contract.
#[derive(Debug, Clone)]
pub struct HelpEntry {
    /// The rendered key label (e.g. `Tab`, `Ctrl+C`).
    pub key_label: String,
    /// The action description.
    pub description: String,
}

/// The declarative keymap — global + per-surface binding sets.
/// FROZEN Wave-0 contract.
#[derive(Debug, Default)]
pub struct Keymap {
    /// All declared bindings, each tagged with the context it applies in.
    bindings: Vec<(KeyContext, KeyBinding)>,
}

impl Keymap {
    /// Build the default keymap with all global + per-surface bindings.
    ///
    /// The global layer carries the keyboard universals every surface
    /// honors (`Esc`, `Tab`, `Shift+Tab`, `Ctrl+C`, `?`). Per-surface
    /// layers add or override bindings for one surface — the mockup
    /// (`mockup.html`) and `ux-krug-sutherland.md` are the visual spec
    /// for which keys each surface exposes.
    pub fn default_map() -> Self {
        use SurfaceId::{
            AgentNav, AgentTranscript, Config, Diagnostics, Onboarding, Palette, PlanReview,
            Plugins, SubAgents, Workspace,
        };

        let bindings = vec![
            // --- Global universals -----------------------------------
            // Active on every surface; per-surface bindings override
            // these for the same key in `resolve`.
            (
                KeyContext::Global,
                binding(plain(KeyCode::Esc), "cancel", "cancel / close / go back"),
            ),
            (
                KeyContext::Global,
                binding(plain(KeyCode::Tab), "next.surface", "next surface"),
            ),
            (
                KeyContext::Global,
                binding(plain(KeyCode::BackTab), "prev.surface", "previous surface"),
            ),
            (
                KeyContext::Global,
                binding(ctrl(KeyCode::Char('c')), "quit", "quit Wayland"),
            ),
            (
                KeyContext::Global,
                binding(plain(KeyCode::Char('?')), "help", "show this help"),
            ),
            // --- Onboarding ------------------------------------------
            // The first-run connect flow. `Up`/`Down` walk the three
            // connect paths; `Enter` commits the highlighted one (the
            // API-key path opens the key field). `o` / `s` are direct
            // shortcuts to the Ollama / Skip paths.
            (
                KeyContext::Surface(Onboarding),
                binding(plain(KeyCode::Up), "onboarding.prev", "previous option"),
            ),
            (
                KeyContext::Surface(Onboarding),
                binding(plain(KeyCode::Down), "onboarding.next", "next option"),
            ),
            (
                KeyContext::Surface(Onboarding),
                binding(plain(KeyCode::Enter), "onboarding.choose", "choose option"),
            ),
            (
                KeyContext::Surface(Onboarding),
                binding(plain(KeyCode::Char('o')), "onboarding.ollama", "use Ollama"),
            ),
            (
                KeyContext::Surface(Onboarding),
                binding(
                    plain(KeyCode::Char('s')),
                    "onboarding.skip",
                    "skip — read-only mode",
                ),
            ),
            // --- Workspace -------------------------------------------
            // The 3-pane conversation surface. `/` opens the slash line,
            // `Shift+Tab` cycles the approval mode (overriding the
            // global prev-surface meaning while the workspace holds
            // focus), `Up` walks input history.
            (
                KeyContext::Surface(Workspace),
                binding(plain(KeyCode::Char('/')), "command", "slash commands"),
            ),
            (
                KeyContext::Surface(Workspace),
                binding(plain(KeyCode::BackTab), "mode.cycle", "cycle approval mode"),
            ),
            (
                KeyContext::Surface(Workspace),
                binding(plain(KeyCode::Up), "history.prev", "previous input"),
            ),
            (
                KeyContext::Surface(Workspace),
                binding(ctrl(KeyCode::Char('b')), "rail.toggle", "show / hide rail"),
            ),
            // v0.9.0 W3 D1: Ctrl+E toggles compact-vs-full tool-card
            // output for every card on screen at once. Defaults to
            // compact; the chord opens up everything in one press.
            (
                KeyContext::Surface(Workspace),
                binding(
                    ctrl(KeyCode::Char('e')),
                    "toolcard.toggle_compact",
                    "expand / collapse tool cards",
                ),
            ),
            // F-043: transcript scrollback.
            (
                KeyContext::Surface(Workspace),
                binding(
                    plain(KeyCode::PageUp),
                    "transcript.scroll-up",
                    "scroll transcript up",
                ),
            ),
            (
                KeyContext::Surface(Workspace),
                binding(
                    plain(KeyCode::PageDown),
                    "transcript.scroll-down",
                    "scroll transcript down",
                ),
            ),
            // F-057: Ctrl+D exits on an empty composer.
            (
                KeyContext::Surface(Workspace),
                binding(
                    ctrl(KeyCode::Char('d')),
                    "quit",
                    "exit (empty composer only)",
                ),
            ),
            // v0.9.0 W1 B10 (P-B1 closure): Ctrl+Space toggles voice
            // capture. Dispatches as the `/voice` slash command so it
            // routes through the same registry path as every other
            // tool — no special-case voice plumbing in the TUI.
            (
                KeyContext::Surface(Workspace),
                binding(
                    ctrl(KeyCode::Char(' ')),
                    "voice.toggle",
                    "toggle voice capture",
                ),
            ),
            // v0.9.3 W7.1/W7.2: open the agent list. Two chord paths
            // are advertised in the `?` overlay; the third path (the
            // composed `'å'` for macOS Terminal.app) is consumed at the
            // workspace handler but intentionally NOT in the help table
            // — it would confuse users on every other terminal where
            // `'å'` is just a typed character.
            (
                KeyContext::Surface(Workspace),
                binding(alt(KeyCode::Char('a')), "agents.open", "open agent list"),
            ),
            (
                KeyContext::Surface(Workspace),
                binding(ctrl(KeyCode::Char(']')), "agents.open", "open agent list"),
            ),
            // --- Sub-agents monitor ----------------------------------
            // `Enter` expands a sub-agent's live feed; `Esc` interrupts
            // the spawn (the global cancel meaning, restated for help).
            (
                KeyContext::Surface(SubAgents),
                binding(
                    plain(KeyCode::Enter),
                    "subagent.expand",
                    "expand a sub-agent's feed",
                ),
            ),
            (
                KeyContext::Surface(SubAgents),
                binding(
                    plain(KeyCode::Esc),
                    "spawn.interrupt",
                    "interrupt the spawn",
                ),
            ),
            // --- Command palette overlay -----------------------------
            // Arrow keys move the selection, `Enter` runs, `Tab` toggles
            // the fuzzy filter, `Esc` closes the overlay.
            (
                KeyContext::Surface(Palette),
                binding(plain(KeyCode::Up), "palette.up", "move up"),
            ),
            (
                KeyContext::Surface(Palette),
                binding(plain(KeyCode::Down), "palette.down", "move down"),
            ),
            (
                KeyContext::Surface(Palette),
                binding(plain(KeyCode::Enter), "palette.run", "run command"),
            ),
            (
                KeyContext::Surface(Palette),
                binding(plain(KeyCode::Tab), "palette.fuzzy", "toggle fuzzy filter"),
            ),
            (
                KeyContext::Surface(Palette),
                binding(plain(KeyCode::Esc), "palette.close", "close palette"),
            ),
            // --- Plan-review -----------------------------------------
            // `a` approves and runs the plan, `r` keeps planning, `Esc`
            // discards. Single-key hints are lowercase per ux finding #17.
            (
                KeyContext::Surface(PlanReview),
                binding(plain(KeyCode::Char('a')), "plan.approve", "approve & run"),
            ),
            (
                KeyContext::Surface(PlanReview),
                binding(plain(KeyCode::Char('r')), "plan.keep", "keep planning"),
            ),
            (
                KeyContext::Surface(PlanReview),
                binding(plain(KeyCode::Esc), "plan.discard", "discard plan"),
            ),
            // --- Config / settings -----------------------------------
            // Arrow keys move, `Enter` opens a row, `Space` toggles, `x`
            // reveals expert mode, `Esc` saves & closes.
            (
                KeyContext::Surface(Config),
                binding(plain(KeyCode::Up), "config.up", "move up"),
            ),
            (
                KeyContext::Surface(Config),
                binding(plain(KeyCode::Down), "config.down", "move down"),
            ),
            (
                KeyContext::Surface(Config),
                binding(plain(KeyCode::Enter), "config.open", "open setting"),
            ),
            (
                KeyContext::Surface(Config),
                binding(plain(KeyCode::Char(' ')), "config.toggle", "toggle setting"),
            ),
            (
                KeyContext::Surface(Config),
                binding(plain(KeyCode::Char('x')), "config.expert", "expert tuning"),
            ),
            (
                KeyContext::Surface(Config),
                binding(plain(KeyCode::Esc), "config.save", "save & close"),
            ),
            // --- Plugins ---------------------------------------------
            // The marketplace panel. Arrow keys move the row cursor,
            // `Enter` installs an available plugin or removes an
            // installed one, `i` opens the details card, `Esc` closes.
            (
                KeyContext::Surface(Plugins),
                binding(plain(KeyCode::Up), "plugins.up", "move up"),
            ),
            (
                KeyContext::Surface(Plugins),
                binding(plain(KeyCode::Down), "plugins.down", "move down"),
            ),
            (
                KeyContext::Surface(Plugins),
                binding(plain(KeyCode::Enter), "plugins.toggle", "install / remove"),
            ),
            (
                KeyContext::Surface(Plugins),
                binding(
                    plain(KeyCode::Char('i')),
                    "plugins.details",
                    "plugin details",
                ),
            ),
            (
                KeyContext::Surface(Plugins),
                binding(plain(KeyCode::Esc), "plugins.close", "close plugins"),
            ),
            // --- Diagnostics -----------------------------------------
            // The /doctor · /cost · /memory triptych. `1`/`2`/`3` jump
            // straight to a panel; on the memory panel `d` deletes the
            // selected entry and `r` re-runs the doctor checks.
            (
                KeyContext::Surface(Diagnostics),
                binding(plain(KeyCode::Char('1')), "diag.doctor", "doctor panel"),
            ),
            (
                KeyContext::Surface(Diagnostics),
                binding(plain(KeyCode::Char('2')), "diag.cost", "cost panel"),
            ),
            (
                KeyContext::Surface(Diagnostics),
                binding(plain(KeyCode::Char('3')), "diag.memory", "memory panel"),
            ),
            (
                KeyContext::Surface(Diagnostics),
                binding(plain(KeyCode::Char('r')), "diag.refresh", "re-run checks"),
            ),
            (
                KeyContext::Surface(Diagnostics),
                binding(
                    plain(KeyCode::Char('d')),
                    "diag.delete",
                    "delete memory entry",
                ),
            ),
            // --- v0.9.3 W7.2: Agent list (AgentNav) ------------------
            // Up/Down walk the selection through the grouped agent list;
            // Enter opens the focused agent's transcript; `/` opens the
            // filter line; Space collapses or expands the focused group;
            // Esc pops back to the workspace (restoring the workspace's
            // captured scroll offset via `SurfaceAction::Pop`).
            (
                KeyContext::Surface(AgentNav),
                binding(plain(KeyCode::Up), "agents.select_prev", "previous agent"),
            ),
            (
                KeyContext::Surface(AgentNav),
                binding(plain(KeyCode::Down), "agents.select_next", "next agent"),
            ),
            (
                KeyContext::Surface(AgentNav),
                binding(
                    plain(KeyCode::Enter),
                    "agents.open_transcript",
                    "open agent transcript",
                ),
            ),
            (
                KeyContext::Surface(AgentNav),
                binding(plain(KeyCode::Char('/')), "agents.filter", "filter agents"),
            ),
            (
                KeyContext::Surface(AgentNav),
                binding(
                    plain(KeyCode::Char(' ')),
                    "agents.toggle_group",
                    "collapse / expand group",
                ),
            ),
            (
                KeyContext::Surface(AgentNav),
                binding(plain(KeyCode::Esc), "agents.back", "back to workspace"),
            ),
            // --- v0.9.3 W7.2: Agent transcript (AgentTranscript) -----
            // PgUp / PgDn scroll a page; Up / Down nudge one line; End
            // jumps to the latest output and re-arms sticky-bottom; Esc
            // pops back to the agent list.
            (
                KeyContext::Surface(AgentTranscript),
                binding(
                    plain(KeyCode::Up),
                    "agent_transcript.scroll_up",
                    "scroll up one line",
                ),
            ),
            (
                KeyContext::Surface(AgentTranscript),
                binding(
                    plain(KeyCode::Down),
                    "agent_transcript.scroll_down",
                    "scroll down one line",
                ),
            ),
            (
                KeyContext::Surface(AgentTranscript),
                binding(
                    plain(KeyCode::PageUp),
                    "agent_transcript.page_up",
                    "scroll up one page",
                ),
            ),
            (
                KeyContext::Surface(AgentTranscript),
                binding(
                    plain(KeyCode::PageDown),
                    "agent_transcript.page_down",
                    "scroll down one page",
                ),
            ),
            (
                KeyContext::Surface(AgentTranscript),
                binding(
                    plain(KeyCode::End),
                    "agent_transcript.jump_bottom",
                    "jump to bottom",
                ),
            ),
            (
                KeyContext::Surface(AgentTranscript),
                binding(
                    plain(KeyCode::Esc),
                    "agent_transcript.back",
                    "back to agent list",
                ),
            ),
        ];

        Self { bindings }
    }

    /// Resolve a key press in a given surface context to its action
    /// identifier, if any. Surface bindings take precedence over global.
    pub fn resolve(&self, surface: SurfaceId, key: KeyEvent) -> Option<&str> {
        // Surface-scoped bindings win over global ones for the same key,
        // so check the surface layer first.
        let surface_hit = self.bindings.iter().find_map(|(ctx, b)| {
            (*ctx == KeyContext::Surface(surface) && key_matches(b.key, key))
                .then_some(b.action.as_str())
        });
        surface_hit.or_else(|| {
            self.bindings.iter().find_map(|(ctx, b)| {
                (*ctx == KeyContext::Global && key_matches(b.key, key)).then_some(b.action.as_str())
            })
        })
    }

    /// The help rows for the `?` overlay in a given surface context
    /// (global bindings plus that surface's bindings).
    ///
    /// Surface bindings render first so the context-specific keys lead
    /// the overlay; the global universals follow. A global binding whose
    /// key the surface overrides is dropped — the overlay shows the
    /// binding actually in effect, never a shadowed one.
    pub fn help(&self, surface: SurfaceId) -> Vec<HelpEntry> {
        let surface_keys: Vec<KeyEvent> = self
            .bindings
            .iter()
            .filter(|(ctx, _)| *ctx == KeyContext::Surface(surface))
            .map(|(_, b)| b.key)
            .collect();

        let mut entries: Vec<HelpEntry> = Vec::new();
        for (_, b) in self
            .bindings
            .iter()
            .filter(|(ctx, _)| *ctx == KeyContext::Surface(surface))
        {
            entries.push(help_entry(b));
        }
        for (_, b) in self
            .bindings
            .iter()
            .filter(|(ctx, _)| *ctx == KeyContext::Global)
        {
            // Skip a global binding the surface has overridden — its row
            // would describe a key the surface no longer maps that way.
            if surface_keys.iter().any(|k| key_matches(*k, b.key)) {
                continue;
            }
            entries.push(help_entry(b));
        }
        entries
    }
}

/// The `?` help overlay — a modal list of the keybindings in effect on the
/// surface it was opened over (D038).
///
/// The Router opens this on a global `?` press (when the active surface did
/// not consume `?` for its own text input) and tears it down on the next
/// key. It holds the pre-resolved [`HelpEntry`] rows for one surface so the
/// render is a pure read — the overlay never re-queries the [`Keymap`].
pub struct HelpOverlaySurface {
    /// The surface whose bindings this overlay documents (its title heads
    /// the modal).
    target: SurfaceId,
    /// The rows to render — `Keymap::help(target)`, resolved once at open.
    rows: Vec<HelpEntry>,
}

impl HelpOverlaySurface {
    /// Build the help overlay for `target`, resolving its help rows from the
    /// default keymap up front.
    pub fn for_surface(target: SurfaceId) -> Self {
        Self {
            target,
            rows: Keymap::default_map().help(target),
        }
    }
}

impl Surface for HelpOverlaySurface {
    fn id(&self) -> SurfaceId {
        // The overlay reports the surface it documents — it is chrome over
        // that surface, not a peer surface with its own identity.
        self.target
    }

    fn render(&mut self, frame: &mut Frame, area: Rect, _app: &App, theme: &Theme) {
        use ratatui::layout::Margin;
        use ratatui::style::{Modifier, Style};
        use ratatui::text::{Line, Span};
        use ratatui::widgets::{Block, Borders, Clear, Paragraph};

        if area.height < 3 || area.width < 12 {
            return;
        }
        // The widest key label decides the gutter so the descriptions align.
        let key_col = self
            .rows
            .iter()
            .map(|r| r.key_label.chars().count())
            .max()
            .unwrap_or(0)
            .max(3);
        // +1 footer row + 2 border rows; cap at the available height.
        let body_rows = (self.rows.len() as u16).saturating_add(2);
        let popup_h = body_rows.saturating_add(2).min(area.height);
        let popup_w = area.width.min(60);
        let popup = Rect::new(
            area.x + (area.width.saturating_sub(popup_w)) / 2,
            area.y + (area.height.saturating_sub(popup_h)) / 2,
            popup_w,
            popup_h,
        );
        frame.render_widget(Clear, popup);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.border))
            .style(Style::default().bg(theme.surface_elevated))
            .title(Span::styled(
                format!(" {} keys ", self.target.title()),
                Style::default().fg(theme.text),
            ));
        let inner = block.inner(popup);
        frame.render_widget(block, popup);

        let mut lines: Vec<Line> = Vec::new();
        for r in &self.rows {
            let pad = key_col.saturating_sub(r.key_label.chars().count());
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{}{}  ", r.key_label, " ".repeat(pad)),
                    Style::default()
                        .fg(theme.text)
                        .add_modifier(Modifier::BOLD)
                        .bg(theme.surface_elevated),
                ),
                Span::styled(
                    r.description.clone(),
                    Style::default()
                        .fg(theme.text_muted)
                        .bg(theme.surface_elevated),
                ),
            ]));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Esc or ? to close",
            Style::default()
                .fg(theme.text_dim)
                .bg(theme.surface_elevated),
        )));
        frame.render_widget(
            Paragraph::new(lines).style(Style::default().bg(theme.surface_elevated)),
            inner.inner(Margin {
                horizontal: 1,
                vertical: 0,
            }),
        );
    }

    fn handle_key(&mut self, _key: KeyEvent, _app: &mut App) -> SurfaceAction {
        // The overlay is read-only: any key dismisses it. The Esc precedence
        // ladder in the Router already routes a bare Esc here as a close, so
        // every other key falling through to this close is the correct
        // "press anything to dismiss" behavior.
        SurfaceAction::CloseOverlay
    }
}

/// Build a `KeyBinding` from its parts.
fn binding(key: KeyEvent, action: &str, description: &str) -> KeyBinding {
    KeyBinding {
        key,
        action: action.to_string(),
        description: description.to_string(),
    }
}

/// A `KeyEvent` with no modifiers.
fn plain(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// A `KeyEvent` with the `Control` modifier.
fn ctrl(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::CONTROL)
}

/// A `KeyEvent` with the `Alt` modifier. v0.9.3 W7.2 — used for the
/// `Alt+A` "open agent list" chord; iTerm2 / Kitty / Wezterm send this
/// for `Option+A` natively, while macOS Terminal.app sends the composed
/// `'å'` instead (handled at the call site in `workspace.rs`).
fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}

/// True if two key events are the same binding.
///
/// Compares code and modifiers but ignores `kind`/`state`, which
/// crossterm fills with terminal-dependent values (key-repeat, lock
/// state) that are irrelevant to "which binding fired".
fn key_matches(declared: KeyEvent, pressed: KeyEvent) -> bool {
    declared.code == pressed.code && declared.modifiers == pressed.modifiers
}

/// Render a `KeyBinding` into a `HelpEntry` with a printable key label.
fn help_entry(b: &KeyBinding) -> HelpEntry {
    HelpEntry {
        key_label: key_label(b.key),
        description: b.description.clone(),
    }
}

/// A human-readable label for a key event (e.g. `Tab`, `Ctrl+C`, `?`).
fn key_label(key: KeyEvent) -> String {
    let mut label = String::new();
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        label.push_str("Ctrl+");
    }
    if key.modifiers.contains(KeyModifiers::ALT) {
        label.push_str("Alt+");
    }
    if key.modifiers.contains(KeyModifiers::SHIFT) {
        label.push_str("Shift+");
    }
    let code = match key.code {
        KeyCode::Char(' ') => "Space".to_string(),
        KeyCode::Char(c) => {
            // Modified letter chords read better uppercase (Ctrl+C).
            if key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
            {
                c.to_ascii_uppercase().to_string()
            } else {
                c.to_string()
            }
        }
        KeyCode::Esc => "Esc".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::BackTab => "Shift+Tab".to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Backspace => "Backspace".to_string(),
        KeyCode::Delete => "Delete".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::F(n) => format!("F{n}"),
        other => format!("{other:?}"),
    };
    label.push_str(&code);
    label
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_universals_resolve_on_every_surface() {
        let map = Keymap::default_map();
        for surface in SurfaceId::TABS {
            assert_eq!(map.resolve(surface, ctrl(KeyCode::Char('c'))), Some("quit"));
            assert_eq!(
                map.resolve(surface, plain(KeyCode::Char('?'))),
                Some("help")
            );
            assert_eq!(
                map.resolve(surface, plain(KeyCode::Tab)),
                Some("next.surface")
            );
        }
        // The palette overlay is not a tab but still gets the universals
        // it does not override (Ctrl+C is never overridden).
        assert_eq!(
            map.resolve(SurfaceId::Palette, ctrl(KeyCode::Char('c'))),
            Some("quit")
        );
    }

    #[test]
    fn surface_binding_overrides_global_for_the_same_key() {
        let map = Keymap::default_map();
        // Globally `Shift+Tab` is "previous surface"; the Workspace
        // surface rebinds it to "cycle approval mode".
        assert_eq!(
            map.resolve(SurfaceId::Diagnostics, plain(KeyCode::BackTab)),
            Some("prev.surface")
        );
        assert_eq!(
            map.resolve(SurfaceId::Workspace, plain(KeyCode::BackTab)),
            Some("mode.cycle")
        );
        // Globally `Tab` is "next surface"; the Palette rebinds it to the
        // fuzzy-filter toggle.
        assert_eq!(
            map.resolve(SurfaceId::Palette, plain(KeyCode::Tab)),
            Some("palette.fuzzy")
        );
        assert_eq!(
            map.resolve(SurfaceId::Config, plain(KeyCode::Tab)),
            Some("next.surface")
        );
    }

    #[test]
    fn esc_resolves_to_the_surface_specific_action_when_one_exists() {
        let map = Keymap::default_map();
        // Esc is a global universal ("cancel"), but surfaces rebind it
        // to their own framed action.
        assert_eq!(
            map.resolve(SurfaceId::Onboarding, plain(KeyCode::Esc)),
            Some("cancel")
        );
        assert_eq!(
            map.resolve(SurfaceId::Palette, plain(KeyCode::Esc)),
            Some("palette.close")
        );
        assert_eq!(
            map.resolve(SurfaceId::PlanReview, plain(KeyCode::Esc)),
            Some("plan.discard")
        );
    }

    #[test]
    fn unbound_key_resolves_to_none() {
        let map = Keymap::default_map();
        assert_eq!(
            map.resolve(SurfaceId::Onboarding, plain(KeyCode::Char('z'))),
            None
        );
    }

    #[test]
    fn help_lists_surface_bindings_before_globals() {
        let map = Keymap::default_map();
        let rows = map.help(SurfaceId::Palette);
        // The first rows are the palette's own bindings.
        assert_eq!(
            rows.first().map(|r| r.description.as_str()),
            Some("move up")
        );
        // The universals still appear (Ctrl+C is never overridden).
        assert!(rows.iter().any(|r| r.key_label == "Ctrl+C"));
        // The help overlay key (`?`) is listed.
        assert!(rows.iter().any(|r| r.key_label == "?"));
    }

    #[test]
    fn help_drops_a_global_binding_the_surface_overrides() {
        let map = Keymap::default_map();
        let rows = map.help(SurfaceId::Workspace);
        // Workspace overrides Shift+Tab; the overlay must show the
        // surface meaning, not the shadowed global one.
        let shift_tab_rows: Vec<&str> = rows
            .iter()
            .filter(|r| r.key_label == "Shift+Tab")
            .map(|r| r.description.as_str())
            .collect();
        assert_eq!(shift_tab_rows, vec!["cycle approval mode"]);
    }

    #[test]
    fn every_tab_surface_has_its_own_bindings_in_help() {
        // Each of the seven tab surfaces must contribute at least one
        // surface-specific help row — a footer hint / `?` overlay that
        // shows nothing but the globals is a discoverability gap.
        let map = Keymap::default_map();
        for surface in SurfaceId::TABS {
            let rows = map.help(surface);
            let surface_rows = map
                .bindings
                .iter()
                .filter(|(ctx, _)| *ctx == KeyContext::Surface(surface))
                .count();
            assert!(
                surface_rows > 0,
                "{surface:?} has no surface-specific bindings"
            );
            assert!(!rows.is_empty(), "{surface:?} help overlay is empty");
        }
    }

    #[test]
    fn onboarding_and_plugins_resolve_their_surface_keys() {
        let map = Keymap::default_map();
        // Onboarding: the `o` / `s` direct path shortcuts.
        assert_eq!(
            map.resolve(SurfaceId::Onboarding, plain(KeyCode::Char('o'))),
            Some("onboarding.ollama")
        );
        assert_eq!(
            map.resolve(SurfaceId::Onboarding, plain(KeyCode::Char('s'))),
            Some("onboarding.skip")
        );
        // Plugins: the install/remove verb and the details key.
        assert_eq!(
            map.resolve(SurfaceId::Plugins, plain(KeyCode::Enter)),
            Some("plugins.toggle")
        );
        assert_eq!(
            map.resolve(SurfaceId::Plugins, plain(KeyCode::Char('i'))),
            Some("plugins.details")
        );
    }

    #[test]
    fn workspace_resolves_ctrl_b_to_the_rail_toggle() {
        let map = Keymap::default_map();
        assert_eq!(
            map.resolve(SurfaceId::Workspace, ctrl(KeyCode::Char('b'))),
            Some("rail.toggle")
        );
        // The `?` overlay documents the chord.
        let rows = map.help(SurfaceId::Workspace);
        assert!(rows.iter().any(|r| r.key_label == "Ctrl+B"));
    }

    #[test]
    fn key_label_renders_chords_and_specials() {
        assert_eq!(key_label(ctrl(KeyCode::Char('c'))), "Ctrl+C");
        assert_eq!(key_label(plain(KeyCode::Tab)), "Tab");
        assert_eq!(key_label(plain(KeyCode::BackTab)), "Shift+Tab");
        assert_eq!(key_label(plain(KeyCode::Esc)), "Esc");
        assert_eq!(key_label(plain(KeyCode::Char(' '))), "Space");
        assert_eq!(key_label(plain(KeyCode::Char('?'))), "?");
    }

    // ---------------------------------------------------------------
    // v0.9.3 W7.2: `?` help registration for the agent-list keybinds
    // wired in W7.1 (Workspace) + the AgentNav / AgentTranscript
    // surfaces built in W4 / W5. The help overlay is the one surface
    // a user can hit blind on any screen, so every new keybind has to
    // show up there or it is effectively undocumented (RULE-S0).
    // ---------------------------------------------------------------

    #[test]
    fn workspace_help_includes_open_agent_list_v093() {
        let map = Keymap::default_map();
        let rows = map.help(SurfaceId::Workspace);
        let alt_a = rows
            .iter()
            .find(|r| r.key_label == "Alt+A")
            .expect("Workspace ? overlay must list Alt+A");
        assert!(
            alt_a.description.to_lowercase().contains("agent"),
            "Alt+A description must mention agents, got {:?}",
            alt_a.description
        );
        let ctrl_bracket = rows
            .iter()
            .find(|r| r.key_label == "Ctrl+]")
            .expect("Workspace ? overlay must list Ctrl+]");
        assert!(ctrl_bracket.description.to_lowercase().contains("agent"));
    }

    #[test]
    fn agent_nav_help_lists_select_open_filter_collapse_back_v093() {
        let map = Keymap::default_map();
        let rows = map.help(SurfaceId::AgentNav);
        // Selection: Up / Down.
        assert!(
            rows.iter().any(|r| r.key_label == "Up"),
            "AgentNav help must list Up for selection"
        );
        assert!(
            rows.iter().any(|r| r.key_label == "Down"),
            "AgentNav help must list Down for selection"
        );
        // Open the focused agent: Enter.
        assert!(
            rows.iter()
                .any(|r| r.key_label == "Enter" && r.description.to_lowercase().contains("open")),
            "AgentNav help must list Enter as open"
        );
        // Filter: `/`.
        assert!(
            rows.iter()
                .any(|r| r.key_label == "/" && r.description.to_lowercase().contains("filter")),
            "AgentNav help must list / as filter"
        );
        // Collapse group: Space.
        assert!(
            rows.iter().any(
                |r| r.key_label == "Space" && r.description.to_lowercase().contains("collapse")
            ),
            "AgentNav help must list Space as collapse-group"
        );
        // Esc back to workspace.
        let esc_rows: Vec<&str> = rows
            .iter()
            .filter(|r| r.key_label == "Esc")
            .map(|r| r.description.as_str())
            .collect();
        assert!(
            esc_rows.iter().any(
                |d| d.to_lowercase().contains("workspace") || d.to_lowercase().contains("back")
            ),
            "AgentNav Esc description must mention 'back' or 'workspace', got {esc_rows:?}"
        );
    }

    #[test]
    fn agent_transcript_help_lists_scroll_back_jump_v093() {
        let map = Keymap::default_map();
        let rows = map.help(SurfaceId::AgentTranscript);
        // Scrollback: Up / Down / PageUp / PageDown.
        for label in ["Up", "Down", "PageUp", "PageDown"] {
            assert!(
                rows.iter().any(|r| r.key_label == label),
                "AgentTranscript help must list {label} for scroll"
            );
        }
        // End jumps to bottom.
        assert!(
            rows.iter().any(|r| r.key_label == "End"
                && (r.description.to_lowercase().contains("bottom")
                    || r.description.to_lowercase().contains("latest"))),
            "AgentTranscript help must list End as jump-to-bottom"
        );
        // Esc back.
        let esc_rows: Vec<&str> = rows
            .iter()
            .filter(|r| r.key_label == "Esc")
            .map(|r| r.description.as_str())
            .collect();
        assert!(
            esc_rows
                .iter()
                .any(|d| d.to_lowercase().contains("back") || d.to_lowercase().contains("list")),
            "AgentTranscript Esc description must mention 'back' or 'list', got {esc_rows:?}"
        );
    }
}
