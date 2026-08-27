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
//
// core#338: the clone also gets NO authority over the user's TERMINAL. Handing
// `git` a `/dev/null` stdin does not stop it — or a credential helper it spawns
// — opening `/dev/tty` directly and drawing a credential prompt the user has no
// way to tell apart from one Wayland wrote. Installing a plugin is consent to
// fetch someone else's code; it is not consent to hand that someone your
// credentials. See `deny_terminal` and the askpass hardening in `run_git`.

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

/// Take the user's terminal away from the quarantine child (core#338).
///
/// `Stdio::null()` on stdin is NOT enough. `git` — and every credential helper,
/// `git-remote-*` and `ssh` it spawns — can `open("/dev/tty")` itself, which is
/// a fresh handle on the controlling terminal and is blind to what we handed
/// the child as fd 0. Measured through this function on git 2.43: with stdin on
/// `/dev/null`, a `401` from an attacker-chosen URL drew
/// `Username for 'http://…':` in the user's own terminal, and a credential
/// helper reading `/dev/tty` drew a prompt of the attacker's own wording there.
///
/// A new process GROUP does not close this, and must not be mistaken for a
/// cheaper spelling of it. Measured on this box, three arms, child stdin on
/// `/dev/null` in all three:
///
/// | child does      | write to `/dev/tty` | read from `/dev/tty` |
/// |-----------------|---------------------|----------------------|
/// | nothing         | lands on the user's screen | blocks, reading their keystrokes |
/// | `setpgid(0, 0)` | STILL lands on the user's screen | `SIGTTIN`, child stops — a wedged install |
/// | `setsid()`      | `ENXIO`, open fails | `ENXIO`, open fails |
///
/// So `Command::process_group(0)` — which is `setpgid`, not `setsid`, whatever
/// the comments at the `cron.rs` and `profile_router.rs` call sites say — would
/// leave an untrusted clone able to paint arbitrary text (a forged prompt, raw
/// escape sequences) on the user's terminal and would turn the credential read
/// into a hang. Only a new SESSION fails closed. Do not "simplify" this.
///
/// The cost, measured rather than assumed: a new session also puts the child
/// out of the terminal's foreground process group, so a `Ctrl+C` that kills
/// `wayland-core` no longer reaches the clone (probe: child survives = false
/// without `setsid`, true with it). The clone is then an orphan until it
/// finishes on its own. That is accepted. The obvious mitigation,
/// `PR_SET_PDEATHSIG`, is Linux-only and fires when the spawning THREAD exits,
/// which in this tokio-threaded binary would kill healthy clones whenever a
/// blocking worker is retired — a worse failure than an orphan.
#[cfg(unix)]
fn deny_terminal(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt as _;
    // SAFETY: the closure runs in the child between `fork` and `exec`, where
    // only async-signal-safe work is legal. `setsid(2)` is a bare syscall — no
    // allocation, no locks, no re-entrancy. It cannot fail here for the one
    // documented reason (`EPERM`, caller already a process-group leader),
    // because a freshly forked child never leads its parent's group; if it
    // somehow does, returning `Err` fails the spawn, which is the behaviour we
    // want. `run_git`'s timeout path kills the child's process GROUP and relies
    // on this call having made the child its own leader, so a child that could
    // not detach must never start.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Windows has no `/dev/tty`. The askpass and `GIT_TERMINAL_PROMPT` hardening
/// in [`run_git`] still applies there, but a GUI credential manager is a
/// different authority (the desktop session) that this function does not claim
/// to close.
#[cfg(not(unix))]
fn deny_terminal(_cmd: &mut std::process::Command) {}

/// Kill the whole quarantine child, not just its leader.
#[cfg(unix)]
fn kill_child_tree(child: &mut std::process::Child) {
    // [`deny_terminal`] made the child a session leader, so its pgid equals its
    // pid and the negated pid names exactly this clone and its descendants —
    // `git-remote-http`, credential helpers, `ssh`. Killing only the leader
    // leaves those alive holding the write ends of our stdout/stderr pipes,
    // which is what stranded a reader thread in `read()` for the lifetime of
    // the process (measured: 2 threads before the clone, 3 after).
    //
    // SAFETY: a bare `kill(2)`. The target group was created by `setsid` in
    // `deny_terminal` and is the child's own, so this can never name our own
    // group; a spawn whose `setsid` failed returns no child at all.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill_child_tree(child: &mut std::process::Child) {
    let _ = child.kill();
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
///
/// core#338 adds the terminal and askpass boundary on top of that; see
/// [`deny_terminal`].
fn run_git(args: &[&str], cwd: Option<&Path>, timeout: Duration) -> Result<String> {
    let mut cmd = std::process::Command::new("git");
    // core#338, the ASKPASS door — a second, independent way for an untrusted
    // clone to collect credentials, and one that needs no terminal at all, so
    // `deny_terminal` cannot reach it. An `askPass` program in the user's own
    // gitconfig answers `git` silently. Measured on git 2.43 against a remote
    // that answers `401`: without this override the askpass ran twice and the
    // remote received an `Authorization: Basic` header; with it the askpass ran
    // zero times and the remote received nothing. An EMPTY value is how git
    // spells "no askpass" (`if (askpass && *askpass)`), and `-c` must precede
    // the subcommand — every `args` array here starts with either `-c` or the
    // subcommand itself, so prepending is always well-formed.
    cmd.arg("-c").arg("core.askPass=");
    cmd.args(args);
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // `GIT_ASKPASS` outranks `core.askPass` and `SSH_ASKPASS` backs it up, so
    // the config override above is only half of that door; these close the
    // environment half, including the desktop-session askpass an IDE exports.
    //
    // `GIT_TERMINAL_PROMPT=0` is NOT the boundary and must never be mistaken
    // for one: it governs git's OWN prompting and says nothing about what a
    // credential helper does with `/dev/tty`. Measured by mutation on Linux, it
    // is REDUNDANT here — delete it and every leg of
    // `quarantine_terminal_authority.rs` still passes, because `deny_terminal`
    // has already taken the terminal away. It is kept for exactly two reasons,
    // neither of them "defence": on non-Unix, where `deny_terminal` is a no-op,
    // it is the ONLY thing holding git's own prompt; and it turns git's refusal
    // into `terminal prompts disabled` instead of an `ENXIO` on `/dev/tty`,
    // which is what a user reading the error has to work with.
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env_remove("SSH_ASKPASS_REQUIRE");
    deny_terminal(&mut cmd);

    let mut child = cmd
        .spawn()
        .map_err(|e| PluginCliError::Git(format!("spawn git: {e}")))?;

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
                    // core#338: kill the GROUP, not just the leader.
                    // Returning after killing only the leader left a reader
                    // thread blocked in `read()` for the lifetime of the
                    // process whenever a descendant — a credential helper that
                    // backgrounds a worker, `git-remote-http` — still held the
                    // inherited write end (measured: 2 threads before the
                    // clone, 3 after). Killing the group closes those write
                    // ends, and the readers then hit EOF and exit on their own;
                    // an explicit bounded join here was MEASURED to change
                    // nothing and was removed. The `setsid` in `deny_terminal`
                    // is what makes the group addressable, so one mechanism
                    // closes both this and the terminal door.
                    kill_child_tree(&mut child);
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
}
