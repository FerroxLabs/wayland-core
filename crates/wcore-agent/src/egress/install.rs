//! B2.4 — install the egress policy into the B1 `wcore-egress` chokepoint.

use std::sync::Arc;

use wcore_config::config::Config;

use super::defaults::build_allowlist;
use super::policy::AgentEgressPolicy;

/// Build the egress policy from `config`. AgentBootstrap uses this constructor
/// to own one policy per session; `install_egress_policy` below remains the
/// compatibility entry point for process-level callers.
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
pub fn policy_from_config(config: &Config) -> AgentEgressPolicy {
    // One line per process, not one per constructed policy. The posture is a
    // property of the installed GLOBAL chokepoint, and only the first policy
    // built ever becomes that — `install_global_policy` is one-shot. A normal
    // CLI launch builds two (main's `install_egress_policy`, then
    // `AgentBootstrap`'s per-session policy), which wrote the identical
    // "egress security ENFORCING" line to the diagnostics log twice, 118 µs
    // apart, describing one decision.
    //
    // One `Once` PER POSTURE, not one shared between them: a process that
    // builds an enforcing policy and later a disabled one has made two
    // security-relevant decisions, and a single latch would silently drop
    // whichever came second. Quieting a repeat must not be able to quiet a
    // change.
    static ENFORCING_ANNOUNCED: std::sync::Once = std::sync::Once::new();
    static DISABLED_ANNOUNCED: std::sync::Once = std::sync::Once::new();
    if config.security.enabled {
        let allow = build_allowlist(config);
        ENFORCING_ANNOUNCED.call_once(|| {
            tracing::info!(
                allowlisted = allow.len(),
                "egress security ENFORCING — exfil-shaped traffic to non-allowlisted external hosts is blocked"
            );
        });
        AgentEgressPolicy::enforcing(allow)
    } else {
        DISABLED_ANNOUNCED.call_once(|| {
            tracing::warn!(
                "egress security DISABLED via [security] enabled=false — outbound exfiltration is NOT gated"
            );
        });
        AgentEgressPolicy::disabled()
    }
}

pub fn install_egress_policy(config: &Config) {
    if wcore_egress::global_policy_installed() {
        return;
    }
    let policy = policy_from_config(config);
    let policy: wcore_egress::SharedPolicy = Arc::new(policy);
    // One-shot: ignore the Err(returned-policy) if another path won the race.
    let _ = wcore_egress::install_global_policy(policy);
}
