# HANDOFF — wayland-core RC, 2026-08-04 overnight

Branch `plan/f20-unified-audit-repair` @ **`b854775b`**. PR #257 → main.
Workspace **0.12.26**. Nothing tagged.

**Goal: release-ready RC by morning.** Sean is asleep. Do not wake him. Do not
ask him to touch a machine — see §5, the host has been exonerated eight ways.

---

## 1. THE ONE REMAINING PRODUCT BLOCKER (Windows)

**Confirmed by the product's own instrument, not inferred:**

> `Cause, verbatim from the probe: the probe exceeded its 15s hard wall-clock
> guard — a Win32 setup call (CreateAppContainerProfile /
> CreateProcessAsUserW) stalled`

That line exists because commit `a4e0e144` made the probe self-reporting. Before
it, the refusal said "the cause was logged" and the log never reached CI. **Read
that line first on every future Windows failure.**

### What it means

AppContainer profile creation is not broken — it is **slow under load**. Idle it
takes 0.2s. With 13,547 tests saturating 32 logical cores and dozens of
independent processes each creating a profile, it crosses the 15s guard.
Explains everything: passes alone, fails in CI, passes on retry as load drops.

`probe_cache()` (`process.rs:92`) and `probe_gate()` (`:118`) are `OnceLock`
statics — #754's single-flight collapses probes **within** a process, and every
sandboxed child is its own process. That is the actual gap.

### FAILED ATTEMPT — do not repeat it

`e17c8dba` added bounded retry for transient probe failures. **It did not
work, and could not have.** I classified the wall-clock timeout as
NON-retryable ("a stall means wedged; retrying multiplies the #125 hang") — and
the timeout arm is exactly the one that fires. I fixed the arm that was not
failing. The retry code is still correct and worth keeping for genuine fast
Win32 failures; it is simply not this bug.

### CORRECTION — the O(N²) sweep theory was WRONG. Measured, not inferred.

The previous revision of this file claimed the cause was `recover_dead_leases_locked`
doing an O(N) directory scan inside the lock, giving O(N²). That was arithmetic
that happened to match (24 × ~150ms ≈ 3.4s), not a measurement. **It is false.**

Direct measurement of the sweep against lease count (SEANDESKTOP, 32 cores):

| leases | 0 | 8 | 16 | 24 | 48 | 96 |
|---|---|---|---|---|---|---|
| sweep | 0.09ms | 4.3ms | 8.0ms | 11.7ms | 23.5ms | 44.8ms |

Linear at ~0.47ms/lease. At 24 concurrent that is **11.7ms of a ~141ms critical
section — 8%.** The sweep is real O(N²) and completely irrelevant. Do not "fix" it.

### The real distribution, per lifecycle, all under one machine-wide mutex

| step | cost |
|---|---|
| `CreateAppContainerProfile` (AppX profile-service RPC) | **14.5ms** |
| `DeleteAppContainerProfile` | **~40ms** |
| lease write + fsync | 1.4ms |
| `apply_intents` | 0.3ms |
| recovery sweep | 0.1ms |
| `MutationLock::acquire` | 0.05ms |

~60ms of profile-service RPC serialized machine-wide, per sandboxed command.

### What was FIXED (commit `4d46ee0f`)

Both RPCs moved out of the lock. They need no cross-process exclusion — profile
names are unique per (pid, creation-time, counter). Results:

| metric, 24-way | before | after |
|---|---|---|
| whole lifecycle, median/op | 140ms | **68ms** (103ms under 32 CPU burners) |
| cold cross-process probe, median | 3381ms | **1188ms** |

Windows `cargo clippy` clean; `cargo nextest run -p wcore-sandbox` 172/172.

### What is STILL OPEN — read this before doing anything else

**The reproducer still fails.** Under 32 CPU burners, `cargo nextest run -p
wcore-swarm --test-threads 16` still produces `sandbox UNAVAILABLE … the probe
exceeded its 15s hard wall-clock guard`.

