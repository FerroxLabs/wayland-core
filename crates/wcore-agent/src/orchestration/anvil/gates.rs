//! Anvil gate machinery — closure pinning, the pre-climb executability probe,
//! injection fencing, and the flake-quarantine policy (A1.3 slice, spec §5).
//!
//! A Tier-1 gate is a REAL executable (repo tests / build / lint / typecheck /
//! schema / land-gate) — the only tier that can earn the reserved `verified`
//! stamp (spec §2). This module pins that gate so the climb runs against a
//! stable, hashed invocation closure and refuses up front if the gate cannot
//! even execute. It deliberately owns only the gate PRIMITIVES; the climb loop
//! that consumes them (probe → ensemble → surgical → escalate, fail-set
//! acceptance, receipt emission) lands in the climb slice (A1.5).
//!
//! ## What is pinned (spec §5 gate closure pinning)
//! The FULL invocation closure — command + argv + env allowlist + cwd + the
//! transitive inputs (config files, fixtures, helpers, goldens, conftest,
//! package scripts) — is hashed into a single `gate_closure_digest`. ANY closure
//! drift during the climb aborts the candidate ([`GateClosure::drifted`]). A
//! task whose deliverable IS a gate change carves the files-under-edit OUT of
//! the closure so authoring them is not mistaken for drift (`carve_out`).
//!
//! ## Untrusted by construction (spec §5)
//! Gates are untrusted code: the probe runs them in the exec sandbox with
//! network DENIED and a minimized env (no ambient secrets), and their raw
//! stdout/stderr never surfaces verbatim — only the typed, bounded,
//! control-stripped [`BoundedGateOutput`] may reach a caller that feeds a model
//! prompt (injection fencing).

use std::path::PathBuf;
use std::time::Duration;

use sha2::{Digest, Sha256};
use wcore_sandbox::backends::SandboxBackend;
use wcore_sandbox::{NetworkPolicy, SandboxCommand, SandboxError, SandboxManifest, SyscallPolicy};

