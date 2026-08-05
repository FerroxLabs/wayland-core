use sha2::{Digest, Sha256};
use wcore_eval_scenarios::fixtures::remote_execution::{
    FixtureArtifact, OutputChannel, RemoteExecutionEventKind, RemoteExecutionFixture,
    RemoteExecutionScript, RemoteFixtureError, RemoteTask, ResourceBudget, ResourceKind,
    ScriptedOutcome, ScriptedOutputEvent, TerminalStatus,
};

fn h64(ch: char) -> String {
    std::iter::repeat_n(ch, 64).collect()
}

fn limits() -> ResourceBudget {
    ResourceBudget::new(2_000, 64 * 1024 * 1024, 30_000, 1024 * 1024).expect("valid fixture limits")
}

fn fixture() -> RemoteExecutionFixture {
    RemoteExecutionFixture::new(
        "fixture-local",
        "worker-01",
        "fixture-v1",
        limits(),
        [23; 32],
    )
    .expect("valid fixture backend")
}

fn task(resources: ResourceBudget) -> RemoteTask {
    RemoteTask::new(
        "task-001",
        h64('a'),
        b"compile the seeded workspace".to_vec(),
        resources,
    )
    .expect("valid fixture task")
}

#[test]
fn accepted_success_is_ordered_content_addressed_and_attested() {
    let fixture = fixture();
    let task = task(ResourceBudget::new(500, 1024 * 1024, 5_000, 4096).unwrap());
    let artifact_bytes = b"deterministic artifact\n".to_vec();
    let artifact_sha256 = format!("{:x}", Sha256::digest(&artifact_bytes));
    let script = RemoteExecutionScript::new(
        [
            ScriptedOutputEvent::new(2, OutputChannel::Stdout, "starting"),
            ScriptedOutputEvent::new(3, OutputChannel::Stderr, "fixture diagnostic"),
        ],
        ScriptedOutcome::success(FixtureArtifact::new("dist/result.txt", artifact_bytes).unwrap()),
    );

    let first = fixture.execute(&task, &script).expect("fixture execution");
    let second = fixture.execute(&task, &script).expect("repeat execution");

    first
        .verify(fixture.identity(), &fixture.verifying_key())
        .expect("pinned fixture attestation");
    assert_eq!(first, second, "same fixture inputs must be byte-stable");
    assert!(matches!(
        &first.attestation.authority,
        wcore_eval_scenarios::fixtures::remote_execution::FixtureAuthority::FixtureOnly
    ));
    assert_eq!(first.body.task.workspace_sha256, h64('a'));
    assert_eq!(first.body.task.input_sha256, task.input_sha256());
    assert_eq!(&first.body.backend, fixture.identity());
    assert_eq!(first.body.events_sha256.len(), 64);
    assert_eq!(
        first
            .body
            .events
            .iter()
            .map(|event| event.sequence)
            .collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    assert!(matches!(
        &first.body.events[0].event,
        RemoteExecutionEventKind::TaskAccepted {
            task_id,
            backend_id,
            workspace_sha256,
            input_sha256,
        } if task_id == "task-001"
            && backend_id == "fixture-local"
            && workspace_sha256 == &h64('a')
            && input_sha256 == &task.input_sha256()
    ));
    assert!(matches!(
        &first.body.events[3].event,
        RemoteExecutionEventKind::ArtifactPublished { name, sha256, bytes }
            if name == "dist/result.txt"
                && sha256 == &artifact_sha256
                && *bytes == 23
    ));
    assert!(matches!(
        &first.body.events[1].event,
        RemoteExecutionEventKind::Output {
            channel: OutputChannel::Stdout,
            text_sha256,
            bytes: 8,
        } if text_sha256 == &format!("{:x}", Sha256::digest(b"starting"))
    ));
    assert!(matches!(
        &first.body.events[4].event,
        RemoteExecutionEventKind::Succeeded { artifact_sha256: observed }
            if observed == &artifact_sha256
    ));
    assert_eq!(
        first
            .body
            .artifact
            .as_ref()
            .map(|artifact| &artifact.sha256),
        Some(&artifact_sha256)
    );
    assert_eq!(first.body.terminal, TerminalStatus::Success);
}

#[test]
fn every_non_success_lifecycle_has_one_attested_terminal_event() {
    let fixture = fixture();
    let task = task(ResourceBudget::new(500, 1024 * 1024, 5_000, 4096).unwrap());
    let cases = [
        (
            ScriptedOutcome::failure("command_failed"),
            TerminalStatus::Failure {
                code: "command_failed".to_string(),
            },
        ),
        (
            ScriptedOutcome::Timeout,
            TerminalStatus::Timeout { limit_ms: 5_000 },
        ),
        (
            ScriptedOutcome::cancelled("operator_cancelled"),
            TerminalStatus::Cancelled {
                reason: "operator_cancelled".to_string(),
            },
        ),
        (
            ScriptedOutcome::disconnected("transport_closed"),
            TerminalStatus::Disconnected {
                reason: "transport_closed".to_string(),
            },
        ),
    ];

    for (outcome, expected_terminal) in cases {
        let script = RemoteExecutionScript::new(
            [ScriptedOutputEvent::new(
                2,
                OutputChannel::Stdout,
                "accepted work",
            )],
            outcome,
        );
        let receipt = fixture.execute(&task, &script).expect("fixture lifecycle");
        receipt
            .verify(fixture.identity(), &fixture.verifying_key())
            .expect("attested lifecycle receipt");
        assert_eq!(receipt.body.terminal, expected_terminal);
        assert!(receipt.body.artifact.is_none());
        assert_eq!(receipt.body.events.len(), 3);
        assert!(matches!(
            &receipt.body.events[0].event,
            RemoteExecutionEventKind::TaskAccepted { .. }
        ));
    }
}

#[test]
fn duplicate_and_out_of_order_script_events_are_rejected_before_execution() {
    let fixture = fixture();
    let task = task(ResourceBudget::new(500, 1024 * 1024, 5_000, 4096).unwrap());
    let duplicate = RemoteExecutionScript::new(
        [
            ScriptedOutputEvent::new(2, OutputChannel::Stdout, "first"),
            ScriptedOutputEvent::new(2, OutputChannel::Stdout, "duplicate"),
        ],
        ScriptedOutcome::failure("unused"),
    );
    let out_of_order = RemoteExecutionScript::new(
        [ScriptedOutputEvent::new(
            3,
            OutputChannel::Stdout,
            "skipped sequence",
        )],
        ScriptedOutcome::failure("unused"),
    );

    assert_eq!(
        fixture.execute(&task, &duplicate).unwrap_err(),
        RemoteFixtureError::InvalidEventSequence {
            expected: 3,
            observed: 2,
        }
    );
    assert_eq!(
        fixture.execute(&task, &out_of_order).unwrap_err(),
        RemoteFixtureError::InvalidEventSequence {
            expected: 2,
            observed: 3,
        }
    );
}

#[test]
fn resource_request_is_denied_before_task_acceptance() {
    let fixture = fixture();
    let task = task(
        ResourceBudget::new(500, limits().memory_bytes + 1, 5_000, 4096)
            .expect("valid over-limit request"),
    );
    let script = RemoteExecutionScript::new([], ScriptedOutcome::Timeout);

    let receipt = fixture.execute(&task, &script).expect("denial receipt");

    receipt
        .verify(fixture.identity(), &fixture.verifying_key())
        .expect("attested denial");
    assert_eq!(receipt.body.events.len(), 1);
    assert!(matches!(
        &receipt.body.events[0].event,
        RemoteExecutionEventKind::ResourceDenied {
            resource: ResourceKind::MemoryBytes,
            requested,
            limit,
        } if *requested == limits().memory_bytes + 1 && *limit == limits().memory_bytes
    ));
    assert!(matches!(
        &receipt.body.terminal,
        TerminalStatus::ResourceDenied {
            resource: ResourceKind::MemoryBytes,
            ..
        }
    ));
    assert!(receipt.body.artifact.is_none());
    assert!(
        !receipt
            .body
            .events
            .iter()
            .any(|event| matches!(&event.event, RemoteExecutionEventKind::TaskAccepted { .. }))
    );
}

#[test]
fn artifact_over_task_output_budget_is_denied_after_acceptance() {
    let fixture = fixture();
    let task = task(ResourceBudget::new(500, 1024 * 1024, 5_000, 4).unwrap());
    let script = RemoteExecutionScript::new(
        [],
        ScriptedOutcome::success(FixtureArtifact::new("result.bin", b"12345".to_vec()).unwrap()),
    );

    let receipt = fixture
        .execute(&task, &script)
        .expect("runtime denial receipt");

    receipt
        .verify(fixture.identity(), &fixture.verifying_key())
        .expect("attested runtime denial");
    assert_eq!(receipt.body.events.len(), 2);
    assert!(matches!(
        &receipt.body.events[0].event,
        RemoteExecutionEventKind::TaskAccepted { .. }
    ));
    assert!(matches!(
        &receipt.body.events[1].event,
        RemoteExecutionEventKind::ResourceDenied {
            resource: ResourceKind::OutputBytes,
            requested,
            limit,
        } if *requested == 5 && *limit == 4
    ));
    assert!(receipt.body.artifact.is_none());
}

#[test]
fn output_budget_counts_streamed_text_and_artifact_together() {
    let fixture = fixture();
    let task = task(ResourceBudget::new(500, 1024 * 1024, 5_000, 4).unwrap());
    let script = RemoteExecutionScript::new(
        [ScriptedOutputEvent::new(2, OutputChannel::Stdout, "12")],
        ScriptedOutcome::success(FixtureArtifact::new("result.bin", b"345".to_vec()).unwrap()),
    );

    let receipt = fixture.execute(&task, &script).expect("bounded receipt");
    assert!(matches!(
        receipt.body.terminal,
        TerminalStatus::ResourceDenied {
            resource: ResourceKind::OutputBytes,
            requested: 5,
            limit: 4,
        }
    ));
    receipt
        .verify(fixture.identity(), &fixture.verifying_key())
        .expect("aggregate denial verifies");
}

#[test]
fn receipt_tampering_or_unpinned_backend_is_rejected() {
    let fixture = fixture();
    let task = task(ResourceBudget::new(500, 1024 * 1024, 5_000, 4096).unwrap());
    let script = RemoteExecutionScript::new([], ScriptedOutcome::failure("command_failed"));
    let receipt = fixture.execute(&task, &script).expect("fixture receipt");

    let mut tampered_body = receipt.clone();
    tampered_body.body.task.input_sha256 = h64('f');
    assert!(
        tampered_body
            .verify(fixture.identity(), &fixture.verifying_key())
            .is_err()
    );

    let other = RemoteExecutionFixture::new(
        "fixture-other",
        "worker-02",
        "fixture-v1",
        limits(),
        [24; 32],
    )
    .unwrap();
    assert_eq!(
        receipt
            .verify(other.identity(), &other.verifying_key())
            .unwrap_err(),
        RemoteFixtureError::BackendIdentityMismatch
    );

    let mut tampered_signature = receipt;
    tampered_signature.attestation.signature_base64 = "AA==".to_string();
    assert!(matches!(
        tampered_signature
            .verify(fixture.identity(), &fixture.verifying_key())
            .unwrap_err(),
        RemoteFixtureError::MalformedSignature | RemoteFixtureError::InvalidSignature
    ));
}

#[test]
fn fixture_rejects_non_portable_artifacts_and_unbounded_inputs() {
    assert!(FixtureArtifact::new("../escape", Vec::new()).is_err());
    assert!(FixtureArtifact::new("C:\\escape", Vec::new()).is_err());
    assert!(FixtureArtifact::new("nested\\escape", Vec::new()).is_err());
    assert!(FixtureArtifact::new("CON.txt", Vec::new()).is_err());
    assert!(FixtureArtifact::new("nested/bad\nname", Vec::new()).is_err());

    let invalid_reason =
        RemoteExecutionScript::new([], ScriptedOutcome::cancelled("raw reason with spaces"));
    assert_eq!(
        fixture()
            .execute(
                &task(ResourceBudget::new(500, 1024 * 1024, 5_000, 4096).unwrap()),
                &invalid_reason,
            )
            .unwrap_err(),
        RemoteFixtureError::InvalidReason
    );

    let oversized = vec![0_u8; 1024 * 1024 + 1];
    assert_eq!(
        RemoteTask::new("task", h64('a'), oversized, limits()).unwrap_err(),
        RemoteFixtureError::InputTooLarge { limit: 1024 * 1024 }
    );
}
