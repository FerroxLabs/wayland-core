---
phase: 23B-continuous-agency
plan: "H1-reverify"
subsystem: session-journal-durability
status: complete
verdict: FINDING REMAINS OPEN — severity HIGH at HEAD
supersedes_claim_in: 23B-H1-DISPOSITION.md §2, 23B-H1-RECOVERY-SUMMARY.md §4.4
tags: [journal, checksum, data-loss, 23B-H1, re-verification, root-cause]
lane: lane/23b-h1
base: ef1d97beb61f1b084bdfba745e8f49830924d757
key-files:
  created:
    - scripts/f23-h1-mutation-check.sh
    - .planning/phases/23B-continuous-agency/evidence/23B-H1-reverify/23B-H1-NOTES.md
  modified: []
---

# 23B-H1 at HEAD — the repair path works; the root cause was never found

**Bottom line.** The two prior lanes fixed a real defect and built a genuinely good repair
path, and I re-proved both at HEAD, live, with falsification legs. But the mechanism they fixed
**cannot be what 23B-01 measured**, because the engine cannot produce the shape it repairs. So
the original HIGH — a cleanly-exited run writing a journal the product cannot read back — is
**still unexplained and still open.** Cross-audit panel 2–1 for HIGH.

I did not fix it. I could not reproduce it. Both statements are in §6, plainly.

---

## 0. The brief's premise was stale, and that is worth saying first

The lane brief states 23B-H1 was "filed nowhere", "routed to an intermediate file that no later
lane ever consumed", and recovered only by tonight's sweep. **That is not the state of the
tree**, and I did not spend the lane re-doing work that already existed:

| Brief's claim | Measured at `ef1d97be` |
|---|---|
| never consumed | consumed twice — `23B-H1-DISPOSITION.md`, `23B-H1-RECOVERY-SUMMARY.md` |
| no fix | `a7beafe5 fix(23B-H1): stop the journal writer emitting an encoding it cannot read back` |
| no repair path | `4b9512a0 fix(23Bb): recover pre-fix journals without loosening the checksum` |
| filed into `.planning/BACKLOG.md` | absent — `grep -c "23B-H1" .planning/BACKLOG.md` → **0** |

So the lane became: **independently re-verify what those two lanes claim, and close the gap
they both left open.** That gap turned out to be the whole finding.

---

## 1. Does it reproduce at HEAD? No — and that is weak evidence, not a disproof

`scripts/f23-h1-repro.sh` against the HEAD release binary on `hetzner-dsm`:

```
F23_H1_REPRO runs=12 resume_ok=12 checksum_mismatch=0 other_failure=0 seed_failure=0
LOAD_BEFORE=5.39 8.28 7.71   LOAD_AFTER=4.15 7.55 7.49
```

Zero reproductions in 12, on top of the previous lane's 34. **I am not banking that as a pass.**
Every run reports `status=OK_DISPATCH_FAILED` — the harness drives a turn against a closed port,
so dispatch fails and **no tool event is ever recorded**. The harness provably cannot reach the
code path under suspicion. Non-reproduction from an instrument that cannot reach the defect is
the evidentiary form of a gate that cannot fail, and I am grading it as such.

One incidental result that undercuts the disposition's reasoning: these journals are **~445 KB**,
more than double the ~203 KB that 23B-01 recorded on its *failing* runs, and they read back
fine. So "the failing runs are bigger, therefore they got further, therefore they reached the
tool boundary" does not hold as a general inference.

---

## 2. Root cause, named specifically: the fixed mechanism is ENGINE-UNREACHABLE

The brief asked me to separate three candidates — a write that never completes its final record,
a schema the reader rejects, and a reader stricter than the writer. The disposition's elimination
argument (§1) settles that correctly and I re-checked it: `ChecksumMismatch` is the **third** of
three checks in `verify_chain_from`, so the frame's own SHA-256 over the on-disk bytes and the
`previous_checksum` chain link both **passed**. Torn writes, partial flushes and interleaved
appends all fail check (1) first. What remains is category three — **a reader stricter than the
writer**, specifically an encoding that is not a round-trip fixed point.

The disposition then identified one such asymmetry and fixed it:
`Option<serde_json::Value>` + `skip_serializing_if = "Option::is_none"` holding `Some(Value::Null)`
writes explicit `null`, decodes to `None`, re-serializes to nothing.

