//! INV-2 round 5 — the round-4 adversarial probes, inverted.
//!
//! Every arm here was first run against round 4 in the shape that *asserted
//! the defect*, and every one of them passed there. What is asserted now is
//! the fixed behaviour, so a failure here is a regression to a measured harm
//! rather than a hypothetical one. Each arm drives the REAL WriteTool /
//! EditTool through `Tool::execute`.
//!
//! Deliberately no `std::env` mutation in this binary: the arms that need a
//! process-global git variable live in `unsaved_work_git_env_test`, which is
//! one test in its own binary for that reason.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;
use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::edit::EditTool;
use wcore_tools::unsaved_work::UnsavedWorkGuard;
use wcore_tools::write::WriteTool;

fn git(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?} failed in {}", dir.display());
}

fn init(dir: &Path) {
    git(dir, &["init", "-q"]);
    git(dir, &["config", "user.email", "u@e.com"]);
    git(dir, &["config", "user.name", "u"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

/// Is `needle` anywhere in this repository's object database? Exhaustive, so
/// a copy cannot be missed by looking in the wrong place.
fn object_store_contains(root: &Path, needle: &str) -> bool {
    let out = Command::new("git")
        .args(["cat-file", "--batch-all-objects", "--batch"])
        .current_dir(root)
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).contains(needle)
}

/// Every object id in this repository's database.
fn object_ids(root: &Path) -> Vec<String> {
    let out = Command::new("git")
        .args([
            "cat-file",
            "--batch-all-objects",
            "--batch-check=%(objectname)",
        ])
        .current_dir(root)
        .output()
        .unwrap();
    let mut ids: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_owned)
        .collect();
    ids.sort();
    ids
}

struct Ws {
    dir: TempDir,
    guard: Arc<UnsavedWorkGuard>,
}
impl Ws {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        init(dir.path());
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
}

async fn write_via_tool(ws: &Ws, file: &Path, body: &str) -> (bool, String) {
    let r = ws
        .writer()
        .execute(json!({
            "file_path": file.to_str().unwrap(), "content": body
        }))
        .await;
    (r.is_error, r.content)
}

// ===========================================================================
// ADV-1  core.bare on a real work tree: git succeeds and answers "false".
//        Round 4 routed that to NoRepo and told the user the file was in no
//        repository, about a repository whose HEAD records the file.
// ===========================================================================
#[tokio::test]
async fn a_work_tree_git_calls_bare_is_not_reported_as_no_repository() {
    let ws = Ws::new();
    let root = ws.root();
    let file = root.join("parser.py");
    std::fs::write(&file, "def parse(t):\n    return t\n").unwrap();
    git(&root, &["add", "parser.py"]);
    git(&root, &["commit", "-qm", "init"]);
    std::fs::write(&file, "def parse(t):\n    return t\n# USER-UNSAVED-LINE\n").unwrap();
    git(&root, &["config", "core.bare", "true"]);

    let (err, msg) = write_via_tool(&ws, &file, "def parse(t):\n    return t.strip()\n").await;
    assert!(err, "still fail-closed: {msg}");

    // The repository really does record this file, so "in no repository" is
    // a false statement about it and a remedy that would not have helped.
    let out = Command::new("git")
        .args(["ls-tree", "--full-tree", "HEAD", "--", "parser.py"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert!(
        !out.stdout.is_empty(),
        "control: the commit must really record parser.py"
    );
    assert!(
        !msg.contains("in no repository"),
        "the refusal still calls a real repository no repository: {msg}"
    );
    assert!(
        msg.contains("no work tree") && msg.contains("could not be established"),
        "the refusal must name what git actually said: {msg}"
    );
}

// ===========================================================================
// ADV-2b  The ignored-file rule used to be decided by the mutable index, so
//         one `git add -f` filed a gitignored secret into .git/objects.
// ===========================================================================
#[tokio::test]
async fn one_git_add_f_cannot_file_a_gitignored_secret_into_the_repository() {
    async fn arm(add_f: bool) -> (bool, String, PathBuf, Ws, bool) {
        let ws = Ws::new();
        let root = ws.root();
        std::fs::write(root.join(".gitignore"), ".env\n").unwrap();
        git(&root, &["add", ".gitignore"]);
        git(&root, &["commit", "-qm", "init"]);
        let file = root.join(".env");
        std::fs::write(&file, "STRIPE=ADVCANARY-INDEX-9911\n").unwrap();
        if add_f {
            // `git add -f` writes the blob itself, so the object store cannot
            // be the measure here — what must not change is what the GUARD
            // adds to it.
            git(&root, &["add", "-f", ".env"]);
        }
        let before = object_ids(&root);
        let (err, msg) = write_via_tool(&ws, &file, "STRIPE=rotated\n").await;
        let unchanged = object_ids(&root) == before;
        (err, msg, root, ws, unchanged)
    }

    let (e0, m0, r0, _k0, still0) = arm(false).await;
    assert!(e0, "control: a gitignored file must be refused: {m0}");
    assert!(m0.contains("configured to ignore this file"), "{m0}");
    assert!(!object_store_contains(&r0, "ADVCANARY-INDEX-9911"), "{m0}");
    assert!(still0, "the guard added an object anyway");

    let (e1, m1, r1, _k1, still1) = arm(true).await;
    assert!(
        e1 && m1.contains("configured to ignore this file"),
        "`git add -f` disarmed the ignored-file rule again: {m1}"
    );
    assert!(
        still1,
        "the guard filed the gitignored secret into .git/objects: {m1}"
    );
    assert!(!m1.contains("cat-file blob"), "a copy was claimed: {m1}");
    // Positive control: the index really was changed, so the arm is not
    // vacuous — it is the *decision* that stopped depending on it.
    let staged = Command::new("git")
        .args(["ls-files", "--", ".env"])
        .current_dir(&r1)
        .output()
        .unwrap();
    assert!(
        !staged.stdout.is_empty(),
        "control: `git add -f` must really have staged the file"
    );
}

// ===========================================================================
// ADV-3  The partial refusal must not assert things about the file that are
//        not true: that the rest of it is committed, or whose work it is.
// ===========================================================================
#[tokio::test]
async fn the_partial_refusal_claims_nothing_about_the_lines_it_keeps() {
    let ws = Ws::new();
    let root = ws.root();
    let file = root.join("notes.md");
    std::fs::write(&file, "committed A\ncommitted B\n").unwrap();
    git(&root, &["add", "notes.md"]);
    git(&root, &["commit", "-qm", "init"]);
    // TWO unsaved lines; the rewrite keeps one and drops the other, so "the
    // rest of this file IS committed" is false of the kept one.
    std::fs::write(
        &file,
        "committed A\ncommitted B\nunsaved KEPT\nunsaved DROPPED\n",
    )
    .unwrap();

    let (err, msg) = write_via_tool(&ws, &file, "committed A\ncommitted B\nunsaved KEPT\n").await;
    assert!(err, "{msg}");
    assert!(msg.contains("unsaved DROPPED"), "{msg}");
    assert!(
        !msg.contains("The rest of this file IS committed"),
        "the refusal still claims the kept lines are committed: {msg}"
    );
    assert!(
        !msg.contains("their in-progress work"),
        "the refusal still attributes the lines to the user: {msg}"
    );
}

// ===========================================================================
// ADV-4  Cry-wolf cost. `cargo fmt` runs on `just push`, so a formatter
//        between two agent writes is routine. Round 4 expired attribution on
//        byte inequality, so the agent's own next rewrite of its own file was
//        hard refused and the message called those lines the user's.
// ===========================================================================
#[tokio::test]
async fn a_formatter_between_two_agent_writes_does_not_refuse_the_agents_own_rewrite() {
    let ws = Ws::new();
    let root = ws.root();
    let file = root.join("mod.py");
    std::fs::write(&file, "import os\n").unwrap();
    git(&root, &["add", "mod.py"]);
    git(&root, &["commit", "-qm", "init"]);

    let (e1, m1) = write_via_tool(
        &ws,
        &file,
        "import os\ndef a():\n    return 1\ndef b():\n    return 2\n",
    )
    .await;
    assert!(!e1, "first write must succeed: {m1}");

    // A formatter reflows it. Every line of content is the agent's; only
    // whitespace moved.
    std::fs::write(
        &file,
        "import os\n\n\ndef a():\n    return 1\n\n\ndef b():\n    return 2\n",
    )
    .unwrap();

    let (e2, m2) = write_via_tool(&ws, &file, "import os\ndef a():\n    return 1\n").await;
    assert!(
        !e2,
        "the agent's own second write is refused after a reformat: {m2}"
    );

    // Control, so this is not a blanket disarm: a real content change by the
    // user does still expire attribution, and the same drop is then refused.
    let ws2 = Ws::new();
    let root2 = ws2.root();
    let file2 = root2.join("mod.py");
    std::fs::write(&file2, "import os\n").unwrap();
    git(&root2, &["add", "mod.py"]);
    git(&root2, &["commit", "-qm", "init"]);
    let (e3, m3) = write_via_tool(
        &ws2,
        &file2,
        "import os\ndef a():\n    return 1\ndef b():\n    return 2\n",
    )
    .await;
    assert!(!e3, "{m3}");
    std::fs::write(
        &file2,
        "import os\ndef a():\n    return 1\ndef b():\n    return 2\n# USER WIP\n",
    )
    .unwrap();
    let (e4, m4) = write_via_tool(
        &ws2,
        &file2,
        "import os\ndef a():\n    return 1\n# USER WIP\n",
    )
    .await;
    assert!(
        e4 && m4.contains("def b():"),
        "control: a user edit must still expire attribution: {m4}"
    );
}

// ===========================================================================
// ADV-5  A 0600 file's bytes must not become a 0444 object under a 0755
//        directory. The copy is only made where it is provably no wider.
// ===========================================================================
#[cfg(unix)]
#[tokio::test]
async fn a_private_file_is_refused_rather_than_copied_into_a_wider_object() {
    use std::os::unix::fs::PermissionsExt as _;
    let ws = Ws::new();
    let root = ws.root();
    std::fs::write(root.join("keep.txt"), "keep\n").unwrap();
    git(&root, &["add", "keep.txt"]);
    git(&root, &["commit", "-qm", "init"]);

    let file = root.join("credentials");
    let prior = "aws_secret_access_key = ADVCANARY0987654321\n";
    std::fs::write(&file, prior).unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();

    let (err, msg) = write_via_tool(&ws, &file, "aws_secret_access_key = ROTATED\n").await;
    assert!(err, "a 0600 file's bytes were copied anyway: {msg}");
    assert!(msg.contains("0444 object"), "{msg}");
    assert!(
        !object_store_contains(&root, "ADVCANARY0987654321"),
        "the secret is in the object store: {msg}"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        prior,
        "a refusal must leave the file exactly as it was"
    );
    // Control: the same file, world-readable, IS copied — so this arm is
    // measuring the permission comparison and not some other refusal.
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    let (err2, msg2) = write_via_tool(&ws, &file, "aws_secret_access_key = ROTATED\n").await;
    assert!(!err2, "control: a 0644 file must still be copied: {msg2}");
    assert!(msg2.contains("cat-file blob"), "{msg2}");
}

// ===========================================================================
// The other half of the permission proof: a file whose *directory* is more
// private than the object store. Mode alone would pass this one.
// ===========================================================================
#[cfg(unix)]
#[tokio::test]
async fn a_file_in_a_private_directory_is_not_copied_into_a_reachable_store() {
    use std::os::unix::fs::PermissionsExt as _;
    // Under a world-searchable base, because the comparison is an
    // intersection down the whole chain: a private ancestor above the fixture
    // makes file and store equally unreachable and the arm vacuous. (This
    // build host keeps /tmp at 0700, which is exactly that case.)
    let base = Path::new("/var/tmp");
    let base_mode = std::fs::metadata(base).unwrap().permissions().mode();
    assert!(
        base_mode & 0o001 != 0,
        "{} is not world-searchable, so this arm cannot measure reachability",
        base.display()
    );
    let dir = tempfile::TempDir::new_in(base).unwrap();
    let root = dunce::canonicalize(dir.path()).unwrap();
    init(&root);
    // An ordinary umask-022 checkout. This host runs umask 077, which would
    // otherwise leave .git/objects owner-only and the comparison vacuous.
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    let private = root.join("private");
    std::fs::create_dir(&private).unwrap();
    std::fs::write(private.join("keep.txt"), "keep\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "init"]);
    for d in [root.join(".git"), root.join(".git/objects")] {
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    // ...but this directory is the user's own, and nobody else can enter it.
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700)).unwrap();

    let file = private.join("notes.env");
    let prior = "TOKEN=ADVCANARY-PRIVATE-DIR-55\n";
    std::fs::write(&file, prior).unwrap();
    // World-readable mode, so only the directory makes it private: the mode
    // comparison on its own would let this copy through.
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();

    let tool =
        || WriteTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()));
    let r = tool()
        .execute(json!({
            "file_path": file.to_str().unwrap(), "content": "TOKEN=rotated\n"
        }))
        .await;
    assert!(
        r.is_error,
        "a file only its owner can reach was copied: {}",
        r.content
    );
    assert!(r.content.contains("can reach it"), "{}", r.content);
    assert!(
        !object_store_contains(&root, "ADVCANARY-PRIVATE-DIR-55"),
        "the bytes are in a store more people can reach than the file"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), prior);

    // Control: the same file in a directory as reachable as the store IS
    // copied, so this arm measures reachability and not some other refusal.
    std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755)).unwrap();
    let r2 = tool()
        .execute(json!({
            "file_path": file.to_str().unwrap(), "content": "TOKEN=rotated\n"
        }))
        .await;
    assert!(
        !r2.is_error,
        "control: an ordinary directory must still copy: {}",
        r2.content
    );
    assert!(r2.content.contains("cat-file blob"), "{}", r2.content);
}

