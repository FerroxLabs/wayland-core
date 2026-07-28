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

## Minute 35-70 — the sandbox measurement that killed option (i)

Cross-audit panel, 3/3 both questions (codex 5.6-sol, gemini 3.1-pro, kimi K3):
**Q1 -> (c)** implemented as a two-run differential; **Q2 -> (ii)** qualify-or-skip.
Codex dropped its first vote to the stdin trap the brief warns about (backgrounded, no
tty -> "Failed to read prompt from stdin"); re-run with `< /dev/null` recovered it.
Codex's amendment is adopted: **a bare "the flag changed" differential is weaker than (c),
because inverted behaviour also changes** — assert POLARITY in each leg, not change.

**The decisive measurement.** Brief offered "install bubblewrap in the CI image" as an
acceptable fix. It is not, on its own. Ubuntu 24.04 sets
`kernel.apparmor_restrict_unprivileged_userns=1`; hetzner is Ubuntu 24.04 + Docker 29.2.1,
a near-exact match for GitHub's ubuntu-latest. In an image WITH bubblewrap installed:

| docker run flags (all with --rm --network=host)                    | result |
|--------------------------------------------------------------------|--------|
| (none — exactly what CI uses today)                                  | `Creating new namespace failed: Operation not permitted` rc=1 |
| `--security-opt seccomp=unconfined`                                  | same, rc=1 |
| `--security-opt apparmor=unconfined`                                 | same, rc=1 |
| `--security-opt seccomp=unconfined --security-opt apparmor=unconfined` | same, rc=1 |
| `--cap-add SYS_ADMIN`                                                | `Failed to make / slave: Permission denied` rc=1 |
| `--cap-add SYS_ADMIN --security-opt apparmor=unconfined`             | `pivot_root: Operation not permitted` rc=1 |
| `--cap-add SYS_ADMIN` + seccomp=unconfined + apparmor=unconfined     | **rc=0** |
| `--privileged`                                                       | rc=0 |

So installing the package changes the failure mode, not the outcome. The relaxation is
mandatory, and is scoped to a NEW dedicated job rather than the 12,775-test suite —
running that whole suite with seccomp disabled could let other sandbox tests pass for
the wrong reason. `--privileged` rejected: the narrower measured grant suffices.

**Second finding, unprompted.** `BubblewrapBackend::is_available()` is
`which::which("bwrap").is_some()` — a PRESENCE check. In a container with bubblewrap
installed but namespaces blocked it reports available and the sandbox then dies at
execution. A qualifier written as `which bwrap` would therefore report READY on exactly
the host where the sandbox cannot work. The qualifier I added **executes** a real
`bwrap --ro-bind / / --dev /dev true` and reads the exit status. Logged to BACKLOG-worthy:
the production `is_available()` has the same shallow-probe shape.

**Structural fact that closes off the obvious alternative:** Linux runs ONLY
containerized. The native matrix is macOS + self-hosted Windows (direct-shell Linux was
removed because the GHA runner agent crashes). There is no non-container Linux job to
move the sandbox test to. And the native matrix invokes `just test-ci`, which is why only
the containerized leg fail-fasted.

`WAYLAND_ALLOW_NO_SANDBOX=1` was never used anywhere. It is forbidden and would make the
test prove nothing.

## Minute 70-150 — verification complete

- Falsification proof: re-broke `narrowed_to_live` to linkage-only on the build host,
  rebuilt the release binary, re-ran. The two `withdraws` tests FAIL; the two
  `advertises` tests (the old shape) PASS. Restored, `diff -q` IDENTICAL.
- Wire-shape correction: flags are `skip_serializing_if = "is_false"`, so withdrawn ==
  ABSENT, not `false`. First implementation failed with `left: Null`. Found by running.
- clippy `-p wcore-cli -p wcore-agent --all-targets -- -D warnings` rc=0.
- wcore-cli FULL: 2317 run, my 6 tests all PASS. Two anomalies, neither mine:
  one known MEDIUM load-flake (passes isolated), one caused by MY rsync excluding
  `.git` (13/13 pass with WAYLAND_BUILD_SOURCE_SHA set).
- Sandbox gate three-case proof: A skip+recorded / B hard-fail rc=100 / C real-sandbox
  pass. Timing 0.866s (ran) vs 0.275s (skipped) corroborates.
- Fence: `git diff $BASE -- wcore-cli/src/lib.rs wcore-cli/src/main.rs` is EMPTY.

## Minute 150-200 — CI run 1 result, and a finding that beat my own probe

Run `30403867920`, HEAD `189599ca`. The dedicated `sandbox-containment-linux` job
FAILED — but not the way the brief predicted, and the failure is informative:

- Its "Prove bubblewrap can actually create a namespace here" step **SUCCEEDED** on
  GitHub's ubuntu-latest with `--cap-add SYS_ADMIN --security-opt seccomp=unconfined
  --security-opt apparmor=unconfined`. My hetzner measurement transferred.
- The test then reached the landing case and panicked:
  `expected LandingReport::Landed, got None`. 4/5 in the binary passed.
- Comparing internals: hetzner builds ONE candidate (`cand-0`) which passes the
  `["true"]` gate and lands. CI built THREE, with `cand-1`/`cand-2` at `tokens=0+0`
  (scripted provider exhausted) — which only happens if `cand-0`'s gate FAILED.
  So the engine's sandboxed gate does not succeed against the bind-mounted `/work`,
  even though bwrap can unshare there.

**My probe proved a proxy, not the capability** — the exact defect class I wrote the
qualifier to avoid, one layer up. Instance fifteen.

Eliminated by measurement, not assumed: git identity (hetzner has NO global
user.name/user.email and passes without one — CASE D, 5/5).

Action: removed the job rather than ship a red I introduced. The test-side
qualify-or-skip stands (panel-unanimous, gate proven able to fail). The residual
blocker + full recipe recorded in CI-TRIAGE.md §2; job recoverable from
`git show 189599ca -- .github/workflows/ci.yml`.
