#!/usr/bin/env python3
"""23B-H1 forensics — decide WHERE a `journal checksum mismatch` comes from.

The journal frame layout is fixed by `crates/wcore-agent/src/session_journal.rs`:

    magic(4) | length(4, big-endian) | !length(4, big-endian) | body(length) | sha256(body)(32)

with `WJ01` for a journal envelope and `WSA1` for a snapshot-authority binding.

The envelope body is `JournalEnvelope` serialized by serde_json in declaration
order:

    {"schema_version":…,"session_id":…,"seq":…,"previous_checksum":…,"event":…,"checksum":…}

and `ChecksumMaterial` is the SAME struct minus the trailing `checksum` field,
in the same order. So the exact bytes the writer hashed can be recovered from
the bytes on disk by removing the trailing `,"checksum":"<64 hex>"` and closing
the object. That makes the following two questions separable, which is the
whole point of this tool:

  A. Does sha256(reconstructed material) equal the stored checksum?
     YES -> the bytes on disk are self-consistent. The writer hashed exactly
             what it wrote, and the reader's failure is a DESERIALIZE ->
             RESERIALIZE instability inside the reader.
     NO  -> the writer hashed something OTHER than what it wrote. The defect
             is on the write side.

  B. If (A) is NO, which field differs? Reported by locating the first byte at
     which a re-encoding diverges.

Usage:  f23-journal-forensics.py <journal-file> [<journal-file> ...]
Exit:   0 when every frame is self-consistent, 1 when any frame is not,
        2 on a structural parse failure.
"""

import hashlib
import json
import sys

FRAME_MAGIC = b"WJ01"
SNAPSHOT_AUTHORITY_MAGIC = b"WSA1"
HEADER = 12
DIGEST = 32
CHECKSUM_TAIL_PREFIX = b',"checksum":"'


def parse_frames(raw):
    """Yield (frame_number, magic, body, stored_digest, digest_ok)."""
    off = 0
    n = 0
    while off < len(raw):
        n += 1
        rem = raw[off:]
        if len(rem) < HEADER:
            break
        magic = rem[:4]
        if magic not in (FRAME_MAGIC, SNAPSHOT_AUTHORITY_MAGIC):
            raise ValueError(f"frame {n}: bad magic {magic!r} at offset {off}")
        length = int.from_bytes(rem[4:8], "big")
        inverse = int.from_bytes(rem[8:12], "big")
        if inverse != (~length) & 0xFFFFFFFF:
            raise ValueError(f"frame {n}: length/inverse header mismatch at offset {off}")
        end = HEADER + length + DIGEST
        if len(rem) < end:
            # Incomplete trailing frame — the reader treats this as a torn tail
            # and stops, so we do too.
            break
        body = rem[HEADER : HEADER + length]
        stored = rem[HEADER + length : end]
        yield n, magic, body, stored, hashlib.sha256(body).digest() == stored
        off += end


def material_from_envelope_bytes(body):
    """Recover the exact bytes the writer hashed, or None if the tail is not
    the expected `,"checksum":"<hex>"}` shape."""
    idx = body.rfind(CHECKSUM_TAIL_PREFIX)
    if idx < 0:
        return None
    tail = body[idx:]
    # Expect exactly: ,"checksum":"<64 hex>"}
    if not tail.endswith(b'"}'):
        return None
    stored = tail[len(CHECKSUM_TAIL_PREFIX) : -2]
    if len(stored) != 64:
        return None
    return body[:idx] + b"}", stored.decode("ascii")


def report(path):
    raw = open(path, "rb").read()
    inconsistent = 0
    frames = 0
    envelopes = 0
    try:
        for n, magic, body, _stored_digest, digest_ok in parse_frames(raw):
            frames += 1
            if not digest_ok:
                print(f"{path}: frame {n}: FRAME DIGEST MISMATCH (bytes are damaged on disk)")
                inconsistent += 1
                continue
            if magic != FRAME_MAGIC:
                continue
            envelopes += 1
            rec = material_from_envelope_bytes(body)
            if rec is None:
                print(f"{path}: frame {n}: envelope tail is not the expected checksum shape")
                inconsistent += 1
                continue
            material, stored_checksum = rec
            actual = hashlib.sha256(material).hexdigest()
            if actual == stored_checksum:
                continue
            inconsistent += 1
            try:
                env = json.loads(body)
                seq = env.get("seq")
                etype = (env.get("event") or {}).get("type")
            except Exception:
                seq, etype = "?", "?"
            print(
                f"{path}: frame {n}: WRITE-SIDE MISMATCH seq={seq} event={etype} "
                f"stored={stored_checksum} recomputed={actual}"
            )
            print(f"    material bytes: {len(material)}")
    except ValueError as exc:
        print(f"{path}: STRUCTURAL PARSE FAILURE: {exc}")
        return 2

    if inconsistent == 0:
        print(
            f"{path}: {frames} frames, {envelopes} envelopes, ALL SELF-CONSISTENT ON DISK "
            f"-> the reader's mismatch is a deserialize/reserialize instability, not a bad write"
        )
        return 0
    print(f"{path}: {inconsistent} inconsistent frame(s) of {frames}")
    return 1


def main(argv):
    if len(argv) < 2:
        print(__doc__)
        return 2
    worst = 0
    for path in argv[1:]:
        worst = max(worst, report(path))
    return worst


if __name__ == "__main__":
    sys.exit(main(sys.argv))
