//! T6 (v0.6.3 Tier 2B) — Notion REST API operations tool.
//!
//! Mirrors the established `wcore-tools` discipline (see
//! [`github_tool`](crate::github_tool), [`gitlab_tool`](crate::gitlab_tool),
//! [`discord_tool`](crate::discord_tool)): this crate ships **no HTTP
//! client** — HTTP is a `wcore-providers` / host concern. This module
//! therefore covers the **dispatch surface only**: schema, the typed
//! [`NotionOp`] enum, per-operation required-parameter validation,
//! request-shape construction, and a pluggable [`NotionBackend`] seam
//! the host wires to a real REST client (typically built on
//! `wcore_providers::http_client`).
//!
//! Without a backend bound, `execute()` returns a structured error
//! ("No Notion backend configured ...") rather than a silent stub —
//! honoring the NO-STUBS contract.
//!
//! ## Operations
//!
//! Four operations across read + write:
//!
//! * `get_page` — read a page's properties (`GET /v1/pages/{id}`).
//! * `get_block_children` — read a page/block's child blocks
//!   (`GET /v1/blocks/{id}/children`).
//! * `append_block_children` — append blocks to a page/block
//!   (`PATCH /v1/blocks/{id}/children`).
//! * `create_page` — create a new page under a parent
//!   (`POST /v1/pages`).
//!
//! ## Request-shape construction
//!
//! [`NotionRequest`] is a pure, testable description of the HTTP call the
//! backend must make: method, fully-qualified URL, header pairs (incl.
//! `Authorization` and the mandatory `Notion-Version`), and an optional
//! JSON body. [`NotionOp::build_request`] is a pure function, so tests
//! assert URL + header + body construction without any network I/O.
//!
//! ## Auth + API version
//!
//! The integration token is read from the tool input (`token`) or, if
//! absent, the `NOTION_TOKEN` env var. It is sent as
//! `Authorization: Bearer <token>`. Notion **requires** a `Notion-Version`
//! header on every request — it is pinned to [`NOTION_API_VERSION`]
//! (`2022-06-28`, the current stable dated revision) so responses stay
//! stable across Notion's API changes.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Notion REST API base URL.
pub const NOTION_API_BASE: &str = "https://api.notion.com/v1";

/// `Notion-Version` pin — Notion rejects requests without this header.
/// `2022-06-28` is the current stable dated API revision; bumping it is
/// a deliberate, reviewed change because newer versions can alter
/// response shapes.
pub const NOTION_API_VERSION: &str = "2022-06-28";

/// Canonical operation set. Order is preserved in the schema enum so the
/// model sees a stable manifest.
pub const NOTION_OPERATIONS: &[&str] = &[
    "get_page",
    "get_block_children",
    "append_block_children",
    "create_page",
];

// ---------------------------------------------------------------------
// Typed operation enum.
// ---------------------------------------------------------------------

/// A typed, validated Notion operation. Each variant maps to exactly one
/// REST endpoint.
#[derive(Debug, Clone, PartialEq)]
pub enum NotionOp {
    /// `GET /v1/pages/{page_id}`
    GetPage { page_id: String },
    /// `GET /v1/blocks/{block_id}/children` — optional pagination via
    /// `start_cursor` (Notion's opaque cursor token).
    GetBlockChildren {
        block_id: String,
        start_cursor: Option<String>,
    },
    /// `PATCH /v1/blocks/{block_id}/children` — append `children` blocks.
    /// `children` is a raw JSON array of Notion block objects supplied by
    /// the caller and forwarded verbatim.
    AppendBlockChildren { block_id: String, children: Value },
    /// `POST /v1/pages` — create a page. `parent` is a Notion parent
    /// object (`{"page_id": ...}` or `{"database_id": ...}`); `properties`
    /// is the page property map. Both are forwarded verbatim.
    CreatePage {
        parent: Value,
        properties: Value,
        /// Optional initial child blocks (JSON array).
        children: Option<Value>,
    },
}

/// HTTP method for a [`NotionRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Patch,
}

