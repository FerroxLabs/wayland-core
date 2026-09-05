use super::*;

fn store(dir: &std::path::Path) -> DailySpendStore {
    DailySpendStore::at(dir.join("nested").join("daily-spend.json"))
}

fn lease() -> Duration {
    Duration::minutes(30)
}

/// A fixed instant in the middle of a UTC day.
///
/// Buckets are keyed by UTC calendar day, so a test that advances `now`
/// past a lease must not straddle midnight: the follow-up call would land
/// on the next day, where the bucket has already reset and the
/// reservation the test was written to observe no longer exists. Anchored
/// on `Utc::now()` the arm below failed for the last 29 minutes of every
/// UTC day, which is when CI happened to run it.
fn anchor() -> DateTime<Utc> {
    at("2026-01-15T12:00:00Z")
}

/// A fixed instant, spelled RFC 3339. Every boundary arm below names the
/// clock explicitly rather than offsetting from wall time.
fn at(rfc3339: &str) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(rfc3339)
        .unwrap()
        .with_timezone(&Utc)
}

#[test]
fn missing_store_is_an_empty_ledger_and_admits_the_first_call() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let now = Utc::now();

    let grant = store.reserve("default", 0.25, 1.00, lease(), now).unwrap();
    store.settle("default", &grant, 0.20, now).unwrap();

    let position = store.position("default", now).unwrap();
    assert!((position.committed_usd - 0.20).abs() < 1e-9);
    assert_eq!(position.reserved_usd, 0.0);
    assert!(store.path().exists(), "ledger was published");
}

#[test]
fn settled_spend_accumulates_across_independent_store_handles() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc::now();

    for _ in 0..4 {
        // A separate handle each round models a separate process: nothing
        // is carried in memory between them.
        let store = store(dir.path());
        let grant = store.reserve("default", 0.25, 1.00, lease(), now).unwrap();
        store.settle("default", &grant, 0.25, now).unwrap();
    }

    let store = store(dir.path());
    let refusal = store.reserve("default", 0.25, 1.00, lease(), now);
    assert!(
        matches!(refusal, Err(DailySpendError::Exceeded { .. })),
        "fifth fresh-process call must be refused, got {refusal:?}"
    );
}

#[test]
fn in_flight_reservations_bound_a_concurrent_process() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc::now();
    let first = store(dir.path());
    let second = store(dir.path());

    // First process holds an unsettled reservation; the second must see it.
    let _held = first.reserve("default", 0.90, 1.00, lease(), now).unwrap();
    let refusal = second.reserve("default", 0.20, 1.00, lease(), now);
    assert!(matches!(refusal, Err(DailySpendError::Exceeded { .. })));
}

#[test]
fn an_expired_same_day_reservation_remains_uncertain_and_consumes_cap() {
    let dir = tempfile::tempdir().unwrap();
    let now = anchor();
    let store = store(dir.path());

    // Model a process that died between reserve and settle.
    let _abandoned = store
        .reserve("default", 0.90, 1.00, Duration::minutes(30), now)
        .unwrap();
    assert!(
        store
            .reserve("default", 0.20, 1.00, lease(), now + Duration::minutes(29))
            .is_err(),
        "the lease must still bind before it expires"
    );
    assert!(
        matches!(
            store.reserve("default", 0.20, 1.00, lease(), now + Duration::minutes(31)),
            Err(DailySpendError::Exceeded { .. })
        ),
        "expiry is not evidence that the paid request never sent"
    );
}

/// The day roll must not release authority that is still in flight.
///
/// A reservation taken at 23:50 with a 30-minute lease is live until 00:20
/// the next day. Until this bound existed the bucket was dropped wholesale
/// at 00:00 and that hold silently stopped binding.
#[test]
fn an_unexpired_reservation_still_binds_after_the_utc_day_rolls() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let before = at("2026-01-15T23:50:00Z");
    let after = at("2026-01-16T00:05:00Z");

    let grant = store
        .reserve("default", 0.90, 1.00, Duration::minutes(30), before)
        .expect("admitted out of day 1");

    let carried = store.position("default", after).unwrap();
    assert!(
        (carried.reserved_usd - 0.90).abs() < 1e-9,
        "the hold is still in flight, so the new day must see it: {carried:?}"
    );
    assert!(
        matches!(
            store.reserve("default", 0.90, 1.00, lease(), after),
            Err(DailySpendError::Exceeded { .. })
        ),
        "a concurrent process must not be admitted against authority that \
             is still held"
    );

    store.settle("default", &grant, 0.90, after).unwrap();
    let closed = store.position("default", after).unwrap();
    assert_eq!(closed.committed_usd, 0.0);
    assert!((store.position("default", before).unwrap().committed_usd - 0.90).abs() < 1e-9);
    assert!(
        closed.total_usd() <= 1.00 + USD_EPSILON,
        "the ceiling must still bind across the boundary: {closed:?}"
    );
}

