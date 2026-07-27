use std::sync::Arc;

use wcore_memory::MemoryApi;
use wcore_memory::v2_types::{AccessToken, Partition, Tier};

use super::{SlashError, SlashHandler, SlashInvocation, SlashOutcome};

/// `/memory` handler. Two variants:
///
/// - [`MemoryHandler::Stub`] is the back-compat shape used by
///   [`crate::slash::Dispatcher::with_builtins`]. It returns placeholder
///   strings that the pre-v0.8.0 code shipped — every existing test
///   continues to pass against this variant.
/// - [`MemoryHandler::Runtime`] carries a live `Arc<dyn MemoryApi>` and
///   reaches the real partition store on every invocation. The CLI
///   construction path swaps the stub for this variant via
///   [`crate::slash::Dispatcher::with_runtime`] right after engine
///   bootstrap.
#[derive(Clone, Default)]
pub enum MemoryHandler {
    #[default]
    Stub,
    Runtime {
        api: Arc<dyn MemoryApi>,
    },
}

impl std::fmt::Debug for MemoryHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stub => f.debug_struct("MemoryHandler::Stub").finish(),
            Self::Runtime { .. } => f.debug_struct("MemoryHandler::Runtime").finish(),
        }
    }
}

impl SlashHandler for MemoryHandler {
    fn name(&self) -> &str {
        "memory"
    }
    fn one_line_help(&self) -> &str {
        "Inspect, correct, forget, privacy-scope or retention-bound memory."
    }
    fn invoke(&self, invocation: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        match invocation.args.split_first() {
            None => self.show(None),
            Some((first, rest)) => match first.as_str() {
                "show" => self.show(rest.first().map(|s| s.as_str())),
                "clear" => self.clear(rest.first().map(|s| s.as_str())),
                // F23-03 control surface. Each is Runtime-only: the Stub has
                // no store to act on, and reporting a control as applied
                // against no store is the one thing these commands must never
                // do.
                "why" | "provenance" => self.provenance(rest),
                "correct" => self.correct(rest),
                "forget" => self.forget(rest),
                "privacy" => self.privacy(rest),
                "retention" => self.retention(rest),
                other => Err(SlashError::Bad(format!(
                    "/memory: unknown sub-action '{other}'. Try: /memory show [partition] | \
                     /memory why <query> | /memory correct <id> <text> | /memory forget <id> | \
                     /memory privacy <partition> [reason|--clear] | \
                     /memory retention <partition> <days> | /memory clear <partition>"
                ))),
            },
        }
    }
}

/// Every control below reports what it did to WHICH item, or refuses out
/// loud. There is deliberately no "applied 0 changes" success path: a user who
/// mistypes an id and is told "ok" believes content is gone when it is not.
impl MemoryHandler {
    fn runtime_api(&self, verb: &str) -> Result<Arc<dyn MemoryApi>, SlashError> {
        match self {
            Self::Stub => Err(SlashError::Bad(format!(
                "/memory {verb} needs a live memory store; this session has none"
            ))),
            Self::Runtime { api } => Ok(api.clone()),
        }
    }

    fn controls(
        &self,
        verb: &str,
    ) -> Result<(Arc<dyn MemoryApi>, wcore_memory::MemoryControls), SlashError> {
        let api = self.runtime_api(verb)?;
        let controls = api.controls().ok_or_else(|| {
            SlashError::Bad(format!(
                "/memory {verb}: this memory backend exposes no operator controls"
            ))
        })?;
        Ok((api, controls))
    }

