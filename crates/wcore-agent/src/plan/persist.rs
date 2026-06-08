//! F16 plan persistence.
//!
//! Plans persist to `dirs::config_dir()/wayland-core/plans/<session-id>.json`
//! on `ExitPlanMode`. Resume on next session is advertised via a
//! `ProtocolEvent::Info` banner — actual resume injects the persisted text
//! into the session's initial context. No new protocol variant needed.
//!
//! `root: Option<&Path>` lets tests override the storage root.

use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedPlan {
    pub session_id: String,
    pub ts_unix: u64,
    pub plan_text: String,
    pub source_product: String,
}

fn plans_dir(root: Option<&Path>) -> io::Result<PathBuf> {
    let base = match root {
        Some(p) => p.to_path_buf(),
        // F-059: honour WAYLAND_HOME so plan files land in the sandbox,
        // not in the host's ~/Library/Application Support/wayland-core/.
        None => wcore_config::config::wayland_config_dir(),
    };
    let dir = base.join("plans");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn save_plan_json(
    session_id: &str,
    plan_text: &str,
    root: Option<&Path>,
) -> io::Result<PathBuf> {
    let dir = plans_dir(root)?;
    let path = dir.join(format!("{session_id}.json"));
    let ts_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let p = PersistedPlan {
        session_id: session_id.to_string(),
        ts_unix,
        plan_text: plan_text.to_string(),
        source_product: "wayland-core".to_string(),
    };
    let json = serde_json::to_string_pretty(&p)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    wcore_config::atomic_write(&path, json.as_bytes())?;
    Ok(path)
}

pub fn load_plan_json(session_id: &str, root: Option<&Path>) -> io::Result<Option<PersistedPlan>> {
    let dir = plans_dir(root)?;
    let path = dir.join(format!("{session_id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let bytes = std::fs::read_to_string(&path)?;
    let p: PersistedPlan = serde_json::from_str(&bytes)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
    Ok(Some(p))
}
