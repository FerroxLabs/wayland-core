# ZOMBIE-PROBE — sixteen sites, not thirteen, and four of them are production

Lane `lane/zombie-probe`, base `plan/f20-unified-audit-repair` @ `797d4889`.
Running notes: `.planning/evidence/zombie-probe/ZOMBIE-PROBE-NOTES.md`.
Raw captures: `MACOS-PROBE-RESULT.txt`, `zombie-probe-macos.c`, `run-capture.sh`.

---

## 0. Headline

- **The finding named 13 failing tests across 4 probe sites. There are 16 code
  sites, and 4 of them are PRODUCTION** — the gateway pidlock, the cron
  already-running guard, the exec-backend child monitor, and the browser
  supervisor's parent watch. Those four are the ones that reach customers; the
  13 red tests were the symptom that made the class visible.
- **The 13 pass with `--init` REMOVED.** That was the brief's real test and it
  is answered directly: same container, `PID1_CMD=sh`, no reaper, all four
  probe targets green.
- **And they go red again the moment the probe reverts** — same commit, same
  no-init container, mutation only: `7 + 2 + 2 + 2 = 13`, the exact split
  CI-IMAGE §2 predicted, reproduced independently from the opposite direction.
  So `--init` is not carrying them; the fix is.
- **The defect was never Linux-only.** Measured on real macOS hardware:
  `kill(pid, 0)` returns 0 for a macOS zombie *and* fails with `EPERM` for a
  live process owned by another user — wrong in **both** directions. A
  Linux-only repair would have looked complete and left macOS broken.
- **The obvious macOS fix is disqualified by measurement, not by argument.**
  `proc_pidinfo` fails identically (`-1`) for a corpse and for a live
  other-user process, so "libproc failed ⇒ dead" is the universal-denial trap
  this lane's brief warns about. The arm that catches it is the one I added
  expecting it to be a formality.
- **Three defects in my own instruments, all repaired in-lane**, one of which
  was found by the very self-test assertion that exists to catch it.

---

## 1. Every site found — 16, established by sweep, not inherited

Swept with `libc::kill`, `/proc/<pid>`, `kill -0`, `tasklist`, `ps`,
`OpenProcess`/`GetExitCodeProcess`, `sysinfo`, and a function-name pass over
`fn *(alive|running|exists|gone|dead|reaped)*`.

### 1a. The four behind the 13 (the finding's own list, confirmed)

| # | site | old shape | tests |
|---|---|---|---|
| 1 | `wcore-eval-scenarios/tests/runner_contracts.rs:125` | `kill(pid,0)==0` | 7 |
| 2 | `wcore-eval-scenarios/src/pty_capture.rs:783` | `kill(pid,0)==0` | 2 |
| 3 | `wcore-sandbox/tests/process_capture.rs:12` | `/proc/<pid>` exists | 2 |
| 4 | `wcore-swarm/src/worktree_tests/linux.rs:629` | `/proc/<pid>` exists | 2 |

### 1b. FOUR PRODUCTION sites the finding did not name — the ones that matter

| # | site | old shape | what it breaks on a host with no reaper |
|---|---|---|---|
| 5 | `wcore-gateway/src/pidlock.rs:282` `process_is_alive` | `/proc/<pid>` exists, else `kill(pid,0)` | a gateway that exited unreaped holds its pidlock **forever**; every later `gateway start` refuses with `AlreadyHeld`. On macOS the `kill` fallback also reported a *foreign-owned live* gateway as gone, so its lock was wrongly reclaimable |
| 6 | `wcore-cli/src/cron.rs:1139` `process_is_alive` | `/proc/<pid>` exists, else **shells out to `kill -0`** | a cron daemon that exited unreaped wedges `cron daemon` permanently. The `kill` binary is also absent from the slim CI image — the same ENOENT that took nextest down in run 26396718138. The Windows arm compared to `STILL_ACTIVE` (259), so a process whose real exit code was 259 read as running |
| 7 | `wcore-exec-backend/src/backends/local.rs:374` `process_alive` | `kill(pid,0)`; Windows `tasklist` substring | an exited-but-unreaped child reads as still executing. The Windows arm substring-matched the pid **anywhere** in `tasklist` output, so `PID eq 42` also matched a memory column containing "42" |
| 8 | `wcore-browser/src/supervisor.rs:475` `process_alive` | `kill(pid,0)`; Windows `tasklist` substring | the supervisor waits on a corpse and **never tears the browser down** |

