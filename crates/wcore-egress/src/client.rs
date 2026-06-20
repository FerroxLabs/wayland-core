//! [`EgressClient`] — the workspace's single outbound HTTP client.
//!
//! This is the **only** place in the workspace permitted to construct a raw
//! [`reqwest::Client`]; a clippy `disallowed-methods` lint (see `clippy.toml`)
//! bans `reqwest::Client::new`/`builder` everywhere else. Every network-capable
//! crate depends up on `wcore-egress` and builds its client here, so the egress
//! policy is enforced in exactly one place.

use std::time::Duration;

use crate::error::EgressError;
use crate::policy::{SharedPolicy, default_policy};
use crate::request::EgressRequestBuilder;

/// A policy-gated HTTP client wrapping [`reqwest::Client`].
///
/// Cheap to clone (the inner reqwest client and the policy `Arc` are both
/// reference-counted). Construct via [`EgressClient::builder`] or the
/// [`EgressClient::streaming`] / [`EgressClient::tool`] presets, which carry the
/// hardened timeout + no-redirect policy that previously lived in
/// `wcore_providers::http_client`.
#[derive(Clone)]
pub struct EgressClient {
    inner: reqwest::Client,
    policy: SharedPolicy,
}

impl std::fmt::Debug for EgressClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The policy is a trait object (not Debug); show the inner client only.
        f.debug_struct("EgressClient")
            .field("inner", &self.inner)
            .finish_non_exhaustive()
    }
}

/// Default TCP+TLS connect timeout (was `wcore_providers::http_client::CONNECT_TIMEOUT`).
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// Default between-bytes read timeout for streaming clients
/// (was `wcore_providers::http_client::READ_TIMEOUT`). 300s tolerates long
/// server-side reasoning gaps without false-tripping.
pub const READ_TIMEOUT: Duration = Duration::from_secs(300);

/// Request-level wall-clock cap for non-streaming tool clients
/// (was `wcore_providers::http_client::TOOL_REQUEST_TIMEOUT`). Backstops a
/// slow-drip endpoint that the between-bytes read timeout cannot catch.
pub const TOOL_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);

impl EgressClient {
    /// A client with reqwest's defaults (no timeouts) and the default policy.
    /// Prefer [`EgressClient::streaming`] / [`EgressClient::tool`] for real work
    /// — a bare client with no timeouts can hang indefinitely.
    pub fn new() -> Self {
        Self::builder()
            .build()
            .expect("reqwest TLS backend must initialize at startup")
    }

    /// Start building a customized client.
    pub fn builder() -> EgressClientBuilder {
        EgressClientBuilder::new()
    }

    /// Preset for **streaming LLM providers**: 30s connect timeout, 300s
    /// between-bytes read timeout, redirects disabled, no request-level cap (a
    /// total timeout would truncate long generations). Replaces
    /// `wcore_providers::http_client::build()`.
    pub fn streaming() -> Self {
        Self::streaming_with_read_timeout(READ_TIMEOUT)
    }

