//! Plateau heuristic regression tests. Default window=3 per High 1 audit fix.

use wcore_evolve::evolve::PlateauDetector;

#[test]
fn no_plateau_when_score_keeps_improving() {
    // Default window = 3. Detector requires >window entries before it can
    // declare a plateau.
    let mut d = PlateauDetector::new(3, 0.01);
    for s in [0.50, 0.55, 0.60, 0.65, 0.70] {
        d.push(s).expect("finite score");
    }
    assert!(!d.should_terminate());
}

#[test]
fn plateau_after_k_flat_generations() {
    let mut d = PlateauDetector::new(3, 0.01);
    for s in [0.80, 0.80, 0.80, 0.80, 0.80] {
        d.push(s).expect("finite score");
    }
    assert!(d.should_terminate());
}

#[test]
fn improvement_within_min_delta_still_counts_as_plateau() {
    let mut d = PlateauDetector::new(3, 0.05);
    for s in [0.80, 0.81, 0.82, 0.83] {
        d.push(s).expect("finite score");
    }
    assert!(
        d.should_terminate(),
        "0.01 deltas should not break a 0.05 min_delta plateau"
    );
}

#[test]
fn one_noisy_dip_inside_window_does_not_trigger_plateau() {
    // High 1 regression test: with window = 3 and 4 mutators rotated
    // round-robin, a single noisy generation (e.g. all-paraphrase dip) must
    // not produce a false plateau if a later generation recovers above
    // baseline by more than min_delta.
    let mut d = PlateauDetector::new(3, 0.01);
    for s in [0.70, 0.71, 0.65, 0.85] {
        d.push(s).expect("finite score");
    }
    assert!(
        !d.should_terminate(),
        "single-generation dip inside the window should not trigger plateau when later generations recover"
    );
}
