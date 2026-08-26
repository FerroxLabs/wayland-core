use std::collections::HashSet;
use std::io::{self, BufRead, IsTerminal, Read, Write};
use std::time::{Duration, Instant};

use wcore_protocol::events::ToolCategory;
use wcore_types::execution_policy::ApprovalPolicy;

/// Wall-clock budget for one answer at the interactive approval prompt.
///
/// `is_terminal()` proves stdin is a terminal; it never proves anyone is
/// reading it. A detached tmux or screen pane, a `script` wrapper, and CI that
/// allocated a tty nobody types into all leave a real pty on stdin with no one
/// on the other end, so the non-terminal guard in `check_for` does not fire and
/// `read_line` parks for the life of the process. Five minutes is far longer
/// than a human needs for a prompt they can see, and unlike the unbounded wait
/// it ends in a decision instead of a dead turn.
///
/// The number is a ratified product choice, not an arbitrary literal, so do not
/// "fix" this by removing the bound. It is defensible because the outcome is
/// fail-closed (a timeout denies, never approves), because a denial is
/// recoverable (the model reports it and the user can ask again) where an
/// indefinite park is not, and because the escape hatch is explicit:
/// `WAYLAND_APPROVAL_TIMEOUT_SECS=0` restores the old unbounded wait for
/// anyone who wants it. Changing the default changes documented behaviour --
/// see the Tool Confirmation section of `docs/getting-started.md`.
const DEFAULT_APPROVAL_TIMEOUT_SECS: u64 = 300;

/// Override for [`DEFAULT_APPROVAL_TIMEOUT_SECS`], in whole seconds. `0`
/// restores the historical unbounded wait for operators who want it; an
/// unparseable value falls back to the default rather than to "never".
const APPROVAL_TIMEOUT_ENV: &str = "WAYLAND_APPROVAL_TIMEOUT_SECS";

/// The configured approval budget, or `None` when the operator asked to wait
/// forever.
fn approval_timeout() -> Option<Duration> {
    let secs = std::env::var(APPROVAL_TIMEOUT_ENV)
        .ok()
        .and_then(|raw| raw.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_APPROVAL_TIMEOUT_SECS);
    (secs > 0).then(|| Duration::from_secs(secs))
}

/// Outcome of waiting for the approver to type something.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AnswerWait {
    /// Input is available, so the blocking read will not park.
    Ready,
    /// The budget expired with nothing typed.
    TimedOut,
    /// Readiness cannot be arbitrated on this stdin. The caller keeps the
    /// historical blocking read: a stdin that cannot be polled cannot be read
    /// either, and `decide_from_answer` already fails closed on that error.
    Unavailable,
}

/// Wait up to `budget` for stdin to have something to read. The single
/// centralized home for this platform difference — no call site cfg-branches.
#[cfg(unix)]
fn wait_for_answer(budget: Duration) -> AnswerWait {
    use std::os::fd::AsRawFd;

    let deadline = Instant::now() + budget;
    let mut fd = libc::pollfd {
        fd: io::stdin().as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        // `poll(2)` takes whole milliseconds in an `int`. Clamping keeps a
        // large budget from wrapping into a negative timeout, which `poll`
        // reads as "wait forever" — precisely the hang this guard ends.
        let millis = i32::try_from(remaining.as_millis()).unwrap_or(i32::MAX);
        // SAFETY: `fd` is a live, fully initialised `pollfd` describing this
        // process's own stdin, and the length matches the single element.
        let rc = unsafe { libc::poll(&mut fd, 1, millis) };
        if rc > 0 {
            return AnswerWait::Ready;
        }
        if rc == 0 {
            return AnswerWait::TimedOut;
        }
        // A signal (SIGWINCH from a resize, SIGCHLD from a finished tool) is
        // not an answer; retry against the same deadline rather than reading
        // the interruption as either consent or a timeout.
        if io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return AnswerWait::Unavailable;
    }
}

