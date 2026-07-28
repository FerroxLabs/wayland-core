#!/usr/bin/env python3
"""Sweep the workspace for operator-facing strings that INSTRUCT, and extract
what each one advertises.

Output: TSV to stdout, one row per (string, advertised token).

This is deliberately a *measurement* tool, not a gate. Its job is to say how big
the surface is and what shape it has. The gate that consumes the classification
lives in Rust, next to the real types.

Design notes that matter:

  * Production source only. `tests/`, `benches/`, `examples/` and every
    `#[cfg(test)] mod tests { .. }` block are excluded, because a string that
    only a test ever sees is not advertised to anybody.
  * String literals are lexed, not regexed out of raw lines: Rust raw strings
    (`r"..."`, `r#"..."#`) and backslash-continued multi-line literals both
    appear in this codebase's error text and a line regex mangles them.
  * A string is INSTRUCTIVE only if it both (a) carries an imperative/remedy cue
    and (b) yields at least one extractable advertised token. (a) alone is prose
    we cannot check; (b) alone is usually a log field name.
"""

from __future__ import annotations

import os
import re
import sys

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))

SKIP_DIR_PARTS = ("/tests/", "/benches/", "/examples/", "/target/", "/.git/")

# ---------------------------------------------------------------- lexing


def strip_test_modules(src: str) -> str:
    """Remove `#[cfg(test)] mod ... { .. }` blocks by brace counting."""
    out = []
    i = 0
    while True:
        m = re.compile(r"#\[cfg\(test\)\]").search(src, i)
        if not m:
            out.append(src[i:])
            break
        out.append(src[i : m.start()])
        # find the opening brace of the item that follows
        j = src.find("{", m.end())
        if j < 0:
            break
        depth = 0
        k = j
        while k < len(src):
            if src[k] == "{":
                depth += 1
            elif src[k] == "}":
                depth -= 1
                if depth == 0:
                    break
            k += 1
        i = k + 1
    return "".join(out)


def lex_strings(src: str):
    """Yield (line_no, text) for every Rust string literal in `src`."""
    i = 0
    n = len(src)
    line = 1
    while i < n:
        c = src[i]
        if c == "\n":
            line += 1
            i += 1
            continue
        # line comment
        if c == "/" and i + 1 < n and src[i + 1] == "/":
            j = src.find("\n", i)
            i = n if j < 0 else j
            continue
        # block comment
        if c == "/" and i + 1 < n and src[i + 1] == "*":
            j = src.find("*/", i + 2)
            j = n if j < 0 else j + 2
            line += src.count("\n", i, j)
            i = j
            continue
        # char literal / lifetime -- skip so `'"'` cannot open a string
        if c == "'":
            m = re.match(r"'(\\.|[^\\'])'", src[i:])
            if m:
                i += m.end()
                continue
            i += 1
            continue
        # raw string
        m = re.match(r'r(#*)"', src[i:])
        if m:
            hashes = m.group(1)
            close = '"' + hashes
            start = i + m.end()
            j = src.find(close, start)
            if j < 0:
                break
            text = src[start:j]
            yield (line, text)
            line += src.count("\n", i, j)
            i = j + len(close)
            continue
        if c == '"':
            j = i + 1
            buf = []
            while j < n:
                if src[j] == "\\":
                    nxt = src[j + 1] if j + 1 < n else ""
                    if nxt == "\n":
                        # backslash-continuation: swallow the newline and the
                        # leading whitespace of the next line, as rustc does
                        k = j + 2
                        while k < n and src[k] in " \t":
                            k += 1
                        j = k
                        continue
                    buf.append({"n": "\n", "t": "\t", '"': '"', "\\": "\\"}.get(nxt, nxt))
                    j += 2
                    continue
                if src[j] == '"':
                    break
                buf.append(src[j])
                j += 1
            text = "".join(buf)
            yield (line, text)
            line += src.count("\n", i, j)
            i = j + 1
            continue
        i += 1


