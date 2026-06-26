//! Slash-command system: the command registry, dispatch, and the
//! `@`-reference module.
//!
//! T1.8 — the [`CommandRegistry`] is the single source of truth for the
//! TUI's slash commands. It feeds three consumers: the command palette
//! (T1.4), the `/help` grouped listing, and the slash dispatcher's
//! "did you mean" suggestion. T1.9 fills the `at_refs` module below; this
//! file must keep the `mod at_refs;` declaration so the two agents build
//! in parallel without colliding.
//!
//! ## Model
//!
//! A [`Command`] carries a name, an [`IntentGroup`], a one-line
//! consequence-framed description, and a `destructive` flag. The ~14
//! grounded commands come straight from `ux-krug-sutherland.md` §3a,
//! sorted into the six intent groups. The registry is *mutable* — user-
//! invocable skills extend it at runtime via [`CommandRegistry::register`]
//! (wiring skills in is a later wave; the API is here so it can).
//!
//! The slash [`dispatch`](CommandRegistry::dispatch) parses a slash line
//! and returns a [`Dispatch`]: an exact match, a "did you mean" with the
//! closest command (Damerau-Levenshtein distance ≤ 2), `/help` rendered
//! as a grouped listing, or "unknown" when nothing is close enough.

// `pub` so the workspace composer's `@`-reference autocomplete (Wave 2)
// can call `at_refs::complete` and render `at_refs::Completion`.
//
// `at_refs` is a thin facade over four focused submodules — W3-B split the
// engine apart once the single file passed the AGENTS.md 1000-line
// guideline. The submodules are crate-private; everything the composer
// needs is re-exported through `at_refs`, so `at_refs::*` paths are
// unchanged for every existing caller.
mod at_ref_complete;
mod at_ref_guard;
mod at_ref_parse;
mod at_ref_resolve;
mod at_ref_send;
pub mod at_refs;

use crate::tui::theme::ThemeMode;

/// Parse the argument of a `/theme <light|dark|auto>` line into a
/// [`ThemeMode`] (v0.9.2 W8 / §5).
///
/// `line` is the raw composer text (e.g. `"/theme light"` or just
/// `"/theme"`). The first whitespace-delimited token after the command word
/// selects the mode, case-insensitively:
/// - `light` → [`ThemeMode::Light`]
/// - `dark` → [`ThemeMode::Dark`]
/// - `auto` → [`ThemeMode::Auto`]
///
/// A missing or unrecognized argument falls back to [`ThemeMode::Auto`] —
/// "do the right thing for this terminal" is the most useful default when
/// the user types a bare `/theme`. The router calls this, then
/// `Theme::for_mode(...)`, to re-resolve the live theme in place.
pub fn parse_theme_mode(line: &str) -> ThemeMode {
    // Skip the `/theme` word; the next token is the mode.
    let arg = line.split_whitespace().nth(1).unwrap_or("");
    match arg.to_ascii_lowercase().as_str() {
        "light" => ThemeMode::Light,
        "dark" => ThemeMode::Dark,
        _ => ThemeMode::Auto,
    }
}

/// One of the six intent groups a slash command belongs to.
///
/// Grouping by intent (rather than an alphabetical wall of 14 rows) is
/// the Krug "billboard test per group" move from `ux-krug-sutherland.md`
/// §3a — the palette and `/help` both render commands grouped by this.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IntentGroup {
    /// Session lifecycle — resume, rewind, new, compact, quit.
    Session,
    /// Model and provider switching.
    ModelProvider,
    /// Context and long-term memory.
    ContextMemory,
    /// Tools and extensions — MCP, skills, plugins, hooks.
    ToolsExtensions,
    /// Workflow — plan mode, approval mode, config.
    Workflow,
    /// Diagnostics — doctor, replay, help.
    Diagnostics,
}

impl IntentGroup {
    /// Display order of the six groups — the order the palette and
    /// `/help` render them in. Matches `ux-krug-sutherland.md` §3a.
    pub const ORDER: [IntentGroup; 6] = [
        IntentGroup::Session,
        IntentGroup::ModelProvider,
        IntentGroup::ContextMemory,
        IntentGroup::ToolsExtensions,
        IntentGroup::Workflow,
        IntentGroup::Diagnostics,
    ];

