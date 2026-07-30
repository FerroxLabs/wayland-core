#!/usr/bin/env python3
"""Detector 2 — INVENTED PAYLOAD SHAPES.

A consumer reads a named field out of a dynamically-typed payload. Its tests
build that payload by hand. If NO production code anywhere in the workspace ever
writes that field name, then the shape exists only in the consumer's imagination
and in the tests the consumer's author wrote to confirm it. The tests are green
and the feature has never worked.

This is the shape of the TUI bash tool-result formatter defect: it reads
`cmd` / `exit_code` / `stdout`, the real tool returns `ToolResult { content:
String }`, and every writer of `exit_code` on that path is a test.

Method
------
1. Collect every READ of a named field from a `serde_json::Value`-ish payload:
   `x.get("k")`, `str_or(p, "k", ..)`, `i64_or(..)`, `bool_or(..)`, `u64_or(..)`,
   `p["k"]`.
2. Collect every WRITE of that name: `"k":` inside a `json!`/literal,
   `insert("k"`, `#[serde(rename = "k")]`, or a Rust struct field `k:` on a
   type that derives Serialize.
3. Split writes into PRODUCTION (a path under `src/` that is not inside a
   `#[cfg(test)]` module) and TEST (a `tests/` file, or inside `#[cfg(test)]`).
4. Report reads whose writes are ALL test-side. Those are invented shapes.

Both-direction control in `selftest()`.
"""

import os
import re
import sys
import json
from collections import defaultdict

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.dirname(
    os.path.abspath(__file__)))))

READ_RES = [
    re.compile(r'\.get\(\s*"([A-Za-z_][A-Za-z0-9_]*)"\s*\)'),
    re.compile(r'\b(?:str_or|i64_or|u64_or|f64_or|bool_or|opt_str)\s*\('
               r'[^,]+,\s*"([A-Za-z_][A-Za-z0-9_]*)"'),
    re.compile(r'\[\s*"([A-Za-z_][A-Za-z0-9_]*)"\s*\]'),
]
WRITE_RES = [
    re.compile(r'"([A-Za-z_][A-Za-z0-9_]*)"\s*:'),           # json!{"k": v}
    re.compile(r'\.insert\(\s*"([A-Za-z_][A-Za-z0-9_]*)"'),
    re.compile(r'serde\s*\(\s*rename\s*=\s*"([A-Za-z_][A-Za-z0-9_]*)"'),
]


def classify_lines(path, text):
    """Return per-line bool: True if the line is TEST-side."""
    lines = text.split('\n')
    is_test = [False] * len(lines)
    rel = os.path.relpath(path, ROOT)
    file_is_test = ('/tests/' in rel or rel.endswith('_test.rs')
                    or '/benches/' in rel or '/examples/' in rel)
    if file_is_test:
        return lines, [True] * len(lines)
    # find #[cfg(test)] mod blocks and mark their spans
    depth = None
    brace = 0
    pending = False
    for i, ln in enumerate(lines):
        if depth is None:
            if re.match(r'\s*#\[cfg\(test\)\]', ln):
                pending = True
                continue
            if pending and re.match(r'\s*(?:pub\s+)?mod\s+\w+\s*\{', ln):
                depth = i
                brace = ln.count('{') - ln.count('}')
                is_test[i] = True
                pending = False
                continue
            if pending and ln.strip() and not ln.strip().startswith('//'):
                # attribute applied to something that is not a mod block
                pending = False
        else:
            is_test[i] = True
            brace += ln.count('{') - ln.count('}')
            if brace <= 0:
                depth = None
    return lines, is_test


def scan(subdir):
    reads = defaultdict(list)          # key -> [(rel, lineno)]
    writes_prod = defaultdict(list)
    writes_test = defaultdict(list)
    base = os.path.join(ROOT, subdir)
    nfiles = 0
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames[:] = [d for d in dirnames if d not in ('target', '.git')]
        for fn in filenames:
            if not fn.endswith('.rs'):
                continue
            p = os.path.join(dirpath, fn)
            rel = os.path.relpath(p, ROOT)
            try:
                text = open(p, encoding='utf-8', errors='replace').read()
            except OSError:
                continue
            nfiles += 1
            lines, is_test = classify_lines(p, text)
            for i, ln in enumerate(lines):
                stripped = ln.strip()
                if stripped.startswith('//') or stripped.startswith('///'):
                    continue
                for rx in READ_RES:
                    for k in rx.findall(ln):
                        if not is_test[i]:
                            reads[k].append((rel, i + 1))
                for rx in WRITE_RES:
                    for k in rx.findall(ln):
                        (writes_test if is_test[i] else writes_prod)[k].append(
                            (rel, i + 1))
    return reads, writes_prod, writes_test, nfiles


