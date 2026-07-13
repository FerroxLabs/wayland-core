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
use wcore_agent::spawner::AgentSpawner;
use wcore_config::anvil::DriverSeatPlan;
use wcore_config::config::{CliArgs, Config, connected_providers, load_merged_config_file};
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

    // Resolve the session provider + a spawner to fork builder sub-agents with.
    // Anvil A1 runs in a TRUSTED / auto-approve posture (spec §5): a forked
    // builder has no approval channel, so its Write/Edit/Bash tools must be
    // pre-approved or they fail closed. The explicit `forge` verb opts the
    // session into auto-approve so the builder can actually make the change.
    let mut session_cfg = Config::resolve(&CliArgs::default())?;
    session_cfg.tools.auto_approve = true;

    // Seat routing (A1.8): builders run on the DRIVER seat — explicit config,
    // else Flux's routed lane when a Flux key is connected, else an in-family
    // mid tier, else the session seat. Materialization failures fall back to
    // the session seat with a visible note: seat routing must never break a
    // forge, only cheapen it.
    let plan = cf
        .anvil
        .resolve_driver_seat(session_cfg.provider, &connected_providers());
    let driver_cfg = match &plan {
        DriverSeatPlan::Session => session_cfg.clone(),
        DriverSeatPlan::SessionModel { model } => {
            let mut c = session_cfg.clone();
            c.model = model.clone();
            c
        }
        DriverSeatPlan::Provider { provider, model } => {
            let args = CliArgs {
                provider: Some(provider.clone()),
                model: model.clone(),
                auto_approve: true,
                ..CliArgs::default()
            };
            match Config::resolve(&args) {
                Ok(mut c) => {
                    c.tools.auto_approve = true;
                    c
                }
                Err(e) => {
                    eprintln!(
                        "forge: driver seat `{provider}` unavailable ({e}); session model drives"
                    );
                    session_cfg.clone()
                }
            }
        }
    };
    eprintln!(
        "forge: driver seat = {}/{}",
        driver_cfg.provider_label, driver_cfg.model
    );
    let (provider, spawner_cfg) =
        match wcore_agent::bootstrap::create_provider_with_oauth(&driver_cfg) {
            Ok(p) => (p, driver_cfg),
            Err(e) if driver_cfg.provider != session_cfg.provider => {
                // Cross-family driver failed to build — fall back, don't fail.
                // The fallback spawner must pair the session provider with the
                // session config (a driver_cfg here would point forks at the
                // failed provider's model).
                eprintln!("forge: driver provider failed ({e}); session model drives");
                let p = wcore_agent::bootstrap::create_provider_with_oauth(&session_cfg)?;
                (p, session_cfg.clone())
            }
            Err(e) => return Err(e),
        };
    let spawner = AgentSpawner::new(provider, spawner_cfg);

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
