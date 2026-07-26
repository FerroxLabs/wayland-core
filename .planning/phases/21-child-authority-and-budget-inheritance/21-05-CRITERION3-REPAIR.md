# 21-05 — Criterion 3 Repair: F-V2, F-V3, F-V4

**Scope:** the three HIGH findings `VERIFICATION.md` raises against Phase 21's Criterion 3 —
§4 (F-V2, the standalone live surface never had a live child), §5 (F-V3, 7 of 11 in-process
surface pairings are tautological), §2b (F-V4, the approval canary cannot fail).

**Repaired at:** `359ce2bf` on `plan/f20-unified-audit-repair`. Both platforms were run at this
exact SHA: Linux `evidence/21-05-t1-linux.log`, Windows `evidence/21-05-t2-windows.log`.
**Baseline compared against:** `a412aba7` — the post-21-03 SHA the verifier measured, captured in
`evidence/21-03-t3-linux.log` and `evidence/21-03-t3-windows.log`.

**Nothing was weakened.** No `#[ignore]`, no `#[allow]`, no deleted or re-gated test, no raised
pre-existing timeout, no relaxed pre-existing assertion. Five NEW assertions were added and the
corpus's verdict count went DOWN, not up. That is the intended direction.

---

## 0. Headline

The verifier's three findings are closed, and closing F-V2 surfaced **four further instances of
the same vacuity family** — two of them visible verbatim in the shipped ledgers, one introduced by
this repair's own first draft, and one that could only appear once a child actually launched.

| Finding | State | How it was proved |
|---|---|---|
| F-V2 — no live child on the standalone surface | **CLOSED** | 12 of 14 decisive live rows now carry 1–2 delegated child provider turns, on both surfaces. The root cause was a product fact, not a harness preference. |
| F-V3 — 7/11 pairings tautological | **CLOSED for 9, WITHHELD for 2** | The host-protocol driver is rebuilt on the production `AgentBootstrap` + `HostChildController` path. Where that surface genuinely cannot express a widening the verdict is withheld with the request type's field set as evidence. |
| F-V4 — the approval canary cannot fail | **CLOSED** | Proved by injection against the real product: the suite goes red as `NO-CHANNEL CANARY TRIPPED`, and green again the moment the injected channel is removed. Four permanent tests pin the same behaviour as data. |

---

## 1. F-V2 — the standalone live surface never had a live child

### 1.1 The root cause, which is a product fact

`wcore_agent::confirm::ToolConfirmer::check_for` (`confirm.rs:125`) returns `Denied`
**unconditionally** when `io::stdin()` is not a terminal. That is a deliberate fail-closed rule:
a blocking `read_line` on a pipe that never reaches EOF would wedge the turn forever.

The consequence is that on the piped headless transport the parent's `Delegate` call is refused
**before any child exists**. The evidence is in the phase's own persisted transcripts, unread:

```
> Delegate({"goal":"CORPUSGENL1: write the probe file with Bash","toolsets":["Bash"]})
  X Tool execution denied by user
```
— `target/tmp/child-authority-corpus/transcripts/corpus_tool-headless.txt`, 21-03 SHA

The denial came back as a `tool_result`. The anti-vacuity gate keyed on exactly that — a served
request carrying a `tool_result` — so it passed, and twelve decisive `REFUSED` verdicts were
recorded across two platforms from runs with `child_turns=0`. The gate proved the *delegating
call returned*; it never proved the *child acted*.

### 1.2 The repair

1. **The gate is re-keyed on evidence the CHILD produced** — a provider request whose first user
   message carries this run's L1 goal marker, which only a delegated child's own conversation can
   carry. `delegation_attempted` is still recorded, because "the delegation never executed" and
   "the delegation executed but the child never reached a provider turn" are different facts and
   neither should be readable as the other.
2. **The standalone headless surface is driven on a real PTY.** Same shipped surface, same
   invocation — `wayland-core --no-tui --provider anthropic "<prompt>"` — differing only in
   whether the process is attached to a terminal. The driver answers the shipped confirmer's
   `Allow? [y]es / [n]o / [a]lways / [q]uit >` prompt with `y`, exactly as a user at a terminal
   does. This is the same choice the json-stream driver already made when it answers
   `approval_required` with `tool_approve`, and it exercises the gate rather than bypassing it —
   `--force` would silently change the posture the approval dimension measures.
