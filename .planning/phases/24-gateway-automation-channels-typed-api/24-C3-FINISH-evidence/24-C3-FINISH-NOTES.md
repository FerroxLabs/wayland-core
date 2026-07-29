# 24-C3-FINISH — running notes (append-only, committed per §6b-i)

Lane `lane/24-c3-finish`. Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-c3-finish`.
Merge-base captured once at start: **`8bcb052b2aa6b1a9e3f2ed00af935a58c92c1f11`**
(= `plan/f20-unified-audit-repair` at fetch time). Every fence diff in this lane is
taken against that SHA, never against the branch name.

---

## T+0 — what I inherited, read from the four prior summaries

The criterion (`ROADMAP.md:119`) has **eight clauses**:

> setup/auth, access, routing, media, native actions, idempotency, reconnect/reload, health

State on arrival, per clause, aggregated across the five driven adapters:

| clause | state on arrival | proven on |
|---|---|---|
| setup/auth | PROVEN | slack, whatsapp, sms, telegram, discord (fixtures mint + ENFORCE their own token) |
| access | PROVEN | same five (telegram/discord with positive admit controls) |
| routing | PROVEN | same five |
| media | **UNTOUCHED** | — |
| native actions | **UNTOUCHED** | — |
| idempotency | PROVEN (inbound dedupe) | same five |
| reconnect/reload | **UNTOUCHED** | — |
| health | **UNTOUCHED** | — |

So **4 of 8 clauses are untouched on the inbound path, for every adapter.** That is the
single largest remaining block, and it is bigger than the adapter axis: adding signal or
matrix would add adapters to clauses already proven, not new clauses.

Adapter axis on arrival:

| adapter | transport | driven? | legs |
|---|---|---|---|
| slack | webhook | yes | 5/5 |
| whatsapp | webhook | yes | 5/5 |
| sms | webhook | yes | 5/5 |
| telegram | poll | yes | 5/5 |
| discord | websocket | yes | 6/6 incl. steady |
| email | poll (IMAP) | HALF — admission proven, reply half blocked at TLS | route/bind NOT MEASURED |
| signal, matrix, msteams, imessage | ? | **never driven** | — |

## T+0 — priority chosen, and why

Ordered by clauses-closed-per-session, with the two cheap determinations pulled forward
because they are deliverables in themselves (§6b-i: a measured refusal is a result) and
because one of them may collapse several adapters onto one seam shape:

1. **P1 — seam survey for signal / matrix / msteams / imessage.** Pure source read, cheap.
   Telegram's seam cost *zero* Rust (the field already existed); Discord's cost two config
   fields and ~3 sessions because its transport is a WebSocket. Establishing which of the
   four are telegram-shaped vs discord-shaped is high-information per minute, and the brief
   explicitly asks for the cost estimate rather than the build.
2. **P2 — email reply-half reachability determination.** The brief is explicit: establish
   whether it is reachable, and if not say what it would take, rather than forcing it. Two
   facts are already MEASURED and must not be re-derived or conflated:
   - Linux/OpenSSL honours a child-scoped `SSL_CERT_FILE` for IMAP — that already worked;
   - macOS `native-tls` = Security.framework — it does not;
   - **SMTP on EVERY platform resolves to `webpki-roots`** — compiled-in, reads no file and
     no env var. Proven executably (IMAP accepted a cert, SMTP refused the identical one
     0.6 s later, 82 identical sessions).
   My job is the *third* question those two leave open: is there a config-level or
   dependency-level knob, and what is the exact cost of adding one.
3. **P3 — re-measure `gateway run`.** Cheap, and re-validates the lease + starvation fixes
   that landed overnight and have not been exercised on the gateway surface since. Doing it
   early de-risks everything downstream: if `gateway run` regressed, every json-stream
   figure I take afterwards is measuring the wrong surface.
4. **P4 — the four untouched clauses** (media, native actions, reconnect/reload, health).
   Biggest clause win, largest build. Taken last because P1-P3 are bounded and P4 is not.

**I will not record 24-C3 as MET unless every clause genuinely is.** Four lanes before me
correctly declined. On present state that is the near-certain outcome again, and saying so
early is not defeatism — it is the thing that stops a premature MET on the last release
blocker.

## T+0 — traps I am carrying forward (from the brief and the prior summaries)

- **A green can be manufactured by universal denial.** The `access` leg once passed on all
  three webhook adapters *because everything was denied*. Every zero I report must be paired
  with a positive admit control **inside the pass condition**, not merely printed beside it.
  A green with zero arrivals grades FAIL.
- **Instruments carry the defect they hunt — 20+ recorded instances**, at least four of them
  in this criterion's own harnesses. Two had opposite sign: one under-counted 8 real arrivals
  as `replied=0` (MarkdownV2 escaping mangled the correlation token), one over-counted a
  duplicate that was really the driver's own 90 s latency against a 60 s TTL. Any suspect run
  grades **INCOMPLETE via an explicit `instrument_fault` state, never LOSS**, and every
  instrument repair carries the three-assertion self-test (known-positive passes,
  known-negative fails, **and the old matcher would have missed it**).
- **§6b-ii: repair the instrument in the lane that finds the defect.** A written-up
  instrument defect is a defect you have agreed to keep; that exact sequence recurred once
  already on this program.
- **Byte-count every capture.** `${PIPESTATUS[0]}` after a pipeline returns empty on this
  host. Capture exit status on the line after the command.
- **Run test targets by file, never by filter.** `cargo test -p wcore-cli migrate` exited 0
  having run 0 tests. Always read the `N passed` count back.
- Contention on hetzner: a full-workspace run while other lanes build is not a measurement.
  Re-run the crate alone at the same commit before reporting any regression.

## T+0 — open questions this lane must answer

1. Do signal / matrix / msteams / imessage have a config-level base-URL seam already? If
   not, is each telegram-shaped (one field, ~0 Rust) or discord-shaped (two fields + a
   protocol fixture, ~3 sessions)?
2. Is the email reply half reachable *at all* as currently built? If not, what is the
   minimum change — a cert-source knob on the adapter, a feature-flag swap of
   `webpki-roots` → `rustls-native-certs`, or a publicly-chained cert?
3. Does `gateway run` still receive inbound after the lease + starvation fixes?
4. What do media / native actions / reconnect-reload / health even MEAN on the inbound path,
   and which are measurable with the fixtures that already exist?

---
<!-- append below this line after every measurement -->
