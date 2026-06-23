//! T4 (v0.6.3 Tier 2B) — GitLab REST API v4 operations tool.
//!
//! Ported from the prior Wayland Python engine and modeled
//! on `discord_tool.rs`. The Python original talks to GitLab's REST API
//! directly (an HTTP client + a `PRIVATE-TOKEN` header). Wayland's
//! engine MUST NOT initiate HTTP from inside `wcore-tools` — HTTP is a
//! `wcore-providers` / plugin / host concern (the crate ships no
//! `reqwest`/`hyper` dependency by design). This port therefore covers
//! the **dispatch surface only**: schema, action manifest, per-action
//! required-parameter validation, base-URL + project-id resolution into
//! a typed `GitLabRequest`, and a pluggable `GitLabBackend` boundary
//! that the host wires to a real REST client (typically wrapping the
//! shared `wcore_providers::http_client`) at construction time.
//!
//! Without a backend bound, `execute()` returns a structured error
//! ("No GitLab backend configured ...") rather than a silent stub —
//! honoring the NO-STUBS contract.
//!
//! ## Operations
//!
//! * `get_issue`  — read a single issue (read).
//! * `get_mr`     — read a single merge request (read).
//! * `get_file`   — read raw file contents at a ref (read).
//! * `create_note` — post a note/comment on an issue or MR (write).
//!
//! ## Auth + base URL
//!
//! Auth is a GitLab personal/project access token sent as a
//! `PRIVATE-TOKEN` header. The token is resolved by the host (config or
//! environment); this module never reads the environment itself — it
//! threads a host-supplied token through to the backend. The base URL
//! defaults to `https://gitlab.com/api/v4` and is configurable for
//! self-hosted GitLab instances via [`GitLabTool::with_base_url`].
//!
//! The request URL, header set, and HTTP method are computed here as
//! pure data (see [`GitLabRequest`]) so the host backend only has to
//! perform the transport, and tests can assert the wire shape without
//! ever touching the network.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Default GitLab REST API base URL (GitLab.com SaaS).
pub const DEFAULT_GITLAB_BASE_URL: &str = "https://gitlab.com/api/v4";

/// Canonical GitLab action set. Order is preserved in the schema enum
/// and the description manifest so the model sees a stable surface.
pub const GITLAB_ACTIONS: &[&str] = &["get_issue", "get_mr", "get_file", "create_note"];

/// Action manifest entry — `(name, signature, one-line description)`.
pub const GITLAB_ACTION_MANIFEST: &[(&str, &str, &str)] = &[
    (
        "get_issue",
        "(project_id, issue_iid)",
        "read a single issue by its project-scoped IID",
    ),
    (
        "get_mr",
        "(project_id, mr_iid)",
        "read a single merge request by its project-scoped IID",
    ),
    (
        "get_file",
        "(project_id, file_path[, ref])",
        "read raw file contents at a branch/tag/commit (ref defaults to HEAD)",
    ),
    (
        "create_note",
        "(project_id, noteable_type, noteable_iid, body)",
        "post a note/comment on an issue or merge request",
    ),
];

/// HTTP method for a GitLab request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
}

impl HttpMethod {
    /// Wire name (`"GET"` / `"POST"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
        }
    }
}

/// Percent-encode a GitLab project identifier for use in a URL path
/// segment. GitLab accepts either a numeric project ID (`12345`) or the
/// URL-encoded `namespace/project` path (`group%2Fsubgroup%2Fproject`).
///
/// This encodes every character that is not unreserved per RFC 3986
/// (`A-Z a-z 0-9 - . _ ~`). In particular `/` becomes `%2F`, which is
/// what GitLab's "URL-encoded path of the project" API contract
/// requires. A bare numeric ID passes through unchanged.
pub fn encode_project_id(project_id: &str) -> String {
    let mut out = String::with_capacity(project_id.len() * 3);
    for byte in project_id.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(*byte as char);
            }
            _ => {
                out.push('%');
                out.push_str(&format!("{byte:02X}"));
            }
        }
    }
    out
}

