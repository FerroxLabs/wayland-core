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

    /// Session id to report on. Accepts either the ledger id printed by
    /// `cache list` or the session id passed to `--session-id` / shown by
    /// `session list`. Defaults to the most recently updated ledger.
    #[arg(long, global = true)]
    pub session: Option<String>,

    /// Session store directory, used to translate a `--session` that names a
    /// SESSION rather than a ledger. Defaults to the resolved
    /// `session.directory` (`$WAYLAND_HOME/sessions`).
    #[arg(long, global = true)]
    pub session_dir: Option<PathBuf>,

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
    let session_dir = args.session_dir;

    match args.cmd {
        CacheCmd::List => {
            let entries = list(&dir)?;
            let mut summaries = Vec::with_capacity(entries.len());
            for (path, ledger) in &entries {
                let s = ledger.summarize();
                println!(
                    "F23_CACHE=session id={} round_trips={} compactions={} hit_ratio={:.4} \
                     cost_usd={} cost_truth={} complete={} updated={} path={}",
                    s.session_id,
                    s.round_trips,
                    s.compactions,
                    s.hit_ratio(),
                    render_cost_usd(s.cost_usd, s.cost_truth()),
                    s.cost_truth().as_str(),
                    s.session_complete,
                    s.updated_at,
                    path.display(),
                );
                summaries.push(s);
            }
            println!(
                "F23_CACHE=list sessions={} dir={}",
                entries.len(),
                dir.display()
            );
            print_totals(&StoreTotals::of(&summaries), &dir);
            Ok(ExitCode::SUCCESS)
        }

        CacheCmd::Report => {
            let (path, ledger) = resolve(&dir, args.session.as_deref(), session_dir.as_deref())?;
            let s = ledger.summarize();
            if args.json {
                println!("{}", serde_json::to_string_pretty(&summary_json(&s))?);
                return Ok(ExitCode::SUCCESS);
            }
            print_report(&s, &path);
            Ok(ExitCode::SUCCESS)
        }

        CacheCmd::Show { round_trip } => {
            let (path, ledger) = resolve(&dir, args.session.as_deref(), session_dir.as_deref())?;
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
            let (path, ledger) =
                match resolve(&dir, args.session.as_deref(), session_dir.as_deref()) {
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
                "F23_CACHE=verify trustworthy={} cost_truth={} saving_truth={} \
                 provider_reported_round_trips={} catalog_priced_round_trips={} \
                 estimated_round_trips={} unpriced_round_trips={} cost_usd={} \
                 session_complete={} session={} path={}",
                truth.is_trustworthy(),
                truth.as_str(),
                // Reported, not enforced: `verify`'s documented contract and
                // exit code 7 are about whether the BILLED figure is spend. A
                // provider-reported spend stays spend even when no catalog can
                // price its counterfactual, so #1163 must not start failing CI
                // for every session on an unlisted model.
                s.saving_truth().as_str(),
                s.provider_reported_round_trips,
                s.catalog_priced_round_trips,
                s.estimated_round_trips,
                s.unpriced_round_trips,
                render_cost_usd(s.cost_usd, truth),
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
                     priced at all, so {} must not be reported as spend.",
                    truth.as_str(),
                    s.round_trips,
                    s.estimated_round_trips,
                    s.unpriced_round_trips,
                    match truth {
                        CostTruth::Unpriced => "the reported figure".to_string(),
                        _ => format!("${:.6}", s.cost_usd),
                    },
                );
                Ok(ExitCode::from(EXIT_COST_NOT_TRUSTWORTHY))
            }
        }
    }
}

