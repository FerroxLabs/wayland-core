/// How a cached [`FileState`] came to hold its `content`.
///
/// This distinguishes "what the model actually saw as a Read tool_result"
/// from "what is currently on disk after a tool wrote it". They diverge after
/// any Edit/Write: the write refreshes the cache `content` and `mtime_ms` to
/// the post-write state, but the model has NOT seen that content as a read —
/// the most recent Read result in the transcript is still the pre-write text.
///
/// The dedup short-circuit ("file unchanged since last read, refer to the
/// earlier Read") is only sound for [`Provenance::ReadResult`]: a
/// [`Provenance::WriteEcho`] entry whose mtime happens to match disk would
/// otherwise point the model at stale pre-edit content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Provenance {
    /// `content` was emitted verbatim as a Read tool_result — i.e. it is what
    /// the model saw. Safe to reference from a "file unchanged" stub.
    #[default]
    ReadResult,
    /// `content` is the post-write disk state echoed by Edit/Write. The model
    /// has NOT seen it as a read, so it must never back a "file unchanged" stub.
    WriteEcho,
}

/// Cached state of a file that the model has seen.
///
/// Stored in an LRU cache keyed by normalized file path.
/// Used by Read/Edit/Write tools for dedup detection and staleness checks.
#[derive(Debug, Clone)]
pub struct FileState {
    /// File content as seen by the model (with line numbers).
    pub content: String,
    /// File modification time when last read (milliseconds since UNIX epoch).
    pub mtime_ms: u64,
    /// Line offset of partial read (None = full read).
    pub offset: Option<usize>,
    /// Line limit of partial read (None = full read).
    pub limit: Option<usize>,
    /// Whether `content` is what the model saw (ReadResult) or a post-write
    /// echo it has not seen (WriteEcho). Gates the dedup "unchanged" stub.
    pub provenance: Provenance,
}

impl FileState {
    /// Byte size of the cached content (used for cache size accounting).
    pub fn content_bytes(&self) -> usize {
        self.content.len()
    }
}
