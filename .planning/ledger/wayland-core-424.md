---
issue: 424
repo: FerroxLabs/wayland-core
kind: defect
title: "mutants-nightly has produced zero data in 87 runs: every leg dies in ~30s on a missing target/ parent, and the step exits 0 so it can never go red"
status: open
last_verified_commit: 375efb70b
criteria:
  - id: c1
    text: "RED ARM: on a checkout with no target/ present, `cargo mutants -p wcore-cron --no-shuffle --timeout 90 --output target/mutants-wcore-cron` fails in under 60 seconds printing `create output parent directory` and `No such file or directory (os error 2)`."
    state: met
    evidence: "commit:b593b206e"
    owner: core
    note: "MET, re-verified at 57e2a244e (b593b206e is an ancestor of it). The red arm this criterion specifies was run and its output is recorded verbatim in the tree: cargo-mutants 27.1.0 against a clean `git archive` of 6e4eca07 with no target/ directory present produced `Error: create output parent directory target/mutants-wcore-cron` / `Caused by: No such file or directory (os error 2)` -- both strings this criterion names -- in roughly 30 seconds, inside the 60s bound. b593b206e carries that before/after pair in its message and is the commit that lands the one-line `mkdir -p target` fix, so the red arm and the change it justifies are anchored to the same object. The cause is also stated there and is self-sustaining: no target/ means mutants fails, so nothing is built, so the cache step saves nothing, so the next run again has no target/. SUPERSEDED CLAUSE: this note previously ended `NOT GRADED: c2 needs the summary line from a CI ARTIFACT... c3 needs a run demonstrating the leg can now conclude failure`. Both of those were exercised on 2026-09-04 in SCHEDULED CI run 33844721279. c3 and c4 are graded met from it. c2 is NOT: it names a `workflow_dispatch` run and no such run exists -- see c2, REGRADED 2026-09-10. c2 and c5 are what this ticket still owes."
  - id: c2
    text: "THE FIX PRODUCES REAL DATA, read from the artifact, not the job log. After the fix, one `workflow_dispatch` run has a `mutants / wcore-cron` leg whose uploaded artifact contains a line matching `^[0-9]+ mutants tested in `."
    state: not-met
    evidence: ""
    owner: core
    note: "REGRADED not-met 2026-09-10. THE TEXT ABOVE IS NOW THE ISSUE`S, VERBATIM. The version this ledger carried until today read `a run has a wcore-cron leg` where wayland-core#424`s own acceptance section reads `one `workflow_dispatch` run has a `mutants / wcore-cron` leg`. That broadening is not a paraphrase: it deletes the only part of the criterion that is not already implied by c3, and it was what let a scheduled run be graded against it. MEASURED, NOT INFERRED: `gh run list --workflow mutants-nightly.yml --limit 200` returns 94 runs and `[.[] | select(.event==\"workflow_dispatch\")] | length` is 0 -- 0 of 94 mutants-nightly runs have event=workflow_dispatch, across the whole life of the workflow. The `workflow_dispatch:` trigger has existed on the file since inception and has never once been used. WHAT THE SCHEDULED RUN DID ESTABLISH, AND IT IS NOT NOTHING: run 33844721279 (event=schedule, head_sha=57e2a244e, five hosted `macos-latest` legs, created 2026-09-04T06:31:35Z) uploaded artifact `mutants-log-wcore-cron` (id 9932557850, 2012390 bytes, still unexpired at 2026-09-10). Re-downloaded today with `gh run download`; the log inside it, .blackboard/E2E-MUTATION-BASELINE/wcore-cron.log, contains exactly one line matching the anchored regex `^[0-9]+ mutants tested in `: `339 mutants tested in 52m: 64 missed, 196 caught, 77 unviable, 2 timeouts`, and its mutants.out/missed.txt is exactly 64 lines. The JOB LOG WAS NOT USED: it returns 10 hits for `mutants tested in` where the truth is 0, because it echoes the workflow`s own format comment once per leg. So the runtime property this criterion is about is real and re-verified. WHAT IS OWED, PRECISELY: one `workflow_dispatch` run of `.github/workflows/mutants-nightly.yml` on a commit containing da51d7a59, whose `mutants / wcore-cron` leg uploads an artifact containing a line matching `^[0-9]+ mutants tested in `. Dispatching a workflow is a maintainer operation; this lane does not run it. Grading a schedule run against a criterion that says workflow_dispatch would be reading the criterion as if it said something easier, which is the failure this ledger exists to prevent."

  - id: c3
    text: "The leg can now conclude `failure`: with the fix in place, a run in which cargo-mutants produces no summary line makes the job conclude failure. Demonstrated, not asserted."
    state: met
    evidence: "symbol:.github/scripts/tests/collect-stabilization-mutants.test.py::test_no_summary_line_reds_the_leg_however_cargo_mutants_exited"
    owner: core
    note: "MET at 57e2a244e, DEMONSTRATED IN THE SAME RUN AS c2 AND WITH ITS POLARITY CONTROL. Run 33844721279 concluded FAILURE. Four of its five legs -- wcore-config, wcore-providers, wcore-agent, wcore-cli -- produced no summary line in their uploaded artifacts (0 hits each, with the seeded control in c4) and each concluded `failure` at step 8 `Run cargo-mutants`, which is the step carrying the REAL_DATA branch this criterion is about; the four jobs read failure and the run reads failure. Step conclusions were read from the structured jobs API, not grepped out of log text. THE POLARITY CONTROL IS IN THE SAME RUN, and it matters as much as the red: wcore-cron found 64 SURVIVING mutants -- cargo-mutants exits 3 for that -- and its step 8 concluded SUCCESS and its `Report red result to issue tracker` step ran and filed wayland-core#449. So the gate reds on a HARNESS FAILURE and stays green on a FINDING, which is exactly the split the fix was built for, and it is why the branch is on REAL_DATA and not on the exit code. Contrast the 87 runs this ticket is named for: conclusion `success` on all 87, zero data, because the step ended `exit 0   # Never fail the matrix leg` and the no-data path emitted only a ::warning. SEPARATE FINDING, EXPLICITLY NOT PART OF THIS TICKET AND NOT FIXED HERE: those four legs fail because their UNMUTATED baseline test run times out before any mutant is built -- `TIMEOUT  Unmutated baseline in 307s build + 180s test` (wcore-config), `457s build + 60s test` (wcore-providers), `1009s build + 120s test` (wcore-cli) and `2011s build + 120s test` (wcore-agent) -- the last figure CORRECTED 2026-09-10 from this ledger`s earlier `a timeout after the test list`, which was wrong: that leg`s own artifact baseline line names a completed 2011s build and a 120s test timeout. All four end `ERROR cargo test failed in an unmutated tree, so no mutants were tested`. Arming the gate has therefore made the nightly red every night for 4 of 5 crates until those per-crate --timeout values are raised. That is a true consequence of this fix working, not a regression in it, and it is now core#451. RE-ANCHORED AND MADE EXECUTABLE 2026-09-10 (da51d7a59). The old anchor, file:.github/workflows/mutants-nightly.yml:214, had moved to 271 and would have gone stale on the next edit anyway; more to the point a line anchor only proves a STRING is present, and this criterion says `demonstrated, not asserted`. The evidence is now NightlyGate, which EXTRACTS the `run:` body of the `mutants` step out of the workflow and RUNS it under bash with a stub `cargo` on a PATH that cannot reach a real one (stub dir + /usr/bin + /bin; cargo lives in ~/.cargo/bin). No build, no rustc. The red arm feeds it a censored baseline and `ERROR cargo test failed in an unmutated tree, so no mutants were tested` with cargo-mutants exiting 0, 1 and 4 in turn, and asserts the step exits non-zero every time: the exit code is NOT what the gate branches on, REAL_DATA is. FAIL-BEFORE/PASS-AFTER ON THE SHIPPED FILE, run 2026-09-10, four mutations applied to .github/workflows/mutants-nightly.yml in the committed tree, each confirmed landed by a non-empty `git diff --numstat` before the test was believed: (1) delete `mkdir -p target` -> 4 tests fail, and the reproduction is checked at the DEFECT, not merely at the exit code -- the captured log contains `create output parent directory` and `No such file or directory (os error 2)`, the exact strings from c1; (2) `exit 1` -> `exit 0` under the ::error:: -> 5 failures including test_removing_the_failure_exit_makes_the_gate_unfalsifiable, i.e. the 87-run behaviour reproduced and rejected; (3) restore a censoring `--timeout` -> 1 failure; (4) `if: always()` on the issue-filing step -> 1 failure. Unmutated control: 8/8 green, `git status --porcelain` empty after every restore. LIMIT, STATED: the stub is not cargo-mutants, so this pins the STEP`S CONTRACT -- what it does with a log that has a summary line and one that does not -- and not cargo-mutants` own behaviour. The real-CI half of this claim is still run 33844721279, whose four data-less legs concluded `failure` at step 8 while wcore-cron`s finding concluded `success`."
  - id: c4
    text: "The `no data ever` claim carries its control: any scan asserting a log contains no summary line is run in the same invocation against a copy of that log with a real summary line inserted, and both arms reported -- real logs 0 hits, seeded copy exactly 1 hit."
    state: met
    evidence: "commit:57e2a244e"
    owner: core
    note: "MET. CONTROL RUN TWICE, ON ARTIFACTS BOTH TIMES, AND IT CAUGHT A BAD QUERY THE FIRST TIME -- which is the entire reason this criterion exists. RUN 2, 2026-09-04, over run 33844721279, one shell invocation, both arms per leg, seeded copy = the real artifact log with `339 mutants tested in 52m: 64 missed, 196 caught, 77 unviable, 2 timeouts` appended. ARM 1 real artifact / ARM 2 seeded copy, scanning `^[0-9]+ mutants tested in `: wcore-config 0/1, wcore-providers 0/1, wcore-agent 0/1, wcore-cli 0/1. KNOWN-POSITIVE CONTROL IN THE SAME INVOCATION: wcore-cron, which really did produce data, reads 1/2 -- so the scan is not returning zero because it is broken, and the four zeroes are the absence of a summary line rather than the absence of a working query. RUN 1, 2026-09-03, over run 33599619308: first attempt scanned the JOB LOG with `grep -c mutants tested in` and got 10 hits, not 0. All 10 deduplicate to ONE line, `54 mutants tested in 2m: 9 missed, 28 caught, 17 unviable`, which is the workflow`s own COMMENT illustrating the summary format, echoed once per leg. Anchoring the regex did not save it -- `[0-9]+ mutants tested in [0-9]` still matched the comment, 5 hits, identical figures on all five legs, which real measurements could never produce. The job log cannot discriminate at all; quoting that comment as if it were output would have been the doc-comment-as-live-code trap. CLEAN ARM on that run`s artifact: mutants-log-wcore-cron from 33599619308 is 121 BYTES and its entire content is `Error: create output parent directory target/mutants-wcore-cron / Caused by: No such file or directory (os error 2)`; real artifact 0 hits, same artifact with one real summary line appended exactly 1 hit; all five artifacts on that run are 309-323 bytes against 2012390 bytes for the post-fix wcore-cron artifact, so SIZE alone separates a run from a non-run. REGRADED FROM not-met: the previous pass ran this control cleanly and then withheld the grade on the stated ground that `the criterion asks for this to be part of the closing evidence`. The criterion text asks for no such thing -- it asks that both arms be RUN and REPORTED, which they were then and are again now. The closing-evidence requirement belongs to c5, and c5 remains not-met. ANCHOR: the token is the commit whose scheduled CI run produced the artifacts both arms were run over; nothing in the tree changes when a control is run, so the commit that the evidence came from is the only thing there is to pin."
  - id: c5
    text: "Scope, recorded so the fix is not over-claimed: the closing comment records the wcore-cron summary line verbatim as the first mutation-coverage baseline this repository has measured, together with the catch rate, and states that fixing the harness does not by itself establish coverage for any other crate."
    state: not-met
    owner: core
    note: "NOT MET, and it is the only thing this ticket still owes. Verified rather than assumed: wayland-core#424 is OPEN and has ZERO comments as of 2026-09-04, so no closing comment of any kind exists. The text it must carry is drafted verbatim in the prose below, ready to paste, and the CI figure now supersedes the build-host one: `339 mutants tested in 52m: 64 missed, 196 caught, 77 unviable, 2 timeouts`, catch rate 196/(196+64) = 75.4%. The 64 survivors are real findings about wcore-cron`s tests and are NOT part of this ticket -- they are what the instrument is for, they were filed automatically as wayland-core#449, and they need their own triage. Recording them here would let a harness fix be mistaken for a coverage result. STRUCTURAL PROBLEM WITH THIS CRITERION, REPORTED NOT PAPERED OVER: c5 as written can never be graded `met` by this gate, because its whole content is a GitHub comment and the evidence grammar has no token for one -- `test:`, `symbol:`, `file:`, `absent:` and `commit:` all resolve against the tree, and a ledger cannot self-anchor with a `file:` token whose fragment lands in the ledger itself. Whoever closes this issue should either restate c5 against an in-tree record or accept it as maintainer-owned closing evidence that this gate does not police. RE-VERIFIED 2026-09-10 (da51d7a59): wayland-core#424 is still OPEN and still has ZERO comments, read from `gh issue view 424 --json comments`, so no closing comment of any kind exists. TWO THINGS CHANGED UNDERNEATH THIS DRAFT TODAY. First, it can no longer be posted as a CLOSING comment on the strength of run 33844721279 alone, because c2 was regraded not-met: that run is event=schedule and c2 names workflow_dispatch. Second, its scope paragraph carried the wrong wcore-agent figure; the draft below is corrected. The catch rate is unchanged and re-derived from the artifact today: 196/(196+64) = 75.4%, and that artifact`s mutants.out/missed.txt is exactly 64 lines, so the headline figure and the survivor inventory in core#449 agree. RE-VERIFIED AGAIN 2026-09-10 (375efb70b) AND UNCHANGED: `gh issue view 424 -R FerroxLabs/wayland-core --json number,state,comments` returns state OPEN with a comment count of 0, so the closing comment this criterion is entirely about still does not exist. The c2 blocker underneath it is unchanged too, re-measured in the same session: `gh run list --workflow mutants-nightly.yml --limit 200 --json event` returns 94 runs, grouped as schedule 94 and workflow_dispatch 0. This lane did NOT post the drafted comment, and that is a decision rather than an omission: posting is a maintainer operation, and the draft cannot be posted as a CLOSING comment while c2 is not-met. No in-tree evidence token was invented for c5 either -- the structural note above still stands, and a `file:` token whose fragment landed in this ledger`s own draft would be the ledger certifying itself."
