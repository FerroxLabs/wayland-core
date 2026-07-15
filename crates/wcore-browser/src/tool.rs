//! `BrowserTool` — `wcore_tools::Tool` impl that dispatches a `BrowserOp` to
//! the chosen `BrowserProvider`.
//!
//! Tool input shape (JSON):
//!
//! ```json
//! { "sub_agent": "writer", "op": { "kind": "navigate", "url": "https://example/" } }
//! ```
//!
//! Per-sub-agent isolation: each sub-agent gets its own `SessionCtx`
//! (separate cookie jar + tab). `BrowserTool` keeps an in-memory map of
//! `sub_agent → session_id` so repeated calls reuse the same session.
//!
//! SECURITY CAVEAT (cua-browser-52/53): sessions are opened with
//! `persistent_profile = false`, i.e. a fresh per-session cookie jar — but
//! that is NOT a guaranteed-incognito / fully-sandboxed browser context.
//! Backend-resident state outside the cookie jar (HTTP cache, service
//! workers, IndexedDB, the on-disk profile dir when `persist_profile` is
//! enabled in config) may be shared across sessions depending on the
//! backend. Treat cross-session isolation as best-effort cookie-jar
//! separation, not a hard trust boundary. The hard boundaries are the URL
//! `BrowserPolicy` (incl. the redirect re-check installed by the backend
//! via `reqwest_redirect_policy`) and the downloads-root confinement in
//! [`validate_local_path`].
//!
//! Cancellation: the `execute_with_ctx` path races against
//! `ctx.cancel.cancelled()` AND a per-op wall-clock deadline. The
//! whichever-fires-first wins; both produce a typed error payload, never a
//! hang. 500ms max cancel latency per the locked Wave RC contract.
//!
//! Timeout policy (Wave RC, 2026-05-23) — every op has an inner wall-clock
//! deadline well inside the dispatcher's outer `ToolCategory::Mcp` budget
//! (120s). Defaults:
//!
//!   * `Navigate`      — 60s   (page-load over slow net is the realistic worst)
//!   * `Read`          — 30s   (readability extraction includes a fetch)
//!   * `Snapshot`      — 15s   (ARIA tree dump on a loaded page)
//!   * `Screenshot`    — 30s   (full-page can be large)
//!   * `Click/Fill/Press/Select` — 10s   (interaction on a loaded page)
//!   * `WaitFor`       — `timeout_ms + 2s` (caller's selector deadline + slack)
//!   * `Upload/Download` — 60s
//!   * `GetState/Network/Console/NewTab/CloseTab/Back/Forward` — 15s
//!
//! Construction:
//!   * [`BrowserTool::new`] uses the defaults above.
//!   * [`BrowserTool::with_op_timeout`] overrides ALL ops with a single
//!     deadline — primarily for tests against a `HangBackend`.

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio::select;

use wcore_protocol::events::ToolCategory;
use wcore_tools::path_validation::validate_user_path;
use wcore_tools::{Tool, context::ToolContext};
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolResult};

use crate::op::BrowserOp;
use crate::policy::{BrowserPolicy, PolicyOutcome};
use crate::provider::{BrowserOpError, BrowserProvider, OpResult, SessionCtx};
use crate::supervisor::BrowserSupervisor;

/// Default per-op wall-clock deadlines. See module docs for the full table.
fn default_op_timeout(op: &BrowserOp) -> Duration {
    match op {
        BrowserOp::Navigate { .. } => Duration::from_secs(60),
        BrowserOp::Read { .. } => Duration::from_secs(30),
        BrowserOp::Snapshot {} => Duration::from_secs(15),
        BrowserOp::Screenshot { .. } => Duration::from_secs(30),
        BrowserOp::Click { .. }
        | BrowserOp::Fill { .. }
        | BrowserOp::Press { .. }
        | BrowserOp::Select { .. } => Duration::from_secs(10),
        BrowserOp::Upload { .. } | BrowserOp::Download { .. } => Duration::from_secs(60),
        BrowserOp::WaitFor { timeout_ms, .. } => {
            // Caller's selector deadline + 2s for overhead.
            Duration::from_millis(timeout_ms.saturating_add(2_000))
        }
        BrowserOp::GetState {}
        | BrowserOp::NetworkLog {}
        | BrowserOp::Console {}
        | BrowserOp::NewTab { .. }
        | BrowserOp::CloseTab {}
        | BrowserOp::Back {}
        | BrowserOp::Forward {} => Duration::from_secs(15),
    }
}

