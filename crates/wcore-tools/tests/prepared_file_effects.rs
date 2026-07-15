//! F13 production classification for ordinary workspace file tools.
//!
//! POSIX rename primitives cannot conditionally replace an existing pathname
//! against a non-cooperating writer. Write and Edit therefore remain usable
//! but deliberately opaque: orchestration journals their physical boundary,
//! never auto-replays an ambiguous start, and does not fabricate a filesystem
//! transaction receipt.

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use tokio_util::sync::CancellationToken;
use wcore_tools::context::ToolContext;
use wcore_tools::edit::EditTool;
use wcore_tools::vfs::{InMemoryFs, VirtualFs};
use wcore_tools::write::WriteTool;
use wcore_tools::{NullToolOutputSink, Tool};
use wcore_types::tool::ToolEffectKind;

fn context(fs: Arc<InMemoryFs>) -> ToolContext {
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

#[test]
fn write_and_edit_are_explicitly_opaque_on_every_platform() {
    for tool in [
        Box::new(WriteTool::new(None)) as Box<dyn Tool>,
        Box::new(EditTool::new(None)) as Box<dyn Tool>,
    ] {
        let contract = tool.effect_contract(&json!({}));
        assert_eq!(contract.kind, ToolEffectKind::Opaque);
        assert!(contract.reconciler.is_none());
    }
}

#[tokio::test]
async fn write_and_edit_keep_the_functional_opaque_path() {
    let fs = Arc::new(InMemoryFs::new());
    let ctx = context(Arc::clone(&fs));
    let path = test_path("opaque-file-effect.txt");
    let path_value = path.to_string_lossy().into_owned();
    let write = WriteTool::new(None);
    let write_input = json!({"file_path": path_value.clone(), "content": "before"});

    assert!(
        write
            .prepare_effect(&write_input, &ctx)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!write.execute_with_ctx(write_input, &ctx).await.is_error);

    let edit = EditTool::new(None);
    let edit_input = json!({
        "file_path": path_value,
        "old_string": "before",
        "new_string": "after",
    });
    assert!(
        edit.prepare_effect(&edit_input, &ctx)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!edit.execute_with_ctx(edit_input, &ctx).await.is_error);
    assert_eq!(fs.read(&path).await.unwrap(), b"after");
}
