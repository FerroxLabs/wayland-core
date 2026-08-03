//! F-28-02-001 — the end-to-end containment gate for `wayland-core sandbox exec`.
//!
//! This test exists because the unit tests in `sandbox_cmd` can only assert how
//! the tool CONTEXT is built. They cannot tell you whether the child was
//! actually contained. This one drives the real shipped binary and asserts a
//! containment DIFFERENTIAL: the identical command, writing to the identical
//! path outside its workspace, is visible on the host when run uncontained and
//! is NOT visible on the host when run through `sandbox exec`.
//!
//! TWO TRAPS THIS TEST IS BUILT AROUND, both of which would make it pass
//! vacuously:
//!
//! 1. **Absence of the escape file is not evidence of containment.** If the
//!    sandbox refused to launch, or the shell died before `touch` ran, the file
//!    would also be absent and a naive assertion would report a green over a
//!    child that never executed. So the probe also prints a marker and the test
//!    REQUIRES that marker in the child's own stdout. Absence of the escape
//!    file only counts once the child is proven to have run.
//!
//! 2. **A host with no containment backend must not silently pass.** There is
//!    no skip branch here. A host whose backend is unavailable or bypassing
//!    takes the other assert: `sandbox exec` MUST refuse. Every host asserts
//!    something.

use std::path::Path;
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_wayland-core");

/// The child prints this on stdout. Its presence proves the child ran, which is
/// what makes the absence of the escape file meaningful.
const RAN: &str = "F28_CHILD_RAN";

fn status_json() -> serde_json::Value {
    let out = Command::new(BIN)
        .args(["sandbox", "status", "--json"])
        .output()
        .expect("run `sandbox status --json`");
    assert!(
        out.status.success(),
        "`sandbox status` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("status emits a JSON object")
}

/// Create `<escape>`, ignore any failure, then print `<RAN>` — one shell
/// command, identical inside and out.
///
/// The Windows spelling is not a translation for tidiness: `touch` does not
/// exist under `cmd.exe` and `;` is not a command separator there, so the
/// previous single Unix spelling made cmd reject the whole line. The child
/// printed nothing, the baseline arm failed on "the uncontained baseline did
/// not run", and `sandbox exec` was never reached — the containment
/// differential was never taken on Windows at all.
fn probe(escape: &Path) -> String {
    let escape = escape.display();
    #[cfg(windows)]
    let script = format!("copy /y nul \"{escape}\" >nul 2>nul & echo {RAN}");
    #[cfg(not(windows))]
    let script = format!("touch {escape} 2>/dev/null; echo {RAN}");
    script
}

/// Run `probe` uncontained, from `cwd`, and return the child's output.
///
/// On Windows the payload must reach `cmd.exe` VERBATIM. `Command::arg`
/// applies `CommandLineToArgvW` quoting on top of it (an inner `"` becomes
/// `\"`), and cmd does not understand that escaping — the redirect target
/// arrives as literal backslash-quotes and the write fails before it can
/// demonstrate anything. The product's own sandboxed path already avoids this
/// (see `quote_cmd_payload` in `wcore-sandbox`), so handing the payload over
/// raw here is what makes both arms of the differential execute the identical
/// command text.
fn run_uncontained(probe: &str, cwd: &Path) -> std::process::Output {
    let mut command = Command::new(shell());
    command.arg(shell_flag());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.raw_arg(probe);
    }
    #[cfg(not(windows))]
    command.arg(probe);
    command
        .current_dir(cwd)
        .output()
        .expect("run the uncontained baseline")
}

/// Pick a path the contained workspace policy does NOT grant.
///
/// This is load-bearing and cost one red to learn: the first version of this
/// test wrote into a `tempfile::tempdir()`, and `WorkspacePolicy::contained`
/// deliberately grants the whole of `std::env::temp_dir()` as a scratch root.
/// The child wrote there legitimately and the test read it as a containment
/// failure. The home directory is granted by neither the `contained` writable
/// set (workspace + temp) nor the macOS profile's read allowlist (`/usr`,
/// `/System`, `/Library`, `/bin`, `/sbin`), so a write landing there really is
/// an escape. The same holds on Windows: the user profile is outside the
/// AppContainer profile's granted ACLs and is a medium-integrity object, which
/// a Low-IL contained child cannot write regardless of ACL.
fn escape_target() -> std::path::PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .expect("HOME or USERPROFILE must be set to site an out-of-policy path");
    let home = std::path::PathBuf::from(home);
    let temp = std::fs::canonicalize(std::env::temp_dir()).unwrap_or_else(|_| std::env::temp_dir());
    assert!(
        !home.starts_with(&temp),
        "HOME ({}) is inside the temp scratch root ({}), which the contained \
         policy grants; this test cannot site an out-of-policy path here",
        home.display(),
        temp.display()
    );
    // Unique per run: a stale marker from an earlier run would otherwise be
    // indistinguishable from a fresh escape.
    home.join(format!("f28-escape-marker-{}", std::process::id()))
}

