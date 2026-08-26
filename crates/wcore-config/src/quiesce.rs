//! Quiesced snapshot lease over profile state (wayland#896).
//!
//! Desktop's recovery-point capture needs a bounded, producer-owned window in
//! which Core's profile state is read-consistent, plus an honest answer to the
//! only question that matters afterwards: *did anything move while I copied?*
//! Without that a host is left with filesystem timestamps, process killing, or
//! an uncoordinated `cp -r` — three ways of guessing.
//!
//! This module is the mechanism. The wire contract lives in
//! `wcore_protocol::quiescence` and the JSON-stream bridge in
//! `wcore_cli::quiesce_control`; nothing here knows about either.
//!
//! ## Coverage is all of it, or it is a refusal
//!
//! "All named profile state" means [`crate::profile::list_profiles`], not an
//! assumption that there is one profile. A requested root that does not exist,
//! or that cannot be read end to end, is [`QuiesceError::PartialCoverage`] —
//! never a quiet success over the subset that happened to work. A grant that
//! covers zero roots is refused for the same reason: an empty capture that
//! reports success is the fail-open predicate this contract exists to remove.
//!
//! ## The epoch is an observation, not a promise
//!
//! [`LeaseRecord::epoch`] is a SHA-256 over the per-root digests, and each root
//! digest is taken over every entry's relative path, kind, length and **content
//! hash**. Metadata alone (size + mtime) is cheaper and unsound: a same-length
//! rewrite inside one timestamp tick reads as unchanged, and a false *clean* is
//! the one error this contract may not make. A false *mutated* only costs a
//! retry.
//!
//! Acquire records the epoch; release recomputes it and reports
//! [`ReleaseVerdict::Mutated`] when they differ. That detection covers every
//! writer — Core's own code, a second Core process, an editor, the user —
//! because it observes the bytes rather than trusting a counter that some write
//! path forgot to bump.
//!
//! ## The lock holds no file descriptor
//!
//! The session-journal lock in this codebase leaked through `fork()` and
//! refused 47.6% of reopens under load; the repair was the symmetric unlock,
//! not a retry. This lease is deliberately descriptor-free: exclusivity is an
//! `O_CREAT|O_EXCL` record on disk plus a wall-clock expiry, so there is no
//! inherited fd to leak and no advisory lock a child can hold against its
//! parent. Every failure path that created the record removes it again
//! ([`LeaseHandle::release`] and its `Drop` do the same), and a holder that
//! dies without releasing forfeits at `expires_unix_ms` — reclaimable, never
//! wedged.
//!
//! Expiry is *observed*, not scheduled: Core is not a daemon, so a lapsed lease
//! is reclaimed and reported by the next [`acquire`], [`release`] or [`status`]
//! that meets it.
//!
//! ## The control plane lives outside every covered root
//!
//! [`control_root`] is `.quiesce` under [`crate::profile::profiles_root`]. A
//! leading `.` is rejected by [`crate::profile::validate_profile_name`], so the
//! directory can never be mistaken for a profile by `list_profiles` and no
//! reserved-name list has to grow (which would have invalidated an existing
//! profile of that name). Writing the lease record therefore cannot mutate the
//! state the lease is freezing — and when a pathological `WAYLAND_HOME` would
//! make it so anyway, [`QuiesceError::ControlPlaneConflict`] refuses.

use std::collections::VecDeque;
use std::fs::{self, File};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::profile::{list_profiles, profile_dir, profiles_root};

/// On-disk lease-record version. Separate from the wire version: a host
/// reasons about frames, this reasons about the file two Core processes share.
pub const QUIESCE_MECHANISM_VERSION: u32 = 1;

/// Shortest lease a caller may ask for. Below this the window closes before a
/// copy of any real profile can start.
pub const MIN_LEASE_TTL_MS: u64 = 1_000;

/// Longest lease a caller may ask for. A lease is a write freeze; an unbounded
/// one is an outage with a receipt.
pub const MAX_LEASE_TTL_MS: u64 = 15 * 60 * 1_000;