impl HttpMethod {
    /// The uppercase wire name (`"GET"` / `"POST"` / `"PATCH"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Patch => "PATCH",
        }
    }
}

/// A fully-described HTTP request the backend must perform. Pure data —
/// no transport. Built by [`NotionOp::build_request`].
#[derive(Debug, Clone, PartialEq)]
pub struct NotionRequest {
    pub method: HttpMethod,
    pub url: String,
    /// Header name/value pairs, including `Authorization` when a token
    /// is present and the mandatory `Notion-Version`.
    pub headers: Vec<(String, String)>,
    /// JSON body for `POST` / `PATCH`; `None` for `GET`.
    pub body: Option<Value>,
}

impl NotionRequest {
    /// Convenience: look up a header value by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Percent-encode a single URL path segment (e.g. a Notion ID). Notion
/// IDs are hex UUIDs (with or without dashes) so this rarely escapes
/// anything, but it keeps the URL well-formed for any unexpected input.
fn encode_segment(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for &byte in seg.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            other => {
                out.push('%');
                out.push_str(&format!("{other:02X}"));
            }
        }
    }
    out
}

impl NotionOp {
    /// Build the pure [`NotionRequest`] for this operation. `token`, when
    /// `Some`, is sent as `Authorization: Bearer <token>`. The
    /// `Notion-Version` header is always present.
    pub fn build_request(&self, token: Option<&str>) -> NotionRequest {
        let mut headers: Vec<(String, String)> = vec![
            ("Notion-Version".to_string(), NOTION_API_VERSION.to_string()),
            ("Accept".to_string(), "application/json".to_string()),
        ];
        if let Some(tok) = token.map(str::trim).filter(|t| !t.is_empty()) {
            headers.push(("Authorization".to_string(), format!("Bearer {tok}")));
        }

        match self {
            NotionOp::GetPage { page_id } => NotionRequest {
                method: HttpMethod::Get,
                url: format!("{NOTION_API_BASE}/pages/{}", encode_segment(page_id)),
                headers,
                body: None,
            },
            NotionOp::GetBlockChildren {
                block_id,
                start_cursor,
            } => {
                let mut url = format!(
                    "{NOTION_API_BASE}/blocks/{}/children",
                    encode_segment(block_id)
                );
                if let Some(c) = start_cursor
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
                {
                    url.push_str("?start_cursor=");
                    url.push_str(&encode_segment(c));
                }
                NotionRequest {
                    method: HttpMethod::Get,
                    url,
                    headers,
                    body: None,
                }
            }
            NotionOp::AppendBlockChildren { block_id, children } => {
                // Write requests carry a JSON body.
                headers.push(("Content-Type".to_string(), "application/json".to_string()));
                NotionRequest {
                    method: HttpMethod::Patch,
                    url: format!(
                        "{NOTION_API_BASE}/blocks/{}/children",
                        encode_segment(block_id)
                    ),
                    headers,
                    body: Some(json!({ "children": children })),
                }
            }
            NotionOp::CreatePage {
                parent,
                properties,
                children,
            } => {
                headers.push(("Content-Type".to_string(), "application/json".to_string()));
                let mut body = serde_json::Map::new();
                body.insert("parent".to_string(), parent.clone());
                body.insert("properties".to_string(), properties.clone());
                if let Some(c) = children {
                    body.insert("children".to_string(), c.clone());
                }
                NotionRequest {
                    method: HttpMethod::Post,
                    url: format!("{NOTION_API_BASE}/pages"),
                    headers,
                    body: Some(Value::Object(body)),
                }
            }
        }
    }

    /// Whether this operation only reads (safe to run concurrently).
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            NotionOp::GetPage { .. } | NotionOp::GetBlockChildren { .. }
        )
    }
}

// ---------------------------------------------------------------------
// Backend seam.
// ---------------------------------------------------------------------

