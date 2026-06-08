//! v0.8.1 U7 + W6-K-rest: `wayland-core cron` subcommands.
//!
//! CRUD ops (add / list / remove / enable / disable) plus diagnostic
//! ops (status / history / logs) against the `wcore-cron` store, and a
//! `daemon` subcommand that spawns the runner detached.
//!
//! Store path: `$WAYLAND_HOME/cron/jobs.json`, falling back to
//! `~/.wayland/cron/jobs.json`. History: same dir, `history.jsonl`.
//!
//! The runner picks up changes on its next tick — there's no in-band
//! `reload()` signal because the file store re-reads from disk every
//! list call.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Subcommand;
use wcore_cron::{CronFireOutcome, CronFireRecord, CronJob, CronStore, FileCronStore, Target};

#[derive(Subcommand, Debug)]
pub enum CronCmd {
    /// List every persisted cron job.
    List,

    /// Add a new job. Exactly one of `--slash`, `--channel`, or
    /// `--skill` must be provided (with matching companion flags).
    ///
    /// Examples:
    ///   wayland-core cron add "0 9 * * *" --skill hello
    ///   wayland-core cron add "*/15 * * * *" --slash "/status"
    ///   wayland-core cron add "0 8 * * 1" --channel team --text "Good morning"
    // F-075: examples surface in `cron add --help` so users can copy-paste
    // a working invocation without reading the full man page.
    Add {
        /// Cron expression (5-field crontab shape or 6-field
        /// `cron`-crate shape, e.g. "0 9 * * *" = daily at 09:00).
        expression: String,

        /// Slash-command target: run the given command on fire.
        #[arg(long, conflicts_with_all = ["channel", "skill"], value_name = "COMMAND")]
        slash: Option<String>,

        /// Channel-message target: send `--text` to the named channel.
        #[arg(long, conflicts_with_all = ["slash", "skill"], value_name = "NAME")]
        channel: Option<String>,

        /// Text body for `--channel`. Required when `--channel` is set.
        #[arg(long, requires = "channel", value_name = "TEXT")]
        text: Option<String>,

        /// Skill-invocation target: invoke the named skill on fire.
        #[arg(long, conflicts_with_all = ["slash", "channel"], value_name = "NAME")]
        skill: Option<String>,

        /// JSON args for `--skill` (default `{}`).
        #[arg(long, requires = "skill", value_name = "JSON")]
        args: Option<String>,
    },

    /// Remove a job by id.
    Remove {
        /// UUID returned by `cron list` / `cron add`.
        id: String,
    },

    /// Enable a job by id.
    Enable {
        /// UUID returned by `cron list`.
        id: String,
    },

    /// Disable a job by id (kept on disk, skipped by the runner).
    Disable {
        /// UUID returned by `cron list`.
        id: String,
    },

    /// Print full details for one job: id, expression, target, state,
    /// created_at, last_fired, and the outcome of the most recent fire
    /// attempt (success + duration, or error message).
    ///
    /// Example:
    ///   wayland-core cron status <id>
    Status {
        /// UUID returned by `cron list` / `cron add`.
        id: String,
    },

    /// Print the last N fire records for a job (timestamp, outcome,
    /// duration, error message if any). Records come from the JSONL
    /// ring-buffer written by the runner alongside jobs.json.
    ///
    /// Example:
    ///   wayland-core cron history <id> --limit 10
    History {
        /// UUID returned by `cron list`.
        id: String,
        /// Maximum records to show (most-recent first). Default 20.
        #[arg(long, short = 'n', default_value = "20")]
        limit: usize,
    },

    /// Tail recent log lines associated with a job's fires. Currently
    /// surfaces fire records from the history file (same data as
    /// `cron history`) formatted as structured log lines compatible
    /// with the engine's tracing output.
    ///
    /// Example:
    ///   wayland-core cron logs <id> --limit 50
    Logs {
        /// UUID returned by `cron list`.
        id: String,
        /// Maximum records to show (most-recent first). Default 50.
        #[arg(long, short = 'n', default_value = "50")]
        limit: usize,
    },

