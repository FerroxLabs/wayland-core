//! Graveyard writer. Losers + scored-but-not-retained children land here
//! with their full lineage + composite score for post-mortem analysis.
//!
//! Path layout: `<graveyard_root>/<run-id>/<generation>/<child_index>.json`

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Error as JsonError;

#[derive(Serialize)]
pub struct LoserEntry {
    pub run_id: String,
    pub generation: u32,
    pub child_index: u32,
    pub parent_id: String,
    pub mutation_kind: String,
    pub score: f64,
    /// First N bytes of the candidate body; trimmed to keep graveyard files
    /// human-pageable. Callers decide N (typically 512).
    pub body_excerpt: String,
}

#[derive(Debug, thiserror::Error)]
pub enum GraveyardError {
    #[error("graveyard io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("graveyard serialize error for run {run_id}/{generation}/{child_index}: {source}")]
    Json {
        run_id: String,
        generation: u32,
        child_index: u32,
        #[source]
        source: JsonError,
    },
}

pub fn write(root: &Path, e: &LoserEntry) -> Result<(), GraveyardError> {
    let dir = root.join(&e.run_id).join(e.generation.to_string());
    std::fs::create_dir_all(&dir).map_err(|source| GraveyardError::Io {
        path: dir.clone(),
        source,
    })?;
    let path = dir.join(format!("{}.json", e.child_index));
    let json = serde_json::to_string_pretty(e).map_err(|source| GraveyardError::Json {
        run_id: e.run_id.clone(),
        generation: e.generation,
        child_index: e.child_index,
        source,
    })?;
    wcore_config::atomic_write(&path, json.as_bytes())
        .map_err(|source| GraveyardError::Io { path, source })
}