/// Outcome of a backend dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum NotionOutcome {
    /// Success — `payload` is the parsed JSON the engine returns verbatim.
    Ok { payload: Value },
    /// Notion returned a non-2xx status. `status` is the HTTP code,
    /// `message` is a human-readable explanation (typically Notion's
    /// `{"message": ...}` field).
    HttpError { status: u16, message: String },
    /// Transport / auth-missing / any other failure path.
    Err { message: String },
}

/// Host-supplied Notion backend. The engine never speaks HTTP; the host
/// implements this trait — typically wrapping a client built via
/// `wcore_providers::http_client::build()` — and binds it at
/// construction time. The backend receives a pre-built [`NotionRequest`]
/// (URL + headers + body already assembled) and performs the call.
#[async_trait]
pub trait NotionBackend: Send + Sync {
    /// Execute `request` against Notion and return the parsed outcome.
    async fn dispatch(&self, request: &NotionRequest) -> NotionOutcome;
}

/// Default backend returned when the host wires nothing — every
/// `dispatch()` fails loudly so the tool never appears to succeed
/// silently (NO-STUBS contract).
pub struct NullNotionBackend;

#[async_trait]
impl NotionBackend for NullNotionBackend {
    async fn dispatch(&self, _request: &NotionRequest) -> NotionOutcome {
        NotionOutcome::Err {
            message: "No Notion backend configured. Wire a NotionBackend implementation \
                      (typically a wcore-providers http_client wrapper) when constructing \
                      NotionTool to enable Notion API operations."
                .to_string(),
        }
    }
}

/// In-memory backend that records every dispatched request and replays a
/// canned [`NotionOutcome`]. Lives in the prod module so downstream
/// crates and tests can reuse it without `#[cfg(test)]` gymnastics —
/// mirrors `CapturingGitHubBackend`.
pub struct CapturingNotionBackend {
    outcome: NotionOutcome,
    pub captured: parking_lot::Mutex<Vec<NotionRequest>>,
}

