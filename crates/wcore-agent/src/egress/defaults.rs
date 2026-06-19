//! B2.4 — build the egress allowlist from config + well-known first-party hosts.
//!
//! The allowlist must cover everything the agent legitimately POSTs to or
//! fetches data from, or the gate would break normal operation. Most important:
//! the **active provider host** (derived from `config.base_url`) — without it
//! the agent's own LLM calls would be denied. Plain data-less reads to any other
//! host are allowed by posture (the `Ask` verdict), so the allowlist only needs
//! to cover exfil-shaped first-party traffic (provider/tool-backend APIs).

use wcore_config::config::{Config, ProviderType};

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
    "nvidia.com",  // NVIDIA NIM: integrate.api.nvidia.com
    "cerebras.ai", // Cerebras: api.cerebras.ai
    // First-party Wayland router + shipped providers whose default base URL is
    // internal to the provider crate, so `config.base_url` is empty and the host
    // is NOT auto-derived below. Without these, configuring the provider blocks
    // its very first request under the default egress policy.
    "fluxrouter.ai",
    "minimax.io",
    "minimaxi.com", // MiniMax's second region + 401 region-failover target
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
/// 3. the operator's `[security] egress_allow` entries (each allowed as the
///    exact host AND, for non-shared-platform entries, its registrable domain).
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

    // C1 — "Sign in with ChatGPT" routes inference through the Codex backend at
    // `chatgpt.com/backend-api/codex/responses`, which is NOT in WELL_KNOWN_DOMAINS.
    // Without this, every POST to the Codex backend is classified Exfil→Deny and
    // the provider is dead-on-arrival under `[security] enabled = true`. Add it
    // explicitly off the provider TYPE (not just the base_url host) so a user who
    // overrides base_url, or an empty base_url, still has the Codex host allowed.
    // `chatgpt.com` is not a shared-platform suffix, so apex-allowing it covers
    // the `chatgpt.com` host and its subdomains (incl. the OAuth token endpoint
    // is on `auth.openai.com`, covered separately by the openai.com well-known).
    if config.provider == ProviderType::OpenAIChatGpt {
        allow.allow_domain("chatgpt.com");
    }

    // Qwen's default base_url is empty (the provider crate supplies the internal
    // DashScope host), so it is not auto-derived above. Its host lives on Alibaba
    // Cloud (`*.aliyuncs.com`), so allow the EXACT DashScope endpoints off the
    // provider type rather than apex-allowing all of aliyuncs.com. Covers both
    // the international default and the mainland-China host.
    if config.provider == ProviderType::Qwen {
        allow.allow_host("dashscope-intl.aliyuncs.com");
        allow.allow_host("dashscope.aliyuncs.com");
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
            // Allow the EXACT host the operator typed (so a full host like
            // `api.example.com` works as users naturally expect, not only the
            // apex), AND its registrable domain (so the apex form keeps covering
            // sibling subdomains). Previously only `allow_domain(e)` ran, so a
            // full-host entry never matched and users had to know to drop down
            // to the apex.
            allow.allow_host(e);
            if let Some(reg) = super::classify::registrable_domain(e) {
                allow.allow_domain(&reg);
            }
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
    fn flux_router_host_is_allowlisted_with_empty_base_url() {
        // flux-router (and other providers whose default base URL is internal to
        // the provider crate) resolves to an EMPTY config.base_url, so the host
        // is not auto-derived. It must be covered by the well-known list or its
        // first request is blocked out of the box. Regression for the live-E2E
        // egress finding.
        let allow = build_allowlist(&cfg("", &[]));
        assert_eq!(
            classify(
                &Method::POST,
                &u("https://api.fluxrouter.ai/v1/chat/completions"),
                &allow
            ),
            EgressVerdict::Allow,
            "flux-router must be reachable out of the box (empty base_url)"
        );
    }

    #[test]
    fn native_providers_with_internal_default_base_url_are_reachable() {
        // nvidia / cerebras / qwen / minimax all resolve to an EMPTY
        // config.base_url (the provider crate supplies the host), so they must be
        // covered explicitly or their first request is blocked out of the box.
        // Regression for the NVIDIA NIM egress finding (2026-06-19).
        let allow = build_allowlist(&cfg("", &[]));
        for url in [
            "https://integrate.api.nvidia.com/v1/chat/completions",
            "https://api.cerebras.ai/v1/chat/completions",
        ] {
            assert_eq!(
                classify(&Method::POST, &u(url), &allow),
                EgressVerdict::Allow,
                "{url} must be reachable out of the box (empty base_url)"
            );
        }
    }

    #[test]
    fn minimax_region_failover_host_is_allowlisted() {
        // MiniMax's 401 region-failover retries `api.minimaxi.com` (a different
        // registrable domain from the default `api.minimax.io`), pinned at runtime
        // — not via config.base_url — so it must be in the well-known list or the
        // failover is dead under the egress policy.
        let allow = build_allowlist(&cfg("", &[]));
        assert_eq!(
            classify(
                &Method::POST,
                &u("https://api.minimaxi.com/anthropic/v1/messages"),
                &allow
            ),
            EgressVerdict::Allow,
            "MiniMax failover host api.minimaxi.com must be reachable"
        );
    }

    #[test]
    fn qwen_dashscope_host_is_allowlisted_for_qwen_provider() {
        // Qwen lives on shared `*.aliyuncs.com`, so its host is allowed off the
        // provider TYPE (exact host), not by apex-allowing all of Alibaba Cloud.
        let mut c = cfg("", &[]);
        c.provider = ProviderType::Qwen;
        let allow = build_allowlist(&c);
        assert_eq!(
            classify(
                &Method::POST,
                &u("https://dashscope-intl.aliyuncs.com/compatible-mode/v1/chat/completions"),
                &allow
            ),
            EgressVerdict::Allow,
            "Qwen DashScope host must be reachable when provider == Qwen"
        );
        // And it must NOT broadly allow other aliyuncs.com hosts (no apex over-allow).
        let allow_other = build_allowlist(&cfg("", &[]));
        assert_ne!(
            classify(
                &Method::POST,
                &u("https://oss-cn-hangzhou.aliyuncs.com/bucket/key"),
                &allow_other
            ),
            EgressVerdict::Allow,
            "Qwen allow must not apex-allow all of aliyuncs.com"
        );
    }

    #[test]
    fn operator_egress_allow_accepts_a_full_host() {
        // A full host the operator types (`api.example.com`) must work, not only
        // the apex `example.com`. Previously only the registrable domain matched.
        let allow = build_allowlist(&cfg("https://api.anthropic.com", &["api.custom.example"]));
        assert_eq!(
            classify(&Method::POST, &u("https://api.custom.example/v1/x"), &allow),
            EgressVerdict::Allow,
            "an exact-host egress_allow entry must match that host"
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
    fn chatgpt_provider_allows_codex_backend() {
        // C1: with the chatgpt provider active, a POST to the Codex backend must
        // be allowed even though chatgpt.com is not a well-known domain.
        let mut c = Config {
            provider: ProviderType::OpenAIChatGpt,
            base_url: "https://chatgpt.com/backend-api/codex".to_string(),
            ..Config::default()
        };
        c.security.egress_allow = vec![];
        let allow = build_allowlist(&c);
        assert_eq!(
            classify(
                &Method::POST,
                &u("https://chatgpt.com/backend-api/codex/responses"),
                &allow
            ),
            EgressVerdict::Allow
        );
    }

    #[test]
    fn chatgpt_codex_allowed_even_when_base_url_overridden() {
        // The allow is keyed off the provider TYPE, so a user who overrides
        // base_url (or leaves it empty) still reaches the Codex backend.
        let allow = build_allowlist(&Config {
            provider: ProviderType::OpenAIChatGpt,
            base_url: String::new(),
            ..Config::default()
        });
        assert_eq!(
            classify(
                &Method::POST,
                &u("https://chatgpt.com/backend-api/codex/responses"),
                &allow
            ),
            EgressVerdict::Allow
        );
    }

    #[test]
    fn non_chatgpt_provider_does_not_allow_chatgpt_host() {
        // The Codex allow is provider-scoped: a different active provider must
        // NOT silently open chatgpt.com.
        let allow = build_allowlist(&cfg("https://api.openai.com", &[]));
        assert!(matches!(
            classify(
                &Method::POST,
                &u("https://chatgpt.com/backend-api/codex/responses"),
                &allow
            ),
            EgressVerdict::Exfil { .. }
        ));
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
