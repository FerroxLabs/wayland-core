//! The process exit-code contract for `wayland-core`.
//!
//! Before this module the code was blind to task outcome: a run whose last
//! tool call failed, a run stopped by the turn cap, and a run interrupted
//! mid-tool all exited `0` — indistinguishable from a clean success. Startup
//! and transport failures were already faithful, so a caller checking `$?`
//! learnt only whether the process had STARTED, never whether the work
//! happened.
//!
//! The values below are the whole contract. They are deliberately few and
//! each one is decidable from state the engine already owns — no code claims
//! knowledge the process does not have. In particular there is NO code for
//! "the model's answer was wrong": nothing in this process can verify that,
//! and inventing a code for it would be a worse lie than the silence it
//! replaced.
//!
//! | Code | Meaning |
//! |------|---------|
//! | 0    | The run completed: the model ended its turn and the last tool batch (if any) had no unrecovered error. |
//! | 1    | Startup, configuration, provider or transport failure — the run never produced an answer. |
//! | 2    | CLI usage error (`clap`). |
//! | 3    | The run ended on an UNRECOVERED tool failure: the last tool results before the model's final answer contained an error and the model made no further tool call. |
//! | 4    | The engine stopped the run at a limit (`max_turns`) instead of the model finishing. |
//! | 5    | The model's response was cut off by the provider's OUTPUT token cap (`finish_reason=length`) — the answer, or a tool call it was writing, is incomplete. |
//! | 6    | The run stopped because it needs a human and the outbound route to one is down. The session is durable; `--resume <id>` continues it. |
//! | 7    | The provider turn ended in an error (`finish_reason=error`) — the model never finished the turn, so the run has no verdict at all. |
//! | 8    | The run completed but produced NO answer text. Nothing was written to stdout for a caller to consume. |
//! | 130  | Interrupted (SIGINT / Ctrl-C). |
//! | 143  | Terminated (SIGTERM). |
//! | 129  | Hung up (SIGHUP). |
//!
//! 128 + N for signals is the shell convention, so `$?` reads the same as it
//! would for any other interrupted Unix program.

use wcore_types::message::{FinishReason, StopReason};

/// The run completed normally.
pub const OK: u8 = 0;
/// Startup / configuration / provider failure (`anyhow::Err` out of `run`).
pub const FAILURE: u8 = 1;
/// The run ended on a tool failure the model never recovered from.
pub const TOOL_FAILURE: u8 = 3;
/// The engine stopped the run at a limit rather than the model finishing.
pub const LIMIT: u8 = 4;
/// The provider cut the model off at its output-token cap. Distinct from
/// [`LIMIT`]: "the agent ran out of turns" and "the model was cut off
/// mid-answer" are unrelated events with different remedies (raise
/// `max_turns` vs. raise `max_tokens` / ask for smaller writes), and while
/// they shared one code no caller or harness could tell them apart.
pub const OUTPUT_TRUNCATED: u8 = 5;

/// The run stopped because it needs a human and could not reach one.
///
/// Row B-3. Distinct from [`TOOL_FAILURE`] on purpose: "the last tool call
/// errored" is the symptom, "there is nobody to ask and I will not act
/// unsupervised" is the diagnosis, and only the second one tells the operator
/// that the remedy is to fix the channel (or answer) and resume rather than to
/// debug a tool. Distinct from a hang for the reason this code exists at all:
/// before it, a run in this state did not exit — the process sat with a live
/// inbound poller until something killed it, so `$?` was 137/-9 and
/// indistinguishable from a crash.
pub const AWAITING_HUMAN: u8 = 6;

/// The provider turn ended in an error.
///
/// `FinishReason::Error` is what the provider layer reports when the API
/// returned an unrecognized stop signal, refused, or the engine never received
/// a `Done` event (a mid-stream error). It is NOT a verdict on the task: the
/// turn did not finish, so the softer readings below it — the human latch, the
/// trailing tool state, the presence of answer text — describe a turn that
/// never happened and cannot be trusted. Distinct from [`FAILURE`] because
/// that code means the process never got as far as a run at all, which is a
/// different thing to debug (config / auth / transport vs. this turn).
pub const PROVIDER_ERROR: u8 = 7;

