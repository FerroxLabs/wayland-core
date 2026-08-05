# CI-IMAGE-NOTES — running notes, lane/ci-image

Base `plan/f20-unified-audit-repair` @ `0b5182ef`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-ci-image`.
Append after every measurement, per LANE-BRIEF §6b-i. Nothing here is a claim
until a named run is quoted next to it.

---

## T0 — inherited facts (read, not measured by me)

From `.planning/RED-68-TRIAGE.md` (lane `red-68`) and `.planning/CI-TRIAGE.md`
(lane `ci-triage`). I treat these as *prior* measurements to be confirmed by a
real CI run, not as established for my own report.

| class | n | inherited claim |
|---|---|---|
| C1 | 23 | `python3` absent from CI image |
| C3 | 20 | `bubblewrap` absent — and installing it is measured NOT to fix it |
| C4 | 13 | descendant reaping; container, NOT parallelism, NOT the missing `ps` |
| C2 | 6 | `ps` (`procps`) absent from CI image |
| C5 | 3 | container timing / provenance |
| K1/S1/R1 | 3 | already-known / stale test / the contract-digest guard (only true red) |

The image is built inline in `ci.yml` at line 304-312:

```
FROM rust:1.95-slim-bookworm
RUN apt-get install ... libdbus-1-dev libseccomp-dev libssl-dev libasound2-dev \
                       pkg-config mold ca-certificates git
