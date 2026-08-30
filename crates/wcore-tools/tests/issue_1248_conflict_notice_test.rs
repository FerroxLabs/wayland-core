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

/// The `InMemoryFs` backend, through the same surface. A different producer,
/// on a backend with no `atomic_write_checked` anywhere in it.
#[tokio::test]
async fn in_memory_backend_conflict_still_renders_todays_wording() {
    let mem = Arc::new(InMemoryFs::new());
    let p = PathBuf::from("/w/f.txt");
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
                "file_path": "/w/f.txt",
                "content": "the only copy of the user's bytes\nand a line the agent added\n",
            }),
            &ctx,
        )
        .await;

    assert!(result.is_error, "not refused at all: {}", result.content);
    assert_unchanged_wording(&result.content, "/w/f.txt", "its contents changed on disk");
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