/// Whether a peeked batch of pending console input already carries a whole
/// line, given the characters on its key-DOWN records.
///
/// Pulled out of the Windows branch deliberately. This predicate is the entire
/// thing #1131 says Windows gets wrong, and as a pure function it is graded by
/// unit tests on every platform instead of only on a box this project cannot
/// run. `peek_filled_buffer` says the peek window was too small to see the
/// whole queue — somebody is actively typing a lot — and keeps the historical
/// "signalled means ready" answer there rather than inventing a new way to
/// time out on a user who IS answering.
#[cfg(any(windows, test))]
fn console_line_ready(key_down_chars: &[u16], peek_filled_buffer: bool) -> bool {
    peek_filled_buffer
        || key_down_chars
            .iter()
            .any(|&ch| ch == u16::from(b'\r') || ch == u16::from(b'\n'))
}

/// Windows counterpart.
///
/// #1131: a console input handle is signalled by ANY input record — a key-UP,
/// a mouse move, a focus change, a resize — not by "a whole line is readable".
/// Returning `Ready` on the bare signal therefore hands a blocking read a queue
/// with no line in it, and the read parks exactly as it did before the guard
/// existed. So wait for the signal, then PEEK the queue and only report `Ready`
/// once a key-down carrying a line terminator is actually pending.
///
/// The peek never consumes: the console's own line editor owns those records,
/// and draining them here would take the user's keystrokes away from the read
/// that follows.
#[cfg(windows)]
fn wait_for_answer(budget: Duration) -> AnswerWait {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Console::{INPUT_RECORD, KEY_EVENT, PeekConsoleInputW};
    use windows_sys::Win32::System::Threading::WaitForSingleObject;

    /// How many pending input records one peek inspects.
    const PEEK_RECORDS: usize = 256;
    /// The handle stays signalled for as long as the records we decline to
    /// consume sit in the queue, so re-waiting immediately would spin a core.
    const RESAMPLE: Duration = Duration::from_millis(25);

    let deadline = Instant::now() + budget;
    let handle = io::stdin().as_raw_handle() as HANDLE;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return AnswerWait::TimedOut;
        }
        // `INFINITE` is `u32::MAX`; staying one below it keeps a large budget
        // from becoming the unbounded wait this guard exists to end.
        let millis = u32::try_from(remaining.as_millis())
            .unwrap_or(u32::MAX - 1)
            .min(u32::MAX - 1);
        // SAFETY: `handle` is this process's own live standard-input handle.
        match unsafe { WaitForSingleObject(handle, millis) } {
            WAIT_OBJECT_0 => {}
            WAIT_TIMEOUT => return AnswerWait::TimedOut,
            _ => return AnswerWait::Unavailable,
        }

        // SAFETY: an all-zero `INPUT_RECORD` is a valid initial state for an
        // out-param array, the requested count matches the array length, and
        // `peeked_count` is a live `u32` slot.
        let mut records: [INPUT_RECORD; PEEK_RECORDS] = unsafe { std::mem::zeroed() };
        let mut peeked_count: u32 = 0;
        let ok = unsafe {
            PeekConsoleInputW(
                handle,
                records.as_mut_ptr(),
                PEEK_RECORDS as u32,
                &mut peeked_count,
            )
        };
        if ok == 0 {
            // Not a console input handle after all, or the peek failed. Keep
            // the historical answer rather than inventing a new refusal: the
            // read that follows is no worse off than it was before this guard.
            return AnswerWait::Ready;
        }
        let peeked = (peeked_count as usize).min(PEEK_RECORDS);
        let chars: Vec<u16> = records[..peeked]
            .iter()
            .filter(|record| u32::from(record.EventType) == KEY_EVENT)
            // SAFETY: the union is read as `KeyEvent` only for records whose
            // `EventType` says that is the live variant.
            .filter(|record| unsafe { record.Event.KeyEvent.bKeyDown } != 0)
            .map(|record| unsafe { record.Event.KeyEvent.uChar.UnicodeChar })
            .collect();
        if console_line_ready(&chars, peeked >= PEEK_RECORDS) {
            return AnswerWait::Ready;
        }

        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return AnswerWait::TimedOut;
        }
        std::thread::sleep(RESAMPLE.min(remaining));
    }
}

#[cfg(not(any(unix, windows)))]
fn wait_for_answer(_budget: Duration) -> AnswerWait {
    AnswerWait::Unavailable
}