    /// The group's section heading, as shown in the palette and `/help`.
    pub fn title(self) -> &'static str {
        match self {
            IntentGroup::Session => "SESSION",
            IntentGroup::ModelProvider => "MODEL & PROVIDER",
            IntentGroup::ContextMemory => "CONTEXT & MEMORY",
            IntentGroup::ToolsExtensions => "TOOLS & EXTENSIONS",
            IntentGroup::Workflow => "WORKFLOW",
            IntentGroup::Diagnostics => "DIAGNOSTICS",
        }
    }
}

/// A single slash command — one row in the palette / `/help`.
///
/// `name` includes the leading slash (e.g. `/rewind`). `description` is
/// the consequence-framed one-liner from `ux-krug-sutherland.md` §3a:
/// present tense, short, says what *happens*. `destructive` is true for
/// commands that discard the user's current work — the palette tags them
/// so a file-discarding command never looks like a read-only one
/// (finding #15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    /// The command name, including the leading slash (e.g. `/rewind`).
    pub name: String,
    /// The intent group this command is filed under.
    pub group: IntentGroup,
    /// A consequence-framed one-line description of what running it does.
    pub description: String,
    /// True when running the command discards the user's current work
    /// (e.g. `/rewind` restores files to an earlier snapshot).
    pub destructive: bool,
}

impl Command {
    /// Build a command. Internal helper for the built-in table; the
    /// public extension path is [`CommandRegistry::register`].
    fn new(name: &str, group: IntentGroup, description: &str, destructive: bool) -> Self {
        Self {
            name: name.to_string(),
            group,
            description: description.to_string(),
            destructive,
        }
    }

    /// D023: public constructor for a runtime-registered, non-destructive
    /// command — the path the router uses to register user-invocable skills
    /// as dispatchable `/name` verbs via [`CommandRegistry::register`]. The
    /// built-in table uses the crate-private [`new`](Self::new); this is the
    /// only `pub` builder, and it always produces a non-destructive command
    /// (a skill never discards the user's work the way `/rewind` does).
    pub fn new_skill(name: &str, group: IntentGroup, description: &str) -> Self {
        Self::new(name, group, description, false)
    }
}

/// The single source of truth for the TUI's slash commands.
///
/// Built with [`CommandRegistry::with_builtins`] (the ~14 grounded
/// commands). Runtime extensions — user-invocable skills — are added with
/// [`register`](CommandRegistry::register); the registry stays the one
/// place every consumer (palette, `/help`, dispatch) reads from.
#[derive(Debug, Default)]
pub struct CommandRegistry {
    /// Registered commands, in registration order. Built-ins come first
    /// (in `ux-krug-sutherland.md` §3a order); extensions append.
    commands: Vec<Command>,
}

impl CommandRegistry {
    /// An empty registry. Mostly useful for tests; production code uses
    /// [`with_builtins`](Self::with_builtins).
    pub fn new() -> Self {
        Self::default()
    }

