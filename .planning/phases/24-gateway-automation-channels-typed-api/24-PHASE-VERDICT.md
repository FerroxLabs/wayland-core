---
phase: 24-gateway-automation-channels-typed-api
graded: 2026-07-29
lane: grade-24
branch: lane/grade-24
base: 861d1b1a716240165209336b1fa38d36f9445716
goal: "Operators can install, run, automate, connect, inspect, recover, and support one persistent Core runtime on every OS family."
goal-verdict: "NOT ACHIEVED — but far closer than any prior document says, and for one specific reason: `support` is the word in the goal that fails, and it fails on a defect this grading found."
grades:
  24-C1: "MET-WITH-STATED-EXCEPTIONS"
  24-C2: "PARTIAL"
  24-C3: "PARTIAL"
  24-C4: "PARTIAL"
  24-C5: "MET-WITH-STATED-EXCEPTIONS"
new-finding: "F24-C4-H1 (HIGH) — `wcore_gateway::support_bundle` has ZERO production call sites and no CLI verb. The half of Criterion 4 that says 'produce useful redacted health/log/support evidence' is unreachable from the shipped binary. Same advertised-but-dead class as F24-MB-1. Found by re-derivation; the gap ledger graded 24-C4 MET."
still-blocks-release: "YES, on one item only — F24-C4-H1 — and it is roughly a half-session to fix. Every other open item on this phase is a can-ship-open gap, NOT a blocker."
instrument-defects-mine: 1
instrument-defects-repaired: 1
fence-exposure: "ZERO. `git diff --stat 861d1b1a HEAD -- crates/wcore-cli/src/{lib,main}.rs` empty, with a live control in the same invocation (24-GRADE-NOTES.md = 209 added lines). 0 files under crates/ or .github/. This lane changed only .planning/."
supersedes: "24-PHASE-REPORT.md (2026-07-26) as the phase's grading document. That report predates ~20 lanes and grades all five criteria NOT MET."
---

# Phase 24 — Gateway, Automation, Channels, and Typed API — PHASE VERDICT

**Graded 2026-07-29 by lane `grade-24` at base `861d1b1a`.** This is the first verdict this phase
has had. Working notes and the full measurement trail: `24-GRADE-NOTES.md`.

---

## 0. Bottom line

**The phase goal is NOT ACHIEVED, and the gap is much narrower than the record suggests.**

Read the goal one verb at a time — *install, run, automate, connect, inspect, recover, support*:

| verb | state |
|---|---|
| install | works on all three OS families, receipted |
| run | works on all three, receipted |
| automate | works for 3 of 5 named work kinds; webhook and poll do not exist |
| connect | works, extensively, on Linux; nothing on macOS or Windows |
| inspect | works — `gateway status`, `channel health/probe`, typed client |
| recover | works on all three, **observed** rather than asserted |
| **support** | **does not work — there is no verb** |

`support` is the one that fails outright, and it fails on a defect this grading found rather than
inherited: **`wcore_gateway::support_bundle` is a library module with zero production call sites
and no CLI surface** (§5, `F24-C4-H1`). The code was written, tested, mutation-gated — and never
connected to anything an operator can run.

**Two of five criteria are MET-WITH-STATED-EXCEPTIONS. Three are PARTIAL. None is NOT MET.** That
last point matters: every prior grading document on file records this phase at NOT MET on four or
five criteria. **None of those documents is current.**

**Does Phase 24 still block a release? Yes — on one item, worth about half a lane-session.** Full
answer in §8. The honest headline is that this phase has been carrying a "last release blocker"
label for a week that its own evidence stopped justifying around 2026-07-28.

---

## 1. What I re-derived, and what changed as a result

The brief instructed: re-derive, never inherit. Four inherited claims did not survive.

| # | Inherited claim | Source | Re-derived result |
|---|---|---|---|
| 1 | `24-C4` is **MET** on Linux/HTTP+SSE | `CRITERIA-GAP-LEDGER.md:327` | **FALSE — PARTIAL.** The support-evidence half has no operator verb. §5 |
| 2 | active-turn visibility is *"never rendered to an operator, no verb exists"* | `24-PHASE-REPORT.md:53` | **FALSE.** `gateway.rs:494` prints `turns in flight:` and `:479` emits it in `--json`. §2 |
| 3 | `24-C5` NOT MET on one of three platforms | `RC-READINESS.md:39-58` | **STALE SECTION.** Superseded by the same file's lines 18-38. §6 |
| 4 | `e2e-product-smoke` proved **12/12** | this lane's brief | **12 of 14**, 2 not reached (TUI, Windows/macOS cold start). §7 |

