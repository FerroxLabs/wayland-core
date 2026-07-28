# 23B-H1 re-verification — running NOTES

Lane `lane/23b-h1`, worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-23b-h1`.
Base HEAD at branch time: `ef1d97beb61f1b084bdfba745e8f49830924d757`.

Append-only. Every measurement gets committed as it is made, not at the end (§6b-i).

---

## T+0 — the lane brief's premise is WRONG at HEAD, and this is established, not suspected

The brief states 23B-H1 was "filed nowhere", "routed to an intermediate file that no later
lane ever consumed", and recovered only by tonight's sweep. That is not the state of the tree.

Measured at `ef1d97be`:

| Claim in brief | Actual state at HEAD | Evidence |
|---|---|---|
| never consumed | consumed by two lanes | `23B-H1-DISPOSITION.md`, `23B-H1-RECOVERY-SUMMARY.md`, both committed |
| no fix | write-path fix merged | `a7beafe5 fix(23B-H1): stop the journal writer emitting an encoding it cannot read back` |
| no repair path | repair path merged | `4b9512a0 fix(23Bb): recover pre-fix journals without loosening the checksum` |
| filed into `.planning/BACKLOG.md` | **not present** — `grep -n 23B .planning/BACKLOG.md` returns 3 hits, all 23B-03 scorecard rows, none H1 | see §Evidence-1 |

Code actually present at HEAD:

- `crates/wcore-agent/src/session_journal/model.rs:40` — `fn is_absent_json_value`
- `model.rs:697`, `model.rs:1083` — both `effect_receipt` fields carry
  `skip_serializing_if = "is_absent_json_value"`
- `crates/wcore-agent/src/session_journal.rs:2185` — `recover_legacy_effect_receipt`
- `crates/wcore-agent/src/session_journal/snapshot.rs:131` — `recover_legacy_effect_receipts`
- `crates/wcore-agent/src/session_journal/model.rs:65` — `LegacyEffectReceiptEncoding` RAII guard
- tests: `crates/wcore-agent/tests/journal_envelope_roundtrip.rs`,
  `crates/wcore-agent/tests/journal_legacy_null_receipt_recovery.rs`

So this lane is NOT "fix an unfixed HIGH". It is: **independently re-verify at HEAD what two
prior lanes claim, and close the gap they both left open.** Trusting their write-ups would be
exactly the failure mode this program keeps measuring.

## T+0 — the gap both prior lanes left open, which is the real work here

Reading the two write-ups against the original finding, there is a hole neither closes:

1. **23B-01 measured the symptom at `sequence 16`** on a ~203 KB journal, from a real run,
   8/8 and 9/10 under load ~28, and 9/10 against a *pristine* `15971d1b` binary.
2. **23B-H1 (the fixing lane) could not reproduce that symptom at all — 0 reproductions in 34
   runs**, including 12 against the pristine base binary at load 130, i.e. well above the load
   under which 23B-01 saw it 17 times out of 18.
3. 23B-H1 instead found a *different* reproduction: a deterministic unit-level
   `ChecksumMismatch { seq: 1 }` from `Some(Value::Null)` + `skip_serializing_if`, and fixed it.
4. It then **inferred** that this mechanism is what 23B-01 hit, on the argument that
   `ChecksumMismatch` is the third of three checks so torn writes are excluded, and that a
   larger journal means the run reached the tool boundary.

The inference is reasonable and the elimination argument in §1 of the disposition is strong.
But it is an inference, and it is load-bearing: if the engine cannot in fact emit
`Some(Value::Null)` into a journal, then the mechanism fixed is real but **unreachable**, and
whatever produced 23B-01's 17-of-18 is still out there, unexplained and unfixed.

**8/8 vs 9/10 is not one measurement written twice.** Burst 1 = 8 runs / 0 readable / 8
mismatch. Burst 2 = 10 runs / 1 readable / 9 mismatch. Burst 3 = 3 runs / 3 readable / 0
mismatch, host quiet. One run in burst 2 came back readable under the same load that failed
the other nine — so the trigger is not "this load level", it is something racing within a run.
A pure serde encoding asymmetry is **deterministic per journal content**: given a run that
records a null receipt it fails every time, given one that does not it never fails. That
squares with 8/8 and 0/3 but sits awkwardly with 9/10 unless the tool-reaching depth itself
varies run to run — which is plausible under load, and is exactly what the disposition argues.
So it is not a contradiction; it is an unproved coincidence. Worth resolving, not assuming.

### Therefore this lane's questions, in priority order

- **Q1 (highest value).** Can the *engine* actually produce `Some(Value::Null)` in
  `effect_receipt` on a real path? If no, 23B-01's finding is NOT root-caused and the HIGH
  stands. Answerable by reading call sites — no cargo needed.
- **Q2.** Do the two prior lanes' fixes actually hold at HEAD? Run both test files BY FILE,
  read the `N passed` count back (§3.2 flavour (c) trap).
- **Q3.** Are their red-before-green claims real? Independently mutate the fix and confirm
  the tests go red. A green that was green at base proves nothing.
- **Q4.** Does the original symptom still reproduce at HEAD under load? Note in advance: a
  non-reproduction here is WEAK evidence, because the prior lane already got 0/34 with a
  harness that never reached a tool event. If I cannot reach a tool event either, my run
  measures nothing and I will say so rather than bank it as a pass.

### Traps I am pre-committing to avoid

- Do NOT report a green from `cargo test -p wcore-agent journal` — that is a *filter*, and a
  filter matching nothing exits 0 having run zero tests. Use `--test <file>` and read `N passed`.
- Do NOT diff against the branch name; capture `BASE=$(git merge-base HEAD plan/f20-unified-audit-repair)` once.
- Do NOT touch `crates/wcore-eval-scenarios/src/journey.rs` (another lane owns the four clippy lines).
- Byte-count every capture; `echo "EXIT=${PIPESTATUS[0]}"` after a pipeline returns empty here.

---

## Evidence-1 — BACKLOG does not carry this finding

```
$ grep -n "23B" .planning/BACKLOG.md
687:...ecision@1 = 0.8125` over the 16-query corpus in `23B-03-LIVE-EVIDENCE.md`. Three
699:...arks that layer optional; 23B-03 deferred it under its termination state 2 and r...
722:... is not a CI step, so no lane can run it on the authoritative build host. 23B-03
$ grep -c "23B-H1" .planning/BACKLOG.md
0
```

