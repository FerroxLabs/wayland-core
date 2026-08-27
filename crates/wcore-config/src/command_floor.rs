//! #693 — the non-bypassable command floor for shell surfaces.
//!
//! Two things a shell command may never reach, no matter which approval mode,
//! sandbox backend or environment override is in force:
//!
//! 1. **The repository control surface** — `.git/hooks`, `.git/config`,
//!    `.wayland-core`, `.wayland-core.toml`. A hook, or a `core.fsmonitor` /
//!    `core.sshCommand` / `filter.*.clean` key in `.git/config`, is arbitrary
//!    code that runs as the operator on their NEXT git command. The file tools
//!    already refuse this (`wcore_tools::workspace_policy::is_repo_control_path`);
//!    `Bash` did not, and that asymmetry is what this module closes.
//!
//! 2. **The agent's own authority state** — `permissions.toml` (the durable
//!    learned-grant store), the global `config.toml` (which carries
//!    `security.enabled` and `tools.auto_approve`), `workspace-trust.json`, the
//!    credential stores and `oauth/`. A command that appends to the grant store
//!    has disabled the guard it is running under, permanently and for every
//!    future session.
//!
//! **This module reads no switch.** No config field, no CLI flag, no
//! enable/disable environment variable. The single `env::var` it performs is
//! `WAYLAND_HOME`, which only ADDS to the protected set — the default
//! `~/.wayland` store stays protected regardless of what that variable says, so
//! an environment variable cannot move the floor off the operator's real store.
//!
//! **Deliberately not wider.** An ancestor of a protected path is NOT matched:
//! the shortest ancestor token of `.git/hooks` is `.`, and refusing that would
//! cost `git add .`, which a floor may not do. `.git` alone is likewise not
//! matched — `git commit`, `git add` and every other porcelain verb are
//! ordinary session work.
//!
//! It lives in `wcore-config` rather than `wcore-tools` because there are TWO
//! shell surfaces — `wcore_tools::bash::BashTool` and
//! `wcore_skills::shell::execute_shell_commands` — and `wcore-skills` does not
//! depend on `wcore-tools`. `wcore-config` is the lowest crate both depend on,
//! and it already owns `wayland_config_dir()` / `profile_home()`, which is what
//! the protected set resolves through.
//!
//! This is a floor, not a boundary: it matches path-shaped tokens in the
//! command text, and a determined attacker has unbounded indirection
//! (`eval $(base64 -d ...)`, a variable holding the path, a helper script). It
//! exists so that the cheapest and most likely form of the catastrophe — a
//! model or a prompt injection writing the obvious path — has something
//! underneath it that no flag can lift.

use std::path::{Component, Path, PathBuf};

/// Basenames of the agent's authority state that are protected wherever they
/// appear, not only under a resolved Wayland directory.
///
/// A command that `cd`s into the profile home first leaves only the bare name
/// in the token stream, so the resolved-path check alone would miss it. These
/// names carry no ordinary meaning in a source tree, which is what makes the
/// bare-name rule affordable — unlike `config.toml`, which is Cargo's own
/// (`.cargo/config.toml`) and is therefore protected by resolved path only.
const AUTHORITY_BASENAMES: &[&str] = &[
    "permissions.toml",
    "workspace-trust.json",
    "credentials.toml",
    "credentials.enc",
    "credentials.kdf.json",
];

/// Entries protected only INSIDE a resolved Wayland config/profile directory.
const AUTHORITY_DIR_ENTRIES: &[&str] = &[
    "permissions.toml",
    "workspace-trust.json",
    "config.toml",
    "oauth",
    "credentials.toml",
    "credentials.enc",
    "credentials.kdf.json",
];

/// Repository-control path components matched wherever they appear in a token.
const REPO_CONTROL_COMPONENTS: &[&str] = &[".wayland-core", ".wayland-core.toml"];

/// The `.git` children that are execute-on-next-command surfaces.
const GIT_CONTROL_CHILDREN: &[&str] = &["hooks", "config"];

const REPO_CONTROL_REFUSAL: &str = "Refused by the command floor: this command references the repository control surface \
     (.git/hooks, .git/config, .wayland-core). Writing there is arbitrary code execution as \
     you on your next git command, so it is refused below approval and --force alike. \
     Ordinary git work (add, commit, status, push) is unaffected.";

const AUTHORITY_REFUSAL: &str = "Refused by the command floor: this command references the agent's own authority state \
     (permissions.toml, config.toml, workspace-trust.json, or a credential store). A command \
     that rewrites the grant store revokes the guard it is running under, so it is refused \
     below approval and --force alike. Ask the user to edit that file directly.";

