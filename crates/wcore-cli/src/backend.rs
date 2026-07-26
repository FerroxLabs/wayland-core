//! `wayland-core backend` — the F25 execution-backend operator surface.
//!
//! This is the thing a human actually runs, and it is the ONLY surface the
//! phase's live exercise is allowed to drive. Success Criterion 1 is closed by
//! running the shipped binary with an exact invocation and observing an exact
//! outcome on a named host — not by calling a library from a test. Phase 20A
//! drove fourteen native acceptance targets to green and nobody ever launched
//! the binary; this surface exists so that cannot happen again here.

use std::path::PathBuf;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};

use wcore_exec_backend::conformance::{reference_budget, reference_task};
use wcore_exec_backend::contract::ExecutionTask;
use wcore_exec_backend::receipt::ExecutionReceipt;
use wcore_exec_backend::{reference_backend_named, reference_backends};

#[derive(Args, Debug)]
pub struct BackendArgs {
    #[command(subcommand)]
    pub cmd: BackendCmd,
}

#[derive(Subcommand, Debug)]
pub enum BackendCmd {
    /// List every reference backend with its availability, the probe that
    /// established it, and its declared capabilities.
    List {
        /// Emit machine-readable JSON instead of the operator table.
        #[arg(long)]
        json: bool,
    },
    /// Probe one backend and print the result plus its lifecycle health.
    Probe { name: String },
    /// Run a task definition on one backend and write its receipt.
    Run {
        #[arg(long)]
        backend: String,
        /// A task definition JSON file. Omit to use the built-in
        /// deterministic reference task — the one the equivalence proof uses.
        #[arg(long)]
        task: Option<PathBuf>,
        #[arg(long = "receipt-out")]
        receipt_out: PathBuf,
    },
    /// Cancel a live task by id, from any process, and report what cleanup
    /// was observed.
    Cancel {
        #[arg(long = "task-id")]
        task_id: String,
        /// Restrict the cancellation to one backend. Without this every
        /// backend that claims the task is asked.
        #[arg(long)]
        backend: Option<String>,
    },
    /// Enumerate surfaces still carrying a task nonce, per backend.
    Orphans {
        #[arg(long)]
        nonce: String,
    },
    /// Receipt operations.
    Receipt {
        #[command(subcommand)]
        cmd: ReceiptCmd,
    },
    /// Diff two or more receipts of the SAME task and report equivalence.
    Diff {
        /// Two or more receipt files.
        receipts: Vec<PathBuf>,
    },
}

#[derive(Subcommand, Debug)]
pub enum ReceiptCmd {
    /// Verify a receipt file's integrity and internal consistency.
    Verify { path: PathBuf },
}

pub async fn run(args: BackendArgs) -> Result<()> {
    match args.cmd {
        BackendCmd::List { json } => list(json).await,
        BackendCmd::Probe { name } => probe(&name).await,
        BackendCmd::Run {
            backend,
            task,
            receipt_out,
        } => execute(&backend, task.as_deref(), &receipt_out).await,
        BackendCmd::Cancel { task_id, backend } => cancel(&task_id, backend.as_deref()).await,
        BackendCmd::Orphans { nonce } => orphans(&nonce).await,
        BackendCmd::Receipt {
            cmd: ReceiptCmd::Verify { path },
        } => verify_receipt(&path),
        BackendCmd::Diff { receipts } => diff(&receipts),
    }
}

