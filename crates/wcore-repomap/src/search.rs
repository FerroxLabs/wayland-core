//! Bounded hybrid retrieval over the persistent index (F23-06).
//!
//! Two modalities, fused by reciprocal rank, with an exact-search fallback
//! for the queries neither can serve — and provenance and a staleness verdict
//! attached to every hit.
//!
//! ## This mirrors a pattern; it does not take a dependency
//!
//! `wcore-memory`'s `retrieve::search_basic` already fuses an FTS5 BM25 pass
//! with a vector pass by reciprocal rank, and it is exactly the right shape.
//! It is also in another crate, and AGENTS.md declares `wcore-repomap`
//! isolated with **no** internal `wcore-*` dependency. So the shape is
//! mirrored here — BM25 ordered ascending because lower is better in SQLite,
//! joined to the base table on rowid, fused at `k = 60` — and nothing is
//! imported. Memory retrieval and repository retrieval are different
//! subsystems that happen to rank the same way.
//!
//! ## Why every query is bounded
//!
//! An unbounded retrieval over a repository of this size is a denial-of-
//! service surface pointed at the caller's own context window, not just at
//! the host. The caller sets a limit, the store clamps it to
//! [`MAX_LIMIT`], and every SQL statement carries it (T-23B03-05).
//!
//! ## Why an unservable query is not an empty result
//!
//! FTS5 cannot serve a punctuation-only literal or a token its tokenizer
//! discards. Returning `[]` for those says "this repository does not contain
//! it", which is a different and false claim. Such a query falls through to
//! an exact substring search over the stored text and the outcome records
//! that it did, so the caller can tell "no matches" from "asked a question
//! the index cannot answer lexically".
//!
//! ## Semantic retrieval
//!
//! F23-06 marks the semantic / dense-vector layer OPTIONAL. It is **not
//! built**. [`semantic_status`] says so, and every [`SearchOutcome`] carries
//! that string, because a product that silently degrades to lexical-only
//! while a caller believes it is getting semantic recall is worse than one
//! that says what it does not have.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use rusqlite::{OptionalExtension, params};

use crate::store::{IndexStore, hash_file_on_disk};
use crate::types::RepoMapError;

/// Hard ceiling the store enforces on any caller-supplied limit.
pub const MAX_LIMIT: usize = 500;

/// Reciprocal-rank-fusion constant. 60 is the canonical value and the one
/// `wcore-memory` already uses, so the two subsystems rank comparably.
const RRF_K: f64 = 60.0;

/// Which pass produced a hit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modality {
    /// BM25 over the full-text index.
    Lexical,
    /// Exact or prefix match against a recorded symbol name.
    Symbol,
    /// Exact substring scan, used only when neither indexed pass could serve
    /// the query.
    ExactFallback,
}

impl Modality {
    /// Stable lowercase token, used in provenance output a script may parse.
    pub fn as_str(self) -> &'static str {
        match self {
            Modality::Lexical => "lexical",
            Modality::Symbol => "symbol",
            Modality::ExactFallback => "exact-fallback",
        }
    }
}

/// One modality's contribution to a fused hit: which pass, and at what rank
/// inside that pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModalityRank {
    /// The pass.
    pub modality: Modality,
    /// Zero-based rank the hit held inside that pass.
    pub rank: usize,
}

/// Whether a hit still describes the working tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Staleness {
    /// The indexed content hash no longer matches the file's bytes on disk.
    pub content_stale: bool,
    /// The file the record describes is no longer present on disk.
    pub missing_on_disk: bool,
    /// The store's recorded scope identity no longer matches the working
    /// tree — a different HEAD, branch or worktree.
    pub scope_drifted: bool,
}

impl Staleness {
    /// True when the hit is current in every respect.
    pub fn is_fresh(self) -> bool {
        !self.content_stale && !self.missing_on_disk && !self.scope_drifted
    }
}

/// One retrieval hit, carrying everything needed to attribute it.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Hit {
    /// Canonical `/`-separated path relative to the indexed root.
    pub path: String,
    /// 1-based line the match was located at, or `1` when no more precise
    /// line could be determined.
    pub line: usize,
    /// Every modality that selected this hit, with its rank inside that pass.
    pub modalities: Vec<ModalityRank>,
    /// Zero-based rank in the fused, returned ordering.
    pub rank: usize,
    /// Reciprocal-rank-fusion score. Higher is better.
    pub fused_score: f64,
    /// The scope identity the index was built against, carried on the hit so
    /// retrieved content is attributable rather than anonymous context
    /// (T-23B03-02).
    pub scope: String,
    /// Freshness verdict (T-23B03-03).
    pub staleness: Staleness,
    /// The matched line's text, trimmed and truncated on a char boundary.
    pub snippet: String,
}

