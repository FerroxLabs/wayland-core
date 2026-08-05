# SELF-EDIT-LOOP — lane summary

Branch `lane/self-edit-loop`, base `c9ab048b952c5bc74c75ea8f76df06788408de59`.

**Verdict: goal ACHIEVED.** The engine no longer reports its own activity to the
model as user edits, and a genuine external edit is still reported — proven on
BOTH platforms, in BOTH directions, with the failing state demonstrated first.

---

## What was wrong, measured

`bootstrap.rs:3139` arms a recursive watcher on cwd unconditionally.
`is_wcore_internal_path` (watch.rs) filters by path COMPONENT, so it drops
`<cwd>/.wayland-core/...` but is structurally unable to drop a bare `<cwd>`.
A directory's own events are reported AGAINST that directory, so engine
activity surfaced as the watch-root path and was injected verbatim:

```
Linux  (chmod of cwd):        ".../project [Modify(Metadata(Any))]"
macOS  (creating .wayland-core/): ".../project [Create(Folder)]"
                                  ".../project [Modify(Metadata(Extended))]"
injected: User edited `.../project` while I was thinking — re-read it before proceeding.
```

The scoping agent's mechanism was right, and **the macOS case is the exact one
the brief described**: creating `.wayland-core/` surfaces the PARENT. On Linux
the same scenario surfaced nothing; there the leak comes from a different
writer. So the defect is not Darwin-only, and neither platform alone tells the
whole story.

## The fix

`is_root_structural_noise(path, kind, root_raw, root_canon)` in `watch.rs`,
applied at the notify callback beside the existing component filter so the
`drain`/`next_external_event` consumers are covered, not just the renderer.
The root is resolved once in `new()` (raw + canonical, since notify normalizes
`/var` → `/private/var`) because the callback runs on a platform thread that
must not block.

**It discriminates by event KIND, and that is the whole point.** My first
version dropped every event whose path was the watch root. It fixed Linux and
**disabled external-edit detection on macOS outright** — 3/3 runs, every
genuine user edit returned empty, because FSEvents also names the root when
something inside it changes. That patch was measured, disproved and replaced.

Census across both platforms:

| path | kinds observed |
|---|---|
| watch root | `Create(Folder)`, `Modify(Metadata(Any/Extended/Ownership))` |
| genuine edited file | `Create(File)`, `Modify(Data(Content))`, `Modify(Metadata(..))` |

A root event was never once observed carrying `Modify(Data(..))` or
`Create(File)`, so only folder-create and metadata-only events **on the root**
are dropped. A content change survives even on the root.

## Both-directions proof

| | Linux (hetzner) | macOS (Mac) |
|---|---|---|
| before fix | 1 failed / 6 passed — root injected | 3 failed / 4 passed — root injected |
| after fix | **7 passed, 0 failed, 0 ignored, 0 filtered out ×3 runs** | **7 passed, 0 failed, 0 ignored, 0 filtered out ×3 runs** |

Direction 2 is load-bearing, not decoration: three tests assert a genuine edit
in a subdirectory, a genuine edit of a file directly in the watch root, and a
real edit racing engine churn. A filter that suppressed everything passes
Direction 1 alone — and my first one did exactly that.

Unit tests additionally pin the string-prefix trap (`/projector` against a
`/proj` root), so rewriting the comparison as `starts_with` reddens.

## Second finding — `await_session_switch`

Confirmed and fixed: `for _ in 0..100 { …; yield_now().await }` is a budget in
scheduler reschedules, and a yield lets no wall-clock pass, so it could not
distinguish a broken switch from a busy binary. Now a wall-clock deadline with
a real sleep. Proven both directions: the 5 `_f14` callers pass unmutated, and
with the completion check forced false the assert fires —
`session switch did not complete within 30s`.

**The brief's claim about `await_recovery_action` is FALSE.** It is not the
same shape: it uses `tokio::time::sleep(5ms)` per iteration, so its 400
iterations are a real if rough 2s budget, not a reschedule count. Left alone.

## Premises I found false

1. **`await_recovery_action` has the identical defect** — false, see above.
2. **`[memory] enabled = false` still creating `memory.db` is a defect** —
   false, and I verified it rather than taking the scoping agent's word:
   `packaged_driver_gate.rs:825` asserts
   `expected_memory_backend = memory_enabled || lifecycle_enabled` across the
   full 2×2×2 matrix. Constructing the backend when skills-lifecycle is on is
   intended, asserted behaviour. "Fixing" it would break that gate.
3. **The defect is a Darwin/FSEvents story** — incomplete. It reproduces on
   Linux too, via a different writer.

## New finding — `agent.watch_files` is a phantom config knob

`file_watcher_notifier.rs:25` documents the watcher as gated on
`agent.watch_files`. It is not: `bootstrap.rs:3139` is unconditional, and
`watch_files` occurs **exactly once in the entire worktree — in that doc
comment** (absence proven with two live known-positive controls in the same
invocation, after zsh ate the first unquoted glob). There is no way to switch
the watcher off, which is why the reporting lane still saw injections across
"three configurations". **Reported, not fixed** — adding a config surface is a
product decision, not a filter repair.

## Instrument defects found and REPAIRED in-lane

1. **`rtk` fabricated a SHA.** `git log -1 --format=%H c9ab048b` returned
   `041ae82c…`, which is not a prefix-extension of the abbreviation asked for.
   Worse than the re-rendering already recorded in §3b, because a SHA looks
   authoritative and would mis-anchor every subsequent diff.
