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

/// `DETACHED_PROCESS` — the child gets no console, and does not inherit ours.
///
/// Module-level because two places need it and they must agree: the hardening
/// hook, and [`spawn_owned`], which has to OR it together with
/// `CREATE_SUSPENDED` (`Command::creation_flags` REPLACES the flags rather than
/// adding to them, so setting the job flag separately would silently drop this
/// one).
#[cfg(windows)]
const DETACHED_PROCESS: u32 = 0x0000_0008;

/// Config pins prepended to every quarantine `git` argv on Windows.
///
/// An empty `credential.helper` value is git's own spelling of "reset the
/// helper list", and `-c` is parsed last, so it clears every helper the system,
/// global, local and URL-scoped config contributed. No helper process is
/// spawned at all, which is the only elimination Windows offers — see
/// [`harden_against_credential_prompt`] for the measurement that rules out the
/// unix approach there.
#[cfg(windows)]
const WINDOWS_CREDENTIAL_PINS: &[&str] = &["-c", "credential.helper="];

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
/// **Windows has no equivalent of that, and this is measured, not assumed.**
/// `DETACHED_PROCESS` withholds the parent's console at CREATION time only; it
/// is not a boundary. On Windows 11 build 26200 all three re-acquisition routes
/// succeed and land text on the launching process's own console:
///
/// * the direct `DETACHED_PROCESS` child calling
///   `AttachConsole(ATTACH_PARENT_PROCESS)`,
/// * a console-less GRANDCHILD calling `AttachConsole(<launcher pid>)`,
/// * a grandchild that was given its own console calling `FreeConsole()` and
///   then `AttachConsole(<launcher pid>)`.
///
/// A `setsid`'d unix child cannot do the analogous thing, because `TIOCSCTTY`
/// refuses a terminal that is already another session's controlling terminal.
/// So route 2 CANNOT be closed on Windows by taking the terminal away.
///
/// `credential.helper` is therefore cleared on WINDOWS ONLY, via
/// [`build_git_command`]. That is the second of the three policies issue #338
/// itself puts on the table, and it is the only one available on a platform
/// where the first (deny the terminal) is measurably unavailable: a helper that
/// is never spawned cannot re-acquire a console. On unix it stays untouched,
/// because there route 2 really is closed and clearing it would break installs
/// from private plugin sources for no gain.
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
    // Before the caller's args, so the caller's subcommand still parses and so
    // a later `-c` from a caller cannot be shadowed by ours.
    #[cfg(windows)]
    cmd.args(WINDOWS_CREDENTIAL_PINS);
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

/// Ownership of the whole process TREE one quarantine `git` invocation creates.
///
/// # Why the direct child is not enough
///
/// `harden_against_credential_prompt` puts the child in a NEW SESSION on unix
/// (`setsid`). That is the fix for issue #338, and it is also exactly what
/// makes `child.kill()` insufficient: `git` spawns credential, askpass and
/// transport helpers with its own stdio inherited and does NOT detach them
/// (the reason [`DRAIN_GRACE`] exists), and those descendants now live in a
/// session `wayland-core` does not own — no group signal of ours reaches them,
/// and a terminal hangup no longer reaps them either. Killing the direct pid
/// leaves them running with no owner after the install has already reported an
/// error. MEASURED on hetzner before this type existed: SIGKILL to a `setsid`'d
/// shell left its backgrounded grandchild alive in the detached session.
///
/// # What each platform can own
///
/// `setsid` makes the child the leader of a fresh process group whose id equals
/// its pid, so `kill(-pid, SIGKILL)` reaches the whole tree. [`spawn_owned`]
/// verifies that leadership against the kernel BEFORE recording it, because
/// signalling a group we do not lead would signal `wayland-core` itself.
///
/// Windows has no process group. The workspace primitive is a kill-on-close Job
/// Object — [`wcore_types::job_object::WindowsJobObject`] — the same one the MCP
/// stdio transport and the sandbox `ProcessTreeGuard` use, so there is one
/// definition of "own this tree" and not a third copy.
struct GitProcessTree {
    #[cfg(unix)]
    process_group: Option<libc::pid_t>,
    #[cfg(windows)]
    job: Option<wcore_types::job_object::WindowsJobObject>,
}

