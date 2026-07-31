//! Size-bounded rotation for `$WAYLAND_HOME/logs/wayland-core.log`.
//!
//! # Why this exists
//!
//! `lane/fix-tui-noise` moved engine diagnostics out of the terminal and into a
//! log file, which was right: a headless run previously kept **no record at
//! all**, so the diagnostics were lost the moment the terminal scrolled. What
//! it shipped without was a bound. The file is written on every run — roughly
//! 7 kB for one trivial turn — and a gateway host answering channel messages
//! runs headless continuously, so the file grew without limit on exactly the
//! deployment that can least afford it.
//!
//! Deleting the file is NOT the fix; that restores the problem the change
//! solved (a trace record existing nowhere). Bounding it is.
//!
//! # The bound
//!
//! At most two files are retained: the live log and one previous generation
//! (`wayland-core.log.1`). Each is capped at [`MAX_LOG_BYTES`], so the total on
//! disk is bounded by `2 × MAX_LOG_BYTES` plus the single record that crossed
//! the boundary.
//!
//! **Rotation keeps the NEWEST bytes.** When the live log fills it is copied
//! over the previous generation and truncated, so the oldest content is the
//! content that gets discarded. A rotation that retained the wrong end would be
//! worse than no rotation at all — the operator would be left holding the least
//! relevant window — so `rotation_discards_the_oldest_and_keeps_the_newest`
//! asserts both directions rather than just asserting that a rotation happened.
//!
//! # Rotation happens mid-process, not only at startup
//!
//! A rotate-on-open scheme would bound a fleet of short CLI runs and do nothing
//! at all for a long-lived gateway, which is the case that motivated the work.
//! The check therefore lives in [`Write::write`].

use std::fs::File;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// Bytes the live log may reach before it rotates.
///
/// 5 MiB is roughly 700 trivial headless runs, or several days of a chatty
/// gateway at INFO. Two generations bound the directory at 10 MiB.
pub const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Printed to stderr when the log file cannot be opened and the process falls
/// back to stderr-only diagnostics.
///
/// The fallback must be OBSERVABLE. "The run still exited 0" is satisfied just
/// as well by logging being dead, disabled, or never attempted — it evidences
/// the absence of a crash, not the presence of a degraded mode. This string is
/// the observable, and it is a `const` so the test asserts the same bytes the
/// product prints instead of a copy that can drift.
pub const LOG_FALLBACK_NOTICE: &str =
    "wayland-core: cannot open the diagnostics log; continuing with stderr-only diagnostics";

/// `wayland-core.log` → `wayland-core.log.1`.
fn rotated_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".1");
    PathBuf::from(s)
}

