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
| **windows** | **NOT MET — the soak did not run at all.** `seandesktop` refused every key and user combination available to this lane. Nothing was measured, so nothing is claimed. |

**The Windows leg is a NOT MET caused by host access, not by a product result.** This plan
predicted a different Windows failure — the carried `KR-01` descendant-process-tree reap
defect reproducing under soak conditions and forcing Criterion 2 NOT MET on that platform.
**That prediction was never tested.** Saying so is the point: an untested prediction and a
confirmed one are as different as a clean scan and an absent scanner, which is the rule this
whole plan is built around.

---

## 1. What each family ran, digest-asserted before the run

**Candidate `e4a3f5fc0f92a7b0126f594146c4b71182e9e378`, tree `6a494c995358d76f0bb296abf3ea8a086b24c28b`, 6/6 targets bound, NOT provisional.**
Re-resolved at execution time — see `28-03-CANDIDATE-LEDGER.md`. 28-02's ledger was not inherited.

| Family | Host | Target | Binary sha256 | Ledger-bound | Sessions | Concurrency |
|---|---|---|---|---|---|---|
| linux | `hetzner-dsm` | `x86_64-unknown-linux-gnu` | `ab8cf3d34457b589…` | **yes** | **1000/1000** | 4 |
| macos | `certification-mac (macOS 26.3, arm64)` | `aarch64-apple-darwin` | `59f57fb3fa6a5546…` | **yes** | **1000/1000** | 4 |
| windows | `seandesktop` | `x86_64-pc-windows-msvc` | — | — | **0/1000 NOT RUN** | — |


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

### The cold start was retained, not deleted

Gemini proposed discarding sessions 1-100 as cold start. Overruled: that makes the gate
stricter but deletes a real product property. Block 1 is in the early window and the whole
per-block series is published.

---

## 7. Windows — what was attempted, and what it would take to close

`seandesktop` is reachable at the network layer (`100.109.207.54:22`, `OpenSSH_for_Windows_9.5`
answers and completes key exchange). **Authentication fails for every combination this lane
has:** `sean`, `seandonahoe`, `sdonahoe`, `wayland` are refused
`(publickey,password,keyboard-interactive)` with both the default `id_ed25519` and the
`wayland_win` identity; `Administrator` is reset immediately after key exchange. The ssh agent
holds no identities.

**Supplying a credential is reserved to Sean.** This lane did not attempt to obtain one, did
not guess further, and did not substitute a different host or a smaller run.

**What would close it:** one quiet scheduled-task run of `scripts/f28-native-soak.mjs` on
`seandesktop` against the digest-bound artifact
`54b12e8e5576ee54e88a93975c360e6c624202059f449d80574b71adf00c631e`, logging to a file with an
exit marker and polled for it — never inferred from the ssh call returning. Nothing else may
run on that box during it: the two registered runners are one physical machine, and a red
produced under concurrent load is a load artifact rather than a recordable red.

**The session-count gate is RED and that is correct.** `f28-check-soak.py
--check-session-count` exits 1 with `F28S-054: family NOT RUN — Criterion 2 is NOT MET for:
windows: 0/1000`. The rule was added so that "the families we ran all passed" can never read
as "the soak passed".

---

## 8. Reds, and their attribution

**No red was produced by either family that ran.** `--check-attribution` passes over an empty
red set, which is a weaker statement than it looks and is recorded as such: an empty set is
easy to attribute.

The one red this plan **expected** — carried entry `KR-01`, the Windows descendant-process-tree
reap failure, `p28_severity` HIGH, `contradicted_criterion` 2, dispositions FIXED/DISPROVED
only — **was not reproduced, because the platform that carries it was not run.** It remains
OPEN and untested by this plan. Under amendment A2 its accept path stays closed.

---

## 9. Gate results, with real numbers

| Gate | Result |
|---|---|
| `f28-check-soak.py --self-test` | **40 assertions, 0 failed** (6 accept-path, 34 rejections) |
| `f28-check-soak.py --check-bands` | accepted; the same file with `floors` emptied is rejected `F28S-111` |
| `f28-check-soak.py --verify` | 8 observable verdicts, **all green**, computed from the retained evidence rather than read from a summary line |
| `f28-check-soak.py --check-controls-caught` | **6/6 controls CAUGHT** across two families |
| `f28-check-soak.py --check-series` | 8 slope evaluations, all within band |
| `f28-check-soak.py --check-attribution` | passes (empty red set) |
| `f28-check-soak.py --check-session-count` | **RED, `F28S-054`** — windows NOT RUN |
| `f28-native-soak.mjs --self-test` | **29 assertions, 0 failed** |
| `cargo clippy -p wcore-eval-scenarios --all-targets -- -D warnings` (hetzner) | **0 warnings, exit 0** |
| `e5_soak_contract` + `e5_soak` unit tests (hetzner nextest) | **28 run, 28 passed, 0 failed** |
| `cargo fmt --all -- --check` | clean |

Every VOID condition is proved by a test that **trips** it: undetected control canary, dropped
channel, unfound control orphan, a census claiming an authority its backend lacks, endpoint-only
series, unflagged growth control, a banded metric never sampled, an absent bands file, a
non-candidate binary, a mostly-broken warm-up baseline, and a collapsed workload.
