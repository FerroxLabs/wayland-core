# CRITERIA GAP LEDGER — what the plan count hides

**Measured**: 2026-07-28, lane `lane/criteria-gap`
**Tree measured**: `plan/f20-unified-audit-repair` @ `873cc389` (includes the `lane/25-cloud` merge of today)
**Method**: every criterion quoted verbatim from `.planning/ROADMAP.md`, then re-measured against
the current tree, tests and evidence artifacts. **Where a phase verdict and the tree disagree, the
tree wins and the disagreement is stated.**

---

## 0. Headline

**18 Success Criteria across 6 phases are open (NOT MET or PARTIAL) and no remaining plan covers
any of them.** The remaining plan queue is `26-04`, `28-04`, `30-01…04` — six plans, none of which
touches Phases 21, 22, 23A, 24, 25 or 27.

| | |
|---|---|
| Open criteria | **18** |
| Release-blocking (see §3) | **7** |
| Estimated total | **33–47 lane-sessions**, midpoint **≈38** |
| Estimated to a release candidate | **13–17 lane-sessions** + one coordinated Core+Desktop release train |
| Blocked on Sean | **1** (a scoping decision, not work) — see §4 |

**Plan count is not progress.** 60 of 66 plans have summaries — 91% — while the criteria those
plans exist to satisfy are 18 short. The gap between those two numbers is roughly **38
lane-sessions of unscheduled work**, which is comparable to the entire executed Phase 20 campaign.
Anyone reading "60 of 66" as "nearly done" is reading a number that does not measure the thing they
care about.

### Corrections this measurement had to make to the tracking documents

Four of the six phases were graded against trees that have since moved. Every correction below is
in the direction the handoff warned about — the documents were stale, in both directions.

| Document | What it says | What the tree says |
|---|---|---|
| `HANDOFF-2026-07-28.md:34` | Phase 21 "**NOT ACHIEVED** (graded three times)" | The third grading, `21-REVERIFICATION.md` (2026-07-27), **upgraded SC1 to MET WITH STATED EXCEPTIONS** and SC2 likewise. Only **SC3** is open. Verified by me at `873cc389`: `spawner.rs:2718` carries the unconditional tool intersection, `spawner.rs:2302` is a live production caller of `set_policy_gate`. Phase 21 has **1** open criterion, not 3. |
| `24-PHASE-REPORT.md:12` | "Four of the five Success Criteria have no evidence at all" | Written 2026-07-26, **before lanes 24b/24c/24d/24e**. Since then: `crates/wcore-cli/src/gateway.rs` (48 KB) exists and is wired at `main.rs:735`; C4 is **MET on Linux**; C1's delivery-arrival half is **closed on Linux**; F24-B-H1 (the profile-isolation defect) is **FIXED**. |
| `24-02-SUMMARY.md` §"NOT delivered" item 6 | "`crates/wcore-cli/src/gateway.rs` was not created … the phase's largest structural hole" | **Closed.** Created 2026-07-27 in the 24-03 lane; `install/uninstall/start/status/drain/run` all present. |
| `25-PHASE-STATUS.md:102` | No SSH trust between the two physical hosts, "**Reserved to Sean**", blocking Criteria 2 and 4 | **CLEARED.** Measured live today: `ssh hetzner-dsm 'ssh -o BatchMode=yes SeanD@seandesktop hostname'` → `SeanDesktop`, `RC=0`. Both blockers are gone; the work is now takeable without Sean. |
| `22-PHASE-VERDICT.md` (2026-07-27 update):173 | Criterion 4: "`Dynamic`, `EventDriven` and `Manual` still have no runtime enforcement" | Partly stale. `goal/record.rs:143-151` returns an `iteration_ceiling()` for `Once`, `Fixed`, `Dynamic` **and** `EventDriven`, and `session_journal/reducer.rs:269-276` enforces it durably for all four. The real remaining gap is narrower and different: only `goal/fleet.rs:475` calls `start_iteration`, so **no non-Fleet engine is bounded at all**. |

---

## 1. The ledger

Every criterion below is quoted verbatim from `.planning/ROADMAP.md` at `873cc389`.

---

### PHASE 21 — Child Authority and Budget Inheritance

#### 21-C3 — NOT MET

> **"Standalone and host-protocol hostile corpora prove equivalent enforcement."** (`ROADMAP.md:81`)

**Grade measured: NOT MET.** Unchanged since `21-04-PHASE-VERDICT.md`, re-confirmed by
`21-REVERIFICATION.md:264-284` at `ac94b1d5` and re-checked by me at `873cc389`.

**Direction of movement since the phase verdict: neutral on this criterion, strongly positive
around it.** Sibling criteria SC1 and SC2 both moved up (see §0). SC3 did not, and the
reverification says plainly that the repairs "did not touch, and did not claim to touch" any of its
three clauses.

**The specific unmet clause: the word *equivalent*.** Three of eleven dimensions have **no
host-protocol expression at all** — the host child-spawn request type carries only
`[name, prompt, max_turns, max_tokens, system_prompt, provider, model, temperature]`, and
`spawn_host_child` hardcodes `ForkOverrides::default()`. Tool and fan-out cannot be *requested* over
the protocol, so their verdicts are correctly WITHHELD. **A withheld verdict is not an
equivalence.** Fan-out is additionally undetermined live on both surfaces (0 provider requests by a
delegated child), and Windows is unmeasured at this SHA by anyone.

The underlying safety property is in better shape than the proof: at HEAD there are **zero ALLOWED
verdicts on any dimension, any surface, any mode** (`21-REVERIFICATION.md:170-184`). What is short
is proof breadth, not enforcement.

**Closing it requires**: adding tool-authority and breadth fields to the host child-spawn request
type in `wcore-protocol` (a schema change ⇒ **fenced seam**, Desktop must re-pin in the same train),
a live fan-out drive on both surfaces, and a Windows run of `child_authority_corpus`.
Note the trap the phase itself names: obtaining live evidence for four of the budget dimensions
would require **adding the attack surface in order to test it**.

**Cost: 2–3 lane-sessions + one fenced protocol seam.** **Not release-blocking** — no customer
symptom; the property holds, the proof is narrow.

---

### PHASE 22 — Supervision, Durable Goals, Fleet, and Loops

#### 22-C1 — FAILED (one surface of three)

> **"CLI, TUI, and host-protocol paths observe and control identical Goal, child, task, wait, log,
> cursor, and terminal producer state, and emit the canonical serialized producer fixtures consumed
> later at D2."** (`ROADMAP.md:90`)

**Grade measured: FAILED.** Confirmed at `873cc389`: `wayland-core goal` exists
(`main.rs:743`, `goal_cmd.rs` with `Open/Task/Run/Status/Effects`). **Zero `Goal` symbols in
`crates/wcore-protocol/src/`** — grep returns nothing. **Zero goal references under
`crates/wcore-cli/src/tui/`.**

**The specific unmet clause: *identical* across three surfaces.** One surface exists. Agreement
needs at least two. No producer fixtures exist, so the D2 consumption clause has nothing to consume.

