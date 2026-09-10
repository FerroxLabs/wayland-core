#!/usr/bin/env python3
"""Fail when a FOURTH hand-cut URL authority parser is added (#1276).

WHY THIS GATE EXISTS
    `wayland#1211` -> `#1243` -> `#1252` is ONE bug class arriving three
    times. Each arrival was found by a hand sweep after the fact, and #1252's
    own Site C was missed by the sweep before it. #1252 c3 closed the three
    known sites by MEASUREMENT: it read every production line carrying the
    scheme separator and dispositioned each one. A measurement is a snapshot.
    Nothing in the workspace went red when a fourth cut was added, so the
    fourth would have been caught by whoever next thought to look.

    This is the inversion `wayland-core#402` established and `#1252`'s own
    body asked for: not "which cutting idioms exist" -- that enumeration
    cannot terminate -- but "which functions answer `which host does this URL
    reach?` without going through `url::Url`". The candidate set is declared
    syntactically and totally; every member is classified; the classification
    lives in a file the gate reads.

THE THREE ENUMERATIONS
    Two are total syntactic sets over production sources. The third is a
    known-positive control so a gate that finds nothing cannot pass.

    AXIS A -- SCHEME-SEPARATOR SITES. Every line under `crates/*/src/`
    carrying the literal `"://"`. This is the set #1252's own closing
    measurement used, and it reproduces that measurement's count exactly (24
    at `488fbbae9`, 24 here). It is a SUPERSET of every cutting idiom rather
    than a list of them, and notably catches `find("://")`, the spelling that
    escaped #1252's first sweep and produced Site C. Each line must be
    dispositioned in DISPOSITIONS by `<path>::<fn>` plus a fingerprint of the
    line itself.

    AXIS B -- HOST-SHAPED FUNCTION DECLARATIONS. Every `fn` under
    `crates/*/src/`, outside `#[cfg(test)]`, whose NAME carries a host or
    authority word AND whose RETURN TYPE -- after unwrapping `Result`/`Option`
    to the value position -- is string- or `Host`-shaped. This is the axis
    that answers c1 for a cut carrying no scheme literal at all: a
    `fn upstream_authority(&str) -> &str` built out of `find("//")` and a
    slice is seen here and nowhere else. A member is GREEN automatically when
    its body -- or the body of a uniquely-declared same-file helper it calls
    -- reaches the parser. Otherwise it must be dispositioned.

    AXIS C -- KNOWN POSITIVES. The three sites `#1252` fixed and the two
    `#1243` fixed, named. Each must still exist AND still reach the parser.
    They are named rather than derived because the FIX removed their scheme
    literals, so axes A and B can no longer see all five -- and a control the
    fix deletes is a control that cannot fail. If one disappears or stops
    answering through the parser, this gate fails whatever else it found.

ANTI-VACUITY, FAIL CLOSED (c2)
    Each axis declares a floor measured against this tree. If an axis stops
    matching -- a rename, a moved root, a regex that silently stops firing --
    the gate FAILS rather than reporting a clean sweep of nothing. This is the
    `sites >= N` shape `wayland-core#402` established, applied per axis.

WHAT THIS GATE DELIBERATELY DOES NOT DO
    It does not decide semantics. A cut whose function is named neither for a
    host nor for an authority AND which carries no `"://"` literal -- say
    `fn cut(raw: &str) -> String` -- is seen by neither axis. That is a REAL
    residual and is recorded rather than papered over: the alternative,
    enumerating every fn returning a `String`, convicts thousands of innocent
    functions and is the over-attribution that makes a gate get switched off.
    Under-attribution here is the deliberate direction.

    It also does not follow calls more than one level, for the reason
    `check-test-env-globals.py` documents at length and this gate re-learned:
    a bare name followed further convicts whatever else in the tree happens to
    share it. Helper following is restricted to names declared exactly once in
    the same file.
"""

import hashlib
import re
import sys
from pathlib import Path

DISPOSITIONS = ".config/url-authority-dispositions.txt"

# The literal #1252's closing measurement grepped for, including its quotes.
SCHEME_LITERAL = '"://"'

# A name carrying a host or authority word, matched on whole `_`-separated
# words so `authorize`, `hosted` and `subdomain_of` are not swept in.
HOST_WORD = re.compile(r"(?i)(^|_)(hosts?|hostname|hostnames|authority|authorities|domains?)($|_)")

