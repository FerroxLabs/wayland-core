//! `ApprovalScope::AlwaysPath` must reach the layer that owns filesystem
//! authority — and must degrade harmlessly when it cannot.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use wcore_protocol::commands::ApprovalScope;
use wcore_protocol::events::ToolCategory;
use wcore_protocol::{PathGrantSink, ToolApprovalManager, ToolApprovalResult};

#[derive(Default)]
struct RecordingSink {
    seen: Mutex<Vec<(PathBuf, bool)>>,
    accept: bool,
}

impl PathGrantSink for RecordingSink {
    fn grant_path(&self, root: &Path, write: bool) -> bool {
        self.seen.lock().unwrap().push((root.to_path_buf(), write));
        self.accept
    }
}

#[tokio::test]
async fn an_approved_path_scope_reaches_the_grant_sink() {
    let manager = ToolApprovalManager::new();
    let sink = Arc::new(RecordingSink {
        accept: true,
        ..Default::default()
    });
    manager.set_path_grant_sink(sink.clone());

    let rx = manager.request_approval("c1", &ToolCategory::Info, "Read");
    manager.approve(
        "c1",
        ApprovalScope::AlwaysPath {
            root: "/srv/reports".to_string(),
            write: false,
        },
        None,
    );

    assert!(matches!(
        rx.await.unwrap(),
        ToolApprovalResult::Approved { .. }
    ));
    assert_eq!(
        sink.seen.lock().unwrap().as_slice(),
        &[(PathBuf::from("/srv/reports"), false)],
        "the root the user approved is the root the policy is asked to grant"
    );
}

#[tokio::test]
async fn the_write_flag_is_forwarded_verbatim_not_assumed() {
    let manager = ToolApprovalManager::new();
    let sink = Arc::new(RecordingSink {
        accept: false,
        ..Default::default()
    });
    manager.set_path_grant_sink(sink.clone());

    let rx = manager.request_approval("c2", &ToolCategory::Info, "Read");
    manager.approve(
        "c2",
        ApprovalScope::AlwaysPath {
            root: "/srv/out".to_string(),
            write: true,
        },
        None,
    );

    // The sink refused, but the ACT the user approved still happened. A
    // refused standing grant must never turn into a refused tool call.
    assert!(matches!(
        rx.await.unwrap(),
        ToolApprovalResult::Approved { .. }
    ));
    assert_eq!(
        sink.seen.lock().unwrap().as_slice(),
        &[(PathBuf::from("/srv/out"), true)]
    );
}

#[tokio::test]
async fn with_no_sink_installed_a_path_scope_is_simply_a_once() {
    // An engine built without a workspace policy has nothing that owns
    // filesystem authority. The approval must still resolve — silently
    // hanging or denying would be a worse failure than not persisting.
    let manager = ToolApprovalManager::new();
    let rx = manager.request_approval("c3", &ToolCategory::Info, "Read");
    manager.approve(
        "c3",
        ApprovalScope::AlwaysPath {
            root: "/srv/reports".to_string(),
            write: false,
        },
        None,
    );
    assert!(matches!(
        rx.await.unwrap(),
        ToolApprovalResult::Approved { .. }
    ));
}

#[tokio::test]
async fn the_host_resolve_path_is_not_a_side_door() {
    // `resolve_host` is the REST/ACP entry point. It must apply the same
    // scope handling as `approve` — a second code path that skipped the sink
    // (or skipped its refusal) would be exactly the kind of drift the
    // duplicated `Always` arms already invite.
    let manager = ToolApprovalManager::new();
    let sink = Arc::new(RecordingSink {
        accept: true,
        ..Default::default()
    });
    manager.set_path_grant_sink(sink.clone());

    let rx = manager.request_approval("c4", &ToolCategory::Info, "Read");
    assert!(manager.resolve_host(
        "c4",
        true,
        ApprovalScope::AlwaysPath {
            root: "/srv/host".to_string(),
            write: false,
        },
        None,
    ));
    assert!(matches!(
        rx.await.unwrap(),
        ToolApprovalResult::Approved { .. }
    ));
    assert_eq!(
        sink.seen.lock().unwrap().as_slice(),
        &[(PathBuf::from("/srv/host"), false)]
    );
}

#[tokio::test]
async fn a_denied_path_scope_grants_nothing() {
    let manager = ToolApprovalManager::new();
    let sink = Arc::new(RecordingSink {
        accept: true,
        ..Default::default()
    });
    manager.set_path_grant_sink(sink.clone());

    let rx = manager.request_approval("c5", &ToolCategory::Info, "Read");
    assert!(manager.resolve_host(
        "c5",
        false,
        ApprovalScope::AlwaysPath {
            root: "/srv/nope".to_string(),
            write: false,
        },
        None,
    ));

    assert!(matches!(
        rx.await.unwrap(),
        ToolApprovalResult::Denied { .. }
    ));
    assert!(
        sink.seen.lock().unwrap().is_empty(),
        "saying no must not mint the grant the scope field asked for"
    );
}
