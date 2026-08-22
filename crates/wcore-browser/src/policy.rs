//! `BrowserPolicy` — URL-level enforcement gate.
//!
//! ## Hard-coded blocks (always-on, regardless of allow / deny lists)
//!
//!   * RFC 1918 private ranges (10/8, 172.16/12, 192.168/16).
//!   * Loopback (127/8, `localhost`, `*.localhost`, `::1`) — the ONE
//!     hard block with a recoverable escape hatch; see
//!     [`LoopbackCapability`] and the "Loopback capability" section below.
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
//! ## Loopback capability (gh#911)
//!
//! Loopback was previously a dead end: `http://localhost:3000` was refused
//! with no recovery path at all, so an operator wanting to drive a local dev
//! server had nothing to turn on. [`LoopbackCapability`] is the recoverable,
//! versioned, scope-bearing grant that reopens exactly that door and nothing
//! else. It is deliberately NOT a sandbox disable:
//!
//!   * It fails closed on absent, version-mismatched, unscoped or portless
//!     grant data — every validation failure keeps loopback blocked.
//!   * It authorizes only the ports the operator enumerated. A grant for
//!     `3000` does not reach the Camoufox sidecar on `9377`.
//!   * It relaxes ONLY loopback (`localhost`, `*.localhost`, 127/8, `::1`,
//!     IPv4-mapped loopback). RFC 1918, link-local, cloud metadata, IPv6 ULA
//!     and `0.0.0.0/8` all stay refused with a grant in hand — including at
//!     the granted port, so a grant cannot be spent across categories.
//!   * It never relaxes [`check_resolved_host`]. A public hostname that
//!     resolves to 127.0.0.1 is still a rebinding attack, grant or no grant.
//!     The one place the grant IS consulted ahead of resolution is
//!     [`BrowserPolicy::evaluate_navigation_target`], which skips the lookup
//!     for a canonical loopback host the grant already authorizes — resolving
//!     `localhost` would otherwise refuse it unconditionally and delete the
//!     recovery path entirely.
//!   * `denied_origins` still wins over an authorized grant.
//!
//! ## DNS resolution gate (gh#1053)
//!
//! [`BrowserPolicy::evaluate`] decides from the URL **string** alone, so a
//! public NAME that resolves to `169.254.169.254` or into RFC 1918 passes it
//! — there is nothing in the string to object to.
//! [`BrowserPolicy::evaluate_navigation_target`] is the gate the executed
//! path calls: it runs `evaluate` first, then resolves the host and requires
//! EVERY resolved address to clear the same block-list, and fails closed on a
//! host that resolves to nothing.
//!
//! ### Why there is no TOFU pin on this path
//!
//! An earlier shape of this gate also pinned the answer (via
//! [`BrowserPolicy::check_resolved_host`]) so a later navigation answering
//! differently was refused. That is wrong twice over:
//!
//!   * It buys nothing. A rebind means the second answer points somewhere the
//!     policy blocks — and every address of every answer already has to clear
//!     `blocked_resolved_ip_reason` above. A public-to-public change is not an
//!     attack this policy has an opinion about.
//!   * It breaks real hosts. MEASURED 2026-08-22 against the upstream resolver
//!     with the local cache bypassed, 11 re-queries over ~60s: the answer set
//!     for `s3.amazonaws.com` was fully DISJOINT from the first answer on
//!     11/11 re-queries, `www.akamai.com` on 9/11, `cdn.jsdelivr.net` on 8/11,
//!     `outlook.office365.com` on 6/11. (Control: `www.wikipedia.org` 0/11 —
//!     the measurement can report "stable".) Neither the first address nor an
//!     order-independent representative such as `min()` survives that, so the
//!     pin refuses the SECOND navigation to those hosts for the rest of the
//!     session, reported as a rebinding attack.
//!
//! `check_resolved_host` itself is unchanged and still available to a caller
//! that resolves once and dials that exact address.
//!
//! **What the gate does not close.** Camoufox is a SIDECAR: Firefox performs
//! its own DNS resolution in another process, so the addresses it actually
//! dials are not the ones checked here. This closes static DNS SSRF. It does
//! NOT close TTL=0 intra-navigation rebinding.
//!
//! ## Redirect re-check
//!
//! [`BrowserPolicy::reqwest_redirect_policy`] returns a
//! [`reqwest::redirect::Policy`] that re-evaluates this policy on every
//! redirect hop of a request **we** issue. Backends that follow redirects via
//! reqwest MUST install it on their client builder. It is the string-only
//! `evaluate`, because a `reqwest` redirect closure is synchronous and cannot
//! block an async worker on a DNS lookup. Redirects the sidecar follows in its
//! own process are covered instead by re-checking the landing URL through the
//! resolution gate after the fact.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
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

