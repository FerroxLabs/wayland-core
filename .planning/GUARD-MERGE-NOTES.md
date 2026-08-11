# Merging the two unsaved-work guards

`integ/round2-base` (INV-2 round 5) and `lane/b1-resume-real` (P2b) both
rewrote `crates/wcore-tools/src/unsaved_work.rs`. They are not in conflict as
designs — round 5 hardens the *Write* surface, B-1 adds the *shell* surface —
but B-1 was written against the pre-round-5 module and its two halves cannot
simply be concatenated. Round 5 replaced the whole data model B-1's addition
was built on.

## The seven conflicts

| # | File / hunk | Resolution | Why |
|---|---|---|---|
| 1 | `unsaved_work.rs` module doc | Round 5's doc, with the `Bash` carve-out bullet rewritten and a new `# The shell surface` section | Round 5's doc is the fuller statement and B-1's "Scope, stated honestly" block describes a baseline model that no longer exists. The one thing B-1's doc had that round 5's lacked — the measured `git checkout -- SHIPPING-API.md` loss — is preserved in the new section. See below. |
| 2 | `use std::sync::{…}` + B-1's `shared()` | Round 5's imports; B-1's free `shared() -> &'static UnsavedWorkGuard` **dropped** | Round 5 already owns a process-wide singleton, `UnsavedWorkGuard::shared() -> Arc<…>`, which additionally pins the working repository eagerly at first call and deliberately does *not* cache a failed resolution. Keeping B-1's second `OnceLock` would have produced two guards and destroyed the very sharing B-1 needs. |
| 3 | `unsaved_lines` / `baseline` vs `note_written` / `authored_lines` | Round 5's methods kept verbatim; `unsaved_lines` **reimplemented** against them | B-1's `unsaved_lines` was one line — `self.dropped_lines(path, "")` — over a per-path first-touch baseline that round 5 deleted. The port asks round 5's question instead: tally the disk, subtract the agent-authored copies, subtract what the *pinned* commit records. |
| 4 | `shell_refusal` block vs `recorded_raw` | Both kept; B-1's block moved to a new `unsaved_work/shell.rs` | Straight ordering conflict. Four substantive changes were made to B-1's code while porting it — see "Changes made to B-1's half". |
| 5 | `mod tests;` vs B-1's inline `mod tests { … }` | Round 5's `mod tests;` | Round 5 split the tests into `unsaved_work/tests.rs`. B-1's inline module also carried the pre-round-5 tests, which exercise `dropped_lines` and `UnsavedWorkGuard::new`, neither of which exists any more. B-1's six P2b tests were ported into `unsaved_work/tests.rs`. |
| 6 | `write.rs` — the `unsaved` field type | Round 5's `Arc<UnsavedWorkGuard>` | Same reason as #2. `&'static` is strictly less capable: `with_unsaved_guard` (used by every isolated-guard test and by hosts running several sessions in one process) needs an owned handle. |
| 7 | `write.rs` — `WriteTool::new` | Round 5's `UnsavedWorkGuard::shared()` | Same instance B-1's `shared()` wanted, already there. |

`bash.rs` (the four `BashTool` entry points) and `wcore-agent/src/context.rs`
auto-merged and were left as git produced them.

## What the merged doc now says the guard covers

The base's doc contained an explicit carve-out — "**`Bash` is not covered at
all**" — which B-1 exists to close. The bullet now reads:

> It covers **all three write surfaces, but not equally**. `Write` is
> refused-or-copied as above. `Edit` is never refused but never claims a copy
> it did not make. **`Bash` is covered for one shape only**: a git command
> whose whole purpose is to throw the work tree away is refused by
> `shell_refusal`, from every `BashTool` entry point, before any shell is
> spawned. Everything else a shell can do to a file — `sed -i '2d'`, `>`,
> `rm` — still does not route through here and cannot at this altitude.

