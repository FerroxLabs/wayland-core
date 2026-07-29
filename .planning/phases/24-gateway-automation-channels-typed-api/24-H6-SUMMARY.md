---
phase: 24-gateway-automation-channels-typed-api
criterion: "24-C3 (reference channels / the inbound matrix)"
lane: 24-h6
branch: lane/24-h6
merge-base: e77b44b0921e3324828c14d1e782fe67eaffc431
status: complete
finding: F24-C3-H6
finding-status: "FIXED, and proven fixed live with the finding lane's own instrument. Four required proofs all met: the gap message arrives after a restart (arrivals=1, on an incremental sync), no duplicate (gap_arrivals=1 exactly, initial_sync_total 1->1), a corrupt or missing cursor degrades safely AND SAYS SO (WARN with reason and path, re-seeds, does not wedge, syncs 13->17 on a verified-LIVE process), and steady state is unaffected (3/3 [1,1,1] on all six measurable adapters)."
grade-24-C3: "STILL NOT MET, and this lane does not claim it. One of its two open HIGHs is now closed. F24-C3-H5 remains unfixed, media and native actions still have zero evidence on any adapter, and every figure in the criterion is still Linux and still --json-stream."
gate-can-fail: "PROVEN by a 4-mutation sweep before any pass was trusted. M1 (the defect itself) reddens the gap gate; M4 (remove the wedge guard) reddens only the wedge gate. Each mutation asserted to apply exactly once; tree restored to 0 bytes of diff."
fence-exposure: "zero — 0 bytes in crates/wcore-cli/src/{lib,main}.rs, .github/workflows/ci.yml and .planning/BACKLOG.md vs the captured merge-base SHA e77b44b0"
instrument-defects-found: 3
instrument-defects-mine: 2
live-runs-graded: 1
live-runs-abandoned: "1 — run 2 discarded as contaminated: f24-inbound.mjs binds a FIXED webhook port and pkills global process patterns, so two lanes cannot run it concurrently. My launcher likely damaged the 24-gwsurface lane's concurrent run; flagged in §2e and §8.0."
---

# 24-H6 — matrix no longer loses what arrives while it is down

**Verdict up front: `F24-C3-H6` is FIXED and live-proven. `24-C3` is still NOT MET and I do
not claim it.**

The finding lane proved this defect 3/3 and handed it over unfixed, with a reasoned argument
for why: persisting a cursor carries decisions about keying, credential rotation, homeserver
change, and — the dangerous one — *what the first-run seed does*, because a partial fix that
persists the cursor but mishandles the seed would pass a naive re-run of the probe while
replaying an entire room's backlog into the agent on first start. Those decisions are made
below, and the first-run seed has its own test and its own live incarnation.

Nothing was merged, no PR opened, nothing tagged, no issue closed. No credential was read,
required or embedded — every token in every run was minted at run time and died with it.

---

## 1. What landed

Three files under `crates/wcore-channel-matrix/`, plus two shared driver files.

| file | change |
|---|---|
| `src/sync_store.rs` | **new.** Persist the `/sync` cursor per (homeserver × bot user × channel) under `$WAYLAND_HOME/channel-state/`. Three-state read. |
| `src/sync.rs` | `sync_loop` resumes the cursor from disk, persists each advance, and discards a cursor the homeserver rejects rather than backing off on it forever. |
| `src/lib.rs` | `start()` computes the state path from the production account identity. No test-only seam. |
| `scripts/f24-inbound.mjs` | instrument repair — the restart probe could not express a PASS (§5a). |
| `scripts/f24-matrix-signal-selftest.mjs` | R5/R6/R7 for that repair; P3/P4 inverted (§5b). |

### 1a. It follows the sibling, and there were TWO of them

The lane brief named `imap.rs:120`. At the merge-base there were in fact **two** adapters
already persisting a resume position, both under the same `channel-state/` directory:

```
/usr/bin/git ls-tree -r --name-only e77b44b0 crates/ | grep -iE 'offset_store|uid_store|sync_store'
  -> crates/wcore-channel-email/src/uid_store.rs
     crates/wcore-channel-telegram/src/offset_store.rs
```

Matrix was the only polling adapter without one. That is worth more than the original
argument: it is not that a third option *exists*, it is that this project had already chosen
it twice and matrix was the outlier. The new file is deliberately the same shape — same
directory, same `DefaultHasher` keying, same best-effort-write contract — rather than a second
concept. Live confirmation, from the run's own home directory:

```
home/channel-state/
  imap-8ea2d80312c458a6.uid          (email, pre-existing)
  matrix-5fbec553f4d71022.since      (this lane)
  telegram-1a32a95a8f58812b.offset   (telegram, pre-existing)
```

### 1b. The decisions the finding lane declined to make, made

| decision | choice | why |
|---|---|---|
| where it lives | `$WAYLAND_HOME/channel-state/matrix-{hash}.since` | beside the other two, so an operator has one place to look |
| keyed by | homeserver URL × bot `user_id` × channel name | repointing at a different homeserver changes the key, so a cursor is never presented to a server that cannot honour it |
| **first-run seed** | **no cursor → initial sync, timeline discarded, persist immediately** | the replay guard is PRESERVED. An upgrade onto this build has no cursor file, so it behaves exactly as today — no backlog replay — and persists from that moment so the *next* restart resumes |
| corrupt file | re-seed, and `WARN` with the reason and the path | `Absent` and `Corrupt` are distinct states; collapsing them is how a corrupt file becomes a silent restart-from-now |
| homeserver rejects it (400) | discard once, `WARN`, re-seed | otherwise the loop backs off on an unusable cursor forever — a permanently wedged channel that reports healthy |
| crash between deliver and persist | persist AFTER the events are in the inbox | at-least-once. The dedupe layer collapses a duplicate; nothing recovers a skip |

---

## 2. The four required proofs

All live figures: `hetzner-dsm`, Linux, `/root/wayland-24-h6`, `wayland-core 0.12.25`, sha256
`d618d3766db2f30eaf75f68a0865b15ac5d831a6aec40189d49f8a05f610f364`, runtime `--json-stream`.

### 2a. A message delivered while the process is down arrives after restart — **PASS**

Driven by the finding lane's own restart probe, on the shipped binary. **6/6 legs PASS**
(it was 5/6 with the leg FAILing at `verdict=LOSS` before this fix):

| # | leg | result |
|---|---|---|
| 1 | `pre-restart-live-control` | PASS arrivals=1 |
| 2 | `binary-down-and-quiet-during-the-gap` | PASS `stopped=true`; fixture `sync_total` 280 → 280 → 280 |
| 3 | `restarted-binary-resyncs` | PASS `sync_total 280 -> 282`, **`initial_sync_total 1 -> 1`** |
| 4 | `gap-event-was-served-to-the-restarted-process` | PASS — sync **281, `initial:false`**, carried `$f240d3685degap` |
| 5 | `post-restart-live-control` | PASS arrivals=1 |
| 6 | **`gap-message-survives-the-restart`** | **PASS — `verdict=PASS arrivals=1`** |

Leg 3 is the mechanism in one number: **the restarted process issued no initial sync at all.**
It resumed. Leg 4 is the H2 exclusion, read from the fixture's own report in another OS
process, and it now also records *which kind* of sync served the event — `incremental`, i.e.
the adapter **asked for** the window it had missed.

From the fixture's independent journal, extracted with an unproxied tool and with
known-positive controls in the same invocation (`journal-mechanism.txt`, 282 `sync.close`
records):

```
total initial syncs in the WHOLE run (both incarnations): 1
PRE  $f240d3685depre : initial_syncs_serving=0  incremental_syncs_serving=1
GAP  $f240d3685degap : initial_syncs_serving=0  incremental_syncs_serving=1
POST $f240d3685depost: initial_syncs_serving=0  incremental_syncs_serving=1
```

The finding lane measured the GAP row as `initial=1 incremental=0` with `arrivals=0`. Same
instrument, same fixture, opposite result. The PRE and POST rows are the controls that keep
this from being a zero obtained for free.

Operator-visible, from the product's own logs across two incarnations:
```
core.log            INFO no persisted /sync cursor (first start for this account); seeding
                         from an initial sync — existing room backlog will NOT be replayed
core-restarted.log  INFO resumed persisted /sync cursor; messages delivered while this
                         process was down will be served
```

Unit-level, in the crate, so this reddens in CI without a live host:
`gap_message_survives_a_restart_and_is_not_duplicated`.

### 2b. No duplicate on restart — **PASS**

Two independent assertions, pulling in opposite directions so neither can be met by doing
nothing:

- **Live:** `gap_arrivals=1` — exactly one, not two. `initial_sync_total 1 -> 1` means the
  restarted process never re-seeded, so it could not have replayed the room. `matrix/dedupe`
  PASS (`arrivals before=1 after=1`, with a fresh-id positive control at 1).
