//! W08: durable daily authority survives JSON recovery and independent processes.
use chrono::{DateTime, Duration, Utc};
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command};
use std::sync::Arc;
use wcore_budget::{
    BudgetCap, BudgetReservation, BudgetTracker, BudgetTrackerSnapshot, DailyAuthority, DailyGrant,
    DailySpendError, DailySpendStore,
};

fn tracker(path: &Path) -> BudgetTracker {
    let mut tracker = BudgetTracker::new(BudgetCap::builder().per_user_daily_usd(1.0).build());
    tracker.set_daily_authority(
        DailyAuthority::new(Arc::new(DailySpendStore::at(path)), "alice")
            .with_lease(Duration::seconds(-1)),
    );
    tracker
}

fn wire(tracker: &BudgetTracker) -> Value {
    serde_json::to_value(tracker.snapshot().unwrap()).unwrap()
}

fn restore(value: Value, path: &Path) -> BudgetTracker {
    // Round-trip the actual JSON representation, including private claim binding.
    let bytes = serde_json::to_vec(&value).unwrap();
    let decoded: BudgetTrackerSnapshot = serde_json::from_slice(&bytes).unwrap();
    let mut restored = tracker(path);
    restored.restore_snapshot(decoded).unwrap();
    restored
}

fn grant(value: &Value) -> DailyGrant {
    serde_json::from_value(value["reservations"][0]["reservation"]["daily_grant"].clone()).unwrap()
}

fn publication_blocker(path: &Path) -> std::path::PathBuf {
    path.parent().unwrap().join(format!(
        ".{}.{}.tmp",
        path.file_name().unwrap().to_str().unwrap(),
        std::process::id()
    ))
}

#[test]
fn json_restore_preserves_daily_grant_and_refunds_proven_no_send_after_expiry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daily.json");
    let mut original = tracker(&path);
    let reservation = original.reserve_turn("session", 10, 20, 0.9).unwrap();
    let snapshot = wire(&original);
    assert_eq!(snapshot["schema_version"], 2);
    let binding = grant(&snapshot);
    assert!(!binding.id().is_empty());
    drop(original);
    let mut restored = restore(snapshot.clone(), &path);
    assert_eq!(grant(&wire(&restored)), binding);
    assert!(restored.release(reservation));
    assert!(!restored.release(reservation));
    assert_eq!(restored.reserved_totals("session"), (0, 0.0));
    // Replay a pre-refund snapshot; the same no-send proof is idempotent.
    let mut replay = restore(snapshot, &path);
    assert!(replay.release(reservation));
    tracker(&path).reserve("new-session", 1, 1.0).unwrap();
}

#[test]
fn conservative_restore_stays_uncertain_until_exact_receipt_and_replay_is_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daily.json");
    let mut original = tracker(&path);
    let reservation = original.reserve_turn("session", 10, 20, 0.9).unwrap();
    let snapshot = wire(&original);
    let binding = grant(&snapshot);
    let mut restored = restore(snapshot.clone(), &path);
    let report = restored.reconcile_restored_reservations_conservatively();
    assert_eq!(report.reservations_settled, 1);
    assert!(report.cap_errors.is_empty());
    assert_eq!(restored.session_totals("session"), (30, 0.9));
    assert_eq!(
        restored
            .reconcile_restored_reservations_conservatively()
            .reservations_settled,
        0
    );
    let store = DailySpendStore::at(&path);
    let now = Utc::now();
    let position = store.position("alice", now).unwrap();
    assert_eq!(position.reserved_usd, 0.9);
    assert_eq!(position.committed_usd, 0.0);
    store.settle("alice", &binding, 0.4, now).unwrap();
    // A restored copy of the original call can publish that receipt again,
    // without duplicating the durable daily contribution.
    let mut replay = restore(snapshot, &path);
    replay.settle_turn(reservation, 5, 10, 0.4).unwrap();
    replay.settle_turn(reservation, 5, 10, 0.4).unwrap();
    assert_eq!(replay.session_totals("session"), (15, 0.4));
    assert_eq!(store.position("alice", now).unwrap().committed_usd, 0.4);
    assert_eq!(store.position("alice", now).unwrap().reserved_usd, 0.0);
}

#[test]
fn duplicate_published_receipt_restores_once_and_conflicting_replay_keeps_binding() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daily.json");
    let mut original = tracker(&path);
    let reservation = original.reserve("session", 30, 0.9).unwrap();
    let snapshot = wire(&original);
    let store = DailySpendStore::at(&path);
    // Model crash after ledger publication but before the tracker snapshot commit.
    store
        .settle("alice", &grant(&snapshot), 0.4, Utc::now())
        .unwrap();
    let mut conflicting = restore(snapshot.clone(), &path);
    assert!(conflicting.settle(reservation, 15, 0.5).is_err());
    assert!(conflicting.has_reservation(reservation));
    assert_eq!(conflicting.session_totals("session"), (0, 0.0));
    let mut restored = restore(snapshot, &path);
    restored.settle(reservation, 15, 0.4).unwrap();
    let mut twice = restore(wire(&restored), &path);
    twice.settle(reservation, 15, 0.4).unwrap();
    assert_eq!(twice.session_totals("session"), (15, 0.4));
    assert_eq!(
        store.position("alice", Utc::now()).unwrap().committed_usd,
        0.4
    );
}

