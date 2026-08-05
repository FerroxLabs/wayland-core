fn authority_baseline(journal: &SessionJournal) {
    journal
        .append(imported(json!({
            "id": "s1",
            "schema_version": 1,
            "messages": [],
        })))
        .unwrap();
}

fn budget_authority(
    journal: &SessionJournal,
    authority_epoch: u64,
    captured_at_unix_millis: u64,
    wall_clock: BudgetWallClockAuthority,
    active_turn_id: Option<&str>,
) -> BudgetAuthorityState {
    let state = journal.state().unwrap();
    let root = ExecutionBudget {
        max_wall_time: Some(std::time::Duration::from_secs(60)),
        max_tool_runtime: Some(std::time::Duration::from_secs(30)),
        max_processes: Some(2),
        max_agent_depth: Some(3),
        ..ExecutionBudget::default()
    }
    .start_root();
    let active_turn = active_turn_id.map(|turn_id| ActiveTurnBudgetAuthority {
        turn_id: turn_id.to_owned(),
        execution: root.sub_budget(None).snapshot().unwrap(),
    });
    BudgetAuthorityState {
        schema_version: BUDGET_AUTHORITY_SCHEMA_VERSION,
        authority_epoch,
        prior_cursor: BudgetAuthorityCursor {
            journal_sequence: state.last_seq,
            journal_checksum: state.last_checksum,
        },
        budget_session_id: "budget-session-stable".to_owned(),
        provider_tracker: BudgetTracker::new(
            BudgetCap::builder()
                .per_session_tokens(10_000)
                .per_session_usd(10.0)
                .build(),
        )
        .snapshot()
        .unwrap(),
        provider_reservations: std::collections::BTreeMap::new(),
        execution_root: root.snapshot().unwrap(),
        active_turn,
        captured_at_unix_millis,
        wall_clock,
        conversation_digest: state_payload_digest(&serde_json::Value::Array(state.conversation))
            .unwrap(),
    }
}

fn commit_budget_authority(
    journal: &SessionJournal,
    authority: BudgetAuthorityState,
) -> Result<JournalEnvelope, JournalError> {
    journal.append(SessionEvent::BudgetAuthorityCommitted { authority })
}

#[test]
fn budget_authority_round_trips_as_latest_reduced_enforcement_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    authority_baseline(&journal);
    let authority = budget_authority(
        &journal,
        1,
        1_000,
        BudgetWallClockAuthority::AbsoluteDeadline {
            deadline_unix_millis: 61_000,
        },
        None,
    );

    commit_budget_authority(&journal, authority.clone()).unwrap();
    assert_eq!(journal.state().unwrap().budget_authority, Some(authority));

    drop(journal);
    let reopened = SessionJournal::open(path, "s1").unwrap();
    assert_eq!(
        reopened
            .state()
            .unwrap()
            .budget_authority
            .as_ref()
            .unwrap()
            .authority_epoch,
        1
    );
}

#[test]
fn legacy_budget_authority_replays_but_cannot_replace_current_schema() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "s1").unwrap();
    authority_baseline(&journal);
    let mut legacy = budget_authority(
        &journal,
        1,
        1_000,
        BudgetWallClockAuthority::ActiveRuntime,
        None,
    );
    legacy.schema_version = LEGACY_BUDGET_AUTHORITY_SCHEMA_VERSION;
    commit_budget_authority(&journal, legacy).unwrap();

    let current = budget_authority(
        &journal,
        2,
        1_001,
        BudgetWallClockAuthority::ActiveRuntime,
        None,
    );
    commit_budget_authority(&journal, current).unwrap();

    let mut regressed = budget_authority(
        &journal,
        3,
        1_002,
        BudgetWallClockAuthority::ActiveRuntime,
        None,
    );
    regressed.schema_version = LEGACY_BUDGET_AUTHORITY_SCHEMA_VERSION;
    assert!(matches!(
        commit_budget_authority(&journal, regressed),
        Err(JournalError::InvalidTransition(message))
            if message.contains("schema regressed")
    ));
}

