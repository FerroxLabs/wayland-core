---
phase: 23B-continuous-agency
plan: "H1-measure"
subsystem: session-journal-durability
status: complete
verdict: NOT REPRODUCED at its own base under a strictly stronger instrument — root cause still UNKNOWN
supersedes_claim_in: 23B-H1-SUMMARY.md §1 (the 0/12 that could not reach)
tags: [journal, checksum, 23B-H1, measurement, reach, flux-router, instrument-repair]
lane: lane/23b-h1-measure
base: 3cfc336fd2d82f57b5a24716262a71e759cb4a24
key-files:
  created:
    - scripts/f23-h1-repro-live.sh
    - .planning/phases/23B-continuous-agency/evidence/23B-H1-measure/
  modified: []
---

# 23B-H1, measured: the harness now reaches, and the defect does not reproduce — including at
# the exact commit where it was seen 17 times in 18

**No credential value appears anywhere in this file, in any log it cites, or in any commit on
this branch.** Every preserved transcript was passed through a fixed-string redactor before it
left the build host, and each file was re-checked against the live key after transfer.

---

## 0. Bottom line

Three lanes have now worked this finding. The first two fixed a real encoding defect and
claimed root cause. The third proved that mechanism engine-unreachable, and then could neither
reproduce nor disprove anything, because **its harness pointed at a closed port and never
dispatched a single tool event** — 0 of 12, on top of 34 before it, all from an instrument that
could not reach the code under suspicion. Its own conclusion was that repairing the harness's
reach was the highest-value next step, and that it needed a real provider credential.

I had the credential. Here is what it bought:

| | Before this lane | After |
|---|---|---|
| Runs that dispatched a real tool event | **0** | **92 of 92** |
| Tool records written to journals under test | **0** | **153** |
| Binaries measured | HEAD only | HEAD, `a7beafe5^`, **and `15971d1b` — 23B-01's own base** |
| Stressors | none (quiet host) | CPU load to 114, 4- and 6-way process concurrency, **fsync saturation at 11k IOPS / 1.25 GB/s**, a real concurrent `cargo build`, and a turn cut off mid-flight |
| Reproductions | 0, and worthless | **0, and they now mean something** |

**23B-H1 does not reproduce at the commit where it was originally measured 8/8 and 9/10.** That
is the single most important number in this document, and it is not a claim I went looking to
make.

**It is still not a disproof, and I am not calling it one.** A non-reproduction can never
strictly disprove an intermittent defect. What it does establish, executably, is that the
*code state* at `15971d1b` is not sufficient to produce the failure under conditions that
strictly dominate the ones 23B-01 recorded. Whatever the remaining variable is, it is not in
the tree, and 23B-01 did not write it down.

**The root cause remains NOT IDENTIFIED.** I did not fix anything, because the mandate's own
sequencing — name the cause, *then* fix — was not satisfied, and inventing a fix for an
unidentified mechanism is how this finding got two wrong root causes already.

---

## 1. How I got the harness dispatching — the whole precondition

### 1.1 The reach defect, precisely

`scripts/f23-h1-repro.sh` lines 58-69 write:

```toml
[providers.anthropic]
api_key = "<assembled placeholder>"
base_url = "http://127.0.0.1:1"
```

A closed port. Every run therefore reports `status=OK_DISPATCH_FAILED`, and — the part that
matters — the harness has **no counter for whether a tool event was ever recorded**. Its
classifier folds a run that never dispatched into `resume_ok` via the `OK_DISPATCH_FAILED` arm
(line 125). So a run that could not possibly have exercised the defect was tallied as evidence
*against* it. That is the same shape as a gate that cannot fail, and it produced 46 of them.

### 1.2 The credential path, and why it never touched disk

`wcore-config::resolve_api_key_from_env` (`config.rs:2849`) resolves `ProviderType::FluxRouter`
from `$FLUX_API_KEY`, so no `api_key` is needed in `config.toml` at all. The harness therefore
writes a config containing only:

```toml
[default]
provider = "flux-router"
model = "flux-fast"

[providers.flux-router]
base_url = "https://api.fluxrouter.ai/v1"
```

The key is sourced on the Mac, piped to the build host over **ssh stdin**, consumed by a single
`IFS= read -r`, and exported only into the child process environment. It is never written to a
file on either host, never placed in argv, and never echoed. Every transcript the harness
preserves is filtered through a fixed-string `awk` redactor first, and I re-verified each
transferred log against the live key value (`CLEAN_b1 … CLEAN_ob5`, 9 of 9).