    /// A registry pre-populated with the ~14 grounded built-in commands
    /// from `ux-krug-sutherland.md` §3a, in document order.
    ///
    /// Only `/rewind` is `destructive` — it restores files to an earlier
    /// snapshot, discarding current work. Every other built-in is either
    /// read-only or non-discarding (`/new` and `/compact` keep your work;
    /// `/quit` ends the session but writes nothing away).
    pub fn with_builtins() -> Self {
        use IntentGroup::*;
        let commands = vec![
            // SESSION
            Command::new("/resume", Session, "reopen a past session", false),
            Command::new(
                "/rewind",
                Session,
                "restore files to an earlier snapshot",
                true,
            ),
            Command::new("/new", Session, "start fresh, keep this provider", false),
            Command::new(
                "/compact",
                Session,
                "fold context now, keep decisions",
                false,
            ),
            Command::new("/quit", Session, "end this session", false),
            Command::new("/exit", Session, "end this session", false),
            // MODEL & PROVIDER
            Command::new("/model", ModelProvider, "switch model", false),
            Command::new("/provider", ModelProvider, "switch provider", false),
            Command::new(
                "/profile",
                ModelProvider,
                "load a saved profile (fast, review)",
                false,
            ),
            // CONTEXT & MEMORY
            Command::new(
                "/repomap",
                ContextMemory,
                "ask about this codebase (build / refresh the symbol index)",
                false,
            ),
            Command::new(
                "/memory",
                ContextMemory,
                "what Wayland remembers about you",
                false,
            ),
            Command::new(
                "/cost",
                ContextMemory,
                "tokens and spend this session",
                false,
            ),
            Command::new(
                "/effective",
                ContextMemory,
                "view the resolved config (redacted)",
                false,
            ),
            // TOOLS & EXTENSIONS
            Command::new(
                "/tools",
                ToolsExtensions,
                "view tools and their permissions",
                false,
            ),
            Command::new(
                "/mcp",
                ToolsExtensions,
                "list MCP servers, add one live",
                false,
            ),
            Command::new(
                "/auth",
                ToolsExtensions,
                "connect a provider via OAuth (google-meet)",
                false,
            ),
            Command::new("/skills", ToolsExtensions, "browse and run skills", false),
            // Crucible Stage 4a — convene a cross-vendor council to cross-check
            // an answer. Non-destructive (it only spends, gated by an approval
            // card showing the certified ceiling before any charge).
            Command::new(
                "/crucible",
                ToolsExtensions,
                "convene a cross-vendor council to cross-check an answer",
                false,
            ),
            Command::new(
                "/plugins",
                ToolsExtensions,
                "install or remove plugins",
                false,
            ),
            Command::new("/hooks", ToolsExtensions, "view configured hooks", false),
            // v0.9.1 W1 E (debt sweep): `/voice` toggles the voice-mode
            // recorder via the registry path the Ctrl+Space binding
            // already dispatches through. The router invokes
            // `TuiEngine::toggle_voice` directly — no LLM round-trip —
            // and a Null/missing backend surfaces the standard "voice
            // unavailable" notice via `/doctor`.
            Command::new(
                "/voice",
                ToolsExtensions,
                "toggle voice capture (Ctrl+Space)",
                false,
            ),
            // WORKFLOW
            Command::new("/plan", Workflow, "plan before acting (read-only)", false),
            Command::new("/mode", Workflow, "Default, Auto-edit, Force", false),
            // v0.9.2 W8 (§5 / Q1): switch the live color theme without a
            // restart. The router re-resolves the `Theme` via
            // `Theme::for_mode(parse_theme_mode(arg))` and swaps it in place;
            // every surface and the status bar read the new palette next
            // frame. Non-destructive — it only changes colors.
            Command::new(
                "/theme",
                Workflow,
                "switch light / dark / auto theme",
                false,
            ),
            Command::new("/config", Workflow, "all settings", false),
            Command::new(
                "/connect",
                Workflow,
                "paste an API key to connect a provider",
                false,
            ),
            Command::new("/setup", Workflow, "re-run the onboarding flow", false),
            // DIAGNOSTICS
            Command::new(
                "/doctor",
                Diagnostics,
                "check provider, keys, MCP health",
                false,
            ),
            Command::new(
                "/replay",
                Diagnostics,
                "how to re-run a recorded trace",
                false,
            ),
            Command::new(
                "/help",
                Diagnostics,
                "list all slash commands (press ? for this screen's keys)",
                false,
            ),
        ];
        Self { commands }
    }

    /// Register a runtime command (e.g. a user-invocable skill).
    ///
    /// If a command with the same `name` already exists it is replaced —
    /// so re-registering a skill after a reload updates rather than
    /// duplicates it. The leading slash is normalized on if absent.
    pub fn register(&mut self, mut command: Command) {
        if !command.name.starts_with('/') {
            command.name = format!("/{}", command.name);
        }
        match self.commands.iter_mut().find(|c| c.name == command.name) {
            Some(existing) => *existing = command,
            None => self.commands.push(command),
        }
    }

    /// All registered commands, in registration order.
    pub fn all(&self) -> &[Command] {
        &self.commands
    }

    /// The number of registered commands.
    pub fn len(&self) -> usize {
        self.commands.len()
    }

    /// True when no commands are registered.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Look up a command by its exact name (leading slash required).
    pub fn get(&self, name: &str) -> Option<&Command> {
        self.commands.iter().find(|c| c.name == name)
    }

