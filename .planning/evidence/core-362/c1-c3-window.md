# core#362 c1–c3 — the ENOENT window, named and measured on both shapes

Lane `lane/f13-s2-linux-sandbox`, base `ca15a48bf`. Host `hetzner-dsm`
(Ubuntu 24.04 noble, kernel 6.8.0-101-generic, 96 cores, bubblewrap 0.9.0).
CI-image arm: `rust:1.95-slim-bookworm` + the workflow's own package list
(bubblewrap 0.8.0), run with `.github/workflows/ci.yml`'s `DOCKER_RUN_SANDBOX`
posture verbatim — `--init --network=host --cap-add SYS_ADMIN --security-opt
seccomp=unconfined --security-opt apparmor=unconfined --security-opt
systempaths=unconfined`.

---

## c1 — the named path and the named window

| role | what |
|---|---|
| **what resolves it** | `bwrap` writes `{"child-pid": N}` to `--json-status-fd`; `read_bwrap_child_pid` (`crates/wcore-sandbox/src/backends/bwrap.rs:931`) deserialises `N` |
| **what opens it** | `ProcessTreeGuard::from_observed_root(N)` → `LinuxProcessIdentity::open(N)` → `linux_process_start_time(N)`, which is `std::fs::read_to_string("/proc/{pid}/stat")` (`process_tree.rs:1498`) |
| **the named path** | **`/proc/<child-pid>/stat`** |
| **what removes it in between** | **bubblewrap itself.** `N` is not our child — it is bwrap's sandboxed PID-namespace init (`--unshare-all` implies `--unshare-pid`), and bwrap `waitpid()`s and reaps it the instant it exits. Reaping frees `/proc/<N>` |
| **the named window** | between bwrap writing that JSON status line and `linux_process_start_time` reading `/proc/<N>/stat` |

Why the window exists here and nowhere else in this file: `ProcessTreeGuard::new`
opens a DIRECT child the caller has not reaped, and a zombie keeps its `stat`
file, so `/proc/<pid>` is guaranteed. `from_observed_root` opens somebody
else's child whose parent reaps it immediately, so it is not.

**MEASURED, not argued.** An instrument was injected at exactly that point
(`WCORE_C362_WINDOW_MS`, a sleep plus a `/proc/<pid>` existence probe). Every
failing trial printed

```
C362-PROBE child_pid=587542 /proc/587542 exists=false
```

i.e. the path named above was gone at probe time, in the window named above.
Not one failing trial reported `exists=true`, and not one passing trial
reported `exists=false`.

---

## c3 — the red arm, verbatim, before the fix

Tree: `ca15a48bf` with `from_observed_root` reverted to its pre-`f07482c80`
semantics (an absent observed root propagates its error). `cargo check
-p wcore-sandbox --tests` RC=0 with the mutation applied, so the red below is
behaviour and not a build break. Plain host, `WCORE_C362_WINDOW_MS=1000`,
`--retries 0`:

```
C362-PROBE child_pid=587542 /proc/587542 exists=false
thread 'bwrap_confines_filesystem_writes_outside_allowlist' (587539) panicked at crates/wcore-sandbox/src/test_support.rs:235:13:
the sandbox backend refused to run the containment probe, so no containment property was tested: ExecFailed("sandbox process-tree ownership: No such file or directory (os error 2)")
test bwrap_confines_filesystem_writes_outside_allowlist ... FAILED

C362-PROBE child_pid=588027 /proc/588027 exists=false
thread 'bwrap_execute_echo_returns_exit_zero' (588024) panicked at crates/wcore-sandbox/tests/backend_integration.rs:210:10:
bwrap execute must succeed for a trivial command: ExecFailed("sandbox process-tree ownership: No such file or directory (os error 2)")
test bwrap_execute_echo_returns_exit_zero ... FAILED

     Summary [   2.118s] 2 tests run: 0 passed, 2 failed, 6 skipped
```

Both messages match CI run 33240249894's, including the two source positions
(`test_support.rs:235:10`/`:13` and `backend_integration.rs:210:10`).

**GREEN ARM, same injected condition, fixed tree** — the probe still reports
`/proc/<pid>` gone, so the condition really was reproduced and not merely
absent:

```
C362-PROBE child_pid=959764 /proc/959764 exists=false
        PASS [   1.010s] (1/2) bwrap_confines_filesystem_writes_outside_allowlist
C362-PROBE child_pid=959898 /proc/959898 exists=false
        PASS [   1.010s] (2/2) bwrap_execute_echo_returns_exit_zero
     Summary [   2.020s] 2 tests run: 2 passed, 6 skipped
```

The containment test PASSES rather than skipping: it ran its probe and its
markers, which is the property `run_contained_probe` refuses to fake.

Same green arm in the CI image, n=10 per window, fixed tree:

```
== GREEN-ci ==  nproc=96 bwrap=bubblewrap 0.8.0
WINDOW 1000ms: 0/10 trials failed; 20 probe(s) saw /proc/<child-pid> already gone
WINDOW   10ms: 0/10 trials failed; 10 probe(s) saw /proc/<child-pid> already gone
```

The probe counts are the point: at both windows the race CONDITION was present
on every trial that could have it, and no trial failed. Compare the same cells
on the pre-fix tree — 10/10 and 7/10 failed.

---

## c2 — does it reach a plain Linux host, or only CI's nested shape?

Trials of the two bwrap tests at `--retries 0`, n=10 per cell, pre-fix tree.

| injected window | plain Linux host | CI image + the four grants |
|---|---|---|
| 1000 ms | **10/10 failed** | **10/10 failed** |
| 100 ms | **10/10 failed** | **10/10 failed** |
| 50 ms | 0/10 | **10/10 failed** |
| 30 ms | 0/10 | **10/10 failed** |
| 10 ms | 0/10 | **7/10 failed** |
| 1 ms | 0/10 | — |

Natural contention, no injection, n=25 per shape (trials and 8 concurrent
bwrap noise loops all pinned to CPUs 0,1):

| | plain Linux host | CI image |
|---|---|---|
| natural | 0/25 | 0/25 |

**VERDICT: it reaches a plain Linux host. It is NOT specific to the nested
shape.** The failing path is `/proc/<pid>/stat` on the host's own procfs, the
code is identical on both, and on a plain host it fires 10/10 once the window
reaches 100 ms.

**Severity follows from the second half of the table.** The nested shape is
susceptible at a window an order of magnitude narrower — 10 ms fires 7/10 in
the container and 0/10 on the host — so CI meets this far more often than a
customer does, which is consistent with every observed natural occurrence
being on the containerized leg. For a user on a plain Linux host with the
sandbox enabled the symptom is a Bash command intermittently failing with
`sandbox process-tree ownership: No such file or directory`, at a rate this
harness could not resolve: 0/25 bounds it below roughly 11 % (one-sided 95 %),
which is not the same as zero and is why c3 was not treated as optional.

**Confounds, stated.** The two shapes differ in more than nesting: bubblewrap
0.9.0 vs 0.8.0 (the bookworm package), and the container's masked-`/proc`
posture is restored by `--security-opt systempaths=unconfined` rather than
being native. Both arms ran on the same 96-core host with the same trial
script, the same pinning and the same injection point, so the comparison is
controlled for everything else. A GitHub-hosted 4-core runner is more
contended than either arm here, which is the direction that makes the natural
CI rate higher than anything measured on this box.