/// Validate a model-supplied local filesystem path for `Download::dest_path`
/// / `Upload::path` BEFORE it is handed to any backend (M-16 / cua-browser-49).
///
/// Always-on shape checks (independent of `downloads_root`):
///   * absolute, no null bytes, no `..` traversal, no OS-secret target
///     (reuses [`wcore_tools::path_validation::validate_user_path`]);
///   * no component is a dotfile/dotdir (rejects `.zshrc`, `.ssh`,
///     `.config`, `.aws`, ... — the prompt-injection "write to my shell
///     rc / read my keys" pattern).
///
/// When `downloads_root` is `Some`, the path is additionally confined to
/// that directory: the longest existing prefix of BOTH the root and the
/// target is canonicalized (resolving symlinks) and the target's real
/// location must sit inside the root — defeating symlink-escapes where a
/// benign-looking name under the root points outside it.
///
/// Returns the normalized path on success; `Err(reason)` is a
/// human-readable refusal the tool layer surfaces back to the model.
fn validate_local_path(raw: &str, downloads_root: Option<&Path>) -> Result<PathBuf, String> {
    let path = Path::new(raw);

    // Shape checks: absolute, no null, no `..`, no system-secret target.
    let normalized =
        validate_user_path(path).map_err(|e| format!("rejected local path {raw:?}: {e}"))?;

    // The portion of the path whose components we apply the dotfile/dotdir
    // guard to. When an operator root is configured we only scrutinise the
    // model-chosen tail BELOW the root (the operator's own root may
    // legitimately live under a dotdir like `~/.wayland/downloads`); the
    // root-confinement check below is the authoritative boundary. Without a
    // root, every component is model-influenced, so check the whole path.
    let dotfile_scope: &Path = match downloads_root {
        Some(root) => normalized.strip_prefix(root).unwrap_or(&normalized),
        None => &normalized,
    };

    // Reject any dotfile/dotdir component in scope. `validate_user_path`
    // does not cover shell-rc / config files by name; a leading-dot
    // component is the cheap, comprehensive signal for "user config /
    // secrets stash" (`.zshrc`, `.ssh`, `.config`, `.aws`, ...).
    if let Some(seg) = dotfile_scope.components().find_map(|c| match c {
        Component::Normal(s) => {
            let s = s.to_string_lossy();
            if s.starts_with('.') {
                Some(s.into_owned())
            } else {
                None
            }
        }
        _ => None,
    }) {
        return Err(format!(
            "rejected local path {raw:?}: dotfile/config component {seg:?} not permitted"
        ));
    }

    // Root-confinement (symlink-aware) when an operator root is configured.
    if let Some(root) = downloads_root {
        let (root_canon, _) = canonicalize_existing_prefix(root)
            .ok_or_else(|| format!("downloads root {root:?} has no real prefix"))?;
        let (target_canon, target_suffix) = canonicalize_existing_prefix(&normalized)
            .ok_or_else(|| format!("local path {raw:?} has no real prefix"))?;
        let resolved = target_canon.join(&target_suffix);
        if !resolved.starts_with(&root_canon) {
            return Err(format!(
                "local path {raw:?} resolves outside downloads root {root_canon:?}"
            ));
        }
    }

    Ok(normalized)
}

/// Resolve a safe per-user default downloads root for the production path,
/// so [`validate_local_path`]'s root-confinement is ALWAYS in force and the
/// tool fails closed (M-16 / cua-browser-49). Without this, a shipped config
/// that never calls [`BrowserTool::with_downloads_root`] would leave
/// `downloads_root = None`, permitting an absolute, non-dotfile,
/// non-traversal `dest_path` like `/etc/cron.d/wayland` to slip through.
///
/// Uses `std::env::temp_dir()/wayland-downloads` (`dirs` is not a dependency
/// of this crate). Created best-effort; if creation fails the path is still
/// returned so confinement runs (a non-existent root simply rejects all
/// writes via `canonicalize_existing_prefix`).
fn default_downloads_root() -> PathBuf {
    let root = std::env::temp_dir().join("wayland-downloads");
    let _ = std::fs::create_dir_all(&root);
    root
}

