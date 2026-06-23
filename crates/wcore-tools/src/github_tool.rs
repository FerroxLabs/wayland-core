//! v0.6.3 Tier 2B T3 — GitHub REST API operations tool.
//!
//! Ported from the prior Wayland Python engine. The Python
//! original talks to `api.github.com` directly (httpx + a `GITHUB_TOKEN`).
//! Mirroring the established `wcore-tools` discipline (see
//! [`discord_tool`](crate::discord_tool), [`web_tools`](crate::web_tools),
//! [`spotify_tool`](crate::spotify_tool)), this crate ships **no HTTP
//! client**: HTTP is a `wcore-providers` / host concern. This port covers
//! the **dispatch surface only** — schema, the typed [`GitHubOp`] enum,
//! per-operation required-parameter validation, request-shape
//! construction, and a pluggable [`GitHubBackend`] seam the host wires to
//! a real REST client (typically built on `wcore-providers::http_client`).
//!
//! Without a backend bound, `execute()` returns a structured error
//! ("no github backend configured …") rather than a silent stub —
//! honoring the NO-STUBS contract.
//!
//! ## Operations
//!
//! Five operations across read + write:
//!
//! * `get_issue` — read a single issue (`GET /repos/{o}/{r}/issues/{n}`)
//! * `get_pull_request` — read a single PR (`GET /repos/{o}/{r}/pulls/{n}`)
//! * `get_file_contents` — read a file's contents
//!   (`GET /repos/{o}/{r}/contents/{path}`)
//! * `create_comment` — post a comment on an issue/PR
//!   (`POST /repos/{o}/{r}/issues/{n}/comments`)
//! * `create_commit` — create or update a file, which produces a commit
//!   (`PUT /repos/{o}/{r}/contents/{path}`)
//!
//! ## Request-shape construction
//!
//! [`GitHubRequest`] is a pure, testable description of the HTTP call the
//! backend must make: method, fully-qualified URL, header pairs (incl.
//! the `Authorization` header), and an optional JSON body. The backend
//! receives this struct and is the only place that owns a transport.
//! [`GitHubOp::build_request`] is a pure function, so tests assert URL +
//! header + body construction without any network I/O.
//!
//! ## Auth
//!
//! The token is read from the tool input (`token`) or, if absent, the
//! `GITHUB_TOKEN` env var. It is sent as `Authorization: Bearer <token>`
//! — GitHub accepts both `Bearer` and the legacy `token` scheme; `Bearer`
//! is the current recommendation. Requests without a token are still
//! built (GitHub allows unauthenticated reads of public repos at a lower
//! rate limit); the `Authorization` header is simply omitted.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// GitHub REST API base URL.
pub const GITHUB_API_BASE: &str = "https://api.github.com";

/// `User-Agent` sent on every request — GitHub rejects requests without
/// one with `403 Forbidden`.
pub const GITHUB_USER_AGENT: &str = "wayland-core";

/// `X-GitHub-Api-Version` pin — keeps responses stable across GitHub's
/// dated API revisions.
pub const GITHUB_API_VERSION: &str = "2022-11-28";

/// Canonical operation set. Order is preserved in the schema enum so the
/// model sees a stable manifest.
pub const GITHUB_OPERATIONS: &[&str] = &[
    "get_issue",
    "get_pull_request",
    "get_file_contents",
    "create_comment",
    "create_commit",
    "search_repos",
];

// ---------------------------------------------------------------------
// Typed operation enum.
// ---------------------------------------------------------------------

/// A typed, validated GitHub operation. Each variant maps to exactly one
/// REST endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitHubOp {
    /// `GET /repos/{owner}/{repo}/issues/{number}`
    GetIssue {
        owner: String,
        repo: String,
        number: u64,
    },
    /// `GET /repos/{owner}/{repo}/pulls/{number}`
    GetPullRequest {
        owner: String,
        repo: String,
        number: u64,
    },
    /// `GET /repos/{owner}/{repo}/contents/{path}` — optional `ref`
    /// (branch / tag / commit SHA).
    GetFileContents {
        owner: String,
        repo: String,
        path: String,
        git_ref: Option<String>,
    },
    /// `POST /repos/{owner}/{repo}/issues/{number}/comments`
    CreateComment {
        owner: String,
        repo: String,
        number: u64,
        body: String,
    },
    /// `PUT /repos/{owner}/{repo}/contents/{path}` — creates a commit by
    /// creating or updating a file. `sha` is required when updating an
    /// existing file (GitHub rejects the update otherwise).
    CreateCommit {
        owner: String,
        repo: String,
        path: String,
        message: String,
        /// Raw (un-encoded) file content; the backend base64-encodes it.
        content: String,
        branch: Option<String>,
        sha: Option<String>,
    },
    /// `GET /search/repositories?q={query}&sort={sort}&order={order}&per_page={per_page}`
    /// — search public repositories.
    SearchRepos {
        query: String,
        sort: Option<String>,
        order: Option<String>,
        per_page: Option<u64>,
    },
}