/// Bound on the opaque identifiers a host supplies.
pub const MAX_IDENTIFIER_LEN: usize = 128;

/// Control-plane directory name under [`profiles_root`]. The leading `.` is
/// load-bearing — see the module note.
const CONTROL_DIR: &str = ".quiesce";
const LEASE_FILE: &str = "lease.json";

const HASH_CHUNK: usize = 64 * 1024;

/// Which profile a covered root is.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RootIdentity {
    /// The active `WAYLAND_HOME` / `~/.wayland` home.
    Default,
    /// A named profile under [`profiles_root`].
    Named { name: String },
}

impl RootIdentity {
    /// Stable label used in refusal detail and in the covered-root digest.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Default => "default".to_string(),
            Self::Named { name } => format!("profile:{name}"),
        }
    }
}

/// One root the lease covers, with the digest that root contributed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoveredRoot {
    pub identity: RootIdentity,
    pub path: PathBuf,
    pub digest: String,
    pub file_count: u64,
    pub byte_count: u64,
}

/// Which named profiles a request wants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileSelector {
    /// Every profile [`list_profiles`] enumerates at acquire time.
    All,
    /// Exactly these, all of which must exist.
    Named(Vec<String>),
}

/// What a lease must cover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseScope {
    pub include_default: bool,
    pub profiles: ProfileSelector,
}

/// A host's acquire request, already free of wire concerns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseRequest {
    pub lease_id: String,
    pub owner: String,
    pub scope: LeaseScope,
    pub ttl_ms: u64,
}

/// Every way a lease operation refuses. Closed, and each variant is a distinct
/// remedy — collapsing any two of them is how a host builds a retry loop
/// against a condition that will never clear.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QuiesceError {
    /// One or more requested roots is absent or unreadable. Never downgraded to
    /// a partial success.
    #[error("coverage is incomplete: {missing:?}")]
    PartialCoverage { missing: Vec<String> },
    /// A different, live lease holds the control plane.
    #[error("lease {holder_lease_id} holds capture until {expires_unix_ms}")]
    ConcurrentCapture {
        holder_lease_id: String,
        expires_unix_ms: u64,
    },
    /// The caller's view of the lease is behind: a reused id with a different
    /// scope, an epoch echo that was never granted, or a lease that lapsed
    /// before it was released.
    #[error("lease {lease_id} is stale: {detail}")]
    StaleLease { lease_id: String, detail: String },
    /// No lease with that id is held.
    #[error("no lease {lease_id} is held")]
    UnknownLease { lease_id: String },
    /// The lease control plane resolves inside a root the lease would cover, so
    /// recording the lease would mutate the state it freezes.
    #[error("lease control plane {control} is inside covered root {root}")]
    ControlPlaneConflict { control: String, root: String },
    /// The request could not be honoured as written.
    #[error("invalid request: {0}")]
    InvalidRequest(&'static str),
    /// The control plane itself could not be read or written.
    #[error("lease control plane unavailable: {0}")]
    ControlPlaneUnavailable(String),
}

/// A lease that lapsed and was reclaimed, reported so the receipt trail has no
/// silent gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiredLease {
    pub lease_id: String,
    pub owner: String,
    pub epoch: String,
    pub expires_unix_ms: u64,
    pub observed_unix_ms: u64,
}

/// The durable lease record. This file IS the lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeaseRecord {
    pub mechanism_version: u32,
    pub lease_id: String,
    pub owner: String,
    pub holder_pid: u32,
    pub acquired_unix_ms: u64,
    pub expires_unix_ms: u64,
    /// Empty only while a claim is being sealed — see [`acquire`].
    pub epoch: String,
    pub roots: Vec<CoveredRoot>,
}

impl LeaseRecord {
    /// Whether this lease has lapsed at `now_unix_ms`.
    #[must_use]
    pub fn is_expired(&self, now_unix_ms: u64) -> bool {
        now_unix_ms >= self.expires_unix_ms
    }

    fn root_labels(&self) -> Vec<String> {
        self.roots.iter().map(|r| r.identity.label()).collect()
    }
}

