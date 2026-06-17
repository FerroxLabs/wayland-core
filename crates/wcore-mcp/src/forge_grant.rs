//! Forge local-MCP grant client — consumer side (Slice 3, Piece 1).
//!
//! Once [`wcore_config::forge_discovery`] has surfaced a discovered Forge server,
//! this module performs the two unauthenticated bootstrap HTTP calls that turn a
//! *hint* into a *connectable, authorized* server:
//!
//! 1. [`probe_metadata`] — `GET <metadata_url>` liveness check. A discovery
//!    entry is only a hint (a producer crash leaves a stale entry), so we never
//!    offer to connect until a `200` whose `name` matches the entry proves the
//!    server is alive and is who it claims to be.
//! 2. [`request_grant`] — `POST <grant_url>` to mint a scoped bearer token. The
//!    `200` path only returns *after the user clicks Approve* in the producer's
//!    GUI; everything else (deny / bad scopes / rate-limit) is mapped to a
//!    structured [`GrantOutcome`] the caller surfaces without treating it as a
//!    hard crash.
//!
//! Wire contract is frozen + live-verified by the producer (Agent Vault):
//! `.audit/waylandcore-mcp-fix/FORGE-MCP-DISCOVERY-SPEC.md` §2 (metadata) / §3
//! (grant). Do NOT change the shapes here.
//!
//! ## Network boundary
//! Every request goes through an [`wcore_egress::EgressClient`] built with the
//! **loopback-permitting** SSRF policy ([`loopback_http_client`]), mirroring
//! [`crate::transport::streamable_http`]'s `allow_local` branch: the resolver +
//! redirect policy dial `127.0.0.1`/`::1` but keep every other private / LAN /
//! link-local / CGNAT / cloud-metadata range blocked. We additionally gate each
//! URL through [`is_safe_url_allow_loopback`](wcore_tools::url_safety::is_safe_url_allow_loopback)
//! *before* sending, failing closed on a non-loopback private target — a Forge
//! server is always on loopback, so a discovery file pointing anywhere else is
//! rejected, not dialed.

use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;

use wcore_tools::url_safety::{
    LoopbackOkResolver, is_safe_url_allow_loopback, ssrf_safe_redirect_policy_allow_loopback,
};

/// Liveness/metadata response from `GET <metadata_url>` (spec §2).
///
/// Tolerant: only `name` is required (it is what we name-match against the
/// discovery entry). Unknown fields are ignored so a newer producer never
/// breaks an older consumer.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Metadata {
    /// Stable machine id — must equal the discovery entry's `name`.
    pub name: String,
    /// Human label (producer may localize/rename).
    #[serde(default)]
    pub display_name: Option<String>,
    /// Number of tools the server currently exposes (display-only).
    #[serde(default)]
    pub tool_count: Option<u32>,
}

/// Outcome of a `POST <grant_url>` token request (spec §3).
///
/// Every non-200 is a *recoverable* outcome the UI surfaces as guidance, not a
/// transport crash — a denied grant means "ask again with Approve", a 400 means
/// "fix the scopes", a 429 means "back off".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantOutcome {
    /// `200` — user clicked Approve. Carries the bearer token + granted scopes
    /// (server-clamped to a subset of `read`/`write`).
    Granted { token: String, scopes: Vec<String> },
    /// `403` — denied, timed out, or no UI present (deny-by-default).
    Denied,
    /// `400` — empty/invalid scopes. Carries the server's human message.
    BadScopes(String),
    /// `429` — grant rate-limit (~10/min/peer). Back off; do not hammer.
    RateLimited,
    /// Any other status, a non-loopback/SSRF-rejected URL, a transport failure,
    /// or an unparseable success body. Carries a diagnostic string.
    Error(String),
}

/// Error envelope shape the producer returns on 4xx (`{"error": "..."}`).
#[derive(Debug, Deserialize)]
struct ErrorBody {
    #[serde(default)]
    error: Option<String>,
}

/// Successful grant body (`{"token": "...", "scopes": ["read","write"]}`).
#[derive(Debug, Deserialize)]
struct GrantBody {
    token: String,
    #[serde(default)]
    scopes: Vec<String>,
}