    fn provenance(&self, args: &[String]) -> Result<SlashOutcome, SlashError> {
        let api = self.runtime_api("why")?;
        if args.is_empty() {
            return Err(SlashError::Bad(
                "/memory why <query> — shows where each recalled item came from".to_string(),
            ));
        }
        let query = wcore_memory::v2_types::Query {
            text: args.join(" "),
            tier: Tier::Project,
            ..Default::default()
        };
        let outcome = block_on(api.search_with_provenance(query, AccessToken::MainAgent));
        let (hits, report) = match outcome {
            Ok(v) => v,
            Err(e) => return Ok(handled(format!("/memory why: {e}"))),
        };
        let mut out = format!("/memory why: {} item(s) recalled\n", hits.len());
        for p in &report.provenance {
            out.push_str(&format!(
                "  #{rank} {id} [{partition}/{tier}] via {modality} score={score:.5} \
                 age={age}s {staleness}\n",
                rank = p.rank,
                id = p.id,
                partition = p.partition.as_str(),
                tier = p.tier.as_str(),
                modality = p.modality_label(),
                score = p.fused_score,
                age = p.age_secs,
                staleness = p.staleness.as_str(),
            ));
            for c in &p.contributions {
                out.push_str(&format!(
                    "      {} at rank {}\n",
                    c.modality.as_str(),
                    c.rank
                ));
            }
        }
        if report.provenance.is_empty() && !hits.is_empty() {
            out.push_str("  (this backend cannot report where these came from)\n");
        }
        // Exclusions are the half a user cannot otherwise see.
        for x in &report.exclusions {
            out.push_str(&format!(
                "  EXCLUDED {}{}: {}\n",
                x.partition.as_str(),
                x.id.as_ref()
                    .map(|i| format!(" {i}"))
                    .unwrap_or_else(|| " (whole cell)".to_string()),
                match &x.cause {
                    wcore_memory::ExclusionCause::PrivacyScope { reason } =>
                        format!("privacy scope ({reason})"),
                    wcore_memory::ExclusionCause::RetentionExpired {
                        max_age_secs,
                        age_secs,
                    } => format!("expired: {age_secs}s old, bound is {max_age_secs}s"),
                }
            ));
        }
        Ok(handled(out))
    }

    fn correct(&self, args: &[String]) -> Result<SlashOutcome, SlashError> {
        let (_api, controls) = self.controls("correct")?;
        let (id, rest) = args
            .split_first()
            .ok_or_else(|| SlashError::Bad("/memory correct <id> <corrected text>".to_string()))?;
        if rest.is_empty() {
            return Err(SlashError::Bad(
                "/memory correct <id> <corrected text> — the corrected text is required"
                    .to_string(),
            ));
        }
        match controls.correct_episode(
            &AccessToken::MainAgent,
            Tier::Project,
            id,
            &rest.join(" "),
            "operator",
        ) {
            Ok(r) => Ok(handled(format!(
                "/memory correct: {} corrected in {}/{}",
                r.id,
                r.partition.as_str(),
                r.tier.as_str()
            ))),
            Err(e) => Ok(handled(format!("/memory correct refused: {e}"))),
        }
    }

    fn forget(&self, args: &[String]) -> Result<SlashOutcome, SlashError> {
        let (_api, controls) = self.controls("forget")?;
        let id = args
            .first()
            .ok_or_else(|| SlashError::Bad("/memory forget <id>".to_string()))?;
        match controls.forget_episode(&AccessToken::MainAgent, Tier::Project, id, "operator") {
            Ok(r) => Ok(handled(format!(
                "/memory forget: {} removed from {}/{} and recorded in the changelog",
                r.id,
                r.partition.as_str(),
                r.tier.as_str()
            ))),
            Err(e) => Ok(handled(format!("/memory forget refused: {e}"))),
        }
    }

    fn privacy(&self, args: &[String]) -> Result<SlashOutcome, SlashError> {
        let (_api, controls) = self.controls("privacy")?;
        let (name, rest) = args.split_first().ok_or_else(|| {
            SlashError::Bad(
                "/memory privacy <partition> [reason] | /memory privacy <partition> --clear"
                    .to_string(),
            )
        })?;
        let partition = parse_partition(name).map_err(SlashError::Bad)?;
        if rest.first().map(|s| s.as_str()) == Some("--clear") {
            return match controls.clear_privacy_scope(
                &AccessToken::MainAgent,
                partition,
                Tier::Project,
                "operator",
            ) {
                Ok(true) => Ok(handled(format!(
                    "/memory privacy: {} is no longer excluded from recall",
                    partition.as_str()
                ))),
                Ok(false) => Ok(handled(format!(
                    "/memory privacy: {} was not excluded",
                    partition.as_str()
                ))),
                Err(e) => Ok(handled(format!("/memory privacy refused: {e}"))),
            };
        }
        let reason = if rest.is_empty() {
            "operator request".to_string()
        } else {
            rest.join(" ")
        };
        match controls.set_privacy_scope(
            &AccessToken::MainAgent,
            partition,
            Tier::Project,
            &reason,
            "operator",
        ) {
            Ok(s) => Ok(handled(format!(
                "/memory privacy: {}/{} excluded from recall ({})",
                s.partition.as_str(),
                s.tier.as_str(),
                s.reason
            ))),
            Err(e) => Ok(handled(format!("/memory privacy refused: {e}"))),
        }
    }

