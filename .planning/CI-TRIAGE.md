# CI-TRIAGE — the three failures the restored signal revealed

Lane `lane/ci-triage`, base `plan/f20-unified-audit-repair` @ `3687cbc2`.
All figures below are read back from real runs; the command and the run are named
for each. Running notes in `.planning/CI-TRIAGE-NOTES.md`.

---

## 1. HIGH — the contract decision (two stale capability tests)

### The decision

`plugin_discovery_e2e` and `release_binary_smoke` asserted
`capabilities.browser_suite` / `.computer_use` **unconditionally true**. Commit
`85b60a2f` narrowed those from **linkage** to **backend liveness**, correctly: on a
headless host both read `true`, Desktop rendered the capability, and the first
operation died with `spawn camoufox: No such file or directory`.

**The engine is right and the tests are stale. The unconditional assertion is NOT
restored** — doing so would re-introduce the false advertisement to make a test pass.

Three candidates were weighed:

| | Candidate | Verdict |
|---|---|---|
| (a) | the capability is present WHEN a backend is live | rejected |
| (b) | the probe answers at all | rejected |
| **(c)** | **the advertisement MATCHES the probe** | **adopted** |

**Why (c) is the one that would have caught the original lie.** The defect was a
*divergence*: the advertisement said `true` while the machine could not start a
backend. "Advertisement agrees with reality" is precisely the invariant that was
violated, so it is the only one whose failure is the defect itself.

**Why (a) would not have caught it.** (a) checks only `live => advertised`. On a
headless host nothing is live, the antecedent is false, and the test passes having
checked nothing — vacuous on exactly the host class where the defect shipped. This
is not a theoretical objection: **(a) is what the old tests effectively were, and it
is measured below passing against a deliberately re-broken engine.**

**Why (b) would not have caught it.** (b) proves an instrument exists and returns.
Under the old code the advertisement path never consulted a probe at all, so (b)
passes while the lie stands. It tests the thermometer, not whether anyone read it.

### How (c) is implemented without becoming a tautology

Computing the expected value by calling the probe under test would be `f(x) == f(x)`.
Instead the tests **plant the environment facts the probes read** and derive the
expectation from what was planted:

| leg | planted | expected |
|-----|---------|----------|
| Live | `WAYLAND_CAMOUFOX_BIN` = an absolute path that resolves; `DISPLAY=:0` | both flags advertised |
| Dead | `WAYLAND_CAMOUFOX_BIN` = a name that cannot resolve; `DISPLAY`/`WAYLAND_DISPLAY` cleared | both withdrawn (Linux) |

Both probes are documented non-executing (`which`-resolution and an env read), so the
live leg costs nothing and launches nothing.

Two refinements adopted from the cross-audit panel:

- **Assert polarity, not change.** Codex: *"a bare 'flag changes' differential is
  weaker than (c) — inverted behaviour also changes."* Each leg asserts an absolute
  expected value.
- **`plugins` is asserted in BOTH legs** as an independent link anchor. Without it the
  Dead leg could pass for the wrong reason (no plugins at all), making the
  differential meaningless.
- **`computer_use` expectation is platform-dependent.** Only Linux can prove a dead
  display without launching anything; macOS/Windows report `Indeterminate`, which by
  design must NOT narrow. Asserting "withdrawn" everywhere would be wrong.

**The linkage guarantee the tests originally existed for is preserved and
strengthened:** a dead-code-stripped plugin cannot be advertised even in the Live leg,
so a dropped `use wayland_<plugin> as _;` still fails.

### Wire-shape correction found by running it

The first implementation asserted `caps["browser_suite"] == false` and FAILED with
`left: Null`. These fields are `#[serde(skip_serializing_if = "is_false")]`, so a
withdrawn capability is **omitted from the wire entirely**. Absent and `false` are the
same claim. The tests now read the flag the way a host does (`advertises()` helper).
This was found by running, not by reading.

### Proof the new assertion catches the original defect — and the old one does not

`narrowed_to_live` was temporarily reverted to linkage-only on the build host
(simulating pre-`85b60a2f`), release binary rebuilt, tests re-run, file restored and
verified byte-identical.