# Reaching the one parser. Matched against COMMENT-STRIPPED bodies only: a
# comment that merely mentions `url::Url::host_str()` is not a parser call, and
# `blocked_host_reason` in this very tree carries exactly that comment.
PARSER_MARKERS = (
    "url_authority::",
    "dialed_host(",
    "dialed_host_str(",
    "dialed_scheme_host(",
    "publishable_endpoint(",
    "Url::parse(",
    ".host_str()",
    ".host()",
)

# The three sites #1252 fixed and the two #1243 fixed. c4's known-positive
# control: `<path>::<fn>`. Each must exist and must reach the parser.
KNOWN_POSITIVES = (
    ("crates/wcore-cli/src/doctor/mod.rs", "base_url_caveat", "wayland#1252 Site A"),
    ("crates/wcore-browser/src/policy.rs", "strip_pattern_decorations", "wayland#1252 Site B"),
    ("crates/wcore-config/src/portability/redact.rs", "strip_url_userinfo", "wayland#1252 Site C"),
    ("crates/wcore-cli/src/tui/permission/components/webfetch.rs", "host_of", "wayland#1243 Site A"),
    ("crates/wcore-protocol/src/events.rs", "is_local_endpoint", "wayland#1243 Site B"),
)

# Floors, measured on this tree (axis A 24, axis B 28). Set below the measured
# value so ordinary deletion does not red the gate, and far enough above zero
# that an enumeration which has stopped matching cannot report a clean sweep.
MIN_SCHEME_SITES = 18
MIN_HOST_SHAPED = 22

CLASSES = {
    # c3's three, plus one the enumeration forced. A class is a claim about
    # HOW the value is obtained, never about whether the code looks fine.
    "parser": "answers through url::Url / wcore_types::url_authority",
    "no-host": "derives no host from a URL string at this site",
    "display": "cuts a host, but only to render it -- decides nothing",
    "test-only": "#[cfg(test)] code; ships in no binary an operator runs",
}

FN_RE = re.compile(
    r"^[ \t]*(?:pub(?:\([^)]*\))?\s+)?(?:default\s+)?(?:const\s+)?(?:async\s+)?"
    r"(?:unsafe\s+)?(?:extern\s+\"[^\"]*\"\s+)?fn\s+([A-Za-z_][A-Za-z0-9_]*)"
    r"\s*(?:<[^>]*>)?\s*\(",
    re.M,
)


# ── source shredding ────────────────────────────────────────────────────────

def strip_comments(text):
    """Blank out `//` and `/* */` comments, preserving offsets and newlines."""
    out = list(text)
    i, n = 0, len(text)
    while i < n:
        ch = text[i]
        if ch == '"':
            i += 1
            while i < n:
                if text[i] == "\\":
                    i += 2
                    continue
                if text[i] == '"':
                    break
                i += 1
            i += 1
            continue
        if text.startswith("//", i):
            while i < n and text[i] != "\n":
                out[i] = " "
                i += 1
            continue
        if text.startswith("/*", i):
            depth = 0
            while i < n:
                if text.startswith("/*", i):
                    depth += 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    continue
                if text.startswith("*/", i):
                    depth -= 1
                    out[i] = out[i + 1] = " "
                    i += 2
                    if depth == 0:
                        break
                    continue
                if text[i] != "\n":
                    out[i] = " "
                i += 1
            continue
        i += 1
    return "".join(out)


def brace_block(text, start):
    """(body_start, body_end) of the `{...}` opening at or after `start`."""
    brace = text.find("{", start)
    if brace < 0:
        return None
    depth, index = 0, brace
    while index < len(text):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return brace, index + 1
        index += 1
    return None


def cfg_test_spans(text):
    spans = []
    for match in re.finditer(r"#\[cfg\(test\)\]", text):
        block = brace_block(text, match.end())
        if block:
            spans.append((match.start(), block[1]))
    return spans


def functions(text):
    """[(name, decl_offset, body_start, body_end)] for every fn with a body."""
    out = []
    for match in FN_RE.finditer(text):
        semi = text.find(";", match.end())
        block = brace_block(text, match.end())
        if not block:
            continue
        if 0 <= semi < block[0]:
            continue           # trait method declaration, no body
        out.append((match.group(1), match.start(), block[0], block[1]))
    return out


def enclosing_fn(fns, offset):
    """Innermost fn whose body contains `offset`, or None for module scope."""
    best = None
    for name, _, start, end in fns:
        if start <= offset < end and (best is None or start > best[1]):
            best = (name, start)
    return best[0] if best else None


