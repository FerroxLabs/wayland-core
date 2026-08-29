---
issue: 340
repo: FerroxLabs/wayland-core
kind: defect
title: "The MCP malware gate does not cover every launch, and its doc claims it does"
status: open
last_verified_commit: 43848f75
criteria:
  - id: c1
    text: "The malware gate's doc comment states the coverage the gate actually has, rather than asserting every stdio launch is checked before execution"
    state: met
    evidence: "file:crates/wcore-mcp/src/malware_gate.rs:23"
    owner: core
    note: "The 'every stdio launch is checked before execution' claim is gone. malware_gate.rs:23-69 is an explicit covered / NOT covered / fails-open section naming the five uncovered shapes. Q2 in .planning/DECISIONS.md is the decision behind it: do NOT try to detect indirect runners by spelling, say so in the doc instead."
  - id: c2
    text: "An indirect runner shape such as a shell command wrapping a registry package does not reach exec unchecked"
    state: met
    evidence: "test:crates/wcore-mcp/tests/mcp_launch_malware_gate.rs::a_shell_wrapped_runner_is_refused_before_exec"
    owner: core
    note: "Drives the real launch path for sh -c 'npx evil-pkg', sh -c 'cd /tmp && npx -y evil-pkg' and sh -c 'exec pipx run evil-pkg', asserting Err(MalwareBlocked) AND !launch.executed. Negative control ordinary_launches_are_still_neither_queried_nor_blocked keeps it from being a gate that refuses everything."
  - id: c3
    text: "The fail-open on an unreachable OSV endpoint is an explicit operator choice, and where it stays open it is visible to the user"
    state: superseded
    owner: core
    note: "HALF SHIPPED, and the remainder is wayland-core#354 which is open. The VISIBILITY half is here and graded: osv_check.rs:754 logs the fail-open at tracing::error! and fail_open_is_visible_at_default_log_levels asserts on the recorded LEVEL so a downgrade to warn! reddens it. The OPERATOR-CHOICE half is not: there is no strict/permissive setting anywhere in wcore-config, wcore-mcp or osv_check. Q3 in .planning/DECISIONS.md rules that the real answer is a typed protocol frame, not a log level, and that frame lands with Q4."
  - id: c4
    text: "The wayland-ijfw npx reachability probe is confirmed to run after the gate, or is moved behind it"
    state: met
    evidence: "test:crates/wayland-ijfw/src/mcp.rs::a_package_runner_spec_is_never_spawned_by_the_probe"
    owner: core
    note: "MOVED BEHIND, not reordered - mcp.rs:283 returns true for a package-runner command without spawning at all, so the probe no longer executes package-runner argv. Stage 1 (npx --version, names no package) still runs. Negative control a_non_runner_command_that_cannot_start_is_still_rejected keeps the probe from becoming always-true. CAVEAT: the test is cfg(unix) while the fixture uses a Windows npx.cmd spelling, so the Windows branch is unexercised."
  - id: c5
    text: "Each runner form has a test pinning which token is queried - uvx, npx, pipx run, pipx install, --from and --with"
    state: met
    evidence: "test:crates/wcore-tools/src/osv_check.rs::every_registry_runner_form_is_checked"
    owner: core
    note: "Pins the queried token for npx, bunx, bun x, pnpm dlx, yarn dlx, npm exec, npm x, uvx, uv tool run and deno run npm:. The remaining named forms are pinned elsewhere: pipx run / pipx install / --spec by pipx_run_queries_the_package_not_the_subcommand, and --from / --with by the_gate_queries_the_from_package_not_the_entry_point. The issue's claim that pipx run queries the literal 'run' is refuted by the tree."
---

The OSV malware gate refuses an MCP stdio launch whose command names a package
with known malware advisories. It is a real improvement - before it, config.toml
MCP servers launched arbitrary npx and uvx packages entirely ungated. The
complaint is that its doc comment claims every stdio launch is checked before
execution, and that claim is broader than the code.

READ THIS BEFORE USING THIS FILE. Unlike the other wayland-core entries seeded
in this pass, #340 was not covered by the v0.13.10 verification sweep. Every
criterion above is transcribed from the issue body alone and every one is
recorded not-met because nothing here has been graded against the shipped tree.
A not-met here means unverified, not measured-absent.

That distinction matters more than usual for this issue, because the body is
itself a cross-audit that labels its own claims: two are marked verified by the
reporter, two are marked reported-but-unverified, and one is marked likely
partially wrong. Anyone picking this up should re-read
crates/wcore-mcp/src/malware_gate.rs and the osv_check parser at the shipped
commit before treating any of the five as established.
