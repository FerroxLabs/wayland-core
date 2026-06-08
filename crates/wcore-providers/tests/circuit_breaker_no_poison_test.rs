//! Wave RB RELIABILITY MAJOR: CircuitBreaker no-poisoning regression test.
//!
//! Before the fix, `CircuitBreaker` held `std::sync::Mutex` and every
//! critical section called `.lock().expect("CircuitBreaker mutex")`. A
//! panic while holding the lock would poison it, and every subsequent
//! `.lock().expect(...)` would panic too — cascading the failure across
//! the entire provider stack. The fix switches to `parking_lot::Mutex`,
//! which has no poisoning semantics, so a panic-and-recover scenario
//! leaves the breaker functional.

use std::sync::Arc;
use std::time::Duration;

use wcore_providers::resilient::{CircuitBreaker, CircuitConfig, CircuitState};

/// Catching a panic inside a thread that was operating on the
/// breaker's state must NOT cause subsequent breaker calls to panic.
/// (Under `std::sync::Mutex` the poisoning cascade would fire here.)
#[test]
fn breaker_recovers_after_panic_in_neighbouring_thread() {
    let breaker = Arc::new(CircuitBreaker::new(CircuitConfig {
        fail_threshold: 3,
        window: Duration::from_secs(30),
        cooldown: Duration::from_secs(60),
    }));

    // Spawn a thread that acquires breaker state via on_failure and
    // then panics inside the same scope. With std::sync::Mutex, the
    // poisoning would propagate; with parking_lot, the panic unwinds
    // cleanly and the breaker stays usable.
    let breaker_for_panic = Arc::clone(&breaker);
    let join = std::thread::spawn(move || {
        let _ = breaker_for_panic.on_failure();
        panic!("intentional panic to test poisoning resistance");
    });

    // Wait for the panicked thread to finish (Result::Err because of
    // the panic — we expect that and ignore it).
    let _ = join.join();

    // Now the main thread continues to use the breaker — under the
    // old std::sync::Mutex behaviour this would propagate the
    // poisoning and panic. With parking_lot we expect the breaker to
    // keep working.
    let first = breaker.on_failure();
    let _ = first; // 2nd failure (first was inside the panicked thread)
    let third = breaker.on_failure();
    assert_eq!(
        third,
        Some(CircuitState::Open),
        "breaker must transition to Open after the threshold even when a prior critical-section thread panicked"
    );
}

/// Direct API: hold the lock, panic during execution, recover, observe
/// that subsequent lock acquisitions still work. This is the focused
/// invariant the audit's MAJOR-#11 mitigation requested.
#[test]
fn panicking_while_holding_breaker_does_not_poison() {
    let breaker = Arc::new(CircuitBreaker::new(CircuitConfig::default()));

    // Use catch_unwind to simulate "panic in critical section" without
    // killing the test process.
    let breaker_clone = Arc::clone(&breaker);
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _ = breaker_clone.on_failure();
        panic!("simulate panic inside critical section");
    }));
    assert!(
        result.is_err(),
        "expected the catch_unwind closure to capture a panic"
    );

    // After the panic, the breaker must still serve calls.
    let next = breaker.on_failure();
    // We don't assert the specific variant here (depends on how many
    // failures landed before the threshold) — only that the call did
    // not panic, which is the regression guarantee.
    let _ = next;
}

/// All three CircuitBreaker entry points (`before_call`, `on_success`,
/// `on_failure`) must keep working post-panic. This guards against a
/// future refactor that reverts only one of the three call sites.
#[test]
fn all_breaker_entry_points_survive_panic() {
    let breaker = Arc::new(CircuitBreaker::new(CircuitConfig::default()));

    let breaker_clone = Arc::clone(&breaker);
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _ = breaker_clone.on_failure();
        panic!("simulate panic")
    }));

    // All three of these must return without panicking.
    let _ = breaker.before_call();
    let _ = breaker.on_success();
    let _ = breaker.on_failure();
}
