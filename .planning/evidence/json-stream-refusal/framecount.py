#!/usr/bin/env python3
"""Frame counter for --json-stream captures, plus its own self-test.

Instrument contract: read a capture file as the HOST reads it — one JSON object
per line on stdout — and tally by `type`. Never grep. A substring match on a raw
byte stream cannot distinguish a frame of type "error" from the word "error"
inside another frame's message field, and that difference is the entire subject
of this lane.

Usage:
    framecount.py tally <file>        # print TYPE=<t> COUNT=<n> lines, plus totals
    framecount.py selftest            # 3-assertion self-test (see below)
"""

import json
import sys


def tally(path):
    """Return (counts_by_type, n_lines, n_parsed, n_unparsed, raw_bytes)."""
    with open(path, "rb") as fh:
        raw = fh.read()
    counts = {}
    n_lines = 0
    n_parsed = 0
    n_unparsed = 0
    for line in raw.splitlines():
        if not line.strip():
            continue
        n_lines += 1
        try:
            obj = json.loads(line)
        except (ValueError, UnicodeDecodeError):
            n_unparsed += 1
            continue
        if not isinstance(obj, dict):
            n_unparsed += 1
            continue
        n_parsed += 1
        t = obj.get("type", "<no-type-field>")
        counts[t] = counts.get(t, 0) + 1
    return counts, n_lines, n_parsed, n_unparsed, len(raw)


def cmd_tally(path):
    counts, n_lines, n_parsed, n_unparsed, raw_bytes = tally(path)
    for t in sorted(counts):
        print(f"TYPE={t} COUNT={counts[t]}")
    print(f"TOTAL_FRAMES={n_parsed}")
    print(f"UNPARSED_LINES={n_unparsed}")
    print(f"NONEMPTY_LINES={n_lines}")
    print(f"RAW_BYTES={raw_bytes}")
    # Name the reason when an error frame is present, so the proof can show the
    # consumer could ACT on it -- not merely that some bytes arrived.
    with open(path, "rb") as fh:
        for line in fh.read().splitlines():
            try:
                obj = json.loads(line)
            except (ValueError, UnicodeDecodeError):
                continue
            if isinstance(obj, dict) and obj.get("type") == "error":
                err = obj.get("error", {})
                print(f"ERROR_CODE={err.get('code')}")
                print(f"ERROR_RETRYABLE={err.get('retryable')}")
                print(f"ERROR_MESSAGE={err.get('message')}")


def _old_broken_matcher(raw_text):
    """The shape this instrument replaced: substring search over the raw stream.

    Kept ONLY so the self-test can prove the repair does something (assertion 3).
    """
    return "error" in raw_text


def cmd_selftest():
    import tempfile
    import os

    failures = []

    # --- Assertion 1: known-positive. A real error frame is counted as one. ---
    positive = (
        '{"type":"ready","session_id":"s1"}\n'
        '{"type":"error","error":{"code":"init_failed",'
        '"message":"boom","retryable":false}}\n'
    )
    fd, p1 = tempfile.mkstemp()
    os.write(fd, positive.encode())
    os.close(fd)
    counts, _, parsed, unparsed, _ = tally(p1)
    if counts.get("error") != 1 or counts.get("ready") != 1 or unparsed != 0:
        failures.append(f"A1 known-positive: counts={counts} unparsed={unparsed}")

    # --- Assertion 2: known-negative. No error frame => zero, not "some". ---
    # This capture MENTIONS the word error inside another frame's payload.
    negative = (
        '{"type":"ready","session_id":"s1"}\n'
        '{"type":"info","message":"no error occurred during startup"}\n'
    )
    fd, p2 = tempfile.mkstemp()
    os.write(fd, negative.encode())
    os.close(fd)
    counts2, _, _, _, _ = tally(p2)
    if counts2.get("error", 0) != 0:
        failures.append(f"A2 known-negative: error count={counts2.get('error')}")

    # --- Assertion 3: the OLD matcher would have MISSED this / got it wrong. ---
    # Without this assertion the self-test passes on the broken instrument too,
    # so it would prove nothing about the repair.
    if not _old_broken_matcher(negative):
        failures.append("A3 setup: old matcher was expected to fire on the negative")
    old_says_error = _old_broken_matcher(negative)
    new_says_error = counts2.get("error", 0) > 0
    if not (old_says_error and not new_says_error):
        failures.append(
            f"A3 divergence: old={old_says_error} new={new_says_error} "
            "(old substring matcher must false-positive where the new one does not)"
        )

    os.unlink(p1)
    os.unlink(p2)

    if failures:
        for f in failures:
            print(f"SELFTEST_FAIL {f}")
        print("SELFTEST=FAIL")
        return 1
    print("SELFTEST_A1=pass known-positive error frame counted exactly once")
    print("SELFTEST_A2=pass known-negative with the word 'error' in a payload counts 0")
    print("SELFTEST_A3=pass old substring matcher false-positives where this one does not")
    print("SELFTEST=PASS")
    return 0


if __name__ == "__main__":
    if len(sys.argv) >= 2 and sys.argv[1] == "selftest":
        sys.exit(cmd_selftest())
    if len(sys.argv) == 3 and sys.argv[1] == "tally":
        cmd_tally(sys.argv[2])
        sys.exit(0)
    print(__doc__)
    sys.exit(2)
