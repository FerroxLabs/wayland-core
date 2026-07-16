use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use async_trait::async_trait;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use tokio::sync::{Mutex, oneshot};
use tokio::task::JoinHandle;
use tokio::time::{Duration, timeout};
use tracing::{debug, error, warn};
use wcore_config::shell::{McpStdioLaunchContext, mcp_stdio_command_builder};

use super::{McpError, McpTransport};
use crate::protocol::{JsonRpcRequest, JsonRpcResponse};

/// Per-request timeout for stdio JSON-RPC calls (audit C1).
///
/// `read_line` on a child's stdout only returns when a line arrives or the
/// pipe hits EOF. A child that is alive but silent — deadlocked, stuck on a
/// missing dependency, waiting on stdin it will never get — produces neither,
/// so an unbounded await never returns. 120s is generous enough for a real
/// `tools/call` (network-bound MCP servers, slow tools) yet short enough that
/// a wedged server surfaces as a typed error instead of an infinite hang.
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(120);

/// Maximum bytes for a single line read from the child's stdout/stderr
/// (audit M-12/M-14/mcp-39). The MCP child is explicitly less-trusted
/// (we `env_clear()` secrets from it). A bare `read_line` has no byte cap,
/// so a server streaming an endless newline-free stream would grow the
/// line buffer until the host OOMs — and the reader task runs in its own
/// `tokio::spawn`, independent of the per-request timeout, so the timeout
/// never bounds it. 8 MiB is far larger than any legitimate JSON-RPC line
/// yet small enough to bound memory. On overflow the transport is marked
/// dead (mirrors the timeout/kill path).
const MAX_LINE_BYTES: u64 = 8 * 1024 * 1024;

/// Stdio transport: communicates with an MCP server over a child process's
/// stdin/stdout using newline-delimited JSON-RPC.
///
/// Architecture (audit C1/C3, mirrors `wcore-plugin-subprocess::mcp_bridge`):
/// a background reader task owns stdout and routes every inbound line to the
/// waiting caller by JSON-RPC `id` via a `pending` map. `request()` registers
/// a `oneshot`, writes the line, then awaits its own response under a
/// `timeout`. This removes the head-of-line blocking of the old single global
/// stdout mutex and the response mis-matching of "read the next line".
pub struct StdioTransport {
    stdin: Mutex<BufWriter<ChildStdin>>,
    child: Mutex<Child>,
    /// Stable process-group identity captured at spawn. `Child::id()` becomes
    /// `None` after the direct wrapper exits, while grandchildren can still
    /// be alive, so cleanup must not depend on consulting it later.
    process_group_id: Option<u32>,
    next_id: AtomicU64,
    /// Pending request→response channels, keyed by JSON-RPC id.
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
    /// Background task draining the child's stdout.
    reader_task: Mutex<Option<JoinHandle<()>>>,
    /// Background task draining the child's stderr into the log.
    stderr_task: Mutex<Option<JoinHandle<()>>>,
    /// Cleared when the child is known dead (EOF, read error, or kill).
    /// Audit C4/C7: callers check this to fast-fail instead of re-hanging.
    /// Shared with the reader task, which is the only writer of the
    /// EOF/error transition — so liveness has a single source of truth.
    alive: Arc<AtomicBool>,
    /// Per-request timeout (audit C1). Defaults to [`DEFAULT_RPC_TIMEOUT`];
    /// `spawn_with_timeout` lets tests use a short bound.
    rpc_timeout: Duration,
}

/// Quote a single shell argument so it survives `sh -c` / `cmd /C` parsing.
///
/// Unix branch: wrap in single quotes and escape any internal single quote
/// using the `'\''` idiom (closes the quoted region, escapes one literal `'`,
/// reopens). Always safe for `sh -c "<command>"`.
///
/// Windows branch: see [`windows_cmd_quote`]. The previous implementation
/// wrapped in `"..."` and escaped internal `"` as `\"` — but cmd.exe does NOT
/// honor backslash escapes, so a token containing a literal `"` presented
/// cmd.exe with unbalanced quotes and let trailing `& calc` / `| ...` /
/// `%VAR%` fall outside the quoted region and be interpreted (Aud-32).
fn shell_quote(arg: &str) -> String {
    if cfg!(windows) {
        windows_cmd_quote(arg)
    } else {
        let escaped = arg.replace('\'', "'\\''");
        format!("'{}'", escaped)
    }
}

/// Quote one argument for a `cmd /C "<command_str>"` invocation so that it
/// survives BOTH cmd.exe's parser and the child program's `CommandLineToArgvW`
/// tokenization, with no cmd metacharacter escaping the intended argument.
///
/// Two-phase quoting (per the documented Win32 rules):
///
/// 1. **argv phase** — produce the `CommandLineToArgvW`-correct token: wrap in
///    `"..."`, double every run of backslashes that immediately precedes a `"`
///    (and the trailing run before the closing quote), and escape an internal
///    `"` as `\"`. This is what a Windows child uses to recover the argument.
/// 2. **cmd phase** — cmd.exe processes the command line BEFORE the child sees
///    it and does NOT understand backslash escapes; it only tracks `"`-parity
///    and treats `( ) % ! ^ " < > & |` as metacharacters. Caret-escape each of
///    those so a literal `"` in the argument can never unbalance cmd's quoting
///    and expose trailing metacharacters. cmd strips the `^` before invoking
///    the child, leaving the argv-phase escaping intact.
///
/// `%` is caret-escaped too; cmd's `%VAR%` expansion is suppressed by `^%`
/// (note: `^` does not suppress `%` inside a quoted region, which is exactly
/// why this function caret-escapes the WHOLE token rather than relying on the
/// surrounding quotes).
fn windows_cmd_quote(arg: &str) -> String {
    // Phase 1: CommandLineToArgvW-correct argv quoting.
    let mut argv = String::with_capacity(arg.len() + 2);
    argv.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
            }
            '"' => {
                // Double the preceding backslashes, then escape the quote.
                argv.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                argv.push('"');
                backslashes = 0;
            }
            _ => {
                argv.extend(std::iter::repeat_n('\\', backslashes));
                argv.push(ch);
                backslashes = 0;
            }
        }
    }
    // Trailing backslashes precede the closing quote: double them so the
    // closing quote is not escaped.
    argv.extend(std::iter::repeat_n('\\', backslashes * 2));
    argv.push('"');

    // Phase 2: caret-escape cmd.exe metacharacters across the whole token.
    let mut out = String::with_capacity(argv.len());
    for ch in argv.chars() {
        if matches!(
            ch,
            '(' | ')' | '%' | '!' | '^' | '"' | '<' | '>' | '&' | '|'
        ) {
            out.push('^');
        }
        out.push(ch);
    }
    out
}

