---
phase: 24-gateway-automation-channels-typed-api
plan: "04"
subsystem: typed-authenticated-clients
tags: [acp, roles, idempotency, cursor, resume, negotiation, criterion-4]
status: partial
lane: 24e
plans-not-executed:
  - "24-04 Tasks 1-4 (journey driver, Windows TUI decision, three platform journeys, acceptance panel) — NOT STARTED"
requires:
  - "24-01"
  - "24-02"
  - "24-03"
provides:
  - wcore_acp::roles::RolePolicy (server-side role assignment; the missing producer of RoledPrincipal)
  - "HttpHandler::authorize_method — the server decides every request before dispatch"
  - "AcpServer event plane: per-session EventLog, run-scoped stream identity, delivery-independent recording"
  - "GET /sessions/:id/events — the resume transport, three refusals with distinct statuses"
  - "REST /v1 authorizes as well as authenticates, against the same role table"
  - "HttpHandler::{create_session_idempotent, delete_session_idempotent} — Idempotency-Key on the request path"
  - "AcpClient::{resume_events, create_session_idempotent, delete_session_idempotent}"
  - "version negotiation on GET /initialize"
  - "wayland-core acp serve --role <viewer|operator|admin> (shipped binary)"
affects:
  - crates/wcore-cli/src/acp.rs (the shipped ACP server installs the policy and announces its state)
tech-stack:
  added: []
  patterns:
    - "recording is decoupled from delivery: a severed client keeps its record"
    - "a resumable position carries the identity of the PROCESS RUN that minted it"
    - "a refusal a client cannot branch on is a refusal the client will retry forever"
    - "the absence of a check is announced, never inferred from a quiet log"
key-files:
  created:
    - crates/wcore-acp/tests/typed_client_recovery.rs
    - crates/wcore-acp/tests/roles_and_idempotency.rs
  modified:
    - crates/wcore-acp/src/{roles,cursor,server,client,lib}.rs
    - crates/wcore-acp/src/transport/http.rs
    - crates/wcore-cli/src/acp.rs
decisions:
  - "message/send tees its events into the log from a drain task, so a client that disconnects mid-turn keeps the record it needs"
  - "stream ids carry a per-process-run uuid, so a pre-restart cursor is refused by name rather than served another stream's positions"
  - "an unrecognised route resolves to its own path and inherits the Admin default — an unclassified addition fails loudly"
  - "a handler that cannot honour an Idempotency-Key REFUSES it; ignoring it leaves the caller believing a retry is safe"
  - "a resume on a handler with no event log is UNSUPPORTED (501), never an empty list"
  - "the client maps the server's error CODE, not the HTTP status, so Forbidden and a named conflict survive the last hop"
  - "with no role policy installed the server gates nothing — the pre-role behaviour, pinned by a test and announced at startup"
metrics:
  tests-green: 148
  mutations-proved: 11
  completed: 2026-07-27
---

# Phase 24 Plan 04: Typed Client Plane Summary (lane 24e)

**24-03's Criterion 4 verdict — "a criterion that says clients *recover event
gaps* is not met by a module that *could* let them" — is overturned. A real
typed client now recovers a real gap over a real socket, and the shipped binary
does it too: a client that severed its connection having received ZERO bytes
recovered, twelve seconds later, an event the server produced entirely after it
had gone. 24-04's own four tasks were NOT STARTED.**

## Termination state

**Partial, with the boundary named.** This lane was directed at the gap 24-03
left rather than at 24-04's journey plan. Criterion 4's wiring is done, proved
and live-exercised. 24-04's journey driver, Windows interface-evidence decision,
three platform journeys and acceptance panel were not begun, and the phase is
NOT closed. Nothing was pushed to main, merged, tagged, released, or used to
close an issue.

## 1. What Criterion 4 was missing, and what closed it

24-03 built four correct, tested, mutation-proved modules and said of them:
*"The contracts exist; the plane does not."* Each had a specific missing half.

