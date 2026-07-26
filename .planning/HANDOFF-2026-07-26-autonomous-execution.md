# HANDOFF — Wayland Core, autonomous execution begins (2026-07-26)

**Read this before touching anything.** It supersedes the operating sections of
`HANDOFF-2026-07-26-phase20-20A-complete.md`; that document's §7 environment traps remain
current and are still worth reading.

Working repo: `/Users/seandonahoe/dev/waylandcore-ferrox`, branch `plan/f20-unified-audit-repair`.
**NEVER touch `/Users/seandonahoe/dev/waylandcore`** — a different, heavily-dirty checkout.
Remote is `gh`, not `origin` (origin is a stale local worktree).

---

## 1. The three rules that govern this work

All three are in `AGENTS.md` §11 and were established by measurement, not preference.

1. **Live testing ranks at least as high as green code.** Exercise the real `wayland-core`
   binary and the real TUI. Phase 20A drove Windows and macOS acceptance to CI-green and nobody
   ever launched the binary; a later live pass found three HIGH defects in that same build.
2. **Lint plan gates to 0 HIGH before executing** — `python3 .planning/scripts/lint-plan-gates.py <dir>`.
   Three rounds of adversarial human review passed five phase plan sets that the linter then
   found 177 self-passing gates in, 68 in Phase 21 alone.
3. **Decide, do not park.** A checkpoint is an instruction to decide well, not to stop. All 18
   blocking gates are now cross-audited autonomous decisions.

Reserved to Sean, and nothing else: **main merge, PR, tag, release, GitHub issue closure,
deleting a retained evidence ref, real credentials/accounts.** Committing and pushing to this
working branch is expected.

---

## 2. Where the program stands

| Phase | State |
|---|---|
| **20** | COMPLETE — 8/8 requirements, seal `01a5b0ae`, Linux 11519/11519 |
| **20A** | COMPLETE — seal `9821ef76`, run `30184651330`, Win 6/6 + mac 8/8. **13/15** REQ-native (r12, r13 open) |
| **21** | **EXECUTING** — 4 plans, serial waves |
| 22, 23A, 23B, 24, 25, 26, 27 | Planned, gate-clean, autonomous. 32 plans total, **682 gates at 0 HIGH** |
| 28, 29, 30 | Not planned deliberately — they certify/package/score what 24–27 build |

**Execution order:** 21 → 22 → 23A → 23B serial (real content dependencies), then
**24 / 25 / 26 / 27 in bounded parallel** (all depend only on 23, on nothing from each other),
then 28 → 29 → 30. Nineteen plans declare SEAMs (`wcore-protocol` events, the contract manifest,
config schema) — serialize those; concurrent regeneration conflicts deterministically.

---

## 3. Config, and the two traps in it

