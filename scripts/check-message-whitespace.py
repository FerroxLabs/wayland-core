#!/usr/bin/env python3
r"""Refuse Rust string literals whose inter-word whitespace has been mangled.

The defect this catches, exactly (FerroxLabs/wayland#1162, defect D22): a
multi-line message written with `\`-newline continuations gets collapsed onto a
single line by a tooling pass, and the source indentation that used to sit after
the continuation survives *inside the string*.  The user then reads

    Ledgers are keyed by the engine's internal          conversation id

on the terminal.  Nothing in the toolchain notices: rustfmt does not look inside
string literals, and an assertion like `err.contains("cache list")` is satisfied
by the mangled text.

What this gate flags: a run of four or more spaces inside a `"..."` literal that
sits BETWEEN TWO WORDS of running prose.  "Between two words of running prose" is
narrowed hard, because deliberate column padding is everywhere in this codebase
and must not be flagged:

  * the character before the run must be a lowercase letter, a comma, a
    semicolon, an apostrophe or a closing paren;
  * the character after the run must be a lowercase letter, an apostrophe or a
    backtick;
  * at least three whitespace-separated words must already precede the run
    inside the literal, so a run right after a leading label is column padding.

That triple keeps `"bundle        intact (...)"`, `"health:        UNAVAILABLE"`,
`"  ON       conversation history ..."` and `"↑↓ move   i details"` out of the
report while catching every collapsed continuation.

Deliberately NOT covered: any literal that carries an explicit `\n` or `\t` —
those are embedded documents, code fixtures and deliberately laid-out multi-line
output, where interior runs of spaces are indentation and mean something.  Also
not covered: raw strings spanning several source lines, non-space runs, and
anything outside a `"` literal.

Usage:
    check-message-whitespace.py [ROOT ...]
    check-message-whitespace.py --self-test
"""

from __future__ import annotations

import os
import re
import sys

# A Rust string literal on one source line, escapes honoured.
LITERAL = re.compile(r'"(?:[^"\\]|\\.)*"')
# The mangled shape: prose word, 4+ spaces, prose word.
MANGLED = re.compile(r"[a-z,;')]( {4,})[a-z'`]")

SKIP_DIRS = {".git", "target", "node_modules", ".venv"}


def offending_runs(literal: str) -> list[str]:
    """Return the mangled space runs in `literal`, in order."""
    # A literal that lays itself out with explicit newlines or tabs is an
    # embedded document or a code fixture; its interior spacing is indentation.
    if "\\n" in literal or "\\t" in literal:
        return []
    found = []
    for m in MANGLED.finditer(literal):
        # A run right after a leading label is column padding, not prose.
        if len(literal[: m.start(1)].strip().split()) < 3:
            continue
        found.append(m.group(1))
    return found


def scan_text(text: str):
    """Yield (line_number, literal, runs) for every offending literal."""
    for lineno, line in enumerate(text.splitlines(), 1):
        for m in LITERAL.finditer(line):
            runs = offending_runs(m.group(0))
            if runs:
                yield lineno, m.group(0), runs


def scan_tree(roots):
    problems = []
    for root in roots:
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
            for name in filenames:
                if not name.endswith(".rs"):
                    continue
                path = os.path.join(dirpath, name)
                with open(path, encoding="utf-8", errors="replace") as fh:
                    text = fh.read()
                for lineno, literal, runs in scan_text(text):
                    problems.append((path, lineno, literal, runs))
    return problems


SELF_TEST_CASES = [
    # (label, source line, expected number of offending literals)
    (
        "collapsed continuation in a user-facing bail",
        '    anyhow::bail!("Ledgers are keyed by the engine\'s internal          conversation id");',
        1,
    ),
    (
        "collapsed continuation in an assertion message",
        '        assert!(ok, "the refusal did not see it and the guard          stayed quiet");',
        1,
    ),
    (
        "two runs in one literal are one problem",
        '    let s = "a leading clause here,          then more,          and more";',
        1,
    ),
    ("label column padding is not prose", '    println!("bundle        intact (published {})");', 0),
    ("uppercase after the run is a column", '    println!("health:        UNAVAILABLE — {e}");', 0),
    ("three spaces is a key hint, not a collapse", '    let s = "↑↓ move   i details   esc close";', 0),
    ("padding right after a leading label", '    println!("  count      not measured here now");', 0),
    ("ordinary prose is untouched", '    anyhow::bail!("no cache ledger for this id in that dir");', 0),
    ("a run inside code, outside any literal", "    let x = foo(a,          b);", 0),
    (
        "an embedded code fixture keeps its indentation",
        '    let src = "def foo():\\n    pass and more words here\\n";',
        0,
    ),
    (
        "multi-line output layout keeps its padding",
        '    println!("the memory entry was corrected as asked\\n      now reads: {}", s);',
        0,
    ),
    ("tab-indented fixture text is not this defect", '    let s = "one\\ttwo";', 0),
]


def self_test() -> int:
    failures = 0
    for label, line, expected in SELF_TEST_CASES:
        got = len(list(scan_text(line)))
        status = "ok  " if got == expected else "FAIL"
        if got != expected:
            failures += 1
        print(f"  {status} {label} (want {expected}, got {got})")
    if failures:
        print(f"SELF-TEST FAILED: {failures} of {len(SELF_TEST_CASES)} case(s)")
        return 1
    print(f"SELF-TEST OK: {len(SELF_TEST_CASES)} cases")
    return 0


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    roots = [a for a in argv[1:] if not a.startswith("-")] or ["crates"]
    problems = scan_tree(roots)
    for path, lineno, literal, runs in problems:
        shown = literal if len(literal) <= 200 else literal[:200] + '…"'
        widths = ", ".join(str(len(r)) for r in runs)
        print(f"{path}:{lineno}: run(s) of {widths} spaces mid-sentence: {shown}")
    if problems:
        print(
            f"FAIL: {len(problems)} message literal(s) carry collapsed continuation whitespace.\n"
            "      Join the words with a single space, or keep the message on several\n"
            "      source lines with explicit `\\n` / concatenation."
        )
        return 1
    print("OK: no message literal carries collapsed continuation whitespace.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
