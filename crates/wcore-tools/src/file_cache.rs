use std::hash::{Hash, Hasher};
use std::num::NonZeroUsize;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;

use lru::LruCache;

use wcore_config::file_cache::FileCacheConfig;
use wcore_types::file_state::{FileState, Provenance};

/// Token-opt (read-once): a re-issued Grep/Glob/Bash whose output is smaller
/// than this many bytes is never deduplicated — the backref stub itself costs
/// ~100 bytes, so deduping a tiny output would be a net loss.
const MIN_DEDUP_BYTES: usize = 300;
/// Token-opt (read-once): max distinct tool outputs tracked for backref dedup.
const OUTPUT_BACKREF_CAP: usize = 128;

/// Token-opt (read-once): the earliest tool call that produced a given output,
/// so a later identical output can point back to it instead of re-sending bytes.
#[derive(Debug, Clone)]
struct OutputBackref {
    /// Human-readable identifier of the originating call (e.g. `Grep for "foo"`).
    label: String,
    /// Compaction generation when recorded; a hit requires it to still match,
    /// proving the referenced result is still in the model's visible transcript.
    gen_at_record: u64,
    /// Byte length of the original output — a second equality check alongside the
    /// hash, so an astronomically-unlikely hash collision can't mis-dedup.
    len: usize,
}

/// LRU cache for file states seen by the model.
///
/// Provides dual eviction: entry-count limit (via LRU) and byte-size limit
/// (manually tracked). All path keys are normalized before access so that
/// `"/a/../b"` and `"/b"` map to the same cache slot.
///
/// Thread safety: wrap in `Arc<std::sync::RwLock<FileStateCache>>` when
/// sharing across tools. Cache operations are brief (hash lookup + insert),
/// so `std::sync::RwLock` is preferred over `tokio::sync::RwLock`.
pub struct FileStateCache {
    entries: LruCache<PathBuf, FileState>,
    max_size_bytes: usize,
    current_size_bytes: usize,
    /// Token-opt (diff-resend): whether re-reads on this route may be answered
    /// with a line diff instead of full content. Set once from the resolved
    /// `ProviderCompat.input_optimization()` (`"client"` → true). Router-side
    /// optimization leaves this false so the engine sends full reads and defers
    /// to the server.
    optimize_reads: bool,
    /// Token-opt (diff-resend): monotonically bumped by the engine whenever a
    /// compaction pass (autocompact OR microcompact) runs. A cached read is only
    /// safe to answer with a diff if this generation is unchanged since the read
    /// was cached — otherwise the base content the diff references may have been
    /// collapsed or cleared out of the model's visible transcript. See
    /// [`FileState::gen_at_read`].
    compaction_generation: u64,
    /// Token-opt (read-once): content-addressed map of recent Grep/Glob/Bash
    /// outputs (hash → first call that produced them). Lets a re-issued tool
    /// whose output is byte-identical return a short backref instead of
    /// re-sending the whole result. Shares `compaction_generation` (visibility)
    /// and `optimize_reads` (route gate) with the read cache.
    output_backrefs: LruCache<u64, OutputBackref>,
}

impl FileStateCache {
    /// Create a new cache from configuration.
    ///
    /// If `max_entries` is 0, defaults to 100.
    pub fn new(config: &FileCacheConfig) -> Self {
        let cap = NonZeroUsize::new(config.max_entries)
            // SAFETY: 100 is a non-zero compile-time constant.
            .unwrap_or(NonZeroUsize::new(100).expect("100 is non-zero"));
        Self {
            entries: LruCache::new(cap),
            max_size_bytes: config.max_size_bytes,
            current_size_bytes: 0,
            optimize_reads: false,
            compaction_generation: 0,
            output_backrefs: LruCache::new(
                NonZeroUsize::new(OUTPUT_BACKREF_CAP).expect("cap is non-zero"),
            ),
        }
    }

