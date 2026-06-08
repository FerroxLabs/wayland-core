//! T2-D3 — Heuristic fact extractor.
//!
//! Ported from `ijfw/mcp-server/src/memory/fact-extractor.js` (H5.5).
//!
//! The extractor walks input text through a small set of hand-rolled regex
//! patterns; each pattern is a [`FactPattern`] with three required named
//! capture groups: `subject`, `predicate`, and `object`. When a pattern
//! matches, the extractor yields a [`Fact`] tagged with that pattern's
//! confidence score.
//!
//! Design constraints (mirroring the JS source):
//!  - Zero LLM calls — purely deterministic.
//!  - Conservative: prefer a miss over a false-positive.
//!  - Output is structural, not semantic. Consumers can post-process.
//!
//! Two constructors are provided:
//!  - [`FactExtractor::default`] ships with hand-validated literal patterns
//!    and is infallible — its inner `Regex::new(...).expect(...)` is safe
//!    because the literals are unit-tested.
//!  - [`FactExtractor::new_validated`] accepts user-supplied patterns and
//!    returns [`FactExtractorError::MissingNamedGroup`] if any pattern lacks
//!    one of the three required capture groups.

use regex::Regex;
use rusqlite::Connection;
use thiserror::Error;

use crate::kg::{EdgeKind, NodeKind, upsert_edge, upsert_node};

/// A single extracted (subject, predicate, object) triple plus its
/// pattern-specificity score.
#[derive(Debug, Clone, PartialEq)]
pub struct Fact {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    /// Confidence in [0.0, 1.0]; higher = more specific pattern.
    pub confidence: f32,
}

/// A single regex pattern paired with the confidence assigned to its
/// matches. The regex MUST contain three named capture groups:
/// `subject`, `predicate`, and `object`.
#[derive(Debug, Clone)]
pub struct FactPattern {
    /// Regex with three named groups: `subject`, `predicate`, `object`.
    pub regex: Regex,
    /// Confidence assigned to facts matched by this pattern.
    pub confidence: f32,
}

/// The heuristic fact extractor.
#[derive(Debug, Clone)]
pub struct FactExtractor {
    patterns: Vec<FactPattern>,
}

/// Errors surfaced by validated constructors.
#[derive(Debug, Error)]
pub enum FactExtractorError {
    /// A user-supplied pattern is missing one of the three required
    /// named capture groups.
    #[error("pattern `{pattern}` is missing required named group `{group}`")]
    MissingNamedGroup {
        pattern: String,
        group: &'static str,
    },
}

/// Required named capture groups (in order: subject, predicate, object).
const REQUIRED_GROUPS: [&str; 3] = ["subject", "predicate", "object"];

impl FactExtractor {
    /// Construct an extractor from the provided patterns without
    /// validating their named-group surface. Useful when patterns have
    /// already been verified (e.g. literal constants).
    pub fn with_patterns(patterns: Vec<FactPattern>) -> Self {
        Self { patterns }
    }

    /// Construct an extractor from user-supplied patterns, verifying that
    /// each regex declares all three required named groups
    /// (`subject`, `predicate`, `object`).
    pub fn new_validated(patterns: Vec<FactPattern>) -> Result<Self, FactExtractorError> {
        for p in &patterns {
            let names: Vec<Option<&str>> = p.regex.capture_names().collect();
            for required in REQUIRED_GROUPS {
                if !names.contains(&Some(required)) {
                    return Err(FactExtractorError::MissingNamedGroup {
                        pattern: p.regex.as_str().to_string(),
                        group: match required {
                            "subject" => "subject",
                            "predicate" => "predicate",
                            "object" => "object",
                            _ => unreachable!(),
                        },
                    });
                }
            }
        }
        Ok(Self { patterns })
    }