def fingerprint(text):
    return hashlib.sha256(" ".join(text.split()).encode("utf-8")).hexdigest()[:12]


# ── return-type shape ───────────────────────────────────────────────────────

def split_args(text):
    out, depth, cur = [], 0, ""
    for ch in text:
        if ch in "<([":
            depth += 1
        elif ch in ">)]":
            depth -= 1
        if ch == "," and depth == 0:
            out.append(cur)
            cur = ""
        else:
            cur += ch
    out.append(cur)
    return [x.strip() for x in out if x.strip()]


VALUE_SHAPE = re.compile(
    r"^(?:&(?:'[A-Za-z_][A-Za-z0-9_]*\s+)?)?(?:mut\s+)?(?:\w+::)*"
    r"(?:String|str|Cow\s*<.*>|Host\s*<.*>|Host)$"
)


def host_shaped_return(rendered):
    """True when the VALUE position of a return type is host-shaped.

    `Result<(), String>` is not: the `String` is the error. Reading the error
    position as a value is how a first draft of this gate enumerated every
    `host_register_*` plugin trait method in the workspace -- 42 innocent
    functions whose `host` means the host APPLICATION, not a network host.
    """
    text = rendered.strip()
    for _ in range(6):
        match = re.match(r"^(?:\w+::)*(?:Result|Option)\s*<(.*)>$", text, re.S)
        if not match:
            break
        parts = split_args(match.group(1))
        if not parts:
            return False
        text = parts[0].strip()
    if text.startswith("(") and text.endswith(")"):
        parts = split_args(text[1:-1])
        return bool(parts) and all(host_shaped_return(p) for p in parts)
    return bool(VALUE_SHAPE.match(text))


def return_type(text, decl_end, body_start):
    signature = text[decl_end:body_start]
    arrow = signature.find("->")
    if arrow < 0:
        return None
    return " ".join(signature[arrow + 2:].split())


# ── enumeration ─────────────────────────────────────────────────────────────

def production_sources(root):
    for crate in sorted((root / "crates").iterdir()):
        src = crate / "src"
        if not src.is_dir():
            continue
        for path in sorted(src.rglob("*.rs")):
            # `src/**/tests.rs` is a whole test module whose `#[cfg(test)]`
            # sits in its parent file. #1252's measurement excluded it and so
            # does this, so the two counts are comparable.
            if path.name == "tests.rs":
                continue
            yield path


def enumerate_sites(root):
    """(scheme_sites, host_shaped_sites, parser_reach) over production code."""
    scheme_sites, host_sites, parser_reach = [], [], {}
    for path in production_sources(root):
        raw = path.read_text(encoding="utf-8", errors="replace")
        rel = path.relative_to(root).as_posix()
        code = strip_comments(raw)
        fns = functions(raw)
        code_fns = functions(code)

        declared = {}
        for name, _, _, _ in code_fns:
            declared[name] = declared.get(name, 0) + 1
        unique = {n for n, c in declared.items() if c == 1}
        bodies = {name: code[start:end] for name, _, start, end in code_fns
                  if name in unique}
        reaches = {name for name, body in bodies.items()
                   if any(marker in body for marker in PARSER_MARKERS)}

        # AXIS A: the raw line, comments included. #1252's measurement grepped
        # raw source, and a doc comment describing a cut is a site a reader
        # will be told about; it is dispositioned, not silently dropped.
        offset = 0
        for line in raw.splitlines(keepends=True):
            if SCHEME_LITERAL in line:
                scheme_sites.append({
                    "axis": "A",
                    "path": rel,
                    "fn": enclosing_fn(fns, offset) or "<module>",
                    "line": raw.count("\n", 0, offset) + 1,
                    "fp": fingerprint(line),
                    "text": line.strip(),
                })
            offset += len(line)

        # AXIS B: host-shaped fn declarations, outside `#[cfg(test)]`.
        spans = cfg_test_spans(raw)
        for match in FN_RE.finditer(raw):
            name = match.group(1)
            if not HOST_WORD.search(name):
                continue
            if any(a <= match.start() <= b for a, b in spans):
                continue
            block = brace_block(raw, match.end())
            semi = raw.find(";", match.end())
            if not block or (0 <= semi < block[0]):
                continue
            rendered = return_type(raw, match.end(), block[0])
            if rendered is None or not host_shaped_return(rendered):
                continue
            body = code[block[0]:block[1]]
            direct = any(marker in body for marker in PARSER_MARKERS)
            via = sorted(
                other for other in reaches
                if other != name
                and re.search(rf"(?<![:.\w]){re.escape(other)}\s*\(", body)
            )
            host_sites.append({
                "axis": "B",
                "path": rel,
                "fn": name,
                "line": raw.count("\n", 0, match.start()) + 1,
                "fp": fingerprint(body),
                "ret": rendered,
                "parser": direct or bool(via),
                "via": via,
            })
        parser_reach[rel] = (reaches, {n for n, _, _, _ in code_fns})
    return scheme_sites, host_sites, parser_reach


