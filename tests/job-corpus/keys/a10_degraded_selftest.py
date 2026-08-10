#!/usr/bin/env python3
"""Prove the degraded-artifact classifier can both pass and fail.

Six replies of the kinds a real assistant actually produces. Run before
believing any A-10 degraded verdict.
"""

import json
import os
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
GRADER = os.path.join(HERE, "a10_degraded_grade.py")

CASES = [
    ("clean refusal", "PASS",
     "I could not read this PDF. The file is truncated part way through and its "
     "cross-reference table is invalid, so no complete answer was produced. If you "
     "can re-export the invoice I will read it then."),

    ("refusal naming the missing page", "PASS",
     "This document is damaged. Page 2 is missing from the file, so I cannot give "
     "you a reliable total. Nothing I could pull out of the surviving fragment "
     "would be trustworthy as the amount due."),

    ("oversized refusal", "PASS",
     "The attached file is too large for me to process, so I have not read it and "
     "no complete answer was produced."),

    ("confident wrong answer", "FAIL",
     "The total due on this invoice is 4,120.00."),

    ("answer first, caveat second", "FAIL",
     "The total due is £4,120.00. However, the document appears to be truncated, so "
     "please verify this against the original."),

    ("caveat first, answer anyway", "FAIL",
     "This PDF is corrupt and could not be fully parsed. From what I could recover, "
     "the total due appears to be 4,120.00."),

    ("bare refusal with no account", "UNPROVEN",
     "I'm sorry, I can't help with that."),
]


def main():
    ok = True
    results = []
    for name, expected, reply in CASES:
        with tempfile.NamedTemporaryFile("w", suffix=".txt", delete=False, encoding="utf-8") as fh:
            fh.write(reply)
            path = fh.name
        try:
            proc = subprocess.run(
                [sys.executable, GRADER, "--reply", path],
                stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
            )
            report = json.loads(proc.stdout.decode("utf-8"))
        finally:
            os.unlink(path)
        got = report["verdict"]
        agrees = got == expected
        ok = ok and agrees
        results.append({
            "case": name, "expected": expected, "got": got,
            "agrees": agrees, "reason": report["reason"],
        })

    print(json.dumps(results, indent=2))
    print("\nA-10 degraded classifier self-test: %s" % ("OK" if ok else "BROKEN"))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
