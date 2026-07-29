# LEDGER-REGRADE — working notes

**Lane**: `lane/ledger-regrade`
**Base**: `gh/plan/f20-unified-audit-repair` @ `71acfd19258e0fc7484d80a0a95be3f29d0ee2b5`
**SHA asserted** against `/usr/bin/git ls-remote gh plan/f20-unified-audit-repair` — match.
**Started**: 2026-07-30

## Mandate

Re-measure every `#### <criterion>` row in `.planning/CRITERIA-GAP-LEDGER.md` against HEAD,
correct headline grades where evidence moved, flag stale correction blocks, and name every
row whose falsifier cannot pass in any achievable world. Produce `.planning/CRITERIA-STATUS.md`.

**Product is an accurate record. No product source file is changed by this lane.**

## Row inventory (19 `####` headers, 18 distinct criteria)

| # | line | Row | Headline as written |
|---|---|---|---|
| 1 | 54 | 21-C3 | NOT MET |
| 2 | 127 | 22-C1 | FAILED (one surface of three) |
| 3 | 170 | 22-C3 | FAILED (measured, not built) |
| 4 | 235 | 22-C3 (2nd header, correction) | PARTIAL, not FAILED |
| 5 | 274 | 22-C4 | PARTIAL |
| 6 | 297 | 22-C5 | PARTIAL |
| 7 | 324 | 23A-C1 | NOT MET |
| 8 | 388 | 24-C1 | NOT MET (re-graded) |
| 9 | 490 | 24-C2 | PARTIAL |
| 10 | 547 | 24-C3 | PARTIAL (Linux), NOT MET (macOS, Windows) |
| 11 | 563 | 24-C4 | MET on Linux / HTTP+SSE only |
| 12 | 582 | 24-C5 | NOT MET (no evidence, any platform) |
| 13 | 616 | 25-C2 | NOT MET |
| 14 | 647 | 25-C4 | NOT MET |
| 15 | 678 | 27-C1 | PARTIAL |
| 16 | 701 | 27-C2 | NOT MET |
| 17 | 740 | 27-C3 | NOT MET |
| 18 | 764 | 27-C4 | NOT MET |
| 19 | 813 | 27-C5 | NOT MET |

`22-C3` occupies two `####` headers (rows 3 and 4) — the 2026-07-29 correction was given its own
header rather than being nested, which is why the file has 19 headers for 18 criteria.

`26-SC2` is named in the orchestrator's movement list but **has no row in this ledger** — §5
states Phases 26/28/29/30 were deliberately out of scope. To be verified, not assumed.

## Method

Every measurement gets a control **in both directions** (LANE-BRIEF §3b-iii):
- known-positive: an instrument that reports a hit, proving it is alive;
- known-negative / can-it-pass: the state that would make the check flip, and whether that state
  is achievable at all.

All load-bearing commands via `/usr/bin/grep`, `/usr/bin/git` (LANE-BRIEF §3b — `rtk` rewrites
`grep`, `git log`, `ls`, `cargo`, `git status --porcelain`).

## Progress log

- [x] Worktree created, SHA asserted.
- [x] LANE-BRIEF read in full (§3b-iii and "your brief's MEASUREMENTS are probably stale" noted).
- [x] Ledger read in full; row inventory above.
- [ ] Per-row measurement.
- [ ] `.planning/CRITERIA-STATUS.md`.

---

## Measurements taken (append-only)

### Instrument liveness baseline

`/usr/bin/grep -c 'pub enum' crates/wcore-protocol/src/commands.rs` → **6** (hit).
`/usr/bin/grep -c 'ZZZ_NOT_A_REAL_SYMBOL_ZZZ' …/commands.rs` → **0**, rc=1 (miss).
The instrument reports in both directions.

### 22-C1 — measured 2026-07-30

| Claim | At HEAD | Control |
|---|---|---|
| "`commands.rs` contains **zero** `Goal` variants" | **FALSE** — 5 typed command variants: `GoalOpen`, `GoalDeclareTask`, `GoalAdvance`, `GoalCancel`, `GoalResync` (`commands.rs:328-340`); 33 `Goal` lines total | known-negative above returned 0 in the same file |
| `GoalControlRefused` event exists | **TRUE** — 5 files (`events.rs:1303`, `contract/spec.rs:1765`, `goal/control.rs:76`, `tui/protocol_bridge.rs:1040`, `wcore-agent/tests/goal_control_test.rs`) | known-positive `GoalSnapshot\|goal_snapshot` → 18 files |
| "TUI CONTROLS: **MET** — `issue_goal_control`" (`22-C1-CONTROL-SUMMARY.md:273`) | **OVERSTATED.** `issue_goal_control` is defined at `tui/engine_bridge.rs:1217` and has **zero call sites** — exactly one non-comment occurrence repo-wide, its own definition. No keybinding, no command, no test | **Both directions on the caller-search:** sibling `run_stop_hooks` in the same file returns **12** references including real call sites (`tui/mod.rs:854`, `main.rs:2125/2164/5837`). The instrument finds callers when callers exist |
| "still **2 of 8** fixtures on disk — orchestrator-only" (`22-C1-CONTROL-SUMMARY.md:275`) | **STALE — 8 of 8 present.** `commands/goal_{open,declare_task,advance,cancel,resync}.json` + `events/goal_{snapshot,transition,control_refused}.json`, all under `crates/wcore-protocol/contracts/desktop/v1/`, all in `manifest.json`'s `fixture_inventory`, capability `durable_goals_v1: available` | manifest `counts` reads `commands:23, events:52, fixtures:159`, `contract.minor:10` — exactly the 23/52/minor-10 the lane requested |
| Ledger's TUI file list (`app.rs`, `protocol_bridge.rs`, **`statusline/mod.rs`**, `widgets/statusbar.rs`) | **WRONG IN ONE ENTRY.** Files naming the `Goal` *type*: `app.rs` (23), `engine_bridge.rs` (7), `protocol_bridge.rs` (54), `widgets/statusbar.rs` (4). `statusline/mod.rs` matched only the English word "goal" in a comment at `:174` ("the goal is to neutralize cursor/escape control") | case-sensitive `Goal` vs case-insensitive `goal` separates them |

