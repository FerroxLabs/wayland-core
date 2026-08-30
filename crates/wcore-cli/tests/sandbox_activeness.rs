//! F-28-02-001 — the end-to-end containment gate for `wayland-core sandbox exec`.
//!
//! This test exists because the unit tests in `sandbox_cmd` can only assert how
//! the tool CONTEXT is built. They cannot tell you whether the child was
//! actually contained. This one drives the real shipped binary and asserts a
//! containment DIFFERENTIAL: the identical command, writing to the identical
//! path outside its workspace, is visible on the host when run uncontained and
//! is NOT visible on the host when run through `sandbox exec`.
//!
//! WHAT IT ASSERTS IS AGREEMENT, NOT CONFINEMENT. `sandbox status` publishes
//! `confines_filesystem`. This test performs the escape the field is about and
//! requires the OUTCOME to match the CLAIM, in both directions:
//!
//! * claim `true`  -> the escape file must NOT appear;
//! * claim `false` -> the escape file MUST appear (or the command must be
//!   refused outright), because a claim that understates what the OS enforces
//!   is drift too, and it is the direction that hides a claim quietly going
//!   stale.
//!
//! It is built this way because the earlier version keyed the differential off
//! `bypasses_containment`, which is SESSION AUTHORITY ("is this the operator's
//! Dangerous launch"), not a filesystem capability. It reads `false` on the
//! Windows `windows_job_object` default while a child writes wherever it likes,
//! so the surface advertised containment the backend could not provide. A
//! status field asserting containment has to be checked against a real escape
//! or the two drift apart in silence.
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
/// THE DIALECT IS READ FROM THE PRODUCT, NOT FROM THE PLATFORM, and that
/// distinction is the whole of FerroxLabs/wayland-core#387. `cmd` really does
/// need its own spelling — `touch` does not exist there and `;` is not a
/// command separator, so a single Unix spelling made cmd reject the whole line
/// and the differential was never taken on Windows at all. But a
/// `cfg!(windows)` arm answers a question nobody asked: since
/// FerroxLabs/wayland#1164 the shell tool resolves a real `bash.exe` wherever
/// the host has one, so on both CI Windows legs the cmd spelling was handed to
/// Git-for-Windows bash.
///
/// MEASURED, SeanDesktop, Windows 11 build 26200, binary at 70a47aaed, with
/// `sandbox exec` driving the identical escape twice into the user profile
/// directory (the same out-of-policy target `escape_target()` picks):
///
/// ```text
/// POSIX spelling  ->  child printed F28_CHILD_RAN, escape file LANDED
/// cmd   spelling  ->  child printed F28_CHILD_RAN, escape file DID NOT LAND
/// ```
///
/// `copy` is not a bash command and `>nul 2>nul` are bash file redirects, so
/// the write failed while `echo` still ran — the child looked alive, the
/// escape looked contained, and the test read that as `sandbox status`
/// understating what the backend enforces. It was not: the escape lands,
/// `confines_filesystem=false` is HONEST, and the broken instrument was this
/// probe. That is why the arm asserting the claim is not overstated must be
/// written in the dialect the child will actually be given.
fn probe(escape: &Path) -> String {
    if interpreter_is_posix() {
        // Git-for-Windows bash reads a drive path with forward slashes; the
        // display form's backslashes would be eaten as escapes.
        let escape = escape.display().to_string().replace('\\', "/");
        format!("touch '{escape}' 2>/dev/null; echo {RAN}")
    } else {
        let escape = escape.display();
        format!("copy /y nul \"{escape}\" >nul 2>nul & echo {RAN}")
    }
}

/// The interpreter `sandbox exec` will actually drive, read from the product.
///
/// `sandbox exec` runs THE agent shell tool, so its interpreter is whatever
/// `bash_shell_argv_prefix()` resolved — and since FerroxLabs/wayland#1164
/// that is a real `bash.exe` on any Windows host with Git for Windows, which
/// both CI Windows legs have. Asking the same function the subject asks is
/// what keeps the two arms of the differential in one dialect.
fn interpreter() -> Vec<String> {
    wcore_config::shell::bash_shell_argv_prefix()
}

fn interpreter_is_posix() -> bool {
    wcore_config::shell::shell_prefix_is_posix(&interpreter())
}

