//! v0.6.3 Tier 2B (T2) — `postgres_schema` introspection tool.
//!
//! A read-only tool that introspects a Postgres database schema:
//!
//! * `list_tables`        — every base table in a schema.
//! * `list_columns`       — columns of one table (name, type, nullable,
//!   default, ordinal position).
//! * `list_foreign_keys`  — foreign-key constraints of one table
//!   (constraint name, local column, referenced table + column).
//!
//! All three operations query the SQL-standard `information_schema`
//! views, so the queries are portable across Postgres versions and
//! require no `SUPERUSER` / `pg_catalog` access beyond the catalogs a
//! normal role can read.
//!
//! ## Seam discipline (mirror of `homeassistant_tool.rs` / `spotify_tool.rs`)
//!
//! Wayland's engine deliberately ships **no embedded database client**
//! by default — Postgres client crates pull native libs (`libpq` for
//! `postgres`, a TLS stack for `tokio-postgres`) that are heavy and
//! platform-sensitive. So:
//!
//! * [`PostgresSchemaBackend`] — async trait with one entry point. The
//!   host wires a concrete implementation against its chosen client.
//! * [`NullPostgresSchemaBackend`] — default fail-loud backend. Returns
//!   a structured "postgres feature not enabled" error on every call so
//!   the tool never silently appears to succeed (NO-STUBS contract).
//! * [`CapturingPostgresSchemaBackend`] — hermetic in-memory test
//!   double; lives in the prod module so downstream crates can reuse it.
//!
//! The actual `tokio-postgres` driver lives behind the optional
//! `postgres` cargo feature (see `[features]` in `Cargo.toml`). When the
//! feature is **off** the tool still compiles and registers; it simply
//! fails loudly via `NullPostgresSchemaBackend` until a host wires a
//! real backend. When the feature is **on**, [`live::TokioPostgresBackend`]
//! becomes available — a thin connect-and-query implementation hosts can
//! use directly.
//!
//! ## Why the SQL is a pure function
//!
//! The three introspection queries are deterministic strings built by
//! [`tables_query`], [`columns_query`], and [`foreign_keys_query`].
//! Keeping them pure means the exact text is unit-testable without any
//! database, and a misbehaving backend cannot substitute a different
//! query — the dispatch layer owns the SQL, the backend only owns the
//! connection.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

// ---------------------------------------------------------------------
// Introspection SQL — pure, deterministic query builders.
// ---------------------------------------------------------------------

/// SQL that lists every base table in `schema`. The schema name is bound
/// as parameter `$1` so the query string itself carries no user input
/// (no SQL injection surface). Ordered for stable output.
pub fn tables_query() -> &'static str {
    "SELECT table_name \
     FROM information_schema.tables \
     WHERE table_schema = $1 AND table_type = 'BASE TABLE' \
     ORDER BY table_name"
}

/// SQL that lists the columns of one table. `$1` = schema, `$2` = table.
pub fn columns_query() -> &'static str {
    "SELECT column_name, data_type, is_nullable, column_default, ordinal_position \
     FROM information_schema.columns \
     WHERE table_schema = $1 AND table_name = $2 \
     ORDER BY ordinal_position"
}

/// SQL that lists the foreign-key constraints of one table, joining
/// `table_constraints` -> `key_column_usage` -> `constraint_column_usage`
/// to resolve the referenced table + column. `$1` = schema, `$2` = table.
pub fn foreign_keys_query() -> &'static str {
    "SELECT tc.constraint_name, kcu.column_name, \
     ccu.table_name AS foreign_table_name, ccu.column_name AS foreign_column_name \
     FROM information_schema.table_constraints AS tc \
     JOIN information_schema.key_column_usage AS kcu \
     ON tc.constraint_name = kcu.constraint_name \
     AND tc.table_schema = kcu.table_schema \
     JOIN information_schema.constraint_column_usage AS ccu \
     ON ccu.constraint_name = tc.constraint_name \
     AND ccu.table_schema = tc.table_schema \
     WHERE tc.constraint_type = 'FOREIGN KEY' \
     AND tc.table_schema = $1 AND tc.table_name = $2 \
     ORDER BY tc.constraint_name, kcu.column_name"
}

