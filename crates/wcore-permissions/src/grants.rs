//! #693 — durable always-allow grants: the bridge between the interactive
//! approval decision and the store in [`crate::learning`].
//!
//! ## What this closes
//!
//! `ToolApprovalManager` keeps every always-allow in memory
//! (`auto_approved_tool_names`, `auto_approved_prefixes`), so a user who
//! answered "always allow Bash `cargo `" was re-prompted for it on the next
//! launch. `LearnedPolicy` was a complete, tested, persistent store with no
//! caller on that path. This module is the caller.
//!
//! ## What is persisted, and what deliberately is not
//!
//! | `ApprovalScope`      | Persisted | Why |
//! |----------------------|-----------|-----|
//! | `Once`               | no        | the user scoped it to one act |
//! | `Always`             | yes       | whole-tool `AllowAlways`, no arg pattern |
//! | `AlwaysPrefix`       | yes       | `AllowAlways` prefix rule, keyed by tool category |
//! | `AlwaysPath`         | no        | see below |
//!
//! `AlwaysPath` EXPANDS the session's filesystem authority past the sandbox
//! root rather than narrowing an authority it already has, and the manager
//! deliberately stores none of it: containment has one source of truth, the
//! session's `WorkspacePolicy`, reached through `PathGrantSink`. That sink is
//! installed during agent bootstrap, strictly AFTER the host restores grants
//! at launch, so a replayed path grant would arrive with no sink and silently
//! degrade to `Once` — a persistence that looks like it works and does not.
//! Making path grants durable is a change to the containment store, not to
//! this one.
//!
//! Denials persist as `DenyAlways` and are honoured on restore, where they
//! BEAT a matching allow. No surface emits one today (the wire has no
//! always-deny scope), so this is the store refusing to be a one-way
//! authority-widening ratchet rather than a feature with a button.

use std::path::PathBuf;

use wcore_protocol::ToolApprovalManager;

use crate::learning::{LearnedDecision, LearnedPolicy, LearningError};

/// The durable always-allow store for one (policy file, workspace) pair.
///
/// Workspace-scoped because the file is per-PROFILE and therefore shared by
/// every checkout that profile opens: an unscoped rule would let one keypress
/// at one prompt authorise that tool everywhere the user ever works. The key
/// comes from [`LearnedPolicy::workspace_key`], which both sides call, so the
/// write and the restore cannot disagree about what "this workspace" means.
#[derive(Debug, Clone)]
pub struct LearnedGrants {
    path: PathBuf,
    workspace: String,
}

impl LearnedGrants {
    /// Bind the store to an explicit file and workspace key.
    pub fn new(path: impl Into<PathBuf>, workspace: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            workspace: workspace.into(),
        }
    }

    /// Persist a whole-tool always-allow (`ApprovalScope::Always`).
    pub fn record_tool_always(&self, tool_name: &str) -> Result<(), LearningError> {
        self.record_tool(tool_name, LearnedDecision::AllowAlways)
    }

    /// Persist a prefix-scoped always-allow (`ApprovalScope::AlwaysPrefix`),
    /// keyed by the tool CATEGORY the approval manager buckets it under.
    pub fn record_prefix_always(&self, category: &str, prefix: &str) -> Result<(), LearningError> {
        self.record_prefix(category, prefix, LearnedDecision::AllowAlways)
    }

    /// Persist a standing DENY for a whole tool. Honoured on restore ahead of
    /// any allow for the same tool.
    pub fn record_tool_deny(&self, tool_name: &str) -> Result<(), LearningError> {
        self.record_tool(tool_name, LearnedDecision::DenyAlways)
    }

    /// Persist a standing DENY for one prefix in a category. Honoured on
    /// restore ahead of any allow for the same (category, prefix).
    pub fn record_prefix_deny(&self, category: &str, prefix: &str) -> Result<(), LearningError> {
        self.record_prefix(category, prefix, LearnedDecision::DenyAlways)
    }

    fn record_tool(&self, tool_name: &str, decision: LearnedDecision) -> Result<(), LearningError> {
        // `update_at` holds the file's exclusive cross-process lock across the
        // READ and the write and publishes atomically, so a grant made at the
        // same moment by another session is neither lost nor half-written.
        LearnedPolicy::update_at(&self.path, |policy| {
            policy.record_in(tool_name, None, decision, &self.workspace)
        })
    }

    fn record_prefix(
        &self,
        category: &str,
        prefix: &str,
        decision: LearnedDecision,
    ) -> Result<(), LearningError> {
        LearnedPolicy::update_at(&self.path, |policy| {
            policy.record_prefix_in(category, prefix, decision, &self.workspace)
        })
    }

    /// Replay this workspace's standing grants into a freshly built
    /// `ToolApprovalManager`, so a decision made in an earlier session does not
    /// re-prompt.
    ///
    /// Fail-open on I/O is NOT an option here in the direction that matters:
    /// a file that exists but does not parse restores NOTHING and warns,
    /// because an operator with a malformed file must not silently get a
    /// different permission posture than the one they wrote. A missing file is
    /// an empty policy — the first-launch case, not an error.
    ///
    /// Only `AllowAlways` is replayed, and only when no `DenyAlways` covers the
    /// same key. `AllowOnce` / `DenyOnce` are session-lifetime by definition
    /// and are never standing authority.
    pub fn restore_into(&self, approval: &ToolApprovalManager) {
        let policy = match LearnedPolicy::load_from(&self.path) {
            Ok(policy) => policy,
            Err(error) => {
                tracing::warn!(
                    target: "wcore_permissions::grants",
                    path = %self.path.display(),
                    %error,
                    "learned policy failed to load; standing grants NOT restored"
                );
                return;
            }
        };

        // Whole-tool grants. A patterned rule (`git *`) is deliberately NOT
        // replayed as a whole-tool allow — that would widen it — and the
        // manager has no argv-pattern bucket to put it in.
        for (tool, rules) in policy.snapshot_in(&self.workspace) {
            let denied = rules.iter().any(|(pattern, decision)| {
                pattern.is_none() && matches!(decision, LearnedDecision::DenyAlways)
            });
            let allowed = rules.iter().any(|(pattern, decision)| {
                pattern.is_none() && matches!(decision, LearnedDecision::AllowAlways)
            });
            if allowed && !denied {
                approval.add_auto_approve_tool_name(&tool);
            }
        }

        // Prefix grants. Collect the denies first: a deny recorded after an
        // allow must win regardless of the order they sit in the file.
        let prefix_rules = policy.prefix_snapshot_in(&self.workspace);
        let denied: Vec<(&str, &str)> = prefix_rules
            .iter()
            .filter(|(_, _, decision)| matches!(decision, LearnedDecision::DenyAlways))
            .map(|(category, prefix, _)| (category.as_str(), prefix.as_str()))
            .collect();
        for (category, prefix, decision) in &prefix_rules {
            if !matches!(decision, LearnedDecision::AllowAlways) {
                continue;
            }
            if denied.contains(&(category.as_str(), prefix.as_str())) {
                continue;
            }
            approval.add_auto_approve_prefix(category, prefix);
        }
    }
}