Item 3 is the "one contains a superseded section" the brief warned about, and I can name it
precisely: **`RC-READINESS.md` contains two Item-3 sections that contradict each other, and the
STALE one appears LAST.** Git settles the order — `24-C5-JOURNEY-SUMMARY.md` (`fd64bd5c`, Windows
RED / macOS NOT RUN) precedes `24-C5-FINISH-SUMMARY.md` (`e535c1a4`), which `5ed01866` merged as
"24-C5 MET on all three platforms". A reader going top-to-bottom gets the wrong answer last.
**Recommend deleting `RC-READINESS.md:39-58`.**

Item 2 is why absence claims need concept searches: `grep active_turn` in the gateway returns **0**
and would have confirmed the stale claim for free. The concept lives under `turns_in_flight`.

---

## 2. Criterion 1 — MET-WITH-STATED-EXCEPTIONS

> **"Native service lifecycle, profile isolation, active-turn visibility, drain, restart, upgrade,
> and rollback work without lost or duplicate delivery."**

Seven named capabilities. **All seven are now demonstrated, and six of the seven on all three OS
families.**

| clause | status | evidence |
|---|---|---|
| native service lifecycle | **MET ×3 platforms** | systemd, launchd, Task Scheduler — install→run→uninstall inside the 17-step journey, receipted on each |
| profile isolation | **MET** | `F24-B-H1` FIXED; one home / one lock / distinct homes do not exclude |
| active-turn visibility | **MET** | `lifecycle.rs:182` `turns_in_flight`; rendered at `gateway.rs:494` (human) and `:479` (`--json`). **Re-derived — the record said this was unrendered** |
| drain | **MET** | ordered observable state, live: `Draining (pending 12)` → `Drained (pending 0)` |
| restart | **MET ×3 platforms** | recovery **observed**: Linux `NRestarts=1`; macOS launchd `LastExitStatus=9`, new PID 46344; Windows killed 44096 → platform-started 46028, **no start command issued** |
| upgrade | **MET ×3 platforms** | running service reported the upgraded `binary_path` on each |
| rollback | **MET ×3 platforms** | then reported the original on each |
| without lost/duplicate delivery | **MET, at the measured scale** | 12 submitted / 12 arrived / 12 unique / **0 duplicates / 0 losses**, counted at an **independent sink running as its own OS process**, on all three platforms |

The upgrade/rollback and tri-platform legs landed in `24-C5-FINISH-SUMMARY.md`; before that lane
they had never been performed anywhere, which is why every older document grades this NOT MET.

### The stated exceptions

**(a) 7 of 10 channel adapters cannot suppress a cross-restart outbound replay.** Census
(`/usr/bin/grep -rn "fn supports_outbound_idempotency" crates/ --include=*.rs`): overrides in
**slack, matrix, discord**; trait default `false`; **10** adapter crates. The other seven **abandon**
an outcome-unknown delivery rather than risk duplicating it. That satisfies "without duplicate
delivery" and **does not** satisfy "without lost delivery" — abandonment is a non-delivery. It is
honest, fails closed, and `24-C1-ABANDON-SURFACE` built a surface so an abandoned delivery is
**nameable** rather than silent. Named, not absorbed.

**(b) The clean tally is 12 messages, not a soak.** 12/12 with 0/0 is a real out-of-process
measurement, not a self-report. It is also small. Nothing here contradicts the criterion; it does
not establish behaviour at volume either. Volume is Phase 28's 1,000-session soak, not this
criterion's job — recorded so no one reads 12/12 as a load result.

**Why not plain MET:** exception (a) is a genuine, permanent-until-fixed hole in the criterion's own
final clause across the majority of adapters. **Why not PARTIAL:** all seven named capabilities work,
on all three platforms, with recovery observed rather than asserted. MET-WITH-STATED-EXCEPTIONS is
exactly the grade this shape calls for.

---

## 3. Criterion 2 — PARTIAL

> **"Scheduled, event-driven, webhook, polling, and commitment work has bounded history, retry,
> continuation, and delivery."**