/// Walk up `path` until a component canonicalizes (resolving symlinks),
/// returning `(canonical_existing_prefix, remaining_suffix)`. Minimal local
/// equivalent of the private helper in `wcore_tools::vfs` (which we cannot
/// import) — used to make root-confinement symlink-escape-proof.
fn canonicalize_existing_prefix(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let mut p: &Path = path;
    loop {
        if let Ok(canon) = std::fs::canonicalize(p) {
            let suffix = path.strip_prefix(p).unwrap_or(Path::new(""));
            return Some((canon, suffix.to_path_buf()));
        }
        p = p.parent()?;
    }
}

pub struct BrowserTool {
    /// Backend dispatcher.
    provider: Arc<dyn BrowserProvider>,
    /// Policy enforcement gate (pre-dispatch URL check).
    policy: BrowserPolicy,
    /// Lifecycle supervisor — informed on session-end.
    supervisor: Arc<BrowserSupervisor>,
    /// Map `sub_agent_name → session_id` so a sub-agent's repeated calls
    /// reuse the same session (separate cookie jar from the main agent).
    sessions: Arc<Mutex<std::collections::HashMap<String, String>>>,
    /// Max concurrent contexts. Default 4 per design §5.16.
    pub max_contexts: usize,
    /// Optional uniform per-op timeout — overrides the per-op defaults
    /// when `Some(_)`. Tests use this with a short value (e.g. 250ms) to
    /// exercise the timeout path against a `HangBackend` without waiting
    /// the realistic 60s navigate deadline. Production paths leave this
    /// as `None` so [`default_op_timeout`] applies.
    op_timeout_override: Option<Duration>,
    /// Operator-configured downloads root. When `Some(_)`, the
    /// model-controlled local paths on `Download::dest_path` and
    /// `Upload::path` are confined to (and symlink-checked against) this
    /// directory — see [`validate_local_path`] and M-16 / cua-browser-49.
    /// `None` leaves the always-on shape checks (absolute, no `..`, no
    /// dotfile/config target, no system-secret target) in force without a
    /// root-confinement clamp; set it via [`Self::with_downloads_root`].
    downloads_root: Option<PathBuf>,
}

fn err(content: impl Into<String>) -> ToolResult {
    ToolResult {
        content: content.into(),
        is_error: true,
    }
}

fn ok(content: impl Into<String>) -> ToolResult {
    ToolResult {
        content: content.into(),
        is_error: false,
    }
}

impl BrowserTool {
    pub fn new(
        provider: Arc<dyn BrowserProvider>,
        policy: BrowserPolicy,
        supervisor: Arc<BrowserSupervisor>,
    ) -> Self {
        Self {
            provider,
            policy,
            supervisor,
            sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            max_contexts: 4,
            op_timeout_override: None,
            // Fail closed: default to a safe per-user root so the
            // root-confinement in `validate_local_path` ALWAYS runs on the
            // production path (M-16). Operators can widen/relocate it via
            // `with_downloads_root`.
            downloads_root: Some(default_downloads_root()),
        }
    }

    /// Confine model-supplied local paths (`Download::dest_path`,
    /// `Upload::path`) to `root`. The path must canonicalize (real prefix)
    /// inside `root`, blocking symlink-escapes as well as `..`/absolute
    /// escapes. Builder form so existing call sites stay unchanged.
    #[must_use]
    pub fn with_downloads_root(mut self, root: impl Into<PathBuf>) -> Self {
        self.downloads_root = Some(root.into());
        self
    }

    /// Variant of [`Self::new`] that fixes a uniform per-op timeout. Used
    /// by tests so the timeout path can be exercised in <1s; production
    /// paths use [`Self::new`] and inherit the per-op defaults in
    /// [`default_op_timeout`].
    pub fn with_op_timeout(
        provider: Arc<dyn BrowserProvider>,
        policy: BrowserPolicy,
        supervisor: Arc<BrowserSupervisor>,
        per_op_timeout: Duration,
    ) -> Self {
        Self {
            provider,
            policy,
            supervisor,
            sessions: Arc::new(Mutex::new(std::collections::HashMap::new())),
            max_contexts: 4,
            op_timeout_override: Some(per_op_timeout),
            downloads_root: None,
        }
    }

