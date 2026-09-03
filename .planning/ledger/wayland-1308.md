---
issue: 1308
repo: FerroxLabs/wayland
kind: defect
title: "Windows: four wcore-skills watcher_tests fail together with ERROR_PATH_NOT_FOUND, and the bare unwrap hides which path"
status: open
last_verified_commit: 2347d8f9
criteria:
  - id: c1
    text: "The four call sites report WHICH path was not found, so the next occurrence names the missing component instead of Os { code: 3 }."
    state: not-met
    owner: core
    note: "Filed 2026-09-03 from run 33751975177 (CI (Array), Windows). Four siblings failed together at watcher_tests.rs:250/285/320/353 in 0.230-0.640s -- fast, so not a timeout -- all with Os { code: 3, kind: NotFound }. ERROR_PATH_NOT_FOUND is 3 and ERROR_FILE_NOT_FOUND is 2; on Windows those are distinct, and 3 means a DIRECTORY COMPONENT is missing rather than the leaf file. A bare unwrap on a filesystem Result is how a diagnosable failure becomes an unreadable one: the path is discarded, so the artifact cannot say which one."
  - id: c2
    text: "The missing directory component is identified: never created, or removed by a sibling test sharing a root."
    state: not-met
    owner: core
    note: "Four different call sites failing in one run points at shared setup or teardown rather than any one test's own path, but that is an inference from co-occurrence and not a measurement. This criterion exists so the ticket cannot be graded on the inference."
  - id: c3
    text: "Measured on Windows at --retries 0, n>=20, with the four tests run BOTH together and alone."
    state: not-met
    owner: core
    note: "Together is the arm that matters if the cause is shared state; alone is the control that separates it from a per-test bug. RATE NOT MEASURED -- observed once. SeanDesktop was not attempted, and a green there would need care anyway: it has a warm checkout and an interactive session, neither of which a hosted runner has."
  - id: c4
    text: "The four entries come off .config/flaky-allowlist.txt and are DELETED rather than renewed."
    state: not-met
    owner: core
    note: "Listed 2026-09-03 with a 2026-09-20 expiry. These four are the whole notification surface of the skills watcher -- modify, delete, rename, debounce -- so while they are listed, Windows carries no coverage of a watcher losing its directory. Recorded here rather than left implicit."
---

# Filed from the fold-in run, not the original outage window

Bringing wayland-core#433 into wayland-core#432 produced a new tree, and a new
tree is a new sample. The retry-flake gate can only ever report whichever
member fires, so this cluster was invisible until then.
