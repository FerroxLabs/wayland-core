//! Windows bash-resolution contract tests (FerroxLabs/wayland#1164, #1151 c2).
//!
//! Every test here runs on EVERY host. That is the point of the design: the
//! candidate list and the selection are pure functions over injected data, so
//! the `System32` / WindowsApps refusals are graded on Linux and macOS without
//! a Windows machine and without those files existing.

use super::*;

/// `bash_shell_prefix_for` is private to the parent `shell` module; a
/// descendant module may name it, and these cases are the Windows-bash arm of
/// that function, so they belong beside the rest of the #1164 contract.
use super::super::{bash_shell_prefix_for, windows_cmd_payload_prefix};

const GIT_BASH: &str = r"C:\Program Files\Git\bin\bash.exe";

fn env_with_program_files() -> WindowsBashEnv {
    WindowsBashEnv {
        program_files: Some(r"C:\Program Files".to_string()),
        ..Default::default()
    }
}

/// c2 — the WSL launcher is refused by NAME, not by hoping it is absent. It is
/// a real executable that exits 0 while running the command inside a Linux
/// distribution against `/mnt/c` paths, so an existence probe can never catch
/// it.
#[test]
fn system32_bash_is_refused_as_the_wsl_launcher() {
    for path in [
        r"C:\Windows\System32\bash.exe",
        r"C:\WINDOWS\SYSTEM32\BASH.EXE",
        r"C:\Windows\system32\bash.exe",
        "C:/Windows/System32/bash.exe",
        r"D:\Windows\Sysnative\bash.exe",
        r"C:\Windows\SysWOW64\bash.exe",
    ] {
        assert_eq!(
            windows_bash_path_refusal(path),
            Some(BashRefusal::WslLauncher),
            "{path} must be refused as the WSL launcher"
        );
    }
    // Refused even when the probe says it is right there.
    let selection =
        select_windows_bash(&[BashCandidate::new(r"C:\Windows\System32\bash.exe", true)]);
    assert_eq!(selection.selected, None);
    assert!(selection.refused_for(BashRefusal::WslLauncher));
}

/// c2 — the Store app-execution alias. A zero-byte reparse stub, present at a
/// path that otherwise looks perfectly ordinary.
#[test]
fn windowsapps_shim_is_refused() {
    for path in [
        r"C:\Users\sean\AppData\Local\Microsoft\WindowsApps\bash.exe",
        r"C:\Users\sean\AppData\Local\Microsoft\windowsapps\bash.exe",
        "C:/Users/sean/AppData/Local/Microsoft/WindowsApps/bash.exe",
    ] {
        assert_eq!(
            windows_bash_path_refusal(path),
            Some(BashRefusal::WindowsAppsShim),
            "{path} must be refused as a Store alias shim"
        );
    }
    let selection = select_windows_bash(&[BashCandidate::new(
        r"C:\Users\sean\AppData\Local\Microsoft\WindowsApps\bash.exe",
        true,
    )]);
    assert_eq!(selection.selected, None);
    assert!(selection.refused_for(BashRefusal::WindowsAppsShim));
}

/// c1 — a bare name is exactly the `PATH` lookup the issue refuses, and a
/// relative path resolves against a working directory an agent can write to.
/// UNC and device paths would load the image over SMB in this process's
/// security context.
#[test]
fn only_local_absolute_paths_are_accepted() {
    for path in [
        "bash",
        "bash.exe",
        r"bin\bash.exe",
        r".\bash.exe",
        r"\Git\bin\bash.exe",
        r"\\fileserver\tools\bash.exe",
        r"\\?\C:\Program Files\Git\bin\bash.exe",
        r"\\.\pipe\bash.exe",
        "//fileserver/tools/bash.exe",
    ] {
        assert_eq!(
            windows_bash_path_refusal(path),
            Some(BashRefusal::NotLocalAbsolutePath),
            "{path} must be refused: it is not a local absolute path"
        );
    }
}

#[test]
fn a_path_that_does_not_name_bash_is_refused() {
    for path in [
        r"C:\Program Files\Git\bin\sh.exe",
        r"C:\Windows\System32\cmd.exe",
        r"C:\tools\notbash.exe",
        r"C:\tools\bash.exe.bak",
        "",
    ] {
        assert_eq!(
            windows_bash_path_refusal(path),
            Some(BashRefusal::NotBash),
            "{path:?} must be refused: it does not name bash"
        );
    }
}

