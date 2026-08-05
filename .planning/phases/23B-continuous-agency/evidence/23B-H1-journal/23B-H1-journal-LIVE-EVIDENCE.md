# 23B-H1-journal — LIVE EVIDENCE

Host `hetzner-dsm`. Lane commit `00015a361d573fdbcc30726bc0d9257e8f048fc6`,
base commit `f8b8ec25372fb4ed4280a5aa365873ae8465abfc`. Run nonce
`64a0d172a8bafcb2`, generated at run time and planted in the session id, so a
stale log cannot satisfy any check below. 1-minute load average during the run:
**10.32**.

Driver `scripts/f23-h1-recovery-drive.sh` (two modes, both required), fixture
generator `scripts/f23-h1-legacy-journal.py`. Each binary's `--build-info` is
asserted against the SHA under test before anything is exercised, so a stale
binary reddens instead of silently proving old code.

---

## 1. The matrix — two shapes x two binaries

| shape | binary | mode | result |
|---|---|---|---|
| `retry_of` | base `f8b8ec25` | unreadable (RED) | **PASS — defect reproduced** |
| `retry_of` | lane `00015a36` | readable (GREEN) | **PASS — journal readable, content recovered** |
| `effect_receipt` | base `f8b8ec25` | unreadable (RED) | **FAIL — could not reproduce** |
| `effect_receipt` | lane `00015a36` | readable (GREEN) | PASS |

The `effect_receipt` RED row failing is the **control, and it is the point of
the whole table**: that shape was already closed at base by the previous lane's
`recover_legacy_effect_receipt`, so the defect is genuinely absent there and the
driver says so. The identical driver, binary, nonce and code path reproduces the
defect for `retry_of`. That is executable proof that the earlier fix was
**necessary but not sufficient** — it closed one member of the class and left
the rest.

## 2. RED — `retry_of`, base binary, verbatim

```
provenance: wayland-core 0.12.25 (source f8b8ec25372fb4ed4280a5aa365873ae8465abfc)
--- session reconcile s-64a0d172a8bafcb2 (exit 1)
wayland-core session: journal '/tmp/tmp.AuzXdVelFw/s-64a0d172a8bafcb2.journal' could not be read: journal checksum mismatch at sequence 1
--- session cancel s-64a0d172a8bafcb2 (exit 1)
wayland-core session: journal '/tmp/tmp.AuzXdVelFw/s-64a0d172a8bafcb2.journal' could not be read: journal checksum mismatch at sequence 1
F23_H1_DRIVE=PASS platform=linux mode=unreadable shape=retry_of nonce=64a0d172a8bafcb2
```

This is the 23B-01 symptom, from the shipped CLI, at integration HEAD, on a
journal whose encoding the reader's own contract declares valid. The sequence
number differs from the field sighting's 16 only because this fixture is two
frames long.

## 3. GREEN — `retry_of`, lane binary, verbatim

```
provenance: wayland-core 0.12.25 (source 00015a361d573fdbcc30726bc0d9257e8f048fc6)
--- session reconcile s-64a0d172a8bafcb2 (exit 0)
F23_SESSION=reconcile_item id=s-64a0d172a8bafcb2 kind=tool_execution ref=x1 tool=Write turn=t1 reason=Prepared resolvable=false
F23_SESSION=reconcile id=s-64a0d172a8bafcb2 outstanding=1
--- session cancel s-64a0d172a8bafcb2 (exit 5)
wayland-core session: session 's-64a0d172a8bafcb2' has 1 outstanding reconcile item(s)
--- session reconcile s-64a0d172a8bafcb2 (exit 0)
F23_SESSION=reconcile_item id=s-64a0d172a8bafcb2 kind=tool_execution ref=x1 tool=Write turn=t1 reason=Prepared resolvable=false
F23_SESSION=reconcile id=s-64a0d172a8bafcb2 outstanding=1
F23_H1_DRIVE=PASS platform=linux mode=readable shape=retry_of nonce=64a0d172a8bafcb2
```

