//! Build-provenance invariant (Known Bug #4).
//!
//! Dual-mode design:
//!
//! - **Default (local dev):** asserts the binary's embedded source SHA is an
//!   ancestor of the current repo HEAD (`git merge-base --is-ancestor`).  This
//!   is green after the common "build → commit → run tests" sequence because
//!   the build SHA is simply an older point in the same lineage.  The invariant
//!   still catches foreign or corrupt builds (SHA not in repo history at all).
//!
//! - **Strict (CI / overnight / fresh-build gate):** when `CI` or
//!   `PROVING_GROUND_STRICT_PROVENANCE` is set, also asserts the embedded SHA
//!   equals the current HEAD exactly.  This is only correct where the binary
//!   is guaranteed to have been freshly built at HEAD (e.g. `cargo build`
//!   immediately before `cargo nextest run` in the same CI job).
//!
//! Why not always strict?  In local dev the workflow is: build → work →
//! commit → run tests.  After the commit HEAD advances but the binary's baked
//! SHA doesn't change (Cargo's `rerun-if-changed` guards don't re-trigger on a
//! plain `git commit`).  Requiring `embedded == HEAD` would red-fail on every
//! commit-after-build, which is noise, not signal.

#[test]
fn binary_matches_repo_head() {
    // ── 1. Resolve git HEAD ───────────────────────────────────────────────
    let Ok(head_out) = std::process::Command::new("git")
        .args(["rev-parse", "--short", "HEAD"])
        .output()
    else {
        // git not available — skip; provenance check is only meaningful where git exists
        return;
    };
    if !head_out.status.success() {
        // not a git repo (box gate, tarball build) — skip
        return;
    }
    let head = String::from_utf8_lossy(&head_out.stdout).trim().to_string();
    if head.is_empty() {
        return;
    }

    // ── 2. Extract embedded SHA from `wayland-core --build-info` ─────────
    let info_out = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-core"))
        .arg("--build-info")
        .output()
        .unwrap();
    let info = String::from_utf8_lossy(&info_out.stdout);

    // Parse the "(source <sha>)" token.
    let embedded_sha = info
        .split_whitespace()
        .skip_while(|t| *t != "(source")
        .nth(1)
        .map(|t| t.trim_end_matches(')').to_string())
        .unwrap_or_default();

    assert!(
        !embedded_sha.is_empty() && embedded_sha != "unknown",
        "binary did not emit a real source SHA in --build-info output: {info:?}"
    );

    // ── 3. Default mode: embedded SHA must be an ancestor of HEAD ─────────
    //
    // `git merge-base --is-ancestor A B` exits 0 iff A is an ancestor of B
    // (or A == B).  This passes when the binary was built at HEAD or any
    // earlier commit in the same lineage.
    let ancestor_status = std::process::Command::new("git")
        .args(["merge-base", "--is-ancestor", &embedded_sha, &head])
        .status();

    match ancestor_status {
        Ok(s) if s.success() => {} // ancestor check passed — fall through to strict
        Ok(_) => {
            panic!(
                "binary source {embedded_sha} is not reachable from HEAD \
                 {head} — foreign or corrupt build"
            );
        }
        Err(e) => {
            panic!("git merge-base --is-ancestor failed: {e}");
        }
    }

    // ── 4. Strict mode: embedded SHA must equal HEAD exactly ──────────────
    //
    // Enabled when CI or PROVING_GROUND_STRICT_PROVENANCE is set (any
    // non-empty value).  Only safe in environments where the binary is
    // freshly built at HEAD immediately before the test run.
    let strict = std::env::var("CI").map(|v| !v.is_empty()).unwrap_or(false)
        || std::env::var("PROVING_GROUND_STRICT_PROVENANCE")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

    if strict {
        assert_eq!(
            embedded_sha, head,
            "STRICT: binary source {embedded_sha} != HEAD {head} \
             (stale build — rebuild required)"
        );
    }
}
