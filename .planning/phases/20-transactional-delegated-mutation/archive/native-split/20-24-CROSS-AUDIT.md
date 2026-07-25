{
  "schema": "wayland-core.phase20-independent-review.v1",
  "source_sha": "95c81ec6a351ec22125497333739fa7c93a0cd8b",
  "source_tree": "784f498002b9944856aedee6cb3db347b55c1dcc",
  "source_executor_id": "wayland-f20-native-repair-builder",
  "reviewer_id": "wayland-f20-24-crossaudit",
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
      "command": "git rev-parse 95c81ec6a351ec22125497333739fa7c93a0cd8b^{tree} == 784f498002b9944856aedee6cb3db347b55c1dcc; HEAD 43c42916 = sealed + one docs commit adding only 20-23-SUMMARY.md (git diff --name-only sealed..HEAD = that single file); working tree clean",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git diff --name-status be84bd2..95c81ec6: functional delta is exactly the 12 declared native-repair files (process.rs/storage.rs/snapshot.rs source, live_fs_acl.rs/dispatch_smoke.rs/hard_process_containment_windows.rs tests, f20-native-windows-proof.ps1/f20-native-macos-proof.sh/f20-native-uat-proof.mjs scripts, nightly-windows-soak.yml, verify-review-result.mjs, Cargo.lock); every other path is planning-doc-only — NO stray production code",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git grep -n WCORE_DIAG_TOKEN 95c81ec6 -- crates/: no matches — the boundary-breaking diagnostic (full-primary-token swap, DISABLE_MAX_PRIVILEGE neutered to 0, env-gated token-group dump) is absent from the sealed tree",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git show 95c81ec6:.../windows_impl/process.rs execute_blocking: CreateRestrictedToken(current_token, DISABLE_MAX_PRIVILEGE, 0, null, ...) drops only the deny-only SidsToDisable; CreateProcessAsUserW is invoked with restricted_token.as_raw() (NEVER current_token / full primary); explicit Low-IL label set on restricted_token plus a post-spawn OS-layer invariant that bails if the child is not Low IL; SECURITY_CAPABILITIES.AppContainerSid preserved; Job-Object KILL_ON_JOB_CLOSE + ActiveProcessLimit + breakaway-deny intact — the deny-only SID drop did not widen the AppContainer isolation boundary (critical EoP surface CLEAN)",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git show 95c81ec6:.../tests/live_fs_acl.rs: normal_sid_only_grant_is_denied (grants file to Everyone/S-1-1-0 with NO package-SID grant, asserts child STILL denied — exit != 0, no MARKER) and deny_ace_still_blocks_granted_read (DENY-before-ALLOW) are genuine falsifiable isolation proofs, not stubs; granted/revoked case strengthened to present-during + absent-after; NATIVE_ACCEPTANCE_CASES=11 with a zero-execution guard keeps the count honest",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git show 95c81ec6:.../tests/hard_process_containment_windows.rs: 5 #![cfg(windows)]/#[ignore] cases drive the ACTUAL Job Object via host-side CIM/tasklist liveness queries (KILL_ON_JOB_CLOSE no-residue observed running mid-flight then GONE, active-process cap fan-out bound, breakaway denial, black-box preflight through the public SandboxBackend trait) — real-mechanism containment assertions, not parent-exit tautologies",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git show 95c81ec6:scripts/f20-native-windows-proof.ps1 Assert-TargetOsGate + f20-native-macos-proof.sh assert_target_os_gate: each OS-specific target's selected test source must be affirmatively cfg-gated for its own OS (positive, load-bearing) AND carry no foreign-OS cfg (negative); a native target re-pointed at the Linux-only Bubblewrap test fails closed; ps1 also asserts expectedCommit/expectedTree before cargo, so a proof can only run against the exact sealed tree — harness cannot map a native target to a wrong-OS test",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "20-22 macOS re-validation: all 8 f20-native-macos-proof.sh targets resolve to existing tests; the two macOS-specific targets resolve to #![cfg(target_os=macos)] sources (live_integrity_macos.rs, hard_process_containment_macos.rs) selected filename-independently by defining function; no aspirational/wrong-OS mapping remains; verify-review-result.mjs f20-native-crossaudit = {all_severity, evidence_integrity, integration_authority} PASS with deferred [native_macos, native_windows], f20-native-16 leaves native NON-deferred, existing f20-09/11/14/15/16 unchanged — writer/verifier parity and self-hosted msvc runner pin sound",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "Cargo.lock receipt binding: git diff be84bd2..95c81ec6 touches Cargo.lock only among manifests (+7/-0, 0 new [[package]] stanzas per 20-23 numstat); 20-23 remote-cargo land-gate harness materialized the committed tree in a clean slot and verified extracted tree == committed HEAD tree 784f4980 before cargo, then --workspace --all-features --locked build EXIT=0 (lock consistent, no 'needs updating') and nextest --profile ci = 11509 passed / 0 failed / 48 skipped (run 1ab1110b) — the aggregate receipt binds to the EXACT sealed SHA (independently confirmed 95c81ec6^{tree} == 784f4980)",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "bash .planning/scripts/verify-task-scope.sh --capture $(git rev-parse --git-path gsd-task-base-20-24) -> scope-base-captured commit=43c42916 tree=6540d32d; node task-base-authority.mjs capture $(git rev-parse --git-path gsd-reviewed-source-20-24) 95c81ec6 784f4980 -> read-back returns 95c81ec6 / 784f4980 (reviewed-source authority binds to the sealed candidate)",
      "exit_code": 0,
      "result": "PASS"
    }
  ],
  "disposition": "PASS"
}
