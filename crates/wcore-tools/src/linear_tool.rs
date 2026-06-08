//! v0.6.3 Tier 2B T5 — Linear GraphQL API operations tool.
//!
//! Linear exposes a single GraphQL endpoint at
//! `https://api.linear.app/graphql`. Mirroring the established
//! `wcore-tools` discipline (see [`github_tool`](crate::github_tool),
//! [`gitlab_tool`](crate::gitlab_tool)), this crate ships **no HTTP
//! client** — HTTP is a `wcore-providers` / host concern. This port
//! covers the **dispatch surface only**: schema, the typed [`LinearOp`]
//! enum, per-operation parameter handling, GraphQL request-shape
//! construction, and a pluggable [`LinearBackend`] seam the host wires
//! to a real GraphQL client (typically built on
//! `wcore-providers::http_client`).
//!
//! Without a backend bound, `execute()` returns a structured error
//! ("No Linear backend configured …") rather than a silent stub —
//! honoring the NO-STUBS contract.
//!
//! ## Operations
//!
//! Three read-only operations, all GraphQL queries against the single
//! endpoint:
//!
//! * `query_issues` — list issues (optionally filtered by team key),
//!   page-limited by `first`.
//! * `query_cycles` — list cycles (optionally filtered by team key).
//! * `query_teams` — list teams in the workspace.
//!
//! ## Request-shape construction
//!
//! [`LinearRequest`] is a pure, testable description of the HTTP call
//! the backend must make: method (always `POST`), the endpoint URL,
//! header pairs (incl. the `Authorization` header), and a JSON body
//! carrying the GraphQL `query` string and `variables` object.
//! [`LinearOp::build_request`] is a pure function, so tests assert
//! query + header + variable construction without any network I/O.
//!
//! ## Auth
//!
//! The API key is read from the tool input (`api_key`) or, if absent,
//! the `LINEAR_API_KEY` env var. Linear sends the key directly in the
//! `Authorization` header (no `Bearer` prefix for personal API keys).
//! A request without a key is still built — the backend forwards it and
//! lets Linear reject it with `401`, matching the github/gitlab tools.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Linear GraphQL API endpoint — Linear exposes exactly one URL.
pub const LINEAR_API_URL: &str = "https://api.linear.app/graphql";

/// Default page size for list queries when the caller omits `first`.
pub const LINEAR_DEFAULT_PAGE_SIZE: u32 = 50;

/// Maximum page size accepted — Linear's GraphQL API caps `first` at
/// 250; clamping here keeps the tool from sending a request Linear will
/// reject outright.
pub const LINEAR_MAX_PAGE_SIZE: u32 = 250;

/// Canonical operation set. Order is preserved in the schema enum so the
/// model sees a stable manifest.
pub const LINEAR_OPERATIONS: &[&str] = &["query_issues", "query_cycles", "query_teams"];

// ---------------------------------------------------------------------
// Typed operation enum.
// ---------------------------------------------------------------------

/// A typed Linear operation. Each variant maps to exactly one GraphQL
/// query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinearOp {
    /// List issues. `team_key` optionally filters to one team (Linear's
    /// short team key, e.g. `ENG`); `first` bounds the page size.
    QueryIssues {
        team_key: Option<String>,
        first: u32,
    },
    /// List cycles. `team_key` optionally filters to one team.
    QueryCycles {
        team_key: Option<String>,
        first: u32,
    },
    /// List teams in the workspace.
    QueryTeams { first: u32 },
}

/// The GraphQL query document for listing issues. `$first` bounds the
/// page; `$filter` is an optional `IssueFilter` (null = unfiltered).
const ISSUES_QUERY: &str = "query Issues($first: Int!, $filter: IssueFilter) { \
issues(first: $first, filter: $filter) { nodes { \
id identifier title state { name type } \
team { id key name } priority createdAt updatedAt } } }";

