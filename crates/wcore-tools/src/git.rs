//! A1 Git tool — typed wrapper over the most-used git ops.
//!
//! **Security (Wave SA):** All ops invoke `git` directly via
//! `wcore_config::shell::shell_command_argv` — NO shell interpreter is
//! involved. Every LLM-supplied parameter (`cwd`, `path`, `paths[]`,
//! `name`, `message`, etc.) is passed as a SEPARATE argv entry, so shell
//! metacharacters in those values are NEVER interpreted as shell syntax.
//! This forecloses the BLOCKER #1 shell-injection class from the
//! v0.2.0 SECURITY audit.
//!
//! Working directory for each invocation is set via `.current_dir(cwd)`
//! on the `Command`, not via a `cd '<cwd>' && ...` prefix.
//!
//! Read-only ops report `is_concurrency_safe = true`; mutating ops
//! (add_*, commit, branch_checkout, stash_*) report `false` to keep them
//! off the parallel-tool path in the agent loop.
//!
//! No auto-commit. The `commit` op requires an explicit `message` field;
//! the agent supplies one (potentially generated via
//! `git_commit_message::commit_message_from_trace` — see T13).

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use wcore_config::shell::shell_command_argv;
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::context::ToolContext;

/// Typed git op variants — not consumed directly by the LLM (the tool input
/// is JSON with an `op` field), but useful for downstream introspection /
/// programmatic callers.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum GitOp {
    Status,
    Diff {
        path: Option<String>,
        staged: Option<bool>,
    },
    Log {
        limit: Option<usize>,
    },
    Blame {
        path: String,
        line: usize,
    },
    AddAll,
    AddPaths {
        paths: Vec<String>,
    },
    Commit {
        message: String,
    },
    BranchCurrent,
    BranchList,
    BranchCheckout {
        name: String,
        create: Option<bool>,
    },
    StashSave,
    StashPop,
}

pub struct GitTool;

