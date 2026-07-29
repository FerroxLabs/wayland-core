---
lane: 24-gateway-surface
branch: lane/24-gateway-surface
base: plan/f20-unified-audit-repair @ e77b44b0
finding: F24-C3-H2 follow-up — the shipped surface, re-measured
end-state: "MEASURED. All 42 cells driven against `gateway run` on an overlap-free host.
  36/42 ran, failed=0, 6 email NOT MEASURED (pre-existing). Cell-by-cell against a
  same-commit `--json-stream` control: 42/42 identical, DIVERGENT=0. The surface an
  operator installs behaves identically to the surface we test, at this commit, on Linux."
grade-24-C3: "STILL NOT MET, and this lane does not claim it. It closes one specific
  doubt — that the two known HIGHs fixed since the last matrix might have left `gateway run`
  behaving differently from `--json-stream` — and that doubt is now closed with 42 measured
  cells. It moves no clause of the criterion: media and native actions remain at ZERO
  evidence on every adapter, email remains unmeasurable, and this is Linux only."
---

# 24-GATEWAY-SURFACE — the installed surface, measured

## 0. The answer

**Everything passes, and the two surfaces are indistinguishable.** Said plainly, because the
brief asks for that when it is true.

Both runs, same binary, differing only in `--runtime`, banners byte-identical apart from the
runtime name:

```
INBOUND MATRIX RED platform=linux runtime=gateway     legs=36/42 failed=0 not_measured=6 accounted=42/42 probe_failed=true restart_verdict=LOSS arrivals_total=18 telegram_arrivals=6 mail_arrivals=0 matrix_arrivals=8 signal_arrivals=6 turns_total=47 instrument_fault=false
INBOUND MATRIX RED platform=linux runtime=json-stream legs=36/42 failed=0 not_measured=6 accounted=42/42 probe_failed=true restart_verdict=LOSS arrivals_total=18 telegram_arrivals=6 mail_arrivals=0 matrix_arrivals=8 signal_arrivals=6 turns_total=47 instrument_fault=false
```

Cell-by-cell, not just aggregate:

```
A runtime=gateway     cells=42 tally={"PASS":36,"NOT_MEASURED":6}
B runtime=json-stream cells=42 tally={"PASS":36,"NOT_MEASURED":6}
union=42 identical=42/42 DIVERGENT=0
```

Aggregates are compared cell-keyed on purpose: two runs can both report "36 ran, failed=0"
while disagreeing about *which* six were not measured, and an aggregate check would call that
identical.

**The `RED` is not a gateway failure.** Both surfaces are RED for the same two pre-existing
reasons: email's six legs are NOT MEASURED (SMTP/`webpki-roots` blocker, unchanged and not
mine), and the matrix restart probe reports `LOSS` — the open HIGH raised by the
24-MATRIX-SIGNAL lane, which reproduces **identically on both surfaces**. `failed=0` means
every leg that ran, passed.

## 1. What was measured, and on what

| | |
|---|---|
| binary | `wayland-core 0.12.25 (source 3fe3832a247187edf15ef50aea943c07848ef524)` |
| sha256 | `851c049a957c8a8c28fcf6e056c0c9873950ffe528bc253f0814bda7598417fa` |
| host | `hetzner-dsm`, Linux, `/root/wayland-24-gwsurface`, node v22.21.1 |
| adapters | 7 — slack, whatsapp, sms, telegram, email, matrix, signal |
| legs | 6 — admit, dedupe, access, bind, route, steady |
| expected cells | **42**, computed as `ADAPTERS.length * LEGS.length`, never hard-coded |
| graded runs | A3 `--runtime gateway`, B2 `--runtime json-stream` |

Both graded runs completed under the WLRC/WLDONE pattern (`WLRC=1`, `WLDONE`) rather than
relying on an exit status or on the banner reaching the caller.

**Both surfaces were run at MY commit, not against the committed baseline.** The committed
`--json-stream` figures in `24-MATRIX-SIGNAL.md` were taken at binary `aa4351aa`; my base is
`e77b44b0`, which is later and contains the double-manager and reload-denial fixes. Comparing
gateway-at-`e77b44b0` against json-stream-at-`aa4351aa` would confound *surface* with *commit*
— the divergence could have been either. Running both surfaces at one binary makes any
difference attributable to the surface alone. The committed baseline agrees with both
(36/42, `failed=0`, 6 email NOT MEASURED, restart `LOSS`), which is a third point of
consistency rather than the basis of the claim.

## 2. Are these numbers from an overlap-free run? **Yes, and it is measured, not assumed.**

This sentence is load-bearing, so it gets evidence rather than an assurance.

Each graded run carried a **witness loop** sampling every 20s for any *other* lane's
`f24-inbound.mjs` driver:

