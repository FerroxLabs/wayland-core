#!/usr/bin/env python3
"""Gate for the macOS admission-control condition in .github/workflows/ci.yml.

This gate MUST be able to fail. It is proven to by three mutation arms in
`--self-test`: each mutates the real workflow text in memory and asserts the
gate reports a failure it would otherwise miss.

It does NOT hardcode the condition. It EXTRACTS the condition text from the
workflow and evaluates it, so a change to the workflow changes the gate's
answer — otherwise the gate would be a tautology over its own constants.

Usage:
    python3 gate.py [path/to/ci.yml]        # gate the real file
    python3 gate.py --self-test [path]      # prove the gate can fail
"""
import json
import re
import sys

COND_RE = re.compile(
    r"\$\{\{ fromJSON\(\((?P<cond>.+?)\) && '(?P<full>.+?)' \|\| '(?P<nomac>.+?)'\) \}\}"
)

# The `ci` job's os matrix nests darwin admission inside a windows-admission
# expression. Its DARWIN condition is the same clause-group; only the shape
# around it differs. Matching it here is what keeps this gate covering the
# matrix that actually schedules the macOS test leg.
NESTED_RE = re.compile(
    r"os: \$\{\{ fromJSON\(\(\((?P<cond>.+?)\) && \(.+?\)\) && '(?P<full>.+?)' "
    r"\|\| \(\(.+?\) && '.+?' \|\| '(?P<nomac>.+?)'\)\) \}\}"
)
REPORT_RE = re.compile(r"DARWIN: \$\{\{ \((?P<cond>.+?)\) && 'true' \|\| 'false' \}\}")


def extract(text):
    """Return (list_of_condition_strings, [(full,nomac), ...])."""
    conds, literals = [], []
    for m in COND_RE.finditer(text):
        conds.append(m.group("cond"))
        literals.append((m.group("full"), m.group("nomac")))
    for m in NESTED_RE.finditer(text):
        conds.append(m.group("cond"))
        literals.append((m.group("full"), m.group("nomac")))
    for m in REPORT_RE.finditer(text):
        conds.append(m.group("cond"))
    return conds, literals


def evaluate(cond, event_name, ref_name, head_msg, commit_msgs):
    """Faithful-enough evaluator for GHA expression semantics used here.

    `contains()` on strings in GitHub Actions is NOT case sensitive, so the
    substring test is lowercased on both sides.
    """
    ctx = {
        "github.event_name": event_name,
        "github.ref_name": ref_name,
    }
    haystack = json.dumps(head_msg if head_msg is not None else "") + json.dumps(
        commit_msgs if commit_msgs is not None else []
    )
    for clause in [c.strip() for c in cond.split("||")]:
        m = re.fullmatch(r"(\S+) == '([^']*)'", clause)
        if m:
            if ctx.get(m.group(1)) == m.group(2):
                return True
            continue
        m = re.fullmatch(r"contains\(format\(.+?\), '([^']*)'\)", clause)
        if m:
            if m.group(1).lower() in haystack.lower():
                return True
            continue
        m = re.fullmatch(r"startsWith\((\S+), '([^']*)'\)", clause)
        if m:
            if (ctx.get(m.group(1)) or "").startswith(m.group(2)):
                return True
            continue
        raise AssertionError("gate cannot parse clause: %r" % clause)
    return False


# name, event, ref, head_msg, commit_msgs, expected_darwin
CASES = [
    ("lane push, no token",        "push", "lane/x", "docs: notes",       ["docs: notes"],                 False),
    ("lane push, [ci-darwin]",     "push", "lane/x", "[ci-darwin] need",  ["[ci-darwin] need"],            True),
    ("lane push, [ci-macos] alias","push", "lane/x", "feat [ci-macos]",   ["feat [ci-macos]"],             True),
    ("lane push, token NOT on tip","push", "lane/x", "fixup",             ["[ci-darwin] earlier", "fixup"], True),
    ("lane push, UPPERCASE token", "push", "lane/x", "[CI-DARWIN] x",     ["[CI-DARWIN] x"],               True),
    ("lane push, hostile message", "push", "lane/x", '"; rm -rf /; #',    ['"; rm -rf /; #'],              False),
    ("integration branch push",    "push", "plan/f20-unified-audit-repair", "merge", ["merge"],            True),
    ("main push",                  "push", "main",   "release",           ["release"],                     True),
    ("integ branch push",          "push", "integ/f13", "merge lane",     ["merge lane"],                  True),
    ("integ, any suffix",          "push", "integ/x",   "wip",            ["wip"],                         True),
    ("NEG: integration-notes",     "push", "integration-notes", "x",      ["x"],                           False),
    ("NEG: integ, no slash",       "push", "integ",     "x",              ["x"],                           False),
    ("pull_request",               "pull_request", "main", None, None,                                     True),
]


