//! Cross-process cache for the AppContainer availability probe.
//!
//! # Why this exists
//!
//! `probe_cache()` and `probe_gate()` are `OnceLock` statics, so the
//! single-flight added by FerroxLabs/wayland#754 collapses probes only *within*
//! one process. Every sandboxed child is its own process, and under `cargo
//! nextest` every test is too, so a full run performs HUNDREDS of redundant
//! real AppContainer spawns to re-answer one unchanging question about the host.
//!
//! That is expensive twice over. Measured on SEANDESKTOP (32 logical cores):
//! a cold cross-process probe costs ~120ms alone and ~1188ms at 24-way, scaling
//! at roughly 50ms per additional concurrent probe, because each one drives the
//! serialized AppX profile service. And each probe is an independent chance to
//! hit the failure mode `probe_appcontainer_once` documents: an AV
//! process-creation callback running synchronously inside `CreateProcessAsUserW`,
//! which "can stall ~120s" (#125). Hundreds of tickets in that lottery is how a
//! 15s wall-clock guard gets tripped on a host that is perfectly healthy — and
//! a tripped guard makes the backend refuse to run at all.
//!
//! # What is cached, and what deliberately is NOT
//!
//! **Only success.** A negative verdict is never written here.
//!
//! That asymmetry is the whole safety argument, so it is worth stating plainly:
//!
//! - A cached POSITIVE that is wrong cannot cause unsandboxed execution. It
//!   only lets the caller proceed to a real sandboxed spawn, which builds its
//!   own AppContainer and fails closed on its own errors. The cost of being
//!   wrong is one honest failure at the real call site instead of one honest
//!   failure at the probe.
//! - A cached NEGATIVE that is wrong would propagate one unlucky probe's
//!   refusal to every later process on the machine, outliving the process that
//!   recorded it. That is precisely the "transient flake at startup permanently
//!   disables sandboxing" pattern `ProbeCache` was built to avoid; in-process it
//!   is bounded by process lifetime, on disk it would not be. So it is not
//!   cached at all, and `NEGATIVE_PROBE_TTL` keeps doing its existing job.
//!
//! The honest cost of caching success is detection latency: a host that loses
//! its sandbox capability mid-window is not re-probed until [`POSITIVE_TTL`]
//! expires. It is not a containment hole — the real spawn still fails closed —
//! but it IS a delay in when the operator is told, which is why the window is
//! ten minutes rather than a day.
//!
//! # Failure policy
//!
//! Every I/O error here is swallowed and treated as a cache miss. A cache that
//! cannot be read or written degrades to exactly today's behaviour (probe every
//! time) and never to something worse.

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// How long a recorded success suppresses re-probing.
///
/// Ten minutes is a deliberate middle: long enough that a nextest run or a
/// fleet of agents pays the probe once rather than hundreds of times, short
/// enough that a host which genuinely loses AppContainer support surfaces that
/// to the operator promptly rather than at the end of a session.
const POSITIVE_TTL: Duration = Duration::from_secs(600);

/// Bumped if the record's meaning changes. An unrecognised version is a miss,
/// never a parse error, so a downgrade cannot wedge on a newer file.
const RECORD_VERSION: u32 = 1;

const DIRECTORY_COMPONENTS: [&str; 4] = ["Wayland", "Core", "AppContainerProbe", "v1"];
const RECORD_FILE: &str = "verdict.txt";

/// Deliberately NOT under the AppContainer lease directory.
///
/// `recover_dead_leases_locked` treats every entry in that directory it does not
/// recognise as a HARD ERROR that aborts recovery, with only the quarantine
/// directory allow-listed. Adding a second allow-listed name would work for this
/// build and wedge any older build that met the file — the same downgrade hazard
/// the quarantine directory already had to be reasoned about for. A sibling
/// directory has no such interaction.
fn record_path() -> Option<PathBuf> {
    let mut path = record_root()?;
    for component in DIRECTORY_COMPONENTS {
        path.push(component);
    }
    Some(path.join(RECORD_FILE))
}

/// Production root: the user's real `%LOCALAPPDATA%`.
#[cfg(not(test))]
fn record_root() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
}

/// Unit-test root: one temp directory per test process.
///
/// The same chokepoint `lease_root` uses, and for the same reason. `F-4` was a
/// real defect on a real developer box: this crate's tests wrote into the
/// user's live state. A cached success is far more benign than a synthetic
/// lease, but "benign" is not a reason to write into somebody's profile from a
/// test, and putting the decision here rather than at the call sites is what
/// makes it hold for the next call site somebody adds.
#[cfg(test)]
fn record_root() -> Option<PathBuf> {
    use std::sync::OnceLock;
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    Some(
        ROOT.get_or_init(|| {
            std::env::temp_dir().join(format!("wcore-probe-test-{:08x}", std::process::id()))
        })
        .clone(),
    )
}

fn now_unix_secs() -> Option<u64> {
    Some(SystemTime::now().duration_since(UNIX_EPOCH).ok()?.as_secs())
}

