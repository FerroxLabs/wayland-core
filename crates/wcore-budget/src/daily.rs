//! Cross-session daily spend ceiling — the durable, atomic ledger behind
//! [`crate::BudgetCap::per_user_daily_usd`].
//!
//! ## Why a dedicated store
//!
//! Every other budget surface in this crate is *per session*. A caller that
//! starts a fresh session per process — a crash-looping daemon, a cron job, a
//! channel gateway answering inbound messages — therefore has NO bound at all:
//! each run legitimately believes it is within budget because each run has its
//! own budget.
//!
//! The diagnostics cost ledger is deliberately NOT the enforcement surface for
//! this: it is prunable, so enforcing on it either fails open (restoring the
//! hole the moment a prune lands) or fails closed (bricking every launch after
//! a prune). This store is the opposite by construction — it holds exactly the
//! spend authority for the current UTC day and nothing else, so it is small,
//! bounded, and never a pruning target.
//!
//! ## Concurrency and crash safety
//!
//! - **Concurrent processes.** Every mutation is a read-modify-write performed
//!   under an exclusive advisory lock (`flock` / `LockFileEx` via `fd-lock`) on
//!   a sibling `.lock` file, then published with a temp-file + `rename`. Two
//!   processes racing the same reservation serialize; neither can observe a
//!   torn file.
//! - **Crash mid-reserve.** A reservation is durably recorded BEFORE the paid
//!   call, so a process that dies between reserve and settle leaves the
//!   reservation counted. It carries a lease deadline and is reclaimed once the
//!   lease expires — the failure mode is *conservative* (spend is
//!   over-counted for at most one lease), never permissive.
//! - **Store that does not exist yet.** A missing file is an empty ledger and
//!   the parent directory is created on first write. A file that exists but
//!   cannot be parsed is a hard refusal, not an empty ledger: a spend ceiling
//!   that fails open on a corrupt file can be defeated by writing garbage into
//!   it. The error names the path so an operator can inspect or remove it.
//! - **Day rollover.** Buckets are keyed by UTC calendar day. A stale day is
//!   reset on the first touch after midnight; entries for older days are
//!   dropped rather than accumulated.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Schema version of the on-disk ledger. A file written by a newer version is
/// refused rather than silently reinterpreted.
pub const DAILY_LEDGER_SCHEMA_VERSION: u32 = 1;

/// Default lifetime of one durable reservation. A provider call that has not
/// settled within this window is assumed to belong to a process that died, and
/// its reservation is reclaimed on the next touch.
pub const DEFAULT_RESERVATION_LEASE_SECS: i64 = 30 * 60;

/// Tolerance for the float comparison that admits a charge. Without it a cap of
/// `1.0` would reject a total of `1.0000000000000002` produced by summing
/// exactly-representable partial charges.
const USD_EPSILON: f64 = 1e-9;

/// Failures raised while consulting or mutating the durable daily ledger.
#[derive(Debug, Clone, PartialEq, Error)]
pub enum DailySpendError {
    /// The charge under attempt would cross the configured daily ceiling.
    #[error("daily spend cap exceeded: limit=${limit:.4}, observed=${observed:.4}")]
    Exceeded { limit: f64, observed: f64 },
    /// The ledger could not be read, parsed, locked, or written. Enforcement
    /// fails CLOSED on this: an unreadable ceiling is not an absent ceiling.
    #[error("daily spend ledger at {path} is unusable: {reason}")]
    Unusable { path: String, reason: String },
    /// A non-finite or negative amount reached the ledger.
    #[error("daily spend amount must be finite and non-negative, got {0}")]
    InvalidAmount(f64),
}

impl DailySpendError {
    fn unusable(path: &Path, reason: impl std::fmt::Display) -> Self {
        Self::Unusable {
            path: path.display().to_string(),
            reason: reason.to_string(),
        }
    }
}

/// A durable claim on daily spend authority, held for the lifetime of one paid
/// provider call. Settle it with the authoritative cost, or release it if the
/// call never happened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DailyGrant {
    id: String,
}

impl DailyGrant {
    /// Opaque identifier of this claim inside the durable ledger.
    pub fn id(&self) -> &str {
        &self.id
    }
}

/// A `subject` bucket's live position for the current UTC day.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DailyPosition {
    /// Spend already settled today.
    pub committed_usd: f64,
    /// Spend claimed by in-flight, unexpired reservations.
    pub reserved_usd: f64,
}

