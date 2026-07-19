#[test]
fn provider_request_digest_is_stable_for_the_exact_request() {
    let request = LlmRequest {
        model: "model-a".into(),
        system: "system".into(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "question".into(),
            }],
        )],
        max_tokens: 1_024,
        conversation_id: Some("conversation-1".into()),
        client_context_tokens: Some(42),
        ..LlmRequest::default()
    };

    let first = provider_request_digest(&request).unwrap();
    let second = provider_request_digest(&request.clone()).unwrap();
    assert_eq!(first, second);
    assert_eq!(first.len(), 64);
}

#[test]
fn provider_request_digest_changes_when_wire_relevant_input_changes() {
    let request = LlmRequest {
        model: "model-a".into(),
        system: "system".into(),
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "question".into(),
            }],
        )],
        max_tokens: 1_024,
        ..LlmRequest::default()
    };
    let original = provider_request_digest(&request).unwrap();

    let mut changed_message = request.clone();
    changed_message.messages[0].content = vec![ContentBlock::Text {
        text: "different question".into(),
    }];
    assert_ne!(original, provider_request_digest(&changed_message).unwrap());

    let mut changed_model = request.clone();
    changed_model.model = "model-b".into();
    assert_ne!(original, provider_request_digest(&changed_model).unwrap());

    let mut changed_limit = request;
    changed_limit.max_tokens += 1;
    assert_ne!(original, provider_request_digest(&changed_limit).unwrap());
}

#[test]
fn prepared_provider_request_snapshot_round_trips_every_request_field() {
    let timestamp = "2026-07-16T01:02:03Z".parse().unwrap();
    let request = LlmRequest {
        model: "model-a".into(),
        system: "system prompt".into(),
        messages: vec![Message {
            role: Role::Assistant,
            content: vec![
                ContentBlock::Text {
                    text: "text".into(),
                },
                ContentBlock::ToolUse {
                    id: "tool-1".into(),
                    name: "read".into(),
                    input: json!({"path":"README.md"}),
                    extra: Some(json!({"thought_signature":"opaque"})),
                },
                ContentBlock::ToolResult {
                    tool_use_id: "tool-1".into(),
                    content: "result".into(),
                    is_error: true,
                },
                ContentBlock::Thinking {
                    thinking: "reasoning".into(),
                },
                ContentBlock::Image {
                    mime: "image/png".into(),
                    data: "aGVsbG8=".into(),
                },
            ],
            timestamp: Some(timestamp),
            cache_breakpoint: Some(MessageCacheHint::Breakpoint),
        }],
        tools: vec![ToolDef {
            name: "read".into(),
            description: "Read a file".into(),
            input_schema: json!({
                "type":"object",
                "properties":{"path":{"type":"string"}},
                "required":["path"]
            }),
            deferred: true,
            server: Some("filesystem".into()),
        }],
        max_tokens: 4_096,
        thinking: Some(ThinkingConfig::Enabled {
            budget_tokens: 1_024,
        }),
        reasoning_effort: Some("high".into()),
        cache_tier: Some(CacheTier::Ephemeral1h),
        routing_hint: Some(RoutingHint::new("frontier")),
        stop_sequences: vec!["STOP".into()],
        web_search: true,
        conversation_id: Some("conversation-1".into()),
        client_context_tokens: Some(12_345),
        temperature: Some(0.25),
        omit_max_tokens: true,
    };

    let snapshot = prepared_provider_request_snapshot(&request).unwrap();
    let restored = decode_prepared_provider_request_snapshot(&snapshot).unwrap();
    let restored_snapshot = prepared_provider_request_snapshot(&restored).unwrap();

    assert_eq!(snapshot, restored_snapshot);
    assert_eq!(
        provider_request_digest(&request).unwrap(),
        provider_request_digest(&restored).unwrap()
    );
}

#[test]
fn prepared_provider_request_snapshot_rejects_unknown_structural_fields() {
    let request = LlmRequest {
        messages: vec![Message::new(
            Role::User,
            vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        )],
        tools: vec![ToolDef {
            name: "read".into(),
            ..ToolDef::default()
        }],
        thinking: Some(ThinkingConfig::Disabled),
        ..LlmRequest::default()
    };
    let snapshot = prepared_provider_request_snapshot(&request).unwrap();

    for path in ["root", "request", "message", "content", "tool", "thinking"] {
        let mut changed = snapshot.clone();
        match path {
            "root" => changed["unknown"] = json!(true),
            "request" => changed["request"]["unknown"] = json!(true),
            "message" => changed["request"]["messages"][0]["unknown"] = json!(true),
            "content" => {
                changed["request"]["messages"][0]["content"][0]["unknown"] = json!(true);
            }
            "tool" => changed["request"]["tools"][0]["unknown"] = json!(true),
            "thinking" => changed["request"]["thinking"]["unknown"] = json!(true),
            _ => unreachable!(),
        }
        assert!(
            decode_prepared_provider_request_snapshot(&changed).is_err(),
            "unknown {path} field was accepted"
        );
    }
}