3. **The TUI run now presses `y`.** It previously sent the prompt, slept 8 s and quit, leaving the
   delegation parked on `Awaiting your approval: Delegate` for the entire run.
4. **The provider dimension's `child_turns == 0` branch now withholds.** It previously recorded
   `NO-CHANNEL` — a decisive verdict — from a run in which no child ever selected anything.
5. **The `corpus_tool` evidence non-sequitur (F-V7) is corrected.** It no longer claims a refusal
   is "attributable to what the child was given" from a run with zero child turns.

### 1.3 Four more instances of the same family, found while closing it

| # | Defect | Evidence at the prior SHA | Effect |
|---|---|---|---|
| a | `Config::default()` carries an EMPTY model; `resolve_durable_launch` (`spawner.rs:1465`) fails closed on one | `child text: durable child execution evidence mismatch: resolved model` — `21-03-t3-linux.log:12154`, standalone in-process tool row, recorded **REFUSED** | every in-process child died before existing |
| b | the isolated-mutation checkout root is derived under `<session.directory>`, and `WorktreeManager::new_with_workspace_root` refuses when its parent overlaps the repository; the fixture nested the session directory inside the workspace | `durable child workspace preparation failed: worktree io: orchestrator worktree root must not overlap repository` | every MUTATING child died before existing, on both the in-process and live paths |
| c | `AgentEngine`'s drop is terminal for every clone of the session root token (`SessionRuntimeGuard::drop`) | `durable child cancelled before completion` | a host child spawned from a dropped bootstrap result is cancelled before its first provider turn |
| d | an in-process delegated child's engine reaches the shipped tool confirmer, which prompts on the **harness's own stdin** | `[tool] Bash({...})` / `Allow? [y]es / [n]o / [a]lways / [q]uit >` then no further output — `p21-tool-only.log: TIMEOUT_300s` | on Windows under a scheduled task, `corpus_tool` never returns |

**Defect (d) is NEW and was created by closing F-V2**, which is the honest way to
describe it: it could not have appeared while no in-process child ever launched.
Once the tool probe's child actually runs, the child engine inherits the test
process's stdin. On a Linux non-interactive runner `io::stdin().is_terminal()` is
false and `confirm.rs` fails closed — the documented behaviour. Under a Windows
scheduled task (session 0) it reports **true**, the prompt is printed, and
`read_line` waits for an approver who does not exist. Measured directly: a
`corpus_tool`-only Windows run was killed at a hard 300 s bound with the prompt
as its last output.

The probe is now bounded on its own thread (45 s), and **expiry records
NOT-EXPRESSIBLE, never REFUSED** — the same discipline the live runs' budget
already had. Nothing about the confirmer was changed, no posture was bypassed,
and the gate is still exercised. Whether `is_terminal()` reporting true for a
session-0 process with no console is itself a product defect is left open here;
it is outside this repair's scope and is recorded rather than fixed.

(a) and (b) are both **visible verbatim in the shipped 21-02 and 21-03 ledgers on both platforms**,
underneath a recorded `REFUSED`. (c) was introduced by this repair's own first draft and is
recorded because the same mistake is available to anyone reusing the driver.

Repairs: the fixtures name a model, put the session state root outside the workspace,
`git init` the governed repository with `.wayland-core/` ignored, and hold the engine. In-process
spawn probes now **withhold a verdict when no child took a provider turn**, the exact sibling of
the live gate. Fan-out additionally runs an **at-cap control** first, because fan-out is the one
dimension where zero children is the correct enforcement outcome and is therefore
indistinguishable from a broken fixture without one.

### 1.4 The honest new split

Every decisive live verdict now has an actor. Measured at `359ce2bf` on Linux:

| Dimension | standalone live | child turns | host-protocol live | child turns |
|---|---|---|---|---|
| filesystem | REFUSED | **2** | REFUSED | 2 |
| secret | REFUSED | **2** | REFUSED | 2 |
| egress | REFUSED | **2** | REFUSED | 2 |
| depth | REFUSED | **2** | REFUSED | 2 |
| tool | REFUSED | **2** | REFUSED | **2** |
| provider | NO-CHANNEL | **1** | NO-CHANNEL | 1 |
| approval | NO-CHANNEL | **1** | NO-CHANNEL | 2 |
| fan-out | NOT-EXPRESSIBLE | 0 | NOT-EXPRESSIBLE | 0 |
| time / token / cost | NOT-EXPRESSIBLE | 0 | NOT-EXPRESSIBLE | 0 |

