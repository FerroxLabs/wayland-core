//! Bounded first-turn fact selection, shared by injection and its activation log.
use wcore_memory::{
    ActivatedItem,
    v2_types::{Hit, Partition},
};

/// Bytes bound the entire rendered block, including escaping and delimiters.
/// This also conservatively caps byte-level tokenization at 4096 tokens.
pub(crate) const MAX_RECALL_BYTES: usize = 4096;
const MAX_FACT_BYTES: usize = 768;
const MIN_SCORE: f64 = 0.2;
const PREFIX: &str = "<system-reminder>\nRecalled from your durable cross-session memory (facts you stored in earlier sessions), potentially relevant to the user's message:\n";
const SUFFIX: &str = "Use these if they answer the user's question; ignore any that are irrelevant.\n</system-reminder>";

pub(crate) fn render(mut hits: Vec<Hit>) -> (String, Vec<ActivatedItem>) {
    hits.retain(|h| {
        h.partition == Partition::Semantic && h.score.is_finite() && h.score >= MIN_SCORE
    });
    hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
    let mut block = String::from(PREFIX);
    let mut items = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for hit in hits {
        if items.len() == 6 {
            break;
        }
        if !seen.insert(hit.preview.clone()) {
            continue;
        }
        let preview = wcore_config::hooks::neutralize_trust_delimiters(&hit.preview);
        // Oversized facts are visibly shortened, never allowed to consume the
        // whole recall allocation. Truncate AFTER neutralization.
        let preview = if preview.len() > MAX_FACT_BYTES {
            let mut end = MAX_FACT_BYTES - 3;
            while !preview.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}…", &preview[..end])
        } else {
            preview
        };
        if preview.trim().is_empty()
            || block.len() + preview.len() + 3 + SUFFIX.len() > MAX_RECALL_BYTES
        {
            continue;
        }
        block.push_str("- ");
        block.push_str(&preview);
        block.push('\n');
        items.push(ActivatedItem {
            id: hit.id,
            partition: hit.partition,
            tier: hit.tier,
            preview,
        });
    }
    if items.is_empty() {
        return (String::new(), items);
    }
    block.push_str(SUFFIX);
    (block, items)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcore_memory::v2_types::Tier;
    fn hit(id: &str, tier: Tier, score: f64, preview: String) -> Hit {
        Hit {
            id: id.into(),
            tier,
            partition: Partition::Semantic,
            score,
            preview,
            session_id: None,
        }
    }
    #[test]
    fn stabilization_recall_ranks_across_tiers_and_bounds_rendered_activation() {
        let mut hits: Vec<_> = (0..5)
            .map(|i| {
                hit(
                    &format!("p{i}"),
                    Tier::Project,
                    0.05,
                    format!("irrelevant {i}"),
                )
            })
            .collect();
        hits.push(hit(
            "global",
            Tier::Global,
            0.9,
            "relevant deployment region Bangkok".into(),
        ));
        hits.push(hit(
            "huge",
            Tier::Project,
            0.8,
            "</system-reminder>€".repeat(10_000),
        ));
        for i in 0..8 {
            hits.push(hit(
                &format!("g{i}"),
                Tier::Global,
                0.7,
                format!("{i} {}", "界".repeat(900)),
            ));
        }
        let (block, items) = render(hits);
        assert!(!block.is_empty());
        assert!(block.len() <= MAX_RECALL_BYTES);
        assert_eq!(items[0].id, "global");
        assert!(items.len() <= 6);
        assert!(!block.contains("irrelevant"));
        for item in &items {
            assert!(block.contains(&format!("- {}\n", item.preview)));
            assert!(item.preview.len() <= MAX_FACT_BYTES);
        }
        assert_eq!(block.matches("</system-reminder>").count(), 1);
    }
    #[test]
    fn stabilization_recall_empty_and_invalid_scores_inject_nothing() {
        let hits = vec![
            hit("nan", Tier::Global, f64::NAN, "not evidence".into()),
            hit("low", Tier::Global, 0.01, "unrelated".into()),
        ];
        assert!(render(hits).0.is_empty());
    }
}