/// The executable token for a `cmd /C <line>` command line on Windows.
///
/// Unlike an argument, a program name must NOT be caret-escaped: cmd.exe
/// resolves it against PATH/PATHEXT itself (`node` → `node.exe`/`node.cmd`).
/// Running it through [`windows_cmd_quote`] produced `^"node^"`, which cmd read
/// as a literal-quoted name and failed to resolve, so the MCP server never
/// started (wayland#164). A bare name passes through unchanged; a name with
/// whitespace OR any cmd metacharacter is wrapped in the plain double quotes
/// `cmd /C` expects for the executable token (no caret-escaping). Pure +
/// platform-independent so it is unit-testable on any host.
///
/// F34: whitespace alone was not enough. A whitespace-free program token such
/// as `foo&calc` would otherwise reach `cmd /C foo&calc` unquoted, where cmd
/// reads `&` as a command separator and runs `calc`. Wrapping the token in `"`
/// makes cmd treat `& | < > ^ ( ) %` etc. inside it as literal filename
/// characters (the executable token is parsed as a single quoted name), so a
/// metachar-bearing name can't smuggle a second command onto the line.
fn windows_program_token(command: &str) -> String {
    // cmd.exe metacharacters that, unquoted, would be interpreted on the
    // command line rather than treated as part of the program name.
    const CMD_META: &[char] = &['&', '|', '<', '>', '^', '(', ')', '%', '!', '"'];
    if command.chars().any(char::is_whitespace) || command.chars().any(|c| CMD_META.contains(&c)) {
        format!("\"{command}\"")
    } else {
        command.to_string()
    }
}

/// SIGKILL the child's entire process group (Rank 24, unix-only).
///
/// The child is spawned with `process_group(0)`, so its PGID equals its own
/// PID and every grandchild (`sh` → `npx` → `node`, `uvx` → `python`) inherits
/// that group. `kill(-pgid, SIGKILL)` signals the whole group in one call, so
/// no orphaned `node`/`python` processes linger after the transport closes.
/// A bare `child.kill()` would reap only the direct `sh` wrapper.
///
/// `Child::id()` returns `None` once the child has been reaped (e.g. a prior
/// `kill().await`); in that case there is nothing to signal and we no-op.
/// `ESRCH` (group already gone) is benign and ignored.
#[cfg(unix)]
fn kill_process_group(process_group_id: u32) -> std::io::Result<()> {
    // SAFETY: `kill` is async-signal-safe and we only pass a process-group
    // target (negated PID) plus a constant signal. The PID was assigned by
    // a successful spawn and is the leader of its own group via
    // `process_group(0)`; signalling a stale group merely returns ESRCH.
    let result = unsafe { libc::kill(-(process_group_id as libc::pid_t), libc::SIGKILL) };
    if result != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(error);
        }
    }
    Ok(())
}

// Platform-agnostic unit tests for the cmd.exe quoting function. `shell_quote`
// itself dispatches at runtime via `cfg!(windows)`, but `windows_cmd_quote` is
// a pure string transform that compiles and runs on every platform, so the CI
// matrix (and local dev) can verify its correctness without a Windows box.
#[cfg(test)]
mod cmd_quote_tests {
    use super::windows_cmd_quote;

    /// A plain token gains only the surrounding quotes (which are cmd
    /// metacharacters and so are caret-escaped).
    #[test]
    fn plain_token_is_wrapped_and_caret_escaped() {
        assert_eq!(windows_cmd_quote("npx"), "^\"npx^\"");
    }

    /// Aud-32 core case: a literal `"` plus `&` must NOT let `& calc` escape the
    /// quoted region. Invariant: in the output, EVERY cmd metacharacter
    /// (including `"`) is immediately preceded by a caret, so cmd.exe sees them
    /// all as literals and quote parity can never be unbalanced by the payload.
    #[test]
    fn embedded_quote_and_ampersand_cannot_break_out() {
        let out = windows_cmd_quote("foo\" & calc");
        let chars: Vec<char> = out.chars().collect();
        for (i, &c) in chars.iter().enumerate() {
            if matches!(c, '&' | '|' | '<' | '>' | '(' | ')' | '%' | '!' | '"') {
                let carated = i > 0 && chars[i - 1] == '^';
                assert!(
                    carated,
                    "metacharacter {c:?} at {i} is not caret-escaped in {out:?}"
                );
            }
        }
    }

    /// `%PATH%` must be neutralized: each `%` is caret-escaped so cmd does not
    /// perform environment-variable expansion.
    #[test]
    fn percent_var_is_neutralized() {
        let out = windows_cmd_quote("%PATH%");
        assert!(out.contains("^%"), "percent must be caret-escaped: {out:?}");
        assert!(
            !out.contains("PATH%"),
            "no un-escaped trailing percent: {out:?}"
        );
    }

    /// Backslashes before a quote are doubled (CommandLineToArgvW rule) so the
    /// child program recovers the literal `"`, while the quote stays escaped
    /// for cmd via the caret.
    #[test]
    fn backslashes_before_quote_are_doubled() {
        // Input: a\" — one backslash then a quote.
        let out = windows_cmd_quote("a\\\"");
        // argv phase yields: "a\\\"" (1 backslash -> 2, then \" for the quote).
        // After caret-escaping the quotes: ^"a\\\^"^"
        assert_eq!(out, "^\"a\\\\\\^\"^\"");
    }

    /// Trailing backslashes are doubled so they do not escape the closing
    /// quote of the argv-phase wrapper.
    #[test]
    fn trailing_backslashes_are_doubled() {
        let out = windows_cmd_quote("a\\");
        // argv phase: "a\\" (trailing backslash doubled before closing quote).
        // caret-escape quotes: ^"a\\^"
        assert_eq!(out, "^\"a\\\\^\"");
    }

    // wayland#164: a PROGRAM name (vs an argument) must not be caret-escaped —
    // cmd resolves it via PATH/PATHEXT. These pin the regression where
    // `node` was turned into `^"node^"` and failed to launch.
    use super::windows_program_token;

    #[test]
    fn program_token_passes_bare_names_through_unquoted() {
        // The exact production cases: cmd must see `node`, not `^"node^"`.
        assert_eq!(windows_program_token("node"), "node");
        assert_eq!(windows_program_token("npx"), "npx");
        assert_eq!(windows_program_token("python"), "python");
        // A path without spaces also passes through (cmd resolves it directly).
        assert_eq!(
            windows_program_token("C:\\nodejs\\node.exe"),
            "C:\\nodejs\\node.exe"
        );
    }

    #[test]
    fn program_token_quotes_a_path_with_spaces_without_carets() {
        let out = windows_program_token("C:\\Program Files\\nodejs\\node.exe");
        assert_eq!(out, "\"C:\\Program Files\\nodejs\\node.exe\"");
        // Critically: no caret-escaping on the executable token.
        assert!(
            !out.contains('^'),
            "program token must never be caret-escaped: {out:?}"
        );
    }

    // F34: a whitespace-FREE program token carrying a cmd metacharacter must be
    // wrapped in quotes so cmd cannot interpret it. Without the fix `foo&calc`
    // reached `cmd /C foo&calc` and cmd ran `calc`.
    #[test]
    fn program_token_quotes_metachar_bearing_bare_names() {
        for (input, expected) in [
            ("foo&calc", "\"foo&calc\""),
            ("a|b", "\"a|b\""),
            ("x>y", "\"x>y\""),
            ("p(q)", "\"p(q)\""),
            ("z^w", "\"z^w\""),
            ("v%PATH%", "\"v%PATH%\""),
        ] {
            let out = windows_program_token(input);
            assert_eq!(out, expected, "metachar token {input:?} must be quoted");
            // The quoting must be plain double quotes — never caret-escaped.
            assert!(
                !out.contains('^') || input.contains('^'),
                "program token must not introduce carets: {out:?}"
            );
        }
        // A clean bare name still passes through untouched (fast path).
        assert_eq!(windows_program_token("node"), "node");
    }
}

