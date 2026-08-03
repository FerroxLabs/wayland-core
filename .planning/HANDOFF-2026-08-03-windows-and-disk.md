# HANDOFF — wayland-core — 2026-08-03 (Windows merges + disk)

Successor to `HANDOFF-2026-08-03.md` (keep both; that one covers the earlier RC work).

**Integration: `c615daed`** on `plan/f20-unified-audit-repair`.
Setup: `export WL_LANE=core`; `gh auth switch --user FerroxLabs` before EVERY gh op.

---

## 0. RESOLVED — the shallow-orphan problem is CLOSED (2026-08-03, later)

**`git fetch --unshallow gh` worked.** Everything the previous version of this
section said about patch recovery is obsolete; that path was never needed.

**Two claims in the previous version were WRONG. Do not carry them forward:**

1. **Only ONE lane was ever an orphan.** `.git/shallow` held a single entry
   (`11e2728f` = `win-engine-prove`). The other three had full ancestry the
   whole time — `win-hidden-fails` 3580 commits, `win-typed-bypass` 3570,
   `win-mutation-lock` 3571, every one with `merge-base = cfb3e19c`. "All four
   are shallow orphans" was one failure generalised to four branches without
   checking the other three.
2. **The 273 worktree registrations are NOT "mostly dangling."**
   `git worktree prune` removed exactly zero — they all point at live
   directories, and many hold UNPUSHED branches. `waylandcore-frontier-worktrees`
   (23.9 GB, the largest remaining item in ~/dev) therefore CANNOT be deleted
   without losing work. Audit branch-by-branch before ever proposing it again.

**ALL FOUR LANES MERGED.** Integration `c615daed` -> `8b35b12c`.
`win-mutation-lock` stays held, and that hold is still correct.

| merge | commit | note |
|---|---|---|
| win-engine-prove | `f11ad2f5` | kept the `run_grep` pre-check and added the test that kills its surviving `grep-target-exists` mutant |
| win-hidden-fails | `617a0d23` | conflict taken to engine-prove's helper; two dead fns removed after verifying the defect they targeted is closed elsewhere |
| win-typed-bypass | `5b05f7a1` | diag workflow + diag test STRIPPED — the file contains ZERO assertions |
| win-formatter | `15b9b7af` | the fix stranded by the mutation-lock hold |

**Merge gate on hetzner: GREEN.** fmt clean, `--locked` clean, clippy 0
diagnostics, **positive control 101** (proves the gate can fail), identifier
gate 0, **nextest 13919/13919** — up from 13873; the wave added 46 tests.

**One test was retry-masked**, now task #174:
`wcore-eval-scenarios::runner_contracts normal_exit_reaps_owned_descendant_listener`
failed TRY 1 and passed on retry. NOT caused by this wave (it touches no file in
that crate), but `2b662fe8 fix(sandbox): own and reap process trees` is recent
and this test is precisely about reaping an owned descendant. Correlate before
assuming coincidence. `scripts/flake-ledger.py` just landed for exactly this.

**A CI gate was red on integration and nobody had noticed** —
`check-no-vacuous-cargo-test.py` failed on two lines of
`nightly-windows-soak.yml`. Both were false positives, but the cascade cost the
whole job: that gate runs BEFORE "Build CI image", so its exit 1 meant the image
never built and every later step died on `pull access denied for
wayland-core-ci` — including the swarm delegated-dispatch step, whose error text
("either a real regression or bwrap has stopped qualifying") sends you hunting a
sandbox regression that does not exist. One cause, three red steps, one very
misleading message. Fixed in `8b35b12c` by adding the `vacuity-checked:`
annotations the gate documents, NOT by rewording code to dodge the matcher.

**The handoff was previously committed to `rc-handoff` only, never to
integration** — a successor reading the integration branch would not have found
it. This version lives on integration.

---

## 1. Windows lanes — dispositions (from the wave-2 cross-audit, still valid)

