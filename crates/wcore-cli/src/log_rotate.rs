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

/// An append-mode log file that rotates once it passes `max_bytes`.
pub struct RotatingLog {
    path: PathBuf,
    file: File,
    /// Bytes in the LIVE file. Seeded from its existing length so an append to
    /// an already-large log rotates promptly instead of allowing another full
    /// `max_bytes` on top of it.
    written: u64,
    max_bytes: u64,
}

impl RotatingLog {
    /// Open (creating parents) in append mode.
    pub fn open(path: PathBuf, max_bytes: u64) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = File::options().create(true).append(true).open(&path)?;
        let written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path,
            file,
            written,
            max_bytes,
        })
    }

    /// Copy the live log over the previous generation, then truncate it.
    ///
    /// Copy-and-truncate rather than rename-and-reopen because the live file is
    /// held open across the operation: on Windows renaming a file with an open
    /// handle fails, and renaming onto an existing destination fails too. This
    /// form needs no platform branch, and `set_len(0)` is correct for an append
    /// handle — the next write lands at the new end of file, which is 0.
    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        std::fs::copy(&self.path, rotated_path(&self.path))?;
        self.file.set_len(0)?;
        self.written = 0;
        Ok(())
    }
}

impl Write for RotatingLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        // `self.written > 0` keeps a single record larger than the whole bound
        // from looping: it is written to an empty file and overshoots once,
        // rather than rotating forever and never making progress.
        if self.written > 0 && self.written.saturating_add(buf.len() as u64) > self.max_bytes {
            self.rotate()?;
        }
        let n = self.file.write(buf)?;
        self.written = self.written.saturating_add(n as u64);
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
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
}