and a new `# The shell surface` section states the measured B-1 defect, the
sharing mechanism, and five scope limits — including one the merge added
rather than inherited: **`git clean` is the gap.** It is the only discard whose
victims are untracked files, and its bare `git clean -fd` form names no path,
so nothing is enumerated for it and nothing is refused.

## Changes made to B-1's half while porting it

Each is a defect in B-1's code that round 5's own doctrine forbids, not a
preference.

1. **`unsaved_work_tree_paths` now goes through `git_run`.** B-1 used a raw
   `Command::new("git")`, which inherits `GIT_DIR`, `GIT_WORK_TREE`,
   `GIT_INDEX_FILE` and the rest. Round 5 clears all seven for exactly this
   reason: with `GIT_DIR` set, the enumeration would list one tree while
   `unsaved_lines` judged another.
2. **`entry[3..]` replaced with `entry.get(3..)`, and rename entries skipped.**
   `git status -z` emits a rename's original path as its own field. Slicing
   byte 3 of one panics when byte 3 lands mid-character — a two-character
   accented path is enough. This was a live panic in production code.
3. **`git -C <dir>` is now honoured.** B-1 skipped `-C` and its operand, so a
   `git -C /elsewhere checkout -- file.py` was judged against the shell's own
   directory. That is the one failure direction B-1's own doc rules out ("the
   worst case is the refusal not firing, never a wrong refusal").
4. **Quoted lines are scrubbed.** The refusal echoes file contents into the
   model's context. Round 5 puts every such quote through
   `wcore_safety::PIIScrubber` (`quote_dropped`); B-1's shell refusal did not.

One behavioural decision is worth naming: `shell_refusal` returns `None`
immediately when the shell's own directory is in no git work tree. Round 5's
`assess` treats "no repository" as *proven nothing recorded* and refuses, but
on the shell surface a discard outside a work tree discards nothing — git
refuses the command itself — so the refusal would be noise. The test is the
`.git` marker on the filesystem, never a git exit code, per round 5's own rule.
`Baseline::Unknown` (a repository git will not open) still fails closed and
refuses.

## Verification

Red arms, three repetitions each, the file `touch`ed after both mutation and
restore. Each mutation is asserted to land on code, not a comment, before it is
applied.

| Property | Enforcement site mutated | Graded by | Green | Mutant | Restored |
|---|---|---|---|---|---|
| Write — INV-2 r5 pre-image / rename | the `pre_image_unchanged` block in `write.rs` | `inv2_round5_adversarial_test::a_save_during_the_assessment_window_is_not_lost` | 3/3 pass (1 test) | 3/3 **FAIL** | 3/3 pass |
| Bash — wiring | `bash.rs::unsaved_shell_refusal` body → `None` | `tests/bash_unsaved_work_test.rs` | 3/3 pass (3 tests) | 3/3 **FAIL** (2 of 3 red; the negative control stays green) | 3/3 pass |
| Bash — logic | `UnsavedWorkGuard::unsaved_lines` → empty | `--lib shell_refusal` | 3/3 pass (7 tests) | 3/3 **FAIL** (4 of 7 red; the three "must be allowed" arms stay green) | 3/3 pass |

Mutant failure text, for the record:

```
a_save_during_the_assessment_window_is_not_lost:
  a save that arrived during the assessment was destroyed uncopied
```

## The singleton, proved end to end

`tests/bash_unsaved_work_test.rs::write_then_bash_lets_the_agent_revert_its_own_new_file`
drives the real `WriteTool::new(None)` to create a file, then the real
`BashTool` to `git checkout --` it. Two tools, no shared handle between them in
the test, and the revert is allowed — which is only possible if both reached
the same `UnsavedWorkGuard::shared()`. Its positive control writes an
identically-shaped untracked file that the guard never wrote, and that one is
still refused.

The carve-out is per line and per copy, not per file:
`the_agents_own_lines_do_not_shelter_the_users` puts a user's uncommitted line
in a file the agent then rewrote, and asserts the refusal quotes the user's
line and does not quote the agent's.
