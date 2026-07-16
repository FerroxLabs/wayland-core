//! Driver-seat materialization — turning a pure [`DriverSeatPlan`] into a
//! ready [`AgentSpawner`] for forge builders (A1.8/A1.9).
//!
//! Shared by the CLI `forge` verb and the session `Forge` tool so the seat
//! policy lives in exactly one place. Materialization is failure-tolerant by
//! contract: a seat that cannot be built falls back to the session seat with
//! a visible note — seat routing can only cheapen a forge, never break it.

use std::sync::Arc;

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

struct ResolvedDriverSeat {
    provider: std::sync::Arc<dyn wcore_providers::LlmProvider>,
    config: Config,
    label: String,
    notes: Vec<String>,
}

/// Resolve + materialize the driver seat for forge builders.
///
/// `session_cfg` is the resolved session config; the returned spawner either
/// shares its provider (in-family) or carries a freshly built cross-family
/// provider (e.g. Flux's routed lane). Auto-approve is forced on the seat
/// config regardless of the session posture — the human decision happens at
/// the forge boundary (CLI verb / tool approval), machinery runs inside.
pub async fn materialize_driver_seat(
    anvil: &AnvilConfig,
    session_cfg: &Config,
    egress_policy: wcore_egress::SharedPolicy,
    session_spawner: &AgentSpawner,
) -> anyhow::Result<MaterializedSeat> {
    let resolved = resolve_driver_seat(anvil, session_cfg, Arc::clone(&egress_policy)).await?;
    let spawner = session_spawner
        .clone_for_resolved_config(resolved.provider, resolved.config)
        .with_egress_policy(egress_policy);
    Ok(MaterializedSeat {
        spawner,
        label: resolved.label,
        notes: resolved.notes,
    })
}

/// Materialize a governed driver for a standalone session.
///
/// The explicit CLI path must select its usable driver first: eagerly building
/// the default session provider would make a valid routed driver fail merely
/// because the best-effort valve provider is unavailable. Governance attaches
/// inside this function so no executable unbound spawner crosses the public
/// boundary.
pub async fn materialize_standalone_driver_seat(
    anvil: &AnvilConfig,
    session_cfg: &Config,
    egress_policy: wcore_egress::SharedPolicy,
) -> anyhow::Result<MaterializedSeat> {
    let resolved = resolve_driver_seat(anvil, session_cfg, Arc::clone(&egress_policy)).await?;
    let spawner = crate::bootstrap::govern_standalone_spawner(
        AgentSpawner::new(resolved.provider, resolved.config),
        session_cfg,
    )?
    .with_egress_policy(egress_policy);
    Ok(MaterializedSeat {
        spawner,
        label: resolved.label,
        notes: resolved.notes,
    })
}

async fn resolve_driver_seat(
    anvil: &AnvilConfig,
    session_cfg: &Config,
    egress_policy: wcore_egress::SharedPolicy,
) -> anyhow::Result<ResolvedDriverSeat> {
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

    let (provider, spawner_cfg) =
        match create_provider_with_policy(&driver_cfg, egress_policy.clone()).await {
            Ok(p) => (p, driver_cfg),
            Err(e) if !matches!(plan, DriverSeatPlan::Session) => {
                // ANY routed seat (cross-family OR in-family model override) that
                // fails to build falls back to the untouched session seat — the
                // "never break a forge" contract. The fallback spawner must pair
                // the session provider with the session config (driver_cfg here
                // would point forks at the failed seat's model).
                notes.push(format!("driver seat failed ({e}); session model drives"));
                let p = create_provider_with_policy(&session_seat, egress_policy.clone()).await?;
                (p, session_seat)
            }
            // plan == Session: driver_cfg IS the session seat — nothing to fall
            // back to; the error is real.
            Err(e) => return Err(e),
        };

    // Double-ladder guard: Flux's `flux-verified` alias runs the router's OWN
    // server-side gated climb (Elevation). Driving Anvil's builders through it
    // would nest two ladders — both paying for iteration, one receipt lying
    // about the other. The auto path only ever picks `flux-auto` (routing,
    // no loop); an explicit user config is honored but loudly flagged.
    if spawner_cfg.model.contains("flux-verified") {
        notes.push(
            "driver model `flux-verified` runs the router's own server-side climb — \
             nested ladders double the work; use `flux-auto` for the driver seat"
                .to_string(),
        );
    }

    let label = format!("{}/{}", spawner_cfg.provider_label, spawner_cfg.model);
    Ok(ResolvedDriverSeat {
        provider,
        config: spawner_cfg,
        label,
        notes,
    })
}

