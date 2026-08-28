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

use std::path::PathBuf;

use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::{SandboxCommand, SandboxManifest};

/// A workspace with one secret in it, plus the manifest that denies reading it.
///
/// Returned rather than inlined so every leg below is asked the SAME question
/// about the SAME field: a backend that claims to enforce `fs_read_deny` has to
/// do something observable with THIS path.
fn denied_secret() -> (tempfile::TempDir, PathBuf, SandboxManifest) {
    let root = tempfile::tempdir().expect("tempdir");
    let secret = root.path().join(".env");
    std::fs::write(&secret, "SECRET=wcore-pairing-canary").expect("write secret");
    let manifest = SandboxManifest {
        fs_read_allow: vec![root.path().to_path_buf()],
        fs_read_deny: vec![secret.clone()],
        ..Default::default()
    };
    (root, secret, manifest)
}

/// Unix-shaped path LITERALS for the pure-function SBPL check.
///
/// `build_profile` renders a macOS profile, which is why the claim is checkable
/// off macOS at all. But it can only be checked against paths the format has a
/// spelling for: handed a Windows tempdir path (`C:\\WINDOWS\\SERVIC~1\\...`)
/// it emits a profile carrying only its Unix defaults, the expected rule can
/// never appear, and the leg fails for the platform rather than for the claim.
///
/// This leg never opens these paths -- it is a string check on a pure function --
/// so a literal is the correct input here and a real tempdir is not. The live
/// legs keep the tempdir, because they do open theirs.
fn denied_secret_unix_literals() -> (PathBuf, SandboxManifest) {
    let root = PathBuf::from("/tmp/wcore-pairing-root");
    let secret = root.join(".env");
    let manifest = SandboxManifest {
        fs_read_allow: vec![root],
        fs_read_deny: vec![secret.clone()],
        ..Default::default()
    };
    (secret, manifest)
}

fn cat(secret: &std::path::Path) -> SandboxCommand {
    SandboxCommand {
        argv: vec!["cat".into(), secret.to_string_lossy().into()],
        cwd: None,
    }
}

