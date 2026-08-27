//! The non-bypassable command floor (#693).
//!
//! `--dangerously-skip-permissions-and-sandbox` removes the approval prompt
//! AND the OS sandbox. Until this module existed nothing at all sat underneath
//! it: with the sandbox gone, `BashTool` would author
//! `<root>/.git/hooks/pre-commit` — arbitrary code execution on the operator's
//! next commit — and overwrite the profile's learned-grant store, both of which
//! the in-process file tools refuse.
//!
//! The cause is recorded in the tree already, at
//! `wcore_tools::workspace_policy::WorkspacePolicy::check_write_grantable`:
//!
//! > a write-deny inside a granted root would have to be expressed to the OS
//! > sandbox, and `SandboxManifest` has no `fs_write_deny` — so it would hold
//! > only for the in-process file tools and fail open for `Bash`, which is two
//! > answers to one question.
//!
//! This is the second answer, given in-process so it survives the loss of the
//! first one.
//!
//! # This is a WRITE deny
//!
//! The invariant is stated verbatim in `workspace_policy.rs`, at the predicate
//! this floor is the shell-side copy of:
//!
//! > The predicate for a WRITE deny, never a read deny. Reading `.git/HEAD` and
//! > loading `.wayland-core/skills/**` are ordinary session work; what must not
//! > happen is the model AUTHORING those bytes.
//!
//! A first revision of this module ignored that and matched every path token
//! regardless of verb. Measured, that refused eight ordinary commands —
//! `ls .wayland-core/skills`, `cat .git/hooks/pre-commit`,
//! `git config --file .git/config --list`, `grep -rn x .wayland-core`, and a
//! `git commit -m "fix .git/config parsing"` whose protected-looking token was
//! inside a COMMIT MESSAGE — with no override, in the DEFAULT posture, for
//! every user. A floor that refuses reads has not floored the flag, it has
//! broken the product; and the refusal string it printed cited "the same denial
//! the Write and Edit tools already make", a precedent that does not exist in
//! the read direction.
//!
//! So the rules below fire only on a token this module has classified as
//! something the command would AUTHOR: a redirection target, or an operand of a
//! program whose job is to create, overwrite, move or delete a file. Reads pass.
//!
//! The one exception is narrow and is justified against a read-deny precedent
//! that DOES exist: `workspace_policy`'s `fs_read_deny` already names the
//! credential stores, and `SandboxManifest` carries that list to the OS. When
//! the sandbox is gone, that list is gone with it, so the floor keeps exactly
//! it — `credentials.toml`, `credentials.enc`, `credentials.kdf.json`, `oauth/`
//! — plus the profile root named as a whole, which is the wholesale-copy shape
//! (`tar cf - ~/.wayland | base64`) that names no leaf at all. Nothing else is
//! read-denied.
//!
//! # Why this lives in `wcore-config`
//!
//! It was written in `wcore-tools`, next to the one shell surface anybody had
//! thought about. There are **two**. `wcore_skills::shell::execute_shell_commands`
//! runs a skill's `` !`…` `` directive under `sh -c` on a path that never
//! touches `BashTool` — and `wcore-skills` does not, and must not, depend on
//! `wcore-tools`.
//!
//! Leaving the floor up there was not merely an unhardened second path. It was
//! a complete two-step bypass of the floor itself, both steps ordinary agent
//! actions:
//!
//!   1. `BashTool` writes `<WAYLAND_HOME>/skills/x/SKILL.md`. The floor does
//!      not refuse this — `skills` is not an authority leaf, and a protected
//!      root matches only by exact equality, never as a prefix.
//!   2. The next session loads that skill, and its shell directive reaches
//!      precisely the authority state step 1 was forbidden to touch.
//!
//! `wcore-config` is the lowest crate both shell surfaces already depend on,
//! and it is where every path the protected set resolves through
//! (`profile_home`, `wayland_config_dir`) already lives. A floor enforced on
//! one of two shell paths is not a floor, so it belongs beneath both.
//!
//! # What is in the floor, and why it is this small
//!
//! Exactly two rules, both of which are EXISTING refusals this codebase already
//! makes somewhere else and that a disabled sandbox silently revokes:
//!
//! 1. **Repository control surface.** `WorkspacePolicy::is_repo_control_path`
//!    write-denies `<root>/.git` and `<root>/.wayland-core` for every
//!    in-process file tool, on the reasoning that a write of
//!    `.git/hooks/pre-commit` is code execution and a write of
//!    `.wayland-core/skills/x/SKILL.md` is instruction injection into the next
//!    session. The shell was never asked the question.
//!
//!    The floor asks it HOST-WIDE, where `is_repo_control_path` asks it only of
//!    `<root>/…`. That divergence is deliberate and was forced by measurement:
//!    the workspace-scoped form did not fire on the red arm at all, because the
//!    only thing keeping a command away from some OTHER repository's hooks was
//!    the sandbox, and the floor exists precisely for when the sandbox is gone.
//!    "A `.git` elsewhere on the host is not this policy's business" is sound
//!    when a policy IS in force; it is an open door when none is. The cost is
//!    near zero — no legitimate workflow has the shell author any repository's
//!    hooks — and `git`'s own porcelain never names these paths, so it is
//!    untouched.
//!
//! 2. **The agent's own authority state.** The files that decide what the agent
//!    may do without asking: the learned-grant store (`permissions.toml`), the
//!    global config (which carries `security` and `tools` policy), the
//!    workspace-trust ledger, and the credential stores. The WRITE side of this
//!    was denied nowhere at all, and it is the sharpest edge in the issue: a
//!    shell command that appends a rule to `permissions.toml` has disabled the
//!    very guard it was running under, permanently and for every future
//!    session. The read side is the narrow `fs_read_deny` carry-over described
//!    above.
//!
//! Deliberately NOT in the floor:
//!
//! * **Reading any of it.** See above. `cat .git/config`, `ls .git/hooks`,
//!   `grep -rn x .wayland-core` and `cat ~/.wayland/permissions.toml` all run.
//! * **Writing outside every declared root.** That is precisely what the flag
//!   buys, and refusing it would be refusing the flag.
//! * **Destroying the user's only copy of unsaved work.** Already floored, and
//!   already non-waivable — see `wcore_tools::bash::bounded_unsaved_shell_refusal`,
//!   which every `BashTool` entry point calls unconditionally and which fails
//!   CLOSED on expiry. Graded by `wcore-tools/tests/command_floor_test.rs`
//!   rather than reimplemented here.
//! * **Credential-value exfiltration through the environment.** Already floored
//!   and already non-waivable — `wcore_tools::bash::policy::check_denylist`.
//! * **`rm -rf <root>/.git`.** Destroying a repository is a data-loss question,
//!   which the unsaved-work guard owns, not an authority question. Putting it
//!   here would mean matching a token that is an ANCESTOR of the protected
//!   surface, and the shortest such token is `.` — which would refuse
//!   `git add .`. The floor may not cost that.
//!
//! # What this is, and what it is not
//!
//! This matches on the command STRING, so it has the same standing as
//! [`deobfuscate`] says the credential denylist has: **defense in depth, not a
//! security boundary.** A caller with unbounded obfuscation budget
//! (`$(printf ...)`, variable indirection, a symlink planted by an earlier
//! command) can express a protected path in a form no string match sees.
//!
//! Classifying by verb widens that gap in one specific, named way: a write
//! performed by a program this module does not recognise as a writer —
//! `python -c "open('.git/hooks/pre-commit','w')…"`, `nvim .git/config`, a
//! shell function — is not seen. The alternative considered was an allowlist of
//! READERS with everything else denied, which closes that gap and re-opens the
//! cost one: every unrecognised program naming a protected path is refused,
//! including the ones that only read it, which is the exact failure this
//! revision exists to remove. The adversary the floor is built for is an
//! injected or confused model emitting plausible shell, and that adversary's
//! plausible shell is `echo … > …`, `rm`, `cp`, `sed -i` — which are seen.
//!
//! It is, however, genuinely **non-waivable**: no flag, no environment
//! variable and no configuration field reaches it. That is asserted
//! structurally as well as behaviourally, in
//! `wcore-tools/tests/command_floor_test.rs` and
//! `wcore-skills/tests/skill_shell_command_floor.rs`.