#[test]
fn failed_daily_publication_preserves_json_reservation_and_conservative_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daily.json");
    let mut original = tracker(&path);
    let reservation = original.reserve_turn("session", 10, 20, 0.9).unwrap();
    let before = wire(&original);
    let blocker = publication_blocker(&path);
    std::fs::create_dir(&blocker).unwrap();
    assert!(original.settle_turn(reservation, 5, 10, 0.4).is_err());
    assert!(!original.release(reservation));
    assert_eq!(wire(&original), before);
    let mut restored = restore(wire(&original), &path);
    let report = restored.reconcile_restored_reservations_conservatively();
    assert_eq!(report.reservations_settled, 0);
    assert_eq!(report.cost_usd_charged, 0.0);
    assert_eq!(report.cap_errors.len(), 1);
    assert!(restored.has_reservation(reservation));
    std::fs::remove_dir(blocker).unwrap();
    let retry = restored.reconcile_restored_reservations_conservatively();
    assert_eq!(retry.reservations_settled, 1);
    assert!(retry.cap_errors.is_empty());
    assert_eq!(restored.session_totals("session"), (30, 0.9));
    assert!(tracker(&path).reserve("second", 1, 0.2).is_err());
}

#[test]
fn restored_binding_requires_matching_authority_before_settlement_or_release() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daily.json");
    let mut original = tracker(&path);
    let reservation = original.reserve("session", 30, 0.9).unwrap();
    let mut restored = BudgetTracker::from_snapshot(original.snapshot().unwrap()).unwrap();
    assert!(!restored.release(reservation));
    assert!(restored.settle(reservation, 15, 0.4).is_err());
    restored.set_daily_authority(DailyAuthority::new(
        Arc::new(DailySpendStore::at(&path)),
        "bob",
    ));
    assert!(!restored.release(reservation));
    assert!(restored.settle(reservation, 15, 0.4).is_err());
    assert!(restored.has_reservation(reservation));
    assert_eq!(
        DailySpendStore::at(&path)
            .position("alice", Utc::now())
            .unwrap()
            .reserved_usd,
        0.9
    );
    restored.set_daily_authority(DailyAuthority::new(
        Arc::new(DailySpendStore::at(&path)),
        "alice",
    ));
    restored.settle(reservation, 15, 0.4).unwrap();
}

#[test]
fn old_snapshots_are_accepted_only_when_no_daily_binding_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("daily.json");
    let mut uncapped = BudgetTracker::new(BudgetCap::default());
    uncapped.reserve("ordinary", 10, 0.1).unwrap();
    let mut old = wire(&uncapped);
    old["schema_version"] = json!(1);
    let accepted = BudgetTracker::from_snapshot(serde_json::from_value(old).unwrap()).unwrap();
    assert_eq!(accepted.reserved_totals("ordinary"), (10, 0.1));
    let mut no_outstanding = wire(&tracker(&path));
    no_outstanding["schema_version"] = json!(1);
    BudgetTracker::from_snapshot(serde_json::from_value(no_outstanding).unwrap()).unwrap();
    let mut capped = tracker(&path);
    capped.reserve("daily", 10, 0.9).unwrap();
    for version in [1, 2] {
        let mut missing = wire(&capped);
        missing["schema_version"] = json!(version);
        missing["reservations"][0]["reservation"]
            .as_object_mut()
            .unwrap()
            .remove("daily_grant");
        assert!(BudgetTracker::from_snapshot(serde_json::from_value(missing).unwrap()).is_err());
    }
    let mut malformed = wire(&capped);
    malformed["reservations"][0]["reservation"]["daily_grant"]["id"] = json!("");
    assert!(BudgetTracker::from_snapshot(serde_json::from_value(malformed).unwrap()).is_err());
}

fn fixed_time() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-01-15T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn child(mode: &str, directory: &Path) -> Command {
    // Launch the Rust test executable directly with argv (no shell/input expansion).
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", "daily_process_helper", "--nocapture"])
        .env("W08_CHILD_MODE", mode)
        .env("W08_CHILD_DIRECTORY", directory);
    command
}

