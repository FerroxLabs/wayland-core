//! Can the browser capability actually start on this machine?
//!
//! Ledger row `27-C2(b)`: `capabilities.browser_suite` on the `ready` event was
//! derived from **linkage** — whether the `wayland-browser` plugin crate was
//! discovered and identity-verified. On a headless box with no browser binary
//! the flag read `true`, the desktop app rendered the capability, and the first
//! operation died with `spawn camoufox: No such file or directory`. The product
//! advertised something it could not do.
//!
//! This module answers the narrower, honest question the flag is supposed to
//! encode: *is there a compiled-in backend that can start?*
//!
//! # Three rules this probe obeys
//!
//! **1. It can only ever narrow `true` → `false`.** Liveness is applied on top
//! of the existing linkage + identity check, never instead of it. A machine
//! without the plugin still reports `false`; the probe cannot resurrect a
//! capability the host never verified.
//!
//! **2. Narrowing requires positive proof of unavailability, not absence of
//! proof of availability.** A cross-audit panel unanimously found the
//! false-negative class this design originally missed: `select_provider` has
//! three backends, and probing only Camoufox would strip a working capability
//! from a Chromium or Browserbase deployment. So any compiled-in backend whose
//! startability cannot be established *without launching it* returns
//! [`BrowserLiveness::Indeterminate`], which does **not** narrow. Only
//! [`BrowserLiveness::Unavailable`] — every compiled-in backend provably unable
//! to start — narrows the flag. Under-advertising a working capability is the
//! same defect class as over-advertising a broken one, pointed the other way.
//!
//! **3. It never executes anything.** `bootstrap.rs` carries a hard-won note
//! against `<command> --help` preflights: they run third-party code with the
//! ambient parent environment, leak secrets, and disagree with Windows PATHEXT
//! semantics. Binary presence is resolved with the `which` crate (PATHEXT-aware,
//! non-executing); the only other probe is an HTTP GET against a loopback
//! healthcheck the engine already performs in `BrowserSupervisor::ensure_ready`.
//!
//! The probe deliberately mirrors `ensure_ready`'s two real startup paths — a
//! resolvable sidecar program, or an externally managed sidecar already
//! answering `/health` — so it predicts the failure the operator would actually
//! hit rather than a proxy for it.

use std::time::Duration;