Content, not merely "it opened": `ref=x1 tool=Write turn=t1` exist nowhere but
inside the recovered journal. `cancel` exiting **5** (the documented
outstanding-reconcile refusal) rather than 1 is itself proof the journal was
read — an unreadable journal fails with `could not be read` before any reducer
state exists. The final repeat shows reading is not a one-shot side effect of
the first open.

## 4. Unit / integration gates at `00015a36`

Counts read back from an **unproxied** `/root/.cargo/bin/cargo`, so the
`0 ignored` / `0 filtered out` fields the anti-vacuity rule depends on are
present.

| suite | result |
|---|---|
| `--test journal_wire_blessed_defaults` (new) | `4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `--test journal_legacy_null_receipt_recovery` | `4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `--test journal_envelope_roundtrip` | `5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `--test session_journal_test` | `48 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `--test session_journal_crash_matrix_test` | `4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out` |
| `--lib session_journal` | `66 passed; 0 failed; 0 ignored; 0 measured; 2150 filtered out` |
| `cargo check --workspace --all-targets` | `Finished` — 0 errors |

### The known-negative, verbatim, before the fix

```
thread 'wire_blessed_defaults_are_readable' panicked at
crates/wcore-agent/tests/journal_wire_blessed_defaults.rs:246:9:
an explicit "retry_of":null is declared wire-compatible by the reader's own
contract, and its producer stored the hash of the bytes it wrote — the journal
must be readable, got Some(ChecksumMismatch { seq: 1 })

test result: FAILED. 3 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
```

The three that passed in that same run are the other three assertions: the
known-positive control (`the_unspliced_fixture_loads`), the tamper guard
(`a_tampered_body_is_still_rejected`) and the blindness demonstration
(`the_value_level_round_trip_invariant_is_blind_to_this`).

**The known-positive earned its keep.** Its first run went red with
`CorruptFrame … missing field 'checksum'`, catching a real bug in my fixture
builder (the second frame was framed from unsealed material). Without it, the
negative case would have been red for a reason having nothing to do with the
defect.

## 5. Full-crate regression, with its base control

A full `-p wcore-agent --lib` run on this host is contended — LANE-BRIEF §6.
Both figures come from the same host within minutes of each other:

| tree | result |
|---|---|
| **base `f8b8ec25`, none of this lane's work** | `2189 passed; 24 failed; 3 ignored; 0 measured; 0 filtered out` |
| lane `ca1a5348` | `2198 passed; 15 failed; 3 ignored; 0 measured; 0 filtered out` |

The failing set is **not stable between two identical runs of the same binary**
(19 then 15, with different members — `live_session_switch_*` appearing in the
second and `orchestration::*` dropping out), which is the signature of
contention rather than a regression. Base fails a strict superset of the
families the lane tree fails: `engine::audit_2026_05_22_tests::*`,
`session::tests::*`, `channel_lease::tests::*`, all lease/timing/filesystem
tests. Every one of them passes in isolation at the lane commit — e.g.
`resumed_engine_holds_journal_lease_until_drop` → `1 passed; 0 failed`.

I am claiming no improvement from `24 → 15`; both numbers are contention noise.
What the base control establishes is only that **this cluster is not mine**.

## 6. What I did NOT prove

- **I did not re-explain the original 23B-01 field sighting.** That run's
  producer was the Rust writer itself, and the only encoding that writer can
  emit in this class is `effect_receipt` — which the previous lane already
  closed. The `retry_of` and `pre_hook_phase_id` encodings require a producer
  other than the current writer (an older build, another version, or a host
  writing through the documented wire contract). Whether the original sighting
  was the `effect_receipt` instance remains unproven in either direction; see
  the NOTES for why the later lane's 92-run non-reproduction does not settle it.
- **No macOS or Windows leg.** This is a pure-serde, platform-independent code
  path and no platform-specific behaviour is claimed.
- **The snapshot digest is the same class and I did not close it.** See the
  SUMMARY residual section.