- **Unit:** the pre-restart initial sync's timeline carries `$pre`; lifetime 2 must deliver
  **exactly `["$gap"]`** — `$pre` never, `$gap` once. The initial-sync mock carries
  `.expect(1)`: a second initial sync (the unfixed behaviour) reddens it.

The replay guard survived the fix, and that is asserted structurally, not just by test:
selftest `P3` requires `sync.rs` to still contain `let is_initial = since.is_none();` and
`if !is_initial {`. Resuming must not be achieved by deleting the guard.

### 2c. A corrupt or missing cursor degrades safely and says so — **PASS (one gap, stated)**

Three incarnations of the shipped binary against one fixture
(`24-H6-evidence/degradation-probe/`):

| incarnation | state | result |
|---|---|---|
| A first start | `Absent` | INFO, seeds, persists `s0`, `initials=1` |
| A **control** | — | **KNOWN-POSITIVE healthy-climb `syncs 3 -> 8` over 10s, `proc=LIVE(S)`** |
| B clean restart | `Cursor` | INFO resumed; **`initials 1 -> 1`** — did not re-seed |
| C corrupt | `Corrupt` | **WARN** `reason="contains control or whitespace characters" path=...`; `initials 1 -> 2` (re-seeded); junk **replaced** with a valid cursor; **NOT WEDGED: `syncs 13 -> 17` over 10s, `proc=LIVE(S)`** |

The known-positive control is what makes C's numbers mean anything: without it, "the counter
moved" has no scale.