// ---------------------------------------------------------------------
// Typed operations + backend seam.
// ---------------------------------------------------------------------

/// A typed, validated schema-introspection request handed to the
/// backend. The dispatch layer builds exactly one of these; the backend
/// never sees raw tool input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PostgresSchemaOp {
    /// List base tables in `schema`.
    ListTables { schema: String },
    /// List columns of `schema.table`.
    ListColumns { schema: String, table: String },
    /// List foreign keys of `schema.table`.
    ListForeignKeys { schema: String, table: String },
}

impl PostgresSchemaOp {
    /// The exact SQL this op runs. Pure — handy for tests and for hosts
    /// that want to log the query before executing it.
    pub fn sql(&self) -> &'static str {
        match self {
            PostgresSchemaOp::ListTables { .. } => tables_query(),
            PostgresSchemaOp::ListColumns { .. } => columns_query(),
            PostgresSchemaOp::ListForeignKeys { .. } => foreign_keys_query(),
        }
    }

    /// The ordered positional bind parameters (`$1`, `$2`, ...) for `sql()`.
    pub fn params(&self) -> Vec<&str> {
        match self {
            PostgresSchemaOp::ListTables { schema } => vec![schema.as_str()],
            PostgresSchemaOp::ListColumns { schema, table }
            | PostgresSchemaOp::ListForeignKeys { schema, table } => {
                vec![schema.as_str(), table.as_str()]
            }
        }
    }
}

/// Outcome of a backend dispatch. `Ok` carries an array of rows, each a
/// JSON object keyed by column name.
#[derive(Debug, Clone)]
pub enum PostgresSchemaOutcome {
    /// A successful query result: a list of row objects.
    Ok(Vec<Value>),
    /// A failed query: connection error, permission error, etc.
    Err(String),
}

/// Pluggable Postgres schema-introspection backend. The host implements
/// this against its chosen client (`tokio-postgres`, `postgres`, a
/// connection pool, ...) and wires it at tool-construction time.
#[async_trait]
pub trait PostgresSchemaBackend: Send + Sync {
    /// Run a single introspection op and return its rows.
    async fn run(&self, op: PostgresSchemaOp) -> PostgresSchemaOutcome;
}

/// Default fail-loud backend. Honors the NO-STUBS contract: every call
/// returns a clear, actionable error rather than silently succeeding.
pub struct NullPostgresSchemaBackend;

#[async_trait]
impl PostgresSchemaBackend for NullPostgresSchemaBackend {
    async fn run(&self, _op: PostgresSchemaOp) -> PostgresSchemaOutcome {
        PostgresSchemaOutcome::Err(
            "No Postgres backend configured. Build wcore-tools with the `postgres` cargo \
             feature and wire a PostgresSchemaBackend (e.g. live::TokioPostgresBackend) \
             when constructing PostgresSchemaTool to enable schema introspection."
                .to_string(),
        )
    }
}

/// In-memory backend that records every dispatched op and returns canned
/// rows. Lives in the prod module so downstream crates can reuse it
/// without `#[cfg(test)]` gymnastics.
pub struct CapturingPostgresSchemaBackend {
    rows: Vec<Value>,
    /// Every op passed to [`run`](Self::run), in call order.
    pub captured: Mutex<Vec<PostgresSchemaOp>>,
}

impl CapturingPostgresSchemaBackend {
    /// Construct a backend that returns `canned_rows` for every op.
    pub fn new(canned_rows: Vec<Value>) -> Self {
        Self {
            rows: canned_rows,
            captured: Mutex::new(Vec::new()),
        }
    }

    /// Snapshot the ops captured so far.
    pub fn snapshot(&self) -> Vec<PostgresSchemaOp> {
        self.captured.lock().clone()
    }
}

#[async_trait]
impl PostgresSchemaBackend for CapturingPostgresSchemaBackend {
    async fn run(&self, op: PostgresSchemaOp) -> PostgresSchemaOutcome {
        self.captured.lock().push(op);
        PostgresSchemaOutcome::Ok(self.rows.clone())
    }
}

// ---------------------------------------------------------------------
// Row parsing — pure, fixture-testable.
// ---------------------------------------------------------------------

