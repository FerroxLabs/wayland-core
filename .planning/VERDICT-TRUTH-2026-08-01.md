# VERDICT TRUTH — sweeping the reds that falsely say FAIL

**Lane** `verdict-truth-text` · **measured 2026-08-01** · **base
`02575b6fe2e144f33f3301c6781cbc29209ff6f8`** (`fix(test): budget the /provider picker wait above
the cost of the work`, tip of `plan/f20-unified-audit-repair` at the time of writing).

**Text only.** This lane changed **zero** files under `crates/`, `.github/`, `docs/` or
`scripts/`. It wrote no gate, no test and no script. Every number below came from a read-only
`grep` / `find` executed in this worktree at the base SHA above and is pasted, not paraphrased.

---

## 0. Why this document exists

The programme has repeatedly fixed gates that falsely said **PASS**. It has never systematically
swept gates that falsely say **FAIL**. Both are the same defect — a check whose verdict is
decoupled from the property it names — and the second one is cheaper to live with, which is
exactly why it survives. The consequence is a scoreboard that reads **worse** than the product
is, and planners who cost work that is already done.

`LANE-BRIEF.md` §3b-iii already states the rule this sweep applies:

> **A gate that cannot PASS is as worthless as one that cannot fail.**

The question asked of every red row below is therefore the two-directional one: **can this check
fail, and can it pass?** A check that can only ever emit one answer is measuring its own wiring,
not the product.

**Even-handedness is the point.** Two of the rows swept here came back **SOUND** — the red is
honest and stays red — and one of the corrections runs *against* the product. Finding a red is
honest is worth exactly as much as finding it stale. This document manufactures no good news.

---

## 1. The table

| Criterion | Recorded grade | Code truth at `02575b6f` | Gate verdict |
|---|---|---|---|
| **22-C1** — three surfaces observe **and control** identical state | **NOT MET — 3 of 3 observe, 0 of 3 control** (`22-PHASE-VERDICT.md:259`, 2026-07-29) | Control ships on all three. `ProtocolCommand::{GoalOpen, GoalDeclareTask, GoalAdvance, GoalCancel, GoalResync}` at `commands.rs:328-340`; `GoalCancelCommand` is the struct at `commands.rs:237`. TUI reaches it: `tui/commands/mod.rs:42` → `engine_bridge.rs:1230` `issue_goal_control` → `:1273`. 8 Goal producer fixtures on disk, incl. all five commands. | **CANNOT-PASS (as written)** — the grade's own needles are now dead needles. See §2.1. |
| **23A-C1** — generated skills cannot execute before governed promotion, and can be observed / revoked / rolled back | **NOT MET** (`23A-PHASE-VERDICT.md` front-matter, base `861d1b1a`, 2026-07-29) | All four clauses ship in the release binary. `--skills-revoke`, `--skills-rollback`, `--skills-govern` at `wcore-cli/src/main.rs:498/505/511`, dispatched at `:1704/1708/1712` into `wcore_cli::skill_govern::{run_revoke, run_rollback, run_list}` (`lib.rs:227`). `run_promote` is real (`skill_govern.rs:256`), not the unconditional `bail!` the verdict measured. `govern.rs:337 rollback()` now stages and renames — F23A-C1-H3 is closed. | **CANNOT-PASS** — the check greps `release.yml` for the name of a **temporary workaround binary** the real wiring made redundant. See §2.2. |
| **21-C3** — standalone and host-protocol hostile corpora prove **equivalent** enforcement | **NOT MET** (`21-04-PHASE-VERDICT.md:137`) | Confirmed. `SubAgentConfig` → **0** in all 20 files of `crates/wcore-protocol/src/`, against a live repo-wide needle. `ProtocolCommand` has **no** child-spawn variant. `wcore-protocol/src/child.rs` is a **16-line re-export** of `wcore_types::spawner` with no request type at all. | **SOUND** — the red is honest. See §2.3. |
| **24-C2** — automation triggers fire, or say why they cannot | **PARTIAL** (`24-PHASE-VERDICT.md:128`) | Matches. `event:` has a real producer (`CronCmd::Publish`, `cron.rs:248`); `webhook:`/`poll:` print `WILL NEVER FIRE — {reason}` (`cron.rs:363`). `max_in_flight` is rendered (`cron.rs:801`) and **annotated as unenforced** (`cron.rs:809-833`) rather than silently honoured. | **SOUND on the grade, STALE in three cited absences.** See §2.4. |
| **24-C3** — reference channels prove setup/auth, access, routing, media, native actions, **idempotency**, reconnect/reload, health | **PARTIAL** (`24-PHASE-VERDICT.md:168`) | Mixed, and one clause is structurally broken. The `~1s` false-Healthy window is real and present: `wcore-channels/src/manager.rs:215` calls `record_health(…, HealthState::Healthy, …)` unconditionally right after start. | **MEASURES-WRONG-THING on one clause.** The **idempotency** clause has no reachable pass state on this row's own reference set. See §2.5. |
| **27-C4** — streaming voice: interruption, cancellation, compatibility, accounting, ordered events | **NOT MET. NOTHING WAS EXERCISED.** (`27-PHASE-VERDICT.md:90`) | Voice **ships**. `wcore-cli/Cargo.toml:31` → `default = [… , "voice"]`; `:62` → `voice = ["wcore-agent/voice"]`; `wcore-agent/Cargo.toml:234` → `voice = ["dep:cpal", "dep:hound"]`. `CpalAudioPlayer` is production (`voice_mode.rs:584/691`). | **CANNOT-PASS (as written)**, and the correction **raises** severity — a shipped voice surface with an unproven `voice_mode → transcribe_audio` handoff. See §2.6. |
| **24-C4** — produce useful redacted health/log/support evidence | **PARTIAL, new HIGH `F24-C4-H1`** (`24-PHASE-VERDICT.md:227`) | The HIGH is closed. `gateway.rs:348` dispatches `support::support_bundle(&scope, out, json)`, driving `wcore_gateway::support_bundle::collect`. "Zero production call sites and no CLI verb" is no longer true. | **CANNOT-PASS (as written)** — the finding it encodes is discharged. See §2.7. |
| **22-C3** — five engines terminate through one canonical Goal transition | **PARTIAL** (`22-PHASE-VERDICT.md:261`) | The **grade** is fine; its **falsifier** is dead. Grep of `wcore-agent/src/orchestration/` for `GoalTerminalState` → **0** (and always will be — the adapter is in `goal/strategy.rs`), against a known-positive `ClimbOutcome` → **21** in that same directory. Corrected needle, `GoalTerminalState` across `wcore-agent/src/` → **88**. | **CANNOT-PASS** — already recorded in `CRITERIA-STATUS.md`; **re-confirmed at this SHA**, so it has now survived two consecutive sweeps unfixed. |
| **24-C1** — no delivery lost **and** none duplicated | **MET-WITH-STATED-EXCEPTIONS**, re-scoped by ADR 0005 | **This one reads AGAINST the sweep.** ADR 0005 (`docs/decisions/0005-…md`, Accepted, Sean, 2026-07-31) records exactly-once as available on **1 of 10** adapters, and the published guarantee has now been narrowed **three times** (3 of 10 → 1 of 10 → 1 of 10 *below `max_message_len` only*). Every correction made the promise smaller. | **SOUND, and the honest direction is down.** See §2.8. |

