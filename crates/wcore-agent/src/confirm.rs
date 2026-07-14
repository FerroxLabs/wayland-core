use std::collections::HashSet;
use std::io::{self, BufRead, IsTerminal, Write};

use wcore_protocol::events::ToolCategory;
use wcore_types::execution_policy::ApprovalPolicy;

pub struct ToolConfirmer {
    approval_policy: ApprovalPolicy,
    allow_list: HashSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmResult {
    Approved,
    Denied,
    Quit,
}

impl ToolConfirmer {
    pub fn new(auto_approve: bool, allow_list: Vec<String>) -> Self {
        let policy = if auto_approve {
            ApprovalPolicy::Bypass
        } else {
            ApprovalPolicy::Prompt
        };
        Self::with_policy(policy, allow_list)
    }

    /// Construct a confirmer from the typed Smart approval policy.
    pub fn with_policy(policy: ApprovalPolicy, allow_list: Vec<String>) -> Self {
        Self {
            approval_policy: policy,
            allow_list: allow_list.into_iter().collect(),
        }
    }

    /// Returns whether auto-approve is enabled
    pub fn is_auto_approve(&self) -> bool {
        self.approval_policy == ApprovalPolicy::Bypass
    }

    /// Return the typed approval policy active for this session.
    pub fn approval_policy(&self) -> ApprovalPolicy {
        self.approval_policy
    }

    /// Returns whether this call would require an interactive decision.
    /// Callers use this before `check` to bind an approval to the exact
    /// arguments that were shown to the user.
    pub fn requires_confirmation(&self, tool_name: &str) -> bool {
        self.requires_confirmation_for(tool_name, ToolCategory::Exec)
    }

    /// Category-aware confirmation predicate used by the production
    /// dispatcher. AutoEdit is intentionally narrower than the protocol's
    /// historical category-wide shortcut: only the built-in file Write/Edit
    /// tools are auto-approved. Other Edit/Info tools can mutate remote or
    /// durable state and therefore still require a decision.
    pub fn requires_confirmation_for(&self, tool_name: &str, category: ToolCategory) -> bool {
        if tool_name == "AskUserQuestion" {
            return true;
        }
        if self.allow_list.contains(tool_name) {
            return false;
        }

        match self.approval_policy {
            ApprovalPolicy::Prompt => true,
            ApprovalPolicy::AutoEdit => {
                !(category == ToolCategory::Edit && matches!(tool_name, "Write" | "Edit"))
            }
            ApprovalPolicy::Bypass => false,
        }
    }

    /// Whether approval authority is scoped to the exact input currently
    /// being dispatched. Interactive decisions and AutoEdit's narrow file
    /// grant are input-bound, so hooks cannot mutate approved arguments into a
    /// different operation. Blanket Bypass and explicit tool-name allow-list
    /// grants are deliberately unbound.
    pub fn approval_is_input_bound(&self, tool_name: &str) -> bool {
        if tool_name == "AskUserQuestion" {
            return true;
        }
        if self.allow_list.contains(tool_name) {
            return false;
        }

        match self.approval_policy {
            ApprovalPolicy::Prompt | ApprovalPolicy::AutoEdit => true,
            ApprovalPolicy::Bypass => false,
        }
    }

    /// Add a tool name to the allow list at runtime.
    /// Used by skill context modifiers to grant auto-approval for specified tools.
    pub fn add_to_allow_list(&mut self, name: &str) {
        self.allow_list.insert(name.to_string());
    }

    /// Check if the tool needs confirmation. Returns the user's decision.
    pub fn check(&mut self, tool_name: &str, tool_input_display: &str) -> ConfirmResult {
        self.check_for(tool_name, ToolCategory::Exec, tool_input_display)
    }