use std::path::{Component, Path, PathBuf};

/// Refusal for rule 1. Names the alternative, so a legitimate caller is not
/// left without a route.
pub const REPO_CONTROL: &str = "Refused by the command floor: this command would WRITE a repository \
     control surface (`.git/hooks`, `.git/config`, `.wayland-core`), whose \
     bytes are executed or obeyed rather than merely read. Authoring them is \
     code execution on the next commit, or instruction injection into the next \
     session. This refusal has no override — it is the same write-denial the \
     Write and Edit tools already make, asked of the shell so that turning the \
     sandbox off does not revoke it. Reading these paths is not refused. To \
     change them, use `git config` / `git hook`, or ask the user to make the \
     edit.";

/// Refusal for rule 2, write direction.
pub const AGENT_AUTHORITY: &str = "Refused by the command floor: this command would WRITE Wayland's own \
     authority state (the learned-permission store, the global config, the \
     workspace-trust ledger, or a credential store). Those files decide what \
     this agent may do without asking, so a command that edits them has \
     disabled the guard it is running under. This refusal has no override, and \
     it applies only to writing — reading them is not refused. Ask the user to \
     make the change, or use `wayland-core config` / the approval prompt.";

/// Refusal for rule 2, read direction — deliberately narrow.
///
/// Fires only for the credential stores that `workspace_policy`'s
/// `fs_read_deny` already names, and for a profile root named as a whole.
pub const AUTHORITY_READ: &str = "Refused by the command floor: this command reads a credential store \
     inside Wayland's own authority state, or copies that profile wholesale. \
     Those exact files are already read-denied to the OS sandbox via \
     `fs_read_deny`; this floor keeps that denial when the sandbox is turned \
     off. This refusal has no override. It does NOT extend to the rest of the \
     profile — the permission store, the config and the trust ledger may be \
     read. Ask the user for a credential rather than reading its store.";

