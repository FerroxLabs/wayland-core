// Cross-project memory federation with BM25 ranking.
//
// Ported from ijfw mcp-server/src/cross-project-search.js. The JS version
// builds a corpus of line-level docs across every registered project and
// ranks with BM25; this Rust port keeps the same scoring shape but is
// scope-aware: the caller selects ScopeMode::Current (default) or
// ScopeMode::All.
//
// Default scope is Current; --scope all is per-session opt-in
// (BATTLE-PLAN-v2 §Pre-flight). The primitive itself accepts ScopeMode as
// a parameter — enforcement of "current vs all" lives at the CLI/agent
// layer.

use std::path::{Path, PathBuf};

// --- Scope -----------------------------------------------------------------

/// Whether a federated search ranges over only the current project or
/// every discovered project's memory db.
///
/// `Default` is `Current` — `--scope all` is per-session opt-in
/// (BATTLE-PLAN-v2 §Pre-flight).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ScopeMode {
    /// Only this project's memory db.
    #[default]
    Current,
    /// Federate across every discovered project's memory db.
    All,
}

/// Wrapper around `ScopeMode` so call sites can spell intent explicitly
/// (`ProjectScope { mode: ScopeMode::All }`) while keeping the bare enum
/// available for cheap argument passing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProjectScope {
    /// "current" — only this project's memory db.
    /// "all" — federate across every discovered project's memory db.
    pub mode: ScopeMode,
}

// --- Discovery -------------------------------------------------------------

/// One row in the cross-project index — a project id plus the absolute
/// path to its memory database file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectIndex {
    pub project_id: String,
    pub memory_db_path: PathBuf,
}

/// Walk one level of subdirectories under `root` looking for
/// `<subdir>/memory.db`. Returns an empty vec if `root` doesn't exist or
/// can't be read. Not recursive.
pub fn discover_projects(root: &Path) -> Vec<ProjectIndex> {
    let entries = match std::fs::read_dir(root) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        // Reject symlinks BEFORE treating the entry as a directory —
        // following a symlink could traverse outside `root` (or into a
        // cycle) and would scan project DBs not owned by this index.
        // `Path::is_dir()` follows symlinks, so the symlink check must
        // come first.
        if path.is_symlink() || !path.is_dir() {
            continue;
        }
        let db = path.join("memory.db");
        if !db.is_file() {
            continue;
        }
        let project_id = match path.file_name().and_then(|s| s.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };
        out.push(ProjectIndex {
            project_id,
            memory_db_path: db,
        });
    }
    out
}

// --- Tokenization ----------------------------------------------------------

/// Tokenize text: split on whitespace, lowercase, strip non-alphanumeric.
/// Public so tests (and consumers building their own corpora) can mirror
/// the exact tokenization the BM25 scorer uses.
pub fn tokenize(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(|c| c.to_lowercase())
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

// --- Documents + scoring ---------------------------------------------------

/// A document in the cross-project corpus.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub id: String,
    pub project_id: String,
    pub content: String,
}

/// A single BM25-ranked hit.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub doc_id: String,
    pub project_id: String,
    pub score: f32,
}

const BM25_K1: f32 = 1.2;
const BM25_B: f32 = 0.75;
const TOP_K_CAP: usize = 50;

/// Compute BM25 scores over `docs` for `query_tokens`. Returns one score
/// per input doc (parallel to `docs`).
fn bm25_scores(query_tokens: &[String], docs: &[Vec<String>]) -> Vec<f32> {
    let n = docs.len();
    if n == 0 || query_tokens.is_empty() {
        return vec![0.0; n];
    }
    let total_len: usize = docs.iter().map(|d| d.len()).sum();
    let avg_doc_len = if n > 0 {
        total_len as f32 / n as f32
    } else {
        0.0
    };

    // Document-frequency per query term (count of docs containing the term).
    let mut scores = vec![0.0f32; n];
    for term in query_tokens {
        let mut df: usize = 0;
        for d in docs {
            if d.iter().any(|t| t == term) {
                df += 1;
            }
        }
        if df == 0 {
            continue;
        }
        let idf = (((n as f32 - df as f32 + 0.5) / (df as f32 + 0.5)) + 1.0).ln();
        for (i, d) in docs.iter().enumerate() {
            let tf = d.iter().filter(|t| *t == term).count() as f32;
            if tf == 0.0 {
                continue;
            }
            let doc_len = d.len() as f32;
            let denom = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * (doc_len / avg_doc_len));
            scores[i] += idf * (tf * (BM25_K1 + 1.0)) / denom;
        }
    }
    scores
}