    /// Commands in `group`, in registration order. Used by the palette
    /// and `/help` to render one section per intent group.
    pub fn in_group(&self, group: IntentGroup) -> Vec<&Command> {
        self.commands.iter().filter(|c| c.group == group).collect()
    }

    /// Parse and dispatch a slash line.
    ///
    /// `line` is the raw composer text — leading/trailing whitespace and
    /// any arguments after the command word are tolerated. The first
    /// whitespace-delimited token is taken as the command name.
    ///
    /// Returns:
    /// - [`Dispatch::Help`] for `/help` — the grouped command listing.
    /// - [`Dispatch::Run`] for any other exact match.
    /// - [`Dispatch::DidYouMean`] when the token is within Damerau-
    ///   Levenshtein distance 2 of exactly one known command.
    /// - [`Dispatch::Unknown`] when the line is not a slash command or no
    ///   command is close enough.
    pub fn dispatch(&self, line: &str) -> Dispatch {
        let trimmed = line.trim();
        if !trimmed.starts_with('/') {
            return Dispatch::Unknown {
                input: trimmed.to_string(),
            };
        }
        // The command word is everything up to the first whitespace —
        // arguments after it are ignored for routing.
        let token = trimmed.split_whitespace().next().unwrap_or(trimmed);

        if let Some(command) = self.get(token) {
            if command.name == "/help" {
                return Dispatch::Help;
            }
            return Dispatch::Run {
                name: command.name.clone(),
            };
        }

        // No exact hit — find the single closest command within an edit
        // distance of 2. Ties (more than one equally-close command)
        // resolve to Unknown: a "did you mean" must be unambiguous.
        let mut best: Option<(&Command, usize)> = None;
        let mut tied = false;
        for command in &self.commands {
            let dist = damerau_levenshtein(token, &command.name);
            if dist > MAX_SUGGEST_DISTANCE {
                continue;
            }
            match best {
                None => best = Some((command, dist)),
                Some((_, best_dist)) => {
                    if dist < best_dist {
                        best = Some((command, dist));
                        tied = false;
                    } else if dist == best_dist {
                        tied = true;
                    }
                }
            }
        }

        match best {
            Some((command, _)) if !tied => Dispatch::DidYouMean {
                input: token.to_string(),
                suggestion: command.name.clone(),
            },
            _ => Dispatch::Unknown {
                input: token.to_string(),
            },
        }
    }

    /// Render `/help` as a grouped, plain-text listing — the six intent
    /// groups in [`IntentGroup::ORDER`], each command as
    /// `  /name — description` under its `GROUP` heading.
    pub fn help_text(&self) -> String {
        let mut out = String::new();
        for (i, group) in IntentGroup::ORDER.iter().enumerate() {
            let commands = self.in_group(*group);
            if commands.is_empty() {
                continue;
            }
            if i > 0 {
                out.push('\n');
            }
            out.push_str(group.title());
            out.push('\n');
            for command in commands {
                out.push_str("  ");
                out.push_str(&command.name);
                out.push_str(" — ");
                out.push_str(&command.description);
                out.push('\n');
            }
        }
        out
    }
}

/// The largest Damerau-Levenshtein distance a "did you mean" suggestion
/// is allowed to span. Beyond this the input is too far from any command
/// to guess.
const MAX_SUGGEST_DISTANCE: usize = 2;

/// The outcome of dispatching a slash line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dispatch {
    /// An exact command match — run the command named `name`.
    Run {
        /// The matched command's name, with leading slash.
        name: String,
    },
    /// The `/help` command — render the grouped command listing.
    Help,
    /// No exact match, but `suggestion` is within edit distance 2 of
    /// `input`. The caller surfaces this as "did you mean `<suggestion>`?".
    DidYouMean {
        /// The unrecognized token the user typed.
        input: String,
        /// The closest known command name.
        suggestion: String,
    },
    /// The line is not a slash command, or nothing was close enough.
    Unknown {
        /// The unrecognized input (trimmed; the command token if it had
        /// a leading slash).
        input: String,
    },
}