---

# A gate that cannot fail is worth what a gate that cannot pass is worth

This workflow read `success` 87 times without ever testing a mutant. Two independent
defects had to line up for that: `cargo mutants --output target/...` cannot create its
own parent, and the step ended `exit 0` with the no-data path downgraded to a warning.
The first made every run die in under 40 seconds; the second made the death unreportable.

It was also self-sustaining. No `target/` means cargo-mutants fails; failing means
nothing is built; nothing built means `actions/cache` logs *"Path(s) specified in the
action for caching do(es) not exist"* and saves nothing; and the next run again has no
`target/`.

## What run 33844721279 settled, 2026-09-04

Both halves, in one scheduled CI run on `macos-latest`, on main at `57e2a244e`:

* the leg **produces data** -- `wcore-cron`, 52 minutes, summary line in the uploaded
  artifact;
* the leg **concludes failure** -- four legs produced no summary line and every one of
  them turned the job red, and the run red with them.

The polarity control is the important part. `wcore-cron` exited 3, the cargo-mutants
code for surviving mutants, and stayed **green**, because surviving mutants are a
finding rather than a harness failure. A gate that reds on its own findings is a gate
people turn off.

Arming it also revealed what was hidden underneath: four of five crates cannot even run
their **unmutated** test suite inside the per-crate `--timeout`, so mutation coverage on
`wcore-config`, `wcore-providers`, `wcore-agent` and `wcore-cli` is still zero -- now
loudly instead of silently. That is a separate ticket, and it is exactly the kind of
thing 87 green runs were hiding.

