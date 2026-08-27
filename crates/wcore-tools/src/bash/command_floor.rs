//! The non-bypassable command floor (#693).
//!
//! `--dangerously-skip-permissions-and-sandbox` removes the approval prompt
//! AND the OS sandbox. Until this module existed nothing at all sat underneath
//! it: with the sandbox gone, `BashTool` would author
//! `<root>/.git/hooks/pre-commit` — arbitrary code execution on the operator's
//! next commit — and print the profile's learned-grant store, both of which the
//! in-process file tools refuse.
//!
//! The cause is recorded in the tree already, at
//! [`WorkspacePolicy::check_write_grantable`](crate::workspace_policy::WorkspacePolicy):
//!
//! > a write-deny inside a granted root would have to be expressed to the OS
//! > sandbox, and `SandboxManifest` has no `fs_write_deny` — so it would hold
//! > only for the in-process file tools and fail open for `Bash`, which is two
//! > answers to one question.
//!
//! This is the second answer, given in-process so it survives the loss of the
//! first one.
//!
//! # What is in the floor, and why it is this small
//!
//! Exactly two rules, both of which are EXISTING refusals this codebase already
//! makes somewhere else and that a disabled sandbox silently revokes:
//!
//! 1. **Repository control surface.**
//!    [`WorkspacePolicy::is_repo_control_path`](crate::workspace_policy::WorkspacePolicy::is_repo_control_path)
//!    write-denies `<root>/.git` and `<root>/.wayland-core` for every
//!    in-process file tool, on the reasoning that a write of
//!    `.git/hooks/pre-commit` is code execution and a write of
//!    `.wayland-core/skills/x/SKILL.md` is instruction injection into the next
//!    session. `Bash` was never asked the question.
//!
//!    The floor asks it HOST-WIDE, where `is_repo_control_path` asks it only of
//!    `<root>/…`. That divergence is deliberate and was forced by measurement:
//!    the workspace-scoped form did not fire on the red arm at all, because the
//!    only thing keeping a command away from some OTHER repository's hooks was
//!    the sandbox, and the floor exists precisely for when the sandbox is gone.
//!    "A `.git` elsewhere on the host is not this policy's business" is sound
//!    when a policy IS in force; it is an open door when none is. The cost is
//!    near zero — no legitimate workflow has the shell author any repository's
//!    hooks — and `git`'s own porcelain (`git config`, `git commit`) never
//!    names these paths, so it is untouched.
//!
//! 2. **The agent's own authority state.** The files that decide what the agent
//!    may do without asking: the learned-grant store (`permissions.toml`), the
//!    global config (which carries `security.enabled` and `tools.auto_approve`),
//!    the workspace-trust ledger, and the credential stores. The read side of
//!    this is already `manifest.fs_read_deny` for the credential files — the
//!    floor is what is left when there is no manifest. The WRITE side was
//!    denied nowhere at all, and it is the sharpest edge in the issue: a shell
//!    command that appends a rule to `permissions.toml` has disabled the very
//!    guard it was running under, permanently and for every future session.
//!
//! Deliberately NOT in the floor:
//!
//! * **Writing outside every declared root.** That is precisely what the flag
//!   buys, and refusing it would be refusing the flag.
//! * **Destroying the user's only copy of unsaved work.** Already floored, and
//!   already non-waivable — see `bounded_unsaved_shell_refusal` in the parent
//!   module, which every `BashTool` entry point calls unconditionally and which
//!   fails CLOSED on expiry. Graded by `tests/command_floor_test.rs` rather
//!   than reimplemented here.
//! * **Credential-value exfiltration through the environment.** Already floored
//!   and already non-waivable — [`check_denylist`](super::policy::check_denylist).
//! * **`rm -rf <root>/.git`.** Destroying a repository is a data-loss question,
//!   which the unsaved-work guard owns, not an authority question. Putting it
//!   here would mean matching a token that is an ANCESTOR of the protected
//!   surface, and the shortest such token is `.` — which would refuse
//!   `git add .`. The floor may not cost that.
//!
//! # What this is, and what it is not
//!
//! This matches on the command STRING, so it has the same standing as
//! [`deobfuscate`](super::policy::deobfuscate) says the credential denylist
//! has: **defense in depth, not a security boundary.** A caller with unbounded
//! obfuscation budget (`$(printf ...)`, variable indirection, a symlink planted
//! by an earlier command) can express a protected path in a form no string
//! match sees. The adversary it is built for is the one that actually exists —
//! an injected or confused model emitting plausible shell — and against that
//! one it is the only thing left standing once the sandbox is off.
//!
//! It is, however, genuinely **non-waivable**: no flag, no environment
//! variable and no configuration field reaches it. That is asserted
//! structurally as well as behaviourally, in `tests/command_floor_test.rs`.

