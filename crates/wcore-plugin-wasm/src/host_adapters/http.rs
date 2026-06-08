//! HTTP host capability — `Deny*` default + real `Gated*` egress.
//!
//! `GatedHostHttp` is the production HTTP egress path for WASM plugins. It is
//! built by the runner only when `manifest.permissions.allow_network = true`;
//! all other plugins get [`DenyHostHttp`] which fails closed.
//!
//! Security posture (defense-in-depth):
//!
//! 1. **Allowlist**: every request URL host must match a pattern in
//!    `manifest.permissions.http_allowlist`. Empty list = nothing reachable.
//! 2. **Host-side secret injection**: outbound headers / body may contain
//!    `{{secret:NAME}}` tokens. The host expands them by reading the secret
//!    value from a [`SecretSource`] IFF `NAME` is in
//!    `manifest.permissions.permitted_secrets`. Secret VALUES never reach the
//!    WASM guest — only the substituted bytes go on the wire.
//! 3. **Response leak-scan**: the response body is scanned for any byte
//!    string equal to a known secret value (restricted to this plugin's
//!    permitted_secrets). Matches are replaced with `<REDACTED>` before the
//!    body is returned to the guest. Guards against an upstream echoing back
//!    a secret it received (intentional or accidental).
//! 4. **Body cap**: per-request body cap of 10 MiB; oversized responses
//!    return `BodyTooLarge`.
//! 5. **Timeout**: 30s default request timeout.
//! 6. **TLS**: rustls (no openssl).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use wcore_plugin_api::access_gate::PluginAccessGate;

/// Hard cap on response body size returned to a WASM guest (10 MiB).
pub const MAX_BODY_BYTES: usize = 10 * 1024 * 1024;

/// Default per-request timeout.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Errors surfaced across the WASM/host HTTP seam.
#[derive(Debug, thiserror::Error)]
pub enum HostHttpError {
    #[error("permission denied: network")]
    PermissionDenied,
    #[error("invalid url")]
    InvalidUrl,
    #[error("blocked: url targets a private or internal network address")]
    BlockedUrl,
    #[error("invalid method")]
    InvalidMethod,
    #[error("invalid header: {0}")]
    InvalidHeader(String),
    #[error("response body too large")]
    BodyTooLarge,
    #[error("network: {0}")]
    Network(String),
    #[error("client build: {0}")]
    ClientBuild(String),
}

// ---------------------------------------------------------------------------
// SSRF guard
//
// Minimal, self-contained mirror of `wcore_tools::url_safety`. Replicated here
// (rather than depending on wcore-tools) to keep the wasm-host dependency graph
// narrow — wcore-tools pulls sandbox/memory/repomap, none of which this crate
// needs. Keep the deny-set logic in sync with that module if it changes.
//
// Closes H-1/plugins-6: the host-glob allowlist is a string match only; without
// this every network-granted WASM plugin could reach 169.254.169.254 (cloud
// IAM creds), loopback admin ports, RFC1918 hosts, or follow an allowlisted
// host's 302 redirect into the same.
// ---------------------------------------------------------------------------

/// Cloud-metadata hostnames blocked regardless of IP resolution.
const CLOUD_METADATA_HOSTNAMES: &[&str] = &[
    "metadata.google.internal",
    "metadata.goog",
    "metadata",
    "kubernetes.default",
    "kubernetes.default.svc",
    "kubernetes.default.svc.cluster.local",
];

/// Specific cloud-metadata IPv4 addresses that must always be blocked.
const CLOUD_METADATA_IPV4: &[Ipv4Addr] = &[
    Ipv4Addr::new(169, 254, 169, 254), // AWS / GCP / Azure / DO / Oracle
    Ipv4Addr::new(169, 254, 170, 2),   // AWS ECS task metadata
    Ipv4Addr::new(169, 254, 169, 253), // Azure IMDS wire server
    Ipv4Addr::new(100, 100, 100, 200), // Alibaba Cloud metadata
    Ipv4Addr::new(192, 0, 0, 192),     // Oracle Cloud metadata (legacy)
    Ipv4Addr::new(10, 96, 0, 1),       // Kubernetes default API server ClusterIP
];

/// AWS IPv6 metadata endpoint (fd00:ec2::254).
const AWS_METADATA_IPV6: Ipv6Addr = Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254);

