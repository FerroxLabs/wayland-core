//! v0.9.0 Wave-4 E2 — provider health probes for the `/doctor`
//! diagnostics surface.
//!
//! For each of the four canonical LLM providers (Anthropic, OpenAI,
//! Gemini, Groq) we issue a short-timeout request against the provider's
//! `/v1/models` (or equivalent) endpoint and classify the result into
//! three states for the TUI:
//!
//! - **Green** — request returned `200 OK`. The key reaches the API.
//! - **Yellow** — no API key set in the environment. Nothing to probe.
//! - **Red** — request failed: a 4xx/5xx status, a TCP timeout, a DNS
//!   error, etc.
//!
//! Each check enforces a hard 5-second wall-clock cap via reqwest's
//! request-level `.timeout(...)` so a wedged provider can't stall the
//! `/doctor` surface. The probes run concurrently — `futures::join!` —
//! so the worst-case wait for the full check set is one timeout, not
//! four.
//!
//! Used by the TUI diagnostics surface (`/doctor`) and by future
//! programmatic health probes.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Wall-clock cap on every provider health probe. The brief: "5s
/// timeout each" — a single hung provider must not stall the whole
/// `/doctor` surface.
pub const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(5);

/// Tri-state outcome of one provider health probe.
///
/// Mirrors the three theme colors the diagnostics surface paints —
/// `success` / `warning` / `error` — so the surface can route the
/// `ProviderHealth` straight into a styled row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    /// HTTP 200 from the provider's model-list endpoint.
    Green,
    /// No API key set — nothing to probe, not an error.
    Yellow,
    /// 4xx, 5xx, timeout, DNS failure, or any other transport error.
    Red,
}

/// One provider's health probe result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderHealth {
    /// Human-readable provider name (e.g. `"Anthropic"`).
    pub name: String,
    /// The probe's tri-state outcome.
    pub status: HealthStatus,
    /// A short detail string — the rationale for the status (e.g.
    /// `"reachable"`, `"no key"`, `"401: invalid_api_key"`,
    /// `"unreachable: connect timeout"`).
    pub detail: String,
}

impl ProviderHealth {
    fn new(name: impl Into<String>, status: HealthStatus, detail: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            status,
            detail: detail.into(),
        }
    }

    fn yellow_no_key(name: impl Into<String>) -> Self {
        Self::new(name, HealthStatus::Yellow, "no key")
    }
}

/// Build a `reqwest::Client` with the 5-second health-check timeout
/// policy. Health probes are single-request request/response, not
/// streams — so a request-level cap is correct (the read-timeout
/// between-bytes guard cannot catch a slow-drip server on its own;
/// see `wcore_providers::http_client::build_tool_client` docs).
fn health_client() -> wcore_egress::EgressClient {
    wcore_egress::EgressClient::builder()
        .connect_timeout(HEALTH_CHECK_TIMEOUT)
        .timeout(HEALTH_CHECK_TIMEOUT)
        .build()
        .expect("reqwest TLS backend must initialize at startup")
}

/// Run all four provider health probes concurrently and return the
/// results in canonical order: Anthropic, OpenAI, Gemini, Groq.
pub async fn provider_health_check_all() -> Vec<ProviderHealth> {
    let client = health_client();
    let anth_url = std::env::var("ANTHROPIC_API_BASE")
        .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
    let openai_url =
        std::env::var("OPENAI_API_BASE").unwrap_or_else(|_| "https://api.openai.com".to_string());
    let gemini_url = std::env::var("GEMINI_API_BASE")
        .unwrap_or_else(|_| "https://generativelanguage.googleapis.com".to_string());
    let groq_url =
        std::env::var("GROQ_API_BASE").unwrap_or_else(|_| "https://api.groq.com".to_string());

    let (anthropic, openai, gemini, groq) = futures::join!(
        check_anthropic(&client, &anth_url),
        check_openai(&client, &openai_url),
        check_gemini(&client, &gemini_url),
        check_groq(&client, &groq_url),
    );
    vec![anthropic, openai, gemini, groq]
}

