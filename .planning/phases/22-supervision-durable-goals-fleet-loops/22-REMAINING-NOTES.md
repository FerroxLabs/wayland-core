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

---

## M4 — LearnedPolicy: the lie was in the API, not in the capability report

Measured at base, from a file (never a pipe), with the control repaired after my first
known-positive (`cfg\.policy_gate`) returned **0** because rustfmt splits it across lines:

| Measurement | Count |
|---|---|
| control 1 `&cfg.tools` (read in the same fn) | 3 |
| control 2 `\.policy_gate` (proves the field-read pattern is greppable) | 3 |
| **`cfg.learned_policy` — any read** | **0** |
| **`\.learned_policy` in ANY form, incl. rustfmt-split** | **0** |
| `CallActor::SubAgent` construction outside tests/docs | **0** |

So `AgentExecutorConfig` carried a **`pub` field with zero readers in the workspace**, whose
own doc comment said "each tool call's `(name, argv)` is run through the policy BEFORE the
approval path". Setting it did nothing. The capability report was honest
(`unavailable / runtime_path_unwired`); **the API was not.**

`node_executor.rs:316-323` recorded the removal and pointed at `52b1ae2~..HEAD` to restore
from. **That revision does not exist** — this repository's history begins at a squashed root
`da5a18b5` (`git rev-list --count da5a18b5` = 1), so the original pre-filter is unrecoverable.
Written fresh against `actor_acl_test.rs`, which is its surviving spec.

## M5 — Decision: WIRE it, not delete it, and narrowing-only

Three grounds, in order of weight:

1. **The audit already decided.** `docs/design/2026-07-13-...-frontier-gap-audit-...md:245`:
   *"wire learned policy only as a narrowing/preapproval aid; it must never override hard
   denial or managed policy."* That is a wiring instruction with a constraint, not a
   deletion instruction.
2. **Deleting the field would not delete the capability.** `CapabilityId::LearnedPolicy` is
   on the wire and rendered by the TUI `/doctor` surface; removing an enum variant is a
   protocol break. Deletion would leave the advertised capability with even less behind it.
3. **The constraint is expressible as control flow, not as a comment.** The gate is
   consulted first and its denial is final; the learned policy sees only the survivors and
   can only move allow→deny. An `AllowAlways` rule therefore cannot resurrect a gate denial
   and cannot skip approval — it merely declines to narrow.

## M6 — Gates, with the falsification

- `cargo check -p wcore-agent --all-targets` rc=0, 0 error lines.
- `cargo check --workspace --all-targets` rc=0, 0 error lines. (Run because this touches a
  shared type — `ToolCallOutcome` gained a field; a `-p` check misses downstream users.)
- `cargo nextest run -p wcore-agent --test actor_acl_test`: **8 run, 8 passed, 0 skipped.**
  At base this binary ran **1 of 6** — five `#[ignore]`d cases and a guard.
- **Negative control, one variable.** Severing only the pre-filter's input
  (`let learned = cfg…` → `let learned … = None`) and changing nothing else:
  `NEGATIVE_CONTROL_RC=100`, **`sub_agent_with_deny_policy_short_circuits` FAILED**, the
  other 7 unchanged. Restored, and the restoration verified by **file content**
  (`grep -c` on both forms: severed 0, restored 1), not by `git diff`'s exit status, which
  is 0 unconditionally.

  The 7 that stayed green under severance is itself a result: it shows
  `allow_always_cannot_override_the_policy_gate` is a guard against escalation and NOT a
  proof of wiring, which is exactly what its doc comment claims.

## Status
- [x] instrument defect found and repaired before publication
- [x] F05 row 2 re-measured against the shipped binary — STALE, closed with live evidence
- [x] F05 row 4 wired, gated, and falsified
- [ ] live sub-agent deny against the real binary
- [ ] re-grade written
- [ ] ledger row corrected
