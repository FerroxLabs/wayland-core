//! Native per-command Bash output compaction (RTK-style, engine-owned).
//!
//! `compact_bash` shrinks verbose shell-command output (cargo/git/test/grep)
//! to a signal-preserving compact form before it enters the model's
//! transcript. It is deterministic (engine-side), fail-open (never drops the
//! error signal; any parser uncertainty falls back to a generic classifier,
//! then to raw), and size-gated (small output is returned verbatim).
//!
//! Dispatch is on the command PREFIX (program + subcommand). Each parser
//! returns `Some(compacted)` only when it confidently parsed; `None` falls
//! through to the generic classifier. This keeps each parser isolated (one
//! file each) and makes the whole thing fail-open by construction.

mod cargo;
mod classifier;
mod git;
mod grep;
mod testrun;

/// Output below either bound is returned verbatim — never pay compaction cost
/// or risk info loss on already-small output.
const SIZE_GATE_LINES: usize = 40;
const SIZE_GATE_BYTES: usize = 8 * 1024;

/// Lines of raw tail always appended after a non-trivial compaction — exit
/// status / final error usually lands here, so this is the insurance against
/// a parser/classifier that missed it.
const GUARANTEED_TAIL_LINES: usize = 10;

/// Result of a compaction attempt: the (possibly unchanged) content plus the
/// byte accounting for savings telemetry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Compacted {
    pub content: String,
    pub raw_bytes: usize,
    pub compacted_bytes: usize,
}

impl Compacted {
    fn unchanged(raw: &str) -> Self {
        Self {
            content: raw.to_string(),
            raw_bytes: raw.len(),
            compacted_bytes: raw.len(),
        }
    }
}

/// Compact the output of `command` (the full Bash command string) given its
/// `raw` combined output and `exit_code`. Fail-open: returns `raw` unchanged
/// when small, unrecognised, or on any parser miss.
pub fn compact_bash(command: &str, raw: &str, exit_code: i32) -> Compacted {
    // Size gate: leave small output alone.
    if raw.len() <= SIZE_GATE_BYTES && raw.lines().count() <= SIZE_GATE_LINES {
        return Compacted::unchanged(raw);
    }

    let compacted_body = dispatch(command, raw, exit_code)
        .or_else(|| classifier::compact(raw))
        .map(|body| with_guaranteed_tail(&body, raw));

    match compacted_body {
        // Only accept the compaction if it actually shrank the output.
        Some(body) if body.len() < raw.len() => Compacted {
            raw_bytes: raw.len(),
            compacted_bytes: body.len(),
            content: body,
        },
        _ => Compacted::unchanged(raw),
    }
}

/// Route to a per-command parser by command prefix. `None` ⇒ no confident
/// parser ⇒ caller falls back to the classifier.
fn dispatch(command: &str, raw: &str, exit_code: i32) -> Option<String> {
    match program_and_sub(command) {
        ("cargo", _) => cargo::compact(raw, exit_code),
        ("git", _) => git::compact(raw, exit_code),
        ("grep", _) | ("rg", _) | ("find", _) => grep::compact(raw, exit_code),
        ("pytest", _) | ("jest", _) | ("vitest", _) => testrun::compact(raw, exit_code),
        ("go", Some("test")) => testrun::compact(raw, exit_code),
        ("python", _) | ("python3", _) | ("node", _) => testrun::compact(raw, exit_code),
        _ => None,
    }
}

/// Extract the leading program name and (optionally) its first subcommand,
/// stripping common wrappers/prefixes (`sudo`, `vx`, `pnpm`, `yarn`, `npx`,
/// env `K=V`). For a `&&`/`;` chain, classify the LAST segment (its output
/// dominates). Lowercased basename.
pub(crate) fn program_and_sub(command: &str) -> (&str, Option<&str>) {
    let segment = last_subcommand(command);

    let mut toks = segment.split_whitespace().filter(|t| {
        // Skip env assignments and known wrappers.
        !t.contains('=')
            && !matches!(
                *t,
                "sudo" | "vx" | "pnpm" | "yarn" | "npx" | "npm" | "command" | "time"
            )
    });
    let prog = toks.next().unwrap_or("");
    let prog = prog.rsplit(['/', '\\']).next().unwrap_or(prog);
    let sub = toks.next();
    (prog, sub)
}