/// HTTP method for a [`GitHubRequest`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
}

impl HttpMethod {
    /// The uppercase wire name (`"GET"` / `"POST"` / `"PUT"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
        }
    }
}

/// A fully-described HTTP request the backend must perform. Pure data —
/// no transport. Built by [`GitHubOp::build_request`].
#[derive(Debug, Clone, PartialEq)]
pub struct GitHubRequest {
    pub method: HttpMethod,
    pub url: String,
    /// Header name/value pairs, including `Authorization` when a token
    /// is present.
    pub headers: Vec<(String, String)>,
    /// JSON body for `POST` / `PUT`; `None` for `GET`.
    pub body: Option<Value>,
}

impl GitHubRequest {
    /// Convenience: look up a header value by case-insensitive name.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

/// Percent-encode a path segment for use in a GitHub `contents` URL.
/// GitHub path components keep `/` (directory separators) but every
/// other reserved / non-ASCII byte must be escaped.
fn encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for &byte in path.as_bytes() {
        match byte {
            // Unreserved per RFC 3986 plus `/` (kept as a separator).
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
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

/// Percent-encode a single URL path segment (`owner` / `repo`).
///
/// D.1 Round 1 (MEDIUM): `owner` / `repo` were interpolated into request
/// URLs raw while `path` was encoded — an inconsistency that let a
/// model-supplied `owner`/`repo` containing `/`, `..`, `?`, `#`, or
/// whitespace manipulate the URL path. Unlike [`encode_path`] this does
/// NOT keep `/` — an owner or repo name is a single segment, so a `/`
/// in it must be escaped.
fn encode_segment(seg: &str) -> String {
    let mut out = String::with_capacity(seg.len());
    for &byte in seg.as_bytes() {
        match byte {
            // Unreserved per RFC 3986. `/` is NOT kept — a segment is atomic.
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

impl GitHubOp {
    /// Build the pure [`GitHubRequest`] for this operation. `token`, when
    /// `Some`, is sent as `Authorization: Bearer <token>`.
    pub fn build_request(&self, token: Option<&str>) -> GitHubRequest {
        let mut headers: Vec<(String, String)> = vec![
            (
                "Accept".to_string(),
                "application/vnd.github+json".to_string(),
            ),
            ("User-Agent".to_string(), GITHUB_USER_AGENT.to_string()),
            (
                "X-GitHub-Api-Version".to_string(),
                GITHUB_API_VERSION.to_string(),
            ),
        ];
        if let Some(tok) = token.map(str::trim).filter(|t| !t.is_empty()) {
            headers.push(("Authorization".to_string(), format!("Bearer {tok}")));
        }

        // D.1 Round 1 (MEDIUM): owner / repo are percent-encoded as URL
        // path segments — consistently with `path` — so a model-supplied
        // value containing `/`, `..`, `?`, `#`, or whitespace cannot
        // manipulate the request URL.
        match self {
            GitHubOp::GetIssue {
                owner,
                repo,
                number,
            } => GitHubRequest {
                method: HttpMethod::Get,
                url: format!(
                    "{GITHUB_API_BASE}/repos/{}/{}/issues/{number}",
                    encode_segment(owner),
                    encode_segment(repo)
                ),
                headers,
                body: None,
            },
            GitHubOp::GetPullRequest {
                owner,
                repo,
                number,
            } => GitHubRequest {
                method: HttpMethod::Get,
                url: format!(
                    "{GITHUB_API_BASE}/repos/{}/{}/pulls/{number}",
                    encode_segment(owner),
                    encode_segment(repo)
                ),
                headers,
                body: None,
            },
            GitHubOp::GetFileContents {
                owner,
                repo,
                path,
                git_ref,
            } => {
                let mut url = format!(
                    "{GITHUB_API_BASE}/repos/{}/{}/contents/{}",
                    encode_segment(owner),
                    encode_segment(repo),
                    encode_path(path)
                );
                if let Some(r) = git_ref.as_deref().map(str::trim).filter(|r| !r.is_empty()) {
                    url.push_str("?ref=");
                    url.push_str(&encode_path(r));
                }
                GitHubRequest {
                    method: HttpMethod::Get,
                    url,
                    headers,
                    body: None,
                }
            }
            GitHubOp::CreateComment {
                owner,
                repo,
                number,
                body,
            } => GitHubRequest {
                method: HttpMethod::Post,
                url: format!(
                    "{GITHUB_API_BASE}/repos/{}/{}/issues/{number}/comments",
                    encode_segment(owner),
                    encode_segment(repo)
                ),
                headers,
                body: Some(json!({ "body": body })),
            },
            GitHubOp::CreateCommit {
                owner,
                repo,
                path,
                message,
                content,
                branch,
                sha,
            } => {
                let mut body = serde_json::Map::new();
                body.insert("message".to_string(), json!(message));
                // GitHub's contents API takes base64-encoded content.
                body.insert("content".to_string(), json!(base64_encode(content)));
                if let Some(b) = branch.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
                    body.insert("branch".to_string(), json!(b));
                }
                if let Some(s) = sha.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    body.insert("sha".to_string(), json!(s));
                }
                GitHubRequest {
                    method: HttpMethod::Put,
                    url: format!(
                        "{GITHUB_API_BASE}/repos/{}/{}/contents/{}",
                        encode_segment(owner),
                        encode_segment(repo),
                        encode_path(path)
                    ),
                    headers,
                    body: Some(Value::Object(body)),
                }
            }
            GitHubOp::SearchRepos {
                query,
                sort,
                order,
                per_page,
            } => {
                let mut url = format!(
                    "{GITHUB_API_BASE}/search/repositories?q={}",
                    encode_path(query)
                );
                if let Some(s) = sort.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
                    url.push_str("&sort=");
                    url.push_str(s);
                }
                if let Some(o) = order.as_deref().map(str::trim).filter(|o| !o.is_empty()) {
                    url.push_str("&order=");
                    url.push_str(o);
                }
                if let Some(pp) = per_page {
                    url.push_str(&format!("&per_page={pp}"));
                }
                GitHubRequest {
                    method: HttpMethod::Get,
                    url,
                    headers,
                    body: None,
                }
            }
        }
    }

    /// Whether this operation only reads (safe to run concurrently).
    pub fn is_read_only(&self) -> bool {
        matches!(
            self,
            GitHubOp::GetIssue { .. }
                | GitHubOp::GetPullRequest { .. }
                | GitHubOp::GetFileContents { .. }
                | GitHubOp::SearchRepos { .. }
        )
    }
}

/// Standard base64 (RFC 4648) encoder. `wcore-tools` ships no base64
/// crate, and the GitHub contents API needs `content` base64-encoded —
/// a ~20-line encoder avoids pulling a new workspace dependency.
fn base64_encode(input: &str) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[((triple >> 18) & 0x3F) as usize] as char);
        out.push(ALPHABET[((triple >> 12) & 0x3F) as usize] as char);
        out.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3F) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3F) as usize] as char
        } else {
            '='
        });
    }
    out
}