2. **My harness was a permanently-green gate.** First run: `6 passed; 0 failed`
   while the same run's dump showed the root HAD leaked. `tempfile` names its
   dir `.tmpXXXXXX` and `path_should_surface_as_edit` drops any name starting
   with `.tmp`, so the watch root was eaten by an unrelated filter and
   Direction 1 could not fail. Repaired to watch `<tmpdir>/project`, with the
   three-assertion self-test — the third being that a `.tmp`-prefixed root
   renders as nothing, which is the only assertion that proves the repair does
   anything.
3. **My Direction-2 waits were budgets, not deadlines** — the same defect as
   `await_session_switch`, and it is why the same test was measured passing and
   failing at the same pre-fix commit. Now polls to a wall-clock deadline and
   accumulates events across drains (draining twice loses the first batch).
4. **My poll loop was self-passing.** `grep -c X f || echo 0` prints `0` AND
   exits 1, so the variable held `"0\n0"`, which `!= "0"` — it declared DONE on
   iteration 1 unconditionally. Repaired with the three-assertion self-test.
   No reported figure rests on it; every run was validated by reading the log.
5. **A timeout message that could not track its constant** — the panic said
   "30s" while the mutated budget was 2s. Now derived from the constant.

## Verification, read from files

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | **0** |
| `cargo metadata --locked` | **0** |
| `cargo check --workspace --all-targets` | **0** |
| `cargo clippy -p wcore-cli --all-targets -D warnings` | **0** |
| `cargo clippy -p wcore-agent --lib --test watch_self_edit_loop_test -D warnings` | **0** |
| `watch_self_edit_loop_test` Linux ×3 | 7 passed; 0 failed; 0 ignored; 0 filtered out |
| `watch_self_edit_loop_test` macOS ×3 | 7 passed; 0 failed; 0 ignored; 0 filtered out |
| `file_watcher_test` | 2 passed; 0 failed; 0 ignored; 0 filtered out |
| `wcore-agent --lib watch::` | 12 passed; 0 failed; 0 ignored; 2245 filtered out |
| `wcore-agent --no-fail-fast` | 3246 passed; 13 failed; 10 ignored; 47 filtered out |
| `wcore-cli --no-fail-fast` | 2455 passed; 6 failed; 19 ignored; 0 filtered out |

### Every failure attributed — none is mine

- **`clippy -p wcore-agent` (whole crate) FAILS**, in
  `tests/user_model_identity_wire.rs` and `tests/cache_ledger_engine_test.rs`.
  `git diff base..HEAD` on both = **0 lines** (control: `watch.rs` = 2 lines,
  non-empty). Scoped to my code, clippy is **RC 0**. Not mine, not fixed —
  out of scope.
- **`wcore-cli` 6 failures.** One is `failing_fixture`, a DELIBERATE fixture
  `plugin/scaffold.rs:314` generates as `fn always_fails() { panic!() }` and
  runs in a nested cargo subprocess — counting it as real would have been
  wrong. The other four binaries fail **identically at base**:
  `f14_sigkill_recovery` 10/1, `harness_regression` 13/2,
  `portability_hostile_corpus` 22/1, `proving_ground` 12/1.
- **`wcore-agent --lib` failures.** Apples-to-apples, `--lib` alone:
  **BASE = 18 failed, then 17 failed on a second run at the same commit;
  HEAD = 17 failed.** HEAD is no worse than base, the failing names differ
  between the two base runs, and no `watch::` test failed in any run. They sit
  in the journal/session/recovery family the LANE-BRIEF already documents as
  wall-clock-flaky.

### Unrun cells — counted, not hidden

- **`cargo test` stops at the first failing BINARY.** My first capture read
  `wcore-agent binaries=1`; cargo had aborted after `--lib`, so ~160
  integration binaries never ran and were silently counted as nothing.
  Re-run with `--no-fail-fast`: 161 binaries started for `wcore-agent`, 61 for
  `wcore-cli`. The figures above are from those runs.
- **Windows: NOT RUN — a real gap in this fix.** `ReadDirectoryChangesW` may
  report a root attribute change as `EventKind::Modify(ModifyKind::Any)`
  rather than `Modify(Metadata(_))`, which my predicate would NOT suppress, so
  the loop could persist on Windows. I deliberately did not widen the
  predicate to `Modify(Any)`, because that kind could equally carry a
  coalesced content change and suppressing it would re-create the
  blinds-everything bug I already made once. Resolving this needs a measured
  Windows event-kind census on `SeanD@seandesktop`; hetzner cannot reach that
  host. **Reported as an open gap, not claimed as covered.**
- No test was `#[ignore]`d, weakened, re-gated or deleted.

## What I did NOT do

No PR, no merge to integration, no tag, no GitHub issue touched, no
`wcore-contract generate`. No `git rebase`, `reset --hard`, `stash` or `clean`
(only `git checkout -- <one named path>` in my own worktree, twice, to swap
`watch.rs` between committed revisions for the control runs). No credential
was used, printed or needed — this lane's proofs need no provider. Pushed
`lane/self-edit-loop` only.

**Darwin exception disclosed (LANE-BRIEF §0):** I ran
`cargo test -p wcore-agent --test watch_self_edit_loop_test` on the Mac —
single crate, single test binary, no workspace build, no clippy, no release.
Justified because the question was whether FSEvents surfaces the watch root,
and hetzner is Linux and structurally cannot answer it. It earned its keep: it
is what caught my first fix disabling the feature on macOS.
