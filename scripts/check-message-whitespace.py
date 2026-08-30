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
    semicolon, an apostrophe, a closing paren or a backtick;
  * the character after the run must be a lowercase letter, an apostrophe or a
    backtick -- or an opening paren, but ONLY when a backtick precedes the run,
    because a lowercase word before `(` is the two-column install hint
    ("brew install ollama          (macOS)") and the SQL fixture paren list;
  * at least three whitespace-separated words must already precede the run
    inside the literal, so a run right after a leading label is column padding.

The scan walks the WHOLE FILE rather than each line on its own.  It has to: a
`\`-newline continuation line carries no `"` of its own, so a per-line literal
regex never scans one, and the first version of this gate was therefore blind to
the exact shape its own opening paragraph describes.  Walking the file means
comments, char literals, lifetimes and raw strings are each stepped over
deliberately -- there are self-test cases for all four, because a desynchronised
lexer would silently stop grading everything after the first raw string.
Continuations are resolved as rustc resolves them: the backslash, the newline
and the next line's leading whitespace all vanish, so a CORRECTLY written
multi-line literal stays quiet (there is a self-test case for that too).

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

# The mangled shape: prose word, 4+ spaces, prose word.  A backtick now counts
# as a prose boundary on the LEFT, because this codebase quotes identifiers in
# backticks mid-sentence and "`call_id`          always equals it" is the same
# collapse as "internal          conversation".
MANGLED = re.compile(r"[a-z,;')`]( {4,})[a-z'`]")
# A backtick-quoted identifier, a run, then a parenthesised aside -- the shape
# that survived the first repair pass, in which NEITHER boundary character is a
# prose letter.  The left boundary is restricted to a backtick deliberately: a
# LOWERCASE word before `(` is the deliberate two-column install hint that
# doctor/mod.rs is full of ("brew install ollama          (macOS)") and the SQL
# fixtures in wcore-memory ("INSERT INTO evolved_prompts        (id, ...)"),
# all of which are column padding and measured to be quiet under this pair.
MANGLED_TICK_PAREN = re.compile(r"`( {4,})\(")

SKIP_DIRS = {".git", "target", "node_modules", ".venv"}

# `r"`, `r#"`, `br##"` ...  A raw string, in which `\` is an ordinary character
# and there are no `\`-newline continuations.
RAW_OPEN = re.compile(r'(?:b?r)(#*)"')
# A complete Rust char literal.  Matched strictly, so that a LIFETIME (`'a`) --
# an apostrophe with no closing quote -- cannot swallow the rest of the file and
# desynchronise every literal after it.
CHAR_LIT = re.compile(r"'(?:[^'\\\n]|\\.)'")



def offending_runs(literal: str) -> list[tuple[int, str]]:
    """Return the mangled space runs in `literal` as (offset, run), in order."""
    # A literal that lays itself out with newlines or tabs is an embedded
    # document or a code fixture; its interior spacing is indentation.
    if "\\n" in literal or "\\t" in literal or "\n" in literal or "\t" in literal:
        return []
    found = {}
    for pattern in (MANGLED, MANGLED_TICK_PAREN):
        for m in pattern.finditer(literal):
            # A run right after a leading label is column padding, not prose.
            if len(literal[: m.start(1)].strip().split()) < 3:
                continue
            found[m.start(1)] = m.group(1)
    return sorted(found.items())


