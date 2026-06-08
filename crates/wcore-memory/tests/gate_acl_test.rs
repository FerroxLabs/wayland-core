// W5 Task B.3 acceptance: MemoryAccessGate deny-by-default + audit log.

use std::sync::Arc;

use wcore_memory::audit::AuditLog;
use wcore_memory::error::MemoryError;
use wcore_memory::gate::{AccessPolicy, MemoryAccessGate};
use wcore_memory::v2_types::{AccessToken, Partition, Tier};

fn fresh_gate(policy: AccessPolicy) -> (MemoryAccessGate, Arc<AuditLog>) {
    let audit = Arc::new(AuditLog::open_memory().unwrap());
    let gate = MemoryAccessGate::new(audit.clone(), policy);
    (gate, audit)
}

#[test]
fn system_can_read_p5() {
    let (gate, _) = fresh_gate(AccessPolicy::empty());
    gate.check_read(&AccessToken::System, Partition::Core, Tier::Global)
        .unwrap();
}

#[test]
fn main_agent_cannot_read_p5() {
    let (gate, audit) = fresh_gate(AccessPolicy::empty());
    let err = gate
        .check_read(&AccessToken::MainAgent, Partition::Core, Tier::Global)
        .unwrap_err();
    assert!(matches!(err, MemoryError::AccessDenied { .. }));
    // Denial recorded.
    assert!(audit.count_denials().unwrap() >= 1);
}

#[test]
fn subagent_no_scope_cannot_read_p5() {
    let (gate, _) = fresh_gate(AccessPolicy::empty());
    let t = AccessToken::SubAgent {
        agent_name: "reviewer".into(),
    };
    let err = gate
        .check_read(&t, Partition::Core, Tier::Global)
        .unwrap_err();
    assert!(matches!(err, MemoryError::AccessDenied { .. }));
}

#[test]
fn subagent_with_scope_can_read_p2_project() {
    let mut policy = AccessPolicy::empty();
    policy.grant_read("reviewer", Partition::Episodic, Tier::Project);
    let (gate, _) = fresh_gate(policy);
    gate.check_read(
        &AccessToken::SubAgent {
            agent_name: "reviewer".into(),
        },
        Partition::Episodic,
        Tier::Project,
    )
    .unwrap();
}

#[test]
fn subagent_cannot_write_p5_even_with_scope() {
    let mut policy = AccessPolicy::empty();
    // Even an over-eager grant should fail — P5 write is hard-coded
    // System-only.
    policy.grant_write("attacker", Partition::Core, Tier::Global);
    let (gate, _) = fresh_gate(policy);
    let err = gate
        .check_write(
            &AccessToken::SubAgent {
                agent_name: "attacker".into(),
            },
            Partition::Core,
            Tier::Global,
        )
        .unwrap_err();
    assert!(matches!(err, MemoryError::AccessDenied { .. }));
    let msg = err.to_string();
    assert!(msg.contains("SystemToken"), "{msg}");
}

#[test]
fn every_denied_access_is_audited() {
    let (gate, audit) = fresh_gate(AccessPolicy::empty());
    // 3 denials.
    let _ = gate.check_read(&AccessToken::MainAgent, Partition::Core, Tier::Global);
    let _ = gate.check_write(
        &AccessToken::SubAgent {
            agent_name: "nope".into(),
        },
        Partition::Episodic,
        Tier::Project,
    );
    let _ = gate.check_read(
        &AccessToken::SubAgent {
            agent_name: "nope".into(),
        },
        Partition::Semantic,
        Tier::Global,
    );
    assert!(audit.count_denials().unwrap() >= 3);
}

#[test]
fn invalid_cell_rejected_before_policy() {
    // P1 + Project is invalid (P1 is session-only).
    let (gate, _) = fresh_gate(AccessPolicy::empty());
    let err = gate
        .check_write(&AccessToken::System, Partition::Working, Tier::Project)
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("invalid"), "{msg}");
}
