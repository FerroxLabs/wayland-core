// S2 — transaction atomicity tests for multi-step write paths.
//
// These tests simulate crash-mid-transaction by dropping an uncommitted
// `rusqlite::Transaction` and verifying that neither the first nor the
// second write is visible after the implicit rollback.

use wcore_memory::db::TierConn;

// ---------------------------------------------------------------------------
// Helper: count rows in a named table via the shared connection.
// ---------------------------------------------------------------------------

fn count_rows(tc: &TierConn, table: &str) -> i64 {
    let conn = tc.conn.lock();
    conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// TC-S2-1: episodes + vec_episodes rollback
//
// Simulates: begin transaction → INSERT into episodes → abort before vec0
// mirror INSERT (process crash). After rollback, episodes table must be
// empty — no orphan row.
// ---------------------------------------------------------------------------

#[test]
fn tc_s2_1_episode_insert_rollback_leaves_no_orphan() {
    let tc = TierConn::open_memory().unwrap();

    // Simulate the first write of record_with_embedding inside a tx,
    // then drop the tx without committing (simulates crash between the
    // two INSERTs).
    {
        let conn = tc.conn.lock();
        let tx = conn.unchecked_transaction().unwrap();
        tx.execute(
            "INSERT INTO episodes (id, tier, ts, episode_type, summary, atomic_facts, \
             source, source_product, session_id, project_root, decay_score, status, embedding) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            rusqlite::params![
                "00000000-0000-0000-0000-000000000001",
                "global",
                1_000_000i64,
                "observation",
                "test summary",
                "[]",
                "test",
                "wayland",
                "sess-1",
                "/tmp/proj",
                1.0f64,
                "active",
                vec![0u8; 8], // dummy blob
            ],
        )
        .unwrap();
        // tx drops here without commit → implicit rollback
    }

    assert_eq!(
        count_rows(&tc, "episodes"),
        0,
        "episode row must not persist after rolled-back transaction"
    );
}

// ---------------------------------------------------------------------------
// TC-S2-2: facts INSERT + superseded_by UPDATE rollback
//
// Simulates: begin transaction → INSERT new fact → crash before UPDATE of
// prior fact's superseded_by. After rollback, facts table must be empty —
// neither the new fact nor a dangling superseded_by update is visible.
// ---------------------------------------------------------------------------

#[test]
fn tc_s2_2_fact_assert_rollback_leaves_no_orphan() {
    let tc = TierConn::open_memory().unwrap();

    // Seed a prior fact that will be superseded.
    {
        let conn = tc.conn.lock();
        conn.execute(
            "INSERT INTO facts (id, tier, ts, subject, predicate, object, confidence, embedding) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                "prior-fact-uuid",
                "global",
                999_999i64,
                "user",
                "prefers",
                "old-value",
                0.9f64,
                vec![0u8; 8],
            ],
        )
        .unwrap();
    }

    assert_eq!(count_rows(&tc, "facts"), 1);

    // Begin the assert transaction: INSERT new fact, then drop before
    // committing (simulates crash before superseded_by UPDATE).
    {
        let conn = tc.conn.lock();
        let tx = conn.unchecked_transaction().unwrap();
        tx.execute(
            "INSERT INTO facts (id, tier, ts, subject, predicate, object, confidence, embedding) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![
                "new-fact-uuid",
                "global",
                1_000_000i64,
                "user",
                "prefers",
                "new-value",
                0.95f64,
                vec![0u8; 8],
            ],
        )
        .unwrap();
        // Drop without commit — new fact must not appear.
    }

    // Only the seeded prior fact must remain; the new fact is gone.
    assert_eq!(
        count_rows(&tc, "facts"),
        1,
        "new fact must not persist after rolled-back transaction"
    );

    // The prior fact's superseded_by must still be NULL (the UPDATE never ran).
    let sup: Option<String> = {
        let conn = tc.conn.lock();
        conn.query_row(
            "SELECT superseded_by FROM facts WHERE id = 'prior-fact-uuid'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert!(
        sup.is_none(),
        "prior fact superseded_by must remain NULL after rollback, got {sup:?}"
    );
}

// ---------------------------------------------------------------------------
// TC-S2-3: procedure transition read-check-write atomicity
//
// Simulates: begin transaction → SELECT current status → crash before UPDATE.
// After rollback, status must be unchanged.
// ---------------------------------------------------------------------------

#[test]
fn tc_s2_3_procedure_transition_rollback_leaves_status_unchanged() {
    let tc = TierConn::open_memory().unwrap();

    // Seed a procedure in "active" status.
    {
        let conn = tc.conn.lock();
        conn.execute(
            "INSERT INTO procedures (id, tier, ts, name, description, artifact, status, \
             created_by, thompson_alpha, thompson_beta, use_count, success_count) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            rusqlite::params![
                "proc-uuid",
                "global",
                1_000_000i64,
                "test-skill",
                "desc",
                "artifact",
                "active",
                "agent",
                1.0f64,
                1.0f64,
                0i64,
                0i64,
            ],
        )
        .unwrap();
    }

    // Simulate the transition transaction: read status, decide to update,
    // then drop before committing.
    {
        let conn = tc.conn.lock();
        let tx = conn.unchecked_transaction().unwrap();
        let _status: String = tx
            .query_row(
                "SELECT status FROM procedures WHERE id = 'proc-uuid'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        tx.execute(
            "UPDATE procedures SET status = 'deprecated' WHERE id = 'proc-uuid'",
            [],
        )
        .unwrap();
        // Drop without commit.
    }

    let status: String = {
        let conn = tc.conn.lock();
        conn.query_row(
            "SELECT status FROM procedures WHERE id = 'proc-uuid'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    assert_eq!(
        status, "active",
        "status must remain 'active' after rolled-back transition"
    );
}
