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

---

## T+60 — coordinator's lead resolved: the 16/17 red tests are a PARALLELISM ARTEFACT

Run on `hetzner-dsm`, worktree `/root/wayland-23b-h1`, commit `ef1d97be`, both runs against
the **same** build (`cargo test -p wcore-agent --lib --no-run` first, rc=0).

| Run | Command | `test result:` line |
|---|---|---|
| SERIAL | `cargo test -p wcore-agent --lib -- --test-threads=1` | **ok. 2131 passed; 0 failed;** 3 ignored; finished in 138.28s |
| PARALLEL | `cargo test -p wcore-agent --lib` | FAILED. 2114 passed; **17 failed**; 3 ignored; finished in 30.37s |

**The suite is green serially at HEAD. There is no real journal-durability regression in
these 17.** Counts read from the `test result:` line, not from exit status (§3.2). Status file
carried `WLRC_BUILD=0 / WLRC_SERIAL=0 / WLRC_PARALLEL=101 / WLDONE`.

The failure mode is `JournalError::AlreadyOwned` — `session journal writer lease is already
held at <path>` — raised at `session_journal/lease.rs:226` when `try_lock_authority` returns
`Contended`. **Each failing test names its own distinct tempdir**
(`/tmp/.tmpV9U4R1/880fb529d4dc.journal`, `/tmp/.tmpqyPuCv/f14de111a101.journal`, …), so this is
not several tests colliding on one shared path; it is each test contending with itself, because
a previous handle on that same path has not released its advisory lock by the time the test
re-opens. Serialising the suite gives the drop time to complete.

Failing set (17), all in the durability area: 12 × `engine::audit_2026_05_22_tests::*`,
`child_transaction::tests::rejects_append_reopen_reduce_corruption`,
`engine::retry_wedge_protection_tests::ceiling_abort_does_not_persist_unrecoverable_session`,
`orchestration::tests::live_dispatcher_crash_cuts_replay_without_repeating_opaque_effects`,
`session::tests::cleanup_error_attempts_all_artifacts_but_retains_index_authority`,
`session_journal::fault_tests::session_retirement_never_deletes_a_replacement_journal_or_collateral`.

**This is NOT 23B-01's defect**, and I am not folding it in. 23B-01's symptom is
`ChecksumMismatch` — a journal that reads back wrong. This is `AlreadyOwned` — a journal that
will not open at all. Different error, different check, different failure surface. Reporting
it as the same thing would be exactly the kind of convenient merge this program keeps catching.
It is a genuine test-isolation weakness worth its own BACKLOG entry, at MEDIUM: it makes the
durability suite unable to run under the default harness, which is how it stayed invisible.

## T+70 — Q2 and Q3: the two prior fixes DO hold at HEAD, and the gate can fail

Run by **file** (`--test <name>`), never by filter, and the `N passed` count read back (§3.2
flavour (c)).

| Leg | Command | Result |
|---|---|---|
| Q2 write-path invariant | `--test journal_envelope_roundtrip` | **ok. 5 passed; 0 failed** |
| Q2 recovery path | `--test journal_legacy_null_receipt_recovery` | **ok. 4 passed; 0 failed** |
| Q3 mutation: predicate reverted to `Option::is_none` | `--test journal_envelope_roundtrip` | **FAILED. 4 passed; 1 failed** — `option_value_null_is_stable_across_a_round_trip` |
| Q3 mutation | `--test journal_legacy_null_receipt_recovery` | ok. 4 passed; 0 failed |
| restore | `git diff --quiet -- model.rs` | `RESTORED_CLEAN=yes` |

So the write-path gate **can** fail — it is not green-at-base.

### But note the discrepancy, because it weakens the protection

23B-H1's disposition reported the same mutation as **2 FAILED / 3 passed**, including the
end-to-end `ChecksumMismatch { seq: 1 }`. I measure **1 FAILED / 4 passed**. The difference is
`4b9512a0`: with the recovery path now in the tree, the end-to-end test no longer goes red
under the mutation, because the recovery layer sees the explicit null, re-hashes under the
legacy encoding — which under the mutation *is* the current encoding — and succeeds.

**The recovery path masks a regression in the write path.** If someone reverted `a7beafe5`
today, only one of nine tests across the two files would notice. That is a real reduction in
regression protection that neither prior lane could have seen, since each only existed on one
side of it. Worth a BACKLOG entry.

## T+75 — Q2b: the recovery summary's exhaustiveness claim is WRONG