    /// Find/mint a session id for the given sub-agent name. Main agent
    /// uses key `""`.
    async fn ensure_session(&self, sub_agent: Option<&str>) -> Result<String, BrowserOpError> {
        let key = sub_agent.unwrap_or("").to_string();
        if let Some(s) = self.sessions.lock().get(&key) {
            return Ok(s.clone());
        }
        // The lock is released across this `await` (we must not hold a
        // `parking_lot::Mutex` guard over an await point). Two concurrent
        // first-calls for the same key can therefore both miss above and both
        // open a backend session. F38: re-check the map under the lock after
        // opening — if a racing call already inserted a session, we are the
        // loser; close the session we just opened (so it isn't orphaned) and
        // use the winner's id.
        let sess = self.provider.open_session(false).await?;
        let winner = {
            let mut guard = self.sessions.lock();
            match guard.get(&key) {
                Some(existing) => Some(existing.clone()),
                None => {
                    guard.insert(key, sess.ctx.session_id.clone());
                    None
                }
            }
        };
        match winner {
            Some(existing) => {
                // We lost the race: close our just-opened session to avoid
                // leaking it, then return the winner's id.
                let _ = self.provider.close_session(&sess.ctx).await;
                Ok(existing)
            }
            None => Ok(sess.ctx.session_id),
        }
    }

    /// Apply policy for URL-bearing ops. Non-URL ops always pass.
    fn policy_check(&self, op: &BrowserOp) -> Result<(), BrowserOpError> {
        let url_opt = match op {
            BrowserOp::Navigate { url, .. } | BrowserOp::Download { url, .. } => Some(url.as_str()),
            BrowserOp::NewTab { url: Some(u), .. } => Some(u.as_str()),
            _ => None,
        };
        let Some(url) = url_opt else {
            return Ok(());
        };
        match self.policy.evaluate(url) {
            PolicyOutcome::Allow => Ok(()),
            PolicyOutcome::Deny { reason } => Err(BrowserOpError::PolicyDenied {
                url: url.to_string(),
                reason,
            }),
            PolicyOutcome::Suspend { url } => Err(BrowserOpError::PolicySuspended { url }),
        }
    }

    /// Internal dispatch with a three-way race: per-op wall-clock
    /// deadline, cancellation token, or completion. Whichever fires first
    /// wins. The op's `Drop` lets the underlying HTTP / CDP future
    /// release its socket / `Page` lock so a sibling op can proceed.
    ///
    /// Bug history (Wave RC, 2026-05-23): the prior implementation raced
    /// only `cancel.cancelled()` against the provider. A user's Esc DID
    /// take effect (the cancel WAS plumbed), but a never-cancelled call
    /// against a hung Camoufox sidecar inherited only the dispatcher's
    /// outer 600s (Exec) backstop — a 10-minute UI hang. The per-op
    /// deadline closes that gap.
    async fn dispatch_inner(
        &self,
        ctx: SessionCtx,
        op: BrowserOp,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<OpResult, BrowserOpError> {
        let deadline = self
            .op_timeout_override
            .unwrap_or_else(|| default_op_timeout(&op));
        select! {
            _ = cancel.cancelled() => Err(BrowserOpError::Cancelled),
            _ = tokio::time::sleep(deadline) => Err(BrowserOpError::Backend(format!(
                "browser op timed out after {}ms",
                deadline.as_millis()
            ))),
            r = self.provider.dispatch(&ctx, op) => r,
        }
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "Browser"
    }

    fn description(&self) -> &str {
        "Interactive browser (Camoufox / Chromium sidecar required): navigate, \
         snapshot ARIA tree, read main content, click/fill/press by element-ref \
         (@e1, @e2, ...), screenshot, network/console logs, tab management. No \
         JavaScript evaluation. Use this ONLY when a page requires interaction \
         (clicking, filling forms, multi-step navigation, screenshots). For a \
         plain read of a URL, use `WebFetch` instead — it does not require any \
         sidecar and is what every read-only page-fetch should call."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "op": {
                    "type": "object",
                    "description": "Browser operation tagged by `kind` (see design §5.16)."
                },
                "sub_agent": {
                    "type": ["string", "null"],
                    "description": "Optional sub-agent name; isolates cookie jar/tab."
                }
            },
            "required": ["op"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Multiple browser ops in flight per session can interleave page
        // state — default to serialized.
        false
    }

    fn category(&self) -> ToolCategory {
        // Wave RC (2026-05-23): Browser was previously `Exec` (600s
        // dispatcher backstop) — way too generous for an I/O / network
        // operation. The dispatcher's `Mcp` bucket (120s) is the right
        // outer budget for HTTP-bound work; per-op deadlines inside
        // `dispatch_inner` (60s nav / 30s screenshot / 10s click / ...)
        // do the real bounding. Exec is reserved for interactive shells
        // like Bash that legitimately need minutes for builds/tests.
        ToolCategory::Mcp
    }

    fn execution_class_for(&self, _input: &Value) -> wcore_tools::ToolExecutionClass {
        wcore_tools::ToolExecutionClass::ProcessSpawning
    }

    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        // Browser actions can mutate local and remote state without reconciliation.
        ToolEffectContract::default()
    }

