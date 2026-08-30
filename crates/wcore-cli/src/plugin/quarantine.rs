// Lane C2: security-critical quarantine git clone.
//
// Foreign plugin sources are git-cloned into an isolated quarantine dir with
// hooks disabled and the `ext` transport blocked, then NORMALIZE-COPIED into a
// clean tree: symlinks are skipped (an escaping symlink must never reach the
// store), `.git` is dropped, and a cumulative size cap bounds the copy. Every
// `git` invocation uses a synchronous `std::process::Command` in argv mode —
// the URL, ref, and sha reach `git` as literal argv entries, never interpolated
// into a shell string (no shell is involved at all). We also reject flag-like
// (`-`-leading) values so a crafted ref can't smuggle a `git` option past the
// argv boundary, and reject absolute/`..` subdir paths so a git-subdir source
// can't escape the clone. See `run_git` for why the async shell helper is unused.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use wcore_pluginsrc::SourceKind;

use crate::plugin::error::{PluginCliError, Result};
use crate::plugin::marketplace::reject_traversal;

const DEFAULT_GIT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_MAX_BYTES: u64 = 100_000_000;

/// How long to wait for a drain thread once `git` itself has exited.
///
/// A pipe reaches EOF only when EVERY write end is closed. `git` spawns
/// helpers (credential, askpass, transport) with its own stdout/stderr
/// INHERITED and does not detach them, so a helper that leaves a background
/// worker running holds the pipe open after `git` has gone. Measured on git
/// 2.43 with a credential helper on the real clone argv: `git` exited after
/// 301 ms with the wall-clock guard above (120 s) untouched, and the stderr
/// drain then blocked forever -- a guard that reports nothing while the
/// install is wedged.
///
/// `git`'s OWN background processes are safe and do not need this: measured,
/// `git-credential-cache--daemon` runs with fd 0 and fd 2 on `/dev/null` and
/// fd 1 closed. This bounds third-party helpers, which git does not detach.
///
/// Five seconds is ~4 orders of magnitude above a healthy drain, which
/// completes as soon as the child exits.
const DRAIN_GRACE: Duration = Duration::from_secs(5);

/// A cloned + normalized source ready to lower. `path` contains only
/// allowlisted regular files and directories; `resolved_sha` pins the exact
/// commit fetched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClonedSource {
    pub path: PathBuf,
    pub resolved_sha: String,
}

