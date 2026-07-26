# 25-01 — Success Criterion 1 equivalence evidence

**Verdict: Success Criterion 1 is NOT MET.**

Three of the four reference backends ran the same deterministic task through the shipped
`wayland-core` binary, produced receipts that verify individually, and diffed to **EQUIVALENT**.
The fourth — the hibernating cloud backend — is **UNEXERCISED** because no vendor credential
exists on any proof host. The criterion names four surfaces, so three out of four is NOT MET, and
this document says so rather than grading the part that passed.

---

## 1. What was run, and where

| | |
|---|---|
| Host | `hetzner-dsm` (`Ubuntu-2404-noble-amd64-base`), the authoritative Linux proof host |
| Date | 2026-07-26 |
| Binary | `./target/release/wayland-core`, built `--release` on that host from the phase worktree `/root/wayland-p25` |
| Surface driven | `wayland-core backend run` / `cancel` / `orphans` / `receipt verify` / `diff` — the shipped operator surface and nothing else |
| Transcript | `evidence/25-01/live-equivalence-transcript.txt` |

Nothing in this file was produced by calling a library from a test. The regression form of the
same claim is `crates/wcore-exec-backend/tests/live_equivalence.rs`, gated behind
`--run-ignored only` so the cheap CI floor never dials a vendor.

## 2. The deterministic task

One definition, byte-identical on every backend **including its id and nonce**:

- `task_id` `f25-reference`, `nonce` `f25-reference-nonce`
- workspace: `README.txt` (45 bytes) + `data/fixed.txt` (17 bytes)
- input: `wayland-f25-deterministic-input\n` (32 bytes)
- argv: `["cat", "input.bin"]` — a computation whose output is a function of the input alone
- artifact: `stdout.bin`, which IS the captured stdout, content-addressed
- budget: cpu 30 000 ms, memory 256 MiB, wall 60 000 ms, output 1 MiB

Determinism is the whole point. A task whose output embedded a timestamp, a hostname or a random
value could not prove equivalence at all.

## 3. The four receipts

| backend | run | artifact sha256 | input sha256 | workspace sha256 | events | terminal |
|---|---|---|---|---|---|---|
| local | PASS | `55eae7c64c240e93…` | `55eae7c64c240e93…` | `9c743b3c399187d9…` | 4 | success |
| container | PASS | `55eae7c64c240e93…` | `55eae7c64c240e93…` | `9c743b3c399187d9…` | 4 | success |
| ssh | PASS | `55eae7c64c240e93…` | `55eae7c64c240e93…` | `9c743b3c399187d9…` | 4 | success |
| **cloud** | **UNEXERCISED** | — | — | — | — | — |

Full receipts: `evidence/25-01/receipt-local.json`, `receipt-container.json`, `receipt-ssh.json`.
Each was verified with `wayland-core backend receipt verify` — body digest, event ordering, single
terminal event and internal consistency all hold on all three, with three DIFFERENT attestation
keys (`4349b5b4…`→`6d25df6d…` local, `4dcd86f4…` container, `60f51cec…` ssh; the keys are minted
per backend per host, so they differ between runs).

`receipt verify` states in plain words that it does **not** establish identity. That is not a
weakness of the command, it is a property of receipts: a receipt cannot authenticate itself,
because verifying identity needs a verifying key the caller already pinned. The conformance
harness does hold the pinned key and does verify identity, and it rejects both a tampered body and
an unpinned identity.

## 4. The normalized diff

```
NORMALIZED DIFF: EQUIVALENT
```

Full artifact: `evidence/25-01-normalized-diff.txt`.

The fields that differed, all of them excluded from the normalized body **by design and reported
alongside it**:

| field | local | container | ssh |
|---|---|---|---|
| `backend.backend_id` | local | container | ssh |
| `backend.key_id` | `6d25df6d…` | `4dcd86f4…` | `60f51cec…` |
| `transport.kind` | local | container | ssh |
| `transport.endpoint` | `localhost` | `wayland-f25-f25-reference` | `f25-ssh-target` |
| `timing.wall_ms` | 1 | 20 999 | 265 |
| `hibernation` | not_applicable | not_applicable | not_applicable |

`backend.instance_id` was identical (`e726e46d…`) because all three ran on the same host — which
is itself honest evidence that the SSH leg did not reach a different machine (see §6).

**No other field differed.** Task digests, resource budget, backend ceiling, event ordering and
content digests, artifact digest, terminal status, exposed-secret names and the egress decision all
agreed. Any other difference would have been a finding, and the diff command says so and exits
non-zero.

