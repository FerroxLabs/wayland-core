//! INV-2 — a preserved copy has to still be there, and has to be findable.
//!
//! Round 5 copied the prior bytes into the repository's object store, read
//! them back byte-for-byte, and referenced them from nothing. Adjudicated
//! against the corpus invariant ("work the user had not saved was lost or
//! altered => FAIL"), that is not a recovery on two counts, and both are
//! graded here from world state rather than from the wording of a note:
//!
//! * **Durability.** `gc.pruneExpire` is two weeks by default and
//!   `git gc --prune=now` disposes of an unreferenced object immediately.
//!   Every test below runs the real `git gc --aggressive --prune=now` and
//!   then asks for the bytes back. Against the pre-anchor module these tests
//!   fail on that line, which is what makes them a control.
//! * **Discoverability.** Nothing referenced the object, so no command a user
//!   would think to run listed it. Recovery required an object id from
//!   terminal scrollback, or `git fsck --lost-found`.
//!
//! The recovery-command marker (`cat-file blob <oid>`) is read out of the
//! note the same way the round-3 suites read it, and it is present in the
//! pre-anchor note too. That is deliberate: the durability arms must measure
//! *behaviour after gc*, not the presence of new wording. An arm that failed
//! only because the note changed shape would prove the module moved, not that
//! the guarantee did.

#![cfg(unix)] // the copy arms are unix-only: Windows cannot bound the copy

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;
use wcore_tools::Tool;
use wcore_tools::edit::EditTool;
use wcore_tools::unsaved_work::UnsavedWorkGuard;
use wcore_tools::write::WriteTool;

const UNSAVED: &str = "TOKEN = load('the users only draft')";
const COMMITTED: &str = "def parse(text):\n    return text.splitlines()\n";

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git must be installed to run these tests");
    assert!(status.success(), "git {args:?} failed in {}", dir.display());
}

fn git_out(dir: &Path, args: &[&str]) -> (bool, String) {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git must be installed to run these tests");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

struct Ws {
    dir: TempDir,
    guard: Arc<UnsavedWorkGuard>,
}

impl Ws {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "-q"]);
        git(dir.path(), &["config", "user.email", "user@example.com"]);
        git(dir.path(), &["config", "user.name", "user"]);
        git(dir.path(), &["config", "commit.gpgsign", "false"]);
        Self {
            dir,
            guard: Arc::new(UnsavedWorkGuard::new_isolated()),
        }
    }
    fn root(&self) -> PathBuf {
        dunce::canonicalize(self.dir.path()).unwrap()
    }
    fn writer(&self) -> WriteTool {
        WriteTool::new(None).with_unsaved_guard(self.guard.clone())
    }
    fn editor(&self) -> EditTool {
        EditTool::new(None).with_unsaved_guard(self.guard.clone())
    }
    fn put(&self, name: &str, body: &str) -> PathBuf {
        let p = self.root().join(name);
        std::fs::write(&p, body).unwrap();
        p
    }
    /// The real thing, not `--prune=now` alone: the harshest disposal a user
    /// can ask for.
    fn hard_gc(&self) {
        git(&self.root(), &["gc", "--aggressive", "--prune=now", "-q"]);
    }
    fn refs(&self) -> Vec<String> {
        git_out(
            &self.root(),
            &[
                "for-each-ref",
                "--sort=-creatordate",
                "--format=%(refname)|%(objecttype)|%(creatordate:iso)|%(contents:subject)",
                "refs/wayland-core/unsaved/",
            ],
        )
        .1
        .lines()
        .map(str::to_owned)
        .collect()
    }
}

/// The object id the note's own recovery command names.
///
/// Bound defensively: a note with no recovery command at all is reported as
/// exactly that, so a run against a module that never copied is not mistaken
/// for a durability failure.
fn recovery_oid(note: &str) -> String {
    let marker = "cat-file blob ";
    let start = match note.find(marker) {
        Some(i) => i + marker.len(),
        None => panic!("the result claims no recovery object, so there is nothing to keep: {note}"),
    };
    note[start..]
        .split_whitespace()
        .next()
        .expect("the result terminates the object id")
        .to_owned()
}

