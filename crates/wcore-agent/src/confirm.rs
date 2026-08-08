use std::collections::HashSet;
use std::io::{self, BufRead, IsTerminal, Write};

use wcore_protocol::events::ToolCategory;
use wcore_types::execution_policy::ApprovalPolicy;

/// Where the gate gets one approval answer from, given the bound it may wait.
///
/// Production reads the process stdin through the single reader thread in
/// [`stdin_answer`]. A field rather than a hard-wired call so the gate's own
/// call site — the bound it passes, the decision it takes — is observable
/// without a terminal.
type AnswerSource = Box<dyn FnMut(Option<std::time::Duration>) -> ApprovalReply + Send>;

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
    /// How long a prompt waits for an answer before refusing the call.
    ///
    /// Resolved once, at construction, so the bound a test pins is the bound
    /// `check_for` uses. `None` waits forever.
    approval_timeout: Option<std::time::Duration>,
    /// Where answers come from. Defaults to the process stdin.
    answers: AnswerSource,
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
    /// The call was refused because the approver did not answer in time.
    ///
    /// Distinct from [`ConfirmResult::DeniedNoApprover`] for the same reason
    /// that one is distinct from [`ConfirmResult::Denied`]: an approver
    /// exists, the terminal is live, and the next prompt can still be
    /// answered. Telling this operator "this run has no interactive approver"
    /// is simply false, and acting on it — disabling the gate for the rest of
    /// the process — turns one slow answer into a run that can never approve
    /// anything again.
    DeniedNoAnswer,
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
            approval_timeout: approval_timeout(),
            answers: Box::new(stdin_answer),
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

        self.prompt_and_decide(tool_name, tool_input_display, &mut io::stderr())
    }

    /// Turn one approval answer into a decision.
    ///
    /// The single place the y/n/a/q vocabulary lives. `check_for` reaches it
    /// through the process stdin; `prompt_and_decide` reaches it through an
    /// in-memory stream so tests exercise this exact logic.
    fn decide_reply(&mut self, tool_name: &str, reply: ApprovalReply) -> ConfirmResult {
        let line = match reply {
            ApprovalReply::Line(line) => line,
            // Nobody answered. NOT an empty answer: falling through to the
            // `"y" | "yes" | ""` arm below — the bare-Enter default — would
            // APPROVE the call. Fail closed.
            // The answering stream ended or broke. There is no approver on
            // the other end of this terminal any more and there will not be
            // one again, so latch it: every later call takes the non-tty
            // guard above instead of asking a terminal that is gone.
            ApprovalReply::Ended => {
                self.interactive_approver = false;
                self.no_approver_denials += 1;
                return ConfirmResult::DeniedNoApprover;
            }
            // Nobody answered inside the bound. Refuse THIS call and nothing
            // more: the terminal is still there, the human was slow, and the
            // gate must still be usable when they come back.
            ApprovalReply::TimedOut => {
                self.no_approver_denials += 1;
                return ConfirmResult::DeniedNoAnswer;
            }
        };

        match line.trim().to_lowercase().as_str() {
            "y" | "yes" | "" => ConfirmResult::Approved,
            "a" | "always" => {
                self.allow_list.insert(tool_name.to_string());
                ConfirmResult::Approved
            }
            "q" | "quit" => ConfirmResult::Quit,
            _ => ConfirmResult::Denied,
        }
    }

    /// Print the approval prompt on `prompt_out`, read one answer from
    /// `answers`, and decide.
    ///
    /// This is the whole interactive gate. `check_for` calls it with the
    /// process's real stdin/stderr; tests call it with in-memory handles so
    /// the decision for every possible answer — including "no answer ever
    /// arrives" — is observable without a terminal.
    /// Ask, wait for one answer, and decide.
    ///
    /// `check_for` calls it with the process stderr, the configured bound and
    /// a read of the process stdin. Tests call it with in-memory handles, so
    /// the prompt, the bound and the decision they exercise are the ones that
    /// ship.
    fn prompt_and_decide<W: Write>(
        &mut self,
        tool_name: &str,
        tool_input_display: &str,
        prompt_out: &mut W,
    ) -> ConfirmResult {
        print_prompt(tool_name, tool_input_display, prompt_out);
        let reply = (self.answers)(self.approval_timeout);
        if reply == ApprovalReply::TimedOut {
            print_timeout_notice(self.approval_timeout, prompt_out);
        }
        self.decide_reply(tool_name, reply)
    }

    /// Pin the bound. Tests only: production resolves it once, from the
    /// environment, at construction.
    #[cfg(test)]
    fn set_approval_timeout(&mut self, timeout: Option<std::time::Duration>) {
        self.approval_timeout = timeout;
    }

    /// Answer the gate from somewhere other than the process stdin. Tests
    /// only — it is how the decision, the bound and the latch are observable
    /// without a terminal.
    #[cfg(test)]
    fn set_answers(&mut self, answers: AnswerSource) {
        self.answers = answers;
    }
}

