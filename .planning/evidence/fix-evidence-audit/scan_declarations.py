#!/usr/bin/env python3
"""Find CAPABILITY DECLARATIONS: functions whose whole body is one literal.

A capability declaration is a function that answers a question about the world
("does this platform honour an idempotency key", "how long a message does it
accept") by returning a constant. The constant is a CLAIM about somebody else's
system. Nothing in the type system checks it.

This scanner finds them. It does NOT judge them — the evidence classification is
done separately, per site.

Both-direction control lives in `selftest()`: the scanner must fire on a known
positive and stay silent on a known negative, and the known negative must be a
function that LOOKS like a declaration but has a real body.
"""

import os
import re
import sys
import json

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(
    os.path.abspath(__file__)))))

FN_RE = re.compile(
    r'^(?P<indent>[ \t]*)(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+'
    r'(?P<name>[A-Za-z_][A-Za-z0-9_]*)\s*'
    r'(?:<[^>]*>)?\s*\((?P<args>[^)]*)\)\s*'
    r'(?:->\s*(?P<ret>[^{;]+?))?\s*\{\s*$'
)

# A body is "one literal" if, between the opening and closing brace, there is
# exactly one non-comment, non-blank line and it is a literal-ish expression.
LITERAL_RE = re.compile(
    r'^\s*(?:'
    r'true|false'
    r'|Some\(\s*[0-9_]+\s*\)'
    r'|None'
    r'|[0-9][0-9_]*(?:usize|u64|u32|u16|u8|i64|i32|f64|f32)?'
    r'|Some\(\s*[A-Z][A-Za-z0-9_]*(?:::[A-Za-z0-9_]+)*\s*\)'
    r'|[A-Z][A-Z0-9_]{2,}'                      # SCREAMING_CONST
    r'|[A-Za-z_][A-Za-z0-9_]*::[A-Za-z0-9_]+\(\)'   # Type::none()
    r'|Duration::from_(?:secs|millis)\([0-9_]+\)'
    r')\s*$'
)

CAP_NAME_RE = re.compile(
    r'^(?:supports_|is_|can_|has_|allows_|requires_|should_|max_|min_|'
    r'.*_limit$|.*_bounds$|.*_cap$|.*_timeout$|.*_is_idempotent$|'
    r'.*_actions$|.*_capabilit)'
)


def scan_file(path):
    try:
        with open(path, 'r', encoding='utf-8', errors='replace') as fh:
            lines = fh.read().split('\n')
    except OSError:
        return []
    out = []
    for i, line in enumerate(lines):
        m = FN_RE.match(line)
        if not m:
            continue
        indent = m.group('indent')
        # collect body until the matching dedented close brace
        body = []
        closed = False
        for j in range(i + 1, min(i + 12, len(lines))):
            if lines[j] == indent + '}':
                closed = True
                break
            body.append((j, lines[j]))
        if not closed:
            continue
        real = [(n, t) for n, t in body
                if t.strip() and not t.strip().startswith('//')]
        if len(real) != 1:
            continue
        expr = real[0][1]
        if not LITERAL_RE.match(expr):
            continue
        out.append({
            'file': os.path.relpath(path, ROOT),
            'line': i + 1,
            'name': m.group('name'),
            'ret': (m.group('ret') or '()').strip(),
            'value': expr.strip(),
            'cap_named': bool(CAP_NAME_RE.match(m.group('name'))),
        })
    return out


def walk(subdir):
    hits = []
    base = os.path.join(ROOT, subdir)
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames[:] = [d for d in dirnames if d not in ('target', '.git')]
        for fn in filenames:
            if fn.endswith('.rs'):
                hits.extend(scan_file(os.path.join(dirpath, fn)))
    return hits


def selftest():
    """Three assertions, per LANE-BRIEF §6b-ii.

    1. known POSITIVE fires
    2. known NEGATIVE (a fn that returns a literal-looking thing but has a
       real multi-line body) stays silent
    3. a fn whose body is a real expression, not a literal, stays silent
       -- this is the assertion that proves the LITERAL_RE is doing work and
       the scanner is not just matching every fn.
    """
    import tempfile
    src = '''
impl Foo {
    fn supports_outbound_idempotency(&self) -> bool {
        true
    }

    fn max_message_len(&self) -> Option<usize> {
        Some(2000)
    }

    fn computed(&self) -> bool {
        let x = self.thing();
        x > 3
    }

    fn delegating(&self) -> bool {
        self.inner.supports_it()
    }
}
'''
    with tempfile.NamedTemporaryFile('w', suffix='.rs', delete=False) as fh:
        fh.write(src)
        p = fh.name
    got = {h['name'] for h in scan_file(p)}
    os.unlink(p)
    ok = True
    if 'supports_outbound_idempotency' not in got:
        print('SELFTEST FAIL: known positive (bool literal) not detected')
        ok = False
    if 'max_message_len' not in got:
        print('SELFTEST FAIL: known positive (Some(N)) not detected')
        ok = False
    if 'computed' in got:
        print('SELFTEST FAIL: multi-line body was reported as a declaration')
        ok = False
    if 'delegating' in got:
        print('SELFTEST FAIL: delegating call was reported as a literal')
        ok = False
    print(f'SELFTEST: detected={sorted(got)}  result={"PASS" if ok else "FAIL"}')
    return ok


if __name__ == '__main__':
    if not selftest():
        sys.exit(2)
    hits = walk('crates')
    hits.sort(key=lambda h: (h['file'], h['line']))
    print(f'TOTAL literal-bodied fns in crates/: {len(hits)}')
    cap = [h for h in hits if h['cap_named']]
    print(f'OF WHICH capability-named: {len(cap)}')
    with open(sys.argv[1], 'w') as fh:
        json.dump(hits, fh, indent=1)
    print(f'written: {sys.argv[1]}')
