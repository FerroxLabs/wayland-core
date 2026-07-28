# 28-ADJ NOTES — independent adjudication of F-28-02-002

Lane: `lane/28-adj`. Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-28-adj`.
Base: b79f141e (plan/f20-unified-audit-repair tip at branch time).

## Question
Does the FIXED claim for F-28-02-002 (stale AppContainer lease wedge, HIGH, persistent DoS)
survive an adversarial independent pass? Only FIXED or DISPROVED are available dispositions.

## Status log
- [t0] Worktree created. Read LANE-BRIEF, 28-H2-SUMMARY.
- 28-H2-SUMMARY claims: repro on real Windows hw at 12fc794f; repair; both legs re-measured at
  3f3f93dc; 133 passed/0 failed/23 ignored unit; M1/M2 mutants each kill exactly one test;
  live acceptance 20 passed 3 failed (3 pre-existing bwrap-on-Windows, identical at base).
- 28-H2-SUMMARY §8 explicitly says fix is on lane/28-h2 ONLY, not merged. Brief to me says
  "the repair merged". MUST VERIFY whether source actually landed on the integration branch,
  or whether only the docs commit (166ce7fe) landed. If only docs landed, FIXED cannot be
  written into the ledger — that would be a paper disposition.

## Open attack lines
1. Is the repair source actually present on plan/f20-unified-audit-repair?
2. Does the quarantine allow-list create a new wedge / writable surface / trust crossing?
3. Is the honour-when-alive leg real (not reclaim-everything)?
4. Fourth self-passing gate the lane did not catch (assume it exists).
5. Is the KR-05 non-closure scope statement accurate?
6. Gate self-test at 28-04-FINDING-LEDGER.md:1182 expects exactly one F28L-002 on F-28-02-002.
   Moving the row without moving the expectation breaks or vacates the gate.

## Measured so far (adjudicator, independent)

### A1. Is the repair actually merged? YES.
`git merge-base --is-ancestor` -> both 15821c03 (source) and 3f3f93dc (report extraction)
are ANCESTORS of lane-28-adj HEAD (b79f141e, tip of plan/f20-unified-audit-repair).
166ce7fe is docs-only (20 files, all under .planning/). The SOURCE landed separately.
So the ledger row would not be a paper disposition. 28-H2-SUMMARY §8's "fix is on
lane/28-h2 only" was true when written and is now STALE — the lane branch was merged after.

### A2. Allow-list scope — NARROW, fails closed on every adjacent shape.
acl_lease.rs:635 skips ONLY `file_type.is_dir() && file_name == "quarantine"`.
- std Windows FileType::is_dir() is false for reparse points (symlink/mount-point tags),
  so a planted junction named `quarantine` falls through to the hard-error branch -> Err.
- A FILE named `quarantine` -> not is_file()+".toml" -> hard error. Unchanged.
- read_dir is NOT recursive and the branch `continue`s, so nothing inside quarantine/ is
  ever parsed, validated, or trusted. No read path crosses into it in product code
  (only tests read it: tests.rs:268).
- create_or_open_child_directory (storage.rs:525) is the SAME helper that builds the
  existing lease root chain, with the same CreateDirectoryW(NULL sa) inherited-DACL and
  the same open_directory_nofollow reparse rejection + same_windows_path check.
  => No new writable surface and no new trust crossing relative to the lease root itself.
Verdict on the allow-list: does not create a new wedge.

### A3. "Dropping the allow-list kills only the re-entrancy test" — TRUE, and I found WHY.
mutate2.ps1 runs each of the four tests in its OWN `cargo test --exact` invocation.
storage.rs:98 test_lease_root() is keyed on `std::process::id()` and wiped at start, so
each invocation gets a FRESH root. Only quarantine_directory_does_not_become_a_second_wedge
performs TWO recovery passes in ONE process, so only it can observe the quarantine dir.
The claim is accurate for that harness. NOTE (strengthens the fix): in the FULL suite all
four share one root, so under M1 more than one would fail there. The allow-list is guarded
harder in the real suite than the by-name mutation suggests.

### A4. Honour-when-alive — STRUCTURALLY sound, stronger than the summary claims.
acl_lease.rs:652 `if owner_is_live(&lease)? { continue; }` is the FIRST statement in the
per-lease loop, dominating ALL THREE mutating branches (DeleteAppContainerProfile at :660,
reclaim at :680/:693, cleanup_locked at :696). A live owner is untouched on every path,
not just the reclaim path. M2 (`if false`) kills live_owner_..._is_honoured_not_reclaimed
and nothing else -> the honour test is not satisfiable by reclaim-everything.

### A5. Open attack lines still to close
- Original finding text (28-02 §7) vs what was actually repaired: scope match?
- Residual-ACE argument: does reclaiming trade DoS for a containment hole?
- Candidate FOURTH self-passing gate: reclamation_reports_grants_it_could_not_revoke
  asserts only the QUARANTINED FILE contents, never the operator message. Check whether
  3f3f93dc's extracted reclamation_report() closes that.
- Adjacent unrepaired wedge: read_validated_lease Err / owner_is_live Err still abort the
  whole pass. Pre-existing, but is it inside F-28-02-002's stated scope?
- The gate self-test at 28-04-FINDING-LEDGER.md:1182.
