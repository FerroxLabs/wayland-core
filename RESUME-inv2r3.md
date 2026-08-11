# INV-2 round 3 — COMPLETE

Branch `fix/inv2-round3` @ **0f4f3549**, off round 2 `e1cb08ed`. Worktree
`/root/wt-inv2r2`, tree clean. Release artifact
**5d56c1a0d6da327d16a59d25ecfef5ebf899a8d7cf5a94b1e8219cc39b4d590e**
built from that exact clean tree.

5 commits: 3d43e680 (fix) / 75153111 (matrix) / 6de7e689 (pin test) /
16eefdab (docs) / 0f4f3549 (lock).

## The three breaks — all closed, all mutation-verified

**B1 fail-open.** Measured root cause on git 2.43.0 (Linux) and 2.54.0 (Windows):
`rev-parse --is-inside-work-tree` exits **128 for a missing repository AND for
dubious ownership AND for a corrupt `.git/config`** — indistinguishable by exit
code, and round 2 mapped the whole bucket to `Ok(None)` -> `NoRepo`.
Fix: answer "is there a repository" from the **filesystem** (`.git` on any
ancestor, plus `GIT_DIR`/`GIT_WORK_TREE`, erring to "present" on any
non-NotFound error), which holds with no git binary at all. Classify git by
exit code: `rev-parse --verify --quiet HEAD` gives **1** for unborn HEAD vs
**128** for fatal; `ls-tree` gives **exit 0 + empty** for "not in that commit"
so an absent path never needs a 128 (`git show <c>:<p>` exits 128 for both —
the same conflation). A repo git will not open is `Unknown` -> **fail closed**.
Corrupt index measured a **non-event** on both platforms (rev-parse and ls-tree
never read the index) — asserted so a future change that starts reading it is
caught here.

**B2 latch.** `baseline_for_dir` returns early WITHOUT caching on `Unknown`.
Proven both directions on one guard instance: break git -> Refuse("git did not
answer"), repair git -> Proceed.

**B3 false snapshot.** The `if target.exists()` short-circuit is gone with the
whole file store. `recoverable_copy` cannot return Ok without having written
the object AND read it back with `git cat-file blob` AND compared bytes.

## Store: REPLACED (profile-home store deleted entirely)

Copies go to the repository's own object DB via `git hash-object -w --stdin`.
Kills all four adversary findings at once instead of hardening a new surface.
Live-proven (`/root/inv2-live-r3/armB`): object is a **blob unreachable from
any ref** (`git rev-list --objects --all | grep -c <oid>` = 0), so `git gc`
prunes it — GC measured, not asserted.
Cost, taken deliberately: **no repository -> no object store -> a Write that
drops lines is REFUSED** (round 2 allowed it against the plaintext copy; round
1 also refused). Edit there proceeds saying "not recoverable" rather than
lying.

## Evidence

- **Mutations 23/23** clean run, `/root/inv2r3-mutate-clean.log`, harness
  `/root/inv2r3-mutate.py`. PASS now requires `rec_m and rec_r` (round 2
  printed the recompile flags and never gated on them). Includes the two
  anchors round 2 lacked: B1a-d (exit-code / filesystem-marker classification)
  and B3a-d (the read-back compare that replaced `exists()`).
  M1a/M1b initially SURVIVED — the blob cache answered before the mutated
  `ls-tree`, so the pin was proven by the cache not by itself. Closed with two
  tests that pin on one file and judge a different one.
- **Hostile matrix** `tests/unsaved_work_hostile_git_test.rs` (7 arms) +
  `tests/unsaved_work_no_git_test.rs` (own binary, mutates PATH). Linux 7/7,
  Windows 6/6 (dubious-ownership is `#[cfg(unix)]`).
- **Windows** (SeanD@seandesktop, D: only, 0 wayland-core processes, restored
  clean): 40 lib + 6 matrix + 1 no-git + 17 integration = **64 green**.
  `icacls` reproduced the finding: `%USERPROFILE%\.wayland` grants
  `CodexSandboxUsers:(OI)(CI)(RX)` + two AppContainer package SIDs, and a file
  created there inherits `(I)(RX)` for all three. The round-3 recovery object's
  ACL is **byte-identical to an ordinary tracked file in the same repo** and
  the sandbox principal is **not** on it.
- **Artifact**: Windows content digest matched Linux at all 6857 paths
  (mode-insensitive; 125 files are 100755 + 1 symlink so raw tree hashes cannot
  match). Binary grep controls positive AND negative, with the greps shown to
  discriminate against round-2 source.
- **Live**, against binary 5d56c1a0, `/root/inv2-live-r3/`: armB task completed,
  2/2 user lines recovered byte-exact, secret hits in profile home = 0, no
  `unsaved-work` dir. armD (mid-session commit) = 1 refusal, TODO survived.
  armC = the model kept the line unprompted, so no refusal fired — invariant
  held but by cooperation, weak evidence; that shape is covered deterministically.

## Known-open (declare, do not hide)
- **Bash uncovered** — now stated in the tool description and docs/tools.md.
- **modified vs dropped still not distinguished**; the rename case still
  refuses. Only the wrong instruction was removed ("reproduce those lines" ->
  "carry those lines ... in their changed form").
- Over-refusal shapes unchanged: 1 recorded + 500 unrecorded, shebang-only.
- `git_run` uses sync `std::process::Command` in argv mode, not
  `wcore_config::shell::shell_command_argv` (that is tokio; `assess` is sync and
  runs during registry construction). Injection property identical.
- No timeout on git invocations; mitigated by `GIT_TERMINAL_PROMPT=0`,
  `GIT_OPTIONAL_LOCKS=0`, `core.fsmonitor=false`, stdin null.
- Round 2's "all 5 original tests kept" was inaccurate (4 verbatim; the
  outside-a-repo one was renamed and changed). Round 3 changes it again — it
  now refuses.
