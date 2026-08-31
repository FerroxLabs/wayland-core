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

/// The built-in deterministic reference task's identity. Fixed, not derived
/// from the backend, because Success Criterion 1 is an EQUIVALENCE claim about
/// one task and not a reach claim about four.
const REFERENCE_TASK_ID: &str = "f25-reference";
const REFERENCE_TASK_NONCE: &str = "f25-reference-nonce";

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
    /// Enumerate leftover surfaces, per backend.
    ///
    /// WITHOUT `--nonce` this is the UNSCOPED scan (#366): every surface this
    /// product created, whichever run created it, with the ones this process
    /// has no live record of called out. That is the only form that can see a
    /// leftover from an EARLIER run — a scan for a nonce this process is
    /// already holding is structurally incapable of returning one.
    ///
    /// WITH `--nonce` it is the scoped scan, unchanged.
    Orphans {
        /// Restrict the scan to one nonce. Omit it to ask the question an
        /// operator actually has: "are there wayland surfaces left over from
        /// ANY run".
        #[arg(long)]
        nonce: Option<String>,
    },
    /// F25-05: scan every backend for orphaned execution left behind by a
    /// task, printing the RAW enumeration alongside the count and naming the
    /// reaping mechanism each backend actually relies on.
    Scan {
        /// The task id, which is also its nonce for the reference task.
        #[arg(long = "task-id")]
        task_id: String,
        /// An explicit nonce, when it differs from the task id.
        #[arg(long)]
        nonce: Option<String>,
        #[arg(long)]
        json: bool,
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
    Verify {
        path: PathBuf,
        /// Additionally verify the receipt's ATTESTATION against the named
        /// backend's live verifying key — identity, not merely integrity.
        ///
        /// Without this flag the command is integrity-only BY DESIGN: a receipt
        /// cannot authenticate itself, because the verifying key is not carried
        /// in the file. With it, the caller names the backend whose live key the
        /// receipt must verify against, which turns "this receipt is internally
        /// consistent" into "this receipt was signed by the backend it claims to
        /// come from". A receipt signed by a rotated-out or foreign key fails
        /// HERE, on identity, while its body digest still checks out — which is
        /// the only way to catch a re-signed or misattributed receipt.
        #[arg(long = "against-backend")]
        against_backend: Option<String>,
    },
}

