---
lane: lane/discord-live
criterion: 24-C3 (live Discord half)
date: 2026-07-30
base_sha: 43c69ca71bc788dcd925fc070204d6918c2d7e0f
status: complete
verdict: gap CLOSED for 4 of 5 capabilities; the 5th FAILED and the shipped doc was wrong
---

# 24-C3 live Discord — lane summary

**The sentence six lanes wrote — "no message was ever sent or received against a live
platform" — is no longer true.** Real bot, real guild, real channel, real messages.

## The five capabilities

| # | Capability | Verdict | What corroborated it |
|---|---|---|---|
| 0 | setup / auth | **PASS** | `channel probe` → `authenticated`, `identity: 1532224324075913297` — equal to the bot id read independently from `GET /users/@me` before the product ran |
| 1 | **send** | **PASS** | shipped binary, `cron add` + `gateway run`; independent observer saw `0 → 1` arrivals, author `WaylandCoreBot`, exact content |
| 2 | **edit** | **PASS** | production registration path; at Discord the content changed and `edited_timestamp` was stamped; known-negative (nonexistent id) → 404 |
| 3 | **delete** | **PASS** | message read back **404** afterwards, with a **200 known-positive in the same capture**; re-delete → 404 |
| 4 | **receive** | **PARTIAL** | transport proven live for the first time — `READY` + a real `MESSAGE_CREATE` with `content_len=50`. The human-authored last hop **NOT RUN**: the adapter drops all bot authors by design and I hold no human Discord account |
| 5 | **outbound idempotency** | **FAIL** | one delivery id, one `once:` trigger, a genuine kill-and-restart — **two messages arrived**. The doc was wrong and has been corrected |

Nothing was skipped silently. Capability 4's missing leg is stated with its reason.

## The headline: `docs/delivery-semantics.md` was wrong, and is now fixed

Discord was in the **exactly-once** column. It is **at-most-once**.

- **Discord does not deduplicate on `nonce`. There is no window.** Identical nonce, same channel,
  same author, replayed at **0 s / 5 s / 30 s / 90 s** → **two distinct message ids every time**.
  `BL-24C1-DISCORD-WINDOW` asked how long the window is; the answer is that there is none.
- **The token is accepted, not rejected** — `POST` 200 and Discord **echoes the nonce back**. So
  this is the platform's behaviour, not a malformed key.
- **End to end:** `once:` trigger (cannot fire twice) → attempt 1 reaches Discord carrying
  `nonce=wle82e6651cfa60bb8`, which is exactly
  `nonce_for_key("cron:97ce67c3-…:1785383566000")` → `kill -9` → restart → the gateway's own
  banner reads **`carried=1 (unattempted 0 / unknown-outcome 1)`** → **2 arrivals**, baseline 0.
  Not the `F24-GWP-H1` new-delivery-id confound: the product itself called it one carried
  unknown-outcome delivery.

**Why the false `true` was worse than an honest `false`:** it made
`LedgeredHandler::dispatch_fire` take the re-attempt arm (`automation.rs:216-220`) rather than
the abandon arm (`:201-215`). `wayland-core gateway abandoned` was empty before and after. The
product did not fail to notice a duplicate — **it created one deliberately**, on a guarantee the
platform does not provide. That is precisely the argument the doc's own §6 makes; Discord was
the row violating it.

Changed: `supports_outbound_idempotency() → false`; the doc's 3/7 → **2/8**, the Discord row,
the provenance table, the §5 Windows note, §6, the machine-readable block, and a new **§8**
recording the measurement. The nonce still goes on the wire — only the false guarantee is gone.

## Defects found

**`F24-C3-D2` (HIGH, FIXED) — the Discord inbound WebSocket could not connect at all.**
`rustls` 0.23 refuses to pick a backend when both `aws-lc-rs` and `ring` are linked (both are, in
this lock) and nothing installed a process-level `CryptoProvider`, so `connect_async` **panicked
on every attempt** — **84 panics in ~120 s** of one `gateway run`. The discriminating control:
outbound REST succeeded in the same process, isolating it to the WS TLS stack. **This is why
inbound was never proven by anyone.** Fixed via a non-clobbering `Once` install; panics **84 → 0**,
health **`Degraded` → `Healthy`**.

