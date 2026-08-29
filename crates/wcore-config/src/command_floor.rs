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
//! command can change: an in-shell `cd` does not move it. The entry list is
//! matched through [`candidate_cwds`] like everything else, so
//! `cd <base>/logs && echo x >> ../config.toml` is refused in that layout too.
//!
//! **This module reads no switch.** No config field, no CLI flag, no
//! enable/disable environment variable. The single `env::var` it performs is
//! `WAYLAND_HOME`, which only ADDS to the protected set — the default
//! `~/.wayland` store stays protected regardless of what that variable says, so
//! an environment variable cannot move the floor off the operator's real store.
//! A base that is a filesystem root is dropped, so that variable cannot turn
//! the floor into a denial of service either.
//!
//! **`cd` is a legal token, so resolution alone cannot hold.** The shell runs
//! `cd` BEFORE the command that follows it: by the time
//! `echo 'tools.auto_approve = true' >> config.toml` runs, a leading
//! `cd $HOME && cd .wayland &&` has already moved the process somewhere the
//! working directory this module was HANDED says nothing about. The first form
//! of rule 2b resolved every relative token against that handed directory and
//! was measured walking straight past all three shapes above in their relative
//! spellings. Two rules answer it, and both are needed:
//!
//! * Rule 2b is checked against every directory the command could be running
//!   in by the time a token is read — the handed one plus each `cd` / `pushd`
//!   target the module can resolve (see [`candidate_cwds`]). This is what
//!   covers an authority directory whose NAME is not distinctive, such as a
//!   config dir at `~/.config/wayland-core` or a `WAYLAND_HOME` pointed
//!   anywhere at all.
//! * Rule 2c refuses the `.wayland` directory name at COMPONENT level whatever
//!   the token resolves to, because a `cd` target the module cannot resolve
//!   (`cd $(printf %s ~)`, `cd $SOMEVAR`) contributes no candidate and would
//!   otherwise reopen the whole class. It is the same "no ordinary meaning in a
//!   source tree" argument that already licenses [`AUTHORITY_BASENAMES`], and
//!   it costs `Bash` a workspace-local `.wayland/` too (`wl init` creates one
//!   for workflows) — orchestration definitions that run, which the file tools
//!   can still read. `.wayland-out`, the session OUTPUT root, is a different
//!   name and stays writable.
//!
//! Rule 2c yields in exactly the layout rule 2b yields in, keyed off the same
//! launch directory: a session launched INSIDE a `.wayland` directory would
//! otherwise have every absolute path it can name refused.
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
//! destroys the store and `cp -r ~ /tmp/backup` still exfiltrates it inside a
//! copy of the whole home directory. Refusing `~` would cost every ordinary
//! command that names the home directory, and refusing it only in a
//! "copy shape" would need a write-shape detector, which is the thing this
//! module refuses to have. Stated as a limit rather than half-closed.
//!
//! The same limit is why a symlink CREATED IN THE SAME COMMAND is not caught
//! (`ln -s ~/.config /tmp/c && echo x >> /tmp/c/wayland-core/config.toml`):
//! it operates on an ancestor, and the link does not exist when the floor
//! runs. A link that ALREADY exists
//! is followed ([`symlinks_followed`]), so splitting that attack across two
//! tool calls — which is otherwise free — does not work.
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

/// Authority DIRECTORY names refused at component level, whatever the token
/// resolves to.
///
/// Rule 2b resolves; `cd` defeats resolution whenever the `cd` target is
/// itself unresolvable (`cd $(printf %s ~)`, `cd $SOMEVAR`), and that is not
/// "unbounded indirection" — the protected directory is still named literally
/// in the command text. This is the name that has no ordinary meaning in a
/// source tree, so it is refused on sight.
const AUTHORITY_DIR_COMPONENTS: &[&str] = &[".wayland"];

/// Ceiling on how many working directories one command may be checked against.
/// A command with dozens of `cd`s is not ordinary work, and the cross product
/// must not become the cost of running the floor.
const MAX_CANDIDATE_CWDS: usize = 32;

/// Repository-control path components matched wherever they appear in a token.
///
/// `.wayland-core` is qualified by [`REPO_CONTROL_DATA_CHILDREN`]; every other
/// entry here is refused flat.
const REPO_CONTROL_COMPONENTS: &[&str] = &[".wayland-core", ".wayland-core.toml"];

/// The `.wayland-core` children that are DATA, not control surface, and are
/// therefore reachable from a shell.
///
/// **This is an ALLOWLIST and the polarity is the whole point. Do not invert
/// it into a denylist of control children.** `.wayland-core` holds
/// `config.toml`, `agents/*.yaml` (project-supplied manifests that
/// `acp_roster` explicitly treats as UNTRUSTED), `plugins/`, `shadow/`,
/// `skills-audit.json` — and more control files will be added. A denylist
/// would silently open a hole on the day one is added; an allowlist fails
/// CLOSED on a child nobody has taught it about yet. That is why `.git`'s
/// CLOSED set of control children ([`GIT_CONTROL_CHILDREN`]) may be expressed
/// the other way round and this one may not.
///
/// `skills` is here because every project skill's executable lives at
/// `.wayland-core/skills/**`. With the component refused at any depth, no
/// skill that ships an executable could run at all — `cd`, `find` and
/// `node <path>` were refused alike — and the refusal did not fail cleanly.
/// Writes under that subtree are still denied a layer up by
/// `RepoControlDenyFs`, which names `.wayland-core/skills/**` for both the
/// strict and the trusted profile; "deny writes, allow reads" is the product
/// policy, and the floor was the layer disagreeing with it.
///
/// Matched by EXACT name, never through [`component_may_be`]: a glob child is
/// not definitely `skills` (`.wayland-core/sk*` also expands onto
/// `skills-audit.json`), so it stays on the refusing side.
const REPO_CONTROL_DATA_CHILDREN: &[&str] = &["skills"];

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