Two hypotheses were tested and REFUTED tonight:
- *CPU starvation.* Under 32 burners the raw profile RPC goes 14.3ms → 15.6ms
  per op. Not it.
- *Queueing alone reaching 15s.* Cold probe scales at ~50ms per additional
  concurrent probe; 15s would need ~300 concurrent. Not obviously reachable.

**The remaining lead, and it is a good one: the probe runs once PER PROCESS.**
`probe_cache()` / `probe_gate()` are `OnceLock` statics, so #754's single-flight
collapses probes only *within* a process — and every sandboxed child is its own
process, as is every nextest test. A full nextest run therefore performs
hundreds of redundant real AppContainer spawns. Each one both lengthens the
queue and, per this file's own comment at `process.rs:541-547`, is another
chance to hit an AV process-creation callback that "can stall ~120s".

**Proposed fix: cache the probe verdict CROSS-process, short TTL.** Clean seam
already exists — `availability()` (`process.rs:140`) passes
`probe_appcontainer_available` into `probe_single_flight`; wrap that one
function. Put the file OUTSIDE the lease directory (the sweep hard-errors on
unknown entries). Write atomically (temp + `MoveFileEx`). Fail open on any
cache I/O error — never worse than today.

Security argument, stated so the next reader can check it: a forged *positive*
cannot cause unsandboxed execution, because the real execution still builds its
own AppContainer and fails closed. A forged *negative* causes refusal, which is
fail-closed and already bounded by `NEGATIVE_PROBE_TTL`. The honest cost is
detection latency: a host that loses sandbox capability mid-window is not
noticed until the TTL expires — but the real spawn still fails closed.

**Do NOT move the fsync'd lease rewrites out of the lock** to chase the last
~28ms. They are atomic (temp + `MoveFileEx`), but an unlocked rewrite races the
recovery sweep's `read_validated_lease` on file replacement, and an
`ERROR_SHARING_VIOLATION` there hard-fails the spawn — reintroducing exactly the
F-28-02-002 wedge class. Not worth 28ms.

### The instrument

`measure_concurrent_lifecycles` (`acl_lease/tests.rs`, `#[ignore]`d, no
assertions on purpose) is the harness that settled this. Run it before
theorising:

```
cargo test -p wcore-sandbox --lib measure_concurrent_lifecycles -- --ignored --nocapture
```

### ADDENDUM 2026-08-05 — CI's Windows failures do NOT reproduce on Windows

This is the most important finding of the night and it reframes the whole leg.

CI run 30910027962's Windows final-4 were `wcore-cli::tool_formatter_real_payloads`
x3 plus one swarm test. On SEANDESKTOP, real hardware, the whole
`tool_formatter_real_payloads` binary is **7/7 PASS — both idle and under 32 CPU
burners.** They are not a Windows product defect, and they are not the sandbox
stall. They are specific to the CI runner environment (service account /
session 0 / container-less step), or they were transient and retry-masked.

