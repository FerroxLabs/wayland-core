# CI-TRIAGE lane — running notes (§6b-i, committed within first 15 min)

Branch `lane/ci-triage`, base `plan/f20-unified-audit-repair` @ `3687cbc2`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-ci-triage`.

Append after every measurement. Do not batch to the end.

---

## Minute 0-10 — instrument defect found before any lane work

**The `rtk` git shim silently drops merge commits from `git log`.** Measured at base:

```
$ /usr/bin/git rev-parse HEAD            -> 3687cbc20f51...   (a merge commit)
$ git --no-pager log --format=%H -3 HEAD -> c57a54c5, 7f5c0455, 8afd1934
$ /usr/bin/git --no-pager log ... -3     -> 3687cbc2, c57a54c5, 5ea07374
```

`git log` through the shim did not merely abbreviate differently — it **omitted
`3687cbc2` and `5ea07374`, both merge commits**, and backfilled with two older
non-merge commits so the output still looked like a well-formed 3-line log.
`rev-parse HEAD` and `log HEAD` disagreed about what HEAD *is*.

This is the §6b-ii defect class carried by my own instrument, and it is directly
load-bearing for this lane: I have to attribute commit `85b60a2f` and reason about
what landed inside the blind window. Every merge in that window is invisible
through the shim.

Also measured: `rtk find` refuses `-not`/`-exec` (loud, fine), and `rtk grep`
rewrites output into a `LINE:COL:` digest form that is not `grep -n` output.

**Mitigation adopted for this lane: `/usr/bin/git`, `/usr/bin/grep`, `/usr/bin/find`
for anything where identity or completeness is load-bearing.** Self-test written at
`.planning/scripts/selftest-git-shim.sh` (three assertions, §6b-ii).

## Minute 10-20 — the three failures located

| # | What | Where |
|---|------|-------|
| 1 | `plugin_discovery_e2e`, `release_binary_smoke` assert capabilities unconditionally | `crates/wcore-cli/tests/plugin_discovery_e2e.rs`, `crates/wcore-cli/tests/release_binary_smoke.rs` |
| 2 | `anvil_forge_transaction::drive_climb_full_*` — `sandbox UNAVAILABLE` in CI only | `crates/wcore-agent/tests/anvil_forge_transaction.rs` |
| 3 | `ci.yml:340` omits `--no-fail-fast` | `.github/workflows/ci.yml` |

Confirmed #3 by direct grep (NOT the shim):

```
ci.yml:340:  command: $DOCKER_RUN "$CI_IMAGE" cargo nextest run --workspace --profile ci
justfile:35: vx cargo nextest run --workspace --profile ci --no-fail-fast
```

The divergence is real. Consequence: the containerized leg aborts on first failure,
so **every historical CI failure count on this repo is a lower bound, not a total.**

`85b60a2f` ("advertise browser/CUA capabilities on liveness, not linkage") is a
deliberate, cross-audited narrowing that fixed a real defect: on a headless host both
flags read `true`, Desktop rendered the capability, and the first operation died with
`spawn camoufox: No such file or directory`. The engine is right; the tests are stale.


## Minute 20-35 — instrument repaired, and it carried the defect class twice

Self-test at `.planning/scripts/selftest-git-shim.sh`: **3 passed, 0 failed**, with
a real differential (A3 fails if the proxy is ever fixed).

Two defects found *while building the instrument that hunts defects*:

**Defect 1 — rtk drops merge commits.** Reproduced deterministically:
```
$ rtk git log --format=%H -n 3 HEAD    # rc=0, 123 bytes
c57a54c5 / 7f5c0455 / 8afd1934         # HEAD (3687cbc2, a merge) is ABSENT
$ /usr/bin/git log --format=%H -n 3 HEAD
3687cbc2 / c57a54c5 / 5ea07374
```
rc=0 and well-formed output. Nothing signals that a commit was withheld.
**Where it bites:** rtk is not on PATH as `git` — a harness hook rewrites *tool-level*
`git ...` into `rtk git ...`. Inside a shell script `git` is the real binary, so this
defect is **invisible to any test that runs in a script and calls plain `git`**. My
first A3 did exactly that and reported "shim no longer drops merges" — a false all-clear.
A3 now invokes `rtk` explicitly, the only path that reaches the bug.

**Defect 2 — my own self-test stole its own exit status.** First draft used
`producer | grep -q PATTERN` under `set -o pipefail`. `grep -q` exits on first match,
producer takes SIGPIPE, pipefail promotes it to 141 — so a **correct match scored as
FAIL**. Measured: `rc=141` while `grep -cx` over identical output returns `1`. This is
LANE-BRIEF §3.2's "a pipe steals exit status", inside the instrument written to hunt
that class. The script now contains **no pipes at all** and matches against files.

**Defect 3 — `wc -c < file` reads 0 through the proxy.** `wc -c < f` → `0`;
`/usr/bin/wc -c < f` → `123`; `stat -f%z` → `123`. The proxy loses the stdin redirect.
Directly relevant to the brief's "byte-count every capture": the byte-counter itself
was the thing lying. All captures in this lane use `/usr/bin/wc`.

Count for the program ledger: this is instance **twelve, thirteen and fourteen** of an
instrument carrying the defect class it hunts — all three found inside one instrument.
