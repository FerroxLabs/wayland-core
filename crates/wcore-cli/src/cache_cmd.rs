//! `wayland-core cache` — the operator surface for F23-04's cache and
//! compaction ledger.
//!
//! Four verbs: `report`, `list`, `show` and `verify`.
//!
//! ## Why a subcommand and not a log line
//!
//! Everything this renders was already being computed inside the engine before
//! F23-04, and none of it was reachable. The cache-health signal was a
//! `tracing::warn!` whose own comment described it as *"greppable in the engine
//! log"*; the hit-rate line was behind `compact.cache_diagnostics = true`, which
//! defaults to `false`; token pressure was not surfaced at all. Phase 23's
//! Success Criterion 4 asks for those four families to be **exposed**, and a
//! number an operator can only reach by grepping a log file they must first
//! know to enable is not exposed.
//!
//! Like `wayland-core index`, this is also the **measurement instrument** for
//! its own criterion: the live proof runs these verbs against a ledger a real
//! session wrote, so the number in the evidence file is a number the product
//! actually printed. Every verb emits stable `F23_CACHE=` `key=value` lines a
//! shell can parse without touching prose.
//!
//! ## Exit-code map
//!
//! | Code | Meaning |
//! |-----:|---------|
//! | `0`  | success; for `verify`, the ledger's cost is fully priced |
//! | `1`  | the operation failed (unreadable or malformed ledger) |
//! | `7`  | `verify` only: the cost is NOT fully priced — the USD figure is a floor, not spend |
//! | `8`  | `verify` only: no ledger exists to verify |
//!
//! `verify` reports through the exit code as well as through its output,
//! because a cost-trust check whose only signal is a line of prose is a check a
//! script will forget to read. Exits 8 rather than 0 on an empty store for the
//! same reason: "there is nothing to check" must not read as "the check passed".

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand};

use wcore_agent::cache_ledger::{
    CacheLedger, CostTruth, LedgerSummary, TurnSample, default_ledger_dir, latest, list, load,
};

/// Exit code: `verify` found the session's cost only partially priced (or not
/// priced at all).
pub const EXIT_COST_NOT_TRUSTWORTHY: u8 = 7;
/// Exit code: `verify` found no ledger to check.
pub const EXIT_NO_LEDGER: u8 = 8;

#[derive(Args, Debug)]
pub struct CacheArgs {
    /// Ledger directory. Defaults to `<wayland home>/cache-ledger`.
    #[arg(long, global = true)]
    pub dir: Option<PathBuf>,

    /// Session id to report on. Defaults to the most recently updated ledger.
    #[arg(long, global = true)]
    pub session: Option<String>,

    /// Emit the raw ledger / summary as JSON instead of the `F23_CACHE=` form.
    #[arg(long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub cmd: CacheCmd,
}

#[derive(Subcommand, Debug)]
pub enum CacheCmd {
    /// Session-level cache quality, invalidation causes, token pressure and
    /// cost truth.
    Report,
    /// Every recorded session, newest first.
    List,
    /// Per-round-trip detail, including every compaction event.
    Show {
        /// Show only this 1-based round-trip.
        #[arg(long)]
        round_trip: Option<u64>,
    },
    /// Exit non-zero when the reported cost cannot be trusted as spend.
    Verify,
}

