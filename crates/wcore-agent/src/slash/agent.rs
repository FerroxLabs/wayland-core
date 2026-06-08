use std::io::{BufRead, Write};
use std::path::Path;

use wcore_agents_pack::AgentPack;
use wcore_agents_pack::factory::{self, FactoryInput};

use super::{SlashError, SlashHandler, SlashInvocation, SlashOutcome};

#[derive(Debug)]
pub struct AgentHandler;

impl SlashHandler for AgentHandler {
    fn name(&self) -> &str {
        "agent"
    }
    fn one_line_help(&self) -> &str {
        "List, show, or create an agent."
    }
    fn invoke(&self, invocation: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        match invocation.args.split_first() {
            None => list(),
            Some((first, rest)) => match first.as_str() {
                "list" => list(),
                "show" => show(rest),
                "new" => run_new_via_stdio(),
                other => Err(SlashError::Bad(format!(
                    "/agent: unknown sub-action '{other}'. Try: list | show <name> | new"
                ))),
            },
        }
    }
}

fn list() -> Result<SlashOutcome, SlashError> {
    let mut lines = vec!["built-in agents:".to_string()];
    for m in AgentPack::list() {
        lines.push(format!("  {:24}  {}", m.name, m.description));
    }
    lines.push(String::new());
    lines.push(
        "user agents: use `wayland-core agent list` from the CLI (or /agent new)".to_string(),
    );
    Ok(SlashOutcome::Handled {
        output: Some(lines.join("\n")),
    })
}

/// Run the interactive `/agent new` flow with stdio as the [`PromptIo`].
/// Production entry-point — bound directly from the slash handler.
fn run_new_via_stdio() -> Result<SlashOutcome, SlashError> {
    let dir = factory::user_agent_dir()
        .map_err(|e| SlashError::Bad(format!("could not resolve user-agent dir: {e}")))?;
    let stdin = std::io::stdin();
    let mut io = StdioPromptIo::new(stdin.lock(), std::io::stdout());
    let name = run_new(&mut io, &dir).map_err(|e| SlashError::Bad(e.to_string()))?;
    Ok(SlashOutcome::Handled {
        output: Some(format!("agent '{name}' saved to {}", dir.display())),
    })
}

fn show(rest: &[String]) -> Result<SlashOutcome, SlashError> {
    let name = rest.first().ok_or_else(|| {
        SlashError::Bad("/agent show requires a name (run /agent list)".to_string())
    })?;
    let m = AgentPack::get(name).ok_or_else(|| {
        SlashError::Bad(format!(
            "no built-in agent named '{name}' (run /agent list)"
        ))
    })?;
    let body = format!(
        "name: {}\ndescription: {}\nmodel: {:?}\nmax_turns: {:?}\nallowed_tools: {:?}\n\nsystem_prompt:\n{}",
        m.name, m.description, m.model, m.max_turns, m.allowed_tools, m.system_prompt
    );
    Ok(SlashOutcome::Handled { output: Some(body) })
}

/// I/O indirection for the interactive `/agent new` flow.
///
/// Implementations: [`StdioPromptIo`] for real sessions; [`ScriptedPromptIo`]
/// for tests (deterministic input/output capture). The session layer (3.C.4)
/// passes a `PromptIo` rooted on the actual terminal sink.
pub trait PromptIo {
    fn prompt(&mut self, msg: &str) -> std::io::Result<String>;
    fn write_line(&mut self, msg: &str) -> std::io::Result<()>;
}

/// Default impl: read from stdin, write to stdout. Used by the session
/// layer when a real terminal is attached.
#[derive(Debug)]
pub struct StdioPromptIo<R, W> {
    reader: R,
    writer: W,
}

impl<R: BufRead, W: Write> StdioPromptIo<R, W> {
    pub fn new(reader: R, writer: W) -> Self {
        Self { reader, writer }
    }
}

