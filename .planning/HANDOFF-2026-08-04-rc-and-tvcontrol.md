# HANDOFF — wayland-core — 2026-08-04

Supersedes `HANDOFF-2026-08-03-windows-and-disk.md` (that file's §0 corrections
still stand; everything else here is newer).

**Integration: `1a07ce0b`** on `plan/f20-unified-audit-repair`.
**Lane `lane/172-refresh-finish` is AHEAD at `b778b8ee`** — one commit, the
wasmtime security patch, held back only because CI was in flight on integration.
**FIRST ACTION: push `b778b8ee` to integration** once the branch is quiet.

Setup: `export WL_LANE=core`; `gh auth switch --user FerroxLabs` before every gh
op. Compiles ONLY on hetzner (`/root/orch-gate`), never the Mac. `cargo fmt` is
the one exception.

---

## 0. THE TWO OBJECTIVES

1. **Ship the RC.** `v0.12.26-rc.1`, tag reserved to Sean. The bar he set:
   *nothing lies to the user, and nothing loses their data.*
2. **NEW — make Wayland work with TVControl.** `FerroxLabs/tvcontrol`, for an
   upcoming masterclass with paying customers. See §5. This is not speculative
   work: Sean is presenting it.

---

## 1. #172 refresh single-flight — the release blocker, now MOSTLY closed

**The cross-process proof LANDED.** This was the gate Sean named.

`crates/wcore-agent/tests/refresh_cross_process.rs`. Two real OS processes, one
profile dir, one expired pair on a real on-disk store, a counting endpoint on
loopback. Each child runs the production path (`ChatGptTokenManager::get()`).