/// Pull a string field from a row object, defaulting to `""` when the
/// field is absent or non-string (Postgres always supplies these for the
/// `information_schema` columns we select, but parsing stays defensive).
fn row_str(row: &Value, key: &str) -> String {
    row.get(key)
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// Reshape raw `list_tables` rows into a flat `["name", ...]` array.
pub fn parse_tables(rows: &[Value]) -> Vec<String> {
    rows.iter().map(|r| row_str(r, "table_name")).collect()
}

/// Reshape raw `list_columns` rows into normalized column descriptors.
pub fn parse_columns(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .map(|r| {
            json!({
                "name": row_str(r, "column_name"),
                "type": row_str(r, "data_type"),
                "nullable": row_str(r, "is_nullable").eq_ignore_ascii_case("yes"),
                "default": r.get("column_default").cloned().unwrap_or(Value::Null),
                "position": r.get("ordinal_position").cloned().unwrap_or(Value::Null),
            })
        })
        .collect()
}

/// Reshape raw `list_foreign_keys` rows into normalized FK descriptors.
pub fn parse_foreign_keys(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .map(|r| {
            json!({
                "constraint": row_str(r, "constraint_name"),
                "column": row_str(r, "column_name"),
                "references_table": row_str(r, "foreign_table_name"),
                "references_column": row_str(r, "foreign_column_name"),
            })
        })
        .collect()
}

// ---------------------------------------------------------------------
// Input validation.
// ---------------------------------------------------------------------

/// A Postgres identifier this tool will accept as a `schema` or `table`.
/// Restricted to `[A-Za-z_][A-Za-z0-9_$]*` — the unquoted-identifier
/// grammar. The identifiers are also bound as query parameters (`$1` /
/// `$2`), so this check is defense-in-depth, not the only barrier.
fn valid_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '$')
}

fn get_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str).map(str::trim)
}

// ---------------------------------------------------------------------
// Result envelope helpers.
// ---------------------------------------------------------------------

fn tool_ok(value: Value) -> ToolResult {
    ToolResult {
        content: value.to_string(),
        is_error: false,
    }
}

fn tool_err(message: impl Into<String>) -> ToolResult {
    ToolResult {
        content: json!({ "success": false, "error": message.into() }).to_string(),
        is_error: true,
    }
}

// ---------------------------------------------------------------------
// Tool: postgres_schema
// ---------------------------------------------------------------------

/// Read-only Postgres schema introspection tool. See module docs.
pub struct PostgresSchemaTool {
    backend: Arc<dyn PostgresSchemaBackend>,
    /// v0.9.0 W1: defaults `false` so `Tool::is_available()` hides the
    /// tool when no real backend is wired. `new(backend)` flips it on.
    backend_configured: bool,
}

impl Default for PostgresSchemaTool {
    fn default() -> Self {
        Self {
            backend: Arc::new(NullPostgresSchemaBackend),
            backend_configured: false,
        }
    }
}

impl PostgresSchemaTool {
    /// Construct the tool over a host-supplied backend.
    pub fn new(backend: Arc<dyn PostgresSchemaBackend>) -> Self {
        Self {
            backend,
            backend_configured: true,
        }
    }
}

#[async_trait]
impl Tool for PostgresSchemaTool {
    fn name(&self) -> &str {
        "postgres_schema"
    }

    /// v0.9.0 W1: hidden when no real `PostgresSchemaBackend` is wired.
    /// `Default::default()` yields `backend_configured == false`, so
    /// `ToolRegistry::register` drops the tool before the model sees it.
    fn is_available(&self) -> bool {
        self.backend_configured
    }

