use std::process::ExitCode;

use wcore_protocol::contract::{
    GENERATOR_VERSION, check_contract, manifest_digests, write_contract,
};

fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    match std::env::args().nth(1).as_deref() {
        Some("generate") => {
            write_contract()?;
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
            return Err("usage: wcore-contract <generate|check|digest>".into());
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