/// Damerau-Levenshtein edit distance between `a` and `b`.
///
/// Counts single-character insertions, deletions, substitutions, and
/// transpositions of two adjacent characters. The transposition case is
/// what separates this from plain Levenshtein — it makes a typo like
/// `/raplay` → `/replay` a distance of 1, matching how people actually
/// mistype. Operates on Unicode scalar values, not bytes.
fn damerau_levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let (n, m) = (a.len(), b.len());
    if n == 0 {
        return m;
    }
    if m == 0 {
        return n;
    }

    // `d[i][j]` is the edit distance between a[..i] and b[..j]. The full
    // matrix is kept (rather than two rows) because the transposition
    // rule needs row `i-2`.
    let mut d = vec![vec![0usize; m + 1]; n + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }

    for i in 1..=n {
        for j in 1..=m {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            let mut best = (d[i - 1][j] + 1) // deletion
                .min(d[i][j - 1] + 1) // insertion
                .min(d[i - 1][j - 1] + cost); // substitution
            // Transposition of two adjacent characters.
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                best = best.min(d[i - 2][j - 2] + 1);
            }
            d[i][j] = best;
        }
    }
    d[n][m]
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── damerau_levenshtein ────────────────────────────────────────────

    #[test]
    fn edit_distance_zero_for_identical_strings() {
        assert_eq!(damerau_levenshtein("/help", "/help"), 0);
    }

    #[test]
    fn edit_distance_counts_substitution() {
        // one substituted character
        assert_eq!(damerau_levenshtein("/halp", "/help"), 1);
    }

    #[test]
    fn edit_distance_counts_a_transposition_as_one() {
        // `/raplay` -> `/replay` is a single adjacent swap, not two
        // substitutions — this is the Damerau extension.
        assert_eq!(damerau_levenshtein("/rpelay", "/replay"), 1);
    }

    #[test]
    fn edit_distance_handles_empty_inputs() {
        assert_eq!(damerau_levenshtein("", "/new"), 4);
        assert_eq!(damerau_levenshtein("/new", ""), 4);
        assert_eq!(damerau_levenshtein("", ""), 0);
    }

    // ── registry construction ──────────────────────────────────────────

    #[test]
    fn builtins_cover_all_grounded_commands() {
        let reg = CommandRegistry::with_builtins();
        // ux-krug-sutherland.md §3a lists 22 commands across 6 groups;
        // `/exit` is the 23rd — an alias of `/quit` for muscle memory.
        // `/setup` is the 24th — re-enters the first-run onboarding flow.
        // `/auth` is the 25th — OAuth connect (v0.9.0 W4 E1, google-meet).
        // `/voice` is the 26th — toggles voice capture (v0.9.1 W1 E,
        // mirrors the Ctrl+Space chord through the same dispatcher).
        // `/theme` is the 27th — light/dark/auto switch (v0.9.2 W8, §5).
        // `/connect` is the 28th — S4b paste-to-detect provider connect.
        // `/effective` is the 29th — S9 redacted effective-config preview.
        // `/crucible` is the 30th — Crucible Stage 4a cross-vendor council.
        assert_eq!(reg.len(), 30);
        for name in [
            "/resume",
            "/rewind",
            "/new",
            "/compact",
            "/quit",
            "/exit",
            "/model",
            "/provider",
            "/profile",
            "/repomap",
            "/memory",
            "/cost",
            "/effective",
            "/tools",
            "/mcp",
            "/auth",
            "/skills",
            "/crucible",
            "/plugins",
            "/hooks",
            "/voice",
            "/plan",
            "/mode",
            "/theme",
            "/config",
            "/connect",
            "/setup",
            "/doctor",
            "/replay",
            "/help",
        ] {
            assert!(reg.get(name).is_some(), "missing built-in {name}");
        }
    }

    #[test]
    fn rewind_is_the_only_destructive_builtin() {
        let reg = CommandRegistry::with_builtins();
        let destructive: Vec<&str> = reg
            .all()
            .iter()
            .filter(|c| c.destructive)
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(destructive, vec!["/rewind"]);
    }

    #[test]
    fn every_builtin_falls_in_one_of_the_six_groups() {
        let reg = CommandRegistry::with_builtins();
        let grouped: usize = IntentGroup::ORDER
            .iter()
            .map(|g| reg.in_group(*g).len())
            .sum();
        assert_eq!(grouped, reg.len(), "a command escaped the six groups");
    }

    #[test]
    fn descriptions_never_show_the_word_frecency() {
        // ux finding #14 — "frecency" is invented jargon, never user-facing.
        let reg = CommandRegistry::with_builtins();
        for c in reg.all() {
            assert!(
                !c.description.to_lowercase().contains("frecency"),
                "{} leaks 'frecency'",
                c.name
            );
        }
    }

    // ── register (runtime extension) ───────────────────────────────────

    #[test]
    fn register_appends_a_new_command() {
        let mut reg = CommandRegistry::new();
        reg.register(Command::new(
            "/lint",
            IntentGroup::ToolsExtensions,
            "run the linter skill",
            false,
        ));
        assert_eq!(reg.len(), 1);
        assert!(reg.get("/lint").is_some());
    }

    #[test]
    fn register_normalizes_a_missing_leading_slash() {
        let mut reg = CommandRegistry::new();
        reg.register(Command::new(
            "lint",
            IntentGroup::ToolsExtensions,
            "run the linter skill",
            false,
        ));
        assert!(reg.get("/lint").is_some(), "slash was not normalized on");
    }

    #[test]
    fn register_replaces_a_command_with_a_duplicate_name() {
        let mut reg = CommandRegistry::new();
        reg.register(Command::new(
            "/lint",
            IntentGroup::ToolsExtensions,
            "first",
            false,
        ));
        reg.register(Command::new(
            "/lint",
            IntentGroup::ToolsExtensions,
            "second",
            false,
        ));
        assert_eq!(reg.len(), 1, "duplicate name should replace, not append");
        assert_eq!(reg.get("/lint").unwrap().description, "second");
    }

    // ── dispatch ───────────────────────────────────────────────────────

    #[test]
    fn dispatch_runs_an_exact_command() {
        let reg = CommandRegistry::with_builtins();
        assert_eq!(
            reg.dispatch("/repomap"),
            Dispatch::Run {
                name: "/repomap".into()
            }
        );
    }

    #[test]
    fn dispatch_tolerates_arguments_and_surrounding_whitespace() {
        let reg = CommandRegistry::with_builtins();
        // Arguments after the command word are ignored for routing.
        assert_eq!(
            reg.dispatch("  /profile fast  "),
            Dispatch::Run {
                name: "/profile".into()
            }
        );
    }

    #[test]
    fn dispatch_returns_help_for_the_help_command() {
        let reg = CommandRegistry::with_builtins();
        assert_eq!(reg.dispatch("/help"), Dispatch::Help);
    }

    #[test]
    fn dispatch_suggests_the_closest_command_for_a_typo() {
        let reg = CommandRegistry::with_builtins();
        // `/repmap` is one deletion from `/repomap` and far from anything
        // else — an unambiguous "did you mean".
        assert_eq!(
            reg.dispatch("/repmap"),
            Dispatch::DidYouMean {
                input: "/repmap".into(),
                suggestion: "/repomap".into(),
            }
        );
    }

    #[test]
    fn dispatch_suggests_across_a_transposition() {
        let reg = CommandRegistry::with_builtins();
        // `/raplay` -> `/replay`: one adjacent swap + one substitution is
        // distance 2 — the Damerau metric keeps it suggestable.
        assert_eq!(
            reg.dispatch("/rpelay"),
            Dispatch::DidYouMean {
                input: "/rpelay".into(),
                suggestion: "/replay".into(),
            }
        );
    }

    #[test]
    fn dispatch_returns_unknown_for_a_non_slash_line() {
        let reg = CommandRegistry::with_builtins();
        assert_eq!(
            reg.dispatch("just a message"),
            Dispatch::Unknown {
                input: "just a message".into()
            }
        );
    }

    #[test]
    fn dispatch_returns_unknown_when_nothing_is_close_enough() {
        let reg = CommandRegistry::with_builtins();
        // `/xyzzy` is more than 2 edits from every command.
        assert_eq!(
            reg.dispatch("/xyzzy"),
            Dispatch::Unknown {
                input: "/xyzzy".into()
            }
        );
    }

    #[test]
    fn dispatch_returns_unknown_when_the_suggestion_is_ambiguous() {
        // Two commands tied at the same distance from the input must not
        // produce a guess — a "did you mean" has to be unambiguous.
        let mut reg = CommandRegistry::new();
        reg.register(Command::new("/cat", IntentGroup::Session, "a", false));
        reg.register(Command::new("/bat", IntentGroup::Session, "b", false));
        // `/aat` is distance 1 from both `/cat` and `/bat`.
        assert_eq!(
            reg.dispatch("/aat"),
            Dispatch::Unknown {
                input: "/aat".into()
            }
        );
    }

    // ── help_text ──────────────────────────────────────────────────────

    #[test]
    fn help_text_renders_every_group_heading_in_order() {
        let reg = CommandRegistry::with_builtins();
        let help = reg.help_text();
        let mut last = 0;
        for group in IntentGroup::ORDER {
            let at = help.find(group.title()).unwrap_or_else(|| {
                panic!("/help missing group heading {}", group.title());
            });
            assert!(at >= last, "group {} is out of order", group.title());
            last = at;
        }
    }

    #[test]
    fn help_text_lists_every_command_under_a_group() {
        let reg = CommandRegistry::with_builtins();
        let help = reg.help_text();
        for c in reg.all() {
            assert!(help.contains(&c.name), "/help omits {}", c.name);
            assert!(
                help.contains(&c.description),
                "/help omits the description of {}",
                c.name
            );
        }
    }

    #[test]
    fn help_text_omits_empty_groups() {
        // A registry with only Session commands renders one heading.
        let mut reg = CommandRegistry::new();
        reg.register(Command::new("/new", IntentGroup::Session, "fresh", false));
        let help = reg.help_text();
        assert!(help.contains("SESSION"));
        assert!(!help.contains("WORKFLOW"));
    }

    // ── /theme (v0.9.2 W8 / §5) ───────────────────────────────────────────

    #[test]
    fn theme_command_is_registered_and_dispatches() {
        let reg = CommandRegistry::with_builtins();
        // Catalog entry exists, filed under WORKFLOW, non-destructive.
        let cmd = reg.get("/theme").expect("/theme must be a built-in");
        assert_eq!(cmd.group, IntentGroup::Workflow);
        assert!(!cmd.destructive, "/theme only changes colors");
        // It dispatches as a runnable command (the router handles the
        // live theme swap; here we just prove routing reaches it).
        assert_eq!(
            reg.dispatch("/theme light"),
            Dispatch::Run {
                name: "/theme".into()
            }
        );
    }

    #[test]
    fn parse_theme_mode_maps_args_and_defaults_to_auto() {
        use crate::tui::theme::ThemeMode;
        assert_eq!(parse_theme_mode("/theme light"), ThemeMode::Light);
        assert_eq!(parse_theme_mode("/theme dark"), ThemeMode::Dark);
        assert_eq!(parse_theme_mode("/theme auto"), ThemeMode::Auto);
        // Case-insensitive.
        assert_eq!(parse_theme_mode("/theme LIGHT"), ThemeMode::Light);
        // Bare command and unknown args fall back to Auto.
        assert_eq!(parse_theme_mode("/theme"), ThemeMode::Auto);
        assert_eq!(parse_theme_mode("/theme bogus"), ThemeMode::Auto);
    }

    #[test]
    fn theme_light_arg_resolves_to_the_light_palette() {
        use crate::tui::theme::Theme;
        // `Theme::for_mode` resolves the palette by terminal capability
        // (truecolor RGB vs 256-indexed vs `Reset` under NO_COLOR). This test
        // verifies the mode→palette MAPPING, not capability detection, so pin
        // truecolor + clear NO_COLOR for a deterministic result in CI's clean
        // env (no COLORTERM/TTY). SAFETY: nextest isolates each test in its
        // own process; the write cannot race another test.
        unsafe {
            std::env::remove_var("NO_COLOR");
            std::env::set_var("COLORTERM", "truecolor");
        }
        // The router does: parse the arg, then `Theme::for_mode(mode)`.
        // `/theme light` must resolve to a Theme whose bg is the light
        // palette's bg — the live-switch contract the status bar reads.
        let mode = parse_theme_mode("/theme light");
        assert_eq!(Theme::for_mode(mode).bg, Theme::hearth_light().bg);
        // And the accent stays pinned to #ff6b35 in the resolved theme.
        assert_eq!(
            Theme::for_mode(mode).orange,
            ratatui::style::Color::Rgb(0xff, 0x6b, 0x35)
        );
    }
}