---

## T+35 — Q1 ANSWERED: the engine CANNOT emit `Some(Value::Null)`. The fix does not explain 23B-01.

This is the load-bearing result of the lane, so the whole chain is written out.

**Production call chain to a journalled effect receipt** (`crates/wcore-agent/`):

```
orchestration/mod.rs:1744   prepared_runtime = tool.prepare_effect(..)  -> Option<PreparedToolEffect>
orchestration/mod.rs:1821   durable_receipt  = prepared.durable_receipt()      -> Option<Value>
orchestration/mod.rs:1917   prepare_tool_effect(.., durable_receipt, ..)
orchestration/mod.rs: 966   (Some(receipt), None) => scope.prepare_tool_with_effect_receipt(.., receipt)
journal_effects.rs   : 249  prepare_tool_recorded(.., Some(effect_receipt), ..)
journal_effects.rs   : 397  journal.append(SessionEvent::ToolIntentRecordedV2 { effect_receipt, .. })
```

`durable_receipt` is the **only** production source of a `Some(..)` on that path. The other
three `prepare_tool_effect` call sites (`mod.rs:1047` `record_tool_not_started`, `mod.rs:2376`
unknown-tool, and the not-started paths) all pass a literal `None`.

And `durable_receipt` cannot be null:

```rust
// crates/wcore-tools/src/effects.rs:247
pub fn durable_receipt(&self) -> Result<Value, serde_json::Error> {
    serde_json::to_value(&self.receipt)          // self.receipt: FilesystemEffectReceiptV1
}
```

```rust
// crates/wcore-tools/src/effects.rs:64
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FilesystemEffectReceiptV1 { .. }
```

A derived `Serialize` on a named-field struct produces `Value::Object`, always. `effects.rs`
contains **no** hand-written `Serialize` impl (`grep -c 'impl Serialize\|impl serde::Serialize'`
= 0), so there is no escape hatch. `serde_json::to_value` of that type is `Object(..)` or `Err`,
never `Null`. The `Err` arm is handled separately at `mod.rs:1824` and journals a
`DispatchFailed` not-started instead.

The one other `ProviderIdempotent` receipt construction, `journal_effects.rs:1441`, is inside
that file's `mod tests` (`#[test] fn secured_input_envelope_and_contract_are_durable_…`), not
production. `engine.rs:25976` is likewise inside a test module (`SessionManager::new(dir.path()…,
"test", "test-model")` at `engine.rs:25940`).

### What follows, stated carefully

The write-path fix `a7beafe5` closes a **real** defect: the API
`prepare_tool_with_effect_receipt` takes a bare `Value` and does not reject a null one, and the
wire contract blesses explicit nulls, so a **third-party producer** — the Desktop host writing
through the documented protocol — can still drive it. The disposition says exactly this and I
am not disputing it. Keeping that fix is right.

But **23B-01's reproduction was a headless `wayland-core` run with no Desktop host and no
third-party producer.** For that reproduction the fixed mechanism is unreachable. So:

> **The fix that claims to have root-caused 23B-01 cannot have caused 23B-01.**

The disposition's own §5 is consistent with this without drawing the conclusion: it reports
**0 reproductions in 34 runs**, including 12 against the pristine base binary at load 130 —
higher than the load 28 under which 23B-01 saw the failure 17 times in 18. It reads that as
"same defect, different reach". Under Q1 that reading does not survive: the reach it needed was
not merely un-taken by the harness, it does not exist on the engine path at all.

**The load-sensitivity argument also collapses.** A serde encoding asymmetry is deterministic in
the journal's *content*: a run that records a null receipt fails every time, a run that does not
never fails. The disposition bridges that to 23B-01's load-sensitivity by arguing load makes a
run get further and reach the tool boundary. But per Q1, reaching the tool boundary produces an
`Object` receipt, not a null one — so getting further cannot produce the null shape, and the
bridge does not hold. 23B-01's burst 2 (9/10 — one run readable under the *same* load that
failed the other nine) is a within-run race, which is what a lease/concurrency fault looks like
and is not what an encoding asymmetry looks like.

**Therefore the HIGH is NOT closed.** What is closed is a latent third-party-reachable variant
plus a genuinely good recovery path. What is open is the original: a cleanly-exited headless run
producing `journal checksum mismatch at sequence 16`, cause unknown.

The coordinator's lead — 16 red journal-durability tests at HEAD, and a measured
`session journal writer lease is already held` failure under parallelism — is a far better
candidate for a load-sensitive within-run race than the encoding asymmetry is. Q4 now has a
specific hypothesis to test rather than a blind re-run.

## STATUS

- [x] worktree created, HEAD confirmed
- [x] brief premise checked against tree — premise is stale
- [x] **Q1 — engine CANNOT emit `Some(Value::Null)`; fix does not explain 23B-01**
- [ ] Q2 tests hold at HEAD
- [ ] Q3 mutation proves the gates can fail
- [ ] Q4 original symptom at HEAD
