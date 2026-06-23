//! T3-3.2.3 — Cross-agent file state coordination registry.
//!
//! Ported from the prior Wayland Python engine. Prevents
//! mangled edits when concurrent subagents (same process, same
//! filesystem) touch the same file. Complements the single-agent
//! [`crate::file_cache::FileStateCache`] — that module is a per-process
//! LRU content cache keyed by resolved path, while this module is a
//! per-process *coordinator* that tracks WHO read/wrote a path and WHEN,
//! and exposes per-path locks for read→modify→write critical sections.
//!
//! ## Design
//!
//! A process-wide singleton [`FileStateRegistry`] tracks, per resolved
//! path:
//!
//! * Per-agent read stamps: `{task_id: {path: (mtime_ms, read_ts_ms, partial)}}`
//! * Last writer globally: `{path: (task_id, write_ts_ms)}`
//! * Per-path [`std::sync::Mutex`] for read→modify→write sections.
//!
//! Public hooks (all no-ops when the env var
//! `WAYLAND_DISABLE_FILE_STATE_GUARD=1` is set):
//!
//! * [`record_read`] — call after every read.
//! * [`note_write`] — call after every successful write/patch.
//! * [`check_stale`] — call BEFORE every write/patch; returns a model-
//!   facing warning string when the write would clobber another agent's
//!   work, otherwise `None`.
//! * [`lock_path`] — RAII guard that serializes other lockers of the
//!   same path (different paths proceed in parallel).
//! * [`writes_since`] — used by the delegate-completion reminder to
//!   surface sibling writes to the parent agent.
//! * [`known_reads`] — list of paths an agent has read.
//!
//! ## Divergence from Python source
//!
//! * Time stamps are integer milliseconds since `UNIX_EPOCH` (`u128`),
//!   not floating-point seconds — avoids monotonic-vs-wall confusion
//!   and `f64` rounding when comparing closely-spaced events. The
//!   semantic (an opaque ordered timestamp) is unchanged.
//! * The Python source uses `collections.defaultdict` insertion order
//!   for the LRU-style cap. We use [`indexmap`]-free equivalent: a
//!   plain [`HashMap`] paired with a `VecDeque` of insertion order so
//!   the oldest entry is dropped on overflow. Behavior matches.
//! * `lock_path` is an RAII guard (`LockGuard`) instead of a context
//!   manager — same semantics: acquire on construction, release on
//!   drop.
//! * `clear()` is gated on `#[cfg(test)]` — production code should
//!   never reset the registry.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// R2 fix A4: emit a single `tracing::warn` per process when any
/// `file_state` mutex is recovered from poison (a prior thread panicked
/// while holding the lock). We tolerate the poison — the registry's
/// internal invariants survive a panic at the mutated sites — but we
/// surface the event so operators investigating odd behavior see it.
///
/// Was: 8 sites silently swallowed via `unwrap_or_else(|e| e.into_inner())`
/// plus 2 sites that *panicked* via `.expect("file_state lock-map poisoned")`.
/// Now unified.
///
/// R3-B3: process-wide warn-once gate intentionally NOT reset between
/// tests. The warn-once semantics are designed for production
/// observability where a single warning per process lifetime is
/// sufficient signal. Tests that exercise the poison path must either:
///   (a) check the AtomicBool's current state BEFORE inducing poison, or
///   (b) accept that only the FIRST poison test in the binary will see
///       the warn fire (and assert on that one).
///
/// A `#[cfg(test)] fn reset_poison_warned()` is intentionally NOT exposed
/// to avoid masking real poison events during the test pass (which would
/// happen if a test reset the gate after observing it, allowing a later
/// genuine bug to appear as a clean run).
static POISON_WARNED: AtomicBool = AtomicBool::new(false);

fn poisoned_warn_once() {
    if !POISON_WARNED.swap(true, Ordering::Relaxed) {
        tracing::warn!(
            poisoned = true,
            "file_state mutex recovered from poison — a thread previously panicked while holding this lock; subsequent recoveries silenced"
        );
    }
}

