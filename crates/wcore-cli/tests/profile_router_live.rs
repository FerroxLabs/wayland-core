//! PR-7 increment 2d — LIVE end-to-end test of the profile SUPERVISOR/ROUTER.
//!
//! The fail-closed unit tests in `profile_router.rs` prove the router rejects
//! bad selectors and never spawns for an unknown profile, but they never spawn
//! a real child. This test exercises the ONE topology the code is built for,
//! across THREE real processes:
//!
//! ```text
//!   test  ──AcpClient/HTTP──▶  wayland-core acp serve --enable-profile-router   (supervisor)
//!                                        │ spawns + routes
//!                                        ▼
//!                             wayland-core acp serve --profile <name>            (child)
//!                                        │ engine turn
//!                                        ▼
//!                                    MockLlm  (POST /v1/messages)
//! ```
//!
//! It asserts the load-bearing behaviour of the increment: opening a
//! `profile:<name>` session SPAWNS a dedicated child (one process per profile),
//! a turn ROUTES through the child to its own credential home + provider and
//! streams the reply back, deleting the session REAPS the child, and an unknown
//! profile FAILS CLOSED without spawning anything.
//!
//! ## Hermetic by construction
//!
//! Every process points `WAYLAND_HOME`/`HOME` at throwaway tempdirs and strips
//! the full provider-credential env set, so a run can neither read nor mutate
//! the developer's real config/keys. `WAYLAND_PROFILES_ROOT` is a throwaway
//! tempdir, so the only profile in existence is the one this test creates. The
//! mock provider scripts the turn, so no real provider is contacted.

use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use futures::StreamExt as _;
use tempfile::TempDir;

use wcore_acp::client::AcpClient;
use wcore_acp::protocol::{MessageEvent, MessageSendRequest, SessionCreateRequest};

#[path = "support/mod.rs"]
mod support;
use support::mock_llm::MockLlm;
use support::owned_tree::OwnedTree;
// The descendant walk is only consulted by the `#[cfg(unix)]` assertions below.
#[cfg(unix)]
use support::owned_tree::child_pids;

/// Path to the debug binary under test (Cargo wires this env var for
/// integration tests of the package that defines the binary).
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

/// The provider-credential env-var set every spawned process must NOT inherit,
/// so a run can neither read the developer's real keys nor auto-detect a stray
/// dev credential. Mirrors `acp_gate_d012.rs::STRIPPED_PROVIDER_ENV`.
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

/// A config.toml routing `anthropic` at `base_url` with a dummy (non-`sk-`) key.
/// The mock ignores the key value; a non-secret literal keeps the pre-push
/// secret scanner out of the picture. Used for both the supervisor home (which
/// never calls a provider for a profile session) and the profile child home
/// (which runs the turn against the mock).
fn config_toml(base_url: &str) -> String {
    format!(
        "[default]\nprovider = \"anthropic\"\nmodel = \"claude-sonnet-4-20250514\"\n\
         \n[providers.anthropic]\napi_key = \"harness-key-not-real\"\nbase_url = \"{base_url}\"\n"
    )
}

/// Reserve a free loopback port by binding `:0`, reading it, then dropping the
/// listener before the child binds it (same trick the router uses internally).
fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").expect("bind :0");
    l.local_addr().expect("local_addr").port()
}

/// Spawn the supervisor: `acp serve --enable-profile-router` on `port`, with a
/// hermetic home, the throwaway profiles root, and a known API key. Provider
/// credentials are stripped so the child's identity can only come from its own
/// profile home.
fn spawn_supervisor(port: u16, home: &Path, profiles_root: &Path, key: &str) -> OwnedTree<Child> {
    let bind = format!("127.0.0.1:{port}");
    let mut cmd = Command::new(binary());
    cmd.args(["acp", "serve", "--enable-profile-router", "--bind", &bind])
        .env("WAYLAND_HOME", home)
        .env("HOME", home)
        .env("TERM", "dumb")
        .env("WAYLAND_PROFILES_ROOT", profiles_root)
        .env("WAYLAND_ACP_SERVER_KEY", key)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    for k in STRIPPED_PROVIDER_ENV {
        cmd.env_remove(k);
    }
    OwnedTree::new(cmd.spawn().expect("spawn supervisor"))
}

/// Poll the supervisor's ACP surface until it answers (server is up + the key
/// handshake works) or the budget expires.
async fn await_supervisor(client: &AcpClient) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if client.list_sessions().await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    panic!("supervisor did not become healthy within 30s");
}