#[test]
fn sandbox_exec_confines_a_write_that_escapes_the_workspace() {
    let workspace = tempfile::tempdir().expect("workspace");
    // The escape target lives outside every root the contained policy grants.
    let escape = escape_target();
    let _ = std::fs::remove_file(&escape);

    let status = status_json();
    let backend = status["backend"].as_str().unwrap_or("<none>").to_owned();
    let available = status["available"].as_bool().unwrap_or(false);
    let bypasses = status["bypasses_containment"].as_bool().unwrap_or(true);

    // ---------------------------------------------------------------
    // Baseline: the SAME command, uncontained. Proves the probe is capable
    // of producing the violation at all — without this, "no escape file"
    // could just mean the command never could have written there.
    // ---------------------------------------------------------------
    let baseline = run_uncontained(&probe(&escape), workspace.path());
    let baseline_out = String::from_utf8_lossy(&baseline.stdout).into_owned();
    assert!(
        baseline_out.contains(RAN),
        "the uncontained baseline did not run: {baseline_out}"
    );
    assert!(
        escape.exists(),
        "the uncontained baseline did not produce the escape file at {}; \
         the probe cannot detect a containment failure and every later \
         assertion would be vacuous",
        escape.display()
    );
    std::fs::remove_file(&escape).expect("clear the baseline escape file");

    // ---------------------------------------------------------------
    // The product. Either it contains, or — on a host with no real backend —
    // it refuses. There is no third branch.
    // ---------------------------------------------------------------
    let out = Command::new(BIN)
        .args([
            "sandbox",
            "exec",
            "--workspace",
            &workspace.path().to_string_lossy(),
            &probe(&escape),
        ])
        .output()
        .expect("run `sandbox exec`");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();

    if !available || bypasses {
        let _ = std::fs::remove_file(&escape);
        assert!(
            !out.status.success(),
            "backend `{backend}` reports available={available} \
             bypasses_containment={bypasses}, so `sandbox exec` must refuse \
             rather than run the child uncontained. stdout={stdout} stderr={stderr}"
        );
        return;
    }

    // The child must have RUN. Otherwise the escape check below proves nothing.
    assert!(
        stdout.contains(RAN),
        "backend `{backend}` reported available, but the child never ran, so \
         containment cannot be evidenced from this execution. \
         stdout={stdout} stderr={stderr}"
    );

    // THE DIFFERENTIAL. Same command, same path, proven-capable probe, child
    // proven to have run: the write is visible on the host uncontained and
    // must NOT be visible on the host through the sandbox.
    //
    // Read the observation BEFORE cleaning up, so a panic cannot leave the
    // marker behind in the operator's home directory.
    let escaped = escape.exists();
    let _ = std::fs::remove_file(&escape);
    assert!(
        !escaped,
        "CONTAINMENT FAILURE on backend `{backend}`: a child run through \
         `sandbox exec` wrote {} — outside every root its workspace policy \
         grants. stdout={stdout}",
        escape.display()
    );
}

/// `sandbox status` reports a containment-required runtime. A runtime that
/// bypasses containment can never be what this verb selects.
#[test]
fn sandbox_status_never_reports_a_containment_bypass() {
    let status = status_json();
    assert_eq!(
        status["bypasses_containment"],
        serde_json::json!(false),
        "`sandbox status` selected a containment-bypassing runtime: {status}"
    );
    assert!(
        status["backend"].as_str().is_some_and(|b| b != "none"),
        "`sandbox status` must name the backend it selected: {status}"
    );
}

fn shell() -> &'static str {
    if cfg!(windows) { "cmd" } else { "/bin/sh" }
}

fn shell_flag() -> &'static str {
    if cfg!(windows) { "/C" } else { "-c" }
}