def iter_literals(text: str):
    """Yield (literal_source_text, line_of_each_char) for each `"` literal.

    The literal is returned in its SOURCE spelling -- an escape is still the two
    characters that were written -- with `\\`-newline continuations resolved the
    way rustc resolves them: the backslash, the newline, and the leading
    whitespace of the next line all vanish.  `line_of_each_char[i]` is the
    1-based source line character `i` came from, so a run found anywhere in a
    multi-line literal is reported on the line it actually sits on.

    This walks the whole file instead of each line independently, which is the
    only way to see a continuation line at all: a continuation line carries no
    `"` of its own, so a per-line literal regex never scans one.  Walking the
    file means comments, char literals, lifetimes and raw strings each have to
    be stepped over deliberately, or the scan desynchronises and every literal
    after the first raw string is read at the wrong offset.
    """
    i, line, n = 0, 1, len(text)
    while i < n:
        c = text[i]
        if c == "\n":
            line += 1
            i += 1
            continue
        if text.startswith("//", i):
            j = text.find("\n", i)
            i = n if j < 0 else j
            continue
        if text.startswith("/*", i):
            depth, i = 1, i + 2
            while i < n and depth:
                if text.startswith("/*", i):
                    depth += 1
                    i += 2
                elif text.startswith("*/", i):
                    depth -= 1
                    i += 2
                else:
                    if text[i] == "\n":
                        line += 1
                    i += 1
            continue
        if c == "'":
            m = CHAR_LIT.match(text, i)
            # No match means a lifetime; step over the apostrophe alone.
            i = m.end() if m else i + 1
            continue
        m = RAW_OPEN.match(text, i) if c in "rb" else None
        if m and (i == 0 or not (text[i - 1].isalnum() or text[i - 1] == "_")):
            close = '"' + m.group(1)
            j = text.find(close, m.end())
            j = n if j < 0 else j + len(close)
            line += text.count("\n", i, j)
            i = j
            continue
        if c == '"' and not (i and (text[i - 1].isalnum() or text[i - 1] == "_")):
            i += 1
            body_chars, body_lines, start_line = [], [], line
            while i < n:
                d = text[i]
                if d == '"':
                    i += 1
                    break
                if d == "\\":
                    if i + 1 < n and text[i + 1] == "\n":
                        # `\`-newline: rustc drops it and the next line's indent.
                        line += 1
                        i += 2
                        while i < n and text[i] in " \t":
                            i += 1
                        continue
                    body_chars.extend(text[i : i + 2])
                    body_lines.extend([line, line])
                    i += 2
                    continue
                if d == "\n":
                    # A bare newline inside a non-raw literal is legal Rust and
                    # is kept; it also means the literal lays itself out, which
                    # offending_runs already declines to grade.
                    body_chars.append("\n")
                    body_lines.append(line)
                    line += 1
                    i += 1
                    continue
                body_chars.append(d)
                body_lines.append(line)
                i += 1
            body = "".join(body_chars)
            per_char = [start_line] + body_lines + [body_lines[-1] if body_lines else start_line]
            yield '"' + body + '"', per_char
            continue
        i += 1


def scan_text(text: str):
    """Yield (line_number, literal, runs) for every offending literal."""
    for literal, per_char in iter_literals(text):
        hits = offending_runs(literal)
        if not hits:
            continue
        offset = hits[0][0]
        lineno = per_char[offset] if offset < len(per_char) else per_char[0]
        yield lineno, literal, [run for _, run in hits]


def iter_rs_files(roots):
    """Yield every .rs file under `roots`, which may name files or directories.

    A root that is a FILE is scanned directly.  `os.walk` yields nothing at all
    for a file argument, so the previous version answered `OK` for any single
    file handed to it, defect and all -- an empty result read as an absence.
    A root that does not exist, or a file that is not .rs, raises rather than
    contributing nothing silently.
    """
    for root in roots:
        if os.path.isfile(root):
            if not root.endswith(".rs"):
                raise ValueError(f"{root}: not a .rs file; this gate reads Rust sources only")
            yield root
            continue
        if not os.path.isdir(root):
            raise ValueError(f"{root}: no such file or directory")
        for dirpath, dirnames, filenames in os.walk(root):
            dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
            for name in sorted(filenames):
                if name.endswith(".rs"):
                    yield os.path.join(dirpath, name)


