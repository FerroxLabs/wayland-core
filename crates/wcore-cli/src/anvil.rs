//! CLI surface: `wayland-core forge "<task>"` — the explicit Anvil verb.
//!
//! Mirrors [`crate::crucible::run_crucible`]: it enforces the `[anvil] enabled`
//! kill-switch, resolves a provider + spawner + protocol emitter, then drives a
//! real gated-forge climb ([`drive_climb_full`]) which forks a builder into an
//! isolated worktree, runs the configured (or auto-detected) gate, climbs, and
//! emits an `AnvilReceipt` (on stdout, as a JSON-stream event).

use std::sync::Arc;

use clap::Args;
use wcore_agent::goal::{AnvilOutcome, StrategyTermination};
use wcore_agent::orchestration::anvil::forge::{FORGE_REQUIRED_STABILITY, drive_climb_full};
use wcore_agent::orchestration::anvil::seat::{
    materialize_standalone_driver_seat, materialize_valve_seat,
};
use wcore_config::config::{CliArgs, Config, load_merged_config_file};
use wcore_protocol::writer::ProtocolWriter;

/// Arguments for `wayland-core forge`.
#[derive(Args, Debug)]
pub struct ForgeArgs {
    /// The task to forge. Anvil is for work with a REAL, checkable gate
    /// (tests / build / lint) — it forges a candidate that passes it.
    pub task: String,
    /// Run this climb as the ONE loop owner of a durable Goal, and terminate it
    /// through the canonical Goal transition (F22C).
    ///
    /// Opt-in. Without `--goal` this is byte-for-byte the pre-F22C path. Anvil
    /// is the only one of the five strategies that can reach
    /// `GoalTerminalState::Verified`, and it reaches it only by handing the
    /// adapter the gate observation the climb engine measured — the CLI has no
    /// parameter through which it could supply one.
    #[command(flatten)]
    pub goal: crate::goal_cmd::GoalAttachArgs,
}

/// Admission gate for the forge verb: refuse before any provider/spawner
/// construction when Anvil is kill-switched off. The denial names the missing
/// prerequisite (the `[anvil] enabled` switch) and never claims a child ran or
/// exposes any secret.
fn ensure_anvil_enabled(cf: &wcore_config::config::ConfigFile) -> anyhow::Result<()> {
    if !cf.anvil.enabled {
        anyhow::bail!(
            "Anvil is disabled (kill-switched). Remove `enabled = false` from \
             `[anvil]` in your config to forge gated tasks."
        );
    }
    Ok(())
}