/// A granted lease, plus what granting it revealed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseGrant {
    pub record: LeaseRecord,
    /// True when this acquire re-observed a lease the same `lease_id` already
    /// held. The epoch is NOT recomputed — an idempotent retry must return the
    /// same answer, not a fresher one.
    pub idempotent_replay: bool,
    /// A lapsed lease this acquire reclaimed on its way in.
    pub reclaimed: Option<ExpiredLease>,
}

/// Whether the covered state moved during the lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseVerdict {
    /// Every covered root hashes exactly as it did at acquire.
    Clean,
    /// Something moved. The capture taken under this lease is not a valid
    /// recovery point.
    Mutated,
}

/// The receipt a release produces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseReceipt {
    pub lease_id: String,
    pub owner: String,
    pub epoch_at_acquire: String,
    pub epoch_at_release: String,
    pub verdict: ReleaseVerdict,
    pub released_unix_ms: u64,
}

/// What [`status`] can see without changing anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusReport {
    /// A live lease, if one is held.
    pub held: Option<LeaseRecord>,
    /// A lapsed lease observed and reclaimed by this call.
    pub reclaimed: Option<ExpiredLease>,
    /// Roots a lease could cover right now.
    pub available: Vec<RootIdentity>,
}

/// The lease control-plane directory. Outside every profile home by
/// construction — see the module note.
#[must_use]
pub fn control_root() -> PathBuf {
    profiles_root().join(CONTROL_DIR)
}

fn lease_path() -> PathBuf {
    control_root().join(LEASE_FILE)
}

/// Milliseconds since the unix epoch, saturating rather than panicking on a
/// clock before 1970.
#[must_use]
pub fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

fn unavailable(context: &str, error: &io::Error) -> QuiesceError {
    QuiesceError::ControlPlaneUnavailable(format!("{context}: {error}"))
}

fn validate_identifier(value: &str, what: &'static str) -> Result<(), QuiesceError> {
    if value.is_empty() {
        return Err(QuiesceError::InvalidRequest(what));
    }
    if value.len() > MAX_IDENTIFIER_LEN {
        return Err(QuiesceError::InvalidRequest(what));
    }
    if !value
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b':'))
    {
        return Err(QuiesceError::InvalidRequest(what));
    }
    Ok(())
}

// --- root resolution -------------------------------------------------------

/// Resolve the scope to concrete roots, refusing anything short of complete
/// coverage.
///
/// Order is deterministic (default first, then named ascending) so the epoch is
/// reproducible regardless of the order a host listed profiles in.
fn resolve_roots(scope: &LeaseScope) -> Result<Vec<(RootIdentity, PathBuf)>, QuiesceError> {
    let mut resolved: Vec<(RootIdentity, PathBuf)> = Vec::new();
    let mut missing: Vec<String> = Vec::new();

    if scope.include_default {
        let home = crate::config::profile_home();
        if home.is_dir() {
            resolved.push((RootIdentity::Default, home));
        } else {
            missing.push(RootIdentity::Default.label());
        }
    }

    let mut names = match &scope.profiles {
        ProfileSelector::All => list_profiles(),
        ProfileSelector::Named(requested) => {
            if requested.is_empty() {
                return Err(QuiesceError::InvalidRequest(
                    "an explicit profile selection must name at least one profile",
                ));
            }
            requested.iter().map(|n| n.to_ascii_lowercase()).collect()
        }
    };
    names.sort();
    names.dedup();

    for name in names {
        match profile_dir(&name) {
            Ok(dir) if dir.is_dir() => {
                resolved.push((RootIdentity::Named { name }, dir));
            }
            _ => missing.push(RootIdentity::Named { name }.label()),
        }
    }

    if !missing.is_empty() {
        return Err(QuiesceError::PartialCoverage { missing });
    }
    // A capture of nothing that reports success is the fail-open answer this
    // contract exists to remove.
    if resolved.is_empty() {
        return Err(QuiesceError::PartialCoverage {
            missing: vec!["<request covers no root>".to_string()],
        });
    }
    Ok(resolved)
}

