//! Platform-free policy for the AppContainer ACL mutation lock (wayland#945).
//!
//! The lock itself is a Windows kernel mutex and can only live behind
//! `#[cfg(windows)]`. Its POLICY is not: the wait budget, the operator
//! override and its clamping, the holder-note format, and the wording of the
//! timeout message are ordinary arithmetic and string handling.
//!
//! They are split out here because `mutation_lock.rs` is reached only through
//! `#[cfg(windows)] mod appcontainer_acl_lease`, so anything written there is
//! not merely untested on Linux and macOS — it does not COMPILE there, and its
//! tests cannot fail on the two platforms CI runs on for free. wayland#945 is a
//! defect in exactly this policy (a 15 s budget that failed a healthy
//! neighbour), so the fix for it must be gradeable everywhere. What is left
//! behind the `cfg` is the part that genuinely needs Windows: taking the
//! `Global\` mutex, and asking the kernel whether a pid is still alive.

use std::time::Duration;

/// One wait slice. Unchanged from the timeout this lock originally shipped
/// with: long enough that an ordinary hold is never interrupted, short enough
/// that a stuck holder is re-identified while the caller is still waiting.
pub(crate) const MUTATION_LOCK_SLICE: Duration = Duration::from_secs(15);

/// Total time `MutationLock::acquire` waits before giving up.
///
/// **This is a raised DEFAULT, not only a new knob.** The shipped behaviour was
/// one 15 s wait with no retry. The phase this mutex serialises is
/// `SUB_CONTAINERS_AND_OBJECTS_INHERIT` propagation at ~100 µs per file under
/// every granted directory, paid once on grant and again on revoke — so one
/// execution holds the lock for roughly `files × 200 µs`: ~20 s over 100 000
/// files, ~40 s over 200 000. A checkout with a populated build directory is
/// routinely that size, which means the old default failed the SECOND process
/// during a completely healthy first one. That is wayland#945 exactly: two Core
/// processes on one Windows box, seven tests, every one of them on this
/// timeout.
///
/// 120 s is one worst-case hold of a ~600 000-file tree. Beyond that the holder
/// is not making progress and failing is the honest answer — which is why this
/// is a longer bound and not an unbounded wait.
pub(crate) const MUTATION_LOCK_DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

/// Operator override for [`MUTATION_LOCK_DEFAULT_TIMEOUT`], in whole seconds.
/// Values below one slice are raised to one slice (the wait is quantised) and
/// values above [`MUTATION_LOCK_MAX_TIMEOUT`] are capped.
pub(crate) const MUTATION_LOCK_TIMEOUT_ENV: &str = "WAYLAND_SANDBOX_ACL_LOCK_TIMEOUT_SECS";

/// Ceiling on the override. This lock is on the critical path of every
/// sandboxed child, so an operator asking for more than ten minutes has asked
/// for a hang rather than a wait.
pub(crate) const MUTATION_LOCK_MAX_TIMEOUT: Duration = Duration::from_secs(600);

/// Number of [`MUTATION_LOCK_SLICE`] waits that fit in the configured budget.
///
/// Always at least one, so a hostile or zeroed override can never turn the
/// lock into a non-blocking probe that fails every concurrent execution.
pub(crate) fn attempt_budget() -> u32 {
    let budget = configured_timeout();
    let slice = MUTATION_LOCK_SLICE.as_secs();
    budget.as_secs().div_ceil(slice).max(1) as u32
}

pub(crate) fn configured_timeout() -> Duration {
    let Some(raw) = std::env::var_os(MUTATION_LOCK_TIMEOUT_ENV) else {
        return MUTATION_LOCK_DEFAULT_TIMEOUT;
    };
    match raw
        .to_str()
        .map(str::trim)
        .and_then(|value| value.parse::<u64>().ok())
    {
        Some(secs) => {
            Duration::from_secs(secs).clamp(MUTATION_LOCK_SLICE, MUTATION_LOCK_MAX_TIMEOUT)
        }
        // Fall back rather than fail: an unparseable override must not make
        // sandboxed execution impossible.
        None => MUTATION_LOCK_DEFAULT_TIMEOUT,
    }
}

