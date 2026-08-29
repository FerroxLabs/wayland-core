//! UAT-T3 regression suite: formatters fed by the REAL tools.
//!
//! # Why this file exists
//!
//! Every per-tool formatter shipped with a green unit-test suite while
//! rendering ``Ran `?` · exit 0 · 0 bytes`` for every shell command the
//! product ever ran, including ones that failed. The tests passed because
//! each of them **constructed the payload by hand**, in the shape the
//! formatter's author had imagined:
//!
//! ```ignore
//! let payload = json!({ "cmd": "ls -la /tmp", "exit_code": 0, "stdout": "…" });
//! ```
//!
//! No tool has ever emitted that object. `BashTool` returns plain text. So
//! the suite was self-consistent and disconnected from the product: it could
//! not fail for the defect it was nominally covering. Eleven of twelve
//! formatters were mismatched with their tool and every one of them was green.
//!
//! # The rule this file enforces
//!
//! **A formatter test's input must come from the producer, not from a
//! literal.** Every case below actually executes the real tool and pipes its
//! real `ToolResult.content` through the same `parse_payload` fallback the
//! TUI uses, then asserts on what the formatter renders.
//!
//! That is what makes it drift-proof. Pasting `"Exit code: 0\nSTDOUT:\n…"`
//! into a fixture would re-create the original disease one level down: the
//! fixture would be a second invented shape, free to drift from `bash.rs`
//! silently. Because these cases call `BashTool::execute`, changing the
//! format string in `wcore-tools/src/bash.rs` breaks them immediately.
//!
//! # Coverage, stated honestly
//!
//! Only tools that run with no network and no external backend are executed
//! here: `Bash`, `Write`, `Edit`, `Read`. The remaining formatters
//! (`web`, `web_fetch`, `vision`, `transcribe`, `image_gen`, `tts`,
//! `github`, `discord`, `homeassistant`) need a live backend or a credential
//! and are **not** covered by an executed-tool case — their key-shape fixes
//! are asserted in their own modules against shapes read out of the tool
//! source. That is weaker, and is called out rather than papered over.
//!
//! # Why this is an integration binary, not a unit-test module
//!
//! It started life inside the lib test binary. Executing the real `BashTool`
//! spawns sandboxed child processes, and that extra load made
//! `tui::surfaces::tests::*_f14` fail intermittently (3-4 of them per run,
//! varying) while the same commit with this module disabled was green 2/2 and
//! the lane base was green 3/3. The cause is in those tests' own helper:
//! `await_session_switch` bounds its wait with `for _ in 0..100 { …
//! yield_now().await }` — a budget in scheduler yields, not in time or in a
//! completion signal — so a loaded machine exhausts it before the background
//! session I/O finishes and the helper panics "session switch did not
//! complete". It cannot distinguish broken from busy.
//!
//! That latent fragility is NOT this lane's to fix and is reported separately.
//! Living in a separate binary keeps these process-spawning cases out of that
//! binary's scheduler entirely.

use std::time::Duration;

use serde_json::Value;
use wcore_tools::Tool;

use wcore_cli::tui::theme::Theme;
use wcore_cli::tui::tool_formatters::formatter_for;

/// The TUI's own payload fallback, reproduced exactly.
///
/// Mirrors `widgets/toolcard.rs::parse_payload` and
/// `surfaces/workspace.rs::parse_card_payload` (both private). If those two
/// ever stop agreeing with this, the assertions here stop describing what a
/// user sees — so the shape is duplicated deliberately and noted.
fn as_card_payload(tool_output: &str) -> Value {
    if tool_output.is_empty() {
        return Value::Null;
    }
    serde_json::from_str(tool_output).unwrap_or_else(|_| Value::String(tool_output.to_string()))
}

/// Run a real tool and render its real result the way the card does.
fn render_real(tool: &dyn Tool, input: Value) -> (String, Vec<String>) {
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("test runtime")
        .block_on(tool.execute(input));
    let payload = as_card_payload(&result.content);
    let f = formatter_for(tool.name());
    let summary = f.summary_line(&payload, Duration::ZERO);
    let detail = f
        .detail_lines(&payload, &Theme::hearth())
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect();
    (summary, detail)
}