#[test]
fn git_for_windows_bash_is_accepted() {
    for path in [
        GIT_BASH,
        r"C:\Program Files\Git\usr\bin\bash.exe",
        r"C:\Program Files (x86)\Git\bin\bash.exe",
        r"C:\Users\sean\AppData\Local\Programs\Git\bin\bash.exe",
        r"D:\msys64\usr\bin\bash.exe",
        "C:/Program Files/Git/bin/bash.exe",
    ] {
        assert_eq!(
            windows_bash_path_refusal(path),
            None,
            "{path} must be an acceptable bash"
        );
    }
}

/// c1 — the candidate list is built from KNOWN install locations. It must not
/// contain a bare `bash.exe`, which is what a `PATH` lookup would amount to.
#[test]
fn candidates_are_known_install_locations_never_a_bare_path_lookup() {
    let env = WindowsBashEnv {
        program_files: Some(r"C:\Program Files".to_string()),
        program_w6432: Some(r"C:\Program Files".to_string()),
        program_files_x86: Some(r"C:\Program Files (x86)".to_string()),
        local_app_data: Some(r"C:\Users\sean\AppData\Local".to_string()),
        ..Default::default()
    };
    let candidates = windows_bash_candidates(&env);
    assert_eq!(
        candidates,
        vec![
            r"C:\Program Files\Git\bin\bash.exe".to_string(),
            r"C:\Program Files\Git\usr\bin\bash.exe".to_string(),
            r"C:\Program Files (x86)\Git\bin\bash.exe".to_string(),
            r"C:\Program Files (x86)\Git\usr\bin\bash.exe".to_string(),
            r"C:\Users\sean\AppData\Local\Programs\Git\bin\bash.exe".to_string(),
            r"C:\Users\sean\AppData\Local\Programs\Git\usr\bin\bash.exe".to_string(),
        ],
        "%ProgramW6432% duplicates %ProgramFiles% in a 64-bit process and must \
         not be probed twice"
    );
    assert!(
        candidates
            .iter()
            .all(|c| windows_bash_path_refusal(c).is_none()),
        "the generated list must never contain a candidate its own rules refuse"
    );
    // An empty environment yields no candidates at all rather than a bare name.
    assert!(windows_bash_candidates(&WindowsBashEnv::default()).is_empty());
}

/// A 32-bit process sees `%ProgramFiles%` redirected to the x86 tree, so
/// `%ProgramW6432%` is the only way to reach a 64-bit Git install.
#[test]
fn program_w6432_contributes_when_it_differs() {
    let env = WindowsBashEnv {
        program_files: Some(r"C:\Program Files (x86)".to_string()),
        program_w6432: Some(r"C:\Program Files".to_string()),
        ..Default::default()
    };
    assert!(windows_bash_candidates(&env).contains(&GIT_BASH.to_string()));
}

#[test]
fn selection_takes_the_first_present_candidate_and_records_the_skipped_ones() {
    let selection = select_windows_bash(&[
        BashCandidate::new(r"C:\Program Files\Git\bin\bash.exe", false),
        BashCandidate::new(r"C:\Program Files\Git\usr\bin\bash.exe", true),
        BashCandidate::new(
            r"C:\Users\sean\AppData\Local\Programs\Git\bin\bash.exe",
            true,
        ),
    ]);
    assert_eq!(
        selection.selected.as_deref(),
        Some(r"C:\Program Files\Git\usr\bin\bash.exe")
    );
    assert_eq!(
        selection.refused,
        vec![(
            r"C:\Program Files\Git\bin\bash.exe".to_string(),
            BashRefusal::NotPresent
        )],
        "candidates after the pick are not examined"
    );
}

/// c4 — with nothing acceptable, the selector picks nothing, which is what
/// makes the caller fall back to `cmd`.
#[test]
fn no_acceptable_bash_selects_nothing() {
    let selection = select_windows_bash(&[
        BashCandidate::new(r"C:\Program Files\Git\bin\bash.exe", false),
        BashCandidate::new(r"C:\Windows\System32\bash.exe", true),
    ]);
    assert_eq!(selection.selected, None);
    assert!(selection.refused_for(BashRefusal::NotPresent));
    assert!(selection.refused_for(BashRefusal::WslLauncher));
    assert_eq!(select_windows_bash(&[]), WindowsBashSelection::default());
}

