# 28-03 DELTA BANDS — what an unacceptable quality/performance delta is

**Requirement:** F28-02. **Plan:** 28-03 task 1. **Machine form:** `evidence/28-03/bands.json`,
enforced by `python3 .planning/scripts/f28-check-soak.py --check-bands`.

**These bands were committed BEFORE any soak session ran, and that ordering is provable from
the commit history:** the commit carrying `bands.json` precedes the commit carrying
`soak.json`. A threshold chosen after seeing the numbers is a threshold fitted to the result it
needs to permit, and it would make Criterion 2 unfalsifiable for exactly the candidate it is
meant to certify.

---

## 1. The decision

Criterion 2 forbids an "unacceptable quality/performance delta" over a 1,000-session soak
without defining *unacceptable* anywhere — not in ROADMAP.md, not in REQUIREMENTS.md, not in
any of the 27 prior phases. There was no prior decision to inherit, so one was taken.

### VERDICT: **OPTION C** — drift within the run as the primary gate, plus deliberately loose
absolute sanity floors. **Unanimous, 4 of 4.**

| Panel member | Invocation | Position | Confidence |
|---|---|---|---|
| codex | `codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check` | **C** | high |
| gemini | `gemini -m gemini-3.1-pro-preview -o text --skip-trust` | **C** | high |
| kimi | `/Users/seandonahoe/.kimi-code/bin/kimi -p … --output-format text` | **C** | medium |
| internal adversarial | argued AGAINST the emerging consensus | **C** | medium |

Votes: `28-03-decision-evidence/panel-{codex,gemini,kimi,internal}.txt`.
Rationale and dissent: `28-03-decision-evidence/decision-dissent.txt`.

**Two vote-loss traps fired and were caught, which is the reason this table can be trusted.**
Both are the same defect class as a self-passing gate — an invocation that returns cleanly
while contributing nothing:

- **gemini silently returned ZERO BYTES with `rc=1`** when the ~7 KB question was passed through
  `-p "$Q"`. `--skip-trust` was present; the documented trap was not the one that fired. Piping
  the same question on **stdin** returned a full answer. Had the artifact not been byte-counted,
  a four-way audit would have been recorded as four-way while being three-way.
- **codex silently produced NO ANSWER** — it echoed the prompt, emitted MCP auth errors and
  exited 0 — on the long question, twice, including with stdin closed. Probed with a **short but
  real** question it answered correctly and immediately. Re-asked with the question condensed to
  2.8 KB it answered in full. A one-word probe would have passed and hidden this.

Each artifact was byte-counted and position-extracted **unanchored** (`grep -o
'PANEL_POSITION=[A-D]'`, last match) before the vote was counted, because kimi bullet-prefixes
and indents its final block and codex repeats its final block.

---

## 2. Why C, and why not the other three

**A (drift only) is blind to the failure mode this candidate is most plausibly in.** A run that
is uniformly slow or uniformly wrong from session 1 produces a drift of zero and passes. Three
members reached this independently; it is the decisive fact.

**B (absolute ceilings only) invents per-platform numbers with no per-platform baseline.** Codex
put the useful form of the objection: *"using the same limits on every platform is more honest
than fabricating platform-specific baselines"* — which is why the floors below are identical on
all three families rather than tuned per box.

**D (characterise first, band after) is circular.** A threshold derived from the run it judges
cannot fail that run.

C is not a compromise. It is the only construction in which **both** failure modes — degradation
across the run, and a run that was bad throughout — have a mechanism that can fire.

---

## 3. THE HONEST LIMIT OF THIS GATE, stated before any result exists

**In a soak of 1,000 fresh short-lived processes, the latency-drift gate is structurally
narrow.** Each session is a separate process; its heap, descriptors and threads die with it. A
per-process leak *cannot* accumulate across 1,000 processes. Latency drift can therefore fire
only through state that **outlives a process**: accumulated on-disk state that later sessions
re-read, orphaned children competing for CPU, or host contention that is not the product's.

Codex, asked this in isolation with no knowledge of the panel: *"It cannot detect process-local
leaks — those die with each CLI process — or uniformly bad latency present from session 1."*

**Consequence, built into the design rather than footnoted:** the detection weight for "no
unbounded resource use" sits on the **slope bands** — state-directory bytes, live product
processes, harness handles — not on the drift bands. The results document may not present a
green latency drift as though it were the finding of a leak. It is the absence of one narrow
symptom.

**And what is called "quality" here is a DETERMINISM-AND-STABILITY measurement, not a semantic
one.** It detects a surface that stops behaving as it behaved at warm-up. It does not detect a
surface that behaves consistently and wrongly. Semantic correctness belongs to the per-surface
probe matrix of plan 28-02, not to the soak.

---

## 4. The bands, mechanically

Every number below is a **PRE-REGISTERED GUESS**. None has a measurement behind it.
`bands.json` declares `numbers_are_measured: false` and the validator **rejects the file** if
that field is ever flipped to true (`F28S-104`).

### Geometry

