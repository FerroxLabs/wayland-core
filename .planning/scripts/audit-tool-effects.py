#!/usr/bin/env python3
"""For each production `/src/` tool declaring `is_concurrency_safe -> true`,
report the mutating effects present in the SAME file.

This is a triage instrument, not a verdict: it surfaces candidates that a human
then reads. Its job is to make sure nothing is missed, so it is deliberately
over-inclusive and every hit is reviewed by hand.

Self-test (--selftest), per LANE-BRIEF 6b-ii, three assertions:
  1. known-positive: a snippet containing `std::fs::write(` is flagged FS_WRITE
  2. known-negative: a snippet with only `std::fs::read_to_string(` is NOT flagged
     as a write
  3. old-broken-matcher control: a substring matcher for "write" WOULD have
     flagged the known-negative (because of `write!`/`writeln!` formatting
     macros), proving the word-boundary/receiver-aware patterns do real work.
"""

import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))

# Each pattern is (label, compiled regex). Ordered most-specific first.
PATTERNS = [
    ("FS_WRITE", re.compile(r"\b(?:std::|tokio::)?fs::write\s*\(")),
    ("FS_CREATE_DIR", re.compile(r"\b(?:std::|tokio::)?fs::create_dir(?:_all)?\s*\(")),
    ("FS_FILE_CREATE", re.compile(r"\bFile::create\s*\(")),
    ("FS_OPENOPTIONS", re.compile(r"\bOpenOptions::new\s*\(")),
    ("FS_REMOVE", re.compile(r"\b(?:std::|tokio::)?fs::remove_(?:file|dir_all|dir)\s*\(")),
    ("FS_RENAME", re.compile(r"\b(?:std::|tokio::)?fs::rename\s*\(")),
    ("FS_COPY", re.compile(r"\b(?:std::|tokio::)?fs::copy\s*\(")),
    ("FS_SET_PERMS", re.compile(r"\bset_permissions\s*\(")),
    ("TEMPFILE", re.compile(r"\b(?:NamedTempFile|tempdir|tempfile)\s*(?:::new)?\s*\(")),
    ("TEMP_DIR", re.compile(r"\benv::temp_dir\s*\(")),
    ("PROC_SPAWN", re.compile(r"\b(?:Command::new|shell_command(?:_argv|_builder)?)\s*\(")),
    ("HTTP_POST", re.compile(r"\.post\s*\(")),
    ("HTTP_PUT", re.compile(r"\.put\s*\(")),
    ("HTTP_DELETE", re.compile(r"\.delete\s*\(")),
    ("HTTP_PATCH", re.compile(r"\.patch\s*\(")),
    ("GLOBAL_STATIC_MUT", re.compile(r"\bstatic\s+\w+\s*:\s*(?:Lazy|OnceLock|Mutex|RwLock|AtomicU?\w*)")),
    # ---- DELEGATED WRITERS (instrument repair #4, this lane) ----------------
    # The live known-positive control caught this hole: `edit.rs` mutates the
    # filesystem via `wcore_config::atomic_write(..)` and its only raw
    # `fs::write` calls are test-only, so the scanner reported hits=0 for a
    # tool that plainly writes. A tool that delegates its mutation to a helper
    # would therefore have produced a false `EFFECTS none-detected` -- the
    # exact free-absence failure LANE-BRIEF 3b-i warns about.
    ("DELEGATED_ATOMIC_WRITE", re.compile(r"\batomic_write\s*\(")),
    # Deliberately broad and therefore noisy; every hit is reviewed by hand.
    # `write!`/`writeln!` are formatting macros, not effects -- excluded via
    # the negative lookahead on `!`.
    ("SUSPECT_WRITE_HELPER", re.compile(
        r"\b\w*(?:_write|write_|persist|spill|save_|_save|store_to|dump_)\w*\s*(?!\!)\("
    )),
]


TESTMOD = re.compile(
    r"#\[cfg\([^\]]*\btest\b[^\]]*\)\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{"
)


def _blank_preserving_lines(s):
    """Replace every char with a space EXCEPT newlines.

    INSTRUMENT REPAIR (found by this lane, 2026-07-30): the first version of
    this script deleted block comments and blanked `#[cfg(test)]` spans with a
    flat run of spaces. Both destroyed the newlines inside the span, so EVERY
    line number reported after a test module was shifted upward. glob.rs was
    reported with effects at lines 53-59 that are really at 1145-1157. Line
    numbers are the whole point of this report, so this is a result-destroying
    defect, not a cosmetic one. Assertion A4 in --selftest guards it.
    """
    return "".join("\n" if c == "\n" else " " for c in s)