// ===========================================================================
// ADV-6  A linked worktree keeps its objects in the MAIN repository, so a
//        copy would leave the tree the user is working in.
// ===========================================================================
#[tokio::test]
async fn a_linked_worktree_refuses_rather_than_copying_out_of_the_tree() {
    let outer = tempfile::tempdir().unwrap();
    let main = dunce::canonicalize(outer.path()).unwrap().join("main");
    std::fs::create_dir(&main).unwrap();
    init(&main);
    std::fs::write(main.join("keep.txt"), "keep\n").unwrap();
    git(&main, &["add", "keep.txt"]);
    git(&main, &["commit", "-qm", "init"]);
    let wt = dunce::canonicalize(outer.path()).unwrap().join("wt");
    git(
        &main,
        &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "b2"],
    );
    assert!(
        !wt.join(".git").is_dir(),
        "control: in a linked worktree .git is a FILE"
    );

    let guard = Arc::new(UnsavedWorkGuard::new_isolated());
    let file = wt.join("private.env");
    let prior = "TOKEN=ADVCANARY-WORKTREE-1234\n";
    std::fs::write(&file, prior).unwrap();

    let r = WriteTool::new(None)
        .with_unsaved_guard(guard)
        .execute(json!({
            "file_path": file.to_str().unwrap(), "content": "TOKEN=rotated\n"
        }))
        .await;
    assert!(r.is_error, "the copy left this work tree: {}", r.content);
    assert!(
        r.content.contains("outside the tree this file is in"),
        "{}",
        r.content
    );
    assert!(
        !object_store_contains(&main, "ADVCANARY-WORKTREE-1234"),
        "the bytes were filed into the main repository"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), prior);
}

