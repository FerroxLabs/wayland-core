//! URL safety checks — block requests to private/internal network addresses.
//!
//! Ported from the prior Wayland Python engine (T3-3.3 sub-wave 3).
//!
//! Prevents SSRF (Server-Side Request Forgery) where a malicious prompt or
//! skill could trick the agent into fetching internal resources like cloud
//! metadata endpoints (169.254.169.254), localhost services, or private
//! network hosts.
//!
//! Limitations (documented, not fixable at pre-flight level):
//!   - DNS rebinding (TOCTOU): an attacker-controlled DNS server with TTL=0
//!     can return a public IP for the check, then a private IP for the actual
//!     connection. Fixing this requires connection-level validation (egress
//!     proxy or HTTP-client redirect/connect hook re-validation).
//!
//! Redirect-based bypass is mitigated at the HTTP client layer by routing
//! every tool-side `reqwest::Client` through [`ssrf_safe_redirect_policy`],
//! which re-validates each redirect target via [`is_safe_url`] before
//! following. F-019 (WebFetch) and #279 (github_api / linear / notion /
//! gitlab) both consume this single source of truth.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::Policy;
use url::Url;

/// Hostnames that should always be blocked regardless of IP resolution.
///
/// Cloud metadata services live here by design — there is no legitimate
/// agent reason to fetch them, and they leak IAM credentials, instance
/// identity tokens, and SSH keys to any caller that can issue an HTTP
/// request from the host.
const CLOUD_METADATA_HOSTNAMES: &[&str] = &[
    // GCP
    "metadata.google.internal",
    "metadata.goog",
    "metadata", // bare hostname inside GCP VPCs
    // Kubernetes
    "kubernetes.default",
    "kubernetes.default.svc",
    "kubernetes.default.svc.cluster.local",
];

/// Specific cloud-metadata IPv4 addresses that must always be blocked.
const CLOUD_METADATA_IPV4: &[Ipv4Addr] = &[
    Ipv4Addr::new(169, 254, 169, 254), // AWS / GCP / Azure / DO / Oracle
    Ipv4Addr::new(169, 254, 170, 2),   // AWS ECS task metadata (task IAM)
    Ipv4Addr::new(169, 254, 169, 253), // Azure IMDS wire server
    Ipv4Addr::new(100, 100, 100, 200), // Alibaba Cloud metadata
    Ipv4Addr::new(192, 0, 0, 192),     // Oracle Cloud metadata (legacy)
    Ipv4Addr::new(10, 96, 0, 1),       // Kubernetes default API server ClusterIP
];

/// Specific cloud-metadata IPv6 addresses that must always be blocked.
const CLOUD_METADATA_IPV6: &[Ipv6Addr] = &[
    // AWS metadata (IPv6): fd00:ec2::254
    Ipv6Addr::new(0xfd00, 0x0ec2, 0, 0, 0, 0, 0, 0x0254),
];

/// Test whether `ip` falls within the IPv4 link-local range 169.254.0.0/16,
/// which covers every major cloud vendor's metadata endpoint.
fn is_ipv4_link_local(ip: Ipv4Addr) -> bool {
    ip.octets()[0] == 169 && ip.octets()[1] == 254
}

/// Test whether `ip` falls within the IPv4 CGNAT / Shared Address Space
/// range 100.64.0.0/10 (RFC 6598). Used by carrier-grade NAT, Tailscale /
/// WireGuard VPNs, and some cloud internal networks.
///
/// Not classified as "private" by `Ipv4Addr::is_private`, so must be
/// blocked explicitly.
fn is_ipv4_cgnat(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (o[1] & 0b1100_0000) == 0b0100_0000 // 100.64.0.0/10
}

/// Test whether `ip` falls in IPv4 ranges reserved by IANA that should be
/// blocked but aren't covered by stable std flags (e.g. 0.0.0.0/8, 192.0.0.0/24,
/// 198.18.0.0/15 benchmarking, class E).
fn is_ipv4_reserved_extra(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    // 0.0.0.0/8 "this network"
    if o[0] == 0 {
        return true;
    }
    // 192.0.0.0/24 IETF protocol assignments
    if o[0] == 192 && o[1] == 0 && o[2] == 0 {
        return true;
    }
    // 198.18.0.0/15 benchmarking
    if o[0] == 198 && (o[1] == 18 || o[1] == 19) {
        return true;
    }
    // 240.0.0.0/4 reserved / class E (covers broadcast 255.255.255.255 too)
    if o[0] >= 240 {
        return true;
    }
    false
}

