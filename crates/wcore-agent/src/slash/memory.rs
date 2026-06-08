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
        "Inspect or clear memory partitions."
    }
    fn invoke(&self, invocation: &SlashInvocation) -> Result<SlashOutcome, SlashError> {
        match invocation.args.split_first() {
            None => self.show(None),
            Some((first, rest)) => match first.as_str() {
                "show" => self.show(rest.first().map(|s| s.as_str())),
                "clear" => self.clear(rest.first().map(|s| s.as_str())),
                other => Err(SlashError::Bad(format!(
                    "/memory: unknown sub-action '{other}'. Try: /memory show [partition] | /memory clear <partition>"
                ))),
            },
        }
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
}
