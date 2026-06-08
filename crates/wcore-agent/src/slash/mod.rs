//! v0.7.0 Task 3.C.1: slash-command parser + dispatcher.
//!
//! Commands are detected client-side before LLM dispatch. A line that
//! starts with `/` (no leading whitespace) and whose first token is a
//! known command is parsed into a [`SlashInvocation`] and routed via
//! the registered handler. Anything else is treated as ordinary user
//! input and forwarded to the agent loop unchanged.
//!
//! Phase 3.C.1 ships only the parser + dispatcher trait + a stub
//! handler per built-in command. Phase 3.C.2 replaces each stub with
//! the real implementation (`/style`, `/memory show|clear`, `/plugin`,
//! `/skill`, `/help`, `/agent`, `/clear`, `/exit`).

mod agent;
mod help;
pub(crate) mod memory;
pub(crate) mod plugin;
pub(crate) mod skill;
mod style;

use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashInvocation {
    pub command: String,
    pub args: Vec<String>,
    pub raw: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashOutcome {
    /// Command handled. Render this string inline. None means handler
    /// produced no output (e.g. `/clear`).
    Handled { output: Option<String> },
    /// Handler explicitly asked the engine to exit.
    Exit,
    /// `/style <opt>` — caller applies the directive to the engine's
    /// system prompt (via `engine.inject_history`) and renders a
    /// confirmation. The String is the ready-to-inject directive.
    SetStyle(String),
    /// `/clear` — caller drops the conversation history
    /// (`engine.clear_conversation()`) and clears the screen.
    ClearConversation,
    /// Command was syntactically valid but the handler is not yet
    /// implemented; the engine should display the message but keep the
    /// session live. 3.C.2 replaces all stubs so this variant is rarely
    /// returned after 3.C.2 lands.
    NotImplemented { message: String },
}

#[derive(Debug, thiserror::Error)]
pub enum SlashError {
    #[error("unknown slash command: /{0} (run /help)")]
    Unknown(String),
    #[error("bad slash invocation: {0}")]
    Bad(String),
}

pub trait SlashHandler: Send + Sync {
    fn name(&self) -> &str;
    fn one_line_help(&self) -> &str;
    fn invoke(&self, invocation: &SlashInvocation) -> Result<SlashOutcome, SlashError>;
}

/// Try to parse a single user-input line into a slash invocation.
///
/// Returns `Some` when the line begins with `/` followed by a non-empty
/// command token. Returns `None` for anything else (regular chat input).
pub fn parse(line: &str) -> Option<SlashInvocation> {
    let trimmed = line.trim_end_matches(['\r', '\n']);
    let rest = trimmed.strip_prefix('/')?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        return None;
    }
    let mut tokens = rest.split_whitespace();
    let command = tokens.next()?.to_string();
    let args: Vec<String> = tokens.map(|s| s.to_string()).collect();
    Some(SlashInvocation {
        command,
        args,
        raw: trimmed.to_string(),
    })
}

#[derive(Default)]
pub struct Dispatcher {
    handlers: HashMap<String, Arc<dyn SlashHandler>>,
}

