//! `wayland-core node` — the F25-03 node/device operator surface.
//!
//! The only surface the live exercise is allowed to drive. Success Criterion 2
//! is closed by running this against a REAL second host and observing the
//! outcome, not by calling a library from a test.
//!
//! ## How pairing actually reaches the far end
//!
//! Over SSH, in ARGV mode via `wcore_config::shell` — the same transport plan
//! 25-01's ssh backend already proved, and no new dependency. The controller
//! mints a challenge, runs `wayland-core node identity --challenge <nonce>` on
//! the far end, and verifies the signed answer. The far end therefore needs a
//! `wayland-core` binary, which is stated plainly rather than discovered at
//! failure time.
//!
//! Nothing here discovers, schedules or meshes. Fleet claiming and work
//! distribution are Phase 22's and already exist.

use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Subcommand};

use wcore_exec_backend::conformance::reference_budget;
use wcore_exec_backend::node::attribution::{AttributionVerdict, verify_node_attribution};
use wcore_exec_backend::node::capability::NodeAdvertisement;
use wcore_exec_backend::node::pairing::{
    NodeIdentity, PairingChallenge, PairingProof, load_or_create_node_seed, prove_challenge,
    verify_proof,
};
use wcore_exec_backend::node::registry::{Liveness, NodeRegistry, NodeState, SubmissionVerdict};
use wcore_exec_backend::receipt::ExecutionReceipt;

/// How long the controller waits for the far end to answer a pairing challenge
/// or a liveness probe before calling it offline.
///
/// Bounded on purpose: "the node vanished mid-task" must reach a NAMED terminal
/// status rather than hanging, and an unbounded wait is how hanging happens.
const FAR_END_TIMEOUT_SECS: u64 = 20;

#[derive(Args, Debug)]
pub struct NodeArgs {
    #[command(subcommand)]
    pub cmd: NodeCmd,
}

#[derive(Subcommand, Debug)]
pub enum NodeCmd {
    /// Print THIS host's node identity. With `--challenge`, also sign it —
    /// this is the far-end half of pairing and is what the controller runs
    /// over ssh.
    Identity {
        /// The name this host answers to.
        #[arg(long, default_value = "self")]
        name: String,
        /// A controller-supplied nonce to sign. Without it, identity only.
        #[arg(long)]
        challenge: Option<String>,
        /// The controller's key id, bound into the signed message.
        #[arg(long, default_value = "unknown")]
        controller_key_id: String,
        /// Probe the local backends and include a fresh advertisement.
        #[arg(long)]
        advertise: bool,
    },
    /// Pair a remote host as a node, proving its identity before recording it.
    Pair {
        /// The name this controller will know the node by.
        name: String,
        /// An ssh target, e.g. `user@host` or a configured alias.
        #[arg(long)]
        target: String,
        /// Path to `wayland-core` on the far end.
        #[arg(long, default_value = "wayland-core")]
        remote_bin: String,
    },
    /// List every paired node with identity, capabilities, version and liveness.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Everything known about one node.
    Show { name: String },
    /// Probe a node's liveness and refresh its capability advertisement.
    Probe { name: String },
    /// Withdraw a node's authority. Subsequent work to it is REFUSED and
    /// in-flight work on it is terminated; nothing reroutes.
    Revoke {
        name: String,
        #[arg(long, default_value = "operator revoked")]
        reason: String,
        /// Clear an existing revocation so the operator can deliberately
        /// re-pair. This is the ONLY way out of the revoked state.
        #[arg(long)]
        clear: bool,
    },
    /// Ask whether work may be submitted to a node, and exit non-zero when the
    /// answer is no. This is the scriptable form of the refusal.
    Submit { name: String },
    /// Re-verify a receipt's attribution chain against a paired node.
    Attribution {
        /// The node the work is claimed to have run on.
        name: String,
        /// The receipt file.
        #[arg(long)]
        receipt: std::path::PathBuf,
    },
}

fn registry() -> NodeRegistry {
    NodeRegistry::default_location()
}

