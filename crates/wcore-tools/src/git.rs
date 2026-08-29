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
//! **The user's unsaved work is guarded here, not only in `Bash`.** The
//! INV-2 guard (`unsaved_work`) refuses a `Bash` command that throws the work
//! tree away, but this tool reaches the same `git` through a different door —
//! and under the STRICT sandbox it is the ONLY door, so the guarded route is
//! the one the model is told not to use. `add_all`, `add_paths`, `commit` and
//! `stash_save` therefore ask the same question before they run; see
//! `unsaved_work::git_ops` for exactly what refuses and what deliberately does
//! not.
//!
//! No auto-commit. The `commit` op requires an explicit `message` field;
//! the agent supplies one (potentially generated via
//! `git_commit_message::commit_message_from_trace` — see T13).
//!
//! **Review-ready by default.** Committing onto the repository's default
//! branch is refused unless the caller passes `allow_default_branch: true`.
//! Landing straight on someone's trunk leaves nothing to review and nothing
//! to revert cleanly, and it is the path of least resistance for a model that
//! is simply never asked to choose — so it now takes the same explicit intent
//! any other irreversible action does. `push` and `pr_create` complete the
//! route the refusal points at.
//!
//! `pr_create` drives the `gh` CLI from `PATH`, in the same argv mode as
//! `git`. It lives here rather than in `github_tool` because that tool speaks
//! the REST API through a host-provided HTTP backend, which an offline or
//! contained workspace does not have; `gh` is the surface a developer's
//! machine actually carries. Under the STRICT Bash sandbox, `git` and `gh`
//! cannot run from `Bash` at all (`<root>/.git/config` is on the secret
//! deny-list), so this tool is the ONLY route a contained session has to a
//! branch, a push or a pull request.

use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use wcore_config::shell::shell_command_argv;
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::context::ToolContext;
use crate::unsaved_work::{Staging, staging_verdict, stash_refusal};
use crate::workspace_policy::is_secret_path_static;

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
    Push {
        remote: Option<String>,
        branch: Option<String>,
    },
    PrCreate {
        title: String,
        body: Option<String>,
        base: Option<String>,
        branch: Option<String>,
    },
    StashSave,
    StashPop,
}

pub struct GitTool;

/// The directory this op runs in, resolved so the unsaved-work guard and
/// `git` are talking about the same tree.
///
/// `cwd` defaults to `"."`, and a relative path cannot be stripped against
/// the absolute repository root the guard resolves, so a bare `"."` would
/// leave every path un-attributable and the guard silent.
fn resolved_cwd(cwd: &str) -> PathBuf {
    std::fs::canonicalize(cwd).unwrap_or_else(|_| PathBuf::from(cwd))
}

/// Turn a guard refusal into the tool's error result.
fn refused(why: String) -> ToolResult {
    ToolResult {
        content: why,
        is_error: true,
    }
}

/// Append a guard note to a result that succeeded.
fn with_note(mut result: ToolResult, note: Option<String>) -> ToolResult {
    if let (Some(note), false) = (note, result.is_error) {
        result.content.push_str(&note);
    }
    result
}

/// Run `program` with arguments passed as separate argv entries and `cwd`
/// as the working directory. No shell wrapping, so the input strings are
/// safe regardless of shell-metacharacter content.
async fn run_program(program: &str, cwd: &str, args: &[&str]) -> ToolResult {
    let mut cmd = shell_command_argv(program, args);
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
            content: format!("Git: failed to spawn {program}: {e}"),
            is_error: true,
        },
    }
}

/// `git`, the common case.
async fn run_git(cwd: &str, args: &[&str]) -> ToolResult {
    run_program("git", cwd, args).await
}

/// Refs and remote names travel as their own argv entries, so a leading `-`
/// can never break out of the argument list — but git's OWN option parser
/// will still read `-f` as a flag and quietly do something the caller never
/// asked for. Reject the shape instead of guessing at the intent.
fn reject_option_shaped(kind: &str, value: &str) -> Option<ToolResult> {
    if value.is_empty() || value.starts_with('-') {
        return Some(ToolResult {
            content: format!(
                "Git: {kind} {value:?} is not usable — it must be non-empty and must not \
                 begin with '-'"
            ),
            is_error: true,
        });
    }
    None
}

