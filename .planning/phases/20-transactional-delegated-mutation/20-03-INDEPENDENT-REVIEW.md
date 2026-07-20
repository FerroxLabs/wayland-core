{
  "schema": "wayland-core.phase20-independent-review.v1",
  "source_sha": "d343fc720c38c05d0097821ff3117f88e12fa203",
  "source_tree": "7eca6e83107f1d7d1692f32ae17d6ed6ddf92135",
  "source_executor_id": "wayland-f20-03-builder",
  "reviewer_id": "wayland-f20-15-independent-review",
  "checks": {
    "all_severity": "PASS",
    "public_lifecycle": "PASS",
    "retained_authority": "PASS"
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
      "command": "bash .planning/scripts/verify-f20-03-scope.sh final",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-03-linux-sandbox <wt> nextest run --all-features -p wcore-sandbox --lib --no-tests=fail",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-03-linux-swarm-lib <wt> nextest run --all-features -p wcore-swarm --lib --no-tests=fail",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-03-linux-swarm-integrations <wt> nextest run --all-features -p wcore-swarm --test dispatch_smoke --test heartbeat_test --test swarm_worker_failure_reporting_e2e --test worker_runtime_limits --test workspace_authority --no-tests=fail",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-03-linux-bash-complete <wt> nextest run --all-features -p wcore-tools --test bash_sandbox_routing_test --no-tests=fail",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-03-linux-bwrap-enforcement <wt> nextest run --all-features -p wcore-sandbox --lib -E 'test(=backends::bwrap::tests::required_live_bwrap_retained_cwd_enforcement)'",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-03-linux-process-teardown <wt> nextest run --all-features -p wcore-sandbox --lib -E 'test(=backends::process_tree::linux_tests::required_live_descendant_teardown_before_workspace_cleanup)'",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-03-linux-docker <wt> nextest run --all-features -p wcore-sandbox --lib -E 'test(=backends::docker::tests::required_live_docker_admission_enforcement_and_teardown)'",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-03-linux-bash <wt> nextest run --all-features -p wcore-tools --test bash_sandbox_routing_test -E 'test(=delegated_mutation_required_live_sandbox_confines_parent_and_descendants)'",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-03-linux-clippy <wt> clippy -p wcore-sandbox -p wcore-swarm -p wcore-tools --all-targets --all-features -- -D warnings",
      "exit_code": 0,
      "result": "PASS"
    },
    {
      "command": "remote-cargo.sh f20-03-linux-fmt <wt> fmt --all -- --check",
      "exit_code": 0,
      "result": "PASS"
    }
  ],
  "disposition": "PASS"
}
