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
// core#338: the clone also gets NO authority over the user's terminal. `git`
// can open `/dev/tty` directly, so `Stdio::null()` on stdin does not stop an
// untrusted source inducing a credential prompt in the user's own terminal.
// See `deny_terminal`.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use wcore_pluginsrc::SourceKind;

use crate::plugin::error::{PluginCliError, Result};
use crate::plugin::marketplace::reject_traversal;

const DEFAULT_GIT_TIMEOUT_MS: u64 = 120_000;
const DEFAULT_MAX_BYTES: u64 = 100_000_000;

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

/// How long the reader threads get to finish after a timeout kill. On Unix the
/// process-GROUP kill makes this effectively instant; it exists so a platform
/// without group-kill can never turn a thread leak into a caller hang.
const REAP_GRACE: Duration = Duration::from_secs(2);

/// Take the user's terminal away from the quarantine child (core#338).
///
/// `Stdio::null()` on stdin is NOT enough. `git` — and every credential helper,
/// `git-remote-*` and `ssh` it spawns — can `open("/dev/tty")` directly, which
/// both writes to and reads from the terminal the user is sitting at,
/// regardless of what we handed it as stdin. Measured on git 2.43: with stdin
/// on `/dev/null`, a `401` from the remote produced
/// `Username for 'http://…':` on the user's terminal and shipped what they
/// typed to the remote in an `Authorization: Basic` header.
///
/// A new process *group* does not close this: `setpgid` keeps the controlling
/// terminal and `/dev/tty` still opens (measured — do not "simplify" this to
/// `Command::process_group(0)`). Only a new *session* detaches the terminal,
/// after which `open("/dev/tty")` fails with `ENXIO`.
///
/// The user consented to installing a plugin. They did not consent to that
/// plugin's author being able to draw a credential prompt in their terminal,
/// and the terminal gives them nothing to tell the two apart.
#[cfg(unix)]
fn deny_terminal(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the closure runs between `fork` and `exec`, where only
    // async-signal-safe work is legal. `setsid(2)` is a bare syscall — no
    // allocation, no locks, no re-entrancy. Returning `Err` fails the spawn,
    // which is the behaviour we want: `run_git`'s timeout path kills the
    // child's process GROUP and relies on `setsid` having made the child its
    // own group leader, so a child that could not detach must never start.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Windows has no `/dev/tty`. The env and `core.askPass` hardening in `run_git`
/// still applies, but a GUI credential manager is a separate authority (the
/// desktop session) that this function does not claim to close.
#[cfg(not(unix))]
fn deny_terminal(_cmd: &mut std::process::Command) {}

/// Kill the whole quarantine child, not just its leader.
#[cfg(unix)]
fn kill_quarantine_child(child: &mut std::process::Child) {
    // `deny_terminal` made the child a session leader, so its pgid equals its
    // pid and the negated pid addresses exactly this clone and its descendants
    // — `git-remote-http`, credential helpers, `ssh`. Killing only the leader
    // leaves those alive holding the write ends of our stdout/stderr pipes,
    // which is what left the two reader threads below blocked in `read()` for
    // the lifetime of the process.
    //
    // SAFETY: a bare `kill(2)`. The target group was created by `setsid` in
    // `deny_terminal` and is the child's own, so it can never name this
    // process's group; a spawn whose `setsid` failed never returns a child.
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
}

#[cfg(not(unix))]
fn kill_quarantine_child(child: &mut std::process::Child) {
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
/// core#338 adds the terminal and askpass boundary; see `deny_terminal`.
fn run_git(args: &[&str], cwd: Option<&Path>, timeout: Duration) -> Result<String> {
    let mut cmd = std::process::Command::new("git");
    // `core.askPass=` is a SECOND door, independent of the terminal: an askpass
    // program set in the user's own gitconfig needs no tty at all, so
    // `deny_terminal` does not touch it. Measured: with a `core.askPass` in
    // config, an attacker-controlled URL that answers `401` receives the
    // credentials the helper produced; with this override it receives none.
    // `-c` must precede the subcommand, and every `args` array here starts with
    // either `-c` or the subcommand, so prepending is always well-formed.
    cmd.arg("-c").arg("core.askPass=");
    cmd.args(args);
    if let Some(c) = cwd {
        cmd.current_dir(c);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // `GIT_TERMINAL_PROMPT=0` is NOT the fix and must not be mistaken for one:
    // it governs git's OWN prompting and says nothing about what a credential
    // helper does. It is here so the refusal is legible rather than a stall.
    // The env askpass vars are the same second door as `core.askPass` above.
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
    // Channels rather than `JoinHandle`s: the timeout path must be able to
    // reclaim whatever the readers have WITHOUT the unbounded wait a `join`
    // would impose if some descendant still held a pipe open.
    let (tx_out, rx_out) = std::sync::mpsc::channel();
    let (tx_err, rx_err) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = out_pipe.read_to_end(&mut b);
        let _ = tx_out.send(b);
    });
    std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = err_pipe.read_to_end(&mut b);
        let _ = tx_err.send(b);
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
                    // core#338: kill the GROUP and reap the readers. Returning
                    // here without draining them left two threads blocked in
                    // `read()` for the lifetime of the process whenever a
                    // descendant (a credential helper that backgrounds a
                    // worker, `git-remote-http`) still held the pipe.
                    kill_quarantine_child(&mut child);
                    let _ = child.wait();
                    let _ = rx_out.recv_timeout(REAP_GRACE);
                    let _ = rx_err.recv_timeout(REAP_GRACE);
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

    let out = rx_out.recv().unwrap_or_default();
    let err = rx_err.recv().unwrap_or_default();
    if !status.success() {
        return Err(PluginCliError::Git(format!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&err).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&out).into_owned())
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}
