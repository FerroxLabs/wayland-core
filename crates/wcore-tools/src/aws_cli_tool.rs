//! v0.6.3 Tier 2B T14 — read-only AWS CLI wrapper tool.
//!
//! Ported from the prior Wayland Python engine. Wraps the
//! host `aws` CLI binary, restricted to **read-only** operations. The
//! AWS CLI command form is `aws <service> <operation> [args...]`, e.g.
//! `aws s3 ls`, `aws ec2 describe-instances`, `aws iam get-user`.
//!
//! ## Read-only enforcement
//!
//! Enforcement is by **allowlist**, not denylist: an operation is
//! rejected unless it matches a known read-only prefix
//! (`describe-` / `list-` / `get-` / `head-` / `lookup-` / `search-` /
//! `batch-get-`) or an exact-match read operation (`ls` / `scan` /
//! `query`). Anything not matching — every `create-*`, `delete-*`,
//! `put-*`, `update-*`, `terminate-*`, `s3 rm`, `s3 sync`, etc. — is
//! refused **before** the CLI is spawned. An allowlist is the correct
//! posture here: a new mutating verb AWS adds in the future fails
//! closed rather than slipping past a denylist.
//!
//! ## Sandbox
//!
//! The assembled `aws ...` argv is run through the sandbox backend
//! (`wcore_sandbox::default_for_platform()` + `SandboxBackend::execute`)
//! exactly as `BashTool` runs shell commands — see [`crate::bash`]. The
//! sandbox confines filesystem / syscalls; network is `Inherit` since
//! the AWS CLI must reach AWS endpoints.
//!
//! ## Env (D.1 Round 1 — HIGH-2; D.2 Round 2 — MED)
//!
//! The child env is a **curated** passthrough via
//! [`crate::env_passthrough::build_sandboxed_env_with_force_allow`], NOT
//! a blanket `std::env::vars()` copy. `PATH` / `HOME` plus the non-secret
//! `AWS_*` discovery family (`AWS_REGION`, `AWS_PROFILE`,
//! `AWS_CONFIG_FILE`, `AWS_DEFAULT_REGION`, …) pass through so AWS
//! credential discovery via `~/.aws/credentials` and instance profiles
//! works.
//!
//! R1's curated-env hardening drops every secret-shaped var via the
//! `is_sensitive_env_var` filter — which catches the env-var AWS
//! credentials (`AWS_ACCESS_KEY_ID` contains `ACCESS_KEY`,
//! `AWS_SECRET_ACCESS_KEY` contains `SECRET`, `AWS_SESSION_TOKEN`
//! contains `TOKEN`). That broke env-var-based AWS auth — the common
//! case in CI runners and many laptops. Since this tool *is* the AWS
//! tool, it legitimately needs those credentials: the D.2 fix passes the
//! three credential vars through an explicit per-tool `force_allow` list
//! ([`AWS_FORCE_ALLOW`]) that bypasses the secret filter **for this tool
//! only**. The R1 principle — never broadcast secrets to *arbitrary*
//! commands — is preserved: the bypass is scoped to these exact names on
//! this one tool, not the whole `AWS_*` family or any other command.
//!
//! Without the `aws` binary on `PATH` the sandbox surfaces a non-zero
//! exit / spawn error — the tool reports it honestly rather than
//! pretending to succeed.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
#[cfg(test)]
use wcore_sandbox::ResourceLimitEnforcement;
use wcore_sandbox::{
    NetworkPolicy, SandboxCommand, SandboxManifest, SandboxOutput, default_for_platform,
};
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Command timeout for an `aws` invocation.
const TIMEOUT_MS: u64 = 120_000;

/// Non-secret env prefix the AWS CLI needs for credential / config
/// discovery (`AWS_REGION`, `AWS_PROFILE`, `AWS_CONFIG_FILE`, …). The
/// secret-shaped `AWS_*` credential names are NOT covered by this
/// prefix path — they are passed explicitly via [`AWS_FORCE_ALLOW`].
const AWS_ENV_PREFIXES: &[&str] = &["AWS_"];

/// AWS credential env vars passed through to the sandboxed `aws` child
/// via the curated builder's `force_allow` escape hatch, bypassing the
/// `is_sensitive_env_var` secret filter **for this tool only**.
///
/// `aws_cli` is the AWS tool, so it must receive AWS credentials when
/// the host's only credential source is environment variables (the
/// common CI / laptop case). The three credential vars
/// (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`)
/// are otherwise dropped by the secret filter — they contain
/// `ACCESS_KEY` / `SECRET` / `TOKEN`. `AWS_PROFILE` /
/// `AWS_REGION` / `AWS_DEFAULT_REGION` are non-secret and already pass
/// through the `AWS_` prefix, but listing them here keeps the credential
/// set explicit and self-documenting; force_allow on a non-secret name
/// is a harmless no-op (the prefix path would keep it anyway).
const AWS_FORCE_ALLOW: &[&str] = &[
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
];

