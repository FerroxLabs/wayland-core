//! Driver-seat materialization — turning a pure [`DriverSeatPlan`] into a
//! ready [`AgentSpawner`] for forge builders (A1.8/A1.9).
//!
//! Shared by the CLI `forge` verb and the session `Forge` tool so the seat
//! policy lives in exactly one place. Materialization is failure-tolerant by
//! contract: a seat that cannot be built falls back to the session seat with
//! a visible note — seat routing can only cheapen a forge, never break it.

use wcore_config::anvil::{AnvilConfig, DriverSeatPlan};
use wcore_config::config::{
    CliArgs, Config, ProviderType, connected_providers, provider_connected,
};

use crate::spawner::AgentSpawner;

/// A materialized driver seat: the spawner forge builders fork through, a
/// human-readable label, and any fallback notes accumulated on the way.
pub struct MaterializedSeat {
    /// Spawner whose base config IS the driver seat (auto-approve forced:
    /// forked builders have no approval channel, spec §5).
    pub spawner: AgentSpawner,
    /// `provider/model` label for receipts and logs.
    pub label: String,
    /// Human-visible notes (e.g. "driver seat unavailable; session drives").
    pub notes: Vec<String>,
}

/// Resolve + materialize the driver seat for forge builders.
///
/// `session_cfg` is the resolved session config; the returned spawner either
/// shares its provider (in-family) or carries a freshly built cross-family
/// provider (e.g. Flux's routed lane). Auto-approve is forced on the seat
/// config regardless of the session posture — the human decision happens at
/// the forge boundary (CLI verb / tool approval), machinery runs inside.
pub fn materialize_driver_seat(
    anvil: &AnvilConfig,
    session_cfg: &Config,
) -> anyhow::Result<MaterializedSeat> {
    let mut session_seat = session_cfg.clone();
    session_seat.tools.auto_approve = true;

    let mut notes = Vec::new();
    // `connected_providers()` iterates KNOWN_PROVIDER_TYPES, which deliberately
    // excludes FluxRouter (it is not a model-catalog provider) — probe Flux
    // connectivity explicitly or the routed lane is unreachable in practice.
    let mut connected = connected_providers();
    if provider_connected(ProviderType::FluxRouter) {
        connected.push(ProviderType::FluxRouter);
    }
    let plan = anvil.resolve_driver_seat(session_seat.provider, &connected);

    let driver_cfg = match &plan {
        DriverSeatPlan::Session => session_seat.clone(),
        DriverSeatPlan::SessionModel { model } => {
            let mut c = session_seat.clone();
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
                    notes.push(format!(
                        "driver seat `{provider}` unavailable ({e}); session model drives"
                    ));
                    session_seat.clone()
                }
            }
        }
    };

    let (provider, spawner_cfg) = match crate::bootstrap::create_provider_with_oauth(&driver_cfg) {
        Ok(p) => (p, driver_cfg),
        Err(e) if !matches!(plan, DriverSeatPlan::Session) => {
            // ANY routed seat (cross-family OR in-family model override) that
            // fails to build falls back to the untouched session seat — the
            // "never break a forge" contract. The fallback spawner must pair
            // the session provider with the session config (driver_cfg here
            // would point forks at the failed seat's model).
            notes.push(format!("driver seat failed ({e}); session model drives"));
            let p = crate::bootstrap::create_provider_with_oauth(&session_seat)?;
            (p, session_seat)
        }
        // plan == Session: driver_cfg IS the session seat — nothing to fall
        // back to; the error is real.
        Err(e) => return Err(e),
    };

    let label = format!("{}/{}", spawner_cfg.provider_label, spawner_cfg.model);
    Ok(MaterializedSeat {
        spawner: AgentSpawner::new(provider, spawner_cfg),
        label,
        notes,
    })
}

/// Materialize the VALVE seat (spec §6.4): the session provider + model — the
/// frontier judgment the user already chose — in the trusted posture. The
/// valve forks read-only, so auto-approve here only normalizes fork behavior.
pub fn materialize_valve_seat(session_cfg: &Config) -> anyhow::Result<MaterializedSeat> {
    let mut cfg = session_cfg.clone();
    cfg.tools.auto_approve = true;
    let provider = crate::bootstrap::create_provider_with_oauth(&cfg)?;
    let label = format!("{}/{}", cfg.provider_label, cfg.model);
    Ok(MaterializedSeat {
        spawner: AgentSpawner::new(provider, cfg),
        label,
        notes: Vec::new(),
    })
}
