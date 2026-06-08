//! B2.4 — install the egress policy into the B1 `wcore-egress` chokepoint.

use std::sync::Arc;

use wcore_config::config::Config;

use super::defaults::build_allowlist;
use super::policy::AgentEgressPolicy;

/// Build the egress policy from `config` and install it as the process-global
/// policy for every [`wcore_egress::EgressClient`].
///
/// Idempotent: the underlying install is one-shot (the first call wins), so
/// repeated calls from sub-agent boots or multiple entry points are no-ops.
/// Call early at startup, before real outbound traffic.
///
/// Posture:
/// - `[security] enabled = true` (default) → **enforcing**: exfil-shaped
///   traffic (POST/PUT/PATCH bodies, shared-platform hosts, GET/HEAD with a
///   long/high-entropy path/query) to non-allowlisted external hosts is denied;
///   local destinations and the auto-derived provider + first-party hosts are
///   allowed.
/// - `[security] enabled = false` → **disabled** (allow-all). This is the
///   config-file-only off switch (C8); the operator accepts the exfiltration
///   risk. A loud warning is logged.
pub fn install_egress_policy(config: &Config) {
    if wcore_egress::global_policy_installed() {
        return;
    }
    let policy: wcore_egress::SharedPolicy = if config.security.enabled {
        let allow = build_allowlist(config);
        tracing::info!(
            allowlisted = allow.len(),
            "egress security ENFORCING — exfil-shaped traffic to non-allowlisted external hosts is blocked"
        );
        let enforcing = AgentEgressPolicy::enforcing(allow);
        // Remember the enforcing policy so a later bootstrap step can attach the
        // B2.5 consent doorbell to this exact (live) instance. The policy is
        // installed here at CLI entry, before the engine/approval bridge exist.
        super::policy::remember_installed(enforcing.clone());
        Arc::new(enforcing)
    } else {
        tracing::warn!(
            "egress security DISABLED via [security] enabled=false — outbound exfiltration is NOT gated"
        );
        Arc::new(AgentEgressPolicy::disabled())
    };
    // One-shot: ignore the Err(returned-policy) if another path won the race.
    let _ = wcore_egress::install_global_policy(policy);
}
