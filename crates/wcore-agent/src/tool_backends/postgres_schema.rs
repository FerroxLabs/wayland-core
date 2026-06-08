//! v0.9.0 Wave-1 B8 — real `tokio-postgres` backend for the
//! `postgres_schema` introspection tool.
//!
//! The resolver picks a connection string from one of three env vars in
//! priority order and constructs a real backend; if NONE is set the
//! resolver returns `None` so `bootstrap.rs` registers the null-backed
//! default (which the registry's availability filter then drops, so the
//! model never sees a tool whose only response is "not configured").
//!
//! ## Resolver order
//!
//! 1. `DATABASE_URL`        — the Heroku / Twelve-Factor convention.
//! 2. `POSTGRES_URL`        — Vercel / Render alternative.
//! 3. `PG_CONN_STRING`      — bare libpq key/value escape hatch.
//!
//! ## SSRF posture
//!
//! Postgres clients connect directly over TCP — there is no HTTP layer
//! whose SSRF redirect policy we can lean on. So we parse the
//! `tokio_postgres::Config` and explicitly REJECT private ranges that an
//! attacker-supplied `DATABASE_URL` could otherwise pivot through:
//!
//! * IPv4 link-local 169.254.0.0/16   (covers cloud metadata endpoints).
//! * IPv4 private 10.0.0.0/8          (corporate / VPC internal).
//!
//! Note: 127.0.0.1 and `localhost` are NOT rejected — Postgres on
//! `localhost` is the common dev / sidecar pattern, and a model
//! talking to its own host has not crossed a trust boundary the way an
//! outbound fetch to 169.254 would. This matches the v0.9.0 Wave-1 B8
//! briefing's "allow localhost" carve-out.
//!
//! ## TLS posture
//!
//! v0.9.0 ships `NoTls` only. If the connection string opts into TLS
//! via `?sslmode=require` we reject with an explicit "TLS not yet
//! implemented" error rather than silently downgrading to cleartext.
//! v0.9.1 will wire `tokio-postgres-rustls`.
//!
//! ## Two-layer timeouts
//!
//! Both `connect()` and every `query()` call wrap in
//! `tokio::time::timeout` so a hung peer cannot park the tool dispatch
//! loop. The connect cap is 5 s; queries cap at 10 s — enough for
//! `information_schema` reads on a healthy DB, short enough that a sick
//! peer surfaces as an error within one tool turn.
//!
//! ## EXPLAIN safety
//!
//! `explain_query` validates that the SQL begins with `SELECT` or
//! `WITH` and contains NO `;` — closing the multi-statement-attack
//! vector even though the connection runs as a read-only role. This is
//! defense-in-depth: the caller's role SHOULD be locked down, but the
//! tool does not get to assume it is.

use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio_postgres::Config;
use tokio_postgres::config::Host;
use tokio_postgres::types::Type;

use wcore_tools::postgres_schema_tool::{
    PostgresSchemaBackend, PostgresSchemaOp, PostgresSchemaOutcome,
};

use super::shared::read_env_key;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);

/// Live `tokio-postgres` backend used by the agent host.
///
/// Holds the parsed `tokio_postgres::Config` so every dispatch reconnects
/// with the validated config (no string re-parsing) — schema
/// introspection is low-frequency, so a per-call connect avoids the
/// long-lived connection-task plumbing.
#[derive(Debug)]
pub struct LiveTokioPostgresBackend {
    config: Config,
}

impl LiveTokioPostgresBackend {
    /// Construct from an already-validated `Config`. Use
    /// [`from_conn_string`](Self::from_conn_string) to validate the SSRF
    /// host policy before instantiating.
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    /// Parse `conn_string` (libpq URL or key/value) and validate:
    ///
    /// * `sslmode=require` is REJECTED ("TLS not yet implemented for
    ///   postgres_schema; v0.9.1").
    /// * Hosts in 169.254/16 or 10.0.0.0/8 are REJECTED (SSRF).
    /// * At least one host is present.
    pub fn from_conn_string(conn_string: &str) -> Result<Self, String> {
        // Reject TLS up front — string match keeps us from depending on
        // a TLS crate. v0.9.1 will wire rustls and lift this gate.
        if conn_string.to_ascii_lowercase().contains("sslmode=require") {
            return Err("TLS not yet implemented for postgres_schema; v0.9.1".to_string());
        }

        let config = Config::from_str(conn_string)
            .map_err(|e| format!("invalid postgres connection string: {e}"))?;

        let hosts = config.get_hosts();
        if hosts.is_empty() {
            return Err("postgres connection string has no host".to_string());
        }
        for host in hosts {
            validate_host(host)?;
        }
        Ok(Self::new(config))
    }
}

