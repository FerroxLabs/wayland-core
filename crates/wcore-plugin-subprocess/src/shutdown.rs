//! Shared reader teardown for the SDK and MCP-bridge transports.

use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::error::{Result, SubprocessPluginError};

/// Host ownership of a spawned runtime, established before its handshake.
/// This does not grant plugin capabilities; it only preserves shutdown access.
pub trait RuntimeCleanupOwner: Send + Sync {
    fn sdk_started(&self, runner: std::sync::Arc<crate::runner::SubprocessPluginRunner>);
    fn mcp_bridge_started(&self, runner: std::sync::Arc<crate::mcp_bridge::McpBridgePluginRunner>);
}

pub(crate) async fn reap_child(
    child: &Mutex<Option<tokio::process::Child>>,
    deadline: tokio::time::Instant,
) -> Result<()> {
    let mut saved = child.lock().await;
    let Some(handle) = saved.as_mut() else {
        return Ok(());
    };
    let outcome = match tokio::time::timeout_at(deadline, handle.wait()).await {
        Ok(result) => result.map(|_| ()),
        Err(_) => handle.kill().await,
    };
    if outcome.is_ok() {
        saved.take();
    }
    outcome.map_err(|error| SubprocessPluginError::CleanupFailed(error.to_string()))
}

pub(crate) async fn join_reader(task: &Mutex<Option<JoinHandle<()>>>) -> Result<()> {
    let mut saved = task.lock().await;
    let Some(handle) = saved.as_mut() else {
        return Ok(());
    };
    // Borrow while waiting so cancellation of shutdown leaves cleanup
    // authority available to the next attempt.
    let joined = match tokio::time::timeout(Duration::from_secs(1), &mut *handle).await {
        Ok(result) => result,
        Err(_) => {
            handle.abort();
            handle.await
        }
    };
    saved.take();
    match joined {
        Ok(()) => Ok(()),
        Err(error) if error.is_cancelled() => Ok(()),
        Err(_) => Err(SubprocessPluginError::WorkerTerminated),
    }
}
