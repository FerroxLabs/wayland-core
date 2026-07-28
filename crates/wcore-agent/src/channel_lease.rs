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
//! # Ownership is NOT first-come — the service outranks a session (F24-CS)
//!
//! The first version of this module decided ownership once, at boot, on a
//! first-come basis. That closed the loss and left a starvation gap that all
//! three cross-audit reviewers graded `must-fix`: a session that started first
//! made the INSTALLED SERVICE the observer, and it stayed one until that
//! session exited. Nothing was lost — the session received — but the thing the
//! user installed to be always-on went silently idle, and mail simply stopped
//! arriving. It is quieter than the defect it replaced, which makes it worse in
//! one specific way: nothing looks wrong.
//!
//! Ownership is now decided by ROLE, and re-decided continuously by
//! [`ChannelPollSupervisor`]:
//!
//! | role | `holder` | rank |
//! |---|---|---|
//! | the installed service | `gateway` | [`RANK_GATEWAY`] (30) |
//! | the scheduled-job daemon | `cron-daemon` | [`RANK_CRON_DAEMON`] (20) |
//! | an ordinary session | `session` | [`RANK_SESSION`] (10) |
//!
//! A gateway exists only because the user installed a unit to be always-on. A
//! session exists because somebody typed a command and will close the terminal.
//! The rule has to encode that intent; **arrival order encodes nothing.**
//! Ranking `cron-daemon` above `session` for the same reason also keeps the one
//! good property first-come had: a session started while no service is running
//! polls immediately, and cedes the moment a service appears.
//!
//! Anything not named above ranks at [`RANK_SESSION`], so an unrecognised
//! holder can never preempt anybody.
//!
//! # How precedence is enforced without a preemption primitive
//!
//! `flock`/`LockFileEx` has NO preemption: a claimant cannot seize a held lock,
//! and the holder cannot be forced to drop it. So "the service wins" is
//! necessarily a VOLUNTARY YIELD by the current holder, driven by an
//! **advisory claim**:
//!
//! - a process that wants to poll but is not the owner publishes
//!   `channel-poll.claim.<pid>` carrying its rank, and refreshes it every tick;
//! - an owner that sees a FRESH claim of strictly higher rank stops polling and
//!   drops the lease;
//! - every non-owner re-attempts the lease each tick and starts polling when it
//!   wins.
//!
//! **The claim is not a second exclusion concept, and that distinction is the
//! whole safety argument.** A claim cannot grant polling and cannot deny it —
//! only the lock does that. If claims are missing, unreadable, corrupt or
//! stale, the behaviour degrades to the previous first-come lease. It cannot
//! degrade toward two simultaneous pollers, because two pollers require two
//! holders of one `flock`.
//!
//! # Why freshness, and not a liveness check on the claimant's pid
//!
//! Two failure modes have to be closed at once, and one device closes both:
//!
//! - **wedge.** If an owner yields to a claimant that then dies, and the
//!   suppression is unconditional, NOBODY polls — a manufactured denial, which
//!   is strictly worse than the starvation being fixed.
//! - **oscillation.** If the yielding owner re-contends immediately, it can
//!   beat the higher-ranked claimant to the free lock and then yield again,
//!   indefinitely. Strict rank inequality alone does NOT prevent this; it only
//!   prevents equal-rank ping-pong.
//!
//! A claim therefore suppresses contention **only while it is fresh** — file
//! mtime within [`CLAIM_TTL_TICKS`]` * tick`. A live claimant refreshes every
//! tick and stays suppressive (no oscillation); a dead one stops refreshing,
//! goes stale within the TTL, and the ex-owner contends and wins (no wedge).
//!
//! All three reviewers asked instead for a pid-liveness check, and one asked
//! for pid plus process-start identity to defeat pid reuse. This module does
//! neither, deliberately. [`wcore_cron::lease`] already argues that a recorded
//! pid proves nothing; freshness needs no process identity at all, so the pid
//! reuse hazard the reviewers then had to patch around does not arise. The pid
//! in a claim is for the operator message and is never consulted for a
//! decision.
//!
//! # The bounded gap loses nothing
//!
//! A handover, and the worst-case `CLAIM_TTL_TICKS * tick` window after a
//! claimant dies mid-handover, are windows in which nobody polls. **Nothing is
//! lost in them.** Telegram retains updates until an `offset=` confirm, IMAP
//! retains until `\Seen`, and the Discord gateway replays on reconnect. The
//! cost of a gap is latency; the cost of a wedge would be silence. That
//! asymmetry is why the TTL exists.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, SystemTime};

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
    attempt_inner(home, holder, true)
}

