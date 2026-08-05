//! Camoufox sidecar HTTP backend — PRIMARY provider per design §5.16.
//!
//! Talks to the `@askjo/camofox-browser` HTTP API at `localhost:9377`
//! (default port; configurable). A Wayland browser session maps to one
//! sidecar tab. The sidecar requires both its opaque `tabId` and the
//! caller-minted `userId` on every operation, so this backend retains that
//! identity pair until [`BrowserProvider::close_session`].
//!
//! Test strategy: wiremock simulates Camoufox at a random port — no real
//! Camoufox install needed for any wcore-browser CI run.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::json;

use crate::aria::{AriaNode, AriaSnapshot, ElementRef};
use crate::op::BrowserOp;
use crate::policy::{BrowserPolicy, PolicyOutcome};
use crate::provider::{BrowserOpError, BrowserProvider, BrowserSession, OpResult, SessionCtx};

#[derive(Debug, Clone)]
pub struct CamoufoxBackend {
    pub base_url: String,
    pub client: wcore_egress::EgressClient,
    /// Policy that the redirect interceptor + post-`Navigate` `url`
    /// re-check consult. `None` keeps the legacy (pre-v0.2.1) behavior of
    /// "trust the sidecar"; production paths construct the backend with a
    /// `Some(policy)` so the BLOCKER #3 SSRF surface is closed.
    policy: Option<BrowserPolicy>,
    /// Monotonic snapshot id counter (returned in `Snapshot` op results).
    snapshot_counter: std::sync::Arc<parking_lot::Mutex<u32>>,
    /// `tabId` -> sidecar caller identity. Camoufox does not encode the
    /// owning user in the tab id, and rejects tab operations without it.
    sessions: Arc<parking_lot::Mutex<HashMap<String, SidecarIdentity>>>,
    identity_counter: Arc<AtomicU64>,
}

#[derive(Debug, Clone)]
struct SidecarIdentity {
    user_id: String,
    session_key: String,
}

impl CamoufoxBackend {
    /// Default sidecar URL — `http://localhost:9377`.
    pub fn default_url() -> &'static str {
        "http://localhost:9377"
    }

    pub fn new(base_url: impl Into<String>) -> Self {
        Self::build(base_url.into(), None, camoufox_access_key())
    }

    /// Construct a backend with a [`BrowserPolicy`] wired in. The reqwest
    /// client gets [`BrowserPolicy::reqwest_redirect_policy`] installed so
    /// any 3xx hop from the sidecar gets policy-checked. After a
    /// `Navigate` op, the response's `url` field is
    /// also re-checked against the policy — closes BLOCKER #3 from
    /// `SECURITY-v0.2.0.md` (one-shot policy bypass via redirects).
    pub fn with_policy(base_url: impl Into<String>, policy: BrowserPolicy) -> Self {
        Self::build(base_url.into(), Some(policy), camoufox_access_key())
    }

    #[cfg(test)]
    fn with_access_key(base_url: impl Into<String>, access_key: impl Into<String>) -> Self {
        Self::build(base_url.into(), None, Some(access_key.into()))
    }

    fn build(base_url: String, policy: Option<BrowserPolicy>, access_key: Option<String>) -> Self {
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
            if let Some(key) = access_key.as_deref()
                && let Ok(mut value) =
                    reqwest::header::HeaderValue::from_str(&format!("Bearer {key}"))
            {
                value.set_sensitive(true);
                let mut headers = reqwest::header::HeaderMap::new();
                headers.insert(reqwest::header::AUTHORIZATION, value);
                b = b.default_headers(headers);
            }
            if let Some(r) = maybe_redirect {
                b = b.redirect(r);
            }
            // Builder errors are configuration bugs — fall back to the
            // default client so the backend remains usable even if the
            // (unlikely) builder fails. The BrowserPolicy still
            // post-checks `url` on Navigate, so the contract
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
            sessions: Arc::new(parking_lot::Mutex::new(HashMap::new())),
            identity_counter: Arc::new(AtomicU64::new(0)),
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

    fn new_identity(&self) -> SidecarIdentity {
        let sequence = self.identity_counter.fetch_add(1, Ordering::Relaxed) + 1;
        let nonce = format!("{}-{sequence}", std::process::id());
        SidecarIdentity {
            user_id: format!("wayland-{nonce}"),
            session_key: format!("wcore-{nonce}"),
        }
    }

    fn identity(&self, tab_id: &str) -> Result<SidecarIdentity, BrowserOpError> {
        self.sessions.lock().get(tab_id).cloned().ok_or_else(|| {
            BrowserOpError::Backend(format!(
                "unknown Camoufox tab {tab_id}; open_session must succeed before dispatch"
            ))
        })
    }
}

