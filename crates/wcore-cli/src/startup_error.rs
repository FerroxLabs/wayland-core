//! The single chokepoint that guarantees a `--json-stream` host is told WHY
//! the engine refused to start.
//!
//! # The defect this closes
//!
//! Over `--json-stream` the host (the Wayland desktop app) reads **stdout** and
//! nothing else. A startup refusal used to return `Err` all the way out of
//! `run()`, where `anyhow` printed it to **stderr** — a channel the protocol
//! consumer does not read. The host saw a pipe that opened and closed carrying
//! zero bytes, and rendered a spinner or a crash instead of the reason.
//!
//! Measured on `hetzner-dsm` before this module existed (debug build, five
//! conditions, positive control in the same run):
//!
//! | condition | rc | stdout | frames |
//! |---|---|---|---|
//! | healthy start (control) | 0 | 4480 B | 27 incl. `ready` |
//! | `credentials.backend = "plaintext"` + durable sessions | 1 | **0 B** | **0** |
//! | corrupt `config.toml` | 1 | **0 B** | **0** |
//! | `--profile` without `WAYLAND_HOME` | 1 | **0 B** | **0** |
//! | no API key | 1 | 496 B | 1 (`init_failed`) |
//!
//! Only the last one emitted, because issue #186 had patched that ONE call
//! site. Patching the other three individually would leave the next `?` added
//! to the startup path silently broken again, so this is a chokepoint instead:
//! it sits at process exit and fires for **any** error that escapes `run()`
//! before the `ready` frame goes out, including paths nobody has enumerated.
//!
//! # Scope — stated precisely
//!
//! This covers **pre-`ready` startup refusals only**. Once `ready` has been
//! emitted the session is live, the engine owns its own error reporting through
//! the protocol sink, and a fatal that escapes afterwards is a different
//! problem with a different owner. [`mark_ready_emitted`] draws that line, and
//! the chokepoint deliberately stays silent on the far side of it rather than
//! double-reporting an error the sink already sent.
//!
//! # Frame shape
//!
//! No new frame and no new field: this reuses [`ProtocolEvent::Error`] with
//! `msg_id: None` — the established session-level error shape, pinned by
//! `crates/wcore-protocol/tests/golden_v0_1_21.rs` — and the `init_failed`
//! code that the #186 site already emits for exactly this class. The `ready`
//! frame is untouched.

use std::sync::atomic::{AtomicBool, Ordering};

use wcore_protocol::events::{ErrorInfo, ProtocolEvent};
use wcore_protocol::writer::{ProtocolEmitter, ProtocolWriter};

/// The error code carried by every startup refusal frame.
///
/// Deliberately the SAME code the pre-existing #186 emit sites use: a host that
/// already handles `init_failed` handles the newly-covered refusals with no
/// change, which is the whole point of not inventing a frame.
pub const STARTUP_ERROR_CODE: &str = "init_failed";

/// Set once `--json-stream` is known to be active. Nothing is ever written to
/// stdout when this is false — a human-facing run must not gain JSON noise.
static JSON_STREAM_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set once the `ready` frame has gone out. After this the session is live and
/// the chokepoint stands down (see "Scope" above).
static READY_EMITTED: AtomicBool = AtomicBool::new(false);

/// Set by whichever site emits the refusal frame first, so the chokepoint can
/// never duplicate a frame that a more specific site already sent.
static STARTUP_ERROR_EMITTED: AtomicBool = AtomicBool::new(false);

/// Record that this process is speaking the JSON stream protocol on stdout.
///
/// Called immediately after argument parsing, before any fallible startup work.
pub fn mark_json_stream_active() {
    JSON_STREAM_ACTIVE.store(true, Ordering::SeqCst);
}

/// Record that the `ready` frame has been emitted and startup therefore
/// succeeded. Everything after this belongs to the live session.
pub fn mark_ready_emitted() {
    READY_EMITTED.store(true, Ordering::SeqCst);
}

/// Claim the exclusive right to emit the startup refusal frame.
///
/// Returns `true` for the first caller only. The pre-existing #186 sites call
/// this so that they keep their more specific message and the chokepoint below
/// stays quiet, rather than the host receiving the same failure twice.
pub fn claim_startup_error_emission() -> bool {
    STARTUP_ERROR_EMITTED
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
}