    /// Category-aware confirmation check used by the production dispatcher.
    pub fn check_for(
        &mut self,
        tool_name: &str,
        category: ToolCategory,
        tool_input_display: &str,
    ) -> ConfirmResult {
        if !self.requires_confirmation_for(tool_name, category) {
            return ConfirmResult::Approved;
        }

        // No interactive terminal — a daemon, a piped invocation, or a
        // channel-driven turn (the inbound subscriber runs turns with no
        // TTY). There is no human to answer the prompt, and a blocking
        // `read_line` on a stdin that never reaches EOF (e.g. a held-open
        // pipe keeping a daemon alive) would hang the turn forever. Fail
        // closed: a tool that needs confirmation but cannot get it is denied.
        // Auto-approve and allow-listed tools are already handled above, so
        // this only gates tools that would otherwise prompt.
        if !io::stdin().is_terminal() {
            tracing::debug!(
                target: "wcore_agent::confirm",
                tool = %tool_name,
                "tool needs confirmation but stdin is not a terminal; denying (no interactive approver)"
            );
            return ConfirmResult::Denied;
        }

        eprint!(
            "\n[tool] {}({})\nAllow? [y]es / [n]o / [a]lways / [q]uit > ",
            tool_name, tool_input_display
        );
        // SAFETY: flushing stderr can fail only if stderr is closed
        // (e.g. parent piped to a sink that disconnected). The very
        // next `read_line` on stdin would also fail in that scenario
        // and bail with `Denied`, so a panic here would simply
        // accelerate the same outcome by one cycle. Keeping the
        // panic preserves the existing "abort if I/O is hosed"
        // semantics for interactive callers.
        let _ = io::stderr().flush();

        let mut input = String::new();
        if io::stdin().lock().read_line(&mut input).is_err() {
            return ConfirmResult::Denied;
        }

        match input.trim().to_lowercase().as_str() {
            "y" | "yes" | "" => ConfirmResult::Approved,
            "a" | "always" => {
                self.allow_list.insert(tool_name.to_string());
                ConfirmResult::Approved
            }
            "q" | "quit" => ConfirmResult::Quit,
            _ => ConfirmResult::Denied,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_approve_always_allows() {
        let mut confirmer = ToolConfirmer::new(true, vec![]);
        assert_eq!(
            confirmer.check("Bash", "echo hello"),
            ConfirmResult::Approved
        );
        assert_eq!(
            confirmer.check("Read", "/tmp/file"),
            ConfirmResult::Approved
        );
        assert_eq!(
            confirmer.check("Write", "/tmp/out"),
            ConfirmResult::Approved
        );
    }

    #[test]
    fn test_allowlist_contains_tool() {
        let mut confirmer = ToolConfirmer::new(false, vec!["Read".into(), "Write".into()]);
        assert_eq!(
            confirmer.check("Read", "/tmp/file"),
            ConfirmResult::Approved
        );
        assert_eq!(
            confirmer.check("Write", "/tmp/out"),
            ConfirmResult::Approved
        );
    }

    #[test]
    fn test_allowlist_approves_even_when_auto_approve_is_false() {
        let mut confirmer = ToolConfirmer::new(false, vec!["Read".into()]);
        assert_eq!(
            confirmer.check("Read", "/some/path"),
            ConfirmResult::Approved
        );
    }

    // Phase 6: add_to_allow_list() grants runtime approval
    #[test]
    fn test_add_to_allow_list_grants_approval() {
        let mut confirmer = ToolConfirmer::new(false, vec![]);
        // Before: tool not in list (would prompt — skip interactive check, just verify membership)
        confirmer.add_to_allow_list("Write");
        // After: auto-approved without interactive prompt
        assert_eq!(
            confirmer.check("Write", "file.txt"),
            ConfirmResult::Approved
        );
    }

    // Phase 6: add_to_allow_list() is idempotent — adding twice has no bad effect
    #[test]
    fn test_add_to_allow_list_idempotent() {
        let mut confirmer = ToolConfirmer::new(false, vec![]);
        confirmer.add_to_allow_list("Bash");
        confirmer.add_to_allow_list("Bash"); // duplicate — HashSet, no panic
        assert_eq!(confirmer.check("Bash", "echo hi"), ConfirmResult::Approved);
    }

    // Security audit H-7 / M-9 regression: a confirmer built from a parent
    // posture of `auto_approve=false` with a read-only allow_list must NOT
    // short-circuit destructive tools (Bash/Write/Edit) to Approved. We assert
    // the short-circuit PREDICATE directly (`is_auto_approve()` + allow_list
    // membership) rather than calling `check()`, because the non-approved path
    // blocks on interactive stdin. With the spawner fix, the sub-agent engine
    // builds exactly such a confirmer, so destructive tools reach the prompt.
    #[test]
    fn test_inherited_no_auto_approve_does_not_short_circuit_destructive() {
        let confirmer = ToolConfirmer::new(false, vec!["Read".into(), "Grep".into()]);
        // Read-only tools the parent allow-listed remain auto-approved.
        assert!(confirmer.allow_list.contains("Read"));
        assert!(confirmer.allow_list.contains("Grep"));
        // Destructive tools are NOT auto-approved and NOT on the allow_list, so
        // `check()` would fall through to the interactive prompt (not Approved).
        assert!(
            !confirmer.is_auto_approve(),
            "inherited posture must not be auto-approve"
        );
        for destructive in ["Bash", "Write", "Edit"] {
            assert!(
                !confirmer.allow_list.contains(destructive),
                "destructive tool '{destructive}' must not be silently approved (H-7)"
            );
        }
    }

    // Phase 6: add_to_allow_list() does not affect unrelated tools
    #[test]
    fn test_add_to_allow_list_does_not_affect_other_tools() {
        let mut confirmer = ToolConfirmer::new(false, vec![]);
        confirmer.add_to_allow_list("Read");
        // Write is not in the list — check returns non-Approved for non-interactive
        // (we cannot test interactive input; verify Read is approved and Write is not in list)
        assert_eq!(confirmer.check("Read", "file.txt"), ConfirmResult::Approved);
        // We can't test the Denied path without stdin, but we verify allow_list state:
        assert!(confirmer.allow_list.contains("Read"));
        assert!(!confirmer.allow_list.contains("Write"));
    }

    #[test]
    fn typed_policy_confirmation_matrix_is_fail_closed() {
        let prompt = ToolConfirmer::with_policy(ApprovalPolicy::Prompt, vec![]);
        let auto_edit = ToolConfirmer::with_policy(ApprovalPolicy::AutoEdit, vec![]);
        let bypass = ToolConfirmer::with_policy(ApprovalPolicy::Bypass, vec![]);

        for category in [
            ToolCategory::Info,
            ToolCategory::Edit,
            ToolCategory::Exec,
            ToolCategory::Mcp,
        ] {
            assert!(prompt.requires_confirmation_for("AnyTool", category));
            assert!(!bypass.requires_confirmation_for("AnyTool", category));
        }

        assert!(!auto_edit.requires_confirmation_for("Write", ToolCategory::Edit));
        assert!(!auto_edit.requires_confirmation_for("Edit", ToolCategory::Edit));
        assert!(auto_edit.approval_is_input_bound("Write"));
        assert!(auto_edit.approval_is_input_bound("Edit"));

        // Category alone grants no authority: remote and durable-state tools
        // are classified Info/Edit too, so unknown names stay gated.
        assert!(auto_edit.requires_confirmation_for("Notion", ToolCategory::Edit));
        assert!(auto_edit.requires_confirmation_for("RecordEpisode", ToolCategory::Info));
        assert!(auto_edit.requires_confirmation_for("Bash", ToolCategory::Exec));
        assert!(auto_edit.requires_confirmation_for("McpTool", ToolCategory::Mcp));
    }

    #[test]
    fn ask_user_question_always_requires_a_host_response() {
        for policy in [
            ApprovalPolicy::Prompt,
            ApprovalPolicy::AutoEdit,
            ApprovalPolicy::Bypass,
        ] {
            let confirmer = ToolConfirmer::with_policy(policy, vec!["AskUserQuestion".into()]);
            assert!(confirmer.requires_confirmation_for("AskUserQuestion", ToolCategory::Info));
            assert!(confirmer.approval_is_input_bound("AskUserQuestion"));
        }
    }

    #[test]
    fn tool_name_allow_list_remains_deliberately_unbound() {
        let confirmer =
            ToolConfirmer::with_policy(ApprovalPolicy::Prompt, vec!["TrustedLocalTool".into()]);
        assert!(!confirmer.requires_confirmation_for("TrustedLocalTool", ToolCategory::Exec));
        assert!(!confirmer.approval_is_input_bound("TrustedLocalTool"));
    }
}
