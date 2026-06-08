//! v0.6.5 Wave 6B.1 — `impl Host for HostState`.
//!
//! Wires the bindgen-generated `wayland:host/host` import trait to the
//! engine-side adapter traits (`WaylandHostHttp`, `WaylandHostWorkspace`,
//! `WaylandHostSecrets`, `WaylandHostTools`, `WaylandHostLog`). All
//! decisions about whether a capability is permitted have already been
//! made by the composition root in [`crate::runner::LoadedWasmPlugin::build_host_state`];
//! this trait impl is the thin translation layer between WIT records and
//! engine-side types.
//!
//! `async: true` mode on the bindgen macro means every Host method is a
//! native `async fn` returning `wasmtime::Result<_>`.

use crate::bindings::tool::wayland::host::host::{Host, HttpReq, HttpResp};
use crate::runner::HostState;

/// Aud-13: decode the WIT `http-req.headers-json` field (a JSON object string)
/// into `(name, value)` pairs for the gated HTTP adapter. An empty string or an
/// empty object means "no headers". A non-empty value that is not a JSON object
/// of string→string is a hard error — surfacing it is the whole point of the
/// fix (the prior code dropped the headers silently). `{{secret:NAME}}` tokens
/// are left intact here; the gated adapter expands them host-side.
fn parse_headers_json(headers_json: &str) -> Result<Vec<(String, String)>, String> {
    let trimmed = headers_json.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return Ok(Vec::new());
    }
    let value: serde_json::Value = serde_json::from_str(trimmed)
        .map_err(|e| format!("http_request: malformed headers-json: {e}"))?;
    let obj = value
        .as_object()
        .ok_or_else(|| "http_request: headers-json must be a JSON object".to_string())?;
    let mut out = Vec::with_capacity(obj.len());
    for (name, v) in obj {
        let s = v
            .as_str()
            .ok_or_else(|| format!("http_request: header '{name}' value must be a JSON string"))?;
        out.push((name.clone(), s.to_string()));
    }
    Ok(out)
}

impl Host for HostState {
    async fn log(&mut self, level: String, msg: String) {
        self.log.log(&level, &msg);
    }

    async fn now_millis(&mut self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    async fn workspace_read(&mut self, path: String) -> Result<Vec<u8>, String> {
        self.workspace.read(&path)
    }

    async fn workspace_write(&mut self, path: String, body: Vec<u8>) -> Result<(), String> {
        self.workspace.write(&path, body)
    }

    async fn http_request(&mut self, req: HttpReq) -> Result<HttpResp, String> {
        // The engine-side trait takes (url, method, headers, body) and returns
        // an HttpResponse {status, body}. Wave 6B.3 made the trait `async`
        // (real reqwest-backed egress); 6B.1 wrote the call site assuming
        // sync — this `.await` + typed-error → String conversion bridges
        // the seam post-merge.
        //
        // Aud-13: the WIT contract advertises `http-req.headers-json` with
        // host-side `{{secret:NAME}}` expansion, but this seam previously
        // forwarded only url/method/body and silently dropped the headers
        // (phantom affordance + lost credentials). Parse the JSON object into
        // name/value pairs and pass them through; the gated adapter expands
        // secrets and validates each header. A malformed `headers-json`
        // surfaces an error instead of being dropped.
        let headers = parse_headers_json(&req.headers_json)?;
        let body = req.body.unwrap_or_default();
        let resp = self
            .http
            .http_request(req.url, req.method, headers, body)
            .await
            .map_err(|e| e.to_string())?;
        Ok(HttpResp {
            status: resp.status,
            headers_json: "{}".to_string(),
            body: resp.body,
        })
    }

    async fn secret_exists(&mut self, name: String) -> bool {
        self.secrets.secret_exists(&name)
    }

    async fn tool_invoke(&mut self, name: String, input: String) -> Result<String, String> {
        self.tools.tool_invoke(&name, &input)
    }

    async fn emit_message(&mut self, role: String, text: String) {
        // No dedicated host trait for chat-emit yet; route through the log
        // adapter at info-level. A first-class emit channel lands in a
        // later wave once the engine surfaces a `WaylandHostChat` trait.
        self.log
            .log("info", &format!("emit-message role={role}: {text}"));
    }

    async fn is_cancelled(&mut self) -> bool {
        // Cooperative cancellation token is not threaded through HostState
        // in v0.6.5; the surface is honest — a guest that polls this gets
        // `false` until the engine cancellation seam lands.
        false
    }
}