`.planning/config.json` — verify any change with `git show HEAD:<path>`, never `git status`
(a commit silently no-op'd this way earlier and the fix never landed).

```
runtime: claude          use_worktrees: true       parallelization: 6-wide
executor: sonnet         verifier/plan-checker/code-reviewer/security-auditor: opus
security_block_on: high  auto_advance: true
test_command: git diff --check && cargo fmt --all -- --check
```

- **`parallelization.enabled` is a no-op while `use_worktrees` is false** — executors without
  `isolation="worktree"` run sequentially against the main tree (`execute-phase.md:337`). Both
  must be on.
- **`runtime: codex` and worktrees are mutually exclusive** — `execute-phase.md:304` fails
  closed. Runtime is now `claude`, which also matches where the agents actually resolve from.
- `worktree.baseRef: "head"` is set in `.claude/settings.local.json` (untracked) so the #683
  fork-base check does not silently degrade this non-`main` branch back to sequential.
- `test_command` was pinned to Phase 20's proof scripts and would have gated Phase 21 on the
  wrong suite. It is now phase-agnostic; each plan's own gates do the real proving.

**Ferrox CLI:** `node ~/.claude/ferrox-core/bin/ferrox-tools.cjs init execute-phase 21` — the
phase is a **positional**, not `--phase`. `--phase 21` silently returns `phase_found: false`.

---

## 4. The cross-audit panel — and the vote-loss traps

```
codex exec -m gpt-5.6-sol --sandbox read-only --skip-git-repo-check "<q>"
gemini -p "<q>" -m gemini-3.1-pro-preview -o text --skip-trust
/Users/seandonahoe/.kimi-code/bin/kimi -p "<q>" --output-format text
+ one internal adversarial pass arguing AGAINST the emerging consensus
```

All three probed live 2026-07-26. **Each of these silently drops a vote**, turning a "four-way"
audit into a three-way — the same defect class as a self-passing gate:

- **gemini requires `--skip-trust`** or it refuses and returns nothing.
- **kimi bullet-prefixes and indents**, so an anchored `^PANEL_POSITION=` regex drops its vote.
  Extraction must be unanchored.
- **codex repeats its final block** — take the LAST match.

A one-word probe passes despite all three. Probe with a real question.

---

## 5. Environment traps that each cost hours

- **Windows OpenSSH kills session children on disconnect.** Every long build started over ssh
  dies the instant the call returns — three agents died on this. Run builds as a scheduled task
  (`schtasks /create ... /ru SeanD` then `schtasks /run`), log to a file, poll for an `EXIT=` marker.
- **AppContainer cannot be observed over SSH.** A session-0 logon reports
  `is_available() == false` regardless of correctness, so sandbox reds from an SSH run are
  artifacts. Establish a control first.
- **hetzner-dsm has no cargo on a non-login shell's PATH** — use `/root/.cargo/bin/cargo`. A bare
  `cargo` exits 127; that is PATH, not a build failure.
- **Use a phase-dedicated worktree on hetzner** (`/root/wayland-p21`), never shared
  `/root/wayland` — concurrent detach on one tree corrupted a real proof run.
- **SSH→PowerShell quoting** breaks on naive nesting. Write a `.ps1`, pass it base64-as-UTF-16LE
  via `-EncodedCommand`. `#< CLIXML` on stderr is noise.
- Mac `grep`/`ls` are rtk-proxied and silently drop lines — use `/usr/bin/grep`.
- **CI now uploads per-target release binaries** (`d9c7683b`), including
  `wayland-core-aarch64-apple-darwin`. It was building and discarding them. "No macOS binary is
  obtainable" is no longer true and must not be used to justify closing a leg on Linux alone.
- `cancel-in-progress: false` protects a **started** run, not a **queued** one. Do not push while
  waiting on a pinned run.

---

## 6. Open items

**Escalations waiting on Sean (nothing else is blocked by them):**
- **23A macOS runner dispatch.** The choice is autonomous; dispatching the ephemeral self-hosted
  macOS runner is not, because its `f20-no-ambient-secrets` label is **known FALSE** — it runs as
  Sean's user with reach over the SSH dir, the AWS dir and an unlocked keychain.
- **25-01 cloud credential.** Backend choice is autonomous; the account is not.
- **24-04** is the only plan still `autonomous: false` — its terminal task performs publication.

**Known-red / non-gating:** `live_future_drop_reaps_descendant_job_tree`; two
`windows_private_dacl_*` unit tests; `multi_worker_output_exhaustion` (~35s vs 20s — timeout NOT
raised); bash under AppContainer (architectural). Under parallel load `admit_delegated_backend`
rejects with `sandbox backend fail_closed`; the proof passes because `--nocapture` forces serial.

**Live Windows UAT defects** (`.planning/phases/20A-native-windows-macos-uat/20A-LIVE-WINDOWS-UAT.md`):
D1 a refused tool call kills the session; D2 crash-interrupted sessions are permanently
unresumable — the error names a `reconcile` command that does not exist (routed to Phase 23B
Criterion 2, which owns those operator verbs); D3 `backend = "plaintext"` silently disables every
turn. Plus five MED/LOW.

**REQ-native r12, r13** remain open on Phase 20A. Neither gates a Success Criterion.

---

## 7. The finding that shapes Phase 21

**F21-02 currently holds VACUOUSLY.** Every production `sub_budget` caller passes `None`
(`spawner.rs:1176`/`:1200`, `engine.rs:6180`/`:6189`); only tests pass `Some(..)`. "A child
cannot widen its budget" is satisfied by the **absence of a request channel**, not by
enforcement — a test suite would prove it green forever. Hence the mandatory NO-CHANNEL canary
class: a case that fails if the property is only vacuously true.

Related, all to be re-verified rather than trusted:
- `intersect_execution_budget` is **not** the spawn primitive — its only caller is
  snapshot-restore (`execution.rs:301`). The spawn seam is `sub_budget(override_)`, where the
  override **replaces** child caps and parent caps bind by ancestor rollup.
- `policy_gate` is constructed `None` at both production constructors
  (`engine.rs:3147`, `:3381`) — fail-open.
- `wcore-egress` defaults to `AllowAllPolicy`; `EgressClient::new().with_policy(..)` bypasses the
  process-global `OnceLock`.
- `limit_for` renders the **leaf** state, reporting the child's wider limit; `minimum_remaining`
  is the correct pattern.
- The two Windows runners are **one physical box**. Concurrent compile load is a proof hazard,
  not capacity.

---

## 8. Resuming

```bash
cd /Users/seandonahoe/dev/waylandcore-ferrox
git log --oneline -15
python3 .planning/scripts/lint-plan-gates.py .planning/phases/2[1-7]*   # expect 0 HIGH
ls .planning/phases/21-child-authority-and-budget-inheritance/*-SUMMARY.md
```

A plan is done when its `NN-SUMMARY.md` exists. Re-read the SUMMARY before redoing anything —
several agents died mid-write today on transport errors, and partial state on disk is the norm
rather than the exception. **Verify what landed; do not assume.**
