//! W8a A.2 — cooperative cancellation primitives.
//!
//! Thin re-export of `tokio_util::sync::CancellationToken` with helpers
//! used by `ToolContext.cancel` (A.3) and by `bash`/`script`/`mcp` tools
//! that race `ctx.cancel.cancelled()` against their long work (A.4).
//!
//! Wave RC (audit MAJOR #8) — [`budget_linked`] /
//! [`budget_linked_with_callback`] return a [`BudgetGuard`] RAII handle
//! that aborts the spawned 50ms-poll task on drop. Previously the
//! watcher task was self-documented as leaking for the lifetime of the
//! session; at the dozen-tasks scale of a single wayland-core process
//! that was tolerable, but a host that recycles sessions thousands of
//! times per hour (e.g. wayland Electron running many short-lived
//! protocol streams) would accumulate idle pollers. The guard makes
//! the lifetime explicit: when the caller drops the guard, the watcher
//! is aborted and the underlying token reference is released.

use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};
use std::time::Duration;

use tokio::task::JoinHandle;
pub use tokio_util::sync::CancellationToken;

use crate::budget::ExecutionBudgetView;

/// Why a session cancellation root became terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTerminationReason {
    Cancelled,
    DangerousLeaseExpired,
    BudgetExceeded,
    EngineDropped,
}

impl SessionTerminationReason {
    pub fn error_code(self) -> &'static str {
        match self {
            Self::Cancelled => "session_cancelled",
            Self::DangerousLeaseExpired => "dangerous_lease_expired",
            Self::BudgetExceeded => "session_budget_exceeded",
            Self::EngineDropped => "session_engine_dropped",
        }
    }

    pub fn user_message(self) -> &'static str {
        match self {
            Self::Cancelled => "session was cancelled; start a new session",
            Self::DangerousLeaseExpired => {
                "Dangerous session authority expired; start a new session"
            }
            Self::BudgetExceeded => "session execution budget was exceeded; start a new session",
            Self::EngineDropped => "session engine shut down",
        }
    }
}

/// Cloneable, read-only view of a session's terminal reason.
#[derive(Clone)]
pub struct SessionTermination {
    reason: Arc<AtomicU8>,
}

/// Host-facing session lifetime handle. Observation clones cannot revive the
/// root; explicit host cancellation records its typed reason before firing.
#[derive(Clone)]
pub struct SessionControl {
    root: CancellationToken,
    termination: SessionTermination,
}

impl SessionControl {
    fn new(root: CancellationToken, termination: SessionTermination) -> Self {
        Self { root, termination }
    }

    /// Return a descendant suitable for observation and turn-scoped work.
    /// Cancelling the returned token cannot cancel the session root.
    pub fn child_token(&self) -> CancellationToken {
        self.root.child_token()
    }

    pub fn is_cancelled(&self) -> bool {
        self.root.is_cancelled()
    }

    pub fn termination(&self) -> SessionTermination {
        self.termination.clone()
    }

    pub fn cancel(&self) {
        self.termination.mark(SessionTerminationReason::Cancelled);
        self.root.cancel();
    }
}

impl SessionTermination {
    fn new() -> Self {
        Self {
            reason: Arc::new(AtomicU8::new(0)),
        }
    }

    pub fn reason(&self) -> Option<SessionTerminationReason> {
        match self.reason.load(Ordering::Acquire) {
            1 => Some(SessionTerminationReason::Cancelled),
            2 => Some(SessionTerminationReason::DangerousLeaseExpired),
            3 => Some(SessionTerminationReason::BudgetExceeded),
            4 => Some(SessionTerminationReason::EngineDropped),
            _ => None,
        }
    }

    pub fn reason_or_cancelled(&self) -> SessionTerminationReason {
        self.reason().unwrap_or(SessionTerminationReason::Cancelled)
    }

    pub(crate) fn mark(&self, reason: SessionTerminationReason) {
        let code = match reason {
            SessionTerminationReason::Cancelled => 1,
            SessionTerminationReason::DangerousLeaseExpired => 2,
            SessionTerminationReason::BudgetExceeded => 3,
            SessionTerminationReason::EngineDropped => 4,
        };
        let _ = self
            .reason
            .compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire);
    }
}