/// Schema version of the loopback capability grant. A grant carrying any
/// other value — including the `0` a missing field deserializes to — is
/// refused, so unknown producer data fails closed rather than being
/// interpreted under the wrong schema (gh#911 acceptance: "Unknown or
/// malformed capability data fails closed").
pub const LOOPBACK_CAPABILITY_VERSION: u32 = 1;

/// Explicit, scoped, human-granted authority to reach loopback.
///
/// Every field is required to be affirmatively set: [`Default`] is the
/// no-authority value and each validation gate below refuses rather than
/// assumes. See the crate-level "Loopback capability" section for the
/// threat model this shape is answering.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct LoopbackCapability {
    /// Master switch. `false` (default) keeps loopback hard-blocked.
    pub enabled: bool,
    /// Must equal [`LOOPBACK_CAPABILITY_VERSION`].
    pub schema_version: u32,
    /// The session / profile this grant was issued for. Carried into the
    /// decision text so a consumer can report which target authorized the
    /// access. Empty is refused: an unattributable grant is not explicit
    /// human authority.
    pub session_scope: String,
    /// Ports this grant authorizes on loopback. Empty is refused — there is
    /// deliberately no "all ports" spelling, because that is the broad
    /// sandbox disable gh#911 rules out.
    pub ports: Vec<u16>,
}

impl LoopbackCapability {
    /// `Ok(scope)` when this grant authorizes `port`; `Err(why)` naming the
    /// specific gate that refused otherwise. The `Err` text is operator-facing
    /// — it is what a stuck operator reads to find out what is wrong with the
    /// grant they just wrote.
    pub fn authorize(&self, port: Option<u16>) -> Result<&str, String> {
        if !self.enabled {
            return Err(
                "no loopback capability granted (browser.policy.loopback.enabled is false)".into(),
            );
        }
        if self.schema_version != LOOPBACK_CAPABILITY_VERSION {
            return Err(format!(
                "loopback capability refused: schema_version {} is not the supported \
                 version {LOOPBACK_CAPABILITY_VERSION}",
                self.schema_version
            ));
        }
        if self.session_scope.trim().is_empty() {
            return Err(
                "loopback capability refused: session_scope is empty, so the grant names no \
                 session or profile to attribute the access to"
                    .into(),
            );
        }
        if self.ports.is_empty() {
            return Err(
                "loopback capability refused: ports is empty, and there is no \
                 all-ports spelling — enumerate the local ports the grant covers"
                    .into(),
            );
        }
        let Some(port) = port else {
            return Err(
                "loopback capability refused: URL has no port and no known default \
                 for its scheme, so it cannot be matched against the granted ports"
                    .into(),
            );
        };
        if !self.ports.contains(&port) {
            return Err(format!(
                "loopback capability refused: port {port} is not in the granted ports {:?}",
                self.ports
            ));
        }
        Ok(self.session_scope.trim())
    }
}

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
    /// Recoverable local-only loopback grant (gh#911). Absent / malformed
    /// grant data leaves loopback hard-blocked.
    #[serde(default)]
    pub loopback: LoopbackCapability,
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
            loopback: LoopbackCapability::default(),
            dns_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

