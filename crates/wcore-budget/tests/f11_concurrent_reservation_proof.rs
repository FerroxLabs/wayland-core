use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex, mpsc};

use wcore_budget::{BudgetCap, BudgetTracker};

#[test]
fn concurrent_provider_reservations_never_oversubscribe() {
    const CONTENDERS: usize = 32;
    const CAP_TOKENS: u64 = 100;

    let tracker = Arc::new(Mutex::new(BudgetTracker::new(
        BudgetCap::builder().per_session_tokens(CAP_TOKENS).build(),
    )));
    let start = Arc::new(Barrier::new(CONTENDERS + 1));
    let release = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel();
    let mut threads = Vec::new();

    for _ in 0..CONTENDERS {
        let tracker = Arc::clone(&tracker);
        let start = Arc::clone(&start);
        let release = Arc::clone(&release);
        let tx = tx.clone();
        threads.push(std::thread::spawn(move || {
            start.wait();
            let reservation = tracker
                .lock()
                .expect("budget tracker mutex poisoned")
                .reserve("shared-session", CAP_TOKENS, 0.0)
                .ok();
            tx.send(reservation.is_some())
                .expect("proof receiver remains alive");
            if let Some(reservation) = reservation {
                while !release.load(Ordering::Acquire) {
                    std::thread::yield_now();
                }
                tracker
                    .lock()
                    .expect("budget tracker mutex poisoned")
                    .release(reservation);
            }
        }));
    }
    drop(tx);
    start.wait();

    let admitted = (0..CONTENDERS)
        .map(|_| rx.recv().expect("every contender reports admission"))
        .filter(|admitted| *admitted)
        .count();
    assert_eq!(
        admitted, 1,
        "only one full-cap provider reservation may be outstanding"
    );
    assert_eq!(
        tracker
            .lock()
            .expect("budget tracker mutex poisoned")
            .reserved_totals("shared-session"),
        (CAP_TOKENS, 0.0),
        "rejected contenders must not create reservation overshoot"
    );

    release.store(true, Ordering::Release);
    for thread in threads {
        thread.join().expect("reservation contender did not panic");
    }
    assert_eq!(
        tracker
            .lock()
            .expect("budget tracker mutex poisoned")
            .reserved_totals("shared-session"),
        (0, 0.0),
        "the admitted reservation must be released"
    );
}