/// A fully-resolved GitLab REST request. The host backend performs the
/// transport; everything else (URL, method, auth header, optional JSON
/// body) is computed by this module so the wire shape is deterministic
/// and testable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitLabRequest {
    /// The action name this request implements (e.g. `"get_issue"`).
    pub action: String,
    /// HTTP method.
    pub method: HttpMethod,
    /// Fully-qualified request URL (base URL + path + query).
    pub url: String,
    /// The GitLab access token to send as the `PRIVATE-TOKEN` header.
    /// Empty string means "no token resolved" — the backend should
    /// still send the request unauthenticated and let GitLab 401.
    pub private_token: String,
    /// Optional JSON request body (set for write actions only).
    pub body: Option<Value>,
}

impl GitLabRequest {
    /// The header set this request must be sent with. Returns
    /// `(name, value)` pairs. `PRIVATE-TOKEN` is always present (even
    /// when empty) so backends have a single, predictable contract.
    pub fn headers(&self) -> Vec<(&'static str, String)> {
        let mut headers = vec![("PRIVATE-TOKEN", self.private_token.clone())];
        if self.body.is_some() {
            headers.push(("Content-Type", "application/json".to_string()));
        }
        headers
    }
}

/// Outcome of a backend dispatch.
#[derive(Debug, Clone)]
pub enum GitLabOutcome {
    /// Success — `payload` is the parsed JSON GitLab returned (for
    /// `get_file` the backend should wrap the raw file body in a JSON
    /// object, e.g. `{"file_path": "...", "content": "..."}`).
    Ok { payload: Value },
    /// Any failure path (network error, non-2xx status, auth missing).
    /// `status_code` is the HTTP status when one was received.
    Err {
        message: String,
        status_code: Option<u16>,
    },
}

/// Host-supplied GitLab backend. The engine never speaks HTTP; the host
/// implements this trait (typically wrapping `wcore_providers::http_client`)
/// and binds it at registration time. The backend receives a
/// fully-resolved [`GitLabRequest`] and only has to perform the
/// transport + parse the response into a [`GitLabOutcome`].
#[async_trait]
pub trait GitLabBackend: Send + Sync {
    async fn dispatch(&self, request: &GitLabRequest) -> GitLabOutcome;
}

/// Default backend — every `dispatch()` fails loudly so the tool never
/// silently appears to succeed (NO-STUBS guarantee).
pub struct NullGitLabBackend;

#[async_trait]
impl GitLabBackend for NullGitLabBackend {
    async fn dispatch(&self, request: &GitLabRequest) -> GitLabOutcome {
        GitLabOutcome::Err {
            message: format!(
                "No GitLab backend configured for action '{}'. Wire a GitLabBackend \
                 implementation (typically over wcore_providers::http_client) when \
                 constructing GitLabTool.",
                request.action
            ),
            status_code: None,
        }
    }
}

/// In-memory backend that records every dispatched request for test
/// assertions and returns a canned JSON payload.
pub struct CapturingGitLabBackend {
    response: Value,
    pub captured: parking_lot::Mutex<Vec<GitLabRequest>>,
}

impl CapturingGitLabBackend {
    pub fn new(canned_response: Value) -> Self {
        Self {
            response: canned_response,
            captured: parking_lot::Mutex::new(Vec::new()),
        }
    }

    pub fn snapshot(&self) -> Vec<GitLabRequest> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl GitLabBackend for CapturingGitLabBackend {
    async fn dispatch(&self, request: &GitLabRequest) -> GitLabOutcome {
        self.captured.lock().push(request.clone());
        GitLabOutcome::Ok {
            payload: self.response.clone(),
        }
    }
}

/// Required-parameter manifest per action.
fn required_params_for(action: &str) -> &'static [&'static str] {
    match action {
        "get_issue" => &["project_id", "issue_iid"],
        "get_mr" => &["project_id", "mr_iid"],
        "get_file" => &["project_id", "file_path"],
        "create_note" => &["project_id", "noteable_type", "noteable_iid", "body"],
        _ => &[],
    }
}

fn str_field(input: &Value, key: &str) -> String {
    input
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string()
}

/// `gitlab_api` tool — Wayland engine port of `gitlab_tool.py`.
pub struct GitLabTool {
    backend: Arc<dyn GitLabBackend>,
    /// REST API base URL, no trailing slash. Defaults to GitLab.com.
    base_url: String,
    /// GitLab access token sent as `PRIVATE-TOKEN`. Host-resolved.
    private_token: String,
    description: String,
    schema: JsonSchema,
}

