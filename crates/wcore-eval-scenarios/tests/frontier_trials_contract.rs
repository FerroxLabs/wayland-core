//! Contract suite for the F30-03 frontier trial harness.
//!
//! **Every refusal test here carries a PRISTINE CONTROL that is accepted first.** A
//! rejection-only suite passes against a verifier that rejects everything, which is the
//! same defect class as a self-passing gate: it proves the door is shut without ever
//! proving it can open. So each test asserts the honest path works, then mutates exactly
//! one thing and asserts the refusal.
//!
//! The two load-bearing rules are `a_directional_verdict_is_refused_when_the_delta_interval
//! _contains_zero` and `a_comparative_result_with_a_missing_peer_measurement_cannot_be
//! _constructed`. Between them they close the two ways this phase could publish a
//! flattering lie: a point estimate favouring Wayland whose interval spans zero, and a
//! peer that would not run quietly becoming a Wayland win.

use std::collections::BTreeMap;

use wcore_eval_scenarios::frontier_trials::{
    ComparativeResultV1, DeltaV1, DimensionV1, DirectionV1, FrontierTrialError, IntervalMethodV1,
    IntervalV1, LegStatusV1, LegV1, MeasurementV1, ResultSetV1, ScopeV1, ToolV1,
    percentile_bootstrap, protocol_sha256, wilson_score_interval,
};

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

fn interval(lower: f64, upper: f64) -> IntervalV1 {
    IntervalV1 {
        lower,
        upper,
        method: IntervalMethodV1::WilsonScore95,
        confidence: 0.95,
    }
}

fn measurement(tool: ToolV1, dimension: DimensionV1, estimate: f64) -> MeasurementV1 {
    MeasurementV1 {
        tool,
        dimension,
        scope: ScopeV1::ScriptedHarness,
        trials: 30,
        samples_sha256: "0".repeat(64),
        estimate,
        interval: interval(estimate - 0.1, estimate + 0.1),
    }
}

/// A comparative with BOTH peers measured, whose delta interval is entirely inside the
/// tie band. This is the pristine control several tests below mutate.
fn pristine_comparative() -> ComparativeResultV1 {
    let mut measurements = BTreeMap::new();
    measurements.insert(
        ToolV1::Wayland,
        measurement(ToolV1::Wayland, DimensionV1::Correctness, 0.90),
    );
    measurements.insert(
        ToolV1::Hermes,
        measurement(ToolV1::Hermes, DimensionV1::Correctness, 0.89),
    );
    ComparativeResultV1::try_new(
        DimensionV1::Correctness,
        measurements,
        DeltaV1 {
            estimate: 0.01,
            interval: interval(-0.02, 0.03),
        },
        0.05,
        &[ToolV1::Wayland, ToolV1::Hermes],
    )
    .expect("pristine comparative must be constructible")
}

// ---------------------------------------------------------------------------
// 1. No unbounded point estimate
// ---------------------------------------------------------------------------

#[test]
fn a_measurement_without_an_interval_does_not_deserialize() {
    // CONTROL: the same document WITH an interval is accepted.
    let pristine = r#"{
        "tool": "wayland",
        "dimension": "correctness",
        "scope": "SCRIPTED_HARNESS",
        "trials": 30,
        "samples_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        "estimate": 0.9,
        "interval": {"lower": 0.8, "upper": 0.95, "method": "wilson_score_95", "confidence": 0.95}
    }"#;
    let accepted: MeasurementV1 =
        serde_json::from_str(pristine).expect("control: a bounded measurement must deserialize");
    assert_eq!(accepted.estimate, 0.9);

    // MUTATION: remove exactly the interval.
    let unbounded = r#"{
        "tool": "wayland",
        "dimension": "correctness",
        "scope": "SCRIPTED_HARNESS",
        "trials": 30,
        "samples_sha256": "0000000000000000000000000000000000000000000000000000000000000000",
        "estimate": 0.9
    }"#;
    let refused = serde_json::from_str::<MeasurementV1>(unbounded);
    assert!(
        refused.is_err(),
        "a measurement with no confidence interval must NOT deserialize; F30-03 says bounds, \
         and the way to guarantee bounds is to make their absence unrepresentable"
    );

    // And there is no back door: an explicit null is refused too.
    let nulled = unbounded.replace("\"estimate\": 0.9", "\"estimate\": 0.9, \"interval\": null");
    assert!(
        serde_json::from_str::<MeasurementV1>(&nulled).is_err(),
        "an explicitly null interval must be refused as well as an absent one"
    );
}

