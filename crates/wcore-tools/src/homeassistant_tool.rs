//! T3-3.7 -- `homeassistant` smart-home tool.
//!
//! Ported from an upstream MIT-licensed library (see THIRD-PARTY-NOTICES.md).
//! The Python original exposes four LLM-callable tools -- `ha_list_entities`,
//! `ha_get_state`, `ha_list_services`, `ha_call_service` -- that each talk
//! directly to the Home Assistant REST API via `aiohttp`. Wayland's
//! engine deliberately ships no embedded HTTP client for vendor
//! integrations; the host wires a concrete backend at registration time.
//!
//! This port collapses the four tools into a single
//! [`HomeAssistantTool`] keyed on an `operation` discriminator. The
//! dispatch surface enforces all input validation and the
//! security-critical service blocklist before the backend is called,
//! so a misbehaving backend cannot bypass safety.
//!
//! Pluggable seam (mirror of `vision_tools.rs` / `send_message.rs`):
//!
//! * [`HomeAssistantBackend`] -- async trait with four methods, one per
//!   HA REST endpoint. Implementations talk to a real HA instance.
//! * [`NullHomeAssistantBackend`] -- default; every call fails loudly
//!   with a structured error. Honors the NO-STUBS contract.
//! * [`CapturingHomeAssistantBackend`] -- in-memory recorder + canned
//!   responses for hermetic tests; lives in the prod module so
//!   downstream crates can reuse it.
//!
//! ## Security invariants enforced at the dispatch layer
//!
//! * `entity_id` must match `^[a-z_][a-z0-9_]*\.[a-z0-9_]+$`.
//! * `domain` and `service` must match `^[a-z][a-z0-9_]*$` -- prevents
//!   path traversal in `/api/services/{domain}/{service}` (e.g.
//!   `shell_command/../light`) and any blocklist bypass.
//! * `domain` is rejected if present in [`BLOCKED_SERVICE_DOMAINS`]
//!   -- `shell_command`, `command_line`, `python_script`, `pyscript`,
//!   `hassio`, `rest_command`. These all expose arbitrary code/command
//!   execution or SSRF on the HA host.
//!
//! All three checks happen before dispatching to the backend; the
//! backend trait sees only validated, safe inputs.

use std::sync::Arc;

use async_trait::async_trait;
use regex::Regex;
use serde_json::{Value, json};
use std::sync::OnceLock;

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Service domains blocked for security. Mirrors the Python
/// `_BLOCKED_DOMAINS` frozenset.
///
/// # Why these specific domains
///
/// All listed domains expose generic code/command execution surfaces:
/// `shell_command`, `command_line`, and `rest_command` execute arbitrary
/// strings; `python_script`/`pyscript` run user-supplied Python; `hassio`
/// reaches into the Home Assistant supervisor.
///
/// # Not on this list
///
/// * `exec` — Home Assistant does NOT expose a generic `exec` service.
///   Add it here only if HA introduces one.
/// * `notify` — notify domains route to legitimate user notifications
///   (push, SMS, email integrations) and have no generic-execution surface.
///
/// When HA adds a new generic-execution surface, append its domain here.
pub const BLOCKED_SERVICE_DOMAINS: &[&str] = &[
    "shell_command",
    "command_line",
    "python_script",
    "pyscript",
    "hassio",
    "rest_command",
];

fn entity_id_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z_][a-z0-9_]*\.[a-z0-9_]+$").unwrap())
}

fn service_name_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"^[a-z][a-z0-9_]*$").unwrap())
}

/// Validate an HA `entity_id`. Pure function -- no IO.
pub fn validate_entity_id(entity_id: &str) -> Result<(), String> {
    if !entity_id_re().is_match(entity_id) {
        return Err(format!("Invalid entity_id format: {entity_id:?}"));
    }
    Ok(())
}