The normalization refuses to be generous. A unit test asserts that two receipts differing only in
their input digest still diverge after normalization — over-normalizing until four receipts compare
equal by construction is the forbidden move here, and it is guarded rather than promised.

## 5. Cancellation and cleanup

Transcript: `evidence/25-01/cancellation-transcript.txt`. The same long task (`sleep 120`,
nonce `f25-cancel-nonce`) was started on each backend through the shipped binary and cancelled
**from a second process** six seconds in.

| backend | terminal status | cleanup method | residual | real process table afterwards |
|---|---|---|---|---|
| local | `Cancelled { reason: "operator cancelled" }` | SIGKILL to the child's own process group, then re-read the process table | none | `no matching local process` |
| container | `Cancelled { … }` | `docker rm -f`, then `docker ps -a --filter label=wayland.task.nonce=<nonce>` re-read | none | no container carries the nonce |
| ssh | `Cancelled { … }` | second ssh connection kills the remote session (`remote-kill-issued`), then the remote process table is re-read | none | `no matching REMOTE process` |
| cloud | — | — | — | UNEXERCISED |

The post-cancel orphan scan reported `found=0` on all three exercised backends and
`enumerated=false` on cloud. That last distinction is load-bearing: an un-enumerated surface is not
a clean surface, and reporting zero orphans because the scan could not run is exactly how an orphan
hides.

Pre-cancel, the same scan found the live work — 1 on local, 1 on container, 2 on ssh — so the
scanner is measured to be capable of finding something, not merely of returning zero.

## 6. What is UNEXERCISED, and exactly what closes it

**Cloud leg — UNEXERCISED.**
Reason: no Fly credential exists on any proof host. Measured 2026-07-26 on hetzner-dsm:
`WAYLAND_F25_CLOUD_TOKEN` absent, `WAYLAND_F25_CLOUD_ORG` absent, `~/.fly/config.yml` absent.
The backend is implemented, routes all HTTP through `wcore-egress`, and **fails closed**:

```
wayland-core backend: backend 'cloud' is unavailable and this command does NOT fall back:
backend cloud has no credential (WAYLAND_F25_CLOUD_TOKEN); refusing to run and NOT falling back
```

Command that closes it, once Sean mints a token scoped to one throwaway Fly org:

```
ssh hetzner-dsm 'cd /root/wayland-p25 && \
  WAYLAND_F25_CLOUD_TOKEN=<fly deploy token> WAYLAND_F25_CLOUD_ORG=<throwaway app slug> \
  ./target/release/wayland-core backend run --backend cloud --receipt-out /tmp/f25-cloud.json'
```

**SSH leg — RAN, but against a containerized sshd on the same physical host.**
The target was a disposable `wayland-f25-sshd` container on hetzner-dsm: a separate network
namespace, filesystem and process table, reached over a real ssh connection with a real key. It
proves the transport, the argv-mode argument handling and remote-session cancellation. It does
**not** prove the cross-machine case, and `backend.instance_id` being identical across all three
receipts is the evidence of that. Reported here rather than presented as a cross-host run.
`SeanD@seandesktop` was reachable but is Windows, which has no `setsid`, no POSIX `cat` and no
`base64` on the default path, so it is not a valid target for this remote runner.

**Windows leg — NOT RUN.** No `wayland-core` build exists on `SeanDesktop` in this window, and the
one cargo build attempted there was not completed. Recorded as unexercised rather than inferred
from the Linux result.

## 7. Defects this live exercise found

Every one of these was invisible to a green test suite and was found only by driving the real
binary. All five are fixed and re-proved.

1. **Task children inherited stdin.** The first task consumed the rest of the operator script that
   was feeding the caller's stdin, and the run silently stopped after `backend list`. Now
   `Stdio::null` on local and container.
2. **The remote orphan scanner found itself.** The nonce travels on the scan script's own argv, so
   `ps | grep <nonce>` matched `sh -s -- <nonce>`. Every scan reported one orphan that did not
   exist. Self and children are now excluded by pid.
3. **The remote killer killed itself.** `pkill -f <nonce>` matched the killer's own argv, so it
   died mid-run and reported `remote kill failed:` with an EMPTY stderr — while the work had in
   fact died. A cleanup that reports failure when it succeeded trains the reader to ignore it.
4. **The remote scan could not see the real work.** The task's own argv (`sleep 120`) carries no
   nonce, so a genuine orphan would have been INVISIBLE. The runner's recorded session leader is
   now the primary signal, with the `ps` sweep secondary.
5. **The first equivalence run compared four different tasks.** The task id was suffixed per
   backend, so the diff correctly reported DIVERGENT on `task` and `events` while every content
   digest matched. The reference task now has one fixed id.
