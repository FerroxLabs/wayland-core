//! Count/encoded-byte bounded live delivery with reserved terminal capacity.
use futures::task::AtomicWaker;
use serde::Serialize;
use std::{
    collections::VecDeque,
    future::Future,
    sync::{Arc, Mutex, OnceLock},
    task::{Context, Poll},
    time::Duration,
};
pub const LIVE_BYTES: usize = 1024 * 1024;
pub const EVENT_BYTES: usize = LIVE_BYTES;
pub const LIVE_EVENTS: usize = 256;
pub const AGGREGATE_BYTES: usize = 64 * LIVE_BYTES;
fn global() -> &'static Mutex<usize> {
    static USED: OnceLock<Mutex<usize>> = OnceLock::new();
    USED.get_or_init(|| Mutex::new(0))
}
fn capacity_changed() -> &'static tokio::sync::Notify {
    static CHANGED: OnceLock<tokio::sync::Notify> = OnceLock::new();
    CHANGED.get_or_init(tokio::sync::Notify::new)
}
/// Charge projection-owned copies after they leave a queue. Dropping the
/// owner (completion, cancellation or stream drop) returns the same budget.
pub struct Retained {
    bytes: usize,
}
impl Retained {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}
impl Drop for Retained {
    fn drop(&mut self) {
        if self.bytes != 0 {
            *global().lock().unwrap_or_else(|e| e.into_inner()) -= self.bytes;
            capacity_changed().notify_waiters();
        }
    }
}
pub fn retain<T: Serialize>(value: &T) -> Result<Retained, &'static str> {
    let bytes = serde_json::to_vec(value)
        .map_err(|_| "cannot encode retained input")?
        .len();
    let mut used = global().lock().unwrap_or_else(|e| e.into_inner());
    if bytes > EVENT_BYTES || *used + bytes > AGGREGATE_BYTES {
        return Err("retained projection input budget exhausted");
    }
    *used += bytes;
    Ok(Retained { bytes })
}

struct State<E> {
    queue: VecDeque<(E, usize)>,
    bytes: usize,
    reserved: usize,
    terminal: Option<E>,
    overflow: bool,
    closed: bool,
    senders: usize,
}
struct Shared<E> {
    state: Mutex<State<E>>,
    wake: AtomicWaker,
    on_overflow: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}
pub struct Sender<E>(Arc<Shared<E>>);
pub struct Receiver<E>(Arc<Shared<E>>);
/// Refuse channel admission if even its reserved terminal cannot fit.
pub fn channel<E: Serialize>(terminal: E) -> Result<(Sender<E>, Receiver<E>), &'static str> {
    let reserved = serde_json::to_vec(&terminal)
        .map_err(|_| "cannot encode terminal")?
        .len();
    if reserved >= LIVE_BYTES {
        return Err("terminal exceeds live byte bound");
    }
    let mut used = global().lock().unwrap_or_else(|e| e.into_inner());
    if *used + reserved > AGGREGATE_BYTES {
        return Err("aggregate live delivery exhausted");
    }
    *used += reserved;
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            queue: VecDeque::new(),
            bytes: 0,
            reserved,
            terminal: Some(terminal),
            overflow: false,
            closed: false,
            senders: 1,
        }),
        wake: AtomicWaker::new(),
        on_overflow: Mutex::new(None),
    });
    Ok((Sender(shared.clone()), Receiver(shared)))
}
impl<E> Clone for Sender<E> {
    fn clone(&self) -> Self {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .senders += 1;
        Self(self.0.clone())
    }
}
impl<E> Drop for Sender<E> {
    fn drop(&mut self) {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .senders -= 1;
        self.0.wake.wake();
    }
}
impl<E: Serialize> Sender<E> {
    pub fn set_on_overflow(&self, callback: impl Fn() + Send + Sync + 'static) {
        *self.0.on_overflow.lock().unwrap_or_else(|e| e.into_inner()) = Some(Arc::new(callback));
    }
    pub fn send(&self, event: E) -> Result<(), &'static str> {
        let size = serde_json::to_vec(&event)
            .map_err(|_| "event serialization failed")?
            .len();
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || state.overflow {
            return Err("delivery closed or overloaded");
        }
        let mut used = global().lock().unwrap_or_else(|e| e.into_inner());
        if size > EVENT_BYTES
            || state.queue.len() + 1 >= LIVE_EVENTS
            || state.bytes + size + state.reserved > LIVE_BYTES
            || *used + size > AGGREGATE_BYTES
        {
            // The reserved terminal explicitly reports loss. No structured
            // event is truncated, and the recorder can keep draining upstream.
            *used -= state.bytes;
            state.queue.clear();
            state.bytes = 0;
            state.overflow = true;
            drop(used);
            drop(state);
            self.0.wake.wake();
            capacity_changed().notify_waiters();
            if let Some(callback) = self
                .0
                .on_overflow
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
            {
                callback();
            }
            return Err("live delivery overloaded");
        }
        *used += size;
        state.bytes += size;
        state.queue.push_back((event, size));
        drop(used);
        drop(state);
        self.0.wake.wake();
        Ok(())
    }
    /// Explicitly detach live delivery after a replay gap or stall. The
    /// pre-reserved terminal remains the only terminal for this receiver.
    pub fn overload(&self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.closed || state.overflow {
            return;
        }
        *global().lock().unwrap_or_else(|e| e.into_inner()) -= state.bytes;
        state.queue.clear();
        state.bytes = 0;
        state.overflow = true;
        drop(state);
        self.0.wake.wake();
        capacity_changed().notify_waiters();
        if let Some(callback) = self
            .0
            .on_overflow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            callback();
        }
    }

    /// Delivery-only backpressure, independent of the upstream recorder.
    /// The supplied charge was acquired before cloning the borrowed replay
    /// event and remains owned until transferred into this queue or dropped.
    pub async fn send_retained(
        &self,
        event: E,
        mut retained: Retained,
        wait_budget: &mut Duration,
        cancelled: impl Future<Output = ()>,
    ) -> Result<(), &'static str> {
        let size = retained.bytes;
        let mut pending = Some(event);
        tokio::pin!(cancelled);
        loop {
            let changed = capacity_changed().notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.closed || state.overflow {
                    return Err("delivery closed or overloaded");
                }
                if state.queue.len() + 1 < LIVE_EVENTS
                    && state.bytes + size + state.reserved <= LIVE_BYTES
                {
                    state.bytes += size;
                    state
                        .queue
                        .push_back((pending.take().expect("pending event"), size));
                    retained.bytes = 0;
                    drop(state);
                    self.0.wake.wake();
                    return Ok(());
                }
                if size + state.reserved > LIVE_BYTES {
                    *wait_budget = Duration::ZERO;
                }
            }
            if wait_budget.is_zero() {
                self.overload();
                return Err("live delivery overloaded");
            }
            let started = tokio::time::Instant::now();
            let available = tokio::select! {
                biased;
                _ = &mut cancelled => false,
                _ = &mut changed => true,
                _ = tokio::time::sleep(*wait_budget) => false,
            };
            *wait_budget = wait_budget.saturating_sub(started.elapsed());
            if !available {
                self.overload();
                return Err("live delivery overloaded");
            }
        }
    }

    pub fn overloaded(&self) -> bool {
        self.0
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .overflow
    }
}
impl<E> Receiver<E> {
    pub fn cancel_producer(&self) {
        if let Some(callback) = self
            .0
            .on_overflow
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
        {
            callback();
        }
    }