**Closing it requires**: a TUI Goal surface (`wcore-cli/src/tui/`), a typed host-protocol Goal
command and event set (`wcore-protocol` ⇒ **fenced seam + Desktop co-pin**), and the canonical
serialized fixtures. The canonical projection they must consume already exists and is emitted by
`goal status`.

**Cost: 3–4 lane-sessions + one fenced protocol seam.** **Not release-blocking** — Goal is a new
capability whose only shipped entry point is the CLI, and the CLI path works.

#### 22-C3 — FAILED (measured, not built)

> **"Direct, ForgeFlows, Fleet, Council, and Anvil terminate through one canonical Goal transition
> with no nested verification/retry owner."** (`ROADMAP.md:92`)

> **CORRECTED 2026-07-29 by lane/record-truth — this row said FAILED and said no lane had
> attempted it. Both are stale. See "Correction" below; the original text is kept intact
> above it, and its falsifier is itself defective.**

**Grade measured: FAILED, unchanged.** Confirmed at `873cc389`: `GoalTerminalState` has consumers
only in `goal/{ledger,kernel}.rs` and `session_journal/model.rs`. **Grep for `GoalTerminalState`
under `crates/wcore-agent/src/orchestration/` returns zero hits.** Anvil still returns
`ClimbOutcome` (`orchestration/anvil/engine.rs:246,346`).

**The specific unmet clause: *one canonical*.** The taxonomy shipped and is green. The **adapter
surface** was never built; the five engines still return five types. A taxonomy everything *could*
map onto is not a construction where nothing can terminate any other way — this is the phase's own
hard criterion and no lane has attempted it.

**Closing it requires**: the adapter surface over five owners in `wcore-agent/src/orchestration/`.
`22-02-LOOP-OWNER-CENSUS.md` already specifies what each of the five produces and where Fleet binds.
Single crate, no protocol change, no credential, no second machine.

**Cost: 2–3 lane-sessions.** **Not release-blocking** — architecture consistency; no customer
symptom.

#### 22-C3 — CORRECTION (2026-07-29, lane/record-truth): **PARTIAL, not FAILED — and the row's own falsifier is broken**

**Three separate things are wrong with the row above, and they need separating.**

**(a) The adapter surface WAS built.** `26be00cd` — *"feat(22-c3): the adapter surface — one
canonical Goal terminal transition over all five loop owners"* — adds
`crates/wcore-agent/src/goal/strategy.rs` (**+667 lines**) plus changes to
`goal/kernel.rs`, `session_journal/model.rs` and `session_journal/reducer.rs`. Merged at
`f68f3ddd` — *"merge(22-c3): one loop owner, enforced over the Goal lifecycle and graded
PARTIAL"*. Measured at `f68f3ddd`: `strategy.rs` carries **45** `GoalTerminalState` references
and adapts all five owners —
`ClimbOutcome | CouncilRunResult | WorkflowRunError | &[ShardSummary] | DirectOutcome`.
So *"no lane has attempted it"* is false, and **PARTIAL is the correct grade** — which is
what the implementing lane graded itself in `aa60fc4b`.

**(b) It is NOT in the integration branch, and that must not be glossed.**
`git branch --contains f68f3ddd` lists `inv/*` and `lane/*` refs only.
`gh/plan/f20-unified-audit-repair` is at `ef1d97be`, where the adapter is genuinely absent.
**Both facts are true at once**: the work exists and is graded PARTIAL by its own lane; the
integration branch does not yet have it. A reader taking either half alone gets the wrong
picture.

**(c) The row's falsifier is a BROKEN INSTRUMENT and would never have noticed.**
The row's evidence is *"Grep for `GoalTerminalState` under
`crates/wcore-agent/src/orchestration/` returns zero hits."* The adapter was built under
`crates/wcore-agent/src/goal/`. **Measured: that grep returns zero even at `f68f3ddd`, where
the adapter exists and is merged.** The instrument therefore reports FAILED forever, including
after the criterion closes — a self-*failing* gate, the mirror of the self-passing class in
LANE-BRIEF §3.2. Per §6b-ii it is repaired here rather than merely noted:

> **Corrected falsifier for 22-C3.** Grep for `GoalTerminalState` across
> `crates/wcore-agent/src/` — **not** `orchestration/` alone — and require a consumer that
> adapts each of the five owner result types. RED when `goal/strategy.rs` is absent or stops
> naming all five. Verify against `f68f3ddd`, where the corrected form finds 45 references and
> the original finds none.

**Grade: PARTIAL** (built, merged to lane refs, self-graded PARTIAL, not yet integrated).
**Not release-blocking**, unchanged.

#### 22-C4 — PARTIAL

> **"Session-local fixed/dynamic, event-driven, and manual loops remain bounded across reconnect,
> preemption, missed intervals, and resume; persistent scheduling is deferred explicitly to
> Phase 24."** (`ROADMAP.md:93`)

**Grade measured: PARTIAL — and better than its own verdict says, in a different place.**
`record.rs:143-151` yields a ceiling for `Once`/`Fixed`/`Dynamic`/`EventDriven` (`Manual` correctly
has none), and `reducer.rs:269-276` refuses an iteration past it at the durable boundary
(`"iteration exceeds the authorized loop bound"`). The 2026-07-27 re-grade's claim that only `Fixed`
is enforced is **stale**.

**The specific unmet clause: *across reconnect, preemption, missed intervals, and resume* — and the
word *loops*, plural, meaning all of them.** `start_iteration` has exactly **one** production caller
(`goal/fleet.rs:475`). Every non-Fleet loop owner is entirely unbounded. Preemption and missed
intervals have no mechanism at all.

**Closing it requires**: routing the four non-Fleet loop owners through the kernel's
`start_iteration`, plus preemption and missed-interval handling, plus a reconnect/resume drive.
Single crate.

**Cost: 1–2 lane-sessions.** **Not release-blocking.**

#### 22-C5 — PARTIAL

> **"Existing journal compatibility is proved or migrated explicitly without silently invalidating
> F12 behavior."** (`ROADMAP.md:94`)

**Grade measured: PARTIAL.** The strongest result in the phase. M1–M5 proved on Linux cross-binary
against a real 84,327-byte journal; F-7 added the falsifiable regression guard the original grading
said was missing.

**The specific unmet clause: *proved* — Linux only.** The Windows M1–M5 legs were never taken; the
reduce instrument died mid-build on a contended box. The writer lease is `#[cfg(unix)]`-gated and
Windows byte-range locks are mandatory rather than advisory (threat T-22-06, an unclosed prior
defect class). Neither corpus contains a `tool_execution_*` frame, because the provisioned Anthropic
credential returns HTTP 401 on both hosts — **the tool region is the densest part of the reduced
state and this determination does not touch it.**

**Closing it requires**: build `p22_reduce.rs` on Windows (`C:\p22` is already a detached worktree
at the right commit with `wayland-core.exe` already built) and take M1–M5. The credential half needs
a working Anthropic key — **Sean-reserved**, and it is the only part of this criterion that is.

**Cost: 1 lane-session** for the Windows legs (cheapest open item in the ledger).
**BLOCKED (partially)**: the `tool_execution_*` region needs a credential. **Not release-blocking.**

---

### PHASE 23A — Governed Skills (Phase 23 Criterion 1)