    fn description(&self) -> &str {
        "Introspect a Postgres database schema (read-only). Use operation='list_tables' to list \
         base tables in a schema, 'list_columns' to list a table's columns, or \
         'list_foreign_keys' to list a table's foreign-key constraints. Queries the standard \
         information_schema views."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": ["list_tables", "list_columns", "list_foreign_keys"],
                    "description": "Which introspection to run."
                },
                "schema": {
                    "type": "string",
                    "description": "Schema name. Defaults to 'public'."
                },
                "table": {
                    "type": "string",
                    "description": "Table name. Required for list_columns and list_foreign_keys."
                }
            },
            "required": ["operation"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Every operation is read-only — safe to run concurrently.
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let operation = match get_str(&input, "operation") {
            Some(op) if !op.is_empty() => op.to_ascii_lowercase(),
            _ => return tool_err("operation is required"),
        };

        let schema = match get_str(&input, "schema") {
            Some("") | None => "public".to_string(),
            Some(s) => s.to_string(),
        };
        if !valid_identifier(&schema) {
            return tool_err(format!(
                "invalid schema name '{schema}': must match [A-Za-z_][A-Za-z0-9_$]*"
            ));
        }

        let op = match operation.as_str() {
            "list_tables" => PostgresSchemaOp::ListTables { schema },
            "list_columns" | "list_foreign_keys" => {
                let table = match get_str(&input, "table") {
                    Some(t) if !t.is_empty() => t.to_string(),
                    _ => {
                        return tool_err(format!("table is required for operation='{operation}'"));
                    }
                };
                if !valid_identifier(&table) {
                    return tool_err(format!(
                        "invalid table name '{table}': must match [A-Za-z_][A-Za-z0-9_$]*"
                    ));
                }
                if operation == "list_columns" {
                    PostgresSchemaOp::ListColumns { schema, table }
                } else {
                    PostgresSchemaOp::ListForeignKeys { schema, table }
                }
            }
            other => return tool_err(format!("unknown operation '{other}'")),
        };

        match self.backend.run(op.clone()).await {
            PostgresSchemaOutcome::Ok(rows) => {
                let payload = match &op {
                    PostgresSchemaOp::ListTables { schema } => json!({
                        "success": true,
                        "operation": "list_tables",
                        "schema": schema,
                        "tables": parse_tables(&rows),
                    }),
                    PostgresSchemaOp::ListColumns { schema, table } => json!({
                        "success": true,
                        "operation": "list_columns",
                        "schema": schema,
                        "table": table,
                        "columns": parse_columns(&rows),
                    }),
                    PostgresSchemaOp::ListForeignKeys { schema, table } => json!({
                        "success": true,
                        "operation": "list_foreign_keys",
                        "schema": schema,
                        "table": table,
                        "foreign_keys": parse_foreign_keys(&rows),
                    }),
                };
                tool_ok(payload)
            }
            PostgresSchemaOutcome::Err(message) => tool_err(message),
        }
    }
}

// ---------------------------------------------------------------------
// Optional live backend — gated behind the `postgres` cargo feature.
// ---------------------------------------------------------------------

/// Live `tokio-postgres` backend. Only compiled when the `postgres`
/// cargo feature is enabled, because the driver pulls a native TLS /
/// socket stack that is heavy and platform-sensitive.
#[cfg(feature = "postgres")]
pub mod live {
    use super::{PostgresSchemaBackend, PostgresSchemaOp, PostgresSchemaOutcome};
    use async_trait::async_trait;
    use serde_json::{Map, Value, json};
    use tokio_postgres::Row;
    use tokio_postgres::types::Type;

    /// A `PostgresSchemaBackend` that connects on demand via
    /// `tokio-postgres` and runs each introspection query.
    ///
    /// The connection string is whatever the host configures — see the
    /// `tokio-postgres` docs for the accepted formats (`postgres://...`
    /// URL or libpq key/value).
    pub struct TokioPostgresBackend {
        conn_string: String,
    }

    impl TokioPostgresBackend {
        /// Construct over a libpq / URL connection string.
        pub fn new(conn_string: impl Into<String>) -> Self {
            Self {
                conn_string: conn_string.into(),
            }
        }
    }