Five work kinds × four properties.

| work kind | status |
|---|---|
| scheduled (`once:` / `every:` / `cron:`) | **works**, live-proven |
| commitment (`commit:`) | **works**, live-proven |
| event-driven (`event:`) | **works** — durable cross-process queue, fired by `cron publish` |
| **webhook** | **DOES NOT EXIST.** Refused at `add`; removed from `--help` |
| **polling** | **DOES NOT EXIST.** Refused at `add`; removed from `--help` |

Verified in the current tree: `crates/wcore-cli/src/cron.rs:44-57` states verbatim *"`webhook:` and
`poll:` are NOT accepted: nothing in this build can fire them."* `cron.rs:350` prints
`WILL NEVER FIRE` for legacy persisted jobs.

Properties: **bounded history — met** (seven trigger types, each one-way-narrowing); **retry — met**,
enforced; **delivery — met** through the 24-01 ledger, and materially strengthened since the ledger
was written by `lane/channel-lease` (a held `flock`/`LockFileEx` `ScheduleLease` replacing a
first-come assumption) and `lane/channel-starvation` (the installed service now wins ownership and
the loser hands it back — 144 polls, three handovers, never more than one concurrent poller, 4/4
delivered). **Continuation — NOT MET**: no run has hard-killed a gateway mid-fire and counted at an
out-of-process sink. `max_in_flight` is stored and clamped but not enforced at dispatch. No macOS
evidence.

**The customer-facing repair is real and should be credited.** What made this the ledger's worst
item was **silent acceptance** — `--trigger poll:URL:300` was measured firing **6 times without ever
contacting the URL**, which is worse than doing nothing. That is gone. A customer can no longer
register automation that lies to them.

**Why PARTIAL and not MET-WITH-STATED-EXCEPTIONS:** two of the five named work kinds do not exist.
That is not an exception to the criterion, it is 40% of its subject matter absent. **Why not NOT
MET:** three of five work with bounded history, retry and delivery, all reachable from the shipped
binary.

---

## 4. Criterion 3 — PARTIAL

> **"Reference channels prove setup/auth, access, routing, media, native actions, idempotency,
> reconnect/reload, and health."**

**This is where I differ from the record, and I want to be exact about how.**

Nine lanes declined to claim `24-C3`. **Every one was right, and every one remains right** — because
each was answering *"does MY work close this criterion?"*, and for each the answer was no. That is
not the same question as *"where does the criterion stand?"* No single lane was in a position to
answer the second, and no one has. Graded as an aggregate against this programme's own calibration —
where Phase 27 used NOT MET for *"none of the four generation shapes was exercised"* and *"no audio
ever flowed on any machine"* — **`24-C3` is PARTIAL. NOT MET is no longer a true description of it.**

### The clause matrix, on Linux

| # | clause | status | evidence |
|---|---|---|---|
| 1 | setup/auth | **PROVEN** | `channel probe` shipped (`channel.rs:96,268`), default is a **named** `Unsupported`, never a green. Driven live on discord (6/6 legs), telegram, matrix, signal (12/12 each), slack, whatsapp, sms |
| 2 | access | **PROVEN** | `F24-C3-H1` fixed — an isolated profile had been **silently denying every allowlisted sender**. `F24-C3-H5` fixed — a reloaded channel now admits, under the posture its config asked for |
| 3 | routing | **PROVEN** | bind/route legs, arrivals derived from an independent sink's journal, not a status line |
| 4 | media | **PARTIAL** | ▸ inbound attachments live on msteams, 5/5, two product-side mutation controls ▸ **audio positive direction**: real voice note on telegram → connector fetch → **real** transcription provider → transcript reaches the model, reproduced twice, with an anti-echo control ▸ declared per-adapter bounds now **read and enforced in production on every adapter** ▸ **image/vision half structurally unreachable** (`F24-C3-H7`) ▸ outbound media untouched |
| 5 | native actions | **PROVEN — evidence unmerged** | 6 adapters × 3 affordances, `gateway run`, counted **platform-side**, per-adapter negative control. 5/5 declaring `react` fire both; 4/4 declaring `send_typing` fire typing. **Zero advertised-but-dead.** §7 |
| 6 | idempotency | **PARTIAL** | inbound dedupe proven end-to-end; **outbound only 3 of 10 adapters** (§2 exception a) |
| 7 | reconnect/reload | **PARTIAL** | **reconnect**: upstream drop under a live process — discord **7/7**, matrix **7/7**, `lost=0 duplicated=0`. slack/whatsapp/sms out of scope by construction (webhook transport — no upstream to lose). **telegram, email, signal, msteams, imessage NOT REACHED.** **reload**: `F24-C3-H5` fixed and live-proven, 11/11 |
| 8 | health | **PROVEN** | `channel health` shipped (`channel.rs:108,269`); `Unknown` is not `Healthy`; PASS measured |

