use std::path::Path;

use async_trait::async_trait;
use serde_json::{Value, json};
use wcore_config::shell::shell_command_argv;

use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolResult};

use crate::Tool;
use crate::context::ToolContext;
use crate::grep_policy::{self, GrepScope};
use crate::path_validation::validate_search_root;
use crate::workspace_policy::is_secret_path_static;

/// The one string that means "the backend looked and found nothing". Shared so
/// `run_grep` can tell it apart from a match list without a magic literal.
const NO_MATCHES: &str = "No matches found";

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

    fn execution_class_for(&self, _input: &Value) -> crate::ToolExecutionClass {
        crate::ToolExecutionClass::ProcessSpawning
    }

    /// Grep spawns a search process but mutates nothing, so a failure — a
    /// refused traversal path, a missing target, a bad regex — is
    /// authoritative rather than an ambiguous external effect. See the note on
    /// `ReadTool::effect_contract` (live UAT D1).
    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        wcore_types::tool::repeat_safe_contract(wcore_types::tool::READ_ONLY_FILESYSTEM_RECONCILER)
    }

    /// Grep's own search subprocess is fixed and argv-invoked — no input it
    /// accepts can turn it into a mutation. Safe under `read_only`.
    fn read_only_safe(&self, _input: &Value) -> bool {
        true
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

    // #661, Windows half. The exit-code discipline in `try_ripgrep`/`try_grep`
    // is what stops an unreadable or absent target being reported as a clean
    // "No matches found" — the failure that let a model conclude a symbol was
    // undefined and safe to delete. POSIX `grep` and `rg` support it by exiting
    // 2 on a target they cannot open. `findstr` does NOT: measured on Windows
    // (`findstr /S /N /R /C:needle "..\escape\*"`) it exits 1 with EMPTY
    // stderr, byte-for-byte indistinguishable from a genuine no-match, so no
    // amount of exit-code or stderr inspection downstream can recover the
    // difference. The `#[cfg(unix)]` gate on `try_grep_reports_real_error_not_
    // no_matches` recorded that gap rather than closing it.
    //
    // Prove the target exists here instead, before any backend is chosen, so
    // the answer is the same on all three platforms and does not depend on
    // which of the three search binaries happens to be installed.
    // Grep is the read-path sibling of Read and returns matched line CONTENT,
    // so it must honour the same credential deny-list. It could not, because
    // `validate_user_path` refuses a relative path and a directory — both
    // legitimate here (`.` is the schema default). `validate_search_root`
    // applies the deny-list half to the RESOLVED root instead, which is also
    // the resolution `try_exists` and the subprocess need, so there is exactly
    // one place a search target is turned into a path.
    let resolved = match validate_search_root(Path::new(path), search_root) {
        Ok(p) => p,
        Err(e) => {
            return ToolResult {
                content: format!("Refused to search {path}: {e}"),
                is_error: true,
            };
        }
    };
    match tokio::fs::try_exists(&resolved).await {
        Ok(true) => {}
        Ok(false) => {
            return ToolResult {
                content: format!("grep error: no such file or directory: {path}"),
                is_error: true,
            };
        }
        Err(error) => {
            return ToolResult {
                content: format!("grep error: cannot access {path}: {error}"),
                is_error: true,
            };
        }
    }

    // SR-05. Grep returns matched line CONTENT, so naming a secret-shaped file
    // outright is the shortest exfil path there is. Refuse it here, before any
    // backend runs, rather than filtering its lines out afterwards — the
    // operator deserves to be told, and no backend gets the chance to read it.
    if is_secret_path_static(&resolved) && !resolved.is_dir() {
        return ToolResult {
            content: format!(
                "Refused to search {path}: it is a credential-bearing file \
                 (Grep returns matched line content)"
            ),
            is_error: true,
        };
    }

    // SR-04/SR-05. Decide what this search may report BEFORE choosing a
    // backend, so `rg`, `grep` and `findstr` are all held to the same answer.
    // See `grep_policy` for the rules and the reasoning.
    let scope = grep_policy::scope_for(&resolved);
    let base = match search_root {
        Some(root) => root.to_path_buf(),
        None => std::env::current_dir().unwrap_or_else(|_| resolved.clone()),
    };

    // Try ripgrep first, fallback to grep.
    let raw = match try_ripgrep(pattern, path, glob_pattern, case_insensitive, search_root).await {
        Ok(output) => output,
        Err(_) => try_grep(pattern, path, case_insensitive, search_root).await,
    };
    apply_policy(raw, &scope, &base)
}

