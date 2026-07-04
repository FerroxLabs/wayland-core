//! v0.6.4 Task 6.6d — semantic.rs::assert wiring with ContradictionResolver.
//!
//! Covers all three resolver verdicts: Supersede, KeepExisting, Coexist.
//! The resolver is the #664 shipping default, so it runs whether the env is
//! unset or set to a non-opt-out value. Also covers the legacy opt-out
//! (`WAYLAND_CONTRADICTION=off` → unconditional supersede path runs).
//!
//! Scenarios derive from the design doc §6.3 golden table:
//!   - Supersede:    existing=0.50, new=0.80 (adjusted_new=0.96 > existing)
//!   - KeepExisting: existing=0.95, new=0.20 (adjusted_new=0.24, diff≥0.1)
//!   - Coexist:      existing=0.85, new=0.70 (adjusted_new=0.84, diff<0.1)
//!
//! Tests serialise on a Mutex because `WAYLAND_CONTRADICTION` is a
//! process-wide env var.

use std::sync::{Arc, OnceLock};

use tokio::sync::Mutex;
use wcore_memory::cdc::CdcWriter;
use wcore_memory::db::Db;
use wcore_memory::embed::HashedEmbedder;
use wcore_memory::partition::SemanticPartition;
use wcore_memory::v2_types::{Fact, FactId, Tier};

const ENV_KEY: &str = "WAYLAND_CONTRADICTION";

// tokio::sync::Mutex so the guard can be held across .await points
// (clippy::await_holding_lock fires on std::sync::Mutex).
fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

async fn fresh_partition() -> SemanticPartition {
    let db = Arc::new(Db::open_memory().unwrap());
    let embedder = Arc::new(HashedEmbedder::new().await.unwrap());
    let cdc = Arc::new(CdcWriter::new_stub());
    SemanticPartition::new(db, embedder, cdc)
}

fn fact(subj: &str, pred: &str, obj: &str, conf: f64) -> Fact {
    Fact {
        id: FactId::new(),
        tier: Tier::Project,
        ts: 0,
        subject: subj.into(),
        predicate: pred.into(),
        object: obj.into(),
        confidence: conf,
        source_episode: None,
        superseded_by: None,
    }
}

/// Helper: read all facts for (subject, predicate, tier) — active and
/// superseded — returning (object, confidence, superseded_by_is_some).
async fn list_all(p: &SemanticPartition, subj: &str) -> Vec<(String, f64, bool)> {
    p.list_by_subject(subj, Tier::Project)
        .await
        .unwrap()
        .into_iter()
        .map(|f| (f.object, f.confidence, f.superseded_by.is_some()))
        .collect()
}

#[tokio::test]
async fn supersede_when_env_set_and_new_confidence_higher() {
    let _guard = env_lock().lock().await;
    // SAFETY: serialised by env_lock above.
    unsafe { std::env::set_var(ENV_KEY, "1") };

    let p = fresh_partition().await;
    // existing=0.50, new=0.80 → adjusted_new=0.96 > 0.50 → Supersede
    let f1 = fact("lang", "version", "2023", 0.50);
    p.assert(f1.clone()).await.unwrap();
    let f2 = fact("lang", "version", "2024", 0.80);
    let id2 = p.assert(f2.clone()).await.unwrap();

    let rows = list_all(&p, "lang").await;
    // Both facts present; old marked superseded, new active.
    assert_eq!(
        rows.len(),
        2,
        "supersede keeps both rows in table: {rows:?}"
    );
    let old = rows
        .iter()
        .find(|r| r.0 == "2023")
        .expect("old object present");
    let new = rows
        .iter()
        .find(|r| r.0 == "2024")
        .expect("new object present");
    assert!(old.2, "old fact must be marked superseded_by");
    assert!(!new.2, "new fact must NOT be marked superseded_by");
    // New confidence preserved at original (Supersede uses new_confidence as-is).
    assert!(
        (new.1 - 0.80).abs() < 1e-9,
        "new confidence preserved at 0.80, got {}",
        new.1
    );
    assert_eq!(id2, f2.id);

    unsafe { std::env::remove_var(ENV_KEY) };
}

