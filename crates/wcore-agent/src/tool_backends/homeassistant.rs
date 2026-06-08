//! Real Home Assistant REST backend — added in v0.9.0 Wave-1 (B5).
//!
//! Wires `wcore_tools::homeassistant_tool::HomeAssistantBackend` to a
//! live HA instance over the official REST API. Resolves credentials
//! from env: `HASS_URL` (base URL like `http://homeassistant.local:8123`
//! or `https://hass.example.com`) and `HASS_TOKEN` (Long-Lived Access
//! Token from HA UI). Both must be present and non-empty — otherwise
//! [`build_homeassistant_backend`] returns `None` and `bootstrap.rs`
//! skips registering the tool entirely, so the model never sees an
//! `homeassistant` tool that cannot work.
//!
//! ## SSRF
//!
//! The reqwest client is built via [`build_ssrf_safe_tool_client`] so
//! a malicious 302 from a public HA URL to `169.254.169.254` /
//! `10.x.x.x` / `127.0.0.1` / `[fd00::]` is refused mid-redirect.
//! Local HA instances on RFC-1918 addresses are reached on the
//! *initial* hop (no redirect), which is the legitimate happy path —
//! the SSRF policy only fires on redirect re-validation. **`HASS_URL`
//! itself is operator-supplied trusted config**; the deployment-time
//! decision is "do you trust this URL?", same as any production
//! HA setup.
//!
//! ## Two-layer timeout (R-H1)
//!
//! The HTTP exchange uses `reqwest`'s built-in `.timeout()` for connect +
//! read. An outer `tokio::time::timeout` wraps the whole request +
//! body decode + JSON parse pipeline at [`PER_CALL_TIMEOUT`] so a
//! pathological deserializer or a HA box that streams a multi-megabyte
//! state dump cannot pin the agent for unbounded wall-clock time.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use wcore_egress::EgressClient as Client;

use super::build_ssrf_safe_tool_client;
use super::shared::read_env_key;
use wcore_tools::homeassistant_tool::{HaOutcome, HomeAssistantBackend};

/// Wall-clock cap on a single HA REST call — covers HTTP exchange +
/// body decode + JSON parse. Set conservatively: HA's `/api/states`
/// can return tens of KB on a busy install but should still complete
/// well under 15 s on any reasonable network.
const PER_CALL_TIMEOUT: Duration = Duration::from_secs(15);

/// Real Home Assistant REST backend over `reqwest`.
///
/// Holds a base URL + long-lived access token plus a single
/// `reqwest::Client` (SSRF-safe redirect policy + the workspace's
/// non-streaming tool HTTP timeouts). Constructed by
/// [`build_homeassistant_backend`] when both `HASS_URL` and
/// `HASS_TOKEN` are set.
pub struct HttpHomeAssistantBackend {
    client: Client,
    /// Base URL with trailing slash stripped — e.g.
    /// `https://hass.example.com:8123` (no `/api` suffix; we append
    /// per call).
    base_url: String,
    /// Long-Lived Access Token. Sent as `Authorization: Bearer …`.
    token: String,
}

impl HttpHomeAssistantBackend {
    /// Build a backend pointed at `base_url` with `token` as the LLAT.
    /// `base_url` is normalized — any trailing `/` is stripped so URL
    /// composition is consistent. TLS is implied by the `https://`
    /// scheme; `http://` goes over bare TCP (HA installs on LAN are
    /// commonly HTTP).
    pub fn new(base_url: String, token: String) -> Self {
        let base = base_url.trim_end_matches('/').to_string();
        Self {
            client: build_ssrf_safe_tool_client(),
            base_url: base,
            token,
        }
    }

    /// Compose `<base_url>/api/<path>`, where `path` does NOT start
    /// with `/`. Centralized so all four methods agree on the join.
    fn api_url(&self, path: &str) -> String {
        format!("{}/api/{}", self.base_url, path)
    }

