//! v0.6.4 Task 2.6 — `swarm` subcommand wiring the `wcore-swarm` crate
//! into the user-facing CLI surface.
//!
//! Usage:
//!   wayland-core swarm --workers <N> --worker-command "<cmd>" \
//!                      [--repo <path>] [--brief <path>] \
//!                      [--base-branch <name>] [--branch-prefix <name>] \
//!                      [--timeout <humantime>] [--task <label>]
//!
//! Behaviour:
//!   - Builds a `SwarmBrief` either from `--brief <toml>` (full file) or
//!     from the individual CLI flags (`--worker-command`, …).
//!   - Constructs a `Swarm` rooted at `--repo` (defaults to CWD).
//!   - Dispatches `--workers` workers, collects results, prints the
//!     `Vec<SwarmResult>` as pretty JSON to stdout, then cleans up.

use std::path::PathBuf;
use std::time::Duration;

use clap::Args;
use clap::builder::RangedU64ValueParser;
use serde::Deserialize;
use wcore_swarm::{Swarm, SwarmBrief};

#[derive(Args, Debug)]
pub struct SwarmArgs {
    /// Number of parallel workers to dispatch (minimum 1).
    // F-071: value_parser rejects 0 at the clap level so the user gets
    // a clear "1..=max" range error before any work is attempted.
    #[arg(long, value_parser = RangedU64ValueParser::<usize>::new().range(1..))]
    pub workers: usize,

    /// argv-style worker command. Split on ASCII whitespace; the first
    /// token is the program (resolved against PATH), the rest are
    /// arguments. No shell interpretation. Ignored when `--brief` is set.
    #[arg(long)]
    pub worker_command: Option<String>,

    /// Repository root the swarm operates on. Defaults to CWD.
    #[arg(long)]
    pub repo: Option<PathBuf>,

    /// Optional TOML brief overriding ALL individual flags. Shape matches
    /// `SwarmBrief` (task, base_branch, worker_branch_prefix,
    /// worker_command, timeout, env).
    #[arg(long)]
    pub brief: Option<PathBuf>,

    /// Free-form telemetry label. Default: "cli-swarm".
    #[arg(long, default_value = "cli-swarm")]
    pub task: String,

    /// Branch each worker worktree is created from. Default: "main".
    #[arg(long, default_value = "main")]
    pub base_branch: String,

    /// Branch prefix for each worker. Default: "swarm/cli".
    #[arg(long, default_value = "swarm/cli")]
    pub branch_prefix: String,

    /// Per-worker wall-clock timeout (humantime, e.g. "30s", "5m").
    /// Default: 1h.
    #[arg(long, default_value = "1h")]
    pub timeout: String,
}

/// TOML shape for `--brief`. Mirrors `SwarmBrief` with humantime-friendly
/// `timeout` (the `wcore-swarm` `SwarmBrief` already uses humantime_serde
/// so plain `toml::from_str` Just Works once we delegate to it).
#[derive(Debug, Deserialize)]
struct BriefFile {
    task: String,
    base_branch: String,
    worker_branch_prefix: String,
    worker_command: Vec<String>,
    /// humantime — parsed via humantime_serde wrapper below.
    #[serde(with = "humantime_serde")]
    timeout: Duration,
    #[serde(default)]
    env: Vec<(String, String)>,
}

impl From<BriefFile> for SwarmBrief {
    fn from(f: BriefFile) -> Self {
        SwarmBrief {
            task: f.task,
            base_branch: f.base_branch,
            worker_branch_prefix: f.worker_branch_prefix,
            worker_command: f.worker_command,
            timeout: f.timeout,
            env: f.env,
        }
    }
}

/// Build a `SwarmBrief` from the parsed CLI args. Separated from `run`
/// so the argv-to-brief mapping has a unit test (no async/git/tokio).
pub fn build_brief(args: &SwarmArgs) -> anyhow::Result<SwarmBrief> {
    if let Some(brief_path) = &args.brief {
        let text = std::fs::read_to_string(brief_path).map_err(|e| {
            anyhow::anyhow!("failed to read --brief '{}': {e}", brief_path.display())
        })?;
        let parsed: BriefFile = toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("failed to parse --brief TOML: {e}"))?;
        return Ok(parsed.into());
    }

    let cmd_str = args
        .worker_command
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("either --worker-command or --brief is required"))?;
    let worker_command: Vec<String> = cmd_str
        .split_ascii_whitespace()
        .map(|s| s.to_string())
        .collect();
    if worker_command.is_empty() {
        anyhow::bail!("--worker-command is empty after whitespace split");
    }

    let timeout = humantime::parse_duration(&args.timeout)
        .map_err(|e| anyhow::anyhow!("invalid --timeout '{}': {e}", args.timeout))?;

    Ok(SwarmBrief {
        task: args.task.clone(),
        base_branch: args.base_branch.clone(),
        worker_branch_prefix: args.branch_prefix.clone(),
        worker_command,
        timeout,
        env: vec![],
    })
}