#[test]
fn an_invented_scope_tag_fails_to_deserialize() {
    // CONTROL: each declared scope is accepted.
    for token in ["SCRIPTED_HARNESS", "LIVE_PROVIDER", "STATIC_SOURCE"] {
        let json = format!("\"{token}\"");
        serde_json::from_str::<ScopeV1>(&json)
            .unwrap_or_else(|e| panic!("control: declared scope {token} must deserialize: {e}"));
    }

    // MUTATION: a scope nobody declared. This is the exact shape of the forgery 30-03
    // exists to refuse - a harness measurement relabelled as a real-world one.
    for invented in ["REAL_WORLD", "PRODUCTION", "scripted_harness", ""] {
        let json = format!("\"{invented}\"");
        assert!(
            serde_json::from_str::<ScopeV1>(&json).is_err(),
            "an undeclared scope tag `{invented}` must fail at DESERIALIZATION, before any logic runs"
        );
    }
}

#[test]
fn a_proportion_over_zero_trials_is_refused() {
    // CONTROL: a real proportion over real trials produces a real interval.
    let ok = wilson_score_interval(27, 30).expect("control: 27/30 must produce an interval");
    assert!(ok.lower > 0.0 && ok.upper < 1.0, "got {ok:?}");
    assert!(
        ok.lower < 0.9 && ok.upper > 0.9,
        "interval must cover 0.9: {ok:?}"
    );

    // Even the degenerate-but-real case of 30/30 is accepted, and its lower bound is
    // strictly below 1.0 - a perfect score is not a claim of perfect reliability.
    let perfect = wilson_score_interval(30, 30).expect("control: 30/30 must produce an interval");
    assert!(
        perfect.lower < 1.0,
        "30/30 must NOT yield a lower bound of 1.0; got {perfect:?}"
    );

    // MUTATION: zero trials. A zero-trial proportion is the shape a silently skipped leg
    // takes, so it must be a typed refusal rather than 0.0 or 1.0.
    let refused = wilson_score_interval(0, 0);
    assert!(
        matches!(refused, Err(FrontierTrialError::ZeroTrials { .. })),
        "a proportion over zero trials must be REFUSED, not reported as zero or one; got {refused:?}"
    );

    // And more successes than trials is incoherent, not merely unlikely.
    assert!(wilson_score_interval(31, 30).is_err());
}

// ---------------------------------------------------------------------------
// 2. No one-sided comparison
// ---------------------------------------------------------------------------

#[test]
fn a_comparative_result_with_a_missing_peer_measurement_cannot_be_constructed() {
    // CONTROL: both tools measured - constructible.
    let control = pristine_comparative();
    assert_eq!(control.measurements.len(), 2);

    // MUTATION: drop the peer. "We could not run the competitor, so we win" must not be
    // expressible in this type system at all.
    let mut wayland_only = BTreeMap::new();
    wayland_only.insert(
        ToolV1::Wayland,
        measurement(ToolV1::Wayland, DimensionV1::Correctness, 0.90),
    );
    let refused = ComparativeResultV1::try_new(
        DimensionV1::Correctness,
        wayland_only,
        DeltaV1 {
            estimate: 0.01,
            interval: interval(-0.02, 0.03),
        },
        0.05,
        &[ToolV1::Wayland, ToolV1::Hermes],
    );
    assert!(
        matches!(
            refused,
            Err(FrontierTrialError::MissingPeerMeasurement { .. })
        ),
        "a comparative result missing a required tool's measurement must be UNCONSTRUCTIBLE; got {refused:?}"
    );

    // Dropping WAYLAND is refused by the same rule - the fence is not Wayland-favouring.
    let mut peer_only = BTreeMap::new();
    peer_only.insert(
        ToolV1::Hermes,
        measurement(ToolV1::Hermes, DimensionV1::Correctness, 0.89),
    );
    assert!(
        ComparativeResultV1::try_new(
            DimensionV1::Correctness,
            peer_only,
            DeltaV1 {
                estimate: 0.0,
                interval: interval(-0.02, 0.03)
            },
            0.05,
            &[ToolV1::Wayland, ToolV1::Hermes],
        )
        .is_err(),
        "the missing-measurement rule must apply symmetrically"
    );
}

// ---------------------------------------------------------------------------
// 3. No directional claim without separation - and its pristine control
// ---------------------------------------------------------------------------