/// Run a git invocation with arguments passed as separate argv entries
/// and `cwd` as the working directory. No shell wrapping, so the input
/// strings are safe regardless of shell-metacharacter content.
async fn run_git(cwd: &str, args: &[&str]) -> ToolResult {
    let mut cmd = shell_command_argv("git", args);
    cmd.current_dir(cwd);
    match cmd.output().await {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            let exit_code = output.status.code().unwrap_or(-1);
            let content = if !output.status.success() {
                format!("git exited {exit_code}: {}", stderr.trim())
            } else if stdout.is_empty() {
                stderr
            } else {
                stdout
            };
            ToolResult {
                content,
                is_error: !output.status.success(),
            }
        }
        Err(e) => ToolResult {
            content: format!("Git: failed to spawn git: {e}"),
            is_error: true,
        },
    }
}

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "Git"
    }

    fn description(&self) -> &str {
        "Read or mutate git state in the current repo. Pass an `op` field naming the operation \
         (status | diff | log | blame | add_all | add_paths | commit | branch_current | branch_list | \
         branch_checkout | stash_save | stash_pop). Read-only ops are safe to run in parallel. \
         Commit requires a non-empty `message`. Optional `cwd` overrides the working directory."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "op": { "type": "string" },
                "path": { "type": "string" },
                "staged": { "type": "boolean" },
                "limit": { "type": "integer" },
                "line": { "type": "integer" },
                "paths": { "type": "array", "items": { "type": "string" } },
                "message": { "type": "string" },
                "name": { "type": "string" },
                "create": { "type": "boolean" },
                "cwd": { "type": "string" }
            },
            "required": ["op"]
        })
    }

    fn is_concurrency_safe(&self, input: &Value) -> bool {
        matches!(
            input.get("op").and_then(|v| v.as_str()),
            Some("status")
                | Some("diff")
                | Some("log")
                | Some("blame")
                | Some("branch_current")
                | Some("branch_list")
        )
    }

    async fn execute(&self, input: Value) -> ToolResult {
        let op = match input.get("op").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                return ToolResult {
                    content: "Git: missing 'op' field".to_string(),
                    is_error: true,
                };
            }
        };
        let cwd = input.get("cwd").and_then(|v| v.as_str()).unwrap_or(".");
        match op {
            "status" => run_git(cwd, &["status", "--porcelain=v1", "--branch"]).await,
            "diff" => {
                let path = input.get("path").and_then(|v| v.as_str()).unwrap_or("");
                let staged = input
                    .get("staged")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                // Build argv: `git diff [--staged] [-- <path>]`. Each
                // element is a separate process arg; the `--` sentinel
                // makes `git` treat any subsequent values as paths even
                // if they begin with `-`.
                let mut args: Vec<&str> = vec!["diff"];
                if staged {
                    args.push("--staged");
                }
                if !path.is_empty() {
                    args.push("--");
                    args.push(path);
                }
                run_git(cwd, &args).await
            }
            "log" => {
                let limit = input.get("limit").and_then(|v| v.as_u64()).unwrap_or(20);
                let n = limit.to_string();
                run_git(cwd, &["log", "--pretty=format:%H%x09%an%x09%s", "-n", &n]).await
            }
            "blame" => {
                let path = match input.get("path").and_then(|v| v.as_str()) {
                    Some(p) => p,
                    None => {
                        return ToolResult {
                            content: "Git::Blame requires 'path'".to_string(),
                            is_error: true,
                        };
                    }
                };
                let line = input.get("line").and_then(|v| v.as_u64()).unwrap_or(1);
                let range = format!("{line},{line}");
                // `git blame -L <range> -- <path>` — argv mode, no shell.
                run_git(cwd, &["blame", "-L", &range, "--", path]).await
            }
            "add_all" => run_git(cwd, &["add", "-A"]).await,
            "add_paths" => {
                let paths: Vec<String> = input
                    .get("paths")
                    .and_then(|v| v.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|p| p.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                if paths.is_empty() {
                    return ToolResult {
                        content: "Git::AddPaths requires non-empty 'paths'".to_string(),
                        is_error: true,
                    };
                }
                // `git add -- <p1> <p2> ...` — `--` sentinel guards
                // against paths beginning with `-`.
                let mut args: Vec<&str> = vec!["add", "--"];
                for p in &paths {
                    args.push(p.as_str());
                }
                run_git(cwd, &args).await
            }
            "commit" => {
                let message = match input.get("message").and_then(|v| v.as_str()) {
                    Some(m) if !m.is_empty() => m,
                    _ => {
                        return ToolResult {
                            content: "Git::Commit requires non-empty 'message'".to_string(),
                            is_error: true,
                        };
                    }
                };
                // Message is a single argv entry — no quoting / escaping
                // needed; shell metacharacters in the message body are
                // never interpreted.
                run_git(cwd, &["commit", "-m", message]).await
            }
            "branch_current" => run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).await,
            "branch_list" => run_git(cwd, &["branch", "--format=%(refname:short)"]).await,
            "branch_checkout" => {
                let name = match input.get("name").and_then(|v| v.as_str()) {
                    Some(n) => n,
                    None => {
                        return ToolResult {
                            content: "Git::BranchCheckout requires 'name'".to_string(),
                            is_error: true,
                        };
                    }
                };
                let create = input
                    .get("create")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let mut args: Vec<&str> = vec!["checkout"];
                if create {
                    args.push("-b");
                }
                // `--` separates options from the ref name so a ref name
                // beginning with `-` cannot be misread as a flag.
                args.push("--");
                args.push(name);
                run_git(cwd, &args).await
            }
            "stash_save" => run_git(cwd, &["stash", "push", "-m", "wcore-stash"]).await,
            "stash_pop" => run_git(cwd, &["stash", "pop"]).await,
            other => ToolResult {
                content: format!("Git: unknown op '{other}'"),
                is_error: true,
            },
        }
    }

    /// W8b — vfs-aware variant. Git shells out (no direct `ctx.vfs`
    /// reads), but the optional `cwd` argument is the sandbox-sensitive
    /// surface: a sub-agent must not be able to `git commit` against a
    /// repo outside its workspace. The guard probes `ctx.vfs.exists()`
    /// on the resolved cwd before invoking the shell command.
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let cwd = input.get("cwd").and_then(|v| v.as_str()).unwrap_or(".");
        let cwd_path = std::path::Path::new(cwd);
        if let Err(e) = ctx.vfs.exists(cwd_path).await {
            return ToolResult {
                content: format!("Git refused: cwd {cwd:?} rejected by sandbox: {e}"),
                is_error: true,
            };
        }
        self.execute(input).await
    }

    fn category(&self) -> ToolCategory {
        // Worst-case category — Git mutates state on add/commit/checkout/stash.
        // The trait signature is `fn category(&self) -> ToolCategory` (no input
        // arg), so per-op categorisation isn't possible here. Parallel-batch
        // routing uses `is_concurrency_safe(input)` for the per-op read-only
        // detection.
        ToolCategory::Exec
    }

    fn describe(&self, input: &Value) -> String {
        let op = input
            .get("op")
            .and_then(|v| v.as_str())
            .unwrap_or("(missing op)");
        format!("Git::{op}")
    }
}
