use std::io::Read;
use std::process::ExitCode;

use wcore_protocol::contract::{
    GENERATOR_VERSION, SOURCE_INPUTS, WireShapeBaseline, check_contract, manifest_diff_report,
    manifest_digests, preflight_notice, write_contract,
};

fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match std::env::args().nth(1).as_deref() {
        Some("generate") => {
            let baseline = match std::env::args().nth(2).as_deref() {
                None => WireShapeBaseline::Required,
                Some("--bootstrap-wire-shapes") => {
                    eprintln!(
                        "--bootstrap-wire-shapes: permitting a corpus that publishes no wire \
                         shapes. Legal only while a contract root is being created, and it \
                         cannot bless a change to a shape that is already published."
                    );
                    WireShapeBaseline::Bootstrap
                }
                Some(other) => return Err(format!("unknown generate option: {other}").into()),
            };
            write_contract(baseline)?;
            println!("generated Desktop contract corpus with {GENERATOR_VERSION}");
        }
        Some("check") => {
            check_contract()?;
            println!("Desktop contract corpus is current ({GENERATOR_VERSION})");
        }
        // Read-only sibling of `check`: the same manifest key diff, printed
        // without writing a byte. `check` is the gate and answers whether the
        // corpus is current; this answers WHAT moved, which is the question an
        // author actually has once the gate is already red. It exits 0 either
        // way - duplicating the gate here would give a red build two verdicts
        // to disagree about.
        Some("diff") => {
            print!("{}", manifest_diff_report()?);
        }
        Some("source-inputs") => {
            // So CI never re-hardcodes the list: it asks the binary that
            // actually hashes them.
            for relative in SOURCE_INPUTS {
                println!("{relative}");
            }
        }
        Some("preflight") => {
            // Advisory only, and INFALLIBLE by construction. A changed-file
            // list arrives on stdin (or in the file named by the second
            // argument) and the answer is a hint, so this arm has no `?` and
            // no failure path: bytes that are not UTF-8 are decoded lossily,
            // and a list that cannot be read at all is reported on stderr
            // instead of returned. A hint that exits non-zero is not a hint.
            // Graded by `preflight_survives_a_non_utf8_changed_file_list` and
            // `preflight_survives_a_changed_file_list_that_cannot_be_read`.
            let raw = match std::env::args().nth(2) {
                Some(path) => std::fs::read(&path).unwrap_or_else(|error| {
                    eprintln!("preflight: cannot read {path}: {error}; no hint emitted");
                    Vec::new()
                }),
                None => {
                    let mut buffer = Vec::new();
                    if let Err(error) = std::io::stdin().read_to_end(&mut buffer) {
                        eprintln!(
                            "preflight: cannot read the changed-file list from stdin: {error}; \
                             no hint emitted"
                        );
                        buffer.clear();
                    }
                    buffer
                }
            };
            let list = String::from_utf8_lossy(&raw);
            let changed = list.lines().map(str::to_string).collect::<Vec<_>>();
            if let Some(notice) = preflight_notice(&changed) {
                println!("::notice title=Desktop contract corpus::{notice}");
            }
        }
        Some("digest") => {
            let (fixtures, schemas, sources) = manifest_digests()?;
            println!("fixture_digest={fixtures}");
            println!("schema_digest={schemas}");
            println!("source_inputs_digest={sources}");
            println!("generator={GENERATOR_VERSION}");
        }
        _ => {
            return Err(
                "usage: wcore-contract <generate [--bootstrap-wire-shapes]|check|diff|source-inputs|preflight [FILE]|digest>"
                    .into(),
            );
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
