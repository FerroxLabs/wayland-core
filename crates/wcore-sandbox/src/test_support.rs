//! Containment-probe test support — the one place a containment, denial or
//! isolation assertion is allowed to be made from.
//!
//! # Why this module exists
//!
//! Every containment test in this repo has the same latent defect: it asserts
//! that a forbidden effect is **absent**, and absence is exactly what a
//! *completely broken sandbox* also produces. If `bwrap` fails to build the
//! mount namespace, or `sandbox-exec` rejects the profile, the child never
//! executes at all — no bytes are read, no file is written, no secret is
//! echoed — and every "must not be observable" assertion passes. The gate goes
//! green while the sandbox is dead.
//!
//! This was not a hypothesis. It was measured on this tree: with a single
//! `--ro-bind` onto an absent source injected into `BubblewrapBackend`, so that
//! `/usr/bin/echo` itself exits 1 with empty stdout,
//! `bwrap_confines_filesystem_writes_outside_allowlist` passed 6 runs out of 6.
//!
//! # The rule
//!
//! A denial observation is only evidence if the subject **ran**. So this module
//! refuses to hand back a result at all unless the child is proven to have
//! executed *inside the sandbox under test*, in the *same invocation* that
//! carried the probe. The proof is in-band: the probe script is wrapped so the
//! child must emit a begin marker before the probe body and an end marker after
//! it, carrying the probe body's own exit status. Both markers must arrive on
//! the child's stdout or [`run_contained_probe`] panics.
//!
//! That closes four distinct ways a containment test can go vacuously green:
//!
//! | failure                                    | what the markers do          |
//! |--------------------------------------------|------------------------------|
//! | sandbox setup aborts before exec           | neither marker arrives       |
//! | the shell is missing inside the namespace  | neither marker arrives       |
//! | the child is killed part-way through       | the END marker is missing    |
//! | the backend returns `Err` before spawning  | `run_contained_probe` panics |
//!
//! It deliberately does NOT try to prove that the sandbox is *correctly
//! configured* — only that the child ran. Proving the policy is the individual
//! test's job; this type just makes sure that job is not being done against a
//! corpse.
//!
//! # Use
//!
//! ```no_run
//! # use wcore_sandbox::test_support::run_contained_probe;
//! # async fn demo(backend: &dyn wcore_sandbox::backends::SandboxBackend,
//! #               manifest: &wcore_sandbox::SandboxManifest) {
//! let probe = run_contained_probe(backend, manifest, "/bin/sh", "cat /etc/shadow").await;
//! probe.assert_absent("root:", "host shadow hashes must not be readable");
//! # }
//! ```

use crate::backends::SandboxBackend;
use crate::{SandboxCommand, SandboxManifest};

/// Emitted by the wrapped child before the probe body runs.
pub const CHILD_BEGIN: &str = "WL-CONTAINMENT-CHILD-BEGIN";
/// Emitted by the wrapped child after the probe body runs, followed by `:<rc>`.
pub const CHILD_END: &str = "WL-CONTAINMENT-CHILD-END";

/// A containment probe whose child is PROVEN to have executed inside the
/// sandbox under test.
///
/// The only way to obtain one is [`run_contained_probe`], which panics rather
/// than return if the proof is missing. So every assertion method below rests
/// on a live child — an absent needle means the sandbox denied it, not that
/// nothing ran.
#[derive(Debug, Clone)]
pub struct ContainedProbe {
    /// The probe body's own stdout, with the liveness markers removed.
    probe_stdout: String,
    /// The child's stderr, verbatim (markers are never written to stderr).
    stderr: String,
    /// The exit status of the probe body itself — NOT of the marker wrapper.
    probe_exit_code: i32,
    /// The exit status the backend reported for the whole wrapped child.
    wrapper_exit_code: i32,
}

impl ContainedProbe {
    /// The probe body's stdout with the liveness markers stripped.
    pub fn stdout(&self) -> &str {
        &self.probe_stdout
    }

    /// The child's stderr, verbatim.
    pub fn stderr(&self) -> &str {
        &self.stderr
    }