| Contract | What was absent | What now exists |
|---|---|---|
| `roles` | Nothing produced a `RoledPrincipal` — verifiers return a bare `Principal` and no role had a source, so `authorize` had no caller | `RolePolicy` assigns roles server-side; the transport decides every request before dispatch from the principal it verified |
| `cursor` | No `EventLog` was appended by anything, and no transport served a resume | `message/send` tees into a per-session log; `GET /sessions/:id/events` serves the resume |
| `idempotency` | No command path consulted the ledger | `Idempotency-Key` on session create/delete |
| `negotiate` | No handshake called it | `GET /initialize` negotiates and refuses below the floor |

### The design decision that matters: recording is decoupled from delivery

Returning the engine's stream straight to the client — which is what
`send_message` did — makes the log a function of what the CLIENT CONSUMED. Axum
drops the response stream the instant the peer disconnects, the engine's
remaining events are never polled, and they are never recorded. The client then
resumes and is told, correctly and uselessly, that there is nothing to resume.

**The events a severed client most needs are exactly the ones that path never
retains.** So `message/send` now drains the engine to completion in its own
task, appending as it goes, and feeds the live client from that drain. A severed
connection loses the DELIVERY and keeps the RECORD. This is mutation M1, and it
is the property the live probe measured from outside the process.

### The second one: a stream identity must name the process run

`stream_id` is `{session}@{instance_uuid}`, minted once per server construction.
A restarted server therefore issues ids no pre-restart cursor can match, so a
stale cursor gets a named `StreamMismatch` instead of being handed positions of
a different stream while believing itself continuous.

## 2. Verification

| Gate | Result |
|---|---|
| `cargo test -p wcore-acp --no-fail-fast` | **148 passed, 0 failed, 0 ignored** (rc=0) |
| — of which `typed_client_recovery` | 4 passed |
| — of which `roles_and_idempotency` | 11 passed |
| `cargo clippy -p wcore-acp --all-targets -- -D warnings` | rc=0 |
| `cargo clippy -p wcore-cli --all-targets -- -D warnings` | rc=0 |
| `cargo check -p wcore-agent --all-targets` | rc=0 |
| `cargo fmt --all -- --check` (Mac) | rc=0 |
| Mutation harness, 10 mutations | **ALL-REDDENED**, every restore SHA-verified |
| Mutation M11 (REST authorization) | baseline_rc=0, mutant_rc=101, restore SHA-verified |
| Seam vs **merge-base** `a0e8bbee`, paths inlined | rc=0, control on `crates/wcore-acp` returns **1** |
| `wcore-cli/src/{lib,main}.rs` fence | **0 diff lines** |
| `Cargo.toml` / `Cargo.lock` | **untouched** |

No gate here terminates in a pipe. The compile gate itself was proved able to
fail before its green was trusted: a deliberate syntax break returned **101**,
the restore was SHA-verified, and the baseline returned to 0.

## 3. Eleven mutations, each reddening its named test

| # | Removed behaviour | Test that reddened |
|---|---|---|
| M1 | the tee — return the engine stream directly | severed client recovers the gap |
| M2 | the run identity in `stream_id` | wrong-stream cursor refused |
| M3 | the middleware's `authorize` call | role refusal vs auth failure |
| M4 | unclassified route falls to least privilege | unclassified route refused |
| M5 | the ledger replay arm on create | one session from a repeated key |
| M6 | the default idempotency refusal | handler refuses a key it cannot honour |
| M7 | `ResumeError::Unsupported` → empty list | same test, the 501 leg |
| M8 | the client's `Forbidden` mapping | role refusal reaches the client typed |
| M9 | `NoSuchSession` → empty list | unknown session refused |
| M10 | the three cursor statuses collapsed to 400 | evicted cursor names where to resync |
| M11 | the REST surface's authorize call | REST cannot walk around a role refusal |

