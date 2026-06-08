//! W5 (A.5): CLI integration smoke test for `wayland-core --doctor`.
//!
//! Asserts only what is platform-independent so the test passes on
//! every CI matrix entry regardless of which system binaries happen
//! to be present on the runner:
//!
//! - The doctor produces structured output (header + summary line).
//! - It exits with some deterministic code (we do not assert the
//!   value because that depends on whether the runner has `wlrctl`,
//!   `grim`, `chromium`, etc. installed — and `[FAIL]` rows are
//!   normal on a stock GitHub macOS / Linux runner).
//! - The `chromium browser` and `binary version` rows always appear,
//!   because those checks run on every platform.
//!
//! Rationale: the harness must be a smoke test, not a hermetic
//! fixture, because doctor *intentionally* probes the host system.
//! Spec-grade assertions (e.g. "FAIL when wlrctl is missing") are
//! covered by the unit tests inside `doctor/mod.rs::tests`.

use std::process::Command;

/// Run the compiled `wayland-core` binary with `--doctor` and return
/// the captured output. The harness sets `CARGO_BIN_EXE_wayland-core`
/// to the path of the freshly built test binary.
fn run_doctor() -> std::process::Output {
    let bin = env!("CARGO_BIN_EXE_wayland-core");
    Command::new(bin)
        .arg("--doctor")
        .output()
        .expect("spawn wayland-core --doctor")
}

#[test]
fn doctor_emits_header_and_summary() {
    let out = run_doctor();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("wayland-core doctor v"),
        "stdout missing header. full stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("Summary:"),
        "stdout missing summary footer. full stdout:\n{stdout}"
    );
}

#[test]
fn doctor_includes_universal_checks() {
    // The `binary version` and `chromium browser` rows run on every
    // platform, so they must appear in the report regardless of
    // whether the binary itself is found.
    let out = run_doctor();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("binary version"),
        "stdout missing 'binary version' row:\n{stdout}"
    );
    assert!(
        stdout.contains("chromium browser"),
        "stdout missing 'chromium browser' row:\n{stdout}"
    );
    // Optional providers always render (Pass or Warn — never absent).
    assert!(
        stdout.contains("BROWSERBASE_API_KEY"),
        "stdout missing 'BROWSERBASE_API_KEY' row:\n{stdout}"
    );
    assert!(
        stdout.contains("ollama"),
        "stdout missing 'ollama' row:\n{stdout}"
    );
}

#[test]
fn doctor_exit_code_is_deterministic() {
    // Don't assert WHICH code — that depends on whether the dev
    // machine has wlrctl/grim/chromium installed. Just assert the
    // process exited (didn't panic / crash with a signal) and that
    // the code is one of {0, 1} per the doctor contract.
    let out = run_doctor();
    let code = out.status.code();
    assert!(
        matches!(code, Some(0) | Some(1)),
        "expected doctor exit code in {{0, 1}}, got {code:?}. \
         stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn doctor_marks_macos_accessibility_correctly_for_platform() {
    // On macOS the row is rendered as MANUAL; on every other platform
    // it is SKIPPED. Either way the label must appear.
    let out = run_doctor();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("macOS Accessibility"),
        "stdout missing 'macOS Accessibility' row:\n{stdout}"
    );

    if cfg!(target_os = "macos") {
        assert!(
            stdout.contains("[MANUAL]"),
            "macOS run should mark Accessibility as [MANUAL]:\n{stdout}"
        );
    } else {
        assert!(
            stdout.contains("[SKIP] macOS Accessibility"),
            "non-macOS run should mark Accessibility as [SKIP]:\n{stdout}"
        );
    }
}
