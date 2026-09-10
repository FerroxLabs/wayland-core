#!/usr/bin/env python3
"""Refuse a live-run-shaped `not-met` criterion whose note names no host class.

FerroxLabs/wayland#1272 c4b. c4 asked that every criterion requiring a LIVE RUN
be "either measured on real hardware or restated as what the tree can actually
grade -- a criterion that cannot be run is as worthless as one that cannot
fail." The failure mode it targets is silent: a row sits `not-met` for months
because nobody ever wrote down WHICH machine it was waiting for, so no lane can
tell whether it is owed a run or owed a rewrite.

This gate does not judge whether the run happened. It judges whether the row
SAYS WHAT IT IS WAITING FOR. A `not-met` criterion whose text is live-run-shaped
must carry, in its `note:`, a named host class -- Windows, macOS, Linux, a CI
leg, a named box, or an explicit statement that no host can produce it.

TWO DEFECTS IN c4 AS ORIGINALLY WRITTEN, both fixed here rather than inherited:

  1. ITS MATCH WAS SUBSTRING AND CASE-INSENSITIVE, so `pty` matched inside the
     word "empty" and swept in an unrelated issue. Every needle below is
     WORD-ANCHORED with \b, and the self-test pins that "empty" does not match
     `pty` and "unmeasured" does not match `measured`.

  2. IT QUANTIFIED OVER A GROWING SET -- "any other criterion" -- so it could
     never be permanently met: filing one new live-run row un-met it. This gate
     is a STANDING ARM instead. It does not ask "are all of them done"; it asks
     "does each one say what it waits for", which a new row satisfies or fails
     on its own, at the moment it is written.

Exit 0 clean, 1 on a violation, and it says what it did NOT check.
"""
import os
import re
import sys

LEDGER = ".planning/ledger"

# Word-anchored. A needle here means "this row's text describes something only a
# real machine can produce". Substring matching is the bug this replaces.
LIVE_SHAPES = [
    r"\blive run\b", r"\breal hardware\b", r"\bPTY\b", r"\bpseudo-?terminal\b",
    r"\bloadavg\b", r"\bload average\b", r"\bon Windows\b", r"\bon macOS\b",
    r"\bhosted runner\b", r"\bself-hosted\b", r"\bsoak\b", r"\bnightly\b",
    r"\bCI \(Array\)\b", r"\bclean container\b", r"\bon real Windows\b",
]

# A note satisfies the gate by naming WHERE the run must happen -- or by saying
# plainly that nowhere can. "NOT ACHIEVABLE" counts: a row that records why no
# host can produce it is exactly what c4 asked for in its second branch.
HOST_CLASSES = [
    r"\bWindows\b", r"\bmacOS\b", r"\bLinux\b", r"\bhetzner\b", r"\bSeanDesktop\b",
    r"\bCI \(Array\)\b", r"\blinux-containerized\b", r"\bmacos-latest\b",
    r"\bhosted runner\b", r"\bcontainer\b", r"\bNOT ACHIEVABLE\b",
    r"\bno host\b", r"\bcannot be produced\b",
]

CRIT = re.compile(r"^  - id: (c\d+)$", re.M)


def records(text):
    """-> [(id, body)] for each criterion block in a ledger file."""
    hits = list(CRIT.finditer(text))
    for n, m in enumerate(hits):
        end = hits[n + 1].start() if n + 1 < len(hits) else len(text)
        yield m.group(1), text[m.start():end]


def field(body, name):
    m = re.search(r'^    %s: "?(.*?)"?$' % name, body, re.M | re.S)
    return m.group(1) if m else ""


def main():
    if "--self-test" in sys.argv:
        return self_test()
    root = os.getcwd()
    d = os.path.join(root, LEDGER)
    if not os.path.isdir(d):
        print("no %s here" % LEDGER)
        return 1
    bad, checked, skipped = [], 0, 0
    for name in sorted(os.listdir(d)):
        if not name.endswith(".md"):
            continue
        text = open(os.path.join(d, name)).read()
        for cid, body in records(text):
            state = field(body, "state").strip()
            if state != "not-met":
                continue
            ctext = field(body, "text")
            shape = [p for p in LIVE_SHAPES if re.search(p, ctext)]
            if not shape:
                skipped += 1
                continue
            checked += 1
            note = field(body, "note")
            if not any(re.search(p, note, re.I) for p in HOST_CLASSES):
                bad.append((name, cid, shape[0]))
    for name, cid, why in bad:
        print("FAIL: %s %s is a live-run-shaped `not-met` (matched %s) whose "
              "note names no host class. Say WHICH machine it waits for, or "
              "record that none can produce it." % (name, cid, why))
    if bad:
        print("\nFAIL: %d live-run row(s) do not say what they are waiting for."
              % len(bad))
        return 1
    print("OK: all %d live-run-shaped `not-met` criteria name a host class."
          % checked)
    print("    NOT CHECKED: whether the run actually happened, and whether the")
    print("    host named is the right one. %d non-live-run `not-met` rows were"
          % skipped)
    print("    not examined. This gate grades disclosure, never completion.")
    return 0


def self_test():
    # The two defects in c4's original form, pinned so they cannot come back.
    assert not re.search(r"\bpty\b", "the queue was empty", re.I), \
        "pty must not match inside 'empty' -- this is the #1328 sweep bug"
    assert not re.search(r"\bmeasured\b", "rate unmeasured", re.I), \
        "measured must not match inside 'unmeasured'"
    assert re.search(r"\bPTY\b", "needs a PTY-driven run")

    ok = '  - id: c1\n    text: "needs a live run"\n    state: not-met\n    note: "owed on SeanDesktop"\n'
    no = '  - id: c1\n    text: "needs a live run"\n    state: not-met\n    note: "still owed"\n'
    quiet = '  - id: c1\n    text: "a pure refactor"\n    state: not-met\n    note: "owed"\n'
    met = '  - id: c1\n    text: "needs a live run"\n    state: met\n    note: "done"\n'

    def judge(src):
        for cid, body in records(src):
            if field(body, "state").strip() != "not-met":
                return "skip"
            if not any(re.search(p, field(body, "text")) for p in LIVE_SHAPES):
                return "skip"
            return "ok" if any(re.search(p, field(body, "note"), re.I)
                               for p in HOST_CLASSES) else "fail"
        return "none"

    assert judge(ok) == "ok", judge(ok)
    assert judge(no) == "fail", judge(no)
    assert judge(quiet) == "skip", judge(quiet)
    assert judge(met) == "skip", judge(met)
    # NOT ACHIEVABLE is a legitimate answer, not an evasion.
    na = '  - id: c1\n    text: "needs a live run"\n    state: not-met\n    note: "NOT ACHIEVABLE: premise refuted"\n'
    assert judge(na) == "ok", judge(na)
    print("SELF-TEST OK: word anchoring holds (empty/unmeasured), a named host "
          "passes, a bare note fails, a non-live row is skipped, a met row is "
          "skipped, and NOT ACHIEVABLE counts as an answer.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
