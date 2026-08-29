//! FerroxLabs/wayland-core#338 — WHICH credential helper a quarantine `git`
//! may invoke, and why the answer has to differ by platform.
//!
//! The #338 fix denies the quarantine child a terminal, so a helper that
//! ignores every `GIT_TERMINAL_PROMPT`-shaped knob still has nothing to prompt
//! on. `setsid(2)` delivers that on unix and it is a real boundary: the child
//! cannot reacquire the parent's terminal, because `TIOCSCTTY` refuses a tty
//! that is already another session's controlling terminal.
//!
//! Windows has no such primitive, and that is MEASURED, not assumed. On
//! Windows 11 build 26200 a `DETACHED_PROCESS` child reaches the launching
//! process's own console through `AttachConsole(ATTACH_PARENT_PROCESS)`, and a
//! console-less grandchild reaches it through `AttachConsole(<launcher pid>)`
//! — both writes landed in the launcher's console screen buffer, read back
//! with `ReadConsoleOutputCharacterW`. So on Windows the terminal cannot be
//! taken away from the helper, and the only elimination left is the second of
//! the three policies #338 itself lists: do not let the helper run at all.
//!
//! This file pins that split end to end against a real `git`:
//!
//! * CONTROL — a plain `git`, with the same helper configured, DOES invoke it.
//!   Without this arm a "the helper never ran" result could come from a
//!   fixture that never reaches a credential lookup at all.
//! * LIVENESS — the loopback server counts the requests it served, so an arm
//!   that failed before `git` ever spoke HTTP cannot pass by doing nothing.
//! * unix — the helper still runs. Clearing it there would break installs from
//!   private plugin sources for no gain, and the recorded decision (Q-338c4)
//!   is to keep it.
//! * windows — the helper does NOT run, through `build_git_command`, the
//!   builder every quarantine spawn goes through.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A loopback HTTP endpoint that answers every request with `401`, which is
/// what makes `git` consult a credential helper. Counts what it served.
struct AuthChallenger {
    port: u16,
    served: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl AuthChallenger {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind loopback");
        let port = listener.local_addr().expect("local addr").port();
        listener
            .set_nonblocking(true)
            .expect("non-blocking listener");
        let served = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (s, p) = (Arc::clone(&served), Arc::clone(&stop));
        let thread = std::thread::spawn(move || {
            while !p.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut sock, _)) => {
                        let mut buf = [0u8; 2048];
                        let _ = sock.read(&mut buf);
                        let _ = sock.write_all(
                            b"HTTP/1.1 401 Unauthorized\r\n\
                              WWW-Authenticate: Basic realm=\"quarantine\"\r\n\
                              Content-Length: 0\r\n\
                              Connection: close\r\n\r\n",
                        );
                        let _ = sock.flush();
                        s.fetch_add(1, Ordering::SeqCst);
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
        });
        Self {
            port,
            served,
            stop,
            thread: Some(thread),
        }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/quarantine.git", self.port)
    }

    fn served(&self) -> usize {
        self.served.load(Ordering::SeqCst)
    }
}

impl Drop for AuthChallenger {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(mut cmd: std::process::Command) -> std::process::Output {
    cmd.stdin(std::process::Stdio::null());
    cmd.output().expect("run git")
}

#[test]
fn the_quarantine_builder_applies_this_platforms_credential_policy() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sentinel = dir.path().join("helper-ran");
    let config = dir.path().join("gitconfig");
    let missing_system = dir.path().join("no-such-system-gitconfig");
    let sh_sentinel = sentinel.display().to_string().replace('\\', "/");
    std::fs::write(
        &config,
        format!("[credential]\n\thelper = !sh -c 'echo ran >{sh_sentinel}'\n"),
    )
    .expect("write gitconfig");

    let isolate = |cmd: &mut std::process::Command| {
        cmd.env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_SYSTEM", &missing_system)
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("no_proxy", "*")
            .env("NO_PROXY", "*")
            .stdin(std::process::Stdio::null());
    };

    let server = AuthChallenger::start();
    let url = server.url();

    // ---- CONTROL: an unhardened `git` with the same config DOES call it ----
    let mut control = std::process::Command::new("git");
    control.args(["ls-remote", "--", url.as_str()]);
    isolate(&mut control);
    let control_out = run(control);
    let control_requests = server.served();
    assert!(
        control_requests > 0,
        "the control never reached the loopback endpoint, so the fixture proves nothing; \
         git said: {}",
        String::from_utf8_lossy(&control_out.stderr)
    );
    assert!(
        sentinel.exists(),
        "CONTROL FAILED: a plain `git` did not invoke the configured credential helper, so \
         this fixture cannot detect one being invoked. git stderr: {}",
        String::from_utf8_lossy(&control_out.stderr)
    );
    let _ = std::fs::remove_file(&sentinel);
    let before = server.served();

    // ---- ARM: the real quarantine builder --------------------------------
    let mut arm = wcore_cli::plugin::quarantine::build_git_command(&["ls-remote", "--", &url], None);
    isolate(&mut arm);
    let arm_out = run(arm);

    // LIVENESS: the arm must have got as far as an HTTP 401, or "no helper
    // ran" would just mean "git never asked for a credential".
    assert!(
        server.served() > before,
        "the quarantine arm never reached the loopback endpoint, so it cannot demonstrate \
         anything about credential helpers; git said: {}",
        String::from_utf8_lossy(&arm_out.stderr)
    );
    assert!(
        !arm_out.status.success(),
        "the endpoint answers 401 to everything; `git ls-remote` must not have succeeded"
    );

    #[cfg(windows)]
    assert!(
        !sentinel.exists(),
        "WINDOWS: the quarantine git invoked a third-party credential helper. Windows cannot \
         take the terminal away from that helper (measured: a DETACHED_PROCESS child, and a \
         console-less grandchild, both reach the launcher's console via AttachConsole), so a \
         helper that runs at all can raise an unattributable prompt on the user's terminal. \
         git stderr: {}",
        String::from_utf8_lossy(&arm_out.stderr)
    );

    #[cfg(unix)]
    assert!(
        sentinel.exists(),
        "UNIX: the quarantine git stopped invoking the credential helper. That is a product \
         regression, not a hardening: installs from private plugin sources need it, and \
         decision Q-338c4 keeps it precisely because setsid already denies the helper a \
         terminal. git stderr: {}",
        String::from_utf8_lossy(&arm_out.stderr)
    );
}
