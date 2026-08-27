//! core#338 — what authority a quarantine clone of UNTRUSTED code has over the
//! user's terminal and credentials.
//!
//! Installing a plugin is consent to fetch and run someone else's code. It is
//! not consent to hand that someone your credentials. Before this file, an
//! interactive `wayland-core` session cloning an attacker-chosen URL would let
//! `git` draw `Username for 'http://…':` in the user's own terminal — the
//! terminal offers nothing that distinguishes that prompt from one Wayland
//! itself wrote — and ship what they typed to the attacker.
//!
//! `Stdio::null()` on the child's stdin does NOT prevent this: `git` opens
//! `/dev/tty` directly. There are two independent doors and each gets a leg:
//!
//! * [`a_credential_helper_cannot_prompt_on_the_users_terminal`] — the terminal
//!   door PROPER, and the leg that measures the fix. A credential helper that
//!   reads `/dev/tty` itself is what `GIT_TERMINAL_PROMPT=0` is blind to:
//!   that variable governs git's OWN prompting and says nothing about a helper.
//!   Verified by mutation — delete the `setsid` and only this leg goes red.
//! * [`quarantine_clone_cannot_prompt_on_the_users_terminal`] — the same door
//!   via git's own prompt. Kept because it is the shape a user meets with no
//!   helper configured, but note it is held by `GIT_TERMINAL_PROMPT=0` alone:
//!   it passes with the `setsid` removed, so it must NOT be read as evidence
//!   that the terminal is denied.
//!
//! Both need a REAL controlling terminal, so they re-exec this test binary
//! under a PTY; an in-process call could never exhibit the defect.
//! * [`quarantine_clone_ignores_an_askpass_from_the_users_gitconfig`] — the
//!   askpass door, which needs no terminal at all and which detaching the
//!   session therefore cannot close.
//! * [`a_timed_out_clone_does_not_leak_its_reader_threads`] — the resource half:
//!   the wall-clock timeout used to return without draining the two pipe reader
//!   threads, and killing only the leader left a backgrounded helper holding
//!   the pipes open, so those threads blocked in `read()` for the lifetime of
//!   the process.
//!
//! Every leg asserts the fake remote was actually CONTACTED before asserting it
//! received no credentials — an unreachable server would otherwise "pass" this
//! file while proving nothing.
//!
//! Unix-only: `/dev/tty`, `setsid`, and process-group kill are the mechanism
//! under test. Windows has no `/dev/tty`; its GUI credential managers are a
//! separate authority this file does not measure.
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// Obviously-fake material. If either of these ever reaches [`Remote::seen`],
/// the product handed a third party what the user typed.
const DECOY_USER: &str = "victim-username";
const DECOY_SECRET: &str = "not-a-real-secret-0000";
/// What a helper dressed as Wayland would print. Distinctive so the
/// assertion cannot match incidental terminal noise.
const HELPER_PROMPT: &str = "Wayland needs your credentials to continue: ";

/// A remote that answers every request with a Basic-auth challenge and records
/// what it was sent. `git` retries with credentials once it has them, so an
/// `Authorization` header here IS the exfiltration.
struct Remote {
    port: u16,
    seen: Arc<Mutex<Vec<Option<String>>>>,
}

impl Remote {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake remote");
        let port = listener.local_addr().unwrap().port();
        let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut s) = stream else { continue };
                let mut buf = Vec::new();
                let mut chunk = [0u8; 1024];
                loop {
                    match s.read(&mut chunk) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let text = String::from_utf8_lossy(&buf).to_string();
                let auth = text
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .map(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()))
                    .unwrap_or(None);
                sink.lock().unwrap().push(auth);
                let _ = s.write_all(
                    b"HTTP/1.1 401 Unauthorized\r\n\
                      WWW-Authenticate: Basic realm=\"wayland-plugin\"\r\n\
                      Content-Length: 0\r\n\r\n",
                );
                let _ = s.flush();
            }
        });
        Self { port, seen }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/attacker-plugin.git", self.port)
    }

    fn requests(&self) -> usize {
        self.seen.lock().unwrap().len()
    }

    /// Every credential the remote managed to collect, decoded.
    fn credentials(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .map(|h| h.to_string())
            .collect()
    }
}