impl PartialEq for BrowserPolicy {
    fn eq(&self, other: &Self) -> bool {
        self.default_action == other.default_action
            && self.allowed_origins == other.allowed_origins
            && self.denied_origins == other.denied_origins
            && self.loopback == other.loopback
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
            loopback: LoopbackCapability::default(),
            dns_cache: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Attach a loopback grant. Separate from [`new`](Self::new) so that
    /// every existing construction site keeps the no-authority default and
    /// granting loopback is always a visible, deliberate call.
    pub fn with_loopback(mut self, loopback: LoopbackCapability) -> Self {
        self.loopback = loopback;
        self
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
            let hard_block = blocked_host_reason(host);

            // 2a. The loopback capability (gh#911) is the ONE recoverable
            //     hard block. It is consulted only for canonical loopback
            //     spellings, so a grant cannot be spent on RFC 1918,
            //     link-local, metadata, ULA, `0.0.0.0/8`, or an obfuscated
            //     legacy encoding of 127.0.0.1 — those return below with
            //     their original reason regardless of what was granted.
            let loopback_scope = if hard_block.is_some() && is_canonical_loopback_host(host) {
                match self.loopback.authorize(parsed.port_or_known_default()) {
                    Ok(scope) => Some(scope.to_string()),
                    Err(why) => {
                        return PolicyOutcome::Deny {
                            reason: format!(
                                "{}; {why}",
                                hard_block.unwrap_or_else(|| "loopback blocked".into())
                            ),
                        };
                    }
                }
            } else {
                if let Some(reason) = hard_block {
                    return PolicyOutcome::Deny { reason };
                }
                None
            };

            // 3. Denied origins (suffix glob). Still wins over an authorized
            //    loopback grant — the deny list is unconditional.
            for pat in &self.denied_origins {
                if origin_matches(host, pat) {
                    return PolicyOutcome::Deny {
                        reason: format!("origin {host} matches denied pattern {pat}"),
                    };
                }
            }

            // An authorized grant IS the explicit allow decision for this
            // host and port. Requiring the operator to ALSO allow-list
            // `localhost` would mean the recovery path Desktop offers still
            // does not work on its own, which is the gh#911 defect.
            if loopback_scope.is_some() {
                return PolicyOutcome::Allow;
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

    /// The gate the executed path calls: [`evaluate`](Self::evaluate) plus a
    /// DNS-resolution check on the host it just approved.
    ///
    /// gh#1053: every URL-bearing op used to reach the backend without its
    /// host ever being resolved, because
    /// [`check_resolved_host`](Self::check_resolved_host) had no production
    /// caller at all. A public name pointing at the cloud metadata endpoint
    /// was therefore permitted outright.
    ///
    /// The order below is load-bearing:
    ///
    ///   1. The string-only gate decides FIRST. A clean resolution never
    ///      launders an allow-list miss — this is an additional refusal, never
    ///      a new way in.
    ///   2. An IP literal already carries its destination; `evaluate` has
    ///      judged it and no lookup is spent on it.
    ///   3. gh#911: a canonical loopback host holding an AUTHORISING grant
    ///      skips resolution. `check_resolved_host` refuses loopback
    ///      unconditionally, so feeding it every resolution would deny
    ///      `http://localhost:3000/` with a valid grant and delete the only
    ///      loopback recovery path an operator has.
    ///   4. EVERY resolved address must clear the block-list, not just the
    ///      first: a host commonly has several A records, and a first-only
    ///      gate is one an attacker picks their way past by ordering the
    ///      answer.
    ///   5. A host that resolves to nothing fails CLOSED. "I could not check"
    ///      is not "allowed".
    ///
    /// There is deliberately NO TOFU pin on this path — see the module
    /// header. Step 4 is what actually stops a rebind, and a pin on top of it
    /// refuses ordinary rotating multi-A hosts for no security gain.
    ///
    /// See the crate-level "DNS resolution gate" section for the residual gap
    /// this cannot close (the sidecar resolves DNS itself, so TTL=0
    /// intra-navigation rebinding stays open).
    pub fn evaluate_navigation_target(&self, url_str: &str) -> PolicyOutcome {
        self.evaluate_navigation_target_with(url_str, system_resolver)
    }

    /// Async form of
    /// [`evaluate_navigation_target`](Self::evaluate_navigation_target) for the
    /// executed path.
    ///
    /// The system resolver is BLOCKING I/O that can stall for seconds against
    /// a slow or unreachable nameserver, and both production call sites sit
    /// inside `async fn`s — so the lookup runs on the blocking pool instead of
    /// an async worker. The clone shares `dns_cache` through its `Arc`, so the
    /// pin the blocking task records is the one this policy reads back.
    pub async fn evaluate_navigation_target_async(&self, url_str: &str) -> PolicyOutcome {
        let policy = self.clone();
        let url = url_str.to_string();
        match tokio::task::spawn_blocking(move || policy.evaluate_navigation_target(&url)).await {
            Ok(outcome) => outcome,
            // A panicked or cancelled resolution task is not an allow.
            Err(e) => PolicyOutcome::Deny {
                reason: format!("DNS resolution gate did not complete: {e}"),
            },
        }
    }

    /// Resolver-injected seam behind
    /// [`evaluate_navigation_target`](Self::evaluate_navigation_target).
    /// Private on purpose: the public surface passes the system resolver, and
    /// only the in-crate tests get to choose the answers.
    fn evaluate_navigation_target_with(&self, url_str: &str, resolve: Resolver) -> PolicyOutcome {
        // 1. String-only gate first.
        match self.evaluate(url_str) {
            PolicyOutcome::Allow => {}
            other => return other,
        }

        let Ok(parsed) = Url::parse(url_str) else {
            // Not reachable in practice — `evaluate` denies an unparseable URL
            // before this point — but fail closed rather than assume.
            return PolicyOutcome::Deny {
                reason: format!("invalid URL at the DNS resolution gate: {url_str}"),
            };
        };
        let Some(host) = parsed.host_str() else {
            // No host to resolve (and nothing for an attacker to point
            // anywhere); `evaluate` already had the final say.
            return PolicyOutcome::Allow;
        };

        // 2. IP literal — nothing to resolve. Brackets stripped the same way
        //    `blocked_host_reason` strips them.
        let bare = host
            .strip_prefix('[')
            .and_then(|s| s.strip_suffix(']'))
            .unwrap_or(host);
        if IpAddr::from_str(bare).is_ok() {
            return PolicyOutcome::Allow;
        }

        // 3. gh#911 — a granted canonical loopback host is exempt from
        //    resolution, and ONLY while the grant authorizes this port.
        if is_canonical_loopback_host(host)
            && self
                .loopback
                .authorize(parsed.port_or_known_default())
                .is_ok()
        {
            return PolicyOutcome::Allow;
        }

        // 4/5. Resolve, and require the whole answer set to clear the gate.
        let addrs = resolve(host);
        if addrs.is_empty() {
            return PolicyOutcome::Deny {
                reason: format!(
                    "{host} resolved to no address at all; refused because the \
                     policy cannot tell where the request would land \
                     (DNS resolution gate)"
                ),
            };
        }
        for ip in &addrs {
            if let Some(reason) = blocked_resolved_ip_reason(host, *ip) {
                return PolicyOutcome::Deny { reason };
            }
        }

        // 6. NO TOFU PIN HERE — deliberately. See the "why there is no pin"
        //    section in the module header. The loop above is the check that
        //    stops a rebind; a pin on top of it stops nothing extra and,
        //    measured, refuses real hosts.
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
            loopback: self.loopback.clone(),
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

/// Resolver seam. Production passes [`system_resolver`]; the in-crate tests
/// pass a deterministic stub. Deliberately the same `fn(&str) -> Vec<IpAddr>`
/// shape `wcore-tools/src/url_safety.rs` already uses for the identical job.
///
/// `wcore_tools::url_safety::safe_url_pinned_ips` is NOT reused here: it
/// embeds url_safety's own block-list, which refuses loopback
/// unconditionally, and would delete the gh#911 loopback grant.
type Resolver = fn(&str) -> Vec<IpAddr>;

/// The system resolver. Port 0 — only the addresses matter, not connectivity.
fn system_resolver(host: &str) -> Vec<IpAddr> {
    match (host, 0u16).to_socket_addrs() {
        Ok(addrs) => addrs.map(|sa| sa.ip()).collect(),
        Err(_) => Vec::new(),
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

/// `true` only for hosts that unambiguously ARE loopback and can therefore be
/// reopened by a [`LoopbackCapability`].
///
/// Polarity matters here: this is an ALLOW-relaxation predicate, so every
/// uncertain input must answer `false`. Deliberately excluded:
///
///   * `0.0.0.0` and the rest of `0.0.0.0/8`, which many stacks route to the
///     local host but which is not a loopback address,
///   * every non-loopback category — `blocked_host_reason` keeps refusing
///     those with their own reason even at a granted port.
///
/// On the legacy IPv4 encodings (`0177.0.0.1`, `2130706433`, `127.1`, ...):
/// measured, `Url::parse` canonicalizes every one of them to `127.0.0.1`
/// before `evaluate` reads `host_str()`, so they never arrive here in their
/// obfuscated form. Strict-parsing (rather than consulting
/// `parse_ipv4_loose`) is therefore not a filter against them — it is the
/// guarantee that the address this predicate judges is byte-for-byte the
/// address the request will actually reach. Obfuscation cannot widen a grant
/// because it cannot change the destination.
fn is_canonical_loopback_host(host: &str) -> bool {
    let host_lc = host.to_ascii_lowercase();
    if host_lc == "localhost" || host_lc.ends_with(".localhost") {
        return true;
    }
    let ip_str = host
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .unwrap_or(host);
    // STRICT parse only — `parse_ipv4_loose` is intentionally not consulted.
    match IpAddr::from_str(ip_str) {
        Ok(IpAddr::V4(v4)) => v4.is_loopback(),
        Ok(IpAddr::V6(v6)) => {
            v6.is_loopback() || ipv4_mapped(v6).is_some_and(|v4| v4.is_loopback())
        }
        Err(_) => false,
    }
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

/// Reduce an operator-written origin pattern to the bare host
/// [`origin_matches`] can actually compare (gh#1075).
///
/// The field is named `allowed_ORIGINS`, and an origin in the web-platform
/// sense is scheme + host + port — so `https://x.example` is the spelling the
/// field name asks for, and it is exactly the spelling that could never match
/// anything. Nothing diagnosed it: a never-matching ALLOW entry silently
/// blocks the operator, and a never-matching DENY entry silently fails OPEN.
///
/// Normalisation lives here rather than in a constructor or the config loader
/// because it is the ONE point every construction path funnels through.
/// `BrowserPolicy` is built from operator config (`adapter.rs`), from the
/// plugin mirror (`wcore-plugin-api` → `browser_adapter.rs`), and directly by
/// serde — `allowed_origins` / `denied_origins` are public fields on a
/// `Deserialize` struct. A fix in `BrowserPolicy::new` alone leaves the serde
/// path broken.
///
/// Only the two schemes the gate itself accepts are stripped. A pattern
/// written with a scheme the allow-list refuses outright (`javascript:`,
/// `file:`, ...) is left verbatim, so it keeps matching nothing: an entry the
/// gate would never honour must not be resurrected by normalisation.
///
/// A port in the pattern is DROPPED, not honoured. Origin matching in this
/// policy has always been host-granular — a port never narrowed anything, it
/// only made the whole entry match nothing — so `x.example:8443` now admits
/// `x.example` on every port. The one port-scoped control here is the
/// [`LoopbackCapability`] grant, which is a separate field and is unaffected.
///
/// Borrows rather than allocating — every step is a prefix/suffix trim.
fn normalize_origin_pattern(pattern: &str) -> &str {
    let rest = strip_scheme_ci(pattern, "https://")
        .or_else(|| strip_scheme_ci(pattern, "http://"))
        .unwrap_or(pattern);
    // Path first: a host never contains `/`.
    let rest = rest.split('/').next().unwrap_or(rest);
    // Then the port. An IPv6 literal keeps its brackets, which is the form
    // `Url::host_str()` hands the matcher.
    if rest.starts_with('[') {
        return match rest.find(']') {
            Some(close) => &rest[..=close],
            None => rest,
        };
    }
    match rest.split_once(':') {
        Some((host, _port)) => host,
        None => rest,
    }
}

/// Case-insensitive scheme-prefix strip. `scheme` must already be lowercase.
fn strip_scheme_ci<'a>(pattern: &'a str, scheme: &str) -> Option<&'a str> {
    let head = pattern.get(..scheme.len())?;
    head.eq_ignore_ascii_case(scheme)
        .then(|| &pattern[scheme.len()..])
}

/// Suffix-glob match: `*.example.com` matches `foo.example.com` and
/// `example.com`. Plain `example.com` matches only the exact host. The
/// pattern is normalized first — see [`normalize_origin_pattern`].
fn origin_matches(host: &str, pattern: &str) -> bool {
    let pattern = normalize_origin_pattern(pattern);
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

    // ======================================================================
    // RED ARM -- gh#1053 resolution gate. These do NOT compile at 0ccaa90b:
    // `evaluate_navigation_target_with` does not exist yet, because the
    // resolution step has to be BUILT (0 hits for
    // to_socket_addrs|lookup_host|getaddrinfo across crates/wcore-browser;
    // positive control: 8 hits across `-- crates`).
    //
    // The seam is `fn(&str) -> Vec<IpAddr>`, deliberately the SAME shape
    // `wcore-tools/src/url_safety.rs:202-207` already uses, and deliberately
    // private -- the public surface is `evaluate_navigation_target(url)`,
    // which passes the system resolver. Integration tests cannot reach a
    // private seam, which is why the resolves-to-a-blocked-IP cases live
    // here and the hermetic `.invalid` cases live in
    // `tests/dns_resolution_gate_test.rs`.
    //
    // NOT covered by design, and the hint must say so: Camoufox is a
    // sidecar. Firefox resolves in its own process, so the addresses it
    // dials cannot be pinned. This closes static DNS SSRF and
    // cross-navigation rebinding via the TOFU cache; it does not close
    // TTL=0 intra-navigation rebinding.
    // ======================================================================

    /// `rebind.example` answers with the cloud metadata endpoint;
    /// `split.example` answers with one good and one bad address;
    /// `public.example` is clean; `gone.example` answers with nothing.
    fn fake_resolver(host: &str) -> Vec<IpAddr> {
        match host {
            "rebind.example" => vec!["169.254.169.254".parse().unwrap()],
            "inward.example" => vec!["10.1.2.3".parse().unwrap()],
            "split.example" => vec![
                "93.184.216.34".parse().unwrap(),
                "169.254.169.254".parse().unwrap(),
            ],
            "public.example" => vec!["93.184.216.34".parse().unwrap()],
            "localhost" => vec!["127.0.0.1".parse().unwrap()],
            _ => Vec::new(),
        }
    }

    /// A resolver that fails the test if it is consulted at all.
    fn never_resolved(host: &str) -> Vec<IpAddr> {
        panic!("the gate resolved {host}, which needs no resolution");
    }

    /// RED. The headline gh#1053 case: a public NAME that resolves to the
    /// cloud metadata endpoint. `evaluate()` alone allows it -- there is
    /// nothing in the URL string to object to.
    #[test]
    fn navigation_gate_refuses_a_name_that_resolves_to_the_metadata_endpoint() {
        let p = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
        assert!(
            matches!(p.evaluate("http://rebind.example/"), PolicyOutcome::Allow),
            "precondition: the string-only gate has no objection, which is the bug"
        );
        let r = p.evaluate_navigation_target_with("http://rebind.example/", fake_resolver);
        assert!(
            matches!(r, PolicyOutcome::Deny { .. }),
            "a name resolving to 169.254.169.254 must be refused, got {r:?}"
        );
    }

    /// RED. The RFC1918 variant of the same attack.
    #[test]
    fn navigation_gate_refuses_a_name_that_resolves_into_the_private_range() {
        let p = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
        let r = p.evaluate_navigation_target_with("http://inward.example/", fake_resolver);
        assert!(
            matches!(r, PolicyOutcome::Deny { .. }),
            "a name resolving to 10.1.2.3 must be refused, got {r:?}"
        );
    }

    /// RED. A host commonly has several A records. Checking only the first
    /// one is a gate an attacker picks their way past by ordering the answer.
    #[test]
    fn navigation_gate_refuses_when_only_one_of_several_addresses_is_blocked() {
        let p = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
        let r = p.evaluate_navigation_target_with("http://split.example/", fake_resolver);
        assert!(
            matches!(r, PolicyOutcome::Deny { .. }),
            "EVERY resolved address must clear the gate, not just the first, \
             got {r:?}"
        );
    }

    /// NEGATIVE CONTROL for all three above. A name resolving to a clean
    /// public address must pass, or the gate is just a blanket refusal.
    #[test]
    fn navigation_gate_allows_a_name_that_resolves_to_a_public_address() {
        let p = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
        let r = p.evaluate_navigation_target_with("http://public.example/", fake_resolver);
        assert!(
            matches!(r, PolicyOutcome::Allow),
            "a clean public resolution must pass, got {r:?}"
        );
    }

    /// RED. "I could not resolve it" is not "allowed" -- the gate has no idea
    /// where the request will land. Paired with the hermetic `.invalid` test
    /// in `tests/dns_resolution_gate_test.rs`, which drives the same
    /// behaviour through the real production entry points.
    #[test]
    fn navigation_gate_refuses_a_name_that_resolves_to_nothing() {
        let p = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
        let r = p.evaluate_navigation_target_with("http://gone.example/", fake_resolver);
        assert!(
            matches!(r, PolicyOutcome::Deny { .. }),
            "an unresolvable host must fail closed, got {r:?}"
        );
    }

    /// NEGATIVE CONTROL. An IP literal already carries its destination; the
    /// gate must not spend a DNS lookup on it. The resolver panics if called.
    #[test]
    fn navigation_gate_does_not_resolve_an_ip_literal() {
        let p = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
        let r = p.evaluate_navigation_target_with("http://93.184.216.34/", never_resolved);
        assert!(matches!(r, PolicyOutcome::Allow), "got {r:?}");
        // ... and a blocked literal is still refused by the string-only half.
        let r = p.evaluate_navigation_target_with("http://169.254.169.254/", never_resolved);
        assert!(matches!(r, PolicyOutcome::Deny { .. }), "got {r:?}");
    }

    /// THE TRAP (gh#911). `check_resolved_host` refuses loopback
    /// unconditionally, so feeding it every resolution kills the loopback
    /// capability: `localhost` resolves to 127.0.0.1 and the grant never gets
    /// a say. The gate must SKIP resolution for a canonical loopback host
    /// holding an authorising grant -- `is_canonical_loopback_host` plus
    /// `self.loopback.authorize(port)`, the two predicates `evaluate()`
    /// already uses. `never_resolved` proves the skip is a real skip.
    ///
    /// `wcore_tools::url_safety::safe_url_pinned_ips` is NOT the shortcut
    /// here: it embeds url_safety's own blocklist, which rejects loopback
    /// unconditionally, and would break gh#911 in exactly this way.
    #[test]
    fn navigation_gate_skips_resolution_for_granted_canonical_loopback() {
        let p = BrowserPolicy::new(PolicyAction::Deny, vec![], vec![]).with_loopback(
            LoopbackCapability {
                enabled: true,
                schema_version: LOOPBACK_CAPABILITY_VERSION,
                session_scope: "local-dev".into(),
                ports: vec![3000],
            },
        );
        let r = p.evaluate_navigation_target_with("http://localhost:3000/", never_resolved);
        assert!(
            matches!(r, PolicyOutcome::Allow),
            "gh#911: a granted loopback port must survive the resolution gate, \
             got {r:?}"
        );
    }

    /// NEGATIVE CONTROL for the trap. The skip is scoped to an AUTHORISING
    /// grant, so an ungranted port stays refused and the exemption cannot be
    /// read as "loopback is exempt".
    #[test]
    fn navigation_gate_still_refuses_loopback_outside_the_grant() {
        let p = BrowserPolicy::new(PolicyAction::Deny, vec![], vec![]).with_loopback(
            LoopbackCapability {
                enabled: true,
                schema_version: LOOPBACK_CAPABILITY_VERSION,
                session_scope: "local-dev".into(),
                ports: vec![3000],
            },
        );
        let r = p.evaluate_navigation_target_with("http://localhost:9999/", fake_resolver);
        assert!(matches!(r, PolicyOutcome::Deny { .. }), "got {r:?}");

        // And with no grant at all, loopback is refused before resolution.
        let bare = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
        let r = bare.evaluate_navigation_target_with("http://localhost:3000/", never_resolved);
        assert!(matches!(r, PolicyOutcome::Deny { .. }), "got {r:?}");
    }

    /// The gate must NOT pin. Two REAL consecutive answer sets for
    /// `s3.amazonaws.com`, captured 45s apart on 2026-08-22 from the upstream
    /// resolver (`dig @185.12.64.1`, local cache bypassed) — they are fully
    /// DISJOINT. A TOFU pin, whether it pins `addrs[0]` or an
    /// order-independent representative such as `min()`, refuses the second
    /// navigation and reports it as a rebinding attack; and because the pin
    /// lives as long as the policy instance, that host stays refused for the
    /// rest of the session.
    ///
    /// Every address in both sets is public and clears the block-list, which
    /// is the check that actually stops a rebind.
    #[test]
    fn navigation_gate_does_not_refuse_a_rotating_multi_a_host() {
        fn s3_first(host: &str) -> Vec<IpAddr> {
            match host {
                "s3.amazonaws.com" => [
                    "16.15.183.181",
                    "16.15.229.87",
                    "16.15.245.37",
                    "16.15.255.55",
                    "16.182.32.224",
                    "52.217.115.248",
                    "52.217.135.16",
                    "52.217.228.192",
                ]
                .iter()
                .map(|s| s.parse().unwrap())
                .collect(),
                _ => Vec::new(),
            }
        }
        fn s3_45s_later(host: &str) -> Vec<IpAddr> {
            match host {
                "s3.amazonaws.com" => [
                    "16.15.191.118",
                    "16.15.207.62",
                    "16.15.245.238",
                    "16.15.253.16",
                    "16.15.255.132",
                    "52.216.32.248",
                    "52.217.203.128",
                    "52.217.69.174",
                ]
                .iter()
                .map(|s| s.parse().unwrap())
                .collect(),
                _ => Vec::new(),
            }
        }

        let p = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
        let r = p.evaluate_navigation_target_with("https://s3.amazonaws.com/b/k", s3_first);
        assert!(matches!(r, PolicyOutcome::Allow), "first navigation: {r:?}");

        let r = p.evaluate_navigation_target_with("https://s3.amazonaws.com/b/k", s3_45s_later);
        assert!(
            matches!(r, PolicyOutcome::Allow),
            "a rotating multi-A public host must not be refused on its second \
             navigation -- the two answer sets are real, measured, and fully \
             disjoint, and neither contains a blocked address: {r:?}"
        );
    }

    /// PAIRED CONTROL for the test above, and the property that actually
    /// matters: rotation is fine, rotating INTO a blocked address is not.
    /// Dropping the pin must not drop this.
    #[test]
    fn navigation_gate_still_refuses_a_later_answer_that_points_inward() {
        let p = BrowserPolicy::new(PolicyAction::Allow, vec![], vec![]);
        let r = p.evaluate_navigation_target_with("http://public.example/", fake_resolver);
        assert!(matches!(r, PolicyOutcome::Allow), "first navigation: {r:?}");

        fn rebound_inward(host: &str) -> Vec<IpAddr> {
            match host {
                "public.example" => vec![
                    "93.184.216.34".parse().unwrap(),
                    "169.254.169.254".parse().unwrap(),
                ],
                _ => Vec::new(),
            }
        }
        let r = p.evaluate_navigation_target_with("http://public.example/", rebound_inward);
        assert!(
            matches!(r, PolicyOutcome::Deny { .. }),
            "a later answer containing the metadata endpoint must be refused \
             by the per-address block-list, pin or no pin: {r:?}"
        );
    }

    /// RED. The origin lists still decide first -- the resolution gate is an
    /// ADDITIONAL refusal, never a new way in. A host that resolves cleanly
    /// but is not on the allow list stays denied.
    #[test]
    fn navigation_gate_does_not_override_the_origin_lists() {
        let p = BrowserPolicy::new(PolicyAction::Deny, vec!["other.example".into()], vec![]);
        let r = p.evaluate_navigation_target_with("http://public.example/", fake_resolver);
        assert!(
            matches!(r, PolicyOutcome::Deny { .. }),
            "a clean resolution must not launder an allow-list miss, got {r:?}"
        );
    }
}