`23B-H1-RECOVERY-SUMMARY.md` §4.4 says the defect *"requires a field typed
`Option<serde_json::Value>` with a `skip_serializing_if` predicate. Across the journal and
snapshot type tree those are exactly the two `effect_receipt` fields."*

The type is not what makes the shape hazardous. The hazard is: **`#[serde(default,
skip_serializing_if = P)]` where the skipped value has an explicit JSON spelling that decodes
back to that same skipped value.** Then `explicit spelling written+hashed` → decode → re-encode
skips it → the recomputed hash covers different bytes. `Option<serde_json::Value>` is one
instance; it is not the class.

Predicate census over `crates/wcore-agent/src/session_journal{,/}`:

| Predicate | Count | Explicit spelling that round-trips to the skipped value |
|---|---|---|
| `Option::is_none` | 21 | `null` |
| `BTreeMap::is_empty` | 5 | `{}` |
| `Vec::is_empty` | 4 | `[]` |
| `is_absent_json_value` | 2 | `null` — **the only two the recovery repairs** |
| `BTreeSet::is_empty` | 1 | `[]` |
| `is_zero_u32` | 1 | `0` |

Every one carries `#[serde(default, …)]`, verified by reading the attribute lines — e.g.
`model.rs:397 prior_attempt_ids: Vec<String>`, `model.rs:594 consumed_hook_phases`,
`model.rs:1348 tasks: BTreeMap<..>`, `model.rs:1615 depends_on: BTreeSet<String>`,
`model.rs:1622 attempts`, `model.rs:1626 handoffs`, `model.rs:1712 hook_phases`,
`model.rs:1721 child_transactions`, `model.rs:1731 goals`, `model.rs:1732 deliveries`,
`model.rs:1364 loop_owner_epochs: u32`.

So **~32 fields carry the hazardous shape, and the recovery repairs 2 of them.** A producer
writing `"handoffs":[]` or `"approvals":{}` explicitly and hashing those bytes creates exactly
the 23B-01 symptom, and the repair path does not fire because it only looks for
`"effect_receipt":null`.

Severity is the same class as what was already fixed — engine-unreachable, third-party
reachable — so this does not change the product's own exposure. But the exhaustiveness claim
is load-bearing for anyone reading §4.4 as "no further journals can be unreadable", and it does
not hold. BACKLOG, MEDIUM.

## T+80 — my own instrument was defective; repaired in-lane (§6b-ii)

My first mutation gate read `MUTATION_SITES=$(grep -c 'Option::is_none' model.rs)` and printed
`23`. Uninterpretable: `model.rs` already had **21** such predicates at base, so 23 = 21 + the
2 I flipped — but 21 (sed silently matching nothing) would have looked equally plausible, and I
captured no baseline. An unapplied mutation would have produced a green "reverted, still
passes", i.e. the self-passing class.

Repaired as `scripts/f23-h1-mutation-check.sh`: count the **target** predicate
`is_absent_json_value`, whose exact population is 2, and require 2 → 0.

Three assertions, run on this Mac:

```
SELFTEST_1_KNOWN_POSITIVE=PASS
SELFTEST_2_KNOWN_NEGATIVE=PASS
SELFTEST_3_OLD_MATCHER_BLIND=PASS old_on_fixed=MUTATED old_on_mutated=MUTATED
SELFTEST_RC=0
$ ./scripts/f23-h1-mutation-check.sh state crates/wcore-agent/src/session_journal/model.rs
FIX_PRESENT
```

Assertion 3 is the one that proves the repair does anything: the old matcher labels the
**unmutated** file `MUTATED` exactly as it labels the mutated one — it never discriminated.

## STATUS

- [x] worktree created, HEAD confirmed
- [x] brief premise checked against tree — premise is stale
- [x] **Q1 — engine CANNOT emit `Some(Value::Null)`; fix does not explain 23B-01**
- [x] coordinator's 17 reds — parallelism artefact, serial is 2131/0, NOT this defect
- [x] Q2 — both fixes hold at HEAD (5/5, 4/4)
- [x] Q3 — mutation reddens the write-path gate (1 failed), but recovery masks 4 of 5
- [x] Q2b — recovery's exhaustiveness claim false; ~32 fields share the shape, 2 repaired
- [x] instrument defect found in my own harness and REPAIRED with a 3-assertion self-test
- [ ] Q4 — original symptom at HEAD (binary building)
- [ ] Q2 tests hold at HEAD
- [ ] Q3 mutation proves the gates can fail
- [ ] Q4 original symptom at HEAD