/// Run `probe` uncontained, from `cwd`, and return the child's output.
///
/// Spawned through the SAME argv prefix `sandbox exec` will use, so the
/// baseline and the product arm differ only in containment. Running the
/// baseline under a hard-coded `cmd` while the product ran bash is what let
/// the two arms disagree for a reason that had nothing to do with the sandbox.
///
/// Where that prefix IS `cmd`, the payload must reach it VERBATIM.
/// `Command::arg` applies `CommandLineToArgvW` quoting on top of it (an inner
/// `"` becomes `\"`), and cmd does not understand that escaping — the redirect
/// target arrives as literal backslash-quotes and the write fails before it
/// can demonstrate anything. The product's own sandboxed path already avoids
/// this (see `quote_cmd_payload` in `wcore-sandbox`). A POSIX shell needs the
/// opposite: `-c <line>` is one ordinary argv entry and `raw_arg` would
/// re-split it.
fn run_uncontained(probe: &str, cwd: &Path) -> std::process::Output {
    let prefix = interpreter();
    let (program, flags) = prefix
        .split_first()
        .expect("a shell prefix names a program");
    let mut command = Command::new(program);
    command.args(flags);
    if interpreter_is_posix() {
        // A POSIX shell takes `-c <line>` as one ordinary argv entry.
        command.arg(probe);
    } else {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.raw_arg(probe);
        }
        #[cfg(not(windows))]
        command.arg(probe);
    }
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

/// Formerly `sandbox_exec_confines_a_write_that_escapes_the_workspace`, when
/// it asserted confinement unconditionally off the wrong field.
#[test]
fn sandbox_status_filesystem_claim_matches_a_real_escape_attempt() {
    let workspace = tempfile::tempdir().expect("workspace");
    // The escape target lives outside every root the contained policy grants.
    let escape = escape_target();
    let _ = std::fs::remove_file(&escape);

    let status = status_json();
    let backend = status["backend"].as_str().unwrap_or("<none>").to_owned();
    let available = status["available"].as_bool().unwrap_or(false);
    let bypasses = status["bypasses_containment"].as_bool().unwrap_or(true);
    // No `unwrap_or`: an absent field is a status surface that stopped
    // reporting the claim, which must fail rather than default to something
    // comfortable.
    let confines = status["confines_filesystem"]
        .as_bool()
        .unwrap_or_else(|| panic!("`sandbox status` must report confines_filesystem: {status}"));

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

    // A backend that claims no filesystem confinement is allowed to refuse the
    // command outright (`fail_closed` does exactly that). Refusal enforces at
    // least as much as the claim, so it agrees with it.
    let ran = stdout.contains(RAN);
    if !confines && !ran {
        let _ = std::fs::remove_file(&escape);
        assert!(
            !out.status.success(),
            "backend `{backend}` claims confines_filesystem=false and the child \
             never ran, yet `sandbox exec` reported success — nothing here \
             evidences anything. stdout={stdout} stderr={stderr}"
        );
        return;
    }

    // The child must have RUN. Otherwise the escape check below proves nothing.
    assert!(
        ran,
        "backend `{backend}` reported available, but the child never ran, so \
         containment cannot be evidenced from this execution. \
         stdout={stdout} stderr={stderr}"
    );

    // THE DIFFERENTIAL. Same command, same path, proven-capable probe, child
    // proven to have run: what the host can see afterwards must be exactly what
    // `confines_filesystem` said it would be.
    //
    // Read the observation BEFORE cleaning up, so a panic cannot leave the
    // marker behind in the operator's home directory.
    let escaped = escape.exists();
    let _ = std::fs::remove_file(&escape);
    if confines {
        assert!(
            !escaped,
            "CONTAINMENT FAILURE on backend `{backend}`: `sandbox status` reports \
             confines_filesystem=true, but a child run through `sandbox exec` \
             wrote {} — outside every root its workspace policy grants. \
             stdout={stdout}",
            escape.display()
        );
    } else {
        assert!(
            escaped,
            "STALE CLAIM on backend `{backend}`: `sandbox status` reports \
             confines_filesystem=false, but the escape to {} did not land. \
             Either this backend now confines the filesystem and the claim \
             understates it, or this probe stopped being able to demonstrate \
             an escape — both make the reported posture untrustworthy. \
             stdout={stdout}",
            escape.display()
        );
    }
}

/// `bypasses_containment` and `confines_filesystem` are DIFFERENT questions,
/// and this asserts the surface keeps answering both. The defect this guards
/// against is a future edit deciding they are redundant and dropping one: on
/// the Windows default they disagree (`bypasses_containment=false` while
/// `confines_filesystem=false`), and collapsing them is how the surface starts
/// advertising containment again.
#[test]
fn status_reports_session_authority_and_filesystem_confinement_separately() {
    let status = status_json();
    for field in ["bypasses_containment", "confines_filesystem"] {
        assert!(
            status[field].is_boolean(),
            "`sandbox status` must report `{field}` as a boolean: {status}"
        );
    }
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