    /// Like [`EgressClient::streaming`] but with a caller-chosen between-bytes
    /// read timeout (e.g. a thinking-heavy model that streams no bytes for
    /// minutes). Replaces `http_client::build_with_read_timeout`.
    pub fn streaming_with_read_timeout(read_timeout: Duration) -> Self {
        Self::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(read_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest TLS backend must initialize at startup")
    }

    /// Preset for **non-streaming HTTP tools** (REST/GraphQL backends): same
    /// connect + read timeouts as [`EgressClient::streaming`], PLUS a 300s
    /// request-level wall-clock cap that catches slow-drip servers. Replaces
    /// `wcore_providers::http_client::build_tool_client()`.
    pub fn tool() -> Self {
        Self::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .timeout(TOOL_REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("reqwest TLS backend must initialize at startup")
    }

    /// Replace this client's egress policy, returning the updated client.
    pub fn with_policy(mut self, policy: SharedPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// The egress policy this client enforces.
    pub fn policy(&self) -> &SharedPolicy {
        &self.policy
    }

    /// Start a `GET` request.
    pub fn get<U: reqwest::IntoUrl>(&self, url: U) -> EgressRequestBuilder {
        self.request(reqwest::Method::GET, url)
    }

    /// Start a `POST` request.
    pub fn post<U: reqwest::IntoUrl>(&self, url: U) -> EgressRequestBuilder {
        self.request(reqwest::Method::POST, url)
    }

    /// Start a `PUT` request.
    pub fn put<U: reqwest::IntoUrl>(&self, url: U) -> EgressRequestBuilder {
        self.request(reqwest::Method::PUT, url)
    }

    /// Start a `PATCH` request.
    pub fn patch<U: reqwest::IntoUrl>(&self, url: U) -> EgressRequestBuilder {
        self.request(reqwest::Method::PATCH, url)
    }

    /// Start a `DELETE` request.
    pub fn delete<U: reqwest::IntoUrl>(&self, url: U) -> EgressRequestBuilder {
        self.request(reqwest::Method::DELETE, url)
    }

    /// Start a `HEAD` request.
    pub fn head<U: reqwest::IntoUrl>(&self, url: U) -> EgressRequestBuilder {
        self.request(reqwest::Method::HEAD, url)
    }

    /// Start a request with an explicit method.
    pub fn request<U: reqwest::IntoUrl>(
        &self,
        method: reqwest::Method,
        url: U,
    ) -> EgressRequestBuilder {
        EgressRequestBuilder::new(
            self.inner.clone(),
            self.policy.clone(),
            self.inner.request(method, url),
        )
    }
}

impl Default for EgressClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Builder for [`EgressClient`]. Mirrors the [`reqwest::ClientBuilder`] methods
/// the workspace actually uses, plus [`EgressClientBuilder::policy`].
pub struct EgressClientBuilder {
    inner: reqwest::ClientBuilder,
    policy: Option<SharedPolicy>,
}

impl EgressClientBuilder {
    fn new() -> Self {
        Self {
            // The single sanctioned raw-reqwest construction in the workspace.
            #[allow(clippy::disallowed_methods)]
            inner: reqwest::Client::builder()
                // Connection-pool hygiene. Half-closed idle sockets are the
                // root of intermittent "error decoding response body" under
                // bursty LLM load: reqwest reuses a pooled connection the
                // server already dropped, and the next request fails mid-body.
                // Expire idle pooled connections quickly and probe liveness
                // with TCP keepalives so a dead socket is retired before reuse.
                // (Only affects IDLE pooled connections — an in-flight stream
                // holds its connection and is unaffected.)
                .pool_idle_timeout(Some(Duration::from_secs(20)))
                .tcp_keepalive(Some(Duration::from_secs(15))),
            policy: None,
        }
    }

    /// TCP+TLS handshake timeout.
    pub fn connect_timeout(mut self, timeout: Duration) -> Self {
        self.inner = self.inner.connect_timeout(timeout);
        self
    }

    /// Between-bytes read timeout (streaming hang guard).
    pub fn read_timeout(mut self, timeout: Duration) -> Self {
        self.inner = self.inner.read_timeout(timeout);
        self
    }

    /// Request-level wall-clock timeout (whole-request cap).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.inner = self.inner.timeout(timeout);
        self
    }

    /// Redirect-following policy. Pass [`reqwest::redirect::Policy::none`] to
    /// disable redirects (the provider/tool default — closes a credential
    /// re-attach exfil vector).
    pub fn redirect(mut self, policy: reqwest::redirect::Policy) -> Self {
        self.inner = self.inner.redirect(policy);
        self
    }

    /// Idle-connection pool timeout.
    pub fn pool_idle_timeout<D: Into<Option<Duration>>>(mut self, value: D) -> Self {
        self.inner = self.inner.pool_idle_timeout(value);
        self
    }

    /// Route requests through an HTTP/HTTPS proxy (e.g. `HTTPS_PROXY`).
    pub fn proxy(mut self, proxy: reqwest::Proxy) -> Self {
        self.inner = self.inner.proxy(proxy);
        self
    }

    /// Pin a domain to a fixed set of socket addresses (SSRF resolve-once: the
    /// plugin-wasm host adapter validates an IP then dials only that IP, so a
    /// redirect cannot rebind the host to the metadata address).
    pub fn resolve_to_addrs(mut self, domain: &str, addrs: &[std::net::SocketAddr]) -> Self {
        self.inner = self.inner.resolve_to_addrs(domain, addrs);
        self
    }

    /// Override DNS resolution. Used by the MCP transports to install the
    /// SSRF-safe resolver (dial only validated public IPs — closes the
    /// check→connect rebind window).
    pub fn dns_resolver<R: reqwest::dns::Resolve + 'static>(
        mut self,
        resolver: std::sync::Arc<R>,
    ) -> Self {
        self.inner = self.inner.dns_resolver(resolver);
        self
    }

    /// Set the default `User-Agent` header.
    pub fn user_agent<V>(mut self, value: V) -> Self
    where
        V: TryInto<reqwest::header::HeaderValue>,
        V::Error: Into<http::Error>,
    {
        self.inner = self.inner.user_agent(value);
        self
    }

    /// Set headers attached to every request from this client.
    pub fn default_headers(mut self, headers: reqwest::header::HeaderMap) -> Self {
        self.inner = self.inner.default_headers(headers);
        self
    }

    /// Disable certificate validation. **Dangerous** — only for explicit
    /// opt-in test/local scenarios that already do this with raw reqwest.
    pub fn danger_accept_invalid_certs(mut self, accept: bool) -> Self {
        self.inner = self.inner.danger_accept_invalid_certs(accept);
        self
    }

    /// Set the egress policy. Defaults to the pass-through policy when omitted.
    pub fn policy(mut self, policy: SharedPolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// Build the [`EgressClient`].
    pub fn build(self) -> Result<EgressClient, EgressError> {
        let inner = self.inner.build()?;
        Ok(EgressClient {
            inner,
            policy: self.policy.unwrap_or_else(default_policy),
        })
    }
}
