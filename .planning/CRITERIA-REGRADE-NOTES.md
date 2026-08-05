# criteria-regrade — working notes (append-only, committed continuously)

Lane: `lane/criteria-regrade`. Worktree:
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-criteria-regrade`.

## Measured SHA

`570056c160a7e497e67bbfe9798aaf3843ac639c` (`fix(lockfile): restore --locked builds, red at
integration head`, 2026-07-30 15:35:02 +0700).

Verified: worktree toplevel is the lane path; `/usr/bin/git status --porcelain` is EMPTY.
(Proxied `git status --short` returned the literal word `ok` — the documented rtk rewrite.
Every number in this lane comes from `/usr/bin/git` redirected to a file and read with the
Read tool, per LANE-BRIEF §3b.)

## Premise check on the orchestrator brief — done FIRST

| Brief claim | Verdict | Evidence |
|---|---|---|
| "grades measured 2026-07-30 at `71acfd19`" (quoting the file header) | **file header is FALSE** | `71acfd19` is an ancestor of HEAD (`merge-base --is-ancestor` = YES) dated 02:28. But `.planning/CRITERIA-STATUS.md` has FOUR commits: `d6a41ecd` 03:08, `25fb1185` 06:03, `e7ef762c` 10:12, `5014f070` 12:04. Rows were re-graded at least three times AFTER the SHA the header names, and the header was never updated. **22 merges landed between `71acfd19` and the file's last edit.** |
| "nine lanes have merged" since | **TRUE**, relative to the file's LAST EDIT | `5014f070..HEAD` contains exactly 9 merge commits (86 commits total), and they are exactly the nine the brief names. Relative to `71acfd19` the count is 31. |

So the brief's list is right but its anchor is wrong: the honest base is `5014f070`, not
`71acfd19`. Both are recorded below and in the rewritten header.

### The nine lanes that landed after the last edit (`5014f070..HEAD`)

```
5265b203 11:31 merge(glibc-reach): lower the Linux glibc floor 2.39 -> 2.34
3595224b 11:32 merge(discord-live): five message actions against a real Discord server
bf95d6a7 11:34 merge(provenance-comparison): per-site provenance findings for the nine notices
c06e1768 11:43 merge(matrix-live): five message actions against matrix.org, and a HIGH
4a3ed957 11:46 merge(slack-live): five legs live, and Slack's exactly-once claim was false too
8f6c80ad 12:31 merge(journey-gate-honesty): the Windows journey gate can now pass
a903142b 14:58 merge(darwin-ci-selfhosted): the task was already done
12b0c18d 15:07 merge(twilio-whatsapp-identity): delivery identity on the wire
c8524ad8 15:34 merge(whatsapp-bridge): opt-in Node bridge, operator-provided
```

## Method

Grade off code and executed tests at HEAD. Never off a `SUMMARY.md`, a lane report, or a
`####` headline. Two rows in the current file are stale *specifically* because someone graded
off a finding lane's summary.

Every row gets a control in BOTH directions (LANE-BRIEF §3b-iii): can this instrument fail,
and can it pass. Absences get a known-positive in the same invocation (§3b-i).

Where I cannot measure: **NOT MEASURED**, and counted. A skip is not a pass.

## Findings log

### 24-C1 — the row publishes a FALSE customer guarantee. Confirmed off CODE.

Row says *"Exactly-once is 3 of 10 — Slack, Matrix, Discord"*. **At HEAD it is 1 of 10 —
Matrix.** Measured by reading every `supports_outbound_idempotency()` override body:

| adapter | `crates/wcore-channel-*/src/lib.rs` | returns |
|---|---|---|
| Matrix | `:294` | **`true`** |
| Slack | `:283` | `false` |
| Discord | `:368` | `false` |
| Twilio SMS | `:338` | `false` |
| WhatsApp | `:384` | `false` |
| the rest | trait default `wcore-channels/src/lib.rs:141` | `false` |

**Both directions:** the grep returns `true` for Matrix and `false` for four others in the same
sweep, so it discriminates. Known-positive `fn send_message` → 34 files; known-negative
`supports_outbound_zzqq` → 0.

The doc agrees with the code: `docs/delivery-semantics.md:538-551` machine-readable block reads
`matrix = exactly-once` and at-most-once for the other nine.

### 24-C3 — the row's "untouched for every adapter" clause is FALSE.

`edit_message` + `delete_message` are implemented by **5 adapters**: Slack (`:341`/`:373`),
Telegram (`:448`/`:489`), MS Teams (`:470`/`:523`), Matrix (`:393`/`:421`), Discord
(`:499`/`:526`). Every adapter overrides `native_actions()`, and `ActionSupport` is a
three-state enum (`Implemented` / `PlatformHasNoApi` / `NotImplemented`) whose default is
`NotImplemented` — so absence is legible rather than a silent green.