/// Cloneable observation handle for one session's immutable cancellation root
/// and replaceable active-turn token. Child spawners can read the current turn
/// without gaining authority to mutate the session root.
#[derive(Clone)]
pub(crate) struct SessionRuntimeHandle {
    root: CancellationToken,
    active_turn: std::sync::Arc<parking_lot::RwLock<CancellationToken>>,
    termination: SessionTermination,
}

impl SessionRuntimeHandle {
    pub(crate) fn new(root: CancellationToken) -> Self {
        Self {
            active_turn: std::sync::Arc::new(parking_lot::RwLock::new(root.clone())),
            root,
            termination: SessionTermination::new(),
        }
    }

    pub(crate) fn root_token(&self) -> CancellationToken {
        self.root.clone()
    }

    pub(crate) fn active_turn_token(&self) -> CancellationToken {
        self.active_turn.read().clone()
    }

    pub(crate) fn termination(&self) -> SessionTermination {
        self.termination.clone()
    }

    pub(crate) fn control(&self) -> SessionControl {
        SessionControl::new(self.root.clone(), self.termination.clone())
    }

    pub(crate) fn set_active_turn(&self, token: CancellationToken) {
        *self.active_turn.write() = token;
    }
}

/// Engine-owned lifetime authority for a session.
///
/// Keeps the budget watcher intact, owns the root-to-turn bridge, and makes
/// dropping the engine terminal for every clone of the session root.
pub(crate) struct SessionRuntimeGuard {
    handle: SessionRuntimeHandle,
    budget_guard: Option<BudgetGuard>,
    turn_bridge: Option<JoinHandle<()>>,
    dangerous_deadline: Option<tokio::time::Instant>,
    dangerous_expiry: Option<JoinHandle<()>>,
}

impl SessionRuntimeGuard {
    pub(crate) fn new(handle: SessionRuntimeHandle) -> Self {
        Self {
            handle,
            budget_guard: None,
            turn_bridge: None,
            dangerous_deadline: None,
            dangerous_expiry: None,
        }
    }

    pub(crate) fn root_token(&self) -> CancellationToken {
        self.handle.root_token()
    }

    pub(crate) fn attach_budget_guard(&mut self, guard: BudgetGuard) {
        if let Some(previous) = self.budget_guard.replace(guard) {
            drop(previous);
        }
    }

    pub(crate) fn set_active_turn(&mut self, token: CancellationToken) {
        if let Some(handle) = self.turn_bridge.take() {
            handle.abort();
        }
        self.handle.set_active_turn(token.clone());
        let root = self.handle.root_token();
        if root.is_cancelled() {
            token.cancel();
            return;
        }
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            self.turn_bridge = Some(runtime.spawn(async move {
                tokio::select! {
                    _ = root.cancelled() => token.cancel(),
                    _ = token.cancelled() => {}
                }
            }));
        } else {
            // A bootstrapped engine without a runtime cannot maintain the
            // session-to-turn safety link. Refuse the turn rather than
            // silently installing a token expiry cannot reach.
            token.cancel();
        }
    }

    /// Arm the resolver-produced Dangerous lease against Tokio's monotonic
    /// clock. Expiry permanently cancels the immutable session root.
    pub(crate) fn arm_dangerous_lease(
        &mut self,
        grant: &wcore_types::execution_policy::DangerousSessionGrant,
    ) -> Result<(), wcore_types::execution_policy::ExecutionPolicyError> {
        if let Some(handle) = self.dangerous_expiry.take() {
            handle.abort();
        }
        let remaining = grant
            .remaining_ttl()
            .ok_or(wcore_types::execution_policy::ExecutionPolicyError::DangerousGrantExpired)?;
        let deadline = tokio::time::Instant::now()
            .checked_add(remaining)
            .ok_or(wcore_types::execution_policy::ExecutionPolicyError::DangerousExpiryOverflow)?;
        let root = self.handle.root_token();
        let termination = self.handle.termination();
        self.dangerous_deadline = Some(deadline);
        self.dangerous_expiry = Some(tokio::spawn(async move {
            tokio::time::sleep_until(deadline).await;
            termination.mark(SessionTerminationReason::DangerousLeaseExpired);
            root.cancel();
        }));
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn dangerous_deadline(&self) -> Option<tokio::time::Instant> {
        self.dangerous_deadline
    }
}