/// Wait until the supervisor has exactly `want` direct children, or the budget
/// expires; returns the last observed count. Unix only.
#[cfg(unix)]
fn await_child_count(parent: u32, want: usize, budget: Duration) -> usize {
    let deadline = Instant::now() + budget;
    let mut last = child_pids(parent).len();
    while Instant::now() < deadline {
        last = child_pids(parent).len();
        if last == want {
            return last;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    last
}

/// Create `<profiles_root>/<name>/config.toml` routing at `base_url`, so the
/// child that this profile spawns runs its turn against the mock. The profile
/// must exist BEFORE the supervisor starts: the supervisor enumerates +
/// authorizes profiles once, at startup, via `list_profiles()`.
fn seed_profile(profiles_root: &Path, name: &str, base_url: &str) {
    let dir = profiles_root.join(name);
    std::fs::create_dir_all(&dir).expect("create profile dir");
    std::fs::write(dir.join("config.toml"), config_toml(base_url)).expect("write profile config");
}

/// End-to-end: a `profile:<name>` session spawns a dedicated child, routes a
/// turn through it to the child's own provider, streams the reply back, and
/// reaps the child when the session is deleted.
#[tokio::test]
async fn profile_session_spawns_child_routes_turn_and_reaps() {
    const SENTINEL: &str = "HELLO_FROM_PROFILE_CHILD";
    let profiles_root = TempDir::new().expect("profiles root");
    let sup_home = TempDir::new().expect("supervisor home");

    // Mock provider the child's engine turn will hit.
    let mock = MockLlm::new().text(SENTINEL);
    let server = mock.start().await;

    // Seed the one profile (before the supervisor enumerates) + a minimal
    // supervisor config (it never calls a provider for a profile session, but a
    // valid config file keeps `Config::resolve` happy at startup).
    seed_profile(profiles_root.path(), "livetest", &server.uri());
    std::fs::write(
        sup_home.path().join("config.toml"),
        config_toml(&server.uri()),
    )
    .expect("write supervisor config");

    let key = "supervisor-live-test-key";
    let port = free_port();
    // Held for its `Drop`: on Windows nothing below reads it, but it still owns
    // and reaps the supervisor on every exit path (FerroxLabs/wayland#1156).
    #[cfg_attr(windows, allow(unused_variables))]
    let sup = spawn_supervisor(port, sup_home.path(), profiles_root.path(), key);

    let client = AcpClient::new(format!("http://127.0.0.1:{port}"))
        .expect("acp client")
        .with_api_key(key);
    await_supervisor(&client).await;

    // Before any profile session, the supervisor has no children.
    #[cfg(unix)]
    assert_eq!(
        child_pids(sup.id()).len(),
        0,
        "supervisor must have no profile children before any profile session"
    );

    // Open a profile session — this spawns + health-checks the dedicated child.
    let created = client
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: Some("profile:livetest".to_string()),
            mcp_servers: Vec::new(),
        })
        .await
        .expect("open profile session (child must spawn + become healthy)");
    let session_id = created.session_id;

    // A dedicated child process must now be running under the supervisor.
    #[cfg(unix)]
    assert_eq!(
        await_child_count(sup.id(), 1, Duration::from_secs(10)),
        1,
        "opening a profile session must spawn exactly one dedicated child process"
    );

    // Drive a turn: it routes supervisor -> child -> mock and streams back.
    let mut stream = client
        .send_message(MessageSendRequest {
            session_id: session_id.clone(),
            text: "hi".to_string(),
            tools: Vec::new(),
        })
        .await
        .expect("send_message to profile session");

    let mut text = String::new();
    let mut saw_done = false;
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_secs(5), stream.next()).await {
            Ok(Some(item)) => match item.expect("stream item ok") {
                MessageEvent::TextDelta { text: t } => text.push_str(&t),
                MessageEvent::Done { .. } => {
                    saw_done = true;
                    break;
                }
                MessageEvent::Error { error, .. } => {
                    panic!("profile turn errored: {}", error.message)
                }
                _ => {}
            },
            Ok(None) => break,
            Err(_) => panic!("timed out waiting for a stream frame from the profile child"),
        }
    }
    assert!(
        saw_done,
        "the routed turn must end with a terminal Done frame"
    );
    assert!(
        text.contains(SENTINEL),
        "the child's mock-provider reply must route back to the supervisor's \
         client; got text {text:?}"
    );

    // Delete the session — the router reaps the (now session-less) child.
    client
        .delete_session(&session_id)
        .await
        .expect("delete profile session");

    // The dedicated child must be gone.
    #[cfg(unix)]
    assert_eq!(
        await_child_count(sup.id(), 0, Duration::from_secs(10)),
        0,
        "deleting the last session for a profile must reap its child process"
    );
}