fn camoufox_access_key() -> Option<String> {
    std::env::var("CAMOFOX_ACCESS_KEY")
        .ok()
        .map(|key| key.trim().to_string())
        .filter(|key| !key.is_empty())
}

#[async_trait]
impl BrowserProvider for CamoufoxBackend {
    async fn open_session(
        &self,
        persistent_profile: bool,
    ) -> Result<BrowserSession, BrowserOpError> {
        let identity = self.new_identity();
        let body = json!({
            "userId": &identity.user_id,
            "sessionKey": &identity.session_key,
        });
        let resp = self
            .client
            .post(self.url("/tabs"))
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
            .get("tabId")
            .and_then(|s| s.as_str())
            .filter(|id| !id.is_empty())
            .ok_or_else(|| BrowserOpError::Backend("open_session response missing tabId".into()))?
            .to_string();
        self.sessions.lock().insert(id.clone(), identity);
        Ok(BrowserSession {
            ctx: SessionCtx::for_test(id),
            persistent_profile,
        })
    }

    async fn close_session(&self, ctx: &SessionCtx) -> Result<(), BrowserOpError> {
        let identity = self.identity(&ctx.session_id)?;
        let response = self
            .client
            .delete(self.url(&format!("/tabs/{}", ctx.session_id)))
            .query(&[("userId", identity.user_id.as_str())])
            .send()
            .await
            .map_err(|e| BrowserOpError::Network(e.to_string()))?;
        if !response.status().is_success() {
            return Err(BrowserOpError::Backend(format!(
                "DELETE /tabs/{} HTTP {}",
                ctx.session_id,
                response.status()
            )));
        }
        self.sessions.lock().remove(&ctx.session_id);
        Ok(())
    }

