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
use std::sync::OnceLock;

use tokio::process::Command;

/// Process-global Bash-tool shell override, sourced from `[tools] windows_shell`
/// in config and set once at boot by the host via [`set_bash_shell_config`].
/// Read by [`bash_shell_argv_prefix`]; the `WAYLAND_BASH_SHELL` env var takes
/// precedence over it. A process-global (rather than a `BashTool` field) keeps
/// the choice in one place: it is the same for every `BashTool` instance and
/// every spawned sub-agent in the process, and avoids threading config through
/// the tool factories.
static BASH_SHELL_CONFIG: OnceLock<Option<String>> = OnceLock::new();

/// Record the configured Bash-tool shell (`[tools] windows_shell`). Call once at
/// boot before any Bash command runs. Idempotent — the first call wins, so a
/// re-bootstrap in the same process cannot flip the shell mid-session.
pub fn set_bash_shell_config(value: Option<String>) {
    let _ = BASH_SHELL_CONFIG.set(value);
}

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

/// Shell argv prefix for the agent **Bash tool**, honoring an optional
/// Windows PowerShell override.
///
/// Returns the program + flag(s) that precede the command string in the
/// BashTool argv: `["sh", "-c"]` on Unix and `["cmd", "/C"]` on Windows by
/// default. On Windows ONLY, the interpreter can be switched to `powershell` →
/// `["powershell", "-NoProfile", "-Command"]` (Windows PowerShell 5.1, always
/// present) or `pwsh` → the same with `pwsh` (PowerShell 7+, if installed). Any
/// other value falls back to `cmd /C`.
///
/// The choice resolves with precedence **`WAYLAND_BASH_SHELL` env (runtime
/// override) > `[tools] windows_shell` config > default `cmd`**. The config key
/// is the path the desktop app writes; the env var is the runtime escape hatch.
///
/// Scope is deliberately the BashTool only — the hook, MCP-stdio, and skill
/// shell paths keep `cmd /C` so their established quoting/shim contracts are
/// unchanged. This does not alter the injection surface: BashTool already
/// runs an LLM-supplied string through a shell interpreter (its contract);
/// this only chooses which interpreter, and the denylist + sandbox still
/// apply to the resulting argv. See issue: PowerShell-on-Windows request.
pub fn bash_shell_argv_prefix() -> Vec<String> {
    let choice = std::env::var("WAYLAND_BASH_SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| BASH_SHELL_CONFIG.get().cloned().flatten());
    bash_shell_prefix_for(cfg!(windows), choice.as_deref())
}

/// Pure core of [`bash_shell_argv_prefix`], split out so every branch —
/// including the Windows/PowerShell ones — is unit-testable on any host.
fn bash_shell_prefix_for(is_windows: bool, win_shell: Option<&str>) -> Vec<String> {
    if !is_windows {
        return vec!["sh".to_string(), "-c".to_string()];
    }
    match win_shell.map(normalize_win_shell).as_deref() {
        Some("powershell") => vec![
            "powershell".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
        ],
        Some("pwsh") => vec![
            "pwsh".to_string(),
            "-NoProfile".to_string(),
            "-Command".to_string(),
        ],
        _ => vec!["cmd".to_string(), "/C".to_string()],
    }
}

/// Normalize a `WAYLAND_BASH_SHELL` / `[tools] windows_shell` value to its
/// lowercased program stem so the selector accepts not only `pwsh` / `powershell`
/// but also `pwsh.exe`, `powershell.exe`, and absolute or relative paths such as
/// `C:\Program Files\PowerShell\7\pwsh.exe` (FerroxLabs/wayland#197 — the selector
/// was previously an exact, length-only match that silently fell back to `cmd /C`
/// for any of these, which also contradicted the sandbox allowlist that *requires*
/// the `.exe`/absolute form). Splits on both `/` and `\` regardless of host OS —
/// this runs on Windows in production but is unit-tested on every platform — then
/// strips a trailing `.exe`.
fn normalize_win_shell(s: &str) -> String {
    let base = s.trim().rsplit(['/', '\\']).next().unwrap_or("");
    let base = base.to_ascii_lowercase();
    base.strip_suffix(".exe").unwrap_or(&base).to_string()
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

/// Shell-string mode for **hook commands**, which reference hook variables as
/// `${VAR}` and expect them expanded from the environment.
///
/// Identical to [`shell_command_builder`] except that on Windows it enables
/// delayed expansion (`cmd /V:ON /C`) so a hook author's `!VAR!` reference is
/// expanded at execution time WITHOUT the shell re-parsing the (model-derived)
/// value for metacharacters. On Unix, `sh -c` expands `${VAR}` from the
/// environment safely (parameter expansion is not re-evaluated for command
/// substitution). Either way, callers MUST pass hook values via `.envs(...)`
/// and never interpolate a value into `command_str` — see the hook runner,
/// which translates `${VAR}` to the platform-native safe reference.
pub fn hook_shell_command_builder(command_str: &str) -> Command {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.arg("/V:ON").arg("/C").arg(command_str);
        c
    } else {
        let info = shell_info();
        let mut c = Command::new(info.program);
        c.arg(info.flag).arg(command_str);
        c
    };
    cmd.kill_on_drop(true);
    cmd
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

    #[test]
    fn bash_prefix_unix_is_sh_dash_c_regardless_of_env() {
        // The PowerShell override is Windows-only; on Unix it is ignored.
        assert_eq!(bash_shell_prefix_for(false, None), vec!["sh", "-c"]);
        assert_eq!(
            bash_shell_prefix_for(false, Some("powershell")),
            vec!["sh", "-c"]
        );
    }

    #[test]
    fn bash_prefix_windows_defaults_to_cmd() {
        assert_eq!(bash_shell_prefix_for(true, None), vec!["cmd", "/C"]);
        // An unrecognized value falls back to cmd, never an empty/invalid argv.
        assert_eq!(bash_shell_prefix_for(true, Some("bash")), vec!["cmd", "/C"]);
    }

    #[test]
    fn bash_prefix_windows_powershell_override() {
        assert_eq!(
            bash_shell_prefix_for(true, Some("powershell")),
            vec!["powershell", "-NoProfile", "-Command"]
        );
        // Case-insensitive and whitespace-tolerant.
        assert_eq!(
            bash_shell_prefix_for(true, Some("  PowerShell ")),
            vec!["powershell", "-NoProfile", "-Command"]
        );
        assert_eq!(
            bash_shell_prefix_for(true, Some("pwsh")),
            vec!["pwsh", "-NoProfile", "-Command"]
        );
    }

    #[test]
    fn bash_prefix_windows_accepts_exe_and_absolute_paths() {
        // FerroxLabs/wayland#197: the selector must accept `.exe` suffixes and
        // absolute/relative paths, not only the bare `pwsh`/`powershell` tokens —
        // previously any of these silently fell back to `cmd /C`.
        let powershell = vec!["powershell", "-NoProfile", "-Command"];
        let pwsh = vec!["pwsh", "-NoProfile", "-Command"];
        for v in [
            "pwsh.exe",
            "PWSH.EXE",
            r"C:\Program Files\PowerShell\7\pwsh.exe",
            "/usr/bin/pwsh",
        ] {
            assert_eq!(
                bash_shell_prefix_for(true, Some(v)),
                pwsh,
                "{v} should select pwsh"
            );
        }
        for v in [
            "powershell.exe",
            "PowerShell.exe",
            r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe",
        ] {
            assert_eq!(
                bash_shell_prefix_for(true, Some(v)),
                powershell,
                "{v} should select powershell"
            );
        }
        // A path to an unrelated shell still falls back to cmd (only pwsh/powershell
        // are sandbox-supported selectors).
        assert_eq!(
            bash_shell_prefix_for(true, Some(r"C:\Program Files\Git\bin\bash.exe")),
            vec!["cmd", "/C"]
        );
    }

    #[test]
    fn bash_prefix_default_branches_match_shell_info() {
        // Guard against drift between the prefix defaults and shell_info().
        let info = shell_info();
        let expected = vec![info.program.to_string(), info.flag.to_string()];
        assert_eq!(bash_shell_prefix_for(cfg!(windows), None), expected);
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
