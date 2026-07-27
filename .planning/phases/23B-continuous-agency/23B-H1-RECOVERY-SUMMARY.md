---
phase: 23B-continuous-agency
plan: "H1-recovery"
subsystem: session-journal-durability
status: complete
supersedes_residual_in: 23B-H1-DISPOSITION.md §4
tags: [journal, snapshot, checksum, data-loss, recovery, 23B-H1]
key-files:
  created:
    - crates/wcore-agent/tests/journal_legacy_null_receipt_recovery.rs
    - scripts/f23-h1-legacy-journal.py
    - scripts/f23-h1-recovery-drive.sh
    - scripts/f23-h1-recovery-drive.ps1
  modified:
    - crates/wcore-agent/src/session_journal.rs
    - crates/wcore-agent/src/session_journal/model.rs
    - crates/wcore-agent/src/session_journal/snapshot.rs
commits:
  - 308832f4
  - 4b9512a0
  - 24e6694e
  - d070cfbe
  - d1719ece
---

# 23B-H1 residual — CLOSED. Journals already on disk are readable again.

23B-H1 root-caused the defect and fixed the **write** path. Its §4 named the residual
plainly and declined to close it: *"a journal ALREADY on disk carrying an explicit
`"effect_receipt":null` still fails its checksum on read … those sessions remain unreadable
and their content is lost."* That is silent, permanent user data loss on every session
journal written before the fix, in a product whose durability claim rests on journals
surviving.

**It is now recovered, with content intact, on Linux, macOS and Windows, against real
binaries — and without loosening the integrity check.**

---

## 1. What was rejected, and why the obvious repair was not taken

The disposition's stated reason for declining was correct as far as it went: *"repairing
them would mean teaching the integrity check to accept two encodings for the same event …
an integrity check with a compatibility branch is an integrity check an attacker gets to
choose the branch of."*

The design that avoids that objection is not a compatibility branch. It is a **repair whose
only acceptance criterion is the stored hash itself**:

1. Decode the frame as usual. All three existing checks are unchanged.
2. If — and only if — the recomputed checksum disagrees with the stored one, consult the
   raw JSON to see whether it carries an explicit `"effect_receipt":null`.
3. If it does, restore the exact value the pre-fix writer held (`Some(Value::Null)`) and
   re-hash under a scoped encoding that reproduces the pre-fix serialization.
4. Accept **only** if that re-hash equals the stored SHA-256 **exactly**. Otherwise put the
   envelope back exactly as decoded and let the unchanged checks report the real failure.

The raw JSON is consulted only to decide *which value to try*. A wrong guess simply fails
the hash. The branch is not chosen by an attacker — it is chosen by the stored hash, which
still has to be matched by preimage. The set of byte-strings the reader accepts for a given
decoded event grows from one to two, and **both decode to the same content**, so nothing the
checksum bound before is unbound now.

The scoped encoding is an RAII guard (`LegacyEffectReceiptEncoding`) around a single
serialization, restored on drop, so a panic cannot leave a thread in legacy mode.

**Both halves of the defect are covered.** 23B-H1 named two fields:
`SessionEvent::ToolIntentRecordedV2::effect_receipt` in the journal, and
`ToolState::effect_receipt`, which lives in `ReducedSessionState` and therefore in the
**snapshot**, where the same asymmetry produces `SnapshotDigestMismatch` instead of
`ChecksumMismatch`. The disposition did not mention the snapshot half; it is real and it is
fixed. For a snapshot the raw JSON is what keeps the repair to **one** candidate rather than
one per subset of tools.

### Cross-audit

Four-way panel (Codex 5.6 Sol, Gemini 3.1 Pro, Kimi K3, internal adversarial) on
Option A (revert the write-path predicate + hash-attested retry) versus Option B (keep it +
byte-level `strip(raw) == canonical` reconciliation plus a trusted flag).

| Panelist | Position |
|---|---|
| Codex 5.6 Sol | OTHER — keep the write path; verify against the exact preimage; avoid a bypass boolean; textual deletion is fragile |
| Gemini 3.1 Pro | OTHER — keep the write path; inject the null into a decoded clone and re-hash; avoid textual byte surgery |
| Kimi K3 | **A** — the hash deciding the decoding is the right conceptual fix; A's downgrade hazard is a rollout-ordering problem |
| Internal adversarial | Neither weakens integrity under collision resistance; the real risk is maintenance, not crypto |