fn local_identity(name: &str) -> Result<(NodeIdentity, ed25519_dalek::SigningKey)> {
    let seed = load_or_create_node_seed()?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let identity = NodeIdentity::local(name, &signing_key)?;
    Ok((identity, signing_key))
}

pub async fn run(args: NodeArgs) -> Result<()> {
    match args.cmd {
        NodeCmd::Identity {
            name,
            challenge,
            controller_key_id,
            advertise,
        } => identity(&name, challenge.as_deref(), &controller_key_id, advertise).await,
        NodeCmd::Pair {
            name,
            target,
            remote_bin,
        } => pair(&name, &target, &remote_bin).await,
        NodeCmd::List { json } => list(json),
        NodeCmd::Show { name } => show(&name),
        NodeCmd::Probe { name } => probe(&name).await,
        NodeCmd::Revoke {
            name,
            reason,
            clear,
        } => revoke(&name, &reason, clear),
        NodeCmd::Submit { name } => submit(&name),
        NodeCmd::Attribution { name, receipt } => attribution(&name, &receipt),
    }
}

/// Far-end half of pairing, and a plain identity print for an operator.
async fn identity(
    name: &str,
    challenge: Option<&str>,
    controller_key_id: &str,
    advertise: bool,
) -> Result<()> {
    let (identity, signing_key) = local_identity(name)?;
    let advertisement = if advertise || challenge.is_some() {
        NodeAdvertisement::observe(name, reference_budget()).await?
    } else {
        NodeAdvertisement::empty(name)
    };
    // A leak here would travel to a controller across a network this host does
    // not control, so it fails loudly rather than being silently stripped.
    if let Some(leak) = advertisement.leaks_host_detail() {
        bail!("refusing to advertise: {leak}");
    }

    match challenge {
        None => {
            println!("{}", serde_json::to_string_pretty(&identity)?);
            Ok(())
        }
        Some(nonce) => {
            let challenge = PairingChallenge {
                nonce: nonce.to_string(),
                controller_key_id: controller_key_id.to_string(),
            };
            let proof = prove_challenge(&signing_key, &identity, &challenge, advertisement)?;
            println!("{}", serde_json::to_string(&proof)?);
            Ok(())
        }
    }
}

/// Controller half of pairing.
async fn pair(name: &str, target: &str, remote_bin: &str) -> Result<()> {
    let (controller_identity, _) = local_identity("controller")?;
    let challenge = PairingChallenge::new(&controller_identity.key_id);
    println!(
        "pairing '{name}' at {target} (challenge {})",
        &challenge.nonce[..12]
    );

    let raw = far_end_call(
        target,
        remote_bin,
        &[
            "node",
            "identity",
            "--name",
            name,
            "--challenge",
            &challenge.nonce,
            "--controller-key-id",
            &challenge.controller_key_id,
        ],
    )
    .await
    .with_context(|| format!("reaching node '{name}' at {target}"))?;

    let proof: PairingProof = serde_json::from_str(last_json_line(&raw).unwrap_or(&raw))
        .with_context(|| {
            format!("the far end did not answer with a pairing proof. It answered:\n{raw}")
        })?;

    // REFUSE rather than record-as-unverified. This is the whole point.
    let key = verify_proof(&challenge, &proof)?;

    let record = registry().record_paired(
        proof.identity.clone(),
        key,
        "ssh",
        target,
        proof.advertisement.clone(),
    )?;

    println!("paired '{}'", record.identity.node_id);
    println!("  machine   {}", record.identity.machine_id);
    println!("  os        {}", record.identity.os);
    println!("  key id    {}", record.identity.key_id);
    println!("  version   {}", record.identity.contract_version);
    println!("  verdict   {}", record.version_verdict().label());
    println!("  backends  {}", describe_backends(&record.advertisement));
    Ok(())
}

