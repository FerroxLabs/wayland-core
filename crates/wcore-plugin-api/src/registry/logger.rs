//! `ScopedPluginLogger` — thin wrapper that auto-tags every record with the
//! plugin's name via `tracing`.

pub struct ScopedPluginLogger<'a> {
    plugin_name: &'a str,
}

impl<'a> ScopedPluginLogger<'a> {
    pub fn new(plugin_name: &'a str) -> Self {
        Self { plugin_name }
    }

    pub fn info(&self, msg: &str) {
        tracing::info!(plugin = self.plugin_name, "{msg}");
    }

    pub fn warn(&self, msg: &str) {
        tracing::warn!(plugin = self.plugin_name, "{msg}");
    }

    pub fn error(&self, msg: &str) {
        tracing::error!(plugin = self.plugin_name, "{msg}");
    }

    pub fn debug(&self, msg: &str) {
        tracing::debug!(plugin = self.plugin_name, "{msg}");
    }
}