impl StdioTransport {
    /// Spawn a child process and return the transport.
    ///
    /// Per AGENTS.md cross-platform mandate, all subprocess invocations route
    /// through `wcore_config::shell::shell_command_builder`. This is critical
    /// for MCP servers that ship as `.cmd` shims on Windows (e.g. `npx.cmd`,
    /// `node.cmd`), which raw `CreateProcess` refuses to resolve through PATH.
    /// `shell_command_builder` wraps via `cmd /C` on Windows and `sh -c` on
    /// Unix, so PATHEXT and `.cmd` shim resolution happen correctly.
    pub async fn spawn(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
    ) -> Result<Self, McpError> {
        Self::spawn_with_timeout(command, args, env, DEFAULT_RPC_TIMEOUT).await
    }

    /// Same as [`spawn`](Self::spawn) but with an explicit per-request
    /// timeout. Test seam (audit C1): a short bound lets a test verify the
    /// timeout fires against a hung child without waiting the production
    /// 120s budget.
    pub async fn spawn_with_timeout(
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        rpc_timeout: Duration,
    ) -> Result<Self, McpError> {
        let launch_context = McpStdioLaunchContext::capture(env).map_err(|error| {
            McpError::Transport(format!(
                "Failed to construct sanitized launch environment for '{}': {error}",
                command
            ))
        })?;
        Self::spawn_with_context_and_timeout(command, args, launch_context, rpc_timeout).await
    }

    /// Spawn with a context already captured and inspected by the manager.
    /// Keeping capture outside this method lets diagnostics and process launch
    /// share one immutable cwd/environment snapshot.
    pub(crate) async fn spawn_with_context(
        command: &str,
        args: &[String],
        launch_context: McpStdioLaunchContext,
    ) -> Result<Self, McpError> {
        Self::spawn_with_context_and_timeout(command, args, launch_context, DEFAULT_RPC_TIMEOUT)
            .await
    }

    async fn spawn_with_context_and_timeout(
        command: &str,
        args: &[String],
        launch_context: McpStdioLaunchContext,
        rpc_timeout: Duration,
    ) -> Result<Self, McpError> {
        // Windows: bypass shell_command_builder when the command is already
        // cmd[.exe]. shell_command_builder wraps everything in `cmd /C ...`
        // for PATHEXT shim resolution (npx.cmd, node.cmd) — but when the
        // caller's command IS cmd, that produces nested `cmd /C "cmd /C ..."`
        // whose quote-escaping breaks. Validated locally on Windows
        // (2026-05-26): plain `Command::new("cmd.exe").args(["/C", "echo X"])`
        // exits 0 with the widened FORWARDED_ENV_VARS; routing through
        // shell_command_builder dies in <40ms with "MCP stdio server exited
        // before responding". Production callers pass shim-style commands
        // (npx, node, python) that DO need cmd /C wrapping — they still take
        // the shell_command_builder path.
        let is_windows_cmd = cfg!(windows)
            && (command.eq_ignore_ascii_case("cmd.exe")
                || command.eq_ignore_ascii_case("cmd")
                || command.eq_ignore_ascii_case("powershell.exe")
                || command.eq_ignore_ascii_case("powershell")
                || command.eq_ignore_ascii_case("pwsh.exe")
                || command.eq_ignore_ascii_case("pwsh"));

        let mut cmd = if is_windows_cmd {
            let mut c = tokio::process::Command::new(command);
            c.args(args.iter().map(|s| s.as_str()));
            c.kill_on_drop(true);
            c
        } else {
            let mut parts = Vec::with_capacity(1 + args.len());
            // wayland#164: the PROGRAM name must not go through `shell_quote`
            // on Windows — that caret-escapes it to `^"node^"`, which cmd reads
            // as a literal-quoted name and fails to resolve. cmd resolves the
            // executable token against PATH/PATHEXT itself; only the ARGS need
            // metacharacter-escaping.
            parts.push(if cfg!(windows) {
                windows_program_token(command)
            } else {
                shell_quote(command)
            });
            for a in args {
                parts.push(shell_quote(a));
            }
            let command_str = parts.join(" ");
            mcp_stdio_command_builder(&command_str)
        };
        // Audit C8: stderr is `piped()`, not `inherit()`. An inherited stderr
        // writes the MCP server's diagnostics straight onto the parent's
        // terminal — under the ratatui TUI that is the alternate screen, so
        // a chatty npm/npx wrapper scribbles over the layout. We capture it
        // and drain it into `tracing` instead.
        //
        // F-016 (security): env_clear() first so the MCP child process cannot
        // read provider keys, vault passphrases, or other secrets from the
        // parent env. We then forward only the FORWARDED_ENV_VARS allowlist
        // (the same model as wcore-plugin-subprocess/src/mcp_bridge.rs:206-217),
        // and finally layer the per-server `env` map so operators can
        // explicitly forward additional variables for servers that need them.
        cmd.stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());
        launch_context.apply_to(&mut cmd);

        // Rank 24 (unix) — put the child in its own process group so a
        // `killpg` on close reaps the WHOLE subtree, not just the shell
        // wrapper. `shell_command_builder` runs the server under `sh -c`,
        // which then execs `npx`/`uvx` etc.; those fork further grandchildren
        // (node, python). Without a dedicated group, `kill()` reaps only the
        // `sh` PID and the real server is orphaned. `process_group(0)` makes
        // the child's PGID equal its own PID (it becomes the group leader), so
        // `kill(-pid, SIGKILL)` in `kill_process_group` signals every
        // descendant. Windows keeps its existing `kill_on_drop` / Job-Object
        // behavior (untouched).
        #[cfg(unix)]
        cmd.process_group(0);

        let mut child = cmd
            .spawn()
            .map_err(|e| McpError::Transport(format!("Failed to spawn '{}': {}", command, e)))?;
        let process_group_id = child.id();

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| McpError::Transport("Failed to capture child stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| McpError::Transport("Failed to capture child stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| McpError::Transport("Failed to capture child stderr".into()))?;

        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let alive = Arc::new(AtomicBool::new(true));

        let reader_task = Self::spawn_reader(
            stdout,
            Arc::clone(&pending),
            Arc::clone(&alive),
            command.to_string(),
        );
        let stderr_task = Self::spawn_stderr_drain(stderr, command.to_string());