/// Store-wide totals across every ledger in the directory.
///
/// ## Why a cross-session total exists
///
/// Measured live on `wayland-core 0.12.25`: the only operator-configurable
/// provider ceiling is **per session** (`budget cap 'per_session_input_tokens'`;
/// the tracker's `per_user_daily_usd` has no TOML counterpart). So five
/// sequential launches, each starting a fresh session, billed 100000 input
/// tokens against a 25000-token cap with **zero** refusals — identically with
/// durable session journalling on and degraded off, because a new session
/// legitimately re-arms the ceiling.
///
/// The per-session ledgers were already on disk for every one of those launches
/// (recording is on by default and survives the headless degrade). What was
/// missing was the **sum**: `report` describes one session and `list` printed a
/// bare `sessions=N` count with no totals at all. An operator whose work has
/// fragmented across restart-sessions therefore had the data and no way to see
/// what it added up to — the exact shape of "every individual run believes it
/// is within budget".
///
/// This is **observability, not enforcement.** It cannot stop a crash loop from
/// spending; it turns an invisible cumulative bleed into one number a human, an
/// alert or a wrapper script can act on. A cross-session *ceiling* belongs in a
/// dedicated, atomically-written budget store with fail-closed semantics — not
/// bolted onto a cache-diagnostics ledger that may be pruned, partially written
/// or absent, where fail-open silently restores the hole and fail-closed bricks
/// every launch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoreTotals {
    pub sessions: usize,
    /// Sessions whose ledger was never marked complete — the signature a crash
    /// loop leaves behind.
    pub incomplete_sessions: usize,
    pub round_trips: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
    /// `None` when ANY session in the store had an unpriceable counterfactual —
    /// a partial sum is a floor, and a floor here understates the saving with
    /// full confidence (#1163).
    pub uncached_equivalent_usd: Option<f64>,
    pub provider_reported_round_trips: u64,
    pub catalog_priced_round_trips: u64,
    pub estimated_round_trips: u64,
    pub unpriced_round_trips: u64,
}

impl StoreTotals {
    #[must_use]
    pub fn of(summaries: &[LedgerSummary]) -> Self {
        let mut t = Self {
            sessions: summaries.len(),
            // An empty store has no counterfactual to total; `Some(0.0)`
            // would render as "the cache saved exactly nothing", which is a
            // claim about a store that holds no data.
            uncached_equivalent_usd: (!summaries.is_empty()).then_some(0.0),
            ..Self::default()
        };
        for s in summaries {
            if !s.session_complete {
                t.incomplete_sessions += 1;
            }
            t.round_trips = t.round_trips.saturating_add(s.round_trips);
            t.input_tokens = t.input_tokens.saturating_add(s.total_input_tokens());
            t.output_tokens = t.output_tokens.saturating_add(s.output_tokens);
            t.cost_usd += s.cost_usd;
            match s.uncached_equivalent_usd {
                Some(usd) => {
                    if let Some(total) = t.uncached_equivalent_usd.as_mut() {
                        *total += usd;
                    }
                }
                None => t.uncached_equivalent_usd = None,
            }
            t.provider_reported_round_trips = t
                .provider_reported_round_trips
                .saturating_add(s.provider_reported_round_trips);
            t.catalog_priced_round_trips = t
                .catalog_priced_round_trips
                .saturating_add(s.catalog_priced_round_trips);
            t.estimated_round_trips = t
                .estimated_round_trips
                .saturating_add(s.estimated_round_trips);
            t.unpriced_round_trips = t
                .unpriced_round_trips
                .saturating_add(s.unpriced_round_trips);
        }
        t
    }

    /// Grade the summed USD with the SAME rule a single session uses.
    ///
    /// Deliberately re-derived by handing the summed counts back to
    /// [`LedgerSummary::cost_truth`] rather than re-implementing the ladder
    /// here: two copies of a four-way grading rule is how a store total starts
    /// reporting `priced` for a set containing an unpriced session.
    #[must_use]
    pub fn cost_truth(&self) -> CostTruth {
        LedgerSummary {
            provider_reported_round_trips: self.provider_reported_round_trips,
            catalog_priced_round_trips: self.catalog_priced_round_trips,
            estimated_round_trips: self.estimated_round_trips,
            unpriced_round_trips: self.unpriced_round_trips,
            ..LedgerSummary::default()
        }
        .cost_truth()
    }
}

