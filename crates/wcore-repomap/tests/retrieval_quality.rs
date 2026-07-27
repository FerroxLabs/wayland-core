//! F23-06 — the retrieval-quality gate.
//!
//! A fixed query-to-expected-file corpus with a recorded precision and recall
//! floor, mirroring the shape of the workspace's existing eval acceptance
//! gate: measure first, choose the threshold from the measurement, and record
//! both so a later widening is visible as a change rather than absorbed.
//!
//! ## Why this corpus is the crate's own tree
//!
//! The obvious corpus is the whole workspace, and that IS where the real
//! number comes from — Phase 23B plan 03's live driver measures precision and
//! recall over all 3,601 in-scope files through the shipped binary, and
//! records a materially lower precision@1 there (see
//! `23B-03-LIVE-EVIDENCE.md`). It is the wrong corpus for a *unit gate*,
//! because its ground truth would depend on files in crates this plan does
//! not own: an unrelated change in `wcore-agent` could move a doc-comment
//! above a definition in the BM25 ordering and redden a suite that has
//! nothing to do with it.
//!
//! So the two gates are split deliberately, and both are reported:
//!
//! - **here**: a hermetic corpus of this crate's own 18 in-scope files, whose
//!   ground truth no other lane can move. It catches a *ranking regression*.
//! - **the live driver**: the full workspace, through the shipped binary. It
//!   reports what retrieval quality actually is over a real repository.
//!
//! ## Metrics
//!
//! For each query with expected file set `E`, retrieving the top `K`:
//!
//! - `precision@1` — 1 if the top hit is in `E`, else 0.
//! - `recall@K`    — `|H ∩ E| / |E|`.
//!
//! Both macro-averaged over the corpus. `precision@1` is used rather than
//! `precision@K` because with `|E| = 1` and `K = 10` a perfect ranker scores
//! 0.1 on `precision@K`, which measures the limit, not the ranker.

use std::path::PathBuf;

use wcore_repomap::{IndexOptions, IndexStore, SearchQuery, search};

/// Hits requested per query.
const K: usize = 10;

// ── The floors, and the order they were chosen in ────────────────────────
//
// CHOSEN AFTER MEASURING, and the measured values are recorded beside them.
// Measured on hetzner-dsm at commit e1fae361 over this crate's own tree
// (18 in-scope files, 214 symbols, 13 ms cold build), through the shipped
// binary's `index search`:
//
//     precision@1 = 1.00  (16/16 queries put an expected file first)
//     recall@10   = 1.00  (16/16 queries found every expected file)
//
// The floors sit BELOW the measurement on purpose. A floor pinned at exactly
// 1.00 reddens on a single ordering wobble caused by an unrelated doc edit in
// this crate, which trains a reader to ignore it. One query's worth of slack
// (1/16 = 0.0625) is the margin, so a genuine ranking regression — which
// moves several queries at once — still fails.
//
// If a future change lowers the measurement, the honest response is to record
// the new measurement and investigate, NOT to lower these constants. A
// threshold widened after a failure must be called out as such.
const PRECISION_AT_1_FLOOR: f64 = 0.90;
const RECALL_AT_K_FLOOR: f64 = 0.95;

/// One corpus entry: a query and every file that legitimately answers it.
struct Case {
    query: &'static str,
    expected: &'static [&'static str],
}

/// The fixed corpus.
///
/// Nine symbol-shaped queries (the case symbol lookup must serve) and seven
/// concept-shaped queries in prose (the case only BM25 can serve), so a
/// regression in either modality shows up rather than being masked by the
/// other.
const CORPUS: &[Case] = &[
    // ── symbol-shaped ──
    Case {
        query: "IndexStore",
        expected: &["src/store.rs"],
    },
    Case {
        query: "normalize_rel",
        expected: &["src/scope.rs"],
    },
    Case {
        query: "ScopeIdentity",
        expected: &["src/scope.rs"],
    },
    Case {
        query: "semantic_status",
        expected: &["src/search.rs"],
    },
    Case {
        query: "extract_rust",
        expected: &["src/extractor/rust.rs"],
    },
    Case {
        query: "SymbolKind",
        expected: &["src/types.rs"],
    },
    Case {
        query: "IndexOptions",
        expected: &["src/types.rs"],
    },
    Case {
        query: "RepoMapError",
        expected: &["src/types.rs"],
    },
    Case {
        query: "first_meaningful",
        expected: &["src/lib.rs"],
    },
    // ── concept-shaped ──
    Case {
        query: "reciprocal rank fusion",
        expected: &["src/search.rs"],
    },
    Case {
        query: "content hash invalidation",
        expected: &["src/store.rs"],
    },
    Case {
        query: "worktree identity",
        expected: &["src/scope.rs"],
    },
    Case {
        query: "bm25 full text",
        expected: &["src/search.rs"],
    },
    Case {
        query: "symbol extractor regex",
        expected: &["src/extractor/typescript.rs", "src/extractor/rust.rs"],
    },
    Case {
        query: "walker gitignore hidden",
        expected: &["src/scope.rs", "src/lib.rs"],
    },
    Case {
        query: "staleness verdict provenance",
        expected: &["src/search.rs"],
    },
];