**Five clauses proven, three partial — all of it on Linux.**

### macOS and Windows have nothing on this criterion

Stated plainly because the brief asks for it: **zero of the eight clauses has been exercised on
macOS or Windows.** Every figure in the table above is Linux, on `hetzner-dsm`. The phase goal says
"on every OS family"; for channels, two of three families are unmeasured. This is the single largest
gap in the criterion and it is a coverage gap, not a defect.

### Two things that raise confidence, and one that lowers it

**Raises:** `24-GATEWAY-SURFACE` drove **42 cells** against `gateway run` and compared each,
cell-by-cell, with a same-commit `--json-stream` control: **42/42 identical, DIVERGENT=0**. That
closes the doubt that the two known HIGHs fixed mid-phase had left the *installed* surface behaving
differently from the *tested* surface. `DIVERGENT=0` is a negative claim, but it is a
**differential** one — 36 cells ran and produced output on both arms, so the instrument demonstrably
worked. Accepted.

**Raises:** the HIGHs found in this criterion were found by **driving the product, not by reading
it** — every allowlisted sender silently denied under an isolated profile; the installed gateway
binding no inbound receiver at all; two `ChannelManager`s racing so **8 of 8 messages were lost at
startup**; matrix silently discarding everything that arrived while it was down. Four HIGHs, all
fixed and mutation-proved. A criterion that surfaced four real HIGHs is a criterion that was
genuinely exercised.

**Lowers:** 6 email inbound cells remain **NOT MEASURED** — email's inbound path cannot be pointed
at a fixture from configuration alone, and email is one of the two adapters `24-03` designated as
its **reference** pair. A criterion about *reference* channels with one reference adapter's inbound
unmeasurable is a real hole, not a rounding error.

---

## 5. Criterion 4 — PARTIAL  *(downgraded from the ledger's MET — new HIGH)*

> **"Typed authenticated clients recover event gaps and produce useful redacted health/log/support
> evidence."**

Two halves. **The first is met. The second is unreachable from the product.**

### Half one — typed authenticated clients recover event gaps: MET on HTTP/SSE, Linux

A typed client authenticates; is refused by **ROLE** distinctly from **CREDENTIAL** (deny-all
default); issues idempotent commands (one effect, two identical receipts); negotiates version or is
refused **by name**; and **recovers an event gap over a real socket from a connection severed
mid-stream, duplicates and losses both zero**. Verified present in the tree: `resume_events` and the
`/sessions/:id/events` route at `wcore-acp/src/transport/http.rs:188,264,481`; `Idempotency-Key`
handling at `:418,430,456`; client side at `client.rs:147,171`. 13 tests, 10 mutations each
reddening its named test.

Transport envelope: **REST `/v1` is role-gated but has no resume route and no idempotency handling;
stdio and WebSocket have none of the three.** Linux only.

### Half two — produce useful redacted health/log/support evidence: NOT MET

**`F24-C4-H1` — NEW, severity HIGH.**

```
/usr/bin/grep -rn "support_bundle" crates/ --include=*.rs
  crates/wcore-gateway/src/lib.rs:19                          pub mod support_bundle;
  crates/wcore-gateway/tests/support_bundle_redaction.rs:20   use ...
  crates/wcore-gateway/tests/support_bundle_redaction.rs:269  (doc comment)
```

**Three hits: one module declaration and two references inside its own test file. Zero production
call sites. No CLI verb.** The module implements structural elision plus exact-secret scrubbing, is
canary-tested and mutation-gated — and **no operator can invoke it**, because nothing outside its
test binary ever calls it.

Absence discipline, because this is a negative claim (LANE-BRIEF §3b-i):

- **Instrument proven alive on a known-positive, same shape:** `/usr/bin/grep -rln "acp"
  crates/wcore-cli/src` → **6 files**. The search finds a sibling subsystem's name in the CLI when
  it is there.