/// D1 / core#244, GitTool half. **`diff` and `blame` return file CONTENT, so
/// they are read-path siblings of `Read` and `Grep` and owe the same secret
/// policy.**
///
/// MEASURED, not reasoned, in the contained posture with `SecretDenyFs`
/// installed: `.env` committed and then deleted from the working tree — so its
/// bytes live ONLY in the object store — and
/// `Git{op: "diff", rev: "HEAD~1", path: ".env"}` returned
/// `-AWS_SECRET_ACCESS_KEY=PROBE-GIT-9931`. That is precisely what core#244's
/// c3 says is unreachable, arriving through a subprocess this product spawns.
/// `SecretDenyFs` cannot see it: this tool gates only `cwd` through the vfs and
/// then runs `git` itself, and under the STRICT sandbox `Bash` cannot run `git`
/// at all, so this is the ONLY door — which makes it the door that has to hold.
///
/// The refusal is deliberately NOT a whole-op refusal. A `diff` over a commit
/// that happens to have touched `.env` is ordinary review work, and refusing it
/// outright would take the only git surface a contained session has away for a
/// reason the caller cannot act on. So the per-FILE sections a diff is already
/// made of are withheld individually and the withholding is REPORTED — the same
/// shape `grep_policy` uses, for the same reason: "could not show you" and
/// "there was nothing" are different answers.
///
/// Splitting on `diff --git ` is exact rather than heuristic: git emits that
/// header once per file, at the start of a line, and every byte of a file's
/// patch — mode lines, index line, hunks — follows it until the next one.
/// Anything before the first header (there is nothing, in `git diff` output) is
/// kept, so a format change cannot silently turn this into a pass-through of
/// the whole diff.
fn withhold_secret_diff_sections(content: &str) -> (String, Vec<String>) {
    const HEADER: &str = "diff --git ";
    let mut kept = String::new();
    let mut withheld: Vec<String> = Vec::new();
    let mut current: Option<(String, String)> = None;

    let flush =
        |current: &mut Option<(String, String)>, kept: &mut String, withheld: &mut Vec<String>| {
            if let Some((name, body)) = current.take() {
                if name.is_empty() {
                    kept.push_str(&body);
                } else {
                    withheld.push(name);
                }
            }
        };

    for line in content.split_inclusive('\n') {
        if let Some(rest) = line.strip_prefix(HEADER) {
            flush(&mut current, &mut kept, &mut withheld);
            let name = secret_diff_target(rest);
            current = Some((name.unwrap_or_default(), line.to_string()));
            continue;
        }
        match current.as_mut() {
            Some((_, body)) => body.push_str(line),
            None => kept.push_str(line),
        }
    }
    flush(&mut current, &mut kept, &mut withheld);
    (kept, withheld)
}

/// The basename of the file a `diff --git a/X b/X` header names, when that file
/// is secret-shaped; `None` otherwise.
///
/// Reads the `b/` side (the post-image), falling back to the `a/` side for a
/// deletion, so a rename INTO or OUT OF a secret name is caught from either
/// end. A path containing a space makes the header ambiguous by git's own
/// format; the whole remainder is then tested, which errs toward withholding.
fn secret_diff_target(rest: &str) -> Option<String> {
    let rest = rest.trim_end_matches(['\n', '\r']);
    let mut candidates: Vec<&str> = Vec::new();
    if let Some((a, b)) = rest.split_once(" b/") {
        candidates.push(b);
        candidates.push(a.strip_prefix("a/").unwrap_or(a));
    } else {
        candidates.push(rest);
    }
    // Anchored under a root before the test. `is_secret_path_static` spells its
    // dotfile rules as PATH suffixes (`/.env`, `/.npmrc`), so a bare relative
    // `.env` — which is exactly what a diff header carries — misses every one
    // of them. MEASURED, not assumed: `is_secret_path_static(Path::new(".env"))`
    // is FALSE while `Path::new("a/.env")` is TRUE. Every other caller happens
    // to hand it an already-resolved absolute path, so the edge had nowhere to
    // show; anchoring here keeps the answer the same as `Read`'s and `Grep`'s
    // rather than introducing a second, weaker spelling of the same list.
    candidates
        .into_iter()
        .find(|c| is_secret_path_static(&std::path::Path::new("/").join(c)))
        .map(|c| {
            std::path::Path::new(c)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| c.to_string())
        })
}