/// The run ended without producing any answer text.
///
/// The remainder of the #946 corpus rows: a headless `-p` run whose every
/// tool call was denied, or whose model emitted only reasoning, ends its turn
/// cleanly and writes nothing to stdout — and exited `0`, so a script could
/// not tell a total no-op from a completed task. The TUI already says this out
/// loud (#1109, "ended without producing any answer"); this is the same fact
/// on the channel a caller actually reads. Ranked BELOW [`TOOL_FAILURE`]
/// deliberately: when the engine knows why nothing came back, the specific
/// diagnosis is the more useful one and this is the catch-all.
pub const NO_OUTPUT: u8 = 8;

/// Shell convention for a process killed by signal N.
const SIGNALLED_BASE: u8 = 128;
/// SIGINT.
pub const INTERRUPTED: u8 = SIGNALLED_BASE + 2;
/// SIGTERM.
pub const TERMINATED: u8 = SIGNALLED_BASE + 15;
/// SIGHUP.
pub const HUNG_UP: u8 = SIGNALLED_BASE + 1;

/// Which shutdown signal ended the process. `CtrlC` is the portable
/// (non-unix) name for the same interruption SIGINT represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownSignal {
    Interrupt,
    Terminate,
    Hangup,
}

impl ShutdownSignal {
    /// The exit code a shell expects for this signal.
    pub fn exit_code(self) -> u8 {
        match self {
            Self::Interrupt => INTERRUPTED,
            Self::Terminate => TERMINATED,
            Self::Hangup => HUNG_UP,
        }
    }
}

