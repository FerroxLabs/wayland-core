{
  "schema": "wayland-core.phase20-independent-review.v1",
  "source_sha": "10d75737a42b0d6b9aeaa42f1dea9fb06e5613c7",
  "source_tree": "a678cb30d0e8b96cb952fe21aed0118a184b9a4b",
  "source_executor_id": "wayland-f20-06-builder",
  "reviewer_id": "wayland-f20-09-independent-review",
  "checks": {
    "all_severity": "PASS",
    "candidate_seal_authority": "PASS",
    "interface_sufficiency": "PASS"
  },
  "deferred": [],
  "findings": {
    "blocker": 0,
    "critical": 0,
    "high": 0,
    "medium": 0,
    "low": 0
  },
  "evidence": [
    {
      "command": "bash .planning/scripts/verify-task-scope.sh <gsd-task-base-20-06> crates/wcore-sandbox/src/directory_authority_file.rs crates/wcore-swarm/Cargo.toml crates/wcore-swarm/src/worktree.rs crates/wcore-swarm/src/worktree/candidate.rs crates/wcore-swarm/src/worktree_tests.rs",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-06-swarm <wt> clippy -p wcore-sandbox -p wcore-swarm --all-targets --all-features -- -D warnings",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-06-swarm-t <wt> test -p wcore-swarm",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-06-swarm-t <wt> test -p wcore-sandbox",
      "exit_code": 0,
      "result": "PASS"
    }
  ],
  "disposition": "PASS"
}