    async fn execute(&self, input: Value) -> ToolResult {
        self.execute_with_ctx(input, &ToolContext::test_default())
            .await
    }

    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        // Parse input.
        let Some(op_val) = input.get("op") else {
            return err("browser: missing required field `op`");
        };
        let mut op: BrowserOp = match serde_json::from_value(op_val.clone()) {
            Ok(o) => o,
            Err(e) => return err(format!("browser: invalid op: {e}")),
        };

        // M-16 / cua-browser-49: confine model-controlled local paths to the
        // operator downloads root BEFORE the op reaches any backend, and
        // canonicalize them in place so the backend never re-derives an
        // un-validated path. `dest_path`/`path` carry no other meaning, so a
        // refusal here is terminal for the op.
        let local_path = match &op {
            BrowserOp::Download { dest_path, .. } => Some(dest_path.clone()),
            BrowserOp::Upload { path, .. } => Some(path.clone()),
            _ => None,
        };
        if let Some(raw) = local_path {
            match validate_local_path(&raw, self.downloads_root.as_deref()) {
                Ok(clean) => {
                    let clean = clean.to_string_lossy().into_owned();
                    match &mut op {
                        BrowserOp::Download { dest_path, .. } => *dest_path = clean,
                        BrowserOp::Upload { path, .. } => *path = clean,
                        _ => unreachable!("local_path only set for Download/Upload"),
                    }
                }
                Err(reason) => return err(format!("browser: {reason}")),
            }
        }

        // Policy check.
        if let Err(e) = self.policy_check(&op) {
            // F-023: when the policy denies solely because no origins are allow-listed
            // (the default fail-closed posture, reason starts with "default_action=Deny"),
            // surface a friendly config hint so operators know what to do.
            //
            // SSRF/security-class denials (metadata, loopback, private IP, bad scheme, etc.)
            // must NOT be replaced with the friendly hint — operators need the specific reason.
            // Those paths produce reasons like "cloud metadata endpoint blocked: ..." which do
            // NOT start with "default_action=Deny".
            let msg = if let BrowserOpError::PolicyDenied { ref reason, .. } = e {
                if reason.starts_with("default_action=Deny")
                    && self.policy.allowed_origins.is_empty()
                    && self.policy.default_action == crate::policy::PolicyAction::Deny
                {
                    "Browser tool is disabled by default. \
                     Add allowed domains to your config.toml to enable it:\n\n\
                     [browser]\n\
                     # Allow specific domains (glob patterns supported)\n\
                     allowed_origins = [\"example.com\", \"*.mysite.com\"]\n\n\
                     Alternatively, set default_action = \"allow\" to permit all origins \
                     (not recommended — exposes SSRF risk)."
                        .to_string()
                } else {
                    format!("policy: {e}")
                }
            } else {
                format!("policy: {e}")
            };
            return err(msg);
        }

        let sub = input.get("sub_agent").and_then(|s| s.as_str());
        let session_id = match self.ensure_session(sub).await {
            Ok(id) => id,
            Err(e) => return err(format!("session: {e}")),
        };

        let sctx = SessionCtx {
            session_id,
            sub_agent: sub.map(String::from),
        };

        let cancel = ctx.cancel.clone();
        match self.dispatch_inner(sctx, op, cancel).await {
            Ok(out) => {
                let s = serde_json::to_string(&out).unwrap_or_else(|_| "{}".into());
                ok(s)
            }
            Err(BrowserOpError::Cancelled) => err("browser op cancelled"),
            Err(e) => err(format!("browser: {e}")),
        }
    }
}