/// Anthropic — `GET /v1/models` with `x-api-key` + `anthropic-version`.
pub async fn check_anthropic(client: &wcore_egress::EgressClient, base: &str) -> ProviderHealth {
    let Some(key) = nonempty_env("ANTHROPIC_API_KEY") else {
        return ProviderHealth::yellow_no_key("Anthropic");
    };
    let url = format!("{}/v1/models", base.trim_end_matches('/'));
    classify(
        "Anthropic",
        client
            .get(&url)
            .header("x-api-key", &key)
            .header("anthropic-version", "2023-06-01")
            .send()
            .await,
    )
    .await
}

/// OpenAI — `GET /v1/models` with `Authorization: Bearer …`.
pub async fn check_openai(client: &wcore_egress::EgressClient, base: &str) -> ProviderHealth {
    let Some(key) = nonempty_env("OPENAI_API_KEY") else {
        return ProviderHealth::yellow_no_key("OpenAI");
    };
    let url = format!("{}/v1/models", base.trim_end_matches('/'));
    classify("OpenAI", client.get(&url).bearer_auth(&key).send().await).await
}

/// Gemini — `GET /v1beta/models` with the key in the `x-goog-api-key`
/// header.
///
/// SECRETS-29: the key must NOT ride in the URL query string (`?key=…`).
/// `classify`'s error branch formats the URL-bearing reqwest error into
/// `ProviderHealth::detail`, which is rendered verbatim in the `/doctor`
/// PROVIDERS row — so a key in the URL leaks into the TUI (and any
/// screenshot / bug report / log it lands in) whenever Gemini is
/// unreachable. Google accepts the key via header, so we use that.
pub async fn check_gemini(client: &wcore_egress::EgressClient, base: &str) -> ProviderHealth {
    let Some(key) = nonempty_env("GEMINI_API_KEY") else {
        return ProviderHealth::yellow_no_key("Gemini");
    };
    let url = format!("{}/v1beta/models", base.trim_end_matches('/'));
    classify(
        "Gemini",
        client.get(&url).header("x-goog-api-key", &key).send().await,
    )
    .await
}

/// Groq — OpenAI-compatible `GET /openai/v1/models` with bearer auth.
pub async fn check_groq(client: &wcore_egress::EgressClient, base: &str) -> ProviderHealth {
    let Some(key) = nonempty_env("GROQ_API_KEY") else {
        return ProviderHealth::yellow_no_key("Groq");
    };
    let url = format!("{}/openai/v1/models", base.trim_end_matches('/'));
    classify("Groq", client.get(&url).bearer_auth(&key).send().await).await
}

/// Read an env var, returning `None` for both "unset" and "empty
/// string" — an empty key is no key.
fn nonempty_env(name: &str) -> Option<String> {
    match std::env::var(name) {
        Ok(v) if !v.is_empty() => Some(v),
        _ => None,
    }
}

