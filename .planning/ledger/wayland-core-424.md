---
issue: 424
repo: FerroxLabs/wayland-core
kind: defect
title: "mutants-nightly has produced zero data in 87 runs: every leg dies in ~30s on a missing target/ parent, and the step exits 0 so it can never go red"
status: open
last_verified_commit: 6e4eca07
criteria:
  - id: c1
    text: "RED ARM: on a checkout with no target/ present, `cargo mutants -p wcore-cron --no-shuffle --timeout 90 --output target/mutants-wcore-cron` fails in under 60 seconds printing `create output parent directory` and `No such file or directory (os error 2)`."
    state: not-met
    owner: core
    note: "Filed 2026-09-03. Anchored at 6e4eca07, which does not carry the fix, so every criterion is not-met here by construction. RED ARM ALREADY RUN, cargo-mutants 27.1.0, on a clean `git archive` of 6e4eca07 into /root/scratch-424 with no target/ directory: `Error: create output parent directory \"target/mutants-wcore-cron\"` / `Caused by: No such file or directory (os error 2)` -- the CI string verbatim. cargo-mutants does not create the parent of --output, and the workflow has no `mkdir -p target` and no build step before it."
  - id: c2
    text: "The fix produces real data, read from the artifact and not the job log: a run has a wcore-cron leg whose uploaded log contains a line matching `^[0-9]+ mutants tested in `."
    state: not-met
    owner: core
    note: "GREEN ARM ALREADY RUN LOCALLY, same command, same tree, the ONLY difference being `mkdir -p target` first: `339 mutants tested in 15m: 64 missed, 196 caught, 77 unviable, 2 timeouts`. Catch rate 196/(196+64) = 75.4%. That is the FIRST mutation-coverage figure this repository has ever measured, and it arrived from a one-line change. 15 minutes versus the 27-38 seconds every one of the 87 runs took -- the duration alone distinguishes a run from a non-run. Still not-met as written, because c2 requires it from a CI ARTIFACT and this was measured on the build host."
  - id: c3
    text: "The leg can now conclude `failure`: with the fix in place, a run in which cargo-mutants produces no summary line makes the job conclude failure. Demonstrated, not asserted."
    state: not-met
    owner: core
    note: "THIS IS THE HALF THAT MATTERS AND THE HALF THAT WAS MISSING FOR 87 RUNS. The step ended `exit 0   # Never fail the matrix leg`, and the no-data path only emitted a ::warning, so the workflow could not report its own death. The change keeps the design intent -- surviving mutants are a FINDING and must not red the leg, they flow via the issue and the annotation -- and separates it from a harness failure: REAL_DATA=false now emits ::error and exits 1. Note the green arm exited 3 (cargo-mutants' code for surviving mutants) and must still pass, which is exactly why the branch is on REAL_DATA and not on the exit code."
  - id: c4
    text: "The `no data ever` claim carries its control: any scan asserting a log contains no summary line is run in the same invocation against a copy of that log with a real summary line inserted, and both arms reported -- real logs 0 hits, seeded copy exactly 1 hit."
    state: not-met
    owner: core
    note: "The claim this criterion polices is `87 runs, conclusion success on all 87, zero data`. The run census is direct (gh run list --workflow mutants-nightly.yml, conclusions all `success`, oldest 27133754825 2026-06-08 which is the workflow's inception, newest 33599619308 2026-09-02) and the newest run's log carries the parent-directory error on every one of its five legs at 27-38 seconds each. The control arm is NOT yet run: an empty grep over a log reads exactly like a log with no summary, and this repository has been bitten by an inverted control on this very question -- an earlier pass asserted run 27133754825 showed HAS_DATA on all five legs when issues #1, #3 and #5 from that same run read `Catch rate: N/A%` and `Summary: no summary`."
  - id: c5
    text: "Scope, recorded so the fix is not over-claimed: the closing comment records the wcore-cron summary line verbatim as the first mutation-coverage baseline this repository has measured, together with the catch rate, and states that fixing the harness does not by itself establish coverage for any other crate."
    state: not-met
    owner: core
    note: "The line to record is `339 mutants tested in 15m: 64 missed, 196 caught, 77 unviable, 2 timeouts`. The 64 survivors are real findings about wcore-cron's tests and are NOT part of this ticket -- they are what the instrument is for, and they need their own triage once the instrument runs. Recording them here would let a harness fix be mistaken for a coverage result."
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