All four agreed the hash should pick the interpretation and that byte-surgery should not.
Two of three explicitly wanted the write-path fix kept. **I took the majority on keeping the
write path** — a journal written by this build stays readable by a build carrying only
23B-H1's fix, and Kimi's counter (that the downgrade window is controllable) trades a
permanent property for a scheduling assumption. **I took Kimi's and Gemini's mechanism** —
re-hash a repaired decoded value, no textual deletion — which also disposes of Codex's
"trusted boolean" objection: the flag selects which of two precisely-specified encodings is
hashed, and an exact SHA-256 match is still required either way. It never skips a check.
Gemini's proposal as written could not work against the retained predicate (the injected
null would be skipped again); the scoped encoding is what makes it executable.

---

## 2. Red before, green after

### Unit level — `crates/wcore-agent/tests/journal_legacy_null_receipt_recovery.rs`

Byte-crafts what the pre-fix writer emitted, for both halves. On `hetzner-dsm`, by disabling
only the two recovery call sites and keeping the tests:

| Recovery | Result |
|---|---|
| disabled | **2 FAILED** — `ChecksumMismatch { seq: 1 }` and `SnapshotDigestMismatch`, the two exact symptoms |
| enabled | **4 passed**, 0 failed |

The two other tests in the file are the boundary: they tamper one byte of *content*
(`"tool":"Write"` → `"tool":"Xrite"`) and recompute the frame digest so checks (1) and (2)
pass and only the envelope checksum can catch it. They pass in **both** modes — if the
recovery were ever relaxed into "the frame digest already proved these bytes, accept them",
they go red.

### Live — the shipped binary, the way 23B-01 found the defect

`scripts/f23-h1-legacy-journal.py` reproduces serde's exact pre-fix encoding. It is not an
approximation: if it were not byte-faithful the recovery's hash check would not match and
the fixture would simply stay unreadable — a failure the driver reports rather than hides.

`session reconcile` and `session cancel` read the journal directly and need no credentials.