    /// Spawn the cron runner as a detached background daemon.
    ///
    /// The daemon:
    /// - Writes its PID to `$WAYLAND_HOME/cron-daemon.pid`
    /// - Logs to `$WAYLAND_HOME/cron-daemon.log`
    /// - Honours SIGTERM for clean shutdown
    /// - Persists fire history to `$WAYLAND_HOME/cron/history.jsonl`
    ///
    /// To install as a persistent system service, see the templates under
    /// `templates/cron-daemon/` (launchd.plist / systemd.service).
    Daemon,
}

pub async fn run(cmd: CronCmd) -> Result<()> {
    let store = FileCronStore::from_default_path()
        .context("could not resolve cron store path (no WAYLAND_HOME and no $HOME)")?;
    let history_path = wcore_cron::default_history_path();
    run_inner(cmd, &store, history_path.as_ref()).await
}

/// Test-friendly entry point — accepts an explicit store so tests can
/// drive the same code path against a tempdir.
pub async fn run_with_store(cmd: CronCmd, store: &FileCronStore) -> Result<()> {
    run_inner(cmd, store, None).await
}

async fn run_inner(
    cmd: CronCmd,
    store: &FileCronStore,
    history_path: Option<&PathBuf>,
) -> Result<()> {
    match cmd {
        CronCmd::List => list_cmd(store).await,
        CronCmd::Add {
            expression,
            slash,
            channel,
            text,
            skill,
            args,
        } => add_cmd(expression, slash, channel, text, skill, args, store).await,
        CronCmd::Remove { id } => {
            store.remove(&id).await.context("cron remove failed")?;
            println!("removed {id}");
            Ok(())
        }
        CronCmd::Enable { id } => {
            store
                .set_enabled(&id, true)
                .await
                .context("cron enable failed")?;
            println!("enabled {id}");
            Ok(())
        }
        CronCmd::Disable { id } => {
            store
                .set_enabled(&id, false)
                .await
                .context("cron disable failed")?;
            println!("disabled {id}");
            Ok(())
        }
        CronCmd::Status { id } => status_cmd(&id, store).await,
        CronCmd::History { id, limit } => history_cmd(&id, limit, history_path).await,
        CronCmd::Logs { id, limit } => logs_cmd(&id, limit, history_path).await,
        CronCmd::Daemon => daemon_cmd(store).await,
    }
}