# ---------------------------------------------------------------- classifying

IMPERATIVE = re.compile(
    r"\b("
    r"set|sets|setting|run|runs|use|using|configure|install|add|pass|supply|provide|"
    r"enable|disable|export|try|select|specify|check|see|remove|rename|create|choose|"
    r"start|stop|re-?run|retry|unset|point|write|edit|put|place|turn|paste|permit|"
    r"allow|deny|grant|launch|invoke|call"
    r")\b",
    re.I,
)

# A pasteable TOML snippet is instruction even with no verb in it -- case 1's
# hint body is a bare `[browser.policy]\nallowed_origins = [..]` const. The
# original classifier dropped it for exactly that reason, which would have made
# this gate blind to the very defect it was built for.
TOML_SNIPPET = re.compile(r"^\s*\[[a-z][a-z0-9_.]*\]\s*$", re.M)
REMEDY_CUE = re.compile(
    r"(instead|to fix|remedy|hint:|help:|did you mean|expected|must be|"
    r"you can|you must|please|or set|or use|or run|or pass|first run|then run)",
    re.I,
)

ENV = re.compile(r"\b([A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+)\b")
FLAG = re.compile(r"(?<![\w-])--([a-z][a-z0-9]*(?:-[a-z0-9]+)*)")
SECTION = re.compile(r"\[([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*)\]")
KEYVAL = re.compile(r"(?<![\w.\-])([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*)\s*=\s*(\"[^\"]*\"|\[[^\]]*\]|true|false|-?\d+)")
DOTTED = re.compile(r"(?<![\w./\-])([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]+)+)(?![\w./(])")
SUBCMD = re.compile(r"wayland-core\s+((?:[a-z][a-z0-9-]*)(?:\s+[a-z][a-z0-9-]*)?)")
DOCPATH = re.compile(r"\b((?:docs/[\w./-]+\.md)|README\.md)")
# A backticked bare token an operator is told to type -- e.g. the `ollama:`
# model-id prefix. Not gate-checkable generically (there is no one consumer),
# but it must be counted, not silently dropped.
BACKTICKED = re.compile(r"`([A-Za-z][A-Za-z0-9_.:-]*)`")

# dotted things that are file names / crate paths / rust expressions, not config keys
DOTTED_DENY = re.compile(
    r"\.(toml|json|md|rs|yaml|yml|txt|log|db|sock|sh|exe|dll|so|dylib|lock|png|jpg|env|com|net|org|io|dev|test|local|ai|co|sh)$"
)


def qualified_keyvals(text: str):
    """Yield fully-qualified `section.key = value` pairs.

    A remediation string is almost never one flat key: it is a section header
    followed by keys, or a prose sentence naming both ("set [storage.credentials]
    backend = \"keyring\""). The gate needs the *joined* path, because the whole
    point of case 1 and case 5(a) is that the key was correct and the SECTION was
    wrong -- checking the key alone passes on both defects.

    Pairing is POSITIONAL -- each assignment binds to the nearest section header
    that precedes it in the text. A "last header on the line wins" rule looked
    fine and was wrong: the live headless-keyring string is a single line reading
    `... set [storage.credentials] backend = "keyring" ... [session] enabled =
    false`, and line-scoped pairing bound `backend` to `[session]`, inventing a
    key that does not exist. That is a FALSE POSITIVE, which is strictly worse
    than a miss -- a gate that reds on correct text gets deleted.
    """
    heads = [(m.start(), m.group(1)) for m in SECTION.finditer(text)]
    # comment lines in a pasted snippet are not instruction
    masked = "\n".join("" if ln.strip().startswith("#") else ln for ln in text.split("\n"))
    for m in KEYVAL.finditer(masked):
        key, value = m.group(1), m.group(2)
        section = None
        for pos, name in heads:
            if pos < m.start():
                section = name
            else:
                break
        if section and not key.startswith(section.split(".")[0] + "."):
            path = f"{section}.{key}"
        else:
            path = key
        yield (path, value)


