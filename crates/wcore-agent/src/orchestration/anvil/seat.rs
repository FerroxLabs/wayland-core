//! Driver-seat materialization — turning a pure [`DriverSeatPlan`] into a
//! ready [`AgentSpawner`] for forge builders (A1.8/A1.9).
//!
//! Shared by the CLI `forge` verb and the session `Forge` tool so the seat
//! policy lives in exactly one place. Materialization is failure-tolerant by
//! contract: a seat that cannot be built falls back to the session seat with
//! a visible note — seat routing can only cheapen a forge, never break it.
//!
//! The ONE exception is the nested-ladder refusal (#893): a seat whose model
//! runs Flux's own server-side climb is refused outright rather than fallen
//! back from, because there is no cheaper-or-equal seat to fall back TO — the
//! collision is in the model the operator named. See
//! [`refuse_nested_server_ladder`].

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
///
/// Workspace-authority propagation: the seat is cloned from the session spawner
/// via [`AgentSpawner::clone_for_resolved_config`], which carries the session's
/// bound parent-workspace authority (and sandbox runtime) into the seat. The
/// forge's builder is an isolated-mutation child (Write/Edit tools), so this
/// bound authority is exactly what lets the production spawner allocate each
/// candidate's transaction-owned standalone checkout — the seat never launches a
/// mutating builder against the parent checkout.
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
///
/// Workspace-authority propagation: governance runs through
/// [`crate::bootstrap::govern_standalone_spawner`], which binds the parent
/// repository identity (`with_parent_workspace`) on the seat spawner. That bound
/// authority is required for the forge to allocate each candidate's
/// transaction-owned standalone checkout through the production run-and-retain
/// seam; the caller supplies the enforcing sandbox runtime before running the
/// forge, so a mutating builder can never run in a shared/parent checkout.
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

/// The Flux alias that runs the ROUTER's own server-side gated climb
/// (Elevation), as opposed to `flux-auto`, which only routes.
const FLUX_SERVER_LADDER_ALIAS: &str = "flux-verified";