/// Refuse a scope whose coverage would swallow the lease's own control plane.
fn assert_control_plane_outside(roots: &[(RootIdentity, PathBuf)]) -> Result<(), QuiesceError> {
    let control = control_root();
    let control_real = fs::canonicalize(&control).unwrap_or_else(|_| control.clone());
    for (_, root) in roots {
        let root_real = fs::canonicalize(root).unwrap_or_else(|_| root.clone());
        if control_real.starts_with(&root_real) {
            return Err(QuiesceError::ControlPlaneConflict {
                control: control_real.display().to_string(),
                root: root_real.display().to_string(),
            });
        }
    }
    Ok(())
}

// --- digests ---------------------------------------------------------------

struct RootObservation {
    digest: String,
    file_count: u64,
    byte_count: u64,
}

fn hash_file(path: &Path) -> io::Result<(String, u64)> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; HASH_CHUNK];
    let mut total: u64 = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        total += read as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), total))
}

/// Observe one root.
///
/// `strict` decides what an unreadable entry means. At acquire it means the
/// coverage claim is not true, so the caller must refuse. At release it means
/// the covered state moved out from under the lease, so the entry hashes to a
/// stable sentinel and the verdict lands on `Mutated` instead of erroring — a
/// deleted profile is a mutation, not a control-plane fault.
fn observe_root(root: &Path, strict: bool) -> Result<RootObservation, io::Error> {
    let mut entries: Vec<String> = Vec::new();
    let mut file_count: u64 = 0;
    let mut byte_count: u64 = 0;
    let mut queue: VecDeque<PathBuf> = VecDeque::new();
    queue.push_back(root.to_path_buf());

    while let Some(dir) = queue.pop_front() {
        let read_dir = match fs::read_dir(&dir) {
            Ok(iter) => iter,
            Err(error) if strict => return Err(error),
            Err(error) => {
                entries.push(format!(
                    "{}\u{0}unreadable-dir\u{0}{:?}",
                    relative_label(root, &dir),
                    error.kind()
                ));
                continue;
            }
        };
        for entry in read_dir {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) if strict => return Err(error),
                Err(error) => {
                    entries.push(format!("?\u{0}unreadable-entry\u{0}{:?}", error.kind()));
                    continue;
                }
            };
            let path = entry.path();
            let label = relative_label(root, &path);
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                Err(error) if strict => return Err(error),
                Err(error) => {
                    entries.push(format!(
                        "{label}\u{0}unreadable-meta\u{0}{:?}",
                        error.kind()
                    ));
                    continue;
                }
            };
            if meta.is_symlink() {
                let target = match fs::read_link(&path) {
                    Ok(target) => target.to_string_lossy().into_owned(),
                    Err(error) if strict => return Err(error),
                    Err(error) => format!("<unreadable:{:?}>", error.kind()),
                };
                entries.push(format!("{label}\u{0}l\u{0}{target}"));
            } else if meta.is_dir() {
                entries.push(format!("{label}\u{0}d"));
                queue.push_back(path);
            } else if meta.is_file() {
                match hash_file(&path) {
                    Ok((hash, len)) => {
                        file_count += 1;
                        byte_count += len;
                        entries.push(format!("{label}\u{0}f\u{0}{len}\u{0}{hash}"));
                    }
                    Err(error) if strict => return Err(error),
                    Err(error) => {
                        entries.push(format!(
                            "{label}\u{0}unreadable-file\u{0}{:?}",
                            error.kind()
                        ));
                    }
                }
            } else {
                entries.push(format!("{label}\u{0}o"));
            }
        }
    }

    entries.sort();
    let mut hasher = Sha256::new();
    for entry in &entries {
        hasher.update(entry.as_bytes());
        hasher.update([0_u8]);
    }
    Ok(RootObservation {
        digest: format!("sha256:{:x}", hasher.finalize()),
        file_count,
        byte_count,
    })
}

