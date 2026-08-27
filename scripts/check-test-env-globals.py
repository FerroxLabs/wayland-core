#!/usr/bin/env python3
"""Fail when TEST code writes a process-global environment variable that
PRODUCTION code linked into the SAME lib test binary reads (issue #1134).

WHY THIS EXISTS
    `cargo nextest`, which CI runs, gives every test its own process, so a
    test-written env var can never contaminate a sibling and the entire class
    is INVISIBLE at `--retries 0`. Under plain `cargo test` -- what a developer
    runs, and what the shared-process CI leg runs -- one lib binary is one
    process, and a global written by test A is read by test B's production
    path. That has already produced silent wrong answers in this repo.

WHAT IT CHECKS
    For each crate, the set of env vars written from `#[cfg(test)]` code in
    `src/`, against the set of env vars READ by production code in that crate
    or any workspace crate it depends on -- i.e. everything linked into its lib
    test binary. A pair is a LIVE hazard when at least one of those writes sits
    in a test with no serialization at all.

WHAT IT DOES NOT CHECK
    Writes inside helper functions are reported but not failed: proving a
    helper safe means proving EVERY caller serialized, which this text scan
    cannot do. That residue is printed on every run so it cannot rot silently.

    This is deliberately NOT the rule "every set_var must be #[serial]". That
    rule was measured against the four defects the shared-process CI leg found
    and would have flagged none of them, while declaring the binary compliant.

    python3 scripts/check-test-env-globals.py --self-test   # prove both directions
    python3 scripts/check-test-env-globals.py               # the gate
"""
import os
import re
import sys
import tempfile

# ── source scanning ──────────────────────────────────────────────────────────


def strip_noise(t: str) -> str:
    """Blank //-comments, /*..*/ comments and non-identifier string bodies,
    preserving offsets and newlines.

    Without this the scanner counts `remove_var(TOKEN_ENV)` written inside a
    doc comment as a real write site -- which it did, on the very commit that
    removed the real one, and reported the fixed test as still defective.
    """
    out = list(t)
    i, n = 0, len(t)
    while i < n:
        c = t[i]
        if c == "/" and i + 1 < n and t[i + 1] == "/":
            while i < n and t[i] != "\n":
                out[i] = " "
                i += 1
        elif c == "/" and i + 1 < n and t[i + 1] == "*":
            d = 0
            while i < n:
                if t[i] == "/" and i + 1 < n and t[i + 1] == "*":
                    d += 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    continue
                if t[i] == "*" and i + 1 < n and t[i + 1] == "/":
                    d -= 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    if d == 0:
                        break
                    continue
                if t[i] != "\n":
                    out[i] = " "
                i += 1
        elif c == "r" and i + 1 < n and t[i + 1] in '"#':
            j = i + 1
            h = 0
            while j < n and t[j] == "#":
                h += 1
                j += 1
            if j < n and t[j] == '"':
                end = '"' + "#" * h
                k = t.find(end, j + 1)
                k = n if k < 0 else k
                for x in range(j + 1, min(k, n)):
                    if t[x] != "\n":
                        out[x] = " "
                i = k + len(end)
            else:
                i += 1
        elif c == '"':
            j = i + 1
            buf = []
            while j < n:
                if t[j] == "\\":
                    j += 2
                    continue
                if t[j] == '"':
                    break
                buf.append(t[j])
                j += 1
            if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", "".join(buf)):
                for x in range(i + 1, min(j, n)):
                    if t[x] != "\n":
                        out[x] = " "
            i = j + 1
        else:
            i += 1
    return "".join(out)


CFG_TEST = re.compile(r"#\[cfg\(test\)\]")
WRITE = re.compile(r"(?:set_var|remove_var)\s*\(\s*([A-Za-z0-9_:\"]+)")
READ = re.compile(r"env::(?:var|var_os)\s*\(\s*([A-Za-z0-9_:\"]+)")
CONST = re.compile(r"const\s+([A-Z][A-Z0-9_]*)\s*:\s*&(?:'static\s+)?str\s*=\s*\"([^\"]+)\"")
FN = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([A-Za-z0-9_]+)", re.M)
PKG_NAME = re.compile(r'^\s*name\s*=\s*"([^"]+)"', re.M)


