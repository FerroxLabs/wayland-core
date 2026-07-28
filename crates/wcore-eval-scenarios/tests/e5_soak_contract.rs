//! The F28-02 soak contract: every VOID condition is proved by a test that TRIPS it, and
//! the executor is proved to agree with the canonical definitions it implements.
//!
//! A canary scan reporting zero detections and a canary scan that never ran produce
//! identical output. The rules that separate them are the whole value of this harness, so
//! each one is exercised here rather than asserted in a comment. A rule that quietly
//! stopped working would otherwise present as a clean soak.

use std::collections::BTreeMap;
use std::path::PathBuf;

use wcore_eval_scenarios::e5_soak::{
    BLOCK_COUNT, BLOCK_SIZE, Bands, CanaryChannel, CanaryScan, CensusBackend, DriftBand,
    DriftMeasurement, FamilyRecord, FloorBand, MIN_CONCURRENCY, Observable, OrphanCensus,
    ResourceSample, ResourceSeries, SESSION_TARGET, SlopeBand, Verdict, canary_verdict,
    concurrency_verdict, drift_verdict, orphan_verdict, resource_verdict, series_growth,
    session_count_verdict,
};

// -------------------------------------------------------------------------------------
// fixtures -- the accept path, so that every rejection below is proved to be the RULE
// firing rather than the fixture being malformed
// -------------------------------------------------------------------------------------

fn clean_scan() -> CanaryScan {
    CanaryScan {
        channels: CanaryChannel::ALL
            .iter()
            .map(|c| (c.as_str().to_string(), 0))
            .collect(),
        channels_scanned: CanaryChannel::ALL
            .iter()
            .map(|c| c.as_str().to_string())
            .collect(),
        control_detected: true,
        control_channel: "all-six".to_string(),
    }
}

fn clean_census() -> OrphanCensus {
    OrphanCensus {
        backend: CensusBackend::CgroupV2,
        authoritative: true,
        orphans_found: 0,
        control_orphan_found: true,
    }
}

fn flat_series() -> ResourceSeries {
    ResourceSeries {
        samples: (0..=10)
            .map(|i| ResourceSample {
                session_index: i * 100,
                metrics: BTreeMap::from([("state_dir_bytes".to_string(), 1_000.0)]),
            })
            .collect(),
        control_growth_flagged: true,
    }
}

fn bands() -> Bands {
    Bands {
        session_target: SESSION_TARGET,
        block_size: BLOCK_SIZE,
        min_concurrency: MIN_CONCURRENCY,
        resource_min_samples: 4,
        early_blocks: vec![1, 2, 3],
        late_blocks: vec![8, 9, 10],
        drift: vec![DriftBand {
            metric: "latency_p50_block_median_ms".to_string(),
            max_ratio: Some(1.5),
            max_absolute_drop: None,
        }],
        floors: vec![FloorBand {
            metric: "quality_correct_rate_run".to_string(),
            op: ">=".to_string(),
            value: 0.99,
        }],
        slopes: vec![SlopeBand {
            metric: "state_dir_bytes".to_string(),
            max_growth: 2.0,
            ratio: true,
        }],
    }
}

fn family() -> FamilyRecord {
    FamilyRecord {
        family: "linux".to_string(),
        host: "fixture".to_string(),
        target: "x86_64-unknown-linux-gnu".to_string(),
        binary_sha256: "a".repeat(64),
        ledger_sha256: "a".repeat(64),
        sessions_completed: SESSION_TARGET,
        session_target: SESSION_TARGET,
        concurrency: 4,
        canary: clean_scan(),
        census: clean_census(),
        resources: flat_series(),
        drift: vec![DriftMeasurement {
            metric: "latency_p50_block_median_ms".to_string(),
            early: 100.0,
            late: 110.0,
        }],
    }
}

#[test]
fn the_accept_path_is_reachable_so_every_rejection_below_is_the_rule_and_not_the_fixture() {
    let f = family();
    let b = bands();
    for (observable, verdict) in f.verdicts(Some(&b)) {
        assert!(
            verdict.is_green(),
            "{observable:?} should be green on the clean fixture, got {verdict:?}"
        );
    }
    assert!(f.criterion2_met(Some(&b)));
}

// -------------------------------------------------------------------------------------
// VOID conditions -- one test per condition, each TRIPPING it
// -------------------------------------------------------------------------------------

