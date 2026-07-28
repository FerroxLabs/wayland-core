# RED-68 NOTES — running log (committed early per LANE-BRIEF §6b-i)

Lane `lane/red-68`, base `plan/f20-unified-audit-repair` @ `3cfc336f`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-red-68`.

Append-and-recommit after every measurement. Do not batch to the end.

---

## T+0 — established (all figures read back from real runs)

### The 68 are enumerated. So are the 81. They are NOT disjoint.

Source: GitHub Actions run `30403867920` on HEAD `189599ca`.

| leg | job | Summary line |
|---|---|---|
| Linux containerized | `90424728480` | `12820 tests run: 12752 passed (2 slow, 2 flaky), 68 failed, 50 skipped` |
| Windows self-hosted | `90424728470` | `12469 tests run: 12388 passed (2 slow, 1 flaky, 2 leaky), 81 failed, 116 skipped` |

The Linux job ran the suite **twice** — `nick-fields/retry@v3` wraps the nextest
invocation. Attempt 1: **69 failed**. Attempt 2: **68 failed**. The single test that
differs is
`wcore-sandbox backends::process_tree::linux_tests::required_live_descendant_teardown_before_workspace_cleanup`
— i.e. it is flaky across whole-suite attempts. The authoritative 68 is attempt 2.

**Overlap answer (the board question):**

| set | count |
|---|---|
| Linux 68 ∩ Windows 81 | **33** |
| Linux-only | 35 |
| Windows-only | 48 |
| **distinct tests failing across both platforms** | **116** |

So the two lists are neither identical nor disjoint. The problem is **116 distinct
failing tests**, not 149 and not 81.

Lists: `linux68.txt`, `win81.txt`, `overlap.txt`, `linux_only.txt`, `win_only.txt`,
plus `linux69.txt` (attempt 1, for the flaky diff).

### Instrument defect found and repaired IN THIS LANE (§6b-ii)

The obvious extractor for a nextest failure list is `grep 'FAIL ['`. **It
under-counts, silently, rc=0.** nextest emits a compound status token when a test
both fails and leaks a process: `FL+LK`. Measured on the ci-linux log:

| matcher | unique failing tests extracted | Summary says |
|---|---|---|
| `grep 'FAIL ['` | **66** | 68 |
| `extract-nextest-failures.py` | **68** | 68 |

The two it dropped:

```
wcore-exec-backend orphan::tests::the_local_scanner_finds_a_descendant_that_was_deliberately_left_behind
wcore-exec-backend::fail_closed_matrix the_local_scan_finds_an_orphan_that_no_registry_remembers
```

This is the same class as every prior sighting: a matcher that answers an easier
question than the one you need, and reports absence rather than failing. It is
repaired rather than written up — `.planning/scripts/extract-nextest-failures.py`,
self-test with three assertions:

```
[ok] A1 known-positive: both FAIL and FL+LK extracted, PASS/LEAK/SKIP excluded
[ok] A2 known-negative: an all-PASS/LEAK/SKIP log yields zero failures
[ok] A3 the OLD matcher grep 'FAIL [' MISSES the FL+LK failure (old=1, new=2)
3 passed, 0 failed
```

A3 is the load-bearing one — A1 and A2 both pass against the broken matcher.

The extractor is additionally cross-checked against three independent oracles (the
three `Summary` lines) and reproduces **68 / 69 / 81 exactly**, via `--expect N`
which returns rc=1 on mismatch. It classifies by exclusion — any status token it has
never seen counts as a failure — so the next novel compound status fails loud instead
of vanishing.

### Second instrument defect, same session

`gh run view -R <repo> --job <id> --log` is intercepted by the `rtk` proxy, which
returns **`rtk: Run ID required` and rc=1** — the log never downloads. Working path:
`gh api /repos/<owner>/<repo>/actions/jobs/<id>/logs`. Recorded because the brief
already warns `rtk` silently filters `git log`; it also breaks `gh run view --job`.
`/bin/cat` and `/usr/bin/cat` differ on this Mac (`/usr/bin/cat` does not exist) —
another way a load-bearing command silently 127s inside a pipeline.

---

## Still to establish

- [ ] Serial (non-parallel) re-run of each cluster on hetzner, per the brief's
      standing warning that a full-workspace run under lane contention is not a
      measurement.
- [ ] Per-cluster root cause and classification (real defect / stale test /
      environment / already-known).
- [ ] Rank the real defects by customer impact — looking first for the
      silent-message-loss shape: loses data, reports success it did not achieve,
      or wedges permanently.
