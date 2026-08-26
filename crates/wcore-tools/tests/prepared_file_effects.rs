//! F13 effect classification for the workspace file tools.
//!
//! Write and Edit bind a durable receipt before the write: the preimage
//! identity, the intended postimage identity, and the kernel object the
//! preparation saw. That does not make the write exclusive — no supported
//! host filesystem offers pathname compare-and-swap against a non-cooperating
//! writer, and [`wcore_tools::vfs::VirtualFs::compare_exchange_file`] stays
//! unimplemented for `RealFs`. It makes the write *classifiable*: after a
//! crash, one read of the target says "landed", "did not land", or "cannot
//! tell", and only the third answer needs a human.
//!
//! Preparation declines rather than fails. A target that cannot be identified
//! before the write — a symlink, a hard link, a preimage past the checkpoint
//! bound, a backend with no identity primitive — produces no receipt, the
//! tool runs its ordinary path, and recovery is opaque exactly as it was.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use wcore_tools::context::ToolContext;
use wcore_tools::edit::EditTool;
use wcore_tools::effects::{
    FILESYSTEM_EFFECT_RECONCILER, FilesystemEffectPrecondition, MAX_PREPARED_PREIMAGE_BYTES,
    ToolEffectDisposition,
};
use wcore_tools::vfs::{InMemoryFs, RealFs, VirtualFs};
use wcore_tools::write::WriteTool;
use wcore_tools::{NullToolOutputSink, Tool};
use wcore_types::tool::ToolEffectKind;

fn context(fs: Arc<dyn VirtualFs>) -> ToolContext {
    ToolContext::new(
        "file-effect-test",
        CancellationToken::new(),
        fs,
        None,
        Arc::new(NullToolOutputSink),
    )
}

fn test_path(name: &str) -> PathBuf {
    std::env::current_dir().unwrap().join(name)
}

fn write_tool() -> WriteTool {
    WriteTool::new(None).with_unsaved_guard(Arc::new(
        wcore_tools::unsaved_work::UnsavedWorkGuard::new_isolated(),
    ))
}

fn edit_tool() -> EditTool {
    EditTool::new(None).with_unsaved_guard(Arc::new(
        wcore_tools::unsaved_work::UnsavedWorkGuard::new_isolated(),
    ))
}

#[test]
fn write_and_edit_declare_a_filesystem_transactional_effect() {
    for tool in [
        Box::new(write_tool()) as Box<dyn Tool>,
        Box::new(edit_tool()) as Box<dyn Tool>,
    ] {
        let contract = tool.effect_contract(&json!({}));
        assert_eq!(contract.kind, ToolEffectKind::FilesystemTransactional);
        assert_eq!(
            contract.reconciler.as_deref(),
            Some(FILESYSTEM_EFFECT_RECONCILER)
        );
    }
}

/// `Opaque` on every other tool is a decision, not an unfinished migration.
///
/// Each of these mutates state this process cannot photograph beforehand and
/// cannot re-read afterwards: a shell command touches arbitrary host state, a
/// fetch touches a remote service's rate limit, a nested step aggregates
/// effects it does not own. There is no preimage, no intended postimage and
/// no object identity to compare, so there is nothing a reconciler could be
/// right about. Promoting any of them requires that same three-way evidence
/// first; without it the honest answer stays "ask the operator".
#[test]
fn the_tools_without_reconcilable_evidence_stay_opaque_on_purpose() {
    let opaque: Vec<Box<dyn Tool>> = vec![
        Box::new(wcore_tools::bash::BashTool),
        Box::new(wcore_tools::web_fetch::WebFetchTool::default()),
    ];
    for tool in opaque {
        let contract = tool.effect_contract(&json!({}));
        assert_eq!(
            contract.kind,
            ToolEffectKind::Opaque,
            "{} must stay opaque",
            tool.name()
        );
        assert!(contract.reconciler.is_none(), "{}", tool.name());
    }
}

