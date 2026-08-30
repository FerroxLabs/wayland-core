//! core#366 d2 ANTI-ROT: no surface outside this crate may reach the orphan
//! scanner without naming its scope.
//!
//! # What went wrong, and why a list could not have caught it
//!
//! `ExecutionBackend::scan_orphans(nonce)` is nonce-scoped by design —
//! `cancel()` wants exactly one run's residue. The trouble was that it was the
//! ONLY shape a caller could write, so "which runs does this cover" was never
//! a decision anybody made; it was a default everybody inherited. The first
//! remedy moved the one call site the ticket named and recorded the caller set
//! in a ledger note written from memory. The note listed three callers. There
//! were five, and one of the two it missed backs `wayland-core backend scan` —
//! the operator gate whose non-zero exit a human wires into CI, which reported
//! `count 0 (MEASURED)` with a labelled leftover sitting in `docker ps -a`.
//!
//! The fix is not a longer list. `orphan::scan_all` / `scan_one` and
//! `ExecutionBackend::scan_orphans_in_scope` all take an
//! [`wcore_exec_backend::contract::OrphanScope`], so a caller that does not
//! choose does not compile, and `cargo check` — not a note — enumerates the
//! caller set. This test closes the one hole the type system leaves: the raw
//! nonce-scoped method is still `pub`, so a new surface could still reach past
//! the scope-carrying entry points and re-acquire the silent default.
//!
//! # The rule is a boundary, not a list of names
//!
//! Inside `crates/wcore-exec-backend/` the two raw methods ARE the
//! implementation: the backends define them, `cancel()` uses the scoped one to
//! verify its own removal, the trait's dispatcher calls both, and the
//! conformance body grades both arms deliberately. Outside it, they are a
//! question asked without saying which question. So the predicate is "is this
//! file inside the crate that owns the contract" — decidable from the path,
//! total over every file, and it needs no maintenance when somebody adds the
//! sixth caller.

use std::path::{Path, PathBuf};

/// The two raw, scope-implicit spellings. `scan_orphans_in_scope` is
/// deliberately absent: it takes an `OrphanScope` and is the sanctioned way
/// through.
const SCOPE_IMPLICIT_CALLS: [&str; 2] = [".scan_orphans(", ".scan_orphans_any_nonce("];

/// The crate that owns the `ExecutionBackend` contract.
const CONTRACT_OWNER: &str = "wcore-exec-backend";

fn crates_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/<crate> has crates/ one level up")
        .to_path_buf()
}

/// Every `.rs` file under `crates/`, with its body. One walk, no skipped
/// directory except build output.
fn workspace_rs_files() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = vec![crates_root()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.expect("readable dir entry").path();
            if path.is_dir() {
                if path.file_name().and_then(|n| n.to_str()) != Some("target") {
                    stack.push(path);
                }
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let body = std::fs::read_to_string(&path).expect("readable source");
            out.push((path, body));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Code lines only. A doc comment quoting `.scan_orphans(` is prose, and a
/// checker that cannot tell prose from code is the instrument error that made
/// the first version of the sibling core#362 guard fail on itself.
fn scope_implicit_call_lines(body: &str) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
        .filter(|line| SCOPE_IMPLICIT_CALLS.iter().any(|c| line.contains(c)))
        .map(str::to_owned)
        .collect()
}

fn owned_by_contract_crate(path: &Path) -> bool {
    path.components().any(|c| c.as_os_str() == CONTRACT_OWNER)
}

#[test]
fn no_surface_outside_the_contract_crate_asks_the_scanner_without_a_scope() {
    let files = workspace_rs_files();

    // CONTROL: the walk must reach real source. An empty walk would make the
    // assertion below vacuously true, which is the exact failure this file
    // exists to prevent one level down.
    assert!(
        files.len() > 100,
        "CONTROL: the walk found only {} .rs files under crates/. Did the layout move?",
        files.len()
    );

    let mut inside = 0usize;
    let mut offenders: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for (path, body) in &files {
        let hits = scope_implicit_call_lines(body);
        if hits.is_empty() {
            continue;
        }
        if owned_by_contract_crate(path) {
            inside += hits.len();
        } else {
            offenders.push((path.clone(), hits));
        }
    }

    // CONTROL, the one that matters: the query must be able to FIND a
    // scope-implicit call. The backends implement both methods and the trait's
    // dispatcher calls both, so a zero here means the spelling changed and the
    // check below has been passing on an empty set — an empty result reads as
    // "absent", and that is how this class of guard dies quietly.
    assert!(
        inside >= 4,
        "CONTROL: found only {inside} scope-implicit call site(s) inside {CONTRACT_OWNER}. The \
         trait dispatcher and the conformance body alone account for four. Were \
         {SCOPE_IMPLICIT_CALLS:?} renamed?"
    );

    assert!(
        offenders.is_empty(),
        "core#366 d2: these files outside {CONTRACT_OWNER} call the scanner without naming a \
         scope:\n{offenders:#?}\nA bare `scan_orphans(nonce)` can only ever answer \"is there \
         residue under the nonce I am already holding\", which a previous run's leftover can \
         never carry — that is the defect, and `wayland-core backend scan` shipped with it after \
         the sibling command was fixed. Call `orphan::scan_all` / `orphan::scan_one`, or \
         `ExecutionBackend::scan_orphans_in_scope`, all of which take an `OrphanScope` and force \
         the choice to be made and read."
    );
}
