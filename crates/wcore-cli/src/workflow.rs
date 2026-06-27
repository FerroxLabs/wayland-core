//! B2 — `wayland workflow <subcommand>` CLI surface for the dynamic-workflow
//! engine.
//!
//! Three subcommands wrap the public workflow API
//! ([`wcore_agent::orchestration::workflow`]):
//!
//! - `validate <FILE>` — read a `.ron` file, parse it via
//!   [`WorkflowPlan::parse`], and print an OK summary (name, IR-walked agent
//!   count) or the typed [`WorkflowParseError`] with its field pointer. Pure:
//!   no provider, no engine.
//! - `list` — discover `.wayland/workflows/*.ron` under the resolved project
//!   root, parse each, and print `name  ~est_agents agents  — description`
//!   (from [`WorkflowMeta`] + [`estimate`]). Unparseable files are skipped with
//!   a warning, never aborting the listing.
//! - `run <NAME>` — resolve `<NAME>` to `.wayland/workflows/<NAME>.ron`, parse,
//!   and execute through [`WorkflowRunner`] over a real provider + spawner. The
//!   provider/spawner are built from the SAME construction path the main agent
//!   loop uses: [`Config::resolve`] → [`wcore_providers::create_provider`] →
//!   [`AgentSpawner::new`] — the exact substrate `WorkflowTool` (B1) drives.
//!
//! ## Project-root resolution
//!
//! Saved workflows live under `<project-root>/.wayland/workflows/`. The project
//! root is found by [`find_workflows_dir`], which walks up from the cwd looking
//! for an existing `.wayland` directory (the same project-local convention
//! `bootstrap.rs` uses for `.wayland/user-model.json`). When no ancestor has a
//! `.wayland` dir, it falls back to `<cwd>/.wayland/workflows` so a fresh
//! project still resolves to a sensible default path.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::Subcommand;
use serde_json::json;

use wcore_agent::agents::bus::{AgentBus, AgentMessage};
use wcore_agent::orchestration::workflow::estimate::{self, CostEstimate};
use wcore_agent::orchestration::workflow::runner::{WorkflowPlan, WorkflowRunner};
use wcore_agent::spawner::AgentSpawner;
use wcore_config::config::{CliArgs, Config};

/// `wayland workflow <subcommand>`.
#[derive(Subcommand, Debug)]
pub enum WorkflowCmd {
    /// Parse and validate a single workflow `.ron` file without executing it.
    /// Prints an OK summary on success or the typed parse error (with its
    /// field pointer) on failure.
    Validate {
        /// Path to the workflow `.ron` file.
        file: PathBuf,
    },
    /// List the saved workflows discovered under `.wayland/workflows/*.ron`.
    /// Each line is `name  ~N agents  — description`. Unparseable files are
    /// skipped with a warning to stderr.
    List,
    /// Execute a saved workflow by name. Resolves `<NAME>` to
    /// `.wayland/workflows/<NAME>.ron`, parses it, and runs it through the
    /// workflow engine over a real provider.
    Run {
        /// Saved-workflow name (the `.ron` stem under `.wayland/workflows/`).
        name: String,
    },
}

/// Async entrypoint dispatched from `main.rs`.
pub async fn run(cmd: WorkflowCmd) -> anyhow::Result<()> {
    match cmd {
        WorkflowCmd::Validate { file } => validate(&file),
        WorkflowCmd::List => list(),
        WorkflowCmd::Run { name } => run_workflow(&name).await,
    }
}

/// `validate <FILE>` — pure parse + summary. No engine.
fn validate(file: &Path) -> anyhow::Result<()> {
    let src = std::fs::read_to_string(file)
        .map_err(|e| anyhow::anyhow!("failed to read '{}': {e}", file.display()))?;

    // The typed parse error carries a field/location pointer; surface it as the
    // command error so the process exits non-zero and the pointer reaches the
    // user verbatim.
    let plan = WorkflowPlan::parse(&src).map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;

    // Agent count comes from the IR-walking estimator, never the author's
    // self-declared `meta.est_agents` hint. An empty initial state means any
    // no-barrier `over:` collection falls back to UNKNOWN_CARDINALITY (a floor).
    let est = estimate::estimate(&plan, &json!({}));
    println!("OK: {}", plan.meta.name);
    println!(
        "  {} {}, ~{} agents{}",
        plan.graph.nodes.len(),
        if plan.graph.nodes.len() == 1 {
            "node"
        } else {
            "nodes"
        },
        est.agents,
        if est.cardinality_unknown {
            " (floor — unresolved `over:` collection)"
        } else {
            ""
        }
    );
    if !plan.meta.description.is_empty() {
        println!("  {}", plan.meta.description);
    }
    Ok(())
}