/// [`attempt`] without the stderr announcements.
///
/// The supervisor re-attempts on every tick. Announcing each of those would
/// bury the transitions that matter in a per-tick stream, so it announces on
/// ROLE CHANGE only — see [`PollSupervisorState::tick`]. The tracing events are
/// still emitted, at their normal levels, on every attempt.
fn attempt_quiet(home: &Path, holder: &str) -> ChannelPollLease {
    attempt_inner(home, holder, false)
}

fn attempt_inner(home: &Path, holder: &str, announce: bool) -> ChannelPollLease {
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
            if announce {
                eprintln!(
                    "{CHANNEL_LEASE_TOKEN}=observer owner_pid={owner} holder={holder}: \
                     another wayland-core process is already receiving messages for this home; \
                     this one will not poll for inbound messages. Sending still works."
                );
            }
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
            if announce {
                eprintln!(
                    "{CHANNEL_LEASE_TOKEN}=unavailable holder={holder}: inbound polling lease \
                     could not be taken ({e}); this process will not poll for inbound messages."
                );
            }
            ChannelPollLease::observer(None)
        }
    }
}

// ───────────────────────────────────────────────────────────────────────────
// F24-CS — role precedence, advisory claims, and the supervisor that applies
// them. Everything below this line is the starvation fix.
// ───────────────────────────────────────────────────────────────────────────

/// The installed always-on service (`wayland-core gateway run`).
pub const RANK_GATEWAY: u8 = 30;
/// The scheduled-job daemon (`wayland-core cron daemon`).
pub const RANK_CRON_DAEMON: u8 = 20;
/// An ordinary interactive or one-shot session, and the floor for anything
/// unrecognised.
pub const RANK_SESSION: u8 = 10;

/// How many ticks a claim stays suppressive after its last refresh.
///
/// This is the ONLY bound on the wedge: an owner that yielded to a claimant
/// which then died resumes polling once the dead claimant's claim ages past
/// this. Three gives a live claimant two missed refreshes of slack before it is
/// declared gone, which matters on a loaded box.
pub const CLAIM_TTL_TICKS: u32 = 3;

/// Default supervisor cadence.
const DEFAULT_TICK_MS: u64 = 2_000;

/// Env override for the supervisor cadence, in milliseconds.
///
/// Exists so a test can drive handovers in seconds rather than minutes. It is
/// clamped, so a hostile or fat-fingered value cannot spin the loop or park it
/// forever.
pub const TICK_ENV: &str = "WAYLAND_CHANNEL_LEASE_TICK_MS";

/// Filename prefix of an advisory claim. Deliberately not a `.toml`, for the
/// same reason as the lock and record sentinels.
const CLAIM_PREFIX: &str = "channel-poll.claim.";

/// Rank of a `holder` string.
///
/// Unknown holders rank at [`RANK_SESSION`], the floor. A holder string is
/// self-reported and carries no authority, so the worst an unrecognised or
/// forged one can do is decline to preempt.
pub fn rank_of(holder: &str) -> u8 {
    match holder {
        "gateway" => RANK_GATEWAY,
        "cron-daemon" => RANK_CRON_DAEMON,
        _ => RANK_SESSION,
    }
}

/// The supervisor cadence, from [`TICK_ENV`] or [`DEFAULT_TICK_MS`], clamped to
/// `[50ms, 60s]`.
pub fn tick_interval() -> Duration {
    let ms = std::env::var(TICK_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(DEFAULT_TICK_MS)
        .clamp(50, 60_000);
    Duration::from_millis(ms)
}

/// An advisory claim on inbound polling. It grants nothing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PollClaim {
    pub pid: u32,
    pub rank: u8,
    pub holder: String,
}

