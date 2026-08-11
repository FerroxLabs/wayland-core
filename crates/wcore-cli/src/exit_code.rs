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
//! | 130  | Interrupted (SIGINT / Ctrl-C). |
//! | 143  | Terminated (SIGTERM). |
//! | 129  | Hung up (SIGHUP). |
//!
//! 128 + N for signals is the shell convention, so `$?` reads the same as it
//! would for any other interrupted Unix program.

use wcore_types::message::StopReason;

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
/// A limit stop wins over a tool failure: "we ran out of turns" is the more
/// actionable fact, and the trailing tool state of a truncated run is not a
/// verdict on the task.
pub fn for_run_outcome(stop_reason: StopReason, unrecovered_tool_failure: bool) -> u8 {
    match stop_reason {
        StopReason::MaxTurns => LIMIT,
        StopReason::MaxTokens => OUTPUT_TRUNCATED,
        _ if unrecovered_tool_failure => TOOL_FAILURE,
        _ => OK,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clean_end_turn_is_zero() {
        assert_eq!(for_run_outcome(StopReason::EndTurn, false), OK);
    }

    #[test]
    fn an_unrecovered_tool_failure_is_distinguishable_from_success() {
        let failed = for_run_outcome(StopReason::EndTurn, true);
        assert_eq!(failed, TOOL_FAILURE);
        assert_ne!(
            failed,
            for_run_outcome(StopReason::EndTurn, false),
            "a failed run must not share the success code"
        );
    }

    #[test]
    fn a_limit_stop_is_its_own_code_and_outranks_the_tool_state() {
        assert_eq!(for_run_outcome(StopReason::MaxTurns, false), LIMIT);
        assert_eq!(for_run_outcome(StopReason::MaxTurns, true), LIMIT);
        assert_ne!(LIMIT, TOOL_FAILURE);
        assert_ne!(LIMIT, OK);
    }

    /// An output-cap truncation and a turn-cap stop are unrelated events. They
    /// shared code 4, so a caller learnt only "some limit" — not which, and
    /// not that its deliverable might be half-written.
    #[test]
    fn an_output_cap_truncation_is_not_the_turn_cap() {
        assert_eq!(
            for_run_outcome(StopReason::MaxTokens, false),
            OUTPUT_TRUNCATED
        );
        assert_eq!(
            for_run_outcome(StopReason::MaxTokens, true),
            OUTPUT_TRUNCATED,
            "a limit stop still outranks the trailing tool state"
        );
        assert_ne!(OUTPUT_TRUNCATED, LIMIT);
        assert_ne!(OUTPUT_TRUNCATED, OK);
        assert_ne!(OUTPUT_TRUNCATED, TOOL_FAILURE);
    }

    #[test]
    fn every_documented_code_is_distinct() {
        let codes = [
            OK,
            FAILURE,
            TOOL_FAILURE,
            LIMIT,
            OUTPUT_TRUNCATED,
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
    }

    #[test]
    fn signal_codes_follow_the_shell_convention() {
        assert_eq!(ShutdownSignal::Interrupt.exit_code(), 130);
        assert_eq!(ShutdownSignal::Terminate.exit_code(), 143);
        assert_eq!(ShutdownSignal::Hangup.exit_code(), 129);
    }
}