/// One `F23_CACHE=total` line, plus the same cost warning `report` emits — an
/// aggregate USD figure that renders like spend when it is a floor is the
/// failure this surface exists to avoid, and summing many sessions makes it
/// look *more* authoritative, not less.
fn print_totals(t: &StoreTotals, dir: &std::path::Path) {
    let truth = t.cost_truth();
    println!(
        "F23_CACHE=total sessions={} incomplete_sessions={} round_trips={} input_tokens={} \
         output_tokens={} cost_usd={} uncached_equivalent_usd={} cost_truth={} \
         provider_reported_round_trips={} catalog_priced_round_trips={} \
         estimated_round_trips={} unpriced_round_trips={} dir={}",
        t.sessions,
        t.incomplete_sessions,
        t.round_trips,
        t.input_tokens,
        t.output_tokens,
        render_cost_usd(t.cost_usd, truth),
        render_opt_usd(t.uncached_equivalent_usd),
        truth.as_str(),
        t.provider_reported_round_trips,
        t.catalog_priced_round_trips,
        t.estimated_round_trips,
        t.unpriced_round_trips,
        dir.display(),
    );
    if truth != CostTruth::Priced {
        println!(
            "F23_CACHE=total_cost_warning text={} cost_truth={}",
            match truth {
                CostTruth::Estimated => "total_usd_is_a_family_rate_estimate_not_spend",
                _ => "total_usd_is_a_floor_not_spend",
            },
            truth.as_str()
        );
    }
}

/// #1139 — render a USD figure, or the word `unpriced` when nothing could be
/// priced at all.
///
/// `cost_usd=0.000000` is a claim: it says the calls were free. An
/// [`CostTruth::Unpriced`] ledger has made no such claim — it has no number —
/// and printing the zero is exactly how a real spend reads as no spend. The
/// media-tool path already draws this line (`cost_from_headers` returns
/// `None`, "never a zero"); this is the same rule at the render edge.
fn render_cost_usd(cost_usd: f64, truth: CostTruth) -> String {
    if truth == CostTruth::Unpriced {
        "unpriced".to_string()
    } else {
        format!("{cost_usd:.6}")
    }
}

/// #1163 — render an optional USD figure, or the word `unknown`.
///
/// Same rule as [`render_cost_usd`] one column over. `uncached_equivalent_usd`
/// and the saving derived from it are absent whenever the catalog cannot price
/// the counterfactual, and `0.000000` there is a claim the ledger has not made:
/// it turns `saving_usd` into exactly `-cost` and tells an operator the cache
/// is costing them everything. MEASURED on `flux-router`/`flux-reasoning`
/// beside a 98% warm hit ratio in the same report.
fn render_opt_usd(value: Option<f64>) -> String {
    value.map_or_else(|| "unknown".to_string(), |v| format!("{v:.6}"))
}

/// The same, for a ratio.
fn render_opt_ratio(value: Option<f64>) -> String {
    value.map_or_else(|| "unknown".to_string(), |v| format!("{v:.4}"))
}

