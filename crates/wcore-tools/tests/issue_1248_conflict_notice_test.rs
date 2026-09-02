//! #1248 c4 — the wrong-refusal control for the intercepted-save notice.
//!
//! c1 gives `FileMutationOutcome::Conflict` a field naming where a refusal's
//! own retraction preserved somebody's save. Exactly ONE construction site can
//! ever fill it: the arm that matched `Err(_)` out of `atomic_write_checked`.
//! Every other `Conflict` refused BEFORE anything was published, so nothing
//! can have been displaced, and what the user is told there must not change by
//! one character.
//!
//! The producers are enumerated from the tree rather than from memory:
//!
//! ```text
//! $ git grep -n 'FileMutationOutcome::Conflict {' -- 'crates/*/src'
//! crates/wcore-tools/src/vfs.rs:534   RealFs   pre-flight, postcondition authority
//! crates/wcore-tools/src/vfs.rs:548   RealFs   pre-flight, precondition / already-intended
//! crates/wcore-tools/src/vfs.rs:583   RealFs   the retraction arm  <-- the only Some
//! crates/wcore-tools/src/vfs.rs:1526  InMemoryFs  postcondition authority
//! crates/wcore-tools/src/vfs.rs:1538  InMemoryFs  destination already holds the intended bytes
//! crates/wcore-tools/src/vfs.rs:1545  InMemoryFs  precondition
//! crates/wcore-tools/src/vfs.rs:2033  SandboxedFs containment pre-flight
//! ```
//!
//! The control on that query is `FileMutationOutcome::Applied`, which the same
//! grep shape finds in the same files — an empty result would otherwise read
//! as "there are no other producers" and is the way to be wrong here.
//!
//! Two things are graded, because two different mistakes are available:
//!
//! * the PRODUCERS answer `None` — a site that defaulted the field to a path
//!   would claim a save nobody displaced;
//! * the RENDERERS still emit `changed_under_write` verbatim — a renderer that
//!   treated the field as always-present would either panic or promote every
//!   conflict to the displacing wording.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tempfile::tempdir;

use wcore_tools::Tool;
use wcore_tools::context::ToolContext;
use wcore_tools::edit::EditTool;
use wcore_tools::file_write_notifier::FileWriteNotifier;
use wcore_tools::unsaved_work::{UnsavedWorkGuard, changed_under_write};
use wcore_tools::vfs::{
    FileContentIdentity, FileMutationOutcome, FileObservation, FilePrecondition, InMemoryFs,
    IntendedFileMutation, RealFs, SandboxedFs, VirtualFs,
};
use wcore_tools::write::WriteTool;

/// The one production seam between "the tool judged the pre-image" and "the
/// tool compare-exchanges against it": Write and Edit both notify a watcher
/// there so it can debounce its own event. A notifier that writes to the file
/// puts a real, non-cooperating change inside that gap, which is what drives
/// the pre-flight classification arms without any test-only hook at all.
struct WritesInTheGap {
    bytes: Vec<u8>,
    vfs: Arc<dyn VirtualFs>,
}

#[async_trait]
impl FileWriteNotifier for WritesInTheGap {
    async fn note_self_originated_write(&self, path: &Path) {
        self.vfs.write(path, &self.bytes).await.unwrap();
    }
}

fn write_tool() -> WriteTool {
    WriteTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()))
}

fn edit_tool() -> EditTool {
    EditTool::new(None).with_unsaved_guard(Arc::new(UnsavedWorkGuard::new_isolated()))
}

fn ctx_with(vfs: Arc<dyn VirtualFs>, notifier: Arc<dyn FileWriteNotifier>) -> ToolContext {
    ToolContext::new(
        "c4",
        tokio_util::sync::CancellationToken::new(),
        vfs,
        None,
        Arc::new(wcore_tools::NullToolOutputSink),
    )
    .with_file_write_notifier(notifier)
}