#### 23A-C1 — NOT MET

> **"Generated skills cannot execute before governed promotion and can be observed, revoked, and
> rolled back."** (`ROADMAP.md:102`)

**Grade measured: NOT MET.** Confirmed at `873cc389`, and this is the one place where the
measurement is worse than the document implies for a *customer*, not for a proof.

**The specific unmet clauses — three of four:**

- *revoked* — **nothing implements it.** Grep for any revoke surface returns zero.
- *rolled back* — **nothing implements it.** Zero.
- *cannot execute before governed promotion* — satisfied **vacuously**. `main.rs:2506`:
  ```rust
  async fn run_skills_promote(_id: &str) -> anyhow::Result<()> {
      anyhow::bail!("skill promotion is temporarily unavailable while governed promotion is being implemented")
  }
  ```
  The pre-promotion state is inert because it is **permanently** inert. This is the same
  vacuous-truth shape the phase's own disposition flags.

**The customer-facing fact the criterion grading does not state.** `--skills-promote <PROCEDURE_ID>`
is declared at `main.rs:463-464` with a plain `#[arg(long, value_name = "PROCEDURE_ID")]` — **not
hidden**. It appears in `--help` on the shipped binary, with a docstring describing exactly what it
will do. It always fails. A customer can draft a skill and can never activate it. That is a shipped,
advertised, permanently dead flag.

> **CORRECTED 2026-07-29 by lane/record-truth — the paragraph above is stale. The flag is
> hidden.** At HEAD (`ef1d97be`), `crates/wcore-cli/src/main.rs:473` reads
> `#[arg(long, value_name = "PROCEDURE_ID", hide = true)]`, above a nine-line comment that
> cites this ledger row by name: *"HIDDEN (ledger row `23A-C1`) … the flag stops being
> advertised while still parsing and still failing loudly for anyone who already scripted it.
> This does NOT close `23A-C1`."* Both halves are guarded by
> `crates/wcore-cli/tests/skills_promote_not_advertised.rs`.
>
> **What this changes:** the *advertisement* complaint is closed, so the
> "RELEASE-BLOCKING at the advertisement level" verdict below no longer holds, and the
> 0.25-lane-session interim has already been spent.
>
> **What this does NOT change: `23A-C1` stays NOT MET.** Governed promotion, *revoked* and
> *rolled back* are still unimplemented; `run_skills_promote` is still a `bail!`. The product
> merely stopped promising something it cannot do. The 3–4 lane-sessions to close the criterion
> stand.
>
> Also stale in the paragraph below: **`F23A-01-H2` is FIXED** (`32a5fc90`, 2026-07-27, five
> wired regression tests) — see `.planning/phases/23A-governed-skills/23A-STATUS-CORRECTION.md`.
> The *observe* clause is no longer degraded by it.

**Closing it requires**: governed promotion (state machine + policy review + provenance),
revocation, rollback, and append-only history — `wcore-cli` + `wcore-memory` + `wcore-skills`.
Also open and unfixed: **F23A-01-H2**, any errored tool call kills the session, which makes even the
*observe* clause degraded in practice.

**Cost: 3–4 lane-sessions to close the criterion. 0.25 lane-sessions for the honest interim** —
`#[arg(hide = true)]` on the flag, or remove it, so the binary stops advertising a dead surface.
**RELEASE-BLOCKING at the advertisement level** (see §3).

---

### PHASE 24 — Gateway, Automation, Channels, and Typed API

Phase 24 has moved more than any other since its own report. Grades below are measured against the
tree, not `24-PHASE-REPORT.md`.

#### 24-C1 — NOT MET (re-graded 2026-07-29; was PARTIAL, was NOT MET on any platform)

> **"Native service lifecycle, profile isolation, active-turn visibility, drain, restart, upgrade,
> and rollback work without lost or duplicate delivery."** (`ROADMAP.md:117`)

**Grade measured: PARTIAL — moved UP substantially.** `wcore-cli/src/gateway.rs` exists and is
wired at `main.rs:735`. Live on `hetzner-dsm` against real `systemctl --user` with real
`wayland-core 0.12.25`: `install` → unit enabled; `status --json` → `state=running pid=… profile=f24b`;
`kill -9` → systemd's own restart counter recorded the recovery; `drain --budget-ms 5000` →
`Draining (pending 12)` → `Drained (pending 0)`; `uninstall` → unit gone. `profile isolation` moved
from NOT MET to met — **F24-B-H1 is FIXED**. The delivery-arrival half is **closed on Linux** by
24-C: ten deliveries, ten distinct messages at an **independent** sink, and the one delivery whose
outcome was UNKNOWN across a `kill -9` produced exactly one message where it previously produced two.

**The specific unmet clauses**: *upgrade* and *rollback* were never performed on any platform;
the whole measurement is **Linux only**; the 12-of-12 clean tally is short (F24-C-M1); and nine
channel adapters inherit `supports_outbound_idempotency() == false`, for which an outcome-unknown
delivery is **abandoned** rather than duplicated — safe and honest, and not the same thing as
delivered.

---

**RE-GRADED 2026-07-29 by `lane/24-idempotency` — this row's PARTIAL does not survive measurement.
Criterion 1 is NOT MET.** Full evidence: `24-C1-IDEMPOTENCY-SUMMARY.md`.

Criterion 1 is a **conjunction** — the gateway's own header reads *"no delivery is lost and none is
duplicated"*. Graded on each half separately:

- **No-duplicate half: HOLDS** on all ten adapters, and is now *measured* on four rather than
  reasoned about on one.
- **No-loss half: FAILS on nine of ten**, by construction, in the crash-during-send window.

**The 12-of-12 tally was graded on the one adapter of ten that implements the property under test.**
`scripts/f24-journey.mjs:380` is `platform = "slack"` and is the only `platform = "…"` line in the
driver; Slack (`wcore-channel-slack/src/lib.rs:234`) is the sole override of a trait method that
defaults to `false` at `wcore-channels/src/lib.rs:139`.

**The design's choice is nonetheless correct, and that is now a fact rather than an inference.** One
delivery key was replayed twice through real adapters over real HTTP, built by the production
factory. Telegram, Twilio SMS and WhatsApp each put **two messages** at the destination with no
dedupe token on the wire; Slack carried the key on both attempts. That known-positive is what makes
the other three interpretable. So abandoning prevents a **genuine** duplicate, not a hypothetical one.

The honest restatement to carry forward: *no duplicate delivery on 10/10 adapters (measured on 4);
exactly-once delivery on 1/10 (Slack, Linux only). On the other nine an outcome-unknown delivery is
abandoned, and that abandonment is currently unrecoverable and unsurfaced.*

**A new HIGH falls out of it, and it is ours alone.** The abandon path claims in-source that such a
delivery is *"recorded, terminal, and nameable by an operator"*. The code does not implement that:
`ledger.rs:214 pending()` and `:223 pending_count()` both exclude `Abandoned`; `:253 compact()`
classes it as terminal history, so the record is **eligible to be deleted**; and `DeliveryState::Abandoned`
has no consumer anywhere outside `ledger.rs`. The only signal is one `tracing::warn!`. This is what
converts a deliberate recorded non-delivery into an unrecoverable one. In flight as
`lane/24-c1-abandoned`.