/// The real shell these Bash cases drive, and why two expectations are
/// split by DIALECT rather than by platform.
///
/// `BashTool` runs the command through `wcore_config::shell::
/// bash_shell_argv_prefix` — `sh -c <cmd>` on Unix, and on Windows either a
/// resolved `<git>\bash.exe -c <cmd>` or `cmd /S /C <cmd>`, whichever that
/// host has (FerroxLabs/wayland#1164). The formatter properties under test (real exit code, real byte
/// count, real stderr) are platform-neutral, but the *inputs that produce
/// them* are not, and hard-coding the Unix answers made two of these cases
/// assert something false on Windows:
///
/// * `;` is a statement separator in `sh` and **ordinary text** in `cmd`.
///   `echo BOOM_TOKEN 1>&2; exit 1` therefore echoed the literal
///   `BOOM_TOKEN ; exit 1` to stderr and exited **0** — a command that
///   genuinely succeeded. The test demanded `exit 1` from it. `cmd`'s
///   unconditional separator is `&` and its non-zero exit is `exit /b N`.
/// * `echo` terminates its line with `\n` under `sh` (15 bytes) and `\r\n`
///   under `cmd` (16 bytes).
///
/// Measured on Windows 10.0.26200 at the parent commit, the formatter
/// rendered `exit 0 · 0 bytes stdout · 21 bytes stderr` and
/// `exit 0 · 16 bytes stdout`. Both are the TRUTH about what `cmd` did.
/// Neither was a misformat, and `bash_failure_never_reports_exit_zero`
/// (which uses `exit 3`, valid in both dialects) passed there unchanged —
/// the fail-loud guarantee held. So the defect was in this file, not in the
/// formatter.
///
/// UPDATED for FerroxLabs/wayland-core#387: on Windows the split is no longer
/// by PLATFORM, because the platform no longer determines the interpreter.
/// Since FerroxLabs/wayland#1164 a Windows host with Git for Windows installed
/// drives a real `bash` and one without it still drives `cmd`, so both of the
/// expectations below move within Windows. They are now selected from the
/// product's own resolved prefix.
///
/// Each answer is still HARD-CODED per dialect, and deliberately: it is
/// computed from an INPUT (which interpreter was selected) and never from the
/// output under test. An expectation derived from the result cannot fail,
/// which is the exact disease this file was written to cure.
///
/// Pinning these to `cmd` with `WAYLAND_BASH_SHELL=cmd` would also make them
/// green, and would be worse than the failure: it tests an interpreter the
/// product does not use on that host.
fn posix_interpreter() -> bool {
    wcore_config::shell::shell_prefix_is_posix(&wcore_config::shell::bash_shell_argv_prefix())
}

/// `echo HELLO_FROM_UAT` terminates its line with `\n` under a POSIX shell
/// (15 bytes) and `\r\n` under `cmd` (16).
fn echo_stdout_bytes() -> usize {
    if posix_interpreter() { 15 } else { 16 }
}

/// A command that writes to stderr AND exits non-zero, in the resolved shell's
/// own dialect. Both halves matter: the exit code proves the card cannot claim
/// success on a failure, the stderr proves a failure still explains itself.
///
/// `;` is a statement separator in a POSIX shell and ordinary text in `cmd`,
/// whose unconditional separator is `&` and whose non-zero exit is `exit /b N`.
/// `exit /b 1` handed to a real bash is `exit` with two arguments, which is a
/// usage error exiting 2 — the shape #387 reported.
fn stderr_then_fail() -> &'static str {
    if posix_interpreter() {
        "echo BOOM_TOKEN 1>&2; exit 1"
    } else {
        "echo BOOM_TOKEN 1>&2 & exit /b 1"
    }
}

/// Assert the invariant that the whole defect class violated.
fn assert_never_fabricates(summary: &str, detail: &[String], ctx: &str) {
    assert!(
        !summary.contains("Ran `?`"),
        "{ctx}: fabricated a command placeholder: {summary}"
    );
    assert!(
        !summary.contains("completed in 0.0s"),
        "{ctx}: fabricated a duration the card does not measure: {summary}"
    );
    let _ = detail;
}

