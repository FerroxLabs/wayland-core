//! T12 (v0.6.3 Tier 2B) — read-only `kubectl` wrapper tool.
//!
//! Wraps the host `kubectl` CLI but accepts **only** read-only verbs. The
//! allowed set is a closed allowlist ([`READ_ONLY_VERBS`]); every verb not
//! in that list — including every mutating verb (`apply`, `delete`,
//! `create`, `edit`, `patch`, `scale`, `rollout`, `exec`, `cp`, `drain`,
//! `cordon`, `label`, `annotate`, …) — is rejected *before* any process is
//! spawned. Rejection is by allowlist, never denylist: anything unknown is
//! refused.
//!
//! Like [`crate::bash::BashTool`], the actual invocation is routed through
//! the sandbox backend (`wcore_sandbox::default_for_platform()` +
//! `SandboxBackend::execute`) so the kubectl child is filesystem- and
//! syscall-confined per Tier S — never a raw `Command::new`.
//!
//! Output is truncated to keep a verbose `kubectl describe` from blowing
//! the model's context window.

use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_protocol::events::ToolCategory;
use wcore_sandbox::{
    NetworkPolicy, SandboxCommand, SandboxManifest, SandboxOutput, default_for_platform,
};
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;

/// Closed allowlist of `kubectl` verbs this tool will run. Every verb here
/// is read-only — it inspects cluster state but never mutates it. A verb
/// absent from this list is rejected unconditionally; this is the security
/// boundary of the tool, so keep it auditable: one literal per line.
const READ_ONLY_VERBS: &[&str] = &[
    "get",
    "describe",
    "logs",
    "top",
    "version",
    "cluster-info",
    "api-resources",
    "explain",
];

/// kubectl can take a while on a slow cluster; cap it so a hung apiserver
/// doesn't wedge the tool dispatch loop.
const KUBECTL_TIMEOUT_MS: u64 = 60_000;

/// Truncate kubectl output to this many bytes before returning it.
const MAX_OUTPUT_BYTES: usize = 50_000;

/// Returns `true` iff `verb` is in the read-only allowlist.
///
/// `pub` so the test suite can assert the allowlist directly without a
/// live cluster.
pub fn is_read_only_verb(verb: &str) -> bool {
    READ_ONLY_VERBS.contains(&verb)
}

/// Validate a requested verb against [`READ_ONLY_VERBS`].
///
/// Returns `Ok(verb)` for an allowed verb, or `Err(reason)` describing why
/// the verb was refused. This runs *before* any argv is built or any
/// process is spawned.
pub fn validate_verb(verb: &str) -> Result<&str, String> {
    let v = verb.trim();
    if v.is_empty() {
        return Err("kubectl `verb` is required and must be non-empty.".to_string());
    }
    if is_read_only_verb(v) {
        Ok(v)
    } else {
        Err(format!(
            "Refused: `{v}` is not a read-only kubectl verb. This tool only \
             permits these verbs: {}. Mutating operations (apply, delete, \
             create, edit, patch, scale, rollout, exec, cp, drain, cordon, \
             label, annotate, ...) are not allowed.",
            READ_ONLY_VERBS.join(", ")
        ))
    }
}

/// Assemble the full `kubectl` argv from a validated verb plus optional
/// extra args, namespace, and context.
///
/// The verb is assumed already validated by [`validate_verb`]. `args` are
/// passed through verbatim (resource names, label selectors, `-o` flags,
/// etc.); `namespace` / `context` become `--namespace` / `--context`
/// flags when present. A pure function so argv construction is unit-tested
/// without needing `kubectl` on `PATH`.
pub fn build_argv(
    verb: &str,
    args: &[String],
    namespace: Option<&str>,
    context: Option<&str>,
) -> Vec<String> {
    let mut argv: Vec<String> = Vec::with_capacity(args.len() + 6);
    argv.push("kubectl".to_string());
    argv.push(verb.to_string());
    if let Some(ns) = namespace {
        let ns = ns.trim();
        if !ns.is_empty() {
            argv.push("--namespace".to_string());
            argv.push(ns.to_string());
        }
    }
    if let Some(ctx) = context {
        let ctx = ctx.trim();
        if !ctx.is_empty() {
            argv.push("--context".to_string());
            argv.push(ctx.to_string());
        }
    }
    argv.extend(args.iter().cloned());
    argv
}

