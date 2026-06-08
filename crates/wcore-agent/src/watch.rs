//! W8b D.2 — `FileWatcher`: filesystem watcher for external-edit
//! detection, with an originated-by-self filter so engine writes
//! don't loop back into the agent's context.
//!
//! Built on `notify` (recommended platform-native watcher: inotify on
//! Linux, FSEvents on macOS, ReadDirectoryChangesW on Windows). Internal
//! callbacks publish events to a tokio `mpsc::Receiver`; the consumer
//! (orchestration loop) polls `next_external_event()` at turn boundaries.
//!
//! Self-origination: `Write` and `Edit` tools (D.4) call
//! `mark_self_originated(path)` immediately before writing. Any event
//! for the same path within `SELF_WRITE_WINDOW` after the mark is dropped on
//! the floor. The TTL prevents an engine write from masking a later
//! genuine external write.
//!
//! Wave RA — RELIABILITY MAJOR. The producer channel is **bounded** at
//! [`EVENT_CHANNEL_CAPACITY`]. `notify` callbacks run on a platform
//! thread (not a Tokio task) and must not block, so the inner publish
//! uses [`mpsc::Sender::try_send`] with drop-on-full semantics. File
//! events under heavy churn (build artifacts landing, large directory
//! syncs, IDE save-storms) can no longer drive engine memory growth.
//! A dropped event is recoverable — the consumer can rescan if needed —
//! while an OOM is not.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::mpsc::{self, Receiver, Sender};

/// How long a self-write mark is RETAINED (for GC only) after it's placed.
///
/// Bug #2(a)-timing: marks must survive until the agent loop next DRAINS the
/// watcher — which happens at a turn boundary, often many seconds after the
/// write. The old code pruned marks by a 1s wall-clock window measured at
/// drain time, so a mark placed before a 24s LLM turn was always gone by the
/// time its own filesystem event was evaluated → every self-write leaked back
/// as a phantom "the user edited N files". Retaining marks for minutes (GC
/// only) decouples mark lifetime from drain cadence; the actual match is
/// time-boxed against the EVENT's observation time via [`SELF_WRITE_WINDOW`].
pub const MARK_GC_TTL: Duration = Duration::from_secs(300);

/// A notify event counts as self-originated when its observation time
/// (`ExternalEvent::at`, stamped at notify-receive — NOT at drain) lands
/// within this window AFTER the mark. Covers the atomic-write temp+rename
/// fan-out plus platform FSEvents delivery latency.
pub const SELF_WRITE_WINDOW: Duration = Duration::from_secs(10);

/// Wave RA — bounded FileWatcher channel capacity. File events can burst
/// hard during IDE save-storms, build artifact churn, or large rsync
/// operations; 1024 keeps normal interactive usage well under the cap
/// while still capping unbounded memory growth from a producer that the
/// orchestration loop hasn't yet drained. Excess events are dropped via
/// `try_send` so the platform watcher thread never blocks.
pub const EVENT_CHANNEL_CAPACITY: usize = 1024;

#[derive(Debug, Error)]
pub enum WatchError {
    #[error("notify error: {0}")]
    Notify(#[from] notify::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Filesystem event reported back to the agent loop. Wraps the notify
/// payload with the `Instant` it was received so callers can fold
/// timestamps into synthetic messages.
#[derive(Debug, Clone)]
pub struct ExternalEvent {
    pub path: PathBuf,
    pub kind: EventKind,
    pub at: Instant,
}

/// Watcher handle. Cheap to clone (Arc internals); the underlying
/// notify watcher stays alive while any clone is held.
pub struct FileWatcher {
    // Watcher is kept alive as long as the handle lives. Stored boxed so
    // we don't surface notify's generic type param on the public API.
    _watcher: Arc<Mutex<RecommendedWatcher>>,
    /// Receiver side of the event channel. Wrapped in a Mutex<Option>
    /// so `next_external_event` can take `&self` (the orchestration
    /// loop holds an `Arc<FileWatcher>`).
    rx: Arc<Mutex<Receiver<ExternalEvent>>>,
    /// Paths that the engine just wrote to, plus the instant of the
    /// most-recent mark. Events for these paths within `SELF_WRITE_WINDOW` are
    /// filtered out as "self-originated".
    self_originated: Arc<Mutex<std::collections::HashMap<PathBuf, Instant>>>,
}

impl FileWatcher {
    /// Build a watcher rooted at `root`. Returns once the platform
    /// watcher is armed; events flow via internal channels until the
    /// last `FileWatcher` clone is dropped.
    pub fn new(root: &Path) -> Result<Self, WatchError> {
        let (tx, rx): (Sender<ExternalEvent>, _) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(ev) = res {
                    let at = Instant::now();
                    for p in ev.paths {
                        // F-007 (CRIT): hard-exclude wcore's own state
                        // directories from the "user edited files" signal.
                        // Session JSON, cron store, plans, and config files
                        // live under `.wayland-core/` and `.wayland/`; without
                        // this filter every session save injects a synthetic
                        // "User edited N files" message that poisons the
                        // model's context and wastes 200-400 tokens per turn.
                        if is_wcore_internal_path(&p) {
                            continue;
                        }
                        // Wave RA — `try_send` so the platform watcher
                        // thread (not a Tokio task) never blocks. Bursts
                        // beyond `EVENT_CHANNEL_CAPACITY` are dropped on
                        // the floor; the next consumer poll re-syncs.
                        let _ = tx.try_send(ExternalEvent {
                            path: p,
                            kind: ev.kind,
                            at,
                        });
                    }
                }
            })?;
        watcher.watch(root, RecursiveMode::Recursive)?;
        Ok(Self {
            _watcher: Arc::new(Mutex::new(watcher)),
            rx: Arc::new(Mutex::new(rx)),
            self_originated: Arc::new(Mutex::new(std::collections::HashMap::new())),
        })
    }

