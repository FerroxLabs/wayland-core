//! Count/encoded-byte bounded live delivery with reserved terminal capacity.
use futures::task::AtomicWaker;
use serde::Serialize;
use std::{
    collections::VecDeque,
    future::Future,
    sync::{Arc, Mutex, MutexGuard, OnceLock},
    task::{Context, Poll},
    time::Duration,
};
pub const LIVE_BYTES: usize = 1024 * 1024;
pub const EVENT_BYTES: usize = LIVE_BYTES;
pub const LIVE_EVENTS: usize = 256;
pub const AGGREGATE_BYTES: usize = 64 * LIVE_BYTES;

/// The aggregate byte budget that live delivery channels and retained
/// projection inputs are charged against, with the notifier woken whenever
/// capacity returns to it.
///
/// Production uses ONE budget for the whole process: [`DeliveryBudget::process`],
/// which the free functions [`channel`], [`retain`] and [`retain_encoded`] use.
/// [`DeliveryBudget::isolated`] exists only so tests that share a process --
/// the shared-process lib suite runs every unit test of a crate as threads of
/// one binary -- cannot refuse or detach each other through this aggregate. It
/// never widens the process budget: it is a separate [`AGGREGATE_BYTES`] used
/// only by whatever explicitly opted into it.
pub struct DeliveryBudget {
    used: Mutex<usize>,
    changed: tokio::sync::Notify,
}

impl DeliveryBudget {
    fn new() -> Self {
        Self {
            used: Mutex::new(0),
            changed: tokio::sync::Notify::new(),
        }
    }

    /// The single process-wide budget production delivery is charged against.
    pub fn process() -> &'static DeliveryBudget {
        static PROCESS: OnceLock<DeliveryBudget> = OnceLock::new();
        PROCESS.get_or_init(DeliveryBudget::new)
    }

    /// A separate budget for ONE test's server, so sibling tests in the same
    /// process cannot exhaust it. Leaked to be `'static`; for tests only.
    #[doc(hidden)]
    pub fn isolated() -> &'static DeliveryBudget {
        Box::leak(Box::new(DeliveryBudget::new()))
    }

    /// Bytes currently charged to this budget.
    #[doc(hidden)]
    pub fn charged(&self) -> usize {
        *self.lock_used()
    }

    fn lock_used(&self) -> MutexGuard<'_, usize> {
        self.used.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn retain<T: Serialize>(&'static self, value: &T) -> Result<Retained, &'static str> {
        let bytes = serde_json::to_vec(value)
            .map_err(|_| "cannot encode retained input")?
            .len();
        self.retain_encoded(bytes)
    }

    /// Charge an input whose exact encoded size is already known, so a caller
    /// holding a lock need not encode the value again to account for it.
    pub fn retain_encoded(&'static self, bytes: usize) -> Result<Retained, &'static str> {
        let mut used = self.lock_used();
        if bytes > EVENT_BYTES || *used + bytes > AGGREGATE_BYTES {
            return Err("retained projection input budget exhausted");
        }
        *used += bytes;
        Ok(Retained {
            bytes,
            budget: self,
        })
    }

    /// Refuse channel admission if even its reserved terminal cannot fit.
    pub fn channel<E: Serialize>(
        &'static self,
        terminal: E,
    ) -> Result<(Sender<E>, Receiver<E>), &'static str> {
        let reserved = serde_json::to_vec(&terminal)
            .map_err(|_| "cannot encode terminal")?
            .len();
        if reserved >= LIVE_BYTES {
            return Err("terminal exceeds live byte bound");
        }
        let mut used = self.lock_used();
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
            budget: self,
        });
        Ok((Sender(shared.clone()), Receiver(shared)))
    }
}

/// Charge projection-owned copies after they leave a queue. Dropping the
/// owner (completion, cancellation or stream drop) returns the same budget.
pub struct Retained {
    bytes: usize,
    budget: &'static DeliveryBudget,
}
impl Retained {
    pub fn bytes(&self) -> usize {
        self.bytes
    }
}
impl Drop for Retained {
    fn drop(&mut self) {
        if self.bytes != 0 {
            *self.budget.lock_used() -= self.bytes;
            self.budget.changed.notify_waiters();
        }
    }
}
pub fn retain<T: Serialize>(value: &T) -> Result<Retained, &'static str> {
    DeliveryBudget::process().retain(value)
}

/// Charge an input whose exact encoded size is already known, so a caller
/// holding a lock need not encode the value again to account for it.
pub fn retain_encoded(bytes: usize) -> Result<Retained, &'static str> {
    DeliveryBudget::process().retain_encoded(bytes)
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
    budget: &'static DeliveryBudget,
}
pub struct Sender<E>(Arc<Shared<E>>);
pub struct Receiver<E>(Arc<Shared<E>>);
/// Refuse channel admission if even its reserved terminal cannot fit.
pub fn channel<E: Serialize>(terminal: E) -> Result<(Sender<E>, Receiver<E>), &'static str> {
    DeliveryBudget::process().channel(terminal)
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
        let mut used = self.0.budget.lock_used();
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
            self.0.budget.changed.notify_waiters();
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
        *self.0.budget.lock_used() -= state.bytes;
        state.queue.clear();
        state.bytes = 0;
        state.overflow = true;
        drop(state);
        self.0.wake.wake();
        self.0.budget.changed.notify_waiters();
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
        if !std::ptr::eq(retained.budget, self.0.budget) {
            // A charge must be taken from the budget of the channel it is
            // queued into. Every caller does; keep the books right if not.
            debug_assert!(
                false,
                "a retained charge was queued into a channel of another budget"
            );
            *retained.budget.lock_used() -= retained.bytes;
            retained.budget.changed.notify_waiters();
            *self.0.budget.lock_used() += retained.bytes;
            retained.budget = self.0.budget;
        }
        let size = retained.bytes;
        let mut pending = Some(event);
        tokio::pin!(cancelled);
        loop {
            let changed = self.0.budget.changed.notified();
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
            *self.0.budget.lock_used() -= size;
            self.0.budget.changed.notify_waiters();
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
        *self.0.budget.lock_used() -= state.bytes + state.reserved;
        state.queue.clear();
        state.bytes = 0;
        state.reserved = 0;
        state.closed = true;
        drop(state);
        self.0.budget.changed.notify_waiters();
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

    /// Second review, MINOR 4: an isolated budget is really separate. Filling
    /// one to the refusal point does not refuse a channel or a charge on
    /// another isolated budget, and never touches the process budget.
    #[test]
    fn exhausting_an_isolated_budget_leaves_other_budgets_untouched() {
        let full = DeliveryBudget::isolated();
        let other = DeliveryBudget::isolated();
        let mut charges = Vec::new();
        while let Ok(charge) = full.retain_encoded(EVENT_BYTES) {
            charges.push(charge);
        }
        assert_eq!(charges.len(), AGGREGATE_BYTES / EVENT_BYTES);
        assert!(full.channel(serde_json::json!("terminal")).is_err());
        assert!(other.retain_encoded(EVENT_BYTES).is_ok());
        assert!(other.channel(serde_json::json!("terminal")).is_ok());
        drop(charges);
        assert_eq!(full.charged(), 0, "every charge returns to its own budget");
    }
}
