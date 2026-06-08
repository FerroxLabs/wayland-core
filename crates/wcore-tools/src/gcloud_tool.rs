// T13 (v0.6.3 Tier 2B): read-only `gcloud` CLI wrapper.
//
// Same posture as T12 (kubectl): wrap the host `gcloud` binary but
// permit READ-ONLY operations ONLY. `gcloud`'s verb is the LAST word of
// the command group — `gcloud compute instances list`, `gcloud projects
// describe`. We validate that final verb against a read-only ALLOWLIST
// *before* execution; anything not on the list (`create`, `delete`,
// `set-*`, `deploy`, ...) is rejected loud.
//
// Execution mirrors `bash.rs`: the assembled argv runs through the
// platform sandbox backend (`default_for_platform()` +
// `SandboxBackend::execute`) with host-env inheritance and
// `NetworkPolicy::Inherit` (gcloud is an API client and needs egress).
// All sandboxed via Tier S.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_sandbox::{
    NetworkPolicy, SandboxCommand, SandboxManifest, SandboxOutput, default_for_platform,
};
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Wall-clock timeout for a gcloud invocation. gcloud round-trips to
/// Google APIs; 120s matches BashTool's default.
const TIMEOUT_MS: u64 = 120_000;

/// Output is returned verbatim to the model; cap it so a large `list`
/// cannot blow the context window.
const MAX_OUTPUT_BYTES: usize = 50_000;

/// Read-only verb ALLOWLIST.
///
/// A `gcloud` command's verb is the final word of the command group.
/// Only verbs that *observe* state are permitted. Validation is by
/// allowlist — any verb not in this set is rejected, so a newly-added
/// mutating gcloud verb fails closed rather than open.
///
/// Prefix-matched read families (`get-*`, `describe-*`) are handled
/// separately in [`verb_is_read_only`].
///
/// D.1 Round 1 (MEDIUM): `print-access-token` / `print-identity-token`
/// were removed. They do not mutate cloud state but they print a live
/// OAuth2 bearer / OIDC identity token to stdout, which is returned
/// verbatim into the model context — a credential-exfiltration path. The
/// correct test is "does not emit a secret", not "does not mutate".
const READ_ONLY_VERBS: &[&str] = &["list", "describe", "info", "version", "versions"];

/// Read-only verb *prefixes*. gcloud exposes a family of getters
/// (`get-iam-policy`, `get-value`, `get-server-config`, ...) and a few
/// `describe-*` variants. Any verb starting with one of these prefixes
/// is treated as read-only.
const READ_ONLY_PREFIXES: &[&str] = &["get-", "describe-", "list-"];

/// Returns `true` if `verb` is a read-only gcloud verb.
///
/// Pure function — no process spawn. Tested directly.
pub fn verb_is_read_only(verb: &str) -> bool {
    let v = verb.trim().to_ascii_lowercase();
    if v.is_empty() {
        return false;
    }
    if READ_ONLY_VERBS.contains(&v.as_str()) {
        return true;
    }
    READ_ONLY_PREFIXES.iter().any(|p| v.starts_with(p))
}

/// Validate a parsed (`group`, `verb`) pair. Returns `Err(reason)` when
/// the verb is not on the read-only allowlist.
///
/// `pub` so tests can assert the policy without spawning `gcloud`.
pub fn validate_invocation(group: &[String], verb: &str) -> Result<(), String> {
    if verb.trim().is_empty() {
        return Err("Missing gcloud verb (the read-only operation, e.g. `list`).".to_string());
    }
    // Defense in depth: a caller could try to smuggle a verb into the
    // `group` segments. Reject any group token that itself looks like a
    // mutating verb is overkill — but we DO require every group token to
    // be a plain identifier (letters, digits, `-`) so flags / shell
    // metacharacters cannot ride in.
    for seg in group {
        if seg.is_empty() || !seg.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            return Err(format!(
                "Invalid gcloud command-group segment {seg:?}: only alphanumerics and `-` allowed."
            ));
        }
    }
    if !verb.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!(
            "Invalid gcloud verb {verb:?}: only alphanumerics and `-` allowed."
        ));
    }
    if !verb_is_read_only(verb) {
        return Err(format!(
            "Refused: `gcloud ... {verb}` is not a read-only operation. \
             GcloudTool permits read-only verbs only (list, describe, info, \
             versions, get-*, describe-*). Mutating verbs (create, delete, \
             update, set-*, deploy, enable, ...) are rejected."
        ));
    }
    Ok(())
}

/// Build the full `gcloud` argv from a validated invocation.
///
/// Layout: `gcloud <group...> <verb> [args...] [--project=P] [--format=F]`.
/// Caller MUST have run [`validate_invocation`] first.
///
/// `pub` so tests can assert argv assembly without spawning a process.
pub fn build_argv(
    group: &[String],
    verb: &str,
    args: &[String],
    project: Option<&str>,
    format: Option<&str>,
) -> Vec<String> {
    let mut argv = Vec::with_capacity(group.len() + args.len() + 4);
    argv.push("gcloud".to_string());
    for seg in group {
        argv.push(seg.clone());
    }
    argv.push(verb.to_string());
    for a in args {
        argv.push(a.clone());
    }
    if let Some(p) = project
        && !p.is_empty()
    {
        argv.push(format!("--project={p}"));
    }
    if let Some(f) = format
        && !f.is_empty()
    {
        argv.push(format!("--format={f}"));
    }
    argv
}

