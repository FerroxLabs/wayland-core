use std::sync::Arc;

use crate::plugins::LoadedRuntimeHandle;

use super::{SlashError, SlashHandler, SlashInvocation, SlashOutcome};

/// `/plugin` handler. Two variants:
///
/// - [`PluginHandler::Stub`] returns the v0.7.0 placeholder strings;
///   used by [`crate::slash::Dispatcher::with_builtins`] and every
///   existing test that constructed a stub dispatcher.
/// - [`PluginHandler::Runtime`] enumerates the live keepalive vector
///   held on the engine ([`crate::engine::AgentEngine::plugin_runtime_handles`])
///   for `list`. `install` / `remove` continue to point at the
///   standalone `wayland-core plugin {install,remove}` CLI because
///   plugin install records live on disk and require a restart for the
///   running session to pick up — the slash variant is documenting the
///   real workflow, not pretending to mutate runtime state.
#[derive(Clone, Default)]
pub enum PluginHandler {
    #[default]
    Stub,
    Runtime {
        handles: Arc<Vec<LoadedRuntimeHandle>>,
    },
}

impl std::fmt::Debug for PluginHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stub => f.debug_struct("PluginHandler::Stub").finish(),
            Self::Runtime { handles } => f
                .debug_struct("PluginHandler::Runtime")
                .field("loaded", &handles.len())
                .finish(),
        }
    }
}

impl SlashHandler for PluginHandler {
    fn name(&self) -> &str {
        "plugin"
    }
    fn one_line_help(&self) -> &str {
        "List / install / remove plugins."
    }
    fn invoke(&self, invocation: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        match invocation.args.split_first() {
            None => self.list(),
            Some((first, rest)) => match first.as_str() {
                "list" => self.list(),
                "install" => {
                    let name = rest
                        .first()
                        .ok_or_else(|| SlashError::Bad("/plugin install <name>".to_string()))?;
                    self.install(name)
                }
                "remove" => {
                    let name = rest
                        .first()
                        .ok_or_else(|| SlashError::Bad("/plugin remove <name>".to_string()))?;
                    self.remove(name)
                }
                other => Err(SlashError::Bad(format!(
                    "/plugin: unknown sub-action '{other}'. Try: list | install <name> | remove <name>"
                ))),
            },
        }
    }
}

impl PluginHandler {
    fn list(&self) -> Result<SlashOutcome, SlashError> {
        match self {
            Self::Stub => Ok(SlashOutcome::Handled {
                output: Some(
                    "/plugin list: full plugin inventory needs the runtime \
                     PluginRegistry handle; use `wayland-core plugin list` \
                     from the CLI in v0.7.0."
                        .to_string(),
                ),
            }),
            Self::Runtime { handles } => Ok(SlashOutcome::Handled {
                output: Some(runtime_list(handles.as_ref())),
            }),
        }
    }

    fn install(&self, name: &str) -> Result<SlashOutcome, SlashError> {
        match self {
            Self::Stub => Ok(SlashOutcome::Handled {
                output: Some(format!(
                    "use `wayland-core plugin install {name}` from the CLI in v0.7.0"
                )),
            }),
            Self::Runtime { .. } => Ok(SlashOutcome::Handled {
                output: Some(format!(
                    "/plugin install {name}: install records live on disk and require \
                     a restart for this session to pick up the new plugin. \
                     Run `wayland-core plugin install {name}` from another \
                     terminal, then restart this session."
                )),
            }),
        }
    }

    fn remove(&self, name: &str) -> Result<SlashOutcome, SlashError> {
        match self {
            Self::Stub => Ok(SlashOutcome::Handled {
                output: Some(format!(
                    "use `wayland-core plugin remove {name}` from the CLI in v0.7.0"
                )),
            }),
            Self::Runtime { .. } => Ok(SlashOutcome::Handled {
                output: Some(format!(
                    "/plugin remove {name}: install records live on disk; \
                     run `wayland-core plugin remove {name}` from another \
                     terminal. The change takes effect at the next session start."
                )),
            }),
        }
    }
}

fn runtime_list(handles: &[LoadedRuntimeHandle]) -> String {
    if handles.is_empty() {
        return "/plugin list: no on-disk plugin runtime handles loaded \
                in this session (built-in static plugins are wired \
                directly into the engine and are not enumerated here)."
            .to_string();
    }
    let mut out = format!("Loaded plugin runtime handles ({}):\n", handles.len());
    for (i, h) in handles.iter().enumerate() {
        let (kind, name) = describe_handle(h);
        out.push_str(&format!("  {i}. [{kind}] {name}\n"));
    }
    out
}

fn describe_handle(h: &LoadedRuntimeHandle) -> (&'static str, String) {
    match h {
        LoadedRuntimeHandle::None => ("none", "(empty)".to_string()),
        LoadedRuntimeHandle::Wasm(p) => ("wasm", p.name().to_string()),
        LoadedRuntimeHandle::Subprocess(_) => {
            // LoadedSubprocessPlugin doesn't expose a public `name()` accessor;
            // its internal `plugin_name` lives on the runner. The runtime list
            // surfaces the variant + tool count so operators can still tell
            // subprocess plugins apart.
            ("subprocess", "subprocess plugin".to_string())
        }
        LoadedRuntimeHandle::McpBridge(p) => (
            "mcp-bridge",
            format!("{} synthesized tools", p.tool_count()),
        ),
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
        let inv = parse("/plugin list").unwrap();
        let out = PluginHandler::Stub.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        assert!(s.contains("CLI"));
    }

    #[test]
    fn stub_install_requires_name() {
        let inv = parse("/plugin install").unwrap();
        assert!(matches!(
            PluginHandler::Stub.invoke(&inv),
            Err(SlashError::Bad(_))
        ));
    }

    #[test]
    fn stub_remove_requires_name() {
        let inv = parse("/plugin remove").unwrap();
        assert!(matches!(
            PluginHandler::Stub.invoke(&inv),
            Err(SlashError::Bad(_))
        ));
    }

    #[test]
    fn default_constructs_stub() {
        let h = PluginHandler::default();
        assert!(matches!(h, PluginHandler::Stub));
    }

    // ------------------------------------------------------------------
    // Runtime variant — exercises the real engine surface
    // ------------------------------------------------------------------

    #[test]
    fn runtime_list_empty_handles() {
        let handler = PluginHandler::Runtime {
            handles: Arc::new(Vec::new()),
        };
        let inv = parse("/plugin list").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        // Must NOT contain the stub-mode placeholder.
        assert!(
            !s.contains("v0.7.0"),
            "runtime list leaked stub string: {s}"
        );
        assert!(s.contains("no on-disk plugin runtime handles"), "got: {s}");
    }

    #[test]
    fn runtime_install_documents_workflow() {
        let handler = PluginHandler::Runtime {
            handles: Arc::new(Vec::new()),
        };
        let inv = parse("/plugin install demo").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        // Runtime variant explains the disk + restart requirement; not the v0.7.0 stub.
        assert!(s.contains("restart"), "got: {s}");
        assert!(s.contains("install demo"), "got: {s}");
    }

    #[test]
    fn runtime_remove_documents_workflow() {
        let handler = PluginHandler::Runtime {
            handles: Arc::new(Vec::new()),
        };
        let inv = parse("/plugin remove demo").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        assert!(s.contains("next session start"), "got: {s}");
        assert!(s.contains("remove demo"), "got: {s}");
    }
}