fn claim_path(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{CLAIM_PREFIX}{pid}"))
}

/// Publish or refresh this process's claim. Best-effort: a claim that cannot be
/// written costs its owner precedence, never correctness.
fn publish_claim(dir: &Path, pid: u32, rank: u8, holder: &str) {
    let claim = PollClaim {
        pid,
        rank,
        holder: holder.to_string(),
    };
    let Ok(body) = serde_json::to_string(&claim) else {
        return;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    // Temp + rename, so a reader never parses a half-written claim. A reader
    // that did would treat it as absent anyway — the safe direction — but a
    // torn read would also carry a misleading mtime.
    let tmp = dir.join(format!("{CLAIM_PREFIX}{pid}.tmp"));
    if std::fs::write(&tmp, body).is_err() {
        return;
    }
    let _ = std::fs::rename(&tmp, claim_path(dir, pid));
}

fn remove_claim(dir: &Path, pid: u32) {
    let _ = std::fs::remove_file(claim_path(dir, pid));
}

/// The highest-ranked FRESH claim in `dir`, ignoring `exclude_pid`'s own.
///
/// "Fresh" is `mtime` within `ttl`. Anything unreadable, unparseable, or whose
/// mtime cannot be compared is treated as ABSENT — the direction that leaves
/// the incumbent polling, never the direction that stops everybody.
fn best_live_claim(dir: &Path, ttl: Duration, exclude_pid: u32) -> Option<PollClaim> {
    let now = SystemTime::now();
    let mut best: Option<PollClaim> = None;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with(CLAIM_PREFIX) || name.ends_with(".tmp") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(mtime) = meta.modified() else { continue };
        // `duration_since` errors when mtime is in the future (clock skew, a
        // restored backup). Treat that as stale rather than as infinitely
        // fresh: an unbounded suppression is the wedge.
        match now.duration_since(mtime) {
            Ok(age) if age <= ttl => {}
            _ => continue,
        }
        let Ok(raw) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Ok(claim) = serde_json::from_str::<PollClaim>(&raw) else {
            continue;
        };
        if claim.pid == exclude_pid {
            continue;
        }
        if best.as_ref().is_none_or(|b| claim.rank > b.rank) {
            best = Some(claim);
        }
    }
    best
}

/// A boxed future the supervisor awaits. Deliberately not borrowing `self`, so
/// implementations clone their handle and the trait stays object-safe without
/// an `async_trait` dependency.
pub type PollFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

/// The polling surface the supervisor arms and disarms.
///
/// A trait rather than a concrete `ChannelManager` for one reason that pays for
/// itself immediately: the whole precedence state machine can then be tested
/// in-process, with two simulated processes, no timers and no network — which
/// is what makes the wedge and oscillation cases testable at all.
pub trait PollControl: Send + Sync + 'static {
    fn start_polling(&self) -> PollFuture;
    fn stop_polling(&self) -> PollFuture;
}

/// [`PollControl`] over the real [`wcore_channels::ChannelManager`].
pub struct ChannelManagerPollControl {
    manager: Arc<tokio::sync::RwLock<wcore_channels::ChannelManager>>,
}

impl ChannelManagerPollControl {
    pub fn new(manager: Arc<tokio::sync::RwLock<wcore_channels::ChannelManager>>) -> Arc<Self> {
        Arc::new(Self { manager })
    }
}

impl PollControl for ChannelManagerPollControl {
    fn start_polling(&self) -> PollFuture {
        let manager = Arc::clone(&self.manager);
        Box::pin(async move {
            if let Err(e) = manager.write().await.start_all().await {
                tracing::warn!(
                    target: "wcore_agent::channel_lease",
                    error = %e,
                    "start_all failed on lease acquisition; inbound polling may be partial"
                );
            }
        })
    }

    fn stop_polling(&self) -> PollFuture {
        let manager = Arc::clone(&self.manager);
        Box::pin(async move {
            if let Err(e) = manager.write().await.stop_all().await {
                tracing::warn!(
                    target: "wcore_agent::channel_lease",
                    error = %e,
                    "stop_all failed while yielding the lease"
                );
            }
        })
    }
}

