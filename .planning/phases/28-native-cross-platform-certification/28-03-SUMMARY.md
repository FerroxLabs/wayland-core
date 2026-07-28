---
phase: 28-native-cross-platform-certification
plan: "03"
subsystem: certification
tags: [F28-02, soak, delta-bands, cross-audit, positive-control, candidate-resolution]
requires: ["28-01", "28-02"]
provides:
  - "evidence/28-03/bands.json — the decided delta bands, committed before the soak"
  - "evidence/28-03/candidate.json — the re-resolved candidate at e4a3f5fc, 6/6 bound"
  - "evidence/28-03/soak.json — the per-family soak record plan 04 consumes"
  - ".planning/scripts/f28-check-soak.py — the soak validator plan 04 also gates on"
  - "crates/wcore-eval-scenarios/src/e5_soak.rs — canonical soak definitions and VOID rules"
affects: ["28-04"]
tech-stack:
  added: []
  patterns: ["canonical-definition-in-rust + black-box-executor-in-node", "positive-control-per-observable", "pre-registered-bands"]
key-files:
  created:
    - .planning/phases/28-native-cross-platform-certification/28-03-DELTA-BANDS.md
    - .planning/phases/28-native-cross-platform-certification/28-03-SOAK-RESULTS.md
    - .planning/phases/28-native-cross-platform-certification/28-03-CANDIDATE-LEDGER.md
    - crates/wcore-eval-scenarios/src/e5_soak.rs
    - crates/wcore-eval-scenarios/tests/e5_soak_contract.rs
    - scripts/f28-native-soak.mjs
    - .planning/scripts/f28-check-soak.py
  modified:
    - crates/wcore-eval-scenarios/src/lib.rs
decisions:
  - "Delta bands = option C (drift within run + loose absolute floors), unanimous 4/4 cross-audit, committed at 1dea6437 before any soak session existed"
  - "The soak executor is Node, not a cargo test, because the certification Mac runs no cargo — same forced split as 28-02"
  - "A family that could not be run at all turns the session-count gate RED (F28S-054) rather than being omitted"
metrics:
  duration: "~1 session"
  completed: "2026-07-28"
status: complete
---

# Phase 28 Plan 03: Soak, Delta Bands and Re-resolution Summary

Decided the undefined "unacceptable delta" by unanimous four-way cross-audit and committed it
before any number existed; built a soak harness in which every observable carries a positive
control and a missed control is VOID rather than green; re-resolved the candidate at execution
time to `e4a3f5fc` with 6/6 digests bound; ran 1,000 sessions on Linux and macOS with all
controls caught; re-ran the macOS matrix and closed 28-02's stated exception at 216/216; and
reported Windows as **NOT MET, unmeasured** because the host refused every credential this lane
has.

**TERMINATION STATE: 2 — COMPLETE WITH STATED EXCEPTIONS.** The exception is Windows, it is
§5, and it is a host-access blocker rather than a product result.

---

## 1. The re-resolved candidate, and what changed against 28-02

**`e4a3f5fc0f92a7b0126f594146c4b71182e9e378`, tree `6a494c995358d76f0bb296abf3ea8a086b24c28b`,
6 of 6 per-target digests BOUND, `provisional: false`.**

28-02 certified `32e2f57d` and said in its own words that it covered `32e2f57d` and **not** the
tip. Eleven merges have landed since. The candidate was **re-resolved**, not inherited: a fresh
surface capture was taken off the candidate binary's own command tree, all six CI artifacts
from run `30316846446`/`30316603150` were downloaded and hashed here, and
`f28-resolve-candidate.py --verify` reports *commit bound to tree, 6 targets each with a digest
or a reason, 131 surfaces, 18 findings, 19 inputs unchanged*.

| | 28-02 (`32e2f57d`) | 28-03 (`e4a3f5fc`) |
|---|---|---|
| surfaces | 116 | **131** (+15, 0 removed) |
| targets bound | 6/6 | **6/6** |
| provisional | yes (tip was moving) | **no** |
| `claimed-but-absent` findings | 1 | **0** |

