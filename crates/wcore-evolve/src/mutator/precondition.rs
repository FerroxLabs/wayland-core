//! Precondition mutator. Adds or drops one row from the `## Preconditions`
//! list. Invariant: never drops below one row.

use rand::Rng;

use super::{Mutation, MutationError, MutationKind, MutationSeed, Mutator};

#[derive(Default)]
pub struct Precondition;

impl Mutator for Precondition {
    fn mutate(&self, parent_body: &str, seed: MutationSeed) -> Result<Mutation, MutationError> {
        let mut rng = seed.rng();
        let (head, block, tail) = split_section(parent_body, "## Preconditions")
            .ok_or(MutationError::NoPreconditionsSection)?;

        let mut items: Vec<String> = block
            .lines()
            .filter(|l| l.trim_start().starts_with("- "))
            .map(|s| s.to_string())
            .collect();

        let do_drop = items.len() > 1 && rng.gen_bool(0.5);
        if do_drop {
            let idx = rng.gen_range(0..items.len());
            // bounds-checked via the gen_range upper bound; Vec::remove panics
            // only on out-of-range which `gen_range(0..len)` cannot produce.
            // We additionally guard via `.get` semantics for clarity.
            if idx < items.len() {
                items.remove(idx);
            }
        } else {
            items.push("- The session has at least one prior tool call".into());
        }

        let body = format!("{head}## Preconditions\n{}\n{tail}", items.join("\n"));
        Ok(Mutation {
            body,
            kind: MutationKind::Precondition,
        })
    }
}

fn split_section<'a>(body: &'a str, heading: &str) -> Option<(&'a str, &'a str, &'a str)> {
    let start = body.find(heading)?;
    let after_heading_rel = body.get(start..)?.find('\n')?;
    let after_heading = start + after_heading_rel + 1;
    let next = body
        .get(after_heading..)
        .and_then(|tail| tail.find("\n## ").map(|p| p + after_heading));
    let end = next.unwrap_or(body.len());
    let head = body.get(..start)?;
    let block = body.get(after_heading..end)?.trim_end();
    let tail = body.get(end..)?;
    Some((head, block, tail))
}