| run | samples | samples with another driver present |
|---|---|---|
| A3 `gateway` | 30 | **0** |
| B2 `json-stream` | 31 | **0** |

**The witness is proven alive, because "no other driver was running" is an absence claim and
§3b-i is explicit that an absence is the easiest thing to pass without doing any work.** A
decoy process was spawned whose command line looks like another lane's driver:

```
baseline (nothing running)          -> others=0
decoy present, SAME expression      -> others=1     <- the instrument can see one
grep 'others=[1-9]' on a nonzero line -> 1
grep 'others=[1-9]' on a zero line    -> 0
```

Port `127.0.0.1:18787` was confirmed free immediately before each launch.

## 3. Three earlier runs were DISCARDED and are not graded

I ran the matrix five times. **Only the last two are graded.** The other three are reported
because discarding them is part of the result, not a gap in it.

| run | surface | disposition |
|---|---|---|
| A | gateway | **DISCARDED** — driver killed just after writing `result.json`; overlap not provable |
| A2 | gateway | **DISCARDED** — 3 concurrent drivers observed, legs burning 90s budgets at `0/1` |
| B | json-stream | **DISCARDED** — completed cleanly, but overlap not provable throughout |
| **A3** | **gateway** | **GRADED** — overlap-free, witnessed |
| **B2** | **json-stream** | **GRADED** — overlap-free, witnessed |

Runs A and B each produced complete, internally consistent results (`accounted=42/42`,
`instrument_fault=false`, `failed=0`) and in fact agree cell-for-cell with the graded pair.
**I discarded them anyway**, because at the time they ran I had verified the host was idle
immediately before launch but not continuously throughout, and a number I cannot defend is
worth less than one I can re-take. The re-take cost ~20 minutes.

I killed A2 as contaminated on my own evidence *before* the `lane/24-h6` report arrived; the
two accounts then agreed independently, including on the pid (`1923079`) they observed holding
the port, which was my A2's gateway child.

## 4. HIGH (instrument, not product) — the shared harness cannot run twice on one host, and it destroyed runs in two lanes

Both halves confirmed by reading the source, and **repaired in this lane** rather than written
up and left (§6b-ii — the one defect class this program has proven recurs *because* an earlier
sighting was documented instead of fixed):

- **`scripts/f24-inbound-run.sh:18-20` pkilled three GLOBAL patterns.**
  `pkill -f 'wayland-core --json-stream'` matches **every lane's** binary, not this
  worktree's. Now scoped to `$BINARY` and `$RUN_DIR`.
- **`scripts/f24-inbound.mjs:168` hard-coded `127.0.0.1:18787`.** Two lanes both bind it; the
  loser's webhook host never comes up and `failWebhookLegs` then reports **18 product FAILs
  that are really a port collision**. Now `F24_WEBHOOK_PORT`-overridable, default unchanged so
  no existing invocation behaves differently.

The launcher's own header comment records that it was **already** moved into a file because a
`pkill -f` matched the launcher itself. That repaired one instance and left the class: the
patterns stayed global. This is that class recurring.

**Verified with decoys, with the third assertion:**

```
T1 KNOWN-POSITIVE  narrowed pattern matches THIS lane's binary : 1
T2 KNOWN-NEGATIVE  narrowed pattern does NOT match other lane  : (other decoy alive, not in T1 set)
T3 THIRD ASSERTION the OLD global pattern matched              : 9
```

T3 is the assertion that makes the other two mean anything — without it the test passes on the
unrepaired script. (The raw decoy counts are inflated by the measuring shell's own command line
matching the pattern, which is the same self-match the script's header describes; the
direction — 9 versus 1 — is the result.)

**Why this matters beyond housekeeping:** a lane hitting this and reporting only its own
failure would file a spurious product regression against `gateway run`. That is the
fabricated-HIGH shape this program keeps catching, with an external cause. It is also the
reason I re-ran rather than reported.

## 5. Traps this lane was warned about, and what each one showed