    /// Tag `path` as an engine-originated write. Subsequent events for
    /// this exact path within `SELF_WRITE_WINDOW` are swallowed.
    pub fn mark_self_originated(&self, path: &Path) {
        self.self_originated
            .lock()
            .insert(path.to_path_buf(), Instant::now());
    }

    /// Returns the next *external* event, awaiting up to `timeout` for
    /// it to arrive. `None` on timeout. Self-originated events within
    /// `SELF_WRITE_WINDOW` are silently dropped.
    pub async fn next_external_event(&self, timeout: Duration) -> Option<ExternalEvent> {
        let deadline = Instant::now() + timeout;
        loop {
            // Pull whatever's available right now.
            let next = {
                let mut rx = self.rx.lock();
                rx.try_recv().ok()
            };
            if let Some(ev) = next {
                if self.is_self_originated(&ev) {
                    continue;
                }
                return Some(ev);
            }
            // Nothing buffered — wait for the channel to wake us, or
            // bail at the deadline.
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }
            // Poll the channel via a short sleep then re-try. Avoids
            // holding the mutex across an await.
            tokio::time::sleep(remaining.min(Duration::from_millis(25))).await;
        }
    }

    /// Drain any buffered events without blocking. Returns events that
    /// were NOT filtered as self-originated, in arrival order.
    pub fn drain_external_events(&self) -> Vec<ExternalEvent> {
        let mut out = Vec::new();
        let mut rx = self.rx.lock();
        while let Ok(ev) = rx.try_recv() {
            if !self.is_self_originated(&ev) {
                out.push(ev);
            }
        }
        out
    }

    fn is_self_originated(&self, ev: &ExternalEvent) -> bool {
        let mut marks = self.self_originated.lock();
        // GC only — drop marks far older than any plausible write→event→drain
        // delay. Crucially this is NOT the match window: pruning by a 1s
        // wall-clock here (the old behaviour) deleted every mark before its
        // event was drained at the next turn boundary, so self-writes always
        // leaked. See MARK_GC_TTL.
        let gc_cutoff = Instant::now()
            .checked_sub(MARK_GC_TTL)
            .unwrap_or_else(Instant::now);
        marks.retain(|_, ts| *ts > gc_cutoff);

        // A mark suppresses an event for the same path when the write (mark)
        // came just before the event was OBSERVED (`ev.at`, stamped at
        // notify-receive — not at drain). Time-box against the event, not the
        // clock, so a late drain doesn't un-suppress a real self-write.
        let within_window = |mark_ts: &Instant| -> bool {
            match ev.at.checked_duration_since(*mark_ts) {
                // Event at/after the mark (the normal case: write then fs event).
                Some(delta) => delta <= SELF_WRITE_WINDOW,
                // Event marginally before the mark (clock granularity / rename
                // event ordering) — allow a small slack.
                None => mark_ts.duration_since(ev.at) <= Duration::from_millis(250),
            }
        };

        // Match canonical paths where possible — notify normalizes paths
        // (macOS resolves /var -> /private/var, etc.) so a literal key lookup
        // may miss. Compare both raw and canonical forms, each gated on the
        // self-write time window.
        if let Some(ts) = marks.get(&ev.path)
            && within_window(ts)
        {
            return true;
        }
        if let Ok(canon) = std::fs::canonicalize(&ev.path)
            && let Some(ts) = marks.get(&canon)
            && within_window(ts)
        {
            return true;
        }
        // Last shot: compare canonical of each (in-window) mark key to the
        // event path (handles the inverse mapping).
        for (key, ts) in marks.iter() {
            if !within_window(ts) {
                continue;
            }
            if key == &ev.path {
                return true;
            }
            if let Ok(canon_key) = std::fs::canonicalize(key)
                && canon_key == ev.path
            {
                return true;
            }
        }
        false
    }
}

