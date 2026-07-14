//! CI signer and release verifier for Wayland evaluation receipts.

use std::io::Read as _;
use std::path::PathBuf;

use clap::{Parser, Subcommand};
use wcore_eval_scenarios::receipt_policy::{
    AuthoritativeReceiptPolicyV1, CiProvenanceV1, sign_ci_receipt, verify_authoritative_receipt,
};

#[derive(Debug, Parser)]
#[command(name = "wayland-receipt")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Sign a local receipt. The base64 32-byte Ed25519 seed is read from stdin.
    Sign {
        #[arg(long)]
        receipt: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        repository: String,
        #[arg(long)]
        source_ref: String,
        #[arg(long)]
        workflow: String,
        #[arg(long)]
        invocation_id: String,
    },
    /// Verify a signed receipt against an independently supplied trust policy.
    Verify {
        #[arg(long)]
        receipt: PathBuf,
        #[arg(long)]
        trust_policy: PathBuf,
    },
}

struct SecretBytes(Vec<u8>);

impl Drop for SecretBytes {
    fn drop(&mut self) {
        wipe(&mut self.0);
    }
}

fn wipe(bytes: &mut [u8]) {
    for byte in bytes {
        // SAFETY: `byte` is a valid unique reference for this write. Volatile
        // prevents the compiler from eliding this security-sensitive wipe.
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

fn main() {
    if let Err(error) = execute(Cli::parse()) {
        eprintln!("wayland-receipt: {error}");
        std::process::exit(1);
    }
}

fn execute(cli: Cli) -> Result<(), String> {
    match cli.command {
        Command::Sign {
            receipt,
            output,
            key_id,
            repository,
            source_ref,
            workflow,
            invocation_id,
        } => {
            let receipt_json = std::fs::read(&receipt).map_err(|error| {
                format!("could not read receipt {}: {error}", receipt.display())
            })?;
            let mut secret = SecretBytes(Vec::new());
            std::io::stdin()
                .take(4097)
                .read_to_end(&mut secret.0)
                .map_err(|error| format!("could not read signing key from stdin: {error}"))?;
            if secret.0.len() > 4096 {
                return Err("signing key input exceeds 4096 bytes".to_string());
            }
            let signed = sign_ci_receipt(
                &receipt_json,
                &key_id,
                &secret.0,
                CiProvenanceV1 {
                    repository,
                    source_ref,
                    workflow,
                    invocation_id,
                },
            )
            .map_err(|error| error.to_string())?;
            let encoded = serde_json::to_vec_pretty(&signed)
                .map_err(|error| format!("could not encode signed receipt: {error}"))?;
            wcore_config::atomic_write(&output, &encoded).map_err(|error| {
                format!(
                    "could not write signed receipt {}: {error}",
                    output.display()
                )
            })?;
            println!("SIGNED receipt_sha256={}", signed.body_sha256);
        }
        Command::Verify {
            receipt,
            trust_policy,
        } => {
            let receipt_json = std::fs::read(&receipt).map_err(|error| {
                format!("could not read receipt {}: {error}", receipt.display())
            })?;
            let policy_json = std::fs::read(&trust_policy).map_err(|error| {
                format!(
                    "could not read trust policy {}: {error}",
                    trust_policy.display()
                )
            })?;
            let policy: AuthoritativeReceiptPolicyV1 = serde_json::from_slice(&policy_json)
                .map_err(|error| format!("invalid trust policy JSON: {error}"))?;
            let (receipt, _) = verify_authoritative_receipt(&receipt_json, &policy)
                .map_err(|error| error.to_string())?;
            println!("AUTHORITATIVE PASS receipt_sha256={}", receipt.body_sha256);
        }
    }
    Ok(())
}