**The fifteen new surfaces are three new top-level verbs and their subcommands:** `channel`
(+4, phase 24), `goal` (+6, phase 22), `sandbox` (+2 — `sandbox status` and `sandbox exec`,
which is the surface that makes the macOS re-run possible at all).

**`F-28-01-R001` resolved itself, exactly as predicted.** `wayland-core channel` was
`claimed-but-absent` at `32e2f57d` because the phase-24 artifact claiming it had not merged when
28-02 measured. It is present now and **the claimed-but-absent class is empty.** `acp` cleared
the same way (was `attribution-weak`, now no finding). Two new **LOW** `attribution-weak`
findings appear for `goal` and `sandbox` — the resolver sees them discussed but not in a form it
recognises as a claim. Everything else is unchanged in class and severity; the `F-28-01-R0nn`
IDs shift because they are positional, which is worth knowing before anyone diffs them by ID.

---

## 2. The delta bands — decided by cross-audit BEFORE the soak, unanimously

Full document: `28-03-DELTA-BANDS.md`. Machine form: `evidence/28-03/bands.json`. Votes and
dissent: `28-03-decision-evidence/`.

### VERDICT: **OPTION C** — drift within the run + deliberately loose absolute floors. **4 of 4.**

| Member | Position | Confidence |
|---|---|---|
| codex (gpt-5.6-sol) | **C** | high |
| gemini (3.1-pro-preview) | **C** | high |
| kimi (K3) | **C** | medium |
| internal adversarial | **C** | medium |

**Committed at `1dea6437`; the soak record landed at `a0ca3ecf`. The ordering is provable from
`git log` and no band was widened afterwards.**

### Two vote-loss traps fired, and catching them is why the 4/4 can be trusted

Both are the self-passing-gate defect class — an invocation that returns cleanly while
contributing nothing:

- **gemini returned ZERO BYTES with `rc=1`** on the ~7 KB question through `-p "$Q"`.
  `--skip-trust` was present; the documented trap was not the one that fired. The same question
  on **stdin** returned a full answer.
- **codex produced NO ANSWER twice** — echoed the prompt, emitted MCP auth errors, exited 0 —
  including with stdin closed. Probed with a **short but real** question it answered
  immediately; re-asked with the question condensed to 2.8 KB it answered in full.

Every artifact was byte-counted and its position extracted **unanchored** (`grep -o
'PANEL_POSITION=[A-D]'`, last match) before the vote was counted. Without the byte count this
would have been recorded as a four-way audit while being a two-way one.

### The bands, and the limit recorded before any number existed

Drift is on **block aggregates** — the statistic per 100-session block, then the median of
blocks 1-3 against the median of blocks 8-10, so a single load spike must move three block
medians to move the verdict. `latency_p50 ≤ 1.5×`, `latency_p90 ≤ 2.0×`, correctness rate drop
`≤ 0.02`. Floors, identical on all three platforms: run correctness `≥ 0.99`, max session
`≤ 60 s`, p95 `≤ 10 s`. Slopes: `state_dir_bytes ≤ 2×`, `live_product_processes ≤ 0`,
`harness_active_handles ≤ 0`, `harness_rss_bytes ≤ 2×`.

**`bands.json` declares `numbers_are_measured: false` and the validator REJECTS the file if that
is ever flipped to true (`F28S-104`).** Every threshold is a pre-registered guess. It carries a
supersedes clause requiring a later phase to re-derive them from this soak's distributions for
the *next* candidate — doing it for this one would be option D.

**The honest limit, recorded in the bands document itself before any result existed:** in a soak
of 1,000 **fresh short-lived processes** a per-process leak cannot accumulate — it dies with each
process. Latency drift can only fire through state that outlives a process. Codex, asked in
isolation with no knowledge of the panel, reached the same conclusion unprompted. So the
detection weight sits on the **slope** bands, not the drift bands, and §4 below does not present
a green drift as the finding of no leak.

