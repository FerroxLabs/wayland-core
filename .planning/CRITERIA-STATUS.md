# CRITERIA STATUS — one line per criterion, measured 2026-07-31

> **2026-08-01 — the PHASE VERDICT files did not carry these grades, and now they do.** This file
> was current; the per-phase verdicts it corrects were not, and those are what a planner opens
> first. `lane/verdict-truth-text` swept them at `02575b6f` for reds that **cannot pass** and wrote
> dated superseding blocks into `21-04`, `22`, `23A`, `24` and `27`. Four were publishing
> worse-than-true grades (`22-C1`, `23A-C1`, `24-C4`, `27-C4`); `21-C3` was re-derived and came
> back **SOUND**; `24-C1`'s correction runs **against** the product. Table, controls and method:
> **`.planning/VERDICT-TRUTH-2026-08-01.md`**. That sweep is source-measurement only — it
> re-executed none of the live figures below.

**All grades re-measured at `659fa4922a62ca9657c600938c6313d017fb859f`** (`docs: handoff rev 3 —
RC status, and the two Grok gaps that are not code`, 2026-07-31 13:33:06 +0700) **by
`lane/criteria-regrade`, with a control in both directions per `LANE-BRIEF.md` §3b-iii** (can it
fail, *and* can it pass). Full evidence and the superseded text live in `CRITERIA-GAP-LEDGER.md`;
the dated correction blocks there are authoritative over the `####` headlines, which are
deliberately left unedited by that file's convention. Working notes and every raw count:
`CRITERIA-REGRADE-NOTES.md`.

> **`659fa492` is a docs-only commit.** Its single parent is `58aa0267`
> (`chore(contract): regeneration #6 over the merged tree`), and
> `git diff --name-only 58aa0267 659fa492 -- crates/` returns **0 files**. The code tree graded
> here is therefore byte-identical to the tree the five merge gates were green on. That is stated
> because a base SHA that names a docs commit invites the question, and the answer is checkable.

> **The previous header named `570056c1` and that was honest at the time — it has simply
> expired.** `570056c1..659fa492` is **228 commits and 32 merges**. Four rows were known to have
> moved; measurement found **three grade changes and six more rows whose justification is now
> wrong**. The rule this file exists to enforce still holds: **a measurement's SHA is part of the
> measurement**, and a header that outlives its tree republishes stale rows under a fresh date.

**This file exists because the headlines misled.** Of 18 rows, **3 change grade at this pass**
(`22-C5` ↑, `24-C3` ↑, `27-C4` ↑) and **6 more hold their grade on a justification that is now
false or materially incomplete** — see the marked section below, because that second state is the
more dangerous one.

> **Two prior passes of this same file were caught by the defect they were auditing for, and the
> record is kept rather than tidied away.** The 2026-07-30 pass graded `27-C2(b)` off
> `bootstrap.rs:754`, a line that reads `true` forever, while the real fix (`85b60a2f`) was already
> in its own ancestry. That same pass's test-counter matched `#[tokio::test]` exactly and
> manufactured an all-`#[ignore]`d vacuity shape for a file whose attributes are
> `#[tokio::test(flavor = "multi_thread", …)]`; it was repaired to `^#\[(tokio::)?test` and
> self-tested with three assertions, the third being *"the old matcher would have missed it"*.
> **No audit is immune to the failure mode it is auditing for** — which is the strongest argument
> there is for the both-direction control.

**Tally: 6 MET-family / 11 PARTIAL / 1 NOT MET.** Previous pass: 5 / 10 / 3.