Provider verified before spending anything: `GET /v1/models` → `MODEL_COUNT=77`; one
tool-calling probe on `flux-fast` → `finish=tool_calls tool_calls=1`, argument
`{"file_path": "/tmp/aardvark.txt", "content": "aardvark"}`, `cost_usd=0.00072`. The mandate's
caveat is real and I honoured it — `flux-fast` reasons before it answers, so the harness passes
`--max-tokens 4000`; a small budget returns HTTP 200 with empty content and the whole budget
burned as reasoning tokens, which reads exactly like a product defect and is not one.

### 1.3 Reach proved with counts, not asserted

`scripts/f23-h1-repro-live.sh` emits a per-run reach line before it grades anything:

```
F23_H1_REACH=1 id=ee638d231dded0 tool_events=1 file_written=yes seed_exit=0 bytes=94409
```

`tool_events` counts `tool_intent_recorded` occurrences in the journal itself; `file_written`
checks that the `Write` tool actually landed a file with a run-time nonce in its name on disk.
A run with zero tool events lands in its own `no_tool_event` bucket and is **not** counted as a
non-reproduction. Across every batch: `no_tool_event=0`, `tool_runs=92/92`.

---

## 2. Reproduce or disprove — the counts, and which run each came from

All on `hetzner-dsm`, release binaries, `--dangerously-skip-permissions` so the tool executes
unattended. Logs are committed under
`.planning/phases/23B-continuous-agency/evidence/23B-H1-measure/`.

| Arm | Binary | Conditions | Runs | Tool events | `checksum_mismatch` |
|---|---|---|---|---|---|
| `b1` | HEAD `97a2602e…` | quiet | 10 | 30 | **0** |
| `b2` | HEAD | concurrent build, load 10 | 12 | 21 | **0** |
| `pf1` | `dba9b9e5` = `a7beafe5^` **pre-fix**, `8a6e9ee4…` | quiet | 12 | 18 | **0** |
| `ob1` | **`15971d1b` — 23B-01's own base**, `dc147bdd…` | quiet | 12 | 15 | **0** |
| `ob2` | `15971d1b` | CPU load 63 → 66 | 12 | 18 | **0** |
| `ob3` | `15971d1b` | **6-way concurrent**, load 70 → 114 | 12 | 18 | **0** |
| `ob4` | `15971d1b` | `--seed-max-turns 1` (turn cut off mid-flight) | 10 | 10 | **0** |
| `ob5` | `15971d1b` | **fsync saturation** 11 139 IOPS / 1.25 GB/s + real `cargo build`, 4-way concurrent | 12 | 23 | **0** |
| | | | **92** | **153** | **0** |

(Plus 1 smoke run and 2 aggregator-regression runs, excluded from the table because they are
instrument checks, not measurements. 95 seed+resume cycles were billed in total.)

Journal sizes spanned **62 KB to 250 KB**, covering both regimes 23B-01 recorded — its failing
shape at ≈203 KB and its passing shape at ≈71 KB.

### 2.1 The provenance of the `15971d1b` binary was proved independently of `--version`

This build's `--version` prints `wayland-core 0.12.25` with **no source sha**, unlike the string
the previous lane quoted, so the version alone cannot pin a commit and I said so in the harness
header rather than letting a coarse guard look precise. The `15971d1b` binary was pinned two
other ways: `sha256[0:16]=dc147bdd9db507ed`, and — the same check 23B-01 used on its own
pristine binary — `wayland-core session --help` **exits 1**, because the `session` subcommand
does not exist in that build. The HEAD binary answers it with twelve verbs.

### 2.2 A size correlation that reframes the original report

23B-01 inferred that its failing runs "get further through the turn" from 203 KB vs 71 KB. My
runs split on exactly that axis, and the thing that moves it is the number of tool records:

| tool events in run | journal size |
|---|---|
| 1 (or a turn cut off at 1) | 62 – 101 KB |
| 4 – 7 | 224 – 250 KB |

So 23B-01's failing runs almost certainly *did* contain tool records and its passing ones did
not — which means the reach gap was a real confound in every measurement taken since, and my
journals are now the shape that failed.