#[tokio::test]
async fn write_binds_the_preimage_and_the_intended_postimage_before_writing() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("bound.txt");
    std::fs::write(&target, b"one\n").unwrap();

    let ctx = context(Arc::new(RealFs));
    let input = json!({
        "file_path": target.to_string_lossy(),
        "content": "one\ntwo\n",
    });
    let prepared = write_tool()
        .prepare_effect(&input, &ctx)
        .await
        .unwrap()
        .expect("an ordinary regular file is preparable");
    let receipt = prepared.filesystem_receipt();

    assert_eq!(receipt.reconciler, FILESYSTEM_EFFECT_RECONCILER);
    assert_eq!(receipt.path(), target.as_path());
    assert!(matches!(
        receipt.precondition,
        FilesystemEffectPrecondition::Present { .. }
    ));
    assert_eq!(
        receipt.precondition_identity().map(|identity| identity.len),
        Some(4)
    );
    assert_eq!(receipt.intended.len, 8);
    assert_eq!(
        receipt.checkpoint_identity(),
        receipt.precondition_identity(),
        "a present precondition must carry the checkpoint that makes it recoverable"
    );
    assert_eq!(prepared.preimage_bytes(), Some(&b"one\n"[..]));
    // The target is untouched: preparation is a read.
    assert_eq!(std::fs::read(&target).unwrap(), b"one\n");
}

#[tokio::test]
async fn edit_binds_the_exact_bytes_its_write_body_will_produce() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("edited.txt");
    std::fs::write(&target, b"hello world\n").unwrap();

    let ctx = context(Arc::new(RealFs));
    let input = json!({
        "file_path": target.to_string_lossy(),
        "old_string": "hello",
        "new_string": "goodbye",
    });
    let tool = edit_tool();
    let prepared = tool
        .prepare_effect(&input, &ctx)
        .await
        .unwrap()
        .expect("an ordinary regular file is preparable");
    let intended = prepared.filesystem_receipt().intended.clone();

    let execution = tool.execute_prepared_effect(prepared, &ctx).await;
    assert!(!execution.result.is_error, "{}", execution.result.content);
    assert_eq!(execution.disposition, ToolEffectDisposition::Applied);
    assert_eq!(std::fs::read(&target).unwrap(), b"goodbye world\n");
    assert_eq!(
        intended.len,
        std::fs::metadata(&target).unwrap().len(),
        "the receipt must bind the bytes the write body actually produces"
    );
}

/// An edit whose `old_string` is not there never reaches the filesystem, and
/// the classification must say so positively rather than leaving the effect
/// ambiguous — an ambiguous record blocks the session until a human clears it.
#[tokio::test]
async fn an_edit_that_cannot_match_is_not_applied_rather_than_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("nomatch.txt");
    std::fs::write(&target, b"hello world\n").unwrap();

    let ctx = context(Arc::new(RealFs));
    // Prepared against a matching edit, then executed against one that cannot
    // match, which is the only way to reach the write body's own refusal with
    // a receipt in hand.
    let tool = edit_tool();
    let prepared = tool
        .prepare_effect(
            &json!({
                "file_path": target.to_string_lossy(),
                "old_string": "hello",
                "new_string": "goodbye",
            }),
            &ctx,
        )
        .await
        .unwrap()
        .expect("preparable");
    std::fs::write(&target, b"nothing to match here\n").unwrap();

    let execution = tool.execute_prepared_effect(prepared, &ctx).await;
    assert!(execution.result.is_error);
    assert_eq!(execution.disposition, ToolEffectDisposition::NotApplied);
    assert_eq!(std::fs::read(&target).unwrap(), b"nothing to match here\n");
}

