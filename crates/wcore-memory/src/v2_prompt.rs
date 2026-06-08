// v2 taxonomy prompt. Describes the 5×3 model, the three access tokens,
// the dream cycle, Letta compaction, and which tools the agent has to
// interact with each partition.
//
// v1 prompt (build_memory_prompt_minimal in prompt.rs) is kept until
// wcore-agent does its cutover; both can coexist.

#[derive(Debug, Clone, Default)]
pub struct MemoryStats {
    pub episode_count: u64,
    pub staged_procedure_count: u64,
    pub days_since_last_dream: f64,
}

pub fn v2_taxonomy_prompt(stats: &MemoryStats) -> String {
    format!(
        "## Memory (v2 — 5 partitions × 3 tiers)

Episodes: {} | Staged procedures: {} | Days since last dream: {:.1}

Partitions (deny-by-default — your token determines what you can do):
- P1 Working    (Session only): live turns + tool calls. Auto-managed.
- P2 Episodic   (Session/Project/Global): timestamped events. Use
                `record_episode` to log a meaningful interaction.
- P3 Semantic   (Project/Global): distilled facts as (subject, predicate,
                object) triples. Use `assert_fact` when a new durable
                truth emerges; supersedes are automatic when the prior
                fact's object differs.
- P4 Procedural (Project/Global): reusable skill artifacts with Thompson
                stats. Use `upsert_procedure` rarely; the dream cycle
                crystallizes patterns into staged procedures.
- P5 Core       (Global only): user model k/v. SYSTEM-only write. You
                can `user_model()` to read if your scope allows it.

Tiers:
- session: ephemeral, scoped to this run. P1+P2 only.
- project: scoped to this project root. P2-P4.
- global:  user-wide, cross-project. P2-P5.

Search: `search(query, tier)` returns episodes (BM25 + vector + KG, RRF
fused). Use precise keywords; ask follow-up queries rather than over-
fetching.

Dream cycle (`dream_now`): compress (P1→P2), consolidate (P2→P3),
crystallize (P3→P4), decay. Runs at session end + idle.

Letta compaction (`compact`): non-destructive offload of oldest P1
turns into a P2 bookmark when the window grows. Recoverable via search.

ACL: deny-by-default. Sub-agents need explicit (partition, tier) scopes
in their YAML. P5 writes are system-only; P5 reads require a granted
scope. Any denial appears in audit.db.
",
        stats.episode_count, stats.staged_procedure_count, stats.days_since_last_dream
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_mentions_all_partitions_and_tiers() {
        let p = v2_taxonomy_prompt(&MemoryStats {
            episode_count: 7,
            staged_procedure_count: 2,
            days_since_last_dream: 1.5,
        });
        for name in ["P1", "P2", "P3", "P4", "P5"] {
            assert!(p.contains(name), "missing {name}");
        }
        for name in ["session", "project", "global"] {
            assert!(p.contains(name), "missing {name}");
        }
        for tool in [
            "record_episode",
            "assert_fact",
            "upsert_procedure",
            "search",
            "dream_now",
            "compact",
        ] {
            assert!(p.contains(tool), "missing tool {tool}");
        }
        assert!(p.contains("deny-by-default"));
        assert!(p.contains("system-only") || p.contains("SYSTEM-only"));
        assert!(p.contains("7"));
        assert!(p.contains("2"));
    }
}
