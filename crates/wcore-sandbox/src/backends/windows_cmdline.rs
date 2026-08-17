//! Windows `cmd.exe` command-line construction — the one place that knows a
//! `cmd /C` payload is NOT argv.
//!
//! # Why this module exists
//!
//! Every Windows process receives ONE command-line string. `CreateProcessW`
//! does not take an argv vector, so anything that accepts argv must join it.
//! Rust's `std::process::Command` joins with the MSVC C-runtime /
//! `CommandLineToArgvW` rules: an argument containing a `"` is emitted with
//! that quote **backslash-escaped** (`\"`), because that is what a CRT-parsing
//! child needs to see in order to recover the original argument.
//!
//! `cmd.exe` is not a CRT-parsing child for its `/C` payload. It strips the
//! outer quote pair off the remainder of the line and then executes the rest
//! **verbatim**, with no backslash processing at all. So the `\` that std
//! inserted as an escape is passed on to the real child as a literal
//! backslash, and the `"` it was escaping is passed on as an argument
//! delimiter. The payload the operator wrote is not the payload that runs.
//!
//! Measured on Windows 11 26200 (`cmd /C` + a child that prints its raw
//! command line and its parsed argv):
//!
//! | asked for | child actually received |
//! |---|---|
//! | `node -e "require('fs').writeFileSync('n.txt', 'ok')"` | `argv[2]="\"require('fs').writeFileSync('n.txt',"`, `argv[3]="'ok')\""` |
//! | `python3 -c "open('p.txt','w').write('ok')"` | `argv[2]="\"open('p.txt','w').write('ok')\""` |
//!
//! The first is the loud failure (node: `Unterminated string constant`). The
//! second is the DANGEROUS one: Python was handed a program whose entire text
//! is a quoted string literal, so it evaluated a `str`, discarded it, wrote no
//! file, printed nothing, and **exited 0**. Same corruption, one loud symptom
//! and one silent one.
//!
//! [`quote_cmd_payload`] is the correct spelling: a single outer double-quote
//! pair with the payload's own quotes passed through untouched. `cmd` strips
//! exactly that outer pair and executes what the caller wrote. This is a
//! quoting-layer concern only — argv discipline is preserved (the payload is
//! still one entry the caller supplied, never a `format!`-interpolated shell
//! string) and no sandbox boundary is involved.
//!
//! # The other thing a `cmd /C` command line cannot carry
//!
//! It cannot carry a line break. `cmd` stops reading its command line at the
//! first CR or LF and runs only what precedes it — silently, and with the exit
//! status of that prefix. Measured, with a two-line payload where each line
//! writes a distinct file: exit 0, empty stderr, `a.txt` present, `b.txt`
//! absent. A multi-line command is extremely ordinary for an agent shell, and
//! `sh -c` runs every line, so this is a real cross-platform divergence that
//! reports success for work it never did. [`reject_undeliverable_cmd_payload`]
//! turns it into a refusal, because an exit code that cannot be trusted is
//! worse than no exit code.
//!
//! Compiled on every target — like `windows_job_object` — so the join rules
//! and the refusal are unit-testable on any host.

// Per-target: every production caller is `#[cfg(windows)]`, so off Windows
// these read as dead. Compiling them anyway is deliberate — the rules are pure
// string handling, and the tests below are the only thing that can catch a
// regression in them on a Linux or macOS CI job.
#![allow(dead_code)]

use crate::error::{Result, SandboxError};

/// True when `program` names `cmd.exe` as its exact FINAL PATH COMPONENT
/// (case-insensitive, `.exe` optional).
///
/// Equality on the final component — not a suffix test — so `notcmd.exe` and
/// `foocmd` do not match. Both `\` and `/` count as separators, which is what
/// the Windows path parser does.
pub(crate) fn program_is_cmd(program: &str) -> bool {
    let lowered = program.to_ascii_lowercase();
    // `std::path::Path` only splits on `/` as well as `\` when compiled for
    // Windows, and this module compiles everywhere, so split explicitly.
    let final_component = lowered.rsplit(['\\', '/']).next().unwrap_or(&lowered);
    final_component == "cmd" || final_component == "cmd.exe"
}