#[test]
fn prepared_provider_request_snapshot_rejects_version_and_malformed_fields() {
    let snapshot = prepared_provider_request_snapshot(&LlmRequest::default()).unwrap();

    let mut unsupported = snapshot.clone();
    unsupported["version"] = json!(2);
    assert!(matches!(
        decode_prepared_provider_request_snapshot(&unsupported),
        Err(JournalError::InvalidTransition(message))
            if message.contains("unsupported prepared provider request snapshot version 2")
    ));

    let mut missing = snapshot.clone();
    missing["request"].as_object_mut().unwrap().remove("model");
    assert!(matches!(
        decode_prepared_provider_request_snapshot(&missing),
        Err(JournalError::Json {
            context: "decoding prepared provider request snapshot",
            ..
        })
    ));

    let mut malformed = snapshot;
    malformed["request"]["max_tokens"] = json!("unbounded");
    assert!(decode_prepared_provider_request_snapshot(&malformed).is_err());

    let noncanonical = prepared_provider_request_snapshot(&LlmRequest {
        temperature: Some(1.0),
        ..LlmRequest::default()
    })
    .unwrap();
    let mut noncanonical = noncanonical;
    noncanonical["request"]["temperature"] = json!(1);
    assert!(matches!(
        decode_prepared_provider_request_snapshot(&noncanonical),
        Err(JournalError::InvalidTransition(message))
            if message == "prepared provider request snapshot is not canonical"
    ));

    assert!(matches!(
        prepared_provider_request_snapshot(&LlmRequest {
            temperature: Some(f32::NAN),
            ..LlmRequest::default()
        }),
        Err(JournalError::InvalidTransition(message))
            if message == "prepared provider request temperature must be finite"
    ));
}

#[test]
fn append_is_contiguous_checksummed_and_exclusively_owned() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    std::fs::write(&path, []).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }
    let first = SessionJournal::open(&path, "s1").unwrap();
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::AlreadyOwned { .. })
    ));
    let second = first.clone();
    let zero = first.append(turn_started("t0")).unwrap();
    let one = second.append(turn_committed("t0")).unwrap();
    assert_eq!((zero.seq, one.seq), (0, 1));
    assert_eq!(zero.previous_checksum, GENESIS_CHECKSUM);
    assert_eq!(one.previous_checksum, zero.checksum);
    assert_eq!(SessionJournal::replay(&path).unwrap(), vec![zero, one]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    drop(first);
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::AlreadyOwned { .. })
    ));
    drop(second);
    assert!(SessionJournal::open(&path, "s1").is_ok());
    assert!(
        dir.path().join("session.journal.writer.lock").exists(),
        "the advisory-lock sentinel must be retained to avoid an unlink/recreate race"
    );
    assert_eq!(
        std::fs::metadata(dir.path().join("session.journal.writer.lock"))
            .unwrap()
            .len(),
        0,
        "released sentinels must not retain stale owner metadata"
    );
}

#[test]
fn hard_link_alias_cannot_acquire_a_second_writer_authority() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let alias = dir.path().join("alias.journal");
    let owner = SessionJournal::open(&path, "s1").unwrap();
    std::fs::hard_link(&path, &alias).unwrap();

    assert!(matches!(
        SessionJournal::open(&alias, "s1"),
        Err(JournalError::MultipleLinks { .. })
    ));

    assert!(matches!(
        owner.compact(),
        Err(JournalError::MultipleLinks { .. })
    ));
    drop(owner);
    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::MultipleLinks { .. })
    ));
    assert!(matches!(
        SessionJournal::open(&alias, "s1"),
        Err(JournalError::MultipleLinks { .. })
    ));
    std::fs::remove_file(alias).unwrap();
    assert!(SessionJournal::open(path, "s1").is_ok());
}

#[test]
fn compacted_replacement_keeps_the_data_inode_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let alias = dir.path().join("alias.journal");
    let owner = SessionJournal::open(&path, "s1").unwrap();
    owner.append(turn_started("t0")).unwrap();
    owner.compact().unwrap();
    std::fs::hard_link(&path, &alias).unwrap();
    std::fs::remove_file(&path).unwrap();

    assert!(matches!(
        SessionJournal::open(&alias, "s1"),
        Err(JournalError::AlreadyOwned { .. })
    ));

    drop(owner);
    assert!(SessionJournal::open(alias, "s1").is_ok());
}

