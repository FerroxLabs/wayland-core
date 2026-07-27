//! `wayland-core gateway` — the operator verb surface over `wcore-gateway`.
//!
//! Phase 24, plan 24-B (the successor lane to 24-02). This file is named by
//! path in two places in the crate it drives:
//!
//! - `crates/wcore-gateway/src/lib.rs`: "The operator verb surface lives in
//!   `crates/wcore-cli/src/gateway.rs` and drives this crate."
//! - `crates/wcore-gateway/src/lifecycle.rs`: "Every operator verb in
//!   `crates/wcore-cli/src/gateway.rs` drives exactly one transition here and
//!   reads back exactly this projection."
//!
//! It did not exist. That was not merely a missing feature — it was a live
//! defect, because every service unit `wcore-gateway::service` generates
//! invokes `<binary> gateway run`:
//!
//! ```text
//! launchd   ProgramArguments = ["{binary}", "gateway", "run"]
//! systemd   ExecStart={binary} gateway run
//! schtasks  /tr "\"{binary}\" gateway run"
//! ```
//!
//! With no `gateway` subcommand, an install on any of the three families
//! registered a unit whose command fails immediately with a clap
//! "unrecognized subcommand" error. The registration succeeded and the
//! service never ran. **`run` is therefore the load-bearing verb here**, and
//! the others exist to drive it.
//!
//! # Verb set, and the recorded gap
//!
//! The 24-01 contract named nine verbs: `install start stop restart status
//! doctor logs drain uninstall`. Seven of those plus `run` are implemented.
//! `doctor` and `logs` are NOT, by a 4/4 cross-audited decision recorded in
//! `24-B-GATEWAY-SURFACE.md`: neither appears in any Phase-24 Success
//! Criterion clause or in any step of 24-04's journey, and the budget they
//! would consume is budget not spent live-proving the kill-and-recover path.
//! The gap is named rather than absorbed.
//!
//! # Where each verb's authority lives
//!
//! No policy is invented here. `install`/`uninstall`/`start`/`stop` render
//! their argv from `wcore_gateway::service::ServiceManager`, whose
//! `for_this_platform()` is the single platform-selection point in the
//! workspace; `status` renders `wcore_gateway::lifecycle::StatusProjection`;
//! `drain` drives `wcore_gateway::drain::DrainController` through the
//! `AutomationPlane`. This module is a surface, not a second implementation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use wcore_gateway::lifecycle::{GatewayState, StatusProjection};
use wcore_gateway::pidlock::{PidLock, process_is_alive};
use wcore_gateway::service::{ServiceSpec, is_registerable_binary};

/// The file the RUNNING gateway republishes its projection into.
///
/// Deliberately separate from `gateway.pid`: the pid record is written once
/// at acquisition and is the *identity*, while this is rewritten every tick
/// and is the *state*. A reader that finds a fresh projection but a dead pid
/// must believe the pid, which is why `status` checks liveness first and
/// never reports a projection for a process that is gone.
const STATUS_FILE: &str = "gateway-status.json";

/// The file `gateway drain` creates and `gateway run` polls.
///
/// A file rather than a signal because SIGTERM already means "stop" on both
/// Unix families and Windows has no equivalent at all; overloading one
/// mechanism with two meanings would make a drain indistinguishable from a
/// stop in exactly the case where the difference matters.
const DRAIN_REQUEST_FILE: &str = "gateway.drain";

/// How often `run` ticks the schedule and republishes its projection.
const TICK_MS: u64 = 1_000;

/// Environment sentinel marking the re-exec'd child, mirroring the shape
/// `cron daemon` already uses.
const RUN_CHILD_ENV: &str = "WAYLAND_GATEWAY_RUN_CHILD";

#[derive(Debug, Args)]
pub struct GatewayArgs {
    #[command(subcommand)]
    pub cmd: GatewayCmd,
}