#[tokio::test]
async fn keep_existing_when_env_set_and_new_confidence_much_lower() {
    let _guard = env_lock().lock().await;
    unsafe { std::env::set_var(ENV_KEY, "1") };

    let p = fresh_partition().await;
    // existing=0.95, new=0.20 → adjusted_new=0.24, diff=0.71 ≥ 0.1 → KeepExisting
    let f1 = fact("lang", "version", "2023", 0.95);
    let id1 = p.assert(f1.clone()).await.unwrap();
    let f2 = fact("lang", "version", "2024", 0.20);
    let id2 = p.assert(f2.clone()).await.unwrap();

    let rows = list_all(&p, "lang").await;
    // KeepExisting → new fact NOT inserted, existing untouched.
    assert_eq!(rows.len(), 1, "KeepExisting skips the new insert: {rows:?}");
    let only = &rows[0];
    assert_eq!(only.0, "2023", "existing object survives intact");
    assert!(
        (only.1 - 0.95).abs() < 1e-9,
        "existing confidence untouched, got {}",
        only.1
    );
    assert!(!only.2, "existing fact NOT superseded");
    // The returned id is the existing fact's id (the new one was discarded).
    assert_eq!(id2, id1, "assert returns existing id when KeepExisting");

    unsafe { std::env::remove_var(ENV_KEY) };
}

#[tokio::test]
async fn coexist_when_env_set_and_confidences_close() {
    let _guard = env_lock().lock().await;
    unsafe { std::env::set_var(ENV_KEY, "1") };

    let p = fresh_partition().await;
    // existing=0.85, new=0.70 → adjusted_new=0.84, diff=0.01 < 0.1 → Coexist
    let f1 = fact("lang", "version", "2023", 0.85);
    p.assert(f1.clone()).await.unwrap();
    let f2 = fact("lang", "version", "2024", 0.70);
    p.assert(f2.clone()).await.unwrap();

    let rows = list_all(&p, "lang").await;
    // Coexist → both rows present, neither superseded, new at 0.56 (=0.70*0.8).
    assert_eq!(rows.len(), 2, "coexist inserts both rows: {rows:?}");
    let old = rows
        .iter()
        .find(|r| r.0 == "2023")
        .expect("old object present");
    let new = rows
        .iter()
        .find(|r| r.0 == "2024")
        .expect("new object present");
    assert!(!old.2, "old fact NOT superseded under Coexist");
    assert!(!new.2, "new fact NOT superseded under Coexist");
    assert!((old.1 - 0.85).abs() < 1e-9, "existing confidence preserved");
    assert!(
        (new.1 - 0.56).abs() < 1e-9,
        "new confidence reduced to 0.56 (0.70 * 0.8), got {}",
        new.1
    );

    unsafe { std::env::remove_var(ENV_KEY) };
}

#[tokio::test]
async fn legacy_supersede_when_env_off() {
    let _guard = env_lock().lock().await;
    // #664: the resolver is now the default; the legacy unconditional-supersede
    // path is the explicit opt-out, reached via WAYLAND_CONTRADICTION=off.
    unsafe { std::env::set_var(ENV_KEY, "off") };

    let p = fresh_partition().await;
    // Same inputs as keep_existing — the resolver would KeepExisting here, so
    // observing a supersede proves the legacy opt-out path ran instead.
    let f1 = fact("lang", "version", "2023", 0.95);
    p.assert(f1).await.unwrap();
    let f2 = fact("lang", "version", "2024", 0.20);
    p.assert(f2).await.unwrap();

    let rows = list_all(&p, "lang").await;
    assert_eq!(rows.len(), 2, "legacy path inserts new + supersedes old");
    let old = rows.iter().find(|r| r.0 == "2023").unwrap();
    let new = rows.iter().find(|r| r.0 == "2024").unwrap();
    assert!(old.2, "legacy path supersedes the old fact");
    assert!(!new.2);
    // Confidence preserved as-supplied (no resolver adjustment in legacy path).
    assert!((new.1 - 0.20).abs() < 1e-9);

    unsafe { std::env::remove_var(ENV_KEY) };
}
