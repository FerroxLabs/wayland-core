//! `wayland-core channel` — the operator verb surface over `wcore-channels`.
//!
//! Phase 24, plan 24-03, Success Criterion 3. Four verbs — `list`, `probe`,
//! `health`, `reload` — and the important thing about them is WHERE each one
//! gets its answer, because the three verbs answer questions with three
//! different truth sources and mixing them up is how a status surface starts
//! lying.
//!
//! | verb | source of truth | needs a running gateway |
//! |---|---|---|
//! | `list` | the on-disk config directory | no |
//! | `probe` | the PLATFORM, asked live | no |
//! | `health` | the RUNNING gateway's observations | **yes** |
//! | `reload` | a request the running gateway acts on | **yes** |
//!
//! # `health` refuses rather than reporting a comfortable nothing
//!
//! Health is an OBSERVATION: it is what the poll loops in a running gateway
//! have actually seen. A separate `channel health` process holds no such
//! observations. It would be trivial to make this verb construct its own
//! `ChannelManager`, start it, and print the result — and that output would be
//! a fabrication, describing adapters that have existed for eight
//! milliseconds rather than the ones carrying traffic.
//!
//! So `health` reads the file the running gateway republishes, and when no
//! gateway is running it FAILS with a message saying nothing has been
//! observed. It never prints an empty list, and it never prints "healthy".
//!
//! This is deliberate closure of a shape this program keeps measuring:
//! F24-C-M2 (`gateway status` reporting `deliveries_pending: 0` while nine
//! messages were already at the destination), F24-B-H3 before it, and the
//! Windows orphan scanner in Phase 25. **A zero from a surface that was not
//! looking is not a zero.**
//!
//! # `probe` deliberately does NOT need a gateway
//!
//! The opposite reasoning applies. A probe's answer comes from the platform,
//! not from local state, so an operator must be able to run it while the
//! gateway is DOWN — that is precisely when they are debugging a credential.
//! It constructs adapters, asks, and reports; it never starts a poll loop and
//! never sends a message.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};

// Reached through the registry rather than a direct dependency edge: adding
// `wcore-channels` to this crate's manifest rewrites `Cargo.lock`, a Phase-24
// shared seam concurrent lanes conflict on.
use wcore_channels::ChannelManager;
use wcore_channels::health::ChannelHealth;
use wcore_channels::probe::{ProbeOutcome, ProbeReport};
use wcore_channels_registry::wcore_channels;

/// The file the RUNNING gateway republishes its observed channel health into.
///
/// Separate from `gateway-status.json` on purpose: the gateway projection is
/// about the runtime, this is about the adapters, and a reader that wants one
/// should not have to parse and ignore the other.
pub const CHANNEL_HEALTH_FILE: &str = "channel-health.json";

/// The file `channel reload` creates and `gateway run` consumes.
///
/// A file rather than a signal, matching `gateway.drain`: both Unix families
/// already spend SIGTERM and SIGHUP on other meanings, Windows has neither,
/// and a reload that was indistinguishable from a stop would be worse than no
/// reload at all.
pub const CHANNEL_RELOAD_FILE: &str = "channel.reload";

#[derive(Debug, Clone, Args)]
pub struct ChannelArgs {
    #[command(subcommand)]
    pub cmd: ChannelCmd,
}