    async fn dispatch(&self, ctx: &SessionCtx, op: BrowserOp) -> Result<OpResult, BrowserOpError> {
        let sid = &ctx.session_id;
        let identity = self.identity(sid)?;
        match op {
            BrowserOp::Navigate {
                url,
                wait_until_loaded: _,
            } => {
                let body = json!({ "userId": &identity.user_id, "url": &url });
                // The Camoufox sidecar's /navigate endpoint returns the
                // post-redirect landing URL as `url`. We re-check it
                // against the policy so a 3xx chain that lands on
                // metadata / loopback / file: gets denied AFTER the
                // sidecar followed it. Combined with the redirect-policy
                // baked into the reqwest client, this closes BLOCKER #3
                // even when the sidecar follows redirects internally.
                let resp: serde_json::Value = self
                    .post_json_value(&format!("/tabs/{sid}/navigate"), &body)
                    .await?;
                if let Some(policy) = self.policy.as_ref() {
                    // FAIL CLOSED: when a policy is in force, the sidecar MUST
                    // hand back a parseable `url`. If it's absent or
                    // non-string we cannot re-check the post-redirect landing
                    // URL — the `and_then` short-circuit would otherwise SKIP
                    // `policy.evaluate` and return Ok, silently bypassing
                    // BLOCKER #3's redirect-SSRF defense. Deny instead.
                    let Some(final_url) = resp.get("url").and_then(|v| v.as_str()) else {
                        return Err(BrowserOpError::PolicyDenied {
                            url: url.clone(),
                            reason: "post-redirect url missing/unparseable; \
                                     failing closed to enforce redirect policy"
                                .to_string(),
                        });
                    };
                    match policy.evaluate(final_url) {
                        PolicyOutcome::Allow => {}
                        PolicyOutcome::Deny { reason } => {
                            return Err(BrowserOpError::PolicyDenied {
                                url: final_url.to_string(),
                                reason: format!("post-redirect url: {reason}"),
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
                let raw = self
                    .get_json_value(
                        &format!("/tabs/{sid}/snapshot"),
                        &[("userId", identity.user_id.as_str())],
                    )
                    .await?;
                let text = raw
                    .get("snapshot")
                    .and_then(|value| value.as_str())
                    .ok_or_else(|| {
                        BrowserOpError::Backend("snapshot response missing snapshot text".into())
                    })?;
                let url = raw
                    .get("url")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                let snap_id = self.next_snapshot_id();
                let snap = sidecar_snapshot(snap_id, url, text);
                Ok(OpResult::Snapshot { snapshot: snap })
            }
            BrowserOp::Read { .. } => Err(BrowserOpError::Unsupported(
                "Camoufox does not expose source HTML for readability extraction; use snapshot"
                    .into(),
            )),
            BrowserOp::GetState {} => {
                let v = self
                    .get_json_value("/tabs", &[("userId", identity.user_id.as_str())])
                    .await?;
                let tab = v
                    .get("tabs")
                    .and_then(|value| value.as_array())
                    .and_then(|tabs| {
                        tabs.iter().find(|tab| {
                            tab.get("tabId").and_then(|value| value.as_str()) == Some(sid)
                        })
                    })
                    .ok_or_else(|| {
                        BrowserOpError::Backend(format!("Camoufox tab {sid} not found"))
                    })?;
                Ok(OpResult::State {
                    url: tab
                        .get("url")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                    title: tab
                        .get("title")
                        .and_then(|s| s.as_str())
                        .unwrap_or("")
                        .to_string(),
                })
            }
            BrowserOp::Click { target } => {
                self.post_ok(
                    &format!("/tabs/{sid}/click"),
                    &json!({ "userId": &identity.user_id, "ref": target.as_str() }),
                )
                .await?;
                Ok(OpResult::Ok)
            }
            BrowserOp::Fill { target, text } => {
                self.post_ok(
                    &format!("/tabs/{sid}/type"),
                    &json!({
                        "userId": &identity.user_id,
                        "ref": target.as_str(),
                        "text": text,
                        "mode": "fill",
                    }),
                )
                .await?;
                Ok(OpResult::Ok)
            }
            BrowserOp::Press { key } => {
                self.post_ok(
                    &format!("/tabs/{sid}/press"),
                    &json!({ "userId": &identity.user_id, "key": key }),
                )
                .await?;
                Ok(OpResult::Ok)
            }
            BrowserOp::Screenshot { opts } => {
                let response = self
                    .client
                    .get(self.url(&format!("/tabs/{sid}/screenshot")))
                    .query(&[
                        ("userId", identity.user_id.as_str()),
                        ("fullPage", if opts.full_page { "true" } else { "false" }),
                    ])
                    .send()
                    .await
                    .map_err(|e| BrowserOpError::Network(e.to_string()))?;
                if !response.status().is_success() {
                    return Err(BrowserOpError::Backend(format!(
                        "GET /tabs/{sid}/screenshot HTTP {}",
                        response.status()
                    )));
                }
                let bytes = response.bytes().await.map_err(|e| {
                    BrowserOpError::Backend(format!("GET /tabs/{sid}/screenshot body: {e}"))
                })?;
                Ok(OpResult::Screenshot {
                    b64: base64::engine::general_purpose::STANDARD.encode(bytes),
                    format: "png".into(),
                })
            }
            history_op @ (BrowserOp::Back {} | BrowserOp::Forward {}) => {
                let endpoint = if matches!(history_op, BrowserOp::Back {}) {
                    "back"
                } else {
                    "forward"
                };
                let response = self
                    .post_json_value(
                        &format!("/tabs/{sid}/{endpoint}"),
                        &json!({ "userId": &identity.user_id }),
                    )
                    .await?;
                if let (Some(policy), Some(url)) = (
                    self.policy.as_ref(),
                    response.get("url").and_then(|value| value.as_str()),
                ) {
                    enforce_post_navigation_policy(policy, url)?;
                }
                Ok(OpResult::Ok)
            }
            unsupported => Err(BrowserOpError::Unsupported(format!(
                "{} does not have a truthful Camoufox API mapping",
                op_name(&unsupported)
            ))),
        }
    }

    fn backend_name(&self) -> &'static str {
        "camoufox"
    }
}

impl CamoufoxBackend {
    async fn post_json_value(
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
        r.json::<serde_json::Value>()
            .await
            .map_err(|e| BrowserOpError::Backend(format!("POST {path} json: {e}")))
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

    async fn get_json_value(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<serde_json::Value, BrowserOpError> {
        let r = self
            .client
            .get(self.url(path))
            .query(query)
            .send()
            .await
            .map_err(|e| BrowserOpError::Network(e.to_string()))?;
        if !r.status().is_success() {
            return Err(BrowserOpError::Backend(format!(
                "GET {path} HTTP {}",
                r.status()
            )));
        }
        r.json::<serde_json::Value>()
            .await
            .map_err(|e| BrowserOpError::Backend(format!("GET {path} json: {e}")))
    }
}

fn op_name(op: &BrowserOp) -> &'static str {
    match op {
        BrowserOp::Navigate { .. } => "navigate",
        BrowserOp::Snapshot {} => "snapshot",
        BrowserOp::Read { .. } => "read",
        BrowserOp::Click { .. } => "click",
        BrowserOp::Fill { .. } => "fill",
        BrowserOp::Press { .. } => "press",
        BrowserOp::Select { .. } => "select",
        BrowserOp::Upload { .. } => "upload",
        BrowserOp::Download { .. } => "download",
        BrowserOp::Screenshot { .. } => "screenshot",
        BrowserOp::GetState {} => "get_state",
        BrowserOp::WaitFor { .. } => "wait_for",
        BrowserOp::NetworkLog {} => "network_log",
        BrowserOp::Console {} => "console",
        BrowserOp::NewTab { .. } => "new_tab",
        BrowserOp::CloseTab {} => "close_tab",
        BrowserOp::Back {} => "back",
        BrowserOp::Forward {} => "forward",
    }
}

fn enforce_post_navigation_policy(
    policy: &BrowserPolicy,
    final_url: &str,
) -> Result<(), BrowserOpError> {
    match policy.evaluate(final_url) {
        PolicyOutcome::Allow => Ok(()),
        PolicyOutcome::Deny { reason } => Err(BrowserOpError::PolicyDenied {
            url: final_url.to_string(),
            reason: format!("post-redirect url: {reason}"),
        }),
        PolicyOutcome::Suspend { url } => Err(BrowserOpError::PolicySuspended { url }),
    }
}

fn sidecar_snapshot(snapshot_id: u32, url: &str, text: &str) -> AriaSnapshot {
    let mut nodes = Vec::new();
    let mut normalized = String::with_capacity(text.len());
    for line in text.lines() {
        let mut cursor = 0;
        while let Some(relative_start) = line[cursor..].find("[e") {
            let start = cursor + relative_start;
            normalized.push_str(&line[cursor..start]);
            let Some(relative_end) = line[start..].find(']') else {
                cursor = start;
                break;
            };
            let end = start + relative_end;
            let reference = &line[start + 1..end];
            if reference.len() > 1
                && reference.starts_with('e')
                && reference[1..]
                    .chars()
                    .all(|character| character.is_ascii_digit())
            {
                normalized.push_str("[@");
                normalized.push_str(reference);
                normalized.push(']');
                if !nodes
                    .iter()
                    .any(|node: &AriaNode| node.element_ref.as_str() == reference)
                {
                    nodes.push(AriaNode {
                        element_ref: ElementRef::new(reference),
                        role: "element".into(),
                        name: line.trim().to_string(),
                        value: None,
                        children: Vec::new(),
                    });
                }
            } else {
                normalized.push_str(&line[start..=end]);
            }
            cursor = end + 1;
        }
        normalized.push_str(&line[cursor..]);
        normalized.push('\n');
    }
    if !text.ends_with('\n') {
        normalized.pop();
    }
    AriaSnapshot {
        snapshot_id,
        url: url.to_string(),
        title: String::new(),
        nodes,
        text: normalized,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::ScreenshotOpts;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn mount_open(server: &MockServer, tab_id: &str) {
        Mock::given(method("POST"))
            .and(path("/tabs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "tabId": tab_id,
                "url": "about:blank"
            })))
            .mount(server)
            .await;
    }

    #[tokio::test]
    async fn open_session_uses_real_tab_contract_and_retains_identity() {
        let server = MockServer::start().await;
        mount_open(&server, "tab-77").await;
        let cf = CamoufoxBackend::new(server.uri());
        let sess = cf.open_session(false).await.unwrap();
        assert_eq!(sess.ctx.session_id, "tab-77");

        let requests = server.received_requests().await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
        assert!(body["userId"].as_str().unwrap().starts_with("wayland-"));
        assert!(body["sessionKey"].as_str().unwrap().starts_with("wcore-"));
        assert_eq!(cf.identity("tab-77").unwrap().user_id, body["userId"]);
    }

    #[tokio::test]
    async fn access_key_authenticates_sidecar_requests() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/tabs"))
            .and(header("authorization", "Bearer local-sidecar-secret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "tabId": "auth-tab",
                "url": "about:blank"
            })))
            .mount(&server)
            .await;
        let cf = CamoufoxBackend::with_access_key(server.uri(), "local-sidecar-secret");
        let session = cf.open_session(false).await.unwrap();
        assert_eq!(session.ctx.session_id, "auth-tab");
    }

    #[tokio::test]
    async fn close_session_deletes_tab_with_owning_user() {
        let server = MockServer::start().await;
        mount_open(&server, "tab-close").await;
        Mock::given(method("DELETE"))
            .and(path("/tabs/tab-close"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&server)
            .await;
        let cf = CamoufoxBackend::new(server.uri());
        let session = cf.open_session(false).await.unwrap();
        let expected_user = cf.identity("tab-close").unwrap().user_id;

        cf.close_session(&session.ctx).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let delete = requests
            .iter()
            .find(|request| request.method.as_str() == "DELETE")
            .unwrap();
        assert_eq!(
            delete
                .url
                .query_pairs()
                .find(|(key, _)| key == "userId")
                .unwrap()
                .1,
            expected_user
        );
        assert!(cf.identity("tab-close").is_err());
    }

    #[tokio::test]
    async fn navigate_uses_tab_route_and_real_response_url() {
        let server = MockServer::start().await;
        mount_open(&server, "tab-1").await;
        Mock::given(method("POST"))
            .and(path("/tabs/tab-1/navigate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "ok": true,
                "tabId": "tab-1",
                "url": "https://example.com/landed"
            })))
            .mount(&server)
            .await;
        let cf = CamoufoxBackend::new(server.uri());
        let session = cf.open_session(false).await.unwrap();
        cf.dispatch(
            &session.ctx,
            BrowserOp::Navigate {
                url: "https://example.com".into(),
                wait_until_loaded: true,
            },
        )
        .await
        .unwrap();

        let requests = server.received_requests().await.unwrap();
        let navigate = requests
            .iter()
            .find(|request| request.url.path().ends_with("/navigate"))
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&navigate.body).unwrap();
        assert_eq!(body["url"], "https://example.com");
        assert!(body["userId"].as_str().unwrap().starts_with("wayland-"));
    }

    #[tokio::test]
    async fn snapshot_uses_get_query_and_exposes_at_refs() {
        let server = MockServer::start().await;
        mount_open(&server, "tab-snap").await;
        Mock::given(method("GET"))
            .and(path("/tabs/tab-snap/snapshot"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "url": "https://example.com/",
                "snapshot": "- heading \"Example\"\n- button \"Submit\" [e1]\n- link \"Help\" [e2]"
            })))
            .mount(&server)
            .await;
        let cf = CamoufoxBackend::new(server.uri());
        let session = cf.open_session(false).await.unwrap();
        let r = cf
            .dispatch(&session.ctx, BrowserOp::Snapshot {})
            .await
            .unwrap();
        match r {
            OpResult::Snapshot { snapshot } => {
                assert_eq!(snapshot.url, "https://example.com/");
                assert_eq!(snapshot.nodes.len(), 2);
                assert_eq!(snapshot.nodes[0].element_ref.display(), "@e1");
                assert!(snapshot.text.contains("[@e1]"));
                assert!(snapshot.text.contains("[@e2]"));
            }
            other => panic!("unexpected: {other:?}"),
        }
        let requests = server.received_requests().await.unwrap();
        let snapshot = requests
            .iter()
            .find(|request| request.url.path().ends_with("/snapshot"))
            .unwrap();
        assert!(
            snapshot
                .url
                .query_pairs()
                .any(|(key, value)| { key == "userId" && value.starts_with("wayland-") })
        );
    }

    #[tokio::test]
    async fn get_state_uses_real_tab_listing() {
        let server = MockServer::start().await;
        mount_open(&server, "tab-state").await;
        Mock::given(method("GET"))
            .and(path("/tabs"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "running": true,
                "tabs": [{
                    "tabId": "tab-state",
                    "url": "https://example.com/x",
                    "title": "Example"
                }]
            })))
            .mount(&server)
            .await;
        let cf = CamoufoxBackend::new(server.uri());
        let session = cf.open_session(false).await.unwrap();
        let r = cf
            .dispatch(&session.ctx, BrowserOp::GetState {})
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
    async fn navigate_fails_closed_when_url_missing_under_policy() {
        use crate::policy::{BrowserPolicy, PolicyAction};
        let server = MockServer::start().await;
        mount_open(&server, "tab-fc").await;
        Mock::given(method("POST"))
            .and(path("/tabs/tab-fc/navigate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&server)
            .await;
        let policy = BrowserPolicy::new(PolicyAction::Allow, vec!["example.com".into()], vec![]);
        let cf = CamoufoxBackend::with_policy(server.uri(), policy);
        let session = cf.open_session(false).await.unwrap();
        let r = cf
            .dispatch(
                &session.ctx,
                BrowserOp::Navigate {
                    url: "https://example.com/".into(),
                    wait_until_loaded: true,
                },
            )
            .await;
        match r {
            Err(BrowserOpError::PolicyDenied { reason, .. }) => {
                assert!(
                    reason.contains("url") && reason.contains("failing closed"),
                    "unexpected reason: {reason}"
                );
            }
            other => panic!("expected PolicyDenied fail-closed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn navigate_without_policy_tolerates_missing_url() {
        let server = MockServer::start().await;
        mount_open(&server, "tab-np").await;
        Mock::given(method("POST"))
            .and(path("/tabs/tab-np/navigate"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
            .mount(&server)
            .await;
        let cf = CamoufoxBackend::new(server.uri());
        let session = cf.open_session(false).await.unwrap();
        let r = cf
            .dispatch(
                &session.ctx,
                BrowserOp::Navigate {
                    url: "https://example.com/".into(),
                    wait_until_loaded: true,
                },
            )
            .await
            .unwrap();
        assert!(matches!(r, OpResult::Ok));
    }

    #[tokio::test]
    async fn interactions_and_screenshot_use_real_routes() {
        let server = MockServer::start().await;
        mount_open(&server, "tab-actions").await;
        for endpoint in ["click", "type", "press"] {
            Mock::given(method("POST"))
                .and(path(format!("/tabs/tab-actions/{endpoint}")))
                .respond_with(ResponseTemplate::new(200).set_body_json(json!({"ok": true})))
                .mount(&server)
                .await;
        }
        Mock::given(method("GET"))
            .and(path("/tabs/tab-actions/screenshot"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes([0x89, b'P', b'N', b'G']))
            .mount(&server)
            .await;
        let cf = CamoufoxBackend::new(server.uri());
        let session = cf.open_session(false).await.unwrap();
        cf.dispatch(
            &session.ctx,
            BrowserOp::Click {
                target: ElementRef::new("e1"),
            },
        )
        .await
        .unwrap();
        cf.dispatch(
            &session.ctx,
            BrowserOp::Fill {
                target: ElementRef::new("e2"),
                text: "hello".into(),
            },
        )
        .await
        .unwrap();
        cf.dispatch(
            &session.ctx,
            BrowserOp::Press {
                key: "Enter".into(),
            },
        )
        .await
        .unwrap();
        let screenshot = cf
            .dispatch(
                &session.ctx,
                BrowserOp::Screenshot {
                    opts: ScreenshotOpts::default(),
                },
            )
            .await
            .unwrap();
        match screenshot {
            OpResult::Screenshot { b64, format } => {
                assert_eq!(format, "png");
                assert_eq!(
                    base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .unwrap(),
                    [0x89, b'P', b'N', b'G']
                );
            }
            other => panic!("unexpected: {other:?}"),
        }
    }

    #[tokio::test]
    async fn unmapped_operations_fail_loud_instead_of_calling_invented_routes() {
        let server = MockServer::start().await;
        mount_open(&server, "tab-unsupported").await;
        let cf = CamoufoxBackend::new(server.uri());
        let session = cf.open_session(false).await.unwrap();
        let result = cf.dispatch(&session.ctx, BrowserOp::NetworkLog {}).await;
        assert!(matches!(
            result,
            Err(BrowserOpError::Unsupported(message)) if message.contains("network_log")
        ));
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }
}