impl DailyPosition {
    /// Total authority consumed today: settled plus in-flight.
    pub fn total_usd(&self) -> f64 {
        self.committed_usd + self.reserved_usd
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct ReservationRecord {
    id: String,
    usd: f64,
    expires_at_unix_ms: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct SubjectLedger {
    /// UTC calendar day this bucket accounts for, `YYYY-MM-DD`.
    day: String,
    committed_usd: f64,
    #[serde(default)]
    reservations: Vec<ReservationRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct DailyLedgerFile {
    schema_version: u32,
    #[serde(default)]
    subjects: BTreeMap<String, SubjectLedger>,
}

impl Default for DailyLedgerFile {
    fn default() -> Self {
        Self {
            schema_version: DAILY_LEDGER_SCHEMA_VERSION,
            subjects: BTreeMap::new(),
        }
    }
}

/// Durable, cross-process ledger of spend for the current UTC day.
#[derive(Debug)]
pub struct DailySpendStore {
    path: PathBuf,
}

impl DailySpendStore {
    /// Bind the store to `path`. Nothing is read or created until the first
    /// operation, so constructing a store never fails.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// The ledger file this store owns.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Claim `usd` of today's authority for `subject` under a ceiling of `cap`.
    ///
    /// The claim is durable before this returns, so a crash before settlement
    /// still counts against the ceiling until its lease expires.
    pub fn reserve(
        &self,
        subject: &str,
        usd: f64,
        cap: f64,
        lease: Duration,
        now: DateTime<Utc>,
    ) -> Result<DailyGrant, DailySpendError> {
        if !usd.is_finite() || usd < 0.0 {
            return Err(DailySpendError::InvalidAmount(usd));
        }
        self.with_locked_ledger(|ledger| {
            let entry = ledger.bucket_for(subject, now);
            let position = entry.position();
            let observed = position.total_usd() + usd;
            if observed > cap + USD_EPSILON {
                return Err(DailySpendError::Exceeded {
                    limit: cap,
                    observed,
                });
            }
            let id = next_grant_id(now);
            entry.reservations.push(ReservationRecord {
                id: id.clone(),
                usd,
                expires_at_unix_ms: (now + lease).timestamp_millis(),
            });
            Ok(DailyGrant { id })
        })
    }

    /// Reconcile a claim with the authoritative cost of the call it admitted.
    ///
    /// `actual_usd` is committed even when the grant has already been reclaimed
    /// by lease expiry — the money was spent either way, and the ledger must
    /// reflect that.
    pub fn settle(
        &self,
        subject: &str,
        grant: &DailyGrant,
        actual_usd: f64,
        now: DateTime<Utc>,
    ) -> Result<(), DailySpendError> {
        if !actual_usd.is_finite() || actual_usd < 0.0 {
            return Err(DailySpendError::InvalidAmount(actual_usd));
        }
        self.with_locked_ledger(|ledger| {
            let entry = ledger.bucket_for(subject, now);
            entry.reservations.retain(|record| record.id != grant.id);
            entry.committed_usd += actual_usd;
            Ok(())
        })
    }

    /// Drop a claim for a call that never reached the provider.
    pub fn release(
        &self,
        subject: &str,
        grant: &DailyGrant,
        now: DateTime<Utc>,
    ) -> Result<(), DailySpendError> {
        self.with_locked_ledger(|ledger| {
            let entry = ledger.bucket_for(subject, now);
            entry.reservations.retain(|record| record.id != grant.id);
            Ok(())
        })
    }

    /// Read `subject`'s position for the UTC day containing `now`, without
    /// mutating durable state.
    pub fn position(
        &self,
        subject: &str,
        now: DateTime<Utc>,
    ) -> Result<DailyPosition, DailySpendError> {
        let ledger = self.load()?;
        let today = day_key(now);
        Ok(ledger
            .subjects
            .get(subject)
            .filter(|entry| entry.day == today)
            .map(|entry| entry.live_position(now))
            .unwrap_or(DailyPosition {
                committed_usd: 0.0,
                reserved_usd: 0.0,
            }))
    }

    /// Run `body` against the ledger under the exclusive cross-process lock,
    /// publishing the result atomically. The ledger is written only when
    /// `body` succeeds, so a refused reservation never mutates durable state.
    fn with_locked_ledger<T>(
        &self,
        body: impl FnOnce(&mut DailyLedgerFile) -> Result<T, DailySpendError>,
    ) -> Result<T, DailySpendError> {
        let mut lock = self.open_lock()?;
        // `fd_lock`'s guard borrows the `RwLock` mutably, so the retry loop
        // cannot return the guard across a function boundary under NLL — the
        // same closure shape `wcore-config`'s credential marker lock uses.
        let _guard = loop {
            match lock.write() {
                Ok(guard) => break guard,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(DailySpendError::unusable(&self.path, error)),
            }
        };
        let mut ledger = self.load()?;
        let outcome = body(&mut ledger)?;
        self.store(&ledger)?;
        Ok(outcome)
    }

    fn open_lock(&self) -> Result<fd_lock::RwLock<std::fs::File>, DailySpendError> {
        if let Some(parent) = self.path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .map_err(|error| DailySpendError::unusable(&self.path, error))?;
        }
        let lock_path = self.path.with_extension("lock");
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| DailySpendError::unusable(&lock_path, error))?;
        Ok(fd_lock::RwLock::new(file))
    }

    fn load(&self) -> Result<DailyLedgerFile, DailySpendError> {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(DailyLedgerFile::default());
            }
            Err(error) => return Err(DailySpendError::unusable(&self.path, error)),
        };
        if bytes.is_empty() {
            // A zero-length file is the observable result of a crash between
            // create and write on a filesystem without atomic publish. It
            // carries no authority, so it is equivalent to "not yet written".
            return Ok(DailyLedgerFile::default());
        }
        let ledger: DailyLedgerFile = serde_json::from_slice(&bytes).map_err(|error| {
            DailySpendError::unusable(
                &self.path,
                format!("ledger is not readable ({error}); refusing to spend without it"),
            )
        })?;
        if ledger.schema_version != DAILY_LEDGER_SCHEMA_VERSION {
            return Err(DailySpendError::unusable(
                &self.path,
                format!(
                    "unsupported ledger schema version {} (expected {DAILY_LEDGER_SCHEMA_VERSION})",
                    ledger.schema_version
                ),
            ));
        }
        for (subject, entry) in &ledger.subjects {
            if !entry.committed_usd.is_finite() || entry.committed_usd < 0.0 {
                return Err(DailySpendError::unusable(
                    &self.path,
                    format!("subject '{subject}' has a non-finite committed total"),
                ));
            }
            if entry
                .reservations
                .iter()
                .any(|record| !record.usd.is_finite() || record.usd < 0.0)
            {
                return Err(DailySpendError::unusable(
                    &self.path,
                    format!("subject '{subject}' has a non-finite reservation"),
                ));
            }
        }
        Ok(ledger)
    }