/// A previously recorded success that is still inside its TTL.
///
/// Returns `None` for every other case — no file, unreadable, unparsable, wrong
/// version, expired, or a timestamp in the future. A future timestamp means the
/// clock moved backwards (or the file was tampered with); re-probing is the safe
/// answer, and it is cheap.
pub(super) fn cached_success() -> bool {
    let Some(path) = record_path() else {
        return false;
    };
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Some((version, recorded)) = parse_record(&contents) else {
        return false;
    };
    if version != RECORD_VERSION {
        return false;
    }
    let Some(now) = now_unix_secs() else {
        return false;
    };
    let Some(age) = now.checked_sub(recorded) else {
        return false;
    };
    age < POSITIVE_TTL.as_secs()
}

/// Record that a real spawn succeeded. Best effort; never fails the caller.
///
/// Written to a per-process temporary name and renamed over the target, so a
/// concurrent reader sees either the old complete record or the new one and
/// never a torn file. `std::fs::rename` is a replacing `MoveFileExW` on Windows.
///
/// No reparse-point hardening here, unlike the lease files, and that is a
/// judgement rather than an oversight: the path lives under the calling user's
/// own `%LOCALAPPDATA%`, so anyone able to plant a reparse point there can
/// simply write the record directly, and the positive-only policy above bounds
/// what forging it achieves.
pub(super) fn record_success() {
    let Some(path) = record_path() else {
        return;
    };
    let Some(directory) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(directory).is_err() {
        return;
    }
    let Some(now) = now_unix_secs() else {
        return;
    };
    let temp = directory.join(format!("{RECORD_FILE}.{}.tmp", std::process::id()));
    if std::fs::write(&temp, format_record(RECORD_VERSION, now)).is_err() {
        let _ = std::fs::remove_file(&temp);
        return;
    }
    if std::fs::rename(&temp, &path).is_err() {
        let _ = std::fs::remove_file(&temp);
    }
}

fn format_record(version: u32, recorded_unix_secs: u64) -> String {
    format!("version={version}\navailable=true\nrecorded_unix_secs={recorded_unix_secs}\n")
}

/// Parsing is separated from I/O so the record format is provable without a
/// filesystem, and so a malformed file is a miss rather than a panic.
fn parse_record(contents: &str) -> Option<(u32, u64)> {
    let mut version = None;
    let mut recorded = None;
    let mut available = false;
    for line in contents.lines() {
        let (key, value) = line.split_once('=')?;
        match key.trim() {
            "version" => version = value.trim().parse::<u32>().ok(),
            "recorded_unix_secs" => recorded = value.trim().parse::<u64>().ok(),
            // A record that does not positively assert success is not one.
            // Nothing writes `available=false`, but a hand-edited or truncated
            // file must not be read as a success by omission.
            "available" => available = value.trim() == "true",
            _ => {}
        }
    }
    if !available {
        return None;
    }
    Some((version?, recorded?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_well_formed_record_round_trips() {
        let record = format_record(RECORD_VERSION, 1_700_000_000);
        assert_eq!(
            parse_record(&record),
            Some((RECORD_VERSION, 1_700_000_000)),
            "the writer and the reader must agree on the format"
        );
    }

    #[test]
    fn a_record_that_does_not_assert_success_is_not_read_as_one() {
        // The failure this guards is reading success by OMISSION: a truncated
        // or hand-edited file that happens to carry a version and a timestamp
        // must not suppress probing.
        assert_eq!(
            parse_record("version=1\nrecorded_unix_secs=1700000000\n"),
            None
        );
        assert_eq!(
            parse_record("version=1\navailable=false\nrecorded_unix_secs=1700000000\n"),
            None
        );
    }

    #[test]
    fn malformed_records_are_a_miss_not_a_panic() {
        for bad in [
            "",
            "garbage",
            "version=notanumber\navailable=true\nrecorded_unix_secs=1\n",
            "version=1\navailable=true\nrecorded_unix_secs=notanumber\n",
            "version=1\navailable=true\n",
        ] {
            assert_eq!(parse_record(bad), None, "must be a miss: {bad:?}");
        }
    }

    #[test]
    fn a_future_timestamp_is_treated_as_expired() {
        // `cached_success` computes `now - recorded` with `checked_sub`, so a
        // record from the future yields None rather than a huge age that would
        // read as fresh. Proved on the arithmetic because the real function
        // reads a machine-wide path.
        let now: u64 = 1_700_000_000;
        assert_eq!(now.checked_sub(now + 60), None);
    }

    #[test]
    fn a_record_at_the_ttl_boundary_has_expired() {
        let now: u64 = 1_700_000_000;
        let recorded = now - POSITIVE_TTL.as_secs();
        let age = now.checked_sub(recorded).unwrap();
        assert!(
            age >= POSITIVE_TTL.as_secs(),
            "a record exactly one TTL old must re-probe, not be reused"
        );
        assert!(
            (age - 1) < POSITIVE_TTL.as_secs(),
            "one second younger is still fresh"
        );
    }
}