fn ip_is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if CLOUD_METADATA_IPV4.contains(&v4) {
                return true;
            }
            if v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_documentation()
            {
                return true;
            }
            let o = v4.octets();
            // 100.64.0.0/10 CGNAT (RFC 6598) — not flagged by std.
            if o[0] == 100 && (o[1] & 0b1100_0000) == 0b0100_0000 {
                return true;
            }
            // 0.0.0.0/8, 192.0.0.0/24, 198.18.0.0/15, 240.0.0.0/4.
            if o[0] == 0
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                || (o[0] == 198 && (o[1] == 18 || o[1] == 19))
                || o[0] >= 240
            {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            if v6 == AWS_METADATA_IPV6 {
                return true;
            }
            if v6.is_loopback() || v6.is_multicast() || v6.is_unspecified() {
                return true;
            }
            let seg0 = v6.segments()[0];
            // Unique local fc00::/7 + link-local fe80::/10.
            if (seg0 & 0xfe00) == 0xfc00 || (seg0 & 0xffc0) == 0xfe80 {
                return true;
            }
            // IPv4-mapped / -compatible forms — re-check via IPv4 rules.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return ip_is_blocked(IpAddr::V4(mapped));
            }
            if let Some(compat) = v6.to_ipv4()
                && (compat.is_unspecified() || compat.is_loopback())
            {
                return true;
            }
            false
        }
    }
}

/// Resolver seam for tests. Production uses the system resolver.
type Resolver = fn(&str) -> Vec<IpAddr>;

fn system_resolver(host: &str) -> Vec<IpAddr> {
    match (host, 0u16).to_socket_addrs() {
        Ok(iter) => iter.map(|sa| sa.ip()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Return `true` if `url` is safe to fetch (not private/internal/metadata).
///
/// Rejects non-http(s) schemes, blocked literal IPs, blocked hostnames, and
/// hostnames that resolve to any blocked IP. **Fails closed** on parse / DNS
/// failure.
fn is_safe_url(url: &str) -> bool {
    is_safe_url_with(url, system_resolver)
}

fn is_safe_url_with(url: &str, resolve: Resolver) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    match parsed.scheme() {
        "http" | "https" => {}
        _ => return false,
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let mut hostname = host.trim().to_ascii_lowercase();
    while hostname.ends_with('.') {
        hostname.pop();
    }
    if hostname.is_empty() {
        return false;
    }

    if CLOUD_METADATA_HOSTNAMES.iter().any(|h| *h == hostname) {
        return false;
    }

    // Literal IP (incl. bracketed IPv6) — skip DNS.
    if let Ok(ip) = hostname.parse::<IpAddr>() {
        return !ip_is_blocked(ip);
    }
    if let Some(ip) = hostname
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .and_then(|s| s.parse::<IpAddr>().ok())
    {
        return !ip_is_blocked(ip);
    }

    let addrs = resolve(&hostname);
    if addrs.is_empty() {
        // DNS failure — fail closed.
        return false;
    }
    !addrs.into_iter().any(ip_is_blocked)
}

/// Test-only: is `url`'s host a loopback literal? Used solely by the
/// loopback test-exemption so wiremock servers (always `127.0.0.1`) are
/// reachable in unit tests; production code never calls this.
#[cfg(test)]
fn url_is_loopback(url: &str) -> bool {
    let Ok(parsed) = url::Url::parse(url) else {
        return false;
    };
    match parsed.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        None => false,
    }
}

/// Host-side secret store. Read-only on the host; never crosses the WASM
/// seam. Implementations resolve a name to a value or `None`.
pub trait SecretSource: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
}

/// Default `SecretSource` that knows about nothing. Used when the engine
/// has no secret store wired yet — secret injection becomes a no-op and the
/// leak-scan has no values to look for.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptySecretSource;

impl SecretSource for EmptySecretSource {
    fn get(&self, _name: &str) -> Option<String> {
        None
    }
}

/// HTTP response shape returned across the WASM/host seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
}