/// Index of the `cmd /C` (or `/K`) payload inside `argv`, if this argv is a
/// `cmd.exe` invocation that has one.
///
/// Returns `None` for every other shape — a non-`cmd` program, a `cmd`
/// invocation with no `/c`/`/k`, or a `/c` that is the last entry — so callers
/// keep ordinary argv handling for everything that is genuinely argv.
pub(crate) fn cmd_payload_index(argv: &[String]) -> Option<usize> {
    let program = argv.first()?;
    if !program_is_cmd(program) {
        return None;
    }
    let flag_idx = argv.iter().position(|a| {
        let flag = a.to_ascii_lowercase();
        flag == "/c" || flag == "/k"
    })?;
    let payload_idx = flag_idx + 1;
    (payload_idx < argv.len()).then_some(payload_idx)
}

/// Quote a `cmd.exe` `/C`/`/K` payload for the RAW command line `cmd` re-reads:
/// one outer double-quote pair, inner quotes verbatim.
///
/// `cmd` strips exactly the outer pair and executes the remainder as written.
/// Deliberately NOT the CRT `\"` escaping `std::process::Command` applies —
/// see the module docs for the measured corruption that produces.
///
/// PRECONDITION: the invocation must carry `/S`. Without it `cmd` has a second,
/// quote-PRESERVING branch for its tail and takes it whenever the tail holds
/// exactly two quotes, no `&<>()@^|` between them, whitespace between them, and
/// text between them that names an executable file — which an ordinary
/// `<program> <args>` payload inside this pair satisfies. The pair then survives
/// into the executed text and its closing `"` reaches the child as data
/// (measured: `cmd /c echo NESTED` printed `NESTED"`, FerroxLabs/wayland#943).
/// Build the argv from `wcore_config::shell::windows_cmd_payload_prefix`, which
/// supplies the switch.
pub(crate) fn quote_cmd_payload(payload: &str) -> String {
    let mut out = String::with_capacity(payload.len() + 2);
    out.push('"');
    out.push_str(payload);
    out.push('"');
    out
}

