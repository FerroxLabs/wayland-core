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
- `[t1]` **ADJ-001 fixed** (`8a870b9a`). Root cause confirmed exactly as filed: the test read the
  quarantined TOML, which the MOVE preserves regardless of the report. Rewrote it to assert the
  EMITTED report, both directions. Added a `cfg(test)` recorder rather than testing the pure
  function alone — the pure function alone would still pass an implementation that logged a
  constant. M3 re-run pending a Windows build.
- `[t2]` **ADJ-002 REPRODUCED** on real hardware, base `1b9f148f`, binary `c732584c…`,
  `SRC_DIRTY=0` (`adj2repro-base.log`). Clean → `ran=True`. 0-byte `.toml` → `ran=False`,
  backend degrades to `fail_closed`, and the predicted diagnostic appears verbatim:
  `invalid AppContainer ACL lease size 0 in \\?\C:\…\WCore-adj2-….toml`. Second run identical →
  **permanent**. Effect and mechanism both match the static reading. Proceeding to fix.

### FIFTH self-passing gate — mine, again, and the same shape I already recorded once

`adj2-repro.ps1` reported `size_error=False` while the raw log contains the string. Cause: the
console wraps long lines, splitting `invalid \n AppContainer ACL lease size` across a newline, so
a literal `-match` of the phrase fails. **This is the same defect I noted in lane 28-h2 as an
under-detecting `reclaimed=False` marker and did NOT fix in the harness — so it recurred.**
An under-detecting classifier is the dangerous direction: it silently reports the defect ABSENT.

Repair: whitespace-normalise the captured text before matching, and self-test the classifier
against a known-positive and a known-negative in the same run. Marker lines from
`adj2repro-base.log` are NOT to be trusted for the `size_error` field; the raw text is the
authority for that observation.

- `[t3]` **ADJ-001 closed.** M3 now FAILS the disclosure test (was byte-identical green). M4 added
  and also fails, proving the test reads the production emit path, not just the pure function.
- `[t4]` **ADJ-002 fixed and live-proven** at `9c4d2612` (binary `b3b235fc`): 0-byte lease reclaimed
  in-flight, execution RUNS, second pass silent. M5/M6 confirm the fix and its guard rail are both
  load-bearing. Suite 136/0/23.
- `[t5]` Classifier repaired and self-tested (`known_positive`, `known_negative`,
  `old_matcher_missed_it` all True). Base log re-classified: the wedge string appears 4x, so the
  original `size_error=False` was purely instrument error.
- `[t6]` Clippy 4 warnings (= base, zero new). Live acceptance 20/3, the 3 pre-existing bwrap reds.
  Box restored: 0 active leases, quarantine removed, backups intact.
- `[t7]` SUMMARY written. Lane complete.