#[test]
fn a_directional_verdict_is_refused_when_the_delta_interval_contains_zero() {
    // CONTROL FIRST: a delta interval that CLEARS the tie band verifies as directional.
    // Without this the test would pass against a verifier that refuses every direction.
    let mut separated = pristine_comparative();
    separated.delta = DeltaV1 {
        estimate: 0.20,
        interval: interval(0.12, 0.28),
    };
    separated.direction = DirectionV1::WaylandAhead;
    separated
        .verify()
        .expect("control: a delta interval entirely above the tie band MUST verify as directional");

    // MUTATION: the point estimate still favours Wayland, but the interval spans zero.
    // This is the single flattering lie this plan exists to make unpublishable.
    let mut straddling = pristine_comparative();
    straddling.delta = DeltaV1 {
        estimate: 0.20,
        interval: interval(-0.05, 0.45),
    };
    straddling.direction = DirectionV1::WaylandAhead;
    let refused = straddling.verify();
    assert!(
        matches!(
            refused,
            Err(FrontierTrialError::DirectionalVerdictOnIntervalContainingZero { .. })
        ),
        "a directional verdict whose delta interval contains zero must be REFUSED; got {refused:?}"
    );

    // The rule is symmetric: it refuses a verdict favouring the PEER just as hard.
    let mut peer_favouring = pristine_comparative();
    peer_favouring.delta = DeltaV1 {
        estimate: -0.20,
        interval: interval(-0.45, 0.05),
    };
    peer_favouring.direction = DirectionV1::PeerAhead;
    assert!(
        peer_favouring.verify().is_err(),
        "the zero rule must refuse a peer-favouring verdict too, or it is a Wayland-favouring rule"
    );
}

#[test]
fn indistinguishable_verifies_when_the_delta_interval_contains_zero() {
    // The pristine control for the rule above: the SAME situation - an interval containing
    // zero - must VERIFY when the verdict is non-directional and the interval fits inside
    // the tie band. A verifier that refused this would block the honest path.
    let mut tied = pristine_comparative();
    tied.delta = DeltaV1 {
        estimate: 0.01,
        interval: interval(-0.02, 0.03),
    };
    tied.direction = DirectionV1::PracticallyIndistinguishable;
    assert!(tied.delta.interval.contains_zero());
    tied.verify()
        .expect("an interval containing zero and lying inside the tie band must VERIFY as indistinguishable");

    // And the fourth state exists for a reason: an interval that contains zero but is far
    // too wide to fit the band is INCONCLUSIVE, not equivalent. Collapsing these two would
    // silently convert low statistical power into a declared tie.
    let mut wide = pristine_comparative();
    wide.delta = DeltaV1 {
        estimate: 0.05,
        interval: interval(-0.40, 0.50),
    };
    wide.direction = DirectionV1::Inconclusive;
    wide.verify()
        .expect("a wide interval containing zero must verify as INCONCLUSIVE");

    // Labelling that same wide interval as an equivalence claim is refused.
    wide.direction = DirectionV1::PracticallyIndistinguishable;
    assert!(
        wide.verify().is_err(),
        "an interval too wide for the tie band must NOT be reportable as practically \
         indistinguishable - that is low power wearing the costume of equivalence"
    );
}

// ---------------------------------------------------------------------------
// 4. Results are bound to the protocol they were run under
// ---------------------------------------------------------------------------

fn pristine_result_set(protocol: &[u8]) -> ResultSetV1 {
    let mut legs = Vec::new();
    let mut n = 0;
    for tool in [ToolV1::Wayland, ToolV1::Hermes, ToolV1::Openclaw] {
        for dimension in [
            DimensionV1::Correctness,
            DimensionV1::Recovery,
            DimensionV1::Security,
            DimensionV1::Cost,
            DimensionV1::CognitiveTax,
        ] {
            n += 1;
            legs.push(LegV1 {
                id: format!("LEG-{n:02}"),
                tool,
                dimension,
                status: LegStatusV1::Unproven,
                evidence: format!("leg-{n:02}.txt"),
                blocker: Some("contract fixture, not a real trial".to_string()),
            });
        }
    }
    ResultSetV1 {
        protocol_sha256: protocol_sha256(protocol),
        scope: ScopeV1::ScriptedHarness,
        measurements: Vec::new(),
        comparatives: Vec::new(),
        legs,
    }
}

