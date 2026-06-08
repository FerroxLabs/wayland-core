use std::sync::Arc;

use wcore_skills::refs::SkillCatalog;

use super::{SlashError, SlashHandler, SlashInvocation, SlashOutcome};

/// `/skill` handler. Two variants:
///
/// - [`SkillHandler::Stub`] returns the v0.7.0 placeholder strings.
/// - [`SkillHandler::Runtime`] enumerates the session's resolved
///   [`SkillCatalog`] — the same catalog that backs the model's
///   `SkillTool`. `show` / `run` are intentionally read-only here:
///   "run" defers to the normal `SkillTool` tool-call path (the model
///   does the dispatch via tool call, not via slash-command), so the
///   handler explains the workflow rather than fabricating a fake
///   execution channel.
#[derive(Clone, Default)]
pub enum SkillHandler {
    #[default]
    Stub,
    Runtime {
        catalog: Arc<SkillCatalog>,
    },
}

impl std::fmt::Debug for SkillHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stub => f.debug_struct("SkillHandler::Stub").finish(),
            Self::Runtime { catalog } => f
                .debug_struct("SkillHandler::Runtime")
                .field("catalog_len", &catalog.len())
                .finish(),
        }
    }
}

impl SlashHandler for SkillHandler {
    fn name(&self) -> &str {
        "skill"
    }
    fn one_line_help(&self) -> &str {
        "List / show / run a skill."
    }
    fn invoke(&self, invocation: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        match invocation.args.split_first() {
            None => self.list(),
            Some((first, rest)) => match first.as_str() {
                "list" => self.list(),
                "show" => {
                    let name = rest
                        .first()
                        .ok_or_else(|| SlashError::Bad("/skill show <name>".to_string()))?;
                    self.show(name)
                }
                "run" => {
                    let name = rest.first().ok_or_else(|| {
                        SlashError::Bad("/skill run <name> [...args]".to_string())
                    })?;
                    self.run(name)
                }
                other => Err(SlashError::Bad(format!(
                    "/skill: unknown sub-action '{other}'. Try: list | show <name> | run <name>"
                ))),
            },
        }
    }
}

impl SkillHandler {
    fn list(&self) -> Result<SlashOutcome, SlashError> {
        match self {
            Self::Stub => Ok(SlashOutcome::Handled {
                output: Some(
                    "/skill list: full skill inventory needs the runtime \
                     SkillRegistry handle; use `wayland-core --skills-audit` \
                     from the CLI in v0.7.0."
                        .to_string(),
                ),
            }),
            Self::Runtime { catalog } => Ok(SlashOutcome::Handled {
                output: Some(runtime_list(catalog)),
            }),
        }
    }

    fn show(&self, name: &str) -> Result<SlashOutcome, SlashError> {
        match self {
            Self::Stub => Ok(SlashOutcome::Handled {
                output: Some(format!(
                    "use `wayland-core --skills-audit` then grep for '{name}' in v0.7.0"
                )),
            }),
            Self::Runtime { catalog } => Ok(SlashOutcome::Handled {
                output: Some(runtime_show(catalog, name)),
            }),
        }
    }

    fn run(&self, name: &str) -> Result<SlashOutcome, SlashError> {
        match self {
            Self::Stub => Ok(SlashOutcome::Handled {
                output: Some(format!(
                    "/skill run '{name}': runtime dispatch wired in 3.C.4 alongside the TUI."
                )),
            }),
            Self::Runtime { catalog } => {
                // The agent dispatches skills via SkillTool tool calls, not via
                // a direct slash-command path: that's the contract SkillTool was
                // wired against (catalog → tool dispatch → execution + procedural
                // telemetry). Calling out from a slash handler would bypass the
                // approval pipeline + the procedural-memory recording.
                // Instead, validate the skill exists and instruct the user.
                let exists = catalog.find(name).is_some();
                if exists {
                    Ok(SlashOutcome::Handled {
                        output: Some(format!(
                            "/skill run '{name}': skill exists in the catalog. \
                             Skill dispatch flows through the agent's SkillTool — \
                             ask the agent to use the skill (e.g. \"use the {name} skill\") \
                             so the request goes through the approval + telemetry pipeline."
                        )),
                    })
                } else {
                    Ok(SlashOutcome::Handled {
                        output: Some(format!(
                            "/skill run '{name}': no skill named '{name}' in the catalog. \
                             Run `/skill list` to see available skills."
                        )),
                    })
                }
            }
        }
    }
}