#[derive(Debug, Subcommand)]
pub enum GatewayCmd {
    /// Register the gateway as a per-user native service for this platform.
    Install(ScopeArgs),
    /// Remove the native service registration.
    Uninstall(ScopeArgs),
    /// Start the registered gateway through the platform's service manager.
    Start(ScopeArgs),
    /// Stop it through the platform's service manager.
    Stop(ScopeArgs),
    /// Stop then start. A stop refusal because nothing was running is not an
    /// error here — restarting a stopped gateway is a start.
    Restart(ScopeArgs),
    /// Report what the gateway is doing.
    Status {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Emit the machine-readable projection instead of the operator view.
        #[arg(long)]
        json: bool,
    },
    /// Close admission, finish in-flight work within a budget, and exit.
    Drain {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Milliseconds to wait before a forced exit abandons work BY NAME.
        #[arg(long, default_value_t = 30_000)]
        budget_ms: u64,
    },
    /// The long-lived runtime itself. This is what every generated service
    /// unit invokes; it is not normally typed by an operator.
    Run {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Re-exec detached and return, instead of running in the foreground.
        ///
        /// DEFAULT IS FOREGROUND, and that is load-bearing: every service
        /// manager supervises the child it launched, so a `run` that forked
        /// and returned would make launchd/systemd/schtasks believe the
        /// gateway had exited immediately and restart it forever. `--detach`
        /// exists for an operator starting one by hand without a service.
        #[arg(long)]
        detach: bool,
    },
}

#[derive(Debug, Clone, Args)]
pub struct ScopeArgs {
    /// The profile this gateway hosts. One gateway, one home, one profile.
    #[arg(long)]
    pub profile: Option<String>,
}

impl ScopeArgs {
    fn profile(&self) -> String {
        self.profile
            .clone()
            .or_else(|| std::env::var("WAYLAND_PROFILE").ok())
            .unwrap_or_else(|| "default".to_string())
    }
}

/// Resolve the gateway home through the crate that owns the resolution, so
/// the workspace keeps ONE home story rather than two that can both claim a
/// directory.
fn home() -> Result<PathBuf> {
    wcore_gateway::resolve_home().context("cannot resolve WAYLAND_HOME or $HOME for the gateway")
}

fn status_path(home: &Path) -> PathBuf {
    home.join(STATUS_FILE)
}

fn drain_request_path(home: &Path) -> PathBuf {
    home.join(DRAIN_REQUEST_FILE)
}

/// Build the spec every service verb registers against.
///
/// The binary is resolved from the RUNNING executable, never from an
/// operator-supplied string (threat T-24-01-01), and a non-absolute result is
/// refused rather than registered: a service that resolves its own binary
/// against a working directory it does not control is a substitution waiting
/// to happen.
fn spec(scope: &ScopeArgs) -> Result<ServiceSpec> {
    let binary = wcore_gateway::service::running_binary()
        .context("cannot resolve the running binary to register")?;
    if !is_registerable_binary(&binary) {
        bail!(
            "refusing to register a non-absolute binary path: {}",
            binary.display()
        );
    }
    Ok(ServiceSpec {
        profile: scope.profile(),
        binary,
        home: home()?,
    })
}

/// Run one service-manager argv in ARGV mode.
///
/// Never a shell string: the profile, the binary path and the home path all
/// cross a trust boundary from the operator, and in argv mode a shell
/// metacharacter in any of them reaches the child as a literal byte.
async fn run_argv(argv: &[String]) -> Result<std::process::Output> {
    if argv.is_empty() {
        bail!("this platform has no service mechanism for the gateway");
    }
    let args: Vec<&str> = argv[1..].iter().map(String::as_str).collect();
    wcore_config::shell::shell_command_argv(&argv[0], &args)
        .output()
        .await
        .with_context(|| format!("failed to invoke `{}`", argv.join(" ")))
}

pub async fn run(args: GatewayArgs) -> Result<()> {
    match args.cmd {
        GatewayCmd::Install(scope) => install(&scope).await,
        GatewayCmd::Uninstall(scope) => uninstall(&scope).await,
        GatewayCmd::Start(scope) => start(&scope).await,
        GatewayCmd::Stop(scope) => stop(&scope, true).await,
        GatewayCmd::Restart(scope) => {
            // A stop refusal because nothing was running is not an error:
            // restarting a stopped gateway is a start. Any OTHER stop failure
            // still propagates, because restarting over a gateway that refused
            // to stop would leave two.
            stop(&scope, false).await?;
            start(&scope).await
        }
        GatewayCmd::Status { scope, json } => status(&scope, json).await,
        GatewayCmd::Drain { scope, budget_ms } => drain(&scope, budget_ms),
        GatewayCmd::Run { scope, detach } => run_gateway(&scope, detach).await,
    }
}

