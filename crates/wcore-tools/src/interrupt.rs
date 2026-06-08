//! T3-3.1.5 — Per-thread interrupt signaling for tools.
//!
//! Port of `agent/tools/interrupt.py` (MIT (c) Nous Research). Provides
//! thread-scoped interrupt tracking so interrupting one agent session
//! does not kill tools running in other sessions. Critical when multiple
//! agents run concurrently in the same process.
//!
//! ## Usage
//!
//! ```ignore
//! use wcore_tools::interrupt::{set_interrupt, is_interrupted, clear_interrupt};
//!
//! // Signal interrupt for the current thread:
//! set_interrupt();
//!
//! // From inside a tool's loop:
//! if is_interrupted() {
//!     return tool_result_interrupted();
//! }
//!
//! // When the agent finishes a turn:
//! clear_interrupt();
//! ```
//!
//! ## Divergence from the Python source
//!
//! * Python uses `threading.current_thread().ident` (an `int`). Rust uses
//!   the opaque `std::thread::ThreadId`. Same semantics: each OS thread
//!   has its own state, and a thread only ever sees its own flag.
//! * Python's source exposes `set_interrupt(active: bool, thread_id=None)`.
//!   We split that into `set_interrupt_for` / `clear_interrupt_for` for
//!   explicit thread targets, with `set_interrupt` / `clear_interrupt`
//!   defaulting to the current thread (matches the Python default arg).
//! * Python keeps a `_ThreadAwareEventProxy` (`is_set` / `set` / `clear`
//!   / `wait`) for legacy `threading.Event`-style call sites. We mirror
//!   that as `EventProxy`; `wait` returns the current state immediately
//!   (the Python source documents this as "not truly supported").
//! * Python's `reset()` (test-only) is mirrored as `reset_all()`.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::thread::ThreadId;

fn interrupted_threads() -> &'static Mutex<HashSet<ThreadId>> {
    static INTERRUPTED: OnceLock<Mutex<HashSet<ThreadId>>> = OnceLock::new();
    INTERRUPTED.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Mark the current thread as interrupted.
pub fn set_interrupt() {
    set_interrupt_for(std::thread::current().id());
}

/// Clear the interrupt flag for the current thread.
pub fn clear_interrupt() {
    clear_interrupt_for(std::thread::current().id());
}

/// Mark the given thread as interrupted.
///
/// Used when a controller thread needs to signal a different worker
/// thread (the source documents this as the gateway-multi-agent case).
pub fn set_interrupt_for(thread_id: ThreadId) {
    let mut guard = interrupted_threads()
        .lock()
        .expect("interrupt mutex poisoned");
    guard.insert(thread_id);
}

/// Clear the interrupt flag for the given thread.
pub fn clear_interrupt_for(thread_id: ThreadId) {
    let mut guard = interrupted_threads()
        .lock()
        .expect("interrupt mutex poisoned");
    guard.remove(&thread_id);
}

/// Check whether an interrupt has been requested for the current thread.
///
/// Safe to call from any thread — each thread only observes its own
/// interrupt state.
pub fn is_interrupted() -> bool {
    let tid = std::thread::current().id();
    let guard = interrupted_threads()
        .lock()
        .expect("interrupt mutex poisoned");
    guard.contains(&tid)
}

/// Clear all per-thread interrupt state.
///
/// Test-only — production code never calls this. The Python source
/// documents that thread-id reuse across tests on the same xdist worker
/// can cause a previously-interrupted thread to look interrupted in the
/// next test; this resets that.
pub fn reset_all() {
    let mut guard = interrupted_threads()
        .lock()
        .expect("interrupt mutex poisoned");
    guard.clear();
}

/// Drop-in shim mapping `threading.Event`-style methods (`is_set` /
/// `set` / `clear` / `wait`) to the per-thread interrupt functions.
///
/// Mirrors the Python `_ThreadAwareEventProxy`. Provided so legacy call
/// sites that hold a handle to `_interrupt_event` and call those methods
/// can be ported mechanically without restructuring.
#[derive(Debug, Default, Clone, Copy)]
pub struct EventProxy;

impl EventProxy {
    pub const fn new() -> Self {
        Self
    }