/// Return the trimmed LAST sub-command of a shell chain, splitting ONLY on the
/// chain separators `;`, `&&`, and `||`. A bare `&` is deliberately NOT a
/// separator: it is part of redirection idioms like `2>&1` / `>&2`, so
/// splitting on it would mis-tokenize the program name (e.g. `cargo test 2>&1`
/// would otherwise classify as program `1` and defeat dispatch).
fn last_subcommand(command: &str) -> &str {
    let bytes = command.as_bytes();
    // Byte index just past the last chain separator; 0 means no separator.
    let mut last_sep_end = 0usize;
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b';' => {
                last_sep_end = i + 1;
                i += 1;
            }
            b'&' if bytes.get(i + 1) == Some(&b'&') => {
                last_sep_end = i + 2;
                i += 2;
            }
            b'|' if bytes.get(i + 1) == Some(&b'|') => {
                last_sep_end = i + 2;
                i += 2;
            }
            _ => i += 1,
        }
    }
    let segment = command[last_sep_end..].trim();
    if segment.is_empty() {
        command.trim()
    } else {
        segment
    }
}

/// Append the last `GUARANTEED_TAIL_LINES` raw lines after the compacted body
/// (deduped if the body already ends with them).
fn with_guaranteed_tail(body: &str, raw: &str) -> String {
    let tail: Vec<&str> = raw.lines().collect();
    let start = tail.len().saturating_sub(GUARANTEED_TAIL_LINES);
    let tail_block = tail[start..].join("\n");
    if body.trim_end().ends_with(tail_block.trim_end()) {
        return body.to_string();
    }
    format!("{body}\n--- last {GUARANTEED_TAIL_LINES} lines ---\n{tail_block}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_output_is_returned_verbatim() {
        let raw = "Exit code: 0\nSTDOUT:\nhello\nSTDERR:\n";
        let got = compact_bash("git status", raw, 0);
        assert_eq!(got.content, raw);
        assert_eq!(got.raw_bytes, got.compacted_bytes);
    }

    #[test]
    fn unrecognised_large_output_falls_back_not_errors() {
        let raw = "x\n".repeat(200);
        let got = compact_bash("some-weird-cmd --flag", &raw, 0);
        // Fail-open: never panics, never larger than raw.
        assert!(got.compacted_bytes <= got.raw_bytes);
    }

    #[test]
    fn program_and_sub_strips_wrappers_and_chains() {
        assert_eq!(program_and_sub("cargo test"), ("cargo", Some("test")));
        assert_eq!(
            program_and_sub("vx cargo nextest run"),
            ("cargo", Some("nextest"))
        );
        assert_eq!(
            program_and_sub("RUST_LOG=debug cargo build"),
            ("cargo", Some("build"))
        );
        assert_eq!(
            program_and_sub("cd /x && git status"),
            ("git", Some("status"))
        );
        assert_eq!(
            program_and_sub("/usr/bin/grep -r foo ."),
            ("grep", Some("-r"))
        );
    }

    #[test]
    fn program_and_sub_keeps_program_with_stderr_redirect() {
        // A bare `&` inside `2>&1` must NOT be treated as a chain separator —
        // otherwise the program tokenizes as `1` and dispatch never engages.
        assert_eq!(program_and_sub("cargo test 2>&1"), ("cargo", Some("test")));
        assert_eq!(
            program_and_sub("cargo build 2>&1 | rg foo"),
            ("cargo", Some("build"))
        );
        // Real chain operators still pick the last segment.
        assert_eq!(
            program_and_sub("cargo build 2>&1; git status"),
            ("git", Some("status"))
        );
        assert_eq!(
            program_and_sub("make 2>&1 || cargo check 2>&1"),
            ("cargo", Some("check"))
        );
    }

    #[test]
    fn stderr_redirected_cargo_dispatches_to_cargo_parser() {
        // Build a large cargo-shaped output so the size gate is cleared and the
        // cargo-aware parser (not the generic shape classifier) handles it.
        let noise = (0..100)
            .map(|i| format!("   Compiling crate{i} v0.1.0"))
            .collect::<Vec<_>>()
            .join("\n");
        let raw = format!(
            "{noise}\nerror[E0599]: no method named `foo` found\n \
             --> src/x.rs:10:20\n   |\n10 |     bar.foo();\n   |         ^^^ method not found\n\
             error: could not compile `x` due to previous error"
        );

        // The redirection idiom must route to the cargo parser, which keeps the
        // diagnostic block AND drops the `Compiling` spam — behavior the generic
        // classifier does not produce.
        let got = compact_bash("cargo test 2>&1", &raw, 1);
        assert!(
            got.content.contains("--> src/x.rs:10:20"),
            "cargo parser must keep the diagnostic location block"
        );
        assert!(
            !got.content.contains("Compiling crate50"),
            "cargo parser must drop compile spam (generic classifier would not)"
        );
        assert!(got.compacted_bytes < got.raw_bytes);
    }
}
