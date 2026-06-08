//! Reproducibility floor for W10B mutations.
//!
//! Same (parent, seed) MUST produce byte-identical children every run.
//! If this regresses, evolutionary runs become non-reproducible and
//! every downstream artifact (graveyard, trace, curator audit) loses
//! its ability to attribute a winner/loser to a specific lineage.

use std::sync::Arc;

use wcore_evolve::mutator::{
    MutationKind, MutationSeed, Mutator, Paraphrase, ParaphraseProvider, Precondition, Reorder,
    SwapSynonym,
};

const PARENT_BODY: &str = include_str!("fixtures/parent_skill.md");
const PARAPHRASE_FIXTURE: &str = include_str!("fixtures/paraphrase/run-0-child-0.txt");

fn seed_for(generation: u32, child: u32) -> MutationSeed {
    MutationSeed::new("test-parent-hash", generation, child)
}

/// Fixture-replay provider. Returns the recorded LLM response regardless of
/// input. This is the only honest determinism contract for Paraphrase: real
/// provider drift is not W10B's problem; the recorded fixture is.
struct FixtureProvider {
    response: &'static str,
}

impl ParaphraseProvider for FixtureProvider {
    fn paraphrase_blocking(&self, _body: &str, _seed_token: &str) -> Result<String, String> {
        Ok(self.response.to_string())
    }
}

#[test]
fn reorder_is_byte_equal_across_repeated_runs() {
    let mutator = Reorder;
    let a = mutator.mutate(PARENT_BODY, seed_for(0, 0)).expect("ok");
    let b = mutator.mutate(PARENT_BODY, seed_for(0, 0)).expect("ok");
    assert_eq!(a.body, b.body, "reorder is non-deterministic");
    assert_eq!(a.kind, MutationKind::Reorder);
}

#[test]
fn swap_synonym_is_byte_equal_across_repeated_runs() {
    let mutator = SwapSynonym;
    let a = mutator.mutate(PARENT_BODY, seed_for(0, 1)).expect("ok");
    let b = mutator.mutate(PARENT_BODY, seed_for(0, 1)).expect("ok");
    assert_eq!(a.body, b.body, "swap_synonym is non-deterministic");
    assert_eq!(a.kind, MutationKind::SwapSynonym);
}

#[test]
fn precondition_is_byte_equal_across_repeated_runs() {
    let mutator = Precondition;
    let a = mutator.mutate(PARENT_BODY, seed_for(0, 2)).expect("ok");
    let b = mutator.mutate(PARENT_BODY, seed_for(0, 2)).expect("ok");
    assert_eq!(a.body, b.body, "precondition is non-deterministic");
    assert_eq!(a.kind, MutationKind::Precondition);
}

#[test]
fn paraphrase_is_byte_equal_via_recorded_fixture() {
    // Paraphrase determinism is FIXTURE-REPLAY, not LLM-level determinism.
    // Anthropic/OpenAI `temperature=0` is near-deterministic, not strictly so
    // (sampler RNG, batched inference, silent provider model updates). The
    // ONLY honest determinism contract is "given the same recorded response,
    // produce the same Mutation." Real-provider calls live behind
    // `--features network-tests` and `#[ignore]`.
    let provider: Arc<dyn ParaphraseProvider> = Arc::new(FixtureProvider {
        response: PARAPHRASE_FIXTURE,
    });
    let mutator = Paraphrase {
        provider,
        temperature: 0.0,
    };
    let a = mutator.mutate(PARENT_BODY, seed_for(0, 0)).expect("ok");
    let b = mutator.mutate(PARENT_BODY, seed_for(0, 0)).expect("ok");
    assert_eq!(
        a.body, b.body,
        "paraphrase fixture-replay is non-deterministic"
    );
    assert_eq!(a.kind, MutationKind::Paraphrase);
    assert_eq!(
        a.body, PARAPHRASE_FIXTURE,
        "paraphrase must return the fixture body verbatim"
    );
}

#[test]
fn different_child_index_produces_different_body() {
    let mutator = Reorder;
    let a = mutator.mutate(PARENT_BODY, seed_for(0, 0)).expect("ok");
    let b = mutator.mutate(PARENT_BODY, seed_for(0, 3)).expect("ok");
    assert_ne!(
        a.body, b.body,
        "two children with different seeds must diverge for at least one mutator"
    );
}

#[test]
fn precondition_never_drops_below_one_item() {
    // Invariant: after `precondition: drop`, the body still has at least one
    // `## Preconditions` row. Otherwise the resulting skill is malformed and
    // the harness rejects it for trivial reasons (not the candidate's fault).
    let mutator = Precondition;
    let result = mutator.mutate(PARENT_BODY, seed_for(99, 99)).expect("ok");
    // count rows in the Preconditions block (between "## Preconditions" and "## Steps")
    let pre_start = result
        .body
        .find("## Preconditions")
        .expect("preconditions section exists");
    let pre_after = result.body[pre_start..]
        .find('\n')
        .map(|p| p + pre_start + 1)
        .expect("preconditions heading newline");
    let pre_end = result.body[pre_after..]
        .find("\n## ")
        .map(|p| p + pre_after)
        .unwrap_or(result.body.len());
    let block = &result.body[pre_after..pre_end];
    let rows = block
        .lines()
        .filter(|l| l.trim_start().starts_with("- "))
        .count();
    assert!(
        rows >= 1,
        "precondition mutator violated 'never empty' invariant: rows={rows}, body=\n{}",
        result.body
    );
}
