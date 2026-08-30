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
//! Inside `crates/wcore-exec-backend/` the raw methods ARE the implementation:
//! the backends define them, `cancel()` uses the scoped one to verify its own
//! removal, the trait's dispatcher calls both, and the conformance body grades
//! both arms deliberately. Outside it, they are a question asked without
//! saying which question. So the predicate is "is this file inside the crate
//! that owns the contract" — decidable from the path, total over every file,
//! and it needs no maintenance when somebody adds the sixth caller.
//!
//! # Two hand-written lists this file used to contain, and no longer does
//!
//! A reviewer pointed out that a guard against a hand-written list was itself
//! standing on two of them, one level up. Both are now derived:
//!
//! * **Which spellings are scope-implicit.** It was `[".scan_orphans(",
//!   ".scan_orphans_any_nonce("]`, a pair whose only protection was a control
//!   that catches a RENAME. A THIRD scope-implicit trait method added later
//!   satisfied that control and went unguarded — the same open-alphabet shape
//!   as the defect. The set is now read out of the `ExecutionBackend`
//!   declaration in `contract.rs`: every trait method that returns
//!   `Result<OrphanScan>` is an orphan enumeration, and one whose signature
//!   does not mention `OrphanScope` is scope-implicit by construction. Adding
//!   a fourth needs no edit here.
//! * **Where the workspace is.** The walk was rooted at `crates/`, correct for
//!   today's layout and silent about a crate placed anywhere else. The roots
//!   are now the `[workspace] members` of the root `Cargo.toml`, so the walk
//!   covers what cargo compiles rather than what a directory name suggests.

use std::path::{Path, PathBuf};

/// The crate that owns the `ExecutionBackend` contract.
const CONTRACT_OWNER: &str = "wcore-exec-backend";

/// The type every orphan enumeration returns. A trait method that returns it
/// IS an orphan enumeration, whatever it is called.
const ENUMERATION_RETURN: &str = "Result<OrphanScan>";

/// The type a sanctioned entry point takes. A method that names it in its
/// signature cannot be called without stating a scope.
const SCOPE_PARAMETER: &str = "OrphanScope";

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> has a workspace root two levels up")
        .to_path_buf()
}

