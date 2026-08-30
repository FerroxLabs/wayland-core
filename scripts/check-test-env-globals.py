#!/usr/bin/env python3
"""Fail when TEST code writes a process-global environment variable that
PRODUCTION code linked into the SAME test binary reads (issue #1134).

WHY THIS EXISTS
    `cargo nextest`, which CI runs, gives every test its own process, so a
    test-written env var can never contaminate a sibling and the entire class
    is INVISIBLE at `--retries 0`. Under plain `cargo test` -- what a developer
    runs, and what the two shared-process CI legs run -- one test binary is
    one process, and a global written by test A is read by test B's production
    path. That has already produced silent wrong answers in this repo.

WHAT IT CHECKS
    Per TEST BINARY, because a process global is shared by every test inside
    ONE binary and by nothing outside it. Three kinds are scanned:
      * `<crate>`             the lib test binary -- writes from `#[cfg(test)]`
                              code anywhere under `src/`.
      * `<crate>::<target>`   one integration binary per `tests/<target>.rs`.
                              An integration file is test code end to end, so
                              there is no `#[cfg(test)]` to look for: EVERY
                              write in it counts.
      * `<crate>::tests/<shared modules>`
                              files under `tests/` that are not targets
                              themselves (`tests/common/mod.rs` and friends).
                              They compile into whichever target declares
                              `mod`, so they are scanned as one pseudo-binary.
    Each binary's write set is checked against the env vars READ by PRODUCTION
    code in that crate or any workspace crate it depends on -- i.e. everything
    linked into that binary. A pair is a LIVE hazard when at least one of those
    writes sits in a test with no serialization at all AND the binary holds a
    sibling test that could observe it.

    Covering `tests/` is the whole point of the widening: the scanner walked
    `src/` only and the shared-process CI leg ran `--lib` only, so every
    integration test in the repository was unexamined by BOTH instruments at
    once. `crates/*/tests` is where most of this repo's env writing lives.

HOW A HELPER WRITE IS AUDITED
    A `set_var` inside a helper -- an RAII env guard, a `temp_state()`, a
    `PinnedRetryBudget::pin` -- used to be classified `helper` and excused,
    on the reasoning that proving a helper safe means proving EVERY caller
    serialized. That excuse covered the defect shape #1134 was OPENED about,
    and 153 sites with it: the lint fired only on a `set_var` written
    lexically inside a test fn, so moving the same write one call deep made it
    invisible while changing nothing about the hazard.

    Callers are now resolved, by attribution key:
      * a write inside `impl [Trait for] Type` is keyed on `Type`, not on the
        method name. That is what makes a guard tractable: `Drop::drop` and
        the constructor belong to ONE key, and `Type` is a specific CamelCase
        identifier rather than `drop`/`new`/`set`, whose bare names collide
        with std on every line of the repository.
      * a write inside a free fn is keyed on the fn name, and ONLY when that
        name is declared exactly once in the binary. Otherwise it is
        unattributable and reported, never failed -- a colliding name would
        otherwise convict the wrong caller.
    Every mention of the key inside the binary, outside the key's own defining
    block, is a call site; its enclosing fn is classified the same way, and a
    helper caller is followed one level further (bounded, cycle-guarded).
    A key with even one UNSERIALIZED-TEST caller is `UNSERIALIZED-HELPER` and
    FAILS exactly like a direct write.

    Pre-existing helper hazards are listed in
    `.config/env-global-helper-debt.txt`, keyed on `<file>.rs::<writing fn>`
    -- the SITE. A line is an exemption for ONE write, not for a (binary, var)
    class: a second helper writing the same var in the same binary is a new
    instance and fails. Direct writes are never exempt, an expired line fails,
    and a line matching nothing fails as stale.

    Under-attribution is the safe direction and is taken deliberately:
    receiver-style calls through a re-export, or a key whose name collides,
    end up in the reported residue rather than in the verdict.

WHAT IT DOES NOT CHECK
    A write whose callers are all production code is a PRODUCTION write, not
    this class, and is reported rather than failed -- `wcore-config`'s
    `.env` loader is the honest example. It is counted on every run so a test
    helper cannot hide in that bucket silently.

    A write in a binary that holds exactly ONE test is reported, not failed:
    with no sibling running in that process there is nothing to contaminate.
    That count is printed too, and it turns into a failure the moment somebody
    adds a second test to the file -- which is the correct direction to fail.

    `benches/` and `examples/` are out of scope: no CI leg executes them as
    tests, so a global written there cannot reach a sibling test.

    This is deliberately NOT the rule "every set_var must be #[serial]". That
    rule was measured against the four defects the shared-process CI leg found
    and would have flagged none of them, while declaring the binary compliant.

    python3 scripts/check-test-env-globals.py --self-test   # prove both directions
    python3 scripts/check-test-env-globals.py               # the gate
    python3 scripts/check-test-env-globals.py --shared-process-targets
                                             # what the CI integration leg runs
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
TESTATTR = re.compile(r"#\[(?:[A-Za-z0-9_]+::)*test(?:\s*\([^)]*\))?\s*\]")
# `impl [Generics] [Trait for] Type` -- group 1 is the TYPE, which is the
# attribution key for every write inside the block. Keying on the type rather
# than the method is what makes an RAII env guard resolvable: `pin` and `drop`
# are the same guard, and `Drop::drop` cannot be found by searching for `drop(`.
IMPL = re.compile(
    r"^[ \t]*impl(?:\s*<[^>]*>)?\s+"
    r"(?:[A-Za-z0-9_:<>, \'&]+?\s+for\s+)?([A-Za-z0-9_]+)",
    re.M,
)
# Deliberately BROADER than the hazard analysis below, and used only to SCOPE
# the shared-process CI leg. That leg's subject is process-global state of every
# kind -- the four defects the lib leg caught on the day it was added were an
# env var, a leaked panic hook, a static chown hook and a config written through
# an env var -- while the pair analysis can only reason about env vars. Scoping
# on env writes alone would hand the leg a blind spot of exactly the shape this
# scanner exists to close. Over-selection costs CI minutes; under-selection
# costs the signal.
GLOBAL_SHAPE = re.compile(
    r"set_var\s*\(|remove_var\s*\(|panic::set_hook\s*\(|panic::take_hook\s*\(|"
    r"^[ \t]*static\s+[A-Z_][A-Z0-9_]*\s*:",
    re.M,
)


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


def brace_span(t, start):
    """(open, close) offsets of the first balanced `{...}` at or after `start`."""
    i = t.find("{", start)
    if i < 0:
        return None
    d, j = 0, i
    while j < len(t):
        if t[j] == "{":
            d += 1
        elif t[j] == "}":
            d -= 1
            if d == 0:
                return (i, j)
        j += 1
    return None


def impl_spans(t):
    """[(open, close, type name)] for every `impl` block in the file."""
    out = []
    for m in IMPL.finditer(t):
        span = brace_span(t, m.end())
        if span:
            out.append((span[0], span[1], m.group(1)))
    return out


def classify_site(attrs, body):
    """The four kinds a write site can carry, from its enclosing fn alone."""
    if "serial" in attrs:
        return "serial-attr"
    if ".lock()" in body:
        return "lock-guarded"
    if TESTATTR.search(attrs):
        return "UNSERIALIZED-TEST"
    return "helper"


def enclosing(t, pos, fns=None):
    """(fn name, attribute lines above it, fn body) for the fn containing pos.

    `fns` is the file's `FN` matches, hoisted by the caller: a file with a
    hundred write sites would otherwise be re-scanned a hundred times, which is
    most of this script's runtime on the widened `tests/` corpus.
    """
    last = None
    for m in fns if fns is not None else FN.finditer(t):
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


def units(pkgs):
    """[(binary, crate, [(path, whole_file_is_test)])] -- one entry per test binary.

    `whole_file_is_test` is what distinguishes an integration target from the
    lib: under `tests/` there is no `#[cfg(test)]` to bound test code, the file
    IS test code.
    """
    out = []
    for c, info in sorted(pkgs.items()):
        srcfiles = []
        for dp, _, fs in os.walk(os.path.join(info["dir"], "src")):
            for f in fs:
                if f.endswith(".rs"):
                    srcfiles.append(os.path.join(dp, f))
        out.append((c, c, [(p, False) for p in sorted(srcfiles)]))

        tdir = os.path.join(info["dir"], "tests")
        if not os.path.isdir(tdir):
            continue
        roots, shared = [], []
        for dp, dirs, fs in os.walk(tdir):
            # A nested package under tests/ is a fixture crate compiled on its
            # own, not linked into any test binary here. Do not descend.
            if dp != tdir and os.path.isfile(os.path.join(dp, "Cargo.toml")):
                dirs[:] = []
                continue
            for f in fs:
                if not f.endswith(".rs"):
                    continue
                p = os.path.join(dp, f)
                rel = os.path.relpath(p, tdir)
                depth = rel.count(os.sep)
                if depth == 0:
                    roots.append((rel[:-3], p))
                elif depth == 1 and f == "main.rs":
                    roots.append((os.path.dirname(rel), p))
                else:
                    shared.append(p)
        for name, p in sorted(roots):
            out.append(("%s::%s" % (c, name), c, [(p, True)]))
        if shared:
            out.append(("%s::tests/<shared modules>" % c, c,
                        [(p, True) for p in sorted(shared)]))
    return out


def scan(pkgs):
    """-> (hazard pairs, write sites by (binary,var), kind counts, tests per binary)"""
    us = units(pkgs)
    raw = {}
    for _, _, files in us:
        for p, _ in files:
            if p not in raw:
                raw[p] = open(p, encoding="utf-8", errors="replace").read()

    consts = {}
    for t in raw.values():
        for n, v in CONST.findall(t):
            consts.setdefault(n, v)

    def resolve(tok):
        if tok.startswith('"'):
            return tok.strip('"') or None
        return consts.get(tok.split("::")[-1])

    stripped = {p: strip_noise(t) for p, t in raw.items()}
    fn_index = {p: list(FN.finditer(t)) for p, t in stripped.items()}
    impl_index = {p: impl_spans(t) for p, t in stripped.items()}

    writers, readers, sites = {}, {}, {}
    kinds, ntests, crate_of = {}, {}, {}
    pending = []          # helper writes awaiting caller resolution
    files_of = {}         # binary -> its (path, whole) list
    for binary, c, files in us:
        crate_of[binary] = c
        files_of[binary] = files
        ntests.setdefault(binary, 0)
        for p, whole in files:
            t = stripped[p]
            fns = fn_index[p]
            sp = [(0, len(t))] if whole else test_spans(t)
            ntests[binary] += sum(
                1 for m in TESTATTR.finditer(t)
                if any(a <= m.start() <= b for a, b in sp)
            )
            # EVERY write in a scanned file counts, not only the ones
            # lexically inside `#[cfg(test)]`. `PinnedRetryBudget::pin` -- the
            # helper #1134 opens with -- lives in an ungated `pub mod
            # test_utils`, and `src/bash/tests.rs` is a whole test module
            # whose `#[cfg(test)]` sits in its PARENT file. Both were invisible
            # to a span-limited scan; whether they are test code is decided
            # below by WHO CALLS THEM, which needs no naming convention.
            for m in WRITE.finditer(t):
                v = resolve(m.group(1))
                if not v:
                    continue
                fn, attrs, body = enclosing(t, m.start(), fns)
                kind = classify_site(attrs, body)
                in_test_span = any(a <= m.start() <= b for a, b in sp)
                if kind != "helper" and not in_test_span:
                    # A `#[test]` outside a test span cannot happen in `src/`;
                    # if it ever does, treat it as the test it declares itself
                    # to be rather than dropping it.
                    in_test_span = True
                if kind == "helper":
                    key = None
                    for a, b, ty in impl_index[p]:
                        if a <= m.start() <= b:
                            key = ty
                    pending.append((binary, v, p,
                                    t[: m.start()].count("\n") + 1, fn, key))
                    continue
                if not in_test_span:
                    continue
                writers.setdefault(binary, set()).add(v)
                kinds[kind] = kinds.get(kind, 0) + 1
                sites.setdefault((binary, v), []).append(
                    (p, t[: m.start()].count("\n") + 1, kind, fn, None)
                )
            # Only PRODUCTION code counts as a reader: `src/` outside
            # `#[cfg(test)]`. A read from a test file is the test observing its
            # own write, which is not the hazard.
            if whole:
                continue
            for m in READ.finditer(t):
                v = resolve(m.group(1))
                if v and not any(a <= m.start() <= b for a, b in sp):
                    readers.setdefault(c, set()).add(v)

    # ── helper caller resolution ────────────────────────────────────────
    #
    # A helper write is only as safe as its WEAKEST caller. Resolve the
    # attribution key (see the module docstring), walk every mention of it in
    # the same binary, and take the strongest kind found: one unserialized
    # test caller makes the write UNSERIALIZED-HELPER and fails the gate.
    fn_decls = {}
    for binary, files in files_of.items():
        counter = {}
        for path, _ in files:
            for m in fn_index[path]:
                counter[m.group(1)] = counter.get(m.group(1), 0) + 1
        fn_decls[binary] = counter

    def callers_of(binary, key, qualified):
        """[(fn name, kind)] for every call site of `key` in this binary."""
        # A type key must be USED, not merely mentioned: `Type::`, `Type {` or
        # `Type(`. The declaration keywords are excluded by lookbehind so
        # `struct Type`/`impl Drop for Type` are not read as call sites --
        # `enclosing()` would otherwise attribute them to whatever fn happens
        # to precede the declaration.
        rx = re.compile(
            r"(?<!struct )(?<!enum )(?<!impl )(?<!for )(?<!fn )\b"
            + re.escape(key)
            + (r"\s*(?:::|\{|\()" if qualified else r"\s*(?:::<[^>]*>)?\s*\("))
        out = []
        for path, whole in files_of[binary]:
            t = stripped[path]
            for m in rx.finditer(t):
                if qualified and any(a <= m.start() <= b and ty == key
                                     for a, b, ty in impl_index[path]):
                    continue          # inside the key's own impl block
                fn, attrs, body = enclosing(t, m.start(), fn_index[path])
                if fn is None or fn == key:
                    continue
                out.append((fn, classify_site(attrs, body)))
        return out

    unattributable = 0
    production_write = 0
    for binary, v, path, line, fn, key in pending:
        qualified = key is not None
        if key is None:
            if fn is None or fn_decls[binary].get(fn, 0) != 1:
                # A colliding or missing name convicts the wrong caller.
                # Report it; never fail on it.
                unattributable += 1
                kinds["unattributable-helper"] = (
                    kinds.get("unattributable-helper", 0) + 1)
                continue
            key = fn
        seen, frontier, verdict, reached = set(), [(key, qualified)], set(), False
        for _ in range(3):
            nxt = []
            for name, qual in frontier:
                if name in seen:
                    continue
                seen.add(name)
                for caller, kind in callers_of(binary, name, qual):
                    reached = True
                    if kind == "helper":
                        nxt.append((caller, False))
                    else:
                        verdict.add(kind)
            frontier = nxt
            if not frontier:
                break
        if "UNSERIALIZED-TEST" in verdict:
            kind = "UNSERIALIZED-HELPER"
        elif verdict:
            kind = "serialized-helper"
        else:
            # Nobody in this binary's test code calls it. Either production
            # code owns the write (`wcore-config`'s .env loader) or the call
            # spelling is one this scan does not resolve. Neither is a verdict.
            production_write += 1
            kinds["unreached-helper"] = kinds.get("unreached-helper", 0) + 1
            continue
        writers.setdefault(binary, set()).add(v)
        kinds[kind] = kinds.get(kind, 0) + 1
        via = sorted(verdict)
        sites.setdefault((binary, v), []).append((path, line, kind, fn, via))

    def closure(n):
        seen, st = set(), [n]
        while st:
            x = st.pop()
            if x in seen or x not in pkgs:
                continue
            seen.add(x)
            st.extend(pkgs[x]["deps"] & set(pkgs))
        return seen

    linked_cache = {}
    pairs = []
    for binary, wv in sorted(writers.items()):
        c = crate_of[binary]
        if c not in linked_cache:
            linked_cache[c] = closure(c)
        for v in sorted(wv):
            rc = sorted(x for x in linked_cache[c] if v in readers.get(x, ()))
            if rc:
                pairs.append((v, binary, rc))
    return pairs, sites, kinds, ntests


# ── self-test ────────────────────────────────────────────────────────────────

_PROD = 'pub fn f() -> String { std::env::var("SHARED_V").unwrap_or_default() }\n'
_TEST_HEAD = "#[cfg(test)]\nmod tests {\n"
# Every fixture carries a SIBLING test: a global written in a binary that runs
# one test has nothing to contaminate, and the gate holds that back on purpose
# (proven separately below).
_SIBLING = "    #[test]\n    fn sibling() { assert!(true); }\n"
_UNSERIAL = '    #[test]\n    fn t() { unsafe { std::env::set_var("SHARED_V", "x") }; }\n' + _SIBLING
_SERIAL = '    #[test]\n    #[serial_test::serial]\n    fn t() { unsafe { std::env::set_var("SHARED_V", "x") }; }\n' + _SIBLING
_COMMENT_ONLY = '    /// This test used to `set_var("SHARED_V", ...)` and no longer does.\n    #[test]\n    fn t() { assert!(true); }\n' + _SIBLING
# #1134 c3. The SAME write, moved one call deep into an RAII guard -- the shape
# that used to be classified `helper` and excused, and the shape the issue
# opens with (`PinnedRetryBudget::pin`). The hazard is identical; only the
# lexical position changed, so the gate must behave identically.
_GUARD = (
    "    struct EnvPin;\n"
    "    impl EnvPin {\n"
    "        fn new() -> Self { unsafe { std::env::set_var(\"SHARED_V\", \"x\") }; Self }\n"
    "    }\n"
)
_HELPER_UNSERIAL = (
    _GUARD + '    #[test]\n    fn t() { let _p = EnvPin::new(); }\n' + _SIBLING
)
_HELPER_SERIAL = (
    _GUARD
    + '    #[test]\n    #[serial_test::serial]\n    fn t() { let _p = EnvPin::new(); }\n'
    + _SIBLING
)
# The guard is defined but never constructed. Nothing in this binary can reach
# the write, so convicting it would be convicting an unreachable line -- the
# residue bucket, not the verdict.
_HELPER_UNCALLED = _GUARD + '    #[test]\n    fn t() { assert!(true); }\n' + _SIBLING
# A SECOND guard writing the SAME var in the SAME binary from a DIFFERENT site.
# This is the shape the debt file must not excuse for free: a line written for
# `EnvPin::new` says nothing about `EnvPin2::newer`.
_GUARD2 = (
    "    struct EnvPin2;\n"
    "    impl EnvPin2 {\n"
    "        fn newer() -> Self { unsafe { std::env::set_var(\"SHARED_V\", \"y\") }; Self }\n"
    "    }\n"
)
_HELPER_TWO_SITES = (
    _GUARD + _GUARD2
    + '    #[test]\n    fn t() { let _p = EnvPin::new(); }\n'
    + '    #[test]\n    fn u() { let _q = EnvPin2::newer(); }\n'
    + _SIBLING
)
_DEMO_SITE1 = "crates/demo/src/lib.rs::new"
_DEMO_SITE2 = "crates/demo/src/lib.rs::newer"
_DEMO_DIRECT = "crates/demo/src/lib.rs::t"


def _debt_line(site, expiry="2999-01-01"):
    return ("%s  demo  SHARED_V  %s  gh#1233  Stated debt, long enough to be a "
            "real reason." % (expiry, site))


def _debt_gate(root, lines, today="2026-01-01"):
    """-> (fired, stale_keys, parse_complaints) over a synthetic tree.

    Runs the REAL `partition` and the REAL parser, so the self-test grades the
    code the gate runs rather than a restatement of it.
    """
    pairs, sites, _, ntests = scan(discover(root))
    debt, complaints = parse_debt_lines(lines, today)
    live, _s, _e, used = partition(root, pairs, sites, ntests, debt)
    return bool(live), sorted(set(debt) - used), complaints


# An integration test file carries no `#[cfg(test)]`; the file IS the test
# code. Two tests, because a global written in a binary with no sibling has
# nothing to contaminate and is reported rather than failed.
_INT_UNSERIAL = (
    '#[test]\nfn a() { unsafe { std::env::set_var("SHARED_V", "x") }; }\n'
    "#[test]\nfn b() { assert!(demo::f().is_empty() || true); }\n"
)
_INT_SERIAL = (
    '#[test]\n#[serial_test::serial]\n'
    'fn a() { unsafe { std::env::set_var("SHARED_V", "x") }; }\n'
    "#[test]\nfn b() { assert!(demo::f().is_empty() || true); }\n"
)
_INT_SOLE = '#[test]\nfn a() { unsafe { std::env::set_var("SHARED_V", "x") }; }\n'
# tokio's attribute takes arguments; the classifier must still see a test.
_INT_TOKIO = (
    '#[tokio::test(flavor = "multi_thread")]\n'
    'async fn a() { unsafe { std::env::set_var("SHARED_V", "x") }; }\n'
    "#[test]\nfn b() { assert!(true); }\n"
)


def _tree(root, lib_body=None, int_body=None):
    d = os.path.join(root, "crates", "demo")
    os.makedirs(os.path.join(d, "src"))
    open(os.path.join(d, "Cargo.toml"), "w").write('[package]\nname = "demo"\n')
    open(os.path.join(d, "src", "lib.rs"), "w").write(
        _PROD + (_TEST_HEAD + lib_body + "}\n" if lib_body else "")
    )
    if int_body is not None:
        os.makedirs(os.path.join(d, "tests"))
        open(os.path.join(d, "tests", "it.rs"), "w").write(int_body)
    return root


def _live(root):
    """Binaries the GATE would fail on -- an unserialized write with a sibling."""
    pairs, sites, _, ntests = scan(discover(root))
    return [
        (v, b)
        for v, b, _ in pairs
        if ntests.get(b, 0) >= 2
        and any(s[2] in ("UNSERIALIZED-TEST", "UNSERIALIZED-HELPER")
                for s in sites.get((b, v), []))
    ]


def _sole(root):
    """Findings held back only because their binary has no sibling test."""
    pairs, sites, _, ntests = scan(discover(root))
    return [
        (v, b)
        for v, b, _ in pairs
        if ntests.get(b, 0) < 2
        and any(s[2] == "UNSERIALIZED-TEST" for s in sites.get((b, v), []))
    ]


def self_test():
    # (label, src #[cfg(test)] body, tests/it.rs body, must the gate FIRE)
    cases = [
        ("src: unserialized writer + prod reader", _UNSERIAL, None, True),
        ("src: the same writer, serialized", _SERIAL, None, False),
        ("src: the write one call deep, caller unserialized", _HELPER_UNSERIAL, None, True),
        ("src: the write one call deep, caller serialized", _HELPER_SERIAL, None, False),
        ("src: a guard nothing ever constructs", _HELPER_UNCALLED, None, False),
        ("src: set_var quoted in a doc comment", _COMMENT_ONLY, None, False),
        ("tests/: unserialized writer + sibling", None, _INT_UNSERIAL, True),
        ("tests/: the same writer, serialized", None, _INT_SERIAL, False),
        ("tests/: #[tokio::test(args)] writer", None, _INT_TOKIO, True),
    ]
    ok = True
    for label, lib_body, int_body, must_fire in cases:
        with tempfile.TemporaryDirectory() as td:
            fired = bool(_live(_tree(td, lib_body, int_body)))
        good = fired == must_fire
        ok &= good
        print(
            "  %-42s expected %-6s got %-6s  %s"
            % (label, "FIRE" if must_fire else "quiet", "FIRE" if fired else "quiet",
               "ok" if good else "SELF-TEST FAILED")
        )

    # ── the debt file, proven in both directions ────────────────────────
    #
    # D2 is the case a verifier broke this gate on: the exemption used to be
    # keyed on (binary, var), so a brand-new helper writing an already-listed
    # var in an already-listed binary landed silently. D3 is its control --
    # the same tree, both sites listed, quiet -- so D2's fire is attributable
    # to the missing LINE and not to the two-guard shape.
    debt_cases = [
        ("debt: the listed site is quiet",
         _HELPER_UNSERIAL, [_debt_line(_DEMO_SITE1)], False, False),
        ("debt: a NEW site of a listed (binary,var) fires",
         _HELPER_TWO_SITES, [_debt_line(_DEMO_SITE1)], True, False),
        ("debt: both sites listed, quiet (control for the above)",
         _HELPER_TWO_SITES, [_debt_line(_DEMO_SITE1), _debt_line(_DEMO_SITE2)],
         False, False),
        ("debt: a line whose site is gone is STALE",
         _HELPER_UNSERIAL,
         [_debt_line(_DEMO_SITE1), _debt_line("crates/demo/src/lib.rs::gone")],
         False, True),
        ("debt: a DIRECT write is never excused",
         _UNSERIAL, [_debt_line(_DEMO_DIRECT)], True, True),
        ("debt: an expired line does not exempt",
         _HELPER_UNSERIAL, [_debt_line(_DEMO_SITE1, "2020-01-01")], True, False),
        ("debt: a line with no site column is refused",
         _HELPER_UNSERIAL,
         ["2999-01-01  demo  SHARED_V  gh#1233  no site column here"],
         True, False),
    ]
    for label, lib_body, lines, must_fire, must_stale in debt_cases:
        with tempfile.TemporaryDirectory() as td:
            fired, stale, complaints = _debt_gate(_tree(td, lib_body, None), lines)
        # A parse complaint fails the gate exactly as a live pair does, so the
        # observable is "would this run exit non-zero", not "was there a pair".
        reds = fired or bool(complaints)
        good = (reds == must_fire) and (bool(stale) == must_stale)
        ok &= good
        print("  %-42s expected %-6s got %-6s  %s"
              % (label, ("FIRE" if must_fire else "quiet")
                 + ("+stale" if must_stale else ""),
                 ("FIRE" if reds else "quiet") + ("+stale" if stale else ""),
                 "ok" if good else "SELF-TEST FAILED"))

    # The sole-test carve-out is the one place this gate deliberately does NOT
    # fail, so it is proven in BOTH directions too: held back with no sibling,
    # and failed the moment a sibling exists (the _INT_UNSERIAL case above).
    with tempfile.TemporaryDirectory() as td:
        r = _tree(td, None, _INT_SOLE)
        held = bool(_sole(r)) and not bool(_live(r))
    ok &= held
    print("  %-42s expected %-6s got %-6s  %s"
          % ("tests/: sole test, no sibling to contaminate", "held", "held" if held else "?",
             "ok" if held else "SELF-TEST FAILED"))
    print("self-test: %s" % ("both directions proven" if ok else "BROKEN"))
    return 0 if ok else 1


# ── gate ─────────────────────────────────────────────────────────────────────


# Tests the shared-process integration leg must NOT run, and why each one
# cannot be graded there. Keyed by (crate, target) -> {test name: reason}.
#
# This is the narrowest exclusion that exists: one test, not a target and not a
# crate, so the other 48 cases in the same binary still run in a shared process.
# Two rules keep it from rotting into a standing excuse, both enforced by
# `validate_skips` on every gate run: an entry with no real reason is a
# suppression and is REFUSED, and an entry naming a target the scan no longer
# selects is stale and is REFUSED too.
SHARED_PROCESS_SKIPS = {
    ("wcore-cli", "migrate_quarantine"): {
        "t19_live_negative_leg_quarantined_payload_does_not_execute": (
            "Not this leg's class, and measured rather than assumed. It drives "
            "the real CLI binary through a 45 s bounded agent turn and asserts "
            "the Skill tool answered inside that window. On hetzner-dsm at load "
            "~130 it is 1 pass / 6 fail under `cargo test` and 4 / 4 under "
            "nextest, and it fails the same way run ALONE in its own process -- "
            "so process sharing is not the variable and this leg cannot grade "
            "it. Left in the leg it would make a blocking gate intermittently "
            "red about something it does not measure, which is the same "
            "worthlessness as a gate that cannot fail. The test being unsafe "
            "under `cargo test` is a real finding and is reported as one; it is "
            "a bounded-window problem in that test, not a process global."
        ),
    },
}


def validate_skips(selected):
    """-> list of complaints. An unreasoned entry is a suppression; a stale one
    is an excuse for code that has already moved."""
    bad = []
    chosen = {(c, t) for c, t in selected}
    for key, tests in sorted(SHARED_PROCESS_SKIPS.items()):
        if key not in chosen:
            bad.append(
                "STALE skip: %s --test %s is no longer selected by the scan, so "
                "its entry excuses nothing. Delete it." % key
            )
        for name, reason in sorted(tests.items()):
            if len(reason.strip()) < 40:
                bad.append(
                    "UNREASONED skip: %s --test %s :: %s. An entry without a "
                    "stated reason is a suppression." % (key[0], key[1], name)
                )
    return bad


def shared_process_targets(pkgs):
    """`<crate> <target>` for every integration binary the shared-process CI leg
    has to run: it has at least two tests (one test in a process has no sibling
    to contaminate) and its test code touches process-global state.

    This is what scopes that leg. `cargo test --workspace --lib` covers the lib
    binaries; `--workspace --tests` would put all 698 integration binaries in a
    shared process, which is hours of wall clock for hundreds of targets that
    touch no global at all -- and a gate that never finishes is a gate that
    cannot fail. The list is DERIVED, so a new hazardous test file joins the leg
    by existing rather than by somebody remembering to add it.
    """
    out = []
    for binary, crate, files in units(pkgs):
        if "::" not in binary:
            continue
        target = binary.split("::", 1)[1]
        # The shared-module pseudo-binary is not a cargo target; its files
        # compile into the targets that declare `mod`, which are listed here in
        # their own right.
        if target.startswith("tests/"):
            continue
        texts = [
            strip_noise(open(f, encoding="utf-8", errors="replace").read())
            for f, _ in files
        ]
        if sum(len(TESTATTR.findall(t)) for t in texts) < 2:
            continue
        if not any(GLOBAL_SHAPE.search(t) for t in texts):
            continue
        out.append((crate, target))
    return out


def format_targets(selected):
    """`<crate> <target> [comma-separated tests to --skip]`, one per line."""
    lines = []
    for crate, target in selected:
        skip = ",".join(sorted(SHARED_PROCESS_SKIPS.get((crate, target), {})))
        lines.append("%s %s%s" % (crate, target, " " + skip if skip else ""))
    return lines


DEBT_FILE = ".config/env-global-helper-debt.txt"


def site_key(root, path, fn):
    """`<repo-relative file>::<writing fn>` -- the SITE a debt line binds to.

    Line numbers are deliberately NOT part of it: they move on every edit above
    the write, and a key that drifts turns unrelated edits into stale-entry
    failures. File plus writing fn is what a reader can find, and it is what
    makes an exemption an INSTANCE rather than a class.
    """
    return "%s::%s" % (os.path.relpath(path, root).replace(os.sep, "/"), fn)


def parse_debt_lines(lines, today):
    """-> ({(binary, var, site): reason}, [complaints]).

    Keyed on the SITE as well as the pair. A line written for one helper does
    not excuse a SECOND helper writing the same var in the same binary: that is
    a new instance of the class, and catching the next one is the whole job.
    """
    out, bad = {}, []
    for n, raw in enumerate(lines, 1):
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        parts = line.split(None, 5)
        if len(parts) < 6:
            bad.append("%s:%d is not `<expiry> <binary> <VAR> <file.rs::fn> "
                       "<gh#N> <reason>`: %r. A list nobody can parse is a "
                       "list that exempts everything." % (DEBT_FILE, n, line))
            continue
        expiry, binary, var, site, issue, reason = parts
        if not re.match(r"^\d{4}-\d{2}-\d{2}$", expiry):
            bad.append("%s:%d expiry %r is not YYYY-MM-DD."
                       % (DEBT_FILE, n, expiry))
            continue
        if not re.match(r"^[\w./-]+\.rs::\w+$", site):
            bad.append("%s:%d site %r is not `<path>.rs::<fn>`. A line naming "
                       "no site would exempt the whole (binary, var) class, "
                       "which is exactly what this column exists to stop."
                       % (DEBT_FILE, n, site))
            continue
        if expiry < today:
            bad.append("%s:%d expired on %s and was not renewed: %s %s at %s "
                       "(%s). An expired entry fails exactly as an unlisted "
                       "site does."
                       % (DEBT_FILE, n, expiry, binary, var, site, issue))
            continue
        out[(binary, var, site)] = "%s expires %s -- %s" % (issue, expiry, reason)
    return out, bad


def load_debt(root, today):
    """`parse_debt_lines` over the on-disk file.

    Absence of the file is not silence: the gate says so and exempts nothing.
    """
    path = os.path.join(root, DEBT_FILE)
    if not os.path.isfile(path):
        return {}, ["%s is missing; no site is exempt." % DEBT_FILE]
    return parse_debt_lines(open(path, encoding="utf-8"), today)


def partition(root, pairs, sites, ntests, debt):
    """-> (live, sole, excused, used_debt).

    `live` fails the gate. A write is excused only when the debt file names
    THAT SITE: an unlisted site inside an already-listed (binary, var) is a new
    instance and fails, and a DIRECT write is never excused whatever the file
    says. A pair with even one uncovered site fails, and only the uncovered
    sites are reported -- the covered ones are already stated debt.
    """
    live, sole, excused, used = [], [], [], set()
    for v, c, rc in pairs:
        bad = [s for s in sites.get((c, v), [])
               if s[2] in ("UNSERIALIZED-TEST", "UNSERIALIZED-HELPER")]
        if not bad:
            continue
        covered, uncovered = [], []
        for site in bad:
            key = (c, v, site_key(root, site[0], site[3]))
            if site[2] == "UNSERIALIZED-HELPER" and key in debt:
                used.add(key)
                covered.append(site)
            else:
                uncovered.append(site)
        if not uncovered:
            excused.append((v, c, rc, covered))
            continue
        (live if ntests.get(c, 0) >= 2 else sole).append((v, c, rc, uncovered))
    return live, sole, excused, used


def main():
    root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    pkgs = discover(root)
    pairs, sites, kinds, ntests = scan(pkgs)

    selected = shared_process_targets(pkgs)
    skip_complaints = validate_skips(selected)

    if "--shared-process-targets" in sys.argv:
        # Refuse to emit a list built on a rotten exclusion: the CI leg reads
        # this and would otherwise silently run less than it claims.
        if skip_complaints:
            for c in skip_complaints:
                print(c, file=sys.stderr)
            return 2
        for line in format_targets(selected):
            print(line)
        return 0

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

    # Second control, for the widening itself. The scanner walked `src/` only
    # for its first eight months and the shared-process CI leg ran `--lib`
    # only, so `crates/*/tests` was unexamined by BOTH instruments at once. A
    # walk that silently stopped reaching `tests/` would restore that blind
    # spot while still printing a clean result, so the presence of integration
    # binaries is asserted rather than assumed.
    integration = sorted(b for b in ntests if "::" in b)
    if not integration:
        print("CONTROL FAILED: the walk found no integration test binary under "
              "any crate's tests/ directory. It reached src/ only, which is the "
              "exact blind spot this scanner was widened to close.")
        return 2

    debt, debt_complaints = load_debt(
        root, __import__("datetime").date.today().isoformat())
    live, sole, excused, used_debt = partition(root, pairs, sites, ntests, debt)

    for key in sorted(set(debt) - used_debt):
        debt_complaints.append(
            "%s lists %s %s at %s, which no longer matches a hazard the scan "
            "reports. A line that has stopped applying would excuse the next "
            "write at that site for free -- delete it."
            % (DEBT_FILE, key[0], key[1], key[2]))

    print("control ok: %s found in %s (production readers: %s)"
          % (control[0][0], control[0][1], ",".join(control[0][2])))
    print("control ok: %d integration test binaries reached under crates/*/tests"
          % len(integration))
    print("(var, test-binary) pairs where test code writes what production reads: %d"
          % len(pairs))
    print("  of those, integration binaries: %d"
          % len([p for p in pairs if "::" in p[1]]))
    print("write sites by kind: %s" % kinds)
    print("helper writes AUDITED by caller (#1134 c3): %d reached only "
          "serialized callers, %d reached an unserialized test and FAIL below."
          % (kinds.get("serialized-helper", 0),
             kinds.get("UNSERIALIZED-HELPER", 0)))
    print("NOT audited by this gate: %d write(s) whose attribution key is a fn "
          "name declared more than once in the binary (a colliding name would "
          "convict the wrong caller), and %d whose callers are production code "
          "or a call spelling this scan does not resolve."
          % (kinds.get("unattributable-helper", 0),
             kinds.get("unreached-helper", 0)))
    print("shared-process integration leg: %d target(s) selected, %d test(s) "
          "excluded from it with a stated reason"
          % (len(selected), sum(len(v) for v in SHARED_PROCESS_SKIPS.values())))
    for (c, t), tests in sorted(SHARED_PROCESS_SKIPS.items()):
        for name in sorted(tests):
            print("      excluded: %s --test %s :: %s" % (c, t, name))
    if excused:
        print("\nDEBT, reported not failed: %d helper-attributed SITE(s) listed "
              "in %s. Each is a real hazard under the shared-process leg; the "
              "list is dated, is keyed on the site rather than on the (binary, "
              "var) class, and is designed to shrink."
              % (len({(x[1], x[0], site_key(root, y[0], y[3]))
                       for x in excused for y in x[3]}), DEBT_FILE))
        shown = set()
        for v, c, _, covered in excused:
            for p, _line, _kind, fn, _via in covered:
                key = (c, v, site_key(root, p, fn))
                if key in shown:   # one site, several writes inside the same fn
                    continue
                shown.add(key)
                print("      %s  %s  %s  (%s)" % (v, c, key[2], debt[key]))

    if skip_complaints or debt_complaints:
        print()
        for c in skip_complaints + debt_complaints:
            print("FAIL: %s" % c)
        return 1

    if sole:
        print("\nREPORTED, not failed: %d pair(s) whose only unserialized writer "
              "sits in a binary with no sibling test -- nothing else runs in that "
              "process to contaminate. Adding a second test to one of these files "
              "turns it into a failure, which is the correct direction." % len(sole))
        for v, c, _, bad in sole:
            for p, line, kind, fn, via in bad:
                print("      %s  %s:%d  fn %s%s"
                      % (v, os.path.relpath(p, root), line, fn,
                         "" if via is None else "  [helper, reached from %s]"
                         % ",".join(via)))

    if not live:
        print("\nOK: no unserialized test writes a global that its own binary's "
              "production code reads.")
        return 0

    print("\nFAIL: %d (var, test-binary) pair(s) have an unserialized test writer.\n"
          % len(live))
    for v, c, rc, bad in live:
        print("  %s  written by test binary %s (%d tests in that process)"
              % (v, c, ntests.get(c, 0)))
        print("      production readers in that binary: %s" % ",".join(rc))
        for p, line, kind, fn, via in bad:
            print("      %s:%d  fn %s%s"
                  % (os.path.relpath(p, root), line, fn,
                     "" if via is None else
                     "  [helper write, reached from an unserialized test; "
                     "caller kinds seen: %s]" % ",".join(via)))
    print("\nFix by STATING the value instead of writing the process global -- pass it "
          "as an argument to the code under test, or use the crate's own per-thread "
          "override where one exists. Serializing the writer is sufficient ONLY when "
          "every test in the same binary that reaches the same global is serialized "
          "too: serial_test orders only the tests that carry the attribute.")
    print("A site that is genuinely pre-existing debt goes in %s keyed on "
          "`<file>.rs::<writing fn>` -- the SITE, never the (binary, var) pair, "
          "so that listing one helper cannot silently excuse the next one."
          % DEBT_FILE)
    return 1


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        sys.exit(self_test())
    sys.exit(main())
