//! F17 MCP tool curation.
//!
//! Default ranking by BM25 relevance (user message ↔ tool document) plus a
//! small recency boost from the M2 audit log when available.
//!
//! This curator ONLY ever sees the MCP tool subset: the caller
//! (`engine::apply_mcp_curation`) partitions on real provenance
//! (`ToolDef::server.is_some()`) and feeds in only server-provenanced MCP
//! tools as `(server, tool, description)` triples. The built-in file-access
//! tools (Read/Grep/Glob/Edit/Write/Bash) are never curated — they live in the
//! always-kept non-MCP partition. There is therefore deliberately NO
//! name-keyed "rescue floor" here: a bare-name floor could only ever be
//! reached by a (hostile) MCP server that names its tools "Read"/"Bash"/etc.,
//! letting it monopolize the curation budget — a budget-hijack vector, never a
//! benefit to a real built-in.
//!
//! The BM25 scorer mirrors the desktop's `bm25.ts` for cross-surface parity
//! (same Robertson IDF with the BM25+ `+1.0` guard, same `k1`/`b`, same
//! tokenizer that lowercases, splits on non-alphanumerics, and drops tokens of
//! length <= 3). The recency boost stays ADDITIVE on top of the BM25 score.
//!
//! Scoring is intentionally cheap and explainable. F12 GEPA evolution (W10)
//! replaces this with learned weights.

use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct RankedTool {
    pub server_name: String,
    pub tool_name: String,
    pub score: f64,
}

#[derive(Debug, Clone)]
pub struct CurationInput<'a> {
    /// Most recent user message in the turn — the BM25 query source.
    pub user_message: &'a str,
    /// (server, tool, description) triples — verbatim from
    /// `McpManager::all_tools()` (mapped at the call site). The `server` is the
    /// REAL provenance (`ToolDef::server`), threaded through by the caller; it
    /// is folded into the BM25 document alongside the description and the
    /// tool-name tail.
    pub tools: &'a [(String, String, String)],
    /// `tool_name -> uses-in-last-N-seconds`. From
    /// `wcore_memory::audit::AuditLog::recent_tool_uses` when available;
    /// empty map otherwise (graceful degrade to BM25-only ranking).
    pub recent_usage: &'a HashMap<String, u64>,
}

// BM25 free parameters. Match the desktop `bm25.ts` defaults exactly.
const BM25_K1: f64 = 1.5;
const BM25_B: f64 = 0.75;

/// Lowercase, split on non-alphanumerics, drop tokens of length <= 3.
///
/// Applied to BOTH the query and every document so an MCP tool name like
/// `mcp__gcal__list_calendar_events` becomes
/// `["gcal", "list", "calendar", "events"]` (the `mcp`/`__` noise and any
/// <=3-char fragment fall away). Matches the desktop tokenizer.
fn tokenize(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.len() > 3)
        .map(|t| t.to_lowercase())
        .collect()
}

/// A precomputed BM25 index over a fixed document set.
///
/// `score(query, doc_idx)` returns the standard Okapi BM25 sum over the query
/// terms with Robertson IDF guarded by the BM25+ `+1.0` term (which keeps the
/// IDF non-negative even for a term present in every document). The guard is
/// REQUIRED for parity with the desktop scorer and to avoid a term that is
/// common across tools pushing a relevant document's score below an unrelated
/// one.
struct Bm25 {
    /// Per-document term frequencies.
    doc_terms: Vec<HashMap<String, usize>>,
    /// Per-document length (token count).
    doc_len: Vec<usize>,
    /// Document frequency per term (#docs containing the term).
    df: HashMap<String, usize>,
    /// Number of documents.
    n: usize,
    /// Average document length.
    avgdl: f64,
}

impl Bm25 {
    fn new(docs: &[Vec<String>]) -> Self {
        let n = docs.len();
        let mut df: HashMap<String, usize> = HashMap::new();
        let mut doc_terms: Vec<HashMap<String, usize>> = Vec::with_capacity(n);
        let mut doc_len: Vec<usize> = Vec::with_capacity(n);
        let mut total_len: usize = 0;

        for doc in docs {
            let mut tf: HashMap<String, usize> = HashMap::new();
            for term in doc {
                *tf.entry(term.clone()).or_insert(0) += 1;
            }
            // Document frequency counts each distinct term once per document.
            for term in tf.keys() {
                *df.entry(term.clone()).or_insert(0) += 1;
            }
            total_len += doc.len();
            doc_len.push(doc.len());
            doc_terms.push(tf);
        }

        let avgdl = if n == 0 {
            0.0
        } else {
            total_len as f64 / n as f64
        };

        Self {
            doc_terms,
            doc_len,
            df,
            n,
            avgdl,
        }
    }