/// Return true when the IP is a known cloud metadata endpoint.
fn ip_is_cloud_metadata(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if CLOUD_METADATA_IPV4.contains(&v4) {
                return true;
            }
            if is_ipv4_link_local(v4) {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            if CLOUD_METADATA_IPV6.contains(&v6) {
                return true;
            }
            // IPv4-mapped IPv6 form (::ffff:169.254.169.254) — unwrap and re-check.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return ip_is_cloud_metadata(IpAddr::V4(mapped));
            }
            false
        }
    }
}

/// Return true if the IP should be blocked for SSRF protection.
fn is_blocked_ip(ip: IpAddr) -> bool {
    is_blocked_ip_with(ip, false)
}

/// Return true if the IP should be blocked for SSRF protection.
///
/// `allow_loopback` relaxes the loopback block ONLY (IPv4 127.0.0.0/8, IPv6
/// `::1`, and IPv4-mapped/compatible loopback). It is used for trusted,
/// user-configured *local* targets (e.g. a local MCP server the user added by
/// hand) where a connection to the user's own machine is not the SSRF threat
/// the guard exists to stop. Every other range — private LAN, link-local,
/// CGNAT, multicast, unspecified, documentation, IPv6 ULA, cloud-metadata —
/// stays blocked regardless.
fn is_blocked_ip_with(ip: IpAddr, allow_loopback: bool) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            if (!allow_loopback && v4.is_loopback())
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_documentation()
            {
                return true;
            }
            if is_ipv4_cgnat(v4) || is_ipv4_reserved_extra(v4) {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            if (!allow_loopback && v6.is_loopback()) || v6.is_multicast() || v6.is_unspecified() {
                return true;
            }
            // Unique local fc00::/7
            let seg0 = v6.segments()[0];
            if (seg0 & 0xfe00) == 0xfc00 {
                return true;
            }
            // Link-local fe80::/10
            if (seg0 & 0xffc0) == 0xfe80 {
                return true;
            }
            // IPv4-mapped IPv6 — unwrap and re-check using IPv4 rules.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_ip_with(IpAddr::V4(mapped), allow_loopback);
            }
            // IPv4-compatible (deprecated): ::a.b.c.d
            if let Some(compat) = v6.to_ipv4() {
                // Reject the all-zeros (and loopback aliases unless explicitly
                // allowed); the mapping path above handles the modern form.
                if compat.is_unspecified() || (!allow_loopback && compat.is_loopback()) {
                    return true;
                }
            }
            false
        }
    }
}

/// Parse `url` and extract the lowercased hostname (trailing dot stripped).
///
/// Returns `None` for parse failures or URLs without a host component.
fn extract_hostname(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    let host = parsed.host_str()?;
    let mut h = host.trim().to_ascii_lowercase();
    while h.ends_with('.') {
        h.pop();
    }
    if h.is_empty() { None } else { Some(h) }
}

/// Resolver injected for tests. Production code uses the system resolver.
type Resolver = fn(&str) -> Vec<IpAddr>;

fn system_resolver(host: &str) -> Vec<IpAddr> {
    // Use port 0 — we only care about the resolved IPs, not connectivity.
    match (host, 0u16).to_socket_addrs() {
        Ok(iter) => iter.map(|sa| sa.ip()).collect(),
        Err(_) => Vec::new(),
    }
}

/// Return `true` if the URL targets a cloud instance metadata endpoint.
///
/// Checks (in order):
///   1. Hostname matches a known metadata service hostname.
///   2. Hostname is a literal IP that matches the metadata deny set or
///      falls in the link-local range (169.254.0.0/16).
///   3. Hostname resolves (via the system resolver) to any IP in the deny set.
///
/// Returns `false` on parse errors or DNS failures — those are the caller's
/// problem; [`is_safe_url`] fails closed on its own. This helper is narrowly
/// scoped to "is this specifically a cloud metadata target", not "is this URL
/// safe".
pub fn is_cloud_metadata_url(url: &str) -> bool {
    is_cloud_metadata_url_with(url, system_resolver)
}

fn is_cloud_metadata_url_with(url: &str, resolve: Resolver) -> bool {
    let Some(hostname) = extract_hostname(url) else {
        return false;
    };

    if CLOUD_METADATA_HOSTNAMES.iter().any(|h| *h == hostname) {
        return true;
    }

    // Literal IP — check directly without DNS.
    if let Ok(ip) = hostname.parse::<IpAddr>() {
        return ip_is_cloud_metadata(ip);
    }
    // url::Url normalizes IPv6 literals into bracketed form which our
    // extractor strips; try the bracketed form too just in case.
    if let Some(stripped) = hostname.strip_prefix('[').and_then(|s| s.strip_suffix(']'))
        && let Ok(ip) = stripped.parse::<IpAddr>()
    {
        return ip_is_cloud_metadata(ip);
    }

    resolve(&hostname).into_iter().any(ip_is_cloud_metadata)
}