async fn list(json: bool) -> Result<()> {
    let backends = reference_backends(reference_budget())?;
    let mut rows = Vec::new();
    for reference in &backends {
        let capabilities = reference.backend.capabilities();
        let availability = reference.backend.availability().await;
        rows.push(serde_json::json!({
            "backend": capabilities.backend_id,
            "kind": capabilities.kind,
            "version": capabilities.version,
            "available": availability.available,
            "probe_basis": availability.probe,
            "probe_detail": availability.detail,
            "capabilities": {
                "artifact_transfer": capabilities.supports_artifact_transfer,
                "cancellation": capabilities.supports_cancellation,
                "hibernation": capabilities.supports_hibernation,
                "secret_channel": capabilities.secret_channel,
                "limits": capabilities.limits,
            },
        }));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    println!(
        "{:<10} {:<10} {:<12} {:<22} {}",
        "BACKEND", "KIND", "AVAILABLE", "PROBE BASIS", "DETAIL"
    );
    for row in &rows {
        println!(
            "{:<10} {:<10} {:<12} {:<22} {}",
            row["backend"].as_str().unwrap_or("?"),
            row["kind"].as_str().unwrap_or("?"),
            if row["available"].as_bool().unwrap_or(false) {
                "yes"
            } else {
                "NO"
            },
            row["probe_basis"].as_str().unwrap_or("?"),
            row["probe_detail"].as_str().unwrap_or("")
        );
    }
    println!(
        "\nAvailability is answered from a real probe in every row — the container backend from a \
         daemon ping rather than socket presence, the ssh backend from a real handshake, the \
         cloud backend from an authenticated API call. A backend with no credential reports NO \
         and names what is missing; it never falls back to another backend."
    );
    Ok(())
}

async fn probe(name: &str) -> Result<()> {
    let reference = reference_backend_named(name, reference_budget())?
        .ok_or_else(|| anyhow!("unknown backend '{name}' (try: local, container, ssh, cloud)"))?;
    let availability = reference.backend.availability().await;
    println!("backend:       {name}");
    println!("available:     {}", availability.available);
    println!("probe basis:   {:?}", availability.probe);
    println!("probe detail:  {}", availability.detail);
    match reference.backend.health().await {
        Ok(health) => {
            println!("healthy:       {}", health.healthy);
            println!("live tasks:    {}", health.live_tasks);
            println!("health detail: {}", health.detail);
        }
        Err(e) => println!("health:        UNAVAILABLE — {e}"),
    }
    Ok(())
}

fn load_task(path: Option<&std::path::Path>, backend: &str) -> Result<ExecutionTask> {
    match path {
        Some(path) => {
            let bytes = std::fs::read(path)
                .with_context(|| format!("reading task definition {}", path.display()))?;
            let task: ExecutionTask = serde_json::from_slice(&bytes)
                .with_context(|| format!("parsing task definition {}", path.display()))?;
            task.validate()?;
            Ok(task)
        }
        None => Ok(reference_task(
            &format!("ref-{backend}"),
            &format!("ref-nonce-{backend}"),
            reference_budget(),
        )),
    }
}

async fn execute(backend: &str, task_path: Option<&std::path::Path>, out: &PathBuf) -> Result<()> {
    let reference = reference_backend_named(backend, reference_budget())?
        .ok_or_else(|| anyhow!("unknown backend '{backend}' (try: local, container, ssh, cloud)"))?;
    let task = load_task(task_path, backend)?;

    let availability = reference.backend.availability().await;
    if !availability.available {
        // Fail closed and loudly. A run that quietly fell back to another
        // backend would produce a receipt that lies about where work ran.
        bail!(
            "backend '{backend}' is unavailable and this command does NOT fall back: {} (probe {:?})",
            availability.detail,
            availability.probe
        );
    }

    let receipt = reference.backend.execute(&task).await?;
    receipt
        .verify(&reference.identity, &reference.verifying_key)
        .context("the backend emitted a receipt that does not verify against its own identity")?;
    let json = serde_json::to_string_pretty(&receipt)?;
    std::fs::write(out, &json).with_context(|| format!("writing receipt to {}", out.display()))?;

    println!("task:        {}", receipt.body.task.task_id);
    println!("backend:     {}", receipt.body.backend.backend_id);
    println!("transport:   {:?} via {}", receipt.body.transport.kind, receipt.body.transport.endpoint);
    println!("terminal:    {:?}", receipt.body.terminal);
    println!("input sha:   {}", receipt.body.task.input_sha256);
    println!("workspace:   {}", receipt.body.task.workspace_sha256);
    if let Some(artifact) = &receipt.body.artifact {
        println!("artifact:    {} {} ({} bytes)", artifact.name, artifact.sha256, artifact.bytes);
    }
    println!("hibernation: {:?}", receipt.body.hibernation);
    println!("wall ms:     {}", receipt.body.timing.wall_ms);
    println!("receipt:     {}", out.display());
    Ok(())
}

async fn cancel(task_id: &str, backend: Option<&str>) -> Result<()> {
    let backends = reference_backends(reference_budget())?;
    let mut observed = false;
    let mut errors = Vec::new();
    for reference in &backends {
        let id = &reference.backend.capabilities().backend_id;
        if let Some(only) = backend {
            if only != id {
                continue;
            }
        }
        match reference.backend.cancel(task_id).await {
            Ok(observation) => {
                observed = true;
                println!("backend:  {}", observation.backend_id);
                println!("task:     {}", observation.task_id);
                println!("method:   {}", observation.method);
                if observation.is_clean() {
                    println!("residual: none — the cleanup was verified by re-enumeration");
                } else {
                    println!("residual: {} ORPHAN(S)", observation.residual.len());
                    for item in &observation.residual {
                        println!("  - {item}");
                    }
                }
            }
            Err(e) => errors.push(format!("{id}: {e}")),
        }
    }
    if !observed {
        bail!("no backend owns task '{task_id}': {}", errors.join("; "));
    }
    Ok(())
}

async fn orphans(nonce: &str) -> Result<()> {
    let backends = reference_backends(reference_budget())?;
    let mut unscannable = 0usize;
    let mut found = 0usize;
    for reference in &backends {
        let scan = reference.backend.scan_orphans(nonce).await?;
        println!(
            "{:<10} enumerated={:<5} found={} via {}",
            scan.backend_id,
            scan.enumerated,
            scan.found.len(),
            scan.method
        );
        for item in &scan.found {
            println!("           - {item}");
        }
        if !scan.enumerated {
            unscannable += 1;
        }
        found += scan.found.len();
    }
    println!(
        "\n{found} orphan(s) found; {unscannable} surface(s) could NOT be enumerated. An \
         un-enumerated surface is not a clean surface — a scan that could not run must never be \
         read as zero orphans."
    );
    Ok(())
}

fn verify_receipt(path: &PathBuf) -> Result<()> {
    let bytes =
        std::fs::read(path).with_context(|| format!("reading receipt {}", path.display()))?;
    let receipt: ExecutionReceipt = serde_json::from_slice(&bytes)
        .with_context(|| format!("parsing receipt {}", path.display()))?;
    receipt.verify_integrity_only()?;
    println!("receipt:   {}", path.display());
    println!("schema:    {} v{}", receipt.schema, receipt.schema_version);
    println!("backend:   {}", receipt.body.backend.backend_id);
    println!("key id:    {}", receipt.body.backend.key_id);
    println!("terminal:  {:?}", receipt.body.terminal);
    println!("INTEGRITY: OK — body digest, event ordering, single terminal event and internal consistency all hold.");
    println!(
        "IDENTITY:  NOT ESTABLISHED by this command. A receipt cannot authenticate itself: \
         verifying identity requires a verifying key the caller already pinned, which a receipt \
         file does not carry. Use the conformance harness or a caller holding the pinned key."
    );
    Ok(())
}

fn diff(paths: &[PathBuf]) -> Result<()> {
    if paths.len() < 2 {
        bail!("a diff needs at least two receipts");
    }
    let mut receipts = Vec::new();
    for path in paths {
        let bytes =
            std::fs::read(path).with_context(|| format!("reading receipt {}", path.display()))?;
        let receipt: ExecutionReceipt = serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing receipt {}", path.display()))?;
        receipt.verify_integrity_only()?;
        receipts.push(receipt);
    }

    let (equivalent, differing) = wcore_exec_backend::normalized_equivalence(&receipts);
    println!("receipts compared: {}", receipts.len());
    for receipt in &receipts {
        println!(
            "  {:<10} transport={:<10} endpoint={} wall_ms={}",
            receipt.body.backend.backend_id,
            receipt.body.transport.kind.as_str(),
            receipt.body.transport.endpoint,
            receipt.body.timing.wall_ms
        );
    }
    println!("\nEXPECTED-DIVERGENT fields (excluded from the normalized body by design):");
    for receipt in &receipts {
        for (name, value) in receipt.body.divergent_fields() {
            println!("  {:<10} {:<22} {}", receipt.body.backend.backend_id, name, value);
        }
    }
    println!("\nNORMALIZED DIFF: {}", if equivalent { "EQUIVALENT" } else { "DIVERGENT" });
    if !equivalent {
        println!("differing normalized fields: {}", differing.join(", "));
        println!(
            "Each of these is a FINDING, not an expected divergence: the normalized body carries \
             only what four backends running the same task must agree on."
        );
        bail!("receipts are not equivalent");
    }
    println!(
        "The four normalized bodies agree on task digests, resource budget, backend ceiling, \
         event ordering and content digests, artifact digest, terminal status, exposed-secret \
         names and egress decision."
    );
    Ok(())
}