/// The leaf names, under a profile root, whose WRITE carries authority.
///
/// `permissions.toml` is the learned-grant store
/// (`wcore_permissions::LearnedPolicy::default_path`). `config.toml` is the
/// global config ([`crate::config::global_config_path`]), the ONLY layer
/// from which the security and auto-approval policy is honoured —
/// deliberately, because a project file travels with a cloned repository.
/// `workspace-trust.json` is the trust ledger
/// ([`crate::workspace_trust::WorkspaceTrustStore`]). The rest are the
/// credential stores.
const AUTHORITY_LEAVES: &[&str] = &[
    "permissions.toml",
    "config.toml",
    "workspace-trust.json",
    "credentials.toml",
    "credentials.enc",
    "credentials.kdf.json",
    "oauth",
];

/// The subset of [`AUTHORITY_LEAVES`] that is read-denied as well.
///
/// This list is not a judgement of this module's own: it is
/// `workspace_policy`'s `fs_read_deny` set, which `SandboxManifest` hands to
/// the OS backend. The floor exists for the case where there is no backend to
/// hand it to.
const CREDENTIAL_LEAVES: &[&str] = &[
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
        crate::config::profile_home(),
        crate::config::wayland_config_dir(),
    ];
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".wayland"));
    }
    // Canonicalized spellings too: on macOS `~` is frequently reached through
    // `/private/var/...` vs `/var/...`, and a protected path compared in only
    // one spelling is not compared at all.
    let mut forms = roots.clone();
    for r in &roots {
        if let Ok(c) = std::fs::canonicalize(r)
            && !forms.contains(&c)
        {
            forms.push(c);
        }
    }
    forms.sort();
    forms.dedup();
    forms
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
/// `.git/config` is on the list because it is write-to-RCE: `core.fsmonitor`,
/// `core.sshCommand`, and the `[alias]` table all name programs git then runs.
/// It is NOT on it for being a credential store, because that would be a read
/// argument and this predicate no longer answers read questions.
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

