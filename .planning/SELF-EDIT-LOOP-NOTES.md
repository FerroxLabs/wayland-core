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