/// Fail-closed refusal of any Anvil seat that would nest Flux's server-side
/// Elevation ladder inside Anvil's own client-side climb (#893).
///
/// **This blocks; it does not warn.** Anvil IS a client-side ladder (worktree
/// builders + sandboxed gate + receipt). Driving any of its seats through
/// `flux-verified` runs a second, server-side ladder on the same task: both
/// pay for iteration, they converge on each other's output, and each receipt
/// is wrong about the other. There is no posture in which that is the cheaper
/// or the more correct answer, so there is nothing for the operator to weigh —
/// a note they can ignore is not a guard.
///
/// Belt-and-braces by design. The load-bearing enforcement is structural
/// (`wcore_types::llm::FluxLoopIntent` makes "Core owns the loop" and "run the
/// server ladder" unrepresentable on one turn) plus the engine's hard fault on
/// an `X-Flux-Loop-Engaged: elevation` echo. This refuses the same collision at
/// the seat layer, BEFORE a provider is built and a builder is forked, so the
/// operator gets a legible message instead of a mid-climb API error.
fn refuse_nested_server_ladder(seat_kind: &str, cfg: &Config) -> anyhow::Result<()> {
    if cfg.model.contains(FLUX_SERVER_LADDER_ALIAS) {
        anyhow::bail!(
            "{seat_kind} model `{}` runs the router's own server-side climb; \
             nesting it inside Anvil's client-side ladder doubles the work and \
             makes both receipts wrong. Use `flux-auto` for the {seat_kind}.",
            cfg.model
        );
    }
    Ok(())
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

    // Fail-closed BEFORE a provider is built: a refused seat must not have
    // spent an API handshake, and the message must name the driver seat rather
    // than whatever the fallback would have been.
    refuse_nested_server_ladder("driver seat", &driver_cfg)?;

    let (provider, spawner_cfg) =
        match create_provider_with_policy(&driver_cfg, egress_policy.clone()).await {
            Ok(p) => (p, driver_cfg),
            Err(e) if !matches!(plan, DriverSeatPlan::Session) => {
                // ANY routed seat (cross-family OR in-family model override) that
                // fails to build falls back to the untouched session seat — the
                // "never break a forge" contract. The fallback spawner must pair
                // the session provider with the session config (driver_cfg here
                // would point forks at the failed seat's model).
                //
                // The fallback TARGET gets its own refusal. The guard above
                // cleared `driver_cfg`, not `session_seat`, so without this a
                // legal routed seat that fails to build silently lands the
                // forge on a `flux-verified` session model — the exact nested
                // ladder, reached by a path nobody chose.
                refuse_nested_server_ladder("driver seat fallback", &session_seat)?;
                notes.push(format!("driver seat failed ({e}); session model drives"));
                let p = create_provider_with_policy(&session_seat, egress_policy.clone()).await?;
                (p, session_seat)
            }
            // plan == Session: driver_cfg IS the session seat — nothing to fall
            // back to; the error is real.
            Err(e) => return Err(e),
        };

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
    // The valve is a seat of the same climb: its diagnostic turn is mid-loop
    // material, so a server-side ladder underneath it is the same collision.
    // Callers treat a valve failure as best-effort, so refusing here costs the
    // escalation turn and never the forge.
    refuse_nested_server_ladder("valve seat", &cfg)?;
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

    /// Config rooted at a throwaway `WAYLAND_HOME`, with the session model
    /// under the caller's control. Shared by the three #893 refusal tests so
    /// they differ only in WHICH seat carries the nested-ladder model.
    fn seat_test_config(home: &std::path::Path, sessions: &std::path::Path, model: &str) -> Config {
        std::fs::write(
            home.join("config.toml"),
            "[default]\nprovider = \"anthropic\"\nmodel = \"claude-test\"\n\
             [providers.anthropic]\napi_key = \"unused\"\n",
        )
        .unwrap();
        Config {
            provider_label: "anthropic".into(),
            provider: ProviderType::Anthropic,
            model: model.into(),
            session: wcore_config::config::SessionConfig {
                directory: sessions.to_string_lossy().into_owned(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    /// Make `create_provider_with_oauth` fail for this config (and every clone
    /// of it) AFTER the primary provider is built: the fallback label list and
    /// the resolved-config list disagree, which `build_fallback_providers`
    /// refuses. Used to force the driver seat onto its fallback path.
    fn break_provider_construction(cfg: &mut Config) {
        cfg.provider_chain.enabled = true;
        cfg.provider_chain
            .fallback_models
            .push("anthropic:haiku".into());
        cfg.resolved_fallbacks.clear();
    }

    /// #893 — seat kind 1 of 3: the DRIVER seat itself.
    ///
    /// A note is not a guard: this asserts the refusal, not the wording, so
    /// reverting `refuse_nested_server_ladder` back to `notes.push(...)` turns
    /// it red instead of leaving it green.
    #[tokio::test]
    #[serial]
    async fn driver_seat_refuses_a_nested_server_ladder() {
        let home = tempfile::tempdir().unwrap();
        let sessions = tempfile::tempdir().unwrap();
        let session_cfg = seat_test_config(home.path(), sessions.path(), "claude-test");
        let _home = WaylandHomeGuard::install(home.path());

        // Explicit `driver_model` pins the session provider and makes the plan
        // `SessionModel`, so the nested-ladder alias lands on the DRIVER seat
        // while the session seat stays clean.
        let anvil = AnvilConfig {
            driver_model: Some("flux-verified".into()),
            ..AnvilConfig::default()
        };
        // `MaterializedSeat` is not `Debug`, so `expect_err` is unavailable;
        // panic with the seat that got through instead.
        let err = match materialize_standalone_driver_seat(
            &anvil,
            &session_cfg,
            wcore_egress::default_policy(),
        )
        .await
        {
            Ok(seat) => panic!(
                "a flux-verified driver seat must be refused, not noted; got `{}`",
                seat.label
            ),
            Err(err) => err,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("driver seat") && msg.contains("flux-verified"),
            "refusal must name the seat and the model: {msg}"
        );
    }

    /// #893 — seat kind 2 of 3: the driver-seat FALLBACK.
    ///
    /// The routed seat is legal and simply fails to build; the untouched
    /// session seat it falls back to is the one carrying the nested ladder.
    /// Without a second refusal on the fallback target the forge lands on it
    /// by a path nobody chose.
    #[tokio::test]
    #[serial]
    async fn driver_seat_fallback_refuses_a_nested_server_ladder() {
        let home = tempfile::tempdir().unwrap();
        let sessions = tempfile::tempdir().unwrap();
        let mut session_cfg = seat_test_config(home.path(), sessions.path(), "flux-verified");
        break_provider_construction(&mut session_cfg);
        let _home = WaylandHomeGuard::install(home.path());
        assert!(
            crate::bootstrap::create_provider_with_oauth(&session_cfg).is_err(),
            "the fallback path is only reached when the routed seat fails to build"
        );

        // A clean driver model: it clears the driver-seat guard, then fails to
        // build (it inherits the broken chain), which is what routes the seat
        // onto its fallback.
        let anvil = AnvilConfig {
            driver_model: Some("claude-driver-test".into()),
            ..AnvilConfig::default()
        };
        let err = match materialize_standalone_driver_seat(
            &anvil,
            &session_cfg,
            wcore_egress::default_policy(),
        )
        .await
        {
            Ok(seat) => panic!(
                "the fallback target must be refused too; got `{}`",
                seat.label
            ),
            Err(err) => err,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("driver seat fallback") && msg.contains("flux-verified"),
            "the refusal must name the FALLBACK, not the routed seat: {msg}"
        );
    }

    /// #893 — seat kind 3 of 3: the VALVE seat.
    ///
    /// The valve's diagnostic turn is mid-loop material of the same climb, so
    /// a server-side ladder underneath it is the same collision. Refusing it
    /// costs the escalation turn, never the forge (callers take the valve
    /// best-effort).
    #[tokio::test]
    #[serial]
    async fn valve_seat_refuses_a_nested_server_ladder() {
        let home = tempfile::tempdir().unwrap();
        let sessions = tempfile::tempdir().unwrap();
        let session_cfg = seat_test_config(home.path(), sessions.path(), "flux-verified");
        let _home = WaylandHomeGuard::install(home.path());

        // The session spawner is only a carrier here; build it from a clean
        // model so the refusal under test can only come from the valve config.
        let carrier = seat_test_config(home.path(), sessions.path(), "claude-test");
        let provider = crate::bootstrap::create_provider_with_oauth(&carrier)
            .expect("the carrier spawner must build");
        let session_spawner = AgentSpawner::new(provider, carrier);

        let err = match materialize_valve_seat(
            &session_cfg,
            wcore_egress::default_policy(),
            &session_spawner,
        )
        .await
        {
            Ok(seat) => panic!(
                "a flux-verified valve seat must be refused; got `{}`",
                seat.label
            ),
            Err(err) => err,
        };
        let msg = err.to_string();
        assert!(
            msg.contains("valve seat") && msg.contains("flux-verified"),
            "refusal must name the valve seat: {msg}"
        );
    }
}