/// Return `true` if the URL target is not a private/internal address.
///
/// Resolves the hostname to an IP and checks against private ranges.
/// **Fails closed:** DNS errors and parse failures block the request.
///
/// Cloud instance metadata endpoints (169.254.169.254, the rest of
/// 169.254.0.0/16, metadata.google.internal, kubernetes.default, …) are
/// checked explicitly via [`is_cloud_metadata_url`] before the general
/// private-range check so the deny reason is clear in audit logs.
///
/// ## Residual DNS-rebinding TOCTOU (tools-io-22 — documented, not closed here)
///
/// This is a *pre-flight* check: it resolves the hostname, validates the
/// IP, then returns — but the subsequent HTTP connection performs its
/// **own** DNS resolution. An attacker-controlled resolver with TTL=0 can
/// return a public IP for this check and a private/metadata IP for the
/// connect (a classic time-of-check/time-of-use gap). [`is_safe_url`]
/// CANNOT close this alone because it never owns the socket.
///
/// To fully close it the HTTP-client layer must **pin** the validated IP
/// into the connection (resolve once, validate, then dial that exact
/// `SocketAddr` rather than re-resolving) — e.g. via
/// `reqwest::ClientBuilder::resolve` / a custom `dns::Resolve`, plus
/// [`ssrf_safe_redirect_policy`] to re-check every hop. That client lives
/// in the host (`wcore-agent::http_client`), outside this crate. Use
/// [`safe_url_pinned_ips`] from that layer to obtain the validated IPs to
/// pin. Until pinning is wired, this residual TOCTOU remains a documented
/// limitation; the redirect policy already re-validates redirect targets.
pub fn is_safe_url(url: &str) -> bool {
    is_safe_url_with(url, system_resolver)
}

/// Like [`is_safe_url`] but permits LOOPBACK targets (127.0.0.0/8, `::1`,
/// `localhost`). For trusted, user-configured local endpoints only — e.g. a
/// local MCP server the user added by hand. Every other private/internal/
/// metadata range stays blocked. An MCP server URL is trusted configuration,
/// not a model-driven fetch, so the SSRF guard's loopback block (meant for
/// untrusted URLs) should not apply to it.
pub fn is_safe_url_allow_loopback(url: &str) -> bool {
    is_safe_url_with_opts(url, system_resolver, true)
}

/// True only when `url`'s host is a LOOPBACK target (127.0.0.0/8, ::1, or a
/// name that resolves exclusively to loopback). Stricter than
/// [`is_safe_url_allow_loopback`], which also accepts public hosts: this is for
/// endpoints that MUST be local — e.g. the Forge MCP suite, whose granted
/// bearer token must never be sent off-box. Fails closed on an unparseable
/// host, empty resolution, or any non-loopback address in the resolved set.
pub fn is_loopback_url(url: &str) -> bool {
    is_loopback_url_with(url, system_resolver)
}

fn is_loopback_url_with(url: &str, resolve: Resolver) -> bool {
    let Some(hostname) = extract_hostname(url) else {
        return false;
    };
    // The `url` crate returns bracketed IPv6 hosts (e.g. "[::1]"); strip the
    // brackets so an IPv6 loopback literal parses instead of falling through
    // to DNS resolution (which would reject a legitimate `http://[::1]` URL).
    let literal = hostname.trim_start_matches('[').trim_end_matches(']');
    if let Ok(ip) = literal.parse::<IpAddr>() {
        return ip.is_loopback();
    }
    let addrs = resolve(&hostname);
    !addrs.is_empty() && addrs.iter().all(|ip| ip.is_loopback())
}

/// Resolve `url`'s host once and return the validated, SSRF-safe IPs so the
/// HTTP-client layer can **pin** them into the connection and close the
/// DNS-rebinding TOCTOU described on [`is_safe_url`].
///
/// Returns `None` when the URL is unsafe (any resolved IP is private /
/// internal / cloud-metadata), unparseable, or resolves to nothing — i.e.
/// `None` means "do not connect" (fails closed, same posture as
/// [`is_safe_url`]). `Some(ips)` is the exact resolved set that passed the
/// guard; the caller should dial only these addresses and must NOT
/// re-resolve the hostname.
///
/// A literal-IP URL returns the single parsed IP when safe. This is the
/// resolve-once primitive `reqwest::ClientBuilder::resolve` consumes.
pub fn safe_url_pinned_ips(url: &str) -> Option<Vec<IpAddr>> {
    safe_url_pinned_ips_with(url, system_resolver)
}

