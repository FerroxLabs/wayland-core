//! D4 — cross-session keystone harness.
//!
//! Every persona run (see [`crate::runner::run`]) gets a throwaway tempdir with
//! `WAYLAND_HOME` *stripped*, so nothing survives between runs — correct for
//! "does one task work" but useless for "does the agent REMEMBER across
//! sessions." The cross-session keystones (memory recall, skill learning) need
//! a home that persists across two separate `wayland-core` processes.
//!
//! ## The substrate
//! `WAYLAND_HOME` → `wcore_config::config::wayland_config_dir()` drives BOTH:
//! - the memory DBs (`wcore-memory` resolves its base dir from
//!   `app_config_dir()` = `$WAYLAND_HOME`; the **global** tier — `memory.db` —
//!   carries across sessions), and
//! - the skills dir (`$WAYLAND_HOME/skills{,/auto}` — where the auto-drafter
//!   writes learned skills).
//!
//! It does NOT reroute the session dir (cross-audit C-3); that's the config's
//! `[session].directory`. So we keep two dirs under one held `TempDir`:
//! - **`home/`** — `WAYLAND_HOME`. Memory + skills live here, deliberately
//!   OUTSIDE the project watch root, so the file-watcher never sees the engine's
//!   own `memory.db`/`sessions/*.wal` churn as "user edits."
//! - **`project/`** — the spawn `cwd` (a fresh "user project" + the watch root).
//!   Holds the seeded `.wayland-core/config.toml`.
//!
//! The `TempDir` is held for the lifetime of the [`CrossSessionEnv`], so both
//! child processes share it; it's still hermetic (cleaned up on drop) — the
//! "persistence" we need is only across the two sessions WITHIN one test run.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;

use crate::providers::ProviderConfig;
use crate::runner::{ScenarioResult, discover_binary, run_session_in};
use crate::scenario::Scenario;
use crate::tempenv::escape_toml_basic;

/// A persistent (for the test's lifetime) home + project pair, seeded with a
/// config that turns memory ON so facts/skills survive across sessions.
pub struct CrossSessionEnv {
    /// Held so the underlying tempdir isn't reaped between sessions.
    _dir: TempDir,
    home: PathBuf,
    project: PathBuf,
}

impl CrossSessionEnv {
    /// Build the env: create `home/` + `home/sessions/` + `project/.wayland-core/`
    /// and seed `project/.wayland-core/config.toml`.
    ///
    /// The seeded config differs from the persona [`crate::tempenv`] config in
    /// three load-bearing ways:
    /// - `[memory] enabled = true` — bootstrap builds a real `Memory` instead of
    ///   `NullMemory` (default is `false`, so persona runs have no memory at all).
    /// - `[memory] dream_cycle_throttle_secs = 0` — the dream cycle consolidates
    ///   session facts at session-end. Its default 1800s throttle would skip
    ///   consolidation between two back-to-back sessions, so recall could never
    ///   work even in principle; 0 lets session 1's facts consolidate before
    ///   session 2 boots.
    /// - `[observability] skills_lifecycle = true` — keeps the auto-draft
    ///   pipeline on (it already defaults on, but we pin it for the keystone).
    pub fn build(provider: &ProviderConfig) -> anyhow::Result<Self> {
        let dir = TempDir::new()?;
        let root = dir.path();

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            // The privileged evaluator retains ownership of the shared root,
            // while the unprivileged candidate must traverse it to reach the
            // separately owned project and home directories.
            fs::set_permissions(root, fs::Permissions::from_mode(0o711))?;
        }

        let home = root.join("home");
        let project = root.join("project");
        let sessions_dir = home.join("sessions");
        let cfg_dir = project.join(".wayland-core");
        fs::create_dir_all(&sessions_dir)?;
        fs::create_dir_all(&cfg_dir)?;

        let session_dir_abs = sessions_dir.to_string_lossy().to_string();
        let provider_id = provider.id.cli_name();

        let mut toml = String::new();
        toml.push_str("# wcore-eval-scenarios — D4 cross-session config\n");
        toml.push_str("# Persistent home shared across two sessions; memory ON.\n\n");

        toml.push_str("[session]\n");
        toml.push_str(&format!(
            "directory = \"{}\"\n\n",
            escape_toml_basic(&session_dir_abs)
        ));

        toml.push_str("[memory]\n");
        toml.push_str("enabled = true\n");
        // Fire the dream cycle at session-end (no throttle) so session 1's
        // facts are consolidated before session 2 reads them.
        toml.push_str("dream_cycle_throttle_secs = 0\n\n");

        toml.push_str("[observability]\n");
        toml.push_str("skills_lifecycle = true\n\n");

        toml.push_str(&format!("[provider.{provider_id}]\n"));
        toml.push_str(&format!(
            "model = \"{}\"\n\n",
            escape_toml_basic(&provider.model)
        ));

