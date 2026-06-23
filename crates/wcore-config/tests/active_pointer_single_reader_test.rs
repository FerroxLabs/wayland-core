//! D2 single-reader lint for the isolated-profile `active` pointer.
//!
//! C2/D2: the `active` profile pointer is resolved ONCE, at process entry
//! (`profile::activate_for_launch`), and materialized into `WAYLAND_HOME`; it is
//! never consulted again. The credential/memory corruption bug is precisely what
//! happens when a sticky pointer is re-read deep in the stack and can disagree
//! with the env var. To keep that discipline locally reviewable, ALL access to
//! the pointer — via the path resolver AND via the raw `<root>/active` join —
//! must stay inside the one module that owns it:
//! `crates/wcore-config/src/profile.rs`. Phase 2's `profile use` writer lives
//! there too; the CLI calls named helpers, never the path directly.
//!
//! Scope/limits (honest): this is a CALL-SITE regression gate, not a proof of
//! impossibility. It bans the two realistic proliferation shapes — calling the
//! `active_pointer_path` resolver, and hand-building the pointer path with a
//! `join` on the `active` literal — from any file but the allow-listed module.
//! A determined bypass (capturing the resolver as a fn-pointer, or reconstructing
//! the path from raw string ops) is not detected; the goal is to stop accidental
//! copy-paste, which is how that failure actually crept in.
//!
//! Mirrors `hermeticity_audit_test.rs`: shells out to `git grep`
//! (dependency-free), skips comment-only mentions, and is skipped entirely if
//! `git`/`.git` is unavailable (no false negative, no flake).

use std::path::PathBuf;
use std::process::Command;

/// The sole file allowed to touch the `active` pointer path.
const ALLOWLIST: &[&str] = &["crates/wcore-config/src/profile.rs"];

fn workspace_root() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir
        .parent()
        .and_then(|p| p.parent())
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            panic!(
                "CARGO_MANIFEST_DIR ({}) has fewer than two ancestors; cannot locate workspace root",
                manifest_dir.display()
            )
        })
}

/// Non-comment hits, under `crates/`, for either the resolver call form or a raw
/// `join` on the `active` pointer literal. The regex carries backslashes, so the
/// pattern string in THIS source file does not match itself (the bytes on disk
/// here are `…path\(`, not `…path(`).
fn violation_hits(root: &PathBuf) -> Option<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .arg("grep")
        .arg("-nE")
        .arg(r#"active_pointer_path\(|\.join\("active"\)"#)
        .arg("--")
        .arg("crates/")
        .output()
        .ok()?;

    // `git grep` exits 1 on zero matches — success for us. Any other non-zero
    // means git itself failed; skip rather than flake.
    if !output.status.success() && output.status.code() != Some(1) {
        eprintln!(
            "active_pointer_single_reader: `git grep` failed (status {:?}); skipping",
            output.status.code()
        );
        return Some(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut hits = Vec::new();
    for raw_line in stdout.lines() {
        // Line shape: `<path>:<lineno>:<content>`.
        let mut parts = raw_line.splitn(3, ':');
        let path = match parts.next() {
            Some(p) => p,
            None => continue,
        };
        let _lineno = parts.next();
        let content = match parts.next() {
            Some(c) => c,
            None => continue,
        };

        let trimmed = content.trim_start();
        // Skip comment-only mentions (`//`, `///`, `//!`, block-comment `*`).
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }
        if ALLOWLIST.contains(&path) {
            continue;
        }
        hits.push(format!("{path}: {}", content.trim()));
    }
    Some(hits)
}

#[test]
fn active_pointer_is_accessed_in_exactly_one_module() {
    let root = workspace_root();
    if !root.join(".git").exists() {
        eprintln!("active_pointer_single_reader: no .git at {root:?}; skipping");
        return;
    }
    let Some(hits) = violation_hits(&root) else {
        eprintln!("active_pointer_single_reader: git unavailable; skipping");
        return;
    };
    assert!(
        hits.is_empty(),
        "the active-pointer path must only be accessed from {ALLOWLIST:?} (C2/D2 \
         single-reader invariant — resolved once at launch, never re-read). \
         Offending sites:\n{}",
        hits.join("\n")
    );
}
