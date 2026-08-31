---
issue: 340
repo: FerroxLabs/wayland-core
kind: defect
title: "The MCP malware gate does not cover every launch, and its doc claims it does"
status: open
last_verified_commit: b92b9656e
criteria:
  - id: c1
    text: "The malware gate's doc comment states the coverage the gate actually has, rather than asserting every stdio launch is checked before execution"
    state: met
    evidence: "test:crates/wcore-mcp/tests/malware_gate_doc_boundary.rs::the_operator_doc_does_not_claim_every_stdio_launch_is_queried"
    owner: core
    note: "MET ON BOTH READINGS NOW. The narrow reading -- the criterion text says the malware gate's DOC COMMENT -- has passed since 43848f75: malware_gate.rs:23-85 is an explicit covered / NOT-covered / fails-open section naming the five uncovered shapes, and Q2 in .planning/DECISIONS.md is the decision behind it (do NOT try to detect indirect runners by spelling; say so in the doc instead). The 0.13.12 close-sweep re-graded this not-met on the WIDER reading -- the issue title is 'its doc claims it does', and the doc an operator actually reads still gave the complete-coverage guarantee -- and the sweep was RIGHT: docs/mcp.md:446 carried 'Every stdio launch is therefore queried against the [OSV] malware feed *before the child process exists*' verbatim, with no coverage boundary anywhere in the section, plus the false converse in its answer table ('Not a package runner | Nothing is fetched, nothing is queried'), which is exactly wrong for the wrapper-script / alias / variable-expanded shapes the module doc admits are uncovered. Not inherited drift: that section was written by this same body of work (#354 c6). CLOSED on lane/f13-n-mcp-gate: the claim is narrowed to the launches the gate can IDENTIFY as a registry fetch, the table row no longer promises nothing is fetched, and the section gains a '### What the gate does not cover' subsection carrying the same five shapes. The cited test grades the narrowing; its siblings the_answer_table_does_not_promise_a_non_runner_fetches_nothing and the_operator_doc_names_the_shapes_that_run_unchecked grade the other two. RED ARM 1 (the fix): git checkout origin/integ/f13 -- docs/mcp.md, touched -- git diff origin/integ/f13 -- docs/mcp.md then EMPTY, so the tests ran against the byte-identical pre-fix doc -- all 3 FAILED, each on its own assertion ('still carries the complete-coverage guarantee', 'still says a command ... fetches nothing', 'no NOT-covered boundary'); restored, touched, 3 passed. RED ARM 2 (the test's own non-vacuity anchor): the cited test also asserts the MODULE doc still owns the boundary it is anchored to, so a rewrite there cannot silently orphan it. Rewriting malware_gate.rs:23 '# What this gate covers, exactly' -> '# Coverage of this gate, precisely' (asserted unique, and it is the doc-comment line itself, not prose about it) reddened EXACTLY that test with 'the module doc's coverage boundary moved; re-anchor this test' and left its two siblings passing; restored, touched, 3 passed, clean git status."
  - id: c2
    text: "An indirect runner shape such as a shell command wrapping a registry package does not reach exec unchecked"
    state: met
    evidence: "test:crates/wcore-mcp/tests/mcp_launch_malware_gate.rs::a_shell_wrapped_runner_is_refused_before_exec"
    owner: core
    note: "Drives the real launch path for sh -c 'npx evil-pkg', sh -c 'cd /tmp && npx -y evil-pkg' and sh -c 'exec pipx run evil-pkg', asserting Err(MalwareBlocked) AND !launch.executed. Negative control ordinary_launches_are_still_neither_queried_nor_blocked keeps it from being a gate that refuses everything."
  - id: c3
    text: "The fail-open on an unreachable OSV endpoint is an explicit operator choice, and where it stays open it is visible to the user"
    state: met
    evidence: "test:crates/wcore-mcp/tests/mcp_launch_malware_gate.rs::permissive_launches_an_unchecked_package_and_logs_at_error"
    owner: core
    note: "RE-ANCHORED AND RE-MEASURED 2026-08-31 (lane/f13-w3-win-honesty). The earlier flip from superseded to met moved this only because the successor wayland-core#354 had CLOSED and check-criteria-ledger.py:923-929 refuses a residual handed to a closed issue; it silenced the gate rather than answering it, and it left the anchor on config.rs::malware_gate_defaults_to_permissive_and_parses_both_modes -- four toml::from_str round trips that never touch OSV, never launch anything, and assert nothing a user sees. NEITHER conjunct was graded by that token. THE RIGHT ANCHOR ALREADY EXISTED: crates/wcore-mcp/tests/mcp_launch_malware_gate.rs drives the real launch path against an unreachable OSV backend in both modes, with a child-written marker file as the did-it-actually-exec instrument. c3 is now anchored to permissive_launches_an_unchecked_package_and_logs_at_error, the one test that grades BOTH conjuncts jointly in the arm this criterion is about: it selects Permissive explicitly, requires the launch to go ahead, and requires an ERROR-level event -- ERROR being the only level a user with RUST_LOG unset ever sees, because wcore-cli caps its stderr writer at Level::ERROR (a warn! there reaches the log file and nobody else). MEASURED, not argued; green 15/15 first, one mutation at a time, restored and touched between arms, 15/15 green again at the end: (a) replacing `match mode` with `match McpMalwareGateMode::Strict` in malware_gate::decide FAILED THIS TEST, so the operator`s permissive selection is load-bearing and not a default that merely happens to agree; (b) downgrading the fail-open at osv_check.rs:790 from tracing::error! to tracing::warn! FAILED THIS TEST, so the visible-to-the-user conjunct is graded and not merely asserted -- the standing worry that a levels.contains(ERROR) assertion could be satisfied by some unrelated ERROR on the same path is refuted by that measurement, this launch path emits none. LIMIT, stated rather than papered over: (c) forcing the opposite direction, `match McpMalwareGateMode::Permissive` (strict ignored), leaves THIS test green and reddens strict_refuses_an_unchecked_package_before_exec instead. So the strict direction is carried by c7, added below, not pretended here. The negative control a_clean_package_launches_in_both_modes stayed green in every arm, so none of this is a gate that passes by refusing everything. RESIDUAL DISPOSITION: the operator-choice remainder was genuinely DELIVERED by #354 (McpMalwareGateMode, the `Unavailable` arm of malware_gate::decide, and install_mode on the doctor probe path) and is graded here against product code, so nothing is left handed to a closed issue. The earlier note`s own history is kept on the issue, not re-copied here: it had been REFUTED once already for claiming there was no strict/permissive setting in the tree when there was."
  - id: c7
    text: "The strict direction of c3`s operator choice: with `malware_gate = strict`, an unperformable malware check refuses the launch BEFORE exec"
    state: met
    evidence: "test:crates/wcore-mcp/tests/mcp_launch_malware_gate.rs::strict_refuses_an_unchecked_package_before_exec"
    owner: core
    note: "ADDED 2026-08-31 (lane/f13-w3-win-honesty) as a bookkeeping split of c3, on the precedent c6 sets in this same file: c3 is a conjunction, this ledger allows ONE machine-resolvable evidence token per criterion, and its own rule is that a criterion needing two pieces of evidence is two criteria. c3 keeps the permissive-and-visible half; this grades the other direction, without which `explicit operator choice` is only half proven. IT IS NOT A NEW ASK and it neither widens nor softens c3 -- c3`s text is untouched. MEASURED: replacing `match mode` with `match McpMalwareGateMode::Permissive` in malware_gate::decide, so strict is ignored, FAILED exactly this test while its permissive sibling stayed green; restored and touched, 15/15. The refusal is asserted to land before exec by the ABSENCE of the marker file the child would have written, and to name the knob that produced it. The negative control a_clean_package_launches_in_both_modes passes in BOTH modes, so strict is not passing here by refusing everything."
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
  - id: c6
    text: "The malware gate's MODULE doc comment is graded by a test, so the coverage boundary cannot be deleted or replaced by the complete-coverage claim without a failure"
    state: met
    evidence: "test:crates/wcore-mcp/src/malware_gate.rs::the_module_doc_states_the_same_coverage_boundary"
    owner: core
    note: "ADDED 2026-08-30 to close a bookkeeping hole the verifier named on c1: the module-doc lint was described in prose and anchored nowhere, so the ledger gate's 'every met criterion is anchored to something that still exists' check never touched it and deleting the lint would not have been caught. It is a separate criterion rather than a second token on c1 because this ledger's own rule is one machine-resolvable evidence token per criterion - 'a criterion needing two pieces of evidence is two criteria'. c1 keeps its operator-doc anchor (tests/malware_gate_doc_boundary.rs, landed by lane/f13-n-mcp-gate); this grades the OTHER half, the doc a developer reads, which is the doc the ticket was actually filed about. The lint reads only the `//!` lines, never the whole file, because its own string literals live in that file and grading the file text would let the assertions satisfy themselves; it carries a positive control on the section header and a negative control proving the `//!` filter is not a no-op. RED ARM 2026-08-30: rewriting 'NOT covered' to 'also handled' in the `//!` lines ONLY - `grep -c '\"NOT covered\",'` still 1, so the test's own literal was untouched - fails with 'the module doc is missing the coverage boundary: \"NOT covered\"'; re-adding 'Every stdio launch is checked before execution.' to the doc fails with 'the module doc has regained the complete-coverage claim'; restored and touched, both green. A FIRST attempt at that red arm was INVALID and is recorded because it is the trap: a whole-file replace rewrote the test's required literal too, so the mutant passed and looked like an ungraded guard."
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