**"Outbound idempotency for the nine adapters" was the wrong closing requirement.** The cost is not
uniform and mostly cannot be paid in code: **7 of 10 platforms provide no idempotency primitive at
all** (Telegram, Twilio, Meta Graph, SMTP, signal-cli, AppleScript iMessage, MS Teams) — for those,
`false` is permanent and truthful, and closing them is a **product decision** (an explicit per-channel
at-most-once vs at-least-once policy, exposed as configuration), not an implementation. Only two are
cheap: **Matrix** already PUTs to its native idempotency slot but derives `txn_id` from a
process-local counter that resets to 1 on restart (`rest.rs:13`), and **Discord** already sends a
dedup `nonce` that is deliberately distinct across restarts. Both ≈0.5 session.

**Severities as graded** (4-way cross-audit: codex 5.6 Sol HIGH, gemini 3.1 Pro MEDIUM, kimi K3
MEDIUM, internal adversarial HIGH): the **abandonment policy itself is MEDIUM** — it is the correct
and only available trade, and a HIGH would demand a fix that cannot exist. The **missing operator
surface and recovery path is HIGH** — fixable entirely in our code. Matrix/Discord are **MEDIUM**.

**Closing it requires**: the same live journey on macOS (launchd) and Windows (Task Scheduler); an
upgrade/rollback drive (`binary_path`/`binary_version` already exist in the projection precisely so
these are distinguishable); the §3 operator-surface HIGH; the two cheap adapters; and a **product
decision** on the remaining seven.

**Cost: 2–3 lane-sessions**, plus the product decision, which is Sean's.
**RELEASE-BLOCKING for macOS and Windows** (see §3).

#### 24-C2 — PARTIAL

> **CORRECTED 2026-07-28 by `lane/24-triggers` (merged `5d93a407`), which probed the real tick
> loop at base rather than reading the source. Two claims below are wrong:**
>
> 1. **`poll` did NOT fail silently — it fired.** Measured at base: `event` 0 fires,
>    `webhook` 0 fires, **`poll` 6 fires**. `poll:URL:300` was `every:300` with the URL string
>    ignored, so it ran the action on the clock **having never contacted the URL**. That is a
>    stronger lie than silence: not a feature that does nothing, but one that does the wrong
>    thing while reporting success.
> 2. **The suspected mechanism is refuted.** `wcore-agent/src/cron.rs`'s "a missing surface logs
>    the fire and returns Ok" comment is **stale** — the arms return `NoDispatcher`, and the
>    comment describes `Target`, not `Trigger`. The real cause was `next_after` plus a total
>    absence of producers.
>
> **Current state:** `event` is implemented on a durable cross-process queue; `webhook` and
> `poll` are refused at add and removed from `--help`, with persisted jobs listed as
> `WILL NEVER FIRE`. **Criterion 2 remains PARTIAL** — the false promise was retired, the plane
> was not built. Open: webhook needs an inbound route and credential scheme (~1.5 sessions,
> should reuse the new event bus); poll needs an egress-routed client and, first, a *defined*
> response contract (~1 session). Full record: `24-C2-REPAIR-SUMMARY.md`.

> **"Scheduled, event-driven, webhook, polling, and commitment work has bounded history, retry,
> continuation, and delivery."** (`ROADMAP.md:118`)

**Grade measured: PARTIAL.** Real work landed: schedule ownership is a held `flock`/`LockFileEx`
lease (not an assumption), seven trigger types each with a one-way-narrowing bound, enforced retry
and history, delivery through the 24-01 ledger. 142 tests green. All reachable from the shipped
binary.

**The specific unmet clauses — and the customer-facing one.** `cron.rs:44-53` documents eight
trigger forms in the shipped `--help`:

```
--trigger once: / every: / cron: / commit:        <- work, live-proven
--trigger event:build.finished                    <- validates, persists, lists, NEVER FIRES
--trigger webhook:/hooks/build                    <- validates, persists, lists, NEVER FIRES
--trigger poll:https://x.test/health:300          <- validates, persists, lists, NEVER FIRES
```

Per `24-02-SUMMARY.md` item 4: *"Nothing publishes an event, routes an inbound HTTP request to a
job, or performs the poll."* They are a complete, bounded, persisted **vocabulary** and an
incomplete **plane**. Threat T-24-02-02's mitigation — that an unauthenticated caller cannot cause a
fire — currently holds only because **no caller can cause a fire at all**.

Also unmet: the **continuation gate does not pass** (no run has hard-killed a gateway mid-delivery
and counted at an out-of-process sink); the **surface gate does not pass** (no PTY drive, no
rendered screen); `max_in_flight` is stored and clamped but not enforced at dispatch; and there is
**no macOS evidence at all**.

**Closing it requires**: producers for `event`, `webhook` and `poll` (an event bus, an inbound HTTP
route with the `require_auth` admission path the stored flag anticipates, and a poller); the
kill-mid-fire continuation run; the PTY surface test; macOS.

**Cost: 3–4 lane-sessions.** **RELEASE-BLOCKING** — three of eight advertised trigger kinds are
inert (see §3).

#### 24-C3 — PARTIAL (Linux), NOT MET (macOS, Windows)

> **"Reference channels prove setup/auth, access, routing, media, native actions, idempotency,
> reconnect/reload, and health."** (`ROADMAP.md:119`)

**Grade measured: PARTIAL on Linux, NOT MET elsewhere** (`24-03-SUMMARY.md:396-403`).

**The specific unmet clause**: the **end-to-end inbound matrix** from the binary against a real
adapter was never finished, and 24-03 Task 3 was only partially run. macOS and Windows have nothing.