/// Path-shaped tokens from a COMMAND.
///
/// Deliberately far more permissive than `wcore_tools`' `is_path_token`, and
/// the asymmetry is the whole design. In the FAILURE path a stray token becomes
/// a fabricated accusation, so a token must look path-shaped before it is
/// trusted. Here a token survives only if it matches a deny list, so that match
/// IS the filter — which is what lets a bare filename through. `cat secret.txt`
/// has no interior separator and would be dropped by `is_path_token`, and it is
/// precisely the case wayland#1078 is about.
///
/// Lives here rather than in `wcore-tools` because it is shared with the
/// masked-read annotation in `wcore_tools::bash::policy`, which sits above this
/// crate. The command floor does NOT use it: splitting on quote characters is
/// what turned a path inside `git commit -m "fix .git/config parsing"` into a
/// path token, so the floor parses with [`parse_segments`] instead.
pub fn command_path_tokens(command: &str) -> Vec<&str> {
    command
        .split(|c: char| {
            c.is_whitespace()
                || matches!(
                    c,
                    '\'' | '"' | '`' | ';' | '|' | '&' | '(' | ')' | '<' | '>'
                )
        })
        .map(|token| token.trim_matches(|c| matches!(c, ',' | ']' | '[')))
        .filter(|token| token.len() >= 2 && !token.contains("://") && !token.starts_with('-'))
        .collect()
}