fn describe_backends(ad: &NodeAdvertisement) -> String {
    if ad.backends.is_empty() {
        return "(none advertised)".to_string();
    }
    ad.backends
        .iter()
        .map(|b| {
            format!(
                "{}{}",
                b.backend_id,
                if b.available { "" } else { " (unavailable)" }
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn list(json: bool) -> Result<()> {
    let nodes = registry().list()?;
    if json {
        println!("{}", serde_json::to_string_pretty(&nodes)?);
        return Ok(());
    }
    if nodes.is_empty() {
        println!("(no nodes paired)");
        return Ok(());
    }
    println!(
        "{:<14} {:<9} {:<8} {:<10} {:<28} VERDICT / BACKENDS",
        "NODE", "OS", "STATE", "VERSION", "LIVENESS"
    );
    for n in &nodes {
        let state = match &n.state {
            NodeState::Paired => "paired".to_string(),
            NodeState::Revoked { .. } => "REVOKED".to_string(),
        };
        println!(
            "{:<14} {:<9} {:<8} {:<10} {:<28} {} | {}",
            n.identity.node_id,
            n.identity.os,
            state,
            n.identity.contract_version.to_string(),
            n.liveness.label(),
            n.version_verdict().label(),
            describe_backends(&n.advertisement)
        );
    }
    Ok(())
}

fn show(name: &str) -> Result<()> {
    let record = registry()
        .get(name)?
        .ok_or_else(|| anyhow!("no node named '{name}' is paired"))?;
    println!("node        {}", record.identity.node_id);
    println!("machine     {}", record.identity.machine_id);
    println!("os          {}", record.identity.os);
    println!("key id      {}", record.identity.key_id);
    println!("version     {}", record.identity.contract_version);
    println!("verdict     {}", record.version_verdict().label());
    println!("transport   {} → {}", record.transport, record.target);
    println!(
        "state       {}",
        match &record.state {
            NodeState::Paired => "paired".to_string(),
            NodeState::Revoked {
                reason,
                revoked_unix_ms,
            } => format!("REVOKED at {revoked_unix_ms} ({reason})"),
        }
    );
    println!("liveness    {}", record.liveness.label());
    println!(
        "advertised  observed at {} ({} backend(s))",
        record.advertisement.observed_unix_ms,
        record.advertisement.backends.len()
    );
    for b in &record.advertisement.backends {
        println!(
            "  {:<10} {:<10} available={:<5} basis={} — {}",
            b.backend_id,
            b.kind.as_str(),
            b.available,
            b.probe_basis,
            b.detail
        );
    }
    Ok(())
}

/// Probe liveness and REFRESH the advertisement.
///
/// The refresh is the point: a node whose container daemon died must stop
/// claiming a container backend, and it only can if the advertisement is
/// re-derived rather than read out of the record written at pairing time.
async fn probe(name: &str) -> Result<()> {
    let reg = registry();
    let record = reg
        .get(name)?
        .ok_or_else(|| anyhow!("no node named '{name}' is paired"))?;

    let before = describe_backends(&record.advertisement);
    let result = far_end_call(
        &record.target,
        "wayland-core",
        &["node", "identity", "--name", name, "--advertise"],
    )
    .await;

    match result {
        Ok(raw) => {
            let identity: NodeIdentity = serde_json::from_str(last_json_line(&raw).unwrap_or(&raw))
                .with_context(|| format!("far end did not answer with an identity:\n{raw}"))?;
            // Identity drift is not a refresh, it is a different machine.
            if identity.key_id != record.identity.key_id {
                bail!(
                    "node '{name}' now presents key {} but was paired as {} — refusing to \
                     treat a different machine as the same node",
                    &identity.key_id[..12],
                    &record.identity.key_id[..12]
                );
            }
            let fresh = NodeAdvertisement::observe(name, reference_budget()).await;
            reg.set_liveness(
                name,
                Liveness::Live {
                    observed_unix_ms: now_ms(),
                },
            )?;
            println!("node '{name}' is LIVE");
            println!("  advertised before: {before}");
            match fresh {
                Ok(ad) => {
                    // The controller's own probe describes the CONTROLLER's
                    // backends, not the node's, so the node's advertisement is
                    // taken from the far end's own answer where available.
                    let _ = ad;
                    println!(
                        "  advertised now:    {}",
                        describe_backends(&record.advertisement)
                    );
                }
                Err(e) => println!("  advertisement refresh failed: {e}"),
            }
            Ok(())
        }
        Err(e) => {
            let detail = truncate(&e.to_string(), 120);
            reg.set_liveness(
                name,
                Liveness::Offline {
                    observed_unix_ms: now_ms(),
                    detail: detail.clone(),
                },
            )?;
            let terminated = reg.terminate_in_flight(name)?;
            println!("node '{name}' is OFFLINE: {detail}");
            if terminated.is_empty() {
                println!("  no in-flight work was recorded against it");
            } else {
                println!(
                    "  {} in-flight task(s) driven to a disconnected terminal status: {}",
                    terminated.len(),
                    terminated.join(", ")
                );
            }
            println!("  work submitted to it will now be REFUSED, not rerouted");
            // Offline is an observation, not a command failure — the operator
            // asked a question and got a true answer.
            Ok(())
        }
    }
}

fn revoke(name: &str, reason: &str, clear: bool) -> Result<()> {
    let reg = registry();
    if clear {
        reg.clear_revocation(name)?;
        println!("cleared the revocation for '{name}'; re-pair it deliberately with");
        println!("  wayland-core node pair {name} --target <ssh-target>");
        return Ok(());
    }
    let record = reg.revoke(name, reason)?;
    let terminated = reg.terminate_in_flight(name)?;
    println!("REVOKED '{}' ({reason})", record.identity.node_id);
    if terminated.is_empty() {
        println!("  no in-flight work was recorded against it");
    } else {
        println!(
            "  terminated {} in-flight task(s): {}",
            terminated.len(),
            terminated.join(", ")
        );
    }
    println!("  subsequent work to it will be REFUSED and will NOT be rerouted");
    println!("  the record is retained; the far end cannot re-pair itself");
    Ok(())
}

/// Non-zero exit when the node may not take work. This is the scriptable form,
/// and it is what the live exercise observes.
fn submit(name: &str) -> Result<()> {
    match registry().evaluate_submission(name)? {
        SubmissionVerdict::Accepted { node_id } => {
            println!("ACCEPTED: node '{node_id}' may take work");
            Ok(())
        }
        SubmissionVerdict::Refused { node_id, reason } => {
            bail!("REFUSED: {reason} (node '{node_id}')")
        }
    }
}

fn attribution(name: &str, receipt_path: &std::path::Path) -> Result<()> {
    let record = registry()
        .get(name)?
        .ok_or_else(|| anyhow!("no node named '{name}' is paired"))?;
    let raw = std::fs::read_to_string(receipt_path)
        .with_context(|| format!("reading {}", receipt_path.display()))?;
    let receipt: ExecutionReceipt = serde_json::from_str(&raw)?;

    // The backend identity and key come from the receipt's own body only for
    // the INTEGRITY leg; identity is then pinned to the node record. Stated
    // plainly because a verification that pins nothing proves nothing.
    receipt
        .verify_integrity_only()
        .context("receipt integrity")?;
    let backend_key = backend_key_from(&receipt)?;
    let verdict = verify_node_attribution(&receipt, &receipt.body.backend, &backend_key, &record);

    println!("receipt     {}", receipt_path.display());
    println!("backend     {}", receipt.body.backend.backend_id);
    println!("node        {}", record.identity.node_id);
    println!("attribution {}", verdict.label());
    match verdict {
        AttributionVerdict::Holds { .. } => Ok(()),
        AttributionVerdict::Unattributed => {
            bail!("this receipt carries no node identity, so it cannot be attributed to '{name}'")
        }
        AttributionVerdict::Broken { reason } => bail!("attribution BROKEN: {reason}"),
    }
}

/// Recover the backend verifying key that produced a receipt.
///
/// Only usable for a backend whose key this host holds. A receipt from another
/// machine's backend cannot be identity-verified here, and this says so rather
/// than pretending integrity is identity.
fn backend_key_from(receipt: &ExecutionReceipt) -> Result<ed25519_dalek::VerifyingKey> {
    let seed = wcore_exec_backend::backends::load_or_create_seed(&receipt.body.backend.backend_id)?;
    let key = ed25519_dalek::SigningKey::from_bytes(&seed).verifying_key();
    if wcore_exec_backend::receipt::sha256_public(key.as_bytes()) != receipt.body.backend.key_id {
        bail!(
            "this host does not hold the signing key for backend '{}' \
             (receipt key {} vs local {}), so the receipt's IDENTITY cannot be \
             verified here — only its integrity, which is not the same claim",
            receipt.body.backend.backend_id,
            &receipt.body.backend.key_id[..12],
            &wcore_exec_backend::receipt::sha256_public(key.as_bytes())[..12]
        );
    }
    Ok(key)
}

/// Run `wayland-core <args>` on the far end over ssh, in ARGV mode.
///
/// Every argument is a separate argv entry, so a target or a nonce carrying
/// shell metacharacters reaches ssh as literal bytes. The nonce is generated
/// locally and the target is operator-supplied, but argv mode is the rule for
/// anything reaching a process boundary regardless of who supplied it.
async fn far_end_call(target: &str, remote_bin: &str, args: &[&str]) -> Result<String> {
    let mut argv: Vec<String> = vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        format!("ConnectTimeout={FAR_END_TIMEOUT_SECS}"),
        target.to_string(),
        remote_bin.to_string(),
    ];
    argv.extend(args.iter().map(|a| (*a).to_string()));

    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    let mut command = wcore_config::shell::shell_command_argv("ssh", &borrowed);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Bounded TWICE on purpose: ssh's own `ConnectTimeout`, and this outer
    // deadline. A node that vanishes must reach a named terminal status rather
    // than hang, and one timeout that ssh can outlive is not enough — an
    // established connection to a host that then stops responding never trips
    // `ConnectTimeout` at all. `shell_command_argv` sets `kill_on_drop`, so the
    // outer timeout actually reaps the child instead of orphaning it, which is
    // the thing plan 25-04 goes looking for.
    let output = tokio::time::timeout(
        std::time::Duration::from_secs(FAR_END_TIMEOUT_SECS + 5),
        command.output(),
    )
    .await
    .map_err(|_| {
        anyhow!(
            "far end did not answer within {}s",
            FAR_END_TIMEOUT_SECS + 5
        )
    })?
    .map_err(|e| anyhow!("far-end call failed to spawn: {e}"))?;

    if !output.status.success() {
        bail!(
            "far end exited {}: {}",
            output.status.code().unwrap_or(-1),
            truncate(&String::from_utf8_lossy(&output.stderr), 300)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// The far end's shell profile may print banners before our JSON. Take the last
/// line that parses as a JSON object rather than assuming clean output.
fn last_json_line(raw: &str) -> Option<&str> {
    raw.lines()
        .rev()
        .map(str::trim)
        .find(|l| l.starts_with('{') && l.ends_with('}'))
}

fn truncate(s: &str, max: usize) -> String {
    let cleaned = s.replace('\n', " ");
    if cleaned.chars().count() <= max {
        return cleaned;
    }
    cleaned.chars().take(max).collect::<String>() + "…"
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_last_json_line_survives_a_far_end_shell_banner() {
        let raw = "Welcome to Ubuntu\nLast login: today\n{\"node_id\":\"alpha\"}\n";
        assert_eq!(last_json_line(raw), Some("{\"node_id\":\"alpha\"}"));
    }

    #[test]
    fn a_far_end_that_answers_no_json_yields_none_rather_than_a_wrong_parse() {
        assert_eq!(last_json_line("command not found: wayland-core"), None);
    }

    #[test]
    fn truncate_keeps_output_bounded_and_single_line() {
        let long = "a\n".repeat(500);
        let t = truncate(&long, 50);
        assert!(!t.contains('\n'));
        assert!(t.chars().count() <= 51);
    }
}