#[test]
fn void_a_clean_canary_scan_whose_control_went_undetected() {
    let mut scan = clean_scan();
    scan.control_detected = false;
    let v = canary_verdict(&scan);
    assert!(v.is_void(), "expected VOID, got {v:?}");
    assert!(
        !v.is_green(),
        "a detector that cannot detect must never produce a green"
    );
}

#[test]
fn void_a_canary_scan_that_dropped_a_channel() {
    for dropped in CanaryChannel::ALL {
        let mut scan = clean_scan();
        scan.channels_scanned.retain(|c| c != dropped.as_str());
        assert!(
            canary_verdict(&scan).is_void(),
            "dropping the {} channel must VOID the scan",
            dropped.as_str()
        );
    }
}

#[test]
fn void_an_orphan_census_whose_control_orphan_was_never_found() {
    let mut census = clean_census();
    census.control_orphan_found = false;
    let v = orphan_verdict(&census);
    assert!(v.is_void(), "expected VOID, got {v:?}");
}

#[test]
fn void_a_census_that_claims_authority_its_backend_does_not_have() {
    let census = OrphanCensus {
        backend: CensusBackend::ProcessGroupObservedNonauthoritative,
        authoritative: true,
        orphans_found: 0,
        control_orphan_found: true,
    };
    assert!(
        orphan_verdict(&census).is_void(),
        "a non-authoritative fallback may not report itself authoritative -- that is the \
         caveat this harness exists to carry forward"
    );
}

#[test]
fn void_a_resource_series_that_retained_only_its_endpoints() {
    let mut series = flat_series();
    series.samples.truncate(2);
    let v = resource_verdict(&series, &bands());
    assert!(
        v.is_void(),
        "an endpoint reading cannot distinguish a leak from a high-water mark; got {v:?}"
    );
}

#[test]
fn void_a_resource_verdict_whose_growth_control_was_never_flagged() {
    let mut series = flat_series();
    series.control_growth_flagged = false;
    assert!(resource_verdict(&series, &bands()).is_void());
}

#[test]
fn void_a_banded_metric_that_was_never_sampled() {
    let mut b = bands();
    b.slopes = vec![SlopeBand {
        metric: "never_sampled".to_string(),
        max_growth: 1.0,
        ratio: false,
    }];
    assert!(resource_verdict(&flat_series(), &b).is_void());
}

#[test]
fn void_a_drift_verdict_with_no_bands_rather_than_a_defaulted_one() {
    let v = drift_verdict(&[], None);
    assert!(
        v.is_void(),
        "the harness must FAIL rather than default; got {v:?}"
    );
    let f = family();
    assert!(
        f.verdicts(None)
            .iter()
            .any(|(o, v)| matches!(o, Observable::QualityPerformanceDrift) && v.is_void()),
        "a family evaluated with no bands must void its drift observable"
    );
}

#[test]
fn void_a_family_whose_binary_is_not_the_candidate() {
    let mut f = family();
    f.binary_sha256 = "b".repeat(64);
    let verdicts = f.verdicts(Some(&bands()));
    assert_eq!(verdicts.len(), Observable::ALL.len());
    assert!(
        verdicts.iter().all(|(_, v)| v.is_void()),
        "a family running a different build is not certifying the candidate, so EVERY \
         observable voids rather than only one"
    );
    assert!(!f.criterion2_met(Some(&bands())));
}

#[test]
fn void_a_family_with_no_ledger_digest_at_all() {
    let mut f = family();
    f.ledger_sha256 = String::new();
    assert!(!f.digest_bound());
    assert!(f.verdicts(Some(&bands())).iter().all(|(_, v)| v.is_void()));
}

// -------------------------------------------------------------------------------------
// RED conditions -- the product's own results, kept distinct from VOID
// -------------------------------------------------------------------------------------

#[test]
fn red_is_not_void_and_void_is_not_red() {
    let mut scan = clean_scan();
    scan.channels.insert("stdout".to_string(), 1);
    let leaked = canary_verdict(&scan);
    assert!(
        matches!(leaked, Verdict::Red { .. }),
        "a real detection is a RED"
    );
    assert!(
        !leaked.is_void(),
        "a measured leak is a measurement, not the absence of one"
    );

    let mut broken = clean_scan();
    broken.control_detected = false;
    assert!(broken_is_void(&canary_verdict(&broken)));

    fn broken_is_void(v: &Verdict) -> bool {
        v.is_void() && !matches!(v, Verdict::Red { .. })
    }
}

