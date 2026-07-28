//! `wayland-scorecard` — the single executable surface of Phase 30 (F30-01, F30-02).
//!
//! Two subcommands:
//!
//! - `surfaces` walks a real binary's own `--help` tree and emits a sorted,
//!   byte-deterministic table. The inventory is therefore a MEASUREMENT of the
//!   shipped artifact, not a reading of a planning document — a gate can
//!   regenerate it on real hardware and diff it against the committed bytes, so
//!   a hand-edited inventory fails.
//! - `verify` checks a scorecard document against a repository: every surface
//!   row must carry its seven truths and every criterion graded MET must pay for
//!   it with resolving, proven evidence.
//!
//! No secret is read, printed, logged or accepted on argv.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use wcore_eval_scenarios::scorecard::{
    ScorecardDocumentV1, render_surfaces_tsv, walk_command_tree,
};

#[derive(Parser)]
#[command(
    name = "wayland-scorecard",
    about = "Walk a shipped binary's command tree and verify a scorecard document"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Walk a binary's own --help tree and emit the sorted surface table.
    Surfaces {
        /// Path to the binary to walk. It is EXECUTED with `--help`.
        #[arg(long)]
        binary: PathBuf,
    },
    /// Verify a scorecard document against a repository.
    Verify {
        /// Path to the scorecard JSON document.
        #[arg(long)]
        document: PathBuf,
        /// Repository root that evidence references are resolved against.
        #[arg(long, default_value = ".")]
        repo_root: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(out) => {
            print!("{out}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("wayland-scorecard: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> anyhow::Result<String> {
    match cli.command {
        Command::Surfaces { binary } => {
            let nodes = walk_command_tree(&binary)?;
            Ok(render_surfaces_tsv(&nodes))
        }
        Command::Verify {
            document,
            repo_root,
        } => {
            let raw = std::fs::read_to_string(&document)?;
            // Unknown fields are refused HERE, before any verification logic
            // runs, so an invented grade or a stray key cannot reach the rules.
            let doc: ScorecardDocumentV1 = serde_json::from_str(&raw)?;
            doc.verify(&repo_root)?;
            Ok(format!(
                "SCORECARD_VERIFY=OK criteria={} surfaces={} source_sha={}\n",
                doc.criteria.len(),
                doc.surfaces.len(),
                doc.source_sha
            ))
        }
    }
}