/// Errors that prevent a gate closure from being pinned. A closure that cannot
/// be pinned cannot gate a climb — these fail the climb closed, never silently.
#[derive(Debug, thiserror::Error)]
pub enum GateError {
    /// A gate must have a command; an empty argv is a caller bug.
    #[error("gate closure has no argv (a Tier-1 gate must have a command)")]
    EmptyArgv,
    /// A declared transitive input could not be read at pin time. A closure that
    /// hashes a file it cannot read is not a closure — surface it.
    #[error("gate closure input unreadable: {path} ({source})")]
    InputUnreadable {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// The unpinned description of a Tier-1 gate invocation (spec §5 closure).
#[derive(Debug, Clone)]
pub struct GateSpec {
    /// `argv[0]` is the program; the rest are its arguments.
    pub argv: Vec<String>,
    /// Working directory the gate runs in.
    pub cwd: PathBuf,
    /// Names of environment variables whose values are allowed to survive into
    /// the sandbox. The NAMES are pinned (part of the closure); the values are
    /// resolved at probe time from a scrubbed env, so an ambient secret outside
    /// this list can never reach the gate.
    pub env_allowlist: Vec<String>,
    /// Transitive inputs whose CONTENT is pinned: config, fixtures, helpers,
    /// goldens, conftest, package scripts. Their bytes are hashed into the
    /// closure digest and re-checked for drift.
    pub inputs: Vec<PathBuf>,
}

/// A pinned gate closure: a [`GateSpec`] plus the content digest of its whole
/// invocation closure. `digest` is the `gate_closure_digest` the climb journal
/// (§6.5) and the `AnvilReceipt` event (§8) carry.
#[derive(Debug, Clone)]
pub struct GateClosure {
    spec: GateSpec,
    /// Content digest of each pinned input, parallel to `spec.inputs`.
    input_digests: Vec<[u8; 32]>,
    digest: [u8; 32],
}

impl GateClosure {
    /// Pin `spec`, excluding any `carve_out` path from the closure (the
    /// test-authoring carve-out, spec §5: a task that edits a gate file must not
    /// trip drift on its own deliverable). Reads + hashes every remaining
    /// transitive input; a missing/unreadable input fails closed.
    pub fn pin(mut spec: GateSpec, carve_out: &[PathBuf]) -> Result<Self, GateError> {
        if spec.argv.is_empty() {
            return Err(GateError::EmptyArgv);
        }
        spec.inputs.retain(|p| !carve_out.iter().any(|c| c == p));
        let mut input_digests = Vec::with_capacity(spec.inputs.len());
        for p in &spec.inputs {
            let bytes = std::fs::read(p).map_err(|e| GateError::InputUnreadable {
                path: p.clone(),
                source: e,
            })?;
            input_digests.push(sha256(&bytes));
        }
        let digest = closure_digest(&spec, &input_digests);
        Ok(Self {
            spec,
            input_digests,
            digest,
        })
    }

    /// The raw `gate_closure_digest`.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        self.digest
    }

    /// The `gate_closure_digest` as 64 lowercase hex chars (receipt/journal form).
    #[must_use]
    pub fn digest_hex(&self) -> String {
        hex32(&self.digest)
    }

    /// The pinned spec (read-only).
    #[must_use]
    pub fn spec(&self) -> &GateSpec {
        &self.spec
    }

    /// Whether any pinned input's content changed since pinning, or an input
    /// became unreadable. Spec §5: ANY closure drift aborts the candidate.
    #[must_use]
    pub fn drifted(&self) -> bool {
        for (path, pinned) in self.spec.inputs.iter().zip(&self.input_digests) {
            match std::fs::read(path) {
                Ok(bytes) => {
                    if sha256(&bytes) != *pinned {
                        return true;
                    }
                }
                // A vanished or unreadable pinned input is drift.
                Err(_) => return true,
            }
        }
        false
    }

    /// Run the pinned gate ONCE on the baseline before any builder spawns
    /// (spec §5 pre-climb probe), through `backend` with network denied and a
    /// minimized env. Returns whether the gate ran (and its baseline result) or
    /// could not execute here — in which case the climb must refuse, never
    /// burning ensemble budget to rediscover it.
    pub async fn probe_baseline(
        &self,
        backend: &dyn SandboxBackend,
        opts: &ProbeOpts,
    ) -> BaselineProbe {
        self.run_at(backend, opts, &self.spec.cwd).await
    }

    /// Run the pinned gate at an ARBITRARY working directory `cwd` (a candidate's
    /// isolated worktree, or the baseline), through `backend` with network denied
    /// and a minimized env — the same tested exec path as [`probe_baseline`], so
    /// the climb (A1.6) gates each candidate with identical sandbox discipline.
    /// The pinned argv + env allowlist are unchanged; only the cwd varies.
    pub async fn run_at(
        &self,
        backend: &dyn SandboxBackend,
        opts: &ProbeOpts,
        cwd: &std::path::Path,
    ) -> BaselineProbe {
        let manifest = SandboxManifest {
            network: NetworkPolicy::Deny,
            env: minimized_env(&self.spec.env_allowlist),
            syscall_policy: SyscallPolicy::Inherit,
            timeout: Some(opts.timeout),
            fs_read_allow: opts.fs_read_allow.clone(),
            fs_write_allow: opts.fs_write_allow.clone(),
            ..Default::default()
        };
        let cmd = SandboxCommand {
            argv: self.spec.argv.clone(),
            cwd: Some(cwd.to_path_buf()),
        };
        match backend.execute(&manifest, cmd).await {
            Ok(out) => BaselineProbe::Ran {
                exit_code: out.exit_code,
                clean: out.exit_code == 0,
                diagnostics: BoundedGateOutput::from_bytes(&out.stderr),
            },
            Err(e) => classify_exec_error(&e),
        }
    }
}

/// Sandbox knobs for the pre-climb probe. The fs allowlists point at the
/// ISOLATED gate snapshot (spec §5) — never the mutable worktree copy — so the
/// baseline gate cannot read or write outside the snapshot.
#[derive(Debug, Clone)]
pub struct ProbeOpts {
    /// Wall-clock budget for the baseline gate run.
    pub timeout: Duration,
    /// Read roots the gate is permitted (the snapshot + toolchain).
    pub fs_read_allow: Vec<PathBuf>,
    /// Write roots the gate is permitted (its own scratch inside the snapshot).
    pub fs_write_allow: Vec<PathBuf>,
}

/// Outcome of the pre-climb gate probe.
#[derive(Debug, Clone)]
pub enum BaselineProbe {
    /// The gate EXECUTED. `clean` is `exit_code == 0`: a clean baseline. A
    /// non-zero exit is a pre-existing failure set, surfaced (never silently
    /// pinned as if green) so a dirty/weakened suite is visible to the climb.
    Ran {
        exit_code: i32,
        clean: bool,
        diagnostics: BoundedGateOutput,
    },
    /// The gate could NOT execute here (no sandbox backend, spawn refused,
    /// missing toolchain, or the required isolation cannot be enforced). Spec §5:
    /// refuse the climb immediately.
    CannotExecute(String),
}

impl BaselineProbe {
    /// Whether the baseline is clean (the gate ran and passed). A climb may only
    /// pin a gate whose baseline is clean; otherwise the pre-existing failures
    /// are surfaced first.
    #[must_use]
    pub fn is_clean_baseline(&self) -> bool {
        matches!(self, BaselineProbe::Ran { clean: true, .. })
    }
}

/// Map a sandbox error from the probe to the "cannot execute → refuse" outcome.
/// EVERY sandbox error means the gate could not run as required (network-denied,
/// minimized env); per spec §5 that refuses the climb rather than proceeding
/// without the gate's isolation. Kept separate + pure so the decision is unit
/// tested without a live backend.
fn classify_exec_error(err: &SandboxError) -> BaselineProbe {
    let reason = match err {
        SandboxError::ExecFailed(m) => format!("gate could not be spawned: {m}"),
        SandboxError::Timeout => "gate probe exceeded its time budget".to_string(),
        SandboxError::PolicyNotSupported(m) => {
            format!("required sandbox isolation cannot be enforced here: {m}")
        }
        other => format!("gate could not execute under the required isolation: {other}"),
    };
    BaselineProbe::CannotExecute(reason)
}

/// Maximum bytes of gate output retained for diagnostics. Gate output is
/// untrusted; only a bounded tail is ever kept.
const MAX_DIAG_BYTES: usize = 2048;

/// A typed, bounded, control-stripped view of a gate's raw output — the ONLY
/// form of gate stdout/stderr allowed to reach a caller that may feed a model
/// prompt (injection fencing, spec §5). Raw bytes never leave the gate layer:
/// this keeps at most [`MAX_DIAG_BYTES`] of the TAIL, lossily decoded, with
/// control characters (other than newline/tab) replaced.
#[derive(Debug, Clone)]
pub struct BoundedGateOutput {
    tail: String,
    truncated: bool,
}

impl BoundedGateOutput {
    /// Build a bounded, sanitized view from raw gate output bytes.
    #[must_use]
    pub fn from_bytes(raw: &[u8]) -> Self {
        let truncated = raw.len() > MAX_DIAG_BYTES;
        let start = raw.len().saturating_sub(MAX_DIAG_BYTES);
        let mut tail = String::new();
        for ch in String::from_utf8_lossy(&raw[start..]).chars() {
            if ch == '\n' || ch == '\t' || !ch.is_control() {
                tail.push(ch);
            } else {
                // Strip other control chars (ANSI/escape/carriage-return noise)
                // so nothing steers a terminal or a prompt.
                tail.push('\u{FFFD}');
            }
        }
        Self { tail, truncated }
    }

