//! `--help` must not leak our sprint vocabulary at users.
//!
//! UAT-TUI-UNIX F5 found `F-089: model catalog commands`,
//! `v0.6.4 Task 2.4: …`, `F23-02 (Phase 23B)`, `W9.1 T4 (T11)`, `F24-B`,
//! `F25-03` and a paragraph beginning `23A-C1: RE-ADVERTISED because governed
//! promotion now exists.` in the help text of a public release. Re-measured on
//! the real binary at `e7bc6d88`: **26 of 215 `--help` lines** carried at least
//! one internal identifier, including 12 of the top-level subcommand
//! one-liners — the first screen a new user sees.
//!
//! Individually each is a leftover from the commit that added the flag. The
//! class recurs because nothing rejects it, which is what this test is for.
//!
//! ## Why this test can actually fail
//!
//! LANE-BRIEF §3.2 and §3b-iii: a gate is only worth running if it has a
//! reachable FAIL state *and* a reachable PASS state. Both are asserted here in
//! the same run, against the same matcher the real assertion uses:
//!
//! * `matcher_rejects_seeded_internal_ids` feeds the matcher the exact strings
//!   the UAT found. Every one must match. If the regex is ever broken — or
//!   commented out — this reddens, so the main test cannot silently become a
//!   tautology over an inert pattern.
//! * `matcher_accepts_ordinary_help_prose` feeds it real help sentences that
//!   must NOT match, so the fix cannot be "make the regex match nothing".
//! * `help_output_is_greppable_at_all` asserts `--help` produced real content,
//!   because an empty string contains zero internal identifiers and would
//!   otherwise pass the main test perfectly.

use regex::Regex;

/// Shapes that only occur in this project's internal planning vocabulary.
///
/// Deliberately narrow. Each alternative is anchored on a structure that a
/// normal English help sentence does not produce, and the accept-side test
/// below pins that claim to real help prose rather than to an opinion:
///
/// | pattern | catches | must not catch |
/// |---|---|---|
/// | `F-089`, `F-092` | dash-numbered finding ids | `F-1` (too short) |
/// | `F23-02`, `F24-B` | phase-dash ids | `UTF-8` (leading letter must be `F` + 2 digits) |
/// | `W9.1`, `W7-N`, `W5` | wave ids | — |
/// | `T4`, `T11` | task ids | — |
/// | `23A-C1`, `22-C3` | phase-criterion ids | — |
/// | `Phase 23B` | literal phase references | — |
/// | `M3.4`, `M5.2` | milestone ids | — |
/// | `Task 2.4` | release-task references | — |
/// | `A4b`, `A5` | appendix/task ids | — |
/// | `#111`, `#667` | bare issue references | `#1` (too short) |
///
/// `F23_SESSION=` / `F23_INDEX=` / `F23_CACHE=` are intentionally NOT matched:
/// those are tokens the product really prints to STDOUT, so documenting them is
/// correct. The underscore is what separates a live contract from a sprint id.
const INTERNAL_ID_PATTERN: &str = concat!(
    r"(\bF-[0-9]{3}\b",
    r"|\bF[0-9]{2}-[0-9A-Z]+\b",
    r"|\bW[0-9]+(\.[0-9]+)?(-[A-Z])?\b",
    r"|\bT[0-9]{1,2}\b",
    r"|\b[0-9]{2}[A-Z]?-C[0-9]+\b",
    r"|\bPhase [0-9]+[A-Z]?\b",
    r"|\bM[0-9]\.[0-9]\b",
    r"|Task [0-9]+\.[0-9]+",
    r"|\bA[0-9]+[a-z]?\b",
    r"|#[0-9]{2,4}\b)",
);

fn matcher() -> Regex {
    Regex::new(INTERNAL_ID_PATTERN).expect("internal-id pattern must compile")
}