#[cfg(unix)]
#[test]
fn symlink_alias_is_rejected_without_mutating_its_target() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let alias = dir.path().join("alias.journal");
    let owner = SessionJournal::open(&path, "s1").unwrap();
    symlink(&path, &alias).unwrap();

    assert!(matches!(
        SessionJournal::open(&alias, "s1"),
        Err(JournalError::SymbolicLink { .. })
    ));

    drop(owner);
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        SessionJournal::open(alias, "s1"),
        Err(JournalError::SymbolicLink { .. })
    ));
    assert_eq!(std::fs::read(path).unwrap(), before);
}

#[cfg(unix)]
#[test]
fn writer_lease_symlink_is_rejected_without_mutating_its_target() {
    use std::os::unix::fs::{symlink, PermissionsExt};

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let lease_path = dir.path().join("session.journal.writer.lock");
    let target = dir.path().join("protected");
    std::fs::write(&target, b"must remain unchanged").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o640)).unwrap();
    symlink(&target, &lease_path).unwrap();

    assert!(matches!(
        SessionJournal::open(&path, "s1"),
        Err(JournalError::SymbolicLink { path }) if path == lease_path
    ));
    assert_eq!(std::fs::read(&target).unwrap(), b"must remain unchanged");
    assert_eq!(
        std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
        0o640
    );
}

const TEST_CHILD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

fn wait_for_child_path(
    child: &mut std::process::Child,
    path: &Path,
    description: &str,
) -> Result<(), String> {
    let deadline = std::time::Instant::now() + TEST_CHILD_TIMEOUT;
    loop {
        if path.exists() {
            return Ok(());
        }
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("could not inspect {description}: {error}"))?
        {
            return Err(format!("{description} exited early with {status}"));
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "timed out after {TEST_CHILD_TIMEOUT:?} waiting for {description}"
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

fn wait_for_child_exit(
    child: &mut std::process::Child,
    description: &str,
) -> Result<std::process::ExitStatus, String> {
    let deadline = std::time::Instant::now() + TEST_CHILD_TIMEOUT;
    loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("could not inspect {description}: {error}"))?
        {
            return Ok(status);
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "timed out after {TEST_CHILD_TIMEOUT:?} waiting for {description}"
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn lease_holder_process_exits_without_drop() {
    let Ok(path) = std::env::var("WCORE_TEST_JOURNAL_LEASE_PATH") else {
        return;
    };
    let _journal = SessionJournal::open(path, "crash-owner").unwrap();
    let ready = std::env::var("WCORE_TEST_JOURNAL_LEASE_READY").unwrap();
    let release = std::env::var("WCORE_TEST_JOURNAL_LEASE_RELEASE").unwrap();
    std::fs::write(ready, b"ready").unwrap();
    let deadline = std::time::Instant::now() + TEST_CHILD_TIMEOUT;
    loop {
        if std::path::Path::new(&release).exists() {
            std::process::exit(0);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "parent did not release lease-holder child before {TEST_CHILD_TIMEOUT:?}"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn operating_system_releases_writer_lease_after_process_exit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let ready = dir.path().join("lease.ready");
    let release = dir.path().join("lease.release");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lease_holder_process_exits_without_drop"])
        .env("WCORE_TEST_JOURNAL_LEASE_PATH", &path)
        .env("WCORE_TEST_JOURNAL_LEASE_READY", &ready)
        .env("WCORE_TEST_JOURNAL_LEASE_RELEASE", &release)
        .spawn()
        .unwrap();

    wait_for_child_path(&mut child, &ready, "lease-holder child").unwrap();
    assert!(matches!(
        SessionJournal::open(&path, "crash-owner"),
        Err(JournalError::AlreadyOwned { .. })
    ));

    std::fs::write(release, b"release").unwrap();
    let status = wait_for_child_exit(&mut child, "lease-holder child").unwrap();
    assert!(status.success());
    assert!(SessionJournal::open(path, "crash-owner").is_ok());
}

#[cfg(unix)]
#[test]
fn read_only_authority_replay_subprocess() {
    let Ok(path) = std::env::var("WCORE_TEST_READ_ONLY_JOURNAL_PATH") else {
        return;
    };
    if unsafe { libc::geteuid() } == 0 {
        // SAFETY: this test runs alone in a dedicated subprocess. Dropping its
        // credentials prevents root from bypassing the read-only fixture.
        assert_eq!(unsafe { libc::setgroups(0, std::ptr::null()) }, 0);
        // SAFETY: the numeric nobody credentials are used only in this child.
        assert_eq!(unsafe { libc::setgid(65_534) }, 0);
        // SAFETY: dropping the child UID cannot affect the parent test process.
        assert_eq!(unsafe { libc::setuid(65_534) }, 0);
    }
    SessionJournal::recovered_state(path).unwrap();
}

#[cfg(unix)]
#[test]
fn replay_accepts_read_only_authority_files() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("t0")).unwrap();
    journal.publish_snapshot().unwrap();
    drop(journal);

    let snapshot_path = snapshot_path_for(&path);
    let authority_path = dir.path().join("session.journal.authority");
    for authority_file in [&path, &snapshot_path, &authority_path] {
        std::fs::set_permissions(authority_file, std::fs::Permissions::from_mode(0o400)).unwrap();
    }
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
    if unsafe { libc::geteuid() } == 0 {
        use std::os::unix::ffi::OsStrExt as _;

        for authority_path in [dir.path(), &path, &snapshot_path, &authority_path] {
            let authority_path = std::ffi::CString::new(authority_path.as_os_str().as_bytes())
                .expect("temporary authority path must not contain NUL");
            // SAFETY: the path is a live test fixture and the parent retains
            // root authority to restore and remove it after the child exits.
            assert_eq!(
                unsafe { libc::chown(authority_path.as_ptr(), 65_534, 65_534) },
                0
            );
        }
    }

    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "read_only_authority_replay_subprocess"])
        .env("WCORE_TEST_READ_ONLY_JOURNAL_PATH", &path)
        .spawn()
        .unwrap();
    let result = wait_for_child_exit(&mut child, "read-only replay child");

    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    for authority_file in [&path, &snapshot_path, &authority_path] {
        std::fs::set_permissions(authority_file, std::fs::Permissions::from_mode(0o600)).unwrap();
    }
    assert!(result.unwrap().success(), "read-only replay child failed");
}

#[test]
fn torn_tail_is_ignored_healed_and_replaced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    append_events(&path, vec![turn_started("t0")]);
    let torn = frame(br#"{"incomplete":true}"#);
    OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(&torn[..torn.len() - 7])
        .unwrap();
    assert_eq!(SessionJournal::replay(&path).unwrap().len(), 1);
    let journal = SessionJournal::open(&path, "s1").unwrap();
    assert_eq!(journal.append(turn_committed("t0")).unwrap().seq, 1);
    assert_eq!(SessionJournal::replay(&path).unwrap().len(), 2);
}

#[test]
fn complete_corrupt_final_frame_is_a_hard_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    std::fs::write(&path, frame(b"{not json}")).unwrap();
    assert!(matches!(
        SessionJournal::replay(path),
        Err(JournalError::CorruptFrame { frame: 1, .. })
    ));
}

