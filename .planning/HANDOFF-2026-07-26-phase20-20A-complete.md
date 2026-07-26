# HANDOFF — 2026-07-26 — Phase 20 + Phase 20A COMPLETE

**Branch:** `plan/f20-unified-audit-repair` @ `8a0c525a` (source seal `9821ef76`)
**Repo:** `/Users/seandonahoe/dev/waylandcore-ferrox` — the ONLY working checkout.
**Never touch** `/Users/seandonahoe/dev/waylandcore` (heavily dirty).

---

## 1. STATUS: BOTH PHASES DONE

| Phase | State | Seal |
|---|---|---|
| **20** Transactional Delegated Mutation | **COMPLETE** — 8/8 requirements | `01a5b0ae` |
| **20A** Native Windows/macOS UAT | **COMPLETE** — all 3 Success Criteria | `9821ef76` |

**Native acceptance, dispatched run `30184651330` = success:**
```
F20_NATIVE_WINDOWS_ACCEPTANCE=PASS  commit=9821ef76 tree=0a1267a9 nonce=96c91107
F20_NATIVE_MACOS_ACCEPTANCE=PASS    commit=9821ef76 tree=0a1267a9 nonce=96c91107
```
Windows 6/6 targets, macOS 8/8 targets, same commit+tree+nonce on both. Linux aggregate `11520/11520`.

Seal is durable via annotated tag **`f20a-candidate-9821ef76`** (the local `refs/f20a/candidate` is NOT on the remote — the tag is the durable ref).

### 20A requirements: 11 complete, 4 deliberately OPEN
- **r2** — `deny_ace_still_blocks_granted_read`, `normal_sid_only_grant_is_denied` never ran; their only runner is the `windows-live-acceptance` job, gated OFF in candidate mode (`nightly-windows-soak.yml:239`). ~15 min wiring fix.
- **r8** — anti-drift guard admits correct targets, but *rejection* of a wrong-OS mapping was never demonstrated. One test.
- **r12 / r13** — need a fresh review round at this SHA (bounded to 2 rounds by the amended rule).

None blocks the Success Criteria. Recorded in `REQUIREMENTS.md`, `ROADMAP.md`, `20A-04-SUMMARY.md` §13.

---

## 2. THE RULE CHANGE THAT ENDED THE 74-PLAN LOOP (do not revert)

Commit `d0837aa7`. Phase 20 metastasised to 74 plans because two rules had no fixed point. Now:
- Findings at **CRITICAL or HIGH** must be fixed or disproved. **MEDIUM and below → BACKLOG, non-blocking.**
- Execution begins at **zero CRITICAL/HIGH, or after 2 review rounds, whichever first. A third round escalates to Sean.**

Config enforces it: `workflow.security_block_on=high` (was `low` — a LOW finding used to block advancement), `workflow.auto_advance=true`, `granularity=standard`, `inline_plan_threshold=3`, `discuss_mode=discuss`. Set in all three `.planning/config.json` files. **These are tracked-but-uncommitted — commit them or a `git checkout` restores the loop.**

**Proof it works: Phase 20 = 74 plans. Phase 20A = 4 plans.** Keep the 4-plan cap and the plan-checker (it caught 2 CRITICALs in 20A that would have made SC1 unreachable, for ~3 min).

Phase 20 was reconciled 74 → 18 live plans; 56 archived (never deleted) under `phases/20-…/archive/{native-split,superseded}/`. Ground truth: `phases/20-…/RECONCILIATION.md`.

---

## 3. ENVIRONMENT TRAPS — these cost hours; read before any work

1. **`origin` in the Mac repo is a STALE LOCAL WORKTREE.** The real remote is **`gh`**. A `git reset --hard FETCH_HEAD` against `origin` moved HEAD onto an unrelated commit (recovered via reflog). On the remote HOSTS `origin` IS correct.
2. **Mac `grep` is rtk-proxied and SILENTLY DROPS LINES** (measured 32 vs 674 on one file). Always `/usr/bin/grep`, `-F` for literals. Same for `ls` (appends sizes).
3. **`cmd`: `set VAR=x && ...` appends a TRAILING SPACE**; Rust's parse fails and silently falls back to the default. Use `set "VAR=x"` or `$env:VAR='x'` and PROVE it took effect. This produced a false "RUST_MIN_STACK doesn't help" conclusion.
4. **PowerShell 5.1 parses `0x80000000` as negative Int32** → `CreateFileW` access args fail UInt32 conversion, handles never open, and probes "succeed" holding nothing. Prefer Rust probes compiled into the crate.
5. **Both hosts' fetch refspecs are pinned to an unrelated branch** — `git fetch --all` silently misses this branch. Always `git fetch origin plan/f20-unified-audit-repair`.
6. **`cargo fmt --all` fails on Windows** (os error 206). `justfile:96-98` already skips it there — fmt on the Mac.
7. **This Mac CAN compile the workspace.** The old "never compiles on Mac" note is a workflow convention, not a fact. It is also now a registered CI runner.
8. **Windows CI runs clippy `-D warnings` BEFORE tests** (`ci.yml:145` → `justfile:75`). Any lint failure means tests never execute — this hid 34 `session_journal` failures for weeks.
9. **The proof's green depends on `--nocapture` forcing SERIAL execution.** Under parallel load `admit_delegated_backend` rejects with `sandbox backend fail_closed`. Fail-closed so safe, but concurrent dispatch degrades under load.

