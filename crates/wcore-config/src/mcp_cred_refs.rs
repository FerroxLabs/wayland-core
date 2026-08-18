//! `${cred:KEY}` credential-reference resolution for MCP server headers and
//! stdio environment variables (Slice 3, Piece 2).
//!
//! MCP `[mcp.servers.*]` `headers` and `env` in `config.toml` are literal
//! strings. To keep a secret OUT of `config.toml`, either a header value or an
//! `env` value may embed a reference of the form `${cred:KEY}`, e.g.
//!
//! ```toml
//! [mcp.servers.agent-vault]
//! transport = "streamable-http"
//! url = "http://127.0.0.1:3456/mcp"
//! allow_local = true
//! [mcp.servers.agent-vault.headers]
//! Authorization = "Bearer ${cred:mcp:agent-vault:token}"
//!
//! [mcp.servers.vendor]
//! transport = "stdio"
//! command = "vendor-mcp"
//! [mcp.servers.vendor.env]
//! VENDOR_API_KEY = "${cred:mcp:vendor:token}"
//! ```
//!
//! `env` matters most for stdio servers: they take their credential through the
//! child process environment, not through an HTTP header, so without this rail
//! their only option is a cleartext secret in `config.toml`.
//!
//! The literal `${cred:...}` stays on disk; the real secret is looked up from
//! the [`CredentialsStore`] and substituted in **at the connect boundary, on a
//! clone** of the server map — never written back into the long-lived in-memory
//! `Config`, so an accidental re-serialize can't leak the token to disk. `KEY`
//! is everything between `${cred:` and the next `}` (so it may itself contain
//! `:`, as the `mcp:<server>:token` convention does).
//!
//! A server whose headers and env carry no `${cred:` reference are passed
//! through untouched and never touch the store — existing literal-value MCP
//! servers are unaffected even when the store is empty or locked.

use std::collections::HashMap;

use crate::config::McpServerConfig;
use crate::credentials::{CredentialsError, CredentialsStore};

/// Marker that opens a credential reference: `${cred:KEY}`.
const CRED_PREFIX: &str = "${cred:";

/// The recommended credentials-store key for a Forge MCP server's bearer token.
/// Namespaced per server so two discovered servers never collide.
pub fn mcp_token_cred_key(server_name: &str) -> String {
    format!("mcp:{server_name}:token")
}

/// Build the `[mcp.servers.<name>]` config for a Forge loopback server: a
/// `streamable-http` server at `url`, `allow_local = true` (it lives on
/// 127.0.0.1), and an `Authorization` header whose value is a `${cred:KEY}`
/// reference — so the bearer token is stored in the credentials store and the
/// config file only ever carries the reference, never the secret.
pub fn build_forge_mcp_server_config(url: &str, cred_key: &str) -> McpServerConfig {
    let mut headers = HashMap::new();
    headers.insert(
        "Authorization".to_string(),
        format!("Bearer ${{cred:{cred_key}}}"),
    );
    McpServerConfig {
        transport: crate::config::TransportType::StreamableHttp,
        command: None,
        args: None,
        env: None,
        url: Some(url.to_string()),
        headers: Some(headers),
        deferred: None,
        allow_local: true,
        only_for_assistant: None,
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CredRefError {
    /// The referenced key is not present in the credentials store.
    #[error("credential reference ${{cred:{key}}} not found in the credentials store")]
    Missing { key: String },
    /// The store itself errored (e.g. keyring locked) while looking the key up.
    #[error("credentials store error resolving ${{cred:{key}}}: {source}")]
    Store {
        key: String,
        #[source]
        source: CredentialsError,
    },
    /// A `${cred:` opener with no closing `}`.
    #[error(
        "malformed credential reference (unterminated `${{cred:...}}`) in an MCP header or env value"
    )]
    Malformed,
}