`pidlock.rs`'s own doc comment claimed it existed "so the workspace has ONE
liveness story". Fifteen other sites had their own.

### 1c. Four hand-rolled zombie checks that already disagreed with each other

All four were Linux-only, so all four were still zombie-blind on macOS.

| # | site | disagreement |
|---|---|---|
| 9 | `wcore-mcp/src/manager.rs:1478` | `.unwrap_or(true)` on an unreadable `/proc/<pid>/stat` — guesses "gone" |
| 10 | `wcore-mcp/src/transport/stdio.rs:1100` | `.unwrap_or(false)` on the **identical** input — guesses "alive". Two files in one crate, opposite answers |
| 11 | `wcore-sandbox/src/backends/no_sandbox.rs:283` | third independent copy |
| 12 | `wcore-agent/tests/dangerous_lease_e2e_test.rs:77` | fourth independent copy |

Site 10 also carried `#[cfg(not(target_os = "linux"))] { false }` with the
comment *"non-Linux runners have a reaper"* — an assumption that is true of a
normal macOS host, false of a macOS runner in a container, and never measured.

### 1d. Four more test probes the finding never named

| # | site | note |
|---|---|---|
| 13 | `wcore-eval-scenarios/tests/smoke.rs:131` | leak check; a leaked-but-dead child was indistinguishable from a leaked live one |
| 14 | `wcore-sandbox/src/backends/process_tree.rs:848` `pid_is_alive` | the containment crate's own test helper |
| 15 | `wcore-tools/tests/cancel_subprocess_test.rs:41` | **its doc comment already said** it returned true for "alive OR zombie" — the defect was written down here and not fixed. A cancellation test whose probe counts a corpse as a survivor cannot tell a cancel that worked from one that did not |
| 16 | `wcore-gateway/src/pidlock.rs` pid-0 guard | folded into the helper (see §2) |

---

## 2. The fix

One module: **`wcore_types::process_liveness`** — `+`the four platform arms,
and every one of the 16 sites above now calls it.

### 2a. Placement, and the dissent

Cross-audit panel (§4 of the brief): codex `A`, kimi `A`, gemini `B`. Majority
`A` = `wcore-types`. The decisive factor all three named was **edge weight, not
edge count**: `A` and `B` each need 3 new dependency edges, but `A`'s land on a
crate with **zero internal dependencies** (no cycle is possible, ever), while
`B` (`wcore-sandbox`) would drag `tokio`/`cap-std`/`tar`/`which`/`uuid` into
`wcore-mcp`, `wcore-gateway` and `wcore-eval-scenarios`. `C` (`wcore-config`,
the home of the existing `wcore_config::shell` helper family) was rejected by
all three: it would force the deliberately dep-light `wcore-sandbox` to inherit
`reqwest`/`keyring`/`argon2`.

Gemini's dissent is real and worth recording: `A` puts OS bindings in a crate
described as "provider-neutral data types". I adopted `A` **with a scope
concession to that objection** — the module is one enum and four functions, it
spawns nothing and signals nothing, and `wcore-types/Cargo.toml` says so
in-line, so the crate does not become an OS grab-bag by precedent. Both new
deps are target-gated leaf system crates that cost nothing on the platform that
does not select them.

### 2b. The answer is three-valued, deliberately

```rust
pub enum ProcessLiveness { Live, Dead, Indeterminate }
```

A probe that answers "dead" for everything makes every containment test in this
workspace pass. A probe that answers "alive" for everything wedges every lock
guard. **Neither failure is visible in a `bool`**, so "I could not tell" (EPERM,
a restricted `/proc`, an unsupported unix) is a distinct third state rather
than a silent lean. `process_is_alive` maps it to `true`, which is the
conservative direction for both caller shapes here: a containment probe fails
**loud** rather than certifying a success it did not observe, and a resource
guard refuses rather than stealing a lock from a process it cannot see.

The four hand-rolled copies disagreed precisely on this branch (§1c). The
helper does not guess there.

