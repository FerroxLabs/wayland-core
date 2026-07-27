//! The bounded fire history.
//!
//! Phase 24 plan 24-02, Task 2.
//!
//! # The bound is ENFORCED, not documented
//!
//! `history.jsonl` was append-only with no cap. A job on a one-minute
//! schedule writes half a million records a year into a file the runtime
//! reads on every `cron history` invocation, and nothing ever removed one.
//! "Ring-buffered" appeared in the module documentation; the code appended.
//!
//! The cap here is applied on the WRITE path, so the file cannot exceed it
//! between reads. A cap applied only when reading leaves the file growing
//! forever and merely hides it from the operator, which is worse than an
//! unbounded file an operator can see.
//!
//! # Trimming keeps the TAIL
//!
//! The most recent records are the ones an operator is asking about. Trimming
//! from the front is what makes "the history verb still returns recent
//! records" true after the file stops growing.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;

use crate::job::CronFireRecord;

/// How many records the history file retains.
///
/// One thousand: enough that a minute-schedule job's last seventeen hours are
/// present, small enough that the whole file is read and rewritten in
/// milliseconds.
pub const DEFAULT_MAX_RECORDS: usize = 1000;

/// Append one record and enforce the cap.
///
/// Diagnostic-only, exactly as the previous unbounded append was: a write
/// failure is reported to the caller but the runner treats it as non-fatal,
/// because losing a history line must never abort a fire.
pub fn append_bounded(
    path: &Path,
    record: &CronFireRecord,
    max_records: usize,
) -> std::io::Result<()> {
    let max = max_records.max(1);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(record)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)?;
        writeln!(f, "{line}")?;
    }
    trim_to(path, max)
}

/// Rewrite `path` keeping at most the last `max` records.
///
/// A no-op when the file is already inside the bound, so the common case
/// costs one line count and no rewrite. The rewrite goes through a
/// same-directory temporary plus a rename, so a crash mid-trim leaves either
/// the old file or the new one and never a truncated one.
pub fn trim_to(path: &Path, max: usize) -> std::io::Result<()> {
    let max = max.max(1);
    let lines = read_lines(path)?;
    if lines.len() <= max {
        return Ok(());
    }
    let keep = &lines[lines.len() - max..];
    let tmp = path.with_extension(format!("jsonl.{}.tmp", std::process::id()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        for l in keep {
            writeln!(f, "{l}")?;
        }
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)
}

/// How many records the file currently holds.
pub fn count(path: &Path) -> std::io::Result<usize> {
    Ok(read_lines(path)?.len())
}

/// The most recent `n` records, oldest first.
///
/// An unparsable line is SKIPPED rather than fatal — a torn tail from a crash
/// mid-write must not make the whole history unreadable — but it is counted
/// and returned so the caller can report it instead of quietly showing fewer
/// records than exist.
pub fn read_recent(path: &Path, n: usize) -> std::io::Result<(Vec<CronFireRecord>, usize)> {
    let lines = read_lines(path)?;
    let start = lines.len().saturating_sub(n);
    let mut out = Vec::new();
    let mut skipped = 0usize;
    for l in &lines[start..] {
        match serde_json::from_str::<CronFireRecord>(l) {
            Ok(r) => out.push(r),
            Err(_) => skipped += 1,
        }
    }
    Ok((out, skipped))
}

fn read_lines(path: &Path) -> std::io::Result<Vec<String>> {
    match std::fs::File::open(path) {
        Ok(f) => {
            let mut out = Vec::new();
            for line in BufReader::new(f).lines() {
                let line = line?;
                if !line.trim().is_empty() {
                    out.push(line);
                }
            }
            Ok(out)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::CronFireOutcome;
    use chrono::Utc;

    fn rec(i: usize) -> CronFireRecord {
        CronFireRecord {
            job_id: format!("job-{i}"),
            fired_at: Utc::now(),
            outcome: CronFireOutcome::Success {
                duration_ms: i as u64,
            },
        }
    }

    #[test]
    fn the_file_stops_growing_at_the_bound() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("history.jsonl");
        for i in 0..250 {
            append_bounded(&p, &rec(i), 50).unwrap();
        }
        assert_eq!(
            count(&p).unwrap(),
            50,
            "sustained firing past the bound must not grow the file"
        );
    }

    #[test]
    fn trimming_keeps_the_most_recent_records() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("history.jsonl");
        for i in 0..120 {
            append_bounded(&p, &rec(i), 10).unwrap();
        }
        let (recent, skipped) = read_recent(&p, 10).unwrap();
        assert_eq!(skipped, 0);
        assert_eq!(recent.len(), 10);
        assert_eq!(
            recent.last().unwrap().job_id,
            "job-119",
            "the newest record must survive the trim"
        );
        assert_eq!(
            recent.first().unwrap().job_id,
            "job-110",
            "the retained window must be the tail, not the head"
        );
    }

    #[test]
    fn a_torn_line_is_skipped_and_counted_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("history.jsonl");
        append_bounded(&p, &rec(1), 100).unwrap();
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
            writeln!(f, "{{\"job_id\":\"torn\",\"fi").unwrap();
        }
        append_bounded(&p, &rec(2), 100).unwrap();
        let (recent, skipped) = read_recent(&p, 100).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(skipped, 1, "a torn line must be reported, not hidden");
    }

    #[test]
    fn a_missing_file_reads_as_empty_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("nope").join("history.jsonl");
        assert_eq!(count(&p).unwrap(), 0);
        assert_eq!(read_recent(&p, 10).unwrap().0.len(), 0);
    }

    #[test]
    fn a_zero_bound_is_treated_as_one_rather_than_erasing_history() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("history.jsonl");
        append_bounded(&p, &rec(1), 0).unwrap();
        append_bounded(&p, &rec(2), 0).unwrap();
        assert_eq!(
            count(&p).unwrap(),
            1,
            "a nonsense bound must degrade to the smallest useful one, not to nothing"
        );
    }
}
