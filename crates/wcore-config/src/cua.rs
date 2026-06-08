//! W8c.2 F.1 — `CuaConfig` TOML schema for the multi-platform computer-
//! use tool family. Matches design §5.18 surface.
//!
//! Thin config crate — the actual platform-backend selection lives in
//! `wcore_cua::backends::for_platform`. We mirror the operator-facing
//! fields here so config loading stays in `wcore-config` (which owns
//! cascade + profile).

use serde::{Deserialize, Serialize};

/// Policy mirror — `wcore_cua::CuaPolicy` accepts these fields too.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CuaPolicyConfig {
    /// Apps that require HITL approval on every op.
    pub require_approval_for_app: Vec<String>,
    /// Apps the agent cannot drive at all.
    pub forbidden_apps: Vec<String>,
    /// Key combinations rejected outright.
    pub forbidden_key_combos: Vec<String>,
    /// When `true`, the first op against a new app routes to Suspend.
    pub first_time_per_app_approval: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CuaConfig {
    pub policy: CuaPolicyConfig,
    /// Blur sensitive UI patterns in screenshots before returning bytes.
    pub redact_screenshots: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_toml() {
        let cfg = CuaConfig {
            policy: CuaPolicyConfig {
                require_approval_for_app: vec!["Keychain Access".into()],
                forbidden_apps: vec!["1Password".into()],
                forbidden_key_combos: vec!["cmd+q+system".into()],
                first_time_per_app_approval: true,
            },
            redact_screenshots: true,
        };
        let s = toml::to_string(&cfg).unwrap();
        let parsed: CuaConfig = toml::from_str(&s).unwrap();
        assert_eq!(parsed.policy.forbidden_apps, vec!["1Password".to_string()]);
        assert!(parsed.redact_screenshots);
        assert!(parsed.policy.first_time_per_app_approval);
    }

    #[test]
    fn empty_toml_uses_defaults() {
        let cfg: CuaConfig = toml::from_str("").unwrap();
        assert!(cfg.policy.require_approval_for_app.is_empty());
        assert!(cfg.policy.forbidden_apps.is_empty());
        assert!(!cfg.redact_screenshots);
        assert!(!cfg.policy.first_time_per_app_approval);
    }
}
