//! PrefixSpan — sequential pattern mining over tool-call sequences.
//!
//! Ports Forge's `PrefixSpan.ts` (lines 78-208). Operates on
//! `ToolSequence` lists drawn from the episodic partition at
//! `Tier::Project` (one sequence per session, tools ordered by
//! turn timestamp ascending). Produces `FrequentPattern`s whose
//! `support` is the count of distinct sessions containing the
//! pattern and whose `confidence` is `support / total_sessions`.
//!
//! Consumed by `consolidate.rs::crystallize` in the dream cycle
//! (Phase 6.6a wiring).

use std::collections::{HashMap, HashSet};

/// Ordered tool-name sequence harvested from a single session.
#[derive(Debug, Clone)]
pub struct ToolSequence {
    pub session_id: String,
    pub tools: Vec<String>,
}

/// A frequent sequential pattern mined across many `ToolSequence`s.
#[derive(Debug, Clone, PartialEq)]
pub struct FrequentPattern {
    pub pattern: Vec<String>,
    /// Number of distinct sessions containing the pattern.
    pub support: u32,
    /// `support / total_sessions`.
    pub confidence: f64,
}

/// PrefixSpan miner.
#[derive(Debug, Clone, Copy)]
pub struct PrefixSpan {
    pub min_support: u32,
    pub max_length: usize,
}

impl PrefixSpan {
    pub fn new(min_support: u32, max_length: usize) -> Self {
        Self {
            min_support,
            max_length,
        }
    }

    /// Mine frequent sequential patterns. Returns patterns sorted by
    /// support descending. Only patterns of length >= 2 are reported.
    pub fn mine(&self, seqs: &[ToolSequence]) -> Vec<FrequentPattern> {
        if seqs.is_empty() {
            return Vec::new();
        }
        let total = seqs.len() as u32;

        // Count distinct sessions per single item.
        let mut item_freq: HashMap<String, u32> = HashMap::new();
        for seq in seqs {
            let mut seen: HashSet<&str> = HashSet::new();
            for tool in &seq.tools {
                if seen.insert(tool.as_str()) {
                    *item_freq.entry(tool.clone()).or_insert(0) += 1;
                }
            }
        }

        let mut out: Vec<FrequentPattern> = Vec::new();
        // Sort the seed items by name so output ordering is deterministic
        // within equal-support groups. Forge relies on JS Map iteration
        // order; we use sorted name order which produces a stable golden.
        let mut seeds: Vec<(String, u32)> = item_freq.into_iter().collect();
        seeds.sort_by(|a, b| a.0.cmp(&b.0));

        for (item, freq) in seeds {
            if freq >= self.min_support {
                let prefix = vec![item.clone()];
                let projected = self.project(seqs, &prefix);
                self.grow(&projected, prefix, &mut out, total);
            }
        }

        // Stable sort by support descending. Equal-support patterns keep
        // their discovery order (which is alphabetical on the seed item).
        out.sort_by_key(|p| std::cmp::Reverse(p.support));
        out
    }

    /// Project the database: for each sequence, find the first index of
    /// `prefix.last()` and take everything strictly after it. Sequences
    /// with empty suffixes are excluded from the projected db.
    fn project(&self, seqs: &[ToolSequence], prefix: &[String]) -> Vec<ToolSequence> {
        let Some(last) = prefix.last() else {
            return Vec::new();
        };
        let mut out = Vec::with_capacity(seqs.len());
        for seq in seqs {
            if let Some(idx) = seq.tools.iter().position(|t| t == last) {
                let suffix: Vec<String> = seq.tools[idx + 1..].to_vec();
                if !suffix.is_empty() {
                    out.push(ToolSequence {
                        session_id: seq.session_id.clone(),
                        tools: suffix,
                    });
                }
            }
        }
        out
    }