/// Render a `SandboxOutput` into a `ToolResult`, truncating stdout/stderr
/// so a large result set cannot overrun the model context.
fn output_to_result(output: SandboxOutput) -> ToolResult {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = crate::truncate_utf8(&stdout, MAX_OUTPUT_BYTES);
    let stderr = crate::truncate_utf8(&stderr, MAX_OUTPUT_BYTES);
    let content = format!(
        "Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
        output.exit_code, stdout, stderr
    );
    ToolResult {
        content,
        is_error: output.exit_code != 0,
    }
}

/// Non-secret env prefixes gcloud needs for config / account discovery.
/// `CLOUDSDK_*` carries config-dir + account + project settings; any
/// secret-shaped name under that prefix (e.g. `CLOUDSDK_AUTH_ACCESS_TOKEN`,
/// which contains `TOKEN`) is still dropped by the curated builder's
/// secret filter.
const GCLOUD_ENV_PREFIXES: &[&str] = &["CLOUDSDK_"];

/// Build the sandbox manifest for a gcloud invocation.
///
/// D.1 Round 1 (HIGH-2): the env is a *curated* passthrough via
/// [`crate::env_passthrough::build_sandboxed_env_with_prefixes`] —
/// `PATH` / `HOME` plus the `CLOUDSDK_*` discovery family reach the
/// child, but provider API keys, `WAYLAND_VAULT_PASSPHRASE`, and other
/// secret-shaped vars (including `CLOUDSDK_AUTH_ACCESS_TOKEN`) are
/// dropped (previously the full host env was copied). Network is
/// `Inherit` (gcloud is an API client).
fn build_manifest() -> SandboxManifest {
    SandboxManifest {
        network: NetworkPolicy::Inherit,
        env: crate::env_passthrough::build_sandboxed_env_with_prefixes(&[], GCLOUD_ENV_PREFIXES),
        ..Default::default()
    }
}

/// Read-only `gcloud` CLI wrapper.
pub struct GcloudTool;

#[async_trait]
impl Tool for GcloudTool {
    fn name(&self) -> &str {
        "gcloud"
    }

    fn description(&self) -> &str {
        "Runs the host `gcloud` (Google Cloud) CLI with READ-ONLY \
         operations only.\n\n\
         A gcloud command's verb is the LAST word of the command group, \
         e.g. `gcloud compute instances list` (group=[compute, instances], \
         verb=list) or `gcloud projects describe` (group=[projects], \
         verb=describe).\n\n\
         Only read-only verbs are permitted: list, describe, info, \
         versions, and get-* / describe-* families (e.g. get-iam-policy). \
         Mutating verbs (create, delete, update, set-*, add-*, remove-*, \
         deploy, enable, disable, reset, start, stop) are rejected before \
         execution.\n\n\
         Provide the command group, the verb, optional positional args, \
         and optional --project / --format flag values separately."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "group": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Command group segments before the verb, \
                                    e.g. [\"compute\", \"instances\"] or [\"projects\"]."
                },
                "verb": {
                    "type": "string",
                    "description": "The read-only operation (last word of the \
                                    command group): list, describe, info, \
                                    versions, or a get-*/describe-* verb."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional positional arguments passed after the verb."
                },
                "project": {
                    "type": "string",
                    "description": "Optional GCP project id; emitted as --project=<id>."
                },
                "format": {
                    "type": "string",
                    "description": "Optional gcloud --format value (e.g. json, yaml, table)."
                }
            },
            "required": ["group", "verb"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Read-only operations have no side effects.
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let group: Vec<String> = input
            .get("group")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let Some(verb) = input.get("verb").and_then(|v| v.as_str()) else {
            return ToolResult {
                content: "Missing required parameter: verb".to_string(),
                is_error: true,
            };
        };

        let args: Vec<String> = input
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let project = input.get("project").and_then(|v| v.as_str());
        let format = input.get("format").and_then(|v| v.as_str());

        // Read-only allowlist check — refuse before spawning gcloud.
        if let Err(reason) = validate_invocation(&group, verb) {
            return ToolResult {
                content: reason,
                is_error: true,
            };
        }

        let argv = build_argv(&group, verb, &args, project, format);

        let backend = default_for_platform();
        let manifest = build_manifest();
        let cmd = SandboxCommand { argv, cwd: None };

        let timeout = Duration::from_millis(TIMEOUT_MS);
        match tokio::time::timeout(timeout, backend.execute(&manifest, cmd)).await {
            Ok(Ok(output)) => output_to_result(output),
            Ok(Err(e)) => ToolResult {
                content: format!("Failed to execute gcloud: {e}"),
                is_error: true,
            },
            Err(_) => ToolResult {
                content: format!("gcloud command timed out after {TIMEOUT_MS}ms"),
                is_error: true,
            },
        }
    }