#[test]
fn budget_authority_with_live_provider_reservation_reopens() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    authority_baseline(&journal);

    let mut provider_tracker = BudgetTracker::new(
        BudgetCap::builder()
            .per_session_tokens(10_000)
            .per_session_usd(10.0)
            .build(),
    );
    provider_tracker
        .reserve("budget-session-stable", 1_000, 1.0)
        .unwrap();
    let mut authority = budget_authority(
        &journal,
        1,
        1_000,
        BudgetWallClockAuthority::ActiveRuntime,
        None,
    );
    authority.provider_tracker = provider_tracker.snapshot().unwrap();
    commit_budget_authority(&journal, authority).unwrap();

    drop(journal);
    let reopened = SessionJournal::open(path, "s1").unwrap();
    assert_eq!(
        reopened
            .state()
            .unwrap()
            .budget_authority
            .unwrap()
            .authority_epoch,
        1
    );
}

#[test]
fn budget_authority_rejects_stale_cursor_and_context_digest() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "s1").unwrap();
    authority_baseline(&journal);
    let stale_cursor = budget_authority(
        &journal,
        1,
        1_000,
        BudgetWallClockAuthority::ActiveRuntime,
        None,
    );
    journal.append(turn_started("turn")).unwrap();
    assert!(matches!(
        commit_budget_authority(&journal, stale_cursor),
        Err(JournalError::InvalidTransition(message))
            if message.contains("prior cursor")
    ));

    let old_context_digest = state_payload_digest(&serde_json::Value::Array(Vec::new())).unwrap();
    journal
        .append(message_committed(
            "turn",
            0,
            json!({"role": "user", "content": "durable"}),
        ))
        .unwrap();
    let mut stale_context = budget_authority(
        &journal,
        1,
        1_001,
        BudgetWallClockAuthority::ActiveRuntime,
        Some("turn"),
    );
    stale_context.conversation_digest = old_context_digest;
    assert!(matches!(
        commit_budget_authority(&journal, stale_context),
        Err(JournalError::InvalidTransition(message))
            if message.contains("conversation digest")
    ));
}

#[test]
fn budget_authority_rejects_malformed_typed_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "s1").unwrap();
    authority_baseline(&journal);
    let authority = budget_authority(
        &journal,
        1,
        1_000,
        BudgetWallClockAuthority::ActiveRuntime,
        None,
    );

    let mut provider_json = serde_json::to_value(SessionEvent::BudgetAuthorityCommitted {
        authority: authority.clone(),
    })
    .unwrap();
    provider_json["authority"]["provider_tracker"]["schema_version"] = json!(999);
    let invalid_provider: SessionEvent = serde_json::from_value(provider_json).unwrap();
    assert!(matches!(
        journal.append(invalid_provider),
        Err(JournalError::InvalidTransition(message))
            if message.contains("provider snapshot is invalid")
    ));

    let mut execution_json =
        serde_json::to_value(SessionEvent::BudgetAuthorityCommitted { authority }).unwrap();
    execution_json["authority"]["execution_root"]["states"] = json!([]);
    let invalid_execution: SessionEvent = serde_json::from_value(execution_json).unwrap();
    assert!(matches!(
        journal.append(invalid_execution),
        Err(JournalError::InvalidTransition(message))
            if message.contains("execution root snapshot is invalid")
    ));
}

