//! The promotion gate, both directions, on the artifacts the product itself writes.
//!
//! `wayland#694` asks for an eval-scored `Staged → Active` transition. The failure mode a
//! gate like this falls into is not "it lets something bad through" — it is that nobody
//! ever sees it pass, or nobody ever sees it fail, and either way it stops being evidence.
//! So every arm here is executed rather than reasoned about:
//!
//! * **Can pass** — [`the_shape_the_drafter_emits_clears_the_gate`] scores the exact body
//!   `wcore_skills::draft::synth_skill_body` produces. If the product's own drafts could
//!   not clear their own gate, the gate would be a ban with extra steps.
//! * **Can fail** — [`a_corrupted_draft_is_refused`] runs the same code over a corrupted
//!   artifact and asserts it lands under the cutoff.
//! * **Cannot no-op** — [`a_missing_skill_md_is_an_error_not_a_pass`] and
//!   [`a_body_with_no_frontmatter_is_an_error_not_a_pass`] cover the two ways of having
//!   nothing to score. Both must be errors. A gate that treats "could not evaluate" as
//!   "nothing to object to" reports green having examined nothing.
//! * **The wiring, not the function** — [`governance_promotes_on_the_gates_own_evidence`]
//!   drives `GovernanceStore` with whatever the gate returned, both ways round, so what is
//!   graded is the path a promotion actually takes rather than the scorer in isolation.

use std::path::Path;

use tempfile::TempDir;
use wcore_eval::evaluate_skill_dir;
use wcore_eval::scorer::LOCKED;
use wcore_skills::govern::GovernanceStore;

/// Exactly the body `wcore_skills::draft::synth_skill_body` renders, for a candidate whose
/// observed sequence was `Bash → Read`. Copied as a literal rather than generated: the
/// point is to score the bytes the drafter writes, and a helper that reproduced them would
/// be free to drift alongside it.
const DRAFTED: &str = "---\n\
name: auto-3f2a9c\n\
description: Run the repository test suite and read the failing output\n\
status: staged\n\
allowed-tools: Bash, Read\n\
---\n\
\n\
# auto-3f2a9c\n\
\n\
Run the repository test suite and read the failing output\n\
\n\
Observed tool sequence: Bash → Read.\n\
Observed input shape: [[\"command\"], [\"file_path\"]].\n\
Observed repeats: 4.\n";

/// The same artifact after the corruptions the W10A corpus enumerates: the declared name
/// no longer matches where it lives, the description is gone, the body reaches for tools it
/// never declared, and the model pin is off the allowlist.
const CORRUPTED: &str = "---\n\
name: something-else-entirely\n\
model: gpt-4o-mini\n\
---\n\
\n\
Use Bash and Write and Edit and Spawn to do whatever seems useful at the time.\n";

fn install(dir: &Path, name: &str, body: &str) -> std::path::PathBuf {
    let d = dir.join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("SKILL.md"), body).unwrap();
    d
}

// ---------------------------------------------------------------------------
// Can it pass
// ---------------------------------------------------------------------------

#[test]
fn the_shape_the_drafter_emits_clears_the_gate() {
    let tmp = TempDir::new().unwrap();
    let dir = install(tmp.path(), "auto-3f2a9c", DRAFTED);

    let gate = evaluate_skill_dir(&dir).expect("a well-formed draft must be scorable");

    assert!(
        gate.clears(),
        "the product's own draft shape scored {:.4} against a {:.4} threshold. A gate no \
         real artifact can clear is a ban, and it would be indistinguishable from one that \
         is simply broken. Breakdown: {:?}",
        gate.evidence.score,
        gate.evidence.threshold,
        gate.outcome.dimensions
    );
    assert_eq!(gate.evidence.verdict, "good");
    assert_eq!(gate.evidence.threshold, LOCKED.acceptance_cutoff());
    assert!(
        gate.evidence.evaluator.contains("DefaultScorer"),
        "the grant has to name whose judgement it records: {}",
        gate.evidence.evaluator
    );
}

// ---------------------------------------------------------------------------
// Can it fail
// ---------------------------------------------------------------------------

#[test]
fn a_corrupted_draft_is_refused() {
    let tmp = TempDir::new().unwrap();
    let dir = install(tmp.path(), "auto-corrupt", CORRUPTED);

    let gate = evaluate_skill_dir(&dir).expect("a corrupted artifact is still scorable");

    assert!(
        !gate.clears(),
        "a draft with a mismatched name, no description, undeclared tool use and an \
         off-allowlist model pin scored {:.4}, over the {:.4} threshold. Breakdown: {:?}",
        gate.evidence.score,
        gate.evidence.threshold,
        gate.outcome.dimensions
    );
    assert_eq!(gate.evidence.verdict, "bad");
}