- **GREEN: exactly 1 POST**, both processes end authenticated, and the ROTATED
  pair is what is on disk (so the loser adopted the winner's, not the spent one).
- **RED: 2.** Source mutant reproducing pre-#172 behaviour — skip the lock, POST
  anyway. Applied out-of-tree on hetzner, reverted after.

There is deliberately NO runtime lock-bypass switch. A bypass of a security
control that ships in the binary is a worse defect than the one it proves. The
red half is a documented mutant procedure, not a flag.

### What the lane already had (its own WIP message was wrong in BOTH directions)

`93432b1f` said "no proofs, no mutants, §4.9 and §4.10 almost certainly not
addressed. Do not merge." Reading the code instead of the message:

- **§4.4 failure policy is encoded in the TYPE**, not a branch. `Acquisition` is
  `Held | Busy`; there is no "proceed unlocked" state to reach. A panicked
  `spawn_blocking` degrades to `Busy`. Fail-closed by construction.
- **§4.9 logout resurrection** — done, `hold` helper + per-writer table.
- **§4.10 cancellation** — done, in `flow.rs` (which I had not grepped when I
  first reported it missing). `PrimaryGuard::drop` vacates the slot only if it
  still holds OUR cell (`Arc::ptr_eq`) and settles subscribers with `Cancelled`.

### What was actually broken, and is now fixed

1. **It did not compile.** `PERSIST_ATTEMPTS`, `PERSIST_RETRY_DELAY`,
   `INVALID_GRANT_SENTINEL` — all used, none defined.
2. **`MAX_HOLD_SECS` was a claim, not a bound** (cross-audit, Kimi K3, and the
   single most valuable thing the panel produced). The post-POST store write
   reaches `chunked_put`, which takes the credential store's OWN lock with a
   **65 s** ceiling — six times the 10 s budget it nests inside. Every ceiling
   derived from `MAX_HOLD` was therefore sized against a hold that could be
   exceeded several times over. Bounded by `PERSIST_TOTAL_BUDGET` (8 s,
   compile-asserted inside `STORE_BUDGET`). The deadline is consulted only
   BETWEEN attempts — it never cancels a store mid-write, because a half-written
   credential is worse than a slow one.
   **This was liveness, not security**: the heartbeat means a live holder is
   never judged stale, so a second POST was never reachable.
3. **`SUBSCRIBER_CEILING` described its invariant and nothing checked it.** Now
   a `const _: () = assert!` against `refresh_lock::PRIMARY_WORST_CASE_SECS`
   (75 s worst case vs 120 s ceiling).
4. **`PER_CALL_TIMEOUT` was dead code** while `chatgpt.rs` and `xai.rs` each
   declared their own `Duration::from_secs(20)` — three copies of a number whose
   doc claimed they "cannot drift apart silently". Both providers now use it.

`credentials.rs` conflicted with #171's `chunked_put` lock in 7 hunks. Resolved
as a **UNION**: kept integration's `CREDENTIAL_WRITE` (five call sites) and
gained the lane's `pub`, `LockPolicy::new`, and the **heartbeat**. The heartbeat
is why the union matters — without it `stale_after` must exceed the maximum
hold, so a refresh holding tens of seconds forces a staleness of minutes and a
crashed holder wedges every waiter that long.

### STILL OWED on #172 — do not mark it closed

- **P6:** lock unavailable → reload and succeed with **zero** POSTs; and when no
  fresh pair appeared → fail retryably, never POST unlocked.
- **Windows steal semantics** for a hold of tens of seconds (the primitive was
  sized for a sub-second migration).
- **Never POST the real provider.** Local endpoint only. Replaying a single-use
  refresh token can make a compliant provider revoke the ENTIRE grant
  (RFC 6819 §5.2.2.3).

---

## 2. SECURITY — RUSTSEC-2026-0222, patched but NOT YET ON INTEGRATION

`cargo-deny` went red with `advisories FAILED, bans ok, licenses ok, sources ok`.
Nothing we changed caused it — the RustSec DB updated under a static lockfile.

**RUSTSEC-2026-0222 / GHSA-hgjw-h833-99q9**, "Stores can mix up type indices
between engines", against **wasmtime 36.0.12**, reaching us through
`wasmtime-wasi → wcore-plugin-wasm → wcore-agent` — the SHIPPED tree, not a
dev-only path.

Fixed as a patch bump to **36.0.13** (advisory allows `>=36.0.13, <37.0.0`), so
the blast radius is the lockfile alone. `wasmtime-wasi` / `wasmtime-wasi-io`
stay at 36.0.12; they are different crates and are not what the advisory names.
Verified by running the gate's own check, not by reading a version string:
`cargo deny check advisories` → **`advisories ok`**.

**Commit `b778b8ee` on `lane/172-refresh-finish`. PUSH IT TO INTEGRATION FIRST.**

---

## 3. Gate + CI state

**hetzner `/root/gate.sh <ref>` — last full run GREEN on `1a07ce0b`:**
fmt, `--locked`, clippy 0, **positive control 101**, identifier gate 0,
**nextest 13928/13928**.

**NEW THIS SESSION — the gate can now lint Windows code.**
`cargo clippy --target x86_64-pc-windows-gnu --workspace --all-targets` works on
hetzner; the target was already installed. Clippy only CHECKS, never links, so
no MSVC toolchain is needed. Full workspace ~1m36s warm. It is wired into
`gate.sh` after the Linux clippy and **fails loudly if the target is missing**
rather than skipping — a leg that silently does not run reports green.

**Its limit, do not overclaim:** CI builds windows-**MSVC**, this is
windows-**GNU**. Both set `cfg(windows)`, so it catches that whole class, but
`target_env = "msvc"` code stays invisible. A pre-push filter, NOT a replacement
for the Windows CI leg.

Why it exists: a `clippy::needless_return` inside a `#[cfg(windows)]` block
passed a fully green hetzner gate and broke CI. Worse, `just lint` aborts the
whole recipe at the first crate, so wcore-tools/wcore-cli/wcore-config Windows
code was **never linted at all** that run. The cross-target run checked every
crate and proved there was exactly ONE warning. **That fix is now confirmed on
real Windows** — the Array leg gets through lint and runs all 13,537 tests.

### CI red on integration, with causes

| leg | state | cause |
|---|---|---|
| `CI (Array)` self-hosted Windows | RED | `packaged_f04_run_is_repeatable_and_content_addressed` — "OpenAI semantic request bodies diverged", first hash matches then 2/3/4 differ. **Pre-existing** (also red on the pre-merge baseline). Undiagnosed. |
| `OSV Scanner` / `Supply Chain` | RED | RUSTSEC-2026-0222 (§2). Should clear once `b778b8ee` lands. **SBOM byte-determinism** also red — probably needs regenerating against the new lockfile; NOT yet investigated. |
| `CI (macos-latest)` | RED | 3× `voice_live_capture_mac` — identical before and after the merges. Almost certainly needs real audio hardware on the runner. |
| `CI (linux-containerized)` | was RED | 3 failures that were **newly VISIBLE, not new**: the baseline died at a vacuity gate and never ran a test at all. Fixing the gate revealed them. |

**PLATFORM VERDICT ON `1a07ce0b` — the cross-process proof PASSES EVERYWHERE.**
`refresh_cross_process::p1_two_processes_issue_exactly_one_refresh_post` is
GREEN on hosted Windows (2.194 s — a real process spawn, not a short-circuit),
macOS (0.183 s) and Linux. All four CI legs are red, and **none of the failures
is the new test**:

| leg | failing |
|---|---|
| Windows hosted | `packaged_f04`, `matching_assistant_dials_scoped_deferred_server`, `every_reference_backend_passes_the_same_harness_or_reports_why_it_did_not`, and FOUR `wcore-swarm` tests (`dispatches_4_noop_workers_in_parallel`, `multi_worker_output_exhaustion_fails_without_retaining_buffers`, `timeout_releases_workspace_and_capacity_before_return`, + heartbeat) |
| macOS | 3× `voice_live_capture_mac` + `corpus_filesystem` |
| linux-containerized | `packaged_lifecycle_memory_matrix_has_real_effects_and_quarantine` |
| Array | `packaged_f04` |

**Two things got WORSE and are unexplained:** `wcore-swarm` on Windows went from
1 failing (baseline) to FOUR, and `every_reference_backend_...` regressed back
after the merge wave had fixed it. This wave touches no file in `wcore-swarm`.
"Does not touch it" is not a diagnosis — treat both as open.

**Windows verdict from the merge wave, measured:** 4 failures FIXED
(`bash_stderr_is_surfaced`, `bash_success_renders_the_real_exit_code_and_byte_count`,
`every_reference_backend_passes_the_same_harness_or_reports_why_it_did_not`,
`sandbox_exec_confines_a_write_that_escapes_the_workspace`), 3 pre-existing
remain, and **3 NEW appeared in `wcore-swarm`** — a crate the wave never
touched. Swarm went 1 → 4 failing on Windows. Could be contention; not assumed.

---

## 4. Open, ranked

**RC-GATING**
- **Push `b778b8ee`** (wasmtime patch) to integration.
- **#172 P6 + Windows steal semantics** (§1).
- **SBOM byte-determinism** red — uninvestigated.
- **3 new `wcore-swarm` Windows failures** — untouched crate, needs a cause.

**HIGH**
- **#174** retry-masked flake `normal_exit_reaps_owned_descendant_listener`.
  Correlate with `2b662fe8 fix(sandbox): own and reap process trees` before
  assuming coincidence.
- **#175** retry-masked flake `narrow_terminal_resize_stays_coherent_without_panicking`.
  Distinct from #174 — two independent ones, not one recurring.
- **#164 / #138** both self-hosted Windows runners and the Darwin one serve zero
  jobs. **Sean-only.**
- **#165** Desktop must be told `ready.session_id` is now JSON `null` BEFORE the
  Desktop branch merges. Core side done; this is a message to Sean, not code.

**CLOSED THIS SESSION, verified not assumed:** #170, #171, #173, and the four
Windows lanes. The earlier blocker list was stale by four items — **re-grade
before planning off any list in this repo.**

---

## 5. NEW OBJECTIVE — Wayland × TVControl

`https://github.com/FerroxLabs/tvcontrol` — public, MIT, v2.2.0, 34 stars.
"TradingView MCP System. 88 MCP tools driving symbols, indicators, Pine,
snapshots, sweeps, replay, live chart vision. All local, zero cloud calls."

### The integration shape is simple and already supported

tvcontrol is an **MCP stdio server**: `main = src/server.js`, `start =
node src/server.js`, deps `@modelcontextprotocol/sdk` + `chrome-remote-interface`.
Core already speaks stdio MCP (`wcore-mcp`, `docs/mcp.md`). So the integration
is configuration, not new code:

```toml
[mcp.servers.tvcontrol]
transport = "stdio"
command = "npx"
args = ["-y", "@ferroxlabs/tvcontrol"]
```

(or `command = "node"`, `args = ["<path>/src/server.js"]` for a checkout).

### What to actually verify — and the traps

1. **Drive it live.** Live testing outranks green code, and this is a customer
   demo. Configure it, start Core, confirm the 88 tools enumerate, then drive a
   real one against a running TradingView Desktop.
2. **CDP dependency.** `chrome-remote-interface` means tvcontrol talks to
   TradingView Desktop over the Chrome DevTools Protocol, so TradingView must be
   running WITH remote debugging enabled. This is the most likely live failure
   and it is environmental, not a code bug.
3. **WINDOWS `npx` RESOLUTION — read `docs/mcp.md` line ~98 first.** A stdio MCP
   server launched by bare `npx` on Windows needs PATHEXT shim resolution for
   the `.cmd` wrapper. This is one of the few sanctioned shell-string-mode
   carve-outs in AGENTS.md. Most masterclass customers will be on Windows, so
   this path has to be exercised on Windows, not just macOS/Linux.
4. **tvcontrol's own freshness.** CI is green on all six legs (ubuntu/macos/
   windows × Node 18/22) as of sha `710f6b06`, but **last push 2026-07-15 —
   three weeks stale**. It automates a third party that changes its UI without
   notice, so green CI does not mean it still drives TradingView today. Re-run
   its CI and do one live end-to-end before the masterclass.
