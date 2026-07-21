{
  "schema": "wayland-core.phase20-independent-review.v1",
  "source_sha": "5e665ec5911fa2a118de70b498b8f0e2841d50ba",
  "source_tree": "e5dcf77c5e9bc0c426a4c68fed83ee06f221d4ba",
  "source_executor_id": "wayland-f20-08-builder",
  "reviewer_id": "wayland-f20-16-independent-review",
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
      "command": "/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-16-e2e /Users/seandonahoe/dev/waylandcore-ferrox test -p wcore-agent --test transactional_delegated_mutation_test",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-16-forge /Users/seandonahoe/dev/waylandcore-ferrox test -p wcore-agent --test anvil_forge_transaction",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "/Users/seandonahoe/.ratchet/harness/remote-cargo.sh f20-16-clippy /Users/seandonahoe/dev/waylandcore-ferrox clippy -p wcore-agent -p wcore-swarm --all-targets --all-features -- -D warnings",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "node --test scripts/f20-native-uat-proof.test.mjs",
      "exit_code": 0,
      "result": "PASS"
    }
  ],
  "disposition": "PASS"
}
