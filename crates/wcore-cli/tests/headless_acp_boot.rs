//! FerroxLabs/wayland#305 — a headless host must be able to boot `acp serve`
//! and probe it.
//!
//! Two blockers were reported by a user stuck on WSL since 2026-07, and both
//! are graded here against the REAL binary, not a unit seam:
//!
//! * **(a) the keychain hard-exit.** With no `WAYLAND_ACP_SERVER_KEY` and no
//!   writable OS keychain, `serve` used to `?` out with `keychain store
//!   failed` before it ever bound a port. It must instead mint a key, persist
//!   it to the profile's `0600` `credentials.toml`, SAY which store it chose,
//!   and serve.
//! * **(b) `/v1/health` behind auth.** A liveness probe has to answer before a
//!   credential exists, otherwise the operator cannot tell "not started" from
//!   "started, key unknown".
//!
//! The control that keeps (b) from degenerating into "auth is off" is asserted
//! in the SAME live process: `/v1/sessions` must still refuse an
//! unauthenticated caller with 401, and must accept the key the server just
//! minted. Without both halves this file would pass on a server with no auth
//! at all.
//!
//! * **(c) the key value never reaches a non-terminal stderr.** Because the
//!   server key is now per-profile, every `WAYLAND_HOME` profile mints one on
//!   first boot; a supervised run that printed it would write one live
//!   credential per profile into whatever captures stderr.
//!   [`headless_serve_boots_reports_its_key_store_and_exposes_only_health`]
//!   drives that path (stderr is a pipe) and asserts the key is ABSENT, and
//!   [`an_interactive_first_run_still_prints_the_key`] is its positive control
//!   — the same binary on a real PTY must still print it. Two legs, because a
//!   single leg cannot tell a guard from an inverted guard.
//!
//! ## Hermetic by construction
//!
//! `WAYLAND_HOME` + `HOME` point at a throwaway tempdir and the full
//! provider-credential env set is stripped, so the run can neither read nor
//! mutate the developer's real config, keys or keychain. An isolated home also
//! means the server key resolver never touches the host keychain at all
//! (`load_or_create_server_key`), which is what makes the "which store" branch
//! deterministic on Linux, macOS and Windows alike.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use tempfile::TempDir;

#[path = "support/mod.rs"]
mod support;
use support::owned_tree::OwnedTree;

/// The credentials-store slot `acp.rs` writes the server key into.
const KEY_SLOT: &str = "acp.acp-server-key";

/// Pull the server key out of a profile's `credentials.toml` without a TOML
/// dependency: the store writes one `"acp.acp-server-key" = "<hex>"` line.
fn key_from_store(credentials: &Path) -> String {
    let text = std::fs::read_to_string(credentials).expect("read credentials.toml");
    let line = text
        .lines()
        .find(|l| l.contains(KEY_SLOT))
        .unwrap_or_else(|| panic!("no {KEY_SLOT} entry in:\n{text}"));
    line.rsplit('"')
        .nth(1)
        .unwrap_or_else(|| panic!("no quoted value in {line:?}"))
        .to_string()
}

/// Path to the debug binary under test (Cargo wires this env var).
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// Mirrors `acp_gate_d012.rs::STRIPPED_PROVIDER_ENV`.
const STRIPPED_PROVIDER_ENV: &[&str] = &[
    "API_KEY",
    "ANTHROPIC_API_KEY",
    "OPENAI_API_KEY",
    "GEMINI_API_KEY",
    "GOOGLE_API_KEY",
    "OPENROUTER_API_KEY",
    "DEEPSEEK_API_KEY",
    "GROQ_API_KEY",
    "AWS_ACCESS_KEY_ID",
    "AWS_SECRET_ACCESS_KEY",
    "AWS_SESSION_TOKEN",
    "AWS_PROFILE",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "VERTEX_PROJECT",
    "VERTEX_LOCATION",
    "GOOGLE_APPLICATION_CREDENTIALS",
];

/// Reserve a free loopback port, then drop the listener before the child binds.
fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind :0");
    l.local_addr().expect("local_addr").port()
}

/// A config the server can resolve without contacting anything. `base_url`
/// points at a port nothing listens on: no turn is driven in this test, and a
/// server that somehow tried would fail loudly rather than reach a real
/// provider.
fn write_config(home: &Path) {
    std::fs::write(
        home.join("config.toml"),
        "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\
         \n[providers.anthropic]\napi_key = \"harness-key-not-real\"\n\
         base_url = \"http://127.0.0.1:1\"\n",
    )
    .expect("write config.toml");
}