async fn list_cmd(store: &FileCronStore) -> Result<()> {
    let jobs = store.list().await.context("cron list failed")?;
    if jobs.is_empty() {
        println!("(no cron jobs)");
        println!("store: {}", store.path().display());
        return Ok(());
    }
    for job in &jobs {
        let state = if job.enabled { "on " } else { "off" };
        let target = render_target(&job.target);
        // F-064 fix (MED): surface last_fired so users can confirm cron is
        // actually firing. Data was always persisted in jobs.json — just
        // never printed. "never" when the job has not fired yet this session.
        let last_fired = job
            .last_fired
            .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
            .unwrap_or_else(|| "never".to_string());
        println!(
            "{state} {id}  {expr:<20}  {target:<30}  last_fired={last_fired}",
            state = state,
            id = job.id,
            expr = job.expression,
            target = target,
            last_fired = last_fired
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn add_cmd(
    expression: String,
    slash: Option<String>,
    channel: Option<String>,
    text: Option<String>,
    skill: Option<String>,
    args: Option<String>,
    store: &FileCronStore,
) -> Result<()> {
    let target = match (slash, channel, text, skill, args) {
        (Some(cmd), None, None, None, None) => Target::Slash { command: cmd },
        (None, Some(ch), Some(body), None, None) => Target::Channel {
            channel_name: ch,
            text: body,
        },
        (None, Some(_), None, None, None) => {
            bail!("`--channel` requires `--text \"...\"`");
        }
        (None, None, None, Some(name), args_raw) => {
            let args_value = match args_raw {
                Some(raw) => serde_json::from_str(&raw)
                    .with_context(|| format!("`--args` is not valid JSON: {raw}"))?,
                None => serde_json::Value::Object(Default::default()),
            };
            Target::Skill {
                name,
                args: args_value,
            }
        }
        (None, None, None, None, _) => bail!(
            "must provide exactly one target: `--slash <CMD>`, `--channel <NAME> --text <TEXT>`, or `--skill <NAME>`"
        ),
        _ => bail!("`--slash`, `--channel`, and `--skill` are mutually exclusive"),
    };
    let job = CronJob::new(expression, target).context("could not create cron job")?;
    let id = job.id.clone();
    store.insert(job).await.context("cron add failed")?;
    println!("added {id}");
    Ok(())
}

fn render_target(t: &Target) -> String {
    match t {
        Target::Slash { command } => format!("slash    {command}"),
        Target::Channel { channel_name, text } => {
            let preview = preview(text, 40);
            format!("channel  {channel_name} :: {preview}")
        }
        Target::Skill { name, args } => format!("skill    {name} {args}"),
    }
}

fn preview(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max - 1).collect();
        format!("{head}...")
    }
}

/// F-065: `cron status <id>` — print full job details including last_result.
async fn status_cmd(id: &str, store: &FileCronStore) -> Result<()> {
    let jobs = store.list().await.context("cron list failed")?;
    let job = jobs
        .iter()
        .find(|j| j.id == id)
        .with_context(|| format!("job not found: {id}"))?;

    let state = if job.enabled { "enabled" } else { "disabled" };
    let target = render_target(&job.target);
    let created = job.created_at.format("%Y-%m-%dT%H:%M:%SZ");
    let last_fired = job
        .last_fired
        .map(|dt| dt.format("%Y-%m-%dT%H:%M:%SZ").to_string())
        .unwrap_or_else(|| "never".to_string());
    let last_result = match &job.last_result {
        None => "none (never fired)".to_string(),
        Some(CronFireOutcome::Success { duration_ms }) => {
            format!("success ({duration_ms}ms)")
        }
        Some(CronFireOutcome::Error { message }) => format!("error: {message}"),
        Some(CronFireOutcome::NoSink) => {
            "no-sink (nothing fired; last_fired not advanced)".to_string()
        }
    };

    println!("id:          {}", job.id);
    println!("expression:  {}", job.expression);
    println!("target:      {target}");
    println!("state:       {state}");
    println!("created_at:  {created}");
    println!("last_fired:  {last_fired}");
    println!("last_result: {last_result}");
    Ok(())
}

/// Read fire records from the JSONL history file, returning up to `limit`
/// records for `job_id`, most-recent first.
fn read_history(job_id: &str, limit: usize, path: Option<&PathBuf>) -> Vec<CronFireRecord> {
    let Some(p) = path else {
        return Vec::new();
    };
    let file = match std::fs::File::open(p) {
        Ok(f) => f,
        Err(_) => return Vec::new(),
    };
    let reader = BufReader::new(file);
    // Collect all matching records then take the tail (most-recent are last
    // in the append-only file).
    let mut records: Vec<CronFireRecord> = reader
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<CronFireRecord>(&line).ok())
        .filter(|r| r.job_id == job_id)
        .collect();
    // Most-recent first.
    records.reverse();
    records.truncate(limit);
    records
}

/// F-065: `cron history <id> [--limit N]` — recent fire records.
async fn history_cmd(id: &str, limit: usize, history_path: Option<&PathBuf>) -> Result<()> {
    let records = read_history(id, limit, history_path);
    if records.is_empty() {
        println!("(no fire records for {id})");
        return Ok(());
    }
    for rec in &records {
        let ts = rec.fired_at.format("%Y-%m-%dT%H:%M:%SZ");
        let outcome = match &rec.outcome {
            CronFireOutcome::Success { duration_ms } => format!("success ({duration_ms}ms)"),
            CronFireOutcome::Error { message } => format!("error: {message}"),
            CronFireOutcome::NoSink => "no-sink".to_string(),
        };
        println!("{ts}  {outcome}");
    }
    Ok(())
}

/// F-065: `cron logs <id> [--limit N]` — fire records as structured log lines.
async fn logs_cmd(id: &str, limit: usize, history_path: Option<&PathBuf>) -> Result<()> {
    let records = read_history(id, limit, history_path);
    if records.is_empty() {
        println!("(no log records for {id})");
        return Ok(());
    }
    for rec in &records {
        let ts = rec.fired_at.format("%Y-%m-%dT%H:%M:%SZ");
        let (level, outcome) = match &rec.outcome {
            CronFireOutcome::Success { duration_ms } => {
                ("INFO ", format!("fired ok duration_ms={duration_ms}"))
            }
            CronFireOutcome::Error { message } => ("WARN ", format!("dispatch failed: {message}")),
            CronFireOutcome::NoSink => ("WARN ", "no sink; last_fired not advanced".to_string()),
        };
        println!("{ts}  {level}  wcore_cron::runner  job_id={id}  {outcome}");
    }
    Ok(())
}

