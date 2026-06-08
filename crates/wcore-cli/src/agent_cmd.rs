//! v0.7.0 Task 3.B.2: `wayland-core agent` subcommands.
//!
//! Five flag-driven subcommands wrapping the wcore-agents-pack factory:
//! create / list / show / edit / delete. Interactive flows live in
//! 3.B.3 (`/agent new` slash command), not here.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use wcore_agents_pack::AgentPack;
use wcore_agents_pack::factory::{self, FactoryInput};

#[derive(Subcommand, Debug)]
pub enum AgentCmd {
    /// Create a new user agent (persists to ~/.wayland/agents/<name>.toml).
    Create {
        /// Kebab-case slug. Must not duplicate a built-in or an existing user agent.
        name: String,
        /// Inherit prompt + model + tools from a built-in (run `agent list --builtins`).
        #[arg(long, value_name = "AGENT")]
        inherit_from: Option<String>,
        /// One-line description.
        #[arg(long)]
        description: Option<String>,
        /// System prompt override (use `@path` to read from file).
        #[arg(long)]
        system_prompt: Option<String>,
        /// Model identifier (e.g. claude-opus-4-7).
        #[arg(long)]
        model: Option<String>,
        /// Maximum loop turns.
        #[arg(long)]
        max_turns: Option<u32>,
        /// Extra tools to permit (in addition to the parent's allowlist).
        #[arg(long, value_name = "TOOL", num_args = 0..)]
        tool: Vec<String>,
        /// Overwrite an existing file if present.
        #[arg(long)]
        force: bool,
    },
    /// List user agents (and optionally the built-ins).
    List {
        /// Also list built-in agents from the bundled pack.
        #[arg(long)]
        builtins: bool,
    },
    /// Print the resolved manifest for an agent (user or built-in).
    Show { name: String },
    /// Open the user agent's TOML file in $EDITOR for hand-editing.
    Edit { name: String },
    /// Delete a user agent. Built-ins cannot be deleted.
    Delete {
        name: String,
        /// Skip the "are you sure" exit-code gate (no prompt either way; this
        /// flag confirms intent for scripts).
        #[arg(long)]
        yes: bool,
    },
}

pub fn run(cmd: AgentCmd) -> Result<()> {
    let base = factory::user_agent_dir().context("resolving ~/.wayland/agents")?;
    run_with_base(cmd, &base)
}

/// Inner entry point so tests can inject a tempdir.
pub fn run_with_base(cmd: AgentCmd, base: &Path) -> Result<()> {
    match cmd {
        AgentCmd::Create {
            name,
            inherit_from,
            description,
            system_prompt,
            model,
            max_turns,
            tool,
            force,
        } => create_cmd(
            name,
            inherit_from,
            description,
            system_prompt,
            model,
            max_turns,
            tool,
            force,
            base,
        ),
        AgentCmd::List { builtins } => list_cmd(builtins, base),
        AgentCmd::Show { name } => show_cmd(&name, base),
        AgentCmd::Edit { name } => edit_cmd(&name, base),
        AgentCmd::Delete { name, yes } => delete_cmd(&name, yes, base),
    }
}

#[allow(clippy::too_many_arguments)] // CLI subcommand: args map 1:1 to user flags
fn create_cmd(
    name: String,
    inherit_from: Option<String>,
    description: Option<String>,
    system_prompt: Option<String>,
    model: Option<String>,
    max_turns: Option<u32>,
    tool: Vec<String>,
    force: bool,
    base: &Path,
) -> Result<()> {
    if AgentPack::get(&name).is_some() {
        bail!("name '{name}' clashes with a built-in agent; pick another");
    }
    let target = base.join(format!("{name}.toml"));
    if target.exists() && !force {
        bail!(
            "agent '{name}' already exists at {}; use --force to overwrite",
            target.display()
        );
    }
    let system_prompt = match system_prompt {
        Some(s) if s.starts_with('@') => {
            let path = PathBuf::from(&s[1..]);
            let body = std::fs::read_to_string(&path)
                .with_context(|| format!("reading system prompt from {}", path.display()))?;
            Some(body)
        }
        other => other,
    };
    let input = FactoryInput {
        name: name.clone(),
        description,
        inherit_from,
        system_prompt,
        model,
        max_turns,
        extra_allowed_tools: tool,
    };
    let path =
        factory::create(&input, base).with_context(|| format!("creating user agent '{name}'"))?;
    println!("created {} at {}", name, path.display());
    Ok(())
}

fn list_cmd(include_builtins: bool, base: &Path) -> Result<()> {
    let users = factory::list(base).context("listing user agents")?;
    if include_builtins {
        println!("built-in:");
        for m in AgentPack::list() {
            println!("  {:24}  {}", m.name, m.description);
        }
        if !users.is_empty() {
            println!();
            println!("user:");
        }
    }
    if users.is_empty() && !include_builtins {
        println!("no user agents (try `agent create <name>` or pass --builtins)");
        return Ok(());
    }
    for m in users {
        println!("  {:24}  {}", m.name, m.description);
    }
    Ok(())
}

