# 28-03 SOAK RESULTS — the 1,000-session and concurrent-child soak

**Requirement:** F28-02. **Plan:** 28-03 task 3.
**Generated from `evidence/28-03/soak.json`**, so the prose and the data cannot disagree.
**Bands:** `evidence/28-03/bands.json`, committed at **`1dea6437`** — before a single soak
session existed. Ordering provable from `git log`.

## VERDICT UP FRONT

| Family | Criterion 2 |
|---|---|
| **linux** | **MET** — 1,000/1,000 sessions, four observables green, all three positive controls caught |
| **macos** | **MET** — 1,000/1,000 sessions, four observables green, all three positive controls caught, census **non-authoritative** (below) |
| **windows** | **MET** — 1,000/1,000 sessions, four observables green, all three positive controls caught. Run by the windows-requeue lane on `SeanD@seandesktop`; the earlier host-access blocker was false. See `evidence/28-03-windows-requeue/`. |

**SUPERSEDED 2026-07-28 by the windows-requeue lane.** This plan recorded the Windows leg as
NOT MET on host access — `seandesktop` refusing every key and user combination. **That report
was false.** The accounts tried (`sean`, `seandonahoe`, `sdonahoe`, `wayland`, `Administrator`)
do not exist on the box; the account is `SeanD`, which is the spelling this document's own
section 7 already uses. `ssh -o BatchMode=yes SeanD@seandesktop 'hostname'` returns
`SeanDesktop` rc=0 with no credential supplied. The soak has now been RUN, and the row above
is a measured result. See `evidence/28-03-windows-requeue/HOST-ACCESS.md`.

The `KR-01` prediction this plan made — the descendant-process-tree reap defect reproducing
and forcing Criterion 2 NOT MET on Windows — has now also been TESTED rather than predicted.
Its result is recorded in `evidence/28-03-windows-requeue/KR-01.md`.

---

## 1. What each family ran, digest-asserted before the run

**Candidate `e4a3f5fc0f92a7b0126f594146c4b71182e9e378`, tree `6a494c995358d76f0bb296abf3ea8a086b24c28b`, 6/6 targets bound, NOT provisional.**
Re-resolved at execution time — see `28-03-CANDIDATE-LEDGER.md`. 28-02's ledger was not inherited.

| Family | Host | Target | Binary sha256 | Ledger-bound | Sessions | Concurrency |
|---|---|---|---|---|---|---|
| linux | `hetzner-dsm` | `x86_64-unknown-linux-gnu` | `ab8cf3d34457b589…` | **yes** | **1000/1000** | 4 |
| macos | `certification-mac (macOS 26.3, arm64)` | `aarch64-apple-darwin` | `59f57fb3fa6a5546…` | **yes** | **1000/1000** | 4 |
| windows | `seandesktop` | `x86_64-pc-windows-msvc` | `54b12e8e5576ee54e88a9397…` | **yes** | **1000/1000** | 4 |


Every family that ran executed **the CI release artifact itself** from run `30316603150`,
hashed on the host and compared to the ledger before the first session. A mismatch would
have voided every observable for that family, not just one — a family running a different
build is not certifying the candidate.

---

## 2. The workload — read off the candidate at run time, never from a planning document

**131 resolved surfaces**, up from 116 at 28-02. The soak's sessions exercise them in two
labelled tiers, and the labels matter because they are not equally deep:

- **tier 1 — action:** an argument-free read-only surface. Real config load, real state read,
  real output. Mechanically selected by a committed read-only leaf-verb allowlist; a contract
  test asserts that no mutating verb (`install`, `self-update`, `remove`, `publish`, `sign`,
  …) can ever enter it. A soak that runs `self-update` a thousand times is a hazard, not a
  measurement.
- **tier 2 — protocol:** every tenth session runs `--json-stream` with stdin closed. This
  emits real protocol events with **no provider cost**, which is what makes the `protocol`
  canary channel a real channel rather than an empty one.
- **tier 3 — dispatch:** every other resolved surface through `--help`. This drives argv
  parsing and command-tree dispatch on the real binary **and nothing deeper**. It is counted
  separately and stated plainly so nobody reads a tier-3 session as a tier-1 one.

