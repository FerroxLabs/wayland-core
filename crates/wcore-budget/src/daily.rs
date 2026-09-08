//! Durable daily claims: expiry preserves uncertainty, and settlement is idempotent.
//! Claims are charged to their originating UTC day. Seven days of retired
//! evidence are retained; live authority is never pruned to satisfy retention.
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use thiserror::Error;

pub const DAILY_LEDGER_SCHEMA_VERSION: u32 = 2;
/// Expiry turns a live reservation into uncertainty; it never proves no-send.
pub const DEFAULT_RESERVATION_LEASE_SECS: i64 = 30 * 60;
const USD_EPSILON: f64 = 1e-9;
const RETAINED_DAYS: i64 = 7;

#[derive(Debug, Clone, PartialEq, Error)]
pub enum DailySpendError {
    #[error("daily spend cap exceeded: limit=${limit:.4}, observed=${observed:.4}")]
    Exceeded { limit: f64, observed: f64 },
    #[error("daily spend ledger at {path} is unusable: {reason}")]
    Unusable { path: String, reason: String },
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

/// The binding to a persisted claim, including its original subject and day.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DailyGrant {
    id: String,
    subject: String,
    day: String,
}
impl DailyGrant {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub(crate) fn valid(&self) -> bool {
        !self.id.is_empty()
            && !self.subject.is_empty()
            && NaiveDate::parse_from_str(&self.day, "%Y-%m-%d").is_ok()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DailyPosition {
    pub committed_usd: f64,
    /// Unresolved claims, including expired uncertainty from the current day.
    pub reserved_usd: f64,
}
impl DailyPosition {
    pub fn total_usd(&self) -> f64 {
        self.committed_usd + self.reserved_usd
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum ClaimState {
    Reserved,
    Uncertain,
    Settled { actual_usd: f64 },
    Released,
}
impl ClaimState {
    fn unresolved(&self) -> bool {
        matches!(self, Self::Reserved | Self::Uncertain)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct ReservationRecord {
    usd: f64,
    expires_at_unix_ms: i64,
    status: ClaimState,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct SubjectLedger {
    days: BTreeMap<String, BTreeMap<String, ReservationRecord>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
struct DailyLedgerFile {
    schema_version: u32,
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
#[derive(Debug)]
pub struct DailySpendStore {
    path: PathBuf,
}

impl DailySpendStore {
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

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
            ledger.refresh(now);
            let entry = ledger.subjects.entry(subject.to_owned()).or_default();
            let position = entry.position(now, &self.path)?;
            let observed = position.total_usd() + usd;
            if observed > cap + USD_EPSILON {
                return Err(DailySpendError::Exceeded {
                    limit: cap,
                    observed,
                });
            }
            let grant = DailyGrant {
                id: next_grant_id(now),
                subject: subject.to_owned(),
                day: day_key(now),
            };
            entry.days.entry(grant.day.clone()).or_default().insert(
                grant.id.clone(),
                ReservationRecord {
                    usd,
                    expires_at_unix_ms: (now + lease).timestamp_millis(),
                    status: ClaimState::Reserved,
                },
            );
            Ok(grant)
        })
    }

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
            let claim = self.claim(ledger, subject, grant, now)?;
            match claim.status {
                ClaimState::Reserved | ClaimState::Uncertain => {
                    claim.status = ClaimState::Settled { actual_usd }
                }
                ClaimState::Settled { actual_usd: prior } if prior == actual_usd => {}
                _ => {
                    return Err(DailySpendError::unusable(
                        &self.path,
                        "conflicting daily settlement",
                    ));
                }
            }
            Ok(())
        })
    }

    fn settle_conservatively(
        &self,
        subject: &str,
        grant: &DailyGrant,
        now: DateTime<Utc>,
    ) -> Result<(), DailySpendError> {
        self.with_locked_ledger(|ledger| {
            let claim = self.claim(ledger, subject, grant, now)?;
            match claim.status {
                ClaimState::Reserved => claim.status = ClaimState::Uncertain,
                ClaimState::Uncertain | ClaimState::Settled { .. } => {}
                ClaimState::Released => {
                    return Err(DailySpendError::unusable(
                        &self.path,
                        "released claim cannot become uncertain",
                    ));
                }
            }
            Ok(())
        })
    }

    /// Release requires the caller's proof that the admitted call never sent.
    pub fn release(
        &self,
        subject: &str,
        grant: &DailyGrant,
        now: DateTime<Utc>,
    ) -> Result<(), DailySpendError> {
        self.with_locked_ledger(|ledger| {
            let claim = self.claim(ledger, subject, grant, now)?;
            match claim.status {
                ClaimState::Reserved | ClaimState::Uncertain | ClaimState::Released => {
                    claim.status = ClaimState::Released
                }
                ClaimState::Settled { .. } => {
                    return Err(DailySpendError::unusable(
                        &self.path,
                        "settled claim cannot be refunded as no-send",
                    ));
                }
            }
            Ok(())
        })
    }

    fn claim<'a>(
        &self,
        ledger: &'a mut DailyLedgerFile,
        subject: &str,
        grant: &DailyGrant,
        now: DateTime<Utc>,
    ) -> Result<&'a mut ReservationRecord, DailySpendError> {
        if !grant.valid() || grant.subject != subject {
            return Err(DailySpendError::unusable(
                &self.path,
                "daily grant subject or identity mismatch",
            ));
        }
        ledger.refresh(now);
        // A live claim can outlast retention. It must remain resolvable so
        // its fail-closed admission block can be cleared with real evidence.
        if grant.day > day_key(now) {
            return Err(DailySpendError::unusable(
                &self.path,
                "daily grant lies outside retained accounting history",
            ));
        }
        ledger
            .subjects
            .get_mut(subject)
            .and_then(|entry| entry.days.get_mut(&grant.day))
            .and_then(|claims| claims.get_mut(&grant.id))
            .ok_or_else(|| {
                DailySpendError::unusable(
                    &self.path,
                    "unknown daily grant; refusing unbound settlement",
                )
            })
    }