| trap | disposition |
|---|---|
| **A green manufactured by universal denial** | Closed structurally. `arrivals_total=18`, journal bytes `arrivals=4860 turns=13387`, non-zero on both surfaces. `gradeSteady` requires `arrived === want && want > 0`, so a path denying everything scores `[0,0,0]` and FAILS; the `access` leg's pass condition *includes* its control (`admit-leg-arrived=1`), so a leg holding while its control is zero grades FAIL. Asserted in the harness self-test as `T3`. |
| **Instruments carry the defect they hunt** | Harness self-test run before trusting anything: **`SELFTEST GREEN passed=41 failed=0`**, carrying third assertions (`T4` the five original legs all pass on an adapter that goes deaf after the burst; `R3`/`V3`/`Z3` the old grader/verdict/liveness check each disagree with the repaired one). My own comparator ships a self-test proving a flipped cell AND a dropped cell are both caught — a comparator that always prints `0` would produce exactly the headline result of this lane. |
| **A liveness check that calls a zombie alive** | The harness already uses `pidIsLive`, not `kill(pid,0)`; `Z3` asserts the old check calls a SIGKILLed unreaped zombie ALIVE. **I did NOT use it for one ad-hoc cleanup** — see §6. |
| **A dedupe FAIL from the harness's own replay timing** | Not hit. Every dedupe leg replayed at ~1.0s, well inside the 60000ms TTL, and says so in its own detail line. |
| **Byte-count every capture** | Done: `{"arrivals":4860,"turns":13387,"telegram":146441,"mail":240988,"matrix":80793,"signal":3174}`. `mail_arrivals=0` is the NOT-MEASURED email, consistent with its byte count being fixture traffic only. |
| **`${PIPESTATUS[0]}` returns empty here** | Avoided for anything load-bearing. Completion came from the WLRC/WLDONE status-file pattern; leg counts came from parsing `result.json`, never from an exit status. |

## 6. One observation I am NOT claiming

A `gateway run` orphan survived 10s of `SIGTERM` and needed `SIGKILL`.

**I am not reporting this as a finding, because my instrument for it was the wrong one.** I
used `kill -0`, which is exactly the check `Z3` proves calls an unreaped zombie alive, and the
brief names `wcore_types::process_liveness` as the thing to use instead. The process was also
an orphan reparented to init whose parent had been killed mid-run, so `cleanup()` never sent
it the SIGTERM it would normally get — which separately explains why it was still up.

The 24-MATRIX-SIGNAL lane already listed the sibling claim (`--json-stream` ignored SIGTERM for
30s) as an observation **NOT established**, for the same reason. I am leaving it there rather
than adding a second under-evidenced sighting. A properly instrumented shutdown probe is a real
piece of work and it is not this lane's.

## 7. Grading 24-C3 honestly

**STILL NOT MET. This is the seventh lane to decline to claim it, and declining is correct.**

What this lane genuinely closes: the specific, named doubt that `gateway run` — the surface an
operator installs — might behave differently from `--json-stream`, the surface every previous
matrix was measured on. Two HIGHs (the subscriber-less manager sweeping the queue, and a
reloaded channel reporting healthy while denying everything) were fixed on that surface and
nobody had run the full matrix against it since. Now someone has: **42 cells, zero divergence.**

What it does **not** move, stated exactly:

- **Media: ZERO evidence on every adapter.** Unchanged by this lane.
- **Native actions: ZERO evidence on every adapter.** Unchanged by this lane.
- **Email: 6 of 42 cells remain NOT MEASURED** on both surfaces (SMTP/`webpki-roots`).
- **The matrix restart HIGH remains open** and reproduces identically on both surfaces.
- **Linux only.** No macOS or Windows matrix on either surface.
- Neither designated reference adapter clause is advanced.

The remaining distance on this criterion is therefore two clauses at zero evidence, one
unmeasurable adapter, one open HIGH, and two unmeasured platforms.

## 8. What I did NOT do

- Did not touch `crates/wcore-channel-matrix/`, `.github/workflows/ci.yml`,
  `crates/wcore-cli/src/{lib,main}.rs`, or `.planning/BACKLOG.md`.
- **Wrote no Rust.** This lane is pure measurement plus a harness repair.
- Did not re-derive whether `gateway run` opts into inbound dispatch — read
  `24-C3-H2-SUMMARY.md` instead; it is built and merged.
- Did not fix the matrix restart `LOSS`. Not this lane, and it is already an open HIGH.
- Did not measure media or native actions — no seam was built for either and inventing one is
  not a ten-minute lane.
- Did not run on macOS or Windows.
- Did not merge, open a PR, tag, or close anything.
- Did not run `cargo` on the Mac; the Darwin-behaviour exception was not needed or used.

## 9. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-GATEWAY-SURFACE-evidence/`

| path | contents |
|---|---|
| `clean-A3-gateway/` | **GRADED** gateway run: `result.json`, `run-gw3.log`, `.status` (`WLRC=1`/`WLDONE`), `.witness` (30 samples, 0 overlap) |
| `clean-B2-jsonstream/` | **GRADED** control: same set, `.witness` 31 samples, 0 overlap |
| `runA-gateway/`, `runB-jsonstream/` | DISCARDED runs, retained so the discard is auditable |
| `compare-surfaces.mjs` | the cell-by-cell comparator, with its `--self-test` |

Reproduce:

```bash
node compare-surfaces.mjs --self-test clean-A3-gateway/linux-gateway-inbound-result.json \
                                      clean-B2-jsonstream/linux-json-stream-inbound-result.json
node compare-surfaces.mjs clean-A3-gateway/linux-gateway-inbound-result.json \
                          clean-B2-jsonstream/linux-json-stream-inbound-result.json
```
