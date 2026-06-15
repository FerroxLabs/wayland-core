//! Lane D (G3): path-variable substitution for installed marketplace plugins.
//!
//! Claude Code plugins reference their own install dir + persistent data dir via
//! `${CLAUDE_PLUGIN_ROOT}` / `${CLAUDE_PLUGIN_DATA}`, and the session project
//! root via `${CLAUDE_PROJECT_DIR}`. These resolve at MCP-load time (not baked
//! at install time) so the install dir can move between sessions. Without this,
//! a stdio MCP `command` like `${CLAUDE_PLUGIN_ROOT}/bin/server` reaches the OS
//! verbatim, fails the reachability probe, and the server is silently skipped.
//!
//! Unknown `${...}` placeholders are left literal (logged once) rather than
//! blanked — a missing var should fail loudly at spawn, not silently mangle a
//! path into something that resolves elsewhere.

use std::path::{Path, PathBuf};

use wcore_plugin_api::mcp_server_spec::{McpServerSpec, McpTransport};

/// Resolution context for one installed plugin.
#[derive(Debug, Clone)]
pub struct PluginPathCtx {
    /// `${CLAUDE_PLUGIN_ROOT}` — the plugin's install directory.
    pub root: PathBuf,
    /// `${CLAUDE_PLUGIN_DATA}` — persistent per-plugin state directory.
    pub data: PathBuf,
    /// `${CLAUDE_PROJECT_DIR}` — the session workspace root.
    pub project: PathBuf,
}

impl PluginPathCtx {
    /// Build the standard context for a plugin installed at `install_dir`.
    /// `${CLAUDE_PLUGIN_DATA}` resolves to
    /// `<data_dir>/wayland/plugins/data/<sanitized-plugin-name>`.
    pub fn for_plugin(install_dir: &Path, plugin_name: &str, project: &Path) -> Self {
        let data = dirs::data_dir()
            .unwrap_or_else(|| install_dir.to_path_buf())
            .join("wayland")
            .join("plugins")
            .join("data")
            .join(sanitize(plugin_name));
        Self {
            root: install_dir.to_path_buf(),
            data,
            project: project.to_path_buf(),
        }
    }

    /// Ensure the per-plugin data dir exists. Called lazily the first time a
    /// plugin references `${CLAUDE_PLUGIN_DATA}` so we don't create dirs for
    /// plugins that never use it.
    fn ensure_data_dir(&self) {
        if let Err(e) = std::fs::create_dir_all(&self.data) {
            tracing::warn!(dir = %self.data.display(), error = %e, "could not create plugin data dir");
        }
    }
}

/// Substitute the three `${CLAUDE_*}` placeholders in `s`. Unknown `${...}`
/// tokens are left verbatim and logged at debug.
pub fn resolve_vars(s: &str, ctx: &PluginPathCtx) -> String {
    if !s.contains("${") {
        return s.to_string();
    }
    if s.contains("${CLAUDE_PLUGIN_DATA}") {
        ctx.ensure_data_dir();
    }
    let out = s
        .replace("${CLAUDE_PLUGIN_ROOT}", &ctx.root.to_string_lossy())
        .replace("${CLAUDE_PLUGIN_DATA}", &ctx.data.to_string_lossy())
        .replace("${CLAUDE_PROJECT_DIR}", &ctx.project.to_string_lossy());
    if out.contains("${") {
        tracing::debug!(value = %out, "plugin path var-subst: unresolved ${{..}} left literal");
    }
    out
}

/// Resolve every path-bearing field of an MCP server spec in place: the stdio
/// `command` + each arg, SSE/HTTP `url`, and every `env` value.
pub fn substitute_spec(spec: &mut McpServerSpec, ctx: &PluginPathCtx) {
    match &mut spec.transport {
        McpTransport::Stdio { command, args } => {
            *command = resolve_vars(command, ctx);
            for a in args.iter_mut() {
                *a = resolve_vars(a, ctx);
            }
        }
        McpTransport::Sse { url } | McpTransport::Http { url } => {
            *url = resolve_vars(url, ctx);
        }
    }
    for v in spec.env.values_mut() {
        *v = resolve_vars(v, ctx);
    }
}

/// Sanitize a plugin name for use as a single on-disk directory component.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ctx() -> PluginPathCtx {
        PluginPathCtx {
            root: PathBuf::from("/install/dir"),
            data: PathBuf::from("/data/dir"),
            project: PathBuf::from("/project"),
        }
    }

    #[test]
    fn resolves_known_vars() {
        let c = ctx();
        assert_eq!(
            resolve_vars("${CLAUDE_PLUGIN_ROOT}/srv", &c),
            "/install/dir/srv"
        );
        assert_eq!(resolve_vars("${CLAUDE_PROJECT_DIR}/x", &c), "/project/x");
    }

    #[test]
    fn unknown_var_left_literal() {
        let c = ctx();
        // No known marker, unknown placeholder preserved verbatim.
        assert_eq!(resolve_vars("${FOO_BAR}/x", &c), "${FOO_BAR}/x");
    }

    #[test]
    fn no_placeholder_is_passthrough() {
        let c = ctx();
        assert_eq!(resolve_vars("/plain/path", &c), "/plain/path");
    }

    #[test]
    fn substitutes_stdio_command_args_and_env() {
        let mut spec = McpServerSpec {
            name: "db".into(),
            transport: McpTransport::Stdio {
                command: "${CLAUDE_PLUGIN_ROOT}/bin/server".into(),
                args: vec!["--root".into(), "${CLAUDE_PLUGIN_ROOT}".into()],
            },
            env: HashMap::from([("CFG".to_string(), "${CLAUDE_PROJECT_DIR}/c".to_string())]),
        };
        substitute_spec(&mut spec, &ctx());
        match &spec.transport {
            McpTransport::Stdio { command, args } => {
                assert_eq!(command, "/install/dir/bin/server");
                assert_eq!(
                    args,
                    &vec!["--root".to_string(), "/install/dir".to_string()]
                );
            }
            _ => panic!("expected stdio"),
        }
        assert_eq!(spec.env.get("CFG").map(String::as_str), Some("/project/c"));
    }

    #[test]
    fn substitutes_http_url() {
        let mut spec = McpServerSpec {
            name: "remote".into(),
            transport: McpTransport::Http {
                url: "${CLAUDE_PROJECT_DIR}/sock".into(),
            },
            env: HashMap::new(),
        };
        substitute_spec(&mut spec, &ctx());
        match &spec.transport {
            McpTransport::Http { url } => assert_eq!(url, "/project/sock"),
            _ => panic!("expected http"),
        }
    }
}
