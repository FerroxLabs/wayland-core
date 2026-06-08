//! Hand-off contract: winners flow through the curator trait; no direct
//! filesystem writes under the active catalog.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use wcore_evolve::curator_handoff::{CuratorPort, Decision, Handoff, Lineage};
use wcore_evolve::generation::ScoredCandidate;
use wcore_evolve::mutator::{Mutation, MutationKind};

#[derive(Default)]
struct MockCurator {
    received: AtomicUsize,
    last_lineage: std::sync::Mutex<Option<Lineage>>,
}

#[async_trait]
impl CuratorPort for MockCurator {
    async fn submit(&self, _body: &str, lineage: &Lineage) -> Result<Decision, String> {
        self.received.fetch_add(1, Ordering::Relaxed);
        *self.last_lineage.lock().expect("lock") = Some(lineage.clone());
        Ok(Decision::Promote)
    }
}

fn make_scored_candidate() -> ScoredCandidate {
    use wcore_eval::{ScoreDimensions, ScoreOutcome, Verdict};
    ScoredCandidate {
        mutation: Mutation {
            body: "## Steps\n- a\n".to_string(),
            kind: MutationKind::Reorder,
        },
        score: ScoreOutcome {
            dimensions: ScoreDimensions {
                outcome: 0.75,
                cost_penalty: 0.0,
                size_penalty: 0.0,
                combined: 0.75,
            },
            predicted: Verdict::Good,
        },
        child_index: 3,
    }
}

#[tokio::test]
async fn winner_routes_to_curator_not_to_disk() {
    let curator = Arc::new(MockCurator::default());
    let handoff = Handoff::new(curator.clone());
    let winner = make_scored_candidate();
    let decision = handoff
        .promote(&winner, "skill-refactor-imports", "run-1")
        .await
        .expect("ok");
    assert_eq!(decision, Decision::Promote);
    assert_eq!(curator.received.load(Ordering::Relaxed), 1);
    let last = curator
        .last_lineage
        .lock()
        .expect("lock")
        .as_ref()
        .cloned()
        .expect("lineage recorded");
    assert_eq!(last.run_id, "run-1");
    assert_eq!(last.parent_id, "skill-refactor-imports");
    assert_eq!(last.child_index, 3);
    assert!((last.score - 0.75).abs() < 1e-9);
}

#[tokio::test]
async fn curator_rejection_surfaces_as_evolve_error() {
    struct RejectingCurator;
    #[async_trait]
    impl CuratorPort for RejectingCurator {
        async fn submit(&self, _body: &str, _lineage: &Lineage) -> Result<Decision, String> {
            Err("policy violation".into())
        }
    }
    let handoff = Handoff::new(Arc::new(RejectingCurator));
    let winner = make_scored_candidate();
    let err = handoff
        .promote(&winner, "p", "r")
        .await
        .expect_err("should reject");
    let msg = err.to_string();
    assert!(
        msg.contains("policy violation"),
        "expected wrapped curator error, got: {msg}"
    );
}