/// Max bytes of combined output returned to the model before truncation.
const MAX_OUTPUT_BYTES: usize = 50_000;

/// Read-only operation prefixes. An operation is accepted if it starts
/// with one of these (case-insensitive). `batch-get-` is listed before
/// `get-` is not required — prefix matching is independent — but it is
/// kept explicit for clarity.
pub const READ_ONLY_PREFIXES: &[&str] = &[
    "describe-",
    "list-",
    "get-",
    "head-",
    "lookup-",
    "search-",
    "batch-get-",
];

/// Exact-match read-only operations that do not follow the `verb-noun`
/// prefix shape. `ls` is the `aws s3 ls` listing; `scan` / `query` are
/// DynamoDB reads.
pub const READ_ONLY_EXACT: &[&str] = &["ls", "scan", "query"];

/// Result of validating a requested AWS operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationCheck {
    /// The operation is on the read-only allowlist.
    Allowed,
    /// The operation is not a recognized read-only operation.
    Rejected { reason: String },
}

/// Validate `operation` against the read-only allowlist.
///
/// Pure function — no I/O. Accepts an operation if it matches a
/// read-only prefix in [`READ_ONLY_PREFIXES`] or is an exact read in
/// [`READ_ONLY_EXACT`]. Matching is case-insensitive. Everything else
/// — mutating verbs, unknown operations — is [`OperationCheck::Rejected`].
pub fn validate_operation(operation: &str) -> OperationCheck {
    let op = operation.trim().to_ascii_lowercase();
    if op.is_empty() {
        return OperationCheck::Rejected {
            reason: "operation must not be empty".to_string(),
        };
    }
    if READ_ONLY_EXACT.contains(&op.as_str()) {
        return OperationCheck::Allowed;
    }
    if READ_ONLY_PREFIXES.iter().any(|p| op.starts_with(p)) {
        return OperationCheck::Allowed;
    }
    OperationCheck::Rejected {
        reason: format!(
            "operation '{operation}' is not a read-only AWS operation. \
             This tool only permits read operations: prefixes [{}] or exact [{}].",
            READ_ONLY_PREFIXES.join(", "),
            READ_ONLY_EXACT.join(", "),
        ),
    }
}

/// Build the `aws` argv for a validated request.
///
/// Produces `["aws", <service>, <operation>, <extra args...>, "--region",
/// <region>?, "--profile", <profile>?, "--output", <output>?]`. The
/// optional flags are appended only when their value is `Some` and
/// non-empty. Pure function — no I/O; tested directly.
pub fn build_argv(
    service: &str,
    operation: &str,
    args: &[String],
    region: Option<&str>,
    profile: Option<&str>,
    output: Option<&str>,
) -> Vec<String> {
    let mut argv = vec![
        "aws".to_string(),
        service.trim().to_string(),
        operation.trim().to_string(),
    ];
    for a in args {
        argv.push(a.clone());
    }
    if let Some(r) = region.map(str::trim).filter(|s| !s.is_empty()) {
        argv.push("--region".to_string());
        argv.push(r.to_string());
    }
    if let Some(p) = profile.map(str::trim).filter(|s| !s.is_empty()) {
        argv.push("--profile".to_string());
        argv.push(p.to_string());
    }
    if let Some(o) = output.map(str::trim).filter(|s| !s.is_empty()) {
        argv.push("--output".to_string());
        argv.push(o.to_string());
    }
    argv
}

/// Render a [`SandboxOutput`] into a `ToolResult`, truncating combined
/// output to [`MAX_OUTPUT_BYTES`].
pub fn output_to_result(output: SandboxOutput) -> ToolResult {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_code = output.exit_code;
    let mut content = format!(
        "Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
        exit_code, stdout, stderr
    );
    if content.len() > MAX_OUTPUT_BYTES {
        let cut = crate::truncate_utf8(&content, MAX_OUTPUT_BYTES).len();
        content.truncate(cut);
        content.push_str("\n... [output truncated]");
    }
    ToolResult {
        content,
        is_error: exit_code != 0,
    }
}

