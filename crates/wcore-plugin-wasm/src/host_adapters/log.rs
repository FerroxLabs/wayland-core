//! Log host capability — always allowed. No `Deny*` variant.
//!
//! Routes plugin log lines through `tracing` with the plugin name as a field.

pub trait WaylandHostLog: Send + Sync {
    fn log(&self, level: &str, msg: &str);
}

/// Gated log host (logging always allowed, so the `Gated` prefix is for naming
/// symmetry with the other adapters).
pub struct GatedHostLog {
    plugin: String,
}

impl GatedHostLog {
    pub fn new(plugin: String) -> Self {
        Self { plugin }
    }
}

impl WaylandHostLog for GatedHostLog {
    fn log(&self, level: &str, msg: &str) {
        match level.to_ascii_lowercase().as_str() {
            "error" => tracing::error!(plugin = %self.plugin, "{msg}"),
            "warn" => tracing::warn!(plugin = %self.plugin, "{msg}"),
            "info" => tracing::info!(plugin = %self.plugin, "{msg}"),
            "debug" => tracing::debug!(plugin = %self.plugin, "{msg}"),
            _ => tracing::trace!(plugin = %self.plugin, level = %level, "{msg}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_routes_through_tracing() {
        // No `tracing-test` dep in workspace — just verify the call doesn't panic
        // and that every level branch is reachable. A subscriber capture test
        // belongs in an integration test once `tracing-test` is added.
        let l = GatedHostLog::new("p".into());
        l.log("error", "e");
        l.log("warn", "w");
        l.log("info", "i");
        l.log("debug", "d");
        l.log("trace", "t");
        l.log("WEIRD", "fallback");
    }
}