/// Read one answer.
///
/// `read_line` reports end-of-input as `Ok(0)`, not `Err`, and leaves the
/// buffer empty; only the byte count separates "the operator pressed Enter"
/// from "the stream ended", and treating the second as the first is what made
/// the gate fail open.
fn read_reply<R: BufRead>(answers: &mut R) -> ApprovalReply {
    let mut input = String::new();
    match answers.read_line(&mut input) {
        Ok(0) | Err(_) => ApprovalReply::Ended,
        Ok(_) => ApprovalReply::Line(input),
    }
}

/// One answer to an approval prompt, or the reason there was not one.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ApprovalReply {
    Line(String),
    /// The answering stream ended or broke before an answer arrived.
    Ended,
    /// Nobody answered inside the bound.
    TimedOut,
}

fn print_prompt<W: Write>(tool_name: &str, tool_input_display: &str, out: &mut W) {
    let _ = write!(
        out,
        "\n[tool] {}({})\nAllow? [y]es / [n]o / [a]lways / [q]uit > ",
        tool_name, tool_input_display
    );
    // SAFETY: flushing can fail only if the sink is closed (e.g. the parent
    // piped to something that disconnected). The read that follows would fail
    // in that scenario too and bail with a refusal, so ignoring the error
    // defers the same outcome by one cycle.
    let _ = out.flush();
}

/// How long to wait for an approval answer before failing closed.
///
/// `WAYLAND_APPROVAL_TIMEOUT_SECS=0` restores the unbounded wait.
fn approval_timeout() -> Option<std::time::Duration> {
    parse_approval_timeout(
        std::env::var("WAYLAND_APPROVAL_TIMEOUT_SECS")
            .ok()
            .as_deref(),
    )
}

/// Split from [`approval_timeout`] so the policy can be tabled without
/// `set_var`, which is unsound in a multi-threaded test binary.
fn parse_approval_timeout(raw: Option<&str>) -> Option<std::time::Duration> {
    /// Long enough that a human still deciding is never refused; short enough
    /// that a silent terminal cannot hold a turn, a budget or a lease open
    /// indefinitely.
    const DEFAULT_SECS: u64 = 300;
    let secs = raw
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_SECS);
    (secs > 0).then(|| std::time::Duration::from_secs(secs))
}

/// Tell the operator what the refusal was, and what their terminal will do.
///
/// The reader is still inside its `read_line` while this prints. The next line
/// typed finishes THAT read and is discarded — an answer aimed at a prompt
/// that has already been decided must not be applied to the next one, which
/// the operator had not seen when they typed it. So exactly one line is
/// swallowed, once, and then the terminal behaves normally; saying so turns a
/// confusing swallow into an instruction.
fn print_timeout_notice<W: Write>(timeout: Option<std::time::Duration>, out: &mut W) {
    let waited = timeout.map(|t| t.as_secs()).unwrap_or_default();
    let _ = write!(
        out,
        "\n[tool] no answer in {waited}s - this call was refused. Approval is \
         still live for the rest of this run; press Enter once to clear the \
         stale prompt before answering the next one.\n"
    );
    let _ = out.flush();
}

/// One request for a line from the process's stdin.
struct AnswerRequest(std::sync::mpsc::SyncSender<ApprovalReply>);