// ---------------------------------------------------------------------
// Backend seam.
// ---------------------------------------------------------------------

/// Outcome of a backend dispatch.
#[derive(Debug, Clone, PartialEq)]
pub enum GitHubOutcome {
    /// Success — `payload` is the parsed JSON the engine returns verbatim.
    Ok { payload: Value },
    /// GitHub returned a non-2xx status. `status` is the HTTP code,
    /// `message` is a human-readable explanation (typically GitHub's
    /// `{"message": ...}` field).
    HttpError { status: u16, message: String },
    /// Transport / auth-missing / any other failure path.
    Err { message: String },
}

/// Host-supplied GitHub backend. The engine never speaks HTTP; the host
/// implements this trait — typically wrapping a client built via
/// `wcore_providers::http_client::build()` — and binds it at
/// construction time. The backend receives a pre-built [`GitHubRequest`]
/// (URL + headers + body already assembled) and performs the call.
#[async_trait]
pub trait GitHubBackend: Send + Sync {
    /// Execute `request` against GitHub and return the parsed outcome.
    async fn dispatch(&self, request: &GitHubRequest) -> GitHubOutcome;
}

/// Default backend returned when the host wires nothing — every
/// `dispatch()` fails loudly so the tool never appears to succeed
/// silently (NO-STUBS contract).
pub struct NullGitHubBackend;