Three panel positions were **overruled** and the reasons are in `decision-dissent.txt`: gemini's
cold-start discard (deletes a real product property — block 1 is retained and published);
gemini's and codex's single-window p95 (replaced by kimi's block aggregation); and kimi's
load-adjusted **INCONCLUSIVE** verdict (rejected outright — a third verdict between pass and
fail is the shape an agent on this program abused when it invented a termination state, and the
validator now rejects a bands file that permits one). Codex's warm-up objection was **adopted**
and changed the design: warm-up may bind a value but may never define whatever happened as
correct, so a surface establishes an invariant only if it satisfies a *committed* sanity schema.

---

## 3. The harness — and the one place it deviates from the plan

`crates/wcore-eval-scenarios/src/e5_soak.rs` holds the canonical definitions and every VOID
rule. `scripts/f28-native-soak.mjs` is the executor.

**DEVIATION, the same one 28-02 recorded and forced by the same constraint:** the plan names
`e5_soak.rs` as "the soak harness". It is the canonical **definition**; the **executor** is a
Node script. A cargo-built harness cannot run on the certification Mac at all, and an observable
whose only implementation is such a harness silently loses a whole OS family.
`tests/e5_soak_contract.rs` asserts the two agree — geometry, the six channels (and that there
are exactly six, so a seventh cannot be invented to dilute a count), the census backend table
including its non-authoritative fallback, the panic sentinels, and that **no mutating verb can
ever enter the workload allowlist**.

**A second, smaller deviation, stated because it is a lever if left unstated:** the workload
classifier separates a surface that fails warm-up because of an *unsatisfied precondition* (it
needs an argument the harness may not invent, or a credential this plan is forbidden to embed)
from one that is *broken*. That distinction lives in the harness, not in `bands.json`, because
it is workload construction rather than a band. To stop it becoming a shrink-the-workload lever,
the validator now rejects a run whose established workload collapses (`F28S-007`, < 20 surfaces
or < 25% of resolved), and the three surfaces that fail without matching a precondition sentinel
were **deliberately left classified as broken** rather than having the sentinel list widened to
absorb them.

---

## 4. The soak — 2,000 sessions across two families, every control caught

Full document: `28-03-SOAK-RESULTS.md`, generated from `evidence/28-03/soak.json`.

| Family | Sessions | Canary | Orphans | Resource slope | Drift | **Criterion 2** |
|---|---|---|---|---|---|---|
| linux (`hetzner-dsm`) | **1000/1000** | 0 detections, control **6/6 channels** | **0**, control **FOUND**, `cgroup-v2` authoritative | green | green | **MET** |
| macos (arm64 cert Mac) | **1000/1000** | 0 detections, control **6/6 channels** | **0**, control **FOUND**, **non-authoritative** | green | green | **MET** |
| windows (`seandesktop`) | **0/1000 — NOT RUN** | — | — | — | — | **NOT MET** |

Concurrency 4 on both, digest-asserted against the ledger before the first session.
2,000 of 2,000 sessions matched their own warm-up invariant; zero panic sentinels anywhere.
Both families got slightly **faster** late in the run (macOS p50 block median 11.13 ms → 9.73 ms;
Linux 52.65 ms → 52.34 ms), so no band came close to firing in the wrong direction.

**Positive controls, which are the point rather than an accessory:** a control canary planted
into all six scan targets and detected in all six on both families; a deliberately orphaned
**product** process found by both censuses; a deliberately growing lane flagged by the same slope
evaluator on both. `--json-stream` was tried first as the orphan control and **replaced** — it
exits immediately without a credential, and a control that has already died proves nothing about
whether a census can see a live one.

**The macOS census is NON-AUTHORITATIVE and says so**, mirroring `process_tree.rs`'s
observed-process-group fallback. Its zero is a zero *observation*, not a containment guarantee.

**`state_dir_bytes` was 301 bytes at the first sample and 301 at the 101st on both families.**
That is a true measurement and a weak one: the workload is read-only by construction, so there
was little for it to write. A green here means "a thousand read-only sessions wrote nothing" —
worth knowing, and **not** the same as "the product does not accumulate state under use". This
is stated in §5 of the results document under its own heading rather than in a footnote.

---

## 5. STATED EXCEPTION — the Windows soak did not run

`seandesktop` answers at `100.109.207.54:22` (`OpenSSH_for_Windows_9.5`) and completes key
exchange. **Authentication fails for every combination available to this lane:** `sean`,
`seandonahoe`, `sdonahoe` and `wayland` are refused `(publickey,password,keyboard-interactive)`
with both the default `id_ed25519` and the `wayland_win` identity; `Administrator` is reset
immediately after kex. The ssh agent holds no identities.

**Supplying a credential is reserved to Sean.** This lane did not attempt to obtain one, did not
guess further, did not substitute a host, and did not shrink the run to fit.

**This plan PREDICTED a Windows failure and did not get to test it.** The prediction was that
carried entry `KR-01` — the descendant-process-tree reap defect, `p28_severity` HIGH,
`contradicted_criterion` 2, dispositions FIXED/DISPROVED only — would reproduce under soak
conditions and force Criterion 2 NOT MET. It remains **OPEN and untested by this plan.** An
untested prediction and a confirmed one differ exactly as a clean scan differs from an absent
scanner, which is the rule this entire plan is built on.

**What would close it:** one quiet scheduled-task run of `scripts/f28-native-soak.mjs` on
`seandesktop` against the digest-bound artifact
`54b12e8e5576ee54e88a93975c360e6c624202059f449d80574b71adf00c631e`, logged to a file with an
exit marker and **polled for it** — never inferred from the ssh call returning — with nothing
else running on that box.

**The gate is RED and that is correct.** A new rule (`F28S-054`) makes `--check-session-count`
exit 1 for a family that could not be run, specifically so that "the families we ran all passed"
can never read as "the soak passed".

---

## 6. The 24 macOS cells — re-run, and they now PASS

The macOS-activeness lane deliberately did not grade the matrix it supplies evidence to. This
lane did. `scripts/f28-native-matrix.mjs` was already merged at the base commit before anything
was re-run.

**216 of 216 macOS cells, 0 red, 0 skip. All 24 sandbox cells PASS and all 24 carry a positive
activeness observation.** Independently confirmed by the marker verifier:
`VERIFIED platform=macos cells=216`.

The activeness evidence is a **containment differential**, taken through the new `sandbox exec`
surface:

> DNS resolves outside and does not inside (network namespace); `/etc` is readable outside and
> denied inside (filesystem read confined) `[inside reading via sandbox-exec-surface]`

**28-02's single stated exception is closed on macOS.** Its diagnosis is confirmed by
measurement: this was an *observability* gap, not a containment gap — containment was real all
along and nothing exposed it.

**Two caveats carried forward, not dropped.** First, the activeness observation is
**run-level**, applied to all 24 sandbox cells, which is the matrix-construction concern 28-02
flagged for 28-04; the set was **not narrowed** here either. Second, **no cell carries any skip
and the `observation-blocked` class remains NOT AUTHORISED**; `KR-06` is not closed, the
`wedge-clearable` verdict is not generalised off `seandesktop`, and neither AppContainer intel
file is cited as evidence for anything.

**Of the 147 critical cells, the 74 that live on macOS and Linux were not all re-run by this
plan — only the macOS 216 were.** This plan re-ran the macOS family because that is the family
whose exception it was asked to resolve. It did **not** re-run the Linux or Windows matrix legs,
so the phase-wide "147 of 147 critical cells" claim still rests on 28-02's run at `32e2f57d`
plus this macOS re-run at `e4a3f5fc`. Plan 28-04 must not read this as a full-matrix
re-certification at the new candidate.

---

## 7. Gates — real numbers, every one read

| Gate | Result |
|---|---|
| `f28-check-soak.py --self-test` | **40 assertions, 0 failed** (6 accept-path, 34 rejections) |
| `f28-check-soak.py --check-bands` (real file) | accepted; same file with `floors` emptied → **rejected `F28S-111`** |
| `f28-check-soak.py --verify` (real record) | 8 observable verdicts, **all green** |
| `f28-check-soak.py --check-controls-caught` | **6/6 CAUGHT** |
| `f28-check-soak.py --check-series` | 8 slope evaluations, all in band |
| `f28-check-soak.py --check-attribution` | passes over an **empty** red set — a weaker statement than it looks, and recorded as such |
| `f28-check-soak.py --check-session-count` | **RED `F28S-054`** — windows NOT RUN |
| `f28-native-soak.mjs --self-test` | **29 assertions, 0 failed** |
| `f28-native-matrix.mjs --verify` (macOS log) | **VERIFIED, 216 cells** |
| `cargo clippy -p wcore-eval-scenarios --all-targets -- -D warnings` (hetzner) | **0 warnings, exit 0** |
| `e5_soak_contract` + `e5_soak` units (hetzner nextest) | **28 run, 28 passed, 0 failed** |
| `cargo fmt --all -- --check` | clean |
| `f28-resolve-candidate.py --verify` | OK — 131 surfaces, 6 targets, 19 inputs unchanged |

**Every VOID condition is proved by a test that trips it**, not asserted in a comment:
undetected control canary; each of the six channels dropped in turn; unfound control orphan; a
census claiming an authority its backend lacks; endpoint-only series; unflagged growth control;
a banded metric never sampled; an absent bands file; a non-candidate binary (all four
observables void, not one); a mostly-broken warm-up baseline; a collapsed workload.

A leak and a high-water mark are distinguished by a contract test using two series that share an
**identical endpoint** and have opposite trends — the receding spike is green, the monotone
climb is red.

**One self-passing shape found in my own gate and corrected.** The first hetzner clippy
invocation piped the remote output through `tail`, so `$?` was `tail`'s status: it reported
`CLIPPY_RC=0` while clippy had failed with two `bool_comparison` errors. Caught by reading the
log rather than the status. The gate was restructured to carry the real exit status and the
lints were fixed (`14d6ed6b`).

---

## 8. What I did NOT do

- **Nothing was repaired.** No production defect was fixed, including `KR-01`, which was not
  even reproducible here because its platform did not run.
- **No production file outside `crates/wcore-eval-scenarios` was touched**, checked against the
  **merge-base `e4a3f5fc`** rather than the branch name. `git diff --name-only e4a3f5fc HEAD --
  crates/` returns exactly `src/e5_soak.rs`, `src/lib.rs`, `tests/e5_soak_contract.rs`. The
  `wcore-cli` shared fence is untouched, so **there is nothing to serialize for this lane**.
- **No session target was reduced.** Two families reached 1,000; one reached 0 and says 0.
- **No band was widened after measurement**, and the commit ordering proves the bands predate
  the record.
- **No clean result was reported from a detector whose control was not caught.**
- **No existing test was modified, `#[ignore]`d, `#[allow]`ed, re-gated or deleted**, and no
  timeout was raised.
- **No `Cargo.toml` / `Cargo.lock` change**, no new dependency, no install.
- **`wcore-contract generate` was NOT run.** No PR, merge, tag, release or issue closure.
- **No receipt was signed** — that is plan 04. **No fifth Phase 28 plan.**
- **The `observation-blocked` skip class was not used**, `KR-06` was not closed, and neither
  AppContainer intel file was cited.
- **The 147 critical cells were not narrowed** — and were not all re-run either (§6).

## 9. Open for 28-04

1. **Windows Criterion 2 is unmeasured.** Host access is a Sean gate.
2. **`KR-01` is untested by this plan** and its accept path stays closed under A2.
3. **The soak's read-only workload** is why `state_dir_bytes` is flat; a state-accumulating
   workload would be a stronger measurement and is a deliberate future choice, not an oversight.
4. **The run-level activeness observation** covers 24 macOS sandbox cells with one differential —
   28-02 flagged this construction and it is still true.
5. **The macOS re-run is at `e4a3f5fc`; the Linux and Windows matrix legs are still 28-02's at
   `32e2f57d`.** The phase does not yet have a single-candidate full matrix.

## Self-Check: PASSED

All created files verified present on disk; all commit hashes verified in `git log`
(`1dea6437`, `1aaeaa2a`, `14d6ed6b`, `69182e38`, `763c7af7`, `a0ca3ecf`).