/// Quarantine-clone a git source into `dest` and return the normalized copy.
/// Relative-path and npm sources are not cloned here (the former is resolved
/// within the already-fetched marketplace repo; the latter is deferred to v1.1).
pub fn quarantine_clone(source: &SourceKind, dest: &Path) -> Result<ClonedSource> {
    let (url, git_ref, sha, subdir) = match source {
        SourceKind::Github { repo, git_ref, sha } => {
            (github_url(repo), git_ref.clone(), sha.clone(), None)
        }
        SourceKind::Url { url, git_ref, sha } => (url.clone(), git_ref.clone(), sha.clone(), None),
        SourceKind::GitSubdir {
            url,
            path,
            git_ref,
            sha,
        } => {
            reject_traversal(path)?;
            (
                url.clone(),
                git_ref.clone(),
                sha.clone(),
                Some(path.clone()),
            )
        }
        SourceKind::RelativePath(_) => {
            return Err(PluginCliError::Quarantine(
                "relative-path source is resolved within the marketplace repo, not cloned".into(),
            ));
        }
        SourceKind::Npm { .. } => {
            return Err(PluginCliError::Quarantine(
                "npm sources are deferred to v1.1 (needs a Node toolchain)".into(),
            ));
        }
    };

    reject_flaglike(&url)?;
    if let Some(r) = &git_ref {
        reject_flaglike(r)?;
    }
    if let Some(s) = &sha {
        reject_flaglike(s)?;
    }

    std::fs::create_dir_all(dest)?;
    let clone_dir = dest.join("clone");
    if clone_dir.exists() {
        std::fs::remove_dir_all(&clone_dir)?;
    }
    let clone_str = clone_dir
        .to_str()
        .ok_or_else(|| PluginCliError::Quarantine("non-UTF8 clone path".into()))?;

    let timeout = Duration::from_millis(env_u64(
        "WAYLAND_PLUGIN_GIT_TIMEOUT_MS",
        DEFAULT_GIT_TIMEOUT_MS,
    ));

    // Shallow clone with hooks + ext transport disabled. `--` ends option
    // parsing before the URL/dest positionals.
    run_git(
        &[
            "-c",
            "core.hooksPath=/dev/null",
            "-c",
            "protocol.ext.allow=never",
            "clone",
            "--depth",
            "1",
            "--no-tags",
            "--",
            url.as_str(),
            clone_str,
        ],
        None,
        timeout,
    )?;

    // A pinned sha or named ref: fetch it shallowly, then detach onto it.
    if let Some(sha) = &sha {
        run_git(
            &["fetch", "--depth", "1", "origin", sha.as_str()],
            Some(&clone_dir),
            timeout,
        )?;
        run_git(
            &[
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "--detach",
                "FETCH_HEAD",
            ],
            Some(&clone_dir),
            timeout,
        )?;
    } else if let Some(r) = &git_ref {
        run_git(
            &["fetch", "--depth", "1", "origin", r.as_str()],
            Some(&clone_dir),
            timeout,
        )?;
        run_git(
            &[
                "-c",
                "advice.detachedHead=false",
                "checkout",
                "--detach",
                "FETCH_HEAD",
            ],
            Some(&clone_dir),
            timeout,
        )?;
    }

    let resolved_sha = run_git(&["rev-parse", "HEAD"], Some(&clone_dir), timeout)?
        .trim()
        .to_string();
    if resolved_sha.is_empty() {
        return Err(PluginCliError::Git("empty HEAD sha after clone".into()));
    }

    let src_root = match &subdir {
        Some(s) => clone_dir.join(s),
        None => clone_dir.clone(),
    };
    if !src_root.is_dir() {
        return Err(PluginCliError::Quarantine(format!(
            "subdir not found in repo: {}",
            subdir.unwrap_or_default()
        )));
    }
    // Defense in depth: even though `reject_traversal` rejected `..` and
    // absolute paths in the subdir string, a symlinked intermediate directory
    // inside the repo could still resolve `src_root` outside the clone. Confirm
    // containment after canonicalization before we copy anything out of it.
    let clone_canon = clone_dir
        .canonicalize()
        .map_err(|e| PluginCliError::Quarantine(format!("clone resolve: {e}")))?;
    let src_canon = src_root
        .canonicalize()
        .map_err(|e| PluginCliError::Quarantine(format!("subdir resolve: {e}")))?;
    if !src_canon.starts_with(&clone_canon) {
        return Err(PluginCliError::PathTraversal(
            src_root.display().to_string(),
        ));
    }

    let out = dest.join("plugin");
    if out.exists() {
        std::fs::remove_dir_all(&out)?;
    }
    let cap = env_u64("WAYLAND_PLUGIN_MAX_BYTES", DEFAULT_MAX_BYTES);
    let mut copied: u64 = 0;
    normalize_copy(&src_root, &out, &mut copied, cap)?;

    Ok(ClonedSource {
        path: out,
        resolved_sha,
    })
}

/// A human-readable source descriptor recorded in the lockfile.
pub fn describe_source(s: &SourceKind) -> String {
    match s {
        SourceKind::RelativePath(p) => format!("path:{}", p.display()),
        SourceKind::Github { repo, .. } => format!("github:{repo}"),
        SourceKind::Url { url, .. } => format!("url:{url}"),
        SourceKind::GitSubdir { url, path, .. } => format!("git-subdir:{url}#{path}"),
        SourceKind::Npm { package, .. } => format!("npm:{package}"),
    }
}

fn github_url(repo: &str) -> String {
    format!("https://github.com/{repo}.git")
}

/// Reject a value that would be parsed as a `git` option. argv mode stops the
/// shell, not `git`'s own option parser — a `--upload-pack=...` ref is still an
/// option unless we refuse leading-`-` positionals.
fn reject_flaglike(s: &str) -> Result<()> {
    if s.starts_with('-') {
        return Err(PluginCliError::Quarantine(format!(
            "refusing flag-like git argument: {s}"
        )));
    }
    Ok(())
}