#[async_trait]
impl GitHubBackend for NullGitHubBackend {
    async fn dispatch(&self, _request: &GitHubRequest) -> GitHubOutcome {
        GitHubOutcome::Err {
            message: "No GitHub backend configured. Wire a GitHubBackend implementation \
                      (typically a wcore-providers http_client wrapper) when constructing \
                      GitHubTool to enable GitHub API operations."
                .to_string(),
        }
    }
}

/// In-memory backend that records every dispatched request and replays a
/// canned [`GitHubOutcome`]. Lives in the prod module so downstream
/// crates and tests can reuse it without `#[cfg(test)]` gymnastics —
/// mirrors `CapturingDiscordBackend`.
pub struct CapturingGitHubBackend {
    outcome: GitHubOutcome,
    pub captured: parking_lot::Mutex<Vec<GitHubRequest>>,
}

impl CapturingGitHubBackend {
    /// New backend that replays `outcome` on every dispatch.
    pub fn new(outcome: GitHubOutcome) -> Self {
        Self {
            outcome,
            captured: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// New backend that replays a successful `payload`.
    pub fn ok(payload: Value) -> Self {
        Self::new(GitHubOutcome::Ok { payload })
    }

    /// Snapshot of every request the tool has dispatched so far.
    pub fn snapshot(&self) -> Vec<GitHubRequest> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl GitHubBackend for CapturingGitHubBackend {
    async fn dispatch(&self, request: &GitHubRequest) -> GitHubOutcome {
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

fn u64_field(input: &Value, key: &str) -> Option<u64> {
    input.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_i64().filter(|n| *n >= 0).map(|n| n as u64))
            .or_else(|| v.as_str().and_then(|s| s.trim().parse::<u64>().ok()))
    })
}

/// Decode the JSON args object into a typed [`GitHubOp`]. Returns the
/// validation message on missing / invalid fields — run *before* the
/// backend is invoked.
fn parse_op(input: &Value) -> Result<GitHubOp, String> {
    let operation = str_field(input, "operation")
        .ok_or_else(|| "Missing required parameter: 'operation'".to_string())?
        .to_ascii_lowercase();

    let owner = || {
        str_field(input, "owner").ok_or_else(|| {
            format!("Missing required parameter 'owner' for operation '{operation}'")
        })
    };
    let repo = || {
        str_field(input, "repo")
            .ok_or_else(|| format!("Missing required parameter 'repo' for operation '{operation}'"))
    };
    let number = || {
        u64_field(input, "number").ok_or_else(|| {
            format!(
                "Missing or invalid required parameter 'number' (positive integer) for \
                 operation '{operation}'"
            )
        })
    };
    let path = || {
        str_field(input, "path")
            .ok_or_else(|| format!("Missing required parameter 'path' for operation '{operation}'"))
    };

    match operation.as_str() {
        "get_issue" => Ok(GitHubOp::GetIssue {
            owner: owner()?.to_string(),
            repo: repo()?.to_string(),
            number: number()?,
        }),
        "get_pull_request" => Ok(GitHubOp::GetPullRequest {
            owner: owner()?.to_string(),
            repo: repo()?.to_string(),
            number: number()?,
        }),
        "get_file_contents" => Ok(GitHubOp::GetFileContents {
            owner: owner()?.to_string(),
            repo: repo()?.to_string(),
            path: path()?.to_string(),
            git_ref: str_field(input, "ref").map(str::to_string),
        }),
        "create_comment" => Ok(GitHubOp::CreateComment {
            owner: owner()?.to_string(),
            repo: repo()?.to_string(),
            number: number()?,
            body: str_field(input, "body")
                .ok_or_else(|| {
                    "Missing required parameter 'body' for operation 'create_comment'".to_string()
                })?
                .to_string(),
        }),
        "create_commit" => Ok(GitHubOp::CreateCommit {
            owner: owner()?.to_string(),
            repo: repo()?.to_string(),
            path: path()?.to_string(),
            message: str_field(input, "message")
                .ok_or_else(|| {
                    "Missing required parameter 'message' for operation 'create_commit'".to_string()
                })?
                .to_string(),
            // `content` may legitimately be an empty string (e.g. an
            // empty file), so accept it even when blank — only require
            // the key to be present and a string.
            content: input
                .get("content")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    "Missing required parameter 'content' for operation 'create_commit'".to_string()
                })?
                .to_string(),
            branch: str_field(input, "branch").map(str::to_string),
            sha: str_field(input, "sha").map(str::to_string),
        }),
        "search_repos" => {
            let query = str_field(input, "query").ok_or_else(|| {
                "Missing required parameter 'query' for operation 'search_repos'".to_string()
            })?;
            Ok(GitHubOp::SearchRepos {
                query: query.to_string(),
                sort: str_field(input, "sort").map(str::to_string),
                order: str_field(input, "order").map(str::to_string),
                per_page: u64_field(input, "per_page"),
            })
        }
        other => Err(format!(
            "Unknown operation: '{other}'. Supported: {}",
            GITHUB_OPERATIONS.join(", ")
        )),
    }
}