**That mechanism is real. It is also unreachable from the engine.** Production chain:

```
orchestration/mod.rs:1744  prepared_runtime = tool.prepare_effect(..)  -> Option<PreparedToolEffect>
orchestration/mod.rs:1821  durable_receipt  = prepared.durable_receipt()
orchestration/mod.rs:1917  prepare_tool_effect(.., durable_receipt, ..)
orchestration/mod.rs: 966  scope.prepare_tool_with_effect_receipt(.., receipt)
journal_effects.rs   : 397  journal.append(SessionEvent::ToolIntentRecordedV2 { effect_receipt, .. })
```

`durable_receipt` is the **only** production source of a `Some(..)` on that path — the other
three `prepare_tool_effect` call sites (`mod.rs:1047`, `mod.rs:2376`, and the not-started paths)
pass a literal `None`. And it cannot be null:

```rust
// wcore-tools/src/effects.rs:247
pub fn durable_receipt(&self) -> Result<Value, serde_json::Error> {
    serde_json::to_value(&self.receipt)          // FilesystemEffectReceiptV1
}
// wcore-tools/src/effects.rs:64
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesystemEffectReceiptV1 { .. }
```

A derived `Serialize` on a named-field struct yields `Value::Object`, always; `effects.rs`
contains no hand-written `Serialize` impl (count = 0). The `Err` arm is handled separately at
`mod.rs:1824` and journals a `DispatchFailed` instead. The two other receipt-bearing sites
(`journal_effects.rs:1441`, `engine.rs:25976`) are inside `mod tests`.

**Consequence.** `Some(Value::Null)` requires a third-party producer writing through the
documented wire contract — the Desktop host. **23B-01's reproduction was a headless
`wayland-core` run with no such producer.** Therefore:

> The fix that claims to have root-caused 23B-01 cannot have caused 23B-01.

The disposition's own §5 is consistent with this without drawing the conclusion: **0
reproductions in 34 runs**, including 12 on the pristine base binary at load 130 — far above the
load 28 under which 23B-01 saw it 17 times in 18. It reads that as "same defect, different
reach". Under the call-chain above, the reach it needed does not exist on the engine path.

**The load-sensitivity argument collapses with it.** An encoding asymmetry is deterministic in
journal *content*: a run recording the null shape fails every time; one that does not, never
does. The bridge to load was "load makes the run get further and reach the tool boundary" — but
reaching the tool boundary produces an `Object` receipt, not a null one. And 23B-01's burst 2
(**9/10** — one run readable under the *same* load that failed the other nine) is a within-run
race, which is not what a content-deterministic encoding fault looks like. That is the 8/8-vs-9/10
difference the brief flagged, and it does matter: 8/8 and 0/3 are consistent with content
determinism, 9/10 is not.

**So the root cause of 23B-01 is: NOT IDENTIFIED.** I can name what it is not — not a torn
write, not a partial flush, not interleaved appends (all excluded by check 1), and not the
`effect_receipt` asymmetry (excluded by reachability). I cannot name what it is.

---

## 3. Both directions proved at HEAD, with falsification

Live, `hetzner-dsm`, HEAD release binary, provenance asserted first
(`wayland-core 0.12.25 (source ef1d97beb61f1b084bdfba745e8f49830924d757)`).

### Positive path — a damaged journal IS read back (counted, not assumed)

```
--- session reconcile s-d4687292c45dd253 (exit 0)
F23_SESSION=reconcile_item id=s-d4687292c45dd253 kind=tool_execution ref=x1 tool=Write turn=t1 reason=Prepared resolvable=false
F23_SESSION=reconcile id=s-d4687292c45dd253 outstanding=1
F23_H1_DRIVE=PASS platform=linux mode=readable nonce=d4687292c45dd253
```

`ref=x1`, `tool=Write`, `turn=t1` exist nowhere but inside the recovered journal, and the nonce
is generated at run time and planted in the session id, so a stale log cannot satisfy it.

### The gates can fail

| Falsification | Required | Observed |
|---|---|---|
| same binary asked to prove the journal **unreadable** | non-zero | **exit 1**, `F23_H1_DRIVE=FAIL` |
| wrong `--sha` (stale-binary guard) | non-zero | **exit 3** |

Without the first row the readable row proves nothing. Exit status is the primary gate; the
nonce-bound marker is a second, independent one. No pipeline carries a gate's status.