def test_spans(t):
    out = []
    for m in CFG_TEST.finditer(t):
        i = t.find("{", m.end())
        if i < 0:
            continue
        d, j = 0, i
        while j < len(t):
            if t[j] == "{":
                d += 1
            elif t[j] == "}":
                d -= 1
                if d == 0:
                    break
            j += 1
        out.append((m.start(), j))
    return out


def enclosing(t, pos):
    """(fn name, attribute lines above it, fn body) for the fn containing pos."""
    last = None
    for m in FN.finditer(t):
        if m.start() > pos:
            break
        last = m
    if not last:
        return None, "", ""
    attrs = []
    for line in reversed(t[: last.start()].rstrip("\n").split("\n")):
        s = line.strip()
        if s.startswith("#[") or s.endswith("]") or s.endswith("],"):
            attrs.append(s)
        elif s == "":
            continue
        else:
            break
    i = t.find("{", last.end())
    d, j = 0, i
    while i >= 0 and j < len(t):
        if t[j] == "{":
            d += 1
        elif t[j] == "}":
            d -= 1
            if d == 0:
                break
        j += 1
    return last.group(1), "\n".join(attrs), t[last.start() : j if j > 0 else last.end()]


def discover(root):
    """{crate: {"dir":…, "deps":{…}}} from Cargo.toml text alone (no toolchain)."""
    pkgs = {}
    for base in ("crates", "plugins", "."):
        d = os.path.join(root, base)
        if not os.path.isdir(d):
            continue
        for entry in sorted(os.listdir(d)):
            man = os.path.join(d, entry, "Cargo.toml")
            if not os.path.isfile(man):
                continue
            text = open(man, encoding="utf-8", errors="replace").read()
            m = PKG_NAME.search(text)
            if not m:
                continue
            deps = set(re.findall(r"^\s*([A-Za-z0-9_-]+)\s*[=.]", text, re.M))
            pkgs[m.group(1)] = {"dir": os.path.join(d, entry), "deps": deps}
    return pkgs


def scan(pkgs):
    """-> (hazard pairs, write sites by (crate,var), kind counts)"""
    srcfiles, raw = {}, {}
    for c, info in pkgs.items():
        files = []
        for dp, _, fs in os.walk(os.path.join(info["dir"], "src")):
            for f in fs:
                if f.endswith(".rs"):
                    p = os.path.join(dp, f)
                    files.append(p)
                    raw[p] = open(p, encoding="utf-8", errors="replace").read()
        srcfiles[c] = files

    consts = {}
    for t in raw.values():
        for n, v in CONST.findall(t):
            consts.setdefault(n, v)

    def resolve(tok):
        if tok.startswith('"'):
            return tok.strip('"') or None
        return consts.get(tok.split("::")[-1])

    writers, readers, sites = {}, {}, {}
    kinds = {}
    for c, files in srcfiles.items():
        for p in files:
            t = strip_noise(raw[p])
            sp = test_spans(t)
            for m in WRITE.finditer(t):
                v = resolve(m.group(1))
                if not v or not any(a <= m.start() <= b for a, b in sp):
                    continue
                writers.setdefault(c, set()).add(v)
                fn, attrs, body = enclosing(t, m.start())
                if "serial" in attrs:
                    kind = "serial-attr"
                elif ".lock()" in body:
                    kind = "lock-guarded"
                elif "test]" in attrs:
                    kind = "UNSERIALIZED-TEST"
                else:
                    kind = "helper"
                kinds[kind] = kinds.get(kind, 0) + 1
                sites.setdefault((c, v), []).append((p, t[: m.start()].count("\n") + 1, kind, fn))
            for m in READ.finditer(t):
                v = resolve(m.group(1))
                if v and not any(a <= m.start() <= b for a, b in sp):
                    readers.setdefault(c, set()).add(v)

    def closure(n):
        seen, st = set(), [n]
        while st:
            x = st.pop()
            if x in seen or x not in pkgs:
                continue
            seen.add(x)
            st.extend(pkgs[x]["deps"] & set(pkgs))
        return seen

    pairs = []
    for c, wv in writers.items():
        linked = closure(c)
        for v in sorted(wv):
            rc = sorted(x for x in linked if v in readers.get(x, ()))
            if rc:
                pairs.append((v, c, rc))
    return pairs, sites, kinds


