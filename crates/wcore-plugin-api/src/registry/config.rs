//! `ScopedConfigReader` — read-only typed view of host configuration values
//! exposed to a plugin. Not permission-gated (read-only by design).
//!
//! T3-1 lift from Forge `@forge-cli/plugin-api`:
//!   - `project_root()` — absolute path to the project root (for plugin-side
//!     path-traversal validation).
//!   - `plugin_config()` — read the plugin's own namespaced config bucket
//!     (`plugins.<plugin-name>` in host config).
//!   - `get_raw_safe()` — `get_raw` with sensitive-key redaction. Keys whose
//!     names look like credentials are forced to `None` regardless of what
//!     the host returns.

use serde::de::DeserializeOwned;

pub trait ConfigReader: Send + Sync {
    /// Returns the raw JSON value for `key`, or `None` if absent.
    fn get_raw(&self, key: &str) -> Option<serde_json::Value>;

    /// Absolute path to the current project root, if the host has one.
    ///
    /// Default impl returns `None` — hosts without a project context (e.g.
    /// `NullConfigReader`) need not implement this.
    fn project_root(&self) -> Option<std::path::PathBuf> {
        None
    }

    /// The plugin's own namespaced configuration bucket, looked up by plugin
    /// name. Hosts typically resolve this from `plugins.<plugin_name>` in the
    /// global config tree.
    ///
    /// Default impl returns an empty object — hosts without per-plugin config
    /// need not implement this. Returning `serde_json::Value::Object` keeps
    /// the contract uniform for plugins (they can always `.as_object()`).
    fn plugin_config(&self, _plugin_name: &str) -> serde_json::Value {
        serde_json::Value::Object(serde_json::Map::new())
    }
}

/// Returns `true` if `key` looks like it names a credential / secret and
/// should therefore be redacted from plugin reads.
///
/// Mirrors Forge's `/key|secret|token|password|credential/i` regex but
/// implemented as a simple case-insensitive substring match to avoid pulling
/// in a regex dependency at this isolation boundary.
fn is_sensitive_key(key: &str) -> bool {
    const NEEDLES: &[&str] = &["key", "secret", "token", "password", "credential"];
    let lower = key.to_ascii_lowercase();
    NEEDLES.iter().any(|n| lower.contains(n))
}

pub struct ScopedConfigReader<'a> {
    host: &'a dyn ConfigReader,
    /// Plugin name used to scope `plugin_config()` reads. `None` when the
    /// caller used `new()` instead of `with_plugin_name()`; in that case
    /// `plugin_config()` returns an empty object.
    plugin_name: Option<&'a str>,
}

impl<'a> ScopedConfigReader<'a> {
    pub fn new(host: &'a dyn ConfigReader) -> Self {
        Self {
            host,
            plugin_name: None,
        }
    }

    /// T3-1 — same as `new()` but also tags the reader with the plugin name so
    /// `plugin_config()` can resolve the correct namespaced bucket. Existing
    /// callers using `new()` continue to compile; only callers that need the
    /// plugin's own config opt into this form.
    pub fn with_plugin_name(host: &'a dyn ConfigReader, plugin_name: &'a str) -> Self {
        Self {
            host,
            plugin_name: Some(plugin_name),
        }
    }

    /// Get a typed value by key. Returns `None` if the key is missing or the
    /// stored value cannot be deserialized into `T`.
    pub fn get<T: DeserializeOwned>(&self, key: &str) -> Option<T> {
        let raw = self.host.get_raw(key)?;
        serde_json::from_value(raw).ok()
    }

    /// T3-1 — like `get_raw` on the host trait but applies sensitive-key
    /// redaction: keys matching the heuristic for credentials are forced to
    /// `None` even if the host has a value for them.
    pub fn get_raw_safe(&self, key: &str) -> Option<serde_json::Value> {
        if is_sensitive_key(key) {
            return None;
        }
        self.host.get_raw(key)
    }

    /// T3-1 — absolute path to the project root, if the host advertises one.
    pub fn project_root(&self) -> Option<std::path::PathBuf> {
        self.host.project_root()
    }