impl CapturingNotionBackend {
    /// New backend that replays `outcome` on every dispatch.
    pub fn new(outcome: NotionOutcome) -> Self {
        Self {
            outcome,
            captured: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// New backend that replays a successful `payload`.
    pub fn ok(payload: Value) -> Self {
        Self::new(NotionOutcome::Ok { payload })
    }

    /// Snapshot of every request the tool has dispatched so far.
    pub fn snapshot(&self) -> Vec<NotionRequest> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl NotionBackend for CapturingNotionBackend {
    async fn dispatch(&self, request: &NotionRequest) -> NotionOutcome {
        self.captured.lock().push(request.clone());
        self.outcome.clone()
    }
}

// ---------------------------------------------------------------------
// Argument parsing.
// ---------------------------------------------------------------------

fn str_field<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Decode the JSON args object into a typed [`NotionOp`]. Returns the
/// validation message on missing / invalid fields — run *before* the
/// backend is invoked.
fn parse_op(input: &Value) -> Result<NotionOp, String> {
    let operation = str_field(input, "operation")
        .ok_or_else(|| "Missing required parameter: 'operation'".to_string())?
        .to_ascii_lowercase();

    match operation.as_str() {
        "get_page" => Ok(NotionOp::GetPage {
            page_id: str_field(input, "page_id")
                .ok_or_else(|| {
                    "Missing required parameter 'page_id' for operation 'get_page'".to_string()
                })?
                .to_string(),
        }),
        "get_block_children" => Ok(NotionOp::GetBlockChildren {
            block_id: str_field(input, "block_id")
                .ok_or_else(|| {
                    "Missing required parameter 'block_id' for operation 'get_block_children'"
                        .to_string()
                })?
                .to_string(),
            start_cursor: str_field(input, "start_cursor").map(str::to_string),
        }),
        "append_block_children" => {
            let children = input.get("children").cloned().ok_or_else(|| {
                "Missing required parameter 'children' for operation 'append_block_children'"
                    .to_string()
            })?;
            if !children.is_array() {
                return Err(
                    "Parameter 'children' must be a JSON array of Notion block objects".to_string(),
                );
            }
            Ok(NotionOp::AppendBlockChildren {
                block_id: str_field(input, "block_id")
                    .ok_or_else(|| {
                        "Missing required parameter 'block_id' for operation \
                         'append_block_children'"
                            .to_string()
                    })?
                    .to_string(),
                children,
            })
        }
        "create_page" => {
            let parent = input.get("parent").cloned().ok_or_else(|| {
                "Missing required parameter 'parent' for operation 'create_page'".to_string()
            })?;
            if !parent.is_object() {
                return Err("Parameter 'parent' must be a JSON object (a Notion parent \
                            reference, e.g. {\"page_id\": ...})"
                    .to_string());
            }
            let properties = input.get("properties").cloned().ok_or_else(|| {
                "Missing required parameter 'properties' for operation 'create_page'".to_string()
            })?;
            if !properties.is_object() {
                return Err(
                    "Parameter 'properties' must be a JSON object of Notion page properties"
                        .to_string(),
                );
            }
            let children = match input.get("children") {
                None | Some(Value::Null) => None,
                Some(c) if c.is_array() => Some(c.clone()),
                Some(_) => {
                    return Err(
                        "Parameter 'children', when present, must be a JSON array of \
                                Notion block objects"
                            .to_string(),
                    );
                }
            };
            Ok(NotionOp::CreatePage {
                parent,
                properties,
                children,
            })
        }
        other => Err(format!(
            "Unknown operation: '{other}'. Supported: {}",
            NOTION_OPERATIONS.join(", ")
        )),
    }
}

// ---------------------------------------------------------------------
// Tool.
// ---------------------------------------------------------------------

/// `notion_api` tool — Notion REST API operations (page + block reads,
/// block append, page create).
pub struct NotionTool {
    backend: Arc<dyn NotionBackend>,
}

impl Default for NotionTool {
    fn default() -> Self {
        Self::new(Arc::new(NullNotionBackend))
    }
}

impl NotionTool {
    /// New tool bound to `backend`.
    pub fn new(backend: Arc<dyn NotionBackend>) -> Self {
        Self { backend }
    }

    /// Resolve the auth token: explicit `token` arg first, then the
    /// `NOTION_TOKEN` env var.
    fn resolve_token(input: &Value) -> Option<String> {
        if let Some(t) = str_field(input, "token") {
            return Some(t.to_string());
        }
        std::env::var("NOTION_TOKEN")
            .ok()
            .map(|t| t.trim().to_string())
            .filter(|t| !t.is_empty())
    }
}

fn err_result(message: impl Into<String>) -> ToolResult {
    ToolResult {
        content: json!({ "error": message.into() }).to_string(),
        is_error: true,
    }
}

#[async_trait]
impl Tool for NotionTool {
    fn name(&self) -> &str {
        "notion_api"
    }

    fn description(&self) -> &str {
        "Operate on the Notion REST API. Read a page's properties (get_page) or a page/block's \
         child blocks (get_block_children); append blocks to a page or block \
         (append_block_children); or create a new page (create_page). Auth via the 'token' \
         argument or the NOTION_TOKEN environment variable."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": NOTION_OPERATIONS,
                    "description": "Which Notion operation to perform."
                },
                "page_id": {
                    "type": "string",
                    "description": "Notion page ID. Required for get_page."
                },
                "block_id": {
                    "type": "string",
                    "description": "Notion block (or page) ID whose children are read or \
                                    appended. Required for get_block_children and \
                                    append_block_children."
                },
                "start_cursor": {
                    "type": "string",
                    "description": "Optional pagination cursor for get_block_children."
                },
                "children": {
                    "type": "array",
                    "description": "Array of Notion block objects. Required for \
                                    append_block_children; optional for create_page."
                },
                "parent": {
                    "type": "object",
                    "description": "Notion parent reference (e.g. {\"page_id\": ...} or \
                                    {\"database_id\": ...}). Required for create_page."
                },
                "properties": {
                    "type": "object",
                    "description": "Notion page property map. Required for create_page."
                },
                "token": {
                    "type": "string",
                    "description": "Notion integration token. Falls back to the NOTION_TOKEN \
                                    env var."
                }
            },
            "required": ["operation"]
        })
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        // Only the read operations are concurrency-safe.
        match parse_op(input) {
            Ok(op) => op.is_read_only(),
            // Unparseable input short-circuits to an error anyway;
            // treat as unsafe so a malformed call is never parallelized.
            Err(_) => false,
        }
    }

    fn category(&self) -> ToolCategory {
        // Includes mutating operations (append / create). Categorize as
        // Edit so hosts that gate side-effecting tools behind approval
        // catch this tool too.
        ToolCategory::Edit
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let op = match parse_op(&input) {
            Ok(op) => op,
            Err(e) => return err_result(e),
        };

        let token = Self::resolve_token(&input);
        let request = op.build_request(token.as_deref());

        match self.backend.dispatch(&request).await {
            NotionOutcome::Ok { payload } => ToolResult {
                content: payload.to_string(),
                is_error: false,
            },
            NotionOutcome::HttpError { status, message } => {
                err_result(format!("Notion API error {status}: {message}"))
            }
            NotionOutcome::Err { message } => err_result(message),
        }
    }
}

