# CRITERIA STATUS — one line per criterion, measured 2026-07-30

**All grades re-measured at `570056c160a7e497e67bbfe9798aaf3843ac639c`** (`fix(lockfile): restore
--locked builds`, 2026-07-30 15:35:02 +0700) **by `lane/criteria-regrade`, with a control in both
directions per `LANE-BRIEF.md` §3b-iii** (can it fail, *and* can it pass). Full evidence and the
superseded text live in `CRITERIA-GAP-LEDGER.md`; the dated correction blocks there are
authoritative over the `####` headlines, which are deliberately left unedited by that file's
convention. Working notes and every raw count: `CRITERIA-REGRADE-NOTES.md`.

> **The previous header's base SHA was itself false, and that is worth stating before the table.**
> It read *"All grades measured at `71acfd19`"*. `71acfd19` is a real ancestor (02:28), but this
> file was edited **four** times — `d6a41ecd` 03:08, `25fb1185` 06:03, `e7ef762c` 10:12,
> `5014f070` 12:04 — and **22 merges landed between `71acfd19` and the last of those edits**. The
> header named the SHA of the first draft and was never advanced, so rows re-graded at three later
> trees were published under a base that predates them. **A measurement's SHA is part of the
> measurement.** Nine further lanes merged after `5014f070`; those are what this pass re-grades
> against.

**This file exists because the headlines misled.** Of 18 rows, **11 were stale** at the previous
pass and 2 were graded off instruments that could never pass. **At this pass 1 row changes grade
and 3 more hold their grade on a justification that is now false** — see the two marked sections
below, because that second state is the more dangerous one.

> **The re-grade that produced this file was itself caught by the same defect.** It graded `27-C2(b)`
> "unchanged" off `bootstrap.rs:754`, a line that reads `true` forever, while the actual fix
> (`85b60a2f`, *"advertise browser/CUA capabilities on liveness, not linkage"*, 2026-07-28) was
> **already in its own ancestry**. Readiness is published 187 lines later via
> `PluginCapabilitySet::from_verified(..).narrowed_to_live()`, which runs real liveness probes.
> Found by `lane/27-c2b-readiness`, verified independently. **No audit is immune to the failure mode
> it is auditing for** — which is the strongest argument there is for the both-direction control.
>
> **It happened again in this pass, to me, and it is recorded rather than quietly fixed.** My
> static test-counter matched `#[tokio::test]` exactly and reported
> `process_count_reaper_baseline_test.rs` as **`tests=0, ignored=2`** — i.e. my own instrument
> manufactured the all-`#[ignore]`d vacuity shape §3.2 warns about, for a file whose real
> attributes are `#[tokio::test(flavor = "multi_thread", …)]`. Repaired to `^#\[(tokio::)?test`
> and self-tested with three assertions (known-positive → 3; **the old matcher → 0, proving the
> repair does something**; known-negative → 0). The runtime later agreed: `running 3 tests`.

