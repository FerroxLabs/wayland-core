//! v0.7.0 Task 3.C.4: interactive permission prompt.
//!
//! When the runtime tries to dispatch a tool whose call has no learned
//! decision (LearnedPolicy::evaluate returned Ask), the engine calls
//! [`prompt_for_decision`] which asks the user via a numbered-choice
//! menu and feeds the answer back to `LearnedPolicy::record`.
//!
//! Arrow-key navigation requires a real TUI library; we don't have one
//! and won't pull one in for v0.7.0. Numbered choice on stdin is the
//! v0.7.0 deliverable; v0.8 can swap in crossterm-driven arrow keys.

use std::io::{self, BufRead, Write};

use wcore_permissions::learning::{LearnedDecision, LearnedPolicy};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptOutcome {
    /// User picked a decision. The caller persists via
    /// `LearnedPolicy::record` and then evaluates the dispatch.
    Decided(LearnedDecision),
    /// User pressed Ctrl-D / EOF or otherwise aborted. The caller must
    /// treat this as DenyOnce + do NOT persist.
    Aborted,
}

/// Render the prompt and read the answer.
///
/// `tool` and `argv` are shown to the user verbatim — they should already
/// be shell-quoted by the caller. Returns the picked decision, or `Aborted`
/// on EOF.
pub fn prompt_for_decision<R, W>(
    reader: &mut R,
    writer: &mut W,
    tool: &str,
    argv: &str,
) -> io::Result<PromptOutcome>
where
    R: BufRead,
    W: Write,
{
    writeln!(writer, "--- permission required ---")?;
    writeln!(writer, "tool: {tool}")?;
    if !argv.is_empty() {
        writeln!(writer, "args: {argv}")?;
    }
    writeln!(writer, "  1) allow once")?;
    writeln!(writer, "  2) allow always (for this tool + args pattern)")?;
    writeln!(writer, "  3) deny once")?;
    writeln!(writer, "  4) deny always (for this tool + args pattern)")?;
    write!(writer, "choose [1-4]: ")?;
    writer.flush()?;

    let mut line = String::new();
    let read = reader.read_line(&mut line)?;
    if read == 0 {
        // EOF
        return Ok(PromptOutcome::Aborted);
    }
    match line.trim() {
        "1" => Ok(PromptOutcome::Decided(LearnedDecision::AllowOnce)),
        "2" => Ok(PromptOutcome::Decided(LearnedDecision::AllowAlways)),
        "3" => Ok(PromptOutcome::Decided(LearnedDecision::DenyOnce)),
        "4" => Ok(PromptOutcome::Decided(LearnedDecision::DenyAlways)),
        other => {
            writeln!(
                writer,
                "unrecognised choice '{other}' — treating as deny once."
            )?;
            Ok(PromptOutcome::Decided(LearnedDecision::DenyOnce))
        }
    }
}

/// Convenience: prompt the user, then persist the answer in `policy`.
/// Records the answer with the given `arg_pattern` (None means
/// "any args for this tool"; pass `Some("git *")` etc. for patterns).
/// Returns the recorded decision so the caller can immediately honour
/// it or call `LearnedPolicy::evaluate` to re-check.
pub fn prompt_and_record<R, W>(
    reader: &mut R,
    writer: &mut W,
    policy: &mut LearnedPolicy,
    tool: &str,
    argv: &str,
    arg_pattern: Option<String>,
) -> io::Result<PromptOutcome>
where
    R: BufRead,
    W: Write,
{
    let outcome = prompt_for_decision(reader, writer, tool, argv)?;
    if let PromptOutcome::Decided(decision) = &outcome {
        policy.record(tool.to_string(), arg_pattern, decision.clone());
    }
    Ok(outcome)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drive(input: &str) -> (PromptOutcome, String) {
        let mut reader = io::Cursor::new(input.to_string());
        let mut writer = Vec::new();
        let outcome =
            prompt_for_decision(&mut reader, &mut writer, "Bash", "git status").expect("drive");
        let out_str = String::from_utf8(writer).expect("utf-8");
        (outcome, out_str)
    }

    #[test]
    fn prompt_choice_1_is_allow_once() {
        let (out, _) = drive("1\n");
        assert_eq!(out, PromptOutcome::Decided(LearnedDecision::AllowOnce));
    }

    #[test]
    fn prompt_choice_2_is_allow_always() {
        let (out, _) = drive("2\n");
        assert_eq!(out, PromptOutcome::Decided(LearnedDecision::AllowAlways));
    }

    #[test]
    fn prompt_choice_3_is_deny_once() {
        let (out, _) = drive("3\n");
        assert_eq!(out, PromptOutcome::Decided(LearnedDecision::DenyOnce));
    }

    #[test]
    fn prompt_choice_4_is_deny_always() {
        let (out, _) = drive("4\n");
        assert_eq!(out, PromptOutcome::Decided(LearnedDecision::DenyAlways));
    }

    #[test]
    fn prompt_eof_aborts() {
        let (out, _) = drive("");
        assert_eq!(out, PromptOutcome::Aborted);
    }

    #[test]
    fn prompt_garbage_falls_back_to_deny_once() {
        let (out, text) = drive("banana\n");
        assert_eq!(out, PromptOutcome::Decided(LearnedDecision::DenyOnce));
        assert!(text.contains("unrecognised"));
    }

    #[test]
    fn prompt_renders_tool_and_args() {
        let (_, text) = drive("1\n");
        assert!(text.contains("tool: Bash"));
        assert!(text.contains("args: git status"));
    }

    #[test]
    fn prompt_and_record_persists_decision() {
        let mut policy = LearnedPolicy::new();
        let mut reader = io::Cursor::new("2\n".to_string());
        let mut writer = Vec::new();
        let out = prompt_and_record(
            &mut reader,
            &mut writer,
            &mut policy,
            "Bash",
            "git status",
            Some("git *".to_string()),
        )
        .expect("prompt_and_record");
        assert_eq!(out, PromptOutcome::Decided(LearnedDecision::AllowAlways));
        // Re-evaluate from the updated policy.
        let eval = policy.evaluate("Bash", "git status");
        assert!(matches!(
            eval,
            wcore_permissions::learning::EvalResult::Match { allow: true, .. }
        ));
    }

    #[test]
    fn prompt_and_record_aborts_does_not_persist() {
        let mut policy = LearnedPolicy::new();
        let mut reader = io::Cursor::new(String::new());
        let mut writer = Vec::new();
        let out = prompt_and_record(
            &mut reader,
            &mut writer,
            &mut policy,
            "Bash",
            "rm -rf /",
            Some("*".to_string()),
        )
        .expect("aborts");
        assert_eq!(out, PromptOutcome::Aborted);
        assert!(policy.is_empty(), "aborted prompt must not persist");
    }
}
