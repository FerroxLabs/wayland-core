# `REACH-*` C1 / C2 / C4 — lane `reach-c1-c2` SUMMARY

| | |
|---|---|
| **Lane** | `lane/reach-c1-c2` |
| **Base / merge-base** | `0d48b5515b816e8930456fcda7c91c0ec9a46ebd` (integration head) |
| **Hosts** | controller `hetzner-dsm`; ssh far end = container `reachc1c2-sshd`; cloud = real Fly machines, app `wayland-f25-test`; `SeanD@seandesktop` reached for the C2 trust measurement only |
| **Binary** | `wayland-core 0.12.25`, built at `0d48b551`, sha256 `1621393b713f7c81…c357f1a3` |
| **Evidence** | `evidence/reach-c1-c2/` |

---

## Verdict

**All three criteria the ledger records as blocked on Sean are unblocked, and none of the three
blockers was ever real.** Two were already known false on 2026-07-28 and were written into the
ledger anyway. My deliverable turned out to be **independent re-verification at the current
integration head plus a ledger correction**, not the legwork the brief anticipated.

| criterion | ledger said | measured at `0d48b551` |
|---|---|---|
| **C1** cloud leg | "unexercised for want of a credential only Sean can mint" | **RUN.** Credential live, four surfaces `EQUIVALENT` in one invocation |
| **C2** second physical host | "no SSH trust exists … creating one is reserved to Sean" | **TRUST EXISTS.** `hetzner-dsm` → `SeanD@seandesktop` rc 0, two negative controls at 255 |
| **C4** orphan counts | "SSH and cloud … `NOT MEASURED`" | **BOTH MEASURED**, both directions, reaped, 0 billable leak |

---

## The brief's own premises, graded

The brief asked me to check two gates and named candidate credentials. Both checks were right to
run; one of its guesses was wrong and is worth stating precisely.

- **C2 — brief correct.** It measured `HZ_TO_WIN_OK` and told me not to re-park. Confirmed.
- **C1 — brief's provider guess wrong.** It pointed at `~/.config/gcloud`, `~/.azure/login/` and
  `GOOGLE_API_KEY`. The cloud backend is **Fly.io Machines**
  (`crates/wcore-exec-backend/src/backends/cloud.rs:1`, `API_BASE = https://api.machines.dev/v1`),
  credential `WAYLAND_F25_CLOUD_TOKEN`. `GOOGLE_API_KEY` **is** set on the Mac and satisfies
  nothing here. The right credential was already on the proof host and had been since 2026-07-28.
- **Both — the brief was unaware that four later lanes had already closed this work.**
  `lane/25-cloud`, `lane/25-hosts`, `lane/25-c1-cleanup`, `lane/25-c4-egress` and
  `lane/25-c4-windows` landed between 2026-07-28 and 2026-07-29. The ledger cell cites
  `Phase 25, 2026-07-28` and was never refreshed — the same staleness `lane/port-import` found in
  the `PORT-*` cell the same night.

---

## C1 — four surfaces, one commit, one diff

`evidence/reach-c1-c2/c1-four-surface-at-base.txt`

```
RC1-ONE-COMMIT: 0d48b5515b816e8930456fcda7c91c0ec9a46ebd
RC1-LOCAL: exit=0     RC1-CONTAINER: exit=0
RC1-SSH:   exit=0     RC1-CLOUD: exit=0 app=wayland-f25-test
NORMALIZED DIFF: EQUIVALENT
TOTAL: 0 orphan(s) measured across 4 backend(s); 0 surface(s) NOT measured
```

**Credential liveness read back from the product, not inferred from my environment** (§3b-ii),
with both arms:

| arm | result |
|---|---|
| credential present | `available: true`, `probe basis: VendorApiCall`, `vendor API answered 200 for app wayland-f25-test` |
| **known-negative** — token unset | `available: false`, `probe basis: CredentialAbsent` |

**Hibernation was genuinely observed in my own run**, not carried over: the cloud receipt records
`hibernation: observed` with transitions `created → started → suspended → resumed
(previous_state=suspended)` and a `/dev/shm` RAM witness surviving the transition.

### The equivalence gate runs in BOTH directions (§3b-iii)

`evidence/reach-c1-c2/c1-equivalence-both-directions.txt`

| arm | result |
|---|---|
| the four real receipts | `NORMALIZED DIFF: EQUIVALENT`, exit **0** |
| a genuinely different task, same product, `INTEGRITY: OK` | `NORMALIZED DIFF: DIVERGENT`, `differing normalized fields: artifact, events, task`, exit **1** |

