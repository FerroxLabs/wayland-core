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

### Batch 2 — measured 2026-07-30

**21-C3 — correction block VERIFIED ACCURATE at HEAD.**
`SubAgentConfig|ForkOverrides` under `crates/wcore-protocol/` → **0**; control, same needle
repo-wide → **34 files**. No protocol seam exists, as the correction says.
`spawn_host_child_with_overrides` (`spawner.rs:1163`) and `spawn_child_with_authority`
(`spawner.rs:95`) both present; `spawn_host_child` delegates with `ForkOverrides::default()`
(`:1142`). Corpus uses the new entry point (`child_authority_corpus/surfaces.rs:1425`).
Grade **NOT MET, unchanged** — tool live cells and Windows still open.

**22-C4 — one measurement now stale, grade unchanged.**
`start_iteration` production callers are now **2**, not 1: `goal/fleet.rs:475` **and**
`goal/control.rs:431` (the host `GoalAdvance` path added by 22-C1). Control: 21 occurrences
repo-wide including tests. The substantive gap is unchanged — the second caller is the host
command path, not one of the four non-Fleet loop owners. **PARTIAL.**

**22-C5 — unchanged.** `22-01-JOURNAL-COMPAT.md:225`: *"the Windows legs of M1–M5 were NOT
RUN"*, `F22-06-LEG-WINDOWS: NOT RUN`. No newer artifact in the phase directory. **PARTIAL.**

**23A-C1 — MOVED TO MET; the row and BOTH its stated absences are false at HEAD.**
- `crates/wcore-cli/src/skill_govern.rs` (13.8 KB) ships `run_promote` (`:256`),
  **`run_revoke` (`:212`)**, **`run_rollback` (`:238`)** and `run_list` (`:96`), with a
  revocation journal (`JournalEvent::Revoked`) and `store.live_revocations()`.
- `run_skills_promote` is **no longer a `bail!`** — `main.rs:2658` delegates to
  `skill_govern::run_promote`.
- The flag is **RE-ADVERTISED**, not hidden: `main.rs:475` reads *"23A-C1: RE-ADVERTISED
  because governed promotion now exists"*, and `:489` has a plain `#[arg(long, …)]` with no
  `hide = true`.
- `lane/23a-c1-governed` head `597c3275` **IS an ancestor of HEAD**; it grades
  **MET-for-the-shipped-surface (was PARTIAL)** with every clause live-proven and a control
  that reddens (rollback PARTIAL 11/35 before the atomic fix, 0/35 after).

**24-C2 — correction block accurate; the §3 ranking built on it is stale.**
`webhook:`/`poll:` are refused at add (`wcore-cli/src/cron.rs:52-54`), persisted jobs print
`WILL NEVER FIRE — {reason}` (`:350`), and `event:` has a **real producer**: `cron publish`
(`CronCmd::Publish` → `publish_cmd`, `:112`/`:235`/`:264`). So §3's *"three of eight
advertised trigger kinds can never fire … the worst failure mode in the ledger"* no longer
describes HEAD — they are not advertised and not silently accepted. Grade **PARTIAL**.

**24-C3 — unchanged NOT MET, plus a NEW open HIGH.**
`24-C3-FINISH.md`: *"STILL NOT MET, and this lane does not claim it."* health PASS,
reconnect/reload PARTIAL, media and native actions untouched on every adapter.
**`F24-C3-H5` — `channel reload` registers a new adapter and reports it healthy but never
reloads its inbound access policy, so every message to it is silently denied. Measured,
controlled, NOT fixed.**

**24-C4 — `24-C4-SUPPORT-SUMMARY.md`:** *"24-C4 half two goes NOT MET → MET on Linux. The
criterion goes PARTIAL → MET-WITH-STATED-EXCEPTIONS. The phase's single release blocker is
closed."* `F24-C4-H1` closed; `F24-C2-M1` and `F24-C4-M1` opened.

**24-C5 — MOVED TO MET. The row is the most stale in the ledger.**
`24-C5-FINISH-SUMMARY.md` frontmatter: *"MET. All three platforms drive the 17-step journey
to a verified receipt: Linux and Windows at candidate `978f49d7`, macOS at `eba6e9d7`.
F24-J-H3 fixed and the Windows recovery OBSERVED, not asserted."*
Receipts on disk: `24-C5-finish-evidence/{linux-journey-at-candidate,windows-journey,macos-journey}.log`.
A receipt schema exists and is guarded: `crates/wcore-eval-scenarios/tests/journey_receipt_contract.rs`,
**21 `#[test]` fns**. The row's *"no journey driver exists under `crates/wcore-eval-scenarios/tests/`
… no receipt schema; no receipt on any platform"* is **false on all three counts**.
Same lane: **24-C1's upgrade and rollback are now PERFORMED and observed on all three
platforms, and the 12-of-12 clean tally holds on all three.**