/// Find the ledger `--session <id>` names, accepting EITHER identifier.
///
/// #1162 — the ledger is keyed by the engine's internal `conversation_id`; the
/// flag says "Session id to report on", which is the id the user actually
/// controls (`--session-id`). Nothing bridged the two, so a scripted run that
/// set its own session id could only ever get
/// `ledger io error at .../<their id>.json: No such file or directory` — a bare
/// io error that reads as "nothing was recorded" when the record is right there
/// under another name.
///
/// The bridge is `Session::conversation_id`, persisted on the session snapshot
/// since #1161. Ledger id is tried FIRST so an operator who copied a key out of
/// `cache list` never pays for a session-store lookup, and so this cannot
/// change the meaning of an id that already resolved.
fn resolve(
    dir: &std::path::Path,
    session: Option<&str>,
    session_dir: Option<&std::path::Path>,
) -> anyhow::Result<(PathBuf, CacheLedger)> {
    let Some(id) = session else {
        return Ok(latest(dir)?);
    };
    let path = wcore_agent::cache_ledger::ledger_path(dir, id);
    match load(&path) {
        Ok(ledger) => return Ok((path, ledger)),
        // Only a MISSING ledger is worth a second interpretation. A malformed
        // or wrong-schema file at that path is a real answer to the question
        // asked, and silently reporting on a different session instead would
        // hide it.
        Err(wcore_agent::cache_ledger::LedgerError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound => {}
        Err(other) => return Err(other.into()),
    }

    let manager = session_manager(session_dir);
    // Read the snapshot directly rather than through `SessionManager::load`.
    // `load` repairs what it reads — it merges and then DELETES a pending WAL,
    // and re-writes the index for an unindexed session. `cache report` is a
    // read verb; it must not mutate the session store to answer a question
    // about a ledger.
    let conversation_id = std::fs::read_to_string(manager.session_file_path(id))
        .ok()
        .and_then(|raw| serde_json::from_str::<wcore_agent::session::Session>(&raw).ok())
        .and_then(|session| session.conversation_id);
    if let Some(conversation_id) = conversation_id {
        let path = wcore_agent::cache_ledger::ledger_path(dir, &conversation_id);
        match load(&path) {
            Ok(ledger) => return Ok((path, ledger)),
            Err(e) => {
                anyhow::bail!(
                    "session '{id}' recorded conversation id '{conversation_id}', but its ledger                      could not be read: {e}"
                );
            }
        }
    }

    // Name the key the store is actually addressed by, and the verb that lists
    // the keys. The old message named a path that had never existed and said
    // nothing else.
    anyhow::bail!(
        "no cache ledger for '{id}' in {}. Ledgers are keyed by the engine's internal          conversation id, not by the session id you set with --session-id; session '{id}' is          either unknown to the session store at {} or was recorded by a build that did not          persist its conversation id. Run `wayland-core cache list` to see the ids that exist.",
        dir.display(),
        manager.directory().display(),
    )
}

/// The session store, without requiring a provider API key — the same contract
/// `session list` honours, and the reason `cache` is dispatched before
/// `Config::resolve` in the first place.
fn session_manager(session_dir: Option<&std::path::Path>) -> wcore_agent::session::SessionManager {
    if let Some(dir) = session_dir {
        return wcore_agent::session::SessionManager::new(dir.to_path_buf(), 50);
    }
    let config = wcore_config::config::Config::resolve(&wcore_config::config::CliArgs {
        provider: None,
        api_key: None,
        base_url: None,
        model: None,
        max_tokens: None,
        max_turns: None,
        system_prompt: None,
        profile: None,
        auto_approve: false,
        project_dir: None,
    })
    .unwrap_or_default();
    wcore_agent::session::SessionManager::new(
        config.session.directory.clone().into(),
        config.session.max_sessions,
    )
}

fn print_turn(t: &TurnSample) {
    println!(
        "F23_CACHE=turn round_trip={} turn={} provider={} model={} retention={} \
         uncached_input={} cache_read={} cache_write={} output={} hit={} hit_ratio={:.4} \
         invalidation={} cost_usd={} cost_source={} uncached_equivalent_usd={} \
         saving_usd={} watermark={} threshold={} emergency_limit={} pressure={:.4}",
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
        // Same rule one level down: an unpriced round-trip has no number, and
        // `cost_usd=0.000000` beside `cost_source=unpriced` reads as "free".
        if t.cost_source.is_priced() {
            format!("{:.6}", t.cost_usd)
        } else {
            "unpriced".to_string()
        },
        t.cost_source.as_str(),
        render_opt_usd(t.uncached_equivalent_usd),
        render_opt_usd(t.cache_saving_usd()),
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
        "F23_CACHE=cost usd={} uncached_equivalent_usd={} saving_usd={} \
         saving_ratio={} cost_truth={} saving_truth={} \
         counterfactual_unpriced_round_trips={} provider_reported_round_trips={} \
         catalog_priced_round_trips={} estimated_round_trips={} unpriced_round_trips={}",
        render_cost_usd(s.cost_usd, s.cost_truth()),
        render_opt_usd(s.uncached_equivalent_usd),
        render_opt_usd(s.cache_saving_usd()),
        render_opt_ratio(s.cache_saving_ratio()),
        s.cost_truth().as_str(),
        s.saving_truth().as_str(),
        s.counterfactual_unpriced_round_trips,
        s.provider_reported_round_trips,
        s.catalog_priced_round_trips,
        s.estimated_round_trips,
        s.unpriced_round_trips,
    );
    // #1163 — the saving is a difference, so it is graded separately from the
    // billed figure. A `cost_truth=priced` session whose counterfactual nothing
    // could price is the exact shape this warning exists for: the spend is a
    // fact, the saving is not, and one verdict cannot say both.
    if s.saving_truth() != CostTruth::Priced && s.cost_truth() == CostTruth::Priced {
        println!(
            "F23_CACHE=saving_warning text=no_catalog_rate_for_the_uncached_counterfactual \
             saving_truth={} counterfactual_unpriced_round_trips={}",
            s.saving_truth().as_str(),
            s.counterfactual_unpriced_round_trips,
        );
    }
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
            "saving_truth".into(),
            serde_json::json!(s.saving_truth().as_str()),
        );
        obj.insert(
            "cost_trustworthy".into(),
            serde_json::json!(s.cost_truth().is_trustworthy()),
        );
        obj.insert(
            "total_input_tokens".into(),
            serde_json::json!(s.total_input_tokens()),
        );
        obj.insert(
            "priced_round_trips".into(),
            serde_json::json!(s.priced_round_trips()),
        );
    }
    v
}

