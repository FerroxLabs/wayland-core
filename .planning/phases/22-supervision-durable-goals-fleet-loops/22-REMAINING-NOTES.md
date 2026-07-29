# 22-remaining — NOTES (append-only, committed early per LANE-BRIEF §6b-i)

Lane `lane/22-remaining`. Base `5457710e5bccd7c91a117f055ed42531bc2327bb`
(= `plan/f20-unified-audit-repair` after the 24-lane merge).
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-22-remaining`.

Brief: (1) re-grade Phase 22 honestly and supersede `22-PHASE-VERDICT.md`, correcting the
`GOAL-*` ledger row; (2) close `22-02 Task 3`'s two capability rows whose runtime path is
unwired (`F05-TRUTH-2` mid-flight monitor, `F05-TRUTH-4` learned policy); (3) then whatever
else the re-grade shows is closeable.

---

## M0 — an instrument defect in my own first measurement, caught before it published

My first sweep for `MidFlightMonitor::tick()` call sites was

```
/usr/bin/grep -rn "\.tick()\|\.tick_provider()\|\.tick_tool(" "--include=*.rs" crates/ | head -30
```

It returned only `crates/wcore-agent/tests/midflight_monitor_test.rs` for the monitor, which
reads as "the monitor has no production caller" — the exact conclusion the ledger asserts.

**That answer was false, and `head -30` produced it.** `crates/wcore-agent/src/channel_lease.rs`
has ~20 `.tick()` hits and sorts before `engine.rs`, so the 30-line cap truncated the output
*before* `engine.rs` — which contains the real production calls at `engine.rs:11868`
(`self.midflight_monitor.tick()`) and `engine.rs:10884` (`tick_provider()`).

This is LANE-BRIEF §3b-i in its purest form: **a truncated pipe manufactures an absence for
free**, and the absence it manufactured agreed with my brief's premise, which is why it would
have shipped. Repair, applied for the rest of this lane:

- every load-bearing sweep is written to a file first, then counted and read from the file;
- **never `| head`** on a measurement whose result is an absence;
- every absence claim carries a known-positive control in the same invocation.

Instrument self-test with three assertions (§6b-ii) is in
`22-REMAINING-EVIDENCE/instrument-selftest.md` (to be written before any absence is reported).

---

## M1 — F05-TRUTH-2 (mid-flight monitor): the ledger row is STALE, not unwired

Measured at base `5457710e`, in `crates/wcore-agent/src/engine.rs`:

| Fact | Site |
|---|---|
| owned field on the engine | `engine.rs:2683 midflight_monitor: MidFlightMonitor` |
| constructed at 3 engine constructors | `engine.rs:3154`, `:3389`, `:15368` |
| rebuilt per run budget | `engine.rs:8663` |
| fed real stream attempts | `engine.rs:9805`, `:10641`, `:10834` |
| fed real tool errors | `engine.rs:11698`, `:11700` |
| **`tick_provider()` consulted in the provider loop** | `engine.rs:10884` |
| **`tick()` consulted in the tool loop** | `engine.rs:11868` |
| decisions emitted to the protocol | `emit_midflight_monitor_decision` ×6 |
| **runtime outcome proof emitted** | `emit_midflight_monitor_occurrence()` `engine.rs:6043`, called ×6 |
| startup truth | `bootstrap.rs:2922` → `engine.midflight_monitor_constructed()` → `true` → `Ready` |

So the product does **not** report this capability `Unavailable: runtime path unwired`; it
reports it **Ready**, and the runtime path is a live consult in both the provider loop and the
tool loop, with a `reached/outcome_changed/observed` occurrence triple on every decision.

**The COMPETITIVE-LEDGER `F05-TRUTH-2` row is stale**, and so is the `GOAL-*` row's use of it
as a checkable blocker. TO VERIFY NEXT: read the capability line back out of the shipped
binary's own activation stream (§3b-ii — do not infer it from source), and prove the
occurrence triple actually fires by driving a repeated tool error.

## M2 — F05-TRUTH-4 (learned policy): genuinely unwired, and the shape of the gap

`capability_activation.rs:93-97` emits `LearnedPolicy → Unavailable(RuntimePathUnwired)`
**unconditionally** — there is no `StartupCapabilityInputs` field for it at all, so no
configuration can ever make it Ready. Downstream:

- `node_executor.rs:132` — `pub learned_policy: Option<Arc<LearnedPolicy>>` exists on the
  executor config, and a pre-filter call site exists;
- `node_executor.rs:318` — comment: "CallActor::SubAgent is never constructed; LearnedPolicy::new is …";
- `permission_prompt.rs` — the ask/record half exists;
- every non-test `LearnedPolicy::new()` construction: **to be counted from a file, not a pipe.**

TO ESTABLISH: whether closing this is a wiring job (populate the config field from a real
store + flip the activation input) or a design decision, and whether it belongs to GOAL-* at
all — `LearnedPolicy` lives in `wcore-permissions`, which is AUTH-*, not loop ownership.

## M3 — position on the verdict file

`22-PHASE-VERDICT.md`'s `UPDATE — 2026-07-27` predates lanes `22-C1`, `22-c3` and
`22-c3-goal`, all three of which are in my base. Its Criterion 1 and Criterion 3 grades are
therefore both stale. Re-grade to be written as a second superseding section; nothing above it
edited, per the file's own convention.

---
## Status
- [x] base verified, worktree identity confirmed
- [x] instrument defect found and repaired before publication
- [ ] absence self-test written
- [ ] F05 rows re-measured against the shipped binary
- [ ] re-grade written
- [ ] ledger row corrected
