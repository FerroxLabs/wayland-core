---
issue: 1256
repo: FerroxLabs/wayland
kind: defect
title: "A lane can break the Desktop contract corpus and pass its own gate: preflight.sh never asks whether the corpus is current"
status: open
last_verified_commit: 93ede3424
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
    evidence: "file:scripts/preflight.sh:345:cargo run -q -p wcore-protocol --bin wcore-contract -- check"
    owner: core
    note: "MET 2026-08-30. A cargo-free Python re-implementation of digest_named_bytes was considered and rejected: a second implementation of a hash drifts silently, and when it drifts it either false-reds forever (and gets deleted) or false-greens (and is worse than nothing). The binary that WRITES the corpus is the only honest oracle for whether the corpus is current. Cost: the pre-flight now needs cargo, which is why it is in its own block and not in GATES -- GATES is a mirror of ci.yml's HOST-side steps and its drift guard derives that set from ci.yml, so putting a container-side gate in it would be a false claim about what CI runs on the host."
  - id: c3
    text: "A lane cannot report a tree green while a crate it chose not to run is red -- the general case, of which the corpus was one instance"
    state: not-met
    evidence: ""
    owner: core
    note: "NOT MET, and stated rather than implied away. c1/c2 close the corpus instance TOTALLY -- that observable no longer depends on the lane's crate list at all. They do not close the class: any other test that reads a file by path at test time, in a crate the lane did not name, has the same shape. The two candidate closures are (a) lanes gate on `cargo nextest run` with no -p at all, which is the honest total answer and costs wall-clock on every lane, and (b) a pre-flight that derives the affected crate set from the diff by reverse-dependency, which is a proxy again and needs a correct base. Neither is decided; this criterion is the reason this issue stays open."
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