#[test]
fn complete_frame_digest_corruption_is_a_hard_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let mut bytes = frame(br#"{"valid":"json"}"#);
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        SessionJournal::replay(path),
        Err(JournalError::FrameDigestMismatch { frame: 1, .. })
    ));
}

#[test]
fn checksum_sequence_previous_and_schema_tampering_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let entries = append_events(
        &dir.path().join("session.journal"),
        vec![turn_started("t0"), turn_committed("t0")],
    );
    let zero = entries[0].clone();
    let one = entries[1].clone();

    let mut bad_checksum = zero.clone();
    bad_checksum.checksum = "bad".into();
    assert!(matches!(
        verify_chain(&[bad_checksum]),
        Err(JournalError::ChecksumMismatch { .. })
    ));

    let mut gap = one.clone();
    gap.seq = 2;
    assert!(matches!(
        verify_chain(&[zero.clone(), gap]),
        Err(JournalError::SequenceMismatch { .. })
    ));

    let mut wrong_previous = one.clone();
    wrong_previous.previous_checksum = GENESIS_CHECKSUM.into();
    assert!(matches!(
        verify_chain(&[zero.clone(), wrong_previous]),
        Err(JournalError::PreviousChecksumMismatch { .. })
    ));
    assert!(matches!(
        verify_chain(&[one]),
        Err(JournalError::SequenceMismatch { .. })
    ));

    let mut future = zero;
    future.schema_version = SESSION_JOURNAL_SCHEMA_VERSION + 1;
    assert!(matches!(
        verify_chain(&[future]),
        Err(JournalError::UnsupportedSchema { .. })
    ));

    let mut obsolete = entries[0].clone();
    obsolete.schema_version = SESSION_JOURNAL_SCHEMA_VERSION - 2;
    assert!(matches!(
        verify_chain(&[obsolete]),
        Err(JournalError::UnsupportedSchema { .. })
    ));
}