### Unit level, run BY FILE with the count read back

| Leg | Result |
|---|---|
| `--test journal_envelope_roundtrip` | **ok. 5 passed; 0 failed** |
| `--test journal_legacy_null_receipt_recovery` | **ok. 4 passed; 0 failed** |
| mutation: predicate reverted to `Option::is_none` → roundtrip | **FAILED. 4 passed; 1 failed** (`option_value_null_is_stable_across_a_round_trip`) |
| restore | `RESTORED_CLEAN=yes` (`git diff --quiet`) |

---

## 4. Three findings neither prior lane could have seen

### 4a. The recovery path MASKS write-path regressions — MEDIUM

The disposition measured its mutation as **2 FAILED / 3 passed**. I measure **1 FAILED /
4 passed**. The difference is `4b9512a0`: with recovery in the tree, the end-to-end case no
longer reddens, because the recovery sees the explicit null and re-hashes under the legacy
encoding — which *under the mutation* is the current encoding — and succeeds. **If someone
reverted `a7beafe5` today, only one of nine tests across the two files would notice.** Neither
lane could see this; each existed on one side of it.

### 4b. The recovery covers 2 of ~32 hazardous fields — MEDIUM

`23B-H1-RECOVERY-SUMMARY.md` §4.4 claims the defect *"requires a field typed
`Option<serde_json::Value>` with a `skip_serializing_if` predicate … those are exactly the two
`effect_receipt` fields."* The type is not what makes the shape hazardous. The hazard is
`#[serde(default, skip_serializing_if = P)]` where the skipped value has an explicit JSON
spelling that decodes back to itself. Census over `session_journal{,/}`:

| Predicate | Count | Explicit spelling |
|---|---|---|
| `Option::is_none` | 21 | `null` |
| `BTreeMap::is_empty` | 5 | `{}` |
| `Vec::is_empty` | 4 | `[]` |
| `is_absent_json_value` | **2** | `null` — **the only two repaired** |
| `BTreeSet::is_empty` | 1 | `[]` |
| `is_zero_u32` | 1 | `0` |

All carry `#[serde(default, …)]` (e.g. `model.rs:397 prior_attempt_ids`, `:1348 tasks`,
`:1615 depends_on`, `:1626 handoffs`, `:1721 child_transactions`, `:1731 goals`, `:1364
loop_owner_epochs`). A producer writing `"handoffs":[]` explicitly and hashing those bytes
reproduces the 23B-01 symptom exactly, and the repair does not fire — it looks only for
`"effect_receipt":null`. **§4.4 must not be read as "no further journals can be unreadable".**

This also matters for §2: the true root cause may well be another member of this class.

### 4c. `wcore-agent --lib` cannot run under the default harness — MEDIUM

Coordinator's lead, resolved. Same build, same commit:

| Run | `test result:` |
|---|---|
| `-- --test-threads=1` | **ok. 2131 passed; 0 failed;** 3 ignored |
| default (parallel) | FAILED. 2114 passed; **17 failed**; 3 ignored |

**The suite is green serially at HEAD — the 17 are a parallelism artefact, not a regression.**
Failure is `JournalError::AlreadyOwned` — `session journal writer lease is already held` — from
`session_journal/lease.rs:226`. Each failing test names its **own** tempdir
(`/tmp/.tmpV9U4R1/880fb529d4dc.journal`), so this is not tests sharing a path; each contends
with itself because a prior handle has not released its advisory lock in time.

**I am not folding this into 23B-H1.** Its symptom is `AlreadyOwned` (journal will not open),
not `ChecksumMismatch` (journal reads back wrong). Different check, different surface. Reporting
them as one thing would be exactly the convenient merge this program keeps catching. It is
nonetheless why the durability suite's true state has been invisible.

---

## 5. Instrument defects found in my own harness, and REPAIRED (§6b-ii)

**5a.** My first mutation gate read `MUTATION_SITES=$(grep -c 'Option::is_none' model.rs)` and
printed `23`. Uninterpretable: the file already had **21** such predicates, so 23 = 21 + 2 — but
21 (sed silently matching nothing) would have looked equally plausible, and I captured no
baseline. An unapplied mutation would have yielded a green "reverted, still passes".

Repaired as `scripts/f23-h1-mutation-check.sh`: count the **target** predicate
`is_absent_json_value`, exact population 2, require 2 → 0. Three assertions, as required:

```
SELFTEST_1_KNOWN_POSITIVE=PASS
SELFTEST_2_KNOWN_NEGATIVE=PASS
SELFTEST_3_OLD_MATCHER_BLIND=PASS old_on_fixed=MUTATED old_on_mutated=MUTATED
SELFTEST_RC=0
$ ./scripts/f23-h1-mutation-check.sh state crates/wcore-agent/src/session_journal/model.rs
FIX_PRESENT
```

Assertion 3 is the only one that proves the repair does anything: the old matcher labels the
**unmutated** file `MUTATED` identically to the mutated one — it never discriminated.

**5b.** A second, smaller one: `git diff --name-only <BASE> -- <fence paths> | wc -l` returned
**1** on an empty diff, because this environment's `rtk` filter splices a `--- Changes ---`
decoration line into git output. It nearly made me report a shared-file fence violation I had
not committed. Fixed by counting with `grep -c .` over a file instead of `wc -l` over the
filtered stream. True values: **0 fence files, 0 Rust files** changed by this lane.

---

## 6. Verdict, and what I did NOT do

**Severity at HEAD: HIGH. The finding is OPEN.**

Cross-audit panel (§4), extracted unanchored, codex taken from the last match:

| Panelist | Vote | Core argument |
|---|---|---|
| Codex 5.6 Sol | **HIGH** | the `Some(Null)` defect cannot explain the headless run, so its recovery path does not establish recovery from the actual mechanism |
| Gemini 3.1 Pro | **HIGH** | a repair that re-hashes structurally valid data will not recover a journal damaged by a different fault; 46 non-reproductions from a harness that fails early are zero evidence |
| Kimi K3 | MEDIUM | severity should track current impact; "permanently unreadable with no repair path" is gone, so demote to non-blocking backlog |
| Internal adversarial | HIGH survives | see below |

**Majority HIGH, 2–1, and the majority also carries the stronger evidence** — which is my own
§2, so I weighted it carefully rather than treating it as agreement with myself. Kimi's argument
is methodologically right (severity tracks current impact, not report history) and I nearly took
it. It fails on one factual point: the repair is keyed literally to `"effect_receipt":null`, so
the impact-elimination it relies on is established **only for the shape that is not the
cause**. Arguing against the HIGH consensus: keeping a HIGH open on an unreproduced ghost blocks
release on nothing, and the operator verbs 23B-01 found broken now exist and work. Rebuttal: per
§4b the repair covers 2 of ~32 shapes, so for the unknown cause the user is still left with a
durable session they cannot recover — which is the exact property that made this HIGH.

Under the standing policy — *CRITICAL/HIGH must be fixed, or disproved with executable
evidence* — this is neither. I am reporting it red.

### What I did NOT do

- **I did not fix 23B-H1.** I could not, because I could not identify its cause; I could only
  prove which cause it is not. Inventing a fix for an unidentified mechanism would have been
  worse than reporting it open.
- **I did not reproduce it.** 0/12, with the harness limitation stated in §1 rather than banked.
- **I did not repair the harness's reach.** Driving a reconciler-declaring tool to the journal
  boundary needs a real provider credential this lane does not have and must not supply. **This
  is the single highest-value next step**: until a harness reaches a real tool event, neither
  reproduction nor disproof is available to anyone.
- **I changed no Rust and touched neither fence file** (verified 0/0 against merge-base
  `ef1d97be`). I did not touch `crates/wcore-eval-scenarios/src/journey.rs`.
- I did not edit `.planning/BACKLOG.md` — three MEDIUM entries (§4a, §4b, §4c) are recommended
  for the orchestrator to file, to avoid a shared-file conflict with concurrent lanes.
- No PR, no merge, no tag, no issue closed.

## 7. Evidence

- `.planning/phases/23B-continuous-agency/evidence/23B-H1-reverify/23B-H1-NOTES.md` — append-only log
- `scripts/f23-h1-mutation-check.sh` — repaired instrument + self-test
- On `hetzner-dsm`, `/root/wayland-23b-h1/f23h1-out/`: `serial.log`, `parallel.log`,
  `q2-*.log`, `q3-*.log`, `live-readable.log`, `live-unreadable.log`, `live-badsha.log`,
  `q4-repro.log`, and the `WLRC=…/WLDONE` status files each run was graded from.