/// Materialize the VALVE seat (spec §6.4): the session provider + model — the
/// frontier judgment the user already chose — in the trusted posture. The
/// valve forks read-only, so auto-approve here only normalizes fork behavior.
pub async fn materialize_valve_seat(
    session_cfg: &Config,
    egress_policy: wcore_egress::SharedPolicy,
    session_spawner: &AgentSpawner,
) -> anyhow::Result<MaterializedSeat> {
    let mut cfg = session_cfg.clone();
    cfg.tools.auto_approve = true;
    let provider = create_provider_with_policy(&cfg, egress_policy.clone()).await?;
    let label = format!("{}/{}", cfg.provider_label, cfg.model);
    let spawner = session_spawner
        .clone_for_resolved_config(provider, cfg)
        .with_egress_policy(egress_policy);
    Ok(MaterializedSeat {
        spawner,
        label,
        notes: Vec::new(),
    })
}

async fn create_provider_with_policy(
    config: &Config,
    policy: wcore_egress::SharedPolicy,
) -> anyhow::Result<std::sync::Arc<dyn wcore_providers::LlmProvider>> {
    wcore_egress::with_default_policy(policy, async {
        crate::bootstrap::create_provider_with_oauth(config)
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    struct WaylandHomeGuard(Option<std::ffi::OsString>);

    impl WaylandHomeGuard {
        fn install(path: &std::path::Path) -> Self {
            let prior = std::env::var_os("WAYLAND_HOME");
            // SAFETY: this test is serialized and the guard restores the prior
            // process value on every normal/panic unwind path.
            unsafe { std::env::set_var("WAYLAND_HOME", path) };
            Self(prior)
        }
    }

    impl Drop for WaylandHomeGuard {
        fn drop(&mut self) {
            // SAFETY: paired with the serialized install above.
            match self.0.take() {
                Some(value) => unsafe { std::env::set_var("WAYLAND_HOME", value) },
                None => unsafe { std::env::remove_var("WAYLAND_HOME") },
            }
        }
    }

    #[tokio::test]
    #[serial]
    async fn standalone_routed_driver_does_not_require_the_default_provider() {
        let home = tempfile::tempdir().unwrap();
        let sessions = tempfile::tempdir().unwrap();
        std::fs::write(
            home.path().join("config.toml"),
            "[default]\nprovider = \"anthropic\"\nmodel = \"claude-test\"\n\
             [providers.anthropic]\napi_key = \"unused\"\n\
             [providers.flux-router]\napi_key = \"flux-test-key\"\n",
        )
        .unwrap();
        let _home = WaylandHomeGuard::install(home.path());

        let mut session_cfg = Config {
            provider_label: "anthropic".into(),
            provider: ProviderType::Anthropic,
            model: "claude-test".into(),
            session: wcore_config::config::SessionConfig {
                directory: sessions.path().to_string_lossy().into_owned(),
                ..Default::default()
            },
            ..Default::default()
        };
        // Deterministically make construction of the otherwise-unused default
        // provider fail after its primary is built.
        session_cfg.provider_chain.enabled = true;
        session_cfg
            .provider_chain
            .fallback_models
            .push("anthropic:haiku".into());
        session_cfg.resolved_fallbacks.clear();
        assert!(crate::bootstrap::create_provider_with_oauth(&session_cfg).is_err());

        let anvil = AnvilConfig {
            driver_provider: Some("flux-router".into()),
            driver_model: Some("flux-auto".into()),
            ..AnvilConfig::default()
        };
        let policy = wcore_egress::default_policy();
        let seat = materialize_standalone_driver_seat(&anvil, &session_cfg, policy)
            .await
            .expect("routed driver must materialize without the default provider");
        assert_eq!(seat.label, "flux-router/flux-auto");

        assert!(!seat.spawner.durable_session_id().unwrap().is_empty());
    }
}
