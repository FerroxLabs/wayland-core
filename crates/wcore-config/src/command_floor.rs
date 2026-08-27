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
//!    `Bash` did not, and that asymmetry is what this module closes. The
//!    control child is matched at ANY depth below `.git`, because a submodule's
//!    hooks live at `.git/modules/<name>/hooks` and run exactly the same way.
//!
//! 2. **The agent's own authority directories, whole** — `profile_home()`,
//!    `wayland_config_dir()`, and always the default `~/.wayland`, together
//!    with everything at or under them.
//!
//! Rule 2 protects the DIRECTORIES, not a list of files inside them, because a
//! list of files inside them was measured and did not hold:
//!
//! * `rm -rf ~/.wayland && mv <evil> ~/.wayland` replaces the durable
//!   grant store — standing auto-approval in every future session, which is the
//!   exact catastrophe this module exists to stop — without ever naming
//!   `permissions.toml`.
//! * `cp /tmp/x ~/.wayland/perm*.toml` reaches the same file through a glob.
//! * `cd ~/.wayland && echo x >> config.toml` reaches `security.enabled` and
//!   `tools.auto_approve` through a name too common to protect bare
//!   (`.cargo/config.toml` is ordinary work).
//! * `cp -r ~/.wayland /tmp/backup` exfiltrates `oauth/`, `.env` and the
//!   credential stores wholesale.
//! * Those directories also hold `plugins/`, `trusted-keys/`, `skills/` and
//!   `agents/` — execution and trust surfaces a per-file list never listed.
//!
//! **The cost, stated plainly.** `Bash` can no longer read or write ANYTHING
//! under those three directories, log files included. That is the price of a
//! rule with no exception list, and an exception list is precisely where the
//! next bypass would live. Nothing else on the machine is affected: the
//! directories are the agent's own state, not the user's work.
//!
//! **With one yield, and only one.** Where the session's own working directory
//! is INSIDE an authority directory, refusing everything under that directory
//! refuses every command the session can make - breaking, not widening - so
//! rule 2b falls back there to the directory ITSELF by exact name plus a
//! named-entry list. The session can still work in its own directory; the
//! directory still cannot be renamed, replaced, symlinked or copied out whole,
//! which is how the store was reached without naming it. Rule 1 and the
//! bare-name rule 2a apply unchanged, so the grant store, workspace trust and
//! the credential stores stay protected by name in that layout too. The yield
//! keys off the working directory the session was launched with, which no
//! command can change: an in-shell `cd` does not move it.
//!
//! **This module reads no switch.** No config field, no CLI flag, no
//! enable/disable environment variable. The single `env::var` it performs is
//! `WAYLAND_HOME`, which only ADDS to the protected set — the default
//! `~/.wayland` store stays protected regardless of what that variable says, so
//! an environment variable cannot move the floor off the operator's real store.
//! A base that is a filesystem root is dropped, so that variable cannot turn
//! the floor into a denial of service either.
//!
//! A component that is a glob is treated as the name it could expand onto
//! (`.git/hoo*/pre-commit`, `~/.way*`), because `cp` globs its arguments - that
//! is how the first form of this floor was walked past. A component that is
//! NOTHING but a glob (`.git/*`) names nothing in particular and is left alone.
//!
//! **Deliberately not wider.** An ancestor of a protected path is NOT matched:
//! the shortest ancestor token of `.git/hooks` is `.`, and refusing that would
//! cost `git add .`, which a floor may not do. `.git` alone is likewise not
//! matched — `git commit`, `git add` and every other porcelain verb are
//! ordinary session work. `~` is not matched either, so `rm -rf ~` still
//! destroys the store; that is a denial, not an escalation, and refusing `~`
//! would cost every ordinary command that names the home directory.
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
//! (`eval $(base64 -d ...)`, a variable holding the path, a helper script), or
//! a rule expressed without a path at all (`git config core.hooksPath /tmp/x`
//! reaches the same execution surface by naming a git KEY, not a file). It
//! exists so that the cheapest and most likely form of the catastrophe — a
//! model or a prompt injection writing the obvious path — has something
//! underneath it that no flag can lift.