    /// Convert one `tokio_postgres::Row` into a JSON object keyed by
    /// column name. Only the column types the introspection queries can
    /// return (`text`/`name`/`int*`) are decoded; anything else becomes
    /// a string-ish fallback so a schema-view change can never panic.
    fn row_to_json(row: &Row) -> Value {
        let mut obj = Map::new();
        for (i, col) in row.columns().iter().enumerate() {
            let value = match *col.type_() {
                Type::INT2 => row
                    .try_get::<_, Option<i16>>(i)
                    .ok()
                    .flatten()
                    .map(|v| json!(v))
                    .unwrap_or(Value::Null),
                Type::INT4 => row
                    .try_get::<_, Option<i32>>(i)
                    .ok()
                    .flatten()
                    .map(|v| json!(v))
                    .unwrap_or(Value::Null),
                Type::INT8 => row
                    .try_get::<_, Option<i64>>(i)
                    .ok()
                    .flatten()
                    .map(|v| json!(v))
                    .unwrap_or(Value::Null),
                _ => row
                    .try_get::<_, Option<String>>(i)
                    .ok()
                    .flatten()
                    .map(Value::String)
                    .unwrap_or(Value::Null),
            };
            obj.insert(col.name().to_string(), value);
        }
        Value::Object(obj)
    }