```

No `python3`, no `procps`, no `bubblewrap`.

### The bubblewrap trap, already measured by lane/ci-triage (CI-TRIAGE §2)

Installing `bubblewrap` changes the failure mode and not the outcome. In an image
WITH bwrap installed, on a near-exact match of the runner:

| docker flags | result |
|---|---|
| none (what CI uses today) | `Creating new namespace failed: Operation not permitted` |
| `seccomp=unconfined` alone | same |
| `apparmor=unconfined` alone | same |
| both unconfined | same |
| `--cap-add SYS_ADMIN` | `Failed to make / slave: Permission denied` |
| `SYS_ADMIN` + `apparmor=unconfined` | `pivot_root: Operation not permitted` |
| **`SYS_ADMIN` + `seccomp=unconfined` + `apparmor=unconfined`** | **rc=0** |
| `--privileged` | rc=0 |

And even with the working grant, lane/ci-triage RAN a dedicated job on
`ubuntu-latest`: bwrap could create a namespace, but the **engine's own gate
execution against the bind-mounted `/work` still failed** (`expected
LandingReport::Landed, got None`; 3 candidates built vs 1 on the build host,
`tokens=0+0` = scripted provider exhausted = `cand-0`'s gate did not pass).
So the SYS_ADMIN recipe is necessary and NOT sufficient. That job was removed
rather than shipped red; recoverable from `git show 189599ca -- .github/workflows/ci.yml`.

### Forbidden / constrained by the lane prompt

- `WAYLAND_ALLOW_NO_SANDBOX=1` is forbidden — it converts a sandbox test into a
  test that proves nothing.
- A skip must be **loud and counted**, and the count must actually count. The
  prior lane's counted skip counted nothing: `record_loud_skip` wrote to a
  relative `"target"` path, a test's cwd is the crate root, the open failed and
  `if let Ok` swallowed it. Repaired there via `CARGO_TARGET_TMPDIR` + panic.
  I must not reintroduce that shape.
- `is_available()` is `which::which("bwrap").is_some()` — **presence, not
  capability**. A presence check reports READY in exactly the container where
  the sandbox cannot work.

---

## T1 — what I intend to establish

1. `python3` + `procps` into the inline Dockerfile → **verify by a real CI run
   id**, reading executed counts back, not by reasoning.
2. The bubblewrap 20: privileged-container vs qualify-or-skip-on-a-real-probe.
   Decide with the §4 cross-audit panel, record the dissent.
3. The 13 reaping failures: name the container mechanism, or state precisely
   that it is still unknown. A wrong mechanism here is worse than none because
   process containment is a security property.

## T1a — traps I am carrying into my own instruments

- A suite can exit 0 having run **zero** tests (four flavours measured). Run
  targets **by file**, never by filter; read `N passed` back.
- Byte-count every capture; `echo "EXIT=${PIPESTATUS[0]}"` after a pipeline
  returns empty on this Mac (zsh).
- `rtk` silently filters `git log` and drops merge commits at rc=0, and
  `wc -c < file` reads 0 through the proxy. Use `/usr/bin/git`, `/usr/bin/wc`.
- `gh run view --job <id> --log` is intercepted by `rtk` (`rtk: Run ID required`,
  rc=1). Working path: `gh api /repos/<owner>/<repo>/actions/jobs/<id>/logs`.
- Push **once** and poll. Re-pushing supersedes my own queued run.

---

## LOG

- **T0** worktree created, `lane/ci-image` @ `0b5182ef`, toplevel verified as the
  lane path (NOT `/Users/seandonahoe/dev/waylandcore`). Brief + both prior triage
  reports read. Nothing measured by me yet. This file committed before any
  investigation, per §6b-i.

- **T2 — C4 MECHANISM NAMED AND MEASURED.** `zombie-probe.c`, built static on
  `hetzner-dsm` (Ubuntu 24.04, Docker 29.2.1 — near-exact runner match), four arms:

  | arm | PID 1 | probe says | `/proc` state | verdict |
  |---|---|---|---|---|
  | native (no container) | `systemd` | GONE | `-` (entry gone) | TEST_WOULD_PASS |
  | container, **CI's exact flags** | the test command itself | **ALIVE** | **`Z`** | **TEST_WOULD_FAIL** |
  | container + `--init` | `docker-init` (tini) | GONE | `-` | TEST_WOULD_PASS |
  | container + `--init`, descendant left **genuinely alive** | `docker-init` | ALIVE | `S` | TEST_WOULD_FAIL |

  Self-test 3/3 on the same binary. A3, the discriminator, reads:
  `/proc state=Z probe_alive=1 (a corpse the probe calls ALIVE)`.

  **The mechanism.** `DOCKER_RUN` (ci.yml:283) carries no `--init`, so PID 1 inside
  the container is the test command. Nothing in `crates/` sets
  `PR_SET_CHILD_SUBREAPER` (grepped — the only `prctl` uses are credential drops in
  `wcore-eval-scenarios/src/process_tree.rs`), so an orphaned descendant reparents to
  PID 1. Rust's `Child::wait()` issues `waitpid(<specific pid>)`, never `wait(-1)`, so
  PID 1 cannot incidentally reap an adopted orphan. The corpse stays a zombie
  indefinitely. **Containment genuinely succeeded** — the process holds nothing and
  its listener is dead — but every one of the 13 probes reports it as surviving.

  **Why it is exactly 13, and not approximately 13.** All four test families use a
  probe that a zombie satisfies. The counts add to 13 with nothing left over:

  | probe site | shape | zombie satisfies it because | n |
  |---|---|---|---|
  | `runner_contracts.rs:125` | `kill(pid,0)==0 \|\| errno != ESRCH` | `kill` returns 0 for a zombie | 7 |
  | `pty_capture.rs:783` | identical | same | 2 |
  | `wcore-sandbox/tests/process_capture.rs:12` | `Path::new("/proc/{pid}").exists()` | a zombie has a `/proc` entry | 2 |
  | `wcore-swarm/src/worktree_tests/linux.rs:629` | `/proc/{pid}` | same | 2 |

  This also explains, without further assumption, every prior observation: native
  passes (systemd reaps), 96-core parallel native passes (PID 1 identity is a
  container property, not a concurrency one), and it is not the missing `ps`
  (no probe shells out).

  **Fix I own: add `--init` to `DOCKER_RUN`.** It does NOT make the tests unfailable
  — arm 4 is the control: under `--init`, a genuinely live descendant still reads
  ALIVE and the test still fails.

  **Second defect, NOT mine to fix, reported not carried:** the probes conflate a
  corpse with a live process. On any host without a reaping init they will fail
  again. The correct probe reads `/proc/<pid>/stat` field 3 and excludes `Z`.

  **Falsifiable prediction, stated before the run:** with `--init` the 13 pass in
  real CI. If they do not, this mechanism is wrong and I will say so.

- **T3 — bubblewrap: the prior lane's blocker is REFUTED, and the grant is measured.**
  Matrix on hetzner (`bwmatrix.sh`, `bwmatrix2.sh`), engine argv from `bwrap.rs:212-349`:

  | case | result |
  |---|---|
  | no grant, workspace = bind-mounted `/work` | `No permissions to create new namespace` |
  | grant, workspace = bind-mounted `/work` | `Can't mount proc on /newroot/proc` |
  | grant, workspace = **container-internal** dir | **identical failure** |

  B and C are the same, so **the bind mount was never the variable** — refuting the
  earlier "mount propagation on the docker bind mount" diagnosis. The blocker is
  Docker's masked `/proc` paths, which are locked mounts that refuse bwrap's
  `--proc`. The earlier namespace probe passed only because it omitted `--proc`.

  Minimal grant, each flag closing a distinct refusal (removing any one restores it):
  `--cap-add SYS_ADMIN --security-opt seccomp=unconfined --security-opt
  apparmor=unconfined --security-opt systempaths=unconfined`.

  End-to-end, real tests, bubblewrap installed in BOTH arms so the package is
  isolated from the grant:

  | crate | no grant | with grant |
  |---|---|---|
  | `wcore-sandbox` | 100 run, 86 passed, **14 failed** | 100 run, **100 passed, 0 failed** |
  | `wcore-swarm` + `wcore-tools` | 1344 run, 1330 passed, **14 failed** | 1344 run, **1344 passed, 0 failed** |

  **Zero regressions in either crate.**

- **T4 — instrument defect in MY OWN reasoning, repaired here (§6b-ii).** I read
  "absent from `win81.txt`" as "passed on Windows". For a `#[cfg(target_os="linux")]`
  test, absence means it does not exist there. I used that to argue the bwrap tests
  had native coverage elsewhere — which would have overstated the case for skipping
  them. Repaired: `classify-windows-status.py` consults the cfg gate as a second
  oracle and returns a third state, `NOT_PRESENT`. Self-test 3/3; A3 reports
  `repaired=NOT_PRESENT_ON_WINDOWS old=PASSED_ON_WINDOWS`.

- **T5 — second self-inflicted defect, caught before it shipped.** My first
  `bwmatrix.sh` put `--ro-bind / /` AFTER `--proc`/`--dev`, overmounting them, which
  produced a spurious `cannot create /dev/null`. It did not change the B-vs-C
  conclusion (both failed identically at the proc mount) but the error text was my
  harness's, not the kernel's. Recorded rather than quietly dropped.

- **T6 — a YAML defect my pre-push validation caught.** My step name contained
  `expected: none`; an unquoted colon-space is invalid YAML and would have made the
  ENTIRE workflow unparseable, producing a run that measured nothing. Renamed. The
  validator now also asserts the grant reaches exactly one step and that the other
  seven keep the hardened `DOCKER_RUN`.