/// Write a throwaway gitconfig and return env pairs that make `git` read ONLY
/// it. Without this the developer's own `~/.gitconfig` decides the outcome and
/// the result is not a measurement of the product.
fn isolated_gitconfig(dir: &Path, body: &str) -> Vec<(String, String)> {
    let cfg = dir.join("gitconfig");
    std::fs::write(&cfg, body).expect("write gitconfig");
    let empty = dir.join("gitconfig.empty");
    std::fs::write(&empty, "").expect("write empty gitconfig");
    vec![
        ("GIT_CONFIG_GLOBAL".to_string(), cfg.display().to_string()),
        ("GIT_CONFIG_SYSTEM".to_string(), empty.display().to_string()),
    ]
}

// ===========================================================================
// The child. Runs the PRODUCTION function; the parent legs below only stage a
// terminal and a remote around it.
// ===========================================================================

/// Not a test of its own — the re-exec target. A no-op unless the parent leg
/// asked for it, so a plain `cargo nextest run` just skips through it.
#[test]
fn quarantine_tty_child_helper() {
    let Ok(url) = std::env::var("WL_QTTY_URL") else {
        return;
    };
    let dest = std::env::var("WL_QTTY_DEST").expect("WL_QTTY_DEST");
    // The result is expected to be an error (the remote never authenticates).
    // What is under test is what happened to the user's terminal on the way.
    let outcome = wcore_cli::plugin::quarantine::quarantine_clone(
        &wcore_pluginsrc::SourceKind::Url {
            url,
            git_ref: None,
            sha: None,
        },
        Path::new(&dest),
    );
    println!("CHILD-DONE err={}", outcome.is_err());
}

// ===========================================================================
// Leg 1 — the terminal door.
// ===========================================================================

/// Drive a quarantine clone of `remote` on a real PTY with `gitconfig_body` as
/// the user's entire git configuration, answering any credential prompt that
/// appears the way a deceived user would. Returns everything painted on the
/// terminal.
fn clone_on_a_terminal(tmp: &Path, remote: &Remote, gitconfig_body: &str) -> String {
    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 100,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open PTY");

    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = CommandBuilder::new(exe);
    cmd.arg("--exact");
    cmd.arg("quarantine_tty_child_helper");
    cmd.arg("--nocapture");
    cmd.env("WL_QTTY_URL", remote.url());
    cmd.env("WL_QTTY_DEST", tmp.join("q").display().to_string());
    cmd.env("TERM", "xterm-256color");
    cmd.env("WL_PROMPT", HELPER_PROMPT);
    for (k, v) in isolated_gitconfig(tmp, gitconfig_body) {
        cmd.env(k, v);
    }
    // The defence must come from the product. If the developer's shell already
    // had this set, the leg would pass without the product doing anything.
    cmd.env_remove("GIT_TERMINAL_PROMPT");

    let mut child = pty.slave.spawn_command(cmd).expect("spawn child on PTY");
    // The slave must be dropped or the master never observes EOF.
    drop(pty.slave);

    let screen: Arc<Mutex<Vec<u8>>> = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&screen);
    let mut reader = pty.master.try_clone_reader().expect("clone reader");
    std::thread::spawn(move || {
        let mut b = [0u8; 4096];
        while let Ok(n) = reader.read(&mut b) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&b[..n]);
        }
    });
    let mut writer = pty.master.take_writer().expect("take writer");

    // Behave like the deceived user: if a credential prompt appears, answer it.
    // The assertion is that it never appears — but a leg that simply refused to
    // type could not tell "no prompt" from "prompt nobody answered".
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut typed_user = false;
    let mut typed_secret = false;
    loop {
        let text = String::from_utf8_lossy(&screen.lock().unwrap().clone()).to_string();
        if text.contains("Username for") && !typed_user {
            let _ = writer.write_all(format!("{DECOY_USER}\n").as_bytes());
            let _ = writer.flush();
            typed_user = true;
        }
        if text.contains(HELPER_PROMPT) && !typed_user {
            let _ = writer.write_all(format!("{DECOY_USER}\n{DECOY_SECRET}\n").as_bytes());
            let _ = writer.flush();
            typed_user = true;
            typed_secret = true;
        }
        if text.contains("Password for") && !typed_secret {
            let _ = writer.write_all(format!("{DECOY_SECRET}\n").as_bytes());
            let _ = writer.flush();
            typed_secret = true;
        }
        if text.contains("CHILD-DONE") {
            break;
        }
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        if Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    std::thread::sleep(Duration::from_millis(200));
    String::from_utf8_lossy(&screen.lock().unwrap().clone()).to_string()
}

