//! SwapSynonym mutator. Deterministic pick from a small static synonym table;
//! replaces the FIRST occurrence in the parent body.

use rand::seq::IteratorRandom;

use super::{Mutation, MutationError, MutationKind, MutationSeed, Mutator};

const SYNONYMS: &[(&str, &[&str])] = &[
    ("read", &["open", "load", "inspect"]),
    ("write", &["emit", "save", "persist"]),
    ("identify", &["locate", "find", "detect"]),
    ("sort", &["order", "arrange"]),
    ("verify", &["confirm", "check"]),
    ("Read", &["Open", "Load", "Inspect"]),
    ("Identify", &["Locate", "Find", "Detect"]),
    ("Sort", &["Order", "Arrange"]),
];

#[derive(Default)]
pub struct SwapSynonym;

impl Mutator for SwapSynonym {
    fn mutate(&self, parent_body: &str, seed: MutationSeed) -> Result<Mutation, MutationError> {
        let mut rng = seed.rng();
        let candidates: Vec<(&str, &str)> = SYNONYMS
            .iter()
            .filter(|(w, _)| parent_body.contains(w))
            .flat_map(|(w, subs)| subs.iter().map(move |s| (*w, *s)))
            .collect();
        let (from, to) = candidates
            .into_iter()
            .choose(&mut rng)
            .ok_or(MutationError::NoSynonymCandidate)?;
        let body = parent_body.replacen(from, to, 1);
        Ok(Mutation {
            body,
            kind: MutationKind::SwapSynonym,
        })
    }
}