        fs::write(cfg_dir.join("config.toml"), toml)?;

        Ok(Self {
            _dir: dir,
            home,
            project,
        })
    }

    /// `WAYLAND_HOME` for both sessions — the shared memory + skills root.
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// The spawn `cwd` (project + watch root) for both sessions.
    pub fn project(&self) -> &Path {
        &self.project
    }
}

/// Drive a sequence of scenarios as SEPARATE `wayland-core` processes that all
/// share ONE persistent home — the cross-session keystone.
///
/// Each scenario boots a fresh engine (so session 2 genuinely cold-starts and
/// must *recall* rather than carry in-memory state), but every process gets the
/// same `WAYLAND_HOME` + `cwd`, so memory DBs and the skills dir carry over.
/// Returns one [`ScenarioResult`] per input scenario, in order, so the caller
/// can assert on each session independently (e.g. session 2's recall).
///
/// Inter-session pacing (default 15s, override `WCORE_EVAL_PACING_SECS`) guards
/// against the DeepSeek server-side burst-throttle that drops a fresh process's
/// first request when it lands too soon after the previous session's tail.
pub async fn run_cross_session(
    sessions: &[Scenario],
    provider: &ProviderConfig,
) -> anyhow::Result<Vec<ScenarioResult>> {
    let env = CrossSessionEnv::build(provider)?;
    let bin = discover_binary().map_err(|e| anyhow::anyhow!(e.to_string()))?;

    let pacing_secs = std::env::var("WCORE_EVAL_PACING_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(15);

    let mut results = Vec::with_capacity(sessions.len());
    for (idx, scenario) in sessions.iter().enumerate() {
        if idx > 0 && pacing_secs > 0 {
            tokio::time::sleep(Duration::from_secs(pacing_secs)).await;
        }
        let result =
            run_session_in(scenario, provider, &bin, env.project(), Some(env.home())).await?;
        results.push(result);
    }
    Ok(results)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{ProviderConfig, ProviderId};

    #[test]
    fn seeded_config_enables_memory_with_no_dream_throttle() {
        let p = ProviderConfig::new(ProviderId::DeepSeek, "deepseek-v4-pro")
            .with_api_key("cross-session-secret-canary");
        let env = CrossSessionEnv::build(&p).expect("build env");
        let cfg = fs::read_to_string(env.project().join(".wayland-core/config.toml"))
            .expect("seeded config exists");
        assert!(
            !cfg.contains("cross-session-secret-canary") && !cfg.contains("api_key"),
            "cross-session config must not persist credentials: {cfg}"
        );
        assert!(cfg.contains("[memory]"), "must declare [memory]: {cfg}");
        assert!(cfg.contains("enabled = true"), "memory must be on: {cfg}");
        assert!(
            cfg.contains("dream_cycle_throttle_secs = 0"),
            "dream cycle must be un-throttled so facts consolidate between \
             sessions: {cfg}"
        );
        assert!(
            cfg.contains("skills_lifecycle = true"),
            "skills lifecycle must be on for the skill-learning keystone: {cfg}"
        );
    }

    #[test]
    fn session_dir_lives_under_home_not_project() {
        // The session store must sit under WAYLAND_HOME (shared, and OUTSIDE
        // the project watch root) — not under the project cwd, or the watcher
        // would see every session save as a phantom user edit.
        let p = ProviderConfig::new(ProviderId::DeepSeek, "deepseek-v4-pro").with_api_key("k");
        let env = CrossSessionEnv::build(&p).expect("build env");
        // The seeded session-dir assertion uses Unix path semantics, so the
        // config read lives inside the cfg(unix) block — otherwise `cfg` is an
        // unused binding on Windows (clippy `-D warnings`).
        #[cfg(unix)]
        {
            let cfg = fs::read_to_string(env.project().join(".wayland-core/config.toml"))
                .expect("seeded config exists");
            let needle = format!("directory = \"{}", env.home().join("sessions").display());
            assert!(
                cfg.contains(&needle),
                "session dir must be <home>/sessions; got:\n{cfg}"
            );
        }
        assert!(
            !env.project().starts_with(env.home()) && !env.home().starts_with(env.project()),
            "home and project must be disjoint dirs"
        );
    }

    #[test]
    fn home_and_project_are_distinct() {
        let p = ProviderConfig::new(ProviderId::Anthropic, "claude-sonnet-4-6").with_api_key("k");
        let env = CrossSessionEnv::build(&p).expect("build env");
        assert_ne!(env.home(), env.project());
        assert!(env.home().is_dir());
        assert!(env.project().join(".wayland-core").is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let root_mode = fs::metadata(env._dir.path())
                .expect("read shared root metadata")
                .permissions()
                .mode();
            assert_eq!(root_mode & 0o007, 0o001);
        }
    }
}