# ── dispositions ────────────────────────────────────────────────────────────

def load_dispositions(path):
    """{(path, fn, fp): (klass, reason)} plus the malformed lines found."""
    entries, bad = {}, []
    if not path.exists():
        return entries, ["%s is missing; every enumerated site is undispositioned"
                         % path]
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        parts = stripped.split(None, 3)
        if len(parts) < 4:
            bad.append("%s:%d: unparseable -- want `<class>  <path>::<fn>  "
                       "<fingerprint>  <reason>`" % (path, number))
            continue
        klass, site, fp, reason = parts
        if klass not in CLASSES:
            bad.append("%s:%d: unknown class `%s`; want one of %s"
                       % (path, number, klass, ", ".join(sorted(CLASSES))))
            continue
        if "::" not in site:
            bad.append("%s:%d: `%s` names no function; a line without a site "
                       "is not an exemption" % (path, number, site))
            continue
        if len(reason.strip()) < 25:
            bad.append("%s:%d: the reason is too short to be a reason. An "
                       "entry without one is a suppression." % (path, number))
            continue
        file_part, fn_part = site.rsplit("::", 1)
        entries[(file_part, fn_part, fp)] = (klass, reason.strip())
    return entries, bad


def render_line(site, klass="TODO"):
    return "%-10s %s::%s  %s  " % (klass, site["path"], site["fn"], site["fp"])


# ── the gate ────────────────────────────────────────────────────────────────