/// Validate an HA `domain` or `service` name. Prevents path traversal
/// in `/api/services/{domain}/{service}`.
pub fn validate_service_name(name: &str, label: &str) -> Result<(), String> {
    if !service_name_re().is_match(name) {
        return Err(format!("Invalid {label} format: {name:?}"));
    }
    Ok(())
}

/// Reject any [`BLOCKED_SERVICE_DOMAINS`] entry.
pub fn check_service_domain_allowed(domain: &str) -> Result<(), String> {
    if BLOCKED_SERVICE_DOMAINS.contains(&domain) {
        let mut sorted = BLOCKED_SERVICE_DOMAINS.to_vec();
        sorted.sort();
        return Err(format!(
            "Service domain {domain:?} is blocked for security. Blocked domains: {}",
            sorted.join(", ")
        ));
    }
    Ok(())
}

/// Outcome of a backend call.
#[derive(Debug, Clone)]
pub enum HaOutcome {
    Ok(Value),
    Err(String),
}

/// Pluggable Home Assistant backend. One method per HA REST endpoint.
#[async_trait]
pub trait HomeAssistantBackend: Send + Sync {
    async fn list_entities(&self, domain: Option<&str>, area: Option<&str>) -> HaOutcome;
    async fn get_state(&self, entity_id: &str) -> HaOutcome;
    async fn list_services(&self, domain: Option<&str>) -> HaOutcome;
    async fn call_service(
        &self,
        domain: &str,
        service: &str,
        entity_id: Option<&str>,
        data: Option<&Value>,
    ) -> HaOutcome;
}

/// Default backend returned when the host wires nothing. Every call
/// fails loudly with a structured error -- honors the NO-STUBS contract.
pub struct NullHomeAssistantBackend;

#[async_trait]
impl HomeAssistantBackend for NullHomeAssistantBackend {
    async fn list_entities(&self, _domain: Option<&str>, _area: Option<&str>) -> HaOutcome {
        HaOutcome::Err(unwired_message())
    }
    async fn get_state(&self, _entity_id: &str) -> HaOutcome {
        HaOutcome::Err(unwired_message())
    }
    async fn list_services(&self, _domain: Option<&str>) -> HaOutcome {
        HaOutcome::Err(unwired_message())
    }
    async fn call_service(
        &self,
        _domain: &str,
        _service: &str,
        _entity_id: Option<&str>,
        _data: Option<&Value>,
    ) -> HaOutcome {
        HaOutcome::Err(unwired_message())
    }
}

fn unwired_message() -> String {
    "No Home Assistant backend configured. Wire a HomeAssistantBackend implementation when \
     constructing HomeAssistantTool to enable smart-home integration."
        .to_string()
}

/// Single captured backend invocation -- useful for assertions in tests.
#[derive(Debug, Clone)]
pub enum CapturedHaCall {
    ListEntities {
        domain: Option<String>,
        area: Option<String>,
    },
    GetState {
        entity_id: String,
    },
    ListServices {
        domain: Option<String>,
    },
    CallService {
        domain: String,
        service: String,
        entity_id: Option<String>,
        data: Option<Value>,
    },
}

/// In-memory backend that captures every call and returns canned
/// responses for tests.
pub struct CapturingHomeAssistantBackend {
    pub list_entities_response: Value,
    pub get_state_response: Value,
    pub list_services_response: Value,
    pub call_service_response: Value,
    pub captured: parking_lot::Mutex<Vec<CapturedHaCall>>,
}

impl CapturingHomeAssistantBackend {
    pub fn new() -> Self {
        Self {
            list_entities_response: json!({"count": 0, "entities": []}),
            get_state_response: json!({"entity_id": "", "state": "", "attributes": {}}),
            list_services_response: json!({"count": 0, "domains": []}),
            call_service_response: json!({"success": true, "service": "", "affected_entities": []}),
            captured: parking_lot::Mutex::new(Vec::new()),
        }
    }
    pub fn with_list_entities_response(mut self, v: Value) -> Self {
        self.list_entities_response = v;
        self
    }
    pub fn with_get_state_response(mut self, v: Value) -> Self {
        self.get_state_response = v;
        self
    }
    pub fn with_list_services_response(mut self, v: Value) -> Self {
        self.list_services_response = v;
        self
    }
    pub fn with_call_service_response(mut self, v: Value) -> Self {
        self.call_service_response = v;
        self
    }
    pub fn snapshot(&self) -> Vec<CapturedHaCall> {
        self.captured.lock().clone()
    }
}