## Draft closing comment for c5 -- not yet posted, c5 stays not-met until it is

> First mutation-coverage baseline this repository has ever measured, from the artifact
> of scheduled CI run 33844721279 (`macos-latest`, main @ `57e2a244e`, 2026-09-04):
>
> `339 mutants tested in 52m: 64 missed, 196 caught, 77 unviable, 2 timeouts`
>
> Catch rate 196/(196+64) = **75.4%**, for `wcore-cron` only.
>
> Fixing the harness does not by itself establish coverage for any other crate. In the
> same run `wcore-config` (307s build + 180s test), `wcore-providers` (457s + 60s),
> `wcore-cli` (1009s + 120s) and `wcore-agent` (2011s + 120s) produced no data at all --
> each unmutated baseline TEST run was killed by that crate's `--timeout`, which
> cargo-mutants applies to every cargo command including the baseline -- so their
> mutation coverage remains unmeasured. That is #451. The 64 surviving `wcore-cron`
> mutants are findings about that crate's tests, not part of this issue; they were
> filed automatically as #449.

**This draft is not yet a CLOSING comment.** c2 asks for a `workflow_dispatch` run and
every figure above comes from a `schedule` run; 0 of 94 mutants-nightly runs have ever
been dispatched. Posting it as a scope comment is useful now, but c2 has to be satisfied
by an actual dispatched run before the issue closes on it.