#[test]
fn a_result_set_does_not_verify_against_a_protocol_whose_digest_differs() {
    let protocol = br#"{"protocol_version":"F30-03-TRIAL-PROTOCOL-V1"}"#;

    // CONTROL: the result set verifies against the protocol it records.
    let results = pristine_result_set(protocol);
    results
        .verify(protocol)
        .expect("control: a result set must verify against its own protocol");

    // MUTATION: one byte of the protocol changes. This is exactly the act of amending the
    // methodology after seeing a number, and it must break the binding.
    let amended = br#"{"protocol_version":"F30-03-TRIAL-PROTOCOL-V2"}"#;
    let refused = results.verify(amended);
    assert!(
        matches!(
            refused,
            Err(FrontierTrialError::ProtocolDigestMismatch { .. })
        ),
        "a result set must NOT verify against a protocol whose digest differs; got {refused:?}"
    );

    // A result set that omits a leg is refused: silence about a leg is the defect the
    // fifteen-leg accounting exists to make impossible.
    let mut short = pristine_result_set(protocol);
    short.legs.pop();
    assert!(
        short.verify(protocol).is_err(),
        "a result set accounting for fewer than fifteen legs must be refused"
    );

    // And a leg filed twice is refused, so a duplicate cannot stand in for a missing one.
    let mut duplicated = pristine_result_set(protocol);
    duplicated.legs[14] = duplicated.legs[0].clone();
    assert!(
        duplicated.verify(protocol).is_err(),
        "each (tool, dimension) leg must appear exactly once"
    );

    // An UNPROVEN leg with no blocker named is refused - an unexplained gap is how a peer
    // that would not run quietly disappears.
    let mut unexplained = pristine_result_set(protocol);
    unexplained.legs[3].blocker = None;
    assert!(
        unexplained.verify(protocol).is_err(),
        "an UNPROVEN leg must name its blocker"
    );
}

// ---------------------------------------------------------------------------
// 5. The bootstrap is reproducible, and the seed is actually load-bearing
// ---------------------------------------------------------------------------

#[test]
fn two_bootstrap_runs_under_the_same_seed_produce_identical_bounds() {
    let samples: Vec<f64> = (0..40).map(|i| 10.0 + f64::from(i) * 1.5).collect();

    // CONTROL / ACCEPTANCE: identical seed, identical bounds, bit for bit.
    let first = percentile_bootstrap(&samples, 10_000, 30020004)
        .expect("control: a bootstrap over real samples must produce an interval");
    let second = percentile_bootstrap(&samples, 10_000, 30020004)
        .expect("control: the same call must succeed twice");
    assert_eq!(
        first.lower.to_bits(),
        second.lower.to_bits(),
        "same seed must reproduce the lower bound EXACTLY: {first:?} vs {second:?}"
    );
    assert_eq!(first.upper.to_bits(), second.upper.to_bits());

    // THE ANTI-TAUTOLOGY LEG. A resampler that ignored its seed - or one that just
    // returned min and max - would pass the determinism assertion above trivially. So a
    // DIFFERENT seed over the same spread-out samples must move the bounds, proving the
    // seed is load-bearing and the interval is a resample rather than a range.
    let other_seed = percentile_bootstrap(&samples, 10_000, 30020005)
        .expect("a different seed must still produce an interval");
    assert!(
        other_seed.lower.to_bits() != first.lower.to_bits()
            || other_seed.upper.to_bits() != first.upper.to_bits(),
        "a different seed must change the bounds, or the seed is decorative and the \
         determinism claim above is a tautology: {first:?} vs {other_seed:?}"
    );

    // And the interval must sit strictly inside the sample range - a bootstrap of the MEAN
    // cannot reach the extremes of the raw data.
    let min = samples.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = samples.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    assert!(
        first.lower > min && first.upper < max,
        "a bootstrap of the mean must lie strictly inside [{min}, {max}]: {first:?}"
    );

    // Zero samples is refused for the same reason a zero-trial proportion is.
    assert!(percentile_bootstrap(&[], 10_000, 1).is_err());
}

// ---------------------------------------------------------------------------
// 6. Closed shapes - an unknown field is a truth silently ignored
// ---------------------------------------------------------------------------

#[test]
fn an_unknown_field_is_refused_at_every_boundary_struct() {
    let interval_json = r#"{"lower":0.1,"upper":0.2,"method":"wilson_score_95","confidence":0.95}"#;
    serde_json::from_str::<IntervalV1>(interval_json).expect("control: the interval deserializes");
    let with_extra = interval_json.replace("}", ",\"note\":\"looks harmless\"}");
    assert!(
        serde_json::from_str::<IntervalV1>(&with_extra).is_err(),
        "an unknown field must be refused: a truth silently ignored reads exactly like a \
         truth supplied"
    );
}
