//! W8b.2.A — `FileWriteNotifier` trait carried on `ToolContext`.
//!
//! Tools that write to disk (Write, Edit) call
//! `ctx.file_write_notifier.note_self_originated_write(path)` immediately
//! before they perform the write. An upstream `FileWatcher` (in
//! `wcore-agent`) implements this trait via an adapter so it can debounce
//! its own change event and avoid feeding engine-originated writes back
//! into the agent's context as "external edits".
//!
//! Trait-inversion: `FileWatcher` lives in `wcore-agent`, which depends
//! on `wcore-tools`. We cannot add `Arc<FileWatcher>` to `ToolContext`
//! without flipping the dep edge. Instead, `wcore-tools` defines the
//! trait surface; `wcore-agent` ships the `FileWatcherNotifier` adapter
//! that forwards to `FileWatcher::mark_self_originated`. Same pattern
//! as W7 `ApprovalProducer` and W8a `ExecutionBudgetView`.

use std::path::Path;

use async_trait::async_trait;

/// Sink that records the engine's intent to write a path so a downstream
/// filesystem watcher can suppress the self-originated change event.
///
/// Implementations must be cheap (a single map insert today); tools call
/// this in the hot path right before every write.
#[async_trait]
pub trait FileWriteNotifier: Send + Sync {
    /// Mark `path` as about-to-be-written-by-us. Called by Write/Edit
    /// tools immediately before performing the write via `ctx.vfs`.
    async fn note_self_originated_write(&self, path: &Path);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;

    use parking_lot::Mutex;

    /// Recording stub used by the wcore-tools unit tests + Write/Edit
    /// integration tests below. Appends every notified path to an
    /// internal Vec so the test can assert call shape.
    #[derive(Default, Clone)]
    pub struct RecordingNotifier {
        pub seen: Arc<Mutex<Vec<PathBuf>>>,
    }

    #[async_trait]
    impl FileWriteNotifier for RecordingNotifier {
        async fn note_self_originated_write(&self, path: &Path) {
            self.seen.lock().push(path.to_path_buf());
        }
    }

    #[tokio::test]
    async fn recording_notifier_captures_each_path() {
        let n = RecordingNotifier::default();
        n.note_self_originated_write(Path::new("/tmp/a")).await;
        n.note_self_originated_write(Path::new("/tmp/b")).await;
        let seen = n.seen.lock().clone();
        assert_eq!(seen, vec![PathBuf::from("/tmp/a"), PathBuf::from("/tmp/b")]);
    }
}
