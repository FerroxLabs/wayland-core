//! Pure egress classification — the decision logic of the B2 egress policy with
//! NO async, NO consent, NO I/O. Given a request and the current allow state it
//! returns an [`EgressVerdict`] that the async policy wrapper resolves (Ask /
//! Exfil go to the consent bridge; Allow / Deny are terminal).
//!
//! Layered model (SPEC Layer 1 + 2):
//! - **Allowlisted** registrable domain (or exact host) → `Allow`.
//! - **Shared-platform** host (gist / raw / S3 / workers.dev / request-bins …)
//!   → never apex-allowlistable; anything but an exact-host allow is `Exfil`
//!   (C2 — allowlisting `amazonaws.com` would open every tenant's bucket).
//! - Body-bearing method (POST/PUT/PATCH) to a non-allowlisted host → `Exfil`.
//! - GET/HEAD with a long / high-entropy path or query to a non-allowlisted
//!   host → `Exfil` (the GET-path/query exfil channel, C2).
//! - Otherwise a new destination → `Ask` (on by default; a future convenience
//!   mode may skip the plain-GET case, but exfil-class never skips).
//!
//! SSRF / metadata / private-range blocking is **Layer 0** and lives in the
//! tools (`is_safe_url` / `blocked_host_reason`); it fires before egress ever
//! reaches the network, so it is intentionally not re-implemented here.

use std::collections::HashSet;

use reqwest::Method;
pub use wcore_egress::EgressOrigin;

/// Suffixes of "shared-platform" hosts where many mutually-untrusted tenants
/// live under one registrable domain. Allowlisting the registrable apex would
/// open every tenant (any user's gist, any S3 bucket, any `*.workers.dev`
/// app), so these are NEVER apex-allowlistable — only an exact full host may be
/// allowed, and any other request to them is exfil-class (SPEC C2). Matched as
/// `host == suffix` or `host.ends_with(".{suffix}")`.
const SHARED_PLATFORM_SUFFIXES: &[&str] = &[
    // code / paste hosting (raw fetch + write-back)
    "raw.githubusercontent.com",
    "gist.github.com",
    "gist.githubusercontent.com",
    "githubusercontent.com",
    "github.io",
    "gitlab.io",
    "pastebin.com",
    "paste.ee",
    "ghostbin.com",
    "hastebin.com",
    "termbin.com",
    // object storage (presigned URLs reach arbitrary buckets)
    "amazonaws.com",
    "blob.core.windows.net",
    "storage.googleapis.com",
    "r2.cloudflarestorage.com",
    "digitaloceanspaces.com",
    // tunnels / preview deploys / serverless (attacker-controllable endpoints)
    "ngrok.io",
    "ngrok-free.app",
    "ngrok.app",
    "trycloudflare.com",
    "workers.dev",
    "vercel.app",
    "netlify.app",
    "pages.dev",
    "onrender.com",
    "herokuapp.com",
    "glitch.me",
    "repl.co",
    "replit.dev",
    // request-bins / OOB-exfil canaries
    "requestbin.com",
    "requestbin.net",
    "pipedream.net",
    "webhook.site",
    "beeceptor.com",
    "burpcollaborator.net",
    "oast.fun",
    "oast.live",
    "oast.pro",
    "oast.site",
    "oastify.com",
    "interact.sh",
    "canarytokens.com",
];

/// Path/query length (chars) above which a GET is treated as carrying data.
const GET_DATA_LEN_THRESHOLD: usize = 96;

/// Minimum length of a single high-entropy token (base64/hex-ish run) that, on
/// its own, marks a GET as data-bearing even under the length threshold.
const HIGH_ENTROPY_TOKEN_LEN: usize = 24;