/// #355 — the marker every command-floor refusal carries in the PAYLOAD the
/// model reads.
///
/// Without it a refusal is indistinguishable from a missing binary, a
/// permission error or a flaky sandbox, and a model that cannot tell those
/// apart does the reasonable thing for a transient failure: it tries another
/// route. That is the reported incident exactly — the work was staged under a
/// temporary directory, written at the destination and reported as finished,
/// and the user was never told a policy had fired at all.
///
/// It lives in the payload and NOT in a log line on purpose. `RUST_LOG` is
/// unset on a default install, so only `ERROR` reaches stderr and a `warn!`
/// reaches nobody; and the model never reads the log under any setting.
pub const POLICY_REFUSAL_MARKER: &str = "[POLICY-REFUSAL: command-floor]";

/// The instruction half of the disclosure: what the model must DO about it.
///
/// Naming the rule was never the gap — the shipped refusal already did that.
/// The gap was that nothing told the model this outcome is terminal, so
/// "try it a different way" stayed the obvious next move.
const DISCLOSURE_DIRECTIVE: &str = "This is a policy decision, not a transient tool failure. \
     Retrying it, respelling or quoting the path, staging the work in a temporary directory and \
     moving it into place, or reaching the same destination by any other route will be refused \
     identically — the floor answers below approval mode, --force and every environment \
     override. Do NOT work around it and do NOT continue as though the step had succeeded. Stop \
     this line of work and tell the user, in your reply, that the command was blocked by the \
     Wayland command floor and which rule it hit, so they can decide what to do.";

/// Compose the refusal payload: the rule that fired, then the policy marker
/// and the instruction to surface it.
fn disclose(reason: &str) -> String {
    format!("{reason}\n\n{POLICY_REFUSAL_MARKER} {DISCLOSURE_DIRECTIVE}")
}

/// True when `text` is a command-floor refusal rather than a tool failure.
///
/// The one predicate every surface that wants to treat a policy refusal
/// differently from a failed command asks. Matching the marker, not the
/// refusal prose, is the point: the prose is written for the model and is
/// free to change.
pub fn is_policy_refusal(text: &str) -> bool {
    text.contains(POLICY_REFUSAL_MARKER)
}

/// The USER-facing notice for a refusal payload, or `None` when the payload is
/// an ordinary tool failure.
///
/// The payload directive asks the model to disclose the refusal; this is the
/// disclosure that does not depend on it complying. `disclose` puts the rule
/// on the payload's first line precisely so this can quote it verbatim
/// instead of paraphrasing a policy.
pub fn policy_refusal_notice(tool_name: &str, payload: &str) -> Option<String> {
    if !is_policy_refusal(payload) {
        return None;
    }
    let rule = payload.lines().next().unwrap_or_default().trim();
    Some(format!(
        "Blocked by policy, not by a failure — `{tool_name}` was refused before it ran. {rule} \
         The agent has been told to stop and report this rather than route around it."
    ))
}

/// Returns `Some(reason)` when `command` must be refused before any shell is
/// spawned. `None` means the floor has no opinion — every other guard still
/// applies.
///
/// `cwd` is the directory the command will run in, used to resolve relative
/// tokens. `None` falls back to the process working directory.
pub fn floor_refusal(command: &str, cwd: Option<&Path>) -> Option<String> {
    // Normalized here so that every comparison below is between paths built
    // the same way. A token resolves through `lexical_normalize`, which
    // rebuilds the path from its components and therefore emits the platform
    // separator; a launch directory or a protected base handed in with the
    // other separator would then never compare equal.
    let cwd = cwd
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .map(|c| lexical_normalize(&c));
    let protected = protected_paths(cwd.as_deref());
    // Rule 2c yields where rule 2b yields, keyed off the same launch directory
    // — which no in-shell `cd` can move.
    let dir_components = !cwd
        .as_deref()
        .is_some_and(cwd_is_inside_an_authority_dirname);

    // Match the raw command AND a de-obfuscated form. `deobfuscate` strips
    // quoting, which both reveals `.git/'hooks'` and destroys Windows
    // separators, so neither form alone is sufficient.
    let deobf = deobfuscate(command);
    for text in [command, deobf.as_str()] {
        // The shell executes `cd` first, so the handed directory is only the
        // FIRST place a relative token can land.
        let cwds = candidate_cwds(text, cwd.as_deref());
        for token in path_tokens(text) {
            if let Some(reason) = token_refusal(&token, &cwds, &protected, dir_components) {
                // #355 — never the bare rule. Every refusal leaves here
                // carrying the policy marker and the stop instruction.
                return Some(disclose(reason));
            }
        }
    }
    None
}

fn token_refusal(
    token: &str,
    cwds: &[PathBuf],
    protected: &Protected,
    dir_components: bool,
) -> Option<&'static str> {
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
            && !opens_a_repo_control_data_subtree(&parts, i)
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

    // Rule 2c — an authority directory by NAME, at component level, whatever
    // this token resolves to. Resolution is defeated by a `cd` whose own target
    // cannot be resolved; the name in the command text is not.
    if dir_components
        && parts.iter().any(|part| {
            AUTHORITY_DIR_COMPONENTS
                .iter()
                .any(|name| component_may_be(part, name))
        })
    {
        return Some(AUTHORITY_REFUSAL);
    }

    // Rule 2b — anything at or under an authority DIRECTORY, and any glob whose
    // literal prefix could expand onto one. Checked against EVERY directory the
    // command could be running in by the time this token is read, because `cd`
    // runs before it.
    if cwds.is_empty() {
        if reaches_protected(token, None, protected) {
            return Some(AUTHORITY_REFUSAL);
        }
        return None;
    }
    for cwd in cwds {
        if reaches_protected(token, Some(cwd), protected) {
            return Some(AUTHORITY_REFUSAL);
        }
    }
    None
}

/// Whether `token`, read from `cwd`, lands on a protected path — lexically, or
/// after the kernel follows the symlinks that are already on disk.
fn reaches_protected(token: &str, cwd: Option<&Path>, protected: &Protected) -> bool {
    let Some(resolved) = resolve(token, cwd) else {
        return false;
    };
    protected.matches(&resolved)
        || symlinks_followed(&resolved).is_some_and(|real| protected.matches(&real))
}

