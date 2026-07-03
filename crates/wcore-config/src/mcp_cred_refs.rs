//! `${cred:KEY}` credential-reference resolution for MCP server headers
//! (Slice 3, Piece 2).
//!
//! MCP `[mcp.servers.*]` `headers` in `config.toml` are literal strings. To keep
//! a bearer token OUT of `config.toml`, a header value may embed a reference of
//! the form `${cred:KEY}`, e.g.
//!
//! ```toml
//! [mcp.servers.agent-vault]
//! transport = "streamable-http"
//! url = "http://127.0.0.1:3456/mcp"
//! allow_local = true
//! [mcp.servers.agent-vault.headers]
//! Authorization = "Bearer ${cred:mcp:agent-vault:token}"
//! ```
//!
//! The literal `${cred:...}` stays on disk; the real secret is looked up from
//! the [`CredentialsStore`] and substituted in **at the connect boundary, on a
//! clone** of the server map — never written back into the long-lived in-memory
//! `Config`, so an accidental re-serialize can't leak the token to disk. `KEY`
//! is everything between `${cred:` and the next `}` (so it may itself contain
//! `:`, as the `mcp:<server>:token` convention does).
//!
//! A server whose header carries no `${cred:` reference is passed through
//! untouched and never touches the store — existing literal-header MCP servers
//! are unaffected even when the store is empty or locked.

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
    #[error("malformed credential reference (unterminated `${{cred:...}}`) in header value")]
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

/// Resolve `${cred:KEY}` references in every header value of one server, in
/// place. Used by the single-server live-add path where a resolution failure is
/// a hard error the user sees (they just asked to connect this server).
pub fn resolve_server_headers(
    server: &mut McpServerConfig,
    store: &dyn CredentialsStore,
) -> Result<(), CredRefError> {
    if let Some(headers) = server.headers.as_mut() {
        for v in headers.values_mut() {
            if v.contains(CRED_PREFIX) {
                *v = resolve_cred_refs(v, store)?;
            }
        }
    }
    Ok(())
}

/// Build a connect-ready clone of a server map with all `${cred:KEY}` header
/// references resolved. Best-effort per server: a server whose reference cannot
/// be resolved is left with its literal header (a warning is logged) so it fails
/// its own connect in isolation instead of blocking every other server at boot.
/// The input map (the long-lived `Config`) is never mutated.
pub fn resolve_servers_for_connect(
    servers: &HashMap<String, McpServerConfig>,
    store: &dyn CredentialsStore,
) -> HashMap<String, McpServerConfig> {
    let mut out = servers.clone();
    for (name, server) in out.iter_mut() {
        if let Err(e) = resolve_server_headers(server, store) {
            tracing::warn!(
                server = %name,
                error = %e,
                "MCP server credential reference did not resolve; \
                 connecting with the literal header (its own connect will fail \
                 if the header is required)"
            );
        }
    }
    out
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
    fn resolve_server_headers_rewrites_in_place() {
        let store = MapStore::with("mcp:agent-vault:token", "tok");
        let mut server = http_server("Bearer ${cred:mcp:agent-vault:token}");
        resolve_server_headers(&mut server, &store).unwrap();
        assert_eq!(
            server.headers.unwrap().get("Authorization").unwrap(),
            "Bearer tok"
        );
    }

    #[test]
    fn map_resolver_is_best_effort_and_leaves_input_untouched() {
        let store = MapStore::with("mcp:ok:token", "good");
        let mut servers = HashMap::new();
        servers.insert("ok".to_string(), http_server("Bearer ${cred:mcp:ok:token}"));
        servers.insert(
            "broken".to_string(),
            http_server("Bearer ${cred:mcp:broken:token}"),
        );

        let resolved = resolve_servers_for_connect(&servers, &store);

        // The resolvable server is concrete...
        assert_eq!(
            resolved["ok"].headers.as_ref().unwrap()["Authorization"],
            "Bearer good"
        );
        // ...the broken one keeps its literal (fails its own connect later)...
        assert_eq!(
            resolved["broken"].headers.as_ref().unwrap()["Authorization"],
            "Bearer ${cred:mcp:broken:token}"
        );
        // ...and the input map was never mutated.
        assert_eq!(
            servers["ok"].headers.as_ref().unwrap()["Authorization"],
            "Bearer ${cred:mcp:ok:token}"
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
        resolve_server_headers(&mut cfg2, &store).unwrap();
        assert_eq!(cfg2.headers.unwrap()["Authorization"], "Bearer live-token");
    }
}
