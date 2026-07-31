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

## Open questions to settle

1. Rotate-on-open alone does NOT bound a long-running gateway process. Needs a
   size-checking writer, else the fix is cosmetic for the exact briefed scenario.
2. Machine-readable schema must gain a way to express a conditional guarantee
   without breaking `delivery_semantics_declaration.rs` drift enforcement.
3. Live: over-cap Matrix send + retry, count arrivals. Room
   `!kntRqkQCkPjhPvMMvf:matrix.org` ONLY, redact after.
