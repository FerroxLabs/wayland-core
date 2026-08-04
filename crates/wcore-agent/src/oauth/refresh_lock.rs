//! Cross-process single-flight for the OAuth refresh POST (#172).
//!
//! ## The defect
//!
//! [`SingleFlightRefresh`](super::SingleFlightRefresh) coalesces refreshes
//! **within one process**. ChatGPT and xAI refresh tokens ROTATE and are
//! single-use, so two Wayland processes sharing one profile near expiry both
//! POST the same refresh token. Under RFC 6819 §5.2.2.3 and the OAuth 2.1
//! security BCP, replaying a single-use refresh token is treated as theft and
//! the sanctioned server response is to revoke the **entire authorization
//! grant** — including the tokens the winner just received. The cost of the
//! race is both processes logged out, not one turn failed.
//!
//! Every policy in this module follows from that asymmetry: an unlocked POST
//! risks an unrecoverable logout, a refusal costs a retry, and those are not
//! comparable quantities.
//!
//! ## The protocol
//!
//! ```text
//! in-process single-flight  →  refresh file lock  →  credential store write
//! ```
//!
//! That order is a total order and it is stated at every acquisition site. The
//! reverse (store → refresh) is structurally impossible: every
//! [`ExclusiveFileLock`] acquisition for a credential write lives in
//! `wcore-config`, and `wcore-config` does not depend on `wcore-agent`, so no
//! code in the store layer can reach the refresh lock. `AGENTS.md`'s
//! "dependencies flow downward" rule is what keeps that true — a future upward
//! dependency from `wcore-config` into `wcore-agent` would break it.
//!
//! Nesting is certain, not forbidden: the critical section spans
//! load → decide → POST → store, and the store write takes its own lock inside.
//! The requirement is a consistent order, not the absence of nesting.
//!
//! ## Timing (derived, not inherited)
//!
//! The migration lock this primitive was generalized from waits 10 s and
//! declares a holder dead after 60 s. Both numbers were sized for a sub-second
//! migration and neither survives contact with a refresh: the hold here is a
//! network round-trip capped at [`PER_CALL_TIMEOUT`] plus a store write, so a
//! 10 s wait would expire on the HAPPY path over a slow network. The constants
//! below are derived from the hold instead, and the derivation is checked at
//! compile time.

use std::path::{Path, PathBuf};
use std::time::Duration;

use wcore_config::credentials::{ExclusiveFileLock, LockPolicy, oauth_tokens_key};

/// Outer wall-clock cap on one refresh round-trip. The providers' own
/// `PER_CALL_TIMEOUT` values are this value; the lock timing is derived from
/// it, so the two cannot drift apart silently.
pub(super) const PER_CALL_TIMEOUT: Duration = Duration::from_secs(POST_TIMEOUT_SECS);

const POST_TIMEOUT_SECS: u64 = 20;

/// Budget for everything the critical section does around the POST: reloading
/// the pair, the credential-store write, and scheduling slop.
///
/// **This is a real bound only because the store write is bounded.** The write
/// goes `OAuthStorage::store` -> `CredentialsStore::put` -> `chunked_put`,
/// which takes the store's OWN lock under [`LockPolicy::CREDENTIAL_WRITE`] —
/// a 65 s wait ceiling, six times this budget. Nested inside the refresh
/// critical section that made `MAX_HOLD_SECS` aspirational rather than true:
/// the real hold could reach `POST + PERSIST_ATTEMPTS * 65 s`, and every
/// number derived from `MAX_HOLD_SECS` (the wait ceiling, and `flow.rs`'s
/// `SUBSCRIBER_CEILING`) was sized against a hold that could be exceeded
/// several times over. Cross-audit finding, Kimi K3.
///
/// So the persist loop is bounded explicitly by [`PERSIST_TOTAL_BUDGET`], and
/// the arithmetic below is checked at compile time. A store write that cannot
/// land inside the budget fails the refresh RETRYABLY — it never leaves the
/// lock held, and it never POSTs again.
const STORE_BUDGET_SECS: u64 = 10;

/// Attempts for the post-POST store write, and the pause between them.
///
/// Small and bounded on purpose: the rotated token is already spent by the
/// time this loop runs, so the useful failure mode is "fail fast and let the
/// next process reload-and-recheck", not "hold the refresh lock retrying".
pub(crate) const PERSIST_ATTEMPTS: u32 = 3;
pub(crate) const PERSIST_RETRY_DELAY: Duration = Duration::from_millis(200);

/// Hard cap on the whole persist loop, retries and pauses included. Sits
/// inside [`STORE_BUDGET_SECS`] so the store write cannot silently blow the
/// hold budget by waiting on the credential store's own 65 s lock.
pub(crate) const PERSIST_TOTAL_BUDGET: Duration = Duration::from_secs(8);

const _: () = assert!(
    PERSIST_TOTAL_BUDGET.as_secs() < STORE_BUDGET_SECS,
    "the persist loop must fit inside the store budget, or MAX_HOLD_SECS is a \
     claim rather than a bound and every ceiling derived from it is undersized"
);

