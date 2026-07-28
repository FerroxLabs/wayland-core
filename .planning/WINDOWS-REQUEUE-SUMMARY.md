# Windows requeue lane — four legs that were graded NOT ACHIEVED on a blocker that was false

**Branch:** `lane/windows-requeue`, off `plan/f20-unified-audit-repair` at `c743f398`.

Four Windows legs across three phases were recorded **NOT ACHIEVED, blocked on a Sean-reserved
credential**. That report was false. This lane ran them.

---

## The correction, first, because it is the reason the lane exists

Three separate lanes reported `seandesktop` unreachable after trying `sean`, `seandonahoe`,
`sdonahoe`, `wayland` and `Administrator`. **None of those accounts exists on the box.**

```
$ ssh -o BatchMode=yes -o ConnectTimeout=15 SeanD@seandesktop 'hostname'
SeanDesktop
rc=0
```

`BatchMode=yes`, so no prompt could have masked a credential being supplied. **None was
supplied, obtained or guessed.** The existing key already authorises the account, and `SeanD`
is the spelling every plan in this program already uses — including 28-03's own closure note
and 28-01's `KR-06` row. The refutation was already committed in the repo when the blocker was
filed: `.planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md` records `live_fs_acl` 12/12
PASS over session-0 SSH on `SeanD@seandesktop` the day before.

**The genuinely real constraint, kept distinct:** `hetzner-dsm` cannot reach `seandesktop`
(`Permission denied (publickey)`). The Mac reaches both hosts; the two hosts cannot reach each
other. Anything requiring host-to-host SSH remains a real pending authorization. Mac→Windows
never was.

---

## Per leg: what ran, the numbers, the disposition

### Leg 1 — Phase 28 Criterion 2, the Windows soak. **MET.**

Ran to the same standard as Linux and macOS, against the same re-resolved candidate
`e4a3f5fc` / tree `6a494c99`, binary hashed **on the host** before the first session and equal
to the ledger's `x86_64-pc-windows-msvc` digest `54b12e8e…c631e`.

**1000/1000 sessions at concurrency 4.** Every gate, run on the merged record:

| Gate | Before | After |
|---|---|---|
| `--check-session-count` | **RED `F28S-054`** windows 0/1000 | **rc=0** — linux 1000/1000, macos 1000/1000, windows 1000/1000 |
| `--check-controls-caught` | 6/6 over two families | **9/9 CAUGHT** over three |
| `--verify` | 8 verdicts | **12 verdicts, all green** |
| `--check-series` | 8 slopes | **12 slopes, all within band** |
| `--check-attribution` | rc=0 | rc=0 |

Windows observables: canary **0 on all six channels** with the control caught in **all six**;
census `windows-job-object`, **0 orphans, control orphan FOUND**; 101 resource samples, rss
growth 1.2491x against a 2x band; drift p50 **25.050 → 24.450** and p90 **36.574 → 33.111**
(both improving), run-level correctness **1.0000 over 1000 sessions**, `session_wall_ms_p95`
43.44 ms against a 10,000 ms floor.

The `F28S-054` red was cleared **by running the leg, not by editing the rule**.

### Leg 2 — `KR-01`. **Tested. Does NOT reproduce as characterised.** HIGH finding.

The carried HIGH row says *"descendant process tree is not reaped; a process survives its
owner"*. The test does fail — but it aborts at `live_integrity.rs:273` with the sandboxed
command exiting 1 on **`Access is denied.`**, so no descendant is ever created and the reap
assertion at ~300 is never reached. `heartbeat.txt` is never written; 0 `choice.exe` survive.

**This is worse than a stale known-red.** `2b662fe8` (Jul 14) is **ancestral** and added *both*
the reap fix *and* this test. So a landed fix has had its own acceptance test red ever since,
with the red attributed to the very defect the fix was meant to close.

`rc=101` reads exactly like "KR-01 reproduces". **The wall clock is what stopped this lane
filing that finding:** 0.53 s against a body that sleeps 10+2+2 s. The harness graded it
`UNREADABLE` rather than recording the headline.

Not a general sandbox failure — `live_integrity` 4/5 pass and `live_fs_acl` passes **12/12 in
16.37 s**. Not load — reproduced deterministically under two accounts at loads 33 and 19.

