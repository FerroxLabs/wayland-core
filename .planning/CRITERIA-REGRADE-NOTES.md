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