**Closing it requires**: the hermetic fixture endpoint 24-03 Task 3 owns (also a prerequisite for
24-C1's remaining tally), the inbound matrix, and the two other platforms.

**Cost: 2–3 lane-sessions.** **Partially release-blocking** — channels are a headline capability
and the inbound half is unproven end-to-end.

#### 24-C4 — MET on Linux / HTTP+SSE only

> **"Typed authenticated clients recover event gaps and produce useful redacted health/log/support
> evidence."** (`ROADMAP.md:120`)

**Grade measured: MET on Linux, HTTP/SSE transport only** — moved UP from NOT MET by lane 24e
(`24-04-SUMMARY.md:355-369`). A typed client authenticates, is refused by ROLE distinctly from
CREDENTIAL, issues idempotent commands (one effect, two identical receipts), negotiates or is
refused by name, and **recovers an event gap** over a real socket from a connection severed
mid-stream, duplicates and losses both zero. 13 tests, 10 mutations each reddening its named test.

**The specific unmet clauses**: REST `/v1` is role-gated but has **no resume route and no
idempotency handling**; stdio and WebSocket have **none of the three**; everything is Linux.

**Closing it requires**: resume + idempotency on REST and stdio/WebSocket, and the two platforms.

**Cost: 2 lane-sessions.** **Not release-blocking if HTTP/SSE is the shipped host transport** —
which it is for the Desktop app. Record the transport envelope explicitly in the release notes.

#### 24-C5 — NOT MET (no evidence, any platform)

> **"Setup-to-recovery journeys pass on macOS, Linux, and Windows."** (`ROADMAP.md:121`)

**Grade measured: NOT MET.** 24-04 was never started. Confirmed at `873cc389`: no journey driver
exists under `crates/wcore-eval-scenarios/tests/` or `crates/wcore-cli/tests/`; no receipt schema;
no receipt on any platform.

**The specific unmet clause: all of it, on all three platforms.** The nearest thing is 24-B's live
Linux gateway journey (§24-C1), which is a real install→run→kill→recover→uninstall sequence but is
not a receipted setup-to-recovery journey and covers one OS.

**This is the criterion the panel unanimously said I had misclassified, and measurement agrees with
them.** `.github/workflows/release.yml:64-80` builds and ships **six targets**: Linux
x86_64/aarch64, **macOS x86_64/aarch64, Windows x86_64/aarch64**. The product's own release pipeline
declares a three-OS support envelope. Shipping macOS and Windows binaries whose install-and-recover
path nobody has ever driven is selling on two platforms with zero journey evidence.

**Closing it requires**: a journey driver + receipt schema, then live runs on macOS and Windows.
Both hosts are reachable (`SeanD@seandesktop` verified; macOS is the local Mac). **Note the standing
constraint: no Cargo on the Mac**, so the macOS leg needs the CI-published
`wayland-core-aarch64-apple-darwin` artifact — which has existed since `d9c7683b`, so the
"unobtainable macOS binary" premise recorded in `23A-04-SUMMARY.md:40` **must not be reused**.

**Cost: 3–4 lane-sessions.** **RELEASE-BLOCKING for the declared platform envelope** (see §3, §4).

---

### PHASE 25 — Remote Reach, Nodes, and Plugin Lifecycle

Criteria 1 and 3 are **MET** and are not in this ledger. C1 was closed today by `lane/25-cloud` at
`5e620ef0` (merged as `873cc389`), which found the cloud backend was **broken, not merely
unexercised** — three HIGH defects, two of which would have produced a **false green**.

#### 25-C2 — NOT MET

> **"Nodes pair, advertise capability, revoke, recover offline, and handle mixed versions without
> losing authority attribution."** (`ROADMAP.md:130`)

**Grade measured: NOT MET.** All six properties were exercised through the shipped binary and
attribution held after all five disruptions — but against a separate *machine identity* (own
hostname, filesystem, minted node key, process table and netns, reached over real ssh, genuinely
stoppable), **not a second physical host**.

**The specific unmet clause: *nodes*, plural, meaning genuinely distinct machines.**

**Direction of movement — this one moved today, and the status document has not caught up.**
`25-PHASE-STATUS.md:102` records this as blocked on SSH trust between the two physical hosts,
"Reserved to Sean". **Measured live by me today:**

```
$ ssh hetzner-dsm 'ssh -o BatchMode=yes -o ConnectTimeout=15 SeanD@seandesktop hostname; echo RC=$?'
SeanDesktop
RC=0
```

The trust exists. `25-03-NODE-EVIDENCE.md` §7 already carries the exact closing commands.

**Closing it requires**: re-running the node corpus with `hetzner-dsm` and `seandesktop` as the two
real hosts. No credential, no new machine, **no Sean action.**

**Cost: 1–2 lane-sessions. NO LONGER BLOCKED.** **Not release-blocking** — multi-host node
federation is not a first-release capability, and the mechanism is proven against a separate machine
identity.

#### 25-C4 — NOT MET

> **"Compromised keys/plugins/backends and denied secret/egress paths fail closed with no orphaned
> execution."** (`ROADMAP.md:132`)

**Grade measured: NOT MET — with only one surface left.** The fail-closed half **holds**: all five
hostile cases refuse on both hosts with named verdicts, nonzero exits, no fallback. The no-orphan
half holds for **local**, **container** and — as of today — **cloud**, where the scan is now checked
in both directions by a real leaked machine (`82d1d97b062338`, leaked by the lane's own `tail -1`
parse defect, not planted) found as `count 1 (MEASURED)` with a nonzero exit.

**The specific unmet clause: *no orphaned execution*, on the SSH backend.** SSH alone reports
`NOT MEASURED` — correctly, never zero. **One unmeasured surface is not "across every reference
backend".**

**Same correction as 25-C2**: recorded as Sean-reserved, and the trust is now live. The backend
exists (`crates/wcore-exec-backend/src/backends/ssh.rs`) and reads
`WAYLAND_EXEC_SSH_TARGET` (`ssh.rs:43`).

**Closing it requires**: set `WAYLAND_EXEC_SSH_TARGET=SeanD@seandesktop` (or a containerised sshd)
and run the orphan scan with the same positive/negative control pair the cloud surface now uses.

**Cost: 0.5–1 lane-session. NO LONGER BLOCKED.** **Not release-blocking** — a measurement gap on
one execution backend, with the fail-closed half already holding.

---

### PHASE 27 — Multimodal, Browser, Generation, and Voice Contracts

The weakest phase in the program: one criterion partial, four not met.

#### 27-C1 — PARTIAL

> **"Standalone and host messages use one bounded, validated attachment/document intake path and
> degrade explicitly on unsupported providers."** (`ROADMAP.md:151`)

**Grade measured: PARTIAL.** The document (PDF) path goes through one bounded, open-once,
magic-byte-validated intake with an ingest cap enforced from the descriptor's metadata before any
payload read. Explicit degradation is met for the image class and **proved live**: the Anthropic and
Gemini builders were measured emitting a byte-identical outbound request whether `supports_vision`
said false or true; both now substitute `[image omitted: model not vision-capable]`.

**The specific unmet clause: the word *one*.** The composer path and the channel enricher were
**measured already correct** and deliberately not rewritten through the new chokepoint, so the
mechanism is shared for documents and duplicated for images and channel media. The plan's own gate
requiring `media_intake` in `attachments.rs` and `channel_media.rs` is **RED and reported RED**. The
TUI half was never exercised (no PTY drive) and macOS has no artifact.

**Closing it requires**: routing the composer and channel-enricher paths through the single
chokepoint (a refactor of correct code — genuine architecture value, zero defect value), a PTY drive,
and a macOS leg.

**Cost: 2 lane-sessions.** **Not release-blocking** — the duplicated paths were measured correct.

#### 27-C2 — NOT MET

> **"Browser, CUA, and web surfaces publish live readiness and preserve sandbox, egress, approval,
> and cleanup policy."** (`ROADMAP.md:152`)

**Grade measured: NOT MET.** Two distinct failures, one trivial to fix and one structurally
expensive. Both confirmed present at `873cc389`.

**(a) The remediation text sends every user in a circle. Still open.**
`crates/wcore-browser/src/tool.rs:499-503` tells a user whose browser tool is default-denied to add:
```
[browser]
allowed_origins = ["example.com", "*.mysite.com"]
```
The key the loader actually reads is **`browser.policy.allowed_origins`** — i.e. a
**`[browser.policy]`** section — confirmed at `wcore-config/src/config.rs:1201` and
`wcore-config/src/browser.rs:42`. Following the product's own instructions verbatim leaves the tool
disabled forever. **An unavailable whose stated fix is wrong fails this criterion's own honesty
bar.** It is a two-word string change.

**(b) Readiness is not published; it is linkage-derived. Still open.** `browser_suite` and
`computer_use` are advertised on the basis of whether a plugin crate is **linked**, not whether a
browser binary or display exists — `bootstrap.rs:696` calls
`PluginRunner::new().with_computer_use_advertised(true)` unconditionally. On a headless box the
flags read `true` and the operation fails with `spawn camoufox: No such file or directory`. The
Desktop app renders a capability that cannot work. Implementation stopped at a fenced protocol seam
(`.planning/SEAM-REQUESTS/27.md`, SR-27-1..3).

**Also unmet**: policy preservation is **one-quarter measured**. Origin admission holds and fails
closed with a stated reason. Downloads-root confinement, the approval gate on a computer-use
operation, and the process count before/during/after a session plus one reaper interval have **no
baseline at all**.

**Closing it requires**: (a) a two-word string fix; (b) probe-based readiness behind SR-27-1..3 ⇒
**fenced seam, Core+Desktop coordinated release**; (c) three policy baselines.

**Cost: 0.2 lane-sessions for (a). 2 lane-sessions + one Desktop co-release train for (b) and (c).**
**RELEASE-BLOCKING** (see §3).

#### 27-C3 — NOT MET

> **"Built-in, MCP-only, late-MCP, and combined media generation expose consistent discovery,
> credentials, accounting, and failures."** (`ROADMAP.md:153`)

**Grade measured: NOT MET.** **None of the four generation shapes was exercised.** No MCP media-tool
fixture was built, so MCP-only, late-MCP and combined were never reachable.

One real result: the honest-degradation advisory reaches the model **verbatim on the wire**, naming
each unavailable capability and the exact variables that would enable it, with an explicit
instruction not to invent a cause. Measured, not assumed.

**The specific unmet clauses**: all four shapes unexercised; the advisory **reaches no host** — zero
events on the protocol stream, so a Desktop user has nothing to render; accounting is SOURCE-ONLY —
cost is token-shaped and a media call produces **no cost record at all**.

**Closing it requires**: an MCP media-tool fixture, drives of all four shapes, a host-visible
failure surface (protocol event ⇒ likely fenced), and a decision on whether unaccounted media cost
matters.

**Cost: 2–3 lane-sessions.** **Not release-blocking**, with one caveat worth Sean's eye: *media
calls produce no cost record.* If media generation is billable or budget-governed in the shipped
product, that becomes a money-correctness issue and moves up immediately.

#### 27-C4 — NOT MET

> **"Streaming voice supports interruption, cancellation, compatibility, accounting, and ordered
> protocol events."** (`ROADMAP.md:154`)

**Grade measured: NOT MET. NOTHING WAS EXERCISED.** No audio flowed on any machine. No interruption.
No cancellation. No event ordering observed. `crates/*/tests/` contains no voice test.

**Reachability — measured, because the whole release classification turns on it.** All three panel
members independently attacked my original classification of this criterion for resting on an
unverified premise. They were right to, so I measured it:

- `crates/wcore-agent/src/tool_backends/mod.rs:82` — `#[cfg(feature = "voice")] pub mod voice_mode;`
- `crates/wcore-cli/Cargo.toml:55-58` — `voice = ["wcore-agent/voice"]`, with the comment
  *"OFF by default to keep libasound.so.2 (ALSA) off the default Linux binary."*
- `voice` appears in **no** `[features] default` list.

**The voice tool is not compiled into the default shipped binary at all.** It is not a half-wired
path a customer can reach; it is absent. When it *is* built, `VoiceModeTool` additionally hides
itself via `Tool::is_available() == false` when capture is unavailable — fail-closed.

**Closing it requires**: `seandesktop` has audio, a toolchain, and answered a reachability probe.
The phase's own verdict calls this "an execution shortfall, not an environmental impossibility", and
that remains true.

**Cost: 2–3 lane-sessions.** **Not release-blocking — on measured grounds**: the feature is not in
the shipped artifact. **If the `voice` feature is ever enabled in a release build, this criterion
becomes blocking immediately**, because a shipped voice surface with zero interruption evidence is
exactly the silent-failure class that blocks 24-C2.

#### 27-C5 — NOT MET

> **"Deterministic corpora and packaged smokes pass on native macOS, Linux, and Windows."**
> (`ROADMAP.md:155`)

**Grade measured: NOT MET.** **Zero packaged smokes ran on zero platforms.** Every Linux measurement
in Phase 27 came from a `cargo build --release` binary **inside a build tree** — not a packaged
artifact, and not counted as one anywhere in the phase's evidence.

**The specific unmet clauses**: no packaged smoke on any platform; the one deterministic corpus that
exists (18 intake entries, pinned bytes, byte lengths, SHA-256 digests, regenerable identically on
any platform) has **no consuming suite**; the browser, generation and voice corpora were never
built.

**Closing it requires**: a packaged-artifact smoke harness and runs on three platforms, plus a suite
that consumes the intake corpus. **Substantially overlaps Phase 28** (native cross-platform
certification), which has already run a 147-cell hostile matrix and a 1000-session soak on Linux and
macOS — but 28 certifies the *candidate*, not Phase 27's corpora.

**Cost: 2 lane-sessions** (less if folded into `28-04`). **Not release-blocking on its own** — 28-04
is the release-facing certification and it is already planned.

---

## 2. Cost roll-up

| Phase | Criterion | Cost (lane-sessions) | Blocked on | Release-blocking |
|---|---|---|---|---|
| 21 | C3 equivalence | 2–3 + fenced seam | — | No |
| 22 | C1 three surfaces | 3–4 + fenced seam | — | No |
| 22 | C3 one terminal transition | 2–3 | — | No |
| 22 | C4 bounded loops | 1–2 | — | No |
| 22 | C5 journal compat (Windows) | 1 | credential for the tool region (Sean) | No |
| 23A | C1 governed skills | 3–4 *(or 0.25 to de-advertise)* | — | **Yes** |
| 24 | C1 service lifecycle | 2–3 | — | **Yes** (macOS/Windows) |
| 24 | C2 automation plane | 3–4 | — | **Yes** |
| 24 | C3 reference channels | 2–3 | — | **Yes** (partial) |
| 24 | C4 typed clients | 2 | — | No (HTTP/SSE ships) |
| 24 | C5 tri-platform journeys | 3–4 | scoping decision (Sean) | **Yes** |
| 25 | C2 nodes | 1–2 | ~~SSH trust~~ **cleared today** | No |
| 25 | C4 fail-closed / SSH orphans | 0.5–1 | ~~SSH trust~~ **cleared today** | No |
| 27 | C1 one intake path | 2 | — | No |
| 27 | C2 readiness + policy | 0.2 + 2 + Desktop train | fenced seam SR-27-1..3 | **Yes** |
| 27 | C3 media generation | 2–3 | — | No* |
| 27 | C4 voice | 2–3 | — | No (feature off by default) |
| 27 | C5 packaged smokes | 2 | — | No (overlaps 28-04) |
| | **TOTAL** | **33–47, midpoint ≈38** | | **7** |

\* 27-C3 flips to blocking if media generation is billable in the shipped product — media calls
currently produce **no cost record**.

**These estimates are for closing the criterion as written.** They do not include re-verification,
cross-audit, or the seam/Desktop coordination latency, which for the two fenced items is measured in
release trains rather than sessions.

---

## 3. What must close before a release candidate, and what can ship open

### MUST CLOSE — 7 criteria, ≈13–17 lane-sessions + one Desktop co-release train

Ranked. The organising principle, sharpened by the panel: **customer promise × reachability ×
failure mode.** A surface the product advertises and a customer can reach, which then does nothing,
outranks any amount of internal proof debt.

**1. `24-C2` — three of eight advertised trigger kinds can never fire.** `--trigger event:`,
`--trigger webhook:` and `--trigger poll:` are documented in the shipped `--help` at `cron.rs:49-52`.
They validate, persist and list. Nothing ever fires them. **This is the worst failure mode in the
ledger**: the customer gets no error. They register automation, see it in `cron list`, and it
silently never runs. Every panel member ranked this at or near the top. **3–4 sessions.**

**2. `27-C2(a)` — the browser remediation string.** Two words. `[browser]` → `[browser.policy]` at
`wcore-browser/src/tool.rs:501`. The product's own error message sends the user into a loop that can
never succeed, which burns support time and corrodes trust in the documentation generally.
**Highest leverage per unit of effort in the entire ledger. 0.2 sessions.** Two panel members
explicitly moved this above item 3; I accepted.

**3. `24-C5` + `24-C1` (macOS/Windows legs) — the platform envelope.**
`.github/workflows/release.yml:64-80` ships **six targets across three OS families.** macOS and
Windows have **zero** setup-to-recovery journey evidence. Install, run, kill, recover, uninstall is
precisely the class that diverges across platforms — service managers, signal semantics, orphan
reaping, path handling. **All three panel members raised this independently and I had it in the
wrong bucket.** Either close it or narrow the declared envelope (§4). **5–7 sessions combined.**

**4. `23A-C1` — `--skills-promote` is an advertised flag that always fails.** Declared unhidden at
`main.rs:463`, implemented as a `bail!`. Two of three panel members argued this can ship open
*because it fails loudly and safely* — provided the flag stops being advertised. I accept that
split: **the blocking element is the advertisement, not the implementation.** Minimum honest fix:
`#[arg(hide = true)]` or remove, plus a Known Issues entry. **0.25 sessions** for the honest interim;
3–4 to actually close the criterion, which need not be in this RC.

**5. `27-C2(b)` — capability flags that lie to the Desktop app.** `browser_suite`/`computer_use`
advertised on linkage, not liveness. Gemini ranked this **first**, on the strongest structural
argument in the whole panel: a protocol capability contract baked into a shipped RC cannot be fixed
post-release without a breaking version bump or a synchronized hotfix. Codex and Kimi countered that
the fix *requires* a Desktop co-release, so "must close" is the wrong demand and **"must stop
lying"** is the achievable one. **I take the minority position on the requirement and the majority
on the mechanism**: the honest-flag change must be in the RC; the full probe-based readiness lands in
the coordinated train. **2 sessions + one Desktop train.**

**6. `24-C3` — the inbound channel matrix.** Channels are a headline capability; the inbound half
was never driven end-to-end from the binary against a real adapter, on any platform. The hermetic
fixture endpoint this needs is also what closes 24-C1's remaining tally, so the two share a
prerequisite. **2–3 sessions.**

### CAN SHIP OPEN — 11 criteria, ≈20–30 lane-sessions, deferrable

Named plainly, with what a customer would and would not notice.

- **`21-C3`, `22-C3`, `22-C5`, `25-C4`** — proof-completeness and internal architecture. The
  enforcement properties hold; the proofs are narrow, one-platform, or one-backend. **No customer
  symptom exists for any of them.** This is exactly the bucket that must not be treated as blocking:
  the unanimous panel view, and the failure mode that turned Phase 20 into a 74-plan two-week loop.
- **`22-C1`, `22-C4`** — durable Goals are a new capability whose only shipped entry point is the
  `wayland-core goal` CLI, and that path works. The missing TUI and host-protocol surfaces are
  capability *breadth*, not defects.
- **`25-C2`** — multi-host node federation. Not a first-release capability; the mechanism is proven
  against a genuinely separate machine identity. Cheap now that the trust exists — do it, just do
  not gate on it.
- **`27-C1`** — "one intake path" is an architecture criterion over code that was **measured already
  correct** on the duplicated paths. Real value, zero defect value.
- **`27-C4` (voice)** — **not compiled into the default binary** (`voice` feature off,
  `wcore-cli/Cargo.toml:55-58`). Not a silent-failure surface because it is not a surface. Document
  it as unshipped. **This classification is now measured; it was a hope when the panel attacked it,
  and the panel was right to.**
- **`27-C5`** — packaged smokes overlap `28-04`, which is planned and release-facing.
- **`24-C4`** — MET on the transport the Desktop app actually uses. Declare the transport envelope
  (HTTP/SSE supported; REST/stdio/WebSocket best-effort) rather than building three more.
- **`27-C3`** — with the caveat in §2: *if media generation is billable, the missing cost record
  moves this to blocking immediately.* That is a product question for Sean, not a measurement.

---

## 4. Cross-audit of the ranking in §3

Panel per LANE-BRIEF §4. **Byte-counted, and the first attempt was thrown away.**

| Member | Bytes | Vote | Received the right question? |
|---|---|---|---|
| `codex exec -m gpt-5.6-sol` | 15,583 | **DISAGREE** | Yes (2nd attempt) |
| `gemini -m gemini-3.1-pro-preview` | 21,100 | **DISAGREE** | Yes (3rd attempt) |
| `/Users/seandonahoe/.kimi-code/bin/kimi` | 4,060 | **AGREE** (with four substantive dissents) | Yes (3rd attempt) |
| internal adversarial pass | — | argued against the emerging consensus | — |

**Three harness defects found, all of the self-passing class, all of which would have produced a
fabricated 3/3 or 4/4:**

1. **The prompt file was silently clobbered by a concurrent lane.** I wrote a 4,624-byte question to
   `scratchpad/panel/Q.txt`; by the time the panel ran, that path held a **5,377-byte question from
   another lane about PR #254** (Windows sandbox DACLs). Codex and Kimi both answered *that*
   question, fluently and at length. A naive reader would have recorded three confident votes on a
   question never asked. **Fix: unique per-lane filenames (`criteria-gap-panel-$$`); verify
   `${#Q}` and the first bytes before dispatch.** This is a new entry for the standing self-passing
   list — *a shared scratchpad path is a shared mutable global.*
2. **`codex exec` reads stdin even when given a prompt argument**, inheriting whatever the calling
   shell had. Fix: `< /dev/null` on every member.
3. **An anchored `PANEL_POSITION=` grep matched the prompt's own echo.** Codex echoes the full user
   message, which contains the literal string `PANEL_POSITION=DISAGREE` as part of the answer
   template. My first extraction recorded a vote from a run that had **timed out before answering**.
   Fix: extract only from the region after the model's final turn, and count occurrences (codex's
   file has 4).