    pub fn is_set(&self) -> bool {
        is_interrupted()
    }

    pub fn set(&self) {
        set_interrupt();
    }

    pub fn clear(&self) {
        clear_interrupt();
    }

    /// Returns the current interrupt state immediately. The `_timeout`
    /// argument is accepted for API compatibility with the Python
    /// `_ThreadAwareEventProxy.wait()` shim, which the source documents
    /// as "not truly supported — returns current state immediately".
    pub fn wait(&self, _timeout: Option<std::time::Duration>) -> bool {
        self.is_set()
    }
}

/// Module-level proxy matching Python's `_interrupt_event` global.
pub const INTERRUPT_EVENT: EventProxy = EventProxy::new();

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::OnceLock;
    use std::sync::mpsc;

    /// Tests in this module manipulate the shared per-process interrupt
    /// table, so they must run serially. cargo runs `#[test]` functions
    /// in parallel by default — without this mutex, one test's
    /// `reset_all()` would wipe out another test's `set_interrupt()`
    /// mid-flight.
    fn test_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    /// RAII guard: take the test lock for the duration of the test and
    /// reset state on drop so a panicking test can't poison the next.
    struct Guard<'a>(#[allow(dead_code)] std::sync::MutexGuard<'a, ()>);
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            reset_all();
        }
    }
    fn isolate() -> Guard<'static> {
        let g = match test_lock().lock() {
            Ok(g) => g,
            // If a prior test panicked while holding the lock, recover
            // the mutex; the per-thread state is still safe to mutate.
            Err(poisoned) => poisoned.into_inner(),
        };
        reset_all();
        Guard(g)
    }

    #[test]
    fn module_exports_reachable_and_default_clear() {
        let _g = isolate();
        // Public API surface (matches the source's exports).
        assert!(!is_interrupted());
        // Ensure the proxy compiles + behaves identically.
        let proxy = EventProxy::new();
        assert!(!proxy.is_set());
        assert!(!INTERRUPT_EVENT.is_set());
    }

    #[test]
    fn set_then_clear_round_trips_on_current_thread() {
        let _g = isolate();
        assert!(!is_interrupted(), "precondition: clean state");

        set_interrupt();
        assert!(is_interrupted(), "after set, current thread is interrupted");

        clear_interrupt();
        assert!(!is_interrupted(), "after clear, no longer interrupted");

        // EventProxy parity:
        INTERRUPT_EVENT.set();
        assert!(INTERRUPT_EVENT.is_set());
        assert!(INTERRUPT_EVENT.wait(Some(std::time::Duration::from_millis(1))));
        INTERRUPT_EVENT.clear();
        assert!(!INTERRUPT_EVENT.is_set());
    }

    /// Core invariant from the Python source docstring: interrupting one
    /// thread MUST NOT make another thread appear interrupted. Without
    /// this, the gateway-multi-agent isolation contract is broken.
    #[test]
    fn interrupts_are_per_thread_isolated() {
        let _g = isolate();

        // Capture this thread's id, mark it interrupted.
        let main_tid = std::thread::current().id();
        set_interrupt_for(main_tid);
        assert!(is_interrupted(), "main thread sees its own interrupt");

        // Spawn a child thread that asserts it does NOT see the
        // interrupt the main thread set on itself.
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::spawn(move || {
            let child_observed = is_interrupted();
            tx.send(child_observed).expect("send from child");
            // Now set on the child specifically and verify isolation
            // in the other direction.
            set_interrupt();
            tx.send(is_interrupted()).expect("send from child #2");
        });
        let child_initial = rx.recv().expect("recv from child");
        let child_after_self_set = rx.recv().expect("recv from child #2");
        handle.join().expect("child join");

        assert!(
            !child_initial,
            "child thread MUST NOT see main thread's interrupt"
        );
        assert!(
            child_after_self_set,
            "child sees its own interrupt after self-set"
        );

        // Main thread state is unaffected by anything the child did.
        assert!(is_interrupted(), "main still interrupted after child ran");
        clear_interrupt_for(main_tid);
        assert!(!is_interrupted(), "main cleared after explicit clear-for");
    }
}
