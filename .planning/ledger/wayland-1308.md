---
issue: 1308
repo: FerroxLabs/wayland
kind: defect
title: "Windows: four wcore-skills watcher_tests fail together with ERROR_PATH_NOT_FOUND, and the bare unwrap hides which path"
status: open
last_verified_commit: 90678f25
criteria:
  - id: c1
    text: "The four call sites report WHICH path was not found, so the next occurrence names the missing component instead of Os { code: 3 }."
    state: met
    evidence: "symbol:crates/wcore-skills/src/watcher_tests.rs::expect_fs"
    owner: core
    note: "MET at 90678f25, and PROVEN by a live failure rather than by reading the diff. All four call sites now go through `expect_fs` -- modify at :304, delete at :339, rename at :375, the debounce writes at :409 -- which prints the operation, the full path, the OS error, and then walks the path from the root marking each component `present` or `MISSING` and stops at the first gone one. Windows, 2026-09-10, at the pre-fix helper: `delete on D:\\...\\wcore_watcher_test_tc07_0\\SKILL.md failed: The system cannot find the file specified. (os error 2)` followed by five `present` lines and `MISSING ...\\SKILL.md`. The old payload for the same failure was `called Result::unwrap() on an Err value: Os { code: 3, kind: NotFound }` and nothing else. ORIGINAL FILING, kept: run 33751975177 (CI (Array), Windows), four siblings failed together at watcher_tests.rs:250/285/320/353 in 0.230-0.640s -- fast, so not a timeout -- all with Os { code: 3, kind: NotFound }. ERROR_PATH_NOT_FOUND is 3 and ERROR_FILE_NOT_FOUND is 2; on Windows those are distinct, and 3 means a DIRECTORY COMPONENT is missing rather than the leaf file. (CI (Array), Windows). Four siblings failed together at watcher_tests.rs:250/285/320/353 in 0.230-0.640s -- fast, so not a timeout -- all with Os { code: 3, kind: NotFound }. ERROR_PATH_NOT_FOUND is 3 and ERROR_FILE_NOT_FOUND is 2; on Windows those are distinct, and 3 means a DIRECTORY COMPONENT is missing rather than the leaf file. A bare unwrap on a filesystem Result is how a diagnosable failure becomes an unreadable one: the path is discarded, so the artifact cannot say which one."
  - id: c2
    text: "The missing directory component is identified: never created, or removed by a sibling test sharing a root."
    state: met
    evidence: "test:crates/wcore-skills/src/watcher_tests.rs::cross_process_siblings_do_not_share_a_test_directory"
    owner: core
    note: "IDENTIFIED at 90678f25. The missing component is the TEST DIRECTORY ITSELF, and it is removed by a same-named sibling in a CONCURRENT PROCESS -- not by a different sibling in the same process, which was the standing inference and is wrong. `make_visible_test_dir` named the directory from the test name plus a PROCESS-LOCAL `AtomicU64`, and every test name is used exactly once, so the counter was always 0 and two identically-launched test binaries computed the SAME path under the same `std::env::temp_dir()`. SeanDesktop runs up to three runner services as one user, so that root is shared. Whichever process finishes first runs `TempDirGuard::drop` -> `remove_dir_all` and takes the directory the other is still watching. ARMS, Windows 2026-09-10, --retries 0: the deterministic two-process handshake control fails 0/3 pass at the pre-fix helper (f7564ea4), naming `MISSING D:\\wcore-stabilization-mcp-0908\\tmp-1303\\wcore_watcher_test_collision_0` with `raw_os_error=Some(3)` -- the outage's exact code -- and passes 5/5 at the fixed helper (785a22ea). Two concurrent processes running the four real tests fail 0/20 pass pre-fix and pass 20/20 post-fix. LIMIT, stated rather than buried: the naturalistic concurrent arm usually reports os error 2, not 3, because the sibling recreates the directory for its own next test before the survivor's call lands; only the handshake control, which holds the directory removed, reproduces 3. And no arm can prove the HISTORICAL run 33751975177 had this cause -- that runner state is gone. What is proven is that this mechanism exists on that box, produces that code, and hits those tests."
  - id: c3
    text: "Measured on Windows at --retries 0, n>=20, with the four tests run BOTH together and alone."
    state: met
    evidence: "file:crates/wcore-skills/src/watcher_tests.rs:678:MEASURED 2026-09-10 (wayland#1308 c2/c3) on SeanDesktop"
    owner: core
    note: "MEASURED at 785a22ea on SeanDesktop -- the same physical box that hosts the `CI (Array)` runner services -- cargo 1.95.0, nextest 0.9.138, debug, `--retries 0`. TOGETHER (all four in one nextest invocation): 20/20 pass. ALONE (each of the four filtered to itself): 20/20 pass each, 80/80 total. The single-process arms are ALSO 20/20 at the PRE-FIX helper, and that is the finding, not a formality: one process alone never collides, so no number of repetitions of the together-or-alone arms could ever have reproduced this, and a green in them is not evidence of a fix. The arm that discriminates is two concurrent processes sharing the temp root: 0/20 pass pre-fix (tc07 and tc08 of the losing process every single time), 20/20 post-fix. LIMITS: this box has a warm checkout and an interactive session, which a hosted runner does not, and its runner services were idle during these runs; and n=20 bounds an unobserved rate only loosely -- it does not exclude a residual mode below ~14 percent."
  - id: c4
    text: "The four entries come off .config/flaky-allowlist.txt and are DELETED rather than renewed."
    state: not-met
    owner: core
    note: "STILL NOT MET at 90678f25, and deliberately not self-graded. The four lines are `.config/flaky-allowlist.txt` :100 (tc06), :101 (tc07), :102 (tc08), :103 (tc09) and they are still in the tree. c1/c2/c3 are met, so the debt they record is discharged and they should be DELETED, not renewed -- but a concurrent worker holds pending deletions in that same file, so the edit is reported to the release owner instead of made here, and this row stays not-met until the lines are actually gone. Flip to met with `absent:.config/flaky-allowlist.txt::tc06_file_modify_triggers_notification` once they are. These four are the whole notification surface of the skills watcher -- modify, delete, rename, debounce -- so while they are listed, Windows carries no coverage of a watcher losing its directory."
---

# Filed from the fold-in run, not the original outage window

Bringing wayland-core#433 into wayland-core#432 produced a new tree, and a new
tree is a new sample. The retry-flake gate can only ever report whichever
member fires, so this cluster was invisible until then.