/// The GraphQL query document for listing cycles.
const CYCLES_QUERY: &str = "query Cycles($first: Int!, $filter: CycleFilter) { \
cycles(first: $first, filter: $filter) { nodes { \
id number name startsAt endsAt completedAt \
team { id key name } } } }";

/// The GraphQL query document for listing teams.
const TEAMS_QUERY: &str = "query Teams($first: Int!) { \
teams(first: $first) { nodes { \
id key name description private } } }";

impl LinearOp {
    /// The GraphQL query document for this operation.
    pub fn query(&self) -> &'static str {
        match self {
            LinearOp::QueryIssues { .. } => ISSUES_QUERY,
            LinearOp::QueryCycles { .. } => CYCLES_QUERY,
            LinearOp::QueryTeams { .. } => TEAMS_QUERY,
        }
    }

    /// The GraphQL `variables` object for this operation. A team-key
    /// filter is expressed as `{ team: { key: { eq: "<KEY>" } } }`,
    /// which is the shape both `IssueFilter` and `CycleFilter` accept.
    pub fn variables(&self) -> Value {
        match self {
            LinearOp::QueryIssues { team_key, first }
            | LinearOp::QueryCycles { team_key, first } => {
                let mut vars = serde_json::Map::new();
                vars.insert("first".to_string(), json!(first));
                match team_key {
                    Some(key) => {
                        vars.insert(
                            "filter".to_string(),
                            json!({ "team": { "key": { "eq": key } } }),
                        );
                    }
                    None => {
                        vars.insert("filter".to_string(), Value::Null);
                    }
                }
                Value::Object(vars)
            }
            LinearOp::QueryTeams { first } => json!({ "first": first }),
        }
    }

    /// Build the pure [`LinearRequest`] for this operation. `api_key`,
    /// when `Some`, is sent as the `Authorization` header value
    /// verbatim (Linear personal API keys carry no scheme prefix).
    pub fn build_request(&self, api_key: Option<&str>) -> LinearRequest {
        let mut headers: Vec<(String, String)> = vec![
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Accept".to_string(), "application/json".to_string()),
        ];
        if let Some(key) = api_key.map(str::trim).filter(|k| !k.is_empty()) {
            headers.push(("Authorization".to_string(), key.to_string()));
        }

        LinearRequest {
            url: LINEAR_API_URL.to_string(),
            headers,
            body: json!({
                "query": self.query(),
                "variables": self.variables(),
            }),
        }
    }
}

/// A fully-described HTTP request the backend must perform. Pure data —
/// no transport. Built by [`LinearOp::build_request`]. The method is
/// always `POST` (every GraphQL call is a POST), so it is not carried
/// as a field.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearRequest {
    /// The Linear GraphQL endpoint URL.
    pub url: String,
    /// Header name/value pairs, including `Authorization` when an API
    /// key is present.
    pub headers: Vec<(String, String)>,
    /// JSON body carrying the GraphQL `query` and `variables`.
    pub body: Value,
}

impl LinearRequest {
    /// Convenience: look up a header value by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// The GraphQL query string carried in the body.
    pub fn graphql_query(&self) -> Option<&str> {
        self.body.get("query").and_then(Value::as_str)
    }

    /// The GraphQL variables object carried in the body.
    pub fn graphql_variables(&self) -> Option<&Value> {
        self.body.get("variables")
    }
}

// ---------------------------------------------------------------------
// Backend seam.
// ---------------------------------------------------------------------

/// Outcome of a backend dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum LinearOutcome {
    /// Success — `payload` is the parsed GraphQL response the engine
    /// returns verbatim (the backend should pass through Linear's
    /// `{"data": ...}` envelope).
    Ok { payload: Value },
    /// Linear returned a non-2xx HTTP status. `status` is the HTTP
    /// code, `message` a human-readable explanation.
    HttpError { status: u16, message: String },
    /// GraphQL-level errors — the HTTP call succeeded (`200`) but the
    /// response carried a non-empty `errors` array. `message` is the
    /// joined error text.
    GraphQlError { message: String },
    /// Transport / auth-missing / any other failure path.
    Err { message: String },
}