| Criterion | Grade | Changed? | One-sentence justification |
|---|---|---|---|
| `21-C3` | **NOT MET** | unchanged | Tool *live* cells remain open and Windows is unmeasured; enforcement is equivalent by construction, so this is a proof gap, not an enforcement hole. Re-verified at HEAD: `SubAgentConfig|ForkOverrides` under `wcore-protocol/` → **0** against a known-positive of **34 files** repo-wide, and `ProtocolCommand` still has **no** child-spawn variant — the "fenced protocol seam" budget remains fictitious. |
| `22-C1` | **PARTIAL — Linux terminal leg now CLOSED** | **CHANGED 2026-07-31 (`lane/goal-control-wiring`)** | The zero-call-sites finding was correct and is repaired. `/goal` is registered in `CommandRegistry::with_builtins`, parsed by `tui/commands/goal.rs`, and dispatched through `TuiEngine::request_goal_control` → `GoalControlBridge::issue_goal_control` (the handler body, moved off `TuiEngine` so a sync slash path can spawn it without a duplicated copy). **Driven live, not asserted:** `crates/wcore-cli/tests/goal_control_tui_pty.rs` spawns the real binary on a PTY and types `/goal open tui-probe direct 2 prove the surface`; the status line renders `goals 1 live / 1`, which `goal_status_summary` can only produce from an `App.goals` entry written by the `GoalSnapshot` arm of `apply_event` — i.e. only if Core accepted. **13 passed / 0 failed on `hetzner-dsm` (Linux)**, including two negative controls (a near-miss `/goalzzzzzz` reaches nothing; a cursor-less `/goal advance` puts no Goal on the status line). **NOT MEASURED: macOS and Windows terminal legs** — the PTY harness is `#![cfg(unix)]` and no run was taken on either host. |
| `22-C3` | **PARTIAL** | unchanged | Half A advanced — the last representable engine-verdict bypass is shut at the durable boundary for 5/5 owners and any sixth; un-goaled invocation stays opt-in; `GoalKernel::terminate` still `pub` (`kernel.rs:146`) — **visibility JUSTIFIED rather than narrowed, 2026-07-31, with the measurement recorded in its doc comment**: `pub` cannot reach `Verified` (refused before append and again by the reducer) and cannot reach a Goal holding a live loop-owner claim (the half-A reducer fix); what remains is terminating a claim-free Goal in a non-verified category, which IS `ProtocolCommand::GoalCancel`. Crate-external **production** callers measured at **0** (`.terminate(` across `crates/wcore-cli/**` and `crates/wcore-agent/examples/**` → no hits; sole production caller repo-wide is `goal/control.rs:482`); every other caller is an integration test whose purpose is to drive this path and assert the refusal. Narrow to `pub(crate)` the moment an external production caller appears. Half B closed, pre-existing. **Its falsifier is still a dead instrument** — see the section below. |
| `22-C4` | **PARTIAL** | **RE-SCOPED 2026-07-31 — the previous row's clause was measured FALSE** | The caller count is right and its interpretation was wrong, twice. (1) **22-C4 names no loop owners.** Its text is *"session-local fixed/dynamic, event-driven, and manual loops remain bounded across reconnect, preemption, missed intervals, and resume"* — four loop **policies** (`LoopPolicy`), not the five loop **owners** of 22-C3. "the four non-Fleet loop owners the criterion names" describes a clause that is not in the criterion. (2) **Wiring the other four would record noise, not a bound.** `GoalLoop::run_direct/run_forgeflows/run_council/run_anvil` are generated by one macro whose contract is *"claim the one loop owner, run the engine exactly once, and terminate"* (`goal/strategy.rs`, `run_entry_points!`): single-pass by construction, no iteration to bound. Only Fleet loops, and `fleet.rs:475` consumes exactly one durable iteration per wave — which is the bound, enforced in the chain rather than in the driving process. `control.rs:431` is the operator-driven `GoalAdvance`. **2 production callers is the correct number.** The genuine remainder, per `22-04-SUMMARY.md` §Criterion 4: `Fixed` is enforced at the durable boundary and survives restart (measured live, 3 of an authorized 8 across a kill); `Dynamic`'s wall-clock bound, `EventDriven`'s delivery cap and `Manual` have **no runtime enforcement**; **preemption and missed-intervals were never driven on any platform.** That is the gap — not the caller count. |
| `22-C5` | **PARTIAL** | unchanged | The row is accurate as written. **The only row in the ledger where nothing has moved at all** — `22-01-JOURNAL-COMPAT.md:225` still reads Windows M1–M5 **NOT RUN**, `:227` still carries `F22-06-LEG-WINDOWS: NOT RUN`, and `git log` finds **no commit** touching that phase directory since 2026-07-30 03:00. Still the cheapest open item. |
| `23A-C1` | **MET** (shipped surface) ↑↑ | unchanged | **Moved further than any other row — both its earlier texts are now false. No longer release-blocking.** Re-verified at HEAD: all four verbs live in `skill_govern.rs` (`run_list:96`, `run_revoke:212`, `run_rollback:238`, `run_promote:256`) and `run_skills_promote` delegates rather than `bail!`s (`main.rs:2687-2689`). |
| `24-C1` | **PARTIAL** | **unchanged grade — JUSTIFICATION WAS FALSE, see below** | The platform half is closed; the conjunction *"no delivery lost **and** none duplicated"* is not. **Exactly-once is 1 of 10 — Matrix alone.** This row previously said *"3 of 10 — Slack, Matrix, Discord"* and that was a **false customer-facing guarantee**; Slack and Discord were each driven at their real API on 2026-07-30 and each produced **two** messages from a replayed key, both having held the claim on mockito evidence. No-loss fails on **9** of 10. Seven platforms provide no idempotency primitive at all, so closing those is a **product decision, not implementation**, and it is still Sean's and still unmade. |
| `24-C2` | **PARTIAL** | unchanged | Grade unchanged, **but the sentence that made this the ledger's number-one release blocker is no longer true** — §3 item 1 must be re-ranked. Re-verified at HEAD: `event:` has a real producer (`CronCmd::Publish`, `cron.rs:123`, dispatched `:248`), and `webhook:`/`poll:` are refused at add with persisted jobs printed `WILL NEVER FIRE — {reason}` (`cron.rs:362-363`). The false promise was retired; the plane was not built. |
| `24-C3` | **NOT MET** | **unchanged grade — JUSTIFICATION WAS FALSE IN TWO PLACES, see below** | Still NOT MET and the repairing lane declines to claim it — but **not for the reasons the row gave**. Its *"a new HIGH is open and unfixed"* named `F24-C3-H5`, which was **already fixed** when the row called it open (`5d4bf4b9`, `44a7cc16`, `7c512fe2` all verified ancestors of HEAD). Its *"media and native actions remain untouched for every adapter"* is **refuted by code**: **5** adapters implement native `edit_message`/`delete_message` (Slack, Telegram, MS Teams, Matrix, Discord) and **9** override `fetch_media`. What genuinely remains: reconnect is untouched, Linux only, Windows uses a mandatory rather than advisory lock and deserves a real run. |
| `24-C4` | **MET-WITH-STATED-EXCEPTIONS** ↑ | unchanged | Was "MET on Linux / HTTP+SSE only"; the exceptions are now stated rather than embedded in the grade. Re-verified: still **no REST resume route** in `wcore-protocol` (concept search found only `approval_resume` and `execution_policy::resume`, both unrelated, against a live known-positive of 5 `idempotency` hits). The transport envelope still needs stating in the release notes. |
| `24-C5` | **MET** ↑↑ | unchanged | **Was the most stale row in the ledger — all three of its claimed absences are false. No longer release-blocking.** Driver, receipt schema and three-platform receipts all exist; `journey_receipt_contract.rs` now carries **39** `#[test]` fns with **0** `#[ignore]` (the row said 21 — it grew). |
| `25-C2` | **MET** (as written) ↑ | unchanged | Carries a recorded **dissenting reading**, deliberately carried forward rather than resolved: the controller cannot verify a node-minted receipt, so a reader who takes "authority attribution" to mean *the controller can audit the node* should read this as NOT MET. `lane/25-hosts` (`6861b3aa`) re-verified as an ancestor of HEAD. |
| `25-C4` | **PARTIAL** ↑ | unchanged | The row's named unmet clause is **closed** (SSH orphan surface measured on two far ends, each in both directions); two open items it never knew about take its place — the un-denied POST path and the missing `--i-accept-exfil-risk` interlock, which is an owner decision. |
| `27-C1` | **PARTIAL** | unchanged | Grade unchanged, **but the row's RED gate is now GREEN** — that sentence must not be read forward. Re-verified at HEAD: `channel_media.rs:41` imports `admit_bytes` and calls it at `:295`/`:329`; `attachments.rs:71` calls `admit_local_image`. Known-negative control (`admit_zzqq`) → 0. The PTY drive was never taken and macOS still has no artifact. |
| `27-C2` | **MET-WITH-STATED-EXCEPTIONS** ↑ | **CHANGED — was PARTIAL** | (a) and (b) were already CLOSED. **(c), the three policy baselines, is now closed too, and I executed the evidence rather than reading it.** All three exist as real tests and pass at this SHA on `hetzner-dsm`: downloads-root **2 passed / 0 failed / 0 ignored / 0 filtered out**; process-count + reaper **running 3, 2 passed, 1 ignored** (3c needs a real Camoufox binary and is disclosed, not hidden); CUA approval **1 passed** by default and **2 passed** with `--features x11-test` under `xvfb-run`. **Exceptions, stated not embedded:** (i) **Linux only** — macOS and Windows are NOT MEASURED for all three; (ii) the *"must land inside the downloads root"* half is **vacuous in the shipped product** because no backend implements `Download`; (iii) the CUA baseline measures the programmatic policy, not the config→policy trust boundary. |
| `27-C3` | **PARTIAL** ↑ | unchanged | `F-27C3-04` (image tool broken by default on FluxRouter) fixed and live-proved through `ProviderCompat`. **late-MCP is still NOT EXERCISED** at HEAD — a concept search found only `translate_mcp_server_spec` unit tests, against a live known-positive of 53 `mcp` files in the phase directory. The missing media cost record is asserted as a test so it cannot drift; **it was not fixed**. |
| `27-C4` | **NOT MET** | unchanged | Grade survives **for a different reason than the row states**: its "nothing was exercised" sentence is false (live capture at ratio 116.66 vs a 1.15 control; barge-in proven against the real player), but `voice` is absent from every `default` list — re-verified at HEAD, `wcore-cli/Cargo.toml:31` is `default = ["remote-registry", "workflow", "monitor", "review_artifact"]` and `voice` appears only at `:58`. The feature is not in the shipped artifact. |
| `27-C5` | **PARTIAL** | **unchanged grade — JUSTIFICATION WAS FALSE, see below** | Three packaged smokes ran on real macOS/Linux/Windows — 8 PASS / 1 RED, byte-identical on all three. **MET for the shipped release, NOT MET for the candidate.** The row said the two aarch64 targets are **NOT MEASURED**; at HEAD they are **MEASURED BUT NOT EXECUTED**, which is a different state and the row should say which. `lane/glibc-reach` symbol-measured the Linux aarch64 floor — `release.yml:75-83` declares `glibc_floor: "2.34"` for it with the in-file note *"the binary references no 2.35 symbol, so the achieved floor is 2.34 — measured, not assumed."* Neither aarch64 target is run: `release.yml:57-59` replaces the Linux aarch64 smoke with **ELF-header verification**, and `:547` verifies the Windows aarch64 **PE COFF machine field** because it "cannot execute on amd64 hosts". |

