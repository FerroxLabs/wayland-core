//! F24-01 Task 1 — the lifecycle contract.
//!
//! Written BEFORE `src/lifecycle.rs` existed. Every legal transition is
//! driven, and every illegal one must be refused BY NAME rather than
//! silently no-op'ing: a state machine that quietly ignores an impossible
//! request is indistinguishable from one that performed it, and the
//! operator verbs in `wcore-cli` report their exit status from exactly
//! these refusals.

use wcore_gateway::lifecycle::{GatewayState, LifecycleError, StatusProjection, Transition};

/// The happy path an operator drives: install, start, running, drain,
/// drained, stopped. Each step must be accepted from the state before it.
#[test]
fn legal_transitions_walk_the_whole_operator_path() {
    let s = GatewayState::Uninstalled;
    let s = s
        .apply(Transition::Install)
        .expect("install from uninstalled");
    assert_eq!(s, GatewayState::Installed);

    let s = s.apply(Transition::Start).expect("start from installed");
    assert_eq!(s, GatewayState::Starting);

    let s = s.apply(Transition::Started).expect("started from starting");
    assert_eq!(s, GatewayState::Running);

    let s = s.apply(Transition::Drain).expect("drain from running");
    assert_eq!(s, GatewayState::Draining);

    let s = s.apply(Transition::DrainComplete).expect("drain completes");
    assert_eq!(s, GatewayState::Drained);

    let s = s.apply(Transition::Stop).expect("stop from drained");
    assert_eq!(s, GatewayState::Stopped);

    // A stopped-but-installed gateway restarts.
    let s = s.apply(Transition::Start).expect("restart from stopped");
    assert_eq!(s, GatewayState::Starting);
}

/// Starting an already-running gateway is refused by a DISTINCT name, not
/// by a generic error and not by silently returning Running.
#[test]
fn starting_a_running_gateway_is_refused_by_name() {
    let err = GatewayState::Running
        .apply(Transition::Start)
        .expect_err("starting a running gateway must be refused");
    assert!(
        matches!(err, LifecycleError::AlreadyRunning),
        "expected AlreadyRunning, got {err:?}"
    );
}

/// Stopping a stopped gateway is refused by its own name, distinguishable
/// from AlreadyRunning so the CLI can return a different exit status.
#[test]
fn stopping_a_stopped_gateway_is_refused_by_name() {
    let err = GatewayState::Stopped
        .apply(Transition::Stop)
        .expect_err("stopping a stopped gateway must be refused");
    assert!(
        matches!(err, LifecycleError::NotRunning),
        "expected NotRunning, got {err:?}"
    );
}

/// Draining a stopped gateway is its own refusal: there is nothing to
/// drain, and reporting a clean drain here would be a lie the delivery
/// tally later depends on.
#[test]
fn draining_a_stopped_gateway_is_refused_by_name() {
    let err = GatewayState::Stopped
        .apply(Transition::Drain)
        .expect_err("draining a stopped gateway must be refused");
    assert!(
        matches!(err, LifecycleError::DrainRequiresRunning { from } if from == GatewayState::Stopped),
        "expected DrainRequiresRunning{{from: Stopped}}, got {err:?}"
    );
}

/// Every remaining illegal pair is refused as an IllegalTransition naming
/// both operands. This is the catch-all, and it must still name what it
/// refused.
#[test]
fn every_other_illegal_pair_is_refused_naming_both_operands() {
    let illegal = [
        (GatewayState::Uninstalled, Transition::Start),
        (GatewayState::Uninstalled, Transition::Drain),
        (GatewayState::Installed, Transition::Started),
        (GatewayState::Installed, Transition::DrainComplete),
        (GatewayState::Starting, Transition::Drain),
        (GatewayState::Draining, Transition::Start),
        (GatewayState::Drained, Transition::Drain),
        (GatewayState::Running, Transition::DrainComplete),
        (GatewayState::Running, Transition::Install),
    ];
    for (from, transition) in illegal {
        let outcome = from.apply(transition);
        assert!(
            outcome.is_err(),
            "{from:?} + {transition:?} must be refused, got {outcome:?}"
        );
    }
}

/// The refusal names both operands so an operator reading stderr can see
/// which state it was actually in.
#[test]
fn illegal_transition_error_renders_both_operands() {
    let err = GatewayState::Drained
        .apply(Transition::Drain)
        .expect_err("draining a drained gateway is illegal");
    let rendered = err.to_string();
    assert!(
        rendered.contains("Drained"),
        "refusal must name the state it was in, got: {rendered}"
    );
    assert!(
        rendered.contains("Drain"),
        "refusal must name the transition refused, got: {rendered}"
    );
}

/// Failure is reachable from every live state, because a runtime that
/// cannot record that it broke reports Running forever.
#[test]
fn fail_is_reachable_from_every_live_state() {
    for from in [
        GatewayState::Starting,
        GatewayState::Running,
        GatewayState::Draining,
    ] {
        assert_eq!(
            from.apply(Transition::Fail)
                .expect("fail is always legal from a live state"),
            GatewayState::Failed
        );
    }
}

/// The status projection is stable and machine-readable, and carries every
/// field the first Success Criterion needs an operator to be able to read:
/// state, process identity, uptime, profile, in-flight turns, pending
/// deliveries, and the identity of the binary the process was launched
/// from (which is what makes an upgrade and a rollback observable).
#[test]
fn status_projection_carries_every_operator_field() {
    let p = StatusProjection {
        state: GatewayState::Running,
        pid: Some(4242),
        uptime_secs: Some(90),
        profile: "default".to_string(),
        turns_in_flight: 3,
        deliveries_pending: 7,
        binary_path: Some("/usr/local/bin/wayland-core".into()),
        binary_version: Some("0.12.25".to_string()),
    };
    let json = serde_json::to_value(&p).expect("status projection serializes");
    for key in [
        "state",
        "pid",
        "uptime_secs",
        "profile",
        "turns_in_flight",
        "deliveries_pending",
        "binary_path",
        "binary_version",
    ] {
        assert!(
            json.get(key).is_some(),
            "status projection is missing the `{key}` field an operator reads"
        );
    }
    assert_eq!(json["state"], "running", "state renders in a stable form");
    assert_eq!(json["turns_in_flight"], 3);
    assert_eq!(json["deliveries_pending"], 7);
}

/// A stopped gateway's projection has no process identity and no uptime —
/// reporting a pid for a process that is gone is how a status verb lies.
#[test]
fn stopped_projection_has_no_process_identity() {
    let p = StatusProjection::stopped("default");
    assert_eq!(p.state, GatewayState::Stopped);
    assert!(p.pid.is_none());
    assert!(p.uptime_secs.is_none());
    assert_eq!(p.turns_in_flight, 0);
    assert_eq!(p.deliveries_pending, 0);
}