use std::path::{Component, Path, PathBuf};

/// Refusal for rule 1. Names the alternative, so a legitimate caller is not
/// left without a route.
const REPO_CONTROL: &str = "Refused by the command floor: this command names a repository \
     control surface (`.git/hooks`, `.git/config`, `.wayland-core`), \
     which is executed or obeyed rather than merely read. Authoring those bytes \
     is code execution on the next commit, or instruction injection into the \
     next session. This refusal has no override — it is the same denial the \
     Write and Edit tools already make, asked of the shell so that turning the \
     sandbox off does not revoke it. Use `git config` / `git hook` for git's \
     own surface, or ask the user to make the edit.";

/// Refusal for rule 2.
const AGENT_AUTHORITY: &str = "Refused by the command floor: this command names Wayland's own \
     authority state (the learned-permission store, the global config, the \
     workspace-trust ledger, or a credential store). Those files decide what \
     this agent may do without asking, so an agent-issued command may not read \
     or write them — a command that edits them has disabled the guard it is \
     running under. This refusal has no override. Ask the user to make the \
     change, or use `wayland-core config` / the approval prompt.";

/// The leaf names, under a profile root, that carry authority.
///
/// `permissions.toml` is the learned-grant store
/// (`wcore_permissions::LearnedPolicy::default_path`). `config.toml` is the
/// global config (`wcore_config::config::global_config_path`), the ONLY layer
/// from which `security.enabled` and `tools.auto_approve` are honoured —
/// deliberately, because a project file travels with a cloned repository.
/// `workspace-trust.json` is the trust ledger
/// (`wcore_config::workspace_trust::WorkspaceTrustStore`). The rest are the
/// credential stores that `workspace_policy`'s `fs_read_deny` already names.
const AUTHORITY_LEAVES: &[&str] = &[
    "permissions.toml",
    "config.toml",
    "workspace-trust.json",
    "credentials.toml",
    "credentials.enc",
    "credentials.kdf.json",
    "oauth",
];

/// Every profile root whose authority files this floor protects.
///
/// Three, not one, and the third is the point: the DEFAULT `~/.wayland` is
/// included even when `WAYLAND_HOME` points somewhere else. `WAYLAND_HOME` is
/// read from the environment, which this codebase already classifies as
/// untrusted provenance (`default_bash_network_policy`, SEC-11). Resolving the
/// protected set through it alone would mean a launcher that exported
/// `WAYLAND_HOME=/tmp/x` had moved the floor off the operator's real store —
/// an environment variable that disables a floor, which is not a floor.
/// Including the default root as well costs two path joins and closes it.
fn protected_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        wcore_config::config::profile_home(),
        wcore_config::config::wayland_config_dir(),
    ];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".wayland"));
    }
    roots.sort();
    roots.dedup();
    roots
}

/// Lexically normalize `path` — no filesystem access, so it answers for a path
/// that does not exist yet, which is exactly the shape of a hook about to be
/// written.
///
/// Interior `..` is resolved against the accumulated prefix so
/// `<root>/x/../.git/config` and `<root>/.git/config` produce one answer.
fn lexical_normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Expand the path spellings a shell would expand before the command ever
/// reached a process: a leading `~`, and `$HOME` / `$WAYLAND_HOME` in either
/// brace form.
///
/// Not exhaustive, and cannot be — see the module note on what this is. It
/// covers the forms a model actually emits.
fn expand(token: &str) -> String {
    let mut s = token.to_string();
    if let Some(home) = dirs::home_dir() {
        let h = home.to_string_lossy().into_owned();
        if let Some(rest) = s.strip_prefix("~/") {
            s = format!("{h}/{rest}");
        } else if s == "~" {
            s = h.clone();
        }
        s = s.replace("${HOME}", &h).replace("$HOME", &h);
    }
    if let Ok(wh) = std::env::var("WAYLAND_HOME")
        && !wh.chars().any(char::is_control)
    {
        s = s
            .replace("${WAYLAND_HOME}", &wh)
            .replace("$WAYLAND_HOME", &wh);
    }
    s
}