/// One HTTP/1.1 GET over a raw socket, so the request carries EXACTLY the
/// headers this test names — no client library can add a credential behind our
/// back and turn the 401 control green.
fn http_get(port: u16, path: &str, api_key: Option<&str>) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("read timeout");
    let auth = match api_key {
        Some(k) => format!("X-API-Key: {k}\r\n"),
        None => String::new(),
    };
    let request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n{auth}Connection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).expect("write request");
    stream.flush().expect("flush");
    let mut raw = String::new();
    stream.read_to_string(&mut raw).expect("read response");

    let status = raw
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status line in response: {raw:?}"));
    let body = raw.split("\r\n\r\n").nth(1).unwrap_or("").to_string();
    (status, body)
}

/// Spawn `acp serve` with a hermetic isolated home and NO injected server key.
///
/// Returns an owning guard, not a bare `Child`: an assertion failure anywhere
/// below used to skip the trailing `kill()` and leave the server running with
/// `PPID 1`, still holding its port (FerroxLabs/wayland#1156).
fn spawn_serve(port: u16, home: &Path) -> OwnedTree<Child> {
    let bind = format!("127.0.0.1:{port}");
    let mut cmd = Command::new(binary());
    cmd.args(["acp", "serve", "--bind", &bind])
        .current_dir(home)
        .env("WAYLAND_HOME", home)
        .env("HOME", home)
        .env("TERM", "dumb")
        .env_remove("WAYLAND_ACP_SERVER_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for k in STRIPPED_PROVIDER_ENV {
        cmd.env_remove(k);
    }
    OwnedTree::new(cmd.spawn().expect("spawn acp serve"))
}

/// Drain the child's stderr on a thread and collect every line until the
/// server announces its bind address (or the budget expires). Returning the
/// whole transcript keeps the assertions readable when a boot fails.
fn collect_startup_lines(child: &mut OwnedTree<Child>, budget: Duration) -> Vec<String> {
    let stderr = child.child_mut().stderr.take().expect("stderr piped");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let mut lines = Vec::new();
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let serving = line.contains("serving on http://");
                lines.push(line);
                if serving {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    lines
}

#[test]
fn headless_serve_boots_reports_its_key_store_and_exposes_only_health() {
    let home = TempDir::new().expect("tempdir");
    write_config(home.path());
    let port = free_port();
    let mut child = spawn_serve(port, home.path());

    let lines = collect_startup_lines(&mut child, Duration::from_secs(60));
    let transcript = lines.join("\n");

    // ── (a) it BOOTS. On a host with no keychain this used to be a hard exit.
    assert!(
        lines.iter().any(|l| l.contains("serving on http://")),
        "acp serve never bound a port; stderr was:\n{transcript}"
    );
    assert!(
        !transcript.contains("keychain store failed"),
        "the keychain hard-exit is back:\n{transcript}"
    );

    // ── (a) it SAYS which store it chose, and it is the profile's own file.
    let credentials = home.path().join("credentials.toml");
    let report = lines
        .iter()
        .find(|l| l.contains("server API key store:"))
        .unwrap_or_else(|| panic!("no key-store report in stderr:\n{transcript}"));
    assert!(
        report.contains(&credentials.display().to_string()),
        "the report must name the store it used; got: {report}"
    );

    // ── (a) the key really is persisted, owner-only, inside THIS profile.
    assert!(
        credentials.exists(),
        "the fallback store was reported but not written:\n{transcript}"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&credentials)
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600, "the fallback store must be owner-only");
    }
    let key = key_from_store(&credentials);
    assert_eq!(key.len(), 64, "expected a 64-char hex key, got {key:?}");

    // ── (c) THE GUARD. stderr here is a PIPE, not a terminal — the supervised
    // shape. The key value must appear NOWHERE in it, and the operator must be
    // told where to read it instead.
    assert!(
        !transcript.contains(&key),
        "the server key was written to a non-terminal stderr, where it outlives \
         the process and is readable by anything that can read the log:\n{transcript}"
    );
    assert!(
        transcript.contains("stderr is NOT a terminal"),
        "a suppressed key must say WHY it is suppressed:\n{transcript}"
    );
    assert!(
        transcript.contains(KEY_SLOT) && transcript.contains("WAYLAND_ACP_SERVER_KEY"),
        "a suppressed key must point at where to read it back:\n{transcript}"
    );
    // The orphaned shared keychain entry is named rather than deleted.
    assert!(
        transcript.contains("no longer uses the SHARED OS keychain entry")
            && transcript.contains("left in place"),
        "the now-unused shared keychain entry must be named, not silently \
         deleted and not silently abandoned:\n{transcript}"
    );

    // ── (b) liveness answers with NO credential.
    let (status, body) = http_get(port, "/v1/health", None);
    assert_eq!(
        status, 200,
        "/v1/health must answer before a credential exists; body: {body}"
    );
    assert!(
        body.contains("\"status\":\"ok\""),
        "unexpected health body: {body}"
    );

    // ── CONTROL: the carve-out did not open the rest of the surface.
    let (status, _) = http_get(port, "/v1/sessions", None);
    assert_eq!(
        status, 401,
        "an unauthenticated caller must still be refused everywhere but health"
    );

    // ── and the key the server just minted is the one that works.
    let (status, _) = http_get(port, "/v1/sessions", Some(&key));
    assert_eq!(
        status, 200,
        "the persisted key must authenticate against the running server"
    );
}