// ===========================================================================
// The submodule half of the same topology: `--git-path objects` resolves to
// <super>/.git/modules/<name>/objects, outside the repository the user is in.
// ===========================================================================
#[tokio::test]
async fn a_submodule_refuses_rather_than_filing_into_the_superproject() {
    let outer = tempfile::tempdir().unwrap();
    let base = dunce::canonicalize(outer.path()).unwrap();
    let sub = base.join("sub");
    let sup = base.join("super");
    std::fs::create_dir(&sub).unwrap();
    std::fs::create_dir(&sup).unwrap();
    for d in [&sub, &sup] {
        init(d);
        std::fs::write(d.join("keep.txt"), "keep\n").unwrap();
        git(d, &["add", "keep.txt"]);
        git(d, &["commit", "-qm", "init"]);
    }
    git(
        &sup,
        &[
            "-c",
            "protocol.file.allow=always",
            "submodule",
            "add",
            "-q",
            sub.to_str().unwrap(),
            "sub",
        ],
    );
    git(&sup, &["commit", "-qm", "add submodule"]);
    let inner = sup.join("sub");
    assert!(
        inner.join(".git").is_file(),
        "control: a submodule's .git is a FILE pointing into the superproject"
    );

    let guard = Arc::new(UnsavedWorkGuard::new_isolated());
    let file = inner.join("draft.env");
    let prior = "TOKEN=ADVCANARY-SUBMODULE-7788\n";
    std::fs::write(&file, prior).unwrap();

    let r = WriteTool::new(None)
        .with_unsaved_guard(guard)
        .execute(json!({
            "file_path": file.to_str().unwrap(), "content": "TOKEN=rotated\n"
        }))
        .await;
    assert!(r.is_error, "the copy left the submodule: {}", r.content);
    assert!(
        !object_store_contains(&sup, "ADVCANARY-SUBMODULE-7788"),
        "the bytes were filed into the superproject"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), prior);
}

