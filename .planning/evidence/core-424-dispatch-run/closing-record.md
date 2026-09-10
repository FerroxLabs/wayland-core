core#424 c5 -- the closing record, as posted
=============================================

Posted 2026-09-10 to FerroxLabs/wayland-core#424 as
https://github.com/FerroxLabs/wayland-core/issues/424#issuecomment-5616750583

This file exists because c5's whole content is a GitHub comment and the
ledger's evidence grammar has no token for one -- test:, symbol:, file:,
absent: and commit: all resolve against the tree. Keeping the posted text
here lets the gate verify that what c5 requires was written, and lets a
reader diff it against the live comment if they suspect drift. The comment
is the artefact; this is its receipt.

---- posted text follows, verbatim ----

## Closing record — scope, so the fix is not over-claimed

**The harness is fixed and it now produces data.** The first `workflow_dispatch` run in
this workflow's history (run `34438249721`, head `fdf4b1e1c`, the 94 runs before it all
`event=schedule`) has a `mutants / wcore-cron` leg that concluded `success` and uploaded
an artifact. Read from that artifact — `mutants-log-wcore-cron`, id `10138518656`, at
`.blackboard/E2E-MUTATION-BASELINE/wcore-cron.log:134` — the summary line, verbatim:

```
339 mutants tested in 28m: 64 missed, 196 caught, 77 unviable, 2 timeouts
```

Exactly one line in that log matches `^[0-9]+ mutants tested in `, so the match is not an
artefact of a loose pattern. The per-outcome files in the same artifact hold 196, 64 and 2
lines respectively, and 196 + 64 + 2 + 77 = 339 — the summary and the outcome files agree,
so neither is a stale copy of the other.

**This is the first mutation-coverage baseline this repository has ever measured.**

**Catch rate, both denominators, because they differ and the choice is not neutral:**

- **74.8%** — 196 caught / 262 (caught + missed + timeouts). This is the figure to quote.
  A timeout is not a kill: the test suite did not demonstrate it detects that mutant, it
  demonstrated it hangs.
- 75.4% — 196 / 260, excluding the 2 timeouts. This is the friendlier number and it is the
  one to distrust, for the reason above.

Unviable mutants are excluded from both: an unviable mutant does not compile, so no test
could have caught it and counting it against the suite would understate coverage.

**What this does NOT establish, stated plainly:**

1. **It is one crate.** `wcore-agent`, `wcore-config`, `wcore-providers` and `wcore-cli`
   were still in progress when this was written. Fixing the harness does not by itself
   establish mutation coverage for any other crate, and this comment should not be cited
   as if it did. That is `wayland-core#451`'s subject, not this ticket's.
2. **A produced baseline is not coverage.** 64 mutants survive `wcore-cron`'s test suite.
   Those survivors are real findings about those tests, they are what the instrument is
   *for*, they were filed as `wayland-core#449`, and they need their own triage. Recording
   them here would let a harness fix be mistaken for a coverage result.
3. **The 64-row missed set is byte-identical to the earlier run's.** That is expected and
   is not a finding about test quality: `crates/wcore-cron/tests/store_hardening.rs` and
   `trigger_arithmetic.rs` — the 14 tests written against those survivors — are absent from
   `fdf4b1e1c` (`git ls-tree -r fdf4b1e1c -- crates/wcore-cron/tests/` lists five files,
   neither of them). This run graded a tree that does not contain them, so it says nothing
   in either direction about whether they kill. `#449` still owes a mutation run on a tree
   that has them.

Baseline the leg measured for itself, from the same artifact:
`{"crate": "wcore-cron", "measured": true, "build_seconds": 116, "test_seconds": 4,
"timeout_multiplier": 5, "minimum_test_timeout": 90, "runner_os": "macOS"}`