/// Extract a trimmed, non-empty string field from the tool input.
fn str_field<'a>(input: &'a Value, key: &str) -> Option<&'a str> {
    input
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Collect the optional `args` array into a `Vec<String>`. Non-string
/// entries are skipped.
fn args_field(input: &Value) -> Vec<String> {
    input
        .get("args")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn err_result(message: impl Into<String>) -> ToolResult {
    ToolResult {
        content: json!({ "error": message.into() }).to_string(),
        is_error: true,
    }
}

/// `aws_cli` tool — read-only AWS CLI wrapper.
pub struct AwsCliTool;

#[async_trait]
impl Tool for AwsCliTool {
    fn name(&self) -> &str {
        "aws_cli"
    }

    fn description(&self) -> &str {
        "Run a read-only AWS CLI command. Form: aws <service> <operation> [args]. \
         Only read operations are permitted: describe-*, list-*, get-*, head-*, lookup-*, \
         search-*, batch-get-*, plus ls/scan/query. Mutating operations (create-*, delete-*, \
         put-*, update-*, terminate-*, s3 rm, s3 sync, ...) are rejected. Examples: \
         {service:'ec2', operation:'describe-instances'}, {service:'s3', operation:'ls'}, \
         {service:'iam', operation:'get-user'}."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "service": {
                    "type": "string",
                    "description": "AWS service, e.g. 'ec2', 's3', 'iam', 'dynamodb'."
                },
                "operation": {
                    "type": "string",
                    "description": "Read-only operation, e.g. 'describe-instances', 'list-buckets', \
                                    'get-user', 'ls', 'scan'. Mutating operations are rejected."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional extra CLI arguments (e.g. '--instance-ids', 'i-123', \
                                    or an S3 URI for 's3 ls')."
                },
                "region": {
                    "type": "string",
                    "description": "Optional AWS region (--region)."
                },
                "profile": {
                    "type": "string",
                    "description": "Optional AWS named profile (--profile)."
                },
                "output": {
                    "type": "string",
                    "description": "Optional output format (--output): json, text, table, yaml."
                }
            },
            "required": ["service", "operation"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Only read-only operations reach the CLI, so a successful
        // invocation has no side effects.
        true
    }

    fn category(&self) -> ToolCategory {
        // Spawns a subprocess — categorize as Exec so hosts that gate
        // process-spawning tools behind approval catch this one.
        ToolCategory::Exec
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(service) = str_field(&input, "service") else {
            return err_result("Missing required parameter: 'service'");
        };
        let Some(operation) = str_field(&input, "operation") else {
            return err_result("Missing required parameter: 'operation'");
        };

        // Read-only allowlist check — refuse before spawning the CLI.
        if let OperationCheck::Rejected { reason } = validate_operation(operation) {
            return err_result(format!("Refused: {reason}"));
        }

        let args = args_field(&input);
        let argv = build_argv(
            service,
            operation,
            &args,
            str_field(&input, "region"),
            str_field(&input, "profile"),
            str_field(&input, "output"),
        );

        // Inherit network (the CLI must reach AWS) with a curated env —
        // PATH/HOME + non-secret AWS_* discovery vars, plus the AWS
        // credential vars force-allowed through for this tool so env-var
        // AWS auth works (D.1 R1 HIGH-2 + D.2 R2 MED).
        let manifest = SandboxManifest {
            network: NetworkPolicy::Inherit,
            env: crate::env_passthrough::build_sandboxed_env_with_force_allow(
                &[],
                AWS_ENV_PREFIXES,
                AWS_FORCE_ALLOW,
            ),
            ..Default::default()
        };
        let cmd = SandboxCommand { argv, cwd: None };

        let backend = default_for_platform();
        let timeout = Duration::from_millis(TIMEOUT_MS);

        match tokio::time::timeout(timeout, backend.execute(&manifest, cmd)).await {
            Ok(Ok(output)) => output_to_result(output),
            Ok(Err(e)) => err_result(format!("Failed to execute aws CLI: {e}")),
            Err(_) => err_result(format!("aws CLI command timed out after {TIMEOUT_MS}ms")),
        }
    }

    fn describe(&self, input: &Value) -> String {
        let service = input.get("service").and_then(Value::as_str).unwrap_or("");
        let operation = input.get("operation").and_then(Value::as_str).unwrap_or("");
        format!("aws {service} {operation}")
    }
}

/// Register the AWS CLI tool into `registry`.
pub fn register_aws_cli_tool(registry: &mut crate::registry::ToolRegistry) {
    registry.register(Box::new(AwsCliTool));
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----------------------------------------------------------------
    // Read-only allowlist validation.
    // ----------------------------------------------------------------

    #[test]
    fn read_only_operations_are_allowed() {
        for op in [
            "describe-instances",
            "list-buckets",
            "get-user",
            "head-object",
            "lookup-events",
            "search-faces",
            "batch-get-item",
            "ls",
            "scan",
            "query",
            // case-insensitive
            "Describe-Instances",
            "GET-USER",
        ] {
            assert_eq!(
                validate_operation(op),
                OperationCheck::Allowed,
                "expected '{op}' to be allowed"
            );
        }
    }

    #[test]
    fn mutating_operations_are_rejected() {
        for op in [
            "terminate-instances",
            "create-bucket",
            "delete-object",
            "put-object",
            "update-stack",
            "modify-instance-attribute",
            "run-instances",
            "start-instances",
            "stop-instances",
            "attach-volume",
            "rm",
            "mv",
            "cp",
            "sync",
            "set-identity-pool-roles",
            // unknown / not a recognized read op
            "frobnicate",
            "",
        ] {
            assert!(
                matches!(validate_operation(op), OperationCheck::Rejected { .. }),
                "expected '{op}' to be rejected"
            );
        }
    }

    #[test]
    fn s3_ls_allowed_but_s3_rm_rejected() {
        // `s3 ls` is the canonical S3 listing read.
        assert_eq!(validate_operation("ls"), OperationCheck::Allowed);
        // `s3 rm` / `s3 sync` / `s3 cp` / `s3 mv` are all mutating.
        assert!(matches!(
            validate_operation("rm"),
            OperationCheck::Rejected { .. }
        ));
        assert!(matches!(
            validate_operation("sync"),
            OperationCheck::Rejected { .. }
        ));
    }

    // ----------------------------------------------------------------
    // argv assembly.
    // ----------------------------------------------------------------

    #[test]
    fn build_argv_constructs_basic_command() {
        let argv = build_argv("ec2", "describe-instances", &[], None, None, None);
        assert_eq!(argv, vec!["aws", "ec2", "describe-instances"]);
    }

    #[test]
    fn build_argv_appends_args_and_region_profile_output_flags() {
        let argv = build_argv(
            "ec2",
            "describe-instances",
            &["--instance-ids".to_string(), "i-0abc".to_string()],
            Some("us-east-1"),
            Some("prod"),
            Some("json"),
        );
        assert_eq!(
            argv,
            vec![
                "aws",
                "ec2",
                "describe-instances",
                "--instance-ids",
                "i-0abc",
                "--region",
                "us-east-1",
                "--profile",
                "prod",
                "--output",
                "json",
            ]
        );

        // Blank / whitespace optional flags are dropped entirely.
        let argv = build_argv("s3", "ls", &[], Some("   "), Some(""), None);
        assert_eq!(argv, vec!["aws", "s3", "ls"]);
    }

    // ----------------------------------------------------------------
    // Output truncation.
    // ----------------------------------------------------------------

    #[test]
    fn output_to_result_truncates_oversized_output() {
        let big = "x".repeat(MAX_OUTPUT_BYTES * 2);
        let out = SandboxOutput {
            stdout: big.into_bytes(),
            stderr: Vec::new(),
            exit_code: 0,
            resource_limits: ResourceLimitEnforcement::None,
        };
        let result = output_to_result(out);
        assert!(
            result.content.len() <= MAX_OUTPUT_BYTES + 64,
            "content not truncated: {} bytes",
            result.content.len()
        );
        assert!(result.content.contains("[output truncated]"));
        assert!(!result.is_error);
    }

    #[test]
    fn output_to_result_marks_nonzero_exit_as_error() {
        let out = SandboxOutput {
            stdout: Vec::new(),
            stderr: b"could not connect".to_vec(),
            exit_code: 255,
            resource_limits: ResourceLimitEnforcement::None,
        };
        let result = output_to_result(out);
        assert!(result.is_error);
        assert!(result.content.contains("could not connect"));
    }

    // ----------------------------------------------------------------
    // execute() — input validation (no live aws binary needed).
    // ----------------------------------------------------------------

    #[test]
    fn execute_rejects_missing_fields_and_mutating_ops() {
        let tool = AwsCliTool;
        let run = |v: Value| futures::executor::block_on(tool.execute(v));

        // Missing service.
        let res = run(json!({ "operation": "describe-instances" }));
        assert!(res.is_error);
        assert!(res.content.contains("'service'"));

        // Missing operation.
        let res = run(json!({ "service": "ec2" }));
        assert!(res.is_error);
        assert!(res.content.contains("'operation'"));

        // Mutating operation rejected before any CLI spawn.
        let res = run(json!({ "service": "ec2", "operation": "terminate-instances" }));
        assert!(res.is_error);
        assert!(res.content.contains("Refused"));
        assert!(res.content.contains("terminate-instances"));
    }

    #[test]
    fn register_aws_cli_tool_populates_registry() {
        use crate::registry::ToolRegistry;
        let mut reg = ToolRegistry::new();
        register_aws_cli_tool(&mut reg);
        assert!(
            reg.tool_names().iter().any(|n| n == "aws_cli"),
            "aws_cli missing from registry"
        );
    }
}