/// The most time a healthy holder can occupy the lock.
const MAX_HOLD_SECS: u64 = POST_TIMEOUT_SECS + STORE_BUDGET_SECS;

/// The primary's worst case as seen by an in-process SUBSCRIBER: it waits for
/// this process to win the cross-process lock AND then to finish the hold.
/// `flow.rs`'s `SUBSCRIBER_CEILING` must exceed this or a healthy-but-slow
/// primary strands its subscribers with a spurious timeout.
pub(crate) const PRIMARY_WORST_CASE_SECS: u64 = WAIT_CEILING_SECS + MAX_HOLD_SECS;

/// How often a holder re-stamps the lockfile.
const HEARTBEAT_SECS: u64 = 2;

/// How long an un-refreshed lockfile may sit before a waiter treats its holder
/// as crashed.
///
/// **Heartbeat, not a larger staleness — and here is why.** Without a
/// heartbeat this number would have to exceed [`MAX_HOLD_SECS`] with margin
/// (90 s or so), because a healthy holder mid-POST that got stolen from would
/// produce exactly the double-POST this lock exists to prevent. But then a
/// holder that CRASHED would wedge every other process for those 90 s, and the
/// proof for that case becomes a 90-second test — the kind that gets disabled.
/// With the holder re-stamping every [`HEARTBEAT_SECS`], the lockfile's mtime
/// tracks liveness rather than acquisition, so staleness is sized against the
/// heartbeat (5 intervals of slack) and is independent of how long the work
/// takes. A crash is detected in 10 s and a live holder is never stolen from.
const STALE_AFTER_SECS: u64 = HEARTBEAT_SECS * 5;

/// How long a waiter keeps trying before giving up.
///
/// Above [`MAX_HOLD_SECS`] so healthy contention never times out, and above
/// [`STALE_AFTER_SECS`] so a crashed holder is always reached by the steal
/// within the wait — a ceiling below staleness would turn a crash into a hard
/// refusal for every waiter until the crash aged out.
const WAIT_CEILING_SECS: u64 = MAX_HOLD_SECS + STALE_AFTER_SECS + 5;

const _: () = assert!(
    WAIT_CEILING_SECS > MAX_HOLD_SECS,
    "a waiter that gives up before a healthy holder can finish fires the \
     contention path on the happy path"
);
const _: () = assert!(
    WAIT_CEILING_SECS > STALE_AFTER_SECS,
    "a ceiling below staleness means a crashed holder is never stolen from"
);
const _: () = assert!(
    STALE_AFTER_SECS >= HEARTBEAT_SECS * 3,
    "staleness must leave a live holder several missed heartbeats of slack"
);

/// The timing contract for the refresh lock.
pub(crate) fn policy() -> LockPolicy {
    LockPolicy::new(
        Duration::from_secs(STALE_AFTER_SECS),
        Duration::from_secs(WAIT_CEILING_SECS),
    )
    .with_heartbeat(Duration::from_secs(HEARTBEAT_SECS))
}

/// Path of the refresh lock for `provider` inside `dir`.
///
/// Named from `SHA-256` of the credential key the pair is stored under, so the
/// lock is scoped to exactly the pair it guards and cannot be confused with the
/// credential store's own write lock (a different name in a different
/// directory).
pub fn lock_path(dir: &Path, provider: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(oauth_tokens_key(provider).as_bytes());
    let mut name = String::from(".oauth-refresh-");
    for byte in &digest[..16] {
        use std::fmt::Write;
        let _ = write!(name, "{byte:02x}");
    }
    name.push_str(".lock");
    dir.join(name)
}

/// What an acquisition attempt produced.
pub(crate) enum Acquisition {
    /// We own the refresh for this pair. POSTing is permitted.
    Held(ExclusiveFileLock),
    /// Another process owns it and did not finish within the ceiling. POSTing
    /// is NOT permitted — see the module docs.
    Busy(String),
}

/// Acquire the refresh lock without blocking a task-executor worker.
///
/// The primitive spins with `std::thread::sleep`, and the wait is now sized
/// above the POST timeout. Awaiting that inline would park a worker for tens of
/// seconds; on a small runtime the parked worker can be the very one that would
/// have driven the POST that releases the lock, which is a starvation deadlock
/// rather than a stall. `spawn_blocking` moves the spin off the worker pool.
pub(crate) async fn acquire(path: PathBuf, label: &'static str) -> Acquisition {
    let policy = policy();
    match tokio::task::spawn_blocking(move || ExclusiveFileLock::acquire(path, policy, label)).await
    {
        Ok(Ok(lock)) => Acquisition::Held(lock),
        Ok(Err(error)) => Acquisition::Busy(error.to_string()),
        // A panicked blocking task tells us nothing about the lock, so it is
        // handled exactly like contention: no POST.
        Err(error) => Acquisition::Busy(format!("refresh lock task failed: {error}")),
    }
}

