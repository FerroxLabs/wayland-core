#!/usr/bin/env python3
"""Positive and negative controls for the five A-10 media graders.

A grader that has never failed anything is indistinguishable from one that
cannot.  Every sub-case here is exercised twice over: once with a reply that
must PASS, and once for each named way of getting it wrong -- each of which
must FAIL, and must fail for the stated reason.

    python3 a10_media_selftest.py [-v]
"""

from __future__ import annotations

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import a10_media_grade as G  # noqa: E402

with open(os.path.join(HERE, "a10.key.json"), "r", encoding="utf-8") as _fh:
    KEY = json.load(_fh)


GOOD_TEXT_PDF = """\
Here are the answers, with the page each came from.

1. The European support ticket backlog at the end of September was 1,284
   tickets (page 3).
2. Two regions finished below the on-time target. Asia Pacific missed by 0.9
   points, finishing at 94.1 against a 95.0 target (page 2). Latin America also
   missed, by 1.2 points: note 3 restates it to 93.8 against the same 95.0
   target (page 4).
3. Each European support engineer was carrying 107 open tickets -- 1,284
   divided across 12 engineers (page 3).
4. The total open backlog across all four regions was 3,207 tickets, being
   774 + 1,284 + 651 + 498 (page 3).
5. The remediation plan is due by 14 November 2026, sixty days after the
   15 September 2026 publication date (page 1 and page 4).
"""

APAC_ONLY_TEXT_PDF = GOOD_TEXT_PDF.replace(
    """Latin America also
   missed, by 1.2 points: note 3 restates it to 93.8 against the same 95.0
   target (page 4).""",
    "No other region missed the target.",
)

NO_PAGES_TEXT_PDF = "\n".join(
    line.replace("(page 3)", "").replace("(page 2)", "").replace("(page 4)", "")
    .replace("(page 1 and page 4)", "")
    for line in GOOD_TEXT_PDF.splitlines()
)

AUGUST_TOTAL_TEXT_PDF = GOOD_TEXT_PDF.replace("3,207 tickets", "3,143 tickets")

BLOB_TEXT_PDF = (
    "The backlog was 1,284 and each engineer carried 107; the four regions add to "
    "3,207 and the plan is due 14 November 2026. Pages 1 to 4."
)

GOOD_SCANNED = """\
Reading the scan:

- Invoice number: INV-2026-04817
- Invoice date: 12 March 2026
- VAT registration: GB 384 2915 77
- Pallet handling quantity: 240
- Total due: 14,395.80

The control line near the bottom reads: OCR CONTROL LINE ZEBRA QUARTZ HALYARD
"""

NO_CONTROL_SCANNED = GOOD_SCANNED.replace(
    "The control line near the bottom reads: OCR CONTROL LINE ZEBRA QUARTZ HALYARD",
    "I could not make out the control line.",
)

WRONG_TOTAL_SCANNED = GOOD_SCANNED.replace("14,395.80", "14,359.80")

GOOD_SPREADSHEET = """\
Total EMEA revenue for 2026 is EUR 939,828.78.

Each quarter's amount was converted at that quarter's rate, taken from
'FX Rates'!B2, 'FX Rates'!B3, 'FX Rates'!B4 and 'FX Rates'!B5. Void and
pending deals were excluded, rows already in euros were left alone, and the
EMEA rows whose labels carry a trailing space or a lower-case spelling were
included.
"""

ANNUAL_RATE_SPREADSHEET = """\
Total EMEA revenue for 2026 is EUR 940,112.36, converted at the annual average
rate in 'FX Rates'!B6.
"""

NO_CELLS_SPREADSHEET = "Total EMEA revenue for 2026 is EUR 939,828.78."

GOOD_AUDIO = """\
They finally agreed on a batch size of 250. That decision is stated at about
0:38 into the recording, after the earlier proposal was overruled.

The control phrase spoken in the opening line is "harbour lantern nine".
"""