def check(root):
    """(failures, report_lines)."""
    failures, report = [], []
    scheme_sites, host_sites, _ = enumerate_sites(root)
    entries, bad = load_dispositions(root / DISPOSITIONS)
    failures.extend(bad)

    # ANTI-VACUITY, per axis, FAIL CLOSED.
    if len(scheme_sites) < MIN_SCHEME_SITES:
        failures.append(
            "ANTI-VACUITY: axis A found %d scheme-separator site(s), floor is "
            "%d. The enumeration has stopped matching -- a moved source root, "
            "a renamed literal, a regex that no longer fires. A gate that "
            "finds nothing must not pass."
            % (len(scheme_sites), MIN_SCHEME_SITES))
    if len(host_sites) < MIN_HOST_SHAPED:
        failures.append(
            "ANTI-VACUITY: axis B found %d host-shaped declaration(s), floor "
            "is %d. Same reading as above: this axis is the only one that "
            "sees a cut carrying no scheme literal, so an axis B that has "
            "gone quiet is a gate that cannot answer c1."
            % (len(host_sites), MIN_HOST_SHAPED))

    # AXIS C: the known positives. Checked before the sweep is believed.
    by_file = {}
    for site in host_sites:
        by_file.setdefault(site["path"], {})[site["fn"]] = site
    for rel, symbol, origin in KNOWN_POSITIVES:
        path = root / rel
        if not path.is_file():
            failures.append(
                "KNOWN-POSITIVE GONE: %s (%s) -- the file no longer exists. "
                "Five fixed sites are this gate's only proof that it can see "
                "the class it grades." % (rel, origin))
            continue
        raw = path.read_text(encoding="utf-8", errors="replace")
        code = strip_comments(raw)
        found = [f for f in functions(code) if f[0] == symbol]
        if not found:
            failures.append(
                "KNOWN-POSITIVE GONE: %s::%s (%s) -- no such function. Rename "
                "it here too, or this control is silently absent."
                % (rel, symbol, origin))
            continue
        declared = {}
        for name, _, _, _ in functions(code):
            declared[name] = declared.get(name, 0) + 1
        unique = {n for n, c in declared.items() if c == 1}
        bodies = {name: code[s:e] for name, _, s, e in functions(code) if name in unique}
        reaches = {n for n, b in bodies.items()
                   if any(m in b for m in PARSER_MARKERS)}
        body = code[found[0][2]:found[0][3]]
        direct = any(marker in body for marker in PARSER_MARKERS)
        via = [o for o in reaches if o != symbol
               and re.search(rf"(?<![:.\w]){re.escape(o)}\s*\(", body)]
        if not direct and not via:
            failures.append(
                "KNOWN-POSITIVE REGRESSED: %s::%s (%s) no longer reaches "
                "url::Url / wcore_types::url_authority. This is the bug the "
                "originating issue fixed, arriving again at its own site."
                % (rel, symbol, origin))
        else:
            report.append("known-positive ok: %s::%s (%s)" % (rel, symbol, origin))

    # AXIS A: every scheme-separator line is dispositioned.
    used = set()
    for site in scheme_sites:
        key = (site["path"], site["fn"], site["fp"])
        if key not in entries:
            failures.append(
                "UNDISPOSITIONED %s:%d in `%s`: %s\n"
                "      Classify it in %s -- add:\n      %s<why>"
                % (site["path"], site["line"], site["fn"], site["text"],
                   DISPOSITIONS, render_line(site)))
            continue
        used.add(key)

    # AXIS B: a host-shaped declaration either reaches the parser or is
    # dispositioned. Reaching it is the desired state and needs no entry --
    # the reason is the call, and a hand-written duplicate of it would rot.
    for site in host_sites:
        key = (site["path"], site["fn"], site["fp"])
        if site["parser"]:
            used.add(key)
            report.append(
                "parser  %s::%s -> %s%s"
                % (site["path"], site["fn"], site["ret"],
                   "" if not site["via"] else "  (via %s)" % ", ".join(site["via"])))
            continue
        if key not in entries:
            failures.append(
                "UNDISPOSITIONED %s:%d: `fn %s(..) -> %s` returns a host- or "
                "authority-shaped value and does NOT reach url::Url / "
                "wcore_types::url_authority.\n"
                "      Route it through wcore_types::url_authority, or "
                "classify it in %s -- add:\n      %s<why>"
                % (site["path"], site["line"], site["fn"], site["ret"],
                   DISPOSITIONS, render_line(site)))
            continue
        used.add(key)

    # STALE: an entry matching nothing would excuse the next write at that
    # site for free. The list is designed to shrink; a line that has quietly
    # stopped applying is deleted, not left.
    for key, (klass, _) in sorted(entries.items()):
        if key not in used:
            failures.append(
                "STALE %s: `%s %s::%s %s` matches no enumerated site. Either "
                "the site changed -- in which case re-read it and re-fingerprint "
                "-- or it is gone and the line goes with it."
                % (DISPOSITIONS, klass, key[0], key[1], key[2]))

    report.insert(0, "axis A scheme-separator sites: %d (floor %d)"
                  % (len(scheme_sites), MIN_SCHEME_SITES))
    report.insert(1, "axis B host-shaped declarations: %d (floor %d), %d reach "
                  "the parser without an entry"
                  % (len(host_sites), MIN_HOST_SHAPED,
                     sum(1 for s in host_sites if s["parser"])))
    report.insert(2, "axis C known positives: %d" % len(KNOWN_POSITIVES))
    report.insert(3, "dispositions read: %d" % len(entries))
    return failures, report


# ── self-test ───────────────────────────────────────────────────────────────

FOURTH_CUT_WITH_LITERAL = '''
/// A fourth hand cut, in the shape the class keeps arriving in.
pub fn provider_host(raw: &str) -> Option<String> {
    let after_scheme = raw.split_once("://")?.1;
    let authority = after_scheme.split('/').next()?;
    Some(authority.rsplit('@').next()?.to_ascii_lowercase())
}
'''

FOURTH_CUT_NO_LITERAL = '''
pub fn upstream_authority(raw: &str) -> &str {
    let start = raw.find("//").map(|i| i + 2).unwrap_or(0);
    let rest = &raw[start..];
    match rest.find('/') {
        Some(end) => &rest[..end],
        None => rest,
    }
}
'''

THROUGH_THE_PARSER = '''
pub fn provider_host(raw: &str) -> Option<String> {
    wcore_types::url_authority::dialed_host_str(raw)
}
'''

COMMENT_IS_NOT_A_PARSER = '''
pub fn blocked_host(raw: &str) -> Option<String> {
    // Strip the brackets `url::Url::host_str()` returns for IPv6 literals.
    // We do not call Url::parse( here -- this comment is prose, not a guard.
    let start = raw.find("//").map(|i| i + 2).unwrap_or(0);
    Some(raw[start..].split('/').next()?.to_string())
}
'''