/// The single place any backend's output is turned into a tool result. The
/// backends themselves neither filter nor truncate, which is what makes the
/// three of them incapable of diverging.
fn apply_policy(raw: ToolResult, scope: &GrepScope, base: &Path) -> ToolResult {
    if raw.is_error || raw.content == NO_MATCHES {
        return raw;
    }

    let filtered = scope.apply(&raw.content, base);
    // Truncate AFTER filtering: withheld lines must not consume the budget and
    // crowd out reportable matches.
    let mut lines: Vec<String> = filtered.lines.iter().take(250).cloned().collect();

    if let Some(footer) = filtered.footer() {
        lines.push(footer);
    } else if lines.is_empty() {
        return ToolResult {
            content: NO_MATCHES.to_string(),
            is_error: false,
        };
    }

    ToolResult {
        content: lines.join("\n"),
        is_error: false,
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
            content: NO_MATCHES.to_string(),
            is_error: false,
        });
    }

    if !output.status.success() && output.status.code() != Some(1) {
        return Ok(ToolResult {
            content: format!("rg error: {}", stderr),
            is_error: true,
        });
    }

    // Raw and untruncated: `run_grep` owns the ignore/secret policy and the
    // 250-line cap, so every backend is held to one answer.
    Ok(ToolResult {
        content: stdout.into_owned(),
        is_error: false,
    })
}

