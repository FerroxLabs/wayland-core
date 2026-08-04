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
3. **The two Windows runners were label-identical but NOT interchangeable.**
   `ferrox-win-msvc` probes AppContainer available; **SEANDESKTOP's real-spawn
   probe FAILS**. Jobs landed on whichever was free. Pinned to a new
   `appcontainer` label (`1b5aaa66`).

> **This is the explanation for the "failure set churns between runs" that was
> being written off as flakiness.** It was never flaky. It was which box served
> the job. Same tree, two verdicts. Anyone re-opening a "flaky Windows test"
> should check the runner first.

**#138 and #164 were both STALE and actively misleading** — they were cited
twice as reasons to route Windows to the hosted pool. Corrected in the tracker.

---

## 3. Open before a tag

| item | state |
|---|---|
| `SEANDESKTOP` AppContainer probe fails | **host config, Sean's box.** Off the critical path now, but the Windows pool is ONE runner until it's fixed or given the `appcontainer` label |
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