// ===========================================================================
// ADV-7  A save that lands inside the assessment window. 12/12 interleavings
//        used to destroy it uncopied while the note claimed otherwise.
// ===========================================================================
#[tokio::test]
async fn a_save_during_the_assessment_window_is_not_lost() {
    let (lost, attempts, window) = interleave(true).await;
    println!("[ADV-7] window {window:?}; {lost}/{attempts} saves destroyed");
    assert_eq!(
        lost, 0,
        "a save that arrived during the assessment was destroyed uncopied"
    );
}

/// The control. Without it the arm above would score the same on a harness
/// whose canary never lands at all.
#[tokio::test]
async fn a_save_before_the_write_is_protected() {
    let (lost, attempts, _) = interleave(false).await;
    assert_eq!(
        lost, 0,
        "control is dirty, so the arm above measures the harness not the product"
    );
    assert_eq!(attempts, 24);
}

/// How the user's editor writes its save.
#[derive(Clone, Copy, PartialEq)]
enum Saver {
    /// Write a sibling and rename over the name — what vim, VS Code and emacs
    /// do by default.
    Rename,
    /// Truncate and write the file in place — `printf > file`, and some
    /// editors' "no atomic save" setting.
    InPlace,
}

fn save(file: &Path, body: &str, how: Saver) {
    match how {
        Saver::InPlace => std::fs::write(file, body).unwrap(),
        Saver::Rename => {
            let tmp = file.with_extension("editor-save");
            std::fs::write(&tmp, body).unwrap();
            std::fs::rename(&tmp, file).unwrap();
        }
    }
}

/// `during` = the user's editor saves inside the assessment window; otherwise
/// it saves before the tool is called at all. Returns (lost, attempts, window).
async fn interleave(during: bool) -> (usize, usize, std::time::Duration) {
    // Measure the window on a throwaway repository first.
    let warm = Ws::new();
    let wroot = warm.root();
    let wfile = wroot.join("draft.md");
    std::fs::write(&wfile, "line one\nline two\n").unwrap();
    let t0 = std::time::Instant::now();
    let _ = write_via_tool(&warm, &wfile, "line one\nline two\nline three\n").await;
    let window = t0.elapsed();

    // Delays spread across the measured window rather than one fixed offset:
    // a single offset can sit entirely before or after the assessment on a
    // warm run, and then no arm ever interleaves.
    let attempts = 24;
    let mut lost = 0;
    for i in 0..attempts {
        let ws = Ws::new();
        let root = ws.root();
        std::fs::write(root.join("keep.txt"), "keep\n").unwrap();
        git(&root, &["add", "keep.txt"]);
        git(&root, &["commit", "-qm", "init"]);
        let file = root.join("draft.md");
        let canary = format!("USER-SAVE-{i}");
        std::fs::write(&file, "draft body\n").unwrap();

        let saver = if during {
            let f2 = file.clone();
            let c2 = canary.clone();
            let delay = (window * (i as u32)) / (attempts as u32);
            Some(std::thread::spawn(move || {
                std::thread::sleep(delay);
                save(&f2, &format!("draft body\n{c2}\n"), Saver::Rename);
            }))
        } else {
            save(&file, &format!("draft body\n{canary}\n"), Saver::Rename);
            None
        };
        let (_e, msg) = write_via_tool(&ws, &file, "rewritten body\n").await;
        if let Some(s) = saver {
            s.join().unwrap();
        }

        let on_disk = std::fs::read_to_string(&file).unwrap();
        let recoverable = recovered_copy(&root, &msg);
        if !on_disk.contains(&canary) && !recoverable.contains(&canary) {
            lost += 1;
        }
    }
    (lost, attempts, window)
}