## Rows whose GRADE HELD but whose JUSTIFICATION WAS FALSE

**This is the most dangerous state a row can be in**, because the grade looks re-confirmed while
the sentence a planner actually reads is wrong. Four rows are in it; two were already flagged and
two are new at this pass.

- **`24-C1` — NEW, and it published a false customer-facing guarantee.** *"Exactly-once is 3 of 10
  — Slack, Matrix, Discord."* **It is 1 of 10 — Matrix.** Measured off code, not off the doc: the
  only `supports_outbound_idempotency()` override returning `true` is
  `wcore-channel-matrix/src/lib.rs:294`. Slack `:283`, Discord `:368`, Twilio SMS `:338` and
  WhatsApp `:384` all return `false`, and the rest inherit the trait default `false`
  (`wcore-channels/src/lib.rs:141`). **This row was the same defect class as the document beneath
  it** — `docs/delivery-semantics.md` had already been corrected on 2026-07-30 (§8 Discord, §9
  Matrix, and the Slack correction) and this file was not.
- **`24-C3` — NEW, wrong in two independent places.** Its HIGH was already fixed, and its
  *"media and native actions remain untouched for every adapter"* clause is refuted by 5
  edit/delete implementations and 9 `fetch_media` overrides. **Both errors come from grading off a
  finding lane's summary while the repair lane merged afterwards.**