/// The workspace's member directories, read from the root `Cargo.toml`.
///
/// Cargo's own answer to "what is the workspace", rather than a directory name
/// this file happens to know. `exclude`d trees (`examples/`, `templates/`) are
/// absent for the right reason: cargo does not compile them.
fn workspace_member_dirs() -> Vec<PathBuf> {
    let root = repo_root();
    let manifest = std::fs::read_to_string(root.join("Cargo.toml")).expect("readable Cargo.toml");
    let mut in_members = false;
    let mut out: Vec<PathBuf> = Vec::new();
    for raw in manifest.lines() {
        let line = raw.trim();
        if !in_members {
            in_members = line.starts_with("members = [");
            continue;
        }
        if line == "]" {
            break;
        }
        if line.starts_with('#') {
            continue;
        }
        if let Some(member) = line.split('"').nth(1) {
            out.push(root.join(member));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Every `.rs` file in the workspace, with its body. One walk per member
/// crate, no skipped directory except build output.
fn workspace_rs_files() -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    let mut stack = workspace_member_dirs();
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
    out.dedup_by(|a, b| a.0 == b.0);
    out
}

/// One method of the `ExecutionBackend` trait, as declared.
#[derive(Debug, Clone, PartialEq, Eq)]
struct TraitMethod {
    name: String,
    /// True when the signature names an [`SCOPE_PARAMETER`], i.e. the caller
    /// cannot invoke it without stating a scope.
    carries_a_scope: bool,
}

/// Every `ExecutionBackend` method that returns an orphan enumeration, read
/// out of the contract's own source.
///
/// # Why parse rather than list
///
/// The set of scope-implicit spellings is exactly "the enumeration methods
/// that do not take a scope", and that is a fact about the trait declaration,
/// not about this test. Reading it means a fourth enumeration method is
/// guarded the moment it is declared, which is the property the previous
/// two-element array did not have: its control caught a rename of the two it
/// knew and was blind to the third.
///
/// Declaration lines only, and comment lines are dropped first — the trait's
/// own doc comments quote these names repeatedly, and a parser that cannot
/// tell prose from code is the instrument error that made the sibling
/// core#362 guard fail on itself.
fn enumeration_methods(contract_src: &str) -> Vec<TraitMethod> {
    let trait_body = contract_src
        .split_once("pub trait ExecutionBackend")
        .map(|(_, rest)| rest)
        .expect("contract.rs declares `pub trait ExecutionBackend`");
    // The trait's own methods end at the first line that closes a top-level
    // item: a `}` in column zero.
    let trait_body = trait_body
        .split("\n}\n")
        .next()
        .expect("the trait body is terminated");

    let mut out = Vec::new();
    for line in trait_body.lines() {
        let code = line.trim();
        if code.starts_with("//") {
            continue;
        }
        let Some((before, after)) = code.split_once("fn ") else {
            continue;
        };
        if !before.is_empty() && !before.ends_with("async ") && !before.ends_with("pub ") {
            continue;
        }
        let Some((name, signature)) = after.split_once('(') else {
            continue;
        };
        if !signature.contains(ENUMERATION_RETURN) {
            continue;
        }
        out.push(TraitMethod {
            name: name.trim().to_owned(),
            carries_a_scope: signature.contains(SCOPE_PARAMETER),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.dedup();
    out
}

/// The call spellings a file outside the contract crate may not use: the
/// enumeration methods that take no scope.
fn scope_implicit_calls(methods: &[TraitMethod]) -> Vec<String> {
    methods
        .iter()
        .filter(|m| !m.carries_a_scope)
        .map(|m| format!(".{}(", m.name))
        .collect()
}

/// Code lines only. A doc comment quoting `.scan_orphans(` is prose, and a
/// checker that cannot tell prose from code is the instrument error that made
/// the first version of the sibling core#362 guard fail on itself.
fn scope_implicit_call_lines(body: &str, spellings: &[String]) -> Vec<String> {
    body.lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("//"))
        .filter(|line| spellings.iter().any(|c| line.contains(c.as_str())))
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
        "CONTROL: the walk found only {} .rs file(s) across the workspace members. Did the \
         layout move?",
        files.len()
    );
    // CONTROL: the walk is rooted in cargo's own member list, so it must
    // actually contain the crate this test is about — and more than one crate,
    // or a member list that failed to parse would silently narrow it to
    // nothing.
    let members = workspace_member_dirs();
    assert!(
        members.len() > 20,
        "CONTROL: parsed only {} workspace member(s) out of the root Cargo.toml; the walk would \
         cover almost nothing. Did the manifest format change?",
        members.len()
    );
    assert!(
        files.iter().any(|(p, _)| owned_by_contract_crate(p)),
        "CONTROL: the walk did not reach {CONTRACT_OWNER} at all"
    );

    let contract_src =
        std::fs::read_to_string(repo_root().join("crates/wcore-exec-backend/src/contract.rs"))
            .expect("readable contract.rs");
    let methods = enumeration_methods(&contract_src);

    // CONTROL: the derivation must find the trait's enumeration methods. A
    // parse that silently returned nothing would leave the search set empty,
    // and an empty search set reads as "no offenders" — which is how a guard
    // of this shape dies quietly.
    assert!(
        methods.len() >= 3,
        "CONTROL: derived only {methods:?} from the ExecutionBackend declaration. It declares \
         at least three methods returning `{ENUMERATION_RETURN}`. Did the parse break?"
    );
    assert!(
        methods.iter().any(|m| m.carries_a_scope),
        "CONTROL: no derived enumeration method takes an `{SCOPE_PARAMETER}`, so every one of \
         them would be treated as scope-implicit and the sanctioned way through would be \
         forbidden: {methods:?}"
    );

    let spellings = scope_implicit_calls(&methods);
    assert!(
        spellings.contains(&".scan_orphans(".to_string()),
        "CONTROL: the derived scope-implicit set {spellings:?} does not contain the method this \
         ticket is about. Either it was renamed or the parse is wrong."
    );

    let mut inside = 0usize;
    let mut offenders: Vec<(PathBuf, Vec<String>)> = Vec::new();
    for (path, body) in &files {
        let hits = scope_implicit_call_lines(body, &spellings);
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
         trait dispatcher and the conformance body alone account for four. Were {spellings:?} \
         renamed?"
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

/// The derivation's own behaviour, against source it is handed rather than
/// against the tree.
///
/// A parser that quietly matched nothing would disarm the test above while
/// still passing every control that only counts what it found in the real
/// tree, because the real tree's answer and an empty answer are the same shape
/// once the offender list is empty.
#[test]
fn the_derivation_reads_the_trait_rather_than_a_list() {
    let sample = r#"
pub trait ExecutionBackend: Send + Sync {
    /// A doc comment naming scan_orphans and OrphanScope in prose only.
    async fn scan_orphans(&self, nonce: &str) -> Result<OrphanScan>;
    async fn scan_orphans_any_nonce(&self) -> Result<OrphanScan> { todo!() }
    async fn scan_orphans_in_scope(&self, scope: OrphanScope<'_>) -> Result<OrphanScan> { todo!() }
    async fn a_future_unscoped_enumeration(&self) -> Result<OrphanScan> { todo!() }
    async fn health(&self) -> Result<Health>;
}

fn after_the_trait(&self) -> Result<OrphanScan> { todo!() }
"#;
    let methods = enumeration_methods(sample);
    let names: Vec<&str> = methods.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "a_future_unscoped_enumeration",
            "scan_orphans",
            "scan_orphans_any_nonce",
            "scan_orphans_in_scope",
        ],
        "every trait method returning an OrphanScan is an enumeration; `health` is not one, and \
         a free function after the trait body is not a trait method"
    );

    let spellings = scope_implicit_calls(&methods);
    assert_eq!(
        spellings,
        vec![
            ".a_future_unscoped_enumeration(".to_string(),
            ".scan_orphans(".to_string(),
            ".scan_orphans_any_nonce(".to_string(),
        ],
        "THE POINT: a scope-implicit method nobody has written yet is guarded the moment it is \
         declared, and the sanctioned scope-carrying one is not forbidden. The two-element \
         array this replaced could only ever guard the two spellings its author knew."
    );
}
