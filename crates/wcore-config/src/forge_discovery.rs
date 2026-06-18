//! Forge local-MCP discovery — consumer side (read-only).
//!
//! Forge-suite desktop apps (Agent Vault, future Foundry tools) advertise their
//! local MCP server by writing a shared file at
//! `<dirs::config_dir()>/forge/mcp-servers.json`. This module reads that file so
//! Wayland Core can auto-detect those servers instead of requiring hand-config.
//!
//! The path deliberately uses the real OS config dir (`dirs::config_dir()`), NOT
//! the WAYLAND_HOME-honoring [`wayland_config_dir`](crate::config::wayland_config_dir):
//! it is a cross-application convention written by *other* apps about the actual
//! machine, exactly like the Claude-Desktop MCP discovery.
//!
//! Per the contract (`.audit/waylandcore-mcp-fix/FORGE-MCP-DISCOVERY-SPEC.md`),
//! entries are **hints, not liveness** — a producer crash leaves a stale entry,
//! so a consumer MUST liveness-probe `metadata_url` before offering to connect,
//! and must never auto-connect silently. This module only parses; the probe +
//! grant + connect live in `wcore-mcp`.

use std::path::PathBuf;

use serde::Deserialize;

/// Schema version this consumer understands.
pub const DISCOVERY_VERSION: u32 = 1;

/// How a consumer obtains a bearer token for a discovered server.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DiscoveredAuth {
    /// `loopback-grant` (ask `grant_url` for a scoped token) or `none`.
    pub scheme: String,
    /// The `POST` endpoint that mints a scoped token (present for `loopback-grant`).
    #[serde(default)]
    pub grant_url: Option<String>,
}

/// One advertised local MCP server. Carries handles only — never a secret.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DiscoveredMcpServer {
    /// Stable machine id, unique per producing app (e.g. `agent-vault`).
    pub name: String,
    /// Human label for the "X detected — connect?" prompt.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Transport string as written by the producer (e.g. `streamable-http`).
    pub transport: String,
    /// The MCP endpoint, e.g. `http://127.0.0.1:3456/mcp`.
    pub url: String,
    /// How to authenticate.
    pub auth: DiscoveredAuth,
    /// Unauthenticated liveness/metadata endpoint to probe before connecting.
    pub metadata_url: String,
}

impl DiscoveredMcpServer {
    /// The label to show the user (falls back to `name` when no display name).
    pub fn label(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.name)
    }

    /// Whether this server uses the loopback-grant handshake (vs. `none`).
    pub fn uses_loopback_grant(&self) -> bool {
        self.auth.scheme == "loopback-grant" && self.auth.grant_url.is_some()
    }
}

#[derive(Debug, Deserialize)]
struct DiscoveryFile {
    // The file's `version` key is intentionally not modelled: we parse
    // best-effort across versions (unknown fields ignored, bad entries
    // dropped), so a newer producer never breaks an older consumer. The
    // version this consumer targets is [`DISCOVERY_VERSION`].
    #[serde(default)]
    servers: Vec<DiscoveredMcpServer>,
}

/// The shared Forge discovery file path: `<config_dir>/forge/mcp-servers.json`.
///
/// Returns `None` only when the OS has no resolvable config dir (rare; e.g. a
/// stripped environment with no `HOME`/`APPDATA`).
pub fn forge_discovery_path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("forge").join("mcp-servers.json"))
}

/// Read and parse the discovery file. Tolerant of a missing or malformed file
/// (→ empty list) — discovery is a best-effort convenience, never an error.
///
/// A file whose `version` is newer than [`DISCOVERY_VERSION`] is still parsed
/// best-effort: unknown fields are ignored and entries that fail to deserialize
/// are dropped, so a forward-compatible producer never breaks an older consumer.
pub fn read_discovered_servers() -> Vec<DiscoveredMcpServer> {
    match forge_discovery_path() {
        Some(path) => read_discovered_servers_at(&path),
        None => Vec::new(),
    }
}