| test | vs FIXED engine | vs RE-BROKEN engine |
|------|-----------------|---------------------|
| `ready_event_advertises_...can_start` (shape (a)) | PASS | **PASS** — proves (a) is blind to the defect |
| `ready_event_withdraws_...cannot_start` (shape (c)) | PASS | **FAIL** |
| `release_binary_ready_event_advertises_...` (shape (a)) | PASS | **PASS** |
| `release_binary_withdraws_...cannot_start` (shape (c)) | PASS | **FAIL** |

That third column is the whole argument, executable.

### Results (build host, Linux)

```
plugin_discovery_e2e   Summary [0.184s]  2 tests run: 2 passed, 0 skipped
release_binary_smoke   Summary [0.167s]  4 tests run: 4 passed, 0 skipped
wcore-cli FULL         Summary [122.657s] 2317 tests run: 2315 passed, 1 failed, 1 timed out, 9 skipped
clippy -p wcore-cli -p wcore-agent --all-targets -- -D warnings   rc=0
```

Both anomalies in the 2,317 run are accounted for and neither is mine:

- `deterministic_openai_loop packaged_core_cancels_an_active_stream` — **passes in
  isolation (2.186s)**. Known MEDIUM, `BACKLOG.md:516` "wall-clock-budgeted binary
  tests are flaky under full-suite load".
- `deterministic_openai_loop packaged_f04_run_is_repeatable_and_content_addressed` —
  failed with `expected source identity is not 40 lowercase hexadecimal characters`.
  **This was my harness, not the repo:** I rsync'd the tree WITHOUT `.git`, so no
  source SHA could be resolved. Re-run with `WAYLAND_BUILD_SOURCE_SHA` set:
  `13 tests run: 13 passed`. Reported here so nobody chases it as a regression.

---

## 2. NEW, CI-only — the sandbox choice

### The brief offered two options. One of them does not work, and that is measured.

`anvil_forge_transaction::drive_climb_full_lands_the_winner_surface_for_accept` fails
in CI with `sandbox UNAVAILABLE` and passes on hetzner (bubblewrap 0.9.0). CI runs
`docker run --rm --network=host` (no `--privileged`, no `--cap-add`, default seccomp)
on a GitHub `ubuntu-latest` runner.

**Installing bubblewrap in the CI image would not have fixed it.** hetzner is Ubuntu
24.04 + Docker 29.2.1 — a near-exact match for the runner — and sets
`kernel.apparmor_restrict_unprivileged_userns=1`. In an image WITH bubblewrap
installed:

| flags (all with `--rm --network=host`) | result |
|---|---|
| none — exactly what CI uses today | `Creating new namespace failed: Operation not permitted` rc=1 |
| `--security-opt seccomp=unconfined` | same, rc=1 |
| `--security-opt apparmor=unconfined` | same, rc=1 |
| `--security-opt seccomp=unconfined --security-opt apparmor=unconfined` | same, rc=1 |
| `--cap-add SYS_ADMIN` | `Failed to make / slave: Permission denied` rc=1 |
| `--cap-add SYS_ADMIN --security-opt apparmor=unconfined` | `pivot_root: Operation not permitted` rc=1 |
| **`--cap-add SYS_ADMIN` + `seccomp=unconfined` + `apparmor=unconfined`** | **rc=0** |
| `--privileged` | rc=0 |

The package install changes the failure mode, not the outcome. Cross-audit panel was
3-of-3 that option (i) as stated is a trap and (ii) is the right test-side answer.

### What was done — (ii), plus the job that stops the skip being permanent

**Test side — qualify-or-skip on a REAL probe.** The qualifier **executes**
`bwrap --ro-bind / / --dev /dev true` and reads the exit status. It is not a `which`
check, and that distinction is load-bearing: `BubblewrapBackend::is_available()` is
`which::which("bwrap").is_some()`, a **presence** check, which would report READY on
exactly the container where the sandbox cannot work.

**`WAYLAND_ALLOW_NO_SANDBOX=1` is not set anywhere.** It is forbidden and would turn
the test into one that proves nothing.

