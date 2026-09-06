//! Owned initialization and turn teardown for ordinary ACP sessions.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use wcore_acp::AcpError;
use wcore_agent::cancel::SessionControl;
use wcore_agent::spawner::HostChildController;

use super::{EngineSession, EngineTurnEngine};

const CANCEL_GRACE: Duration = Duration::from_secs(2);

pub(super) struct Initialization {
    pub(super) closing: AtomicBool,
    pub(super) panicked: AtomicBool,
    pub(super) cleanup: Arc<wcore_agent::bootstrap_cleanup::BootstrapCleanup>,
    pub(super) cancel: CancellationToken,
    pub(super) result: watch::Sender<Option<Result<(), String>>>,
    pub(super) task: Mutex<Option<JoinHandle<()>>>,
}

impl Initialization {
    pub(super) fn new() -> Self {
        let (result, _) = watch::channel(None);
        Self {
            closing: AtomicBool::new(false),
            panicked: AtomicBool::new(false),
            cleanup: Arc::new(wcore_agent::bootstrap_cleanup::BootstrapCleanup::default()),
            cancel: CancellationToken::new(),
            result,
            task: Mutex::new(None),
        }
    }

    pub(super) async fn wait(&self) -> Result<(), AcpError> {
        let mut result = self.result.subscribe();
        loop {
            if let Some(outcome) = result.borrow().clone() {
                return outcome.map_err(AcpError::Protocol);
            }
            result
                .changed()
                .await
                .map_err(|_| AcpError::Cleanup("initializer vanished".into()))?;
        }
    }
}

pub(super) struct InitializationCompletion(pub(super) Arc<Initialization>);

impl Drop for InitializationCompletion {
    fn drop(&mut self) {
        let unpublished = self.0.result.borrow().is_none();
        if unpublished {
            self.0
                .result
                .send_replace(Some(Err("engine initializer did not complete".into())));
        }
    }
}

#[derive(Default)]
pub(super) struct SessionLifetime {
    pub(super) last_used: Arc<std::sync::Mutex<Option<std::time::Instant>>>,
    validation: Mutex<Option<JoinHandle<Result<bool, String>>>>,
    pub(super) closing: Arc<AtomicBool>,
    pub(super) turns: Mutex<Vec<(CancellationToken, JoinHandle<()>)>>,
    pub(super) root: Option<SessionControl>,
    pub(super) children: Option<HostChildController>,
    #[cfg(test)]
    pub(super) channels: Option<Arc<tokio::sync::RwLock<wcore_channels::ChannelManager>>>,
    #[cfg(test)]
    pub(super) inbound_started: bool,
}

impl SessionLifetime {
    async fn validate_journal(
        &self,
        journal: Option<wcore_agent::session_journal::SessionJournal>,
    ) -> Result<(), AcpError> {
        let Some(journal) = journal else {
            return Ok::<(), AcpError>(());
        };
        let mut task = self.validation.lock().await;
        if task.is_none() {
            *task = Some(tokio::task::spawn_blocking(move || {
                wcore_agent::recovery::RecoveryPlan::validate_cleanup(&journal)
                    .map_err(|e| e.to_string())
            }));
        }
        // Retain the handle in the session on timeout. A retry joins this
        // same validation, and its owned journal keeps the writer lease.
        let result = task.as_mut().expect("validation installed").await;
        task.take();
        let needs_recovery = result
            .map_err(|e| AcpError::Cleanup(e.to_string()))?
            .map_err(AcpError::Cleanup)?;
        tracing::debug!(needs_recovery, "ACP committed cleanup authority validated");
        Ok(())
    }
}