def gate(text, verbose=True):
    fails = []
    conds, literals = extract(text)

    if len(conds) != 3:
        fails.append("expected 3 copies of the condition (2 matrices + 1 report step), found %d" % len(conds))
    elif len(set(conds)) != 1:
        fails.append("the %d condition copies are NOT byte-identical -> drift" % len(conds))
    if not conds:
        print("GATE_FAILURES=%d" % len(fails))
        for f in fails:
            print("  FAIL", f)
        return fails

    if len(literals) != 2:
        fails.append("expected 2 matrix expressions, found %d" % len(literals))
    for full, nomac in literals:
        f = json.loads(full)
        n = json.loads(nomac)
        fl = f["include"] if isinstance(f, dict) else f
        nl = n["include"] if isinstance(n, dict) else n
        if len(nl) < 1:
            fails.append("narrowed matrix vector is EMPTY -> hard workflow error")
        if len(fl) <= len(nl):
            fails.append("full matrix (%d) is not larger than narrowed (%d)" % (len(fl), len(nl)))
        if "macos-latest" not in json.dumps(fl):
            fails.append("full matrix lost macos-latest")
        if "macos-latest" in json.dumps(nl):
            fails.append("narrowed matrix still contains macos-latest")
    # both darwin BUILD targets must survive in the full literal (never drop the
    # arm64 leg: it is the only job that uploads the arm64 binary)
    for full, _ in literals:
        if "apple-darwin" in full:
            for t in ("x86_64-apple-darwin", "aarch64-apple-darwin"):
                if t not in full:
                    fails.append("full build matrix lost %s" % t)

    cond = conds[0]
    for name, ev, ref, hm, cm, exp in CASES:
        got = evaluate(cond, ev, ref, hm, cm)
        if got != exp:
            fails.append("case %r: darwin=%s expected=%s" % (name, got, exp))
        elif verbose:
            print("  ok   %-30s darwin=%s" % (name, got))

    print("GATE_FAILURES=%d" % len(fails))
    for f in fails:
        print("  FAIL", f)
    return fails


def self_test(text):
    """Prove the gate discriminates. Each arm MUST produce failures."""
    print("=== ARM 0: unmutated workflow (MUST pass) ===")
    base = gate(text)
    assert not base, "gate failed on the unmutated workflow"

    arms = [
        ("drop the integration-branch clause",
         lambda t: t.replace(" || github.ref_name == 'plan/f20-unified-audit-repair'", "")),
        ("break the opt-in token literal",
         lambda t: t.replace("'[ci-darwin]'", "'[ci-NOPE]'")),
        ("drift one condition copy (report step only)",
         lambda t: REPORT_RE.sub(
             lambda m: m.group(0).replace("github.event_name == 'pull_request' || ", ""), t, count=1)),
        ("drop aarch64-apple-darwin from the full build matrix",
         lambda t: t.replace(',{"os":"macos-latest","target":"aarch64-apple-darwin"}', "")),
    ]
    ok = True
    for label, mutate in arms:
        mutated = mutate(text)
        assert mutated != text, "mutation %r did not change the file" % label
        print("\n=== ARM: %s (MUST fail) ===" % label)
        f = gate(mutated, verbose=False)
        if not f:
            print("  !! GATE DID NOT DETECT THIS MUTATION — gate is self-passing")
            ok = False
    print("\nSELF_TEST=%s" % ("PASS" if ok else "FAIL"))
    return ok


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if a != "--self-test"]
    path = args[0] if args else ".github/workflows/ci.yml"
    text = open(path).read()
    if "--self-test" in sys.argv[1:]:
        sys.exit(0 if self_test(text) else 1)
    sys.exit(1 if gate(text) else 0)