/// FAIL CLOSED, live: selecting an unknown profile is rejected and spawns no
/// child — the supervisor never falls through to a default identity.
#[tokio::test]
async fn unknown_profile_fails_closed_and_spawns_no_child() {
    let profiles_root = TempDir::new().expect("profiles root");
    let sup_home = TempDir::new().expect("supervisor home");
    let mock = MockLlm::new().text("unused");
    let server = mock.start().await;
    // A real, authorized profile exists — but we request a DIFFERENT one, so the
    // rejection is about the unknown selector, not an empty roster.
    seed_profile(profiles_root.path(), "livetest", &server.uri());
    std::fs::write(
        sup_home.path().join("config.toml"),
        config_toml(&server.uri()),
    )
    .expect("write supervisor config");

    let key = "supervisor-live-test-key-2";
    let port = free_port();
    // Held for its `Drop`: on Windows nothing below reads it, but it still owns
    // and reaps the supervisor on every exit path (FerroxLabs/wayland#1156).
    #[cfg_attr(windows, allow(unused_variables))]
    let sup = spawn_supervisor(port, sup_home.path(), profiles_root.path(), key);
    let client = AcpClient::new(format!("http://127.0.0.1:{port}"))
        .expect("acp client")
        .with_api_key(key);
    await_supervisor(&client).await;

    // Rejected without pinning the exact wire wording.
    let _ = client
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: Some("profile:does-not-exist".to_string()),
            mcp_servers: Vec::new(),
        })
        .await
        .expect_err("an unknown profile must be rejected");

    #[cfg(unix)]
    assert_eq!(
        await_child_count(sup.id(), 0, Duration::from_secs(3)),
        0,
        "an unknown profile must never spawn a child"
    );
}

/// Is `pid` still a live process? A zombie is NOT live for this test's purpose:
/// the supervisor is `wait`ed before the assertion, and an orphaned child
/// reparents to init, which reaps it. Unix only.
#[cfg(unix)]
fn is_running(pid: u32) -> bool {
    // SAFETY: signal 0 performs the permission + existence check only; it takes
    // no pointers and delivers nothing.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// FerroxLabs/wayland#1156 — a SIGKILLed supervisor must not leave its profile
/// children running.
///
/// # Why SIGKILL specifically
///
/// Every reaping path the router owns — the signal handler, `Drop`, the
/// per-session reap — needs the supervisor to still be executing. SIGKILL is
/// the class of death that gives it none of them, and it stands in for the
/// panic and the OOM kill that reach the child the same way. Measured before
/// the fix on Linux: the child survived with `PPID 1` and its own PGID, still
/// bound to its loopback port, and nine such orphans had accumulated on one
/// host with the oldest 24 hours old.
///
/// The child must therefore hold something the KERNEL closes when we die. It
/// does: the read end of a pipe delivered as its stdin, whose write end the
/// supervisor holds for the child's registered lifetime
/// (`wcore_cli::parent_channel`). This test fails if that stdin ever goes back
/// to `Stdio::null()`, or if the child stops parking on it.
#[cfg(unix)]
#[tokio::test]
async fn a_sigkilled_supervisor_does_not_leave_an_orphaned_profile_child() {
    let profiles_root = TempDir::new().expect("profiles root");
    let sup_home = TempDir::new().expect("supervisor home");

    // The child never takes a turn here — it only has to boot and pass the
    // supervisor's health check — but both homes still need a resolvable
    // provider config, and it must point at the mock rather than a real one.
    let mock = MockLlm::new().text("unused");
    let server = mock.start().await;
    seed_profile(profiles_root.path(), "orphantest", &server.uri());
    std::fs::write(
        sup_home.path().join("config.toml"),
        config_toml(&server.uri()),
    )
    .expect("write supervisor config");

    let key = "supervisor-live-test-key-3";
    let port = free_port();
    let mut sup = spawn_supervisor(port, sup_home.path(), profiles_root.path(), key);
    let client = AcpClient::new(format!("http://127.0.0.1:{port}"))
        .expect("acp client")
        .with_api_key(key);
    await_supervisor(&client).await;

    // Open a profile session so a dedicated child spawns and health-checks.
    client
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: Some("profile:orphantest".to_string()),
            mcp_servers: Vec::new(),
        })
        .await
        .expect("open profile session (child must spawn + become healthy)");

    let children = child_pids(sup.id());
    assert_eq!(
        children.len(),
        1,
        "expected exactly one profile child before killing the supervisor, saw {children:?}"
    );
    let child = children[0];

    // SIGKILL: no handler, no unwinding, no Drop — the supervisor gets no
    // chance to reap anything.
    //
    // `kill_direct_only` rather than the guard's full `reap`: this test's whole
    // subject is whether the CHILD dies when its parent is killed, so the
    // harness must not kill the child itself and mask the answer. The guard
    // still snapshotted the child before the kill, so its `Drop` remains a real
    // safety net if the assertion below panics.
    sup.kill_direct_only();

    let deadline = Instant::now() + Duration::from_secs(30);
    while is_running(child) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    if is_running(child) {
        // Do not leave the orphan behind just because the assertion failed —
        // that would be this very defect, planted by its own regression test.
        // SAFETY: a plain SIGKILL to a pid this test spawned.
        unsafe { libc::kill(child as libc::pid_t, libc::SIGKILL) };
        panic!(
            "profile child {child} outlived its SIGKILLed supervisor — it has been reparented to \
             init and holds this profile's credentials forever (FerroxLabs/wayland#1156)"
        );
    }
}