    /// Publish `ledger` atomically: write a sibling temp file, flush it to
    /// stable storage, then rename over the target. A reader therefore observes
    /// either the previous ledger or this one, never a partial write.
    fn store(&self, ledger: &DailyLedgerFile) -> Result<(), DailySpendError> {
        use std::io::Write;

        let bytes = serde_json::to_vec(ledger)
            .map_err(|error| DailySpendError::unusable(&self.path, error))?;
        let parent = self.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)
            .map_err(|error| DailySpendError::unusable(&self.path, error))?;
        let temp = parent.join(format!(
            ".{}.{}.tmp",
            self.path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "daily-spend".to_string()),
            std::process::id()
        ));
        {
            let mut file = std::fs::File::create(&temp)
                .map_err(|error| DailySpendError::unusable(&temp, error))?;
            file.write_all(&bytes)
                .map_err(|error| DailySpendError::unusable(&temp, error))?;
            file.sync_all()
                .map_err(|error| DailySpendError::unusable(&temp, error))?;
        }
        std::fs::rename(&temp, &self.path).map_err(|error| {
            let _ = std::fs::remove_file(&temp);
            DailySpendError::unusable(&self.path, error)
        })?;
        // Best effort: durably record the rename itself. Not supported on every
        // platform/filesystem, and a failure here does not invalidate the
        // published ledger.
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    }
}

impl DailyLedgerFile {
    /// Fetch `subject`'s bucket for the UTC day containing `now`, resetting a
    /// stale day and reclaiming expired reservations. Buckets for other
    /// subjects whose day has rolled over are dropped so the file stays small.
    fn bucket_for(&mut self, subject: &str, now: DateTime<Utc>) -> &mut SubjectLedger {
        let today = day_key(now);
        self.subjects.retain(|_, entry| entry.day == today);
        let entry = self
            .subjects
            .entry(subject.to_string())
            .or_insert_with(|| SubjectLedger {
                day: today.clone(),
                committed_usd: 0.0,
                reservations: Vec::new(),
            });
        entry.expire(now);
        entry
    }
}

impl SubjectLedger {
    fn expire(&mut self, now: DateTime<Utc>) {
        let now_ms = now.timestamp_millis();
        self.reservations
            .retain(|record| record.expires_at_unix_ms > now_ms);
    }