/// Entry point for `wayland-core forge`.
pub async fn run_forge(args: ForgeArgs) -> anyhow::Result<()> {
    let cf = load_merged_config_file(None)?;

    ensure_anvil_enabled(&cf)?;
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
    wcore_agent::egress::install_egress_policy(&session_cfg);
    let egress_policy: wcore_egress::SharedPolicy =
        Arc::new(wcore_agent::egress::policy_from_config(&session_cfg));
    let sandbox = Arc::new(wcore_sandbox::SandboxRegistry::required_for_session(
        session_cfg.tools.sandbox.as_deref(),
    )?);
    let mut seat =
        materialize_standalone_driver_seat(&cf.anvil, &session_cfg, Arc::clone(&egress_policy))
            .await?;
    seat.spawner = seat
        .spawner
        .with_sandbox_runtime(Arc::clone(&sandbox))
        .with_egress_policy(Arc::clone(&egress_policy));
    let session_id = seat.spawner.durable_session_id()?;
    for note in &seat.notes {
        eprintln!("forge: {note}");
    }
    eprintln!("forge: driver seat = {}", seat.label);
    // Valve seat (spec §6.4): the session/frontier model, read-only, one
    // diagnostic turn on a stall. Best-effort — a forge without a valve is
    // still a forge (it just stays cheap-dumb on a stall).
    let valve_seat = match materialize_valve_seat(&session_cfg, egress_policy, &seat.spawner).await
    {
        Ok(s) => {
            eprintln!("forge: valve seat = {}", s.label);
            Some(s)
        }
        Err(e) => {
            eprintln!("forge: no valve seat ({e}); climbing without escalation");
            None
        }
    };
    let spawner = seat.spawner;

    // The top-level protocol writer — the AnvilReceipt is trusted ONLY from this
    // top-level emission (host trust boundary, spec §8).
    let emitter = Arc::new(ProtocolWriter::new());

    let workspace = std::env::current_dir()?;

    let valve_spawner = valve_seat
        .as_ref()
        .map(|s| &s.spawner as &dyn wcore_types::spawner::Spawner);

    // ── F22C: the canonical terminal transition, when asked for ─────────────
    //
    // The climb below is THE production one — the same `drive_climb_full` over
    // the same seat, workspace, gate and sandbox. Attaching a Goal wraps that
    // one call in `GoalLoop::run_anvil`; it does not build a second forge.
    if let Some((driver, goal_id)) = args.goal.resolve()? {
        let cursor = driver
            .run_anvil(&goal_id, |owner| async move {
                match drive_climb_full(
                    &args.task,
                    &cf.anvil,
                    &workspace,
                    &spawner,
                    valve_spawner,
                    &emitter,
                    &session_id,
                    &uuid::Uuid::new_v4().to_string(),
                    &uuid::Uuid::new_v4().to_string(),
                    sandbox,
                )
                .await
                {
                    Ok(outcome) => {
                        print_climb_summary(&outcome);
                        StrategyTermination::from_anvil(
                            owner,
                            AnvilOutcome::Climbed(&outcome),
                            FORGE_REQUIRED_STABILITY,
                        )
                    }
                    // A forge failure is NOT an `EngineError`; it gets the
                    // driver-failure carrier and lands in `Blocked` with the
                    // real reason rather than a fabricated engine category.
                    Err(error) => {
                        eprintln!("forge: {error}");
                        StrategyTermination::from_anvil(
                            owner,
                            AnvilOutcome::ForgeFailed {
                                detail: error.to_string(),
                            },
                            FORGE_REQUIRED_STABILITY,
                        )
                    }
                }
            })
            .await
            .map_err(|e| anyhow::anyhow!("goal {} did not terminate: {e}", goal_id.as_str()))?;
        crate::goal_cmd::print_canonical_transition(&driver, &goal_id, "anvil", &cursor);
        return Ok(());
    }

    match drive_climb_full(
        &args.task,
        &cf.anvil,
        &workspace,
        &spawner,
        valve_spawner,
        &emitter,
        &session_id,
        &uuid::Uuid::new_v4().to_string(),
        &uuid::Uuid::new_v4().to_string(),
        sandbox,
    )
    .await
    {
        Ok(outcome) => {
            print_climb_summary(&outcome);
            Ok(())
        }
        Err(e) => anyhow::bail!("forge: {e}"),
    }
}

/// The human summary on stderr. The receipt itself already went to stdout.
///
/// Extracted so the Goal-attached path and the unattached path print the SAME
/// line — a second copy would let them drift, and "the attached run behaves
/// identically" is what makes the attachment a wrapper rather than a fork.
fn print_climb_summary(outcome: &wcore_agent::orchestration::anvil::engine::ClimbOutcome) {
    eprintln!(
        "forge: terminal={:?} stamp={} checks={}/{} iterations={} valve_fires={}",
        outcome.terminal,
        outcome.stamp,
        outcome.checks_passed,
        outcome.checks_total,
        outcome.iterations,
        outcome.valve_fires,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_config::config::ConfigFile;

    #[test]
    fn forge_admitted_when_anvil_enabled() {
        // Availability, not activity: a default config admits the forge verb so
        // the workspace-aware production seat can be constructed downstream.
        let cf = ConfigFile::default();
        assert!(cf.anvil.enabled, "anvil defaults to available");
        assert!(ensure_anvil_enabled(&cf).is_ok());
    }

    #[test]
    fn forge_denied_before_dispatch_when_kill_switched() {
        // The kill-switch denies BEFORE any provider/spawner/seat construction.
        let mut cf = ConfigFile::default();
        cf.anvil.enabled = false;
        let err = ensure_anvil_enabled(&cf).expect_err("kill-switch must deny");
        let message = err.to_string();
        // Names the missing prerequisite...
        assert!(message.contains("disabled"));
        assert!(message.contains("[anvil]"));
        // ...without exposing secrets or claiming a child ran.
        let lowered = message.to_lowercase();
        assert!(!lowered.contains("api_key"));
        assert!(!lowered.contains("token"));
        assert!(!lowered.contains("forged"));
    }
}
