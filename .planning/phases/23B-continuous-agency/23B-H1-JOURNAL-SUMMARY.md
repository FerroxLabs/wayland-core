# 23B-H1-journal — LANE SUMMARY

Branch `lane/23b-h1-journal`. Base `f8b8ec25372fb4ed4280a5aa365873ae8465abfc`
(asserted against `git ls-remote` before any work).

**Verdict: the defect is root-caused, deterministically reproduced, fixed, and
the fix repairs journals already on disk. One twin surface is left open and
named.**

---

## 1. First, the brief was stale — and the correction matters

My dispatch brief described `23B-H1` as HIGH / open / no fix / no repair path,
quoting 23B-01's original numbers. At `f8b8ec25` all three were already false:
a write-path fix (`is_absent_json_value`) and a read-side repair
(`recover_legacy_effect_receipt`) were in the tree, and `BACKLOG.md:1496`
(`MEDIUM, non-reproducing`) supersedes the HIGH row at `BACKLOG.md:1771` that
the brief cites. Both rows are live in the same file.

I did not treat the brief as fact, and I did not simply ratify the MEDIUM
either — that downgrade rests on a **non-reproduction**, which LANE-BRIEF §3b-i
names as the easiest assertion in the programme to pass without doing any work.

## 2. Root cause, at file:line

**`crates/wcore-agent/src/session_journal.rs:107` — `JournalEnvelope::computed_checksum`
hashed a RE-SERIALIZATION of the decoded event, not the bytes on disk.**

That makes the integrity check depend on serde encode/decode being a bijection
over `SessionEvent`. It is not, and **the reader says so itself**:

* `session_journal.rs:2405` `known_omitted_default` blesses **fifteen** raw
  encodings as explicit defaults equivalent to absent — `"retry_of":null`,
  `"effect_receipt":null`, `"pre_hook_phase_id":null`,
  `"provider_reservations":{}`, `"artifact_digests":[]`, and eleven more.
* Every one of those is, by definition, dropped by the decode.
* So re-serializing covered different bytes than the producer hashed, and
  `verify_chain_from` (`session_journal.rs:2004`) called corrupt exactly what
  `reject_unknown_event_fields` (`session_journal.rs:2154`) had just declared
  valid — three lines earlier, on the same frame.

Two checks on the same bytes, disagreeing. The frame parses; the chain then
rejects it with `journal checksum mismatch at sequence N`, permanently, and all
twelve `session` operator verbs read through that chain.

**`effect_receipt` was one member of that class, not the class.** The previous
lane's root-cause analysis was correct about the mechanism and fixed the one
field it found; `retry_of` and `pre_hook_phase_id` sit in the **same match arm**
of `known_omitted_default` and were left open.

### Why it looked load-sensitive

It is not intrinsically load-sensitive. It fires when a run reaches an event
carrying one of the blessed encodings. Longer runs reach more event kinds —
which is exactly 23B-01's own observation that the failing journals were ~203 KB
against ~71 KB for passing ones, i.e. *the failing runs got further through the
turn*. Load correlates with reaching the tool-event boundary, it does not cause
the defect. **Sequence 16 is simply where the first such event landed in that
run shape**; my two-frame fixture reproduces the identical error at sequence 1.

## 3. Reproduction rate — before and after

The honest headline: **once the frame is built the way a real producer must
build it, this is not intermittent at all.** A producer stores the SHA-256 of
the bytes it writes; both pre-existing tests paired blessed bytes with a
checksum computed from a *different* encoding, which no producer could store —
which is why two prior lanes were chasing a 9/10.

| | base `f8b8ec25` | lane `00015a36` |
|---|---|---|
| unit/integration (`journal_wire_blessed_defaults`) | **1/1 fail**, `ChecksumMismatch { seq: 1 }`, 0.00s | **0/1 fail** |
| live CLI drive, `retry_of`, 1-min load 10.32 | **reproduced**, `journal checksum mismatch at sequence 1` from `reconcile` and `cancel` | **readable**, content recovered |
| live CLI drive, `effect_receipt`, same run | **not reproducible** — already closed by the previous lane | readable |