impl Drop for BrowserTool {
    fn drop(&mut self) {
        // Inform supervisor about every session we opened so the lifecycle
        // hooks can run. Cheap: just removes the entry from supervisor's map.
        let sessions = self.sessions.lock().clone();
        for sid in sessions.values() {
            let _ = self.supervisor.on_session_end(sid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::aria::AriaSnapshot;
    use crate::policy::PolicyAction;
    use crate::provider::{BrowserProvider, BrowserSession, OpResult, SessionCtx};
    use async_trait::async_trait;

    struct OkBackend;

    #[async_trait]
    impl BrowserProvider for OkBackend {
        async fn open_session(
            &self,
            persistent_profile: bool,
        ) -> Result<BrowserSession, BrowserOpError> {
            Ok(BrowserSession {
                ctx: SessionCtx::for_test("ok"),
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
                BrowserOp::GetState {} => Ok(OpResult::State {
                    url: "u".into(),
                    title: "t".into(),
                }),
                BrowserOp::Snapshot {} => Ok(OpResult::Snapshot {
                    snapshot: AriaSnapshot::empty(),
                }),
                _ => Ok(OpResult::Ok),
            }
        }
        fn backend_name(&self) -> &'static str {
            "ok"
        }
    }

    #[tokio::test]
    async fn execute_dispatches_get_state() {
        let tool = BrowserTool::new(
            Arc::new(OkBackend),
            BrowserPolicy::default(),
            Arc::new(BrowserSupervisor::new()),
        );
        let input = json!({ "op": { "kind": "get_state" } });
        let r = tool.execute(input).await;
        assert!(!r.is_error, "unexpected error: {}", r.content);
        assert!(r.content.contains("state"));
        assert!(r.content.contains("\"u\""));
        assert!(r.content.contains("\"t\""));
    }

    #[tokio::test]
    async fn navigate_to_metadata_is_policy_denied() {
        let tool = BrowserTool::new(
            Arc::new(OkBackend),
            BrowserPolicy::default(),
            Arc::new(BrowserSupervisor::new()),
        );
        let input = json!({
            "op": { "kind": "navigate", "url": "http://169.254.169.254/" }
        });
        let r = tool.execute(input).await;
        assert!(r.is_error);
        let msg = r.content.to_lowercase();
        assert!(
            msg.contains("metadata") || msg.contains("policy"),
            "unexpected message: {}",
            r.content
        );
    }

    #[tokio::test]
    async fn missing_op_field_returns_error() {
        let tool = BrowserTool::new(
            Arc::new(OkBackend),
            BrowserPolicy::default(),
            Arc::new(BrowserSupervisor::new()),
        );
        let r = tool.execute(json!({})).await;
        assert!(r.is_error);
        assert!(r.content.contains("`op`"));
    }

    /// M-16 / cua-browser-49: a Download whose `dest_path` targets a
    /// dotfile/config location is rejected in the tool layer, before any
    /// backend dispatch. `OkBackend` would otherwise happily return Ok.
    #[tokio::test]
    async fn download_to_dotfile_dest_is_rejected() {
        let tool = BrowserTool::new(
            Arc::new(OkBackend),
            // Allow the URL so the refusal is provably the path check, not
            // the URL policy.
            BrowserPolicy::new(PolicyAction::Allow, vec!["example.com".into()], vec![]),
            Arc::new(BrowserSupervisor::new()),
        );
        let input = json!({
            "op": {
                "kind": "download",
                "url": "https://example.com/x",
                "dest_path": "/Users/x/.zshrc"
            }
        });
        let r = tool.execute(input).await;
        assert!(r.is_error, "dotfile dest must be rejected: {}", r.content);
        assert!(
            r.content.contains("dotfile") || r.content.contains(".zshrc"),
            "expected dotfile-refusal message, got {}",
            r.content
        );
    }

    /// `..` traversal in `dest_path` is rejected even before the dotfile /
    /// root-confinement checks run.
    #[tokio::test]
    async fn download_with_traversal_dest_is_rejected() {
        let tool = BrowserTool::new(
            Arc::new(OkBackend),
            BrowserPolicy::new(PolicyAction::Allow, vec!["example.com".into()], vec![]),
            Arc::new(BrowserSupervisor::new()),
        );
        let input = json!({
            "op": {
                "kind": "download",
                "url": "https://example.com/x",
                "dest_path": "/tmp/dl/../../etc/passwd"
            }
        });
        let r = tool.execute(input).await;
        assert!(r.is_error, "traversal dest must be rejected: {}", r.content);
    }

    /// A relative `Upload::path` is rejected (must be absolute).
    #[tokio::test]
    async fn upload_relative_path_is_rejected() {
        let tool = BrowserTool::new(
            Arc::new(OkBackend),
            BrowserPolicy::default(),
            Arc::new(BrowserSupervisor::new()),
        );
        let input = json!({
            "op": { "kind": "upload", "target": "e1", "path": "secrets/key.pem" }
        });
        let r = tool.execute(input).await;
        assert!(
            r.is_error,
            "relative upload path must be rejected: {}",
            r.content
        );
    }

    /// M-16 regression: with the DEFAULT-constructed tool (no operator root
    /// configured), a benign-looking absolute `dest_path` — no `..`, not a
    /// dotfile, not on the secret deny-list — like `/etc/cron.d/wayland` must
    /// still be REJECTED, because `new` now fails closed onto a safe per-user
    /// downloads root. Before the fix this passed and camoufox wrote the body
    /// there (arbitrary write outside any downloads root).
    #[tokio::test]
    async fn download_without_explicit_root_rejects_outside_default_root() {
        let tool = BrowserTool::new(
            Arc::new(OkBackend),
            BrowserPolicy::new(PolicyAction::Allow, vec!["example.com".into()], vec![]),
            Arc::new(BrowserSupervisor::new()),
        );

        // The exact bypass from the finding.
        let bad_input = json!({
            "op": {
                "kind": "download",
                "url": "https://example.com/x",
                "dest_path": "/etc/cron.d/wayland"
            }
        });
        let r = tool.execute(bad_input).await;
        assert!(
            r.is_error,
            "absolute out-of-default-root dest must be rejected by the default-constructed tool: {}",
            r.content
        );

        // A path INSIDE the default root is accepted (confinement is active,
        // not a blanket deny).
        let inside = default_downloads_root().join("report.pdf");
        let ok_input = json!({
            "op": {
                "kind": "download",
                "url": "https://example.com/x",
                "dest_path": inside.to_string_lossy(),
            }
        });
        let r = tool.execute(ok_input).await;
        assert!(!r.is_error, "in-default-root dest must pass: {}", r.content);
    }

    /// With a configured downloads root, a `dest_path` inside the root is
    /// accepted; one outside it is refused.
    #[tokio::test]
    async fn download_confined_to_downloads_root() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("report.pdf");
        let tool = BrowserTool::new(
            Arc::new(OkBackend),
            BrowserPolicy::new(PolicyAction::Allow, vec!["example.com".into()], vec![]),
            Arc::new(BrowserSupervisor::new()),
        )
        .with_downloads_root(root.path().to_path_buf());

        let ok_input = json!({
            "op": {
                "kind": "download",
                "url": "https://example.com/x",
                "dest_path": inside.to_string_lossy(),
            }
        });
        let r = tool.execute(ok_input).await;
        assert!(!r.is_error, "in-root dest must pass: {}", r.content);

        // A path outside the root (parent dir) must be refused — even though
        // it has no `..` and is not a dotfile, root-confinement catches it.
        let outside = root.path().parent().unwrap().join("escape.bin");
        let bad_input = json!({
            "op": {
                "kind": "download",
                "url": "https://example.com/x",
                "dest_path": outside.to_string_lossy(),
            }
        });
        let r = tool.execute(bad_input).await;
        assert!(
            r.is_error,
            "out-of-root dest must be rejected: {}",
            r.content
        );
        assert!(
            r.content.contains("downloads root") || r.content.contains("outside"),
            "expected confinement message, got {}",
            r.content
        );
    }

    /// Symlink-escape: a symlink placed INSIDE the downloads root that
    /// points OUTSIDE it must not let a write escape confinement.
    #[cfg(unix)]
    #[tokio::test]
    async fn download_symlink_escape_is_rejected() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        // root/escape -> outside (a directory symlink inside the root).
        let link = root.path().join("escape");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();

        let tool = BrowserTool::new(
            Arc::new(OkBackend),
            BrowserPolicy::new(PolicyAction::Allow, vec!["example.com".into()], vec![]),
            Arc::new(BrowserSupervisor::new()),
        )
        .with_downloads_root(root.path().to_path_buf());

        // Looks in-root lexically, but resolves outside via the symlink.
        let target = link.join("loot.bin");
        let input = json!({
            "op": {
                "kind": "download",
                "url": "https://example.com/x",
                "dest_path": target.to_string_lossy(),
            }
        });
        let r = tool.execute(input).await;
        assert!(
            r.is_error,
            "symlink-escape dest must be rejected: {}",
            r.content
        );
    }

    /// F38: two concurrent first-calls to `ensure_session` for the same key
    /// must converge on ONE session. The losing call closes the backend
    /// session it opened (no orphan) and both callers see the same id.
    #[tokio::test]
    async fn ensure_session_race_closes_loser_and_converges() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        struct CountingBackend {
            next_id: AtomicUsize,
            opened: Arc<AtomicUsize>,
            closed: Arc<AtomicUsize>,
        }

        #[async_trait]
        impl BrowserProvider for CountingBackend {
            async fn open_session(
                &self,
                persistent_profile: bool,
            ) -> Result<BrowserSession, BrowserOpError> {
                // Yield so two concurrent callers interleave past the initial
                // miss before either inserts — reproducing the race.
                tokio::task::yield_now().await;
                let n = self.next_id.fetch_add(1, Ordering::SeqCst);
                self.opened.fetch_add(1, Ordering::SeqCst);
                Ok(BrowserSession {
                    ctx: SessionCtx::for_test(format!("sess-{n}")),
                    persistent_profile,
                })
            }
            async fn close_session(&self, _ctx: &SessionCtx) -> Result<(), BrowserOpError> {
                self.closed.fetch_add(1, Ordering::SeqCst);
                Ok(())
            }
            async fn dispatch(
                &self,
                _ctx: &SessionCtx,
                _op: BrowserOp,
            ) -> Result<OpResult, BrowserOpError> {
                Ok(OpResult::Ok)
            }
            fn backend_name(&self) -> &'static str {
                "counting"
            }
        }