### 2c. Per platform — and none of it is guessed

| platform | mechanism | corpse |
|---|---|---|
| Linux | `/proc/<pid>/stat` field 3, parsed from the RIGHT (`comm` may contain `)` **and** spaces) | `Z`, `X`, `x` |
| macOS | `sysctl KERN_PROC_PID` → `kp_proc.p_stat` | `SZOMB` (5) |
| Windows | `OpenProcess(QUERY_LIMITED\|SYNCHRONIZE)` + `WaitForSingleObject(h, 0)` | `WAIT_OBJECT_0` |
| other unix | `kill(pid,0)` only | **indistinguishable → `Indeterminate`** |

`pid == 0` is refused on every platform before it reaches the probe: POSIX
defines `kill(0, sig)` as "the CALLER's process group", so a pid-0 probe answers
a different question than it looks like it asks. This preserves the existing
`pidlock` guard and its test.

The Windows arm uses `WaitForSingleObject`, **not** `GetExitCodeProcess !=
STILL_ACTIVE`, because a process whose genuine exit code is 259 is
indistinguishable from a running one under `STILL_ACTIVE`. Two production
sites (`pidlock`, `cron`) carried that ambiguity.

---

## 3. Proof, both directions, against a real corpse

`crates/wcore-types/tests/real_zombie.rs`. The corpse is created, not
simulated: a child is spawned, allowed to exit, and **never waited on**.
Sequencing needs no external tool — the parent reads the child's stdout to EOF,
which happens inside the child's exit path.

Three assertions, and the third is the one that proves the repair does
anything:

1. **known-positive** — a genuinely running process reads `Live`.
2. **known-negative** — a real unreaped corpse reads `Dead`.
3. **the old shape would have missed it** — asserted FIRST, at the same
   instant: `kill(pid,0) == 0` and `/proc/<pid>` exists both still report the
   corpse ALIVE. Without this the corpse could already have been reaped and
   assertion 2 would pass for the trivial reason.

### Linux — hetzner-dsm, `cargo test -p wcore-types --test real_zombie`

`4 passed; 0 failed; 0 ignored` (executed count read back, not exit status).
Raw `/proc/<pid>/stat` printed by the test itself:

```
independent oracle for pid 3572344: 3572344 (sh) Z 3572337 3572298 ...
```

### The gate can fail — mutation-proved

Mutating `proc_stat_state_is_corpse` to `false` (exactly the old shape):
`3 passed; 1 failed`, `left: Live  right: Dead`. **The three positive-direction
tests stayed green**, which is the other half of the proof: the fix is not
universal denial.

### macOS — measured in C, because cargo is forbidden on the Mac

LANE-BRIEF §0 forbids cargo on the Mac, and neither build host (Linux,
Windows) executes Darwin code, so the macOS *semantics* were measured with
`cc` instead — which is not cargo. `zombie-probe-macos.c`, raw capture in
`MACOS-PROBE-RESULT.txt`:

| arm | `kill(pid,0)` (OLD) | `proc_pidinfo` | `sysctl` p_stat |
|---|---|---|---|
| A real zombie (`ps` says `Z`) | **ALIVE — wrong** | -1 (ESRCH) | **5 = SZOMB** |
| B genuinely live | alive | 2 = SRUN | 2 = SRUN |
| D **live, other user** (launchd) | **NOT alive — wrong** | **-1 (EPERM)** | 2 = SRUN |
| C fully reaped | gone | -1 (ESRCH) | rc=0, size=0 |

Then the **exact algorithm the Rust arm uses** was implemented in C alongside
the struct-typed read and run on all four arms: `4/4 correct`, agreeing with
the struct read every time.

`libc` has no `kinfo_proc` for Apple targets (three `E0425`s from
`cargo check --target aarch64-apple-darwin`), so the Rust arm reads `p_stat`
and `p_pid` from the raw kernel buffer at offsets printed by `offsetof` on real
hardware (`36` and `40`; `sizeof = 648`), and **reads `p_pid` back to verify
the offsets**. An ABI drift becomes `Indeterminate`, never a wrong answer.

### Cross-target typechecks, hetzner

`cargo check -p wcore-types --all-targets --target …` — `TRUE_RC=0` for
`aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-pc-windows-msvc`.

