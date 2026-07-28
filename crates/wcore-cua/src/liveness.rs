//! Can computer use actually start on this machine?
//!
//! Ledger row `27-C2(b)`, CUA half. `capabilities.computer_use` on the `ready`
//! event was derived from **linkage** — `PluginRunner::new()
//! .with_computer_use_advertised(true)` is called unconditionally, so a
//! headless box advertised computer use and the first operation failed.
//!
//! Same three rules as `wcore_browser::liveness`: the probe can only narrow
//! `true` → `false`, it narrows only on positive proof of unavailability, and
//! it never executes anything.
//!
//! The Linux check is not a guess about the environment — it is exactly the
//! precondition the backends themselves enforce.
//! `backends/linux_x11.rs` fails fast with a typed error when `DISPLAY` is
//! unset, and `Platform::current` distinguishes Wayland from X11 solely by
//! `WAYLAND_DISPLAY`. So a `false` here predicts a failure the operator would
//! certainly have hit, rather than substituting our judgement for the backend's.
//!
//! macOS and Windows report [`CuaLiveness::Indeterminate`]: their backends
//! (CGEvent, UI Automation) need a live window-server session, and there is no
//! non-executing probe that reliably distinguishes one from an SSH shell. Being
//! unsure keeps the capability — stripping a working feature is the same defect
//! class as advertising a broken one.

use crate::backend::Platform;

/// Why computer use cannot start, and what the operator can do about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub reason: String,
    pub remedy: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CuaLiveness {
    /// A backend can start. `via` names the platform path.
    Ready { via: &'static str },
    /// Provably unable to start. **Only this variant narrows the flag.**
    Unavailable(Unavailable),
    /// Might work; determining it would mean driving the platform API.
    Indeterminate { platform: &'static str },
}

impl CuaLiveness {
    /// `true` ONLY for [`CuaLiveness::Unavailable`].
    pub fn should_narrow(&self) -> bool {
        matches!(self, CuaLiveness::Unavailable(_))
    }

    pub fn unavailable(&self) -> Option<&Unavailable> {
        match self {
            CuaLiveness::Unavailable(u) => Some(u),
            _ => None,
        }
    }
}

/// Probe whether computer use can start on this host.
pub fn probe() -> CuaLiveness {
    match Platform::current() {
        Platform::Unsupported => CuaLiveness::Unavailable(Unavailable {
            reason: "wcore-cua has no backend for this build target".to_string(),
            remedy: "computer use is available on macOS, Linux (X11/Wayland) and Windows"
                .to_string(),
        }),
        Platform::LinuxWayland => CuaLiveness::Ready {
            via: "linux-wayland",
        },
        Platform::LinuxX11 => {
            // `Platform::current` returns LinuxX11 whenever WAYLAND_DISPLAY is
            // absent — including on a headless box with no display at all. That
            // is precisely the case the X11 backend refuses at connect time.
            if std::env::var_os("DISPLAY").is_some() {
                CuaLiveness::Ready { via: "linux-x11" }
            } else {
                CuaLiveness::Unavailable(Unavailable {
                    reason: "neither DISPLAY nor WAYLAND_DISPLAY is set, so no display server \
                             is reachable and the X11 backend cannot connect"
                        .to_string(),
                    remedy: "run inside a graphical session, or export DISPLAY for an \
                             available X server (e.g. an Xvfb instance) before starting"
                        .to_string(),
                })
            }
        }
        Platform::MacOs => CuaLiveness::Indeterminate { platform: "macos" },
        Platform::Windows => CuaLiveness::Indeterminate {
            platform: "windows",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unavailable_narrows_the_flag() {
        assert!(!CuaLiveness::Ready { via: "x" }.should_narrow());
        assert!(!CuaLiveness::Indeterminate { platform: "x" }.should_narrow());
        assert!(
            CuaLiveness::Unavailable(Unavailable {
                reason: "r".into(),
                remedy: "m".into(),
            })
            .should_narrow(),
            "if a provably dead backend does not narrow, this module is a no-op"
        );
    }

    /// The Linux arm is the whole point of the CUA half of 27-C2(b): a headless
    /// box must stop advertising computer use. Both directions are asserted so
    /// the probe cannot be trivially always-true or always-false.
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_narrows_without_a_display_and_keeps_the_capability_with_one() {
        // These two vars are what `Platform::current` and the X11 backend read;
        // no other test in this module touches them.
        let prior_x11 = std::env::var_os("DISPLAY");
        let prior_wl = std::env::var_os("WAYLAND_DISPLAY");

        unsafe {
            std::env::remove_var("DISPLAY");
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        let headless = probe();
        assert!(
            headless.should_narrow(),
            "a machine with no display still advertised computer use: {headless:?}"
        );
        let u = headless
            .unavailable()
            .expect("headless must explain itself");
        assert!(u.reason.contains("DISPLAY"), "{}", u.reason);
        assert!(!u.remedy.is_empty());

        unsafe { std::env::set_var("DISPLAY", ":0") };
        let with_display = probe();
        assert!(
            !with_display.should_narrow(),
            "a machine WITH a display lost the capability: {with_display:?}"
        );

        unsafe {
            match prior_x11 {
                Some(v) => std::env::set_var("DISPLAY", v),
                None => std::env::remove_var("DISPLAY"),
            }
            if let Some(v) = prior_wl {
                std::env::set_var("WAYLAND_DISPLAY", v);
            }
        }
    }

    /// macOS/Windows must not narrow — there is no honest non-executing probe
    /// for a window-server session, and guessing would strip a working feature.
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    #[test]
    fn gui_platforms_do_not_narrow() {
        assert!(!probe().should_narrow(), "{:?}", probe());
    }
}