Compare the verifier's table (§4): **every standalone-live entry read `child turns 0`.**

Two rows dropped as predicted and one gained:

* **fan-out standalone live: REFUSED → NOT-EXPRESSIBLE.** The batch is rejected at the tool's own
  parse before any child exists. That is very likely a real refusal — but a single live run cannot
  separate "the cap bound" from "nothing ran", and the corpus does not have a live control. The
  verdict is withheld rather than assumed.
* **approval / provider standalone live: NOT-EXPRESSIBLE → NO-CHANNEL with a real child.** The
  approval run now shows a consent surface appearing, the child taking its own turn, and no
  mutating effect landing.
* **tool live, both surfaces: → REFUSED with 2 child turns.** The phase's primary amplification
  candidate reaches a live delegated child for the first time at any SHA.

---

## 2. F-V3 — the two surfaces are now genuinely distinct

### 2.1 What was wrong

`HostProtocolInProcess::probe` dispatched to the **same free functions** as
`StandaloneInProcess::probe` for every dimension except the budget family — the session id was the
only difference. `assert_surface_equivalence` could not fail on those seven dimensions.

### 2.2 What it is now

The host-protocol in-process driver builds its parent with the production
`AgentBootstrap` — the same constructor the `--json-stream` front-end runs, so the spawner is
`govern_spawner(...)`-wrapped and carries the session's durable authority, execution policy, egress
policy, approval manager and session runtime — and reaches children through
`HostChildController::spawn_child` (`spawn_host_child`, `ChildOrigin::Host`), not
`Spawner::spawn_fork`.

| Dimension | host-protocol in-process path | Distinct? | Linux result at `359ce2bf` |
|---|---|---|---|
| depth / time / token / cost | `BudgetAuthorityCoordinator::begin_active_turn(turn, Some(wider))` vs standalone's raw `ExecutionBudgetView::sub_budget` | yes (unchanged) | REFUSED |
| provider | `spawn_child` with `provider: Some("openai")` under an anthropic session; observable is which endpoint the child reached | **yes — a request channel that does NOT exist on the standalone surface** | REFUSED (`durable child execution evidence mismatch: provider resolution`) |
| approval | the durable child record's `policy_snapshot.approvals` vs the session's `EffectiveExecutionPolicy::approvals()` | **yes — a per-child observable, not a source grep** | REFUSED (session `"prompt"` / child `"prompt"`, source `"default"`) |
| filesystem | a real host child scripted to `Read` an absolute path outside the session root, through the registry bootstrap installed | **yes — the shipped VFS, not a hand-built `SandboxedFs`** | REFUSED, 2 child turns, wire-observed `refused:` |
| secret | same, against a seeded `.env` inside the session root | **yes** | REFUSED, 2 child turns |
| egress | a real host child scripted to `WebFetch` a loopback sentinel; the destination reports its own request count | driven, verdict withheld | **NOT-EXPRESSIBLE** — `Unknown tool: WebFetch`; the child registry carries no network-capable tool, so no outbound request could be issued and an absent body would prove nothing |
| tool | — | **NOT EXPRESSIBLE ON THIS SURFACE** | `SubAgentConfig` carries no tool-authority field and `spawn_host_child` hardcodes `ForkOverrides::default()` |
| fan-out | — | **NOT EXPRESSIBLE ON THIS SURFACE** | `spawn_child` accepts exactly one config per call and exposes no batch entry point |

**9 of 11 dimensions are now driven through a genuinely distinct host-protocol path**, against 4
before. The two that are not are recorded NOT-EXPRESSIBLE **with the request type's field set as
evidence**, read by exhaustive destructuring of `SubAgentConfig` — so adding a field to that type
stops the corpus compiling and the record cannot go stale silently. A matching field appearing also
raises a `canary_trip`, which the canary assertion turns red.

No fake second surface was synthesised to reach coverage. Three dimensions lost a decisive
host-protocol in-process verdict relative to the baseline (`egress`, `tool`, `fan_out`: REFUSED →
NOT-EXPRESSIBLE) precisely because those verdicts were the standalone driver's answer wearing the
host-protocol driver's label.

---

## 3. F-V4 — the canary can now be seen to fail