    /// The exit status of the probe body itself.
    pub fn exit_code(&self) -> i32 {
        self.probe_exit_code
    }

    /// The exit status the backend reported for the wrapped child as a whole.
    pub fn wrapper_exit_code(&self) -> i32 {
        self.wrapper_exit_code
    }

    /// Assert a forbidden marker is absent from BOTH streams.
    ///
    /// Checking stderr as well as stdout is deliberate: a denied read that
    /// nonetheless echoes the secret into a diagnostic is still a leak.
    pub fn assert_absent(&self, needle: &str, why: &str) {
        assert!(
            !self.probe_stdout.contains(needle) && !self.stderr.contains(needle),
            "{why}: forbidden marker {needle:?} was observable from inside the sandbox \
             (the child provably ran; probe rc={}, stdout={:?}, stderr={:?})",
            self.probe_exit_code,
            self.probe_stdout,
            self.stderr,
        );
    }

    /// Assert an expected marker IS present on stdout.
    ///
    /// This is the no-over-deny direction: the sandbox must still permit what
    /// the policy allows.
    pub fn assert_present(&self, needle: &str, why: &str) {
        assert!(
            self.probe_stdout.contains(needle),
            "{why}: expected marker {needle:?} was NOT produced inside the sandbox \
             (the child ran but the allowed operation failed; probe rc={}, stdout={:?}, \
             stderr={:?})",
            self.probe_exit_code,
            self.probe_stdout,
            self.stderr,
        );
    }

    /// Assert the probe body itself failed — the shape of "the write was
    /// refused", as distinct from "the sandbox never started".
    ///
    /// Only meaningful because the child is proven to have run: a bare
    /// `assert_ne!(exit_code, 0)` on a raw backend result is satisfied by a
    /// sandbox that aborted before exec, which is precisely the vacuity this
    /// module exists to prevent.
    pub fn assert_probe_failed(&self, why: &str) {
        assert_ne!(
            self.probe_exit_code, 0,
            "{why}: the probe body succeeded inside the sandbox (rc=0); \
             stdout={:?} stderr={:?}",
            self.probe_stdout, self.stderr,
        );
    }

    /// Assert the probe body itself succeeded — the no-over-deny direction.
    pub fn assert_probe_succeeded(&self, why: &str) {
        assert_eq!(
            self.probe_exit_code, 0,
            "{why}: the probe body failed inside the sandbox; \
             stdout={:?} stderr={:?}",
            self.probe_stdout, self.stderr,
        );
    }
}

/// Wrap `script` so the child must prove it executed.
///
/// `{ ...; }` groups the body so a multi-statement probe still yields one
/// status, and `$?` is captured immediately so the END marker's own `printf`
/// cannot clobber it. Markers are newline-delimited on stdout only.
fn wrap(script: &str) -> String {
    format!(
        "printf '\\n{CHILD_BEGIN}\\n'; {{ {script}\n }}; __wl_rc=$?; \
         printf '\\n{CHILD_END}:%s\\n' \"$__wl_rc\""
    )
}

/// Extract the probe body's stdout and exit status, panicking unless the child
/// is proven to have executed.
///
/// Split out from [`run_contained_probe`] so the parse is unit-testable without
/// a live sandbox — see the tests at the foot of this file, which are the
/// negative controls for the helper itself.
fn parse(stdout: &str) -> Result<(String, i32), String> {
    let begin_tag = format!("\n{CHILD_BEGIN}\n");
    let Some(begin) = stdout.find(&begin_tag) else {
        return Err(format!(
            "the child never reached the probe: no {CHILD_BEGIN} marker on stdout. \
             The sandbox did not execute the command, so NOTHING about containment \
             was tested. Raw stdout: {stdout:?}"
        ));
    };
    let body_start = begin + begin_tag.len();

    let end_tag = format!("\n{CHILD_END}:");
    let Some(end) = stdout[body_start..].find(&end_tag) else {
        return Err(format!(
            "the child began the probe but never finished it: no {CHILD_END} marker \
             on stdout. The command was killed or the shell died part-way, so an \
             absent forbidden marker proves nothing. Raw stdout: {stdout:?}"
        ));
    };
    let body = stdout[body_start..body_start + end].to_owned();

    let rc_text = stdout[body_start + end + end_tag.len()..]
        .lines()
        .next()
        .unwrap_or("")
        .trim();
    let rc = rc_text.parse::<i32>().map_err(|err| {
        format!("the {CHILD_END} marker carried an unparseable status {rc_text:?}: {err}")
    })?;
    Ok((body, rc))
}