Note they call `BashTool` DIRECTLY through `render_real`, not through the engine,
so they are also NOT the known engine-path stdout defect (task #162). Do not
conflate them.

Equally: the one test that finally fails in the local reproducer
(`multi_worker_output_exhaustion_fails_without_retaining_buffers`) is **not in
CI's failure set at all** — it was retry-masked there. And alone it PASSES:
14.3s idle, 24.1s under 32 burners. It only fails inside the full suite at
`--test-threads 16`.

**So the local reproducer and the CI failure are different problems.** A night
was spent optimising the sandbox against a harness that is both harsher than CI
and pointed at other tests. The two landed fixes are real and measured, but
neither was aimed at what CI actually reports.

**Do this next, in this order:**
1. Get the Windows CI log for the CURRENT run and read the failure list fresh.
   Do not assume it matches the b854775b list.
2. For each failure, first ask "does this reproduce on SEANDESKTOP?" before
   touching product code. Three of four did not.
3. Only then decide whether anything in the sandbox path is implicated.

### Trap: do not tar from the Mac without `COPYFILE_DISABLE=1`

Shipping source to `D:\wincheck` with a plain macOS `tar -czf` deposited **4381
AppleDouble `._*` files** into the tree. They are invisible to most builds but
break `wcore-plugin-wasm`: `wasmtime::component::bindgen!` scans the whole `wit`
directory and dies on `._hook.wit` with "stream did not contain valid UTF-8".
That looks exactly like a real build regression and is not one. Use:

```
COPYFILE_DISABLE=1 tar -czf ... && scp ...
```

and on the box, `Get-ChildItem -Recurse -Force -Filter "._*" | Remove-Item`.

### CORRECTION + CURRENT STATE — run 30923295755 on `a0bbc235` (Windows)

**Windows went 4 final failures -> 1.** `13547 tests run: 13546 passed
(3 slow, 9 flaky, 1 leaky), 1 failed, 132 skipped`.

**I was wrong in the addendum above, and it matters.** I wrote that the three
`tool_formatter_real_payloads` tests are "not the sandbox stall" because they
pass 7/7 on SEANDESKTOP idle and under load. The CI log refutes that:

```
TRY 1 FAIL [ 15.067s] wcore-cli::tool_formatter_real_payloads bash_failure_never_reports_exit_zero
  panicked at crates\wcore-cli\tests\tool_formatter_real_payloads.rs:192:5:
  real non-zero exit code missing: Failed to execute command: sandbox child execution failed: ...
```

`15.067s`, `15.058s`, `15.067s` — that is the probe's 15s wall-clock guard, to
three decimal places. These tests DO go through the sandbox and they ARE probe
victims. They pass locally because the probe does not stall locally; the local
run could not reproduce the CI condition, and I over-concluded from its absence.
**Absence of a local repro is not evidence of a different cause.** They now
recover on TRY 3 rather than failing finally, which is where the 4 -> 1 came
from.

The probe stall is therefore still live on Windows CI. It is no longer
*fatal* (retries absorb it) but it costs 15s per affected test and is one
unlucky retry away from red again.

### THE ONE REMAINING WINDOWS FAILURE — well-evidenced, unverified fix

`wcore-swarm::dispatch_smoke::malformed_heartbeat_fails_closed_and_preserves_bounded_diagnostic`,
failing all three tries:

```
panicked at crates\wcore-swarm\tests\dispatch_smoke.rs:425:5:
transaction cleanup: worktree io: io: The process cannot access the file
because it is being used by another process. (os error 32)
```

`os error 32` is `ERROR_SHARING_VIOLATION` — the canonical Windows transient,
raised when another process (AV scan, a just-exited child still closing
handles, the search indexer) briefly holds a file inside the directory being
removed. It clears in milliseconds.

**The lead, and it is a good one: the design anticipates a retry that the
caller never performs.**

- `crates/wcore-swarm/src/dispatch.rs:623` calls `manager.release_transaction(&workspace)`
  EXACTLY ONCE and converts any error into `WorkerHandle::failed("transaction
  cleanup: {error}")`.
- `crates/wcore-swarm/src/worktree.rs:236` already words its own refusal as
  "transaction cleanup refused and its reservation held **for retry**".
- `DirectoryAuthority::remove_open_dir_all`
  (`crates/wcore-sandbox/src/directory_authority.rs:791`) deliberately returns
  the authority BACK inside the error — `Err(Box<(SandboxError, Self)>)` —
  which is the shape you choose precisely so a caller CAN try again.

Every piece of the retry machinery exists. Nobody calls it twice.

**Why I did not just write the retry:** it cannot be verified from here. This
failure does not reproduce on SEANDESKTOP on demand — it is CI-environment
specific — so the fix would ship unproven into an RC, in a security-hardened
subsystem (retained handles, handle-loan accounting, strong-count assertions),
after a night in which three of my Windows root causes were wrong. "Live
testing outranks green code" applies most exactly when the change looks easy.

**When implementing it, decide these two explicitly:**
1. Which errors are retryable. `SwarmError::WorktreeIo(String)` flattens the OS
   code into text, so matching on the errno is not currently possible without
   widening the error type. Retrying the security refusals ("refused cleanup
   outside owned transaction root") would be wrong even if harmless.
2. Backoff and bound. The violation clears in ms; something like 3 attempts at
   50/150ms. After the attempts are spent it must fail EXACTLY as it does today.

### CORRECTION — the ERROR_SHARING_VIOLATION IS reproducible, and it is no longer a blocker

Two updates to the section above, both important.

**1. It reproduces.** I wrote that the worktree-cleanup `ERROR_SHARING_VIOLATION`
"does not reproduce on SEANDESKTOP on demand" and used that to justify not
writing the retry. That was wrong — I had not actually tried. Running the
failing test in a loop under 32 CPU burners:

```
for 20 iterations: cargo nextest run -p wcore-swarm \
  -E 'test(malformed_heartbeat_fails_closed_and_preserves_bounded_diagnostic)'
ITER 1 : SHARING_VIOLATION
ITER 2..16 : pass          (ssh dropped at 16)
```

**~1 hit in 16 under load.** So the retry fix CAN be verified here. Do it with a
much larger N than 16 in each arm — at that base rate, 0/30 after a change is
weak evidence on its own; aim for a few hundred iterations, or drive the
contention harder to raise the rate first.

**2. It is no longer a release blocker.** On CI run 30934350294 the Windows leg
went fully green — `13552 tests run: 13552 passed` — with this failure absent.
It is a rare load-correlated flake that retries absorb, not a hard failure.

Status: a real, characterised, reproducible defect with a known fix shape
(bounded retry at `dispatch.rs:623`, using the authority the error already
hands back), NOT gating the RC. The two design decisions it needs are still the
ones listed above.

---

## 2. Exact CI state — run 30910027962 on `b854775b`

| leg | verdict | failures |
|---|---|---|
| `CI (Array)` (Windows) | **fail** | 4 final; 11 `sandbox UNAVAILABLE`. All downstream of §1 |
| `CI (macos-latest)` | **fail** | 3 final, ALL `voice_live_capture_mac` — pre-existing audio hardware |
| `CI (linux-containerized)` | **fail** | step `F01 packaged wayland-eval driver gate`. **NOT YET DIAGNOSED** |
| all 6 `Build (*)` | success | |
| Eval acceptance gate, Browser live e2e | success | |

Windows final 4: `wcore-cli::tool_formatter_real_payloads` ×3
(`bash_stderr_is_surfaced`, `bash_failure_never_reports_exit_zero`,
`bash_success_renders_the_real_exit_code_and_byte_count`) and
`wcore-swarm::worker_runtime_limits::timeout_releases_workspace_and_capacity_before_return`.
Retry-masked as well: `dispatch_smoke` ×4, `sandbox_activeness`,
`multi_worker_output_exhaustion`, `swarm_worker_failure_reporting_e2e`.

**macOS is at its known baseline.** `corpus_secret`, `corpus_filesystem` and
`wcore-mcp f016_real_spawn_uses_sanitized_launch_context` failed then PASSED on
retry — flaky under load, not deterministic. An earlier claim in this repo that
`corpus_fan_out`/`corpus_secret` were deterministic came from ONE run and was
wrong.

---

## 3. Landed today (do not redo)

- **MCP fixed** — 3 defects, live-proven Linux (`ToolSearch → tiny_ping →
  PONG-7431`). Whole-query substring match → tokenised; bulk `register_mcp_tools`
  never refreshed the catalog (caller-obligation bug); no callability signal.
- `df2f81ae` — reverted the `appcontainer` runner pin (false premise, §5).
- `a4e0e144` — self-reporting probe. **This is what cracked the case.**
- `e17c8dba` — probe retry (correct code, wrong arm for this bug).
- `b854775b` — rustfmt (macOS `Check formatting` was failing before any test ran).
- Runner 22's `appcontainer` label deleted; both Windows runners back in pool.
- Version 0.12.26; `release.yml` tag/tree version guard.

---

## 4. Working harnesses — already warm, reuse them

- **SeanDesktop** `ssh SeanD@seandesktop` (PowerShell, not cmd). `D:\wincheck`
  has a WARM cargo target with `wcore-sandbox` AND `wcore-swarm` built. Ship
  updates with `tar -czf` + `scp` + `tar -xzf` (no rsync on the box).
- **Reproducer for the concurrency defect** (works, use it to prove any fix):
  ```
  cargo nextest run -p wcore-swarm --test-threads 16     # under ~32 CPU burners
  ```
  fails `multi_worker_output_exhaustion_fails_without_retaining_buffers`.
- **Load generator** that reproduces saturation: 32–48 background PowerShell
  busy-loops, then run the suite.
- **hetzner-dsm** `/root/wincheck` — `cargo clippy --target
  x86_64-pc-windows-msvc -p <crate> --all-targets` WORKS and catches
  Windows-only errors from Linux. Also plain Linux clippy. `export
  PATH=$HOME/.cargo/bin:$PATH`.
- **CI logs**: `rtk proxy gh api repos/FerroxLabs/wayland-core/actions/jobs/<id>/logs`.
  Plain `gh run view --log` gets mangled by rtk — always `rtk proxy`.

---

## 5. The host is NOT the problem. Eight measurements.

`ferrox-win-msvc` is **not a second machine** — it is `C:\actions-runner-ferrox`
on SeanDesktop. Three runner services, one 32-core/4090 box, all as
`NT AUTHORITY\NetworkService`. Any plan to "route to the other Windows runner"
is void.

Dead theories: sick machine; fleet cannot AppContainer; the NetworkService
account (tested AS NetworkService via a scheduled task — available, exit 0);
`C:` vs `D:`; disk (597GB free C:, 5TB D:); 153 sandbox tests at 16 threads;
**40 concurrent separate-process cold probes**; that same 40-way stampede under
48 CPU burners pinning all 32 cores. Every one: available, exit 0.

**Do not send Sean to change a service account, policy, or runner config.**

---

## 6. Order of work

1. **Windows §1 fix** — named-mutex serialization, proven with the §4 reproducer.
2. **Linux `F01 packaged wayland-eval driver gate`** — undiagnosed, may be
   trivial. Get its log first.
3. **macOS voice ×3** — pre-existing, needs real audio hardware. Almost certainly
   NOT fixable on a hosted runner; decide whether to gate-skip with an honest
   reason or accept as known-red. Do not silently skip.
4. Re-run CI, confirm green, then RC.

**Sean's bar: nothing lies to the user, nothing loses their data.** A test
skipped without a stated reason, or a gate that cannot fail, violates it.

**Reserved to Sean: merging to main, opening PRs, closing issues.** He said
"release the motherfucker" tonight — tagging is therefore authorised IF and ONLY
IF CI is genuinely green. A tag over red CI is exactly the lie the bar forbids.

---

## 7. Standing rules

- **NEVER run cargo on the Mac.** `cargo fmt` only. Linux → hetzner, Windows →
  SeanDesktop.
- Check `gh run list` before pushing — a push cancels that branch's in-flight run.
- Never `git rebase`/`reset --hard` — ~200 lane branches share the object store.
- `gh auth switch --user FerroxLabs` before every gh op.
- Windows: work on `D:`, never `C:\actions-runner-*`.
- `rtk` silently mangles output — use `rtk proxy` for anything you will quote.
- **Measure the specific thing.** Today: six theories on the Windows failure,
  all wrong, all from inference. The one that held came from making the product
  say what it hit.