fn relative_label(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// Fold per-root digests into the opaque mutation epoch a host echoes back.
fn fold_epoch(roots: &[CoveredRoot]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(QUIESCE_MECHANISM_VERSION.to_string().as_bytes());
    hasher.update([0_u8]);
    for root in roots {
        hasher.update(root.identity.label().as_bytes());
        hasher.update([0_u8]);
        hasher.update(root.digest.as_bytes());
        hasher.update([0_u8]);
    }
    format!("sha256:{:x}", hasher.finalize())
}

fn observe_all(
    roots: &[(RootIdentity, PathBuf)],
    strict: bool,
) -> Result<Vec<CoveredRoot>, QuiesceError> {
    let mut covered = Vec::with_capacity(roots.len());
    let mut missing = Vec::new();
    for (identity, path) in roots {
        match observe_root(path, strict) {
            Ok(observation) => covered.push(CoveredRoot {
                identity: identity.clone(),
                path: path.clone(),
                digest: observation.digest,
                file_count: observation.file_count,
                byte_count: observation.byte_count,
            }),
            Err(_) => missing.push(identity.label()),
        }
    }
    if !missing.is_empty() {
        return Err(QuiesceError::PartialCoverage { missing });
    }
    Ok(covered)
}

// --- control-plane record --------------------------------------------------

fn read_record() -> Result<Option<LeaseRecord>, QuiesceError> {
    match fs::read(lease_path()) {
        Ok(bytes) => Ok(serde_json::from_slice::<LeaseRecord>(&bytes).ok()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(unavailable("read lease record", &error)),
    }
}

/// True when a lease file exists but does not parse as a record. Such a file
/// can never expire on its own, so acquire reclaims it rather than letting the
/// control plane wedge forever.
fn lease_file_present() -> Result<bool, QuiesceError> {
    match fs::metadata(lease_path()) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(unavailable("stat lease record", &error)),
    }
}

fn remove_lease_file() -> Result<(), QuiesceError> {
    match fs::remove_file(lease_path()) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(unavailable("remove lease record", &error)),
    }
}

fn write_record(record: &LeaseRecord) -> Result<(), QuiesceError> {
    let bytes = serde_json::to_vec(record)
        .map_err(|error| QuiesceError::ControlPlaneUnavailable(format!("encode lease: {error}")))?;
    crate::atomic_io::atomic_write(lease_path(), &bytes)
        .map_err(|error| unavailable("write lease record", &error))
}