#[test]
fn bash_success_renders_the_real_exit_code_and_byte_count() {
    let (summary, detail) = render_real(
        &wcore_tools::bash::BashTool,
        serde_json::json!({ "command": "echo HELLO_FROM_UAT" }),
    );

    // The exact case the UAT reported as `Ran `?` · exit 0 · 0 bytes`.
    // The real payload is "HELLO_FROM_UAT" plus the shell's own line
    // terminator — see `echo_stdout_bytes`.
    assert!(
        summary.contains("exit 0"),
        "real exit code missing: {summary}"
    );
    assert!(
        summary.contains(&format!("{} bytes stdout", echo_stdout_bytes())),
        "byte count still wrong (was always 0): {summary}"
    );
    assert!(
        detail.iter().any(|l| l.contains("HELLO_FROM_UAT")),
        "stdout still not displayed: {detail:?}"
    );
    assert_never_fabricates(&summary, &detail, "bash success");
}

#[test]
fn bash_failure_never_reports_exit_zero() {
    // THE headline defect: the card said `exit 0` on a call the adjacent
    // glyph marked `error`.
    let (summary, detail) = render_real(
        &wcore_tools::bash::BashTool,
        serde_json::json!({ "command": "exit 3" }),
    );
    assert!(
        summary.contains("exit 3"),
        "real non-zero exit code missing: {summary}"
    );
    assert!(
        !summary.contains("exit 0"),
        "still claiming success on a failed command: {summary}"
    );
    assert_never_fabricates(&summary, &detail, "bash failure");
}

#[test]
fn bash_stderr_is_surfaced() {
    let (summary, detail) = render_real(
        &wcore_tools::bash::BashTool,
        serde_json::json!({ "command": stderr_then_fail() }),
    );
    assert!(summary.contains("exit 1"), "wrong exit code: {summary}");
    assert!(
        detail.iter().any(|l| l.contains("BOOM_TOKEN")),
        "stderr not surfaced, so a failure explains nothing: {detail:?}"
    );
}

#[test]
fn write_then_edit_then_read_render_the_tools_own_words() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("uat_t3.txt");
    let path_s = path.to_string_lossy().to_string();

    let (summary, _) = render_real(
        &wcore_tools::write::WriteTool::new(None),
        serde_json::json!({ "file_path": path_s, "content": "alpha\nbeta\n" }),
    );
    // `WriteTool` returns "Created <path> (2 lines)". Previously the card
    // rendered `file_ops ?` — no path, no count, nothing true.
    assert!(
        summary.contains("uat_t3.txt"),
        "write summary lost the path: {summary}"
    );
    assert!(
        !summary.contains("file_ops ?"),
        "write still renders the fabricated placeholder: {summary}"
    );
    assert!(
        !summary.contains('?'),
        "write summary still fabricates: {summary}"
    );
}

#[test]
fn write_summary_keeps_the_filename_at_a_realistic_project_path_depth() {
    // The short-path case above passes on Linux by accident: a bare
    // `/tmp/.tmpXXXXXX/uat_t3.txt` line is ~44 chars, under the 60-char
    // preview budget, so no truncation happens at all. macOS and Windows
    // tempdirs are longer, which is the ONLY reason the defect showed up
    // there first. Any real checkout path — `crates/<crate>/src/<module>/…` —
    // blows the budget on every platform, so pin the invariant with a path
    // deep enough to truncate everywhere.
    let dir = tempfile::tempdir().expect("tempdir");
    let nested = dir
        .path()
        .join("crates")
        .join("wcore-agent")
        .join("src")
        .join("orchestration")
        .join("workflow");
    std::fs::create_dir_all(&nested).expect("nested dirs");
    let path = nested.join("runner.rs");
    let path_s = path.to_string_lossy().to_string();
    assert!(
        format!("Created {path_s} (2 lines)").chars().count() > 60,
        "fixture path is too short to exercise truncation: {path_s}"
    );

    let (summary, _) = render_real(
        &wcore_tools::write::WriteTool::new(None),
        serde_json::json!({ "file_path": path_s, "content": "alpha\nbeta\n" }),
    );
    assert!(
        summary.contains("runner.rs"),
        "write summary lost the filename at project-path depth: {summary}"
    );
    assert!(
        summary.contains("(2 lines)"),
        "write summary lost the line count at project-path depth: {summary}"
    );
    assert!(
        summary.starts_with("Created"),
        "write summary lost the verb: {summary}"
    );
    assert!(
        summary.chars().count() <= 60,
        "write summary blew the preview budget: {summary}"
    );
}