/// Convert a `reqwest` send result into a tri-state probe outcome.
///
/// - HTTP 200 → Green / "reachable"
/// - HTTP 4xx/5xx → Red / "`<status>`: `<body excerpt>`"
/// - timeout / connect error / DNS → Red / "unreachable: `<error>`"
async fn classify(
    name: &'static str,
    result: Result<reqwest::Response, wcore_egress::EgressError>,
) -> ProviderHealth {
    match result {
        Ok(resp) => {
            let status = resp.status();
            if status.is_success() {
                ProviderHealth::new(name, HealthStatus::Green, "reachable")
            } else {
                let code = status.as_u16();
                // Pull a small body excerpt for the error detail —
                // bounded so a 10MB error page can't blow up the row.
                let body = resp.text().await.unwrap_or_default();
                let excerpt: String = body.chars().take(120).collect();
                ProviderHealth::new(name, HealthStatus::Red, format!("{code}: {excerpt}"))
            }
        }
        Err(err) => {
            // SECRETS-29: strip the URL from the error before formatting.
            // The detail is rendered verbatim in the `/doctor` PROVIDERS row;
            // a URL-bearing error must never carry a credential into that
            // surface. Defense-in-depth alongside header-based Gemini auth.
            let detail = match err {
                wcore_egress::EgressError::Denied(reason) => {
                    format!("unreachable: egress denied — {reason}")
                }
                wcore_egress::EgressError::Transport(e) => {
                    let kind = if e.is_timeout() {
                        "timeout"
                    } else if e.is_connect() {
                        "connect error"
                    } else {
                        "transport error"
                    };
                    format!("unreachable: {kind} — {}", e.without_url())
                }
            };
            ProviderHealth::new(name, HealthStatus::Red, detail)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // Mutex to serialize env-mutating tests. Tests run in parallel by
    // default, and these tests mutate process-wide env vars — a stray
    // overlap can mask the assertion. The lock is per-test (taken on
    // construction of `EnvBatch`) so a single test can mutate multiple
    // env vars without re-entrant lock acquisition (std::sync::Mutex
    // is NOT re-entrant).
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// A per-test batch of env-var mutations: takes `ENV_LOCK` on
    /// construction and restores every changed key on drop. Use one
    /// `EnvBatch` per test and call `set`/`unset` on it.
    struct EnvBatch {
        priors: Vec<(&'static str, Option<String>)>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvBatch {
        fn new() -> Self {
            let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            Self {
                priors: Vec::new(),
                _lock: lock,
            }
        }

        fn set(&mut self, key: &'static str, val: &str) {
            self.priors.push((key, std::env::var(key).ok()));
            // SAFETY: env mutations are serialized by `ENV_LOCK`.
            unsafe { std::env::set_var(key, val) };
        }

        fn unset(&mut self, key: &'static str) {
            self.priors.push((key, std::env::var(key).ok()));
            // SAFETY: env mutations are serialized by `ENV_LOCK`.
            unsafe { std::env::remove_var(key) };
        }
    }

    impl Drop for EnvBatch {
        fn drop(&mut self) {
            for (key, prior) in self.priors.iter().rev() {
                // SAFETY: env mutations are serialized by `ENV_LOCK`.
                match prior {
                    Some(v) => unsafe { std::env::set_var(key, v) },
                    None => unsafe { std::env::remove_var(key) },
                }
            }
        }
    }

    #[tokio::test]
    async fn provider_health_yellow_when_no_key() {
        let mut env = EnvBatch::new();
        env.unset("ANTHROPIC_API_KEY");
        let client = health_client();
        let h = check_anthropic(&client, "https://api.anthropic.com").await;
        assert_eq!(h.status, HealthStatus::Yellow);
        assert_eq!(h.detail, "no key");
    }

    #[tokio::test]
    async fn provider_health_yellow_when_key_empty_string() {
        // R-H2: empty string is "no key", not "key set to empty".
        let mut env = EnvBatch::new();
        env.set("OPENAI_API_KEY", "");
        let client = health_client();
        let h = check_openai(&client, "https://api.openai.com").await;
        assert_eq!(h.status, HealthStatus::Yellow);
    }

    #[tokio::test]
    async fn provider_health_green_on_200() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_string("{}"))
            .mount(&server)
            .await;

        let mut env = EnvBatch::new();
        env.set("OPENAI_API_KEY", "sk-test");
        let client = health_client();
        let h = check_openai(&client, &server.uri()).await;
        assert_eq!(h.status, HealthStatus::Green);
        assert_eq!(h.detail, "reachable");
    }

    #[tokio::test]
    async fn provider_health_red_on_401() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(401).set_body_string("invalid_api_key"))
            .mount(&server)
            .await;

        let mut env = EnvBatch::new();
        env.set("OPENAI_API_KEY", "sk-bad");
        let client = health_client();
        let h = check_openai(&client, &server.uri()).await;
        assert_eq!(h.status, HealthStatus::Red);
        assert!(h.detail.contains("401"), "detail must include status code");
        assert!(
            h.detail.contains("invalid_api_key"),
            "detail must carry body excerpt: {}",
            h.detail
        );
    }

    #[tokio::test]
    async fn provider_health_red_on_timeout() {
        // TEST-NET-1 (192.0.2.0/24) per RFC 5737 — IANA-reserved for
        // documentation, guaranteed not to route. A connect there
        // either times out or fails with an unreachable error;
        // either way it must classify as Red.
        let mut env = EnvBatch::new();
        env.set("GROQ_API_KEY", "gsk-test");
        // Use a short-cap client so the test finishes quickly while
        // still exercising the timeout branch of `classify`.
        let client = wcore_egress::EgressClient::builder()
            .connect_timeout(Duration::from_millis(200))
            .timeout(Duration::from_millis(200))
            .build()
            .expect("client");
        let h = check_groq(&client, "http://192.0.2.1").await;
        assert_eq!(h.status, HealthStatus::Red);
        assert!(
            h.detail.starts_with("unreachable:"),
            "detail must mark unreachable: {}",
            h.detail
        );
    }

    #[tokio::test]
    async fn provider_health_times_out_at_5s() {
        // Spin a TCP listener that accepts but never replies. The
        // request-level 5s cap must trip the request — and finish in
        // well under 10s end-to-end. We assert both: it errored AND
        // it returned within the budget.
        use tokio::io::AsyncReadExt;
        use tokio::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                std::future::pending::<()>().await;
            }
        });

        // Use the real default 5s health client — that's the contract
        // we ship. Probe against the dead-listening server.
        let mut env = EnvBatch::new();
        env.set("OPENAI_API_KEY", "sk-test");
        let client = health_client();
        let base = format!("http://{addr}");

        let started = Instant::now();
        let h = check_openai(&client, &base).await;
        let elapsed = started.elapsed();

        assert_eq!(h.status, HealthStatus::Red);
        // Must enforce the 5s cap — give a 3s slack for slow CI.
        assert!(
            elapsed < Duration::from_secs(8),
            "health probe must time out within 5s (got {elapsed:?})"
        );

        server.abort();
    }

    #[tokio::test]
    async fn gemini_unreachable_detail_omits_api_key() {
        // SECRETS-29: when Gemini is unreachable, the `/doctor` row detail
        // must NOT carry the API key (or a `key=` query param). It is
        // rendered verbatim in the TUI and copied into bug reports/logs.
        const SECRET_KEY: &str = "AIzaSyTEST_secrets29_leak_canary_value";
        let mut env = EnvBatch::new();
        env.set("GEMINI_API_KEY", SECRET_KEY);
        // TEST-NET-1 (RFC 5737), short cap so the connect fails fast.
        let client = wcore_egress::EgressClient::builder()
            .connect_timeout(Duration::from_millis(200))
            .timeout(Duration::from_millis(200))
            .build()
            .expect("client");
        let h = check_gemini(&client, "http://192.0.2.1").await;
        assert_eq!(h.status, HealthStatus::Red);
        assert!(
            !h.detail.contains(SECRET_KEY),
            "detail leaked the API key: {}",
            h.detail
        );
        assert!(
            !h.detail.contains("key="),
            "detail leaked a key= query param: {}",
            h.detail
        );
    }

    #[tokio::test]
    async fn provider_health_check_all_returns_four_entries_in_order() {
        // Clear every key — the all-yellow path is what we're after.
        let mut env = EnvBatch::new();
        env.unset("ANTHROPIC_API_KEY");
        env.unset("OPENAI_API_KEY");
        env.unset("GEMINI_API_KEY");
        env.unset("GROQ_API_KEY");
        let results = provider_health_check_all().await;
        assert_eq!(results.len(), 4);
        assert_eq!(results[0].name, "Anthropic");
        assert_eq!(results[1].name, "OpenAI");
        assert_eq!(results[2].name, "Gemini");
        assert_eq!(results[3].name, "Groq");
        // All yellow with no keys set.
        for h in &results {
            assert_eq!(h.status, HealthStatus::Yellow, "{} not yellow", h.name);
        }
    }
}