- **Concept search, not one keyword:** also searched `supportbundle`, `support bundle`,
  `diagnostic`, `bundle`, `doctor`, `redact`, case-insensitively, across `crates/wcore-cli/src`. The
  hits are a TUI `/doctor` diagnostics panel and a `/config` *"resolved config (redacted)"* view.
  **Neither is a support bundle an operator can hand to support.** `ReleaseVerifier::bundled()` is an
  unrelated homonym.
- **Queries recorded above so a reader can re-run them.**

This is the **same advertised-but-dead class** as `F24-MB-1` (`media_bounds()` read at exactly one
site, and that site a test) — the class with nine recorded instances in this programme. **It is the
tenth.**

Secondary, and much smaller: the suite's only **live** test, `live_bundle_canary`, is `#[ignore]`d
(`support_bundle_redaction.rs:272`) and runs only under `-- --ignored` with three env vars. It is
well built — it **FAILS rather than skips** when they are absent — but a plain
`cargo test -p wcore-gateway` exercises 4 offline tests and not the canary. So `24-03-SUMMARY.md`'s
"canary-proved" describes an opt-in gate. **Confidence downgraded**, independently of the decisive
fact above.

**Why the ledger got this wrong, and why it matters.** `CRITERIA-GAP-LEDGER.md:327` grades `24-C4`
**MET on Linux / HTTP+SSE only**, deriving it from `24-04-SUMMARY.md`'s evidence — which is entirely
about the **recovery** half. Nobody tested the **support-evidence** half against the shipped surface.
**This is the clearest vindication in this grading of the instruction not to inherit arithmetic**,
and it is the one item that keeps Phase 24 blocking a release.

---

## 6. Criterion 5 — MET-WITH-STATED-EXCEPTIONS

> **"Setup-to-recovery journeys pass on macOS, Linux, and Windows."**

**All three platforms drive the identical 17-step journey to a receipt that a verifier — proved able
to refuse — accepts.**

| platform | result | recovery |
|---|---|---|
| Linux | **17/17**, `LRC=0`, at `978f49d7` | `kill -9`, no manual start, `NRestarts=1`, new pid |
| Windows | **17/17**, at `978f49d7` | killed 44096, **no start command issued**, platform-started 46028 |
| macOS | **17/17**, `MACRC=0`, at `eba6e9d7` | killed 44903, launchd `LastExitStatus=9`, live pid 46344 |

Each receipt: **12 submitted / 12 arrived / 12 unique / 0 duplicates / 0 losses**, counted at an
independent sink that is its own OS process, with the verifier **hashing the binary itself** and
**deriving** duplicates and losses rather than trusting a reported figure.

**The gates were proved able to fail before their passes were believed** — the verifier was given a
wrong platform, a wrong commit, a truncated receipt and a one-byte-appended binary, and returned
rc=1 on all four; the three new Windows tests were run against a mutated implementation and all
three went red; the lane **caught a self-passing gate of its own, twice** (a `python3` heredoc
mutation that died on a quoting error while the suite reported green on unmutated code) and rewrote
it with a pre-flight occurrence count.

### The stated exceptions

**(a) The three receipts are not at one commit, so the phase's own `bind` verb refuses them
(`BIND_RC=1`).** Linux and Windows are at `978f49d7`; macOS is at `eba6e9d7`, an ancestor differing
**only in `.planning/` documentation**. The macOS CI artifact for `978f49d7` sat **queued 45 minutes**
behind the darwin runner pool and the lane stopped waiting rather than hang. This is a **provenance**
gap, not a coverage gap. **I grade it as an exception rather than ignoring it, because grading MET
while the project's own verifier returns rc=1 on the trio is precisely the inflation this verdict
exists to avoid.** Cost to close: one macOS artifact and one journey run.

**(b) `gateway install` on Windows needs an elevated token** (`F24-J-M1`), while the module
documentation still claims the mechanism was chosen because it "does not require elevation". On
measurement, it does. Non-blocking → BACKLOG.

**(c) Windows `gateway stop` is not durable while the task is registered** — the one-minute
repetition restarts the runtime. macOS makes the same trade (`KeepAlive` undoes `launchctl stop`);
systemd is the outlier. `uninstall` removes the supervisor, so drain-then-uninstall is unaffected.
→ BACKLOG, MEDIUM.