Every one recorded a **baseline first** (a mutant reddening an already-red test
proves nothing), every replacement **asserted it applied** (a mutation that did
not apply is indistinguishable from one the test missed), and the harness
**required the baseline to have executed at least one test**, parsed from
cargo's own summary — the F24-D-P1 shape, where a filter matching zero tests
exits 0.

**M2 SURVIVED on the first run, and the finding was in my own test.** The stale
cursor was a hand-written string, and any invented string mismatches anything —
including on a server whose stream ids carried no run identity at all. The test
proved the CHECK and said nothing about the IDENTITY. It now mints the stale
cursor from a SECOND server instance, which is the only form that fails when the
run identity is dropped.

## 4. Live evidence — the shipped binary, not the suite

`wayland-core 0.12.25`, release build, sha256
`7624e43206df22fa03e3de81a4defa539c6f5924d3d946e0f51c2b756b5e9d1d`, driven from
outside the process by `curl`. Every status captured into a variable on the line
after the command; body and status kept as separate artifacts so neither can
stand in for the other.

**The two refusals, one server, same route:**

```
GET  /sessions              -> 200   (positive control: a viewer may read)
POST /sessions              -> 403   {"code":-32005,"message":"forbidden: session/create requires role operator; caller holds viewer"}
POST /sessions (bad key)    -> 401   {"code":-32002,"message":"authentication error: api key mismatch"}
```

The refusal names what is required and what is held, and does NOT name the
principal.

**The resume plane, severed:**

```
severed after 0.4s; live client received bytes=0
B: events appeared after ~12s
B: recorded_after_disconnect=1 next_position=2 positions=[1] duplicates=0 ordered=True
```

The client received **nothing** and disconnected. The event was produced
**after** it had gone, was recorded anyway, and was served on resume. Under the
pre-wiring behaviour that event would never have been polled and the resume
would have been empty forever.

**The stream identity and the refusals:**

```
GET events (wrong stream)  -> 409  data:{"error":"stream_mismatch","actual":"…@c9ae2339-…","requested":"…@wrong-run"}
GET events (ahead)         -> 400  data:{"error":"ahead","next":1,"requested":99999}
```

**Idempotency, counted from the session list rather than from the ledger:**

```
POST /sessions (key, x2)    -> 200 then 200, id1 == id2 == e59d7efc-…
POST /sessions (key, other) -> 400  "…already bound to a different command…"
sessions_now=2
```

**Negotiation:** `X-ACP-Protocol-Version: 9.4` → 200 with
`x-acp-negotiated-version: 0.1` and `x-acp-client-is-newer: true`; `0.0` → 400,
*"client protocol 0.0 is below this server's minimum; upgrade the client to at
least 0.1"*.

**Startup states, both announced:** `role gating ENABLED` / `role gating NOT
CONFIGURED`. `--role superuser` exits **1** at startup, and the refusal was
**attributed** to the role by grepping for its own message — the first version of
that control passed for the wrong reason (a missing provider key), which is the
same self-passing shape as a gate that was already red.

**No real credential was used, read, copied or printed.** The provider key is a
synthetic literal and no turn reached a vendor.

## 5. Findings

| ID | Severity | Status |
|---|---|---|
| **F24-E-H1** role gating was bypassable via the REST `/v1` surface on the same server | **HIGH** | **FIXED**, found live, re-measured live, mutation-proved (M11) |
| **F24-E-P1** `rsync -a` preserves mtimes, so a tree synced back after a mutation harness can leave cargo running the MUTANT binary | **HIGH (process)** | **FIXED** in the sync path; measured, see below |
| F24-E-M1 a role refusal reached the typed client as an anonymous transport error | MEDIUM | **FIXED** — client maps the server's error code |
| F24-E-M2 an idempotency CONFLICT reached the typed client as an anonymous `HTTP 400` | MEDIUM | **FIXED** — same change |
| F24-E-L1 `acp serve` refuses to start without an LLM provider key, though session, resume and initialize need no engine | LOW | BACKLOG |
| F24-E-L2 the live turn emitted ONE event on this host, so multi-event ordering is proved only in the suite | LOW | BACKLOG — stated in §6 |

