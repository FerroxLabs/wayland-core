#!/usr/bin/env python3
"""Census of `fn is_concurrency_safe` declarations across the workspace.

Instrument notes (LANE-BRIEF 3b): rtk rewrites grep/wc/git output, so every
number this lane reports comes from this script, which uses only the Python
stdlib and writes to a file that is then read with the Read tool.

Self-test: run with --selftest. Three assertions, per LANE-BRIEF 6b-ii:
  1. known-positive  -> a body of `true` classifies UNCOND_TRUE
  2. known-negative  -> a body of `false` classifies UNCOND_FALSE
  3. discrimination  -> a body with a conditional does NOT classify as either
"""

import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
DECL = re.compile(r"\bfn\s+is_concurrency_safe\s*\(")


def scan_files():
    out = []
    for dirpath, dirnames, filenames in os.walk(os.path.join(ROOT, "crates")):
        dirnames[:] = [d for d in dirnames if d not in (".git", "target")]
        for fn in filenames:
            if fn.endswith(".rs"):
                out.append(os.path.join(dirpath, fn))
    return sorted(out)


def body_after(text, open_paren_idx):
    """Return the brace-balanced body text of the fn whose '(' is at idx.

    Returns None for a bodiless declaration (trait method ending in ';').
    """
    # walk to the closing paren
    depth = 0
    i = open_paren_idx
    while i < len(text):
        if text[i] == "(":
            depth += 1
        elif text[i] == ")":
            depth -= 1
            if depth == 0:
                break
        i += 1
    # now find the next '{' or ';'
    j = i + 1
    while j < len(text) and text[j] not in "{;":
        j += 1
    if j >= len(text) or text[j] == ";":
        return None
    depth = 0
    k = j
    while k < len(text):
        if text[k] == "{":
            depth += 1
        elif text[k] == "}":
            depth -= 1
            if depth == 0:
                return text[j + 1 : k]
        k += 1
    return None


def strip_comments(body):
    body = re.sub(r"//[^\n]*", "", body)
    body = re.sub(r"/\*.*?\*/", "", body, flags=re.S)
    return body


def classify(body):
    if body is None:
        return "BODILESS"
    stripped = strip_comments(body).strip()
    if stripped == "true":
        return "UNCOND_TRUE"
    if stripped == "false":
        return "UNCOND_FALSE"
    return "OTHER"


def in_test_module(text, idx):
    """True if idx sits inside a `#[cfg(test)] mod ... { }` span."""
    for m in re.finditer(r"#\[cfg\(test\)\]\s*(?:pub\s+)?mod\s+\w+\s*\{", text):
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
        if start <= idx <= k:
            return True
    return False


def line_of(text, idx):
    return text.count("\n", 0, idx) + 1


def census():
    rows = []
    for path in scan_files():
        with open(path, "r", encoding="utf-8", errors="replace") as f:
            text = f.read()
        for m in DECL.finditer(text):
            op = text.index("(", m.start())
            body = body_after(text, op)
            kind = classify(body)
            rel = os.path.relpath(path, ROOT)
            rows.append(
                {
                    "file": rel,
                    "line": line_of(text, m.start()),
                    "kind": kind,
                    "in_test_mod": in_test_module(text, m.start()),
                    "src_path": "/src/" in rel.replace(os.sep, "/"),
                }
            )
    return rows


def selftest():
    pos = "fn is_concurrency_safe(&self, _i: &Value) -> bool {\n    // c\n    true\n}"
    neg = "fn is_concurrency_safe(&self, _i: &Value) -> bool {\n    false\n}"
    cond = (
        "fn is_concurrency_safe(&self, i: &Value) -> bool {\n"
        "    i.get(\"x\").is_some()\n}"
    )
    bodiless = "fn is_concurrency_safe(&self, _i: &Value) -> bool;"
    fails = []

    def cls(s):
        m = DECL.search(s)
        return classify(body_after(s, s.index("(", m.start())))

    if cls(pos) != "UNCOND_TRUE":
        fails.append("A1 known-positive: expected UNCOND_TRUE got %s" % cls(pos))
    if cls(neg) != "UNCOND_FALSE":
        fails.append("A2 known-negative: expected UNCOND_FALSE got %s" % cls(neg))
    if cls(cond) in ("UNCOND_TRUE", "UNCOND_FALSE"):
        fails.append("A3 discrimination: conditional body mis-bucketed as %s" % cls(cond))
    if cls(bodiless) != "BODILESS":
        fails.append("A4 bodiless: expected BODILESS got %s" % cls(bodiless))
    # A5: the NAIVE matcher (regex for `-> bool {\s*true`) would MISS the
    # comment-bearing positive. Proves this instrument does something the
    # obvious broken one does not.
    naive = re.compile(r"fn is_concurrency_safe[^{]*\{\s*true\s*\}")
    if naive.search(pos):
        fails.append("A5 old-broken-matcher control: naive matcher unexpectedly HIT")
    print("SELFTEST_ASSERTIONS=5")
    for f in fails:
        print("SELFTEST_FAIL: " + f)
    print("SELFTEST=%s" % ("PASS" if not fails else "FAIL"))
    return 0 if not fails else 1


def main():
    if "--selftest" in sys.argv:
        sys.exit(selftest())
    rows = census()
    print("ROOT=%s" % ROOT)
    print("TOTAL_DECLARATIONS=%d" % len(rows))
    buckets = {}
    for r in rows:
        key = "%s|%s" % (r["kind"], "test" if r["in_test_mod"] else "nontest")
        buckets[key] = buckets.get(key, 0) + 1
    for k in sorted(buckets):
        print("BUCKET %s = %d" % (k, buckets[k]))
    prod_true = [
        r for r in rows
        if r["kind"] == "UNCOND_TRUE" and not r["in_test_mod"] and r["src_path"]
    ]
    print("PRODUCTION_SRC_UNCOND_TRUE=%d" % len(prod_true))
    for r in sorted(prod_true, key=lambda r: (r["file"], r["line"])):
        print("  PROD_TRUE %s:%d" % (r["file"], r["line"]))
    nonsrc_true = [
        r for r in rows
        if r["kind"] == "UNCOND_TRUE" and not r["in_test_mod"] and not r["src_path"]
    ]
    print("NONSRC_UNCOND_TRUE=%d" % len(nonsrc_true))
    for r in sorted(nonsrc_true, key=lambda r: (r["file"], r["line"])):
        print("  NONSRC_TRUE %s:%d" % (r["file"], r["line"]))


if __name__ == "__main__":
    main()