impl<R: BufRead, W: Write> PromptIo for StdioPromptIo<R, W> {
    fn prompt(&mut self, msg: &str) -> std::io::Result<String> {
        write!(self.writer, "{msg}")?;
        self.writer.flush()?;
        let mut line = String::new();
        self.reader.read_line(&mut line)?;
        Ok(line.trim_end_matches(['\r', '\n']).to_string())
    }
    fn write_line(&mut self, msg: &str) -> std::io::Result<()> {
        writeln!(self.writer, "{msg}")?;
        self.writer.flush()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentNewError {
    #[error("interactive flow aborted at step '{step}'")]
    Aborted { step: &'static str },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("factory: {0}")]
    Factory(#[from] wcore_agents_pack::factory::FactoryError),
    #[error("{0}")]
    Validation(String),
}

/// Run the multi-step `/agent new` flow against a [`PromptIo`]
/// implementation. Persists the resulting manifest to `base_dir/<name>.toml`
/// (normally `~/.wayland/agents/`). Returns the persisted manifest's name on
/// success so the caller can immediately enable it via `--agent=<name>`.
pub fn run_new<P: PromptIo>(io: &mut P, base_dir: &Path) -> Result<String, AgentNewError> {
    io.write_line("--- /agent new ---")?;
    io.write_line("(blank line at any prompt aborts; defaults shown in [brackets])")?;

    // Step 1: persona description (becomes the description field)
    let description = require_non_empty(io, "1. One-line description: ", "description")?;

    // Step 2: inherit from?
    io.write_line("\n2. Inherit prompt + tools from a built-in? (blank = no)")?;
    io.write_line("   built-ins:")?;
    let names = AgentPack::names();
    let mut row = String::new();
    for (i, n) in names.iter().enumerate() {
        if !row.is_empty() {
            row.push_str(", ");
        }
        row.push_str(n);
        if (i + 1) % 4 == 0 {
            io.write_line(&format!("     {row}"))?;
            row.clear();
        }
    }
    if !row.is_empty() {
        io.write_line(&format!("     {row}"))?;
    }
    let parent = io.prompt("   inherit_from: ")?;
    let inherit_from = if parent.is_empty() {
        None
    } else if AgentPack::get(&parent).is_none() {
        return Err(AgentNewError::Validation(format!(
            "unknown built-in '{parent}' — pick one from the list above or leave blank"
        )));
    } else {
        Some(parent)
    };

    // Step 3: extra allowed tools
    io.write_line(
        "\n3. Extra tools to permit (in addition to parent's allowlist).\n   \
         comma-separated, e.g. 'Read,Bash,Edit'. Leave blank for none.",
    )?;
    let tools_line = io.prompt("   tools: ")?;
    let extra_allowed_tools: Vec<String> = tools_line
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    // Step 4: model override
    let model_line = io.prompt(
        "\n4. Model override (e.g. claude-opus-4-7). Leave blank to inherit parent's.\n   model: ",
    )?;
    let model = if model_line.is_empty() {
        None
    } else {
        Some(model_line)
    };

    // Step 5: max_turns override
    let max_turns_line =
        io.prompt("\n5. Max loop turns. Leave blank to inherit parent's.\n   max_turns: ")?;
    let max_turns = if max_turns_line.is_empty() {
        None
    } else {
        Some(max_turns_line.parse::<u32>().map_err(|_| {
            AgentNewError::Validation(format!(
                "max_turns must be a positive integer (got '{max_turns_line}')"
            ))
        })?)
    };

    // Step 6: save name
    let name = require_non_empty(io, "\n6. Save name (kebab-case): ", "name")?;
    if AgentPack::get(&name).is_some() {
        return Err(AgentNewError::Validation(format!(
            "'{name}' clashes with a built-in; pick a different name"
        )));
    }

    let input = FactoryInput {
        name: name.clone(),
        description: Some(description),
        inherit_from,
        system_prompt: None,
        model,
        max_turns,
        extra_allowed_tools,
    };

    let path = factory::create(&input, base_dir)?;
    io.write_line(&format!(
        "\nSaved {} at {}.\nUse --agent={} on the next session to enable it.",
        name,
        path.display(),
        name
    ))?;
    Ok(name)
}

fn require_non_empty<P: PromptIo>(
    io: &mut P,
    msg: &str,
    step: &'static str,
) -> Result<String, AgentNewError> {
    let s = io.prompt(msg)?;
    if s.is_empty() {
        return Err(AgentNewError::Aborted { step });
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash::parse;

    /// Scripted IO for tests — feeds queued answers, captures every
    /// printed line.
    #[derive(Debug, Default)]
    struct ScriptedPromptIo {
        answers: std::collections::VecDeque<String>,
        captured: Vec<String>,
    }

    impl ScriptedPromptIo {
        fn new(answers: &[&str]) -> Self {
            Self {
                answers: answers.iter().map(|s| s.to_string()).collect(),
                captured: Vec::new(),
            }
        }
    }

    impl PromptIo for ScriptedPromptIo {
        fn prompt(&mut self, msg: &str) -> std::io::Result<String> {
            self.captured.push(format!("PROMPT: {msg}"));
            self.answers.pop_front().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "no scripted answer")
            })
        }
        fn write_line(&mut self, msg: &str) -> std::io::Result<()> {
            self.captured.push(format!("LINE: {msg}"));
            Ok(())
        }
    }

    // /agent list + show still work from 3.C.2
    #[test]
    fn list_shows_builtins() {
        let inv = parse("/agent list").unwrap();
        let out = AgentHandler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!("expected Handled");
        };
        assert!(s.contains("architect"));
        assert!(s.contains("debugger"));
    }

    #[test]
    fn show_known_builtin() {
        let inv = parse("/agent show architect").unwrap();
        let out = AgentHandler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!("expected Handled");
        };
        assert!(s.contains("name: architect"));
    }

