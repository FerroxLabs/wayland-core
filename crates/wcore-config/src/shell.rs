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

mod executable_readiness;
mod mcp_stdio_launch_context;

use std::process::Output;
use std::sync::OnceLock;

use tokio::process::Command;

pub use executable_readiness::{
    ExecutableEnvironmentVariable, ExecutableReadinessError, ExecutableReadinessLimit,
    ResolvedExecutable, resolve_mcp_stdio_executable,
};
pub use mcp_stdio_launch_context::{
    LaunchValueSource, McpStdioLaunchContext, McpStdioLaunchContextError,
};

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
    cmd.arg(info.flag);
    push_shell_command_line(&mut cmd, command_str);
    cmd.kill_on_drop(true);
    cmd
}

/// Append a shell **command line** (not an argv entry) to an already-built
/// `sh -c` / `cmd /C` invocation.
///
/// On Unix this is `Command::arg`: `sh -c` takes the whole line as a single
/// argv entry and no re-quoting happens.
///
/// On Windows it MUST be `raw_arg`. `cmd.exe` does not parse its `/C` tail
/// with `CommandLineToArgvW` — that is cmd's documented contract — but
/// `Command::arg` escapes for `CommandLineToArgvW` anyway, wrapping any line
/// containing whitespace in `"…"` and rewriting every embedded quote as `\"`.
/// cmd has no backslash escape, so it strips the outer pair and hands the
/// remaining literal `\"` to the parser. A hook or skill directive that quotes
/// a path — `type nul > "C:\dir\f"`, the everyday case as soon as a path has a
/// space in it — therefore reached cmd as `type nul > \"C:\dir\f\"` and failed
/// with *"The filename, directory name, or volume label syntax is incorrect"*.
/// Since a failing PreToolUse hook BLOCKS the call, that error surfaced to the
/// operator as every tool call in the session being refused (hosted-Windows
/// run 30727017681).
///
/// This is the same defect [`mcp_stdio_command_builder`] closed for the MCP
/// transport in #262/#263; it was never a transport-specific problem, so the
/// `raw_arg` call now lives here and every shell-string builder shares it.
///
/// `raw_arg` is `cfg(windows)`-gated in tokio, so the branch must be a
/// compile-time `#[cfg]`, not a runtime `cfg!(windows)`.
fn push_shell_command_line(cmd: &mut Command, command_line: &str) {
    #[cfg(windows)]
    {
        cmd.raw_arg(command_line);
    }
    #[cfg(not(windows))]
    {
        cmd.arg(command_line);
    }
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
        c.arg("/V:ON").arg("/C");
        c
    } else {
        let info = shell_info();
        let mut c = Command::new(info.program);
        c.arg(info.flag);
        c
    };
    push_shell_command_line(&mut cmd, command_str);
    cmd.kill_on_drop(true);
    cmd
}

/// Shell-string mode for the **MCP stdio transport**, whose caller assembles a
/// command line that is ALREADY escaped for the target shell's parser.
///
/// PRECONDITION (load-bearing): on Windows, every token in `command_line` MUST
/// already be escaped for cmd.exe's parser by the MCP transport
/// (`windows_program_token` for the program, `windows_cmd_quote` for each arg).
/// The MCP stdio transport is the ONLY sanctioned caller — do not reuse this
/// for un-escaped input.
///
/// It is otherwise the same construction as [`shell_command_builder`] — both
/// append the command line through [`push_shell_command_line`], which is
/// `raw_arg` on Windows. That was originally a transport-local workaround for
/// issues #262 and #263 (std's `CommandLineToArgvW` quoting layered ON TOP of
/// the caller's cmd.exe escaping wraps the spaced line in an outer `"..."`
/// where cmd ignores the caret/`\"` escaping, so the child receives a split
/// fragment: `"C:\Program` for a spaced program path, `pkg^"` for an npx/uvx
/// arg). The same layering broke every quoting hook and skill directive, so
/// the fix now lives in one place; what remains specific to this builder is
/// the PRECONDITION above, not the spawn mechanics.
pub fn mcp_stdio_command_builder(command_line: &str) -> Command {
    let info = shell_info();
    let mut cmd = Command::new(info.program);
    cmd.arg(info.flag);
    // Append the pre-escaped line LITERALLY so std's CommandLineToArgvW quoting
    // is not layered on top of the caller's caret/quote escaping (#262/#263).
    push_shell_command_line(&mut cmd, command_line);
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
    push_argv(&mut cmd, program, args);
    cmd.kill_on_drop(true);
    cmd
}