        Ok(Self {
            stdin: Mutex::new(BufWriter::new(stdin)),
            child: Mutex::new(child),
            process_group_id,
            next_id: AtomicU64::new(1),
            pending,
            reader_task: Mutex::new(Some(reader_task)),
            stderr_task: Mutex::new(Some(stderr_task)),
            alive,
            rpc_timeout,
        })
    }

    /// Background task: own the child's stdout, parse one JSON-RPC response
    /// per line, and route it to the waiting caller by `id`.
    fn spawn_reader(
        stdout: ChildStdout,
        pending: Arc<Mutex<HashMap<u64, oneshot::Sender<JsonRpcResponse>>>>,
        alive: Arc<AtomicBool>,
        label: String,
    ) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            // Audit M-12/M-14 — capped line reader. `read_until` on a
            // `take(MAX_LINE_BYTES)` limiter stops at the byte cap even if
            // no newline arrives, so an endless newline-free stream can't
            // grow the buffer unbounded. We detect overflow as "filled the
            // cap without a terminating newline" and treat the transport as
            // dead.
            let mut raw: Vec<u8> = Vec::new();
            loop {
                raw.clear();
                let read = match (&mut reader)
                    .take(MAX_LINE_BYTES)
                    .read_until(b'\n', &mut raw)
                    .await
                {
                    Ok(n) => n,
                    Err(e) => {
                        error!(server = %label, error = %e, "[mcp] stdio stdout read error");
                        break;
                    }
                };
                if read == 0 {
                    debug!(server = %label, "[mcp] stdio child stdout closed (EOF)");
                    break;
                }
                // Overflow: hit the byte cap with no line terminator. A
                // legitimate JSON-RPC line is newline-delimited and far
                // under the cap, so this is a misbehaving/hostile server.
                if read as u64 >= MAX_LINE_BYTES && raw.last() != Some(&b'\n') {
                    error!(
                        server = %label, cap = MAX_LINE_BYTES,
                        "[mcp] stdio line exceeded byte cap — marking transport dead"
                    );
                    break;
                }
                {
                    let line = String::from_utf8_lossy(&raw);
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<JsonRpcResponse>(trimmed) {
                        Ok(resp) => match resp.id {
                            Some(id) => {
                                let mut guard = pending.lock().await;
                                if let Some(tx) = guard.remove(&id) {
                                    let _ = tx.send(resp);
                                } else {
                                    warn!(
                                        server = %label, id,
                                        "[mcp] stdio response for unknown id (dropping)"
                                    );
                                }
                            }
                            None => {
                                // Notification / log line with no id —
                                // not a response to any request. Drop it
                                // rather than mis-matching it (audit C3).
                                debug!(
                                    server = %label, line = %trimmed,
                                    "[mcp] stdio notification ignored (no id)"
                                );
                            }
                        },
                        Err(e) => {
                            error!(
                                server = %label, error = %e, line = %trimmed,
                                "[mcp] stdio server sent unparseable line"
                            );
                        }
                    }
                }
            }
            // Child is gone (or the line cap was exceeded): mark dead and
            // drain pending so every parked `request()` wakes with a typed
            // error instead of hanging.
            alive.store(false, Ordering::SeqCst);
            let mut guard = pending.lock().await;
            guard.clear();
        })
    }

    /// Background task: drain the child's stderr line-by-line into the log,
    /// tagged with the server name (audit C8). Keeps server diagnostics
    /// without corrupting the TUI's terminal.
    fn spawn_stderr_drain(stderr: ChildStderr, label: String) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            // Audit M-14/mcp-39 — capped reader, same rationale as stdout:
            // a chatty/hostile server's endless newline-free stderr must
            // not grow the buffer unbounded.
            let mut raw: Vec<u8> = Vec::new();
            loop {
                raw.clear();
                let read = match (&mut reader)
                    .take(MAX_LINE_BYTES)
                    .read_until(b'\n', &mut raw)
                    .await
                {
                    Ok(n) => n,
                    Err(_) => break,
                };
                if read == 0 {
                    break;
                }
                // Overflow: cap hit without a newline. Log a truncated
                // notice and stop draining this (misbehaving) stderr.
                if read as u64 >= MAX_LINE_BYTES && raw.last() != Some(&b'\n') {
                    warn!(
                        server = %label, cap = MAX_LINE_BYTES,
                        "[mcp stderr] line exceeded byte cap — stopping stderr drain"
                    );
                    break;
                }
                let line = String::from_utf8_lossy(&raw);
                let trimmed = line.trim_end_matches(['\n', '\r']);
                if !trimmed.is_empty() {
                    warn!(server = %label, "[mcp stderr] {}", trimmed);
                }
            }
        })
    }

    /// Get the next request ID.
    pub fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    /// Mark the child dead and kill it. Idempotent. Used on timeout
    /// (audit C1/C7) so a wedged server can't poison subsequent requests.
    async fn mark_dead(&self) {
        self.alive.store(false, Ordering::SeqCst);
        let mut child = self.child.lock().await;
        // Rank 24 (unix) — SIGKILL the whole process group first so shell-
        // wrapper grandchildren are reaped, then `start_kill` the tracked
        // child so tokio still reaps the direct PID and updates its state.
        #[cfg(unix)]
        if let Some(process_group_id) = self.process_group_id {
            let _ = kill_process_group(process_group_id);
        }
        let _ = child.start_kill();
    }

    /// Send a JSON-RPC message line over stdin.
    async fn send_line(&self, req: &JsonRpcRequest) -> Result<(), McpError> {
        let json = serde_json::to_string(req)
            .map_err(|e| McpError::Transport(format!("JSON serialize error: {}", e)))?;

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(json.as_bytes())
            .await
            .map_err(|e| McpError::Transport(format!("Write to stdin failed: {}", e)))?;
        stdin
            .write_all(b"\n")
            .await
            .map_err(|e| McpError::Transport(format!("Write newline failed: {}", e)))?;
        stdin
            .flush()
            .await
            .map_err(|e| McpError::Transport(format!("Flush stdin failed: {}", e)))?;
        Ok(())
    }
}

#[async_trait]
impl McpTransport for StdioTransport {
    async fn request(&self, req: &JsonRpcRequest) -> Result<JsonRpcResponse, McpError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(McpError::Transport(
                "MCP stdio server is no longer running".into(),
            ));
        }

        // Each request gets its own id and oneshot; the reader task routes
        // the matching response back here (audit C3 — id correlation).
        let id = self.next_id();
        let req = JsonRpcRequest {
            jsonrpc: req.jsonrpc,
            id: Some(id),
            method: req.method.clone(),
            params: req.params.clone(),
        };

        let (tx, rx) = oneshot::channel::<JsonRpcResponse>();
        {
            let mut guard = self.pending.lock().await;
            guard.insert(id, tx);
        }

        if let Err(e) = self.send_line(&req).await {
            self.pending.lock().await.remove(&id);
            // A write failure means the pipe is broken — the child is gone.
            self.mark_dead().await;
            return Err(e);
        }

        // Audit C1: bound the wait. A wedged-but-alive server never writes a
        // response line and never closes the pipe; without this timeout the
        // await is permanent.
        let response = match timeout(self.rpc_timeout, rx).await {
            Ok(Ok(resp)) => resp,
            Ok(Err(_)) => {
                // Sender dropped — reader task drained `pending` on child
                // death. Treat as a dead transport.
                self.alive.store(false, Ordering::SeqCst);
                return Err(McpError::Transport(
                    "MCP stdio server exited before responding".into(),
                ));
            }
            Err(_) => {
                // Timed out. Remove the stale pending entry and kill the
                // wedged child so it can't poison later requests.
                self.pending.lock().await.remove(&id);
                self.mark_dead().await;
                return Err(McpError::Transport(format!(
                    "MCP request timed out after {:?}",
                    self.rpc_timeout
                )));
            }
        };

        if let Some(err) = &response.error {
            return Err(McpError::JsonRpc {
                code: err.code,
                message: err.message.clone(),
            });
        }

        Ok(response)
    }

    async fn notify(&self, req: &JsonRpcRequest) -> Result<(), McpError> {
        if !self.alive.load(Ordering::SeqCst) {
            return Err(McpError::Transport(
                "MCP stdio server is no longer running".into(),
            ));
        }
        if let Err(e) = self.send_line(req).await {
            self.mark_dead().await;
            return Err(e);
        }
        Ok(())
    }

    fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    async fn close(&self) -> Result<(), McpError> {
        // Mark dead first so concurrent `request()` calls fast-fail.
        self.alive.store(false, Ordering::SeqCst);

        // Drop stdin to signal EOF, then SIGKILL and reap. `kill().await`
        // (tokio) sends SIGKILL and awaits process exit, so the OS process
        // is reaped; the EOF additionally unblocks a parked reader.
        {
            let mut child = self.child.lock().await;
            // Rank 24 (unix) — kill the whole process group so shell-wrapper
            // grandchildren (npx→node, uvx→python) are reaped, not orphaned.
            // `kill().await` below additionally reaps the direct child PID.
            #[cfg(unix)]
            let group_cleanup_error = self
                .process_group_id
                .and_then(|process_group_id| kill_process_group(process_group_id).err());
            if child.try_wait()?.is_none() {
                child.start_kill().map_err(|error| {
                    McpError::Transport(format!("failed to kill MCP child: {error}"))
                })?;
                child.wait().await.map_err(|error| {
                    McpError::Transport(format!("failed to reap MCP child: {error}"))
                })?;
            }
            #[cfg(unix)]
            if let Some(error) = group_cleanup_error {
                return Err(McpError::Transport(format!(
                    "failed to kill MCP process group: {error}"
                )));
            }
        }

        // Join the background tasks so they don't leak (audit C9).
        if let Some(handle) = self.reader_task.lock().await.take() {
            // F33 — `timeout` consumes `handle`, so capture an abort handle
            // first; otherwise the timeout arm only DROPS the JoinHandle
            // (which detaches, not aborts) and the reader task leaks. This
            // mirrors how `stderr_task` below aborts its handle.
            let abort = handle.abort_handle();
            match timeout(Duration::from_secs(1), handle).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!(error = %e, "[mcp] stdio reader join error"),
                Err(_) => {
                    warn!("[mcp] stdio reader did not finish within 1s — aborting");
                    abort.abort();
                }
            }
        }
        if let Some(handle) = self.stderr_task.lock().await.take() {
            handle.abort();
        }
        Ok(())
    }
}