    /// The sanitized, bounded diagnostic tail (safe to surface).
    #[must_use]
    pub fn tail(&self) -> &str {
        &self.tail
    }

    /// Whether the original output exceeded the cap and was truncated.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.truncated
    }
}

/// N-of-M stability required for the reserved `verified` stamp (spec §5 flake
/// quarantine): a check that flips on identical code is flaky and quarantined,
/// and `verified` requires `required`-of-`of` passing runs on the final
/// candidate. A1.3 defines the policy + predicates; the climb slice (A1.5)
/// drives the repeated gate runs that feed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StabilityPolicy {
    pub required: u32,
    pub of: u32,
}

impl StabilityPolicy {
    /// Build a policy, clamped so `1 <= required <= of`.
    #[must_use]
    pub fn new(required: u32, of: u32) -> Self {
        let of = of.max(1);
        Self {
            required: required.clamp(1, of),
            of,
        }
    }

    /// Whether `passes` passing runs (out of `self.of` identical-code runs) meets
    /// the stability bar.
    #[must_use]
    pub fn met(&self, passes: u32) -> bool {
        passes >= self.required
    }

    /// A check whose per-run pass/fail observations include BOTH a pass and a
    /// fail on identical code is flaky → quarantined out of the gate.
    #[must_use]
    pub fn is_flaky(observations: &[bool]) -> bool {
        observations.iter().any(|&b| b) && observations.iter().any(|&b| !b)
    }
}