/// What one supervisor tick did. Returned for tests and diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TickOutcome {
    /// Owner, and no claim outranked it.
    HeldOwner,
    /// Owner until this tick; stopped polling and released for a better claim.
    Yielded { to_pid: u32, to_rank: u8 },
    /// Observer that declined to contend because a fresh better claim exists.
    Deferred { to_pid: u32 },
    /// Observer until this tick; took the lease and started polling.
    Acquired,
    /// Observer, contended, and did not win.
    StillObserver,
}

#[derive(Debug, Default)]
struct SupervisorShared {
    is_owner: AtomicBool,
    /// Pid believed to own polling; `0` when unknown.
    owner_pid: AtomicU32,
    yields: AtomicU32,
    acquisitions: AtomicU32,
}

/// The precedence state machine, with every input injectable.
struct PollSupervisorState {
    home: PathBuf,
    dir: PathBuf,
    holder: String,
    rank: u8,
    pid: u32,
    ttl: Duration,
    /// `Some` exactly while this process owns polling. Dropping it releases.
    owned: Option<ScheduleLease>,
    control: Arc<dyn PollControl>,
    shared: Arc<SupervisorShared>,
}

impl PollSupervisorState {
    fn new(
        home: PathBuf,
        holder: String,
        pid: u32,
        ttl: Duration,
        owned: Option<ScheduleLease>,
        control: Arc<dyn PollControl>,
        shared: Arc<SupervisorShared>,
    ) -> Self {
        let dir = lease_dir(&home);
        let rank = rank_of(&holder);
        shared.is_owner.store(owned.is_some(), Ordering::SeqCst);
        if owned.is_some() {
            shared.owner_pid.store(pid, Ordering::SeqCst);
            // An owner must carry no claim: a claim is what a NON-owner
            // publishes. Leaving one behind would make an owner look like a
            // pending claimant to everybody else.
            remove_claim(&dir, pid);
        } else {
            publish_claim(&dir, pid, rank, &holder);
        }
        Self {
            home,
            dir,
            holder,
            rank,
            pid,
            ttl,
            owned,
            control,
            shared,
        }
    }

    fn is_owner(&self) -> bool {
        self.owned.is_some()
    }

    async fn tick(&mut self) -> TickOutcome {
        if self.is_owner() {
            self.tick_as_owner().await
        } else {
            self.tick_as_observer().await
        }
    }

    async fn tick_as_owner(&mut self) -> TickOutcome {
        let Some(claim) = best_live_claim(&self.dir, self.ttl, self.pid) else {
            return TickOutcome::HeldOwner;
        };
        if claim.rank <= self.rank {
            // Strictly-greater only. Equal ranks never preempt, so two
            // sessions — or two daemons — cannot chase each other.
            return TickOutcome::HeldOwner;
        }

        // ORDER MATTERS. Stop polling BEFORE releasing the lock: releasing
        // first would open a window in which the successor acquires and arms
        // its pollers while this process's are still running, which is the
        // two-poller defect this whole module exists to prevent.
        self.control.stop_polling().await;
        self.owned = None;
        self.shared.is_owner.store(false, Ordering::SeqCst);
        self.shared.owner_pid.store(claim.pid, Ordering::SeqCst);
        self.shared.yields.fetch_add(1, Ordering::SeqCst);
        // Publish this process's own claim on the way out, so that when the
        // successor eventually leaves, this process is a declared candidate
        // rather than a silent one.
        publish_claim(&self.dir, self.pid, self.rank, &self.holder);

        tracing::warn!(
            target: "wcore_agent::channel_lease",
            holder = self.holder.as_str(),
            to_pid = claim.pid,
            to_holder = claim.holder.as_str(),
            "{CHANNEL_LEASE_TOKEN}=yielded — a higher-precedence process claimed inbound \
             polling; this process has stopped polling"
        );
        eprintln!(
            "{CHANNEL_LEASE_TOKEN}=yielded holder={} to_pid={} to_holder={}: \
             a higher-precedence wayland-core process ({}) is taking over inbound message \
             polling for this home; this one has stopped polling. Sending still works.",
            self.holder, claim.pid, claim.holder, claim.holder
        );

        TickOutcome::Yielded {
            to_pid: claim.pid,
            to_rank: claim.rank,
        }
    }