impl Drop for SessionRuntimeGuard {
    fn drop(&mut self) {
        self.handle
            .termination()
            .mark(SessionTerminationReason::EngineDropped);
        self.handle.root_token().cancel();
        if let Some(handle) = self.turn_bridge.take() {
            handle.abort();
        }
        if let Some(handle) = self.dangerous_expiry.take() {
            handle.abort();
        }
    }
}

/// Build a child token that fires when the parent (or any ancestor) fires.
/// Wraps `CancellationToken::child_token()` for callers that don't want
/// to depend on `tokio_util` directly.
pub fn child_of(parent: &CancellationToken) -> CancellationToken {
    parent.child_token()
}

/// RAII handle returned by [`budget_linked`] / [`budget_linked_with_callback`].
///
/// Wraps the linked [`CancellationToken`] plus a [`JoinHandle`] for the
/// spawned watcher task. Dropping the guard aborts the watcher (closing
/// audit MAJOR #8 — previously the task could outlive the caller and
/// leak per-session). `Deref<Target=CancellationToken>` keeps the old
/// `is_cancelled()` / `cancel()` / `cancelled()` ergonomics so call
/// sites that treated the return as a token still compile.
#[must_use = "dropping a BudgetGuard aborts the watcher task immediately; bind it to a name"]
pub struct BudgetGuard {
    token: CancellationToken,
    /// `Option` so `Drop` can `.take()` the handle and abort it. After
    /// drop the field is `None`.
    handle: Option<JoinHandle<()>>,
}

impl BudgetGuard {
    /// Borrow the underlying [`CancellationToken`]. Equivalent to the
    /// `Deref` impl; provided for callers that prefer an explicit name.
    pub fn token(&self) -> &CancellationToken {
        &self.token
    }

    /// Clone the underlying token. The clone outlives the guard
    /// (tokens are `Arc`-backed); a clone is safe to pass to tools
    /// that need to observe cancellation after the guard is dropped.
    pub fn token_clone(&self) -> CancellationToken {
        self.token.clone()
    }

    /// Cancel the linked token (without dropping the guard).
    pub fn cancel(&self) {
        self.token.cancel();
    }

    /// `true` if the linked token has fired (cap tripped, caller
    /// cancelled, or parent fired).
    pub fn is_cancelled(&self) -> bool {
        self.token.is_cancelled()
    }

    /// Wait for cancellation. Mirrors `CancellationToken::cancelled`.
    pub async fn cancelled(&self) {
        self.token.cancelled().await
    }
}

impl Deref for BudgetGuard {
    type Target = CancellationToken;

    fn deref(&self) -> &Self::Target {
        &self.token
    }
}

impl Drop for BudgetGuard {
    fn drop(&mut self) {
        // Abort the watcher task. The task already checks `is_cancelled()`
        // on every poll iteration and returns naturally, so the abort
        // is best-effort cleanup — it covers the case where the watcher
        // is mid-sleep when the guard goes out of scope.
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
        // Also cancel the token so any clones still observed by downstream
        // tooling immediately see the parent session has ended. Without
        // this, a tool holding `token_clone()` could hang in `cancelled()`
        // until its own timeout, even though the session is over.
        self.token.cancel();
    }
}

/// Pair a token with a budget watcher: returns a [`BudgetGuard`] whose
/// inner token fires when either the parent fires OR
/// `budget.is_exceeded()` flips true.
///
/// Spawns a tokio task that polls the budget every 50ms. The watcher
/// terminates on cancellation; in addition, dropping the returned
/// [`BudgetGuard`] aborts the task explicitly (Wave RC audit MAJOR #8
/// fix).
pub fn budget_linked(parent: CancellationToken, budget: ExecutionBudgetView) -> BudgetGuard {
    budget_linked_with_callback(parent, budget, |_| {})
}