/// The process that most recently took the lock.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct HolderNote {
    pub(crate) pid: u32,
    pub(crate) image: String,
}

pub(crate) fn parse_holder_note(raw: &str) -> Option<HolderNote> {
    let mut lines = raw.lines();
    let pid = lines.next()?.trim().parse::<u32>().ok()?;
    let image = lines.next().unwrap_or_default().trim();
    Some(HolderNote {
        pid,
        image: if image.is_empty() {
            "unknown".to_string()
        } else {
            image.to_string()
        },
    })
}

/// The user-facing timeout message.
///
/// The old message was `timed out acquiring AppContainer ACL mutation lock` and
/// named no contender, no remedy and no bound — wayland#945 records that it
/// reads as a mystery hang rather than a lock conflict. This one answers all
/// three.
///
/// `holder_is_running` is the caller's liveness verdict on `holder.pid`, and is
/// meaningless when `holder` is `None`. It is a PARAMETER rather than a call to
/// the probe because the probe is `OpenProcess`, and threading it in is what
/// lets both the live-contender and the abandoned-note wordings be graded off
/// Windows.
pub(crate) fn contended_timeout_message(
    attempts: u32,
    holder: Option<HolderNote>,
    holder_is_running: bool,
) -> String {
    let waited = MUTATION_LOCK_SLICE.as_secs() * u64::from(attempts);
    let slice = MUTATION_LOCK_SLICE.as_secs();
    let who = render_holder(holder, holder_is_running);
    format!(
        "timed out acquiring the AppContainer ACL mutation lock after {waited}s \
         ({attempts} × {slice}s): {who}. Sandbox setup is serialised per Windows user, so \
         two Wayland Core processes running sandboxed commands on one machine take turns. \
         Wait for the other run to finish, or raise {MUTATION_LOCK_TIMEOUT_ENV} \
         (whole seconds, default {}, maximum {}).",
        MUTATION_LOCK_DEFAULT_TIMEOUT.as_secs(),
        MUTATION_LOCK_MAX_TIMEOUT.as_secs(),
    )
}

