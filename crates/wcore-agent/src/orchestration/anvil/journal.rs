//! Anvil climb journal — the append-only crash-recovery log (spec §6.5).
//!
//! Every paid step of a climb (the baseline probe, each candidate's gate run,
//! each promotion, the terminal state) is appended here BEFORE its effect is
//! trusted, so a crashed climb can resume from its journal: re-verify the pinned
//! gate/dependency digests, replay NOTHING that was already paid for (the
//! idempotency keys dedupe provider calls and gate executions), and always emit
//! an honest terminal receipt. There is no silent fourth exit (spec §6.5).
//!
//! The on-disk format is newline-delimited JSON with a deliberately plain schema
//! (strings + ints, not the in-memory climb types) so the journal is a stable,
//! forward-compatible record independent of internal refactors. Because a crash
//! can tear the final append mid-write, [`ClimbJournal::replay`] tolerates a
//! single unparseable *last* line (a torn write) but treats corruption anywhere
//! earlier as a hard error — a silently-dropped middle entry would lose paid work.
//!
//! This is the A1.5b persistence substrate; the engine loop that writes to it and
//! resumes from it lands with the builder/gate seams (A1.6). Spec:
//! `docs/design/2026-07-12-anvil-native-gated-forge-design.md` (v2) §6.5.

use std::collections::BTreeSet;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// What a journal entry records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JournalKind {
    /// The pre-climb baseline gate probe (spec §5).
    Probe,
    /// A candidate build was evaluated against the gate.
    Candidate,
    /// A candidate was promoted to the working best.
    Promote,
    /// The climb reached a terminal state (spec §6.5).
    Terminal,
}

/// One appended record. `seq` is assigned by the journal (monotonic from 0); the
/// `idempotency_key` identifies the paid operation so a resume never repeats it.
/// Optional fields are omitted from the JSON when empty to keep the log compact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JournalEntry {
    /// Monotonic sequence number, assigned on append.
    pub seq: u64,
    /// What this entry records.
    pub kind: JournalKind,
    /// Idempotency key of the paid operation (provider call / gate exec). On
    /// resume, an operation whose key is already present is NEVER re-run or
    /// re-charged (spec §6.5).
    pub idempotency_key: String,
    /// The `gate_closure_digest` (hex) in effect — re-checked for drift on resume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_closure_digest: Option<String>,
    /// Which candidate this concerns, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_id: Option<String>,
    /// The candidate's gate score (passing checks), if recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score: Option<u32>,
    /// The failing check ids at this step (for the receipt's coverage scope).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fail_ids: Vec<String>,
    /// Cumulative settled spend (microcents) at this step.
    pub spend_microcents: u64,
}

impl JournalEntry {
    /// A skeleton entry with `seq` unset (the journal assigns it on append). Use
    /// the `with_*` setters to attach the optional fields.
    #[must_use]
    pub fn new(
        kind: JournalKind,
        idempotency_key: impl Into<String>,
        spend_microcents: u64,
    ) -> Self {
        Self {
            seq: 0,
            kind,
            idempotency_key: idempotency_key.into(),
            gate_closure_digest: None,
            candidate_id: None,
            score: None,
            fail_ids: Vec::new(),
            spend_microcents,
        }
    }

    /// Attach the pinned gate closure digest (hex).
    #[must_use]
    pub fn with_gate_digest(mut self, digest_hex: impl Into<String>) -> Self {
        self.gate_closure_digest = Some(digest_hex.into());
        self
    }

    /// Attach the candidate id.
    #[must_use]
    pub fn with_candidate(mut self, candidate_id: impl Into<String>) -> Self {
        self.candidate_id = Some(candidate_id.into());
        self
    }

    /// Attach the candidate's score and failing check ids.
    #[must_use]
    pub fn with_result(mut self, score: u32, fail_ids: Vec<String>) -> Self {
        self.score = Some(score);
        self.fail_ids = fail_ids;
        self
    }
}

/// Errors from journal I/O.
#[derive(Debug, Error)]
pub enum JournalError {
    /// The journal file could not be opened, appended to, or read.
    #[error("climb journal I/O failed at {path}: {source}")]
    Io {
        /// The journal path.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A non-final journal line could not be parsed — the journal is corrupt and
    /// resuming from it would silently lose paid work, so it fails loud.
    #[error("climb journal {path} is corrupt at line {line}: {source}")]
    Corrupt {
        /// The journal path.
        path: PathBuf,
        /// 1-based line number of the unparseable entry.
        line: usize,
        /// The parse error.
        #[source]
        source: serde_json::Error,
    },
}

/// An append-only climb journal at a fixed path. Each [`append`](Self::append)
/// writes one JSON line and fsyncs it, so a crash loses at most the in-flight
/// line — never a line reported as written.
#[derive(Debug)]
pub struct ClimbJournal {
    path: PathBuf,
    next_seq: u64,
}

impl ClimbJournal {
    /// Open the journal at `path`, creating it if absent and resuming the
    /// sequence counter past any entries already present. A torn final line from
    /// a previous crash is tolerated (and will be overwritten by the next line);
    /// corruption earlier in the file is an error.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self, JournalError> {
        let path = path.into();
        let existing = Self::replay(&path)?;
        let next_seq = existing.last().map_or(0, |e| e.seq + 1);
        Ok(Self { path, next_seq })
    }