/// Entry point for the `cache` subcommand.
///
/// # Errors
///
/// Returns the underlying failure when the ledger directory or a named ledger
/// is unreadable.
pub fn run(args: CacheArgs) -> anyhow::Result<ExitCode> {
    let dir = args.dir.unwrap_or_else(default_ledger_dir);

    match args.cmd {
        CacheCmd::List => {
            let entries = list(&dir)?;
            for (path, ledger) in &entries {
                let s = ledger.summarize();
                println!(
                    "F23_CACHE=session id={} round_trips={} compactions={} hit_ratio={:.4} \
                     cost_usd={:.6} cost_truth={} complete={} updated={} path={}",
                    s.session_id,
                    s.round_trips,
                    s.compactions,
                    s.hit_ratio(),
                    s.cost_usd,
                    s.cost_truth().as_str(),
                    s.session_complete,
                    s.updated_at,
                    path.display(),
                );
            }
            println!(
                "F23_CACHE=list sessions={} dir={}",
                entries.len(),
                dir.display()
            );
            Ok(ExitCode::SUCCESS)
        }

        CacheCmd::Report => {
            let (path, ledger) = resolve(&dir, args.session.as_deref())?;
            let s = ledger.summarize();
            if args.json {
                println!("{}", serde_json::to_string_pretty(&summary_json(&s))?);
                return Ok(ExitCode::SUCCESS);
            }
            print_report(&s, &path);
            Ok(ExitCode::SUCCESS)
        }

        CacheCmd::Show { round_trip } => {
            let (path, ledger) = resolve(&dir, args.session.as_deref())?;
            if args.json {
                println!("{}", serde_json::to_string_pretty(&ledger)?);
                return Ok(ExitCode::SUCCESS);
            }
            let mut shown = 0u64;
            for t in &ledger.turns {
                if round_trip.is_some_and(|want| want != t.round_trip) {
                    continue;
                }
                print_turn(t);
                shown += 1;
            }
            for c in &ledger.compactions {
                println!(
                    "F23_CACHE=compaction after_round_trip={} kind={} trigger={} \
                     watermark={} threshold={} pre_tokens={} tokens_freed={} \
                     items_collapsed={} error={}",
                    c.after_round_trip,
                    c.kind.as_str(),
                    c.trigger.as_str(),
                    c.watermark_tokens,
                    c.threshold_tokens,
                    c.pre_tokens,
                    c.tokens_freed,
                    c.items_collapsed,
                    c.error.as_deref().unwrap_or("-"),
                );
            }
            println!(
                "F23_CACHE=show session={} round_trips_shown={} compactions={} path={}",
                ledger.session_id,
                shown,
                ledger.compactions.len(),
                path.display(),
            );
            Ok(ExitCode::SUCCESS)
        }

        CacheCmd::Verify => {
            let (path, ledger) = match resolve(&dir, args.session.as_deref()) {
                Ok(v) => v,
                Err(e) => {
                    // An empty store is not a pass. Say so on stdout in the
                    // same greppable form, and exit distinctly.
                    println!(
                        "F23_CACHE=verify trustworthy=false reason=no_ledger dir={}",
                        dir.display()
                    );
                    eprintln!("wayland-core cache verify: {e}");
                    return Ok(ExitCode::from(EXIT_NO_LEDGER));
                }
            };
            let s = ledger.summarize();
            let truth = s.cost_truth();
            println!(
                "F23_CACHE=verify trustworthy={} cost_truth={} catalog_priced_round_trips={} \
                 estimated_round_trips={} unpriced_round_trips={} cost_usd={:.6} \
                 session_complete={} session={} path={}",
                truth.is_trustworthy(),
                truth.as_str(),
                s.catalog_priced_round_trips,
                s.estimated_round_trips,
                s.unpriced_round_trips,
                s.cost_usd,
                s.session_complete,
                s.session_id,
                path.display(),
            );
            if truth.is_trustworthy() {
                Ok(ExitCode::SUCCESS)
            } else {
                eprintln!(
                    "wayland-core cache verify: cost is {} — of {} round-trips, {} were priced \
                     from provider-family defaults rather than a catalog row and {} could not be \
                     priced at all, so ${:.6} must not be reported as spend.",
                    truth.as_str(),
                    s.round_trips,
                    s.estimated_round_trips,
                    s.unpriced_round_trips,
                    s.cost_usd,
                );
                Ok(ExitCode::from(EXIT_COST_NOT_TRUSTWORTHY))
            }
        }
    }
}

fn resolve(
    dir: &std::path::Path,
    session: Option<&str>,
) -> Result<(PathBuf, CacheLedger), wcore_agent::cache_ledger::LedgerError> {
    match session {
        Some(id) => {
            let path = wcore_agent::cache_ledger::ledger_path(dir, id);
            let ledger = load(&path)?;
            Ok((path, ledger))
        }
        None => latest(dir),
    }
}

fn print_turn(t: &TurnSample) {
    println!(
        "F23_CACHE=turn round_trip={} turn={} provider={} model={} retention={} \
         uncached_input={} cache_read={} cache_write={} output={} hit={} hit_ratio={:.4} \
         invalidation={} cost_usd={:.6} cost_source={} uncached_equivalent_usd={:.6} \
         saving_usd={:.6} watermark={} threshold={} emergency_limit={} pressure={:.4}",
        t.round_trip,
        t.turn,
        t.provider,
        t.model,
        t.retention.as_str(),
        t.uncached_input_tokens,
        t.cache_read_tokens,
        t.cache_write_tokens,
        t.output_tokens,
        t.is_hit(),
        t.hit_ratio(),
        t.invalidation_cause.map(|c| c.as_str()).unwrap_or("-"),
        t.cost_usd,
        t.cost_source.as_str(),
        t.uncached_equivalent_usd,
        t.cache_saving_usd(),
        t.watermark_tokens,
        t.autocompact_threshold_tokens,
        t.emergency_limit_tokens,
        t.pressure_ratio(),
    );
}

