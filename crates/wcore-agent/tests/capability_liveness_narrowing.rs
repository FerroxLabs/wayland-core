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
//! 4. **It says so.** A narrowing that only ever reached `tracing::warn!` was
//!    invisible: with `RUST_LOG` unset stderr takes ERROR only, so the one
//!    diagnostic explaining a vanished capability never reached the user
//!    (#1130). `narrowed_to_live` now returns the fact, and
//!    `narrowing_carries_the_words_a_user_needs` pins that the returned line
//!    names the capability, the reason and the remedy.
//!
//! **No wire-contract surface is touched.** Nothing here constructs, compares
//! or regenerates a `wcore-protocol` contract artifact: the field name, type
//! and value domain are unchanged, and `false` is already what a host sees when
//! the plugin is absent.

use wcore_agent::output::protocol_sink::{CapabilityNarrowing, PluginCapabilitySet};
use wcore_browser::liveness::BrowserLiveness;
use wcore_cua::liveness::CuaLiveness;

/// A loopback port that is reserved and never served. Planted as the sidecar
/// base URL so the healthcheck arm of the probe is provably dead.
const DEAD_SIDECAR_URL: &str = "http://127.0.0.1:1";

/// Point the browser probe at a program that cannot exist **and** at a sidecar
/// URL nothing answers, so "no backend can start" is the true state of the
/// world rather than an assumption.
///
/// Both facts have to be planted, because the probe deliberately mirrors
/// `BrowserSupervisor::ensure_ready`'s TWO real startup paths: a resolvable
/// sidecar program, or an externally managed sidecar already answering
/// `/health`. This guard used to plant only the first and let the second fall
/// through to `CamoufoxBackend::default_url()`, an ambient fact it does not
/// control. On any host actually running a Camoufox sidecar on the default
/// port — a supported deployment, and the standing state of the Linux build
/// box — that second path stayed live, the probe correctly answered `Ready`,
/// and this test failed against a product that was telling the truth. The
/// oracle below then compared a `probe(127.0.0.1:1)` verdict against a
/// `narrowed_to_live()` that had probed port 9377: two different experiments.
struct NoBackend {
    prior_bin: Option<std::ffi::OsString>,
    prior_url: Option<std::ffi::OsString>,
}

impl NoBackend {
    fn install() -> Self {
        let prior_bin = std::env::var_os("WAYLAND_CAMOUFOX_BIN");
        let prior_url = std::env::var_os("WAYLAND_CAMOUFOX_URL");
        unsafe {
            std::env::set_var(
                "WAYLAND_CAMOUFOX_BIN",
                "wcore-agent-liveness-guard-no-such-program",
            );
            std::env::set_var("WAYLAND_CAMOUFOX_URL", DEAD_SIDECAR_URL);
        };
        Self {
            prior_bin,
            prior_url,
        }
    }
}

impl Drop for NoBackend {
    fn drop(&mut self) {
        unsafe {
            match self.prior_bin.take() {
                Some(v) => std::env::set_var("WAYLAND_CAMOUFOX_BIN", v),
                None => std::env::remove_var("WAYLAND_CAMOUFOX_BIN"),
            }
            match self.prior_url.take() {
                Some(v) => std::env::set_var("WAYLAND_CAMOUFOX_URL", v),
                None => std::env::remove_var("WAYLAND_CAMOUFOX_URL"),
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

    let (after, narrowings) = none.narrowed_to_live().await;

    assert!(
        narrowings.is_empty(),
        "nothing was advertised, so nothing could be narrowed, yet a notice was produced: \
         {narrowings:?}"
    );
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

    // Same URL the guard planted, so the oracle and `narrowed_to_live` below
    // are the same experiment. The URL is a fact this test PLANTS, not one it
    // reads back out of the probe.
    let browser_verdict = wcore_browser::liveness::probe(DEAD_SIDECAR_URL).await;
    let cua_verdict = wcore_cua::liveness::probe();

    let advertised = PluginCapabilitySet {
        browser_suite: true,
        computer_use: true,
    };
    let (after, narrowings) = advertised.narrowed_to_live().await;

    // #1130 — every cleared flag must arrive with the words to explain it.
    // A narrowing the caller is not handed is a narrowing the user is never
    // told about, which is the whole of the defect.
    let narrowed_count =
        usize::from(browser_verdict.should_narrow()) + usize::from(cua_verdict.should_narrow());
    assert_eq!(
        narrowings.len(),
        narrowed_count,
        "{narrowed_count} capability/capabilities were dropped but {} notice(s) came back \
         ({narrowings:?}) — a dropped capability with no notice is #1130 reopened",
        narrowings.len()
    );

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
            backend: "browserbase"
        }
        .should_narrow(),
        "an undecidable browser backend dropped the capability — this strips browsing \
         from working Browserbase deployments"
    );
    assert!(
        !CuaLiveness::Indeterminate { platform: "macos" }.should_narrow(),
        "an undecidable CUA platform dropped the capability"
    );
}

/// Property 4 — the returned narrowing carries the words a person can act on.
///
/// Pure, so it grades the SENTENCE on every host rather than only on one with
/// no display and no browser. Both directions in one instrument: the notice
/// must contain the capability, the probe's reason and the probe's remedy, and
/// must NOT contain a field that was never put in it — without that half, an
/// assertion that three substrings are present is satisfied by a renderer that
/// concatenates the whole struct's `Debug`.
#[test]
fn narrowing_carries_the_words_a_user_needs() {
    let narrowing = CapabilityNarrowing {
        capability: "browser_suite",
        reason: "no browser backend can start: `camofox-browser` does not resolve on PATH"
            .to_string(),
        remedy: "npm install -g @askjo/camofox-browser".to_string(),
    };
    let notice = narrowing.notice();

    assert!(
        notice.contains("browser_suite"),
        "the notice does not name the capability: {notice}"
    );
    assert!(
        notice.contains("does not resolve on PATH"),
        "the notice drops the probe's reason, so the user cannot tell what is wrong: {notice}"
    );
    assert!(
        notice.contains("npm install -g @askjo/camofox-browser"),
        "the notice drops the probe's remedy, so the user cannot tell what to do: {notice}"
    );
    // CAN-FAIL half: a renderer that dumped the struct would pass the three
    // assertions above while being unreadable. `Unavailable`/`capability:` are
    // shapes only a Debug dump produces.
    assert!(
        !notice.contains("capability:") && !notice.contains("CapabilityNarrowing"),
        "the notice looks like a Debug dump rather than a sentence: {notice}"
    );
}