The associated false log line: after each panic the supervisor printed
`INFO channel reconnected; resuming polling`. Nothing reconnected, 84 times.

**`F24-C3-D1` (MEDIUM, NOT fixed — needs a product decision).** Discord implements
`edit`/`delete`/`react`/`typing` and `channel actions` will report them, but **no operator
surface invokes them**: zero call sites in `wcore-cli`, `wcore-gateway`, `wcore-agent`,
`wcore-tools`, `wcore-protocol` (known-positive in the same search: `.send_message(`, 6 hits);
`edit_on`/`delete_on` are called only from tests. A capability the product can advertise and
cannot use.

**`channel health` exits 0 while `Degraded`** (`HEALTH_RC=0` with `errors: 5`, `reconnects: 16`).
Credit where due — it did **not** falsely claim `Healthy` — but it cannot be used as the gate
`channel actions --require` is shaped to be. Not fixed; MEDIUM, noted for BACKLOG.

## Premises in my brief that did NOT hold

- **The ledger's "media and native actions remain untouched for every adapter"
  (`CRITERIA-GAP-LEDGER.md:824-825`, `:868`) is STALE for Discord.** Edit/delete/react/typing are
  declared and implemented at `wcore-channel-discord/src/lib.rs:465-472`.
- **"`docs/delivery-semantics.md` puts Discord in the exactly-once column" — TRUE, and the doc
  was already honest that the row was `No — mock only`.** I did not catch the doc lying; I
  replaced a labelled unknown with a measurement, and the measurement dissented.

## Both directions, for every instrument I relied on

| instrument | can it pass? | can it fail? |
|---|---|---|
| Discord observer (curl) | 200 on a real message | **404** on a nonexistent id, before any product run |
| delete corroboration | 200 known-positive, same capture | 404 on the deleted id |
| dedup comparator | two GETs compare equal; `PATCH` returns the same id | four DUPLICATE verdicts |
| log scans | known-positive counts (84, 4, 1) | known-negative `0` every time |
| doc/code enforcement gate | green after the rename | **went RED for real** when the capability flipped |
| secret sweep | known-positive control returns 1 | 0 across all 8 committed files |
| live action test | `1 passed; 0 failed; 0 ignored; 0 filtered out` | in-test known-negatives both 404 |

## Gates

- hetzner `6170d6d6`: `wcore-channels-registry + wcore-channel-discord + wcore-gateway +
  wcore-channels` → **304 passed, 0 failed, rc=0**.
- `wcore-cli --test f24_c1_outbound_idempotency` → **6 passed, 0 failed, rc=0**.
- `cargo fmt --all -- --check` → clean (Mac).
- **Shared-file fence exposure: ZERO.** `wcore-cli/src/lib.rs` and `main.rs` untouched; the only
  `wcore-cli` change is a comment repair in `tests/`.

## For the orchestrator to serialize

1. **`docs/delivery-semantics.md` is a customer-facing guarantee change.** It is enforced by
   `delivery_semantics_declaration.rs`, so it cannot silently drift — but any other lane that
   merges a Discord idempotency assumption will now conflict with it, deliberately.
2. **I did NOT edit `CRITERIA-GAP-LEDGER.md`** — five lanes are live in it and a re-grade there
   is a merge hazard. The 24-C3 re-grade this lane earns: **send/edit/delete PASS live,
   inbound transport PASS, inbound human hop open, idempotency FAILED-and-corrected**, plus the
   stale "native actions untouched" claim to retire.
3. **One human action closes capability 4**: with this build running, post any message in
   `#general`. No credentials, no code.

## What I did NOT do

No `main` merge, no PR, no tag, no release, no issue closed, no `wcore-contract generate`, no
`git rebase`/`reset --hard`/`clean`/`stash`, no `git add -A`. No Windows or macOS leg — this is
Linux-only, on hetzner. I did not fix `F24-C3-D1` (needs a product decision on which surface
should own edit/delete) or the `channel health` exit code.