/// True when `program` names `cmd.exe` as its exact FINAL PATH COMPONENT
/// (case-insensitive, `.exe` optional).
///
/// Equality on the final component — not a suffix test — so `notcmd.exe` and
/// `foocmd` do not match. Both `\` and `/` count as separators, which is what
/// the Windows path parser does. Compiled on every target so the rule stays
/// unit-testable from a Linux or macOS CI job.
fn program_is_cmd(program: &str) -> bool {
    let lowered = program.to_ascii_lowercase();
    // `std::path::Path` only treats `/` as a separator when compiled for
    // Windows, and this fn compiles everywhere, so split explicitly.
    let final_component = lowered.rsplit(['\\', '/']).next().unwrap_or(&lowered);
    final_component == "cmd" || final_component == "cmd.exe"
}

/// Index within `args` of the `cmd /C` (or `/K`) payload, if this argv is a
/// `cmd.exe` invocation that has one. `args` excludes the program itself.
///
/// Returns `None` for every other shape — a non-`cmd` program, a `cmd`
/// invocation with no `/c`/`/k`, or a `/c` that is the last entry — so
/// ordinary argv keeps ordinary argv handling.
fn cmd_payload_index(program: &str, args: &[&str]) -> Option<usize> {
    if !program_is_cmd(program) {
        return None;
    }
    let flag_idx = args.iter().position(|a| {
        let flag = a.to_ascii_lowercase();
        flag == "/c" || flag == "/k"
    })?;
    let payload_idx = flag_idx + 1;
    (payload_idx < args.len()).then_some(payload_idx)
}

/// Append `args`, delivering a `cmd.exe` `/C`/`/K` payload the way cmd reads it.
///
/// ARGV MODE IS NOT ARGV FOR CMD'S PAYLOAD. Every Windows process receives one
/// command-line string; `Command::arg` builds it with the MSVC-CRT /
/// `CommandLineToArgvW` rules, which rewrite an embedded `"` as `\"`. `cmd.exe`
/// does not parse its `/C` tail that way — it strips the outer quote pair and
/// runs the remainder verbatim, with no backslash processing — so the `\` std
/// inserted as an escape survives into the command cmd actually executes.
///
/// Measured on hosted-Windows CI run 31507873209: a goal worker argv of
/// `["cmd", "/c", "echo %T% > \"%SINK%\\c.%RANDOM%\" & exit 91"]` reached cmd as
/// `echo %T% > \"C:\...\c.123\" & exit 91`, whose redirect target begins with a
/// literal `\"`. cmd answered *"The filename, directory name, or volume label
/// syntax is incorrect"*, wrote no file, and still ran the trailing `exit 91` —
/// so the worker reported a clean declared-no-effect exit for an effect that
/// never happened. That silent-wrong-answer shape is why this is corrected in
/// the helper rather than at each call site.
///
/// This is the same defect [`push_shell_command_line`] closed for the
/// shell-STRING builders (#262/#263); `shell_command_argv` was the remaining
/// hole, because an argv whose program is `cmd` still carries a shell payload.
/// Quoting layer only: the payload is still exactly the one entry the caller
/// supplied, never a `format!`-interpolated shell string, so argv discipline
/// and the injection boundary are unchanged.
///
/// KNOWN REMAINING GAP: a payload containing CR/LF still truncates at the first
/// line break, because cmd stops reading there.
/// `wcore_sandbox::backends::windows_cmdline::reject_undeliverable_cmd_payload`
/// refuses that case on the sandbox exec path; refusing here would have to
/// change this fn's signature to `Result`, so it is tracked separately rather
/// than half-done. The two cmd payload rules should be consolidated into this
/// crate once that lands.
fn push_argv(cmd: &mut Command, program: &str, args: &[&str]) {
    let payload_idx = cmd_payload_index(program, args);
    if payload_idx.is_none() {
        cmd.args(args);
        return;
    }
    for (idx, arg) in args.iter().enumerate() {
        if Some(idx) == payload_idx {
            push_cmd_payload(cmd, arg);
        } else {
            cmd.arg(arg);
        }
    }
}