- **`27-C5` — NEW, and it conflates two different states.** "NOT MEASURED" and "measured but not
  executed" are not the same claim, and only the second is true at HEAD.
- **`24-C2` and `27-C1` — pre-existing, still true, still carried.** Both already say in-row that
  their load-bearing sentence must not be read forward. Re-verified at HEAD; both warnings stand.

## Not in this ledger

`26-SC2` has **no row here** — §5 declares Phases 26/28/29/30 out of scope, and `lane/ledger-regrade`
**refused to create one**, on the grounds that inventing a row would misrepresent the file's declared
scope. The work is real and recorded in `26-SC2-PEERS-SUMMARY.md`: peer coverage **2 of 4 → 4 of 4**.

Also out of scope, and named so a reader does not mistake absence for completion: the nine lanes
merged since `5014f070` include work with **no criterion row at all** — `lane/glibc-reach` (a new
`.planning/phases/32-glibc-reach/`, lowering the Linux floor 2.39 → 2.34, which widens *reach* and
is graded by no criterion), `lane/provenance-comparison`, `lane/whatsapp-bridge` (an eleventh
`Channel` reached through the `whatsapp` platform string with an opt-in `backend` key, deliberately
**not** covered by the delivery-semantics drift test because that harness enumerates platforms and
this adapter adds none), and `lane/darwin-ci-selfhosted` (refuted; no change). **Unscored work is
not ungraded work — it is work no grade will ever notice.**

## Rows graded off instruments that could never pass

- **`22-C3` — STILL DEAD, re-confirmed at this SHA.** Its falsifier grepped
  `crates/wcore-agent/src/orchestration/` for `GoalTerminalState`; the adapter lives in
  `goal/strategy.rs`, so the check reports FAILED **forever**. At HEAD it returns **0**, exactly as
  it always will, while the known-positive in that same directory (`ClimbOutcome`) returns **22** —
  proving the grep is alive and the needle is wrong. The corrected form: `GoalTerminalState` across
  `wcore-agent/src/` → **88**, `goal/strategy.rs` alone → **62**, file size **45,886 bytes**.