/// Structural check 7 — declared name against the directory the loader will use — is live
/// here and is NOT live in the loader's own path, where `parse_skill_fields` is handed the
/// directory name for both halves and the check cannot fail. This asserts the difference,
/// because a check that cannot fail is the thing this whole issue is about.
#[test]
fn the_declared_name_is_checked_against_the_directory() {
    let tmp = TempDir::new().unwrap();
    let matching = install(tmp.path(), "auto-3f2a9c", DRAFTED);
    let misfiled = install(tmp.path(), "auto-somewhere-else", DRAFTED);

    let a = evaluate_skill_dir(&matching).unwrap();
    let b = evaluate_skill_dir(&misfiled).unwrap();

    assert!(
        b.evidence.score < a.evidence.score,
        "identical bytes scored the same ({:.4}) in a matching and a mismatched directory, \
         so the name/location check is inert",
        a.evidence.score
    );
}

// ---------------------------------------------------------------------------
// It cannot no-op
// ---------------------------------------------------------------------------

#[test]
fn a_missing_skill_md_is_an_error_not_a_pass() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("auto-empty");
    std::fs::create_dir_all(&dir).unwrap();

    let err = evaluate_skill_dir(&dir)
        .expect_err("an artifact with no SKILL.md must not be evaluated as acceptable");
    assert!(
        err.to_string().contains("cannot evaluate the artifact"),
        "the error must say the evaluation did not happen: {err}"
    );
}

#[test]
fn a_body_with_no_frontmatter_is_an_error_not_a_pass() {
    let tmp = TempDir::new().unwrap();

    // A body long enough that the size penalty is negligible and generic enough that the
    // structural checks it can still reach would carry it over the cutoff. This is the
    // artifact a permissive gate would wave through.
    let body = format!(
        "# Helper\n\n{}\n",
        "Run the repository test suite. ".repeat(4)
    );
    let dir = install(tmp.path(), "auto-bare", &body);

    let err = evaluate_skill_dir(&dir).expect_err(
        "a body with no frontmatter declares no name, description or when-to-use, and \
         scoring it would grade a missing header as a terse skill",
    );
    assert!(
        err.to_string().contains("no YAML frontmatter"),
        "the error must name the reason: {err}"
    );

    // Known-positive for this instrument: the SAME body WITH a frontmatter block IS
    // scorable. Without this, the error above could equally be caused by the path, the
    // permissions, or the file simply not being there.
    let with_fm = format!("---\nname: auto-fm\n---\n\n{body}");
    let ok = install(tmp.path(), "auto-fm", &with_fm);
    evaluate_skill_dir(&ok).expect("the frontmatter is the only difference and it must parse");
}

// ---------------------------------------------------------------------------
// The wiring
// ---------------------------------------------------------------------------

#[test]
fn governance_promotes_on_the_gates_own_evidence() {
    let tmp = TempDir::new().unwrap();
    let store = GovernanceStore::new(tmp.path().join("governance"));

    let good = install(tmp.path(), "auto-3f2a9c", DRAFTED);
    let bad = install(tmp.path(), "auto-corrupt", CORRUPTED);

    // Pass arm: the gate's evidence, unmodified, is accepted and lands in the grant.
    let g = evaluate_skill_dir(&good).unwrap();
    let grant = store
        .promote_existing(&good, None, "test", &g.evidence)
        .expect("a draft that clears the gate must be promotable");
    let recorded = grant
        .evidence
        .as_ref()
        .expect("the grant must record what it was issued against");
    assert_eq!(recorded.score, g.evidence.score);
    assert_eq!(recorded.threshold, LOCKED.acceptance_cutoff());
    assert_eq!(recorded.evaluator, g.evidence.evaluator);

    // Fail arm: same call, same store, evidence from the corrupted artifact.
    let b = evaluate_skill_dir(&bad).unwrap();
    let err = store
        .promote_existing(&bad, None, "test", &b.evidence)
        .expect_err("a draft that fails the gate must not be promotable");
    assert!(
        err.to_string().contains("promotion threshold"),
        "the refusal must name the gate: {err}"
    );

    assert_eq!(
        store.promotions().unwrap().len(),
        1,
        "exactly one grant: the passing artifact's. A second would mean the refusal still \
         wrote one; zero would mean the pass arm never worked and the refusal proved nothing"
    );
}
