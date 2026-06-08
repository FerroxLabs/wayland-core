//! `Generation::run` must:
//!   1. Produce exactly `fan_out` scored candidates given an unbounded budget.
//!   2. Terminate early (returning whatever was scored so far) when the
//!      Budget signals cancellation, within one bounded step's slack.
//!   3. Cancel an in-flight slow child when the per-child timeout fires.

use std::sync::Arc;
use std::time::Duration;

use wcore_eval::{DefaultScorer, Scorer};
use wcore_evolve::generation::{BudgetStub, Generation, GenerationParams, TerminationCause};
use wcore_evolve::mutator::{
    Mutation, MutationError, MutationKind, MutationSeed, Mutator, Reorder,
};

fn parent() -> &'static str {
    include_str!("fixtures/parent_skill.md")
}

fn fixture_scorer() -> Box<dyn Scorer + Send + Sync> {
    Box::new(DefaultScorer::default())
}

#[tokio::test]
async fn produces_exact_fan_out_when_budget_is_unbounded() {
    // Per W10A LOCKED PUBLIC SURFACE: Harness::fixture_for_tests requires the
    // `test-utils` feature on wcore-eval (dev-dependencies enable it).
    let mutator = Arc::new(Reorder);
    let params = GenerationParams {
        fan_out: 4,
        budget: Box::new(BudgetStub::unbounded()),
        run_id: "test-run".into(),
        generation: 0,
        parent_hash: "p".into(),
        parent_body: parent().to_string(),
        child_timeout: Duration::from_secs(5),
    };
    let result = Generation::new(mutator, fixture_scorer())
        .run(params)
        .await
        .expect("ok");
    assert_eq!(result.scored.len(), 4);
    assert_eq!(result.terminated_by, TerminationCause::Completed);
}

#[tokio::test]
async fn terminates_early_when_budget_exhausted() {
    let mutator = Arc::new(Reorder);
    let params = GenerationParams {
        fan_out: 100,
        budget: Box::new(BudgetStub::with_max_steps(3)),
        run_id: "test-run".into(),
        generation: 0,
        parent_hash: "p".into(),
        parent_body: parent().to_string(),
        child_timeout: Duration::from_secs(5),
    };
    let result = Generation::new(mutator, fixture_scorer())
        .run(params)
        .await
        .expect("ok");
    assert!(
        result.scored.len() <= 4, // 3 + one bounded step's slack
        "budget exhaustion did not terminate within slack: got {}",
        result.scored.len()
    );
    assert_eq!(result.terminated_by, TerminationCause::BudgetExhausted);
}

/// Slow-mutator that sleeps for `delay` before returning the parent body
/// unchanged. Exercises the per-child timeout path.
struct SlowMockMutator {
    delay: Duration,
}

impl Mutator for SlowMockMutator {
    fn mutate(&self, parent_body: &str, _seed: MutationSeed) -> Result<Mutation, MutationError> {
        std::thread::sleep(self.delay);
        Ok(Mutation {
            body: parent_body.to_string(),
            kind: MutationKind::Reorder,
        })
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn per_child_timeout_cancels_slow_mutator() {
    let mutator = Arc::new(SlowMockMutator {
        delay: Duration::from_secs(2),
    });
    let params = GenerationParams {
        fan_out: 2,
        budget: Box::new(BudgetStub::unbounded()),
        run_id: "test-run".into(),
        generation: 0,
        parent_hash: "p".into(),
        parent_body: parent().to_string(),
        child_timeout: Duration::from_millis(200),
    };
    let started = std::time::Instant::now();
    let result = Generation::new(mutator, fixture_scorer())
        .run(params)
        .await
        .expect("ok");
    let elapsed = started.elapsed();
    // 2 children × ~200ms timeout = ~400ms total, plus minor overhead.
    // Without the timeout this would block for 4s.
    assert!(
        elapsed < Duration::from_millis(1500),
        "per-child timeout did not cancel slow mutator (elapsed = {elapsed:?})"
    );
    // Slow children record as timed_out and are excluded from `scored`.
    assert!(
        result.scored.is_empty(),
        "timed-out children should not appear in scored"
    );
    assert_eq!(result.timed_out_children.len(), 2);
    assert_eq!(result.terminated_by, TerminationCause::ChildTimedOut);
}
