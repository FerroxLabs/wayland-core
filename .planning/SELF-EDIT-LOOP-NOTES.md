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

## MEASUREMENT — Darwin leg (LANE-BRIEF §0 single-crate/single-test exception)

Used `cargo test -p wcore-agent --test watch_self_edit_loop_test` on the Mac.
Justified: the question is whether FSEvents surfaces the watch root, and
**hetzner is Linux and structurally cannot answer it**. No workspace build, no
clippy, no release build.

### The brief's mechanism is CONFIRMED on Darwin, and it is the stronger case

Pre-fix, `engine_state_dir_creation` (creating `.wayland-core/`):
```
".../project [Create(Folder)]"
".../project [Modify(Metadata(Extended))]"
injected: User edited `.../project` while I was thinking — re-read it before proceeding.
```
So on macOS the PARENT (cwd) really does surface when the engine creates its
own state dir — exactly as the scoping agent inferred, and the platform where
the loop was originally observed. On Linux that same scenario surfaced `[]`;
Linux leaks the root via `chmod` instead (`Modify(Metadata(Any))`).

### My first fix was WRONG — it disabled the feature on macOS

Blanket "drop any event whose path is the watch root", 3/3 repeat runs:
every Direction-2 test returned `surfaced paths: []`. That is the
"suppresses everything is as broken as suppresses nothing" failure the brief
warned about, in my own patch. **Not shipped.**

### Event-kind census — this is the discriminator

| Path | Kinds observed |
|---|---|
| watch ROOT | `Create(Folder)`, `Modify(Metadata(Extended))`, `Modify(Metadata(Ownership))`, `Modify(Metadata(Any))` (Linux chmod) |
| genuine edited FILE | `Create(File)`, `Modify(Data(Content))`, `Modify(Metadata(Extended))` |

A root event was **never** observed carrying `Modify(Data(Content))` or
`Create(File)`. So: suppress root events that are folder-creation or
metadata-only, and never suppress a content change.

### PRE-EXISTING, NOT MINE — macOS delivery granularity is flaky

The same test at the same PRE-FIX commit both passed and failed across runs:
`genuine_edit_in_subdirectory` = `ok` in `DARWIN-PREFIX.log`, FAILED in
`DARWIN-KINDS.log`. In the failing run a write to `src/main.rs` surfaced only
`project` and `project/src` — directory granularity, no file event at all.

So FSEvents sometimes coalesces a file change up to its parent directory. That
is independent of this lane's change and is why the Direction-2 assertions
need a deadline-bounded wait rather than a fixed 600 ms sleep — the same
"budget, not a deadline" defect as `await_session_switch`, in my own harness.

## ID-4 — my own poll loop was a self-passing gate

`grep -c WLDONE "$f" || echo 0` prints `0` **and exits 1**, so `$D` became the
two-line string `"0\n0"`, which `!= "0"` — the loop declared DONE on iteration
1 every time, regardless of the run's state. Repaired to
`if grep -q …; then echo yes; else echo no; fi`, with the three-assertion
self-test: known-positive `yes`, known-negative `no`, and the old matcher
proven to return not-`0` on a file containing no marker.

Earlier polls used the broken matcher, but every result was validated by
READING the log file and confirming it contained `WLDONE` plus a complete
`test result:` line, so no reported figure rests on it.

## ID-5 — `cargo test` stops at the first failing BINARY (unrun cells)

The first full-suite capture reported `wcore-agent binaries=1`. That is not a
one-binary crate — cargo aborted after `--lib` failed, so every integration
binary after it never ran. Same for `wcore-cli`, which stopped at
`f14_sigkill_recovery`. Those were unrun cells being silently counted as
nothing rather than as skips. Re-run with `--no-fail-fast`.

Also: a `test result: FAILED. 0 passed; 1 failed` line in the `wcore-cli`
section came from `failing_fixture`, a DELIBERATE fixture that
`plugin/scaffold.rs:314` generates as
`#[test] fn always_fails() { panic!("deliberate"); }` and runs in a nested
cargo subprocess. Counting it as a real failure would have been wrong.

## Attribution of the suite failures — measured, not assumed

| Failure | Verdict | Evidence |
|---|---|---|
| `wcore-cli` `isolated_profile_without_secure_store_fails_before_turn_or_provider_intent` | **PRE-EXISTING** | fails identically at base `c9ab048b` in a dedicated worktree (`BASECTL.log`) |
| `wcore-agent --lib` failures (4 / 13 / 17 depending on run) | **PRE-EXISTING AND FLAKY, no regression** | apples-to-apples `--lib` alone: **BASE = 18 failed then 17 failed** on two runs at the same commit; **HEAD = 17 failed**. HEAD is no worse than base. The failing NAMES differ between the two base runs, which is the flake signature. Filtered subset is `78 passed; 0 failed` at both base and HEAD. No `watch::` test failed in any run. |
| `clippy -p wcore-agent` errors in `user_model_identity_wire.rs`, `cache_ledger_engine_test.rs` | **NOT MINE** | `git diff base..HEAD` on both files = 0 lines (control: `watch.rs` = 2 lines, non-empty). `clippy -p wcore-agent --lib --test watch_self_edit_loop_test -D warnings` = **RC 0** |

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
