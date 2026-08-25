use std::process::ExitCode;

use wcore_protocol::contract::{
    GENERATOR_VERSION, WireShapeBaseline, check_contract, manifest_digests, write_contract,
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
        Some("digest") => {
            let (fixtures, schemas, sources) = manifest_digests()?;
            println!("fixture_digest={fixtures}");
            println!("schema_digest={schemas}");
            println!("source_inputs_digest={sources}");
            println!("generator={GENERATOR_VERSION}");
        }
        _ => {
            return Err(
                "usage: wcore-contract <generate [--bootstrap-wire-shapes]|check|digest>".into(),
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