/// Preparation declines instead of failing, and the tool keeps working.
#[cfg(unix)]
#[tokio::test]
async fn a_symlinked_target_stays_opaque_and_still_writes() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.txt");
    let link = dir.path().join("link.txt");
    std::fs::write(&real, b"one\n").unwrap();
    std::os::unix::fs::symlink(&real, &link).unwrap();

    let ctx = context(Arc::new(RealFs));
    let input = json!({
        "file_path": link.to_string_lossy(),
        "content": "one\ntwo\n",
    });
    let tool = write_tool();
    assert!(
        tool.prepare_effect(&input, &ctx).await.unwrap().is_none(),
        "a symlinked leaf has no identity the reconciler could compare, so it \
         must stay opaque rather than bind a receipt it cannot honour"
    );
    // Control: the write itself is unaffected by the declined preparation.
    // Note what it does — the atomic tmp+rename replaces the LINK with a
    // regular file and leaves the link target alone. That is shipped
    // behaviour, and it is also precisely why a symlinked leaf has no stable
    // object identity for a receipt to be compared against afterwards.
    let result = tool.execute_with_ctx(input, &ctx).await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(std::fs::read(&link).unwrap(), b"one\ntwo\n");
    assert_eq!(std::fs::read(&real).unwrap(), b"one\n");
    assert!(!std::fs::symlink_metadata(&link).unwrap().is_symlink());
}

#[tokio::test]
async fn a_preimage_past_the_checkpoint_bound_stays_opaque() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("huge.bin");
    let oversized = vec![b'x'; MAX_PREPARED_PREIMAGE_BYTES as usize + 1];
    std::fs::write(&target, &oversized).unwrap();

    let ctx = context(Arc::new(RealFs));
    let input = json!({
        "file_path": target.to_string_lossy(),
        "content": "small",
    });
    assert!(
        write_tool()
            .prepare_effect(&input, &ctx)
            .await
            .unwrap()
            .is_none()
    );

    // Control at the other side of the bound: an ordinary file still prepares.
    let small = dir.path().join("small.bin");
    std::fs::write(&small, b"one\n").unwrap();
    let small_input = json!({"file_path": small.to_string_lossy(), "content": "one\ntwo\n"});
    assert!(
        write_tool()
            .prepare_effect(&small_input, &ctx)
            .await
            .unwrap()
            .is_some()
    );
}

/// Malformed input is the ordinary path's error to report, with the wording it
/// already has. Preparation must decline, never refuse the call itself.
#[tokio::test]
async fn malformed_input_declines_preparation_instead_of_refusing_the_call() {
    let ctx = context(Arc::new(InMemoryFs::new()));
    assert!(
        write_tool()
            .prepare_effect(&json!({"file_path": "/x"}), &ctx)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        edit_tool()
            .prepare_effect(&json!({"file_path": "/x", "old_string": "a"}), &ctx)
            .await
            .unwrap()
            .is_none()
    );
    // A relative path never survives `validate_user_path`, so it cannot be
    // bound into a receipt either.
    assert!(
        write_tool()
            .prepare_effect(&json!({"file_path": "relative.txt", "content": "x"}), &ctx)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn the_prepared_path_writes_exactly_once_through_a_cooperative_backend() {
    let fs = Arc::new(InMemoryFs::new());
    let ctx = context(fs.clone());
    let path = test_path("prepared-file-effect.txt");
    let path_value = path.to_string_lossy().into_owned();

    let write = write_tool();
    let write_input = json!({"file_path": path_value.clone(), "content": "before"});
    let prepared = write
        .prepare_effect(&write_input, &ctx)
        .await
        .unwrap()
        .expect("an absent target is preparable");
    assert!(matches!(
        prepared.filesystem_receipt().precondition,
        FilesystemEffectPrecondition::Absent
    ));
    assert_eq!(
        prepared.preimage_bytes(),
        None,
        "an absent precondition has no preimage to checkpoint"
    );
    let execution = write.execute_prepared_effect(prepared, &ctx).await;
    assert!(!execution.result.is_error, "{}", execution.result.content);
    assert_eq!(execution.disposition, ToolEffectDisposition::Applied);
    assert_eq!(fs.read(&path).await.unwrap(), b"before");

    let edit = edit_tool();
    let edit_input = json!({
        "file_path": path_value,
        "old_string": "before",
        "new_string": "after",
    });
    let prepared = edit
        .prepare_effect(&edit_input, &ctx)
        .await
        .unwrap()
        .expect("a present target is preparable");
    let execution = edit.execute_prepared_effect(prepared, &ctx).await;
    assert!(!execution.result.is_error, "{}", execution.result.content);
    assert_eq!(execution.disposition, ToolEffectDisposition::Applied);
    assert_eq!(fs.read(&path).await.unwrap(), b"after");
}