    fn describe(&self, input: &Value) -> String {
        let group = input
            .get("group")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        let verb = input.get("verb").and_then(|v| v.as_str()).unwrap_or("");
        format!("gcloud {group} {verb}").trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_only_verb_list_is_allowed() {
        let group = vec!["compute".to_string(), "instances".to_string()];
        assert!(validate_invocation(&group, "list").is_ok());
        assert!(verb_is_read_only("list"));
        assert!(verb_is_read_only("describe"));
        assert!(verb_is_read_only("get-iam-policy"));
        assert!(verb_is_read_only("DESCRIBE")); // case-insensitive
    }

    #[test]
    fn list_builds_expected_argv() {
        let group = vec!["compute".to_string(), "instances".to_string()];
        validate_invocation(&group, "list").expect("list must be allowed");
        let argv = build_argv(&group, "list", &[], None, None);
        assert_eq!(argv, vec!["gcloud", "compute", "instances", "list"]);
    }

    #[test]
    fn mutating_verb_delete_is_rejected() {
        let group = vec!["compute".to_string(), "instances".to_string()];
        let err = validate_invocation(&group, "delete").expect_err("delete must be rejected");
        assert!(err.contains("not a read-only operation"), "got: {err}");

        // Spot-check the rest of the mutating family.
        for verb in [
            "create",
            "update",
            "set-iam-policy",
            "add-iam-policy-binding",
            "deploy",
            "enable",
            "disable",
            "reset",
            "start",
            "stop",
            "remove-tags",
        ] {
            assert!(
                validate_invocation(&group, verb).is_err(),
                "{verb} must be rejected"
            );
            assert!(!verb_is_read_only(verb), "{verb} must not be read-only");
        }
    }

    #[test]
    fn unknown_verb_is_rejected() {
        let group = vec!["projects".to_string()];
        assert!(validate_invocation(&group, "frobnicate").is_err());
        assert!(!verb_is_read_only("frobnicate"));
        // Empty verb is rejected too.
        assert!(validate_invocation(&group, "").is_err());
        assert!(validate_invocation(&group, "   ").is_err());
    }

    #[test]
    fn print_token_verbs_are_rejected() {
        // D.1 Round 1 (MEDIUM): print-access-token / print-identity-token
        // emit live credentials to stdout — they must NOT be on the
        // read-only allowlist even though they do not mutate state.
        let group = vec!["auth".to_string()];
        for verb in ["print-access-token", "print-identity-token"] {
            assert!(
                !verb_is_read_only(verb),
                "{verb} must not be treated as read-only (it prints a credential)"
            );
            assert!(
                validate_invocation(&group, verb).is_err(),
                "{verb} must be rejected — it exfiltrates a token"
            );
        }
    }

    #[test]
    fn argv_assembly_with_project_and_format_flags() {
        let group = vec!["projects".to_string()];
        validate_invocation(&group, "describe").expect("describe must be allowed");
        let args = vec!["my-proj-123".to_string()];
        let argv = build_argv(&group, "describe", &args, Some("my-proj-123"), Some("json"));
        assert_eq!(
            argv,
            vec![
                "gcloud",
                "projects",
                "describe",
                "my-proj-123",
                "--project=my-proj-123",
                "--format=json",
            ]
        );
        // Empty flag values are not emitted.
        let argv_empty = build_argv(&group, "describe", &[], Some(""), Some(""));
        assert_eq!(argv_empty, vec!["gcloud", "projects", "describe"]);
    }

    #[test]
    fn output_is_truncated() {
        let big = "x".repeat(MAX_OUTPUT_BYTES + 5_000);
        let output = SandboxOutput {
            exit_code: 0,
            stdout: big.into_bytes(),
            stderr: Vec::new(),
            resource_limits: wcore_sandbox::ResourceLimitEnforcement::None,
        };
        let result = output_to_result(output);
        assert!(!result.is_error);
        // STDOUT segment must be clamped to MAX_OUTPUT_BYTES.
        assert!(
            result.content.len() < MAX_OUTPUT_BYTES + 200,
            "content not truncated: {} bytes",
            result.content.len()
        );
    }

    #[test]
    fn group_segment_with_metacharacter_is_rejected() {
        // A `;` smuggled into a group segment must fail validation.
        let group = vec!["compute; rm -rf /".to_string()];
        assert!(validate_invocation(&group, "list").is_err());
    }

    #[tokio::test]
    async fn execute_missing_verb_returns_error() {
        let tool = GcloudTool;
        let result = tool.execute(json!({"group": ["projects"]})).await;
        assert!(result.is_error);
        assert!(result.content.contains("verb"));
    }

    #[tokio::test]
    async fn execute_mutating_verb_returns_error_without_spawning() {
        let tool = GcloudTool;
        let result = tool
            .execute(json!({"group": ["compute", "instances"], "verb": "delete"}))
            .await;
        assert!(result.is_error);
        assert!(result.content.contains("not a read-only operation"));
    }
}
