//! `wayland-journey` — the receipt tool for Phase 24 Success Criterion 5.
//!
//! Deliberately a SEPARATE binary from `wayland-receipt`. That tool's `verify`
//! means "authoritative, Ed25519-signed release evidence, checked against a
//! policy that binds provider, model, fixture digest and required evaluation
//! cells". A journey receipt has no provider, no model and no cells, and this
//! phase holds no signing key. Widening `wayland-receipt` to accept an unsigned
//! journey receipt would blur an authority boundary that exists on purpose.
//!
//! Four subcommands, every argument long-named and required, so a gate written
//! against this surface cannot be written against a shape the parser rejects.
//!
//! ```text
//! wayland-journey verify --receipt R --binary B --expect-platform P --expect-commit C
//! wayland-journey scan   --document D --canary-file F --raw-capture R [--raw-capture R]...
//! wayland-journey redact --input I --output O --secrets-file S
//! wayland-journey bind   --receipt R --receipt R [--receipt R]...
//! ```
//!
//! Every subcommand exits non-zero on refusal and prints ONE machine-readable
//! success line otherwise, so a gate can end on an exit status and a grep for a
//! line this tool emitted rather than on prose in a document the executor wrote.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use wcore_eval_scenarios::journey::{
    self, JourneyReceipt, MIN_CANARY_BYTES, bind_receipts, parse_canary_file, parse_receipt,
    scan_canaries, verify_receipt,
};
use wcore_eval_scenarios::redaction::SecretRedactor;

#[derive(Parser, Debug)]
#[command(
    name = "wayland-journey",
    about = "Verify, scan, redact and bind Phase 24 setup-to-recovery journey receipts"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Verify one platform receipt against the binary it claims to have driven.
    Verify {
        /// The receipt JSON.
        #[arg(long)]
        receipt: PathBuf,
        /// The driven binary. THIS tool hashes it; the receipt's own recorded
        /// digest is checked against that, never trusted in its place.
        #[arg(long)]
        binary: PathBuf,
        /// linux | macos | windows.
        #[arg(long)]
        expect_platform: String,
        /// The 40-hex source commit the driven binary must have been built from.
        #[arg(long)]
        expect_commit: String,
        /// Refuse a receipt exercising fewer than N distinct channel adapters.
        ///
        /// Omitted, a one-adapter journey verifies — it is a legitimate journey
        /// — but the success line reports `adapters=1/10` so it cannot be read
        /// as a matrix. Supply this when the claim being made IS a matrix.
        #[arg(long)]
        min_adapters: Option<u64>,
    },
    /// Prove every canary travelled a real capture path and none reached the
    /// document about to be committed.
    Scan {
        #[arg(long)]
        document: PathBuf,
        /// Newline-delimited, one sentinel per platform.
        #[arg(long)]
        canary_file: PathBuf,
        /// Repeatable. At least one required.
        #[arg(long, required = true)]
        raw_capture: Vec<PathBuf>,
    },
    /// Apply the exact-secret redactor and prove no secret survived.
    Redact {
        #[arg(long)]
        input: PathBuf,
        #[arg(long)]
        output: PathBuf,
        /// Newline-delimited secrets. A too-short entry is refused, not dropped.
        #[arg(long)]
        secrets_file: PathBuf,
    },
    /// Prove two or more receipts describe ONE candidate.
    Bind {
        #[arg(long, required = true)]
        receipt: Vec<PathBuf>,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("wayland-journey: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<String> {
    match Cli::parse().command {
        Command::Verify {
            receipt,
            binary,
            expect_platform,
            expect_commit,
            min_adapters,
        } => {
            let raw = read_to_string(&receipt)?;
            let line = verify_receipt(
                &raw,
                &binary,
                &expect_platform,
                &expect_commit,
                min_adapters,
            )
            .map_err(|error| anyhow::anyhow!("{error}"))?;
            Ok(line)
        }
        Command::Scan {
            document,
            canary_file,
            raw_capture,
        } => {
            let canaries = parse_canary_file(&read_to_string(&canary_file)?);
            let document_text = read_to_string(&document)?;
            let mut captures = Vec::new();
            for path in &raw_capture {
                captures.push((path.display().to_string(), read_to_string(path)?));
            }
            let verdicts = scan_canaries(&document_text, &canaries, &captures)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            for verdict in &verdicts {
                // The canary itself is never printed: this line goes into a log
                // that may be pasted anywhere, and a tool that proves a secret
                // was redacted by echoing it is its own leak.
                println!(
                    "canary[{}..] control=present published=absent",
                    &verdict.canary[..MIN_CANARY_BYTES.min(verdict.canary.len()) / 2]
                );
            }
            Ok(format!(
                "SCAN PASS canaries={} document={}",
                verdicts.len(),
                document.display()
            ))
        }
        Command::Redact {
            input,
            output,
            secrets_file,
        } => {
            let secrets: Vec<String> = read_to_string(&secrets_file)?
                .lines()
                .map(|line| line.trim().to_string())
                .filter(|line| !line.is_empty())
                .collect();
            if secrets.is_empty() {
                anyhow::bail!("secrets file {} is empty", secrets_file.display());
            }
            let redactor = SecretRedactor::from_secret_set(secrets)
                .map_err(|error| anyhow::anyhow!("{error}"))?;
            let source = read_to_string(&input)?;
            let (redacted, hit) = redactor.text(source);
            std::fs::write(&output, &redacted)?;
            // Re-read what was WRITTEN, not what was computed. A redactor that
            // reports success from its in-memory copy has not proved the file
            // on disk is clean, and the file on disk is what gets committed.
            let written = read_to_string(&output)?;
            if redactor.any_present(&written) {
                anyhow::bail!(
                    "a secret survived into {} — refusing to report success",
                    output.display()
                );
            }
            Ok(format!(
                "REDACTED input={} output={} secrets={} hits={}",
                input.display(),
                output.display(),
                redactor.secret_count(),
                u8::from(hit),
            ))
        }
        Command::Bind { receipt } => {
            let mut receipts: Vec<JourneyReceipt> = Vec::new();
            for path in &receipt {
                let raw = read_to_string(path)?;
                receipts.push(
                    parse_receipt(&raw)
                        .map_err(|error| anyhow::anyhow!("{}: {error}", path.display()))?,
                );
            }
            bind_receipts(&receipts).map_err(|error| anyhow::anyhow!("{error}"))
        }
    }
}

/// Read a file, refusing an empty one by name. Every input this tool takes is
/// evidence, and an empty evidence file that reads as "nothing to object to" is
/// the self-passing shape this whole tool exists to refuse.
fn read_to_string(path: &PathBuf) -> anyhow::Result<String> {
    let contents = std::fs::read_to_string(path)
        .map_err(|error| anyhow::anyhow!("cannot read {}: {error}", path.display()))?;
    if contents.trim().is_empty() {
        anyhow::bail!("{} is empty", path.display());
    }
    Ok(contents)
}

// Keeps the `journey` import honest if the module's re-exports change shape.
const _: [&str; 17] = journey::CANONICAL_STEPS;