/// Rank `docs` against `query`. If `scope == Current`, filter to
/// `d.project_id == current_project` BEFORE scoring; if `All`, score
/// across all docs. Returns up to 50 hits sorted descending by score.
pub fn search(
    query: &str,
    docs: &[Document],
    scope: ScopeMode,
    current_project: &str,
) -> Vec<SearchHit> {
    let query_tokens = tokenize(query);
    if query_tokens.is_empty() {
        return Vec::new();
    }

    // Filter by scope BEFORE scoring so IDF reflects the actual corpus.
    let filtered: Vec<&Document> = match scope {
        ScopeMode::Current => docs
            .iter()
            .filter(|d| d.project_id == current_project)
            .collect(),
        ScopeMode::All => docs.iter().collect(),
    };
    if filtered.is_empty() {
        return Vec::new();
    }

    let tokenized: Vec<Vec<String>> = filtered.iter().map(|d| tokenize(&d.content)).collect();
    let scores = bm25_scores(&query_tokens, &tokenized);

    let mut hits: Vec<SearchHit> = filtered
        .iter()
        .zip(scores.iter())
        .filter(|(_, s)| **s > 0.0)
        .map(|(d, s)| SearchHit {
            doc_id: d.id.clone(),
            project_id: d.project_id.clone(),
            score: *s,
        })
        .collect();

    hits.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    hits.truncate(TOP_K_CAP.min(hits.len()));
    hits
}