/// Read-only check that `path` could be opened for append later, returning the
/// bytes already in it.
///
/// Creates nothing — that is the whole point, see [`RotatingLog`]. It therefore
/// cannot see every failure the real open can: a `logs/` directory that exists
/// but is not writable is only discovered at the first record. It does see the
/// shape that a probe is actually for — something already occupying the path,
/// or the `logs/` path, that is not what the log needs it to be — which is the
/// condition `the_binary_survives_an_unopenable_log_dir` constructs.
fn probe(path: &Path) -> io::Result<u64> {
    match std::fs::metadata(path) {
        Ok(m) if m.is_dir() => {
            return Err(io::Error::other(format!(
                "{} is a directory, not a log file",
                path.display()
            )));
        }
        Ok(m) => return Ok(m.len()),
        // Unix reports ENOTDIR here when an ancestor is a plain file; Windows
        // reports a not-found. Both are resolved by the ancestor walk below,
        // which is why only NotFound falls through rather than every error.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    // The log does not exist yet, so the nearest ancestor that DOES exist must
    // be a directory or `create_dir_all` will fail at the first record.
    let mut ancestor = path.parent();
    while let Some(dir) = ancestor {
        if dir.as_os_str().is_empty() {
            // A relative path whose parent is the current directory.
            return Ok(0);
        }
        match std::fs::metadata(dir) {
            Ok(m) if m.is_dir() => return Ok(0),
            Ok(_) => {
                return Err(io::Error::other(format!(
                    "{} exists and is not a directory",
                    dir.display()
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => ancestor = dir.parent(),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other(format!(
        "no existing ancestor directory for {}",
        path.display()
    )))
}

/// An append-mode log file that rotates once it passes `max_bytes`.
///
/// # The file is created by the FIRST RECORD, not by `open`
///
/// [`RotatingLog::open`] binds a path and proves the location is usable. It
/// creates neither the `logs/` directory nor the file, so a run that emits no
/// records leaves NOTHING behind.
///
/// That is not tidiness. `$WAYLAND_HOME` is also the directory
/// `wayland-core backup restore --home …` operates on, and `main` opens the log
/// before dispatching the subcommand. Creating the file eagerly meant every
/// invocation of every offline subcommand — `models list`, `--config-path`,
/// `backup restore` — planted an empty `logs/wayland-core.log` in that home.
/// Measured on `backup restore` (issue #932), which emits no records at all:
///
/// * a REFUSED restore left the target no longer byte-identical, because the
///   process doing the refusing had already created `logs/` inside it —
///   `portability_hostile_corpus::hostile_refused_restore_leaves_an_occupied_target_byte_identical`;
/// * a restore into a genuinely EMPTY home was refused with "the target already
///   holds state", because [`crate::backup::dir_holds_state`] saw the `logs/`
///   directory this same process had created microseconds earlier. The
///   disaster-recovery path — restore your backup into your own home — was
///   unreachable, and the refusal told the operator to "choose an empty target"
///   when they already had.
///
/// Neither is a logging defect in the abstract. Both are the consequence of
/// writing before there was anything to write.
pub struct RotatingLog {
    path: PathBuf,
    /// `None` until the first record — see the type docs.
    file: Option<File>,
    /// Bytes in the LIVE file. Seeded from its existing length so an append to
    /// an already-large log rotates promptly instead of allowing another full
    /// `max_bytes` on top of it.
    written: u64,
    max_bytes: u64,
}

impl RotatingLog {
    /// Bind to `path`, creating nothing.
    ///
    /// An error means the location is already unusable and the caller prints
    /// [`LOG_FALLBACK_NOTICE`]. The check is read-only: a probe that opened the
    /// file to find out would be exactly the write this type exists to avoid.
    pub fn open(path: PathBuf, max_bytes: u64) -> io::Result<Self> {
        let written = probe(&path)?;
        Ok(Self {
            path,
            file: None,
            written,
            max_bytes,
        })
    }

    /// Materialize the directory and the file. Called by the first record.
    fn file_mut(&mut self) -> io::Result<&mut File> {
        if self.file.is_none() {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let file = File::options().create(true).append(true).open(&self.path)?;
            // Re-seed from the handle: another process may have appended
            // between the probe and here.
            self.written = file.metadata().map(|m| m.len()).unwrap_or(self.written);
            self.file = Some(file);
        }
        // Assigned immediately above whenever it was `None`, so this cannot be
        // `None` — expressed as an error rather than an `expect` because a
        // logging path must never be the thing that aborts the process.
        self.file
            .as_mut()
            .ok_or_else(|| io::Error::other("log file handle vanished immediately after open"))
    }

    /// Copy the live log over the previous generation, then truncate it.
    ///
    /// Copy-and-truncate rather than rename-and-reopen because the live file is
    /// held open across the operation: on Windows renaming a file with an open
    /// handle fails, and renaming onto an existing destination fails too. This
    /// form needs no platform branch, and `set_len(0)` is correct for an append
    /// handle — the next write lands at the new end of file, which is 0.
    fn rotate(&mut self) -> io::Result<()> {
        // Nothing has been written, so there is no live generation to retire.
        let Some(file) = self.file.as_mut() else {
            return Ok(());
        };
        file.flush()?;
        std::fs::copy(&self.path, rotated_path(&self.path))?;
        file.set_len(0)?;
        self.written = 0;
        Ok(())
    }
}

impl Write for RotatingLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // Materialize the file BEFORE the rotation decision. `written` may have
        // been seeded from a log an earlier process left behind, in which case
        // this process's very first record is the one that rotates — and
        // `rotate` needs a real handle to do it.
        self.file_mut()?;
        // `self.written > 0` keeps a single record larger than the whole bound
        // from looping: it is written to an empty file and overshoots once,
        // rather than rotating forever and never making progress.
        if self.written > 0 && self.written.saturating_add(buf.len() as u64) > self.max_bytes {
            self.rotate()?;
        }
        let n = self.file_mut()?.write(buf)?;
        self.written = self.written.saturating_add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.file.as_mut() {
            Some(file) => file.flush(),
            // No record was ever written. Creating the file in order to flush
            // an empty buffer is the defect, not the fix.
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write `n` numbered records through a writer bounded at `max_bytes`.
    /// Returns `(first_record, last_record, live_contents, rotated_contents)`.
    fn drive(max_bytes: u64, n: usize) -> (String, String, String, Option<String>) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("logs").join("wayland-core.log");
        let mut log = RotatingLog::open(path.clone(), max_bytes).unwrap();

        let record = |i: usize| format!("RECORD-{i:06}-payload-payload-payload\n");
        for i in 0..n {
            log.write_all(record(i).as_bytes()).unwrap();
        }
        log.flush().unwrap();

        let live = std::fs::read_to_string(&path).unwrap();
        let rotated = std::fs::read_to_string(rotated_path(&path)).ok();
        (record(0), record(n - 1), live, rotated)
    }

    /// BOTH directions of B2.1: a rotation happened, AND the bytes that
    /// survived it are the newest ones.
    #[test]
    fn rotation_discards_the_oldest_and_keeps_the_newest() {
        // 40 records of ~38 bytes ≈ 1520 bytes against a 300-byte bound, so
        // several rotations occur and the first record is several generations
        // behind the retention window.
        let (first, last, live, rotated) = drive(300, 40);

        // (i) rotation occurred.
        let rotated = rotated.expect(
            "no wayland-core.log.1 was produced, so nothing rotated — the bound did not bind",
        );

        // (ii) the retained bytes are the NEWEST. A rotation that kept the
        // wrong end would satisfy (i) and fail here, which is the entire
        // reason this is asserted separately.
        let retained = format!("{rotated}{live}");
        assert!(
            retained.contains(last.trim_end()),
            "the most recent record is not in the retained set; rotation kept the wrong end"
        );
        assert!(
            !retained.contains(first.trim_end()),
            "the OLDEST record survived {} bytes of rotation while newer records were \
             discarded — the retention window is inverted",
            retained.len()
        );

        // The bound actually bounds. Two generations, each capped, plus at most
        // one record that crossed the boundary.
        assert!(
            live.len() as u64 <= 300 + 64,
            "live log is {} bytes against a 300-byte bound",
            live.len()
        );
        assert!(
            retained.len() as u64 <= 2 * 300 + 64,
            "retained {} bytes against a 600-byte two-generation bound",
            retained.len()
        );
    }

    /// The negative control for the test above.
    ///
    /// Same driver, same records, a bound large enough that nothing rotates.
    /// Every assertion in `rotation_discards_the_oldest_and_keeps_the_newest`
    /// inverts here: no `.1` file, and the first record is still present. That
    /// is what makes the other test a measurement of rotation rather than of
    /// the harness.
    #[test]
    fn no_rotation_below_the_bound_and_nothing_is_discarded() {
        let (first, last, live, rotated) = drive(1024 * 1024, 40);

        assert!(
            rotated.is_none(),
            "a .1 file appeared without the bound being reached"
        );
        assert!(
            live.contains(first.trim_end()),
            "the first record was discarded below the rotation bound"
        );
        assert!(live.contains(last.trim_end()), "the last record is missing");
    }

    /// A single record larger than the whole bound must still be written, once.
    /// The guard against this is `self.written > 0`; without it the writer
    /// rotates on every attempt and never makes progress.
    #[test]
    fn an_oversized_single_record_is_written_rather_than_looping() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("logs").join("wayland-core.log");
        let mut log = RotatingLog::open(path.clone(), 64).unwrap();

        let big = "X".repeat(500);
        log.write_all(big.as_bytes()).unwrap();
        log.flush().unwrap();

        assert_eq!(std::fs::read_to_string(&path).unwrap(), big);
    }

    /// Reopening an existing log counts what is already there. Otherwise every
    /// process start would grant a fresh `max_bytes` on top of the current
    /// file, and a fleet of short runs would never rotate.
    #[test]
    fn an_existing_log_seeds_the_byte_count() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("logs").join("wayland-core.log");

        let mut first = RotatingLog::open(path.clone(), 300).unwrap();
        first.write_all(&b"A".repeat(280)).unwrap();
        first.flush().unwrap();
        drop(first);
        assert!(
            !rotated_path(&path).exists(),
            "nothing should have rotated yet"
        );

        let mut second = RotatingLog::open(path.clone(), 300).unwrap();
        second.write_all(&b"B".repeat(50)).unwrap();
        second.flush().unwrap();

        assert!(
            rotated_path(&path).exists(),
            "the reopened log did not count the 280 bytes already present, so it never rotated"
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "B".repeat(50));
    }

    /// BOTH directions of the lazy-creation contract (#932). Opening must leave
    /// the tree untouched, and the first record must still land where the
    /// operator is told to look — an `open` that created nothing but a `write`
    /// that also created nothing would satisfy half of this and be useless.
    #[test]
    fn open_creates_nothing_and_the_first_record_creates_everything() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("logs");
        let path = dir.join("wayland-core.log");

        let mut log = RotatingLog::open(path.clone(), MAX_LOG_BYTES).unwrap();
        assert!(
            !dir.exists(),
            "opening the log created {} — the home is the directory `backup restore` \
             is deciding about, and a run with nothing to say must leave it alone",
            dir.display()
        );
        assert!(!path.exists(), "opening the log created the file");
        // Flushing an empty writer is the other way an empty file appears.
        log.flush().unwrap();
        assert!(!path.exists(), "flushing an empty log created the file");

        log.write_all(b"RECORD\n").unwrap();
        log.flush().unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "RECORD\n",
            "the first record did not reach the documented path"
        );
    }

    /// The probe reports a location that can never work, without writing to it.
    /// A plain file where `logs/` belongs fails `create_dir_all` on every
    /// platform, and is what the binary-level degraded leg constructs.
    #[test]
    fn open_refuses_a_location_that_can_never_be_created() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("logs"), b"not a directory").unwrap();
        let path = tmp.path().join("logs").join("wayland-core.log");

        assert!(
            RotatingLog::open(path, MAX_LOG_BYTES).is_err(),
            "a log path blocked by a plain file was accepted, so the caller would \
             never print the degraded-mode notice"
        );
    }
}
