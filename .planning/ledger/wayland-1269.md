---
issue: 1269
repo: FerroxLabs/wayland
kind: defect
title: "Unmerged lane/f13-* branches held out of 0.13.12: four carry work integ lacks, two await their owning lane, five archived as redundant"
status: open
last_verified_commit: 2fe7a70df
criteria:
  - id: c1
    text: "Each of the four 'carries work `integ` lacks' branches reaches a terminal state: merged after an adversarial pass, archived with the reason recorded, or filed as its own ticket. None is left as a bare branch."
    state: not-met
    owner: core
    note: "Graded 2026-09-10 against 2fe7a70df. All four dispositions are now DETERMINED and evidenced in .planning/RECON-1269-1272-1324-2026-09-10.md, from each branch's fork point off gh/integ/f13 to its head, because 0.13.12 was integrated as one squash (93ede3424) and plain ancestry cannot see it. n-ci-evidence ee1c95157 REDUNDANT -- its four not-met criteria were .planning/ledger/wayland-1235.md c1-c4, which landed at 93ede3424 and now read closed/all-met with wl#1235 CLOSED on the tracker. relgate-ci 000db814e REDUNDANT -- release.yml:167-168 and ci.yml:1976-1978 carry it. serde-float-roundtrip 715781e00 INTEGRATED at Cargo.toml:236, see c2. n-dataloss 4eed60a4b INTEGRATED WITH RESIDUAL -- all three production fixes are in the tree via 93ede3424 (reasoning_filter.rs:231 finish, atomic_io.rs restore Swap discriminant, bridge_doorbell.rs:148 install_consent_doorbell) but three of its tests are absent by name: a_prose_mention_of_a_reasoning_tag_survives_in_history, a_json_stream_egress_consent_prompt_is_put_on_the_wire, a_sink_that_cannot_prompt_is_never_given_a_doorbell. NOT MET because the criterion's own words are that none is left as a bare branch, and all four still are: this lane changes no refs by instruction, so the analysis is supplied and the act is owed. Owed: archive-rename n-ci-evidence, relgate-ci and serde-float-roundtrip citing that record; for n-dataloss port the three tests (needs a Linux build slot; all three were occupied) or file them as their own ticket first."
  - id: c2
    text: "`lane/f13-serde-float-roundtrip` specifically: the corpus question is answered with a measurement — regenerate and key-diff, confirming whether `schema_digest` moves — before it is merged or discarded."
    state: not-met
    owner: core
    note: "NOT MET 2026-09-10 and stated rather than implied away. No regenerate was run: it needs a build and all three Linux slots were occupied, and cargo is banned on this host. Two facts the tree does give, recorded in .planning/RECON-1269-1272-1324-2026-09-10.md. (1) source_inputs in the Desktop manifest is a 43-entry list, every entry under crates/wcore-protocol/src/; the root Cargo.toml is NOT in it, so source_inputs_digest cannot move by construction -- one of the three keys settled without a build. (2) fixture_digest is NOT settled: 24 of 156 corpus fixture files carry a non-integer numeral over 14 distinct values, so float rendering is in scope for the fixture bytes. The branch's own manifest being byte-identical to its fork point 4c3e2cd50 in all three keys is NOT evidence -- it is one commit touching only Cargo.toml, so it proves only that nobody regenerated on it. Awkward part stated plainly: float_roundtrip is ALREADY in the shipped tree (Cargo.toml:236, landed by 93ede3424), so the measurement this criterion asks for pre-merge can now only be a post-hoc check of a merged change."
  - id: c3
    text: "`lane/f13-n-security` and `lane/f13-u-protocol` are re-checked against `wayland-core#356` / `#366` **after** their owning lanes land, and either merged for what those lanes missed or archived with that comparison recorded."
    state: not-met
    owner: core
    note: "Precondition SATISFIED and the comparison is DONE and recorded in .planning/RECON-1269-1272-1324-2026-09-10.md; the disposing act is not. Read 2026-09-10: wayland-core#356 CLOSED/COMPLETED 2026-08-31T13:37:05Z, wayland-core#366 CLOSED/COMPLETED 2026-08-31T13:37:13Z, and the ledgers for core#244, #314, #356 and #366 are all status closed with every criterion met at 93ede3424. The tree reimplements both subjects independently: for #366 the unscoped sweep is at contract.rs:340-361 with per-backend declarations and conformance.rs:380, and the tree's test file is container_orphan_scan.rs where the branch's is container_orphan_sweep.rs. For #356 both trees carry a grep_vcs_store_deny.rs but they are DIFFERENT files with different tests; the branch names five cases the tree does not (svn pristine, absolute control-dir spelling, symlink alias, vendored checkout, context-free entry point) and carries git_secret_content_test.rs and workspace_policy_resolver_class.rs which the tree lacks entirely. The tree guards GitTool through SecretDenyFs (git.rs:733-761) where the branch used withhold_secret_diff_sections / refuse_secret_path. Whether those are behaviourally equivalent for git diff and git blame pointed at a committed secret is NOT settled by reading, and the branch's own test file is the instrument -- which is a build. NOT MET: owed is that run on a Linux slot, then archive both branches citing the record, or treat a failure as a live wcore-tools security gap rather than a branch chore."
  - id: c4
    text: "The archived branches stay recoverable until 0.13.12 ships; deleting them is a separate, deliberate act."
    state: met
    evidence: "commit:01b2ac64c4c5d02110c3e3bc15d20f96135e1179"
    owner: core
    note: "MET 2026-09-10. All five archived heads resolve to commit objects with full trees in this worktree and match the SHAs the issue body records: 01b2ac64c4c5d02110c3e3bc15d20f96135e1179 atref-guard, 20f0a060deaea69a20b3a15c1d7489f830e0e1da n-atref, 6a9906cd4b16bea6da46d8aaeba6a9af860e9c21 n-cache, b1e925473377a2186ea4f1f0a74da32e9569853d n-mcp-prov, 760f4f9a20799187b102d6eb23eca3e6fe959149 n-win-escape. git ls-remote hz returns all five at those exact SHAs from ssh://hetzner-dsm/root/w-f13/integ on 2026-09-10. Tag v0.13.12 exists and the tree is at 0.13.14, so the criterion's window closed and was honoured; this lane changed no ref and deleted nothing. The evidence token is one archived head deliberately, because commit: re-resolves on every gate run and reds if the object is ever collected. LIMIT, measured not assumed: git ls-remote gh on 2026-09-10 returns NO archive/f13-* ref at all and does not return lane/f13-relgate-ci either, though it does return the other five lane branches. The only network-reachable copy of all five archived heads and of lane/f13-relgate-ci is one Hetzner working repository at /root/w-f13/integ; fetched clones' object stores are not a backup. Recoverable today, one rm -rf from not being. Pushing archive/f13-* and lane/f13-relgate-ci to FerroxLabs/wayland-core is the mirror-image deliberate act and is recommended, not done here."
---

Created 2026-08-31 to close a COVERAGE gap; re-graded 2026-09-10 by the w14/recon
reconciliation lane against `2fe7a70df`. The full archaeology -- fork points,
per-branch artifact tracing and the remote-reachability measurement -- is in
`.planning/RECON-1269-1272-1324-2026-09-10.md`.

`scripts/check-criteria-ledger.py` scopes every open `area:core` issue on
wayland and EVERY open issue on wayland-core. This issue was in scope from
the moment it was filed and had no ledger file, so
`scripts/check-release-readiness.py` -- which reads ledger files and nothing
else -- could not count it. CI runs the coverage gate with `--offline`, the
arm that would have reported the gap, so nothing said so for two days.

Criteria are transcribed from the issue body without edit. Where the body's
wording is loose it is LEFT loose rather than tightened here: sharpening a
criterion inside the ledger is how a criterion quietly becomes an easier
adjacent property. Whoever takes this restates it on the ISSUE first.