    /// Apply the standard auth + content-type headers + reqwest's
    /// own per-call timeout. The outer `tokio::time::timeout` is
    /// applied by the trait methods.
    fn auth_get(&self, url: &str) -> wcore_egress::EgressRequestBuilder {
        self.client
            .get(url)
            .timeout(PER_CALL_TIMEOUT)
            .bearer_auth(&self.token)
            .header(reqwest::header::ACCEPT, "application/json")
    }

    fn auth_post(&self, url: &str) -> wcore_egress::EgressRequestBuilder {
        self.client
            .post(url)
            .timeout(PER_CALL_TIMEOUT)
            .bearer_auth(&self.token)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(reqwest::header::ACCEPT, "application/json")
    }
}

/// Resolver — only returns a backend when BOTH env vars are set
/// (and non-empty per [`read_env_key`]). When either is missing the
/// tool stays hidden via `is_available()`.
pub fn build_homeassistant_backend() -> Option<Arc<dyn HomeAssistantBackend>> {
    let url = read_env_key("HASS_URL")?;
    let token = read_env_key("HASS_TOKEN")?;
    tracing::info!(
        "homeassistant: HASS_URL + HASS_TOKEN resolved, wiring HttpHomeAssistantBackend"
    );
    Some(Arc::new(HttpHomeAssistantBackend::new(url, token)))
}

/// Map a reqwest send error to a structured HA outcome string. Keeps
/// the messages compact and actionable — "timeout"/"connect"/"transport"
/// is enough for a model or a human to diagnose without leaking URL/token.
fn map_send_error(e: &wcore_egress::EgressError) -> String {
    if e.is_timeout() {
        format!("HA request timed out after {}s", PER_CALL_TIMEOUT.as_secs())
    } else if e.is_connect() {
        format!("HA connect failed: {e}")
    } else if e.is_redirect() {
        format!("HA refused redirect (SSRF policy): {e}")
    } else {
        format!("HA transport error: {e}")
    }
}

/// Helper that wraps the whole HTTP + body-read + parse pipeline in a
/// `tokio::time::timeout`. Returns `Err(deadline)` if the outer wall
/// clock fires — surfaces as a clean `HaOutcome::Err`.
async fn run_with_deadline<F, T>(label: &str, fut: F) -> Result<T, String>
where
    F: std::future::Future<Output = Result<T, String>>,
{
    match tokio::time::timeout(PER_CALL_TIMEOUT, fut).await {
        Ok(inner) => inner,
        Err(_) => Err(format!(
            "HA {label} exceeded wall-clock deadline of {}s",
            PER_CALL_TIMEOUT.as_secs()
        )),
    }
}

/// Read the response body as text, then attempt JSON parse. Centralizes
/// the 5xx / 429+Retry-After / malformed-JSON failure paths so all four
/// HA methods agree on shape.
async fn handle_response(label: &str, resp: reqwest::Response) -> Result<Value, String> {
    let status = resp.status();
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let retry_after = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
            .unwrap_or_else(|| "<unset>".to_string());
        return Err(format!(
            "HA {label} returned HTTP 429 (rate limited); Retry-After: {retry_after}"
        ));
    }
    if status.is_server_error() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "HA {label} returned HTTP {}: {}",
            status.as_u16(),
            body.chars().take(300).collect::<String>()
        ));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "HA {label} returned HTTP {}: {}",
            status.as_u16(),
            body.chars().take(300).collect::<String>()
        ));
    }
    let text = resp
        .text()
        .await
        .map_err(|e| format!("HA {label} body read failed: {e}"))?;
    if text.trim().is_empty() {
        // POST /api/services often returns `[]` but also sometimes 200
        // with an empty body for unknown services pre-2024. Treat empty
        // as a null-equivalent ok payload rather than a parse failure.
        return Ok(Value::Null);
    }
    serde_json::from_str::<Value>(&text)
        .map_err(|e| format!("HA {label} response was not valid JSON: {e}"))
}

