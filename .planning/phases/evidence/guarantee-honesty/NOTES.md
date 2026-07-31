# lane/guarantee-honesty — NOTES

Base `c9ab048b`. Two independent honesty-of-the-record items.

## Premise verification at base (all measured, not assumed)

Instrument controls: `/usr/bin/grep -rn --include='*.rs'` → 47 hits for
`supports_outbound_idempotency`, **85** for the known-positive `fn send_message`,
**0** for a known-absent needle. Instrument alive in both directions.

My FIRST grep was itself defective twice and both were repaired before use:
zsh ate the unquoted `--include=*.rs` (returned "no matches found" — the §3b-i
trap verbatim), and a `cap` needle matched "capability". Repaired invocation is
the one above.

### (A) Matrix exactly-once precondition — CONFIRMED OPEN

- `manager.rs:776-793` verbatim as scoped: key rides ONLY the `chunks.len() <= 1`
  arm (`:790`). Multi-chunk loop `:800-812` calls `send_message(part)` with no key.
- `manager.rs:751-756` `supports_outbound_idempotency` forwards a per-adapter,
  **cap-blind** bool. Trait default `lib.rs:144-146`.
- Matrix cap `Some(32_768)` at `wcore-channel-matrix/src/lib.rs:165-167`;
  declares `true` at `:294`.
- `docs/delivery-semantics.md`: message-length-cap vocabulary
  (`max_message_len|chunk|over-long|oversize|32_768|truncat`) = **0 lines**.
  Control: same expression style on `manager.rs` = **25**. So the absence is real.
  Machine-readable block (`:549`) carries the bare label `matrix = exactly-once`.

### NEW, not in the scoping brief — the sharpest instance

The per-adapter answer is not merely documentation drift; it is **printed to the
user as a false statement**:

- `wcore-cli/src/gateway.rs:958` `let dedupes = manager.supports_outbound_idempotency(..)`
  then `:963` `send_to_keyed(...)` with the SAME text, then `:984-992` prints
  `replay-safe: yes — the destination honours the delivery key`.
  For an over-cap body the send chunks and drops the key, so `gateway resend`
  **prints "replay-safe: yes" about a send that was not replay-safe.**
- `wcore-agent/src/cron.rs:182` `dispatch_is_idempotent` feeds the retry decision
  from the same cap-blind bool, and `Target::Channel` already carries `text`,
  so a truthful per-message answer is available at both call sites.

### (B) Log rotation — CONFIRMED OPEN

- `wcore-cli/src/main.rs:861-878` plain `OpenOptions::create+append`. No size cap,
  no generation count.
- `main.rs:1187` `let log_to_file = will_enter_tui || !rust_log_set;` → with
  RUST_LOG unset EVERY mode (headless `-p`, REPL, `--json-stream`) appends.
- `main.rs:1189` `open_tui_log_file().ok()` degrades to stderr-only at `:1226-1229`.
  That fallback has **zero test references** (only the two production call sites).

Note main.rs is a §6 FENCED file — edits must stay additive and minimal.

## Open questions — settled 2026-07-31 by the successor run

The run that wrote everything above was interrupted after `c8b062d6`. A successor
started fresh from integration `0675c051`, found this branch **on push** (the push
was rejected as non-fast-forward), and merged rather than force-pushed. It had
independently reached the same three conclusions and even the same label,
`exactly-once-below-cap`. Where the two overlapped the earlier B1 was kept, because
its `chunks_for` factoring makes the send and the query share ONE cap decision — the
successor had reimplemented the check beside the send, which can drift.

1. **SETTLED — rotate-on-open is insufficient, as suspected.** `wcore-cli/src/log_rotate.rs`
   checks the bound inside `Write::write`, so a long-lived gateway rotates mid-process.
   Proven in both directions: `rotation_discards_the_oldest_and_keeps_the_newest`
   asserts a rotation happened AND that the newest bytes are what survived, and
   `no_rotation_below_the_bound_and_nothing_is_discarded` inverts every one of those
   assertions under a large bound. 4 tests run, 4 passed on hetzner-dsm.
2. **SETTLED — `exactly-once-below-cap` plus a `<platform>.cap` line.** The label is not
   decorative: an adapter carrying it must declare a real `max_message_len()`, the number
   must match, and a bare `exactly-once` row must belong to a capless adapter. 14 tests
   run, 14 passed, including four negative controls.
3. **NOT RUN — blocked on a dead credential, and this is the one thing the lane could
   not close.** `matrix_cap_replay.rs` is written and committed, with the mandatory
   below-cap control in the same session (without it, "2 arrivals above the cap" is
   equally explained by retry always duplicating). matrix.org answered the first
   authenticated call `M_UNKNOWN_TOKEN — "Token is not active"`. A working token is a
   Sean-only input.
   - **Nothing was written to the room.** The failure is at the baseline read, which
     precedes the first send; neither `MCR_CTRL_RECEIPTS` nor `MCR_SUBJ_RECEIPTS` printed.
   - The run still measured OUR half before it died:
     `MCR_BODY ctrl_chars=51 ctrl_chunks=1 subj_chars=36814 subj_chunks=2`,
     `MCR_PREDICTED ctrl=true subj=false`. The production manager answers `true` for a
     body that will carry the key and `false` for one that will not.
   - `docs/delivery-semantics.md` was corrected to say so: the §2 evidence cell now reads
     "BELOW the cap: Yes … ABOVE the cap: NOT MEASURED". Leaving the old unqualified
     "Yes" beside a two-part guarantee would have been the same evidence-vs-claim
     mismatch the Slack correction in that document diagnoses.

## B2.2 — the decision, and what it was judged against

**A one-shot headless run keeps writing a trace file by default.** The defect was the
missing bound, not the file; `log_rotate` supplies the bound. Full reasoning lives at
the decision point in `wcore-cli/src/main.rs`, next to `log_to_file`.

Judged against the gateway case specifically, because that is where the question is
sharp. A host answering channel messages runs headless CONTINUOUSLY, so "headless
writes a trace by default" is most expensive exactly there — and most necessary: that
host has no terminal anyone is watching, no TUI to route traces to, and its failures
(a channel that stopped polling, a credential that expired, a delivery abandoned at
04:00) are found hours later from the record or not at all. Defaulting headless to no
file would make the gateway the only mode of the product with no diagnostics
whatsoever, which is the "trace record existing nowhere" state the original change
ended. The cost it was actually challenged on — unbounded growth on a continuous host
— is answered by capping the directory at 2 × MAX_LOG_BYTES, not by removing the file
(B2.4). `RUST_LOG` remains the lever for anyone who wants the old stderr behaviour.

## B2.3 — the fallback is now observable, and the control was run

The open-failure path was not merely untested, it was **unobservable**: it degraded
silently, so "the run still exits 0" was equally consistent with logging being dead,
disabled, or never attempted. It now prints `LOG_FALLBACK_NOTICE`.

Negative control, run on hetzner-dsm: deleting the `eprintln!` from `main.rs` and
rebuilding makes `the_binary_survives_an_unopenable_log_dir` FAIL on the missing
notice **while the process still exits 0** — which is precisely the false green. The
mutation was reverted (`git checkout --` verified).

## B1.4 — recorded, not fixed

`max_message_len` is asserted against its own literal at six sites and untested at two.
Matrix's is now machine-checked against the constructed adapter, which closes doc-vs-code
drift but still compares two numbers we wrote. The residual — does a declared cap equal the
*platform's* limit — needs live credentials for eight platforms.
Filed as **FerroxLabs/wayland#934**.