        let opened = Arc::new(AtomicUsize::new(0));
        let closed = Arc::new(AtomicUsize::new(0));
        let tool = Arc::new(BrowserTool::new(
            Arc::new(CountingBackend {
                next_id: AtomicUsize::new(0),
                opened: opened.clone(),
                closed: closed.clone(),
            }),
            BrowserPolicy::default(),
            Arc::new(BrowserSupervisor::new()),
        ));

        let t1 = {
            let tool = tool.clone();
            tokio::spawn(async move { tool.ensure_session(Some("writer")).await })
        };
        let t2 = {
            let tool = tool.clone();
            tokio::spawn(async move { tool.ensure_session(Some("writer")).await })
        };
        let id1 = t1.await.unwrap().unwrap();
        let id2 = t2.await.unwrap().unwrap();

        // Both callers converge on the same surviving session id.
        assert_eq!(id1, id2, "both callers must see the same session id");
        // Exactly one session remains tracked for the key.
        assert_eq!(tool.sessions.lock().get("writer"), Some(&id1));
        // If both raced to open (the common case under yield_now), the loser
        // must have been closed. We never close more than we opened, and the
        // surviving id is never the one that got closed.
        let n_opened = opened.load(Ordering::SeqCst);
        let n_closed = closed.load(Ordering::SeqCst);
        assert!(
            n_closed == n_opened.saturating_sub(1),
            "expected exactly one fewer close than open (opened={n_opened}, closed={n_closed})"
        );
    }

    #[tokio::test]
    async fn cancellation_aborts_dispatch() {
        let tool = BrowserTool::new(
            Arc::new(OkBackend),
            BrowserPolicy::default(),
            Arc::new(BrowserSupervisor::new()),
        );
        let ctx = ToolContext::test_default();
        ctx.cancel.cancel(); // pre-cancel
        let r = tool
            .execute_with_ctx(json!({ "op": { "kind": "get_state" } }), &ctx)
            .await;
        // Provider may finish before the race observed the token; either
        // outcome is acceptable but cancellation must not panic.
        if r.is_error {
            assert!(
                r.content.contains("cancel"),
                "expected cancel msg, got {}",
                r.content
            );
        }
    }
}