impl Default for GitLabTool {
    fn default() -> Self {
        Self::new(Arc::new(NullGitLabBackend))
    }
}

impl GitLabTool {
    /// Construct with the default GitLab.com base URL and no token.
    pub fn new(backend: Arc<dyn GitLabBackend>) -> Self {
        Self {
            backend,
            base_url: DEFAULT_GITLAB_BASE_URL.to_string(),
            private_token: String::new(),
            description: build_description(),
            schema: build_schema(),
        }
    }

    /// Override the REST API base URL for a self-hosted GitLab instance
    /// (e.g. `https://gitlab.example.com/api/v4`). A trailing slash is
    /// stripped so URL composition stays canonical.
    pub fn with_base_url(mut self, base_url: impl Into<String>) -> Self {
        let url = base_url.into();
        self.base_url = url.trim_end_matches('/').to_string();
        self
    }

    /// Set the GitLab access token sent as the `PRIVATE-TOKEN` header.
    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.private_token = token.into();
        self
    }

    /// Build the typed request for a parsed action. Returns the
    /// rejection reason on validation failure.
    fn build_request(&self, action: &str, input: &Value) -> Result<GitLabRequest, String> {
        let project_id = str_field(input, "project_id");
        let encoded_project = encode_project_id(&project_id);
        match action {
            "get_issue" => {
                let issue_iid = str_field(input, "issue_iid");
                Ok(GitLabRequest {
                    action: action.to_string(),
                    method: HttpMethod::Get,
                    url: format!(
                        "{}/projects/{}/issues/{}",
                        self.base_url, encoded_project, issue_iid
                    ),
                    private_token: self.private_token.clone(),
                    body: None,
                })
            }
            "get_mr" => {
                let mr_iid = str_field(input, "mr_iid");
                Ok(GitLabRequest {
                    action: action.to_string(),
                    method: HttpMethod::Get,
                    url: format!(
                        "{}/projects/{}/merge_requests/{}",
                        self.base_url, encoded_project, mr_iid
                    ),
                    private_token: self.private_token.clone(),
                    body: None,
                })
            }
            "get_file" => {
                let file_path = str_field(input, "file_path");
                // GitLab's "get raw file" endpoint requires the file
                // path itself to be URL-encoded (slashes -> %2F). `ref`
                // defaults to HEAD when the caller omits it.
                let encoded_file = encode_project_id(&file_path);
                let git_ref = {
                    let r = str_field(input, "ref");
                    if r.is_empty() { "HEAD".to_string() } else { r }
                };
                Ok(GitLabRequest {
                    action: action.to_string(),
                    method: HttpMethod::Get,
                    url: format!(
                        "{}/projects/{}/repository/files/{}/raw?ref={}",
                        self.base_url,
                        encoded_project,
                        encoded_file,
                        encode_project_id(&git_ref),
                    ),
                    private_token: self.private_token.clone(),
                    body: None,
                })
            }
            "create_note" => {
                let noteable_type = str_field(input, "noteable_type").to_ascii_lowercase();
                let segment = match noteable_type.as_str() {
                    "issue" => "issues",
                    "merge_request" => "merge_requests",
                    other => {
                        return Err(format!(
                            "noteable_type must be 'issue' or 'merge_request', got '{other}'"
                        ));
                    }
                };
                let noteable_iid = str_field(input, "noteable_iid");
                let body = str_field(input, "body");
                Ok(GitLabRequest {
                    action: action.to_string(),
                    method: HttpMethod::Post,
                    url: format!(
                        "{}/projects/{}/{}/{}/notes",
                        self.base_url, encoded_project, segment, noteable_iid
                    ),
                    private_token: self.private_token.clone(),
                    body: Some(json!({ "body": body })),
                })
            }
            other => Err(format!("Unknown action: {other}")),
        }
    }
}

fn build_description() -> String {
    let manifest: Vec<String> = GITLAB_ACTION_MANIFEST
        .iter()
        .map(|(name, sig, desc)| format!("  {name}{sig}  — {desc}"))
        .collect();
    format!(
        "Query and comment on a GitLab project via the GitLab REST API v4.\n\n\
         Available actions:\n{}\n\n\
         project_id is either a numeric project ID or the namespace/project path \
         (e.g. 'group/subgroup/project'); it is URL-encoded automatically. \
         Works against GitLab.com or a self-hosted instance.",
        manifest.join("\n")
    )
}