| Platform | Binary | Provenance | Mode | Result |
|---|---|---|---|---|
| Linux | CI artifact, run 30228836800 | `source b75e640c` (pre-dates even 23B-H1's write fix) | unreadable | **PASS** — exit 1, `journal checksum mismatch at sequence 1` |
| Linux | built on `hetzner-dsm` | `source 4b9512a0` | readable | **PASS** — exit 0 |
| macOS (arm64) | CI artifact, run 30228836800 | `source b75e640c` | unreadable | **PASS** — exit 1, same error |
| macOS (arm64) | CI artifact, run 30232472209 | `source 12bd0834` | readable | **PASS** — exit 0 |
| Windows (x64) | CI artifact, run 30228836800 | `source b75e640c` | unreadable | **PASS** — exit 1, same error |
| Windows (x64) | CI artifact, run 30232472209 | `source 12bd0834` | readable | **PASS** — exit 0 |

The recovered content, from a journal the pre-fix binary refuses outright — identical on all
three platforms:

```
F23_SESSION=reconcile_item id=s-<nonce> kind=tool_execution ref=x1 tool=Write turn=t1 reason=Prepared resolvable=false
F23_SESSION=reconcile id=s-<nonce> outstanding=1
```

`ref=x1`, `tool=Write` and `turn=t1` exist nowhere but inside the recovered journal, and the
nonce is generated by the caller at run time and planted in the session id, so a stale log
cannot satisfy the check.

**A recovered journal is usable, not merely inspectable.** With a terminal tool outcome in
the fixture (`--terminal-tool`), against the same journal the pre-fix binary rejects:

```
--- PRE-FIX     session reconcile  ->  could not be read: journal checksum mismatch at sequence 1   (exit 1)
--- FIXED       session reconcile  ->  F23_SESSION=reconcile id=… outstanding=0                     (exit 0)
--- FIXED       session cancel     ->  F23_SESSION=cancel_turn id=… turn=t1
                                       F23_SESSION=cancel id=… cancelled=1                          (exit 0)
--- FIXED       session reconcile  ->  F23_SESSION=reconcile id=… outstanding=0                     (exit 0)
```

It was read, **appended to**, and re-read.

### The gates can fail

| Falsification | Required | Observed |
|---|---|---|
| `--expect readable` against the pre-fix binary (Linux) | non-zero | exit 1 |
| `--expect readable` against the pre-fix binary (macOS) | non-zero | exit 1 |
| `--expect readable` against the pre-fix binary (Windows) | non-zero | exit 1 |
| `--expect unreadable` against the fixed binary (Linux) | non-zero | exit 1 |
| wrong `--sha` | non-zero | exit 3 |

Exit status is the primary gate; the nonce-bound `F23_H1_DRIVE=PASS` marker is the second,
independent one. No pipe carries a gate's exit status.

Logs: `.planning/phases/23B-continuous-agency/evidence/23Bb-h1-{linux,macos,windows}-drive.log`.

---

## 3. Regression — `hetzner-dsm`, commit 4b9512a0

| Gate | Result |
|---|---|
| `cargo clippy -p wcore-agent --all-targets -- -D warnings` | clean, exit 0 |
| `cargo fmt --all -- --check` (Mac) | clean, exit 0 |
| `cargo test -p wcore-agent --lib session_journal` | **66 passed**, 0 failed (unchanged from 23B-H1's baseline) |
| `--test session_journal_test` | **48 passed**, 0 failed |
| `--test session_journal_crash_matrix_test` | **4 passed**, 0 failed |
| `--test journal_envelope_roundtrip` | **5 passed**, 0 failed — 23B-H1's own invariant tests, untouched |
| `--test session_journal_compaction_test` | **24 passed**, 0 failed |
| `cargo nextest run -p wcore-agent --profile ci --no-fail-fast` | **2928 passed**, 0 failed, 12 skipped, exit 0 |

The pre-existing wire-contract test
`known_explicit_event_defaults_are_wire_compatible_but_unknowns_fail_closed` is unchanged
and still passes: it writes an explicit null while keeping the checksum computed over the
*stripped* bytes, so the canonical decode matches and the repair never fires. Explicit null
still means absent for any producer that hashes it that way. The stored hash decides.

---

## 4. What remains UNRECOVERABLE, stated plainly

1. **A genuinely corrupt journal.** By design. It matches neither encoding and is rejected
   with the same error as before. This is the boundary the two tamper tests pin.

2. **An explicit-null receipt on an event whose `effect_contract.reconciler` is `None`.**
   The recovery restores the value and the checksum matches, but the reducer then refuses —
   it rejects an effect receipt without a reconciler. This engine could not have written
   such a journal (the same check runs on append), but the documented wire contract blesses
   explicit nulls, so a third-party producer could have. Those journals stay unreadable.
   **This is not a regression** — before this change they failed at the checksum instead;
   they now fail with a more informative, structurally accurate error.

3. **Content the journal never carried.** The recovery restores encoding fidelity, not
   missing events. A journal truncated, deleted, or never flushed is out of scope.

4. **Nothing else that I could find.** The defect requires a field typed
   `Option<serde_json::Value>` *with* a `skip_serializing_if` predicate. Across the journal
   and snapshot type tree those are exactly the two `effect_receipt` fields. Fields typed
   `Option<serde_json::Value>` without the skip attribute (`ToolState::result`,
   `resolution_evidence`) always serialize explicitly and therefore round-trip byte-stably.
   `wcore_types::message::ContentBlock::ToolUse::extra` has the hazardous shape but is not
   reachable from a journal event — the conversation is stored as opaque
   `Vec<serde_json::Value>`, which round-trips verbatim.

**So: for every journal this engine could have written carrying the 23B-H1 artifact, no
content is lost.** The recovered `ReducedSessionState` is the state the writer held, field
for field — `Some(Value::Null)`, not a normalisation of it, so replay reproduces the writing
run rather than a slightly different one.

---

## 5. Limits of the measurement

- **`--resume` itself was not driven.** The recovery sits in `parse_complete_frames`, which
  every reader including `--resume` goes through, and `session cancel`'s successful append
  proves the *writer* open path recovers too. But the literal `--resume` invocation needs a
  real provider I do not have, so it is inferred from a shared code path, not observed.
- **The unit suite ran on Linux only.** CI on this branch fails before the test step at a
  **pre-existing** gate: `Check Desktop protocol contract corpus drift`
  (`drifted=["adversarial/events/…", "events/ready.json", "manifest.json"]`). The same step
  fails identically on the untouched base branch (run 30232008236 on
  `plan/f20-unified-audit-repair`), so it is not caused by this work, and repairing it means
  `wcore-contract generate`, which this lane is forbidden to run. macOS and Windows are
  covered by the live legs instead, which per this program's standing rule rank at least as
  high.
- **23B-H1's own reproduction gap is unchanged.** The previous lane's 34 harness runs
  produced 0 reproductions because the harness never reached a tool event. This work does
  not narrow that further; it makes the outcome moot for data loss, since the journal such a
  run produces is now readable either way.

## 6. Note for the orchestrator

`ci.yml` carried a **temporary** `lane/23Bb` push trigger so CI would build the macOS and
Windows binaries this evidence needs (the mechanism recorded in
`.planning/intel/MACOS-BINARY-IS-OBTAINABLE.md`). It is reverted in this lane's final commit
and must not merge.