/// Copy `src` into `dst`, skipping symlinks and `.git`, enforcing a cumulative
/// byte cap. Skipping ALL symlinks is the conservative v1 posture: an escaping
/// symlink must never materialize in the store, and within-dir symlinks are
/// rare in content plugins.
fn normalize_copy(src: &Path, dst: &Path, copied: &mut u64, cap: u64) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if name == ".git" {
            continue;
        }
        let ft = entry.file_type()?;
        let from = entry.path();
        let to = dst.join(&name);

        if ft.is_symlink() {
            tracing::warn!(path = %from.display(), "quarantine: skipping symlink");
            continue;
        }
        if ft.is_dir() {
            normalize_copy(&from, &to, copied, cap)?;
        } else if ft.is_file() {
            let len = entry.metadata()?.len();
            *copied = copied.saturating_add(len);
            if *copied > cap {
                return Err(PluginCliError::Quarantine(format!(
                    "plugin exceeds size cap of {cap} bytes"
                )));
            }
            std::fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// Run `git` in argv mode with a wall-clock timeout. stdout/stderr are drained
/// on dedicated threads so a chatty `git` can never deadlock on a full pipe.
///
/// `git` is invoked via a synchronous `std::process::Command` (not the async
/// `wcore_config::shell::shell_command_argv` helper, which returns a tokio
/// `Command`) because the whole plugin install path is blocking. The security
/// property is identical: each arg is a separate argv entry, so no shell
/// interprets `;`/`&&`/`$()` — combined with `--`, `protocol.ext.allow=never`,
/// and the leading-`-` reject above. Mirrors the sync git calls in
/// `tui/commands/at_ref_send.rs` and `wcore-skills/src/discovery.rs`.
/// Deny an untrusted quarantine child every route to the user's terminal.
///
/// The clone URL on this path is CATALOG-controlled and the install fires from
/// inside the TUI alt screen, so a prompt that reaches the terminal is both
/// attacker-triggered and unattributable. Two distinct routes exist, and only
/// one of them is the stdio we hand the child.
///
/// **Route 1 — the child asks its parent's plumbing to prompt.** Pinned by
/// environment, matching the three git-spawning sites that already do this
/// (`wcore-tools/src/unsaved_work.rs`, `wcore-swarm/src/worktree_cleanup.rs`,
/// `wcore-eval-scenarios/src/child_env.rs`):
///
/// * `GIT_TERMINAL_PROMPT=0` — git's own prompt.
/// * `GIT_ASKPASS=""` — git's `prompt.c` takes the first NON-EMPTY of
///   `GIT_ASKPASS`, `core.askpass`, `SSH_ASKPASS`, so an empty value is git's
///   own spelling of "no askpass", not a broken program path. Setting it here
///   also shadows a `core.askpass` in the user's global config, which an
///   inherited-env-only fix would leave live.
/// * `SSH_ASKPASS=""` + `SSH_ASKPASS_REQUIRE=never` — the OpenSSH equivalent,
///   for an `ssh://` or `git@` source.
/// * `GCM_INTERACTIVE=Never` — Git Credential Manager's GUI dialog.
/// * `GIT_PAGER=cat` — a pager takes the terminal too.
///
/// **Route 2 — `open("/dev/tty")`.** This needs none of our stdio and reads no
/// environment, so route 1 does not touch it: a third-party credential helper
/// that opens `/dev/tty` itself prompts regardless of every variable above.
/// That is the route issue #338 is actually about. A process can only open
/// `/dev/tty` if it has a CONTROLLING terminal, so the fix is to take the
/// controlling terminal away rather than to ask the child not to use it:
/// `setsid(2)` between fork and exec puts the child in a fresh session with no
/// ctty, `open("/dev/tty")` then fails with `ENXIO`, and every descendant the
/// child spawns — helpers included — inherits that session.
///
/// **Windows is NOT the same primitive, and this doc used to say it was.**
/// `DETACHED_PROCESS` withholds the parent's console AT CREATION; it does not
/// make that console unreachable afterwards. MEASURED on Windows 11 build
/// 10.0.26200.9168 by `crates/wcore-cli/tests/quarantine_console_authority_
/// windows.rs`, which is the reason that file exists:
///
/// ```text
/// [plain]    SHARES_USER_CONSOLE_BEFORE=true   CONOUT_BEFORE=OPEN
/// [hardened] SHARES_USER_CONSOLE_BEFORE=false  CONOUT_BEFORE=DENIED(6)
/// [hardened] ATTACH_PARENT_PROCESS=SUCCEEDED   SHARES_USER_CONSOLE_AFTER=true
/// [hardened] ATTACH_BY_EXPLICIT_PID=SUCCEEDED  CONOUT_AFTER_EXPLICIT=OPEN
/// ```
///
/// One documented call — `AttachConsole(ATTACH_PARENT_PROCESS)` — puts a
/// `DETACHED_PROCESS` child back on the USER'S OWN console, and attaching by
/// EXPLICIT pid works too, so reparenting the child onto a console-less
/// process is not a remedy either. `FreeConsole` then `AttachConsole` also
/// defeats giving the child a console of its own (`CREATE_NO_WINDOW`).
/// A `setsid`'d unix child has no such move: `TIOCSCTTY` refuses a terminal
/// that is already another session's controlling terminal.
///
/// So on Windows this is a REDUCTION (the child is not created on the user's
/// console, and never has it by default), not the elimination unix gets. #338
/// c2's own `text:` field opens `ON UNIX:` and names the Windows
/// non-delivery — `.planning/ledger/wayland-core-338.md`, one `grep` away, so
/// this sentence is checkable rather than asserted. It did not, when this
/// comment first claimed it did: the field was byte-identical to base and only
/// the criterion's STATE had moved, which is the same overstatement in a
/// different file. The Windows remainder is tracked as
/// FerroxLabs/wayland-core#389. Do not restore the analogy sentence: an
/// overstated security guarantee is worse than an understated one, because it
/// stops the next person looking.
///
/// `credential.helper` is deliberately NOT cleared. Clearing it would break
/// installs from private plugin sources, which is a real product cost, and
/// route 2 already removes the helper's terminal.
///
/// Fail-closed: if `setsid` fails the spawn fails, and the install reports an
/// error rather than proceeding with a child that still holds the terminal.
///
/// `pub` because the `/dev/tty` property is only observable from a process that
/// HAS a controlling terminal, which a test must build with a PTY — see
/// `crates/wcore-cli/tests/quarantine_terminal_authority.rs`.
pub fn harden_against_credential_prompt(cmd: &mut std::process::Command) {
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        .env("SSH_ASKPASS_REQUIRE", "never")
        .env("GCM_INTERACTIVE", "Never")
        .env("GIT_PAGER", "cat");

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // SAFETY: the hook runs between fork and exec in the child. `setsid(2)`
        // is async-signal-safe, allocates nothing, and touches no state shared
        // with the parent; it is the only thing this closure does.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        /// `DETACHED_PROCESS` — the child is CREATED with no console and
        /// does not inherit ours. Creation-time only; see the measured
        /// residual in this function's doc comment and
        /// `tests/quarantine_console_authority_windows.rs`.
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        cmd.creation_flags(DETACHED_PROCESS);
    }
}