def strip_comments_and_strings(text):
    """Blank Rust comments, preserving every byte offset and line number.

    INSTRUMENT REPAIR #2 (found by this lane, 2026-07-30, and this one is the
    dangerous direction). The previous version used
    `re.sub(r"/\\*.*?\\*/", ..., flags=re.S)`. In `crates/wcore-tools/src/glob.rs`
    the GLOB PATTERN STRING `"**/*.rs"` contains the two bytes `/` `*`, so the
    regex opened a phantom block comment there and blanked everything up to the
    next `*/` -- swallowing ~130 lines of real code including the
    `#[cfg(test)]` marker the test-module suppressor keys on. Consequence: the
    suppressor silently found ZERO test modules in that file, and, worse, a
    phantom comment can blank a REAL production `fs::write` and produce a false
    `EFFECTS none-detected`. An absence this instrument reports is exactly the
    claim LANE-BRIEF 3b-i says is free to pass on a dead instrument, so it had
    to be fixed rather than noted.

    This replacement is a character scanner that understands the four Rust
    lexical contexts that can contain a `/*`-looking byte pair: line comments,
    (nestable) block comments, normal string literals with backslash escapes,
    raw strings `r"..."` / `r#"..."#`, and char literals. Comments are blanked;
    string CONTENT is left intact (a path literal is evidence), except that any
    `/*`, `*/` or `//` inside a string can no longer be misread as a comment
    delimiter, which is the entire point.
    """
    out = list(text)
    i = 0
    n = len(text)

    def blank(a, b):
        for k in range(a, b):
            if out[k] != "\n":
                out[k] = " "

    while i < n:
        c = text[i]
        # raw string: r"..."  r#"..."#  r##"..."##
        if c == "r" and i + 1 < n and text[i + 1] in '"#':
            j = i + 1
            hashes = 0
            while j < n and text[j] == "#":
                hashes += 1
                j += 1
            if j < n and text[j] == '"':
                term = '"' + "#" * hashes
                end = text.find(term, j + 1)
                i = n if end == -1 else end + len(term)
                continue
        if c == '"':
            j = i + 1
            while j < n:
                if text[j] == "\\":
                    j += 2
                    continue
                if text[j] == '"':
                    break
                j += 1
            i = j + 1
            continue
        if c == "'":
            # char literal or a lifetime. Only treat as a literal when it
            # closes within 4 chars; otherwise it is `'a` and we move on.
            j = i + 1
            if j < n and text[j] == "\\":
                j += 2
            elif j < n:
                j += 1
            if j < n and text[j] == "'":
                i = j + 1
                continue
            i += 1
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "/":
            j = text.find("\n", i)
            j = n if j == -1 else j
            blank(i, j)
            i = j
            continue
        if c == "/" and i + 1 < n and text[i + 1] == "*":
            depth = 1
            j = i + 2
            while j < n and depth:
                if text[j] == "/" and j + 1 < n and text[j + 1] == "*":
                    depth += 1
                    j += 2
                    continue
                if text[j] == "*" and j + 1 < n and text[j + 1] == "/":
                    depth -= 1
                    j += 2
                    continue
                j += 1
            blank(i, j)
            i = j
            continue
        i += 1
    return "".join(out)


def line_of(text, idx):
    return text.count("\n", 0, idx) + 1


def scan(path):
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        raw = f.read()
    text = strip_comments_and_strings(raw)
    # Blank out test-gated `mod` spans so test helpers do not count.
    #
    # INSTRUMENT REPAIR #3 (this lane): the first pattern demanded a literal
    # `#[cfg(test)]`, so it did not see
    # `#[cfg(all(test, feature = "image-inspect"))] mod tests` in
    # image_inspect_tool.rs, and that file's five test-only `fs::write` calls
    # were reported as production effects. TESTMOD below accepts any `cfg`
    # attribute whose predicate mentions `test` as a whole word.
    for m in TESTMOD.finditer(text):
        start = m.end() - 1
        depth = 0
        k = start
        while k < len(text):
            if text[k] == "{":
                depth += 1
            elif text[k] == "}":
                depth -= 1
                if depth == 0:
                    break
            k += 1
        text = (
            text[:start]
            + _blank_preserving_lines(text[start : k + 1])
            + text[k + 1 :]
        )
    hits = []
    for label, rx in PATTERNS:
        for m in rx.finditer(text):
            hits.append((label, line_of(text, m.start())))
    return hits