/// Register the Notion tool into `registry`, bound to `backend`. Hosts
/// typically call this once at startup after resolving a Notion token.
pub fn register_notion_tool(
    registry: &mut crate::registry::ToolRegistry,
    backend: Arc<dyn NotionBackend>,
) {
    registry.register(Box::new(NotionTool::new(backend)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(tool: &NotionTool, input: Value) -> ToolResult {
        futures::executor::block_on(tool.execute(input))
    }

    fn parse_json(result: &ToolResult) -> Value {
        serde_json::from_str(&result.content).expect("tool result must be valid JSON")
    }

    // ----------------------------------------------------------------
    // Request URL + header construction per operation.
    // ----------------------------------------------------------------

    #[test]
    fn build_request_constructs_correct_urls_and_methods() {
        let page = NotionOp::GetPage {
            page_id: "abc123".into(),
        }
        .build_request(None);
        assert_eq!(page.method, HttpMethod::Get);
        assert_eq!(page.url, "https://api.notion.com/v1/pages/abc123");
        assert!(page.body.is_none());

        // get_block_children with a pagination cursor.
        let children = NotionOp::GetBlockChildren {
            block_id: "blk-1".into(),
            start_cursor: Some("cursor-xyz".into()),
        }
        .build_request(None);
        assert_eq!(children.method, HttpMethod::Get);
        assert_eq!(
            children.url,
            "https://api.notion.com/v1/blocks/blk-1/children?start_cursor=cursor-xyz"
        );

        // get_block_children without a cursor → no query string.
        let children_nocursor = NotionOp::GetBlockChildren {
            block_id: "blk-2".into(),
            start_cursor: None,
        }
        .build_request(None);
        assert_eq!(
            children_nocursor.url,
            "https://api.notion.com/v1/blocks/blk-2/children"
        );

        // append_block_children → PATCH with a {"children": [...]} body.
        let append = NotionOp::AppendBlockChildren {
            block_id: "blk-3".into(),
            children: json!([{ "type": "paragraph" }]),
        }
        .build_request(None);
        assert_eq!(append.method, HttpMethod::Patch);
        assert_eq!(
            append.url,
            "https://api.notion.com/v1/blocks/blk-3/children"
        );
        assert_eq!(
            append.body,
            Some(json!({ "children": [{ "type": "paragraph" }] }))
        );
        assert_eq!(append.header("Content-Type"), Some("application/json"));

        // create_page → POST /v1/pages with parent + properties merged.
        let create = NotionOp::CreatePage {
            parent: json!({ "page_id": "parent-1" }),
            properties: json!({ "title": [] }),
            children: Some(json!([{ "type": "heading_1" }])),
        }
        .build_request(None);
        assert_eq!(create.method, HttpMethod::Post);
        assert_eq!(create.url, "https://api.notion.com/v1/pages");
        let body = create.body.expect("create_page has a body");
        assert_eq!(body["parent"], json!({ "page_id": "parent-1" }));
        assert_eq!(body["properties"], json!({ "title": [] }));
        assert_eq!(body["children"], json!([{ "type": "heading_1" }]));
    }

    #[test]
    fn build_request_sets_auth_and_mandatory_notion_version_header() {
        // With a token → Authorization: Bearer present.
        let req = NotionOp::GetPage {
            page_id: "p1".into(),
        }
        .build_request(Some("secret_token"));
        assert_eq!(req.header("Authorization"), Some("Bearer secret_token"));
        // Notion-Version is mandatory and always present.
        assert_eq!(req.header("Notion-Version"), Some(NOTION_API_VERSION));
        assert_eq!(req.header("notion-version"), Some("2022-06-28"));
        assert_eq!(req.header("accept"), Some("application/json"));

        // Without a token → no Authorization header, but Notion-Version
        // is still sent.
        let anon = NotionOp::GetPage {
            page_id: "p1".into(),
        }
        .build_request(None);
        assert!(anon.header("Authorization").is_none());
        assert_eq!(anon.header("Notion-Version"), Some(NOTION_API_VERSION));

        // A blank/whitespace token is treated as absent.
        let blank = NotionOp::GetPage {
            page_id: "p1".into(),
        }
        .build_request(Some("   "));
        assert!(blank.header("Authorization").is_none());
        assert_eq!(blank.header("Notion-Version"), Some(NOTION_API_VERSION));
    }

    // ----------------------------------------------------------------
    // Response JSON parsing from fixture payloads.
    // ----------------------------------------------------------------

    #[test]
    fn execute_returns_parsed_fixture_payload_for_get_page() {
        // Fixture mirrors the shape of a real Notion page response.
        let fixture = json!({
            "object": "page",
            "id": "page-abc",
            "archived": false,
            "properties": {
                "Name": { "title": [{ "plain_text": "Roadmap" }] }
            }
        });
        let backend = Arc::new(CapturingNotionBackend::ok(fixture.clone()));
        let tool = NotionTool::new(backend.clone());
        let res = run(
            &tool,
            json!({
                "operation": "get_page",
                "page_id": "page-abc",
                "token": "secret_x"
            }),
        );
        assert!(!res.is_error, "expected ok, got: {}", res.content);
        let v = parse_json(&res);
        assert_eq!(v["object"], json!("page"));
        assert_eq!(v["id"], json!("page-abc"));
        assert_eq!(
            v["properties"]["Name"]["title"][0]["plain_text"],
            json!("Roadmap")
        );

        // The backend saw exactly one correctly-built request, carrying
        // both the auth header and the mandatory Notion-Version header.
        let reqs = backend.snapshot();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].url, "https://api.notion.com/v1/pages/page-abc");
        assert_eq!(reqs[0].header("Authorization"), Some("Bearer secret_x"));
        assert_eq!(reqs[0].header("Notion-Version"), Some(NOTION_API_VERSION));
    }

    #[test]
    fn execute_appends_block_children_with_body_and_version_header() {
        let fixture = json!({
            "object": "list",
            "results": [{ "object": "block", "type": "paragraph" }]
        });
        let backend = Arc::new(CapturingNotionBackend::ok(fixture));
        let tool = NotionTool::new(backend.clone());
        let res = run(
            &tool,
            json!({
                "operation": "append_block_children",
                "block_id": "blk-7",
                "children": [
                    { "type": "paragraph", "paragraph": { "rich_text": [] } }
                ],
                "token": "secret_y"
            }),
        );
        assert!(!res.is_error, "got: {}", res.content);
        let reqs = backend.snapshot();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].method, HttpMethod::Patch);
        assert_eq!(
            reqs[0].url,
            "https://api.notion.com/v1/blocks/blk-7/children"
        );
        assert_eq!(reqs[0].header("Notion-Version"), Some(NOTION_API_VERSION));
        // Body wraps the caller's children array under "children".
        let body = reqs[0].body.as_ref().expect("append has a body");
        assert!(body["children"].is_array());
        assert_eq!(body["children"][0]["type"], json!("paragraph"));
    }

    // ----------------------------------------------------------------
    // Error path — 404 + null backend fail-loud.
    // ----------------------------------------------------------------

    #[test]
    fn execute_surfaces_http_error_for_404() {
        let backend = Arc::new(CapturingNotionBackend::new(NotionOutcome::HttpError {
            status: 404,
            message: "Could not find page".to_string(),
        }));
        let tool = NotionTool::new(backend);
        let res = run(
            &tool,
            json!({ "operation": "get_page", "page_id": "missing" }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("Notion API error 404"));
        assert!(res.content.contains("Could not find page"));
    }

    #[test]
    fn null_backend_fails_loud_no_silent_stub() {
        let tool = NotionTool::default();
        let res = run(
            &tool,
            json!({ "operation": "get_block_children", "block_id": "blk-1" }),
        );
        assert!(res.is_error);
        assert!(
            res.content.contains("No Notion backend configured"),
            "expected fail-loud, got: {}",
            res.content
        );
    }

    // ----------------------------------------------------------------
    // Input-schema validation.
    // ----------------------------------------------------------------

    #[test]
    fn invalid_input_rejected_before_backend() {
        let backend = Arc::new(CapturingNotionBackend::ok(json!({})));

        // Missing operation.
        let tool = NotionTool::new(backend.clone());
        let res = run(&tool, json!({ "page_id": "p" }));
        assert!(res.is_error);
        assert!(res.content.contains("'operation'"));

        // Unknown operation.
        let res = run(&tool, json!({ "operation": "delete_page", "page_id": "p" }));
        assert!(res.is_error);
        assert!(res.content.contains("Unknown operation"));

        // get_page missing 'page_id'.
        let res = run(&tool, json!({ "operation": "get_page" }));
        assert!(res.is_error);
        assert!(res.content.contains("'page_id'"));

        // append_block_children missing 'children'.
        let res = run(
            &tool,
            json!({ "operation": "append_block_children", "block_id": "b" }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("'children'"));

        // append_block_children with non-array 'children'.
        let res = run(
            &tool,
            json!({
                "operation": "append_block_children",
                "block_id": "b",
                "children": { "not": "an array" }
            }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("must be a JSON array"));

        // create_page missing 'properties'.
        let res = run(
            &tool,
            json!({
                "operation": "create_page",
                "parent": { "page_id": "p" }
            }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("'properties'"));

        // No request reached the backend for any rejected call.
        assert!(
            backend.snapshot().is_empty(),
            "backend must not be called on invalid input"
        );
    }

    #[test]
    fn schema_and_concurrency_safety() {
        let tool = NotionTool::default();
        let schema = tool.input_schema();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .expect("required array")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(required, ["operation"]);
        let ops: Vec<&str> = schema["properties"]["operation"]["enum"]
            .as_array()
            .expect("operation enum")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(ops, NOTION_OPERATIONS);

        // Reads are concurrency-safe; writes are not.
        assert!(tool.is_concurrency_safe(&json!({
            "operation": "get_page", "page_id": "p"
        })));
        assert!(tool.is_concurrency_safe(&json!({
            "operation": "get_block_children", "block_id": "b"
        })));
        assert!(!tool.is_concurrency_safe(&json!({
            "operation": "append_block_children", "block_id": "b",
            "children": []
        })));
        assert!(!tool.is_concurrency_safe(&json!({
            "operation": "create_page",
            "parent": { "page_id": "p" },
            "properties": {}
        })));
    }

    #[test]
    fn register_notion_tool_populates_registry() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        register_notion_tool(&mut reg, Arc::new(NullNotionBackend));
        let names = reg.tool_names();
        assert!(
            names.iter().any(|n| n == "notion_api"),
            "notion_api missing from registry: {names:?}"
        );
    }
}
