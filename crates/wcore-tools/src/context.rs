//! W8a A.3 — `ToolContext` threaded into the new `Tool::execute_with_ctx`
//! entry point.
//!
//! Tools receive a `ToolContext` so they can:
//!   * race their long work against `ctx.cancel.cancelled()` (S2);
//!   * read/write through `ctx.vfs` (X2) — `RealFs` for top-level tools,
//!     `SandboxedFs { root }` for sandboxed sub-agents (reads and writes
//!     both sandbox-checked since Wave SD; `fallthrough_reads` is gone),
//!     `InMemoryFs` in tests;
//!   * emit streaming chunks/progress via `ctx.sink` without depending on
//!     the orchestration crate (W7 ToolOutputSink lives in wcore-tools).
//!
//! Budget tracking (`wcore-agent::budget::ExecutionBudgetView`) does NOT
//! ride on `ToolContext` itself because that would invert the crate
//! dependency graph (wcore-tools < wcore-agent). Orchestration in
//! wcore-agent records token/cost usage on the budget view around each
//! tool dispatch; tools observe cancellation indirectly through `ctx.cancel`
//! (which orchestration links to the budget watcher in A.6).

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::file_write_notifier::FileWriteNotifier;
use crate::vfs::{RealFs, VirtualFs};
use crate::workspace_policy::WorkspacePolicy;
use crate::{NullToolOutputSink, ToolOutputSink};

/// Per-tool-call context. Cheap to construct in tests via
/// `ToolContext::test_default()`; orchestration builds one per dispatch
/// from the root `RootContext` (W8a A.6).
pub struct ToolContext {
    /// Stable ID for the in-flight tool call (matches `tool_call_id` on
    /// `ProtocolEvent::ToolRequest`). `String::new()` for synthetic test
    /// contexts.
    pub call_id: String,

    /// Cooperative cancellation token. Tools that perform long work
    /// MUST race their await against `ctx.cancel.cancelled()` and abort
    /// in <500ms when fired.
    pub cancel: CancellationToken,

    /// Virtual filesystem the tool reads/writes through. RealFs for
    /// top-level tools; SandboxedFs for sub-agents.
    pub vfs: Arc<dyn VirtualFs>,

    /// Optional sub-agent name. `None` = main agent. Used by tools
    /// (and the protocol sink) to route output back through the
    /// correct relay channel.
    pub source_agent: Option<String>,

    /// Output sink for chunked / progress emission. Tools that don't
    /// stream may ignore this; the default `NullToolOutputSink` is a
    /// no-op for tests and non-streaming hosts.
    pub sink: Arc<dyn ToolOutputSink>,

    /// W8b.2.A (D.4) — optional sink to inform an upstream FileWatcher
    /// that a path is about to be written by the engine. Write/Edit
    /// tools call `note_self_originated_write` immediately before the
    /// write so the watcher can debounce its own change event.
    ///
    /// `None` (the default in `test_default()` and any context built
    /// without a live watcher) means the tools skip the notify call and
    /// behave exactly as before.
    pub file_write_notifier: Option<Arc<dyn FileWriteNotifier>>,

    /// Contained-or-trusted workspace policy for this session. `None` (the
    /// pure test default) means unconfined legacy behaviour. When set,
    /// BashTool derives its OS-sandbox manifest (cwd, allowlists, cache env,
    /// network) from it; in `Contained` mode the dispatcher also installs a
    /// `SandboxedFs ∘ SecretDenyFs` jail as `vfs`.
    pub workspace: Option<Arc<WorkspacePolicy>>,
}

impl ToolContext {
    /// Synthesize a context for tests. RealFs root, an open cancel
    /// token, no source agent, null sink, no file-write notifier.
    pub fn test_default() -> Self {
        Self {
            call_id: String::new(),
            cancel: CancellationToken::new(),
            vfs: Arc::new(RealFs),
            source_agent: None,
            sink: Arc::new(NullToolOutputSink),
            file_write_notifier: None,
            workspace: None,
        }
    }

    /// Builder helper used by orchestration to mint a context for one
    /// tool call from the root state. The `file_write_notifier` field
    /// defaults to `None`; callers that have a live watcher use
    /// `with_file_write_notifier` to attach one.
    pub fn new(
        call_id: impl Into<String>,
        cancel: CancellationToken,
        vfs: Arc<dyn VirtualFs>,
        source_agent: Option<String>,
        sink: Arc<dyn ToolOutputSink>,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            cancel,
            vfs,
            source_agent,
            sink,
            file_write_notifier: None,
            workspace: None,
        }
    }

    /// W8b.2.A — fluent helper for orchestration / bootstrap to attach
    /// a `FileWriteNotifier` after construction. Lets the existing 5-arg
    /// `new` stay back-compatible while new wiring stays explicit at
    /// the call site.
    pub fn with_file_write_notifier(mut self, n: Arc<dyn FileWriteNotifier>) -> Self {
        self.file_write_notifier = Some(n);
        self
    }

    pub fn with_workspace(mut self, policy: Arc<WorkspacePolicy>) -> Self {
        self.workspace = Some(policy);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_write_notifier::FileWriteNotifier;
    use async_trait::async_trait;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    #[test]
    fn workspace_defaults_none_and_attaches() {
        use crate::workspace_policy::WorkspacePolicy;
        use std::sync::Arc;
        let ctx = ToolContext::test_default();
        assert!(ctx.workspace.is_none());
        let dir = tempfile::tempdir().unwrap();
        let policy = Arc::new(WorkspacePolicy::trusted_local(dir.path()));
        let ctx = ctx.with_workspace(Arc::clone(&policy));
        assert_eq!(ctx.workspace.as_ref().unwrap().root(), policy.root());
    }

    #[derive(Default)]
    struct CountingNotifier {
        seen: parking_lot::Mutex<Vec<PathBuf>>,
    }

    #[async_trait]
    impl FileWriteNotifier for CountingNotifier {
        async fn note_self_originated_write(&self, path: &Path) {
            self.seen.lock().push(path.to_path_buf());
        }
    }

    #[test]
    fn test_default_has_no_file_write_notifier() {
        let ctx = ToolContext::test_default();
        assert!(
            ctx.file_write_notifier.is_none(),
            "test_default() must NOT wire a notifier so legacy tests stay byte-identical"
        );
    }

    #[test]
    fn new_constructs_without_notifier() {
        let ctx = ToolContext::new(
            "call-1".to_string(),
            CancellationToken::new(),
            Arc::new(RealFs),
            None,
            Arc::new(NullToolOutputSink),
        );
        assert!(ctx.file_write_notifier.is_none());
    }

    #[tokio::test]
    async fn with_file_write_notifier_attaches_arc() {
        let notifier = Arc::new(CountingNotifier::default());
        let ctx = ToolContext::test_default()
            .with_file_write_notifier(notifier.clone() as Arc<dyn FileWriteNotifier>);
        assert!(ctx.file_write_notifier.is_some());
        // The Arc on the context propagates the same trait object —
        // calling note() through ctx must land in our shared counter.
        ctx.file_write_notifier
            .as_ref()
            .unwrap()
            .note_self_originated_write(Path::new("/tmp/wired"))
            .await;
        assert_eq!(
            notifier.seen.lock().as_slice(),
            &[PathBuf::from("/tmp/wired")]
        );
    }
}