/// tools-exec-14/16: best-effort de-obfuscation of trivial shell quoting
/// tricks before a string-matching guard runs. A model (or prompt-injection
/// payload) can dodge a literal regex with shell forms that the shell
/// collapses back at parse time but that the raw regex misses: `e''nv`,
/// `e""nv`, `e\nv`, `"env"`, `'env'`. We strip empty quote pairs,
/// backslash-escapes of ordinary chars, and surrounding quotes from each
/// word so the SAME pattern set sees the post-collapse token.
///
/// This is **defense-in-depth only** — it does NOT make the caller a security
/// boundary. A determined attacker has unbounded obfuscation
/// (`$(printf '\145nv')`, variable indirection, base64-decode-then-eval,
/// runtime path expansion). The real boundaries are the secret-scrubbed
/// sandbox env and the default-Deny network policy; this layer just raises the
/// cost of the cheapest one-liner bypasses.
///
/// Used by `wcore_tools::bash::policy::check_denylist`. The command floor does
/// not call it: [`parse_segments`] collapses the same quoting by construction,
/// and preserves the word boundaries this function destroys.
pub fn deobfuscate(command: &str) -> String {
    let mut out = String::with_capacity(command.len());
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            // Empty quote pair: `''` / `""` — shell collapses to nothing.
            '\'' | '"' if chars.peek() == Some(&c) => {
                chars.next(); // consume the closing quote, emit nothing
            }
            // Lone surrounding quote — drop it so `"env"` -> `env`.
            '\'' | '"' => {}
            // Backslash-escape of an ordinary char (`e\nv` -> `env`). Keep
            // the escaped char only; never the backslash. We do not try to
            // interpret C-style escapes — `\n` here is a literal `n` to the
            // shell outside of `$'...'`, which is the case we are hardening.
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

// ---------------------------------------------------------------------------
// Parsing: words, not substrings
// ---------------------------------------------------------------------------

/// One simple command from a pipeline or list, with its words already
/// unquoted and its write-redirection targets kept separately.
#[derive(Debug, Default)]
struct Segment {
    /// Every word of the command, quoting collapsed. A quoted run is ONE word
    /// however many spaces it contains — which is what keeps a path inside a
    /// commit message from becoming a path token.
    words: Vec<String>,
    /// Targets of `>`, `>>`, `>|`, `N>`, `&>`. Unconditionally written.
    redirect_writes: Vec<String>,
}

/// Accumulator for [`parse_segments`].
#[derive(Default)]
struct Lexer {
    segments: Vec<Segment>,
    cur: Segment,
    word: String,
    have_word: bool,
    /// The next word is the target of a write redirection.
    redirect_pending: bool,
}

impl Lexer {
    fn push(&mut self, c: char) {
        self.word.push(c);
        self.have_word = true;
    }

    fn flush_word(&mut self) {
        if !self.have_word {
            return;
        }
        let w = std::mem::take(&mut self.word);
        self.have_word = false;
        if self.redirect_pending {
            self.cur.redirect_writes.push(w);
        } else {
            self.cur.words.push(w);
        }
        self.redirect_pending = false;
    }

    fn end_segment(&mut self) {
        self.flush_word();
        self.redirect_pending = false;
        if !self.cur.words.is_empty() || !self.cur.redirect_writes.is_empty() {
            self.segments.push(std::mem::take(&mut self.cur));
        }
    }

    /// A bare descriptor number immediately before a redirection operator is
    /// part of the operator, not a word: `echo 1>x` writes `x`.
    fn drop_descriptor(&mut self) {
        if self.have_word && !self.word.is_empty() && self.word.chars().all(|c| c.is_ascii_digit())
        {
            self.word.clear();
            self.have_word = false;
        }
    }
}

/// Arm the redirection target, unless the redirection is a descriptor dup
/// (`2>&1`, `>&2`) — in which case what follows is a number, not a file, and
/// there is nothing to classify. Returns the new scan position.
fn begin_redirect_write(lexer: &mut Lexer, src: &[char], i: usize) -> usize {
    let mut j = i;
    while j < src.len() && src[j].is_whitespace() {
        j += 1;
    }
    if j < src.len() && src[j] == '&' {
        j += 1;
        while j < src.len() && (src[j].is_ascii_digit() || src[j] == '-') {
            j += 1;
        }
        lexer.redirect_pending = false;
        return j;
    }
    lexer.redirect_pending = true;
    i
}

/// Split `command` into simple commands, collapsing shell quoting as the shell
/// would and keeping word boundaries as the shell would.
///
/// This is a lexer, not a shell: it does not evaluate substitutions, and it
/// treats `` ` ``, `$(`, `(` and `)` as boundaries so the text inside them is
/// analysed as its own command rather than swallowed into the enclosing one.
fn parse_segments(command: &str) -> Vec<Segment> {
    let src: Vec<char> = command.chars().collect();
    let mut lexer = Lexer::default();
    let mut i = 0usize;

    while i < src.len() {
        let c = src[i];
        match c {
            '\'' => {
                i += 1;
                lexer.have_word = true;
                while i < src.len() && src[i] != '\'' {
                    lexer.push(src[i]);
                    i += 1;
                }
                i = (i + 1).min(src.len());
            }
            '"' => {
                i += 1;
                lexer.have_word = true;
                while i < src.len() && src[i] != '"' {
                    if src[i] == '\\' && i + 1 < src.len() {
                        lexer.push(src[i + 1]);
                        i += 2;
                    } else {
                        lexer.push(src[i]);
                        i += 1;
                    }
                }
                i = (i + 1).min(src.len());
            }
            '\\' => {
                if i + 1 < src.len() {
                    lexer.push(src[i + 1]);
                    i += 2;
                } else {
                    i += 1;
                }
            }
            c if c.is_whitespace() => {
                lexer.flush_word();
                i += 1;
            }
            ';' => {
                lexer.end_segment();
                i += 1;
            }
            '|' => {
                lexer.end_segment();
                i += 1;
                if i < src.len() && src[i] == '|' {
                    i += 1;
                }
            }
            '&' if i + 1 < src.len() && src[i + 1] == '>' => {
                lexer.flush_word();
                i += 2;
                if i < src.len() && src[i] == '>' {
                    i += 1;
                }
                i = begin_redirect_write(&mut lexer, &src, i);
            }
            '&' => {
                lexer.end_segment();
                i += 1;
                if i < src.len() && src[i] == '&' {
                    i += 1;
                }
            }
            '>' => {
                lexer.drop_descriptor();
                lexer.flush_word();
                i += 1;
                if i < src.len() && (src[i] == '>' || src[i] == '|') {
                    i += 1;
                }
                i = begin_redirect_write(&mut lexer, &src, i);
            }
            '<' => {
                lexer.drop_descriptor();
                lexer.flush_word();
                i += 1;
                while i < src.len() && src[i] == '<' {
                    i += 1;
                }
            }
            '`' | '(' | ')' => {
                lexer.end_segment();
                i += 1;
            }
            '$' if i + 1 < src.len() && src[i + 1] == '(' => {
                lexer.end_segment();
                i += 2;
            }
            other => {
                lexer.push(other);
                i += 1;
            }
        }
    }
    lexer.end_segment();
    lexer.segments
}

// ---------------------------------------------------------------------------
// Classification: which words would be AUTHORED
// ---------------------------------------------------------------------------

/// Programs that stand in front of the real one and must be stepped over.
const WRAPPERS: &[&str] = &[
    "sudo", "doas", "env", "nohup", "command", "exec", "time", "nice", "ionice", "stdbuf", "xargs",
];

/// Programs whose every non-flag operand is created, overwritten or removed.
///
/// `mv` is here rather than with the destination-only group because its SOURCE
/// is unlinked, which is a write of that path.
const MUTATES_EVERY_OPERAND: &[&str] = &[
    "rm", "rmdir", "unlink", "shred", "truncate", "touch", "mkdir", "chmod", "chown", "chgrp",
    "tee", "mv", "mktemp",
];

/// Programs that write only their destination; the other operands are read.
const MUTATES_DESTINATION: &[&str] = &["cp", "ln", "install", "rsync"];

/// `git config` selectors that make the invocation a query.
const GIT_QUERY_SELECTORS: &[&str] = &[
    "--list",
    "-l",
    "--get",
    "--get-all",
    "--get-regexp",
    "--get-urlmatch",
    "--get-color",
    "--get-colorbool",
];

/// `git`'s own options that consume the next word, so the word after them is
/// not the subcommand.
const GIT_VALUE_OPTIONS: &[&str] = &[
    "-C",
    "-c",
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--exec-path",
    "--super-prefix",
];

fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

fn is_flag(word: &str) -> bool {
    word.starts_with('-') && word != "-"
}

/// `NAME=value` in front of a command is an environment assignment, not the
/// program.
fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        None => false,
    }
}