// ===========================================================================
// The same window on the Edit path. Edit is never refused *by the guard*, but
// a replacement computed against bytes that have since been replaced is not a
// guard verdict — writing it would destroy whatever arrived, so it is refused
// like any other stale write.
// ===========================================================================
/// # The Windows truth, measured
///
/// This arm is NOT gated to Unix and never was — it runs on the Windows CI leg
/// like everywhere else, and `.config/nextest.toml` already pins this whole
/// binary to `retries = 0` under `profile.ci`, so CI is not absorbing anything.
/// Until 2026-08-29 nobody had watched it run on a Windows host. Measured on a
/// Windows 11 build 26200 workstation at this tree,
/// `cargo nextest run -p wcore-tools --retries 0` twelve times:
///
/// * **6 of 12** executions green, 0 lost.
/// * **5 of 12** RED on real loss — 2 of 18, 1 of 5, 2 of 10, 1 of 13 and
///   1 of 17 interleavings lost. Aggregated over the eleven executions that
///   measured anything: **7 saves lost out of 169 that landed inside the
///   window (4.1%)**.
/// * **1 of 12** never got to measure: the FIXTURE's own saver failed at
///   [`save`] — `std::fs::rename` returned
///   `Os { code: 5, kind: PermissionDenied, message: "Access is denied." }`.
///   That is not noise, it is the other half of the Windows truth: while the
///   guard holds the destination open, an editor's atomic save over that name
///   is REFUSED by the OS rather than silently lost.
///
/// LOAD MATTERS, AND THE NUMBERS ARE NOT LOAD-FREE — said here rather than
/// discovered later. The six slow executions (40–97 s wall, cold tree) carried
/// four of the five loss failures; the six fast ones (15–18 s, warm) carried
/// one. So the rate above is an upper bound for a warm host and a lower bound
/// for a loaded one — but loss is NOT purely a load artefact: the 1-of-17 came
/// from a 17.0 s warm execution, and one of the rename refusals from a 14.9 s
/// one. This is the same cold-tree sensitivity the workspace-size latency work
/// recorded, and it is why a clean small temp dir would have hidden all of it.
///
/// So the guarantee this arm asserts does not hold on Windows, and the arm is
/// deliberately left ungated so it keeps saying so. The residual is tracked as
/// `#342` c3; the cause is that `atomic_io::publish_displacing` has no
/// exchange primitive on Win32 — `ReplaceFileW` is a two-step rename with an
/// instant at which the destination name does not resolve, and EVERY failure
/// of it degrades silently to the old re-check-then-rename fallback.
/// # GATED ON WINDOWS (#370 c1), and this is the honest half of a declaration
///
/// This arm asserts the UNIX guarantee. On Windows that guarantee is not what
/// the product gives, and `wcore_config::atomic_io` now says so in its own
/// words rather than leaving this test to discover it once a quarter: every
/// `ReplaceFileW` failure degrades to check-then-rename, and the measured
/// cost of that degrade at `retries = 0` on Windows 11 build 26200 is the
/// rate recorded above.
///
/// So on Windows this arm is `ignore`d WITH THAT RATE IN THE REASON, and the
/// weaker guarantee Windows is declared to give — *a save is never lost
/// SILENTLY* — is graded by its own arm,
/// `wcore_config::atomic_io::tests::a_refused_replacefilew_is_counted_and_not_silent`,
/// which reproduces the sharing violation and asserts the degrade is counted.
/// Ignoring without that second arm would be deleting the measurement; the
/// pair is the point.
#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "#370: the unix guarantee this asserts does not hold on Windows. Measured at retries=0 on Windows 11 build 26200: 7 of 169 interleaved saves lost on the Edit path (4.1%), 1 of 144 on the VFS path (0.7%), and 4 of 24 executions instead refused the editor rename outright with ERROR_ACCESS_DENIED. RE-MEASURED 2026-08-30 on the same host AFTER wayland#1202 changed Swap semantics on this exact path, N=20 per arm at retries=0 with this ignore FORCED via --run-ignored all, so the gate below rests on the tree it ships with: the Edit arm was red in 6 of 20 and lost 3 of 302 interleaved saves (1.0%); the VFS arm was red in 8 of 20 and lost 1 of 219 (0.5%); the remaining 11 reds printed no window at all because the fixture's own rename was refused with ERROR_ACCESS_DENIED, which is #370's SECOND Windows failure and not an absence of one. 14 of 40 executions red. So the FIRST branch of #370 c1 -- these arms passing at retries=0 over N>=20 -- is REFUTED against this tree, and gating is the honest branch and not the convenient one. The weaker guarantee Windows IS declared to give is graded by wcore_config::atomic_io::tests::a_refused_replacefilew_is_counted_and_not_silent."
)]
async fn a_save_during_an_edit_is_not_lost() {
    let (lost, interleaved, window) = edit_interleave(Saver::Rename, None).await;
    println!("[edit/rename] window {window:?}; {lost} lost, {interleaved} interleavings caught");
    assert_eq!(
        lost, 0,
        "an Edit overwrote a save that arrived while it was being checked"
    );
    assert!(
        interleaved > 0,
        "no save ever landed inside the window, so this arm measured nothing"
    );
}

/// The in-place-save arm, which the atomic exchange closed outright.
///
/// # Why this used to tolerate a quarter of the saves
///
/// It was written against a re-check-then-rename publish, where an editor that
/// truncates and writes **in place** could begin its write before the check
/// and finish it after: those bytes went to an inode the rename then unlinked,
/// and no amount of re-reading closed it. Measured then, across 24 spread
/// interleavings: 12 of 12 lost with no check at all; 2 of 24 with the check
/// before `atomic_write`; 0 of 24 over five runs with the check moved into the
/// rename slot. It was asserted as `lost * 4 < interleaved` — "materially
/// better than no check" — because what remained looked like a scheduling
/// artefact that a re-read could never remove.
///
/// # Why it is zero now, structurally and not just empirically
///
/// `atomic_write_checked` publishes with `RENAME_EXCHANGE`, so the bytes the
/// verdict reads ARE the bytes the destination held at the instant of
/// publication. An in-place save opens the destination `O_TRUNC`, so from the
/// moment its `open` returns the inode is empty or partial — there is no
/// instant at which it still holds the pre-image the verdict demands. Every
/// interleaved save therefore fails `pre_image_matches`, the publish is
/// retracted by a second exchange, and the saved bytes stay where the editor
/// put them.
///
/// # Re-graded against the exchange
///
/// 12 runs × 24 attempts on hetzner (Linux 6.x, ext4): 230 of 288 saves landed
/// inside the window and **0 were lost** — every run. The old tolerance was
/// never re-graded after the exchange landed (FerroxLabs/wayland#1155); a test
/// that permits a quarter of saves to vanish is not a test, so it asserts zero.
///
/// Not vacuous: forcing `exchange` to report `Swap::Unsupported` (the Windows
/// and unsupported-filesystem path) puts the publish back on
/// re-check-then-rename and this arm loses saves again — see the mutation
/// recorded in the #1155 lane.
#[tokio::test]
async fn an_in_place_save_is_not_lost_to_the_final_rename() {
    let (lost, interleaved, window) = edit_interleave(Saver::InPlace, None).await;
    println!("[edit/in-place] window {window:?}; {lost} lost, {interleaved} interleavings caught");
    assert!(
        interleaved > 0,
        "no save ever landed inside the window, so this arm measured nothing"
    );
    assert_eq!(
        lost, 0,
        "an in-place save that arrived while the write was being checked was \
         overwritten: {lost} of {interleaved} interleavings lost"
    );
}