    async fn tick_as_observer(&mut self) -> TickOutcome {
        // Claim before contending, and refresh every tick — the refresh is what
        // keeps this claim suppressive, and stopping it is what un-wedges the
        // system if this process dies.
        publish_claim(&self.dir, self.pid, self.rank, &self.holder);

        if let Some(claim) = best_live_claim(&self.dir, self.ttl, self.pid)
            && claim.rank > self.rank
        {
            // Do not race a better candidate for a lock it is about to take.
            // Winning that race and yielding again on the next tick is the
            // oscillation the panel identified.
            self.shared.owner_pid.store(claim.pid, Ordering::SeqCst);
            return TickOutcome::Deferred { to_pid: claim.pid };
        }

        let lease = attempt_quiet(&self.home, &self.holder);
        let owner_pid = lease.owner_pid();
        let Some(inner) = into_inner(lease) else {
            if let Some(p) = owner_pid {
                self.shared.owner_pid.store(p, Ordering::SeqCst);
            }
            return TickOutcome::StillObserver;
        };

        self.owned = Some(inner);
        remove_claim(&self.dir, self.pid);
        self.shared.is_owner.store(true, Ordering::SeqCst);
        self.shared.owner_pid.store(self.pid, Ordering::SeqCst);
        self.shared.acquisitions.fetch_add(1, Ordering::SeqCst);
        self.control.start_polling().await;

        tracing::info!(
            target: "wcore_agent::channel_lease",
            holder = self.holder.as_str(),
            pid = self.pid,
            "{CHANNEL_LEASE_TOKEN}=acquired — this process has taken over inbound polling"
        );
        eprintln!(
            "{CHANNEL_LEASE_TOKEN}=acquired holder={} pid={}: this process has taken over \
             inbound message polling for this home.",
            self.holder, self.pid
        );

        TickOutcome::Acquired
    }
}

impl Drop for PollSupervisorState {
    fn drop(&mut self) {
        remove_claim(&self.dir, self.pid);
    }
}

/// Take the `ScheduleLease` out of a [`ChannelPollLease`], if it owns one.
fn into_inner(mut lease: ChannelPollLease) -> Option<ScheduleLease> {
    lease._lease.take()
}

/// Keeps one process's polling role correct for as long as the process lives.
///
/// **Hold this for the whole process lifetime.** Dropping it aborts the tick
/// loop and releases the lease, which is the same contract the bare
/// [`ChannelPollLease`] carried.
#[derive(Debug)]
pub struct ChannelPollSupervisor {
    shared: Arc<SupervisorShared>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl ChannelPollSupervisor {
    /// Adopt the boot lease attempt and start supervising.
    ///
    /// Must be called from inside a tokio runtime.
    pub fn spawn(
        home: &Path,
        holder: &str,
        boot: ChannelPollLease,
        control: Arc<dyn PollControl>,
    ) -> Self {
        let tick = tick_interval();
        let shared = Arc::new(SupervisorShared::default());
        let mut state = PollSupervisorState::new(
            home.to_path_buf(),
            holder.to_string(),
            std::process::id(),
            tick * CLAIM_TTL_TICKS,
            into_inner(boot),
            control,
            Arc::clone(&shared),
        );

        let task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(tick);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // `interval` fires immediately; the boot decision has already been
            // made and announced, so drop that one.
            ticker.tick().await;
            loop {
                ticker.tick().await;
                let outcome = state.tick().await;
                tracing::trace!(
                    target: "wcore_agent::channel_lease",
                    ?outcome,
                    "channel poll supervisor tick"
                );
            }
        });