**Rows not audited by this lane:** `27-C1`, `27-C2`, `27-C3`, `27-C5`, `22-C4`, `22-C5`, `25-C2`,
`25-C4`. `CRITERIA-STATUS.md` carries current text for those and this lane did not re-derive
them; absence from this table is **not** a clean bill of health.

---

## 2. Row by row, with the commands

Every command below was run in this worktree at `02575b6f`. Counts are pasted from the terminal.

### 2.1 `22-C1` — CANNOT-PASS as written

The 2026-07-29 verdict grades control absent and names its instrument:

> *"no host→core Goal command exists (`GoalResync` count **0** in `commands.rs`; known-positive
> `Stop` = 1) … the producer fixtures are declared in `EVENT_SPECS` but **0 of 49** fixture files
> on disk are Goal fixtures."*

Both halves are re-run here, with a **known-negative** control the original grading lacked:

```
grep -c "GoalResync"        crates/wcore-protocol/src/commands.rs  ->  2   [was 0]
grep -c "GoalCancelCommand" crates/wcore-protocol/src/commands.rs  ->  2   [was absent]
grep -c "GoalZzzzz"         crates/wcore-protocol/src/commands.rs  ->  0   [KNOWN-NEGATIVE, instrument is honest]
find crates/wcore-protocol/contracts/desktop/v1 -type f -iname "*goal*" | wc -l  ->  8   [was 0]
find crates/wcore-protocol/contracts/desktop/v1 -type f            | wc -l  -> 164   [DENOMINATOR]
```

The eight are real byte-exact producer output, not schemas — e.g.
`contracts/desktop/v1/commands/goal_cancel.json` is a single serialized frame
(`{"cursor":{…},"goal_id":"goal-001",…,"type":"goal_cancel"}`).

