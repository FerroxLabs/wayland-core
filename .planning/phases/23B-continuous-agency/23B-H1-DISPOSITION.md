# 23B-H1 — disposition: ROOT-CAUSED AND FIXED on the write path

**Finding, as 23B-01 raised it:** a `wayland-core` run that exits normally can write a
session journal the product cannot read back. `--resume` fails with
`journal checksum mismatch at sequence 16` on a session `--list-sessions` still shows, and
every operator verb that reads the journal fails identically, so there is no repair path.
Measured 8/8 and 9/10 in two bursts under concurrent compile load on `hetzner-dsm`, 0/3
when the host was quiet. Confirmed pre-existing against a pristine `15971d1b` binary.

**Severity: HIGH. Disposition: FIXED (write path), with a named residual.**

---

## 1. What the error can and cannot mean

`JournalError::ChecksumMismatch` is the **third** of three checks
`session_journal.rs::verify_chain_from` runs, in this order:

1. the frame's own SHA-256, over the exact bytes on disk (`FrameDigestMismatch` otherwise);
2. `previous_checksum` linking this envelope to the last (`PreviousChecksumMismatch`);
3. `computed_checksum() == checksum`.

So a `ChecksumMismatch` proves checks 1 and 2 **passed**. The bytes on disk are byte-for-byte
what the writer wrote and the chain is intact. That eliminates torn writes, disk exhaustion,
partial flushes and interleaved appends — every one of those fails check 1 first.

`JournalEnvelope::create` hashes and then writes one immutable value, so the writer cannot
hash something different from what it writes. Exactly one mechanism remains: **serializing a
deserialized event does not reproduce the bytes it was deserialized from.**

## 2. The mechanism, reproduced deterministically

Both `effect_receipt` fields — `SessionEvent::ToolIntentRecordedV2` and `ToolState` — are

```rust
#[serde(default, skip_serializing_if = "Option::is_none")]
effect_receipt: Option<serde_json::Value>,
```

`Some(Value::Null)` therefore:

- **writes** an explicit `"effect_receipt":null`;
- **decodes** back to `None`, because serde maps a JSON `null` to `None` for any `Option<_>`;
- **re-serializes** to nothing at all, because `skip_serializing_if` then fires.

The recomputed hash covers different bytes than the stored one. The reader rejects a journal
the writer wrote correctly, permanently, from a run that exited normally.

Captured red, before the fix, in `crates/wcore-agent/tests/journal_envelope_roundtrip.rs`:

```
left:  ...,"effect_contract":{"kind":"opaque","reconciler":null},"effect_receipt":null}
right: ...,"effect_contract":{"kind":"opaque","reconciler":null}}
```

and end to end through the real `SessionJournal` writer and a fresh reader:

```
a journal the product just wrote must be readable back: ChecksumMismatch { seq: 1 }
```

**Reachability is not theoretical.** The reducer refuses an effect receipt without a
reconciler, so the shape needs a tool whose effect contract names one — a
`FilesystemTransactional` or `ProviderIdempotent` tool. For those,
`JournalEffectScope::prepare_tool_with_effect_receipt` takes a bare `serde_json::Value` and
does not reject a null one. The wire contract independently blesses explicit nulls, pinned by
the pre-existing `known_explicit_event_defaults_are_wire_compatible_but_unknowns_fail_closed`,
so any producer writing through the documented contract — including the Desktop host — can
create an unreadable journal.

That also explains why the defect looked load-sensitive rather than deterministic: it needs a
run to get far enough to record a reconciler-bearing tool intent. 23B-01 measured the failing
journals at ~203 KB against ~71 KB for passing ones and read it as "the failing runs get
further through the turn" — which is exactly right, and is the tool-event boundary.

## 3. The fix

`skip_serializing_if = "is_absent_json_value"`, which skips `Some(Value::Null)` as well as
`None`, making the encoding a round-trip fixed point.

Nothing changes meaning. The wire contract already treats an explicit null as equivalent to an
absent field, and that test still passes unchanged: a null that is never written is a null
nobody can miss.

Red-before-green, on the build host, by reverting only the predicate and keeping the tests:

| Predicate | Result |
|---|---|
| `Option::is_none` (pre-fix) | **2 FAILED**, 3 passed — including `ChecksumMismatch { seq: 1 }` end to end |
| `is_absent_json_value` (fixed) | **5 passed**, 0 failed |

No regression: `wcore-agent --lib session_journal` 66/66, `session_journal_test` 48/48,
`session_journal_crash_matrix_test` 4/4.

## 4. Residual, stated plainly

**This closes the write path only.** A journal already on disk carrying an explicit
`"effect_receipt":null` still fails its checksum on read, because the stored hash covers bytes
this encoding no longer produces. Those sessions remain unreadable and their content is lost.

Repairing them would mean teaching the integrity check to accept two encodings for the same
event. That is a worse trade than losing them — an integrity check with a compatibility
branch is an integrity check an attacker gets to choose the branch of — and it is deliberately
not done here.

## 5. What I could NOT prove, and the honest limit on the measurement

I ran the reproduction harness against the shipped binary **34 times with 0 reproductions**:

| Binary | Runs | `--resume` OK | checksum mismatch | 1-min load during |
|---|---|---|---|---|
| lane HEAD `de977949` | 10 | 10 | 0 | ~7 (quiet) |
| lane HEAD `de977949` | 12 | 12 | 0 | 44 → 61 |
| **pristine base `15971d1b`** | 12 | 12 | 0 | **79 → 130** |

The base binary was verified pristine the same way 23B-01 verified it: `--build-info` reports
`source 15971d1b…` and `wayland-core session --help` exits non-zero because that subcommand
does not exist in it.

**This does not disprove 23B-01's measurement, and I am not claiming it does.** It narrows the
trigger. My harness drives a turn against a closed port, so the dispatch fails before any tool
runs and the reconciler-bearing tool intent that carries the defective shape is never written.
23B-01's runs were real work under six concurrent builds on a host at 93% disk, and got to the
tool boundary. The two results are consistent: same defect, different reach.

Load alone is ruled out as the trigger — 130 is well above the 28 that 23B-01 recorded.

The fix is proved at the journal layer, deterministically, red before green. It is **not**
proved by observing a live crash-then-resume of a session carrying a null receipt, because
driving a reconciler-declaring tool to emit one needs a real provider I do not have.

## 6. Files

- `crates/wcore-agent/src/session_journal/model.rs` — the fix and its rationale.
- `crates/wcore-agent/tests/journal_envelope_roundtrip.rs` — the round-trip invariant, five
  cases, two of which go red without the fix.
- `scripts/f23-h1-repro.sh` — reproduction driver against the shipped binary.
- `scripts/f23-h1-repro-under-load.sh` — load wrapper; generators self-terminate under
  `timeout` so a dropped ssh cannot orphan them.
- `scripts/f23-journal-forensics.py` — frame-level tool that separates a bad write from an
  unstable read by recovering the exact bytes the writer hashed.
