//! S3 verification — construct a Landlock ruleset from a manifest and
//! confirm the helper builds it without error before the
//! `restrict_self()` call. We deliberately do NOT actually invoke
//! `restrict_self()` from the test process: that would lock the entire
//! cargo-test runner out of paths beyond `/tmp`, breaking every test that
//! runs after this one in the same binary.
//!
//! Linux + `landlock` feature only.

#![cfg(all(target_os = "linux", feature = "landlock"))]

use std::path::PathBuf;
use wcore_sandbox::backends::bwrap_landlock::{LandlockOutcome, restrict_self_from_paths};
use wcore_sandbox::error::SandboxError;

#[test]
fn empty_manifest_returns_unsupported() {
    let out = restrict_self_from_paths(&[], &[]).expect("ok");
    assert_eq!(out, LandlockOutcome::Unsupported);
}

#[test]
fn nonexistent_path_is_path_denied() {
    let bogus = PathBuf::from("/this/path/does/not/exist/wcore-sandbox-landlock-test");
    let res = restrict_self_from_paths(&[bogus], &[]);
    assert!(
        matches!(res, Err(SandboxError::PathDenied(_))),
        "expected PathDenied, got {res:?}"
    );
}
