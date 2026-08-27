//! Test-only helpers, behind the `test-utils` feature.
//!
//! Nothing here compiles into a release build.

/// A real, spawnable MCP server that accepts the launch and then never speaks.
///
/// # Why re-execute the test binary
///
/// Every test that needs "a server that starts and stays mute" reached for
/// `sleep 3600`. That is not a portable fixture: on Windows `sleep` is not a
/// system command at all, it is `C:\Program Files\Git\usr\bin\sleep.exe`, so
/// the fixture silently encodes "Git for Windows is installed on this runner"
/// as a test requirement. It is also the wrong SHAPE for what these tests
/// actually grade — the production tree is `cmd /C <shim>` -> real server, and
/// the interesting process is the grandchild, whose pid nothing could observe.
///
/// Re-executing the current test binary in mute mode fixes both: it exists on
/// every platform by construction, and it reports its own pid, so a test can
/// assert the process tree was genuinely reaped rather than trusting that the
/// harness tidied up afterwards.
pub mod mute_server {
    use std::collections::HashMap;
    use std::path::Path;
    use std::time::Duration;

    /// Names the file the mute server writes its own pid into. Its presence is
    /// also the mode switch: set, the process serves; unset, it is an ordinary
    /// test run.
    pub const PID_FILE_ENV: &str = "WCORE_MCP_MUTE_SERVER_PIDFILE";

    /// Call this as the FIRST statement of a `#[test]` that exists only to be
    /// re-executed by [`launch_parts`].
    ///
    /// Returns immediately during an ordinary test run. When
    /// [`PID_FILE_ENV`] is set it writes this process's pid there and then
    /// never returns — the caller is the mute server.
    pub fn serve_if_requested() {
        let Ok(pid_file) = std::env::var(PID_FILE_ENV) else {
            return;
        };
        // Write-then-rename so a reader can never observe a half-written pid.
        let temporary = format!("{pid_file}.partial");
        if std::fs::write(&temporary, std::process::id().to_string()).is_ok() {
            let _ = std::fs::rename(&temporary, &pid_file);
        }
        loop {
            std::thread::sleep(Duration::from_secs(3600));
        }
    }

    /// The `command`, `args` and `env` for an MCP stdio server config that
    /// re-executes the current test binary as a mute server.
    ///
    /// `helper_test_name` is the full path of the `#[test]` function in THIS
    /// binary that calls [`serve_if_requested`] (for a top-level test in
    /// `tests/foo.rs`, just its name).
    pub fn launch_parts(
        helper_test_name: &str,
        pid_file: &Path,
    ) -> (String, Vec<String>, HashMap<String, String>) {
        let exe = std::env::current_exe().expect("current test executable");
        let command = exe
            .to_str()
            .expect("utf-8 test executable path")
            .to_string();
        let args = vec![
            "--exact".to_string(),
            helper_test_name.to_string(),
            "--test-threads".to_string(),
            "1".to_string(),
        ];
        // The stdio transport clears the environment and forwards an
        // allowlist, so the mode switch has to travel as a per-server `env`
        // entry — the same route an operator uses in `mcp-servers.toml`.
        let env = HashMap::from([(
            PID_FILE_ENV.to_string(),
            pid_file.to_str().expect("utf-8 pid file path").to_string(),
        )]);
        (command, args, env)
    }

    /// Block until the mute server has published its pid, or fail.
    ///
    /// Reading a pid back is the fixture's own positive control: it proves the
    /// server really launched, so a later "the process is gone" assertion
    /// cannot pass merely because nothing was ever started.
    pub fn wait_for_pid(pid_file: &Path, within: Duration) -> u32 {
        let deadline = std::time::Instant::now() + within;
        loop {
            if let Ok(raw) = std::fs::read_to_string(pid_file)
                && let Ok(pid) = raw.trim().parse::<u32>()
            {
                return pid;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "mute server never published its pid to {} within {within:?}",
                pid_file.display()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Poll until `pid` is observably dead, returning whether it got there.
    ///
    /// Uses the workspace's zombie-aware probe, so a corpse awaiting reaping
    /// counts as dead rather than as a survivor.
    pub fn wait_until_gone(pid: u32, within: Duration) -> bool {
        let deadline = std::time::Instant::now() + within;
        loop {
            if !wcore_types::process_liveness::process_is_alive(pid) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