/// Today's wording, verbatim, so a diff to it is a diff to this assertion.
/// Checked BOTH ways: against the renderer the tool is supposed to call, and
/// against the literal sentence #1248 names ("does not end 'Nothing was
/// changed.'" is the c2 property, so its negation is the c4 property).
#[track_caller]
fn assert_unchanged_wording(rendered: &str, display_path: &str, why: &str) {
    assert_eq!(
        rendered,
        changed_under_write(display_path, why),
        "a conflict that displaced nothing no longer renders today's wording"
    );
    assert!(
        rendered.ends_with(
            "Nothing was changed. Read the file as it stands now and redo the change against that."
        ),
        "the unchanged wording lost its ending: {rendered}"
    );
    assert!(
        !rendered.contains("preserved at"),
        "a conflict that displaced nothing claims a preserved save: {rendered}"
    );
}

/// RealFs pre-flight, through the real `WriteTool` surface.
#[tokio::test]
async fn real_fs_preflight_conflict_still_renders_todays_wording() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("f.txt");
    std::fs::write(&p, "the only copy of the user's bytes\n").unwrap();

    let ctx = ctx_with(
        Arc::new(RealFs),
        Arc::new(WritesInTheGap {
            bytes: b"what the user saved before the exchange\n".to_vec(),
            vfs: Arc::new(RealFs),
        }),
    );

    let result = write_tool()
        .execute_with_ctx(
            json!({
                "file_path": p.to_str().unwrap(),
                "content": "the only copy of the user's bytes\nand a line the agent added\n",
            }),
            &ctx,
        )
        .await;

    assert!(result.is_error, "not refused at all: {}", result.content);
    assert_unchanged_wording(
        &result.content,
        p.to_str().unwrap(),
        "its contents changed on disk",
    );

    // Sensitivity control: the refusal really was taken before any publish, so
    // the destination still holds what the gap wrote and NOT the agent's bytes.
    assert_eq!(
        std::fs::read(&p).unwrap(),
        b"what the user saved before the exchange\n"
    );
}

/// The `InMemoryFs` arm's destination, absolute on BOTH platforms.
///
/// `/w/f.txt` has a root but NO drive prefix, so `Path::is_absolute()` is
/// FALSE on Windows -- the same fact `archive_tool.rs` relies on for its own
/// Windows arm. `WriteTool` runs `validate_user_path` on the raw argument
/// before it reaches any backend, so a POSIX-shaped literal made this test
/// measure `PathValidationError::NotAbsolute` ("Refused to write /w/f.txt:
/// path must be absolute") instead of the conflict renderer it names: green
/// on Unix, and grading nothing at all on Windows (#409 c5).
///
/// Refusing a rooted-but-driveless path on Windows is the tool's intended
/// contract -- such a path resolves against whichever drive happens to be
/// current -- so the fixture moves, not the guard. The backend under test is
/// a `HashMap<PathBuf, _>` with no notion of a real filesystem, and the
/// wording assertion compares against the raw argument the tool echoes back,
/// so a real Windows absolute changes nothing that is graded here.
#[cfg(windows)]
const MEM_DEST: &str = r"C:\w\f.txt";
#[cfg(not(windows))]
const MEM_DEST: &str = "/w/f.txt";

/// The `InMemoryFs` backend, through the same surface. A different producer,
/// on a backend with no `atomic_write_checked` anywhere in it.
#[tokio::test]
async fn in_memory_backend_conflict_still_renders_todays_wording() {
    let mem = Arc::new(InMemoryFs::new());
    let p = PathBuf::from(MEM_DEST);
    mem.write(&p, b"the only copy of the user's bytes\n")
        .await
        .unwrap();

    let ctx = ctx_with(
        Arc::clone(&mem) as Arc<dyn VirtualFs>,
        Arc::new(WritesInTheGap {
            bytes: b"what the user saved before the exchange\n".to_vec(),
            vfs: Arc::clone(&mem) as Arc<dyn VirtualFs>,
        }),
    );

    let result = write_tool()
        .execute_with_ctx(
            json!({
                "file_path": MEM_DEST,
                "content": "the only copy of the user's bytes\nand a line the agent added\n",
            }),
            &ctx,
        )
        .await;

    assert!(result.is_error, "not refused at all: {}", result.content);
    assert_unchanged_wording(&result.content, MEM_DEST, "its contents changed on disk");
    assert_eq!(
        mem.read(&p).await.unwrap(),
        b"what the user saved before the exchange\n"
    );
}

