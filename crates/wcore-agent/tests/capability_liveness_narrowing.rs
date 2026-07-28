//! Regression guard for ledger row `27-C2(b)` — capability flags that lie to
//! the Desktop app.
//!
//! `capabilities.browser_suite` / `.computer_use` on the `ready` event were
//! derived from LINKAGE: whether the plugin crate was discovered and
//! identity-verified. On a headless host both read `true`, the desktop app
//! rendered the capability, and the first operation died with
//! `spawn camoufox: No such file or directory`.
//!
//! `PluginCapabilitySet::narrowed_to_live` layers the backend crates' own
//! liveness probes on top. This file pins the three properties that make that
//! safe, and each can fail independently:
//!
//! 1. **It narrows.** A host with the plugin loaded and no backend that can
//!    start must stop advertising. If `narrowed_to_live` became a no-op — the
//!    easy way for this fix to rot — `narrows_when_no_backend_can_start` goes
//!    red and `27-C2(b)` is open again.
//! 2. **It only ever narrows.** A `false` from the identity check must never
//!    become `true`. That check is a Wave SC SECURITY MAJOR fix (a malicious
//!    crate named `wayland-browser` must not flip a UI badge); a probe that
//!    could widen would silently undo it.
//! 3. **It does not narrow on ignorance.** A backend whose startability cannot
//!    be determined without launching it keeps the capability. A cross-audit
//!    panel found this false-negative class unanimously: stripping a working
//!    capability from a user's UI is the same defect as advertising a broken
//!    one, pointed the other way.
//!
//! **No wire-contract surface is touched.** Nothing here constructs, compares
//! or regenerates a `wcore-protocol` contract artifact: the field name, type
//! and value domain are unchanged, and `false` is already what a host sees when
//! the plugin is absent.

use wcore_agent::output::protocol_sink::PluginCapabilitySet;
use wcore_browser::liveness::BrowserLiveness;
use wcore_cua::liveness::CuaLiveness;

/// Point the browser probe at a program that cannot exist and a loopback port
/// nothing listens on, so "no backend can start" is the true state of the world
/// rather than an assumption.
struct NoBackend {
    prior: Option<std::ffi::OsString>,
}

impl NoBackend {
    fn install() -> Self {
        let prior = std::env::var_os("WAYLAND_CAMOUFOX_BIN");
        unsafe {
            std::env::set_var(
                "WAYLAND_CAMOUFOX_BIN",
                "wcore-agent-liveness-guard-no-such-program",
            )
        };
        Self { prior }
    }
}

impl Drop for NoBackend {
    fn drop(&mut self) {
        unsafe {
            match self.prior.take() {
                Some(v) => std::env::set_var("WAYLAND_CAMOUFOX_BIN", v),
                None => std::env::remove_var("WAYLAND_CAMOUFOX_BIN"),
            }
        }
    }
}

/// Property 2, and the security-critical one: the probe must not be able to
/// advertise a capability the identity check refused.
#[tokio::test]
async fn never_widens_a_capability_the_identity_check_refused() {
    let none = PluginCapabilitySet::default();
    assert!(!none.browser_suite && !none.computer_use, "precondition");

    let after = none.narrowed_to_live().await;

    assert!(
        !after.browser_suite,
        "liveness probing SET browser_suite that identity verification had refused — \
         this would undo the Wave SC plugin-impersonation fix"
    );
    assert!(
        !after.computer_use,
        "liveness probing SET computer_use that identity verification had refused"
    );
}

/// Property 1. Only meaningful where the probe is capable of a definite
/// negative verdict; on a build with `chromium`/`browserbase` compiled in the
/// browser probe is deliberately `Indeterminate` (property 3), so the assertion
/// is gated on the probe's own verdict rather than on an assumption about the
/// host.
#[tokio::test]
async fn narrows_when_no_backend_can_start() {
    let _guard = NoBackend::install();

    let browser_verdict = wcore_browser::liveness::probe("http://127.0.0.1:1").await;
    let cua_verdict = wcore_cua::liveness::probe();

    let advertised = PluginCapabilitySet {
        browser_suite: true,
        computer_use: true,
    };
    let after = advertised.narrowed_to_live().await;

    if browser_verdict.should_narrow() {
        assert!(
            !after.browser_suite,
            "the browser backend is provably unable to start ({browser_verdict:?}) yet \
             browser_suite is still advertised — 27-C2(b) is not fixed"
        );
    }
    if cua_verdict.should_narrow() {
        assert!(
            !after.computer_use,
            "computer use is provably unable to start ({cua_verdict:?}) yet computer_use \
             is still advertised — 27-C2(b) is not fixed"
        );
    }

    // The guard must not be vacuous: on this build+host at least one probe has
    // to be able to reach a definite verdict, or neither branch above ran and
    // the test proved nothing.
    assert!(
        browser_verdict.should_narrow() || cua_verdict.should_narrow() ||
            // A host that genuinely has a display and a browser installed is a
            // legitimate reason for both to be Ready. Name it explicitly so a
            // silently-inert probe cannot hide behind this arm.
            matches!(browser_verdict, BrowserLiveness::Ready { .. })
            || matches!(cua_verdict, CuaLiveness::Ready { .. }),
        "both probes returned Indeterminate ({browser_verdict:?}, {cua_verdict:?}); this \
         run asserted nothing about narrowing"
    );
}

/// Property 3 — ignorance keeps the capability.
#[test]
fn indeterminate_never_narrows() {
    assert!(
        !BrowserLiveness::Indeterminate {
            backend: "chromium"
        }
        .should_narrow(),
        "an undecidable browser backend dropped the capability — this strips browsing \
         from working Chromium/Browserbase deployments"
    );
    assert!(
        !CuaLiveness::Indeterminate { platform: "macos" }.should_narrow(),
        "an undecidable CUA platform dropped the capability"
    );
}
