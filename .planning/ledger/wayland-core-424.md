---
issue: 424
repo: FerroxLabs/wayland-core
kind: defect
title: "mutants-nightly has produced zero data in 87 runs: every leg dies in ~30s on a missing target/ parent, and the step exits 0 so it can never go red"
status: open
last_verified_commit: 57e2a244e
criteria:
  - id: c1
    text: "RED ARM: on a checkout with no target/ present, `cargo mutants -p wcore-cron --no-shuffle --timeout 90 --output target/mutants-wcore-cron` fails in under 60 seconds printing `create output parent directory` and `No such file or directory (os error 2)`."
    state: met
    evidence: "commit:b593b206e"
    owner: core
    note: "MET, re-verified at 57e2a244e (b593b206e is an ancestor of it). The red arm this criterion specifies was run and its output is recorded verbatim in the tree: cargo-mutants 27.1.0 against a clean `git archive` of 6e4eca07 with no target/ directory present produced `Error: create output parent directory target/mutants-wcore-cron` / `Caused by: No such file or directory (os error 2)` -- both strings this criterion names -- in roughly 30 seconds, inside the 60s bound. b593b206e carries that before/after pair in its message and is the commit that lands the one-line `mkdir -p target` fix, so the red arm and the change it justifies are anchored to the same object. The cause is also stated there and is self-sustaining: no target/ means mutants fails, so nothing is built, so the cache step saves nothing, so the next run again has no target/. SUPERSEDED CLAUSE: this note previously ended `NOT GRADED: c2 needs the summary line from a CI ARTIFACT... c3 needs a run demonstrating the leg can now conclude failure`. Both of those arrived on 2026-09-04 in scheduled CI run 33844721279 and are graded met below; c4 is graded met from a control run on that run`s artifacts. c5 remains not-met and is the only thing this ticket still owes."
  - id: c2
    text: "The fix produces real data, read from the artifact and not the job log: a run has a wcore-cron leg whose uploaded log contains a line matching `^[0-9]+ mutants tested in `."
    state: met
    evidence: "file:.github/workflows/mutants-nightly.yml:144:mkdir -p target"
    owner: core
    note: "MET at 57e2a244e, FROM THE ARTIFACT AND NOT THE JOB LOG. The run is CI, not the build host, established from run metadata rather than inference: run 33844721279 has event=schedule (the 06:00 UTC cron in this workflow), all five legs are GitHub-hosted `runs-on: macos-latest` jobs, head sha 57e2a244e on branch main, created 2026-09-04T06:31:35Z and updated 10:14:23Z, and the wcore-cron leg ran 09:21:57Z -> 10:14:22Z, 52 minutes of runner wall time. Artifact `mutants-log-wcore-cron` (id 9932557850, 2012390 bytes, not expired) was downloaded with `gh run download`; the log inside it, .blackboard/E2E-MUTATION-BASELINE/wcore-cron.log, is 18883 bytes and contains EXACTLY ONE line matching the anchored regex `^[0-9]+ mutants tested in `, namely `339 mutants tested in 52m: 64 missed, 196 caught, 77 unviable, 2 timeouts`. Identical mutant counts to the build-host green arm below (339 / 64 missed / 196 caught / 77 unviable / 2 timeouts) at 52m of hosted-runner time instead of 15m on the build host -- independent corroboration on different hardware, not a re-quote of the same measurement. The JOB LOG WAS NOT USED for any part of this: it echoes the workflow`s own summary-format comment once per leg and returns 10 hits for `mutants tested in` where the truth is 0, which is precisely why this criterion says ARTIFACT. The anchor is the one-line change that makes any data possible; remove `mkdir -p target` from the step and this claim dies with it."
  - id: c3
    text: "The leg can now conclude `failure`: with the fix in place, a run in which cargo-mutants produces no summary line makes the job conclude failure. Demonstrated, not asserted."
    state: met
    evidence: "file:.github/workflows/mutants-nightly.yml:214:This is a harness failure, not a coverage result."
    owner: core
    note: "MET at 57e2a244e, DEMONSTRATED IN THE SAME RUN AS c2 AND WITH ITS POLARITY CONTROL. Run 33844721279 concluded FAILURE. Four of its five legs -- wcore-config, wcore-providers, wcore-agent, wcore-cli -- produced no summary line in their uploaded artifacts (0 hits each, with the seeded control in c4) and each concluded `failure` at step 8 `Run cargo-mutants`, which is the step carrying the REAL_DATA branch this criterion is about; the four jobs read failure and the run reads failure. Step conclusions were read from the structured jobs API, not grepped out of log text. THE POLARITY CONTROL IS IN THE SAME RUN, and it matters as much as the red: wcore-cron found 64 SURVIVING mutants -- cargo-mutants exits 3 for that -- and its step 8 concluded SUCCESS and its `Report red result to issue tracker` step ran and filed wayland-core#449. So the gate reds on a HARNESS FAILURE and stays green on a FINDING, which is exactly the split the fix was built for, and it is why the branch is on REAL_DATA and not on the exit code. Contrast the 87 runs this ticket is named for: conclusion `success` on all 87, zero data, because the step ended `exit 0   # Never fail the matrix leg` and the no-data path emitted only a ::warning. SEPARATE FINDING, EXPLICITLY NOT PART OF THIS TICKET AND NOT FIXED HERE: those four legs fail because their UNMUTATED baseline test run times out before any mutant is built -- `TIMEOUT  Unmutated baseline in 307s build + 180s test` (wcore-config), `457s build + 60s test` (wcore-providers), `1009s build + 120s test` (wcore-cli), and a timeout after the test list for wcore-agent -- ending `ERROR cargo test failed in an unmutated tree, so no mutants were tested`. Arming the gate has therefore made the nightly red every night for 4 of 5 crates until those per-crate --timeout values are raised. That is a true consequence of this fix working, not a regression in it, and it needs its own issue."
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
    note: "NOT MET, and it is the only thing this ticket still owes. Verified rather than assumed: wayland-core#424 is OPEN and has ZERO comments as of 2026-09-04, so no closing comment of any kind exists. The text it must carry is drafted verbatim in the prose below, ready to paste, and the CI figure now supersedes the build-host one: `339 mutants tested in 52m: 64 missed, 196 caught, 77 unviable, 2 timeouts`, catch rate 196/(196+64) = 75.4%. The 64 survivors are real findings about wcore-cron`s tests and are NOT part of this ticket -- they are what the instrument is for, they were filed automatically as wayland-core#449, and they need their own triage. Recording them here would let a harness fix be mistaken for a coverage result. STRUCTURAL PROBLEM WITH THIS CRITERION, REPORTED NOT PAPERED OVER: c5 as written can never be graded `met` by this gate, because its whole content is a GitHub comment and the evidence grammar has no token for one -- `test:`, `symbol:`, `file:`, `absent:` and `commit:` all resolve against the tree, and a ledger cannot self-anchor with a `file:` token whose fragment lands in the ledger itself. Whoever closes this issue should either restate c5 against an in-tree record or accept it as maintainer-owned closing evidence that this gate does not police."
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
> same run `wcore-config`, `wcore-providers`, `wcore-agent` and `wcore-cli` produced no
> data at all -- their unmutated baseline test run times out -- so their mutation
> coverage remains unmeasured. The 64 surviving `wcore-cron` mutants are findings about
> that crate's tests, not part of this issue; they were filed automatically as #449.