// ---------------------------------------------------------------------
// Tool.
// ---------------------------------------------------------------------

/// `github_api` tool — GitHub REST API operations (issue / PR / file
/// reads + comment / commit writes).
pub struct GitHubTool {
    backend: Arc<dyn GitHubBackend>,
}

impl Default for GitHubTool {
    fn default() -> Self {
        Self::new(Arc::new(NullGitHubBackend))
    }
}

impl GitHubTool {
    /// New tool bound to `backend`.
    pub fn new(backend: Arc<dyn GitHubBackend>) -> Self {
        Self { backend }
    }

    /// Resolve the auth token: explicit `token` arg first, then the
    /// `GITHUB_TOKEN` env var.
    fn resolve_token(input: &Value) -> Option<String> {
        if let Some(t) = str_field(input, "token") {
            return Some(t.to_string());
        }
        std::env::var("GITHUB_TOKEN")
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
impl Tool for GitHubTool {
    fn name(&self) -> &str {
        "github_api"
    }

    fn description(&self) -> &str {
        "Operate on the GitHub REST API. Read an issue (get_issue), a pull request \
         (get_pull_request), or a file's contents (get_file_contents); post a comment on an \
         issue or PR (create_comment); or create/update a file to produce a commit \
         (create_commit). Auth via the 'token' argument or the GITHUB_TOKEN environment \
         variable."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": GITHUB_OPERATIONS,
                    "description": "Which GitHub operation to perform."
                },
                "owner": {
                    "type": "string",
                    "description": "Repository owner (user or organization). Required for all \
                                    operations except search_repos."
                },
                "repo": {
                    "type": "string",
                    "description": "Repository name. Required for all operations except search_repos."
                },
                "number": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Issue or pull-request number. Required for get_issue, \
                                    get_pull_request, create_comment."
                },
                "path": {
                    "type": "string",
                    "description": "File path within the repository. Required for \
                                    get_file_contents and create_commit."
                },
                "ref": {
                    "type": "string",
                    "description": "Optional branch, tag, or commit SHA for get_file_contents."
                },
                "body": {
                    "type": "string",
                    "description": "Comment text. Required for create_comment."
                },
                "message": {
                    "type": "string",
                    "description": "Commit message. Required for create_commit."
                },
                "content": {
                    "type": "string",
                    "description": "Raw file content (un-encoded). Required for create_commit."
                },
                "branch": {
                    "type": "string",
                    "description": "Optional target branch for create_commit (defaults to the \
                                    repository default branch)."
                },
                "sha": {
                    "type": "string",
                    "description": "Blob SHA of the file being replaced. Required by GitHub when \
                                    create_commit updates an existing file."
                },
                "token": {
                    "type": "string",
                    "description": "GitHub access token. Falls back to the GITHUB_TOKEN env var."
                },
                "query": {
                    "type": "string",
                    "description": "Search query string. Required for search_repos (e.g. \"rust async\" or \"topic:cli language:rust\")."
                },
                "sort": {
                    "type": "string",
                    "description": "Sort field for search_repos: stars, forks, help-wanted-issues, or updated. Defaults to best-match."
                },
                "order": {
                    "type": "string",
                    "description": "Sort order for search_repos: asc or desc (default desc)."
                },
                "per_page": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 100,
                    "description": "Results per page for search_repos (max 100, default 30)."
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
        // Includes mutating operations (comment / commit). Categorize as
        // Exec so hosts that gate side-effecting tools behind approval
        // catch this tool too.
        ToolCategory::Exec
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let op = match parse_op(&input) {
            Ok(op) => op,
            Err(e) => return err_result(e),
        };

        let token = Self::resolve_token(&input);
        let request = op.build_request(token.as_deref());

        match self.backend.dispatch(&request).await {
            GitHubOutcome::Ok { payload } => ToolResult {
                content: payload.to_string(),
                is_error: false,
            },
            GitHubOutcome::HttpError { status, message } => {
                err_result(format!("GitHub API error {status}: {message}"))
            }
            GitHubOutcome::Err { message } => err_result(message),
        }
    }
}