/// The one thread in this process that reads stdin for an approval answer.
///
/// There is no portable way to cancel a blocking read, so a prompt that times
/// out necessarily leaves its read outstanding — and that read holds the
/// process-wide `Stdin` lock. A thread per prompt therefore piles up threads
/// all queued on that lock, and every one of them is a line the operator will
/// type and lose. One long-lived reader can only ever have a single read
/// outstanding: it takes the lock per request and releases it before waiting
/// for the next one, so the REPL's own `read_line` is blocked only while a
/// prompt is genuinely waiting, and the cost of a timeout is bounded at the
/// one line that finishes the stale read.
fn stdin_reader() -> &'static std::sync::mpsc::Sender<AnswerRequest> {
    static READER: std::sync::OnceLock<std::sync::mpsc::Sender<AnswerRequest>> =
        std::sync::OnceLock::new();
    READER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<AnswerRequest>();
        // A thread that cannot be spawned drops `rx` with the closure, so every
        // later request fails to send and is reported as `Ended`. Fail closed,
        // rather than panicking in the middle of a tool call.
        let _ = std::thread::Builder::new()
            .name("wayland-approval-stdin".to_string())
            .spawn(move || {
                for AnswerRequest(answer) in rx {
                    let reply = read_reply(&mut io::stdin().lock());
                    let stream_ended = !matches!(reply, ApprovalReply::Line(_));
                    // The requester may have given up. A late answer is
                    // dropped here, never handed to the next prompt.
                    let _ = answer.send(reply);
                    if stream_ended {
                        // stdin will not produce another line; leaving the loop
                        // drops the receiver so later requests fail fast rather
                        // than spinning on EOF.
                        break;
                    }
                }
            });
        tx
    })
}

/// Ask the process's stdin for one answer, bounded by `timeout`.
fn stdin_answer(timeout: Option<std::time::Duration>) -> ApprovalReply {
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    if stdin_reader().send(AnswerRequest(tx)).is_err() {
        return ApprovalReply::Ended;
    }
    await_answer(&rx, timeout)
}