    /// T3-1 — this plugin's own namespaced configuration bucket. Returns an
    /// empty JSON object if the reader was constructed without a plugin name
    /// (via `new()`) or if the host has no entry for this plugin.
    pub fn plugin_config(&self) -> serde_json::Value {
        match self.plugin_name {
            Some(name) => self.host.plugin_config(name),
            None => serde_json::Value::Object(serde_json::Map::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::path::PathBuf;

    /// Test double that lets us assert which keys / plugin names were queried
    /// and what the host returned.
    #[derive(Default)]
    struct FakeHost {
        values: HashMap<String, serde_json::Value>,
        project_root: Option<PathBuf>,
        plugin_configs: HashMap<String, serde_json::Value>,
    }

    impl ConfigReader for FakeHost {
        fn get_raw(&self, key: &str) -> Option<serde_json::Value> {
            self.values.get(key).cloned()
        }
        fn project_root(&self) -> Option<PathBuf> {
            self.project_root.clone()
        }
        fn plugin_config(&self, plugin_name: &str) -> serde_json::Value {
            self.plugin_configs
                .get(plugin_name)
                .cloned()
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()))
        }
    }

    // --- get_raw_safe ------------------------------------------------------

    #[test]
    fn get_raw_safe_returns_value_for_non_sensitive_key() {
        let mut host = FakeHost::default();
        host.values
            .insert("log_level".to_string(), serde_json::json!("debug"));
        let reader = ScopedConfigReader::new(&host);

        let v = reader.get_raw_safe("log_level");
        assert_eq!(v, Some(serde_json::json!("debug")));
    }

    #[test]
    fn get_raw_safe_redacts_sensitive_keys_even_if_host_returns_value() {
        let mut host = FakeHost::default();
        // Host happily returns each — reader must NOT pass any through.
        for k in [
            "api_key",
            "OPENAI_API_KEY",
            "auth_token",
            "user_password",
            "client_secret",
            "aws_credential",
        ] {
            host.values.insert(k.to_string(), serde_json::json!("xxx"));
        }
        let reader = ScopedConfigReader::new(&host);

        for k in [
            "api_key",
            "OPENAI_API_KEY",
            "auth_token",
            "user_password",
            "client_secret",
            "aws_credential",
        ] {
            assert_eq!(
                reader.get_raw_safe(k),
                None,
                "expected sensitive key `{k}` to be redacted"
            );
        }
    }

    // --- project_root ------------------------------------------------------

    #[test]
    fn project_root_returns_host_advertised_path() {
        let host = FakeHost {
            project_root: Some(PathBuf::from("/var/projects/wayland")),
            ..FakeHost::default()
        };
        let reader = ScopedConfigReader::new(&host);

        assert_eq!(
            reader.project_root(),
            Some(PathBuf::from("/var/projects/wayland"))
        );
    }

    #[test]
    fn project_root_is_none_when_host_has_no_project_context() {
        let host = FakeHost::default(); // project_root: None
        let reader = ScopedConfigReader::new(&host);

        assert_eq!(reader.project_root(), None);
    }

    // --- plugin_config -----------------------------------------------------

    #[test]
    fn plugin_config_returns_namespaced_bucket_when_plugin_name_set() {
        let mut host = FakeHost::default();
        host.plugin_configs.insert(
            "wayland-ijfw".to_string(),
            serde_json::json!({"enabled": true, "max_items": 7}),
        );
        let reader = ScopedConfigReader::with_plugin_name(&host, "wayland-ijfw");

        let cfg = reader.plugin_config();
        assert_eq!(cfg["enabled"], serde_json::json!(true));
        assert_eq!(cfg["max_items"], serde_json::json!(7));
    }

    #[test]
    fn plugin_config_returns_empty_object_when_no_plugin_name() {
        // Constructed via `new()` — `plugin_config()` must not leak any other
        // plugin's config and must return an empty object.
        let mut host = FakeHost::default();
        host.plugin_configs
            .insert("other-plugin".to_string(), serde_json::json!({"x": 1}));
        let reader = ScopedConfigReader::new(&host);

        let cfg = reader.plugin_config();
        assert!(
            cfg.as_object().map(|o| o.is_empty()).unwrap_or(false),
            "expected empty object, got {cfg}"
        );
    }

    #[test]
    fn plugin_config_returns_empty_object_when_host_has_no_entry() {
        // Plugin name set, but host has no bucket for it — default impl path.
        let host = FakeHost::default();
        let reader = ScopedConfigReader::with_plugin_name(&host, "unknown-plugin");

        let cfg = reader.plugin_config();
        assert!(
            cfg.as_object().map(|o| o.is_empty()).unwrap_or(false),
            "expected empty object, got {cfg}"
        );
    }

    // --- sensitive-key heuristic unit -------------------------------------

    #[test]
    fn is_sensitive_key_matches_case_insensitively() {
        assert!(is_sensitive_key("API_KEY"));
        assert!(is_sensitive_key("openaiToken"));
        assert!(is_sensitive_key("user-password"));
        assert!(is_sensitive_key("CLIENT_SECRET"));
        assert!(is_sensitive_key("aws_credential"));
        assert!(!is_sensitive_key("log_level"));
        assert!(!is_sensitive_key("project_root"));
    }
}