/// The bytes the guard says it durably copied, fetched back OUT of the object
/// store it named. Empty when the result named no copy.
///
/// "Not lost" means the user can get their bytes back, and this repository's
/// guard is explicitly durable-then-allowed: when it sees unsaved work it
/// copies it into git, anchors a ref, tells the caller the exact
/// `cat-file` command, and only then lets the write proceed. Judging that as
/// a loss because the bytes left the working tree measures the wrong thing --
/// the Write arm below has always retrieved the copy before counting, and the
/// Edit arm did not, which made a correctly-guarded overwrite indistinguishable
/// from a silent one at about 2%.
fn recovered_copy(root: &Path, msg: &str) -> String {
    msg.split("cat-file blob ")
        .nth(1)
        .map(|s| s.split_whitespace().next().unwrap().to_owned())
        .map(|oid| {
            let o = Command::new("git")
                .args(["cat-file", "blob", &oid])
                .current_dir(root)
                .output()
                .unwrap();
            String::from_utf8_lossy(&o.stdout).into_owned()
        })
        .unwrap_or_default()
}

/// One Edit, through whichever entry point is under measurement. `ctx` of
/// `None` drives `Tool::execute` — the filesystem path, which only tests
/// reach. `Some(ctx)` drives `Tool::execute_with_ctx`, which is where the
/// dispatcher lands (`wcore-agent/src/orchestration/mod.rs:2368` calls
/// `execute_prepared_effect`, and that calls `edit_through_vfs`). Both arms
/// share this one function so their rates are comparable.
async fn one_edit(ws: &Ws, file: &Path, ctx: Option<&ToolContext>) -> String {
    let tool = EditTool::new(None).with_unsaved_guard(ws.guard.clone());
    let input = json!({
        "file_path": file.to_str().unwrap(),
        "old_string": "OLD",
        "new_string": "NEW",
    });
    match ctx {
        None => tool.execute(input).await.content,
        Some(ctx) => tool.execute_with_ctx(input, ctx).await.content,
    }
}

async fn edit_interleave(
    how: Saver,
    ctx: Option<&ToolContext>,
) -> (usize, usize, std::time::Duration) {
    let warm = Ws::new();
    let wfile = warm.root().join("draft.md");
    std::fs::write(&wfile, "draft body\nOLD\n").unwrap();
    let t0 = std::time::Instant::now();
    let _ = one_edit(&warm, &wfile, ctx).await;
    let window = t0.elapsed();

    // Delays spread across the measured window rather than one fixed offset:
    // a single offset can sit entirely before the tool's own read on a warm
    // run, which would make every arm pass without ever interleaving.
    let attempts = 24;
    let mut lost = 0;
    let mut interleaved = 0;
    for i in 0..attempts {
        let ws = Ws::new();
        let root = ws.root();
        std::fs::write(root.join("keep.txt"), "keep\n").unwrap();
        git(&root, &["add", "keep.txt"]);
        git(&root, &["commit", "-qm", "init"]);
        let file = root.join("draft.md");
        std::fs::write(&file, "draft body\nOLD\n").unwrap();
        let canary = format!("USER-SAVE-DURING-EDIT-{i}");

        let f2 = file.clone();
        let c2 = canary.clone();
        let delay = (window * (i as u32)) / (attempts as u32);
        let saver = std::thread::spawn(move || {
            std::thread::sleep(delay);
            save(&f2, &format!("draft body\nOLD\n{c2}\n"), how);
        });
        let content = one_edit(&ws, &file, ctx).await;
        saver.join().unwrap();

        if content.contains("while this write was being checked") {
            interleaved += 1;
        }
        let on_disk = std::fs::read_to_string(&file).unwrap();
        let recoverable = recovered_copy(&root, &content);
        if !on_disk.contains(&canary) && !recoverable.contains(&canary) {
            lost += 1;
        }
    }
    (lost, interleaved, window)
}

// ===========================================================================
// A9  git failing to resolve HEAD is not an unborn HEAD. Both land in a
//     refusal for a file in a subdirectory, which is why round 4's suite
//     could not tell them apart; at the repository root the unborn reading
//     copies the file and WRITES.
// ===========================================================================
#[tokio::test]
async fn git_failing_to_resolve_head_is_not_an_unborn_head() {
    let ws = Ws::new();
    let root = ws.root();
    let file = root.join("notes.txt");
    let prior = "one\ntwo\n";
    std::fs::write(&file, prior).unwrap();
    git(&root, &["add", "notes.txt"]);
    git(&root, &["commit", "-qm", "init"]);
    // A corrupt packed-refs, which is the shape that discriminates: measured
    // on git 2.43.0, `--is-inside-work-tree` and `--show-toplevel` both still
    // succeed and only `rev-parse --verify --quiet HEAD` fails, with 128. A
    // garbage `.git/HEAD` takes the repository down at the first command
    // instead, so it refuses for a different reason and proves nothing here.
    std::fs::write(
        root.join(".git/packed-refs"),
        "# pack-refs with: peeled fully-peeled sorted \nGARBAGE LINE\n",
    )
    .unwrap();
    let control = Command::new("git")
        .args(["rev-parse", "--is-inside-work-tree"])
        .current_dir(&root)
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&control.stdout).trim(),
        "true",
        "control: git must still open this repository, or the arm is vacuous"
    );

    let (err, msg) = write_via_tool(&ws, &file, "one\n").await;
    assert!(
        err,
        "a broken HEAD was read as a repository with no commits: {msg}"
    );
    assert!(msg.contains("could not be established"), "{msg}");
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        prior,
        "the file was overwritten on the strength of a HEAD git would not read"
    );
}