    /// The journal's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append `entry` (its `seq` is assigned here, monotonic), durably. Returns
    /// the assigned sequence number.
    pub fn append(&mut self, mut entry: JournalEntry) -> Result<u64, JournalError> {
        entry.seq = self.next_seq;
        // serde_json::to_string on a plain struct cannot fail; map defensively.
        let mut line = serde_json::to_string(&entry).map_err(|e| JournalError::Corrupt {
            path: self.path.clone(),
            line: self.next_seq as usize + 1,
            source: e,
        })?;
        line.push('\n');
        let mut file = OpenOptions::new()
            .read(true)
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|source| JournalError::Io {
                path: self.path.clone(),
                source,
            })?;
        // Heal a torn trailing line left by a prior crash (bytes after the last
        // newline) BEFORE appending, so this record starts on a clean line and
        // never concatenates onto a half-written one — which would corrupt the
        // torn line's replacement and lose this entry on the next replay.
        Self::truncate_torn_tail(&mut file).map_err(|source| JournalError::Io {
            path: self.path.clone(),
            source,
        })?;
        file.write_all(line.as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|source| JournalError::Io {
                path: self.path.clone(),
                source,
            })?;
        let seq = self.next_seq;
        self.next_seq += 1;
        Ok(seq)
    }

    /// Drop any bytes after the last newline — a torn record from a crash mid
    /// append. A file that already ends in a newline (or is empty) is untouched,
    /// so the common path pays only a one-byte read. Only when a torn tail is
    /// present is the file scanned back to the last complete line and truncated.
    fn truncate_torn_tail(file: &mut std::fs::File) -> std::io::Result<()> {
        let len = file.metadata()?.len();
        if len == 0 {
            return Ok(());
        }
        let mut last = [0u8; 1];
        file.seek(SeekFrom::End(-1))?;
        file.read_exact(&mut last)?;
        if last[0] == b'\n' {
            return Ok(()); // ends on a complete line — nothing torn.
        }
        // Torn tail: keep everything up to and including the last newline.
        let mut data = Vec::new();
        file.seek(SeekFrom::Start(0))?;
        file.read_to_end(&mut data)?;
        let keep = data
            .iter()
            .rposition(|&b| b == b'\n')
            .map_or(0, |i| i as u64 + 1);
        file.set_len(keep)?;
        Ok(())
    }

    /// Replay every entry in `path`, in order (crash recovery). A missing file is
    /// an empty journal. A single unparseable FINAL line is tolerated as a torn
    /// crash-time write and dropped; an unparseable earlier line is
    /// [`JournalError::Corrupt`].
    pub fn replay(path: impl AsRef<Path>) -> Result<Vec<JournalEntry>, JournalError> {
        let path = path.as_ref();
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(source) => {
                return Err(JournalError::Io {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        let raw_lines: Vec<String> = BufReader::new(file)
            .lines()
            .collect::<Result<_, _>>()
            .map_err(|source| JournalError::Io {
                path: path.to_path_buf(),
                source,
            })?;
        // Keep only non-blank lines, remembering their original line numbers so a
        // parse error reports the true position.
        let lines: Vec<(usize, &String)> = raw_lines
            .iter()
            .enumerate()
            .filter(|(_, l)| !l.trim().is_empty())
            .map(|(i, l)| (i + 1, l))
            .collect();
        let mut entries = Vec::with_capacity(lines.len());
        let last_idx = lines.len().saturating_sub(1);
        for (i, (line_no, line)) in lines.iter().enumerate() {
            match serde_json::from_str::<JournalEntry>(line) {
                Ok(entry) => entries.push(entry),
                // A torn write can only be the final line; anything earlier is
                // real corruption that must not be silently skipped.
                Err(_) if i == last_idx => break,
                Err(source) => {
                    return Err(JournalError::Corrupt {
                        path: path.to_path_buf(),
                        line: *line_no,
                        source,
                    });
                }
            }
        }
        Ok(entries)
    }

    /// The set of idempotency keys already recorded — the paid operations a
    /// resume must NOT repeat (spec §6.5 "replays nothing paid").
    pub fn recorded_keys(&self) -> Result<BTreeSet<String>, JournalError> {
        Ok(Self::replay(&self.path)?
            .into_iter()
            .map(|e| e.idempotency_key)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_journal() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("climb.journal");
        (dir, path)
    }

    #[test]
    fn append_assigns_monotonic_seq_and_replays_in_order() {
        let (_d, path) = tmp_journal();
        let mut j = ClimbJournal::open(&path).unwrap();
        let s0 = j
            .append(
                JournalEntry::new(JournalKind::Probe, "probe-1", 0).with_gate_digest("deadbeef"),
            )
            .unwrap();
        let s1 = j
            .append(
                JournalEntry::new(JournalKind::Candidate, "cand-1", 5000)
                    .with_candidate("c1")
                    .with_result(3, vec!["t_a".into()]),
            )
            .unwrap();
        assert_eq!((s0, s1), (0, 1));

        let replayed = ClimbJournal::replay(&path).unwrap();
        assert_eq!(replayed.len(), 2);
        assert_eq!(replayed[0].kind, JournalKind::Probe);
        assert_eq!(replayed[0].gate_closure_digest.as_deref(), Some("deadbeef"));
        assert_eq!(replayed[1].candidate_id.as_deref(), Some("c1"));
        assert_eq!(replayed[1].score, Some(3));
        assert_eq!(replayed[1].fail_ids, vec!["t_a".to_string()]);
        assert_eq!(replayed[1].spend_microcents, 5000);
    }

    #[test]
    fn reopening_resumes_the_sequence() {
        let (_d, path) = tmp_journal();
        {
            let mut j = ClimbJournal::open(&path).unwrap();
            j.append(JournalEntry::new(JournalKind::Probe, "p", 0))
                .unwrap();
            j.append(JournalEntry::new(JournalKind::Candidate, "c", 1))
                .unwrap();
        }
        // A fresh handle must continue past seq 1, not restart at 0.
        let mut j = ClimbJournal::open(&path).unwrap();
        let seq = j
            .append(JournalEntry::new(JournalKind::Terminal, "t", 2))
            .unwrap();
        assert_eq!(seq, 2);
        let all = ClimbJournal::replay(&path).unwrap();
        assert_eq!(all.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![0, 1, 2]);
    }

    #[test]
    fn recorded_keys_are_the_paid_operations() {
        let (_d, path) = tmp_journal();
        let mut j = ClimbJournal::open(&path).unwrap();
        j.append(JournalEntry::new(JournalKind::Candidate, "call-a", 10))
            .unwrap();
        j.append(JournalEntry::new(JournalKind::Candidate, "call-b", 20))
            .unwrap();
        let keys = j.recorded_keys().unwrap();
        assert!(keys.contains("call-a") && keys.contains("call-b"));
        assert!(!keys.contains("call-c"));
    }

    #[test]
    fn missing_journal_replays_empty() {
        let (_d, path) = tmp_journal();
        assert!(ClimbJournal::replay(&path).unwrap().is_empty());
        // open() on a missing file starts the sequence at 0.
        assert_eq!(ClimbJournal::open(&path).unwrap().next_seq, 0);
    }

    #[test]
    fn torn_final_line_is_tolerated() {
        let (_d, path) = tmp_journal();
        let mut j = ClimbJournal::open(&path).unwrap();
        j.append(JournalEntry::new(JournalKind::Probe, "p", 0))
            .unwrap();
        // Simulate a crash mid-append: a truncated JSON fragment as the last line.
        let mut f = OpenOptions::new().append(true).open(&path).unwrap();
        f.write_all(b"{\"seq\":1,\"kind\":\"candi").unwrap();
        drop(f);
        // Replay drops the torn tail and keeps the good entry.
        let entries = ClimbJournal::replay(&path).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].idempotency_key, "p");
        // And the journal can be reopened and continued: the next append heals
        // the torn tail before writing, so the resumed entry is a clean line and
        // BOTH entries survive a subsequent replay (the torn fragment is gone,
        // not concatenated onto the new record).
        let mut j2 = ClimbJournal::open(&path).unwrap();
        let seq = j2
            .append(JournalEntry::new(JournalKind::Candidate, "c", 1))
            .unwrap();
        assert_eq!(seq, 1);
        let after = ClimbJournal::replay(&path).unwrap();
        assert_eq!(after.len(), 2, "resumed entry must survive re-replay");
        assert_eq!(after[0].idempotency_key, "p");
        assert_eq!(after[1].idempotency_key, "c");
        assert_eq!(after.iter().map(|e| e.seq).collect::<Vec<_>>(), vec![0, 1]);
    }

    #[test]
    fn corruption_before_the_last_line_is_a_hard_error() {
        let (_d, path) = tmp_journal();
        {
            // A good line, a corrupt middle line, then a good final line.
            let mut f = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                .unwrap();
            let good =
                serde_json::to_string(&JournalEntry::new(JournalKind::Probe, "p", 0)).unwrap();
            writeln!(f, "{good}").unwrap();
            writeln!(f, "{{not json}}").unwrap();
            let good2 =
                serde_json::to_string(&JournalEntry::new(JournalKind::Terminal, "t", 0)).unwrap();
            writeln!(f, "{good2}").unwrap();
        }
        let err = ClimbJournal::replay(&path).unwrap_err();
        match err {
            JournalError::Corrupt { line, .. } => assert_eq!(line, 2),
            other => panic!("expected Corrupt at line 2, got {other:?}"),
        }
    }

    #[test]
    fn optional_fields_are_omitted_from_the_json() {
        // A bare entry serializes without the optional keys (compact log).
        let entry = JournalEntry::new(JournalKind::Terminal, "t", 0);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(!json.contains("candidate_id"));
        assert!(!json.contains("gate_closure_digest"));
        assert!(!json.contains("fail_ids"));
        assert!(!json.contains("score"));
    }
}