/// The decision itself, as a pure function of the three flags.
///
/// Split out from the atomics so it can be tested directly. Testing a local
/// re-implementation of this rule instead would be a tautology — it would pass
/// whatever the shipped rule actually did.
fn refusal_is_owed(json_stream: bool, ready_emitted: bool, already_reported: bool) -> bool {
    json_stream && !ready_emitted && !already_reported
}

/// Whether a startup refusal frame still needs to be sent for `--json-stream`.
///
/// False when this is not a protocol run, when startup already succeeded
/// (`ready` went out), or when a more specific site already reported.
fn refusal_frame_is_owed() -> bool {
    refusal_is_owed(
        JSON_STREAM_ACTIVE.load(Ordering::SeqCst),
        READY_EMITTED.load(Ordering::SeqCst),
        STARTUP_ERROR_EMITTED.load(Ordering::SeqCst),
    )
}

/// Emit the startup refusal frame for an error escaping `run()`, if one is owed.
///
/// This is the last thing that happens before the process exits, so it also
/// flushes: the pump delivers on exit in practice (measured — the #186 site's
/// 496 bytes arrive), but a bounded flush removes the dependence on that timing
/// entirely. A failed flush is swallowed because there is no second channel
/// left to report it on, and the process is already exiting non-zero.
pub fn report_startup_refusal(err: &anyhow::Error) {
    if !refusal_frame_is_owed() {
        return;
    }
    if !claim_startup_error_emission() {
        return;
    }
    let writer = ProtocolWriter::new();
    let _ = writer.emit(&ProtocolEvent::Error {
        msg_id: None,
        error: ErrorInfo {
            code: STARTUP_ERROR_CODE.to_string(),
            // `{:#}` renders the full anyhow chain, so a refusal raised deep in
            // bootstrap keeps the specific cause rather than the outermost
            // context line. This is the same rendering the #186 sites use.
            message: format!("Engine failed to start: {err:#}"),
            retryable: false,
            // wayland#1237: the local process failed to start.
            category: wcore_protocol::events::FailureCategory::LocalWayland,
        },
    });
    let _ = writer.flush_bounded();
}

#[cfg(test)]
mod tests {
    use super::*;

    // These exercise the SHIPPED decision function, not a copy of it. The real
    // wiring is proven end-to-end against the shipped binary by
    // `crates/wcore-cli/tests/json_stream_startup_refusal.rs`, which reads the
    // child's stdout exactly as the host does — that is the test that matters,
    // because reading it from anywhere else is the defect itself.

    #[test]
    fn refusal_is_owed_only_for_a_protocol_run_that_never_reached_ready() {
        assert!(
            refusal_is_owed(true, false, false),
            "protocol run that failed before ready must owe the host a frame"
        );
    }

    #[test]
    fn no_refusal_frame_on_a_human_facing_run() {
        // A TUI/REPL user must never see a JSON line spliced into their output.
        assert!(!refusal_is_owed(false, false, false));
    }

    #[test]
    fn no_refusal_frame_once_ready_has_gone_out() {
        // Past `ready` the session is live and the sink owns error reporting.
        assert!(!refusal_is_owed(true, true, false));
    }

    #[test]
    fn no_duplicate_when_a_more_specific_site_already_reported() {
        assert!(!refusal_is_owed(true, false, true));
    }

    #[test]
    fn claim_is_exclusive() {
        // Exactly one site may report, so the host never sees the same startup
        // failure twice. Whether THIS test wins the process-global claim depends
        // on test ordering; what must always hold is that a claim following any
        // other claim loses.
        let _first = claim_startup_error_emission();
        assert!(
            !claim_startup_error_emission(),
            "a claim following an earlier claim must always lose"
        );
    }

    #[test]
    fn startup_code_matches_the_pre_existing_186_code() {
        // Reusing the established code is what lets an existing host handle the
        // newly-covered refusals with no change.
        assert_eq!(STARTUP_ERROR_CODE, "init_failed");
    }
}
