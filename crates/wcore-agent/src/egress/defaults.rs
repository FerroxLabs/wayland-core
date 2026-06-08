//! B2.4 — build the egress allowlist from config + well-known first-party hosts.
//!
//! The allowlist must cover everything the agent legitimately POSTs to or
//! fetches data from, or the gate would break normal operation. Most important:
//! the **active provider host** (derived from `config.base_url`) — without it
//! the agent's own LLM calls would be denied. Plain data-less reads to any other
//! host are allowed by posture (the `Ask` verdict), so the allowlist only needs
//! to cover exfil-shaped first-party traffic (provider/tool-backend APIs).

use wcore_config::config::Config;

use super::classify::{AllowList, is_shared_platform};

/// Well-known first-party registrable domains the agent reaches with bodies or
/// data (provider APIs, built-in tool backends, package registries). Allowing a
/// registrable domain covers its subdomains (`anthropic.com` ⇒ `api.anthropic.com`).
/// Shared-platform apexes are intentionally absent (they're added per-exact-host
/// from config when needed).
const WELL_KNOWN_DOMAINS: &[&str] = &[
    // LLM providers
    "anthropic.com",
    "openai.com",
    "x.ai",
    "mistral.ai",
    "cohere.com",
    "cohere.ai",
    "groq.com",
    "perplexity.ai",
    "deepseek.com",
    "together.xyz",
    "together.ai",
    "openrouter.ai",
    "fireworks.ai",
    "voyageai.com",
    "moonshot.cn",
    "moonshot.ai",
    // built-in tool backends (web search / code hosts / docs APIs)
    "tavily.com",
    "brave.com",
    "duckduckgo.com",
    "github.com",
    "gitlab.com",
    "notion.com",
    "notion.so",
    "linear.app",
    // package registries (rare POST, common metadata fetch with long paths)
    "crates.io",
    "pypi.org",
    "pythonhosted.org",
    "npmjs.org",
    "npmjs.com",
    "rubygems.org",
];

/// Build the egress allowlist for the given resolved config:
/// 1. the well-known first-party registrable domains,
/// 2. the active provider host from `config.base_url` (exact host AND its
///    registrable domain — covers Bedrock/Vertex regional hosts that live under
///    shared-platform suffixes and so must be exact-allowed),
/// 3. the operator's `[security] egress_allow` entries (registrable domain, or
///    exact host for shared-platform entries).
pub fn build_allowlist(config: &Config) -> AllowList {
    let mut allow = AllowList::default();

    for d in WELL_KNOWN_DOMAINS {
        allow.allow_domain(d);
    }

    // The active provider endpoint MUST be reachable or the agent can't talk to
    // its own model. Derive it from the resolved base_url.
    if let Some(host) = host_of(&config.base_url) {
        // Exact host always (covers shared-platform-suffixed provider hosts like
        // bedrock-runtime.us-east-1.amazonaws.com or *-aiplatform.googleapis.com).
        allow.allow_host(&host);
        // And the registrable domain for the ordinary (non-shared) case so
        // sibling subdomains (e.g. a token endpoint) are covered too.
        if !is_shared_platform(&host)
            && let Some(reg) = super::classify::registrable_domain(&host)
        {
            allow.allow_domain(&reg);
        }
    }

    // Operator additions.
    for entry in &config.security.egress_allow {
        let e = entry.trim();
        if e.is_empty() {
            continue;
        }
        if is_shared_platform(e) {
            allow.allow_host(e);
        } else {
            allow.allow_domain(e);
        }
    }

    allow
}

/// Extract the lowercased host from a base URL string.
fn host_of(base_url: &str) -> Option<String> {
    url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_ascii_lowercase()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::egress::classify::{EgressVerdict, classify};
    use reqwest::Method;

    fn cfg(base_url: &str, allow: &[&str]) -> Config {
        let mut c = Config {
            base_url: base_url.to_string(),
            ..Config::default()
        };
        c.security.egress_allow = allow.iter().map(|s| s.to_string()).collect();
        c
    }

    fn u(s: &str) -> url::Url {
        s.parse().unwrap()
    }

    #[test]
    fn active_provider_host_is_allowlisted() {
        let allow = build_allowlist(&cfg("https://api.anthropic.com", &[]));
        // The agent's own POST to its provider must be allowed.
        assert_eq!(
            classify(
                &Method::POST,
                &u("https://api.anthropic.com/v1/messages"),
                &allow
            ),
            EgressVerdict::Allow
        );
    }

    #[test]
    fn shared_platform_provider_host_is_exact_allowed() {
        // Bedrock regional host lives under amazonaws.com (shared-platform) — it
        // must be exact-allowed, NOT apex-allowed.
        let allow = build_allowlist(&cfg("https://bedrock-runtime.us-east-1.amazonaws.com", &[]));
        assert_eq!(
            classify(
                &Method::POST,
                &u("https://bedrock-runtime.us-east-1.amazonaws.com/model/x/invoke"),
                &allow
            ),
            EgressVerdict::Allow
        );
        // A DIFFERENT bucket under amazonaws.com is still exfil-class.
        assert!(matches!(
            classify(
                &Method::GET,
                &u("https://victim.s3.amazonaws.com/o"),
                &allow
            ),
            EgressVerdict::Exfil { .. }
        ));
    }

    #[test]
    fn well_known_tool_backends_are_allowed() {
        let allow = build_allowlist(&cfg("https://api.openai.com", &[]));
        assert_eq!(
            classify(&Method::POST, &u("https://api.tavily.com/search"), &allow),
            EgressVerdict::Allow
        );
        assert_eq!(
            classify(&Method::GET, &u("https://api.github.com/repos/x/y"), &allow),
            EgressVerdict::Allow
        );
    }

    #[test]
    fn operator_additions_apply() {
        let allow = build_allowlist(&cfg(
            "https://api.openai.com",
            &["example.com", "myapp.workers.dev"],
        ));
        assert_eq!(
            classify(&Method::POST, &u("https://api.example.com/x"), &allow),
            EgressVerdict::Allow
        );
        // shared-platform entry was added as an exact host.
        assert_eq!(
            classify(&Method::GET, &u("https://myapp.workers.dev/api"), &allow),
            EgressVerdict::Allow
        );
    }

    #[test]
    fn non_allowlisted_exfil_still_blocked() {
        let allow = build_allowlist(&cfg("https://api.anthropic.com", &[]));
        assert!(matches!(
            classify(&Method::POST, &u("https://evil.test/collect"), &allow),
            EgressVerdict::Exfil { .. }
        ));
    }
}
