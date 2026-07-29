---
lane: lane/22-remaining
base: 5457710e5bccd7c91a117f055ed42531bc2327bb
scope: "re-grade Phase 22; close 22-02 Task 3's two capability rows; then whatever the re-grade shows is closeable"
regrade: "C1 NOT MET (3/3 observe, 0/3 control) · C2 PASSED · C3 PARTIAL (5/5 production paths) · C4 PARTIAL (wider than recorded) · C5 PARTIAL · GOAL NOT ACHIEVED, third time"
f05-row-2: "STALE, not unwired — both columns false; corrected with live evidence off the shipped binary"
f05-row-4: "genuinely unwired AND an API lie (pub field, zero readers) — wired narrowing-only, live-proven, falsified"
ledger: "GOAL-* CONSTRUCTED -> REACHED on the ledger's own recorded deciding test; panel 3-0 + adversarial dissent preserved"
fence: "crates/wcore-cli/src/{lib,main}.rs UNTOUCHED — empty diff control-tested against a file I did change"
status: complete
---

# 22-remaining — the two dead capability rows, and an honest re-grade of Phase 22

Lane `lane/22-remaining`. Base `5457710e` (captured once, quoted everywhere). Built, tested and
live-proven on `hetzner-dsm`; every live leg against the shipped `wayland-core 0.12.25`
**release** binary, Linux.

---

## 0. Headline

| | |
|---|---|
| **F05-TRUTH-2** (mid-flight monitor) | **was STALE, not unwired.** Both columns of the ledger row were false. Corrected with live evidence and a one-variable negative control. |
| **F05-TRUTH-4** (learned policy) | **was real, and understated.** Wired as a narrowing-only sub-agent pre-filter; live-proven parent-vs-child in one run; falsified at compile time. |
| **Phase 22 re-grade** | every criterion moved; goal **still NOT ACHIEVED**, and it now fails on one word — *supervise*. |
| **`GOAL-*` ledger row** | **CONSTRUCTED → REACHED**, on the test the ledger itself wrote down against this row. |
| **Deliberately left open** | Criterion 1's control half, C3's structural half, C4's reconnect/preemption clauses, C5's Windows legs, and one new limitation I found in my own change. |

---

## 1. F05-TRUTH-2 — the row was stale, and my first measurement nearly agreed with it

The ledger says *"Mid-flight monitor | Unavailable: runtime path unwired | None"*, and cites it
as a checkable blocker on `GOAL-*`. **Both columns are false**, from the shipped binary's own
JSON stream:

```
mid_flight_monitor: declared → configured → constructed → ready → reached → outcome_changed → observed
{"type":"mid_flight_monitor_decision","directive":"replan","reason":"repeated_error"}
```

Driven by five real `Read` dispatches whose root-cause signatures collapse to one, against a
local canned OpenAI-compatible endpoint (no credential; the provider identity is read back from
the product's own log line and the endpoint's request log, per LANE-BRIEF §3b-ii).

**One-variable negative control:** taking the identical-error count from 3 (`REPEAT_THRESHOLD`)
to 2 takes `mid_flight_monitor_decision` **1 → 0** and the occurrence **1 → 0** while `ready`
stays **1**. Both arms exit `PRODUCT_RC=0`, so a gate asserting exit status would have
distinguished nothing.

**How close this came to shipping the wrong answer.** My first sweep for the monitor's call
sites ended in `| head -30`. `channel_lease.rs` has ~20 `.tick()` hits and sorts before
`engine.rs`, so the cap truncated the output *before* the two real production calls — and
returned "test file only", which is exactly what the ledger asserts. **A truncated pipe
manufactures an absence for free, and the absence it manufactured agreed with my brief.** Caught
before publication; the repair (file-first, never `| head` on an absence, known-positive control
in the same invocation) is in `22-REMAINING-NOTES.md` §M0 and was applied for the rest of the
lane.

Full record: `22-REMAINING-EVIDENCE/midflight/RESULT.md`.

## 2. F05-TRUTH-4 — the row was real, and the lie was in the API rather than the report

The capability report was honest (`unavailable / runtime_path_unwired`). **The API was not.**
Measured from a file, with the control repaired after my first known-positive returned 0 because
rustfmt splits the expression across lines:

