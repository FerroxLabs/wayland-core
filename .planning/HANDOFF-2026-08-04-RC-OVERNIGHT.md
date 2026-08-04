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

### ROOT CAUSE — found, and it is O(N²) under a global lock

**Do NOT add a named mutex. Every AppContainer spawn is ALREADY globally
serialized, and that is the problem.**

`ExecutionIdentity::start_with_apply` (`acl_lease.rs:250`) runs on every
sandboxed spawn and holds ONE machine-wide mutex
(`MutationLock::acquire()`, `acl_lease.rs:256`, itself a 15s-timeout named
mutex in `acl_lease/mutation_lock.rs`) across all of:

1. `recover_dead_leases_locked` — **a full `read_dir` + a `file_type()` stat per
   entry** (`acl_lease.rs:614`)
2. `allocate_unique_profile` → `CreateAppContainerProfile` (profile-service RPC)
3. an fsync'd lease-file write
4. `apply_intents` — ACL application

Step 1 is the killer. The lease directory holds **one .toml per LIVE lease**, so
with N concurrent spawns each spawn scans ~N files: **O(N²) work, serialized**.

**Measured on SEANDESKTOP (32 logical cores), probe wall-time:**

| concurrency | mean | note |
|---|---|---|
| 1 (idle) | ~200ms | 577/202/180ms over three runs |
| 24 | **3381ms** (min 2656, max 3807) | ~17× |

24 × ~150ms critical section ≈ 3.6s — the measurement matches the mechanism.
Extrapolating, ~100 concurrent spawns crosses BOTH the mutex's own 15s acquire
timeout AND the probe's 15s wall-clock guard. CI runs 13,547 tests on 32 cores
with swarm tests dispatching 4 workers each, so 100 concurrent is reachable.

**This is a genuine product defect, not a CI artifact.** Any user running many
parallel agents on Windows hits the same O(N²) serialized stall.

### The fix

Take the directory sweep OFF the per-spawn hot path. `allocate_unique_profile`
does NOT depend on it — profile names are unique per (process-creation-time,
counter), so a stale lease does not block allocation. The sweep is hygiene
(reclaiming leaked profiles from dead processes, the F-28-02-002 DoS fix), not
a precondition.

Plan: gate the sweep on a **cross-process** TTL marker (~30s) so a spawn pays
one `stat` instead of an O(N) scan. Process-local suppression is NOT enough —
nextest gives every test its own process, so N processes would still sweep N
times.

**Trap to avoid:** the scan treats any non-`.toml` entry as a HARD ERROR that
aborts recovery (`acl_lease.rs:640`), with only `QUARANTINE_DIRECTORY`
allow-listed. A marker file placed INSIDE the lease directory must be
allow-listed — and doing so wedges any older build that meets it. **Put the
marker outside the lease directory** to avoid the downgrade hazard.

Keep: the sweep still runs under the lock when it does run; fail-closed
unchanged; F-28-02-002 recovery preserved (consider also sweeping on demand if
allocation ever fails).

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