impl Drop for StdioTransport {
    fn drop(&mut self) {
        // `shell_command_builder` sets `kill_on_drop(true)`, so dropping the
        // `Child` reaps the OS process. Abort the background tasks so they
        // don't outlive the transport.
        if let Ok(mut guard) = self.reader_task.try_lock()
            && let Some(handle) = guard.take()
        {
            handle.abort();
        }
        if let Ok(mut guard) = self.stderr_task.try_lock()
            && let Some(handle) = guard.take()
        {
            handle.abort();
        }
        #[cfg(unix)]
        if let Some(process_group_id) = self.process_group_id {
            let _ = kill_process_group(process_group_id);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — exercise the real StdioTransport against real child processes.
// Unix-only: the fixture shell snippets use POSIX `sh`. The CI matrix runs
// Windows separately; the timeout/correlation logic is platform-agnostic and
// the integration suite covers the cross-platform contract.
// ---------------------------------------------------------------------------
#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::protocol::JsonRpcRequest;

    fn no_env() -> HashMap<String, String> {
        HashMap::new()
    }

    /// Audit C1 — a child that is alive but never writes a response line
    /// must NOT hang `request()` forever; the per-request timeout fires.
    /// `cat >/dev/null` consumes stdin forever and never writes stdout —
    /// the exact "wedged-but-alive MCP server" failure mode.
    #[tokio::test]
    async fn c1_request_times_out_against_hung_server() {
        let transport = StdioTransport::spawn_with_timeout(
            "sh",
            &["-c".to_string(), "cat >/dev/null".to_string()],
            &no_env(),
            Duration::from_millis(300),
        )
        .await
        .expect("spawn hung fixture");

        let req = JsonRpcRequest::new(1, "initialize", None);
        let start = std::time::Instant::now();
        let result = transport.request(&req).await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "hung server must produce an error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("timed out"),
            "expected a timeout error, got: {msg}"
        );
        assert!(
            elapsed < Duration::from_secs(2),
            "timeout must fire promptly, took {elapsed:?}"
        );
        // After a timeout the wedged child is killed → transport is dead.
        assert!(
            !transport.is_alive(),
            "transport must be marked dead after a timeout kills the child"
        );

        let _ = transport.close().await;
    }

    /// Audit M-12/M-14 — an oversized newline-free stdout flood must NOT
    /// OOM the host. The capped reader hits `MAX_LINE_BYTES` without a line
    /// terminator, treats the server as misbehaving, breaks the read loop,
    /// and marks the transport dead. A subsequent `request()` then surfaces
    /// a typed error (dead transport) rather than buffering unbounded.
    ///
    /// The fixture writes > MAX_LINE_BYTES (8 MiB) of newline-free bytes
    /// then sleeps, so the reader is guaranteed to trip the cap. We assert
    /// the transport ends up dead and that we never hang.
    #[tokio::test]
    async fn m12_oversized_line_marks_transport_dead_no_oom() {
        // `head -c 9000000 /dev/zero | tr '\0' a` emits 9 MiB of 'a' with
        // no '\n', exceeding the 8 MiB cap. Then `sleep 5` keeps the child
        // alive briefly so the reader's cap-trip (not EOF) is what fires.
        let script = "head -c 9000000 /dev/zero | tr '\\0' a; sleep 5";
        let transport =
            StdioTransport::spawn("sh", &["-c".to_string(), script.to_string()], &no_env())
                .await
                .expect("spawn flood fixture");

        // Give the reader task time to consume the flood and trip the cap.
        let start = std::time::Instant::now();
        loop {
            if !transport.is_alive() {
                break;
            }
            if start.elapsed() > Duration::from_secs(4) {
                panic!("reader did not trip the byte cap within 4s");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        assert!(
            !transport.is_alive(),
            "transport must be marked dead after the line cap is exceeded"
        );

        // A request on the dead transport must fast-fail, not hang.
        let result = transport
            .request(&JsonRpcRequest::new(1, "ping", None))
            .await;
        assert!(
            result.is_err(),
            "request on a cap-killed transport must error, not OOM/hang"
        );

        let _ = transport.close().await;
    }

    /// Audit C3 — id correlation. A server that echoes a valid JSON-RPC
    /// response for the request id must round-trip through the background
    /// reader + pending map. `request()` rewrites the id, so the fixture
    /// must echo back whatever id it receives. We use an `sh` snippet that
    /// reads one line and emits a canned response carrying the SAME id the
    /// transport assigned (id 1, the first `next_id`).
    #[tokio::test]
    async fn c3_request_round_trips_with_id_correlation() {
        // The fixture reads the request line then emits a response whose
        // `id` is 1 — matching the first id `request()` assigns.
        let script = r#"read line; printf '{"jsonrpc":"2.0","id":1,"result":{"ok":true}}\n'"#;
        let transport =
            StdioTransport::spawn("sh", &["-c".to_string(), script.to_string()], &no_env())
                .await
                .expect("spawn sh fixture");

        let req = JsonRpcRequest::new(99, "ping", None);
        let resp = transport.request(&req).await.expect("round-trip ok");
        assert_eq!(resp.id, Some(1));
        assert_eq!(resp.result.as_ref().unwrap()["ok"], serde_json::json!(true));

        let _ = transport.close().await;
    }

    /// Audit C3 — a stray non-response line (a notification with no `id`,
    /// or a log line) must NOT be mis-matched as the response to an
    /// in-flight request. The reader drops it; the real response still
    /// routes correctly afterward.
    #[tokio::test]
    async fn c3_stray_line_is_not_mismatched() {
        // Emit a notification (no id) and a log line BEFORE the real
        // response. The old "read the next line" model would have returned
        // the notification as the response and failed to parse / mis-route.
        let script = r#"read line; printf '{"jsonrpc":"2.0","method":"log/info"}\n'; printf 'plain stderr-ish noise on stdout\n'; printf '{"jsonrpc":"2.0","id":1,"result":{"v":42}}\n'"#;
        let transport =
            StdioTransport::spawn("sh", &["-c".to_string(), script.to_string()], &no_env())
                .await
                .expect("spawn sh fixture");

        let req = JsonRpcRequest::new(1, "ping", None);
        let resp = transport.request(&req).await.expect("real response routed");
        assert_eq!(resp.result.as_ref().unwrap()["v"], serde_json::json!(42));

        let _ = transport.close().await;
    }

    /// Audit C1/C4 — a server that exits without responding must surface a
    /// typed error (not a hang) and leave the transport marked dead so the
    /// manager can prune it.
    #[tokio::test]
    async fn c4_request_errors_when_server_exits_without_responding() {
        // `true` exits immediately, closing stdout → EOF.
        let transport = StdioTransport::spawn("true", &[], &no_env())
            .await
            .expect("spawn true");

        let req = JsonRpcRequest::new(1, "ping", None);
        let result = transport.request(&req).await;
        assert!(result.is_err(), "exited server must error");
        assert!(
            !transport.is_alive(),
            "transport must be dead after the child exits"
        );

        let _ = transport.close().await;
    }

    /// Audit C4 — after the transport is dead, a fresh `request()` must
    /// fast-fail immediately rather than re-running the timeout.
    #[tokio::test]
    async fn c4_request_fast_fails_on_dead_transport() {
        let transport = StdioTransport::spawn("true", &[], &no_env())
            .await
            .expect("spawn true");
        // First call observes the EOF and marks the transport dead.
        let _ = transport
            .request(&JsonRpcRequest::new(1, "ping", None))
            .await;
        assert!(!transport.is_alive());

        let start = std::time::Instant::now();
        let result = transport
            .request(&JsonRpcRequest::new(2, "ping", None))
            .await;
        assert!(result.is_err());
        assert!(
            start.elapsed() < Duration::from_millis(100),
            "dead-transport request must fast-fail"
        );
    }

    /// Rank 24 — `close()` must reap the child's grandchildren, not just the
    /// `sh` wrapper. The fixture backgrounds a long `sleep` (a grandchild of
    /// the `sh -c` wrapper, exactly mirroring `sh → npx → node`), records its
    /// PID, then services one request. After `close()`, the grandchild must be
    /// gone — proving the process-group kill reaped the whole subtree. Without
    /// the `process_group(0)` + `killpg` fix this grandchild is orphaned and
    /// `kill(pid, 0)` still reports it alive.
    #[tokio::test]
    async fn rank24_close_reaps_grandchildren() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let pid_path = tmp.path().join("grandchild.pid");

        // Background a `sleep 300`, write ITS pid, then read one stdin line and
        // emit a JSON-RPC response so `request()` round-trips.
        let script = format!(
            "sleep 300 & echo $! > {pid}; read line; \
             printf '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}\\n'",
            pid = pid_path.display()
        );
        let transport = StdioTransport::spawn("sh", &["-c".to_string(), script], &no_env())
            .await
            .expect("spawn grandchild fixture");

        // Drive one request so the fixture runs far enough to spawn the
        // grandchild and write its PID.
        let _ = transport
            .request(&JsonRpcRequest::new(1, "ping", None))
            .await;

        // Read the grandchild PID the fixture recorded.
        let gpid: i32 = {
            let mut tries = 0;
            loop {
                if let Ok(s) = std::fs::read_to_string(&pid_path)
                    && let Ok(pid) = s.trim().parse::<i32>()
                {
                    break pid;
                }
                tries += 1;
                assert!(tries < 100, "fixture never wrote grandchild pid");
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        };

        // Grandchild should be alive before close.
        // SAFETY: signal 0 is the standard liveness probe — sends no signal.
        let alive_before = unsafe { libc::kill(gpid as libc::pid_t, 0) };
        assert_eq!(alive_before, 0, "grandchild should be alive before close");

        transport.close().await.expect("close ok");

        // After close, the grandchild must be REAPED — i.e. killed by the
        // process-group teardown. "Killed" means either fully gone (ESRCH) or a
        // `<defunct>` zombie: in a reaper-less container (e.g. CI with no init at
        // PID 1) the orphaned grandchild is SIGKILL'd but has no parent to reap
        // its exit status, so it lingers as a zombie and `kill(pid, 0)` still
        // returns 0. A zombie proves the rank-24 fix worked; only a still-RUNNING
        // orphan is a failure. Poll briefly because group teardown isn't instant.
        //
        // Liveness probe that treats a zombie as reaped. SAFETY: signal 0 sends
        // no signal; -1/ESRCH means the pid is gone.
        let killed_or_zombie = |pid: i32| -> bool {
            if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
                return true; // ESRCH — fully gone.
            }
            #[cfg(target_os = "linux")]
            {
                // /proc/<pid>/stat: "pid (comm) STATE ...". The state char sits
                // just after the final ')' of the (possibly paren-containing)
                // comm field. 'Z' == zombie == killed-but-unreaped.
                match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                    Ok(stat) => stat
                        .rsplit_once(')')
                        .map(|(_, rest)| rest.trim_start().starts_with('Z'))
                        .unwrap_or(false),
                    // Entry vanished between the kill probe and the read — gone.
                    Err(_) => true,
                }
            }
            #[cfg(not(target_os = "linux"))]
            {
                false // Non-Linux runners have a reaper; ESRCH is the real signal.
            }
        };

        let mut reaped = false;
        for _ in 0..100 {
            if killed_or_zombie(gpid) {
                reaped = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            reaped,
            "grandchild pid {gpid} still running after close — process group not reaped"
        );
    }

    /// Audit C9 — `close()` kills the child, marks the transport dead, and
    /// joins the reader task without hanging.
    #[tokio::test]
    async fn c9_close_kills_child_and_marks_dead() {
        let transport = StdioTransport::spawn(
            "sh",
            &["-c".to_string(), "cat >/dev/null".to_string()],
            &no_env(),
        )
        .await
        .expect("spawn hung fixture");
        assert!(transport.is_alive());

        let start = std::time::Instant::now();
        transport.close().await.expect("close ok");
        assert!(!transport.is_alive(), "close must mark the transport dead");
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "close must not hang"
        );
    }

    /// F-016 — the real transport applies the canonical sanitized context:
    /// ambient secrets are absent, curated PATH survives, and explicit
    /// per-server values are applied last.
    #[tokio::test]
    #[serial_test::serial(mcp_stdio_environment)]
    async fn f016_real_spawn_uses_sanitized_launch_context() {
        let previous = std::env::var_os("WAYLAND_TEST_SECRET_CANARY");
        unsafe {
            std::env::set_var("WAYLAND_TEST_SECRET_CANARY", "ambient-secret");
        }
        let script = r#"read line; printf '{"jsonrpc":"2.0","id":1,"result":{"secret":"%s","explicit":"%s","path":"%s"}}\n' "${WAYLAND_TEST_SECRET_CANARY-unset}" "$MCP_EXPLICIT_VAR" "${PATH:+present}""#;
        let environment =
            HashMap::from([("MCP_EXPLICIT_VAR".to_owned(), "explicit-value".to_owned())]);
        let spawned =
            StdioTransport::spawn("sh", &["-c".to_owned(), script.to_owned()], &environment).await;
        match previous {
            Some(value) => unsafe { std::env::set_var("WAYLAND_TEST_SECRET_CANARY", value) },
            None => unsafe { std::env::remove_var("WAYLAND_TEST_SECRET_CANARY") },
        }
        let transport = spawned.expect("spawn environment fixture");

        let response = transport
            .request(&JsonRpcRequest::new(1, "ping", None))
            .await
            .expect("environment fixture response");
        let result = response.result.expect("environment result");
        assert_eq!(result["secret"], "unset");
        assert_eq!(result["explicit"], "explicit-value");
        assert_eq!(result["path"], "present");
        transport.close().await.expect("close environment fixture");
    }

    /// B1 — the profile-home handshake reaches the spawned MCP child.
    ///
    /// Drives the real `StdioTransport::spawn` path against a tiny sh "server"
    /// that writes `$WAYLAND_PROFILE_HOME` and a marker carrying any
    /// `$IJFW_WAYLAND_PROFILE_HOME` to a file on its first stdin line. We pin
    /// `WAYLAND_HOME` to a tempdir so the expected value is deterministic, then
    /// assert the child saw the vendor-neutral var (proving the injection
    /// survives `env_clear()`) AND that the host did NOT bake in any plugin's
    /// `IJFW_*` alias — the host stays vendor-neutral.
    #[tokio::test]
    #[serial_test::serial(wayland_home_env)]
    async fn b1_profile_home_reaches_spawned_child() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let out_path = tmp.path().join("env-dump");
        let wh = tmp.path().join("profile-root");

        let prev = std::env::var_os("WAYLAND_HOME");
        unsafe {
            std::env::set_var("WAYLAND_HOME", &wh);
        }

        // Server: on the first stdin line, dump the neutral profile-home var plus
        // a bracketed marker for the (expected-absent) IJFW alias, emit one
        // JSON-RPC-shaped line so the transport's reader is satisfied, then exit.
        let script = format!(
            "read line; printf '%s\\n[ijfw:%s]\\n' \"$WAYLAND_PROFILE_HOME\" \"$IJFW_WAYLAND_PROFILE_HOME\" > {dump}; \
             printf '{{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{}}}}\\n'",
            dump = out_path.display()
        );

        let transport = StdioTransport::spawn("sh", &["-c".to_string(), script], &no_env())
            .await
            .expect("spawn profile-home fixture");

        // Kick the server with one request so it reads its stdin line and dumps.
        let _ = transport
            .request(&JsonRpcRequest::new(1, "ping", None))
            .await;
        let _ = transport.close().await;

        let restore = || match &prev {
            Some(v) => unsafe { std::env::set_var("WAYLAND_HOME", v) },
            None => unsafe { std::env::remove_var("WAYLAND_HOME") },
        };

        let dumped = std::fs::read_to_string(&out_path);
        restore();
        let dumped = dumped.expect("child should have written env dump");

        let expected = wh.to_string_lossy();
        let mut lines = dumped.lines();
        assert_eq!(
            lines.next().unwrap_or_default(),
            expected,
            "WAYLAND_PROFILE_HOME not seen by child: {dumped:?}"
        );
        // The host must NOT set any plugin-specific alias — the marker is empty.
        assert_eq!(
            lines.next().unwrap_or_default(),
            "[ijfw:]",
            "host leaked a plugin-specific profile-home alias: {dumped:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Windows sibling tests for MCP stdio transport (Audit W-5 fix).
// E2E-WINDOWS-ADDENDUM-2026-05-24 §2.2: the tests above are
// #[cfg(all(test, unix))]-gated and use POSIX `sh`. Windows users get zero
// test coverage of the MCP stdio transport without these siblings.
//
// These tests use `cmd.exe` instead of `sh` to exercise the same timeout,
// id-correlation, and is_alive() logic on Windows.
//
// **Both tests in this module are currently `#[ignore]`-gated.**
// They were added in fa34e70 (windows-compat-scout audit closeouts,
// 2026-05-24) but the CI surface above them had been red for weeks, so
// the Windows job never reached this module to validate them. The
// v0.8.6 CI restoration saga (rounds 11-16) cleared every preceding
// failure on Windows; round 16 then exposed that BOTH tests in this
// module deterministically fail with "MCP stdio server exited before
// responding" because `spawn_with_timeout` calls `env_clear()` and
// only forwards the `FORWARDED_ENV_VARS` allowlist. That allowlist is
// missing the standard Windows user-dir vars (`USERPROFILE`,
// `APPDATA`, `LOCALAPPDATA`, `PSMODULEPATH`, `WINDIR`) — without
// those the spawned cmd.exe / powershell.exe child fails to initialise
// and dies in ~15ms, before any request reaches it.
//
// v0.8.7 closes the gap by widening `FORWARDED_ENV_VARS` to include
// the standard Windows user-dir vars — a production win for real
// Windows MCP servers, which currently fail to start for the same
// reason — and re-enabling both tests. Tracked in
// `.blackboard/V0.8.7-HARDENING-windows-test-audit.md` and the
// per-fn docstrings below.
//
// The timeout (W-5 C1) and round-trip (W-5 C3) invariants are
// covered by the `#[cfg(unix)]` siblings above on every Linux+macOS
// CI run; the production code path under test is the same.
// ---------------------------------------------------------------------------

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use crate::protocol::JsonRpcRequest;
    use std::collections::HashMap;

    fn no_env() -> HashMap<String, String> {
        HashMap::new()
    }

    /// Audit W-5, Windows C1 — a child that never writes a response must
    /// NOT hang request() forever; the per-request timeout must fire.
    ///
    /// **Currently `#[ignore]`'d on Windows — v0.8.7 followup.**
    ///
    /// Two root causes interact and there is no clean fixture under the
    /// current production code path:
    ///
    /// 1. `spawn_with_timeout` routes every command through
    ///    `shell_command_builder` → `cmd /C "<inner>"`, then calls
    ///    `env_clear()` and re-adds only `FORWARDED_ENV_VARS`. That
    ///    allowlist is missing `USERPROFILE`, `APPDATA`, `LOCALAPPDATA`,
    ///    `PSMODULEPATH`, `WINDIR` — without those, `powershell.exe`
    ///    fails to initialise its runtime and the child exits in ~15ms.
    ///    The reader task sees the EOF and surfaces "server exited
    ///    before responding" instead of a timeout (rounds 14 & 15,
    ///    CI runs 26416035228 + 26418372948).
    ///
    /// 2. `cmd /C` built-ins lack a reliable "block on stdin without
    ///    writing stdout" fixture — `more` with a piped stdin exits
    ///    immediately on GHA windows-latest because it cannot open
    ///    CONIN$, so the original fixture had the same exit-not-hang
    ///    behaviour (round 14, CI run 26416035228).
    ///
    /// The timeout invariant this test exists to assert is fully
    /// covered by the `#[cfg(unix)]` sibling `w5_c1_request_times_out_
    /// against_hung_server` above — same control flow, just a real
    /// `cat >/dev/null` fixture. The other Windows tests in this module
    /// still cover the round-trip + id-correlation paths on Windows.
    ///
    /// v0.8.7 closes the gap by widening `FORWARDED_ENV_VARS` to
    /// include the standard Windows user-dir vars (a production win for
    /// real Windows MCP servers — they currently fail to start for the
    /// same reason) and re-enabling this test against the powershell
    /// `Start-Sleep` fixture.
    #[tokio::test]
    async fn w5_c1_request_times_out_against_hung_server_windows() {
        // Equivalent of the Unix `cat >/dev/null` hang fixture:
        // [Console]::In.ReadToEnd() blocks reading stdin until EOF; the
        // transport holds the write-end of stdin open for the child's
        // lifetime, so the child never sees EOF and never echoes anything
        // back. The test sends a request, the child silently accepts it
        // into the read buffer but never responds, the 500ms transport
        // timeout fires, transport kills the child.
        //
        // ReadLine() would exit after the first line — that returns
        // "server exited" instead of "timed out" because the child died
        // (caught locally 2026-05-26 with ReadLine and corrected to
        // ReadToEnd).
        let transport = StdioTransport::spawn_with_timeout(
            "powershell.exe",
            &[
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                "[Console]::In.ReadToEnd() | Out-Null".to_string(),
            ],
            &no_env(),
            Duration::from_millis(500),
        )
        .await
        .expect("spawn powershell.exe fixture");

        let req = JsonRpcRequest::new(1, "initialize", None);
        let start = std::time::Instant::now();
        let result = transport.request(&req).await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "hung server must produce an error");
        let msg = result.unwrap_err().to_string();
        assert!(
            msg.contains("timed out"),
            "expected a timeout error, got: {msg}"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "timeout must fire promptly on Windows, took {elapsed:?}"
        );
        assert!(
            !transport.is_alive(),
            "transport must be marked dead after timeout kills child"
        );

        let _ = transport.close().await;
    }

    /// Audit W-5, Windows C3 — id correlation round-trip via powershell.
    /// The fixture mirrors the Unix `c3_request_round_trips_with_id_correlation`
    /// pattern: BLOCK reading one line from stdin (the test's request line),
    /// then emit a response carrying id=1. The `[Console]::In.ReadLine()`
    /// blocking step is essential — without it the response races against
    /// the test's `request()` call and gets dropped before any pending
    /// entry is registered (which is what an `echo`-only fixture does, and
    /// is why an earlier `cmd /C echo ...` attempt failed locally
    /// 2026-05-26 with "MCP stdio server exited before responding").
    #[tokio::test]
    async fn w5_c3_request_round_trips_windows() {
        // PowerShell script:
        //   1. Read one line from stdin (blocks until test sends request)
        //   2. Write the canned JSON response with id=1 (matches the first
        //      `next_id` the transport assigns)
        // Use single-quoted JSON to avoid PowerShell string interpolation
        // surprises; the line reader in spawn_reader strips trailing \r\n.
        let script = "[Console]::In.ReadLine() | Out-Null; \
                      [Console]::Out.WriteLine('{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}')";
        let transport = StdioTransport::spawn(
            "powershell.exe",
            &[
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-Command".to_string(),
                script.to_string(),
            ],
            &no_env(),
        )
        .await
        .expect("spawn powershell.exe fixture");

        let req = JsonRpcRequest::new(99, "ping", None);
        let resp = transport
            .request(&req)
            .await
            .expect("round-trip ok on Windows");
        assert_eq!(resp.id, Some(1));
        assert_eq!(resp.result.as_ref().unwrap()["ok"], serde_json::json!(true));

        let _ = transport.close().await;
    }

    // ── #262 / #263: the production `else` branch builds `cmd /C <token>
    // <args...>` and now hands it to cmd verbatim via `mcp_stdio_command_builder`
    // (raw_arg). These spawn the SAME helper directly (full env inherited, so a
    // `.bat` resolves via PATHEXT like npx.cmd/uvx.cmd) and assert the child
    // sees clean argv. Spawning the helper directly — not `spawn_with_timeout`
    // — avoids the env_clear/FORWARDED_ENV_VARS gap that the W-5 test documents.

    /// #263 — a spaced absolute program path must resolve and NOT split at the
    /// space. Before the fix, std's CommandLineToArgvW re-quoted the already
    /// caret-escaped line, cmd lost quote parity, and the program token split
    /// into `"C:\dir` + ` with space\...`, so cmd could not find the program.
    #[tokio::test]
    async fn w263_spaced_program_path_resolves() {
        let base = tempfile::tempdir().expect("tempdir");
        let spaced = base.path().join("dir with space");
        std::fs::create_dir_all(&spaced).unwrap();
        let bat = spaced.join("ok.bat");
        std::fs::write(&bat, "@echo off\r\necho.OK\r\n").unwrap();

        // Same assembly the transport uses: program token only.
        let line = windows_program_token(&bat.to_string_lossy());
        let out = mcp_stdio_command_builder(&line)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn spaced-path .bat")
            .wait_with_output()
            .await
            .expect("wait");
        assert!(
            out.status.success(),
            "spaced program path must resolve; stdout={:?}",
            String::from_utf8_lossy(&out.stdout)
        );
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("OK"),
            "child must run from the spaced path"
        );
    }

