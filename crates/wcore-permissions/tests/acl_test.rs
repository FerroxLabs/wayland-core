//! Integration tests for the public `PolicyEngine` ACL contract.

use wcore_permissions::{Action, Actor, DenyReason, Permission, PolicyEngine, Resource};

#[test]
fn user_can_invoke_allowed_tool() {
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::User("alice".into()),
        resource: Resource::Tool("Read".into()),
        action: Action::Invoke,
    });
    assert!(
        engine
            .check(
                &Actor::User("alice".into()),
                &Resource::Tool("Read".into()),
                Action::Invoke,
            )
            .is_ok()
    );
}

#[test]
fn unknown_actor_is_denied_with_no_matching_grant() {
    let engine = PolicyEngine::new();
    let res = engine.check(
        &Actor::User("mallory".into()),
        &Resource::Tool("Read".into()),
        Action::Invoke,
    );
    assert_eq!(res.unwrap_err(), DenyReason::NoMatchingGrant);
}

#[test]
fn user_with_grant_on_different_tool_is_denied() {
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::User("alice".into()),
        resource: Resource::Tool("Read".into()),
        action: Action::Invoke,
    });
    let res = engine.check(
        &Actor::User("alice".into()),
        &Resource::Tool("Write".into()),
        Action::Invoke,
    );
    assert_eq!(res.unwrap_err(), DenyReason::NoMatchingGrant);
}

#[test]
fn action_mismatch_denies() {
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::User("alice".into()),
        resource: Resource::Tool("Read".into()),
        action: Action::Invoke,
    });
    let res = engine.check(
        &Actor::User("alice".into()),
        &Resource::Tool("Read".into()),
        Action::Delete,
    );
    assert_eq!(res.unwrap_err(), DenyReason::NoMatchingGrant);
}

#[test]
fn agent_denied_writing_outside_workspace_glob() {
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::Agent("worker-1".into()),
        resource: Resource::File("/tmp/workspace/**".into()),
        action: Action::Write,
    });
    let res = engine.check(
        &Actor::Agent("worker-1".into()),
        &Resource::File("/etc/passwd".into()),
        Action::Write,
    );
    assert_eq!(res.unwrap_err(), DenyReason::PathNotInAllowlist);
}

#[test]
fn agent_allowed_writing_inside_workspace_glob() {
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::Agent("worker-1".into()),
        resource: Resource::File("/tmp/workspace/**".into()),
        action: Action::Write,
    });
    assert!(
        engine
            .check(
                &Actor::Agent("worker-1".into()),
                &Resource::File("/tmp/workspace/output.txt".into()),
                Action::Write,
            )
            .is_ok()
    );
}

#[test]
fn agent_allowed_writing_at_glob_root() {
    // `/<dir>/**` includes `<dir>` itself, per `glob_match` contract.
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::Agent("worker-1".into()),
        resource: Resource::File("/tmp/workspace/**".into()),
        action: Action::Write,
    });
    assert!(
        engine
            .check(
                &Actor::Agent("worker-1".into()),
                &Resource::File("/tmp/workspace".into()),
                Action::Write,
            )
            .is_ok()
    );
}

#[test]
fn system_actor_bypasses_acl() {
    let engine = PolicyEngine::new();
    assert!(
        engine
            .check(
                &Actor::System,
                &Resource::Tool("Any".into()),
                Action::Invoke,
            )
            .is_ok()
    );
    assert!(
        engine
            .check(
                &Actor::System,
                &Resource::File("/etc/passwd".into()),
                Action::Delete,
            )
            .is_ok()
    );
}

#[test]
fn mcp_server_grant_is_exact_match() {
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::User("alice".into()),
        resource: Resource::McpServer("github".into()),
        action: Action::Invoke,
    });
    assert!(
        engine
            .check(
                &Actor::User("alice".into()),
                &Resource::McpServer("github".into()),
                Action::Invoke,
            )
            .is_ok()
    );
    let denied = engine.check(
        &Actor::User("alice".into()),
        &Resource::McpServer("gitlab".into()),
        Action::Invoke,
    );
    assert_eq!(denied.unwrap_err(), DenyReason::NoMatchingGrant);
}

#[test]
fn memory_resource_is_exact_match_on_tier_name() {
    let mut engine = PolicyEngine::new();
    engine.grant(Permission {
        actor: Actor::Agent("planner".into()),
        resource: Resource::Memory("episodic".into()),
        action: Action::Read,
    });
    assert!(
        engine
            .check(
                &Actor::Agent("planner".into()),
                &Resource::Memory("episodic".into()),
                Action::Read,
            )
            .is_ok()
    );
}

#[test]
fn matrix_user_agent_system_x_tool_file_mcp() {
    // Coverage matrix: every actor kind crossed with every resource kind,
    // with explicit grants for the two non-system actors and implicit bypass
    // for `System`. All 9 combinations must allow.
    let mut engine = PolicyEngine::new();
    let actors = [
        Actor::User("alice".into()),
        Actor::Agent("agent-1".into()),
        Actor::System,
    ];
    let resources = [
        Resource::Tool("Read".into()),
        Resource::File("/tmp/x".into()),
        Resource::McpServer("github".into()),
    ];
    for a in &actors[..2] {
        for r in &resources {
            engine.grant(Permission {
                actor: a.clone(),
                resource: r.clone(),
                action: Action::Invoke,
            });
        }
    }
    for a in &actors {
        for r in &resources {
            let res = engine.check(a, r, Action::Invoke);
            assert!(res.is_ok(), "{a:?} -> {r:?} should be allowed");
        }
    }
}

#[test]
fn empty_engine_is_empty() {
    let engine = PolicyEngine::new();
    assert!(engine.is_empty());
    assert_eq!(engine.len(), 0);
}

#[test]
fn len_tracks_grants() {
    let mut engine = PolicyEngine::new();
    assert_eq!(engine.len(), 0);
    engine.grant(Permission {
        actor: Actor::User("alice".into()),
        resource: Resource::Tool("Read".into()),
        action: Action::Invoke,
    });
    assert_eq!(engine.len(), 1);
    engine.grant(Permission {
        actor: Actor::User("bob".into()),
        resource: Resource::Tool("Read".into()),
        action: Action::Invoke,
    });
    assert_eq!(engine.len(), 2);
}
