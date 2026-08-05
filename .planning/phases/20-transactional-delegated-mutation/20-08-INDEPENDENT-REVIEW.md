{
  "schema": "wayland-core.phase20-independent-review.v1",
  "source_sha": "6937ef61aa2ad2074dd7875f9cde2369fc104461",
  "source_tree": "6db6fc859539b43f083aa0a22f3e3e0a014721ae",
  "source_executor_id": "wayland-f20-repair-executor",
  "reviewer_id": "wayland-f20-16-repair-review",
  "checks": {
    "all_severity": "PASS",
    "asvs_level_2": "PASS",
    "code_review": "PASS",
    "phase_validation": "PASS"
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
      "command": "/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-diag7 /Users/seandonahoe/dev/waylandcore-ferrox nextest run --workspace --profile ci --no-fail-fast (source 6937ef6: 11509 passed / 0 failed / 48 skipped; includes transactional_delegated_mutation_test 9/9 and anvil_forge_transaction 5/5)",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-16-clippy /Users/seandonahoe/dev/waylandcore-ferrox clippy -p wcore-agent -p wcore-swarm --all-targets --all-features -- -D warnings",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "node --test scripts/f20-native-uat-proof.test.mjs (source 6937ef6: 34/34)",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "git diff --quiet 5e665ec 6937ef6 -- crates/wcore-agent/src/orchestration/anvil/landing.rs crates/wcore-agent/src/orchestration/anvil/gate_authorization.rs crates/wcore-agent/src/orchestration/anvil/tool.rs crates/wcore-agent/src/orchestration/anvil/forge.rs crates/wcore-agent/src/engine.rs crates/wcore-agent/src/child_transaction.rs crates/wcore-agent/src/child_transaction/gate_executor.rs crates/wcore-agent/src/child_transaction/parent.rs crates/wcore-agent/src/spawner.rs crates/wcore-swarm/src/worktree_manager.rs crates/wcore-swarm/src/worktree.rs (core 20-08 files byte-identical to the prior CLEAN review at 5e665ec)",
      "exit_code": 0,
      "result": "PASS"
    }
  ],
  "disposition": "PASS"
}