/// Wait for one answer, giving up after `timeout`.
///
/// `None` waits forever, which is what the whole process used to do
/// unconditionally.
fn await_answer(
    rx: &std::sync::mpsc::Receiver<ApprovalReply>,
    timeout: Option<std::time::Duration>,
) -> ApprovalReply {
    let Some(timeout) = timeout else {
        return rx.recv().unwrap_or(ApprovalReply::Ended);
    };
    match rx.recv_timeout(timeout) {
        Ok(reply) => reply,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => ApprovalReply::TimedOut,
        // The reader is gone: stdin ended, or it could not be spawned at all.
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => ApprovalReply::Ended,
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
        let bytes = answer.as_bytes().to_vec();
        confirmer.set_answers(Box::new(move |_| {
            read_reply(&mut std::io::Cursor::new(bytes.clone()))
        }));
        let mut prompt = Vec::new();
        let result = confirmer.prompt_and_decide("Bash", "rm -rf /", &mut prompt);
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
        let mut gate = gate();
        gate.set_answers(Box::new(|_| {
            read_reply(&mut std::io::BufReader::new(Broken))
        }));
        let mut prompt = Vec::new();
        assert_eq!(
            gate.prompt_and_decide("Bash", "x", &mut prompt),
            ConfirmResult::DeniedNoApprover
        );
    }

    // ---- An answer that never comes must not hold the turn forever ----
    //
    // The `is_terminal()` guard above stops a blocking `read_line` on a pipe
    // that never reaches EOF. It does NOT cover a terminal that is present but
    // delivers no further line — a detached tmux/screen pane, a wedged ssh
    // session whose pty stays open, a harness that allocates a pty and never
    // writes to it. There the prompt waited forever, so the turn, the run and
    // any budget or lease it held waited forever too.

    /// Drive the real wait with an answer that arrives after `delay`.
    fn wait_for(
        delay: std::time::Duration,
        answer: ApprovalReply,
        timeout: Option<std::time::Duration>,
    ) -> ApprovalReply {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        std::thread::spawn(move || {
            std::thread::sleep(delay);
            let _ = tx.send(answer);
        });
        await_answer(&rx, timeout)
    }

    #[test]
    fn an_answer_that_never_comes_is_bounded_and_refused() {
        let started = std::time::Instant::now();
        // Stands in for a terminal that is open and silent.
        let reply = wait_for(
            std::time::Duration::from_secs(30),
            ApprovalReply::Line("y\n".to_string()),
            Some(std::time::Duration::from_millis(150)),
        );
        let waited = started.elapsed();

        assert_eq!(
            reply,
            ApprovalReply::TimedOut,
            "the gate waited out a silent terminal instead of giving up"
        );
        assert!(
            waited < std::time::Duration::from_secs(5),
            "the wait was not bounded: {waited:?}"
        );
        // And the bound must fail CLOSED, not inherit the bare-Enter default.
        assert_eq!(
            gate().decide_reply("Bash", ApprovalReply::TimedOut),
            ConfirmResult::DeniedNoAnswer
        );
    }

    #[test]
    fn an_answer_that_arrives_inside_the_bound_is_honoured() {
        // The positive control. Without it, "always time out" passes the test
        // above, and that would break every interactive approval there is.
        let reply = wait_for(
            std::time::Duration::ZERO,
            ApprovalReply::Line("n\n".to_string()),
            Some(std::time::Duration::from_secs(30)),
        );
        assert_eq!(reply, ApprovalReply::Line("n\n".to_string()));

        // A slow but real answer, well inside a generous bound.
        let reply = wait_for(
            std::time::Duration::from_millis(200),
            ApprovalReply::Line("y\n".to_string()),
            Some(std::time::Duration::from_secs(30)),
        );
        assert_eq!(reply, ApprovalReply::Line("y\n".to_string()));

        // And the documented escape hatch: no bound at all still waits.
        let reply = wait_for(
            std::time::Duration::from_millis(200),
            ApprovalReply::Line("a\n".to_string()),
            None,
        );
        assert_eq!(reply, ApprovalReply::Line("a\n".to_string()));
    }

    #[test]
    fn the_approval_bound_is_generous_by_default_and_can_be_switched_off() {
        // Not a wall-clock guess dressed as a constant: a human who has not
        // answered in five minutes has walked away, and the default must be
        // long enough that "still thinking" is never refused.
        // Parsed from the raw value rather than through `set_var`, which is
        // unsound in a multi-threaded test binary.
        let secs = |d: Option<std::time::Duration>| d.map(|d| d.as_secs());
        assert_eq!(secs(parse_approval_timeout(None)), Some(300));
        assert_eq!(secs(parse_approval_timeout(Some("42"))), Some(42));
        assert_eq!(secs(parse_approval_timeout(Some(" 42 "))), Some(42));
        assert_eq!(
            parse_approval_timeout(Some("0")),
            None,
            "0 must restore the unbounded wait"
        );
        assert_eq!(
            secs(parse_approval_timeout(Some("not-a-number"))),
            Some(300),
            "an unparseable value must fall back to the default, not to no bound"
        );
        // The live reader agrees with the parser on this host.
        assert_eq!(
            approval_timeout(),
            parse_approval_timeout(
                std::env::var("WAYLAND_APPROVAL_TIMEOUT_SECS")
                    .ok()
                    .as_deref()
            )
        );
    }

    /// Answer each prompt from a fixed script, one answer per prompt.
    fn scripted(answers: Vec<ApprovalReply>) -> AnswerSource {
        let mut answers = answers.into_iter();
        Box::new(move |_| answers.next().expect("one scripted answer per prompt"))
    }

    #[test]
    fn a_second_prompt_after_a_timeout_is_still_answerable() {
        // The whole point of not latching. Driven through `check_for`, which
        // is where the latch is read, so a fix that only changes the variant
        // cannot pass this.
        let mut gate = gate();
        gate.set_interactive_approver(true);
        gate.set_answers(scripted(vec![
            ApprovalReply::TimedOut,
            ApprovalReply::Line("y\n".to_string()),
        ]));

        assert_eq!(
            gate.check_for("Bash", ToolCategory::Exec, "rm -rf /"),
            ConfirmResult::DeniedNoAnswer,
            "an unanswered prompt refuses THIS call"
        );
        assert!(
            gate.has_interactive_approver(),
            "the terminal is still there; the human was slow"
        );
        assert_eq!(
            gate.check_for("Bash", ToolCategory::Exec, "ls"),
            ConfirmResult::Approved,
            "the operator came back and approved the next call; a latched gate \
             would refuse it and tell them the run has no interactive approver"
        );
    }

    #[test]
    fn an_answering_stream_that_ended_does_latch() {
        // The other half of the distinction: this one is not coming back, and
        // asking a terminal that is gone once per tool call is pointless.
        let mut gate = gate();
        gate.set_interactive_approver(true);
        gate.set_answers(Box::new(|_| ApprovalReply::Ended));

        assert_eq!(
            gate.check_for("Bash", ToolCategory::Exec, "rm -rf /"),
            ConfirmResult::DeniedNoApprover
        );
        assert!(
            !gate.has_interactive_approver(),
            "an ended answering stream never produces another answer"
        );
    }

    #[test]
    fn a_timeout_tells_the_operator_the_gate_is_still_live() {
        // The refusal the operator reads must not claim the run has no
        // approver: it has one, and it is still usable.
        let mut gate = gate();
        gate.set_interactive_approver(true);
        gate.set_approval_timeout(Some(std::time::Duration::from_secs(300)));
        gate.set_answers(Box::new(|_| ApprovalReply::TimedOut));
        let mut prompt = Vec::new();
        assert_eq!(
            gate.prompt_and_decide("Bash", "rm -rf /", &mut prompt),
            ConfirmResult::DeniedNoAnswer
        );
        let shown = String::from_utf8(prompt).expect("prompt is utf8");
        assert!(shown.contains("no answer in 300s"), "{shown:?}");
        assert!(shown.contains("still live"), "{shown:?}");
        assert!(
            !shown.contains("no interactive approver"),
            "the operator is at a live terminal; {shown:?}"
        );
    }

    #[test]
    fn the_bound_reaches_the_reader_from_the_gate_that_ships() {
        // A RATCHET, not a restatement. The bound was live and every test of
        // it passed while the CALL SITE handed the reader `None` instead: the
        // parser was tested, the waiter was tested, and the one line joining
        // them to `check_for` was not. Replacing `self.approval_timeout` with
        // `None` here restores the unbounded wait, and nothing went red.
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorder = std::sync::Arc::clone(&seen);
        let bound = std::time::Duration::from_secs(7);

        let mut gate = gate();
        gate.set_interactive_approver(true);
        gate.set_approval_timeout(Some(bound));
        gate.set_answers(Box::new(move |handed| {
            recorder.lock().expect("recorder").push(handed);
            ApprovalReply::Line("n\n".to_string())
        }));

        assert_eq!(
            gate.check_for("Bash", ToolCategory::Exec, "rm -rf /"),
            ConfirmResult::Denied,
            "positive control: the answer still decides the gate"
        );
        assert_eq!(
            *seen.lock().expect("recorder"),
            vec![Some(bound)],
            "the production call site must hand the reader the configured \
             bound; `None` there is the unbounded wait this closed"
        );
    }

    #[test]
    fn a_new_gate_is_born_with_the_configured_bound() {
        // The other half of the ratchet: wiring the field through to the
        // reader is worth nothing if construction leaves the field empty.
        for gate in [
            ToolConfirmer::new(false, vec![]),
            ToolConfirmer::with_policy(ApprovalPolicy::Prompt, vec![]),
            ToolConfirmer::with_policy(ApprovalPolicy::AutoEdit, vec![]),
        ] {
            assert_eq!(
                gate.approval_timeout,
                approval_timeout(),
                "a confirmer must be born with the bound the environment says"
            );
            assert!(
                gate.approval_timeout.is_some(),
                "and on a host that has not switched it off, that is a bound"
            );
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