fn runtime_list(catalog: &SkillCatalog) -> String {
    if catalog.is_empty() {
        return "/skill list: no skills loaded in this session.".to_string();
    }
    let mut out = format!("Skills in catalog ({}):\n", catalog.len());
    let mut visible = 0usize;
    let mut hidden = 0usize;
    for r in catalog.refs() {
        let tag = if r.disable_model_invocation {
            hidden += 1;
            "(hidden)"
        } else {
            visible += 1;
            ""
        };
        let src = format!("{:?}", r.source).to_lowercase();
        out.push_str(&format!(
            "  - {name}{tag} [src={src}]\n",
            name = r.name,
            tag = if tag.is_empty() {
                String::new()
            } else {
                format!(" {tag}")
            },
        ));
    }
    out.push_str(&format!(
        "\nSummary: {visible} visible to the model, {hidden} hidden.\n"
    ));
    out
}

fn runtime_show(catalog: &SkillCatalog, name: &str) -> String {
    match catalog.find(name) {
        None => format!(
            "/skill show '{name}': not found in catalog. Run `/skill list` to see available skills."
        ),
        Some(r) => {
            let mut out = format!("Skill: {}\n", r.name);
            if let Some(d) = &r.display_name {
                out.push_str(&format!("  display_name: {d}\n"));
            }
            out.push_str(&format!("  description: {}\n", r.description));
            if let Some(w) = &r.when_to_use {
                out.push_str(&format!("  when_to_use: {w}\n"));
            }
            if !r.paths.is_empty() {
                out.push_str(&format!("  paths: {:?}\n", r.paths));
            }
            out.push_str(&format!("  source: {:?}\n", r.source));
            out.push_str(&format!("  loaded_from: {:?}\n", r.loaded_from));
            out.push_str(&format!("  file_path: {}\n", r.file_path.display()));
            out.push_str(&format!(
                "  visibility: {}\n",
                if r.disable_model_invocation {
                    "hidden from model"
                } else {
                    "visible to model"
                }
            ));
            out.push_str(&format!("  user_invocable: {}\n", r.user_invocable));
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash::parse;

    // ------------------------------------------------------------------
    // Back-compat tests — Stub variant preserves the v0.7.0 behaviour
    // ------------------------------------------------------------------

    #[test]
    fn stub_list_handled() {
        let inv = parse("/skill list").unwrap();
        let out = SkillHandler::Stub.invoke(&inv).unwrap();
        assert!(matches!(out, SlashOutcome::Handled { output: Some(_) }));
    }

    #[test]
    fn stub_show_requires_name() {
        let inv = parse("/skill show").unwrap();
        assert!(matches!(
            SkillHandler::Stub.invoke(&inv),
            Err(SlashError::Bad(_))
        ));
    }

    #[test]
    fn stub_run_requires_name() {
        let inv = parse("/skill run").unwrap();
        assert!(matches!(
            SkillHandler::Stub.invoke(&inv),
            Err(SlashError::Bad(_))
        ));
    }

    #[test]
    fn default_constructs_stub() {
        let h = SkillHandler::default();
        assert!(matches!(h, SkillHandler::Stub));
    }

    // ------------------------------------------------------------------
    // Runtime variant — exercises the real catalog
    // ------------------------------------------------------------------

    fn empty_catalog() -> Arc<SkillCatalog> {
        Arc::new(SkillCatalog::from_refs(Vec::new()))
    }

    #[test]
    fn runtime_list_empty_catalog() {
        let handler = SkillHandler::Runtime {
            catalog: empty_catalog(),
        };
        let inv = parse("/skill list").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        // Must NOT contain the stub-mode placeholder.
        assert!(
            !s.contains("--skills-audit"),
            "runtime list leaked stub string: {s}"
        );
        assert!(s.contains("no skills"), "got: {s}");
    }

    #[test]
    fn runtime_show_missing_skill() {
        let handler = SkillHandler::Runtime {
            catalog: empty_catalog(),
        };
        let inv = parse("/skill show nonexistent").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        assert!(s.contains("not found"), "got: {s}");
        assert!(s.contains("nonexistent"), "got: {s}");
    }

    #[test]
    fn runtime_run_missing_skill_says_not_found() {
        let handler = SkillHandler::Runtime {
            catalog: empty_catalog(),
        };
        let inv = parse("/skill run nope").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        // Must NOT contain the stub-mode placeholder.
        assert!(!s.contains("3.C.4"), "runtime run leaked stub string: {s}");
        assert!(s.contains("no skill named"), "got: {s}");
    }
}
