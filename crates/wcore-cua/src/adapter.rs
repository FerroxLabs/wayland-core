//! Host adapter — translate a `wcore-plugin-api`-mirrored `CuaToolSpec`
//! into a concrete `CuaTool`.
//!
//! Mirrors the audit F2 pattern from `wcore_browser::adapter`. The
//! plugin shell (`wayland-cua`) registers a `CuaToolSpec` through the
//! `wcore-plugin-api` mirror; the host (which DOES depend on
//! `wcore-cua`) calls into this module to reify the spec into a real
//! `CuaTool`.
//!
//! **Audit F7 positive invariance.** When the platform resolves to
//! `LinuxWayland` AND the compositor probe returns false, `from_spec`
//! returns `Err(CuaError::WaylandRestricted)` — registration is
//! refused at bootstrap rather than silently falling back.
//!
//! **Capability gating.** `from_spec` consults `computer_use_advertised`
//! and refuses the registration with `CuaError::CapabilityDisabled`
//! when the host hasn't advertised the flag. The wayland-cua plugin
//! propagates this from `Capabilities.computer_use`.

use std::sync::Arc;

use wcore_plugin_api::cua_spec::CuaToolSpec as ApiCuaToolSpec;

use crate::backend::Platform;
use crate::backends;
use crate::backends::linux_wayland::compositor_allows_background_input;
use crate::error::{CuaError, CuaResult};
use crate::policy::CuaPolicy;
use crate::tool::CuaTool;

/// Field-for-field mirror of `wcore_plugin_api::cua_spec::CuaToolSpec`.
/// The api-crate version lives in `wcore-plugin-api`; this struct allows
/// the host adapter to hand a normalized payload to `from_spec` without
/// forcing `wcore-plugin-api` to depend on `wcore-cua`.
#[derive(Debug, Clone)]
pub struct CuaToolSpecLocal {
    pub tool_namespace: String,
    pub policy: CuaPolicy,
    pub redact_screenshots: bool,
    /// When `false`, the host has NOT advertised `Capabilities.computer_use`
    /// — `from_spec` returns `CapabilityDisabled` so the plugin layer
    /// can surface a clean "no-display" error.
    pub computer_use_advertised: bool,
}

impl Default for CuaToolSpecLocal {
    fn default() -> Self {
        Self {
            tool_namespace: "Cua".into(),
            policy: CuaPolicy::default(),
            redact_screenshots: false,
            computer_use_advertised: true,
        }
    }
}

/// Translate an api-crate `CuaToolSpec` into the host-local form so the
/// host adapter can mint a `CuaTool` via [`from_spec`]. Mirrors the
/// `wcore_browser::adapter` translation surface.
pub fn from_api_spec(spec: ApiCuaToolSpec, computer_use_advertised: bool) -> CuaToolSpecLocal {
    let mut policy = CuaPolicy::permissive();
    policy.require_approval_for_app = spec.policy.require_approval_for_app;
    policy.forbidden_apps = spec.policy.forbidden_apps;
    policy.forbidden_key_combos = spec.policy.forbidden_key_combos;
    policy.first_time_per_app_approval = spec.policy.first_time_per_app_approval;
    CuaToolSpecLocal {
        tool_namespace: spec.tool_namespace,
        policy,
        redact_screenshots: spec.redact_screenshots,
        computer_use_advertised,
    }
}

/// Translate a normalized spec into a concrete `CuaTool`. Bootstrap-time
/// helper for `wcore-agent` once it carries the plugin-spec → tool wiring.
///
/// REV-2 audit F7 fix: positive invariance check at registration time
/// (Linux Wayland with a restricted compositor refuses to register).
pub fn from_spec(spec: CuaToolSpecLocal) -> CuaResult<Arc<CuaTool>> {
    if !spec.computer_use_advertised {
        return Err(CuaError::CapabilityDisabled);
    }
    let platform = Platform::current();
    if matches!(platform, Platform::LinuxWayland) && !compositor_allows_background_input() {
        return Err(CuaError::WaylandRestricted {
            reason: "compositor does not permit cross-application background input".into(),
        });
    }
    let backend = backends::for_platform(platform);
    let _ = spec.redact_screenshots; // wired through `CuaOp::Screenshot.redact` at call sites.
    Ok(Arc::new(
        CuaTool::new(backend, spec.policy).with_namespace(spec.tool_namespace),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    fn clear_wayland_env() {
        unsafe {
            std::env::remove_var("WCORE_CUA_TEST_WAYLAND_PERMISSIVE");
            std::env::remove_var("WCORE_CUA_TEST_WAYLAND_RESTRICTED");
        }
    }

    #[test]
    fn capability_disabled_refuses_to_register() {
        let r = from_spec(CuaToolSpecLocal {
            computer_use_advertised: false,
            ..CuaToolSpecLocal::default()
        });
        assert!(matches!(r, Err(CuaError::CapabilityDisabled)));
    }

    #[test]
    #[serial]
    fn from_spec_succeeds_on_non_linux_or_permissive_wayland() {
        // Force permissive on Linux Wayland; ignored on macOS/Windows.
        clear_wayland_env();
        unsafe { std::env::set_var("WCORE_CUA_TEST_WAYLAND_PERMISSIVE", "1") };
        let r = from_spec(CuaToolSpecLocal::default());
        clear_wayland_env();
        assert!(
            r.is_ok(),
            "expected Ok, got error: {}",
            r.err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "<no error>".into())
        );
        let tool = r.unwrap();
        use wcore_tools::Tool;
        assert_eq!(tool.name(), "Cua");
    }

    /// Audit F7: restricted Wayland compositor refuses registration.
    /// Only meaningful on Linux because `Platform::current()` returns
    /// non-Wayland on other targets; the test stays gated.
    #[cfg(target_os = "linux")]
    #[test]
    #[serial]
    fn restricted_wayland_compositor_refuses_to_register() {
        clear_wayland_env();
        // Force the platform probe to think we're on Wayland AND
        // restricted.
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", "wayland-test");
            std::env::set_var("WCORE_CUA_TEST_WAYLAND_RESTRICTED", "1");
        }
        let r = from_spec(CuaToolSpecLocal::default());
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        clear_wayland_env();
        assert!(
            matches!(r, Err(CuaError::WaylandRestricted { .. })),
            "expected WaylandRestricted, got {}",
            match &r {
                Err(e) => format!("Err({e:?})"),
                Ok(_) => "Ok(_)".to_string(),
            }
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    #[serial]
    fn permissive_wayland_compositor_succeeds() {
        clear_wayland_env();
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", "wayland-test");
            std::env::set_var("WCORE_CUA_TEST_WAYLAND_PERMISSIVE", "1");
        }
        let r = from_spec(CuaToolSpecLocal::default());
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        clear_wayland_env();
        assert!(
            r.is_ok(),
            "expected Ok on permissive Wayland, got {}",
            r.err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| "<no error>".into())
        );
    }
}
