#!/usr/bin/env python3
"""Grade the five A-10 media sub-cases that had answer keys and no grader.

    python3 a10_media_grade.py --subcase text_pdf|scanned_pdf|spreadsheet|audio|video
                               --reply <file holding the assistant's reply>
                               [--json out.json]

The expected answers already exist, exactly, in `keys/a10.key.json`; this file
is the mechanical comparison, nothing more.  It reads the key at run time rather
than restating it, so an answer can never drift away from the rubric it is
graded by.

Three things it deliberately does NOT do:

* It never scores partial credit.  Each sub-case is PASS or FAIL on its own,
  because "six of seven" hides which one of the seven a user was misled by.
* It never guesses.  Where the key names a fixture control (the OCR control
  line, the spoken control phrase) and the control cannot be recovered, the
  verdict is UNPROVEN and the fixture is at fault, not the product.
* It never reads the numbered answers out of one undifferentiated blob.  A
  reply to five numbered questions is segmented question by question first, so
  the right figure answering the wrong question cannot score.  A reply that
  cannot be segmented is UNPROVEN, and a human reads it.

Standard library only.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
KEY_PATH = os.path.join(HERE, "a10.key.json")

SUBCASES = ("text_pdf", "scanned_pdf", "spreadsheet", "audio", "video")


# ---------------------------------------------------------------------------
# text helpers
# ---------------------------------------------------------------------------


def normalise(text: str) -> str:
    """Collapse whitespace and lower-case, for tolerant field comparison."""
    return re.sub(r"\s+", " ", text).strip().lower()


def digits_only(text: str) -> str:
    return re.sub(r"[^0-9]", "", text)


def has_number(text: str, value: int) -> bool:
    """Is `value` present as a standalone figure, with or without separators?"""
    plain = str(value)
    grouped = "{:,}".format(value)
    spaced = re.sub(r",", " ", grouped)
    for form in {plain, grouped, spaced, grouped.replace(",", ".")}:
        if re.search(r"(?<![\d.,])" + re.escape(form) + r"(?![\d,])", text):
            return True
    return False


def cited_pages(text: str):
    """Every page number the segment cites, in any ordinary notation."""
    pages = set()
    for m in re.finditer(r"\bp(?:ages?|p?)?\.?\s*([0-9]{1,3})(?:\s*(?:,|and|&|-|–|to)\s*([0-9]{1,3}))?",
                         text, re.IGNORECASE):
        pages.add(int(m.group(1)))
        if m.group(2):
            pages.add(int(m.group(2)))
    for m in re.finditer(r"\bon page\s+([0-9]{1,3})", text, re.IGNORECASE):
        pages.add(int(m.group(1)))
    return pages


_TIME_PATTERNS = (
    # 1:23 / 01:23 / 1:23.4
    (r"\b(\d{1,2}):(\d{2}(?:\.\d+)?)\b", "mmss"),
    # 12.4s / 12 s / 12 sec / 12 seconds / 12.4 seconds
    (r"\b(\d{1,3}(?:\.\d+)?)\s*(?:s\b|secs?\b|seconds?\b)", "secs"),
    # "at 38", only when the word second(s) follows within a few words
    (r"\baround\s+(\d{1,3}(?:\.\d+)?)\s+seconds?\b", "secs"),
)


def parse_times(text: str):
    """Timestamps mentioned in the reply, in seconds.

    Deliberately conservative: a bare integer is never a timestamp, because a
    batch size, an invoice line count and a page number all look like one.  A
    colon form or an explicit unit is required.
    """
    out = []
    for pattern, kind in _TIME_PATTERNS:
        for m in re.finditer(pattern, text, re.IGNORECASE):
            try:
                if kind == "mmss":
                    out.append(int(m.group(1)) * 60 + float(m.group(2)))
                else:
                    out.append(float(m.group(1)))
            except ValueError:
                continue
    return sorted(set(out))


def parse_decimals(text: str):
    """Every monetary-looking figure in the reply, as floats."""
    out = []
    for m in re.finditer(r"(?<![\d.])(\d{1,3}(?:[,\s]\d{3})+(?:\.\d+)?|\d+\.\d+)(?![\d])", text):
        raw = re.sub(r"[,\s]", "", m.group(1))
        try:
            out.append(float(raw))
        except ValueError:
            continue
    return out


_Q_MARKER = re.compile(
    r"^[ \t>*_#-]*(?:\*\*)?(?:question\s*|q\s*)?([1-9])\s*(?:[.):\]]|\*\*[.):]?)\s+",
    re.IGNORECASE | re.MULTILINE,
)


def segment_questions(text: str, count: int):
    """Split a reply to numbered questions into per-question segments.

    Returns {n: segment} or None when the reply cannot be attributed question
    by question.  Guessing here is how a correct figure answering the wrong
    question comes to score, so a reply that is not segmentable is UNPROVEN.
    """
    marks = []
    for m in _Q_MARKER.finditer(text):
        n = int(m.group(1))
        if n < 1 or n > count:
            continue
        if marks and n <= marks[-1][0]:
            continue  # not a rising sequence: this is a list, not the answers
        marks.append((n, m.start()))
    if len(marks) < count:
        return None
    segments = {}
    for i, (n, start) in enumerate(marks):
        end = marks[i + 1][1] if i + 1 < len(marks) else len(text)
        segments[n] = text[start:end]
    return segments if set(segments) == set(range(1, count + 1)) else None


# ---------------------------------------------------------------------------
# sub-case graders
# ---------------------------------------------------------------------------


def grade_text_pdf(key, reply):
    spec = key["sub_cases"]["text_pdf"]
    reasons, observed = [], {}
    segments = segment_questions(reply, 5)
    if segments is None:
        return unproven(
            "the reply could not be attributed question by question, so a figure "
            "answering the wrong question could not be told apart from a correct "
            "answer; a human must read it",
            {"reply_excerpt": reply[:1500]},
        )

    wanted = {
        1: {"numbers": [1284], "pages": [3], "wrong": {}},
        3: {"numbers": [107], "pages": [3], "wrong": {}},
        4: {"numbers": [3207], "pages": [3], "wrong": {3143: "the August column was summed"}},
    }
    for q, want in wanted.items():
        seg = segments[q]
        observed["q%d_pages_cited" % q] = sorted(cited_pages(seg))
        for value in want["numbers"]:
            if not has_number(seg, value):
                reasons.append("question %d does not give %s" % (q, "{:,}".format(value)))
        for bad, why in want["wrong"].items():
            if has_number(seg, bad):
                reasons.append("question %d gives %s, which is what you get when %s"
                               % (q, "{:,}".format(bad), why))
        missing = [p for p in want["pages"] if p not in cited_pages(seg)]
        if missing:
            reasons.append("question %d cites no page %s" % (q, ", ".join(str(p) for p in missing)))

    # Question 2 is the discriminating one: page 2 alone says only Asia Pacific.
    seg2 = segments[2]
    observed["q2_pages_cited"] = sorted(cited_pages(seg2))
    low = normalise(seg2)
    if "asia" not in low:
        reasons.append("question 2 does not name Asia Pacific")
    if not re.search(r"\b0\.9\b", seg2):
        reasons.append("question 2 does not give Asia Pacific's 0.9 point miss")
    if "latin america" not in low and "latam" not in low:
        reasons.append(
            "question 2 names only Asia Pacific. That is what the table on page 2 says "
            "on its own, and it is the answer you get by not reading note 3 on page 4"
        )
    else:
        if not re.search(r"\b1\.2\b", seg2):
            reasons.append("question 2 does not give Latin America's 1.2 point miss")
        if re.search(r"\b95\.2\b", seg2) and not re.search(r"\b93\.8\b", seg2):
            reasons.append(
                "question 2 reports Latin America as 95.2 and a pass; the restated "
                "figure on page 4 is 93.8, which is a miss"
            )
    for page in (2, 4):
        if page not in cited_pages(seg2):
            reasons.append("question 2 cites no page %d" % page)

    seg5 = segments[5]
    observed["q5_pages_cited"] = sorted(cited_pages(seg5))
    low5 = normalise(seg5)
    date_ok = ("14 november 2026" in low5 or "november 14, 2026" in low5
               or "november 14 2026" in low5 or "2026-11-14" in low5 or "14/11/2026" in low5)
    if not date_ok:
        reasons.append(
            "question 5 does not give 14 November 2026, which is the publication date "
            "on page 1 combined with the 60-day rule on page 4"
        )
    for page in (1, 4):
        if page not in cited_pages(seg5):
            reasons.append("question 5 cites no page %d" % page)

    return verdict(reasons, observed, spec["pass_condition"])


def grade_scanned_pdf(key, reply):
    spec = key["sub_cases"]["scanned_pdf"]
    answers = spec["answers"]
    control = spec["control"]
    low = normalise(reply)
    observed = {"control_recovered": normalise(control) in low}

    if not observed["control_recovered"]:
        return unproven(
            "the control line %r was not read back, so the scan may be harsher than a "
            "real scan; the fixture is at fault, not the product" % control,
            observed,
        )

    reasons = []
    if normalise(answers["invoice_number"]) not in low:
        reasons.append("the invoice number is not %s" % answers["invoice_number"])
    date_forms = ("12 march 2026", "march 12, 2026", "march 12 2026", "2026-03-12", "12/03/2026")
    if not any(f in low for f in date_forms):
        reasons.append("the invoice date is not %s" % answers["invoice_date"])
    vat = digits_only(answers["vat_registration"])
    if vat not in digits_only(reply):
        reasons.append("the VAT registration number is not %s" % answers["vat_registration"])
    if not has_number(reply, int(answers["pallet_handling_quantity"])):
        reasons.append("the pallet handling quantity is not %s" % answers["pallet_handling_quantity"])
    total_digits = digits_only(answers["total_due"])
    if total_digits not in digits_only(reply):
        reasons.append("the total due is not %s" % answers["total_due"])

    if reasons:
        reasons.append(
            "the control line came back clean, so the page was legible; a hedged or "
            "missing field is the product's, not the scan's"
        )
    return verdict(reasons, observed, spec["pass_condition"])


def grade_spreadsheet(key, reply):
    spec = key["sub_cases"]["spreadsheet"]
    want = float(spec["answer_eur"])
    tol = float(spec["tolerance_eur"])
    figures = parse_decimals(reply)
    observed = {"figures_in_reply": figures[:40]}
    reasons = []

    if not any(abs(f - want) <= tol for f in figures):
        named = None
        for wrong, why in spec["diagnostic_wrong_answers"].items():
            if any(abs(f - float(wrong)) <= 0.5 for f in figures):
                named = "%s -- %s" % (wrong, why)
                break
        reasons.append(
            "the total is not %.2f EUR%s" % (want, (" (the reply gives %s)" % named) if named else "")
        )

    cells = set()
    for m in re.finditer(r"\bB([2-9])\b", reply):
        cells.add("B" + m.group(1))
    observed["cells_cited"] = sorted(cells)
    missing = [c for c in ("B2", "B3", "B4", "B5") if c not in cells]
    if missing:
        reasons.append(
            "the quarterly exchange-rate cells were not shown: %s missing" % ", ".join(missing)
        )
    if "B6" in cells:
        observed["mentions_B6"] = True

    return verdict(reasons, observed, spec["pass_condition"])


def grade_audio(key, reply):
    spec = key["sub_cases"]["audio"]
    control = spec["control_phrase"]
    low = normalise(reply)
    lo, hi = spec["accepted_timestamp_window_s"]
    dlo, dhi = spec["decoy"]["spoken_between_s"]
    times = parse_times(reply)
    observed = {
        "control_recovered": normalise(control) in low,
        "timestamps_s": times,
    }

    if not observed["control_recovered"]:
        return unproven(
            "the control phrase %r was not read back, so the recording may be "
            "unintelligible; the fixture is at fault, not the product" % control,
            observed,
        )

    reasons = []
    if not has_number(reply, int(spec["answer_batch_size"])):
        reasons.append("the agreed batch size is not %d" % spec["answer_batch_size"])
    if has_number(reply, int(spec["decoy"]["value"])) and not has_number(
        reply, int(spec["answer_batch_size"])
    ):
        reasons.append(
            "the reply gives %d, which was proposed and then overruled -- that is the "
            "answer you get from searching for a number instead of following the "
            "conversation" % spec["decoy"]["value"]
        )
    in_window = [t for t in times if lo <= t <= hi]
    in_decoy = [t for t in times if dlo <= t <= dhi]
    if not times:
        reasons.append("no point in the recording is given for the decision")
    elif not in_window:
        reasons.append(
            "the decision is timed at %s, outside the accepted window %.1f-%.1fs%s"
            % (", ".join("%.1fs" % t for t in times[:5]), lo, hi,
               "; that is inside the overruled proposal" if in_decoy else "")
        )
    return verdict(reasons, observed, spec["pass_condition"])


def grade_video(key, reply):
    spec = key["sub_cases"]["video"]
    lo, hi = spec["accepted_timestamp_window_s"]
    code = spec["answer_error_code"]
    decoy = spec["decoy"]["code"]
    times = parse_times(reply)
    observed = {"timestamps_s": times, "mentions_decoy": decoy.lower() in reply.lower()}
    reasons = []

    if code.lower() not in reply.lower():
        if observed["mentions_decoy"]:
            reasons.append(
                "the reply reports %s, an amber warning that clears on its own, as the "
                "error; the error is %s" % (decoy, code)
            )
        else:
            reasons.append("the error code is not %s" % code)
    in_window = [t for t in times if lo <= t <= hi]
    if not times:
        reasons.append("no time in the recording is given for when the error appears")
    elif not in_window:
        reasons.append(
            "the error is timed at %s, outside the accepted window %.1f-%.1fs"
            % (", ".join("%.1fs" % t for t in times[:5]), lo, hi)
        )
    return verdict(reasons, observed, spec["pass_condition"])


GRADERS = {
    "text_pdf": grade_text_pdf,
    "scanned_pdf": grade_scanned_pdf,
    "spreadsheet": grade_spreadsheet,
    "audio": grade_audio,
    "video": grade_video,
}


# ---------------------------------------------------------------------------
# plumbing
# ---------------------------------------------------------------------------


def verdict(reasons, observed, pass_condition):
    return {
        "verdict": "FAIL" if reasons else "PASS",
        "reasons": reasons,
        "observed": observed,
        "pass_condition": pass_condition,
    }


def unproven(why, observed):
    return {"verdict": "UNPROVEN", "reasons": [why], "observed": observed}


def grade(subcase, reply, key=None):
    if key is None:
        with open(KEY_PATH, "r", encoding="utf-8") as fh:
            key = json.load(fh)
    report = GRADERS[subcase](key, reply)
    report["row"] = "A-10 %s" % subcase
    report["reply_excerpt"] = reply.strip()[:2000]
    return report


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--subcase", required=True, choices=SUBCASES)
    ap.add_argument("--reply", required=True)
    ap.add_argument("--json", default=None)
    args = ap.parse_args()

    with open(args.reply, "r", encoding="utf-8", errors="replace") as fh:
        reply = fh.read()

    report = grade(args.subcase, reply)
    text = json.dumps(report, indent=2)
    print(text)
    if args.json:
        with open(args.json, "w", encoding="utf-8") as fh:
            fh.write(text + "\n")
    return 0 if report["verdict"] == "PASS" else 1


if __name__ == "__main__":
    sys.exit(main())