    pub fn position(
        &self,
        subject: &str,
        now: DateTime<Utc>,
    ) -> Result<DailyPosition, DailySpendError> {
        self.load()?
            .subjects
            .get(subject)
            .map(|entry| entry.position(now, &self.path))
            .unwrap_or(Ok(DailyPosition {
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
        for entry in ledger.subjects.values() {
            for (day, claims) in &entry.days {
                if NaiveDate::parse_from_str(day, "%Y-%m-%d").is_err() {
                    return Err(DailySpendError::unusable(
                        &self.path,
                        "invalid daily origin day",
                    ));
                }
                for (id, claim) in claims {
                    let valid_actual = match claim.status {
                        ClaimState::Settled { actual_usd } => {
                            actual_usd.is_finite() && actual_usd >= 0.0
                        }
                        _ => true,
                    };
                    if id.is_empty() || !claim.usd.is_finite() || claim.usd < 0.0 || !valid_actual {
                        return Err(DailySpendError::unusable(&self.path, "invalid daily claim"));
                    }
                }
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
    fn refresh(&mut self, now: DateTime<Utc>) {
        let oldest = day_key(now - Duration::days(RETAINED_DAYS - 1));
        let stamp = now.timestamp_millis();
        for entry in self.subjects.values_mut() {
            for claims in entry.days.values_mut() {
                for claim in claims.values_mut() {
                    if claim.expires_at_unix_ms <= stamp
                        && matches!(claim.status, ClaimState::Reserved)
                    {
                        claim.status = ClaimState::Uncertain;
                    }
                }
            }
            entry.days.retain(|day, claims| {
                day >= &oldest
                    || claims
                        .values()
                        .any(|c| c.status.unresolved() && c.expires_at_unix_ms > stamp)
            });
        }
        self.subjects.retain(|_, entry| !entry.days.is_empty());
    }
}
impl SubjectLedger {
    fn position(&self, now: DateTime<Utc>, path: &Path) -> Result<DailyPosition, DailySpendError> {
        let today = day_key(now);
        let oldest = day_key(now - Duration::days(RETAINED_DAYS - 1));
        let mut position = DailyPosition {
            committed_usd: 0.0,
            reserved_usd: 0.0,
        };
        for (day, claims) in &self.days {
            for claim in claims.values() {
                let live =
                    claim.status.unresolved() && claim.expires_at_unix_ms > now.timestamp_millis();
                if day > &today || (day < &oldest && live) {
                    return Err(DailySpendError::unusable(
                        path,
                        "live daily authority outside supported accounting window",
                    ));
                }
                match claim.status {
                    ClaimState::Settled { actual_usd } if day == &today => {
                        position.committed_usd += actual_usd
                    }
                    ClaimState::Reserved | ClaimState::Uncertain if day == &today || live => {
                        position.reserved_usd += claim.usd
                    }
                    _ => {}
                }
            }
        }
        Ok(position)
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

    pub(crate) fn settle_conservatively(
        &self,
        grant: &DailyGrant,
        now: DateTime<Utc>,
    ) -> Result<(), DailySpendError> {
        self.store.settle_conservatively(&self.subject, grant, now)
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
mod tests;
