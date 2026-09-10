# Load-conditioned flake rates — hetzner-dsm, 2026-09-10 (lane w15/loadD)

Every rate here carries the 1-minute loadavg it was measured at. A rate with no
load figure cannot be separated from ambient noise, and all four findings below
are load-conditioned by their own tickets.

## Host and instruments

* Host `hetzner-dsm`: 96 cores, 251 GiB RAM. Ambient loadavg at session start
  29.50 (other lanes' builds).
* Cargo runs go through `tools/remote-proof.py`, slot `default`. A run counts
  ONLY when its receipt carries `"complete": true` with the expected
  `remote_exit`. Ten attempts in this session returned `complete: false` — nine
  `Proof slot busy; no build ran` and one `checkout is not clean` — those never
  executed and are excluded from every denominator rather than counted as
  passes.
* Synthetic contention: N busy-loop processes on the host, each under its own
  `timeout`, PIDs recorded so they are stopped by PID and never by pattern.
  DECLARED AS FORCED: this is manufactured runqueue pressure, not another lane's
  real work, and it carries almost no memory or page-cache pressure.
* Containerised leg: `wayland-core-ci:rust-1.95-slim-bookworm`, built on
  hetzner-dsm from the inline Dockerfile in the `Build CI image` step of
  `.github/workflows/ci.yml` — the workflow's own `CI_IMAGE` tag. Identified
  from inside the image: `rustc 1.95.0 (59807616e 2026-04-14)`, `cargo 1.95.0`,
  `cargo-nextest 0.9.143`, `PRETTY_NAME="Debian GNU/Linux 12 (bookworm)"`.
  Invoked with the workflow's own `DOCKER_RUN_SANDBOX` grants (`--cap-add
  SYS_ADMIN`, `seccomp=unconfined`, `apparmor=unconfined`,
  `systempaths=unconfined`). Source materialised by `git archive 14aa2f1ca`.

Commits: `335328967` = pre-fix; `14aa2f1ca` = pre-fix plus the wayland#1245 fix.
`git diff --stat 335328967 14aa2f1ca` is ONE file,
`crates/wcore-cli/tests/migrate_quarantine.rs`, 58 insertions / 11 deletions.
Nothing wcore-agent or wcore-exec-backend compiles from differs between them,
which is what licenses pooling the wayland#1282 and wayland#1240 samples across
both commits.

## wayland#1245 — `t19_live_negative_leg_quarantined_payload_does_not_execute`

### Pre-fix (335328967) — 20 observations, 0 failures

| instrument | n | 1-min loadavg | result |
|---|---|---|---|
| `nextest -p wcore-cli --test migrate_quarantine -E t19+t20` | 1 | 29.50 | PASS, t19 45.324 s |
| same, + 140 spinners | 4 | 167.04–168.20 | 4/4 PASS, t19 45.547–45.762 s |
| `cargo test -p wcore-cli --test migrate_quarantine` (49 tests, ONE process) | 8 | 179.10–207.08 | 8/8 PASS 49/49, binary 52.50–56.05 s |
| `nextest -p wcore-cli --profile ci --retries 0` (3893 tests) | 1 | 74.4–101.5 | PASS, t19 45.547 s |
| `nextest --workspace --profile ci --retries 0 --no-fail-fast` (18118 tests) | 1 | 44.6–62.9 | PASS, t19 45.521 s |
| same, + 100 spinners | 5 | peak 174.99–189.55 | 5/5 PASS, t19 45.752–45.829 s |

**Measured failure rate 0/20, loadavg 29.50–207.08.** The issue body's "fails 3/3
above loadavg ~150" and `scripts/check-test-env-globals.py`'s "at load ~130 it is
1 pass / 6 fail under `cargo test`" were NOT reproduced by any of these.

The load reached the test, controlled rather than asserted: the same full-suite
run takes 87 s unloaded and 186–223 s loaded, and t20 — the positive leg driving
the SAME binary through the SAME turn — goes from 2.5 s to 6.4–6.9 s. The child
is genuinely starved, just not the ~9x it would take to overrun 45 s.

### Post-fix (14aa2f1ca) — 30 observations, 0 failures

| instrument | n | 1-min loadavg | t19 | t20 (positive leg) |
|---|---|---|---|---|
| `nextest ... -E t19+t20` | 1 | 25.99 | 2.913 s | 2.550 s |
| same, + 100 spinners | 5 | 180.14–182.06 | 7.485–7.769 s | 6.420–6.814 s |
| `cargo test -p wcore-cli --test migrate_quarantine` | 8 | 180.71–191.14 | PASS 49/49, binary 15.30–18.04 s | — |
| `nextest --workspace --profile ci --retries 0` + spinners | 16 | peak 174.99–199.09 | 7.413–8.899 s | — |

t20 pre-fix: 2.537 s at loadavg 29.50, 6.374–6.678 s at loadavg 167–168.
t20 post-fix: 2.550 s at loadavg 25.99, 6.420–6.814 s at loadavg 180–182.
The positive leg is unchanged at both loads.

## wayland#1282 — `dangerous_expiry_cancels_production_streaming_bash_process_tree`

Sampled inside `cargo nextest run --workspace --profile ci --retries 0
--no-fail-fast` (18118 tests, one concurrent full-suite run per trial) with 100
spinners on top. 5 trials at 335328967 and 16 at 14aa2f1ca; the test, its crate
and every crate it compiles from are byte-identical across the two.

| trials | peak 1-min loadavg per trial | result |
|---|---|---|
| 21 | 174.99–199.09 | 21/21 PASS, 6.457–7.916 s in-test |

One further full-suite trial without spinners (peak loadavg 62.90) also passed,
6.305 s.

**Measured failure rate 0/21 at `--retries 0` under concurrent full-suite load,
peak loadavg 175–199.** `read_pid` never lost its 4 s `TREE_UP_BUDGET` — the
payload the flaky-allowlist entry quotes — in any of them.

LIMIT: measured on bare hetzner-dsm, NOT inside the containerised CI leg.

## wayland#1240 — `await_completion_returns_on_match`

On the containerised CI image, `cargo test -p wcore-agent --lib --no-fail-fast`
— the shared-process leg, 2733 tests in ONE process. `cargo test` has no retry
mechanism, so every run is `--retries 0` by construction.

| trials | 1-min loadavg | result |
|---|---|---|
| 20 | 142.69–198.23 | 20/20 `ok. 2733 passed; 0 failed`, 29.77–32.89 s |

**KNOWN-POSITIVE CONTROL, same image, same command, same mounted tree.** With the
`bus.publish(AgentMessage::Completed {..})` call deleted from the test body
(`diff` confirmed the six removed lines were CODE, not a comment):

```
test agents::observer::tests::await_completion_returns_on_match ... FAILED
thread '...' panicked at crates/wcore-agent/src/agents/observer.rs:370:9:
waiter must resolve on the matching Completed event, got Err(Timeout)
test result: FAILED. 2732 passed; 1 failed; 3 ignored
```

Restored byte-identical (`diff` clean) and `touch`ed so cargo could not skip the
rebuild, then re-run in the same image: `ok. 2733 passed; 0 failed`. So the 20/20
above is a live instrument, not a dead one.

Also sampled in the 21 workspace nextest runs (process-per-test, which cannot see
this class by construction): 21/21 PASS, 0.052–0.100 s, peak loadavg 175–199.

## wayland#1250 — `conformance_matrix` shared-process interleaving

`cargo test -p wcore-exec-backend --test conformance_matrix` — plain `cargo
test`, never nextest, because nextest gives each test its own process and the
whole hazard class is invisible under it.

| arm | trials | 1-min loadavg | result |
|---|---|---|---|
| pre-fix `temp_state()` restored verbatim (process global), this lane | 10 | 179.72–183.74 | 10/10 `2 passed; 0 failed`, 2.28–2.90 s |
| same mutation, earlier pass | 20 | 26–29 | 20/20 `2 passed; 0 failed`, 0.93–1.05 s |
| fixed tree (`StateDirGuard`), this lane | 5 | 129.39–138.32 | 5/5 `2 passed; 0 failed`, 2.17–2.33 s |

**The `1 passed / 1 failed` signature was never observed: 0/30 across both
passes.**

THE OVERLAP CONTROL DID NOT CARRY AT LOAD, and that is recorded rather than
glossed. At loadavg 26–29 the earlier pass separated parallel (0.93–1.05 s) from
`--test-threads=1` (1.26 s), which is what proved the two tests genuinely
overlap. Repeating it on the mutated tree at loadavg ~130 gave 2.66 s and 2.84 s
serial against 2.28–2.90 s parallel — overlapping ranges, no separation, so at
this load the control establishes nothing about the width of the interleaving
window. The 0/30 therefore rests on the earlier pass's overlap control, not on a
fresh one.
