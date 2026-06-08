//! Camoufox sidecar HTTP backend — PRIMARY provider per design §5.16.
//!
//! Talks to a long-running Camoufox sidecar process via HTTP at
//! `localhost:9377/` (default port; configurable). Endpoints:
//!
//!   * `POST /sessions`                   — open a new isolated session
//!   * `DELETE /sessions/<id>`            — close a session
//!   * `POST /sessions/<id>/navigate`     — navigate the tab
//!   * `POST /sessions/<id>/snapshot`     — ARIA tree
//!   * `POST /sessions/<id>/click`        — click element by ref
//!   * `POST /sessions/<id>/fill`         — fill text
//!   * `POST /sessions/<id>/screenshot`   — capture
//!   * `GET  /sessions/<id>/state`        — URL + title
//!   * `GET  /sessions/<id>/network`      — network log
//!   * `GET  /sessions/<id>/console`      — console log
//!
//! Test strategy: wiremock simulates Camoufox at a random port — no real
//! Camoufox install needed for any wcore-browser CI run.

use async_trait::async_trait;
use serde_json::json;

use crate::aria::{RawAriaNode, decode_snapshot};
use crate::op::BrowserOp;
use crate::policy::{BrowserPolicy, PolicyOutcome};
use crate::provider::{BrowserOpError, BrowserProvider, BrowserSession, OpResult, SessionCtx};

#[derive(Debug, Clone)]
pub struct CamoufoxBackend {
    pub base_url: String,
    pub client: wcore_egress::EgressClient,
    /// Policy that the redirect interceptor + post-`Navigate` `final_url`
    /// re-check consult. `None` keeps the legacy (pre-v0.2.1) behavior of
    /// "trust the sidecar"; production paths construct the backend with a
    /// `Some(policy)` so the BLOCKER #3 SSRF surface is closed.
    policy: Option<BrowserPolicy>,
    /// Monotonic snapshot id counter (returned in `Snapshot` op results).
    snapshot_counter: std::sync::Arc<parking_lot::Mutex<u32>>,
}

impl CamoufoxBackend {
    /// Default sidecar URL — `http://localhost:9377`.
    pub fn default_url() -> &'static str {
        "http://localhost:9377"
    }

    pub fn new(base_url: impl Into<String>) -> Self {
        Self::build(base_url.into(), None)
    }

    /// Construct a backend with a [`BrowserPolicy`] wired in. The reqwest
    /// client gets [`BrowserPolicy::reqwest_redirect_policy`] installed so
    /// any 3xx hop from the sidecar gets policy-checked. After a
    /// `Navigate` op, the response's `final_url` field (when present) is
    /// also re-checked against the policy — closes BLOCKER #3 from
    /// `SECURITY-v0.2.0.md` (one-shot policy bypass via redirects).
    pub fn with_policy(base_url: impl Into<String>, policy: BrowserPolicy) -> Self {
        Self::build(base_url.into(), Some(policy))
    }

    fn build(base_url: String, policy: Option<BrowserPolicy>) -> Self {
        let mut url = base_url;
        // Normalize: drop trailing slash.
        if url.ends_with('/') {
            url.pop();
        }
        // Wire the redirect-policy when a BrowserPolicy is present so a
        // 3xx from the sidecar to (say) the metadata endpoint cannot
        // smuggle past the per-hop check.
        //
        // Wave RA RELIABILITY BLOCKER #2 — `pool_idle_timeout` so a
        // browser-op cancelled mid-flight (LLM cancel signal racing the
        // `select!` in `BrowserTool::dispatch_inner`) doesn't leave the
        // underlying TCP connection loitering in the pool, where retry
        // storms could exhaust local fd / remote socket budgets.
        //
        // Wave RC (2026-05-23) — also pin explicit connect + request
        // timeouts on the sidecar HTTP client. The prior build set NO
        // timeouts at all, so a stalled Camoufox sidecar would wedge
        // every reqwest call until the dispatcher's 600s outer backstop
        // (the 10-minute UI hang in the original bug report). The
        // BrowserTool::dispatch_inner per-op deadline races this too, but
        // pinning the HTTP layer is defense-in-depth: it makes the failure
        // mode "fast Network error" instead of "wait for the outer tier."
        //
        // 90s is comfortably larger than the longest per-op deadline
        // (60s Navigate) so a slow-but-completing op isn't punished by
        // the wrong layer.
        let make_client = |maybe_redirect: Option<reqwest::redirect::Policy>| {
            let mut b = wcore_egress::EgressClient::builder()
                .pool_idle_timeout(std::time::Duration::from_secs(5))
                .connect_timeout(std::time::Duration::from_secs(10))
                .timeout(std::time::Duration::from_secs(90));
            if let Some(r) = maybe_redirect {
                b = b.redirect(r);
            }
            // Builder errors are configuration bugs — fall back to the
            // default client so the backend remains usable even if the
            // (unlikely) builder fails. The BrowserPolicy still
            // post-checks `final_url` on Navigate, so the contract
            // isn't lost.
            b.build()
                .unwrap_or_else(|_| wcore_egress::EgressClient::new())
        };
        let client = match policy.as_ref() {
            Some(p) => make_client(Some(p.reqwest_redirect_policy())),
            None => make_client(None),
        };
        Self {
            base_url: url,
            client,
            policy,
            snapshot_counter: std::sync::Arc::new(parking_lot::Mutex::new(0)),
        }
    }

    fn next_snapshot_id(&self) -> u32 {
        let mut g = self.snapshot_counter.lock();
        *g += 1;
        *g
    }

    fn url(&self, path: &str) -> String {
        if path.starts_with('/') {
            format!("{}{}", self.base_url, path)
        } else {
            format!("{}/{}", self.base_url, path)
        }
    }
}

