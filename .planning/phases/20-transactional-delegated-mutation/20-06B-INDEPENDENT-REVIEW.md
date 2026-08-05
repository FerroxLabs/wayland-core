{
  "schema": "wayland-core.phase20-independent-review.v1",
  "source_sha": "b1de890363ab82ba952ad03bb5e692461c1cc8b5",
  "source_tree": "8f3ef81889825ead9c26df1b453e871a89e14b34",
  "source_executor_id": "wayland-f20-10-builder",
  "reviewer_id": "wayland-f20-11-independent-review",
  "checks": {
    "all_severity": "PASS",
    "containment_authority": "PASS",
    "policy_sufficiency": "PASS"
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
      "command": "remote-cargo.sh f20-06-swarm <wt> clippy -p wcore-sandbox --all-targets --all-features -- -D warnings",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-06-swarm-t <wt> test -p wcore-sandbox",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "bash .planning/scripts/verify-task-scope.sh <gsd-task-base-20-11> crates/wcore-sandbox/src/lib.rs crates/wcore-sandbox/src/manifest.rs crates/wcore-sandbox/src/backends/mod.rs crates/wcore-sandbox/src/backends/appcontainer.rs crates/wcore-sandbox/src/backends/bwrap.rs crates/wcore-sandbox/src/backends/docker.rs crates/wcore-sandbox/src/backends/process_tree.rs",
      "exit_code": 0,
      "result": "PASS"
    }
  ],
  "disposition": "PASS"
}
