//! R3(b) — the provider-failover receipt must reach a CLI/TUI operator.
//!
//! `OutputSink::emit_provider_failover_receipt` defaults to a no-op and was
//! overridden only by the JSON-stream `ProtocolSink`. A Desktop host therefore
//! saw which fallback candidates were refused and why; a terminal user saw
//! nothing at all. These tests drive the real `TerminalSink` and read the
//! bytes it actually wrote to fd 2.
//!
//! Unix-only: the harness redirects fd 2 with `dup2`, and `libc` is a
//! `cfg(unix)` dependency of this crate. The rendering itself is
//! platform-neutral and is covered on every platform by the unit tests on
//! `wcore_providers::describe_failover_receipt`.
#![cfg(unix)]

use std::io::Write;
use std::os::unix::io::AsRawFd;
use std::sync::{Mutex, MutexGuard, OnceLock};

use wcore_agent::output::{OutputSink, null_sink::NullSink, terminal::TerminalSink};
use wcore_providers::{
    CandidateReceipt, CandidateRejection, FailoverReason, FailoverReceipt, PricingEvidence,
};

/// fd 2 is process-wide, so only one capture may be installed at a time.
fn capture_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Run `body` with fd 2 pointed at a temp file; return everything written.
fn capture_stderr(body: impl FnOnce()) -> String {
    let _guard = capture_lock();
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("stderr.log");
    let file = std::fs::File::create(&path).expect("create capture file");

    let saved = unsafe { libc::dup(2) };
    assert!(saved >= 0, "dup(2) failed");
    assert!(
        unsafe { libc::dup2(file.as_raw_fd(), 2) } >= 0,
        "dup2 onto fd 2 failed"
    );

    body();
    let _ = std::io::stderr().flush();

    assert!(
        unsafe { libc::dup2(saved, 2) } >= 0,
        "restoring fd 2 failed"
    );
    unsafe { libc::close(saved) };

    std::fs::read_to_string(&path).expect("read capture file")
}

fn refused(provider: &str, model: &str, why: CandidateRejection) -> CandidateReceipt {
    CandidateReceipt {
        provider: provider.into(),
        model: model.into(),
        region: None,
        disposition: Err(why),
        failure_reason: None,
        cooldown_reason: None,
        retry_after_ms: None,
        pricing: PricingEvidence::default(),
    }
}

/// The harness itself must be live. Without this, an empty capture in the
/// tests below would be indistinguishable from a broken `dup2`.
///
/// The probe writes to fd 2 directly rather than through a sink, so its
/// success depends on the redirection alone and not on the code under test.
#[test]
fn the_stderr_capture_harness_records_a_write_to_fd_2() {
    let seen = capture_stderr(|| {
        let _ = writeln!(std::io::stderr(), "harness liveness probe");
    });
    assert!(
        seen.contains("harness liveness probe"),
        "capture harness is dead — it recorded {seen:?}"
    );
}

/// CAN-FAIL direction for the two tests below: a sink that leaves
/// `emit_provider_failover_receipt` at its trait default — which is what
/// EVERY sink but `ProtocolSink` did before this change — writes nothing at
/// all. Without this, the assertions above could be satisfied by ambient
/// output from anything in the process.
#[test]
fn a_default_impl_sink_still_prints_nothing_for_the_same_receipt() {
    let mut receipt =
        FailoverReceipt::new(FailoverReason::RateLimit, "anthropic", "claude-sonnet-4-6");
    receipt.candidates.push(refused(
        "openai",
        "gpt-5",
        CandidateRejection::ContextWindowUnknown,
    ));
    let json = serde_json::to_value(&receipt).expect("receipt serializes");

    let seen = capture_stderr(|| {
        NullSink.emit_provider_failover_receipt(json);
    });
    assert_eq!(
        seen, "",
        "the default trait impl wrote something, so the terminal assertions          below do not measure TerminalSink's override"
    );
}

#[test]
fn terminal_sink_names_every_refused_candidate_and_a_remedy() {
    let mut receipt =
        FailoverReceipt::new(FailoverReason::RateLimit, "anthropic", "claude-sonnet-4-6");
    receipt.candidates.push(refused(
        "openai",
        "gpt-5",
        CandidateRejection::ContextWindowUnknown,
    ));
    receipt.candidates.push(refused(
        "google",
        "gemini-3-pro",
        CandidateRejection::CooldownActive,
    ));
    let json = serde_json::to_value(&receipt).expect("receipt serializes");

    let seen = capture_stderr(|| {
        TerminalSink::new(true).emit_provider_failover_receipt(json);
    });

    for needle in [
        "anthropic",
        "claude-sonnet-4-6",
        "rate_limit",
        "openai",
        "gpt-5",
        "context_window_unknown",
        "google",
        "gemini-3-pro",
        "cooldown_active",
        "provider_chain",
    ] {
        assert!(
            seen.contains(needle),
            "the terminal receipt never mentioned {needle:?}; a CLI operator \
             cannot tell which candidate was refused or why. Got: {seen:?}"
        );
    }
}

#[test]
fn terminal_sink_names_the_fallback_it_actually_switched_to() {
    let mut receipt =
        FailoverReceipt::new(FailoverReason::Overloaded, "anthropic", "claude-opus-4-1");
    receipt.candidates.push(refused(
        "openai",
        "gpt-5-mini",
        CandidateRejection::ContextWindowTooSmall,
    ));
    receipt.candidates.push(CandidateReceipt {
        provider: "openai".into(),
        model: "gpt-5".into(),
        region: Some("us-east".into()),
        disposition: Ok(()),
        failure_reason: None,
        cooldown_reason: None,
        retry_after_ms: None,
        pricing: PricingEvidence::default(),
    });
    receipt.selected_provider = Some("openai".into());
    receipt.selected_model = Some("gpt-5".into());
    let json = serde_json::to_value(&receipt).expect("receipt serializes");

    let seen = capture_stderr(|| {
        TerminalSink::new(true).emit_provider_failover_receipt(json);
    });

    assert!(
        seen.contains("openai") && seen.contains("gpt-5"),
        "a silently rerouted turn told the operator nothing about the new \
         provider or model. Got: {seen:?}"
    );
    assert!(
        seen.contains("context_window_too_small"),
        "the candidate that was skipped on the way to the selection was not \
         named. Got: {seen:?}"
    );
    assert!(
        !seen.contains("provider_chain"),
        "a SUCCESSFUL failover printed the exhausted-chain remedy, which tells \
         the operator to reconfigure something that just worked. Got: {seen:?}"
    );
}
