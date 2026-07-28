# 23B-H1-MEASURE — append-only notes

Lane `lane/23b-h1-measure`. Base `3cfc336fd2d82f57b5a24716262a71e759cb4a24`
(`plan/f20-unified-audit-repair`). Worktree
`/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-23b-h1-measure`.

**No credential value appears in this file or any artifact this lane writes.**

---

## T+0 — mandate and inherited state

Mandate: a `flux-router` credential now exists (`~/.wayland-secrets/flux.env`, mode 600,
outside every repo). Use it to repair the durability harness's *reach* — get it dispatching
real tool events — then reproduce or disprove `23B-H1` at HEAD.

Inherited from `23B-H1-SUMMARY.md` (lane `lane/23b-h1`), read in full before acting:

- Finding OPEN at HIGH. A cleanly-exited run writes a journal the product cannot read back
  (`journal checksum mismatch at sequence N`), and every operator verb that reads it fails
  identically.
- **The previously-claimed root cause is WRONG and I do not need to re-derive it.** The
  repaired mechanism (`Option<Value>` + `skip_serializing_if = Option::is_none` holding
  `Some(Value::Null)`) is engine-unreachable: `durable_receipt()` serialises a derived
  named-field struct (`FilesystemEffectReceiptV1`) → always `Value::Object`; the other three
  `prepare_tool_effect` call sites pass literal `None`. 23B-01's repro was headless, no
  third-party producer, so that shape cannot have been written.
- Three candidates remain, one already excluded: (1) a write that never completes its final
  record — **excluded**, `ChecksumMismatch` is check *3* of 3 in `verify_chain_from`, so the
  frame SHA and the chain link both passed; (2) a schema the reader rejects; (3) a reader
  stricter than the writer. §2 of the prior summary lands on category (3), i.e. an encoding
  that is not a round-trip fixed point — but the specific member is unknown.
- Prior lane's §4b census is the strongest lead: **~34 fields** in `session_journal` carry
  `#[serde(default, skip_serializing_if = P)]` where the skipped value has an explicit JSON
  spelling that decodes back to itself. Only **2** (`is_absent_json_value`) were repaired.
  `Option::is_none` × 21 (`null`), `BTreeMap::is_empty` × 5 (`{}`), `Vec::is_empty` × 4
  (`[]`), `BTreeSet::is_empty` × 1, `is_zero_u32` × 1 (`0`).
- Harness reach: `scripts/f23-h1-repro.sh` writes a config with a fake key and
  `base_url = http://127.0.0.1:1` (closed port). Every run therefore ends
  `status=OK_DISPATCH_FAILED`; **no tool event is ever recorded**, 0/12 and 0/34 before that.
  Non-reproduction from an instrument that cannot reach the defect is the evidentiary form of
  a gate that cannot fail.

## T+0 — what I must establish

1. Harness reaches a real tool event (counted, not asserted).
2. Reproduce or disprove at HEAD, with counts, neither forced.
3. If it reproduces: name WHICH of the remaining candidates, with evidence, before fixing.
4. Only then fix, plus a repair path for an already-unreadable journal (reclaim + quarantine,
   the shape this program used in the sandbox), not a permanent refusal.

## T+0 — measured caveat carried in from the mandate

`flux-fast` is a **reasoning** model. A 16-token budget returned HTTP 200 with empty content
and all 16 tokens spent as `reasoning_tokens`. Budget completions properly or an empty
completion will be misread as a product defect.

## T+0 — open question being decided now

Where to run. Hetzner is the only host that builds. The mandate says the key must never be
written into a file; the lane brief says never copy a credential off the Mac. Checking first
whether a HEAD-provenance `wayland-core` binary already exists on the Mac, which would satisfy
both. Decision and its evidence appended below.