    /// Token-opt (read-once): check whether `content` (a Grep/Glob/Bash output)
    /// is byte-identical to an output already emitted this generation, and if so
    /// return a short backref stub pointing at the earlier call. Otherwise record
    /// this call as the canonical source and return `None` (the caller keeps the
    /// full output).
    ///
    /// Gated on `optimize_reads` (client routes only) and a minimum size, so a
    /// tiny output is never deduped into a longer stub. The generation check
    /// makes a hit safe: it only fires while the referenced result is still in
    /// the model's visible transcript.
    pub fn output_backref(&mut self, content: &str, label: &str) -> Option<String> {
        if !self.optimize_reads || content.len() < MIN_DEDUP_BYTES {
            return None;
        }
        let hash = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            content.as_bytes().hash(&mut h);
            h.finish()
        };
        if let Some(entry) = self.output_backrefs.get(&hash)
            && entry.gen_at_record == self.compaction_generation
            && entry.len == content.len()
        {
            return Some(format!(
                "(Identical to the earlier result of {} in this conversation — that output is \
                 unchanged and still shown above; refer to it instead of this repeat.)",
                entry.label
            ));
        }
        // Miss (or a stale entry from before a compaction): this call becomes the
        // canonical reference for its content at the current generation.
        self.output_backrefs.put(
            hash,
            OutputBackref {
                label: label.to_string(),
                gen_at_record: self.compaction_generation,
                len: content.len(),
            },
        );
        None
    }

    /// Token-opt: enable/disable diff-resend for this cache's route. Called once
    /// at bootstrap from the resolved provider compat.
    pub fn set_optimize_reads(&mut self, enabled: bool) {
        self.optimize_reads = enabled;
    }

    /// Token-opt: whether diff-resend is enabled for this route.
    pub fn optimize_reads(&self) -> bool {
        self.optimize_reads
    }

    /// Token-opt: the current compaction generation. Stamped onto each cached
    /// read; a diff is only emitted when it still matches.
    pub fn compaction_generation(&self) -> u64 {
        self.compaction_generation
    }

    /// Token-opt: bump the compaction generation. The engine calls this after
    /// any compaction pass so stale read bases stop qualifying for diff-resend.
    pub fn bump_compaction_generation(&mut self) {
        self.compaction_generation = self.compaction_generation.saturating_add(1);
    }

    /// Look up a file state, promoting it to most-recently-used.
    pub fn get(&mut self, path: &Path) -> Option<&FileState> {
        let normalized = normalize_path(path);
        self.entries.get(&normalized)
    }

    /// Insert or update a file state entry.
    ///
    /// Evicts least-recently-used entries when the byte-size limit or
    /// entry-count limit would be exceeded.
    pub fn insert(&mut self, path: PathBuf, state: FileState) {
        let normalized = normalize_path(&path);
        let new_size = state.content_bytes();

        // Remove existing entry for this key first (simplifies size accounting).
        if let Some(old) = self.entries.pop(&normalized) {
            self.current_size_bytes = self.current_size_bytes.saturating_sub(old.content_bytes());
        }

        // Evict LRU entries until byte-size budget is available.
        while self.current_size_bytes + new_size > self.max_size_bytes && !self.entries.is_empty() {
            if let Some((_k, v)) = self.entries.pop_lru() {
                self.current_size_bytes = self.current_size_bytes.saturating_sub(v.content_bytes());
            }
        }

        // push() returns evicted (key, value) if entry-count capacity is reached.
        if let Some((_evicted_key, evicted_val)) = self.entries.push(normalized, state) {
            self.current_size_bytes = self
                .current_size_bytes
                .saturating_sub(evicted_val.content_bytes());
        }
        self.current_size_bytes += new_size;
    }

    /// Remove a specific entry by path.
    pub fn remove(&mut self, path: &Path) -> Option<FileState> {
        let normalized = normalize_path(path);
        let removed = self.entries.pop(&normalized);
        if let Some(ref v) = removed {
            self.current_size_bytes = self.current_size_bytes.saturating_sub(v.content_bytes());
        }
        removed
    }

    /// Remove all entries (read states AND read-once backrefs). Called on a
    /// conversation reset (`/clear`, `/resume`): none of the prior reads or tool
    /// outputs are visible to the model any more, so neither a dedup stub nor a
    /// backref may reference them.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.current_size_bytes = 0;
        self.output_backrefs.clear();
    }

    /// Number of cached entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Current total byte size of all cached content.
    pub fn current_size_bytes(&self) -> usize {
        self.current_size_bytes
    }
}

