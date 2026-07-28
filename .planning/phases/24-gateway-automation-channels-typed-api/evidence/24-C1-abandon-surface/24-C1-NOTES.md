# 24-C1 NOTES — abandon surface + adapter dedup tokens

Lane `lane/24-abandon-surface`. Base `1b2577e1b61447f1599e127679b8e2eb3552b61b`.
Worktree `/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-24-abandon-surface`.

Append after EVERY measurement. Never batch to the end (§6b-i).

---

## M1 — the brief's file path is wrong; the defect is real

Brief says `crates/wcore-channels/src/ledger.rs:214`. **There is no `ledger.rs` in
`wcore-channels`.** The real file is `crates/wcore-gateway/src/ledger.rs`, and the cited
line numbers land correctly there. Recording the correction rather than silently
retargeting.

Verified in-tree at base:

- `pending()` — `ledger.rs:214-220`, filter is `Accepted | Attempted`. `Abandoned` excluded. **CONFIRMED.**
- `pending_count()` — `:223-228`, same filter. `Abandoned` excluded. **CONFIRMED.**
- `compact()` — `:253`, `Abandoned` grouped with `Settled` as `terminal`, droppable past
  `retain_settled`. **CONFIRMED.**

## M2 — there are TWO abandon call sites, not one, and they differ

`grep -rn "\.abandon("` across `crates` returns exactly three hits (one is the definition):

| Site | What abandons | Operator surface today |
|---|---|---|
| `wcore-gateway/src/drain.rs:176` | forced drain past budget | `DrainReport.abandoned: Vec<String>` → printed by `wcore-cli/src/gateway.rs:1033-1036` |
| `wcore-gateway/src/automation.rs:179` | F24-C-H1 unknown-outcome, destination cannot dedupe | **nothing but `tracing::warn!` (`:182`)** |

So the brief's "no consumer anywhere outside `ledger.rs`" is **too strong for the drain
path and exactly right for the automation path.** Refining the claim rather than
inheriting it:

- The drain path has a surface, but it is **ephemeral** — it exists only in the stdout of
  the `gateway drain` invocation that caused it. An operator who was not watching that
  terminal cannot recover it afterwards.
- The automation path (`automation.rs:179`) has **no surface at all**. This is the one the
  brief's HIGH is really about, and it is the path that fires unattended, from a scheduled
  cron delivery after a crash — i.e. precisely when nobody is watching a terminal.

## M3 — the claim in the source

`automation.rs:172-175` states the delivery is *"ABANDONED rather than dropped: recorded,
terminal, and nameable by an operator."* `recorded` and `terminal` are implemented.
**`nameable by an operator` is not** — no query path reaches it. `automation.rs:551` asserts
in a test `"it is recorded terminally and nameable, not silently dropped"`, which tests
`ledger.state(id) == Abandoned` — an in-process ledger read, not an operator surface. That
assertion's message overclaims what it checks.

## M4 — two further losses in the record itself (found while reading, not in the brief)

1. **No reason is stored.** `Record` (`ledger.rs:89-97`) has `id`, `state`, `at`,
   `delivered`. There is no field for WHY. The two abandon sites have materially different
   causes (drain-budget expiry vs. unknown-outcome-no-dedupe) and the journal cannot tell
   them apart. The brief asks the surface to answer "why" — the data to answer it is not
   currently persisted.
2. **Compaction destroys `at`.** `compact()` (`:258, :265-270`) rewrites every retained
   record with `at: now` and `delivered: None`. So even the "when" an operator would get is
   the compaction time, not the abandonment time. This silently corrupts the timestamp of
   surviving records — worth grading on its own.

Neither is a reason to widen scope beyond the brief; both are directly load-bearing for
"the message, the destination, when, and why".

## M5 — destination is not in the ledger either

`Record` has no destination. The ledger keys on the caller-supplied delivery id only
(`ledger.rs:19-26` is explicit that the key is the delivery id and nothing rides the wire).
So "the destination" the brief asks for must either be derivable from the delivery id or
persisted at accept time. To establish before designing the surface.

---

## Still to establish

- [ ] Whether `fire.delivery_id()` encodes the target, i.e. whether destination is recoverable
- [ ] Matrix `txn_id` seeding — `wcore-channel-matrix/src/rest.rs:13,47`
- [ ] Discord nonce "distinct across restarts"
- [ ] `supports_outbound_idempotency` trait default and implementors
- [ ] Task 3: whether a restarted Matrix counter causes a homeserver to DROP a new message
