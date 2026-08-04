# HANDOFF — wayland-core, 2026-08-04 — MCP closed, CI made honest

Integration `plan/f20-unified-audit-repair` @ **`2e2fb8d3`** (or later). PR #257 → main.
Workspace version **0.12.26**. Nothing tagged. Tagging is Sean's.

---

## 1. MCP is FIXED. It is not what blocks the RC.

Three defects, all landed, all with evidence. Two were found by the Wayland
Desktop lane (`HANDOFF-TO-CORE-2026-08-04-toolsearch-mcp.md`, in
`/Users/seandonahoe/dev/wayland-worktrees/packet-attribution/.planning/`).

| # | Defect | Fix | Proof |
|---|---|---|---|
| 1 | `ToolSearch` matched the WHOLE query as one substring, so any multi-word query could only miss | tokenised, AND over tokens | Desktop's Test B + **mutant verified**: restore the old compare and that test fails, and only that test |
| 2 | `register_mcp_tools` (the BULK path every config server takes) never refreshed the ToolSearch catalog | refresh moved INSIDE `register_mcp_tools` so no caller can forget | Test A pinned in `registry.rs` |
| 3 | A hydrated tool was callable but nothing SAID so, so the model re-searched until the no-progress guard killed the run | match now carries `status: LOADED …` | **live**: `ToolSearch → tiny_ping({}) → PONG-7431`, one search one call |

**Defect 2 is the one worth understanding.** The refresh used to be a caller
obligation and the callers disagreed: `wcore-cli` paired it by hand, `bootstrap`
happened to refresh later for unrelated reasons, and `plugins/mcp_delivery.rs`
never refreshed at all. That asymmetry is why CLI probes could reach MCP tools
while the Desktop path could not — same code, different call path, opposite
outcome.

**Two root causes were REFUTED by measurement. Do not revisit:**
- `truncate_result` breaking the hydration JSON parse at 50 000 chars — the
  observed ToolSearch body is **283 bytes**.
- Scale / provider tool cap / curation — a purpose-built **two-tool** server
  failed identically to tvcontrol's 101, and curation early-returns below
  `top_k=15`.

**NOT verified:** Desktop's acceptance gate — fresh profile, one prompt, the MCP
tool actually executes, on *their* path. Proven on Linux only. That gate is
theirs to run and a tool count is not evidence.

---

## 2. Windows CI was lying, in three separate ways

All three are fixed; each was a measurement I should have taken sooner.

1. **Every commit ran the matrix TWICE** — `event=push` and
   `event=pull_request`, in different concurrency groups so neither cancelled
   the other. The branch sat in ci.yml's push list while PR #257 was open.
   Fixed `c7a1bfbf`.
2. **The hosted Windows leg was redundant**, added under #164 on the false
   premise that self-hosted Windows was down. Now **opt-in via `[ci-windows]`**
   (`7357db3e`). Measured same-run: self-hosted **32.1 min DONE** vs hosted
   **39+ min still running**.
3. **The two Windows runners were label-identical.** Pinned to a new
   `appcontainer` label (`1b5aaa66`) on the belief that `ferrox-win-msvc` could
   sandbox and `SEANDESKTOP` could not.

   > ### ⚠ THAT PIN WAS WRONG. IT IS NOW REVERTED (`df2f81ae`). ⚠
   >
   > Kept here because the reasoning matters, not because action is pending.
   >
   > **`ferrox-win-msvc` fails the AppContainer probe too — 7 occurrences of
   > the same `sandbox UNAVAILABLE` refusal in its own run (30887202242).**
   >
   > The "probed available" lines I based the pin on came from
   > `CI (windows-latest, hosted)`, NOT from the Array leg. I attributed a
   > hosted-runner result to Sean's box and never checked it. The pin therefore
   > sorts jobs onto one runner, **fixes nothing**, and halves the Windows pool.
   >
   > Both runners also run as the SAME account — `NT AUTHORITY\NetworkService`
   > — so the earlier "one sick machine" story is dead. This is common-mode:
   > Windows build, policy, or that service account, or a real product defect.
   >
   > It also means the "churn between runs" explanation is only PARTLY right.
   > Which box served the job is still a real variable, but it is not the
   > AppContainer difference I claimed, because there is no such difference.

**On the "failure set churns between runs", stated carefully.** Which box
served a job IS a real variable and worth checking before calling a Windows test
flaky — the two runners are not guaranteed equivalent. But the specific
AppContainer explanation above is RETRACTED: both runners fail that probe, so it
cannot be what differed. The churn is still not fully explained. Do not close it
as "flaky" and do not close it as "runner difference" either.

**#138 and #164 were both STALE and actively misleading** — they were cited
twice as reasons to route Windows to the hosted pool. Corrected in the tracker.

---

## 2b. BOTH ACTIONS ARE DONE — and the Windows story changed again

Executed 2026-08-04. Two commits: `df2f81ae` (revert) and `a4e0e144` (probe).

**1. The `appcontainer` pin is reverted.** `1b5aaa66` is undone in `ci.yml` and
both runners are back in the pool. The inline comment was REPLACED rather than
deleted — it asserted the false capability difference as fact, and a repo
comment that lies is worse than no comment. It now carries the refutation.