    #[test]
    fn show_missing_name_errors() {
        let inv = parse("/agent show").unwrap();
        assert!(matches!(AgentHandler.invoke(&inv), Err(SlashError::Bad(_))));
    }

    #[test]
    fn new_via_slash_invokes_run_new_and_aborts_on_empty_stdin() {
        // Phase 5: `/agent new` is now actually wired to `run_new` with a
        // stdio PromptIo. Under `cargo test`, stdin is EOF, so the flow
        // aborts at the first required prompt ("description") and the
        // handler surfaces that as SlashError::Bad. Asserting the abort
        // path verifies the wire-up — the stub message no longer exists.
        let inv = parse("/agent new").unwrap();
        match AgentHandler.invoke(&inv) {
            Err(SlashError::Bad(msg)) => {
                // AgentNewError::Aborted { step: "description" } stringifies
                // to "interactive flow aborted at step 'description'" via
                // thiserror's Display impl.
                assert!(
                    msg.contains("aborted") || msg.contains("description"),
                    "expected aborted-flow error, got: {msg}"
                );
            }
            other => panic!("expected Bad on EOF stdin, got {other:?}"),
        }
    }

    #[test]
    fn run_new_happy_path() {
        let tmp = tempfile::tempdir().unwrap();
        let mut io = ScriptedPromptIo::new(&[
            "My custom debugger", // description
            "debugger",           // inherit_from
            "WebFetch",           // extra tools
            "",                   // model — inherit
            "20",                 // max_turns
            "my-debugger",        // save name
        ]);
        let name = run_new(&mut io, tmp.path()).expect("happy path");
        assert_eq!(name, "my-debugger");

        let path = tmp.path().join("my-debugger.toml");
        assert!(path.exists());
        let loaded = factory::load(tmp.path(), "my-debugger").unwrap();
        assert_eq!(loaded.description, "My custom debugger");
        assert_eq!(loaded.max_turns, Some(20));
        assert!(loaded.allowed_tools.iter().any(|t| t == "WebFetch"));
        // inherited from debugger
        assert!(loaded.system_prompt.contains("debugging"));
    }

    #[test]
    fn run_new_minimal_no_parent_no_tools() {
        let tmp = tempfile::tempdir().unwrap();
        let mut io = ScriptedPromptIo::new(&[
            "A blank-slate agent", // description
            "",                    // inherit_from: none
            "",                    // tools: none
            "",                    // model: none
            "",                    // max_turns: none
            "scratch",             // name
        ]);
        let name = run_new(&mut io, tmp.path()).expect("minimal");
        assert_eq!(name, "scratch");
        let loaded = factory::load(tmp.path(), "scratch").unwrap();
        assert_eq!(loaded.description, "A blank-slate agent");
        assert!(loaded.system_prompt.contains("terse")); // factory default
    }

    #[test]
    fn run_new_rejects_clash_with_builtin() {
        let tmp = tempfile::tempdir().unwrap();
        let mut io = ScriptedPromptIo::new(&[
            "Custom architect", // description
            "",                 // inherit_from: none
            "",                 // tools
            "",                 // model
            "",                 // max_turns
            "architect",        // clashes
        ]);
        match run_new(&mut io, tmp.path()) {
            Err(AgentNewError::Validation(msg)) => assert!(msg.contains("clashes")),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn run_new_rejects_unknown_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let mut io = ScriptedPromptIo::new(&[
            "Custom",       // description
            "made-up-name", // bad parent
        ]);
        match run_new(&mut io, tmp.path()) {
            Err(AgentNewError::Validation(msg)) => assert!(msg.contains("unknown")),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn run_new_rejects_non_numeric_max_turns() {
        let tmp = tempfile::tempdir().unwrap();
        let mut io = ScriptedPromptIo::new(&[
            "Custom", "",        // inherit
            "",        // tools
            "",        // model
            "fifteen", // bad max_turns
        ]);
        match run_new(&mut io, tmp.path()) {
            Err(AgentNewError::Validation(msg)) => assert!(msg.contains("max_turns")),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn run_new_aborts_on_blank_description() {
        let tmp = tempfile::tempdir().unwrap();
        let mut io = ScriptedPromptIo::new(&[""]);
        match run_new(&mut io, tmp.path()) {
            Err(AgentNewError::Aborted { step }) => assert_eq!(step, "description"),
            other => panic!("expected Aborted, got {other:?}"),
        }
    }
}