/// Append a `cmd /C` payload as ONE outer double-quote pair with the payload's
/// own quotes passed through untouched — the spelling cmd strips exactly.
///
/// `raw_arg` is `cfg(windows)`-gated in tokio, so the branch must be a
/// compile-time `#[cfg]`, not a runtime `cfg!(windows)`. Off Windows this shape
/// is unreachable in production (nothing spawns `cmd`), and the plain `arg`
/// keeps the non-Windows build honest rather than silently adding quotes.
fn push_cmd_payload(cmd: &mut Command, payload: &str) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let mut quoted = String::with_capacity(payload.len() + 2);
        quoted.push('"');
        quoted.push_str(payload);
        quoted.push('"');
        cmd.as_std_mut().raw_arg(quoted);
    }
    #[cfg(not(windows))]
    {
        cmd.arg(payload);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `cmd /C` payload rule fires on exactly the argv shape that carries a
    /// shell payload, and on nothing else. If this ever matched an ordinary
    /// program, that program's args would lose their CRT escaping.
    #[test]
    fn cmd_payload_is_recognised_only_in_a_real_cmd_invocation() {
        // Recognised: bare, .exe, cased, path-qualified, both separators, /k.
        assert_eq!(cmd_payload_index("cmd", &["/c", "echo hi"]), Some(1));
        assert_eq!(cmd_payload_index("CMD.EXE", &["/C", "echo hi"]), Some(1));
        // A program literally named "/k" is not cmd, flag position notwithstanding.
        assert!(cmd_payload_index("/k", &["/k", "echo hi"]).is_none());
        assert_eq!(cmd_payload_index("cmd", &["/k", "echo hi"]), Some(1));
        assert_eq!(
            cmd_payload_index(r"C:\Windows\System32\cmd.exe", &["/c", "echo hi"]),
            Some(1)
        );
        assert_eq!(
            cmd_payload_index("C:/Windows/cmd.exe", &["/c", "echo hi"]),
            Some(1)
        );

        // NOT recognised — these must keep ordinary argv escaping.
        assert_eq!(cmd_payload_index("git", &["/c", "echo hi"]), None);
        assert_eq!(cmd_payload_index("notcmd.exe", &["/c", "echo hi"]), None);
        assert_eq!(cmd_payload_index("foocmd", &["/c", "echo hi"]), None);
        // A cmd invocation with no /c or /k carries no payload.
        assert_eq!(cmd_payload_index("cmd", &["/q", "echo hi"]), None);
        // A trailing /c has nothing after it — must not index past the end.
        assert_eq!(cmd_payload_index("cmd", &["/c"]), None);
        assert_eq!(cmd_payload_index("cmd", &[]), None);
    }

    /// The final-component equality that keeps `notcmd.exe` out.
    #[test]
    fn program_is_cmd_matches_the_final_component_only() {
        assert!(program_is_cmd("cmd"));
        assert!(program_is_cmd("cmd.exe"));
        assert!(program_is_cmd("CmD.ExE"));
        assert!(program_is_cmd(r"C:\Windows\System32\cmd.exe"));
        assert!(!program_is_cmd("notcmd.exe"));
        assert!(!program_is_cmd("cmdlet"));
        assert!(!program_is_cmd(r"C:\bin\cmd.bat"));
        assert!(!program_is_cmd(""));
    }

    /// THE PROPERTY the CI failure was: argv mode must not corrupt a quoted
    /// redirect target in a cmd payload. Off Windows there is no raw command
    /// line to inspect, so this asserts the classification that drives it —
    /// the payload is the entry the caller supplied, at the index the rule
    /// found, and the surrounding args are untouched.
    #[test]
    fn a_quoted_redirect_target_stays_the_callers_payload() {
        let payload = r#"echo %T% > "%SINK%\c.%RANDOM%" & exit 91"#;
        let args = ["/c", payload];
        let idx = cmd_payload_index("cmd", &args).expect("cmd /c payload must be found");
        assert_eq!(
            args[idx], payload,
            "the payload must reach the spawn layer byte-identical to what the caller wrote"
        );
        // The flag itself is an ordinary arg and must NOT be raw-appended.
        assert_ne!(idx, 0);
    }

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

    /// Shell-string mode must hand the interpreter the command line the
    /// caller wrote, **quotes intact**.
    ///
    /// On Windows `Command::arg` applies std's `CommandLineToArgvW` quoting,
    /// which is the wrong escaping for `cmd /C`: cmd has no backslash escape,
    /// so `type nul > "C:\dir\f"` reaches it as `type nul > \"C:\dir\f\"` and
    /// the redirect target becomes an invalid filename — cmd answers "The
    /// filename, directory name, or volume label syntax is incorrect." That
    /// broke every hook command and every skill `` !`…` `` directive that
    /// quotes a path, and because a failing PreToolUse hook BLOCKS the call,
    /// the blast radius was every tool call in the session. Same defect class
    /// as #262/#263, which [`mcp_stdio_command_builder`] closed with
    /// `raw_arg`.
    ///
    /// On Unix `sh -c` receives the line as one argv entry either way, so
    /// this case is a standing guard there and the real assertion on Windows.
    #[tokio::test]
    async fn shell_string_mode_preserves_a_quoted_path() {
        let dir = tempfile::tempdir().unwrap();
        // A space in the directory name is the everyday Windows case -- a
        // user profile directory named for a person with a given name and a
        // surname -- and is exactly what forces a caller to quote in the
        // first place.
        let sub = dir.path().join("a dir");
        std::fs::create_dir(&sub).unwrap();
        let target = sub.join("written.txt");
        let command = if cfg!(windows) {
            format!("type nul > \"{}\"", target.display())
        } else {
            format!("touch '{}'", target.display())
        };

        let output = shell_command(&command).await.expect("spawn failed");

        assert!(
            target.exists(),
            "shell-string mode did not create {} — the command line was mangled \
             before the interpreter saw it. status={:?} stdout={:?} stderr={:?}",
            target.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    /// The same guarantee for the hook builder (`cmd /V:ON /C` on Windows),
    /// which is a separate construction site and the one the PreToolUse /
    /// PostToolUse runner actually uses.
    #[tokio::test]
    async fn hook_shell_mode_preserves_a_quoted_path() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("a dir");
        std::fs::create_dir(&sub).unwrap();
        let target = sub.join("hook-fired");
        let command = if cfg!(windows) {
            format!("type nul > \"{}\"", target.display())
        } else {
            format!("touch '{}'", target.display())
        };

        let output = hook_shell_command_builder(&command)
            .output()
            .await
            .expect("spawn failed");

        assert!(
            target.exists(),
            "the hook shell did not create {} — a hook that quotes a path is \
             mangled, and a failed hook blocks the tool call. status={:?} \
             stdout={:?} stderr={:?}",
            target.display(),
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
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