/// THE leg for core#338. A credential helper that opens `/dev/tty` itself
/// reaches the user's terminal no matter what we hand the child as stdin, and
/// `GIT_TERMINAL_PROMPT=0` cannot see it — the variable governs git's own
/// prompting. The prompt this helper writes is deliberately dressed as Wayland's
/// own, because that is the whole attack: the terminal gives the user nothing
/// to tell "Wayland is asking" from "the plugin author is asking".
///
/// Only detaching the session closes this. Mutation-verified: remove the
/// `setsid` from `deny_terminal` and this leg is the one that goes red.
#[test]
fn a_credential_helper_cannot_prompt_on_the_users_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = Remote::start();

    let helper = tmp.path().join("prompting_helper.sh");
    std::fs::write(
        &helper,
        "#!/bin/sh\n\
         [ \"$1\" = get ] || exit 0\n\
         exec 3<>/dev/tty || exit 1\n\
         printf '%s' \"$WL_PROMPT\" >&3\n\
         read u <&3\n\
         read p <&3\n\
         echo \"username=$u\"\n\
         echo \"password=$p\"\n",
    )
    .unwrap();
    std::fs::set_permissions(
        &helper,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();

    let transcript = clone_on_a_terminal(
        tmp.path(),
        &remote,
        &format!(
            "[user]\n\tname = test\n[credential]\n\thelper = {}\n",
            helper.display()
        ),
    );

    assert!(
        remote.requests() > 0,
        "fake remote was never contacted — this leg proved nothing.\nterminal:\n{transcript}"
    );
    assert!(
        !transcript.contains(HELPER_PROMPT),
        "a credential helper run by an untrusted quarantine clone drew a prompt in \
         the user's terminal.\nterminal:\n{transcript}"
    );
    assert!(
        remote.credentials().is_empty(),
        "the untrusted remote received credentials: {:?}\nterminal:\n{transcript}",
        remote.credentials()
    );
}

/// The same door via git's OWN prompt, which is the shape a user with no
/// credential helper meets.
///
/// Read this leg for what it is: `GIT_TERMINAL_PROMPT=0` alone holds it, and it
/// still passes with the `setsid` deleted. It is a regression guard on the env
/// hardening, NOT evidence that the terminal is denied — that is
/// [`a_credential_helper_cannot_prompt_on_the_users_terminal`]'s job.
#[test]
fn quarantine_clone_cannot_prompt_on_the_users_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = Remote::start();
    let transcript = clone_on_a_terminal(tmp.path(), &remote, "[user]\n\tname = test\n");

    assert!(
        remote.requests() > 0,
        "fake remote was never contacted — this leg proved nothing.\nterminal:\n{transcript}"
    );
    assert!(
        !transcript.contains("Username for") && !transcript.contains("Password for"),
        "an untrusted quarantine clone drew a credential prompt in the user's terminal.\n\
         terminal:\n{transcript}"
    );
    assert!(
        remote.credentials().is_empty(),
        "the untrusted remote received credentials: {:?}\nterminal:\n{transcript}",
        remote.credentials()
    );
}

// ===========================================================================
// Leg 2 — the askpass door, which needs no terminal.
// ===========================================================================