/// Longest answer accepted at the approval prompt. The answers are `y`/`n`/
/// `a`/`q` and their spellings; without a cap a stdin that streams bytes and
/// never sends a terminator would grow this buffer for the whole budget.
const MAX_ANSWER_BYTES: usize = 4096;

/// How much of a pending answer a single read takes. Large enough that a
/// console read returns a whole line in one call (Windows retains nothing for
/// us between readiness waits), small enough to stay a stack buffer.
const ANSWER_CHUNK_BYTES: usize = 256;

/// Outcome of reading one whole answer under a deadline.
#[derive(Debug, Clone, PartialEq, Eq)]
enum AnswerRead {
    /// A complete line arrived; the terminator is stripped.
    Line(String),
    /// The budget expired with no complete answer. The operator gets the
    /// budget notice.
    Expired,
    /// stdin ended or failed with nothing usable typed, or the "answer" ran
    /// past `MAX_ANSWER_BYTES` without a terminator. Deny, quietly.
    NoAnswer,
    /// Readiness cannot be arbitrated on this stdin and nothing has been
    /// consumed yet, so the caller can still fall back to the historical
    /// blocking read with nothing lost.
    Unavailable,
}

/// Read one whole line, giving the WHOLE answer `budget` instead of only its
/// first byte.
///
/// This is the #1131 fix. Readiness is not completeness. A canonical terminal's
/// line discipline holds a partial line back, so waiting once for readiness and
/// then doing a blocking `read_line` was correct there — but in a non-canonical
/// (raw) terminal the byte is delivered the instant it is typed, the readiness
/// wait reports ready on `y` with no Enter behind it, and the blocking read then
/// parks for the life of the process on a terminator that never comes. Looping
/// the readiness wait against a single deadline and accumulating what is
/// actually available makes the bound cover the answer in every tty mode, with
/// no global terminal state touched and no change to what an answer means.
///
/// Fail-closed is preserved and load-bearing: every exit that is not a complete
/// line is a non-answer, and `check_for` denies on all of them.
fn read_answer_within(
    budget: Duration,
    mut wait: impl FnMut(Duration) -> AnswerWait,
    mut read: impl FnMut(&mut [u8]) -> io::Result<usize>,
) -> AnswerRead {
    let deadline = Instant::now() + budget;
    let mut answer: Vec<u8> = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return AnswerRead::Expired;
        }
        match wait(remaining) {
            AnswerWait::Ready => {}
            AnswerWait::TimedOut => return AnswerRead::Expired,
            // Falling back to the blocking read is safe only while nothing has
            // been consumed. Once a partial line is in hand, that fallback is
            // precisely the park this function exists to end, so fail closed.
            AnswerWait::Unavailable if answer.is_empty() => return AnswerRead::Unavailable,
            AnswerWait::Unavailable => return AnswerRead::NoAnswer,
        }

        let mut chunk = [0u8; ANSWER_CHUNK_BYTES];
        let read_len = match read(&mut chunk) {
            // EOF. A partial answer already typed is still an answer and
            // `read_line` has always returned it; EOF with nothing typed is the
            // absence of one and fails closed, exactly as `decide_from_answer`
            // makes it.
            Ok(0) if answer.is_empty() => return AnswerRead::NoAnswer,
            Ok(0) => return AnswerRead::Line(String::from_utf8_lossy(&answer).into_owned()),
            Ok(n) => n,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return AnswerRead::NoAnswer,
        };

        for &byte in &chunk[..read_len] {
            // CR as well as LF. Enter in a raw terminal sends CR — there is no
            // ICRNL to translate it — so accepting only LF would expire the
            // budget on a raw-mode user who did answer.
            //
            // Bytes past the terminator are dropped. A canonical tty read
            // returns at most one line, so nothing is ever dropped there; only
            // a raw tty can deliver more, and a single-key raw prompt is not a
            // place anyone types whole extra lines ahead.
            if byte == b'\n' || byte == b'\r' {
                return AnswerRead::Line(String::from_utf8_lossy(&answer).into_owned());
            }
            if answer.len() >= MAX_ANSWER_BYTES {
                return AnswerRead::NoAnswer;
            }
            answer.push(byte);
        }
    }
}

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

        // A terminal is not an approver. The guard above proves stdin is a
        // tty, never that anything is attached to the other end of it, so a
        // detached tmux/screen pane, a `script` wrapper, or CI that allocated
        // a tty nobody types into leaves `read_line` parked for the life of
        // the process and the turn stops with no output and no way to answer.
        // Bound the wait and fail closed, exactly like the non-terminal and
        // EOF cases: a tool that needs confirmation and never gets one is
        // denied, never auto-approved.
        // An operator who asked to wait forever keeps the historical
        // unbounded blocking read, byte for byte.
        let Some(budget) = approval_timeout() else {
            return self.decide_from_answer(tool_name, &mut io::stdin().lock());
        };

        match read_answer_within(budget, wait_for_answer, |buf| io::stdin().read(buf)) {
            AnswerRead::Line(answer) => self.decide_from_line(tool_name, &answer),
            // A stdin whose readiness cannot be arbitrated cannot be read
            // either, and `decide_from_answer` already fails closed on that
            // error; nothing has been consumed, so nothing is lost.
            AnswerRead::Unavailable => self.decide_from_answer(tool_name, &mut io::stdin().lock()),
            AnswerRead::Expired => {
                eprintln!(
                    "\nNo answer after {}s - denying {}. Set {}=<seconds> to change \
                     the budget (0 waits forever).",
                    budget.as_secs(),
                    tool_name,
                    APPROVAL_TIMEOUT_ENV
                );
                let _ = io::stderr().flush();
                tracing::debug!(
                    target: "wcore_agent::confirm",
                    tool = %tool_name,
                    timeout_secs = budget.as_secs(),
                    "no answer at the approval prompt within the budget; denying (terminal with no approver)"
                );
                ConfirmResult::Denied
            }
            AnswerRead::NoAnswer => {
                tracing::debug!(
                    target: "wcore_agent::confirm",
                    tool = %tool_name,
                    "stdin ended or failed at the approval prompt; denying (no answer given)"
                );
                ConfirmResult::Denied
            }
        }
    }

    /// Read one answer from `reader` and map it to a decision.
    ///
    /// Split out of `check_for` so the mapping is testable without a real
    /// TTY. EOF is the security-critical case: `read_line` reports it as
    /// `Ok(0)`, not `Err`, and leaves `input` empty. Falling through to the
    /// empty-line "yes" default would turn a closed stdin — Ctrl-D at the
    /// prompt, or a pipe that ended — into consent that was never given, so
    /// EOF fails closed exactly like an I/O error.
    fn decide_from_answer<R: BufRead>(&mut self, tool_name: &str, reader: &mut R) -> ConfirmResult {
        let mut input = String::new();
        match reader.read_line(&mut input) {
            Ok(0) | Err(_) => return ConfirmResult::Denied,
            Ok(_) => {}
        }

        self.decide_from_line(tool_name, &input)
    }

    /// Map one answered line to a decision. Shared by the historical blocking
    /// read and by the bounded read in `check_for` so the two cannot drift into
    /// answering the same keystroke differently.
    fn decide_from_line(&mut self, tool_name: &str, answer: &str) -> ConfirmResult {
        match answer.trim().to_lowercase().as_str() {
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

    // Ctrl-D at the approval prompt must NOT approve the tool. `read_line`
    // reports EOF as `Ok(0)`, never `Err`, so the old `is_err()` guard fell
    // straight through to the `"" => Approved` arm: closing stdin granted
    // consent the user never gave.
    #[test]
    fn eof_at_prompt_is_denied_not_approved() {
        let mut confirmer = ToolConfirmer::new(false, vec![]);
        let mut eof = io::Cursor::new(Vec::new());
        assert_eq!(
            confirmer.decide_from_answer("Bash", &mut eof),
            ConfirmResult::Denied,
            "EOF (Ctrl-D / closed stdin) must fail closed, not be read as the \
             empty-line default"
        );
    }

    // The boundary the fix turns on: an empty LINE is a real answer and keeps
    // its historical "yes" meaning, while EOF is the absence of any answer.
    // Both leave `input` empty after trimming, so only the `Ok(0)` vs `Ok(n)`
    // distinction separates them.
    #[test]
    fn empty_line_still_approves_but_eof_does_not() {
        let mut confirmer = ToolConfirmer::new(false, vec![]);
        let mut newline = io::Cursor::new(b"\n".to_vec());
        assert_eq!(
            confirmer.decide_from_answer("Bash", &mut newline),
            ConfirmResult::Approved,
            "pressing Enter is an affirmative answer and must stay Approved"
        );
        let mut eof = io::Cursor::new(Vec::new());
        assert_eq!(
            confirmer.decide_from_answer("Bash", &mut eof),
            ConfirmResult::Denied,
            "EOF is the absence of an answer and must not share Enter's default"
        );
    }

    /// Drive `read_answer_within` with a scripted readiness sequence and a
    /// scripted byte stream. No tty, no env, no process globals — so nextest's
    /// per-process isolation is not load-bearing for any of these.
    fn scripted(waits: &[AnswerWait], chunks: &[io::Result<&'static [u8]>]) -> AnswerRead {
        let mut waits = waits.iter().copied();
        let mut chunks = chunks
            .iter()
            .map(|chunk| match chunk {
                Ok(bytes) => Ok(*bytes),
                Err(err) => Err(err.kind()),
            })
            .collect::<Vec<_>>()
            .into_iter();
        read_answer_within(
            Duration::from_secs(30),
            |_| waits.next().unwrap_or(AnswerWait::TimedOut),
            |buf| match chunks.next() {
                Some(Ok(bytes)) => {
                    let n = bytes.len().min(buf.len());
                    buf[..n].copy_from_slice(&bytes[..n]);
                    Ok(n)
                }
                Some(Err(kind)) => Err(io::Error::from(kind)),
                None => Ok(0),
            },
        )
    }

    // THE #1131 DEFECT, at the unit level. Raw mode delivers `y` the instant it
    // is typed, so readiness is real — but the answer is not complete, and the
    // old code handed that to a blocking `read_line` that never returned. The
    // budget must cover the whole answer and expire on it.
    #[test]
    fn a_partial_line_expires_on_the_budget_instead_of_parking() {
        assert_eq!(
            scripted(&[AnswerWait::Ready, AnswerWait::TimedOut], &[Ok(b"y")],),
            AnswerRead::Expired,
            "a keystroke with no terminator behind it is not an answer and must \
             run out the budget, not park a blocking read"
        );
    }

    // Negative control for the above: the same path must still hear an answer
    // that IS complete, or "fixing" the hang would just be a blanket denial.
    #[test]
    fn a_complete_line_is_returned_without_its_terminator() {
        assert_eq!(
            scripted(&[AnswerWait::Ready], &[Ok(b"y\n")]),
            AnswerRead::Line("y".to_string())
        );
    }

    // Enter in a raw terminal sends CR, not LF: there is no ICRNL to translate
    // it. Treating only LF as the terminator would expire the budget on a
    // raw-mode user who did answer.
    #[test]
    fn carriage_return_terminates_a_raw_mode_answer() {
        assert_eq!(
            scripted(&[AnswerWait::Ready], &[Ok(b"y\r")]),
            AnswerRead::Line("y".to_string())
        );
    }

    // A raw tty hands over whatever has been typed so far, which for a
    // multi-character answer can be several reads.
    #[test]
    fn an_answer_split_across_reads_is_reassembled() {
        assert_eq!(
            scripted(
                &[AnswerWait::Ready, AnswerWait::Ready, AnswerWait::Ready],
                &[Ok(b"al"), Ok(b"way"), Ok(b"s\n")],
            ),
            AnswerRead::Line("always".to_string())
        );
    }

    // EOF keeps the meaning `decide_from_answer` already gives it: nothing
    // typed is the absence of an answer and fails closed, while bytes already
    // typed are the answer `read_line` would have returned.
    #[test]
    fn eof_is_an_answer_only_when_something_was_typed() {
        assert_eq!(
            scripted(&[AnswerWait::Ready], &[Ok(b"")]),
            AnswerRead::NoAnswer,
            "EOF with nothing typed must fail closed"
        );
        assert_eq!(
            scripted(
                &[AnswerWait::Ready, AnswerWait::Ready],
                &[Ok(b"y"), Ok(b"")]
            ),
            AnswerRead::Line("y".to_string()),
            "EOF after a partial answer returns what read_line would have"
        );
    }

    // The fallback to the historical blocking read is only safe while nothing
    // has been consumed. After a partial line it would re-park on exactly the
    // bytes this loop exists to bound, so it must fail closed instead.
    #[test]
    fn unpollable_stdin_falls_back_only_before_the_first_byte() {
        assert_eq!(
            scripted(&[AnswerWait::Unavailable], &[]),
            AnswerRead::Unavailable
        );
        assert_eq!(
            scripted(&[AnswerWait::Ready, AnswerWait::Unavailable], &[Ok(b"y")],),
            AnswerRead::NoAnswer,
            "a partial line plus an unpollable stdin must deny, never re-park"
        );
    }

    // An unterminated flood is not an answer to a [y/n/a/q] prompt, and must
    // not be allowed to grow the buffer for the whole budget.
    #[test]
    fn an_unterminated_flood_is_refused_rather_than_buffered() {
        let waits = vec![AnswerWait::Ready; 64];
        let chunk: &'static [u8] = &[b'x'; ANSWER_CHUNK_BYTES];
        let chunks: Vec<io::Result<&'static [u8]>> = (0..64).map(|_| Ok(chunk)).collect();
        assert_eq!(
            scripted(&waits, &chunks),
            AnswerRead::NoAnswer,
            "an answer past MAX_ANSWER_BYTES with no terminator must be refused"
        );
    }

    // A read error is not consent.
    #[test]
    fn a_failed_read_denies() {
        assert_eq!(
            scripted(
                &[AnswerWait::Ready],
                &[Err(io::Error::from(io::ErrorKind::BrokenPipe))],
            ),
            AnswerRead::NoAnswer
        );
    }

    // The Windows half of #1131, graded on every platform. A console input
    // handle is signalled by any record; only a key-down carrying a terminator
    // means a line is actually there to read.
    #[test]
    fn console_readiness_means_a_line_not_merely_an_event() {
        // A key-down `y` with no Enter behind it: the exact raw-mode partial
        // line, and the exact thing the old WaitForSingleObject arm called
        // Ready.
        assert!(
            !console_line_ready(&[u16::from(b'y')], false),
            "a keystroke that is not a whole line must not report ready"
        );
        // Non-key records (mouse, focus, resize) contribute no characters at
        // all, so an empty batch is the shape they arrive in.
        assert!(
            !console_line_ready(&[], false),
            "a signal carrying no key-down records at all must not report ready"
        );
        assert!(
            console_line_ready(&[u16::from(b'y'), u16::from(b'\r')], false),
            "Enter (CR) completes the line"
        );
        assert!(
            console_line_ready(&[u16::from(b'\n')], false),
            "LF completes the line too"
        );
        // Peek window too small to see the whole queue: somebody is typing a
        // lot, so keep the historical answer rather than timing them out.
        assert!(
            console_line_ready(&[u16::from(b'y')], true),
            "a full peek window keeps the historical ready answer"
        );
    }

    // Both readers must answer the same keystroke the same way; `Quit` and
    // `always` are the two arms with side effects beyond the return value.
    #[test]
    fn the_bounded_and_blocking_readers_share_one_mapping() {
        for (answer, expected) in [
            ("y", ConfirmResult::Approved),
            ("", ConfirmResult::Approved),
            ("n", ConfirmResult::Denied),
            ("q", ConfirmResult::Quit),
        ] {
            let mut bounded = ToolConfirmer::new(false, vec![]);
            let mut blocking = ToolConfirmer::new(false, vec![]);
            let line = format!("{answer}\n");
            assert_eq!(
                bounded.decide_from_line("Bash", answer),
                expected,
                "bounded reader disagreed on {answer:?}"
            );
            assert_eq!(
                blocking.decide_from_answer("Bash", &mut io::Cursor::new(line.into_bytes())),
                expected,
                "blocking reader disagreed on {answer:?}"
            );
        }

        let mut bounded = ToolConfirmer::new(false, vec![]);
        assert_eq!(
            bounded.decide_from_line("Bash", "a"),
            ConfirmResult::Approved
        );
        assert!(
            bounded.allow_list.contains("Bash"),
            "`always` must still grant the run-long allowance"
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
