//! Build-provenance invariant (Known Bug #4).
//!
//! Asserts that the compiled binary was built from the current repo HEAD.
//! A stale prebuilt binary (built from a different commit) will fail this
//! test, catching the "forgot to rebuild after commit" class of error.

#[test]
fn binary_matches_repo_head() {
    let Ok(output) = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    else {
        // git not available — skip; provenance check is only meaningful where git exists
        return;
    };
    if !output.status.success() {
        // not a git repo (box gate, tarball build) — skip
        return;
    }
    let head = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if head.is_empty() {
        return;
    }

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-core"))
        .arg("--build-info")
        .output()
        .unwrap();
    let info = String::from_utf8_lossy(&out.stdout);

    assert!(
        info.contains(&head),
        "binary built from {info:?} != repo HEAD {head} (stale build)"
    );
}