#[test]
fn public_replay_boundary_enforces_forward_only_schema_history() {
    #[derive(serde::Serialize)]
    struct ChecksumMaterial<'a> {
        schema_version: u32,
        session_id: &'a str,
        seq: u64,
        previous_checksum: &'a str,
        event: &'a SessionEvent,
    }

    fn with_schema(mut envelope: JournalEnvelope, schema_version: u32) -> JournalEnvelope {
        envelope.schema_version = schema_version;
        let body = serde_json::to_vec(&ChecksumMaterial {
            schema_version,
            session_id: &envelope.session_id,
            seq: envelope.seq,
            previous_checksum: &envelope.previous_checksum,
            event: &envelope.event,
        })
        .unwrap();
        envelope.checksum = format!("{:x}", Sha256::digest(body));
        envelope
    }

    assert_eq!(SESSION_JOURNAL_SCHEMA_VERSION, 5);
    let dir = tempfile::tempdir().unwrap();
    let entries = append_events(
        &dir.path().join("public-schema-boundary.journal"),
        vec![turn_started("t0"), turn_committed("t0")],
    );

    let current = entries[0].clone();
    let regressed = with_schema(entries[1].clone(), 4);
    assert!(matches!(
        replay_state(&[current, regressed]),
        Err(JournalError::SchemaRegression {
            previous: 5,
            found: 4,
        })
    ));

    let legacy = with_schema(entries[0].clone(), 4);
    let mut upgraded = entries[1].clone();
    upgraded.previous_checksum.clone_from(&legacy.checksum);
    let upgraded = with_schema(upgraded, 5);
    assert_eq!(replay_state(&[legacy, upgraded]).unwrap().last_seq, Some(1));
}

#[test]
fn unreleased_v3_journal_schema_is_explicitly_rejected_before_event_decode() {
    assert_eq!(SESSION_JOURNAL_SCHEMA_VERSION, 5);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("v3.journal");
    let obsolete = serde_json::to_vec(&json!({
        "schema_version": 3,
        "session_id": "s1",
        "seq": 0,
        "previous_checksum": GENESIS_CHECKSUM,
        "event": {
            "type": "stream_delta_committed",
            "stream_id": "stream",
            "ordinal": 0,
            "content": "lossy-v1"
        },
        "checksum": "irrelevant-for-unsupported-schema"
    }))
    .unwrap();
    std::fs::write(&path, frame(&obsolete)).unwrap();
    assert!(matches!(
        SessionJournal::replay(path),
        Err(JournalError::UnsupportedSchema {
            found: 3,
            supported: 5
        })
    ));
}

#[test]
fn started_tool_is_running_while_other_unresolved_effects_remain_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let entries = append_events(
        &dir.path().join("session.journal"),
        vec![
            turn_started("turn"),
            provider_prepared("p", "turn"),
            SessionEvent::ProviderAttemptStarted {
                attempt_id: "p".into(),
            },
            tool_intent(
                "tool-exec",
                "provider-call",
                "turn",
                0,
                "bash",
                json!({"cmd":"true"}),
                json!({"cmd":"true"}),
            ),
            SessionEvent::ToolExecutionStarted {
                tool_execution_id: "tool-exec".into(),
            },
            SessionEvent::ChildPrepared {
                child_id: "c".into(),
                turn_id: "turn".into(),
                request: json!({"task":"x"}),
            },
            SessionEvent::ChildStarted {
                child_id: "c".into(),
            },
            SessionEvent::DeliveryPrepared {
                delivery_id: "d".into(),
                origin: DeliveryOrigin::Turn {
                    turn_id: "turn".into(),
                },
                destination: "host".into(),
                payload: json!({"text":"x"}),
            },
            SessionEvent::DeliveryStarted {
                delivery_id: "d".into(),
            },
        ],
    );
    let state = replay_state(&entries).unwrap();
    for effect in [
        &state.provider_attempts["p"].effect,
        &state.children["c"].effect,
        &state.deliveries["d"].effect,
    ] {
        assert_eq!(effect, &ExternalEffectState::Unknown);
        assert!(effect.requires_reconciliation());
    }
    assert_eq!(state.tools["tool-exec"].effect, ToolEffectState::Running);
    assert!(state.tools["tool-exec"].effect.requires_reconciliation());
}

#[test]
fn full_replay_equals_snapshot_plus_suffix() {
    let dir = tempfile::tempdir().unwrap();
    let entries = append_events(
        &dir.path().join("session.journal"),
        vec![
            SessionEvent::TurnStarted {
                turn_id: "t".into(),
                user_message: "hello".into(),
            },
            provider_prepared("p", "t"),
            SessionEvent::ProviderAttemptStarted {
                attempt_id: "p".into(),
            },
            SessionEvent::StreamStarted {
                stream_id: "s".into(),
                attempt_id: "p".into(),
            },
            text_batch("s", 0, "done"),
            SessionEvent::StreamFinished {
                stream_id: "s".into(),
            },
            SessionEvent::ProviderAttemptFinished {
                attempt_id: "p".into(),
                outcome: CompletionOutcome::Succeeded,
                response_digest: Some("response".into()),
            },
            SessionEvent::TurnCommitted {
                turn_id: "t".into(),
                assistant_message: "done".into(),
            },
        ],
    );
    let full = replay_state(&entries).unwrap();
    let snapshot = SessionSnapshot::new("s1", replay_state(&entries[..5]).unwrap()).unwrap();
    assert_eq!(
        full,
        replay_from_snapshot(&snapshot, &entries[5..]).unwrap()
    );
}