#[test]
fn retrieval_quality_meets_the_recorded_precision_and_recall_floor() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let store_dir = tempfile::tempdir().expect("store tempdir");
    let store_path = store_dir.path().join("quality.db");

    let mut store = IndexStore::open(&store_path, &root).expect("open store");
    let stats = store.refresh(&IndexOptions::default()).expect("refresh");
    assert!(
        stats.scanned >= 15,
        "the corpus must actually be indexed before any quality number means \
         anything — scanned {}: {stats:?}",
        stats.scanned
    );
    assert!(
        store.symbol_count().expect("symbols") > 100,
        "a corpus with no symbols would make every symbol-modality query \
         vacuous"
    );

    let mut precision_sum = 0.0;
    let mut recall_sum = 0.0;
    let mut misses: Vec<String> = Vec::new();

    for case in CORPUS {
        let outcome = search(&store, &SearchQuery::new(case.query).with_limit(K))
            .unwrap_or_else(|e| panic!("search {:?}: {e}", case.query));
        let returned: Vec<&str> = outcome.hits.iter().map(|h| h.path.as_str()).collect();

        let top_hit_expected = returned.first().is_some_and(|p| case.expected.contains(p));
        let found = case
            .expected
            .iter()
            .filter(|e| returned.contains(e))
            .count();

        let precision = f64::from(u8::from(top_hit_expected));
        let recall = found as f64 / case.expected.len() as f64;
        precision_sum += precision;
        recall_sum += recall;

        if precision < 1.0 || recall < 1.0 {
            misses.push(format!(
                "  {:?}: precision@1={precision} recall@{K}={recall:.2} \
                 expected={:?} got={:?}",
                case.query, case.expected, returned
            ));
        }
    }

    let n = CORPUS.len() as f64;
    let precision_at_1 = precision_sum / n;
    let recall_at_k = recall_sum / n;

    // Printed unconditionally so a passing run still publishes the number.
    // A gate that only speaks when it fails cannot show a slow decline.
    println!(
        "F23_QUALITY=corpus queries={} files={} symbols={}",
        CORPUS.len(),
        stats.scanned,
        store.symbol_count().expect("symbols")
    );
    println!(
        "F23_QUALITY=measured precision_at_1={precision_at_1:.4} \
         recall_at_{K}={recall_at_k:.4}"
    );
    println!(
        "F23_QUALITY=floor precision_at_1={PRECISION_AT_1_FLOOR:.4} \
         recall_at_{K}={RECALL_AT_K_FLOOR:.4}"
    );

    assert!(
        precision_at_1 >= PRECISION_AT_1_FLOOR,
        "precision@1 {precision_at_1:.4} fell below the recorded floor \
         {PRECISION_AT_1_FLOOR:.4}. Do NOT lower the floor to make this pass \
         — record the new measurement and investigate.\n{}",
        misses.join("\n")
    );
    assert!(
        recall_at_k >= RECALL_AT_K_FLOOR,
        "recall@{K} {recall_at_k:.4} fell below the recorded floor \
         {RECALL_AT_K_FLOOR:.4}. Do NOT lower the floor to make this pass.\n{}",
        misses.join("\n")
    );
}

#[test]
fn every_hit_carries_provenance_and_a_staleness_verdict() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let store_dir = tempfile::tempdir().expect("store tempdir");
    let mut store = IndexStore::open(&store_dir.path().join("prov.db"), &root).expect("open store");
    store.refresh(&IndexOptions::default()).expect("refresh");

    let outcome = search(&store, &SearchQuery::new("IndexStore").with_limit(5)).expect("search");
    assert!(!outcome.hits.is_empty());
    for (index, hit) in outcome.hits.iter().enumerate() {
        assert_eq!(hit.rank, index, "rank must be the position in the result");
        assert!(!hit.path.is_empty());
        assert!(hit.line >= 1, "lines are 1-based");
        assert!(
            !hit.modalities.is_empty(),
            "a hit with no modality cannot be attributed to any pass"
        );
        assert!(hit.fused_score > 0.0);
        assert!(
            !hit.scope.is_empty(),
            "the scope identity the index was built against must ride on the hit"
        );
        // The tree is unmodified, so every hit must be fresh. Asserting
        // freshness rather than merely asserting the field exists is what
        // makes the staleness case in the next test meaningful.
        assert!(
            !hit.staleness.content_stale && !hit.staleness.missing_on_disk,
            "unexpectedly stale on an unmodified tree: {hit:?}"
        );
    }
    assert!(
        outcome.semantic.starts_with("unavailable"),
        "with the optional semantic layer unbuilt, the product must SAY so \
         rather than silently serving lexical-only: {}",
        outcome.semantic
    );
}