#[test]
fn quarantine_clone_ignores_an_askpass_from_the_users_gitconfig() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = Remote::start();

    // A `core.askPass` in the user's own gitconfig answers `git` with no
    // terminal involved at all, so detaching the session cannot close this.
    let askpass = tmp.path().join("askpass.sh");
    std::fs::write(
        &askpass,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n  *Username*) echo {DECOY_USER} ;;\n  *) echo {DECOY_SECRET} ;;\nesac\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(
        &askpass,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();

    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--exact")
        .arg("quarantine_tty_child_helper")
        .arg("--nocapture")
        .env("WL_QTTY_URL", remote.url())
        .env("WL_QTTY_DEST", tmp.path().join("q").display().to_string())
        .env_remove("GIT_TERMINAL_PROMPT");
    for (k, v) in isolated_gitconfig(
        tmp.path(),
        &format!(
            "[user]\n\tname = test\n[core]\n\taskPass = {}\n",
            askpass.display()
        ),
    ) {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("run child");

    assert!(
        remote.requests() > 0,
        "fake remote was never contacted — this leg proved nothing.\nchild stdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        remote.credentials().is_empty(),
        "an askpass from the user's gitconfig handed credentials to an untrusted remote: {:?}",
        remote.credentials()
    );
}

// ===========================================================================
// Leg 3 — the timed-out clone must not leak its reader threads.
// ===========================================================================

/// Threads currently alive in this process. Linux-only: this is a measurement
/// of an implementation detail (two `read_to_end` threads per `run_git`), and
/// `/proc/self/task` is the only cheap honest way to count them.
#[cfg(target_os = "linux")]
fn live_threads() -> usize {
    std::fs::read_dir("/proc/self/task")
        .map(|d| d.count())
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
#[test]
fn a_timed_out_clone_does_not_leak_its_reader_threads() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = Remote::start();

    // A credential helper that BACKGROUNDS a worker is the whole point: the
    // worker inherits the write ends of our stdout/stderr pipes and outlives a
    // kill aimed at `git` alone, so the reader threads never see EOF. This is
    // the shape the issue describes, not a model of it.
    let helper = tmp.path().join("slow_helper.sh");
    std::fs::write(
        &helper,
        "#!/bin/sh\n# background worker inherits our stdout/stderr\nsleep 120 &\nsleep 120\n",
    )
    .unwrap();
    std::fs::set_permissions(
        &helper,
        <std::fs::Permissions as std::os::unix::fs::PermissionsExt>::from_mode(0o755),
    )
    .unwrap();

    let cfg = isolated_gitconfig(
        tmp.path(),
        &format!(
            "[user]\n\tname = test\n[credential]\n\thelper = {}\n",
            helper.display()
        ),
    );
    for (k, v) in &cfg {
        // SAFETY: single-threaded with respect to git config for this leg; the
        // child process below is what reads these.
        unsafe { std::env::set_var(k, v) };
    }
    // SAFETY: same.
    unsafe { std::env::set_var("WAYLAND_PLUGIN_GIT_TIMEOUT_MS", "3000") };

    let before = live_threads();
    let outcome = wcore_cli::plugin::quarantine::quarantine_clone(
        &wcore_pluginsrc::SourceKind::Url {
            url: remote.url(),
            git_ref: None,
            sha: None,
        },
        &tmp.path().join("q"),
    );

    assert!(
        remote.requests() > 0,
        "fake remote was never contacted — this leg proved nothing"
    );
    let err = outcome.expect_err("clone against a stalling helper must fail");
    assert!(
        err.to_string().contains("timed out"),
        "expected the wall-clock timeout branch, got: {err}"
    );

    // Give the reaped readers a moment to unwind. The fix kills the child's
    // whole process GROUP, so the backgrounded worker dies with it, the pipes
    // close and both threads exit; leaving the group alive is what stranded
    // them.
    let deadline = Instant::now() + Duration::from_secs(15);
    while live_threads() > before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let after = live_threads();
    assert!(
        after <= before,
        "the timed-out clone stranded {} thread(s): {before} before, {after} after",
        after.saturating_sub(before)
    );
}