### 2.3 What is left, stated plainly rather than papered over

I controlled the binary (three, including the original), the reach (counted), CPU load (to 4×
the original), process concurrency (4- and 6-way), turn interruption, and disk/fsync
contention. What I could **not** control is the one thing 23B-01 never recorded: its provider
configuration and the exact shape of its bursts. My provider succeeds cleanly and quickly; if
the original's stalled, timed out, or failed part-way through a stream, that is a condition I
have not reproduced. **That is the honest residual, and it is why this is a non-reproduction
and not a disproof.**

---

## 3. Root cause: still NOT IDENTIFIED — but the exclusion is much stronger now

I can name more things it is not than any previous lane, and I still cannot name what it is.

**Already excluded by the record, re-checked and confirmed:**
- Torn writes, partial flushes and interleaved appends. `ChecksumMismatch` is check **3** of
  three in `verify_chain_from` (`session_journal.rs:1969`); all of those fail check 1 or the
  frame digest first.
- The `Option<Value>` + `Option::is_none` / `Some(Value::Null)` asymmetry that `a7beafe5`
  repaired — engine-unreachable, as the previous lane argued structurally.

**Newly excluded, by measurement:**
- **`a7beafe5` is not what changed the outcome.** The pre-fix binary (`pf1`) behaves
  identically to HEAD: 12/12 clean, 18 tool events. The previous lane's reachability argument
  is now corroborated empirically, not only structurally.
- **The code state at `15971d1b` is not sufficient.** 58 runs on that exact binary, 84 tool
  records, four stressor regimes, zero failures.

**Newly excluded, by reading, in the categories the brief named:**
- *A write that never completes its final record.* `append` (`session_journal.rs:1341`) builds
  the envelope via `JournalEnvelope::create`, which computes the checksum from the same
  `previous_checksum` value it stores, under `&mut self` behind an exclusive lease. There is no
  window in which `previous_checksum` is fixed up without the checksum being recomputed — which
  is the only shape that would pass check 2 and fail check 3.
- *A reader stricter than the writer.* `computed_checksum` re-serialises the **deserialised**
  event, so `ChecksumMismatch` ⟺ the event is not a serde round-trip fixed point. Within one
  version that requires a type whose `serialize` is non-injective on the image of
  `deserialize`. I swept for every member of that class in the journal model:
  - `HashMap`/`HashSet` iteration-order nondeterminism — the textbook cause of precisely this
    signature, and content- and load-sensitive in exactly the way 23B-01 described:
    `grep -cE 'HashMap|HashSet' session_journal/model.rs` → **0**. Designed out. `preserve_order`
    is not enabled, so `serde_json::Value` maps are `BTreeMap` and re-serialise sorted.
  - `#[serde(skip_deserializing)]`, or a bare `#[serde(skip)]` on a field that is serialized —
    **0** (the two `#[serde(skip)]` fields are the `legacy_effect_receipt` flags, which are
    neither written nor read).
  - `#[serde(untagged)]`, `#[serde(other)]`, `#[serde(flatten)]`, custom `impl Serialize`,
    `f64`/`f32` in the model — **0** each.
  - `LegacyEffectReceiptEncoding` (`model.rs:65`) is a thread-local encoding switch, but its
    guard restores the prior value on `Drop`, so it cannot leak across a serialization.
  - The remaining `skip_serializing_if` census the previous lane raised (~32 fields) is a
    hazard **only against a third-party producer**: within one version the writer skips the
    field, the reader defaults it, and the re-serialization skips it again — a fixed point in
    bytes. It cannot be the mechanism for a headless single-version run.

So the round-trip theory, which is the only theory consistent with `ChecksumMismatch` being
check 3, has no surviving member inside the journal model. Combined with §2, the most probable
remaining explanation is that **23B-01's decisive variable was environmental or procedural and
is not recoverable from the record.** I am stating that as the most probable explanation, not
as a finding — I did not measure it.

---

## 4. The fix: none, deliberately, and the repair path that is genuinely still missing

The mandate sequences this correctly: name the cause, *then* fix. The cause is not named, so I
did not fix. Two lanes have already shipped a repair for a mechanism that turned out not to be
the cause; a third would be worse than the disease.