    fn retention(&self, args: &[String]) -> Result<SlashOutcome, SlashError> {
        let (_api, controls) = self.controls("retention")?;
        let name = args
            .first()
            .ok_or_else(|| SlashError::Bad("/memory retention <partition> <days>".to_string()))?;
        let partition = parse_partition(name).map_err(SlashError::Bad)?;
        let days: i64 = args
            .get(1)
            .ok_or_else(|| SlashError::Bad("/memory retention <partition> <days>".to_string()))?
            .parse()
            .map_err(|_| {
                SlashError::Bad("/memory retention: <days> must be a number".to_string())
            })?;
        match controls.set_retention(
            &AccessToken::MainAgent,
            partition,
            Tier::Project,
            days.saturating_mul(86_400),
            "operator",
        ) {
            Ok(b) => Ok(handled(format!(
                "/memory retention: {}/{} bounded to {} day(s); older items are reported expired, not deleted",
                b.partition.as_str(),
                b.tier.as_str(),
                days
            ))),
            Err(e) => Ok(handled(format!("/memory retention refused: {e}"))),
        }
    }
}

fn handled(output: String) -> SlashOutcome {
    SlashOutcome::Handled {
        output: Some(output),
    }
}

impl MemoryHandler {
    fn show(&self, _partition: Option<&str>) -> Result<SlashOutcome, SlashError> {
        match self {
            Self::Stub => Ok(SlashOutcome::Handled {
                output: Some(
                    "/memory show: not yet routed to wcore_memory in v0.7.0; use \
                     `wayland-core --memory-show <session-id>` from the CLI."
                        .to_string(),
                ),
            }),
            Self::Runtime { api } => Ok(SlashOutcome::Handled {
                output: Some(runtime_show(api.clone())),
            }),
        }
    }

    fn clear(&self, partition: Option<&str>) -> Result<SlashOutcome, SlashError> {
        let partition_name = partition.ok_or_else(|| {
            SlashError::Bad(
                "/memory clear requires a partition (working / episodic / semantic / procedural / core)"
                    .to_string(),
            )
        })?;
        match self {
            Self::Stub => Ok(SlashOutcome::Handled {
                output: Some(format!(
                    "[noop] would clear partition '{partition_name}' (confirmation prompt arrives in 3.C.4)"
                )),
            }),
            Self::Runtime { api } => {
                let partition_enum = parse_partition(partition_name).map_err(SlashError::Bad)?;
                Ok(SlashOutcome::Handled {
                    output: Some(runtime_clear(api.clone(), partition_enum)),
                })
            }
        }
    }
}

fn parse_partition(name: &str) -> Result<Partition, String> {
    match name {
        "working" => Ok(Partition::Working),
        "episodic" => Ok(Partition::Episodic),
        "semantic" => Ok(Partition::Semantic),
        "procedural" => Ok(Partition::Procedural),
        "core" => Ok(Partition::Core),
        other => Err(format!(
            "unknown partition '{other}' (valid: working / episodic / semantic / procedural / core)"
        )),
    }
}