### Where the panel changed my ranking

- **`24-C5` / the platform envelope — 3 of 3, and I was wrong.** All three independently said F is
  release-blocking for any platform in the RC's support envelope. I had it in the ship-open bucket
  on the reasoning that Phase 28 owns cross-platform certification. Gemini's rebuttal is the one
  that lands: *"a later phase certifies all three OS families"* is not the same as the journeys being
  proven on the RC build. **I then measured the envelope rather than assuming it**:
  `release.yml:64-80` ships six targets across three OS families. The panel's point survives its own
  precondition. **Moved to blocking.**
- **`27-C4` (voice) — 2 of 3 attacked my classification as resting on an unverified premise.** Kimi:
  *"you apply rigorous customer-harm reasoning to A–D and then exempt E on a hope."* Correct as
  stated. I measured it (§27-C4): the `voice` feature is off by default and the tool is not compiled
  into the shipped binary. **Classification unchanged, but now on evidence instead of hope** — which
  was the whole of the panel's objection.
- **`27-C2(a)` above `23A-C1` — 3 of 3.** All three ranked the misleading remediation string above
  the stubbed flag: a stub fails honestly and the user learns the truth; a wrong instruction fails
  *dishonestly and recursively*. **Reordered.**
- **`23A-C1` de-scoped from "implement" to "de-advertise" — 2 of 3.** Codex and Kimi both argued a
  loudly-failing stub is acceptable in an RC *provided it is not advertised*. Kimi dissented and
  kept it blocking. I took the majority on the mechanism and kept it in the blocking list at the
  advertisement level — which satisfies both readings.