def selftest():
    pos = 'fn f() { std::fs::write(&p, s).ok(); }'
    neg = 'fn f() { let s = std::fs::read_to_string(&p)?; write!(out, "{}", s)?; }'
    fails = []
    fs_write = dict(PATTERNS)["FS_WRITE"]
    if not fs_write.search(pos):
        fails.append("A1 known-positive: fs::write NOT flagged")
    if fs_write.search(neg):
        fails.append("A2 known-negative: read_to_string/write! WAS flagged as a write")
    naive = re.compile(r"write")
    if not naive.search(neg):
        fails.append("A3 old-broken-matcher control: naive 'write' substring did NOT hit "
                     "the negative, so the control is vacuous")

    # A4 LINE PRESERVATION. A block comment and a #[cfg(test)] mod both sit
    # BEFORE the target, so a length-collapsing blanker reports the wrong line.
    sample = (
        "fn a() {}\n"                      # 1
        "/* block\n"                       # 2
        "   comment\n"                     # 3
        "   spanning */\n"                 # 4
        "#[cfg(test)]\n"                   # 5
        "mod t {\n"                        # 6
        "    fn helper() {\n"              # 7
        "        std::fs::write(&p, s);\n" # 8  <- MUST NOT be reported
        "    }\n"                          # 9
        "}\n"                              # 10
        "fn b() { std::fs::write(&q, s); }\n"  # 11 <- MUST be reported, as 11
    )
    import tempfile as _tf

    with _tf.NamedTemporaryFile("w", suffix=".rs", delete=False) as fh:
        fh.write(sample)
        sample_path = fh.name
    try:
        hits = scan(sample_path)
    finally:
        os.unlink(sample_path)
    writes = sorted(l for (lab, l) in hits if lab == "FS_WRITE")
    if writes != [11]:
        fails.append(
            "A4 line-preservation: expected FS_WRITE at [11] (test-mod write "
            "suppressed, real write at its true line), got %s" % writes
        )
    # A5 OLD-BROKEN-INSTRUMENT CONTROL. Re-run the identical sample through the
    # pre-repair logic (delete block comments outright, blank the test module
    # with a flat run of spaces) and assert it gets a DIFFERENT, WRONG answer.
    # Without this, A4 would also pass on a hypothetical instrument that never
    # had the bug, and would not prove the repair does anything.
    def _old_broken_scan(src):
        t = re.sub(r"//[^\n]*", "", src)
        t = re.sub(r"/\*.*?\*/", "", t, flags=re.S)          # <- eats newlines
        for mm in re.finditer(r"#\[cfg\(test\)\]\s*(?:pub\s+)?mod\s+\w+\s*\{", t):
            st = mm.end() - 1
            d = 0
            kk = st
            while kk < len(t):
                if t[kk] == "{":
                    d += 1
                elif t[kk] == "}":
                    d -= 1
                    if d == 0:
                        break
                kk += 1
            t = t[:st] + (" " * (kk - st + 1)) + t[kk + 1 :]  # <- eats newlines
        rx = dict(PATTERNS)["FS_WRITE"]
        return sorted(t.count("\n", 0, mm2.start()) + 1 for mm2 in rx.finditer(t))

    old_answer = _old_broken_scan(sample)
    if old_answer == [11]:
        fails.append(
            "A5 old-broken-instrument control: the PRE-REPAIR logic returned the "
            "correct answer [11], so this sample does not exercise the bug and "
            "A4 proves nothing"
        )
    print("OLD_BROKEN_INSTRUMENT_ANSWER=%s  REPAIRED_ANSWER=%s" % (old_answer, writes))

    # ---- A6/A7: GLOB-PATTERN PHANTOM COMMENT (instrument repair #2) ----------
    # `"**/*.rs"` contains the bytes `/` `*`. The old regex opened a block
    # comment there and blanked forward to the next `*/`, swallowing real code.
    # The write must sit BETWEEN the phantom opener and the phantom closer,
    # otherwise the old logic never blanks it and A7 cannot discriminate.
    #   line 1: "**/*.rs"  -> the bytes `/` `*` look like a block-comment OPEN
    #   line 3: "logs/**/x" -> the bytes `*` `/` look like the CLOSE
    # so the pre-repair regex blanks lines 1-3 and loses the write on line 2.
    glob_sample = (
        'fn a() { let p = "**/*.rs"; }\n'            # 1  phantom opener
        'fn c() { std::fs::write(&z, s); }\n'        # 2  <- MUST be reported
        'fn b() { let q = "logs/**/x"; }\n'          # 3  phantom closer
    )
    with _tf.NamedTemporaryFile("w", suffix=".rs", delete=False) as fh:
        fh.write(glob_sample)
        gpath = fh.name
    try:
        ghits = scan(gpath)
    finally:
        os.unlink(gpath)
    gwrites = sorted(l for (lab, l) in ghits if lab == "FS_WRITE")
    if gwrites != [2]:
        fails.append(
            "A6 glob-phantom-comment: expected FS_WRITE at [2], got %s -- a glob "
            "pattern string is being misread as a block comment" % gwrites
        )
    old_g = _old_broken_scan(glob_sample)
    if old_g == [2]:
        fails.append(
            "A7 old-broken-instrument control (glob): PRE-REPAIR logic also "
            "returned [3], so this sample does not exercise the bug"
        )
    print("OLD_BROKEN_GLOB_ANSWER=%s  REPAIRED_GLOB_ANSWER=%s" % (old_g, gwrites))

    # ---- A8: cfg(all(test, feature=..)) suppression (instrument repair #3) ---
    cfgall = (
        '#[cfg(all(test, feature = "x"))]\n'         # 1
        'mod tests {\n'                              # 2
        '    fn h() { std::fs::write(&a, b); }\n'    # 3  <- MUST be suppressed
        '}\n'                                        # 4
        'fn real() { std::fs::write(&c, d); }\n'     # 5  <- MUST be reported
    )
    with _tf.NamedTemporaryFile("w", suffix=".rs", delete=False) as fh:
        fh.write(cfgall)
        cpath = fh.name
    try:
        chits = scan(cpath)
    finally:
        os.unlink(cpath)
    cwrites = sorted(l for (lab, l) in chits if lab == "FS_WRITE")
    if cwrites != [5]:
        fails.append(
            "A8 cfg(all(test,..)) suppression: expected FS_WRITE at [5], got %s"
            % cwrites
        )
    old_c = re.compile(r"#\[cfg\(test\)\]").search(cfgall)
    if old_c:
        fails.append(
            "A8b old-broken-instrument control: literal `#[cfg(test)]` matched "
            "the cfg(all(..)) sample, so it does not exercise the bug"
        )
    print("SELFTEST_ASSERTIONS=9")
    for f in fails:
        print("SELFTEST_FAIL: " + f)
    print("SELFTEST=%s" % ("PASS" if not fails else "FAIL"))
    return 0 if not fails else 1