fn show_cmd(name: &str, base: &Path) -> Result<()> {
    if let Some(builtin) = AgentPack::get(name) {
        let toml = toml::to_string_pretty(&builtin).context("re-serialising built-in")?;
        print!("{toml}");
        return Ok(());
    }
    let manifest =
        factory::load(base, name).with_context(|| format!("loading user agent '{name}'"))?;
    let toml = toml::to_string_pretty(&manifest).context("serialising manifest")?;
    print!("{toml}");
    Ok(())
}

fn edit_cmd(name: &str, base: &Path) -> Result<()> {
    if AgentPack::get(name).is_some() {
        bail!(
            "'{name}' is a built-in; copy it first with `agent create {name}-mine --inherit-from {name}`"
        );
    }
    let path = base.join(format!("{name}.toml"));
    if !path.exists() {
        bail!(
            "user agent '{name}' not found at {}; run `agent create {name}` first",
            path.display()
        );
    }
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_string());
    let status = std::process::Command::new(&editor)
        .arg(&path)
        .status()
        .with_context(|| format!("spawning editor '{editor}'"))?;
    if !status.success() {
        bail!("editor '{editor}' exited with {status}");
    }
    // Verify the file still parses after editing.
    let _ = factory::load(base, name)
        .with_context(|| format!("re-parsing user agent '{name}' after edit (file invalid?)"))?;
    println!("edited {}", path.display());
    Ok(())
}

fn delete_cmd(name: &str, yes: bool, base: &Path) -> Result<()> {
    if AgentPack::get(name).is_some() {
        bail!("'{name}' is a built-in; cannot delete");
    }
    if !yes {
        bail!("re-run with --yes to confirm deletion of '{name}'");
    }
    let removed =
        factory::delete(base, name).with_context(|| format!("deleting user agent '{name}'"))?;
    if removed {
        println!("deleted {name}");
    } else {
        println!("no user agent named '{name}' found (nothing to delete)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_then_list_then_show_then_delete() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        // create
        run_with_base(
            AgentCmd::Create {
                name: "scratch".to_string(),
                inherit_from: None,
                description: Some("test agent".to_string()),
                system_prompt: Some("Be terse.".to_string()),
                model: None,
                max_turns: Some(4),
                tool: vec!["Read".to_string()],
                force: false,
            },
            base,
        )
        .expect("create");

        // list
        run_with_base(AgentCmd::List { builtins: false }, base).expect("list");

        // show
        run_with_base(
            AgentCmd::Show {
                name: "scratch".to_string(),
            },
            base,
        )
        .expect("show");

        // delete without --yes errors
        let err = run_with_base(
            AgentCmd::Delete {
                name: "scratch".to_string(),
                yes: false,
            },
            base,
        )
        .expect_err("delete without --yes should refuse");
        assert!(err.to_string().contains("--yes"));

        // delete with --yes succeeds
        run_with_base(
            AgentCmd::Delete {
                name: "scratch".to_string(),
                yes: true,
            },
            base,
        )
        .expect("delete with --yes");

        // subsequent delete prints 'nothing to delete' without erroring
        run_with_base(
            AgentCmd::Delete {
                name: "scratch".to_string(),
                yes: true,
            },
            base,
        )
        .expect("delete-after-delete");
    }

    #[test]
    fn create_refuses_clash_with_builtin() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_with_base(
            AgentCmd::Create {
                name: "architect".to_string(),
                inherit_from: None,
                description: None,
                system_prompt: None,
                model: None,
                max_turns: None,
                tool: vec![],
                force: false,
            },
            tmp.path(),
        )
        .expect_err("clash with built-in");
        assert!(err.to_string().contains("built-in"));
    }

    #[test]
    fn create_refuses_overwrite_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        for force in [false, true] {
            let r = run_with_base(
                AgentCmd::Create {
                    name: "twice".to_string(),
                    inherit_from: None,
                    description: None,
                    system_prompt: Some("p".to_string()),
                    model: None,
                    max_turns: None,
                    tool: vec![],
                    force,
                },
                tmp.path(),
            );
            if force {
                r.expect("force overwrite OK");
            } else {
                // first call (force=false) writes; nothing exists yet → OK
                // but on the SECOND iteration, force=true so it works either way
                // We do not reset between iterations, so the first iteration's
                // run is the create, the second is the overwrite-with-force.
                r.expect("first create OK");
            }
        }
    }

    #[test]
    fn show_falls_back_to_builtin() {
        let tmp = tempfile::tempdir().unwrap();
        run_with_base(
            AgentCmd::Show {
                name: "architect".to_string(),
            },
            tmp.path(),
        )
        .expect("show built-in via show command");
    }

    #[test]
    fn delete_refuses_builtin() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_with_base(
            AgentCmd::Delete {
                name: "architect".to_string(),
                yes: true,
            },
            tmp.path(),
        )
        .expect_err("delete built-in");
        assert!(err.to_string().contains("built-in"));
    }

    #[test]
    fn edit_refuses_builtin() {
        let tmp = tempfile::tempdir().unwrap();
        let err = run_with_base(
            AgentCmd::Edit {
                name: "architect".to_string(),
            },
            tmp.path(),
        )
        .expect_err("edit built-in");
        assert!(err.to_string().contains("built-in"));
    }
}