### Recorded dissent

- **Gemini (minority, 1 of 3) ranks `27-C2(b)` FIRST**, above everything, on the argument that a
  protocol capability contract baked into a shipped RC cannot be patched post-release without
  breaking clients, whereas a CLI returning a text error is self-contained. Codex and Kimi both
  counter that D cannot close without the Desktop's release schedule, so demanding closure blocks on
  another product's timeline. **I took the minority's requirement and the majority's mechanism**
  (§3 item 5). Gemini's structural point is the stronger *argument*; the majority's point about
  schedule coupling is the stronger *fact*, and both can be honoured at once.
- **Gemini also argues `23A-C1` should be dropped from the blocking set entirely.** Not taken: a
  flag that appears in `--help` and always fails is an advertised dead surface, and the fix costs
  0.25 sessions.
- **Internal adversarial pass, arguing against the emerging consensus** that `24-C5` is blocking:
  the panel imported a premise — that the RC ships on macOS and Windows — and their own proposed
  remedy (*declare the support envelope*) is a release note, not a code change. Treating F as
  blocking converts one line of prose into a multi-lane platform campaign, which is the Phase 20
  failure mode exactly. **This pass partly survives**, and it is why §4's Sean item is framed as a
  **decision** rather than as work: if the envelope narrows to Linux, `24-C5` costs 0 sessions; if it
  stays at three OS families, it costs 3–4 and blocks. It lost on the point that the envelope is
  **already declared by `release.yml`** — so the default, absent a decision, is that it blocks.

