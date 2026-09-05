//! Shared reader teardown for the SDK and MCP-bridge transports.

use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::error::{Result, SubprocessPluginError};

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
