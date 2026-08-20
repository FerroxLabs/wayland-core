//! End to end for the pre-flight escalation prompt (#1099).
//!
//! The dead end this closes: a `Read` of a path outside the workspace ran, hit
//! `VfsError::OutsideSandbox`, came back as a tool error, and the model had to
//! improvise. Here the real `ReadTool` runs behind the real `SandboxedFs` jail
//! and the real `ToolApprovalManager`, and the whole loop is exercised — the
//! call gates, the host answers `always_path` with the root Core suggested, the
//! read succeeds, and a SIBLING file in the same folder does not prompt again.

use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio_util::sync::CancellationToken;
use wcore_agent::orchestration::execute_tool_calls_with_approval;
use wcore_compact::CompactionLevel;
use wcore_protocol::events::{ProtocolEvent, ToolEscalation};
use wcore_protocol::writer::ProtocolEmitter;
use wcore_protocol::{
    PathGrantSink, ToolApprovalManager,
    commands::{ApprovalScope, SessionMode},
};
use wcore_tools::registry::ToolRegistry;
use wcore_tools::vfs::{RealFs, SandboxedFs};
use wcore_tools::workspace_policy::WorkspacePolicy;
use wcore_types::message::ContentBlock;

/// Answers every `tool_request` by taking Core at its word: if the card names a
/// path boundary, grant exactly the folder Core suggested.
struct GrantingHost {
    events: Mutex<Vec<ProtocolEvent>>,
    manager: Arc<ToolApprovalManager>,
    approve: bool,
}

impl ProtocolEmitter for GrantingHost {
    fn emit(&self, event: &ProtocolEvent) -> std::io::Result<()> {
        self.events.lock().unwrap().push(event.clone());
        if let ProtocolEvent::ToolRequest { call_id, tool, .. } = event {
            let scope = match (&tool.escalation, self.approve) {
                (Some(ToolEscalation::PathBoundary { suggested_root, .. }), true) => {
                    ApprovalScope::AlwaysPath {
                        root: suggested_root.clone(),
                        write: false,
                    }
                }
                _ => ApprovalScope::Once,
            };
            self.manager
                .resolve_host(call_id, self.approve, scope, None);
        }
        Ok(())
    }
}

impl GrantingHost {
    fn requests(&self) -> Vec<ProtocolEvent> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| matches!(e, ProtocolEvent::ToolRequest { .. }))
            .cloned()
            .collect()
    }
}

struct Fixture {
    registry: ToolRegistry,
    manager: Arc<ToolApprovalManager>,
    policy: Arc<WorkspacePolicy>,
    _workspace: tempfile::TempDir,
    outside: tempfile::TempDir,
}

/// A genuinely-local contained session, wired exactly as `AgentBootstrap`
/// wires one: jail rooted at the workspace, live grant handle shared with the
/// jail, and the policy installed as the approval manager's grant sink.
fn fixture() -> Fixture {
    let workspace = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("brief.md"), b"quarterly numbers").unwrap();
    std::fs::write(outside.path().join("sibling.md"), b"appendix numbers").unwrap();

    let policy =
        Arc::new(WorkspacePolicy::contained(workspace.path()).with_local_operator_principal());
    let jail = SandboxedFs::new(RealFs, workspace.path().to_path_buf())
        .with_read_grants(policy.session_read_grant_handle());

    let mut registry = ToolRegistry::new();
    registry.register(Box::new(wcore_tools::read::ReadTool::new(None)));
    registry.set_tool_vfs(Arc::new(jail));
    registry.set_workspace_policy(Arc::clone(&policy));

    let manager = Arc::new(ToolApprovalManager::new());
    let sink: Arc<dyn PathGrantSink> = Arc::clone(&policy) as Arc<dyn PathGrantSink>;
    manager.set_path_grant_sink(sink);

    Fixture {
        registry,
        manager,
        policy,
        _workspace: workspace,
        outside,
    }
}

fn read_call(id: &str, path: &std::path::Path) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.into(),
        name: "Read".into(),
        input: json!({ "file_path": path }),
        extra: None,
    }
}

