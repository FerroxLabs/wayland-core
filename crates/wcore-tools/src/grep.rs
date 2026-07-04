use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};
use wcore_config::shell::shell_command_argv;

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::context::ToolContext;

pub struct GrepTool;

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "Grep"
    }

    fn description(&self) -> &str {
        "Searches file contents using regex patterns (powered by ripgrep).\n\n\
         IMPORTANT: ALWAYS use this Grep tool for content search. \
         NEVER run grep or rg as a Bash command.\n\n\
         - Supports full regex syntax (e.g., \"log.*Error\", \"fn\\\\s+\\\\w+\").\n\
         - Use the glob parameter to filter by file pattern (e.g., \"*.rs\").\n\
         - Output is truncated to 250 lines.\n\
         - Set case_insensitive to true for case-insensitive search."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "The regex pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Directory to search in (default: cwd)"
                },
                "glob": {
                    "type": "string",
                    "description": "File filter pattern, e.g. \"*.rs\""
                },
                "case_insensitive": {
                    "type": "boolean",
                    "description": "Case insensitive search"
                }
            },
            "required": ["pattern"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        true
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // No `ToolContext` here, so no jail root to anchor the scan to.
        run_grep(&input, None).await
    }

    /// W8b — vfs-aware variant. Grep itself shells out to rg/grep so it
    /// doesn't go through `ctx.vfs` for the actual scan, but it does
    /// gate the user-supplied `path` argument through `ctx.vfs.exists()`
    /// first. For top-level RealFs that's a no-op; for sandboxed sub-
    /// agents, paths outside the sandbox return OutsideSandbox and the
    /// tool refuses to launch the subprocess.
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let path_arg = input["path"].as_str().unwrap_or(".");
        let path = Path::new(path_arg);
        // Containment probe — only the error variant matters; we don't
        // care whether the path currently exists, just that the vfs
        // would allow access to it.
        if let Err(e) = ctx.vfs.exists(path).await {
            return ToolResult {
                content: format!("Grep refused: path {path_arg:?} rejected by sandbox: {e}"),
                is_error: true,
            };
        }
        // F36: anchor the subprocess working directory to the sandbox root so a
        // relative search path (the default ".") resolves against the jail, not
        // the process cwd — mirroring how Read/Write/Edit resolve against the
        // jail root. `None` for an unconstrained vfs (top-level RealFs) leaves
        // the subprocess in the process cwd, preserving existing behaviour.
        run_grep(&input, ctx.vfs.root()).await
    }

    fn max_result_size(&self) -> usize {
        20_000
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Info
    }

    fn describe(&self, input: &Value) -> String {
        let pattern = input.get("pattern").and_then(|v| v.as_str()).unwrap_or("");
        let path = input.get("path").and_then(|v| v.as_str()).unwrap_or(".");
        format!("Grep '{}' in {}", pattern, path)
    }
}

/// Shared entry point for both `execute` and `execute_with_ctx`. `search_root`
/// is the jail root the subprocess should run inside (`Some` for a sandboxed
/// sub-agent, `None` for the unconstrained top-level case).
async fn run_grep(input: &Value, search_root: Option<&Path>) -> ToolResult {
    let Some(pattern) = input["pattern"].as_str() else {
        return ToolResult {
            content: "Missing required parameter: pattern".to_string(),
            is_error: true,
        };
    };
    let path = input["path"].as_str().unwrap_or(".");
    let glob_pattern = input["glob"].as_str();
    let case_insensitive = input["case_insensitive"].as_bool().unwrap_or(false);

    // Try ripgrep first, fallback to grep.
    match try_ripgrep(pattern, path, glob_pattern, case_insensitive, search_root).await {
        Ok(output) => output,
        Err(_) => try_grep(pattern, path, case_insensitive, search_root).await,
    }
}

async fn try_ripgrep(
    pattern: &str,
    path: &str,
    glob_pattern: Option<&str>,
    case_insensitive: bool,
    search_root: Option<&Path>,
) -> Result<ToolResult, std::io::Error> {
    // F43: route through `wcore_config::shell::shell_command_argv` for
    // cross-platform PATHEXT resolution and kill-on-drop, rather than
    // `Command::new` directly. Still argv mode — the pattern/path reach `rg`
    // as literal argv entries, no shell ever interprets them.
    let mut args: Vec<&str> = vec!["--no-config", "-n"];
    if let Some(g) = glob_pattern {
        args.push("--glob");
        args.push(g);
    }
    if case_insensitive {
        args.push("-i");
    }
    // `--` terminates option parsing: a model-supplied pattern such as
    // `--pre=<cmd>` is then treated as a search pattern, not a ripgrep flag
    // (which would otherwise allow arbitrary per-file command execution).
    args.push("--");
    args.push(pattern);
    args.push(path);

    let mut cmd = shell_command_argv("rg", &args);
    // F36: anchor the scan inside the jail root so a relative `path` resolves
    // against the sandbox, not the process cwd.
    if let Some(root) = search_root {
        cmd.current_dir(root);
    }

    let output = cmd.output().await?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if output.status.code() == Some(1) && stdout.is_empty() {
        return Ok(ToolResult {
            content: "No matches found".to_string(),
            is_error: false,
        });
    }

    if !output.status.success() && output.status.code() != Some(1) {
        return Ok(ToolResult {
            content: format!("rg error: {}", stderr),
            is_error: true,
        });
    }

    // Truncate to 250 lines (global limit, not per-file)
    let lines: Vec<&str> = stdout.lines().take(250).collect();
    Ok(ToolResult {
        content: lines.join("\n"),
        is_error: false,
    })
}