/// A bounded retrieval request.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    /// Raw query text as the operator typed it.
    pub text: String,
    /// Maximum hits to return. Clamped to `1..=`[`MAX_LIMIT`].
    pub limit: usize,
}

impl SearchQuery {
    /// A query for `text` with the default limit of 20.
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            limit: 20,
        }
    }

    /// Set the limit.
    #[must_use]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = limit;
        self
    }
}

/// The result of one retrieval, including what the index could and could not
/// do with the query.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SearchOutcome {
    /// Fused hits, best first, never longer than the clamped limit.
    pub hits: Vec<Hit>,
    /// The query could not be served by the indexed passes and was answered
    /// by an exact substring scan.
    pub used_fallback: bool,
    /// Either indexed pass returned at least one row.
    pub servable_by_index: bool,
    /// Scope identity recorded in the store.
    pub scope: String,
    /// The store's scope identity no longer matches the working tree.
    pub scope_drifted: bool,
    /// Readiness truth for the optional semantic layer. See
    /// [`semantic_status`].
    pub semantic: &'static str,
    /// Wall time this retrieval took, in microseconds.
    pub elapsed_micros: u128,
}

/// Readiness truth for the OPTIONAL semantic / dense-vector layer.
///
/// F23-06 marks that layer optional and it is not built in this crate. This
/// function is the product saying so out loud rather than degrading to
/// lexical-only in silence.
pub fn semantic_status() -> &'static str {
    "unavailable: dense/semantic retrieval is not built in wcore-repomap; \
     lexical BM25 + symbol retrieval only"
}

