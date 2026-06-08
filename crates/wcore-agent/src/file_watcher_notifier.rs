//! W8b.2.A — adapter that lets `FileWatcher` implement the
//! `wcore_tools::file_write_notifier::FileWriteNotifier` trait without
//! flipping the dep edge.
//!
//! `wcore-tools` defines the trait; `wcore-agent::watch::FileWatcher`
//! owns the self-origination map. This adapter holds an
//! `Arc<FileWatcher>` and forwards `note_self_originated_write(path)`
//! to `FileWatcher::mark_self_originated(path)`. Bootstrap constructs
//! one alongside any live `FileWatcher` and threads it into the
//! orchestration-side `ToolContext` builder so Write/Edit tools can
//! suppress their own change events.

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use wcore_tools::file_write_notifier::FileWriteNotifier;

use crate::watch::FileWatcher;

/// Bridge adapter: holds an `Arc<FileWatcher>`, implements
/// `FileWriteNotifier` by forwarding to `FileWatcher::mark_self_originated`.
///
/// Cheap to clone (Arc-backed). Constructed once in bootstrap when
/// `agent.watch_files` is enabled and reused for the lifetime of the
/// session.
pub struct FileWatcherNotifier {
    watcher: Arc<FileWatcher>,
}

impl FileWatcherNotifier {
    pub fn new(watcher: Arc<FileWatcher>) -> Self {
        Self { watcher }
    }

    /// Convenience constructor: wraps the supplied watcher and returns
    /// an `Arc<dyn FileWriteNotifier>` ready to drop into
    /// `ToolContext::with_file_write_notifier`.
    pub fn arc(watcher: Arc<FileWatcher>) -> Arc<dyn FileWriteNotifier> {
        Arc::new(Self::new(watcher))
    }
}

#[async_trait]
impl FileWriteNotifier for FileWatcherNotifier {
    async fn note_self_originated_write(&self, path: &Path) {
        self.watcher.mark_self_originated(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tempfile::tempdir;

    /// W8b.2.A integration: wire a FileWatcher, expose it as a
    /// FileWriteNotifier, mark a path as self-originated, then trigger
    /// a real fs event on that path. The watcher must NOT surface the
    /// event externally — confirming the debounce path is intact.
    #[tokio::test]
    async fn self_originated_write_via_notifier_is_debounced() {
        let tmp = tempdir().expect("tempdir");
        let watcher = Arc::new(FileWatcher::new(tmp.path()).expect("watcher"));
        let notifier: Arc<dyn FileWriteNotifier> = FileWatcherNotifier::arc(watcher.clone());

        let file_path = tmp.path().join("self_write.txt");

        // Mark the write BEFORE we actually touch the fs.
        notifier.note_self_originated_write(&file_path).await;

        // Now perform the write the way an engine tool would.
        std::fs::write(&file_path, b"engine-originated content").expect("write");

        // The watcher must debounce this — drain_external_events should
        // not contain the path within the debounce window. Wait briefly
        // for notify to deliver the event into the channel, then drain.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let events = watcher.drain_external_events();
        assert!(
            events.iter().all(|ev| ev.path != file_path
                && std::fs::canonicalize(&ev.path).ok() != std::fs::canonicalize(&file_path).ok()),
            "self-originated write should be debounced; got events: {:?}",
            events
        );
    }

    /// External edit (no self-origination mark) must still surface so
    /// the agent can re-read. This pins the negative side of the
    /// adapter — proves we didn't silence every event.
    #[tokio::test]
    async fn external_write_without_notifier_mark_surfaces() {
        let tmp = tempdir().expect("tempdir");
        let watcher = Arc::new(FileWatcher::new(tmp.path()).expect("watcher"));
        // notifier exists but we never call note_self_originated_write,
        // so the watcher treats the write as external.
        let _notifier: Arc<dyn FileWriteNotifier> = FileWatcherNotifier::arc(watcher.clone());

        let file_path = tmp.path().join("external_write.txt");
        std::fs::write(&file_path, b"user-originated content").expect("write");

        // Allow the notify backend to deliver. Use the async wait API
        // since macOS FSEvents can take >50ms on first event.
        let ev = watcher
            .next_external_event(Duration::from_secs(2))
            .await
            .expect("watcher should surface external write");
        // notify may report `ev.path` as the canonical resolution
        // (e.g. /var -> /private/var on macOS); compare canonical forms.
        let want = std::fs::canonicalize(&file_path).ok();
        let got = std::fs::canonicalize(&ev.path).ok();
        assert_eq!(want, got, "watcher should surface the written path");
    }
}