/// Local trait — Task 2.7 will wire this to the wasmtime-generated host
/// import via the `Linker`.
///
/// `headers` are guest-supplied request headers (decoded from the WIT
/// `http-req.headers-json` field). Each value MAY carry `{{secret:NAME}}`
/// tokens that the host expands IFF `NAME` is in the plugin's
/// `permitted_secrets` — secret VALUES never reach the guest. Aud-13 closed
/// the gap where these headers were silently dropped.
#[async_trait]
pub trait WaylandHostHttp: Send + Sync {
    async fn http_request(
        &self,
        url: String,
        method: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<HttpResponse, HostHttpError>;
}

/// Fail-closed HTTP host. Every request denied.
#[derive(Debug, Default)]
pub struct DenyHostHttp;

#[async_trait]
impl WaylandHostHttp for DenyHostHttp {
    async fn http_request(
        &self,
        _url: String,
        _method: String,
        _headers: Vec<(String, String)>,
        _body: Vec<u8>,
    ) -> Result<HttpResponse, HostHttpError> {
        Err(HostHttpError::PermissionDenied)
    }
}

/// Capability-gated HTTP host with real `reqwest`-backed egress.
pub struct GatedHostHttp {
    #[allow(dead_code)] // Held for the day the gate becomes stateful.
    gate: Arc<PluginAccessGate>,
    plugin: String,
    allowlist: Vec<glob::Pattern>,
    permitted_secrets: Vec<String>,
    secrets: Arc<dyn SecretSource>,
    max_body: usize,
    timeout: Duration,
    /// DNS resolver used to pin connect-time addresses (H-1 TOCTOU guard).
    /// Production uses [`system_resolver`]; tests override it via the
    /// `Resolver` fn-seam to drive the rebinding scenario.
    resolver: Resolver,
    /// Test-only: permit loopback targets so the wiremock-backed egress /
    /// leak-scan / body-cap integration tests can exercise the real network
    /// path against `127.0.0.1`. NEVER set outside tests — production always
    /// runs the full SSRF guard.
    #[cfg(test)]
    allow_loopback_for_test: bool,
}

impl GatedHostHttp {
    /// Build a gated HTTP adapter.
    ///
    /// `allowlist_patterns` are host glob patterns (e.g. `api.example.com`,
    /// `*.example.com`). Invalid patterns are silently dropped with a
    /// warning — they can never match, which keeps fail-closed behavior.
    pub fn new(
        gate: Arc<PluginAccessGate>,
        plugin: String,
        allowlist_patterns: &[String],
        permitted_secrets: Vec<String>,
        secrets: Arc<dyn SecretSource>,
    ) -> Self {
        let allowlist = allowlist_patterns
            .iter()
            .filter_map(|p| match glob::Pattern::new(p) {
                Ok(pat) => Some(pat),
                Err(e) => {
                    tracing::warn!(plugin = %plugin, pattern = %p, error = %e,
                        "gated http: dropping invalid allowlist pattern");
                    None
                }
            })
            .collect();
        Self {
            gate,
            plugin,
            allowlist,
            permitted_secrets,
            secrets,
            max_body: MAX_BODY_BYTES,
            timeout: DEFAULT_TIMEOUT,
            resolver: system_resolver,
            #[cfg(test)]
            allow_loopback_for_test: false,
        }
    }

    /// Override the response-body cap (for tests).
    #[cfg(test)]
    fn with_max_body(mut self, max: usize) -> Self {
        self.max_body = max;
        self
    }

    /// Test-only: relax the SSRF guard to permit loopback so the
    /// wiremock-backed integration tests can hit `127.0.0.1`.
    #[cfg(test)]
    fn allowing_loopback_for_test(mut self) -> Self {
        self.allow_loopback_for_test = true;
        self
    }

    /// Whether the URL passes the SSRF guard. In production this is exactly
    /// [`is_safe_url`]; under test the loopback exemption (when explicitly
    /// enabled) lets wiremock servers be reached.
    #[cfg(not(test))]
    fn ssrf_allows(&self, url: &str) -> bool {
        is_safe_url(url)
    }

    #[cfg(test)]
    fn ssrf_allows(&self, url: &str) -> bool {
        is_safe_url(url) || (self.allow_loopback_for_test && url_is_loopback(url))
    }

    fn host_allowed(&self, host: &str) -> bool {
        self.allowlist.iter().any(|p| p.matches(host))
    }

    /// Test-only: override the DNS resolver to drive the rebinding scenario.
    #[cfg(test)]
    fn with_resolver(mut self, resolver: Resolver) -> Self {
        self.resolver = resolver;
        self
    }