def scan_tree(roots):
    problems, scanned = [], 0
    for path in iter_rs_files(roots):
        scanned += 1
        with open(path, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
        for lineno, literal, runs in scan_text(text):
            problems.append((path, lineno, literal, runs))
    return problems, scanned


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
    # --- continuation lines: the shape the module docstring opens with, and
    # --- which a per-line literal regex can never see (wayland#1204, F1).
    (
        "collapsed run on a CONTINUATION line of a multi-line literal",
        'let e = format!(\n    "approval_required declares {x}, but the public handle is \\\n        `call_id`              (`correlation_id` always equals it) and is read \\\n        by the host.",\n);',
        1,
    ),
    (
        "a correctly written continuation keeps its stripped indent quiet",
        'let e = format!(\n    "approval_required declares {x}, but the public handle is `call_id` \\\n                and an ordinary gate is answered with tool_approve keyed by it.",\n);',
        0,
    ),
    (
        "backtick before the run and an opening paren after it",
        '    let s = "the public handle is `call_id`              (`correlation_id` equals it)";',
        1,
    ),
    (
        "backtick before the run and a lowercase word after it",
        '    let s = "the handle it reports is `call_id`          always equals the gate";',
        1,
    ),
    # --- desynchronisation controls: walking the whole file means a raw string,
    # --- a lifetime or a char literal could swallow everything after it.
    (
        "a defect AFTER a raw string is still found",
        '    let re = r"a\\d+          b";\n    let s = "the refusal did not see it and the guard          stayed quiet";',
        1,
    ),
    (
        "a defect AFTER a hashed raw string is still found",
        '    let re = r#"he said "no"          twice"#;\n    let s = "the refusal did not see it and the guard          stayed quiet";',
        1,
    ),
    (
        "a lifetime does not swallow the literal after it",
        "    fn f<'a>(x: &'a str) -> &'a str { x }\n    let s = \"the refusal did not see it and the guard          stayed quiet\";",
        1,
    ),
    (
        "a quote char literal does not swallow the literal after it",
        '    let q = \'"\';\n    let s = "the refusal did not see it and the guard          stayed quiet";',
        1,
    ),
    (
        "an escaped-apostrophe char literal does not desynchronise",
        "    let q = '\\'';\n    let s = \"the refusal did not see it and the guard          stayed quiet\";",
        1,
    ),
    (
        "a run inside a line comment is not a message",
        '    // the refusal did not see it and the guard          stayed quiet\n    let s = "ordinary prose is fine here";',
        0,
    ),
    (
        "a run inside a block comment is not a message",
        '    /* the refusal did not see it and the guard          stayed quiet */\n    let s = "ordinary prose is fine here";',
        0,
    ),
    (
        "a nested block comment closes at the right place",
        '    /* outer /* inner */ still comment          here */\n    let s = "the refusal did not see it and the guard          stayed quiet";',
        1,
    ),
    # --- the two live families the paren rule must NOT reach.  Both are real
    # --- text from the tree; widening the after-class to `(` unconditionally
    # --- reported all of them, which is why the left boundary is a backtick.
    (
        "a two-column install hint with a parenthesised platform tag",
        '    println!("  apt install wlrctl              (Debian/Ubuntu - may need PPA)");',
        0,
    ),
    (
        "a SQL fixture's column-aligned paren list",
        '    let q = "INSERT INTO evolved_prompts              (id, skill_name, score)";',
        0,
    ),
    (
        "a raw string is documented as not covered and stays quiet",
        '    let s = r"the refusal did not see it and the guard          stayed quiet";',
        0,
    ),
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
    try:
        problems, scanned = scan_tree(roots)
    except ValueError as exc:
        print(f"FAIL: {exc}")
        return 2
    if not scanned:
        # An empty result must never read as an absence of defects.
        print(f"FAIL: {' '.join(roots)} contains no .rs file; this gate graded nothing.")
        return 2
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
    print(f"OK: no message literal carries collapsed continuation whitespace ({scanned} file(s)).")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
