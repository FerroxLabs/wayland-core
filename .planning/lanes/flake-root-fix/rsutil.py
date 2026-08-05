"""Shared Rust-source scanning helpers.

INSTRUMENT REPAIR (LANE-BRIEF §6b-ii). The first version of the census counted
`{`/`}` with a naive `line.count("{")`, which counts braces INSIDE string
literals and comments. `std::fs::write(&path, "{ not json")` therefore left the
brace depth permanently positive, so a function body never "closed" and the
scanner attributed every following test's env mutations to the wrong function.
That produced a FALSE POSITIVE: `google_meet_token_status_absent_when_unparsable`
was reported as a WAYLAND_HOME mutator when its body does not touch env at all.

`strip_literals` removes string/char literals (including raw strings) and
comments before any brace counting.
"""
import re

_RAW = re.compile(r'r(#*)"')


def strip_literals(line: str, state: dict) -> str:
    """Blank out string/char literals and comments on one line.

    `state` carries multi-line context across calls: {'block': bool, 'raw': int|None}
    Returns the line with literal/comment content replaced by spaces, so brace
    counting sees only real code.
    """
    out = []
    i = 0
    n = len(line)
    while i < n:
        # inside a /* */ block comment
        if state.get("block"):
            end = line.find("*/", i)
            if end == -1:
                return "".join(out) + " " * (n - i)
            out.append(" " * (end + 2 - i))
            i = end + 2
            state["block"] = False
            continue
        # inside a multi-line ordinary string
        if state.get("str"):
            j = i
            closed = False
            while j < n:
                if line[j] == "\\":
                    j += 2
                    continue
                if line[j] == '"':
                    j += 1
                    closed = True
                    break
                j += 1
            out.append(" " * (min(j, n) - i))
            i = j
            if closed:
                state["str"] = False
            continue
        # inside a raw string r#"..."#
        if state.get("raw") is not None:
            hashes = state["raw"]
            term = '"' + "#" * hashes
            end = line.find(term, i)
            if end == -1:
                return "".join(out) + " " * (n - i)
            out.append(" " * (end + len(term) - i))
            i = end + len(term)
            state["raw"] = None
            continue
        ch = line[i]
        # line comment
        if ch == "/" and i + 1 < n and line[i + 1] == "/":
            out.append(" " * (n - i))
            break
        if ch == "/" and i + 1 < n and line[i + 1] == "*":
            state["block"] = True
            out.append("  ")
            i += 2
            continue
        # raw string start
        m = _RAW.match(line, i)
        if m:
            hashes = len(m.group(1))
            state["raw"] = hashes
            out.append(" " * (m.end() - i))
            i = m.end()
            continue
        # ordinary string (MAY span lines -- see state['str'] below)
        if ch == '"':
            j = i + 1
            closed = False
            while j < n:
                if line[j] == "\\":
                    j += 2
                    continue
                if line[j] == '"':
                    j += 1
                    closed = True
                    break
                j += 1
            out.append(" " * (min(j, n) - i))
            i = j
            if not closed:
                # Rust string literals may continue onto the next line (with or
                # without a trailing `\` continuation). Without this state the
                # scanner restarted mid-string and counted the literal's
                # contents as code -- e.g. a shell `printf '{{...}}'` inside a
                # format! string leaked two unmatched `{` into the brace depth,
                # so a function body never closed and swallowed 8 later tests.
                state["str"] = True
            continue
        # char literal / lifetime: only treat as literal if it closes on this line
        if ch == "'":
            m2 = re.match(r"'(\\.|[^'\\])'", line[i:])
            if m2:
                out.append(" " * m2.end())
                i += m2.end()
                continue
        out.append(ch)
        i += 1
    return "".join(out)


def fn_body_range(lines, start, state_seed=None):
    """Return the last line index (inclusive) of the fn body starting at `start`.

    Brace counting runs over literal-stripped code.
    """
    state = dict(state_seed or {})
    depth = 0
    started = False
    k = start
    while k < len(lines):
        code = strip_literals(lines[k], state)
        depth += code.count("{") - code.count("}")
        if "{" in code:
            started = True
        if started and depth <= 0:
            return k
        k += 1
    return len(lines) - 1