/// A verdict from the pure classifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EgressVerdict {
    /// Destination is allowlisted — allow silently.
    Allow,
    /// New, non-exfil-shaped destination — ask the operator (ask-with-memory).
    Ask {
        /// The exact request host (what gets exact-allowed for shared platforms).
        host: String,
        /// The registrable domain (what "always" persists for ordinary hosts).
        registrable: String,
        /// Short human reason for the prompt.
        reason: String,
    },
    /// wayland#1264 — a TOOL-originated, DATA-BEARING request to a host that
    /// IS on the allowlist.
    ///
    /// Distinct from [`Self::Ask`] in exactly one way, and it is the way that
    /// matters: an unattended session (no consent doorbell) resolves `Ask` to
    /// ALLOW, because nothing sensitive leaves on a data-less read to a new
    /// host. That reasoning does not transfer here — the payload is the point
    /// — so this variant resolves to DENY when there is nobody to ask.
    ///
    /// Not [`Self::Exfil`] either: the host is allowlisted, so with an
    /// approval surface present the operator gets a prompt rather than a hard
    /// refusal, and the tool keeps working in an interactive session.
    ToolData {
        /// The exact request host.
        host: String,
        /// The registrable domain, for the prompt.
        registrable: String,
        /// Short human reason.
        reason: String,
    },
    /// Exfil-class — must be gated even in YOLO. The operator still decides, but
    /// this never silently auto-allows and never persists an apex allow.
    Exfil {
        /// The exact request host.
        host: String,
        /// Whether this host is a shared-platform host (per-URL approval only —
        /// "always" must persist the full host, never the registrable apex).
        shared_platform: bool,
        /// Short human reason.
        reason: String,
    },
}

/// The set of destinations the operator has allowed. Two tiers: registrable
/// domains (cover their subdomains) and exact hosts (for shared-platform hosts
/// that must never be apex-allowed).
#[derive(Debug, Clone, Default)]
pub struct AllowList {
    /// Registrable domains (e.g. `github.com` covers `api.github.com`). Never
    /// contains a shared-platform apex.
    domains: HashSet<String>,
    /// Exact full hosts (e.g. `myapp.workers.dev`).
    hosts: HashSet<String>,
}

impl AllowList {
    /// Allow a registrable domain wholesale. A shared-platform apex is rejected
    /// (downgraded to a no-op) so it can never be apex-allowed by mistake.
    pub fn allow_domain(&mut self, domain: &str) {
        let d = domain.trim().to_ascii_lowercase();
        if d.is_empty() || is_shared_platform(&d) {
            return;
        }
        self.domains.insert(d);
    }

    /// Allow an exact full host (the only allow form for shared-platform hosts).
    pub fn allow_host(&mut self, host: &str) {
        let h = host.trim().to_ascii_lowercase();
        if !h.is_empty() {
            self.hosts.insert(h);
        }
    }

    fn host_allowed(&self, host: &str) -> bool {
        self.hosts.contains(host)
    }

    fn domain_allowed(&self, registrable: &str) -> bool {
        self.domains.contains(registrable)
    }

    /// Number of allowed entries (for diagnostics).
    pub fn len(&self) -> usize {
        self.domains.len() + self.hosts.len()
    }

    /// True if nothing is allowed.
    pub fn is_empty(&self) -> bool {
        self.domains.is_empty() && self.hosts.is_empty()
    }
}

/// The registrable domain (eTLD+1) of `host` via the Public Suffix List, lower-
/// cased. Returns `None` for an IP literal or an unlistable name (caller falls
/// back to the full host).
pub fn registrable_domain(host: &str) -> Option<String> {
    psl::domain_str(host).map(|d| d.to_ascii_lowercase())
}

/// True if `host` is a local (non-exfil) destination: `localhost`, or an IP in
/// a loopback / private / link-local / CGNAT / IPv6-ULA / unspecified range.
pub fn is_local_destination(host: &str) -> bool {
    use std::net::{Ipv4Addr, Ipv6Addr};

    let h = host.trim_matches(['[', ']']); // strip IPv6 brackets if present
    if h.eq_ignore_ascii_case("localhost") || h.eq_ignore_ascii_case("localhost.localdomain") {
        return true;
    }
    if let Ok(v4) = h.parse::<Ipv4Addr>() {
        let o = v4.octets();
        return v4.is_loopback()
            || v4.is_private()
            || v4.is_link_local()
            || v4.is_unspecified()
            || o[0] == 100 && (64..=127).contains(&o[1]) // 100.64.0.0/10 CGNAT
            || o[0] == 0; // 0.0.0.0/8
    }
    if let Ok(v6) = h.parse::<Ipv6Addr>() {
        let seg = v6.segments();
        return v6.is_loopback()
            || v6.is_unspecified()
            || (seg[0] & 0xfe00) == 0xfc00 // fc00::/7 ULA
            || (seg[0] & 0xffc0) == 0xfe80; // fe80::/10 link-local
    }
    false
}

