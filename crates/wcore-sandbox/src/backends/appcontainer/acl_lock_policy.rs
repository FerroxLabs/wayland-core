//! Timeout, retry, and contention-reporting policy for the AppContainer ACL
//! mutation lock.
//!
//! Compiled on every platform for the same reason `windows_cmdline` is: this is
//! pure logic, and it is the difference between the bare
//! "timed out acquiring AppContainer ACL mutation lock" — a message that names
//! neither a contender nor a remedy — and a failure the operator can act on.
//! The Win32 wait itself stays in `acl_lease::mutation_lock`; this module owns
//! only the policy around it, so the policy is provable off Windows. Same split
//! as `windows_impl::process::probe_with_retry`, which separates its retry
//! policy from the Win32 spawn for exactly that reason.

// Per-target, exactly as in `windows_cmdline`: every production caller is
// `#[cfg(windows)]`, so off Windows these read as dead. Narrowed to
// non-Windows so genuinely dead policy code is still caught where it runs.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Operator override for the acquisition budget, in whole seconds.
pub(crate) const ACL_LOCK_TIMEOUT_ENV: &str = "WAYLAND_SANDBOX_ACL_LOCK_TIMEOUT_SECS";

/// The historical hard-coded budget, kept as the default so an operator who
/// sets nothing sees the behaviour they saw before this knob existed.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(15);
const MIN_TIMEOUT_SECS: u64 = 1;
const MAX_TIMEOUT_SECS: u64 = 300;

/// How many waits the budget is split into.
///
/// The attempts SLICE the budget, they do not multiply it: total wall clock
/// stays bounded by `timeout_from`, whatever the attempt count. Multiplying
/// would silently break the caller's own guard — the availability probe upstream
/// gives the whole of setup a 15 s `PROBE_WALL_CLOCK`, so a 3 x 15 s wait here
/// could never finish before that fired.
///
/// The point of splitting at all is that a real Win32 failure (as opposed to a
/// contended timeout) stops the wait immediately instead of burning the rest of
/// the budget, and each expiry is a place to observe who is holding the lock.
/// There is deliberately NO sleep between attempts: the wait itself is the
/// backoff, and stepping out of the kernel mutex queue to sleep would only
/// lengthen the contention this exists to survive.
pub(crate) const ACL_LOCK_ATTEMPTS: u32 = 3;

/// A slice is never zero, or the wait degenerates into a non-blocking poll.
const MIN_SLICE: Duration = Duration::from_millis(1);

/// Sidecar naming the process that currently holds the lock. A Win32 mutex
/// cannot report its owner, so the holder publishes itself.
const HOLDER_FILE: &str = "acl-lock-holder.txt";

/// Resolve the acquisition budget from the raw environment value.
///
/// Kept a pure function of the raw string so every branch — absent, garbage,
/// out of range — is unit-testable without an environment or Win32.
pub(crate) fn timeout_from(raw: Option<&str>) -> Duration {
    let Some(raw) = raw else {
        return DEFAULT_TIMEOUT;
    };
    match raw.trim().parse::<u64>() {
        Ok(secs) => Duration::from_secs(secs.clamp(MIN_TIMEOUT_SECS, MAX_TIMEOUT_SECS)),
        Err(_) => {
            tracing::warn!(
                target: "wcore_sandbox",
                env = ACL_LOCK_TIMEOUT_ENV,
                value = %raw,
                "ignoring unparseable AppContainer ACL lock timeout; using the default"
            );
            DEFAULT_TIMEOUT
        }
    }
}

/// What one Win32 wait told us, classified so the policy never sees a raw
/// `WAIT_*` value and this module needs no Windows types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WaitVerdict {
    /// The lock is ours (`WAIT_OBJECT_0`, or `WAIT_ABANDONED` from a holder
    /// that died while holding it).
    Acquired,
    /// The slice expired with someone else still holding it.
    Timeout,
    /// The wait itself failed. Never retried — a broken handle stays broken.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WaitOutcome {
    pub(crate) verdict: WaitVerdict,
    pub(crate) attempts: u32,
    pub(crate) waited: Duration,
}

/// Split the budget into equal slices.
fn attempt_slices(total: Duration) -> [Duration; ACL_LOCK_ATTEMPTS as usize] {
    let slice = (total / ACL_LOCK_ATTEMPTS).max(MIN_SLICE);
    [slice; ACL_LOCK_ATTEMPTS as usize]
}

/// Run the bounded wait, taking the wait as a parameter so the policy is
/// provable: a test cannot conjure a genuinely contended machine-wide mutex,
/// but it can assert that a timeout is retried and a Win32 failure is not.
pub(crate) fn wait_with_retry(
    total: Duration,
    mut wait: impl FnMut(Duration) -> WaitVerdict,
) -> WaitOutcome {
    let slices = attempt_slices(total);
    let mut waited = Duration::ZERO;
    for (index, slice) in slices.iter().copied().enumerate() {
        let verdict = wait(slice);
        waited += slice;
        let attempts = index as u32 + 1;
        if verdict != WaitVerdict::Timeout {
            return WaitOutcome {
                verdict,
                attempts,
                waited,
            };
        }
    }
    WaitOutcome {
        verdict: WaitVerdict::Timeout,
        attempts: ACL_LOCK_ATTEMPTS,
        waited,
    }
}