/// W8a A.7: budget-linked cancel with a one-shot `on_exceeded` callback
/// fired the instant the watcher observes the first cap trip. Used by
/// bootstrap to emit `ProtocolEvent::BudgetExceeded { reason, observed,
/// limit }` via `OutputSink::emit_budget_exceeded` without coupling the
/// watcher to wcore-protocol or to a specific sink type.
///
/// The callback runs in the watcher's tokio task, gets called at most
/// once per session, and receives the `(reason, observed, limit)`
/// snapshot derived from `ExecutionBudgetView::observed_for` /
/// `limit_for`.
///
/// Returns a [`BudgetGuard`]; dropping the guard aborts the watcher
/// (Wave RC, audit MAJOR #8).
pub fn budget_linked_with_callback<F>(
    parent: CancellationToken,
    budget: ExecutionBudgetView,
    on_exceeded: F,
) -> BudgetGuard
where
    F: FnOnce(BudgetTripPayload) + Send + 'static,
{
    budget_guard_for_token_with_callback(parent.child_token(), budget, on_exceeded)
}

/// Attach a budget watcher to an existing session token.
///
/// Unlike [`budget_linked_with_callback`], this does not mint a child token.
/// Bootstrap uses it so the engine and every child spawner share one exact
/// session cancellation root before the budget watcher is installed.
pub(crate) fn budget_guard_for_token_with_callback<F>(
    token: CancellationToken,
    budget: ExecutionBudgetView,
    on_exceeded: F,
) -> BudgetGuard
where
    F: FnOnce(BudgetTripPayload) + Send + 'static,
{
    let watcher = token.clone();
    let handle = tokio::spawn(async move {
        let mut cb = Some(on_exceeded);
        loop {
            if watcher.is_cancelled() {
                return;
            }
            if let Some(reason) = budget.first_exceeded_reason() {
                if let Some(callback) = cb.take() {
                    callback(BudgetTripPayload {
                        reason: reason.to_string(),
                        observed: budget.observed_for(reason),
                        limit: budget.limit_for(reason),
                    });
                }
                watcher.cancel();
                return;
            }
            tokio::select! {
                _ = watcher.cancelled() => return,
                _ = tokio::time::sleep(Duration::from_millis(50)) => {}
            }
        }
    });
    BudgetGuard {
        token,
        handle: Some(handle),
    }
}

/// Snapshot of the cap that tripped, passed to the
/// `budget_linked_with_callback` on-exceeded hook so the caller can
/// emit `BudgetExceeded { reason, observed, limit }` without re-reading
/// the budget state.
#[derive(Debug, Clone)]
pub struct BudgetTripPayload {
    pub reason: String,
    pub observed: String,
    pub limit: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_reason_is_typed_and_first_writer_wins() {
        let termination = SessionTermination::new();
        termination.mark(SessionTerminationReason::DangerousLeaseExpired);
        termination.mark(SessionTerminationReason::EngineDropped);

        assert_eq!(
            termination.reason(),
            Some(SessionTerminationReason::DangerousLeaseExpired)
        );
        assert_eq!(
            termination.reason_or_cancelled().user_message(),
            "Dangerous session authority expired; start a new session"
        );
        assert_eq!(
            termination.reason_or_cancelled().error_code(),
            "dangerous_lease_expired"
        );
    }

    #[test]
    fn host_control_marks_cancellation_before_firing_root() {
        let root = CancellationToken::new();
        let termination = SessionTermination::new();
        let control = SessionControl::new(root.clone(), termination.clone());

        control.cancel();

        assert!(root.is_cancelled());
        assert_eq!(
            termination.reason(),
            Some(SessionTerminationReason::Cancelled)
        );
    }

    #[test]
    fn host_child_cannot_cancel_session_root() {
        let root = CancellationToken::new();
        let termination = SessionTermination::new();
        let control = SessionControl::new(root.clone(), termination);

        control.child_token().cancel();

        assert!(!root.is_cancelled());
        assert!(!control.is_cancelled());
    }
}