/// Update the cache after a successful file write (Edit or Write).
///
/// Reads the new mtime from disk and stores line-numbered content.
/// This is the single point for post-write cache updates, eliminating
/// duplication between EditTool and WriteTool.
///
/// The entry is tagged [`Provenance::WriteEcho`]: the content reflects the new
/// disk state, but the model has NOT seen it as a Read result. A later
/// verify-read therefore must not be short-circuited to the "file unchanged"
/// stub (which would point the model at the pre-write content still sitting in
/// the transcript). See [`Provenance`].
pub fn update_cache_after_write(
    cache_arc: &Arc<std::sync::RwLock<FileStateCache>>,
    path: &Path,
    content: &str,
) {
    let Ok(mut cache) = cache_arc.write() else {
        return;
    };
    let Some(new_mtime) = file_mtime_ms(path) else {
        return;
    };
    let numbered: Vec<String> = content
        .lines()
        .enumerate()
        .map(|(i, line)| format!("{:>6}\t{}", i + 1, line))
        .collect();
    let gen_at_read = cache.compaction_generation();
    cache.insert(
        path.to_path_buf(),
        FileState {
            content: numbered.join("\n"),
            mtime_ms: new_mtime,
            offset: None,
            limit: None,
            provenance: Provenance::WriteEcho,
            gen_at_read,
        },
    );
}

/// Get file modification time as milliseconds since UNIX epoch.
///
/// Returns `None` if the file does not exist or metadata is unavailable.
pub fn file_mtime_ms(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let duration = modified.duration_since(UNIX_EPOCH).ok()?;
    Some(duration.as_millis() as u64)
}