/// Substitute every `${cred:KEY}` occurrence in `value` with the secret stored
/// under `KEY`. A value with no reference is returned unchanged (the store is
/// never consulted). Fails closed: a missing key or store error aborts the whole
/// value rather than emitting a half-resolved or empty bearer.
pub fn resolve_cred_refs(
    value: &str,
    store: &dyn CredentialsStore,
) -> Result<String, CredRefError> {
    if !value.contains(CRED_PREFIX) {
        return Ok(value.to_string());
    }
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(start) = rest.find(CRED_PREFIX) {
        out.push_str(&rest[..start]);
        let after = &rest[start + CRED_PREFIX.len()..];
        let end = after.find('}').ok_or(CredRefError::Malformed)?;
        let key = &after[..end];
        let secret = store
            .get(key)
            .map_err(|source| CredRefError::Store {
                key: key.to_string(),
                source,
            })?
            .ok_or_else(|| CredRefError::Missing {
                key: key.to_string(),
            })?;
        out.push_str(&secret);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Resolve `${cred:KEY}` references in every header AND stdio `env` value of
/// one server, in place. Used by the single-server live-add path where a
/// resolution failure is a hard error the user sees (they just asked to connect
/// this server).
///
/// Fails closed on the FIRST unresolvable reference: the caller must discard the
/// server rather than launch it, because a partially-resolved `env` would hand a
/// child process a literal `${cred:...}` (or an empty value) that looks like a
/// successful launch.
pub fn resolve_server_credential_refs(
    server: &mut McpServerConfig,
    store: &dyn CredentialsStore,
) -> Result<(), CredRefError> {
    let header_values = server
        .headers
        .as_mut()
        .into_iter()
        .flat_map(HashMap::values_mut);
    let env_values = server
        .env
        .as_mut()
        .into_iter()
        .flat_map(HashMap::values_mut);
    for value in header_values.chain(env_values) {
        if value.contains(CRED_PREFIX) {
            *value = resolve_cred_refs(value, store)?;
        }
    }
    Ok(())
}

/// Whether one server declaration requires credentials-store resolution. Reads
/// headers and stdio `env` alike — a stdio server whose only reference lives in
/// `env` must still be treated as credential-bearing, or the store-unavailable
/// path would launch it with the raw placeholder.
pub fn server_has_credential_references(server: &McpServerConfig) -> bool {
    let has_ref = |map: &Option<HashMap<String, String>>| {
        map.as_ref()
            .is_some_and(|values| values.values().any(|value| value.contains(CRED_PREFIX)))
    };
    has_ref(&server.headers) || has_ref(&server.env)
}

/// Secret-free reason a configured server was excluded before transport
/// creation. The referenced key and backend error are deliberately omitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpCredentialSkipReason {
    MissingCredential,
    CredentialStoreUnavailable,
    MalformedReference,
}

impl McpCredentialSkipReason {
    pub fn message(self) -> &'static str {
        match self {
            Self::MissingCredential => "required MCP credential is missing",
            Self::CredentialStoreUnavailable => "MCP credential store is unavailable",
            Self::MalformedReference => "MCP credential reference is malformed",
        }
    }
}

/// Connectable declarations plus explicit, redacted omissions. Callers that
/// advertise capabilities or diagnostics must use both halves.
#[derive(Debug, Default)]
pub struct McpServerResolution {
    pub connectable: HashMap<String, McpServerConfig>,
    pub skipped: Vec<(String, McpCredentialSkipReason)>,
}

/// Build a connect-ready clone of a server map with all `${cred:KEY}` header
/// and stdio `env` references resolved. A server whose reference cannot be
/// resolved is omitted:
/// sending the literal placeholder is both an authority error and a potential
/// credential-reference disclosure. Other servers continue independently. The
/// input map (the long-lived `Config`) is never mutated.
pub fn resolve_servers_for_connect(
    servers: &HashMap<String, McpServerConfig>,
    store: &dyn CredentialsStore,
) -> HashMap<String, McpServerConfig> {
    resolve_servers_for_connect_with_report(servers, store).connectable
}

pub fn resolve_servers_for_connect_with_report(
    servers: &HashMap<String, McpServerConfig>,
    store: &dyn CredentialsStore,
) -> McpServerResolution {
    let mut resolution = McpServerResolution::default();
    for (name, server) in servers {
        let mut resolved = server.clone();
        match resolve_server_credential_refs(&mut resolved, store) {
            Ok(()) => {
                resolution.connectable.insert(name.clone(), resolved);
            }
            Err(error) => {
                let reason = match error {
                    CredRefError::Missing { .. } => McpCredentialSkipReason::MissingCredential,
                    CredRefError::Store { .. } => {
                        McpCredentialSkipReason::CredentialStoreUnavailable
                    }
                    CredRefError::Malformed => McpCredentialSkipReason::MalformedReference,
                };
                tracing::warn!(
                    server = %name,
                    reason = reason.message(),
                    "MCP server skipped because its credential reference did not resolve"
                );
                resolution.skipped.push((name.clone(), reason));
            }
        }
    }
    resolution
        .skipped
        .sort_by(|left, right| left.0.cmp(&right.0));
    resolution
}

