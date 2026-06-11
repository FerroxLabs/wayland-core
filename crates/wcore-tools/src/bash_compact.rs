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
    let segment = command
        .rsplit(|c| c == ';' || c == '&')
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or(command.trim());

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
}