/// Claim the lease file atomically. `Ok(false)` means someone else got there.
fn claim_lease_file(record: &LeaseRecord) -> Result<bool, QuiesceError> {
    let bytes = serde_json::to_vec(record)
        .map_err(|error| QuiesceError::ControlPlaneUnavailable(format!("encode lease: {error}")))?;
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(lease_path())
    {
        Ok(mut file) => {
            use std::io::Write as _;
            file.write_all(&bytes)
                .map_err(|error| unavailable("write lease claim", &error))?;
            file.sync_all()
                .map_err(|error| unavailable("sync lease claim", &error))?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(false),
        Err(error) => Err(unavailable("create lease record", &error)),
    }
}

// --- public operations -----------------------------------------------------

/// Acquire a read-consistent lease over the requested profile state.
///
/// Idempotent: re-issuing the same `lease_id` while that lease is live returns
/// the SAME grant, epoch included. Recomputing would make a retry answer a
/// different question than the call it retries.
///
/// Reusing a live `lease_id` with a different scope is [`QuiesceError::StaleLease`],
/// not a silent re-scope — a host that changed its mind must release first.
pub fn acquire(request: &LeaseRequest) -> Result<LeaseGrant, QuiesceError> {
    validate_identifier(&request.lease_id, "lease_id")?;
    validate_identifier(&request.owner, "owner")?;
    if request.ttl_ms < MIN_LEASE_TTL_MS || request.ttl_ms > MAX_LEASE_TTL_MS {
        return Err(QuiesceError::InvalidRequest(
            "ttl_ms outside the supported lease window",
        ));
    }

    let control = control_root();
    fs::create_dir_all(&control).map_err(|error| unavailable("create control plane", &error))?;

    let roots = resolve_roots(&request.scope)?;
    assert_control_plane_outside(&roots)?;

    let now = now_unix_ms();
    let mut reclaimed: Option<ExpiredLease> = None;

    match read_record()? {
        Some(existing) if !existing.is_expired(now) => {
            if existing.lease_id == request.lease_id {
                let requested: Vec<String> =
                    roots.iter().map(|(identity, _)| identity.label()).collect();
                if requested != existing.root_labels() {
                    return Err(QuiesceError::StaleLease {
                        lease_id: request.lease_id.clone(),
                        detail: "lease id is live with a different coverage scope".to_string(),
                    });
                }
                if existing.epoch.is_empty() {
                    // A claim that has not finished sealing. Treat as live and
                    // contended rather than handing back a half-built grant.
                    return Err(QuiesceError::ConcurrentCapture {
                        holder_lease_id: existing.lease_id,
                        expires_unix_ms: existing.expires_unix_ms,
                    });
                }
                return Ok(LeaseGrant {
                    record: existing,
                    idempotent_replay: true,
                    reclaimed: None,
                });
            }
            return Err(QuiesceError::ConcurrentCapture {
                holder_lease_id: existing.lease_id,
                expires_unix_ms: existing.expires_unix_ms,
            });
        }
        Some(expired) => {
            reclaimed = Some(ExpiredLease {
                lease_id: expired.lease_id.clone(),
                owner: expired.owner.clone(),
                epoch: expired.epoch.clone(),
                expires_unix_ms: expired.expires_unix_ms,
                observed_unix_ms: now,
            });
            remove_lease_file()?;
        }
        None => {
            // A present-but-unparsable file has no expiry to reach, so leaving
            // it would wedge the control plane permanently.
            if lease_file_present()? {
                reclaimed = Some(ExpiredLease {
                    lease_id: "<unparsable>".to_string(),
                    owner: "<unknown>".to_string(),
                    epoch: String::new(),
                    expires_unix_ms: 0,
                    observed_unix_ms: now,
                });
                remove_lease_file()?;
            }
        }
    }

    // Claim FIRST, observe second: the freeze must be in force while the epoch
    // is being taken, or two captures can interleave inside the observation.
    let mut record = LeaseRecord {
        mechanism_version: QUIESCE_MECHANISM_VERSION,
        lease_id: request.lease_id.clone(),
        owner: request.owner.clone(),
        holder_pid: std::process::id(),
        acquired_unix_ms: now,
        expires_unix_ms: now.saturating_add(request.ttl_ms),
        epoch: String::new(),
        roots: Vec::new(),
    };
    if !claim_lease_file(&record)? {
        let holder = read_record()?;
        return Err(QuiesceError::ConcurrentCapture {
            holder_lease_id: holder
                .as_ref()
                .map_or_else(|| "<unparsable>".to_string(), |r| r.lease_id.clone()),
            expires_unix_ms: holder.as_ref().map_or(0, |r| r.expires_unix_ms),
        });
    }

    // Symmetric unlock: every path out of here that fails releases the claim it
    // just took. An asymmetric failure path is how the journal lock wedged.
    let covered = match observe_all(&roots, true) {
        Ok(covered) => covered,
        Err(error) => {
            let _ = remove_lease_file();
            return Err(error);
        }
    };
    record.epoch = fold_epoch(&covered);
    record.roots = covered;
    if let Err(error) = write_record(&record) {
        let _ = remove_lease_file();
        return Err(error);
    }

    Ok(LeaseGrant {
        record,
        idempotent_replay: false,
        reclaimed,
    })
}

/// Release a lease and report whether the covered state moved while it was
/// held.
///
/// `epoch_at_acquire` is the epoch the caller was granted. A mismatch is
/// [`QuiesceError::StaleLease`] and does NOT release: an actor holding a stale
/// epoch has no business freeing a live lease.
pub fn release(lease_id: &str, epoch_at_acquire: &str) -> Result<ReleaseReceipt, QuiesceError> {
    validate_identifier(lease_id, "lease_id")?;
    let now = now_unix_ms();

    let Some(record) = read_record()? else {
        return Err(QuiesceError::UnknownLease {
            lease_id: lease_id.to_string(),
        });
    };
    if record.lease_id != lease_id {
        return Err(QuiesceError::UnknownLease {
            lease_id: lease_id.to_string(),
        });
    }
    if record.epoch != epoch_at_acquire {
        return Err(QuiesceError::StaleLease {
            lease_id: lease_id.to_string(),
            detail: "epoch echo does not match the granted epoch".to_string(),
        });
    }
    if record.is_expired(now) {
        remove_lease_file()?;
        return Err(QuiesceError::StaleLease {
            lease_id: lease_id.to_string(),
            detail: "lease expired before it was released".to_string(),
        });
    }

    let roots: Vec<(RootIdentity, PathBuf)> = record
        .roots
        .iter()
        .map(|root| (root.identity.clone(), root.path.clone()))
        .collect();
    // Lossy: a root that vanished under the lease is a mutation, not a
    // control-plane fault, and must land on a verdict rather than an error.
    let observed = observe_all(&roots, false).unwrap_or_default();
    let epoch_at_release = fold_epoch(&observed);
    let verdict = if epoch_at_release == record.epoch {
        ReleaseVerdict::Clean
    } else {
        ReleaseVerdict::Mutated
    };
    remove_lease_file()?;

    Ok(ReleaseReceipt {
        lease_id: record.lease_id,
        owner: record.owner,
        epoch_at_acquire: record.epoch,
        epoch_at_release,
        verdict,
        released_unix_ms: now,
    })
}

/// Report the control plane without granting anything. Reclaims a lapsed lease
/// it meets, so a crashed holder does not have to wait for the next acquire to
/// be reported.
pub fn status() -> Result<StatusReport, QuiesceError> {
    let now = now_unix_ms();
    let mut available = vec![RootIdentity::Default];
    available.extend(
        list_profiles()
            .into_iter()
            .map(|name| RootIdentity::Named { name }),
    );

    let record = read_record()?;
    match record {
        Some(record) if record.is_expired(now) => {
            let reclaimed = ExpiredLease {
                lease_id: record.lease_id,
                owner: record.owner,
                epoch: record.epoch,
                expires_unix_ms: record.expires_unix_ms,
                observed_unix_ms: now,
            };
            remove_lease_file()?;
            Ok(StatusReport {
                held: None,
                reclaimed: Some(reclaimed),
                available,
            })
        }
        Some(record) => Ok(StatusReport {
            held: Some(record),
            reclaimed: None,
            available,
        }),
        None => Ok(StatusReport {
            held: None,
            reclaimed: None,
            available,
        }),
    }
}

/// RAII holder for in-process callers.
///
/// `Drop` releases the claim, and releases ONLY a record that still names this
/// lease. Removing a record it does not own is the asymmetric unlock that let
/// one holder free another's lock.
#[derive(Debug)]
pub struct LeaseHandle {
    lease_id: String,
    epoch: String,
    released: bool,
}

impl LeaseHandle {
    /// Take ownership of a granted lease.
    #[must_use]
    pub fn adopt(grant: &LeaseGrant) -> Self {
        Self {
            lease_id: grant.record.lease_id.clone(),
            epoch: grant.record.epoch.clone(),
            released: false,
        }
    }

    /// The lease this handle owns.
    #[must_use]
    pub fn lease_id(&self) -> &str {
        &self.lease_id
    }

    /// Release explicitly. Idempotent — a second call is a no-op, not a
    /// refusal, so `release()` followed by `Drop` cannot double-free.
    pub fn release(&mut self) -> Result<Option<ReleaseReceipt>, QuiesceError> {
        if self.released {
            return Ok(None);
        }
        self.released = true;
        release(&self.lease_id, &self.epoch).map(Some)
    }
}

impl Drop for LeaseHandle {
    fn drop(&mut self) {
        if self.released {
            return;
        }
        self.released = true;
        // Best effort, and strictly ours: read the record and only remove it
        // when it still names this lease.
        if let Ok(Some(record)) = read_record()
            && record.lease_id == self.lease_id
        {
            let _ = remove_lease_file();
        }
    }
}