        Self {
            shared,
            task: Some(task),
        }
    }

    /// Whether this process is polling RIGHT NOW. Unlike the boot-time answer,
    /// this changes over the process's life.
    pub fn is_owner(&self) -> bool {
        self.shared.is_owner.load(Ordering::SeqCst)
    }

    pub fn role(&self) -> ChannelPollRole {
        if self.is_owner() {
            ChannelPollRole::Owner
        } else {
            ChannelPollRole::Observer
        }
    }

    /// Pid believed to own polling, or `None` when nothing is known.
    pub fn owner_pid(&self) -> Option<u32> {
        match self.shared.owner_pid.load(Ordering::SeqCst) {
            0 => None,
            p => Some(p),
        }
    }

    /// How many times this process has stood down for a better candidate.
    pub fn yields(&self) -> u32 {
        self.shared.yields.load(Ordering::SeqCst)
    }

    /// How many times this process has taken over polling after boot.
    pub fn acquisitions(&self) -> u32 {
        self.shared.acquisitions.load(Ordering::SeqCst)
    }
}

impl Drop for ChannelPollSupervisor {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
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

    // ── F24-CS: role precedence ────────────────────────────────────────────
    //
    // These drive the state machine DIRECTLY rather than through the spawned
    // tick loop. Two simulated processes live in one test process with
    // injected pids, so every ordering below is exact rather than raced
    // against a timer — which is the only way the wedge and oscillation cases
    // are testable at all.

    #[derive(Debug, Default)]
    struct FakeControl {
        polling: AtomicBool,
        starts: AtomicU32,
        stops: AtomicU32,
    }

    impl FakeControl {
        fn polling(&self) -> bool {
            self.polling.load(Ordering::SeqCst)
        }
        fn starts(&self) -> u32 {
            self.starts.load(Ordering::SeqCst)
        }
        fn stops(&self) -> u32 {
            self.stops.load(Ordering::SeqCst)
        }
    }

    impl PollControl for FakeControl {
        fn start_polling(&self) -> PollFuture {
            self.polling.store(true, Ordering::SeqCst);
            self.starts.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }
        fn stop_polling(&self) -> PollFuture {
            self.polling.store(false, Ordering::SeqCst);
            self.stops.fetch_add(1, Ordering::SeqCst);
            Box::pin(async {})
        }
    }

    const LONG_TTL: Duration = Duration::from_secs(3600);

    /// A process that boots holding the lease.
    fn owner_state(
        home: &Path,
        holder: &str,
        pid: u32,
        ttl: Duration,
        ctl: &Arc<FakeControl>,
    ) -> PollSupervisorState {
        let inner = into_inner(attempt_quiet(home, holder));
        assert!(
            inner.is_some(),
            "{holder} was supposed to win the boot lease"
        );
        ctl.start_polling();
        PollSupervisorState::new(
            home.to_path_buf(),
            holder.to_string(),
            pid,
            ttl,
            inner,
            Arc::clone(ctl) as Arc<dyn PollControl>,
            Arc::new(SupervisorShared::default()),
        )
    }

    /// A process that boots without the lease.
    fn observer_state(
        home: &Path,
        holder: &str,
        pid: u32,
        ttl: Duration,
        ctl: &Arc<FakeControl>,
    ) -> PollSupervisorState {
        PollSupervisorState::new(
            home.to_path_buf(),
            holder.to_string(),
            pid,
            ttl,
            None,
            Arc::clone(ctl) as Arc<dyn PollControl>,
            Arc::new(SupervisorShared::default()),
        )
    }

    #[test]
    fn the_rank_order_is_total_and_floors_unknown_holders() {
        assert!(rank_of("gateway") > rank_of("cron-daemon"));
        assert!(rank_of("cron-daemon") > rank_of("session"));
        // A holder string is self-reported. If an unrecognised one ranked
        // ABOVE the floor it could preempt the installed service by typo.
        assert_eq!(rank_of("something-else"), RANK_SESSION);
        assert_eq!(rank_of(""), RANK_SESSION);
    }

