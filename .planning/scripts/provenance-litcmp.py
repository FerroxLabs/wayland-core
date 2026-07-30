#!/usr/bin/env python3
"""Distinctive-literal overlap between two source files (any language).

Method, applied identically to every pairing so the control is comparable:
  1. Extract every quoted string literal (single, double, backtick) of length >= 5.
  2. Lowercase; strip surrounding whitespace.
  3. Drop literals that are pure punctuation/format noise.
  4. Report |A|, |B|, |A n B|, Jaccard, and the actual shared literals.

A high overlap on a pair that CANNOT be derived means the metric is measuring
shared vendor vocabulary, not copying. That is what the control establishes.
"""
import re
import sys

LIT = re.compile(r'"([^"\\\n]{5,200})"' r"|'([^'\\\n]{5,200})'" r'|`([^`\\\n]{5,200})`')
NOISE = re.compile(r'^[\s\W_]*$')


def literals(path):
    with open(path, 'r', encoding='utf-8', errors='replace') as fh:
        src = fh.read()
    out = set()
    for m in LIT.finditer(src):
        s = (m.group(1) or m.group(2) or m.group(3)).strip().lower()
        if len(s) < 5 or NOISE.match(s):
            continue
        out.add(s)
    return out


def main():
    a_path, b_path = sys.argv[1], sys.argv[2]
    label = sys.argv[3] if len(sys.argv) > 3 else ''
    a, b = literals(a_path), literals(b_path)
    shared = sorted(a & b)
    union = len(a | b)
    jac = (len(shared) / union) if union else 0.0
    print(f'PAIR: {label}')
    print(f'  A = {a_path}   literals={len(a)}')
    print(f'  B = {b_path}   literals={len(b)}')
    print(f'  SHARED = {len(shared)}   JACCARD = {jac:.4f}')
    for s in shared:
        print(f'    | {s}')
    print()


if __name__ == '__main__':
    main()