/// Normalize a path by resolving `.` and `..` components without filesystem access.
///
/// Unlike `std::fs::canonicalize`, this does not require the path to exist on disk,
/// which is important because cache lookups can happen before the file is created.
///
/// Examples:
/// - `/a/../b/file` -> `/b/file`
/// - `a/./b/../c`   -> `a/c`
/// - `/../b`        -> `/b` (can't go above root)
fn normalize_path(path: &Path) -> PathBuf {
    let mut components: Vec<Component> = Vec::new();
    for component in path.components() {
        match component {
            Component::ParentDir => match components.last() {
                Some(Component::Normal(_)) => {
                    components.pop();
                }
                Some(Component::RootDir) => {
                    // Can't go above filesystem root; ignore the `..`
                }
                _ => {
                    // Preserve leading `..` in relative paths (e.g. `../../foo`)
                    components.push(component);
                }
            },
            Component::CurDir => {} // skip `.`
            other => components.push(other),
        }
    }
    let mut result = PathBuf::new();
    for c in &components {
        result.push(c);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(max_entries: usize, max_size_bytes: usize) -> FileCacheConfig {
        FileCacheConfig {
            max_entries,
            max_size_bytes,
            enabled: true,
        }
    }

    fn make_state(content: &str, mtime_ms: u64) -> FileState {
        FileState {
            content: content.to_string(),
            mtime_ms,
            offset: None,
            limit: None,
            provenance: Provenance::ReadResult,
            gen_at_read: 0,
        }
    }

    // -- normalize_path tests --

    #[test]
    fn normalize_resolves_parent_dir() {
        let result = normalize_path(Path::new("/a/../b/file"));
        assert_eq!(result, PathBuf::from("/b/file"));
    }

    #[test]
    fn normalize_resolves_cur_dir() {
        let result = normalize_path(Path::new("/a/./b/file"));
        assert_eq!(result, PathBuf::from("/a/b/file"));
    }

    #[test]
    fn normalize_above_root_is_clamped() {
        let result = normalize_path(Path::new("/../b"));
        assert_eq!(result, PathBuf::from("/b"));
    }

    #[test]
    fn normalize_preserves_leading_parent_in_relative() {
        let result = normalize_path(Path::new("../../foo"));
        assert_eq!(result, PathBuf::from("../../foo"));
    }

    #[test]
    fn normalize_mixed() {
        let result = normalize_path(Path::new("a/./b/../c"));
        assert_eq!(result, PathBuf::from("a/c"));
    }

    #[test]
    fn normalize_absolute_identity() {
        let result = normalize_path(Path::new("/usr/local/bin"));
        assert_eq!(result, PathBuf::from("/usr/local/bin"));
    }

    // -- FileStateCache core tests --

    #[test]
    fn insert_and_get() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        let path = PathBuf::from("/tmp/test.rs");
        let state = make_state("hello", 1000);

        cache.insert(path.clone(), state);
        let got = cache.get(&path).unwrap();
        assert_eq!(got.content, "hello");
        assert_eq!(got.mtime_ms, 1000);
    }

    #[test]
    fn get_nonexistent_returns_none() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);
        assert!(cache.get(Path::new("/does/not/exist")).is_none());
    }

    #[test]
    fn lru_eviction_by_count() {
        let config = make_config(3, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("a", 1));
        cache.insert(PathBuf::from("/b"), make_state("b", 2));
        cache.insert(PathBuf::from("/c"), make_state("c", 3));
        // Cache is at capacity (3). Inserting a 4th evicts the LRU (/a).
        cache.insert(PathBuf::from("/d"), make_state("d", 4));

        assert!(cache.get(Path::new("/a")).is_none(), "/a should be evicted");
        assert!(cache.get(Path::new("/b")).is_some());
        assert!(cache.get(Path::new("/c")).is_some());
        assert!(cache.get(Path::new("/d")).is_some());
        assert_eq!(cache.len(), 3);
    }

    #[test]
    fn path_normalization_hits_same_slot() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a/../b/file"), make_state("v1", 100));
        let got = cache.get(Path::new("/b/file")).unwrap();
        assert_eq!(got.content, "v1");
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn clear_removes_all() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("a", 1));
        cache.insert(PathBuf::from("/b"), make_state("b", 2));
        assert_eq!(cache.len(), 2);

        cache.clear();
        assert_eq!(cache.len(), 0);
        assert!(cache.is_empty());
        assert_eq!(cache.current_size_bytes(), 0);
    }

    #[test]
    fn remove_deletes_entry() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("a-content", 1));
        let removed = cache.remove(Path::new("/a"));
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().content, "a-content");
        assert!(cache.get(Path::new("/a")).is_none());
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.current_size_bytes(), 0);
    }

    #[test]
    fn byte_size_eviction() {
        // max_size_bytes = 10, each entry ~5 bytes ("aaaaa").
        let config = make_config(100, 10);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("aaaaa", 1)); // 5 bytes
        cache.insert(PathBuf::from("/b"), make_state("bbbbb", 2)); // 5 bytes -> total 10
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.current_size_bytes(), 10);

        // Inserting /c (5 bytes) would exceed 10 -> evicts /a (LRU)
        cache.insert(PathBuf::from("/c"), make_state("ccccc", 3));
        assert!(cache.get(Path::new("/a")).is_none(), "/a should be evicted");
        assert!(cache.get(Path::new("/b")).is_some());
        assert!(cache.get(Path::new("/c")).is_some());
        assert_eq!(cache.current_size_bytes(), 10);
    }

    #[test]
    fn overwrite_same_key() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("v1", 100));
        cache.insert(PathBuf::from("/a"), make_state("v2-longer", 200));

        let got = cache.get(Path::new("/a")).unwrap();
        assert_eq!(got.content, "v2-longer");
        assert_eq!(got.mtime_ms, 200);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.current_size_bytes(), "v2-longer".len());
    }

    #[test]
    fn size_accounting_after_remove() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("hello", 1)); // 5 bytes
        cache.insert(PathBuf::from("/b"), make_state("world!", 2)); // 6 bytes
        assert_eq!(cache.current_size_bytes(), 11);

        cache.remove(Path::new("/a"));
        assert_eq!(cache.current_size_bytes(), 6);
    }

    #[test]
    fn zero_max_entries_defaults_to_100() {
        let config = make_config(0, 1_000_000);
        let mut cache = FileStateCache::new(&config);
        // Should not panic; defaults to capacity 100.
        for i in 0..100 {
            cache.insert(PathBuf::from(format!("/f{}", i)), make_state("x", i as u64));
        }
        assert_eq!(cache.len(), 100);
    }

    #[test]
    fn get_promotes_entry_preventing_eviction() {
        let config = make_config(3, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/a"), make_state("a", 1));
        cache.insert(PathBuf::from("/b"), make_state("b", 2));
        cache.insert(PathBuf::from("/c"), make_state("c", 3));

        // Access /a to promote it; now /b is the LRU.
        cache.get(Path::new("/a"));

        // Insert /d -> evicts /b (LRU), not /a.
        cache.insert(PathBuf::from("/d"), make_state("d", 4));
        assert!(cache.get(Path::new("/a")).is_some(), "/a should survive");
        assert!(cache.get(Path::new("/b")).is_none(), "/b should be evicted");
    }

    #[test]
    fn empty_content_cached() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        cache.insert(PathBuf::from("/empty"), make_state("", 1));
        assert!(cache.get(Path::new("/empty")).is_some());
        assert_eq!(cache.current_size_bytes(), 0);
    }

    // -- read-once output-backref tests --

    fn opt_cache() -> FileStateCache {
        let mut c = FileStateCache::new(&make_config(10, 1_000_000));
        c.set_optimize_reads(true);
        c
    }

    #[test]
    fn output_backref_dedups_identical_repeat() {
        let mut cache = opt_cache();
        let big = "match: foo\n".repeat(40); // > MIN_DEDUP_BYTES
        // First occurrence records and keeps the full output.
        assert!(cache.output_backref(&big, "`Grep` (foo)").is_none());
        // Second identical occurrence → a shorter backref stub.
        let stub = cache
            .output_backref(&big, "`Grep` (foo)")
            .expect("identical repeat should dedup");
        assert!(
            stub.len() < big.len(),
            "backref must be smaller than output"
        );
        assert!(stub.contains("Identical to the earlier result"));
        assert!(stub.contains("Grep"));
    }

    #[test]
    fn output_backref_route_gated_off_by_default() {
        // optimize_reads defaults false (router route) → never dedups.
        let mut cache = FileStateCache::new(&make_config(10, 1_000_000));
        let big = "x".repeat(500);
        assert!(cache.output_backref(&big, "l").is_none());
        assert!(cache.output_backref(&big, "l").is_none());
    }

    #[test]
    fn output_backref_skips_small_outputs() {
        let mut cache = opt_cache();
        let small = "tiny output".to_string(); // < MIN_DEDUP_BYTES
        assert!(cache.output_backref(&small, "l").is_none());
        assert!(cache.output_backref(&small, "l").is_none());
    }

    #[test]
    fn output_backref_distinct_outputs_do_not_dedup() {
        let mut cache = opt_cache();
        let a = "a".repeat(500);
        let b = "b".repeat(500);
        assert!(cache.output_backref(&a, "l").is_none());
        assert!(cache.output_backref(&b, "l").is_none());
    }

    #[test]
    fn output_backref_generation_bump_invalidates() {
        let mut cache = opt_cache();
        let big = "z".repeat(500);
        assert!(cache.output_backref(&big, "l").is_none()); // recorded at gen 0
        cache.bump_compaction_generation(); // a compaction collapsed the transcript
        // The referenced result may be gone → must NOT dedup; re-records at new gen.
        assert!(cache.output_backref(&big, "l").is_none());
        // A repeat at the new generation dedups again.
        assert!(cache.output_backref(&big, "l").is_some());
    }

    #[test]
    fn clear_wipes_output_backrefs() {
        let mut cache = opt_cache();
        let big = "w".repeat(500);
        assert!(cache.output_backref(&big, "l").is_none());
        cache.clear();
        // After a conversation reset the prior output is gone → first occurrence.
        assert!(cache.output_backref(&big, "l").is_none());
    }

    #[test]
    fn partial_read_state_preserved() {
        let config = make_config(10, 1_000_000);
        let mut cache = FileStateCache::new(&config);

        let state = FileState {
            content: "partial content".to_string(),
            mtime_ms: 500,
            offset: Some(10),
            limit: Some(20),
            provenance: Provenance::ReadResult,
            gen_at_read: 0,
        };
        cache.insert(PathBuf::from("/file"), state);
        let got = cache.get(Path::new("/file")).unwrap();
        assert_eq!(got.offset, Some(10));
        assert_eq!(got.limit, Some(20));
    }
}