---

## 4. The `--init`-removed experiment — the brief's real question

Container: the image `ci.yml` builds, on `hetzner-dsm`. Environment **confirmed,
not assumed**: `PID1_CMD=sh`, `MY_PID=1` — PID 1 is the test command and there
is no reaper, exactly the condition `--init` was added to remove.

| arm | code | `--init` | grants | result |
|---|---|---|---|---|
| 1 | FIXED | **REMOVED** | no | all four probe targets **GREEN** |
| 2 | pre-fix shape | **REMOVED** | no | **13 failed, split 7+2+2+2** |
| 3 | FIXED | **REMOVED** | yes | `wcore-sandbox --lib` **80/80** |

### ARM 1 — fixed, no `--init`

```
types-zombie TRUE_RC=0 |   4 tests run:   4 passed, 0 skipped
runner-contr TRUE_RC=0 |  21 tests run:  21 passed, 0 skipped
evalsc-lib   TRUE_RC=0 | 220 tests run: 220 passed, 2 skipped
sandbox-pcap TRUE_RC=0 |   4 tests run:   4 passed, 0 skipped
swarm-lib    TRUE_RC=0 | 114 tests run: 114 passed, 0 skipped
tools-cancel TRUE_RC=0 |   4 tests run:   4 passed, 0 skipped
```

Run **by target file, never by filter**, with executed counts read back.
Nothing converted to a skip — the only `skipped` figure anywhere is
`evalsc-lib`'s 2, unchanged.

### ARM 2 — falsification, and it reproduces the cluster exactly

One mutation restores the pre-fix shape for **all four** probes at once (a
zombie satisfies `kill(pid,0)`, and it satisfies `/proc/<pid>` existence). Same
commit, same container, still no `--init`:

```
runner_contracts         21 run:  14 passed,  7 failed   <- 7
pty_capture (lib)       220 run: 218 passed,  2 failed   <- 2
sandbox process_capture   4 run:   2 passed,  2 failed   <- 2
swarm worktree linux    114 run: 112 passed,  2 failed   <- 2
                                             --------
                                                  13
```

**7 + 2 + 2 + 2 = 13, nothing left over** — the same four sites and the same
per-site counts CI-IMAGE §2 predicted, reproduced here independently and from
the opposite direction. `real_zombie` went red too, on exactly the one
assertion meant to catch this and on none of the other three.

So: **they do not only pass with `--init`.** They pass without it, and they
fail without it the moment the probe reverts.

### ARM 3 — the 6 `wcore-sandbox --lib` failures are not mine

ARM 1's `sandbox --lib` showed `80 run: 74 passed, 6 failed`. All six are
`backends::bwrap::tests::*` — the grant class CI-IMAGE §3 measured — and I ran
without the four `--security-opt` grants. Isolated by changing only that
variable, same commit, still no `--init`: **`80 tests run: 80 passed`**.
Reported as an isolated measurement, per LANE-BRIEF §6.

### Other migrated crates, hetzner native

```
gateway-lib 42/42   gateway-pidlock 8/8   gateway-lifecycle 9/9
mcp-lib 129/129     execbackend-lib 88/88  browser-lib 89/89
agent-lease 2/2
```

Workspace gates at `9f0f3af5`: `cargo check --workspace --all-targets`
`TRUE_RC=0`; `cargo clippy --workspace --all-targets -- -D warnings`
`TRUE_RC=0` (one pre-existing third-party note, `imap-proto v0.10.2`);
`cargo fmt --all -- --check` `TRUE_RC=0`.

---

## 5. Defects in my own instruments, repaired here (§6b-ii)

Three, all repaired in-lane rather than written up and carried.

1. **`rtk` served me a stale git ref.** `git log --oneline gh/plan/f20-…`
   returned `45f1a567` while `git rev-parse` on the same ref returned
   `797d4889`. Had I branched on the first reading I would have based the lane
   on the wrong commit. Repaired by routing **every** load-bearing `git`/`rg`
   invocation in this lane through `rtk proxy`. The same hook rewrites `rg` to
   `grep`, which rejects `--no-heading` — a call-site sweep that silently
   degraded would have under-counted the very thing this lane exists to count.