**My own wrong-needle catch:** grepping `tui/` for the five `ProtocolCommand::Goal*` variant names
returned **0** and looked like "the TUI cannot control". It is a wrong needle — the TUI calls
`wcore_agent::goal::handle_goal_control` directly (`engine_bridge.rs:1227`), not the wire variants.
Caught by checking that commit `a2017d20` ("TUI issues Goal control commands") is an ancestor of
HEAD and reading its diff. Recorded because it is the §3b-iii shape.

### 22-C3 — measured 2026-07-30

- **Falsifier as written is confirmed DEAD at HEAD.** `GoalTerminalState` under
  `crates/wcore-agent/src/orchestration/` → **0**. Known-positive `ClimbOutcome` in the same
  directory → **21**. The adapter lives in `goal/strategy.rs`, so **no achievable world makes this
  gate pass**; it reports FAILED forever.
- Corrected falsifier: `GoalTerminalState` across `crates/wcore-agent/src/` → **88** refs;
  `goal/strategy.rs` alone → **62**; repo-wide → **24 files across 4 crates**
  (wcore-agent 17, wcore-cli 3, wcore-protocol 3, wcore-types 1). The 2026-07-30 correction's
  "24 files across 4 crates" is **exactly right at HEAD**.
- `goal/strategy.rs` is **45,886 bytes** at HEAD (correction says 41.8 KB — the file grew).
- Five owner result types all adapted in `strategy.rs`: `ClimbOutcome` 8, `CouncilRunResult` 8,
  `WorkflowRunError` 6, `ShardSummary` 6, `DirectOutcome` 11. Control: `FakeOutcomeZZZ` → 0.
- `requires_loop_owner()` at `wcore-types/src/goal.rs:207` — **exhaustive match, no wildcard arm**,
  read in source; enforced unconditionally at `session_journal/reducer.rs:522`.
- **`f68f3ddd` IS an ancestor of HEAD.** The 2026-07-29 correction block's claim
  *"(b) It is NOT in the integration branch"* is **STALE**.
- Residual open, verified: `GoalKernel::terminate` is still `pub` (`goal/kernel.rs:146`).

### 24-C1 — measured 2026-07-30

`acknowledge` and `resend` are implemented in `crates/wcore-cli/src/gateway.rs`:
`resend_needs_confirmation` (`:820`), `async fn resend(… also_ack: bool)` (`:861`), the
`Acknowledge` doc at `:184`, and a unit test at `:1854`. The 2026-07-29 correction block's
"now built and driven on a real systemd gateway" holds at HEAD.

### 25-C2 / 25-C4 — measured 2026-07-30

`lane/25-hosts` (merge `6861b3aa`, **ancestor of HEAD**) closes both.

- **25-C4's named unmet clause — the SSH-backend orphan scan — is CLOSED.**
  `25-HOSTS-SUMMARY.md:80-150`: two far ends, each checked in **both directions**.
  Far end A (containerised sshd): negative nonce → `0 (MEASURED)` exit 0; **positive, a real
  un-planted orphan the product itself leaked** (`setsid` remote child, controller killed -9) →
  `2 (MEASURED)` exit 1; after reap → `0 (MEASURED)`.
  Far end B (real Windows host): `1 (MEASURED)` exit 1 on the primary `.pid` signal, and a
  before/after fix pair on the secondary sweep (`0` before → `1` after) proving the gate could
  previously only ever report zero. A no-`ps` far end was **built** to prove the `NOT MEASURED`
  branch is reachable rather than dead code.
- **25-C4 Windows egress-denial leg: MET** (`25-C4-WINDOWS-SUMMARY.md:275`), with a vendor-answered
  positive control, a pre-fix fail-open arm proving the gate can fail, and a measured **TLS peer
  identity** excluding local fabrication. The implementing lane grades **Criterion 4 overall
  PARTIAL**, not MET: the machine-create POST is un-denied on both platforms, the
  `--i-accept-exfil-risk` interlock does not exist (owner decision), and the Windows container
  surface could not be enumerated.
- **25-C2: MET as written** — all six properties PASS across `hetzner-dsm` + `SeanD@seandesktop`,
  including a real iptables partition and a second Windows binary built with
  `NODE_CONTRACT_MAJOR = 99`. Attribution HOLDS after all five disruptions, with a negative control
  that differs in the node key **only** (backend key byte-identical). **Recorded dissent kept:** the
  controller cannot verify a node-minted receipt, so a reader who takes "authority attribution" to
  mean *the controller can audit the node* should read C2 as NOT MET. Both readings are in the
  source summary and both are carried forward.

### 27-C3 — measured 2026-07-30

- **`F-27C3-04` is CLOSED**, live-proved on both arms with a live known-negative
  (`F27-IMAGE-DEFAULT-SUMMARY.md:18`).
- The implementing lane grades **C3 NOT MET → PARTIAL** (`27-C3-MEDIA-SUMMARY.md:8`).
  Of the four shapes: built-in and MCP-only exercised, combined measured (MCP **can** shadow the
  built-in with no marker — threat T-27-03-08), **late-MCP NOT EXERCISED** and said so plainly.