`F24-C3-H5` fix ancestry re-verified independently: `5d4bf4b9`, `44a7cc16`, `7c512fe2` are all
ancestors of HEAD. **Negative control for the ancestry instrument**: my own notes commit is NOT
an ancestor of `570056c1` → instrument discriminates.

### 27-C2 — the row is STALE and no lane applied the reconciliation.

`.planning/27-C2C-BASELINES.md` §0 measured the parked blocker **false in both halves**: hetzner
has Xvfb + XTest present, and `@askjo/camofox-browser` 1.13.0 installs and serves
`HTTP=200 {"browserConnected":true}`. That file says explicitly *"I am not editing
CRITERIA-STATUS.md — `lane/release-rank` owns it. This file is the input for that
reconciliation."* **The reconciliation never happened.**

Per the brief I do not grade off that report. Verified as executable tests at HEAD:

| baseline | file | tests | `#[ignore]` |
|---|---|---|---|
| 1 downloads-root | `wcore-browser/tests/downloads_root_baseline_test.rs` | 2 | 0 |
| 2 approval gate | `wcore-cua/tests/approval_gate_baseline_test.rs` | 2 | 0 |
| 3 process/reaper | `wcore-browser/tests/process_count_reaper_baseline_test.rs` | 3 | 1 (3c, real Camoufox, disclosed) |

### MY OWN INSTRUMENT WAS DEFECTIVE — repaired in-lane per §6b-ii

My first counter matched `#[tokio::test]` exactly and reported baseline 3 as **`tests=0
ignored=2`** — i.e. it manufactured the all-ignored vacuity shape §3.2 warns about. The real
attributes are `#[tokio::test(flavor = "multi_thread", worker_threads = 2)]`.

Repaired matcher `^#\[(tokio::)?test`, self-tested with **three** assertions:

- **A (known-positive)** repaired matcher on the file → **3**;
- **B (the old matcher would have missed it)** `#[tokio::test]` on the same file → **0**;
- **C (known-negative)** repaired matcher on `wcore-browser/src/tool.rs` → **0**.

B is the assertion that proves the repair does anything. Note also `grep -c '#\[ignore'` = 2 on
baseline 3 but only **1** is an attribute; the other is prose inside a doc comment at `:415`.

### Rows re-measured and CONFIRMED unchanged (justification still true)

- **21-C3** `SubAgentConfig|ForkOverrides` under `wcore-protocol/` → **0**; known-positive same
  needle repo-wide → **34 files**. No child-spawn variant in `ProtocolCommand` (0).
  `spawn_host_child_with_overrides` present at `spawner.rs:1163`.
- **22-C1** `issue_goal_control` still has **ZERO call sites** — `engine_bridge.rs:1217`
  definition plus two comments. Known-positive caller-search `run_stop_hooks` → 12 refs across
  7 files, so the search finds callers when they exist. Five typed Goal variants confirmed in
  `ProtocolCommand` (`commands.rs:328-340`).
- **22-C3** dead gate re-confirmed dead at HEAD: `GoalTerminalState` under `orchestration/` →
  **0**; known-positive `ClimbOutcome` in that same directory → **22** (was 21 — the directory
  moved, the needle still cannot hit). Real counts: `wcore-agent/src` **88**,
  `goal/strategy.rs` **62**, file size **45,886 bytes**. `GoalKernel::terminate` still `pub`
  (`kernel.rs:146`).
- **22-C4** `start_iteration` production callers: **2** (`goal/control.rs:431`,
  `goal/fleet.rs:475`), neither a non-Fleet loop owner. Grade holds.
- **23A-C1** all four verbs present in `skill_govern.rs`: `run_list:96`, `run_revoke:212`,
  `run_rollback:238`, `run_promote:256`. `run_skills_promote` delegates (`main.rs:2687-2689`).
- **24-C4** no REST resume route (concept search over `wcore-protocol/src` found only
  `approval_resume` and `execution_policy::resume`, both unrelated); known-positive
  `idempotency` → 5 hits, so the search was alive.
- **24-C5** `journey_receipt_contract.rs` now **39** `#[test]`, **0** ignored (row said 21 — it
  grew).
- **27-C1** chokepoint gate still GREEN: `channel_media.rs:41` imports `admit_bytes`, calls at
  `:295`/`:329`; `attachments.rs:71` calls `admit_local_image`. Known-negative `admit_zzqq` → 0.