/// The four criterion clauses, one `F23_CACHE=` line each, plus a per-cause
/// breakdown. Kept to fixed key names so a gate can assert on them.
fn print_report(s: &LedgerSummary, path: &std::path::Path) {
    println!(
        "F23_CACHE=session id={} round_trips={} complete={} started={} updated={} path={}",
        s.session_id,
        s.round_trips,
        s.session_complete,
        s.started_at,
        s.updated_at,
        path.display(),
    );

    // 1 — quality
    println!(
        "F23_CACHE=quality hit_ratio={:.4} warm_hit_ratio={:.4} hit_round_trips={} \
         miss_round_trips={} warm_round_trips={} cache_read={} cache_write={} \
         uncached_input={} output={} total_input={}",
        s.hit_ratio(),
        s.warm_hit_ratio(),
        s.hit_round_trips,
        s.miss_round_trips,
        s.warm_round_trips,
        s.cache_read_tokens,
        s.cache_write_tokens,
        s.uncached_input_tokens,
        s.output_tokens,
        s.total_input_tokens(),
    );

    // 2 — invalidation
    let causes = if s.invalidation_causes.is_empty() {
        "-".to_string()
    } else {
        s.invalidation_causes
            .iter()
            .map(|(k, v)| format!("{k}:{v}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    println!(
        "F23_CACHE=invalidation distinct_causes={} causes={}",
        s.invalidation_causes.len(),
        causes,
    );
    for (cause, count) in &s.invalidation_causes {
        println!("F23_CACHE=invalidation_cause name={cause} count={count}");
    }

    // 3 — token pressure
    println!(
        "F23_CACHE=pressure peak_watermark={} autocompact_threshold={} emergency_limit={} \
         peak_pressure={:.4} compactions={} micro={} auto={} failed={} tokens_reclaimed={}",
        s.peak_watermark_tokens,
        s.autocompact_threshold_tokens,
        s.emergency_limit_tokens,
        s.peak_pressure_ratio(),
        s.compactions,
        s.micro_compactions,
        s.auto_compactions,
        s.failed_compactions,
        s.tokens_reclaimed,
    );

    // 4 — cost truth
    println!(
        "F23_CACHE=cost usd={:.6} uncached_equivalent_usd={:.6} saving_usd={:.6} \
         saving_ratio={:.4} cost_truth={} catalog_priced_round_trips={} \
         estimated_round_trips={} unpriced_round_trips={}",
        s.cost_usd,
        s.uncached_equivalent_usd,
        s.cache_saving_usd(),
        s.cache_saving_ratio(),
        s.cost_truth().as_str(),
        s.catalog_priced_round_trips,
        s.estimated_round_trips,
        s.unpriced_round_trips,
    );
    if s.cost_truth() != CostTruth::Priced {
        // Loud, on stdout, in the same run — an unpriced number that renders
        // like a priced one is the failure mode this whole surface exists to
        // avoid.
        println!(
            "F23_CACHE=cost_warning text={} cost_truth={}",
            match s.cost_truth() {
                CostTruth::Estimated => "usd_is_a_family_rate_estimate_not_spend",
                _ => "usd_is_a_floor_not_spend",
            },
            s.cost_truth().as_str()
        );
    }
}

fn summary_json(s: &LedgerSummary) -> serde_json::Value {
    let mut v = serde_json::to_value(s).unwrap_or_else(|_| serde_json::json!({}));
    // Derived figures are methods, not fields; surface them explicitly so a
    // JSON consumer does not have to re-implement the arithmetic (and get the
    // warm-window rule subtly wrong).
    if let Some(obj) = v.as_object_mut() {
        obj.insert("hit_ratio".into(), serde_json::json!(s.hit_ratio()));
        obj.insert(
            "warm_hit_ratio".into(),
            serde_json::json!(s.warm_hit_ratio()),
        );
        obj.insert(
            "cache_saving_usd".into(),
            serde_json::json!(s.cache_saving_usd()),
        );
        obj.insert(
            "cache_saving_ratio".into(),
            serde_json::json!(s.cache_saving_ratio()),
        );
        obj.insert(
            "peak_pressure_ratio".into(),
            serde_json::json!(s.peak_pressure_ratio()),
        );
        obj.insert(
            "cost_truth".into(),
            serde_json::json!(s.cost_truth().as_str()),
        );
        obj.insert(
            "cost_trustworthy".into(),
            serde_json::json!(s.cost_truth().is_trustworthy()),
        );
        obj.insert(
            "total_input_tokens".into(),
            serde_json::json!(s.total_input_tokens()),
        );
    }
    v
}
