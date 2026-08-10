//! What does `AppContainerBackend::enforces_read_deny()` actually answer?
//!
//! # Why this file exists
//!
//! One predicate carries the entire safety argument for weakening the Windows
//! sandbox posture. Two independent-looking gates consult it:
//!
//! - `wcore-agent/src/channel_tools.rs` — `keep_under` drops `Bash` from the
//!   `Workspace` channel posture when `read_deny_enforced` is false. `Bash` is
//!   in `WORKSPACE_FS_TOOLS`, so without that drop the tool is advertised.
//! - `wcore-tools/src/bash.rs` — the exec-time gate refuses the spawn when
//!   `p.secret_read_deny_required() && !backend.enforces_read_deny()`.
//!
//! The second is documented as "the authoritative boundary" and the first as
//! its "UX-drop companion". They are not independent: both call THE SAME
//! predicate. So a wrong answer here opens both at once, and nothing else is
//! checking.
//!
//! The net is sound only if the predicate is POLICY-derived — "is the active
//! manifest's `fs_read_deny` actually enforced?". These tests record that it is
//! LIVENESS-derived — "has the availability probe settled as FAILED?" — and
//! that it is fail-open on the unsettled case.
//!
//! # These are characterization tests
//!
//! They pin behaviour that is currently WRONG so the next change to it is
//! deliberate rather than accidental. They are EXPECTED to go red when the
//! predicate is corrected. When that happens, invert them and keep them; do not
//! delete them. Each assertion message says what the corrected answer should be.
//!
//! # Running them
//!
//! `settled_verdict()` is process-global, so a sibling test that probes first
//! would destroy the unprobed observation. Run this file's cases in their own
//! process:
//!
//! ```text
//! cargo test -p wcore-sandbox read_deny_claim -- --test-threads=1 --nocapture
//! ```

use super::process::{AppContainerBackend, containment_claim, settled_verdict};
use crate::backends::SandboxBackend;
use crate::manifest::SandboxManifest;
use std::path::PathBuf;

/// The predicate's entire input domain, and its answer for each.
///
/// `containment_claim` is `settled != Some(false)`. That means the claim is
/// asserted for `None` — before any probe has demonstrated anything about this
/// host. A capability is being advertised in advance of evidence for it.
#[test]
fn read_deny_claim_is_asserted_before_any_probe_has_run() {
    assert!(
        containment_claim(Some(true)),
        "a settled-AVAILABLE probe claims containment. Correct, and expected."
    );
    assert!(
        !containment_claim(Some(false)),
        "a settled-UNAVAILABLE probe withdraws the claim. This is the ONLY \
         input that fires the Bash-drop safety net."
    );
    assert!(
        containment_claim(None),
        "FAIL-OPEN, and this is the finding: an UNPROBED backend already claims \
         to enforce secret-read-deny. Correct behaviour would be to withhold the \
         claim until a probe has settled AVAILABLE, i.e. `settled == Some(true)`. \
         If this assertion ever fails, the fail-open has been closed — invert this \
         test, do not delete it."
    );
}

/// The claim cannot depend on the policy it claims to enforce, because the
/// predicate never receives it.
///
/// `fn enforces_read_deny(&self) -> bool` takes no manifest. So relaxing the
/// Windows profile — emptying `fs_read_deny` while keeping this backend —
/// cannot change the answer. The safety net does not fire, and the shell stays
/// advertised and spawnable while the deny it is predicated on is gone.
#[test]
fn read_deny_claim_cannot_observe_the_policy_it_speaks_for() {
    let strict = SandboxManifest {
        fs_read_deny: vec![PathBuf::from(r"C:\p0-fixture\.env")],
        ..SandboxManifest::default()
    };
    let permissive = SandboxManifest::default();

    // Non-vacuity guard. Invariance across two IDENTICAL manifests would prove
    // nothing at all, so assert the inputs genuinely differ in exactly the
    // field the predicate purports to speak for.
    assert!(
        !strict.fs_read_deny.is_empty(),
        "vacuity guard: the strict manifest must actually deny something"
    );
    assert!(
        permissive.fs_read_deny.is_empty(),
        "vacuity guard: the permissive manifest must actually deny nothing"
    );
    assert_ne!(
        strict.fs_read_deny, permissive.fs_read_deny,
        "vacuity guard: the two manifests must differ in `fs_read_deny`"
    );

    let backend = AppContainerBackend::new();
    let claim = backend.enforces_read_deny();

    // There is deliberately no call here that could consume either manifest:
    // demonstrating that is the whole point. `enforces_read_deny` has no
    // parameter through which the difference above could ever reach it.
    assert!(
        claim,
        "on a host whose probe has not settled UNAVAILABLE the claim stands \
         regardless of policy. If this fails, check whether this host's \
         AppContainer probe settled as failed — that is a host condition, not \
         a refutation of the finding."
    );
}

/// The live backend on this host, measured before and after a real probe.
///
/// Prints both observations so a reader can see which branch produced the
/// verdict rather than taking the assertion's word for it.
#[test]
fn live_backend_claims_read_deny_both_unprobed_and_probed() {
    let backend = AppContainerBackend::new();

    let unprobed_settled = settled_verdict();
    let unprobed_claim = backend.enforces_read_deny();
    eprintln!(
        "P0(a) UNPROBED: settled_verdict={unprobed_settled:?} \
         enforces_read_deny={unprobed_claim}"
    );

    // Force the real availability probe, then ask the same question again.
    let available = backend.is_available();
    let probed_settled = settled_verdict();
    let probed_claim = backend.enforces_read_deny();
    eprintln!(
        "P0(b) PROBED: is_available={available} settled_verdict={probed_settled:?} \
         enforces_read_deny={probed_claim}"
    );

    if unprobed_settled.is_none() {
        assert!(
            unprobed_claim,
            "measured fail-open: no probe had settled, yet the backend claimed \
             read-deny enforcement"
        );
    } else {
        eprintln!(
            "P0(a) NOT GRADED: a sibling test probed first, so the unprobed \
             observation was unavailable in this process. Re-run this case \
             alone to grade it."
        );
    }

    assert_eq!(
        probed_claim,
        probed_settled != Some(false),
        "the claim must track the settled verdict and nothing else"
    );

    if available {
        assert!(
            probed_claim,
            "host probe succeeded, so the claim stands — and would stand \
             identically under a permissive profile"
        );
    } else {
        eprintln!(
            "P0(b) HOST CONDITION: this host's AppContainer probe did not \
             succeed, so the claim was withdrawn for a liveness reason. That is \
             the net firing for the wrong reason, not evidence that the \
             predicate is policy-aware."
        );
    }
}