async fn try_grep(
    pattern: &str,
    path: &str,
    case_insensitive: bool,
    search_root: Option<&Path>,
) -> ToolResult {
    // F43: route through `shell_command_argv` (argv mode, no shell) on both
    // platforms for consistent PATHEXT resolution + kill-on-drop.
    let mut cmd = if cfg!(windows) {
        // F35: pass the pattern via `/R /C:<pattern>` rather than a bare
        // positional arg. findstr treats any positional arg beginning with `/`
        // as a switch (it has no `--` terminator), so a pattern like `/C:foo`
        // was consumed as an option. `/C:` names the search string explicitly,
        // and `/R` keeps it a REGULAR EXPRESSION — preserving the regex
        // semantics the bare-`/R` form had (and matching the Unix `grep`/`rg`
        // regex contract). The `/C:` value is a single argv entry, so a leading
        // `/` in the pattern can no longer be switch-parsed.
        let dir = format!("{}\\*", path.trim_end_matches(['\\', '/']));
        let cflag = format!("/C:{pattern}");
        let mut args: Vec<&str> = vec!["/S", "/N", "/R"];
        if case_insensitive {
            args.push("/I");
        }
        args.push(&cflag);
        args.push(&dir);
        shell_command_argv("findstr", &args)
    } else {
        let mut args: Vec<&str> = vec!["-rn"];
        if case_insensitive {
            args.push("-i");
        }
        // `--` stops option parsing so a pattern beginning with `-` cannot be
        // interpreted as a grep flag.
        args.push("--");
        args.push(pattern);
        args.push(path);
        shell_command_argv("grep", &args)
    };
    // F36: contain the scan to the jail root (see `try_ripgrep`).
    if let Some(root) = search_root {
        cmd.current_dir(root);
    }

    match cmd.output().await {
        Ok(output) => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            // #661 — POSIX grep / Windows findstr exit codes: 0 = matches,
            // 1 = no matches, >=2 = a real error (bad regex, unreadable path,
            // permission denied). Previously ANY empty stdout was reported as
            // "No matches found" with is_error=false, so an exit-2 failure was
            // swallowed and the model concluded the symbol was undefined and
            // safe to delete. Mirror try_ripgrep: surface a real error loudly.
            if !output.status.success() && output.status.code() != Some(1) {
                ToolResult {
                    content: format!("grep error: {}", stderr.trim()),
                    is_error: true,
                }
            } else if stdout.is_empty() {
                ToolResult {
                    content: "No matches found".to_string(),
                    is_error: false,
                }
            } else {
                let lines: Vec<&str> = stdout.lines().take(250).collect();
                ToolResult {
                    content: lines.join("\n"),
                    is_error: false,
                }
            }
        }
        Err(e) => ToolResult {
            content: format!("grep failed: {}", e),
            is_error: true,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// #661: a real grep error (exit >= 2 — here an unreadable/nonexistent
    /// target) must be surfaced as `is_error: true`, not swallowed as
    /// "No matches found". Previously `try_grep` only checked whether stdout
    /// was empty, so an exit-2 failure looked identical to a clean no-match.
    #[cfg(unix)]
    #[tokio::test]
    async fn try_grep_reports_real_error_not_no_matches() {
        // `grep -rn -- pattern <nonexistent>` exits 2 with empty stdout.
        let out = try_grep(
            "pattern",
            "this_path_does_not_exist_9f3a2b.txt",
            false,
            None,
        )
        .await;
        assert!(
            out.is_error,
            "grep exit-2 must be is_error=true, got: {}",
            out.content
        );
        assert!(
            !out.content.contains("No matches found"),
            "a real error must not be reported as a clean no-match: {}",
            out.content
        );
    }

    #[tokio::test]
    async fn grep_tool_finds_pattern_in_own_source() {
        let tool = GrepTool;
        let input = json!({
            "pattern": "GrepTool",
            "path": env!("CARGO_MANIFEST_DIR")
        });
        let result = tool.execute(input).await;
        assert!(!result.is_error, "grep failed: {}", result.content);
        assert!(result.content.contains("GrepTool"));
    }

    /// F36 — under a `SandboxedFs` jail, a relative search path (the default
    /// ".") must resolve against the JAIL ROOT, not the process cwd. We plant a
    /// marker file inside a tempdir jail (and NOT in the process cwd) and assert
    /// the grep finds it via `path: "."` — which is only possible if the
    /// subprocess ran with `.current_dir(jail_root)`.
    #[cfg(unix)]
    #[tokio::test]
    async fn grep_relative_path_is_contained_to_the_jail_root() {
        use crate::context::ToolContext;
        use crate::vfs::{RealFs, SandboxedFs};
        use std::sync::Arc;

        let jail = tempfile::tempdir().expect("tempdir");
        let marker = "WAYLAND_GREP_JAIL_MARKER_F36";
        std::fs::write(jail.path().join("needle.txt"), format!("{marker}\n"))
            .expect("write marker into the jail");

        let mut ctx = ToolContext::test_default();
        ctx.vfs = Arc::new(SandboxedFs::new(RealFs, jail.path()));

        let tool = GrepTool;
        // Default path "." — must be anchored to the jail, not the test's cwd.
        let input = json!({ "pattern": marker, "path": "." });
        let result = tool.execute_with_ctx(input, &ctx).await;

        assert!(!result.is_error, "grep failed: {}", result.content);
        assert!(
            result.content.contains(marker),
            "relative '.' grep must find the marker inside the jail root, got: {}",
            result.content
        );
    }
}