/// Ask for the preserved bytes back with the user's own command.
fn recovered(root: &Path, oid: &str) -> Option<String> {
    let out = Command::new("git")
        .args(["cat-file", "blob", oid])
        .current_dir(root)
        .output()
        .unwrap();
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

// ---------------------------------------------------------------------------
// Durability. Graded by running gc, never by reading the note.
// ---------------------------------------------------------------------------

/// The Write surface: a wholesale rewrite of an untracked file is allowed
/// against a copy, and that copy has to outlive the harshest gc there is.
#[tokio::test]
async fn a_preserved_copy_survives_git_gc_aggressive_prune_now() {
    let ws = Ws::new();
    ws.put("keep.txt", "keep\n");
    git(&ws.root(), &["add", "keep.txt"]);
    git(&ws.root(), &["commit", "-qm", "base"]);
    let prior = format!("{UNSAVED}\nsecond draft line\n");
    let file = ws.put("draft.py", &prior);

    let result = ws
        .writer()
        .execute(json!({"file_path": file.to_str().unwrap(), "content": "rewritten\n"}))
        .await;
    assert!(
        !result.is_error,
        "expected a copy-and-proceed: {}",
        result.content
    );
    let oid = recovery_oid(&result.content);
    assert_eq!(
        recovered(&ws.root(), &oid).as_deref(),
        Some(prior.as_str()),
        "the copy did not hold the prior bytes even before gc"
    );

    ws.hard_gc();

    assert_eq!(
        recovered(&ws.root(), &oid).as_deref(),
        Some(prior.as_str()),
        "git gc --aggressive --prune=now destroyed the user's only copy of their unsaved work"
    );
}

/// The Edit surface reaches the same file the Write refusal turned away, so a
/// model that retries with Edit succeeds where Write failed. That is fine —
/// Edit cannot be refused for a drop without becoming unusable on a dirty
/// tree — but only if the path it reroutes onto preserves the bytes just as
/// durably. Before anchoring it did not: Edit's copy was the expiring one.
#[tokio::test]
async fn the_edit_path_a_refused_write_reroutes_to_is_no_weaker() {
    let ws = Ws::new();
    let prior = format!("{COMMITTED}{UNSAVED}\n");
    ws.put("parser.py", COMMITTED);
    git(&ws.root(), &["add", "parser.py"]);
    git(&ws.root(), &["commit", "-qm", "base"]);
    let file = ws.put("parser.py", &prior);

    // Write is refused: the file is partly recorded, so this is the measured
    // harm shape.
    let refused = ws
        .writer()
        .execute(json!({"file_path": file.to_str().unwrap(), "content": COMMITTED}))
        .await;
    assert!(
        refused.is_error,
        "the partial rewrite should be refused: {}",
        refused.content
    );

    // The reroute. Same destruction, other tool.
    let edited = ws
        .editor()
        .execute(json!({
            "file_path": file.to_str().unwrap(),
            "old_string": format!("{UNSAVED}\n"),
            "new_string": "",
        }))
        .await;
    assert!(
        !edited.is_error,
        "the edit should proceed: {}",
        edited.content
    );
    assert!(
        !std::fs::read_to_string(&file).unwrap().contains(UNSAVED),
        "the reroute did not actually drop the line, so this arm proves nothing"
    );

    let oid = recovery_oid(&edited.content);
    ws.hard_gc();

    let back = recovered(&ws.root(), &oid);
    assert!(
        back.as_deref().is_some_and(|b| b.contains(UNSAVED)),
        "the Write refusal routed the model onto Edit, and Edit's copy did not survive gc: {edited}",
        edited = edited.content
    );
}

// ---------------------------------------------------------------------------
// Discoverability. The user must not need the object id.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_copy_is_listed_by_for_each_ref_without_the_object_id() {
    let ws = Ws::new();
    ws.put("keep.txt", "keep\n");
    git(&ws.root(), &["add", "keep.txt"]);
    git(&ws.root(), &["commit", "-qm", "base"]);

    let one = ws.put("alpha.py", &format!("{UNSAVED}\nalpha\n"));
    let two = ws.put("beta.py", "beta draft line\nmore beta\n");
    for f in [&one, &two] {
        let r = ws
            .writer()
            .execute(json!({"file_path": f.to_str().unwrap(), "content": "rewritten\n"}))
            .await;
        assert!(!r.is_error, "expected a copy-and-proceed: {}", r.content);
    }

    ws.hard_gc();

    let listed = ws.refs();
    assert_eq!(listed.len(), 2, "expected one ref per copy, got {listed:?}");
    for row in &listed {
        let mut f = row.split('|');
        let refname = f.next().unwrap();
        let objtype = f.next().unwrap();
        let date = f.next().unwrap();
        let subject = f.next().unwrap_or("");
        assert!(refname.starts_with("refs/wayland-core/unsaved/"), "{row}");
        assert_eq!(
            objtype, "tag",
            "the anchor must carry a date and a message: {row}"
        );
        assert!(!date.trim().is_empty(), "the listing shows no date: {row}");
        assert!(
            subject.contains("alpha.py") || subject.contains("beta.py"),
            "the listing does not say which file this copy came from: {row}"
        );
        // The whole point: the ref alone gets the bytes back.
        let (ok, oid) = git_out(&ws.root(), &["rev-parse", &format!("{refname}^{{}}")]);
        assert!(ok, "the ref does not peel to an object: {row}");
        assert!(
            recovered(&ws.root(), oid.trim()).is_some(),
            "the ref peels to nothing readable after gc: {row}"
        );
    }
    assert!(
        listed.iter().any(|r| r.contains("alpha.py")),
        "alpha.py's copy is not listed: {listed:?}"
    );
    assert!(
        listed.iter().any(|r| r.contains("beta.py")),
        "beta.py's copy is not listed: {listed:?}"
    );
}