- **`27-C1` — RESOLVED as an instrument, retained as a warning.** The row's RED gate is now GREEN
  (verified above). It stays listed because the row text still reads RED and must not be read
  forward.
- **`27-C2` — NEW ENTRY, a parked blocker that was false in both halves.** The row parked (c) on
  *"two of three legs are blocked on a display-capable host that hetzner cannot provide."* Both
  halves are false: hetzner has `Xvfb` and `libXtst`/XTEST, and the real Camoufox sidecar installs
  and serves `HTTP=200`. **A blocker nobody re-probes is the same defect as a gate nobody can
  redden** — it holds a row open with no reachable path to closing it. I confirmed the display
  half by *executing* the X11 arm myself (`--features x11-test` under `xvfb-run`, **2 passed**),
  not by reading the report that claimed it.
- **My own test-counter, repaired in-lane.** See the header block. Logged here because §6b-ii is
  explicit that a written-up instrument defect is a defect you have agreed to keep.

**A permanently-red gate proves as little as a permanently-green one.** See `LANE-BRIEF.md` §3b-iii.

## What I could NOT measure — counted, not skipped

**A skip is not a pass.** No row was fully unmeasurable at HEAD, but **10 of 18 carry at least one
NOT MEASURED leg**, and none of those legs is counted toward its grade.

| Row | The leg I could not measure | What it needs |
|---|---|---|
| `21-C3` | Windows `child_authority_corpus`; tool *live* cells on both surfaces | a Windows run (`SeanD@seandesktop`); the 21-C3-03 confirmer |
| `22-C1` | the *"consumed later at D2"* clause | **not observable from this repository** — Desktop-side deliverable |
| `22-C5` | Windows M1–M5; the `tool_execution_*` journal region | a Windows build of `p22_reduce.rs`; a working Anthropic key (**Sean-reserved**) |
| `24-C1` | replay at a real destination for 7 of 10 adapters | Telegram/Twilio/Meta/SMTP/Signal/iMessage/Teams credentials we do not hold; then a **product decision** on at-most-once vs at-least-once |
| `24-C2` | macOS evidence; the PTY surface gate; the kill-mid-fire continuation run | a macOS leg and a PTY drive |
| `24-C3` | Windows reload/lease behaviour (mandatory vs advisory lock); the reconnect half | a real Windows run |
| `25-C4` | the Windows **container** surface | Docker Desktop on `seandesktop` — the product correctly refuses to read its absence as zero |
| `27-C2` | all three baselines on **macOS and Windows** | a display-capable macOS/Windows host; hetzner covered Linux |
| `27-C3` | **late-MCP**, the fourth generation shape | *"The fixture makes it reachable; I did not reach it"* — still true at HEAD |
| `27-C5` | **execution** of `aarch64-unknown-linux-gnu` and `aarch64-pc-windows-msvc` | real aarch64 hosts (M2.4's self-hosted ARM runner, parked). Header/symbol verification is in place and is **not** execution |

## How this pass was measured

Every count above came from an **unproxied absolute-path tool** (`/usr/bin/git`, `/usr/bin/grep`,
`/usr/bin/sed`) **redirected to a file and read with the Read tool**, never from Bash-rendered
output — `LANE-BRIEF.md` §3b, after `--numstat` was measured fabricating counts. The proxy defect
was live in this session: `git status --short` returned the literal word `ok` where
`/usr/bin/git status --porcelain` correctly returned an empty tree.

Executable evidence, all at `570056c1` on `hetzner-dsm`, counts read back per §3.2 (the
`0 ignored; 0 filtered out` fields survived because the log was read from a file, not through the
cargo proxy that strips them):

| gate | result |
|---|---|
| `wcore-channels-registry --test delivery_semantics_declaration` | **8 passed; 0 failed; 0 ignored; 0 filtered out** — the drift test that fails the build if the doc and the adapters disagree |
| `wcore-browser --test downloads_root_baseline_test` | **2 passed; 0 failed; 0 ignored; 0 filtered out** |
| `wcore-browser --test process_count_reaper_baseline_test` | **running 3; 2 passed; 1 ignored** |
| `wcore-cua --test approval_gate_baseline_test` | **1 passed** (default) |
| `wcore-cua --test approval_gate_baseline_test --features x11-test` under `xvfb-run` | **2 passed; 0 failed; 0 ignored; 0 filtered out** |

The fourth and fifth rows are a pair and are the reason the X11 leg is claimed at all: the default
invocation runs **1** test because the second is behind `#[cfg(all(target_os = "linux", feature =
"x11-test"))]`. Reporting the default run alone would have silently dropped the arm that matters.
