#[allow(dead_code)]
#[path = "../build.rs"]
mod build_script;

use std::ffi::OsString;

/// Cargo's `PROFILE` build-script variable for a debug build.
fn debug() -> Option<OsString> {
    Some(OsString::from("debug"))
}

/// Cargo's `PROFILE` build-script variable for a release build.
fn release() -> Option<OsString> {
    Some(OsString::from("release"))
}

#[test]
fn explicit_source_sha_wins_over_git() {
    let explicit = "0123456789abcdef0123456789abcdef01234567";
    let resolved =
        build_script::resolve_source_sha(Some(OsString::from(explicit)), debug(), || {
            Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string())
        })
        .expect("valid explicit source SHA");

    assert_eq!(resolved, explicit);
}

#[test]
fn absent_explicit_source_sha_uses_git() {
    let git = "fedcba9876543210fedcba9876543210fedcba98";
    let resolved = build_script::resolve_source_sha(None, debug(), || Some(git.to_string()))
        .expect("valid Git source SHA");

    assert_eq!(resolved, git);
}

#[test]
fn absent_source_identity_remains_non_authoritative_for_a_debug_build() {
    let resolved = build_script::resolve_source_sha(None, debug(), || None)
        .expect("ordinary archive build fallback");

    assert_eq!(resolved, "unknown");
}

/// The supply-chain half of the fallback, added 2026-07-29.
///
/// A RELEASE binary built where neither `WAYLAND_BUILD_SOURCE_SHA` nor git can
/// supply an identity used to silently embed the string `"unknown"`, producing
/// an artifact nobody can attribute to a commit. That is the input Phase 29's
/// release-integrity ledger depends on. Release builds now refuse.
#[test]
fn a_release_build_with_no_source_identity_fails_closed() {
    let error = build_script::resolve_source_sha(None, release(), || None)
        .expect_err("a release build with no attributable source must fail closed");

    assert!(error.contains("RELEASE"), "{error}");
    assert!(error.contains("WAYLAND_BUILD_SOURCE_SHA"), "{error}");
}

/// The counterpart that keeps the test above honest: the refusal must be about
/// the ABSENCE of an identity, not about the release profile. A release build
/// that can name its commit — from either source — still succeeds.
#[test]
fn a_release_build_that_can_name_its_commit_still_succeeds() {
    let git = "fedcba9876543210fedcba9876543210fedcba98";
    assert_eq!(
        build_script::resolve_source_sha(None, release(), || Some(git.to_string()))
            .expect("release build with a git identity"),
        git
    );

    let explicit = "0123456789abcdef0123456789abcdef01234567";
    assert_eq!(
        build_script::resolve_source_sha(Some(OsString::from(explicit)), release(), || None)
            .expect("release build with an explicit identity"),
        explicit
    );
}

/// `PROFILE` absent (a build script invoked outside cargo, or a cargo that
/// stopped setting it) must NOT be read as "release" — an unknown profile falls
/// back rather than failing a build for a reason it cannot substantiate.
#[test]
fn an_unknown_profile_is_not_treated_as_a_release_build() {
    assert_eq!(
        build_script::resolve_source_sha(None, None, || None).expect("unknown profile falls back"),
        "unknown"
    );
}

#[test]
fn explicit_source_sha_rejects_uppercase_or_malformed_values() {
    for invalid in [
        "unknown",
        "ABCDEF0123456789ABCDEF0123456789ABCDEF01",
        "abc123",
        "gggggggggggggggggggggggggggggggggggggggg",
    ] {
        let error =
            build_script::resolve_source_sha(Some(OsString::from(invalid)), debug(), || None)
                .expect_err("invalid explicit source SHA must fail closed");
        assert!(error.contains("40 lowercase hexadecimal"), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn explicit_source_sha_rejects_non_unicode_values() {
    use std::os::unix::ffi::OsStringExt;

    let error =
        build_script::resolve_source_sha(Some(OsString::from_vec(vec![0xff])), debug(), || None)
            .expect_err("non-Unicode explicit source SHA must fail closed");

    assert!(error.contains("valid Unicode"), "{error}");
}