/// F-066: `cron daemon` — detached runner.
///
/// Spawns a child process that runs the cron runner detached from the
/// controlling terminal. The child:
/// - Writes its PID to `$WAYLAND_HOME/cron-daemon.pid`
/// - Logs to `$WAYLAND_HOME/cron-daemon.log`
/// - Honours SIGTERM for clean shutdown (via tokio::signal)
/// - Persists fire history to `$WAYLAND_HOME/cron/history.jsonl`
///
/// On non-Unix platforms, prints an informational error and exits cleanly.
async fn daemon_cmd(store: &FileCronStore) -> Result<()> {
    use std::fs;

    // Resolve home dir for PID + log files.
    let wayland_home = std::env::var_os("WAYLAND_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".wayland")))
        .context("cannot resolve WAYLAND_HOME for daemon files")?;

    fs::create_dir_all(&wayland_home).context("cannot create WAYLAND_HOME")?;

    let pid_path = wayland_home.join("cron-daemon.pid");
    let log_path = wayland_home.join("cron-daemon.log");
    let history_path = wayland_home.join("cron").join("history.jsonl");

    // If we are the daemon child body, run the runner loop.
    if std::env::var("WAYLAND_CRON_DAEMON_CHILD").is_ok() {
        return daemon_body(store, &pid_path, &history_path).await;
    }

    // Check for a stale PID file — if the process is still alive, refuse to
    // start a second daemon.
    if pid_path.exists() {
        if let Ok(raw) = fs::read_to_string(&pid_path) {
            let existing_pid = raw.trim().parse::<u32>().unwrap_or(0);
            if existing_pid > 0 && process_is_alive(existing_pid) {
                bail!(
                    "cron daemon already running (pid {existing_pid}). \
                     Use `kill {existing_pid}` to stop it first."
                );
            }
        }
        // Stale file — remove it.
        let _ = fs::remove_file(&pid_path);
    }

    // Re-exec the current binary with the sentinel env var, redirecting
    // stdout/stderr to the log file so the child is decoupled from the
    // calling terminal.
    let current_exe = std::env::current_exe().context("cannot resolve current binary")?;
    let log_file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("cannot open log file: {}", log_path.display()))?;

    #[cfg(unix)]
    let child = {
        use std::os::unix::process::CommandExt as _;
        std::process::Command::new(&current_exe)
            .args(["cron", "daemon"])
            .env("WAYLAND_CRON_DAEMON_CHILD", "1")
            .env("WAYLAND_HOME", wayland_home.to_string_lossy().as_ref())
            .stdin(std::process::Stdio::null())
            .stdout(log_file.try_clone().context("log file clone")?)
            .stderr(log_file)
            // process_group(0) calls setsid() in the child — detaches from
            // the parent's process group and controlling terminal.
            .process_group(0)
            .spawn()
            .context("failed to spawn daemon child")?
    };

    #[cfg(not(unix))]
    let child = {
        std::process::Command::new(&current_exe)
            .args(["cron", "daemon"])
            .env("WAYLAND_CRON_DAEMON_CHILD", "1")
            .env("WAYLAND_HOME", wayland_home.to_string_lossy().as_ref())
            .stdin(std::process::Stdio::null())
            .stdout(log_file.try_clone().context("log file clone")?)
            .stderr(log_file)
            .spawn()
            .context("failed to spawn daemon child")?
    };

    let child_pid = child.id();
    fs::write(&pid_path, format!("{child_pid}\n"))
        .with_context(|| format!("cannot write PID file: {}", pid_path.display()))?;

    println!(
        "cron daemon started (pid {child_pid})\n  pid:  {}\n  log:  {}",
        pid_path.display(),
        log_path.display()
    );
    Ok(())
}