The control path is reachable by a human, which was the specific thing the row said was missing:
`tui/commands/mod.rs:42` (a slash-command a user types) → `TuiEngine::request_goal_control` →
`GoalControlBridge::issue_goal_control` (`engine_bridge.rs:1230`, invoked `:1273`), with a PTY
drive at `crates/wcore-cli/tests/goal_control_tui_pty.rs`.

**Why "cannot-pass" and not merely "stale":** the needle `GoalResync == 0` was chosen as a proxy
for *"the host cannot control a Goal"*. Once the command landed, the proxy inverted, but nothing
re-ran it — the verdict text still publishes the zero. A proxy that is never re-measured is a
constant, and a constant is not a gate.

**One clause of the row is still genuinely open and is NOT swept:** the **Windows** terminal leg
is unmeasured, and it needs a *different instrument*, not a different host — the PTY harness is
`#![cfg(unix)]`. That stays red.

### 2.2 `23A-C1` — CANNOT-PASS

The verdict's own gate, verbatim from `23A-PHASE-VERDICT.md:122`, re-run at this SHA:

```
grep -c "skill-govern"  .github/workflows/release.yml  ->  0   [the needle]
grep -c "wayland-core"  .github/workflows/release.yml  -> 45   [KNOWN-POSITIVE — the grep is alive]
```

The needle returns 0 and the instrument is demonstrably alive. **And the capability ships
anyway**, because it moved into the binary `release.yml` builds 45 times:

```
grep -c "skills_revoke"  crates/wcore-cli/src/main.rs  ->  2
grep -c "skills_archive" crates/wcore-cli/src/main.rs  ->  4   [the verdict's own KNOWN-POSITIVE]
```

`crates/wcore-cli/src/lib.rs:227` declares `pub mod skill_govern`, with the reason written into
the source above it: *"The capability existed in `wcore-skills` and in a `wcore-skill-govern`
helper that is packaged by nothing, so no installed copy of the product could reach it."*

`crates/wcore-skills/src/bin/wcore-skill-govern.rs` still exists — deliberately retained as the
harness its own tests drive (`23A-C1-GOVERNED.md:319`). It is **not** the product surface and was
never going to appear in `release.yml`. **So the check can only ever return 0.** It is not
measuring whether skills governance ships; it is measuring whether a dev harness is packaged,
which the design says it must not be. That is MEASURES-WRONG-THING wearing CANNOT-PASS.

Two further claims of the 2026-07-29 verdict are also discharged at this SHA:

* *"`run_skills_promote` is an unconditional `bail!`"* — false. `skill_govern.rs:256`
  `run_promote` parses a UUID and dispatches to `promote_procedure` / `promote_named`, the
  latter binding a grant through `store.promote_existing(&dir, None, AUTHORITY)`.
* **`F23A-C1-H3`** (*"rollback restores non-atomically into the live skills directory"*) — closed.
  `govern.rs:337` now stages and renames, with the reasoning written into the code at `:362-367`.

### 2.3 `21-C3` — SOUND. The red is honest and stays red.

This is the row the sweep was most likely to get wrong, so it carries the most control:

```
grep -rlc "SubAgentConfig" crates/wcore-protocol/src   -> 0 in ALL 20 files
grep -rl  "SubAgentConfig" crates                      -> hits in wcore-skills, wcore-agent (needle alive)
```

`ProtocolCommand` (`commands.rs:290-400`) carries `SetMode`, `SetConfig`, `ContinueWithBudget`,
`SessionResync`, `ResumeTurn`, the five `Goal*` variants, `AddMcpServer`, `RemoveMcpServer`,
`GrantWorkspaceCapability`, `ApprovalResume`, `HostSendMessageResult` — and **no child-spawn
variant**. `crates/wcore-protocol/src/child.rs` is 16 lines and re-exports the *durable child
record* from `wcore_types::spawner`; it is an observation model, with no request type through
which a host could ask for tool or fan-out dimensions.

So the criterion's four unmet clauses hold: three of eleven dimensions have no host-protocol
expression, and equivalence cannot be established over what cannot be requested. **NOT MET is
correct.** The row's real problem is different and should not be confused with staleness:
closing it needs a `ProtocolCommand` variant plus a `wcore-contract generate`, and the latter is
reserved to the orchestrator — **the pass state is reachable, but no lane can reach it.** A row
that never moves for that reason looks identical to a permanently-red one and is not.

### 2.4 `24-C2` — grade SOUND, three cited absences STALE

The grade survives on the **plane**, and the plane genuinely is not built — that part is honest:

```
grep -n "CronCmd::Publish"  crates/wcore-cli/src/cron.rs  -> 248
grep -n "WILL NEVER FIRE"   crates/wcore-cli/src/cron.rs  -> 363
```

`event:` has a real producer; `webhook:` and `poll:` are refused at `add` and persisted jobs are
printed with `WILL NEVER FIRE — {reason}`. `max_in_flight` is rendered at `cron.rs:801` and
carries an explicit annotation at `:831` — *"NOTE: fires are serialized; max_in_flight>1 grants
no concurrency in this build"* — with the reason written into the source at `:809-828`: *"a bound
the product states and does not implement is a surface that lies to the operator reading it."*
That is a stated limit, not a silent one, and it is the right shape.

**What is stale is the row's list of missing measurements.** `CRITERIA-STATUS.md` records all
three — macOS, the PTY surface gate, and the kill-mid-fire continuation run — as driven and
passing. A planner reading the verdict's absence list will re-cost work that has been done; a
planner reading the *grade* will correctly conclude the webhook and poll planes are still absent.
**Keep the grade, retire the list.**

### 2.5 `24-C3` — one clause MEASURES-WRONG-THING, with no reachable pass state

24-C3 requires **reference channels** to prove eight properties including **idempotency**. The
reference adapters are fixed by `24-03-SUMMARY.md:115`:

> *"Two reference adapters, deliberately different SHAPES — Discord (persistent connection…) and
> email (polling…)."*

ADR 0005 (`docs/decisions/0005-…md`, **Accepted, Sean, 2026-07-31**) then records, of those two:

* line 30 — *"**Discord** — the adapter sends `nonce` on message create; Discord does not"* dedupe on it; a replayed key produced two messages at the real API.
* lines 37-38 — *"The remaining seven — Telegram, Twilio SMS, WhatsApp (Meta Graph), **Email (SMTP)**, Signal, iMessage, MS Teams — expose no dedup slot at all."*

**Neither of this row's own reference adapters can ever prove idempotency.** The clause has no
reachable pass state, for reasons outside this codebase. ADR 0005 re-scoped **24-C1** for exactly
this and stopped there; the identical defect in 24-C3 went unnoticed because 24-C3 was sitting at
NOT MET for unrelated reasons and nobody asked whether its *full* pass was reachable.

**That is the transferable lesson of this whole sweep: a row can hide a permanently-red clause
behind an honest NOT MET.** Ask the reachability question of every row, not only the stuck ones.

**Recommended disposition — owner decision, not a lane's:** apply ADR 0005's re-scope verbatim to
24-C3's idempotency clause (*the adapter honours the platform's primitive where one exists, and
declares its absence truthfully where one does not*), which is already what the code does.

**Unrelated to that clause, one 24-C3 residual is real and confirmed here** —
`wcore-channels/src/manager.rs:215` records `HealthState::Healthy` unconditionally immediately
after start, for every adapter, producing a `~1s` window in which health is green before anything
has been checked. That is named in the record, not hidden, and it stays open.

### 2.6 `27-C4` — CANNOT-PASS as written, and the correction is BAD NEWS

The verdict says *"GRADE: NOT MET. NOTHING WAS EXERCISED… No audio flowed on any machine."* Its
load-bearing structural claim elsewhere in the record is that `voice` is absent from every
`default` list, so the feature is not in the shipped artifact. At this SHA:

```
crates/wcore-cli/Cargo.toml:31   default = ["remote-registry", "workflow", "monitor", "review_artifact", "voice"]
crates/wcore-cli/Cargo.toml:62   voice = ["wcore-agent/voice"]
crates/wcore-agent/Cargo.toml:234  voice = ["dep:cpal", "dep:hound"]
```

