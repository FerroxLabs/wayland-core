//! A2 (#922 R1) — the ONE safety assumption `secret_deny_paths_for_backend`
//! rests on: **no backend that actually enforces `manifest.fs_read_deny`
//! reports `enforces_read_deny() == false`.**
//!
//! R1 skips producing the deny list when this predicate says `false`. That is
//! observationally free only while `false` really does mean "this field is
//! discarded". A backend that UNDER-reports enforcement would turn the skip
//! into a stale-NEGATIVE — the security direction. Over-reporting is the safe
//! direction: we compute a list the backend may not use, which costs latency
//! and nothing else.
//!
//! The `AppContainerBackend` leg is the load-bearing one and is Windows-only,
//! because its answer is DERIVED (`!containment_withdrawn()`) rather than
//! hardcoded. It must answer `true` while the availability probe is unsettled
//! — the property measured on SEANDESKTOP 2026-08-10 and documented at
//! `platform_candidate` in the crate root. Run it on Windows or it proves
//! nothing about the platform #922 is about.
//!
//! Its own file so each assertion gets a fresh process: the AppContainer probe
//! cache is process-global, and a sibling test that probed first would settle
//! the verdict this one needs to observe UNSETTLED.

use wcore_sandbox::backends::SandboxBackend;

#[test]
fn every_enforcing_backend_reports_it_and_every_relaxed_one_does_not() {
    // Enforcing: hardcoded `true`, and they really do apply the field —
    // bwrap by mounting over each path, sandbox_exec by an SBPL deny rule.
    assert!(
        wcore_sandbox::backends::bwrap::BubblewrapBackend::new().enforces_read_deny(),
        "bwrap enforces fs_read_deny by mounting over every denied path"
    );
    assert!(
        wcore_sandbox::backends::sandbox_exec::SandboxExecBackend::new().enforces_read_deny(),
        "sandbox_exec enforces fs_read_deny via SBPL"
    );

    // Relaxed: the trait default. `windows_job_object` is the shipped Windows
    // session default and delegates execution to `NoSandboxBackend`, which
    // takes no filesystem action at all — so `false` here is a statement of
    // fact, and it is exactly what makes #922's walk pure waste.
    assert!(
        !wcore_sandbox::backends::windows_job_object::WindowsJobObjectBackend::new()
            .enforces_read_deny(),
        "windows_job_object must keep the trait default — R1's whole premise"
    );
    assert!(
        !wcore_sandbox::backends::no_sandbox::NoSandboxBackend::new().enforces_read_deny(),
        "no_sandbox enforces nothing"
    );

    // Docker without the `live-docker` feature cannot enforce anything and
    // must keep the trait default. With the feature it hardcodes `true`.
    #[cfg(not(feature = "live-docker"))]
    assert!(
        !wcore_sandbox::backends::docker::DockerBackend::new().enforces_read_deny(),
        "the non-live docker build must not claim an enforcement it cannot apply"
    );
    #[cfg(feature = "live-docker")]
    assert!(
        wcore_sandbox::backends::docker::DockerBackend::new().enforces_read_deny(),
        "the live docker build enforces fs_read_deny via /dev/null binds and empty-dir overlays"
    );
}

/// The AppContainer leg. THIS is the assertion R1's safety rests on: an
/// UNPROBED backend must over-report enforcement, never under-report it.
///
/// Red arm for this test: make `containment_claim` answer `settled ==
/// Some(true)` instead of `settled != Some(false)`. An unsettled verdict then
/// yields `enforces_read_deny() == false`, R1 skips the walk on a backend that
/// would have enforced, and this test must go red. If it does not, it is not
/// testing the property R1 depends on.
#[cfg(windows)]
#[test]
fn appcontainer_over_reports_enforcement_while_the_probe_is_unsettled() {
    // Constructed, never probed: nothing in this process has called
    // `is_available()`, so `settled_verdict()` is `None`.
    let backend = wcore_sandbox::backends::appcontainer::AppContainerBackend::new();
    assert!(
        backend.enforces_read_deny(),
        "an UNPROBED AppContainer backend must claim enforcement: an unknown \
         answer has to over-report (we walk, stale-POSITIVE) and never \
         under-report (we skip, stale-NEGATIVE — the leak direction)"
    );
}

/// Off Windows the AppContainer type is a compile stub that can execute
/// nothing; it must not claim an enforcement either. Pinned so the stub can
/// never drift into a `true` that a non-Windows host would then act on.
#[cfg(not(windows))]
#[test]
fn appcontainer_stub_claims_nothing() {
    let backend = wcore_sandbox::backends::appcontainer::AppContainerBackend::new();
    assert_eq!(backend.name(), "appcontainer_stub");
    assert!(!backend.enforces_read_deny());
    assert!(!backend.is_available());
}