# ── self-test ────────────────────────────────────────────────────────────────

_PROD = 'pub fn f() -> String { std::env::var("SHARED_V").unwrap_or_default() }\n'
_TEST_HEAD = "#[cfg(test)]\nmod tests {\n"
_UNSERIAL = '    #[test]\n    fn t() { unsafe { std::env::set_var("SHARED_V", "x") }; }\n'
_SERIAL = '    #[test]\n    #[serial_test::serial]\n    fn t() { unsafe { std::env::set_var("SHARED_V", "x") }; }\n'
_COMMENT_ONLY = '    /// This test used to `set_var("SHARED_V", ...)` and no longer does.\n    #[test]\n    fn t() { assert!(true); }\n'


def _tree(root, body):
    d = os.path.join(root, "crates", "demo")
    os.makedirs(os.path.join(d, "src"))
    open(os.path.join(d, "Cargo.toml"), "w").write('[package]\nname = "demo"\n')
    open(os.path.join(d, "src", "lib.rs"), "w").write(_PROD + _TEST_HEAD + body + "}\n")
    return root


def _live(root):
    pairs, sites, _ = scan(discover(root))
    return [
        (v, c)
        for v, c, _ in pairs
        if any(s[2] == "UNSERIALIZED-TEST" for s in sites.get((c, v), []))
    ]


def self_test():
    cases = [
        ("unserialized writer + production reader", _UNSERIAL, True),
        ("the same writer, serialized", _SERIAL, False),
        ("set_var quoted in a doc comment only", _COMMENT_ONLY, False),
    ]
    ok = True
    for label, body, must_fire in cases:
        with tempfile.TemporaryDirectory() as td:
            fired = bool(_live(_tree(td, body)))
        good = fired == must_fire
        ok &= good
        print(
            "  %-42s expected %-6s got %-6s  %s"
            % (label, "FIRE" if must_fire else "quiet", "FIRE" if fired else "quiet",
               "ok" if good else "SELF-TEST FAILED")
        )
    print("self-test: %s" % ("both directions proven" if ok else "BROKEN"))
    return 0 if ok else 1


# ── gate ─────────────────────────────────────────────────────────────────────


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    pairs, sites, kinds = scan(discover(root))

    # A zero result is worthless without proof the query can see a real hit.
    # WAYLAND_MAX_STREAM_RETRIES is the var #1134 was opened about: test code in
    # wcore-agent writes it and wcore-agent production reads it on every engine
    # run. If that pair ever stops being FOUND, the scanner is broken, not the
    # tree clean.
    control = [p for p in pairs if p[0] == "WAYLAND_MAX_STREAM_RETRIES"]
    if not control:
        print("CONTROL FAILED: the known (var, binary) pair of #1134 was not found.")
        print("The scanner is broken; a clean result here means nothing.")
        return 2

    live = []
    for v, c, rc in pairs:
        bad = [s for s in sites.get((c, v), []) if s[2] == "UNSERIALIZED-TEST"]
        if bad:
            live.append((v, c, rc, bad))

    print("control ok: %s found in %s (production readers: %s)"
          % (control[0][0], control[0][1], ",".join(control[0][2])))
    print("(var, lib-binary) pairs where test code writes what production reads: %d" % len(pairs))
    print("write sites by kind: %s" % kinds)
    print("NOT audited by this gate: %d write(s) inside helper functions -- "
          "safe only if every caller is serialized." % kinds.get("helper", 0))

    if not live:
        print("\nOK: no unserialized test writes a global that its own binary's "
              "production code reads.")
        return 0

    print("\nFAIL: %d (var, lib-binary) pair(s) have an unserialized test writer.\n" % len(live))
    for v, c, rc, bad in live:
        print("  %s  written by lib binary %s" % (v, c))
        print("      production readers in that binary: %s" % ",".join(rc))
        for p, line, _, fn in bad:
            print("      %s:%d  fn %s" % (os.path.relpath(p, root), line, fn))
    print("\nFix by STATING the value instead of writing the process global -- pass it "
          "as an argument to the code under test. Serializing the writer is not "
          "sufficient: serial_test only orders tests that carry the attribute, so a "
          "serialized writer still races every unserialized reader.")
    return 1


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(main())