use std::path::{Component, Path, PathBuf};

/// Basenames of the agent's authority state that are protected wherever they
/// appear, not only under a resolved Wayland directory.
///
/// A command that reaches the profile home by a spelling this module cannot
/// resolve (an unknown variable, a symlink) still leaves the bare name in the
/// token stream. These names carry no ordinary meaning in a source tree, which
/// is what makes the bare-name rule affordable — unlike `config.toml`, which is
/// Cargo's own (`.cargo/config.toml`) and is therefore protected by resolved
/// path only.
const AUTHORITY_BASENAMES: &[&str] = &[
    "permissions.toml",
    "workspace-trust.json",
    "credentials.toml",
    "credentials.enc",
    "credentials.kdf.json",
];

/// Entries protected INSIDE an authority directory when the whole-directory
/// rule has to yield — see [`protected_paths`]. This is a fallback list, not
/// the primary rule, and it is why the primary rule protects the directory.
const AUTHORITY_DIR_ENTRIES: &[&str] = &[
    "permissions.toml",
    "workspace-trust.json",
    "config.toml",
    "config.yaml",
    "oauth",
    "credentials.toml",
    "credentials.enc",
    "credentials.kdf.json",
    "plugins",
    "trusted-keys",
    ".env",
];

/// Repository-control path components matched wherever they appear in a token.
const REPO_CONTROL_COMPONENTS: &[&str] = &[".wayland-core", ".wayland-core.toml"];

/// The `.git` children that are execute-on-next-command surfaces.
const GIT_CONTROL_CHILDREN: &[&str] = &["hooks", "config"];

/// Shell glob metacharacters. A token carrying one of these is not the path it
/// looks like — the shell expands it before the command sees it.
const GLOB_CHARS: [char; 3] = ['*', '?', '['];

const REPO_CONTROL_REFUSAL: &str = "Refused by the command floor: this command references the repository control surface \
     (.git/hooks, .git/config, .wayland-core). Writing there is arbitrary code execution as \
     you on your next git command, so it is refused below approval and --force alike. \
     Ordinary git work (add, commit, status, push) is unaffected.";

const AUTHORITY_REFUSAL: &str = "Refused by the command floor: this command references the agent's own authority state \
     (the Wayland profile/config directories — the grant store, workspace trust, credentials, \
     oauth, plugins and skills — or one of their files by name). A command that rewrites or \
     replaces that state revokes the guard it is running under, so it is refused below \
     approval and --force alike. Ask the user to edit those files directly.";

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
    let protected = protected_paths(cwd.as_deref());

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

fn token_refusal(token: &str, cwd: Option<&Path>, protected: &Protected) -> Option<&'static str> {
    let parts = components(token);
    if parts.is_empty() {
        return None;
    }

    // Rule 1 — repository control surface, host-wide. Not cwd-relative: the
    // only thing keeping a command away from ANOTHER repository's hooks is the
    // sandbox, and the sandbox is exactly the layer a floor sits under.
    for (i, part) in parts.iter().enumerate() {
        if REPO_CONTROL_COMPONENTS
            .iter()
            .any(|name| component_may_be(part, name))
        {
            return Some(REPO_CONTROL_REFUSAL);
        }
        // `.git` alone is NOT a match — only `.git` followed by a child that is
        // an execute-on-next-command surface. That child is looked for at any
        // depth, not just the next component: a submodule's hooks live at
        // `.git/modules/<name>/hooks`. `parts` is lexically normalized first,
        // so `.git/x/../hooks` is seen as `.git/hooks` (refused) while
        // `.git/../hooks` is seen as plain `hooks` (allowed).
        if component_may_be(part, ".git")
            && parts[i + 1..].iter().any(|child| {
                GIT_CONTROL_CHILDREN
                    .iter()
                    .any(|name| component_may_be(child, name))
            })
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

    // Rule 2b — anything at or under an authority DIRECTORY, and any glob whose
    // literal prefix could expand onto one.
    let resolved = resolve(token, cwd)?;
    if protected.matches(&resolved) {
        return Some(AUTHORITY_REFUSAL);
    }
    None
}