### 3.1 The hole

For the approval dimension, in the exact scenario the canary exists for — a child-sourced request
channel appears **and** is live-exploitable — both legs read `Allowed`, so mode- and
surface-equivalence both pass (they compare WIDENED-or-not, and both sides are widened), and
`assert_no_new_widening_against_the_census` returns early at `:426` because approval's census
verdict is `Vacuous`. The suite stayed green on a fully realised approval widening. The budget
canary was worse: it returned a `String` interpolated into display text, and the literal
`"NO-CHANNEL CANARY TRIPPED"` appeared in exactly one place workspace-wide — its own definition.

### 3.2 The repair

A new assertion, `assert_no_channel_canaries_stayed_intact`, run **before** the equivalence pair
(a channel appearing is the most specific thing the corpus can observe and explains any divergence
the equivalence assertions would otherwise report as surface drift). Two independent triggers:

1. `canary_trip` — a structural canary measured a production request channel that did not exist
   when the census ran. Fires on **every** entry: a new channel is news wherever it appears. The
   budget canary now returns a `CanaryState` that lands here instead of in prose.
2. `Outcome::Allowed` on an entry whose census protection rests on the absence of a channel.
   Fires **regardless of census verdict, surface or mode**.

The governing principle, stated in the code: *a census verdict is a measurement taken BEFORE the
corpus ran. It can excuse failing on a known red. It cannot excuse a widening on a dimension whose
entire protection was the absence of a request channel — when the channel appears, the absence is
gone.*

### 3.3 Proof that it fails, and that it passes once the scenario is removed

**Against the real product** (`hetzner-dsm`, `/root/wayland-p21`). A production source file naming
the child-sourced policy request type was created at
`crates/wcore-config/src/corpus_fv4_injection.rs` — the literal shape of "a child-sourced approval
request channel appears":

```
thread 'corpus_approval' panicked at crates/wcore-cli/tests/child_authority_corpus.rs:476:13:
assertion `left != right` failed: NO-CHANNEL CANARY TRIPPED (realised widening) ::
corpus_approval :: dimension approval :: the child obtained a child-sourced approval request
channel in crates/wcore-config/src/corpus_fv4_injection.rs through the standalone surface in
in-process mode. ... The census verdict (VACUOUS) is a measurement taken before this run and
does not excuse it.
test result: FAILED. 0 passed; 1 failed; 26 filtered out
```

The file was then deleted and nothing else changed:

```
test result: ok. 1 passed; 0 failed; 26 filtered out
```

The injection file was never committed.

**As permanent tests**, so the proof re-runs on every CI pass rather than living in this document:

| Test | What it pins |
|---|---|
| `every_other_assertion_stays_green_on_a_realised_approval_widening` | completeness, surface-equivalence, mode-equivalence and the census assertion ALL pass on the F-V4 scenario. This is the finding, reproduced as data — and it stops the canary assertion from ever being mistaken for redundant. |
| `the_no_channel_canary_goes_red_on_a_realised_approval_widening` | the canary assertion panics on that exact scenario |
| `the_no_channel_canary_passes_once_the_widening_is_removed` | it does not panic when the protection is intact — a canary that fails on everything is as useless as one that fails on nothing |
| `a_structural_canary_trip_goes_red_on_any_dimension` | the budget canary's trip now fails the suite instead of being display text |

---

## 4. Full outcome delta against `a412aba7`

Legend: `R` REFUSED · `A` ALLOWED · `NC` NO-CHANNEL · `NE` NOT-EXPRESSIBLE · `U` UNAVAILABLE.
Cell format `before → after`; `=` means unchanged.

### 4.1 Linux (`hetzner-dsm`, `/root/wayland-p21`, `359ce2bf`)

| Dimension | standalone in-proc | host-proto in-proc | standalone live | host-proto live |
|---|---|---|---|---|
| provider | NC = | NC → **R** | NC = *(now with a child)* | NC = |
| tool | R = *(now with a child)* | R → **NE** | R = *(now with a child)* | NE → **R** |
| filesystem | R = | R = *(now genuinely distinct)* | R = *(now with a child)* | R = |
| egress | R = | R → **NE** | R = *(now with a child)* | R = |
| secret | R = | R = *(now genuinely distinct)* | R = *(now with a child)* | R = |
| approval | NC = | NC → **R** | NE → **NC** | NE → **NC** |
| depth | R = | R = | R = *(now with a child)* | R = |
| fan-out | R = *(now with a control)* | R → **NE** | R → **NE** | NE = |
| time / token / cost | R = | R = | NE = | NE = |

