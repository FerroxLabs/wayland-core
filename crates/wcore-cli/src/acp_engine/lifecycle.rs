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
    pub(super) closing: Arc<AtomicBool>,
    pub(super) turns: Mutex<Vec<(CancellationToken, JoinHandle<()>)>>,
    pub(super) root: Option<SessionControl>,
    pub(super) children: Option<HostChildController>,
    #[cfg(test)]
    pub(super) channels: Option<Arc<tokio::sync::RwLock<wcore_channels::ChannelManager>>>,
    #[cfg(test)]
    pub(super) inbound_started: bool,
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
        if engine.session_journal().is_some() {
            engine
                .recovery_plan()
                .map_err(|error| AcpError::Cleanup(error.to_string()))?;
            if let Some(children) = &self.lifetime.children {
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
                        .list()
                        .map_err(|error| AcpError::Cleanup(error.to_string()))?
                        .iter()
                        .all(|child| child.status.is_terminal())
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
        }
        let cleanup = engine.prepare_shutdown();
        drop(engine);
        cleanup
            .close()
            .await
            .map_err(|error| AcpError::Cleanup(error.to_string()))?;
        *self.relay.lock().unwrap() = None;
        Ok(())
    }
}

impl EngineTurnEngine {
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
        Ok(())
    }
}