- **27-C4** `voice` still absent from `default` (`wcore-cli/Cargo.toml:31`), defined only at
  `:58`. Not in the shipped artifact. Grade holds.

---

# PASS 2 — 2026-07-31, re-graded at `659fa492`

Everything above this line was measured at `570056c1` and is retained as the record of that
pass. This section supersedes it.

## Measured SHA

`659fa4922a62ca9657c600938c6313d017fb859f` (`docs: handoff rev 3 — RC status, and the two Grok
gaps that are not code`, 2026-07-31 13:33:06 +0700).

Verified: worktree toplevel is the lane path; `/usr/bin/git status --porcelain` returned **0
lines** before any edit. Branch fast-forwarded to the integration head with `merge --ff-only`
(`9fc9a2ff` is a verified ancestor), never `reset --hard`.

**`659fa492` is docs-only.** Single parent `58aa0267`;
`git diff --name-only 58aa0267 659fa492 -- crates/` → **0 files**. The code tree graded here is
the tree the five merge gates passed on.

## Premise check on the orchestrator brief — done FIRST

| Brief claim | Verdict | Evidence |
|---|---|---|
| header claims all grades measured at `570056c1` | **TRUE** | the header did say that, and it was honest for its pass |
| "87 commits and 14 lane merges" since | **TRUE relative to rev 2's `674b72c8`, not to `570056c1`** | `570056c1..659fa492` is **228 commits, 32 merges**. Both anchors are real; the brief mixed them |
| "the 24 criterion rows" | **FALSE — there are 18** | the table has 18 rows and the ledger has 18 `####` criterion headings |
| "current tallies: 7 MET / 11 PARTIAL / 6 NOT MET" | **FALSE — it was 5 / 10 / 3** | counted off the table at `570056c1`; `HANDOFF-2026-07-30-EVENING.md:113` independently says *"was 5 MET / 10 PARTIAL / 3 NOT MET"*. 7+11+6=24 matches the row-count error, so the two are one mistake |
| "22-C5, 27-C2 (macOS), 22-C1, 27-C4 have moved" | **ALL FOUR TRUE** | and two more moved that the brief did not name: `24-C3` and `27-C1` |
| "the Mac compiles this repo" | **TRUE, and I used it** | built `wcore-cli` and ran the 22-C1 PTY suite locally |

## New measurement taken by this lane

`cargo test -p wcore-cli --test goal_control_tui_pty -- --test-threads=1` on this Mac
(Darwin 25.3.0 arm64), tree at `659fa492`, lane-private `CARGO_TARGET_DIR`:

```
running 13 tests
test goal_open_is_accepted_by_core_on_a_durable_host ... ok
test advance_without_a_projection_produces_no_goal_on_the_status_line ... ok
test a_near_miss_command_does_not_reach_the_goal_surface ... ok
test goal_open_names_the_cause_on_a_degraded_host ... ok
test a_bare_goal_is_reachable_from_the_palette ... ok
(+8 harness self-tests)
test result: ok. 13 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 25.25s
CARGO_RC=0
```

