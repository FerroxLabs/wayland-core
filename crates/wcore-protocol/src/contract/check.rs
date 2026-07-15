use std::collections::BTreeSet;
use std::fs;
use std::io;

use super::ContractResult;
use super::generate::{all_relative_files, contract_path, generated_artifacts};

/// Regenerate in memory and reject missing, extra, or byte-drifted artifacts.
pub fn check_contract() -> ContractResult<()> {
    let root = contract_path();
    let expected = generated_artifacts()?;
    let expected_paths = expected.keys().cloned().collect::<BTreeSet<_>>();
    let actual_paths = all_relative_files(&root)?;

    let missing = expected_paths
        .difference(&actual_paths)
        .cloned()
        .collect::<Vec<_>>();
    let extra = actual_paths
        .difference(&expected_paths)
        .cloned()
        .collect::<Vec<_>>();
    let mut drifted = Vec::new();
    for path in expected_paths.intersection(&actual_paths) {
        if fs::read(root.join(path))? != expected[path] {
            drifted.push(path.clone());
        }
    }
    if missing.is_empty() && extra.is_empty() && drifted.is_empty() {
        return Ok(());
    }

    Err(io::Error::other(format!(
        "Desktop contract corpus drift: missing={missing:?}, extra={extra:?}, drifted={drifted:?}; run `wcore-contract generate`",
    ))
    .into())
}