/// True if `host` is (or is a subdomain of) a shared-platform suffix.
pub fn is_shared_platform(host: &str) -> bool {
    let h = host.to_ascii_lowercase();
    SHARED_PLATFORM_SUFFIXES
        .iter()
        .any(|s| h == *s || h.ends_with(&format!(".{s}")))
}

/// Classify an outbound request against the current allow state and the
/// request's [`EgressOrigin`].
///
/// ## wayland#1264 — why the origin is a parameter
///
/// This function receives method + URL and nothing else (headers and body are
/// never read), and it returned `Allow` on the host match ALONE. Because
/// `get_carries_data` had exactly one call site, on the non-allowlisted
/// branch, the shape check was unreachable for any host on the 38-entry
/// shipped default: an unattended `WebFetch` of
/// `https://<allowlisted-apex>/?leak=<secret>` was admitted with no approval
/// in any mode.
///
/// Narrowing the allowlist was not available — it would deny the agent's own
/// LLM POSTs, and two tests pin that as intended. The grant is therefore split
/// by ORIGIN, not by host: provider traffic to an allowlisted apex keeps its
/// unconditional `Allow`, and tool-driven traffic to the same apex is
/// shape-checked. Nothing about provider semantics changes.
///
/// Two other shapes were considered and refuted; see
/// `wcore_egress::origin` for both, and `.planning/DECISIONS.md` for the
/// recorded decision.
pub fn classify(
    method: &Method,
    url: &url::Url,
    allow: &AllowList,
    origin: EgressOrigin,
) -> EgressVerdict {
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();

    // Local destinations are not exfiltration — loopback, RFC1918, link-local,
    // CGNAT, IPv6 ULA, and `localhost`. The egress gate (Layer 1) is about data
    // leaving the machine to an external host; reaching the local mock/dev
    // server or a sidecar is allowed here. (SSRF/metadata blocking for the TOOL
    // surfaces is the separate always-on Layer 0 floor — `is_safe_url` /
    // `blocked_host_reason` — which is unaffected by this allow.)
    if is_local_destination(&host) {
        return EgressVerdict::Allow;
    }

    let shared = is_shared_platform(&host);
    let registrable = registrable_domain(&host).unwrap_or_else(|| host.clone());

    // Allowlist check. Shared-platform hosts match ONLY by exact host (never the
    // registrable apex); ordinary hosts match by registrable domain or exact host.
    let allowed = if shared {
        allow.host_allowed(&host)
    } else {
        allow.domain_allowed(&registrable) || allow.host_allowed(&host)
    };
    if allowed {
        // wayland#1264. The allowlist is the operator saying "this agent may
        // reach GitHub". It is not the operator saying "the model may choose
        // a query string against GitHub", and those are different grants.
        if origin == EgressOrigin::Tool && request_carries_data(method, url) {
            return EgressVerdict::ToolData {
                host,
                registrable,
                reason: format!(
                    "{method} to an allowlisted host, carrying data the model \
                     chose (a body, or a long/high-entropy path or query)"
                ),
            };
        }
        return EgressVerdict::Allow;
    }

    // Shared-platform, not exact-allowed → exfil-class (per-URL only).
    if shared {
        return EgressVerdict::Exfil {
            host,
            shared_platform: true,
            reason: "shared-platform host — per-URL approval only (no apex allowlist)".into(),
        };
    }

    // Body-bearing method to a non-allowlisted host → exfil-class.
    if matches!(*method, Method::POST | Method::PUT | Method::PATCH) {
        return EgressVerdict::Exfil {
            host,
            shared_platform: false,
            reason: format!("{method} with body to a non-allowlisted host"),
        };
    }

    // GET/HEAD carrying data (long or high-entropy path/query) → exfil-class.
    if get_carries_data(url) {
        return EgressVerdict::Exfil {
            host,
            shared_platform: false,
            reason: "GET with a long or high-entropy path/query to a non-allowlisted host".into(),
        };
    }

    // Otherwise a plain new destination → ask.
    EgressVerdict::Ask {
        host,
        registrable,
        reason: "first request to a new destination".into(),
    }
}

/// Does this request carry data OUT, by either of the two shapes the exfil
/// boundary already recognises — a body-bearing method, or a GET/HEAD whose
/// path/query is long or high-entropy?
///
/// wayland#1264: this is the predicate the non-allowlisted branch below has
/// always applied in two separate steps. It is factored out so the allowlisted
/// tool-origin branch asks the SAME question rather than a second, drifting
/// copy of it — and so `get_carries_data` stops having exactly one call site.
fn request_carries_data(method: &Method, url: &url::Url) -> bool {
    matches!(*method, Method::POST | Method::PUT | Method::PATCH) || get_carries_data(url)
}

