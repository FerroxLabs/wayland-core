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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use wcore_config::shell::shell_command_argv;
use wcore_protocol::events::ToolCategory;
use wcore_types::tool::{JsonSchema, ToolResult};

use crate::Tool;
use crate::context::ToolContext;
use crate::unsaved_work::{Staging, staging_verdict, stash_refusal};
use crate::workspace_policy::WorkspacePolicy;

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

/// FerroxLabs/wayland-core#388 — **`git diff` reconstructs committed content
/// from the object store, and the object store is exactly what a `Contained`
/// workspace refuses to read.**
///
/// `SecretDenyFs` refuses `<root>/.env` and `<root>/.git/objects/...`, and the
/// read-deny sandbox shadows the store from `Bash`. `GitTool` spawns `git`
/// through `shell_command_argv`, OUTSIDE both layers, and `git diff HEAD~1`
/// hands back the deleted `.env`'s plaintext with no path argument required at
/// all. MEASURED on #388 before this: `leaked=true is_error=false`.
///
/// The filter is per-FILE section, not per-line: a diff is `git`'s own
/// stable machine format, one `diff --git` header per file, and the paths are
/// named in it. Every candidate spelling in a section is asked — `--- a/X`,
/// `+++ b/Y` and the header's own pair — so a rename that touches a denied
/// path on either side is withheld from both sides.
///
/// The hunks go; the header STAYS. "This file changed and you may not see how"
/// and "this file did not change" are different answers and a model acts on
/// them differently — the same rule `grep_policy`'s footer exists for.
struct WithheldDiff {
    body: String,
    files: BTreeSet<String>,
}

impl WithheldDiff {
    /// Append the footer, in the shape `grep_policy::Filtered::footer` uses.
    fn render(self) -> String {
        if self.files.is_empty() {
            return self.body;
        }
        let named: Vec<&str> = self.files.iter().map(String::as_str).collect();
        format!(
            "{}\n[Git] {} file(s)' hunks withheld ({})",
            self.body.trim_end(),
            self.files.len(),
            named.join(", ")
        )
    }
}

/// Split a unified diff into per-file sections and drop the body of every
/// section naming a path this policy refuses to hand back as content.
fn withhold_denied_hunks(diff: &str, cwd: &Path, policy: &WorkspacePolicy) -> WithheldDiff {
    let mut out: Vec<String> = Vec::new();
    let mut files: BTreeSet<String> = BTreeSet::new();
    // Sections are delimited by the header line, so the whole diff is walked
    // once and buffered one file at a time.
    let mut section: Vec<&str> = Vec::new();
    let flush = |section: &mut Vec<&str>, out: &mut Vec<String>, files: &mut BTreeSet<String>| {
        if section.is_empty() {
            return;
        }
        let denied = diff_section_paths(section)
            .into_iter()
            .find(|rel| policy.denies_read_content(&cwd.join(rel)));
        match denied {
            Some(rel) => {
                out.push(section[0].to_string());
                out.push(format!(
                    "[Git] hunks withheld: {rel} is denied for content reads in this workspace posture"
                ));
                files.insert(rel);
            }
            None => out.extend(section.iter().map(|line| (*line).to_string())),
        }
        section.clear();
    };
    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            flush(&mut section, &mut out, &mut files);
        }
        section.push(line);
    }
    flush(&mut section, &mut out, &mut files);
    WithheldDiff {
        body: out.join("\n"),
        files,
    }
}

/// Every workspace-relative path a diff section names.
///
/// All three spellings, because any one of them can be missing: a pure mode
/// change has no `---`/`+++` pair at all, and a create/delete puts `/dev/null`
/// on one side. Over-collecting is the safe direction — an extra candidate can
/// only add a refusal, and a missed one is a leak.
fn diff_section_paths(section: &[&str]) -> Vec<String> {
    let mut paths: Vec<String> = Vec::new();
    let mut push = |raw: &str| {
        let raw = raw.trim();
        if raw.is_empty() || raw == "/dev/null" {
            return;
        }
        // `git` quotes a path containing unusual bytes; the quotes are not part
        // of the name and a quoted name must still be matched.
        let raw = raw.trim_matches('"');
        let rel = raw
            .strip_prefix("a/")
            .or_else(|| raw.strip_prefix("b/"))
            .unwrap_or(raw);
        if !paths.iter().any(|seen| seen == rel) {
            paths.push(rel.to_string());
        }
    };
    for line in section {
        if let Some(rest) = line.strip_prefix("--- ") {
            push(rest);
        } else if let Some(rest) = line.strip_prefix("+++ ") {
            push(rest);
        }
    }
    // The header names both sides even when the `---`/`+++` pair is absent.
    // Split on " b/" rather than on whitespace: a path may contain spaces.
    if let Some(header) = section.first().and_then(|l| l.strip_prefix("diff --git "))
        && let Some((left, right)) = header.rsplit_once(" b/")
    {
        push(left);
        push(&format!("b/{right}"));
    }
    paths
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
        // FerroxLabs/wayland-core#388 — the content-emitting verbs, filtered
        // against THIS session's read-deny boundary.
        //
        // `denies_read_content` is the whole conjunction `SecretDenyFs::guard`
        // asks, so the posture boundary needs no second opinion here: a
        // `Contained` workspace requires the project-secret deny and withholds,
        // while a genuinely-local `Trusted` session does not require it, the
        // predicate answers false, and Sean's #667 carve-out is preserved
        // untouched. Pinned in both directions by
        // `crates/wcore-tools/tests/git_content_store_deny.rs`.
        let op = input
            .get("op")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        // The POSTURE gate, and it is `secret_read_deny_required` rather than
        // `denies_read_content` alone. That predicate is unconditional on
        // posture — `is_project_secret_resolved` matches `.env` under the root
        // for EVERY policy — because #667's carve-out is expressed by NOT
        // INSTALLING `SecretDenyFs` for a genuinely-local session, not by the
        // predicate answering differently. `GitTool` has no such installation
        // switch, so it asks the flag that decides one: the same
        // `secret_read_deny_required` #667 (F2) minted for exactly this
        // question — true for `contained` and for a Full/remote session, false
        // for `trusted_local`. Pinned in BOTH directions by
        // `tests/git_content_store_deny.rs::the_posture_decides_and_trusted_local_is_left_alone`;
        // a filter that withheld everywhere would overturn Sean's ruling
        // silently.
        let Some(policy) = ctx
            .workspace
            .clone()
            .filter(|p| p.secret_read_deny_required())
        else {
            return self.execute(input).await;
        };
        let repo = resolved_cwd(cwd);
        if op == "blame" {
            // `blame` prints the committed line itself. There is no hunk to
            // strip, so the refusal is taken BEFORE `git` runs.
            if let Some(path) = input.get("path").and_then(|v| v.as_str())
                && policy.denies_read_content(&repo.join(path))
            {
                return ToolResult {
                    content: format!(
                        "[Git] blame withheld: {path} is denied for content reads in this \
                         workspace posture"
                    ),
                    is_error: true,
                };
            }
            return self.execute(input).await;
        }
        let result = self.execute(input).await;
        if op != "diff" || result.is_error {
            return result;
        }
        ToolResult {
            content: withhold_denied_hunks(&result.content, &repo, &policy).render(),
            is_error: result.is_error,
        }
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