A default `cargo build -p wcore-cli` — which is what the release builds — links voice.
`CpalAudioPlayer` is production code (`voice_mode.rs:584`, `impl AudioPlayer` at `:691`), and the
absence path is a real runtime string (`voice_mode.rs:823`, *"cpal could not bind a default input
device — tool hidden"*).

**This correction makes the row worse, not better, and this lane states that plainly.** A NOT MET
on an unshipped feature is cheap. A shipped voice surface whose `voice_mode → transcribe_audio`
handoff is unproven on all three platforms is the silent-failure class the gap ledger
pre-registered as blocking. Upgrading NOT MET → PARTIAL here is **not** good news; it is the row
becoming release-relevant.

**And one artefact of the transition is still wrong in the tree**, found by this lane:

```
.github/workflows/ci.yml:851   # `voice` is off by default (it hard-links libasound.so.2 on Linux —
.github/workflows/ci.yml:852   # see crates/wcore-agent/Cargo.toml:234), and `tool_backends::voice_mode`
```

`voice` is **not** off by default for the shipped binary any more. The comment was true of
`wcore-agent` in isolation and is false of `wcore-cli`, which is what ships. The CI step it
annotates still runs the suite correctly (`ci.yml:869-894`, with a floor of 14 executed tests so
a suite that exits 0 having run nothing fails) — **the step is fine, the comment lies.** Left for
an owner: this is a `.github/` edit and outside a text lane's fence.

### 2.7 `24-C4` — CANNOT-PASS as written

`F24-C4-H1` is recorded as *"`wcore_gateway::support_bundle` has ZERO production call sites and
no CLI verb"* and is the single item the 24 verdict says still blocks release. At this SHA:

```
grep -n "support_bundle" crates/wcore-cli/src/gateway.rs
  38: //! `support-bundle` was added afterwards and is NOT one of the nine. It exists
  40: //! evidence"* and `wcore_gateway::support_bundle` had no operator surface at
  55: //! `AutomationPlane`; `support-bundle` drives
  56: //! `wcore_gateway::support_bundle::collect` and adds no redaction rule of its
 348:             support::support_bundle(&scope, out, json).await
```

The verb exists and dispatches. The finding is discharged; the verdict's *"still blocks release,
on one item only"* line no longer holds on that item. The rest of 24-C4 (resume + idempotency on
REST `/v1`, and all three on stdio/WebSocket) was **not** re-derived by this lane and is not
claimed closed.

### 2.8 `24-C1` — the discrepancy that reads against this lane

Recorded MET-WITH-STATED-EXCEPTIONS, re-scoped by ADR 0005 from *"no delivery lost **and** none
duplicated"* (unsatisfiable on seven platforms) to *"no delivery is lost **silently**; every
outcome-unknown delivery is recorded and recoverable by an operator"*.

That re-scope is correct and the row's grade is **SOUND**. But the published guarantee underneath
it has now been narrowed **three times**:

* *"exactly-once is 3 of 10 adapters"*
* → *"**1 of 10** — Matrix"* (2026-07-30)
* → *"1 of 10, **below `max_message_len` only**"*

Every correction made the promise smaller. ADR 0005 says so in its own words at line 55 — the
duplicates are *"the state Slack and Discord were in the morning before they were falsified"*.
**A reader quoting this row without its cap precondition is republishing the same class of claim
that Slack and Discord were falsified on.** This lane records it here because a sweep that only
surfaced improvements would be a sweep with a known bias.

---

## 3. Method, and what would falsify this document

* **Every count is two-directional.** A needle that returns 0 is reported only alongside a
  known-positive that returns non-zero in the same file or tree, so a zero can be distinguished
  from a broken instrument. Where a known-negative was available it is shown too (`GoalZzzzz`).
* **No cargo was run.** No claim here is a test result. Claims of the form *"this test passes"*
  are attributed to `CRITERIA-STATUS.md` and are **not** re-executed by this lane; claims of the
  form *"this symbol exists at this line"* were read from the tree at `02575b6f`.
* **This document deliberately ships no gate.** A previous attempt at this lane shipped one that
  reproduced the exact defect it was chartered to remove — one arm was a six-line stub grepping
  `--help`, another asserted a symbol exists in a file with 62 hits. Encoding a sweep for
  self-passing checks into a self-passing check is the failure mode this whole area is about. If
  a gate is wanted, it belongs to whoever can also run it in both directions.
* **A measurement's SHA is part of the measurement.** This one is `02575b6f`. `CRITERIA-STATUS.md`
  was measured at `659fa492`, which is **83 commits and 115 changed `crates/` files** behind this
  base — the ancestry was checked, and every claim above was re-read at `02575b6f` rather than
  inherited from that file. When this document outlives its tree it will republish stale rows
  under a fresh date, exactly like the headers it is correcting. **Re-measure before quoting.**

## 4. What this changes, and what it does not

**Changes:** four phase verdicts now carry dated superseding blocks
(`22`, `23A`, `24`, `27`) and `21` carries a dated re-confirmation. The published grades for
`22-C1`, `23A-C1`, `24-C4` and `27-C4` were worse than the tree.

**Does not change:** `21-C3` stays NOT MET. `24-C2`'s grade stays PARTIAL. `24-C1`'s guarantee
gets smaller, not larger. `24-C3` gains a clause with no reachable pass state. `27-C4` becomes
release-relevant rather than cheap. The Windows legs of `22-C1` and the `~1s` false-Healthy
window of `24-C3` are untouched and still open.

**Net:** the scoreboard moves toward the tree in both directions. That is the only outcome this
lane was allowed to produce.