/// POSITIVE CONTROL for the terminal guard, on the real binary.
///
/// The pipe-driven test above asserts the key is absent. On its own that
/// passes just as well against a build that never announces a key at all, and
/// against one whose condition is INVERTED. This leg drives the same binary on
/// a real pseudo-terminal and asserts the key IS printed and IS the persisted
/// one, so the pair fails in one direction or the other for every way the
/// guard can be wrong.
///
/// Reads the master side RAW rather than through the shared vt100 `Pty`
/// harness: that harness renders into a 120-column grid, which would wrap a
/// 64-character key across two rows and make an exact-match assertion depend
/// on terminal width rather than on the guard.
///
/// `#![cfg(unix)]`-style gate for the same reason every other PTY test here
/// carries one: `portable_pty`'s ConPTY backend does not surface the child's
/// output to the master end on a headless Windows runner. The Windows terminal
/// leg of this behaviour is NOT measured here; the pure `new_key_notice` unit
/// tests cover the decision cross-platform.
#[cfg(unix)]
#[test]
fn an_interactive_first_run_still_prints_the_key() {
    use std::io::Read;

    use portable_pty::{CommandBuilder, PtySize, native_pty_system};

    let home = TempDir::new().expect("tempdir");
    write_config(home.path());
    let port = free_port();

    let pty = native_pty_system()
        .openpty(PtySize {
            rows: 40,
            cols: 200,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open PTY");

    let mut cmd = CommandBuilder::new(binary());
    for arg in ["acp", "serve", "--bind", &format!("127.0.0.1:{port}")] {
        cmd.arg(arg);
    }
    cmd.cwd(home.path());
    cmd.env("WAYLAND_HOME", home.path());
    cmd.env("HOME", home.path());
    cmd.env("TERM", "xterm-256color");
    cmd.env_remove("WAYLAND_ACP_SERVER_KEY");
    for k in STRIPPED_PROVIDER_ENV {
        cmd.env_remove(k);
    }
    let _child = OwnedTree::new(
        pty.slave
            .spawn_command(cmd)
            .expect("spawn acp serve on a PTY"),
    );

    let mut reader = pty.master.try_clone_reader().expect("clone PTY reader");
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });

    let mut out = String::new();
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                out.push_str(&String::from_utf8_lossy(&chunk));
                if out.contains("serving on http://") {
                    break;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    let credentials = home.path().join("credentials.toml");
    assert!(
        out.contains("serving on http://"),
        "acp serve never bound a port on a PTY; output was:\n{out}"
    );
    let key = key_from_store(&credentials);
    assert!(
        out.contains(&key),
        "an interactive first run must still print the key it minted; output \
         was:\n{out}"
    );
    assert!(
        out.contains("pass as X-API-Key header"),
        "the interactive wording must be unchanged:\n{out}"
    );
    assert!(
        !out.contains("stderr is NOT a terminal"),
        "the suppressed-key notice must not appear on a real terminal:\n{out}"
    );
}
