//! Hermetic per-scenario tempdir + seeded `config.toml`.
//!
//! Cross-audit C-3 caught the trap: `WAYLAND_HOME` does NOT reroute
//! the session directory. `config.session.directory` defaults to the
//! relative string `".wayland-core/sessions"` (`wcore-config/src/config.rs:482-484`).
//! Setting env vars without also `cd`-ing or pointing the config at an
//! absolute path leaks session files into the worktree.
//!
//! [`build`] therefore:
//! 1. Mints a `TempDir`.
//! 2. Writes `<tempdir>/.wayland-core/config.toml` with an **absolute**
//!    `[session].directory = "<tempdir>/sessions"`.
//! 3. Writes `[provider.<id>] api_key = "..."` so the binary picks up
//!    the key from config rather than env (more deterministic).
//! 4. Optionally writes `[budget] max_cost_usd = X` for scenarios
//!    that exercise the budget cap (per H-6 — env vars don't work).
//!
//! The runner then spawns the binary with `cwd = tempdir`; the engine
//! reads its config via its normal cwd-walk and lands inside this
//! sandbox.

use std::fs;
use std::path::{Path, PathBuf};

use tempfile::TempDir;

use crate::providers::ProviderConfig;

/// Owns the [`TempDir`] for one scenario run. Dropping this struct
/// removes the tempdir; the runner holds it for the lifetime of the
/// child process.
pub struct TempEnv {
    dir: TempDir,
    /// Cached for ergonomics — same as `dir.path().join("sessions")`.
    sessions_dir: PathBuf,
}

/// Options for [`build`]. Construct via `Default::default()` then set
/// what you need; T2 only uses the budget knob (for the future S24
/// scenario), but the struct is the extension seam for T6-T8.
#[derive(Debug, Clone, Default)]
pub struct TempEnvOptions {
    /// If set, writes `[budget] max_cost_usd = X` into the seeded
    /// `config.toml` so the engine halts at that cap (H-6).
    pub budget_max_cost_usd: Option<f64>,
}

/// Build a fresh hermetic env for one run.
///
/// The seeded config picks up the API key from `provider.resolved_key()`
/// — if the caller has no key resolved, an empty string is written and
/// the spawned binary will surface a clear "missing api key" error
/// rather than 401-ing against a real provider with a placeholder. T4's
/// `--strict` mode catches this earlier (SKIP vs FAIL).
pub fn build(provider: &ProviderConfig) -> anyhow::Result<TempEnv> {
    build_with(provider, &TempEnvOptions::default())
}

pub fn build_with(provider: &ProviderConfig, opts: &TempEnvOptions) -> anyhow::Result<TempEnv> {
    let dir = TempDir::new()?;
    let root = dir.path();
    let sessions_dir = root.join("sessions");
    fs::create_dir_all(&sessions_dir)?;

    let cfg_dir = root.join(".wayland-core");
    fs::create_dir_all(&cfg_dir)?;

    // Build the TOML by hand — `toml::to_string` of a `serde_json::Value`
    // would re-quote keys awkwardly, and we want the file to read cleanly
    // when a debugger opens it. The provider key MUST be quoted (TOML
    // basic string), and the session dir is absolute (per C-3).
    let session_dir_abs = sessions_dir.to_string_lossy().to_string();
    let provider_id = provider.id.cli_name();
    let api_key = provider.resolved_key().unwrap_or_default();

    let mut toml = String::new();
    toml.push_str("# wcore-eval-scenarios — seeded per-scenario config\n");
    toml.push_str("# Generated; absolute session dir per cross-audit C-3.\n\n");

    toml.push_str("[session]\n");
    toml.push_str(&format!(
        "directory = \"{}\"\n\n",
        escape_toml_basic(&session_dir_abs)
    ));

    // Egress: keep the security gate ENFORCING but allowlist the specific hosts
    // the tool-coverage probes legitimately read from. The enforcing policy
    // blocks/gates a fetch to any non-allowlisted host, and the headless
    // json-stream eval can't grant interactive consent, so e.g. WebFetch to
    // `example.com` otherwise fails (surfaced as an opaque ~30s timeout). Rather
    // than the blunt `[security] enabled = false` off-switch — which would also
    // drop the Exfil hard-deny and could mask a future SSRF/exfil regression if
    // an egress-denial scenario is ever added to the sweep — we add only the
    // exact test host. The gate (incl. exfil-shaped POST denial) stays ON, which
    // also keeps the eval closer to the real shipped posture. `web_search`
    // (DuckDuckGo) is already covered by the built-in first-party allowlist.
    toml.push_str("[security]\n");
    toml.push_str("egress_allow = [\"example.com\"]\n\n");

    toml.push_str(&format!("[provider.{provider_id}]\n"));
    toml.push_str(&format!("api_key = \"{}\"\n", escape_toml_basic(&api_key)));
    toml.push_str(&format!(
        "model = \"{}\"\n\n",
        escape_toml_basic(&provider.model)
    ));

    if let Some(cap) = opts.budget_max_cost_usd {
        toml.push_str("[budget]\n");
        toml.push_str(&format!("max_cost_usd = {cap}\n\n"));
    }

    fs::write(cfg_dir.join("config.toml"), toml)?;

    Ok(TempEnv { dir, sessions_dir })
}