fn render_holder(holder: Option<HolderNote>, holder_is_running: bool) -> String {
    match holder {
        Some(holder) if holder_is_running => format!(
            "another Wayland Core process is holding it (pid {}, {})",
            holder.pid, holder.image
        ),
        Some(holder) => format!(
            "the last process to take it (pid {}, {}) is no longer running, so the lock was \
             probably abandoned mid-mutation",
            holder.pid, holder.image
        ),
        None => "no holder could be identified".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// wayland#945 (c). A knob whose default still fails is the old default
    /// with extra steps: 0.13.5 shipped exactly that mistake on the provider
    /// retry budget. The ASK here is the POLICY, so the default itself has to
    /// clear a realistic hold — see [`MUTATION_LOCK_DEFAULT_TIMEOUT`] for the
    /// ~100 µs/file × 2 arithmetic this number is derived from.
    #[test]
    fn the_default_budget_survives_a_large_workspace_hold() {
        assert!(
            MUTATION_LOCK_DEFAULT_TIMEOUT > MUTATION_LOCK_SLICE,
            "the default must be more than the single 15 s wait that wayland#945 \
             reported failing; got {MUTATION_LOCK_DEFAULT_TIMEOUT:?}"
        );
        // 200 000 files × 200 µs = 40 s of grant+revoke propagation for ONE
        // holder. A default that cannot outlast that fails a healthy first
        // process's second neighbour, which is the reported defect.
        assert!(
            MUTATION_LOCK_DEFAULT_TIMEOUT >= Duration::from_secs(40),
            "the default must outlast one worst-case hold; got {MUTATION_LOCK_DEFAULT_TIMEOUT:?}"
        );
        assert!(MUTATION_LOCK_DEFAULT_TIMEOUT <= MUTATION_LOCK_MAX_TIMEOUT);
    }

    /// The budget quantises into whole slices and never collapses to zero.
    ///
    /// Serialised inside ONE test function because it mutates the process
    /// environment, which `cargo test` shares across threads.
    #[test]
    fn the_override_is_honoured_and_clamped() {
        let restore = std::env::var_os(MUTATION_LOCK_TIMEOUT_ENV);
        let cases = [
            // (override, expected attempts)
            (None, 8u32),              // default 120 s / 15 s
            (Some("30"), 2),           // exact multiple
            (Some("31"), 3),           // partial slice rounds UP
            (Some("0"), 1),            // clamped to one slice, never zero
            (Some("999999"), 40),      // clamped to the 600 s ceiling
            (Some("  45  "), 3),       // surrounding whitespace
            (Some("not-a-number"), 8), // unparseable falls back to default
            (Some(""), 8),             // empty falls back to default
        ];
        for (value, expected) in cases {
            match value {
                Some(value) => unsafe { std::env::set_var(MUTATION_LOCK_TIMEOUT_ENV, value) },
                None => unsafe { std::env::remove_var(MUTATION_LOCK_TIMEOUT_ENV) },
            }
            assert_eq!(
                attempt_budget(),
                expected,
                "override {value:?} must yield {expected} attempts"
            );
        }
        match restore {
            Some(value) => unsafe { std::env::set_var(MUTATION_LOCK_TIMEOUT_ENV, value) },
            None => unsafe { std::env::remove_var(MUTATION_LOCK_TIMEOUT_ENV) },
        }
    }

    #[test]
    fn a_holder_note_round_trips() {
        assert_eq!(
            parse_holder_note("4242\nwayland.exe"),
            Some(HolderNote {
                pid: 4242,
                image: "wayland.exe".to_string()
            })
        );
        // A note truncated by a crash mid-write still names the pid.
        assert_eq!(
            parse_holder_note("4242"),
            Some(HolderNote {
                pid: 4242,
                image: "unknown".to_string()
            })
        );
        // Garbage must not be reported as a contender.
        assert_eq!(parse_holder_note(""), None);
        assert_eq!(parse_holder_note("not-a-pid\nwayland.exe"), None);
    }

    /// wayland#945 (b). The shipped message named no contender, no remedy and
    /// no bound. Assert all three, because "timed out acquiring ... lock" on
    /// its own is what made this read as a mystery hang.
    #[test]
    fn the_timeout_message_names_the_contender_the_bound_and_the_remedy() {
        let message = contended_timeout_message(
            8,
            Some(HolderNote {
                pid: 4242,
                image: "wayland.exe".to_string(),
            }),
            true,
        );
        assert!(
            message.contains("4242") && message.contains("wayland.exe"),
            "the holder must be named: {message}"
        );
        assert!(
            message.contains("120s"),
            "the bound must be stated: {message}"
        );
        assert!(
            message.contains(MUTATION_LOCK_TIMEOUT_ENV),
            "the remedy must be named: {message}"
        );

        // An abandoned note must NOT claim someone is still holding the lock.
        let abandoned = contended_timeout_message(
            8,
            Some(HolderNote {
                pid: 4242,
                image: "wayland.exe".to_string(),
            }),
            false,
        );
        assert!(
            abandoned.contains("no longer running"),
            "a dead holder must be reported as abandoned, not as a live contender: {abandoned}"
        );
        assert!(
            !abandoned.contains("is holding it"),
            "the abandoned wording must not also claim a live holder: {abandoned}"
        );

        // No note at all: still bounded and still actionable.
        let anonymous = contended_timeout_message(8, None, false);
        assert!(anonymous.contains("no holder could be identified"));
        assert!(anonymous.contains(MUTATION_LOCK_TIMEOUT_ENV));
    }

    /// The stated wait must be the budget actually spent, not the configured
    /// ceiling: `attempts × slice`. Without this the message could name a bound
    /// the loop never honoured.
    #[test]
    fn the_stated_wait_is_the_budget_actually_spent() {
        assert!(contended_timeout_message(8, None, false).contains("after 120s (8 × 15s)"));
        assert!(contended_timeout_message(2, None, false).contains("after 30s (2 × 15s)"));
    }
}
