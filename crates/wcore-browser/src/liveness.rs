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

/// The Camoufox sidecar program the supervisor would spawn, **asked of the
/// supervisor's own production config** rather than re-derived here.
///
/// This used to be a second copy of `SupervisorConfig::local_camoufox`'s
/// resolution — the same `WAYLAND_CAMOUFOX_BIN`-or-`"camofox-browser"`
/// expression, written out twice, with a docstring *claiming* the two agreed
/// and nothing enforcing it. That claim is the whole probe: if the probe
/// resolves a different program than the supervisor spawns, the flag it
/// publishes is about a program nobody runs, and `27-C2(b)` is back in both
/// directions —
///
/// * probe finds the stale name, supervisor spawns the new one ⇒ advertise
///   `true`, then die at first use, which is the original defect verbatim;
/// * probe finds the new name absent while the supervisor would have spawned
///   the installed stale one ⇒ withdraw a **working** capability, which the
///   `Indeterminate` variant exists specifically to prevent.
///
/// Deriving it from the config makes that drift unrepresentable rather than
/// merely tested-for. `None` is meaningful and is not a fallback to a guess:
/// the supervisor documents it as observe-only mode, in which `ensure_ready`
/// spawns nothing, so there is no binary for this probe to look for either.
fn camoufox_program(cfg: &crate::supervisor::SupervisorConfig) -> Option<&str> {
    cfg.sidecar_program.as_deref()
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
///
/// The `chromium` feature returns `Indeterminate` unconditionally below, which
/// makes the Camoufox block — the only reader of `camoufox_base_url` — dead
/// code in that configuration. The parameter stays in the signature because the
/// default shipped build does use it; the allow is scoped to the one feature
/// that cannot, and mirrors the existing `unreachable_code` allow on that block.
#[cfg_attr(feature = "chromium", allow(unused_variables))]
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
        // Build the supervisor's real production config ONCE and read both the
        // program and the healthcheck URL out of it, so the probe cannot
        // disagree with the thing it is predicting. See `camoufox_program`.
        let cfg = crate::supervisor::SupervisorConfig::local_camoufox(camoufox_base_url);

        // `None` (observe-only mode) is NOT a failure here — it means the
        // supervisor was told not to spawn anything, so fall through to the
        // healthcheck rather than looking for a binary.
        if camoufox_program(&cfg).is_some_and(program_resolves) {
            return BrowserLiveness::Ready {
                via: "camoufox-binary",
            };
        }

        // No local binary, but an externally managed sidecar (e.g. one the
        // desktop app launched) is a real, working deployment. This is the
        // same first check `BrowserSupervisor::ensure_ready` makes — and now
        // literally against the same config value it makes it against.
        let healthcheck_url = cfg.healthcheck_url.clone();
        let program = camoufox_program(&cfg).map(str::to_owned);
        let supervisor = crate::supervisor::BrowserSupervisor::with_config(cfg);
        if supervisor
            .healthcheck(Duration::from_millis(500))
            .await
            .unwrap_or(false)
        {
            return BrowserLiveness::Ready {
                via: "camoufox-sidecar",
            };
        }

        // Report the URL the supervisor would actually poll, not a URL
        // reconstructed here — `local_camoufox` trims a trailing slash before
        // appending `/health`, so a caller passing `".../"` previously got a
        // remedy naming `...//health`, an address nothing serves.
        let reason = match program.as_deref() {
            Some(program) => format!(
                "no browser backend can start: `{program}` does not resolve on PATH and no \
                 sidecar answered {healthcheck_url}"
            ),
            None => format!(
                "no browser backend can start: the supervisor is in observe-only mode (no \
                 sidecar command configured) and no externally managed sidecar answered \
                 {healthcheck_url}"
            ),
        };
        BrowserLiveness::Unavailable(Unavailable {
            reason,
            // gh#491 - the ONE install instruction, shared with the
            // supervisor's refusal and with `--doctor`.
            remedy: crate::install::CAMOUFOX.remedy(),
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
    #[serial_test::serial(camoufox_bin_env)]
    fn camoufox_program_honours_the_operator_override() {
        // The previous comment claimed "serialised implicitly: the only env var
        // touched is this one, and no other test in this module reads it".
        // False on the second half: `nothing_installed_and_nothing_listening_is_
        // unavailable`, in THIS module, also sets `WAYLAND_CAMOUFOX_BIN`. Under
        // `cargo test` both run concurrently in one process, so each could
        // observe the other's value. Nothing about touching a single variable
        // serialises anything.
        let key = "WAYLAND_CAMOUFOX_BIN";
        let prior = std::env::var_os(key);
        unsafe { std::env::set_var(key, "/opt/custom/camoufox") };
        let cfg = crate::supervisor::SupervisorConfig::local_camoufox("http://127.0.0.1:1");
        assert_eq!(camoufox_program(&cfg), Some("/opt/custom/camoufox"));
        unsafe {
            match prior {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }
    }

    /// **The drift guard.** `27-C2(b)` was closed by making the advertised flag
    /// depend on a probe. The probe is only worth the flag if it predicts what
    /// the supervisor *actually does*, and the original version re-derived the
    /// program name in a second copy of `local_camoufox`'s expression — two
    /// literals, a docstring asserting they agreed, and nothing enforcing it.
    ///
    /// This is modelled on the guard that closed clause (a): rather than compare
    /// the probe's string to the supervisor's string (which passes just as
    /// happily when *both* have drifted away from what ships), it round-trips
    /// through the **real production constructor** and asserts the probe's own
    /// public verdict flips with it. `local_camoufox` is the exact call
    /// `adapter.rs:51` makes on the engine's startup path.
    ///
    /// Both directions are asserted in one test, because a same-string check is
    /// satisfied by a probe that is stuck on either answer.
    #[tokio::test]
    #[serial_test::serial(camoufox_bin_env)]
    #[cfg(not(any(feature = "chromium", feature = "browserbase")))]
    async fn the_probe_reads_the_program_out_of_the_supervisors_own_config() {
        let key = "WAYLAND_CAMOUFOX_BIN";
        let prior = std::env::var_os(key);

        // Positive arm. Something the supervisor could really spawn. `which`
        // takes an absolute path as given and never executes it, so the running
        // test binary is a valid stand-in for an installed sidecar.
        let real = std::env::current_exe().expect("resolve this test binary");
        unsafe { std::env::set_var(key, &real) };
        let live_cfg = crate::supervisor::SupervisorConfig::local_camoufox("http://127.0.0.1:1");
        let live_program = camoufox_program(&live_cfg).map(str::to_owned);
        let live_verdict = probe("http://127.0.0.1:1").await;

        // Negative arm. Same code path, same URL; only the program is dead.
        unsafe { std::env::set_var(key, "wcore-browser-drift-guard-no-such-program") };
        let dead_cfg = crate::supervisor::SupervisorConfig::local_camoufox("http://127.0.0.1:1");
        let dead_program = camoufox_program(&dead_cfg).map(str::to_owned);
        let dead_verdict = probe("http://127.0.0.1:1").await;

        unsafe {
            match prior {
                Some(v) => std::env::set_var(key, v),
                None => std::env::remove_var(key),
            }
        }

        // 1. The supervisor's production config is what decides, and it moved.
        //    If these are equal the arms are not different experiments and
        //    everything below would be measuring one state twice.
        assert_ne!(
            live_program, dead_program,
            "the two arms produced the same sidecar program, so this test compares a \
             state with itself"
        );
        assert_eq!(
            live_program.as_deref(),
            Some(real.to_str().expect("test binary path is UTF-8")),
            "the probe did not read the program the supervisor would spawn — it is \
             predicting a different binary than `ensure_ready` launches, which is \
             27-C2(b) with an extra step"
        );

        // 2. The probe's public verdict tracks it, in BOTH directions. A stuck
        //    probe fails one of these two whichever answer it is stuck on.
        assert!(
            !live_verdict.should_narrow(),
            "the supervisor would spawn `{live_program:?}`, which resolves, yet the probe \
             withdrew the capability: {live_verdict:?}"
        );
        assert!(
            dead_verdict.should_narrow(),
            "the supervisor would spawn `{dead_program:?}`, which cannot resolve, and no \
             sidecar answers port 1, yet the probe kept advertising: {dead_verdict:?}"
        );

        // 3. The reason names the program the SUPERVISOR would spawn and the
        //    URL it would poll. This is the half a string-compare cannot reach:
        //    an operator acts on this text, so a reason quoting a binary the
        //    engine never launches sends them to fix the wrong thing.
        let u = dead_verdict
            .unavailable()
            .expect("Unavailable carries detail");
        assert!(
            u.reason
                .contains("wcore-browser-drift-guard-no-such-program"),
            "reason does not name the supervisor's own program: {}",
            u.reason
        );
        assert_eq!(
            dead_cfg.healthcheck_url, "http://127.0.0.1:1/health",
            "the config's healthcheck URL is not what this test believes it is"
        );
        assert!(
            u.reason.contains(&dead_cfg.healthcheck_url),
            "reason does not name the URL the supervisor would poll ({}): {}",
            dead_cfg.healthcheck_url,
            u.reason
        );
    }

    /// Observe-only mode is a real supported configuration (`sidecar_program:
    /// None` — the supervisor spawns nothing and an external owner runs the
    /// sidecar). The probe must then fall through to the healthcheck instead of
    /// looking for a binary, and must say so. Before the config became the
    /// single source of truth this state was unrepresentable in the probe, which
    /// would have kept hunting a `"camofox-browser"` the supervisor had been
    /// told not to launch.
    #[test]
    fn observe_only_mode_has_no_binary_to_look_for() {
        let cfg = crate::supervisor::SupervisorConfig::default();
        assert_eq!(
            cfg.sidecar_program, None,
            "the default is documented as observe-only; this test's premise is gone"
        );
        assert_eq!(camoufox_program(&cfg), None);
        // And the production constructor is genuinely different from the
        // default, or the assertion above would be describing the shipped path.
        let prod = crate::supervisor::SupervisorConfig::local_camoufox("http://127.0.0.1:1");
        assert!(
            camoufox_program(&prod).is_some(),
            "the production config names no sidecar command; the engine would never \
             start a browser"
        );
    }

    /// End-to-end on the default feature set: a program that cannot exist plus
    /// a port nothing is listening on must produce `Unavailable`, carrying both
    /// a reason and a remedy. If this ever returns `Ready`, the probe is inert.
    #[cfg(not(any(feature = "chromium", feature = "browserbase")))]
    #[tokio::test]
    #[serial_test::serial(camoufox_bin_env)]
    async fn nothing_installed_and_nothing_listening_is_unavailable() {
        // Shares `WAYLAND_CAMOUFOX_BIN` with
        // `camoufox_program_honours_the_operator_override`; same serial group.
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