5. Its `NOASSERTION` license on GitHub is a FALSE ALARM — standard MIT plus
   deliberate TradingView non-affiliation and trademark notices. Good hygiene.
6. Open PR #1 is a third-party badge from `mseep-ai`. Sean's call; a tracking
   badge on a customer-facing README is worth a deliberate decision.

**A Windows path bug of exactly our recurring class was fixed there on 07-15:**
`ENOENT ... scandir 'D:\D:\a\tvcontrol\...'` — a **doubled drive letter** from
joining an already-absolute Windows path onto a root. Harmless on POSIX. Expect
that family to recur.

---

## 6. TRAPS that cost real time — do not rediscover these

1. **`rtk` mangles output SILENTLY.** No error, just wrong data. It gave a `du`
   listing missing the four largest directories (I briefly believed 23.9 GB had
   been deleted), `wc -l` = 0 on a 171-line file, and a 0-byte `git show >` 
   redirect. **Use `rtk proxy` for anything whose exact value you will reason
   about or quote as evidence.**
2. **Check ALL in-flight CI runs before pushing, not the top rows.** I cancelled
   a run because `gh run list --limit 2` showed two completed non-CI runs while
   the CI run was still going. Use
   `gh run list --branch <b> --limit 10 --json status,name` and count everything
   not `completed`. A check that looks at the wrong rows is worse than none.
3. **Commit messages lie in both directions.** The #172 WIP undersold what
   worked and oversold what didn't. Read the code.
4. **A blocker list decays faster than the code.** Four of five named blockers
   were already fixed and never re-graded.
5. **An early-failing gate hides everything downstream.** One vacuity-gate
   failure produced three red steps and an error message pointing at a sandbox
   regression that did not exist.
6. **Kill every mutant.** Two proofs this session (§4.10 cancellation, and the
   cross-process POST count) were only trustworthy after being watched to fail.