Full analysis: `28-native-cross-platform-certification/evidence/28-03-windows-requeue/KR-01.md`.

### Leg 3 — 26-02's Windows quarantine leg. **MET.**

The **same paired live construction as Linux**, not a weaker path check: the real binary driven
through a real agent turn, negative leg asserting the Skill tool **ran and reported the skill
unavailable**, positive control same payload same turn differing only by `migrate promote`.

```
test t19_live_negative_leg_quarantined_payload_does_not_execute        ... ok
test t20_live_positive_control_same_payload_executes_once_promoted     ... ok
test result: ok. 26 passed; 0 failed; 0 ignored; finished in 141.82s
```

`t20` firing is the load-bearing half — it makes `t19`'s absent sentinel *containment* rather
than a payload that never loads. Its precondition was measured **before** the run: the payload's
`touch` directive is not a cmd builtin, and had it not resolved, a `t20` failure would have
meant "the fixture cannot run here", not "containment leaked".

29-vs-26 count delta against Linux is **3 Unix-only PTY support self-tests**; all 22 authored
tests `t1`–`t22` pass on both families.

### Leg 4 — 29-03's Windows downgrade refusal. **MET.**

Same construction as Linux: through the shipped binary, against the **real public GitHub API**,
**no update-source redirect**, **no credential**, rebuilt at `0.99.0` so `v0.12.25` is a
downgrade.

```
wayland-core 0.99.0
current: v0.99.0   latest: v0.12.25
REFUSED: the offered release v0.12.25 is OLDER than the running v0.99.0. ... Nothing was installed.
check-only rc=0     install rc=1     version after: 0.99.0 (did not swap itself)
```

**Identical to Linux on every clause** — Linux's `check-rc=0` / `install-rc=1` reproduced
exactly. `F29-LIMIT-06` is closed; it was never a real-credential limit. The other five
`F29-LIMIT-*` rows are untouched and remain open — they need Sean's real release keys or a real
published signed release.

---

## Findings

| ID | Severity | Finding |
|---|---|---|
| **F-WR-01** | **HIGH** | `KR-01` is **misattributed**. Its test aborts on an access failure in its own setup and never evaluates descendant reaping, so a HIGH row has been held against Criterion 2 on evidence that does not support it. It may take neither FIXED nor DISPROVED on this evidence; the reap property under that scenario is **unproven**. |
| **F-WR-02** | **HIGH** | `cargo test -p wcore-sandbox --test live_fs_acl` exits **0** and prints **`test result: ok`** while running **0 of 12** tests — all `#[ignore]`d behind a gate needing `-- --ignored`. The obvious command yields a green that proves nothing, on the suite certifying the sandbox filesystem boundary. |
| **F-WR-03** | **MEDIUM** | Execute-from-an-fs-granted-directory is exercised by exactly one test, the failing one, which also uses `%PUBLIC%` — a placement no passing test uses. That path is uncovered by any green. |
| **F-WR-04** | **MEDIUM** | **~600 leaked `wcoresandbox-<pid>` AppContainer profiles** under `%LOCALAPPDATA%\Packages`, plus **20 leaked `wcore-job-cancel-*` work directories** under `C:\Users\Public` (removed only on success, so the count is a census of historical failures). |
| **F-WR-05** | **LOW** | Running the sandbox as `SYSTEM` trips `validate_mutex_security`: `acquire()` always builds a 2-entry DACL while the validator expects 1 when the caller's SID *is* SYSTEM. Fails closed with a message that reads like a platform limitation — the `KR-05` pattern. Not the shipping configuration. |
| **F-WR-06** | **MEDIUM** | Every non-zero exit status **collapses to 1** over `ssh … powershell -EncodedCommand` (measured: 2, 3, 7, 100, 255 all arrive as 1). Any Windows gate asserting a *specific* exit code over ssh is asserting a value that cannot arrive. |

---

## Measurement traps this lane walked into and caught

Recorded because each would have produced a false report, and two of them nearly did.

1. **A fabricated HIGH, avoided.** A PowerShell `Start-Process` probe reported `--json-stream`
   hanging with **zero output** on Windows — which reads as a product defect. The probe was
   wrong: it redirected stdin from a file it created *after* the process started. Re-probed
   through the harness's own `spawn(...)` + `stdin.end()` mechanism, the surface exits in
   **57 ms with 496 bytes of protocol JSON**. A probe that does not use the harness's mechanism
   is not measuring what the harness measures.
