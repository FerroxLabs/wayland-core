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
    semicolon, an apostrophe, a closing paren, a backtick, a sentence-ending
    `.`, `!` or `?`, or an em/en dash;
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

The boundary classes are an ALLOWLIST and three separate rounds have found a
collapse that fell outside the class as it then stood, so a bare `OK` from this
gate has repeatedly been read as "the tree is clean" when it only ever meant
"nothing this gate looks at is wrong".  Every successful run therefore also
prints the SIZE OF ITS OWN BLIND ZONE — the interior 4+ space runs inside graded
literals that the boundary narrowing declined to examine.  That number is not a
failure and is not expected to be zero; it exists so the next reader can see
how much of the tree the OK does not speak for, and can enumerate it rather
than assume it.

Usage:
    check-message-whitespace.py [ROOT ...]
    check-message-whitespace.py --self-test
"""

from __future__ import annotations

import os
import re
import sys

# The mangled shape: prose word, 4+ spaces, prose word.  A backtick counts as a
# prose boundary on the LEFT, because this codebase quotes identifiers in
# backticks mid-sentence and "`call_id`          always equals it" is the same
# collapse as "internal          conversation".  So does a SENTENCE-ENDING
# `.`/`!`/`?`: a second collapse survived the first repair pass in the very same
# file, one sentence apart from the one it fixed, only because the character to
# the left of the run was a full stop -- "!= call_id {b}.          `Approval...`".
# Two spaces after a full stop is typography; four is the collapse.  So does an
# EM or EN DASH, and that one was the third recurrence of a single root cause: a
# fourth collapse sat three source lines below one an earlier pass had repaired,
# in the same run of `assert!` statements, and survived only because the
# character to its left was `—` rather than `;`.  Measured over the tree each time:
# the `.`/`!`/`?` widening reported exactly one literal and the `—`/`–` widening
# exactly two, and all three were the defect.
#
# THIS CLASS IS AN ALLOWLIST, NOT A CLOSED SET, and it is deliberately not
# closed.  Enumerated over the whole tree (this is what `unreported_runs` counts
# and what `main` prints on success): of the interior 4+ space runs this gate
# does not report, exactly TWELVE have a prose character on the RIGHT, and after
# the dash widening not one of them is a defect — seven sit behind a `:` (the
# `next:` / `bound:` / `retry:` column separator in cron.rs and its siblings),
# three behind an upper-case status word (`  ON       conversation history ...`
# in doctor/mod.rs), one behind a `|` in the TUI banner art, and one is
# `"bundle        intact"`, which the three-word floor holds out.  Admitting `:`
# or an upper-case letter on the left re-admits all eleven of those, which is
# precisely the column padding the narrowing exists to allow.  So the NAMED
# REMAINING GAPS on the left are: `:`, upper-case letters, digits, `]`/`}`, and
# typographic quotes and ellipsis.  The first two are excluded with measured
# false positives; the rest are simply absent from this tree today, and should
# be admitted the same way the dash was — measure first, then widen.
MANGLED = re.compile(r"[a-z,;')`.!?—–]( {4,})[a-z'`]")
# A backtick-quoted identifier, a run, then a parenthesised aside -- the shape
# that survived the first repair pass, in which NEITHER boundary character is a
# prose letter.  The left boundary is restricted to a backtick deliberately: a
# LOWERCASE word before `(` is the deliberate two-column install hint that
# doctor/mod.rs is full of ("brew install ollama          (macOS)") and the SQL
# fixtures in wcore-memory ("INSERT INTO evolved_prompts        (id, ...)"),
# all of which are column padding and measured to be quiet under this pair.
MANGLED_TICK_PAREN = re.compile(r"`( {4,})\(")

# Every interior run of 4+ spaces, with no boundary narrowing whatsoever.
# Used only to SIZE the blind zone, never to fail a run.
ANY_RUN = re.compile(r" {4,}")

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
            # The OPENING QUOTE is dropped before counting: it is not a word,
            # and counting it made the documented three-word floor a two-word
            # one, so `"  step 1.          initial import"` was reported as
            # prose.  A self-test case grades each side of the boundary.
            before = literal[: m.start(1)].lstrip('"').strip()
            if len(before.split()) < 3:
                continue
            found[m.start(1)] = m.group(1)
    return sorted(found.items())


def unreported_runs(literal: str) -> int:
    """How many interior 4+ space runs in `literal` this gate declines to grade.

    The boundary classes in `MANGLED` are an allowlist, so `offending_runs`
    returning nothing has never meant the literal is clean.  This counts what
    was skipped, on the same literals `offending_runs` grades and with the same
    leading/trailing pads excluded, so `main` can print the size of its own
    blind zone instead of letting `OK` stand in for it.
    """
    body = literal[1:-1]
    if "\\n" in body or "\\t" in body or "\n" in body or "\t" in body:
        return 0
    reported = {offset - 1 for offset, _ in offending_runs(literal)}
    skipped = 0
    for m in ANY_RUN.finditer(body):
        # A run at either end is padding around the message, not inside it.
        if m.start() == 0 or m.end() == len(body):
            continue
        if m.start() not in reported:
            skipped += 1
    return skipped


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
    problems, scanned, blind = [], 0, 0
    for path in iter_rs_files(roots):
        scanned += 1
        with open(path, encoding="utf-8", errors="replace") as fh:
            text = fh.read()
        for literal, _ in iter_literals(text):
            blind += unreported_runs(literal)
        for lineno, literal, runs in scan_text(text):
            problems.append((path, lineno, literal, runs))
    return problems, scanned, blind


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
        "a defect AFTER a raw string holding an odd number of quotes",
        '    let re = r#"a bare " lives here;"#;\n    let s = "the refusal did not see it and the guard          stayed quiet";',
        1,
    ),
    (
        "one lifetime does not pair with an apostrophe in the next message",
        "    fn f<'a>(x: &str) {}\n    let s = \"Ledgers are keyed by the engine's internal          conversation id\";",
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
        "an unclosed quote in a line comment does not eat the next message",
        '    // the operator said "run it and see\n    let s = "the refusal did not see it and the guard          stayed quiet";',
        1,
    ),
    (
        "a run inside a block comment is not a message",
        '    /* the refusal did not see it and the guard          stayed quiet */\n    let s = "ordinary prose is fine here";',
        0,
    ),
    (
        "an unclosed quote in a block comment does not eat the next message",
        '    /* the operator said "run it */\n    let s = "the refusal did not see it and the guard          stayed quiet";',
        1,
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
    # --- sentence-ending punctuation on the LEFT of the run.  The first repair
    # --- pass left a second collapse in generate.rs one sentence away from the
    # --- one it fixed, because a full stop was not a prose boundary here.
    (
        "a full stop before the run and a backtick after it",
        '    let s = "correlation_id {a} does not equal call_id {b}.                  `ApprovalRequired` equals it";',
        1,
    ),
    (
        "a full stop before the run and a lowercase word after it",
        '    let s = "there is no cache ledger row for that id.          run cache list to see them";',
        1,
    ),
    (
        "a question mark before the run",
        '    let s = "why did the gate stay quiet about it?          a continuation line carries no quote";',
        1,
    ),
    (
        "an exclamation mark before the run",
        '    let s = "the sandbox refused to start at all!          the probe never ran on this host";',
        1,
    ),
    (
        "a full stop right after a leading label is still column padding",
        '    println!("  step 1.          initial import");',
        0,
    ),
    (
        "two words before the run is a label column, not prose",
        '    println!("the ledger          has no row for it");',
        0,
    ),
    (
        "three words before the run is prose",
        '    println!("the cache ledger          has no row for it");',
        1,
    ),
    (
        "an upper-case second column after a full stop is still a column",
        '    println!("  the migration ran to completion.          DONE");',
        0,
    ),
    # --- an EM/EN DASH on the LEFT of the run.  Third recurrence of the same
    # --- root cause: the repair pass that closed the full-stop half left a
    # --- fourth collapse three source lines below one it had just fixed, in
    # --- the same run of `assert!` statements, because the character to the
    # --- left of the run was a dash and not a prose letter.
    (
        "an em dash before the run and a lowercase word after it",
        '    let s = "two full connect deadlines must be IN this window —              '
        'otherwise the bound is measuring a turn that never dialled";',
        1,
    ),
    (
        "an en dash before the run",
        '    let s = "the adapter drops the boundaries the engine marked –          '
        'the cache never warms on this path";',
        1,
    ),
    (
        "an em dash before a correctly written continuation stays quiet",
        'let e = format!(\n    "{p} honours cache breakpoints but defaults prompt_caching OFF — \\\n'
        '                the engine would mark boundaries the adapter then drops",\n);',
        0,
    ),
    (
        "an em dash before an upper-case second column is still a column",
        '    println!("  session persistence —          UNAVAILABLE on this host");',
        0,
    ),
]


DEFECT_LINE = '    anyhow::bail!("Ledgers are keyed by the engine\'s internal          conversation id");\n'
CLEAN_LINE = '    anyhow::bail!("no cache ledger for this id in that dir");\n'
# A run the boundary narrowing declines to grade: `:` on the left is the column
# separator this gate must not report, so this line is quiet AND blind.
PADDED_LINE = '    println!("  next:    driven externally and not from the clock");\n'


def _tree_cases() -> list[tuple[str, bool]]:
    """Grade scan_tree's ROOT HANDLING, which no line case can reach.

    os.walk yields nothing for a file, so before this was graded the gate
    printed OK and exited 0 for any single file handed to it, defect and all.
    Every case below distinguishes "graded and found nothing" from "graded
    nothing", which is the failure the file case is an instance of.
    """
    import contextlib
    import io
    import tempfile

    def quiet(*argv):
        """main()'s exit code without its report on this run's stdout."""
        with contextlib.redirect_stdout(io.StringIO()):
            return main(list(argv))

    results = []
    with tempfile.TemporaryDirectory() as d:
        sub = os.path.join(d, "sub")
        os.makedirs(sub)
        bad = os.path.join(d, "bad.rs")
        good = os.path.join(sub, "good.rs")
        other = os.path.join(d, "notes.txt")
        empty = os.path.join(d, "empty")
        os.makedirs(empty)
        with open(bad, "w", encoding="utf-8") as fh:
            fh.write(DEFECT_LINE)
        with open(good, "w", encoding="utf-8") as fh:
            fh.write(CLEAN_LINE)
        with open(other, "w", encoding="utf-8") as fh:
            fh.write(DEFECT_LINE)

        problems, scanned, _ = scan_tree([d])
        results.append(("a directory root finds the defect under it", (len(problems), scanned) == (1, 2)))

        problems, scanned, _ = scan_tree([bad])
        results.append(("a FILE root is scanned, not silently skipped", (len(problems), scanned) == (1, 1)))

        problems, scanned, blind = scan_tree([good])
        results.append(("a clean file root is graded and reports nothing", (len(problems), scanned) == (0, 1)))
        results.append(("a clean file has an EMPTY blind zone, not an unmeasured one", blind == 0))

        # The blind-zone count is the one number that says how much of the tree
        # an OK does not speak for.  If it silently went to zero, OK would go
        # back to reading as "clean", so it is graded in both directions.
        padded = os.path.join(d, "padded.rs")
        with open(padded, "w", encoding="utf-8") as fh:
            fh.write(PADDED_LINE)
        problems, scanned, blind = scan_tree([padded])
        results.append(
            ("column padding is counted as blind, not reported", (len(problems), scanned, blind) == (0, 1, 1))
        )

        results.append(("a directory with no Rust in it exits 2, not OK", quiet("x", empty) == 2))
        results.append(("a missing root exits 2, not OK", quiet("x", os.path.join(d, "nope")) == 2))
        results.append(("a non-.rs file root exits 2, not OK", quiet("x", other) == 2))
        results.append(("a file root with a defect exits 1", quiet("x", bad) == 1))
        results.append(("a file root with no defect exits 0", quiet("x", good) == 0))
    return results


def self_test() -> int:
    failures = 0
    for label, line, expected in SELF_TEST_CASES:
        got = len(list(scan_text(line)))
        status = "ok  " if got == expected else "FAIL"
        if got != expected:
            failures += 1
        print(f"  {status} {label} (want {expected}, got {got})")
    tree_cases = _tree_cases()
    for label, ok in tree_cases:
        print(f"  {'ok  ' if ok else 'FAIL'} {label}")
        if not ok:
            failures += 1
    total = len(SELF_TEST_CASES) + len(tree_cases)
    if failures:
        print(f"SELF-TEST FAILED: {failures} of {total} case(s)")
        return 1
    print(f"SELF-TEST OK: {total} cases")
    return 0


def main(argv: list[str]) -> int:
    if "--self-test" in argv:
        return self_test()
    roots = [a for a in argv[1:] if not a.startswith("-")] or ["crates"]
    try:
        problems, scanned, blind = scan_tree(roots)
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
    # The boundary classes are an allowlist, so OK means "this gate found
    # nothing", never "the tree is clean".  Print how much it did not look at.
    print(
        f"    blind zone: {blind} interior run(s) of 4+ spaces sit outside the boundary\n"
        "    classes and were NOT graded.  This is not a failure; it is the part of\n"
        "    the tree this OK does not speak for."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