/// Keep only servers that do not require credential lookup. Used when the
/// credentials store itself cannot be opened; literal-value servers remain
/// usable while reference-bearing servers (headers or stdio `env`) fail closed
/// before transport spawn.
pub fn without_credential_references(
    servers: &HashMap<String, McpServerConfig>,
) -> HashMap<String, McpServerConfig> {
    without_credential_references_with_report(servers).connectable
}

pub fn without_credential_references_with_report(
    servers: &HashMap<String, McpServerConfig>,
) -> McpServerResolution {
    let mut resolution = McpServerResolution::default();
    for (name, server) in servers {
        if server_has_credential_references(server) {
            tracing::warn!(
                server = %name,
                "MCP server skipped because the credentials store is unavailable"
            );
            resolution.skipped.push((
                name.clone(),
                McpCredentialSkipReason::CredentialStoreUnavailable,
            ));
        } else {
            resolution.connectable.insert(name.clone(), server.clone());
        }
    }
    resolution
        .skipped
        .sort_by(|left, right| left.0.cmp(&right.0));
    resolution
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TransportType;

    /// A trivial in-memory store so resolution is testable without a backend.
    #[derive(Default)]
    struct MapStore(HashMap<String, String>);
    impl MapStore {
        fn with(key: &str, val: &str) -> Self {
            let mut m = HashMap::new();
            m.insert(key.to_string(), val.to_string());
            Self(m)
        }
    }
    impl CredentialsStore for MapStore {
        fn get(&self, key: &str) -> Result<Option<String>, CredentialsError> {
            Ok(self.0.get(key).cloned())
        }
        fn put(&self, _: &str, _: &str) -> Result<(), CredentialsError> {
            Ok(())
        }
        fn delete(&self, _: &str) -> Result<(), CredentialsError> {
            Ok(())
        }
    }

    fn stdio_server(env_key: &str, env_val: &str) -> McpServerConfig {
        let mut env = HashMap::new();
        env.insert(env_key.to_string(), env_val.to_string());
        McpServerConfig {
            transport: TransportType::Stdio,
            command: Some("vendor-mcp".to_string()),
            args: None,
            env: Some(env),
            url: None,
            headers: None,
            deferred: None,
            allow_local: false,
            only_for_assistant: None,
        }
    }

    fn http_server(header_val: &str) -> McpServerConfig {
        let mut headers = HashMap::new();
        headers.insert("Authorization".to_string(), header_val.to_string());
        McpServerConfig {
            transport: TransportType::StreamableHttp,
            command: None,
            args: None,
            env: None,
            url: Some("http://127.0.0.1:3456/mcp".to_string()),
            headers: Some(headers),
            deferred: None,
            allow_local: true,
            only_for_assistant: None,
        }
    }

    #[test]
    fn resolves_a_single_reference_inside_a_bearer() {
        let store = MapStore::with("mcp:agent-vault:token", "secret-xyz");
        let got = resolve_cred_refs("Bearer ${cred:mcp:agent-vault:token}", &store).unwrap();
        assert_eq!(got, "Bearer secret-xyz");
    }

    #[test]
    fn key_may_contain_colons() {
        // The `mcp:<server>:token` convention puts colons inside KEY; resolution
        // must stop at `}`, not at the first `:`.
        let store = MapStore::with("mcp:a:b:c:token", "deep");
        assert_eq!(
            resolve_cred_refs("${cred:mcp:a:b:c:token}", &store).unwrap(),
            "deep"
        );
    }

    #[test]
    fn resolves_multiple_references_in_one_value() {
        let mut m = HashMap::new();
        m.insert("k1".to_string(), "A".to_string());
        m.insert("k2".to_string(), "B".to_string());
        let store = MapStore(m);
        assert_eq!(
            resolve_cred_refs("${cred:k1}-${cred:k2}", &store).unwrap(),
            "A-B"
        );
    }

    #[test]
    fn value_without_reference_is_passed_through_without_touching_store() {
        // Empty store: a literal header must still resolve fine (no lookup).
        let store = MapStore::default();
        assert_eq!(
            resolve_cred_refs("Bearer static-token", &store).unwrap(),
            "Bearer static-token"
        );
    }

    #[test]
    fn missing_key_fails_closed() {
        let store = MapStore::default();
        match resolve_cred_refs("Bearer ${cred:absent}", &store) {
            Err(CredRefError::Missing { key }) => assert_eq!(key, "absent"),
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn unterminated_reference_is_malformed() {
        let store = MapStore::with("k", "v");
        assert!(matches!(
            resolve_cred_refs("Bearer ${cred:k", &store),
            Err(CredRefError::Malformed)
        ));
    }

    #[test]
    fn resolve_server_credential_refs_rewrites_headers_in_place() {
        let store = MapStore::with("mcp:agent-vault:token", "tok");
        let mut server = http_server("Bearer ${cred:mcp:agent-vault:token}");
        resolve_server_credential_refs(&mut server, &store).unwrap();
        assert_eq!(
            server.headers.unwrap().get("Authorization").unwrap(),
            "Bearer tok"
        );
    }

    #[test]
    fn map_resolver_skips_unresolved_servers_and_leaves_input_untouched() {
        let store = MapStore::with("mcp:ok:token", "good");
        let mut servers = HashMap::new();
        servers.insert("ok".to_string(), http_server("Bearer ${cred:mcp:ok:token}"));
        servers.insert(
            "broken".to_string(),
            http_server("Bearer ${cred:mcp:broken:token}"),
        );

        let report = resolve_servers_for_connect_with_report(&servers, &store);
        let resolved = report.connectable;

        // The resolvable server is concrete...
        assert_eq!(
            resolved["ok"].headers.as_ref().unwrap()["Authorization"],
            "Bearer good"
        );
        // ...the broken one is omitted before any transport can see a literal
        // credential reference...
        assert!(!resolved.contains_key("broken"));
        assert_eq!(
            report.skipped,
            vec![(
                "broken".to_string(),
                McpCredentialSkipReason::MissingCredential
            )]
        );
        // ...and the input map was never mutated.
        assert_eq!(
            servers["ok"].headers.as_ref().unwrap()["Authorization"],
            "Bearer ${cred:mcp:ok:token}"
        );
    }

    #[test]
    fn unavailable_store_path_keeps_only_literal_header_servers() {
        let servers = HashMap::from([
            ("literal".into(), http_server("Bearer static")),
            (
                "referenced".into(),
                http_server("Bearer ${cred:mcp:referenced:token}"),
            ),
        ]);
        let report = without_credential_references_with_report(&servers);
        assert!(report.connectable.contains_key("literal"));
        assert!(!report.connectable.contains_key("referenced"));
        assert_eq!(
            report.skipped,
            vec![(
                "referenced".to_string(),
                McpCredentialSkipReason::CredentialStoreUnavailable
            )]
        );
    }

    #[test]
    fn token_cred_key_is_namespaced_per_server() {
        assert_eq!(mcp_token_cred_key("agent-vault"), "mcp:agent-vault:token");
    }

    #[test]
    fn forge_server_config_carries_a_cred_ref_not_a_secret() {
        let key = mcp_token_cred_key("agent-vault");
        let cfg = build_forge_mcp_server_config("http://127.0.0.1:3456/mcp", &key);
        assert_eq!(cfg.transport, TransportType::StreamableHttp);
        assert!(cfg.allow_local);
        assert_eq!(cfg.url.as_deref(), Some("http://127.0.0.1:3456/mcp"));
        let auth = &cfg.headers.as_ref().unwrap()["Authorization"];
        assert_eq!(auth, "Bearer ${cred:mcp:agent-vault:token}");
        // The on-disk value must be a reference, never a literal token.
        assert!(auth.contains("${cred:"));

        // And it round-trips back through the resolver with the real token.
        let store = MapStore::with(&key, "live-token");
        let mut cfg2 = cfg;
        resolve_server_credential_refs(&mut cfg2, &store).unwrap();
        assert_eq!(cfg2.headers.unwrap()["Authorization"], "Bearer live-token");
    }

    // --- stdio `env` credential rail (fix/904) ---------------------------

    #[test]
    fn env_credential_reference_is_resolved_not_passed_through_literally() {
        let store = MapStore::with("mcp:vaulted:token", "s3cr3t");
        let mut server = stdio_server("VENDOR_API_KEY", "${cred:mcp:vaulted:token}");
        resolve_server_credential_refs(&mut server, &store).unwrap();
        let value = server.env.as_ref().unwrap()["VENDOR_API_KEY"].clone();
        assert!(
            !value.contains(CRED_PREFIX),
            "stdio env still carries a literal `${{cred:...}}`: the rail resolves \
             headers only, so a stdio MCP server has no way to reference a \
             stored credential and must keep its secret in cleartext config"
        );
        assert!(
            value == "s3cr3t",
            "stdio env value did not resolve to the stored credential"
        );
    }

    #[test]
    fn unresolvable_env_reference_fails_closed_instead_of_launching() {
        let store = MapStore::default();
        let servers = HashMap::from([(
            "stdio-broken".to_string(),
            stdio_server("VENDOR_API_KEY", "${cred:mcp:stdio-broken:token}"),
        )]);

        let report = resolve_servers_for_connect_with_report(&servers, &store);

        assert!(
            !report.connectable.contains_key("stdio-broken"),
            "a stdio server whose env credential reference cannot be resolved \
             was still handed to the transport: it would launch a child process \
             with a literal `${{cred:...}}` (or an empty value) in its environment"
        );
        assert_eq!(
            report.skipped,
            vec![(
                "stdio-broken".to_string(),
                McpCredentialSkipReason::MissingCredential
            )]
        );
    }

    #[test]
    fn unavailable_store_drops_stdio_servers_with_env_references() {
        let servers = HashMap::from([
            ("literal".to_string(), stdio_server("LOG_LEVEL", "debug")),
            (
                "referenced".to_string(),
                stdio_server("VENDOR_API_KEY", "${cred:mcp:referenced:token}"),
            ),
        ]);

        let report = without_credential_references_with_report(&servers);

        assert!(report.connectable.contains_key("literal"));
        assert!(
            !report.connectable.contains_key("referenced"),
            "with the credentials store unavailable, a stdio server whose env \
             holds a `${{cred:...}}` reference must be dropped, not launched with \
             the placeholder as its environment value"
        );
        assert_eq!(
            report.skipped,
            vec![(
                "referenced".to_string(),
                McpCredentialSkipReason::CredentialStoreUnavailable
            )]
        );
    }

    #[test]
    fn literal_env_value_is_not_rewritten_or_dropped() {
        // The rail must not be over-broad: a plain literal env value is passed
        // through byte-for-byte and never consults the store.
        let store = MapStore::default();
        let servers = HashMap::from([("plain".to_string(), stdio_server("LOG_LEVEL", "debug"))]);

        let report = resolve_servers_for_connect_with_report(&servers, &store);

        let env = report.connectable["plain"]
            .env
            .as_ref()
            .expect("literal env must survive resolution");
        assert!(
            env["LOG_LEVEL"] == "debug",
            "a literal env value must pass through the rail untouched"
        );
        assert!(report.skipped.is_empty());
    }

    #[test]
    fn malformed_env_reference_is_rejected_not_launched() {
        let store = MapStore::with("k", "v");
        let mut server = stdio_server("VENDOR_API_KEY", "${cred:k");
        assert!(
            matches!(
                resolve_server_credential_refs(&mut server, &store),
                Err(CredRefError::Malformed)
            ),
            "an unterminated `${{cred:` in env must be a hard error, not a \
             literal pass-through to the child process"
        );
    }

    #[test]
    fn env_reference_alone_marks_a_server_as_credential_bearing() {
        let server = stdio_server("VENDOR_API_KEY", "${cred:mcp:vendor:token}");
        assert!(
            server_has_credential_references(&server),
            "a stdio server whose only credential reference lives in env is not \
             recognised as credential-bearing, so store-unavailable paths let it \
             launch with the raw placeholder"
        );
    }
}