    #[tokio::test]
    async fn the_service_takes_polling_from_a_session_that_got_there_first() {
        // THE defect this lane owns. Session first, gateway second: the
        // gateway must end up polling and the session must stop.
        let home = tempfile::tempdir().unwrap();
        let sctl = Arc::new(FakeControl::default());
        let gctl = Arc::new(FakeControl::default());

        let mut session = owner_state(home.path(), "session", 1001, LONG_TTL, &sctl);
        let mut gateway = observer_state(home.path(), "gateway", 2002, LONG_TTL, &gctl);
        assert!(sctl.polling(), "the session owns polling at boot");
        assert!(!gctl.polling(), "the gateway lost the boot race");

        // The gateway does not get the lock while the session holds it — the
        // exclusion is unchanged.
        assert_eq!(gateway.tick().await, TickOutcome::StillObserver);
        assert!(!gctl.polling());

        // The session sees the gateway's claim and stands down.
        assert_eq!(
            session.tick().await,
            TickOutcome::Yielded {
                to_pid: 2002,
                to_rank: RANK_GATEWAY
            }
        );
        assert!(!sctl.polling(), "a yielding owner must STOP polling");
        assert_eq!(sctl.stops(), 1);
        assert!(!session.is_owner());

        // And the gateway takes over.
        assert_eq!(gateway.tick().await, TickOutcome::Acquired);
        assert!(gctl.polling(), "the service must end up polling");
        assert!(gateway.is_owner());

        // At no point did both poll: the session stopped before it released.
        assert!(!sctl.polling() && gctl.polling());

        // The session now sits as a declared observer and does NOT take the
        // lock back.
        assert_eq!(session.tick().await, TickOutcome::StillObserver);
        assert!(!sctl.polling());
        assert_eq!(sctl.starts(), 1, "the session must not have restarted");
    }

    #[tokio::test]
    async fn the_service_keeps_polling_when_it_started_first() {
        // The other direction, and the one a naive precedence rule breaks: a
        // session arriving second must NOT disturb the service.
        let home = tempfile::tempdir().unwrap();
        let gctl = Arc::new(FakeControl::default());
        let sctl = Arc::new(FakeControl::default());

        let mut gateway = owner_state(home.path(), "gateway", 2002, LONG_TTL, &gctl);
        let mut session = observer_state(home.path(), "session", 1001, LONG_TTL, &sctl);

        for _ in 0..5 {
            assert_eq!(session.tick().await, TickOutcome::StillObserver);
            assert_eq!(gateway.tick().await, TickOutcome::HeldOwner);
        }
        assert!(gctl.polling());
        assert!(!sctl.polling());
        assert_eq!(
            gctl.stops(),
            0,
            "the service must never have been asked to stop"
        );
    }

    #[tokio::test]
    async fn equal_ranks_never_preempt_each_other() {
        // Two sessions. If preemption used `>=`, these would hand the lease
        // back and forth forever and every tick would stop and restart real
        // pollers.
        let home = tempfile::tempdir().unwrap();
        let actl = Arc::new(FakeControl::default());
        let bctl = Arc::new(FakeControl::default());

        let mut a = owner_state(home.path(), "session", 1001, LONG_TTL, &actl);
        let mut b = observer_state(home.path(), "session", 1002, LONG_TTL, &bctl);

        for _ in 0..5 {
            assert_eq!(a.tick().await, TickOutcome::HeldOwner);
            assert_eq!(b.tick().await, TickOutcome::StillObserver);
        }
        assert_eq!(actl.stops(), 0);
        assert_eq!(bctl.starts(), 0);
    }

    #[tokio::test]
    async fn an_observer_defers_to_a_fresh_better_claim_rather_than_racing_it() {
        // The oscillation guard, isolated. The lock is FREE and the session
        // would win it — but a fresh gateway claim is outstanding, so taking
        // it would only mean yielding again next tick.
        let home = tempfile::tempdir().unwrap();
        let sctl = Arc::new(FakeControl::default());
        let mut session = observer_state(home.path(), "session", 1001, LONG_TTL, &sctl);

        publish_claim(&lease_dir(home.path()), 2002, RANK_GATEWAY, "gateway");

        assert_eq!(session.tick().await, TickOutcome::Deferred { to_pid: 2002 });
        assert!(!sctl.polling());

        // Proof the deferral is what stopped it, not an absent lock: with the
        // claim gone, the very same state acquires immediately.
        remove_claim(&lease_dir(home.path()), 2002);
        assert_eq!(session.tick().await, TickOutcome::Acquired);
        assert!(sctl.polling());
    }