impl Default for CapturingHomeAssistantBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl HomeAssistantBackend for CapturingHomeAssistantBackend {
    async fn list_entities(&self, domain: Option<&str>, area: Option<&str>) -> HaOutcome {
        self.captured.lock().push(CapturedHaCall::ListEntities {
            domain: domain.map(str::to_string),
            area: area.map(str::to_string),
        });
        HaOutcome::Ok(self.list_entities_response.clone())
    }
    async fn get_state(&self, entity_id: &str) -> HaOutcome {
        self.captured.lock().push(CapturedHaCall::GetState {
            entity_id: entity_id.to_string(),
        });
        HaOutcome::Ok(self.get_state_response.clone())
    }
    async fn list_services(&self, domain: Option<&str>) -> HaOutcome {
        self.captured.lock().push(CapturedHaCall::ListServices {
            domain: domain.map(str::to_string),
        });
        HaOutcome::Ok(self.list_services_response.clone())
    }
    async fn call_service(
        &self,
        domain: &str,
        service: &str,
        entity_id: Option<&str>,
        data: Option<&Value>,
    ) -> HaOutcome {
        self.captured.lock().push(CapturedHaCall::CallService {
            domain: domain.to_string(),
            service: service.to_string(),
            entity_id: entity_id.map(str::to_string),
            data: data.cloned(),
        });
        HaOutcome::Ok(self.call_service_response.clone())
    }
}

/// `homeassistant` tool -- dispatches to a host-wired backend.
pub struct HomeAssistantTool {
    backend: Arc<dyn HomeAssistantBackend>,
    /// v0.9.0 W1: defaults `false` so `Tool::is_available()` hides the
    /// tool when no real backend is wired. `new(backend)` flips it on.
    backend_configured: bool,
}

impl HomeAssistantTool {
    pub fn new(backend: Arc<dyn HomeAssistantBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

impl Default for HomeAssistantTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullHomeAssistantBackend),
            backend_configured: false,
        }
    }
}

/// Parse the `data` argument as either a JSON object value or a JSON
/// string that decodes to an object. Mirrors the Python original which
/// accepts both forms.
fn parse_data_arg(v: Option<&Value>) -> Result<Option<Value>, String> {
    match v {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Object(_)) => Ok(v.cloned()),
        Some(Value::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return Ok(None);
            }
            serde_json::from_str::<Value>(trimmed)
                .map(Some)
                .map_err(|e| format!("Invalid JSON string in 'data' parameter: {e}"))
        }
        Some(other) => Err(format!(
            "'data' must be a JSON object or JSON-encoded string (got {})",
            type_label(other)
        )),
    }
}

