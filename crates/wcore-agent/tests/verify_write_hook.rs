//! F15 verification loop tests (Write only — Edit verification is W6.1).

use serde_json::json;
use wcore_agent::hooks::verify_write::VerifyWriteHook;
use wcore_agent::hooks::{Hook, HookAction};
use wcore_types::message::{ContentBlock, Role};

fn injected_text(action: &HookAction) -> Option<String> {
    match action {
        HookAction::InjectMessage(m) => {
            assert_eq!(m.role, Role::User, "verification injects as user role");
            m.content.iter().find_map(|b| match b {
                ContentBlock::Text { text } => Some(text.clone()),
                _ => None,
            })
        }
        _ => None,
    }
}

#[tokio::test]
async fn verify_write_continues_when_file_matches_recorded_content() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.txt");
    let content = "hello world\n";
    std::fs::write(&path, content).unwrap();

    let hook = VerifyWriteHook::new();
    let input = json!({ "file_path": path.to_str().unwrap(), "content": content });
    let action = hook
        .post_tool_use("Write", "call-1", &input, "wrote a.txt", false)
        .await;
    assert!(matches!(action, HookAction::Continue));
}

#[tokio::test]
async fn verify_write_injects_message_when_file_disagrees_with_recorded_content() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("a.txt");
    let expected = "expected\n";
    std::fs::write(&path, "actual\n").unwrap(); // disagrees on purpose

    let hook = VerifyWriteHook::new();
    let input = json!({ "file_path": path.to_str().unwrap(), "content": expected });
    let action = hook
        .post_tool_use("Write", "call-2", &input, "wrote a.txt", false)
        .await;
    let text = injected_text(&action).expect("expected InjectMessage with Text content");
    assert!(
        text.contains("verification"),
        "message must mention verification: {text}"
    );
    assert!(text.contains("a.txt"), "message must name the file: {text}");
}

#[tokio::test]
async fn verify_write_skips_non_write_tools() {
    let hook = VerifyWriteHook::new();
    // Edit is intentionally skipped — its output is a status string,
    // not a recoverable post-state. Edit verification is W6.1.
    let action = hook
        .post_tool_use(
            "Edit",
            "call-3",
            &json!({"file_path": "/x"}),
            "Edited /x",
            false,
        )
        .await;
    assert!(matches!(action, HookAction::Continue));

    let action = hook
        .post_tool_use("Bash", "call-4", &json!({}), "ok", false)
        .await;
    assert!(matches!(action, HookAction::Continue));
}

#[tokio::test]
async fn verify_write_skips_when_is_error_true() {
    let hook = VerifyWriteHook::new();
    let action = hook
        .post_tool_use(
            "Write",
            "call-5",
            &json!({"file_path": "/nonexistent", "content": "x"}),
            "err",
            true,
        )
        .await;
    assert!(matches!(action, HookAction::Continue));
}

#[tokio::test]
async fn verify_write_injects_when_file_missing_after_successful_write() {
    let hook = VerifyWriteHook::new();
    let input = json!({ "file_path": "/nonexistent/does/not/exist.txt", "content": "x" });
    let action = hook
        .post_tool_use("Write", "call-6", &input, "wrote", false)
        .await;
    let text = injected_text(&action).expect("expected InjectMessage when re-read fails");
    assert!(text.contains("could not be re-read"));
}