| | count |
|---|---|
| control `&cfg.tools` (read in the same function) | 3 |
| control `\.policy_gate` (proves the pattern is greppable) | 3 |
| **`cfg.learned_policy` — any read** | **0** |
| **`\.learned_policy` in ANY form** | **0** |
| `CallActor::SubAgent` constructed outside tests/docs | **0** |

So `AgentExecutorConfig` carried a `pub learned_policy` field with **zero readers in the entire
workspace**, whose own doc comment said `dispatch_once` ran every tool call through it. Setting
it did nothing. The removal note pointed at `52b1ae2~..HEAD` to restore from; **that revision
does not exist** (this repository's history begins at a squashed root, `da5a18b5`,
`rev-list --count` = 1), so the pre-filter was written fresh against `actor_acl_test.rs`, its
surviving spec.

**Decision: wire it, narrowing-only — not delete it.** Three grounds, weightiest first: the
2026-07-13 gap audit §4 already decided (*"wire learned policy only as a narrowing/preapproval
aid; it must never override hard denial or managed policy"*); deleting the field would not delete
the capability, which is on the wire and in the TUI `/doctor` surface, so deletion would leave an
advertised capability with even less behind it; and the constraint is expressible as **control
flow** rather than as a comment.

That last point is the design: `filter_tool_calls_by_policy` consults the policy gate **first**
and its denial is final, then offers only the survivors to the learned policy, which can only
move allow → deny. **An `AllowAlways` rule cannot resurrect a gate denial and cannot skip
approval.**

### Live — parent and child, one run, one variable

```
last_tool_result[parent:2] = "     1\tparent probe content"
last_tool_result[child:2]  = "Denied by sub-agent learned policy: Read matched rule `*`"
```

Same run, same on-disk `permissions.toml`, same tool, same argument shape. The only difference is
the caller class. The child's tool results never reach the parent's JSON stream, so they are read
back from **the product's own conversation state** — the engine feeds each result into the
child's next provider request, which the canned endpoint logs verbatim.

Control arm, one variable (`POLICY=0`, no policy file): the child **reads** the file, and the
capability reports `unavailable / disabled_by_config` instead of `ready`.

Full record: `22-REMAINING-EVIDENCE/learnedpolicy/RESULT.md`. Both proofs reproduced against the
HEAD binary.

## 3. The gates, with the falsification

| Gate | Result |
|---|---|
| `cargo fmt --all` (Mac) | clean |
| `cargo check -p wcore-agent --all-targets` | **rc=0, 0 error lines** |
| `cargo check --workspace --all-targets` | **rc=0, 0 error lines** — run because this touches a shared type (`ToolCallOutcome` gained a field); a `-p` check is blind to downstream users |
| `cargo clippy -p wcore-agent -p wcore-permissions --all-targets --all-features -- -D warnings` | **rc=0, 0 error lines** — first run was **RED at 101** (`then` → `then_some`), fixed |
| `cargo nextest run -p wcore-agent` | **3096 run, 3096 passed, 6 skipped** (at base: 3079 run, 11 skipped) |
| `cargo nextest run -p wcore-permissions` | **53 run, 53 passed, 1 skipped** |
| `cargo nextest run -p wcore-agent --test actor_acl_test` | **8 run, 8 passed, 0 skipped** — at base this binary ran **1 of 6** |
| **Negative control** | severing only the pre-filter's input, changing nothing else: `NEGATIVE_CONTROL_RC=100`, **`sub_agent_with_deny_policy_short_circuits` FAILED**, other 7 unchanged. Restored; restoration verified by **file content** on both forms (severed 0, restored 1), not by `git diff`'s unconditionally-zero exit status |

The 7 that stayed green under severance is itself a result: it shows
`allow_always_cannot_override_the_policy_gate` is a guard against *escalation*, not a proof of
wiring — exactly what its doc comment claims.

**No test was weakened.** Five `#[ignore]`d cases were **un**-ignored, two cases added, and the
suite's zero-execution guard was inverted so it now fails if anything here is `#[ignore]`d back
into inertness.

## 4. The re-grade — `22-PHASE-VERDICT.md`, superseding section 3

Graded from source and from the shipped binary, never from a prior summary. The full table is in
the verdict file; the changes that matter:

- **C1 → NOT MET, 3 of 3 observe, 0 of 3 control.** All three surfaces exist now. Two clauses
  fail and both are measured: **no host→core Goal command exists** (`GoalResync` count 0 in
  `commands.rs`; known-positive `Stop` = 1), and the producer fixtures are declared in
  `EVENT_SPECS` but are **0 of 49** files on disk.
- **C3 → PARTIAL, was recorded FAILED.** The 2026-07-27 section predates two lanes. Five engines
  now have a production path to one canonical transition, live-proven. Not PASSED: attachment is
  opt-in and zero engine signatures changed.
- **C4 → the 2026-07-27 grade is measurably wrong.** It says *"`Dynamic`, `EventDriven` and
  `Manual` still have no runtime enforcement."* `GoalAuthorityRecord::iteration_ceiling` returns a
  numeric ceiling for `Once`, `Fixed`, `Dynamic` **and** `EventDriven`, and
  `session_journal/reducer.rs:326` refuses `GoalIterationStarted` past it at the durable boundary.
  Only `Manual` has no ceiling, by design. Still PARTIAL: reconnect, preemption and missed
  intervals are untouched by anyone.

**Goal: NOT ACHIEVED, a third time — and the reason is different each time.** 2026-07-26: nothing
was reachable. 2026-07-27: Criterion 3 was untouched. **2026-07-29: it fails on one word.** A user
can open a durable Goal, drive any of five engines through it, `kill -9`, restart, and see exactly
one termination — *from a terminal*. What they cannot do is **supervise** it from the TUI or a
host: both are read-only. "Supervise" is not "observe".

## 5. The ledger — `GOAL-*` CONSTRUCTED → REACHED

The 2026-07-28 refresh wrote down its own deciding test **against this row**: `REACH-*` was
promoted because it *"carries two-platform live product exercise on the criterion that passed
and — unlike GOAL-\*, has no mapped F05 identity recorded `runtime path unwired`."*

Both conjuncts now hold for `GOAL-*`: the F05 blocker is gone (half of it never real), and the
two-platform live exercise was already there — `F22-FLEET-WIRE`, which this ledger already called
*"REACHED-kind on its own"*. Two more of the five recorded blockers are simply no longer true
(C3 *"never attempted"*; C1 *"failed on two of three surfaces"*).

**Panel 3-0 REACHED** — codex gpt-5.6-sol, gemini 3.1-pro-preview, kimi K3, each given the
ledger's own blockers verbatim, each vote extracted unanchored from its own capture
(`22-REMAINING-EVIDENCE/panel/`). **Plus an internal adversarial pass that argued to hold at
CONSTRUCTED and lost**; its surviving case is preserved inside the row, not discarded: the
previous refresh listed *"goal NOT ACHIEVED"* and *"every requirement OPEN"* as blockers in their
own right, and `GOAL-*`'s passed criterion is one member of five while the family's **namesake**
concept is PARTIAL. It lost because both points argue against EFFECTIVE, not REACHED — `REACH-*`
itself promoted with three of four criteria NOT MET, so criterion incompleteness was already
ruled non-decisive by this ledger.

**Not EFFECTIVE, and the boundary is stated in the row.**

## 6. Four instrument defects in my own tools, all repaired in-lane (§6b-ii)

1. **`| head -30` manufactured a false absence** that agreed with my brief (§1). Repaired:
   file-first, never `| head` on an absence.
2. **A known-positive returned 0** — `cfg\.policy_gate` fails because rustfmt splits it across
   lines. The control was dead; replaced with two live ones before publishing any zero.
3. **A poll loop reported BUILD DONE on a running build.** `grep -c X f 2>/dev/null || echo 0`
   emits `0\n0` when the pattern is absent, which `!= "0"` reads as true. Repaired to `grep -q`
   with a **three-assertion** self-test: known-positive YES, known-negative NO, **and the old
   matcher on the same file returns the two-line `0 0` that made it fire**.
4. **`rtk` rewrites `ls` too** — it adds a size column and reorders, so `ls | grep -c '^ready.json$'`
   returned 0 for a file that exists. LANE-BRIEF §3b documents this for `git`, `grep` and `cargo`;
   **`ls` is a new member of the class.** Re-measured with `/bin/ls` under a three-assertion
   self-test. The load-bearing number (0 Goal fixtures of 49) is stable across both instruments.

## 7. Advertised-but-dead sweep of the Goal surface — clean

The class this lane exists to close, so I checked whether the surface I was grading carried any
more of it. Across all eight Goal source files (`goal_cmd.rs` + `wcore-agent/src/goal/*.rs`):
**0** matches for `not implemented` / `todo!` / `unimplemented!` / `TODO` / `FIXME` /
`placeholder`. Known-positive control: the same matcher finds TODOs in three other files in the
same crate, so the matcher is alive. `durable_goals_v1` is registered **`ShapeOnly`**, which
honestly matches a read-only surface.

## 8. What I deliberately did NOT do, and why

1. **Criterion 1's control half — the host→core Goal command.** This is the highest-leverage item
   left in the phase and I am naming it open rather than half-landing it. Two reasons, and the
   second is decisive: it is capability *breadth* (a new host command surface) rather than a
   defect, since the shipped `wayland-core goal` CLI path works; and **a `ProtocolCommand` variant
   needs a contract regeneration this lane may not run**, so landing one would produce a command
   the host cannot negotiate — *the advertised-but-dead class, committed by the lane hired to
   close it.* It belongs with seam request `SR-22-C1`'s single regeneration pass.
2. **Criterion 3's structural half** — making an engine incapable of terminating outside a Goal
   means threading a token through five entry points and changing five signatures. Breadth against
   an already-PARTIAL working path, with real blast radius, before a deadline.
3. **C4's reconnect / preemption / missed-interval clauses** and **C5's Windows M1–M5 legs** —
   both previously listed, neither attempted here.
4. **A Windows leg for anything in this lane.** Linux only. Stated plainly.
5. **Two further stale F05 rows** found in the same capture, both `CONT-*` and neither mine:
   `cooldown_tracker` reports **`ready`** where the receipt says "no production constructor", and
   `pricing_refresher` reports **`disabled_by_config`** where the receipt says the same. Flagged
   in the ledger amendment, **not edited** — re-grading `CONT-*` belongs to its owner. The pattern
   is worth naming: that table has now been wrong about **three of eight rows in the same
   direction — understating what the product does** — because it was transcribed once in July and
   re-asserted at each refresh without re-reading the binary.

## 9. A limitation I introduced and am NOT counting as closed

`emit_learned_policy_occurrence` fires on a real narrowing, but **no current topology can observe
it.** `OutputSink::emit_capability_activation` (`output/mod.rs:240`) is a **default no-op** that
only `ProtocolSink` overrides, and every spawned child gets `NullSink` (`Delegate`) or
`ChannelSink` (`Spawn` / workflow runner) — neither overrides it. Since `Root` bypasses the
pre-filter by design, the occurrence can only ever fire inside a child, and every child discards
it.

So `F05-TRUTH-4`'s **runtime outcome proof column stays `None` in practice**, and I have said so
in the row, in the truth table and here rather than letting the startup-truth close imply both.
Generalised, and this is the useful form: **no sub-agent capability activation of any kind is
observable on any topology in this tree.** The fix — `ChannelSink` forwarding activations to the
parent — needs a relay event and therefore the same contract regeneration.

## 10. For the orchestrator to serialize

- **Shared fence: NOTHING.** `crates/wcore-cli/src/lib.rs` and `crates/wcore-cli/src/main.rs` are
  **untouched**. Measured with `git diff --numstat` against the merge-base **SHA** captured once
  (`5457710e`), never against the branch name. **The empty result is control-tested**: the
  identical command against a file I did change returns `40  31`.
- **`ToolCallOutcome` gained a public field** (`learned_policy_denials: usize`). Any lane
  constructing it needs the field. Four construction sites in-tree, all updated;
  `cargo check --workspace --all-targets` is 0 errors at HEAD.
- **`AgentSpawner` gained `with_learned_policy`** and **`AgentEngine` gained
  `set_call_actor` / `set_learned_policy`** — all additive, all `None`/`Root` by default, so a
  session with no policy on disk keeps byte-identical dispatch.
- **`filter_tool_calls_by_policy`'s signature changed** (crate-private): it now takes
  `Option<&PolicyGate>` and `Option<&LearnedPolicy>`.
- **No new `ProtocolEvent` or `ProtocolCommand` variant. `wcore-contract generate` was NOT run.
  No new seam request is owed by this lane** — but `SR-22-C1` (already fenced in
  `22-C1-SUMMARY.md` §6) now has two more things to fold into its single regeneration pass: the
  host→core Goal command (§8.1) and `ChannelSink` capability-activation forwarding (§9).
- **`.planning/intel/COMPETITIVE-LEDGER.md` was edited** — one row re-graded, the F05 truth table
  amended, one superseding pointer added to a historical table, and a dated single-row refresh
  section appended. If another lane also edits that file, this is the conflict surface.