/// The operand of `flag` (either `--flag value` or `--flag=value`).
fn option_values(words: &[&str], names: &[&str]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < words.len() {
        let w = words[i];
        if names.contains(&w) {
            if let Some(next) = words.get(i + 1) {
                out.push((*next).to_string());
            }
            i += 2;
            continue;
        }
        if let Some((name, value)) = w.split_once('=')
            && names.contains(&name)
        {
            out.push(value.to_string());
        }
        i += 1;
    }
    out
}

/// A short-option cluster carrying `i` — `-i`, `-i.bak`, `-pi`.
fn has_in_place(words: &[&str]) -> bool {
    words.iter().any(|w| {
        *w == "--in-place"
            || (w.starts_with('-')
                && !w.starts_with("--")
                && w.chars()
                    .skip(1)
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .any(|c| c == 'i'))
    })
}

fn git_subcommand<'a>(words: &[&'a str]) -> Option<&'a str> {
    let mut i = 0usize;
    while i < words.len() {
        let w = words[i];
        if GIT_VALUE_OPTIONS.contains(&w) {
            i += 2;
            continue;
        }
        if is_flag(w) {
            i += 1;
            continue;
        }
        return Some(w);
    }
    None
}

/// `git` names a path only through an option, never as a bare operand: a bare
/// operand is a key, a value, a pathspec or a ref. `git config --file X k v`
/// authors `X`; `git config --file X --list` reads it.
fn git_write_targets(words: &[&str]) -> Vec<String> {
    if git_subcommand(words) != Some("config") {
        return Vec::new();
    }
    if words.iter().any(|w| GIT_QUERY_SELECTORS.contains(w)) {
        return Vec::new();
    }
    option_values(words, &["--file", "-f", "--blob"])
}

