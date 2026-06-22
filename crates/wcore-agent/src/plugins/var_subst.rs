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
    /// `<profile_home>/plugins/data/<sanitized-plugin-name>` so it is
    /// sandboxed by `WAYLAND_HOME`. On first access a one-time best-effort
    /// migration copies the pre-isolation
    /// `<data_dir>/wayland/plugins/data/<name>` tree here — but ONLY when
    /// `WAYLAND_HOME` is unset (an isolated profile must not inherit
    /// another profile's live plugin state / secrets).
    pub fn for_plugin(install_dir: &Path, plugin_name: &str, project: &Path) -> Self {
        let safe_name = sanitize(plugin_name);
        let data = wcore_config::config::profile_home()
            .join("plugins")
            .join("data")
            .join(&safe_name);
        migrate_legacy_plugin_data(&safe_name, &data);
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

/// One-time best-effort migration of a plugin's pre-isolation data dir
/// (`<data_dir>/wayland/plugins/data/<name>`) into the `WAYLAND_HOME`-rooted
/// location.
///
/// Gated + idempotent + atomic:
///   * **Gate:** if `WAYLAND_HOME` is set, return — an isolated profile must
///     NOT inherit another profile's live plugin state (credential caches,
///     tokens, per-plugin DBs);
///   * skip once `new_dir` exists (means *fully* migrated — atomic publish
///     below, so this is never a torn partial);
///   * copy the legacy tree into a temp sibling dir, then atomic-`rename` it
///     onto `new_dir`.
///
/// Failures log at `warn` and never propagate — losing plugin data degrades
/// gracefully (the plugin re-initializes); it must never crash boot.
fn migrate_legacy_plugin_data(safe_name: &str, new_dir: &Path) {
    // Explicit-isolation profiles never inherit shared legacy data.
    if std::env::var_os("WAYLAND_HOME").is_some() {
        return;
    }
    if new_dir.exists() {
        return;
    }
    let Some(legacy) = dirs::data_dir().map(|d| {
        d.join("wayland")
            .join("plugins")
            .join("data")
            .join(safe_name)
    }) else {
        return;
    };
    if legacy == new_dir || !legacy.exists() {
        return;
    }
    let Some(parent) = new_dir.parent() else {
        return;
    };
    if let Err(e) = std::fs::create_dir_all(parent) {
        tracing::warn!(error = %e, path = %new_dir.display(),
            "plugin data: failed to create parent for legacy migration");
        return;
    }
    // Stage into a temp sibling, then atomic-rename so `new_dir.exists()`
    // is a "fully migrated" signal, never "started migrating".
    let staging = parent.join(format!(".{safe_name}.migrating"));
    let _ = std::fs::remove_dir_all(&staging); // clear any prior crash debris
    if let Err(e) = copy_dir_recursive(&legacy, &staging) {
        tracing::warn!(error = %e, from = %legacy.display(), to = %staging.display(),
            "plugin data: legacy copy failed (plugin will re-init)");
        let _ = std::fs::remove_dir_all(&staging);
        return;
    }
    if let Err(e) = std::fs::rename(&staging, new_dir) {
        // A concurrent migrator may have just published new_dir → discard.
        tracing::warn!(error = %e, to = %new_dir.display(),
            "plugin data: publish rename failed (plugin will re-init)");
        let _ = std::fs::remove_dir_all(&staging);
    }
}

/// Recursively copy `src` → `dst`, creating `dst` and all parents.
fn copy_dir_recursive(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&from, &to)?;
        } else {
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    #[serial_test::serial]
    fn plugin_data_roots_under_wayland_home() {
        let tmp = tempfile::tempdir().unwrap();
        let prev = std::env::var_os("WAYLAND_HOME");
        unsafe { std::env::set_var("WAYLAND_HOME", tmp.path()) };
        let ctx = PluginPathCtx::for_plugin(Path::new("/install"), "my-plugin", Path::new("/proj"));
        match prev {
            Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        }
        assert_eq!(
            ctx.data,
            tmp.path().join("plugins").join("data").join("my-plugin")
        );
    }

    #[test]
    fn copy_dir_recursive_copies_nested_tree() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        let dst = tmp.path().join("dst");
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("a.txt"), b"A").unwrap();
        std::fs::write(src.join("sub").join("b.txt"), b"B").unwrap();
        super::copy_dir_recursive(&src, &dst).unwrap();
        assert_eq!(std::fs::read(dst.join("a.txt")).unwrap(), b"A");
        assert_eq!(std::fs::read(dst.join("sub").join("b.txt")).unwrap(), b"B");
    }

    #[test]
    #[serial_test::serial]
    fn migrate_skipped_when_wayland_home_set() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("plugins").join("data").join("p");
        let prev = std::env::var_os("WAYLAND_HOME");
        unsafe { std::env::set_var("WAYLAND_HOME", tmp.path()) };
        super::migrate_legacy_plugin_data("p", &new_dir);
        match prev {
            Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        }
        assert!(!new_dir.exists());
    }

    #[test]
    #[serial_test::serial]
    fn migrate_noop_when_new_dir_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let new_dir = tmp.path().join("plugins").join("data").join("p");
        std::fs::create_dir_all(&new_dir).unwrap();
        std::fs::write(new_dir.join("keep.txt"), b"keep").unwrap();
        let prev = std::env::var_os("WAYLAND_HOME");
        unsafe { std::env::remove_var("WAYLAND_HOME") };
        super::migrate_legacy_plugin_data("p", &new_dir);
        if let Some(v) = prev {
            unsafe { std::env::set_var("WAYLAND_HOME", v) }
        }
        assert_eq!(std::fs::read(new_dir.join("keep.txt")).unwrap(), b"keep");
    }

    /// Trust-boundary guard: `wcore-plugin-api` is build-forbidden from
    /// importing `wcore-config`, so `default_plugin_root()` hand-mirrors
    /// `profile_home()`. This pins the mirror to the canonical resolver.
    #[test]
    #[serial_test::serial]
    fn default_plugin_root_matches_canonical_resolver() {
        use wcore_plugin_api::manifest::PluginIdentity;
        let cases: [Option<&str>; 3] = [None, Some("/tmp/wh-equality-x"), Some("bad\nvalue")];
        for case in cases {
            let prev = std::env::var_os("WAYLAND_HOME");
            match case {
                Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
                None => unsafe { std::env::remove_var("WAYLAND_HOME") },
            }
            let mirror = PluginIdentity::default_plugin_root();
            let canonical = wcore_config::config::profile_home().join("plugins");
            match prev {
                Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
                None => unsafe { std::env::remove_var("WAYLAND_HOME") },
            }
            assert_eq!(
                mirror, canonical,
                "default_plugin_root() drifted from profile_home()/plugins for WAYLAND_HOME={case:?}"
            );
        }
    }

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