**The most valuable single result in this phase is in this criterion**, and it is a negative one:
the obvious, documentation-shaped Windows fix — add `<RestartOnFailure>` — **registers cleanly, reads
back cleanly through Task Scheduler's own `/query /xml`, and leaves the service dead 3m20s after
`taskkill /F`**. A reviewer checking the XML would have signed it off. Only killing the process and
watching for two minutes found it. Two further measurements each of which would have broken real
installs: `encoding="UTF-8"` is rejected while UTF-16-declared UTF-8 bytes are accepted, and
`%USERDOMAIN%\%USERNAME%` is rejected on a **workgroup machine**, so emitting a `<Principals>` block
would have broken install on **every non-domain-joined desktop**.

---

## 7. Merged vs pending — and the distinction that changes a grade

| lane | branch state | bears on |
|---|---|---|
| media-live | **merged** | C3 media (audio positive direction) |
| media-bounds | **merged** | C3 media (nine adapters' bounds now enforced) |
| msteams-attach | **merged** | C3 media (inbound attachments, 5/5) |
| reconnect | **merged** | C3 reconnect (discord + matrix, lost=0 duplicated=0) |
| gateway-surface | **merged** | C3 surface equivalence (42 cells, DIVERGENT=0) |
| **native-actions** | **`lane/24-native-actions` — PENDING** | C3 native actions |
| **e2e-product-smoke** | **`lane/e2e-product-smoke` — PENDING** | phase goal (install/run), no single criterion |

Both confirmed unmerged: `/usr/bin/git merge-base --is-ancestor <branch> HEAD` → false for each;
merge-base `75babf32` for both.

### The structural fact that matters, and that I nearly missed

**`lane/24-native-actions` changes ZERO files under `crates/`.** `git diff --name-status 75babf32
lane/24-native-actions` returns **10 files, all `A`** — one new driver script
(`scripts/f24-native-actions.mjs`), one report, one notes file, seven evidence artifacts.

**So "pending merge" here means the EVIDENCE is unmerged, not the capability.** The native-actions
clause it proves is exercising product code that is **already in the release candidate**. Merging
that branch would add no product behaviour whatsoever. **The `native actions` clause of `24-C3` is
therefore genuinely proven for the RC today** — a reader who discounts it as "unmerged work" would
be wrong, and I have graded it as proven with its evidence location flagged.

Its central claim — **zero advertised-but-dead across six adapters** — is a negative, so I checked
its instrument: the lane proved its census grep alive on a known-positive of the same shape
(`max_message_len` → 9 files) before reporting zeros, and it grades `not-supported` as a **distinct
verdict** from `not-fired`. That second control is the one that matters, because `send_typing`'s
trait default is a **silent `Ok(())` no-op** — any log-side or error-side instrument would report
"typing succeeded" on a platform with no typing API. Only platform-side counting separates them, and
that is what the lane did. **Accepted.**

### The brief's `12/12` is not what that report says

`lane/e2e-product-smoke` frontmatter: `steps-total: 14, steps-passed: 12, steps-failed: 0,
steps-not-reached: 2`. **12 of 14**, and the two not reached are **TUI on a real terminal** and
**Windows/macOS cold start**. Correcting per the never-inherit-arithmetic rule.

**I credit this lane to no criterion.** It is a general cold-start product smoke — first run, a real
turn, five tools, a genuinely contained sandbox, a skill, memory across a session boundary, MCP,
resume, SIGKILL cleanup. It touches neither gateway lifecycle nor automation nor channels nor the
typed API. It is strong evidence the product is not hollow; it is not C1-C5. Two MEDIUM findings,
both about what the user is told. Its most useful disclosure is about itself: **four instrument
defects, three of which produced FAIL verdicts on correct product behaviour, one a false security
escape** — had run 1 been reported, four false findings would have been filed.

---

## 8. Does Phase 24 still block a release?

**Yes — on exactly one item, and it is small.**

### The one blocker

**`F24-C4-H1` — no operator verb for the support bundle.** It blocks for three compounding reasons:

1. **It is in the goal sentence.** "install, run, automate, connect, inspect, recover, and **support**".
2. **It is advertised-but-dead**, the class this programme has been burned by nine times, and it
   would ship inside a criterion currently recorded as **MET**.
3. **A shipped product whose users cannot produce a redacted diagnostic bundle generates support
   load that cannot be discharged.** The redaction logic already exists and is tested — the failure
   is purely that nothing calls it.

**Cost: ~0.5 lane-session.** A `gateway support-bundle --out <dir>` verb calling the existing module,
plus one live drive that produces a bundle and runs the existing `live_bundle_canary` against it with
its three env vars — which also converts that opt-in gate into an exercised one.

### What does NOT block, and must not be quietly re-added

Stating this explicitly, because treating every open criterion as blocking is what turned Phase 20
into a 74-plan loop lasting two weeks.

- **`24-C2`'s missing webhook and poll planes** — no longer block. The silent-acceptance defect that
  made them dangerous is **fixed**: both are refused at `add`, gone from `--help`, and legacy jobs
  list as `WILL NEVER FIRE`. A missing feature that says so is a can-ship-open gap. Ship the trigger
  vocabulary as documented.
- **`24-C3` on macOS and Windows** — a coverage gap, not a known defect. Ship with a **declared
  platform envelope**: channels are proven on Linux.
- **`24-C4`'s stdio/WebSocket transports** — HTTP/SSE is the transport the Desktop app uses, and it
  is the one that is met. Declare the transport envelope in the release notes.
- **`24-C5`'s `bind` provenance gap** — one macOS artifact. Real, cosmetic in risk terms.
- **7-of-10 outbound idempotency** — the seven **abandon rather than duplicate**, fail closed, and
  the abandonment is now nameable. Correct behaviour pending a better one.

### The honest meta-point

**Phase 24 has been called the last release blocker for a week. Its own evidence stopped supporting
that claim around 2026-07-28**, when `24-C5` closed on all three platforms and `24-C2`'s
silent-acceptance defect was retired. The label persisted because nobody graded the phase as a
whole — which is the specific cost of a phase running twenty lanes with no verdict file. The one
thing that genuinely does block was **not on anyone's list**, because it sits inside the one
criterion the ledger recorded as MET.

---

## 9. Costed gap list

Per unmet criterion: the missing capability in one line, estimated lane-sessions, credential needed.

| # | criterion | missing capability | sessions | credential? | blocks release? |
|---|---|---|---|---|---|
| 1 | **24-C4** | **A CLI verb that produces the support bundle** (module exists, nothing calls it) | **0.5** | **no** | **YES** |
| 2 | 24-C4 | Resume + idempotency on REST `/v1`, and all three on stdio/WebSocket | 2 | no | no |
| 3 | 24-C2 | `webhook` producer — inbound HTTP route reusing the new event bus + the `require_auth` admission path the stored flag already anticipates | 1.5 | no | no |
| 4 | 24-C2 | `poll` producer — an egress-routed client, and first a *defined* response contract | 1 | no | no |
| 5 | 24-C2 | Continuation gate — hard-kill a gateway mid-fire, count at an out-of-process sink | 0.5 | no | no |
| 6 | 24-C3 | Email inbound fixture seam (6 cells NOT MEASURED; email is a designated *reference* adapter) | 1 | no | no |
| 7 | 24-C3 | Reconnect on the 5 unreached adapters — telegram, email, signal, msteams, imessage | 1.5 | no | no |
| 8 | 24-C3 | Inbound vision/image half — `F24-C3-H7`: `build_vision_backend()` takes no `&Config` and reads only ANTHROPIC/OPENAI/GEMINI, while transcription got two extra arms | 1 | **yes** (a vision-capable key; `FLUX_API_KEY` was measured working over the OpenAI chat wire) | no |
| 9 | 24-C3 | Outbound media round-trip — untouched on every adapter | 1 | yes | no |
| 10 | 24-C1/C3 | Outbound idempotency for the remaining 7 adapters | 2 | no | no |
| 11 | **24-C3** | **The whole 8-clause matrix on macOS and Windows** — currently zero of eight on either | **4-6** | no | no |
| 12 | 24-C5 | `bind` provenance — one macOS artifact at `978f49d7`, one journey run | 0.25 | no | no |
| 13 | 24-C2 | macOS automation evidence (none exists) + the PTY surface test | 1 | no | no |

**Total to close every gap: ~17-19 lane-sessions.** **Total to unblock the release: 0.5.**

Item 11 is the largest single line and the one to be most deliberate about: it is 4-6 sessions to
extend a Linux-proven matrix to two more OS families, and it buys coverage rather than fixing a known
defect. It is the right call to ship without it **only if the platform envelope is declared** —
which is precisely the mistake Phase 20A made by not declaring one.

---

## 10. Ungradeable, and other things I could not establish

**(a) Whether any full-workspace suite genuinely passes at this commit.** Both instruments named in
my brief are bad here: nextest "flakiness" on this host was **fd/inotify exhaustion** (40 runs, zero
real failures) and `no-tests = "fail"` is **silently ignored** by the installed nextest, so a green
suite may have run nothing. I therefore **did not** run or cite a workspace suite. Every number in
this verdict comes from a per-crate count read back with its `filtered out` field, or from a live
drive against the real binary. `24-C5-FINISH-SUMMARY.md` §7 item 5 records that workspace suites and
clippy were **not** run on either host — that remains open for an integrator, and I did not close it.

**(b) The 6 email inbound cells.** NOT MEASURED, and not measurable by any configuration-only
mechanism today. Not a defect finding — a stated inability.

**(c) Whether `live_bundle_canary` has ever actually been executed.** It is `#[ignore]`d and
env-gated. I found no evidence capture of a run. Given `F24-C4-H1` — there is no verb to produce the
bundle it would check — I think it very likely has not, but I did not prove that and do not claim it.

**(d) macOS and Windows behaviour on `24-C3`.** Zero evidence exists. This is unmeasured, not
failing; nothing should be inferred in either direction.

**(e) A finding-ID collision that should be resolved by whoever owns the register.** **`F24-C3-H6`
is used for two different findings**: the matrix `/sync` cursor loss (HIGH, `24-MATRIX-SIGNAL`, fixed
by `24-H6`) and the decorative `media_bounds()` API (MEDIUM, `24-MEDIA-ACTIONS`, fixed by
`24-MEDIA-BOUNDS`). Both are closed, so nothing is lost — but the register is ambiguous and a future
reader tracing "F24-C3-H6" will find two answers.

### One instrument defect of my own, repaired in-lane

Per LANE-BRIEF §6b-ii, a documented instrument defect is a defect one has agreed to keep, so this
was repaired rather than only noted. Measuring anti-vacuity on the support-bundle suite I ran
`/usr/bin/grep -c '#\[ignore\]'` → **0**. **That zero was false** — the attribute is
`#[ignore = "live: …"]`, so the literal `]` could never match. Repaired to `#\[ignore` and self-tested
with **three** assertions:

| # | assertion | result |
|---|---|---|
| A1 | known-positive: repaired matcher finds the ignore | **1** ✅ |
| A2 | known-negative: repaired matcher on `lifecycle.rs` | **0** ✅ |
| A3 | **the old broken matcher would have missed it** | **0** ✅ |

A3 is the assertion that proves the repair does something; without it the self-test passes on the
broken matcher too. Corrected result: 5 `#[test]`, 1 `#[ignore]`d. I record this because I produced
the exact failure I had written a note about ten minutes earlier — a known-negative that passes for
free on a dead instrument.

---

## 11. Fences and reserved actions

**Fence exposure vs `861d1b1a`: ZERO.**

```
BASE=861d1b1a716240165209336b1fa38d36f9445716
/usr/bin/git diff --stat "$BASE" HEAD -- crates/wcore-cli/src/lib.rs crates/wcore-cli/src/main.rs
  (empty)
/usr/bin/git diff --numstat "$BASE" HEAD | grep GRADE
  209  0  .planning/phases/24-…/24-GRADE-NOTES.md      <- live control, instrument alive
/usr/bin/git diff --name-only "$BASE" HEAD | grep -cE "^(crates|\.github)/"
  0
```

The empty fence diff is a real zero, not a dead instrument: the control in the same invocation
reports a non-zero line count for a file this lane did change.

**Not done, as fenced:** no merge to `main`, no PR, no tag, no publish, no issue closed, no
`wcore-contract generate`, nothing under `.github/workflows/*` or `crates/`. No credential read,
used, or transmitted — this grading required none. Nothing was compiled on the Mac.

---

_Graded by lane `grade-24`, 2026-07-29, at base `861d1b1a`. Measurement trail: `24-GRADE-NOTES.md`._