/// Returns `Some(reason)` when `command` must be refused before any shell is
/// spawned. `None` means the floor has no opinion — every other guard still
/// applies.
///
/// `cwd` is the directory the command will run in, used to resolve relative
/// tokens. `None` falls back to the process working directory.
pub fn floor_refusal(command: &str, cwd: Option<&Path>) -> Option<String> {
    let cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok());
    let protected = protected_roots();

    // Match the raw command AND a de-obfuscated form. `deobfuscate` strips
    // quoting, which both reveals `.git/'hooks'` and destroys Windows
    // separators, so neither form alone is sufficient.
    let deobf = deobfuscate(command);
    for text in [command, deobf.as_str()] {
        for token in path_tokens(text) {
            if let Some(reason) = token_refusal(&token, cwd.as_deref(), &protected) {
                return Some(reason.to_string());
            }
        }
    }
    None
}

fn token_refusal(token: &str, cwd: Option<&Path>, protected: &[PathBuf]) -> Option<&'static str> {
    let parts = components(token);
    if parts.is_empty() {
        return None;
    }

    // Rule 1 — repository control surface, host-wide. Not cwd-relative: the
    // only thing keeping a command away from ANOTHER repository's hooks is the
    // sandbox, and the sandbox is exactly the layer a floor sits under.
    for (i, part) in parts.iter().enumerate() {
        if REPO_CONTROL_COMPONENTS.contains(&part.as_str()) {
            return Some(REPO_CONTROL_REFUSAL);
        }
        // `.git` alone is NOT a match — only `.git` followed by a child that
        // is an execute-on-next-command surface.
        if part == ".git"
            && parts
                .get(i + 1)
                .is_some_and(|next| GIT_CONTROL_CHILDREN.contains(&next.as_str()))
        {
            return Some(REPO_CONTROL_REFUSAL);
        }
    }

    // Rule 2a — the authority basenames, wherever they appear.
    if let Some(last) = parts.last()
        && AUTHORITY_BASENAMES.contains(&last.as_str())
    {
        return Some(AUTHORITY_REFUSAL);
    }

    // Rule 2b — anything at or under a resolved authority path.
    let resolved = resolve(token, cwd)?;
    if protected
        .iter()
        .any(|p| resolved == *p || resolved.starts_with(p))
    {
        return Some(AUTHORITY_REFUSAL);
    }
    None
}