impl std::fmt::Debug for Dispatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Dispatcher")
            .field("commands", &self.handlers.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl Dispatcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Dispatcher pre-populated with stub handlers for the 8 built-in
    /// commands. 3.C.2 replaces each stub with the real handler at the
    /// same name — code that called `Dispatcher::with_builtins().invoke(...)`
    /// switches transparently.
    pub fn with_builtins() -> Self {
        let mut d = Self::new();
        for stub in builtin_stubs() {
            d.register(stub);
        }
        d
    }

    /// v0.8.0 N.1 + N.2 + N.3 — Dispatcher pre-populated with the same
    /// 8 built-ins but with `/memory`, `/plugin`, `/skill` swapped to
    /// the `Runtime` enum variants that reach the real engine
    /// surfaces. `with_builtins` still constructs the back-compat
    /// `Stub` variants — this constructor is invoked once at session
    /// start by the CLI, after the engine has assembled its runtime
    /// handles.
    ///
    /// `skill_catalog` is optional because the engine constructs the
    /// catalog only when bootstrap runs. When `None`, the skill handler
    /// falls back to the `Stub` variant.
    pub fn with_runtime(
        memory_api: Arc<dyn wcore_memory::MemoryApi>,
        plugin_handles: Arc<Vec<crate::plugins::LoadedRuntimeHandle>>,
        skill_catalog: Option<Arc<wcore_skills::refs::SkillCatalog>>,
    ) -> Self {
        let mut d = Self::with_builtins();
        // `register` overwrites by handler name; the Stub variants
        // installed by `with_builtins` are replaced cleanly.
        d.register(Arc::new(memory::MemoryHandler::Runtime { api: memory_api }));
        d.register(Arc::new(plugin::PluginHandler::Runtime {
            handles: plugin_handles,
        }));
        if let Some(catalog) = skill_catalog {
            d.register(Arc::new(skill::SkillHandler::Runtime { catalog }));
        }
        d
    }

    pub fn register(&mut self, handler: Arc<dyn SlashHandler>) {
        self.handlers.insert(handler.name().to_string(), handler);
    }

    pub fn commands(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.handlers.values().map(|h| h.name()).collect();
        names.sort();
        names
    }

    pub fn help_lines(&self) -> Vec<String> {
        let mut h: Vec<String> = self
            .handlers
            .values()
            .map(|h| format!("/{:<8}  {}", h.name(), h.one_line_help()))
            .collect();
        h.sort();
        h
    }

    pub fn try_dispatch(&self, invocation: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        let handler = self
            .handlers
            .get(&invocation.command)
            .ok_or_else(|| SlashError::Unknown(invocation.command.clone()))?;
        handler.invoke(invocation)
    }
}

/// Built-in handlers shipped with the engine. 3.C.2 swapped the placeholder
/// stubs for the real per-command handlers; ExitHandler + ClearHandler
/// were already real.
fn builtin_stubs() -> Vec<Arc<dyn SlashHandler>> {
    vec![
        Arc::new(help::HelpHandler),
        Arc::new(agent::AgentHandler),
        Arc::new(style::StyleHandler),
        Arc::new(memory::MemoryHandler::Stub),
        Arc::new(plugin::PluginHandler::Stub),
        Arc::new(skill::SkillHandler::Stub),
        Arc::new(ExitHandler),
        Arc::new(ClearHandler),
    ]
}

struct ExitHandler;

impl SlashHandler for ExitHandler {
    fn name(&self) -> &str {
        "exit"
    }
    fn one_line_help(&self) -> &str {
        "Exit the session."
    }
    fn invoke(&self, _: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        Ok(SlashOutcome::Exit)
    }
}

struct ClearHandler;

