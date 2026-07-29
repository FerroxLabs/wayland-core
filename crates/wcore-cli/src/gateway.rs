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
//! `support-bundle` was added afterwards and is NOT one of the nine. It exists
//! because Success Criterion 4 asks for *"useful redacted health/log/support
//! evidence"* and `wcore_gateway::support_bundle` had no operator surface at
//! all (`F24-C4-H1`) — a census returned one `pub mod` declaration and two
//! references inside the module's own test file, so the criterion's second half
//! was unreachable from the shipped binary while the gap ledger recorded the
//! criterion MET. It covers part of what `doctor` and `logs` were wanted for,
//! by SHIPPING the evidence rather than rendering it — which is the shape the
//! criterion actually asks for. See `gateway/support.rs`.
//!
//! # Where each verb's authority lives
//!
//! No policy is invented here. `install`/`uninstall`/`start`/`stop` render
//! their argv from `wcore_gateway::service::ServiceManager`, whose
//! `for_this_platform()` is the single platform-selection point in the
//! workspace; `status` renders `wcore_gateway::lifecycle::StatusProjection`;
//! `drain` drives `wcore_gateway::drain::DrainController` through the
//! `AutomationPlane`; `support-bundle` drives
//! `wcore_gateway::support_bundle::collect` and adds no redaction rule of its
//! own. This module is a surface, not a second implementation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

use wcore_gateway::lifecycle::{GatewayState, StatusProjection};
use wcore_gateway::pidlock::{PidLock, process_is_alive};
use wcore_gateway::service::{ServiceSpec, is_registerable_binary};

/// `support-bundle`. A child module rather than more of this file, which is
/// already past the 1000-line bound AGENTS.md sets — and a child module keeps
/// the declaration here instead of in `lib.rs`, which every lane shares.
mod support;

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
    /// List the deliveries this gateway GAVE UP on, and why.
    ///
    /// The gateway abandons a delivery in two situations: a shutdown drain ran
    /// out of budget, or an attempt's outcome became unknown against a
    /// destination that cannot recognise a replay (re-sending it would be a
    /// duplicate). Both are deliberate — but before this command existed
    /// neither left anything an operator could query. `Abandoned` is excluded
    /// from the pending list and from the pending count, and its only trace was
    /// a log line written by a process that had usually already exited.
    ///
    /// Reads the ledger journal on disk, so it answers whether or not a gateway
    /// is currently running — which matters, because the abandonment is most
    /// often recorded by a process that is now gone.
    Abandoned {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Emit JSON instead of the operator view.
        #[arg(long)]
        json: bool,
    },
    /// Record that you have reviewed an abandoned delivery and dealt with it.
    ///
    /// Until a delivery is acknowledged the gateway treats it as outstanding
    /// work and will NEVER compact its record away, however old it is. That is
    /// what makes `gateway abandoned` trustworthy: a message the product failed
    /// to deliver cannot age out of the journal before anybody sees it. The
    /// acknowledgement is your signature that it can.
    ///
    /// Nothing writes this automatically, including a successful re-send. A
    /// surface that empties itself is not a surface.
    Ack {
        #[command(flatten)]
        scope: ScopeArgs,
        /// The delivery id, exactly as `gateway abandoned` prints it.
        id: String,
    },
    /// Send an abandoned delivery again, through the channel it was bound for.
    ///
    /// The message body is not kept in the ledger; it is read back from the
    /// cron job the delivery id names. The original delivery key rides the send,
    /// so a destination that can recognise a replay will suppress this if the
    /// first copy did land — which is the safest possible re-send and costs
    /// nothing to do.
    ///
    /// Destinations that CANNOT recognise a replay are the reason the delivery
    /// was abandoned in the first place, so for anything that was already
    /// in flight this verb refuses without `--confirm-not-delivered`. Check the
    /// destination before you pass it.
    Resend {
        #[command(flatten)]
        scope: ScopeArgs,
        /// The delivery id, exactly as `gateway abandoned` prints it.
        id: String,
        /// Confirm you have checked the destination and the message is NOT
        /// there. Required whenever an attempt was already in flight when the
        /// delivery was abandoned, because that attempt may have landed and
        /// re-sending would put a second copy at the destination.
        #[arg(long)]
        confirm_not_delivered: bool,
        /// Acknowledge the abandonment as well, in one step. Explicit, because
        /// a re-send does not answer whether the destination now has two copies.
        #[arg(long)]
        ack: bool,
    },
    /// Collect redacted health, log and configuration evidence a support
    /// engineer can act on, into a directory you can read before you send it.
    ///
    /// Reads only from disk, so it answers whether or not a gateway is
    /// running — which is the point, since a bundle is wanted precisely when
    /// something has already failed.
    SupportBundle {
        #[command(flatten)]
        scope: ScopeArgs,
        /// Where to write the bundle. Must not already contain anything.
        /// Defaults to a timestamped directory inside the gateway home.
        #[arg(long)]
        out: Option<PathBuf>,
        /// Emit the manifest as JSON instead of the operator view.
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

    /// The gateway home, overriding `WAYLAND_HOME`.
    ///
    /// F24-J-H1: Windows Task Scheduler cannot set an environment variable on a
    /// registered task, so a Windows registration has no way to carry
    /// `WAYLAND_HOME` the way the launchd plist and the systemd unit both do.
    /// The registration therefore passes the home as an argument, and this is
    /// the flag it passes. It is on `ScopeArgs` rather than on `run` alone so
    /// an operator can point `status` and `drain` at the same home without
    /// exporting anything.
    #[arg(long)]
    pub home: Option<PathBuf>,
}

impl ScopeArgs {
    fn profile(&self) -> String {
        self.profile
            .clone()
            .or_else(|| std::env::var("WAYLAND_PROFILE").ok())
            .unwrap_or_else(|| "default".to_string())
    }

