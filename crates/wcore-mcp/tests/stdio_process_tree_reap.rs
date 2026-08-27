//! Closing a stdio transport must reap the SERVER, not just the shim.
//!
//! The stdio transport never launches the configured command directly: it
//! routes through `sh -c` on Unix and `cmd /C` on Windows so `.cmd`/`.bat`
//! shims and PATHEXT resolution work. That makes the real MCP server a
//! GRANDCHILD, and killing the tracked child kills only the shim. The
//! grandchild survives holding the stdout/stderr pipe write handles it
//! inherited, which is a leaked process from the user's point of view and a
//! reader that can never observe EOF from the transport's.
//!
//! Unix has had a reaper for this since Rank 24 (`process_group(0)` plus
//! `killpg`), so on Unix these tests are a regression guard on a mechanism
//! that already works. On Windows there was none — the code carried comments
//! claiming a Job Object that did not exist — and these tests are the ones
//! that go from red to green.
//!
//! Deliberately asserts against the OS, not against a transport field: the
//! claim is "no process survived", and only the OS can answer that.

use std::collections::HashMap;
use std::time::Duration;

use wcore_mcp::test_utils::mute_server;
use wcore_mcp::transport::McpTransport;
use wcore_mcp::transport::stdio::StdioTransport;

/// Not a test. This is the mute MCP server: `launch_parts` re-executes this
/// binary with `--exact` pointed at this function, and the env var switch
/// turns it into a process that publishes its pid and then says nothing
/// forever. During an ordinary run `serve_if_requested` returns at once and
/// this passes trivially.
#[test]
fn mute_server_helper() {
    mute_server::serve_if_requested();
}

/// Long enough for a debug-profile test binary to start on a cold, loaded
/// Windows runner; short enough to fail rather than hang if it never does.
const SPAWN_BUDGET: Duration = Duration::from_secs(30);
/// The tree is killed synchronously; this is slack for the OS, not a retry.
const REAP_BUDGET: Duration = Duration::from_secs(10);

async fn spawn_mute_transport(pid_file: &std::path::Path) -> (StdioTransport, u32) {
    let (command, args, env) = mute_server::launch_parts("mute_server_helper", pid_file);
    let transport = StdioTransport::spawn_with_timeout(
        &command,
        &args,
        &env,
        // The server never answers, so no request is issued at all; this only
        // has to be finite.
        Duration::from_secs(1),
    )
    .await
    .expect("mute server spawns");

    let pid = tokio::task::spawn_blocking({
        let pid_file = pid_file.to_path_buf();
        move || mute_server::wait_for_pid(&pid_file, SPAWN_BUDGET)
    })
    .await
    .expect("pid wait task");

    // Positive control. Without it, a "the process is gone" assertion below
    // would also pass for a server that never started, for a pid that was
    // never real, and for a liveness probe that answers "dead" to everything.
    assert!(
        wcore_types::process_liveness::process_is_alive(pid),
        "the mute server published pid {pid} but is not alive — the fixture, \
         not the transport, is what this test would be measuring"
    );

    (transport, pid)
}

#[tokio::test]
async fn close_reaps_the_grandchild_server_not_only_the_shim() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let pid_file = dir.path().join("server.pid");
    let (transport, pid) = spawn_mute_transport(&pid_file).await;

    transport.close().await.expect("close succeeds");

    assert!(
        mute_server::wait_until_gone(pid, REAP_BUDGET),
        "the MCP server (pid {pid}) was still alive {REAP_BUDGET:?} after close() — \
         closing the transport reaped the shell wrapper and orphaned the server"
    );
}

/// `close()` is the polite path. A session that is torn down, cancelled or
/// panics only ever runs `Drop`, and a reaper that exists on one of those two
/// paths leaks on the other.
#[tokio::test]
async fn drop_reaps_the_grandchild_server_too() {
    let dir = tempfile::TempDir::new().expect("tempdir");
    let pid_file = dir.path().join("server.pid");
    let (transport, pid) = spawn_mute_transport(&pid_file).await;

    drop(transport);

    assert!(
        mute_server::wait_until_gone(pid, REAP_BUDGET),
        "the MCP server (pid {pid}) survived the transport being dropped"
    );
}

/// The spawn contract the two tests above depend on: a transport that failed
/// to take ownership of its tree must not be handed back as if it had.
#[tokio::test]
async fn a_command_that_cannot_start_is_an_error_not_a_silent_orphan() {
    let missing = if cfg!(windows) {
        "wcore-mcp-no-such-program.exe"
    } else {
        "wcore-mcp-no-such-program"
    };
    let result = StdioTransport::spawn(missing, &[], &HashMap::new()).await;

    // The shell wrapper reports "command not found" on its own stderr and
    // exits, so the spawn itself succeeds and the failure surfaces as a dead
    // transport rather than a spawn error. Either shape is acceptable; a
    // LIVE transport for a program that does not exist is not.
    if let Ok(transport) = result {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while transport.is_alive() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(
            !transport.is_alive(),
            "a transport for a nonexistent program reported itself alive"
        );
    }
}