/// The other half of the same bound: carrying a reservation across
/// midnight must not charge the next day indefinitely. The expired claim
/// remains uncertain on its original day until reconciliation or retention.
#[test]
fn expired_previous_day_uncertainty_stays_on_its_origin_day() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());

    let abandoned = store
        .reserve(
            "default",
            0.90,
            1.00,
            Duration::minutes(30),
            at("2026-01-15T23:50:00Z"),
        )
        .unwrap();

    assert!(
        store
            .reserve("default", 0.20, 1.00, lease(), at("2026-01-16T00:19:00Z"))
            .is_err(),
        "one minute before the carried lease expires it must still bind"
    );
    store
        .reserve("default", 0.20, 1.00, lease(), at("2026-01-16T00:21:00Z"))
        .expect("expired prior-day uncertainty does not consume the new day");
    let ledger = store.load().unwrap();
    let claim = &ledger.subjects["default"].days["2026-01-15"][abandoned.id()];
    assert_eq!(claim.usd, 0.90);
    assert_eq!(claim.status, ClaimState::Uncertain);
}

/// Cross-subject blast radius. The roll walks every subject, so it must
/// not be able to release a hold belonging to a subject that is not even
/// the one being reserved against.
#[test]
fn a_day_roll_for_one_subject_does_not_release_anothers_hold() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let before = at("2026-01-15T23:50:00Z");
    let after = at("2026-01-16T00:05:00Z");

    let _alpha_hold = store
        .reserve("alpha", 0.90, 1.00, Duration::minutes(30), before)
        .unwrap();
    // An unrelated subject touches the same ledger on the new day.
    store.reserve("beta", 0.10, 1.00, lease(), after).unwrap();

    let alpha = store.position("alpha", after).unwrap();
    assert!(
        (alpha.reserved_usd - 0.90).abs() < 1e-9,
        "one subject's day roll must not release another's authority: {alpha:?}"
    );
    assert!(
        matches!(
            store.reserve("alpha", 0.90, 1.00, lease(), after),
            Err(DailySpendError::Exceeded { .. })
        ),
        "alpha's ceiling must still bind"
    );
}

#[test]
fn released_reservations_do_not_consume_authority() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc::now();
    let store = store(dir.path());

    let grant = store.reserve("default", 0.90, 1.00, lease(), now).unwrap();
    store.release("default", &grant, now).unwrap();

    store
        .reserve("default", 0.90, 1.00, lease(), now)
        .expect("released authority is available again");
}

#[test]
fn settlement_records_spend_even_when_the_grant_already_expired() {
    let dir = tempfile::tempdir().unwrap();
    let now = anchor();
    let store = store(dir.path());

    let grant = store
        .reserve("default", 0.50, 1.00, Duration::seconds(1), now)
        .unwrap();
    let later = now + Duration::minutes(5);
    store.settle("default", &grant, 0.50, later).unwrap();

    let position = store.position("default", later).unwrap();
    assert!((position.committed_usd - 0.50).abs() < 1e-9);
}

#[test]
fn the_bucket_resets_at_the_utc_day_boundary() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let now = Utc::now();

    let grant = store.reserve("default", 1.00, 1.00, lease(), now).unwrap();
    store.settle("default", &grant, 1.00, now).unwrap();
    assert!(store.reserve("default", 0.10, 1.00, lease(), now).is_err());

    let tomorrow = now + Duration::days(1);
    store
        .reserve("default", 0.10, 1.00, lease(), tomorrow)
        .expect("a new UTC day starts from zero");
}

#[test]
fn subjects_are_independent_buckets() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let now = Utc::now();

    let grant = store.reserve("alice", 1.00, 1.00, lease(), now).unwrap();
    store.settle("alice", &grant, 1.00, now).unwrap();

    store
        .reserve("bob", 1.00, 1.00, lease(), now)
        .expect("bob has his own daily authority");
}

#[test]
fn a_corrupt_ledger_fails_closed_instead_of_resetting_the_ceiling() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let now = Utc::now();
    let grant = store.reserve("default", 0.10, 1.00, lease(), now).unwrap();
    store.settle("default", &grant, 0.10, now).unwrap();

    std::fs::write(store.path(), b"{ not json").unwrap();

    let refusal = store.reserve("default", 0.10, 1.00, lease(), now);
    assert!(
        matches!(refusal, Err(DailySpendError::Unusable { .. })),
        "a ceiling that resets when its file is overwritten is no ceiling, got {refusal:?}"
    );
}