| | |
|---|---|
| session target | **1,000** — the requirement; this decision has no authority to reduce it |
| block size | 100 sessions → 10 blocks |
| early window | blocks **1, 2, 3** — **block 1 included** |
| guard interval | blocks 4-7 |
| late window | blocks **8, 9, 10** |
| aggregation | statistic per block, then **median of the three early blocks vs median of the three late blocks** |
| interleave | round-robin over workload surfaces so every block carries the same surface mix |
| minimum concurrency | **2** — a zero-concurrency configuration fails the contract test |
| resource sampling | every **10** sessions at a quiescent point; **≥ 90 samples retained** |

Cold start is **retained, not discarded**. Gemini proposed dropping sessions 1-100; that makes
the gate stricter but deletes a real product property. The per-block series is published so a
reader sees the cold start instead of having it removed for them.

### Drift bands (primary gate)

| Metric | Band | Why this number |
|---|---|---|
| `latency_p50_block_median_ms` | late ≤ early × **1.50** | 50% median degradation over a thousand sessions is generous for a shared host and far short of what O(N) on-disk state produces |
| `latency_p90_block_median_ms` | late ≤ early × **2.00** | the tail degrades first under resource pressure; p90 over a 100-session block has ten samples above it, enough to be an estimator |
| `quality_correct_rate_block_mean` | late ≥ early − **0.02** | a rate near 1.0 has no room for a ratio band; two points of late correctness loss is not reachable by host noise |

### Absolute floors (the reason this is C and not A)

| Metric | Band | Why |
|---|---|---|
| `quality_correct_rate_run` | **≥ 0.99** | answers the uniformly-broken run, which drift cannot see |
| `session_wall_ms_max` | **≤ 60,000** | also the harness per-session timeout; one minute for a local CLI invocation is defective on any of the three boxes |
| `session_wall_ms_p95` | **≤ 10,000** | catastrophic-only, identical on all three platforms |

### Resource slope bands (where the real detection weight sits)

| Metric | Band | Why |
|---|---|---|
| `state_dir_bytes` | ≤ **2.0×** per 1,000 sessions | the primary unbounded-growth path when nothing survives a process except what it wrote down |
| `live_product_processes` | ≤ **0** growth | sampled at quiescent points, so any growth is a process that outlived its session |
| `harness_active_handles` | ≤ **0** growth | a child whose pipes never close leaks a handle per session |
| `harness_rss_bytes` | ≤ **2.0×** | recorded as a property of the *measuring* process; a doubling means the harness is the leak and the run is suspect |

### Warm-up — and the rule that stops it certifying its own breakage

Codex's objection changed the design: *"Warm-up may bind variable values, but it may not define
'whatever happened' as correct. Otherwise a uniformly broken response could teach the validator
that broken output is normal."* This is the phase's positive-control rule one level up — an
invariant learned from a broken baseline cannot fail.

So warm-up runs **one occurrence of every workload surface before the 1,000 counted sessions**,
and a surface establishes an invariant **only if** its warm-up satisfies a **committed sanity
schema**: exit status 0, non-empty output, and none of `panicked at`,
`STATUS_ACCESS_VIOLATION`, `stack backtrace:`. A surface failing that is **BROKEN INVENTORY**
and establishes nothing. If broken inventory exceeds **5%** of the workload, the run is **VOID**
rather than passed. The invariant captured is exit-status class, output-non-emptiness, and a
**structural signature** — sorted top-level JSON key set where output parses as JSON, otherwise
line count plus sentinel absence — because "exits 0 but silently malformed" is the most
plausible quality-rot mode under resource pressure and is free to catch here.

### The noise rule, and the lever it deliberately does not give anyone

A shared host **can** manufacture drift the product did not cause. The remedy is evidentiary and
pre-registered, never discretionary:

- host load (1m) and live soak-child count are recorded **as part of the results**, always on;
- a drift breach is **RED**. The covariate series may justify **exactly one re-run on a quiet
  host**; it may **never** convert a breach into a pass;
- a re-run must be green **on its own numbers**; a second breach is final;
- **there is no third verdict between pass and fail.** Kimi proposed a load-adjusted
  INCONCLUSIVE and it was rejected: that is a dismissal lever with a statistician's vocabulary,
  and inventing a state to escape an unwanted result is a named prohibition on this program. The
  validator rejects a bands file that permits one (`F28S-113`).

### Supersedes

This soak's observed distributions become the program's **first** per-platform baseline. A later
phase is **required** to re-derive these bands from that baseline for the **next** candidate.
Re-deriving them for *this* candidate would be option D. Without this clause today's guesses
become precedent and never get revisited — which is how invented numbers get widened.

---

## 5. Proof these gates can fail

`f28-check-soak.py --self-test`: **36 assertions, 0 failed** — six accept-path fixtures and
**thirty rejections**, including one fixture per VOID condition (undetected control canary,
unfound control orphan, dropped channel, endpoint-only series, unflagged growth control, banded
metric never sampled, absent bands file, non-candidate binary, non-authoritative census claiming
authority) and nine bands rejections (dropped panel vote, reduced session target, floors removed
so C degenerates to A, overlapping windows, invented numbers claimed as measured, a third
verdict permitted, warm-up allowed to define whatever happened as correct, no sampling interval,
concurrency configured away).

Executed against the **real** `bands.json`, not a fixture: `--check-bands` accepts it (rc 0), and
the same file with `floors` emptied is **rejected with `F28S-111`** (rc 1). The gate is not
green-by-construction.