/// The Edit renderer is a SEPARATE match arm from Write's, so it is graded
/// separately rather than by analogy.
#[tokio::test]
async fn the_edit_path_conflict_still_renders_todays_wording() {
    let dir = tempdir().unwrap();
    let p = dir.path().join("f.txt");
    std::fs::write(&p, "the only copy of the user's bytes\n").unwrap();

    let ctx = ctx_with(
        Arc::new(RealFs),
        Arc::new(WritesInTheGap {
            bytes: b"what the user saved before the exchange\n".to_vec(),
            vfs: Arc::new(RealFs),
        }),
    );

    let result = edit_tool()
        .execute_with_ctx(
            json!({
                "file_path": p.to_str().unwrap(),
                "old_string": "the only copy",
                "new_string": "the only surviving copy",
            }),
            &ctx,
        )
        .await;

    assert!(result.is_error, "not refused at all: {}", result.content);
    assert_unchanged_wording(
        &result.content,
        p.to_str().unwrap(),
        "its contents changed on disk",
    );
}

/// The producers, at the `compare_exchange_file` surface, one arm each.
///
/// This is the half that fails if c1's field is treated as always-present at
/// the SOURCE: a site that filled it with a plausible-looking path (the temp
/// sibling, the destination itself) would satisfy every rendering test above
/// and be a lie here.
#[tokio::test]
async fn no_conflict_without_a_publish_names_an_intercepted_save() {
    let dir = tempdir().unwrap();

    // 1 + 2. RealFs pre-flight: the precondition names bytes that are not
    // there.
    let real = dir.path().join("real.txt");
    std::fs::write(&real, b"after").unwrap();
    let stale = IntendedFileMutation::new(
        FilePrecondition::Present(FileContentIdentity::from_bytes(b"before")),
        b"stale".to_vec(),
    );
    assert_eq!(
        RealFs.compare_exchange_file(&real, &stale).await.unwrap(),
        FileMutationOutcome::Conflict {
            current: FileObservation::Present(FileContentIdentity::from_bytes(b"after")),
            intercepted_save: None,
        }
    );

    // 3. RealFs pre-flight, the create-over-something arm.
    let created = dir.path().join("created.txt");
    std::fs::write(&created, b"somebody got there first").unwrap();
    let create = IntendedFileMutation::new(FilePrecondition::Absent, b"ours".to_vec());
    assert_eq!(
        RealFs
            .compare_exchange_file(&created, &create)
            .await
            .unwrap(),
        FileMutationOutcome::Conflict {
            current: FileObservation::Present(FileContentIdentity::from_bytes(
                b"somebody got there first"
            )),
            intercepted_save: None,
        }
    );

    // 4. InMemoryFs.
    let mem = InMemoryFs::new();
    let mp = PathBuf::from("/w/f.txt");
    mem.write(&mp, b"after").await.unwrap();
    assert_eq!(
        mem.compare_exchange_file(&mp, &stale).await.unwrap(),
        FileMutationOutcome::Conflict {
            current: FileObservation::Present(FileContentIdentity::from_bytes(b"after")),
            intercepted_save: None,
        }
    );

    // 5. The containment wrapper's own pre-flight, which refuses before it
    // ever delegates to the backend underneath it.
    let jail = SandboxedFs::new(RealFs, dir.path());
    assert_eq!(
        jail.compare_exchange_file(&real, &stale).await.unwrap(),
        FileMutationOutcome::Conflict {
            current: FileObservation::Present(FileContentIdentity::from_bytes(b"after")),
            intercepted_save: None,
        }
    );

    // 6. RealFs, the postcondition-AUTHORITY arm — the FIRST refusal
    //    `compare_exchange_file` can take, before the precondition is ever
    //    consulted. A mutation prepared against one path object, offered at
    //    another. Reached by preparing elsewhere rather than by racing.
    let elsewhere = dir.path().join("elsewhere.txt");
    std::fs::write(&elsewhere, b"prepared against this one").unwrap();
    let prepared = RealFs.observe_file(&elsewhere).await.unwrap();
    let rebound = IntendedFileMutation::from_observation(&prepared, b"ours".to_vec());
    assert_eq!(
        RealFs.compare_exchange_file(&real, &rebound).await.unwrap(),
        FileMutationOutcome::Conflict {
            current: FileObservation::Present(FileContentIdentity::from_bytes(b"after")),
            intercepted_save: None,
        }
    );

    // 7. InMemoryFs, the same arm. Separate code, so graded separately.
    let mem_elsewhere = PathBuf::from("/w/elsewhere.txt");
    mem.write(&mem_elsewhere, b"prepared against this one")
        .await
        .unwrap();
    let mem_prepared = mem.observe_file(&mem_elsewhere).await.unwrap();
    let mem_rebound = IntendedFileMutation::from_observation(&mem_prepared, b"ours".to_vec());
    assert_eq!(
        mem.compare_exchange_file(&mp, &mem_rebound).await.unwrap(),
        FileMutationOutcome::Conflict {
            current: FileObservation::Present(FileContentIdentity::from_bytes(b"after")),
            intercepted_save: None,
        }
    );

    // 8. InMemoryFs, the arm where the destination already holds exactly the
    //    intended bytes but the prepared object underneath them is a different
    //    generation — same bytes, different file. Not `AlreadyApplied`, and
    //    still nothing published.
    let settled = PathBuf::from("/w/settled.txt");
    mem.write(&settled, b"settled").await.unwrap();
    let pinned = mem.observe_file(&settled).await.unwrap();
    let same_bytes = IntendedFileMutation::from_observation(&pinned, b"settled".to_vec());
    mem.write(&settled, b"settled").await.unwrap();
    assert_eq!(
        mem.compare_exchange_file(&settled, &same_bytes)
            .await
            .unwrap(),
        FileMutationOutcome::Conflict {
            current: FileObservation::Present(FileContentIdentity::from_bytes(b"settled")),
            intercepted_save: None,
        }
    );

    // Sensitivity control. The arms above would all pass against a backend
    // that answered `Conflict { intercepted_save: None }` to EVERYTHING, which
    // would make this test vacuous. The same mutations against a matching
    // precondition must apply.
    let fresh = IntendedFileMutation::new(
        FilePrecondition::Present(FileContentIdentity::from_bytes(b"after")),
        b"ours".to_vec(),
    );
    assert!(matches!(
        RealFs.compare_exchange_file(&real, &fresh).await.unwrap(),
        FileMutationOutcome::Applied { .. }
    ));
    assert!(matches!(
        mem.compare_exchange_file(&mp, &fresh).await.unwrap(),
        FileMutationOutcome::Applied { .. }
    ));
}