#[test]
fn red_an_orphaned_process_survived_the_run() {
    let mut census = clean_census();
    census.orphans_found = 1;
    assert!(matches!(orphan_verdict(&census), Verdict::Red { .. }));
}

#[test]
fn red_a_resource_metric_grew_past_its_decided_band() {
    let series = ResourceSeries {
        samples: (0..=10)
            .map(|i| ResourceSample {
                session_index: i * 100,
                metrics: BTreeMap::from([(
                    "state_dir_bytes".to_string(),
                    1_000.0 * (1.0 + i as f64),
                )]),
            })
            .collect(),
        control_growth_flagged: true,
    };
    assert!(matches!(
        resource_verdict(&series, &bands()),
        Verdict::Red { .. }
    ));
}

#[test]
fn red_late_latency_exceeds_the_decided_ratio() {
    let m = vec![DriftMeasurement {
        metric: "latency_p50_block_median_ms".to_string(),
        early: 100.0,
        late: 200.0,
    }];
    assert!(matches!(
        drift_verdict(&m, Some(&bands())),
        Verdict::Red { .. }
    ));
    let ok = vec![DriftMeasurement {
        metric: "latency_p50_block_median_ms".to_string(),
        early: 100.0,
        late: 150.0,
    }];
    assert!(
        drift_verdict(&ok, Some(&bands())).is_green(),
        "the band is inclusive at 1.5x"
    );
}

#[test]
fn a_shortfall_against_the_thousand_session_target_is_never_a_pass() {
    assert!(session_count_verdict(SESSION_TARGET, SESSION_TARGET).is_green());
    for completed in [0, 1, 250, 999] {
        let v = session_count_verdict(completed, SESSION_TARGET);
        assert!(
            !v.is_green(),
            "{completed} sessions must not pass a 1,000-session target"
        );
        match v {
            Verdict::Red { detail, .. } => assert!(
                detail.contains("NOT MET"),
                "a shortfall must say the criterion is NOT MET, not merely that it was short"
            ),
            other => panic!("expected a RED, got {other:?}"),
        }
    }
}

#[test]
fn a_zero_concurrency_soak_fails_the_contract_and_cannot_be_configured_away() {
    assert!(!concurrency_verdict(0).is_green());
    assert!(!concurrency_verdict(1).is_green());
    assert!(concurrency_verdict(MIN_CONCURRENCY).is_green());
    let mut f = family();
    f.concurrency = 0;
    assert!(
        !f.criterion2_met(Some(&bands())),
        "a serial run is invisible to sibling-dependent defects and may not meet the criterion"
    );
}

// -------------------------------------------------------------------------------------
// slope arithmetic -- the endpoint survives only as one term of the trend
// -------------------------------------------------------------------------------------

#[test]
fn a_high_water_mark_and_a_leak_are_distinguished_by_the_series() {
    // Same ENDPOINT, opposite trends: a spike that recedes, and a monotone climb.
    let spike = ResourceSeries {
        samples: vec![
            (0u64, 1_000.0),
            (200, 9_000.0),
            (400, 5_000.0),
            (600, 2_000.0),
            (800, 1_100.0),
            (1000, 1_000.0),
        ]
        .into_iter()
        .map(|(i, v)| ResourceSample {
            session_index: i,
            metrics: BTreeMap::from([("state_dir_bytes".to_string(), v)]),
        })
        .collect(),
        control_growth_flagged: true,
    };
    let leak = ResourceSeries {
        samples: vec![
            (0u64, 1_000.0),
            (200, 1_800.0),
            (400, 2_600.0),
            (600, 3_400.0),
            (800, 4_200.0),
            (1000, 5_000.0),
        ]
        .into_iter()
        .map(|(i, v)| ResourceSample {
            session_index: i,
            metrics: BTreeMap::from([("state_dir_bytes".to_string(), v)]),
        })
        .collect(),
        control_growth_flagged: true,
    };
    assert!(
        resource_verdict(&spike, &bands()).is_green(),
        "a receding spike is not a leak"
    );
    assert!(
        matches!(resource_verdict(&leak, &bands()), Verdict::Red { .. }),
        "a monotone climb to 5x is a leak"
    );
    let g = series_growth(&leak, "state_dir_bytes").expect("metric is present");
    assert!((g.ratio - 5.0).abs() < 1e-9, "growth ratio was {}", g.ratio);
}