No CRITICAL. One HIGH product defect, found by driving the shipped binary and
fixed with live re-measurement; one HIGH process defect in my own harness
plumbing, also fixed.

### F24-E-H1 in full — the control that guarded one of two doors

`acp serve` merges the ACP router and the REST router onto ONE listener over
ONE `AcpServer`, and passes the SAME verifier to both. The previous commit added
authorization to the ACP surface only. Measured with `--role viewer`:

```
POST /sessions              -> 403  forbidden: session/create requires role operator; caller holds viewer
POST /v1/sessions           -> 200  {"session_id":"59de47b2-5e8b-4048-9e4b-6ab9bc33652a"}
POST /v1/sessions (bad key) -> 401  (control: REST DOES authenticate)
```

The control proves this was specifically an AUTHORIZATION gap, not a missing
middleware. An operator who set `--role viewer` believing they had restricted a
client had restricted nothing, and no surface in the product would have told
them.

**I introduced the reachable form of this defect** by shipping a role control in
the previous commit that covered one of the two doors. It was found because the
live probe drove the real binary rather than stopping at a green suite.

Fixed by having the REST middleware authorize against the SAME method-name
strings — one role table, because two would drift and the drift would look
exactly like this again. Re-measured after the fix:

```
POST /sessions      -> 403
POST /v1/sessions   -> 403
POST /v1/sessions (bad key) -> 401
GET  /v1/sessions (viewer)  -> 200   (the fix gates by role, it does not shut the surface)
```

### F24-E-P1 in full, because it can manufacture a false GREEN

Two integration tests reported red. The source was correct on both hosts. Cause:
`rsync -a` set the synced file's mtime to the Mac's (`14:14:55`), which was
OLDER than the build artifact cargo had produced from the **M10 mutant**
(`14:19:15`). Cargo concluded nothing had changed and re-ran the mutant binary.
Confirmed by hypothesis test: `touch`ing the sources produced `Compiling
wcore-acp` and 4 passed.

It surfaced as a false red. **The same mechanism produces a false GREEN whenever
the stale binary is the permissive one** — a mutation harness plus an
mtime-preserving sync can silently certify a tree nobody built. Every result in
§2 and §3 was re-run from a freshened tree afterwards. The harness's own results
were never at risk: it writes each mutant with a current mtime, so cargo always
rebuilt inside it; only the external sync could go backwards.

This is a new entry for the standing self-passing list: **an artifact newer than
its source is a build that did not happen.**

## 6. What was NOT delivered — stated plainly

1. **24-04's four tasks were NOT STARTED.** No `scripts/f24-journey.mjs`, no
   `wayland-journey` binary, no receipt schema, no Windows interface-evidence
   measurement or panel, no platform journeys, no acceptance panel, no
   `24-04-JOURNEY-RECEIPTS.md`, no `24-04-TUI-EVIDENCE-DECISION.md`. **Phase 24
   is NOT closed and no requirement was marked complete.** Success Criterion 5
   is untouched by this lane.

2. **No macOS and no Windows evidence.** Everything here is Linux (hetzner) plus
   `cargo fmt` on the Mac. This is the same gap 24c and 24d reported and it is
   **not closed**. It is a budget outcome, not an impossibility.

3. **The live turn recorded ONE event, not a long ordered run.** The engine on
   that headless host fails fast (`no OS keyring … no encrypted vault`), so the
   turn emits a single honest `error` frame. The delivery-independence property
   is fully proved by it — the event was produced after the client had gone —
   but live multi-event **ordering, duplicate and loss counting** is not. Those
   rest on `typed_client_recovery.rs`, which counts 13 events across a real
   severed socket. **Unit-and-integration evidence is not offered as a
   substitute for the live count; the live count is simply absent.**