/// Map a completed run to its exit code.
///
/// `unrecovered_tool_failure` is decided by the engine from the conversation
/// itself: the most recent tool-result batch carried an error AND the model
/// answered instead of calling another tool. A tool failure the model went on
/// to recover from is NOT one — otherwise an agent that probes for a missing
/// file and moves on would report failure.
///
/// `finish_reason` is the protocol-level outcome of the model's LAST turn.
/// `FinishReason::Error` outranks every soft reading below it — see
/// [`PROVIDER_ERROR`].
///
/// `final_text` is the run's answer. An empty one is [`NO_OUTPUT`]: the
/// process finished having written nothing for the caller to consume, which
/// before this was indistinguishable from a successful run.
///
/// A limit stop wins over a tool failure: "we ran out of turns" is the more
/// actionable fact, and the trailing tool state of a truncated run is not a
/// verdict on the task.
///
/// `awaiting_human` is the engine's unreachable-human latch
/// (`AgentEngine::awaiting_human`) read at the end of the run: the last word
/// from the outbound human-contact route was a failure, so the dispatcher was
/// refusing every state-changing call. A limit stop still wins over it — a run
/// the engine cut short is not evidence about why the model would have
/// stopped.
pub fn for_run_outcome(
    stop_reason: StopReason,
    finish_reason: FinishReason,
    final_text: &str,
    unrecovered_tool_failure: bool,
    awaiting_human: bool,
) -> u8 {
    match stop_reason {
        StopReason::MaxTurns => LIMIT,
        StopReason::MaxTokens => OUTPUT_TRUNCATED,
        // Outranks every reading below it: those all describe HOW a completed
        // turn ended, and this says the turn did not complete.
        _ if finish_reason == FinishReason::Error => PROVIDER_ERROR,
        // Outranks TOOL_FAILURE: on the row this was measured on the two are
        // the SAME event (the failed sends are the trailing tool errors), and
        // of the two readings only this one names the remedy.
        _ if awaiting_human => AWAITING_HUMAN,
        _ if unrecovered_tool_failure => TOOL_FAILURE,
        // The catch-all remainder: the run ended cleanly and still handed the
        // caller nothing. Trimmed because whitespace is not an answer.
        _ if final_text.trim().is_empty() => NO_OUTPUT,
        _ => OK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pre-existing test in this module described a run that DID answer,
    /// so they all pass a real `final_text`. Passing "" would now be
    /// [`NO_OUTPUT`], which is the point of that code.
    const ANSWERED: &str = "here is the answer";

    #[test]
    fn a_clean_end_turn_is_zero() {
        assert_eq!(
            for_run_outcome(
                StopReason::EndTurn,
                FinishReason::Stop,
                ANSWERED,
                false,
                false
            ),
            OK
        );
    }

    #[test]
    fn an_unrecovered_tool_failure_is_distinguishable_from_success() {
        let failed = for_run_outcome(
            StopReason::EndTurn,
            FinishReason::Stop,
            ANSWERED,
            true,
            false,
        );
        assert_eq!(failed, TOOL_FAILURE);
        assert_ne!(
            failed,
            for_run_outcome(
                StopReason::EndTurn,
                FinishReason::Stop,
                ANSWERED,
                false,
                false
            ),
            "a failed run must not share the success code"
        );
    }

    #[test]
    fn a_limit_stop_is_its_own_code_and_outranks_the_tool_state() {
        assert_eq!(
            for_run_outcome(
                StopReason::MaxTurns,
                FinishReason::MaxTurns,
                ANSWERED,
                false,
                false
            ),
            LIMIT
        );
        assert_eq!(
            for_run_outcome(
                StopReason::MaxTurns,
                FinishReason::MaxTurns,
                ANSWERED,
                true,
                false
            ),
            LIMIT
        );
        assert_ne!(LIMIT, TOOL_FAILURE);
        assert_ne!(LIMIT, OK);
    }

    /// An output-cap truncation and a turn-cap stop are unrelated events. They
    /// shared code 4, so a caller learnt only "some limit" — not which, and
    /// not that its deliverable might be half-written.
    #[test]
    fn an_output_cap_truncation_is_not_the_turn_cap() {
        assert_eq!(
            for_run_outcome(
                StopReason::MaxTokens,
                FinishReason::Length,
                ANSWERED,
                false,
                false
            ),
            OUTPUT_TRUNCATED
        );
        assert_eq!(
            for_run_outcome(
                StopReason::MaxTokens,
                FinishReason::Length,
                ANSWERED,
                true,
                false
            ),
            OUTPUT_TRUNCATED,
            "a limit stop still outranks the trailing tool state"
        );
        assert_ne!(OUTPUT_TRUNCATED, LIMIT);
        assert_ne!(OUTPUT_TRUNCATED, OK);
        assert_ne!(OUTPUT_TRUNCATED, TOOL_FAILURE);
    }

    /// The whole point of code 6: a run that stopped for want of a human must
    /// not read as an ordinary success, nor as a plain tool failure, nor (the
    /// pre-fix behaviour) as a signal death.
    #[test]
    fn needing_a_human_is_not_success_and_not_a_bare_tool_failure() {
        let waiting = for_run_outcome(
            StopReason::EndTurn,
            FinishReason::Stop,
            ANSWERED,
            true,
            true,
        );
        assert_eq!(waiting, AWAITING_HUMAN);
        assert_ne!(waiting, OK);
        assert_ne!(waiting, TOOL_FAILURE);
        assert_ne!(
            waiting, 137,
            "the state this code names used to be reported as a SIGKILL"
        );
        // Same trailing tool state, latch clear: still the old answer.
        assert_eq!(
            for_run_outcome(
                StopReason::EndTurn,
                FinishReason::Stop,
                ANSWERED,
                true,
                false
            ),
            TOOL_FAILURE
        );
        // Latch armed with a clean tool tail still reports the latch.
        assert_eq!(
            for_run_outcome(
                StopReason::EndTurn,
                FinishReason::Stop,
                ANSWERED,
                false,
                true
            ),
            AWAITING_HUMAN
        );
    }

    /// A run the ENGINE cut short says nothing about whether the model would
    /// have ended needing a person, so the limit keeps precedence.
    #[test]
    fn a_limit_stop_outranks_the_human_latch() {
        assert_eq!(
            for_run_outcome(
                StopReason::MaxTurns,
                FinishReason::MaxTurns,
                ANSWERED,
                false,
                true
            ),
            LIMIT
        );
        assert_eq!(
            for_run_outcome(
                StopReason::MaxTokens,
                FinishReason::Length,
                ANSWERED,
                false,
                true
            ),
            OUTPUT_TRUNCATED
        );
    }

    /// #946 — a provider turn that ended in error must not read as success.
    /// The engine returns `Ok(AgentResult)` for this: the run mechanically
    /// completed, the TURN did not, and `stop_reason` alone cannot tell them
    /// apart because both are `EndTurn`.
    #[test]
    fn a_provider_error_turn_is_not_success() {
        let errored = for_run_outcome(
            StopReason::EndTurn,
            FinishReason::Error,
            ANSWERED,
            false,
            false,
        );
        assert_eq!(errored, PROVIDER_ERROR);
        assert_ne!(errored, OK);
        // It outranks the soft readings: they describe a turn that finished.
        assert_eq!(
            for_run_outcome(StopReason::EndTurn, FinishReason::Error, "", true, true),
            PROVIDER_ERROR
        );
    }

    /// #946 — the seven-row remainder: every tool denied, so the model ends
    /// its turn cleanly having written nothing. `stop_reason` is `EndTurn`,
    /// `finish_reason` is `Stop`, no tool error is recorded, and the run
    /// exited 0 — identical to a completed task.
    #[test]
    fn a_run_that_produced_no_answer_is_not_success() {
        let silent = for_run_outcome(StopReason::EndTurn, FinishReason::Stop, "", false, false);
        assert_eq!(silent, NO_OUTPUT);
        assert_ne!(silent, OK);
        // Whitespace is not an answer.
        assert_eq!(
            for_run_outcome(
                StopReason::EndTurn,
                FinishReason::Stop,
                "  \n ",
                false,
                false
            ),
            NO_OUTPUT
        );
        // NEGATIVE CONTROL: the same shape with real text is still a success.
        // Without this the fix could simply always return non-zero.
        assert_eq!(
            for_run_outcome(
                StopReason::EndTurn,
                FinishReason::Stop,
                ANSWERED,
                false,
                false
            ),
            OK
        );
        // A known cause outranks the catch-all.
        assert_eq!(
            for_run_outcome(StopReason::EndTurn, FinishReason::Stop, "", true, false),
            TOOL_FAILURE
        );
    }

    #[test]
    fn every_documented_code_is_distinct() {
        let codes = [
            OK,
            FAILURE,
            TOOL_FAILURE,
            LIMIT,
            OUTPUT_TRUNCATED,
            AWAITING_HUMAN,
            PROVIDER_ERROR,
            NO_OUTPUT,
            INTERRUPTED,
            TERMINATED,
            HUNG_UP,
        ];
        let unique: std::collections::BTreeSet<u8> = codes.iter().copied().collect();
        assert_eq!(
            unique.len(),
            codes.len(),
            "two outcomes share an exit code, so a caller cannot tell them apart: {codes:?}"
        );
        // 2 is clap's usage error and is never produced by this function.
        assert!(!codes.contains(&2));
    }

    #[test]
    fn signal_codes_follow_the_shell_convention() {
        assert_eq!(ShutdownSignal::Interrupt.exit_code(), 130);
        assert_eq!(ShutdownSignal::Terminate.exit_code(), 143);
        assert_eq!(ShutdownSignal::Hangup.exit_code(), 129);
    }
}