// -------------------------------------------------------------------------------------
// the executor mirror -- the executor cannot drift from the definition it implements
// -------------------------------------------------------------------------------------

fn executor_source() -> String {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.pop();
    p.push("scripts/f28-native-soak.mjs");
    std::fs::read_to_string(&p).unwrap_or_else(|e| panic!("cannot read {}: {e}", p.display()))
}

#[test]
fn the_executor_mirrors_the_canonical_geometry() {
    let src = executor_source();
    assert!(
        src.contains(&format!("const SESSION_TARGET = {SESSION_TARGET}")),
        "the executor must carry the canonical 1,000-session target"
    );
    assert!(src.contains(&format!("const BLOCK_SIZE = {BLOCK_SIZE}")));
    assert!(src.contains(&format!("const MIN_CONCURRENCY = {MIN_CONCURRENCY}")));
    assert_eq!(SESSION_TARGET / BLOCK_SIZE, BLOCK_COUNT);
}

#[test]
fn the_executor_scans_exactly_the_six_modelled_channels() {
    let src = executor_source();
    let line = src
        .lines()
        .find(|l| l.trim_start().starts_with("const CHANNELS ="))
        .expect("the executor must declare its channel list in one place");
    for channel in CanaryChannel::ALL {
        assert!(
            line.contains(channel.as_str()),
            "the executor's channel list omits `{}`; a dropped channel is how a leak \
             survives a clean-looking scan",
            channel.as_str()
        );
    }
    // and nothing beyond them, so a seventh channel cannot be invented to dilute a count
    assert_eq!(
        line.matches('\'').count(),
        CanaryChannel::ALL.len() * 2,
        "the executor declares a different number of channels than the receipt models"
    );
}

#[test]
fn the_executor_mirrors_the_census_backend_table_including_its_caveat() {
    let src = executor_source();
    for backend in [
        CensusBackend::CgroupV2,
        CensusBackend::WindowsJobObject,
        CensusBackend::ProcessGroupObservedNonauthoritative,
    ] {
        assert!(
            src.contains(backend.as_str()),
            "the executor does not name backend `{}`",
            backend.as_str()
        );
    }
    assert!(
        src.contains("'process-group-observed-nonauthoritative': false"),
        "the executor must record the fallback as NON-authoritative; dropping that is \
         dropping the only honest thing the macOS census can say"
    );
}

#[test]
fn the_executor_never_puts_a_mutating_verb_in_the_workload() {
    let src = executor_source();
    let block = src
        .split("const SAFE_LEAF_VERBS = new Set([")
        .nth(1)
        .and_then(|s| s.split("]);").next())
        .expect("the executor must declare its safe-verb allowlist in one place");
    for forbidden in [
        "install",
        "remove",
        "delete",
        "create",
        "publish",
        "sign",
        "rollback",
        "recover",
        "restore",
        "self-update",
        "uninstall",
        "revoke",
        "cancel",
        "drain",
        "stop",
        "start",
        "restart",
        "daemon",
        "run",
        "login",
        "logout",
        "add",
        "edit",
        "rename",
        "use",
        "submit",
        "pair",
        "advertise",
        "fork",
        "rewind",
        "retry",
        "checkpoint",
        "reconcile",
        "retain",
        "enable",
        "disable",
        "new",
        "test",
        "update",
        "approve",
        "backup",
    ] {
        assert!(
            !block.contains(&format!("'{forbidden}'")),
            "`{forbidden}` is not read-only and must never enter the soak workload -- a \
             soak that runs it a thousand times is a hazard, not a measurement"
        );
    }
}

#[test]
fn the_executor_declares_the_same_panic_sentinels_the_bands_committed() {
    let src = executor_source();
    for sentinel in ["panicked at", "STATUS_ACCESS_VIOLATION", "stack backtrace:"] {
        assert!(
            src.contains(sentinel),
            "the executor does not treat `{sentinel}` as a forbidden sentinel, so a \
             panicking surface could establish an invariant at warm-up"
        );
    }
}