impl GitProcessTree {
    /// Give up ownership WITHOUT killing anything.
    ///
    /// Called on the one path where the tree is not ours to reap: `git` ran to
    /// completion and succeeded. What can still be alive there is git's OWN
    /// deliberately-daemonized helper (`git-credential-cache--daemon`, measured
    /// in [`DRAIN_GRACE`]'s note to hold none of our pipes), which outliving
    /// the clone is its documented job.
    fn disarm(&mut self) {
        #[cfg(unix)]
        {
            self.process_group = None;
        }
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            // Windows offers no way to release a kill-on-close job except by
            // closing its last handle, and closing it is what terminates the
            // tree. Holding the handle instead is deliberate: it is bounded at
            // one per quarantine `git` invocation (an install makes at most
            // five), and its only effect is that anything still in the job is
            // reaped no later than process exit.
            std::mem::forget(job);
        }
    }

    /// Kill the tree. Idempotent — a second call has nothing left to take.
    fn reap(&mut self) {
        #[cfg(unix)]
        if let Some(pgid) = self.process_group.take() {
            // SAFETY: a plain signal call. `pgid` was verified in
            // `spawn_owned` to be a group this call created and leads; the
            // negative pid addresses that group.
            //
            // The leader may already have been reaped by `try_wait` by now, so
            // in principle its pid could be recycled. Signalling the wrong tree
            // would need a double coincidence: the kernel handing that exact
            // pid to a new process AND that process becoming a group leader
            // (processes inherit their parent's group unless they call
            // `setsid`/`setpgid`). While any member of our group is alive the
            // id cannot be reused at all, and if none is, there was nothing to
            // kill.
            unsafe {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
        #[cfg(windows)]
        if let Some(job) = self.job.take() {
            job.terminate();
        }
    }
}

impl Drop for GitProcessTree {
    /// The backstop that makes this correct by construction: EVERY error
    /// return from [`run_git`] reaps the tree, including the ones added later.
    /// Only the success path opts out, and it has to say so explicitly.
    fn drop(&mut self) {
        self.reap();
    }
}

/// Spawn `cmd` and take ownership of the process tree it will create.
///
/// Fails closed on unix: a child that is not its own process-group leader is
/// killed and reported rather than run, because its descendants could not be
/// reaped and `kill(-pgid)` on the group we would have recorded is our own.
fn spawn_owned(cmd: &mut std::process::Command) -> Result<(std::process::Child, GitProcessTree)> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
        // `WindowsJobObject::create_suspended` would REPLACE the creation
        // flags, dropping `DETACHED_PROCESS`; both are set together here.
        // Suspended until the job assignment lands, so the child cannot create
        // a descendant outside the job — see `wcore_types::job_object`.
        cmd.creation_flags(DETACHED_PROCESS | CREATE_SUSPENDED);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| PluginCliError::Git(format!("spawn git: {e}")))?;

    #[cfg(unix)]
    {
        let pid = libc::pid_t::try_from(child.id()).unwrap_or(-1);
        // SAFETY: a plain query taking no pointers.
        let pgid = unsafe { libc::getpgid(pid) };
        if pid <= 0 || pgid != pid {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PluginCliError::Git(format!(
                "quarantine git child {pid} is not the leader of its own process group                  (pgid {pgid}); refusing to run a git whose helpers could not be reaped"
            )));
        }
        Ok((
            child,
            GitProcessTree {
                process_group: Some(pid),
            },
        ))
    }
    #[cfg(windows)]
    {
        match wcore_types::job_object::WindowsJobObject::attach(child.id()) {
            Ok(job) => Ok((child, GitProcessTree { job: Some(job) })),
            Err(e) => {
                // `attach` may fail before or after the assignment landed; the
                // kill is correct for both shapes (see its docs).
                let _ = child.kill();
                let _ = child.wait();
                Err(PluginCliError::Git(format!(
                    "own the quarantine git process tree: {e}"
                )))
            }
        }
    }
}

fn run_git(args: &[&str], cwd: Option<&Path>, timeout: Duration) -> Result<String> {
    let mut cmd = build_git_command(args, cwd);

    // `tree` outlives every early return below: its `Drop` reaps the helpers
    // `git` spawned on EVERY error path, and only the success path disarms it.
    let (mut child, mut tree) = spawn_owned(&mut cmd)?;

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
                    // The tree FIRST, while the leader is still alive to
                    // anchor the group id, then the direct pid so the zombie
                    // is reaped and `Child` updates its own state.
                    tree.reap();
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(PluginCliError::Git(format!(
                        "git {:?} timed out after {} ms",
                        args,
                        timeout.as_millis()
                    )));
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    };

    let out = join_drain(h_out, "stdout")?;
    let err = join_drain(h_err, "stderr")?;
    if !status.success() {
        return Err(PluginCliError::Git(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&err).trim()
        )));
    }
    // Success: git exited 0 and both pipes reached EOF, so nothing of OURS is
    // left in the tree. Anything still there is git's own daemon and outliving
    // the clone is its job.
    tree.disarm();
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