// ---------------------------------------------------------------------------
// install / uninstall
// ---------------------------------------------------------------------------

async fn install(scope: &ScopeArgs) -> Result<()> {
    let spec = spec(scope)?;
    let mgr = wcore_gateway::service::for_this_platform();
    std::fs::create_dir_all(&spec.home)
        .with_context(|| format!("cannot create gateway home {}", spec.home.display()))?;

    // The unit is written BEFORE the registration command runs. Both Unix
    // families' registration commands read the unit off disk, so registering
    // first would register a unit that is not there yet.
    if let (Some(text), Some(path)) = (mgr.unit_text(&spec), mgr.unit_path(&spec)) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create unit directory {}", parent.display()))?;
        }
        std::fs::write(&path, text)
            .with_context(|| format!("cannot write service unit {}", path.display()))?;
        println!("wrote unit: {}", path.display());
    }

    let argv = mgr.install_argv(&spec);
    let out = run_argv(&argv).await?;
    if !out.status.success() {
        bail!(
            "`{}` failed with status {}: {}",
            argv.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    println!(
        "gateway installed ({}): {}\n  home:    {}\n  binary:  {}",
        mgr.family(),
        spec.service_name(),
        spec.home.display(),
        spec.binary.display()
    );
    Ok(())
}

async fn uninstall(scope: &ScopeArgs) -> Result<()> {
    let spec = spec(scope)?;
    let mgr = wcore_gateway::service::for_this_platform();

    let argv = mgr.uninstall_argv(&spec);
    let out = run_argv(&argv).await?;
    if !out.status.success() {
        bail!(
            "`{}` failed with status {}: {}",
            argv.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }

    // The unit file goes AFTER the deregistration succeeds. Removing it first
    // would leave a registration naming a file that is gone, which is the one
    // state neither `uninstall` nor `install` can then reason about.
    if let Some(path) = mgr.unit_path(&spec) {
        if path.exists() {
            std::fs::remove_file(&path)
                .with_context(|| format!("cannot remove service unit {}", path.display()))?;
            println!("removed unit: {}", path.display());
        }
    }
    println!(
        "gateway uninstalled ({}): {}",
        mgr.family(),
        spec.service_name()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// start / stop / restart
// ---------------------------------------------------------------------------

async fn start(scope: &ScopeArgs) -> Result<()> {
    let spec = spec(scope)?;
    let mgr = wcore_gateway::service::for_this_platform();
    let argv = mgr.start_argv(&spec);
    let out = run_argv(&argv).await?;
    if !out.status.success() {
        bail!(
            "`{}` failed with status {}: {}",
            argv.join(" "),
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    println!("gateway start requested ({})", mgr.family());
    Ok(())
}

/// `strict` distinguishes `stop` (a refusal is an error the operator asked
/// about) from the stop inside `restart` (a refusal because nothing was
/// running is the normal case).
async fn stop(scope: &ScopeArgs, strict: bool) -> Result<()> {
    let spec = spec(scope)?;
    let mgr = wcore_gateway::service::for_this_platform();
    let argv = mgr.stop_argv(&spec);
    let out = run_argv(&argv).await?;
    if !out.status.success() {
        if strict {
            bail!(
                "`{}` failed with status {}: {}",
                argv.join(" "),
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        eprintln!(
            "gateway stop reported: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return Ok(());
    }
    println!("gateway stop requested ({})", mgr.family());
    Ok(())
}

// ---------------------------------------------------------------------------
// status
// ---------------------------------------------------------------------------

/// Read the projection a running gateway publishes, if there is one and its
/// process is genuinely alive.
///
/// LIVENESS IS CHECKED FIRST AND THE PID DECIDES. A crashed gateway leaves
/// both its record and its last projection on disk; believing the projection
/// would report `running` with a pid that is gone, which is exactly the lie
/// `StatusProjection::stopped` exists to refuse.
pub fn read_live_projection(home: &Path) -> Option<StatusProjection> {
    let record = PidLock::read_record(home)?;
    if !process_is_alive(record.pid) {
        return None;
    }
    let raw = std::fs::read_to_string(status_path(home)).ok()?;
    let mut proj: StatusProjection = serde_json::from_str(&raw).ok()?;
    // The record is the authority on identity; the projection is the
    // authority on counts. A projection carrying a stale pid from a previous
    // process must not survive into the answer.
    proj.pid = Some(record.pid);
    if proj.binary_path.is_none() {
        proj.binary_path = record.binary_path.clone();
    }
    Some(proj)
}

/// Whether a REGISTRATION exists for this spec — which is not the same
/// question as whether anything is running.
///
/// F24-B-H2, found by the live Linux journey. This first read
/// `status_argv`, which is `systemctl --user is-active`: that answers
/// ACTIVITY, so during the five seconds systemd spent restarting the
/// gateway after a hard kill, and again after a clean drain, the verb
/// reported `Uninstalled` for a service whose unit was on disk and enabled.
/// An operator debugging a service that will not stay up would have been
/// told it was never installed.
///
/// The branch is on a CAPABILITY the trait already exposes rather than on
/// the platform: a family that writes an on-disk unit has that file as its
/// registration record, and a family that does not (Windows registers
/// through a command line) is asked its query verb, which for `schtasks
/// /query` genuinely answers registration rather than activity.
async fn is_registered(
    mgr: &dyn wcore_gateway::service::ServiceManager,
    spec: &ServiceSpec,
) -> bool {
    if let Some(unit) = mgr.unit_path(spec) {
        return unit.exists();
    }
    let argv = mgr.status_argv(spec);
    run_argv(&argv)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn status(scope: &ScopeArgs, json: bool) -> Result<()> {
    let home = home()?;
    let profile = scope.profile();
    let mgr = wcore_gateway::service::for_this_platform();

    let proj = match read_live_projection(&home) {
        Some(p) => p,
        None => {
            // Nothing is running. Distinguish "never installed" from
            // "installed and down": an operator debugging a service that will
            // not start needs to know the registration exists.
            let mut p = StatusProjection::stopped(&profile);
            if let Ok(spec) = spec(scope) {
                if !is_registered(&*mgr, &spec).await {
                    p.state = GatewayState::Uninstalled;
                }
            }
            p
        }
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&proj)?);
        return Ok(());
    }

    println!("gateway: {}", proj.state);
    println!("  profile:            {}", proj.profile);
    println!("  home:               {}", home.display());
    println!("  service family:     {}", mgr.family());
    match proj.pid {
        Some(pid) => println!("  pid:                {pid}"),
        None => println!("  pid:                -"),
    }
    match proj.uptime_secs {
        Some(s) => println!("  uptime:             {s}s"),
        None => println!("  uptime:             -"),
    }
    println!("  turns in flight:    {}", proj.turns_in_flight);
    println!("  deliveries pending: {}", proj.deliveries_pending);
    println!(
        "  binary:             {}",
        proj.binary_path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "-".into())
    );
    println!(
        "  binary version:     {}",
        proj.binary_version.as_deref().unwrap_or("-")
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// drain
// ---------------------------------------------------------------------------

fn drain(scope: &ScopeArgs, budget_ms: u64) -> Result<()> {
    let home = home()?;
    let Some(proj) = read_live_projection(&home) else {
        // The lifecycle machine refuses Drain from anything that is not
        // Running, by name. Rendering that refusal rather than inventing one
        // keeps the CLI's exit statuses derived from the machine.
        bail!("cannot drain: no gateway is running for {}", home.display());
    };
    let _ = scope;

    std::fs::write(drain_request_path(&home), format!("{budget_ms}\n"))
        .context("cannot write the drain request")?;
    println!(
        "drain requested (budget {budget_ms}ms); gateway pid {}",
        proj.pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "-".into())
    );

    // Wait for the runtime to publish a terminal state.
    //
    // F24-B-H3, measured: the earlier bound of `budget_ms + 2 ticks` was too
    // tight and reported a timeout over a drain that was working. The
    // runtime can legitimately consume the request-notice latency (up to one
    // tick), then the operator's WHOLE budget, then a final publish. The
    // bound is therefore the budget plus four ticks, and the failure message
    // now names what was actually observed rather than only the budget.
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_millis(budget_ms + 4 * TICK_MS);
    let mut last_seen = GatewayState::Running;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(200));
        match read_live_projection(&home) {
            Some(p) => {
                if p.state != last_seen {
                    // The observable half of the drain contract: the operator
                    // sees the state move and the pending count with it.
                    println!(
                        "  {} (deliveries pending {})",
                        p.state, p.deliveries_pending
                    );
                    last_seen = p.state;
                }
                if matches!(p.state, GatewayState::Drained | GatewayState::Stopped) {
                    println!(
                        "drain complete: {} (deliveries pending {})",
                        p.state, p.deliveries_pending
                    );
                    return Ok(());
                }
            }
            None => {
                // The process is gone. Its last published projection is the
                // record of how it ended, and it is read from disk rather
                // than assumed clean.
                if let Ok(raw) = std::fs::read_to_string(status_path(&home)) {
                    if let Ok(p) = serde_json::from_str::<StatusProjection>(&raw) {
                        println!(
                            "drain complete: {} (deliveries pending {})",
                            p.state, p.deliveries_pending
                        );
                        return Ok(());
                    }
                }
                println!("drain complete: gateway exited");
                return Ok(());
            }
        }
    }
    bail!(
        "drain did not reach a terminal state within {budget_ms}ms + slack; last observed state was {last_seen}"
    );
}

// ---------------------------------------------------------------------------
// run — the runtime every generated service unit invokes
// ---------------------------------------------------------------------------

/// Publish the projection a second process reads.
///
/// Written to a same-directory temporary and renamed, so a `status` racing a
/// republish reads either the previous projection or the next one and never a
/// half-written file.
fn publish(home: &Path, proj: &StatusProjection) -> Result<()> {
    let final_path = status_path(home);
    let tmp = final_path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(proj)?)
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &final_path)
        .with_context(|| format!("cannot publish {}", final_path.display()))?;
    Ok(())
}

async fn run_gateway(scope: &ScopeArgs, detach: bool) -> Result<()> {
    let home = home()?;
    std::fs::create_dir_all(&home)
        .with_context(|| format!("cannot create gateway home {}", home.display()))?;

    if detach && std::env::var(RUN_CHILD_ENV).is_err() {
        return spawn_detached(&home);
    }

    let profile = scope.profile();
    let started = std::time::Instant::now();

    // The lock is taken FIRST and held for the whole run. Acquisition is what
    // refuses a second gateway on this home, and the refusal names the holder.
    let _lock = PidLock::acquire(&home).context("cannot claim this gateway home")?;

    let store: Arc<dyn wcore_cron::CronStore> = Arc::new(wcore_cron::FileCronStore::new(
        wcore_gateway::automation::AutomationPlane::schedule_dir(&home).join("jobs.json"),
    ));
    let cwd = std::env::current_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| ".".to_string());
    // The REAL headless handler, not a recorder. A gateway whose dispatch is
    // a log line proves its own loop and nothing about delivery.
    let handler: Arc<dyn wcore_cron::JobHandler> =
        Arc::new(wcore_agent::cron::build_headless_cron_handler(&cwd).await);
    let history =
        wcore_gateway::automation::AutomationPlane::schedule_dir(&home).join("history.jsonl");

    let mut plane =
        wcore_gateway::automation::AutomationPlane::start(&home, store, handler, Some(history))
            .context("cannot start the automation plane")?;

    // Everything the previous process left unfinished is picked up BEFORE the
    // first tick. A gateway that starts ticking before it resumes has a window
    // in which a carried delivery is invisible to its own status.
    let resumed = plane.resume().context("cannot resume carried deliveries")?;
    eprintln!(
        "[gateway] started pid={} role={:?} profile={profile} carried={} (unattempted {} / unknown-outcome {}) quarantined={}",
        std::process::id(),
        plane.role(),
        resumed.carried(),
        resumed.unattempted.len(),
        resumed.unknown_outcome.len(),
        resumed.quarantined,
    );

    let binary_path = std::env::current_exe().ok();
    let binary_version = Some(env!("CARGO_PKG_VERSION").to_string());
    let project = |plane: &wcore_gateway::automation::AutomationPlane, state: GatewayState| {
        StatusProjection {
            state,
            pid: Some(std::process::id()),
            uptime_secs: Some(started.elapsed().as_secs()),
            profile: profile.clone(),
            turns_in_flight: 0,
            deliveries_pending: plane.pending_deliveries().len(),
            binary_path: binary_path.clone(),
            binary_version: binary_version.clone(),
        }
    };

    publish(&home, &project(&plane, plane.state()))?;

    // A drain request left by a previous run is removed rather than honoured:
    // it was addressed to a process that is gone, and honouring it would make
    // a fresh gateway drain itself immediately on every start after one drain.
    let _ = std::fs::remove_file(drain_request_path(&home));

    let mut ticker = tokio::time::interval(std::time::Duration::from_millis(TICK_MS));
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;

    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);

    let mut drain_budget: Option<u64> = None;
    loop {
        tokio::select! {
            biased;
            _ = &mut shutdown => {
                eprintln!("[gateway] shutdown signal received");
                break;
            }
            _ = ticker.tick() => {
                if let Ok(raw) = std::fs::read_to_string(drain_request_path(&home)) {
                    drain_budget = Some(raw.trim().parse::<u64>().unwrap_or(30_000));
                    let _ = std::fs::remove_file(drain_request_path(&home));
                    eprintln!("[gateway] drain requested (budget {:?}ms)", drain_budget);
                    // F24-B-H3. Publish Draining BEFORE the drain runs. The
                    // tick loop is about to exit, so nothing republishes
                    // until the drain finishes; without this the projection
                    // stays `Running` for the whole budget and an operator
                    // watching `gateway drain` sees a state that contradicts
                    // what it asked for. The drain contract is that the
                    // counts are OBSERVABLE, and a projection frozen at
                    // `Running` observes nothing.
                    publish(&home, &project(&plane, GatewayState::Draining))?;
                    break;
                }
                if let Err(e) = plane.tick(chrono::Utc::now()).await {
                    eprintln!("[gateway] tick error: {e}");
                }
                publish(&home, &project(&plane, plane.state()))?;
            }
        }
    }

    // The drain path is the SAME whether the trigger was a signal or a
    // request file. A stop that skipped the drain would abandon deliveries
    // without recording that it had.
    let budget = drain_budget.unwrap_or(30_000);
    let report = plane
        .drain_and_release(budget, |ledger| {
            // The wait is one bounded sleep per observation rather than a
            // spin, and it returns the elapsed milliseconds the controller
            // charges against the budget.
            let _ = ledger.flush();
            std::thread::sleep(std::time::Duration::from_millis(100));
            100
        })
        .context("drain failed")?;

    let clean = wcore_gateway::automation::AutomationPlane::drained_cleanly(&report);
    eprintln!(
        "[gateway] drain {:?}: observations={} abandoned={} flushed={}",
        report.outcome,
        report.trace.len(),
        report.abandoned.len(),
        report.flushed
    );
    for id in &report.abandoned {
        eprintln!("[gateway] ABANDONED delivery {id}");
    }

    let mut final_proj = project(&plane, GatewayState::Drained);
    final_proj.pid = None;
    final_proj.uptime_secs = None;
    publish(&home, &final_proj)?;
    eprintln!("[gateway] stopped (clean={clean})");
    Ok(())
}

/// Re-exec detached, mirroring the shape `cron daemon` already uses so the
/// workspace has one detach story. The Windows flag set is the measured one
/// from `wcore_gateway::service`.
fn spawn_detached(home: &Path) -> Result<()> {
    let current_exe = std::env::current_exe().context("cannot resolve current binary")?;
    let log_path = home.join("gateway.log");
    let log_file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("cannot open log file: {}", log_path.display()))?;

    #[cfg(unix)]
    let child = {
        use std::os::unix::process::CommandExt as _;
        std::process::Command::new(&current_exe)
            .args(["gateway", "run"])
            .env(RUN_CHILD_ENV, "1")
            .env("WAYLAND_HOME", home.as_os_str())
            .stdin(std::process::Stdio::null())
            .stdout(log_file.try_clone().context("log file clone")?)
            .stderr(log_file)
            .process_group(0)
            .spawn()
            .context("failed to spawn the gateway child")?
    };

    #[cfg(not(unix))]
    let child = {
        let mut cmd = std::process::Command::new(&current_exe);
        cmd.args(["gateway", "run"])
            .env(RUN_CHILD_ENV, "1")
            .env("WAYLAND_HOME", home.as_os_str())
            .stdin(std::process::Stdio::null())
            .stdout(log_file.try_clone().context("log file clone")?)
            .stderr(log_file);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt as _;
            // The measured set. CREATE_BREAKAWAY_FROM_JOB is load-bearing:
            // Windows OpenSSH reaps session children through a Job Object and
            // only a breakaway leaves it. See 24-01-GATEWAY-CONTRACT.md §6.
            cmd.creation_flags(
                wcore_gateway::service::DETACHED_PROCESS
                    | wcore_gateway::service::CREATE_NEW_PROCESS_GROUP
                    | wcore_gateway::service::CREATE_BREAKAWAY_FROM_JOB,
            );
        }
        cmd.spawn().context("failed to spawn the gateway child")?
    };

    println!("gateway run detached (pid {})", child.id());
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = tokio::signal::ctrl_c() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_profile_is_named_not_empty() {
        let s = ScopeArgs { profile: None };
        // An empty profile would produce the service identifier
        // `wayland-core-gateway-`, which every family accepts and no operator
        // can tell apart from another empty one.
        assert!(!s.profile().is_empty());
    }

    #[test]
    fn an_explicit_profile_wins_over_the_environment() {
        let s = ScopeArgs {
            profile: Some("explicit".into()),
        };
        assert_eq!(s.profile(), "explicit");
    }

    #[test]
    fn a_status_file_from_a_dead_process_is_not_reported_as_running() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();

        // Seed exactly the state a crashed gateway leaves: a pid record
        // naming a process that is gone, plus its last published projection
        // still claiming Running.
        PidLock::write_stale_record_for_test(home, 999_999);
        let stale = StatusProjection {
            state: GatewayState::Running,
            pid: Some(999_999),
            uptime_secs: Some(42),
            profile: "default".into(),
            turns_in_flight: 3,
            deliveries_pending: 7,
            binary_path: Some(PathBuf::from("/opt/x/wayland-core")),
            binary_version: Some("0.0.0".into()),
        };
        std::fs::write(
            status_path(home),
            serde_json::to_vec_pretty(&stale).unwrap(),
        )
        .unwrap();

        // The projection is present and says Running. The pid decides.
        assert!(
            read_live_projection(home).is_none(),
            "a projection whose process is gone must not be reported"
        );
    }

    #[test]
    fn a_live_projection_takes_its_identity_from_the_record() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let _lock = PidLock::acquire(home).unwrap();

        // A projection written by an EARLIER process, carrying that
        // process's pid. The record is the authority on identity, so the
        // answer must carry this process's pid rather than the stale one.
        let proj = StatusProjection {
            state: GatewayState::Running,
            pid: Some(4_242),
            uptime_secs: Some(9),
            profile: "default".into(),
            turns_in_flight: 0,
            deliveries_pending: 2,
            binary_path: None,
            binary_version: Some("0.0.0".into()),
        };
        std::fs::write(status_path(home), serde_json::to_vec_pretty(&proj).unwrap()).unwrap();

        let live = read_live_projection(home).expect("the holder is alive");
        assert_eq!(live.pid, Some(std::process::id()));
        assert_eq!(live.deliveries_pending, 2);
    }

    #[test]
    fn a_publish_is_atomic_and_leaves_no_temporary_behind() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path();
        let proj = StatusProjection::stopped("default");
        publish(home, &proj).unwrap();
        assert!(status_path(home).exists());
        assert!(
            !status_path(home).with_extension("json.tmp").exists(),
            "the same-directory temporary must be renamed away, not left"
        );
        let raw = std::fs::read_to_string(status_path(home)).unwrap();
        let back: StatusProjection = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, proj);
    }

    /// The verbs clap will actually accept, read off the built command
    /// rather than off the enum, so a `#[command(skip)]` or a rename would
    /// change the answer.
    fn verb_names() -> Vec<String> {
        use clap::Subcommand as _;
        GatewayCmd::augment_subcommands(clap::Command::new("gateway"))
            .get_subcommands()
            .map(|c| c.get_name().to_string())
            .collect()
    }

    #[test]
    fn every_generated_unit_invokes_the_verb_this_module_implements() {
        // THE REGRESSION THIS FILE EXISTS FOR. Before this module, all three
        // families registered `<binary> gateway run` and no `gateway`
        // subcommand existed, so every install produced a unit whose command
        // failed with a clap "unrecognized subcommand" error. This asserts the
        // two halves still agree; it goes red if either the unit text or the
        // subcommand name moves.
        let s = ServiceSpec {
            profile: "default".into(),
            binary: PathBuf::from("/opt/x/wayland-core"),
            home: PathBuf::from("/home/op/.wayland"),
        };
        let managers: Vec<Box<dyn wcore_gateway::service::ServiceManager>> = vec![
            Box::new(wcore_gateway::service::LaunchdManager),
            Box::new(wcore_gateway::service::SystemdManager),
            Box::new(wcore_gateway::service::ScheduledTaskManager),
        ];
        for m in managers {
            let rendered = match m.unit_text(&s) {
                Some(t) => t,
                // Windows carries the command in its registration argv rather
                // than in an on-disk unit; that is where the verb must appear.
                None => m.install_argv(&s).join(" "),
            };
            assert!(
                rendered.contains("gateway"),
                "{} unit must invoke the gateway verb: {rendered}",
                m.family()
            );
            assert!(
                rendered.contains("run"),
                "{} unit must invoke `gateway run`: {rendered}",
                m.family()
            );
        }

        // And `run` must actually be a parseable subcommand of this surface.
        let names = verb_names();
        assert!(
            names.iter().any(|n| n == "run"),
            "`gateway run` must exist — the service units invoke it: {names:?}"
        );
    }

    /// A unit-writing family whose unit path is under our control, so the
    /// registration question can be asked without a real service registry.
    struct UnitWritingFamily(PathBuf);
    impl wcore_gateway::service::ServiceManager for UnitWritingFamily {
        fn family(&self) -> &'static str {
            "test-unit-writing"
        }
        fn install_argv(&self, _: &ServiceSpec) -> Vec<String> {
            vec!["true".into()]
        }
        fn uninstall_argv(&self, _: &ServiceSpec) -> Vec<String> {
            vec!["true".into()]
        }
        fn start_argv(&self, _: &ServiceSpec) -> Vec<String> {
            vec!["true".into()]
        }
        fn stop_argv(&self, _: &ServiceSpec) -> Vec<String> {
            vec!["true".into()]
        }
        /// The activity query, and it FAILS — exactly as `systemctl --user
        /// is-active` does for an installed-but-stopped unit. If
        /// `is_registered` ever consults this again, the test reddens.
        fn status_argv(&self, _: &ServiceSpec) -> Vec<String> {
            vec!["false".into()]
        }
        fn unit_text(&self, _: &ServiceSpec) -> Option<String> {
            Some("unit".into())
        }
        fn unit_path(&self, _: &ServiceSpec) -> Option<PathBuf> {
            Some(self.0.clone())
        }
    }

    #[tokio::test]
    async fn an_installed_but_stopped_service_is_not_reported_uninstalled() {
        // F24-B-H2. The live journey caught this: during systemd's five-second
        // restart window after a hard kill, and again after a clean drain, the
        // status verb said `Uninstalled` about a unit that was on disk and
        // enabled, because the registration question was being answered by an
        // ACTIVITY query.
        let dir = tempfile::tempdir().unwrap();
        let unit = dir.path().join("registered.service");
        let spec = ServiceSpec {
            profile: "t".into(),
            binary: PathBuf::from("/opt/x/wayland-core"),
            home: dir.path().to_path_buf(),
        };

        let mgr = UnitWritingFamily(unit.clone());
        assert!(
            !is_registered(&mgr, &spec).await,
            "no unit on disk means not registered"
        );

        std::fs::write(&unit, "unit").unwrap();
        assert!(
            is_registered(&mgr, &spec).await,
            "a unit on disk IS a registration, even though the activity query fails"
        );
    }

    #[test]
    fn the_seven_lifecycle_verbs_are_all_present() {
        let names = verb_names();
        for v in [
            "install",
            "uninstall",
            "start",
            "stop",
            "restart",
            "status",
            "drain",
            "run",
        ] {
            assert!(names.iter().any(|n| n == v), "missing verb {v}: {names:?}");
        }
        // The recorded gap, asserted rather than described. If a later plan
        // adds `doctor` or `logs` this test reddens and the contract note in
        // this module's header must be updated with it.
        for gap in ["doctor", "logs"] {
            assert!(
                !names.iter().any(|n| n == gap),
                "`{gap}` is a RECORDED GAP — adding it means updating the module header and 24-B-GATEWAY-SURFACE.md"
            );
        }
    }
}