/// Every directory the authority state can live in: the active profile home,
/// the resolved config dir, and — always — the default `~/.wayland`, so that
/// pointing `WAYLAND_HOME` elsewhere cannot move the floor off the operator's
/// real store.
fn protected_roots() -> Vec<PathBuf> {
    let mut bases = vec![
        crate::config::profile_home(),
        crate::config::wayland_config_dir(),
    ];
    if let Some(home) = dirs::home_dir() {
        bases.push(home.join(".wayland"));
    }

    let mut out = Vec::new();
    for base in bases {
        for entry in AUTHORITY_DIR_ENTRIES {
            out.push(base.join(entry));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Expand a leading `~`, `$HOME`/`${HOME}` or `$WAYLAND_HOME`/`${WAYLAND_HOME}`
/// — the shell would — then make the token absolute against `cwd` and
/// normalize `.` / `..` lexically.
fn resolve(token: &str, cwd: Option<&Path>) -> Option<PathBuf> {
    let expanded = expand_home_prefix(token)?;
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        cwd?.join(expanded)
    };
    Some(lexical_normalize(&joined))
}

fn expand_home_prefix(token: &str) -> Option<PathBuf> {
    let unix = token.replace('\\', "/");
    for (prefix, value) in [
        ("$WAYLAND_HOME", wayland_home_var()),
        ("${WAYLAND_HOME}", wayland_home_var()),
        ("$HOME", dirs::home_dir()),
        ("${HOME}", dirs::home_dir()),
        ("~", dirs::home_dir()),
    ] {
        if let Some(rest) = unix.strip_prefix(prefix)
            && (rest.is_empty() || rest.starts_with('/'))
        {
            let base = value?;
            return Some(base.join(rest.trim_start_matches('/')));
        }
    }
    if unix.starts_with('$') {
        // Some other variable — we cannot know its value, so we cannot resolve
        // it. The component rules above still saw the literal token.
        return None;
    }
    Some(PathBuf::from(token))
}

/// The ONE environment read in this module. It can only ADD to the protected
/// set — nothing here consults a value to decide whether to enforce.
fn wayland_home_var() -> Option<PathBuf> {
    std::env::var("WAYLAND_HOME").ok().map(PathBuf::from)
}

fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Split a token on both separators and drop empty / `.` segments, so
/// `./.git//hooks/pre-commit` yields `[".git", "hooks", "pre-commit"]`.
fn components(token: &str) -> Vec<String> {
    token
        .split(['/', '\\'])
        .filter(|s| !s.is_empty() && *s != ".")
        .map(str::to_owned)
        .collect()
}

/// Split a command into path-shaped words. Everything the shell treats as a
/// word separator or an operator is a delimiter, so `dd of=.git/hooks/x` and
/// `echo x>>~/.wayland/permissions.toml` both surface their path.
fn path_tokens(command: &str) -> Vec<String> {
    command
        .split(|c: char| {
            c.is_whitespace() || matches!(c, ';' | '|' | '&' | '<' | '>' | '(' | ')' | '=' | ',')
        })
        .filter(|s| !s.is_empty())
        .map(|s| s.trim_matches(['"', '\'', '`']).to_owned())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Best-effort de-obfuscation of trivial shell quoting tricks, so
/// `.git/'hooks'/pre-commit` and `.g\it/hooks` collapse onto the same token the
/// shell will produce. Mirrors `wcore_tools::bash::policy::deobfuscate`; kept
/// here so the floor does not depend on a crate above it.
fn deobfuscate(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' | '"' if chars.peek() == Some(&c) => {
                chars.next();
            }
            '\'' | '"' => {}
            '\\' => {
                if let Some(n) = chars.next() {
                    out.push(n);
                }
            }
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_hooks_are_refused_in_every_write_shape() {
        for command in [
            "printf '#!/bin/sh\\nid\\n' > .git/hooks/pre-commit",
            "echo id >> .git/hooks/pre-push",
            "cp /tmp/evil .git/hooks/pre-commit",
            "tee .git/hooks/post-merge",
            "vim /home/u/other-repo/.git/hooks/pre-commit",
            "echo x > .git/config",
        ] {
            assert!(
                floor_refusal(command, Some(Path::new("/work"))).is_some(),
                "must be refused: {command}"
            );
        }
    }

    #[test]
    fn ordinary_work_is_not_refused() {
        // The wrong-refusal arm. The shortest ancestor token of `.git/hooks`
        // is `.`, and an ancestor rule would cost every one of these.
        for command in [
            "git add .",
            "git commit -m x",
            "git status",
            "cargo build",
            "git push origin main",
            "ls -la .git",
            "cat Cargo.toml",
            "cat .cargo/config.toml",
            "rg --files .",
        ] {
            assert_eq!(
                floor_refusal(command, Some(Path::new("/work"))),
                None,
                "must NOT be refused: {command}"
            );
        }
    }

    #[test]
    fn the_authority_store_is_refused_by_bare_name_and_by_path() {
        for command in [
            "echo x >> $HOME/.wayland/permissions.toml",
            "echo x >> ${HOME}/.wayland/permissions.toml",
            "echo x >> ~/.wayland/permissions.toml",
            "cat permissions.toml",
            "cp /tmp/p workspace-trust.json",
        ] {
            assert!(
                floor_refusal(command, Some(Path::new("/work"))).is_some(),
                "must be refused: {command}"
            );
        }
    }

    #[test]
    fn the_resolved_config_dir_is_protected_without_the_bare_name_rule() {
        // `config.toml` is Cargo's own basename, so it is protected by
        // RESOLVED PATH only. This is the arm that grades rule 2b on its own:
        // drop the resolved-path check and the bare-name rule does not cover it.
        let target = crate::config::wayland_config_dir().join("config.toml");
        let command = format!("echo 'security.enabled = false' >> {}", target.display());
        assert!(
            floor_refusal(&command, None).is_some(),
            "the global config.toml carries security.enabled and tools.auto_approve"
        );
        let oauth = crate::config::profile_home()
            .join("oauth")
            .join("token.json");
        assert!(floor_refusal(&format!("cat {}", oauth.display()), None).is_some());
    }

    #[test]
    fn the_module_reads_no_switch() {
        // Structural: a floor that can be turned off is not a floor. The only
        // environment read is WAYLAND_HOME, which can only ADD to the protected
        // set. Comments are stripped so a doc line mentioning a flag does not
        // pass or fail this by accident.
        let source = include_str!("command_floor.rs");
        // Grade the module, not its own test module — the switch names below
        // are themselves code and would match.
        let source = source
            .split_once("#[cfg(test)]")
            .expect("the test module marker must be present")
            .0;
        let code: String = source
            .lines()
            .map(str::trim_start)
            .filter(|l| !l.starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        // Anti-vacuity: the stripped source must still be the real module.
        assert!(code.len() > 2000, "comment stripping ate the code");
        assert!(
            code.contains("permissions.toml"),
            "stripped source is not the floor module"
        );
        assert_eq!(
            code.matches("env::var").count(),
            1,
            "exactly one environment read is allowed, and it must be WAYLAND_HOME"
        );
        assert!(code.contains("env::var(\"WAYLAND_HOME\")"));
        for switch in ["enabled", "disable", "auto_approve", "Config::", "cfg!("] {
            assert!(
                !code.contains(switch),
                "the floor must consult no switch, found {switch:?}"
            );
        }
    }

    #[test]
    fn quoting_does_not_dodge_the_floor() {
        assert!(floor_refusal("echo x > .git/'hooks'/pre-commit", None).is_some());
        assert!(floor_refusal("echo x > \".git\"/hooks/pre-commit", None).is_some());
    }

    #[test]
    fn dot_git_alone_is_not_a_match() {
        // Not an ancestor rule: `.git` is only a match when followed by a
        // control child.
        assert_eq!(floor_refusal("du -sh .git", Some(Path::new("/w"))), None);
        assert_eq!(
            floor_refusal("cat .git/HEAD", Some(Path::new("/w"))),
            None,
            ".git/HEAD is not an execute-on-next-command surface"
        );
    }
}