/// Every `enforces_read_deny()` answer must be PAID FOR by what the backend does
/// with `manifest.fs_read_deny` — not merely declared.
///
/// # Why the old shape was not a test (wayland#934, 2026-08-28)
///
/// This asserted `assert!(BubblewrapBackend::new().enforces_read_deny())` and four
/// siblings: five hardcoded booleans compared against the five hardcoded booleans
/// the functions return. **Measured:** with bwrap's `deny_mounts` forced empty AND
/// `SandboxExecBackend::build_profile` emitting no `(deny file-read* …)` rule at
/// all — two backends that claim enforcement and apply nothing, which is precisely
/// the stale-NEGATIVE that makes R1's skip unsafe — this test passed.
///
/// So each leg now observes the field. The enforcing backends are asked to act on
/// it; the relaxed ones are asked to prove they do not, because `false` being a
/// STATEMENT OF FACT rather than modesty is what makes #922's skip observationally
/// free rather than a leak.
#[tokio::test]
async fn every_enforcing_backend_reports_it_and_every_relaxed_one_does_not() {
    let (_root, secret, manifest) = denied_secret();

    // -- ENFORCING, leg 1: bwrap, live. Linux only, and the one backend whose
    //    enforcement can be exercised end to end on the host this suite runs on.
    #[cfg(target_os = "linux")]
    {
        let bwrap = wcore_sandbox::backends::bwrap::BubblewrapBackend::new();
        assert!(
            bwrap.enforces_read_deny(),
            "bwrap claims to enforce fs_read_deny"
        );
        assert!(
            bwrap.is_available(),
            "bwrap is not installed, so the one leg of this pairing that can be driven live \
             cannot run. Refusing to report green: a claim checked by nothing is the defect \
             this test exists to catch. Install bubblewrap, or run this suite on a host with it."
        );
        let out = bwrap
            .execute(&manifest, cat(&secret))
            .await
            .expect("bwrap must spawn");
        assert!(
            !String::from_utf8_lossy(&out.stdout).contains("wcore-pairing-canary"),
            "bwrap answers enforces_read_deny() == true but the denied bytes came back: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    // -- ENFORCING, leg 2: sandbox_exec. Its enforcement is an SBPL rule and
    //    `build_profile` is a pure function, so the claim is checkable off macOS
    //    even though the sandbox itself is not.
    {
        let se = wcore_sandbox::backends::sandbox_exec::SandboxExecBackend::new();
        assert!(
            se.enforces_read_deny(),
            "sandbox_exec claims to enforce fs_read_deny"
        );
        let (unix_secret, unix_manifest) = denied_secret_unix_literals();
        let profile =
            wcore_sandbox::backends::sandbox_exec::SandboxExecBackend::build_profile(&unix_manifest)
                .expect("the profile must build for a manifest with an ordinary path");
        let rule = format!(
            "(deny file-read* (subpath \"{}\"))",
            unix_secret.to_string_lossy()
        );
        assert!(
            profile.contains(&rule),
            "sandbox_exec answers enforces_read_deny() == true but its profile carries no rule \
             for the denied path. Expected {rule:?} in:\n{profile}"
        );
        // SBPL is last-match-wins, so a deny emitted BEFORE the allow of the
        // enclosing root is inert. The ordering is part of the enforcement.
        let allow_root = format!(
            "(subpath \"{}\")",
            unix_secret.parent().expect("literal has a parent").to_string_lossy()
        );
        if let Some(a) = profile.find(&allow_root) {
            assert!(
                profile.find(&rule).is_some_and(|d| d > a),
                "the deny rule precedes the allow of its enclosing root, so SBPL's \
                 last-match-wins semantics make it inert"
            );
        }
    }

    // -- RELAXED. `false` here is load-bearing for #922 R1, which SKIPS building
    //    the deny list when a backend reports it. That skip is free only while
    //    `false` really does mean "this field is discarded", so prove it is.
    {
        let no_sandbox = wcore_sandbox::backends::no_sandbox::NoSandboxBackend::new();
        assert!(
            !no_sandbox.enforces_read_deny(),
            "no_sandbox enforces nothing"
        );
        let out = no_sandbox
            .execute(&manifest, cat(&secret))
            .await
            .expect("no_sandbox must spawn");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("wcore-pairing-canary"),
            "no_sandbox reports enforces_read_deny() == false but the denied path was NOT \
             readable — either it grew an enforcement it does not declare (R1 would then skip \
             a backend that enforces: the stale-NEGATIVE leak direction) or this probe is \
             broken. stdout={:?} stderr={:?}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
    }

    // `windows_job_object` is the shipped Windows session default and delegates
    // execution to `NoSandboxBackend`. The delegation is the whole claim, so it is
    // exercised rather than assumed: the same manifest, the same secret, the same
    // readable result.
    {
        let jo = wcore_sandbox::backends::windows_job_object::WindowsJobObjectBackend::new();
        assert!(
            !jo.enforces_read_deny(),
            "windows_job_object must keep the trait default — R1's whole premise"
        );
        let out = jo
            .execute(&manifest, cat(&secret))
            .await
            .expect("windows_job_object must spawn");
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("wcore-pairing-canary"),
            "windows_job_object reports false but took some filesystem action on fs_read_deny; \
             its `false` is only honest while it delegates to NoSandboxBackend unchanged"
        );
    }

    // Docker without the `live-docker` feature cannot enforce anything and
    // must keep the trait default. With the feature it hardcodes `true`.
    //
    // Both arms are DECLARATION-ONLY and that is stated rather than hidden: this
    // suite has no daemon to drive, so the live arm's claim is checked by
    // `crates/wcore-sandbox/tests/` docker suites under that feature, not here.
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