/// What rule 2b refuses, split by how much of it is off limits.
#[derive(Default)]
struct Protected {
    /// Refused at the path AND anywhere under it.
    under: Vec<PathBuf>,
    /// Refused only when named EXACTLY: an authority directory that contains
    /// the session's own working directory. Refusing everything under it would
    /// refuse every command the session can make; refusing the directory itself
    /// still stops it being renamed, replaced, symlinked or copied out whole —
    /// which is how the store was reached without naming it.
    exact: Vec<PathBuf>,
}

impl Protected {
    fn matches(&self, resolved: &Path) -> bool {
        self.under.iter().any(|p| resolved.starts_with(p))
            || self.exact.iter().any(|p| resolved == p)
            || self.glob_could_reach(resolved)
    }

    /// A token containing a glob metacharacter is not the path it looks like.
    /// It is refused when its literal prefix could expand onto a protected
    /// path: `~/.way*` is `~/.wayland` to the shell, and `cp -r /tmp/evil
    /// ~/.way*` would otherwise walk straight past the checks above.
    ///
    /// The prefix has to end MID-component. A glob that starts a fresh
    /// component (`~/*`, `src/*.rs`, `ls *`) names no directory in particular,
    /// and refusing on it would cost ordinary work for no gain — a glob that
    /// expands onto a protected FILE still carries its basename, which rule 2a
    /// already has.
    fn glob_could_reach(&self, resolved: &Path) -> bool {
        let text = resolved.to_string_lossy();
        let Some(cut) = text.find(GLOB_CHARS) else {
            return false;
        };
        let prefix = &text[..cut];
        if prefix.is_empty() || prefix.ends_with('/') || prefix.ends_with('\\') {
            return false;
        }
        self.under
            .iter()
            .chain(self.exact.iter())
            .any(|p| p.to_string_lossy().starts_with(prefix))
    }
}

/// The authority DIRECTORIES themselves: the active profile home, the resolved
/// config dir, and — always — the default `~/.wayland`, so that pointing
/// `WAYLAND_HOME` elsewhere cannot move the floor off the operator's real
/// store.
///
/// A base with no parent (a filesystem root, or empty) is dropped. Widening the
/// protected set is the only thing `WAYLAND_HOME` may do; turning every command
/// on the machine into a refusal is not widening, it is breaking.
fn protected_paths(cwd: Option<&Path>) -> Protected {
    let mut out = Protected::default();
    for base in [
        Some(crate::config::profile_home()),
        Some(crate::config::wayland_config_dir()),
        dirs::home_dir().map(|h| h.join(".wayland")),
    ]
    .into_iter()
    .flatten()
    // A base that is a filesystem root would refuse every command on the
    // machine.
    .filter(|p| p.parent().is_some())
    {
        if cwd.is_some_and(|c| c.starts_with(&base)) {
            // The operator has put the session's own working directory inside the
            // authority directory (the migrate-quarantine live legs do exactly
            // this: the sandbox only grants writes inside the workspace, so the
            // workspace has to BE the per-run home). Refusing the whole
            // directory there refuses every command the session can make, which
            // is breaking, not widening, so rule 2b falls back to the named
            // entries. Rule 1 and the bare-name rule 2a are untouched, so the
            // grant store, workspace trust and the credential stores stay
            // protected by name even in this layout.
            out.under
                .extend(AUTHORITY_DIR_ENTRIES.iter().map(|entry| base.join(entry)));
            out.exact.push(base);
        } else {
            out.under.push(base);
        }
    }
    out.under.sort();
    out.under.dedup();
    out.exact.sort();
    out.exact.dedup();
    out
}

