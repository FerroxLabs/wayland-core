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
//! - The `browser backend` and `binary version` rows always appear,
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
    // The `binary version` and `browser backend` rows run on every
    // platform, so they must appear in the report regardless of
    // whether the binary itself is found.
    let out = run_doctor();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("binary version"),
        "stdout missing 'binary version' row:\n{stdout}"
    );
    assert!(
        stdout.contains("browser backend"),
        "stdout missing 'browser backend' row:\n{stdout}"
    );
    // gh#491: the doctor must not send the operator after a backend this
    // binary did not compile.
    assert!(
        !stdout.contains("chromium"),
        "the doctor recommends Chromium, which is not compiled into this build:\n{stdout}"
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

/// `br-default` WIRING, through the real binary.
///
/// `doctor::with_config_rows` has unit coverage, but a unit test on it cannot
/// notice if nothing in `run()` calls it -- the browser-policy verdict would be
/// perfectly correct and never printed. This drives the shipped `--doctor`
/// command and reads the rendered table.
///
/// Deliberately verdict-agnostic: the row is WARN on a default install, PASS
/// once an operator opens the policy, and SKIP where the config does not
/// resolve. All three are honest; a MISSING row is the defect.
#[test]
fn doctor_prints_a_browser_policy_row_adjacent_to_the_backend_row() {
    let out = run_doctor();
    let stdout = String::from_utf8_lossy(&out.stdout);

    // Row lines are the ones the printer prefixes with `[PASS] `/`[WARN] `/...
    let rows: Vec<&str> = stdout.lines().filter(|l| l.starts_with('[')).collect();
    let backend = rows
        .iter()
        .position(|l| l.contains("browser backend"))
        .unwrap_or_else(|| {
            panic!(
                "no 'browser backend' row at all; the table is not what this test reads:\n{stdout}"
            )
        });
    let policy = rows
        .iter()
        .position(|l| l.contains("browser policy"))
        .unwrap_or_else(|| {
            panic!(
                "`--doctor` prints no 'browser policy' row. The doctor probes only whether a \
                 browser BINARY resolves, so on a host with the sidecar installed it reports \
                 `[PASS] browser backend` while `[browser.policy]` refuses every URL:\n{stdout}"
            )
        });
    assert_eq!(
        policy,
        backend + 1,
        "the policy row is not the row immediately after the backend row; a reader who stops \
         at `[PASS] browser backend` never reaches it:\n{stdout}"
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
fn doctor_prints_mcp_section_and_does_not_probe_by_default() {
    // A4b: bare `--doctor` must render the CLI-only MCP section AND, since
    // it is side-effect-free by default, print the `--probe-mcp` hint
    // instead of connect-testing anything. The presence of the hint (and
    // the absence of the "Probing ..." banner) proves the default path did
    // NOT spawn any stdio command or dial any URL.
    let out = run_doctor();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("MCP servers (declared):"),
        "stdout missing MCP section header:\n{stdout}"
    );
    assert!(
        stdout.contains("Run with --probe-mcp"),
        "bare --doctor must print the probe hint (proving it did not probe):\n{stdout}"
    );
    assert!(
        !stdout.contains("Probing config-declared MCP servers"),
        "bare --doctor must NOT connect-test (no probe banner expected):\n{stdout}"
    );
}

#[test]
fn doctor_marks_macos_accessibility_correctly_for_platform() {
    // On macOS the row must be ANSWERED; on every other platform it is
    // SKIPPED. Either way the label must appear.
    //
    // This used to assert `[MANUAL]` on macOS. That tag no longer exists on
    // this row: `check_macos_tcc` now runs a real probe and maps
    // Granted -> Pass, Denied -> Warn, NotApplicable -> Skip. So the old
    // assertion could not pass on macOS in ANY permission state, and it
    // failed 3/3 on `macos-latest` where the grant happens to be present.
    //
    // The replacement deliberately does NOT assert which of the two answers
    // comes back. That depends on the host's TCC database — the runner's
    // permission state, not the product's behaviour — and pinning it would
    // make this test green or red according to who is running it. What the
    // test is named for, and what must hold, is that macOS answers the row
    // instead of skipping it as a foreign platform.
    let out = run_doctor();
    let stdout = String::from_utf8_lossy(&out.stdout);

    let row = stdout
        .lines()
        .find(|line| line.contains("macOS Accessibility"))
        .unwrap_or_else(|| panic!("stdout missing 'macOS Accessibility' row:\n{stdout}"))
        .trim_start();

    if cfg!(target_os = "macos") {
        assert!(
            row.starts_with("[PASS]") || row.starts_with("[WARN]"),
            "macOS must answer the Accessibility row (granted -> [PASS], \
             denied -> [WARN]); got: {row}"
        );
    } else {
        assert!(
            row.starts_with("[SKIP]"),
            "non-macOS run should mark Accessibility as [SKIP]; got: {row}"
        );
    }
}