That last row is the control that makes the table mean something: same driver,
same nonce, same code path, and the only difference is which blessed field the
fixture carries. It is executable proof the earlier fix was **necessary but not
sufficient**.

Full transcripts, verbatim: `evidence/23B-H1-journal/23B-H1-journal-LIVE-EVIDENCE.md`.

### The known-negative, verbatim

```
thread 'wire_blessed_defaults_are_readable' panicked at
crates/wcore-agent/tests/journal_wire_blessed_defaults.rs:246:9:
an explicit "retry_of":null is declared wire-compatible by the reader's own
contract, and its producer stored the hash of the bytes it wrote — the journal
must be readable, got Some(ChecksumMismatch { seq: 1 })

test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

## 4. The three-assertion gate

1. **Known-positive** — `the_unspliced_fixture_loads`. It went red on its first
   run (`CorruptFrame … missing field 'checksum'`) and caught a genuine bug in
   my own fixture builder. Without it the negative case would have been red for
   a reason unrelated to the defect.
2. **Known-negative genuinely fails** — quoted above, and
   `a_tampered_body_is_still_rejected` is the guard in the other direction: it
   goes green, and the integrity check is gone, if the fix is ever mistaken for
   "the frame digest already proved these bytes, trust them". It stayed green
   through the fix.
3. **The old shape would have missed it** —
   `the_value_level_round_trip_invariant_is_blind_to_this` demonstrates that
   `journal_envelope_roundtrip.rs`'s invariant
   (`serialize(deserialize(serialize(e))) == serialize(e)`, over Rust *values*)
   holds for every `Option<String>` input it can construct, because **no
   `Option<String>` value serializes to `null`**. It found `effect_receipt` only
   because `Option<serde_json::Value>` can hold `Some(Value::Null)`. It is
   structurally incapable of reaching `retry_of`.

## 5. The fix

`computed_checksum` hashes the checksum material **exactly as it was written**,
reconstructed from the stored bytes via `serde_json`'s `RawValue`
(`checksum_of_stored_bytes`, populated only by `parse_complete_frames`).

**This is strictly tighter than what it replaces, not looser:**

* every difference the re-encoding caught is still caught — changing a value
  changes the bytes;
* differences the re-encoding **normalised away** (reordered members, inserted
  whitespace) are now caught, where before they verified clean;
* the only forms newly accepted are ones the reader has *already* declared
  valid. Anything the decode drops that is **not** allowlisted is still rejected
  by `reject_unknown_event_fields`, which runs first and fails closed with
  `UnknownCriticalField`. The set of newly-accepted bytes is exactly the
  allowlist — no new surface.

I did not suppress the error, widen a tolerance, or skip bad frames. Failing
closed on a corrupt journal is correct and still happens.

`recover_legacy_effect_receipt` shrinks to `restore_explicit_null_receipt`: with
the checksum taken from disk it no longer arbitrates integrity, and only
restores the one blessed field whose decode genuinely loses information
(`Option<serde_json::Value>` collapses `Some(Value::Null)` and `None` onto the
same encoding). The envelope's `legacy_effect_receipt` flag and its
thread-local scoped encoding are gone from the journal path.

## 6. Repair path for journals already corrupted — BUILT, not deferred

**The fix and the repair are the same change.** A journal already on disk was
written by a producer that stored the hash of its own bytes; hashing those bytes
back reproduces it. Every journal made unreadable by *any* member of this class
becomes readable again, with content intact — not just the null-receipt
instance, and nothing is discarded or truncated. Proven live: the `retry_of`
GREEN row recovers `ref=x1 tool=Write turn=t1`, identifiers that exist nowhere
but inside the recovered journal.

This is strictly better than the previous read-side repair, which was keyed
literally to a null receipt and covered one of the fifteen.

## 7. Severity — I disagree with the current MEDIUM, and would restore HIGH

`BACKLOG.md:1496` downgraded this to MEDIUM as "non-reproducing". That grade was
reasonable on the evidence available then and is wrong now: it rests on a
non-reproduction, and I have a **deterministic** reproduction at integration
HEAD, live at the CLI, plus a control proving the discriminating power of the
harness that produced it. Impact is unchanged from the original filing — a
session the user can see and cannot enter, every operator verb down at once, and
previously no general repair.

The one thing that legitimately narrows it: the `retry_of` and
`pre_hook_phase_id` encodings cannot be produced by the *current* Rust writer,
so they need another producer — an older build, another version, or a host
writing through the wire contract that `known_explicit_event_defaults_…` pins as
supported. That is version skew and host interop, not a hypothetical, and it is
precisely why the allowlist exists at all.

**Recommend: HIGH, and closed by this lane** — with the `retry_of` row now
having something the original filing never had, a deterministic reproduction.

## 8. Open / not done

- **The snapshot digest is the same defect, unfixed.**
  `reducer.rs:15 ReducedSessionState::digest` is `sha256(serde_json::to_vec(self))`
  — the same re-serialization dependency — and `session_journal.rs:2538` carries
  a parallel `"session snapshot"` allowlist of ~10 blessed encodings, guarded by
  `known_explicit_snapshot_defaults_are_wire_compatible`, which is vacuous in
  exactly the same way (it injects into a value whose `state_digest` was computed
  from the clean state). Failure mode is `SnapshotDigestMismatch` and it
  propagates out of `recovered_state`, so the impact is the same: session
  unreadable. The raw bytes are available at `snapshot.rs:359`, so the same fix
  applies almost verbatim.
  **I did not change it.** Deliberate: it is a second integrity surface and I
  was not willing to ship a change to one without the same depth of red-before-
  green proof I gave the journal. Named here rather than left in a comment;
  this should be the next lane and it is small.
- The original 23B-01 field sighting is **still not re-explained**. See
  LIVE-EVIDENCE §6.
- No macOS or Windows leg — pure-serde, platform-independent path; nothing
  platform-specific is claimed.

## 9. Gates

`cargo fmt --all` clean. `cargo check --workspace --all-targets` on hetzner:
`Finished`, 0 errors. All counts read from an unproxied
`/root/.cargo/bin/cargo`, so `0 ignored` / `0 filtered out` are present.

| suite | result |
|---|---|
| `journal_wire_blessed_defaults` (new) | `4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `journal_legacy_null_receipt_recovery` | `4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `journal_envelope_roundtrip` | `5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `session_journal_test` | `48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `session_journal_crash_matrix_test` | `4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `--lib session_journal` | `66 passed; 0 failed; 0 ignored; 0 measured; 2150 filtered out` |

Full `-p wcore-agent --lib` is contended on this host: base `f8b8ec25` (none of
my work) `2189 passed; 24 failed`, lane `2198 passed; 15 failed`, with the
failing set **unstable across two identical runs**. I claim no improvement from
those numbers; the base control establishes only that the cluster is not mine.

## 10. One test was modified — declared explicitly

`known_explicit_event_defaults_are_wire_compatible_but_unknowns_fail_closed`
paired explicit-null bytes with a checksum computed over the clean encoding, and
stopped at `parse_complete_frames` without ever reaching the chain check. Its
own failure output made this visible: `on_disk_checksum: Some("a779…")` against
`checksum: "e17d…"` — the frame did not hash to its own stored checksum.

I re-sealed the fixture over its own bytes and **added** a `verify_chain`
assertion it never made, plus an `assert_ne!` so it cannot silently stop
exercising the defect. The `unknowns fail closed` half is untouched. This
strengthens the test; no assertion was removed or relaxed. Flagged here because
modifying a test to make a change pass is the pattern §5 exists to catch, and a
reader should audit it rather than take my word.