4. **`TooOld` is wire-proved only in the suite**, using
   `with_event_retention(4)`. The live probe did not emit enough events to
   evict.

5. **Roles are bound to ONE principal on the shipped binary.** `acp serve` has a
   single api-key identity, so `--role` sets that identity's role.
   `RolePolicy::grant` supports many principals and multi-principal
   configuration has no CLI surface.

6. **The stdio and WebSocket transports were NOT wired.** REST `/v1` now
   AUTHORIZES (F24-E-H1) but still does not record or resume: there is no
   `/v1` resume route and no `Idempotency-Key` handling on `/v1/sessions`, so a
   REST-only client is gated but cannot recover a gap. `transport/stdio.rs` and
   `transport/ws.rs` were not touched at all and neither authorizes, records nor
   resumes. Whether either is reachable in a deployment was NOT investigated,
   and that absence is itself unmeasured — see BACKLOG.

7. **The event log is in-memory and per-process.** A restart loses history; the
   run-scoped stream id makes that loss LOUD rather than silent, which is the
   contract, but no persistence was added.

8. **24-02's CONTINUATION and SURFACE gates remain unattempted** — the third
   lane in a row to leave them so.

9. **The PTY surface gate was not attempted.**

## 7. Deviations

**[Scope] This lane executed the gap 24-03 named rather than 24-04's plan.** The
orchestrator directed it at Criterion 4. 24-04's tasks are recorded as NOT
STARTED in §6 rather than partially attempted.

**[Rule 3 — blocking] Files outside 24-04's `files_modified` were edited.** All
of `crates/wcore-acp/**` and `crates/wcore-cli/src/acp.rs`. 24-04's declared file
set belongs to the journey driver, which this lane did not build; the files
edited are the ones 24-03 declared for the work it left unfinished.

**[Deviation] `crates/wcore-cli/src/acp.rs` gained a `--role` flag.** Without it
roles would be enforced by the server and unreachable from the product — the
same defect one layer up.

**Shared-file fence honoured.** `crates/wcore-cli/src/{lib,main}.rs`: **0 diff
lines** against the merge-base. `Cargo.toml` and `Cargo.lock` untouched.

**Base not merged forward.** Based at `a0e8bbee`; gates were run against the lane
tree. An integrator should re-run them post-merge.

## 8. Verdict — Success Criterion 4

**Criterion 4 (typed authenticated clients): MET on Linux, with two named
limits.**

A typed client authenticates, is refused by ROLE distinctly from being refused
by CREDENTIAL, issues idempotent commands whose repeat produces one effect and
two identical receipts, negotiates a protocol version or is refused by name, and
**recovers an event gap** — over a real socket, against the real server, from a
connection that was severed mid-stream, with duplicates and losses counted at
zero. The shipped binary does the same. The claim rests on 13 tests, 10
mutations each reddening its named test, and a live transcript.

The limits: **recovery and idempotency are proved on the HTTP/SSE transport
only** — REST `/v1` is now gated by role but has no resume route and no
idempotency handling, and stdio/WebSocket have none of the three (§6.6) — and
everything is on **Linux only** (§6.2).

**Criterion 5 (live journeys on three platforms): NOT ADDRESSED.** 24-04 was not
started.

**Phase 24: NOT CLOSED.** No requirement was marked complete.

## Self-Check

Every count, exit status, HTTP status, response body and mutation result above
was copied from captured tool output. The binary sha256 and version were read
from the host. The `recorded_after_disconnect=1` claim is the probe's own
printed line, and the "0 bytes received" figure is `wc -c` on the severed
client's output file. The M2 survival and the F24-E-P1 stale-binary diagnosis
are recorded as they happened, including the fact that the first version of the
bad-role control passed for the wrong reason, and including that the HIGH in §5
was a defect this lane itself introduced one commit earlier. The gates that do NOT pass —
macOS, Windows, the REST transport, live multi-event counting, and every one of
24-04's four tasks — are named as not passing rather than sampled.

**Self-Check: PASSED**