/// Build the loopback-permitting HTTP client used for the metadata + grant
/// calls. Short timeouts (a local server answers in milliseconds; a stale entry
/// must fail fast, not hang the connect flow). Mirrors the `allow_local` branch
/// of [`crate::transport::streamable_http::StreamableHttpTransport::connect`].
pub fn loopback_http_client() -> wcore_egress::EgressClient {
    wcore_egress::EgressClient::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .redirect(ssrf_safe_redirect_policy_allow_loopback())
        .dns_resolver(Arc::new(LoopbackOkResolver))
        .build()
        .unwrap_or_else(|_| wcore_egress::EgressClient::new())
}

/// `GET <metadata_url>` and confirm the server is alive AND is the server the
/// discovery entry named.
///
/// Returns `Some(metadata)` only on a `200` whose `name` equals `expected_name`.
/// Any non-200, a name mismatch, a non-loopback URL, a transport failure, or an
/// unparseable body → `None` (treat the discovery entry as stale; do not offer
/// to connect).
pub async fn probe_metadata(
    client: &wcore_egress::EgressClient,
    metadata_url: &str,
    expected_name: &str,
) -> Option<Metadata> {
    if !is_safe_url_allow_loopback(metadata_url) {
        return None;
    }
    let resp = client.get(metadata_url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let meta = resp.json::<Metadata>().await.ok()?;
    (meta.name == expected_name).then_some(meta)
}

/// `POST <grant_url>` with `{client_name, scopes}` and map the response to a
/// [`GrantOutcome`] (spec §3).
///
/// `client_name` is displayed-but-untrusted by the producer — it is shown to the
/// user verbatim in the Approve modal. `scopes` is clamped server-side to a
/// subset of `read`/`write`.
pub async fn request_grant(
    client: &wcore_egress::EgressClient,
    grant_url: &str,
    client_name: &str,
    scopes: &[&str],
) -> GrantOutcome {
    if !is_safe_url_allow_loopback(grant_url) {
        return GrantOutcome::Error(format!(
            "grant url rejected — resolves to a non-loopback/internal address \
             (SSRF guard): {grant_url}"
        ));
    }

    let body = serde_json::json!({ "client_name": client_name, "scopes": scopes });
    let resp = match client.post(grant_url).json(&body).send().await {
        Ok(r) => r,
        Err(e) => return GrantOutcome::Error(format!("grant request failed: {e}")),
    };

    let status = resp.status().as_u16();
    match status {
        200 => match resp.json::<GrantBody>().await {
            Ok(g) => GrantOutcome::Granted {
                token: g.token,
                scopes: g.scopes,
            },
            Err(e) => GrantOutcome::Error(format!("grant 200 but unparseable body: {e}")),
        },
        403 => GrantOutcome::Denied,
        400 => {
            let msg = resp
                .json::<ErrorBody>()
                .await
                .ok()
                .and_then(|b| b.error)
                .unwrap_or_else(|| "invalid scopes".to_string());
            GrantOutcome::BadScopes(msg)
        }
        429 => GrantOutcome::RateLimited,
        other => GrantOutcome::Error(format!("unexpected grant status {other}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// The frozen `GET /.well-known/mcp` shape (spec §2) with a matching name is
    /// accepted and its display fields are surfaced.
    #[tokio::test]
    async fn probe_accepts_live_matching_server() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "agent-vault",
                "display_name": "Agent Vault",
                "version": "1.0.0",
                "transport": "streamable-http",
                "url": "http://127.0.0.1:3456/mcp",
                "auth": { "scheme": "loopback-grant", "grant_url": "http://127.0.0.1:3456/grant" },
                "tool_count": 13
            })))
            .mount(&server)
            .await;

        let client = loopback_http_client();
        let url = format!("{}/.well-known/mcp", server.uri());
        let meta = probe_metadata(&client, &url, "agent-vault").await;

        let meta = meta.expect("a live matching server must probe Some");
        assert_eq!(meta.name, "agent-vault");
        assert_eq!(meta.display_name.as_deref(), Some("Agent Vault"));
        assert_eq!(meta.tool_count, Some(13));
    }

    /// A live server whose `name` does NOT match the discovery entry is treated
    /// as stale/impostor — `None`, never offered for connect.
    #[tokio::test]
    async fn probe_rejects_name_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/mcp"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "name": "some-other-app",
                "tool_count": 1
            })))
            .mount(&server)
            .await;

        let client = loopback_http_client();
        let url = format!("{}/.well-known/mcp", server.uri());
        assert!(probe_metadata(&client, &url, "agent-vault").await.is_none());
    }

    /// A non-200 (server up but unhealthy / wrong route) → stale → `None`.
    #[tokio::test]
    async fn probe_rejects_non_200() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/.well-known/mcp"))
            .respond_with(ResponseTemplate::new(503))
            .mount(&server)
            .await;

        let client = loopback_http_client();
        let url = format!("{}/.well-known/mcp", server.uri());
        assert!(probe_metadata(&client, &url, "agent-vault").await.is_none());
    }

    /// A metadata URL that resolves off-loopback is rejected before any I/O —
    /// the SSRF gate fails closed (no request is even attempted).
    #[tokio::test]
    async fn probe_rejects_non_loopback_url() {
        let client = loopback_http_client();
        let got = probe_metadata(
            &client,
            "http://169.254.169.254/.well-known/mcp",
            "agent-vault",
        )
        .await;
        assert!(got.is_none());
    }

    /// `200` after Approve → `Granted` with the token + clamped scopes, and the
    /// request carries the frozen `{client_name, scopes}` body (spec §3).
    #[tokio::test]
    async fn grant_200_yields_token_and_sends_contract_body() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/grant"))
            .and(body_partial_json(
                json!({ "client_name": "Wayland Core", "scopes": ["read", "write"] }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "token": "abc123.base64url.bearer",
                "scopes": ["read", "write"]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = loopback_http_client();
        let url = format!("{}/grant", server.uri());
        let outcome = request_grant(&client, &url, "Wayland Core", &["read", "write"]).await;

        assert_eq!(
            outcome,
            GrantOutcome::Granted {
                token: "abc123.base64url.bearer".to_string(),
                scopes: vec!["read".to_string(), "write".to_string()],
            }
        );
    }

    /// `403` (user denied / no UI / timeout) → `Denied`, not an error.
    #[tokio::test]
    async fn grant_403_is_denied() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/grant"))
            .respond_with(
                ResponseTemplate::new(403).set_body_json(json!({ "error": "Grant denied." })),
            )
            .mount(&server)
            .await;

        let client = loopback_http_client();
        let url = format!("{}/grant", server.uri());
        assert_eq!(
            request_grant(&client, &url, "Wayland Core", &["read", "write"]).await,
            GrantOutcome::Denied
        );
    }

    /// `400` → `BadScopes` carrying the server's human message.
    #[tokio::test]
    async fn grant_400_is_bad_scopes_with_message() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/grant"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": "No valid scopes requested. Valid scopes: read, write."
            })))
            .mount(&server)
            .await;

        let client = loopback_http_client();
        let url = format!("{}/grant", server.uri());
        match request_grant(&client, &url, "Wayland Core", &[]).await {
            GrantOutcome::BadScopes(msg) => assert!(msg.contains("Valid scopes")),
            other => panic!("expected BadScopes, got {other:?}"),
        }
    }

    /// `429` → `RateLimited` (caller backs off).
    #[tokio::test]
    async fn grant_429_is_rate_limited() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/grant"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({
                "error": "Too many grant requests. Try again shortly."
            })))
            .mount(&server)
            .await;

        let client = loopback_http_client();
        let url = format!("{}/grant", server.uri());
        assert_eq!(
            request_grant(&client, &url, "Wayland Core", &["read", "write"]).await,
            GrantOutcome::RateLimited
        );
    }

    /// An off-loopback grant URL is SSRF-rejected before any request.
    #[tokio::test]
    async fn grant_rejects_non_loopback_url() {
        let client = loopback_http_client();
        match request_grant(&client, "http://10.0.0.5/grant", "Wayland Core", &["read"]).await {
            GrantOutcome::Error(msg) => assert!(msg.contains("SSRF") || msg.contains("loopback")),
            other => panic!("expected SSRF Error, got {other:?}"),
        }
    }
}
