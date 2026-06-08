//! W6 F17 — AuditLog::recent_tool_uses smoke + edge tests.

use std::collections::HashMap;

use wcore_memory::audit::{AuditEntry, AuditLog, now_secs};
use wcore_memory::v2_types::{Partition, Tier};

fn rec(op: &str) -> AuditEntry {
    AuditEntry {
        ts: now_secs(),
        token_kind: "tool_use".into(),
        agent_name: None,
        partition: Partition::Episodic,
        tier: Tier::Project,
        op: op.into(),
        decision: "allow".into(),
        reason: "test".into(),
    }
}

#[test]
fn recent_tool_uses_counts_within_window() {
    let log = AuditLog::open_memory().unwrap();
    log.record(rec("Read")).unwrap();
    log.record(rec("Read")).unwrap();
    log.record(rec("Grep")).unwrap();

    let recent: HashMap<String, u64> = log.recent_tool_uses(3600).unwrap();
    assert_eq!(recent.get("Read").copied(), Some(2));
    assert_eq!(recent.get("Grep").copied(), Some(1));
}

#[test]
fn recent_tool_uses_empty_on_fresh_log() {
    let log = AuditLog::open_memory().unwrap();
    let recent = log.recent_tool_uses(3600).unwrap();
    assert!(recent.is_empty());
}

#[test]
fn recent_tool_uses_excludes_entries_before_window() {
    let log = AuditLog::open_memory().unwrap();
    // Synthesize an "old" entry by writing ts far in the past.
    let mut old = rec("Bash");
    old.ts = now_secs() - 10_000; // 10_000s ago
    log.record(old).unwrap();
    log.record(rec("Read")).unwrap();

    let recent = log.recent_tool_uses(60).unwrap(); // only last 60s
    assert!(!recent.contains_key("Bash"), "old entry must be excluded");
    assert_eq!(recent.get("Read").copied(), Some(1));
}