Under the LANE-BRIEF §0 Darwin exception: single crate, single test target, on a cell hetzner
structurally cannot produce (`#![cfg(unix)]`, and Linux says nothing about Darwin's PTY path).
Raw capture: `.planning/evidence/criteria-regrade-659fa492/22-c1-macos-pty.txt`.

**The row said this leg was NOT MEASURED because "the PTY harness is `#![cfg(unix)]`".** macOS
*is* unix. The real obstacle was the inherited "this Mac cannot compile Rust" belief, refuted
on 2026-07-30. An unmeasured cell should be re-probed when the reason it was unmeasured changes.

## Grade changes

| Row | was | now | what moved it |
|---|---|---|---|
| `22-C5` | PARTIAL | **MET-WITH-STATED-EXCEPTIONS** | M0–M5 + NC1 + XP taken on real Windows (`a787f6de`); the row's two lease premises measured FALSE |
| `24-C3` | NOT MET | **PARTIAL** | its named unmet clause — the end-to-end inbound matrix at a real destination — was driven (`4476b151`); plus config/credential/probe/health/reconnect repairs across five merges |
| `27-C4` | NOT MET | **PARTIAL, release-blocking** | `voice` added to `default` (`8c826c8f`); the ledger pre-registered this exact transition at `CRITERIA-GAP-LEDGER.md:1340` |

## Justification changed, grade held

`22-C1` (macOS leg closed by me) · `24-C1` (Matrix exactly-once now cap-conditional, `810b5f73`) ·
`24-C2` (all three absent legs driven on macOS, `e89356c0`) · `27-C1` (PTY drive + macOS artifact
taken, and the artifact is HIGH #937) · `27-C2` (macOS exception spent) · `27-C3` (both supporting
sentences wrong, in opposite directions).

## Defective criterion found: 24-C3's `idempotency` clause

24-C3's reference adapters are **Discord and email** (`24-03-SUMMARY.md:115`). ADR 0005 records
that Discord does not dedupe on `nonce` (a replayed key produced two messages at the real API)
and that email/SMTP is one of seven platforms exposing no dedup slot at all — *"No amount of
engineering makes option 3 reachable."*

**So neither of 24-C3's own reference adapters can ever prove idempotency.** ADR 0005 re-scoped
24-C1 and stopped. The identical defect in 24-C3 went unnoticed because 24-C3 sat at NOT MET for
unrelated reasons — **an honest NOT MET can hide a permanently-red clause**, which is the general
lesson: ask the reachability question of every row, not only the ones that look stuck.

## Rows re-measured and CONFIRMED unchanged (justification still true at `659fa492`)

- **21-C3** `SubAgentConfig|ForkOverrides` under `wcore-protocol/` → **0 files**; known-positive
  same needle repo-wide → **34 files**. `Spawn` in `commands.rs` → **0**, known-positive `Goal`
  in that same file → **33**. `spawn_host_child_with_overrides` at `spawner.rs:101,1142`.
- **22-C3** dead gate still dead: `GoalTerminalState` under `orchestration/` → **0**;
  known-positive `ClimbOutcome` there → **21**. `wcore-agent/src` → **88**.
  `GoalKernel::terminate` still `pub`, **line moved `:146` → `:174`**. Crate-external production
  callers → **0** of 28 repo-wide; known-positive `.terminate_verified(` → 1.
- **22-C4** `start_iteration` production callers: **2** (`control.rs:431`, `fleet.rs:475`) of 21
  refs. Unchanged; the re-scope holds.
- **23A-C1** four verbs at `skill_govern.rs:96/212/238/256`; known-negative `fn run_zzqq` → 0;
  `run_skills_promote` delegates (`main.rs:1690`, defined `:2768`).
- **24-C1** only production `true` is `matrix/lib.rs:294`; slack `:361`, discord `:368`,
  sms `:338`, whatsapp `:384` all `false`; trait default `wcore-channels/src/lib.rs:144`. The
  one other `true` is a test double at `manager.rs:1301` — which is what makes the sweep
  discriminate. Known-positive `fn send_message` → 34 files.
- **24-C4** no REST resume route (`approval_resume` and `execution_policy::resume` only, both
  unrelated); known-positive `idempotency` in `wcore-protocol/src` → **5**.
- **24-C5** `journey_receipt_contract.rs` → **39** tests, **0** ignored. Crate untouched in range.
- **25-C2 / 25-C4** no node, `wcore-exec-backend` or `wcore-egress` file changed in the range, so
  neither row could have moved. `--i-accept-exfil-risk` → **3** refs against a known-positive of
  **161** `exfil` hits: the concept is present, the interlock is not.
- **27-C1 (gate half)** `channel_media.rs:41` imports `admit_bytes`, calls `:295`/`:329`;
  `attachments.rs:71` calls `admit_local_image`; known-negative `admit_zzqq` → 0.
- **27-C3 (late-MCP)** `f27_media_generation.rs` → **12 tests, 0 ignored**: 7 built-in, 3
  MCP-only, 1 combined, 1 honest-negative, **0 late-MCP**. The `integrate_deferred_mcp` test at
  `main.rs:7372` predates `570056c1` (`838c4d97`, ancestry verified) and is a NoopTransport unit
  test about tool discovery, not media generation.
- **27-C5** `release.yml:129,139` `glibc_floor: "2.34"`; `:654` ELF-header check; `:772` PE COFF
  check. Unchanged, plus a **new unsmoked release asset** (the desktop contract bundle).

## Cross-cutting, not a row: CI is a self-passing gate

`lane/fix-clippy-gate` (`61a561be`, in range): 100 integration runs → **91 cancelled, 5 failure,
2 success, 1 pending, 1 queued**; every sampled cancelled run has `jobs=0`. The `report` job
concluded **SUCCESS on zero tests** — every completed Windows self-hosted job in the last 40 runs
(5 of 5) read green having run nothing. Hard assertion now added to `ci.yml`. Also: the `vx`
toolchain pin does not hold — a clean run came back on rustc 1.97.1 against two files pinning
1.95.0. **No GitHub verdict exists for anything in this range (#158).**