impl TempEnv {
    /// Root of the hermetic dir — the runner uses this as the spawn
    /// `current_dir`.
    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Absolute session directory — the engine writes
    /// `<sessions_dir>/<date>_<id>.json` here after each session. The
    /// runner re-reads files from this directory post-run for the T3
    /// trace cross-check.
    pub fn sessions_dir(&self) -> &Path {
        &self.sessions_dir
    }
}

/// Minimal TOML basic-string escaper — handles backslash + quote +
/// newline. Sufficient for filesystem paths and API keys (which never
/// contain control characters in practice). We deliberately do NOT
/// pull in `toml::Value` for this — the round-trip would normalize key
/// ordering, breaking the readable layout above.
pub(crate) fn escape_toml_basic(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ProviderConfig, ProviderId};

    #[test]
    fn seeded_config_has_absolute_session_dir() {
        let p = ProviderConfig::new(ProviderId::DeepSeek, "deepseek-chat").with_api_key("test-key");
        let env = build(&p).expect("build env");
        let cfg = fs::read_to_string(env.path().join(".wayland-core/config.toml"))
            .expect("seeded config exists");
        assert!(
            cfg.contains("[session]"),
            "config should declare a [session] block: {cfg}"
        );
        // Absolute path per C-3 — must start with `/` on unix or a
        // drive letter on windows. We only check unix here since the
        // worktree runs on macOS; CI matrix can extend.
        #[cfg(unix)]
        {
            let needle = format!("directory = \"{}", env.sessions_dir().display());
            assert!(
                cfg.contains(&needle),
                "session.directory must be absolute & equal to env.sessions_dir; got:\n{cfg}"
            );
        }
    }

    #[test]
    fn seeded_config_includes_provider_api_key() {
        let p = ProviderConfig::new(ProviderId::Anthropic, "claude-sonnet-4-6")
            .with_api_key("sk-ant-test-12345");
        let env = build(&p).expect("build env");
        let cfg = fs::read_to_string(env.path().join(".wayland-core/config.toml"))
            .expect("seeded config exists");
        assert!(cfg.contains("[provider.anthropic]"), "config: {cfg}");
        assert!(cfg.contains("sk-ant-test-12345"), "config: {cfg}");
    }

    #[test]
    fn optional_budget_block_appears_when_set() {
        let p = ProviderConfig::new(ProviderId::DeepSeek, "deepseek-chat").with_api_key("test-key");
        let env = build_with(
            &p,
            &TempEnvOptions {
                budget_max_cost_usd: Some(0.05),
            },
        )
        .expect("build env");
        let cfg = fs::read_to_string(env.path().join(".wayland-core/config.toml"))
            .expect("seeded config exists");
        assert!(cfg.contains("[budget]"), "config: {cfg}");
        assert!(cfg.contains("max_cost_usd = 0.05"), "config: {cfg}");
    }

    #[test]
    fn escape_quotes_and_backslashes() {
        assert_eq!(escape_toml_basic(r#"a"b\c"#), r#"a\"b\\c"#);
    }
}