async fn run(
    fixture: &Fixture,
    host: &Arc<dyn ProtocolEmitter>,
    call: ContentBlock,
) -> ContentBlock {
    // "Read" is on the allow-list on purpose: an allow-list grants the TOOL,
    // and must not be able to wave through a path the session cannot reach.
    execute_tool_calls_with_approval(
        &fixture.registry,
        &[call],
        &fixture.manager,
        host,
        "msg-1",
        &["Read".to_string()],
        None,
        CompactionLevel::Off,
        false,
        &CancellationToken::new(),
        None,
    )
    .await
    .expect("dispatch must not abort the turn")
    .results
    .into_iter()
    .next()
    .expect("one result")
}

fn result_text(block: &ContentBlock) -> (&str, bool) {
    match block {
        ContentBlock::ToolResult {
            content, is_error, ..
        } => (content.as_str(), *is_error),
        other => panic!("expected a tool result, got {other:?}"),
    }
}

#[tokio::test]
async fn an_out_of_workspace_read_prompts_grants_and_then_succeeds() {
    let fixture = fixture();
    let host = Arc::new(GrantingHost {
        events: Mutex::new(Vec::new()),
        manager: Arc::clone(&fixture.manager),
        approve: true,
    });
    let emitter: Arc<dyn ProtocolEmitter> = host.clone();

    let first = run(
        &fixture,
        &emitter,
        read_call("t1", &fixture.outside.path().join("brief.md")),
    )
    .await;

    let requests = host.requests();
    assert_eq!(
        requests.len(),
        1,
        "the boundary must force the gate even though Read is allow-listed"
    );
    let ProtocolEvent::ToolRequest { tool, .. } = &requests[0] else {
        unreachable!()
    };
    let Some(ToolEscalation::PathBoundary {
        target,
        suggested_root,
        ..
    }) = &tool.escalation
    else {
        panic!("the card must carry the boundary as structured data, not prose");
    };
    assert_eq!(
        std::path::Path::new(suggested_root),
        std::fs::canonicalize(fixture.outside.path()).unwrap(),
        "the card names the FOLDER a grant opens, not the file"
    );
    assert!(target.ends_with("brief.md"));

    let (content, is_error) = result_text(&first);
    assert!(
        !is_error && content.contains("quarterly numbers"),
        "after the grant the read must actually succeed: {content}"
    );

    // The DoD's second half: a sibling in the SAME folder must not re-prompt.
    let second = run(
        &fixture,
        &emitter,
        read_call("t2", &fixture.outside.path().join("sibling.md")),
    )
    .await;
    assert_eq!(
        host.requests().len(),
        1,
        "'always allow this folder' has to mean the folder — a second prompt \
         for a sibling file is the dead end this replaced, one step later"
    );
    let (content, is_error) = result_text(&second);
    assert!(
        !is_error && content.contains("appendix numbers"),
        "{content}"
    );
}

#[tokio::test]
async fn denying_the_card_grants_nothing() {
    let fixture = fixture();
    let host = Arc::new(GrantingHost {
        events: Mutex::new(Vec::new()),
        manager: Arc::clone(&fixture.manager),
        approve: false,
    });
    let emitter: Arc<dyn ProtocolEmitter> = host.clone();

    let result = run(
        &fixture,
        &emitter,
        read_call("t1", &fixture.outside.path().join("brief.md")),
    )
    .await;

    assert_eq!(host.requests().len(), 1);
    let (content, is_error) = result_text(&result);
    assert!(is_error, "a denied call must not read the file: {content}");
    assert!(
        fixture.policy.session_read_grant_roots().is_empty(),
        "a refusal must leave no standing grant behind"
    );
}

#[tokio::test]
async fn force_mode_never_raises_the_card() {
    let fixture = fixture();
    fixture.manager.set_mode(SessionMode::Force);
    let host = Arc::new(GrantingHost {
        events: Mutex::new(Vec::new()),
        manager: Arc::clone(&fixture.manager),
        approve: true,
    });
    let emitter: Arc<dyn ProtocolEmitter> = host.clone();

    let result = run(
        &fixture,
        &emitter,
        read_call("t1", &fixture.outside.path().join("brief.md")),
    )
    .await;

    assert!(
        host.requests().is_empty(),
        "the operator explicitly asked not to be asked; force mode must be \
         byte-for-byte what it was before this feature"
    );
    let (content, is_error) = result_text(&result);
    assert!(
        is_error,
        "and the read still fails at the jail, exactly as it does on main: {content}"
    );
    assert!(fixture.policy.session_read_grant_roots().is_empty());
}