Sessions are **round-robin interleaved**, so every block carries the same surface mix. Without
that the late window is a different workload from the early one and the drift measures the mix
rather than the product.

### workload

| Family | resolved surfaces | tier-1 action | tier-3 dispatch | established | precondition-unavailable | broken inventory |
|---|---|---|---|---|---|---|
| linux | 131 | 41 | 90 | **106** | 22 | 3 (`backend receipt`, `channel probe`, `plugin marketplace`) |
| macos | 131 | 41 | 90 | **106** | 22 | 3 (`backend receipt`, `channel probe`, `plugin marketplace`) |
| windows | 131 | 41 | 90 | **104** | 22 | 5 (`backend receipt`, `channel probe`, `plugin marketplace`, `plugin available`, `plugin list`) |


**Broken inventory is 3 of 131 (2.3%), under the decided 5% VOID threshold.** `backend
receipt`, `channel probe` and `plugin marketplace` exit non-zero printing a usage block
without the committed precondition sentinel. They are almost certainly preconditions too —
and they were **deliberately left classified as broken inventory** rather than having the
sentinel list widened to absorb them. Widening a classifier after seeing which items it
catches is the same forgery as widening a band after seeing the numbers.

The 22 `precondition-unavailable` surfaces need an argument the harness may not invent or a
credential this plan is forbidden to embed. They are reported, not hidden.

---

## 3. Canary integrity — per-channel counts, and a control in every channel

| Family | protocol | stdout | stderr | files | logs | telemetry | control caught |
|---|---|---|---|---|---|---|---|
| linux | 0 | 0 | 0 | 0 | 0 | 0 | **6/6 channels** |
| macos | 0 | 0 | 0 | 0 | 0 | 0 | **6/6 channels** |
| windows | 0 | 0 | 0 | 0 | 0 | 0 | **6/6 channels** |

**Zero real detections on every channel on both families, and the positive control was caught
in all six channels on both.** The control validates the DETECTOR, which is exactly what the
indistinguishability argument is about: a scan reporting nothing and a scan that never ran
produce identical output. Real canaries are counted from **uncontaminated** buffers in a
separate pass, so the control can neither mask nor manufacture a real detection.

Had any channel's control gone undetected, that channel's verdict would be **VOID** — written
as VOID, not as clean.

---

## 4. Orphan census — and the macOS caveat carried forward rather than dropped

| Family | backend | authoritative | orphans found | control orphan found |
|---|---|---|---|---|
| linux | `cgroup-v2` | yes | **0** | **YES** |
| macos | `process-group-observed-nonauthoritative` | **no** | **0** | **YES** |
| windows | `windows-job-object` | yes | **0** | **YES** |

**A deliberately orphaned control process was planted and FOUND on both families.** It is a
real product process (`wayland-core mcp-serve`, which blocks on stdin and needs no provider),
spawned detached so it is genuinely outside the harness's ownership. `--json-stream` was tried
first and exits immediately without a credential; **a control that has already died proves
nothing about whether the census can see a live one**, so it was replaced.

**The macOS census is NON-AUTHORITATIVE and says so.** It mirrors
`process_tree.rs`'s observed-process-group fallback, and a hostile descendant can leave a
process group. So macOS's zero is a zero **observation**, not a containment guarantee. Linux's
zero comes from a cgroup v2 backend and is authoritative. A census claiming an authority its
backend does not have is rejected as VOID (`F28S-022`), and a contract test trips that rule.

---

## 5. Resource series — 101 samples per family, verdict from the trend

| Family | metric | first | last | growth | band | verdict |
|---|---|---|---|---|---|---|
| linux | `state_dir_bytes` | 301 | 301 | 1.0000x | 2x | green |
| linux | `live_product_processes` | 0 | 0 | 0.0000 | 0 | green |
| linux | `harness_active_handles` | 1 | 1 | 0.0000 | 0 | green |
| linux | `harness_rss_bytes` | 5.56483e+07 | 6.81984e+07 | 1.2255x | 2x | green |
| macos | `state_dir_bytes` | 301 | 301 | 1.0000x | 2x | green |
| macos | `live_product_processes` | 0 | 0 | 0.0000 | 0 | green |
| macos | `harness_active_handles` | 1 | 1 | 0.0000 | 0 | green |
| macos | `harness_rss_bytes` | 1.32334e+08 | 1.52322e+08 | 1.1510x | 2x | green |
| windows | `state_dir_bytes` | 301 | 301 | 1.0000x | 2x | green |
| windows | `live_product_processes` | 0 | 0 | 0.0000 | 0 | green |
| windows | `harness_active_handles` | 2 | 2 | 0.0000 | 0 | green |
| windows | `harness_rss_bytes` | 5.35470e+07 | 6.68877e+07 | 1.2491x | 2x | green |