/// [`read_discovered_servers`] against an explicit path (testable).
pub fn read_discovered_servers_at(path: &std::path::Path) -> Vec<DiscoveredMcpServer> {
    use std::io::Read;
    // F31: this file is cross-application and writable by any local app, so cap
    // the read to defend against an oversized/garbage file exhausting memory.
    const MAX_DISCOVERY_FILE_BYTES: u64 = 256 * 1024;
    let mut raw = String::new();
    let read = std::fs::File::open(path)
        .and_then(|f| f.take(MAX_DISCOVERY_FILE_BYTES).read_to_string(&mut raw));
    if read.is_err() {
        return Vec::new();
    }
    match serde_json::from_str::<DiscoveryFile>(&raw) {
        Ok(file) => file.servers,
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// The exact file content the live Agent Vault producer writes (from the
    /// frozen contract) must parse into one usable entry.
    const LIVE_CONTRACT_JSON: &str = r#"{
      "version": 1,
      "servers": [{
        "name": "agent-vault",
        "display_name": "Agent Vault",
        "transport": "streamable-http",
        "url": "http://127.0.0.1:3456/mcp",
        "auth": { "scheme": "loopback-grant", "grant_url": "http://127.0.0.1:3456/grant" },
        "metadata_url": "http://127.0.0.1:3456/.well-known/mcp"
      }]
    }"#;

    fn write_tmp(content: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().expect("temp file");
        f.write_all(content.as_bytes()).expect("write");
        f.flush().expect("flush");
        f
    }

    #[test]
    fn parses_the_frozen_live_contract() {
        let f = write_tmp(LIVE_CONTRACT_JSON);
        let servers = read_discovered_servers_at(f.path());
        assert_eq!(servers.len(), 1);
        let s = &servers[0];
        assert_eq!(s.name, "agent-vault");
        assert_eq!(s.label(), "Agent Vault");
        assert_eq!(s.url, "http://127.0.0.1:3456/mcp");
        assert_eq!(s.metadata_url, "http://127.0.0.1:3456/.well-known/mcp");
        assert!(s.uses_loopback_grant());
        assert_eq!(
            s.auth.grant_url.as_deref(),
            Some("http://127.0.0.1:3456/grant")
        );
    }

    #[test]
    fn missing_file_yields_no_servers_not_an_error() {
        let servers =
            read_discovered_servers_at(std::path::Path::new("/nonexistent/forge/mcp-servers.json"));
        assert!(servers.is_empty());
    }

    #[test]
    fn corrupt_file_yields_no_servers() {
        let f = write_tmp("{ not valid json …");
        assert!(read_discovered_servers_at(f.path()).is_empty());
    }

    #[test]
    fn label_falls_back_to_name_without_display_name() {
        let f = write_tmp(
            r#"{"version":1,"servers":[{"name":"foundry-x","transport":"streamable-http",
              "url":"http://127.0.0.1:9/mcp","auth":{"scheme":"none"},
              "metadata_url":"http://127.0.0.1:9/.well-known/mcp"}]}"#,
        );
        let servers = read_discovered_servers_at(f.path());
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].label(), "foundry-x");
        // `auth.scheme = none` is not a loopback-grant server.
        assert!(!servers[0].uses_loopback_grant());
    }

    #[test]
    fn preserves_multiple_servers_and_ignores_unknown_fields() {
        // Forward-compat: a newer producer may add fields/versions; we ignore
        // unknowns and still read every well-formed entry.
        let f = write_tmp(
            r#"{"version":2,"extra":"ignored","servers":[
              {"name":"a","transport":"streamable-http","url":"http://127.0.0.1:1/mcp",
               "auth":{"scheme":"loopback-grant","grant_url":"http://127.0.0.1:1/grant"},
               "metadata_url":"http://127.0.0.1:1/.well-known/mcp","future":true},
              {"name":"b","transport":"streamable-http","url":"http://127.0.0.1:2/mcp",
               "auth":{"scheme":"none"},"metadata_url":"http://127.0.0.1:2/.well-known/mcp"}
            ]}"#,
        );
        let servers = read_discovered_servers_at(f.path());
        assert_eq!(
            servers.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
    }
}