def tokens(text: str):
    """Yield (kind, token) advertised by `text`."""
    for m in ENV.finditer(text):
        yield ("env", m.group(1))
    for m in FLAG.finditer(text):
        yield ("flag", "--" + m.group(1))
    for m in SECTION.finditer(text):
        yield ("config_section", m.group(1))
    for path, value in qualified_keyvals(text):
        yield ("config_assign", f"{path} = {value}")
    for m in DOTTED.finditer(text):
        tok = m.group(1)
        if DOTTED_DENY.search(tok):
            continue
        yield ("config_key", tok)
    for m in SUBCMD.finditer(text):
        yield ("subcommand", m.group(1).strip())
    for m in DOCPATH.finditer(text):
        yield ("doc_path", m.group(1))
    for m in BACKTICKED.finditer(text):
        tok = m.group(1)
        if tok.isupper() and "_" in tok:
            continue  # already yielded as env
        if "." in tok and DOTTED_DENY.search(tok):
            continue
        yield ("code_token", tok)


def context_of(src: str, pos_line: int, lines: list[str]) -> str:
    idx = pos_line - 1
    window = "\n".join(lines[max(0, idx - 2) : idx + 1])
    if "#[error(" in window:
        return "error_display"
    if re.search(r"\b(bail!|anyhow!|panic!|Err\()", window):
        return "error_construct"
    if re.search(r"\b(eprintln!|println!|writeln!|print!)", window):
        return "stdout"
    if re.search(r"(long_help|\.help\(|about =|help =)", window):
        return "clap_help"
    if re.search(r"\b(warn!|error!|info!)", window):
        return "log"
    return "other"


def main() -> int:
    rows = []
    prose_only = 0
    files = 0
    lits = 0
    for dirpath, dirnames, filenames in os.walk(os.path.join(ROOT, "crates")):
        dirnames[:] = [d for d in dirnames if d not in ("target", ".git")]
        for fn in filenames:
            if not fn.endswith(".rs"):
                continue
            path = os.path.join(dirpath, fn)
            rel = os.path.relpath(path, ROOT)
            if any(p in "/" + rel for p in SKIP_DIR_PARTS):
                continue
            files += 1
            try:
                src = open(path, encoding="utf-8", errors="replace").read()
            except OSError:
                continue
            src = strip_test_modules(src)
            lines = src.split("\n")
            for line, text in lex_strings(src):
                lits += 1
                if len(text) < 12:
                    continue
                instructive = (
                    bool(IMPERATIVE.search(text))
                    or bool(REMEDY_CUE.search(text))
                    or bool(TOML_SNIPPET.search(text))
                )
                if not instructive:
                    continue
                ctx = context_of(src, line, lines)
                toks = sorted(set(tokens(text)))
                if not toks:
                    # Tokenless instruction is prose. Only count it when it is
                    # in a surface an operator actually reads -- otherwise every
                    # internal comment-ish literal inflates the denominator.
                    if ctx == "other":
                        continue
                    prose_only += 1
                    rows.append((rel, line, ctx, "prose", "", text))
                    continue
                for kind, tok in toks:
                    rows.append((rel, line, ctx, kind, tok, text))

    sys.stderr.write(
        f"scanned {files} production .rs files, {lits} string literals; "
        f"{len(rows)} inventory rows; {prose_only} instructive-but-tokenless (prose)\n"
    )
    print("file\tline\tcontext\ttoken_kind\ttoken\tstring")
    for rel, line, ctx, kind, tok, text in sorted(rows):
        flat = text.replace("\t", " ").replace("\n", " ")
        flat = re.sub(r"\s+", " ", flat).strip()
        print(f"{rel}\t{line}\t{ctx}\t{kind}\t{tok}\t{flat}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
