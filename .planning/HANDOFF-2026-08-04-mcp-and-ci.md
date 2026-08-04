# HANDOFF — wayland-core, 2026-08-04 — MCP closed, CI made honest

Integration `plan/f20-unified-audit-repair` @ **`1b5aaa66`**. PR #257 → main.
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

   > ### ⚠ THAT PIN IS WRONG. UNDO IT FIRST. ⚠
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

> **This is the explanation for the "failure set churns between runs" that was
> being written off as flakiness.** It was never flaky. It was which box served
> the job. Same tree, two verdicts. Anyone re-opening a "flaky Windows test"
> should check the runner first.

**#138 and #164 were both STALE and actively misleading** — they were cited
twice as reasons to route Windows to the hosted pool. Corrected in the tracker.

---

## 2b. FIRST TWO ACTIONS NEXT SESSION

Agreed with Sean 2026-08-04, deferred to the next session on purpose.

**1. Revert the `appcontainer` runner pin.** Undo `1b5aaa66` in `ci.yml` (drop
`"appcontainer"` from the three matrix label sets) and remove the label from
runner id **22** (`ferrox-win-msvc`):

```
gh api -X DELETE repos/FerroxLabs/wayland-core/actions/runners/22/labels/appcontainer
```

It rests on a false premise and costs a runner. Do this before anything else so
nobody inherits it as intentional.

**2. Make the AppContainer probe self-reporting.** This is the actual blocker to
diagnosing Windows, and it is a product defect in its own right:

> the refusal says *"the cause was logged by `probe_appcontainer_available` at
> the first execution"* — **and that log never reaches CI output.** The product
> asserts a cause exists, then does not show it. Nobody can act on it.

`probe_appcontainer_available` is at
`crates/wcore-sandbox/src/backends/appcontainer/windows_impl/process.rs:391`.
It does a REAL spawn: `CreateAppContainerProfile` (profile-service RPC) then
`CreateProcessAsUserW`. Carry the Win32 error from whichever call failed into
the refusal MESSAGE rather than a `tracing::error!` the harness swallows.

Only after that does anyone know why both Windows boxes refuse to sandbox — and
it may well be one environment setting rather than code. **Do not send Sean to
change service accounts or policy on a hunch before the probe says what failed.**

*Note for whoever picks this up:* this is the fourth time in one session that a
conclusion drawn from a proxy — a stale ticket, a log line from the wrong job, a
crate I did not test — turned out wrong when finally measured. Measure the
specific thing. It is faster than being wrong twice.

---

## 3. Open before a tag

| item | state |
|---|---|
| AppContainer probe fails on **BOTH** Windows runners | common-mode, cause UNKNOWN. Blocked on making the probe self-reporting — see §2b |
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