/// Apply [`withhold_secret_diff_sections`] to a successful result, appending a
/// deterministic footer naming what was withheld.
fn apply_diff_secret_policy(mut result: ToolResult) -> ToolResult {
    if result.is_error {
        return result;
    }
    let (kept, withheld) = withhold_secret_diff_sections(&result.content);
    if withheld.is_empty() {
        return result;
    }
    let mut names: Vec<String> = withheld;
    names.sort();
    names.dedup();
    result.content = format!(
        "{kept}[Git policy: {} secret-shaped file section(s) withheld ({})]",
        names.len(),
        names.join(", ")
    );
    result
}

/// A content-returning op may not be pointed AT a secret. Separate from the
/// section filter because `blame` has no per-file sections to withhold — the
/// whole answer is one file's content — and because naming the file outright
/// deserves to be told, not silently emptied.
fn refuse_secret_path(cwd: &str, path: &str) -> Option<ToolResult> {
    if path.is_empty() {
        return None;
    }
    let target = resolved_cwd(cwd).join(path);
    if !is_secret_path_static(&target) {
        return None;
    }
    Some(ToolResult {
        content: format!(
            "Refused: {path:?} is a credential-bearing file, and this op returns its \
             content"
        ),
        is_error: true,
    })
}

/// The branch currently checked out, or `None` on a detached / unborn HEAD.
async fn current_branch(cwd: &str) -> Option<String> {
    let result = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).await;
    if result.is_error {
        return None;
    }
    let name = result.content.trim().to_string();
    if name.is_empty() || name == "HEAD" {
        None
    } else {
        Some(name)
    }
}

/// Conventional trunk names, used only when the remote has published no
/// `origin/HEAD` for this clone to read.
const CONVENTIONAL_DEFAULT_BRANCHES: &[&str] = &["main", "master", "trunk", "develop"];

/// The trunk the REMOTE published, via `refs/remotes/origin/HEAD`. `None`
/// when the remote published none — a shallow clone, or an origin added by
/// hand, has no such ref.
async fn published_default_branch(cwd: &str) -> Option<String> {
    let head = run_git(
        cwd,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    )
    .await;
    if head.is_error {
        return None;
    }
    head.content
        .trim()
        .strip_prefix("origin/")
        .filter(|name| !name.is_empty())
        .map(str::to_string)
}

/// The branch a pull request should target when the caller named no base.
async fn default_branch_name(cwd: &str) -> String {
    published_default_branch(cwd)
        .await
        .unwrap_or_else(|| "main".to_string())
}

/// Whether `branch` is the branch this repository treats as its trunk. With
/// nothing published, fall back to the conventional names rather than fail
/// open onto someone's trunk.
async fn is_default_branch(cwd: &str, branch: &str) -> bool {
    match published_default_branch(cwd).await {
        Some(published) => published == branch,
        None => CONVENTIONAL_DEFAULT_BRANCHES.contains(&branch),
    }
}

#[async_trait]
impl Tool for GitTool {
    fn name(&self) -> &str {
        "Git"
    }