**My first attempt at this control was weak and is retained rather than deleted.** It mutated a
receipt field to a wrong *type*, so the command exited 1 on a **parse error** — proving only that
the binary can exit 1, not that the *comparison* can return DIVERGENT. Replaced with an arm whose
receipt the product itself produced and whose integrity it confirms, so ARM 2 cannot be dismissed
as a corrupt file.

---

## C2 — the trust, re-measured, both directions

`evidence/reach-c1-c2/c2-ssh-trust-{positive,negative}.txt`

```
MAC -> hetzner              Ubuntu-2404-noble-amd64-base   rc=0
MAC -> seandesktop          SeanDesktop                    rc=0
hetzner -> seandesktop      SeanDesktop                    INNER_RC=0   <-- the blocker
```

Known-negatives over the identical transport, so none of those zeros is free:

```
hetzner -> seandesktop-does-not-exist   Could not resolve hostname      NEG_RC=255
hetzner -> seandonahoe@seandesktop      Permission denied (publickey)   NEG2_RC=255
```

**I did not re-run C2's node corpus** (pair / advertise / revoke / offline / recover / mixed
versions / attribution) at `0d48b551`. `lane/25-hosts` ran all of it across the two physical hosts
at `2da46485` and `25-PHASE-VERDICT.md` graded C2 **MET-WITH-STATED-EXCEPTIONS**. I verified the
*transport premise* that grade rests on, not the corpus. Counted as **unrun** below.

---

## C4 — SSH and cloud orphan surfaces, both MEASURED

### SSH — `evidence/reach-c1-c2/c4-ssh-full-cycle.txt`

The orphan is **produced by the product**, not planted: the controller is `kill -9`'d mid-task so
its `setsid` remote child is genuinely stranded. Participants asserted present **before** the kill,
per §6a-i.

| stage | product | independent far-end `ps` |
|---|---|---|
| participants at T+8 | controller alive **YES**, far-end root **YES** | **1** proc |
| **positive** — after `kill -9` | ssh `enumerated=true found=2` | — |
| **known-negative**, same instant, unused nonce | ssh `enumerated=true found=0` | — |
| `backend cancel --backend ssh` | exit 0, `residual: none — verified by re-enumeration` | — |
| **after the reap**, same nonce | ssh `found=0` | **0** |

### Cloud — `evidence/reach-c1-c2/c4-cloud-both-directions.txt`

Census taken on **two instruments**, one of which does not go through the product (raw
`GET /v1/apps/<app>/machines`, token supplied to `curl` on **stdin** via `--config -`, never argv).

| stage | product `backend orphans` | raw vendor census |
|---|---|---|
| T0 | — | **0** |
| machine created | — | **1** |
| **positive** | cloud `enumerated=true found=1` | **1** |
| **known-negative**, unused nonce, same credential | cloud `enumerated=true found=0` | — |
| `backend cancel --backend cloud` | exit 0, `residual: none` | — |
| after teardown | cloud `found=0` | **0** — no billable machine leaked |

**Cost: one `alpine:3.20` shared-CPU machine alive for under two minutes.** Sub-cent; no account,
app, org or persistent resource was created, and nothing survives.

---

## Two void runs and one dead instrument — recorded and repaired, not buried

All three are the classes this programme keeps finding, and all three would have produced a
false zero had I graded on exit status.

1. **A "0 orphans" that measured nothing.** My first SSH task asked for `cpu_millis: 600000`
   against a backend ceiling of `30000`, so it was `ResourceDenied` **before acceptance**. The far
   end never ran anything and the scan honestly returned zero — for an experiment that did not
   happen (§6a-i, "a participant that never started is a dead instrument"). Retained as
   `c4-ssh-VOID-first-attempt-resourcedenied.txt`.

2. **A reap arm that reaped an already-dead orphan.** My second attempt ran the reap in a separate
   script minutes later; the task body was `sleep 45` and had expired, so `INDEP_BEFORE=0`. The
   `residual: none` it printed was true and meaningless. Fixed by running leak → positive →
   negative → reap → re-measure **inside one live window**, with an explicit elapsed guard.

3. **`backend scan --kill` does not exist — and my script graded its failure as a failed reap.**
   clap rejected it (`error: unexpected argument '--kill' found`, exit **2**) and I initially read
   that as "the reap ran and did not work", which would have been a **false regression report**
   against the product. **Repaired in-lane** (§6b-ii) by switching to `backend cancel`, with the
   three assertions that rule demands — including the third, which is the only one that proves the
   repair does anything:

   ```
   A3 VERDICT: the old --kill invocation produced BYTE-IDENTICAL output before and
               after the reap -- it could not distinguish the two states. Instrument was dead.
   ```