/// The process holding the lock, as it published itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LockHolder {
    pub(crate) pid: u32,
    pub(crate) exe: String,
}

fn holder_path(lease_directory: &Path) -> PathBuf {
    lease_directory.join(HOLDER_FILE)
}

fn encode_holder(pid: u32, exe: &str) -> String {
    format!("{pid} {}", exe.replace(['\r', '\n'], " "))
}

fn decode_holder(raw: &str) -> Option<LockHolder> {
    let line = raw.lines().next()?.trim();
    let (pid, exe) = line.split_once(' ').unwrap_or((line, ""));
    Some(LockHolder {
        pid: pid.parse().ok()?,
        exe: exe.trim().to_string(),
    })
}

/// Publish the current holder. Best effort by construction: the sidecar is
/// diagnostics, and a lock that could not be reported is still held.
pub(crate) fn publish_holder(lease_directory: &Path, pid: u32, exe: &str) {
    let _ = std::fs::write(holder_path(lease_directory), encode_holder(pid, exe));
}

/// Drop the sidecar on release. Also best effort — a leftover file is only
/// ever read after a timeout, and `read_holder` re-checks it then.
pub(crate) fn clear_holder(lease_directory: &Path) {
    let _ = std::fs::remove_file(holder_path(lease_directory));
}

/// Read the published holder, treating our own pid as no holder at all.
///
/// A sidecar naming this process is stale by definition — we are the one
/// waiting — and reporting ourselves as the contender would be worse than
/// reporting nothing.
pub(crate) fn read_holder(lease_directory: &Path, self_pid: u32) -> Option<LockHolder> {
    let holder = decode_holder(&std::fs::read_to_string(holder_path(lease_directory)).ok()?)?;
    (holder.pid != self_pid).then_some(holder)
}