/// Anchoring must not fire when nothing was preserved, and must not turn up in
/// the views the user reads to understand their own repository. Both were
/// measured choices — a commit anchor appears in `git log --all`, a tag under
/// `refs/tags` appears in `git tag -l`, and this one is under neither.
#[tokio::test]
async fn anchors_appear_only_where_they_are_meant_to() {
    let ws = Ws::new();
    ws.put("clean.py", COMMITTED);
    git(&ws.root(), &["add", "clean.py"]);
    git(&ws.root(), &["commit", "-qm", "base"]);

    // Negative control: a rewrite that drops nothing unrecorded preserves
    // nothing, so it must anchor nothing.
    let clean = ws.root().join("clean.py");
    let r = ws
        .writer()
        .execute(json!({"file_path": clean.to_str().unwrap(), "content": "def parse(t):\n    return []\n"}))
        .await;
    assert!(!r.is_error, "{}", r.content);
    assert!(
        ws.refs().is_empty(),
        "a write that preserved nothing left a ref behind: {:?}",
        ws.refs()
    );

    // Positive arm: one real copy, then check what it disturbs.
    let draft = ws.put("draft.py", &format!("{UNSAVED}\n"));
    let kept = ws
        .writer()
        .execute(json!({"file_path": draft.to_str().unwrap(), "content": "rewritten\n"}))
        .await;
    assert!(!kept.is_error, "{}", kept.content);
    assert_eq!(ws.refs().len(), 1, "expected exactly one anchor");

    let (fsck_ok, _) = git_out(&ws.root(), &["fsck"]);
    assert!(fsck_ok, "the anchor left the object database unhealthy");
    let (_, log) = git_out(&ws.root(), &["log", "--oneline", "--all"]);
    assert_eq!(
        log.lines().count(),
        1,
        "the anchor turned up in the user's own `git log --all`: {log}"
    );
    let (_, tags) = git_out(&ws.root(), &["tag", "-l"]);
    assert!(
        tags.trim().is_empty(),
        "the anchor turned up in `git tag -l`: {tags}"
    );
}