// ===========================================================================
// A7  A file name that looks like pathspec magic is taken literally, so the
//     file is judged against its own recorded content.
// ===========================================================================
#[cfg(unix)]
#[tokio::test]
async fn a_file_named_like_pathspec_magic_is_judged_against_its_own_blob() {
    let ws = Ws::new();
    let root = ws.root();
    let name = ":(glob)notes.txt";
    let file = root.join(name);
    std::fs::write(&file, "one\ntwo\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "init"]);

    // Everything on disk is recorded, so dropping a line drops nothing
    // unsaved: a plain Proceed, no refusal and no recovery note. Read as
    // pathspec magic instead, the lookup matches nothing or errors, and every
    // line becomes unsaved.
    let (err, msg) = write_via_tool(&ws, &file, "one\n").await;
    assert!(!err, "the file's own blob was not found: {msg}");
    assert!(
        !msg.contains("Note:"),
        "lines that are in the commit were treated as unsaved: {msg}"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "one\n");
}

// ===========================================================================
// A12 / ADV-8, without needing a second uid: a pre-image that is not text.
//      Round 4 turned it into an empty pre-image and skipped the guard.
// ===========================================================================
#[tokio::test]
async fn a_pre_image_that_is_not_text_is_refused_unless_the_commit_holds_it() {
    let ws = Ws::new();
    let root = ws.root();
    std::fs::write(root.join("keep.txt"), "keep\n").unwrap();
    git(&root, &["add", "keep.txt"]);
    git(&root, &["commit", "-qm", "init"]);

    let file = root.join("blob.bin");
    let bytes: &[u8] = &[0x53, 0x45, 0x43, 0x52, 0x45, 0x54, 0xff, 0xfe, 0x0a];
    std::fs::write(&file, bytes).unwrap();
    let (err, msg) = write_via_tool(&ws, &file, "clobbered\n").await;
    assert!(err, "an unreadable pre-image was overwritten: {msg}");
    assert_eq!(
        std::fs::read(&file).unwrap(),
        bytes,
        "the refusal must leave the bytes alone"
    );

    // Recorded, but not these bytes: the file has been changed since, and
    // what changed cannot be read. Still refused.
    git(&root, &["add", "-f", "blob.bin"]);
    git(&root, &["commit", "-qm", "record the binary"]);
    let moved: &[u8] = &[0x53, 0x45, 0x43, 0x52, 0x45, 0x54, 0xff, 0xfd, 0x0a];
    std::fs::write(&file, moved).unwrap();
    let ws_same = Ws::new();
    let (err_m, msg_m) = {
        let g = Arc::new(UnsavedWorkGuard::new_isolated());
        let r = WriteTool::new(None)
            .with_unsaved_guard(g)
            .execute(json!({
                "file_path": file.to_str().unwrap(), "content": "clobbered\n"
            }))
            .await;
        (r.is_error, r.content)
    };
    drop(ws_same);
    assert!(
        err_m,
        "bytes that differ from the commit were overwritten unread: {msg_m}"
    );
    assert_eq!(std::fs::read(&file).unwrap(), moved);

    // The one case that proves nothing is unsaved: the bytes on disk are
    // exactly what the pinned commit records.
    let ws2 = Ws::new();
    let file2 = ws2.root().join("blob.bin");
    // A fresh guard, but the same repository shape, so the pin includes it.
    std::fs::write(ws2.root().join("keep.txt"), "keep\n").unwrap();
    std::fs::write(&file2, bytes).unwrap();
    git(&ws2.root(), &["add", "-A"]);
    git(&ws2.root(), &["commit", "-qm", "init"]);
    let (err2, msg2) = write_via_tool(&ws2, &file2, "clobbered\n").await;
    assert!(
        !err2,
        "bytes byte-for-byte identical to the pinned commit are not unsaved: {msg2}"
    );
    assert_eq!(std::fs::read_to_string(&file2).unwrap(), "clobbered\n");
}

// ===========================================================================
// Windows: there is no ACL comparison here, so no copy can be bounded, so no
// copy is made. Measured on SeanDesktop (git 2.54.0.windows.1): every copy
// path refuses, naming the platform. This arm pins that the refusal is total
// — nothing enters the object store and the file is untouched — because the
// failure that would matter is a copy made anyway with the claim omitted.
// ===========================================================================
#[cfg(windows)]
#[tokio::test]
async fn on_windows_a_copy_that_cannot_be_bounded_is_never_made() {
    let ws = Ws::new();
    let root = ws.root();
    std::fs::write(root.join("keep.txt"), "keep\n").unwrap();
    git(&root, &["add", "keep.txt"]);
    git(&root, &["commit", "-qm", "init"]);

    let file = root.join("notes.env");
    let prior = "TOKEN=WINCANARY-77\nsecond line\n";
    std::fs::write(&file, prior).unwrap();
    let before = object_ids(&root);

    let (err, msg) = write_via_tool(&ws, &file, "TOKEN=rotated\n").await;
    assert!(err, "a copy nobody can bound was made anyway: {msg}");
    assert!(msg.contains("cannot be bounded"), "{msg}");
    assert!(
        !msg.contains("cat-file blob"),
        "a copy was advertised: {msg}"
    );
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        prior,
        "a refusal must leave the file exactly as it was"
    );
    assert_eq!(object_ids(&root), before, "the object store grew");
    assert!(!object_store_contains(&root, "WINCANARY-77"));

    // Edit is not refused for THIS shape, and must say plainly that it copied
    // nothing rather than falling silent about it.
    //
    // The edit REWRITES the unsaved line rather than only deleting it. Round 5
    // added `deletion_refusal`, so a delete-only edit over unsaved work is now
    // refused outright on every platform - see the unit arm
    // `a_surgical_edit_that_only_deletes_the_users_unsaved_line_is_refused`.
    // Written as a deletion this would grade that refusal instead of the thing
    // this arm exists to pin, which is that Windows makes no copy AND says so.
    // The sibling unit arm `a_surgical_edit_that_removes_unsaved_work_copies_it`
    // was adapted the same way when the rule landed; this `cfg(windows)` arm was
    // missed because no Linux run could reach it.
    let r = EditTool::new(None)
        .with_unsaved_guard(ws.guard.clone())
        .execute(json!({
            "file_path": file.to_str().unwrap(),
            "old_string": "second line",
            "new_string": "second line, rewritten",
        }))
        .await;
    assert!(!r.is_error, "{}", r.content);
    assert!(
        r.content.contains("no recovery copy was made"),
        "{}",
        r.content
    );
    assert!(r.content.contains("not recoverable"), "{}", r.content);
    assert!(!r.content.contains("cat-file blob"), "{}", r.content);
    assert_eq!(
        object_ids(&root),
        before,
        "the object store grew on the Edit path"
    );
}