#[test]
fn a_refused_reservation_leaves_no_trace_in_the_ledger() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let now = Utc::now();

    assert!(store.reserve("default", 5.00, 1.00, lease(), now).is_err());
    let position = store.position("default", now).unwrap();
    assert_eq!(position.total_usd(), 0.0);
}

#[test]
fn concurrent_reservations_never_oversubscribe_the_ceiling() {
    // 16 threads, each a would-be process, race 1 unit against a cap of 8.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("race").join("daily-spend.json");
    let now = Utc::now();
    let admitted = std::sync::Arc::new(AtomicU64::new(0));

    std::thread::scope(|scope| {
        for _ in 0..16 {
            let path = path.clone();
            let admitted = std::sync::Arc::clone(&admitted);
            scope.spawn(move || {
                let store = DailySpendStore::at(path);
                if let Ok(grant) = store.reserve("default", 1.0, 8.0, lease(), now) {
                    store.settle("default", &grant, 1.0, now).unwrap();
                    admitted.fetch_add(1, Ordering::SeqCst);
                }
            });
        }
    });

    assert_eq!(
        admitted.load(Ordering::SeqCst),
        8,
        "exactly the ceiling may be admitted, no more and no fewer"
    );
    let store = DailySpendStore::at(&path);
    let position = store.position("default", now).unwrap();
    assert!((position.committed_usd - 8.0).abs() < 1e-9);
}

#[test]
fn exact_receipts_replace_uncertainty_and_duplicate_receipts_do_not_add_spend() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let now = anchor();
    let grant = store.reserve("alice", 0.9, 1.0, lease(), now).unwrap();
    let later = now + Duration::minutes(31);
    store.settle_conservatively("alice", &grant, later).unwrap();
    assert_eq!(store.position("alice", later).unwrap().reserved_usd, 0.9);
    for _ in 0..2 {
        DailySpendStore::at(store.path())
            .settle("alice", &grant, 0.4, later)
            .unwrap();
    }
    // A conservative replay must not overwrite an authoritative receipt.
    store.settle_conservatively("alice", &grant, later).unwrap();
    let position = store.position("alice", later).unwrap();
    assert_eq!(position.committed_usd, 0.4);
    assert_eq!(position.reserved_usd, 0.0);
    let before = std::fs::read(store.path()).unwrap();
    assert!(store.settle("alice", &grant, 0.5, later).is_err());
    assert!(store.release("alice", &grant, later).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}

#[test]
fn no_send_refund_after_expiry_is_idempotent_and_rejects_later_spend() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let now = anchor();
    let grant = store.reserve("alice", 0.9, 1.0, lease(), now).unwrap();
    let later = now + Duration::minutes(31);
    store.settle_conservatively("alice", &grant, later).unwrap();
    for _ in 0..2 {
        store.release("alice", &grant, later).unwrap();
    }
    assert_eq!(store.position("alice", later).unwrap().total_usd(), 0.0);
    assert!(store.settle("alice", &grant, 0.2, later).is_err());
    assert!(store.settle_conservatively("alice", &grant, later).is_err());
    store.reserve("alice", 1.0, 1.0, lease(), later).unwrap();
}

#[test]
fn wrong_subject_unknown_identity_and_origin_are_refused_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let now = anchor();
    let grant = store.reserve("alice", 0.9, 1.0, lease(), now).unwrap();
    let before = std::fs::read(store.path()).unwrap();
    assert!(store.settle("bob", &grant, 0.4, now).is_err());
    assert!(store.release("bob", &grant, now).is_err());
    let mut unknown = grant.clone();
    unknown.id.push_str("-unknown");
    assert!(store.settle("alice", &unknown, 0.4, now).is_err());
    assert!(store.release("alice", &unknown, now).is_err());
    let mut wrong_day = grant.clone();
    wrong_day.day = "2026-01-14".into();
    assert!(store.settle("alice", &wrong_day, 0.4, now).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}