impl Default for StabilityPolicy {
    /// 3-of-3: three identical-code runs must all pass to stamp `verified`.
    fn default() -> Self {
        Self { required: 3, of: 3 }
    }
}

/// SHA-256 of `bytes` (the codebase-wide content-hash primitive).
fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().into()
}

/// Lowercase-hex encode a 32-byte digest.
fn hex32(d: &[u8; 32]) -> String {
    let mut s = String::with_capacity(64);
    for b in d {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Minimized env for the sandboxed gate: the allowlisted NAMES resolved from the
/// current process env. The sandbox scrubs the host env `env -i` style, so only
/// these survive — no ambient `*_API_KEY`/token reaches the untrusted gate.
fn minimized_env(allowlist: &[String]) -> Vec<(String, String)> {
    allowlist
        .iter()
        .filter_map(|k| std::env::var(k).ok().map(|v| (k.clone(), v)))
        .collect()
}

/// Canonical, unambiguous SHA-256 over the whole invocation closure. Every field
/// is length-prefixed so no two distinct closures can collide by concatenation.
/// argv is ordered (order is semantic); the env allowlist and inputs are sorted
/// (they are sets), so an identical closure always yields an identical digest.
fn closure_digest(spec: &GateSpec, input_digests: &[[u8; 32]]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(b"anvil-gate-closure-v1");

    // argv — ordered.
    h.update((spec.argv.len() as u64).to_le_bytes());
    for a in &spec.argv {
        h.update((a.len() as u64).to_le_bytes());
        h.update(a.as_bytes());
    }

    // env allowlist — a set: sort + dedup for order-independence.
    let mut env = spec.env_allowlist.clone();
    env.sort();
    env.dedup();
    h.update((env.len() as u64).to_le_bytes());
    for e in &env {
        h.update((e.len() as u64).to_le_bytes());
        h.update(e.as_bytes());
    }

    // cwd.
    let cwd = spec.cwd.to_string_lossy();
    h.update((cwd.len() as u64).to_le_bytes());
    h.update(cwd.as_bytes());

    // inputs — a set of (path, content-digest): sort by path.
    let mut inputs: Vec<(&PathBuf, &[u8; 32])> =
        spec.inputs.iter().zip(input_digests.iter()).collect();
    inputs.sort_by(|a, b| a.0.cmp(b.0));
    h.update((inputs.len() as u64).to_le_bytes());
    for (path, d) in inputs {
        let p = path.to_string_lossy();
        h.update((p.len() as u64).to_le_bytes());
        h.update(p.as_bytes());
        h.update(&d[..]);
    }

    h.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn spec_in(dir: &std::path::Path, argv: &[&str], inputs: &[&str]) -> GateSpec {
        GateSpec {
            argv: argv.iter().map(|s| s.to_string()).collect(),
            cwd: dir.to_path_buf(),
            env_allowlist: vec!["PATH".to_string()],
            inputs: inputs.iter().map(|n| dir.join(n)).collect(),
        }
    }

    #[test]
    fn empty_argv_fails_closed() {
        let dir = TempDir::new().unwrap();
        let spec = spec_in(dir.path(), &[], &[]);
        assert!(matches!(
            GateClosure::pin(spec, &[]),
            Err(GateError::EmptyArgv)
        ));
    }

    #[test]
    fn missing_input_fails_closed() {
        let dir = TempDir::new().unwrap();
        let spec = spec_in(dir.path(), &["cargo", "test"], &["does-not-exist.toml"]);
        assert!(matches!(
            GateClosure::pin(spec, &[]),
            Err(GateError::InputUnreadable { .. })
        ));
    }

    #[test]
    fn digest_is_stable_and_hex_is_64_chars() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("fix.toml"), b"fixture-bytes").unwrap();
        let a =
            GateClosure::pin(spec_in(dir.path(), &["cargo", "test"], &["fix.toml"]), &[]).unwrap();
        let b =
            GateClosure::pin(spec_in(dir.path(), &["cargo", "test"], &["fix.toml"]), &[]).unwrap();
        assert_eq!(
            a.digest(),
            b.digest(),
            "identical closures hash identically"
        );
        assert_eq!(a.digest_hex().len(), 64);
        assert!(
            a.digest_hex()
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn argv_env_and_input_content_all_change_the_digest() {
        let dir = TempDir::new().unwrap();
        std::fs::write(dir.path().join("fix.toml"), b"v1").unwrap();
        let base =
            GateClosure::pin(spec_in(dir.path(), &["cargo", "test"], &["fix.toml"]), &[]).unwrap();

        // Different argv.
        let argv2 = GateClosure::pin(
            spec_in(dir.path(), &["cargo", "nextest"], &["fix.toml"]),
            &[],
        )
        .unwrap();
        assert_ne!(base.digest(), argv2.digest(), "argv must affect the digest");

        // Different env allowlist.
        let mut s = spec_in(dir.path(), &["cargo", "test"], &["fix.toml"]);
        s.env_allowlist.push("HOME".to_string());
        let env2 = GateClosure::pin(s, &[]).unwrap();
        assert_ne!(
            base.digest(),
            env2.digest(),
            "env allowlist must affect the digest"
        );

        // Different input content.
        std::fs::write(dir.path().join("fix.toml"), b"v2").unwrap();
        let content2 =
            GateClosure::pin(spec_in(dir.path(), &["cargo", "test"], &["fix.toml"]), &[]).unwrap();
        assert_ne!(
            base.digest(),
            content2.digest(),
            "input content must affect the digest"
        );
    }

    #[test]
    fn drift_detects_content_change_and_deletion_but_not_stability() {
        let dir = TempDir::new().unwrap();
        let fix = dir.path().join("fix.toml");
        std::fs::write(&fix, b"pinned").unwrap();
        let closure =
            GateClosure::pin(spec_in(dir.path(), &["cargo", "test"], &["fix.toml"]), &[]).unwrap();
        assert!(!closure.drifted(), "an unchanged closure has not drifted");

        std::fs::write(&fix, b"tampered").unwrap();
        assert!(closure.drifted(), "changed input content is drift");

        std::fs::write(&fix, b"pinned").unwrap();
        assert!(
            !closure.drifted(),
            "restoring the exact bytes clears the drift"
        );

        std::fs::remove_file(&fix).unwrap();
        assert!(closure.drifted(), "a vanished pinned input is drift");
    }

    #[test]
    fn carveout_excludes_edited_gate_files_from_the_closure() {
        let dir = TempDir::new().unwrap();
        let edited = dir.path().join("test_under_edit.rs");
        std::fs::write(&edited, b"original").unwrap();
        let closure = GateClosure::pin(
            spec_in(dir.path(), &["cargo", "test"], &["test_under_edit.rs"]),
            std::slice::from_ref(&edited),
        )
        .unwrap();
        // The carved-out file is not part of the closure, so editing it — the
        // task's own deliverable — is not drift.
        std::fs::write(&edited, b"the authored change").unwrap();
        assert!(
            !closure.drifted(),
            "editing a carved-out gate file must not trip closure drift"
        );
    }

    #[test]
    fn cannot_execute_maps_every_sandbox_error_to_refusal() {
        for e in [
            SandboxError::ExecFailed("no backend".into()),
            SandboxError::Timeout,
            SandboxError::PolicyNotSupported("network-deny".into()),
            SandboxError::NetworkDenied("blocked".into()),
        ] {
            assert!(
                matches!(classify_exec_error(&e), BaselineProbe::CannotExecute(_)),
                "{e:?} must refuse the climb"
            );
        }
    }

    #[test]
    fn baseline_clean_predicate() {
        let clean = BaselineProbe::Ran {
            exit_code: 0,
            clean: true,
            diagnostics: BoundedGateOutput::from_bytes(b""),
        };
        let dirty = BaselineProbe::Ran {
            exit_code: 1,
            clean: false,
            diagnostics: BoundedGateOutput::from_bytes(b"1 failed"),
        };
        assert!(clean.is_clean_baseline());
        assert!(
            !dirty.is_clean_baseline(),
            "a non-zero baseline is not clean"
        );
        assert!(!BaselineProbe::CannotExecute("x".into()).is_clean_baseline());
    }

    #[test]
    fn bounded_output_strips_control_chars_and_caps_length() {
        // Control chars (except \n and \t) are replaced.
        let out = BoundedGateOutput::from_bytes(b"ok\x1b[31mred\x07\nline\ttab");
        assert!(!out.truncated());
        assert!(out.tail().contains('\n') && out.tail().contains('\t'));
        assert!(
            !out.tail().contains('\x1b') && !out.tail().contains('\x07'),
            "escape/bell control chars must not survive: {:?}",
            out.tail()
        );

        // Oversized output is truncated to the TAIL and flagged.
        let big = vec![b'a'; MAX_DIAG_BYTES + 500];
        let out = BoundedGateOutput::from_bytes(&big);
        assert!(out.truncated());
        assert!(out.tail().len() <= MAX_DIAG_BYTES);
    }

    #[test]
    fn stability_policy_clamps_and_scores() {
        let p = StabilityPolicy::new(5, 3); // required clamped to of
        assert_eq!(p, StabilityPolicy { required: 3, of: 3 });
        assert!(p.met(3));
        assert!(!p.met(2));

        let z = StabilityPolicy::new(0, 0); // clamped up to 1-of-1
        assert_eq!(z, StabilityPolicy { required: 1, of: 1 });

        assert_eq!(
            StabilityPolicy::default(),
            StabilityPolicy { required: 3, of: 3 }
        );
    }

    #[test]
    fn flake_is_a_mixed_observation_set() {
        assert!(StabilityPolicy::is_flaky(&[true, false, true]));
        assert!(!StabilityPolicy::is_flaky(&[true, true, true]));
        assert!(!StabilityPolicy::is_flaky(&[false, false]));
        assert!(!StabilityPolicy::is_flaky(&[]));
    }

    #[test]
    fn minimized_env_only_surfaces_allowlisted_present_vars() {
        // SAFETY: single-threaded test; set a known var then read it back.
        unsafe {
            std::env::set_var("ANVIL_GATES_TEST_VAR", "present");
        }
        let env = minimized_env(&[
            "ANVIL_GATES_TEST_VAR".to_string(),
            "ANVIL_GATES_DEFINITELY_UNSET_VAR".to_string(),
        ]);
        assert!(
            env.iter()
                .any(|(k, v)| k == "ANVIL_GATES_TEST_VAR" && v == "present")
        );
        assert!(
            !env.iter()
                .any(|(k, _)| k == "ANVIL_GATES_DEFINITELY_UNSET_VAR"),
            "an unset allowlisted var must not appear"
        );
        unsafe {
            std::env::remove_var("ANVIL_GATES_TEST_VAR");
        }
    }
}