/// Build the `git` command `run_git` runs, hardened, without spawning it.
///
/// Split out so a test grades the WIRING and not just the function: an
/// assertion against `harden_against_credential_prompt` alone still passes
/// when `run_git` stops calling it, which is the failure mode that has reached
/// a PR here before. Every quarantine `git` spawn goes through this.
pub fn build_git_command(args: &[&str], cwd: Option<&Path>) -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    cmd.args(args);
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    harden_against_credential_prompt(&mut cmd);
    cmd
}

/// Tear down the detached session `harden_against_credential_prompt` created.
///
/// # Why a group signal, and not `Child::kill`
///
/// The hardening calls `setsid(2)` in the child, so the child is a session
/// leader AND a process-group leader whose pgid equals its pid, and every
/// helper `git` spawns — credential, askpass, transport — inherits that group.
/// `Child::kill` signals ONE pid. On the abort paths that reaped the leaf and
/// left the helpers running in a session this process no longer owns: no group
/// signal reached them, and having no controlling terminal, no hangup ever
/// would either. That is the leak `FerroxLabs/wayland-core#379` reports, and it
/// is a consequence of the hardening rather than of `git` — before `setsid` the
/// helpers shared our group and our terminal, so they were strictly MORE
/// reachable. The teardown therefore belongs beside the hardening, which is why
/// it lives in this file and next to it.
///
/// # Why addressing the group by the child's pid stays safe after the reap
///
/// A pid is not recycled while it is still in use as a process-group id: POSIX
/// forbids reusing a pgid while the group has members, and Linux keeps the
/// `struct pid` alive for its `PIDTYPE_PGID` holders. So `kill(-pgid, …)` after
/// the leader has been waited on either reaches the surviving members of OUR
/// group or fails with `ESRCH`; it cannot land on an unrelated process that
/// happened to inherit the number. That property is what lets the drain-grace
/// exit below call this at all — there, `git` itself has already been reaped.
///
/// # What this does NOT reach, stated rather than implied
///
/// * A descendant that calls `setsid`/`setpgid` for itself leaves the group and
///   is out of reach of any group signal. Hard containment is the sandbox's
///   job (a PID namespace or a Job Object), never a process group's.
/// * On Windows the hardening creates no session and no group — `DETACHED_PROCESS`
///   is a creation-time console decision — so there is nothing here for a group
///   signal to address and descendants remain reachable only through a Job
///   Object. Windows never regressed the way unix did (it had no group teardown
///   to lose), but it has no teardown either, and composing a Job Object with
///   the console flag is not free -- `WindowsJobObject::create_suspended` SETS
///   `creation_flags` rather than OR-ing them, so a naive composition drops
///   `DETACHED_PROCESS` and reopens #338. Tracked as its own remainder with
///   that trap recorded: `FerroxLabs/wayland-core#393`.
///
/// The cost is deliberate and bounded to the FAILING exits. `git`'s own
/// `git-credential-cache--daemon` is in this group when `git` started one, and
/// on those exits it dies with the rest. That is why the successful exit does
/// not call this: killing a shared credential daemon after an install that
/// WORKED would be a product regression, and a drained pipe is evidence no
/// descendant is holding our stdio open.
#[cfg(unix)]
fn terminate_hardened_tree(child_pid: u32) {
    let Ok(pgid) = libc::pid_t::try_from(child_pid) else {
        return;
    };
    // `kill(0, …)` addresses the CALLER'S group and `kill(-1, …)` every process
    // we may signal. Neither is this tree, and both are catastrophic here.
    if pgid <= 1 {
        return;
    }
    // SAFETY: a negative target addresses the process GROUP `pgid`, which this
    // process created by spawning a `setsid` child and has not handed to anyone
    // else. `kill` touches no memory in this address space.
    unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    }
}