| lane | verdict |
|---|---|
| `win-engine-prove` `11e2728f` | **MERGE FIRST.** The only lane that repairs product behaviour *and* measures it: a baseline branch (integration + one workflow, **zero source bytes**) measured 2243 pass / 8 fail on real Windows; the lane 2247 / 4 on the identical command. 7 of 8 mutants killed. Root repair: `RealFs::observe_file` was `cfg(unix)`-only, so durable receipts and crash reconciliation were **inert on Windows**. Its one hole is admitted: the `grep-target-exists` mutant **survived**, and it is the only change in the lane that alters Linux/macOS too — decide at merge whether to pin a behaviour the `run_grep` `try_exists` pre-check uniquely produces, or drop that pre-check as superseded. Delete `.github/workflows/win-engine-prove.yml` on merge. |
| `win-hidden-fails` `4438b188` | **MERGE SECOND, resolve the conflict deliberately.** Both lanes fix the same Windows `is_absolute` fixture defect in `crates/wcore-agent/src/child_transaction/gate_executor.rs`. **Take engine-prove's version** — it is the one measured red-before/green-after on Windows for that exact test. Also delete the orphan `crates/wcore-agent/tests/common/mod.rs::bootstrap_workspace` (38 lines, zero callers; invisible to clippy only because the file carries `#![allow(dead_code)]`). The rest is measurement tooling, no production code. |
| `win-typed-bypass` `58a07755` | **MERGE THIRD, STRIPPED.** Delete `.github/workflows/win-typed-bypass-diag.yml` and `crates/wcore-agent/tests/win_bash_path_diag.rs` first — those diagnostics **print a verdict and return**, so promoting them creates a permanently-green gate. Verified no product change: the only `src/` files touched are `voice_mode.rs` and `wcore-egress/src/lib.rs`, both entirely inside `#[cfg(test)]`. What lands is a widened egress refusal budget (1s→15s connect, 2s→30s total) for Windows' ~2s `WSAECONNREFUSED` — correct, ~28s worst-case slowdown everywhere. |
| `win-mutation-lock` `30f28ff4` | **HELD, and the hold is correct.** Zero Windows execution at any tip; every changed line is `cfg(windows)` and has never run. Two harness defects it did not admit: PHASE 1 BEFORE is `continue-on-error: true` and asserts nothing about the red count — **the job can go green having proven nothing about RED-before**; and `runs-on: [self-hosted, Windows, X64, msvc]` does **not** select SEANDESKTOP, because `ferrox-win-msvc` carries an identical label set. Unremarked behaviour change: slicing the wait into a 1s poll loop makes a Win32 mutex waiter surrender its queue position ~15× per former budget — fairness under contention is precisely its target regime. **Stranded by the hold:** the two-line Windows formatter dialect fix in `crates/wcore-cli/tests/tool_formatter_real_payloads.rs`, from `a7d50967` on `gh/lane/win-formatter`. Cherry-pick separately. |