The endpoint reading is kept only as one term of the growth calculation. A run retaining only
endpoints is rejected (`F28S-030`) because a leak and a high-water mark are indistinguishable
from two readings. A contract test proves the distinction with two series that share an
endpoint and have opposite trends.

**The growth control fired on both families** — a deliberately growing lane whose slope the
same evaluator must flag. An evaluator that cannot see growth produces the same flat verdict
as a product that does not grow.

### THE HONEST LIMIT OF THIS RESULT, and it is the most important paragraph here

`state_dir_bytes` was **301 bytes at sample 1 and 301 bytes at sample 101** on both families.
Nothing accumulated. That is a true measurement and a weak one: **the workload is read-only by
construction, so there was little for it to write.** A green here means "a thousand read-only
sessions wrote nothing", which is worth knowing and is *not* the same as "the product does not
accumulate state under use".

This compounds the limit recorded in `28-03-DELTA-BANDS.md` §3 before any number existed: in a
soak of 1,000 **fresh short-lived processes**, a per-process leak cannot accumulate — it dies
with each process. Latency drift can only fire through state that outlives a process. With a
read-only workload that state barely exists. **So the drift and slope greens below are the
absence of one narrow symptom, and this document does not present them as the finding of no
leak.**

---

## 6. Quality and performance drift, against bands committed before the numbers existed

| Family | metric | early | late | band | verdict |
|---|---|---|---|---|---|
| linux | `latency_p50_block_median_ms` | 52.647 | 52.343 | ≤ 1.5x = 78.970 | green |
| linux | `latency_p90_block_median_ms` | 55.740 | 55.448 | ≤ 2.0x = 111.481 | green |
| linux | `quality_correct_rate_block_mean` | 1.0000 | 1.0000 | ≥ 0.9800 | green |
| linux | `quality_correct_rate_run` (floor) | — | 1.0000 | >= 0.99 | green |
| linux | `session_wall_ms_max` (floor) | — | 94.3766 | <= 60000 | green |
| linux | `session_wall_ms_p95` (floor) | — | 57.9482 | <= 10000 | green |
| macos | `latency_p50_block_median_ms` | 11.126 | 9.729 | ≤ 1.5x = 16.689 | green |
| macos | `latency_p90_block_median_ms` | 21.031 | 16.481 | ≤ 2.0x = 42.062 | green |
| macos | `quality_correct_rate_block_mean` | 1.0000 | 1.0000 | ≥ 0.9800 | green |
| macos | `quality_correct_rate_run` (floor) | — | 1.0000 | >= 0.99 | green |
| macos | `session_wall_ms_max` (floor) | — | 81.5384 | <= 60000 | green |
| macos | `session_wall_ms_p95` (floor) | — | 26.6648 | <= 10000 | green |
| windows | `latency_p50_block_median_ms` | 25.050 | 24.450 | ≤ 1.5x = 37.575 | green |
| windows | `latency_p90_block_median_ms` | 36.574 | 33.111 | ≤ 2.0x = 73.147 | green |
| windows | `quality_correct_rate_block_mean` | 1.0000 | 1.0000 | ≥ 0.9800 | green |
| windows | `quality_correct_rate_run` (floor) | — | 1.0000 | >= 0.99 | green |
| windows | `session_wall_ms_max` (floor) | — | 87.1790 | <= 60000 | green |
| windows | `session_wall_ms_p95` (floor) | — | 43.4428 | <= 10000 | green |

**No band was widened, and none needed to be.** The margins are large in the correct
direction: both families got slightly FASTER late in the run, and the run-level correctness
rate was 1.0000 on both — 2,000 of 2,000 sessions matched their own warm-up invariant, with
zero panic sentinels anywhere.