Six corrupt shapes are classified with a reason at unit level — empty, whitespace-only,
embedded space, embedded NUL, oversized, invalid UTF-8 — and a *trailing newline* (what an
operator's editor adds) is explicitly trimmed rather than rejected.

**The one gap, stated rather than papered over.** The wedge guard for a cursor the
**homeserver rejects** (HTTP 400) is proven against a real 400 from `mockito` at unit level,
and is **not** live-proven: `f24-matrix-fixture.mjs`'s `parseSince` treats any unparseable
token as an initial sync and never answers 400, so that fixture structurally cannot exercise
the path. Closing it needs a fixture change, which would touch a file four other drivers
depend on; I judged that not worth the shared-file risk for a path the unit test covers with
a genuine HTTP 400. Named here so the next lane can decide differently.

### 2d. Steady-state delivery unaffected — **PASS, counted**

Live, after 30s of deliberate quiet, three messages 4s apart each with its own correlation
token so a single swallowed message shows as a count and not a boolean:

| adapter | steady |
|---|---|
| slack / whatsapp / sms / telegram / **matrix** / signal | **PASS 3/3 `[1,1,1]`** each |
| email | NOT MEASURED (pre-existing SMTP/webpki-roots blocker, untouched) |

Matrix 6/6 and signal 6/6 on all legs. Whole run: `legs=36/42 failed=0 not_measured=6
accounted=42/42 probe_failed=false restart_verdict=PASS`. **`failed=0` — every leg that ran,
passed.** The run still exits RED because email's six legs remain NOT MEASURED; that is
unchanged, pre-existing, and not mine.

Unit-level: `steady_state_delivery_is_unaffected_by_cursor_persistence` demands exactly
`["$m1","$m2","$m3"]` in order across three successive resumed syncs. It demands arrivals > 0,
so a path that denied everything scores zero and fails.

### 2e. Only ONE graded live run, and why the second was abandoned rather than reported

The finding lane reproduced 3/3. **I have one graded run.** I attempted a second and killed it
as contaminated; I am not reporting its numbers in either direction.

`f24-inbound.mjs` **cannot be run concurrently on `hetzner-dsm`**, and I established the
mechanism rather than assuming a flake:

- The config it writes carries a **fixed** `[inbound_webhook] bind = "127.0.0.1:18787"`. While
  my run 2 was live, `ss -lptn` showed that port held by **pid 1923079 — another lane's
  binary**, started ~04:20. My run's webhook could never bind, so its webhook-delivered
  adapters (slack / whatsapp / sms) stalled. Run 2's first visible symptom was
  `awaiting f24c3-sms-steady2: 0/1 after 51s`.
- `scripts/f24-inbound-run.sh:18-20` runs `pkill -f 'wayland-core --json-stream'`,
  `pkill -f 'scripts/f24-sink.mjs'` and `pkill -f 'scripts/f24-llm-fixture.mjs'` — **global
  patterns, not run-scoped.** The other lane's run launched at 04:20:36; my launcher fired
  those `pkill`s at 04:22:50. **I very probably killed that lane's sink and LLM fixture about
  two minutes into their run**, and their run was simultaneously stuck on
  `awaiting f24c3-matrix-steady2: 0/1`. Reported here because they need to know, and because a
  lane seeing only its own red would file a product regression.
- Host state at the time: 3 sinks, 3 LLM fixtures, 4 telegram fixtures and 4 matrix fixtures
  alive at once.

I killed **only my own** processes (run 2's process group, my binaries by absolute path, plus a
binary leaked from run 1) — by pid, never by shared pattern, precisely so I would not repeat
the damage. Verified afterwards: zero `wayland-24-h6` processes remain, and the other lane's
listener on 18787 is untouched.

**Run 1 was clean and is the graded one.** It completed at `04:08:04`; the other lane's run
started at `04:20:36`, so there was no overlap. Its `failed=0` across 36 legs — including every
webhook-delivered adapter — is itself evidence the webhook bound normally on that run.

What carries the reproduction load instead is the unit suite: five loop tests over the real
`sync_loop`, proven able to fail by a four-mutation sweep, runnable by anyone on any host with
no fixture and no port.

---

## 3. The gates were proven able to fail BEFORE any pass was trusted

The lane brief is explicit that the finding lane's verdict **could not fail on a restart
loss** and only went red for an unrelated reason. So no green here is trusted on its own.

Four mutations, each asserted to apply **exactly once** (a replacement that silently matched
nothing would make the whole sweep meaningless), each run, tree restored afterwards to **0
bytes** of diff:

| mutation | result | tests that failed |
|---|---|---|
| **M1** ignore the persisted cursor — *the defect itself* | **REDDENED**, 3 failed | `gap_message_survives_a_restart_and_is_not_duplicated`, wedge, steady |
| **M2** never persist | **REDDENED**, 5 failed | all five loop tests |
| **M3** `Corrupt` reads as `Absent` | **REDDENED**, 2 failed | corrupt-classification, corrupt-reseed |
| **M4** remove the wedge guard | **REDDENED**, 1 failed | `a_cursor_the_homeserver_rejects_is_discarded_rather_than_wedging` |

M4 failing **only** the wedge test, and M1 failing the gap test, is the attribution that
matters: these are not one blanket assertion wearing five names.

Counts are read back, never inferred from exit status: `36 passed; 0 failed; **0 ignored**`.

---

## 4. Gate results, all at `bf894aeb` on hetzner

| gate | result |
|---|---|
| `cargo test -p wcore-channel-matrix` | **36 passed / 0 failed / 0 ignored** |
| `cargo test -p wcore-channels-registry` (downstream consumer) | **11 passed / 0 failed** |
| `cargo clippy -p wcore-channel-matrix --all-targets` | **0 warnings** |
| `cargo fmt --all -- --check` | clean |
| `node scripts/f24-matrix-signal-selftest.mjs` | **GREEN passed=44 failed=0** (41 before this lane) |
| fence diff vs captured BASE **SHA** `e77b44b0` | **0 bytes** |
| live `f24-inbound.mjs`, `--json-stream` | `failed=0 accounted=42/42 restart_verdict=PASS` |

No full-workspace run was taken (disk/contention rule); `df -h /root` checked first, 701G free.

---

## 5. Three instrument defects — two of them mine, two blaming the product

### 5a. The restart probe could not express a PASS — it could report the defect but not the fix

The finding lane's probe keyed its H2 exclusion on `initial_syncs[].served` and its liveness
leg on `initial_sync_total`. **Both are true only while the product is broken.** An adapter
that resumes issues an *incremental* sync after a restart and never an initial one — so on
correct code the exclusion was false, `gradeRestart` returned `INCOMPLETE`, and leg 3 burned
its full 90s budget before failing *for the one reason that means the fix worked*.

This is the mirror image of the self-passing gate this same probe was already repaired for
once: that one could not fail, this one could not pass. Had I run it and reported the result,
the fix would have read as **unproven**.

Repaired to `servedAfterRestartFrom(report.syncs, …)` — any sync after the restart, recording
which **kind** — which is what H2 always meant ("the fixture never served it", not "never
served it on an initial sync"). Strictly stronger: it still excludes H2, and it now grades
both states of the product. Three assertions, per §6b-ii, with
`legacyServedInInitialOnly` kept executable:

- **R5** known-positive: gap on a resumed incremental sync → `served`, `where='incremental'`, PASS reachable.
- **R6** known-negative: a gap the fixture never served → still `INCOMPLETE`, never `LOSS`. The widening did not weaken the exclusion.
- **R7** third assertion: on the **fixed** shape the old extraction returns `false` and would have graded a working fix `INCOMPLETE`; on the **broken** shape both agree — which is exactly why the defect went unnoticed, the instrument having only ever been exercised against broken code.

### 5b. P3/P4 fired exactly as designed, and are now inverted rather than deleted

The finding lane wrote `P3`/`P4` to assert the defect was *still present*, explicitly so that
"if someone repairs `sync.rs`, `P3` reddens and names itself — so a future green cannot be
misread as 'the defect was never there'." On the first post-fix selftest run they did exactly
that: `SELFTEST RED passed=42 failed=2`, with `P3` printing *"the restart finding may be
FIXED; re-read sync.rs before reporting the probe result."* That is a design working.

Inverted to assert the fix — including that the **replay guard survived it** and that the
cursor still lives in `channel-state/` — rather than deleted, so a regression reddens again.

### 5c. MINE — the live probe called an exited process a wedged one

My first degradation probe started the binary with `< /dev/null`. `--json-stream` is a stdio
protocol surface: stdin at EOF means the peer hung up, so it shut down after two syncs. The
NOT-WEDGED check then read a stalled sync counter off an **already-exited** process:
`syncs 5 -> 5`. That reads as *"the fix wedges the channel on a corrupt cursor"* — a
fabricated HIGH against code that works, and the fifth-plus sighting of this exact class
(an unchecked liveness assumption; the same shape as the zombie that made a clean shutdown
read as "ignored SIGTERM for 30s").

Repaired two ways, both needed: stdin is held open by `tail -f /dev/null` as the shipped
driver does, and **every counter reading is now paired with a zombie-aware liveness state**
read from `/proc/<pid>/stat` (`Z`/`X` = DEAD), so an exited process can never again be read as
a wedged one. The third assertion is a measured outcome change on the same product at the same
commit: v1 `syncs 5 -> 5` (process gone) versus v3 **`syncs 13 -> 17`, `proc=LIVE(S)`**.

### 5d. MINE — the v2 repair broke the probe entirely

My first attempt at holding stdin open used a FIFO opened inside a command substitution. The
binary never started; every counter read `0`, which would have read as *"the adapter never
syncs at all."* Also blaming the product. Replaced with `tail -f /dev/null` and a pid taken
from the OS rather than from `$!`.

**Two of the three failed in the direction that blames the product.** That is not coincidence;
it is what an under-instrumented probe does by default, and it is why each had to be caught by
a control rather than by a red.

---

## 6. What I did NOT do

- **Did not mark `24-C3` MET.** Seven lanes have now declined to; this one closes one of its
  two open HIGHs and does not come close to the criterion.
- **Did not fix `F24-C3-H5`** (the reload half of `reconnect`). Not my lane's assignment, and
  still unfixed.
- **Did not measure `media` or `native actions`** — two of the criterion's eight clauses,
  still zero adapters, exactly where the last two lanes left them.
- **Did not measure the upstream-drop half of `reconnect`.** My probe restarts the *process*;
  dropping the *connection* under a running process is untouched.
- **Did not live-prove the HTTP-400 wedge guard** (§2c) — unit-proven against a real 400; the
  fixture cannot answer 400 to a `since` token.
- **Did not run on `gateway run`.** Every figure is `--json-stream`. `F24-C3-H2` and `H5` are
  both `gateway run` findings and this does not clear that surface.
- **Did not measure anything on macOS or Windows.** Every figure in this criterion, from every
  lane, remains Linux. I did not use the Darwin-behaviour exception; nothing here is
  Darwin-specific and hetzner could prove all of it.
- **Did not run a full-workspace build or test** — three targeted crates only, per the
  disk/contention rule.
- **Did not touch the fence** (0 bytes vs the captured SHA), `wcore-contract generate`, a
  merge, a PR, a tag, or an issue.
- **Did not modify any shared fixture** (`f24-sink.mjs`, `f24-matrix-fixture.mjs`,
  `f24-llm-fixture.mjs`, `f24-correlate.mjs`, `f24-tg-fixture.mjs`, `f24-mail-fixture.mjs`,
  `f24-signal-fixture.mjs`). The only shared files changed are `f24-inbound.mjs` and
  `f24-matrix-signal-selftest.mjs`, and only in the matrix restart probe.
- **Did not use, read or require any vendor credential.**

---

## 7. Remaining distance to `24-C3` MET

| # | gap | status after this lane |
|---|---|---|
| 1 | **`media`** | **UNCHANGED — zero adapters** |
| 2 | **`native actions`** | **UNCHANGED — zero adapters** |
| 3 | `reconnect/reload` | **restart path CLOSED**; reload half still PARTIAL (H5); upstream-drop half untouched |
| 4 | **F24-C3-H5** | **still unfixed** |
| 5 | **F24-C3-H6** | **FIXED and live-proven** |
| 6 | email `route`/`bind` | unchanged — reachable, costed, not built |
| 7 | matrix, signal adapters | unchanged — done by the prior lane |
| 8 | msteams / imessage | unchanged (~2 sessions / platform-blocked) |
| 9 | `gateway run` for the new leg + 2 adapters | unchanged — still the cheapest next measurement (~10 min) |
| 10 | macOS + Windows | unchanged — every figure is Linux |

`24-C3` is a **release blocker and it is still open.** Two of its eight clauses have zero
evidence on any adapter, a third is PARTIAL with one unfixed HIGH still against it, and every
number in the criterion is single-platform. **Marking it MET would be wrong.**

---

## 8. For the orchestrator to serialize

**Almost nothing.** Zero fence bytes, no protocol seam, no contract fixture, no dependency
change, no `Cargo.lock` edit. `MatrixChannel::new` / `with_base` signatures are unchanged, so
no downstream crate is affected (`wcore-channels-registry` 11/11 green).

**THREE coordination notes — the third is a live hazard affecting other lanes right now.**

0. **`f24-inbound.mjs` must be serialized across lanes on `hetzner-dsm`.** It binds a **fixed**
   `127.0.0.1:18787`, and its launcher `pkill`s three **global** process-name patterns
   (`wayland-core --json-stream`, `scripts/f24-sink.mjs`, `scripts/f24-llm-fixture.mjs`). Two
   concurrent runs destroy each other: one loses the webhook port, and whichever launches
   second reaps the other's sink and LLM fixture. This happened between my run 2 and the
   `24-gwsurface` lane's gateway run on 2026-07-29 — **both** stalled on a `steady2` leg, and
   **my launcher is the likely cause of theirs.** Neither run is a measurement. Any lane that
   saw only its own red here would file a spurious product regression. The cheap fixes are a
   run-scoped webhook port and run-scoped `pkill` patterns; I did not make them because that
   file is shared with four other drivers and a change to its process handling mid-flight is
   exactly the wrong thing to do while other lanes are running against it. **See §2e.**

**Two further coordination notes.**

1. `scripts/f24-inbound.mjs` is shared with four other drivers. `gradeRestart`'s parameter is
   renamed `servedInInitial` → `servedAfterRestart`, and two new exports are added
   (`servedAfterRestartFrom`, `legacyServedInInitialOnly`). Any other caller of `gradeRestart`
   must pass the new name — passing the old one yields `undefined`, which fails the control
   and grades `INCOMPLETE` rather than silently passing. `LEGS` (6) and `ADAPTERS` (7) are
   **unchanged**; expected total is still 42 and the restart probe is still recorded outside
   `results`, so leg reconciliation is untouched. Exit codes unchanged: 0 GREEN, 1 RED,
   2 USAGE, 3 INCOMPLETE.

2. **New on-disk state.** `$WAYLAND_HOME/channel-state/matrix-{hash}.since` is written at
   runtime by any configured matrix channel. It joins the two files already written there by
   email and telegram. No migration is needed — its absence is a valid first-run state that
   behaves exactly as the previous build did.

---

## 9. Evidence

`.planning/phases/24-gateway-automation-channels-typed-api/24-H6-evidence/`

| path | what it holds |
|---|---|
| `24-H6-NOTES.md` (parent dir) | the running log, committed at T+0 before any measurement and re-committed after each |
| `run1-json-stream/` | `result.json` (328 KB), `run.log`, `matrix-fixture.jsonl`, `arrivals.jsonl`, **both incarnations' `core.log`**, `journal-mechanism.txt`, `channel-state-listing.txt` |
| `degradation-probe/` | `probe.sh` (v3, with both of its own repaired defects documented in the header), `core-a.log`, `core-c.log` |

The restart result is reproducible from `matrix-fixture.jsonl` alone: every `sync.close`
record carries `initial`, `since` and `served`.