    /// Extract facts by walking every pattern across the full text. The
    /// returned list preserves match order and may contain duplicates if
    /// overlapping patterns hit the same triple — use
    /// [`Self::extract_with_dedup`] to collapse those.
    pub fn extract(&self, text: &str) -> Vec<Fact> {
        let mut facts = Vec::new();
        if text.trim().is_empty() {
            return facts;
        }
        for pattern in &self.patterns {
            for caps in pattern.regex.captures_iter(text) {
                // .expect() is safe here: validated patterns are
                // guaranteed to declare these groups, and `Default` uses
                // hand-checked literals. `captures_iter` only yields
                // when the whole match succeeds, so the named sub-
                // captures exist.
                let subject = caps
                    .name("subject")
                    .expect("pattern enforces subject group")
                    .as_str()
                    .trim()
                    .to_string();
                let predicate = caps
                    .name("predicate")
                    .expect("pattern enforces predicate group")
                    .as_str()
                    .trim()
                    .to_string();
                let object = caps
                    .name("object")
                    .expect("pattern enforces object group")
                    .as_str()
                    .trim()
                    .to_string();
                if subject.is_empty() || predicate.is_empty() || object.is_empty() {
                    continue;
                }
                facts.push(Fact {
                    subject,
                    predicate,
                    object,
                    confidence: pattern.confidence,
                });
            }
        }
        facts
    }

    /// Like [`Self::extract`], but collapses duplicates keyed on the
    /// lowercased (subject, predicate, object) triple. First occurrence
    /// wins — keeps the confidence of whichever pattern matched first.
    pub fn extract_with_dedup(&self, text: &str) -> Vec<Fact> {
        let raw = self.extract(text);
        let mut seen: std::collections::HashSet<(String, String, String)> =
            std::collections::HashSet::new();
        let mut out = Vec::with_capacity(raw.len());
        for f in raw {
            let key = (
                f.subject.to_lowercase(),
                f.predicate.to_lowercase(),
                f.object.to_lowercase(),
            );
            if seen.insert(key) {
                out.push(f);
            }
        }
        out
    }
}

/// W5 — fact extractor → knowledge-graph ingest pipeline.
///
/// Runs the default [`FactExtractor`] over `transcript`, then upserts every
/// extracted triple into the knowledge graph against the raw `&Connection`:
/// the subject and object become [`NodeKind::Entity`] nodes and the
/// predicate becomes a [`EdgeKind`]-tagged edge between them (the fact's
/// confidence is carried as the edge weight).
///
/// Returns the number of facts that were upserted. Callers wire this at
/// session/turn end behind [`crate::kg::kg_enabled`]; the KG schema must
/// already exist on `conn` (W2 wires `init_kg` under the same gate).
pub fn ingest_facts_to_kg(conn: &Connection, transcript: &str) -> crate::error::Result<usize> {
    let facts = FactExtractor::default().extract_with_dedup(transcript);
    let mut ingested = 0usize;
    for fact in &facts {
        let subj = upsert_node(conn, &fact.subject, &NodeKind::Entity)?;
        let obj = upsert_node(conn, &fact.object, &NodeKind::Entity)?;
        // The predicate is structural metadata, not one of the three known
        // edge kinds — round-trip it losslessly through `EdgeKind::Other`.
        let kind = EdgeKind::Other(fact.predicate.clone());
        upsert_edge(conn, subj, obj, &kind, fact.confidence)?;
        ingested += 1;
    }
    Ok(ingested)
}