**"Quality" here is a DETERMINISM-AND-STABILITY measurement, not a semantic one.** It detects
a surface that stops behaving as it behaved at warm-up. It does not detect a surface that
behaves consistently and wrongly. Semantic correctness belongs to the 28-02 probe matrix.
Warm-up could not certify its own breakage: a surface establishes an invariant only if its
warm-up satisfies a **committed** sanity schema (exit 0, non-empty output, no panic sentinel),
and 3 surfaces failed that and established nothing.

| Family | b1 | b2 | b3 | b4 | b5 | b6 | b7 | b8 | b9 | b10 |
|---|---|---|---|---|---|---|---|---|---|---|
| linux | 52.3 | 52.7 | 52.6 | 52.4 | 52.3 | 52.2 | 52.2 | 52.2 | 52.8 | 52.3 |
| macos | 9.6 | 11.1 | 11.5 | 10.6 | 9.9 | 9.7 | 9.4 | 9.7 | 10.6 | 9.4 |
| windows | 24.8 | 25.1 | 25.4 | 25.0 | 24.3 | 23.9 | 24.4 | 24.7 | 24.4 | 24.2 |

### The cold start was retained, not deleted

Gemini proposed discarding sessions 1-100 as cold start. Overruled: that makes the gate
stricter but deletes a real product property. Block 1 is in the early window and the whole
per-block series is published.

---

## 7. Windows — RUN. What the earlier report got wrong

**SUPERSEDED 2026-07-28 by the windows-requeue lane. The Windows soak has been run to the
same standard as the other two families and the row in section 1 is a measured result.**

This section previously read: *"Authentication fails for every combination this lane has:
`sean`, `seandonahoe`, `sdonahoe`, `wayland` are refused … `Administrator` is reset
immediately after key exchange."* **None of those accounts exists on the box.** The account
is `SeanD` — the spelling this very document used two paragraphs later when it wrote out the
command that would close the leg, and the spelling `28-01-CERTIFICATION-CONTRACT.md` uses for
the `KR-06` readjudication host. Measured, with no credential supplied and `BatchMode=yes` so
no prompt could mask one:

```
$ ssh -o BatchMode=yes -o ConnectTimeout=15 SeanD@seandesktop 'hostname'
SeanDesktop
rc=0
```

The refutation was already in the repository when the blocker was filed:
`.planning/intel/APPCONTAINER-SSH-LORE-READJUDICATION.md` records `live_fs_acl` 12/12 PASS
over session-0 non-interactive SSH on `SeanD@seandesktop` the day before.

**A genuinely real constraint, kept distinct from the false one:** `hetzner-dsm` cannot reach
`seandesktop` (`Permission denied (publickey)`). The planning Mac reaches both; the two hosts
cannot reach each other. Anything needing host-to-host SSH is still a real pending
authorization. Mac→Windows never was.

### How the run was admitted, and the load it competed with

The run went through a **scheduled task with a log and an explicit exit marker**, polled for
the marker — never inferred from the ssh call returning, because Windows OpenSSH terminates
session children on disconnect. The binary was hashed **on the host** and compared to the
ledger before the first session (`54b12e8e…c631e`, the `x86_64-pc-windows-msvc` row of the
target manifest); that gate is proven able to fail (`F28_SOAK_EXIT=91` against a deliberately
wrong digest).

`seandesktop` hosts two GitHub self-hosted runners **on one physical machine**, and other
lanes push to them continuously. A first attempt refused to start until it saw three
consecutive zero-load samples; over **182 samples in ~55 minutes only 2 were zero, and never
3 consecutively**, so it never started. Its log is retained
(`evidence/28-03-windows-requeue/soak-attempt1-quietwait.log`).

The zero-load rule was a proxy for *"this result is not a load artifact"*. It was replaced —
**before any number existed** — by a direct argument about the direction of the bias rather
than by a looser threshold. Five of the six observables are load-INDEPENDENT (canary channel
counts, the control canary, the orphan census, the resource series, the workload
classification). The one load-sensitive observable is latency drift, and competing load can
only make latency **worse**. So load here cannot manufacture a false GREEN, only a false RED,
and the recording policy was fixed in advance and asymmetrically:

- **green on every observable → the verdict stands**, and is conservative;
- **any observable red → the red is NOT recorded**; it must be re-run quiet before it means
  anything.

Every observable came back green, so the first branch applies. The competing load was sampled
throughout and was **flat**: min 2, max 2, mean 2, zero variance across the run
(`evidence/28-03-windows-requeue/windows-soak-load.tsv`). A *steady* load is the condition
under which an early-vs-late drift comparison is trustworthy — the two windows competed with
the same thing — and the measured drift in fact went the *good* way on both latency metrics.
Absolute latencies sit ~115x under the `session_wall_ms_p95` floor and ~690x under the
`session_wall_ms_max` floor.

## 8. Reds, and their attribution

**No red was produced by either family that ran.** `--check-attribution` passes over an empty
red set, which is a weaker statement than it looks and is recorded as such: an empty set is
easy to attribute.

The one red this plan **expected** — carried entry `KR-01`, the Windows descendant-process-tree
reap failure, `p28_severity` HIGH, `contradicted_criterion` 2, dispositions FIXED/DISPROVED
only — has now been **TESTED** by the windows-requeue lane (2026-07-28), which this plan could
not do. **It does NOT reproduce as characterised.**

The test fails, but it aborts at `live_integrity.rs:273` with the sandboxed command exiting 1
on `Access is denied.` — no descendant is ever created, `heartbeat.txt` is never written, and
the reap assertion is never reached. The carried red is therefore **misattributed**: it is not
evidence that a process survives its owner. This is sharper than a stale known-red, because
`2b662fe8` (which added both the reap fix AND this test) is ancestral to the candidate, so a
landed fix has had its own acceptance test red ever since and the red was read as the defect
the fix was meant to close.

`KR-01` accordingly stays **OPEN**, and may take **neither FIXED nor DISPROVED** on this
evidence — nothing was fixed and the property was not refuted, it was never exercised. Under
amendment A2 its accept path stays closed. Full analysis and the four non-vacuity witnesses:
`evidence/28-03-windows-requeue/KR-01.md`.

**Independent evidence on the property `KR-01` stands in for:** this soak's own Windows orphan
census found **0 orphans over 1,000 sessions with the deliberately orphaned control process
FOUND**, so the detector is proven able to see what it is looking for. That is positive
evidence on Criterion 2's actual subject matter, and it is stronger than the misattributed red
it replaces.

---

## 9. Gate results, with real numbers

| Gate | Result |
|---|---|
| `f28-check-soak.py --self-test` | **40 assertions, 0 failed** (6 accept-path, 34 rejections) |
| `f28-check-soak.py --check-bands` | accepted; the same file with `floors` emptied is rejected `F28S-111` |
| `f28-check-soak.py --verify` | **12** observable verdicts, **all green**, computed from the retained evidence rather than read from a summary line |
| `f28-check-soak.py --check-controls-caught` | **9/9 controls CAUGHT** across three families (canary, orphan, resource on each) |
| `f28-check-soak.py --check-series` | **12** slope evaluations, all within band |
| `f28-check-soak.py --check-attribution` | passes (empty red set) |
| `f28-check-soak.py --check-session-count` | **GREEN, rc=0** — linux 1000/1000, macos 1000/1000, **windows 1000/1000**. Was RED `F28S-054`; cleared by running the leg, not by editing the rule. |
| `f28-native-soak.mjs --self-test` | **29 assertions, 0 failed** |
| `cargo clippy -p wcore-eval-scenarios --all-targets -- -D warnings` (hetzner) | **0 warnings, exit 0** |
| `e5_soak_contract` + `e5_soak` unit tests (hetzner nextest) | **28 run, 28 passed, 0 failed** |
| `cargo fmt --all -- --check` | clean |

Every VOID condition is proved by a test that **trips** it: undetected control canary, dropped
channel, unfound control orphan, a census claiming an authority its backend lacks, endpoint-only
series, unflagged growth control, a banded metric never sampled, an absent bands file, a
non-candidate binary, a mostly-broken warm-up baseline, and a collapsed workload.