### Blocked on Sean

**One item, and it is a decision, not work.**

- **The RC's supported-platform envelope.** `release.yml` currently ships macOS and Windows binaries
  with zero journey evidence. Either (a) accept 5–7 lane-sessions to close `24-C5` and `24-C1`'s
  macOS/Windows legs, or (b) declare the RC Linux-only and say so in the release notes. **Absent a
  decision, (a) is what the pipeline has already promised.**

Two smaller Sean-reserved items, neither of which gates the RC:

- A working Anthropic credential for `22-C5`'s `tool_execution_*` journal region (both hosts
  currently return HTTP 401).
- The Desktop half of the `27-C2(b)` readiness re-pin, in the same release train as Core.

**Two items previously recorded as Sean-reserved are now cleared and were not:** `25-C2` and
`25-C4`'s SSH-trust blocker. Measured live today — `hetzner-dsm` reaches `SeanD@seandesktop` with
`RC=0`. Any lane can take them now.

---

## 5. What was NOT measured

Stated so none of it renders as zero or as done.

- **No Cargo was run anywhere.** Per the standing rules, no compilation on the Mac, and I did not
  build or run tests on `hetzner-dsm` — three lanes were building concurrently and a contended
  full-workspace run is not a measurement. **Every test count, pass/fail figure and live transcript
  quoted in this ledger is copied from the phase evidence artifacts, attributed to them, and is not
  a fresh measurement by this lane.** Source-tree facts (grep, file contents, line numbers) and the
  live SSH probe **are** fresh, taken at `873cc389`.
- **Windows and macOS were not exercised at all** by this lane. Every "no macOS/Windows evidence"
  statement is the absence of an artifact, re-checked, not a run I attempted.
- **The lane-session estimates are judgement, not measurement.** They are calibrated against what
  the executed plans in these phases actually consumed, and they carry the program's own recorded
  bias: `24-01` was a four-task plan estimated as one session that one executor could not finish.
  **Read the upper bound.**
- **I did not re-grade Phases 26, 28, 29 or 30**, which are covered by the remaining plan queue
  (`26-04`, `28-04`, `30-01…04`) and are out of this ledger's scope. Phase 29's own verdict records
  all four criteria PARTIAL, and `30-01…04` have not started — **so the true program-wide open-criteria
  count is higher than 18.** This ledger measures only the six phases nobody has costed.
- **I did not verify that `27-C3`'s media generation is billable**, which is the fact that decides
  whether its missing cost record is release-blocking. That is a product question.
- **Phase 23B was not measured.** The handoff records `F23-05` as day-one-only with a multi-day
  clock running to 2026-07-30T23:54:26Z. It has open work but it is time-gated, not uncosted.

---

## 6. The honest read

The program is further from done than the plan count suggests, and the gap is quantifiable.

**91% of plans have summaries. 18 criteria those plans exist to satisfy are open, at ≈38
lane-sessions of unscheduled work.** For comparison, Phase 20 — the campaign this program treats as
its cautionary tale — was 74 plans over two calendar weeks. The uncosted criteria gap is roughly half
that, and it is *invisible* on the plan burndown because every plan that was supposed to close it
has a summary saying it ran.

The good news is real and should not be lost in that number. **Four of the six phases moved up since
their own verdicts were written**, three of them today: Phase 21's SC1 upgraded on a repaired
product, Phase 24 gained a shipped gateway verb surface and a Criterion 4 pass, Phase 25 closed its
cloud leg and found the backend was broken rather than merely unexercised, and two blockers recorded
as Sean-reserved turned out to be already cleared. **The phases that graded themselves honestly red
are the ones that produced the repairs.**

The path to a release candidate is **13–17 lane-sessions and one Desktop co-release train**, not 38 —
provided Sean decides the platform envelope, and provided nobody tries to close the internal-quality
criteria on the way there.

---

*Measured by lane `lane/criteria-gap` at `873cc389`. Every claim carries a file:line, an evidence
filename, or a captured command transcript. Where none is given, the statement is marked NOT
MEASURED.*