impl EngineSession {
    pub(super) async fn close(&self) -> Result<(), AcpError> {
        self.lifetime.closing.store(true, Ordering::Release);
        if let Some(root) = &self.lifetime.root {
            root.cancel();
        }
        let mut turns = self.lifetime.turns.lock().await;
        for (cancel, _) in turns.iter() {
            cancel.cancel();
        }
        let deadline = tokio::time::Instant::now() + CANCEL_GRACE;
        while let Some((_, handle)) = turns.last_mut() {
            if tokio::time::timeout_at(deadline, &mut *handle)
                .await
                .is_err()
            {
                handle.abort();
                let _ = handle.await;
            }
            // Retire each joined handle before the next await. If an outer
            // close deadline interrupts us, retry must not re-poll a completed
            // JoinHandle (Tokio correctly treats that as a programming error).
            turns.pop();
        }
        // The task has returned/been joined, so its finalizer or durable drop
        // path owns the truth. Unknown physical outcomes are retained as such.
        let mut engine = self.engine.lock().await;
        let journal = engine.session_journal().cloned();
        if journal.is_some()
            && let Some(children) = &self.lifetime.children
        {
            let supervisor = children
                .supervisor()
                .map_err(|error| AcpError::Cleanup(error.to_string()))?;
            let children = supervisor
                .list()
                .map_err(|error| AcpError::Cleanup(error.to_string()))?;
            for child in children.iter().filter(|child| !child.status.is_terminal()) {
                supervisor
                    .request_cancel(&child.child_id)
                    .map_err(|error| AcpError::Cleanup(error.to_string()))?;
            }
            // Cancellation is cooperative. Give the child executor time to
            // publish its terminal outcome before judging cleanup incomplete.
            // Keep the session owner on timeout so DELETE can retry safely.
            let child_deadline = tokio::time::Instant::now() + CANCEL_GRACE;
            loop {
                if supervisor
                    .cleanup_ready()
                    .map_err(|error| AcpError::Cleanup(error.to_string()))?
                {
                    break;
                }
                if tokio::time::Instant::now() >= child_deadline {
                    return Err(AcpError::Cleanup(
                        "child cancellation has not completed".into(),
                    ));
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        let cleanup = engine.prepare_shutdown();
        drop(engine);
        let validation = self.lifetime.validate_journal(journal);
        let (validated, cleaned) = tokio::join!(validation, cleanup.close());
        validated?;
        cleaned.map_err(|error| AcpError::Cleanup(error.to_string()))?;
        *self.relay.lock().unwrap() = None;
        Ok(())
    }
}

impl EngineTurnEngine {
    /// Retire only durably reloadable, quiescent engines. Failed cleanup keeps
    /// its counted owner quarantined; admission never frees uncertain capacity.
    pub(super) async fn retire_idle_engines(&self) {
        let mut initializers = self.initializers.lock().await;
        let candidates = self.sessions.lock().await.clone();
        for (id, session) in candidates {
            let Ok(turns) = session.lifetime.turns.try_lock() else {
                continue;
            };
            let old = session
                .lifetime
                .last_used
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_some_and(|time| time.elapsed() >= Duration::from_secs(900));
            if !old || turns.iter().any(|(_, task)| !task.is_finished()) {
                continue;
            }
            let Ok(engine) = session.engine.try_lock() else {
                continue;
            };
            let safe = engine.session_journal().is_some()
                && engine.recovery_plan().is_ok_and(|plan| {
                    matches!(
                        plan.disposition,
                        wcore_agent::recovery::RecoveryDisposition::Ready
                    )
                });
            let children_quiet = session
                .lifetime
                .children
                .as_ref()
                .and_then(|children| children.supervisor().ok())
                .and_then(|supervisor| supervisor.cleanup_ready().ok())
                .unwrap_or(false);
            if !safe || !children_quiet {
                continue;
            }
            session.lifetime.closing.store(true, Ordering::Release);
            drop(engine);
            drop(turns);
            if tokio::time::timeout(Duration::from_secs(10), session.close())
                .await
                .is_ok_and(|result| result.is_ok())
            {
                self.sessions.lock().await.remove(&id);
                initializers.remove(&id);
            }
        }
    }

    pub(super) async fn signal_close(&self, session_id: &str) {
        if let Some(initializer) = self.initializers.lock().await.get(session_id) {
            initializer.closing.store(true, Ordering::Release);
            initializer.cancel.cancel();
        }
        if let Some(session) = self.sessions.lock().await.get(session_id) {
            session.lifetime.closing.store(true, Ordering::Release);
            if let Some(root) = &session.lifetime.root {
                root.cancel();
            }
        }
    }

    pub(super) async fn close_owned_session(&self, session_id: &str) -> Result<(), AcpError> {
        self.signal_close(session_id).await;
        let initializer = self.initializers.lock().await.get(session_id).cloned();
        if let Some(initializer) = &initializer {
            initializer.closing.store(true, Ordering::Release);
            let _initialization_outcome = initializer.wait().await;
            let mut task = initializer.task.lock().await;
            let joined = match task.as_mut() {
                Some(handle) => Some(handle.await),
                None => None,
            };
            // Retain ownership while pending, but retire any completed handle
            // before propagating failure: Tokio forbids polling it twice.
            task.take();
            if let Some(Err(error)) = joined {
                initializer.panicked.store(true, Ordering::Release);
                return Err(AcpError::Cleanup(format!("initializer failed: {error}")));
            }
        }
        let session = self.sessions.lock().await.get(session_id).cloned();
        if let Some(session) = session {
            session.close().await?;
            self.sessions.lock().await.remove(session_id);
            // Drop the final engine owner before reporting lease release.
            drop(session);
        } else if let Some(initializer) = &initializer {
            // A normal bootstrap error still has a registered cleanup owner.
            // A panic can interrupt registration, so it remains explicitly
            // incomplete even after stopping the resources we do know about.
            initializer
                .cleanup
                .close()
                .await
                .map_err(|error| AcpError::Cleanup(error.to_string()))?;
            if initializer.panicked.load(Ordering::Acquire) {
                return Err(AcpError::Cleanup(
                    "bootstrap panicked before cleanup ownership could be verified".into(),
                ));
            }
        }
        self.initializers.lock().await.remove(session_id);
        let no_sessions = self.sessions.lock().await.is_empty();
        let no_initializers = self.initializers.lock().await.is_empty();
        // This is an idle hint, not an admission barrier: trimming is safe if
        // a new session arrives, and no pool lock spans the allocator work.
        if no_sessions && no_initializers {
            wcore_config::allocator::release_idle_memory().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod resource_tests {
    use super::*;
    #[tokio::test]
    async fn stabilization_cleanup_timeout_retains_and_rejoins_same_validation() {
        let root = tempfile::tempdir().unwrap();
        let journal = wcore_agent::session_journal::SessionJournal::open(
            root.path().join("session.journal"),
            "session",
        )
        .unwrap();
        let lifetime = SessionLifetime::default();
        let owned = journal.clone();
        let (release, wait) = tokio::sync::oneshot::channel();
        *lifetime.validation.lock().await = Some(tokio::spawn(async move {
            wait.await.unwrap();
            wcore_agent::recovery::RecoveryPlan::validate_cleanup(&owned).map_err(|e| e.to_string())
        }));
        let first = tokio::time::timeout(
            Duration::from_millis(10),
            lifetime.validate_journal(Some(journal.clone())),
        )
        .await;
        assert!(first.is_err());
        assert!(
            !lifetime
                .validation
                .lock()
                .await
                .as_ref()
                .unwrap()
                .is_finished()
        );
        release.send(()).unwrap();
        lifetime.validate_journal(Some(journal)).await.unwrap();
        assert!(lifetime.validation.lock().await.is_none());
    }

    #[tokio::test]
    async fn stabilization_pool_counts_initializing_and_closing_owners() {
        let root = tempfile::tempdir().unwrap();
        let engine = EngineTurnEngine::new(
            wcore_config::config::Config::default(),
            root.path().to_string_lossy().into_owned(),
        );
        for i in 0..64 {
            engine
                .initializers
                .lock()
                .await
                .insert(format!("pending-{i}"), Arc::new(Initialization::new()));
        }
        let error = engine
            .session_for("overflow", None, &[], &[])
            .await
            .err()
            .expect("64 initializing owners consume capacity");
        assert!(error.to_string().contains("resource_limit"));
        assert_eq!(engine.initializers.lock().await.len(), 64);
        engine
            .initializers
            .lock()
            .await
            .get("pending-0")
            .unwrap()
            .closing
            .store(true, Ordering::Release);
        assert!(
            engine
                .session_for("still-overflow", None, &[], &[])
                .await
                .is_err(),
            "quarantined owner must not free capacity"
        );
    }
}