/// Extract the `args` array from the tool input as a `Vec<String>`.
/// Non-string array entries are skipped; a missing `args` yields an empty
/// vec.
fn parse_args(input: &Value) -> Vec<String> {
    input
        .get("args")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Truncate `s` to at most [`MAX_OUTPUT_BYTES`], snapping to a char
/// boundary and appending a notice when output was cut.
fn truncate_output(s: &str) -> String {
    if s.len() <= MAX_OUTPUT_BYTES {
        return s.to_string();
    }
    let head = crate::truncate_utf8(s, MAX_OUTPUT_BYTES);
    format!(
        "{head}\n... [truncated: {} of {} bytes shown]",
        head.len(),
        s.len()
    )
}

/// Extra (non-secret) env vars kubectl needs for config discovery. The
/// curated builder already passes `HOME` / `PATH`; `KUBECONFIG` points at
/// the kubeconfig file and is not a secret. The kubeconfig itself may
/// hold credentials, but it is a file path here, not a secret value.
const KUBECTL_ENV_ALLOW: &[&str] = &["KUBECONFIG"];

/// Build the sandbox manifest + command for a kubectl argv.
///
/// D.1 Round 1 (HIGH-2): the env is now a *curated* passthrough via
/// [`crate::env_passthrough::build_sandboxed_env`] — `PATH` / `HOME` /
/// `KUBECONFIG` reach the child so kubectl finds its config, but provider
/// API keys, `WAYLAND_VAULT_PASSPHRASE`, and other secret-shaped vars are
/// dropped (previously the full host env was copied). `NetworkPolicy::Inherit`
/// keeps the apiserver reachable; filesystem / syscall confinement still
/// applies through the real backend.
fn build_sandbox_pieces(argv: Vec<String>) -> (SandboxManifest, SandboxCommand) {
    let manifest = SandboxManifest {
        network: NetworkPolicy::Inherit,
        env: crate::env_passthrough::build_sandboxed_env(KUBECTL_ENV_ALLOW),
        ..Default::default()
    };
    (manifest, SandboxCommand { argv, cwd: None })
}

/// Render a `SandboxOutput` into a `ToolResult`, truncating the combined
/// output.
fn output_to_result(output: SandboxOutput) -> ToolResult {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_code = output.exit_code;
    let content = format!(
        "Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
        exit_code, stdout, stderr
    );
    ToolResult {
        content: truncate_output(&content),
        is_error: exit_code != 0,
    }
}

/// `kubectl` — read-only Kubernetes CLI wrapper.
///
/// Zero-state tool. Concurrency-safe: every invocation is independent and
/// the sandbox backend owns its own child process.
#[derive(Default)]
pub struct KubectlTool;

impl KubectlTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for KubectlTool {
    fn name(&self) -> &str {
        "kubectl"
    }

    fn description(&self) -> &str {
        "Runs the host `kubectl` CLI restricted to READ-ONLY verbs only. \
         Allowed verbs: get, describe, logs, top, version, cluster-info, \
         api-resources, explain. Every mutating verb (apply, delete, \
         create, edit, patch, scale, rollout, exec, cp, drain, cordon, \
         label, annotate, ...) is rejected before execution.\n\n\
         Parameters:\n\
         - verb: the kubectl subcommand (must be in the read-only list).\n\
         - args: array of extra arguments (resource type/name, -o flags, \
         label selectors, etc.).\n\
         - namespace: optional, becomes --namespace.\n\
         - context: optional, becomes --context.\n\n\
         The command runs inside the sandbox. Requires kubectl to be \
         installed and a working kubeconfig on the host."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "verb": {
                    "type": "string",
                    "enum": READ_ONLY_VERBS,
                    "description": "Read-only kubectl subcommand to run."
                },
                "args": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Extra arguments (resource type/name, -o flags, selectors)."
                },
                "namespace": {
                    "type": "string",
                    "description": "Optional namespace (passed as --namespace)."
                },
                "context": {
                    "type": "string",
                    "description": "Optional kubeconfig context (passed as --context)."
                }
            },
            "required": ["verb"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        // Read-only against the cluster, independent child process per call.
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let Some(verb) = input.get("verb").and_then(|v| v.as_str()) else {
            return ToolResult {
                content: "Missing required parameter: verb".to_string(),
                is_error: true,
            };
        };

        // Allowlist check FIRST — before any argv is built or process spawned.
        let verb = match validate_verb(verb) {
            Ok(v) => v,
            Err(reason) => {
                return ToolResult {
                    content: reason,
                    is_error: true,
                };
            }
        };

        let args = parse_args(&input);
        let namespace = input.get("namespace").and_then(|v| v.as_str());
        let context = input.get("context").and_then(|v| v.as_str());
        let argv = build_argv(verb, &args, namespace, context);

        let backend = default_for_platform();
        let (manifest, cmd) = build_sandbox_pieces(argv);
        let timeout = Duration::from_millis(KUBECTL_TIMEOUT_MS);

        match tokio::time::timeout(timeout, backend.execute(&manifest, cmd)).await {
            Ok(Ok(output)) => output_to_result(output),
            Ok(Err(e)) => ToolResult {
                content: format!("Failed to execute kubectl: {e}"),
                is_error: true,
            },
            Err(_) => ToolResult {
                content: format!("kubectl timed out after {KUBECTL_TIMEOUT_MS}ms"),
                is_error: true,
            },
        }
    }

    fn max_result_size(&self) -> usize {
        MAX_OUTPUT_BYTES
    }

    fn category(&self) -> ToolCategory {
        // Inspects external (cluster) state but never mutates it.
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let verb = input
            .get("verb")
            .and_then(|v| v.as_str())
            .unwrap_or("(missing verb)");
        let args = parse_args(input);
        if args.is_empty() {
            format!("kubectl {verb}")
        } else {
            format!("kubectl {verb} {}", args.join(" "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn read_only_verb_builds_the_right_argv() {
        // A `get` invocation with a resource name produces the expected
        // `kubectl get pods` argv.
        let verb = validate_verb("get").expect("get is read-only");
        let args = vec!["pods".to_string()];
        let argv = build_argv(verb, &args, None, None);
        assert_eq!(argv, vec!["kubectl", "get", "pods"]);
    }

    #[test]
    fn mutating_verb_is_rejected_before_execution() {
        // `delete` is a mutating verb — it must never validate, so no argv
        // is ever built and no process is spawned.
        let err = validate_verb("delete").expect_err("delete must be refused");
        assert!(err.contains("not a read-only kubectl verb"), "msg: {err}");
        assert!(!is_read_only_verb("delete"));
        // Spot-check the rest of the mutating set is also refused.
        for mutating in [
            "apply", "create", "edit", "patch", "scale", "rollout", "exec", "cp", "drain",
            "cordon", "label", "annotate",
        ] {
            assert!(
                validate_verb(mutating).is_err(),
                "`{mutating}` must be rejected"
            );
            assert!(!is_read_only_verb(mutating));
        }
    }

    #[test]
    fn unknown_verb_is_rejected() {
        // A verb that is neither read-only nor a known mutating verb is
        // still refused — the allowlist is closed.
        assert!(validate_verb("frobnicate").is_err());
        assert!(validate_verb("").is_err());
        assert!(validate_verb("   ").is_err());
        // Every allowed verb, conversely, validates.
        for ok in READ_ONLY_VERBS {
            assert!(validate_verb(ok).is_ok(), "`{ok}` should be allowed");
        }
    }

    #[test]
    fn argv_includes_namespace_and_context_flags() {
        // namespace + context become --namespace / --context, inserted
        // before the trailing positional args.
        let verb = validate_verb("describe").unwrap();
        let args = vec!["pod".to_string(), "my-pod".to_string()];
        let argv = build_argv(verb, &args, Some("kube-system"), Some("prod"));
        assert_eq!(
            argv,
            vec![
                "kubectl",
                "describe",
                "--namespace",
                "kube-system",
                "--context",
                "prod",
                "pod",
                "my-pod",
            ]
        );
        // Blank / whitespace-only namespace/context are dropped entirely.
        let bare = build_argv(verb, &[], Some("  "), None);
        assert_eq!(bare, vec!["kubectl", "describe"]);
    }

    #[test]
    fn output_truncation_caps_long_output() {
        // Output longer than the cap is truncated with a notice; short
        // output is returned verbatim.
        let short = "kubectl said hello";
        assert_eq!(truncate_output(short), short);

        let long = "x".repeat(MAX_OUTPUT_BYTES + 5_000);
        let truncated = truncate_output(&long);
        assert!(truncated.len() < long.len());
        assert!(truncated.contains("[truncated"));
    }

    #[tokio::test]
    async fn execute_rejects_missing_verb() {
        let tool = KubectlTool::new();
        let res = tool.execute(json!({})).await;
        assert!(res.is_error);
        assert!(res.content.contains("verb"));
    }

    #[tokio::test]
    async fn execute_rejects_mutating_verb_without_spawning() {
        // `execute` must reject a mutating verb at the validation gate,
        // not by failing the spawn — so this passes even without kubectl
        // installed.
        let tool = KubectlTool::new();
        let res = tool
            .execute(json!({ "verb": "delete", "args": ["pod", "x"] }))
            .await;
        assert!(res.is_error);
        assert!(res.content.contains("not a read-only kubectl verb"));
    }

    #[test]
    fn describe_renders_verb_and_args() {
        let tool = KubectlTool::new();
        let d = tool.describe(&json!({ "verb": "get", "args": ["pods", "-A"] }));
        assert_eq!(d, "kubectl get pods -A");
    }

    /// Live-cluster smoke test — requires `kubectl` on PATH and a working
    /// kubeconfig, so it is `#[ignore]`d by default and only run with
    /// `cargo test -- --ignored` in an environment that has both.
    #[tokio::test]
    #[ignore = "requires kubectl installed and a reachable cluster"]
    async fn execute_version_against_live_kubectl() {
        let tool = KubectlTool::new();
        let res = tool
            .execute(json!({ "verb": "version", "args": ["--client"] }))
            .await;
        assert!(!res.is_error, "kubectl version failed: {}", res.content);
    }
}