2. **`rc=101` that was not a reproduction** — leg 2, caught by elapsed time (0.53 s).
3. **`Q_PAIRED_SECONDS=0.08`** — `cargo test` rejects a second positional filter, so the paired
   invocation never ran. Caught by the same too-fast signal.
4. **A version bump that silently did nothing** — leg 4 attempt 1 rewrote the first
   `version = "..."` in `crates/wcore-cli/Cargo.toml`, which declares `version.workspace = true`
   and has no such line. It would have built a **0.12.25** binary, for which the newest release
   is the *same* version, not a downgrade — the refusal under test would never have been
   exercised. Caught because `D_BASELINE_VERSION_LINE` came back **empty**, and a field that
   cannot legitimately be empty was treated as a failure rather than rendered as blank.
5. **`live_fs_acl` green on 0 of 12 tests** — F-WR-02.

---

## On the quiet-window rule, and why it changed

`seandesktop` hosts two GitHub self-hosted runners **on one physical box**, and other lanes push
to them continuously. The soak's first attempt refused to start until three consecutive
zero-load samples: over **182 samples in ~55 minutes only 2 were zero, and never 3
consecutively**. Its log is retained.

The zero-load rule was a *proxy* for "this is not a load artifact". It was replaced —
**before any number existed** — by a direct argument about the direction of the bias, not a
looser threshold. Five of the six observables are load-independent; the one that is not is
latency drift, and load can only make latency **worse**. So load here cannot manufacture a
false green, only a false red. The policy was fixed in advance and asymmetrically: **green
stands; any red would not be recorded without a quiet re-run.** Every observable came back
green, and the load was **flat at min=max=mean=2 with zero variance** — a steady load being
exactly the condition under which an early-vs-late drift comparison is trustworthy.

For `KR-01` the asymmetry runs the **other** way — a starved-but-alive descendant could miss
its sampling windows and be scored as reaped — so that leg carried an independent
survivor witness (`choice.exe`) that does not depend on file length at all.

---

## State left behind

- **`C:\ferrox-win-23B04` untouched.** Asserted rather than assumed: its `LastWriteTime` is
  `2026-07-27 22:42:18`, which predates this lane's first action. The live multi-day journey
  bound to those binaries' provenance until 2026-07-30T23:54:26Z is intact. `/root/wayland-p28-03*`,
  `/root/wayland-29-03` and `/root/wayland-f26-02` were never touched — this lane used hetzner not at all.
- **All six of this lane's scheduled tasks unregistered**; `LEFTOVER_TASKS=0`.
- This lane's own tree is `C:\wl-winrequeue` (source at `C:\wl-winrequeue\src`, cloned
  read-only from `C:\ferrox-win`, never mutating it). Its root `Cargo.toml` version is
  restored to `0.12.25`, verified after the run.
- **This lane changed no source file.** `git diff --name-only <merge-base> HEAD` outside
  `.planning/` is empty, and both shared-fence files (`crates/wcore-cli/src/{lib,main}.rs`) are
  untouched. Diffed against the captured merge-base SHA `c743f398`, never the branch name.

## What remains genuinely unrunnable, and why

- **`hetzner-dsm` → `seandesktop` SSH.** Real, measured (`Permission denied (publickey)`), and a
  separate pending authorization with Sean. Nothing in this lane needed it — the Mac reaches
  both hosts.
- **`F29-LIMIT-01..05`** (29-03): Sean's real release trust root, a published signed release
  manifest, the runtime plugin trust root, the `gh attestation verify` accept path, and an
  end-to-end install of a real signed artifact. All genuinely need inputs this lane may not
  supply. Untouched and still open.
- **The `KR-01` reap property itself.** Its scenario cannot currently run (F-WR-01/F-WR-03), so
  whether descendants are reaped on future-drop is **unproven** — not proven, and not refuted.
  Closing it needs the test's own setup repaired first.

## To serialize

- `seandesktop` is **one physical box** hosting two GitHub runners. Any lane taking a Windows
  measurement should record competing load alongside it, and state which direction load biases
  that particular measurement — it is not the same direction for every leg (it biased the soak
  toward a false red and `KR-01` toward a false green).
- No protocol seam, contract request or shared-file edit is produced by this lane.
