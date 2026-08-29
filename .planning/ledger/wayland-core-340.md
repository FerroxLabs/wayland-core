---
issue: 340
repo: FerroxLabs/wayland-core
kind: defect
title: "The MCP malware gate does not cover every launch, and its doc claims it does"
status: open
last_verified_commit: 4e24bda6
criteria:
  - id: c1
    text: "The malware gate's doc comment states the coverage the gate actually has, rather than asserting every stdio launch is checked before execution"
    state: met
    evidence: "test:crates/wcore-mcp/tests/malware_gate_doc_boundary.rs::the_operator_doc_does_not_claim_every_stdio_launch_is_queried"
    owner: core
    note: "Both docs, not just the module one. malware_gate.rs:23-85 has been an explicit covered / NOT-covered / fails-open section since 43848f75, and Q2 in .planning/DECISIONS.md is the decision behind it: do NOT try to detect indirect runners by spelling, say so in the doc instead. The SHIPPED OPERATOR doc was still carrying the removed claim verbatim — docs/mcp.md:446 `Every stdio launch is therefore queried against the [OSV](https://osv.dev) malware feed *before the child process exists*` — in a section written by the same work (#354 c6), together with the false converse in its answer table (`Not a package runner | Nothing is fetched, nothing is queried, the server launches`), which is exactly wrong for the wrapper-script / alias / variable-expanded shapes the module doc admits are uncovered. Closed on lane/f13-n-mcp-gate: the claim is narrowed to the launches the gate can IDENTIFY as a registry fetch, the table row no longer promises nothing is fetched, and docs/mcp.md gains a `### What the gate does not cover` subsection carrying the same five shapes. The cited test anchors all three and asserts the module doc still owns the boundary it is anchored to, so a rewrite there cannot silently orphan it; its siblings `the_answer_table_does_not_promise_a_non_runner_fetches_nothing` and `the_operator_doc_names_the_shapes_that_run_unchecked` grade the other two. Red arm: `git checkout origin/integ/f13 -- docs/mcp.md` (diff against the branch then empty) failed all three; restored, touched, 3 passed."
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
    note: "REMAINDER DISCHARGED IN THE TREE; the successor issue wayland-core#354 is still OPEN and closing it is Sean's action. This note previously said `there is no strict/permissive setting anywhere in wcore-config, wcore-mcp or osv_check` — that was true at 43848f75 and is FALSE at this commit: `[mcp] malware_gate` is `symbol:crates/wcore-config/src/config.rs::McpMalwareGateMode`, the asymmetric cascade is `McpMalwareGateMode::stricter_of`, and `malware_gate::decide` maps `Unavailable` through it. The VISIBILITY half is graded by `osv_check::tests::fail_open_is_visible_at_default_log_levels`, which asserts EXACT equality against `vec![tracing::Level::ERROR]`, so a downgrade to `warn!` reddens it. The OPERATOR-CHOICE half is graded on #354 c1-c7; its last hole — `wayland --doctor --probe-mcp` launching every configured stdio server on the uninstalled permissive default while printing `strict` — is closed by `test:crates/wcore-cli/tests/doctor_probe_malware_gate.rs::doctor_probe_launches_under_the_operator_malware_gate`. State stays `superseded` rather than `met` because this entry's own text is graded on #354, and that issue is open."
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