| Criterion | Grade | Changed? | One-sentence justification |
|---|---|---|---|
| `21-C3` | **NOT MET** | unchanged | Tool *live* cells remain open and Windows is unmeasured; enforcement is equivalent by construction, so this is a proof gap, not an enforcement hole. Re-verified at `659fa492`: `SubAgentConfig\|ForkOverrides` under `crates/wcore-protocol/` → **0 files** against a known-positive of **34 files** for the same needle repo-wide, and `ProtocolCommand` still has **no** child-spawn variant (`Spawn` in `commands.rs` → **0**, against a known-positive of **33** `Goal` hits in that same file) — the "fenced protocol seam" budget remains fictitious. **Structurally blocked for a lane**: the remaining work needs a `ProtocolCommand` variant, and `wcore-contract generate` is orchestrator-only, so no lane can close this row on its own. |
| `22-C1` | **PARTIAL — Linux AND macOS terminal legs now CLOSED** | **justification CHANGED — the macOS leg is no longer NOT MEASURED** | The zero-call-sites finding was correct and is repaired. Re-verified at `659fa492`: `issue_goal_control` has **10** references, including the real dispatch chain `tui/commands/mod.rs:42` → `engine_bridge.rs:1230/1273`, not the definition-plus-two-comments the row once measured. **Driven live on two platforms, not asserted.** Linux: **13 passed / 0 failed** on `hetzner-dsm`. **macOS, taken by this lane on 2026-07-31 and the reason this row moved: `13 passed; 0 failed; 0 ignored; 0 filtered out` in 25.25s on Darwin 25.3.0 arm64** — including `goal_open_is_accepted_by_core_on_a_durable_host`, whose `goals 1 live / 1` can only be written by the `GoalSnapshot` arm of `apply_event`, i.e. only if Core accepted; plus both negative controls (`/goalzzzzzz` reaches nothing; a cursor-less `/goal advance` puts no Goal on the status line). Evidence: `.planning/evidence/criteria-regrade-659fa492/22-c1-macos-pty.txt`. **NOT MEASURED: the Windows terminal leg** — `goal_control_tui_pty.rs:45` is `#![cfg(unix)]` because `portable_pty`'s ConPTY backend does not surface child stdout to the master end, so this file cannot be the Windows instrument and a Windows leg needs a different one. |
| `22-C3` | **PARTIAL** | unchanged | Half A advanced — the last representable engine-verdict bypass is shut at the durable boundary for 5/5 owners and any sixth; un-goaled invocation stays opt-in; `GoalKernel::terminate` still `pub` (**`kernel.rs:174`** — the row's old `:146` has moved), **visibility JUSTIFIED rather than narrowed, with the measurement in its doc comment**: `pub` cannot reach `Verified` (refused before append and again by the reducer) and cannot reach a Goal holding a live loop-owner claim. Crate-external **production** callers re-measured at `659fa492`: `.terminate(` across `crates/wcore-cli/**` and `crates/wcore-agent/examples/**` → **0**, against **28** occurrences repo-wide and a live known-positive of **1** for `.terminate_verified(`; the sole production caller is `goal/control.rs:482`. Narrow to `pub(crate)` the moment an external production caller appears. Half B closed, pre-existing. **Its falsifier is still a dead instrument** — see the section below. |
| `22-C4` | **PARTIAL** | unchanged (re-scoped 2026-07-31, and the re-scope holds) | `start_iteration` production callers re-measured at `659fa492`: **2** (`goal/control.rs:431`, `goal/fleet.rs:475`), out of 21 references repo-wide. **That is the correct number and is not the gap.** 22-C4 names four loop **policies** (`LoopPolicy`), not the five loop **owners** of 22-C3, and `GoalLoop::run_direct/run_forgeflows/run_council/run_anvil` are generated by one macro that runs the engine exactly once and terminates — single-pass by construction, with no iteration to bound. The genuine remainder: `Fixed` is enforced at the durable boundary and survives restart (measured live, 3 of an authorized 8 across a kill); `Dynamic`'s wall-clock bound, `EventDriven`'s delivery cap and `Manual` have **no runtime enforcement**; **preemption and missed intervals were never driven on any platform.** |
| `22-C5` | **MET-WITH-STATED-EXCEPTIONS** ↑ | **CHANGED — was PARTIAL, and was "the only row where nothing has moved at all"** | The row's single named unmet clause was *"**proved** — Linux only"*, and it is closed. **M0 through M5 were all taken on real Windows** (`SeanDesktop`, NT 10.0.26200, detached worktree `C:\p22` at `2ecdfdf5`, `git rev-parse HEAD` verified): M1 IDENTICAL `sha256=e95de5c1…`, M2 prefix identical with only `last_seq` 13→14, M3 fails closed exit 3 zero stdout, M4 accepts the pre-change snapshot, M5a/b/c lease taken sequentially, refused concurrently, released on exit. **Two controls make it a measurement**: NC1 flips one byte and is refused (`frame 8 digest mismatch`) — the gate can fail; XP reduces the **Linux** journal on Windows to the same canonical `sha256=4f5713e2…`. **Two of the row's own premises were measured FALSE**: the lease half is not `#[cfg(unix)]`-gated (`lease.rs:67` is a full `#[cfg(windows)]` `LockFileEx`), and the mandatory-byte-range-lock concern was already mitigated (`AUTHORITY_LOCK_OFFSET = u64::MAX - 1` locks a sentinel past the largest addressable offset). **Exceptions, stated not embedded:** (i) **neither corpus contains a `tool_execution_*` frame** — the densest region of the reduced state, credential-bound and **Sean-reserved**; (ii) three verifier findings carried forward, not suppressed — M0 was reported with **no artifact**, `GATE-RESULTS-WINDOWS.txt` claims to be a verbatim transcript and is edited, and **the mandatory-lock leg shipped with no positive control**, so nothing showed `ERROR_LOCK_VIOLATION` is reachable on that host and that leg as run could not have failed. |
| `23A-C1` | **MET** (shipped surface) | unchanged | Re-verified at `659fa492`: all four verbs live in `skill_govern.rs` (`run_list:96`, `run_revoke:212`, `run_rollback:238`, `run_promote:256`), known-negative `fn run_zzqq` → 0, and `run_skills_promote` delegates rather than `bail!`s (`main.rs:1690` calls it, defined `:2768`). Not release-blocking. |
| `24-C1` | **PARTIAL** | **unchanged grade — JUSTIFICATION NARROWED AGAIN, see below** | The platform half is closed; the conjunction *"no delivery lost **and** none duplicated"* is not. **Exactly-once is 1 of 10 — Matrix — and now conditionally so.** Re-measured at `659fa492`: the only production `supports_outbound_idempotency()` override returning `true` is `wcore-channel-matrix/src/lib.rs:294`; Slack `:361`, Discord `:368`, Twilio SMS `:338` and WhatsApp `:384` return `false` and the rest inherit the trait default `false` (`wcore-channels/src/lib.rs:144`). The one other `true` in the sweep is a test double (`manager.rs:1301`) — which is what makes the sweep discriminating. **NEW at this pass: Matrix's exactly-once holds only BELOW `max_message_len`.** The multi-chunk arm correctly drops the idempotency key (one key cannot identify N destination messages), and callers now get a truthful **per-message** answer via `manager.rs:792 supports_outbound_idempotency_for`; `docs/delivery-semantics.md` states *"exactly-once below cap, at-least-once above"* instead of a bare label. **DECIDED 2026-07-31 (Sean) — `docs/decisions/0005`: keep at-most-once, no auto-retry, and RE-SCOPE this criterion** to *no delivery is lost **silently***, because as written it had **no reachable pass state**. **Residual is measurement, not design:** at-most-once is still **NOT MEASURED at a real destination on 7 of 10** adapters, and the live over-cap Matrix drive is **BLOCKED** — matrix.org returns `401 M_UNKNOWN_TOKEN`, the credential is dead (**Sean-reserved**, #936). #934 tracks `max_message_len` being asserted against itself at 9 sites. |
| `24-C2` | **PARTIAL** | **unchanged grade — three NOT MEASURED legs CLOSED on macOS, see below** | Re-verified at `659fa492`: `event:` has a real producer (`CronCmd::Publish` dispatched at `cron.rs:248`), and `webhook:`/`poll:` are refused at add with persisted jobs printed `WILL NEVER FIRE — {reason}` (`cron.rs:362-363`). **All three legs the row listed as absent were driven on real macOS 26.3 arm64 by `lane/macos-legs` and all three PASS:** (i) macOS evidence — the hetzner record reproduces exactly, same refusal strings, same `staged (no live dispatcher)`, `history_before=0 history_after=1 queue_after=0`; (ii) the **PTY surface gate** — a real controlling terminal at 40×110 with instrument liveness in both directions in the same run (`ISATTY=1` under the PTY, `ISATTY=0` through a pipe), three rendered screens, `rc_all_expected=True`; (iii) the **kill-mid-fire continuation run** — SIGKILL at the first fire record with **5 events still outstanding on both arms**, restart 1→4 fired / 5→2 queued against a **no-restart control that made no progress**, `fired + queued == published == 6` on both. **The lane recorded its own first verdict criterion as wrong** (`queued_final == 0` measured the rate limiter, not continuation) and preserved the bad log rather than deleting it. **Still PARTIAL and I do not upgrade it:** the *plane* was not built — `webhook` needs an inbound route and credential scheme, `poll` an egress-routed client and a defined response contract, and `max_in_flight` is stored and clamped but not enforced at dispatch. The false promise was retired; the capability was not delivered. |
| `24-C3` | **PARTIAL** ↑ | **CHANGED — was NOT MET. Its named unmet clause was driven end to end at a real destination.** | The row's unmet clause was *"the end-to-end inbound matrix from the binary against a real adapter was never finished"*. At `659fa492` it has been: `UAT-CHANNELS-LIVE.md` records **a real agent turn answered into the real private Slack channel and read back off Slack's API** — configure → gateway → inbound → think → reply, the first time the whole product path ran at a real destination. Clause by clause, measured at `659fa492`: **setup/auth** — `docs/channels.md`'s own config now loads first try, guarded by `channels_doc_configs_load.rs` which lifts TOML out of the shipped doc and runs it through the real loader **and** `channel_factory_for`, proven can-fail by mutating the document itself; a `channel credential set\|list\|remove` verb exists (`channel.rs:158,183,411`), reads the value from **stdin only**, and was driven against real Discord. **access** — proven **both directions** on real Slack with a real-signature injection (ALLOW produced the marker, DENY logged the refusal, and the read-back asserted the known-positive present and the leak absent). **health** — `HealthState::Unauthenticated` went from an unreachable state to **4 of 10** producers (telegram via `ConnectionState::AuthError` → `health.rs:57`; matrix, slack, discord via `ChannelEvent::AuthExpired`), driven live in four quadrants where arms 1 and 4 differ **only in the binary** and 1 and 3 **only in the credential**; `channel probe` no longer exits 0 on a config that fails to parse. **reconnect** — a failed Discord RESUME driven against the real platform (op10 HELLO → op6 RESUME stale → op9 INVALID_SESSION → op2 IDENTIFY → READY), with `Connected` now published on READY/RESUMED rather than before the handshake. **reload** — H5 and H6 both fixed. **native actions / media** — 5 adapters implement `edit_message`+`delete_message` and **9** override `fetch_media` (known-negative `fn edit_zzqq` → 0); five message actions were driven against real Discord, matrix.org and Slack. **Why PARTIAL and not MET:** **idempotency cannot pass on this row's own reference set — see the defective-criteria section**; a **~1s false-Healthy window survives at process start for ALL adapters** (`manager.rs:215` records Healthy unconditionally after start), named not hidden; 6 of 10 adapters still have no auth producer; Slack `probe()` is unimplemented; **7 of 27 UAT cells are UNRUN** because no credential this programme holds can author a *human* message on any of the three platforms; and **macOS and Windows have nothing**. |
| `24-C4` | **MET-WITH-STATED-EXCEPTIONS** | unchanged | Re-verified at `659fa492`: still **no REST resume route** in `wcore-protocol` (concept search over `crates/wcore-protocol/src` found only `approval_resume` and `execution_policy::resume`, both unrelated, against a live known-positive of **5** `idempotency` hits). The transport envelope still needs stating in the release notes: **HTTP/SSE supported; REST `/v1` role-gated but with no resume route and no idempotency handling; stdio and WebSocket have none of the three.** |
| `24-C5` | **MET** | unchanged grade, **with a staleness note the row did not carry** | Driver, receipt schema and three-platform receipts all exist; `crates/wcore-eval-scenarios/tests/journey_receipt_contract.rs` carries **39** `#[test]` fns with **0** `#[ignore]`. **The note:** the three receipts were taken at candidates `978f49d7` (Linux, Windows) and `eba6e9d7` (macOS), **not at `659fa492`**, and `crates/wcore-cli/` has moved by **35 files** since this pass's previous base alone. The grade is MET on the evidence that exists; the evidence is not at this tree. Re-driving the journey at the RC candidate is a cheap and worthwhile pre-tag action. |
| `25-C2` | **MET** (as written) | unchanged | No file under any node/remote-reach crate changed in `570056c1..659fa492`, so this row could not have moved and did not. Carries a recorded **dissenting reading**, deliberately carried forward rather than resolved: the controller cannot verify a node-minted receipt, so a reader who takes "authority attribution" to mean *the controller can audit the node* should read this as NOT MET. `lane/25-hosts` (`6861b3aa`) re-verified as an ancestor of `659fa492`. |
| `25-C4` | **PARTIAL** | unchanged | No `wcore-exec-backend` or `wcore-egress` file changed in `570056c1..659fa492`. The row's named unmet clause stays closed (SSH orphan surface measured on two far ends, each in both directions, with a positive control the product itself leaked); two open items take its place — the un-denied POST path and the missing `--i-accept-exfil-risk` interlock (**3** in-tree references against a known-positive of **161** `exfil` hits, i.e. the concept is present and the interlock is not), which is an owner decision. The Windows **container** surface is still unmeasurable without Docker Desktop, and the product correctly refuses to read its absence as zero. |
| `27-C1` | **PARTIAL** | **unchanged grade — BOTH of the row's NOT-MEASURED legs are closed, and the artifact they produced is a HIGH, see below** | Re-verified at `659fa492`: the chokepoint gate is still GREEN — `channel_media.rs:41` imports `admit_bytes` and calls it at `:295`/`:329`; `attachments.rs:71` calls `admit_local_image`; known-negative `admit_zzqq` → 0. **The row's *"the PTY drive was never taken and macOS still has no artifact"* is now false in both halves — and the artifact is RED.** `lane/macos-legs` drove the real TUI on a real 44×120 PTY with two arms identical except the fixture's directory: `$HOME` → intake succeeded (`HTTP 401 Invalid API Key` from Groq, i.e. bytes were read and uploaded), `$TMPDIR` → `Cannot open audio path component …: Not a directory (os error 20)`, with `MOCK_REQUESTS_CAPTURED=2` on both so neither arm is void. **F-M1-01 (HIGH, macOS only, #937) — FIXED, corrected 2026-08-17.** The grade below was accurate at `659fa492` and is stale at `v0.13.0`: `media_intake::resolve_ancestors` canonicalises symlinked ANCESTORS once before the walk (`media_intake.rs:396-412`, which names #937 in its own doc comment), so `$TMPDIR` under `/var/folders/...` now resolves instead of being refused. The `O_NOFOLLOW` leaf guarantee is untouched and `admit_open` re-runs the deny-list over the resolved path. **The original finding is preserved verbatim below rather than deleted, because the vacuity note travelling with it is still live.** ORIGINAL FINDING, as graded at `659fa492`: `media_intake::open_once` walks from `/` with `O_RDONLY\|O_DIRECTORY\|O_NOFOLLOW\|O_CLOEXEC` (`media_intake.rs:403,445`), and on macOS `/tmp`, `/var` and `/etc` are OS-provided symlinks into `/private` while `$TMPDIR` is always under `/var/folders/…` — so **every one of the six consolidated media surfaces refuses every path the platform's own temp APIs hand out**, root-caused at the syscall by an out-of-product `openat` replay. **And a vacuity finding that must travel with it: two of the negative arms are VACUOUS on macOS** — `symlink` and `over-cap` are recorded as refusals but were refused by the component walk before the symlink check or the byte cap was ever reached, so a reader comparing the two platforms' "all arms refused" columns would conclude the gates agree when they do not. |
| `27-C2` | **MET-WITH-STATED-EXCEPTIONS** | **unchanged grade — the macOS exception is REMOVED, see below** | (a) and (b) were already CLOSED. (c), the three policy baselines, is closed on **two** platforms now: Linux (executed at `570056c1` on `hetzner-dsm`: downloads-root **2 passed / 0 failed / 0 ignored / 0 filtered out**; process-count + reaper **running 3, 2 passed, 1 ignored**; CUA approval **1 passed** by default and **2 passed** with `--features x11-test` under `xvfb-run`) and **macOS** (`lane/macos-legs`: baseline 1 **2 passed / 0 failed / 0 ignored / 0 filtered out** including the symlink-escape and naive-prefix arms; baseline 2 **1 passed**; baseline 3 **3 passed / 1 ignored** after being ported to `ps`). **The macOS port is the most valuable single line in this row**, because before it `process_count_reaper_baseline_test.rs` carried `#![cfg(target_os = "linux")]` and on macOS compiled to an empty harness printing `test result: ok. 0 passed` at **exit 0** — a gate that could not fail, and `LISTED_TESTS=0` was the only line that distinguished it. Four mutations were run against the ported baselines and **all four went RED**, including one aimed at the lane's own new `ps` reader (`MUT-3` → 0 passed; 3 failed), proving the new instrument's zeros are not free. **Exceptions, stated not embedded:** (i) **Windows is NOT MEASURED for all three**; (ii) baseline 2's **real-desktop half is NOT MEASURED on macOS** — `baseline_approval_gate_observed_on_real_x11` is `#[cfg(all(target_os = "linux", feature = "x11-test"))]` with no macOS twin, and writing one posts real HID events to the machine Sean is using, so it is a **deliberate non-attempt recorded as a gap, not a pass**; (iii) baseline 3c (real Camoufox) is `1 ignored` on macOS, disclosed not hidden; (iv) the *"must land inside the downloads root"* half is **vacuous in the shipped product** because no backend implements `Download`; (v) the CUA baseline measures the programmatic policy, not the config→policy trust boundary. |
| `27-C3` | **PARTIAL** | **unchanged grade — the row's own supporting sentence was incomplete in both directions, see below** | `F-27C3-04` was fixed and live-proved on the **tool** path — and at `570056c1` that repair was **half done**, which the previous row did not say. `570bfc94` (in this range) found `DEFAULT_IMAGE_MODEL` — what the `wayland-core image` **subcommand** uses — still pointing at a dead arm, measured against `api.fluxrouter.ai/v1` in both directions **in the same minute with the same key**: `flux-image` → HTTP 200 with a 137 KB image, `flux-image-together-flux` → HTTP 401, and `GET /v1/models` lists only the former. It stayed hidden because the failure is 401, not 404, and it fixed **two tests that could not have caught it** (one compared `body["model"]` to `DEFAULT_IMAGE_MODEL`, i.e. the constant to itself, which holds for any value including the dead one). **late-MCP is still NOT EXERCISED**, and the row's evidence for that was wrong: the hermetic fixture (`crates/wcore-agent/tests/f27_media_generation.rs`) *does* serve both wire protocols and *does* make the shape reachable, and it carries **12 tests, 0 ignored** — **7 built-in, 3 MCP-only, 1 combined, 1 honest-negative, and zero late-MCP**. Accounting is **not** consistent across the four shapes and the tree says so on purpose: `mcp_shape_produces_no_product_cost_record_today` pins the gap so it cannot change silently. |
| `27-C4` | **PARTIAL** ↑ | **CHANGED — was NOT MET, and it is now RELEASE-BLOCKING for the reason the ledger itself pre-registered** | The row's single load-bearing clause was *"`voice` is absent from every `default` list … the feature is not in the shipped artifact."* At `659fa492`, `wcore-cli/Cargo.toml:31` reads `default = ["remote-registry", "workflow", "monitor", "review_artifact", "voice"]`. **Voice is in the artifact, proven with a control rather than by reading the manifest**: the "cpal could not bind" string appears in the voice build (1) and not in the no-voice build (0); Linux links `libasound.so.2` as a hard `DT_NEEDED` and macOS links AudioUnit + CoreAudio. The capability half was already live-proven — capture at ratio **116.66** against a same-device control at **1.15**, and barge-in against the real `CpalAudioPlayer` rather than a mock. **The ledger pre-registered this exact transition** (`CRITERIA-GAP-LEDGER.md:1340`): *"If the `voice` feature is ever enabled in a release build, this criterion becomes blocking immediately, because a shipped voice surface with zero interruption evidence is exactly the silent-failure class that blocks 24-C2."* It has been, so it is. **Not MET, and the shipping lane declines to claim it:** `voice_mode → transcribe_audio` in ONE agent flow is **UNPROVEN on all three platforms** (capture and transcribe are each proven; the handoff between them is not); **no product surface enumerates the tool registry headlessly**, so "the tool is REACHABLE" cannot be observed from the CLI — only that the code is linked, which is the same blindness that let 22-C1 sit with zero call sites; **#938** (FluxRouter STT returns 402 `premium_locked` through the product while a direct curl with the same key returns 200 — our own client-side gate may be refusing a request the provider would serve) is OPEN; and `ci.yml:631` still comments *"voice is off by default"*, which is now false. |
| `27-C5` | **PARTIAL** | unchanged grade, **and a new unsmoked release asset**| Three packaged smokes ran on real macOS/Linux/Windows — 8 PASS / 1 RED, byte-identical on all three. **MET for the shipped release, NOT MET for the candidate.** The two aarch64 targets are **MEASURED BUT NOT EXECUTED**, which is a different state from NOT MEASURED: `release.yml:129,139` declare `glibc_floor: "2.34"` for Linux aarch64 with the in-file note *"the binary references no 2.35 symbol, so the achieved floor is 2.34 — measured, not assumed"*, while `:654` replaces the Linux aarch64 smoke with **ELF-header verification** and `:772` verifies the Windows aarch64 **PE COFF machine field** because it "cannot execute on amd64 hosts". **New in this range:** `release.yml` now publishes a `desktop-contract-v1.tar.gz` bundle as a release asset with an `>= 80 files` fail-closed guard — **a new shipped artifact that no packaged smoke covers.** |

## Rows whose GRADE HELD but whose JUSTIFICATION WAS FALSE OR INCOMPLETE

**This is the most dangerous state a row can be in**, because the grade looks re-confirmed while
the sentence a planner actually reads is wrong. **Six rows are in it at this pass.**

- **`22-C1` — its NOT MEASURED cell shrank, and this lane closed it rather than re-reporting it.**
  The row said *"NOT MEASURED: macOS and Windows terminal legs — the PTY harness is `#![cfg(unix)]`
  and no run was taken on either host."* macOS **is** unix, so the harness was never the obstacle
  there; the obstacle was the inherited belief that this Mac cannot compile Rust, which was
  measured FALSE on 2026-07-30. Run here: **13 passed / 0 failed / 0 ignored / 0 filtered out.**
  Windows remains genuinely NOT MEASURED and needs a *different instrument*, not a different host.
- **`24-C1` — narrowed twice now, and the narrowing is customer-facing both times.** *"Exactly-once
  is 3 of 10"* became *"1 of 10 — Matrix"* on 2026-07-30; at this pass it becomes **"1 of 10,
  below `max_message_len` only"**. Each correction made a published guarantee smaller. A reader
  quoting the row without its cap precondition is republishing the same class of claim that Slack
  and Discord were falsified on.
- **`24-C2` — the row's three named absences are all present.** macOS, the PTY surface gate and
  the kill-mid-fire continuation run were all driven and all pass. The grade survives on the
  *plane*, not on the measurement gap the row describes.
- **`27-C1` — NEW, and the worst of the six.** *"The PTY drive was never taken and macOS still has
  no artifact"* is false in both halves, and the artifact is a **HIGH that Linux structurally
  cannot see** (#937). Anyone reading the old sentence would rank this row as cheap
  proof-gap work; it is an open macOS defect across six media surfaces.
  **Corrected 2026-08-17: #937 is FIXED as of `v0.13.0`** (`media_intake.rs:396-412`). It was
  an open macOS defect when graded at `659fa492` and is not one now. The vacuity finding that
  travelled with it — two macOS negative arms refused by the component walk before the symlink
  check or the byte cap was reached — was NOT re-verified here and is still to be treated as open.
- **`27-C2` — the macOS exception is spent, and the way it was spent is the finding.** The port of
  baseline 3 replaced a suite that reported `ok, exit 0` while running **zero** tests. The row's
  "Linux only" was true; what it did not say is that the macOS column was not empty, it was
  **falsely green**.
- **`27-C3` — NEW, wrong in both directions at once.** Its "fixed and live-proved" overstated the
  `F-27C3-04` repair (the subcommand arm was still dead), and its "late-MCP is NOT EXERCISED"
  understated the tree (the fixture exists and three of four shapes are exercised). The
  conclusion held; neither supporting sentence did.

## Defective criteria — rows with no reachable pass state

**A gate that cannot PASS is as worthless as one that cannot fail** (`LANE-BRIEF.md` §3b-iii).
These are **not** NOT MET. NOT MET means "we have not built it"; these mean "the row is broken".

- **`24-C1` — FOUND AND FIXED, retained for the pattern.** *"No delivery lost **and** none
  duplicated"* is unsatisfiable on seven platforms for reasons outside the codebase. Re-scoped by
  `docs/decisions/0005` (Accepted, Sean, 2026-07-31) to *"no delivery is lost **silently**; every
  outcome-unknown delivery is recorded and recoverable by an operator"* — already true and
  shipping (`wayland-core gateway abandoned`, #109).

- **`24-C3`'s `idempotency` clause — NEW AT THIS PASS, AND IT IS THE SAME DEFECT, UNFIXED.**
  24-C3 reads *"**Reference channels** prove setup/auth, access, routing, media, native actions,
  **idempotency**, reconnect/reload, and health."* The reference adapters are named in
  `24-03-SUMMARY.md:115` as **Discord and email**, chosen for their deliberately different shapes.
  Per ADR 0005: Discord *"does not dedupe on `nonce`"* — a replayed key produced **two** messages
  at the real API — and email/SMTP is one of the seven that *"expose no dedup slot at all … No
  amount of engineering makes option 3 reachable."* **So neither of this row's own reference
  adapters can ever prove idempotency, and the clause has no reachable pass state.** ADR 0005
  re-scoped 24-C1 and stopped there; the identical defect in 24-C3 was not noticed because 24-C3
  was sitting at NOT MET for unrelated reasons and nobody asked whether its *full* pass was
  reachable. **This is the argument for asking the reachability question of every row, not only
  the ones that look stuck** — a row can hide a permanently-red clause behind an honest NOT MET.
  **Recommended disposition:** apply ADR 0005's re-scope verbatim to 24-C3's idempotency clause
  (*the adapter honours the platform's primitive where one exists, and declares its absence
  truthfully where one does not*), which is already what the code does and what
  `delivery_semantics_declaration` build-fails on. Owner decision, not a lane's.

- **`21-C3` — not defective, but structurally unclosable by a lane.** Its remaining work needs a
  child-spawn variant on `ProtocolCommand`, and `wcore-contract generate` is reserved to the
  orchestrator. The pass state is reachable; **no lane can reach it**, which is a different
  problem with the same symptom (a row that never moves) and should not be mistaken for this one.

## Rows graded off instruments that could never pass — or could never fail

- **`22-C3` — STILL DEAD, re-confirmed at `659fa492`.** Its falsifier greps
  `crates/wcore-agent/src/orchestration/` for `GoalTerminalState`; the adapter lives in
  `goal/strategy.rs`, so the check reports FAILED **forever**. At this SHA it returns **0**,
  exactly as it always will, while the known-positive in that same directory (`ClimbOutcome`)
  returns **21** — proving the grep is alive and the needle is wrong. Corrected form:
  `GoalTerminalState` across `wcore-agent/src/` → **88**.
- **`27-C2`'s macOS baseline 3 — the inverse defect, and it was live until this range.**
  `#![cfg(target_os = "linux")]` made the suite compile to an empty harness that printed
  `test result: ok. 0 passed` and exited **0** on macOS. **A gate that cannot fail and a gate that
  cannot pass are the same bug wearing different colours**, and this file has now recorded one of
  each in a single row.
- **`27-C1`'s macOS symlink and over-cap arms — vacuous, and they read as agreement.** Both are
  refused before their own check is reached, so the "all arms refused" column matches Linux's for
  the wrong reason.
- **`22-C5`'s Windows mandatory-lock leg — no positive control.** Nothing showed
  `ERROR_LOCK_VIOLATION` is reachable on that host, so as run the leg could not have failed.
  Recorded against the row above rather than treated as a pass.
- **`27-C1` (chokepoint gate) — RESOLVED as an instrument, retained as a warning.** The row's RED
  gate is GREEN. It stays listed because the row text still reads RED and must not be read forward.

**A permanently-red gate proves as little as a permanently-green one.** See `LANE-BRIEF.md` §3b-iii.

## Beyond the rows: the CI signal itself is a self-passing gate

Not a criterion, and it conditions every one of them. Measured by `lane/fix-clippy-gate` in this
range and independently re-measured at merge: across the last **100** runs on integration,
**91 cancelled, 5 failure, 2 success, 1 pending, 1 queued**, and every sampled cancelled run has
`jobs=0` — they never started. Separately, the `report` job **concluded SUCCESS on zero tests**,
because a leg that dies before its test step writes no JUnit, `if-no-files-found: ignore` creates
no artifact, and the `hashFiles(…) != ''` guard then skips the only step that does anything —
which is how **every completed Windows self-hosted job in the last 40 runs (5 of 5)** read green
while running no tests. A hard assertion now fails on zero JUnit reports (`ci.yml`, this range).
Also open: **the `vx` toolchain pin does not hold in CI** — a clean run came back on rustc 1.97.1
against a `vx.toml` and a `rust-toolchain.toml` both pinning 1.95.0.

**Read every "green" in this file as *this programme's gates on `hetzner-dsm`, plus the two
hosts*, never as a GitHub verdict.** No GitHub verdict exists for the work in this range (#158).

## Not in this ledger

`26-SC2` has **no row here** — §5 declares Phases 26/28/29/30 out of scope, and `lane/ledger-regrade`
**refused to create one**, on the grounds that inventing a row would misrepresent the file's declared
scope. The work is real and recorded in `26-SC2-PEERS-SUMMARY.md`: peer coverage **2 of 4 → 4 of 4**.

Also out of scope, and named so a reader does not mistake absence for completion — work merged in
`570056c1..659fa492` with **no criterion row at all**: `lane/decision-record` (durable-session
posture, ADR 0003, a five-boundary crash matrix), `lane/effect-accounting`, `lane/concurrency-safe`
(`doc_tool` atomic publish, 16,843/17,015 torn reads → 0), `lane/core-contract-defects`,
`lane/self-edit-loop`, `lane/boot-walk` (`openat` 20,031 → 10,017), `lane/fix-keyringless-inbound`,
`lane/fix-headless-keyring`, the four TUI repair lanes, the three UAT lanes, `lane/uat-desktop-contract`
(**Desktop does not agree with Core, and the contract is unread by its only consumer**), and
contract regeneration #6. Plus the two Grok gaps, which are **finished work never landed where it
counts**: the recon ledger row was never pasted, and `migrate/grok.rs` (499 lines, 6 inline tests)
has **0** integration tests and was never pointed at a real grok-build install.
**Unscored work is not ungraded work — it is work no grade will ever notice.**

## What I could NOT measure — counted, not skipped

**A skip is not a pass.** No row was fully unmeasurable at `659fa492`, but **10 of 18 carry at
least one NOT MEASURED leg**, and none of those legs is counted toward its grade. Three cells
closed since the previous pass (`22-C1` macOS, `22-C5` Windows, `27-C2` macOS) and are struck from
this table rather than silently dropped.

| Row | The leg I could not measure | What it needs | Why not this pass |
|---|---|---|---|
| `21-C3` | Windows `child_authority_corpus`; tool *live* cells on both surfaces | a Windows run (`SeanD@seandesktop`); the 21-C3-03 confirmer | the live cells need a `ProtocolCommand` variant and `wcore-contract generate`, which is orchestrator-only |
| `22-C1` | the **Windows** terminal leg; the *"consumed later at D2"* clause | a non-ConPTY Windows TUI instrument; D2 is **not observable from this repository** (Desktop-side) | `goal_control_tui_pty.rs` is `#![cfg(unix)]` for a measured reason, so this is a new instrument, not a new host |
| `22-C4` | preemption and missed intervals, on any platform | a driven reconnect/resume run | no harness exists; out of a measurement lane's scope |
| `22-C5` | the `tool_execution_*` journal region, **both** platforms | a working Anthropic key (**Sean-reserved**) | credential-bound |
| `24-C1` | replay at a real destination for **7 of 10** adapters; the over-cap Matrix drive | Telegram/Twilio/Meta/SMTP/Signal/iMessage/Teams credentials we do not hold; a live Matrix token (**Sean-reserved**, #936 — matrix.org returns `401 M_UNKNOWN_TOKEN`) | credential-bound. The design decision is MADE (ADR 0005); what is left is measurement |
| `24-C3` | macOS and Windows entirely; **7 of 27** UAT cells; the `~1s` false-Healthy window at process start | two platform runs; a **human at a keyboard** on Slack/Discord/Matrix — no held credential can author a human-authored inbound | the human-message half is not a tooling gap and cannot be automated around |
| `24-C5` | the journey **at this candidate** | re-drive the 17-step journey at the RC SHA on all three hosts | the existing receipts are at `978f49d7`/`eba6e9d7`; measurement, cheap, and worth doing before a tag |
| `25-C4` | the Windows **container** surface | Docker Desktop on `seandesktop` — the product correctly refuses to read its absence as zero | absent dependency |
| `27-C2` | all three baselines on **Windows**; baseline 2's real-desktop half on **macOS**; baseline 3c (real Camoufox) on macOS | a Windows run; a **disposable** macOS host or an accepted window in which this Mac takes synthetic HID events; `@askjo/camofox-browser` installed | the macOS HID half is a **deliberate non-attempt** — it posts real clicks to the machine Sean is working on, and that is an owner decision, not a lane's |
| `27-C3` | **late-MCP**, the fourth generation shape | one more test against the existing hermetic fixture | *"The fixture makes it reachable; nobody reached it"* — still true at `659fa492`, and now demonstrably cheap: 3 of the 4 shapes already run against that fixture |
| `27-C4` | `voice_mode → transcribe_audio` in ONE agent flow, on all three platforms; whether the tool is *reachable* rather than merely linked | an end-to-end voice drive; a headless surface that enumerates the tool registry | capture and transcription are each proven; the handoff between them is not, and no product surface can show you the registry |
| `27-C5` | **execution** of `aarch64-unknown-linux-gnu` and `aarch64-pc-windows-msvc`; a smoke over the new contract bundle | real aarch64 hosts (M2.4's self-hosted ARM runner, parked) | header/symbol verification is in place and is **not** execution |

## How this pass was measured

Every count above came from an **unproxied absolute-path tool** (`/usr/bin/git`, `/usr/bin/grep`,
`/usr/bin/sed`) **redirected to a file and read with the Read tool**, never from Bash-rendered
output — `LANE-BRIEF.md` §3b, after `--numstat` was measured fabricating counts and after `rtk`
fabricated a commit SHA in this repository on 2026-07-30. Each absence carries a known-positive in
the same capture, per §3b-i.

Executable evidence taken **by this lane** at `659fa492`:

| gate | host | result |
|---|---|---|
| `wcore-cli --test goal_control_tui_pty` | **this Mac** (Darwin 25.3.0 arm64) | **13 passed; 0 failed; 0 ignored; 0 filtered out** in 25.25s — the macOS terminal leg of `22-C1`, previously NOT MEASURED |

Run under the `LANE-BRIEF.md` §0 Darwin exception — single crate, single test target, on a cell
`hetzner-dsm` structurally cannot produce because the harness is `#![cfg(unix)]` and Linux says
nothing about Darwin's PTY path. Disclosed here because the exception requires it. Raw capture and
provenance: `.planning/evidence/criteria-regrade-659fa492/22-c1-macos-pty.txt`.

Executable evidence **inherited and re-attributed**, all in this range and all read back with the
`0 ignored; 0 filtered out` fields intact:

| gate | host | result |
|---|---|---|
| journal-compat M0–M5 + NC1 + XP | **SeanDesktop**, NT 10.0.26200 | all legs taken; NC1 reddens on one flipped byte; XP agrees with Linux on canonical JSON |
| `wcore-browser --test downloads_root_baseline_test` | macOS 26.3 arm64 | **2 passed; 0 failed; 0 ignored; 0 filtered out** |
| `wcore-cua --test approval_gate_baseline_test` | macOS 26.3 arm64 | **1 passed** |
| `wcore-browser --test process_count_reaper_baseline_test` | macOS 26.3 arm64 | **3 passed; 1 ignored** (was `0 passed` at exit 0 before the port) |
| the same four, plus the `x11-test` arm under `xvfb-run` | `hetzner-dsm` | unchanged from the previous pass |
| `wcore-channels-registry --test delivery_semantics_declaration` | `hetzner-dsm` | **8 passed; 0 failed; 0 ignored; 0 filtered out** — the drift test that fails the build if the doc and the adapters disagree |