/// Daemon body — runs inside the re-exec'd child process.
async fn daemon_body(
    store: &FileCronStore,
    pid_path: &std::path::Path,
    history_path: &PathBuf,
) -> Result<()> {
    let my_pid = std::process::id();
    let _ = std::fs::write(pid_path, format!("{my_pid}\n"));

    eprintln!("[cron-daemon] started pid={my_pid}");

    let cron_store: Arc<dyn wcore_cron::CronStore> = Arc::new(store.clone());
    // Daemon has no engine session — use a log-only handler. Skill/channel
    // sinks are not available without a bootstrapped engine. The daemon's
    // primary value is keeping last_fired accurate and history.jsonl populated
    // between interactive sessions. A future improvement can wire a minimal
    // engine bootstrap here.
    let handler: Arc<dyn wcore_cron::JobHandler> =
        Arc::new(wcore_cron::runner::RecordingHandler::new());

    let mut ticker = tokio::time::interval(wcore_cron::runner::TICK_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await; // eat the immediate first tick

    // SIGTERM handler via tokio::signal (safe, no raw libc required).
    // On non-Unix, Ctrl+C is the closest equivalent.
    let shutdown = shutdown_signal();

    tokio::pin!(shutdown);

    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                eprintln!("[cron-daemon] shutdown signal received; stopping");
                break;
            }
            _ = ticker.tick() => {
                if let Err(e) = wcore_cron::tick_once_with_history(
                    &cron_store,
                    &handler,
                    Some(history_path),
                ).await {
                    eprintln!("[cron-daemon] tick error: {e}");
                }
            }
        }
    }

    // Remove PID file on graceful exit so a subsequent `cron daemon` start
    // doesn't see a stale entry.
    let _ = std::fs::remove_file(pid_path);
    eprintln!("[cron-daemon] stopped");
    Ok(())
}

/// Returns a future that completes on the first daemon-appropriate shutdown
/// signal: SIGTERM on Unix, Ctrl+C on other platforms.
async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        if let Ok(mut s) = signal(SignalKind::terminate()) {
            s.recv().await;
        } else {
            // Fallback: Ctrl+C.
            let _ = tokio::signal::ctrl_c().await;
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

/// Returns `true` if a process with the given PID appears to be alive.
/// Uses `/proc/<pid>` on Linux or `kill -0` on macOS; uses
/// `OpenProcess` + `GetExitCodeProcess` on Windows.
///
/// Audit W-1 fix (E2E-WINDOWS-ADDENDUM-2026-05-24 §2.2):
/// The previous `#[cfg(not(unix))]` branch returned hardcoded `false`,
/// causing every Windows `cron daemon` invocation to spawn a duplicate
/// daemon because the PID check always reported "dead."
fn process_is_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // /proc/<pid> existence check — no libc required.
        std::path::Path::new(&format!("/proc/{pid}")).exists()
            // macOS doesn't have /proc; fall back to kill(pid, 0) via std.
            || {
                use std::process::Command;
                Command::new("kill")
                    .args(["-0", &pid.to_string()])
                    .output()
                    .map(|o| o.status.success())
                    .unwrap_or(false)
            }
    }
    #[cfg(windows)]
    {
        // OpenProcess with PROCESS_QUERY_LIMITED_INFORMATION does not
        // require SeDebugPrivilege and works for processes owned by the
        // same user. GetExitCodeProcess returns STILL_ACTIVE (0x103)
        // while the process is running; any other code means it exited.
        // `STILL_ACTIVE` lives in `Win32::Foundation` (typed as `NTSTATUS = i32`)
        // in `windows-sys = 0.59`, not under `System::Threading`.
        use windows_sys::Win32::Foundation::{CloseHandle, FALSE, HANDLE, STILL_ACTIVE};
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
        };

        // SAFETY: Win32 FFI. OpenProcess returns NULL on failure.
        let handle: HANDLE = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid) };
        if handle.is_null() {
            // OpenProcess failed — process does not exist or access denied.
            // Treat as dead so a stale PID file does not block a restart.
            return false;
        }
        let mut exit_code: u32 = 0;
        // SAFETY: handle is valid (non-NULL) and exit_code is a local u32.
        let ok = unsafe { GetExitCodeProcess(handle, &mut exit_code) };
        // SAFETY: handle was opened by us and must be closed exactly once.
        unsafe { CloseHandle(handle) };
        // STILL_ACTIVE is NTSTATUS (i32); GetExitCodeProcess returns u32. Cast
        // for the comparison — both encode the same 0x103 bit pattern.
        ok != 0 && exit_code as i32 == STILL_ACTIVE
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        false
    }
}

