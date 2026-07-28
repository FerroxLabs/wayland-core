//! The single-owner INBOUND POLLING lease.
//!
//! # The defect this closes
//!
//! `F24-C3-H4` closed a double [`ChannelManager`](wcore_channels::ChannelManager)
//! **within one process**: `gateway run` built its own manager and also let the
//! cron handler build a second one, so one process registered every adapter
//! twice and only one of the two managers carried a subscriber. Measured loss
//! was 8 of 8 at startup and 5 of 6 in steady state, silently.
//!
//! That fix is scoped to one process. It says nothing about a second one, and
//! **three production sites each construct a manager and call `start_all()`**:
//!
//! | site | reached by |
//! |---|---|
//! | `bootstrap.rs` | EVERY ordinary `wayland-core` session (`without_channels` is set only in tests and the per-session recursion guard) |
//! | `cron.rs` | `wayland-core cron daemon`, which ships launchd and systemd templates |
//! | `wcore-cli`'s `gateway.rs` | the installed service |
//!
//! Before this module there was no cross-process exclusion anywhere in the
//! channel stack.
//!
//! # Why a second poller DESTROYS rather than duplicates
//!
//! Inbound polling is a **destructive read**. Telegram's `getUpdates?offset=N`
//! permanently deletes every update below `N` — for every consumer, not just
//! the caller. IMAP `FETCH` sets `\Seen`. Discord allows one gateway session
//! per token. So whichever process wins the poll takes delivery *away* from the
//! other, and the loser is never told: no error, no warning, no retry.
//!
//! Measured on the shipped binary at `0.12.25` against a consume-on-read
//! endpoint, before this module existed:
//!
//! - **startup** — eight updates pending, an ordinary one-shot session started,
//!   and on its SECOND poll it confirmed `offset=9`, deleting updates 1-8. The
//!   installed service, started afterwards, received **0 of 8**.
//! - **steady state** — with the service alone the endpoint saw a maximum of
//!   **1** concurrent `getUpdates`; with an ordinary session open alongside it
//!   saw **2**, and the poll rate over equal windows went to **2.16x**.
//!
//! # The model, and why it is this crate's `ScheduleLease`
//!
//! Exactly one process is the OWNER of a home's inbound polling. Every other
//! process is an OBSERVER: it runs normally, it can still SEND, and it does not
//! poll.
//!
//! The exclusion is [`wcore_cron::lease::ScheduleLease`] — the lease Phase 24
//! already shipped for the cron schedule — reused through
//! [`ScheduleLease::attempt_named`] with channel-specific file names. It is
//! **not** a second mechanism, deliberately: two mechanisms for one invariant is
//! how the double-manager defect arose, and a lease with its own release story
//! would be a fresh opportunity to reintroduce the stale-lock wedge that
//! `ScheduleLease`'s OS-owned lock exists to prevent.
//!
//! That reuse buys the property that matters most here: **the lock is released
//! by the operating system when the holding descriptor closes**, including on
//! `SIGKILL`, on a panic and on power loss. Nothing has to run for the next
//! process to take over, so a dead holder cannot wedge inbound delivery
//! permanently. A lease that never released would convert message loss into
//! permanent unavailability — a strictly worse failure, and one this program
//! has already hit once elsewhere.
//!
//! # What the loser does, and why it is loud
//!
//! An observer **runs without inbound polling and says so at WARN, naming the
//! owner's pid**. It does not block (the loser is usually an interactive
//! session the user opened for unrelated work, and hanging it would be a worse
//! regression than the defect) and it does not exit (same reason).
//!
//! This choice was cross-audited three ways plus an internal adversarial pass;
//! all three external reviewers independently returned the same answer.
//!
//! The loudness is load-bearing rather than decorative. A second process that
//! silently has no channels is a NEW silent failure substituted for the old
//! one, and would be just as hard to diagnose. [`ChannelPollLease::observer`]
//! therefore emits a stable, greppable token on stderr as well as a tracing
//! event, so the condition is visible without `RUST_LOG` set.
//!
//! # Known residual: ownership is first-come, and that can starve the service
//!
//! Ownership is first-come-first-served. If an ordinary session takes the lease
//! and the installed service starts afterwards, the SERVICE becomes the
//! observer even though it is the intended owner, and it stays one until that
//! session exits.
//!
//! This is strictly better than the defect it replaces — today both poll and
//! messages are destroyed outright — and starvation is bounded by the
//! interactive session's lifetime rather than unbounded. It is nonetheless a
//! real gap, all three cross-audit reviewers independently graded it
//! `must-fix`, and **this module does not fix it**. See
//! `24-CHANNEL-LEASE.md` for the recommended follow-up (role-aware
//! reacquisition: a long-lived service that loses the lease retries in the
//! background and takes ownership the moment the session leaves, which needs no
//! new mechanism).