/// Reject hosts that look like the cloud-metadata or RFC1918 ranges.
/// Allows loopback (`localhost`, `127.0.0.1`, `::1`) — see module docs.
fn validate_host(host: &Host) -> Result<(), String> {
    match host {
        Host::Tcp(name) => {
            // Try to parse as an IP literal first. Hostnames that resolve
            // to a private IP at connect time are out of scope for v0.9.0
            // (we don't pre-resolve DNS); the local network operator who
            // sets DATABASE_URL is already inside the trust boundary.
            if let Ok(ip) = name.parse::<IpAddr>()
                && is_blocked_postgres_ip(ip)
            {
                return Err(format!(
                    "postgres host {ip} is in a blocked private range \
                     (169.254/16 link-local or 10.0.0.0/8)"
                ));
            }
            Ok(())
        }
        // Unix sockets / Windows named pipes — local-only by definition;
        // no SSRF surface.
        #[allow(unreachable_patterns)]
        _ => Ok(()),
    }
}

/// SSRF block list specific to `postgres_schema`. NARROWER than the
/// general `wcore_tools::url_safety::is_safe_url` policy because
/// localhost-Postgres is a legitimate dev pattern. See module docs.
fn is_blocked_postgres_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            // 169.254.0.0/16 link-local (covers cloud metadata).
            if o[0] == 169 && o[1] == 254 {
                return true;
            }
            // 10.0.0.0/8 RFC1918 private.
            if o[0] == 10 {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            // IPv4-mapped IPv6 — unwrap and re-check.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_postgres_ip(IpAddr::V4(mapped));
            }
            // Block IPv6 link-local fe80::/10 (mirror of 169.254/16).
            let seg0 = v6.segments()[0];
            if (seg0 & 0xffc0) == 0xfe80 {
                return true;
            }
            false
        }
    }
}

/// Resolver — picks the first env-var that is set + non-empty and tries
/// to build a backend over it. Returns `None` when ALL three vars are
/// unset (the documented "no postgres available" state).
///
/// Returns `None` (not an error) on validation failures too — the
/// bootstrap path then registers the null-backed default. The validation
/// error is logged at WARN so an operator who misconfigured
/// `DATABASE_URL` can see why the tool is hidden.
pub async fn build_postgres_schema_backend() -> Option<Arc<dyn PostgresSchemaBackend>> {
    // Resolver order is load-bearing — see module docs.
    let (var_name, conn_string) = if let Some(v) = read_env_key("DATABASE_URL") {
        ("DATABASE_URL", v)
    } else if let Some(v) = read_env_key("POSTGRES_URL") {
        ("POSTGRES_URL", v)
    } else if let Some(v) = read_env_key("PG_CONN_STRING") {
        ("PG_CONN_STRING", v)
    } else {
        tracing::info!(
            "postgres_schema: no DATABASE_URL / POSTGRES_URL / PG_CONN_STRING set — tool hidden"
        );
        return None;
    };

    match LiveTokioPostgresBackend::from_conn_string(&conn_string) {
        Ok(backend) => {
            tracing::info!(
                env = var_name,
                "postgres_schema: backend configured from {var_name}"
            );
            Some(Arc::new(backend))
        }
        Err(err) => {
            tracing::warn!(
                env = var_name,
                error = %err,
                "postgres_schema: rejecting connection string — tool hidden"
            );
            None
        }
    }
}

#[async_trait]
impl PostgresSchemaBackend for LiveTokioPostgresBackend {
    async fn run(&self, op: PostgresSchemaOp) -> PostgresSchemaOutcome {
        // ── Connect (with timeout) ────────────────────────────────────
        let connect_fut = self.config.connect(tokio_postgres::NoTls);
        let (client, connection) = match tokio::time::timeout(CONNECT_TIMEOUT, connect_fut).await {
            Ok(Ok(pair)) => pair,
            Ok(Err(e)) => {
                return PostgresSchemaOutcome::Err(format!("connect failed: {e}"));
            }
            Err(_) => {
                return PostgresSchemaOutcome::Err(format!(
                    "connect timed out after {}s",
                    CONNECT_TIMEOUT.as_secs()
                ));
            }
        };

        // Drive the connection task so the client makes progress; abort
        // it once the query is done so we don't leak a tokio task.
        let conn_task = tokio::spawn(connection);

        // ── Query (with timeout) ──────────────────────────────────────
        let params: Vec<&str> = op.params();
        let dyn_params: Vec<&(dyn tokio_postgres::types::ToSql + Sync)> = params
            .iter()
            .map(|p| p as &(dyn tokio_postgres::types::ToSql + Sync))
            .collect();

        let query_fut = client.query(op.sql(), &dyn_params);
        let outcome = match tokio::time::timeout(QUERY_TIMEOUT, query_fut).await {
            Ok(Ok(rows)) => PostgresSchemaOutcome::Ok(rows.iter().map(row_to_json).collect()),
            Ok(Err(e)) => PostgresSchemaOutcome::Err(format!("query failed: {e}")),
            Err(_) => PostgresSchemaOutcome::Err(format!(
                "query timed out after {}s",
                QUERY_TIMEOUT.as_secs()
            )),
        };

        conn_task.abort();
        outcome
    }
}

