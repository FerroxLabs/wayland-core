//! `BrowserProvider` trait + supporting value types.
//!
//! The trait surface is **backend-neutral**: Camoufox, chromiumoxide, and
//! Browserbase all implement it. The `BrowserTool` (in `tool.rs`) dispatches
//! every `BrowserOp` to a single provider for the lifetime of a session.
//!
//! Design §5.16: ARIA-tree-first. Snapshots return `AriaSnapshot` (compact
//! text representation, not raw DOM dump) so element refs `@e1`, `@e2`, ...
//! drive subsequent ops.

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::aria::AriaSnapshot;
use crate::op::BrowserOp;

/// Typed errors returned by `BrowserProvider` impls. Crash-recovery in the
/// supervisor uses `BackendCrashed` as the retry signal; policy denials
/// surface as `PolicyDenied` so the protocol sink can emit
/// `BrowserPolicyDenied` events.
#[derive(Debug, Error)]
pub enum BrowserOpError {
    #[error("backend crashed (retryable): {0}")]
    BackendCrashed(String),

    #[error("policy denied: {reason} (url={url})")]
    PolicyDenied { url: String, reason: String },

    #[error("policy suspended: ask required for {url}")]
    PolicySuspended { url: String },

    #[error("operation cancelled")]
    Cancelled,

    #[error("element ref not resolvable: {0}")]
    UnknownElementRef(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("backend error: {0}")]
    Backend(String),

    #[error("not implemented in this backend: {0}")]
    Unsupported(String),
}

/// Per-session execution context. Each `BrowserTool::execute` call mints a
/// session-scoped context tied to a single `BrowserContext` (CDP term) so
/// sub-agents are isolated by cookie jar + tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionCtx {
    pub session_id: String,
    /// Optional sub-agent name; main agent if `None`.
    pub sub_agent: Option<String>,
}

impl SessionCtx {
    pub fn for_test(id: impl Into<String>) -> Self {
        Self {
            session_id: id.into(),
            sub_agent: None,
        }
    }
}

/// Cookie-jar-isolated browser session managed by a backend. Returned from
/// `BrowserProvider::open_session`; the `SessionCtx` keeps the handle that
/// later `dispatch` calls thread back through.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserSession {
    pub ctx: SessionCtx,
    pub persistent_profile: bool,
}

/// Element pointed to by `@eN` (post-snapshot ref). Resolved against the
/// most-recent `AriaSnapshot` for the session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClickTarget {
    pub element_ref: String,
}

/// Screenshot options. Default: full-page PNG. Backends that don't support
/// full-page emit a single viewport-sized PNG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenshotOpts {
    pub full_page: bool,
    pub format: ScreenshotFormat,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScreenshotFormat {
    Png,
    Jpeg,
}

impl Default for ScreenshotOpts {
    fn default() -> Self {
        Self {
            full_page: true,
            format: ScreenshotFormat::Png,
        }
    }
}

/// One row in the per-session network log returned by `BrowserOp::NetworkLog`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetEntry {
    pub url: String,
    pub method: String,
    pub status: Option<u16>,
    pub duration_ms: Option<u64>,
    pub size_bytes: Option<u64>,
}

/// One row in the per-session console log returned by `BrowserOp::Console`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConsoleEntry {
    pub level: ConsoleLevel,
    pub text: String,
    /// Source location (file:line if available).
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsoleLevel {
    Log,
    Info,
    Warn,
    Error,
    Debug,
}

/// Compact result of any `BrowserOp` dispatch. Designed for round-trip via
/// `serde_json::Value` in `Tool::execute`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OpResult {
    /// Empty success — for ops that don't return data (Navigate, Click, ...).
    Ok,
    /// ARIA snapshot summary.
    Snapshot { snapshot: AriaSnapshot },
    /// Page-state snapshot (URL + title).
    State { url: String, title: String },
    /// Network log dump.
    Network { entries: Vec<NetEntry> },
    /// Console log dump.
    Console { entries: Vec<ConsoleEntry> },
    /// Screenshot payload — base64 PNG/JPEG.
    Screenshot { b64: String, format: String },
    /// Read result — markdown body extracted by readability.
    Read { markdown: String },
}

/// Provider-neutral browser backend. Implementations: Camoufox sidecar
/// (PRIMARY), chromiumoxide (FALLBACK, feature-gated), Browserbase
/// (cloud, feature-gated).
///
/// Cancellation: long-running impls MUST race their await against a
/// `tokio_util::sync::CancellationToken` taken from the `ToolContext` —
/// `BrowserTool::execute_with_ctx` plumbs the cancel through to provider
/// methods via the `Cancelled` error path.
#[async_trait]
pub trait BrowserProvider: Send + Sync {
    /// Open a new isolated session (separate cookie jar + tab).
    async fn open_session(
        &self,
        persistent_profile: bool,
    ) -> Result<BrowserSession, BrowserOpError>;

    /// Close a session and release backend resources.
    async fn close_session(&self, ctx: &SessionCtx) -> Result<(), BrowserOpError>;

    /// Dispatch a single op. The tool layer is responsible for `BrowserPolicy`
    /// pre-checks on URL-bearing ops — providers SHOULD assume the URL has
    /// already been allowed.
    async fn dispatch(&self, ctx: &SessionCtx, op: BrowserOp) -> Result<OpResult, BrowserOpError>;

    /// Backend identifier — used for trace / metrics tagging.
    fn backend_name(&self) -> &'static str;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aria::AriaSnapshot;
    use crate::op::BrowserOp;
    use async_trait::async_trait;

    /// Stub `BrowserProvider` impl exercising the trait surface. The
    /// failing-test-first contract from E.1 — if this stops compiling, the
    /// trait surface drifted and we need a deliberate edit.
    struct StubBackend;

    #[async_trait]
    impl BrowserProvider for StubBackend {
        async fn open_session(
            &self,
            persistent_profile: bool,
        ) -> Result<BrowserSession, BrowserOpError> {
            Ok(BrowserSession {
                ctx: SessionCtx::for_test("stub-1"),
                persistent_profile,
            })
        }

        async fn close_session(&self, _ctx: &SessionCtx) -> Result<(), BrowserOpError> {
            Ok(())
        }

        async fn dispatch(
            &self,
            _ctx: &SessionCtx,
            op: BrowserOp,
        ) -> Result<OpResult, BrowserOpError> {
            match op {
                BrowserOp::GetState { .. } => Ok(OpResult::State {
                    url: "about:blank".into(),
                    title: "".into(),
                }),
                BrowserOp::Snapshot { .. } => Ok(OpResult::Snapshot {
                    snapshot: AriaSnapshot::empty(),
                }),
                _ => Ok(OpResult::Ok),
            }
        }

        fn backend_name(&self) -> &'static str {
            "stub"
        }
    }

    #[tokio::test]
    async fn provider_trait_surface_compiles() {
        let backend = StubBackend;
        let session = backend.open_session(false).await.unwrap();
        assert_eq!(session.ctx.session_id, "stub-1");
        let r = backend
            .dispatch(&session.ctx, BrowserOp::GetState {})
            .await
            .unwrap();
        assert!(matches!(r, OpResult::State { .. }));
        backend.close_session(&session.ctx).await.unwrap();
        assert_eq!(backend.backend_name(), "stub");
    }
}