#[test]
fn failed_publication_keeps_original_authority_for_a_reopened_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let now = anchor();
    let grant = store.reserve("alice", 0.9, 1.0, lease(), now).unwrap();
    let before = std::fs::read(store.path()).unwrap();
    // Deterministically fail File::create even when tests run as root.
    let temp = store
        .path()
        .parent()
        .unwrap()
        .join(format!(".daily-spend.json.{}.tmp", std::process::id()));
    std::fs::create_dir(&temp).unwrap();
    assert!(store.settle("alice", &grant, 0.4, now).is_err());
    assert!(store.release("alice", &grant, now).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
    std::fs::remove_dir(temp).unwrap();
    let reopened = DailySpendStore::at(store.path());
    let later = now + Duration::minutes(31);
    assert!(matches!(
        reopened.reserve("alice", 0.2, 1.0, lease(), later),
        Err(DailySpendError::Exceeded { .. })
    ));
    reopened.settle("alice", &grant, 0.4, later).unwrap();
    assert_eq!(
        reopened.position("alice", later).unwrap().committed_usd,
        0.4
    );
}

#[test]
fn schema_one_empty_corrupt_and_future_ledgers_fail_closed_preserving_bytes() {
    for bytes in [
        br#"{"schema_version":1,"subjects":{}}"#.as_slice(),
        br#"{"schema_version":3,"subjects":{}}"#.as_slice(),
        b"".as_slice(),
        b"{broken".as_slice(),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        std::fs::create_dir_all(store.path().parent().unwrap()).unwrap();
        std::fs::write(store.path(), bytes).unwrap();
        assert!(matches!(
            store.position("alice", anchor()),
            Err(DailySpendError::Unusable { .. })
        ));
        assert!(matches!(
            store.reserve("alice", 0.1, 1.0, lease(), anchor()),
            Err(DailySpendError::Unusable { .. })
        ));
        assert_eq!(std::fs::read(store.path()).unwrap(), bytes);
    }
}

#[test]
fn retention_keeps_current_and_six_prior_days_then_refuses_old_receipts() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let origin = anchor();
    let mut grants = Vec::new();
    for offset in 0..8 {
        let now = origin + Duration::days(offset);
        let grant = store.reserve("alice", 0.9, 1.0, lease(), now).unwrap();
        store.settle("alice", &grant, 0.4, now).unwrap();
        grants.push(grant);
    }
    let now = origin + Duration::days(7);
    let ledger = store.load().unwrap();
    let days = &ledger.subjects["alice"].days;
    assert_eq!(days.len(), 7);
    assert!(!days.contains_key(&day_key(origin)));
    assert!(days.contains_key(&day_key(origin + Duration::days(1))));
    store.settle("alice", &grants[1], 0.4, now).unwrap();
    let before = std::fs::read(store.path()).unwrap();
    assert!(store.settle("alice", &grants[0], 0.4, now).is_err());
    assert!(store.release("alice", &grants[0], now).is_err());
    assert_eq!(std::fs::read(store.path()).unwrap(), before);
}

#[test]
fn retained_live_authority_outside_window_blocks_admission_but_can_be_resolved() {
    for release in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let store = store(dir.path());
        let origin = anchor();
        let grant = store
            .reserve("alice", 0.9, 1.0, Duration::days(10), origin)
            .unwrap();
        let now = origin + Duration::days(7);
        // A different subject can progress while refresh retains Alice's live claim.
        store.reserve("bob", 1.0, 1.0, lease(), now).unwrap();
        assert!(
            store.load().unwrap().subjects["alice"]
                .days
                .contains_key(&day_key(origin))
        );
        assert!(matches!(
            store.reserve("alice", 0.1, 1.0, lease(), now),
            Err(DailySpendError::Unusable { .. })
        ));
        if release {
            store.release("alice", &grant, now).unwrap();
        } else {
            store.settle("alice", &grant, 0.4, now).unwrap();
        }
        store.reserve("alice", 1.0, 1.0, lease(), now).unwrap();
        assert!(
            !store.load().unwrap().subjects["alice"]
                .days
                .contains_key(&day_key(origin))
        );
    }
}

#[test]
fn late_prior_day_receipt_replaces_only_its_originating_contribution() {
    let dir = tempfile::tempdir().unwrap();
    let store = store(dir.path());
    let before = at("2026-01-15T23:50:00Z");
    let after = at("2026-01-16T00:21:00Z");
    let old = store.reserve("alice", 0.9, 1.0, lease(), before).unwrap();
    let current = store.reserve("alice", 0.8, 1.0, lease(), after).unwrap();
    store.settle("alice", &current, 0.7, after).unwrap();
    store.settle("alice", &old, 0.4, after).unwrap();
    store.settle("alice", &old, 0.4, after).unwrap();
    assert_eq!(store.position("alice", after).unwrap().committed_usd, 0.7);
    assert_eq!(
        store.load().unwrap().subjects["alice"].days[&old.day][old.id()].status,
        ClaimState::Settled { actual_usd: 0.4 }
    );
}
