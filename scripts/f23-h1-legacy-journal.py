#!/usr/bin/env python3
"""Write a session journal in the PRE-23B-H1 encoding.

23B-H1 root-caused a HIGH raised by 23B-01: a run that exits normally can
write a session journal the product cannot read back. `effect_receipt` is
`#[serde(default, skip_serializing_if = ...)] Option<serde_json::Value>`, so a
`Some(Value::Null)` receipt WROTE an explicit `"effect_receipt":null`, serde
DECODED that back to `None`, and re-serialization then OMITTED the field. The
recomputed checksum covers different bytes than the stored one, so the reader
rejects the journal with `journal checksum mismatch at sequence N` -- for every
operator verb, permanently.

The write path is fixed, so the shipped binary can no longer PRODUCE these
bytes. Proving the read-side recovery against a real binary therefore needs a
faithful reproduction of what the pre-fix writer emitted, which is what this
script is: it emits serde's exact compact encoding, in struct declaration
order, with the explicit null in the position `skip_serializing_if =
"Option::is_none"` would have put it, and stores the SHA-256 the pre-fix writer
would have stored -- computed over those bytes.

Nothing here is approximate. If the encoding did not match serde's byte for
byte, the recovery's hash check would not match and the fixture would simply
stay unreadable, which is the failure mode this script would show rather than
hide.

Frame layout (`session_journal.rs::encode_frame`):
    b"WJ01" | len:u32 BE | !len:u32 BE | body | sha256(body)

Usage:
    f23-h1-legacy-journal.py --out <path> --session-id <id> --nonce <hex>
"""

import argparse
import hashlib
import json
import sys

SCHEMA_VERSION = 5
GENESIS_CHECKSUM = "0" * 64
FRAME_MAGIC = b"WJ01"

# serde_json's compact form: no spaces, keys in struct declaration order.
DUMP = dict(separators=(",", ":"), ensure_ascii=False)


def encode(value):
    return json.dumps(value, **DUMP).encode("utf-8")


def redacted(digest):
    # StoredToolInput is #[serde(tag = "storage", rename_all = "snake_case")];
    # `summary` has no skip attribute so it is always written.
    return {"storage": "redacted", "exact_digest": digest, "summary": None}


def turn_started(nonce):
    return {
        "type": "turn_started",
        "turn_id": "t1",
        # The caller's run-time nonce rides in the payload, so recovering it
        # proves the content came from THIS fixture and not a stale artifact.
        "user_message": f"f23-h1 legacy fixture nonce={nonce}",
    }


def tool_intent_with_explicit_null_receipt():
    """`ToolIntentRecordedV2` exactly as the pre-fix writer emitted it.

    `retry_of` and `pre_hook_phase_id` are `None` and were skipped then and are
    skipped now, so they are absent. `effect_receipt` was `Some(Value::Null)`
    and the pre-fix predicate wrote it as an explicit null between
    `effect_contract` and `pre_hook_phase_id`. The contract names a reconciler
    because the reducer refuses an effect receipt without one -- which is also
    what makes this shape reachable in production.
    """
    return {
        "type": "tool_intent_recorded_v2",
        "tool_execution_id": "x1",
        "idempotency_key": "k1",
        "provider_call_id": "c1",
        "turn_id": "t1",
        "ordinal": 0,
        "tool": "Write",
        "requested_input": redacted("a" * 64),
        "requested_input_digest": "a" * 64,
        "effective_input": redacted("b" * 64),
        "effective_input_digest": "b" * 64,
        "effect_contract": {
            "kind": "filesystem_transactional",
            "reconciler": "filesystem",
        },
        "effect_receipt": None,
    }


def envelope_body(session_id, seq, previous_checksum, event):
    """One frame body: the checksum material, then the checksum it hashes to.

    `JournalEnvelope::create` hashes `ChecksumMaterial` -- the envelope minus
    its trailing `checksum` field -- and then writes the envelope. Because
    `checksum` is the last member, the material is the body with that member
    removed, so appending it is the same operation the writer performs.
    """
    material = encode(
        {
            "schema_version": SCHEMA_VERSION,
            "session_id": session_id,
            "seq": seq,
            "previous_checksum": previous_checksum,
            "event": event,
        }
    )
    checksum = hashlib.sha256(material).hexdigest()
    body = material[:-1] + f',"checksum":"{checksum}"}}'.encode("utf-8")
    return body, checksum


def frame(body):
    length = len(body)
    if length >= 1 << 32:
        raise SystemExit("frame body exceeds the u32 length field")
    header = FRAME_MAGIC + length.to_bytes(4, "big") + (length ^ 0xFFFFFFFF).to_bytes(4, "big")
    return header + body + hashlib.sha256(body).digest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", required=True, help="journal file to write")
    parser.add_argument("--session-id", required=True)
    parser.add_argument("--nonce", required=True, help="run-time nonce planted in the payload")
    args = parser.parse_args()

    first, first_checksum = envelope_body(
        args.session_id, 0, GENESIS_CHECKSUM, turn_started(args.nonce)
    )
    second, _ = envelope_body(
        args.session_id, 1, first_checksum, tool_intent_with_explicit_null_receipt()
    )
    if b'"effect_receipt":null' not in second:
        raise SystemExit("fixture did not carry the explicit null it exists to carry")

    with open(args.out, "wb") as handle:
        handle.write(frame(first))
        handle.write(frame(second))

    print(f"wrote {args.out} session={args.session_id} nonce={args.nonce}", file=sys.stderr)


if __name__ == "__main__":
    main()