fn safe_url_pinned_ips_with(url: &str, resolve: Resolver) -> Option<Vec<IpAddr>> {
    let hostname = extract_hostname(url)?;

    // Internal-hostname and cloud-metadata gates first (mirror is_safe_url).
    if CLOUD_METADATA_HOSTNAMES.iter().any(|h| *h == hostname) {
        return None;
    }
    if is_cloud_metadata_url_with(url, resolve) {
        return None;
    }

    // Literal IP — no DNS, pin the single address if safe.
    if let Ok(ip) = hostname.parse::<IpAddr>() {
        return if is_blocked_ip(ip) {
            None
        } else {
            Some(vec![ip])
        };
    }

    let addrs = resolve(&hostname);
    if addrs.is_empty() || addrs.iter().copied().any(is_blocked_ip) {
        // Fail closed: empty resolution OR any blocked IP in the set.
        return None;
    }
    Some(addrs)
}

fn is_safe_url_with(url: &str, resolve: Resolver) -> bool {
    is_safe_url_with_opts(url, resolve, false)
}

fn is_safe_url_with_opts(url: &str, resolve: Resolver, allow_loopback: bool) -> bool {
    let Some(hostname) = extract_hostname(url) else {
        return false;
    };

    // Block known internal hostnames.
    if CLOUD_METADATA_HOSTNAMES.iter().any(|h| *h == hostname) {
        return false;
    }

    // Explicit cloud-metadata check — reject before falling through to the
    // general private-IP block so the log line names the threat. (Loopback is
    // never a metadata endpoint, so this gate is unaffected by allow_loopback.)
    if is_cloud_metadata_url_with(url, resolve) {
        return false;
    }

    // Literal IP — skip DNS.
    if let Ok(ip) = hostname.parse::<IpAddr>() {
        return !is_blocked_ip_with(ip, allow_loopback);
    }

    let addrs = resolve(&hostname);
    if addrs.is_empty() {
        // DNS resolution failed — fail closed. If DNS can't resolve it,
        // the HTTP client will also fail, so blocking loses nothing.
        return false;
    }

    !addrs
        .into_iter()
        .any(|ip| is_blocked_ip_with(ip, allow_loopback))
}

/// Build the SSRF-resistant `reqwest` redirect policy shared by every
/// tool-side HTTP client.
///
/// The default `reqwest` policy follows up to 10 hops without inspecting
/// any redirect target — an attacker-controlled server can return
/// `302 → http://169.254.169.254/latest/meta-data/iam/...` and reqwest
/// will follow it, handing back AWS IAM credentials to the model. The
/// same class of bypass applies to `http://10.x.x.x`, `http://127.0.0.1`,
/// `http://[::1]`, `http://[fd00::]`, etc.
///
/// This policy re-validates every redirect target via [`is_safe_url`]
/// (same logic as the pre-flight check) before following. A blocked
/// target aborts the chain immediately; `reqwest` surfaces the abort as
/// an `Error` whose `Display` contains the literal substring
/// `"redirect blocked"` so backends can recognise it without sniffing
/// internal error variants.
///
/// Legitimate redirects (http → https on the same host, CDN hops between
/// two public IPs, OAuth dances) pass through unchanged. The 10-hop
/// fallthrough cap is preserved.
///
/// F-019 (WebFetch) and #279 (`github_api`, `linear`, `notion`,
/// `gitlab`) both consume this single helper so the redirect policy is
/// one edit, not five.
pub fn ssrf_safe_redirect_policy() -> Policy {
    Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        let url = attempt.url().to_string();
        if is_safe_url(&url) {
            attempt.follow()
        } else {
            attempt.error(format!(
                "redirect blocked — target URL is a private or internal \
                 network address: {url}"
            ))
        }
    })
}

/// Loopback-permitting variant of [`ssrf_safe_redirect_policy`] for trusted
/// local endpoints. Re-validates every hop with [`is_safe_url_allow_loopback`],
/// so loopback hops are followed but all other private/internal/metadata
/// redirect targets are still blocked. Pair with [`LoopbackOkResolver`].
pub fn ssrf_safe_redirect_policy_allow_loopback() -> Policy {
    Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many redirects");
        }
        let url = attempt.url().to_string();
        if is_safe_url_allow_loopback(&url) {
            attempt.follow()
        } else {
            attempt.error(format!(
                "redirect blocked — target URL is a private or internal \
                 network address: {url}"
            ))
        }
    })
}

