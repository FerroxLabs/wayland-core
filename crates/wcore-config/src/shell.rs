//! Cross-platform shell / process helpers.
//!
//! Two execution modes are provided. **New callers must use argv mode**:
//!
//! - **Argv mode** ([`shell_command_argv`]): spawn `program` directly with
//!   a list of arguments, each `Command::arg`-passed. NO shell interpreter
//!   is invoked, so shell metacharacters (`;`, `&&`, `|`, `$()`, backticks,
//!   redirection, glob expansion) in the args are NEVER interpreted. This
//!   is the only safe mode for LLM-supplied parameters.
//!
//! - **Shell-string mode** ([`shell_command_builder`] / [`shell_command`]):
//!   run `sh -c <str>` on Unix and `cmd /C <str>` on Windows. Shell
//!   metacharacters in the string ARE interpreted. Use only when the
//!   caller genuinely requires shell semantics (`&&`, pipes, redirection,
//!   shim resolution like the MCP stdio transport's `.cmd` lookup). NEVER
//!   `format!`-interpolate LLM-supplied data into the string — every such
//!   site is a shell injection.
//!
//! See `AGENTS.md` "Shell Execution" for the policy and migration guidance.

use std::process::Output;

use tokio::process::Command;

pub struct ShellInfo {
    pub program: &'static str,
    pub flag: &'static str,
}

pub fn shell_info() -> ShellInfo {
    if cfg!(windows) {
        ShellInfo {
            program: "cmd",
            flag: "/C",
        }
    } else {
        ShellInfo {
            program: "sh",
            flag: "-c",
        }
    }
}

/// Shell-string mode: run `sh -c <str>` (Unix) / `cmd /C <str>` (Windows).
///
/// Returns an unstarted `tokio::process::Command` so callers can attach
/// env, cwd, stdio, etc.
///
/// **Wave RA — RELIABILITY BLOCKER #1.** `.kill_on_drop(true)` is applied
/// here by default. Tokio's `Command` otherwise leaks the child process
/// when the Command future is dropped (e.g. when the calling tool's
/// `tokio::select!` against `ctx.cancel.cancelled()` wins the race). With
/// this flag set, dropping the future signals the kernel to SIGKILL the
/// child so subprocess cancellation actually frees CPU/memory.
///
/// **Do not interpolate LLM-supplied input into `command_str`** — that is
/// a shell injection. New callers should prefer [`shell_command_argv`].
pub fn shell_command_builder(command_str: &str) -> Command {
    let info = shell_info();
    let mut cmd = Command::new(info.program);
    cmd.arg(info.flag).arg(command_str);
    cmd.kill_on_drop(true);
    cmd
}

/// Shell-string mode one-shot: builds via [`shell_command_builder`] and
/// awaits `output()`. Inherits all the safety caveats of that helper.
pub async fn shell_command(command_str: &str) -> std::io::Result<Output> {
    shell_command_builder(command_str).output().await
}

/// Argv mode: spawn `program` directly with each `arg` passed as a
/// separate process-arg. No shell interpreter is invoked, so shell
/// metacharacters in `args` are NEVER interpreted by a shell. The OS
/// `execvp`/`CreateProcess` resolves `program` against `PATH` (and
/// `PATHEXT` on Windows, which makes `.exe`/`.cmd`/`.bat` shims work
/// transparently for binaries like `git`).
///
/// Returns an unstarted `tokio::process::Command` so callers can:
/// - attach env via `.env(...)` / `.env_clear()`
/// - set working directory via `.current_dir(...)`
/// - configure stdio via `.stdout(...)` / `.stderr(...)`
///
/// **Wave RA — RELIABILITY BLOCKER #1.** `.kill_on_drop(true)` is applied
/// here by default. Tokio's `Command` otherwise leaves the child running
/// when the Command future is dropped (e.g. when the calling tool's
/// `tokio::select!` against `ctx.cancel.cancelled()` wins the race),
/// producing zombie subprocesses that the agent reports as "cancelled"
/// while they keep consuming CPU. With this flag set, dropping the
/// future signals the kernel to SIGKILL the child.
///
/// This is the only safe mode for any command whose arguments include
/// LLM-supplied data.
///
/// # Example
///
/// ```no_run
/// # use wcore_config::shell::shell_command_argv;
/// # tokio_test::block_on(async {
/// let output = shell_command_argv("git", &["status", "--porcelain=v1"])
///     .current_dir("/tmp/repo")
///     .output()
///     .await
///     .unwrap();
/// # });
/// ```
pub fn shell_command_argv(program: &str, args: &[&str]) -> Command {
    let mut cmd = Command::new(program);
    cmd.args(args);
    cmd.kill_on_drop(true);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_info_returns_platform_appropriate_values() {
        let info = shell_info();
        if cfg!(windows) {
            assert_eq!(info.program, "cmd");
            assert_eq!(info.flag, "/C");
        } else {
            assert_eq!(info.program, "sh");
            assert_eq!(info.flag, "-c");
        }
    }

    #[tokio::test]
    async fn shell_command_runs_echo() {
        let output = shell_command("echo hello")
            .await
            .expect("shell_command failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("hello"));
    }

    #[tokio::test]
    async fn shell_command_builder_allows_env_and_cwd() {
        let tmp = std::env::temp_dir();
        let cmd_str = if cfg!(windows) {
            "echo %MY_VAR%"
        } else {
            "echo $MY_VAR"
        };
        let output = shell_command_builder(cmd_str)
            .env("MY_VAR", "test_value")
            .current_dir(&tmp)
            .output()
            .await
            .expect("builder failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("test_value"));
    }

    /// Argv mode: shell metacharacters in args are passed literally to the
    /// program, NOT interpreted by any shell. This is the load-bearing
    /// invariant for shell injection eradication (Wave SA).
    #[tokio::test]
    async fn shell_command_argv_does_not_interpret_metacharacters() {
        // Echo a string containing `; rm -rf /` literally. If a shell were
        // wrapping this, the `;` would terminate the echo and try to run
        // `rm`. In argv mode, the whole string is one arg to echo, which
        // prints it back verbatim.
        let payload = "hello; rm -rf /";
        let output = shell_command_argv("echo", &[payload])
            .output()
            .await
            .expect("argv echo failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(payload),
            "expected literal payload echoed; got {stdout:?}"
        );
        // And of course no filesystem damage occurred — we're alive.
    }

    /// Argv mode: command-substitution syntax `$()` is NOT evaluated.
    #[tokio::test]
    async fn shell_command_argv_does_not_evaluate_command_substitution() {
        // In a shell, `echo $(whoami)` would print the user. In argv mode,
        // echo receives `$(whoami)` as a literal arg.
        let payload = "$(whoami)";
        let output = shell_command_argv("echo", &[payload])
            .output()
            .await
            .expect("argv echo failed");
        let stdout = String::from_utf8_lossy(&output.stdout);
        // The literal must appear; the resolved username must NOT appear
        // (we don't try to match the username, just assert the literal
        // `$(whoami)` survived).
        assert!(stdout.contains("$(whoami)"), "got {stdout:?}");
    }

    /// Argv mode resolves the program against `PATH` / `PATHEXT`, so a
    /// portable binary like `git --version` works without a shell.
    #[tokio::test]
    async fn shell_command_argv_resolves_path_for_git() {
        let output = shell_command_argv("git", &["--version"])
            .output()
            .await
            .expect("git --version failed");
        // git --version prints `git version X.Y.Z`. Either platform.
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.starts_with("git version"),
            "expected `git version ...`, got {stdout:?}"
        );
    }
}