#[derive(Debug, Clone, Subcommand)]
pub enum ChannelCmd {
    /// List configured channels from the on-disk config directory.
    ///
    /// Read-only: never constructs an adapter, never touches the network, and
    /// never reads a credential. A channel whose config will not parse is
    /// listed WITH its parse error rather than omitted — an absent row and a
    /// broken row look identical otherwise.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Ask each configured channel's platform whether its setup is complete,
    /// whether its credential authenticates, and what identity it
    /// authenticated as. Sends no message.
    ///
    /// Exits non-zero when any probed channel is not ready, so this verb is
    /// usable as a gate rather than only as a report.
    Probe {
        /// Probe only this channel. Default: every configured channel.
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Show the per-adapter health a RUNNING gateway has observed.
    ///
    /// Fails when no gateway is running. See the module docs: a health
    /// surface that answers when it has observed nothing is the false-zero
    /// shape this phase has measured three times.
    Health {
        #[arg(long)]
        json: bool,
    },
    /// Ask the running gateway to re-read the channel config directory.
    ///
    /// Adapters whose configuration did not change keep their running
    /// instance; changed ones are replaced and deconfigured ones are stopped.
    Reload,
}

/// Resolve the home whose `channels/` directory and gateway files we act on.
/// Routed through `wcore_gateway::resolve_home` so this surface and the
/// gateway agree on which directory they are talking about.
fn home() -> Result<PathBuf> {
    wcore_gateway::resolve_home().context("cannot resolve WAYLAND_HOME or $HOME")
}

fn channels_dir(home: &Path) -> PathBuf {
    home.join("channels")
}

pub fn channel_health_path(home: &Path) -> PathBuf {
    home.join(CHANNEL_HEALTH_FILE)
}

pub fn channel_reload_path(home: &Path) -> PathBuf {
    home.join(CHANNEL_RELOAD_FILE)
}

/// Publish observed channel health for a second process to read.
///
/// Written to a same-directory temporary and renamed, so a `channel health`
/// racing a republish reads either the previous set or the next one and never
/// a half-written file. Same discipline as the gateway projection.
pub fn publish_health(home: &Path, health: &[ChannelHealth]) -> Result<()> {
    let final_path = channel_health_path(home);
    let tmp = final_path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(health)?)
        .with_context(|| format!("cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, &final_path)
        .with_context(|| format!("cannot publish {}", final_path.display()))?;
    Ok(())
}

/// Read the health a LIVE gateway published, or `None`.
///
/// Liveness is checked FIRST and the file second. A stale
/// `channel-health.json` left by a gateway that has since been killed
/// describes adapters that no longer exist, and reporting it would be exactly
/// the stale-projection failure `gateway status` already guards against.
pub fn read_live_health(home: &Path) -> Option<Vec<ChannelHealth>> {
    let record = wcore_gateway::pidlock::PidLock::read_record(home)?;
    if !wcore_gateway::pidlock::process_is_alive(record.pid) {
        return None;
    }
    let raw = std::fs::read_to_string(channel_health_path(home)).ok()?;
    serde_json::from_str(&raw).ok()
}

pub async fn run(args: ChannelArgs) -> Result<()> {
    match args.cmd {
        ChannelCmd::List { json } => list(json),
        ChannelCmd::Probe { name, json } => probe(name.as_deref(), json).await,
        ChannelCmd::Health { json } => health(json),
        ChannelCmd::Reload => reload(),
    }
}

// ---------------------------------------------------------------------------
// list
// ---------------------------------------------------------------------------

fn list(json: bool) -> Result<()> {
    let home = home()?;
    let dir = channels_dir(&home);
    let summaries = wcore_channels_registry::scan_channel_summaries(&dir);

    if json {
        let rows: Vec<serde_json::Value> = summaries
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "platform": s.platform,
                    "enabled": s.enabled,
                    "known_platform": s.known_platform,
                    "option_keys": s.option_keys,
                    "secret_keys": s.secret_keys,
                    "parse_error": s.parse_error,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }

    if summaries.is_empty() {
        println!("no channels configured in {}", dir.display());
        return Ok(());
    }
    println!("channels in {}:", dir.display());
    for s in &summaries {
        let state = if !s.known_platform {
            "UNKNOWN PLATFORM"
        } else if s.enabled {
            "enabled"
        } else {
            "disabled"
        };
        println!("  {:<20} {:<12} {}", s.name, s.platform, state);
        if let Some(err) = &s.parse_error {
            println!("      config error: {err}");
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// probe
// ---------------------------------------------------------------------------

async fn probe(only: Option<&str>, json: bool) -> Result<()> {
    let home = home()?;
    let dir = channels_dir(&home);

    let config = wcore_config::config::Config::resolve(&wcore_config::config::CliArgs::default())
        .context("cannot resolve configuration")?;
    let store = config
        .open_credentials_store()
        .context("cannot open the credentials store")?;
    let creds: Arc<dyn wcore_config::credentials::CredentialsStore> = Arc::from(store);

    // Adapters are CONSTRUCTED but never started: `probe` takes `&self` and
    // asks the platform directly. Starting them would open gateways and poll
    // loops that this short-lived process is about to drop, which is traffic
    // on a production surface for no reason.
    let mut mgr = ChannelManager::new();
    let registered = wcore_channels_registry::auto_register_from_dir(&mut mgr, &dir, creds)
        .await
        .with_context(|| format!("cannot load channels from {}", dir.display()))?;

    if registered == 0 {
        bail!(
            "no channels could be constructed from {} — nothing to probe",
            dir.display()
        );
    }

    let reports: Vec<ProbeReport> = match only {
        Some(name) => vec![
            mgr.probe_one(name)
                .await
                .with_context(|| format!("cannot probe channel {name:?}"))?,
        ],
        None => mgr.probe_all().await,
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&reports)?);
    } else {
        for r in &reports {
            println!("{} ({})", r.channel, r.platform);
            println!("  outcome:  {:?}", r.outcome);
            println!(
                "  config:   {}",
                if r.config_complete {
                    "complete"
                } else {
                    "INCOMPLETE"
                }
            );
            println!(
                "  auth:     {}",
                if r.authenticated {
                    "authenticated"
                } else {
                    "NOT authenticated"
                }
            );
            println!("  identity: {}", r.identity.as_deref().unwrap_or("-"));
            for f in &r.findings {
                println!("  finding:  {f}");
            }
        }
    }

    // Non-zero when anything is not ready, so this is a GATE and not just a
    // report. `Unsupported` counts as not ready — an adapter that declined to
    // check has not established that it is fine.
    let not_ready: Vec<&ProbeReport> = reports.iter().filter(|r| !r.outcome.is_ready()).collect();
    if !not_ready.is_empty() {
        let names: Vec<&str> = not_ready.iter().map(|r| r.channel.as_str()).collect();
        bail!(
            "{} of {} channels are not ready: {}",
            not_ready.len(),
            reports.len(),
            names.join(", ")
        );
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// health
// ---------------------------------------------------------------------------

fn health(json: bool) -> Result<()> {
    let home = home()?;
    let Some(health) = read_live_health(&home) else {
        // The refusal is the feature. See the module docs.
        bail!(
            "no running gateway for {} — channel health is an OBSERVATION and \
             nothing has observed anything. Start one with `wayland-core \
             gateway start`, or use `wayland-core channel probe`, which asks \
             the platform directly and needs no gateway.",
            home.display()
        );
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&health)?);
        return Ok(());
    }

    if health.is_empty() {
        println!("gateway is running and has registered no channels");
        return Ok(());
    }
    for h in &health {
        println!("{} ({})", h.channel, h.platform);
        println!("  state:      {:?}", h.state);
        println!("  reason:     {}", h.reason.as_deref().unwrap_or("-"));
        println!("  errors:     {}", h.consecutive_errors);
        println!("  reconnects: {}", h.reconnects);
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// reload
// ---------------------------------------------------------------------------

fn reload() -> Result<()> {
    let home = home()?;
    let record = wcore_gateway::pidlock::PidLock::read_record(&home);
    match record {
        Some(r) if wcore_gateway::pidlock::process_is_alive(r.pid) => {
            std::fs::write(channel_reload_path(&home), "1").with_context(|| {
                format!("cannot write {}", channel_reload_path(&home).display())
            })?;
            println!(
                "reload requested; gateway pid {} will re-read {}",
                r.pid,
                channels_dir(&home).display()
            );
            Ok(())
        }
        // Writing a request nobody will read, and reporting success for it, is
        // the same lie as an unobserved health report.
        _ => bail!(
            "no running gateway for {} — nothing would act on a reload request",
            home.display()
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_channels::health::HealthState;

    fn sample() -> Vec<ChannelHealth> {
        vec![ChannelHealth {
            channel: "acme".into(),
            platform: "discord".into(),
            state: HealthState::Degraded,
            reason: Some("supervised reconnect in progress".into()),
            consecutive_errors: 5,
            reconnects: 2,
        }]
    }

    #[test]
    fn published_health_round_trips_through_the_file() {
        let dir = tempfile::tempdir().unwrap();
        publish_health(dir.path(), &sample()).unwrap();
        let raw = std::fs::read_to_string(channel_health_path(dir.path())).unwrap();
        let back: Vec<ChannelHealth> = serde_json::from_str(&raw).unwrap();
        assert_eq!(back, sample());
    }

    #[test]
    fn publishing_leaves_no_temporary_behind() {
        // A leftover `.json.tmp` is how a reader eventually finds a
        // half-written file and parses it as truth.
        let dir = tempfile::tempdir().unwrap();
        publish_health(dir.path(), &sample()).unwrap();
        let tmp = channel_health_path(dir.path()).with_extension("json.tmp");
        assert!(
            !tmp.exists(),
            "temporary {} survived the rename",
            tmp.display()
        );
    }

    #[test]
    fn health_is_not_reported_when_no_gateway_is_running() {
        // THE case this verb exists to get right. A published file with no
        // live process must produce NOTHING, not a stale all-clear.
        let dir = tempfile::tempdir().unwrap();
        publish_health(dir.path(), &sample()).unwrap();
        assert!(
            read_live_health(dir.path()).is_none(),
            "health was reported from a file with no live gateway behind it"
        );
    }

    #[test]
    fn health_is_not_reported_from_a_stale_pid_record() {
        // Positive control for the case above: the file IS parseable and the
        // record IS present, so the only thing withholding the report is the
        // liveness check. Without this, the previous test would also pass if
        // `read_live_health` were simply broken.
        let dir = tempfile::tempdir().unwrap();
        publish_health(dir.path(), &sample()).unwrap();
        // A pid that cannot be alive: 0 is never a live user process on any
        // supported platform.
        wcore_gateway::pidlock::PidLock::write_stale_record_for_test(dir.path(), 0);
        assert!(
            wcore_gateway::pidlock::PidLock::read_record(dir.path()).is_some(),
            "positive control: the record really is readable"
        );
        assert!(
            std::fs::read_to_string(channel_health_path(dir.path()))
                .is_ok_and(|s| s.contains("acme")),
            "positive control: the published file really is present and parseable"
        );
        assert!(
            read_live_health(dir.path()).is_none(),
            "a dead pid must withhold the report"
        );
    }

    #[test]
    fn health_reads_back_when_the_recorded_process_is_this_one() {
        // The mirror case: with a LIVE pid the report comes through. Without
        // it, `read_live_health` returning None unconditionally would pass
        // every test above.
        let dir = tempfile::tempdir().unwrap();
        publish_health(dir.path(), &sample()).unwrap();
        wcore_gateway::pidlock::PidLock::write_stale_record_for_test(
            dir.path(),
            std::process::id(),
        );
        assert_eq!(
            read_live_health(dir.path()),
            Some(sample()),
            "a live pid plus a published file must produce the report"
        );
    }

    #[test]
    fn probe_readiness_gate_treats_unsupported_as_not_ready() {
        // The exit-status rule `probe` gates on. An adapter that implements no
        // probe must not satisfy a readiness gate.
        assert!(!ProbeReport::unsupported("c", "p").outcome.is_ready());
        assert!(ProbeOutcome::Ok.is_ready());
    }
}