    fn position(&self) -> DailyPosition {
        DailyPosition {
            committed_usd: self.committed_usd,
            reserved_usd: self.reservations.iter().map(|record| record.usd).sum(),
        }
    }

    fn live_position(&self, now: DateTime<Utc>) -> DailyPosition {
        let now_ms = now.timestamp_millis();
        DailyPosition {
            committed_usd: self.committed_usd,
            reserved_usd: self
                .reservations
                .iter()
                .filter(|record| record.expires_at_unix_ms > now_ms)
                .map(|record| record.usd)
                .sum(),
        }
    }
}

fn day_key(now: DateTime<Utc>) -> String {
    let date: NaiveDate = now.date_naive();
    date.format("%Y-%m-%d").to_string()
}

/// Grant ids must be unique across every process sharing one ledger. The pid
/// separates processes, the millisecond stamp separates pid reuse across
/// reboots, and the counter separates grants inside one process.
fn next_grant_id(now: DateTime<Utc>) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("p{}-{}-{seq}", std::process::id(), now.timestamp_millis())
}

/// One tracker's binding to the durable daily ceiling: which ledger, which
/// subject bucket, and how long a reservation survives an unclean exit.
///
/// Process-local wiring, never part of a `BudgetTrackerSnapshot`: the authority
/// itself lives in the file, not in the tracker.
#[derive(Debug, Clone)]
pub struct DailyAuthority {
    store: Arc<DailySpendStore>,
    subject: Arc<str>,
    lease: Duration,
}

impl DailyAuthority {
    /// Bind `subject`'s bucket in `store` with the default reservation lease.
    pub fn new(store: Arc<DailySpendStore>, subject: impl Into<String>) -> Self {
        Self {
            store,
            subject: Arc::from(subject.into().as_str()),
            lease: Duration::seconds(DEFAULT_RESERVATION_LEASE_SECS),
        }
    }

    /// Override how long a durable reservation survives a process that dies
    /// before settling it.
    pub fn with_lease(mut self, lease: Duration) -> Self {
        self.lease = lease;
        self
    }

    /// The subject bucket this authority debits.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// The ledger this authority debits.
    pub fn store(&self) -> &Arc<DailySpendStore> {
        &self.store
    }

    pub(crate) fn reserve(
        &self,
        usd: f64,
        cap: f64,
        now: DateTime<Utc>,
    ) -> Result<DailyGrant, DailySpendError> {
        self.store.reserve(&self.subject, usd, cap, self.lease, now)
    }

    pub(crate) fn settle(
        &self,
        grant: &DailyGrant,
        actual_usd: f64,
        now: DateTime<Utc>,
    ) -> Result<(), DailySpendError> {
        self.store.settle(&self.subject, grant, actual_usd, now)
    }

    pub(crate) fn release(
        &self,
        grant: &DailyGrant,
        now: DateTime<Utc>,
    ) -> Result<(), DailySpendError> {
        self.store.release(&self.subject, grant, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(dir: &std::path::Path) -> DailySpendStore {
        DailySpendStore::at(dir.join("nested").join("daily-spend.json"))
    }

    fn lease() -> Duration {
        Duration::minutes(30)
    }

    #[test]
    fn missing_store_is_an_empty_ledger_and_admits_the_first_call() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let now = Utc::now();

        let grant = store.reserve("default", 0.25, 1.00, lease(), now).unwrap();
        store.settle("default", &grant, 0.20, now).unwrap();

        let position = store.position("default", now).unwrap();
        assert!((position.committed_usd - 0.20).abs() < 1e-9);
        assert_eq!(position.reserved_usd, 0.0);
        assert!(store.path().exists(), "ledger was published");
    }

    #[test]
    fn settled_spend_accumulates_across_independent_store_handles() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();

        for _ in 0..4 {
            // A separate handle each round models a separate process: nothing
            // is carried in memory between them.
            let store = store(dir.path());
            let grant = store.reserve("default", 0.25, 1.00, lease(), now).unwrap();
            store.settle("default", &grant, 0.25, now).unwrap();
        }

        let store = store(dir.path());
        let refusal = store.reserve("default", 0.25, 1.00, lease(), now);
        assert!(
            matches!(refusal, Err(DailySpendError::Exceeded { .. })),
            "fifth fresh-process call must be refused, got {refusal:?}"
        );
    }

    #[test]
    fn in_flight_reservations_bound_a_concurrent_process() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let first = store(dir.path());
        let second = store(dir.path());