### macOS runner (this Mac)
- `~/actions-runner-macos/`, **ephemeral** — consumes itself per job, needs re-register each dispatch.
- Labels: `self-hosted,macOS,ARM64,f20-native-macos,f20-ephemeral,f20-no-ambient-secrets,f20-image-1d05364078523334605249687228ffec79964b7ecf731d7c9512b40e67fd1a64`
- **Keychain fix (durable):** `runsvc.sh` exports `DOCKER_CONFIG=~/.docker-runner-noauth` + `DOCKER_HOST`; that config has `{"credsStore":"none"}`; `~/bin/docker-credential-none` is a no-op helper; `~/bin` prepended to `.path`. Sean's own `~/.docker/config.json` is RESTORED to `credsStore: desktop`.
- **`f20-no-ambient-secrets` is a FALSE label** — the runner runs as Sean's user with reach over `~/.ssh`, `~/.aws`, unlocked keychain. Real fix: a dedicated macOS runner account.

---

## 4. KNOWN-RED, NON-GATING (do not rediscover)

- `wcore-sandbox::live_integrity::live_future_drop_reaps_descendant_job_tree` — Windows, deterministic. Escalated: every candidate fix changes what the sandbox permits.
- `snapshot.rs` `windows_private_dacl_accepts_restrictive_deny_ace` / `..._rejects_null_empty_and_broad_allow` — WRITE_DAC reopen error 5; fails identically at parent; unit tests not reached by the four suites.
- `worker_runtime_limits::multi_worker_output_exhaustion_fails_without_retaining_buffers` — ~35s vs a 20s budget. **Timeout deliberately NOT raised.**
- **bash cannot run under AppContainer at all** — msys needs `\BaseNamedObjects`; AppContainer confines to `AppContainerNamedObjects` by construction (`0xC0000022`). Not an ACL gap — every file on the load chain already grants ALL APPLICATION PACKAGES. The test now asserts the real fail-closed contract.

### Follow-ups for the NEXT candidate
- `scripts/f20-native-macos-proof.sh:134` pulls `alpine:3.19` **unconditionally** — should `docker image inspect` first. This caused 4 failed macOS dispatches.
- Dedicated macOS runner account (see false label above).

---

## 5. WINDOWS DEFECT CLASSES FOUND AND FIXED (this knowledge transfers to phases 21-30)

1. **Path representation** — `std::fs::canonicalize` returns verbatim `\\?\`; git-for-Windows/MSYS and PowerShell `[IO.DriveInfo]` cannot parse it. Helper: `normalized_root` in `crates/wcore-swarm/src/worktree_paths.rs`. `dunce::simplified` is **CONDITIONAL** (no-ops on >255-char components, non-UTF-8, reserved DOS names, trailing dot/space) — so normalise at the **comparison** boundary, both operands, not only at storage. **Exception:** `worktree_manager.rs` `git_common_dir` is DELIBERATELY excluded (spawner compares it without re-canonicalising; normalising creates a fail-open).
2. **Handle semantics** — `LockFileEx` invalid on a DIRECTORY handle (err 87); a `DELETE`-bearing handle blocks `SetCurrentDirectory` into that dir; `SetFileInformationByHandle`+`FileRenameInfo` rejects a HANDLE in `RootDirectory` (use `NtSetInformationFile`); `MoveFileExW(REPLACE_EXISTING)` fails on an open destination.
3. **Mandatory vs advisory locks** — Windows byte-range locks are mandatory; a whole-file lock breaks the crate's own readers. Use a one-byte sentinel.
4. **`#[cfg(unix)]` blindness** — gates that hide Windows behaviour so a path is never exercised. This is how a dead rename primitive stayed invisible for months.
5. **Test-suite health** — see `.planning/TEST-AUDIT.md`: 283 tests had no execution evidence, ~145 ran in no workflow at all. The problem was never "too many tests"; it was tests that never run or pass for the wrong reason.

---

## 6. NEXT ACTIONS

1. **D1 admission control** must pass before Phase 21 broad execution (ROADMAP execution rules): `intel/COMPETITIVE-LEDGER.md`, `intel/FIELD-REGRESSIONS.md`, `intel/DESKTOP-PROTOCOL-CHECKPOINT.md`.
2. **Phase 21** → 22 → 23 **serial** (real dependencies). Then **24–27 in bounded parallel worktrees** (already sanctioned).
3. Optionally sweep the 4 open r-requirements (r2 and r8 are ~15 min each).
4. Commit the three `.planning/config.json` files so the rule fix is durable.

### SPEED — measured bottlenecks, in order
1. **Serial dispatch of independent work** — biggest loss. Use the **Workflow tool** (`pipeline()`, no barriers) instead of manual serial subagent dispatch.
2. **One shared Windows box.** `ferrox-win-msvc` is a SECOND registered Windows runner and was idle the whole time. Use both, with a **separate worktree per agent** — concurrent compile load corrupted one proof run.
3. **Rediscovery** — this file exists so agents read the traps once instead of re-deriving them.
4. Realistic ceiling ~4x. Wall clock is dominated by compile+test on real hardware.

**Do NOT:** create new Phase-20 plans, resume the archived native chain plan-by-plan, or revert the amended termination rules.