fn build_schema() -> JsonSchema {
    json!({
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": GITLAB_ACTIONS,
            },
            "project_id": {
                "type": "string",
                "description": "Numeric project ID or 'namespace/project' path."
            },
            "issue_iid": {
                "type": "string",
                "description": "Project-scoped issue IID (get_issue)."
            },
            "mr_iid": {
                "type": "string",
                "description": "Project-scoped merge request IID (get_mr)."
            },
            "file_path": {
                "type": "string",
                "description": "Repository file path (get_file)."
            },
            "ref": {
                "type": "string",
                "description": "Branch, tag, or commit SHA for get_file (default HEAD)."
            },
            "noteable_type": {
                "type": "string",
                "enum": ["issue", "merge_request"],
                "description": "What to comment on (create_note)."
            },
            "noteable_iid": {
                "type": "string",
                "description": "IID of the issue or merge request to comment on (create_note)."
            },
            "body": {
                "type": "string",
                "description": "Note/comment text (create_note)."
            }
        },
        "required": ["action"]
    })
}

#[async_trait]
impl Tool for GitLabTool {
    fn name(&self) -> &str {
        "gitlab_api"
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn input_schema(&self) -> JsonSchema {
        self.schema.clone()
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        // Read actions are concurrency-safe; create_note mutates.
        let action = input.get("action").and_then(Value::as_str).unwrap_or("");
        matches!(action, "get_issue" | "get_mr" | "get_file")
    }

    fn category(&self) -> ToolCategory {
        // Includes a mutating action (create_note). Categorize as Edit
        // so hosts that gate side-effecting tools catch this tool too.
        ToolCategory::Edit
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let action = match input.get("action").and_then(Value::as_str) {
            Some(s) if !s.trim().is_empty() => s.trim().to_string(),
            _ => {
                return ToolResult {
                    content: json!({"error": "Missing required parameter: 'action'"}).to_string(),
                    is_error: true,
                };
            }
        };

        if !GITLAB_ACTIONS.contains(&action.as_str()) {
            return ToolResult {
                content: json!({
                    "error": format!("Unknown action: {action}"),
                    "available_actions": GITLAB_ACTIONS,
                })
                .to_string(),
                is_error: true,
            };
        }

        let missing: Vec<&str> = required_params_for(&action)
            .iter()
            .copied()
            .filter(|p| str_field(&input, p).is_empty())
            .collect();
        if !missing.is_empty() {
            return ToolResult {
                content: json!({
                    "error": format!(
                        "Missing required parameters for '{}': {}",
                        action,
                        missing.join(", ")
                    )
                })
                .to_string(),
                is_error: true,
            };
        }

        let request = match self.build_request(&action, &input) {
            Ok(r) => r,
            Err(e) => {
                return ToolResult {
                    content: json!({"error": e}).to_string(),
                    is_error: true,
                };
            }
        };

        match self.backend.dispatch(&request).await {
            GitLabOutcome::Ok { payload } => ToolResult {
                content: payload.to_string(),
                is_error: false,
            },
            GitLabOutcome::Err {
                message,
                status_code,
            } => {
                let mut err = serde_json::Map::new();
                err.insert("error".into(), json!(message));
                if let Some(code) = status_code {
                    err.insert("status_code".into(), json!(code));
                }
                ToolResult {
                    content: Value::Object(err).to_string(),
                    is_error: true,
                }
            }
        }
    }
}

/// Register a single `gitlab_api` tool into the supplied registry,
/// bound to `backend`. The host wires `base_url` (for self-hosted) and
/// `token` from its config/environment.
pub fn register_gitlab_tool(
    registry: &mut crate::registry::ToolRegistry,
    backend: Arc<dyn GitLabBackend>,
    base_url: Option<String>,
    token: Option<String>,
) {
    let mut tool = GitLabTool::new(backend);
    if let Some(url) = base_url {
        tool = tool.with_base_url(url);
    }
    if let Some(t) = token {
        tool = tool.with_token(t);
    }
    registry.register(Box::new(tool));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(t: &GitLabTool, args: Value) -> ToolResult {
        futures::executor::block_on(t.execute(args))
    }

    fn header_value(req: &GitLabRequest, name: &str) -> Option<String> {
        req.headers()
            .into_iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| v)
    }