        // First process holds an unsettled reservation; the second must see it.
        let _held = first.reserve("default", 0.90, 1.00, lease(), now).unwrap();
        let refusal = second.reserve("default", 0.20, 1.00, lease(), now);
        assert!(matches!(refusal, Err(DailySpendError::Exceeded { .. })));
    }

    #[test]
    fn an_expired_reservation_is_reclaimed_rather_than_leaked_forever() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let store = store(dir.path());

        // Model a process that died between reserve and settle.
        let _abandoned = store
            .reserve("default", 0.90, 1.00, Duration::minutes(30), now)
            .unwrap();
        assert!(
            store
                .reserve("default", 0.20, 1.00, lease(), now + Duration::minutes(29))
                .is_err(),
            "the lease must still bind before it expires"
        );
        store
            .reserve("default", 0.20, 1.00, lease(), now + Duration::minutes(31))
            .expect("the lease expired, so the authority is reclaimed");
    }

    #[test]
    fn released_reservations_do_not_consume_authority() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let store = store(dir.path());

        let grant = store.reserve("default", 0.90, 1.00, lease(), now).unwrap();
        store.release("default", &grant, now).unwrap();

        store
            .reserve("default", 0.90, 1.00, lease(), now)
            .expect("released authority is available again");
    }

    #[test]
    fn settlement_records_spend_even_when_the_grant_already_expired() {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc::now();
        let store = store(dir.path());

        let grant = store
            .reserve("default", 0.50, 1.00, Duration::seconds(1), now)
            .unwrap();
        let later = now + Duration::minutes(5);
        store.settle("default", &grant, 0.50, later).unwrap();

        let position = store.position("default", later).unwrap();
        assert!((position.committed_usd - 0.50).abs() < 1e-9);
    }

    #[test]
    fn the_bucket_resets_at_the_utc_day_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let now = Utc::now();

        let grant = store.reserve("default", 1.00, 1.00, lease(), now).unwrap();
        store.settle("default", &grant, 1.00, now).unwrap();
        assert!(store.reserve("default", 0.10, 1.00, lease(), now).is_err());

        let tomorrow = now + Duration::days(1);
        store
            .reserve("default", 0.10, 1.00, lease(), tomorrow)
            .expect("a new UTC day starts from zero");
    }

    #[test]
    fn subjects_are_independent_buckets() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let now = Utc::now();

        let grant = store.reserve("alice", 1.00, 1.00, lease(), now).unwrap();
        store.settle("alice", &grant, 1.00, now).unwrap();

        store
            .reserve("bob", 1.00, 1.00, lease(), now)
            .expect("bob has his own daily authority");
    }

    #[test]
    fn a_corrupt_ledger_fails_closed_instead_of_resetting_the_ceiling() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let now = Utc::now();
        let grant = store.reserve("default", 0.10, 1.00, lease(), now).unwrap();
        store.settle("default", &grant, 0.10, now).unwrap();

        std::fs::write(store.path(), b"{ not json").unwrap();

        let refusal = store.reserve("default", 0.10, 1.00, lease(), now);
        assert!(
            matches!(refusal, Err(DailySpendError::Unusable { .. })),
            "a ceiling that resets when its file is overwritten is no ceiling, got {refusal:?}"
        );
    }

    #[test]
    fn a_refused_reservation_leaves_no_trace_in_the_ledger() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let now = Utc::now();

        assert!(store.reserve("default", 5.00, 1.00, lease(), now).is_err());
        let position = store.position("default", now).unwrap();
        assert_eq!(position.total_usd(), 0.0);
    }

    #[test]
    fn concurrent_reservations_never_oversubscribe_the_ceiling() {
        // 16 threads, each a would-be process, race 1 unit against a cap of 8.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("race").join("daily-spend.json");
        let now = Utc::now();
        let admitted = std::sync::Arc::new(AtomicU64::new(0));

        std::thread::scope(|scope| {
            for _ in 0..16 {
                let path = path.clone();
                let admitted = std::sync::Arc::clone(&admitted);
                scope.spawn(move || {
                    let store = DailySpendStore::at(path);
                    if let Ok(grant) = store.reserve("default", 1.0, 8.0, lease(), now) {
                        store.settle("default", &grant, 1.0, now).unwrap();
                        admitted.fetch_add(1, Ordering::SeqCst);
                    }
                });
            }
        });

        assert_eq!(
            admitted.load(Ordering::SeqCst),
            8,
            "exactly the ceiling may be admitted, no more and no fewer"
        );
        let store = DailySpendStore::at(&path);
        let position = store.position("default", now).unwrap();
        assert!((position.committed_usd - 8.0).abs() < 1e-9);
    }
}