fn help_text(arg: &str) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_wayland-core"))
        .arg(arg)
        .output()
        .expect("failed to spawn wayland-core");
    // clap writes long help to stdout; keep stderr too so a usage error cannot
    // hide as an empty stdout that trivially contains no identifiers.
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// Control 1 — the matcher must FIRE on every string the UAT actually found.
///
/// Without this the main assertion is unfalsifiable: a regex that matches
/// nothing reports a clean `--help` forever (LANE-BRIEF §3b-iii).
#[test]
fn matcher_rejects_seeded_internal_ids() {
    let re = matcher();
    let seeded = [
        "F-089: model catalog commands",
        "v0.6.4 Task 2.4: serve the engine's tool registry as an MCP server",
        "F23-02 (Phase 23B): operator verbs over saved sessions",
        "W9.1 T4 (T11): promote a P4 procedure",
        "23A-C1: RE-ADVERTISED because governed promotion now exists.",
        "F24-B: the persistent gateway runtime",
        "F25-03: nodes — pair / list / show",
        "M3.4: dump the memory state for a given session id",
        "M5.2: replay a session trace JSON file",
        "F-092 (W7-N): enable live online evolution",
        "W5 (A.5): run the system-dependency doctor",
        "A4b: when running --doctor, actually CONNECT-TEST",
        "W4 F19: run the skills audit",
        "#111 — the host's active assistant identity",
    ];
    for line in seeded {
        assert!(
            re.is_match(line),
            "matcher is DEAD: it failed to flag a known-internal string: {line:?}"
        );
    }
}

/// Control 2 — the matcher must NOT fire on ordinary user-facing help prose.
///
/// Prevents the degenerate "fix" of widening the pattern until the real test is
/// unpassable, which is the inverse defect and just as useless.
#[test]
fn matcher_accepts_ordinary_help_prose() {
    let re = matcher();
    let clean = [
        "Print config file path and exit",
        "Disable colored output",
        "Manage installed plugins (install / list / available / remove)",
        "Max output tokens per response",
        "Enable JSON streaming mode for host client integration",
        "Every operation prints a machine-readable `F23_SESSION=` token to STDOUT",
        "Archive, verify, restore and recover a Wayland home.",
        "Provider: \"anthropic\" or \"openai\"",
        "Resume a previous session",
        "Auto-approve all tool executions (skip confirmation)",
    ];
    for line in clean {
        assert!(
            !re.is_match(line),
            "matcher is OVER-BROAD: it flagged ordinary help prose: {line:?} \
             (matched {:?})",
            re.find(line).map(|m| m.as_str())
        );
    }
}

/// Control 3 — `--help` must have produced real content.
///
/// An empty string satisfies "contains no internal identifiers" perfectly. This
/// is the participant-alive assertion for the two tests below.
#[test]
fn help_output_is_greppable_at_all() {
    for arg in ["--help", "-h"] {
        let text = help_text(arg);
        assert!(
            text.lines().count() > 20,
            "{arg} produced only {} lines — the binary did not really print help",
            text.lines().count()
        );
        assert!(
            text.contains("Usage"),
            "{arg} output has no `Usage` section; this is not help text"
        );
    }
}

/// The assertion itself: long help carries no internal identifiers.
#[test]
fn long_help_has_no_internal_identifiers() {
    let re = matcher();
    let text = help_text("--help");
    let offenders: Vec<String> = text
        .lines()
        .filter(|line| re.is_match(line))
        .map(|line| {
            let hit = re.find(line).map(|m| m.as_str()).unwrap_or("");
            format!("  [{hit}] {}", line.trim())
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "`--help` leaks {} internal identifier(s) at users:\n{}\n\n\
         Describe what the thing DOES. Our sprint numbering is not a feature \
         description. (Machine-readable tokens the product really prints, like \
         `F23_SESSION=`, are fine and are not matched.)",
        offenders.len(),
        offenders.join("\n")
    );
}

/// Short help too — `-h` was 142 lines and carried 25 of the 26 offenders.
#[test]
fn short_help_has_no_internal_identifiers() {
    let re = matcher();
    let text = help_text("-h");
    let offenders: Vec<String> = text
        .lines()
        .filter(|line| re.is_match(line))
        .map(|line| line.trim().to_string())
        .collect();
    assert!(
        offenders.is_empty(),
        "`-h` leaks {} internal identifier(s) at users:\n{}",
        offenders.len(),
        offenders.join("\n")
    );
}