/// Hold the refresh lock around a token WRITE that is not itself a refresh.
///
/// **Why a writer that never POSTs still needs this (§4.9).** Serializing the
/// two writes is not enough, because the hazard is a stale read-modify-write
/// spanning two DIFFERENT operations: a refresh reads the pair, spends up to
/// [`PER_CALL_TIMEOUT`] on the network, and stores. A `logout` that lands in
/// that window is overwritten by the refresh's store — the credential the user
/// just removed is resurrected and they are still signed in believing they are
/// not. The same window turns a fresh `login` into an overwrite by a refresh of
/// the pair it replaced.
///
/// Writers of the ChatGPT/xAI pair, and what each does:
///
/// | writer | disposition |
/// |---|---|
/// | `ChatGptTokenManager::refresh` / `XaiTokenManager::refresh` | holds the lock across load → POST → store |
/// | `auth login <provider>` (loopback PKCE) | holds it around the store |
/// | `auth login chatgpt --device` | holds it around the store |
/// | `auth login chatgpt --import-codex` | holds it around the store |
/// | `auth status` (auto-import of a Codex login) | holds it around the store |
/// | `auth logout <provider>` | holds it around the delete |
/// | `OAuthStorage::load`'s legacy cleartext promotion | cannot race: it only runs inside a caller that already holds the lock, and re-acquiring is not possible — the lockfile is not reentrant |
/// | Google Meet's refresh (`tool_backends::google_meet`) | out of scope: a different provider key, and Google's installed-app refresh token does not rotate |
pub async fn hold_for_writer(path: PathBuf) -> Result<ExclusiveFileLock, String> {
    match acquire(path, "oauth token write").await {
        Acquisition::Held(lock) => Ok(lock),
        Acquisition::Busy(why) => Err(format!(
            "another Wayland process is refreshing this provider's OAuth token and did not \
             finish in time, so changing the stored token now could undo or be undone by it. \
             Nothing was changed — run the command again in a moment ({why})"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_name_is_pair_scoped_and_not_the_provider_name() {
        let dir = Path::new("/profile/oauth");
        let chatgpt = lock_path(dir, "chatgpt");
        let xai = lock_path(dir, "xai");
        assert_ne!(
            chatgpt, xai,
            "two providers must not serialize against each other"
        );
        let name = chatgpt.file_name().unwrap().to_string_lossy().to_string();
        assert!(
            !name.contains("chatgpt"),
            "the lock name must be the digest, not the cleartext key: {name}"
        );
        assert_eq!(
            name.len(),
            ".oauth-refresh-".len() + 32 + ".lock".len(),
            "digest must be truncated to a fixed 32 hex chars: {name}"
        );
        assert_eq!(chatgpt, lock_path(dir, "chatgpt"), "must be deterministic");
    }

    /// The regression this guards: reusing the migration lock's 10 s wait
    /// against a 20 s POST timeout, which fires the contention path on a merely
    /// slow network.
    ///
    /// The ORDERING of the constants is asserted at compile time above
    /// (`const _: () = assert!(...)`), which is strictly stronger than a
    /// runtime check and is what clippy's `assertions_on_constants` was
    /// pointing at. What a runtime test can add, and what this now does, is
    /// prove the constants actually reach the `LockPolicy` that gets used —
    /// the derivation being right is worth nothing if `policy()` hands the
    /// lock something else.
    #[test]
    fn policy_carries_the_derived_timings_and_a_heartbeat() {
        let policy = policy();
        let expected = LockPolicy::new(
            Duration::from_secs(STALE_AFTER_SECS),
            Duration::from_secs(WAIT_CEILING_SECS),
        )
        .with_heartbeat(Duration::from_secs(HEARTBEAT_SECS));
        assert_eq!(
            format!("{policy:?}"),
            format!("{expected:?}"),
            "policy() must hand the lock the DERIVED timings; a hard-coded or \
             stale set here silently undoes the derivation the constants above \
             are checked for"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn contended_acquire_does_not_starve_the_only_worker() {
        // Proof #7. A single-worker runtime is the harshest case: if the spin
        // ran on the worker, nothing else could make progress while it waited.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("busy.lock");
        std::fs::write(&path, "someone-else").unwrap();

        let ticks = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter = ticks.clone();
        let unrelated = tokio::spawn(async move {
            for _ in 0..20 {
                counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });

        // A short ceiling keeps the test fast; the mechanism under test is
        // where the spin RUNS, not how long it runs.
        let blocking_path = path.clone();
        let waiter = tokio::task::spawn_blocking(move || {
            ExclusiveFileLock::acquire(
                blocking_path,
                LockPolicy::new(Duration::from_secs(600), Duration::from_millis(300)),
                "refresh",
            )
        });
        assert!(waiter.await.unwrap().is_err(), "the lock was held");
        unrelated.await.unwrap();
        assert_eq!(
            ticks.load(std::sync::atomic::Ordering::Relaxed),
            20,
            "the unrelated task must have run to completion while the acquire waited"
        );
    }
}