/// Resolve `host` and return the SSRF-safe socket addresses to dial (port `0`;
/// reqwest sets the real port per the URL/scheme). Returns empty — i.e. "do
/// not connect", failing closed — for a cloud-metadata hostname, a literal
/// blocked IP, a host that does not resolve, or a host where ANY resolved IP
/// is private/internal (no split-horizon). Mirrors [`safe_url_pinned_ips`] but
/// keyed on the bare hostname, which is all a [`Resolve`] impl receives.
fn ssrf_safe_socket_addrs_with(host: &str, resolve: Resolver) -> Vec<SocketAddr> {
    ssrf_safe_socket_addrs_with_opts(host, resolve, false)
}

fn ssrf_safe_socket_addrs_with_opts(
    host: &str,
    resolve: Resolver,
    allow_loopback: bool,
) -> Vec<SocketAddr> {
    if CLOUD_METADATA_HOSTNAMES.contains(&host) {
        return Vec::new();
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return if is_blocked_ip_with(ip, allow_loopback) {
            Vec::new()
        } else {
            vec![SocketAddr::new(ip, 0)]
        };
    }
    let addrs = resolve(host);
    if addrs.is_empty()
        || addrs
            .iter()
            .copied()
            .any(|ip| is_blocked_ip_with(ip, allow_loopback))
    {
        return Vec::new();
    }
    addrs.into_iter().map(|ip| SocketAddr::new(ip, 0)).collect()
}

/// A `reqwest` DNS resolver that returns ONLY SSRF-safe IPs for every
/// hostname, dropping private / internal / cloud-metadata addresses.
///
/// This is the **connect-time** half of the SSRF defense and closes the
/// DNS-rebinding TOCTOU that [`ssrf_safe_redirect_policy`] alone cannot:
/// reqwest dials exactly the addresses this resolver returns and performs no
/// resolution of its own, so the validation *is* the resolution — there is no
/// separate check→connect window for an attacker's TTL=0 resolver to rebind a
/// host onto `169.254.169.254`. It applies to the initial request AND every
/// redirect hop, which is why it fits the long-lived, multi-host tool clients
/// (WebFetch, the API backends, the MCP HTTP transports) that cannot pin a
/// single host up front via `ClientBuilder::resolve_to_addrs`.
///
/// Pair it with [`ssrf_safe_redirect_policy`]: the policy is the fast
/// string-level gate that also rejects literal private-IP redirect targets;
/// this resolver is the connect-time guarantee.
#[derive(Debug, Clone, Copy, Default)]
pub struct SsrfSafeResolver;

impl Resolve for SsrfSafeResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            // `system_resolver` is a blocking getaddrinfo; keep it off the
            // async reactor thread.
            let safe = tokio::task::spawn_blocking(move || {
                ssrf_safe_socket_addrs_with(&host, system_resolver)
            })
            .await
            .unwrap_or_default();
            if safe.is_empty() {
                return Err(Box::<dyn std::error::Error + Send + Sync>::from(
                    "SSRF guard: host has no safe (public) address to dial",
                ));
            }
            Ok(Box::new(safe.into_iter()) as Addrs)
        })
    }
}

/// Loopback-permitting counterpart to [`SsrfSafeResolver`]. Returns loopback
/// IPs (in addition to safe public IPs) so reqwest can dial a trusted local
/// endpoint, while still dropping every other private/internal/metadata
/// address. Use ONLY for user-configured local targets (e.g. local MCP
/// servers), paired with [`ssrf_safe_redirect_policy_allow_loopback`].
#[derive(Debug, Clone, Copy, Default)]
pub struct LoopbackOkResolver;

