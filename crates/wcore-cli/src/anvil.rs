//! CLI surface: `wayland-core forge "<task>"` — the explicit Anvil verb.
//!
//! Mirrors [`crate::crucible::run_crucible`]: it enforces the `[anvil] enabled`
//! kill-switch before doing anything, then routes to the `drive_climb` engine
//! seam. A1 skeleton — the climb currently returns an honest
//! not-yet-implemented terminal; the real loop lands in later A1 slices.

use clap::Args;
use wcore_agent::orchestration::anvil::drive_climb;
use wcore_config::config::load_merged_config_file;

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
            "Anvil is disabled. Set `enabled = true` under `[anvil]` in your config \
             to forge gated tasks. (A1 is kill-switched until the slice completes.)"
        );
    }

    match drive_climb(&args.task, &cf.anvil).await {
        Ok(result) => {
            println!("forge: {:?}", result.terminal);
            Ok(())
        }
        Err(e) => anyhow::bail!("forge: {e}"),
    }
}
