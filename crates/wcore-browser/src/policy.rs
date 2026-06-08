//! `BrowserPolicy` — URL-level enforcement gate.
//!
//! ## Hard-coded blocks (always-on, regardless of allow / deny lists)
//!
//!   * RFC 1918 private ranges (10/8, 172.16/12, 192.168/16).
//!   * Loopback (127/8, `localhost`, `*.localhost`, `::1`).
//!   * Cloud metadata endpoint (169.254.169.254 — AWS / GCP / Azure /
//!     OpenStack share this address).
//!   * Link-local IPv4 (169.254/16) and IPv6 (`fe80::/10`).
//!   * IPv6 unique-local (`fc00::/7`).
//!   * IPv4-mapped IPv6 literals (`::ffff:a.b.c.d`) where the embedded v4
//!     hits any of the above categories.
//!   * Legacy IPv4 encodings — octal (`0177.0.0.1`), hex (`0x7f.0.0.1`),
//!     and decimal-overflow forms (`2130706433`) — are normalized before
//!     the loopback / private / metadata / link-local check.
//!
//! ## Scheme allowlist (always-on)
//!
//! Only `http` and `https` are accepted. Everything else
//! (`javascript:`, `data:`, `blob:`, `file:`, `ftp:`, `gopher:`,
//! `view-source:`, ...) is refused at the gate.
//!
//! ## Origin lists (operator-configured)
//!
//!   * `denied_origins` — suffix glob (`*.evil.example`). Always wins.
//!   * `allowed_origins` — suffix glob. When non-empty, only explicit
//!     matches pass; everything else falls through to `default_action`.
//!
//! ## `default_action`
//!
//!   * `Deny` (default since v0.2.1) — fail-closed. Unknown origins blocked
//!     unless explicitly allow-listed.
//!   * `Allow` — explicit-block list still applies; everything else passes.
//!   * `Ask` — unknown origins route to `Suspend` so the orchestration
//!     layer can request HITL approval (S4 suspend pattern).
//!
//! ## DNS rebinding (TOFU cache)
//!
//! Hostnames resolved once via [`check_resolved_host`] are pinned per
//! policy instance. Subsequent resolutions returning a different IP are
//! refused — defense against DNS rebinding attacks that swap a benign
//! initial resolve for a private / metadata target.
//!
//! ## Redirect re-check
//!
//! [`BrowserPolicy::reqwest_redirect_policy`] returns a
//! [`reqwest::redirect::Policy`] that re-evaluates this policy on every
//! redirect hop. Backends that follow redirects via reqwest MUST install
//! it on their client builder.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::str::FromStr;
use std::sync::Arc;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("URL parse error: {0}")]
    UrlParse(String),
    #[error("policy violation: {reason}")]
    Violation { reason: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PolicyAction {
    Allow,
    Deny,
    Ask,
}

impl Default for PolicyAction {
    /// Default policy action is **Deny** (fail-closed) as of v0.2.1.
    /// Earlier versions defaulted to `Allow` which permitted arbitrary
    /// origins by default — see `STABILITY-v0.2.0.md` MAJOR #6.
    fn default() -> Self {
        PolicyAction::Deny
    }
}

/// Outcome of a `check_url`. `Ok(())` means allowed; structured outcome
/// surfaces the suspend/deny path so the tool layer can map it to a
/// protocol event.
#[derive(Debug, Clone)]
pub enum PolicyOutcome {
    Allow,
    Deny { reason: String },
    Suspend { url: String },
}

/// Schemes that pass the scheme allow-list. Any other scheme is denied
/// at the gate. The list is intentionally minimal: HTTP + HTTPS only.
const ALLOWED_SCHEMES: &[&str] = &["http", "https"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserPolicy {
    /// What to do when no rule matches. Default `Deny` (fail-closed) as
    /// of v0.2.1 — explicit allow-list required to do anything. Pre-v0.2.1
    /// this defaulted to `Allow` which was a fail-open SSRF risk.
    #[serde(default)]
    pub default_action: PolicyAction,
    /// Origin allow list (suffix glob, e.g. `*.example.com`). When non-empty,
    /// origins not on the list fall through to `default_action`.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// Origin deny list (suffix glob). Takes precedence over allow.
    #[serde(default)]
    pub denied_origins: Vec<String>,
    /// DNS-rebinding TOFU cache. Pinned hostname → first-seen IP. On
    /// subsequent resolution of the same hostname, if the IP differs the
    /// request is refused. Cleared when the policy is dropped.
    #[serde(skip)]
    dns_cache: Arc<Mutex<HashMap<String, IpAddr>>>,
}

impl Default for BrowserPolicy {
    /// Fail-closed default. Unknown origins denied unless explicitly
    /// allow-listed. Pre-v0.2.1 this was fail-open — see
    /// `STABILITY-v0.2.0.md` MAJOR #6.
    fn default() -> Self {
        Self {
            default_action: PolicyAction::Deny,
            allowed_origins: Vec::new(),
            denied_origins: Vec::new(),
            dns_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl PartialEq for BrowserPolicy {
    fn eq(&self, other: &Self) -> bool {
        self.default_action == other.default_action
            && self.allowed_origins == other.allowed_origins
            && self.denied_origins == other.denied_origins
    }
}

impl BrowserPolicy {
    /// Construct a policy from the three operator-facing fields. The DNS
    /// cache starts empty.
    pub fn new(
        default_action: PolicyAction,
        allowed_origins: Vec<String>,
        denied_origins: Vec<String>,
    ) -> Self {
        Self {
            default_action,
            allowed_origins,
            denied_origins,
            dns_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Check a URL. Convenience wrapper returning `Result<(), PolicyError>`
    /// — useful in TDD assertions.
    pub fn check_url(&self, url: &str) -> Result<(), PolicyError> {
        match self.evaluate(url) {
            PolicyOutcome::Allow => Ok(()),
            PolicyOutcome::Deny { reason } => Err(PolicyError::Violation { reason }),
            PolicyOutcome::Suspend { url } => Err(PolicyError::Violation {
                reason: format!("suspend (ask required): {url}"),
            }),
        }
    }

    /// Full structured outcome.
    pub fn evaluate(&self, url_str: &str) -> PolicyOutcome {
        let parsed = match Url::parse(url_str) {
            Ok(u) => u,
            Err(e) => {
                return PolicyOutcome::Deny {
                    reason: format!("invalid URL: {e}"),
                };
            }
        };

        // 1. Scheme allowlist — `http` and `https` only.
        let scheme = parsed.scheme().to_ascii_lowercase();
        if !ALLOWED_SCHEMES.iter().any(|s| *s == scheme) {
            return PolicyOutcome::Deny {
                reason: format!(
                    "scheme {scheme:?} not in allow list {ALLOWED_SCHEMES:?} \
                     (refused: javascript / data / blob / file / ftp / ...)"
                ),
            };
        }

        // 2. Hostname checks (IP literals + loopback names + legacy IPv4
        //    encodings + IPv4-mapped IPv6).
        if let Some(host) = parsed.host_str() {
            if let Some(reason) = blocked_host_reason(host) {
                return PolicyOutcome::Deny { reason };
            }

            // 3. Denied origins (suffix glob).
            for pat in &self.denied_origins {
                if origin_matches(host, pat) {
                    return PolicyOutcome::Deny {
                        reason: format!("origin {host} matches denied pattern {pat}"),
                    };
                }
            }

            // 4. Allowed origins gate (if non-empty, must match).
            if !self.allowed_origins.is_empty() {
                let any_match = self.allowed_origins.iter().any(|p| origin_matches(host, p));
                if !any_match {
                    return match self.default_action {
                        PolicyAction::Allow | PolicyAction::Deny => PolicyOutcome::Deny {
                            reason: format!(
                                "origin {host} not in allow list {:?}",
                                self.allowed_origins
                            ),
                        },
                        PolicyAction::Ask => PolicyOutcome::Suspend {
                            url: url_str.to_string(),
                        },
                    };
                }
            } else {
                // Empty allow list — fall through to default action.
                match self.default_action {
                    PolicyAction::Allow => {}
                    PolicyAction::Deny => {
                        return PolicyOutcome::Deny {
                            reason: format!(
                                "default_action=Deny and no rules matched origin {host}"
                            ),
                        };
                    }
                    PolicyAction::Ask => {
                        return PolicyOutcome::Suspend {
                            url: url_str.to_string(),
                        };
                    }
                }
            }
        }

        PolicyOutcome::Allow
    }

    /// DNS-rebinding TOFU check. Call this when the *resolved* IP for a
    /// hostname is known (e.g. from the OS resolver). The first call
    /// records the IP; subsequent calls with a different IP for the same
    /// hostname return `PolicyOutcome::Deny`.
    ///
    /// Note: this is in addition to [`evaluate`]. Backends that resolve
    /// DNS themselves (or care about rebinding) should call BOTH:
    /// `evaluate(url)` then `check_resolved_host(host, ip)`.
    pub fn check_resolved_host(&self, host: &str, ip: IpAddr) -> PolicyOutcome {
        // Block resolved-IP categories the same way [`blocked_host_reason`]
        // blocks IP literals — same set, same reasons.
        if let Some(reason) = blocked_resolved_ip_reason(host, ip) {
            return PolicyOutcome::Deny { reason };
        }

        let mut cache = self.dns_cache.lock();
        match cache.get(host) {
            Some(&first) if first != ip => PolicyOutcome::Deny {
                reason: format!(
                    "DNS rebinding refused: {host} resolved to {ip}, \
                     first-seen resolve was {first}"
                ),
            },
            Some(_) => PolicyOutcome::Allow,
            None => {
                cache.insert(host.to_string(), ip);
                PolicyOutcome::Allow
            }
        }
    }

    /// Number of host pins in the DNS-rebinding cache. Test / introspection
    /// helper.
    pub fn dns_cache_len(&self) -> usize {
        self.dns_cache.lock().len()
    }

    /// Construct a `reqwest::redirect::Policy` that re-evaluates this
    /// `BrowserPolicy` on every redirect hop. Backends that follow
    /// redirects via reqwest MUST install this on their client builder
    /// so a 3xx to a metadata / loopback / data-URI target is refused.
    ///
    /// Cap on redirect-chain length: 10 (reqwest default-ish).
    pub fn reqwest_redirect_policy(&self) -> reqwest::redirect::Policy {
        const MAX_HOPS: usize = 10;
        // Clone the operator-facing fields by value; share the DNS
        // cache by `Arc` so per-hop checks update the same TOFU set.
        let snapshot = BrowserPolicy {
            default_action: self.default_action,
            allowed_origins: self.allowed_origins.clone(),
            denied_origins: self.denied_origins.clone(),
            dns_cache: Arc::clone(&self.dns_cache),
        };
        reqwest::redirect::Policy::custom(move |attempt| {
            let url = attempt.url().to_string();
            if attempt.previous().len() >= MAX_HOPS {
                return attempt.error(format!("redirect chain exceeded {MAX_HOPS} hops at {url}"));
            }
            match snapshot.evaluate(&url) {
                PolicyOutcome::Allow => attempt.follow(),
                PolicyOutcome::Deny { reason } => attempt.error(format!(
                    "redirect to {url} refused by BrowserPolicy: {reason}"
                )),
                PolicyOutcome::Suspend { url: u } => attempt.error(format!(
                    "redirect to {u} requires approval (Ask policy); \
                     backend follow-through not supported on redirect hop"
                )),
            }
        })
    }
}

/// Returns `Some(reason)` if `host` (string from `Url::host_str`) is in one
/// of the hardcoded block lists. Handles:
///
///   * loopback hostnames (`localhost`, `*.localhost`),
///   * IPv4 literals — including legacy octal / hex / decimal-overflow
///     encodings that bypass `IpAddr::from_str`,
///   * IPv6 literals — including IPv4-mapped IPv6 (`::ffff:a.b.c.d`).
fn blocked_host_reason(host: &str) -> Option<String> {
    // Loopback hostnames.
    let host_lc = host.to_ascii_lowercase();
    if host_lc == "localhost" || host_lc.ends_with(".localhost") {
        return Some(format!("loopback hostname blocked: {host}"));
    }

    // Strip the surrounding brackets that `url::Url::host_str()` returns for
    // IPv6 literals (e.g. "[::1]" → "::1") so `IpAddr::from_str` can parse them.
    let ip_str = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);

    // Strict-parse first (covers `127.0.0.1`, `::1`, `169.254.169.254`).
    if let Ok(ip) = IpAddr::from_str(ip_str) {
        return blocked_ip_literal_reason(host, ip);
    }

    // Strict parse failed — try the loose IPv4 parser to catch legacy
    // octal / hex / decimal-overflow encodings that browsers accept but
    // `IpAddr::from_str` rejects.
    if let Some(v4) = parse_ipv4_loose(host) {
        return blocked_ip_literal_reason(host, IpAddr::V4(v4))
            .or_else(|| Some(format!("legacy IPv4 encoding refused: {host} -> {v4}")))
            .map(|reason| format!("{reason} (loose-parsed)"));
    }

    None
}

/// Reusable IP-literal block-list check. Split out from
/// [`blocked_host_reason`] so the resolved-IP path can use the same rules.
fn blocked_ip_literal_reason(host: &str, ip: IpAddr) -> Option<String> {
    match ip {
        IpAddr::V4(v4) => blocked_v4_reason(host, v4),
        IpAddr::V6(v6) => {
            // IPv4-mapped: `::ffff:a.b.c.d` — extract embedded v4 and
            // re-run the IPv4 rules. This closes MAJOR #6 from
            // SECURITY-v0.2.0.md.
            if let Some(v4) = ipv4_mapped(v6)
                && let Some(reason) = blocked_v4_reason(host, v4)
            {
                return Some(format!("{reason} (IPv4-mapped IPv6: {host} -> {v4})"));
            }
            blocked_v6_reason(host, v6)
        }
    }
}

fn blocked_v4_reason(host: &str, v4: Ipv4Addr) -> Option<String> {
    // Metadata endpoint (link-local for AWS / GCP / Azure / OpenStack).
    if v4.octets() == [169, 254, 169, 254] {
        return Some(format!(
            "cloud metadata endpoint blocked: {host} (169.254.169.254)"
        ));
    }
    if v4.is_loopback() {
        return Some(format!("loopback IP blocked: {host}"));
    }
    if v4.is_private() {
        return Some(format!("RFC 1918 private IP blocked: {host}"));
    }
    // Link-local block (169.254/16 minus metadata, but block all to be safe).
    if v4.is_link_local() {
        return Some(format!("link-local IP blocked: {host}"));
    }
    // Block CGN range (100.64.0.0/10, RFC 6598) and "this network"
    // (0.0.0.0/8) which are private-ish.
    let octets = v4.octets();
    if octets[0] == 0 {
        return Some(format!("\"this network\" 0.0.0.0/8 IP blocked: {host}"));
    }
    if octets[0] == 100 && (octets[1] & 0xc0) == 0x40 {
        return Some(format!("RFC 6598 CGN private IP blocked: {host}"));
    }
    // Multicast / broadcast: not a typical SSRF target but conservative
    // to block.
    if v4.is_multicast() || v4.is_broadcast() {
        return Some(format!("multicast/broadcast IP blocked: {host}"));
    }
    None
}

fn blocked_v6_reason(host: &str, v6: Ipv6Addr) -> Option<String> {
    if v6.is_loopback() {
        return Some(format!("loopback IP blocked: {host}"));
    }
    let segments = v6.segments();
    // Unique-local addresses — `fc00::/7`. First byte high-7 bits == 0xfc>>1.
    let first_byte = (segments[0] >> 8) as u8;
    if (first_byte & 0xfe) == 0xfc {
        return Some(format!("IPv6 ULA private IP blocked: {host}"));
    }
    // Link-local — `fe80::/10`. Top 10 bits == 0xfe80 >> 6.
    if (segments[0] & 0xffc0) == 0xfe80 {
        return Some(format!("IPv6 link-local IP blocked: {host}"));
    }
    // Multicast — `ff00::/8`.
    if (segments[0] & 0xff00) == 0xff00 {
        return Some(format!("IPv6 multicast IP blocked: {host}"));
    }
    None
}

/// Returns `Some(IPv4)` if `v6` is an IPv4-mapped IPv6 address
/// (`::ffff:a.b.c.d` per RFC 4291 §2.5.5.2). Stable manual implementation
/// — equivalent to the unstable `Ipv6Addr::to_ipv4_mapped`.
fn ipv4_mapped(v6: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = v6.segments();
    if s[0] == 0 && s[1] == 0 && s[2] == 0 && s[3] == 0 && s[4] == 0 && s[5] == 0xffff {
        let octets = v6.octets();
        Some(Ipv4Addr::new(
            octets[12], octets[13], octets[14], octets[15],
        ))
    } else {
        None
    }
}

/// Reusable IP-literal check for a resolved-host IP. Mirrors
/// [`blocked_ip_literal_reason`] but with a different reason prefix so
/// resolved-IP denials are distinguishable in logs.
fn blocked_resolved_ip_reason(host: &str, ip: IpAddr) -> Option<String> {
    blocked_ip_literal_reason(host, ip)
        .map(|reason| format!("DNS resolved {host} to blocked IP: {reason}"))
}

/// Parse legacy IPv4 encodings that browsers accept but `IpAddr::from_str`
/// rejects:
///
///   * `0177.0.0.1`         — leading-zero octal octet
///   * `0x7f.0.0.1`         — hex octet
///   * `127.0x1`            — two-octet form (a.b → a/24 . b/8)
///   * `2130706433`         — single-integer 32-bit form
///   * `0x7f000001`         — single-integer 32-bit hex form
///
/// Returns `None` if the input isn't a valid IPv4 in any of these forms.
fn parse_ipv4_loose(host: &str) -> Option<Ipv4Addr> {
    // Sanity: hostnames containing colons / brackets are not IPv4.
    if host.is_empty() || host.contains(':') || host.contains('[') || host.contains(']') {
        return None;
    }
    let parts: Vec<&str> = host.split('.').collect();
    if parts.is_empty() || parts.len() > 4 {
        return None;
    }
    // Disallow trailing dot or empty parts mid-string (browsers tolerate
    // some forms but we err on the side of rejection — the URL won't pass
    // policy either way since the host is non-canonical).
    if parts.iter().any(|p| p.is_empty()) {
        return None;
    }

    // Parse each part as an integer in the appropriate base.
    let mut nums: Vec<u64> = Vec::with_capacity(parts.len());
    for p in &parts {
        let n = parse_legacy_octet(p)?;
        nums.push(n);
    }

    // Combine according to the count of parts. Rules from inet_aton(3):
    //
    //   4 parts: a.b.c.d  -> each must fit in u8.
    //   3 parts: a.b.c    -> c is u16, others u8.
    //   2 parts: a.b      -> b is up to 24-bit, a is u8.
    //   1 part:  a        -> a is the full 32-bit address.
    let bits: u32 = match nums.len() {
        4 => {
            if nums.iter().any(|n| *n > 0xff) {
                return None;
            }
            ((nums[0] as u32) << 24)
                | ((nums[1] as u32) << 16)
                | ((nums[2] as u32) << 8)
                | (nums[3] as u32)
        }
        3 => {
            if nums[0] > 0xff || nums[1] > 0xff || nums[2] > 0xffff {
                return None;
            }
            ((nums[0] as u32) << 24) | ((nums[1] as u32) << 16) | (nums[2] as u32)
        }
        2 => {
            if nums[0] > 0xff || nums[1] > 0x00ff_ffff {
                return None;
            }
            ((nums[0] as u32) << 24) | (nums[1] as u32)
        }
        1 => {
            if nums[0] > 0xffff_ffff {
                return None;
            }
            nums[0] as u32
        }
        _ => return None,
    };
    Some(Ipv4Addr::from(bits.to_be_bytes()))
}

/// Parse a single octet in legacy form: hex (`0x...`), octal (leading `0`),
/// or decimal. Returns `None` if the string fails all three.
fn parse_legacy_octet(s: &str) -> Option<u64> {
    if let Some(rest) = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")) {
        // Hex.
        if rest.is_empty() {
            return None;
        }
        u64::from_str_radix(rest, 16).ok()
    } else if s.starts_with('0') && s.len() > 1 {
        // Octal — but only if every remaining char is in [0-7].
        // Pure `"0"` is decimal-zero, not octal.
        u64::from_str_radix(s, 8).ok()
    } else {
        s.parse::<u64>().ok()
    }
}

/// Suffix-glob match: `*.example.com` matches `foo.example.com` and
/// `example.com`. Plain `example.com` matches only the exact host.
fn origin_matches(host: &str, pattern: &str) -> bool {
    if let Some(suffix) = pattern.strip_prefix("*.") {
        host == suffix || host.ends_with(&format!(".{suffix}"))
    } else {
        host == pattern
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow_default() -> BrowserPolicy {
        // Construct a policy with the *pre-v0.2.1* fail-open behavior for
        // tests that need to verify hard-coded blocks fire on top of an
        // otherwise-Allow default.
        BrowserPolicy::new(PolicyAction::Allow, Vec::new(), Vec::new())
    }

    #[test]
    fn blocks_aws_metadata_endpoint() {
        let policy = allow_default();
        let r = policy.check_url("http://169.254.169.254/latest/meta-data/");
        assert!(r.is_err(), "metadata endpoint must be blocked");
        assert!(format!("{r:?}").to_lowercase().contains("metadata"));
    }

    #[test]
    fn blocks_rfc_1918_private() {
        let policy = allow_default();
        for ip in ["10.0.0.1", "172.16.0.1", "192.168.0.1"] {
            let r = policy.check_url(&format!("http://{ip}/"));
            assert!(r.is_err(), "RFC 1918 IP {ip} must be blocked");
        }
    }

    #[test]
    fn blocks_loopback() {
        let policy = allow_default();
        for u in ["http://localhost/", "http://127.0.0.1/", "http://[::1]/"] {
            assert!(
                policy.check_url(u).is_err(),
                "loopback URL {u} must be blocked"
            );
        }
    }

    #[test]
    fn blocks_non_http_schemes() {
        let policy = allow_default();
        for u in [
            "file:///etc/passwd",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "blob:https://example.com/abc",
            "ftp://example.com/x",
            "gopher://example.com/x",
            "view-source:https://example.com/",
        ] {
            let r = policy.check_url(u);
            assert!(r.is_err(), "scheme refused expected for {u}, got {r:?}");
            assert!(
                format!("{r:?}").to_lowercase().contains("scheme"),
                "expected scheme-refusal message, got {r:?}"
            );
        }
    }

    #[test]
    fn allowed_origins_whitelist_overrides() {
        let policy = BrowserPolicy::new(
            PolicyAction::Allow,
            vec!["*.example.com".into()],
            Vec::new(),
        );
        assert!(policy.check_url("https://foo.example.com/").is_ok());
        assert!(policy.check_url("https://example.com/").is_ok());
        let r = policy.check_url("https://other.org/");
        assert!(r.is_err(), "non-matching origin must be denied");
    }

    #[test]
    fn denied_origins_override_allow_list_gap() {
        let policy = BrowserPolicy::new(
            PolicyAction::Allow,
            Vec::new(),
            vec!["*.evil.example".into()],
        );
        assert!(policy.check_url("https://foo.evil.example/").is_err());
        assert!(policy.check_url("https://safe.example/").is_ok());
    }

    #[test]
    fn ask_default_routes_to_suspend() {
        let policy = BrowserPolicy::new(PolicyAction::Ask, Vec::new(), Vec::new());
        let outcome = policy.evaluate("https://unknown.example.org/");
        assert!(
            matches!(outcome, PolicyOutcome::Suspend { .. }),
            "Ask default must route to Suspend, got {outcome:?}"
        );
    }

    #[test]
    fn ipv6_loopback_and_ula_blocked() {
        let policy = allow_default();
        assert!(policy.check_url("http://[::1]/").is_err());
        assert!(policy.check_url("http://[fc00::1]/").is_err());
        assert!(policy.check_url("http://[fe80::1]/").is_err());
    }

    #[test]
    fn default_is_fail_closed() {
        let policy = BrowserPolicy::default();
        // No allow-list and Deny default → arbitrary origin refused.
        let r = policy.check_url("https://example.com/");
        assert!(r.is_err(), "fail-closed default must deny example.com");
        assert!(
            format!("{r:?}").contains("default_action=Deny"),
            "expected Deny-default reason, got {r:?}"
        );
    }

    #[test]
    fn legacy_ipv4_octal_blocked() {
        let policy = allow_default();
        let r = policy.check_url("http://0177.0.0.1/");
        assert!(r.is_err(), "octal IP {r:?}");
        let r = policy.check_url("http://0177.0.0.2/"); // not loopback
        // 0177 octal = 127 — still loopback.
        assert!(r.is_err(), "0177.0.0.2 should still hit 127.0.0.2 loopback");
    }

    #[test]
    fn legacy_ipv4_hex_blocked() {
        let policy = allow_default();
        let r = policy.check_url("http://0x7f.0.0.1/");
        assert!(r.is_err(), "hex IP {r:?}");
    }

    #[test]
    fn legacy_ipv4_decimal_blocked() {
        let policy = allow_default();
        let r = policy.check_url("http://2130706433/"); // 127.0.0.1
        assert!(r.is_err(), "decimal IP {r:?}");
    }

    #[test]
    fn ipv4_mapped_ipv6_blocked() {
        let policy = allow_default();
        // IPv4-mapped IPv6 form of 169.254.169.254 — must block.
        let r = policy.check_url("http://[::ffff:169.254.169.254]/");
        assert!(r.is_err(), "IPv4-mapped IPv6 metadata {r:?}");
        // Loopback.
        let r = policy.check_url("http://[::ffff:127.0.0.1]/");
        assert!(r.is_err(), "IPv4-mapped IPv6 loopback {r:?}");
    }

    #[test]
    fn dns_rebinding_tofu() {
        let policy = BrowserPolicy::new(
            PolicyAction::Allow,
            vec!["foo.example.com".into()],
            Vec::new(),
        );
        // First resolve to a benign public IP.
        let first = "203.0.113.5".parse().unwrap();
        let r1 = policy.check_resolved_host("foo.example.com", first);
        assert!(matches!(r1, PolicyOutcome::Allow));
        // Same IP again — still OK.
        let r2 = policy.check_resolved_host("foo.example.com", first);
        assert!(matches!(r2, PolicyOutcome::Allow));
        // Rebind to loopback — refused.
        let second = "127.0.0.1".parse().unwrap();
        let r3 = policy.check_resolved_host("foo.example.com", second);
        assert!(
            matches!(r3, PolicyOutcome::Deny { .. }),
            "rebind must be refused, got {r3:?}"
        );
    }

    #[test]
    fn dns_resolved_to_blocked_ip_is_refused_on_first_resolve() {
        let policy = BrowserPolicy::new(
            PolicyAction::Allow,
            vec!["foo.example.com".into()],
            Vec::new(),
        );
        // First-and-only resolve to a private IP — must refuse even
        // before TOFU has anything pinned.
        let priv_ip = "10.0.0.5".parse().unwrap();
        let r = policy.check_resolved_host("foo.example.com", priv_ip);
        assert!(matches!(r, PolicyOutcome::Deny { .. }));
    }

    #[test]
    fn parse_ipv4_loose_handles_all_forms() {
        assert_eq!(
            parse_ipv4_loose("0177.0.0.1"),
            Some(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(
            parse_ipv4_loose("0x7f.0.0.1"),
            Some(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(
            parse_ipv4_loose("2130706433"),
            Some(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(
            parse_ipv4_loose("0x7f000001"),
            Some(Ipv4Addr::new(127, 0, 0, 1))
        );
        assert_eq!(parse_ipv4_loose("127.1"), Some(Ipv4Addr::new(127, 0, 0, 1)));
        // Not IPv4-shaped.
        assert_eq!(parse_ipv4_loose("example.com"), None);
        assert_eq!(parse_ipv4_loose("::1"), None);
        // Strict-parseable — also fine to return Some here.
        assert_eq!(parse_ipv4_loose(""), None);
        // Out-of-range octet rejected.
        assert_eq!(parse_ipv4_loose("999.0.0.1"), None);
    }

    #[test]
    fn ipv4_mapped_helper_extracts_embedded_v4() {
        let v6: Ipv6Addr = "::ffff:127.0.0.1".parse().unwrap();
        assert_eq!(ipv4_mapped(v6), Some(Ipv4Addr::new(127, 0, 0, 1)));
        let v6_loopback: Ipv6Addr = "::1".parse().unwrap();
        assert_eq!(ipv4_mapped(v6_loopback), None);
    }
}