/// Register the GitHub tool into `registry`, bound to `backend`. Hosts
/// typically call this once at startup after resolving a GitHub token.
pub fn register_github_tool(
    registry: &mut crate::registry::ToolRegistry,
    backend: Arc<dyn GitHubBackend>,
) {
    registry.register(Box::new(GitHubTool::new(backend)));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(tool: &GitHubTool, input: Value) -> ToolResult {
        futures::executor::block_on(tool.execute(input))
    }

    fn parse_json(result: &ToolResult) -> Value {
        serde_json::from_str(&result.content).expect("tool result must be valid JSON")
    }

    // ----------------------------------------------------------------
    // base64 — sanity, since GitHub's contents API depends on it.
    // ----------------------------------------------------------------

    #[test]
    fn base64_encode_matches_known_vectors() {
        assert_eq!(base64_encode(""), "");
        assert_eq!(base64_encode("f"), "Zg==");
        assert_eq!(base64_encode("fo"), "Zm8=");
        assert_eq!(base64_encode("foo"), "Zm9v");
        assert_eq!(base64_encode("foob"), "Zm9vYg==");
        assert_eq!(base64_encode("hello world"), "aGVsbG8gd29ybGQ=");
    }

    // ----------------------------------------------------------------
    // Request URL + header construction per operation.
    // ----------------------------------------------------------------

    #[test]
    fn build_request_constructs_correct_urls_and_methods() {
        let issue = GitHubOp::GetIssue {
            owner: "rust-lang".into(),
            repo: "rust".into(),
            number: 42,
        }
        .build_request(None);
        assert_eq!(issue.method, HttpMethod::Get);
        assert_eq!(
            issue.url,
            "https://api.github.com/repos/rust-lang/rust/issues/42"
        );
        assert!(issue.body.is_none());

        let pr = GitHubOp::GetPullRequest {
            owner: "rust-lang".into(),
            repo: "rust".into(),
            number: 7,
        }
        .build_request(None);
        assert_eq!(
            pr.url,
            "https://api.github.com/repos/rust-lang/rust/pulls/7"
        );

        // File path with a directory separator + a space → space encoded,
        // `/` preserved.
        let file = GitHubOp::GetFileContents {
            owner: "o".into(),
            repo: "r".into(),
            path: "src/my file.rs".into(),
            git_ref: Some("main".into()),
        }
        .build_request(None);
        assert_eq!(
            file.url,
            "https://api.github.com/repos/o/r/contents/src/my%20file.rs?ref=main"
        );

        let comment = GitHubOp::CreateComment {
            owner: "o".into(),
            repo: "r".into(),
            number: 3,
            body: "looks good".into(),
        }
        .build_request(None);
        assert_eq!(comment.method, HttpMethod::Post);
        assert_eq!(
            comment.url,
            "https://api.github.com/repos/o/r/issues/3/comments"
        );
        assert_eq!(comment.body, Some(json!({ "body": "looks good" })));

        let commit = GitHubOp::CreateCommit {
            owner: "o".into(),
            repo: "r".into(),
            path: "README.md".into(),
            message: "docs: update".into(),
            content: "hello world".into(),
            branch: Some("feat/x".into()),
            sha: Some("abc123".into()),
        }
        .build_request(None);
        assert_eq!(commit.method, HttpMethod::Put);
        assert_eq!(
            commit.url,
            "https://api.github.com/repos/o/r/contents/README.md"
        );
        let body = commit.body.expect("commit has a body");
        assert_eq!(body["message"], json!("docs: update"));
        // content must be base64-encoded.
        assert_eq!(body["content"], json!("aGVsbG8gd29ybGQ="));
        assert_eq!(body["branch"], json!("feat/x"));
        assert_eq!(body["sha"], json!("abc123"));
    }

    #[test]
    fn build_request_percent_encodes_owner_and_repo() {
        // D.1 Round 1 (MEDIUM): owner / repo with URL-significant chars
        // must be percent-encoded — a `/` in owner cannot smuggle an
        // extra path segment, a `?`/`#` cannot start a query/fragment.
        let issue = GitHubOp::GetIssue {
            owner: "a/b".into(),
            repo: "c d?x".into(),
            number: 1,
        }
        .build_request(None);
        assert_eq!(
            issue.url, "https://api.github.com/repos/a%2Fb/c%20d%3Fx/issues/1",
            "owner/repo must be percent-encoded as single segments"
        );

        // A `..` in repo is preserved literally (encoded chars only) and
        // the slash that would let `..` act as traversal is escaped.
        let file = GitHubOp::GetFileContents {
            owner: "o".into(),
            repo: "../../etc".into(),
            path: "f.rs".into(),
            git_ref: None,
        }
        .build_request(None);
        assert_eq!(
            file.url,
            "https://api.github.com/repos/o/..%2F..%2Fetc/contents/f.rs"
        );

        let commit = GitHubOp::CreateCommit {
            owner: "o w".into(),
            repo: "r".into(),
            path: "README.md".into(),
            message: "m".into(),
            content: "c".into(),
            branch: None,
            sha: None,
        }
        .build_request(None);
        assert_eq!(
            commit.url,
            "https://api.github.com/repos/o%20w/r/contents/README.md"
        );
    }

    #[test]
    fn build_request_sets_auth_and_standard_headers() {
        // With a token → Authorization: Bearer present.
        let req = GitHubOp::GetIssue {
            owner: "o".into(),
            repo: "r".into(),
            number: 1,
        }
        .build_request(Some("ghp_secrettoken"));
        assert_eq!(req.header("Authorization"), Some("Bearer ghp_secrettoken"));
        assert_eq!(req.header("accept"), Some("application/vnd.github+json"));
        assert_eq!(req.header("User-Agent"), Some(GITHUB_USER_AGENT));
        assert_eq!(req.header("X-GitHub-Api-Version"), Some(GITHUB_API_VERSION));

        // Without a token → no Authorization header (unauth public read).
        let anon = GitHubOp::GetIssue {
            owner: "o".into(),
            repo: "r".into(),
            number: 1,
        }
        .build_request(None);
        assert!(anon.header("Authorization").is_none());
        // A blank/whitespace token is treated as absent.
        let blank = GitHubOp::GetIssue {
            owner: "o".into(),
            repo: "r".into(),
            number: 1,
        }
        .build_request(Some("   "));
        assert!(blank.header("Authorization").is_none());
    }

    // ----------------------------------------------------------------
    // Response JSON parsing from fixture payloads.
    // ----------------------------------------------------------------

    #[test]
    fn execute_returns_parsed_fixture_payload_for_get_issue() {
        // Fixture mirrors the shape of a real GitHub issue response.
        let fixture = json!({
            "number": 42,
            "title": "A bug",
            "state": "open",
            "body": "steps to reproduce",
            "user": { "login": "octocat" }
        });
        let backend = Arc::new(CapturingGitHubBackend::ok(fixture.clone()));
        let tool = GitHubTool::new(backend.clone());
        let res = run(
            &tool,
            json!({
                "operation": "get_issue",
                "owner": "rust-lang",
                "repo": "rust",
                "number": 42,
                "token": "ghp_x"
            }),
        );
        assert!(!res.is_error, "expected ok, got: {}", res.content);
        let v = parse_json(&res);
        assert_eq!(v["number"], json!(42));
        assert_eq!(v["title"], json!("A bug"));
        assert_eq!(v["user"]["login"], json!("octocat"));

        // The backend saw exactly one correctly-built request.
        let reqs = backend.snapshot();
        assert_eq!(reqs.len(), 1);
        assert_eq!(
            reqs[0].url,
            "https://api.github.com/repos/rust-lang/rust/issues/42"
        );
        assert_eq!(reqs[0].header("Authorization"), Some("Bearer ghp_x"));
    }

    // ----------------------------------------------------------------
    // Error path — 404 / bad token.
    // ----------------------------------------------------------------

    #[test]
    fn execute_surfaces_http_error_for_404() {
        let backend = Arc::new(CapturingGitHubBackend::new(GitHubOutcome::HttpError {
            status: 404,
            message: "Not Found".to_string(),
        }));
        let tool = GitHubTool::new(backend);
        let res = run(
            &tool,
            json!({
                "operation": "get_issue",
                "owner": "o",
                "repo": "missing",
                "number": 1
            }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("GitHub API error 404"));
        assert!(res.content.contains("Not Found"));
    }

    #[test]
    fn null_backend_fails_loud_no_silent_stub() {
        let tool = GitHubTool::default();
        let res = run(
            &tool,
            json!({
                "operation": "get_pull_request",
                "owner": "o",
                "repo": "r",
                "number": 1
            }),
        );
        assert!(res.is_error);
        assert!(
            res.content.contains("No GitHub backend configured"),
            "expected fail-loud, got: {}",
            res.content
        );
    }

    // ----------------------------------------------------------------
    // Input-schema validation.
    // ----------------------------------------------------------------

    #[test]
    fn invalid_input_rejected_before_backend() {
        let backend = Arc::new(CapturingGitHubBackend::ok(json!({})));

        // Missing operation.
        let tool = GitHubTool::new(backend.clone());
        let res = run(&tool, json!({ "owner": "o", "repo": "r" }));
        assert!(res.is_error);
        assert!(res.content.contains("'operation'"));

        // Unknown operation.
        let res = run(
            &tool,
            json!({ "operation": "delete_repo", "owner": "o", "repo": "r" }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("Unknown operation"));

        // get_issue missing 'number'.
        let res = run(
            &tool,
            json!({ "operation": "get_issue", "owner": "o", "repo": "r" }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("'number'"));

        // create_comment missing 'body'.
        let res = run(
            &tool,
            json!({
                "operation": "create_comment",
                "owner": "o",
                "repo": "r",
                "number": 1
            }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("'body'"));

        // create_commit missing 'content'.
        let res = run(
            &tool,
            json!({
                "operation": "create_commit",
                "owner": "o",
                "repo": "r",
                "path": "f.txt",
                "message": "m"
            }),
        );
        assert!(res.is_error);
        assert!(res.content.contains("'content'"));

        // No request reached the backend for any rejected call.
        assert!(
            backend.snapshot().is_empty(),
            "backend must not be called on invalid input"
        );
    }

    #[test]
    fn schema_and_concurrency_safety() {
        let tool = GitHubTool::default();
        let schema = tool.input_schema();
        let required: Vec<&str> = schema["required"]
            .as_array()
            .expect("required array")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        // Only `operation` is universally required; owner/repo/etc are op-specific
        // (e.g. SearchRepos uses `query` and does not take owner/repo).
        assert!(required.contains(&"operation"));
        let ops: Vec<&str> = schema["properties"]["operation"]["enum"]
            .as_array()
            .expect("operation enum")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(ops, GITHUB_OPERATIONS);

        // Reads are concurrency-safe; writes are not.
        assert!(tool.is_concurrency_safe(&json!({
            "operation": "get_issue", "owner": "o", "repo": "r", "number": 1
        })));
        assert!(tool.is_concurrency_safe(&json!({
            "operation": "get_file_contents", "owner": "o", "repo": "r", "path": "f"
        })));
        assert!(!tool.is_concurrency_safe(&json!({
            "operation": "create_comment", "owner": "o", "repo": "r",
            "number": 1, "body": "x"
        })));
        assert!(!tool.is_concurrency_safe(&json!({
            "operation": "create_commit", "owner": "o", "repo": "r",
            "path": "f", "message": "m", "content": "c"
        })));
    }

    #[test]
    fn register_github_tool_populates_registry() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        register_github_tool(&mut reg, Arc::new(NullGitHubBackend));
        let names = reg.tool_names();
        assert!(
            names.iter().any(|n| n == "github_api"),
            "github_api missing from registry: {names:?}"
        );
    }
}