    /// Resolve `host` ONCE and return the connect-time [`SocketAddr`]s to pin
    /// into the reqwest client, closing the DNS-rebinding TOCTOU (H-1).
    ///
    /// `is_safe_url`/`ssrf_allows` validated the host pre-flight, but reqwest
    /// dials by hostname and would re-resolve at connect time — an attacker
    /// resolver (TTL=0) could return a public IP for the check and
    /// 169.254.169.254 for the connect. By resolving here and pinning the
    /// validated IPs via `resolve_to_addrs`, reqwest never re-resolves.
    ///
    /// **Fails closed**: returns `BlockedUrl` if resolution is empty or ANY
    /// resolved IP is blocked. For a literal-IP host the single IP is pinned.
    /// Under test, a loopback literal/`localhost` is pinned as-is so wiremock
    /// servers remain reachable without reopening the prod gap.
    fn pinned_safe_addrs(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, HostHttpError> {
        let mut hostname = host.trim().to_ascii_lowercase();
        while hostname.ends_with('.') {
            hostname.pop();
        }
        let bracketed = hostname
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .map(str::to_string);
        let ip_literal = hostname
            .parse::<IpAddr>()
            .ok()
            .or_else(|| bracketed.as_deref().and_then(|s| s.parse::<IpAddr>().ok()));

        // Literal-IP host: pin that single address (already validated above).
        if let Some(ip) = ip_literal {
            #[cfg(test)]
            let permit = !ip_is_blocked(ip) || (self.allow_loopback_for_test && ip.is_loopback());
            #[cfg(not(test))]
            let permit = !ip_is_blocked(ip);
            if !permit {
                return Err(HostHttpError::BlockedUrl);
            }
            return Ok(vec![SocketAddr::new(ip, port)]);
        }

        let addrs = (self.resolver)(&hostname);
        if addrs.is_empty() {
            return Err(HostHttpError::BlockedUrl);
        }
        // Fail closed: every resolved IP must pass. The loopback test-exemption
        // still pins the resolved loopback addr (it does not skip validation in
        // a way that would reopen the prod gap).
        for ip in &addrs {
            #[cfg(test)]
            let permit = !ip_is_blocked(*ip) || (self.allow_loopback_for_test && ip.is_loopback());
            #[cfg(not(test))]
            let permit = !ip_is_blocked(*ip);
            if !permit {
                return Err(HostHttpError::BlockedUrl);
            }
        }
        Ok(addrs
            .into_iter()
            .map(|ip| SocketAddr::new(ip, port))
            .collect())
    }

    /// Expand `{{secret:NAME}}` tokens. Only `NAME`s in `permitted_secrets`
    /// AND known to the `SecretSource` are expanded; everything else passes
    /// through unchanged (keeps the token literal so the upstream sees the
    /// failure rather than the host silently dropping it).
    fn expand_secrets(&self, input: &str) -> String {
        // Cheap path: no token marker.
        if !input.contains("{{secret:") {
            return input.to_string();
        }
        let mut out = String::with_capacity(input.len());
        let mut rest = input;
        while let Some(start) = rest.find("{{secret:") {
            out.push_str(&rest[..start]);
            let after = &rest[start + "{{secret:".len()..];
            if let Some(end) = after.find("}}") {
                let name = &after[..end];
                let tail = &after[end + 2..];
                if self.permitted_secrets.iter().any(|n| n == name) {
                    if let Some(v) = self.secrets.get(name) {
                        out.push_str(&v);
                    } else {
                        // Permitted but unset — leave the literal token in.
                        out.push_str("{{secret:");
                        out.push_str(name);
                        out.push_str("}}");
                    }
                } else {
                    // Not permitted — leave literal in so behavior is
                    // observable; the plugin can't trick the host into
                    // attempting unscoped lookups.
                    out.push_str("{{secret:");
                    out.push_str(name);
                    out.push_str("}}");
                }
                rest = tail;
            } else {
                // No closing `}}` — bail; emit the rest verbatim.
                out.push_str(&rest[start..]);
                rest = "";
                break;
            }
        }
        out.push_str(rest);
        out
    }

    /// Scan `body` for any byte sequence matching a permitted secret's
    /// VALUE; redact matches. Defense-in-depth against echo-back leaks.
    fn scan_for_secret_leaks(&self, body: Vec<u8>) -> Vec<u8> {
        // Collect known-permitted secret values once per call.
        let values: Vec<Vec<u8>> = self
            .permitted_secrets
            .iter()
            .filter_map(|n| self.secrets.get(n))
            .filter(|v| !v.is_empty())
            .map(|v| v.into_bytes())
            .collect();
        if values.is_empty() {
            return body;
        }
        let redacted = b"<REDACTED>";
        let mut out: Vec<u8> = Vec::with_capacity(body.len());
        let mut i = 0usize;
        let mut hits = 0usize;
        while i < body.len() {
            let mut matched = None;
            for v in &values {
                if !v.is_empty()
                    && i + v.len() <= body.len()
                    && &body[i..i + v.len()] == v.as_slice()
                {
                    matched = Some(v.len());
                    break;
                }
            }
            if let Some(len) = matched {
                out.extend_from_slice(redacted);
                i += len;
                hits += 1;
            } else {
                out.push(body[i]);
                i += 1;
            }
        }
        if hits > 0 {
            tracing::warn!(plugin = %self.plugin, hits = hits,
                "gated http: secret value detected in response body, redacted");
        }
        out
    }
}

#[async_trait]
impl WaylandHostHttp for GatedHostHttp {
    async fn http_request(
        &self,
        url: String,
        method: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    ) -> Result<HttpResponse, HostHttpError> {
        let parsed = url::Url::parse(&url).map_err(|_| HostHttpError::InvalidUrl)?;
        let host = parsed.host_str().ok_or(HostHttpError::InvalidUrl)?;
        if !self.host_allowed(host) {
            tracing::warn!(plugin = %self.plugin, host = %host,
                "gated http: host not in allowlist; denying");
            return Err(HostHttpError::PermissionDenied);
        }

        // Aud-13: validate + host-side secret-expand guest-supplied headers
        // BEFORE any DNS/SSRF network work, so a malformed header fails fast
        // with a clear error rather than being masked by a downstream network
        // result. Each value may carry `{{secret:NAME}}`; a name/value that
        // cannot form a valid HTTP header is a hard error (surfaced to the
        // guest) rather than a silently dropped header — dropping would
        // reproduce the phantom-affordance the guest expected the host to honor
        // (e.g. an Authorization the request needs).
        let mut header_map = reqwest::header::HeaderMap::with_capacity(headers.len());
        for (name, value) in headers {
            let expanded = self.expand_secrets(&value);
            let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
                .map_err(|e| HostHttpError::InvalidHeader(format!("name '{name}': {e}")))?;
            let header_value = reqwest::header::HeaderValue::from_str(&expanded)
                .map_err(|e| HostHttpError::InvalidHeader(format!("value for '{name}': {e}")))?;
            header_map.append(header_name, header_value);
        }

        // SSRF guard (H-1): the allowlist is a pure host-glob string match —
        // it does not stop a metadata-IP literal, an allowlisted `*`, or a
        // hostname that resolves into private space. Route the URL through the
        // same private/internal-address check the rest of the codebase uses,
        // as an ADDITIONAL filter on top of the allowlist. Redirects are NOT
        // auto-followed (see the client builder below) so this gate is the
        // only egress path — every hop is a fresh gated request.
        if !self.ssrf_allows(parsed.as_str()) {
            tracing::warn!(plugin = %self.plugin, host = %host,
                "gated http: url targets a private/internal address; denying");
            return Err(HostHttpError::BlockedUrl);
        }

        let method_parsed: reqwest::Method =
            method.parse().map_err(|_| HostHttpError::InvalidMethod)?;

        // Host-side secret expansion on the body (best-effort: only valid
        // UTF-8 bodies are expanded; binary bodies pass through).
        let body_for_send: Vec<u8> = match std::str::from_utf8(&body) {
            Ok(s) => self.expand_secrets(s).into_bytes(),
            Err(_) => body,
        };

        // Pin the validated connect-time addresses so reqwest does NOT perform
        // a second DNS resolution (H-1 DNS-rebinding TOCTOU). Resolve the host
        // once here, re-validate every IP, and feed them to `resolve_to_addrs`.
        let port = parsed.port_or_known_default().unwrap_or(0);
        let pinned = self.pinned_safe_addrs(host, port)?;

        // H-1 (DNS-rebinding via redirect): do NOT auto-follow redirects.
        // `resolve_to_addrs` pins ONLY the original `host`; reqwest re-resolves
        // any redirect-target host at connect time via the system resolver, so
        // a 302 to an attacker hostname could rebind to the metadata IP after a
        // benign check-time lookup. `Policy::none()` returns the 3xx to the
        // guest verbatim — if it wants to follow, it issues a fresh
        // `http_request` that re-runs `host_allowed` + `ssrf_allows` + the
        // resolve-once pin on the new URL. No unvalidated hop is ever dialed.
        let client = wcore_egress::EgressClient::builder()
            .timeout(self.timeout)
            .redirect(reqwest::redirect::Policy::none())
            .resolve_to_addrs(host, &pinned)
            .build()
            .map_err(|e| HostHttpError::ClientBuild(e.to_string()))?;

        let resp = client
            .request(method_parsed, parsed)
            .headers(header_map)
            .body(body_for_send)
            .send()
            .await
            .map_err(|e| HostHttpError::Network(e.to_string()))?;

        let status = resp.status().as_u16();
        let raw = resp
            .bytes()
            .await
            .map_err(|e| HostHttpError::Network(e.to_string()))?;
        if raw.len() > self.max_body {
            return Err(HostHttpError::BodyTooLarge);
        }
        let body_out = self.scan_for_secret_leaks(raw.to_vec());

        Ok(HttpResponse {
            status,
            body: body_out,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// In-memory SecretSource for tests.
    #[derive(Default, Clone)]
    struct MapSecrets(HashMap<String, String>);
    impl MapSecrets {
        fn with(pairs: &[(&str, &str)]) -> Arc<dyn SecretSource> {
            let mut m = HashMap::new();
            for (k, v) in pairs {
                m.insert((*k).into(), (*v).into());
            }
            Arc::new(MapSecrets(m))
        }
    }
    impl SecretSource for MapSecrets {
        fn get(&self, name: &str) -> Option<String> {
            self.0.get(name).cloned()
        }
    }

    fn empty_secrets() -> Arc<dyn SecretSource> {
        Arc::new(EmptySecretSource)
    }

    #[tokio::test]
    async fn denied_http_returns_err() {
        let deny = DenyHostHttp;
        let res = deny
            .http_request("https://x".into(), "GET".into(), vec![], vec![])
            .await;
        assert!(matches!(res, Err(HostHttpError::PermissionDenied)));
    }

    #[tokio::test]
    async fn gated_http_invalid_url() {
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &["*".to_string()],
            vec![],
            empty_secrets(),
        );
        let res = h
            .http_request("not a url".into(), "GET".into(), vec![], vec![])
            .await;
        assert!(matches!(res, Err(HostHttpError::InvalidUrl)));
    }

    #[tokio::test]
    async fn gated_http_host_not_in_allowlist_denies() {
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &["api.example.com".to_string()],
            vec![],
            empty_secrets(),
        );
        let res = h
            .http_request(
                "https://other.example.com/".into(),
                "GET".into(),
                vec![],
                vec![],
            )
            .await;
        assert!(matches!(res, Err(HostHttpError::PermissionDenied)));
    }

    #[tokio::test]
    async fn gated_http_empty_allowlist_denies_all() {
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &[],
            vec![],
            empty_secrets(),
        );
        let res = h
            .http_request("https://anywhere/".into(), "GET".into(), vec![], vec![])
            .await;
        assert!(matches!(res, Err(HostHttpError::PermissionDenied)));
    }