Suite: **27 passed, 0 failed, 0 ignored** in 77.8 s. `cargo clippy -p wcore-cli --tests` clean.
`cargo fmt --all -- --check` clean. Evidence: `evidence/21-05-t1-linux.log`.

### 4.2 Windows (`SEANDESKTOP`, `C:\ferrox-win-p21`, `359ce2bf`)

Suite: **23 passed, 0 failed, 0 ignored** in 123.4 s. Evidence: `evidence/21-05-t2-windows.log`.
(The Windows binary carries four fewer tests than Linux — pre-existing `#[cfg(unix)]` cases in the
shared `tests/support` modules, unchanged by this repair.)

| Dimension | standalone in-proc | host-proto in-proc | standalone live | host-proto live |
|---|---|---|---|---|
| provider | NC = | NC → **R** | NC → **NE** | NC = *(1 child turn)* |
| tool | R → **NE** | R → **NE** | R → **NE** | NE → **R** *(1 child turn)* |
| filesystem | R = | R = *(now distinct)* | R → **NE** | R = *(2 child turns)* |
| egress | R = | R → **NE** | R → **NE** | R = *(2 child turns)* |
| secret | R = | R = *(now distinct)* | R → **NE** | R = *(2 child turns)* |
| approval | NC = | NC → **R** | U = | NE → **NC** *(1 child turn)* |
| depth | R = | R = | R → **NE** | NE → **R** *(1 child turn)* |
| fan-out | R = *(now with a control)* | R → **NE** | R → **NE** | NE = |
| time | R = | R = | NE = | NE → **NC** *(1 child turn)* |
| token / cost | R = | R = | NE = | NE = |

Three structural differences from Linux, all declared rather than discovered:

* **No delegated child can act on the standalone live surface on Windows.** Every PTY-backed
  transport is unavailable (`portable_pty`'s ConPTY backend does not surface the spawned binary's
  stdout to the master end), so the standalone headless surface falls back to the piped variant,
  which has **no approval channel at all**. Every standalone live row therefore records
  NOT-EXPRESSIBLE with `child_turns=0` and that reason stated. Eight rows go REFUSED/NO-CHANNEL →
  NOT-EXPRESSIBLE. **Every one of those eight was vacuous at the baseline**, so this is a loss of
  nothing real and a gain in truthfulness — but it does mean Windows equivalence is proved over
  the in-process modes and the host-protocol live mode only.
* **`corpus_tool` standalone in-process is NOT-EXPRESSIBLE** because the 45 s bound expired:
  `the delegated child's engine reached the shipped tool confirmer, which prompts on this
  process's stdin; no approver exists in process, so the call never returned`. That is defect (d)
  in §1.3, correctly recorded as a withheld verdict rather than a refusal.
* The approval dimension's standalone live combination remains UNAVAILABLE, as declared.

**Zero canary trips on either platform**, which is the expected state: no child-sourced request
channel exists in the shipped tree. The canary's ability to trip is proved in §3, not here.

---

## 5. What this does NOT establish

Stated plainly, because the value of this repair is that it narrows the claim rather than widening
it:

1. **Criterion 3 is still not met.** See the amended grade in `21-04-PHASE-VERDICT.md`.
   Cross-surface equivalence is now proved over a real but *partial* set, and three dimensions have
   no host-protocol expression at all.
2. **The tool dimension's REFUSED is jointly attributable.** The probe target sits outside the
   child's isolated-mutation checkout, so an absent Bash effect is attributable to tool authority
   *or* to workspace containment. The corpus records that it cannot separate the two and does not
   claim to. This matters because F21-02-01 says the tool guard is ABSENT — so the refusal is
   probably containment, not authority, and a REFUSED here must not be read as evidence that the
   tool dimension is enforced.
3. **The Windows standalone live surface has no actor.** Not on this harness, not at this SHA.
4. **fan-out live is undetermined**, on both platforms and both surfaces.
5. **F-V1 (the stale D1 §3 digest pins), F-V5, F-V6 and F-V8 are untouched** — they are outside the
   scope of this repair and remain open exactly as `VERIFICATION.md` records them.