    #[tokio::test]
    async fn a_dead_claimants_stale_claim_cannot_wedge_polling() {
        // The failure mode that would be WORSE than the starvation being
        // fixed: an owner yields to a claimant that then dies, and nobody
        // polls, forever. Staleness is the only thing that bounds it.
        let home = tempfile::tempdir().unwrap();
        let sctl = Arc::new(FakeControl::default());
        let mut session = owner_state(home.path(), "session", 1001, LONG_TTL, &sctl);

        // A gateway claims, so the session stands down...
        publish_claim(&lease_dir(home.path()), 2002, RANK_GATEWAY, "gateway");
        assert!(matches!(session.tick().await, TickOutcome::Yielded { .. }));
        assert!(!sctl.polling(), "nobody is polling at this instant");

        // ...and then dies without ever taking the lock. Its claim file is
        // still on disk. While that claim is FRESH the session correctly waits.
        assert_eq!(session.tick().await, TickOutcome::Deferred { to_pid: 2002 });
        assert!(!sctl.polling());

        // Once it ages past the TTL — which is what a dead claimant's claim
        // does, because nothing refreshes it — the session resumes.
        session.ttl = Duration::ZERO;
        assert_eq!(session.tick().await, TickOutcome::Acquired);
        assert!(
            sctl.polling(),
            "a stale claim must not leave the home with NO poller"
        );
    }

    #[tokio::test]
    async fn an_already_running_observer_takes_over_when_the_owner_dies() {
        // Residual (2) of the landing lane: previously only a NEWLY STARTED
        // process could take over, so an operator had to restart something.
        let home = tempfile::tempdir().unwrap();
        let gctl = Arc::new(FakeControl::default());
        let sctl = Arc::new(FakeControl::default());

        let gateway = owner_state(home.path(), "gateway", 2002, LONG_TTL, &gctl);
        let mut session = observer_state(home.path(), "session", 1001, LONG_TTL, &sctl);
        assert_eq!(session.tick().await, TickOutcome::StillObserver);

        // Drop stands in for process death: the OS releases the lock when the
        // descriptor closes either way.
        drop(gateway);

        assert_eq!(session.tick().await, TickOutcome::Acquired);
        assert!(
            sctl.polling(),
            "an already-running observer must take over with no operator action"
        );
    }

    #[test]
    fn a_claim_file_cannot_be_mistaken_for_a_channel_config() {
        // Same hazard as the lock and record sentinels: `auto_register_from_dir`
        // globs `*.toml` in this very directory.
        let home = tempfile::tempdir().unwrap();
        let dir = lease_dir(home.path());
        publish_claim(&dir, 4242, RANK_SESSION, "session");
        let tomls: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
            .collect();
        assert!(
            tomls.is_empty(),
            "a claim must not look like a channel config"
        );
        assert!(claim_path(&dir, 4242).exists());
    }

    #[test]
    fn an_unparseable_or_future_dated_claim_is_treated_as_absent() {
        // Both directions of "I cannot read this claim" must resolve to
        // "there is no claim", because the alternative — honouring it — stops
        // the incumbent polling on the strength of a file nobody can read.
        let home = tempfile::tempdir().unwrap();
        let dir = lease_dir(home.path());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(format!("{CLAIM_PREFIX}7777")), b"{not json").unwrap();
        assert_eq!(best_live_claim(&dir, LONG_TTL, 1), None);

        // A readable claim in the same directory is still found, so the empty
        // answer above is a rejection and not a broken reader.
        publish_claim(&dir, 8888, RANK_GATEWAY, "gateway");
        assert_eq!(
            best_live_claim(&dir, LONG_TTL, 1).map(|c| c.pid),
            Some(8888)
        );
        // ...and excluding one's own pid works, or every process would defer
        // to itself and nothing would ever poll.
        assert_eq!(best_live_claim(&dir, LONG_TTL, 8888), None);
    }

    #[test]
    fn the_tick_interval_is_clamped() {
        // A zero or absurd override must not spin the loop or park it past any
        // plausible handover.
        let lo = Duration::from_millis(50);
        let hi = Duration::from_millis(60_000);
        assert!(tick_interval() >= lo && tick_interval() <= hi);
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
