//! NoSandbox backend — runs the command directly via
//! `tokio::process::Command`, NO isolation. Used when the platform's
//! primary sandbox is unavailable. Emits a warn-once log so operators
//! know they are running unsandboxed.
//!
//! The host env is NOT inherited: the child receives only the explicit
//! `env` entries from the manifest. This matches the security
//! contract of the real backends so flipping `WAYLAND_SANDBOX=none`
//! does not silently widen env exposure (Audit B H5).

use super::SandboxBackend;
use crate::error::{Result, SandboxError};
use crate::manifest::SandboxManifest;
use crate::{ResourceLimitEnforcement, SandboxChunk, SandboxCommand, SandboxOutput};
use async_trait::async_trait;
use std::process::Stdio;
use std::sync::{Arc, Once};
use tokio::io::AsyncReadExt;

static WARN_ONCE: Once = Once::new();

/// Emit a single warn-level log for the lifetime of the process telling
/// the operator that sandboxing is disabled.
pub fn warn_once_sandbox_disabled() {
    WARN_ONCE.call_once(|| {
        tracing::warn!(
            target: "wcore_sandbox",
            "sandbox DISABLED — child processes run with host permissions. \
             Install bubblewrap (Linux), or set WAYLAND_SANDBOX=docker for opt-in Docker.",
        );
    });
}

pub struct NoSandboxBackend;

impl NoSandboxBackend {
    pub fn new() -> Self {
        Self
    }

    fn command(
        manifest: &SandboxManifest,
        cmd: &SandboxCommand,
    ) -> Result<tokio::process::Command> {
        let program = cmd
            .argv
            .first()
            .ok_or_else(|| SandboxError::ExecFailed("empty argv".into()))?;
        let mut builder = tokio::process::Command::new(program);
        Self::append_args(&mut builder, &cmd.argv)?;
        if let Some(cwd) = &cmd.cwd {
            builder.current_dir(cwd);
        }
        builder.kill_on_drop(true);
        super::process_tree::isolate(&mut builder);
        builder.env_clear();
        for (k, v) in &manifest.env {
            builder.env(k, v);
        }
        builder.stdout(Stdio::piped()).stderr(Stdio::piped());
        Ok(builder)
    }

    /// Append `argv[1..]` to `builder`.
    ///
    /// Everywhere except a Windows `cmd.exe` invocation this is a plain
    /// `args()` — argv is argv. A `cmd /C` payload is NOT argv: see
    /// [`crate::backends::windows_cmdline`] for the measured corruption that
    /// treating it as argv produces (`std` inserts CRT `\"` escapes that
    /// `cmd.exe` does not undo, so the real child receives literal
    /// backslashes and a split argument — loudly for `node`, and SILENTLY at
    /// exit 0 for `python -c`).
    #[cfg(not(windows))]
    fn append_args(builder: &mut tokio::process::Command, argv: &[String]) -> Result<()> {
        if argv.len() > 1 {
            builder.args(&argv[1..]);
        }
        Ok(())
    }

    #[cfg(windows)]
    fn append_args(builder: &mut tokio::process::Command, argv: &[String]) -> Result<()> {
        use std::os::windows::process::CommandExt;

        let payload_idx = super::windows_cmdline::cmd_payload_index(argv);
        let std_builder = builder.as_std_mut();
        for (idx, arg) in argv.iter().enumerate().skip(1) {
            if Some(idx) == payload_idx {
                // Refuse before spawning: cmd would truncate at the first line
                // break, run the prefix, and return ITS status — success for
                // work that never happened.
                super::windows_cmdline::reject_undeliverable_cmd_payload(arg)?;
                std_builder.raw_arg(super::windows_cmdline::quote_cmd_payload(arg));
            } else {
                std_builder.arg(arg);
            }
        }
        Ok(())
    }
}