struct KillOnDrop(Child);
impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn sent_crashed_process_cannot_free_daily_capacity_without_reopening_its_session() {
    let dir = tempfile::tempdir().unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let address = listener.local_addr().unwrap();
    let mut command = child("send", dir.path());
    command.env("W08_PROVIDER_ADDRESS", address.to_string());
    let mut sending = KillOnDrop(command.spawn().unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    let mut socket = loop {
        match listener.accept() {
            Ok((socket, _)) => break socket,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                assert!(
                    sending.0.try_wait().unwrap().is_none(),
                    "sender exited before dispatch"
                );
                assert!(
                    std::time::Instant::now() < deadline,
                    "sender did not dispatch"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            Err(error) => panic!("fixture accept failed: {error}"),
        }
    };
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(10)))
        .unwrap();
    let mut sent = [0; 9];
    socket.read_exact(&mut sent).unwrap();
    assert_eq!(&sent, b"W08-spend");
    // Abrupt process termination: no tracker Drop/recovery/settlement is allowed.
    sending.0.kill().unwrap();
    assert!(!sending.0.wait().unwrap().success());
    // A distinct process advances an injected daily-store clock past the lease.
    let output = child("probe-expired", dir.path()).output().unwrap();
    assert!(
        output.status.success(),
        "{} {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn failed_publication_then_fresh_process_recovery_retains_claim_and_retries_once() {
    let dir = tempfile::tempdir().unwrap();
    for mode in [
        "fail-publication",
        "recover-publication",
        "verify-recovered",
    ] {
        let output = child(mode, dir.path()).output().unwrap();
        assert!(
            output.status.success(),
            "{mode}: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn daily_process_helper() {
    let Ok(mode) = std::env::var("W08_CHILD_MODE") else {
        return;
    };
    let dir = std::path::PathBuf::from(std::env::var_os("W08_CHILD_DIRECTORY").unwrap());
    let path = dir.join("daily.json");
    match mode.as_str() {
        "send" => {
            let store = DailySpendStore::at(&path);
            store
                .reserve("alice", 0.9, 1.0, Duration::minutes(30), fixed_time())
                .unwrap();
            let mut socket =
                TcpStream::connect(std::env::var("W08_PROVIDER_ADDRESS").unwrap()).unwrap();
            socket.write_all(b"W08-spend").unwrap();
            // Stay at the after-send/before-settlement barrier until killed.
            let mut barrier = [0; 1];
            let _ = socket.read(&mut barrier);
            panic!("sender must be killed before settling");
        }
        "probe-expired" => {
            let store = DailySpendStore::at(&path);
            let later = fixed_time() + Duration::minutes(31);
            assert!(matches!(
                store.reserve("alice", 0.2, 1.0, Duration::minutes(30), later),
                Err(DailySpendError::Exceeded { .. })
            ));
            assert_eq!(store.position("alice", later).unwrap().reserved_usd, 0.9);
            // Remaining allowance and unrelated subjects still work.
            store
                .reserve("alice", 0.1, 1.0, Duration::minutes(30), later)
                .unwrap();
            store
                .reserve("bob", 1.0, 1.0, Duration::minutes(30), later)
                .unwrap();
        }
        "fail-publication" => {
            let mut tracker = tracker(&path);
            let reservation = tracker.reserve("session", 30, 0.9).unwrap();
            let blocker = publication_blocker(&path);
            std::fs::create_dir(&blocker).unwrap();
            assert!(tracker.settle(reservation, 15, 0.4).is_err());
            assert!(tracker.has_reservation(reservation));
            std::fs::write(
                dir.join("snapshot.json"),
                serde_json::to_vec(&tracker.snapshot().unwrap()).unwrap(),
            )
            .unwrap();
            std::fs::write(
                dir.join("reservation.json"),
                serde_json::to_vec(&reservation).unwrap(),
            )
            .unwrap();
            std::fs::remove_dir(blocker).unwrap();
        }
        "recover-publication" => {
            let mut tracker = restore(
                serde_json::from_slice(&std::fs::read(dir.join("snapshot.json")).unwrap()).unwrap(),
                &path,
            );
            let reservation: BudgetReservation =
                serde_json::from_slice(&std::fs::read(dir.join("reservation.json")).unwrap())
                    .unwrap();
            assert!(tracker.reserve("other", 1, 0.2).is_err());
            tracker.settle(reservation, 15, 0.4).unwrap();
            std::fs::write(
                dir.join("snapshot.json"),
                serde_json::to_vec(&tracker.snapshot().unwrap()).unwrap(),
            )
            .unwrap();
        }
        "verify-recovered" => {
            let mut tracker = restore(
                serde_json::from_slice(&std::fs::read(dir.join("snapshot.json")).unwrap()).unwrap(),
                &path,
            );
            assert_eq!(
                tracker
                    .reconcile_restored_reservations_conservatively()
                    .reservations_settled,
                0
            );
            assert_eq!(tracker.session_totals("session"), (15, 0.4));
            assert_eq!(
                DailySpendStore::at(&path)
                    .position("alice", Utc::now())
                    .unwrap()
                    .committed_usd,
                0.4
            );
            tracker.reserve("remaining", 1, 0.6).unwrap();
            assert!(tracker.reserve("over", 1, 0.1).is_err());
        }
        _ => panic!("unknown child mode"),
    }
}