async fn try_grep(
    pattern: &str,
    path: &str,
    case_insensitive: bool,
    search_root: Option<&Path>,
) -> ToolResult {
    // #661 (Windows half) — resolve the target BEFORE spawning anything.
    //
    // The exit-code guard below is sufficient for POSIX `grep` (>=2 == error)
    // but is a no-op on Windows: `findstr` returns exit **1** with empty stdout
    // both for a clean no-match and for "FINDSTR: Cannot open <path>". Measured
    // on Windows 10.0.26200: an explicitly named missing file exits 1, and a
    // missing directory under the `<dir>\*` lowering exits 1 with no output at
    // all. So an unreadable target was reported as "No matches found" with
    // is_error=false — the model concludes the symbol is undefined and deletes
    // live code. A stat of the target makes "could not look" unrepresentable as
    // "looked and found nothing", on every platform.
    //
    // Resolve against `search_root` because that is the subprocess cwd (F36), so
    // a relative `path` must be interpreted the same way here as it will be
    // there.
    let target = match search_root {
        Some(root) => root.join(path),
        None => std::path::PathBuf::from(path),
    };
    let is_dir = match std::fs::metadata(&target) {
        Ok(meta) => meta.is_dir(),
        Err(e) => {
            return ToolResult {
                content: format!("grep error: cannot search {path:?}: {e}"),
                is_error: true,
            };
        }
    };

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
        //
        // #661 (single file) — findstr has no recursive-directory form, so a
        // DIRECTORY must be lowered to a `<dir>\*` wildcard. Applying that
        // lowering to a FILE yields `file.rs\*`, which matches nothing: exit 1,
        // empty stdout, reported as "No matches found" for a pattern the file
        // demonstrably contains. Pass a file through verbatim, and drop `/S`
        // with it so a same-named file in a subdirectory cannot be picked up
        // instead.
        //
        // #661 (drive walk) — build the spec from the RESOLVED absolute target,
        // never from the raw argument. `path.trim_end_matches(['\\', '/'])` maps
        // both "" and "/" to the empty string, so the spec became `\*` — the
        // root of the current drive — and `/S` then walked the entire drive.
        // (Measured: still running after 25s, against 157ms for the same scan
        // scoped to its intended directory.)
        let resolved = std::path::absolute(&target).unwrap_or_else(|_| target.clone());
        let resolved = resolved.to_string_lossy().into_owned();
        let spec = if is_dir {
            format!("{}\\*", resolved.trim_end_matches(['\\', '/']))
        } else {
            resolved
        };
        let cflag = format!("/C:{pattern}");
        let mut args: Vec<&str> = vec!["/N", "/R"];
        if is_dir {
            args.push("/S");
        }
        if case_insensitive {
            args.push("/I");
        }
        args.push(&cflag);
        args.push(&spec);
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
                    content: NO_MATCHES.to_string(),
                    is_error: false,
                }
            } else {
                // Raw and untruncated — see the note in `try_ripgrep`.
                ToolResult {
                    content: stdout.into_owned(),
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

    /// #661: an unreadable/nonexistent target must be surfaced as
    /// `is_error: true`, not swallowed as "No matches found" — otherwise the
    /// model concludes the symbol is undefined and deletes live code.
    ///
    /// This test is deliberately NOT `#[cfg(unix)]`. The original guard was,
    /// and so it could never execute on Windows — which is exactly where the
    /// defect was reported and where it was still live: POSIX `grep` signals a
    /// hard error with exit >=2, but `findstr` returns exit **1** with empty
    /// stdout for BOTH a clean no-match and "FINDSTR: Cannot open <path>", so
    /// the exit-code guard alone was a no-op there. A guard that cannot run
    /// where the defect lives is not a guard.
    #[tokio::test]
    async fn try_grep_reports_unreadable_target_not_no_matches() {
        let out = try_grep(
            "pattern",
            "this_path_does_not_exist_9f3a2b.txt",
            false,
            None,
        )
        .await;
        assert!(
            out.is_error,
            "an unreadable target must be is_error=true, got: {}",
            out.content
        );
        assert!(
            !out.content.contains("No matches found"),
            "a real error must not be reported as a clean no-match: {}",
            out.content
        );
    }

    /// #661: grepping a SINGLE FILE must return that file's matches.
    ///
    /// `findstr` has no recursive-directory form, so the Windows arm lowers a
    /// directory to a `<dir>\*` wildcard. Applying that lowering to a file
    /// produced `file.txt\*`, which matches nothing — exit 1, empty stdout,
    /// reported as a clean "No matches found" for a pattern the file plainly
    /// contains. Runs on every platform; on Windows it fails without the
    /// file/directory split.
    #[tokio::test]
    async fn try_grep_finds_pattern_when_target_is_a_single_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = "WAYLAND_GREP_SINGLE_FILE_MARKER_661";
        let file = dir.path().join("needle.txt");
        std::fs::write(&file, format!("first line\n{marker}\nlast line\n")).expect("write needle");
        // A decoy in a subdirectory: if `/S` leaked back in, a same-named file
        // below the target could be matched instead of the file we asked for.
        std::fs::create_dir_all(dir.path().join("sub")).expect("mkdir sub");

        let out = try_grep(marker, &file.to_string_lossy(), false, None).await;
        assert!(!out.is_error, "single-file grep failed: {}", out.content);
        assert!(
            out.content.contains(marker),
            "grepping a single file must return its matches, got: {}",
            out.content
        );
    }

    /// #661: a target that resolves to nothing must never become an unbounded
    /// scan. The Windows arm built its spec with
    /// `path.trim_end_matches(['\\', '/'])`, which maps both "" and "/" to the
    /// empty string — so the spec became `\*`, the root of the current drive,
    /// and `/S` walked the whole drive silently (measured: still running after
    /// 25s, vs 157ms for the same scan scoped to its intended directory).
    /// Resolving the target first makes the degenerate spec unreachable.
    #[tokio::test]
    async fn try_grep_rejects_an_empty_path_instead_of_scanning_from_the_root() {
        let out = try_grep("pattern", "", false, None).await;
        assert!(
            out.is_error,
            "an empty path must be a loud error, not a scan: {}",
            out.content
        );
    }

    /// Positive control for the two guards above: with the same helper and the
    /// same tempdir, a DIRECTORY target still finds the marker. Without this,
    /// a `try_grep` that errored on everything would satisfy both guards.
    #[tokio::test]
    async fn try_grep_finds_pattern_when_target_is_a_directory() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = "WAYLAND_GREP_DIR_MARKER_661";
        std::fs::create_dir_all(dir.path().join("sub")).expect("mkdir sub");
        std::fs::write(
            dir.path().join("sub").join("needle.txt"),
            format!("{marker}\n"),
        )
        .expect("write needle");

        let out = try_grep(marker, &dir.path().to_string_lossy(), false, None).await;
        assert!(!out.is_error, "directory grep failed: {}", out.content);
        assert!(
            out.content.contains(marker),
            "recursive directory grep must find the marker, got: {}",
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

    /// #661 mutation-kill. The `try_exists` pre-check in `run_grep` is the only
    /// thing that makes a missing target an error *identically on all three
    /// platforms*: `rg`/POSIX `grep` exit >=2, but `findstr` exits 1 with empty
    /// stderr, indistinguishable from a clean no-match.
    ///
    /// The lane's mutation run reported `grep-target-exists` as SURVIVED —
    /// deleting the pre-check failed no test, because the backend tests call
    /// `try_grep` directly and on Unix the backend still errors on its own. So
    /// this asserts the exact message ONLY the pre-check emits. Assert on
    /// `is_error` alone and the mutant survives again.
    #[tokio::test]
    async fn run_grep_pre_check_rejects_missing_target_on_every_platform() {
        let dir = tempfile::tempdir().expect("tempdir");
        let input = json!({
            "pattern": "anything",
            "path": "this_path_does_not_exist_4c81de.txt",
        });
        let out = run_grep(&input, Some(dir.path())).await;

        assert!(
            out.is_error,
            "a missing target must be an error: {}",
            out.content
        );
        assert!(
            out.content.contains("no such file or directory"),
            "the pre-check must own this refusal on every platform, not the \
             backend's exit code (which findstr cannot provide); got: {}",
            out.content
        );
    }
}
