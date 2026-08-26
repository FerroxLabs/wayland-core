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
//! printed. Without both halves this file would pass on a server with no auth
//! at all.
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
fn spawn_serve(port: u16, home: &Path) -> Child {
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
    cmd.spawn().expect("spawn acp serve")
}

/// Drain the child's stderr on a thread and collect every line until the
/// server announces its bind address (or the budget expires). Returning the
/// whole transcript keeps the assertions readable when a boot fails.
fn collect_startup_lines(child: &mut Child, budget: Duration) -> Vec<String> {
    let stderr = child.stderr.take().expect("stderr piped");
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
    let key = lines
        .iter()
        .position(|l| l.contains("pass as X-API-Key header"))
        .and_then(|i| lines.get(i + 1))
        .map(|l| l.trim().to_string())
        .unwrap_or_else(|| panic!("the minted key was never printed:\n{transcript}"));
    assert_eq!(key.len(), 64, "expected a 64-char hex key, got {key:?}");
    assert!(
        std::fs::read_to_string(&credentials)
            .unwrap()
            .contains(&key),
        "the printed key is not the one that was persisted"
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

    let _ = child.kill();
    let _ = child.wait();
}