impl Resolve for LoopbackOkResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_string();
        Box::pin(async move {
            let safe = tokio::task::spawn_blocking(move || {
                ssrf_safe_socket_addrs_with_opts(&host, system_resolver, true)
            })
            .await
            .unwrap_or_default();
            if safe.is_empty() {
                return Err(Box::<dyn std::error::Error + Send + Sync>::from(
                    "SSRF guard: host has no safe address to dial (loopback allowed)",
                ));
            }
            Ok(Box::new(safe.into_iter()) as Addrs)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Test resolver: returns public IPs for "example.com", private for
    /// "intranet.local", localhost for "myhost.local"; everything else NXDOMAIN.
    fn fake_resolver(host: &str) -> Vec<IpAddr> {
        match host {
            "example.com" => vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
            "intranet.local" => vec![IpAddr::V4(Ipv4Addr::new(10, 0, 0, 5))],
            "myhost.local" => vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
            "metadata-shim.example" => vec![IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))],
            "ipv6-pub.example" => vec![IpAddr::V6("2606:4700::1111".parse().unwrap())],
            _ => Vec::new(),
        }
    }

    #[test]
    fn public_https_passes() {
        assert!(is_safe_url_with("https://example.com/path", fake_resolver));
        assert!(is_safe_url_with("http://example.com:8080/", fake_resolver));
        assert!(is_safe_url_with("https://ipv6-pub.example/", fake_resolver));
    }

    // allow_local / loopback opt-in: a trusted user-configured local MCP server
    // (e.g. Agent Vault at http://127.0.0.1:3456/mcp) must be reachable, while
    // EVERY other private/internal/metadata range stays blocked.
    #[test]
    fn allow_loopback_permits_loopback_but_still_blocks_others() {
        // Loopback now allowed (literal IPs skip the resolver).
        assert!(is_safe_url_with_opts(
            "http://127.0.0.1:3456/mcp",
            fake_resolver,
            true
        ));
        assert!(is_safe_url_with_opts(
            "http://127.0.0.1/",
            fake_resolver,
            true
        ));
        // A hostname that resolves to loopback is allowed too (covers localhost
        // and IPv6 `::1` via the resolver path). `myhost.local` -> 127.0.0.1.
        assert!(is_safe_url_with_opts(
            "http://myhost.local/",
            fake_resolver,
            true
        ));
        // Note: a *bracketed* IPv6 literal (`http://[::1]/`) is parsed with the
        // brackets retained and so routes through the resolver rather than the
        // literal-IP fast path — a pre-existing behavior of `extract_hostname`,
        // unchanged here. Use a hostname (e.g. `localhost`) for IPv6 loopback.

        // Non-loopback ranges remain blocked even with allow_loopback=true.
        assert!(!is_safe_url_with_opts(
            "http://169.254.169.254/",
            fake_resolver,
            true
        ));
        assert!(!is_safe_url_with_opts(
            "http://10.0.0.5/",
            fake_resolver,
            true
        ));
        assert!(!is_safe_url_with_opts(
            "http://192.168.1.1/",
            fake_resolver,
            true
        ));
        assert!(!is_safe_url_with_opts(
            "http://172.16.0.1/",
            fake_resolver,
            true
        ));
        assert!(!is_safe_url_with_opts(
            "http://100.64.0.1/",
            fake_resolver,
            true
        ));
        // intranet.local resolves to 10.0.0.5 — still blocked.
        assert!(!is_safe_url_with_opts(
            "http://intranet.local/",
            fake_resolver,
            true
        ));
        // Public still fine.
        assert!(is_safe_url_with_opts(
            "https://example.com/",
            fake_resolver,
            true
        ));

        // The DEFAULT path (allow_loopback=false) still blocks loopback.
        assert!(!is_safe_url_with("http://127.0.0.1/", fake_resolver));
        assert!(is_safe_url_allow_loopback("http://127.0.0.1:3456/mcp"));
    }

    // H-1-broad — the SSRF-safe DNS resolver is the connect-time half that
    // closes the rebinding TOCTOU. Validation happens AT resolution, so a host
    // that resolves to a blocked IP yields NO dialable address.
    #[test]
    fn ssrf_safe_socket_addrs_filters_blocked() {
        // Public host → its public IP, port 0 (reqwest fills the real port).
        let pub_addrs = ssrf_safe_socket_addrs_with("example.com", fake_resolver);
        assert_eq!(
            pub_addrs,
            vec![SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
                0
            )]
        );
        // Hostname resolving into private space → empty (do not connect). This
        // is the rebind target: even if an attacker's resolver returns this at
        // connect time, the resolver drops it.
        assert!(ssrf_safe_socket_addrs_with("intranet.local", fake_resolver).is_empty());
        // Cloud-metadata IP (link-local) → empty.
        assert!(ssrf_safe_socket_addrs_with("metadata-shim.example", fake_resolver).is_empty());
        // Literal metadata IP → empty; literal public IP → dialable.
        assert!(ssrf_safe_socket_addrs_with("169.254.169.254", fake_resolver).is_empty());
        assert_eq!(
            ssrf_safe_socket_addrs_with("8.8.8.8", fake_resolver),
            vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)), 0)]
        );
        // Unresolvable → empty (fail closed).
        assert!(ssrf_safe_socket_addrs_with("nope.invalid", fake_resolver).is_empty());
    }

    #[tokio::test]
    async fn ssrf_resolver_errors_on_metadata_literal_allows_public() {
        use std::str::FromStr;
        let r = SsrfSafeResolver;
        // Literal IPs need no DNS, so this is deterministic.
        assert!(
            r.resolve(Name::from_str("169.254.169.254").unwrap())
                .await
                .is_err(),
            "metadata IP must yield no dialable address"
        );
        let ok = r
            .resolve(Name::from_str("8.8.8.8").unwrap())
            .await
            .expect("public literal IP must resolve");
        assert!(ok.count() >= 1, "public IP must produce a dialable address");
    }

    #[test]
    fn localhost_literal_blocked() {
        assert!(!is_safe_url_with("http://127.0.0.1/", fake_resolver));
        assert!(!is_safe_url_with("http://localhost/", fake_resolver)); // resolves to nothing → fail closed
        assert!(!is_safe_url_with("http://[::1]/", fake_resolver));
    }

    #[test]
    fn private_ip_blocked() {
        // RFC1918
        assert!(!is_safe_url_with("http://10.0.0.5/", fake_resolver));
        assert!(!is_safe_url_with("http://172.16.0.1/", fake_resolver));
        assert!(!is_safe_url_with("http://172.31.255.254/", fake_resolver));
        assert!(!is_safe_url_with("http://192.168.1.1/", fake_resolver));
        // Hostname resolving to a private IP must also be blocked.
        assert!(!is_safe_url_with("http://intranet.local/", fake_resolver));
    }

    #[test]
    fn cgnat_blocked() {
        // 100.64.0.0/10 is not classified by std as private — must still block.
        assert!(!is_safe_url_with("http://100.64.0.1/", fake_resolver));
        assert!(!is_safe_url_with("http://100.127.255.254/", fake_resolver));
        // Just outside the range stays unblocked (public-ish IP, fake resolver
        // returns no record so it fails closed — verify with literal that *is*
        // resolvable in the public space).
        // 100.63.x is class A private-adjacent but actually public; literal
        // path should return Ok if not blocked.
        assert!(is_safe_url_with("http://100.63.0.1/", fake_resolver));
        assert!(is_safe_url_with("http://100.128.0.1/", fake_resolver));
    }

    #[test]
    fn cloud_metadata_blocked() {
        // Literal IPs across all major vendors.
        assert!(!is_safe_url_with(
            "http://169.254.169.254/latest/meta-data/",
            fake_resolver
        ));
        assert!(!is_safe_url_with("http://169.254.170.2/", fake_resolver));
        assert!(!is_safe_url_with("http://100.100.100.200/", fake_resolver));
        assert!(!is_safe_url_with("http://192.0.0.192/", fake_resolver));
        // Whole link-local /16 covered, not just the well-known IP.
        assert!(!is_safe_url_with("http://169.254.10.10/", fake_resolver));
        // Hostnames.
        assert!(!is_safe_url_with(
            "http://metadata.google.internal/",
            fake_resolver
        ));
        assert!(!is_safe_url_with(
            "http://kubernetes.default.svc/",
            fake_resolver
        ));
        // Hostname that resolves to metadata IP via DNS.
        assert!(!is_safe_url_with(
            "http://metadata-shim.example/",
            fake_resolver
        ));
        // Direct cloud-metadata-only helper.
        assert!(is_cloud_metadata_url_with(
            "http://169.254.169.254/",
            fake_resolver
        ));
        assert!(is_cloud_metadata_url_with(
            "http://metadata.google.internal/",
            fake_resolver
        ));
        assert!(!is_cloud_metadata_url_with(
            "https://example.com/",
            fake_resolver
        ));
    }

    #[test]
    fn ipv6_link_local_and_ula_blocked() {
        // fe80::/10 link-local
        assert!(!is_safe_url_with("http://[fe80::1]/", fake_resolver));
        // fc00::/7 ULA
        assert!(!is_safe_url_with("http://[fc00::1]/", fake_resolver));
        assert!(!is_safe_url_with("http://[fd00::1]/", fake_resolver));
        // IPv4-mapped IPv6 pointing at private IP must also be blocked.
        assert!(!is_safe_url_with(
            "http://[::ffff:10.0.0.1]/",
            fake_resolver
        ));
        // IPv4-mapped IPv6 to metadata endpoint.
        assert!(!is_safe_url_with(
            "http://[::ffff:169.254.169.254]/",
            fake_resolver
        ));
    }

    #[test]
    fn malformed_url_blocked() {
        assert!(!is_safe_url_with("", fake_resolver));
        assert!(!is_safe_url_with("not a url", fake_resolver));
        assert!(!is_safe_url_with("http:///no-host", fake_resolver));
        // file:// has no host → blocked (no network safety guarantee).
        assert!(!is_safe_url_with("file:///etc/passwd", fake_resolver));
    }

    #[test]
    fn dns_failure_fails_closed() {
        // Hostname not in fake_resolver → empty result → block.
        assert!(!is_safe_url_with("http://nxdomain.invalid/", fake_resolver));
    }

    #[test]
    fn ssrf_safe_redirect_policy_constructs_without_panic() {
        // The policy is consumed by `reqwest::Client::builder().redirect(...)`;
        // construction must succeed without panicking so callers can wire it
        // unconditionally at startup.
        let _policy = ssrf_safe_redirect_policy();
    }

    // tools-io-22: the resolve-once pinning primitive must return the exact
    // validated IP set for safe URLs and `None` (fail closed) for unsafe /
    // metadata / unresolvable URLs, so the HTTP layer can dial only the
    // pinned addresses and never re-resolve.
    #[test]
    fn pinned_ips_returns_validated_set_for_safe_url() {
        let ips = safe_url_pinned_ips_with("https://example.com/x", fake_resolver).unwrap();
        assert_eq!(ips, vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]);
        // Literal public IP pins to itself.
        let ips = safe_url_pinned_ips_with("https://93.184.216.34/", fake_resolver).unwrap();
        assert_eq!(ips, vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))]);
    }

    #[test]
    fn pinned_ips_fails_closed_on_unsafe_url() {
        // Private resolution.
        assert!(safe_url_pinned_ips_with("http://intranet.local/", fake_resolver).is_none());
        // Loopback literal.
        assert!(safe_url_pinned_ips_with("http://127.0.0.1/", fake_resolver).is_none());
        // Cloud-metadata literal.
        assert!(safe_url_pinned_ips_with("http://169.254.169.254/", fake_resolver).is_none());
        // Hostname resolving to metadata IP.
        assert!(safe_url_pinned_ips_with("http://metadata-shim.example/", fake_resolver).is_none());
        // NXDOMAIN — fail closed.
        assert!(safe_url_pinned_ips_with("http://nxdomain.invalid/", fake_resolver).is_none());
        // Unparseable.
        assert!(safe_url_pinned_ips_with("not a url", fake_resolver).is_none());
    }

    // F12: the loopback-ONLY predicate gates the Forge granted-token egress.
    // It must accept loopback literals and loopback-only resolutions, and
    // reject public, private, mixed, and unparseable hosts (fails closed).
    #[test]
    fn is_loopback_url_accepts_only_loopback() {
        // Loopback literals (IPv4 + bracketed IPv6).
        assert!(is_loopback_url_with("http://127.0.0.1:3456", fake_resolver));
        assert!(is_loopback_url_with("http://[::1]:80", fake_resolver));
        // Private (not loopback) → false.
        assert!(!is_loopback_url_with("http://10.0.0.5", fake_resolver));
        // Public host (resolver returns a public IP) → false.
        let public = |host: &str| match host {
            "evil.example.com" => vec![IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34))],
            _ => Vec::new(),
        };
        assert!(!is_loopback_url_with("https://evil.example.com", public));
        // Host resolving to a MIX of loopback + public → false (all must be loopback).
        let mixed = |host: &str| match host {
            "mixed.local" => vec![
                IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
                IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
            ],
            _ => Vec::new(),
        };
        assert!(!is_loopback_url_with("http://mixed.local", mixed));
        // Unparseable → false.
        assert!(!is_loopback_url_with("not a url", fake_resolver));
        // Empty resolution → false (fail closed).
        assert!(!is_loopback_url_with(
            "http://nxdomain.invalid",
            fake_resolver
        ));
    }

    #[test]
    fn reserved_ipv4_blocked() {
        // 0.0.0.0/8
        assert!(!is_safe_url_with("http://0.0.0.0/", fake_resolver));
        // 240.0.0.0/4
        assert!(!is_safe_url_with("http://240.0.0.1/", fake_resolver));
        // Broadcast
        assert!(!is_safe_url_with("http://255.255.255.255/", fake_resolver));
        // 198.18.0.0/15 benchmarking
        assert!(!is_safe_url_with("http://198.18.0.1/", fake_resolver));
        // 192.0.0.0/24 IETF
        assert!(!is_safe_url_with("http://192.0.0.5/", fake_resolver));
    }
}
