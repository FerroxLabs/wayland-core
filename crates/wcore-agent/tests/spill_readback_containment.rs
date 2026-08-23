//! FerroxLabs/wayland#1097 — the engine spills an oversized tool result to a
//! file and then tells the model "Full output saved to: <path>". A path the
//! same session cannot read back is the trap this issue is about: the write
//! succeeds, the work finishes, and the failure lands at the moment the result
//! was about to be delivered.
//!
//! These tests grade the DECISION and the READ-BACK together, because either
//! one alone passes while the trap is intact: a spill directory that satisfies
//! `WorkspacePolicy::readable_roots()` can still be refused by the session's
//! own file tools, which read through `ctx.vfs` (a `SandboxedFs` rooted at the
//! workspace) and do not carry the policy's readable extras or its writable
//! scratch tree.

use std::sync::Arc;

use wcore_agent::compact::degrade::shed_tool_outputs_until_under;
use wcore_tools::registry::ToolRegistry;
use wcore_tools::tool_result_storage::{
    BudgetConfig, PERSISTED_OUTPUT_TAG, StorageDir, maybe_persist_tool_result,
};
use wcore_tools::vfs::{RealFs, SandboxedFs, VirtualFs};
use wcore_tools::workspace_policy::WorkspacePolicy;
use wcore_types::message::{ContentBlock, Message, Role};

/// The jail the session's `Read`/`Grep`/`Glob` actually go through in a
/// `Workspace` posture — installed on the registry as `tool_vfs`
/// (`crates/wcore-tools/src/registry.rs`) and rooted at the workspace.
fn session_file_tools(workspace: &std::path::Path) -> SandboxedFs<RealFs> {
    SandboxedFs::new(RealFs, workspace)
}

fn tool_result(id: &str, content: &str) -> Message {
    Message::new(
        Role::User,
        vec![ContentBlock::ToolResult {
            tool_use_id: id.to_string(),
            content: content.to_string(),
            is_error: false,
        }],
    )
}

fn tool_use(id: &str) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse {
            id: id.to_string(),
            name: "Bash".to_string(),
            input: serde_json::json!({}),
            extra: None,
        }],
    )
}

fn spilled_path(message: &Message) -> std::path::PathBuf {
    let ContentBlock::ToolResult { content, .. } = &message.content[0] else {
        panic!("expected a tool result");
    };
    assert!(
        content.contains(PERSISTED_OUTPUT_TAG),
        "expected this result to have been spilled: {content}"
    );
    let line = content
        .lines()
        .find_map(|line| line.strip_prefix("Full output saved to: "))
        .expect("the persisted block must name the file it told the model to read");
    std::path::PathBuf::from(line.trim())
}

/// THE property. The directory the session spills into is one the same
/// session's file tools can open — proven by reading the spilled bytes back
/// through the real jail, not by asserting where the path is.
#[tokio::test]
async fn a_spilled_tool_result_is_readable_by_the_session_that_spilled_it() {
    let workspace = tempfile::tempdir().expect("workspace");
    let policy = Arc::new(WorkspacePolicy::contained(workspace.path()));

    let mut registry = ToolRegistry::new();
    registry.set_workspace_policy(Arc::clone(&policy));
    let storage = registry.spill_storage();

    let content = "x".repeat(60_000);
    let (replacement, _outcome) = maybe_persist_tool_result(
        &content,
        "Bash",
        "toolu_readback",
        &storage,
        &BudgetConfig::default(),
        None,
    );
    assert!(
        replacement.contains("Full output saved to:"),
        "a readable target must still be handed over: {replacement}"
    );

    let spill = storage.path_for("toolu_readback");
    let read_back = session_file_tools(workspace.path())
        .read(&spill)
        .await
        .unwrap_or_else(|err| {
            panic!(
                "the session was told to read {} and its own file tools refused: {err}",
                spill.display()
            )
        });
    assert_eq!(String::from_utf8(read_back).unwrap(), content);
}

/// CONTROL for the test above: the same read, against the location v0.13.5
/// spilled to (`StorageDir::os_default()` — `$TMPDIR/wayland-results`). It has
/// to be refused, otherwise the test above proves nothing about the jail.
#[tokio::test]
async fn the_host_temp_spill_target_is_refused_by_the_same_file_tools() {
    let workspace = tempfile::tempdir().expect("workspace");
    let host_temp = StorageDir::os_default();
    let spill = host_temp.path_for("toolu_1097_control");
    wcore_tools::tool_result_storage::write_spill_file(&spill, "full output").expect("write");

    let refusal = session_file_tools(workspace.path())
        .read(&spill)
        .await
        .expect_err("the host temp tree is outside the workspace jail");
    assert!(
        format!("{refusal}").contains("outside sandbox root"),
        "expected an out-of-jail refusal, got: {refusal}"
    );
    let _ = std::fs::remove_file(&spill);
}

/// The engine's own shed path (`AgentEngine::run_turn` -> this function, with
/// the registry-derived storage) end to end: every path it puts in front of
/// the model is readable by the session it hands them to.
#[tokio::test]
async fn every_path_the_shed_hands_the_model_is_readable_by_that_session() {
    let workspace = tempfile::tempdir().expect("workspace");
    let policy = Arc::new(WorkspacePolicy::contained(workspace.path()));
    let mut registry = ToolRegistry::new();
    registry.set_workspace_policy(Arc::clone(&policy));
    let storage = registry.spill_storage();

    let mut messages = vec![
        tool_use("a"),
        tool_result("a", &"x".repeat(50_000)),
        tool_use("b"),
        tool_result("b", &"y".repeat(30_000)),
    ];
    let estimate = |messages: &[Message]| -> u64 {
        messages
            .iter()
            .flat_map(|m| m.content.iter())
            .filter_map(|b| match b {
                ContentBlock::ToolResult { content, .. } => Some(content.chars().count() as u64),
                _ => None,
            })
            .sum()
    };

    let shed = shed_tool_outputs_until_under(
        &mut messages,
        &storage,
        &BudgetConfig::default(),
        8_000,
        40_000,
        estimate,
    );
    assert_eq!(shed, 1, "one shed should bring 80k under a 40k ceiling");

    let jail = session_file_tools(workspace.path());
    let path = spilled_path(&messages[1]);
    policy
        .ensure_write_target_readable(&path)
        .expect("the shed wrote outside this session's readable roots");
    let bytes = jail.read(&path).await.unwrap_or_else(|err| {
        panic!(
            "the shed told the model to read {} and the session's file tools refused: {err}",
            path.display()
        )
    });
    assert_eq!(bytes.len(), 50_000);
}

/// A session with no workspace policy has no jail on its read path either, so
/// the host temp default is not a trap for it — and staying there keeps spill
/// files out of a directory nobody asked us to write to.
#[test]
fn a_session_without_a_policy_keeps_the_host_temp_default() {
    let registry = ToolRegistry::new();
    assert_eq!(
        registry.spill_storage().path(),
        StorageDir::os_default().path()
    );
}