/// Convert one `tokio_postgres::Row` into a JSON object keyed by column
/// name. Mirrors the in-tree `live::TokioPostgresBackend` in
/// `wcore-tools` so the row shape matches the introspection tool's
/// `parse_*` helpers without coupling the two modules.
fn row_to_json(row: &tokio_postgres::Row) -> Value {
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

/// Validate an EXPLAIN target SQL. v0.9.0 only allows read-only
/// `SELECT`/`WITH` explains and rejects any `;` (multi-statement
/// attack). Returns the validated SQL on success.
///
/// Public so the future `explain` tool wrapper (and tests) can call it
/// without duplicating the validation.
pub fn validate_explain_sql(sql: &str) -> Result<&str, String> {
    let trimmed = sql.trim();
    if trimmed.is_empty() {
        return Err("explain target SQL is empty".to_string());
    }
    if trimmed.contains(';') {
        return Err(
            "multi-statement SQL is not allowed (no ';' permitted in explain target)".to_string(),
        );
    }
    let head = trimmed
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_uppercase();
    if head != "SELECT" && head != "WITH" {
        return Err(format!(
            "only read-only SELECT / WITH queries are allowed for EXPLAIN (got: {head})"
        ));
    }
    Ok(trimmed)
}

// ────────────────────────────────────────────────────────────────────────
// Tests
// ────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ENV-var manipulation in tests must serialize so we don't race the
    // other tests in this binary. `serial_test` is a dev-dep on
    // `wcore-agent` already.
    use serial_test::serial;

    /// Clear all three env vars the resolver reads — every env-resolver
    /// test runs this so prior state cannot leak through.
    fn clear_env() {
        // SAFETY: tests using this helper are `#[serial]` so the env
        // mutation cannot race.
        unsafe {
            std::env::remove_var("DATABASE_URL");
            std::env::remove_var("POSTGRES_URL");
            std::env::remove_var("PG_CONN_STRING");
        }
    }

    fn set_env(name: &str, value: &str) {
        // SAFETY: see clear_env.
        unsafe { std::env::set_var(name, value) };
    }

    // ── Resolver: env-var matrix ──────────────────────────────────────

    #[tokio::test]
    #[serial]
    async fn build_postgres_schema_backend_returns_none_when_unset() {
        clear_env();
        assert!(build_postgres_schema_backend().await.is_none());
    }

    #[tokio::test]
    #[serial]
    async fn build_postgres_schema_backend_reads_database_url_first() {
        clear_env();
        set_env("DATABASE_URL", "postgres://u:p@db.example.com/app");
        set_env(
            "POSTGRES_URL",
            "postgres://u:p@should-not-use.example.com/x",
        );
        set_env("PG_CONN_STRING", "postgres://u:p@nope.example.com/y");
        let backend = build_postgres_schema_backend().await;
        assert!(backend.is_some(), "DATABASE_URL must produce a backend");
        clear_env();
    }

    #[tokio::test]
    #[serial]
    async fn build_postgres_schema_backend_falls_back_to_postgres_url() {
        clear_env();
        set_env("POSTGRES_URL", "postgres://u:p@db.example.com/app");
        let backend = build_postgres_schema_backend().await;
        assert!(backend.is_some(), "POSTGRES_URL must produce a backend");
        clear_env();
    }

    #[tokio::test]
    #[serial]
    async fn build_postgres_schema_backend_falls_back_to_pg_conn_string() {
        clear_env();
        // libpq key/value form — exercises Config::from_str on non-URL syntax.
        set_env("PG_CONN_STRING", "host=db.example.com user=u dbname=app");
        let backend = build_postgres_schema_backend().await;
        assert!(backend.is_some(), "PG_CONN_STRING must produce a backend");
        clear_env();
    }

    #[tokio::test]
    #[serial]
    async fn empty_string_env_is_ignored() {
        clear_env();
        set_env("DATABASE_URL", "   ");
        assert!(
            build_postgres_schema_backend().await.is_none(),
            "blank-only env var must not satisfy the resolver"
        );
        clear_env();
    }

    #[tokio::test]
    #[serial]
    async fn malformed_url_returns_none_and_logs_warning() {
        clear_env();
        set_env("DATABASE_URL", "this is not a postgres url");
        assert!(build_postgres_schema_backend().await.is_none());
        clear_env();
    }

    // ── Connection-string parsing + SSRF host validation ──────────────

    #[test]
    fn parses_database_url_correctly() {
        let backend =
            LiveTokioPostgresBackend::from_conn_string("postgres://u:p@db.example.com:5432/app")
                .expect("valid URL must parse");
        let hosts = backend.config.get_hosts();
        assert!(matches!(&hosts[0], Host::Tcp(h) if h == "db.example.com"));
        // Port lives in a parallel vec.
        assert_eq!(backend.config.get_ports(), &[5432]);
    }

    #[test]
    fn allows_localhost_127_0_0_1() {
        // Postgres on localhost is the common dev / sidecar pattern —
        // explicitly allow it (overrides the broader SSRF policy).
        LiveTokioPostgresBackend::from_conn_string("postgres://u:p@127.0.0.1:5432/app")
            .expect("localhost must be allowed");
        LiveTokioPostgresBackend::from_conn_string("postgres://u:p@localhost:5432/app")
            .expect("'localhost' hostname must be allowed");
    }

    #[test]
    fn rejects_host_in_link_local_169_254() {
        let err =
            LiveTokioPostgresBackend::from_conn_string("postgres://u:p@169.254.169.254:5432/app")
                .expect_err("link-local must be rejected");
        assert!(
            err.contains("169.254") || err.contains("blocked"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_host_in_private_10_range() {
        let err = LiveTokioPostgresBackend::from_conn_string("postgres://u:p@10.0.0.5:5432/app")
            .expect_err("10.x must be rejected");
        assert!(
            err.contains("10.0.0.0/8") || err.contains("blocked"),
            "got: {err}"
        );
    }

    #[test]
    fn rejects_sslmode_require_for_v090() {
        let err = LiveTokioPostgresBackend::from_conn_string(
            "postgres://u:p@db.example.com/app?sslmode=require",
        )
        .expect_err("sslmode=require must be rejected in v0.9.0");
        assert!(err.contains("TLS not yet implemented"), "got: {err}");
    }

    #[test]
    fn rejects_malformed_url() {
        let err = LiveTokioPostgresBackend::from_conn_string("ftp://nope")
            .expect_err("non-postgres URL must fail to parse");
        assert!(
            err.contains("invalid postgres connection string"),
            "got: {err}"
        );
    }

    // ── EXPLAIN safety ────────────────────────────────────────────────

    #[test]
    fn explain_rejects_semicolon_injection() {
        let err = validate_explain_sql("SELECT 1; DROP TABLE users")
            .expect_err("semicolon must be rejected");
        assert!(err.contains("multi-statement"), "got: {err}");
    }

    #[test]
    fn explain_rejects_non_select() {
        for bad in [
            "DROP TABLE users",
            "UPDATE users SET admin=true",
            "INSERT INTO x VALUES (1)",
            "DELETE FROM x",
        ] {
            validate_explain_sql(bad)
                .err()
                .unwrap_or_else(|| panic!("non-SELECT '{bad}' was wrongly accepted"));
        }
        // Positive path: SELECT + WITH allowed.
        validate_explain_sql("SELECT 1").expect("plain SELECT must pass");
        validate_explain_sql("WITH x AS (SELECT 1) SELECT * FROM x").expect("WITH must pass");
    }

    #[test]
    fn explain_rejects_empty() {
        let err = validate_explain_sql("   ").expect_err("empty SQL must be rejected");
        assert!(err.contains("empty"), "got: {err}");
    }

    // ── Failure paths: connection refused + query timeout ─────────────

    #[tokio::test]
    async fn connection_refused_surfaces_as_outcome_err() {
        // Port 1 is privileged + unbound on essentially every host.
        // Connect attempts should fail within CONNECT_TIMEOUT (5s) with
        // either a refused-connection error or a timeout error.
        let backend = LiveTokioPostgresBackend::from_conn_string("postgres://u:p@127.0.0.1:1/app")
            .expect("valid URL");
        let outcome = backend
            .run(PostgresSchemaOp::ListTables {
                schema: "public".into(),
            })
            .await;
        match outcome {
            PostgresSchemaOutcome::Err(msg) => {
                assert!(
                    msg.contains("connect") || msg.contains("timed out"),
                    "expected connect-failure error, got: {msg}"
                );
            }
            PostgresSchemaOutcome::Ok(_) => {
                panic!("connection to unbound port must not succeed")
            }
        }
    }
}