2. **I wrote the canonical self-passing gate by hand.** My first cross-target
   check was `cargo check --target $T 2>&1 | tail -25 ; echo "RC=$?"` and it
   printed **`RC=0` for a check that had failed with three `E0425` errors**,
   because `$?` after a pipeline is `tail`'s status. Inside the lane whose
   entire subject is instruments that cannot distinguish the outcomes they
   exist to distinguish. Repaired as `run-capture.sh` (no pipeline; prints
   `TRUE_RC=` and `LOG_BYTES=`), with a three-assertion self-test:
   **3 checks, 0 failed.**

3. **The self-test's own third assertion was broken, and caught itself.** Its
   first version reproduced the defective idiom *inside* `run-capture.sh`,
   which sets `pipefail` — so the pipeline returned 7 and the assertion
   reported "this platform does not exhibit the defect". It does. The
   reproduction now runs in a plain `bash -c` with `set +o pipefail`, matching
   the ssh shell where the defect actually occurred. **A self-test that
   reproduces a defect under settings the defect cannot survive is a slower way
   of not testing** — and this is the clearest demonstration I can offer that
   the mandated third assertion earns its place: assertions 1 and 2 passed on
   the broken self-test.

A fourth, from the panel rather than my own code: **`codex exec` silently hung
reading stdin** and produced a 39-byte file containing `Reading additional input
from stdin...`. Left unnoticed that is a dropped vote — the exact failure mode
LANE-BRIEF §4 warns about. Fixed with `< /dev/null`; votes extracted unanchored
and, for codex, from the LAST match.

---

## 6. What I did NOT do — stated plainly

- **Did not verify the macOS arm by running Rust on macOS.** LANE-BRIEF §0
  forbids cargo on the Mac and no permitted host executes Darwin code. What IS
  proven there: the kernel semantics, the ABI offsets, and the exact algorithm
  the Rust arm uses, all measured in C on real hardware (§3), plus a clean
  `cargo check` for both Apple targets. What is NOT proven: the Rust
  translation of that algorithm executing on macOS. The single command that
  would close it is `cargo test -p wcore-types --test real_zombie` on a Mac.
  This is a rule-imposed gap, not a technical one, and I am naming it rather
  than implying macOS is as proven as Linux.
- **Did not touch `.github/workflows/ci.yml`.** `--init` stays. The correct
  posture is a container that behaves like a real host **and** probes that do
  not need it to; this lane delivered the second half. Whether to now remove
  `--init` is a separate decision with its own evidence, and removing it would
  discard the `procps`/`python3`/bwrap work in the same file.
- **Did not weaken, `#[ignore]`, re-gate, delete or re-time any test.** Test
  count went **up** by 4 (`real_zombie`); skip counts are unchanged everywhere
  measured.
- **Did not arm-to-arm compare the whole workspace.** Nine crates' targets were
  run; the rest is covered by `cargo check --workspace --all-targets` and
  `clippy --workspace --all-targets`, not by a test run.
- **Did not fix the `wcore-sandbox` bwrap-grant failures.** Not this lane's
  subject; isolated and shown to be the container's grants (§4, ARM 3).
- Did not merge, open a PR, tag, close an issue, or touch `main`.

---

## 7. For the orchestrator to serialize

- **New dependency edges**, all onto `wcore-types` (zero internal deps, so no
  cycle is possible): `wcore-eval-scenarios`, `wcore-exec-backend`,
  `wcore-gateway`. Plus `libc` (unix) and `windows-sys` (windows), target-gated,
  added to `wcore-types` itself.
- **Shared-file fence: untouched.** No edits to `crates/wcore-cli/src/lib.rs`
  or `main.rs` (verified against the merge-base SHA, not the branch name),
  none to `.github/workflows/ci.yml`, none to `.planning/BACKLOG.md`.
  `wcore-cli/src/cron.rs` IS edited — it is not a fenced file.
- **`wcore-gateway::pidlock::process_is_alive` changes behaviour on Windows for
  `ERROR_ACCESS_DENIED`**: previously `false` (lock reclaimable), now
  `Indeterminate → true` (lock held). That is the safe direction for a lock,
  but it is a behaviour change and should be read as one.