impl SlashHandler for ClearHandler {
    fn name(&self) -> &str {
        "clear"
    }
    fn one_line_help(&self) -> &str {
        "Clear the screen."
    }
    fn invoke(&self, _: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        // Caller drops the conversation history and clears the screen;
        // the engine owns the message buffer, so the effect is applied
        // at the consumer site, not here.
        Ok(SlashOutcome::ClearConversation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_simple_command() {
        let inv = parse("/help").expect("parsed");
        assert_eq!(inv.command, "help");
        assert!(inv.args.is_empty());
        assert_eq!(inv.raw, "/help");
    }

    #[test]
    fn parses_command_with_args() {
        let inv = parse("/style terse").expect("parsed");
        assert_eq!(inv.command, "style");
        assert_eq!(inv.args, vec!["terse".to_string()]);
    }

    #[test]
    fn ignores_non_slash_input() {
        assert!(parse("hello world").is_none());
        assert!(
            parse(" /help").is_none(),
            "leading whitespace must not match"
        );
        assert!(
            parse("/ help").is_none(),
            "space after slash must not match"
        );
        assert!(parse("/").is_none(), "bare slash must not match");
    }

    #[test]
    fn trims_trailing_newline() {
        let inv = parse("/help\n").expect("parsed");
        assert_eq!(inv.raw, "/help");
    }

    #[test]
    fn dispatcher_lists_8_builtins() {
        let d = Dispatcher::with_builtins();
        let cmds = d.commands();
        assert_eq!(cmds.len(), 8, "expected 8 builtins, got {:?}", cmds);
        for expected in &[
            "help", "agent", "style", "memory", "plugin", "skill", "clear", "exit",
        ] {
            assert!(cmds.contains(expected), "missing builtin {expected}");
        }
    }

    #[test]
    fn dispatcher_routes_real_style_handler() {
        let d = Dispatcher::with_builtins();
        let inv = parse("/style terse").unwrap();
        let outcome = d.try_dispatch(&inv).expect("dispatched");
        match outcome {
            SlashOutcome::SetStyle(directive) => {
                assert!(
                    directive.to_lowercase().contains("terse"),
                    "got: {directive}"
                );
            }
            other => panic!("expected SetStyle, got {other:?}"),
        }
    }

    #[test]
    fn dispatcher_returns_exit() {
        let d = Dispatcher::with_builtins();
        let inv = parse("/exit").unwrap();
        let outcome = d.try_dispatch(&inv).expect("dispatched");
        assert_eq!(outcome, SlashOutcome::Exit);
    }

    #[test]
    fn dispatcher_routes_clear_as_handled() {
        let d = Dispatcher::with_builtins();
        let inv = parse("/clear").unwrap();
        let outcome = d.try_dispatch(&inv).expect("dispatched");
        assert_eq!(outcome, SlashOutcome::ClearConversation);
    }

    #[test]
    fn dispatcher_unknown_command() {
        let d = Dispatcher::with_builtins();
        let inv = parse("/totally-fake").unwrap();
        match d.try_dispatch(&inv) {
            Err(SlashError::Unknown(name)) => assert_eq!(name, "totally-fake"),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn help_lines_are_sorted() {
        let d = Dispatcher::with_builtins();
        let lines = d.help_lines();
        let mut sorted = lines.clone();
        sorted.sort();
        assert_eq!(lines, sorted);
    }

    // v0.8.0 N.* — Dispatcher::with_runtime wires the Runtime variants
    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn dispatcher_with_runtime_uses_runtime_variants() {
        let memory_api: Arc<dyn wcore_memory::MemoryApi> = Arc::new(wcore_memory::NullMemory);
        let plugin_handles: Arc<Vec<crate::plugins::LoadedRuntimeHandle>> = Arc::new(Vec::new());
        let skill_catalog: Arc<wcore_skills::refs::SkillCatalog> =
            Arc::new(wcore_skills::refs::SkillCatalog::from_refs(Vec::new()));

        let d = Dispatcher::with_runtime(memory_api, plugin_handles, Some(skill_catalog));

        // /memory show: runtime variant returns the "Memory partitions"
        // header — not the v0.7.0 "not yet routed" stub string.
        let inv = parse("/memory show").unwrap();
        let outcome = d.try_dispatch(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = outcome else {
            panic!("expected Handled output");
        };
        assert!(
            !s.contains("not yet routed"),
            "expected runtime variant, leaked stub: {s}"
        );
        assert!(s.contains("Memory partitions"), "got: {s}");

        // /plugin list: runtime variant returns the "no on-disk plugin
        // runtime handles" message — not the v0.7.0 "use the CLI" stub.
        let inv = parse("/plugin list").unwrap();
        let outcome = d.try_dispatch(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = outcome else {
            panic!("expected Handled output");
        };
        assert!(
            !s.contains("PluginRegistry handle"),
            "expected runtime variant, leaked stub: {s}"
        );

        // /skill list: runtime variant returns the "no skills loaded"
        // message — not the v0.7.0 "skills-audit" stub.
        let inv = parse("/skill list").unwrap();
        let outcome = d.try_dispatch(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = outcome else {
            panic!("expected Handled output");
        };
        assert!(
            !s.contains("--skills-audit"),
            "expected runtime variant, leaked stub: {s}"
        );
    }
}