/// Heuristic: does this GET/HEAD URL carry data in its path or query? True when
/// the combined path+query is long, or when it contains a high-entropy token
/// (a base64/hex-ish run) that looks like encoded/secret data.
fn get_carries_data(url: &url::Url) -> bool {
    let path = url.path();
    let query = url.query().unwrap_or("");
    if path.len() + query.len() > GET_DATA_LEN_THRESHOLD {
        return true;
    }
    // A long unbroken base64/hex-ish token anywhere in path or query.
    let combined = format!("{path}?{query}");
    longest_token_run(&combined) >= HIGH_ENTROPY_TOKEN_LEN
}

/// Longest run of base64/hex URL-safe token characters ([A-Za-z0-9_-]) — a
/// proxy for an encoded blob. Separators (`/ . ? & = % :`) break the run.
fn longest_token_run(s: &str) -> usize {
    let mut best = 0usize;
    let mut cur = 0usize;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
            cur += 1;
            best = best.max(cur);
        } else {
            cur = 0;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> url::Url {
        url::Url::parse(s).unwrap()
    }

    /// Provider-origin classification — the three-argument shape every test
    /// below predates wayland#1264 with, so those tests keep asserting exactly
    /// what they always asserted. Tool-origin behaviour has its own tests at
    /// the end of this module; the production caller
    /// (`AgentEgressPolicy::check`) passes the origin it read off the request
    /// and never this shim.
    fn classify(method: &Method, url: &url::Url, allow: &AllowList) -> EgressVerdict {
        super::classify(method, url, allow, EgressOrigin::Provider)
    }

    #[test]
    fn registrable_domain_uses_the_public_suffix_list() {
        assert_eq!(
            registrable_domain("api.github.com").as_deref(),
            Some("github.com")
        );
        // Multi-part eTLD: evil.co.uk is registrable, NOT co.uk — the bug a
        // naive last-two-labels heuristic would introduce.
        assert_eq!(
            registrable_domain("evil.co.uk").as_deref(),
            Some("evil.co.uk")
        );
        assert_eq!(
            registrable_domain("a.b.evil.co.uk").as_deref(),
            Some("evil.co.uk")
        );
    }

    #[test]
    fn allowlisted_registrable_domain_covers_subdomains() {
        let mut allow = AllowList::default();
        allow.allow_domain("github.com");
        assert_eq!(
            classify(&Method::GET, &url("https://api.github.com/repos"), &allow),
            EgressVerdict::Allow
        );
    }

    #[test]
    fn new_plain_get_asks() {
        let allow = AllowList::default();
        match classify(&Method::GET, &url("https://react.dev/learn"), &allow) {
            EgressVerdict::Ask { registrable, .. } => assert_eq!(registrable, "react.dev"),
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    #[test]
    fn post_to_non_allowlisted_host_is_exfil() {
        let allow = AllowList::default();
        match classify(&Method::POST, &url("https://example.org/collect"), &allow) {
            EgressVerdict::Exfil {
                shared_platform, ..
            } => assert!(!shared_platform),
            other => panic!("expected Exfil, got {other:?}"),
        }
    }

    /// PINNED BEHAVIOUR, and wayland#1264 c3's wrong-refusal control.
    ///
    /// This test and its sibling `policy::tests::allowlisted_post_is_allowed`
    /// are why narrowing the allowlist was never an available fix for #1264:
    /// they pin the agent's own LLM POST to an allowlisted apex as an
    /// unconditional `Allow`, and a change that broke them would deny every
    /// provider call. The recorded decision (`.planning/DECISIONS.md`,
    /// "Egress: split the allowlist grant by traffic origin") splits the grant
    /// by ORIGIN instead, which is why this call -- through the
    /// provider-origin shim -- still passes unchanged.
    #[test]
    fn post_to_allowlisted_host_is_allowed() {
        let mut allow = AllowList::default();
        allow.allow_domain("api.anthropic.com");
        // registrable of api.anthropic.com is anthropic.com — allow the apex.
        allow.allow_domain("anthropic.com");
        assert_eq!(
            classify(
                &Method::POST,
                &url("https://api.anthropic.com/v1/messages"),
                &allow
            ),
            EgressVerdict::Allow
        );
    }

    #[test]
    fn shared_platform_host_is_never_apex_allowlistable() {
        let mut allow = AllowList::default();
        // Trying to allow the apex must be a no-op for shared platforms.
        allow.allow_domain("amazonaws.com");
        assert!(allow.is_empty(), "shared-platform apex must not be stored");
        let v = classify(
            &Method::GET,
            &url("https://victim-bucket.s3.amazonaws.com/secret"),
            &allow,
        );
        match v {
            EgressVerdict::Exfil {
                shared_platform, ..
            } => assert!(shared_platform),
            other => panic!("expected shared-platform Exfil, got {other:?}"),
        }
    }

    #[test]
    fn shared_platform_exact_host_allow_works() {
        let mut allow = AllowList::default();
        allow.allow_host("myapp.workers.dev");
        assert_eq!(
            classify(&Method::GET, &url("https://myapp.workers.dev/api"), &allow),
            EgressVerdict::Allow
        );
        // A DIFFERENT tenant under the same suffix is still exfil-class.
        match classify(&Method::GET, &url("https://evil.workers.dev/grab"), &allow) {
            EgressVerdict::Exfil { .. } => {}
            other => panic!("expected Exfil for other tenant, got {other:?}"),
        }
    }

    #[test]
    fn get_with_long_query_is_exfil() {
        let allow = AllowList::default();
        let long = "x".repeat(120);
        let u = format!("https://example.org/p?d={long}");
        match classify(&Method::GET, &url(&u), &allow) {
            EgressVerdict::Exfil { .. } => {}
            other => panic!("expected Exfil for long query, got {other:?}"),
        }
    }

    #[test]
    fn get_with_high_entropy_token_is_exfil() {
        let allow = AllowList::default();
        // A 32-char base64-ish blob in the path, under the length threshold.
        let u = "https://example.org/aGVsbG8gd29ybGQgc2VjcmV0Cg";
        match classify(&Method::GET, &url(u), &allow) {
            EgressVerdict::Exfil { .. } => {}
            other => panic!("expected Exfil for high-entropy token, got {other:?}"),
        }
    }

    #[test]
    fn local_destinations_are_allowed() {
        let allow = AllowList::default();
        // A POST to the local mock server (the shape every mock-LLM test uses)
        // must NOT be gated — local is not exfil.
        for u in [
            "http://127.0.0.1:8080/v1/messages",
            "http://localhost:9377/cdp",
            "http://192.168.1.10/x",
            "http://10.0.0.5/y",
            "http://169.254.169.254/latest", // link-local: allowed HERE (Layer 0 SSRF blocks it for tools)
            "http://[::1]:3000/z",
        ] {
            assert_eq!(
                classify(&Method::POST, &url(u), &allow),
                EgressVerdict::Allow,
                "local destination must be allowed: {u}"
            );
        }
        // A public host is still classified normally.
        assert!(matches!(
            classify(&Method::POST, &url("https://evil.test/c"), &allow),
            EgressVerdict::Exfil { .. }
        ));
    }

    #[test]
    fn short_plain_get_is_only_ask_not_exfil() {
        let allow = AllowList::default();
        match classify(&Method::GET, &url("https://example.org/docs"), &allow) {
            EgressVerdict::Ask { .. } => {}
            other => panic!("expected Ask, got {other:?}"),
        }
    }

    // =======================================================================
    // wayland#1264 — the allowlist grant is split by traffic ORIGIN.
    // =======================================================================

    /// THE DEFECT. A tool-driven, data-bearing request to an allowlisted apex
    /// was admitted on the host match alone: `classify` returned `Allow`
    /// BEFORE the method check and before the path/query check, and
    /// `get_carries_data` had its only call site on the non-allowlisted
    /// branch, so for any of the 38 shipped default domains the shape check
    /// was unreachable.
    #[test]
    fn tool_driven_data_to_an_allowlisted_apex_is_not_allowed_on_the_host_match() {
        let mut allow = AllowList::default();
        allow.allow_domain("github.com");

        // A query payload the model chose. The token run is base64url, which
        // is why `-` must stay IN the alphabet: excluding it would blind the
        // check to the shape it exists to see.
        let leak = url("https://github.com/?leak=aG93LW11Y2gtc2VjcmV0LWRhdGEtZml0cy1oZXJl");
        assert!(
            matches!(
                super::classify(&Method::GET, &leak, &allow, EgressOrigin::Tool),
                EgressVerdict::ToolData { .. }
            ),
            "a tool-chosen query payload to an allowlisted apex must be gated"
        );

        // Every data-bearing shape, not just the one in the ticket.
        for (method, raw) in [
            (Method::POST, "https://github.com/collect"),
            (Method::PUT, "https://github.com/collect"),
            (Method::PATCH, "https://github.com/collect"),
            (
                Method::GET,
                "https://api.github.com/?q=c2VjcmV0LXZhbHVlLWJhc2U2NHVybC1lbmNvZGVk",
            ),
            (
                Method::HEAD,
                "https://github.com/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        ] {
            assert!(
                matches!(
                    super::classify(&method, &url(raw), &allow, EgressOrigin::Tool),
                    EgressVerdict::ToolData { .. }
                ),
                "tool-origin {method} {raw} must be gated"
            );
        }
    }

    /// THE WRONG-REFUSAL CONTROL, and the one the ticket's c3 names: provider
    /// traffic to the SAME apex keeps its unconditional `Allow`. A fix that
    /// applied the new check to provider traffic would deny the agent's own
    /// LLM POSTs — which is precisely why narrowing the allowlist was not an
    /// available fix.
    #[test]
    fn provider_traffic_to_the_same_apex_is_still_unconditionally_allowed() {
        let mut allow = AllowList::default();
        allow.allow_domain("anthropic.com");
        allow.allow_domain("github.com");

        for (method, raw) in [
            // The agent's own streaming call: a POST with a large body.
            (Method::POST, "https://api.anthropic.com/v1/messages"),
            // A provider GET with a long, high-entropy path — a model id, a
            // signed URL, a resource token. Data-bearing BY SHAPE, and still
            // allowed, because the origin is not the model.
            (
                Method::GET,
                "https://api.anthropic.com/v1/models/claude-opus-4-1-20250805-v1-0-thinking",
            ),
            (
                Method::POST,
                "https://github.com/?leak=aG93LW11Y2gtc2VjcmV0LWRhdGEtZml0cy1oZXJl",
            ),
        ] {
            assert_eq!(
                super::classify(&method, &url(raw), &allow, EgressOrigin::Provider),
                EgressVerdict::Allow,
                "provider-origin {method} {raw} must keep its unconditional Allow"
            );
        }
    }

    /// The split narrows nothing else. A tool request that carries NO data to
    /// an allowlisted host is still allowed silently — WebFetch of a docs page
    /// on an allowlisted domain must not start prompting.
    #[test]
    fn a_data_less_tool_read_of_an_allowlisted_host_is_still_allowed() {
        let mut allow = AllowList::default();
        allow.allow_domain("github.com");
        for raw in [
            "https://github.com/",
            "https://github.com/rust-lang/rust",
            "https://api.github.com/repos/o/r",
        ] {
            assert_eq!(
                super::classify(&Method::GET, &url(raw), &allow, EgressOrigin::Tool),
                EgressVerdict::Allow,
                "a data-less tool read of an allowlisted host must stay silent: {raw}"
            );
        }
        // ...and a tool read of a LOCAL destination is untouched by any of
        // this: the local short-circuit runs before the allowlist.
        assert_eq!(
            super::classify(
                &Method::POST,
                &url("http://127.0.0.1:8080/collect"),
                &allow,
                EgressOrigin::Tool
            ),
            EgressVerdict::Allow
        );
    }

    /// A NON-allowlisted host is unaffected in both directions: it was already
    /// `Exfil` for a data-bearing request and `Ask` for a data-less one, and
    /// the origin split must not have reclassified either.
    #[test]
    fn the_non_allowlisted_branch_is_unchanged_by_the_origin_split() {
        let allow = AllowList::default();
        for origin in [EgressOrigin::Provider, EgressOrigin::Tool] {
            assert!(matches!(
                super::classify(&Method::POST, &url("https://evil.test/c"), &allow, origin),
                EgressVerdict::Exfil { .. }
            ));
            assert!(matches!(
                super::classify(&Method::GET, &url("https://evil.test/docs"), &allow, origin),
                EgressVerdict::Ask { .. }
            ));
        }
    }
}