DECOY_AUDIO = """\
They agreed on a batch size of 500, stated at around 17 seconds in.

The control phrase spoken in the opening line is "harbour lantern nine".
"""

NO_TIME_AUDIO = """\
They finally agreed on a batch size of 250.

The control phrase spoken in the opening line is "harbour lantern nine".
"""

NO_CONTROL_AUDIO = "They finally agreed on a batch size of 250, at 0:38."

GOOD_VIDEO = """\
The error code is ERR-5521. It first becomes visible at 12.4 seconds into the
recording. The amber W-1180 warning earlier at 0:05 clears on its own and is
not the error.
"""

DECOY_VIDEO = "The error is W-1180, first visible at 5 seconds."

WRONG_TIME_VIDEO = "The error code is ERR-5521, first visible at 0:18."


CASES = [
    ("text_pdf", "a complete, correctly cited answer", GOOD_TEXT_PDF, "PASS", None),
    ("text_pdf", "only Asia Pacific named", APAC_ONLY_TEXT_PDF, "FAIL", "names only Asia Pacific"),
    ("text_pdf", "no page citations", NO_PAGES_TEXT_PDF, "FAIL", "cites no page"),
    ("text_pdf", "the August column summed", AUGUST_TOTAL_TEXT_PDF, "FAIL", "3,143"),
    ("text_pdf", "an unsegmentable blob", BLOB_TEXT_PDF, "UNPROVEN", "question by question"),
    ("scanned_pdf", "all five fields read", GOOD_SCANNED, "PASS", None),
    ("scanned_pdf", "the control line unreadable", NO_CONTROL_SCANNED, "UNPROVEN", "control line"),
    ("scanned_pdf", "the total misread", WRONG_TOTAL_SCANNED, "FAIL", "total due"),
    ("spreadsheet", "the right total with all four rate cells", GOOD_SPREADSHEET, "PASS", None),
    ("spreadsheet", "the annual average rate used", ANNUAL_RATE_SPREADSHEET, "FAIL", "940112.36"),
    ("spreadsheet", "the rate cells not shown", NO_CELLS_SPREADSHEET, "FAIL", "rate cells"),
    ("audio", "the agreed size with a timestamp and the control", GOOD_AUDIO, "PASS", None),
    ("audio", "the overruled proposal answered", DECOY_AUDIO, "FAIL", "250"),
    ("audio", "no timestamp given", NO_TIME_AUDIO, "FAIL", "no point in the recording"),
    ("audio", "the control phrase not recovered", NO_CONTROL_AUDIO, "UNPROVEN", "control phrase"),
    ("video", "the error and when it appears", GOOD_VIDEO, "PASS", None),
    ("video", "the amber warning reported as the error", DECOY_VIDEO, "FAIL", "W-1180"),
    ("video", "the wrong moment", WRONG_TIME_VIDEO, "FAIL", "outside the accepted window"),
]


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    verbose = "-v" in argv or "--verbose" in argv
    failures = []
    for subcase, label, reply, expected, fragment in CASES:
        report = G.grade(subcase, reply, KEY)
        got = report["verdict"]
        why = " ".join(str(r) for r in report.get("reasons", []))
        ok = got == expected and (fragment is None or fragment.lower() in why.lower())
        status = "ok  " if ok else "FAIL"
        print("%s %-13s %-9s %s" % (status, subcase, got, label))
        if verbose or not ok:
            print("       reasons: %s" % (why[:400] or "(none)"))
        if not ok:
            failures.append(
                "%s/%s: expected %s%s, got %s (%s)"
                % (subcase, label, expected,
                   " mentioning %r" % fragment if fragment else "", got, why[:200])
            )

    print()
    if failures:
        print("%d of %d controls misbehaved:" % (len(failures), len(CASES)))
        for f in failures:
            print("  - " + f)
        return 1
    print("%d/%d controls behaved correctly (A-10 media graders)" % (len(CASES), len(CASES)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