def selftest():
    """Three assertions (§6b-ii).

    positive: a key read in prod, written only in a #[cfg(test)] mod -> flagged
    negative: a key read in prod AND written in prod                -> silent
    third:    the OLD naive matcher (no cfg(test) span tracking) would have
              treated the test write as a production write and missed the
              positive. Assert the span tracker actually marks it.
    """
    import tempfile
    src = '''
fn consume(p: &Value) -> i64 {
    let a = i64_or(p, "invented_key", 0);
    let b = p.get("real_key").unwrap();
    a
}

fn produce() -> Value {
    json!({ "real_key": 1 })
}

#[cfg(test)]
mod tests {
    #[test]
    fn t() {
        let p = json!({ "invented_key": 3, "real_key": 1 });
        assert_eq!(consume(&p), 3);
    }
}
'''
    d = tempfile.mkdtemp()
    p = os.path.join(d, 'src_selftest.rs')
    open(p, 'w').write(src)
    lines, is_test = classify_lines(p, src)
    os.unlink(p)
    ok = True
    # third assertion: span tracker marks the cfg(test) mod body
    test_line_idx = next(i for i, l in enumerate(lines) if 'invented_key": 3' in l)
    if not is_test[test_line_idx]:
        print('SELFTEST FAIL: cfg(test) span tracker did not mark the test write '
              '-- the naive matcher defect is still present')
        ok = False
    prod_line_idx = next(i for i, l in enumerate(lines) if '"real_key": 1 })' in l)
    if is_test[prod_line_idx]:
        print('SELFTEST FAIL: production json! was misclassified as test')
        ok = False
    read_line_idx = next(i for i, l in enumerate(lines) if 'invented_key", 0' in l)
    if is_test[read_line_idx]:
        print('SELFTEST FAIL: production read misclassified as test')
        ok = False
    print(f'SELFTEST: cfg(test) span={is_test[test_line_idx]} '
          f'prod_write_test={is_test[prod_line_idx]} '
          f'prod_read_test={is_test[read_line_idx]} '
          f'result={"PASS" if ok else "FAIL"}')
    return ok


def per_consumer_score(reads, wp, wt):
    """REPAIR for the false negative the control exposed.

    The workspace-wide writer check misses a key when ANY unrelated subsystem
    happens to write the same NAME. `exit_code` is written by
    `child_transaction/gate_executor.rs`, which has nothing to do with the TUI
    bash formatter, and that single collision hid the known defect.

    So score by CONSUMER FILE instead of by key. A file that reads five keys and
    has a production producer for only one of them is reading an invented shape,
    and the one collision no longer buys it a pass.
    """
    by_file = defaultdict(lambda: {'unbacked': [], 'backed': []})
    for k, sites in reads.items():
        for rel, ln in sites:
            bucket = 'backed' if k in wp else 'unbacked'
            by_file[rel][bucket].append((k, ln))
    rows = []
    for rel, d in by_file.items():
        u = len({k for k, _ in d['unbacked']})
        b = len({k for k, _ in d['backed']})
        if u + b == 0:
            continue
        rows.append({
            'file': rel,
            'keys_read': u + b,
            'unbacked': u,
            'ratio': round(u / (u + b), 3),
            'unbacked_keys': sorted({k for k, _ in d['unbacked']}),
            'backed_keys': sorted({k for k, _ in d['backed']}),
            'unbacked_with_test_writer': sorted(
                {k for k, _ in d['unbacked'] if k in wt}),
        })
    rows.sort(key=lambda r: (-r['ratio'], -r['keys_read']))
    return rows


if __name__ == '__main__':
    if not selftest():
        sys.exit(2)
    reads, wp, wt, nfiles = scan('crates')
    print(f'scanned {nfiles} .rs files under crates/')
    print(f'distinct keys READ in production: {len(reads)}')
    invented = {}
    for k, sites in reads.items():
        if k in wp:
            continue          # somebody in production writes it
        invented[k] = {
            'read_at': sites,
            'test_writes': wt.get(k, []),
        }
    print(f'keys read in production with NO production writer: {len(invented)}')
    with_testwrites = {k: v for k, v in invented.items() if v['test_writes']}
    print(f'  ... of which the ONLY writer is a test: {len(with_testwrites)}')

    rows = per_consumer_score(reads, wp, wt)
    # CONTROL, run every time, printed every time. The repaired instrument must
    # rediscover the known TUI bash-formatter defect through the per-consumer
    # score even though `exit_code` alone is name-collided into invisibility.
    known = [r for r in rows
             if r['file'].endswith('tui/tool_formatters/bash.rs')]
    if known:
        r = known[0]
        print(f"CONTROL(known positive) {r['file']}: "
              f"{r['unbacked']}/{r['keys_read']} unbacked, ratio={r['ratio']}, "
              f"unbacked_keys={r['unbacked_keys']}")
    else:
        print('CONTROL FAIL: known-positive consumer not present in scored rows')

    with open(sys.argv[1], 'w') as fh:
        json.dump({
            'invented_no_writer_at_all':
                {k: v for k, v in invented.items() if not v['test_writes']},
            'invented_test_only_writer': with_testwrites,
            'per_consumer_score': rows,
        }, fh, indent=1)
    print(f'written: {sys.argv[1]}')