use std::path::{Path, PathBuf};

use wcore_cron::lease::{LeaseAttempt, ScheduleLease};

/// One-byte sentinel the OS lock is taken on. Distinct from the schedule's
/// `schedule.lock` so a home can own its schedule and its inbound polling
/// independently — they are separate concerns with separate lifetimes.
const CHANNEL_LOCK_FILE: &str = "channel-poll.lock";

/// Freely readable record naming the current polling owner. Never locked, so an
/// observer can identify the owner while the owner holds the lock — which is
/// what makes the WARN below able to name a pid.
const CHANNEL_RECORD_FILE: &str = "channel-poll.owner";

/// Stable, greppable token emitted when a process declines to poll. Operators
/// and tests match on this; it is deliberately not a prose sentence, because
/// prose gets reworded and a console line-wrap can split it — a defect this
/// program has measured twice.
pub const CHANNEL_LEASE_TOKEN: &str = "F24_CHANNEL_LEASE";

/// The role this process plays for one home's inbound polling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelPollRole {
    /// This process polls. It is the only one that may.
    Owner,
    /// This process does not poll. It still runs, and it may still send.
    Observer,
}

/// An owned claim on one home's inbound polling.
///
/// Hold this for as long as pollers should run. Dropping it — including by
/// process death — releases the OS lock, which is what lets the next process
/// take over without any timeout heuristic.
#[derive(Debug)]
pub struct ChannelPollLease {
    role: ChannelPollRole,
    owner_pid: Option<u32>,
    /// `None` for an observer. Held, never read, for an owner: its `Drop` is
    /// the release.
    _lease: Option<ScheduleLease>,
}

impl ChannelPollLease {
    pub fn role(&self) -> ChannelPollRole {
        self.role
    }

    pub fn is_owner(&self) -> bool {
        self.role == ChannelPollRole::Owner
    }

    /// The pid recorded as owning inbound polling, when one could be read.
    ///
    /// Diagnostic only. The refusal rests on the OS lock, never on this pid —
    /// an unrelated process that inherited a recycled identifier cannot hold
    /// the lock, so it cannot masquerade as the owner.
    pub fn owner_pid(&self) -> Option<u32> {
        self.owner_pid
    }

    /// An observer that never polls. Used where a lease could not be attempted
    /// at all.
    fn observer(owner_pid: Option<u32>) -> Self {
        Self {
            role: ChannelPollRole::Observer,
            owner_pid,
            _lease: None,
        }
    }
}

/// The directory a home's polling lease lives in.
///
/// `<home>/channels` — the same directory the channel configs are read from, so
/// two processes that read the same configs necessarily contend for the same
/// lease. The two sentinel files do not end in `.toml`, so neither
/// `auto_register_from_dir` nor `auto_register_from_user_config` can mistake
/// them for a channel.
pub fn lease_dir(home: &Path) -> PathBuf {
    home.join("channels")
}