impl Default for NoSandboxBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SandboxBackend for NoSandboxBackend {
    fn name(&self) -> &'static str {
        "no_sandbox"
    }

    fn is_available(&self) -> bool {
        true
    }

    async fn execute(
        &self,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<SandboxOutput> {
        // S9: kill the child if this future is dropped (e.g. when a caller
        // races us against a timeout / cancellation token). Without this
        // a dropped `output()` future leaves a zombie subprocess — the
        // same reliability blocker `wcore_config::shell` fixed for the
        // shell helpers. Routing BashTool through the sandbox must not
        // reintroduce that leak.
        let mut child = Self::command(manifest, &cmd)?
            .spawn()
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        let mut process_tree = super::process_tree::ProcessTreeGuard::new(child.id())
            .map_err(|e| SandboxError::ExecFailed(format!("process-tree ownership: {e}")))?;
        let output =
            super::wait_with_bounded_output_on_exit(&mut child, || process_tree.disarm()).await?;
        Ok(SandboxOutput {
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            resource_limits: ResourceLimitEnforcement::None,
        })
    }

    fn execute_streaming(
        self: Arc<Self>,
        manifest: &SandboxManifest,
        cmd: SandboxCommand,
    ) -> Result<tokio::sync::mpsc::Receiver<SandboxChunk>> {
        let mut child = Self::command(manifest, &cmd)?
            .spawn()
            .map_err(|e| SandboxError::ExecFailed(e.to_string()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| SandboxError::ExecFailed("child stdout was not piped".into()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| SandboxError::ExecFailed("child stderr was not piped".into()))?;
        let process_tree = super::process_tree::ProcessTreeGuard::new(child.id())
            .map_err(|e| SandboxError::ExecFailed(format!("process-tree ownership: {e}")))?;
        let (tx, rx) = tokio::sync::mpsc::channel(super::STREAM_CHANNEL_CAP);

        tokio::spawn(async move {
            let mut process_tree = process_tree;
            let mut stdout_open = true;
            let mut stderr_open = true;
            let mut stdout_buf = [0_u8; 8 * 1024];
            let mut stderr_buf = [0_u8; 8 * 1024];
            let mut exit_code = None;
            let wait = child.wait();
            tokio::pin!(wait);

            while stdout_open || stderr_open || exit_code.is_none() {
                tokio::select! {
                    _ = tx.closed() => return,
                    read = stdout.read(&mut stdout_buf), if stdout_open => match read {
                        Ok(0) => stdout_open = false,
                        Ok(n) => {
                            if tx.send(SandboxChunk::Stdout(stdout_buf[..n].to_vec())).await.is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = tx.send(SandboxChunk::Stderr(
                                format!("failed to read child stdout: {error}").into_bytes(),
                            )).await;
                            return;
                        }
                    },
                    read = stderr.read(&mut stderr_buf), if stderr_open => match read {
                        Ok(0) => stderr_open = false,
                        Ok(n) => {
                            if tx.send(SandboxChunk::Stderr(stderr_buf[..n].to_vec())).await.is_err() {
                                return;
                            }
                        }
                        Err(error) => {
                            let _ = tx.send(SandboxChunk::Stderr(
                                format!("failed to read child stderr: {error}").into_bytes(),
                            )).await;
                            return;
                        }
                    },
                    status = &mut wait, if exit_code.is_none() => match status {
                        Ok(status) => exit_code = Some(status.code().unwrap_or(-1)),
                        Err(error) => {
                            let _ = tx.send(SandboxChunk::Stderr(
                                format!("failed to wait for child: {error}").into_bytes(),
                            )).await;
                            return;
                        }
                    },
                }
            }

            process_tree.disarm();
            let _ = tx
                .send(SandboxChunk::Exit {
                    exit_code: exit_code.expect("loop exits only after child status is available"),
                    resource_limits: ResourceLimitEnforcement::None,
                })
                .await;
        });

        Ok(rx)
    }
}

#[cfg(all(test, windows))]
mod windows_cmd_delivery_tests {
    //! The Windows Bash surface runs `["cmd", "/S", "/C", <command>]` (see
    //! `wcore_config::shell::bash_shell_argv_prefix`), and this backend is what
    //! the relaxed Windows default and `WAYLAND_SANDBOX=none` both spawn
    //! through. These are LIVE tests: they run a real `cmd.exe` and grade the
    //! bytes it produced and the files it left on disk.

    use super::*;

    /// The production Windows Bash argv with the minimum env `cmd.exe` needs to
    /// start, so these live tests measure the shape the agent actually spawns.
    /// The prefix is owned by `wcore_config::shell::windows_cmd_payload_prefix`
    /// (this crate does not depend on `wcore-config`, so it is mirrored here);
    /// `/S` is what makes cmd strip the outer pair `quote_cmd_payload` adds
    /// instead of leaving it in the executed text (#943). The env is still
    /// scrubbed — only these two names are injected.
    fn cmd_job(payload: &str) -> (SandboxManifest, SandboxCommand) {
        let mut manifest = SandboxManifest::default();
        for key in ["PATH", "SYSTEMROOT"] {
            if let Some(value) = std::env::var_os(key) {
                manifest
                    .env
                    .push((key.to_string(), value.to_string_lossy().into_owned()));
            }
        }
        (
            manifest,
            SandboxCommand {
                argv: vec!["cmd".into(), "/S".into(), "/C".into(), payload.into()],
                cwd: None,
            },
        )
    }

    async fn run(payload: &str) -> SandboxOutput {
        let (manifest, cmd) = cmd_job(payload);
        NoSandboxBackend::new()
            .execute(&manifest, cmd)
            .await
            .expect("cmd.exe must run")
    }

    /// **C1.** The command that runs must be the command that was written.
    ///
    /// `std::process::Command` joins argv with MSVC C-runtime rules, which
    /// escape an embedded `"` as `\"`. `cmd.exe` does not undo backslash
    /// escapes in its `/C` payload, so those backslashes reach the real child
    /// as literal characters and the `"` they were escaping stops delimiting.
    /// Measured before the fix: `node -e "…writeFileSync('n.txt', 'ok')"`
    /// arrived as two arguments, the first beginning with a literal `"`.
    ///
    /// This asserts on the delivered TEXT, so it cannot be satisfied by the
    /// multi-line refusal that closes C2's second instance.
    #[tokio::test]
    async fn a_quoted_argument_reaches_the_shell_without_crt_escapes() {
        let output = run(r#"echo "alpha beta gamma""#).await;
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            output.exit_code,
            0,
            "stderr: {:?}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !stdout.contains('\\'),
            "the payload carried no backslash; a `\\` in the output is a CRT \
             escape that cmd.exe executed literally: {stdout:?}"
        );
        assert_eq!(
            stdout.trim_end(),
            r#""alpha beta gamma""#,
            "cmd must receive the payload byte-for-byte"
        );
    }

    /// The same delivery guarantee across every shape `cmd.exe` treats
    /// specially. `&`, `|` and `^` are shell metacharacters and MUST still be
    /// interpreted (BashTool is a shell surface, exactly like `sh -c`), and
    /// `%VAR%` must still expand — from the SCRUBBED manifest env, not the
    /// host's.
    #[tokio::test]
    async fn hostile_payload_shapes_keep_their_shell_semantics() {
        assert_eq!(run("echo A & echo B").await.stdout, b"A \r\nB\r\n");
        assert_eq!(
            String::from_utf8_lossy(&run("echo hello | findstr hello").await.stdout).trim_end(),
            "hello"
        );
        assert_eq!(
            String::from_utf8_lossy(&run("echo a^^b").await.stdout).trim_end(),
            "a^b"
        );

        // %VAR% resolves against the manifest env only.
        let (mut manifest, cmd) = cmd_job("echo [%WCORE_CMD_PROBE%]");
        manifest
            .env
            .push(("WCORE_CMD_PROBE".into(), "from-manifest".into()));
        let out = NoSandboxBackend::new()
            .execute(&manifest, cmd)
            .await
            .expect("cmd.exe must run");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim_end(),
            "[from-manifest]"
        );

        // A trailing backslash immediately before a closing quote — the shape
        // CRT escaping doubles and cmd then executes literally.
        assert_eq!(
            String::from_utf8_lossy(&run(r#"echo "C:\dir\""#).await.stdout).trim_end(),
            r#""C:\dir\""#
        );

        // A non-ASCII argument, graded on the FILESYSTEM rather than on
        // stdout. `cmd`'s `echo` writes bytes in the console output code page,
        // so a stdout comparison would be measuring CP437/CP1252 round-tripping
        // (`日本` -> `??`) and not whether the argument was delivered. A file
        // name is UTF-16 end to end, so its existence is a clean statement
        // that the exact characters reached the shell.
        let dir = tempfile::tempdir().expect("temp dir");
        let target = dir.path().join("café 日本.txt");
        let out = run(&format!(r#"echo ok> "{}""#, target.to_string_lossy())).await;
        assert_eq!(
            out.exit_code,
            0,
            "stderr: {:?}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            target.exists(),
            "a non-ASCII, space-bearing argument must reach cmd intact; \
             directory now holds {:?}",
            std::fs::read_dir(dir.path())
                .map(|entries| entries.flatten().map(|e| e.file_name()).collect::<Vec<_>>())
                .unwrap_or_default()
        );
    }

    /// **#943.** A payload that is itself a `cmd /c ...` must arrive whole.
    ///
    /// MEASURED on Windows 11 build 26200 against the shipped v0.13.0 binary
    /// (and identically on 0.12.26-rc.2): `cmd /c echo NESTED` printed
    /// `4e 45 53 54 45 44 22` — `NESTED` plus one `"` the operator never wrote.
    /// `cmd.exe` had taken the quote-PRESERVING branch for its tail, so the
    /// pair `quote_cmd_payload` adds survived into the executed text and its
    /// closing quote reached the child as data. The argv now carries `/S`,
    /// which leaves cmd only the stripping branch.
    ///
    /// The two shapes that measured CLEAN are asserted beside it, because they
    /// are what bounds the fix: `echo NOQUOTE` never took the preserving branch
    /// (`echo` is internal, so nothing on disk answers its executable test) and
    /// neither did the chained form (`&` disqualifies it). A fix that trimmed
    /// trailing quotes instead of correcting the switch would move these.
    #[tokio::test]
    async fn a_nested_cmd_payload_arrives_without_a_stray_quote() {
        for (payload, expected) in [
            ("cmd /c echo NESTED", "NESTED"),
            ("cmd /c echo SHELL_CMD", "SHELL_CMD"),
            ("echo NOQUOTE", "NOQUOTE"),
        ] {
            let output = run(payload).await;
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert_eq!(
                stdout.trim_end(),
                expected,
                "{payload:?} produced {:x?}; a trailing 0x22 is #943 — the \
                 wrapper's own quote executed as part of the command",
                output.stdout
            );
        }
        // cmd's `echo` emits the space that precedes `&&`, so grade each line
        // trimmed: what is under test is the absence of a stray quote, not
        // cmd's trailing whitespace.
        let chained = run("cmd /c echo A && cmd /c echo B").await;
        let text = String::from_utf8_lossy(&chained.stdout);
        let lines: Vec<&str> = text.lines().map(str::trim_end).collect();
        assert_eq!(
            lines,
            vec!["A", "B"],
            "the chained nested form was clean before the fix and must stay so"
        );
    }

    /// **C2.** A `cmd /C` command line stops at the first line break: cmd runs
    /// the prefix and returns ITS status. Measured before the fix, with a
    /// payload whose two lines each write a distinct file: exit 0, empty
    /// stderr, `a.txt` present, `b.txt` ABSENT. Nothing anywhere said half the
    /// command had been discarded.
    ///
    /// This instance is INDEPENDENT of C1: applying only the quoting fix left
    /// it reproducing exactly (measured `a.txt=true b.txt=false` under the
    /// corrected join), so a fix for C1 cannot turn this test green.
    #[tokio::test]
    async fn a_multi_line_command_is_refused_rather_than_half_run_at_exit_zero() {
        let dir = tempfile::tempdir().expect("temp dir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        let payload = format!(
            "echo one>{}\necho two>{}",
            a.to_string_lossy(),
            b.to_string_lossy()
        );

        let (manifest, cmd) = cmd_job(&payload);
        let error = NoSandboxBackend::new()
            .execute(&manifest, cmd)
            .await
            .expect_err("a payload cmd.exe cannot carry whole must not be run at all");

        // `RequestRefused`, not `ExecFailed`: a payload the OS command line
        // cannot carry is a refused REQUEST, and callers that track tool health
        // must not count it as an execution failure (see `error.rs`). The
        // co-located test in `windows_cmdline.rs` was re-pinned when the variant
        // changed; this cfg(windows) one was not, so no non-Windows leg saw it.
        assert!(
            matches!(error, SandboxError::RequestRefused(_)),
            "got {error:?}"
        );
        assert!(
            !a.exists() && !b.exists(),
            "a refused command must leave NO partial side effect: a={} b={}",
            a.exists(),
            b.exists()
        );
    }

    /// Negative control for the refusal: the single-line spelling of the very
    /// same work is accepted and both effects land. Without this, the test
    /// above could be passed by refusing everything.
    #[tokio::test]
    async fn the_single_line_spelling_of_the_same_work_still_runs() {
        let dir = tempfile::tempdir().expect("temp dir");
        let a = dir.path().join("a.txt");
        let b = dir.path().join("b.txt");
        let output = run(&format!(
            "echo one>{} && echo two>{}",
            a.to_string_lossy(),
            b.to_string_lossy()
        ))
        .await;
        assert_eq!(output.exit_code, 0);
        assert!(
            a.exists() && b.exists(),
            "a={} b={}",
            a.exists(),
            b.exists()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolve a real `echo` binary on disk. We do NOT inherit PATH (env
    /// is scrubbed by the backend), so the test passes an absolute path.
    fn echo_path() -> Option<&'static str> {
        ["/bin/echo", "/usr/bin/echo"]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())
    }

    #[tokio::test]
    async fn echo_runs() {
        let Some(echo) = echo_path() else {
            eprintln!("skip: no /bin/echo or /usr/bin/echo on this host");
            return;
        };
        let backend = NoSandboxBackend::new();
        let out = backend
            .execute(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec![echo.into(), "hi".into()],
                    cwd: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(out.exit_code, 0);
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "hi");
        assert_eq!(out.resource_limits, ResourceLimitEnforcement::None);
    }

    #[tokio::test]
    async fn empty_argv_is_error() {
        let backend = NoSandboxBackend::new();
        let err = backend
            .execute(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec![],
                    cwd: None,
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, SandboxError::ExecFailed(_)));
    }

    /// Run `/bin/sh -c <script>` through the buffered path. The manifest env
    /// is empty on purpose — that is the production shape, and `sh` does not
    /// need one.
    #[cfg(unix)]
    async fn buffered_sh(script: &str) -> SandboxOutput {
        let backend = NoSandboxBackend::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(60),
            backend.execute(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec!["/bin/sh".into(), "-c".into(), script.into()],
                    cwd: None,
                },
            ),
        )
        .await
        .expect("a finite producer must not hang the buffered path")
        .expect("a finite producer must not fail the buffered path")
    }

    /// CONTROL for `over_cap_output_is_truncated_not_discarded`: an under-cap
    /// payload arrives byte-for-byte with no marker, so that test cannot pass
    /// by truncating everything.
    #[cfg(unix)]
    #[tokio::test]
    async fn under_cap_output_passes_through_byte_exact() {
        const SIZE: usize = 2_000_000;
        let output = buffered_sh("head -c 2000000 /dev/zero | tr '\\0' A").await;

        assert_eq!(output.exit_code, 0);
        assert_eq!(
            output.stdout.len(),
            SIZE,
            "under-cap output must not be reshaped"
        );
        assert!(
            output.stdout.iter().all(|byte| *byte == b'A'),
            "under-cap output must be the child's exact bytes"
        );
        assert!(
            !String::from_utf8_lossy(&output.stdout).contains("OUTPUT TRUNCATED"),
            "an under-cap payload must not be marked truncated"
        );
    }

    /// FerroxLabs/wayland#1071: 20 MB in, 129 bytes out. Overflowing the cap
    /// discarded the whole stream, including the head the cap was sized to let
    /// the caller keep.
    #[cfg(unix)]
    #[tokio::test]
    async fn over_cap_output_is_truncated_not_discarded() {
        let cap = super::super::BUFFERED_OUTPUT_LIMIT_BYTES;
        let output = buffered_sh("head -c 20000000 /dev/zero | tr '\\0' A").await;

        // The retained BYTES are the assertion, not that the call returned.
        assert!(
            output.stdout.len() > cap,
            "over-cap output retained {} bytes; the cap alone allows {cap}",
            output.stdout.len()
        );
        assert!(
            output.stdout[..cap].iter().all(|byte| *byte == b'A'),
            "the kept bytes must be the child's own, in order"
        );
        let marker = format!(
            "[wcore-sandbox: OUTPUT TRUNCATED. This command produced more than the \
             {cap}-byte buffered output cap. The {cap} bytes above are the start of the \
             output; everything after them was discarded, and the command was STOPPED at \
             that point"
        );
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(
            text.contains(&marker),
            "expected marker {marker:?}; got {:?}",
            &text[cap..]
        );
        assert_eq!(
            output.stdout.len(),
            cap + truncation_marker_len(cap),
            "retained bytes must be exactly the cap plus the marker"
        );
    }

    /// Length of the marker `read_bounded` appends after keeping `kept` bytes.
    /// Rebuilt here rather than imported so the test states the shape it
    /// expects instead of restating the implementation's own expression.
    #[cfg(unix)]
    fn truncation_marker_len(kept: usize) -> usize {
        let cap = super::super::BUFFERED_OUTPUT_LIMIT_BYTES;
        format!(
            "\n[wcore-sandbox: OUTPUT TRUNCATED. This command produced more than the \
             {cap}-byte buffered output cap. The {kept} bytes above are the start of the \
             output; everything after them was discarded, and the command was STOPPED at \
             that point — it did not run to completion, so any work it had not yet done \
             has not happened.]\n"
        )
        .len()
    }

    /// The cap's OTHER job, which truncation must not forfeit: crossing it
    /// stops the child at once. `yes` never ends on its own, so a run that
    /// waited for EOF would sit here until the caller's wall-clock timeout.
    #[cfg(unix)]
    #[tokio::test]
    async fn crossing_the_cap_stops_the_child_and_still_yields_its_head() {
        let Some(yes) = ["/usr/bin/yes", "/bin/yes"]
            .into_iter()
            .find(|path| std::path::Path::new(path).exists())
        else {
            eprintln!("skip: no yes binary on this host");
            return;
        };
        let backend = NoSandboxBackend::new();
        let output = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            backend.execute(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec![yes.into(), "0123456789abcdef".into()],
                    cwd: None,
                },
            ),
        )
        .await
        .expect("crossing the cap must stop an infinite producer promptly")
        .expect("a stopped producer still reports what it printed");

        let cap = super::super::BUFFERED_OUTPUT_LIMIT_BYTES;
        assert!(
            output.stdout.len() > cap,
            "an infinite producer must still yield its head; got {} bytes",
            output.stdout.len()
        );
        assert_eq!(
            output.stdout.len(),
            cap + truncation_marker_len(cap),
            "host memory must stay bounded by the cap plus the marker"
        );
        assert!(
            String::from_utf8_lossy(&output.stdout).contains("OUTPUT TRUNCATED"),
            "a truncated stream must say so"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_stream_kills_direct_child_and_background_descendant() {
        use std::sync::Arc;

        // Second of the four hand-rolled zombie checks; Linux-only, so this
        // containment assertion was still zombie-blind on macOS. Replaced by
        // the single cross-platform probe. See `.planning/ZOMBIE-PROBE.md`.
        use wcore_types::process_liveness::process_is_alive as process_running;

        async fn read_pid(path: &std::path::Path) -> u32 {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    if let Ok(raw) = std::fs::read_to_string(path)
                        && let Ok(pid) = raw.trim().parse()
                    {
                        break pid;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("child must publish its PID")
        }

        async fn wait_gone(pid: u32) {
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                while process_running(pid) {
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("process group member must die after receiver drop");
        }

        let dir = tempfile::tempdir().unwrap();
        let shell_pid_file = dir.path().join("shell.pid");
        let child_pid_file = dir.path().join("child.pid");
        let script = format!(
            "echo $$ > '{}'; sleep 30 & echo $! > '{}'; wait",
            shell_pid_file.display(),
            child_pid_file.display()
        );
        let backend = Arc::new(NoSandboxBackend::new());
        let rx = backend
            .execute_streaming(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec!["/bin/sh".into(), "-c".into(), script],
                    cwd: None,
                },
            )
            .unwrap();
        let shell_pid = read_pid(&shell_pid_file).await;
        let child_pid = read_pid(&child_pid_file).await;
        assert!(process_running(shell_pid));
        assert!(process_running(child_pid));

        drop(rx);

        wait_gone(shell_pid).await;
        wait_gone(child_pid).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn dropping_execute_future_prevents_delayed_descendant_effect() {
        let dir = tempfile::tempdir().expect("create sentinel directory");
        let started = dir.path().join("started");
        let sentinel = dir.path().join("escaped");
        let backend = NoSandboxBackend::new();
        let manifest = SandboxManifest::default();

        {
            let execution = backend.execute(
                &manifest,
                SandboxCommand {
                    argv: vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        "/usr/bin/touch \"$1\"; (/bin/sleep 1; /usr/bin/touch \"$2\") & wait"
                            .into(),
                        "wcore-sentinel".into(),
                        started.to_string_lossy().into_owned(),
                        sentinel.to_string_lossy().into_owned(),
                    ],
                    cwd: None,
                },
            );
            tokio::pin!(execution);
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    tokio::select! {
                        result = &mut execution => {
                            panic!("child exited before cancellation: {result:?}");
                        }
                        _ = tokio::time::sleep(std::time::Duration::from_millis(10)) => {
                            if started.exists() {
                                break;
                            }
                        }
                    }
                }
            })
            .await
            .expect("child must start before future drop");
        }

        tokio::time::sleep(std::time::Duration::from_millis(1_250)).await;
        assert!(
            !sentinel.exists(),
            "background descendant wrote after execute future drop"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn successful_direct_child_cannot_leave_background_descendant() {
        let dir = tempfile::tempdir().expect("create sentinel directory");
        let sentinel = dir.path().join("escaped-after-success");
        let backend = NoSandboxBackend::new();
        let output = backend
            .execute(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec![
                        "/bin/sh".into(),
                        "-c".into(),
                        "(/bin/sleep 1; /usr/bin/touch \"$1\") &".into(),
                        "wcore-success-sentinel".into(),
                        sentinel.to_string_lossy().into_owned(),
                    ],
                    cwd: None,
                },
            )
            .await
            .expect("direct child should exit successfully");
        assert_eq!(output.exit_code, 0);

        tokio::time::sleep(std::time::Duration::from_millis(1_250)).await;
        assert!(
            !sentinel.exists(),
            "background descendant survived successful direct-child completion"
        );
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn dropping_stream_reaps_windows_job_descendant() {
        use std::sync::Arc;

        let system_root = std::env::var_os("SYSTEMROOT").expect("SYSTEMROOT must be set");
        let cmd = std::path::PathBuf::from(&system_root)
            .join("System32")
            .join("cmd.exe");
        let choice = std::path::PathBuf::from(system_root)
            .join("System32")
            .join("choice.exe");
        let dir = tempfile::tempdir().expect("create process-tree test directory");
        let heartbeat = dir.path().join("heartbeat.txt");
        let script = dir.path().join("heartbeat.cmd");
        std::fs::write(
            &script,
            format!(
                "@echo off\r\n:loop\r\necho x>>heartbeat.txt\r\n\"{}\" /t 1 /d y /n >nul\r\ngoto loop\r\n",
                choice.display()
            ),
        )
        .expect("write process-tree heartbeat script");
        // The nested command line carries NO quotes and NO absolute paths. The
        // previous form embedded two quoted absolute paths
        // (`"<cmd.exe>" /d /c "<script>"`); passing that through the argv vector
        // let std's `CommandLineToArgvW` quoting escape the inner quotes as
        // `\"`, which cmd.exe does not understand, so the inner shell never
        // launched and the heartbeat was never written. Setting the child's
        // working directory to the temp directory lets both the script and its
        // output file be BARE relative names, which removes the nesting instead
        // of escaping it harder. `cmd` resolves through PATH.
        //
        // Two process levels are RETAINED deliberately: the reaped descendant is
        // the inner shell, so collapsing this to a single level would delete the
        // property the test exists to prove.
        let nested = "cmd /d /c heartbeat.cmd".to_owned();
        let backend = Arc::new(NoSandboxBackend::new());
        let rx = backend
            .execute_streaming(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec![
                        cmd.display().to_string(),
                        "/d".into(),
                        "/s".into(),
                        "/c".into(),
                        nested,
                    ],
                    cwd: Some(dir.path().to_path_buf()),
                },
            )
            .expect("spawn nested Windows command");

        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                if std::fs::metadata(&heartbeat)
                    .map(|meta| meta.len() > 0)
                    .unwrap_or(false)
                {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("descendant must begin writing its heartbeat");

        drop(rx);
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let settled = std::fs::metadata(&heartbeat)
            .expect("heartbeat remains readable")
            .len();
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let final_len = std::fs::metadata(&heartbeat)
            .expect("heartbeat remains readable")
            .len();
        assert_eq!(final_len, settled, "Windows Job descendant survived drop");
    }

    #[test]
    fn warn_once_is_idempotent() {
        // The warn-once contract: calling `warn_once_sandbox_disabled` any
        // number of times is safe and produces exactly one warn over the
        // lifetime of the process. We cannot directly observe the log line
        // from inside the test binary (no tracing subscriber wired here),
        // so we instead assert via `Once::is_completed()` that the `Once`
        // transitions to the completed state and stays there across
        // repeated calls.
        //
        // Note: `WARN_ONCE` is process-global; other tests in this binary
        // may have already invoked it. That is fine — completion is
        // monotonic, so the assertions below hold either way.
        warn_once_sandbox_disabled();
        assert!(
            WARN_ONCE.is_completed(),
            "first call must mark Once complete"
        );
        // Repeated calls must not panic and must not flip the state.
        for _ in 0..5 {
            warn_once_sandbox_disabled();
        }
        assert!(
            WARN_ONCE.is_completed(),
            "Once remains complete after repeats"
        );
    }

    #[tokio::test]
    async fn execute_streaming_yields_chunks_then_exit() {
        use crate::SandboxChunk;
        use std::sync::Arc;
        let Some(echo) = echo_path() else {
            eprintln!("skip: no /bin/echo or /usr/bin/echo on this host");
            return;
        };
        let backend: Arc<NoSandboxBackend> = Arc::new(NoSandboxBackend::new());
        let mut rx = backend
            .execute_streaming(
                &SandboxManifest::default(),
                SandboxCommand {
                    argv: vec![echo.into(), "stream_hi".into()],
                    cwd: None,
                },
            )
            .expect("execute_streaming must return a receiver");

        let mut stdout = Vec::new();
        let mut exit = None;
        while let Some(chunk) = rx.recv().await {
            match chunk {
                SandboxChunk::Stdout(b) => stdout.extend_from_slice(&b),
                SandboxChunk::Stderr(_) => {}
                SandboxChunk::Exit {
                    exit_code,
                    resource_limits,
                } => {
                    exit = Some((exit_code, resource_limits));
                }
            }
        }
        assert_eq!(
            String::from_utf8_lossy(&stdout).trim(),
            "stream_hi",
            "stdout chunk must carry the child's output"
        );
        let (code, limits) = exit.expect("a terminal Exit chunk must arrive");
        assert_eq!(code, 0);
        assert_eq!(limits, ResourceLimitEnforcement::None);
    }

    #[tokio::test]
    async fn env_is_scrubbed_then_repopulated() {
        // Skip on hosts without `/usr/bin/env` (e.g. Windows CI). The
        // backend MUST scrub host env then inject only manifest env.
        let env_bin = "/usr/bin/env";
        if !std::path::Path::new(env_bin).exists() {
            eprintln!("skip: no /usr/bin/env on this host");
            return;
        }
        // SAFETY: test-only env mutation; serial-tests would be nicer but
        // the key is unique to this test and no other thread reads it.
        unsafe {
            std::env::set_var("WAYLAND_SANDBOX_TEST_LEAK", "leaked");
        }
        let backend = NoSandboxBackend::new();
        let mut manifest = SandboxManifest::default();
        manifest.env.push(("FOO".into(), "bar".into()));
        let out = backend
            .execute(
                &manifest,
                SandboxCommand {
                    argv: vec![env_bin.into()],
                    cwd: None,
                },
            )
            .await
            .unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("FOO=bar"), "FOO must be set: {stdout}");
        assert!(
            !stdout.contains("WAYLAND_SANDBOX_TEST_LEAK"),
            "host env must be scrubbed: {stdout}"
        );
    }
}
