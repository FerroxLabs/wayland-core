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
//! `killpg`). On Windows there was none — the code carried comments claiming a
//! Job Object that did not exist — and these tests are the ones that go from
//! red to green.
//!
//! # Their power is NOT uniform across Unix — the macOS arm is vacuous
//!
//! "Unix" is two different measurements here, because the grandchild these
//! tests assert on only exists on one of them. The launch path is
//! `mcp_stdio_command_builder` -> `sh -c "<program> <args>"`, and the inner
//! command is a single simple command:
//!
//! * **macOS**: `/bin/sh` EXEC-REPLACES itself with the inner command, so the
//!   "grandchild" IS the direct child (measured on Darwin 25.3.0:
//!   `/bin/sh -c "/bin/sleep 4"` reported pid 96378, and pid 96378 was
//!   `/bin/sleep`). `child.start_kill()` alone reaps it, so BOTH tests below
//!   pass with the process-group reaper removed entirely. On macOS they are a
//!   smoke test of the fixture, not a guard on the reaper.
//! * **Linux**: `dash` FORKS (measured: `/bin/sh -c "/bin/sleep 4"` reported
//!   pid 1737448 as `sh`, with a separate `sleep` at pid 1737450, ppid
//!   1737448). The grandchild is real, and so is what these tests grade.
//!
//! So the real coverage is Linux and Windows. Do not read a green macOS run as
//! evidence the reaper works, and do not "simplify" the launch path on the
//! strength of one.
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

/// A transport for a program that does not exist must not stay LIVE.
///
/// Renamed from `a_command_that_cannot_start_is_an_error_not_a_silent_orphan`,
/// which claimed to grade "a transport that failed to take ownership of its
/// tree". It never did: a nonexistent program is reported by the shell shim,
/// which starts fine, so `attach` is reached and SUCCEEDS and no ownership
/// failure ever occurs. That path is graded where it lives, by the `attach`
/// unit tests in `wcore_types::job_object` — the `Err` return here has no
/// bearing on it.
///
/// It was also vacuous: the whole body sat inside `if let Ok(transport)`, so
/// an `Err` passed the test having asserted nothing. The spawn result is
/// asserted directly now. If a platform ever starts refusing the spawn
/// outright, this fails loudly and gets re-graded rather than going quiet.
#[tokio::test]
async fn a_nonexistent_program_never_yields_a_live_transport() {
    let missing = if cfg!(windows) {
        "wcore-mcp-no-such-program.exe"
    } else {
        "wcore-mcp-no-such-program"
    };

    // The shell wrapper reports "command not found" on its own stderr and
    // exits, so the spawn itself succeeds on every platform this runs on and
    // the failure surfaces as a transport that goes dead.
    let transport = StdioTransport::spawn(missing, &[], &HashMap::new())
        .await
        .expect("the shell shim starts even when the program it names does not");

    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while transport.is_alive() && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        !transport.is_alive(),
        "a transport for a nonexistent program reported itself alive"
    );
}
