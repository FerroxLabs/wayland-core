//! Smoke tests for the `--force` CLI flag (formerly `--yolo`).
//!
//! `--force` (aliases `--yolo`, `--dangerously-skip-permissions`) flips the
//! engine's session approval mode to `Force` at boot so every tool call is
//! auto-approved without prompting. These tests prove:
//!
//! 1. `--help` advertises `--force` as the canonical flag.
//! 2. All three flag forms (`--force`, `--yolo`, `--dangerously-skip-permissions`)
//!    are accepted by clap without error.
//! 3. The `--force` form works end-to-end (canonical name visible in help).

use std::process::Command;

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