// --- Tests -----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn doc(id: &str, project: &str, content: &str) -> Document {
        Document {
            id: id.to_string(),
            project_id: project.to_string(),
            content: content.to_string(),
        }
    }

    #[test]
    fn tokenize_basic_words() {
        let toks = tokenize("Hello, World!");
        assert_eq!(toks, vec!["hello".to_string(), "world".to_string()]);
    }

    #[test]
    fn tokenize_empty_returns_empty() {
        let toks = tokenize("");
        assert!(toks.is_empty());
        let toks2 = tokenize("   \t  \n  ");
        assert!(toks2.is_empty());
    }

    #[test]
    fn discover_projects_returns_empty_for_nonexistent_root() {
        let bogus = std::path::Path::new("/definitely/does/not/exist/xyzzy");
        let projects = discover_projects(bogus);
        assert!(projects.is_empty());
    }

    #[test]
    fn discover_projects_finds_subdir_dbs() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        let p1 = root.join("alpha");
        let p2 = root.join("beta");
        let p3 = root.join("no-db");
        fs::create_dir(&p1).unwrap();
        fs::create_dir(&p2).unwrap();
        fs::create_dir(&p3).unwrap();
        fs::write(p1.join("memory.db"), b"").unwrap();
        fs::write(p2.join("memory.db"), b"").unwrap();
        // p3 intentionally has no memory.db

        let mut found = discover_projects(root);
        found.sort_by(|a, b| a.project_id.cmp(&b.project_id));
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].project_id, "alpha");
        assert_eq!(found[0].memory_db_path, p1.join("memory.db"));
        assert_eq!(found[1].project_id, "beta");
        assert_eq!(found[1].memory_db_path, p2.join("memory.db"));
    }

    #[cfg(unix)]
    #[test]
    fn discover_projects_does_not_follow_symlinks() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        // A real project dir with a memory.db.
        let real = root.join("real");
        fs::create_dir(&real).unwrap();
        fs::write(real.join("memory.db"), b"").unwrap();
        // An "external" project the symlink will point to.
        let external_root = TempDir::new().unwrap();
        let external = external_root.path().join("external");
        fs::create_dir(&external).unwrap();
        fs::write(external.join("memory.db"), b"").unwrap();
        // Symlink inside `root` that points to the external project.
        std::os::unix::fs::symlink(&external, root.join("linked")).unwrap();

        let found = discover_projects(root);
        let ids: Vec<&str> = found.iter().map(|p| p.project_id.as_str()).collect();
        assert!(
            ids.contains(&"real"),
            "real project must be discovered: {ids:?}"
        );
        assert!(
            !ids.contains(&"linked"),
            "symlinked project must NOT be discovered: {ids:?}"
        );
    }

    #[test]
    fn search_current_scope_filters_to_current_project() {
        let docs = vec![
            doc("a1", "alpha", "rust borrow checker tutorial"),
            doc("b1", "beta", "rust borrow checker advanced"),
            doc("a2", "alpha", "completely unrelated text"),
        ];
        let hits = search("borrow checker", &docs, ScopeMode::Current, "alpha");
        // Only docs from alpha can appear.
        for h in &hits {
            assert_eq!(h.project_id, "alpha");
        }
        // a1 matches; a2 does not.
        assert!(hits.iter().any(|h| h.doc_id == "a1"));
        assert!(hits.iter().all(|h| h.doc_id != "b1"));
    }

    #[test]
    fn search_all_scope_includes_all_projects() {
        let docs = vec![
            doc("a1", "alpha", "rust borrow checker tutorial"),
            doc("b1", "beta", "rust borrow checker advanced"),
        ];
        let hits = search("borrow checker", &docs, ScopeMode::All, "alpha");
        let projects: std::collections::HashSet<String> =
            hits.iter().map(|h| h.project_id.clone()).collect();
        assert!(projects.contains("alpha"));
        assert!(projects.contains("beta"));
    }

    #[test]
    fn search_returns_results_sorted_descending_by_score() {
        let docs = vec![
            doc("low", "alpha", "borrow"),
            doc("high", "alpha", "borrow borrow borrow checker"),
            doc("mid", "alpha", "borrow checker"),
        ];
        let hits = search("borrow checker", &docs, ScopeMode::All, "alpha");
        assert!(hits.len() >= 2);
        for w in hits.windows(2) {
            assert!(
                w[0].score >= w[1].score,
                "hits not sorted descending: {:?}",
                hits
            );
        }
    }

    #[test]
    fn search_top_k_capped_at_50() {
        // 60 distinct docs all containing the query term — only 50 should
        // come back.
        let docs: Vec<Document> = (0..60)
            .map(|i| doc(&format!("d{i}"), "alpha", "borrow checker"))
            .collect();
        let hits = search("borrow", &docs, ScopeMode::All, "alpha");
        assert_eq!(hits.len(), 50);
    }

    #[test]
    fn search_empty_query_returns_empty() {
        let docs = vec![doc("a1", "alpha", "anything goes here")];
        let hits = search("", &docs, ScopeMode::All, "alpha");
        assert!(hits.is_empty());
        let hits2 = search("   \t  ", &docs, ScopeMode::All, "alpha");
        assert!(hits2.is_empty());
    }

    #[test]
    fn bm25_higher_for_more_query_term_matches() {
        let docs = vec![
            doc("once", "alpha", "borrow filler filler filler"),
            doc("thrice", "alpha", "borrow borrow borrow filler"),
        ];
        let hits = search("borrow", &docs, ScopeMode::All, "alpha");
        assert_eq!(hits.len(), 2);
        // The doc with more matches must rank first.
        assert_eq!(hits[0].doc_id, "thrice");
        assert_eq!(hits[1].doc_id, "once");
        assert!(hits[0].score > hits[1].score);
    }

    #[test]
    fn bm25_decays_with_doc_length() {
        // Same number of "borrow" hits (1) in both docs, but the second
        // doc is padded with unrelated words. BM25's length normalization
        // should give the shorter doc the higher score.
        let docs = vec![
            doc("short", "alpha", "borrow checker"),
            doc(
                "long",
                "alpha",
                "borrow checker filler filler filler filler filler filler filler filler",
            ),
        ];
        let hits = search("borrow", &docs, ScopeMode::All, "alpha");
        assert_eq!(hits.len(), 2);
        let short = hits.iter().find(|h| h.doc_id == "short").unwrap();
        let long = hits.iter().find(|h| h.doc_id == "long").unwrap();
        assert!(
            short.score > long.score,
            "length normalization broken: short={} long={}",
            short.score,
            long.score
        );
    }

    #[test]
    fn default_scope_mode_is_current() {
        assert_eq!(ScopeMode::default(), ScopeMode::Current);
        let scope = ProjectScope::default();
        assert_eq!(scope.mode, ScopeMode::Current);
    }
}