/// Synchronous wrapper around an async MemoryApi call. The slash handler
/// surface is sync (the `SlashHandler::invoke` signature); production
/// callers always run inside a tokio runtime (CLI / engine session), so
/// `Handle::current().block_on` is safe. Tests construct an `#[tokio::test]`
/// runtime and call us through `tokio::task::block_in_place` only when
/// they need to.
fn block_on<F, T>(f: F) -> T
where
    F: std::future::Future<Output = T>,
{
    // Use `tokio::task::block_in_place` so we don't deadlock the runtime
    // when invoked from inside a multi-thread tokio context. For the
    // current-thread runtime (single-threaded tests) we fall back to a
    // fresh handle-blocking call.
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            // Multi-thread runtime: block_in_place is safe; current-thread
            // runtime: `block_in_place` panics, so fall through.
            if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread {
                tokio::task::block_in_place(|| handle.block_on(f))
            } else {
                // Current-thread runtime — spawn on the same handle as a
                // detached future via `futures::executor::block_on` so we
                // don't recursively call `block_on` on a single-threaded
                // executor (which panics).
                futures::executor::block_on(f)
            }
        }
        Err(_) => futures::executor::block_on(f),
    }
}

fn runtime_show(api: Arc<dyn MemoryApi>) -> String {
    let mut out = String::new();
    out.push_str("Memory partitions (procedural / core / per-partition counts)\n");

    // Procedural at Project tier — same view `run_memory_show` produces.
    let procs_result = block_on(api.list_procedures(Tier::Project, AccessToken::System));
    match procs_result {
        Ok(procs) => {
            out.push_str(&format!(
                "  Procedural [project]: {} entries\n",
                procs.len()
            ));
            for p in procs.iter().take(10) {
                out.push_str(&format!(
                    "    - {name} [{status}] uses={success}/{total}\n",
                    name = p.name,
                    status = p.status.as_str(),
                    success = p.success_count,
                    total = p.use_count
                ));
            }
            if procs.len() > 10 {
                out.push_str(&format!("    ... +{} more\n", procs.len() - 10));
            }
        }
        Err(e) => {
            out.push_str(&format!("  Procedural [project]: error: {e}\n"));
        }
    }

    // User model (Core).
    let user_model_result = block_on(api.user_model(AccessToken::System));
    match user_model_result {
        Ok(um) => {
            out.push_str(&format!(
                "  Core (user_model): {} entries\n",
                um.entries.len()
            ));
            for entry in um.entries.iter().take(10) {
                out.push_str(&format!("    - {} = {}\n", entry.key, entry.value));
            }
            if um.entries.len() > 10 {
                out.push_str(&format!("    ... +{} more\n", um.entries.len() - 10));
            }
        }
        Err(e) => {
            out.push_str(&format!("  Core (user_model): error: {e}\n"));
        }
    }

    out
}