#[cfg(test)]
#[cfg(windows)]
mod windows_tests {
    /// Audit W-1 regression guard: process_is_alive() must return true for
    /// the current process on Windows (not the hardcoded-false stub).
    ///
    /// This test would have caught the W-1 bug on any Windows CI run.
    #[test]
    fn process_is_alive_current_process_is_alive() {
        let my_pid = std::process::id();
        assert!(
            super::process_is_alive(my_pid),
            "process_is_alive() returned false for the running process (pid={my_pid}); \
             W-1 regression: the Windows stub returned hardcoded false"
        );
    }

    /// process_is_alive() must return false for a PID that cannot exist.
    #[test]
    fn process_is_alive_invalid_pid_is_dead() {
        // PID 0 is the System Idle Process; OpenProcess with
        // PROCESS_QUERY_LIMITED_INFORMATION will fail → returns false.
        assert!(
            !super::process_is_alive(0),
            "process_is_alive(0) must return false"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    use wcore_cron::FileCronStore;

    fn store(dir: &std::path::Path) -> FileCronStore {
        FileCronStore::new(dir.join("jobs.json"))
    }

    #[tokio::test]
    async fn add_slash_round_trip() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        run_with_store(
            CronCmd::Add {
                expression: "0 9 * * *".into(),
                slash: Some("/morning".into()),
                channel: None,
                text: None,
                skill: None,
                args: None,
            },
            &s,
        )
        .await
        .unwrap();
        let jobs = s.list().await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert!(matches!(jobs[0].target, Target::Slash { .. }));
    }

    #[tokio::test]
    async fn add_channel_requires_text() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        let r = run_with_store(
            CronCmd::Add {
                expression: "*/15 * * * *".into(),
                slash: None,
                channel: Some("team".into()),
                text: None,
                skill: None,
                args: None,
            },
            &s,
        )
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn add_channel_ok() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        run_with_store(
            CronCmd::Add {
                expression: "*/15 * * * *".into(),
                slash: None,
                channel: Some("team-slack".into()),
                text: Some("status check".into()),
                skill: None,
                args: None,
            },
            &s,
        )
        .await
        .unwrap();
        let jobs = s.list().await.unwrap();
        assert!(matches!(jobs[0].target, Target::Channel { .. }));
    }

    #[tokio::test]
    async fn add_skill_default_args() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        run_with_store(
            CronCmd::Add {
                expression: "0 8 * * *".into(),
                slash: None,
                channel: None,
                text: None,
                skill: Some("morning-brief".into()),
                args: None,
            },
            &s,
        )
        .await
        .unwrap();
        let jobs = s.list().await.unwrap();
        match &jobs[0].target {
            Target::Skill { name, args } => {
                assert_eq!(name, "morning-brief");
                assert!(args.is_object());
            }
            _ => panic!("expected skill target"),
        }
    }

    #[tokio::test]
    async fn add_no_target_errors() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        let r = run_with_store(
            CronCmd::Add {
                expression: "0 9 * * *".into(),
                slash: None,
                channel: None,
                text: None,
                skill: None,
                args: None,
            },
            &s,
        )
        .await;
        assert!(r.is_err());
    }

    #[tokio::test]
    async fn enable_disable_remove() {
        let dir = tempdir().unwrap();
        let s = store(dir.path());
        run_with_store(
            CronCmd::Add {
                expression: "0 9 * * *".into(),
                slash: Some("/x".into()),
                channel: None,
                text: None,
                skill: None,
                args: None,
            },
            &s,
        )
        .await
        .unwrap();
        let id = s.list().await.unwrap()[0].id.clone();

        run_with_store(CronCmd::Disable { id: id.clone() }, &s)
            .await
            .unwrap();
        assert!(!s.list().await.unwrap()[0].enabled);

        run_with_store(CronCmd::Enable { id: id.clone() }, &s)
            .await
            .unwrap();
        assert!(s.list().await.unwrap()[0].enabled);

        run_with_store(CronCmd::Remove { id }, &s).await.unwrap();
        assert!(s.list().await.unwrap().is_empty());
    }
}
