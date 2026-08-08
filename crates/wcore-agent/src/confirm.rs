use std::collections::HashSet;
use std::io::{self, BufRead, IsTerminal, Write};

use wcore_protocol::events::ToolCategory;
use wcore_types::execution_policy::ApprovalPolicy;

pub struct ToolConfirmer {
    approval_policy: ApprovalPolicy,
    allow_list: HashSet<String>,
    /// Whether there is an interactive approver this session can reach.
    ///
    /// Resolved once, at construction, from `io::stdin().is_terminal()`. A
    /// process's stdin cannot become a terminal mid-run, so caching it costs
    /// nothing and buys two things: the CLI and the confirmer stop probing
    /// the terminal independently (they can no longer disagree), and tests
    /// can pin the condition with `set_interactive_approver` instead of
    /// depending on whatever stdin the test runner happened to inherit.
    interactive_approver: bool,
    /// How many calls this confirmer refused for want of an approver.
    ///
    /// The CLI reads it once, after the run, to decide whether the process
    /// accomplished anything. Counted here rather than plumbed through the
    /// dispatcher because this is the only place the fact exists.
    no_approver_denials: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfirmResult {
    Approved,
    /// A human was asked and refused.
    Denied,
    /// The call was refused because no human could be asked: the run has no
    /// interactive approver, or the one it had disappeared before answering.
    /// A separate variant because the two are not the same fact and the
    /// message the operator and the model see must not claim they are.
    DeniedNoApprover,
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
            interactive_approver: io::stdin().is_terminal(),
            no_approver_denials: 0,
        }
    }

    /// How many tool calls were refused because no approver could be reached.
    pub fn no_approver_denials(&self) -> usize {
        self.no_approver_denials
    }

    /// Whether this session can reach an interactive approver.
    pub fn has_interactive_approver(&self) -> bool {
        self.interactive_approver
    }

    /// Override approver presence. Exists so tests can pin the condition
    /// without a terminal, and so a host that knows better than
    /// `is_terminal()` (Windows ConPTY, MSYS) can say so.
    pub fn set_interactive_approver(&mut self, present: bool) {
        self.interactive_approver = present;
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
        if !self.interactive_approver {
            tracing::debug!(
                target: "wcore_agent::confirm",
                tool = %tool_name,
                "tool needs confirmation but stdin is not a terminal; denying (no interactive approver)"
            );
            self.no_approver_denials += 1;
            return ConfirmResult::DeniedNoApprover;
        }

        self.prompt_and_decide(
            tool_name,
            tool_input_display,
            &mut io::stdin().lock(),
            &mut io::stderr(),
        )
    }

    /// Print the approval prompt on `prompt_out`, read one answer from
    /// `answers`, and decide.
    ///
    /// This is the whole interactive gate. `check_for` calls it with the
    /// process's real stdin/stderr; tests call it with in-memory handles so
    /// the decision for every possible answer — including "no answer ever
    /// arrives" — is observable without a terminal.
    fn prompt_and_decide<R: BufRead, W: Write>(
        &mut self,
        tool_name: &str,
        tool_input_display: &str,
        answers: &mut R,
        prompt_out: &mut W,
    ) -> ConfirmResult {
        let _ = write!(
            prompt_out,
            "\n[tool] {}({})\nAllow? [y]es / [n]o / [a]lways / [q]uit > ",
            tool_name, tool_input_display
        );
        // SAFETY: flushing can fail only if the sink is closed (e.g. parent
        // piped to something that disconnected). The very next read on
        // `answers` would also fail in that scenario and bail with `Denied`,
        // so ignoring the error simply defers the same outcome by one cycle.
        let _ = prompt_out.flush();

        let mut input = String::new();
        match answers.read_line(&mut input) {
            // End of input. NOT an error, and NOT an empty answer: nobody
            // ever answered. Falling through would hand `""` to the
            // `"y" | "yes" | ""` arm below — the bare-Enter default — and
            // APPROVE the call. Fail closed instead. This is the difference
            // between "the operator pressed Enter" and "the operator's
            // terminal went away", and only the byte count can tell them
            // apart.
            Ok(0) => {
                self.no_approver_denials += 1;
                return ConfirmResult::DeniedNoApprover;
            }
            Ok(_) => {}
            Err(_) => {
                self.no_approver_denials += 1;
                return ConfirmResult::DeniedNoApprover;
            }
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

    // ---- The interactive gate must fail CLOSED when no answer arrives ----
    //
    // `BufRead::read_line` returns `Ok(0)` at end-of-input — not an `Err` —
    // leaving the buffer empty, and the empty string is matched by the
    // `"y" | "yes" | ""` arm that exists so a bare Enter means yes. So
    // "the answering terminal went away" was decided as "the operator
    // pressed Enter" and the call was APPROVED. Reachable whenever the
    // answering side disappears: an ssh disconnect mid-run, a harness that
    // writes its answer and then closes the pty master (macOS surfaces that
    // as EOF, so an answer supplied before the prompt was printed is
    // discarded and the tool the operator meant to refuse then runs), or a
    // closed ConPTY on Windows.

    fn gate() -> ToolConfirmer {
        ToolConfirmer::new(false, vec![])
    }

    /// Drive the real interactive gate with an in-memory answer stream.
    /// Returns the decision and everything the gate printed.
    fn ask(confirmer: &mut ToolConfirmer, answer: &str) -> (ConfirmResult, String) {
        let mut answers = std::io::Cursor::new(answer.as_bytes().to_vec());
        let mut prompt = Vec::new();
        let result = confirmer.prompt_and_decide("Bash", "rm -rf /", &mut answers, &mut prompt);
        (result, String::from_utf8(prompt).expect("prompt is utf8"))
    }

    #[test]
    fn end_of_input_without_an_answer_is_never_approval() {
        let (result, prompt) = ask(&mut gate(), "");
        // Anti-vacuity: the gate really did reach the prompt and ask. A
        // decision taken before asking would leave this empty.
        assert!(
            prompt.contains("Allow?"),
            "the gate must ask before it decides; it printed {prompt:?}"
        );
        assert_ne!(
            result,
            ConfirmResult::Approved,
            "end-of-input with no answer must not approve the tool call"
        );
        assert_eq!(result, ConfirmResult::DeniedNoApprover);
    }

    #[test]
    fn eof_is_not_the_bare_enter_default() {
        // A real empty line (the operator pressed Enter) still means yes.
        assert_eq!(ask(&mut gate(), "\n").0, ConfirmResult::Approved);
        // The stream simply ending must NOT be read as the same thing.
        assert_eq!(ask(&mut gate(), "").0, ConfirmResult::DeniedNoApprover);
    }

    #[test]
    fn typed_answers_still_decide_the_gate() {
        // Positive controls: a blanket "deny everything" would pass the two
        // tests above, so pin that every real answer still works.
        assert_eq!(ask(&mut gate(), "y\n").0, ConfirmResult::Approved);
        assert_eq!(ask(&mut gate(), "YES\n").0, ConfirmResult::Approved);
        assert_eq!(ask(&mut gate(), "n\n").0, ConfirmResult::Denied);
        assert_eq!(ask(&mut gate(), "no\n").0, ConfirmResult::Denied);
        assert_eq!(ask(&mut gate(), "q\n").0, ConfirmResult::Quit);
        // A last answer with no trailing newline is still an answer.
        assert_eq!(ask(&mut gate(), "n").0, ConfirmResult::Denied);
        assert_eq!(ask(&mut gate(), "y").0, ConfirmResult::Approved);

        let mut always = gate();
        assert_eq!(ask(&mut always, "a\n").0, ConfirmResult::Approved);
        assert!(always.allow_list.contains("Bash"));
    }

    #[test]
    fn a_broken_answer_stream_is_denied() {
        struct Broken;
        impl std::io::Read for Broken {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("terminal went away"))
            }
        }
        let mut answers = std::io::BufReader::new(Broken);
        let mut prompt = Vec::new();
        assert_eq!(
            gate().prompt_and_decide("Bash", "x", &mut answers, &mut prompt),
            ConfirmResult::DeniedNoApprover
        );
    }

    #[test]
    fn tool_name_allow_list_remains_deliberately_unbound() {
        let confirmer =
            ToolConfirmer::with_policy(ApprovalPolicy::Prompt, vec!["TrustedLocalTool".into()]);
        assert!(!confirmer.requires_confirmation_for("TrustedLocalTool", ToolCategory::Exec));
        assert!(!confirmer.approval_is_input_bound("TrustedLocalTool"));
    }
}
