//! CLI surface: `wayland-core forge "<task>"` — the explicit Anvil verb.
//!
//! Mirrors [`crate::crucible::run_crucible`]: it enforces the `[anvil] enabled`
//! kill-switch, resolves a provider + spawner + protocol emitter, then drives a
//! real gated-forge climb ([`drive_climb_full`]) which forks a builder into an
//! isolated worktree, runs the configured (or auto-detected) gate, climbs, and
//! emits an `AnvilReceipt` (on stdout, as a JSON-stream event).

use std::sync::Arc;

use clap::Args;
use wcore_agent::orchestration::anvil::forge::drive_climb_full;
use wcore_agent::orchestration::anvil::seat::materialize_driver_seat;
use wcore_config::config::{CliArgs, Config, load_merged_config_file};
use wcore_protocol::writer::{ProtocolEmitter, ProtocolWriter};

/// Arguments for `wayland-core forge`.
#[derive(Args, Debug)]
pub struct ForgeArgs {
    /// The task to forge. Anvil is for work with a REAL, checkable gate
    /// (tests / build / lint) — it forges a candidate that passes it.
    pub task: String,
}

/// Entry point for `wayland-core forge`.
pub async fn run_forge(args: ForgeArgs) -> anyhow::Result<()> {
    let cf = load_merged_config_file(None)?;

    if !cf.anvil.enabled {
        anyhow::bail!(
            "Anvil is disabled (kill-switched). Remove `enabled = false` from \
             `[anvil]` in your config to forge gated tasks."
        );
    }
    // No gate pre-check here: an empty `[anvil] gate` means auto-detect (A1.7),
    // and `drive_climb_full` refuses with a precise error if nothing is found.

    // Resolve the session config, then materialize the DRIVER seat (A1.8):
    // explicit config → Flux routed lane when a Flux key is connected →
    // in-family mid tier → session seat. The shared helper forces the
    // trusted / auto-approve posture (spec §5: forked builders have no
    // approval channel) and falls back to the session seat on any
    // materialization failure — seat routing can only cheapen a forge,
    // never break it.
    let session_cfg = Config::resolve(&CliArgs::default())?;
    let seat = materialize_driver_seat(&cf.anvil, &session_cfg)?;
    for note in &seat.notes {
        eprintln!("forge: {note}");
    }
    eprintln!("forge: driver seat = {}", seat.label);
    let spawner = seat.spawner;

    // The top-level protocol writer — the AnvilReceipt is trusted ONLY from this
    // top-level emission (host trust boundary, spec §8).
    let emitter: Arc<dyn ProtocolEmitter> = Arc::new(ProtocolWriter::new());

    let workspace = std::env::current_dir()?;

    match drive_climb_full(&args.task, &cf.anvil, &workspace, &spawner, &emitter, None).await {
        Ok(outcome) => {
            // The receipt already went to stdout; this is a human summary on stderr.
            eprintln!(
                "forge: terminal={:?} stamp={} checks={}/{} iterations={}",
                outcome.terminal,
                outcome.stamp,
                outcome.checks_passed,
                outcome.checks_total,
                outcome.iterations,
            );
            Ok(())
        }
        Err(e) => anyhow::bail!("forge: {e}"),
    }
}