/// The words of `segment` that the command would AUTHOR.
///
/// Everything else in the segment is a read as far as this module is concerned.
/// See the module note for the named gap this leaves and why it is preferred to
/// the alternative.
fn write_targets(segment: &Segment) -> Vec<String> {
    let mut out = segment.redirect_writes.clone();

    let mut words = segment.words.iter();
    let mut program: Option<&str> = None;
    for w in words.by_ref() {
        if is_assignment(w) || WRAPPERS.contains(&basename(w)) {
            continue;
        }
        program = Some(basename(w));
        break;
    }
    let Some(program) = program else {
        return out;
    };
    let rest: Vec<&str> = words.map(String::as_str).collect();
    let operands: Vec<&str> = rest.iter().copied().filter(|w| !is_flag(w)).collect();

    if MUTATES_EVERY_OPERAND.contains(&program) {
        out.extend(operands.iter().map(|w| (*w).to_string()));
    } else if MUTATES_DESTINATION.contains(&program) {
        if let Some(last) = operands.last() {
            out.push((*last).to_string());
        }
        out.extend(option_values(&rest, &["-t", "--target-directory"]));
    } else if program == "dd" {
        out.extend(
            rest.iter()
                .filter_map(|w| w.strip_prefix("of="))
                .map(str::to_string),
        );
    } else if matches!(program, "sed" | "perl" | "ruby" | "awk" | "gawk") && has_in_place(&rest) {
        out.extend(operands.iter().map(|w| (*w).to_string()));
    } else if program == "git" {
        out.extend(git_write_targets(&rest));
    }
    out
}

/// Worth resolving as a path at all. Mirrors [`command_path_tokens`]' filter:
/// a single character cannot name anything protected, and `.` in particular
/// must never be resolved, because `git add .` is ordinary work.
fn path_shaped(token: &str) -> bool {
    token.len() >= 2 && !token.contains("://") && !token.starts_with('-')
}

/// The floor. `Some(refusal)` means the command does not run, on any tier,
/// under any configuration, on either shell surface.
///
/// `cwd` is the directory the shell will run in — the workspace root when a
/// `WorkspacePolicy` supplies one, the process directory otherwise, and for a
/// skill the directory that skill's directives are executed in — so a relative
/// token is resolved against the tree the command will really touch.
pub fn check_command_floor(command: &str, cwd: Option<&Path>) -> Option<String> {
    let roots = protected_roots();
    let leaves = |set: &[&str]| -> Vec<PathBuf> {
        let mut out = Vec::with_capacity(roots.len() * set.len());
        for root in &roots {
            for leaf in set {
                let p = root.join(leaf);
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        }
        out
    };
    let authority = leaves(AUTHORITY_LEAVES);
    let credentials = leaves(CREDENTIAL_LEAVES);

    let segments = parse_segments(command);

    // Read side — the narrow `fs_read_deny` carry-over only.
    for segment in &segments {
        for token in segment
            .words
            .iter()
            .chain(segment.redirect_writes.iter())
            .filter(|t| path_shaped(t))
        {
            for form in candidate_forms(token.as_str(), cwd) {
                if credentials.iter().any(|p| under(&form, p)) || roots.iter().any(|r| &form == r) {
                    return Some(AUTHORITY_READ.to_string());
                }
            }
        }
    }

    // Write side — the floor proper.
    for segment in &segments {
        let targets = write_targets(segment);
        for token in targets.iter().filter(|t| path_shaped(t)) {
            for form in candidate_forms(token.as_str(), cwd) {
                if authority.iter().any(|p| under(&form, p)) || roots.iter().any(|r| &form == r) {
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
