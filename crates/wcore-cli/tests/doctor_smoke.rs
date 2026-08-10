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

/// OBS-03, both directions, on a PATH the test owns.
///
/// `doctor_exit_code_is_deterministic` above accepts either code, which means
/// it cannot fail for the thing that matters: it would pass just as happily
/// against a doctor that always exits 0. This row decides the code instead of
/// observing it, by building a synthetic PATH that holds every binary the
/// Linux doctor requires and then removing exactly one.
///
/// Linux-only. macOS downgrades the browser row to `Warn` and has no
/// `wlrctl`/`grim` rows at all, so on macOS there is no required PATH binary
/// to remove — the arm would be untestable rather than merely skipped, and
/// pretending otherwise is how a green row that cannot fail gets shipped.
#[cfg(target_os = "linux")]
#[test]
fn doctor_fails_when_a_required_dependency_is_missing_and_names_it() {
    use std::os::unix::fs::PermissionsExt;

    let root = std::env::temp_dir().join(format!("wlc-doctor-path-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let bin_dir = root.join("bin");
    let home = root.join("home");
    std::fs::create_dir_all(&bin_dir).expect("bin dir");
    std::fs::create_dir_all(&home).expect("home");

    // `doctor` resolves everything through `which(1)`, so the real one has to
    // be reachable; the probed binaries are stubs, because the doctor only
    // asks whether they RESOLVE.
    let real_which = which_on_the_host("which");
    std::os::unix::fs::symlink(&real_which, bin_dir.join("which")).expect("link which");
    for prog in ["chromium", "wlrctl", "grim"] {
        let p = bin_dir.join(prog);
        std::fs::write(&p, "#!/bin/sh\nexit 0\n").expect("stub");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    let doctor = |bin_dir: &std::path::Path| -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_wayland-core"))
            .env_clear()
            .env("PATH", bin_dir)
            .env("HOME", &home)
            .env("WAYLAND_HOME", &home)
            .arg("--doctor")
            .output()
            .expect("spawn wayland-core --doctor")
    };

    // ARM 1 — control. Every required binary resolves, so the doctor must say
    // so. Without this arm, arm 2's non-zero could just be "this host is
    // missing something else entirely".
    let ok = doctor(&bin_dir);
    assert_eq!(
        ok.status.code(),
        Some(0),
        "control arm: with every required dependency on PATH the doctor must \
         exit 0, else arm 2 proves nothing.\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&ok.stdout),
        String::from_utf8_lossy(&ok.stderr)
    );

    // ARM 2 — remove exactly one required dependency.
    std::fs::remove_file(bin_dir.join("grim")).expect("remove grim");
    let missing = doctor(&bin_dir);
    let stdout = String::from_utf8_lossy(&missing.stdout).into_owned();
    let _ = std::fs::remove_dir_all(&root);

    assert_eq!(
        missing.status.code(),
        Some(1),
        "a missing REQUIRED dependency must exit 1:\n{stdout}"
    );
    assert!(
        stdout
            .lines()
            .any(|l| l.starts_with("[FAIL]") && l.contains("grim")),
        "the report must NAME the dependency that is missing — a bare \
         non-zero exit tells the user nothing actionable:\n{stdout}"
    );
}

/// Resolve a program on the REAL host PATH, for wiring into the synthetic one.
#[cfg(target_os = "linux")]
fn which_on_the_host(prog: &str) -> std::path::PathBuf {
    let out = Command::new("/usr/bin/env")
        .args(["which", prog])
        .output()
        .unwrap_or_else(|e| panic!("locate {prog}: {e}"));
    assert!(out.status.success(), "`which {prog}` failed on this host");
    std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_owned())
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