#[async_trait]
impl BrowserProvider for CamoufoxBackend {
    async fn open_session(
        &self,
        persistent_profile: bool,
    ) -> Result<BrowserSession, BrowserOpError> {
        let body = json!({ "persistent_profile": persistent_profile });
        let resp = self
            .client
            .post(self.url("/sessions"))
            .json(&body)
            .send()
            .await
            .map_err(|e| BrowserOpError::Network(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(BrowserOpError::Backend(format!(
                "open_session HTTP {}",
                resp.status()
            )));
        }
        let v: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| BrowserOpError::Backend(format!("open_session json: {e}")))?;
        let id = v
            .get("session_id")
            .and_then(|s| s.as_str())
            .unwrap_or("camoufox-sess");
        Ok(BrowserSession {
            ctx: SessionCtx::for_test(id.to_string()),
            persistent_profile,
        })
    }

    async fn close_session(&self, ctx: &SessionCtx) -> Result<(), BrowserOpError> {
        let _ = self
            .client
            .delete(self.url(&format!("/sessions/{}", ctx.session_id)))
            .send()
            .await
            .map_err(|e| BrowserOpError::Network(e.to_string()))?;
        Ok(())
    }

    async fn dispatch(&self, ctx: &SessionCtx, op: BrowserOp) -> Result<OpResult, BrowserOpError> {
        let sid = &ctx.session_id;
        match op {
            BrowserOp::Navigate {
                url,
                wait_until_loaded,
            } => {
                let body = json!({ "url": url, "wait_until_loaded": wait_until_loaded });
                // The Camoufox sidecar's /navigate endpoint MUST return the
                // post-redirect landing URL as `final_url`. We re-check it
                // against the policy so a 3xx chain that lands on
                // metadata / loopback / file: gets denied AFTER the
                // sidecar followed it. Combined with the redirect-policy
                // baked into the reqwest client, this closes BLOCKER #3
                // even when the sidecar follows redirects internally.
                let resp: serde_json::Value = self
                    .post_json_lenient(&format!("/sessions/{sid}/navigate"), &body)
                    .await?;
                if let Some(policy) = self.policy.as_ref()
                    && let Some(final_url) = resp.get("final_url").and_then(|v| v.as_str())
                {
                    match policy.evaluate(final_url) {
                        PolicyOutcome::Allow => {}
                        PolicyOutcome::Deny { reason } => {
                            return Err(BrowserOpError::PolicyDenied {
                                url: final_url.to_string(),
                                reason: format!("post-redirect final_url: {reason}"),
                            });
                        }
                        PolicyOutcome::Suspend { url: final_url } => {
                            return Err(BrowserOpError::PolicySuspended { url: final_url });
                        }
                    }
                }
                Ok(OpResult::Ok)
            }
            BrowserOp::Snapshot {} => {
                let raw: RawAriaNode = self
                    .post_json(&format!("/sessions/{sid}/snapshot"), &json!({}))
                    .await?;
                let snap_id = self.next_snapshot_id();
                let snap = decode_snapshot(snap_id, "", "", &raw);
                Ok(OpResult::Snapshot { snapshot: snap })
            }
            BrowserOp::Read { mode } => {
                let v: serde_json::Value = self
                    .post_json(&format!("/sessions/{sid}/read"), &json!({ "mode": mode }))
                    .await?;
                let html = v
                    .get("html")
                    .and_then(|s| s.as_str())
                    .unwrap_or("")
                    .to_string();
                Ok(OpResult::Read {
                    markdown: crate::readability::extract(&html, mode),
                })
            }
            BrowserOp::GetState {} => {
                let v: serde_json::Value = self.get_json(&format!("/sessions/{sid}/state")).await?;
                Ok(OpResult::State {
                    url: v
                        .get("url")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                    title: v
                        .get("title")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
            }
            // All other ops POST a JSON body with the op tag + payload; the
            // sidecar interprets and replies with `{"ok":true}` or data.
            other => {
                let endpoint = endpoint_for(&other);
                let body = serde_json::to_value(&other)
                    .map_err(|e| BrowserOpError::Backend(format!("serialize op: {e}")))?;
                self.post_ok(&format!("/sessions/{sid}{endpoint}"), &body)
                    .await?;
                Ok(OpResult::Ok)
            }
        }
    }

    fn backend_name(&self) -> &'static str {
        "camoufox"
    }
}

impl CamoufoxBackend {
    /// Lenient JSON POST — returns `Value::Null` when the response body
    /// isn't JSON-parseable (older sidecar versions may emit
    /// `{"ok": true}` or a bare 200 with no body). The non-Navigate ops
    /// don't need the response body, so this only matters where the
    /// response carries enforcement-bearing fields like `final_url`.
    async fn post_json_lenient(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<serde_json::Value, BrowserOpError> {
        let r = self
            .client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .map_err(|e| BrowserOpError::Network(e.to_string()))?;
        if !r.status().is_success() {
            return Err(BrowserOpError::Backend(format!(
                "POST {path} HTTP {}",
                r.status()
            )));
        }
        // Tolerate missing / empty / non-JSON bodies — return Null.
        Ok(r.json::<serde_json::Value>()
            .await
            .unwrap_or(serde_json::Value::Null))
    }

    async fn post_ok(&self, path: &str, body: &serde_json::Value) -> Result<(), BrowserOpError> {
        let r = self
            .client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .map_err(|e| BrowserOpError::Network(e.to_string()))?;
        if !r.status().is_success() {
            return Err(BrowserOpError::Backend(format!(
                "POST {path} HTTP {}",
                r.status()
            )));
        }
        Ok(())
    }

    async fn post_json<T: for<'de> serde::Deserialize<'de>>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, BrowserOpError> {
        let r = self
            .client
            .post(self.url(path))
            .json(body)
            .send()
            .await
            .map_err(|e| BrowserOpError::Network(e.to_string()))?;
        if !r.status().is_success() {
            return Err(BrowserOpError::Backend(format!(
                "POST {path} HTTP {}",
                r.status()
            )));
        }
        r.json::<T>()
            .await
            .map_err(|e| BrowserOpError::Backend(format!("POST {path} json: {e}")))
    }

    async fn get_json<T: for<'de> serde::Deserialize<'de>>(
        &self,
        path: &str,
    ) -> Result<T, BrowserOpError> {
        let r = self
            .client
            .get(self.url(path))
            .send()
            .await
            .map_err(|e| BrowserOpError::Network(e.to_string()))?;
        if !r.status().is_success() {
            return Err(BrowserOpError::Backend(format!(
                "GET {path} HTTP {}",
                r.status()
            )));
        }
        r.json::<T>()
            .await
            .map_err(|e| BrowserOpError::Backend(format!("GET {path} json: {e}")))
    }
}

fn endpoint_for(op: &BrowserOp) -> &'static str {
    match op {
        BrowserOp::Click { .. } => "/click",
        BrowserOp::Fill { .. } => "/fill",
        BrowserOp::Press { .. } => "/press",
        BrowserOp::Select { .. } => "/select",
        BrowserOp::Upload { .. } => "/upload",
        BrowserOp::Download { .. } => "/download",
        BrowserOp::Screenshot { .. } => "/screenshot",
        BrowserOp::WaitFor { .. } => "/wait_for",
        BrowserOp::NetworkLog {} => "/network",
        BrowserOp::Console {} => "/console",
        BrowserOp::NewTab { .. } => "/new_tab",
        BrowserOp::CloseTab {} => "/close_tab",
        BrowserOp::Back {} => "/back",
        BrowserOp::Forward {} => "/forward",
        // The four ops with custom dispatch paths handled directly above:
        BrowserOp::Navigate { .. }
        | BrowserOp::Snapshot {}
        | BrowserOp::Read { .. }
        | BrowserOp::GetState {} => "/unused",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn open_session_posts_and_returns_id() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sessions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "session_id": "sess-77"
            })))
            .mount(&server)
            .await;
        let cf = CamoufoxBackend::new(server.uri());
        let sess = cf.open_session(false).await.unwrap();
        assert_eq!(sess.ctx.session_id, "sess-77");
    }

    #[tokio::test]
    async fn navigate_calls_sidecar() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/sessions/sess-1/navigate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&server)
            .await;
        let cf = CamoufoxBackend::new(server.uri());
        cf.dispatch(
            &SessionCtx::for_test("sess-1"),
            BrowserOp::Navigate {
                url: "https://example.com".into(),
                wait_until_loaded: true,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn get_state_returns_url_and_title() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sessions/sess-2/state"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "url": "https://example.com/x",
                "title": "Example"
            })))
            .mount(&server)
            .await;
        let cf = CamoufoxBackend::new(server.uri());
        let r = cf
            .dispatch(&SessionCtx::for_test("sess-2"), BrowserOp::GetState {})
            .await
            .unwrap();
        match r {
            OpResult::State { url, title } => {
                assert_eq!(url, "https://example.com/x");
                assert_eq!(title, "Example");
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn http_failure_surfaces_as_backend_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/sessions/sess-3/state"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let cf = CamoufoxBackend::new(server.uri());
        let r = cf
            .dispatch(&SessionCtx::for_test("sess-3"), BrowserOp::GetState {})
            .await;
        assert!(matches!(r, Err(BrowserOpError::Backend(_))));
    }
}
