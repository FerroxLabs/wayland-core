# lane/28-adj2 — running notes (re-committed after every measurement)

Branch `lane/28-adj2` off integration HEAD `1b9f148f`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-28-adj2`.
Windows: `SeanD@seandesktop`, worktree `C:\f28h2-repo`, target `C:\f28h2-target`
(reused from lane/28-h2; this code is `#[cfg(windows)]` and cannot build on hetzner).

## Assignments

- **F-28-ADJ-001** (MEDIUM) — `reclamation_reports_grants_it_could_not_revoke` never calls
  `reclamation_report`; it asserts only that the quarantined file still contains the grant
  path, which the MOVE guarantees regardless. Adjudicator mutant M3 (delete the disclosure
  branch) → 133 passed / 0 failed / 23 ignored, identical to pristine. Make the test assert
  what its name promises; re-run M3 to prove it now fails. Do NOT duplicate
  `a_leaked_test_lease_is_diagnosed_by_name`, which already pins the message clause.
- **F-28-ADJ-002** (MEDIUM) — static reading, NOT reproduced: a crash between
  `create_new_nofollow` and `write_and_sync` leaves a 0-byte `.toml`; `read_validated_lease`
  rejects it, aborting the whole recovery pass forever = the same wedge by another door.
  **Reproduce FIRST.** If it does not reproduce, say so and stop. If it does, reuse the
  existing quarantine path — do not invent a second recovery concept.

## Standing discipline

- Every checker gets a known-positive AND a known-negative before it is trusted.
- Assume a fifth self-passing gate exists. Four are on record:
  (1) `--exact` filter matching no test name; (2) stale binary via `Copy-Item` mtime
  preservation; (3) nested child test-process summary spliced into the parent stream, first
  regex match wins; (4) M3 — a test that never calls the function it is named for.
  Adjudicator's own instrument carried a fifth shape: `--list` regex anchoring `$` against
  trailing CRs.
- Grade on markers in a status file (`WLRC=`/`WLDONE`), never an exit code across ssh.
- Assert executed counts; never trust `test result: ok`.
- Take the LAST `test result:` match — nested helpers splice their own summaries in.
- Force rebuilds: stamp `LastWriteTime` after `Copy-Item` AND assert `Compiling wcore-sandbox`.
- Only `--lib` under `WAYLAND_SANDBOX_LIVE_WINDOWS=1`; integration tests under `tests/` lease
  into the REAL `%LOCALAPPDATA%` (finding F-4).
- Never merge, no PR, no `wcore-contract generate`. Never `git add -A`.

## Log

- `[t0]` Branch + worktree created off `1b9f148f`. Confirmed `166ce7fe` (lane/28-h2) is an
  ancestor of integration HEAD, so the F-28-02-002 fix is present in this tree.
- `[t0]` NOTES committed before any analysis.
