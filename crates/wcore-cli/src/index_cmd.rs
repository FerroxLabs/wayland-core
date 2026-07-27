//! `wayland-core index` — the operator surface for F23-06's persistent
//! repository index.
//!
//! Four verbs: `build`, `status`, `search` and `verify`.
//!
//! ## This is also the measurement instrument
//!
//! Phase 23B plan 03's perf and quality gates are measured **through this
//! subcommand against a real repository**, not through a benchmark harness
//! wired to the library. That is deliberate: a number produced by a path the
//! product does not actually take is not a measurement of the product. It
//! constrains the output format — every verb prints one or more stable,
//! greppable `F23_INDEX=` lines to STDOUT carrying `key=value` fields a shell
//! can parse without touching prose.
//!
//! ## Exit-code map
//!
//! | Code | Meaning |
//! |-----:|---------|
//! | `0`  | success; for `verify`, the store agrees with the working tree |
//! | `1`  | the operation failed (unreadable store, unreadable root) |
//! | `6`  | `verify` only: the store DISAGREES with the working tree |
//!
//! `verify` reports disagreement through the exit code as well as through its
//! output, because a drift check whose only signal is a line of text is a
//! check a script will forget to read.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};

use wcore_repomap::{IndexOptions, IndexStore, SearchQuery, search, semantic_status};

/// Exit code: `verify` found the store and the working tree in disagreement.
pub const EXIT_STORE_DISAGREES: u8 = 6;

#[derive(Args, Debug)]
pub struct IndexArgs {
    /// Repository root to index. Defaults to the current directory.
    #[arg(long, global = true)]
    pub root: Option<PathBuf>,

    /// Store file. Defaults to `<root>/.wayland/repomap-index.db`.
    #[arg(long, global = true)]
    pub store: Option<PathBuf>,

    #[command(subcommand)]
    pub cmd: IndexCmd,
}

#[derive(Subcommand, Debug)]
pub enum IndexCmd {
    /// Build or incrementally refresh the index.
    Build,
    /// Report record count, on-disk size, scope identity and staleness.
    Status,
    /// Bounded hybrid retrieval: BM25 plus symbol lookup, reciprocal-rank
    /// fused, with an exact-search fallback.
    Search {
        /// Query text.
        query: String,
        /// Maximum hits.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Compare the store against the working tree without modifying it.
    Verify,
}

/// Entry point for the `index` subcommand.
///
/// # Errors
///
/// Returns the underlying failure when the root or the store is unusable.
pub fn run(args: IndexArgs) -> anyhow::Result<ExitCode> {
    let root = match args.root {
        Some(root) => root,
        None => std::env::current_dir()?,
    };
    let store_path = args
        .store
        .unwrap_or_else(|| root.join(".wayland").join("repomap-index.db"));

    match args.cmd {
        IndexCmd::Build => {
            let started = std::time::Instant::now();
            let mut store = IndexStore::open(&store_path, &root)?;
            let stats = store.refresh(&IndexOptions::default())?;
            let elapsed_ms = started.elapsed().as_millis();
            println!(
                "F23_INDEX=build scanned={} read={} extracted={} added={} changed={} \
                 deleted={} renamed={} unchanged={} scope_changed={} elapsed_ms={} \
                 records={} symbols={} store_bytes={}",
                stats.scanned,
                stats.files_read,
                stats.files_extracted,
                stats.added,
                stats.changed,
                stats.deleted,
                stats.renamed,
                stats.unchanged,
                stats.scope_changed,
                elapsed_ms,
                store.record_count()?,
                store.symbol_count()?,
                store.on_disk_bytes(),
            );
            Ok(ExitCode::SUCCESS)
        }

        IndexCmd::Status => {
            let started = std::time::Instant::now();
            let store = IndexStore::open(&store_path, &root)?;
            // Measured as a WARM START: the wall time to open a store and be
            // able to answer from it, with no refresh. That is the number a
            // long-running session actually pays.
            let records = store.record_count()?;
            let open_ms = started.elapsed().as_millis();
            let recorded = store
                .recorded_scope()?
                .map(|s| s.fingerprint())
                .unwrap_or_else(|| "-".to_string());
            println!(
                "F23_INDEX=status records={} symbols={} store_bytes={} \
                 scope_drifted={} warm_open_ms={} store={}",
                records,
                store.symbol_count()?,
                store.on_disk_bytes(),
                store.scope_drifted()?,
                open_ms,
                store_path.display(),
            );
            println!("F23_INDEX=scope recorded={recorded}");
            println!(
                "F23_INDEX=scope current={}",
                store.current_scope().fingerprint()
            );
            println!("F23_INDEX=semantic status={}", semantic_status());
            Ok(ExitCode::SUCCESS)
        }

        IndexCmd::Search { query, limit } => {
            let store = IndexStore::open(&store_path, &root)?;
            let outcome = search(&store, &SearchQuery::new(query).with_limit(limit))?;
            for hit in &outcome.hits {
                let modalities = hit
                    .modalities
                    .iter()
                    .map(|m| format!("{}:{}", m.modality.as_str(), m.rank))
                    .collect::<Vec<_>>()
                    .join(",");
                println!(
                    "F23_INDEX=hit rank={} path={} line={} modalities={} score={:.6} \
                     content_stale={} missing={} scope_drifted={} scope={}",
                    hit.rank,
                    hit.path,
                    hit.line,
                    modalities,
                    hit.fused_score,
                    hit.staleness.content_stale,
                    hit.staleness.missing_on_disk,
                    hit.staleness.scope_drifted,
                    hit.scope,
                );
            }
            println!(
                "F23_INDEX=search hits={} fallback={} servable={} scope_drifted={} \
                 elapsed_us={}",
                outcome.hits.len(),
                outcome.used_fallback,
                outcome.servable_by_index,
                outcome.scope_drifted,
                outcome.elapsed_micros,
            );
            println!("F23_INDEX=semantic status={}", outcome.semantic);
            Ok(ExitCode::SUCCESS)
        }

        IndexCmd::Verify => {
            let store = IndexStore::open(&store_path, &root)?;
            let report = store.verify(&IndexOptions::default())?;
            println!(
                "F23_INDEX=verify agrees={} records={} in_scope={} changed={} \
                 missing_from_store={} missing_from_disk={} scope_drifted={}",
                report.agrees(),
                report.records,
                report.in_scope,
                report.changed,
                report.missing_from_store,
                report.missing_from_disk,
                report.scope_drifted,
            );
            if report.agrees() {
                Ok(ExitCode::SUCCESS)
            } else {
                Ok(ExitCode::from(EXIT_STORE_DISAGREES))
            }
        }
    }
}