#[cfg(test)]
mod store_total_tests {
    use super::*;

    /// One session's ledger, parameterised on the axes the total sums.
    ///
    /// `round_trips` is derived rather than passed: a ledger whose declared
    /// round-trip count disagrees with its priced/estimated/unpriced breakdown
    /// is not a state the recorder can produce, and letting a test build one
    /// would let the totals pass on inputs the product never emits.
    struct Session {
        id: &'static str,
        complete: bool,
        uncached_input: u64,
        output: u64,
        usd: f64,
        priced: u64,
        estimated: u64,
        unpriced: u64,
    }

    impl Session {
        /// A complete, catalog-priced session — the ordinary case. Grade and
        /// completeness are then overridden per test.
        fn priced(id: &'static str, round_trips: u64, uncached_input: u64, usd: f64) -> Self {
            Self {
                id,
                complete: true,
                uncached_input,
                output: 100 * round_trips,
                usd,
                priced: round_trips,
                estimated: 0,
                unpriced: 0,
            }
        }

        fn incomplete(mut self) -> Self {
            self.complete = false;
            self
        }

        /// Move this session's round-trips to a different pricing grade,
        /// preserving the total count.
        fn graded(mut self, estimated: u64, unpriced: u64) -> Self {
            let total = self.priced + self.estimated + self.unpriced;
            self.estimated = estimated;
            self.unpriced = unpriced;
            self.priced = total.saturating_sub(estimated).saturating_sub(unpriced);
            self
        }

        fn build(&self) -> LedgerSummary {
            LedgerSummary {
                session_id: self.id.to_owned(),
                session_complete: self.complete,
                round_trips: self.priced + self.estimated + self.unpriced,
                uncached_input_tokens: self.uncached_input,
                output_tokens: self.output,
                cost_usd: self.usd,
                uncached_equivalent_usd: Some(self.usd),
                catalog_priced_round_trips: self.priced,
                estimated_round_trips: self.estimated,
                unpriced_round_trips: self.unpriced,
                ..LedgerSummary::default()
            }
        }
    }

