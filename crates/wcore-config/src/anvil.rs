//! Anvil (native gated-forge engine) configuration — the on-disk `[anvil]`
//! block. ON by default: the forge is invocation-only and structurally refuses
//! without a real executable gate, so availability is safe. `enabled = false`
//! is the kill-switch for operators who want the rail inert regardless.
//!
//! Anvil is the checkable-reward sibling of Crucible: when a task has a REAL
//! executable gate, Anvil forges a candidate that passes it and stamps a
//! `verified` receipt. See
//! `docs/design/2026-07-12-anvil-native-gated-forge-design.md`.

use serde::{Deserialize, Serialize};

use crate::config::{ProviderType, driver_model_for};

/// Top-level `[anvil]` configuration.
///
/// `enabled` defaults to `true`: availability, not activity. The forge only
/// ever runs when explicitly invoked AND a gate exists (configured or
/// auto-detected); with neither it refuses (fails safe).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct AnvilConfig {
    /// Kill-switch. `true` (the default) makes the forge available; it still
    /// only runs when invoked and only against a real gate. `false` keeps
    /// Anvil inert: the `forge` subcommand (and the `/forge` verb) refuse.
    pub enabled: bool,
    /// The Tier-1 gate command as an argv (e.g. `["cargo", "test"]`). The forge
    /// runs this against each candidate; a `0` exit means the candidate passes.
    /// EMPTY (the default) means auto-detect: the forge probes the workspace
    /// for its native suite (Cargo, npm, go, pytest, just, make). If nothing is
    /// configured AND nothing is detected, the forge refuses — a gated-forge
    /// with no gate can verify nothing (fails safe).
    pub gate: Vec<String>,
    /// Explicit driver-seat provider for forge builders (e.g. "flux-router").
    /// Unset (the default) means auto: Flux's routed lane when a Flux key is
    /// connected, else an in-family mid-tier split of the session provider.
    pub driver_provider: Option<String>,
    /// Explicit driver-seat model. With `driver_provider` unset this pins a
    /// model on the SESSION provider; with it set, on that provider.
    pub driver_model: Option<String>,
}

impl Default for AnvilConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            gate: Vec::new(),
            driver_provider: None,
            driver_model: None,
        }
    }
}

/// Where forge BUILDER turns run (the Smart Loops driver seat). The climb's
/// verification never moves: the gate is machinery regardless of the driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DriverSeatPlan {
    /// The session provider + model drive unchanged (no confident cheaper
    /// seat — correct is better than clever).
    Session,
    /// In-family split: session provider, mid-tier driver model.
    SessionModel { model: String },
    /// Cross-family: a different provider drives (e.g. Flux's routed lane,
    /// where per-turn routing happens server-side).
    Provider {
        provider: String,
        model: Option<String>,
    },
}

impl AnvilConfig {
    /// Resolve the driver seat: explicit config → Flux routed lane when
    /// connected → in-family mid tier → session unchanged. Pure decision
    /// logic; the caller materializes (and falls back to the session seat if
    /// materialization fails — seat routing must never break a forge).
    pub fn resolve_driver_seat(
        &self,
        session: ProviderType,
        connected: &[ProviderType],
    ) -> DriverSeatPlan {
        // 1. Explicit configuration always wins.
        if let Some(provider) = &self.driver_provider {
            return DriverSeatPlan::Provider {
                provider: provider.clone(),
                model: self.driver_model.clone(),
            };
        }
        if let Some(model) = &self.driver_model {
            return DriverSeatPlan::SessionModel {
                model: model.clone(),
            };
        }
        // 2. A connected Flux key routes the driver seat through the router's
        //    auto lane (per-turn tiering server-side) — unless Flux already IS
        //    the session provider, in which case the session config governs.
        if session != ProviderType::FluxRouter && connected.contains(&ProviderType::FluxRouter) {
            return DriverSeatPlan::Provider {
                provider: "flux-router".to_string(),
                model: Some("flux-auto".to_string()),
            };
        }
        // 3. In-family mid tier, when the family has a confident one.
        match driver_model_for(session) {
            "" => DriverSeatPlan::Session,
            model => DriverSeatPlan::SessionModel {
                model: model.to_string(),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_on() {
        // ON by default: availability, not activity — the forge is still
        // invocation-only and refuses without a gate.
        assert!(AnvilConfig::default().enabled);
    }

    #[test]
    fn absent_table_deserializes_to_enabled() {
        let cfg: AnvilConfig = toml::from_str("").unwrap();
        assert!(cfg.enabled);
        assert!(cfg.gate.is_empty());
    }

    #[test]
    fn kill_switch_round_trips() {
        // `enabled = false` must survive parse — it is the only way to make
        // the rail inert now that the default is on.
        let cfg: AnvilConfig = toml::from_str("enabled = false\n").unwrap();
        assert!(!cfg.enabled);
    }

    #[test]
    fn explicit_driver_provider_beats_flux_auto_routing() {
        let cfg: AnvilConfig =
            toml::from_str("driver_provider = \"openai\"\ndriver_model = \"gpt-5.5\"\n").unwrap();
        let plan = cfg.resolve_driver_seat(
            ProviderType::Anthropic,
            &[ProviderType::Anthropic, ProviderType::FluxRouter],
        );
        assert_eq!(
            plan,
            DriverSeatPlan::Provider {
                provider: "openai".into(),
                model: Some("gpt-5.5".into()),
            }
        );
    }

    #[test]
    fn explicit_driver_model_pins_session_provider() {
        let cfg: AnvilConfig = toml::from_str("driver_model = \"claude-haiku-4-5\"\n").unwrap();
        let plan = cfg.resolve_driver_seat(ProviderType::Anthropic, &[ProviderType::FluxRouter]);
        assert_eq!(
            plan,
            DriverSeatPlan::SessionModel {
                model: "claude-haiku-4-5".into(),
            }
        );
    }

    #[test]
    fn connected_flux_key_routes_the_driver_seat() {
        let cfg = AnvilConfig::default();
        let plan = cfg.resolve_driver_seat(
            ProviderType::Anthropic,
            &[ProviderType::Anthropic, ProviderType::FluxRouter],
        );
        assert_eq!(
            plan,
            DriverSeatPlan::Provider {
                provider: "flux-router".into(),
                model: Some("flux-auto".into()),
            }
        );
    }

    #[test]
    fn flux_session_does_not_route_to_itself() {
        // Flux-as-session already routes server-side; re-routing would fight
        // the user's own session model choice.
        let cfg = AnvilConfig::default();
        let plan = cfg.resolve_driver_seat(ProviderType::FluxRouter, &[ProviderType::FluxRouter]);
        assert_eq!(plan, DriverSeatPlan::Session);
    }

    #[test]
    fn anthropic_without_flux_splits_in_family() {
        let cfg = AnvilConfig::default();
        let plan = cfg.resolve_driver_seat(ProviderType::Anthropic, &[ProviderType::Anthropic]);
        match plan {
            DriverSeatPlan::SessionModel { model } => assert!(model.contains("sonnet")),
            other => panic!("expected in-family split, got {other:?}"),
        }
    }

    #[test]
    fn unknown_family_keeps_the_session_seat() {
        // No confident mid tier for Tier-2 catalogs → the session drives.
        let cfg = AnvilConfig::default();
        let plan = cfg.resolve_driver_seat(ProviderType::Deepseek, &[ProviderType::Deepseek]);
        assert_eq!(plan, DriverSeatPlan::Session);
    }
}