#[async_trait]
impl HomeAssistantBackend for HttpHomeAssistantBackend {
    async fn list_entities(&self, domain: Option<&str>, area: Option<&str>) -> HaOutcome {
        let url = self.api_url("states");
        let label = "list_entities";
        let fut = async {
            let resp = self
                .auth_get(&url)
                .send()
                .await
                .map_err(|e| map_send_error(&e))?;
            handle_response(label, resp).await
        };
        match run_with_deadline(label, fut).await {
            Err(msg) => HaOutcome::Err(msg),
            Ok(payload) => {
                // HA returns a JSON array of state objects. Filter by
                // domain prefix (`light.*`) and area (matched against
                // `friendly_name` — HA does not return area on this
                // endpoint, so this is a best-effort name contains).
                let arr = match payload.as_array() {
                    Some(a) => a.clone(),
                    None => {
                        return HaOutcome::Err(format!("HA {label} returned a non-array payload"));
                    }
                };
                let filtered: Vec<Value> = arr
                    .into_iter()
                    .filter(|s| {
                        if let Some(d) = domain {
                            match s.get("entity_id").and_then(Value::as_str) {
                                Some(eid) => eid
                                    .split_once('.')
                                    .map(|(prefix, _)| prefix == d)
                                    .unwrap_or(false),
                                None => false,
                            }
                        } else {
                            true
                        }
                    })
                    .filter(|s| {
                        if let Some(a) = area {
                            s.pointer("/attributes/friendly_name")
                                .and_then(Value::as_str)
                                .map(|fn_| fn_.to_lowercase().contains(&a.to_lowercase()))
                                .unwrap_or(false)
                        } else {
                            true
                        }
                    })
                    .collect();
                HaOutcome::Ok(json!({
                    "count": filtered.len(),
                    "entities": filtered,
                }))
            }
        }
    }

    async fn get_state(&self, entity_id: &str) -> HaOutcome {
        let url = self.api_url(&format!("states/{entity_id}"));
        let label = "get_state";
        let fut = async {
            let resp = self
                .auth_get(&url)
                .send()
                .await
                .map_err(|e| map_send_error(&e))?;
            handle_response(label, resp).await
        };
        match run_with_deadline(label, fut).await {
            Err(msg) => HaOutcome::Err(msg),
            Ok(payload) => HaOutcome::Ok(payload),
        }
    }

    async fn list_services(&self, domain: Option<&str>) -> HaOutcome {
        let url = self.api_url("services");
        let label = "list_services";
        let fut = async {
            let resp = self
                .auth_get(&url)
                .send()
                .await
                .map_err(|e| map_send_error(&e))?;
            handle_response(label, resp).await
        };
        match run_with_deadline(label, fut).await {
            Err(msg) => HaOutcome::Err(msg),
            Ok(payload) => {
                // HA returns an array of {domain, services:{…}} objects.
                let arr = match payload.as_array() {
                    Some(a) => a.clone(),
                    None => {
                        return HaOutcome::Err(format!("HA {label} returned a non-array payload"));
                    }
                };
                let filtered: Vec<Value> = arr
                    .into_iter()
                    .filter(|s| match domain {
                        None => true,
                        Some(d) => s.get("domain").and_then(Value::as_str) == Some(d),
                    })
                    .collect();
                HaOutcome::Ok(json!({
                    "count": filtered.len(),
                    "domains": filtered,
                }))
            }
        }
    }