/// F-007 (CRIT): return `true` when a path is inside wcore's own internal
/// state directories (`.wayland-core/` or `.wayland/`). These directories
/// hold session JSON, cron store, config, and plan files that the engine
/// writes constantly during normal operation. Surfacing them as "user edited"
/// events injects false context and wastes tokens every turn.
///
/// The check walks the path's ancestors and tests each component against the
/// known internal directory names. This handles both absolute paths (from
/// FSEvents on macOS, which resolves `/var` → `/private/var`) and relative
/// paths that a caller might synthesise in tests.
fn is_wcore_internal_path(path: &Path) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        s == ".wayland-core" || s == ".wayland"
    })
}

/// v0.9.1.1 F7: cap on the number of edited paths the synthetic
/// "User edited N files…" message will name verbatim before
/// collapsing the rest into "…and N more". A 683-path dump is no
/// signal — it's just noise that wastes context tokens AND, prior
/// to the F7 fix, leaked into the user transcript via `emit_info`.
const MAX_PATHS_IN_MESSAGE: usize = 20;

/// v0.9.1.1 F7: filter for the rustfmt scratch-file pattern
/// `<name>.tmp.<digits>.<hex>`. rustfmt writes a sibling temp file
/// per source it touches; without this filter a single `cargo fmt`
/// burst lights up the watcher with hundreds of paths that aren't
/// meaningful "user edits" at all.
///
/// Match shape: the file name has the form `<base>.tmp.<n>.<hex>`
/// where `<n>` is purely digits and `<hex>` is purely lowercase
/// hex. Tested independently below.
fn is_rustfmt_scratch(name: &str) -> bool {
    let Some(rest) = name.split_once(".tmp.") else {
        return false;
    };
    let after_tmp = rest.1;
    let Some((digits, hex)) = after_tmp.split_once('.') else {
        return false;
    };
    !digits.is_empty()
        && digits.chars().all(|c| c.is_ascii_digit())
        && !hex.is_empty()
        && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// v0.9.1.1 F7: filter to drop scratch / build / planning paths from
/// the "User edited N files while I was thinking" synthetic message
/// before it reaches the LLM (and, more importantly, before it
/// leaks into the user transcript).
///
/// Returns `true` when the path SHOULD be surfaced (i.e. it represents
/// a real user edit), `false` when it should be silently dropped.
///
/// Drops:
/// * rustfmt scratch files matching `*.tmp.<digits>.<hex>`
/// * `target/`, `node_modules/`, `dist/`, `build/` — build artifacts
/// * `.git/` — VCS metadata
/// * `.planning/`, `.blackboard/` — local agent workflow scratch
/// * `sessions/` — the engine's own per-session DB/JSON store (B#2(b))
/// * SQLite sidecars (`*.wal`, `*.shm`, `*-wal`, `*-shm`) + lock files
///   (`*.lock`, `index.lock`) — engine state, never hand-edited (B#2(b))
///
/// `.wayland-core/` and `.wayland/` are already filtered at the
/// notify-callback level (`is_wcore_internal_path` above) so we
/// don't repeat that check here. The `sessions/` + sidecar drops handle
/// the case where the engine's session dir resolves under the watch root
/// (a custom or test `WAYLAND_HOME` that isn't named `.wayland-core`), so
/// `save_session()`'s constant `*.wal`/`*.json`/`index.lock` writes don't
/// surface as phantom "the user edited N files" events.
fn path_should_surface_as_edit(path: &Path) -> bool {
    // Component-wise filter for directory-style excludes. `sessions`
    // covers the engine's per-session store ($WAYLAND_HOME/sessions/…)
    // when it falls under the watch root.
    for comp in path.components() {
        let s = comp.as_os_str().to_string_lossy();
        match s.as_ref() {
            "target" | "node_modules" | ".git" | "dist" | "build" | ".planning" | ".blackboard"
            | "sessions" => {
                return false;
            }
            _ => {}
        }
    }
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        // rustfmt scratch pattern: `foo.rs.tmp.87752.b02a96568587`.
        if is_rustfmt_scratch(name) {
            return false;
        }
        // Engine/SQLite state files no human edits — drop them wherever
        // they appear (the WAL/SHM sidecars + lock files are written on
        // every session save and would otherwise read as user edits).
        if name == "index.lock"
            || name.ends_with(".wal")
            || name.ends_with(".shm")
            || name.ends_with("-wal")
            || name.ends_with("-shm")
            || name.ends_with(".lock")
        {
            return false;
        }
        // Atomic-write scratch: the `tempfile` crate (used for session/state
        // saves) creates hidden `.tmpXXXXXX` siblings, then renames them into
        // place. The create event for the temp is never marked self-originated
        // (only the final path is), so it would otherwise leak as a user edit.
        if name.starts_with(".tmp") {
            return false;
        }
    }
    true
}

