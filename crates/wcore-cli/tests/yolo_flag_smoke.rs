//! Smoke tests for the tier-1 approval-bypass flag and its aliases.
//!
//! Tier 1 is canonically `--dangerously-skip-permissions`, with `--force` and
//! `--yolo` as visible aliases of the SAME clap field. It flips the engine's
//! session approval mode to `Force` at boot so every tool call is
//! auto-approved without prompting — and it leaves the OS sandbox ON.
//!
//! Tier 2 is `--dangerously-skip-permissions-and-sandbox` (deprecated alias
//! `--dangerous`), which additionally bypasses the sandbox under a lease.
//! The tier boundary itself is asserted in the `danger_spellings_never_change_tier`
//! unit test; these tests cover the binary-level `--help` surface.

use std::process::Command;

/// `--help` text of the binary under test, asserted to have been produced by
/// a successful run so a crashed binary cannot masquerade as "flag absent".
fn help_text() -> String {
    let output = Command::new(binary())
        .arg("--help")
        .output()
        .expect("spawn wayland-core --help");
    assert!(
        output.status.success(),
        "--help should exit 0; got {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Path to the debug binary under test.
fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_wayland-core")
}

#[test]
fn help_advertises_the_force_flag_as_canonical() {
    // `--help` must mention `--force` as the canonical flag name.
    let output = Command::new(binary())
        .arg("--help")
        .output()
        .expect("spawn wayland-core --help");
    assert!(
        output.status.success(),
        "--help should exit 0; got {}",
        output.status
    );
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("--force"),
        "`--help` does not advertise `--force`:\n{help}"
    );
}

#[test]
fn help_still_advertises_the_yolo_alias() {
    // `--yolo` must remain visible in --help as a backward-compat alias.
    let output = Command::new(binary())
        .arg("--help")
        .output()
        .expect("spawn wayland-core --help");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("--yolo") || help.contains("yolo"),
        "`--help` does not advertise the `--yolo` backward-compat alias:\n{help}"
    );
}

#[test]
fn help_advertises_the_dangerously_skip_permissions_alias() {
    // The long-form safety alias must also be visible.
    let output = Command::new(binary())
        .arg("--help")
        .output()
        .expect("spawn wayland-core --help");
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(
        help.contains("--dangerously-skip-permissions"),
        "`--help` does not advertise the danger-alias `--dangerously-skip-permissions`:\n{help}"
    );
}

#[test]
fn clap_accepts_the_force_flag_without_error() {
    // `--force --help` must succeed — canonical flag name.
    let output = Command::new(binary())
        .arg("--force")
        .arg("--help")
        .output()
        .expect("spawn wayland-core --force --help");
    assert!(
        output.status.success(),
        "clap must accept --force; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn clap_accepts_the_yolo_alias() {
    // `--yolo` backward-compat alias must parse identically to `--force`.
    let output = Command::new(binary())
        .arg("--yolo")
        .arg("--help")
        .output()
        .expect("spawn wayland-core --yolo --help");
    assert!(
        output.status.success(),
        "clap must accept --yolo alias; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn clap_accepts_the_dangerously_skip_permissions_alias() {
    // The long form must parse the same way as `--force`.
    let output = Command::new(binary())
        .arg("--dangerously-skip-permissions")
        .arg("--help")
        .output()
        .expect("spawn wayland-core --dangerously-skip-permissions --help");
    assert!(
        output.status.success(),
        "clap must accept --dangerously-skip-permissions; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// The tier-2 canonical spelling must be advertised, and it must name the
/// superset relationship so an operator reading `--help` can see that tier 2
/// contains tier 1 rather than having to infer it.
#[test]
fn help_advertises_the_tier_two_canonical_spelling() {
    let help = help_text();
    assert!(
        help.contains("--dangerously-skip-permissions-and-sandbox"),
        "`--help` does not advertise the tier-2 canonical flag:\n{help}"
    );
}

/// `--dangerous` keeps working and `--help` says it is deprecated. Removing
/// the alias, or dropping the deprecation wording, both redden this.
#[test]
fn help_marks_the_dangerous_alias_deprecated_and_still_accepts_it() {
    let help = help_text();
    assert!(
        help.contains("--dangerous"),
        "`--help` must still advertise the `--dangerous` alias:\n{help}"
    );
    assert!(
        help.to_uppercase().contains("DEPRECATED"),
        "`--help` must mark the `--dangerous` spelling deprecated:\n{help}"
    );

    let output = Command::new(binary())
        .arg("--dangerous")
        .arg("--help")
        .output()
        .expect("spawn wayland-core --dangerous --help");
    assert!(
        output.status.success(),
        "the deprecated --dangerous alias must keep parsing; stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

/// `--help` must state that tier 1 leaves the sandbox on. This is the sentence
/// an operator relies on when choosing between the two flags, and it is the
/// one that would become a lie if a tier-1 alias were ever moved to tier 2.
#[test]
fn help_states_that_tier_one_retains_the_sandbox() {
    let help = help_text();
    let lower = help.to_lowercase();
    assert!(
        lower.contains("sandbox stays on") || lower.contains("sandbox remains"),
        "`--help` must state that tier 1 retains the OS sandbox:\n{help}"
    );
}

/// The two tiers must refuse to stack at the binary level too, not merely in
/// the in-process clap unit test.
///
/// NOTE ON THE HARNESS, because the obvious spelling of this test is broken:
/// do NOT append `--help`. clap handles `--help` BEFORE it validates
/// `conflicts_with`, so `--force --dangerously-skip-permissions-and-sandbox
/// --help` exits 0 and prints help — the test then passes for a conflicting
/// pair and would keep passing if the conflict were deleted outright. That is
/// exactly a gate that cannot fail. Measured on this binary, 2026-07-30.
///
/// `--list-agents` is used instead: it is validated like any other argument,
/// so a conflicting pair errors at parse, and on the pass side it exits
/// promptly rather than opening a session the test would have to time out.
#[test]
fn the_binary_refuses_to_stack_the_two_tiers() {
    let output = Command::new(binary())
        .arg("--force")
        .arg("--dangerously-skip-permissions-and-sandbox")
        .arg("--list-agents")
        .output()
        .expect("spawn wayland-core --force --dangerously-skip-permissions-and-sandbox");
    assert!(
        !output.status.success(),
        "stacking tier 1 and tier 2 must be refused; stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("cannot be used with"),
        "the refusal must be clap's conflict error, not an unrelated failure \
         (an unrelated crash would also make this test pass):\n{stderr}"
    );

    // CONTROL IN THE PASS DIRECTION. The same invocation with only ONE of the
    // two flags must succeed, proving the failure above is caused by the
    // conflict and not by `--list-agents` being broken.
    for solo in ["--force", "--dangerously-skip-permissions-and-sandbox"] {
        let ok = Command::new(binary())
            .arg(solo)
            .arg("--list-agents")
            .output()
            .expect("spawn wayland-core <solo flag> --list-agents");
        assert!(
            ok.status.success(),
            "{solo} --list-agents must succeed; stderr: {}",
            String::from_utf8_lossy(&ok.stderr)
        );
    }
}
