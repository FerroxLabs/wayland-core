//! Cancellation-safe ownership of background transport tasks.

use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::Poll;
use tokio::task::{JoinError, JoinHandle};

#[derive(Debug)]
pub(super) struct RetainedTask {
    handle: Mutex<Option<JoinHandle<()>>>,
    joining: tokio::sync::Mutex<()>,
}

impl From<JoinHandle<()>> for RetainedTask {
    fn from(handle: JoinHandle<()>) -> Self {
        Self {
            handle: Mutex::new(Some(handle)),
            joining: tokio::sync::Mutex::new(()),
        }
    }
}

impl RetainedTask {
    pub(super) fn abort(&self) {
        if let Some(handle) = self
            .handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
        {
            handle.abort();
        }
    }

    pub(super) fn is_finished(&self) -> bool {
        self.handle
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_none_or(JoinHandle::is_finished)
    }

    pub(super) async fn join(&self) -> Result<(), JoinError> {
        let _joining = self.joining.lock().await;
        std::future::poll_fn(|cx| {
            let mut saved = self.handle.lock().unwrap_or_else(|e| e.into_inner());
            let Some(handle) = saved.as_mut() else {
                return Poll::Ready(Ok(()));
            };
            match Pin::new(handle).poll(cx) {
                Poll::Ready(result) => {
                    saved.take();
                    Poll::Ready(result)
                }
                Poll::Pending => Poll::Pending,
            }
        })
        .await
    }
}

impl Drop for RetainedTask {
    fn drop(&mut self) {
        self.abort();
    }
}