/// Host-supplied Linear backend. The engine never speaks HTTP; the host
/// implements this trait — typically wrapping a client built via
/// `wcore_providers::http_client::build()` — and binds it at
/// construction time. The backend receives a pre-built [`LinearRequest`]
/// (URL + headers + GraphQL body already assembled) and performs the
/// call.
#[async_trait]
pub trait LinearBackend: Send + Sync {
    /// Execute `request` against Linear and return the parsed outcome.
    async fn dispatch(&self, request: &LinearRequest) -> LinearOutcome;
}

/// Default backend returned when the host wires nothing — every
/// `dispatch()` fails loudly so the tool never appears to succeed
/// silently (NO-STUBS contract).
pub struct NullLinearBackend;

#[async_trait]
impl LinearBackend for NullLinearBackend {
    async fn dispatch(&self, _request: &LinearRequest) -> LinearOutcome {
        LinearOutcome::Err {
            message: "No Linear backend configured. Wire a LinearBackend implementation \
                      (typically a wcore-providers http_client wrapper) when constructing \
                      LinearTool to enable Linear API operations."
                .to_string(),
        }
    }
}

/// In-memory backend that records every dispatched request and replays a
/// canned [`LinearOutcome`]. Lives in the prod module so downstream
/// crates and tests can reuse it without `#[cfg(test)]` gymnastics —
/// mirrors `CapturingGitHubBackend`.
pub struct CapturingLinearBackend {
    outcome: LinearOutcome,
    pub captured: parking_lot::Mutex<Vec<LinearRequest>>,
}

impl CapturingLinearBackend {
    /// New backend that replays `outcome` on every dispatch.
    pub fn new(outcome: LinearOutcome) -> Self {
        Self {
            outcome,
            captured: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// New backend that replays a successful `payload`.
    pub fn ok(payload: Value) -> Self {
        Self::new(LinearOutcome::Ok { payload })
    }

    /// Snapshot of every request the tool has dispatched so far.
    pub fn snapshot(&self) -> Vec<LinearRequest> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl LinearBackend for CapturingLinearBackend {
    async fn dispatch(&self, request: &LinearRequest) -> LinearOutcome {
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

/// Resolve the `first` page-size argument, defaulting and clamping to
/// Linear's accepted range. Accepts an integer or a numeric string.
fn first_field(input: &Value) -> u32 {
    let raw = input.get("first").and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_i64().filter(|n| *n >= 0).map(|n| n as u64))
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
    });
    match raw {
        Some(0) | None => LINEAR_DEFAULT_PAGE_SIZE,
        Some(n) => (n as u32).min(LINEAR_MAX_PAGE_SIZE),
    }
}

/// Decode the JSON args object into a typed [`LinearOp`]. Returns the
/// validation message on a missing / unknown operation — run *before*
/// the backend is invoked.
fn parse_op(input: &Value) -> Result<LinearOp, String> {
    let operation = str_field(input, "operation")
        .ok_or_else(|| "Missing required parameter: 'operation'".to_string())?
        .to_ascii_lowercase();

    let first = first_field(input);
    let team_key = str_field(input, "team_key").map(str::to_string);

    match operation.as_str() {
        "query_issues" => Ok(LinearOp::QueryIssues { team_key, first }),
        "query_cycles" => Ok(LinearOp::QueryCycles { team_key, first }),
        "query_teams" => Ok(LinearOp::QueryTeams { first }),
        other => Err(format!(
            "Unknown operation: '{other}'. Supported: {}",
            LINEAR_OPERATIONS.join(", ")
        )),
    }
}

// ---------------------------------------------------------------------
// Tool.
// ---------------------------------------------------------------------

/// `linear_api` tool — Linear GraphQL API read operations (issue /
/// cycle / team queries).
pub struct LinearTool {
    backend: Arc<dyn LinearBackend>,
}

impl Default for LinearTool {
    fn default() -> Self {
        Self::new(Arc::new(NullLinearBackend))
    }
}

impl LinearTool {
    /// New tool bound to `backend`.
    pub fn new(backend: Arc<dyn LinearBackend>) -> Self {
        Self { backend }
    }

    /// Resolve the auth key: explicit `api_key` arg first, then the
    /// `LINEAR_API_KEY` env var.
    fn resolve_api_key(input: &Value) -> Option<String> {
        if let Some(k) = str_field(input, "api_key") {
            return Some(k.to_string());
        }
        std::env::var("LINEAR_API_KEY")
            .ok()
            .map(|k| k.trim().to_string())
            .filter(|k| !k.is_empty())
    }
}

fn err_result(message: impl Into<String>) -> ToolResult {
    ToolResult {
        content: json!({ "error": message.into() }).to_string(),
        is_error: true,
    }
}

#[async_trait]
impl Tool for LinearTool {
    fn name(&self) -> &str {
        "linear_api"
    }

    fn description(&self) -> &str {
        "Query the Linear GraphQL API (read-only). List issues (query_issues), cycles \
         (query_cycles), or teams (query_teams) in a Linear workspace. query_issues and \
         query_cycles accept an optional 'team_key' filter (the short team key, e.g. 'ENG'); \
         all operations accept an optional 'first' page size (default 50, max 250). Auth via \
         the 'api_key' argument or the LINEAR_API_KEY environment variable."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": LINEAR_OPERATIONS,
                    "description": "Which Linear query to run."
                },
                "team_key": {
                    "type": "string",
                    "description": "Optional Linear team key (e.g. 'ENG') to filter results. \
                                    Applies to query_issues and query_cycles; ignored by \
                                    query_teams."
                },
                "first": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": LINEAR_MAX_PAGE_SIZE,
                    "description": "Maximum number of results to return (default 50, max 250)."
                },
                "api_key": {
                    "type": "string",
                    "description": "Linear API key. Falls back to the LINEAR_API_KEY env var."
                }
            },
            "required": ["operation"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Every Linear operation here is a read-only GraphQL query.
        true
    }

