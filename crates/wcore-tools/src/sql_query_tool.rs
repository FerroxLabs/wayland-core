//! v0.6.3 T1 -- `sql_query` database query tool.
//!
//! A tool that opens a connection to a SQL database, runs a single query,
//! and returns the result set as a text table truncated to sane row and
//! byte caps.
//!
//! ## Backend support
//!
//! SQLite is supported out of the box via the workspace `rusqlite` dep
//! (built with the `bundled` feature, so no system SQLite is required).
//! The `database` argument is a SQLite file path, or the literal
//! `:memory:` for a throwaway in-memory database.
//!
//! Postgres / MySQL are deliberately **out of scope** for this tool. They
//! would pull heavy native drivers into every build; if a future task
//! adds them they must be gated behind the `sql-extra` cargo feature with
//! optional deps so they never become a default dependency.
//!
//! ## Read-only posture
//!
//! This tool is intended for *read* queries. Statements whose first
//! keyword is an obviously-mutating verb (`INSERT`, `UPDATE`, `DELETE`,
//! `DROP`, `ALTER`, `CREATE`, `REPLACE`, `TRUNCATE`, `ATTACH`, `DETACH`,
//! `PRAGMA`, `VACUUM`) are rejected at the dispatch layer before the
//! database is touched. This is a best-effort guard, not a security
//! boundary: the SQLite connection itself is not opened read-only because
//! tests use `:memory:` databases that must be seeded first. Callers that
//! need a hard guarantee should point the tool at a read-only file or a
//! database the agent's OS user cannot write.
//!
//! ## Truncation
//!
//! Result sets are capped two ways, mirroring `tool_output_limits.rs`:
//!
//! * at most [`MAX_ROWS`] rows are formatted; extra rows are summarised.
//! * the rendered table is truncated to [`DEFAULT_MAX_BYTES`] bytes on a
//!   char boundary via [`crate::truncate_utf8`].

use async_trait::async_trait;
use rusqlite::Connection;
use rusqlite::types::ValueRef;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::tool_output_limits::DEFAULT_MAX_BYTES;
use crate::truncate_utf8;

/// Maximum number of result rows formatted into the output table.
/// Rows past this cap are dropped and summarised with a `... N more rows`
/// line so the LLM still learns the result set was larger.
pub const MAX_ROWS: usize = 200;

/// Statement-leading keywords that mark an obviously-mutating query.
/// Matched case-insensitively against the first whitespace-delimited
/// token of the (comment-stripped) SQL. See the module docs for posture.
const MUTATING_KEYWORDS: &[&str] = &[
    "insert", "update", "delete", "drop", "alter", "create", "replace", "truncate", "attach",
    "detach", "pragma", "vacuum",
];

/// `sql_query` tool -- runs a single SQL query against a SQLite database.
///
/// Stateless: a fresh connection is opened per call and closed when the
/// call returns, so the tool is cheap to construct and safe to share.
#[derive(Debug, Clone, Copy, Default)]
pub struct SqlQueryTool;

impl SqlQueryTool {
    pub fn new() -> Self {
        Self
    }
}

fn err_result(message: impl Into<String>) -> ToolResult {
    ToolResult {
        content: json!({ "success": false, "error": message.into() }).to_string(),
        is_error: true,
    }
}

/// Extract a required, non-empty string argument.
fn req_str(input: &Value, key: &str) -> Result<String, ToolResult> {
    match input.get(key).and_then(Value::as_str).map(str::trim) {
        Some(s) if !s.is_empty() => Ok(s.to_string()),
        _ => Err(err_result(format!(
            "Missing or empty required parameter: '{key}'"
        ))),
    }
}

/// Strip leading SQL line/block comments and whitespace, then return the
/// lowercased first token. Used by the read-only guard so a query like
/// `-- note\n  SELECT 1` is still recognised as a `select`.
fn leading_keyword(sql: &str) -> String {
    let mut rest = sql.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            // Line comment: drop through the next newline (or to the end).
            rest = match after.find('\n') {
                Some(nl) => after[nl + 1..].trim_start(),
                None => "",
            };
        } else if let Some(after) = rest.strip_prefix("/*") {
            // Block comment: drop through the closing `*/`.
            rest = match after.find("*/") {
                Some(end) => after[end + 2..].trim_start(),
                None => "",
            };
        } else {
            break;
        }
    }
    rest.split(|c: char| c.is_whitespace() || c == '(' || c == ';')
        .find(|t| !t.is_empty())
        .unwrap_or("")
        .to_ascii_lowercase()
}