/// No-op counterpart: see the unix doc comment for why Windows has no session
/// or process group for a group signal to address.
#[cfg(not(unix))]
fn terminate_hardened_tree(_child_pid: u32) {}

fn run_git(args: &[&str], cwd: Option<&Path>, timeout: Duration) -> Result<String> {
    run_hardened(
        build_git_command(args, cwd),
        &format!("git {args:?}"),
        timeout,
    )
}

/// Run an already-hardened command to completion, with the wall-clock guard,
/// the bounded drains, and the session teardown on every failing exit.
///
/// Split out of `run_git` so a test can drive the REAL control flow — the
/// timeout branch and the drain-grace branch both tear the tree down here —
/// without needing a `git` on `PATH` that can be made to hang on demand.
/// `label` is what the caller calls this process in an error message, and is
/// `git ["clone", …]` for every production call.
fn run_hardened(mut cmd: std::process::Command, label: &str, timeout: Duration) -> Result<String> {
    let mut child = cmd
        .spawn()
        .map_err(|e| PluginCliError::Git(format!("spawn git: {e}")))?;
    // Captured before anything can reap the child: after the wait its `id()` is
    // still readable, but taking it here keeps the teardown's input independent
    // of the `Child`'s state.
    let child_pid = child.id();

    let mut out_pipe = child.stdout.take().expect("stdout piped");
    let mut err_pipe = child.stderr.take().expect("stderr piped");
    let h_out = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out_pipe.read_to_end(&mut b);
        b
    });
    let h_err = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err_pipe.read_to_end(&mut b);
        b
    });

    let start = Instant::now();
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| PluginCliError::Git(format!("wait git: {e}")))?
        {
            Some(s) => break s,
            None => {
                if start.elapsed() > timeout {
                    // The group FIRST, while the leader is still unreaped, then
                    // the leaf and its corpse. Either order is safe (see
                    // `terminate_hardened_tree`), this one needs no argument.
                    terminate_hardened_tree(child_pid);
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(PluginCliError::Git(format!(
                        "{label} timed out after {} ms",
                        timeout.as_millis()
                    )));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };

    // The OTHER door onto the same abandoned tree. Here the process exited and
    // was reaped, and a helper it spawned is still holding the inherited pipe —
    // so a descendant is provably alive, in the detached session, with the
    // install about to fail. Returning without this is the #379 leak reached by
    // the drain guard instead of by the wall clock.
    let out = match join_drain(h_out, "stdout") {
        Ok(bytes) => bytes,
        Err(e) => {
            terminate_hardened_tree(child_pid);
            return Err(e);
        }
    };
    let err = match join_drain(h_err, "stderr") {
        Ok(bytes) => bytes,
        Err(e) => {
            terminate_hardened_tree(child_pid);
            return Err(e);
        }
    };
    if !status.success() {
        return Err(PluginCliError::Git(format!(
            "{label} failed: {}",
            String::from_utf8_lossy(&err).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

/// Join one drain thread within [`DRAIN_GRACE`], or fail.
///
/// FAILS CLOSED in both directions, and that is the whole point. A thread still
/// blocked on a pipe some helper is holding, and a thread that panicked, both
/// become `Err`. Neither may turn into an empty-but-successful capture: this
/// function's stdout is the supply-chain pin (`rev-parse HEAD` at the call site
/// below) that gets written to the lockfile, and `resolved_sha.is_empty()` is
/// the only thing standing between an empty capture and a recorded provenance
/// entry.
fn join_drain(handle: std::thread::JoinHandle<Vec<u8>>, stream: &str) -> Result<Vec<u8>> {
    let deadline = Instant::now() + DRAIN_GRACE;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return Err(PluginCliError::Git(format!(
                "git exited but its {stream} pipe is still open after {} s. A process \
                 git spawned (a credential, askpass or transport helper) has left a \
                 background worker holding the inherited pipe, so this read can never \
                 reach EOF. Refusing to wait: an unbounded join here hangs the install \
                 with no diagnostic, after the wall-clock guard has already passed.",
                DRAIN_GRACE.as_secs()
            )));
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    handle.join().map_err(|_| {
        PluginCliError::Git(format!(
            "the git {stream} drain thread panicked; the capture is unusable and is \
             reported as an error rather than as empty output"
        ))
    })
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use wcore_types::process_liveness::{
        ProcessGroupCensus, ProcessLiveness, process_group_census, process_liveness,
    };

    /// Every quarantine `git` spawn carries the credential-prompt pins.
    ///
    /// Asserted against a real `Command` built the way `run_git` builds one,
    /// not against the pin list itself — deleting the `.env()` calls reddens
    /// this, whereas a test that compared the constant to itself would not.
    #[test]
    fn quarantine_git_spawns_cannot_be_asked_to_prompt() {
        let cmd = build_git_command(&["clone", "--", "https://example.invalid/p", "/x"], None);

        let seen: std::collections::HashMap<String, Option<String>> = cmd
            .get_envs()
            .map(|(k, v)| {
                (
                    k.to_string_lossy().into_owned(),
                    v.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect();

        for (key, value) in [
            ("GIT_TERMINAL_PROMPT", "0"),
            ("GIT_ASKPASS", ""),
            ("SSH_ASKPASS", ""),
            ("SSH_ASKPASS_REQUIRE", "never"),
            ("GCM_INTERACTIVE", "Never"),
            ("GIT_PAGER", "cat"),
        ] {
            assert_eq!(
                seen.get(key).cloned().flatten().as_deref(),
                Some(value),
                "{key} is not pinned on the quarantine git command: {seen:?}"
            );
        }

        // Negative control: hardening pins prompting, it does not blanket-clear
        // the environment. `credential.helper` must keep working for private
        // plugin sources, so nothing here may remove an unrelated variable.
        assert!(
            !seen.contains_key("PATH"),
            "hardening must not touch unrelated environment entries: {seen:?}"
        );
    }

    /// `run_git` must not be wedgeable by a process `git` leaves behind.
    ///
    /// The production trigger is a credential / askpass / transport helper that
    /// keeps a background worker alive; git spawns those with its own stdio
    /// INHERITED and does not detach them. A `!`-alias is the cheapest way to
    /// reproduce that exact shape with no network and no stored credentials.
    ///
    /// Graded at the call site: this drives `run_git` itself, so rewiring it
    /// around `join_drain` is caught, not just a change to `join_drain`.
    ///
    /// Unix-only because backgrounding a process portably from a git alias
    /// needs a POSIX shell. The guard itself is platform-independent.
    #[test]
    fn a_helper_holding_a_pipe_is_reported_instead_of_hanging_the_install() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        let budget = Duration::from_secs(60);
        run_git(&["init", "-q", "."], Some(repo), budget).expect("git init");

        // CONTROL FIRST. An ordinary call through the same path still returns,
        // so the arm below cannot pass merely because run_git is broken.
        let inside = run_git(&["rev-parse", "--is-inside-work-tree"], Some(repo), budget)
            .expect("control: an ordinary git call must still succeed");
        assert_eq!(inside.trim(), "true", "control returned {inside:?}");

        // The defect arm: git spawns a child that backgrounds a worker holding
        // the pipes it inherited, then exits promptly. The pipe never sees EOF.
        let started = Instant::now();
        let err = run_git(
            &["-c", "alias.leak=!sh -c 'sleep 120 & exit 0'", "leak"],
            Some(repo),
            budget,
        )
        .expect_err("run_git returned instead of reporting a pipe held open");
        let elapsed = started.elapsed();

        let message = err.to_string();
        assert!(
            message.contains("pipe is still open"),
            "the failure must name the held pipe so it is not read as a git error: {message}"
        );
        assert!(
            elapsed >= DRAIN_GRACE,
            "it must actually wait the grace period, not fail early for some other \
             reason: {elapsed:?}"
        );
        assert!(
            elapsed < budget,
            "it must be the drain guard that fires, not the wall-clock guard: {elapsed:?}"
        );
    }

    /// The two pids a probe script records, once the file it writes is whole.
    ///
    /// The script writes to `<path>.tmp` and renames, so a partial read is not
    /// possible; the loop is for the ordinary case where the probe has not been
    /// scheduled yet. Fails rather than returning a guess.
    fn recorded_pids(path: &Path) -> (u32, u32) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                let mut fields = text.split_whitespace();
                if let (Some(first), Some(second)) = (fields.next(), fields.next()) {
                    if let (Ok(first), Ok(second)) = (first.parse(), second.parse()) {
                        return (first, second);
                    }
                }
            }
            assert!(
                Instant::now() < deadline,
                "the probe never recorded its pids at {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// SIGKILL a process group, ignoring every failure.
    ///
    /// Test-local cleanup ONLY: the assertions below are taken BEFORE this runs
    /// so that a red arm still reports what survived instead of tidying the
    /// evidence away, and this then stops a failing run leaving five minutes of
    /// `sleep` on the host.
    fn reap_group(pgid: u32) {
        if let Ok(pgid) = libc::pid_t::try_from(pgid) {
            if pgid > 1 {
                // SAFETY: signalling a process group this test created.
                unsafe {
                    libc::kill(-pgid, libc::SIGKILL);
                }
            }
        }
    }

    /// THE WALL-CLOCK EXIT. A hardened child that never exits is torn down as a
    /// whole session, not as one pid.
    ///
    /// The child is spawned through `harden_against_credential_prompt` and run
    /// through `run_hardened`, so this grades the production control flow;
    /// `run_git` is that function with a `git` command built for it. `/bin/sh`
    /// stands in for `git` because the leak has to be provoked on demand and no
    /// real `git` invocation hangs to order.
    ///
    /// Shape: background a descendant, record both pids, then `exec` into a
    /// long sleep so the direct child is unkillably idle rather than merely
    /// slow. The descendant inherits the process group `setsid` created and
    /// nothing else — no terminal, no parent that will outlive the kill — so
    /// after the timeout it is reachable ONLY through a group signal. That is
    /// precisely what a `git` credential/askpass/transport helper's background
    /// worker is, and precisely what `Child::kill` alone cannot reach.
    #[test]
    fn a_timed_out_quarantine_child_takes_its_whole_session_with_it() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pids = tmp.path().join("pids");
        let script = format!(
            "sleep 300 & printf '%d %d\\n' \"$$\" \"$!\" > \"{p}.tmp\" \
             && mv \"{p}.tmp\" \"{p}\"; exec sleep 300",
            p = pids.display()
        );

        let mut cmd = std::process::Command::new("/bin/sh");
        cmd.arg("-c")
            .arg(&script)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        harden_against_credential_prompt(&mut cmd);

        let started = Instant::now();
        let err = run_hardened(cmd, "git [\"clone\"]", Duration::from_millis(750))
            .expect_err("the wall-clock guard must fire on a child that never exits");
        let elapsed = started.elapsed();

        // CONTROL on the arm itself: this must be the timeout exit, not the
        // drain exit or a spawn failure, or the teardown under test never ran.
        let message = err.to_string();
        assert!(
            message.contains("timed out after"),
            "the run must end on the WALL-CLOCK guard for this to grade the \
             timeout teardown: {message}"
        );
        assert!(
            elapsed < DRAIN_GRACE,
            "it must be the wall-clock guard that fires, not the drain guard: {elapsed:?}"
        );

        let (leader, descendant) = recorded_pids(&pids);
        assert_ne!(
            leader, descendant,
            "the probe recorded one pid twice, so the arm proves nothing"
        );

        // MEASURE FIRST. `SIGKILL` is delivered synchronously but the corpse is
        // reaped by whoever inherits it, so allow that to land; the loop exits
        // early on success and the assertion below reports the final state.
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut descendant_state = process_liveness(descendant);
        let mut census = process_group_census(leader);
        while (descendant_state.is_live() || census != ProcessGroupCensus::Live(0))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
            descendant_state = process_liveness(descendant);
            census = process_group_census(leader);
        }

        reap_group(leader);

        assert_eq!(
            descendant_state,
            ProcessLiveness::Dead,
            "a descendant ({descendant}) of the timed-out quarantine child survived the \
             timeout. `setsid` put it in a session this process does not own, so killing \
             the direct child alone leaves it running with nobody to reap it \
             (FerroxLabs/wayland-core#379)"
        );
        assert_eq!(
            census,
            ProcessGroupCensus::Live(0),
            "the process group the quarantine spawn created still has live members after \
             the timeout; `Indeterminate` is NOT zero and must not be read as success"
        );
    }

    /// THE DRAIN-GRACE EXIT — the same abandoned tree reached by the other door.
    ///
    /// `git` exits promptly and IS reaped, so the wall-clock guard never fires;
    /// what fails the install is a helper's background worker still holding the
    /// inherited pipe. That worker is provably alive, provably in the detached
    /// session, and until #379 it was returned from and forgotten. Graded
    /// separately from the timeout arm because a fix applied to one branch does
    /// not reach the other: each is reddened by its own mutation.
    #[test]
    fn a_helper_that_outlives_the_drain_guard_is_torn_down_with_its_session() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        let pids = repo.join("pids");
        let budget = Duration::from_secs(60);
        run_git(&["init", "-q", "."], Some(repo), budget).expect("git init");

        let alias = format!(
            "alias.leak=!sh -c 'sleep 300 & printf \"%d %d\\n\" \"$$\" \"$!\" \
             > \"{p}.tmp\" && mv \"{p}.tmp\" \"{p}\"; exit 0'",
            p = pids.display()
        );
        let err = run_git(&["-c", &alias, "leak"], Some(repo), budget)
            .expect_err("the drain guard must fire on a helper holding the pipe");

        let message = err.to_string();
        assert!(
            message.contains("pipe is still open"),
            "the run must end on the DRAIN guard for this to grade the drain-exit \
             teardown: {message}"
        );

        let (_helper_shell, descendant) = recorded_pids(&pids);

        let deadline = Instant::now() + Duration::from_secs(10);
        let mut descendant_state = process_liveness(descendant);
        while descendant_state.is_live() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
            descendant_state = process_liveness(descendant);
        }

        reap_group(descendant);

        assert_eq!(
            descendant_state,
            ProcessLiveness::Dead,
            "the background worker ({descendant}) that held the pipe open survived the \
             drain-grace exit. It is the same unreaped detached tree as the timeout \
             path, reached through the drain guard instead of the wall clock \
             (FerroxLabs/wayland-core#379)"
        );
    }
}