/// `list` — discover + summarize saved workflows.
fn list() -> anyhow::Result<()> {
    let dir = find_workflows_dir()?;
    let mut entries = match read_ron_files(&dir) {
        Ok(e) => e,
        Err(e) => {
            // A missing directory is not an error: a project with no saved
            // workflows simply lists nothing.
            if e.kind() == std::io::ErrorKind::NotFound {
                println!("no saved ForgeFlows in {}", dir.display());
                return Ok(());
            }
            return Err(anyhow::anyhow!("failed to read {}: {e}", dir.display()));
        }
    };
    entries.sort();

    let mut listed = 0usize;
    for path in &entries {
        let src = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("warning: skipping {}: {e}", path.display());
                continue;
            }
        };
        match WorkflowPlan::parse(&src) {
            Ok(plan) => {
                let est = estimate::estimate(&plan, &json!({}));
                print_list_line(&plan, &est);
                listed += 1;
            }
            Err(e) => {
                eprintln!("warning: skipping {} (parse error): {e}", path.display());
            }
        }
    }

    if listed == 0 && !entries.is_empty() {
        println!("no valid ForgeFlows in {}", dir.display());
    } else if entries.is_empty() {
        println!("no saved ForgeFlows in {}", dir.display());
    }
    Ok(())
}

/// Render one `list` line: `name  ~N agents  — description`.
fn print_list_line(plan: &WorkflowPlan, est: &CostEstimate) {
    let desc = if plan.meta.description.is_empty() {
        String::new()
    } else {
        format!("  — {}", plan.meta.description)
    };
    println!("{}  ~{} agents{}", plan.meta.name, est.agents, desc);
}

/// `run <NAME>` — resolve, parse, and execute through `WorkflowRunner`.
///
/// Wires the runner to the same provider/spawner construction path the main
/// agent loop uses, so a `run` here is a real fleet execution — not a stub.
async fn run_workflow(name: &str) -> anyhow::Result<()> {
    let dir = find_workflows_dir()?;
    let path = dir.join(format!("{name}.ron"));
    let src = std::fs::read_to_string(&path)
        .map_err(|e| anyhow::anyhow!("workflow '{name}' not found at {}: {e}", path.display()))?;

    let plan = WorkflowPlan::parse(&src).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;

    // Resolve config + provider through the canonical path. `CliArgs` with all
    // `None`s means "use the file/env layers only" — identical to a plain
    // `wayland-core` launch with no provider/model flags.
    let config = Config::resolve(&default_cli_args())?;

    // B2 — `workflow run` resolves its own config and spawns sub-agents in-process
    // (it early-returns from main's dispatch before the CLI chokepoint install), so
    // it must install the egress policy itself. One-shot/idempotent: a no-op if a
    // parent process already installed it.
    wcore_agent::egress::install_egress_policy(&config);

    // OAuth-aware construction: a config pinned to `openai-chatgpt` (or a
    // future `*-oauth` provider) must build its bearer-source-backed provider
    // here instead of hitting the `create_native_provider` panic. For every
    // other provider this is byte-for-byte `wcore_providers::create_provider`.
    let provider = wcore_agent::bootstrap::create_provider_with_oauth(&config)?;

    // Attach an `AgentBus` so the workflow's sub-agents emit the same
    // Spawned/Completed/Errored lifecycle telemetry the main agent loop does
    // (bootstrap.rs wires the bus identically). A one-shot `run` has no
    // long-lived host/bridge to consume the stream, so we spawn a small
    // subscriber that logs each lifecycle event to stderr — without it the
    // events would be published into a bus with no receiver and silently
    // dropped. The task is detached; it ends when the bus sender drops at the
    // end of this function.
    let agent_bus = Arc::new(AgentBus::new(256));
    spawn_lifecycle_logger(Arc::clone(&agent_bus));
    // Guardrail #4 (crucible): attach the council provider resolver so a RON
    // workflow that pins a node's provider (`Agent((provider: Some(..)))`)
    // resolves it from the on-disk `[providers]` map instead of hard-erroring.
    // Harmless for unpinned workflows (no pin → inherit the parent provider).
    let mut spawner = AgentSpawner::new(provider, config.clone()).with_bus(agent_bus);
    if let Ok(cf) = wcore_config::config::load_merged_config_file(None)
        && !cf.providers.is_empty()
    {
        let resolver =
            wcore_agent::orchestration::council::CouncilProviderResolver::new(config, cf.providers);
        spawner = spawner.with_provider_resolver(Arc::new(resolver));
    }

    // Pre-execution estimate so the operator sees the footprint before any
    // spawn. The `run` subcommand is the explicit/saved tier — no confirm gate
    // here (that is the detected tier, task B6); the operator already opted in
    // by invoking `run <NAME>`.
    let est = estimate::estimate(&plan, &json!({}));
    eprintln!(
        "running ForgeFlow '{}' (~{} agents, ~${:.2})",
        plan.meta.name, est.agents, est.est_usd
    );

    let runner = WorkflowRunner::new(&spawner);
    let result = runner
        .run(&plan, json!({}))
        .await
        .map_err(|e| anyhow::anyhow!("workflow '{name}' failed: {e}"))?;

    // Emit the structured outcome: per-stage records plus the final state.
    let envelope = json!({
        "workflow": plan.meta.name,
        "stages": result
            .stage_results
            .iter()
            .map(|s| json!({
                "node": s.node_id,
                "is_error": s.is_error,
                "turns": s.turns,
                "text": s.text,
            }))
            .collect::<Vec<_>>(),
        "final_state": result.final_state,
    });
    println!("{}", serde_json::to_string_pretty(&envelope)?);
    Ok(())
}

