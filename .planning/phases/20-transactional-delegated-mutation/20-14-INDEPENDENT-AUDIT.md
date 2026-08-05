{
  "schema": "wayland-core.phase20-independent-review.v1",
  "source_sha": "ace4bd26fa3d831b2129ce319248652dbc25f5b7",
  "source_tree": "25c2c6c8b5d5d6eed7c33fc8e89c1c98619e2c5d",
  "source_executor_id": "wayland-f20-13-builder",
  "reviewer_id": "wayland-f20-14-independent-audit",
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
      "command": "git rev-parse ace4bd26fa3d831b2129ce319248652dbc25f5b7^{tree} == 25c2c6c8b5d5d6eed7c33fc8e89c1c98619e2c5d; 20-13-SUMMARY source_sha/source_tree consistent with the tree; HEAD 9d67fdd clean",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git rev-list --parents ace4bd2..9d67fdd: sole-parent, merge-free single metadata commit changing only .planning/STATE.md and 20-13-SUMMARY.md (summary changed exactly once)",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "node .planning/scripts/verify-review-result.mjs 8d66277 20-06A-INDEPENDENT-REVIEW.md 10d7573 a678cb30 f20-09",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "bash .planning/scripts/verify-review-pair.sh 10d7573 b188ab4 8d66277 20-06-SUMMARY.md 20-06A-INDEPENDENT-REVIEW.md <06A source paths>",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "node .planning/scripts/verify-review-result.mjs 5cd67c5 20-06B-INDEPENDENT-REVIEW.md b1de890 8f3ef818 f20-11",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "bash .planning/scripts/verify-review-pair.sh b1de890 542b917 5cd67c5 20-10-SUMMARY.md 20-06B-INDEPENDENT-REVIEW.md <06B sandbox paths>",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "bash .planning/scripts/verify-task-scope.sh --capture $(git rev-parse --git-path gsd-task-base-20-14) -> scope-base-captured commit=9d67fdd tree=c2042f5e",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "node .planning/scripts/task-base-authority.mjs capture gsd-review-base-20-14 9d67fdd c2042f5e; capture gsd-reviewed-source-20-14 ace4bd2 25c2c6c8 (both read back exact)",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "grep -rn windows_impl over 06A-06D audited blob set: only appcontainer.rs #[cfg(windows)] mod-declaration wrapper; real Win32 code in appcontainer/{command,handles,process}.rs is OUT of scope; in-scope non-Windows path is the fail-closed stub",
      "exit_code": 0,
      "result": "PASS"
    }
  ],
  "disposition": "PASS"
}