**The skip is loud and counted, and the count actually counts** — see the instrument
defect below. `WCORE_REQUIRE_ENFORCING_SANDBOX=1` (mirroring the existing
`WCORE_SMOKE_REQUIRE_PREBUILT` idiom, #190) converts a non-qualifying sandbox into a
**hard failure**.

**CI side — a dedicated `sandbox-containment-linux` job was built, RUN, and then
REMOVED, because it does not pass and I will not ship a red I introduced.** What it
established is worth more than the job would have been:

- The measured docker grant **transfers to GitHub's runner**. The job's
  "Prove bubblewrap can actually create a namespace here" step **succeeded** on
  `ubuntu-latest` with `--cap-add SYS_ADMIN --security-opt seccomp=unconfined
  --security-opt apparmor=unconfined`. The hetzner measurement was sound.
- **The test still failed — and NOT with `sandbox UNAVAILABLE`.** It got all the way
  into the landing case and panicked with `expected LandingReport::Landed, got None`.
  4 of the 5 tests in the binary passed.
- **The diagnosis, from comparing the two runs' internals** (`--success-output=final`
  on the build host): hetzner builds **one** candidate, `cand-0`, which passes the
  `["true"]` gate and is landed. CI built **three** — `cand-1` and `cand-2` with
  `tokens=0+0`, i.e. the scripted provider exhausted — which only happens when
  `cand-0`'s gate did **not** pass. So bwrap can create a namespace in that container,
  but the **engine's own sandboxed gate execution does not succeed against the
  bind-mounted `/work` workspace**. That is a different and narrower problem than the
  one the brief described, and it is now isolated.
- **My namespace probe was itself too weak** — it proved a proxy (can bwrap unshare?)
  rather than the capability (can the engine run its gate under bwrap here?). Same
  defect class as `is_available()`; logged in section 4.

Two hypotheses were tested and **eliminated**: git identity (hetzner has no global
`user.name`/`user.email` either, and passes without one — measured) and the qualifier
itself (it passed, so the sandbox was deemed usable and the test genuinely ran).

This matters because **Linux runs ONLY containerized** — the native matrix is macOS +
self-hosted Windows (direct-shell Linux was removed after runner-agent crashes). There
is no non-container Linux job to relocate the test to. So today the containment
guarantee is proven **on the Linux build host only**, the main leg skips loudly and
counted, and the CI step says so in a `::warning::` rather than pretending otherwise.

**Follow-up needed (not done here):** determine why the engine's bwrap gate fails
against a bind-mounted `/work`, most plausibly mount propagation on the docker bind
mount. The recipe is otherwise ready — the removed job is recoverable from
`git show 189599ca -- .github/workflows/ci.yml`.

### Proof the gate can fail (build host, bwrap hidden via a symlink farm)

| case | condition | result |
|---|---|---|
| A | bwrap hidden, no require-env | `5 tests run: 5 passed` + `WCORE_SANDBOX_SKIP test=drive_climb_full_... reason=bubblewrap is not installed` recorded |
| B | bwrap hidden, `WCORE_REQUIRE_ENFORCING_SANDBOX=1` | **rc=100**, `5 tests run: 4 passed, 1 failed` |
| C | bwrap available, `WCORE_REQUIRE_ENFORCING_SANDBOX=1` | `5 tests run: 5 passed` (0.866s) |

Case C takes 0.866s against case A's 0.275s — the timing independently corroborates
that the landing case really executed rather than qualifying out.

---

## 3. The instrument finding — `--no-fail-fast`

`ci.yml` line 340 lacked `--no-fail-fast`; `just test-ci` (justfile:35) has always
carried it, and the native matrix leg invokes `just test-ci` directly. **Only the
containerized leg fail-fasted, and Linux runs only there.**

Fixed, with the consequence recorded in the workflow itself:

> **every historical CI failure count on this repository is a LOWER BOUND, not a
> total**, and any triage that treated a count as complete was reading a truncated
> suite.

A companion step reports any recorded sandbox skips as a `::warning::` with a count,
so the compensated skip in the main leg stays visible.

---

## 4. Defects found in my OWN instruments, repaired in this lane (§6b-ii)

Four, all repaired here rather than written up and carried:

1. **The `rtk` git proxy silently drops merge commits.** `rtk git log --format=%H -n 3
   HEAD` returned rc=0 and 123 well-formed bytes with HEAD (a merge) **absent**,
   backfilled with older non-merge commits; `rev-parse HEAD` and `log HEAD` disagreed
   about what HEAD is. Load-bearing here — this lane must attribute `85b60a2f` and
   reason about the blind window. Mitigation: `/usr/bin/git` at the tool layer.
   Self-test `.planning/scripts/selftest-git-shim.sh`, **3 passed, 0 failed**, third
   assertion fails if the proxy is ever fixed (so the workaround gets retired rather
   than carried forever).
2. **My first self-test stole its own exit status.** `producer | grep -q` under
   `set -o pipefail`: grep exits on first match, producer takes SIGPIPE, pipefail
   promotes it to 141, and a **correct match scored as FAIL**. Measured rc=141 while
   `grep -cx` over identical output returns 1. The script now contains no pipes.
3. **My first A3 assertion produced a false all-clear.** It called plain `git` inside
   a script, where `git` is the real binary — the proxy only applies at the
   tool-invocation layer. It reported "shim no longer drops merges" while the defect
   was live. A3 now invokes `rtk` explicitly.
4. **My "counted" skip counted nothing.** `record_loud_skip` wrote to a relative
   `"target"`, but a test's cwd is the crate root, so the open failed and `if let Ok`
   swallowed it. Measured: CASE A produced `5 passed` with no record written anywhere.
   Now resolves the real target dir via `CARGO_TARGET_TMPDIR`, creates it, and
   **panics rather than degrading silently** — a count that can vanish is a claim, not
   a fact. Also documented: **nextest captures a PASSING test's output**, so a skip
   that only prints is invisible in the run that matters; the file is the load-bearing
   channel.

5. **My CI namespace probe proved a proxy, not the capability.** The step
   "Prove bubblewrap can actually create a namespace here" ran
   `bwrap --ro-bind / / --dev /dev true` and **passed**, then the thing it was
   qualifying (the engine running its gate under bwrap in that same container)
   **failed**. A probe that answers an easier question than the one you need is the
   same defect as `is_available()`'s presence check — which is the defect I had just
   written this lane's qualifier to avoid. Repaired by deletion: the job is removed
   rather than left green-probing-and-red-testing, and the real requirement is now
   stated in section 2 for whoever closes it.

Also hit and worth recording: `wc -c < file` reads **0** through the proxy
(`/usr/bin/wc` reads 123) — the byte-counter the brief tells you to trust was itself
lying; `${PIPESTATUS[0]}` is empty in zsh **and** `Bad substitution` in dash, which
silently killed one remote harness run at line 2.

---

## 5. CI runs

**Run 1 — `30403867920`** (HEAD `189599ca`). The one that produced the finding above.

| job | conclusion |
|---|---|
| Browser live e2e (chromium) | success |
| Eval acceptance gate (Linux, containerized) | success |
| **Hard-containment gate (Linux, bubblewrap)** | **failure** — `expected LandingReport::Landed, got None`; 4/5 passed; the bubblewrap namespace probe step SUCCEEDED |

That job is removed in run 2. Its log is the evidence for section 2 and is worth
keeping: job id `90424728437`.

**Run 2 — HEAD after removing the job.** Id and per-job conclusions appended below.

---

## 6. Verdict

- **Failure 1 (HIGH, contract): FIXED**, with the decision argued and the choice proven
  by executable falsification — the adopted assertion fails against the re-broken
  engine, the rejected one passes.
- **Failure 2 (sandbox): PARTIALLY CLOSED, HONESTLY.** The test no longer fails in CI:
  it qualifies on a real execution probe and skips loudly and counted, and the gate is
  proven able to fail (`WCORE_REQUIRE_ENFORCING_SANDBOX=1` → rc=100). The containment
  guarantee itself is proven **on the Linux build host, not in CI**. The brief's
  option (i) is disproved with a measurement table; the residual blocker is isolated
  to the engine's sandboxed gate against a bind-mounted workspace. **This is not a
  complete fix and is not claimed as one.**
- **Failure 3 (`--no-fail-fast`): FIXED**, with the "every historical count is a lower
  bound" consequence recorded in the workflow itself.