    #[tokio::test]
    async fn allowed_host_succeeds_via_wiremock() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/hello"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let host = url::Url::parse(&server.uri())
            .unwrap()
            .host_str()
            .unwrap()
            .to_string();
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &[host],
            vec![],
            empty_secrets(),
        )
        .allowing_loopback_for_test();
        let res = h
            .http_request(
                format!("{}/hello", server.uri()),
                "GET".into(),
                vec![],
                vec![],
            )
            .await
            .expect("request");
        assert_eq!(res.status, 200);
        assert_eq!(res.body, b"ok");
    }

    /// Aud-13: guest-supplied request headers are applied to the outbound
    /// request, and a `{{secret:NAME}}` token in a header value is expanded
    /// host-side (the secret VALUE never originates from the guest). Proven by
    /// having wiremock require the exact expanded `Authorization` header.
    #[tokio::test]
    async fn request_headers_are_applied_and_secrets_expanded() {
        use wiremock::matchers::{header, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/secure"))
            .and(header("authorization", "Bearer s3cr3t-value"))
            .and(header("x-custom", "plain"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let host = url::Url::parse(&server.uri())
            .unwrap()
            .host_str()
            .unwrap()
            .to_string();
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &[host],
            vec!["API_KEY".to_string()],
            MapSecrets::with(&[("API_KEY", "s3cr3t-value")]),
        )
        .allowing_loopback_for_test();

        let headers = vec![
            (
                "Authorization".to_string(),
                "Bearer {{secret:API_KEY}}".to_string(),
            ),
            ("X-Custom".to_string(), "plain".to_string()),
        ];
        let res = h
            .http_request(
                format!("{}/secure", server.uri()),
                "GET".into(),
                headers,
                vec![],
            )
            .await
            .expect("request");
        // wiremock only returns 200 if BOTH headers matched, proving the
        // header map (and the host-side secret expansion) reached the wire.
        assert_eq!(res.status, 200);
    }

    /// Aud-13: a header name that cannot form a valid HTTP header is a hard
    /// error, not a silent drop.
    #[tokio::test]
    async fn invalid_header_name_is_rejected() {
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &["api.example.com".to_string()],
            vec![],
            empty_secrets(),
        );
        let res = h
            .http_request(
                "https://api.example.com/".into(),
                "GET".into(),
                vec![("Bad Header Name".to_string(), "v".to_string())],
                vec![],
            )
            .await;
        assert!(
            matches!(res, Err(HostHttpError::InvalidHeader(_))),
            "got {res:?}"
        );
    }

    /// H-1 (residual bypass) — redirects must NOT be auto-followed. The old
    /// `ssrf_safe_redirect_policy` followed a 302 after a rebindable check-time
    /// `is_safe_url`, so a redirect to an attacker hostname could DNS-rebind to
    /// the metadata IP at connect time (the redirect host was never pinned via
    /// `resolve_to_addrs`). With `Policy::none()` the 3xx is handed back to the
    /// guest verbatim and the redirect target is never dialed by this client.
    #[tokio::test]
    async fn redirect_is_not_followed_returns_3xx_to_guest() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // /redirect → 302 pointing at /landing.
        Mock::given(method("GET"))
            .and(path("/redirect"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("location", format!("{}/landing", server.uri())),
            )
            .mount(&server)
            .await;
        // /landing must NEVER be hit — expect(0) makes wiremock fail on drop
        // if the redirect was followed.
        Mock::given(method("GET"))
            .and(path("/landing"))
            .respond_with(ResponseTemplate::new(200).set_body_string("FOLLOWED"))
            .expect(0)
            .mount(&server)
            .await;

        let host = url::Url::parse(&server.uri())
            .unwrap()
            .host_str()
            .unwrap()
            .to_string();
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &[host],
            vec![],
            empty_secrets(),
        )
        .allowing_loopback_for_test();
        let res = h
            .http_request(
                format!("{}/redirect", server.uri()),
                "GET".into(),
                vec![],
                vec![],
            )
            .await
            .expect("request");
        // The 302 is surfaced to the guest; the landing page is not reached.
        assert_eq!(res.status, 302, "redirect must be returned, not followed");
        assert_ne!(res.body, b"FOLLOWED", "redirect target must not be dialed");
    }