/// Run one bounded hybrid retrieval against `store`.
///
/// # Errors
///
/// [`RepoMapError::Store`] on any database failure. A query FTS5 refuses to
/// parse is **not** an error: it is an unservable query, and it falls through
/// to the exact-search path with `used_fallback` set.
pub fn search(store: &IndexStore, query: &SearchQuery) -> Result<SearchOutcome, RepoMapError> {
    let started = Instant::now();
    let limit = query.limit.clamp(1, MAX_LIMIT);
    let conn = store.connection();
    let store_path = store.store_path().to_path_buf();
    let err = |e: rusqlite::Error| RepoMapError::Store {
        path: store_path.clone(),
        message: e.to_string(),
    };

    let scope = store
        .recorded_scope()?
        .map(|s| s.fingerprint())
        .unwrap_or_else(|| "-".to_string());
    let scope_drifted = store.scope_drifted()?;

    // ── Pass 1: BM25 over the full-text index ────────────────────────────
    // Ordered ASC because a lower bm25() is a better match in SQLite. The
    // MATCH argument is a bound parameter built by `fts5_query`, which strips
    // every FTS5 operator character — an operator query string reaching the
    // engine unescaped is the injection surface here (T-23B03-04).
    let mut lexical: Vec<(i64, String)> = Vec::new();
    if let Some(match_expr) = fts5_query(&query.text) {
        let mut stmt = conn
            .prepare(
                "SELECT files.id, files.path, bm25(files_fts) AS score
                 FROM files_fts JOIN files ON files.id = files_fts.rowid
                 WHERE files_fts MATCH ?1
                 ORDER BY score ASC
                 LIMIT ?2",
            )
            .map_err(err)?;
        // A MATCH the engine rejects is an unservable query, not a failure.
        if let Ok(rows) = stmt.query_map(params![match_expr, limit as i64], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        }) {
            for row in rows.flatten() {
                lexical.push(row);
            }
        }
    }

    // ── Pass 2: exact-then-prefix symbol lookup ──────────────────────────
    let symbol_term = query.text.trim();
    let mut symbolic: Vec<(i64, String, usize, String)> = Vec::new();
    if !symbol_term.is_empty() {
        let prefix = format!("{}%", escape_like(symbol_term));
        let mut stmt = conn
            .prepare(
                "SELECT files.id, files.path, symbols.line, symbols.name
                 FROM symbols JOIN files ON files.id = symbols.file_id
                 WHERE symbols.name = ?1 OR symbols.name LIKE ?2 ESCAPE '\\'
                 ORDER BY (symbols.name = ?1) DESC, LENGTH(symbols.name) ASC, files.path ASC
                 LIMIT ?3",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(params![symbol_term, prefix, limit as i64], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, i64>(2)? as usize,
                    r.get::<_, String>(3)?,
                ))
            })
            .map_err(err)?;
        for row in rows {
            symbolic.push(row.map_err(err)?);
        }
    }

    let servable_by_index = !lexical.is_empty() || !symbolic.is_empty();

    // ── Fallback: exact substring over the stored text ───────────────────
    // `instr` takes the needle as a BOUND PARAMETER and has no pattern
    // metacharacters, so a punctuation-heavy literal is searched literally —
    // which is precisely the class FTS5 cannot serve.
    let mut fallback: Vec<(i64, String)> = Vec::new();
    if !servable_by_index && !symbol_term.is_empty() {
        let mut stmt = conn
            .prepare(
                "SELECT files.id, files.path
                 FROM files_fts JOIN files ON files.id = files_fts.rowid
                 WHERE instr(files_fts.content, ?1) > 0
                 ORDER BY files.path ASC
                 LIMIT ?2",
            )
            .map_err(err)?;
        let rows = stmt
            .query_map(params![symbol_term, limit as i64], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })
            .map_err(err)?;
        for row in rows {
            fallback.push(row.map_err(err)?);
        }
    }
    let used_fallback = !servable_by_index && !fallback.is_empty();

    // ── Reciprocal-rank fusion ───────────────────────────────────────────
    let mut scores: HashMap<i64, f64> = HashMap::new();
    let mut provenance: HashMap<i64, Vec<ModalityRank>> = HashMap::new();
    let mut paths: HashMap<i64, String> = HashMap::new();
    let mut symbol_lines: HashMap<i64, (usize, String)> = HashMap::new();

    for (rank, (id, path)) in lexical.iter().enumerate() {
        *scores.entry(*id).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
        provenance.entry(*id).or_default().push(ModalityRank {
            modality: Modality::Lexical,
            rank,
        });
        paths.entry(*id).or_insert_with(|| path.clone());
    }
    for (rank, (id, path, line, name)) in symbolic.iter().enumerate() {
        *scores.entry(*id).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
        provenance.entry(*id).or_default().push(ModalityRank {
            modality: Modality::Symbol,
            rank,
        });
        paths.entry(*id).or_insert_with(|| path.clone());
        symbol_lines.entry(*id).or_insert((*line, name.clone()));
    }
    for (rank, (id, path)) in fallback.iter().enumerate() {
        *scores.entry(*id).or_insert(0.0) += 1.0 / (RRF_K + rank as f64 + 1.0);
        provenance.entry(*id).or_default().push(ModalityRank {
            modality: Modality::ExactFallback,
            rank,
        });
        paths.entry(*id).or_insert_with(|| path.clone());
    }

    let mut ordered: Vec<(i64, f64)> = scores.into_iter().collect();
    // Ties broken by path so the ordering is deterministic across platforms.
    ordered.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| paths.get(&a.0).cmp(&paths.get(&b.0)))
    });
    ordered.truncate(limit);

    // ── Provenance + staleness on every surviving hit ────────────────────
    let root = store.root().to_path_buf();
    let mut hits = Vec::with_capacity(ordered.len());
    for (rank, (id, score)) in ordered.into_iter().enumerate() {
        let path = paths.get(&id).cloned().unwrap_or_default();
        let content: Option<String> = conn
            .query_row(
                "SELECT content FROM files_fts WHERE rowid = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;
        let recorded_hash: Option<String> = conn
            .query_row(
                "SELECT content_hash FROM files WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .optional()
            .map_err(err)?;

        let (line, snippet) = match symbol_lines.get(&id) {
            Some((line, name)) => (*line, name.clone()),
            None => locate(content.as_deref(), &query.text),
        };

        let abs = root.join(path.replace('/', std::path::MAIN_SEPARATOR_STR));
        let staleness = verdict(&abs, recorded_hash.as_deref(), scope_drifted);

        hits.push(Hit {
            path,
            line,
            modalities: provenance.remove(&id).unwrap_or_default(),
            rank,
            fused_score: score,
            scope: scope.clone(),
            staleness,
            snippet,
        });
    }

    Ok(SearchOutcome {
        hits,
        used_fallback,
        servable_by_index,
        scope,
        scope_drifted,
        semantic: semantic_status(),
        elapsed_micros: started.elapsed().as_micros(),
    })
}

/// Compare a record against the file on disk.
///
/// A file that cannot be read is reported `missing_on_disk` rather than
/// silently treated as fresh — an unreadable file is exactly the case where a
/// caller must not act on the indexed copy without knowing.
fn verdict(abs: &Path, recorded_hash: Option<&str>, scope_drifted: bool) -> Staleness {
    match (recorded_hash, hash_file_on_disk(abs)) {
        (Some(recorded), Ok(current)) => Staleness {
            content_stale: recorded != current,
            missing_on_disk: false,
            scope_drifted,
        },
        (_, Err(_)) => Staleness {
            content_stale: true,
            missing_on_disk: true,
            scope_drifted,
        },
        (None, Ok(_)) => Staleness {
            content_stale: true,
            missing_on_disk: false,
            scope_drifted,
        },
    }
}

/// Find the first line of `content` containing any query token, returning its
/// 1-based number and a trimmed snippet.
fn locate(content: Option<&str>, query: &str) -> (usize, String) {
    let Some(content) = content else {
        return (1, String::new());
    };
    let needles: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_lowercase())
        .filter(|t| !t.is_empty())
        .collect();
    for (index, line) in content.lines().enumerate() {
        let lowered = line.to_lowercase();
        if needles.iter().any(|n| lowered.contains(n)) {
            return (index + 1, truncate_on_char_boundary(line.trim(), 200));
        }
    }
    let first = content.lines().next().unwrap_or("").trim();
    (1, truncate_on_char_boundary(first, 200))
}