#[test]
fn a_query_full_text_cannot_serve_falls_back_and_says_so() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let store_dir = tempfile::tempdir().expect("store tempdir");
    let mut store =
        IndexStore::open(&store_dir.path().join("fallback.db"), &root).expect("open store");
    store.refresh(&IndexOptions::default()).expect("refresh");

    // A punctuation-only literal: FTS5's tokenizer discards every character,
    // so the indexed passes cannot serve it. Returning `[]` would assert this
    // repository contains no `=> {`, which is false.
    let outcome = search(&store, &SearchQuery::new("=> {").with_limit(5)).expect("search");
    assert!(
        outcome.used_fallback,
        "the query must be reported as answered by the fallback"
    );
    assert!(!outcome.servable_by_index);
    assert!(
        !outcome.hits.is_empty(),
        "exact search must find `=> {{` in this crate's own sources"
    );
    for hit in &outcome.hits {
        assert!(
            hit.modalities
                .iter()
                .any(|m| m.modality == wcore_repomap::Modality::ExactFallback),
            "a fallback hit must name the fallback as its modality: {hit:?}"
        );
    }

    // And the contrast case: a token that genuinely is not present must come
    // back EMPTY and NOT flagged as a fallback answer, so "no matches" and
    // "cannot be served" stay distinguishable.
    let absent = search(
        &store,
        &SearchQuery::new("zzzznosuchtokenanywhere").with_limit(5),
    )
    .expect("search");
    assert!(absent.hits.is_empty(), "{:?}", absent.hits);
    assert!(!absent.used_fallback);
}

#[test]
fn editing_an_indexed_file_makes_its_hit_report_itself_stale() {
    // The store is built against a COPY of this crate's sources, so the edit
    // that produces the stale verdict cannot touch the real tree.
    let work = tempfile::tempdir().expect("work tempdir");
    let root = work.path().join("crate");
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::write(
        root.join("src/staleme.rs"),
        "pub fn stale_marker_function() {}\n",
    )
    .expect("write");

    let store_dir = tempfile::tempdir().expect("store tempdir");
    let mut store =
        IndexStore::open(&store_dir.path().join("stale.db"), &root).expect("open store");
    store.refresh(&IndexOptions::default()).expect("refresh");

    let before = search(&store, &SearchQuery::new("stale_marker_function")).expect("search");
    assert_eq!(before.hits.len(), 1, "{:?}", before.hits);
    assert!(!before.hits[0].staleness.content_stale);

    std::fs::write(
        root.join("src/staleme.rs"),
        "pub fn stale_marker_function() { /* edited after indexing */ }\n",
    )
    .expect("rewrite");

    let after = search(&store, &SearchQuery::new("stale_marker_function")).expect("search");
    assert_eq!(after.hits.len(), 1, "{:?}", after.hits);
    assert!(
        after.hits[0].staleness.content_stale,
        "a hit whose file changed after indexing must report itself stale \
         rather than being served as current: {:?}",
        after.hits[0]
    );
}

#[test]
fn a_query_is_bounded_by_the_limit_the_caller_sets() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let store_dir = tempfile::tempdir().expect("store tempdir");
    let mut store =
        IndexStore::open(&store_dir.path().join("bound.db"), &root).expect("open store");
    store.refresh(&IndexOptions::default()).expect("refresh");

    // `the` matches nearly every file in the corpus; without the bound this
    // would return the whole index into the caller's context window
    // (T-23B03-05).
    let unbounded = search(&store, &SearchQuery::new("the").with_limit(500)).expect("search");
    assert!(unbounded.hits.len() > 3, "precondition: a broad query");

    for limit in [1usize, 2, 3] {
        let outcome = search(&store, &SearchQuery::new("the").with_limit(limit)).expect("search");
        assert!(
            outcome.hits.len() <= limit,
            "limit {limit} was not enforced: got {}",
            outcome.hits.len()
        );
    }

    // And the ceiling is the store's, not the caller's: a caller asking for
    // more than MAX_LIMIT does not get more than MAX_LIMIT.
    let over = search(&store, &SearchQuery::new("the").with_limit(usize::MAX)).expect("search");
    assert!(over.hits.len() <= wcore_repomap::search::MAX_LIMIT);
}