#[test]
fn provider_stream_requires_started_attempt_and_preserves_ordered_structured_batches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("turn")).unwrap();

    assert!(matches!(
        journal.append(provider_prepared("orphan", "missing-turn")),
        Err(JournalError::InvalidTransition(_))
    ));

    let stream_started = SessionEvent::StreamStarted {
        stream_id: "stream".into(),
        attempt_id: "attempt".into(),
    };
    assert!(matches!(
        journal.append(stream_started.clone()),
        Err(JournalError::InvalidTransition(_))
    ));

    journal
        .append(provider_prepared("attempt", "turn"))
        .unwrap();
    assert!(matches!(
        journal.append(stream_started.clone()),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::ProviderAttemptStarted {
            attempt_id: "attempt".into(),
        })
        .unwrap();
    journal.append(stream_started).unwrap();
    assert!(matches!(
        journal.append(SessionEvent::StreamStarted {
            stream_id: "other".into(),
            attempt_id: "attempt".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    assert!(matches!(
        journal.append(SessionEvent::StreamBatchCommitted {
            stream_id: "stream".into(),
            ordinal: 0,
            events: vec![],
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    assert!(matches!(
        journal.append(text_batch("stream", 1, "gap")),
        Err(JournalError::InvalidTransition(_))
    ));

    let batch = vec![
        ProviderStreamEvent::ThinkingDelta {
            text: "reason".into(),
        },
        ProviderStreamEvent::ToolUse {
            id: "call".into(),
            name: "read".into(),
            input: json!({"path":"README.md"}),
            extra: Some(json!({"signature":"opaque"})),
        },
        ProviderStreamEvent::Done {
            stop_reason: json!("tool_use"),
            finish_reason: json!("tool_calls"),
            usage: json!({"input_tokens":10,"output_tokens":2}),
        },
    ];
    journal
        .append(SessionEvent::StreamBatchCommitted {
            stream_id: "stream".into(),
            ordinal: 0,
            events: batch.clone(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ProviderAttemptFinished {
            attempt_id: "attempt".into(),
            outcome: CompletionOutcome::Succeeded,
            response_digest: Some("response".into()),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::StreamFinished {
            stream_id: "stream".into(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(text_batch("stream", 1, "late")),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::ProviderAttemptFinished {
            attempt_id: "attempt".into(),
            outcome: CompletionOutcome::Succeeded,
            response_digest: Some("response".into()),
        })
        .unwrap();

    journal
        .append(SessionEvent::ProviderAttemptPrepared {
            attempt_id: "compaction".into(),
            turn_id: "turn".into(),
            purpose: ProviderAttemptPurpose::Compaction,
            provider: "x".into(),
            model: "m".into(),
            request_digest: "compact-request".into(),
        })
        .unwrap();
    let state = journal.state().unwrap();
    assert_eq!(state.streams["stream"].batches, vec![batch.clone()]);
    assert_eq!(state.provider_attempts["attempt"].turn_id, "turn");
    assert_eq!(
        state.provider_attempts["compaction"].purpose,
        ProviderAttemptPurpose::Compaction
    );
    let replayed = replay_state(&SessionJournal::replay(&path).unwrap()).unwrap();
    assert_eq!(replayed.streams["stream"].batches, vec![batch]);
}

#[test]
fn approval_linkage_and_terminal_resolution_are_enforced() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("turn")).unwrap();
    journal
        .append(tool_intent(
            "exec",
            "call",
            "turn",
            0,
            "bash",
            json!({"cmd":"true"}),
            json!({"cmd":"true"}),
        ))
        .unwrap();

    assert!(matches!(
        journal.append(SessionEvent::ApprovalRequested {
            approval_id: "missing".into(),
            origin: ApprovalOrigin::ToolExecution {
                tool_execution_id: "unknown".into(),
            },
            intent_digest: "intent".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::ApprovalRequested {
            approval_id: "approval".into(),
            origin: ApprovalOrigin::ToolExecution {
                tool_execution_id: "exec".into(),
            },
            intent_digest: "intent".into(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ApprovalRequested {
            approval_id: "duplicate-origin".into(),
            origin: ApprovalOrigin::ToolExecution {
                tool_execution_id: "exec".into(),
            },
            intent_digest: "intent".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    let resolved = SessionEvent::ApprovalResolved {
        approval_id: "approval".into(),
        resolution: ApprovalResolution::TimedOut,
    };
    journal.append(resolved.clone()).unwrap();
    assert!(matches!(
        journal.append(resolved),
        Err(JournalError::InvalidTransition(_))
    ));
    let state = journal.state().unwrap();
    assert_eq!(
        state.approvals["approval"].origin,
        ApprovalOrigin::ToolExecution {
            tool_execution_id: "exec".into(),
        }
    );
    assert_eq!(
        state.approvals["approval"].resolution,
        Some(ApprovalResolution::TimedOut)
    );

    journal
        .append(tool_intent(
            "exec-cancel",
            "call-cancel",
            "turn",
            1,
            "bash",
            json!({}),
            json!({}),
        ))
        .unwrap();
    journal
        .append(SessionEvent::ApprovalRequested {
            approval_id: "cancelled".into(),
            origin: ApprovalOrigin::ToolExecution {
                tool_execution_id: "exec-cancel".into(),
            },
            intent_digest: "cancel-intent".into(),
        })
        .unwrap();
    journal
        .append(SessionEvent::ApprovalResolved {
            approval_id: "cancelled".into(),
            resolution: ApprovalResolution::Cancelled,
        })
        .unwrap();

    journal
        .append(tool_intent(
            "exec-allow",
            "call-allow",
            "turn",
            2,
            "bash",
            json!({}),
            json!({}),
        ))
        .unwrap();
    journal
        .append(SessionEvent::ApprovalRequested {
            approval_id: "allowed".into(),
            origin: ApprovalOrigin::ToolExecution {
                tool_execution_id: "exec-allow".into(),
            },
            intent_digest: "allow-intent".into(),
        })
        .unwrap();
    journal
        .append(SessionEvent::ApprovalResolved {
            approval_id: "allowed".into(),
            resolution: ApprovalResolution::Decided {
                decision: ApprovalDecision::AllowOnce,
            },
        })
        .unwrap();

    journal
        .append(provider_prepared("attempt", "turn"))
        .unwrap();
    journal
        .append(SessionEvent::ApprovalRequested {
            approval_id: "provider-approval".into(),
            origin: ApprovalOrigin::ProviderAttempt {
                attempt_id: "attempt".into(),
            },
            intent_digest: "provider-intent".into(),
        })
        .unwrap();
}

#[test]
fn children_and_deliveries_distinguish_prepared_from_started_unknown() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("session.journal");
    let journal = SessionJournal::open(&path, "s1").unwrap();
    journal.append(turn_started("turn")).unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ChildStarted {
            child_id: "child".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::ChildPrepared {
            child_id: "child".into(),
            turn_id: "turn".into(),
            request: json!({"task":"inspect"}),
        })
        .unwrap();
    assert_eq!(
        journal.state().unwrap().children["child"].effect,
        ExternalEffectState::Prepared
    );
    journal
        .append(SessionEvent::ChildStarted {
            child_id: "child".into(),
        })
        .unwrap();
    assert_eq!(
        journal.state().unwrap().children["child"].effect,
        ExternalEffectState::Unknown
    );
    journal
        .append(SessionEvent::ChildFinished {
            child_id: "child".into(),
            outcome: CompletionOutcome::Succeeded,
            result: json!({"answer":"done"}),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ChildStarted {
            child_id: "child".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    assert!(matches!(
        journal.append(SessionEvent::DeliveryStarted {
            delivery_id: "delivery".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::DeliveryPrepared {
            delivery_id: "delivery".into(),
            origin: DeliveryOrigin::Turn {
                turn_id: "turn".into(),
            },
            destination: "host".into(),
            payload: json!({"text":"hello"}),
        })
        .unwrap();
    assert_eq!(
        journal.state().unwrap().deliveries["delivery"].effect,
        ExternalEffectState::Prepared
    );
    journal
        .append(SessionEvent::DeliveryStarted {
            delivery_id: "delivery".into(),
        })
        .unwrap();
    assert_eq!(
        journal.state().unwrap().deliveries["delivery"].effect,
        ExternalEffectState::Unknown
    );
    journal
        .append(SessionEvent::DeliveryFinished {
            delivery_id: "delivery".into(),
            completion: DeliveryCompletion::Confirmed {
                outcome: CompletionOutcome::Succeeded,
                receipt: json!({"accepted":true}),
            },
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::DeliveryStarted {
            delivery_id: "delivery".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    journal
        .append(SessionEvent::DeliveryPrepared {
            delivery_id: "delivery-denied".into(),
            origin: DeliveryOrigin::Turn {
                turn_id: "turn".into(),
            },
            destination: "host".into(),
            payload: json!({"text":"blocked"}),
        })
        .unwrap();
    let denied = DeliveryNotStartedReason::PolicyDenied {
        policy: "managed".into(),
    };
    journal
        .append(SessionEvent::DeliveryNotStarted {
            delivery_id: "delivery-denied".into(),
            reason: denied.clone(),
        })
        .unwrap();
    let denied_state = &journal.state().unwrap().deliveries["delivery-denied"];
    assert_eq!(denied_state.effect, ExternalEffectState::NotStarted);
    assert_eq!(denied_state.not_started_reason, Some(denied));
    assert!(matches!(
        journal.append(SessionEvent::DeliveryStarted {
            delivery_id: "delivery-denied".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
}

#[test]
fn prepared_provider_tool_and_child_can_finish_without_a_fabricated_start() {
    let dir = tempfile::tempdir().unwrap();
    let journal = SessionJournal::open(dir.path().join("session.journal"), "s1").unwrap();
    journal.append(turn_started("turn")).unwrap();

    journal
        .append(provider_prepared("attempt", "turn"))
        .unwrap();
    let provider_reason = ProviderAttemptNotStartedReason::EgressDenied {
        policy: "network-boundary".into(),
    };
    journal
        .append(SessionEvent::ProviderAttemptNotStarted {
            attempt_id: "attempt".into(),
            reason: provider_reason.clone(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ProviderAttemptStarted {
            attempt_id: "attempt".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(provider_prepared("started-attempt", "turn"))
        .unwrap();
    journal
        .append(SessionEvent::ProviderAttemptStarted {
            attempt_id: "started-attempt".into(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ProviderAttemptNotStarted {
            attempt_id: "started-attempt".into(),
            reason: ProviderAttemptNotStartedReason::Cancelled {
                reason: "too late".into(),
            },
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    let requested = json!({"path":"requested"});
    let effective = json!({"path":"effective"});
    journal
        .append(tool_intent(
            "execution",
            "provider-call",
            "turn",
            0,
            "read",
            requested.clone(),
            effective.clone(),
        ))
        .unwrap();
    let tool_reason = ToolNotStartedReason::ApprovalDenied {
        approval_id: "approval".into(),
    };
    journal
        .append(SessionEvent::ToolExecutionNotStarted {
            tool_execution_id: "execution".into(),
            reason: tool_reason.clone(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ToolExecutionStarted {
            tool_execution_id: "execution".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(tool_intent(
            "started-execution",
            "started-provider-call",
            "turn",
            1,
            "read",
            json!({}),
            json!({}),
        ))
        .unwrap();
    journal
        .append(SessionEvent::ToolExecutionStarted {
            tool_execution_id: "started-execution".into(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ToolExecutionNotStarted {
            tool_execution_id: "started-execution".into(),
            reason: ToolNotStartedReason::Cancelled {
                reason: "too late".into(),
            },
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    journal
        .append(SessionEvent::ChildPrepared {
            child_id: "child".into(),
            turn_id: "turn".into(),
            request: json!({"task":"inspect"}),
        })
        .unwrap();
    let child_reason = ChildNotStartedReason::PolicyDenied {
        policy: "spawn-disabled".into(),
    };
    journal
        .append(SessionEvent::ChildNotStarted {
            child_id: "child".into(),
            reason: child_reason.clone(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ChildStarted {
            child_id: "child".into(),
        }),
        Err(JournalError::InvalidTransition(_))
    ));
    journal
        .append(SessionEvent::ChildPrepared {
            child_id: "started-child".into(),
            turn_id: "turn".into(),
            request: json!({"task":"started"}),
        })
        .unwrap();
    journal
        .append(SessionEvent::ChildStarted {
            child_id: "started-child".into(),
        })
        .unwrap();
    assert!(matches!(
        journal.append(SessionEvent::ChildNotStarted {
            child_id: "started-child".into(),
            reason: ChildNotStartedReason::Cancelled {
                reason: "too late".into(),
            },
        }),
        Err(JournalError::InvalidTransition(_))
    ));

    let state = journal.state().unwrap();
    assert_eq!(
        state.provider_attempts["attempt"].effect,
        ExternalEffectState::NotStarted
    );
    assert_eq!(
        state.provider_attempts["attempt"].not_started_reason,
        Some(provider_reason)
    );
    assert_eq!(state.tools["execution"].provider_call_id, "provider-call");
    assert_eq!(state.tools["execution"].turn_id, "turn");
    assert_eq!(state.tools["execution"].ordinal, 0);
    assert_eq!(
        state.tools["execution"].requested_input.exact_digest(),
        state_payload_digest(&requested).unwrap()
    );
    assert_eq!(
        state.tools["execution"].effective_input.exact_digest(),
        state_payload_digest(&effective).unwrap()
    );
    assert_eq!(state.tools["execution"].effect, ToolEffectState::NotStarted);
    assert_eq!(
        state.tools["execution"].not_started_reason,
        Some(tool_reason)
    );
    assert_eq!(
        state.children["child"].effect,
        ExternalEffectState::NotStarted
    );
    assert_eq!(
        state.children["child"].not_started_reason,
        Some(child_reason)
    );
}