fn runtime_clear(api: Arc<dyn MemoryApi>, partition: Partition) -> String {
    // For each valid (partition, tier) combo, attempt the clear. Bulk-clear
    // all tiers in one go — the slash-command UX expects "clear this
    // partition" not "clear at tier X".
    let mut total: usize = 0;
    let mut per_tier: Vec<(Tier, std::result::Result<usize, String>)> = Vec::new();

    for (p, t) in wcore_memory::v2_types::valid_combinations() {
        if *p != partition {
            continue;
        }
        let result = block_on(api.clear_partition(partition, *t, AccessToken::System));
        match result {
            Ok(n) => {
                total += n;
                per_tier.push((*t, Ok(n)));
            }
            Err(e) => {
                per_tier.push((*t, Err(e.to_string())));
            }
        }
    }

    let mut out = format!(
        "/memory clear {partition_name}: cleared {total} rows\n",
        partition_name = partition.as_str(),
    );
    for (tier, r) in per_tier {
        match r {
            Ok(n) => out.push_str(&format!("  - {} tier: {} deleted\n", tier.as_str(), n)),
            Err(e) => out.push_str(&format!("  - {} tier: error: {}\n", tier.as_str(), e)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash::parse;

    // ------------------------------------------------------------------
    // Back-compat tests — Stub variant preserves the v0.7.0 behaviour
    // ------------------------------------------------------------------

    #[test]
    fn stub_show_default_handled() {
        let inv = parse("/memory show").unwrap();
        let out = MemoryHandler::Stub.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!("expected Handled");
        };
        assert!(s.contains("not yet routed"));
    }

    #[test]
    fn stub_clear_requires_partition() {
        let inv = parse("/memory clear").unwrap();
        assert!(matches!(
            MemoryHandler::Stub.invoke(&inv),
            Err(SlashError::Bad(_))
        ));
    }

    #[test]
    fn stub_clear_with_partition_handled() {
        let inv = parse("/memory clear procedural").unwrap();
        let out = MemoryHandler::Stub.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!();
        };
        assert!(s.contains("procedural"));
    }

    #[test]
    fn stub_unknown_subcommand_errors() {
        let inv = parse("/memory destroy").unwrap();
        assert!(matches!(
            MemoryHandler::Stub.invoke(&inv),
            Err(SlashError::Bad(_))
        ));
    }

    #[test]
    fn default_constructs_stub() {
        let h = MemoryHandler::default();
        assert!(matches!(h, MemoryHandler::Stub));
    }

    // ------------------------------------------------------------------
    // Runtime variant — exercises the real wcore_memory surface
    // ------------------------------------------------------------------

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn runtime_show_reaches_memory_api() {
        // NullMemory is a concrete impl that participates in the same
        // trait but returns empty collections. Sufficient to prove the
        // runtime arm reaches the api surface (not the stub string).
        let api: Arc<dyn MemoryApi> = Arc::new(wcore_memory::NullMemory);
        let handler = MemoryHandler::Runtime { api };
        let inv = parse("/memory show").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!("expected Handled");
        };
        // Must NOT contain the stub-mode placeholder.
        assert!(
            !s.contains("not yet routed"),
            "runtime show leaked stub string: {s}"
        );
        // Must contain the runtime header.
        assert!(s.contains("Memory partitions"), "got: {s}");
        assert!(s.contains("Procedural"), "got: {s}");
        assert!(s.contains("Core"), "got: {s}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn runtime_clear_invokes_memory_api() {
        let api: Arc<dyn MemoryApi> = Arc::new(wcore_memory::NullMemory);
        let handler = MemoryHandler::Runtime { api };
        let inv = parse("/memory clear procedural").unwrap();
        let out = handler.invoke(&inv).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = out else {
            panic!("expected Handled");
        };
        // Must NOT contain the stub-mode placeholder.
        assert!(!s.contains("noop"), "runtime clear leaked stub string: {s}");
        assert!(s.contains("/memory clear procedural"), "got: {s}");
        // NullMemory returns Ok(0) for every (partition, tier) combo.
        assert!(s.contains("cleared 0 rows"), "got: {s}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn runtime_clear_unknown_partition_errors() {
        let api: Arc<dyn MemoryApi> = Arc::new(wcore_memory::NullMemory);
        let handler = MemoryHandler::Runtime { api };
        let inv = parse("/memory clear bogus").unwrap();
        assert!(matches!(handler.invoke(&inv), Err(SlashError::Bad(_))));
    }

    #[test]
    fn parse_partition_maps_all_five() {
        for name in ["working", "episodic", "semantic", "procedural", "core"] {
            assert!(parse_partition(name).is_ok(), "{name} should parse");
        }
        assert!(parse_partition("collaboration").is_err());
    }

    // ------------------------------------------------------------------
    // F23-03 control surface
    // ------------------------------------------------------------------

    #[test]
    fn stub_refuses_every_control_instead_of_reporting_success() {
        // The Stub has no store. Reporting "forgotten" against no store is
        // the one failure these commands must never have.
        for line in [
            "/memory forget ep-1",
            "/memory correct ep-1 new text",
            "/memory privacy episodic",
            "/memory retention episodic 7",
            "/memory why aardvark",
        ] {
            let inv = parse(line).unwrap();
            let err = MemoryHandler::Stub.invoke(&inv);
            assert!(
                matches!(err, Err(SlashError::Bad(_))),
                "{line} must refuse on the Stub, got {err:?}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn runtime_without_controls_says_so_rather_than_claiming_success() {
        // NullMemory implements MemoryApi but exposes no controls.
        let api: Arc<dyn MemoryApi> = Arc::new(wcore_memory::NullMemory);
        let handler = MemoryHandler::Runtime { api };
        let inv = parse("/memory forget ep-1").unwrap();
        let out = handler.invoke(&inv);
        assert!(
            matches!(out, Err(SlashError::Bad(ref m)) if m.contains("no operator controls")),
            "got {out:?}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn controls_reach_a_real_store_and_report_the_item_they_acted_on() {
        let mem = wcore_memory::open_for_test(std::path::Path::new("."))
            .await
            .unwrap();
        let api: Arc<dyn MemoryApi> = Arc::new(mem);
        let controls = api.controls().expect("a real store must expose controls");

        // Seed one episode through the same store the controls act on.
        let ep = wcore_memory::v2_types::Episode {
            id: wcore_memory::v2_types::EpisodeId::new(),
            tier: Tier::Project,
            ts: 0,
            episode_type: "note".into(),
            summary: "the aardvark is blue".into(),
            atomic_facts: vec![],
            source: "test".into(),
            source_product: "test".into(),
            session_id: None,
            project_root: None,
            decay_score: 1.0,
            status: wcore_memory::v2_types::EpisodeStatus::Active,
        };
        let id = api
            .record_episode(ep, AccessToken::MainAgent)
            .await
            .unwrap()
            .0
            .to_string();

        let handler = MemoryHandler::Runtime { api: api.clone() };

        let inv = parse(&format!("/memory correct {id} the aardvark is brown")).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = handler.invoke(&inv).unwrap() else {
            panic!("expected Handled");
        };
        assert!(s.contains(&id), "the correction must name the item: {s}");
        assert!(!s.contains("refused"), "{s}");

        let inv = parse(&format!("/memory forget {id}")).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = handler.invoke(&inv).unwrap() else {
            panic!("expected Handled");
        };
        assert!(s.contains(&id), "the forget must name the item: {s}");
        assert!(s.contains("changelog"), "{s}");

        // Forgetting the same id again must NOT report success.
        let inv = parse(&format!("/memory forget {id}")).unwrap();
        let SlashOutcome::Handled { output: Some(s) } = handler.invoke(&inv).unwrap() else {
            panic!("expected Handled");
        };
        assert!(
            s.contains("refused") && s.contains("not found"),
            "a second forget must refuse, not report success: {s}"
        );

        // Privacy scope round-trips through the surface.
        let inv = parse("/memory privacy episodic medical notes").unwrap();
        let SlashOutcome::Handled { output: Some(s) } = handler.invoke(&inv).unwrap() else {
            panic!();
        };
        assert!(s.contains("excluded from recall"), "{s}");
        assert!(
            controls
                .privacy_scope(wcore_memory::v2_types::Partition::Episodic, Tier::Project)
                .unwrap()
                .is_some()
        );

        let inv = parse("/memory privacy episodic --clear").unwrap();
        let SlashOutcome::Handled { output: Some(s) } = handler.invoke(&inv).unwrap() else {
            panic!();
        };
        assert!(s.contains("no longer excluded"), "{s}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 1)]
    async fn a_privacy_scope_is_reported_as_an_exclusion_not_as_an_empty_result() {
        // The difference between "nothing matched" and "you excluded this" is
        // the entire point of reporting exclusions.
        let mem = wcore_memory::open_for_test(std::path::Path::new("."))
            .await
            .unwrap();
        let api: Arc<dyn MemoryApi> = Arc::new(mem);
        let handler = MemoryHandler::Runtime { api: api.clone() };

        let inv = parse("/memory privacy episodic sealed").unwrap();
        handler.invoke(&inv).unwrap();

        let inv = parse("/memory why aardvark").unwrap();
        let SlashOutcome::Handled { output: Some(s) } = handler.invoke(&inv).unwrap() else {
            panic!();
        };
        assert!(s.contains("EXCLUDED"), "the scope must be visible: {s}");
        assert!(s.contains("sealed"), "the reason must be visible: {s}");
    }
}
