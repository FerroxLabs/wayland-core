use rstest::rstest;
use wcore_protocol::commands::ApprovalScope;
use wcore_protocol::events::ToolCategory;
use wcore_protocol::{ToolApprovalManager, ToolApprovalResult};

// W5.6 H-2: ApprovalScope::Always now scopes to tool NAME, not category.
// The `should_auto_approve` column reflects the NEW semantics:
//   - Once: no auto-approval (unchanged)
//   - Always: approves by tool name, NOT by category — is_auto_approved(category) is false
// Callers that need to verify Always-scope should use is_tool_name_auto_approved.
#[rstest]
#[case(ApprovalScope::Once, ToolCategory::Exec, "exec_tool", "exec", false)]
#[case(ApprovalScope::Always, ToolCategory::Edit, "edit_tool", "edit", false)]
#[tokio::test]
async fn approve_resolves_request_and_updates_auto_approval(
    #[case] scope: ApprovalScope,
    #[case] category: ToolCategory,
    #[case] tool_name: &str,
    #[case] category_name: &str,
    #[case] should_category_auto_approve: bool,
) {
    let manager = ToolApprovalManager::new();
    let rx = manager.request_approval("call-1", &category, tool_name);

    manager.approve("call-1", scope.clone(), None);

    let result = rx.await.expect("approval result should arrive");
    assert!(matches!(result, ToolApprovalResult::Approved { .. }));
    // Category-wide auto-approve is no longer set by Always scope (W5.6 H-2).
    assert_eq!(
        manager.is_auto_approved(category_name),
        should_category_auto_approve
    );
    // For Always scope, the tool name is registered instead.
    if scope == ApprovalScope::Always {
        assert!(
            manager.is_tool_name_auto_approved(tool_name),
            "Always scope must register the tool name for auto-approval"
        );
    }
}

#[tokio::test]
async fn resolve_preserves_denial_reason() {
    let manager = ToolApprovalManager::new();
    let rx = manager.request_approval("call-2", &ToolCategory::Exec, "exec_tool");

    manager.resolve(
        "call-2",
        ToolApprovalResult::Denied {
            reason: "policy violation".to_string(),
        },
    );

    let result = rx.await.expect("denial result should arrive");
    assert!(matches!(
        result,
        ToolApprovalResult::Denied { reason } if reason == "policy violation"
    ));
    assert!(!manager.is_auto_approved("exec"));
}

// Blocker #2 — `resolve_host` reports presence so the ACP/REST endpoint can
// answer 200-resolved vs 404-not-found and stay idempotent.

#[tokio::test]
async fn resolve_host_approves_pending_and_reports_presence() {
    let manager = ToolApprovalManager::new();
    let rx = manager.request_approval("call-host-1", &ToolCategory::Exec, "Bash");

    let resolved = manager.resolve_host("call-host-1", true, ApprovalScope::Once, None);
    assert!(resolved, "a pending entry must report resolved=true");

    let result = rx.await.expect("approval result should arrive");
    assert!(matches!(
        result,
        ToolApprovalResult::Approved { answer: None }
    ));
}

#[tokio::test]
async fn resolve_host_denies_pending_with_host_reason() {
    let manager = ToolApprovalManager::new();
    let rx = manager.request_approval("call-host-2", &ToolCategory::Exec, "Bash");

    let resolved = manager.resolve_host("call-host-2", false, ApprovalScope::Once, None);
    assert!(resolved);

    let result = rx.await.expect("denial result should arrive");
    assert!(matches!(
        result,
        ToolApprovalResult::Denied { reason } if reason == "denied by host"
    ));
}

#[tokio::test]
async fn resolve_host_always_scope_registers_tool_name() {
    let manager = ToolApprovalManager::new();
    let rx = manager.request_approval("call-host-3", &ToolCategory::Exec, "Bash");

    assert!(manager.resolve_host("call-host-3", true, ApprovalScope::Always, None));
    let _ = rx.await.expect("approval result should arrive");

    // Always scope must persist auto-approval by tool NAME (W5.6 H-2), not
    // category — same semantics as `approve`.
    assert!(manager.is_tool_name_auto_approved("Bash"));
    assert!(!manager.is_auto_approved("exec"));
}

#[tokio::test]
async fn resolve_host_threads_answer_through() {
    let manager = ToolApprovalManager::new();
    let rx = manager.request_approval("call-host-4", &ToolCategory::Exec, "AskUserQuestion");

    assert!(manager.resolve_host(
        "call-host-4",
        true,
        ApprovalScope::Once,
        Some("option B".to_string()),
    ));

    let result = rx.await.expect("approval result should arrive");
    assert!(matches!(
        result,
        ToolApprovalResult::Approved { answer: Some(a) } if a == "option B"
    ));
}

#[test]
fn resolve_host_unknown_call_id_reports_not_found_and_is_idempotent() {
    let manager = ToolApprovalManager::new();
    // Never registered → not found.
    assert!(!manager.resolve_host("ghost", true, ApprovalScope::Once, None));

    // Register, resolve once (presence), then a SECOND resolve is a clean
    // no-op returning false — the idempotency contract the REST 200/404
    // mapping relies on. No panic on the double resolve.
    let _rx = manager.request_approval("call-host-5", &ToolCategory::Exec, "Bash");
    assert!(manager.resolve_host("call-host-5", true, ApprovalScope::Once, None));
    assert!(
        !manager.resolve_host("call-host-5", true, ApprovalScope::Once, None),
        "second resolution of the same id must report not-found, not re-fire"
    );
}