/// An operator path outranks discovery, but does not buy an exemption: it goes
/// through the same refusals, so `windows_shell = "C:\Windows\System32\bash.exe"`
/// is still the WSL launcher and is still refused.
#[test]
fn explicit_operator_path_is_first_but_still_subject_to_the_refusals() {
    let mut env = env_with_program_files();
    env.explicit = Some(r"D:\msys64\usr\bin\bash.exe".to_string());
    assert_eq!(
        windows_bash_candidates(&env).first().map(String::as_str),
        Some(r"D:\msys64\usr\bin\bash.exe")
    );

    env.explicit = Some(r"C:\Windows\System32\bash.exe".to_string());
    let candidates: Vec<BashCandidate> = windows_bash_candidates(&env)
        .into_iter()
        .map(|p| BashCandidate::new(p, true))
        .collect();
    let selection = select_windows_bash(&candidates);
    assert!(selection.refused_for(BashRefusal::WslLauncher));
    assert_eq!(selection.selected.as_deref(), Some(GIT_BASH));

    // A bare word means "find one for me" and must not become a candidate.
    env.explicit = Some("bash".to_string());
    assert_eq!(
        windows_bash_candidates(&env).first().map(String::as_str),
        Some(GIT_BASH)
    );
}

/// #1151 c2 / #1164 c1 — when a real bash is resolved, THAT is the interpreter
/// the Bash tool spawns, by absolute path.
#[test]
fn windows_prefix_uses_the_resolved_bash() {
    assert_eq!(
        bash_shell_prefix_for(true, None, Some(GIT_BASH)),
        vec![GIT_BASH.to_string(), "-c".to_string()]
    );
    // A `bash` / `sh` selection resolves the same way rather than spawning a
    // bare name.
    for choice in ["bash", "BASH.EXE", "sh", GIT_BASH] {
        assert_eq!(
            bash_shell_prefix_for(true, Some(choice), Some(GIT_BASH)),
            vec![GIT_BASH.to_string(), "-c".to_string()],
            "{choice} must reach the resolved bash"
        );
    }
}

/// c4 — no acceptable bash means `cmd`, unchanged from today's behaviour.
#[test]
fn windows_prefix_falls_back_to_cmd_without_an_acceptable_bash() {
    assert_eq!(
        bash_shell_prefix_for(true, None, None),
        windows_cmd_payload_prefix()
    );
    for choice in ["bash", "sh", r"C:\Windows\System32\bash.exe"] {
        assert_eq!(
            bash_shell_prefix_for(true, Some(choice), None),
            windows_cmd_payload_prefix(),
            "{choice} with no acceptable bash must fall back to cmd"
        );
    }
}

/// An explicit non-bash selection is still honoured: resolving a bash must not
/// override an operator who asked for cmd or PowerShell.
#[test]
fn an_explicit_non_bash_selection_still_wins_over_a_resolved_bash() {
    assert_eq!(
        bash_shell_prefix_for(true, Some("cmd"), Some(GIT_BASH)),
        windows_cmd_payload_prefix()
    );
    assert_eq!(
        bash_shell_prefix_for(true, Some("zsh"), Some(GIT_BASH)),
        windows_cmd_payload_prefix()
    );
    assert_eq!(
        bash_shell_prefix_for(true, Some("pwsh"), Some(GIT_BASH)),
        vec!["pwsh", "-NoProfile", "-Command"]
    );
    // Unix is untouched: a resolved Windows bash is never consulted there.
    assert_eq!(
        bash_shell_prefix_for(false, Some("bash"), Some(GIT_BASH)),
        vec!["sh", "-c"]
    );
}

/// `windows_shell_prefers_bash` is what stops the resolver from stat-ing the
/// filesystem when the operator has already chosen a different interpreter,
/// and it must agree with the prefix function's own arms.
#[test]
fn prefers_bash_agrees_with_the_prefix_arms() {
    for (choice, prefers) in [
        (None, true),
        (Some("bash"), true),
        (Some(r"C:\Program Files\Git\bin\bash.exe"), true),
        (Some("sh"), true),
        (Some("cmd"), false),
        (Some("powershell"), false),
        (Some("pwsh"), false),
        (Some("zsh"), false),
    ] {
        assert_eq!(
            super::super::windows_shell_prefers_bash(choice),
            prefers,
            "{choice:?}"
        );
        let with_bash = bash_shell_prefix_for(true, choice, Some(GIT_BASH));
        assert_eq!(
            with_bash.first().map(String::as_str) == Some(GIT_BASH),
            prefers,
            "{choice:?} prefix disagrees with windows_shell_prefers_bash"
        );
    }
}

/// The resolver reads the real environment; on a host with no Git for Windows
/// install it must produce no selection rather than inventing one. (On Linux
/// none of the `%ProgramFiles%`-family variables exist, so the candidate list
/// is empty; the assertion holds on Windows too unless Git is installed.)
#[test]
fn resolve_never_selects_a_refused_path() {
    let selection = resolve_windows_bash(None);
    if let Some(picked) = selection.selected.as_deref() {
        assert_eq!(windows_bash_path_refusal(picked), None);
    }
    for (path, refusal) in &selection.refused {
        assert_ne!(*refusal, BashRefusal::WslLauncher, "{path}");
    }
}