TARGETS_FILE = os.path.join(
    ROOT, ".planning", "evidence", "lane-concurrency-safe", "census.txt"
)


def main():
    if "--selftest" in sys.argv:
        sys.exit(selftest())
    targets = []
    with open(TARGETS_FILE, encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if line.startswith("PROD_TRUE "):
                fl = line[len("PROD_TRUE ") :]
                path, ln = fl.rsplit(":", 1)
                targets.append((path, int(ln)))
    print("TARGETS=%d" % len(targets))

    # ---- LIVE CORPUS CONTROLS (LANE-BRIEF 3b-i) -----------------------------
    # 30 of the 34 targets below report `EFFECTS none-detected`. That is an
    # ABSENCE claim, and an absence is free on a dead instrument. So prove the
    # scanner is alive on real files in the SAME invocation, using tools that
    # MUST have the effect: write.rs and edit.rs both mutate the filesystem.
    print("")
    print("--- CONTROLS (known-positive, same invocation) ---")
    control_ok = True
    for ctrl in [
        "crates/wcore-tools/src/write.rs",
        "crates/wcore-tools/src/edit.rs",
    ]:
        cpath = os.path.join(ROOT, ctrl)
        if not os.path.exists(cpath):
            print("CONTROL %-40s MISSING-FILE" % ctrl)
            control_ok = False
            continue
        chits = scan(cpath)
        labels = sorted({h[0] for h in chits})
        print("CONTROL %-40s hits=%d labels=%s" % (ctrl, len(chits), labels))
        if not chits:
            control_ok = False
    print("CONTROLS_ALIVE=%s" % ("YES" if control_ok else "NO -- ABSENCES BELOW ARE VOID"))
    print("")
    byfile = {}
    for path, ln in targets:
        byfile.setdefault(path, []).append(ln)
    for path in sorted(byfile):
        full = os.path.join(ROOT, path)
        hits = scan(full)
        labels = sorted({h[0] for h in hits})
        print("")
        print("FILE %s  decl_lines=%s" % (path, byfile[path]))
        if not labels:
            print("  EFFECTS none-detected")
        else:
            for label in labels:
                lines = sorted(h[1] for h in hits if h[0] == label)
                print("  EFFECT %-18s lines=%s" % (label, lines[:12]))


if __name__ == "__main__":
    main()