/// Reject obviously-mutating statements. Best-effort, not a security
/// boundary -- see the module-level docs.
fn reject_if_mutating(sql: &str) -> Result<(), ToolResult> {
    let kw = leading_keyword(sql);
    if MUTATING_KEYWORDS.contains(&kw.as_str()) {
        return Err(err_result(format!(
            "Query rejected: '{kw}' statements are not allowed. The sql_query \
             tool is read-only; use SELECT (or WITH ... SELECT)."
        )));
    }
    Ok(())
}

/// Render one SQLite cell as a display string.
fn cell_to_string(v: ValueRef<'_>) -> String {
    match v {
        ValueRef::Null => "NULL".to_string(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(f) => f.to_string(),
        ValueRef::Text(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        ValueRef::Blob(bytes) => format!("<blob {} bytes>", bytes.len()),
    }
}

/// Outcome of running a query: column headers + up to `MAX_ROWS` rows
/// plus the total row count (which may exceed the rows vector length).
struct QueryOutcome {
    columns: Vec<String>,
    rows: Vec<Vec<String>>,
    total_rows: usize,
}

/// Run the query synchronously. Lives in a free fn so it can be wrapped
/// in `spawn_blocking` -- rusqlite is a blocking API.
fn run_query(database: &str, sql: &str) -> Result<QueryOutcome, String> {
    let conn = Connection::open(database).map_err(|e| format!("Failed to open database: {e}"))?;
    let mut stmt = conn
        .prepare(sql)
        .map_err(|e| format!("Failed to prepare query: {e}"))?;

    let columns: Vec<String> = stmt.column_names().iter().map(|c| c.to_string()).collect();
    let col_count = columns.len();

    let mut query_rows = stmt
        .query([])
        .map_err(|e| format!("Failed to execute query: {e}"))?;

    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut total_rows = 0usize;
    while let Some(row) = query_rows
        .next()
        .map_err(|e| format!("Failed to read row: {e}"))?
    {
        total_rows += 1;
        if rows.len() < MAX_ROWS {
            let mut cells = Vec::with_capacity(col_count);
            for i in 0..col_count {
                let v = row
                    .get_ref(i)
                    .map_err(|e| format!("Failed to read column {i}: {e}"))?;
                cells.push(cell_to_string(v));
            }
            rows.push(cells);
        }
    }

    Ok(QueryOutcome {
        columns,
        rows,
        total_rows,
    })
}

/// Format a [`QueryOutcome`] into a plain-text table with a header row,
/// a separator, and a trailing row-count summary.
fn format_outcome(outcome: &QueryOutcome) -> String {
    let QueryOutcome {
        columns,
        rows,
        total_rows,
    } = outcome;

    if columns.is_empty() {
        return format!("Query OK. {total_rows} row(s).");
    }

    let col_count = columns.len();
    // Column widths: max of the header and every formatted cell.
    let mut widths: Vec<usize> = columns.iter().map(|c| c.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(col_count) {
            let w = cell.chars().count();
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }

    let pad = |s: &str, w: usize| -> String {
        let len = s.chars().count();
        if len >= w {
            s.to_string()
        } else {
            format!("{s}{}", " ".repeat(w - len))
        }
    };

    let mut out = String::new();
    // Header.
    let header: Vec<String> = columns
        .iter()
        .enumerate()
        .map(|(i, c)| pad(c, widths[i]))
        .collect();
    out.push_str(&header.join(" | "));
    out.push('\n');
    // Separator.
    let sep: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    out.push_str(&sep.join("-+-"));
    out.push('\n');
    // Rows.
    for row in rows {
        let cells: Vec<String> = (0..col_count)
            .map(|i| pad(row.get(i).map(String::as_str).unwrap_or(""), widths[i]))
            .collect();
        out.push_str(&cells.join(" | "));
        out.push('\n');
    }

    // Row-count summary, including truncation notice if applicable.
    if *total_rows > rows.len() {
        out.push_str(&format!(
            "\n({} rows total; {} shown, {} more rows truncated)",
            total_rows,
            rows.len(),
            total_rows - rows.len()
        ));
    } else {
        out.push_str(&format!("\n({} row(s))", total_rows));
    }
    out
}

#[async_trait]
impl Tool for SqlQueryTool {
    fn name(&self) -> &str {
        "sql_query"
    }

    fn description(&self) -> &str {
        "Run a read-only SQL query against a SQLite database and return the \
         results as a text table. The 'database' argument is a SQLite file \
         path (or ':memory:' for a throwaway database). Only SELECT-style \
         queries are allowed: INSERT/UPDATE/DELETE/DROP and other mutating \
         statements are rejected. Large result sets are truncated to a row \
         and byte cap. Postgres/MySQL are not supported."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "database": {
                    "type": "string",
                    "description": "Path to the SQLite database file, or ':memory:' for an in-memory database.",
                },
                "query": {
                    "type": "string",
                    "description": "The SQL query to run. Must be a read-only (SELECT-style) statement.",
                },
            },
            "required": ["database", "query"],
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Read-only by posture: every accepted query is non-mutating.
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let database = match req_str(&input, "database") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let query = match req_str(&input, "query") {
            Ok(s) => s,
            Err(e) => return e,
        };

        if let Err(e) = reject_if_mutating(&query) {
            return e;
        }

        // rusqlite is blocking; run it off the async runtime.
        let result = tokio::task::spawn_blocking(move || run_query(&database, &query)).await;

        let outcome = match result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(msg)) => return err_result(msg),
            Err(join_err) => return err_result(format!("Query task failed: {join_err}")),
        };

        let table = format_outcome(&outcome);
        let truncated = truncate_utf8(&table, DEFAULT_MAX_BYTES);
        let byte_truncated = truncated.len() < table.len();
        let content = if byte_truncated {
            format!("{truncated}\n... [output truncated at {DEFAULT_MAX_BYTES} bytes]")
        } else {
            truncated.to_string()
        };

        ToolResult {
            content,
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build an in-memory database, seed it, then run `query` against the
    /// *same* connection-backing path. Each `:memory:` open is a distinct
    /// database, so seeding + querying in one call is not possible across
    /// two `execute()` invocations -- tests that need seeded data use a
    /// tempfile instead. This helper covers the seed-and-query-in-one-go
    /// case by running a multi-statement seed directly.
    fn seeded_tempfile() -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().expect("create temp db file");
        let conn = Connection::open(file.path()).expect("open temp db");
        conn.execute_batch(
            "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER);
             INSERT INTO users (name, age) VALUES ('alice', 30), ('bob', 25), ('carol', 41);",
        )
        .expect("seed temp db");
        file
    }

    #[tokio::test]
    async fn connect_and_simple_select() {
        let tool = SqlQueryTool::new();
        // `:memory:` is fine for a query that needs no seeded tables.
        let res = tool
            .execute(json!({"database": ":memory:", "query": "SELECT 1 AS n"}))
            .await;
        assert!(!res.is_error, "simple SELECT should succeed: {res:?}");
        assert!(res.content.contains('n'), "header column present");
        assert!(res.content.contains('1'), "value present");
        assert!(res.content.contains("1 row(s)"), "row count summary");
    }

    #[tokio::test]
    async fn multi_column_result_formatting() {
        let file = seeded_tempfile();
        let tool = SqlQueryTool::new();
        let res = tool
            .execute(json!({
                "database": file.path().to_str().unwrap(),
                "query": "SELECT name, age FROM users ORDER BY age",
            }))
            .await;
        assert!(!res.is_error, "multi-column select should succeed: {res:?}");
        assert!(res.content.contains("name"), "name header");
        assert!(res.content.contains("age"), "age header");
        assert!(res.content.contains("bob"), "row data present");
        assert!(res.content.contains(" | "), "column separator present");
        assert!(res.content.contains("3 row(s)"), "all three rows counted");
    }

    #[tokio::test]
    async fn truncation_kicks_in_past_the_cap() {
        let file = tempfile::NamedTempFile::new().expect("temp db");
        let conn = Connection::open(file.path()).expect("open");
        conn.execute_batch("CREATE TABLE big (id INTEGER);")
            .expect("create");
        // Insert more rows than MAX_ROWS.
        let total = MAX_ROWS + 50;
        let tx = conn.unchecked_transaction().expect("tx");
        for i in 0..total {
            tx.execute("INSERT INTO big (id) VALUES (?1)", [i as i64])
                .expect("insert");
        }
        tx.commit().expect("commit");
        drop(conn);

        let tool = SqlQueryTool::new();
        let res = tool
            .execute(json!({
                "database": file.path().to_str().unwrap(),
                "query": "SELECT id FROM big",
            }))
            .await;
        assert!(!res.is_error, "query should succeed: {res:?}");
        assert!(
            res.content.contains("truncated"),
            "row truncation notice expected, got: {}",
            res.content
        );
        assert!(
            res.content.contains(&total.to_string()),
            "total row count should be reported"
        );
    }

    #[tokio::test]
    async fn bad_sql_returns_error() {
        let tool = SqlQueryTool::new();
        let res = tool
            .execute(json!({"database": ":memory:", "query": "SELEKT bogus FROM nowhere"}))
            .await;
        assert!(res.is_error, "malformed SQL must be an error");
        assert!(
            res.content.contains("prepare") || res.content.contains("error"),
            "error message should mention the failure: {}",
            res.content
        );
    }

    #[tokio::test]
    async fn empty_result_set() {
        let file = seeded_tempfile();
        let tool = SqlQueryTool::new();
        let res = tool
            .execute(json!({
                "database": file.path().to_str().unwrap(),
                "query": "SELECT name FROM users WHERE age > 999",
            }))
            .await;
        assert!(!res.is_error, "empty result is not an error: {res:?}");
        assert!(res.content.contains("0 row(s)"), "zero-row summary");
    }

    #[tokio::test]
    async fn mutating_statements_are_rejected() {
        let tool = SqlQueryTool::new();
        for stmt in [
            "INSERT INTO users VALUES (1)",
            "  delete from users",
            "-- comment\nDROP TABLE users",
            "/* block */ UPDATE users SET age = 0",
        ] {
            let res = tool
                .execute(json!({"database": ":memory:", "query": stmt}))
                .await;
            assert!(res.is_error, "mutating statement must be rejected: {stmt}");
            assert!(
                res.content.contains("read-only"),
                "rejection should explain the read-only posture: {}",
                res.content
            );
        }
    }

    #[tokio::test]
    async fn missing_parameters_error() {
        let tool = SqlQueryTool::new();
        let res = tool.execute(json!({"query": "SELECT 1"})).await;
        assert!(res.is_error, "missing database param must error");
        let res = tool.execute(json!({"database": ":memory:"})).await;
        assert!(res.is_error, "missing query param must error");
    }

    #[test]
    fn leading_keyword_strips_comments() {
        assert_eq!(leading_keyword("SELECT 1"), "select");
        assert_eq!(leading_keyword("  -- note\n select 2"), "select");
        assert_eq!(leading_keyword("/* a */ WITH x AS (SELECT 1)"), "with");
        assert_eq!(leading_keyword("insert into t"), "insert");
    }

    #[test]
    fn tool_registers_in_registry() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        reg.register(Box::new(SqlQueryTool::new()));
        let defs = reg.to_tool_defs();
        let found = defs.iter().find(|d| d.name == "sql_query");
        assert!(found.is_some(), "sql_query must be present in registry");
        let required = found.unwrap().input_schema["required"]
            .as_array()
            .expect("required array");
        let required_strs: Vec<&str> = required.iter().filter_map(Value::as_str).collect();
        assert!(required_strs.contains(&"database"));
        assert!(required_strs.contains(&"query"));
    }
}