#[test]
fn formatter_never_invents_on_an_empty_payload() {
    // A card with no output yet (running / cancelled) must not render a
    // completed-looking summary for ANY tool the dispatcher knows.
    for name in [
        "Bash",
        "Read",
        "Write",
        "Edit",
        "web",
        "WebFetch",
        "vision_analyze",
        "transcribe_audio",
        "image_generate",
        "text_to_speech",
        "github_api",
        "discord_server",
        "homeassistant",
        "some_unknown_tool_zzz",
    ] {
        let s = formatter_for(name).summary_line(&Value::Null, Duration::ZERO);
        assert!(
            !s.contains('?'),
            "{name}: fabricated a `?` placeholder on an empty payload: {s}"
        );
        assert!(
            !s.contains("exit 0"),
            "{name}: claimed a successful exit with no payload: {s}"
        );
        assert!(
            !s.contains("0 bytes"),
            "{name}: claimed a zero byte count with no payload: {s}"
        );
        assert!(
            !s.contains("0.0s"),
            "{name}: rendered an unmeasured duration: {s}"
        );
    }
}

/// The known-shape cases for the tools this suite cannot execute.
///
/// These payloads are transcribed from the tool sources (each cited inline),
/// so they are weaker than the executed cases above — a tool could change its
/// keys and this would keep passing. Recorded as a limitation, not a claim.
#[test]
fn json_tools_read_the_keys_their_sources_actually_emit() {
    let cases: &[(&str, Value, &str)] = &[
        // wcore-tools/src/web_tools.rs::dispatch_search
        (
            "web",
            serde_json::json!({"success": true, "data": {"web": [{"title":"A","url":"https://a.com"}]}}),
            "Found 1 result",
        ),
        // wcore-tools/src/web_fetch.rs (FetchOutcome::Ok)
        (
            "WebFetch",
            serde_json::json!({"url":"https://x.com/p","status":200,"content_type":"text/html","text":"abcde","truncated":false}),
            "5 bytes",
        ),
        // wcore-tools/src/vision_tools.rs
        (
            "vision_analyze",
            serde_json::json!({"success":true,"analysis":"a cat","mime":"image/png","bytes":99}),
            "99 bytes",
        ),
        // wcore-tools/src/transcription_tools.rs
        (
            "transcribe_audio",
            serde_json::json!({"success":true,"transcript":"hi","mime":"audio/wav","bytes":10,"language":"en","segments":[{"text":"hi"}]}),
            "1 segment",
        ),
        // wcore-tools/src/image_generation_tool.rs
        (
            "image_generate",
            serde_json::json!({"success":true,"image":"https://i/x.png","usedProvider":"Google Imagen","width":1024,"height":768}),
            "Google Imagen",
        ),
        // wcore-tools/src/tts_tool.rs
        (
            "text_to_speech",
            serde_json::json!({"success":true,"file_path":"/tmp/a.wav","provider":"openai","format":"wav","bytes_written":4242}),
            "a.wav",
        ),
        // wcore-tools/src/homeassistant_tool.rs::ok_result
        (
            "homeassistant",
            serde_json::json!({"success":true,"result":{"service":"light.turn_on","affected_entities":["light.kitchen"]}}),
            "1 entity",
        ),
    ];

    for (tool, payload, expected) in cases {
        let s = formatter_for(tool).summary_line(payload, Duration::ZERO);
        assert!(
            s.contains(expected),
            "{tool}: expected {expected:?} in summary, got {s:?}"
        );
        assert!(!s.contains('?'), "{tool}: still fabricating: {s}");
    }
}