    #[test]
    fn get_issue_builds_url_and_private_token_header() {
        let backend = Arc::new(CapturingGitLabBackend::new(json!({"iid": 7})));
        let tool = GitLabTool::new(backend.clone()).with_token("glpat-secret");
        let res = run(
            &tool,
            json!({"action": "get_issue", "project_id": "42", "issue_iid": "7"}),
        );
        assert!(!res.is_error, "expected ok, got: {}", res.content);
        let calls = backend.snapshot();
        assert_eq!(calls.len(), 1);
        assert_eq!(
            calls[0].url,
            "https://gitlab.com/api/v4/projects/42/issues/7"
        );
        assert_eq!(calls[0].method, HttpMethod::Get);
        assert_eq!(
            header_value(&calls[0], "PRIVATE-TOKEN"),
            Some("glpat-secret".to_string())
        );
        // Read action carries no body header.
        assert!(header_value(&calls[0], "Content-Type").is_none());
        assert!(res.content.contains("\"iid\":7"));
    }

    #[test]
    fn self_hosted_base_url_is_honored_for_get_mr() {
        let backend = Arc::new(CapturingGitLabBackend::new(json!({"iid": 3})));
        let tool = GitLabTool::new(backend.clone())
            // trailing slash must be stripped
            .with_base_url("https://gitlab.example.com/api/v4/")
            .with_token("tok");
        let res = run(
            &tool,
            json!({"action": "get_mr", "project_id": "99", "mr_iid": "3"}),
        );
        assert!(!res.is_error, "got: {}", res.content);
        let calls = backend.snapshot();
        assert_eq!(
            calls[0].url,
            "https://gitlab.example.com/api/v4/projects/99/merge_requests/3"
        );
        assert_eq!(
            header_value(&calls[0], "PRIVATE-TOKEN"),
            Some("tok".to_string())
        );
    }

    #[test]
    fn project_id_path_is_url_encoded() {
        // namespace/project path -> %2F-encoded segment.
        assert_eq!(
            encode_project_id("group/subgroup/my-project"),
            "group%2Fsubgroup%2Fmy-project"
        );
        // numeric ID passes through unchanged.
        assert_eq!(encode_project_id("12345"), "12345");

        let backend = Arc::new(CapturingGitLabBackend::new(json!({"ok": true})));
        let tool = GitLabTool::new(backend.clone());
        let res = run(
            &tool,
            json!({
                "action": "get_file",
                "project_id": "group/sub/proj",
                "file_path": "src/main.rs",
                "ref": "feature/x"
            }),
        );
        assert!(!res.is_error, "got: {}", res.content);
        let calls = backend.snapshot();
        // project, file path, and ref are all percent-encoded.
        assert_eq!(
            calls[0].url,
            "https://gitlab.com/api/v4/projects/group%2Fsub%2Fproj\
             /repository/files/src%2Fmain.rs/raw?ref=feature%2Fx"
        );
    }

