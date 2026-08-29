import re, io, sys
p = "/root/w-f13/win-bash/crates/wcore-config/src/shell.rs"
s = open(p, encoding="utf-8").read()
orig = s

# 1. module declaration
s = s.replace(
    "mod executable_readiness;\nmod mcp_stdio_launch_context;\n",
    "mod executable_readiness;\nmod mcp_stdio_launch_context;\nmod windows_bash;\n",
    1)

# 2. re-export
s = s.replace(
    "pub use mcp_stdio_launch_context::{",
    "pub use windows_bash::{\n"
    "    BashCandidate, BashRefusal, WindowsBashEnv, WindowsBashSelection, resolve_windows_bash,\n"
    "    select_windows_bash, windows_bash_candidates, windows_bash_path_refusal,\n"
    "};\n"
    "pub use mcp_stdio_launch_context::{",
    1)

# 3. bash_shell_argv_prefix body
old = """    let choice = std::env::var("WAYLAND_BASH_SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| BASH_SHELL_CONFIG.get().cloned().flatten());
    bash_shell_prefix_for(cfg!(windows), choice.as_deref())
}"""
new = """    let choice = std::env::var("WAYLAND_BASH_SHELL")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| BASH_SHELL_CONFIG.get().cloned().flatten());
    // Only probe the filesystem when a bash could actually be selected — an
    // operator who asked for cmd or PowerShell must not pay for the walk.
    let bash = if cfg!(windows) && windows_shell_prefers_bash(choice.as_deref()) {
        resolve_windows_bash(choice.as_deref()).selected
    } else {
        None
    };
    bash_shell_prefix_for(cfg!(windows), choice.as_deref(), bash.as_deref())
}

/// Whether this `WAYLAND_BASH_SHELL` / `[tools] windows_shell` value leaves the
/// Windows interpreter open to a real bash.
///
/// True when the setting is unset (the default, which is now "a real bash if
/// this host has one") or names bash explicitly. An operator who named `cmd`,
/// `powershell`, `pwsh` — or anything else — has already chosen, and discovery
/// must not override them. Kept in step with [`bash_shell_prefix_for`]'s arms
/// by `prefers_bash_agrees_with_the_prefix_arms`.
fn windows_shell_prefers_bash(win_shell: Option<&str>) -> bool {
    matches!(
        win_shell.map(normalize_win_shell).as_deref(),
        None | Some("bash") | Some("sh")
    )
}"""
assert old in s
s = s.replace(old, new, 1)

# 4. bash_shell_prefix_for
old = """/// Pure core of [`bash_shell_argv_prefix`], split out so every branch —
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
        _ => windows_cmd_payload_prefix(),
    }
}"""
new = """/// Pure core of [`bash_shell_argv_prefix`], split out so every branch —
/// including the Windows/PowerShell/bash ones — is unit-testable on any host.
///
/// `bash` is the already-resolved absolute path to a real bash on this host, or
/// `None` when none was found (see [`windows_bash`]). Resolution is injected
/// rather than performed here so this stays pure: the Windows bash arm is
/// graded from Linux.
fn bash_shell_prefix_for(
    is_windows: bool,
    win_shell: Option<&str>,
    bash: Option<&str>,
) -> Vec<String> {
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
        // Unset, or an explicit bash: use a real bash when this host has one
        // (FerroxLabs/wayland#1164), and cmd when it does not. Any other
        // explicit value is the operator's choice and still means cmd.
        None | Some("bash") | Some("sh") => match bash {
            Some(path) => vec![path.to_string(), "-c".to_string()],
            None => windows_cmd_payload_prefix(),
        },
        _ => windows_cmd_payload_prefix(),
    }
}"""
assert old in s
s = s.replace(old, new, 1)

# 5. existing tests: add the third argument
s = re.sub(r"bash_shell_prefix_for\((true|false|cfg!\(windows\)), (Some\([^\n]*?\)|None)\)",
           r"bash_shell_prefix_for(\1, \2, None)", s)

# 6. the one existing assertion that encoded the OLD contract
old = """        // A path to an unrelated shell still falls back to cmd (only pwsh/powershell
        // are sandbox-supported selectors).
        assert_eq!(
            bash_shell_prefix_for(true, Some(r"C:\\Program Files\\Git\\bin\\bash.exe"), None),
            vec!["cmd", "/S", "/C"]
        );"""
new = """        // A path to an unrelated shell still falls back to cmd (only pwsh/powershell
        // are sandbox-supported selectors).
        assert_eq!(
            bash_shell_prefix_for(true, Some(r"C:\\Program Files\\zsh\\zsh.exe"), None),
            vec!["cmd", "/S", "/C"]
        );
        // A bash path is NO LONGER one of them: #1164 resolves a real bash and
        // runs it. It still falls back to cmd when this host has none, which is
        // the `None` third argument here; the selected-bash arm is covered in
        // `windows_bash_tests.rs`.
        assert_eq!(
            bash_shell_prefix_for(true, Some(r"C:\\Program Files\\Git\\bin\\bash.exe"), None),
            vec!["cmd", "/S", "/C"]
        );"""
assert old in s, "old bash-path assertion not found"
s = s.replace(old, new, 1)

assert s != orig
open(p, "w", encoding="utf-8").write(s)
print("patched")