impl Default for FactExtractor {
    /// Ships with 5 hand-validated default patterns ported from the JS
    /// source. Each literal regex is unit-tested below, so the
    /// `.expect(...)` calls are statically safe.
    fn default() -> Self {
        let patterns = vec![
            // "X is a/an/the Y" — copular sentence with article.
            // Confidence 0.70 (covers JS's copular "is" branch).
            FactPattern {
                regex: Regex::new(
                    r"(?P<subject>\w+) (?P<predicate>is) (?:a|an|the) (?P<object>\w+)",
                )
                .expect("hand-validated literal `is a/an/the`"),
                confidence: 0.70,
            },
            // "X has/owns/contains Y" — possession.
            // Confidence 0.55.
            FactPattern {
                regex: Regex::new(
                    r"(?P<subject>\w+) (?P<predicate>has|owns|contains) (?P<object>[\w]+(?:[ \-]\w+)*)",
                )
                .expect("hand-validated literal `has/owns/contains`"),
                confidence: 0.55,
            },
            // "X uses/imports/calls Y" — code-flavoured relations.
            // Allows dotted identifiers like `foo.bar.baz`.
            // Confidence 0.70.
            FactPattern {
                regex: Regex::new(
                    r"(?P<subject>\w+(?:\.\w+)*) (?P<predicate>uses|imports|calls) (?P<object>\w+(?:\.\w+)*)",
                )
                .expect("hand-validated literal `uses/imports/calls`"),
                confidence: 0.70,
            },
            // "decided to <verb>" — team decisions (JS pattern at line 105).
            // Subject is the literal "team"; predicate "decided_to";
            // object captured as the trailing clause (1..=4 words).
            // Confidence 0.85.
            FactPattern {
                regex: Regex::new(
                    r"(?P<subject>team|we) (?P<predicate>decided to) (?P<object>\w+(?:[ \-]\w+){0,4})",
                )
                .expect("hand-validated literal `decided to`"),
                confidence: 0.85,
            },
            // "<Key>: <value>" — generic key/value declaration with a
            // capitalised single-token key. Mirrors JS pattern at L95.
            // Confidence 0.60.
            FactPattern {
                regex: Regex::new(
                    r"(?m)^(?P<subject>[A-Z][A-Za-z0-9_\-]{1,30})(?P<predicate>:)\s+(?P<object>\S[^\r\n]{1,200})$",
                )
                .expect("hand-validated literal `Key: value`"),
                confidence: 0.60,
            },
        ];
        Self { patterns }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_extractor_constructs_with_patterns() {
        let fx = FactExtractor::default();
        assert!(
            !fx.patterns.is_empty(),
            "default extractor must ship with patterns"
        );
        assert!(
            fx.patterns.len() >= 3,
            "spec calls for 3-5 default patterns, got {}",
            fx.patterns.len()
        );
        // Every default pattern must satisfy the named-group contract.
        for p in &fx.patterns {
            let names: Vec<Option<&str>> = p.regex.capture_names().collect();
            for required in REQUIRED_GROUPS {
                assert!(
                    names.contains(&Some(required)),
                    "default pattern `{}` missing group `{required}`",
                    p.regex.as_str()
                );
            }
        }
    }

    #[test]
    fn extract_finds_facts_in_simple_prose() {
        let fx = FactExtractor::default();
        let facts = fx.extract("Rust is a language");
        assert!(
            facts
                .iter()
                .any(|f| f.subject == "Rust" && f.predicate == "is" && f.object == "language"),
            "expected (Rust, is, language), got {facts:?}"
        );
    }

    #[test]
    fn extract_returns_empty_for_no_match() {
        let fx = FactExtractor::default();
        // Whitespace input -> empty.
        assert!(fx.extract("   \n\t  ").is_empty());
        // Empty input -> empty.
        assert!(fx.extract("").is_empty());
        // Non-matching prose -> empty.
        assert!(
            fx.extract("zzz qqq xxx").is_empty(),
            "no default pattern should match meaningless tokens"
        );
    }

    #[test]
    fn extract_with_dedup_removes_duplicates_across_patterns() {
        // Construct two patterns that will both match the same triple.
        let p1 = FactPattern {
            regex: Regex::new(r"(?P<subject>Alpha) (?P<predicate>uses) (?P<object>Beta)").unwrap(),
            confidence: 0.9,
        };
        let p2 = FactPattern {
            regex: Regex::new(r"(?P<subject>\w+) (?P<predicate>uses) (?P<object>\w+)").unwrap(),
            confidence: 0.5,
        };
        let fx = FactExtractor::with_patterns(vec![p1, p2]);
        let raw = fx.extract("Alpha uses Beta");
        assert_eq!(raw.len(), 2, "raw extract should yield two duplicates");
        let deduped = fx.extract_with_dedup("Alpha uses Beta");
        assert_eq!(deduped.len(), 1, "dedup should collapse to one");
        // First occurrence wins: confidence comes from p1 (0.9).
        assert!((deduped[0].confidence - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn extract_preserves_confidence_from_matching_pattern() {
        let pattern = FactPattern {
            regex: Regex::new(r"(?P<subject>foo) (?P<predicate>bars) (?P<object>baz)").unwrap(),
            confidence: 0.42,
        };
        let fx = FactExtractor::with_patterns(vec![pattern]);
        let facts = fx.extract("foo bars baz");
        assert_eq!(facts.len(), 1);
        assert!(
            (facts[0].confidence - 0.42).abs() < f32::EPSILON,
            "confidence should be carried over from the matching pattern, got {}",
            facts[0].confidence
        );
    }

    #[test]
    fn extract_multiple_facts_in_one_text() {
        let fx = FactExtractor::default();
        let text = "Rust is a language\nfoo.bar uses baz.qux";
        let facts = fx.extract(text);
        let has_rust = facts
            .iter()
            .any(|f| f.subject == "Rust" && f.object == "language");
        let has_uses = facts
            .iter()
            .any(|f| f.subject == "foo.bar" && f.predicate == "uses" && f.object == "baz.qux");
        assert!(has_rust, "missing Rust fact in {facts:?}");
        assert!(has_uses, "missing dotted-uses fact in {facts:?}");
        assert!(facts.len() >= 2, "expected ≥2 facts, got {}", facts.len());
    }

    #[test]
    fn new_validated_rejects_pattern_missing_subject_group() {
        // Predicate + object only.
        let bad = FactPattern {
            regex: Regex::new(r"(?P<predicate>uses) (?P<object>\w+)").unwrap(),
            confidence: 0.5,
        };
        let err = FactExtractor::new_validated(vec![bad]).unwrap_err();
        match err {
            FactExtractorError::MissingNamedGroup { group, .. } => {
                assert_eq!(group, "subject");
            }
        }
    }

    #[test]
    fn new_validated_rejects_pattern_missing_predicate_group() {
        let bad = FactPattern {
            regex: Regex::new(r"(?P<subject>\w+) is (?P<object>\w+)").unwrap(),
            confidence: 0.5,
        };
        let err = FactExtractor::new_validated(vec![bad]).unwrap_err();
        match err {
            FactExtractorError::MissingNamedGroup { group, .. } => {
                assert_eq!(group, "predicate");
            }
        }
    }

    #[test]
    fn new_validated_rejects_pattern_missing_object_group() {
        let bad = FactPattern {
            regex: Regex::new(r"(?P<subject>\w+) (?P<predicate>is) here").unwrap(),
            confidence: 0.5,
        };
        let err = FactExtractor::new_validated(vec![bad]).unwrap_err();
        match err {
            FactExtractorError::MissingNamedGroup { group, .. } => {
                assert_eq!(group, "object");
            }
        }
    }

    // -- W5: fact extractor → KG ingest pipeline ----------------------------

    #[test]
    fn ingest_facts_to_kg_creates_nodes_for_extracted_facts() {
        use crate::kg::{find_nodes_by_name, init_kg};

        let conn = Connection::open_in_memory().unwrap();
        init_kg(&conn).unwrap();

        // "Rust is a language" matches the default copular pattern →
        // (Rust, is, language).
        let n = ingest_facts_to_kg(&conn, "Rust is a language").unwrap();
        assert_eq!(n, 1, "exactly one fact should be ingested");

        let rust = find_nodes_by_name(&conn, "Rust", 10).unwrap();
        assert!(
            rust.iter().any(|node| node.name == "Rust"),
            "subject node `Rust` should exist in the KG, got {rust:?}"
        );
        let language = find_nodes_by_name(&conn, "language", 10).unwrap();
        assert!(
            language.iter().any(|node| node.name == "language"),
            "object node `language` should exist in the KG, got {language:?}"
        );
    }

    #[test]
    fn ingest_facts_to_kg_links_subject_to_object() {
        use crate::kg::{edges_from, find_nodes_by_name, init_kg};

        let conn = Connection::open_in_memory().unwrap();
        init_kg(&conn).unwrap();
        ingest_facts_to_kg(&conn, "foo.bar uses baz.qux").unwrap();

        let subj = &find_nodes_by_name(&conn, "foo.bar", 1).unwrap()[0];
        let edges = edges_from(&conn, subj.id).unwrap();
        assert_eq!(edges.len(), 1, "subject should have one outgoing edge");
        // The predicate ("uses") round-trips through the edge kind.
        assert_eq!(edges[0].kind.as_str(), "uses");
    }

    #[test]
    fn ingest_facts_to_kg_empty_transcript_is_noop() {
        use crate::kg::init_kg;

        let conn = Connection::open_in_memory().unwrap();
        init_kg(&conn).unwrap();
        assert_eq!(ingest_facts_to_kg(&conn, "   \n  ").unwrap(), 0);
        assert_eq!(ingest_facts_to_kg(&conn, "zzz qqq xxx").unwrap(), 0);
    }

    #[test]
    fn new_validated_accepts_pattern_with_all_three_groups() {
        let good = FactPattern {
            regex: Regex::new(r"(?P<subject>\w+) (?P<predicate>\w+) (?P<object>\w+)").unwrap(),
            confidence: 0.5,
        };
        let fx = FactExtractor::new_validated(vec![good]).expect("validation should pass");
        let facts = fx.extract("alpha beta gamma");
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].subject, "alpha");
        assert_eq!(facts[0].predicate, "beta");
        assert_eq!(facts[0].object, "gamma");
    }
}