I also **independently reproduced the known self-match false positive**: a diagnostic that carried
the nonce on its own command line made the `local` scanner report an orphan whose raw row was *my
own `bash -c`*. That is `lane/25-c4-egress`'s finding, reproduced at HEAD. Every measured run in
this lane generates its nonce **on the controller** with `openssl rand -hex 8`.

---

## Prior lanes' fixes are present at `0d48b551`

`evidence/reach-c1-c2/code-presence-at-base.txt` — counts redirected to a file and read with the
Read tool, never through Bash (§3b's `--numstat` finding).

| marker | file | count |
|---|---|---|
| `wait "$child" \|\| status=$?` (G2 ssh cleanup) | `backends/ssh.rs` | 1 |
| `posix_quote` (remote-injection fix) | `backends/ssh.rs` | 13 |
| `cancel_marker_taken` (G1 cloud cancel receipt) | `backends/cloud.rs` | 3 |
| `arm_egress_policy` (G4) | `wcore-cli/src/backend.rs` | 2 |
| `MissingApiKey` (C4-win HIGH regression fix) | `wcore-cli/src/backend.rs` | 2 |
| KNOWN-POSITIVE `REMOTE_RUNNER` | `backends/ssh.rs` | 9 |
| **KNOWN-NEGATIVE** `ZZZ_NOT_REAL` | `backends/ssh.rs` | **0** |

---

## Unrun cells, counted rather than skipped

**A skip is not a pass.** What I did not run:

| # | cell | why |
|---|---|---|
| 1 | C2's seven-property node corpus at `0d48b551` | ran by `lane/25-hosts` at `2da46485`; I verified only the transport premise |
| 2 | Any **Windows** leg at `0d48b551` | C4's Windows egress was closed by `lane/25-c4-windows` at `fa16cb53`; not re-measured here |
| 3 | Any **macOS** leg | no permitted host runs macOS builds; no Phase 25 criterion names a platform |
| 4 | Cloud **POST** egress denial | open on both platforms; the denied request is still the orphan-scan GET |
| 5 | `--i-accept-exfil-risk` two-key interlock | an **owner** decision, not a lane's and not a credential |
| 6 | G7 controller-side receipt identity | MEDIUM, BACKLOG |
| 7 | `setsid` on a Windows ssh far end | MEDIUM, BACKLOG (capability gap, fails loudly) |

**7 unrun cells. 0 skipped cells inside the legs I did run** — every arm in C1, C2 and C4 above
executed and produced a number.

---

## Secrets

The Fly token was consumed by sourcing the pre-existing `0600`
`/root/.wayland-f25-cloud.env` **on the host that already held it**; it never crossed the wire,
never entered an argv (supplied to `curl` on stdin via `--config -`), and was never printed.
Swept afterwards with the live value as the needle, over my committed evidence **and** the raw
hetzner captures:

```
needle length: 641   (non-empty asserted -- an empty grep -F pattern matches everything)
files swept: 69
TOKEN-VALUE-HITS:      0   (expect 0)
KNOWN-POSITIVE-HITS:   4   (a string that IS present -- the sweep is alive)
KNOWN-NEGATIVE-HITS:   0   (a string that is NOT present -- it can return zero)
ORG-VALUE-HITS:        0
```

---

## Gates

| gate | result |
|---|---|
| `cargo build -p wcore-cli` at `0d48b551` on hetzner | rc **0** |
| Fence exposure vs merge-base `0d48b551` in `wcore-cli/src/{lib,main}.rs` | **0 lines** |
| Any `crates/` or `.github/` change | **0 files** — this lane changed only `.planning/` |
| Secret sweep | 0 hits, instrument alive |

No `cargo` ran on the Mac. No test suite result is load-bearing in this summary: every claim
traces to a committed capture with an explicit exit code or a raw enumeration.

---

## What I did NOT do

- Did **not** merge, open a PR, tag, release, close an issue, push to `plan/f20-unified-audit-repair`,
  or run `wcore-contract generate`.
- Did **not** change any product code. Zero `crates/` diff.
- Did **not** create any persistent cloud resource; the one machine created was destroyed and its
  absence confirmed on two instruments.
- Did **not** touch `C:\actions-runner-*` or write anything to `seandesktop` — the only Windows
  operation in this lane was `hostname` over ssh.
- Did **not** re-run C2's node corpus or any Windows/macOS leg (items 1–3 above).