    fn category(&self) -> ToolCategory {
        // Read-only GraphQL queries — no side effects.
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let op = match parse_op(&input) {
            Ok(op) => op,
            Err(e) => return err_result(e),
        };

        let api_key = Self::resolve_api_key(&input);
        let request = op.build_request(api_key.as_deref());

        match self.backend.dispatch(&request).await {
            LinearOutcome::Ok { payload } => ToolResult {
                content: payload.to_string(),
                is_error: false,
            },
            LinearOutcome::HttpError { status, message } => {
                err_result(format!("Linear API error {status}: {message}"))
            }
            LinearOutcome::GraphQlError { message } => {
                err_result(format!("Linear GraphQL error: {message}"))
            }
            LinearOutcome::Err { message } => err_result(message),
        }
    }
}

/// Register the Linear tool into `registry`, bound to `backend`. Hosts
/// typically call this once at startup after resolving a Linear API key.
pub fn register_linear_tool(
    registry: &mut crate::registry::ToolRegistry,
    backend: Arc<dyn LinearBackend>,
) {
    registry.register(Box::new(LinearTool::new(backend)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(tool: &LinearTool, input: Value) -> ToolResult {
        futures::executor::block_on(tool.execute(input))
    }

    fn parse_json(result: &ToolResult) -> Value {
        serde_json::from_str(&result.content).expect("tool result must be valid JSON")
    }

    // ----------------------------------------------------------------
    // GraphQL query + variable construction per operation.
    // ----------------------------------------------------------------

    #[test]
    fn build_request_constructs_correct_query_and_variables() {
        // query_issues with a team filter.
        let issues = LinearOp::QueryIssues {
            team_key: Some("ENG".into()),
            first: 25,
        }
        .build_request(None);
        assert_eq!(issues.url, "https://api.linear.app/graphql");
        let q = issues.graphql_query().expect("query present");
        assert!(q.contains("issues(first: $first, filter: $filter)"));
        let vars = issues.graphql_variables().expect("variables present");
        assert_eq!(vars["first"], json!(25));
        assert_eq!(
            vars["filter"],
            json!({ "team": { "key": { "eq": "ENG" } } })
        );

        // query_cycles without a team filter → filter is JSON null.
        let cycles = LinearOp::QueryCycles {
            team_key: None,
            first: 50,
        }
        .build_request(None);
        let cq = cycles.graphql_query().expect("query present");
        assert!(cq.contains("cycles(first: $first, filter: $filter)"));
        let cvars = cycles.graphql_variables().expect("variables present");
        assert_eq!(cvars["first"], json!(50));
        assert_eq!(cvars["filter"], Value::Null);

        // query_teams — no filter variable at all.
        let teams = LinearOp::QueryTeams { first: 10 }.build_request(None);
        let tq = teams.graphql_query().expect("query present");
        assert!(tq.contains("teams(first: $first)"));
        let tvars = teams.graphql_variables().expect("variables present");
        assert_eq!(tvars["first"], json!(10));
        assert!(
            tvars.get("filter").is_none(),
            "query_teams takes no filter variable"
        );
    }

    // ----------------------------------------------------------------
    // Auth header construction.
    // ----------------------------------------------------------------

    #[test]
    fn build_request_sets_auth_and_standard_headers() {
        // With a key → Authorization carries the key verbatim (no scheme).
        let req = LinearOp::QueryTeams { first: 50 }.build_request(Some("lin_api_secret"));
        assert_eq!(req.header("Authorization"), Some("lin_api_secret"));
        assert_eq!(req.header("content-type"), Some("application/json"));
        assert_eq!(req.header("Accept"), Some("application/json"));

        // Without a key → no Authorization header (backend will 401).
        let anon = LinearOp::QueryTeams { first: 50 }.build_request(None);
        assert!(anon.header("Authorization").is_none());

        // A blank/whitespace key is treated as absent.
        let blank = LinearOp::QueryTeams { first: 50 }.build_request(Some("   "));
        assert!(blank.header("Authorization").is_none());
    }

    // ----------------------------------------------------------------
    // `first` defaulting + clamping.
    // ----------------------------------------------------------------

    #[test]
    fn first_field_defaults_and_clamps() {
        // Absent → default.
        assert_eq!(first_field(&json!({})), LINEAR_DEFAULT_PAGE_SIZE);
        // Zero → default (a zero page is never useful).
        assert_eq!(
            first_field(&json!({ "first": 0 })),
            LINEAR_DEFAULT_PAGE_SIZE
        );
        // In range → passes through.
        assert_eq!(first_field(&json!({ "first": 75 })), 75);
        // Over the cap → clamped to the max.
        assert_eq!(first_field(&json!({ "first": 9999 })), LINEAR_MAX_PAGE_SIZE);
        // Numeric string is accepted.
        assert_eq!(first_field(&json!({ "first": "30" })), 30);
    }

    // ----------------------------------------------------------------
    // Response JSON parsing from a fixture payload.
    // ----------------------------------------------------------------

    #[test]
    fn execute_returns_parsed_fixture_payload_for_query_issues() {
        // Fixture mirrors a real Linear GraphQL `issues` response.
        let fixture = json!({
            "data": {
                "issues": {
                    "nodes": [
                        {
                            "id": "abc-123",
                            "identifier": "ENG-42",
                            "title": "Fix the thing",
                            "state": { "name": "In Progress", "type": "started" },
                            "team": { "id": "t1", "key": "ENG", "name": "Engineering" },
                            "priority": 2
                        }
                    ]
                }
            }
        });
        let backend = Arc::new(CapturingLinearBackend::ok(fixture.clone()));
        let tool = LinearTool::new(backend.clone());
        let res = run(
            &tool,
            json!({
                "operation": "query_issues",
                "team_key": "ENG",
                "first": 20,
                "api_key": "lin_x"
            }),
        );
        assert!(!res.is_error, "expected ok, got: {}", res.content);
        let v = parse_json(&res);
        assert_eq!(
            v["data"]["issues"]["nodes"][0]["identifier"],
            json!("ENG-42")
        );
        assert_eq!(v["data"]["issues"]["nodes"][0]["team"]["key"], json!("ENG"));

        // The backend saw exactly one correctly-built request.
        let reqs = backend.snapshot();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].url, "https://api.linear.app/graphql");
        assert_eq!(reqs[0].header("Authorization"), Some("lin_x"));
        let vars = reqs[0].graphql_variables().expect("variables");
        assert_eq!(vars["first"], json!(20));
        assert_eq!(vars["filter"]["team"]["key"]["eq"], json!("ENG"));
    }

    // ----------------------------------------------------------------
    // Error paths — HTTP error, GraphQL error, null backend.
    // ----------------------------------------------------------------

    #[test]
    fn execute_surfaces_http_error() {
        let backend = Arc::new(CapturingLinearBackend::new(LinearOutcome::HttpError {
            status: 401,
            message: "Authentication required".to_string(),
        }));
        let tool = LinearTool::new(backend);
        let res = run(&tool, json!({ "operation": "query_teams" }));
        assert!(res.is_error);
        assert!(res.content.contains("Linear API error 401"));
        assert!(res.content.contains("Authentication required"));
    }

    #[test]
    fn execute_surfaces_graphql_error() {
        let backend = Arc::new(CapturingLinearBackend::new(LinearOutcome::GraphQlError {
            message: "Field 'bogus' doesn't exist on type 'Query'".to_string(),
        }));
        let tool = LinearTool::new(backend);
        let res = run(&tool, json!({ "operation": "query_cycles" }));
        assert!(res.is_error);
        assert!(res.content.contains("Linear GraphQL error"));
        assert!(res.content.contains("doesn't exist"));
    }

    #[test]
    fn null_backend_fails_loud_no_silent_stub() {
        let tool = LinearTool::default();
        let res = run(&tool, json!({ "operation": "query_issues" }));
        assert!(res.is_error);
        assert!(
            res.content.contains("No Linear backend configured"),
            "expected fail-loud, got: {}",
            res.content
        );
    }

    // ----------------------------------------------------------------
    // Input-schema validation.
    // ----------------------------------------------------------------

    #[test]
    fn invalid_input_rejected_before_backend() {
        let backend = Arc::new(CapturingLinearBackend::ok(json!({})));
        let tool = LinearTool::new(backend.clone());

        // Missing operation.
        let res = run(&tool, json!({}));
        assert!(res.is_error);
        assert!(res.content.contains("'operation'"));

        // Unknown operation.
        let res = run(&tool, json!({ "operation": "delete_workspace" }));
        assert!(res.is_error);
        assert!(res.content.contains("Unknown operation"));

        // No request reached the backend for any rejected call.
        assert!(
            backend.snapshot().is_empty(),
            "backend must not be called on invalid input"
        );
    }

    #[test]
    fn schema_and_registration() {
        use crate::registry::ToolRegistry;

        let tool = LinearTool::default();
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
        assert_eq!(ops, LINEAR_OPERATIONS);

        // All operations are read-only → concurrency-safe.
        assert!(tool.is_concurrency_safe(&json!({ "operation": "query_issues" })));
        assert!(tool.is_concurrency_safe(&json!({ "operation": "query_teams" })));
        assert_eq!(tool.category(), ToolCategory::Info);

        // register_linear_tool populates the registry.
        let mut reg = ToolRegistry::new();
        register_linear_tool(&mut reg, Arc::new(NullLinearBackend));
        let names = reg.tool_names();
        assert!(
            names.iter().any(|n| n == "linear_api"),
            "linear_api missing from registry: {names:?}"
        );
    }
}