**But one thing in the mandate is a real, currently-open product gap regardless of 23B-H1, and
I verified it rather than assumed it:** there is **no general quarantine or reclaim path for an
unreadable journal.** The only recovery in the tree is `recover_legacy_effect_receipt`
(`session_journal.rs:2185`), keyed literally to `"effect_receipt":null`. Every one of the
twelve `session` verbs — `show`, `search`, `export`, `retry`, `fork`, `reconcile`, `cancel`,
`retain`, `checkpoint`, `rewind` — reads the journal, so a checksum mismatch takes all of them
down together and the session is lost with no operator move available. This program solved the
same shape in the sandbox by reclaiming and quarantining rather than refusing forever. **That
capability should exist on its own merits**, and it is the correct backlog item to carry
forward from this finding — filed as a recommendation rather than written by me, because it
changes journal read paths and is not this lane's mandate.

---

## 5. Instrument defects found in MY OWN harness, and repaired in this lane (§6b-ii)

Two, both found by the instrument disagreeing with a number I could see with my own eyes.

**5a. No reach, and no bucket for its absence** — inherited from `f23-h1-repro.sh`, described
in §1.1. Repaired by the `F23_H1_REACH=` counter plus a dedicated `no_tool_event` bucket.

**5b. `[^\n]` is not "not newline" in a POSIX BRE.** My first aggregator summed tool events with
`grep -o 'F23_H1_REACH=[^\n]*'`. In a bracket expression `\n` is not an escape: it is the two
characters `\` and `n`. `tool_events` contains an `n`, so every match stopped **before** the
field it was reaching for, and the harness printed `tool_events=0` for a run whose own per-run
lines plainly read `tool_events=1` and `tool_events=7`. Caught live, on the aggregation
regression run:

```
F23_H1_REACH=1 … tool_events=1
F23_H1_REACH=2 … tool_events=7
F23_H1_LIVE runs=2 tool_runs=2 tool_events=0     <-- wrong, and silently so
```

This is the twelfth-plus instance of an instrument carrying the defect class it hunts: a
harness built to catch under-counting, under-counting. Repaired with `.` (grep is line-oriented,
so `.` is both correct and sufficient) and, per §6b-ii, **repaired here rather than written up
and left**.

**Self-test: six assertions, and both third assertions are the ones that prove the repair does
something.** `bash scripts/f23-h1-repro-live.sh --selftest`, run on both hosts:

```
SELFTEST_1_KNOWN_POSITIVE=PASS count=1
SELFTEST_2_KNOWN_NEGATIVE=PASS count=0
SELFTEST_3_OLD_MATCHER_BLIND=PASS old_on_reaching=resume_ok old_on_nonreaching=resume_ok new_counts=1/0
SELFTEST_4_AGG_KNOWN_POSITIVE=PASS sum=8
SELFTEST_5_AGG_KNOWN_NEGATIVE=PASS sum=0
SELFTEST_6_OLD_AGG_BLIND=PASS old_sum=0 new_sum=8
SELFTEST_RC=0
```

Assertion 3 replays the old classifier over a reaching and a non-reaching run and shows it
returns `resume_ok` for both — it never discriminated. Assertion 6 replays the broken `[^\n]`
pattern over markers whose true sum is 8 and shows it returns **0**. Without those two the
self-test would pass on the broken instrument.

**5c. A third, in the panel harness, worth recording.** `codex exec` hung to a 400 s timeout
twice, printing `Reading additional input from stdin...` — it blocks on an inherited pipe even
when the prompt is passed as an argument. Its vote was silently absent both times. Fixed with
`< /dev/null`. This is a new member of the brief's §4 "each of these silently drops a vote if
you invoke it wrong" list.

---

## 6. Cross-audit panel, and the internal pass that argued against it

Question put to all three: given 92 reach-proven runs at 0 across three binaries including
23B-01's own base, should 23B-H1 stay HIGH or drop to MEDIUM? Votes extracted unanchored;
codex taken from the last match.

| Panelist | Vote | Core argument |
|---|---|---|
| Codex 5.6 Sol | **MEDIUM** | 92 reach-proven runs at greater concurrency, load, journal size and interruption stress directly contradict a 17/18 recurrence rate; continued HIGH is disproportionate absent a reproducible trigger |
| Gemini 3.1 Pro | **MEDIUM** | the code state alone is executably shown not to cause it; the original depended on an unrecorded external variable, and a defect unactionable on its own parent commit cannot block a release |
| Kimi K3 | **MEDIUM** | the burden has flipped — backlog it *with the harness attached* so any future occurrence re-escalates automatically |
| Internal adversarial | **partly upheld → see below** | |

**My adversarial pass found a real hole, and all three panelists had accepted my framing
without it.** I told them "4× load". At that point my load was **64 spinning shells** — pure
CPU, essentially zero disk I/O. The journal write path is `write_all` + `sync_all`:
**fsync-bound**. 23B-01's stressor was *concurrent compile load from other phases*, which is
heavy I/O, page-cache and memory pressure. So my strongest arm may not have touched the actual
stressor at all, and three MEDIUM votes rested on a number that did not mean what it sounded
like.

Rather than report that as a caveat, I closed it: arm `ob5` ran twelve 4-way-concurrent runs on
the `15971d1b` binary under **12 parallel `dd oflag=dsync` writers sustaining 11 139 IOPS and
1.25 GB/s** plus a genuine concurrent `cargo build -p wcore-cli --release` — literally the
stressor 23B-01 describes. Result: 12/12 clean, 23 tool events, 0 mismatches. The rebuttal is
answered on its own terms.

**Verdict: I take the majority, MEDIUM, and the majority now also carries the stronger
evidence** — which it did not at the time it was cast. Kimi's condition is the right one and I
adopt it: this drops to MEDIUM **with the reach-proven harness attached**, so any future
sighting re-escalates against an instrument that can actually see it.

**Recommended disposition: 23B-H1 → MEDIUM, BACKLOG, non-blocking**, per the standing severity
policy, on the grounds that the instrument that originally observed it is not reproducible and
a strictly stronger one sees 0/92 at the same commit. I am **not** claiming it fixed and I am
**not** claiming it disproved. I did not edit `.planning/BACKLOG.md` — that is a shared file and
concurrent lanes are live; the orchestrator should file it.

---

## 7. Spend

Measured, not guessed, where I could measure it:
- One representative call at realistic prompt size: 2 421 prompt tokens → **`cost_usd`
  0.002429** (≈ $1.00 per million tokens on `flux-fast`).
- One tool-calling probe: 414 tokens → `cost_usd` 0.00072.
- Flux exposes no usage/billing endpoint on this key (`/v1/usage`, `/v1/account`, `/v1/me`,
  `/v1/billing/usage`, `/v1/dashboard/usage` all 404), so a per-lane total cannot be read back.

95 billed seed+resume cycles, each 2–5 provider calls over a growing context. On the measured
rate that is **roughly $4–7 — call it $5.** I deliberately used the cheapest model that
exercises the path, `flux-fast`, since the lane needed tool dispatch and not intelligence, and
every batch was bounded and attended.

---

## 8. What I did NOT do

- **I did not fix 23B-H1**, and I did not invent one. The cause is unnamed; the mandate's own
  sequencing forbids a fix before that, and two wrong root causes have already been shipped.
- **I did not build the journal quarantine/reclaim path.** I verified it is genuinely absent
  and recommended it as a standalone backlog item (§4) rather than half-building a change to
  journal read paths outside this lane's mandate.
- **I did not disprove the finding**, and nothing here should be read as a disproof. 0/92 is a
  non-reproduction under a stronger instrument. The residual is in §2.3.
- **I changed no Rust.** This lane's diff is one new shell script and evidence files. Merge-base
  `3cfc336f`; the two fence files `crates/wcore-cli/src/{lib,main}.rs` are untouched.
- I did not edit `.planning/BACKLOG.md` (shared file, concurrent lanes).
- **No credential value was written to any file, log, commit, or capture**, on either host.
- No merge, no PR, no tag, no issue closed.

## 9. Evidence

- `scripts/f23-h1-repro-live.sh` — the reach-proven harness, `--jobs`, `--seed-max-turns`,
  `--selftest`.
- `.planning/phases/23B-continuous-agency/evidence/23B-H1-measure/` — `23B-H1-MEASURE-NOTES.md`
  (append-only, committed from minute 15) and the eight batch logs `b1 b2 pf1 ob1 ob2 ob3 ob4
  ob5`, each redaction-verified after transfer.
- On `hetzner-dsm`: worktrees `/root/wayland-23b-h1-{measure,prefix,origbase}` and
  `/root/f23h1m/` hold the preserved journals and build logs. Removed at lane close except
  where noted in the final report.