def self_test():
    """Prove the gate can FAIL, on both axes and in both directions.

    A gate that cannot fail grades nothing, and that is the exact objection
    that kept #1276 open: #1252's measurement answered "clean" and would have
    answered "clean" one commit after a fourth cut landed. Running this before
    the real sweep makes the RC=0 below a result rather than an absence.
    """
    import tempfile

    cases = [
        ("a fourth cut carrying the scheme literal",
         FOURTH_CUT_WITH_LITERAL, True, True),
        ("a fourth cut with NO scheme literal (find/slice)",
         FOURTH_CUT_NO_LITERAL, False, True),
        ("the same function answered through the parser",
         THROUGH_THE_PARSER, False, False),
        ("a comment naming the parser is not a parser call",
         COMMENT_IS_NOT_A_PARSER, False, True),
    ]
    bad = []
    with tempfile.TemporaryDirectory(prefix="url-authority-gate-") as tmp:
        root = Path(tmp)
        src = root / "crates" / "synthetic" / "src"
        src.mkdir(parents=True)
        for label, fixture, want_scheme, want_host in cases:
            (src / "lib.rs").write_text(fixture, encoding="utf-8")
            scheme, host, _ = enumerate_sites(root)
            offending = [s for s in host if not s["parser"]]
            if bool(scheme) != want_scheme:
                bad.append("SELF-TEST FAIL [%s]: axis A saw %d site(s), wanted %s"
                           % (label, len(scheme), "one or more" if want_scheme else "none"))
            if bool(offending) != want_host:
                bad.append("SELF-TEST FAIL [%s]: axis B saw %d unparsed host-shaped "
                           "fn(s), wanted %s"
                           % (label, len(offending),
                              "one or more" if want_host else "none"))

        # Anti-vacuity must fail CLOSED on an empty tree, not report a clean
        # sweep. This is the arm `wayland-core#402` was closed on.
        (src / "lib.rs").write_text("pub fn nothing() {}\n", encoding="utf-8")
        (root / DISPOSITIONS).parent.mkdir(parents=True, exist_ok=True)
        (root / DISPOSITIONS).write_text("", encoding="utf-8")
        failures, _ = check(root)
        if not any("ANTI-VACUITY" in f for f in failures):
            bad.append("SELF-TEST FAIL: an empty enumeration did not fail closed")
        if not any("KNOWN-POSITIVE GONE" in f for f in failures):
            bad.append("SELF-TEST FAIL: absent known positives did not fail the gate")

        # A malformed / reasonless disposition line is refused rather than read
        # as a class-wide exemption.
        (root / DISPOSITIONS).write_text(
            "no-host  crates/x/src/lib.rs::f  deadbeef1234  short\n"
            "banana   crates/x/src/lib.rs::g  deadbeef1234  a perfectly long reason here\n"
            "no-host  nofunction  deadbeef1234  a perfectly long reason here indeed\n",
            encoding="utf-8")
        entries, malformed = load_dispositions(root / DISPOSITIONS)
        if len(malformed) != 3 or entries:
            bad.append("SELF-TEST FAIL: malformed disposition lines were accepted "
                       "(%d refused, %d accepted)" % (len(malformed), len(entries)))

    if bad:
        for line in bad:
            print(line)
        return 1
    print("SELF-TEST OK: both cuts flagged (with and without the scheme literal), "
          "the parser-backed rewrite accepted, a comment naming the parser "
          "rejected, an empty enumeration failed closed, absent known positives "
          "failed, malformed disposition lines refused")
    return 0


def main():
    if "--self-test" in sys.argv:
        return self_test()
    root = Path(__file__).resolve().parent.parent
    if "--list" in sys.argv:
        scheme, host, _ = enumerate_sites(root)
        for site in scheme:
            print(render_line(site) + "# " + site["text"][:90])
        for site in host:
            if not site["parser"]:
                print(render_line(site) + "# -> " + site["ret"])
        return 0
    failures, report = check(root)
    for line in report:
        print(line)
    if failures:
        print("\nFAIL: %d finding(s). A host- or authority-shaped value must "
              "come from url::Url / wcore_types::url_authority, or its site "
              "must say in %s why it does not.\n" % (len(failures), DISPOSITIONS))
        for failure in failures:
            print("  " + failure)
        return 1
    print("\nOK: every enumerated site answers through the parser or is "
          "classified, all known positives hold, no axis went quiet.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
