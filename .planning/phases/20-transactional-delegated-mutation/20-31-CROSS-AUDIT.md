{
  "schema": "wayland-core.phase20-independent-review.v1",
  "source_sha": "17412cf2f6a8be9d2ec7272f6693f998db4ba2e5",
  "source_tree": "00e41519ac6782b05e610fcf7fafc772d5040a5d",
  "source_executor_id": "wayland-f20-native-repair-builder",
  "reviewer_id": "wayland-f20-31-crossaudit",
  "checks": {
    "all_severity": "PASS",
    "evidence_integrity": "PASS",
    "integration_authority": "PASS"
  },
  "deferred": ["native_macos", "native_windows"],
  "findings": {
    "blocker": 0,
    "critical": 0,
    "high": 0,
    "medium": 0,
    "low": 0
  },
  "evidence": [
    {
      "command": "git rev-parse 17412cf2f6a8be9d2ec7272f6693f998db4ba2e5^{tree} == 00e41519ac6782b05e610fcf7fafc772d5040a5d (exact sealed tree, verified); commit object present; 17412cf2 is an ancestor of HEAD 978478cb and git diff --name-only sealed..HEAD = exactly the two docs commits 20-29-SUMMARY.md + 20-30-SUMMARY.md (no buildable-surface drift since the seal)",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git diff --numstat 95c81ec6..17412cf2 -- crates/ .github/ scripts/ Cargo.lock Cargo.toml: functional delta is EXACTLY the 3 declared files — .github/workflows/nightly-windows-soak.yml (+17/-0), crates/wcore-sandbox/src/directory_authority.rs (+161/-33), crates/wcore-sandbox/src/directory_authority_file.rs (+33/-2); no Cargo.toml/Cargo.lock touched, no new [[package]] stanza, no stray production code (integration authority: the two RED-cause fixes and nothing else)",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git show 17412cf2:crates/wcore-sandbox/src/directory_authority.rs DirectoryAuthority::rename_into (T-20-31-01 critical isolation): #[cfg(unix)] derives source_name via retained_child_name(self.display_path()) as a validated single component used ONLY as a name, re-proves identity through destination_parent.open_child_directory(source_name) (openat AT the retained dirfd with O_NOFOLLOW|O_DIRECTORY) and rejects on identity_token() drift, then renameat_child(parent_fd, source_name, parent_fd, child_name, replace) resolves BOTH names only through destination_parent.handle.as_raw_fd() — never display_path / the ambient namespace; syncs the destination parent. #[cfg(not(any(unix,windows))) fails closed with PolicyNotSupported. Isolation boundary preserved, not merely compiled",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git show 17412cf2:crates/wcore-sandbox/src/directory_authority.rs renameat_no_replace + validate_child_name: replace=false routes to the OS no-replace primitive — Linux libc::renameat2(RENAME_NOREPLACE), Apple libc::renameatx_np(RENAME_EXCL), other-unix fails closed with PolicyNotSupported (never a silent clobbering fallback) — so replace=false cannot overwrite an existing target (fail-closed). validate_child_name requires exactly one Component::Normal, rejecting '.', '..', absolute, empty, and multi-component names for both source and destination; renameat_child rejects interior NUL via CString. No TOCTOU window beyond the pre-existing open_child + identity-token pattern used crate-wide",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git show 17412cf2:crates/wcore-sandbox/src/directory_authority.rs atomic_write_child: the former #[cfg(unix)] inline renameat(self.handle, temp, self.handle, name) publish is unified into #[cfg(any(unix,windows))] temporary_authority.rename_into(self, name, true) — replace=true keeps overwrite semantics byte-for-byte behaviour-preserving vs the historical inline renameat and matches the existing Windows publish branch; the failure path still removes the exact temporary and reports both publish and cleanup errors. The unix file rename_into is therefore genuinely used (not dead code under -D warnings)",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git show 17412cf2:crates/wcore-sandbox/src/directory_authority_file.rs RegularFileAuthority::rename_into (pub(super), unix): source_name via retained_child_name(self.display_path()); re-proves through target_parent.open_child_file(source_name) (openat AT the retained dirfd, O_NOFOLLOW) and rejects on identity drift via file_identity_changed; renameat_child(parent_fd, source_name, parent_fd, name, replace) resolves both names only through target_parent.handle — durability owned by the caller (atomic publish syncs the parent). Module wiring: directory_authority_file.rs is #[path]-included with `use super::*;`, so the private renameat_child/retained_child_name helpers and open_child_file/.identity/file_identity_changed all resolve (confirmed by the receipts below)",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git show 17412cf2:crates/wcore-sandbox/src/directory_authority.rs + directory_authority_file.rs: the Windows delegation path is UNCHANGED — #[cfg(windows)] on both authorities still calls windows::rename_directory_into / windows::rename_file_into verbatim (only wrapped inside the now-unified method body), preserving the replace=false fail-closed Windows semantics. DirectoryAuthority::rename_into is pub and API-reachable (test directory_authority_tests.rs:149 + windows tests); RegularFileAuthority::rename_into is pub(super) and reached by atomic_write_child. All three .rename_into( call sites (atomic_write_child, macOS acceptance test, two windows tests) are same-parent renames whose source lives under destination_parent; a cross-parent misuse fails closed at the open-under-destination re-proof",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git show 17412cf2:.github/workflows/nightly-windows-soak.yml (workflow guard scope): the `git config --global --add safe.directory '*'` step is added ONLY to the two candidate jobs f20-windows-candidate (pwsh, line 230) and f20-macos-candidate (bash, line 279) — both gated `if: github.event.inputs.f20_candidate == 'true'` — after actions/checkout@v4 and BEFORE the `git rev-parse EXPECTED_COMMIT^{tree}` resolve (lines 249/291). The non-candidate windows-2022 soak job (if: f20_candidate != 'true') does NOT receive the guard. Candidate gating, self-hosted runner labels, contents:read permissions, and no-secrets posture are all unchanged — the step only bypasses the intended dubious-ownership abort (accepted disposition T-20-29-03: single-tenant Sean-owned runners, contents:read, no secrets, no untrusted checkout). No security downgrade beyond the intended ownership-guard bypass",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "Evidence-integrity of the 20-30 seal receipts binding to the EXACT sealed SHA/tree: 20-30-SUMMARY records all three proofs run against a detached checkout of 17412cf2 so the remote-cargo land-gate harness verified the materialized tree == 00e41519 before cargo — locked --workspace --all-features build EXIT=0 with NO 'lock file needs to be updated' line (Cargo.lock blob 2b8c6cdf consistent, 1015 packages, byte-identical to the 20-23 seal), plain all-features build EXIT=0, aggregate nextest --profile ci run 961262a4-8133-471b-866b-43fbc5c662f0 = 11509 passed / 0 failed / 48 skipped (exact prior-seal baseline, honesty gate untripped). 20-29 closed the macOS E0599 via the sanctioned Mac `cargo check -p wcore-sandbox --features live-docker --tests` and Hetzner clippy -p wcore-sandbox --all-targets --all-features -D warnings = 0 warnings + nextest -p wcore-sandbox = 100 passed (added identity re-proof rejects nothing legitimate). Receipts bind to the sealed SHA; no Cargo run performed on the Mac by this cross-audit",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "Native deferral + authority captures: native_macos / native_windows classified DEFERRED to the 20-32 native-proof re-dispatch (decoy-planted macos_retained_parent_rename_delete_enumeration_and_cwd_stay_handle_relative runtime isolation + SEANDESKTOP dubious-ownership clearance are hardware facts, NOT passed from source here). bash verify-task-scope.sh --capture $(git rev-parse --git-path gsd-task-base-20-31) -> scope-base-captured commit=978478cb tree=4c263a0c generation=g-d28aa5e6…; node task-base-authority.mjs capture $(git rev-parse --git-path gsd-reviewed-source-20-31) 17412cf2 00e41519 -> read-back returns 17412cf2 / 00e41519 (reviewed-source authority binds to the sealed successor). Fresh non-author reviewer identity wayland-f20-31-crossaudit is distinct from the repair/seal source_executor_id wayland-f20-native-repair-builder; separation established",
      "exit_code": 0,
      "result": "PASS"
    }
  ],
  "disposition": "PASS"
}