/// The path a token names once EXISTING symlinks are followed, or `None` if
/// that is the path itself.
///
/// A symlink is not a lexical construct. `ln -s ~/.config /tmp/c` names no
/// protected path — `~/.config` is an ANCESTOR of the config dir, and this
/// module does not match ancestors — and a LATER command then reaches
/// `security.enabled` and `tools.auto_approve` through
/// `/tmp/c/wayland-core/config.toml`, whose every component is ordinary.
/// Splitting an attack across two tool calls is free, so the second call is
/// where it has to be caught.
///
/// The DEEPEST EXISTING ancestor is canonicalized and the remainder re-joined,
/// because the token usually names a file that does not exist yet.
///
/// This does not close the SAME-COMMAND form (`ln -s ~/.config /tmp/c && echo
/// x >> /tmp/c/wayland-core/config.toml`): the link does not exist when the
/// floor runs, and no check that runs before the shell can see it. That is the
/// ancestor limit in the module doc, not a hole this function pretends to fill.
fn symlinks_followed(path: &Path) -> Option<PathBuf> {
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut probe = path.to_path_buf();
    loop {
        // `dunce`, not `std::fs`: the latter answers with the `\\?\` verbatim
        // spelling on Windows, which matches no protected base and made this
        // whole fallback dead there.
        if let Ok(real) = dunce::canonicalize(&probe) {
            let mut out = real;
            for part in tail.iter().rev() {
                out.push(part);
            }
            return (out != *path).then_some(out);
        }
        tail.push(probe.file_name()?.to_os_string());
        if !probe.pop() {
            return None;
        }
    }
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
    {
        // Decided ONCE for the base, then applied to every spelling of it, so
        // the yield cannot fire for one name and not the other.
        let yields = cwd_is_under(cwd, &base);
        for base in spellings(&base)
            .into_iter()
            // A base that is a filesystem root would refuse every command on
            // the machine.
            .filter(|p| p.parent().is_some())
        {
            if yields {
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
    }
    out.under.sort();
    out.under.dedup();
    out.exact.sort();
    out.exact.dedup();
    out
}

/// Every directory the command could be running in by the time a relative
/// token is read.
///
/// `cd` is a legal token and the shell executes it FIRST. Resolving relative
/// tokens against the directory the tool was handed is what let
/// `cd $HOME && cd .wayland && echo 'tools.auto_approve = true' >> config.toml`
/// reach the durable grant store through a floor that refused the same command
/// spelled absolutely.
///
/// Deliberately an over-approximation: a `cd` inside a subshell, or on a branch
/// that never runs, still contributes a candidate, and every `cd` target is
/// resolved against every candidate so far rather than tracking one position.
/// The direction of that error is a refusal the operator can rephrase; the
/// other direction is the catastrophe this module exists to stop. A target this
/// module cannot resolve contributes nothing — which is why rule 2c does not
/// depend on resolution at all.
fn candidate_cwds(text: &str, cwd: Option<&Path>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = cwd.map(Path::to_path_buf).into_iter().collect();
    let tokens = path_tokens(text);
    for (i, token) in tokens.iter().enumerate() {
        if !matches!(token.as_str(), "cd" | "pushd") {
            continue;
        }
        let Some(target) = tokens.get(i + 1) else {
            continue;
        };
        let reached: Vec<PathBuf> = if out.is_empty() {
            resolve(target, None).into_iter().collect()
        } else {
            out.iter()
                .filter_map(|base| resolve(target, Some(base)))
                .collect()
        };
        for path in reached {
            if out.len() >= MAX_CANDIDATE_CWDS {
                return out;
            }
            if !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

/// Whether the session was LAUNCHED inside a directory rule 2c names. There,
/// refusing the name would refuse every absolute path the session can write —
/// breaking, not widening — so rule 2c yields exactly as rule 2b does.
fn cwd_is_inside_an_authority_dirname(cwd: &Path) -> bool {
    cwd.components().any(|component| match component {
        Component::Normal(name) => AUTHORITY_DIR_COMPONENTS
            .iter()
            .any(|dir| name.to_string_lossy() == **dir),
        _ => false,
    })
}

/// Whether `parts[i]` is the `.wayland-core` directory entered through a child
/// that is DATA rather than control surface — see [`REPO_CONTROL_DATA_CHILDREN`].
///
/// `parts` is already lexically normalized by [`components`], so
/// `.wayland-core/./skills` and `.wayland-core/x/../skills` both arrive here as
/// `skills` and are allowed, while `.wayland-core/skills/../config.toml`
/// arrives as `config.toml` and is not.
///
/// A bare `.wayland-core` has no next component and stays refused, as does any
/// child the allowlist does not name — including one that does not exist yet.
fn opens_a_repo_control_data_subtree(parts: &[String], i: usize) -> bool {
    if parts[i] != ".wayland-core" {
        // Only the directory is qualified. `.wayland-core.toml` is a separate
        // literal, and a GLOB standing in for either names no child in
        // particular, so neither may reach the allowlist.
        return false;
    }
    parts
        .get(i + 1)
        .is_some_and(|child| REPO_CONTROL_DATA_CHILDREN.contains(&child.as_str()))
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

/// Every spelling of `base` a resolved token might arrive in.
///
/// Windows hands the same directory out under two names: `std::env::temp_dir`
/// on a service account answers the 8.3 form
/// `C:\WINDOWS\SERVIC~1\NETWOR~1\...`, while the workspace the tool layer is
/// handed is the long form. macOS does the same with a symlinked ancestor
/// (`/var` -> `/private/var`).
///
/// BOTH are kept, never one INSTEAD of the other. A base is a thing to refuse,
/// so listing an extra spelling of it can only ever refuse more — which is the
/// only direction this module may be wrong in. Replacing the lexical form with
/// the canonical one looked equivalent and was not: it moved the protected set
/// rather than widening it, and measured as the floor under-refusing on both
/// Windows and macOS.
///
/// `dunce::canonicalize` rather than `std::fs::canonicalize`: the latter returns
/// the `\\?\` verbatim spelling on Windows, which is a THIRD form that matches
/// neither of the two a command can actually contain.
fn spellings(base: &Path) -> Vec<PathBuf> {
    let lexical = lexical_normalize(base);
    match dunce::canonicalize(base) {
        Ok(real) if real != lexical => vec![lexical, real],
        _ => vec![lexical],
    }
}

/// Whether the session was launched at or under `base`, in ANY spelling of
/// either.
///
/// This decides rule 2b's yield, and the yield NARROWS what is refused, so it
/// must not be defeated by a spelling: a session whose workspace IS the
/// authority directory under one name and not under another would have every
/// command it can make refused. Compared both ways for the same reason
/// `spellings` returns both.
fn cwd_is_under(cwd: Option<&Path>, base: &Path) -> bool {
    let Some(cwd) = cwd else {
        return false;
    };
    let cwds = spellings(cwd);
    spellings(base)
        .iter()
        .any(|b| cwds.iter().any(|c| c.starts_with(b)))
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

/// Split a command into path-shaped words, the way the shell would.
///
/// Everything the shell treats as a word separator or an operator is a
/// delimiter, so `dd of=.git/hooks/x` and `echo x>>~/.wayland/permissions.toml`
/// both surface their path — but a delimiter INSIDE a quoted span, or escaped
/// with a backslash, is not one. A plain `split_whitespace` cut every path
/// containing a space in half, and neither half matched anything: on macOS the
/// default config dir is `~/Library/Application Support/wayland-core`, so the
/// floor did not protect the operator's own config directory at all, in the one
/// spelling a shell actually executes.
///
/// The UNQUOTED spelling of such a path stays split, and correctly: `echo x >>
/// /a b/c` writes to `/a`, so there is nothing there for the floor to refuse.
fn path_tokens(command: &str) -> Vec<String> {
    // The UNION of both readings, never one instead of the other.
    //
    // Quote-aware tokenization is only right about a command whose quoting is
    // BALANCED. A stray apostrophe (`echo don't && cat <home>/notes.txt`) opens
    // a span that never closes and swallows every path after it into one token
    // — the direction that refuses LESS.
    //
    // On Unix the de-obfuscated form covers that: it deletes the quote, so the
    // second form `floor_refusal` scans has a boundary where the raw one has
    // none. ON WINDOWS IT DOES NOT: `deobfuscate` also eats every `\` as an
    // escape, so `C:\Users\x\notes.txt` arrives as `C:Usersxnotes.txt` and is
    // no longer a path at all. Both passes therefore miss it, and the naive
    // split of the RAW command is the only reading that still contains the
    // path. Measured: `a_stray_quote_does_not_swallow_a_protected_path` passes
    // on Linux with this union removed and FAILS on the Windows leg.
    let mut out = quoted_tokens(command);
    out.extend(
        command
            .split(|c: char| {
                c.is_whitespace()
                    || matches!(c, ';' | '|' | '&' | '<' | '>' | '(' | ')' | '=' | ',')
            })
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_matches(['"', '\'', '`']).to_owned())
            .filter(|s| !s.is_empty()),
    );
    out
}

/// Tokenize the way the shell does: a delimiter inside a quoted span, or
/// escaped with a backslash, is not a delimiter.
fn quoted_tokens(command: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut token = String::new();
    let mut single = false;
    let mut double = false;
    let mut chars = command.chars().peekable();

    while let Some(c) = chars.next() {
        match c {
            // A quoted span is ONE token, space included. Without this, every
            // path containing a space was split in half and matched nothing --
            // and on macOS the default config dir is inside `Library/
            // Application Support`, so the floor did not protect the
            // operator's own config directory at all.
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            // A backslash keeps the NEXT character in the token, but only when
            // that character is one the shell would otherwise treat as a
            // boundary. Escaping unconditionally would eat Windows separators
            // (`C:\Users\x` -> `C:Usersx`), which is exactly the damage the
            // de-obfuscated pass does and the reason both forms are scanned.
            '\\' if !single
                && chars
                    .peek()
                    .is_some_and(|n| n.is_whitespace() || matches!(n, '"' | '\'' | '`')) =>
            {
                if let Some(n) = chars.next() {
                    token.push(n);
                }
            }
            '`' if !single && !double => {}
            c if !single
                && !double
                && (c.is_whitespace()
                    || matches!(c, ';' | '|' | '&' | '<' | '>' | '(' | ')' | '=' | ',')) =>
            {
                if !token.is_empty() {
                    out.push(std::mem::take(&mut token));
                }
            }
            other => token.push(other),
        }
    }
    if !token.is_empty() {
        out.push(token);
    }
    out
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

    /// The structural invariant behind the Windows separator bug: every path
    /// `Protected` holds must already be `lexical_normalize`d, because that is
    /// what every token it is compared against went through.
    ///
    /// Asserted on the SHAPE rather than on a spelling, so it holds on any
    /// platform. On Windows the two forms genuinely differ — a `WAYLAND_HOME`
    /// spelled with forward slashes normalises to backslashes and then never
    /// compares equal to itself — and the leg that caught it lives only there.
    /// This is the arm that fails on Linux and macOS too if the normalization
    /// is dropped from `protected_paths`.
    /// BOTH spellings of an authority directory are refused — the one the
    /// operator configured and the one the kernel answers with.
    ///
    /// This is the arm that fails if the protected set is MOVED to the
    /// canonical spelling instead of WIDENED to include it. That looked
    /// equivalent and was not: measured as the floor under-refusing on Windows
    /// (8.3 short components) and macOS (`/var` -> `/private/var`), on a change
    /// whose Linux run was 2,604/2,604 green.
    ///
    /// Built from a symlink so the two spellings differ on any Unix, rather
    /// than relying on a platform quirk only one runner has.
    #[test]
    #[serial_test::serial]
    fn both_spellings_of_the_profile_home_are_refused() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        std::fs::create_dir_all(real.join("oauth")).unwrap();
        let link = root.path().join("link");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&real, &link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&real, &link);
        if made.is_err() {
            // Windows without the symlink privilege. The invariant is real
            // there too, but it cannot be built here; say so rather than pass.
            eprintln!("skipped: this platform would not create a directory symlink");
            return;
        }

        let prior = std::env::var_os("WAYLAND_HOME");
        // SAFETY: test-only env mutation, serialized against this module's
        // other env-driven tests.
        unsafe { std::env::set_var("WAYLAND_HOME", &link) };
        let through_link = floor_refusal(
            &format!("cat {}/oauth/token.json", link.display()),
            Some(Path::new("/work")),
        );
        let through_real = floor_refusal(
            &format!("cat {}/oauth/token.json", real.display()),
            Some(Path::new("/work")),
        );
        // Control: an ordinary path beside them is still allowed, so a
        // refuse-everything regression cannot satisfy the two above.
        let beside = floor_refusal(
            &format!("cat {}/notes.txt", root.path().display()),
            Some(Path::new("/work")),
        );
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }

        assert!(
            through_link.is_some(),
            "the configured spelling of the profile home must be refused"
        );
        assert!(
            through_real.is_some(),
            "the spelling the KERNEL answers with names the same directory and \
             must be refused too — a command can contain either"
        );
        assert_eq!(
            beside, None,
            "control: an ordinary sibling path must still be allowed"
        );
    }

    /// A protected path containing a SPACE is refused in every spelling a
    /// shell would actually execute, and only in those.
    ///
    /// `path_tokens` used to split on whitespace with no quote awareness, so
    /// any such path was cut in half and neither half matched anything. That is
    /// not a corner case: on macOS the default config dir is
    /// `~/Library/Application Support/wayland-core`, so on that platform the
    /// floor did not protect the operator's own config directory at all — and
    /// the suite did not show it, because every test spelled the command the
    /// one way a shell would not run.
    #[test]
    #[serial_test::serial]
    fn a_protected_path_with_a_space_is_refused_in_the_spellings_a_shell_runs() {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("Application Support").join("wayland-core");
        std::fs::create_dir_all(&home).unwrap();
        let target = home.join("config.toml");
        let prior = std::env::var_os("WAYLAND_HOME");
        // SAFETY: test-only env mutation, serialized against this module's
        // other env-driven tests.
        unsafe { std::env::set_var("WAYLAND_HOME", &home) };

        let quoted = floor_refusal(
            &format!("echo x >> \"{}\"", target.display()),
            Some(Path::new("/work")),
        );
        let single_quoted = floor_refusal(
            &format!("echo x >> '{}'", target.display()),
            Some(Path::new("/work")),
        );
        let escaped = floor_refusal(
            &format!(
                "echo x >> {}",
                target.display().to_string().replace(' ', "\\ ")
            ),
            Some(Path::new("/work")),
        );
        // The BARE spelling stays split, and correctly: the shell writes to
        // `<root>/Application`, which is not protected, so there is nothing
        // here to refuse. Asserting a refusal would be asserting that the floor
        // guesses at a command the shell will not run.
        let bare = floor_refusal(
            &format!("echo x >> {}", target.display()),
            Some(Path::new("/work")),
        );
        // Control: quoting alone must not become a refusal.
        let ordinary = floor_refusal(
            &format!(
                "echo x >> \"{}/notes with spaces.txt\"",
                root.path().display()
            ),
            Some(Path::new("/work")),
        );
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }

        assert!(quoted.is_some(), "a double-quoted path must be refused");
        assert!(
            single_quoted.is_some(),
            "a single-quoted path must be refused"
        );
        assert!(
            escaped.is_some(),
            "a backslash-escaped space must not split the token"
        );
        assert_eq!(
            bare, None,
            "an UNQUOTED path with a space reaches nothing protected — refusing \
             it would be refusing a command the shell will not run"
        );
        assert_eq!(
            ordinary, None,
            "control: quoting is not itself a reason to refuse"
        );
    }

    /// An UNBALANCED quote must not hide a protected path.
    ///
    /// Quote-aware tokenization is only right about a command whose quoting is
    /// balanced. A stray apostrophe opens a span that never closes and swallows
    /// every path after it into one token — the direction that refuses LESS.
    ///
    /// On Unix the DE-OBFUSCATED pass answers it: `deobfuscate` removes the
    /// quote, so that form has a token boundary where the raw one has none.
    /// **On Windows it does not** — `deobfuscate` also eats every `\` as an
    /// escape, so the path arrives as `C:Usersxnotes.txt` and is not a path any
    /// more. There the naive split of the RAW command is the only reading that
    /// still holds it, which is why `path_tokens` returns the union.
    ///
    /// This test was written, then briefly "proved unnecessary" by a red arm
    /// that only ran on Linux, and then failed on the Windows leg. It is the
    /// reason the union is in the tree.
    ///
    /// The profile home here has an ORDINARY name and the file an ordinary
    /// basename, so neither the component rule nor the bare-name rule can
    /// answer — only rule 2b. A first version used
    /// `~/.wayland/permissions.toml` and proved nothing: rule 2a caught it on
    /// the basename.
    #[test]
    #[serial_test::serial]
    fn a_stray_quote_does_not_swallow_a_protected_path() {
        let home = tempfile::tempdir().unwrap();
        let target = home.path().join("notes.txt");
        let prior = std::env::var_os("WAYLAND_HOME");
        // SAFETY: test-only env mutation, serialized against this module's
        // other env-driven tests.
        unsafe { std::env::set_var("WAYLAND_HOME", home.path()) };

        // The apostrophe in `don't` is never closed.
        let hidden = floor_refusal(
            &format!("echo don't && cat {}", target.display()),
            Some(Path::new("/work")),
        );
        // Control: the same command with balanced quoting is refused too, so
        // the assertion above is about the tokenization and not about this
        // path being unreachable for some other reason.
        let balanced = floor_refusal(
            &format!("echo dont && cat {}", target.display()),
            Some(Path::new("/work")),
        );
        // Control: an apostrophe is not itself a reason to refuse.
        let ordinary = floor_refusal("echo don't && rm -rf ./build", Some(Path::new("/work")));
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }

        assert!(
            hidden.is_some(),
            "an unbalanced quote before a protected path must not hide it"
        );
        assert!(
            balanced.is_some(),
            "control: the same path with balanced quoting must be refused"
        );
        assert_eq!(
            ordinary, None,
            "control: an apostrophe is not itself a reason to refuse"
        );
    }

    /// Rule 2b's yield must recognise the workspace as being inside the
    /// authority directory even when the two are spelled differently.
    ///
    /// The yield exists so a session LAUNCHED inside an authority directory can
    /// still write its own files — refusing there refuses every command it can
    /// make. Windows hands the same directory out under an 8.3 short name and a
    /// long one, so `starts_with` said "different" and a live
    /// migrate-quarantine leg had its own sentinel write refused by the floor.
    ///
    /// Built from a symlink so the two spellings differ on any Unix rather than
    /// only on the runner that showed it. The subject asserts a NON-refusal, so
    /// it carries two controls: the same command outside the workspace must
    /// still be refused, and the grant store must still be refused by name
    /// inside the yield.
    #[test]
    #[serial_test::serial]
    fn the_yield_recognises_the_workspace_under_a_second_spelling() {
        let root = tempfile::tempdir().unwrap();
        let real = root.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = root.path().join("link");
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&real, &link);
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&real, &link);
        if made.is_err() {
            eprintln!("skipped: this platform would not create a directory symlink");
            return;
        }

        let prior = std::env::var_os("WAYLAND_HOME");
        // The profile home is the LINK, and so is the path in the command; the
        // WORKSPACE is that same directory reached by its real name — exactly
        // as Windows hands out a long path for a home configured through a
        // short one.
        // SAFETY: test-only env mutation, serialized against this module's
        // other env-driven tests.
        unsafe { std::env::set_var("WAYLAND_HOME", &link) };
        let own_file = floor_refusal(
            &format!("echo x > \"{}/run-sentinel\"", link.display()),
            Some(&real),
        );
        let without_yield = floor_refusal(
            &format!("echo x > \"{}/run-sentinel\"", link.display()),
            Some(Path::new("/work")),
        );
        let store = floor_refusal(
            &format!("echo x > \"{}/permissions.toml\"", link.display()),
            Some(&real),
        );
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }

        assert_eq!(
            own_file, None,
            "a session launched inside its own profile home must be able to \
             write there — the yield has to see through the second spelling"
        );
        assert!(
            without_yield.is_some(),
            "control: the same path is refused when the session is NOT inside \
             the profile home, or the assertion above proves nothing"
        );
        assert!(
            store.is_some(),
            "control: the yield narrows rule 2b, it does not switch off the \
             bare-name rule that protects the grant store"
        );
    }

    #[test]
    #[serial_test::serial]
    fn every_protected_base_is_already_normalized() {
        let prior = std::env::var_os("WAYLAND_HOME");
        // A home whose spelling normalization would change: a redundant `.`
        // and a doubled separator, both of which `components()` removes.
        // SAFETY: test-only env mutation, serialized against this module's
        // other env-driven tests.
        unsafe { std::env::set_var("WAYLAND_HOME", "/tmp/wl693-norm/./deep//home") };
        let protected = protected_paths(Some(Path::new("/work")));
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }

        let bases: Vec<&PathBuf> = protected
            .under
            .iter()
            .chain(protected.exact.iter())
            .collect();
        assert!(
            !bases.is_empty(),
            "no protected base at all, so the assertion below would be vacuous"
        );
        for base in &bases {
            assert!(
                spellings(base).contains(base),
                "a protected base must be one of its own spellings, or it can \
                 never compare equal to a token that resolved to the same \
                 directory: {base:?}"
            );
        }
        // And the specific one: the un-normalized spelling must not survive.
        assert!(
            bases
                .iter()
                .any(|b| b.ends_with("home") && !b.to_string_lossy().contains("/./")),
            "the WAYLAND_HOME base is missing or kept its raw spelling: {bases:?}"
        );
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

    /// The shape that overturned the previous form of this floor.
    ///
    /// `cd` is a legal token and the shell runs it FIRST, so a path spelled
    /// relatively lands somewhere the working directory this module was handed
    /// says nothing about. Every command here names the protected directory
    /// literally — no `eval`, no `base64`, no variable holding the path — so
    /// none of them is covered by the indirection limit in the module doc.
    #[test]
    fn a_cd_cannot_move_the_floor_off_the_authority_directories() {
        let home = dirs::home_dir()
            .expect("a home directory")
            .display()
            .to_string();
        for command in [
            format!("cd {home} && cd .wayland && echo 'tools.auto_approve = true' >> config.toml"),
            format!("cd {home} && cp -r .wayland /tmp/backup"),
            format!("cd {home} && rm -rf .wayland && mv /tmp/evil .wayland"),
            format!("cd {home} && cd .way* && cat permissions.toml"),
            format!("cd {home}/src && cd ../.wayland && cat config.toml"),
            format!("cd {home} ; cd .wayland ; cat config.toml"),
            format!("(cd {home} && cd .wayland && cat config.toml)"),
            // The `cd` TARGET is itself unresolvable here, so no candidate
            // directory is produced and ONLY the component rule can catch it.
            "cd $(printf %s ~) && cd .wayland && echo x >> config.toml".to_string(),
            "cd $UNKNOWABLE && cd .wayland && cat config.toml".to_string(),
            // No `cd` at all: a name-only reference the resolver cannot place.
            "find / -maxdepth 3 -name '.way*' -exec rm -rf {} +".to_string(),
        ] {
            assert!(
                floor_refusal(&command, Some(Path::new("/work"))).is_some(),
                "must be refused: {command}"
            );
        }
    }

    /// A symlink is not a lexical construct.
    ///
    /// `ln -s ~/.config /tmp/c` names no protected path — `~/.config` is an
    /// ANCESTOR of the config dir, and this module does not match ancestors.
    /// A LATER command then reaches `security.enabled` and
    /// `tools.auto_approve` through `/tmp/c/<config-dir>/config.toml`, whose
    /// every component is ordinary. Splitting an attack across two tool calls
    /// is free, so the second call is where this has to be caught.
    #[test]
    #[serial_test::serial]
    #[cfg(unix)]
    fn a_symlink_does_not_manufacture_a_new_name_for_the_store() {
        let tmp = tempfile::tempdir().unwrap();
        let real = std::fs::canonicalize(tmp.path()).unwrap();
        let store = real.join("store");
        let other = real.join("other");
        std::fs::create_dir_all(&store).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let link = real.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();

        let prior = std::env::var_os("WAYLAND_HOME");
        // SAFETY: test-only env mutation, serialized against this crate's
        // other env-driven tests.
        unsafe { std::env::set_var("WAYLAND_HOME", &store) };
        let direct = floor_refusal(
            &format!("echo x >> {}/config.toml", store.display()),
            Some(Path::new("/work")),
        );
        let through_link = floor_refusal(
            &format!("echo x >> {}/store/config.toml", link.display()),
            Some(Path::new("/work")),
        );
        let ordinary = floor_refusal(
            &format!("echo x >> {}/other/config.toml", link.display()),
            Some(Path::new("/work")),
        );
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }
        // Known-positive control: this store IS protected here by its own
        // resolved path.
        assert!(
            direct.is_some(),
            "control: the resolved store must be refused"
        );
        assert!(
            through_link.is_some(),
            "the same file through a symlinked ancestor must be refused"
        );
        // ...and the identical shape onto a sibling directory is not refused,
        // so the arm above is not passing because every symlink is.
        assert_eq!(
            ordinary, None,
            "a symlink that reaches nothing protected must not be refused"
        );
    }

    /// Grades the candidate-directory rule ON ITS OWN.
    ///
    /// The component rule can only refuse a name with no ordinary meaning in a
    /// source tree. An authority directory does not have to carry one: the
    /// config dir is `~/.config/wayland-core` by default and whatever
    /// `WAYLAND_HOME` says otherwise. Delete the candidate rule and this test
    /// fails with the component rule fully intact.
    #[test]
    #[serial_test::serial]
    fn a_cd_into_an_authority_directory_that_is_not_named_wayland() {
        let prior = std::env::var_os("WAYLAND_HOME");
        // SAFETY: test-only env mutation, serialized against this crate's
        // other env-driven tests.
        unsafe { std::env::set_var("WAYLAND_HOME", "/tmp/wl693-store") };
        let two_step = floor_refusal(
            "cd /tmp && cd wl693-store && echo 'tools.auto_approve = true' >> config.toml",
            Some(Path::new("/work")),
        );
        let direct = floor_refusal(
            "echo 'tools.auto_approve = true' >> /tmp/wl693-store/config.toml",
            Some(Path::new("/work")),
        );
        let ordinary = floor_refusal(
            "cd /tmp && cd wl693-elsewhere && echo x >> config.toml",
            Some(Path::new("/work")),
        );
        unsafe {
            match prior {
                Some(v) => std::env::set_var("WAYLAND_HOME", v),
                None => std::env::remove_var("WAYLAND_HOME"),
            }
        }
        // Known-positive control: this store IS protected here, by resolved
        // path, with no help from any name rule.
        assert!(
            direct.is_some(),
            "control: the resolved store must be refused"
        );
        assert!(
            two_step.is_some(),
            "a two-step cd reaches the same file and must be refused"
        );
        // ...and the same two-step shape into an UNPROTECTED directory is not
        // refused, so the arm above is not passing because every `cd` is.
        assert_eq!(
            ordinary, None,
            "the candidate rule must not refuse ordinary work"
        );
    }

    /// The component rule yields in the same layout rule 2b yields in, and for
    /// the same reason: a session LAUNCHED inside a `.wayland` directory would
    /// otherwise have every absolute path it can name refused.
    #[test]
    fn the_component_rule_yields_to_a_session_launched_inside_one() {
        let inside = PathBuf::from("/srv/run/.wayland/work");
        // Control: from an ordinary workspace the component rule IS live on
        // this exact path, so the `None` below is the yield and not a floor
        // that never fired.
        assert!(
            floor_refusal(
                "cat /srv/run/.wayland/work/notes.txt",
                Some(Path::new("/work"))
            )
            .is_some(),
            "control: the component rule must refuse this from outside"
        );
        assert_eq!(
            floor_refusal("cat /srv/run/.wayland/work/notes.txt", Some(&inside)),
            None,
            "a session launched inside a .wayland directory must still work"
        );
        // Rules 1 and 2a are untouched by the yield.
        assert!(floor_refusal("cat /srv/run/.wayland/permissions.toml", Some(&inside)).is_some());
        assert!(
            floor_refusal(
                "echo id > /srv/run/.wayland/work/.git/hooks/pre-commit",
                Some(&inside)
            )
            .is_some()
        );
    }

    /// `.wayland-out` is the session OUTPUT root — the one directory under the
    /// workspace the agent must be able to write. A component rule that
    /// matched it by prefix would break every skill artifact and every spilled
    /// tool result.
    #[test]
    fn the_session_output_root_is_not_an_authority_directory() {
        for command in [
            "cat .wayland-out/session/x.txt",
            "mkdir -p .wayland-out/skills",
            "cd .wayland-out && ls",
            "cd /work/.wayland-out && cat spill.txt",
            "ls .wayland-core-notes.md",
        ] {
            assert_eq!(
                floor_refusal(command, Some(Path::new("/work"))),
                None,
                "must NOT be refused: {command}"
            );
        }
    }

    // Serialized for the same reason as
    // `rule_2b_yields_where_the_workspace_is_inside_the_authority_directory`:
    // `wayland_config_dir()` / `profile_home()` read `WAYLAND_HOME`.
    #[test]
    #[serial_test::serial]
    fn the_resolved_config_dir_is_protected_without_the_bare_name_rule() {
        // `config.toml` is Cargo's own basename, so it is protected by
        // RESOLVED PATH only. This is the arm that grades rule 2b on its own:
        // drop the resolved-path check and the bare-name rule does not cover it.
        let target = crate::config::wayland_config_dir().join("config.toml");
        // QUOTED, because that is the only spelling a shell actually executes
        // when the path contains a space — and on macOS the default config dir
        // is inside `Library/Application Support`. Unquoted, this command
        // writes to `.../Library/Application`, reaches nothing protected, and
        // the floor is right not to refuse it; asserting the bare spelling made
        // this test pass on Linux and Windows for a reason that had nothing to
        // do with the rule it grades, and fail on macOS for one that had
        // nothing to do with the rule either.
        let command = format!(
            "echo 'security.enabled = false' >> \"{}\"",
            target.display()
        );
        assert!(
            floor_refusal(&command, None).is_some(),
            "the global config.toml carries security.enabled and tools.auto_approve"
        );
        let oauth = crate::config::profile_home()
            .join("oauth")
            .join("token.json");
        assert!(floor_refusal(&format!("cat \"{}\"", oauth.display()), None).is_some());
    }

    // `profile_home()` reads `WAYLAND_HOME`, which this module's env-driven
    // tests set and restore globally, so this test has to take the same lock
    // they do. Without it the read races them: the base becomes another
    // test's `/tmp/wl693-store`, the whole-directory rule stops matching
    // `/work`, and the control assertion at the top fails with
    // `control: the whole-directory rule must refuse this from outside`.
    // `cargo nextest` process-isolates every test and cannot see this, so the
    // gate is green while plain `cargo test` fails intermittently.
    #[test]
    #[serial_test::serial]
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

        // The yield's own residual hole, which the first form of it left open:
        // from INSIDE the layout, a `cd` into a subdirectory plus a relative
        // `..` reached the entry list without naming it. The
        // candidate-directory rule closes it.
        assert!(
            floor_refusal(
                &format!("cd {}/logs && echo x >> ../config.toml", base.display()),
                inside
            )
            .is_some(),
            "a cd + relative `..` must not reach the entry list"
        );

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

    /// The hotfix arm. `.wayland-core/skills/**` is where every project
    /// skill's executable lives. Refusing the `.wayland-core` component at
    /// any depth made every skill that ships an executable unrunnable —
    /// `cd`, `find` and `node <path>` alike — and the refusal did not fail
    /// cleanly: the model staged the skill under `/tmp` and told the user it
    /// could not run the brief.
    #[test]
    fn skill_scripts_under_wayland_core_are_runnable() {
        for command in [
            "node .wayland-core/skills/foo/run.js",
            "cd .wayland-core/skills",
            "find .wayland-core/skills -name '*.sh'",
            "bash .wayland-core/skills/tide/scripts/build.sh",
            "cat /work/.wayland-core/skills/foo/SKILL.md",
            "cp -R .wayland-core/skills/foo /tmp/staged",
            // Through the same normalization every other rule uses.
            "ls .wayland-core/./skills",
            "ls .wayland-core/x/../skills",
        ] {
            assert_eq!(
                floor_refusal(command, Some(Path::new("/work"))),
                None,
                "must NOT be refused: {command}"
            );
        }
    }

    /// The security half of the same change, and the half that matters more.
    /// The allowlist is one entry wide; every other child of `.wayland-core`
    /// — including one nobody has invented yet — must still be refused.
    #[test]
    fn the_wayland_core_control_surface_stays_refused() {
        for command in [
            "echo x > .wayland-core/config.toml",
            // `..` is resolved BEFORE the child is read, so a token that
            // enters through `skills` and leaves again is not a skill path.
            "cat .wayland-core/skills/../config.toml",
            "cp /tmp/evil .wayland-core/agents/evil.yaml",
            "cp /tmp/evil .wayland-core/plugins/x.so",
            "cat .wayland-core/shadow/x",
            "cat .wayland-core/skills-audit.json",
            "rm -rf .wayland-core",
            "du -sh .wayland-core",
            "cat .wayland-core.toml",
            "echo x > /home/u/other/.wayland-core/config.toml",
            // A child that is a glob is not definitely `skills`: `sk*` also
            // expands onto `skills-audit.json`, so the allow side matches by
            // exact name only.
            "cp /tmp/evil .wayland-core/sk*",
            // A control surface added next month, with no code change here.
            "echo x > .wayland-core/newcontrolsurface/x",
            // The whole-component glob still reaches the directory itself.
            "cp -r /tmp/evil .wayland-cor*",
            // Only the LITERAL directory is qualified by the allowlist. A glob
            // standing in for it names no directory in particular, so it may
            // not be admitted by the child that follows it.
            "cp -r /tmp/evil .wayland-cor*/skills/x",
            // `.wayland-core.toml` is a separate literal and is never
            // qualified, whatever follows it.
            "cat .wayland-core.toml/skills",
        ] {
            assert!(
                floor_refusal(command, Some(Path::new("/work"))).is_some(),
                "must be refused: {command}"
            );
        }
    }

    // ── #355 — disclosure ──────────────────────────────────────────────────

    /// The spelling of the marker is duplicated in
    /// `crates/wcore-agent/tests/floor_refusal_reaches_the_user.rs`, which
    /// spells it locally so its red arm compiles against a tree without the
    /// fix. This pins the two together; if the constant is renamed, this
    /// fails rather than the e2e test going quietly vacuous.
    #[test]
    fn the_policy_marker_spelling_is_pinned_to_its_e2e_test() {
        assert_eq!(POLICY_REFUSAL_MARKER, "[POLICY-REFUSAL: command-floor]");
    }

    #[test]
    fn every_refusal_carries_the_marker_and_the_stop_instruction() {
        for command in [
            "echo x > .git/hooks/pre-commit",
            "cat .wayland-core/config.toml",
            "cp /tmp/evil ~/.wayland/permissions.toml",
        ] {
            let refusal = floor_refusal(command, Some(Path::new("/work")))
                .unwrap_or_else(|| panic!("must be refused: {command}"));
            assert!(
                refusal.contains(POLICY_REFUSAL_MARKER),
                "no policy marker in the payload for {command}: {refusal}"
            );
            assert!(
                refusal.contains("Do NOT work around it"),
                "no stop instruction in the payload for {command}: {refusal}"
            );
            assert!(
                is_policy_refusal(&refusal),
                "the predicate must recognise its own payload: {refusal}"
            );
        }
    }

    /// The rule stays the FIRST line, because `policy_refusal_notice` quotes
    /// that line verbatim to the user rather than paraphrasing a policy.
    #[test]
    fn the_rule_is_the_first_line_and_the_notice_quotes_it() {
        let refusal =
            floor_refusal("echo x > .git/hooks/pre-commit", Some(Path::new("/work"))).unwrap();
        let first = refusal.lines().next().unwrap();
        assert!(first.starts_with("Refused by the command floor:"));
        assert!(first.contains("repository control surface"));

        let notice = policy_refusal_notice("Bash", &refusal).expect("a policy refusal");
        assert!(notice.contains("Blocked by policy"), "{notice}");
        assert!(notice.contains("`Bash`"), "{notice}");
        assert!(
            notice.contains(first),
            "the notice must quote the rule: {notice}"
        );
    }

    /// The polarity that makes the marker mean anything: an ordinary tool
    /// failure is not a policy refusal and raises no notice.
    #[test]
    fn a_transient_failure_is_not_a_policy_refusal() {
        for text in [
            "bash: wl: command not found",
            "error: exit code 1",
            "Refused by the credential denylist: this command reads a secret",
        ] {
            assert!(!is_policy_refusal(text), "{text}");
            assert!(policy_refusal_notice("Bash", text).is_none(), "{text}");
        }
    }
}