/// Run `script` under `shell` inside `backend`, and return a [`ContainedProbe`]
/// only if the child is PROVEN to have executed inside the sandbox.
///
/// Panics — never returns a degraded result — when:
///   * the backend refuses to spawn at all (`execute` returns `Err`);
///   * the child produced no begin marker (sandbox setup failed before exec);
///   * the child produced no end marker (it died part-way through the probe).
///
/// `shell` must be an absolute path: every backend scrubs `PATH`.
pub async fn run_contained_probe(
    backend: &dyn SandboxBackend,
    manifest: &SandboxManifest,
    shell: &str,
    script: &str,
) -> ContainedProbe {
    let out = backend
        .execute(
            manifest,
            SandboxCommand {
                argv: vec![shell.to_owned(), "-c".to_owned(), wrap(script)],
                cwd: None,
            },
        )
        .await
        .unwrap_or_else(|err| {
            panic!(
                "the sandbox backend refused to run the containment probe, so no \
                 containment property was tested: {err:?}"
            )
        });

    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let (probe_stdout, probe_exit_code) = parse(&stdout).unwrap_or_else(|why| {
        panic!(
            "CONTAINMENT PROBE IS VACUOUS — {why}\n  backend exit={}\n  stderr={stderr:?}",
            out.exit_code
        )
    });

    ContainedProbe {
        probe_stdout,
        stderr,
        probe_exit_code,
        wrapper_exit_code: out.exit_code,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The helper's own positive control: a well-formed child transcript parses
    /// into the probe body and its status.
    #[test]
    fn parse_recovers_body_and_status_from_a_live_child() {
        let stdout = format!("\n{CHILD_BEGIN}\nhello\n\n{CHILD_END}:7\n");
        let (body, rc) = parse(&stdout).expect("a complete transcript must parse");
        assert_eq!(body, "hello\n");
        assert_eq!(rc, 7);
    }

    /// The helper's own negative control #1 — a dead sandbox. This is the exact
    /// shape a broken `bwrap` produces (empty stdout), and it MUST be rejected.
    #[test]
    fn parse_rejects_a_child_that_never_ran() {
        let err = parse("").expect_err("empty stdout must never be read as a passing denial");
        assert!(err.contains("never reached the probe"), "{err}");
    }

    /// Negative control #2 — a child killed part-way. The begin marker arrives,
    /// the body is empty, and a naive reader would call that a clean denial.
    #[test]
    fn parse_rejects_a_child_that_died_mid_probe() {
        let stdout = format!("\n{CHILD_BEGIN}\n");
        let err = parse(&stdout).expect_err("a truncated transcript must not parse");
        assert!(err.contains("never finished it"), "{err}");
    }

    /// Negative control #3 — the markers are present but the status is garbage,
    /// which would otherwise silently become a default exit code.
    #[test]
    fn parse_rejects_an_unparseable_status() {
        let stdout = format!("\n{CHILD_BEGIN}\nx\n\n{CHILD_END}:not-a-number\n");
        let err = parse(&stdout).expect_err("a corrupt status must not parse");
        assert!(err.contains("unparseable status"), "{err}");
    }

    /// The wrapper must group the body so a multi-statement probe yields one
    /// status, and must capture `$?` before the END `printf` overwrites it.
    #[test]
    fn wrap_groups_the_body_and_captures_its_status_first() {
        let w = wrap("false; true");
        assert!(w.contains("{ false; true\n }; __wl_rc=$?"), "{w}");
        assert!(
            w.find("__wl_rc=$?").unwrap() < w.rfind("printf").unwrap(),
            "the status must be captured before the END printf: {w}"
        );
    }
}