/// FerroxLabs/wayland#1156 — the HARNESS must own the tree it spawned, on the
/// exit path the leak actually took: a panic.
///
/// The nine orphans in the ticket were not left by tests that returned
/// normally; every site here ended with `let _ = child.kill()`, which a failing
/// assertion jumps straight over. Measured on this file before the fix: a test
/// that panicked after spawning left BOTH the supervisor (`PPID 1`, still bound
/// to its loopback port) and its profile child alive after the test binary had
/// exited.
///
/// The guard is dropped inside a caught unwind, which is the same `Drop` the
/// runtime would run on a real assertion failure, so this grades the mechanism
/// rather than the happy path. `is_running` answers TRUE for a zombie, so
/// asserting the supervisor is gone also grades the `wait()`: a guard that
/// killed without reaping would leave a zombie and fail here.
#[cfg(unix)]
#[tokio::test]
async fn a_panicking_test_body_still_reaps_the_supervisor_tree() {
    let profiles_root = TempDir::new().expect("profiles root");
    let sup_home = TempDir::new().expect("supervisor home");
    let mock = MockLlm::new().text("unused");
    let server = mock.start().await;
    seed_profile(profiles_root.path(), "panictest", &server.uri());
    std::fs::write(
        sup_home.path().join("config.toml"),
        config_toml(&server.uri()),
    )
    .expect("write supervisor config");

    let key = "supervisor-live-test-key-4";
    let port = free_port();
    let sup = spawn_supervisor(port, sup_home.path(), profiles_root.path(), key);
    let client = AcpClient::new(format!("http://127.0.0.1:{port}"))
        .expect("acp client")
        .with_api_key(key);
    await_supervisor(&client).await;

    client
        .create_session(SessionCreateRequest {
            model: None,
            tools: Vec::new(),
            system_prompt: None,
            agent: Some("profile:panictest".to_string()),
            mcp_servers: Vec::new(),
        })
        .await
        .expect("open profile session (child must spawn + become healthy)");

    let sup_pid = sup.id();
    assert_eq!(
        await_child_count(sup_pid, 1, Duration::from_secs(10)),
        1,
        "expected one profile child before the panic"
    );
    let profile_child = child_pids(sup_pid)[0];

    // The unwind the leaking sites never survived. The guard moves in, so its
    // `Drop` runs here and nowhere else.
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let _owned = sup;
        panic!("deliberate panic with the supervisor tree still running");
    }));
    assert!(panicked.is_err(), "the deliberate panic must have unwound");

    let deadline = Instant::now() + Duration::from_secs(30);
    while (is_running(sup_pid) || is_running(profile_child)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    let sup_alive = is_running(sup_pid);
    let child_alive = is_running(profile_child);
    if sup_alive || child_alive {
        // Never leave the orphan the assertion is about to report — that would
        // be this very defect, planted by its own regression test.
        // SAFETY: plain SIGKILLs to pids this test is responsible for.
        unsafe {
            libc::kill(profile_child as libc::pid_t, libc::SIGKILL);
            libc::kill(sup_pid as libc::pid_t, libc::SIGKILL);
        }
    }
    assert!(
        !sup_alive,
        "supervisor {sup_pid} outlived the panicking test that spawned it — the \
         harness does not own what it spawns (FerroxLabs/wayland#1156)"
    );
    assert!(
        !child_alive,
        "profile child {profile_child} outlived the panicking test — the harness \
         owns the leaf but not the TREE (FerroxLabs/wayland#1156)"
    );
}