/// Refuse a `cmd /C` payload this transport cannot deliver intact.
///
/// The only such payload is one containing CR or LF: `cmd` reads up to the
/// first line break and runs that prefix, reporting ITS exit status. The
/// caller's remaining lines never run and nothing says so. Refusing is the
/// only honest option — a successful-looking exit code for work that did not
/// happen is the failure mode this whole path was fixed for.
pub(crate) fn reject_undeliverable_cmd_payload(payload: &str) -> Result<()> {
    let Some(break_at) = payload.find(['\r', '\n']) else {
        return Ok(());
    };
    let skipped = payload[break_at..]
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    Err(SandboxError::RequestRefused(format!(
        "a Windows `cmd /C` command line cannot carry a line break. \
         cmd.exe stops reading at the first CR/LF, so only `{}` would have run \
         and the remaining {skipped} line(s) would have been skipped silently \
         while the shell still reported success. Rewrite this as a single line \
         (join the steps with `&&`), or write the script to a file and run the \
         file.",
        payload[..break_at].trim_end()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_final_component_is_cmd() {
        for yes in [
            "cmd",
            "cmd.exe",
            "CMD.EXE",
            "Cmd",
            r"C:\Windows\System32\cmd.exe",
            "C:/Windows/System32/CMD.EXE",
        ] {
            assert!(program_is_cmd(yes), "{yes} must be recognized as cmd.exe");
        }
        for no in [
            "notcmd.exe",
            "foocmd",
            "cmd.exe.bak",
            r"C:\tools\mycmd.exe",
            "powershell",
            "sh",
        ] {
            assert!(
                !program_is_cmd(no),
                "{no} must NOT be recognized as cmd.exe"
            );
        }
    }

    #[test]
    fn payload_index_is_found_only_for_a_cmd_invocation_that_has_one() {
        let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            cmd_payload_index(&argv(&["cmd", "/S", "/C", "echo hi"])),
            Some(3),
            "the default BashTool argv on Windows — the `/S` that makes cmd \
             strip this payload's outer pair must not hide the payload from \
             the rule that adds it (#943)"
        );
        assert_eq!(cmd_payload_index(&argv(&["cmd", "/C", "echo hi"])), Some(2));
        assert_eq!(cmd_payload_index(&argv(&["cmd", "/d", "/c", "x"])), Some(3));
        assert_eq!(cmd_payload_index(&argv(&["cmd", "/K", "x"])), Some(2));
        // Not a cmd invocation: `/c` here is the child program's own flag and
        // must keep ordinary CRT argv quoting.
        assert_eq!(cmd_payload_index(&argv(&["python", "/c", "x"])), None);
        assert_eq!(cmd_payload_index(&argv(&["sh", "-c", "echo hi"])), None);
        // A cmd invocation with nothing to run.
        assert_eq!(cmd_payload_index(&argv(&["cmd"])), None);
        assert_eq!(cmd_payload_index(&argv(&["cmd", "/C"])), None);
        assert_eq!(cmd_payload_index(&[]), None);
    }

    /// The payload's own quotes must survive byte-for-byte. A `\"` anywhere in
    /// this output is the defect: `cmd` does not undo backslash escapes, so it
    /// would reach the real child as a literal backslash.
    #[test]
    fn payload_quoting_adds_one_outer_pair_and_escapes_nothing() {
        assert_eq!(
            quote_cmd_payload(r#"node -e "require('fs').writeFileSync('n.txt', 'ok')""#),
            r#""node -e "require('fs').writeFileSync('n.txt', 'ok')"""#
        );
        assert_eq!(quote_cmd_payload("echo hi"), r#""echo hi""#);
        assert_eq!(quote_cmd_payload(r"echo C:\dir\"), r#""echo C:\dir\""#);
        assert_eq!(quote_cmd_payload("echo café 日本"), r#""echo café 日本""#);
        assert!(
            !quote_cmd_payload(r#"echo "a""#).contains(r#"\""#),
            "a backslash-escaped quote would be executed literally by cmd.exe"
        );
    }

    #[test]
    fn a_single_line_payload_is_deliverable() {
        for ok in [
            "echo hi",
            r#"python3 -c "open('p.txt','w').write('ok')""#,
            "cargo build && cargo test",
            "",
        ] {
            assert!(reject_undeliverable_cmd_payload(ok).is_ok(), "{ok:?}");
        }
    }

    /// cmd truncates at the first line break and reports the prefix's status.
    /// Anything we cannot deliver has to be refused, never silently trimmed.
    #[test]
    fn a_multi_line_payload_is_refused_and_the_message_names_the_loss() {
        for payload in [
            "echo one>a.txt\necho two>b.txt",
            "echo one>a.txt\r\necho two>b.txt",
            "echo one>a.txt\recho two>b.txt",
        ] {
            let error = reject_undeliverable_cmd_payload(payload)
                .expect_err("a multi-line cmd payload must be refused");
            // The VARIANT is load-bearing, not just the text: `RequestRefused`
            // is what tells a tool-health tracker that nothing ran and the
            // host is fine. Graded as `ExecFailed` this refusal counted as a
            // sick shell and took the Bash tool out for a full cooldown.
            assert!(
                matches!(error, SandboxError::RequestRefused(_)),
                "a payload this transport cannot carry is a refusal, not an \
                 execution failure: {error:?}"
            );
            let text = error.to_string();
            assert!(
                text.contains("echo one>a.txt"),
                "the message must name the only line that would have run: {text}"
            );
            assert!(
                text.contains("skipped silently"),
                "the message must say why refusing beats a misleading exit 0: {text}"
            );
        }
    }
}