/// The timeout failure, as the operator should read it.
///
/// The leading clause is unchanged on purpose: it is the string the teardown
/// annotation test asserts, and the string in the field report.
pub(crate) fn timeout_message(holder: Option<&LockHolder>, outcome: &WaitOutcome) -> String {
    let contender = match holder {
        Some(LockHolder { pid, exe }) if exe.is_empty() => {
            format!("another Wayland Core process (pid {pid}) on this machine holds it")
        }
        Some(LockHolder { pid, exe }) => {
            format!("another Wayland Core process (pid {pid}, {exe}) on this machine holds it")
        }
        None => "another process on this machine holds it (holder unknown)".to_string(),
    };
    format!(
        "timed out acquiring AppContainer ACL mutation lock: {contender}; waited {:.1}s across {} \
         attempts. Wait for it to finish, raise {ACL_LOCK_TIMEOUT_ENV}, or unset WAYLAND_SANDBOX \
         to use the default relaxed Windows backend.",
        outcome.waited.as_secs_f32(),
        outcome.attempts
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_absent_or_unparseable_override_keeps_the_historical_fifteen_seconds() {
        assert_eq!(timeout_from(None), Duration::from_secs(15));
        assert_eq!(timeout_from(Some("abc")), Duration::from_secs(15));
        assert_eq!(timeout_from(Some("")), Duration::from_secs(15));
        assert_eq!(timeout_from(Some("12x")), Duration::from_secs(15));
        assert_eq!(timeout_from(Some("-4")), Duration::from_secs(15));
    }

    #[test]
    fn the_operator_can_raise_the_budget() {
        assert_eq!(timeout_from(Some("45")), Duration::from_secs(45));
        assert_eq!(timeout_from(Some("  90  ")), Duration::from_secs(90));
    }

    #[test]
    fn the_budget_is_clamped_at_both_ends() {
        // Zero would make the wait a non-blocking poll, which fails instantly
        // under exactly the contention this knob exists for.
        assert_eq!(
            timeout_from(Some("0")),
            Duration::from_secs(MIN_TIMEOUT_SECS)
        );
        assert_eq!(
            timeout_from(Some("99999")),
            Duration::from_secs(MAX_TIMEOUT_SECS)
        );
    }

    #[test]
    fn retrying_never_exceeds_the_configured_budget() {
        // The guard this protects is upstream: the availability probe gives the
        // whole of setup 15 s, so attempts that MULTIPLIED the budget could
        // never finish inside it.
        for secs in [1, 5, 15, 300] {
            let total = Duration::from_secs(secs);
            let outcome = wait_with_retry(total, |_| WaitVerdict::Timeout);
            assert!(
                outcome.waited <= total,
                "{secs}s budget waited {:?}",
                outcome.waited
            );
        }
    }

    #[test]
    fn a_contended_wait_is_retried_to_the_attempt_limit() {
        let mut slices = Vec::new();
        let outcome = wait_with_retry(Duration::from_secs(15), |slice| {
            slices.push(slice);
            WaitVerdict::Timeout
        });
        assert_eq!(outcome.verdict, WaitVerdict::Timeout);
        assert_eq!(outcome.attempts, ACL_LOCK_ATTEMPTS);
        assert_eq!(slices.len(), ACL_LOCK_ATTEMPTS as usize);
    }

    #[test]
    fn a_lock_that_frees_up_mid_wait_is_acquired_without_further_attempts() {
        let mut calls = 0;
        let outcome = wait_with_retry(Duration::from_secs(15), |_| {
            calls += 1;
            if calls == 2 {
                WaitVerdict::Acquired
            } else {
                WaitVerdict::Timeout
            }
        });
        assert_eq!(outcome.verdict, WaitVerdict::Acquired);
        assert_eq!(outcome.attempts, 2);
        assert_eq!(calls, 2, "acquisition must stop the loop");
    }

    #[test]
    fn a_win32_failure_is_never_retried() {
        // The interesting case is the one a healthy host cannot conjure: a
        // broken handle must fail fast, not burn the operator's whole budget.
        let mut calls = 0;
        let outcome = wait_with_retry(Duration::from_secs(300), |_| {
            calls += 1;
            WaitVerdict::Failed
        });
        assert_eq!(outcome.verdict, WaitVerdict::Failed);
        assert_eq!(calls, 1, "a real wait failure must not be retried");
    }

    #[test]
    fn a_slice_is_never_zero() {
        for slice in attempt_slices(Duration::ZERO) {
            assert!(slice >= MIN_SLICE);
        }
    }

    #[test]
    fn the_timeout_message_names_the_contending_process_and_a_remedy() {
        let outcome = wait_with_retry(Duration::from_secs(15), |_| WaitVerdict::Timeout);
        let holder = LockHolder {
            pid: 4242,
            exe: r"C:\Program Files\WaylandCore\wayland.exe".to_string(),
        };
        let message = timeout_message(Some(&holder), &outcome);
        assert!(
            message.starts_with("timed out acquiring AppContainer ACL mutation lock"),
            "the teardown annotation asserts this leading clause: {message:?}"
        );
        assert!(
            message.contains("pid 4242"),
            "the contending process must be named: {message:?}"
        );
        assert!(
            message.contains("wayland.exe"),
            "the contending image must be named: {message:?}"
        );
        assert!(
            message.contains("3 attempts") && message.contains("15.0s"),
            "the operator must see what was actually waited: {message:?}"
        );
        assert!(
            message.contains(ACL_LOCK_TIMEOUT_ENV) && message.contains("WAYLAND_SANDBOX"),
            "a message with no remedy is the defect: {message:?}"
        );
    }

    #[test]
    fn an_unpublished_holder_still_yields_a_remedy_rather_than_a_dead_end() {
        let outcome = wait_with_retry(Duration::from_secs(15), |_| WaitVerdict::Timeout);
        let message = timeout_message(None, &outcome);
        assert!(
            message.contains("holder unknown"),
            "an absent sidecar must read as unknown, not as no contention: {message:?}"
        );
        assert!(message.contains(ACL_LOCK_TIMEOUT_ENV), "{message:?}");
    }

    #[test]
    fn a_published_holder_round_trips_through_the_sidecar() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_holder(dir.path(), 1), None, "nothing published yet");

        publish_holder(dir.path(), 4242, r"C:\wayland.exe");
        assert_eq!(
            read_holder(dir.path(), 1),
            Some(LockHolder {
                pid: 4242,
                exe: r"C:\wayland.exe".to_string()
            })
        );

        clear_holder(dir.path());
        assert_eq!(
            read_holder(dir.path(), 1),
            None,
            "the sidecar must not outlive the hold"
        );
    }

    #[test]
    fn a_sidecar_naming_this_process_is_reported_as_no_holder() {
        let dir = tempfile::tempdir().unwrap();
        publish_holder(dir.path(), 77, "self.exe");
        assert_eq!(
            read_holder(dir.path(), 77),
            None,
            "we are the waiter; naming ourselves as the contender is worse than saying nothing"
        );
    }

    #[test]
    fn a_corrupt_sidecar_is_holder_unknown_rather_than_an_error() {
        let dir = tempfile::tempdir().unwrap();
        for junk in ["", "not-a-pid path", "\n\n"] {
            std::fs::write(dir.path().join(HOLDER_FILE), junk).unwrap();
            assert_eq!(read_holder(dir.path(), 1), None, "junk: {junk:?}");
        }
    }

    #[test]
    fn a_holder_with_a_newline_in_its_path_cannot_forge_a_second_record() {
        let dir = tempfile::tempdir().unwrap();
        publish_holder(dir.path(), 5, "evil.exe\n99999 other.exe");
        let holder = read_holder(dir.path(), 1).unwrap();
        assert_eq!(holder.pid, 5);
        assert!(!holder.exe.contains('\n'));
    }
}