/// Attempt to become the inbound-polling owner for `home`.
///
/// `holder` is a free-text description of the calling process kind
/// (`"gateway"`, `"session"`, `"cron-daemon"`) recorded for operator
/// diagnostics. It is self-reported and carries no authority.
///
/// Contention is **not** an error — it is the other valid role. This returns an
/// observer rather than failing, and never returns `Err`: a home whose lease
/// directory cannot be created or locked must not take the whole session down,
/// so such a failure degrades to observer and is reported at WARN. Refusing to
/// poll is always safe; polling when another process already is, is not.
pub fn attempt(home: &Path, holder: &str) -> ChannelPollLease {
    let dir = lease_dir(home);
    match ScheduleLease::attempt_named(&dir, holder, CHANNEL_LOCK_FILE, CHANNEL_RECORD_FILE) {
        Ok(LeaseAttempt::Owner(lease)) => {
            tracing::info!(
                target: "wcore_agent::channel_lease",
                holder,
                pid = std::process::id(),
                dir = %dir.display(),
                "{CHANNEL_LEASE_TOKEN}=owner — this process owns inbound polling for this home"
            );
            ChannelPollLease {
                role: ChannelPollRole::Owner,
                owner_pid: Some(std::process::id()),
                _lease: Some(lease),
            }
        }
        Ok(LeaseAttempt::Observer { holder_pid }) => {
            // LOUD. A second process that silently has no channels is a new
            // silent failure replacing the old one. This goes to stderr as well
            // as tracing so it is visible without RUST_LOG.
            let owner = holder_pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            tracing::warn!(
                target: "wcore_agent::channel_lease",
                holder,
                owner_pid = owner.as_str(),
                dir = %dir.display(),
                "{CHANNEL_LEASE_TOKEN}=observer — another process owns inbound polling; \
                 this process will NOT poll (sending is unaffected)"
            );
            eprintln!(
                "{CHANNEL_LEASE_TOKEN}=observer owner_pid={owner} holder={holder}: \
                 another wayland-core process is already receiving messages for this home; \
                 this one will not poll for inbound messages. Sending still works."
            );
            ChannelPollLease::observer(holder_pid)
        }
        Err(e) => {
            // Fail CLOSED. If ownership cannot be established it has not been
            // established, and polling anyway is the exact defect this module
            // exists to prevent.
            tracing::warn!(
                target: "wcore_agent::channel_lease",
                holder,
                error = %e,
                dir = %dir.display(),
                "{CHANNEL_LEASE_TOKEN}=unavailable — inbound polling lease could not be \
                 taken; this process will NOT poll"
            );
            eprintln!(
                "{CHANNEL_LEASE_TOKEN}=unavailable holder={holder}: inbound polling lease \
                 could not be taken ({e}); this process will not poll for inbound messages."
            );
            ChannelPollLease::observer(None)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_process_in_one_process_is_refused() {
        // `flock` is owned by the OPEN FILE DESCRIPTION, not the process, so two
        // attempts inside ONE test process genuinely conflict. Under `fcntl`
        // record locks they would merge and this test could never go red —
        // which is exactly why `ScheduleLease` uses `flock`, and why reusing it
        // rather than writing a second primitive matters.
        let home = tempfile::tempdir().unwrap();
        let first = attempt(home.path(), "gateway");
        assert!(first.is_owner(), "the first attempt must own");

        let second = attempt(home.path(), "session");
        assert_eq!(second.role(), ChannelPollRole::Observer);
        assert_eq!(
            second.owner_pid(),
            Some(std::process::id()),
            "the observer must be able to NAME the owner, or its warning is not actionable"
        );
    }

    #[test]
    fn dropping_the_owner_lets_the_next_process_poll() {
        // The takeover property. `Drop` here stands in for process death: the
        // OS releases the lock when the descriptor closes either way, which is
        // what stops a dead holder wedging inbound delivery forever.
        let home = tempfile::tempdir().unwrap();
        {
            let first = attempt(home.path(), "gateway");
            assert!(first.is_owner());
            let blocked = attempt(home.path(), "session");
            assert!(!blocked.is_owner(), "a live owner must exclude");
        }
        let after = attempt(home.path(), "session");
        assert!(
            after.is_owner(),
            "a released lease must be reclaimable, or loss becomes unavailability"
        );
    }

    #[test]
    fn the_lease_files_cannot_be_mistaken_for_channel_configs() {
        // `auto_register_from_dir` globs `*.toml`. If either sentinel ever
        // gained that extension it would be parsed as a channel and the
        // registration would fail for every channel in the home.
        assert!(!CHANNEL_LOCK_FILE.ends_with(".toml"));
        assert!(!CHANNEL_RECORD_FILE.ends_with(".toml"));

        let home = tempfile::tempdir().unwrap();
        let _l = attempt(home.path(), "gateway");
        let tomls: Vec<_> = std::fs::read_dir(lease_dir(home.path()))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
            .collect();
        assert!(
            tomls.is_empty(),
            "the lease must not leave anything a channel loader would try to parse"
        );
    }

    #[test]
    fn the_polling_lease_is_independent_of_the_schedule_lease() {
        // A home owns its schedule and its inbound polling separately. If these
        // shared a sentinel, taking one would silently deny the other, and a
        // gateway holding the schedule would stop any session polling for a
        // reason no operator could see.
        let home = tempfile::tempdir().unwrap();
        let dir = lease_dir(home.path());
        let poll = attempt(home.path(), "gateway");
        assert!(poll.is_owner());

        let schedule = ScheduleLease::attempt(&dir, "gateway").unwrap();
        assert!(
            schedule.is_owner(),
            "the schedule lease must not be blocked by the polling lease"
        );
    }
}