/// Bounded cap on per-agent read entries. Prevents long sessions from
/// accumulating unbounded state. On overflow, the oldest entry by
/// insertion order is dropped.
const MAX_PATHS_PER_AGENT: usize = 4096;

/// Bounded cap on the global last-writer map. Same policy.
const MAX_GLOBAL_WRITERS: usize = 4096;

/// Per-path read record kept for a single agent.
///
/// `partial = true` when the corresponding read used offset/limit
/// pagination — subsequent writes should still warn so the model
/// re-reads the file in full.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadStamp {
    pub mtime_ms: u64,
    pub read_ts_ms: u128,
    pub partial: bool,
}

/// Per-path last-writer record.
#[derive(Debug, Clone)]
struct WriterStamp {
    task_id: String,
    write_ts_ms: u128,
}

/// Insertion-ordered map used to back per-agent and global maps so the
/// oldest entry can be evicted on overflow.
#[derive(Debug, Default)]
struct OrderedMap<V> {
    map: HashMap<String, V>,
    order: VecDeque<String>,
}

impl<V> OrderedMap<V> {
    fn new() -> Self {
        Self {
            map: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// Insert or update an entry. Preserves first-seen insertion order
    /// (matches Python `dict` semantics used in the source).
    fn insert(&mut self, key: String, value: V) {
        if !self.map.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.map.insert(key, value);
    }

    fn get(&self, key: &str) -> Option<&V> {
        self.map.get(key)
    }

    fn cap(&mut self, limit: usize) {
        while self.map.len() > limit {
            // SAFETY: len() > limit > 0 guarantees order is non-empty
            // and order/map are kept in sync by `insert`.
            let oldest = match self.order.pop_front() {
                Some(k) => k,
                None => break,
            };
            self.map.remove(&oldest);
        }
    }

    fn keys(&self) -> impl Iterator<Item = &String> {
        self.order.iter()
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

/// Process-wide coordinator for cross-agent file edits.
#[derive(Debug)]
pub struct FileStateRegistry {
    state: Mutex<RegistryState>,
    path_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

#[derive(Debug)]
struct RegistryState {
    reads: HashMap<String, OrderedMap<ReadStamp>>,
    last_writer: OrderedMap<WriterStamp>,
}

impl RegistryState {
    fn new() -> Self {
        Self {
            reads: HashMap::new(),
            last_writer: OrderedMap::new(),
        }
    }
}

impl FileStateRegistry {
    fn new() -> Self {
        Self {
            state: Mutex::new(RegistryState::new()),
            path_locks: Mutex::new(HashMap::new()),
        }
    }

    /// Record that `task_id` read the file at `resolved`.
    ///
    /// `partial = true` if the read used offset/limit pagination. If
    /// `mtime_ms_override` is `None`, the on-disk mtime is sampled; if
    /// the file is unreadable, the call is silently dropped.
    pub fn record_read(
        &self,
        task_id: &str,
        resolved: &str,
        partial: bool,
        mtime_ms_override: Option<u64>,
    ) {
        if is_disabled() {
            return;
        }
        let mtime_ms = match mtime_ms_override.or_else(|| file_mtime_ms(Path::new(resolved))) {
            Some(m) => m,
            None => return,
        };
        let now = now_ms();
        let mut state = self.state.lock().unwrap_or_else(|e| {
            poisoned_warn_once();
            e.into_inner()
        });
        let entry = state
            .reads
            .entry(task_id.to_string())
            .or_insert_with(OrderedMap::new);
        entry.insert(
            resolved.to_string(),
            ReadStamp {
                mtime_ms,
                read_ts_ms: now,
                partial,
            },
        );
        entry.cap(MAX_PATHS_PER_AGENT);
    }

    /// Record a successful write.
    ///
    /// Updates the global last-writer map AND this agent's own read
    /// stamp (a write is an implicit read — the agent now knows the
    /// current content).
    pub fn note_write(&self, task_id: &str, resolved: &str, mtime_ms_override: Option<u64>) {
        if is_disabled() {
            return;
        }
        let mtime_ms = match mtime_ms_override.or_else(|| file_mtime_ms(Path::new(resolved))) {
            Some(m) => m,
            None => return,
        };
        let now = now_ms();
        let mut state = self.state.lock().unwrap_or_else(|e| {
            poisoned_warn_once();
            e.into_inner()
        });
        state.last_writer.insert(
            resolved.to_string(),
            WriterStamp {
                task_id: task_id.to_string(),
                write_ts_ms: now,
            },
        );
        state.last_writer.cap(MAX_GLOBAL_WRITERS);

        let entry = state
            .reads
            .entry(task_id.to_string())
            .or_insert_with(OrderedMap::new);
        entry.insert(
            resolved.to_string(),
            ReadStamp {
                mtime_ms,
                read_ts_ms: now,
                partial: false,
            },
        );
        entry.cap(MAX_PATHS_PER_AGENT);
    }

    /// Return a model-facing warning when the next write would be
    /// stale, or `None` when the write is safe.
    ///
    /// Three staleness classes, in order of severity:
    ///
    /// 1. Sibling subagent wrote this file after this agent's last read.
    /// 2. External / unknown change (on-disk mtime differs from our
    ///    last read).
    /// 3. Agent never read the file (write-without-read).
    pub fn check_stale(&self, task_id: &str, resolved: &str) -> Option<String> {
        if is_disabled() {
            return None;
        }
        let (stamp, last_writer): (Option<ReadStamp>, Option<WriterStamp>) = {
            let state = self.state.lock().unwrap_or_else(|e| {
                poisoned_warn_once();
                e.into_inner()
            });
            let s = state
                .reads
                .get(task_id)
                .and_then(|m| m.get(resolved))
                .copied();
            let w = state.last_writer.get(resolved).cloned();
            (s, w)
        };

        // Case 3a: never read AND no writer recorded → net-new file or
        // first touch. Existing path-validation handles permission;
        // nothing to warn about here.
        if stamp.is_none() && last_writer.is_none() {
            return None;
        }

        // If the file doesn't exist on disk, the write will create it;
        // not stale.
        let current_mtime = file_mtime_ms(Path::new(resolved))?;

        // Case 1: sibling subagent modified after our last read.
        if let Some(ref w) = last_writer
            && w.task_id != task_id
        {
            match stamp {
                None => {
                    return Some(format!(
                        "{resolved} was modified by sibling subagent {writer:?} \
                         but this agent never read it. Read the file before \
                         writing to avoid overwriting the sibling's changes.",
                        writer = w.task_id
                    ));
                }
                Some(s) if w.write_ts_ms > s.read_ts_ms => {
                    return Some(format!(
                        "{resolved} was modified by sibling subagent {writer:?} \
                         after this agent's last read. Re-read the file before \
                         writing.",
                        writer = w.task_id
                    ));
                }
                _ => {}
            }
        }

        // Case 2: external / unknown modification (mtime drift).
        if let Some(s) = stamp {
            if current_mtime != s.mtime_ms {
                return Some(format!(
                    "{resolved} was modified since you last read it on disk \
                     (external edit or unrecorded writer). Re-read the file \
                     before writing."
                ));
            }
            if s.partial {
                return Some(format!(
                    "{resolved} was last read with offset/limit pagination \
                     (partial view). Re-read the whole file before \
                     overwriting it."
                ));
            }
            return None;
        }

        // Case 3b: agent truly never read the file.
        Some(format!(
            "{resolved} was not read by this agent. Read the file first so \
             you can write an informed edit."
        ))
    }

    /// Return `{writer_task_id -> [paths]}` for writes done after
    /// `since_ts_ms` by agents OTHER than `exclude_task_id`, restricted
    /// to `paths`.
    ///
    /// Used by the delegate-completion reminder to surface sibling
    /// writes to the parent agent.
    pub fn writes_since<I, S>(
        &self,
        exclude_task_id: &str,
        since_ts_ms: u128,
        paths: I,
    ) -> HashMap<String, Vec<String>>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        if is_disabled() {
            return HashMap::new();
        }
        let paths_set: std::collections::HashSet<String> =
            paths.into_iter().map(|p| p.as_ref().to_string()).collect();
        let mut out: HashMap<String, Vec<String>> = HashMap::new();
        let state = self.state.lock().unwrap_or_else(|e| {
            poisoned_warn_once();
            e.into_inner()
        });
        for p in state.last_writer.keys() {
            let w = match state.last_writer.get(p) {
                Some(w) => w,
                None => continue,
            };
            if w.task_id == exclude_task_id {
                continue;
            }
            if w.write_ts_ms < since_ts_ms {
                continue;
            }
            if !paths_set.contains(p) {
                continue;
            }
            out.entry(w.task_id.clone()).or_default().push(p.clone());
        }
        out
    }

    /// Return the list of resolved paths this agent has read.
    pub fn known_reads(&self, task_id: &str) -> Vec<String> {
        if is_disabled() {
            return Vec::new();
        }
        let state = self.state.lock().unwrap_or_else(|e| {
            poisoned_warn_once();
            e.into_inner()
        });
        state
            .reads
            .get(task_id)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default()
    }

    /// Acquire the per-path lock. Same process, same filesystem —
    /// threads on the same path serialize; different paths proceed in
    /// parallel.
    ///
    /// Returns an RAII guard; the lock is released on drop.
    pub fn lock_path(&self, resolved: &str) -> PathLockGuard {
        let arc = {
            let mut locks = self.path_locks.lock().unwrap_or_else(|e| {
                poisoned_warn_once();
                e.into_inner()
            });
            locks
                .entry(resolved.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        // Convert to a 'static guard via the Arc — the guard holds its
        // own clone of the Arc so the Mutex outlives the borrow.
        PathLockGuard::new(arc)
    }

    /// Reset all state. Tests only.
    #[cfg(test)]
    pub fn clear(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| {
            poisoned_warn_once();
            e.into_inner()
        });
        state.reads.clear();
        state.last_writer.clear();
        drop(state);
        let mut locks = self.path_locks.lock().unwrap_or_else(|e| {
            poisoned_warn_once();
            e.into_inner()
        });
        locks.clear();
    }
}

/// Widen a [`MutexGuard`]'s lifetime to `'static`.
///
/// This is a *lifetime-only* extension: the `Output = MutexGuard<'static, ()>`
/// associated-type bound below guarantees the function can only ever change the
/// lifetime parameter — never the underlying type. That is strictly narrower
/// than a bare `std::mem::transmute`, which would silently accept a wrong-typed
/// or wrong-sized value if the surrounding code were ever edited.
///
/// # Safety
/// The caller MUST keep the `Arc<Mutex<()>>` that produced `guard` alive for at
/// least as long as the returned guard. [`PathLockGuard`] upholds this by
/// storing that very `Arc` and dropping it *after* the guard (see the `Drop`
/// impl). Violating it is a use-after-free.
unsafe fn extend_guard_lifetime(guard: MutexGuard<'_, ()>) -> MutexGuard<'static, ()> {
    // Bound the transmute to a lifetime change only: both sides are
    // `MutexGuard<'_, ()>`, identical layout, so this cannot be repurposed
    // into an arbitrary type pun by a future edit.
    trait WidenLifetime {
        type Output;
    }
    impl<'a> WidenLifetime for MutexGuard<'a, ()> {
        type Output = MutexGuard<'static, ()>;
    }
    unsafe {
        std::mem::transmute::<MutexGuard<'_, ()>, <MutexGuard<'_, ()> as WidenLifetime>::Output>(
            guard,
        )
    }
}

/// RAII guard returned by [`FileStateRegistry::lock_path`]. The lock is
/// released when the guard is dropped.
///
/// # Self-referential soundness invariant — DO NOT REORDER FIELDS
///
/// `_guard` borrows from the `Mutex` owned by `_arc`. The `'static` lifetime on
/// `_guard` is a lie that is only safe because:
///   1. `_arc` keeps that `Mutex` alive for the whole life of this struct, and
///   2. on drop, `_guard` is released *before* `_arc` is dropped.
///
/// Both fields are [`ManuallyDrop`] so the struct's auto-drop does nothing, and
/// the explicit `Drop` impl below performs the release in the correct order.
/// Reordering these fields, removing `ManuallyDrop`, or dropping `_arc` first
/// reintroduces a use-after-free. The compile-time check in `new` pins the
/// types so a field-type swap won't silently compile.
pub struct PathLockGuard {
    _guard: std::mem::ManuallyDrop<MutexGuard<'static, ()>>,
    _arc: std::mem::ManuallyDrop<Arc<Mutex<()>>>,
}

impl PathLockGuard {
    fn new(arc: Arc<Mutex<()>>) -> Self {
        let guard: MutexGuard<'_, ()> = arc.lock().unwrap_or_else(|e| {
            poisoned_warn_once();
            e.into_inner()
        });
        // SAFETY: `arc` is moved into `self._arc` below and, per the `Drop`
        // impl + field-order invariant documented on this struct, outlives
        // `self._guard`. The lifetime extension is type-constrained (see
        // `extend_guard_lifetime`), so it cannot become a type pun.
        let guard: MutexGuard<'static, ()> = unsafe { extend_guard_lifetime(guard) };
        Self {
            _guard: std::mem::ManuallyDrop::new(guard),
            _arc: std::mem::ManuallyDrop::new(arc),
        }
    }
}

impl Drop for PathLockGuard {
    fn drop(&mut self) {
        // Drop the guard FIRST (releases the mutex), then the Arc.
        // SAFETY: ManuallyDrop fields are only dropped here, exactly
        // once per guard. Order matches the invariant documented on the
        // struct: releasing the borrow before freeing what it borrows.
        unsafe {
            std::mem::ManuallyDrop::drop(&mut self._guard);
            std::mem::ManuallyDrop::drop(&mut self._arc);
        }
    }
}

// ── Module-level singleton + helpers ─────────────────────────────────

/// Process-wide singleton. Lazy-initialised on first use.
pub fn registry() -> &'static FileStateRegistry {
    static REG: OnceLock<FileStateRegistry> = OnceLock::new();
    REG.get_or_init(FileStateRegistry::new)
}

/// Read each call so tests can toggle via `std::env::set_var`.
fn is_disabled() -> bool {
    std::env::var("WAYLAND_DISABLE_FILE_STATE_GUARD")
        .map(|v| v.trim() == "1")
        .unwrap_or(false)
}

/// Current wall-clock time in milliseconds since UNIX epoch.
fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Sample a file's modification time in milliseconds since UNIX epoch,
/// or `None` when the file is missing / metadata unavailable.
pub fn file_mtime_ms(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(duration.as_millis() as u64)
}

// ── Convenience wrappers (short names mirroring Python source) ───────

/// Convenience wrapper around [`FileStateRegistry::record_read`] on the
/// process singleton.
pub fn record_read(task_id: &str, resolved: &str, partial: bool) {
    registry().record_read(task_id, resolved, partial, None);
}

/// Convenience wrapper around [`FileStateRegistry::note_write`].
pub fn note_write(task_id: &str, resolved: &str) {
    registry().note_write(task_id, resolved, None);
}

/// Convenience wrapper around [`FileStateRegistry::check_stale`].
pub fn check_stale(task_id: &str, resolved: &str) -> Option<String> {
    registry().check_stale(task_id, resolved)
}

/// Convenience wrapper around [`FileStateRegistry::lock_path`].
pub fn lock_path(resolved: &str) -> PathLockGuard {
    registry().lock_path(resolved)
}

/// Convenience wrapper around [`FileStateRegistry::known_reads`].
pub fn known_reads(task_id: &str) -> Vec<String> {
    registry().known_reads(task_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Mutex as StdMutex, OnceLock};
    use std::time::Duration;
    use tempfile::tempdir;

    /// Tests use the process singleton, so they MUST run serially —
    /// `cargo test` runs threads in parallel by default. A test-only
    /// mutex serialises every test in this module against every other.
    fn test_serializer() -> &'static StdMutex<()> {
        static LOCK: OnceLock<StdMutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| StdMutex::new(()))
    }

    /// Guard that takes the test serializer, ensures the disable flag
    /// is OFF for the duration of the test, and clears the registry on
    /// both entry and exit so test ordering cannot leak state.
    struct TestEnv {
        _serial: std::sync::MutexGuard<'static, ()>,
    }

    impl TestEnv {
        fn new() -> Self {
            let guard = test_serializer().lock().unwrap_or_else(|p| p.into_inner());
            // Force-disable the env flag; some shells inherit it.
            // SAFETY: We hold the test serializer lock, so no other
            // thread inside this module is touching env vars.
            unsafe {
                std::env::remove_var("WAYLAND_DISABLE_FILE_STATE_GUARD");
            }
            registry().clear();
            Self { _serial: guard }
        }
    }

    impl Drop for TestEnv {
        fn drop(&mut self) {
            registry().clear();
        }
    }

    fn write_file(path: &std::path::Path, contents: &str) {
        fs::write(path, contents).expect("write tmp file");
    }

    #[test]
    fn record_then_check_clean_returns_none() {
        let _env = TestEnv::new();
        let dir = tempdir().unwrap();
        let p = dir.path().join("a.txt");
        write_file(&p, "hello");
        let resolved = p.to_str().unwrap();

        record_read("agent-1", resolved, false);
        assert_eq!(check_stale("agent-1", resolved), None);
    }

    #[test]
    fn write_without_read_warns() {
        let _env = TestEnv::new();
        let dir = tempdir().unwrap();
        let p = dir.path().join("b.txt");
        write_file(&p, "init");
        let resolved = p.to_str().unwrap();

        // Some unrelated agent wrote first to register the path.
        note_write("other-agent", resolved);

        let warning = check_stale("agent-2", resolved)
            .expect("write-without-read by different agent should warn");
        assert!(
            warning.contains("sibling subagent"),
            "expected sibling-subagent warning, got: {warning}"
        );
    }

    #[test]
    fn sibling_write_after_read_warns() {
        let _env = TestEnv::new();
        let dir = tempdir().unwrap();
        let p = dir.path().join("c.txt");
        write_file(&p, "v1");
        let resolved = p.to_str().unwrap();

        // agent-A reads.
        record_read("agent-A", resolved, false);
        // Sleep so the write timestamp is strictly later than the read.
        std::thread::sleep(Duration::from_millis(5));
        // agent-B writes — must also update mtime on disk so the disk
        // mtime no longer matches agent-A's read stamp.
        std::thread::sleep(Duration::from_millis(10));
        write_file(&p, "v2-from-B");
        note_write("agent-B", resolved);

        let warning = check_stale("agent-A", resolved).expect("sibling write after read must warn");
        assert!(
            warning.contains("sibling subagent"),
            "expected sibling warning, got: {warning}"
        );
    }

    #[test]
    fn external_mtime_drift_warns() {
        let _env = TestEnv::new();
        let dir = tempdir().unwrap();
        let p = dir.path().join("d.txt");
        write_file(&p, "v1");
        let resolved = p.to_str().unwrap();

        record_read("agent-X", resolved, false);
        // Simulate external editor: bump mtime via a fresh write
        // without going through note_write().
        std::thread::sleep(Duration::from_millis(10));
        write_file(&p, "v1-external-edit-different-size");

        let warning = check_stale("agent-X", resolved).expect("external mtime drift must warn");
        assert!(
            warning.contains("modified since you last read"),
            "expected external-edit warning, got: {warning}"
        );
    }

    #[test]
    fn partial_read_warns_on_write() {
        let _env = TestEnv::new();
        let dir = tempdir().unwrap();
        let p = dir.path().join("e.txt");
        write_file(&p, "line1\nline2\nline3");
        let resolved = p.to_str().unwrap();

        // Partial read (offset/limit window).
        record_read("agent-P", resolved, true);
        // No mtime drift, no sibling write — only the partial flag
        // should trip the warning.
        let warning = check_stale("agent-P", resolved)
            .expect("partial read followed by full overwrite must warn");
        assert!(
            warning.contains("partial view"),
            "expected partial-view warning, got: {warning}"
        );
    }

    #[test]
    fn note_write_clears_own_partial_flag() {
        let _env = TestEnv::new();
        let dir = tempdir().unwrap();
        let p = dir.path().join("f.txt");
        write_file(&p, "init");
        let resolved = p.to_str().unwrap();

        // Partial read first.
        record_read("agent-W", resolved, true);
        // Then own write — the agent now has the full content.
        note_write("agent-W", resolved);
        // No staleness because the writer's own view was refreshed.
        assert_eq!(check_stale("agent-W", resolved), None);
    }

    #[test]
    fn disable_flag_makes_all_ops_noop() {
        let _env = TestEnv::new();
        let dir = tempdir().unwrap();
        let p = dir.path().join("g.txt");
        write_file(&p, "init");
        let resolved = p.to_str().unwrap();

        // SAFETY: serialised by TestEnv.
        unsafe {
            std::env::set_var("WAYLAND_DISABLE_FILE_STATE_GUARD", "1");
        }
        // Even with a sibling write, check_stale must return None.
        note_write("other", resolved);
        assert_eq!(check_stale("me", resolved), None);
        assert!(known_reads("me").is_empty());
        // Cleanup so the next test doesn't see the flag.
        // SAFETY: serialised by TestEnv.
        unsafe {
            std::env::remove_var("WAYLAND_DISABLE_FILE_STATE_GUARD");
        }
    }

    #[test]
    fn writes_since_filters_correctly() {
        let _env = TestEnv::new();
        let dir = tempdir().unwrap();
        let p1 = dir.path().join("h1.txt");
        let p2 = dir.path().join("h2.txt");
        let p3 = dir.path().join("h3.txt");
        write_file(&p1, "1");
        write_file(&p2, "2");
        write_file(&p3, "3");
        let r1 = p1.to_str().unwrap().to_string();
        let r2 = p2.to_str().unwrap().to_string();
        let r3 = p3.to_str().unwrap().to_string();

        let cutoff = now_ms();
        std::thread::sleep(Duration::from_millis(2));

        note_write("alice", &r1); // included
        note_write("bob", &r2); // included
        note_write("parent", &r3); // excluded (matches exclude_task_id)

        let out = registry().writes_since("parent", cutoff, [&r1, &r2, &r3]);
        // Parent's own write must be filtered.
        assert!(!out.contains_key("parent"));
        assert_eq!(out.get("alice").map(|v| v.as_slice()), Some(&[r1][..]));
        assert_eq!(out.get("bob").map(|v| v.as_slice()), Some(&[r2][..]));
    }

    #[test]
    fn known_reads_returns_recorded_paths() {
        let _env = TestEnv::new();
        let dir = tempdir().unwrap();
        let p1 = dir.path().join("k1.txt");
        let p2 = dir.path().join("k2.txt");
        write_file(&p1, "x");
        write_file(&p2, "y");
        record_read("agent-K", p1.to_str().unwrap(), false);
        record_read("agent-K", p2.to_str().unwrap(), false);
        let reads = known_reads("agent-K");
        assert_eq!(reads.len(), 2);
        assert!(reads.iter().any(|s| s.ends_with("k1.txt")));
        assert!(reads.iter().any(|s| s.ends_with("k2.txt")));
    }

    #[test]
    fn per_path_lock_serializes_same_path() {
        let _env = TestEnv::new();
        let dir = tempdir().unwrap();
        let p = dir.path().join("locked.txt");
        write_file(&p, "v");
        let resolved = p.to_str().unwrap().to_string();

        let order: Arc<StdMutex<Vec<&'static str>>> = Arc::new(StdMutex::new(Vec::new()));

        let r1 = resolved.clone();
        let o1 = order.clone();
        let t1 = std::thread::spawn(move || {
            let _g = lock_path(&r1);
            o1.lock().unwrap().push("A-enter");
            std::thread::sleep(Duration::from_millis(50));
            o1.lock().unwrap().push("A-exit");
        });
        // Give thread A a head start so it grabs the lock first.
        std::thread::sleep(Duration::from_millis(5));
        let r2 = resolved.clone();
        let o2 = order.clone();
        let t2 = std::thread::spawn(move || {
            let _g = lock_path(&r2);
            o2.lock().unwrap().push("B-enter");
            o2.lock().unwrap().push("B-exit");
        });
        t1.join().unwrap();
        t2.join().unwrap();
        let final_order = order.lock().unwrap();
        // B-enter must come after A-exit because the lock serialises
        // them.
        let a_exit = final_order.iter().position(|s| *s == "A-exit").unwrap();
        let b_enter = final_order.iter().position(|s| *s == "B-enter").unwrap();
        assert!(
            a_exit < b_enter,
            "lock must serialise: order was {final_order:?}"
        );
    }

    /// Regression for `supply-unsafe-62` / `rel-panic-69`: the
    /// `PathLockGuard` self-referential drop-order invariant. Dropping a
    /// guard must release the mutex (so the same path can be re-locked) and
    /// must not free the backing `Arc<Mutex>` before the guard unlocks it
    /// (which would be a use-after-free at unlock time). Repeated
    /// acquire→drop cycles on one path exercise the unlock path under the
    /// `ManuallyDrop` ordering; under Miri this would trip on a wrong order.
    #[test]
    fn path_lock_guard_releases_on_drop() {
        let _env = TestEnv::new();
        let dir = tempdir().unwrap();
        let p = dir.path().join("relock.txt");
        write_file(&p, "v");
        let resolved = p.to_str().unwrap().to_string();

        // Sequential acquire/drop: each iteration unlocks the mutex held by
        // the previous guard. If the Arc were dropped before the guard, the
        // unlock would touch freed memory.
        for _ in 0..1000 {
            let g = lock_path(&resolved);
            drop(g);
        }

        // After the loop the lock is free, so a fresh guard can be taken on
        // the *same* path without deadlocking.
        let g = lock_path(&resolved);
        drop(g);
    }

    #[test]
    fn cap_drops_oldest_per_agent() {
        let _env = TestEnv::new();
        // Synthetic mtime override so we don't need to touch the
        // filesystem 4097 times.
        let task = "cap-agent";
        for i in 0..(MAX_PATHS_PER_AGENT + 5) {
            registry().record_read(task, &format!("/synthetic/{i}"), false, Some(1));
        }
        let reads = known_reads(task);
        assert_eq!(reads.len(), MAX_PATHS_PER_AGENT);
        // The first 5 (oldest) entries should have been dropped.
        assert!(!reads.iter().any(|s| s == "/synthetic/0"));
        assert!(!reads.iter().any(|s| s == "/synthetic/4"));
        // Latest entries must survive.
        assert!(
            reads
                .iter()
                .any(|s| s == &format!("/synthetic/{}", MAX_PATHS_PER_AGENT + 4))
        );
    }

    #[test]
    fn nonexistent_file_does_not_warn() {
        let _env = TestEnv::new();
        // record_read on a path that does not exist must be silently
        // dropped (mtime sample fails), and check_stale must return
        // None for a path with no recorded reads AND no writers.
        record_read("agent-N", "/no/such/path/ever", false);
        assert_eq!(check_stale("agent-N", "/no/such/path/ever"), None);
        assert!(known_reads("agent-N").is_empty());
    }
}