    fn totals(sessions: &[Session]) -> StoreTotals {
        let built: Vec<LedgerSummary> = sessions.iter().map(Session::build).collect();
        StoreTotals::of(&built)
    }

    /// The measured scenario, encoded: five restart-fragments of one workload,
    /// each a fresh session that legitimately re-armed a 25000-token ceiling.
    /// Per session the product refuses nothing; the store total is the only
    /// place the 100000 becomes visible.
    #[test]
    fn five_restart_fragments_sum_to_the_spend_no_single_session_can_show() {
        let fragments: Vec<LedgerSummary> = ["f0", "f1", "f2", "f3", "f4"]
            .into_iter()
            .map(|id| Session::priced(id, 1, 20_000, 0.25).build())
            .collect();

        let total = StoreTotals::of(&fragments);

        assert_eq!(total.sessions, 5);
        assert_eq!(total.round_trips, 5);
        assert_eq!(total.input_tokens, 100_000);
        assert_eq!(total.output_tokens, 500);
        assert!((total.cost_usd - 1.25).abs() < 1e-9, "{}", total.cost_usd);
        // Every fragment is individually under a 25000-token ceiling; the sum
        // is 4x it. That gap is the whole reason this total exists.
        assert!(fragments.iter().all(|s| s.total_input_tokens() < 25_000));
        assert!(total.input_tokens > 25_000 * 3);
    }

    /// A crash loop leaves ledgers that were never marked complete. Counting
    /// them is what distinguishes "you ran five jobs" from "one job died four
    /// times".
    #[test]
    fn incomplete_sessions_are_counted_separately_from_sessions() {
        let total = totals(&[
            Session::priced("done", 2, 10, 0.1),
            Session::priced("died-1", 1, 10, 0.1).incomplete(),
            Session::priced("died-2", 1, 10, 0.1).incomplete(),
        ]);
        assert_eq!(total.sessions, 3);
        assert_eq!(total.incomplete_sessions, 2);
        assert_eq!(total.round_trips, 4);
    }

    /// Both directions on the grade. A store total that renders `priced`
    /// because most of its sessions were priced is the single worst thing this
    /// surface could do — it makes a floor look like a fact, and summing makes
    /// it look more authoritative rather than less.
    #[test]
    fn the_aggregate_grade_takes_the_worst_session_in_the_store() {
        let all_priced = totals(&[
            Session::priced("a", 1, 10, 1.0),
            Session::priced("b", 1, 10, 1.0),
        ]);
        assert_eq!(all_priced.cost_truth(), CostTruth::Priced);
        assert!(all_priced.cost_truth().is_trustworthy());

        let one_unpriced = totals(&[
            Session::priced("a", 1, 10, 1.0),
            Session::priced("b", 1, 10, 0.0).graded(0, 1),
        ]);
        assert_eq!(one_unpriced.cost_truth(), CostTruth::Partial);
        assert!(!one_unpriced.cost_truth().is_trustworthy());

        let one_estimated = totals(&[
            Session::priced("a", 1, 10, 1.0),
            Session::priced("b", 1, 10, 1.0).graded(1, 0),
        ]);
        assert_eq!(one_estimated.cost_truth(), CostTruth::Estimated);

        let none_priced = totals(&[Session::priced("a", 1, 10, 0.0).graded(0, 1)]);
        assert_eq!(none_priced.cost_truth(), CostTruth::Unpriced);
    }

    /// An empty store must not grade `priced` at $0.00 — "there is nothing to
    /// total" must not read as "the total is a trustworthy zero". Same rule
    /// `verify` already applies with its distinct exit code.
    #[test]
    fn an_empty_store_totals_to_unpriced_not_to_a_trustworthy_zero() {
        let total = StoreTotals::of(&[]);
        assert_eq!(total.sessions, 0);
        assert_eq!(total.round_trips, 0);
        assert_eq!(total.cost_usd, 0.0);
        assert_eq!(total.cost_truth(), CostTruth::Unpriced);
        assert!(!total.cost_truth().is_trustworthy());
    }
}
