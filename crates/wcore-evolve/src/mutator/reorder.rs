//! Reorder mutator. Shuffles the `## Steps` list deterministically.

use rand::seq::SliceRandom;

use super::{Mutation, MutationError, MutationKind, MutationSeed, Mutator};

#[derive(Default)]
pub struct Reorder;

impl Mutator for Reorder {
    fn mutate(&self, parent_body: &str, seed: MutationSeed) -> Result<Mutation, MutationError> {
        let mut rng = seed.rng();
        let (head, steps_block, tail) =
            split_steps_section(parent_body).ok_or(MutationError::NoStepsSection)?;

        let mut items: Vec<&str> = steps_block
            .lines()
            .filter(|l| l.trim_start().starts_with("- "))
            .collect();
        items.shuffle(&mut rng);

        let reordered = items
            .iter()
            .map(|s| (*s).to_string())
            .collect::<Vec<_>>()
            .join("\n");
        let body = format!("{head}## Steps\n{reordered}\n{tail}");

        Ok(Mutation {
            body,
            kind: MutationKind::Reorder,
        })
    }
}

fn split_steps_section(body: &str) -> Option<(&str, &str, &str)> {
    let start = body.find("## Steps")?;
    let after_heading_rel = body.get(start..)?.find('\n')?;
    let after_heading = start + after_heading_rel + 1;
    let next_section = body
        .get(after_heading..)
        .and_then(|tail| tail.find("\n## ").map(|p| p + after_heading));
    let end = next_section.unwrap_or(body.len());
    let head = body.get(..start)?;
    let block = body.get(after_heading..end)?.trim_end();
    let tail = body.get(end..)?;
    Some((head, block, tail))
}