fn type_label(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn err_result(message: impl Into<String>) -> ToolResult {
    ToolResult {
        content: json!({ "success": false, "error": message.into() }).to_string(),
        is_error: true,
    }
}

fn ok_result(result: Value) -> ToolResult {
    ToolResult {
        content: json!({ "success": true, "result": result }).to_string(),
        is_error: false,
    }
}

fn opt_str<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

#[async_trait]
impl Tool for HomeAssistantTool {
    fn name(&self) -> &str {
        "homeassistant"
    }

    /// v0.9.0 W1: hidden when no real `HomeAssistantBackend` is wired.
    /// `Default::default()` yields `backend_configured == false`, so
    /// `ToolRegistry::register` drops the tool before the model sees it.
    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Control and inspect a Home Assistant smart-home instance. Supports four operations: \
         'list_entities' (filter by domain/area), 'get_state' (one entity), 'list_services' \
         (discover what actions are available), and 'call_service' (turn_on, turn_off, \
         set_temperature, etc.). High-risk service domains (shell_command, command_line, \
         python_script, pyscript, hassio, rest_command) are blocked at the dispatch layer."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["list_entities", "get_state", "list_services", "call_service"],
                    "description": "Which Home Assistant operation to perform.",
                },
                "domain": {
                    "type": "string",
                    "description": "HA domain (e.g. 'light', 'switch', 'climate'). Required for 'call_service'; optional filter for 'list_entities' and 'list_services'.",
                },
                "service": {
                    "type": "string",
                    "description": "HA service name (e.g. 'turn_on', 'set_temperature'). Required for 'call_service'.",
                },
                "entity_id": {
                    "type": "string",
                    "description": "Target entity ID (e.g. 'light.living_room'). Required for 'get_state'; optional for 'call_service'.",
                },
                "area": {
                    "type": "string",
                    "description": "Area/room name filter for 'list_entities' (matches against friendly names).",
                },
                "data": {
                    "description": "Extra service-call data -- a JSON object or a JSON-encoded string.",
                },
            },
            "required": ["operation"],
        })
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        matches!(
            input.get("operation").and_then(Value::as_str),
            Some("list_entities") | Some("get_state") | Some("list_services")
        )
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let operation = match opt_str(&input, "operation") {
            Some(s) => s.to_string(),
            None => return err_result("Missing required parameter: 'operation'"),
        };

        match operation.as_str() {
            "list_entities" => {
                let domain = opt_str(&input, "domain");
                let area = opt_str(&input, "area");
                if let Some(d) = domain
                    && let Err(e) = validate_service_name(d, "domain")
                {
                    return err_result(e);
                }
                match self.backend.list_entities(domain, area).await {
                    HaOutcome::Ok(v) => ok_result(v),
                    HaOutcome::Err(e) => err_result(format!("Failed to list entities: {e}")),
                }
            }
            "get_state" => {
                let entity_id = match opt_str(&input, "entity_id") {
                    Some(s) => s,
                    None => return err_result("Missing required parameter: 'entity_id'"),
                };
                if let Err(e) = validate_entity_id(entity_id) {
                    return err_result(e);
                }
                match self.backend.get_state(entity_id).await {
                    HaOutcome::Ok(v) => ok_result(v),
                    HaOutcome::Err(e) => {
                        err_result(format!("Failed to get state for {entity_id}: {e}"))
                    }
                }
            }
            "list_services" => {
                let domain = opt_str(&input, "domain");
                if let Some(d) = domain
                    && let Err(e) = validate_service_name(d, "domain")
                {
                    return err_result(e);
                }
                match self.backend.list_services(domain).await {
                    HaOutcome::Ok(v) => ok_result(v),
                    HaOutcome::Err(e) => err_result(format!("Failed to list services: {e}")),
                }
            }
            "call_service" => {
                let domain = match opt_str(&input, "domain") {
                    Some(s) => s,
                    None => {
                        return err_result("Missing required parameters: 'domain' and 'service'");
                    }
                };
                let service = match opt_str(&input, "service") {
                    Some(s) => s,
                    None => {
                        return err_result("Missing required parameters: 'domain' and 'service'");
                    }
                };
                if let Err(e) = validate_service_name(domain, "domain") {
                    return err_result(e);
                }
                if let Err(e) = validate_service_name(service, "service") {
                    return err_result(e);
                }
                if let Err(e) = check_service_domain_allowed(domain) {
                    return err_result(e);
                }
                let entity_id = opt_str(&input, "entity_id");
                if let Some(eid) = entity_id
                    && let Err(e) = validate_entity_id(eid)
                {
                    return err_result(e);
                }
                let data = match parse_data_arg(input.get("data")) {
                    Ok(d) => d,
                    Err(e) => return err_result(e),
                };
                match self
                    .backend
                    .call_service(domain, service, entity_id, data.as_ref())
                    .await
                {
                    HaOutcome::Ok(v) => ok_result(v),
                    HaOutcome::Err(e) => {
                        err_result(format!("Failed to call {domain}.{service}: {e}"))
                    }
                }
            }
            other => err_result(format!(
                "Unknown operation: {other:?} (expected one of: list_entities, get_state, \
                 list_services, call_service)"
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn must_exec(t: &HomeAssistantTool, input: Value) -> ToolResult {
        futures::executor::block_on(t.execute(input))
    }

    #[test]
    fn validate_entity_id_accepts_canonical_forms() {
        assert!(validate_entity_id("light.living_room").is_ok());
        assert!(validate_entity_id("sensor.temperature_1").is_ok());
        assert!(validate_entity_id("_underscore.start").is_ok());
        assert!(validate_entity_id("binary_sensor.front_door").is_ok());
    }

    #[test]
    fn validate_entity_id_rejects_traversal_and_bad_shape() {
        for bad in [
            "Light.LIVING",
            "light",
            "light.",
            ".living_room",
            "light/living",
            "light.living/../etc/passwd",
            "light..living",
            "1light.x",
            "",
        ] {
            assert!(validate_entity_id(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn validate_service_name_rejects_traversal() {
        for bad in [
            "shell_command/../light",
            "light/turn_on",
            "Light",
            "1light",
            "",
            "with-dash",
            "with.dot",
        ] {
            assert!(
                validate_service_name(bad, "domain").is_err(),
                "should reject {bad:?}"
            );
        }
        assert!(validate_service_name("light", "domain").is_ok());
        assert!(validate_service_name("turn_on", "service").is_ok());
    }

    #[test]
    fn blocked_domain_check_covers_full_list() {
        for d in BLOCKED_SERVICE_DOMAINS {
            assert!(
                check_service_domain_allowed(d).is_err(),
                "domain {d:?} must be blocked"
            );
        }
        assert!(check_service_domain_allowed("light").is_ok());
        assert!(check_service_domain_allowed("climate").is_ok());
    }

    #[test]
    fn null_backend_fails_loudly_for_every_op() {
        let tool = HomeAssistantTool::default();
        for input in [
            json!({"operation": "list_entities"}),
            json!({"operation": "get_state", "entity_id": "light.kitchen"}),
            json!({"operation": "list_services"}),
            json!({"operation": "call_service", "domain": "light", "service": "turn_on"}),
        ] {
            let r = must_exec(&tool, input.clone());
            assert!(r.is_error, "op {input} should be an error");
            assert!(
                r.content.contains("No Home Assistant backend configured"),
                "got: {}",
                r.content
            );
        }
    }

    #[test]
    fn list_entities_happy_path() {
        let backend = Arc::new(
            CapturingHomeAssistantBackend::new().with_list_entities_response(json!({
                "count": 1,
                "entities": [{"entity_id": "light.kitchen", "state": "on", "friendly_name": "Kitchen"}]
            })),
        );
        let tool = HomeAssistantTool::new(backend.clone());
        let r = must_exec(
            &tool,
            json!({"operation": "list_entities", "domain": "light", "area": "Kitchen"}),
        );
        assert!(!r.is_error, "got error: {}", r.content);
        let parsed: Value = serde_json::from_str(&r.content).unwrap();
        assert_eq!(parsed["success"], json!(true));
        assert_eq!(parsed["result"]["count"], json!(1));

        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        match &snap[0] {
            CapturedHaCall::ListEntities { domain, area } => {
                assert_eq!(domain.as_deref(), Some("light"));
                assert_eq!(area.as_deref(), Some("Kitchen"));
            }
            other => panic!("unexpected capture: {other:?}"),
        }
    }

    #[test]
    fn get_state_requires_valid_entity_id() {
        let backend = Arc::new(CapturingHomeAssistantBackend::new());
        let tool = HomeAssistantTool::new(backend.clone());

        let r = must_exec(&tool, json!({"operation": "get_state"}));
        assert!(r.is_error);
        assert!(r.content.contains("entity_id"));

        let r = must_exec(&tool, json!({"operation": "get_state", "entity_id": "BAD"}));
        assert!(r.is_error);
        assert!(r.content.contains("Invalid entity_id"));

        assert!(backend.snapshot().is_empty());

        let r = must_exec(
            &tool,
            json!({"operation": "get_state", "entity_id": "light.kitchen"}),
        );
        assert!(!r.is_error);
        assert_eq!(backend.snapshot().len(), 1);
    }

    #[test]
    fn call_service_blocks_high_risk_domains() {
        let backend = Arc::new(CapturingHomeAssistantBackend::new());
        let tool = HomeAssistantTool::new(backend.clone());
        for bad_domain in BLOCKED_SERVICE_DOMAINS {
            let r = must_exec(
                &tool,
                json!({
                    "operation": "call_service",
                    "domain": bad_domain,
                    "service": "run",
                }),
            );
            assert!(r.is_error, "domain {bad_domain} must be blocked");
            assert!(
                r.content.contains("blocked for security"),
                "got: {}",
                r.content
            );
        }
        assert!(backend.snapshot().is_empty());
    }

    #[test]
    fn call_service_rejects_traversal_in_domain_or_service() {
        let backend = Arc::new(CapturingHomeAssistantBackend::new());
        let tool = HomeAssistantTool::new(backend.clone());

        let r = must_exec(
            &tool,
            json!({
                "operation": "call_service",
                "domain": "shell_command/../light",
                "service": "turn_on",
            }),
        );
        assert!(r.is_error);
        assert!(r.content.contains("Invalid domain"));

        let r = must_exec(
            &tool,
            json!({
                "operation": "call_service",
                "domain": "light",
                "service": "../../api/config",
            }),
        );
        assert!(r.is_error);
        assert!(r.content.contains("Invalid service"));

        let r = must_exec(
            &tool,
            json!({
                "operation": "call_service",
                "domain": "light",
                "service": "turn_on",
                "entity_id": "BAD",
            }),
        );
        assert!(r.is_error);
        assert!(r.content.contains("Invalid entity_id"));

        assert!(backend.snapshot().is_empty());
    }

    #[test]
    fn call_service_happy_path_with_data_object_and_string() {
        let backend = Arc::new(
            CapturingHomeAssistantBackend::new().with_call_service_response(json!({
                "success": true,
                "service": "light.turn_on",
                "affected_entities": [{"entity_id": "light.kitchen", "state": "on"}],
            })),
        );
        let tool = HomeAssistantTool::new(backend.clone());

        let r = must_exec(
            &tool,
            json!({
                "operation": "call_service",
                "domain": "light",
                "service": "turn_on",
                "entity_id": "light.kitchen",
                "data": {"brightness": 255, "color_name": "blue"},
            }),
        );
        assert!(!r.is_error, "got: {}", r.content);

        let r = must_exec(
            &tool,
            json!({
                "operation": "call_service",
                "domain": "light",
                "service": "turn_off",
                "entity_id": "light.kitchen",
                "data": "{\"transition\": 5}",
            }),
        );
        assert!(!r.is_error, "got: {}", r.content);

        let snap = backend.snapshot();
        assert_eq!(snap.len(), 2);
        match &snap[0] {
            CapturedHaCall::CallService {
                domain,
                service,
                entity_id,
                data,
            } => {
                assert_eq!(domain, "light");
                assert_eq!(service, "turn_on");
                assert_eq!(entity_id.as_deref(), Some("light.kitchen"));
                assert_eq!(data.as_ref().unwrap()["brightness"], json!(255));
            }
            other => panic!("unexpected capture: {other:?}"),
        }
        match &snap[1] {
            CapturedHaCall::CallService { service, data, .. } => {
                assert_eq!(service, "turn_off");
                assert_eq!(data.as_ref().unwrap()["transition"], json!(5));
            }
            other => panic!("unexpected capture: {other:?}"),
        }
    }

    #[test]
    fn call_service_rejects_malformed_data_string() {
        let backend = Arc::new(CapturingHomeAssistantBackend::new());
        let tool = HomeAssistantTool::new(backend.clone());
        let r = must_exec(
            &tool,
            json!({
                "operation": "call_service",
                "domain": "light",
                "service": "turn_on",
                "data": "not json {",
            }),
        );
        assert!(r.is_error);
        assert!(r.content.contains("Invalid JSON string"));
        assert!(backend.snapshot().is_empty());
    }

    #[test]
    fn call_service_accepts_empty_or_missing_data() {
        let backend = Arc::new(CapturingHomeAssistantBackend::new());
        let tool = HomeAssistantTool::new(backend.clone());

        let r = must_exec(
            &tool,
            json!({"operation": "call_service", "domain": "light", "service": "turn_on"}),
        );
        assert!(!r.is_error);

        let r = must_exec(
            &tool,
            json!({
                "operation": "call_service",
                "domain": "light",
                "service": "turn_on",
                "data": "  ",
            }),
        );
        assert!(!r.is_error);

        let snap = backend.snapshot();
        assert_eq!(snap.len(), 2);
        for c in &snap {
            match c {
                CapturedHaCall::CallService { data, .. } => assert!(data.is_none()),
                other => panic!("unexpected: {other:?}"),
            }
        }
    }

    #[test]
    fn unknown_operation_returns_error() {
        let tool = HomeAssistantTool::new(Arc::new(CapturingHomeAssistantBackend::new()));
        let r = must_exec(&tool, json!({"operation": "nuke_everything"}));
        assert!(r.is_error);
        assert!(r.content.contains("Unknown operation"));
    }

    #[test]
    fn missing_operation_returns_error() {
        let tool = HomeAssistantTool::default();
        let r = must_exec(&tool, json!({}));
        assert!(r.is_error);
        assert!(r.content.contains("operation"));
    }

    #[test]
    fn list_entities_rejects_bad_domain_format() {
        let backend = Arc::new(CapturingHomeAssistantBackend::new());
        let tool = HomeAssistantTool::new(backend.clone());
        let r = must_exec(
            &tool,
            json!({"operation": "list_entities", "domain": "Light/../"}),
        );
        assert!(r.is_error);
        assert!(r.content.contains("Invalid domain"));
        assert!(backend.snapshot().is_empty());
    }

    #[test]
    fn concurrency_safety_is_per_operation() {
        let tool = HomeAssistantTool::default();
        assert!(tool.is_concurrency_safe(&json!({"operation": "list_entities"})));
        assert!(tool.is_concurrency_safe(&json!({"operation": "get_state"})));
        assert!(tool.is_concurrency_safe(&json!({"operation": "list_services"})));
        assert!(!tool.is_concurrency_safe(&json!({"operation": "call_service"})));
    }

    #[test]
    fn tool_registers_in_registry() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        // v0.9.0 W1: registry now skips tools whose `is_available()`
        // returns false, so we must wire a real backend for this test.
        reg.register(Box::new(HomeAssistantTool::new(Arc::new(
            CapturingHomeAssistantBackend::new(),
        ))));
        let defs = reg.to_tool_defs();
        let found = defs.iter().find(|d| d.name == "homeassistant");
        assert!(found.is_some(), "homeassistant must be present in registry");
        let def = found.unwrap();
        let schema = &def.input_schema;
        let required = schema["required"].as_array().expect("required array");
        let required_strs: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
        assert!(required_strs.contains(&"operation"));
    }

    // --- v0.9.0 W1 backend gate ---

    #[test]
    fn default_is_hidden_when_no_backend_wired() {
        let tool = HomeAssistantTool::default();
        assert!(
            !tool.is_available(),
            "Default::default() must yield backend_configured == false"
        );
    }

    #[test]
    fn with_real_backend_is_available() {
        let tool = HomeAssistantTool::new(Arc::new(CapturingHomeAssistantBackend::new()));
        assert!(
            tool.is_available(),
            "new(backend) must yield backend_configured == true"
        );
    }
}
