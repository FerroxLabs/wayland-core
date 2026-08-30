# core#362 — the bwrap process-tree ownership race, measured

Lane `lane/f13-w2-sandbox-linux`, wave 2 of the 0.13.12 cut. Host: `hetzner-dsm`,
Ubuntu 24.04, kernel 6.8.0-101-generic, 96 cores, bubblewrap 0.9.0.

The fix itself is NOT this lane's: it landed in the integration base as
`f07482c80 fix(sandbox,tools): root-cause the two retry-flake families reddening
the gate`. What was missing was c2 and c5 — the ticket asks for the race to be
characterised BY MEASUREMENT, and the base commit reproduced it only by injecting
a one-second deschedule. "It reproduces when I stop the thread for a second" does
not tell anyone whether a user will ever see it.

---

## The window, exactly

`crates/wcore-sandbox/src/backends/bwrap.rs` reads bubblewrap's `--json-status-fd`
line for `child-pid`, then opens `/proc/<child-pid>/stat` through
`LinuxProcessIdentity::open` inside `ProcessTreeGuard::from_observed_root`.

- **resolved**: the pid on bubblewrap's status line.
- **opened**: `/proc/<pid>/stat`, plus `pidfd_open(pid)`.
- **removed in between**: bubblewrap itself, which reaps its PID-namespace init
  the instant it exits. The observed root is SOMEBODY ELSE'S child, which is why
  the `/proc/<pid>` guarantee `ProcessTreeGuard::new` relies on — a direct,
  unreaped child keeps its `stat` file even as a zombie — does not hold here.

## How wide the window is: measured, not argued

A temporary probe (not committed) was inserted immediately after the status line
was read, reporting whether `/proc/<child-pid>` still existed at the instant of
the open, plus an injectable stall before it. Two bwrap arms of
`wcore-sandbox::backend_integration`, `--test-threads=1`, `/bin/echo hello`:

| injected stall | `/proc/<child>` present at the open | pre-fix run |
|---|---|---|
| 0 ms  | 10/10 present | pass |
| 1 ms  | 10/10 present | pass |
| 2 ms  | 10/10 present | pass |
| 5 ms  | 10/10 present | pass |
| 10 ms | 10/10 present | pass |
| 50 ms | 10/10 present | pass |
| 75 ms | 6/6 ABSENT | — |
| 100 ms | 6/6 ABSENT | — |
| 125 ms | 6/6 ABSENT | — |
| 150 ms | 6/6 ABSENT | — |
| 200 ms | 10/10 ABSENT | FAIL |
| 1000 ms | 10/10 ABSENT | FAIL |

**The race needs the reading thread stalled for somewhere between 50 ms and 75 ms
after bubblewrap's status line.** That is the number severity turns on, and it is
a large stall — far more than an ordinary deschedule.

## c2 — does it reproduce naturally? Measured on both shapes. No.

Both arms are the SAME source tree, differing only in
`observed_root_is_gone` (the fix). Binaries fingerprinted by sha256 so the two
arms are known to be different builds and not the same one twice:

- pre-fix `backend_integration`: `3996d8b050c4adbe…` (host), `395a39c11c7bcc65…` (CI image)
- fixed   `backend_integration`: `a64cc7cf4357b28f…` (host), `26d03683ce7566f8…` (CI image)

| shape | arm | executions | ENOENT |
|---|---|---|---|
| plain host, 12-way on 2 cpus | fixed | 60 | 0 |
| plain host, 12-way on 2 cpus | pre-fix | 60 | 0 |
| plain host, 24-way on 1 cpu | fixed | 240 | 0 |
| plain host, 24-way on 1 cpu | pre-fix | 240 | 0 |
| plain host, 32 spinners saturating 1 cpu | pre-fix | 60 | 0 |
| CI image + the `ci-linux` security-opts, 24-way on 1 cpu | pre-fix | 240 | 0 |
| CI image, `cargo nextest run --retries 0` x25 | pre-fix | 25 runs | 0 |
| CI image, `cargo nextest run --retries 0` x25 | fixed | 25 runs | 0 |

Direct probe of the deciding condition under the spinner-saturated arm:
`/proc/<child>` was present **120 times out of 120**.

**The ticket's own hypothesis is refuted.** It asked whether the race is specific
to "CI's nested-bwrap-in-docker shape". Reproducing the nesting exactly — same
image, same `--cap-add SYS_ADMIN --security-opt {seccomp,apparmor,systempaths}=unconfined`,
same `WCORE_REQUIRE_ENFORCING_SANDBOX=1` — produced zero occurrences. Nesting is
not the discriminator. **Scheduling pressure is**, and the only environment that
has ever exhibited it is a 4-core GitHub-hosted runner executing the whole
12,000-test workspace in parallel.

**Severity.** A real product defect: a command that RAN TO COMPLETION is returned
to the caller as `ExecFailed`, so a user's Bash command fails on Linux with the
sandbox enabled for no reason the user can act on. Its natural rate outside a
saturated small-core runner is below the resolution of ~600 executions across
both shapes on this host. It is worth fixing — it is fixed — and it is not worth
holding a release for.

## c3 — the red arm, quoted verbatim from before the fix

Mutation M1, on executable code in
`crates/wcore-sandbox/src/backends/process_tree.rs`:

    -    error.raw_os_error() == Some(libc::ESRCH) || error.kind() == std::io::ErrorKind::NotFound
    +    let _ = error; false // M1 RED ARM: pre-fix behaviour, every open error propagates

With the window held open (200 ms), the mutated tree reproduces the CI failure
byte for byte — both tests, both panic sites:

    thread 'bwrap_confines_filesystem_writes_outside_allowlist' panicked at crates/wcore-sandbox/src/test_support.rs:235:13:
    the sandbox backend refused to run the containment probe, so no containment property was tested: ExecFailed("sandbox process-tree ownership: No such file or directory (os error 2)")

    thread 'bwrap_execute_echo_returns_exit_zero' panicked at crates/wcore-sandbox/tests/backend_integration.rs:210:10:
    bwrap execute must succeed for a trivial command: ExecFailed("sandbox process-tree ownership: No such file or directory (os error 2)")

The fixed tree, same 200 ms window, same binary path, passes both.

## c5 — rate at `--retries 0`, N >= 20, on the CI image

Image built from the `ci.yml` `ci-linux` Dockerfile verbatim
(`rust:1.95-slim-bookworm` + libdbus/libseccomp/libssl/libasound/pkg-config/mold/
ca-certificates/git/python3/procps/bubblewrap/curl + cargo-nextest + cargo-audit),
run under that job's own `docker run` flags.

    cargo nextest run -p wcore-sandbox \
      -E 'binary(=backend_integration) and test(/^bwrap_/)' --retries 0

**25 runs, 25 passed, 0 failed, 0 ENOENT — a rate of 0/25.** The same 25 on the
pre-fix arm: also 0/25, which is the c2 result and the reason the rate alone does
not settle severity.

## c4 — the interim allowlist

The ticket says two SHORT-DATED entries in `.config/flaky-allowlist.txt` name it
and should be deleted when c3/c4 land. There are none: the file carries no `362`
and neither test name appears in it. They were never merged into this line of
history. Nothing to delete; the pin is now a `retries = 0` override instead,
which is what that file's own header prescribes for a flake that IS the bug.
