#[allow(dead_code)]
#[path = "../build.rs"]
mod build_script;

use std::ffi::OsString;

#[test]
fn explicit_source_sha_wins_over_git() {
    let explicit = "0123456789abcdef0123456789abcdef01234567";
    let resolved = build_script::resolve_source_sha(Some(OsString::from(explicit)), || {
        Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string())
    })
    .expect("valid explicit source SHA");

    assert_eq!(resolved, explicit);
}

#[test]
fn absent_explicit_source_sha_uses_git() {
    let git = "fedcba9876543210fedcba9876543210fedcba98";
    let resolved = build_script::resolve_source_sha(None, || Some(git.to_string()))
        .expect("valid Git source SHA");

    assert_eq!(resolved, git);
}

#[test]
fn absent_source_identity_remains_non_authoritative() {
    let resolved =
        build_script::resolve_source_sha(None, || None).expect("ordinary archive build fallback");

    assert_eq!(resolved, "unknown");
}

#[test]
fn explicit_source_sha_rejects_uppercase_or_malformed_values() {
    for invalid in [
        "unknown",
        "ABCDEF0123456789ABCDEF0123456789ABCDEF01",
        "abc123",
        "gggggggggggggggggggggggggggggggggggggggg",
    ] {
        let error = build_script::resolve_source_sha(Some(OsString::from(invalid)), || None)
            .expect_err("invalid explicit source SHA must fail closed");
        assert!(error.contains("40 lowercase hexadecimal"), "{error}");
    }
}

#[cfg(unix)]
#[test]
fn explicit_source_sha_rejects_non_unicode_values() {
    use std::os::unix::ffi::OsStringExt;

    let error = build_script::resolve_source_sha(Some(OsString::from_vec(vec![0xff])), || None)
        .expect_err("non-Unicode explicit source SHA must fail closed");

    assert!(error.contains("valid Unicode"), "{error}");
}