/// Resolve config and arm the process-global egress boundary for this command.
///
/// SECURITY (F25 Criterion 4): `TopCmd::Backend` early-returns from `main`'s
/// dispatch (`main.rs`, the `TopCmd::Backend` arm returns an `ExitCode`
/// directly) hundreds of lines BEFORE main's own `install_egress_policy`
/// chokepoint. Without this call the `cloud` backend's three
/// `wcore_egress::EgressClient::new()` sites — `api_get`, `api_post_json` and
/// `api_delete` in `wcore-exec-backend/src/backends/cloud.rs` — run under
/// `GlobalDefaultPolicy`, which falls back to `EgressDecision::Allow` when
/// nothing is installed. Every outbound request this operator surface makes was
/// therefore ungated, and the receipt said so out loud:
/// `"egress_decision": "allow-all-default-no-policy-installed"`.
///
/// This is the same defect, and the same fix, as `acp.rs` and `workflow.rs`
/// already carry; `backend` was simply not on the list. The install is one-shot
/// and idempotent, so a parent process that already installed a policy wins and
/// this is a no-op.
///
/// A config that fails to resolve must NOT silently leave the boundary down:
/// the whole point is that an unarmed policy is a fail-open. But refusing to run
/// is not the only way to avoid that, and it was the wrong one:
/// `Config::resolve` fails with *"No API key found"* on any machine with no
/// provider credential configured, and `backend` needs no provider at all. The
/// first version of this function propagated that error, which killed the whole
/// operator surface — `list`, `probe`, `run`, `cancel`, `orphans`, `scan` — on
/// such a machine. Measured on `seandesktop` 2026-07-29 by lane
/// `25-c4-windows`: `backend list` exits 0 with a full table on 0.12.25 and
/// exits 1 with *"No API key found"* on the fixed binary. It was invisible on
/// the Linux proof host only because `/root/.wayland/.env` there injects
/// `ANTHROPIC_API_KEY` into every process.
///
/// Two failure shapes, deliberately handled differently, because collapsing
/// them loses the operator's allowlist:
///
/// - [`wcore_config::config::MissingApiKey`] is documented in `wcore-config` as
///   a *recoverable "needs setup"* condition rather than a config error — the
///   TOML parsed fine, only the provider credential is absent. Falling back to
///   [`Config::default`] here would silently discard the operator's real
///   `[security] egress_allow`, so a host with no provider key could not
///   allowlist anything for `backend` at all. Measured: with that fallback, an
///   `egress_allow = ["api.machines.dev"]` config still DENIED. Instead we
///   re-resolve with a sentinel in `api_key`, which satisfies the provider gate
///   and nothing else — `backend` opens no provider connection on any path, and
///   `resolve` does not persist a CLI-supplied key. The `[security]` block then
///   flows through the ordinary global+project merge, unchanged.
/// - Any OTHER error is a genuine config fault (e.g. a TOML parse failure). We
///   do NOT refuse — that is what killed the surface — but we arm from
///   [`Config::default`], which is `[security] enabled = true` with an EMPTY
///   operator allowlist, i.e. strictly *stricter* than any resolved config
///   could have been, since `egress_allow` only ever adds hosts. Fail-closed,
///   and loud rather than silent.
fn arm_egress_policy() {
    /// Satisfies `resolve`'s provider gate on a command that never speaks to a
    /// provider. Never sent anywhere, never written to the credentials store.
    const NO_PROVIDER_SENTINEL: &str = "unused-by-backend-no-provider-call-on-this-path";

    fn cli_args(api_key: Option<String>) -> wcore_config::config::CliArgs {
        wcore_config::config::CliArgs {
            provider: None,
            api_key,
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

    fn armed_from_defaults(err: &anyhow::Error) -> wcore_config::config::Config {
        tracing::warn!(
            error = %err,
            "could not resolve config to arm the egress boundary for `backend`; \
             arming it from defaults instead — enforcing, with NO operator \
             allowlist entries, so any `[security] egress_allow` you configured \
             is NOT in effect for this command"
        );
        wcore_config::config::Config::default()
    }

    let config = match wcore_config::config::Config::resolve(&cli_args(None)) {
        Ok(config) => config,
        Err(err)
            if err
                .downcast_ref::<wcore_config::config::MissingApiKey>()
                .is_some() =>
        {
            match wcore_config::config::Config::resolve(&cli_args(Some(
                NO_PROVIDER_SENTINEL.to_string(),
            ))) {
                Ok(config) => config,
                Err(err) => armed_from_defaults(&err),
            }
        }
        Err(err) => armed_from_defaults(&err),
    };
    wcore_agent::egress::install_egress_policy(&config);
}

pub async fn run(args: BackendArgs) -> Result<()> {
    arm_egress_policy();
    match args.cmd {
        BackendCmd::List { json } => list(json).await,
        BackendCmd::Probe { name } => probe(&name).await,
        BackendCmd::Run {
            backend,
            task,
            receipt_out,
        } => execute(&backend, task.as_deref(), &receipt_out).await,
        BackendCmd::Cancel { task_id, backend } => cancel(&task_id, backend.as_deref()).await,
        BackendCmd::Orphans { nonce } => orphans(nonce.as_deref()).await,
        BackendCmd::Scan {
            task_id,
            nonce,
            json,
        } => scan(&task_id, nonce.as_deref(), json).await,
        BackendCmd::Receipt {
            cmd:
                ReceiptCmd::Verify {
                    path,
                    against_backend,
                },
        } => verify_receipt(&path, against_backend.as_deref()),
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
        "{:<10} {:<10} {:<12} {:<22} DETAIL",
        "BACKEND", "KIND", "AVAILABLE", "PROBE BASIS"
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
        // THE SAME task, byte for byte, on every backend — including its id
        // and nonce. Suffixing the id per backend would make four runs four
        // DIFFERENT tasks, and the equivalence diff would then be comparing
        // things that were never supposed to be equal. That exact mistake was
        // made and caught by the first live run on 2026-07-26, which reported
        // DIVERGENT on `task` and `events` while every content digest matched.
        None => {
            let _ = backend;
            Ok(reference_task(
                REFERENCE_TASK_ID,
                REFERENCE_TASK_NONCE,
                reference_budget(),
            ))
        }
    }
}

async fn execute(backend: &str, task_path: Option<&std::path::Path>, out: &PathBuf) -> Result<()> {
    let reference = reference_backend_named(backend, reference_budget())?.ok_or_else(|| {
        anyhow!("unknown backend '{backend}' (try: local, container, ssh, cloud)")
    })?;
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
    println!(
        "transport:   {:?} via {}",
        receipt.body.transport.kind, receipt.body.transport.endpoint
    );
    println!("terminal:    {:?}", receipt.body.terminal);
    println!("input sha:   {}", receipt.body.task.input_sha256);
    println!("workspace:   {}", receipt.body.task.workspace_sha256);
    if let Some(artifact) = &receipt.body.artifact {
        println!(
            "artifact:    {} {} ({} bytes)",
            artifact.name, artifact.sha256, artifact.bytes
        );
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
        if let Some(only) = backend
            && only != id
        {
            continue;
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

async fn orphans(nonce: Option<&str>) -> Result<()> {
    match nonce {
        Some(nonce) => orphans_scoped(nonce).await,
        None => orphans_unscoped().await,
    }
}

/// The UNSCOPED scan (#366 d1/d3). Reports; never reclaims (#366 d6).
async fn orphans_unscoped() -> Result<()> {
    let backends = reference_backends(reference_budget())?;
    let mut indeterminate = 0usize;
    let mut leftovers = 0usize;
    for reference in &backends {
        let scan = reference.backend.scan_all_orphans().await?;
        if let Some(why) = &scan.unsupported_reason {
            println!("{:<10} NO UNSCOPED SCAN — {why}", scan.backend_id);
            indeterminate += 1;
            continue;
        }
        if !scan.enumerated {
            println!(
                "{:<10} COULD NOT ENUMERATE via {} — this is NOT zero orphans",
                scan.backend_id, scan.method
            );
            indeterminate += 1;
            continue;
        }
        println!(
            "{:<10} enumerated={:<5} found={} via {}",
            scan.backend_id,
            scan.enumerated,
            scan.found.len(),
            scan.method
        );
        for item in &scan.found {
            // The distinction is the whole point: a surface this process
            // created is bookkeeping, one it did not is a LEFTOVER.
            let tag = if item.known_to_this_process {
                "live in this process"
            } else {
                "LEFTOVER — no live record in this process"
            };
            println!(
                "           - {} (nonce {}) [{tag}]",
                item.handle, item.nonce
            );
        }
        leftovers += scan.leftovers().count();
    }
    println!(
        "\n{leftovers} leftover surface(s) with no live record in this process; \
         {indeterminate} surface(s) could NOT be scanned without a nonce. An un-enumerated \
         surface is not a clean surface. Nothing here was removed: an unscoped scan holds no \
         claim on what it finds, and a leftover may be a live task in another process on the \
         same daemon."
    );
    Ok(())
}

async fn orphans_scoped(nonce: &str) -> Result<()> {
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

fn verify_receipt(path: &PathBuf, against_backend: Option<&str>) -> Result<()> {
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
    println!(
        "INTEGRITY: OK — body digest, event ordering, single terminal event and internal consistency all hold."
    );
    let Some(name) = against_backend else {
        println!(
            "IDENTITY:  NOT ESTABLISHED by this command. A receipt cannot authenticate itself: \
             verifying identity requires a verifying key the caller already pinned, which a receipt \
             file does not carry. Pass --against-backend <name> to check it against a live \
             backend's key, or use the conformance harness."
        );
        return Ok(());
    };

    // Identity. The caller has named the backend whose live key this receipt
    // must verify against, so we now hold a pinned key and the "a receipt
    // cannot authenticate itself" objection no longer applies. This check is
    // INDEPENDENT of the body digest above: a receipt re-signed by a
    // rotated-out or foreign key has a perfectly intact body and is caught
    // only here.
    let reference = reference_backend_named(name, reference_budget())
        .with_context(|| format!("resolving backend '{name}' to obtain its live verifying key"))?
        .ok_or_else(|| {
            anyhow!("unknown backend '{name}' — cannot establish identity against it")
        })?;
    match receipt.verify(&reference.identity, &reference.verifying_key) {
        Ok(()) => {
            println!(
                "IDENTITY:  OK — the attestation verifies against backend '{name}' (key id {}). \
                 This receipt was signed by the backend it claims to come from.",
                reference.identity.key_id
            );
            Ok(())
        }
        Err(e) => {
            // Fail closed, and say which check failed. A caller must be able to
            // tell "the bytes were altered" from "the signer was not who this
            // receipt claims" — they have different responses.
            bail!(
                "IDENTITY: REFUSED — this receipt does NOT verify against backend '{name}' \
                 (expected key id {}, receipt carries key id {}): {e}. The body digest is \
                 intact, so this is an identity failure, not a tampering failure: the receipt \
                 was signed by a different (rotated-out, foreign or compromised) key.",
                reference.identity.key_id,
                receipt.body.backend.key_id
            )
        }
    }
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
            println!(
                "  {:<10} {:<22} {}",
                receipt.body.backend.backend_id, name, value
            );
        }
    }
    println!(
        "\nNORMALIZED DIFF: {}",
        if equivalent {
            "EQUIVALENT"
        } else {
            "DIVERGENT"
        }
    );
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

/// F25-05: `wayland-core backend scan --task-id <id>`.
///
/// Prints the RAW enumeration alongside the count so an operator can check the
/// scanner's own work, and names each backend's reaping mechanism so nobody has
/// to infer it. A surface that could not be enumerated prints NOT MEASURED —
/// never zero, because "did not look" and "looked and found nothing" are
/// different facts and only one of them is evidence.
async fn scan(task_id: &str, nonce: Option<&str>, json: bool) -> Result<()> {
    let nonce = nonce.unwrap_or(task_id);
    let evidence = wcore_exec_backend::orphan::scan_all(nonce, reference_budget()).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&evidence)?);
    } else {
        println!("orphan scan for task {task_id} (nonce {nonce})");
        for e in &evidence {
            println!();
            println!("  backend    {}", e.backend_id);
            println!("  mechanism  {}", e.mechanism.label());
            println!("  method     {}", e.method);
            match e.orphan_count {
                Some(n) => println!("  count      {n} (MEASURED)"),
                None => println!(
                    "  count      NOT MEASURED — {}",
                    e.unobserved_reason
                        .as_deref()
                        .unwrap_or("no reason recorded")
                ),
            }
            if e.rows.is_empty() {
                println!("  rows       (none)");
            } else {
                for row in &e.rows {
                    println!("  row        {row}");
                }
            }
        }
    }

    // A found orphan is a non-zero exit so this is scriptable as a gate.
    let found: u64 = evidence.iter().filter_map(|e| e.orphan_count).sum();
    let unmeasured = evidence.iter().filter(|e| !e.is_observed()).count();
    println!();
    println!(
        "TOTAL: {found} orphan(s) measured across {} backend(s); {unmeasured} surface(s) NOT measured",
        evidence.len()
    );
    if found > 0 {
        bail!("{found} orphaned execution surface(s) still carry nonce {nonce}");
    }
    Ok(())
}
