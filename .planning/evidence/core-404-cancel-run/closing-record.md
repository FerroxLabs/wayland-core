# core#404 c1 — JUnit evidence survives a cancellation of a later step

Demonstrated on a real run, not argued. Every figure below was read from the
run itself or from its downloaded artifacts.

## The run

| | |
|---|---|
| run | FerroxLabs/wayland-core actions run **34474108078** (`CI`) |
| branch | `lane/404-cancel-evidence` — a throwaway ref, so the release run was never cancelled |
| source | `5715b2b352f605e031962f59456a9115a9734312` |
| job | `CI (linux-containerized)` |

`ci.yml` fires on `push -> 'lane/**'`, which is why this did not have to wait
for the release PR.

## The cancellation landed where the criterion needs it

A watcher polled the job's steps and called `gh run cancel` only once BOTH held:
the checkpoint upload had concluded `success`, and a LATER step was
`in_progress`. The recorded moment:

- step **34** `Upload nextest JUnit checkpoint (survives a later cancellation)` — `success`
- step **36** `Voice suite (--features voice; runs in no other configuration)` — `in_progress`
- cancel submitted `2026-09-10T13:07:15Z`; job concluded `cancelled`

## What survived

Artifact `nextest-junit-linux-containerized-checkpoint` (id 10153388760,
1,283,116 bytes zipped, `sha256:59697b7f1efbc0e4a1de49f0751d18be66b536541c5b850ad3404944e88600e2`):

- `junit.xml` — 3,327,190 bytes, root `testsuites`, **813 suites, 18,112
  `testcase` elements, 0 failures** — the completed test identities
- `outer-attempts/final-status.txt` — `success`
- `outer-attempts/attempt-1/runner-exit-code.txt` — `0`
- `outer-attempts/attempt-1/metadata.json` — `cargo nextest run --workspace
  --profile ci --no-fail-fast`, `started_at 2026-09-10T12:35:11Z`,
  `finished_at 2026-09-10T13:06:33Z`, `exit_code 0`

The test step finished 42 seconds before the cancel.

## The report job's own verdict, verbatim

Job `report` (id 102882556512), step `Assert test evidence exists (a skipped
test step is not a pass)`:

    required leg   : nextest-junit-linux-containerized (ci-linux) was cancelled AFTER its test step — 2 report(s), 36224 test case(s) SURVIVED

and the notice it raised:

    The leg 'ci-linux' concluded 'cancelled', and the JUnit it had already
    produced SURVIVED: 2 report(s) holding 36224 test case(s) are downloadable
    from this run under nextest-junit-linux-containerized(-checkpoint). The
    suite RAN; what was lost is whatever ran after it. This is not the same run
    as one that produced no evidence at all (FerroxLabs/wayland-core#404).

The report job still concluded `failure`, correctly: `dependency 'ci-linux'
concluded 'cancelled' -- the required check cannot pass over it.` Surviving
evidence does not turn a cancelled leg into a pass, and it must not.

## THE LIMIT — what this run does NOT prove

The end-of-job upload `nextest-junit-linux-containerized` (id 10153429994)
ALSO survived, with the IDENTICAL digest. That upload carries `if: always()`,
and a manual `gh run cancel` gives `always()` steps time to run. So this run
proves the evidence SURVIVES a later-step cancellation. It does not prove the
CHECKPOINT was what saved it, because the always() upload saved a copy too.

#404's original failure was a cancellation at the 120-minute job BUDGET, where
the runner kills the job and `always()` steps may not get to run. That regime
was not reproduced here: forcing a budget kill means burning a 120-minute leg.
The checkpoint's position — after the test step, before everything else — is
what closes that case, and its ordering is pinned by
`.github/scripts/tests/report-gate-wiring.test.sh`.