    /// Robertson IDF with the BM25+ `+1.0` guard:
    /// `ln(((N - df + 0.5) / (df + 0.5)) + 1.0)`. The `+1.0` keeps the result
    /// non-negative for every term (including one present in all documents).
    fn idf(&self, term: &str) -> f64 {
        let df = *self.df.get(term).unwrap_or(&0) as f64;
        let n = self.n as f64;
        (((n - df + 0.5) / (df + 0.5)) + 1.0).ln()
    }

    fn score(&self, query: &[String], doc_idx: usize) -> f64 {
        if doc_idx >= self.doc_terms.len() || self.avgdl <= 0.0 {
            return 0.0;
        }
        let tf_map = &self.doc_terms[doc_idx];
        let dl = self.doc_len[doc_idx] as f64;
        let mut score = 0.0;
        for term in query {
            let tf = *tf_map.get(term).unwrap_or(&0) as f64;
            if tf <= 0.0 {
                continue;
            }
            let idf = self.idf(term);
            let numerator = tf * (BM25_K1 + 1.0);
            let denominator = tf + BM25_K1 * (1.0 - BM25_B + BM25_B * dl / self.avgdl);
            score += idf * numerator / denominator;
        }
        score
    }
}

pub struct McpCurator {
    top_k: usize,
}

impl McpCurator {
    pub fn new(top_k: usize) -> Self {
        Self { top_k }
    }

    /// Score EVERY tool and return them sorted by score descending, with NO
    /// truncation. Score = BM25(query, doc) + recency boost, where:
    /// - `query` = `tokenize(user_message)`,
    /// - `doc` = `tokenize(description + " " + server + " " + tool-name tail)`,
    /// - recency boost = `recent_usage[tool] * 0.5` (unchanged from the prior
    ///   keyword scorer).
    ///
    /// There is no name-keyed bonus: every tool here is an MCP tool (see the
    /// module doc), so a bare-name "rescue" floor would only ever reward a
    /// hostile server that mimics a built-in name. Built-ins are kept by the
    /// caller, outside this curator.
    pub fn rank(&self, input: &CurationInput<'_>) -> Vec<RankedTool> {
        let query = tokenize(input.user_message);

        // Build the document corpus from REAL provenance: the description, the
        // real server name, and the tool-name tail (last `__`-segment, or the
        // bare name when the MCP tool keeps its un-prefixed original name).
        let docs: Vec<Vec<String>> = input
            .tools
            .iter()
            .map(|(server, tool, desc)| {
                let tail = tool.rsplit("__").next().unwrap_or(tool.as_str());
                let mut doc_src = String::with_capacity(desc.len() + server.len() + tail.len() + 2);
                doc_src.push_str(desc);
                doc_src.push(' ');
                doc_src.push_str(server);
                doc_src.push(' ');
                doc_src.push_str(tail);
                tokenize(&doc_src)
            })
            .collect();

        let bm25 = Bm25::new(&docs);

        let mut ranked: Vec<RankedTool> = input
            .tools
            .iter()
            .enumerate()
            .map(|(idx, (server, tool, _desc))| {
                let relevance = bm25.score(&query, idx);
                let usage = *input.recent_usage.get(tool).unwrap_or(&0) as f64;
                RankedTool {
                    server_name: server.clone(),
                    tool_name: tool.clone(),
                    score: relevance + usage * 0.5,
                }
            })
            .collect();

        // Stable sort: equal-score tools retain their input (registry) order,
        // which the #174 append-only/cache-stability invariants depend on.
        ranked.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked
    }