    #[async_trait]
    impl PostgresSchemaBackend for TokioPostgresBackend {
        async fn run(&self, op: PostgresSchemaOp) -> PostgresSchemaOutcome {
            let (client, connection) =
                match tokio_postgres::connect(&self.conn_string, tokio_postgres::NoTls).await {
                    Ok(pair) => pair,
                    Err(e) => return PostgresSchemaOutcome::Err(format!("connect failed: {e}")),
                };
            // The connection future must be driven for the client to
            // make progress; abort the handle when this scope ends.
            let conn_task = tokio::spawn(connection);

            let params: Vec<&str> = op.params();
            let dyn_params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
                .iter()
                .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
                .collect();

            let result = client.query(op.sql(), &dyn_params).await;
            conn_task.abort();

            match result {
                Ok(rows) => PostgresSchemaOutcome::Ok(rows.iter().map(row_to_json).collect()),
                Err(e) => PostgresSchemaOutcome::Err(format!("query failed: {e}")),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_tool(tool: &PostgresSchemaTool, input: Value) -> ToolResult {
        futures::executor::block_on(tool.execute(input))
    }

    fn parse(result: &ToolResult) -> Value {
        serde_json::from_str(&result.content).expect("valid JSON")
    }

    // -----------------------------------------------------------------
    // 1. Introspection-SQL construction is exact + stable.
    // -----------------------------------------------------------------

    #[test]
    fn introspection_sql_strings_are_exact() {
        let tables = tables_query();
        assert!(tables.contains("information_schema.tables"));
        assert!(tables.contains("table_type = 'BASE TABLE'"));
        assert!(tables.contains("table_schema = $1"));
        assert!(tables.contains("ORDER BY table_name"));

        let columns = columns_query();
        assert!(columns.contains("information_schema.columns"));
        assert!(columns.contains("table_schema = $1 AND table_name = $2"));
        assert!(columns.contains("ordinal_position"));
        assert!(columns.contains("ORDER BY ordinal_position"));

        let fks = foreign_keys_query();
        assert!(fks.contains("information_schema.table_constraints"));
        assert!(fks.contains("information_schema.key_column_usage"));
        assert!(fks.contains("information_schema.constraint_column_usage"));
        assert!(fks.contains("constraint_type = 'FOREIGN KEY'"));
        assert!(fks.contains("foreign_table_name"));
        assert!(fks.contains("foreign_column_name"));

        // The op carries the right SQL + parameter arity.
        let list_tables = PostgresSchemaOp::ListTables {
            schema: "public".into(),
        };
        assert_eq!(list_tables.sql(), tables);
        assert_eq!(list_tables.params(), vec!["public"]);

        let list_cols = PostgresSchemaOp::ListColumns {
            schema: "app".into(),
            table: "users".into(),
        };
        assert_eq!(list_cols.sql(), columns);
        assert_eq!(list_cols.params(), vec!["app", "users"]);

        let list_fks = PostgresSchemaOp::ListForeignKeys {
            schema: "app".into(),
            table: "orders".into(),
        };
        assert_eq!(list_fks.sql(), fks);
        assert_eq!(list_fks.params(), vec!["app", "orders"]);
    }

    // -----------------------------------------------------------------
    // 2. Result-row parsing from fixture rows — tables.
    // -----------------------------------------------------------------

    #[test]
    fn list_tables_parses_fixture_rows() {
        let rows = vec![
            json!({"table_name": "users"}),
            json!({"table_name": "orders"}),
        ];
        let backend = Arc::new(CapturingPostgresSchemaBackend::new(rows));
        let tool = PostgresSchemaTool::new(backend.clone());

        let r = run_tool(&tool, json!({"operation": "list_tables", "schema": "app"}));
        assert!(!r.is_error, "expected success: {}", r.content);
        let v = parse(&r);
        assert_eq!(v["success"], json!(true));
        assert_eq!(v["operation"], json!("list_tables"));
        assert_eq!(v["schema"], json!("app"));
        assert_eq!(v["tables"], json!(["users", "orders"]));

        // The backend received the right op (schema bound, not defaulted).
        let snap = backend.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(
            snap[0],
            PostgresSchemaOp::ListTables {
                schema: "app".into()
            }
        );
    }

    #[test]
    fn list_tables_defaults_schema_to_public() {
        let backend = Arc::new(CapturingPostgresSchemaBackend::new(vec![]));
        let tool = PostgresSchemaTool::new(backend.clone());
        let r = run_tool(&tool, json!({"operation": "list_tables"}));
        assert!(!r.is_error, "{}", r.content);
        let v = parse(&r);
        assert_eq!(v["schema"], json!("public"));
        assert_eq!(v["tables"], json!([]));
        assert_eq!(
            backend.snapshot()[0],
            PostgresSchemaOp::ListTables {
                schema: "public".into()
            }
        );
    }

    // -----------------------------------------------------------------
    // 3. Result-row parsing — columns + foreign keys.
    // -----------------------------------------------------------------

    #[test]
    fn list_columns_normalizes_fixture_rows() {
        let rows = vec![
            json!({
                "column_name": "id",
                "data_type": "integer",
                "is_nullable": "NO",
                "column_default": "nextval('users_id_seq'::regclass)",
                "ordinal_position": 1
            }),
            json!({
                "column_name": "email",
                "data_type": "text",
                "is_nullable": "YES",
                "column_default": null,
                "ordinal_position": 2
            }),
        ];
        let backend = Arc::new(CapturingPostgresSchemaBackend::new(rows));
        let tool = PostgresSchemaTool::new(backend.clone());

        let r = run_tool(
            &tool,
            json!({"operation": "list_columns", "schema": "public", "table": "users"}),
        );
        assert!(!r.is_error, "{}", r.content);
        let v = parse(&r);
        assert_eq!(v["operation"], json!("list_columns"));
        assert_eq!(v["table"], json!("users"));
        let cols = v["columns"].as_array().expect("columns array");
        assert_eq!(cols.len(), 2);
        assert_eq!(cols[0]["name"], json!("id"));
        assert_eq!(cols[0]["type"], json!("integer"));
        assert_eq!(cols[0]["nullable"], json!(false));
        assert_eq!(cols[0]["position"], json!(1));
        assert_eq!(cols[1]["name"], json!("email"));
        assert_eq!(cols[1]["nullable"], json!(true));
        assert_eq!(cols[1]["default"], Value::Null);

        assert_eq!(
            backend.snapshot()[0],
            PostgresSchemaOp::ListColumns {
                schema: "public".into(),
                table: "users".into(),
            }
        );
    }

    #[test]
    fn list_foreign_keys_normalizes_fixture_rows() {
        let rows = vec![json!({
            "constraint_name": "orders_user_id_fkey",
            "column_name": "user_id",
            "foreign_table_name": "users",
            "foreign_column_name": "id"
        })];
        let backend = Arc::new(CapturingPostgresSchemaBackend::new(rows));
        let tool = PostgresSchemaTool::new(backend);

        let r = run_tool(
            &tool,
            json!({"operation": "list_foreign_keys", "table": "orders"}),
        );
        assert!(!r.is_error, "{}", r.content);
        let v = parse(&r);
        let fks = v["foreign_keys"].as_array().expect("foreign_keys array");
        assert_eq!(fks.len(), 1);
        assert_eq!(fks[0]["constraint"], json!("orders_user_id_fkey"));
        assert_eq!(fks[0]["column"], json!("user_id"));
        assert_eq!(fks[0]["references_table"], json!("users"));
        assert_eq!(fks[0]["references_column"], json!("id"));
    }

    // -----------------------------------------------------------------
    // 4. Input validation + error handling.
    // -----------------------------------------------------------------

    #[test]
    fn invalid_input_is_rejected() {
        let backend = Arc::new(CapturingPostgresSchemaBackend::new(vec![]));
        let tool = PostgresSchemaTool::new(backend.clone());

        // Missing operation.
        let r = run_tool(&tool, json!({}));
        assert!(r.is_error);
        assert!(r.content.contains("operation is required"));

        // Unknown operation.
        let r = run_tool(&tool, json!({"operation": "drop_table"}));
        assert!(r.is_error);
        assert!(r.content.contains("unknown operation"));

        // list_columns without a table.
        let r = run_tool(&tool, json!({"operation": "list_columns"}));
        assert!(r.is_error);
        assert!(r.content.contains("table is required"));

        // Injection-shaped schema name is rejected before dispatch.
        let r = run_tool(
            &tool,
            json!({"operation": "list_tables", "schema": "public; DROP TABLE users"}),
        );
        assert!(r.is_error);
        assert!(r.content.contains("invalid schema name"));

        // Injection-shaped table name is rejected.
        let r = run_tool(
            &tool,
            json!({"operation": "list_columns", "table": "users--"}),
        );
        assert!(r.is_error);
        assert!(r.content.contains("invalid table name"));

        // No backend dispatch happened for any rejected input.
        assert!(backend.snapshot().is_empty());
    }

    #[test]
    fn backend_error_surfaces_as_tool_error() {
        struct FailingBackend;
        #[async_trait]
        impl PostgresSchemaBackend for FailingBackend {
            async fn run(&self, _op: PostgresSchemaOp) -> PostgresSchemaOutcome {
                PostgresSchemaOutcome::Err("connection refused".into())
            }
        }
        let tool = PostgresSchemaTool::new(Arc::new(FailingBackend));
        let r = run_tool(&tool, json!({"operation": "list_tables"}));
        assert!(r.is_error);
        assert!(r.content.contains("connection refused"));
    }

    // -----------------------------------------------------------------
    // 5. Feature-disabled path — Null backend fails loudly (NO-STUBS).
    // -----------------------------------------------------------------

    #[test]
    fn null_backend_fails_loudly() {
        // `PostgresSchemaTool::default()` wires the Null backend, which
        // is what callers get when the `postgres` feature is off and no
        // host backend is supplied.
        let tool = PostgresSchemaTool::default();
        let r = run_tool(&tool, json!({"operation": "list_tables"}));
        assert!(r.is_error);
        assert!(
            r.content.contains("No Postgres backend configured"),
            "got: {}",
            r.content
        );
        assert!(r.content.contains("`postgres` cargo feature"));
    }

    // -----------------------------------------------------------------
    // Registry plumbing.
    // -----------------------------------------------------------------

    #[test]
    fn registers_into_registry() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        // v0.9.0 W1: registry now skips tools whose `is_available()`
        // returns false, so we must wire a real backend for this test.
        reg.register(Box::new(PostgresSchemaTool::new(Arc::new(
            CapturingPostgresSchemaBackend::new(vec![]),
        ))));
        let defs = reg.to_tool_defs();
        let def = defs
            .iter()
            .find(|d| d.name == "postgres_schema")
            .expect("postgres_schema registered");
        let required: Vec<&str> = def.input_schema["required"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(required.contains(&"operation"));
    }

    // -----------------------------------------------------------------
    // v0.9.0 W1 backend gate.
    // -----------------------------------------------------------------

    #[test]
    fn default_is_hidden_when_no_backend_wired() {
        let tool = PostgresSchemaTool::default();
        assert!(
            !tool.is_available(),
            "Default::default() must yield backend_configured == false"
        );
    }

    #[test]
    fn with_real_backend_is_available() {
        let tool = PostgresSchemaTool::new(Arc::new(CapturingPostgresSchemaBackend::new(vec![])));
        assert!(
            tool.is_available(),
            "new(backend) must yield backend_configured == true"
        );
    }
}