    #[tokio::test]
    async fn leak_scan_redacts_secret_value_in_response() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Server echoes a body that happens to contain a known secret.
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_body_string("token=sk-super-secret-xyz here"),
            )
            .mount(&server)
            .await;

        let host = url::Url::parse(&server.uri())
            .unwrap()
            .host_str()
            .unwrap()
            .to_string();
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &[host],
            vec!["MY_KEY".into()],
            MapSecrets::with(&[("MY_KEY", "sk-super-secret-xyz")]),
        )
        .allowing_loopback_for_test();
        let res = h
            .http_request(server.uri(), "GET".into(), vec![], vec![])
            .await
            .expect("request");
        let body = String::from_utf8(res.body).unwrap();
        assert!(
            !body.contains("sk-super-secret-xyz"),
            "secret not redacted: {body}"
        );
        assert!(body.contains("<REDACTED>"), "no redaction marker: {body}");
    }

    #[tokio::test]
    async fn body_too_large_returns_err() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("xxxxxxxxxx"))
            .mount(&server)
            .await;

        let host = url::Url::parse(&server.uri())
            .unwrap()
            .host_str()
            .unwrap()
            .to_string();
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &[host],
            vec![],
            empty_secrets(),
        )
        .with_max_body(4)
        .allowing_loopback_for_test();
        let res = h
            .http_request(server.uri(), "GET".into(), vec![], vec![])
            .await;
        assert!(matches!(res, Err(HostHttpError::BodyTooLarge)));
    }

    #[tokio::test]
    async fn secret_injection_expands_only_permitted_names() {
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &["x".into()],
            vec!["ALLOWED".into()],
            MapSecrets::with(&[("ALLOWED", "VAL_OK"), ("OTHER", "VAL_NO")]),
        );
        let out = h.expand_secrets("a={{secret:ALLOWED}};b={{secret:OTHER}};c=plain");
        assert_eq!(out, "a=VAL_OK;b={{secret:OTHER}};c=plain");
    }

    // --- SSRF guard (H-1/plugins-6) -------------------------------------

    #[test]
    fn ssrf_guard_blocks_metadata_and_private_literals() {
        // Literal IPs skip DNS, so these assertions are network-free.
        assert!(!is_safe_url("http://169.254.169.254/latest/meta-data/"));
        assert!(!is_safe_url("http://169.254.170.2/"));
        assert!(!is_safe_url("http://100.100.100.200/"));
        assert!(!is_safe_url("http://10.0.0.1/"));
        assert!(!is_safe_url("http://127.0.0.1/"));
        assert!(!is_safe_url("http://192.168.1.1/"));
        assert!(!is_safe_url("http://[::1]/"));
        assert!(!is_safe_url("http://[fd00::1]/"));
        // Whole link-local /16, not just the well-known IP.
        assert!(!is_safe_url("http://169.254.10.10/"));
        // Cloud-metadata hostnames.
        assert!(!is_safe_url("http://metadata.google.internal/"));
        assert!(!is_safe_url("http://kubernetes.default.svc/"));
        // Non-http(s) schemes rejected.
        assert!(!is_safe_url("file:///etc/passwd"));
        assert!(!is_safe_url("ftp://169.254.169.254/"));
        // A public literal IP passes (no DNS needed).
        assert!(is_safe_url("https://1.1.1.1/"));
    }

    #[tokio::test]
    async fn http_request_to_metadata_ip_is_blocked_even_if_allowlisted() {
        // Allowlist literally permits the metadata host; the SSRF guard must
        // still reject it. No network is touched — the guard fires first.
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &["169.254.169.254".to_string()],
            vec![],
            empty_secrets(),
        );
        let res = h
            .http_request(
                "http://169.254.169.254/latest/meta-data/iam/security-credentials/".into(),
                "GET".into(),
                vec![],
                vec![],
            )
            .await;
        assert!(matches!(res, Err(HostHttpError::BlockedUrl)), "got {res:?}");
    }

    #[tokio::test]
    async fn http_request_to_wildcard_allowlist_metadata_blocked() {
        // The dangerous `["*"]` allowlist still cannot reach the metadata IP.
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &["*".to_string()],
            vec![],
            empty_secrets(),
        );
        let res = h
            .http_request(
                "http://169.254.169.254/".into(),
                "GET".into(),
                vec![],
                vec![],
            )
            .await;
        assert!(matches!(res, Err(HostHttpError::BlockedUrl)), "got {res:?}");
    }

    // --- DNS-rebinding TOCTOU (H-1) -------------------------------------

    /// Resolver that returns a PUBLIC IP on the first call (passes the
    /// pre-flight `is_safe_url` check) and the AWS metadata IP on the second
    /// (the connect-time resolution reqwest would otherwise perform). Mirrors
    /// an attacker-controlled TTL=0 resolver.
    fn rebinding_resolver(_host: &str) -> Vec<IpAddr> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        if CALLS.fetch_add(1, Ordering::SeqCst) == 0 {
            vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))] // public
        } else {
            vec![IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))] // metadata
        }
    }

    #[test]
    fn pinned_addrs_use_validated_resolution_not_a_second_lookup() {
        // The pre-flight `is_safe_url`-style check would have used the FIRST
        // (public) resolution. `pinned_safe_addrs` performs the authoritative
        // resolution and pins it; reqwest never re-resolves. Assert the pinned
        // set is exactly the validated public IP — never the metadata IP.
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &["evil.example.com".to_string()],
            vec![],
            empty_secrets(),
        )
        .with_resolver(rebinding_resolver);

        let pinned = h
            .pinned_safe_addrs("evil.example.com", 80)
            .expect("first resolution is public and must be pinned");
        assert_eq!(
            pinned,
            vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
                80
            )],
            "pinned addrs must be the validated public IP, not a re-resolved metadata IP"
        );
        assert!(
            !pinned
                .iter()
                .any(|sa| sa.ip() == IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))),
            "metadata IP must never be pinned"
        );
    }

    #[test]
    fn pinned_addrs_fail_closed_when_resolution_is_blocked() {
        // A resolver that returns ONLY a metadata IP must yield BlockedUrl —
        // resolution happens once, here, and every IP is validated.
        fn metadata_resolver(_host: &str) -> Vec<IpAddr> {
            vec![IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))]
        }
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &["evil.example.com".to_string()],
            vec![],
            empty_secrets(),
        )
        .with_resolver(metadata_resolver);
        assert!(matches!(
            h.pinned_safe_addrs("evil.example.com", 80),
            Err(HostHttpError::BlockedUrl)
        ));
    }

    #[test]
    fn pinned_addrs_fail_closed_on_empty_resolution() {
        fn empty_resolver(_host: &str) -> Vec<IpAddr> {
            Vec::new()
        }
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &["evil.example.com".to_string()],
            vec![],
            empty_secrets(),
        )
        .with_resolver(empty_resolver);
        assert!(matches!(
            h.pinned_safe_addrs("evil.example.com", 80),
            Err(HostHttpError::BlockedUrl)
        ));
    }

    #[test]
    fn pinned_addrs_literal_metadata_ip_is_blocked() {
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &["*".to_string()],
            vec![],
            empty_secrets(),
        );
        assert!(matches!(
            h.pinned_safe_addrs("169.254.169.254", 80),
            Err(HostHttpError::BlockedUrl)
        ));
    }

    #[tokio::test]
    async fn glob_pattern_matches_subdomain() {
        let h = GatedHostHttp::new(
            Arc::new(PluginAccessGate),
            "p".into(),
            &["*.example.com".into()],
            vec![],
            empty_secrets(),
        );
        assert!(h.host_allowed("api.example.com"));
        assert!(h.host_allowed("v2.example.com"));
        assert!(!h.host_allowed("example.com"));
        assert!(!h.host_allowed("evil.com"));
    }
}