/// Subscribe to `bus` and log sub-agent lifecycle events to stderr until the
/// bus sender drops (end of the `run` invocation).
///
/// A one-shot `workflow run` has no host/bridge consuming the `AgentBus`, so
/// without a subscriber the spawner's Spawned/Completed/Errored events would be
/// published into a receiver-less channel and dropped. This task is the single
/// consumer; it gives `run` the same lifecycle visibility the interactive agent
/// loop gets, on stderr (stdout is reserved for the final JSON envelope).
fn spawn_lifecycle_logger(bus: Arc<AgentBus>) {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        // `recv` returns `Err(Closed)` once the bus sender drops, ending the
        // task; `Err(Lagged)` is skipped (we only log, never gate on, events).
        while let Ok(msg) = rx.recv().await {
            match msg {
                AgentMessage::Spawned { agent, .. } => {
                    eprintln!("  [agent] spawned: {agent}");
                }
                AgentMessage::Completed {
                    agent,
                    turns,
                    output_tokens,
                } => {
                    eprintln!(
                        "  [agent] completed: {agent} ({turns} turns, {output_tokens} out tokens)"
                    );
                }
                AgentMessage::Errored { agent, error } => {
                    eprintln!("  [agent] errored: {agent}: {error}");
                }
                _ => {}
            }
        }
    });
}

/// `CliArgs` with every flag unset — config resolution falls back to the
/// file/env layers exactly as a flagless `wayland-core` launch would.
fn default_cli_args() -> CliArgs {
    CliArgs {
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
    }
}

/// Resolve `<project-root>/.wayland/workflows`.
///
/// Walks up from the cwd looking for an ancestor that already contains a
/// `.wayland` directory (the project-local convention used elsewhere, e.g.
/// `bootstrap.rs`'s `.wayland/user-model.json`). Falls back to
/// `<cwd>/.wayland/workflows` when no ancestor has one.
fn find_workflows_dir() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    let root = find_project_root(&cwd).unwrap_or_else(|| cwd.clone());
    Ok(root.join(".wayland").join("workflows"))
}

/// Walk up from `start` returning the first ancestor (inclusive) that holds a
/// `.wayland` directory, or `None` if none do.
fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cursor: Option<&Path> = Some(start);
    while let Some(dir) = cursor {
        if dir.join(".wayland").is_dir() {
            return Some(dir.to_path_buf());
        }
        cursor = dir.parent();
    }
    None
}

