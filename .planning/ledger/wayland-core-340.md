---
issue: 340
repo: FerroxLabs/wayland-core
kind: defect
title: "The MCP malware gate does not cover every launch, and its doc claims it does"
status: open
last_verified_commit: 9de21aa1
criteria:
  - id: c1
    text: "The malware gate's doc comment states the coverage the gate actually has, rather than asserting every stdio launch is checked before execution"
    state: not-met
    evidence: "file:crates/wcore-mcp/src/malware_gate.rs:23"
    owner: core
    note: "The 'every stdio launch is checked before execution' claim is gone. malware_gate.rs:23-69 is an explicit covered / NOT covered / fails-open section naming the five uncovered shapes. Q2 in .planning/DECISIONS.md is the decision behind it: do NOT try to detect indirect runners by spelling, say so in the doc instead. REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: Evidence resolves: `sed -n 23p crates/wcore-mcp/src/malware_gate.rs` prints `//! # What this gate covers, exactly (FerroxLabs/wayland-core#340)`, and lines 23-85 are a real covered / NOT-covered / fails-open section. The Rust doc comment IS fixed, and `grep -rni 'every stdio launch'` confirms the old sentence survives there only as the historical note at :25. BUT THE CLASS IS NOT CLOSED. The same overclaim is live in the SHIPPED USER-FACING DOC: docs/mcp.md:446 reads verbatim `Every stdio launch is therefore queried against the [OSV](https://osv.dev) malware feed *before the child process exists*.` That whole section ('Supply-chain malware gate — `[mcp] malware_gate`', docs/mcp.md:441-500) contains NO coverage boundary at all — not one of the five uncovered shapes the Rust doc enumerates. Worse, its four-row answer table states the false converse: `Not a package runner | Nothing is fetched, nothing is queried, the server launches` — which is exactly wrong for the wrapper-script / alias / variable-expanded-runner shapes the module doc admits are uncovered. This is not pre-existing drift the lane inherited: that section was WRITTEN OR REWRITTEN BY THIS WORK (it documents `[mcp] malware_gate`, the #354 key, and #354 c6 is literally 'The mode is documented in docs/mcp.md'). The ticket's stated purpose is 'an overstated security guarantee is worse than an understated one, because it stops the next person looking' — the next person is an operator, and the operator's doc still gives the complete-coverage guarantee. The ledger criterion narrowed 'its doc' (issue title) to 'the malware gate's doc comment' and graded the instance. On the narrow reading of the criterion text alone it passes; on the ticket's own close condition it does not."
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
    note: "HALF SHIPPED, and the remainder is wayland-core#354 which is open. The VISIBILITY half is here and graded: osv_check.rs:754 logs the fail-open at tracing::error! and fail_open_is_visible_at_default_log_levels asserts on the recorded LEVEL so a downgrade to warn! reddens it. The OPERATOR-CHOICE half is not: there is no strict/permissive setting anywhere in wcore-config, wcore-mcp or osv_check. Q3 in .planning/DECISIONS.md rules that the real answer is a typed protocol frame, not a log level, and that frame lands with Q4. REFUTED 2026-08-29 by the 0.13.12 close-sweep, recorded verbatim: THE LEDGER NOTE IS FACTUALLY WRONG ABOUT THE TREE. It states 'there is no strict/permissive setting anywhere in wcore-config, wcore-mcp or osv_check.' At origin/integ/f13 there is: `McpMalwareGateMode` at crates/wcore-config/src/config.rs:336, `mcp.malware_gate` at :298, an asymmetric `stricter_of` cascade at :5745-5749, `malware_gate::decide` mapping `Unavailable` through the mode (malware_gate.rs:299-329), and a `/doctor` line at wcore-cli/src/doctor/mod.rs:1130. The ledger was never re-graded after #354 landed (its `last_verified_commit: 43848f75`; worktree HEAD is 5eb2d1ef). The VISIBILITY half genuinely holds and is non-vacuous: `osv_check::tests::fail_open_is_visible_at_default_log_levels` (osv_check.rs:1280) PASSED and asserts EXACT equality `levels == vec![tracing::Level::ERROR]`, so a downgrade to `warn!` reddens it; its sibling `ssrf_refusal_is_visible_at_default_log_levels` also PASSED. THE OPERATOR-CHOICE HALF DOES NOT HOLD AS A CLASS. `grep -rn install_mode crates/ --include=*.rs` returns exactly ONE caller in the entire tree: crates/wcore-agent/src/bootstrap.rs:815. `wayland --doctor --probe-mcp` connect-tests EVERY config-declared MCP server (wcore-cli/src/doctor/mod.rs:654, `McpManager::connect_all(&probe_servers)`) and returns from main.rs:1819 BEFORE any engine bootstrap — the comment at main.rs:1822 even says so ('runs before config/OAuth/engine bootstrap'). So on that path GATE_MODE is unset, `mode()` falls to `unwrap_or_default()` = Permissive, and a `strict` operator's unperformable check fails OPEN — while doctor prints `[mcp] malware_gate = /'strict/'` in the same output (doctor/mod.rs:575). Line-pointer drift in the note too: it cites osv_check.rs:754 as where the fail-open logs at ERROR; at HEAD :754 is inside the SSRF `Unavailable` return, and the SSRF `tracing::error!` is a few lines above. The test name — the load-bearing evidence — resolves exactly. Finally, per the ledger's own bookkeeping this criterion's remainder is wayland-core#354, which I confirmed is STILL OPEN (`gh issue view 354`: state OPEN, closedAt null). State kept `superseded` because the operator-choice remainder genuinely is carried by FerroxLabs/wayland-core#354, which is open and whose c7 is not-met -- but the NOTE above is factually wrong about the tree and is corrected here."
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
