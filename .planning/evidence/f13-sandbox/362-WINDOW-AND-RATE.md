# core#362 — the ENOENT window, measured in both shapes, and the rate after the fix

Lane `sandbox`, branch `lane/f13-sandbox`, host `hetzner-dsm`, 2026-08-31.
Raw arms behind `.planning/ledger/wayland-core-362.md` c2, c3 and c5.

## What was varied

`ProcessTreeGuard::from_observed_root` reads `/proc/<child-pid>/stat` for the pid
bubblewrap reports over `--json-status-fd`. Bubblewrap reaps that child the
instant the command exits, so there is a window between the status line and our
read; losing it returned a command that had SUCCEEDED to the caller as
`ExecFailed("sandbox process-tree ownership: No such file or directory (os error 2)")`.

Two trees:

* **unfixed** — `git checkout f07482c80^ -- crates/wcore-sandbox/src/backends/process_tree.rs crates/wcore-sandbox/src/backends/bwrap.rs`.
  `cargo check -p wcore-sandbox --tests` RC=0 before any arm was believed.
* **fixed** — the same two files at `HEAD`.

One instrument, identical in both trees: a temporary env-gated
`std::thread::sleep(WCORE_TEST_OWNERSHIP_DELAY_MS)` inserted immediately after
`read_bwrap_child_pid` (bwrap.rs:686). It WIDENS the window and changes nothing
else. It was removed afterwards and the tree verified clean (`git diff HEAD`
empty, `grep -c WCORE_TEST_OWNERSHIP_DELAY_MS` = 0) before any green below was
recorded.

Two shapes:

* **plain host** — hetzner-dsm, bubblewrap 0.9.0, no container.
* **CI image** — `wayland-core-ci:rust-1.95-slim-bookworm` (bubblewrap 0.8.0)
  under the real `DOCKER_RUN_SANDBOX` grants: `--init --network=host
  --cap-add SYS_ADMIN --security-opt seccomp=unconfined --security-opt
  apparmor=unconfined --security-opt systempaths=unconfined`.

Test: `bwrap_execute_echo_returns_exit_zero`, 5 executions per point.

## The window — UNFIXED tree

    plain host   0ms 0/5   25ms 0/5   50ms 0/5   75ms 0/5
                 100ms 2/5  125ms 5/5  150ms 5/5  175ms 5/5  200ms 5/5

    CI image     0ms 0/5    3ms 0/5    5ms 0/5   10ms 0/5
                 15ms 2/5   20ms 5/5   25ms 5/5   50ms 5/5   75ms 5/5
                 100ms 5/5 125ms 5/5  150ms 5/5  175ms 5/5  200ms 5/5

**It reaches a plain Linux host.** It is not an artefact of CI's
nested-bwrap-in-docker shape; that shape is roughly 5x more exposed (15 ms
against 100 ms), which is why CI is where it was first seen.

## The window — FIXED tree, same instrument, same shapes

    plain host   0ms 0/5  100ms 0/5  125ms 0/5  200ms 0/5  500ms 0/5  2000ms 0/5
    CI image     0ms 0/5   15ms 0/5   20ms 0/5  100ms 0/5  500ms 0/5  2000ms 0/5

The fix is the only variable between the two tables.

## The verbatim red arm (unfixed tree, plain host, 200 ms)

    thread 'bwrap_confines_filesystem_writes_outside_allowlist' panicked at
      crates/wcore-sandbox/src/test_support.rs:235:13:
    the sandbox backend refused to run the containment probe, so no containment
    property was tested: ExecFailed("sandbox process-tree ownership: No such
    file or directory (os error 2)")

    thread 'bwrap_execute_echo_returns_exit_zero' panicked at
      crates/wcore-sandbox/tests/backend_integration.rs:210:10:
    bwrap execute must succeed for a trivial command: ExecFailed("sandbox
    process-tree ownership: No such file or directory (os error 2)")

    test result: FAILED. 0 passed; 2 failed; 0 ignored; 0 measured; 6 filtered out

Both file:line anchors are the ones CI run 33240249894 reported.

## The natural rate — UNFIXED tree, no instrument. NEGATIVE RESULT

Recorded because a negative result that is dropped becomes a claim nobody made.

| arm | executions | ENOENT |
|---|---|---|
| CI image, `nextest --retries 0`, the two named tests | 25 runs | 0 |
| CI image, whole `wcore-sandbox` crate, `--cpuset-cpus=0,1` | 5 runs | 0 |
| plain host, one binary pinned to ONE CPU, 24 concurrent × 10 rounds | 240 | 0 |
| CI image, one binary pinned to ONE CPU, 24 concurrent × 10 rounds | 240 | 0 |

Every arm carries a positive control that each execution actually RAN the test.
That control was added because the first version of this harness was **vacuous**:
`taskset -c 0` inside a container started with `--cpuset-cpus=3` fails with
`Invalid argument` and launches nothing, and 240 non-executions read exactly like
240 clean passes.

CPU starvation is not the trigger, and there is a mechanism for that: starving
the CPU slows the sandboxed child by as much as it slows our `/proc` read, so it
widens both sides of the window at once. The doc comment on `from_observed_root`
previously claimed reproduction "by pinning concurrent bwrap execs onto two
CPUs"; that is corrected in the same change as this file.

## The rate AFTER the fix (ledger c5)

`cargo nextest run -p wcore-sandbox --profile ci --retries 0`, CI image, real
grants, tree at `ede0ceaca` with `git diff HEAD` empty:

    the two named tests, N=25   ->  25 pass, 0 fail   (0.0%)
    whole crate, one run        ->  Summary [33.229s] 210 tests run: 210 passed, 11 skipped

A first N=25 pass returned 22/3. All three failures were this lane's own red-arm
mutation landing in the tree while the loop was still running. The tree was
re-verified clean and the loop re-run before the number above was taken.

## The retry behaviour (ledger c4)

A probe refusal was forced (a panic carrying the real message inserted into
`run_contained_probe`) and the same test run under `--profile ci` with NO
`--retries` override, in both directions:

    pin PRESENT   FAIL [0.166s] (1/1) ... bwrap_confines_filesystem_writes_outside_allowlist
                  Summary [0.168s] 1 test run: 0 passed, 1 failed
    pin REMOVED   TRY 1 FAIL / TRY 2 FAIL / TRY 3 FAIL ... same test
                  Summary [0.418s] 1 test run: 0 passed, 1 failed

One attempt with the pin, three without it.