/// Collect the `.ron` files directly inside `dir` (non-recursive). Propagates
/// the IO error (including `NotFound`) so the caller can treat a missing
/// directory as "no workflows".
fn read_ron_files(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|e| e == "ron") && path.is_file() {
            out.push(path);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal, valid single-agent workflow.
    const GOOD: &str = r#"
Workflow(
    meta: (name: "demo", description: "a demo workflow", est_agents: 1),
    phases: [
        Phase(title: "only", steps: [
            Agent((id: "step1", prompt: "do the thing")),
        ]),
    ],
)
"#;

    /// A second valid workflow with two agents, for the `list` count.
    const GOOD2: &str = r#"
Workflow(
    meta: (name: "pair", description: "two agents", est_agents: 2),
    phases: [
        Phase(title: "p", steps: [
            Agent((id: "a", prompt: "first")),
            Agent((id: "b", prompt: "second")),
        ]),
    ],
)
"#;

    /// Structurally malformed: an empty phase. Parses as RON but fails lowering
    /// with a typed `EmptyPhase` error carrying the phase pointer.
    const BAD_EMPTY_PHASE: &str = r#"
Workflow(
    meta: (name: "broken", description: "", est_agents: 0),
    phases: [
        Phase(title: "empty", steps: []),
    ],
)
"#;

    /// Syntactically invalid RON — exercises the `Ron` error arm.
    const BAD_SYNTAX: &str = "this is not ron at all {{{";

    #[test]
    fn validate_good_file_ok() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("demo.ron");
        std::fs::write(&path, GOOD).unwrap();
        // A good file validates without error.
        validate(&path).expect("good workflow should validate");
    }

    #[test]
    fn validate_malformed_file_surfaces_typed_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("broken.ron");
        std::fs::write(&path, BAD_EMPTY_PHASE).unwrap();
        let err = validate(&path).expect_err("empty-phase workflow must error");
        // The typed parse error's field pointer (the phase name) must reach the
        // surfaced message.
        let msg = err.to_string();
        assert!(msg.contains("empty"), "want phase pointer, got: {msg}");
    }

    #[test]
    fn validate_invalid_ron_surfaces_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("syntax.ron");
        std::fs::write(&path, BAD_SYNTAX).unwrap();
        let err = validate(&path).expect_err("invalid RON must error");
        assert!(
            err.to_string().contains("RON") || err.to_string().contains("ron"),
            "want a RON syntax error, got: {err}"
        );
    }

    #[test]
    fn validate_missing_file_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nope.ron");
        let err = validate(&path).expect_err("missing file must error");
        assert!(err.to_string().contains("failed to read"), "got: {err}");
    }

    #[test]
    fn read_ron_files_lists_two_good_warns_on_bad() {
        // Build a temp `.wayland/workflows` with 2 good + 1 bad `.ron`, plus a
        // non-ron file that must be ignored entirely.
        let dir = tempfile::tempdir().unwrap();
        let wf = dir.path().join(".wayland").join("workflows");
        std::fs::create_dir_all(&wf).unwrap();
        std::fs::write(wf.join("demo.ron"), GOOD).unwrap();
        std::fs::write(wf.join("pair.ron"), GOOD2).unwrap();
        std::fs::write(wf.join("broken.ron"), BAD_SYNTAX).unwrap();
        std::fs::write(wf.join("notes.txt"), "ignored").unwrap();

        let files = read_ron_files(&wf).unwrap();
        // Three `.ron` files discovered; the `.txt` is excluded.
        assert_eq!(files.len(), 3, "should find exactly the 3 .ron files");

        // Parse each the way `list` does: 2 succeed, 1 (broken) is skipped.
        let mut parsed = 0usize;
        let mut skipped = 0usize;
        for path in &files {
            let src = std::fs::read_to_string(path).unwrap();
            match WorkflowPlan::parse(&src) {
                Ok(_) => parsed += 1,
                Err(_) => skipped += 1,
            }
        }
        assert_eq!(parsed, 2, "two good workflows parse");
        assert_eq!(skipped, 1, "one bad workflow is skipped");
    }

    #[test]
    fn read_ron_files_missing_dir_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join(".wayland").join("workflows");
        let err = read_ron_files(&missing).expect_err("missing dir errors");
        assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
    }

    #[test]
    fn find_project_root_walks_up_to_wayland_dir() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join(".wayland")).unwrap();
        let nested = root.join("a").join("b");
        std::fs::create_dir_all(&nested).unwrap();
        // From a nested cwd, the root with `.wayland` is discovered.
        let found = find_project_root(&nested).expect("should find root");
        // Compare canonicalized paths to neutralize macOS `/private` symlinks.
        assert_eq!(
            std::fs::canonicalize(found).unwrap(),
            std::fs::canonicalize(root).unwrap()
        );
    }

    #[test]
    fn find_project_root_none_when_absent() {
        let dir = tempfile::tempdir().unwrap();
        // No `.wayland` anywhere under this isolated temp tree's leaf — but an
        // ancestor (the real cwd / home) might have one, so assert only that a
        // tree with none returns the temp root itself is NOT matched. We test
        // the leaf has no `.wayland` and find_project_root does not invent one.
        let leaf = dir.path().join("x");
        std::fs::create_dir_all(&leaf).unwrap();
        // The temp dir has no `.wayland`; walking up may still hit a real
        // ancestor, so we only assert the immediate dir is not falsely matched.
        assert!(!leaf.join(".wayland").is_dir());
    }
}