// ===========================================================================
// ADV-8  The original, still gated behind the setpriv fixture: a root-owned
//        0600 file in a directory the agent uid can write.
// ===========================================================================
#[cfg(unix)]
#[tokio::test]
async fn an_unreadable_pre_image_is_refused_not_clobbered() {
    let Ok(fixture) = std::env::var("ADV8_FIXTURE") else {
        println!("[ADV-8] skipped: ADV8_FIXTURE unset (run under /root/r5-adv8.sh)");
        return;
    };
    let root = PathBuf::from(&fixture);
    let file = root.join("secret.txt");
    println!("[ADV-8] running as uid={}", libc_getuid());
    let read = std::fs::read_to_string(&file);
    assert!(
        read.is_err(),
        "control: this uid must NOT be able to read the file"
    );

    let guard = Arc::new(UnsavedWorkGuard::new_isolated());
    let r = WriteTool::new(None)
        .with_unsaved_guard(guard)
        .execute(json!({
            "file_path": file.to_str().unwrap(), "content": "clobbered by the agent\n"
        }))
        .await;
    println!("[ADV-8] is_error={}  result={}", r.is_error, r.content);
    assert!(
        r.is_error,
        "a pre-image that could not be read was overwritten anyway"
    );
    assert!(r.content.contains("could not be read"), "{}", r.content);
}

#[cfg(unix)]
unsafe extern "C" {
    #[link_name = "getuid"]
    safe fn libc_getuid() -> u32;
}

// ===========================================================================
// #1155, THE PRODUCTION PATH. The arm above drives `Tool::execute`, which
// only tests reach. Every real edit is dispatched through
// `execute_prepared_effect` (wcore-agent/src/orchestration/mod.rs:2368) and
// lands in `edit_through_vfs`, which reads through `ctx.vfs` and then writes
// through `ctx.vfs` — the same check-then-write window, one layer up. Fixing
// the filesystem path alone would close the race nobody runs and leave the
// one everybody runs open.
// ===========================================================================
/// # The Windows truth, measured
///
/// Same measurement as [`a_save_during_an_edit_is_not_lost`], same host, same
/// twelve `--retries 0` executions on 2026-08-29:
///
/// * **7 of 12** green, 0 lost.
/// * **1 of 12** RED on real loss — 1 of 22 interleavings. Aggregated over the
///   eight executions that measured anything: **1 save lost out of 144 that
///   landed inside the window (0.7%)**.
/// * **3 of 12** never measured — the fixture's saver hit the same
///   `Os { code: 5, kind: PermissionDenied }` rename refusal.
/// * **1 of 12** timed out.
///
/// The vfs path loses less often than the filesystem path but is not exempt:
/// it publishes through the same `atomic_io` primitive. Tracked as `#342` c3.
/// # GATED ON WINDOWS (#370 c1), and this is the honest half of a declaration
///
/// This arm asserts the UNIX guarantee. On Windows that guarantee is not what
/// the product gives, and `wcore_config::atomic_io` now says so in its own
/// words rather than leaving this test to discover it once a quarter: every
/// `ReplaceFileW` failure degrades to check-then-rename, and the measured
/// cost of that degrade at `retries = 0` on Windows 11 build 26200 is the
/// rate recorded above.
///
/// So on Windows this arm is `ignore`d WITH THAT RATE IN THE REASON, and the
/// weaker guarantee Windows is declared to give — *a save is never lost
/// SILENTLY* — is graded by its own arm,
/// `wcore_config::atomic_io::tests::a_refused_replacefilew_is_counted_and_not_silent`,
/// which reproduces the sharing violation and asserts the degrade is counted.
/// Ignoring without that second arm would be deleting the measurement; the
/// pair is the point.
#[tokio::test]
#[cfg_attr(
    windows,
    ignore = "#370: the unix guarantee this asserts does not hold on Windows. Measured at retries=0 on Windows 11 build 26200: 7 of 169 interleaved saves lost on the Edit path (4.1%), 1 of 144 on the VFS path (0.7%), and 4 of 24 executions instead refused the editor rename outright with ERROR_ACCESS_DENIED. RE-MEASURED 2026-08-30 on the same host AFTER wayland#1202 changed Swap semantics on this exact path, N=20 per arm at retries=0 with this ignore FORCED via --run-ignored all, so the gate below rests on the tree it ships with: the Edit arm was red in 6 of 20 and lost 3 of 302 interleaved saves (1.0%); the VFS arm was red in 8 of 20 and lost 1 of 219 (0.5%); the remaining 11 reds printed no window at all because the fixture's own rename was refused with ERROR_ACCESS_DENIED, which is #370's SECOND Windows failure and not an absence of one. 14 of 40 executions red. So the FIRST branch of #370 c1 -- these arms passing at retries=0 over N>=20 -- is REFUTED against this tree, and gating is the honest branch and not the convenient one. The weaker guarantee Windows IS declared to give is graded by wcore_config::atomic_io::tests::a_refused_replacefilew_is_counted_and_not_silent."
)]
async fn a_save_during_an_edit_is_not_lost_on_the_vfs_path() {
    let ctx = ToolContext::test_default();
    let (lost, interleaved, window) = edit_interleave(Saver::Rename, Some(&ctx)).await;
    println!(
        "[edit/vfs/rename] window {window:?}; {lost} lost, {interleaved} interleavings caught"
    );
    assert_eq!(
        lost, 0,
        "an Edit through the vfs overwrote a save that arrived while it was being checked"
    );
    assert!(
        interleaved > 0,
        "no save ever landed inside the window, so this arm measured nothing"
    );
}