    pub fn try_recv(&mut self) -> Result<E, &'static str> {
        let waker = futures::task::noop_waker();
        match self.poll_recv(&mut Context::from_waker(&waker)) {
            Poll::Ready(Some(event)) => Ok(event),
            _ => Err("empty or closed"),
        }
    }
    pub fn poll_recv(&mut self, cx: &mut Context<'_>) -> Poll<Option<E>> {
        self.0.wake.register(cx.waker());
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.overflow {
            return Poll::Ready(state.terminal.take());
        }
        if let Some((event, size)) = state.queue.pop_front() {
            state.bytes -= size;
            *global().lock().unwrap_or_else(|e| e.into_inner()) -= size;
            capacity_changed().notify_waiters();
            return Poll::Ready(Some(event));
        }
        if state.senders == 0 || state.closed {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }
    pub async fn recv(&mut self) -> Option<E> {
        futures::future::poll_fn(|cx| self.poll_recv(cx)).await
    }
}
impl<E> Drop for Receiver<E> {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|e| e.into_inner());
        *global().lock().unwrap_or_else(|e| e.into_inner()) -= state.bytes + state.reserved;
        state.queue.clear();
        state.bytes = 0;
        state.reserved = 0;
        state.closed = true;
        drop(state);
        capacity_changed().notify_waiters();
    }
}

#[cfg(test)]
mod backpressure_tests {
    use super::*;

    #[tokio::test]
    async fn pending_delivery_is_cancelled_without_waiting_for_its_budget() {
        let terminal = serde_json::json!({"overflow":true});
        let (tx, mut rx) = channel(terminal.clone()).unwrap();
        let event = serde_json::json!("x".repeat(LIVE_BYTES / 2));
        tx.send(event.clone()).unwrap();
        let charge = retain(&event).unwrap();
        let mut budget = Duration::from_secs(60);
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        let waiting = tx.send_retained(event, charge, &mut budget, async {
            let _ = cancelled.await;
        });
        tokio::pin!(waiting);
        assert!(
            futures::poll!(&mut waiting).is_pending(),
            "full queue must await capacity"
        );
        cancel.send(()).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), &mut waiting)
                .await
                .unwrap()
                .is_err()
        );
        assert_eq!(rx.recv().await, Some(terminal));
        assert!(rx.recv().await.is_none(), "only one reserved terminal");
    }
}