**WHERE WINDOWS EVIDENCE COMES FROM — do not confuse these.** Hosted
`windows-latest`, **not** SeanDesktop. Both `ferrox-win-msvc` and `SEANDESKTOP`
report `online, busy=true` while serving **zero** jobs (#164/#138, Sean-only). The
merge gate runs on **hetzner** and proves Linux + clippy + fmt + `--locked` only —
it **cannot** compile `cfg(windows)`. Never present a hetzner-green as Windows
evidence.

---

## 2. The merge gate (hardened this session)

`/root/gate.sh <ref>` on hetzner, isolated in `/root/orch-gate`:

- `reset --hard` + `clean` **before and after** checkout. A dirty `Cargo.lock` once
  made checkout abort and **the gate silently did not run**, reporting nothing.
- `fmt --check` | `metadata --locked` | `clippy -D warnings`
- **CLIPPY POSITIVE CONTROL:** `clippy -p wcore-agent --lib -D warnings -D clippy::pedantic`
  must be **nonzero**. The previous known-bad control silently went green when
  somebody fixed it, leaving six clean clippy runs unproven as *able to fail*.
  Pedantic cannot go green by accident.
- `python3 scripts/check-no-personal-identifiers.py`
- `cargo nextest run --workspace --profile ci --no-fail-fast`
- prints `GATE_HEAD_AFTER` + dirty count

Last full green: **13,873 / 13,873**, clippy 0, control 101, identifier gate 31/31.

**Check `df -h /` on hetzner FIRST every session.** At ~92% admission control
refuses and it presents as test failure. `/root/lanes/*` for merged lanes is
reclaimable (184 GB reclaimed this session).

---

## 3. Disk — DONE

**Mando move COMPLETE: 17 of 17 directories moved, 0 kept, 0 failures.**
Every one passed both checks — exact file-count match AND a dry-run rsync with
nothing left to transfer — before its source was removed. Largest: `resources`
163,353 files, `ferroxfactory` 192,028, `ai-foundry` 96,667.

Also done: `~/.cargo/registry` deleted (2.5 GB; cargo is forbidden on this Mac
anyway, so it was pure waste) and 18 GB of stale agent worktrees cleared.

**Mac: 63 GB free -> 115 GB free.** Mando has 2.5 TB free.

The safety contract, if this is ever rewritten: **never `mv` across volumes** —
an interrupted `mv` loses the file. `rsync -a`, then two INDEPENDENT checks
before deleting the source. Worst case is a duplicate on Mando, never a hole on
the Mac.

**NOT reclaimable, contrary to the earlier handoff:**
`waylandcore-frontier-worktrees` (23.9 GB) holds many UNPUSHED branches —
`lane/macos-journal`, `feat/d1-budget-contract`, `feat/f17-closure`,
`feat/f18-*`, `feat/f19-*`, `fix/f20-exec-fixture-race`, `integrate/f20-gsd-*`,
`repair/f20-*` and more, plus ~16 worktrees with uncommitted changes. Deleting
it would destroy work. `wayland-worktrees` (14 GB) is Sean's Desktop repo —
not this lane's to touch.

## 4. OPEN — ranked

**RELEASE BLOCKERS**
- **#170 P0 privacy:** `memory.enabled = false` does **not** stop memory recording.
  `want_memory = config.memory.enabled || skills_lifecycle_enabled`, and
  skills_lifecycle **defaults ON**. Both-directions bar: opt-out must record
  nothing, *and* a default install must still record.
- **#162 / #163** Windows stdout + `read_only` vacuity. Root causes fixed and
  merged (CRLF in `collapse_cr_lines`; shell-string quoting). Open until the
  Windows lanes land and re-prove them.

**HIGH**
- **#172 refresh single-flight.** Plan v3 at
  `.planning/PLAN-172-refresh-single-flight.md`, four-leg cross-audited (Gemini
  REJECT; Kimi + Codex SOUND-WITH-FIXES; internal). Lane `refresh-single-flight`
  stalled on a watchdog at `93432b1f`; work is pushed. **Failure policy reversed
  twice** — final: on lock timeout reload and succeed if a new pair appeared,
  otherwise **fail retryably, never POST unlocked**, because replaying a
  single-use refresh token can make a compliant provider revoke the whole grant.
- **#165** Desktop follow-up: `ready.session_id` is now JSON `null`; Desktop types
  it `string | undefined` and assigns directly. Must reach Sean **before** the
  Desktop branch merges.
- **#173** A TUI operator on a keyless host is never told crash replay is off.
- **#164 / #138** Both self-hosted Windows runners advertise busy, serve zero jobs.
  **Sean-only** — his machines.

**MEDIUM:** #166, #167, #168 (a test mutates a committed evidence file — the gate
now reports `dirty=1` after every run), #169, #149, #155, #156.

---

## 5. LESSONS — why this record is worth keeping

1. **Five lanes refused a stale premise and were right every time.** Task
   descriptions written days earlier were wrong about the code. Treat every open
   task as a hypothesis, not a fact.
2. **A green test can pin a defect in place.** `events.rs:1684` asserted
   `session_id.is_none()` — the missing wire key was *codified as expected*.
3. **The worst defect was found by a differential ladder, not a suite.** L0–L5
   green, L6 red, L7 proved the child ran and its output was lost.
4. **Gates that cannot fail keep reappearing:** a stale clippy control, a
   `continue-on-error` RED-before phase, permanently-green diag workflows, and a
   gate script that aborted and reported nothing.
5. **A merge gate passing 13,845 tests proved nothing about a credential race.**
   Only an adversarial verifier running a scheduled race found it. Thorough and
   honest ≠ verified.
6. **`rtk` summarises and hides** — `rtk proxy` when exactness matters. Piping into
   `powershell`, or leaving stdin open for `codex exec`, truncates or hangs.
