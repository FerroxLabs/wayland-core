# SELF-EDIT-LOOP — lane notes (append-only, committed after every measurement)

Lane: `lane/self-edit-loop`
Worktree: `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-self-edit-loop`
Base: `c9ab048b952c5bc74c75ea8f76df06788408de59` (asserted with `/usr/bin/git rev-parse`)
Integration head at lane start: `33f249751430df02401503a1067919454811c89c` (already ahead of my base)

## Brief

The engine reports its own writes to the model as user edits, causing infinite
"re-read" loops. Fix the filter so engine writes are not mistaken for user
edits, and prove BOTH directions: a genuine external edit must STILL surface.

Secondary: `surfaces/mod.rs::await_session_switch` is a budget in scheduler
reschedules, not a deadline.

---

## Instrument defects found (repair in-lane, per LANE-BRIEF §6b-ii)

### ID-1 — `rtk` fabricated a SHA (NEW instance of the §3b class)

```
git log -1 --format=%H c9ab048b     -> 041ae82c3111b73522b0016799a6c4b868e74f23
/usr/bin/git rev-parse c9ab048b     -> c9ab048b952c5bc74c75ea8f76df06788408de59
```

The proxied answer is **not a prefix-extension of the abbreviation asked for**,
so it is fabricated, not merely re-rendered. This is worse than the
`--numstat` case already recorded in §3b because a SHA looks authoritative and
would silently mis-anchor every subsequent diff.

Mitigation used for the rest of this lane: every load-bearing command is
`/usr/bin/…`, redirected to a file, and read back with the Read tool.

---

## Premise verification (LANE-BRIEF: "your brief's measurements are probably stale")

Verified at base `c9ab048b95`, by reading the files, not by grep summary.

| Claim | Verdict | Evidence |
|---|---|---|
| `bootstrap.rs:3139` installs the watcher unconditionally on cwd | HOLDS | `install_file_watcher_eventually` has exactly 2 non-comment sites: `bootstrap.rs:3139` (call), `engine.rs:4888` (def) |
| `watch.rs:127` watches recursively | HOLDS | `watcher.watch(root, RecursiveMode::Recursive)?` |
| `is_wcore_internal_path` (watch.rs:255-260) is COMPONENT-based | HOLDS | `path.components().any(|c| s == ".wayland-core" \|\| s == ".wayland")` |
| `path_should_surface_as_edit` (watch.rs:316) is also component-based | HOLDS | matches `target/node_modules/.git/dist/build/.planning/.blackboard/sessions` by component |
| Neither stage catches a bare watch-root path | HOLDS (by construction) | a root path has none of those components |
| `mark_self_originated` is keyed on the EXACT PathBuf | HOLDS | `watch.rs:137-141`, `insert(path.to_path_buf(), …)` |
| Its only production wiring is the Write/Edit tools | HOLDS | production sites: `wcore-tools/src/write.rs:193`, `wcore-tools/src/edit.rs:391` -> `file_watcher_notifier.rs:47`. Everything else in the grep is docs/tests. The engine's OWN session/memory writes never mark. |
| `await_session_switch` is `for _ in 0..100 { yield_now() }` then panic | TO VERIFY |
| `[memory] enabled = false` still creates memory.db — a defect | TO VERIFY (scoping agent says PREMISE-FALSE; documented intended behaviour) |

## ID-2 — MY OWN harness was a permanently-green gate (repaired in-lane)

First run, hetzner, commit `9c336a4c`, `BASELINE.log`:
`test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`

…while the raw event dump in the SAME run read:
`[shape] chmod_on_watch_root surfaced: ["/tmp/.tmpbNcTnJ [Modify(Metadata(Any))]"]`

The watch root HAD leaked and the gate could not see it. Cause: `tempfile`
names its directory `.tmpXXXXXX`, and `path_should_surface_as_edit`
(watch.rs:351) drops any path whose file name starts with `.tmp`. The watch
root was being eaten by the atomic-write scratch filter — a filter with
nothing to do with the property under test. Direction 1 could not fail.

Repair: watch `<tmpdir>/project`. Self-test carries the three assertions
LANE-BRIEF §6b-ii requires, the third being "the old matcher would have missed
it". Committed `7c42063e`.

## MEASUREMENT — the defect, reproduced (hetzner, Linux, commit 7c42063e)

`RED.log`: `test result: FAILED. 6 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out`

```
injected: Some("User edited `/tmp/.tmp5rMPaP/project` while I was thinking — re-read it before proceeding.")
surfaced paths: ["/tmp/.tmp5rMPaP/project [Modify(Metadata(Any))]"]
watch root:     /tmp/.tmp5rMPaP/project
```

The watch ROOT is surfaced as an ExternalEvent and rendered as an edited file.
That is the production symptom exactly. **Confirmed on Linux — not
Darwin-only**, which the scoping agent's FSEvents-centred mechanism did not
predict.

Refinement to the scoping agent's mechanism: on Linux the trigger is an
`IN_ATTRIB`-class event on the watch descriptor itself (`Modify(Metadata(Any))`
against the root path). `.wayland-core/` *creation* surfaced `[]` on Linux —
inotify names the child, which the component filter already drops. So the
root-path leak is real and reachable, but the specific writer that triggers it
is platform-dependent.

## ID-3 — `agent.watch_files` is a documented config knob that does not exist

`file_watcher_notifier.rs:25` says the watcher is "Constructed once in
bootstrap when `agent.watch_files` is enabled". It is not: `bootstrap.rs:3139`
installs it unconditionally, and `watch_files` occurs **exactly once in the
entire worktree — in that doc comment**.

Absence proven with live controls in the same invocation (quoted globs, after
zsh ate the first unquoted attempt):
`install_file_watcher_eventually` = 3 hits, `is_wcore_internal_path` = 4 hits,
`watch_files` = 1 hit (the doc comment). So there is **no way to switch the
watcher off**, which is why the reporting lane still saw injections across
"three configurations". Reported, not fixed — adding a config surface is a
product decision, not a filter repair.

## Open questions to measure, not assume

1. What path does `notify` ACTUALLY surface when `.wayland-core/` is created
   inside the watch root? The mechanism claim ("the parent, i.e. cwd") is the
   scoping agent's inference. It must be measured on a real watcher.
2. Is it Darwin-only? On Linux inotify a `chmod` of the watch root yields
   `IN_ATTRIB` on the root, so a root-path event is plausible on BOTH
   platforms. If so, most of the proof can run on hetzner.
3. Does any genuine file edit surface ONLY as a root path (which would mean a
   root filter blinds the feature)? Pre-existing test
   `file_watcher_notifier.rs:92 external_write_without_notifier_mark_surfaces`
   asserts a file written directly into the watch root surfaces as the FILE
   path, which is evidence against — but it needs re-reading, not assuming.