    /// The explicit flag wins over the environment, which wins over the
    /// default. A registration that named a home must not be silently
    /// redirected by whatever the service manager's environment happens to
    /// hold — that redirection is the defect this flag exists to close.
    fn home(&self) -> Result<PathBuf> {
        match &self.home {
            Some(path) => Ok(path.clone()),
            None => home(),
        }
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
        home: scope.home()?,
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
        GatewayCmd::Abandoned { scope, json } => abandoned(&scope, json),
        GatewayCmd::Ack { scope, id } => ack(&scope, &id),
        GatewayCmd::Resend {
            scope,
            id,
            confirm_not_delivered,
            ack: also_ack,
        } => resend(&scope, &id, confirm_not_delivered, also_ack).await,
        GatewayCmd::SupportBundle { scope, out, json } => {
            support::support_bundle(&scope, out, json).await
        }
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
    if let Some(path) = mgr.unit_path(&spec)
        && path.exists()
    {
        std::fs::remove_file(&path)
            .with_context(|| format!("cannot remove service unit {}", path.display()))?;
        println!("removed unit: {}", path.display());
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
/// The branch is on a CAPABILITY the trait exposes rather than on the
/// platform: a family whose on-disk unit IS its registration record has that
/// file as the answer, and a family whose unit is not (Windows now writes an
/// XML document, but Task Scheduler copies it into its own store at create
/// time and never reads it again) is asked its query verb, which for
/// `schtasks /query` genuinely answers registration rather than activity.
///
/// F24-J-H3 moved this off `unit_path().is_some()`. That inference was
/// correct only while Windows was the sole family without a unit file; once
/// it had one, presence-of-file would have reported `Registered` for a task
/// deleted out of band.
async fn is_registered(
    mgr: &dyn wcore_gateway::service::ServiceManager,
    spec: &ServiceSpec,
) -> bool {
    if mgr.unit_is_registration_record()
        && let Some(unit) = mgr.unit_path(spec)
    {
        return unit.exists();
    }
    let argv = mgr.status_argv(spec);
    run_argv(&argv)
        .await
        .map(|o| o.status.success())
        .unwrap_or(false)
}

async fn status(scope: &ScopeArgs, json: bool) -> Result<()> {
    let home = scope.home()?;
    let profile = scope.profile();
    let mgr = wcore_gateway::service::for_this_platform();

    let proj = match read_live_projection(&home) {
        Some(p) => p,
        None => {
            // Nothing is running. Distinguish "never installed" from
            // "installed and down": an operator debugging a service that will
            // not start needs to know the registration exists.
            let mut p = StatusProjection::stopped(&profile);
            if let Ok(spec) = spec(scope)
                && !is_registered(&*mgr, &spec).await
            {
                p.state = GatewayState::Uninstalled;
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
    let home = scope.home()?;
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
                if let Ok(raw) = std::fs::read_to_string(status_path(&home))
                    && let Ok(p) = serde_json::from_str::<StatusProjection>(&raw)
                {
                    println!(
                        "drain complete: {} (deliveries pending {})",
                        p.state, p.deliveries_pending
                    );
                    return Ok(());
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
// abandoned — "what did you give up on?"
// ---------------------------------------------------------------------------

/// List every abandonment still recorded in this home's delivery ledger.
///
/// Deliberately reads the JOURNAL rather than asking a running gateway. An
/// abandonment is usually written by a process that has since exited — the
/// forced-drain path runs during shutdown, and the unknown-outcome path runs
/// right after a crash — so a surface that required a live gateway would be
/// unavailable in exactly the cases it exists for.
fn abandoned(scope: &ScopeArgs, json: bool) -> Result<()> {
    let home = scope.home()?;
    let ledger = wcore_gateway::ledger::DeliveryLedger::open(&home)
        .with_context(|| format!("cannot read the delivery ledger in {}", home.display()))?;

    let found = ledger.abandoned();
    let dropped = ledger.dropped_abandonments();

    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "home": home.display().to_string(),
                "abandoned": found,
                "unacknowledged": ledger.unacknowledged_abandoned_count(),
                "dropped_past_retention": dropped,
                "quarantined": ledger.quarantined(),
            }))?
        );
        return Ok(());
    }

    if found.is_empty() {
        println!("No abandoned deliveries recorded in {}.", home.display());
    } else {
        println!(
            "{} abandoned {} in {}:",
            found.len(),
            if found.len() == 1 {
                "delivery"
            } else {
                "deliveries"
            },
            home.display()
        );
        for a in &found {
            println!();
            println!("  {}", a.id);
            println!(
                "    to:     {}",
                a.destination.as_deref().unwrap_or("(not recorded)")
            );
            println!("    when:   {}", a.at);
            match a.reason {
                Some(r) => println!("    why:    {}", r.describe()),
                // A record written before the reason was persisted. Named as
                // unknown rather than guessed — the two reasons call for
                // opposite operator actions, so inventing one would be worse
                // than admitting the gap.
                None => println!("    why:    (not recorded — pre-dates reason tracking)"),
            }
            // Whether re-sending this can duplicate is the operator's next
            // question, so it is answered here rather than left to be
            // discovered when `gateway resend` refuses.
            match a.was_attempted {
                Some(false) => println!(
                    "    resend: safe — no attempt had started, so it cannot be at the \
                     destination"
                ),
                Some(true) => println!(
                    "    resend: CHECK THE DESTINATION FIRST — an attempt was in flight and \
                     may have landed"
                ),
                None => println!(
                    "    resend: CHECK THE DESTINATION FIRST — this record pre-dates attempt \
                     tracking, so whether it landed is unknown"
                ),
            }
            if let Some(r) = &a.resent {
                println!("    resent: {r}");
            }
            match &a.acknowledged {
                Some(t) => println!("    acked:  {t}"),
                None => println!("    acked:  no — retained until `gateway ack {}`", a.id),
            }
        }
    }

    // The count that has to stay small. Exempting unacknowledged abandonments
    // from compaction means the journal's bound is review rather than a cap, so
    // an unreviewed backlog must be loud rather than quietly truncated.
    let unacked = ledger.unacknowledged_abandoned_count();
    if unacked > 0 {
        println!();
        println!(
            "{unacked} abandonment(s) are UNACKNOWLEDGED and are exempt from compaction \
             until they are. Review each, then `gateway ack <id>`."
        );
    }

    // Never silent about its own incompleteness.
    if dropped > 0 {
        println!();
        println!(
            "WARNING: {dropped} further abandonment(s) were dropped by compaction past the \
             retention cap of {}. Those deliveries can no longer be named.",
            wcore_gateway::ledger::ABANDON_RETENTION
        );
    }
    if ledger.quarantined() > 0 {
        println!();
        println!(
            "WARNING: {} unparsable journal record(s) were quarantined on load; this list \
             may be incomplete.",
            ledger.quarantined()
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ack / resend — disposing of an abandonment
// ---------------------------------------------------------------------------

/// Open this home's ledger, naming the home in any failure.
fn open_ledger(scope: &ScopeArgs) -> Result<(PathBuf, wcore_gateway::ledger::DeliveryLedger)> {
    let home = scope.home()?;
    let ledger = wcore_gateway::ledger::DeliveryLedger::open(&home)
        .with_context(|| format!("cannot read the delivery ledger in {}", home.display()))?;
    Ok((home, ledger))
}

/// Whether re-sending this abandonment could put a SECOND copy at the
/// destination, and therefore needs the operator to confirm it checked.
///
/// The only safe case is a delivery that provably never left: `Some(false)`,
/// abandoned before any attempt started. `Some(true)` was in flight and may have
/// landed. `None` is a record written before attempt tracking existed, and is
/// read the SAME as `Some(true)` — deliberately. An unknown here means the
/// journal cannot say whether the message landed, and resolving an unknown in
/// favour of "just send it" is exactly the duplicate the abandonment was written
/// to prevent.
fn resend_needs_confirmation(was_attempted: Option<bool>) -> bool {
    was_attempted != Some(false)
}

/// Mark an abandonment reviewed, so compaction may eventually retire it.
fn ack(scope: &ScopeArgs, id: &str) -> Result<()> {
    let (home, mut ledger) = open_ledger(scope)?;
    let already = ledger
        .abandoned()
        .into_iter()
        .find(|a| a.id == id)
        .and_then(|a| a.acknowledged);

    // The ledger refuses an id that is not abandoned; the message names the
    // surface that lists the valid ones rather than leaving the operator to
    // guess whether they typed the id wrong or the delivery is simply live.
    ledger.acknowledge(id).with_context(|| {
        format!(
            "cannot acknowledge {id} in {}; `gateway abandoned` lists what can be",
            home.display()
        )
    })?;
    ledger.flush().context("cannot flush the delivery ledger")?;

    match already {
        Some(at) => println!("{id} was already acknowledged at {at}; left unchanged."),
        None => println!(
            "Acknowledged {id}. It is now eligible for compaction once the journal passes \
             its retention cap."
        ),
    }
    Ok(())
}

/// Re-send an abandoned delivery through the channel it was bound for.
///
/// The whole point of the ledger is that it does NOT hold message bodies, so
/// this reconstructs the message from the cron job the delivery id names. That
/// is also why the re-send is honest about what it can do: a delivery whose job
/// has since been edited or deleted cannot be re-sent, and this says so rather
/// than sending something that was never scheduled.
async fn resend(scope: &ScopeArgs, id: &str, confirmed: bool, also_ack: bool) -> Result<()> {
    let (home, mut ledger) = open_ledger(scope)?;

    let record = ledger
        .abandoned()
        .into_iter()
        .find(|a| a.id == id)
        .ok_or_else(|| {
            anyhow::anyhow!(
                "no abandoned delivery {id} in {}; `gateway abandoned` lists what there is",
                home.display()
            )
        })?;

    if resend_needs_confirmation(record.was_attempted) && !confirmed {
        bail!(
            "{id} was already in flight when it was abandoned, so it may have reached {}. \
             Re-sending could put a second copy there.\n\n  {}\n\nCheck the destination. If \
             the message is NOT there, re-run with --confirm-not-delivered.",
            record.destination.as_deref().unwrap_or("its destination"),
            record
                .reason
                .map(|r| r.describe().to_string())
                .unwrap_or_else(|| "(reason not recorded)".to_string()),
        );
    }

    // The body lives with the schedule, not with the ledger.
    let store = wcore_cron::FileCronStore::new(
        wcore_gateway::automation::AutomationPlane::schedule_dir(&home).join("jobs.json"),
    );
    let jobs = wcore_cron::CronStore::list(&store)
        .await
        .context("cannot read the schedule this delivery came from")?;
    // Matched by prefix rather than by splitting on ':', because a job id may
    // itself contain a colon and the delivery id is `cron:{job}:{millis}[:{occ}]`.
    let job = jobs
        .iter()
        .filter(|j| id.starts_with(&format!("cron:{}:", j.id)))
        // Longest id wins, so one job id that is a prefix of another cannot
        // shadow it.
        .max_by_key(|j| j.id.len())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "the job that produced {id} is no longer in the schedule, so its message \
                 cannot be reconstructed. The ledger deliberately does not keep bodies."
            )
        })?;

    let (channel_name, text) = match &job.target {
        wcore_cron::Target::Channel { channel_name, text } => (channel_name.clone(), text.clone()),
        other => bail!(
            "job {} no longer targets a channel ({other:?}), so {id} cannot be re-sent",
            job.id
        ),
    };

    // Real adapters, from this home's own channel configuration — the same
    // registration path `gateway run` uses. A re-send that went through a
    // different code path would not be a re-send.
    let mut manager = wcore_channels_registry::wcore_channels::ChannelManager::new();
    let config = crate::channel::resolve_config_for_credentials()
        .context("cannot resolve the configuration that holds channel credentials")?;
    let creds: Arc<dyn wcore_config::credentials::CredentialsStore> = Arc::from(
        config
            .open_credentials_store()
            .map_err(|e| anyhow::anyhow!("cannot open the credentials store: {e}"))?,
    );
    let registered = wcore_channels_registry::auto_register_from_dir(
        &mut manager,
        &home.join("channels"),
        creds,
    )
    .await
    .context("cannot register channels for the re-send")?;
    if registered == 0 {
        bail!(
            "no channels are registered in {}, so {id} cannot be re-sent",
            home.join("channels").display()
        );
    }
    // Registering an adapter does NOT connect it. Without this the send fails
    // with "channel not started" — measured on the first live run of this verb,
    // against a real gateway home and a real destination, and invisible to every
    // unit test because they drive the adapters directly.
    manager
        .start_all()
        .await
        .map_err(|e| anyhow::anyhow!("cannot start the channels for the re-send: {e}"))?;

    // The ORIGINAL delivery key rides the send. On a destination that can
    // recognise a replay this makes the re-send free of risk: if the first copy
    // did land, the destination suppresses this one. On a destination that
    // cannot, it changes nothing — and that is precisely the case
    // `--confirm-not-delivered` exists to gate.
    let dedupes = manager.supports_outbound_idempotency(&channel_name).await;
    let msg = wcore_channels_registry::wcore_channels::OutgoingMessage::text(
        channel_name.clone(),
        text.clone(),
    );
    let receipt = manager
        .send_to_keyed(&channel_name, msg, Some(id))
        .await
        .with_context(|| format!("re-send of {id} to {channel_name} failed"))?;

    // Recorded only after the send actually returned a receipt. A re-send noted
    // before the send would be the same false-success the abandonment exists to
    // avoid.
    ledger
        .mark_resent(id)
        .context("the message was re-sent but the ledger could not record it")?;
    if also_ack {
        ledger
            .acknowledge(id)
            .context("the message was re-sent but the acknowledgement could not be recorded")?;
    }
    ledger.flush().context("cannot flush the delivery ledger")?;

    println!("Re-sent {id} to {channel_name}.");
    println!("  receipt:      {}", receipt.id);
    println!(
        "  replay-safe:  {}",
        if dedupes {
            "yes — the destination honours the delivery key, so a landed first copy \
             suppressed this one"
        } else {
            "no — this destination cannot recognise a replay; if the first copy did land, \
             there are now two"
        }
    );
    println!(
        "  abandonment:  still listed{}",
        if also_ack {
            ", acknowledged"
        } else {
            " and UNACKNOWLEDGED — `gateway ack` it once you have confirmed the outcome"
        }
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// run — the runtime every generated service unit invokes
// ---------------------------------------------------------------------------

/// One observation interval of a drain.
///
/// F24-B-H4 (HIGH), found by the live Linux journey and NOT by any test.
/// `DrainController::drain`'s injected clock is documented as returning
/// **total** elapsed milliseconds — the loop compares the return value
/// against the whole budget. The first version of this closure returned the
/// per-iteration increment (a constant `100`), so `elapsed` was pinned at
/// 100, never reached the budget, and the loop could only exit through its
/// other condition: nothing left pending. With work pending that never
/// settled, `gateway drain` HUNG in `Draining` indefinitely.
///
/// It passed the first live journey because that gateway had zero pending
/// deliveries, so the loop broke on its first observation. The defect is
/// only reachable with real carried work, which is exactly why it survived
/// a green suite and a green journey.
fn drain_wait(
    started: std::time::Instant,
    ledger: &mut wcore_gateway::ledger::DeliveryLedger,
) -> u64 {
    // Durable before each observation: the report must never claim a count
    // that is not yet on disk.
    let _ = ledger.flush();
    std::thread::sleep(std::time::Duration::from_millis(100));
    // TOTAL elapsed, per the controller's contract. Returning the increment
    // here is the bug above.
    started.elapsed().as_millis() as u64
}

/// Assemble the channel-health document a second process reads.
///
/// `configured` is counted from the config DIRECTORY here rather than taken
/// from the manager, so the two numbers in the published report come from two
/// independent places and a disagreement between them is detectable. A report
/// that sourced both from the manager could only ever agree with itself.
fn channel_health_report(
    home: &Path,
    registered: usize,
    registration_error: &Option<String>,
    channels: &wcore_channels_registry::wcore_channels::ChannelManager,
) -> crate::channel::ChannelHealthReport {
    crate::channel::ChannelHealthReport {
        configured: crate::channel::configured_count(home),
        registered,
        registration_error: registration_error.clone(),
        channels: channels.health(),
    }
}

/// Compose the single `registration_error` string `channel health` fails on out
/// of the three independent things that can degrade inbound.
///
/// F24-C3-H6a. These were previously one accumulated variable, which is why a
/// `channel reload` — an operation that re-evaluates ONLY adapter registration —
/// could clear the other two by assigning `None` to the lot. They are separated
/// here by WHO ESTABLISHES THEM and WHEN, because that is what decides whether a
/// given event is entitled to clear one:
///
/// - `registration` — adapter registration and the credentials store. Redone by
///   every reload, so a reload legitimately clears it.
/// - `inbound_absent` — this process built no inbound stack. Decided once, at
///   startup, and nothing rebuilds it; true for the process's life.
/// - `not_polling` — this process is not the inbound poller RIGHT NOW. Read live
///   from the supervisor at every publish, never cached, because the supervisor
///   re-claims each tick and this can go from true to false without anything
///   else happening. Caching it produced a permanently-red health surface; see
///   LANE-BRIEF 3b-iii.
fn compose_registration_error(
    registration: &Option<String>,
    inbound_absent: &Option<String>,
    not_polling: bool,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if let Some(e) = inbound_absent {
        parts.push(e.clone());
    }
    if not_polling {
        parts.push("inbound polling owned by another process".to_string());
    }
    if let Some(e) = registration {
        parts.push(e.clone());
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("; "))
    }
}

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
    let home = scope.home()?;

    // F24-J-H2. `--home` is a NARROWER carrier than the environment variable
    // the Unix units set, and the difference is not cosmetic. The launchd plist
    // and the systemd unit both export `WAYLAND_HOME`, which scopes the gateway
    // home AND everything `wcore_config::wayland_config_dir` resolves under it
    // — config, and with it the credentials store. Task Scheduler cannot set an
    // environment variable, so the Windows registration passes `--home`, which
    // scoped only the gateway's own files.
    //
    // Measured live on the real box at d89b81b6: the gateway came up in the
    // right home and published a correct projection, then every delivery failed
    // with `no value for credential handle "slack.f24j.bot_token"` because the
    // credentials store had resolved under `%APPDATA%\wayland-core` while the
    // credentials file sat in the home the task was registered for. Twelve
    // submitted, zero arrived.
    //
    // So the flag exports what the units export, and the one carrier scopes the
    // whole process on every platform rather than two thirds of it on one.
    if scope.home.is_some() && std::env::var_os("WAYLAND_HOME").is_none() {
        // SAFETY: this runs before any configuration is read and before the
        // gateway spawns any work, so no other thread is reading the
        // environment concurrently. It is also a no-op whenever the variable is
        // already set, so a unit that exports it keeps authority over a flag.
        unsafe { std::env::set_var("WAYLAND_HOME", &home) };
    }

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
    let history =
        wcore_gateway::automation::AutomationPlane::schedule_dir(&home).join("history.jsonl");

    // F24-C3-H4. THE CHANNEL STACK IS BUILT BEFORE THE AUTOMATION PLANE, and
    // that ordering is the fix, not a tidy-up.
    //
    // This function used to build the cron handler here via
    // `build_headless_cron_handler(&cwd)`, which constructs its OWN
    // `ChannelManager`, auto-registers every adapter into it and calls
    // `start_all()` on it — and then, sixty lines below, registered the same
    // adapters a second time into the manager the gateway keeps. One process,
    // two managers, two poll loops per account, six registration events for
    // three channels, and a subscriber on only one of the two.
    //
    // For the three webhook adapters that was merely wasteful. For POLLING
    // adapters it is a consumption race with no error path: `getUpdates`
    // confirming an offset deletes the update server-side and IMAP's `\Seen`
    // is likewise destructive, so the manager WITHOUT a subscriber can take
    // delivery of a message that then reaches nobody. Nothing logs, nothing
    // fails, the message is simply gone.
    //
    // So the manager is now built, registered, subscribed and started FIRST,
    // and the cron handler is handed that same `Arc` instead of making one.
    // The plane follows, because `plane.resume()` dispatches carried
    // deliveries through the channel sink and adapters that resolve their
    // credential in `start()` cannot send before `start_all` has run.
    //
    // F24-03. The gateway hosts the channel adapters and REPUBLISHES their
    // observed health every tick, because `wayland-core channel health` runs
    // in a different process and would otherwise have to fabricate an answer
    // by starting its own adapters — describing channels eight milliseconds
    // old instead of the ones carrying traffic. Every failure here is
    // non-fatal: a gateway with no channels still runs the schedule, and
    // taking the whole runtime down because one adapter's credential is
    // missing would be a worse outcome than a Disconnected row in `health`.
    //
    // Non-fatal is NOT the same as unreported. F24-D-H2: the first live run
    // registered zero channels because the credentials store would not open,
    // published an empty list, and `channel health` rendered that as "you have
    // no channels". Every failure below is therefore carried into
    // `registration_error` and surfaced against an independently-counted
    // `configured`.
    let mut channel_manager = wcore_channels_registry::wcore_channels::ChannelManager::new();
    let channels_dir = home.join("channels");
    let mut registration_error: Option<String> = None;
    // F24-C3-H6a. The subset of `registration_error` that a `channel reload`
    // does NOT re-evaluate and CANNOT repair, so it must survive one.
    //
    // `registration_error` is the only field `channel health` fails on once
    // `registered >= configured`, and the reload success path used to clear it
    // to `None` outright. Reload re-runs adapter registration and nothing else,
    // so clearing the whole field also erased two process-lifetime facts —
    // that this process has no inbound stack at all, and that it lost the
    // inbound polling lease. Measured on the shipped binary: `channel health`
    // exited 1 naming the lost lease, one `channel reload` later it exited 0,
    // and nothing about the dead path had changed.
    //
    // An operator running the documented recovery command was told the
    // degradation was gone. That is worse than not reporting it, because it
    // actively retires the operator's suspicion.
    let mut persistent_error: Option<String> = None;
    let mut registered_n = 0usize;
    let mut gateway_config: Option<wcore_config::config::Config> = None;
    match crate::channel::resolve_config_for_credentials().and_then(|c| {
        c.open_credentials_store()
            .map_err(|e| anyhow::anyhow!("{e}"))
            .map(|s| (c, s))
    }) {
        Ok((resolved, store)) => {
            gateway_config = Some(resolved);
            let creds: Arc<dyn wcore_config::credentials::CredentialsStore> = Arc::from(store);
            match wcore_channels_registry::auto_register_from_dir(
                &mut channel_manager,
                &channels_dir,
                creds,
            )
            .await
            {
                Ok(n) => {
                    registered_n = n;
                    eprintln!("[gateway] channels registered={n}");
                }
                Err(e) => {
                    eprintln!("[gateway] channel registration failed: {e}");
                    registration_error = Some(e.to_string());
                }
            }
        }
        Err(e) => {
            eprintln!("[gateway] credentials store unavailable, channels disabled: {e}");
            registration_error = Some(format!("credentials store unavailable: {e}"));
        }
    }

    // F24-C3-H2. Everything above this point existed before, and produced a
    // gateway that polled its adapters and dropped every inbound event: it
    // constructed no `InboundSubscriber` on the manager's broadcast and no
    // inbound webhook host. Both lived only in `AgentBootstrap`, which the
    // gateway does not use, so inbound dispatch was opted into at exactly three
    // interactive call sites and `gateway run` — the systemd unit, the launchd
    // plist, the scheduled task — was not one of them.
    //
    // Measured against the running gateway at `e88cf43f`, not read off the
    // source: process alive, `[inbound_webhook] enabled = true` in its own
    // config, and a request to the configured bind got ECONNREFUSED. A config
    // key that reads `enabled = true` over a socket nobody is listening on is
    // the same silent-false-advertising defect as a trigger that never fires.
    //
    // The manager is lifted to `Arc<RwLock<..>>` because both the subscriber
    // and the webhook host hold it for the life of the process.
    let channels = Arc::new(tokio::sync::RwLock::new(channel_manager));

    // ORDER IS LOAD-BEARING. The subscriber acquires its broadcast receiver in
    // `spawn`, and tokio's broadcast drops events published before a receiver
    // exists. `start_all` therefore runs AFTER this, not before — arming the
    // poll loops first would lose every message that arrived in the gap.
    let inbound_host = match &gateway_config {
        Some(config) => {
            match wcore_agent::channel_inbound_host::spawn(
                Arc::clone(&channels),
                config,
                cwd.clone(),
            )
            .await
            {
                Ok(host) => {
                    match &host.webhook_bind {
                        Some(bind) => eprintln!(
                            "[gateway] inbound: subscriber spawned, webhook host listening bind={bind} policies={}",
                            host.policies_loaded
                        ),
                        None => eprintln!(
                            "[gateway] inbound: subscriber spawned, webhook host disabled policies={}",
                            host.policies_loaded
                        ),
                    }
                    Some(host)
                }
                // The refusal. `[inbound_webhook] enabled = true` is an explicit
                // operator opt-in; if the gateway cannot serve it, starting
                // healthy and listening on nothing is precisely the defect. So
                // it refuses, and the error names what is unsupported.
                //
                // With the webhook NOT enabled the same failure is degraded
                // rather than fatal — a gateway with no model still runs its
                // schedule — but it is carried into `registration_error`, which
                // `channel health` reads, rather than left as a log line.
                Err(e) if config.inbound_webhook.enabled => {
                    return Err(anyhow::anyhow!(
                        "gateway refusing to start: [inbound_webhook] enabled = true but \
                         this runtime cannot host inbound. {e}"
                    ));
                }
                Err(e) => {
                    eprintln!("[gateway] inbound dispatch unavailable: {e}");
                    // F24-C3-H6a. Recorded as PERSISTENT, not as a registration
                    // error. The inbound host is built exactly once, here, and no
                    // reload rebuilds it — so this is true for the life of the
                    // process and a reload is not entitled to clear it. It is
                    // deliberately not also folded into `registration_error`, or
                    // the composed health message would carry it twice.
                    persistent_error = Some(match persistent_error.take() {
                        Some(prev) => format!("{prev}; inbound dispatch unavailable: {e}"),
                        None => format!("inbound dispatch unavailable: {e}"),
                    });
                    None
                }
            }
        }
        None => {
            // No resolvable config means no credentials store either, so no
            // adapter registered and there is nothing to receive on. Already
            // carried into `registration_error` above.
            None
        }
    };

    // F24-CL. The single-owner INBOUND POLLING lease.
    //
    // F24-C3-H4 stopped THIS process building two managers. It could not stop a
    // second PROCESS building one — and both an ordinary session and the
    // `cron daemon` do exactly that, against the same `<home>/channels`. Polling
    // is a destructive read (Telegram's `offset=` confirm deletes; IMAP sets
    // `\Seen`), so the loser of that race does not see a duplicate, it sees
    // nothing at all. Measured 8 of 8 lost at startup on the shipped binary.
    //
    // Bound to the gateway's own lifetime deliberately: it is released by the
    // OS when this process dies, however it dies, so a killed gateway hands
    // inbound polling straight to the next process instead of wedging it.
    let poll_lease = wcore_agent::channel_lease::attempt(&home, "gateway");
    if !poll_lease.is_owner() {
        eprintln!(
            "[gateway] inbound polling is owned by another process (pid {:?}); \
             this gateway will send but not poll",
            poll_lease.owner_pid()
        );
        // NOT folded into `registration_error`. F24-C3-H6a: the lease is not a
        // fixed fact. `ChannelPollSupervisor` below re-claims every tick and
        // wins as soon as the current holder exits, so a boot-time string
        // frozen into the health document would go PERMANENTLY red — reporting
        // a degradation that had resolved and that nothing could ever clear.
        // The lease component is therefore recomputed from the supervisor's
        // LIVE role at every publish; see `lease_note`.
    }

    if registered_n > 0
        && poll_lease.is_owner()
        && let Err(e) = channels.write().await.start_all().await
    {
        eprintln!("[gateway] channel start_all: {e}");
        registration_error = Some(match registration_error.take() {
            Some(prev) => format!("{prev}; start_all: {e}"),
            None => format!("start_all: {e}"),
        });
    }

    // F24-CS. The gateway is the INSTALLED, always-on role, so it outranks both
    // an ordinary session and the cron daemon. Ownership is no longer decided
    // once at boot: if a session got to the lease first, this supervisor claims
    // it and the session stands down; if this gateway is ever the loser it
    // keeps claiming until it wins.
    //
    // Bound to the gateway's own lifetime deliberately, exactly as the bare
    // lease was: dropping it releases the OS lock, and the OS releases it
    // anyway however this process dies.
    //
    // Held rather than named `_`: `_` would drop it immediately, releasing the
    // lease at the top of `gateway run` and reopening the race.
    //
    // Named (no longer `_poll_supervisor`) because F24-C3-H6 reads its LIVE
    // ownership on every tick — both to decide whether a reload may start poll
    // tasks, and to report the current lease role in `channel health`.
    let poll_supervisor = wcore_agent::channel_lease::ChannelPollSupervisor::spawn(
        &home,
        "gateway",
        poll_lease,
        wcore_agent::channel_lease::ChannelManagerPollControl::new(Arc::clone(&channels)),
    );

    // The REAL headless handler, not a recorder. A gateway whose dispatch is
    // a log line proves its own loop and nothing about delivery.
    //
    // F24-C3-H4: it ADOPTS the manager built above rather than constructing a
    // second one. `..._with_channels(cwd, Some(arc))` registers nothing and
    // starts nothing — this call site owns the manager's whole lifecycle
    // (registration, `start_all`, the reload below, shutdown) and the cron
    // handler only borrows it as a send path. Channel cron jobs therefore
    // dispatch through the SAME adapters the subscriber is listening on, which
    // is also what makes a cron fire and an inbound reply observable as one
    // conversation instead of two.
    let handler: Arc<dyn wcore_cron::JobHandler> = Arc::new(
        wcore_agent::cron::build_headless_cron_handler_with_channels(
            &cwd,
            Some(Arc::clone(&channels)),
        )
        .await,
    );

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

    let _ = crate::channel::publish_health(
        &home,
        &channel_health_report(
            &home,
            registered_n,
            &compose_registration_error(
                &registration_error,
                &persistent_error,
                !poll_supervisor.is_owner(),
            ),
            &*channels.read().await,
        ),
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
                // A reload request is honoured BEFORE the tick, so a tick that
                // dispatches through a channel uses the adapter set the
                // operator just asked for rather than the previous one.
                if std::fs::remove_file(crate::channel::channel_reload_path(&home)).is_ok() {
                    match crate::channel::resolve_config_for_credentials()
                        .and_then(|c| {
                            c.open_credentials_store().map_err(|e| anyhow::anyhow!("{e}"))
                        }) {
                        Ok(store) => {
                            let creds: Arc<dyn wcore_config::credentials::CredentialsStore> =
                                Arc::from(store);
                            // Build the DESIRED set from disk, then hand it to
                            // `reload`, which keeps the running instance of any
                            // adapter whose configuration did not change.
                            let mut staging = wcore_channels_registry::wcore_channels::ChannelManager::new();
                            match wcore_channels_registry::auto_register_from_dir(
                                &mut staging,
                                &channels_dir,
                                creds,
                            )
                            .await
                            {
                                Ok(_) => {
                                    let desired = staging.take_registered().await;
                                    // One write guard covers reload + recount so
                                    // a health republish on the same tick cannot
                                    // read a half-swapped adapter set. The
                                    // subscriber and webhook host hold the SAME
                                    // Arc, so a reloaded adapter is delivered
                                    // through the already-running inbound stack
                                    // without re-spawning it.
                                    // F24-C3-H6b. The start decision is stated
                                    // here rather than left to `reload`,
                                    // because the right to poll belongs to the
                                    // lease and `ChannelManager` cannot see it.
                                    // The STARTUP path gates `start_all` on lease
                                    // ownership; this is the same gate on the
                                    // same decision, which is why it must not be
                                    // a default hidden inside `reload`. Read from
                                    // the SUPERVISOR, not the boot lease, so a
                                    // gateway that has since won the lease does
                                    // start polling and one that has not does
                                    // not.
                                    let start_policy = if poll_supervisor.is_owner() {
                                        wcore_channels_registry::wcore_channels::StartPolicy::StartNewlyRegistered
                                    } else {
                                        wcore_channels_registry::wcore_channels::StartPolicy::LeaveStopped
                                    };
                                    let (report, names) = {
                                        let mut guard = channels.write().await;
                                        let report =
                                            guard.reload(desired, start_policy).await;
                                        let names = guard.list_names().len();
                                        (report, names)
                                    };
                                    registered_n = names;
                                    // F24-C3-H6a. Clears ONLY the adapter
                                    // registration error, which is the only
                                    // thing this block re-evaluated. It used to
                                    // clear the whole composed health error,
                                    // which also erased "this process has no
                                    // inbound stack" and "this process is not
                                    // the poller" — neither of which a reload
                                    // establishes anything about. Those two are
                                    // now separate and are recomposed at publish
                                    // time, so this assignment cannot reach
                                    // them.
                                    registration_error = None;

                                    // F24-C3-H5. Re-registering the ADAPTER is
                                    // only half a reload. The inbound access
                                    // policy and the tool posture were both read
                                    // once at startup and were unreachable
                                    // afterwards, so a channel added here was
                                    // absent from them, fell through to the
                                    // fail-closed `InboundPolicy::default` — an
                                    // empty allowlist — and had every message
                                    // silently denied. Meanwhile `channel
                                    // health` reported it `healthy`, the
                                    // registration count said it was there, and
                                    // its webhook answered 200. Three surfaces,
                                    // all wrong, and no error anywhere.
                                    //
                                    // Measured with a one-variable control: the
                                    // identical config from the identical
                                    // generator was ADMITTED when present at
                                    // startup and DENIED when introduced by
                                    // reload.
                                    //
                                    // `reload_policies` refreshes the policy and
                                    // the posture in ONE swap. Refreshing only
                                    // the policy would let messages arrive under
                                    // the dispatcher's fallback posture instead
                                    // of the configured one — a passing test
                                    // over the wrong permissions, which is worse
                                    // than the fail-closed defect it replaces.
                                    //
                                    // Nothing is torn down: the subscriber and
                                    // the webhook host hold the same registry
                                    // Arc, so the swap is visible on the very
                                    // next inbound event.
                                    let policies_note = match inbound_host
                                        .as_ref()
                                        .map(|h| h.reload_policies())
                                    {
                                        Some(Ok(n)) => n.to_string(),
                                        // The policy files did not parse. The
                                        // registry kept what it had rather than
                                        // revoking every running channel, and
                                        // the operator is told — silently
                                        // serving stale policy is how a
                                        // half-applied reload becomes the next
                                        // finding.
                                        Some(Err(e)) => {
                                            registration_error = Some(format!(
                                                "channel reload: adapters reloaded but inbound \
                                                 policies did NOT: {e}. The previously loaded \
                                                 policies are still in effect, so a newly added \
                                                 channel will deny every message until this is \
                                                 fixed and reload is run again."
                                            ));
                                            format!("KEPT-STALE ({e})")
                                        }
                                        // No inbound stack in this process (no
                                        // provider, or config absent). Said out
                                        // loud rather than printed as `0`,
                                        // which would read as "the reload found
                                        // no policies".
                                        None => "no-inbound-host".to_string(),
                                    };

                                    eprintln!(
                                        "[gateway] channel reload: added={:?} replaced={:?} removed={:?} unchanged={:?} policies={}",
                                        report.added,
                                        report.replaced,
                                        report.removed,
                                        report.unchanged,
                                        policies_note
                                    );
                                }
                                Err(e) => {
                                    eprintln!("[gateway] channel reload failed: {e}");
                                    registration_error = Some(e.to_string());
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("[gateway] channel reload: credentials store: {e}");
                            registration_error =
                                Some(format!("credentials store unavailable: {e}"));
                        }
                    }
                }
                if let Err(e) = plane.tick(chrono::Utc::now()).await {
                    eprintln!("[gateway] tick error: {e}");
                }
                publish(&home, &project(&plane, plane.state()))?;
                // Republished on the SAME tick as the projection so the two
                // surfaces can never disagree about when they were observed.
                // The lease component is read from the supervisor HERE rather
                // than from a cached boot value, so it tracks the live role in
                // both directions (F24-C3-H6a).
                let health_error = compose_registration_error(
                    &registration_error,
                    &persistent_error,
                    !poll_supervisor.is_owner(),
                );
                let _ = crate::channel::publish_health(
                    &home,
                    &channel_health_report(
                        &home,
                        registered_n,
                        &health_error,
                        &*channels.read().await,
                    ),
                );
            }
        }
    }

    // The drain path is the SAME whether the trigger was a signal or a
    // request file. A stop that skipped the drain would abandon deliveries
    // without recording that it had.
    let budget = drain_budget.unwrap_or(30_000);
    let started_drain = std::time::Instant::now();
    let report = plane
        .drain_and_release(budget, |ledger| drain_wait(started_drain, ledger))
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
    if !report.abandoned.is_empty() {
        // This stderr listing is ephemeral — it exists only in the terminal of
        // the invocation that caused it, and a service-managed drain has no
        // such terminal. Point at the durable surface so the operator can find
        // these again afterwards.
        eprintln!(
            "[gateway] these are recorded durably; list them later with \
             `wayland-core gateway abandoned`"
        );
    }

    // Channels stop with the runtime. The published health is then REMOVED
    // rather than left describing a set of adapters that no longer exists —
    // `channel health` already refuses on a dead pid, and leaving a file
    // behind gives a second reader a chance to get it wrong.
    // The inbound stack is torn down BEFORE the adapters stop, so the webhook
    // host stops accepting POSTs while the connectors that would have to send
    // the replies are still alive. Stopping the adapters first would leave a
    // window in which an accepted inbound message could never be answered.
    if let Some(host) = inbound_host {
        host.shutdown();
    }
    let _ = channels.write().await.stop_all().await;
    let _ = std::fs::remove_file(crate::channel::channel_health_path(&home));

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

    // -----------------------------------------------------------------------
    // F24-C3-H6a — what a reload is and is not entitled to clear.
    //
    // These are unit-level and deliberately NOT the proof of the finding; the
    // proof is the driven run in
    // `scripts/f24-c3-h6-reload-clears-error.sh`, which reads `channel
    // health`'s real exit code across a real reload of a real gateway. What
    // these pin down is the SEPARATION the fix depends on, so a later edit that
    // re-merges the three components fails here and not only in a live run.
    // -----------------------------------------------------------------------

    /// The regression, expressed on the composer: clearing the registration
    /// component must NOT clear a live lease degradation.
    ///
    /// Before the fix a reload assigned `None` to one accumulated variable that
    /// held all three facts, and `channel health` went from failing to passing
    /// without anything about the dead path changing.
    #[test]
    fn clearing_the_registration_error_leaves_a_live_lease_degradation_reported() {
        // A reload has just succeeded, so the registration component is gone.
        let after_reload = compose_registration_error(&None, &None, true);
        assert_eq!(
            after_reload.as_deref(),
            Some("inbound polling owned by another process"),
            "a reload cleared the report of a poll lease it never re-attempted"
        );
        // ... and the health surface must therefore still be failing.
        assert!(
            !crate::channel::ChannelHealthReport {
                configured: 1,
                registered: 1,
                registration_error: after_reload,
                channels: Vec::new(),
            }
            .is_complete(),
            "registered >= configured, so this is the ONLY thing that can fail; \
             if it passes, `channel health` reports a non-polling gateway as complete"
        );
    }

    /// The same for an absent inbound stack, which no reload rebuilds.
    #[test]
    fn clearing_the_registration_error_leaves_an_absent_inbound_stack_reported() {
        let absent = Some("inbound dispatch unavailable: no provider".to_string());
        let after_reload = compose_registration_error(&None, &absent, false);
        assert_eq!(
            after_reload.as_deref(),
            Some("inbound dispatch unavailable: no provider")
        );
    }

    /// CAN IT PASS? (LANE-BRIEF 3b-iii.) The composed error must reach `None`
    /// in the achievable state where nothing is degraded — otherwise the two
    /// assertions above hold against a health surface that is simply always
    /// red, and they would prove nothing.
    #[test]
    fn a_healthy_gateway_composes_no_error_at_all() {
        assert_eq!(compose_registration_error(&None, &None, false), None);
        assert!(
            crate::channel::ChannelHealthReport {
                configured: 1,
                registered: 1,
                registration_error: None,
                channels: Vec::new(),
            }
            .is_complete()
        );
    }

    /// The lease component must track the LIVE role in the clearing direction
    /// too. The supervisor re-claims every tick and wins when the previous
    /// holder exits; a cached boot value would leave this permanently red.
    #[test]
    fn winning_the_lease_back_clears_the_lease_component_without_a_reload() {
        let while_observer = compose_registration_error(&None, &None, true);
        let after_winning = compose_registration_error(&None, &None, false);
        assert!(
            while_observer.is_some(),
            "known-positive: reported as observer"
        );
        assert_eq!(
            after_winning, None,
            "the supervisor won the lease and nothing else changed, so the \
             degradation must clear on its own"
        );
    }

    /// All three at once, in a fixed order, joined once each. Guards against a
    /// component being dropped or duplicated by a future edit to the composer.
    #[test]
    fn every_component_appears_exactly_once_and_in_a_stable_order() {
        let composed = compose_registration_error(
            &Some("registration boom".to_string()),
            &Some("inbound dispatch unavailable: x".to_string()),
            true,
        )
        .expect("three degradations must compose to something");
        assert_eq!(
            composed,
            "inbound dispatch unavailable: x; inbound polling owned by another \
             process; registration boom"
        );
        assert_eq!(composed.matches("inbound polling owned").count(), 1);
        assert_eq!(composed.matches("inbound dispatch unavailable").count(), 1);
    }

    #[test]
    fn the_default_profile_is_named_not_empty() {
        let s = ScopeArgs {
            profile: None,
            home: None,
        };
        // An empty profile would produce the service identifier
        // `wayland-core-gateway-`, which every family accepts and no operator
        // can tell apart from another empty one.
        assert!(!s.profile().is_empty());
    }

    #[test]
    fn an_explicit_profile_wins_over_the_environment() {
        let s = ScopeArgs {
            profile: Some("explicit".into()),
            home: None,
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
    fn the_drain_clock_reports_total_elapsed_not_the_increment() {
        // F24-B-H4. `DrainController::drain` exits its wait loop when the
        // value this returns reaches the budget. A closure returning the
        // per-iteration increment pins that value and the drain never
        // terminates while anything is pending — measured live as a gateway
        // stuck in `Draining` with 12 carried deliveries.
        //
        // Goes red the moment this returns a constant again.
        let dir = tempfile::tempdir().unwrap();
        let mut ledger = wcore_gateway::ledger::DeliveryLedger::open(dir.path()).unwrap();
        let started = std::time::Instant::now();

        let first = drain_wait(started, &mut ledger);
        let second = drain_wait(started, &mut ledger);
        let third = drain_wait(started, &mut ledger);

        assert!(
            second > first && third > second,
            "the drain clock must ACCUMULATE: got {first}, {second}, {third}"
        );
        // And it must actually be able to reach a budget. Three intervals of
        // 100ms cannot still be reporting one interval.
        assert!(
            third >= 250,
            "three 100ms observations must report >=250ms total, got {third}"
        );
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

    /// An abandonment that can be listed but not disposed of is only half a
    /// surface. Both verbs must be REACHABLE, not merely written: a handler
    /// that is never wired into `GatewayCmd` is dead code that reads as done.
    #[test]
    fn the_abandonment_can_be_disposed_of_as_well_as_listed() {
        let verbs = verb_names();
        for v in ["abandoned", "ack", "resend"] {
            assert!(
                verbs.contains(&v.to_string()),
                "`gateway {v}` must exist; got {verbs:?}"
            );
        }
    }

    /// The one decision that stands between `gateway resend` and a duplicate at
    /// the destination.
    ///
    /// `None` is the case that matters and the easy one to get wrong. It means
    /// the record pre-dates attempt tracking, so the journal cannot say whether
    /// the message landed — and an unknown must be read as "it might have".
    /// Reading it as "it did not" would silently authorise exactly the second
    /// copy the abandonment was written to prevent.
    #[test]
    fn resend_demands_confirmation_unless_the_delivery_provably_never_left() {
        assert!(
            !resend_needs_confirmation(Some(false)),
            "never attempted: it cannot be at the destination, so no confirmation"
        );
        assert!(
            resend_needs_confirmation(Some(true)),
            "in flight when abandoned: it may have landed"
        );
        assert!(
            resend_needs_confirmation(None),
            "UNKNOWN must be treated as 'may have landed', never as 'safe'"
        );
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
