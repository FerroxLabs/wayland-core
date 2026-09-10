---
issue: 1256
repo: FerroxLabs/wayland
kind: defect
title: "A lane can break the Desktop contract corpus and pass its own gate: preflight.sh never asks whether the corpus is current"
status: open
last_verified_commit: 0e423455a
criteria:
  - id: c1
    text: "scripts/preflight.sh FAILS on a stale Desktop contract corpus rather than emitting an advisory hint or nothing at all"
    state: met
    evidence: "test:crates/wcore-protocol/tests/contract_gate_topology.rs::the_lane_preflight_gates_on_corpus_currency_rather_than_hinting_at_it"
    owner: core
    note: "MET 2026-08-30. THE PREDICATE IS INVERTED, which is the point: the two obvious fixes both ask a question about the CHANGE -- 'did the lane also run -p wcore-protocol?' and 'did the diff touch a SOURCE_INPUTS path?' -- and both are proxies that need a correct diff base, a matching path spelling, and someone to have remembered. The question asked instead is about the TREE and is total: is the checked-in corpus current with what is on disk, right now. No diff, no base, no path list, no crate selection, so no lane can be green while the corpus is stale whatever it edited and whatever it chose to test. RED ARMS, measured on lane/f13-w2-mcp-transports 2026-08-30, `cmd > file 2>&1; echo $?` throughout: (1) the drifted tree the lane reported PREFLIGHT=0 for -> EXIT=1, naming fixture_digest and source_inputs_digest and printing the remedy; (2) after `wcore-contract -- generate` and commit -> EXIT=0; (3) a comment appended to a DIFFERENT SOURCE_INPUTS file, crates/wcore-cli/src/budget_grants.rs -> EXIT=1, so this is not specific to the file the defect was found on; (4) NEGATIVE CONTROL, a comment appended to a non-SOURCE_INPUTS file, crates/wcore-cli/src/tui/engine_bridge.rs -> EXIT=0, so it is not a gate that refuses every change; (5) the CORPUS_GATE line commented out -> the topology test EXIT=101; (6) `-- check` swapped for the advisory `-- preflight` -> the topology test EXIT=101, which is the arm that stops the hint being reinstated under the gate's name. Every mutation restored and touched afterwards; git status --porcelain empty."
  - id: c2
    text: "The currency question is asked of the real generator, not of a second implementation of the digest, so the pre-flight and -p wcore-protocol cannot disagree"
    state: met
    evidence: "file:scripts/preflight.sh:373:cargo run -q -p wcore-protocol --bin wcore-contract -- check"
    owner: core
    note: "MET 2026-08-30. A cargo-free Python re-implementation of digest_named_bytes was considered and rejected: a second implementation of a hash drifts silently, and when it drifts it either false-reds forever (and gets deleted) or false-greens (and is worse than nothing). The binary that WRITES the corpus is the only honest oracle for whether the corpus is current. Cost: the pre-flight now needs cargo, which is why it is in its own block and not in GATES -- GATES is a mirror of ci.yml's HOST-side steps and its drift guard derives that set from ci.yml, so putting a container-side gate in it would be a false claim about what CI runs on the host."
  - id: c3
    text: "A lane cannot report a tree green while a crate it chose not to run is red -- the general case, of which the corpus was one instance"
    state: met
    evidence: "symbol:scripts/check-test-scope-coverage.py::receipts_for"
    owner: core
    note: "MET at 0e423455a. THE QUESTION ASKED IS NEITHER OF THE TWO CANDIDATES THIS ROW PARKED ON. (a) `no -p at all` was rejected because it costs wall-clock on every lane and gets ignored; (b) reverse-dependency from the diff was rejected for the reason c1 gives -- it is a proxy needing a correct base. The question asked instead is arithmetic and total: WHICH WORKSPACE MEMBERS DID THIS LANE`S TEST RUNS NOT COVER? No diff, no base, no reverse-dependency graph. The answer is a set, and the lever is DISCLOSURE, not refusal: a scoped run is not failed, it returns the reserved DEGRADED exit code the #1254 protocol defines, which turns preflight`s banner from PASSED into INCOMPLETE and prints every unrun crate by name. Refusing scoped runs outright would be a gate switched off within a day, and lanes scope for good reasons. THE EVIDENCE IS WHAT RAN, NOT WHAT A LANE SAYS RAN: coverage is derived from `tools/remote-proof.py`s receipts for the CURRENT HEAD -- each carries the commit built, the exact cargo_args and the remote exit code -- so a lane cannot talk its way to coverage. A `check` or `clippy` receipt buys nothing (the verb filter has both directions in the self-test), and a receipt whose run FAILED buys nothing either, because `I ran it and it was red` is not evidence a crate is green. Red runs are reported by name and degrade; they do not FAIL, deliberately, because deliberate red arms are how this repo grades its own guards. ARMED RED AGAINST A LIVE TREE THAT REPRODUCES THE DEFECT, BEFORE BEING BELIEVED -- scratch commit 545967662 (tree 5293f58b), since reset off this branch, planted a failing test in wcore-protocol, the exact crate #1256 was found on. Three measurements at that one commit: (1) THE LANE`S OWN VERDICT -- `cargo nextest run -p wcore-mcp` -> remote_exit 0, `Summary [5.027s] 225 tests run: 225 passed, 0 skipped`; (2) THE CRATE IT CHOSE NOT TO RUN -- `cargo nextest run -p wcore-protocol` -> remote_exit 100, `TRY 2 FAIL (212/437) wcore-protocol scratch_1256_red_arm::a_crate_the_lane_did_not_run_is_red`; (3) THE GATE -> EXIT=3 with `DEGRADED: 56 of 57 workspace member(s) were NOT run by anything at this commit`, naming `unrun: wcore-protocol  (crates/wcore-protocol)` explicitly, and separately naming the red run `FAILED: ... cargo nextest run -p wcore-protocol`. That is the #1256 shape end to end: a green scoped verdict beside a red unrun crate, and the gate refusing to let the first stand unqualified. AND IT CAN PASS, which matters as much: `--invocation \"cargo nextest run\"` -> EXIT=0, `OK: every one of the 57 workspace member(s) was covered`; `--self-test` proves rc 0 / 3 / 1 through the real `report` on synthetic member sets, 17 arms, both directions. WIRED into scripts/preflight.sh in its own block beside the corpus gate, NOT in GATES: GATES mirrors ci.yml`s host-side steps and this gate has no meaning in CI, which has no receipts and would be DEGRADED there forever. WHAT IS NOT CLAIMED, and it is a real limit rather than a caveat: (i) it grades CRATE coverage, not TARGET coverage -- `cargo test -p wcore-agent --test one_thing` counts wcore-agent as covered though its lib tests did not run; closing that needs cargo metadata, which this gate is deliberately without so it can run beside the other host-side gates; (ii) the EXIT=0 arm above came from the STATED-invocation path, which the gate itself labels as unverified, because the workspace suite is not green on this tree (wcore-cli::f14_sigkill_recovery is red for reasons predating this lane) so no passing whole-workspace receipt exists here to reach 0 from; (iii) every receipt it reads is Linux, and it says nothing about macOS or Windows."
---

`crates/wcore-cli/src/main.rs` is listed in `wcore_protocol::contract::spec::SOURCE_INPUTS`,
and `source_digest()` reads those files from disk at test time. Lane
`lane/f13-w2-mcp-transports` added 773 lines to it, touched nothing under
`crates/wcore-protocol/contracts/desktop/v1/`, and gated with
`cargo nextest run -p wcore-mcp -p wcore-cli`. `-p wcore-protocol` was EXIT=100:
`checked_corpus_matches_real_serializers_byte_for_byte` and
`the_published_corpus_is_current`, both red on a source-hash rebase.

Nothing the lane ran could have caught it. `scripts/preflight.sh` did not look at
the corpus, and `contract::preflight` is advisory by construction, so
`PREFLIGHT=0` was true and meaningless.

Related, not duplicate: #1254 is the other half of the same file — `preflight.sh`
discards a gate's self-disclosed downgrade on the success path. Distinct
mechanism, distinct fix, and neither fixes the other.