#[test]
fn budget_authority_epoch_identity_time_and_deadline_cannot_regress() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "s1").unwrap();
    authority_baseline(&journal);
    commit_budget_authority(
        &journal,
        budget_authority(
            &journal,
            1,
            2_000,
            BudgetWallClockAuthority::AbsoluteDeadline {
                deadline_unix_millis: 62_000,
            },
            None,
        ),
    )
    .unwrap();

    let mut wrong_epoch = budget_authority(
        &journal,
        3,
        2_001,
        BudgetWallClockAuthority::AbsoluteDeadline {
            deadline_unix_millis: 62_000,
        },
        None,
    );
    assert!(matches!(
        commit_budget_authority(&journal, wrong_epoch.clone()),
        Err(JournalError::InvalidTransition(message)) if message.contains("epoch regression or gap")
    ));

    wrong_epoch.authority_epoch = 2;
    wrong_epoch.budget_session_id = "replacement-session".to_owned();
    assert!(matches!(
        commit_budget_authority(&journal, wrong_epoch),
        Err(JournalError::InvalidTransition(message)) if message.contains("identity changed")
    ));

    let regressed_time = budget_authority(
        &journal,
        2,
        1_999,
        BudgetWallClockAuthority::AbsoluteDeadline {
            deadline_unix_millis: 62_000,
        },
        None,
    );
    assert!(matches!(
        commit_budget_authority(&journal, regressed_time),
        Err(JournalError::InvalidTransition(message)) if message.contains("capture time regressed")
    ));

    let widened_deadline = budget_authority(
        &journal,
        2,
        2_001,
        BudgetWallClockAuthority::AbsoluteDeadline {
            deadline_unix_millis: 62_001,
        },
        None,
    );
    assert!(matches!(
        commit_budget_authority(&journal, widened_deadline),
        Err(JournalError::InvalidTransition(message)) if message.contains("deadline was widened")
    ));

    let changed_semantics = budget_authority(
        &journal,
        2,
        2_001,
        BudgetWallClockAuthority::ActiveRuntime,
        None,
    );
    assert!(matches!(
        commit_budget_authority(&journal, changed_semantics),
        Err(JournalError::InvalidTransition(message)) if message.contains("semantics changed")
    ));
}

#[test]
fn budget_authority_retains_active_turn_until_that_turn_is_terminal() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "s1").unwrap();
    authority_baseline(&journal);
    commit_budget_authority(
        &journal,
        budget_authority(
            &journal,
            1,
            1_000,
            BudgetWallClockAuthority::ActiveRuntime,
            None,
        ),
    )
    .unwrap();
    journal.append(turn_started("turn")).unwrap();
    commit_budget_authority(
        &journal,
        budget_authority(
            &journal,
            2,
            1_001,
            BudgetWallClockAuthority::ActiveRuntime,
            Some("turn"),
        ),
    )
    .unwrap();

    let dropped_active_turn = budget_authority(
        &journal,
        3,
        1_002,
        BudgetWallClockAuthority::ActiveRuntime,
        None,
    );
    assert!(matches!(
        commit_budget_authority(&journal, dropped_active_turn),
        Err(JournalError::InvalidTransition(message))
            if message.contains("dropped active-turn state")
    ));

    journal.append(turn_committed("turn")).unwrap();
    commit_budget_authority(
        &journal,
        budget_authority(
            &journal,
            3,
            1_003,
            BudgetWallClockAuthority::ActiveRuntime,
            None,
        ),
    )
    .unwrap();
    assert!(
        journal
            .state()
            .unwrap()
            .budget_authority
            .unwrap()
            .active_turn
            .is_none()
    );
}

#[test]
fn budget_authority_active_turn_must_be_active_and_keep_root_ancestry() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "s1").unwrap();
    authority_baseline(&journal);
    let unknown_turn = budget_authority(
        &journal,
        1,
        1_000,
        BudgetWallClockAuthority::ActiveRuntime,
        Some("unknown"),
    );
    assert!(matches!(
        commit_budget_authority(&journal, unknown_turn),
        Err(JournalError::InvalidTransition(message)) if message.contains("unknown turn")
    ));

    journal.append(turn_started("turn")).unwrap();
    let mut root_only = budget_authority(
        &journal,
        1,
        1_001,
        BudgetWallClockAuthority::ActiveRuntime,
        Some("turn"),
    );
    root_only.active_turn.as_mut().unwrap().execution = root_only.execution_root.clone();
    assert!(matches!(
        commit_budget_authority(&journal, root_only),
        Err(JournalError::InvalidTransition(message)) if message.contains("root ancestor")
    ));
}