/// Why the capability cannot start, and what the operator can do about it.
///
/// The remedy exists because of a recorded panel dissent: silently dropping a
/// capability from the UI replaces an explicit, actionable runtime error with an
/// un-debuggable missing feature. Callers log this so the reason survives even
/// though the flag does not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unavailable {
    pub reason: String,
    pub remedy: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserLiveness {
    /// A compiled-in backend can start. `via` names which one.
    Ready { via: &'static str },
    /// Every compiled-in backend is provably unable to start. **Only this
    /// variant narrows the advertised flag.**
    Unavailable(Unavailable),
    /// A compiled-in backend might work, and finding out would mean launching
    /// it. Never narrows — see rule 2 in the module docs.
    Indeterminate { backend: &'static str },
}

impl BrowserLiveness {
    /// The single question the capability flag asks. `true` ONLY for
    /// [`BrowserLiveness::Unavailable`] — `Indeterminate` keeps the capability.
    pub fn should_narrow(&self) -> bool {
        matches!(self, BrowserLiveness::Unavailable(_))
    }

    /// The unavailability detail, when there is one.
    pub fn unavailable(&self) -> Option<&Unavailable> {
        match self {
            BrowserLiveness::Unavailable(u) => Some(u),
            _ => None,
        }
    }
}

/// The Camoufox sidecar program the supervisor would spawn — the same
/// resolution `SupervisorConfig::local_camoufox` performs.
fn camoufox_program() -> String {
    std::env::var("WAYLAND_CAMOUFOX_BIN").unwrap_or_else(|_| "camofox-browser".to_string())
}

/// Does `program` resolve to an executable? PATHEXT-aware on Windows via
/// `which`. An absolute or relative path is checked as given, matching how
/// `Command::spawn` would treat it. **Resolves only — never executes.**
fn program_resolves(program: &str) -> bool {
    which::which(program).is_ok()
}

/// Probe whether the browser capability can start.
///
/// `camoufox_base_url` is the sidecar base URL (no trailing slash), normally
/// `CamoufoxBackend::default_url()`. Only contacted when the local binary does
/// not resolve, so an installed deployment pays nothing.
pub async fn probe(camoufox_base_url: &str) -> BrowserLiveness {
    // Cloud backend: compiled in AND credentialed means a machine with no local
    // browser at all can still browse. Whether `select_provider` ultimately
    // picks it depends on the hint and on the F17 policy refusal, which this
    // probe deliberately does not try to predict — being unsure keeps the
    // capability rather than dropping it.
    #[cfg(feature = "browserbase")]
    if std::env::var_os("BROWSERBASE_API_KEY").is_some()
        && std::env::var_os("BROWSERBASE_PROJECT_ID").is_some()
    {
        return BrowserLiveness::Indeterminate {
            backend: "browserbase",
        };
    }

    // chromiumoxide discovers a system Chrome/Chromium during `Browser::launch`.
    // There is no non-executing probe for that, and rule 3 forbids launching it,
    // so a build with this feature on never narrows.
    #[cfg(feature = "chromium")]
    {
        return BrowserLiveness::Indeterminate {
            backend: "chromium",
        };
    }

    // Camoufox — the only backend in the default shipped build.
    #[cfg_attr(feature = "chromium", allow(unreachable_code))]
    {
        let program = camoufox_program();
        if program_resolves(&program) {
            return BrowserLiveness::Ready {
                via: "camoufox-binary",
            };
        }

        // No local binary, but an externally managed sidecar (e.g. one the
        // desktop app launched) is a real, working deployment. This is the
        // same first check `BrowserSupervisor::ensure_ready` makes.
        let supervisor = crate::supervisor::BrowserSupervisor::with_config(
            crate::supervisor::SupervisorConfig::local_camoufox(camoufox_base_url),
        );
        if supervisor
            .healthcheck(Duration::from_millis(500))
            .await
            .unwrap_or(false)
        {
            return BrowserLiveness::Ready {
                via: "camoufox-sidecar",
            };
        }

        BrowserLiveness::Unavailable(Unavailable {
            reason: format!(
                "no browser backend can start: `{program}` does not resolve on PATH and no \
                 sidecar answered {camoufox_base_url}/health"
            ),
            remedy: "install @askjo/camofox-browser, or set WAYLAND_CAMOUFOX_BIN to the \
                     executable, or start the Camoufox sidecar before the session"
                .to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_unavailable_narrows_the_flag() {
        assert!(
            !BrowserLiveness::Ready { via: "x" }.should_narrow(),
            "a working backend must never drop the capability"
        );
        assert!(
            !BrowserLiveness::Indeterminate { backend: "x" }.should_narrow(),
            "unsure must keep the capability — under-advertising is the same defect \
             class as over-advertising"
        );
        assert!(
            BrowserLiveness::Unavailable(Unavailable {
                reason: "r".into(),
                remedy: "m".into(),
            })
            .should_narrow(),
            "a provably dead backend must drop the capability — otherwise this whole \
             module is a no-op and 27-C2(b) is untouched"
        );
    }

    #[test]
    fn a_program_that_cannot_exist_does_not_resolve() {
        assert!(!program_resolves(
            "wcore-browser-liveness-probe-no-such-program"
        ));
    }

    /// The probe must resolve a real executable, or `should_narrow` would be
    /// unconditionally true and the flag would read `false` everywhere — a
    /// self-passing probe pointed the wrong way.
    #[test]
    fn a_program_that_certainly_exists_does_resolve() {
        let real = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(
            program_resolves(real),
            "`{real}` did not resolve; the PATH probe is broken and would strip the \
             capability from every machine"
        );
    }

    #[test]
    fn camoufox_program_honours_the_operator_override() {
        // Serialised implicitly: the only env var touched is this one, and no
        // other test in this module reads it.
        let key = "WAYLAND_CAMOUFOX_BIN";
        let prior = std::env::var_os(key);
        unsafe { std::env::set_var(key, "/opt/custom/camoufox") };
        assert_eq!(camoufox_program(), "/opt/custom/camoufox");
        unsafe {
            match prior {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    /// End-to-end on the default feature set: a program that cannot exist plus
    /// a port nothing is listening on must produce `Unavailable`, carrying both
    /// a reason and a remedy. If this ever returns `Ready`, the probe is inert.
    #[cfg(not(any(feature = "chromium", feature = "browserbase")))]
    #[tokio::test]
    async fn nothing_installed_and_nothing_listening_is_unavailable() {
        let key = "WAYLAND_CAMOUFOX_BIN";
        let prior = std::env::var_os(key);
        unsafe { std::env::set_var(key, "wcore-browser-liveness-probe-no-such-program") };

        // Port 1 on loopback: reserved, never a Camoufox sidecar, refuses fast.
        let got = probe("http://127.0.0.1:1").await;

        unsafe {
            match prior {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }

        let u = got
            .unavailable()
            .unwrap_or_else(|| panic!("expected Unavailable with no backend present, got {got:?}"));
        assert!(u.reason.contains("does not resolve"), "{}", u.reason);
        assert!(
            !u.remedy.is_empty(),
            "an unavailable must say how to fix it"
        );
    }
}