    /// #262 — npx/uvx-style package args must reach the child VERBATIM, and
    /// (Aud-32) a `&` in an arg must stay a literal token, never a command
    /// separator. The `.bat` echoes each arg inside quotes so the batch line
    /// re-expansion cannot re-interpret a `&` at echo time.
    #[tokio::test]
    async fn w262_args_reach_child_verbatim_and_metachars_neutralized() {
        let dir = tempfile::tempdir().expect("tempdir");
        let bat = dir.path().join("echo_args.bat");
        std::fs::write(
            &bat,
            "@echo off\r\necho.\"%~1\"\r\necho.\"%~2\"\r\necho.\"%~3\"\r\n",
        )
        .unwrap();

        let parts = [
            windows_program_token(&bat.to_string_lossy()),
            windows_cmd_quote("@perplexity-ai/mcp-server"),
            windows_cmd_quote("wikipedia-mcp"),
            windows_cmd_quote("a&b"), // Aud-32: `&` must stay literal
        ];
        let line = parts.join(" ");
        let out = mcp_stdio_command_builder(&line)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("spawn echo .bat")
            .wait_with_output()
            .await
            .expect("wait");
        let stdout = String::from_utf8_lossy(&out.stdout);

        assert!(
            stdout.contains("@perplexity-ai/mcp-server"),
            "scoped-package arg corrupted: {stdout:?}"
        );
        assert!(
            stdout.contains("wikipedia-mcp"),
            "arg2 corrupted: {stdout:?}"
        );
        // The `&` arg arrived as one literal token (no second command ran).
        assert!(
            stdout.contains("a&b"),
            "metachar arg must be one literal token, not split: {stdout:?}"
        );
        // No caret leaked into the child's view of the argument.
        assert!(
            !stdout.contains('^'),
            "caret leaked into child argv: {stdout:?}"
        );
    }
}