/// Async entrypoint. Dispatches `args.workers` workers using the brief
/// built from the args, collects results, prints them as pretty JSON,
/// then cleans up.
pub async fn run(args: SwarmArgs) -> anyhow::Result<()> {
    let brief = build_brief(&args)?;
    let repo = match args.repo {
        Some(p) => p,
        None => std::env::current_dir()?,
    };

    let swarm = Swarm::new(&repo)
        .map_err(|e| anyhow::anyhow!("Swarm::new failed for {}: {e}", repo.display()))?;
    let handles = swarm
        .dispatch(brief, args.workers)
        .await
        .map_err(|e| anyhow::anyhow!("swarm dispatch failed: {e}"))?;
    let results = swarm
        .collect(handles)
        .await
        .map_err(|e| anyhow::anyhow!("swarm collect failed: {e}"))?;
    // Wrap in a top-level object so the smoke test can grep for "workers".
    let envelope = serde_json::json!({ "workers": results });
    println!("{}", serde_json::to_string_pretty(&envelope)?);
    swarm
        .cleanup()
        .await
        .map_err(|e| anyhow::anyhow!("swarm cleanup failed: {e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(workers: usize, cmd: &str) -> SwarmArgs {
        SwarmArgs {
            workers,
            worker_command: Some(cmd.to_string()),
            repo: None,
            brief: None,
            task: "cli-swarm".into(),
            base_branch: "main".into(),
            branch_prefix: "swarm/cli".into(),
            timeout: "30s".into(),
        }
    }

    #[test]
    fn build_brief_from_flags_splits_command_whitespace() {
        let a = args(2, "echo hello world");
        let b = build_brief(&a).unwrap();
        assert_eq!(b.worker_command, vec!["echo", "hello", "world"]);
        assert_eq!(b.base_branch, "main");
        assert_eq!(b.worker_branch_prefix, "swarm/cli");
        assert_eq!(b.task, "cli-swarm");
        assert_eq!(b.timeout, Duration::from_secs(30));
        assert!(b.env.is_empty());
    }

    #[test]
    fn build_brief_rejects_missing_command_and_brief() {
        let mut a = args(1, "");
        a.worker_command = None;
        let err = build_brief(&a).unwrap_err().to_string();
        assert!(err.contains("--worker-command or --brief"), "got: {err}");
    }

    #[test]
    fn build_brief_rejects_whitespace_only_command() {
        let a = args(1, "   ");
        let err = build_brief(&a).unwrap_err().to_string();
        assert!(err.contains("empty after whitespace split"), "got: {err}");
    }

    #[test]
    fn build_brief_rejects_bad_timeout() {
        let mut a = args(1, "true");
        a.timeout = "not-a-duration".into();
        let err = build_brief(&a).unwrap_err().to_string();
        assert!(err.contains("invalid --timeout"), "got: {err}");
    }

    #[test]
    fn build_brief_from_toml_file_overrides_flags() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("brief.toml");
        std::fs::write(
            &path,
            r#"
task = "from-file"
base_branch = "develop"
worker_branch_prefix = "swarm/file"
worker_command = ["bash", "-c", "echo from-file"]
timeout = "5m"
env = [["FOO", "bar"]]
"#,
        )
        .unwrap();
        let mut a = args(3, "ignored");
        a.brief = Some(path);
        let b = build_brief(&a).unwrap();
        assert_eq!(b.task, "from-file");
        assert_eq!(b.base_branch, "develop");
        assert_eq!(b.worker_branch_prefix, "swarm/file");
        assert_eq!(b.worker_command, vec!["bash", "-c", "echo from-file"]);
        assert_eq!(b.timeout, Duration::from_secs(300));
        assert_eq!(b.env, vec![("FOO".into(), "bar".into())]);
    }
}
