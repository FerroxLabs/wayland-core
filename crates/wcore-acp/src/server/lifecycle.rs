//! Session admission and owned close completion, independent of HTTP callers.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, Notify, OwnedRwLockReadGuard, RwLock, watch};

use super::{AcpError, AcpServer};

pub(super) const CLOSE_DEADLINE: Duration = Duration::from_secs(10);
type CloseResult = Option<Result<(), String>>;

#[derive(Debug, Default)]
pub(super) struct SessionLifecycle {
    closing: AtomicBool,
    close_requested: Notify,
    admission: Arc<RwLock<()>>,
    close: Mutex<Option<watch::Receiver<CloseResult>>>,
    streams: AtomicUsize,
    turn_slots: std::sync::OnceLock<Arc<tokio::sync::Semaphore>>,
    drained: Notify,
}

impl SessionLifecycle {
    pub(super) async fn closed(&self) {
        loop {
            let notified = self.close_requested.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.closing.load(Ordering::Acquire) {
                return;
            }
            notified.await;
        }
    }
    pub(super) async fn admit(&self) -> Result<OwnedRwLockReadGuard<()>, AcpError> {
        let permit = self.admission.clone().read_owned().await;
        if self.closing.load(Ordering::Acquire) {
            return Err(AcpError::Cleanup("session is closing".to_string()));
        }
        Ok(permit)
    }

    pub(super) async fn admit_turn(
        &self,
    ) -> Result<(OwnedRwLockReadGuard<()>, tokio::sync::OwnedSemaphorePermit), AcpError> {
        let admission = self.admit().await?;
        let slot = self
            .turn_slots
            .get_or_init(|| Arc::new(tokio::sync::Semaphore::new(9)))
            .clone()
            .try_acquire_owned()
            .map_err(|_| AcpError::Protocol("resource_limit:8 pending turns per session".into()))?;
        Ok((admission, slot))
    }

    pub(super) fn stream(self: &Arc<Self>) -> RecordingGuard {
        self.streams.fetch_add(1, Ordering::AcqRel);
        RecordingGuard(self.clone())
    }

    async fn await_recording(&self) {
        loop {
            let notified = self.drained.notified();
            // Register before observing the count, avoiding a lost last drop.
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.streams.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }

    pub(super) async fn begin_close(
        self: &Arc<Self>,
        server: AcpServer,
        session_id: String,
        profile: bool,
    ) -> watch::Receiver<CloseResult> {
        let mut state = self.close.lock().await;
        if let Some(existing) = state.as_ref() {
            let retry = match &*existing.borrow() {
                Some(Err(_)) => true,
                // Panic/abort may close the channel without publishing. The
                // admission flag stays closed, but cleanup must be retryable.
                None => existing.has_changed().is_err(),
                Some(Ok(())) => false,
            };
            if !retry {
                return existing.clone();
            }
        }
        self.closing.store(true, Ordering::Release);
        self.close_requested.notify_waiters();
        let (tx, rx) = watch::channel(None);
        *state = Some(rx.clone());
        let lifecycle = self.clone();
        let deadline = tokio::time::Instant::now() + CLOSE_DEADLINE;
        tokio::spawn(async move {
            // A request owns admission only until its upstream is established
            // and its recorder registered. The running turn is owned by Core.
            let result = tokio::time::timeout_at(deadline, async {
                if !profile && let Some(engine) = &server.turn_engine {
                    engine.request_close(&session_id).await?;
                }
                let _admission = lifecycle.admission.write().await;
                if profile {
                    let router = server.router.as_ref().ok_or_else(|| {
                        AcpError::Cleanup("profile supervisor is unavailable".to_string())
                    })?;
                    router.delete(&session_id).await?;
                } else if let Some(engine) = &server.turn_engine {
                    engine.close_session(&session_id).await?;
                }
                lifecycle.await_recording().await;
                let mut sessions = server.sessions.write().await;
                let mut events = server.events.write().await;
                sessions.remove(&session_id);
                if let Some(log) = events.remove(&session_id) {
                    // Recording has finished, so this is the log's final size;
                    // its history leaves the cross-session total with it.
                    let retained = super::lock_log(&log).retained_bytes();
                    super::account_retained(&server.retained_total, retained, 0);
                }
                Ok::<(), AcpError>(())
            })
            .await
            .unwrap_or_else(|_| {
                Err(AcpError::Cleanup(
                    "owned cleanup deadline exceeded; retry DELETE".into(),
                ))
            });
            tx.send_replace(Some(result.map_err(|error| error.to_string())));
        });
        rx
    }
}

pub(super) async fn wait_for_close(mut rx: watch::Receiver<CloseResult>) -> Result<(), AcpError> {
    loop {
        if let Some(result) = rx.borrow().clone() {
            return result.map_err(AcpError::Cleanup);
        }
        rx.changed().await.map_err(|_| {
            AcpError::Cleanup("session cleanup task did not publish completion".to_string())
        })?;
    }
}

impl AcpServer {
    pub(super) async fn begin_session_close(
        &self,
        session_id: String,
    ) -> Result<watch::Receiver<CloseResult>, AcpError> {
        let record = self
            .sessions
            .read()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| AcpError::Session(format!("session not found: {session_id}")))?;
        Ok(record
            .lifecycle
            .begin_close(
                self.clone(),
                session_id,
                Self::is_profile_agent(record.agent.as_deref()),
            )
            .await)
    }
}

pub(super) struct RecordingGuard(Arc<SessionLifecycle>);

impl Drop for RecordingGuard {
    fn drop(&mut self) {
        self.0.streams.fetch_sub(1, Ordering::AcqRel);
        self.0.drained.notify_waiters();
    }
}
