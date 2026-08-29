# Orphan lane branch dispositions — FerroxLabs/wayland#1181

Recorded 2026-08-29 against `integ/f13-base` @ `0df4c47d` (= `integ/next` @
`43848f75` plus the ledger re-grade commit).

One verdict per branch, as #1181 requires. All four are **SUPERSEDED**, each by a
named commit. None is merged by ancestry, and none is obsolete.

## Why ancestry, `git diff` and `git cherry` were not used

This repository **squash-merges**. A squash destroys both the ancestry link and
the patch-id, so all three of the usual answers are wrong here in the same
direction — they report "not present" for work that is fully present:

- `git merge-base --is-ancestor <tip> HEAD` fails for every one of the four.
- `git cherry` lists every commit as unmerged.
- `git diff HEAD..<tip>` is dominated by the thousands of commits of unrelated
  lineage the branch forked before.

The test used instead is **reverse application**: `git show <sha> | git apply
--reverse --check`. It succeeds only if the exact hunks are present in the
working tree, whatever route they took to get there.

Reverse-apply is necessary but not sufficient: a commit whose lines were later
edited by a follow-up will not reverse-apply even though its substance landed.
Every commit that failed the reverse-apply check was therefore graded a second
time by naming its introduced symbols and grepping the tree for them, and the
superseding commit was found with `git log -S '<distinctive string>'
--first-parent HEAD`.

## lane/walk-parallel — 13a81ab8 — SUPERSEDED by addb4f48

Three commits: `13cc6ef5` (red arm), `b024b922` (parallel walk + deduped walk
roots), `13a81ab8` (grade the parallel arm rather than the serial one twice).
`13a81ab8` reverse-applies cleanly; the other two are present under later
edits — `SERIAL_WALK_BUDGET`, `walk_root_is_covered` and the whole of
`tests/walk_parallel_identity_test.rs` are in the tree, introduced by
`addb4f48` and refined by `92ee5374`, `d1f55f0b` and `620ddc79`.

**This branch is in the "an assertion that cannot fail" class, so presence was
not accepted as the answer.** `13a81ab8`'s own message states the guard it
added: a `node_modules` prune inserted into the PARALLEL closure alone must
fail the suite, where before the fix it passed all 38 secret-walk tests. That
mutation was re-run against this tree on 2026-08-29 (hetzner, `-j 6`):

```
                    && !entry.path().to_string_lossy().contains("node_modules")
```

inserted at `workspace_policy.rs:2574`, inside the `build_parallel().run`
closure — asserted to be CODE and not one of the four `node_modules` mentions
in the comment block at `:2483-2492`. Result:

```
Summary [3.510s] 5 tests run: 3 passed, 2 failed, 0 skipped
  FAIL wcore-tools::walk_parallel_identity_test deny_set_is_complete_and_identical_on_the_parallel_arm
  FAIL wcore-tools::walk_parallel_identity_test the_parallel_arm_returns_exactly_what_the_serial_arm_returns
    the parallel arm returned a different deny set than the serial arm -
    only in parallel: [], only in serial: ["node_modules/vendor/deep/client.pem"]
```

Exactly the two tests `13a81ab8` said would fail. Restored and re-run: 5/5 pass.
The fix is not merely textually present, it is live.

## lane/winpath — 4089798c — SUPERSEDED by addb4f48

Three commits: `7d8a8a8b` (red arm for the pre-connect silence),
`50b30a1c` (cover dispatch-to-first-byte with the silence timer),
`4089798c` (normalize the skills output-dir token to forward slashes).
`4089798c` reverse-applies cleanly. The other two are present:
`pub async fn awaiting_first_byte` at `http_client.rs:157` with its four
tests — `a_fast_dispatch_does_not_fire_the_connect_silence_signal`,
`an_established_stream_that_goes_quiet_still_emits_exactly_one_notice`,
`a_stalled_dispatch_surfaces_a_silence_signal_before_the_connect_timeout`,
`the_silence_threshold_must_beat_the_connect_deadline` — and
`wcore-skills/src/paths.rs::normalize_path_separators`. Both introduced by
`addb4f48`.

## lane/tools-bash — c7aeaf2d — SUPERSEDED by addb4f48

Three commits: `cb43a0b9` (red arm), `1aaaef9b` (cancellable Bash deny walk,
lossy-output flag), `c7aeaf2d` (name the cause when the manifest build times
out). None reverse-applies; all three are present:
`LOSSY_OUTPUT_NOTE` (`bash.rs:419`), `decode_lossy` (`:430`),
`drain_lines` (`:446`), `spawn_manifest_build` (`:496`), and the named
build-timeout message — "…the workspace secret-scan); the command never ran" at
`:994` and `:1156`, both arms. Introduced by `addb4f48`.

## lane/win-fix — c5ce3857 — SUPERSEDED by 9150ff1f

The oldest and largest (2026-08-01, forked at `61b79c4f`, 0.12.25). Eleven
substantive commits were graded individually; two reverse-apply (`a9be1214`,
`82455bd6`) and the other nine are present under later edits:

| commit | evidence in this tree |
|---|---|
| `c5ce3857` | `justfile` `[unix]`/`[windows] test-ci` split (:44-68); ci.yml "Assert this leg produced test signal" (:591, :991); `config.rs::home_alone_isolates_on_unix_and_does_not_isolate_on_windows`; `snapshot.rs::set_hostile_file_dacl` / `release_hostile_dacl`; `.planning/WINDOWS-TRIAGE-2026-07-31.md` |
| `0c264c0e` | `scripts/wayland-e2e-windows-soak.ps1`: `Exit-Soak`, `Assert-NativeExit`, `Get-NextestExecutedCount`, `Assert-TestsExecuted` |
| `0ee602b6` | `portability_hostile_corpus.rs::python()`, `receipt_contract.rs::abs()` |
| `5ca243b8` | `prove_process_group_empty`, `a_group_holding_only_the_anchor_corpse_is_cleaned_not_failed`, `a_group_that_still_holds_a_live_process_cannot_prove_itself_empty` |
| `90861a56` | `a_group_whose_every_member_is_gone_censuses_as_empty` |
| `075b541b` | `MacIdentityRecheck`, `root_exit_after_sentinel_joins_still_yields_containment`, `joining_a_vanished_process_group_is_refused_by_the_kernel` |
| `1d40b88d` | `process_tree.rs:2580` `ProcessGroupCensus::Live(n) if n >= 1` |
| `284d8b8e` | `artifact.rs:40` `PROBE_TIMEOUT: Duration = Duration::from_secs(90)` |
| `df4602c6` | `bash.rs:174` the `overridden` TMPDIR replacement |
| `5d1eda16` | `.planning/HANDOFF-2026-08-01.md` |

**This branch is in the "a green check that ran nothing" class, so the gate it
added was exercised rather than read.** The per-leg step at `ci.yml:591` was
extracted verbatim and driven against synthetic JUnit on 2026-08-29:

```
ARM A  no junit at all           -> ::error NO TEST SIGNAL (Windows)      exit=1
ARM B  junit declaring tests="0" -> ::error ZERO TESTS (Windows)          exit=1
ARM C  junit declaring 13350     -> leg: Windows … tests declared: 13350  exit=0
```

Arms A and B are the two ways the leg went dark for eight consecutive runs; arm
C is the control that keeps this from being a permanently-red gate. Both a
reachable pass and a reachable fail — the gate can do its job.

## Archive refs

Three of the four tips are now preserved on the remote, following the precedent
of `origin/archive/finish-criteria-superseded`:

- `origin/archive/lane-walk-parallel-superseded`
- `origin/archive/lane-winpath-superseded`
- `origin/archive/lane-tools-bash-superseded`

`lane/win-fix` **could not be archived**: it carries `.github/workflows/ci.yml`
and the push is rejected with *"refusing to allow an OAuth App to create or
update workflow `.github/workflows/ci.yml` without `workflow` scope"* — the
same token limitation `MASTER-PLAN.md` already records against #1177. Rejected
as a branch and as a tag. Its tip `c5ce3857` therefore lives only on hetzner,
and archiving it needs a `workflow`-scope token (Sean). Nothing is at risk from
that: every commit on it is verified present in `origin/main` via `9150ff1f`,
so a box loss costs the history, not the work.

None of the four local branches was deleted. Deleting them is not required by
the criteria, and it would remove the only copy of `lane/win-fix`.

## Why these outcomes are cited as a FILE and not as `commit:<sha>`

The obvious evidence token for "superseded by a named commit" is
`commit:addb4f48`. It resolves on hetzner and it does NOT resolve in CI, and
that is worth writing down because it is not specific to this issue.

`.github/workflows/ci.yml` checks the repository out at `fetch-depth: 1`. The
criteria-ledger gate resolves a `commit:` token with `git cat-file -t`, which
in a one-commit checkout can only ever succeed for HEAD. Measured on run
33248439907 (job 99089835386), step *"Criteria ledger is anchored and
parseable"*: **76 problems, and 72 of them are the same sentence** —

```
last_verified_commit 43848f75 is not a commit in this tree --
the entry was verified against something that is not here
```

— one for each of the 61 ledger files plus the `commit:` evidence tokens in
`wayland-1168` and `wayland-1181`. That step therefore cannot pass in CI for
any branch, on any tree, no matter what a lane does: every ledger file the
convention requires carries a `last_verified_commit` sha by construction. It is
a gate with no reachable pass state, which this repo treats as worth exactly as
much as one that cannot fail.

Nothing here fixes that — it belongs to whoever owns `check-criteria-ledger.py`
and the workflow, the script would need to detect a shallow clone and downgrade
commit resolution to a skip with a named reason, and a `ci.yml` edit needs a
`workflow`-scope token this box does not have. What this lane does do is
decline to make it worse: the four outcomes above are anchored to the section
of this record that states them, which resolves in a shallow checkout and in a
full one, and the superseding sha is named in the section and in the ledger
note either way.