#[cfg(test)]
mod tests {
    use super::*;

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
        // the environment. Nothing here may remove an unrelated variable.
        assert!(
            !seen.contains_key("PATH"),
            "hardening must not touch unrelated environment entries: {seen:?}"
        );

        // Windows cannot deny the helper a console (measured — see
        // `harden_against_credential_prompt`), so it denies the helper
        // instead, and the reset must lead the argv or a later `-c` would
        // re-add what it cleared.
        #[cfg(windows)]
        {
            let argv: Vec<String> = cmd
                .get_args()
                .map(|a| a.to_string_lossy().into_owned())
                .collect();
            assert_eq!(
                argv.first().map(String::as_str),
                Some("-c"),
                "the credential pins must lead the quarantine argv: {argv:?}"
            );
            assert_eq!(
                argv.get(1).map(String::as_str),
                Some("credential.helper="),
                "the quarantine argv must reset the credential helper list: {argv:?}"
            );
        }
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
    #[cfg(unix)]
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

    /// Is `pid` a live (non-reaped) process?
    ///
    /// `kill(pid, 0)` is the only portable oracle here. It also answers "yes"
    /// for a zombie, which is the conservative direction for these tests: a
    /// false "alive" fails them, it never passes them.
    #[cfg(unix)]
    fn is_alive(pid: libc::pid_t) -> bool {
        // SAFETY: signal 0 performs the permission/existence check only.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// Wait up to `budget` for `pid` to disappear.
    #[cfg(unix)]
    fn wait_gone(pid: libc::pid_t, budget: Duration) -> bool {
        let deadline = Instant::now() + budget;
        while Instant::now() < deadline {
            if !is_alive(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        !is_alive(pid)
    }

    /// Prove the liveness oracle can say BOTH things in this process, so a
    /// "the descendant is gone" result below cannot come from an oracle that
    /// only ever says "gone".
    #[cfg(unix)]
    fn assert_oracle_is_bidirectional() {
        let mut probe = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn the oracle control");
        let pid = probe.id() as libc::pid_t;
        assert!(
            is_alive(pid),
            "the liveness oracle failed to see a process it just spawned"
        );
        let _ = probe.kill();
        let _ = probe.wait();
        assert!(
            wait_gone(pid, Duration::from_secs(5)),
            "the liveness oracle failed to see a process it just killed"
        );
    }

    /// A quarantine `git` that times out must take the helpers it spawned with
    /// it — not just its own pid.
    ///
    /// The `setsid` hardening for #338 is what makes this load-bearing: the
    /// descendants are in a session this process does not own, so the previous
    /// `child.kill()` left them running with no owner and nothing else would
    /// ever reap them. A `!`-alias reproduces the exact production shape (a
    /// helper `git` spawns that backgrounds a worker) with no network.
    #[cfg(unix)]
    #[test]
    fn a_timed_out_git_reaps_the_whole_detached_tree() {
        assert_oracle_is_bidirectional();

        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        run_git(&["init", "-q", "."], Some(repo), Duration::from_secs(60)).expect("git init");

        let pidfile = repo.join("worker.pid");
        let alias = format!(
            "alias.wedge=!sh -c 'sleep 300 & echo $! > {} ; sleep 300'",
            pidfile.display()
        );

        let started = Instant::now();
        let err = run_git(
            &["-c", &alias, "wedge"],
            Some(repo),
            Duration::from_millis(1_500),
        )
        .expect_err("the wall-clock guard must fire");
        assert!(
            err.to_string().contains("timed out"),
            "it must be the timeout path that fired: {err}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the guard must fire on its own budget: {:?}",
            started.elapsed()
        );

        // Non-vacuity: the helper really did create a backgrounded descendant.
        let worker: libc::pid_t = std::fs::read_to_string(&pidfile)
            .expect("the helper must have recorded its background worker's pid")
            .trim()
            .parse()
            .expect("worker pid");
        assert!(worker > 0, "worker pid {worker}");

        assert!(
            wait_gone(worker, Duration::from_secs(10)),
            "the background worker {worker} that the timed-out git spawned is STILL ALIVE — \
             killing the direct child does not reach a descendant in the detached session"
        );
    }

    /// The same obligation on the OTHER failure exit: `git` exits promptly but
    /// a helper it spawned holds the inherited pipe, so `join_drain` refuses.
    ///
    /// Enumerated deliberately — `run_git` has two failure shapes that leave a
    /// tree behind, and an entry written from one of them leaves the other to
    /// surface later.
    #[cfg(unix)]
    #[test]
    fn a_pipe_holding_helper_is_reaped_when_the_drain_guard_fires() {
        assert_oracle_is_bidirectional();

        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        run_git(&["init", "-q", "."], Some(repo), Duration::from_secs(60)).expect("git init");

        let pidfile = repo.join("worker.pid");
        let alias = format!(
            "alias.leak=!sh -c 'sleep 300 & echo $! > {} ; exit 0'",
            pidfile.display()
        );

        let err = run_git(&["-c", &alias, "leak"], Some(repo), Duration::from_secs(60))
            .expect_err("the drain guard must fire");
        assert!(
            err.to_string().contains("pipe is still open"),
            "it must be the drain guard that fired, not the wall clock: {err}"
        );

        let worker: libc::pid_t = std::fs::read_to_string(&pidfile)
            .expect("the helper must have recorded its background worker's pid")
            .trim()
            .parse()
            .expect("worker pid");
        assert!(worker > 0, "worker pid {worker}");

        assert!(
            wait_gone(worker, Duration::from_secs(10)),
            "the pipe-holding worker {worker} survived the drain-guard failure — the install \
             reported an error and left an unowned process running"
        );
    }

    /// A worker `git` leaves behind must stop doing work when the guard fires
    /// — on EVERY platform, not just the one that has process groups.
    ///
    /// Deliberately observes WORK rather than a pid. The pid-based arms above
    /// are the stronger unix evidence, but `git`'s `!`-alias shell on Windows
    /// is an msys one whose `$!` is an msys pid, not a Win32 pid, so a pid
    /// oracle there would be measuring the wrong namespace. A file that stops
    /// growing is the same claim in a namespace both platforms share.
    #[test]
    fn a_timed_out_git_leaves_no_descendant_still_doing_work() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path();
        run_git(&["init", "-q", "."], Some(repo), Duration::from_secs(60)).expect("git init");

        let beat = repo.join("heartbeat");
        let script = repo.join("worker.sh");
        let sh_beat = beat.display().to_string().replace('\\', "/");
        let sh_script = script.display().to_string().replace('\\', "/");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n( while : ; do echo x >> '{sh_beat}' ; sleep 0.1 ; done ) &\nsleep 300\n"
            ),
        )
        .expect("write worker");

        // ORACLE CONTROL: prove that "the file stopped growing" is a signal
        // this test can distinguish from "the file was never growing".
        //
        // Its own file and a BOUNDED, self-terminating writer that is the
        // shell's own foreground work — no background job, so it cannot
        // outlive the control and go on writing into the arm's measurement.
        // (It did, in the first draft of this test, and produced a red arm
        // that was the control's writer rather than git's.)
        let ctl_beat = repo.join("control-heartbeat");
        let sh_ctl_beat = ctl_beat.display().to_string().replace('\\', "/");
        let mut ctl = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!(
                "i=0; while [ $i -lt 20 ]; do echo x >> '{sh_ctl_beat}'; sleep 0.1; \
                 i=$((i+1)); done"
            ))
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn the growth control");
        std::thread::sleep(Duration::from_millis(600));
        let a = size(&ctl_beat);
        std::thread::sleep(Duration::from_millis(600));
        let b = size(&ctl_beat);
        assert!(
            b > a && a > 0,
            "the growth oracle never saw the heartbeat grow ({a} -> {b}); this test could not \
             tell a live worker from a dead one"
        );
        let _ = ctl.wait();

        let alias = format!("alias.wedge=!sh '{sh_script}'");
        let err = run_git(
            &["-c", &alias, "wedge"],
            Some(repo),
            Duration::from_millis(2_000),
        )
        .expect_err("the wall-clock guard must fire");
        assert!(
            err.to_string().contains("timed out"),
            "it must be the timeout path that fired: {err}"
        );

        // Non-vacuity: the helper really did start a background worker.
        let at_kill = size(&beat);
        assert!(
            at_kill > 0,
            "the helper never wrote a heartbeat, so nothing was left running to reap and this \
             test proves nothing"
        );

        std::thread::sleep(Duration::from_secs(3));
        let later = size(&beat);
        assert_eq!(
            later, at_kill,
            "a background worker the timed-out git spawned is STILL WRITING ({at_kill} -> \
             {later} bytes) — the guard reaped the direct child and not the tree"
        );
    }

    fn size(p: &Path) -> u64 {
        std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
    }
}