/// Every form of `token` worth comparing: resolved against `cwd` when
/// relative, lexically normalized, and — when the path is really on disk —
/// canonicalized as well, so a symlink already in place cannot smuggle a
/// protected path past a prefix match.
fn candidate_forms(token: &str, cwd: Option<&Path>) -> Vec<PathBuf> {
    let expanded = expand(token);
    let raw = Path::new(&expanded);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        match cwd {
            Some(c) => c.join(raw),
            None => return Vec::new(),
        }
    };
    let normalized = lexical_normalize(&joined);
    let mut forms = vec![normalized.clone()];
    if let Ok(c) = std::fs::canonicalize(&normalized)
        && c != normalized
    {
        forms.push(c);
    }
    // A path that does not exist yet cannot be canonicalized, but its PARENT
    // usually can — which is the shape of `<symlink-to-.git/hooks>/pre-commit`.
    if let (Some(parent), Some(leaf)) = (normalized.parent(), normalized.file_name())
        && let Ok(c) = std::fs::canonicalize(parent)
    {
        let via_parent = c.join(leaf);
        if !forms.contains(&via_parent) {
            forms.push(via_parent);
        }
    }
    forms
}

/// True when `candidate` is at or under `protected`.
fn under(candidate: &Path, protected: &Path) -> bool {
    candidate == protected || candidate.starts_with(protected)
}

/// Repository-control component sequences: a path is on the control surface
/// when its components CONTAIN one of these in order.
///
/// Component-wise, never substring: `foo.git-hooks` and `mywayland-core` are
/// ordinary names and a substring match would refuse them.
const REPO_CONTROL_SEQUENCES: &[&[&str]] = &[
    &[".git", "hooks"],
    &[".git", "config"],
    &[".wayland-core"],
    &[".wayland-core.toml"],
];

/// True when `candidate` names the repository control surface of ANY
/// repository — see the module note on why this is host-wide.
///
/// `.git/config` is on the list for two reasons, not one: it is write-to-RCE
/// (`core.fsmonitor`, `core.sshCommand`, and the `[alias]` table all name
/// programs git then runs), and it is a credential store in its own right
/// whenever a remote is `https://user:token@host`.
fn is_repo_control(candidate: &Path) -> bool {
    let parts: Vec<&str> = candidate
        .components()
        .filter_map(|c| match c {
            Component::Normal(p) => p.to_str(),
            _ => None,
        })
        .collect();
    REPO_CONTROL_SEQUENCES.iter().any(|seq| {
        parts
            .windows(seq.len())
            .any(|w| w.iter().zip(seq.iter()).all(|(a, b)| a == b))
    })
}

/// The floor. `Some(refusal)` means the command does not run, on any tier,
/// under any configuration.
///
/// `cwd` is the directory the shell will run in — the workspace root when a
/// `WorkspacePolicy` supplies one, the process directory otherwise — resolved
/// by the caller exactly as `unsaved_shell_refusal` resolves it, so the two
/// guards cannot disagree about which tree a relative token names.
pub fn check_command_floor(command: &str, cwd: Option<&Path>) -> Option<String> {
    let roots = protected_roots();
    let mut authority: Vec<PathBuf> = Vec::with_capacity(roots.len() * AUTHORITY_LEAVES.len());
    for root in &roots {
        for leaf in AUTHORITY_LEAVES {
            authority.push(root.join(leaf));
        }
    }
    // Canonicalized spellings of the roots too: on macOS `~` is frequently
    // reached through `/private/var/...` vs `/var/...`, and a protected path
    // compared in only one spelling is not compared at all.
    let mut root_forms: Vec<PathBuf> = roots.clone();
    for r in &roots {
        if let Ok(c) = std::fs::canonicalize(r)
            && !root_forms.contains(&c)
        {
            root_forms.push(c);
        }
    }
    for r in &root_forms {
        for leaf in AUTHORITY_LEAVES {
            let p = r.join(leaf);
            if !authority.contains(&p) {
                authority.push(p);
            }
        }
    }

    // Both the raw command and the de-obfuscated form, for the reason
    // `check_denylist` tests both: `e''nv` collapses at shell parse time and a
    // raw match never sees it.
    let deobf = super::policy::deobfuscate(command);
    for variant in [command, deobf.as_str()] {
        for token in super::policy::command_path_tokens(variant) {
            for form in candidate_forms(token, cwd) {
                if authority.iter().any(|p| under(&form, p))
                    || root_forms.iter().any(|r| &form == r)
                {
                    return Some(AGENT_AUTHORITY.to_string());
                }
                if is_repo_control(&form) {
                    return Some(REPO_CONTROL.to_string());
                }
            }
        }
    }
    None
}
