# wayland#1272 c2 — tranche 3a closing record

Read from the tracker on 2026-09-10, not carried forward from an earlier note.

| issue | state | reason |
|---|---|---|
| FerroxLabs/wayland#1231 | CLOSED | COMPLETED |
| FerroxLabs/wayland#1252 | CLOSED | COMPLETED (decomposed — see below) |
| FerroxLabs/wayland#1254 | CLOSED | COMPLETED |
| FerroxLabs/wayland#1256 | CLOSED | COMPLETED |
| FerroxLabs/wayland-core#393 | CLOSED | COMPLETED |
| FerroxLabs/wayland-core#400 | CLOSED | COMPLETED |

All six reached CLOSED. Six of six.

## The decomposition is owned, not a hiding place

wayland#1252 closed by DECOMPOSITION: its c3 residual became
FerroxLabs/wayland#1276, which is OPEN at milestone 0.13.14. That is what
decomposition means and is not a residual on this row — but the thing c2's
second clause actually guards against is a split that closes a parent and
leaves the child untracked. Checked rather than assumed: #1276 carries its own
ledger at `.planning/ledger/wayland-1276.md`, so it is inside the release gate
(which counts only issues holding a ledger file) rather than outside it.

## wayland#1256 was the last one, and it was closed on its merits

It was the sixth and it stayed open longest. All three of its criteria are met
at the release tree: c3's evidence
`symbol:scripts/check-test-scope-coverage.py::receipts_for` resolves at
`scripts/check-test-scope-coverage.py:169`, and the gate is wired into
`scripts/preflight.sh:416`. c3 was armed RED against a live tree that
reproduced the defect — a scratch commit planted a failing test in
wcore-protocol, the lane's own scoped verdict `cargo nextest run -p wcore-mcp`
returned exit 0 with `225 tests run: 225 passed`, the crate it chose NOT to run
returned exit 100, and the gate exited 3 with `DEGRADED: 56 of 57 workspace
member(s) were NOT run by anything at this commit`, naming `unrun:
wcore-protocol`. The passing direction was shown too, at EXIT=0.

It was NOT closed administratively to make this criterion read met. The
distinction matters: an earlier grading of this row explicitly refused that,
recording that "administrative closure of #1256 would make this criterion read
met over a live gap".
