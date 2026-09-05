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
use wcore_mcp::manager::McpManager;

use super::{EngineSession, EngineTurnEngine};

const CANCEL_GRACE: Duration = Duration::from_secs(2);

pub(super) struct Initialization {
    pub(super) closing: AtomicBool,
    pub(super) cancel: CancellationToken,
    pub(super) result: watch::Sender<Option<Result<(), String>>>,
    pub(super) task: Mutex<Option<JoinHandle<()>>>,
}

impl Initialization {
    pub(super) fn new() -> Self {
        let (result, _) = watch::channel(None);
        Self {
            closing: AtomicBool::new(false),
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
    pub(super) mcp: Vec<Arc<McpManager>>,
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
        let engine = self.engine.lock().await;
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
                if supervisor
                    .list()
                    .map_err(|error| AcpError::Cleanup(error.to_string()))?
                    .iter()
                    .any(|child| !child.status.is_terminal())
                {
                    return Err(AcpError::Cleanup(
                        "child cancellation has not completed".into(),
                    ));
                }
            }
        }
        drop(engine);
        // Attempt every manager/server even if one cannot establish cleanup.
        // Retain the managers on error so another DELETE can retry them.
        let results =
            futures::future::join_all(self.lifetime.mcp.iter().map(|manager| async move {
                futures::future::join_all(
                    manager
                        .server_names()
                        .into_iter()
                        .map(|name| async move { manager.close_server(&name).await }),
                )
                .await
            }))
            .await;
        let errors: Vec<_> = results
            .into_iter()
            .flatten()
            .filter_map(Result::err)
            .collect();
        if !errors.is_empty() {
            return Err(AcpError::Cleanup(
                errors
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("; "),
            ));
        }
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
        let mut initialization_error = None;
        if let Some(initializer) = &initializer {
            initializer.closing.store(true, Ordering::Release);
            initialization_error = initializer.wait().await.err();
            let mut task = initializer.task.lock().await;
            let joined = match task.as_mut() {
                Some(handle) => Some(handle.await),
                None => None,
            };
            // Retain ownership while pending, but retire any completed handle
            // before propagating failure: Tokio forbids polling it twice.
            task.take();
            if let Some(Err(error)) = joined {
                return Err(AcpError::Cleanup(format!("initializer failed: {error}")));
            }
        }
        let session = self.sessions.lock().await.get(session_id).cloned();
        if let Some(session) = session {
            session.close().await?;
            self.sessions.lock().await.remove(session_id);
            // Drop the final engine owner before reporting lease release.
            drop(session);
        } else if let Some(error) = initialization_error {
            return Err(AcpError::Cleanup(format!(
                "bootstrap cleanup could not be established: {error}"
            )));
        }
        self.initializers.lock().await.remove(session_id);
        Ok(())
    }
}