/// Whether a path component IS `name`, or is a glob that could expand onto it.
/// `cp evil .git/hoo*/pre-commit` globs exactly like `cp evil ~/.wayland/perm*.toml`
/// did, so a component rule that compares only equality carries the same hole
/// the resolved-path rule carried. A component that is nothing but a glob
/// (`.git/*`) names no child in particular and is not treated as one.
fn component_may_be(part: &str, name: &str) -> bool {
    if part == name {
        return true;
    }
    match part.find(GLOB_CHARS) {
        None | Some(0) => false,
        Some(cut) => name.starts_with(&part[..cut]),
    }
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

/// Split a token on both separators, drop empty / `.` segments, and resolve
/// `..` lexically, so `./.git//hooks/pre-commit` and `.git/x/../hooks/pre-commit`
/// both yield `[".git", "hooks", "pre-commit"]` while `.git/../hooks` yields
/// `["hooks"]`.
fn components(token: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in token.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            other => out.push(other.to_owned()),
        }
    }
    out
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
    fn a_git_control_child_is_matched_at_any_depth() {
        // A submodule's hooks are at `.git/modules/<name>/hooks` and run the
        // same way, so the child cannot be looked for at i+1 only.
        for command in [
            "echo id > .git/modules/sub/hooks/pre-commit",
            "echo x >> .git/modules/sub/config",
            // Lexical `..` inside the token must not dodge it either.
            "echo id > .git/objects/../hooks/pre-commit",
            // `cp` globs its arguments, so a partial name reaches the same
            // file. This is the shape that walked past the first form of the
            // floor on the authority side.
            "cp /tmp/evil .git/hoo*/pre-commit",
            "cp /tmp/evil .git/conf*",
        ] {
            assert!(
                floor_refusal(command, Some(Path::new("/work"))).is_some(),
                "must be refused: {command}"
            );
        }
        // ...and the same normalization must not INVENT a match: these two
        // name `hooks` in the working tree, not under `.git`.
        for command in [
            "cat .git/../hooks/pre-commit",
            "cat hooks/pre-commit",
            // A component that is nothing but a glob names no child in
            // particular.
            "ls .git/*",
        ] {
            assert_eq!(
                floor_refusal(command, Some(Path::new("/work"))),
                None,
                "must NOT be refused: {command}"
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
            "git config user.name x",
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
    fn the_authority_directory_itself_is_protected() {
        // Every one of these reaches the grant store, the credential stores or
        // an execution surface WITHOUT naming a protected file. Protecting a
        // list of entries inside the directory left all of them open.
        for command in [
            // Replace the directory around the store — standing auto-approval
            // in every future session, in one line, no indirection.
            "rm -rf ~/.wayland && mv /tmp/evil ~/.wayland",
            "ln -sfn /tmp/evil ~/.wayland",
            // Reach the file through a glob.
            "cp /tmp/x ~/.wayland/perm*.toml",
            // Reach the directory through a glob.
            "cp -r /tmp/evil ~/.way*",
            // Reach `security.enabled` / `tools.auto_approve` through a name
            // too common to protect bare.
            "cd ~/.wayland && echo 'tools.auto_approve = true' >> config.toml",
            // Exfiltrate oauth/, .env and the credential stores wholesale.
            "cp -r ~/.wayland /tmp/backup",
            "tar czf /tmp/x.tgz ~/.wayland",
            // Execution and trust surfaces no per-file list ever listed.
            "cp /tmp/evil.so ~/.wayland/plugins/evil.so",
            "cp /tmp/k ~/.wayland/trusted-keys/k.pub",
        ] {
            assert!(
                floor_refusal(command, Some(Path::new("/work"))).is_some(),
                "must be refused: {command}"
            );
        }
    }

    #[test]
    #[serial_test::serial]
    fn a_glob_that_names_no_directory_is_not_refused() {
        // Known-positive control in the same test: the glob arm of rule 2b IS
        // live here. The decoy home matters — every `~/.way*` spelling is also
        // a prefix of `.wayland-core`, so rule 1 would satisfy this assertion
        // and the control would prove nothing about the arm it is guarding.
        let prior = std::env::var_os("WAYLAND_HOME");
        // SAFETY: test-only env mutation, serialized against the crate's other
        // env-driven tests.
        unsafe { std::env::set_var("WAYLAND_HOME", "/tmp/wl693-decoy-home") };
        let control = floor_refusal(
            "cp -r /tmp/evil /tmp/wl693-decoy-hom*",
            Some(Path::new("/work")),
        );
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }
        assert!(
            control.is_some(),
            "control: a mid-component glob onto the profile home must be refused"
        );
        // A glob that starts a fresh component names no directory in
        // particular, and refusing on it would cost ordinary work.
        for command in ["ls *", "cat src/*.rs", "ls ~/*", "wc -l crates/*/src/*.rs"] {
            assert_eq!(
                floor_refusal(command, Some(Path::new("/work"))),
                None,
                "must NOT be refused: {command}"
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
    fn rule_2b_yields_where_the_workspace_is_inside_the_authority_directory() {
        // The migrate-quarantine live legs put the session workspace INSIDE the
        // per-run profile home, because the sandbox only grants writes inside
        // the workspace. Refusing the whole directory there refuses every
        // command the session can make.
        let base = crate::config::profile_home();
        let sentinel = base.join("run-sentinel");
        let inside = Some(base.as_path());

        // Control: from an ordinary workspace the whole-directory rule IS live
        // on this exact path. Without it the assertion below would be satisfied
        // by a floor that never fired at all.
        assert!(
            floor_refusal(
                &format!("touch {}", sentinel.display()),
                Some(Path::new("/work"))
            )
            .is_some(),
            "control: the whole-directory rule must refuse this from outside"
        );
        assert_eq!(
            floor_refusal(&format!("touch {}", sentinel.display()), inside),
            None,
            "a session working inside the authority directory must still work"
        );

        // ...and the directory ITSELF is still refused by exact name, so the
        // shape that reached the store without naming it — replacing the
        // directory around it — does not reopen in this layout.
        for command in [
            format!(
                "rm -rf {} && mv /tmp/evil {}",
                base.display(),
                base.display()
            ),
            format!("ln -sfn /tmp/evil {}", base.display()),
            format!("cp -r {} /tmp/backup", base.display()),
        ] {
            assert!(
                floor_refusal(&command, inside).is_some(),
                "the authority directory itself must still be refused: {command}"
            );
        }

        // ...and the authority state itself is STILL refused there.
        for command in [
            format!("echo x >> {}", base.join("permissions.toml").display()),
            format!("echo x >> {}", base.join("config.toml").display()),
            format!(
                "cp /tmp/evil {}",
                base.join("plugins").join("e.so").display()
            ),
            format!("cat {}", base.join("oauth").join("t.json").display()),
            format!("cat {}", base.join(".env").display()),
        ] {
            assert!(
                floor_refusal(&command, inside).is_some(),
                "must still be refused from inside: {command}"
            );
        }
    }

    #[test]
    #[serial_test::serial]
    fn a_root_wayland_home_cannot_brick_every_command() {
        // `WAYLAND_HOME` may only ADD to the protected set. Pointed at a
        // filesystem root it would otherwise refuse every command on the
        // machine, so a base with no parent is dropped.
        let prior = std::env::var_os("WAYLAND_HOME");
        // SAFETY: test-only env mutation, serialized against the crate's other
        // env-driven tests.
        unsafe { std::env::set_var("WAYLAND_HOME", "/") };
        let ordinary = floor_refusal("cat /work/src/main.rs", Some(Path::new("/work")));
        let store = floor_refusal("echo x >> ~/.wayland/permissions.toml", None);
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }
        assert_eq!(ordinary, None, "a root base must not refuse ordinary work");
        assert!(
            store.is_some(),
            "the default ~/.wayland store stays protected regardless"
        );
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