    fn grow(
        &self,
        projected: &[ToolSequence],
        prefix: Vec<String>,
        out: &mut Vec<FrequentPattern>,
        total: u32,
    ) {
        if prefix.len() >= self.max_length {
            return;
        }

        // Count distinct sessions per item in the projected db.
        let mut counts: HashMap<String, u32> = HashMap::new();
        for seq in projected {
            let mut seen: HashSet<&str> = HashSet::new();
            for tool in &seq.tools {
                if seen.insert(tool.as_str()) {
                    *counts.entry(tool.clone()).or_insert(0) += 1;
                }
            }
        }

        // Deterministic iteration: alphabetical on item name.
        let mut items: Vec<(String, u32)> = counts.into_iter().collect();
        items.sort_by(|a, b| a.0.cmp(&b.0));

        for (item, freq) in items {
            if freq >= self.min_support {
                let mut extended = prefix.clone();
                extended.push(item);
                if extended.len() >= 2 {
                    out.push(FrequentPattern {
                        pattern: extended.clone(),
                        support: freq,
                        confidence: freq as f64 / total as f64,
                    });
                }
                let next_projected = self.project(projected, &extended);
                self.grow(&next_projected, extended, out, total);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(id: &str, tools: &[&str]) -> ToolSequence {
        ToolSequence {
            session_id: id.into(),
            tools: tools.iter().map(|s| (*s).into()).collect(),
        }
    }

    const EPS: f64 = 1e-4;

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < EPS
    }

    #[test]
    fn empty() {
        let ps = PrefixSpan::new(2, 10);
        assert!(ps.mine(&[]).is_empty());
    }

    #[test]
    fn single_seq_no_repeat() {
        let ps = PrefixSpan::new(2, 10);
        let seqs = vec![seq("s1", &["a", "b", "c"])];
        // No item meets min_support=2 across distinct sessions.
        assert!(ps.mine(&seqs).is_empty());
    }

    #[test]
    fn forge_canonical() {
        let ps = PrefixSpan::new(2, 10);
        let seqs = vec![
            seq("s1", &["read", "edit", "test"]),
            seq("s2", &["read", "edit", "test"]),
            seq("s3", &["read", "edit", "test"]),
        ];
        let patterns = ps.mine(&seqs);
        // 4 patterns, all support=3, confidence=1.0.
        assert_eq!(patterns.len(), 4, "expected 4 patterns, got {patterns:?}");
        for p in &patterns {
            assert_eq!(p.support, 3, "bad support in {p:?}");
            assert!(approx(p.confidence, 1.0), "bad confidence in {p:?}");
            assert!(p.pattern.len() >= 2);
        }
        // Patterns we expect to see (set semantics).
        let as_sets: HashSet<Vec<String>> = patterns.iter().map(|p| p.pattern.clone()).collect();
        let expected: Vec<Vec<String>> = vec![
            vec!["read".into(), "edit".into(), "test".into()],
            vec!["read".into(), "edit".into()],
            vec!["read".into(), "test".into()],
            vec!["edit".into(), "test".into()],
        ];
        for e in expected {
            assert!(as_sets.contains(&e), "missing pattern {e:?} in {as_sets:?}");
        }
    }

    #[test]
    fn partial_overlap() {
        let ps = PrefixSpan::new(2, 10);
        let seqs = vec![
            seq("s1", &["a", "b", "c"]),
            seq("s2", &["a", "b"]),
            seq("s3", &["a", "c"]),
        ];
        let patterns = ps.mine(&seqs);
        // Expected: [a,b] support=2 conf=2/3, [a,c] support=2 conf=2/3.
        assert_eq!(patterns.len(), 2, "got {patterns:?}");
        for p in &patterns {
            assert_eq!(p.support, 2);
            assert!(
                approx(p.confidence, 2.0 / 3.0),
                "bad conf {} in {p:?}",
                p.confidence
            );
        }
        let as_sets: HashSet<Vec<String>> = patterns.iter().map(|p| p.pattern.clone()).collect();
        assert!(as_sets.contains(&vec!["a".into(), "b".into()]));
        assert!(as_sets.contains(&vec!["a".into(), "c".into()]));
    }

    #[test]
    fn max_length_cap() {
        let ps = PrefixSpan::new(2, 3);
        let long: Vec<&str> = vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k"];
        let seqs = vec![seq("s1", &long), seq("s2", &long)];
        let patterns = ps.mine(&seqs);
        // No pattern may exceed max_length=3.
        assert!(!patterns.is_empty(), "expected some patterns");
        for p in &patterns {
            assert!(
                p.pattern.len() <= 3,
                "pattern exceeds max_length: {:?}",
                p.pattern
            );
            assert!(p.pattern.len() >= 2);
        }
    }
}