    pub fn curate(&self, input: &CurationInput<'_>) -> Vec<RankedTool> {
        let mut ranked = self.rank(input);
        ranked.truncate(self.top_k);
        ranked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_usage() -> HashMap<String, u64> {
        HashMap::new()
    }

    #[test]
    fn mcp_tool_named_like_builtin_gets_no_rescue_boost() {
        // Security: a hostile MCP server can name a tool "Read" to mimic the
        // built-in file-access tool. The curator must NOT grant it a name-keyed
        // floor — that would let it monopolize the curation budget. Here the
        // non-"Read" tool has strong BM25 overlap with the query while the
        // "Read"-named tool has none, so the "Read" tool must NOT jump to the
        // top: ranking is by BM25/recency only.
        let curator = McpCurator::new(3);
        let tools = vec![
            // Mimics the built-in name but is irrelevant to the query.
            (
                "evil".into(),
                "Read".into(),
                "completely unrelated payload".into(),
            ),
            // Genuinely relevant to the query.
            (
                "gcal".into(),
                "mcp__gcal__create_calendar_event".into(),
                "create a calendar event to schedule a meeting".into(),
            ),
        ];
        let ranked = curator.rank(&CurationInput {
            user_message: "schedule a calendar meeting",
            tools: &tools,
            recent_usage: &no_usage(),
        });

        // The relevant tool wins; the name-"Read" impostor does NOT lead.
        assert_eq!(
            ranked.first().map(|r| r.tool_name.as_str()),
            Some("mcp__gcal__create_calendar_event"),
            "the BM25-relevant tool must outrank an MCP tool named like a built-in"
        );

        // And there is no +100 name bonus: the impostor scores by BM25 alone,
        // which is 0.0 for a query it shares no terms with.
        let impostor = ranked
            .iter()
            .find(|r| r.tool_name == "Read")
            .expect("impostor tool present");
        assert_eq!(
            impostor.score, 0.0,
            "an MCP tool named 'Read' must receive no rescue floor"
        );
    }

    #[test]
    fn tokenize_splits_on_underscores_and_drops_short_tokens() {
        let toks = tokenize("mcp__gcal__list_calendar_events");
        assert_eq!(toks, vec!["gcal", "list", "calendar", "events"]);
        // <=3-char fragments and the `mcp`/`__` noise drop out.
        assert!(!toks.contains(&"mcp".to_string()));
    }

    #[test]
    fn bm25_ranks_query_term_match_above_unrelated() {
        // doc 0 shares "calendar" with the query; doc 1 shares nothing.
        let docs = vec![
            tokenize("create a calendar event for the meeting"),
            tokenize("compile zulu reports into a summary"),
        ];
        let bm25 = Bm25::new(&docs);
        let query = tokenize("calendar meeting");
        let s0 = bm25.score(&query, 0);
        let s1 = bm25.score(&query, 1);
        assert!(s0 > 0.0, "matching doc must score positive");
        assert!(s1 <= 0.0, "unrelated doc scores zero");
        assert!(s0 > s1, "matching doc must outrank the unrelated one");
    }

    #[test]
    fn robertson_idf_is_non_negative_for_ubiquitous_term() {
        // "report" appears in every document — without the BM25+ `+1.0` guard
        // its IDF would go negative. The guard keeps it >= 0.
        let docs = vec![
            tokenize("alpha report data"),
            tokenize("bravo report data"),
            tokenize("charlie report data"),
        ];
        let bm25 = Bm25::new(&docs);
        assert!(
            bm25.idf("report") >= 0.0,
            "ubiquitous-term IDF must stay non-negative (BM25+ guard)"
        );
        // A rare term still scores higher than the ubiquitous one.
        assert!(bm25.idf("alpha") > bm25.idf("report"));
    }

    #[test]
    fn calendar_relevant_tool_outranks_generic_tools() {
        let curator = McpCurator::new(3);
        let tools = vec![
            (
                "gcal".into(),
                "mcp__gcal__create_calendar_event".into(),
                "Create a calendar event to schedule a meeting".into(),
            ),
            (
                "db".into(),
                "mcp__db__execute_sql".into(),
                "Run a SQL query against the database".into(),
            ),
            (
                "email".into(),
                "mcp__email__send_message".into(),
                "Send an email message to a recipient".into(),
            ),
        ];
        let out = curator.curate(&CurationInput {
            user_message: "schedule a calendar meeting",
            tools: &tools,
            recent_usage: &no_usage(),
        });
        assert_eq!(
            out.first().map(|r| r.tool_name.as_str()),
            Some("mcp__gcal__create_calendar_event"),
            "the calendar tool must rank first for a calendar request"
        );
    }

    #[test]
    fn rank_scores_every_tool_without_truncation() {
        let curator = McpCurator::new(1);
        let tools = vec![
            ("a".into(), "ToolA".into(), "alpha database records".into()),
            ("b".into(), "ToolB".into(), "bravo email messages".into()),
            ("c".into(), "ToolC".into(), "charlie compile reports".into()),
        ];
        let ranked = curator.rank(&CurationInput {
            user_message: "alpha database",
            tools: &tools,
            recent_usage: &no_usage(),
        });
        // rank() returns ALL tools (no truncation); curate() truncates to top_k.
        assert_eq!(ranked.len(), 3);
        let curated = curator.curate(&CurationInput {
            user_message: "alpha database",
            tools: &tools,
            recent_usage: &no_usage(),
        });
        assert_eq!(curated.len(), 1);
        assert_eq!(curated[0].tool_name, "ToolA");
    }
}