/// W8b D.3 — render the synthetic system message the agent loop will
/// inject at the next turn boundary when one or more external-edit
/// events have accumulated. The wording is deliberately direct: it
/// names every affected path and tells the agent to re-read before
/// acting.
///
/// Returns `None` when `events` is empty so callers can use
/// `if let Some(msg) = render_external_edit_message(&events)` to gate
/// the inject.
///
/// **v0.9.1.1 F7:** the input list is filtered (rustfmt scratch,
/// build dirs, `.git/`, `.planning/`, `.blackboard/`) before the
/// count + name list are emitted. The visible name list is also
/// capped at `MAX_PATHS_IN_MESSAGE` (20); the tail collapses to
/// "…and N more". Both changes prevent the cycle-1 BLOCKER where a
/// `cargo fmt` burst named 683 paths verbatim into the transcript.
pub fn render_external_edit_message(events: &[ExternalEvent]) -> Option<String> {
    if events.is_empty() {
        return None;
    }
    // Dedup paths in arrival order; multiple notify events may fire for
    // a single user-driven edit (write + chmod, atomic rename, etc.).
    let mut seen = std::collections::HashSet::new();
    let mut unique_paths: Vec<&Path> = Vec::with_capacity(events.len());
    for ev in events {
        if !path_should_surface_as_edit(&ev.path) {
            continue;
        }
        if seen.insert(ev.path.clone()) {
            unique_paths.push(&ev.path);
        }
    }
    // After filtering every path might be noise — caller must treat
    // `None` as "nothing real changed" so no injection happens.
    if unique_paths.is_empty() {
        return None;
    }
    let total = unique_paths.len();
    let visible = unique_paths.iter().take(MAX_PATHS_IN_MESSAGE);
    let mut joined = visible
        .map(|p| format!("`{}`", p.display()))
        .collect::<Vec<_>>()
        .join(", ");
    if total > MAX_PATHS_IN_MESSAGE {
        joined.push_str(&format!(", …and {} more", total - MAX_PATHS_IN_MESSAGE));
    }
    let msg = if total == 1 {
        format!(
            "User edited {} while I was thinking — re-read it before proceeding.",
            joined
        )
    } else {
        format!(
            "User edited {} files while I was thinking ({}) — re-read each before proceeding.",
            total, joined
        )
    };
    Some(msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_message_is_empty_when_no_events() {
        assert!(render_external_edit_message(&[]).is_none());
    }

    #[test]
    fn synthetic_message_names_single_path() {
        let ev = ExternalEvent {
            path: PathBuf::from("/proj/src/main.rs"),
            kind: notify::EventKind::Any,
            at: Instant::now(),
        };
        let msg = render_external_edit_message(&[ev]).unwrap();
        assert!(msg.contains("/proj/src/main.rs"));
        assert!(msg.contains("re-read"));
    }

    #[test]
    fn synthetic_message_dedups_repeat_paths() {
        let same = PathBuf::from("/proj/a.rs");
        let events: Vec<_> = (0..3)
            .map(|_| ExternalEvent {
                path: same.clone(),
                kind: notify::EventKind::Any,
                at: Instant::now(),
            })
            .collect();
        let msg = render_external_edit_message(&events).unwrap();
        // Two paths => uses singular phrasing.
        assert!(
            !msg.contains("files while I was thinking"),
            "single-path message must NOT be pluralised, got: {msg}"
        );
    }

    #[test]
    fn synthetic_message_lists_every_distinct_path() {
        let events = vec![
            ExternalEvent {
                path: PathBuf::from("/proj/a.rs"),
                kind: notify::EventKind::Any,
                at: Instant::now(),
            },
            ExternalEvent {
                path: PathBuf::from("/proj/b.rs"),
                kind: notify::EventKind::Any,
                at: Instant::now(),
            },
        ];
        let msg = render_external_edit_message(&events).unwrap();
        assert!(msg.contains("/proj/a.rs"));
        assert!(msg.contains("/proj/b.rs"));
        assert!(msg.contains("2 files"));
    }

    #[test]
    fn self_write_suppressed_even_when_drained_late_bug2a() {
        // Bug #2(a)-timing regression: a self-write mark must suppress its OWN
        // filesystem event even when the watcher drains long after the write
        // (the agent thought for a full LLM turn in between). The match is
        // time-boxed against the EVENT's observation time (`ev.at`), not
        // wall-clock-at-drain — the old DEBOUNCE-at-drain pruning deleted the
        // mark before its event was ever evaluated, leaking every self-write.
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let fw = FileWatcher::new(dir.path()).unwrap();
        let file = dir.path().join("out.txt");
        fs::write(&file, b"x").unwrap(); // must exist for canonicalize()

        fw.mark_self_originated(&file);
        let mark_at = Instant::now();

        // Event observed just after the write but only DRAINED now (i.e. far
        // past a 1s wall-clock window) — must still be recognised as self.
        let ev_recent = ExternalEvent {
            path: file.clone(),
            kind: notify::EventKind::Any,
            at: mark_at + Duration::from_millis(500),
        };
        assert!(
            fw.is_self_originated(&ev_recent),
            "a self-write event within the window must be suppressed regardless \
             of how late the watcher drains"
        );

        // An edit observed well AFTER the self-write window is a genuine
        // external edit and must surface.
        let ev_late = ExternalEvent {
            path: file.clone(),
            kind: notify::EventKind::Any,
            at: mark_at + SELF_WRITE_WINDOW + Duration::from_secs(5),
        };
        assert!(
            !fw.is_self_originated(&ev_late),
            "an edit long after the self-write window must surface as external"
        );
    }

    // ── v0.9.1.1 F7 — pre-turn edit-detector dump filter ──────────────
    // Sean direct screenshot 2026-05-27: `cargo fmt` lit up the watcher
    // with 683 paths (rustfmt scratch files dominant) and the synthetic
    // "User edited N files…" message dumped every one verbatim into
    // the transcript. The fix filters scratch + build + planning paths
    // and caps the visible name list at 20.

    fn ev(p: &str) -> ExternalEvent {
        ExternalEvent {
            path: PathBuf::from(p),
            kind: notify::EventKind::Any,
            at: Instant::now(),
        }
    }

    #[test]
    fn is_rustfmt_scratch_matches_real_pattern_v0911() {
        // The exact shape `cargo fmt` produced on Sean's drive — a
        // sibling tempfile next to each source rustfmt is rewriting.
        assert!(is_rustfmt_scratch(
            "2026-05-27-v0.9.1.1-findings-f6-markdown-headers.md.tmp.87752.b02a96568587"
        ));
        assert!(is_rustfmt_scratch("workspace.rs.tmp.87752.73cdf092b118"));
        // Hex with mixed case is hex.
        assert!(is_rustfmt_scratch("foo.tmp.1.aB"));
        // Negative cases.
        assert!(!is_rustfmt_scratch("foo.rs"));
        assert!(!is_rustfmt_scratch("foo.tmp.rs")); // no digits-then-hex tail
        assert!(!is_rustfmt_scratch("foo.tmp..aabb")); // empty digits
        assert!(!is_rustfmt_scratch("foo.tmp.12345.")); // empty hex
        assert!(!is_rustfmt_scratch("foo.tmp.123z.abc")); // non-digit in N
        assert!(!is_rustfmt_scratch("foo.tmp.123.xyz")); // non-hex in trailer
        assert!(!is_rustfmt_scratch(""));
    }

    #[test]
    fn external_edit_message_filters_rustfmt_scratch_v0911() {
        // Pure-noise burst: every event is a rustfmt scratch file.
        // After filtering there's nothing real to surface, so the
        // synthetic message must collapse to None and no injection
        // happens.
        let events = vec![
            ev("/proj/src/lib.rs.tmp.87752.b02a96568587"),
            ev("/proj/src/main.rs.tmp.87752.73cdf092b118"),
            ev("/proj/src/util.rs.tmp.87753.abcd1234ef56"),
        ];
        assert!(render_external_edit_message(&events).is_none());
    }

    #[test]
    fn external_edit_message_filters_build_and_planning_paths_v0911() {
        // Real-shape mixed list: 1 source edit + a pile of build /
        // VCS / planning noise. The message should name only the
        // real edit and report a count of 1.
        let events = vec![
            ev("/proj/src/lib.rs"),
            ev("/proj/target/debug/build/whatever/out.rs"),
            ev("/proj/.git/objects/ab/cd1234"),
            ev("/proj/.planning/audits/2026-05-27-x.md"),
            ev("/proj/.blackboard/HANDOFF.md"),
            ev("/proj/node_modules/foo/index.js"),
            ev("/proj/dist/bundle.js"),
            ev("/proj/build/Release/x.o"),
        ];
        let msg = render_external_edit_message(&events).unwrap();
        assert!(msg.contains("/proj/src/lib.rs"));
        assert!(!msg.contains("/target/"), "target/ leaked into msg: {msg}");
        assert!(!msg.contains("/.git/"), ".git/ leaked into msg: {msg}");
        assert!(
            !msg.contains("/.planning/"),
            ".planning/ leaked into msg: {msg}"
        );
        assert!(
            !msg.contains("/.blackboard/"),
            ".blackboard/ leaked into msg: {msg}"
        );
        assert!(
            !msg.contains("/node_modules/"),
            "node_modules/ leaked into msg: {msg}"
        );
        assert!(!msg.contains("/dist/"), "dist/ leaked into msg: {msg}");
        assert!(!msg.contains("/build/"), "build/ leaked into msg: {msg}");
        // Single survivor → singular phrasing.
        assert!(
            !msg.contains("files while I was thinking"),
            "single survivor must use singular phrasing, got: {msg}"
        );
    }

    #[test]
    fn external_edit_message_filters_engine_session_state_bug2b() {
        // Bug #2(b): when the engine's session dir resolves under the watch
        // root (a custom/test WAYLAND_HOME not named `.wayland-core`), every
        // session save writes `sessions/*.wal|*.json|index.lock`. None of those
        // are user edits — only the real source edit may surface.
        let events = vec![
            ev("/proj/src/lib.rs"),
            ev("/home/sessions/2026-06-05_abc.json"),
            ev("/home/sessions/2026-06-05_abc.db"),
            ev("/home/sessions/2026-06-05_abc.db-wal"),
            ev("/home/memory/memory.db-wal"),
            ev("/home/memory/memory.db-shm"),
            ev("/proj/.wayland-core/index.lock"),
            ev("/proj/some/dir/foo.lock"),
            ev("/proj/data.wal"),
            ev("/proj/cache.shm"),
        ];
        let msg = render_external_edit_message(&events).unwrap();
        assert!(
            msg.contains("/proj/src/lib.rs"),
            "the real source edit must survive: {msg}"
        );
        assert!(!msg.contains("/sessions/"), "sessions/ leaked: {msg}");
        assert!(!msg.contains(".wal"), "WAL sidecar leaked: {msg}");
        assert!(!msg.contains(".shm"), "SHM sidecar leaked: {msg}");
        assert!(!msg.contains(".lock"), "lock file leaked: {msg}");
        // Exactly one survivor → singular phrasing, not "N files".
        assert!(
            !msg.contains("files while I was thinking"),
            "single survivor must use singular phrasing, got: {msg}"
        );
    }

    #[test]
    fn external_edit_message_caps_at_max_paths_v0911() {
        // 30 real edits → message names 20 of them + collapses the
        // rest into "…and 10 more". Without the cap a `git rebase`
        // touching dozens of source files would pour 30 verbatim
        // paths into the model's user-turn message.
        let events: Vec<ExternalEvent> = (0..30)
            .map(|i| ev(&format!("/proj/src/file_{i:02}.rs")))
            .collect();
        let msg = render_external_edit_message(&events).unwrap();
        assert!(msg.contains("/proj/src/file_00.rs"));
        assert!(msg.contains("/proj/src/file_19.rs"));
        assert!(
            !msg.contains("/proj/src/file_25.rs"),
            "path past the visible cap should be collapsed, got: {msg}"
        );
        assert!(
            msg.contains("…and 10 more"),
            "tail collapse missing in: {msg}"
        );
        // Count is still the true filtered total.
        assert!(msg.contains("30 files"));
    }
}