**27-C1 — the row's RED gate is now GREEN; grade unchanged.**
The plan gate requiring `media_intake` in both files now passes:
`wcore-agent/src/channel_media.rs:41` imports `admit_bytes` and calls it at **`:295`** and
**`:329`**; `wcore-cli/src/attachments.rs:71` calls `admit_local_image`, which is
`wcore-tools/src/vision_tools.rs:192` and itself routes through `media_intake`
(`vision_tools.rs:57`, and `:179-180` records that the UNC guard is now applied ONCE inside
`media_intake::admit_open` "when the six media surfaces were consolidated onto one intake").
`crates/wcore-tools/tests/media_intake_unification_test.rs` guards it with paired
audio/image controls. `27-MEDIA-INTAKE.md` still grades **PARTIAL** (PTY drive and macOS
leg outstanding), so the headline stands and only the RED-gate sentence is false.

**27-C2 — (a) FIXED, (b) unchanged. Grade moves NOT MET → PARTIAL.**
- (a) **CLOSED.** `wcore-browser/src/tool.rs:500-501` now records *"the section header here
  MUST be the one the config loader reads (`[browser.policy]`). It named `[browser]`, which
  `#[serde(default)]` silently drops"*, and the text comes from
  `config_hint::disabled_by_default_hint()`, whose snippets emit `[browser.policy]`
  (`config_hint.rs:29,37`) and are round-tripped through the real loader by
  `crates/wcore-agent/tests/browser_config_hint_roundtrip.rs` (6.1 KB, present).
- (b) **STILL OPEN.** `bootstrap.rs:754` is still
  `PluginRunner::new().with_computer_use_advertised(true)` — unconditional. The in-source
  justification is reify-time self-gating, which is a different property from *publishing
  live readiness*.
- Policy baselines **still absent**: `27-BROWSER-VOICE.md:141-142` — downloads-root
  confinement, the CUA approval gate, and process count before/during/after plus one reaper
  interval "still have **no baseline**". `27-BROWSER-VOICE.md` grades **27-C2 PARTIAL**.

**27-C4 — grade unchanged NOT MET; the row's central sentence is false.**
The row says *"NOTHING WAS EXERCISED. No audio flowed on any machine. No interruption. …
`crates/*/tests/` contains no voice test."* At HEAD:
- `crates/wcore-agent/tests/voice_live_capture_mac.rs` (28 KB) exists — **4 test fns, 0
  `#[ignore]`**, no env-gated early return. Control: 502 test files repo-wide.
- `27-VOICE-MAC.md`: capture proven live — a 1 kHz tone detected at ratio **116.66** against
  a same-device same-duration control arm at **1.15**.
- `27-VOICE-BARGEIN.md`: *"NOT MET (3 of 5, up from 1 of 5)"*; barge-in **IMPLEMENTED and
  proven against the REAL `CpalAudioPlayer`, not the mock**.
- `voice` is still absent from every `default` list — `wcore-cli/Cargo.toml:31` reads
  `default = ["remote-registry", "workflow", "monitor", "review_artifact"]`, and `voice`
  appears only at `:58`. The not-in-the-shipped-binary classification **holds**.
- Blocker named by `27-GAPS-SUMMARY.md:140-143`: no local speech-to-text path in the tree.

**27-C5 — moved NOT MET → PARTIAL.**
`27-GAPS-SUMMARY.md:146-166`: *"MET for the shipped release on three platforms. NOT MET for
the candidate."* Three packaged smokes, each a published release archive extracted and
executed on the real OS — macOS aarch64, Linux x86_64 (digest-verified), Windows x86_64
(digest-verified) — **8 PASS / 1 RED, byte-identical grades on all three**, nine probes under
a throwaway `WAYLAND_HOME` with 18 credential variables stripped. The row's *"Zero packaged
smokes ran on zero platforms"* is false. Still open: the phase **candidate** is unsmoked, and
the two aarch64 targets are **NOT MEASURED** (recorded as neither 0 nor passing).

**26-SC2 — NOT A ROW IN THIS LEDGER.**
`/usr/bin/grep -n '26-SC2\|26-C2' .planning/CRITERIA-GAP-LEDGER.md` → **0**; control,
`25-C2` in the same file → **4**. §5 of the ledger states Phases 26/28/29/30 were
deliberately out of scope. The work is real — `26-SC2-PEERS-SUMMARY.md`: peer coverage
**2 of 4 → 4 of 4**, both new peers driven end to end against real installed homes, plus a
LOW found only by driving the real tree (`F26-SC2P-L1`, `peer_version` returned `None`
against a real `~/.grok` whose `version.json` declares `0.2.103`). **There is no row to
regrade, and I am not creating one** — inventing a row would misrepresent the ledger's
declared scope.