    async fn call_service(
        &self,
        domain: &str,
        service: &str,
        entity_id: Option<&str>,
        data: Option<&Value>,
    ) -> HaOutcome {
        let url = self.api_url(&format!("services/{domain}/{service}"));
        let label = "call_service";

        // Build the POST body. HA expects a flat object with
        // `entity_id` and any extra service params merged in. Domain
        // / service traversal + blocklist were already enforced at the
        // tool dispatch layer (see homeassistant_tool.rs §security
        // invariants) — the backend can trust its inputs.
        let mut body = serde_json::Map::new();
        if let Some(eid) = entity_id {
            body.insert("entity_id".to_string(), Value::String(eid.to_string()));
        }
        if let Some(Value::Object(extra)) = data {
            for (k, v) in extra {
                body.insert(k.clone(), v.clone());
            }
        }
        let body_value = Value::Object(body);

        let fut = async {
            let resp = self
                .auth_post(&url)
                .json(&body_value)
                .send()
                .await
                .map_err(|e| map_send_error(&e))?;
            handle_response(label, resp).await
        };
        match run_with_deadline(label, fut).await {
            Err(msg) => HaOutcome::Err(msg),
            Ok(payload) => {
                // HA returns an array of affected state objects on
                // success — surface as `affected_entities` for the
                // tool layer's stable contract.
                let affected = payload.as_array().cloned().unwrap_or_default();
                HaOutcome::Ok(json!({
                    "success": true,
                    "service": format!("{domain}.{service}"),
                    "affected_entities": affected,
                }))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use wiremock::matchers::{header, method, path, path_regex};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // ----- env-resolver tests (serial: mutate process env) ----------

    #[test]
    #[serial]
    fn build_hass_backend_returns_none_when_url_missing() {
        // SAFETY: tests in this fn are marked `serial` so no other
        // test races on these env vars.
        unsafe {
            std::env::remove_var("HASS_URL");
            std::env::set_var("HASS_TOKEN", "token123");
        }
        assert!(build_homeassistant_backend().is_none());
        unsafe { std::env::remove_var("HASS_TOKEN") };
    }

    #[test]
    #[serial]
    fn build_hass_backend_returns_none_when_token_missing() {
        unsafe {
            std::env::set_var("HASS_URL", "http://homeassistant.local:8123");
            std::env::remove_var("HASS_TOKEN");
        }
        assert!(build_homeassistant_backend().is_none());
        unsafe { std::env::remove_var("HASS_URL") };
    }

    #[test]
    #[serial]
    fn build_hass_backend_returns_none_when_env_var_empty_string() {
        // Closes R-H2: empty / whitespace HASS_URL or HASS_TOKEN must
        // be treated as unset, not as "configured with empty value".
        unsafe {
            std::env::set_var("HASS_URL", "");
            std::env::set_var("HASS_TOKEN", "token123");
        }
        assert!(build_homeassistant_backend().is_none());
        unsafe {
            std::env::set_var("HASS_URL", "http://homeassistant.local:8123");
            std::env::set_var("HASS_TOKEN", "   ");
        }
        assert!(build_homeassistant_backend().is_none());
        unsafe {
            std::env::remove_var("HASS_URL");
            std::env::remove_var("HASS_TOKEN");
        }
    }

    #[test]
    #[serial]
    fn build_hass_backend_returns_some_when_both_set() {
        unsafe {
            std::env::set_var("HASS_URL", "http://homeassistant.local:8123");
            std::env::set_var("HASS_TOKEN", "token123");
        }
        let backend = build_homeassistant_backend();
        assert!(backend.is_some());
        unsafe {
            std::env::remove_var("HASS_URL");
            std::env::remove_var("HASS_TOKEN");
        }
    }

    #[test]
    fn null_default_skips_registration() {
        // Parallel test in homeassistant_tool.rs (Tool layer)
        // already enforces this. This test confirms the resolver +
        // tool agree end-to-end: resolver=None means default tool,
        // which means `is_available()==false`, which means registry
        // filters it out. The full path is exercised in the upstream
        // tool-layer test; here we just sanity-check that resolver
        // returns None when both env vars are unset.
        unsafe {
            std::env::remove_var("HASS_URL");
            std::env::remove_var("HASS_TOKEN");
        }
        assert!(build_homeassistant_backend().is_none());
    }

    // ----- happy-path & body-shape tests ----------------------------

    #[tokio::test]
    async fn hass_list_entities_parses_state_array() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/states"))
            .and(header("Authorization", "Bearer t0k3n"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {
                    "entity_id": "light.kitchen",
                    "state": "on",
                    "attributes": {"friendly_name": "Kitchen Light"}
                },
                {
                    "entity_id": "switch.fan",
                    "state": "off",
                    "attributes": {"friendly_name": "Bedroom Fan"}
                }
            ])))
            .mount(&server)
            .await;

        let backend = HttpHomeAssistantBackend::new(server.uri(), "t0k3n".into());
        let outcome = backend.list_entities(None, None).await;
        match outcome {
            HaOutcome::Ok(v) => {
                assert_eq!(v["count"], json!(2));
                assert!(v["entities"].is_array());
                assert_eq!(v["entities"][0]["entity_id"], json!("light.kitchen"));
            }
            HaOutcome::Err(e) => panic!("expected Ok, got Err: {e}"),
        }

        // Filtered by domain — should return only the light.
        let outcome = backend.list_entities(Some("light"), None).await;
        match outcome {
            HaOutcome::Ok(v) => {
                assert_eq!(v["count"], json!(1));
                assert_eq!(v["entities"][0]["entity_id"], json!("light.kitchen"));
            }
            HaOutcome::Err(e) => panic!("expected Ok filtered, got Err: {e}"),
        }
    }