Remaining cleanup, after the push lands: remove the now-unused label from
runner id **22**. Order matters — push the workflow first, because an extra
runner label is harmless while a required-but-absent one strands every job.

```
gh api -X DELETE repos/FerroxLabs/wayland-core/actions/runners/22/labels/appcontainer
```

**2. The probe now reports its own cause.** All four failure arms record why
(thread-spawn refusal, non-zero child exit, `execute_blocking` error, wall-clock
timeout) and the refusal quotes it verbatim instead of pointing at a
`tracing::error!` no CI harness ever emits. Success clears the record, so a host
that recovers cannot keep quoting a stale cause. The no-cause arm deliberately
reads as OUR defect, not the operator's.

### The measurement that changes the picture

Ran the real production path on SEANDESKTOP, as user `SeanD`:

```
OBSERVED: AppContainer settled verdict on this host: Some(true)
OBSERVED: AppContainer executed normally, exit_code=0
```

**SEANDESKTOP sandboxes fine.** The box was never broken. Two theories are now
dead: "one sick machine" (§2 killed that) and "the Windows fleet cannot
AppContainer" (this kills that).

Three explanations are now dead, each killed by a direct measurement on
SEANDESKTOP with the new probe:

| theory | test | verdict |
|---|---|---|
| one sick machine | ferrox-win-msvc run 30887202242 | **dead** — 7 refusals there too |
| the Windows fleet cannot AppContainer | ran as `SeanD` | **dead** — `Some(true)`, exit 0 |
| the `NT AUTHORITY\NetworkService` account | ran the same test AS NetworkService via a scheduled task | **dead** — `Some(true)`, exit 0 |
| parallel-nextest contention on profile creation | full `wcore-sandbox` suite, `--test-threads 16` | **not reproduced** — 153/153 passed, zero refusals |

**So the cause is still unknown, and I could not reproduce the CI failure on
SEANDESKTOP at all.** Do not write this up as solved. What remains different
between my runs and a CI run: a different physical box (`ferrox-win-msvc`, which
I did not test directly), the FULL workspace suite rather than one crate, a `C:`
working directory rather than `D:`, and the runner's own environment. The
original 7 refusals were in other crates' tests, not `wcore-sandbox`'s.

**This is exactly what the self-reporting probe is for.** The next Windows CI
run prints `Cause, verbatim from the probe: <Win32 call>: 0x…`. Read that line
first — it is one run away and it names the failing call and status code.

**Do not send Sean to change a service account, a policy, or a runner
configuration.** Two of the four theories above would have produced exactly that
instruction, and both were wrong.

Verification carried by these commits: clippy clean on
`x86_64-pc-windows-msvc` AND Linux, both `--all-targets`; both new tests pass on
real Windows (`NEXTEST_EXIT=0`); and the cause-carrying assertion is
**mutant-verified** — replacing the two `push_str` lines with `let _ = cause;`
fails exactly that test, on the message it prints.

*Note for whoever picks this up:* the proxy-vs-measurement trap caught me four
times in the prior session and the fix each time was the same — go run the
specific thing on the specific host. Doing that here took ~20 minutes and
overturned the standing explanation twice in one day.

---

## 3. Open before a tag

| item | state |
|---|---|
| AppContainer refuses under Windows CI, but PASSES on SEANDESKTOP every way I could run it | cause UNKNOWN and **not reproducible locally**. Four theories tested and dead — see the table in §2b. No leading hypothesis; do not invent one. The self-reporting probe ships in `a4e0e144` and the next Windows CI run prints the Win32 cause. Read that line before touching any host config |
| `mcp_assistant_scoping_e2e` (Windows) | swarm finding, **unlanded**, self-declared partial |
| `exec-backend conformance_matrix` | swarm root cause **REFUTED** by the adversarial verifier; do not build on it |
| `RC-READINESS.md` re-grade | drafted by swarm, **not applied**; five of seven verified closed, two stale in our favour |
| Desktop end-to-end MCP gate | theirs |
| macOS `voice_live_capture_mac` ×3 | pre-existing, audio hardware |

Swarm findings + unapplied patches: `.planning/evidence/rc-swarm-2026-08-04/`.
**Read its README disposition table first** — three refuters died on a session
limit and the harness counted a null verdict as "survived", so three rows are
unchallenged, not vindicated.

---

## 4. Hard-won operating notes

- **`cargo check --workspace` is not verification.** I ran it plus tests for
  ONE crate, and shipped two regressions in the crates I had actually edited
  (`wcore-mcp`, `Cargo.toml`). Run the tests for what you changed.
- **`CI (Array)` IS the self-hosted Windows job** — GitHub renders
  `${{ matrix.os }}` as "Array" for a label list. It is not a mystery leg.
- **Check live state before quoting a ticket.** #138 said the runner was dead;
  it was serving jobs. I repeated it twice and nearly had Sean kill a live CI
  job on a runner that was working correctly.
- Never `grep 'msvc]'` for the runner labels — they live inside a JSON string
  (`msvc"]`), so it returns nothing and reads as "no workflow targets it".
- `release.yml` now hard-fails on tag/tree version mismatch. The RC would have
  shipped a binary whose `--version` contradicted its own signed manifest.
- Contract corpus regenerates with `cargo run -p wcore-protocol --bin
  wcore-contract -- generate`. A version bump legitimately drifts it.