fn truncate_on_char_boundary(s: &str, max: usize) -> String {
    let mut end = s.len().min(max);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Build a safe FTS5 MATCH expression from free text.
///
/// Every FTS5 operator character is stripped, each surviving token is quoted,
/// and tokens are OR-ed. Returns `None` when nothing usable survives — which
/// is the signal that the query is unservable by full text and must fall
/// through to exact search rather than return an empty result.
fn fts5_query(text: &str) -> Option<String> {
    let tokens: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty() && t.chars().any(|c| c.is_alphanumeric()))
        .map(|t| format!("\"{t}\""))
        .collect();
    if tokens.is_empty() {
        return None;
    }
    Some(tokens.join(" OR "))
}

/// Escape the SQL `LIKE` metacharacters, for use with `ESCAPE '\'`.
fn escape_like(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c == '%' || c == '_' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fts5_query_strips_operators_and_quotes_tokens() {
        assert_eq!(
            fts5_query("hello world"),
            Some("\"hello\" OR \"world\"".into())
        );
        // An FTS5 operator reaching the engine unescaped is the injection
        // surface; `*`, `"`, `:`, `^`, `(`/`)`, `-` and `AND`-as-syntax must
        // all lose their meaning.
        assert_eq!(
            fts5_query("foo* OR \"bar\" NEAR(baz)"),
            Some("\"foo\" OR \"OR\" OR \"bar\" OR \"NEAR\" OR \"baz\"".into())
        );
    }

    #[test]
    fn fts5_query_returns_none_for_a_query_full_text_cannot_serve() {
        // This is the load-bearing case: `None` is what routes the query to
        // exact search instead of returning `[]` and implying absence.
        assert_eq!(fts5_query("=>"), None);
        assert_eq!(fts5_query("   "), None);
        assert_eq!(fts5_query("!!!"), None);
    }

    #[test]
    fn escape_like_neutralises_wildcards() {
        assert_eq!(escape_like("a%b_c\\d"), "a\\%b\\_c\\\\d");
    }

    #[test]
    fn locate_finds_the_matching_line_and_falls_back_to_the_first() {
        let content = "alpha\nbeta gamma\ndelta\n";
        assert_eq!(locate(Some(content), "GAMMA").0, 2);
        assert_eq!(locate(Some(content), "nothing-here"), (1, "alpha".into()));
        assert_eq!(locate(None, "x"), (1, String::new()));
    }

    #[test]
    fn truncate_respects_char_boundaries() {
        let s = "ünïcode";
        let t = truncate_on_char_boundary(s, 3);
        assert!(s.starts_with(&t));
        assert!(t.len() <= 3);
    }

    #[test]
    fn staleness_is_fresh_only_when_every_axis_is_clean() {
        let fresh = Staleness {
            content_stale: false,
            missing_on_disk: false,
            scope_drifted: false,
        };
        assert!(fresh.is_fresh());
        assert!(
            !Staleness {
                scope_drifted: true,
                ..fresh
            }
            .is_fresh()
        );
    }
}