    fn description(&self) -> &str {
        "Read or mutate git state in the current repo, and open a pull request for it. Pass an \
         `op` field naming the operation (status | diff | log | blame | add_all | add_paths | \
         commit | branch_current | branch_list | branch_checkout | push | pr_create | stash_save | \
         stash_pop). Read-only ops are safe to run in parallel. `diff` takes an optional `rev` \
         naming a revision or a range — `rev: \"main\"`, `rev: \"main...HEAD\"` (what a pull \
         request changed), `rev: \"<sha>\"` — and an optional `path` that is ALWAYS a pathspec, \
         never a revision; with neither it diffs the working tree. Commit requires a non-empty \
         `message`. Work that is meant to be reviewed goes on its own branch: \
         `branch_checkout` with `create: true` (staged changes come with you), then `commit`, then \
         `push`, then `pr_create` — committing onto the repository's default branch is REFUSED \
         unless you also pass `allow_default_branch: true`. `push` takes optional `remote` \
         (default `origin`) and `branch` (default: current). `pr_create` drives the `gh` CLI and \
         takes `title` (required), `body`, `base` and `branch`; push the branch first. Optional \
         `cwd` overrides the working directory."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "op": { "type": "string" },
                "path": { "type": "string" },
                "rev": { "type": "string" },
                "staged": { "type": "boolean" },
                "limit": { "type": "integer" },
                "line": { "type": "integer" },
                "paths": { "type": "array", "items": { "type": "string" } },
                "message": { "type": "string" },
                "name": { "type": "string" },
                "create": { "type": "boolean" },
                "allow_default_branch": { "type": "boolean" },
                "remote": { "type": "string" },
                "branch": { "type": "string" },
                "title": { "type": "string" },
                "body": { "type": "string" },
                "base": { "type": "string" },
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
                let rev = input.get("rev").and_then(|v| v.as_str()).unwrap_or("");
                let staged = input
                    .get("staged")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !rev.is_empty()
                    && let Some(err) = reject_option_shaped("revision", rev)
                {
                    return err;
                }
                // Build argv: `git diff [--staged] [<rev>] [-- <path>]`. Each
                // element is a separate process arg; the `--` sentinel
                // makes `git` treat any subsequent values as paths even
                // if they begin with `-`.
                //
                // `rev` is the difference between reviewing a branch and
                // guessing at it. Without it this op could only ever diff the
                // WORKING TREE, and a revision handed to `path` landed after
                // the `--` as a pathspec — `git diff -- main` matches no file
                // and exits 0 with empty output, so the caller was told the
                // branch changed nothing. Under the STRICT sandbox this tool
                // is the only git surface there is (see the module docs), so
                // that silent empty diff left a "review this PR" job with no
                // way to see the pull request at all.
                if let Some(err) = refuse_secret_path(cwd, path) {
                    return err;
                }
                let mut args: Vec<&str> = vec!["diff"];
                if staged {
                    args.push("--staged");
                }
                if !rev.is_empty() {
                    args.push(rev);
                }
                if !path.is_empty() {
                    args.push("--");
                    args.push(path);
                }
                apply_diff_secret_policy(run_git(cwd, &args).await)
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
                if let Some(err) = refuse_secret_path(cwd, path) {
                    return err;
                }
                let line = input.get("line").and_then(|v| v.as_u64()).unwrap_or(1);
                let range = format!("{line},{line}");
                // `git blame -L <range> -- <path>` — argv mode, no shell.
                run_git(cwd, &["blame", "-L", &range, "--", path]).await
            }
            "add_all" => {
                // `add -A` stages files nobody named, which is how the user's
                // untracked scratch file ends up in a commit.
                let note = match staging_verdict(&resolved_cwd(cwd), Staging::Everything) {
                    Ok(note) => note,
                    Err(why) => return refused(why),
                };
                with_note(run_git(cwd, &["add", "-A"]).await, note)
            }
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
                let note = match staging_verdict(&resolved_cwd(cwd), Staging::Named(&paths)) {
                    Ok(note) => note,
                    Err(why) => return refused(why),
                };
                // `git add -- <p1> <p2> ...` — `--` sentinel guards
                // against paths beginning with `-`.
                let mut args: Vec<&str> = vec!["add", "--"];
                for p in &paths {
                    args.push(p.as_str());
                }
                with_note(run_git(cwd, &args).await, note)
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
                // Landing on the trunk is irreversible in the way that
                // matters to a reviewer: there is no branch to open, and no
                // clean revert. Require the same explicit intent as any other
                // irreversible action, and say exactly how to supply it.
                let allow_default = input
                    .get("allow_default_branch")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                if !allow_default
                    && let Some(branch) = current_branch(cwd).await
                    && is_default_branch(cwd, &branch).await
                {
                    return ToolResult {
                        content: format!(
                            "Git: refusing to commit onto {branch:?} — this repository's \
                             default branch. Work that is meant to be reviewed belongs on its \
                             own branch: there is nothing to open a pull request against and \
                             nothing to revert cleanly once it is on the trunk. Run \
                             {{\"op\":\"branch_checkout\",\"name\":\"<branch>\",\"create\":true}} \
                             first — anything already staged comes with you — then commit, then \
                             {{\"op\":\"push\"}} and {{\"op\":\"pr_create\",\"title\":\"…\"}}. \
                             If this commit really is meant to land on {branch:?}, re-send the \
                             same call with \"allow_default_branch\": true."
                        ),
                        is_error: true,
                    };
                }
                // Asked again on the index, not only at `add` time: the
                // index can be staged from `Bash`, or by an earlier op whose
                // note the model ignored, and the commit is the irreversible
                // step.
                let note = match staging_verdict(&resolved_cwd(cwd), Staging::Index) {
                    Ok(note) => note,
                    Err(why) => return refused(why),
                };
                // Message is a single argv entry — no quoting / escaping
                // needed; shell metacharacters in the message body are
                // never interpreted.
                with_note(run_git(cwd, &["commit", "-m", message]).await, note)
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
                if let Some(err) = reject_option_shaped("branch name", name) {
                    return err;
                }
                // `git checkout [-b] <name> --`. The `--` goes AFTER the ref:
                // it ends the ref and opens an (empty) pathspec list, so a
                // branch that shares a name with a file cannot be read as a
                // path checkout. Putting it BEFORE the ref — as this did
                // until the A-2 corpus row caught it — made `-b` swallow the
                // `--` as the new branch name (`fatal: … a branch '--' cannot
                // be created from it`) and made the non-create form a
                // `git checkout -- <pathspec>` file restore that never
                // switched branch at all. Both forms were unusable.
                let mut args: Vec<&str> = vec!["checkout"];
                if create {
                    args.push("-b");
                }
                args.push(name);
                args.push("--");
                run_git(cwd, &args).await
            }
            "push" => {
                let remote = input
                    .get("remote")
                    .and_then(|v| v.as_str())
                    .unwrap_or("origin");
                if let Some(err) = reject_option_shaped("remote name", remote) {
                    return err;
                }
                let branch = match input.get("branch").and_then(|v| v.as_str()) {
                    Some(b) => b.to_string(),
                    None => match current_branch(cwd).await {
                        Some(b) => b,
                        None => {
                            return ToolResult {
                                content: "Git::Push: HEAD is detached or unborn, so there is \
                                          no branch to push — pass 'branch'"
                                    .to_string(),
                                is_error: true,
                            };
                        }
                    },
                };
                if let Some(err) = reject_option_shaped("branch name", &branch) {
                    return err;
                }
                run_git(cwd, &["push", "--set-upstream", remote, &branch]).await
            }
            "pr_create" => {
                let title = match input.get("title").and_then(|v| v.as_str()) {
                    Some(t) if !t.is_empty() => t,
                    _ => {
                        return ToolResult {
                            content: "Git::PrCreate requires non-empty 'title'".to_string(),
                            is_error: true,
                        };
                    }
                };
                let body = input.get("body").and_then(|v| v.as_str()).unwrap_or("");
                let head = match input.get("branch").and_then(|v| v.as_str()) {
                    Some(b) => b.to_string(),
                    None => match current_branch(cwd).await {
                        Some(b) => b,
                        None => {
                            return ToolResult {
                                content: "Git::PrCreate: HEAD is detached or unborn, so there \
                                          is no branch to open a pull request for — pass \
                                          'branch'"
                                    .to_string(),
                                is_error: true,
                            };
                        }
                    },
                };
                let base = match input.get("base").and_then(|v| v.as_str()) {
                    Some(b) => b.to_string(),
                    None => default_branch_name(cwd).await,
                };
                for (kind, value) in [("branch name", &head), ("base branch name", &base)] {
                    if let Some(err) = reject_option_shaped(kind, value) {
                        return err;
                    }
                }
                if head == base {
                    return ToolResult {
                        content: format!(
                            "Git::PrCreate: head and base are both {head:?} — create a branch \
                             for the work first"
                        ),
                        is_error: true,
                    };
                }
                run_program(
                    "gh",
                    cwd,
                    &[
                        "pr", "create", "--head", &head, "--base", &base, "--title", title,
                        "--body", body,
                    ],
                )
                .await
            }
            "stash_save" => {
                if let Some(why) = stash_refusal(&resolved_cwd(cwd)) {
                    return refused(why);
                }
                run_git(cwd, &["stash", "push", "-m", "wcore-stash"]).await
            }
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

    fn execution_class_for(&self, _input: &Value) -> crate::ToolExecutionClass {
        crate::ToolExecutionClass::ProcessSpawning
    }

    fn describe(&self, input: &Value) -> String {
        let op = input
            .get("op")
            .and_then(|v| v.as_str())
            .unwrap_or("(missing op)");
        format!("Git::{op}")
    }
}