    #[test]
    fn create_note_posts_json_body_with_content_type_header() {
        let backend = Arc::new(CapturingGitLabBackend::new(json!({"id": 1, "body": "hi"})));
        let tool = GitLabTool::new(backend.clone()).with_token("glpat-x");
        let res = run(
            &tool,
            json!({
                "action": "create_note",
                "project_id": "7",
                "noteable_type": "merge_request",
                "noteable_iid": "12",
                "body": "looks good"
            }),
        );
        assert!(!res.is_error, "got: {}", res.content);
        let calls = backend.snapshot();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].method, HttpMethod::Post);
        assert_eq!(
            calls[0].url,
            "https://gitlab.com/api/v4/projects/7/merge_requests/12/notes"
        );
        assert_eq!(
            calls[0].body,
            Some(json!({"body": "looks good"})),
            "request body must carry the note text"
        );
        assert_eq!(
            header_value(&calls[0], "Content-Type"),
            Some("application/json".to_string()),
            "write requests must declare a JSON content type"
        );
        assert_eq!(
            header_value(&calls[0], "PRIVATE-TOKEN"),
            Some("glpat-x".to_string())
        );
    }

    #[test]
    fn response_payload_is_parsed_from_fixture() {
        // A realistic GitLab issue fixture payload.
        let fixture = json!({
            "id": 76,
            "iid": 6,
            "project_id": 42,
            "title": "Fix the thing",
            "state": "opened",
            "author": {"username": "octocat"}
        });
        let backend = Arc::new(CapturingGitLabBackend::new(fixture.clone()));
        let tool = GitLabTool::new(backend);
        let res = run(
            &tool,
            json!({"action": "get_issue", "project_id": "42", "issue_iid": "6"}),
        );
        assert!(!res.is_error);
        let parsed: Value = serde_json::from_str(&res.content).expect("valid JSON");
        assert_eq!(parsed, fixture);
        assert_eq!(parsed["title"], json!("Fix the thing"));
        assert_eq!(parsed["state"], json!("opened"));
    }

    #[test]
    fn backend_error_surfaces_status_code() {
        struct ErrBackend;
        #[async_trait]
        impl GitLabBackend for ErrBackend {
            async fn dispatch(&self, _req: &GitLabRequest) -> GitLabOutcome {
                GitLabOutcome::Err {
                    message: "404 Project Not Found".to_string(),
                    status_code: Some(404),
                }
            }
        }
        let tool = GitLabTool::new(Arc::new(ErrBackend));
        let res = run(
            &tool,
            json!({"action": "get_issue", "project_id": "x", "issue_iid": "1"}),
        );
        assert!(res.is_error);
        let parsed: Value = serde_json::from_str(&res.content).expect("valid JSON");
        assert_eq!(parsed["status_code"], json!(404));
        assert!(
            parsed["error"].as_str().unwrap().contains("404"),
            "got: {}",
            res.content
        );
    }

    #[test]
    fn null_backend_fails_loud_no_silent_stub() {
        let tool = GitLabTool::default();
        let res = run(
            &tool,
            json!({"action": "get_issue", "project_id": "1", "issue_iid": "1"}),
        );
        assert!(res.is_error);
        assert!(
            res.content.contains("No GitLab backend configured"),
            "expected fail-loud, got: {}",
            res.content
        );
    }

    #[test]
    fn input_schema_validation_rejects_missing_and_unknown() {
        let backend = Arc::new(CapturingGitLabBackend::new(json!({})));
        let tool = GitLabTool::new(backend.clone());

        // Missing action.
        let res = run(&tool, json!({}));
        assert!(res.is_error);
        assert!(res.content.contains("Missing required parameter: 'action'"));

        // Unknown action.
        let res = run(&tool, json!({"action": "delete_repo"}));
        assert!(res.is_error);
        assert!(res.content.contains("Unknown action"));
        assert!(res.content.contains("get_issue"));

        // Missing required params short-circuits before the backend.
        let res = run(&tool, json!({"action": "get_issue"}));
        assert!(res.is_error);
        assert!(res.content.contains("Missing required parameters"));
        assert!(res.content.contains("project_id"));
        assert!(res.content.contains("issue_iid"));

        // Invalid noteable_type is rejected.
        let res = run(
            &tool,
            json!({
                "action": "create_note",
                "project_id": "1",
                "noteable_type": "snippet",
                "noteable_iid": "2",
                "body": "x"
            }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("noteable_type must be"));

        assert!(
            backend.snapshot().is_empty(),
            "backend must never be called on invalid input"
        );

        // Schema declares `action` required.
        let schema = tool.input_schema();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(required, ["action"]);
    }

    #[test]
    fn register_gitlab_tool_populates_registry() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        let backend: Arc<dyn GitLabBackend> = Arc::new(NullGitLabBackend);
        register_gitlab_tool(
            &mut reg,
            backend,
            Some("https://gitlab.example.com/api/v4".to_string()),
            Some("tok".to_string()),
        );
        let names = reg.tool_names();
        assert!(
            names.contains(&"gitlab_api".to_string()),
            "found: {names:?}"
        );
    }

    #[test]
    fn concurrency_safety_distinguishes_read_vs_mutate() {
        let tool = GitLabTool::default();
        assert!(tool.is_concurrency_safe(&json!({"action": "get_issue"})));
        assert!(tool.is_concurrency_safe(&json!({"action": "get_mr"})));
        assert!(tool.is_concurrency_safe(&json!({"action": "get_file"})));
        assert!(!tool.is_concurrency_safe(&json!({"action": "create_note"})));
    }
}