// ---------------------------------------------------------------------------
// #1248 — the SHAPE, not the three instances.
// ---------------------------------------------------------------------------

/// Comment lines carry no code, and `FileMutationOutcome::Conflict` appears in
/// doc links in this tree.
fn without_comment_lines(source: &str) -> String {
    source
        .lines()
        .map(|line| {
            let t = line.trim_start();
            if t.starts_with("//") || t.starts_with('*') {
                ""
            } else {
                line
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Every `FileMutationOutcome::Conflict` in `source` whose own brace group does
/// not name `intercepted_save`, and how many sites were examined to say so.
///
/// The count comes back so a caller can refuse a run that examined nothing: an
/// empty offender list off an empty scan reads exactly like a clean tree.
fn conflict_sites_missing_the_notice(source: &str) -> (usize, Vec<String>) {
    const NEEDLE: &str = "FileMutationOutcome::Conflict";
    let cleaned = without_comment_lines(source);
    let (mut seen, mut offenders, mut from) = (0usize, Vec::new(), 0usize);

    while let Some(rel) = cleaned[from..].find(NEEDLE) {
        let at = from + rel;
        from = at + NEEDLE.len();
        seen += 1;

        let rest = &cleaned[from..];
        let Some(open) = rest.find('{') else {
            offenders.push(rest.chars().take(90).collect::<String>());
            continue;
        };
        let mut depth = 0usize;
        let mut end = None;
        for (i, c) in rest[open..].char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(open + i);
                        break;
                    }
                }
                _ => {}
            }
        }
        let group = end.map_or(&rest[open..], |e| &rest[open..=e]);
        if !group.contains("intercepted_save") {
            offenders.push(format!("{NEEDLE} {}", group.replace('\n', " ")));
        }
    }
    (seen, offenders)
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_dir() {
            rust_sources(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// #1248, asked as a decidable question instead of as a list.
///
/// Three consumers of `Conflict` existed when this was written — `write.rs`,
/// `edit.rs`, and wcore-agent's `rollback_tool.rs` — and all three discarded
/// the notice. Fixing three sites is an enumeration, and an enumeration is
/// correct only until the fourth consumer is written: the field is an
/// `Option`, so ignoring it is silent and compiles.
///
/// The total form of the same question is: does any production site NAME
/// `FileMutationOutcome::Conflict` without naming `intercepted_save` inside
/// its own brace group? A construction site cannot fail that — the compiler
/// requires every field — so what it actually decides is the CONSUMER
/// question, over the whole workspace, including consumers that do not exist
/// yet. `Conflict { .. }` is precisely the defect this issue reports, one
/// layer up, and it is now a test failure rather than a code review.
#[test]
fn no_production_site_names_a_conflict_without_naming_the_notice() {
    // The checker's own known-positive control, in the same test: a scan that
    // silently matched nothing would satisfy every assertion below.
    let (bad_seen, bad) = conflict_sites_missing_the_notice(
        "match o { Ok(FileMutationOutcome::Conflict { .. }) => refuse() }",
    );
    assert_eq!(
        (bad_seen, bad.len()),
        (1, 1),
        "the checker missed a known offender"
    );
    let (good_seen, good) = conflict_sites_missing_the_notice(
        "match o { Ok(FileMutationOutcome::Conflict { intercepted_save, .. }) => n(intercepted_save) }",
    );
    assert_eq!(
        (good_seen, good.len()),
        (1, 0),
        "the checker rejects a known-good site"
    );
    assert_eq!(
        conflict_sites_missing_the_notice("/// see FileMutationOutcome::Conflict { .. } in vfs.rs")
            .0,
        0,
        "the checker reads doc comments as code"
    );

    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> has two ancestors")
        .to_path_buf();
    let mut files = Vec::new();
    for crate_dir in std::fs::read_dir(workspace.join("crates"))
        .unwrap()
        .flatten()
    {
        rust_sources(&crate_dir.path().join("src"), &mut files);
    }
    assert!(
        files.len() > 100,
        "the walk found {} production sources, so it did not run over this \
         workspace and a clean result would mean nothing",
        files.len()
    );

    let (mut examined, mut offenders) = (0usize, Vec::new());
    for file in &files {
        let (seen, bad) =
            conflict_sites_missing_the_notice(&std::fs::read_to_string(file).unwrap());
        examined += seen;
        offenders.extend(bad.into_iter().map(|b| format!("{}: {b}", file.display())));
    }
    assert!(
        examined >= 10,
        "only {examined} Conflict sites were examined; the query stopped \
         matching the tree it grades"
    );
    assert!(
        offenders.is_empty(),
        "these sites name a Conflict and drop the intercepted-save notice \
         (#1248): {offenders:#?}"
    );
}