    #[tokio::test]
    async fn hass_call_service_posts_correct_body_shape() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/services/light/turn_on"))
            .and(header("Authorization", "Bearer t0k3n"))
            .and(header("Content-Type", "application/json"))
            .and(wiremock::matchers::body_json(serde_json::json!({
                "entity_id": "light.kitchen",
                "brightness": 200
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
                {"entity_id": "light.kitchen", "state": "on"}
            ])))
            .mount(&server)
            .await;

        let backend = HttpHomeAssistantBackend::new(server.uri(), "t0k3n".into());
        let extra = json!({"brightness": 200});
        let outcome = backend
            .call_service("light", "turn_on", Some("light.kitchen"), Some(&extra))
            .await;
        match outcome {
            HaOutcome::Ok(v) => {
                assert_eq!(v["success"], json!(true));
                assert_eq!(v["service"], json!("light.turn_on"));
                assert_eq!(v["affected_entities"].as_array().unwrap().len(), 1);
            }
            HaOutcome::Err(e) => panic!("expected Ok, got Err: {e}"),
        }
    }

    #[tokio::test]
    async fn hass_get_state_parses_single_entity() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/states/sensor.temperature"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entity_id": "sensor.temperature",
                "state": "21.5",
                "attributes": {"unit_of_measurement": "°C"}
            })))
            .mount(&server)
            .await;

        let backend = HttpHomeAssistantBackend::new(server.uri(), "t0k3n".into());
        let outcome = backend.get_state("sensor.temperature").await;
        match outcome {
            HaOutcome::Ok(v) => {
                assert_eq!(v["entity_id"], json!("sensor.temperature"));
                assert_eq!(v["state"], json!("21.5"));
            }
            HaOutcome::Err(e) => panic!("expected Ok, got Err: {e}"),
        }
    }

    // ----- failure-path tests (R-H2: at least 5 per backend) --------

    #[tokio::test]
    async fn hass_handles_http_5xx_returns_typed_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/states"))
            .respond_with(ResponseTemplate::new(503).set_body_string("HA is down"))
            .mount(&server)
            .await;

        let backend = HttpHomeAssistantBackend::new(server.uri(), "t0k3n".into());
        match backend.list_entities(None, None).await {
            HaOutcome::Err(msg) => {
                assert!(msg.contains("503"), "expected 503 in msg, got: {msg}");
            }
            HaOutcome::Ok(v) => panic!("expected Err 5xx, got Ok: {v}"),
        }
    }

    #[tokio::test]
    async fn hass_handles_http_429_with_retry_after_backoff() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/states"))
            .respond_with(
                ResponseTemplate::new(429)
                    .insert_header("Retry-After", "30")
                    .set_body_string("too many requests"),
            )
            .mount(&server)
            .await;

        let backend = HttpHomeAssistantBackend::new(server.uri(), "t0k3n".into());
        match backend.list_entities(None, None).await {
            HaOutcome::Err(msg) => {
                assert!(msg.contains("429"), "expected 429 in msg, got: {msg}");
                assert!(
                    msg.contains("Retry-After: 30"),
                    "expected Retry-After surfaced, got: {msg}"
                );
            }
            HaOutcome::Ok(v) => panic!("expected Err 429, got Ok: {v}"),
        }
    }

    #[tokio::test]
    async fn hass_handles_malformed_json_response() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/states"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json {{"))
            .mount(&server)
            .await;

        let backend = HttpHomeAssistantBackend::new(server.uri(), "t0k3n".into());
        match backend.list_entities(None, None).await {
            HaOutcome::Err(msg) => {
                assert!(
                    msg.contains("not valid JSON"),
                    "expected JSON parse error, got: {msg}"
                );
            }
            HaOutcome::Ok(v) => panic!("expected Err malformed JSON, got Ok: {v}"),
        }
    }

    #[tokio::test]
    async fn hass_handles_network_timeout() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/states"))
            .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
            .mount(&server)
            .await;

        // Build a backend with the real PER_CALL_TIMEOUT (15s) — but
        // override the outer deadline via the trait method by racing
        // against tokio::time::timeout at the test layer to avoid a
        // 15s test run. We just assert that the backend completes with
        // a timeout-shaped error within a bounded wall clock.
        let backend = HttpHomeAssistantBackend::new(server.uri(), "t0k3n".into());
        let outcome =
            tokio::time::timeout(Duration::from_secs(20), backend.list_entities(None, None))
                .await
                .expect("outer wall clock fired before backend returned");
        match outcome {
            HaOutcome::Err(msg) => {
                // Either reqwest's timeout fires first ("timed out"),
                // or the outer tokio::time::timeout in run_with_deadline
                // fires ("exceeded wall-clock deadline"). Both are
                // acceptable timeout-class errors.
                assert!(
                    msg.contains("timed out") || msg.contains("deadline"),
                    "expected timeout-class error, got: {msg}"
                );
            }
            HaOutcome::Ok(v) => panic!("expected Err timeout, got Ok: {v}"),
        }
    }

    #[tokio::test]
    async fn hass_handles_transport_error_when_host_unreachable() {
        // Point at a port nothing listens on — reqwest yields a connect
        // error, which `map_send_error` should translate to a clean
        // "HA connect failed" message.
        let backend =
            HttpHomeAssistantBackend::new("http://127.0.0.1:1".to_string(), "t0k3n".into());
        // SSRF policy refuses 127.0.0.1 mid-redirect, but a direct
        // request to 127.0.0.1 is allowed (loopback is reachable by
        // design — same as a LAN HA install). Here we just verify the
        // request fails cleanly, not why it fails.
        match backend.list_entities(None, None).await {
            HaOutcome::Err(_msg) => {
                // Either "connect failed" (port 1 not listening) or
                // "transport error" — both are clean failure modes.
            }
            HaOutcome::Ok(v) => {
                panic!("expected Err transport, got Ok: {v}");
            }
        }
    }

    // ----- SSRF redirect refusal -----------------------------------

    #[tokio::test]
    async fn hass_refuses_ssrf_redirect_to_metadata_service() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path_regex(r"^/api/.*"))
            .respond_with(
                ResponseTemplate::new(302)
                    .insert_header("Location", "http://169.254.169.254/latest/meta-data/"),
            )
            .mount(&server)
            .await;

        let backend = HttpHomeAssistantBackend::new(server.uri(), "t0k3n".into());
        match backend.list_entities(None, None).await {
            HaOutcome::Err(msg) => {
                // The SSRF policy returns an error containing "redirect"
                // or "blocked" via reqwest's redirect error path. Either
                // is acceptable — what matters is we did NOT follow.
                assert!(
                    msg.contains("redirect") || msg.contains("blocked") || msg.contains("SSRF"),
                    "expected SSRF-shaped refusal, got: {msg}"
                );
            }
            HaOutcome::Ok(v) => {
                panic!("SSRF redirect was NOT refused — backend followed to metadata: {v}");
            }
        }
    }

    // ----- API URL composition correctness --------------------------

    #[test]
    fn api_url_strips_trailing_slash_from_base() {
        let with_slash =
            HttpHomeAssistantBackend::new("http://hass.local:8123/".to_string(), "t".into());
        assert_eq!(
            with_slash.api_url("states"),
            "http://hass.local:8123/api/states"
        );

        let without_slash =
            HttpHomeAssistantBackend::new("http://hass.local:8123".to_string(), "t".into());
        assert_eq!(
            without_slash.api_url("states/light.kitchen"),
            "http://hass.local:8123/api/states/light.kitchen"
        );
    }
}
