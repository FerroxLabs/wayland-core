#!/usr/bin/env python3
"""Literal-overlap metric — REIMPLEMENTATION of lane/provenance-comparison's
`litcmp.py`, which was not committed with that lane.

Spec, quoted from `.planning/PROVENANCE-COMPARISON-NOTES.md` MEASUREMENT 3:

    Method (`litcmp.py`, identical for every pairing): extract every quoted
    literal of length >= 5, lowercase, drop punctuation-only, report |A n B|
    and Jaccard.

This file does NOT invent a metric. Its correctness criterion is that it
REPRODUCES that lane's published numbers on that lane's published pairings.
`--controls` runs them and prints PASS/FAIL per row. If the controls do not
reproduce, the calibration bands do not transfer and no score from this file
may be compared against them.

Calibration bands, from the same table:
  known copy (our own template copies)      0.2364 - 0.3438
  independently written vs peer             0.0000 - 0.0435
  elevated, below the copy band             0.0948 - 0.1493
"""

import re
import subprocess
import sys

# Quoted literals. Rust raw strings, Rust/TS/Python double, single, and
# backtick-quoted. Non-greedy, no escape handling -- deliberately simple, as
# the metric is a bag-of-literals not a parser.
LIT_RES = [
    re.compile(r'r#"(.*?)"#', re.S),
    re.compile(r'"((?:[^"\\\n]|\\.)*)"'),
    re.compile(r"'((?:[^'\\\n]|\\.)*)'"),
    re.compile(r'`([^`]*)`', re.S),
]
PUNCT_ONLY = re.compile(r'^[^0-9A-Za-z]+$')


def literals(text):
    out = set()
    for rx in LIT_RES:
        for m in rx.findall(text):
            # Plain strip only. A quote-stripping variant was tried and scored
            # WORSE against the published controls (it fixed deepseek's union
            # but broke cerebras's, which the raw form reproduces exactly), so
            # the raw form is the one that matches the original instrument.
            s = m.strip()
            if len(s) < 5:
                continue
            if PUNCT_ONLY.match(s):
                continue
            out.add(s.lower())
    return out


def read_local(path):
    with open(path, encoding='utf-8', errors='replace') as fh:
        return fh.read()


def read_peer(repo, sha, path):
    """Peer source is read at the PINNED sha via `git show`, never from the
    working tree -- the peer working trees are ~1 month ahead of the pinned
    baselines (PROVENANCE-COMPARISON-NOTES, peer baseline assertion). READ ONLY:
    `git show` mutates nothing.
    """
    return subprocess.run(
        ['/usr/bin/git', '-C', repo, 'show', f'{sha}:{path}'],
        capture_output=True, text=True, check=True).stdout


def compare(a_text, b_text):
    A, B = literals(a_text), literals(b_text)
    inter = A & B
    union = A | B
    j = len(inter) / len(union) if union else 0.0
    return len(inter), round(j, 4), sorted(inter), len(A), len(B)


CORE = '/Users/seandonahoe/dev/waylandcore-frontier-worktrees/lane-fix-evidence-audit'
OPENCLAW = '/Users/seandonahoe/dev/resources/openclaw'
OC_SHA = '11a0ad10'

# (label, our path, peer repo, peer sha, peer path, published_shared, published_jaccard)
CONTROLS = [
    ('POS cerebras vs moonshot (both ours)',
     'crates/wcore-providers/src/cerebras.rs', None, None,
     'crates/wcore-providers/src/moonshot.rs', 11, 0.3438),
    ('POS deepseek vs moonshot (both ours)',
     'crates/wcore-providers/src/deepseek.rs', None, None,
     'crates/wcore-providers/src/moonshot.rs', 13, 0.2364),
    ('NEG cooldown.rs vs errors.ts',
     'crates/wcore-providers/src/cooldown.rs', OPENCLAW, OC_SHA,
     'src/utils/errors.ts', 0, 0.0000),
    ('S1 failover.rs vs failover-error.ts',
     'crates/wcore-providers/src/failover.rs', OPENCLAW, OC_SHA,
     'src/agents/failover-error.ts', 10, 0.1493),
    ('S3 classify.rs vs errors.ts',
     'crates/wcore-providers/src/classify.rs', OPENCLAW, OC_SHA,
     'src/utils/errors.ts', 29, 0.0948),
]


def run_controls():
    print('CONTROL REPRODUCTION — my litcmp vs lane/provenance-comparison\'s '
          'published numbers')
    print(f"{'pairing':44} {'mine':>14}  {'published':>14}  verdict")
    allok = True
    for label, ours, prepo, psha, ppath, xs, xj in CONTROLS:
        a = read_local(f'{CORE}/{ours}')
        b = read_peer(prepo, psha, ppath) if prepo else read_local(f'{CORE}/{ppath}')
        n, j, _, _, _ = compare(a, b)
        ok = (n == xs and abs(j - xj) < 0.0002)
        allok &= ok
        print(f'{label:44} {n:5} {j:8.4f}  {xs:5} {xj:8.4f}  '
              f'{"MATCH" if ok else "DIFFERS"}')
    print('CONTROLS:', 'REPRODUCED — calibration transfers' if allok
          else 'NOT REPRODUCED — bands do NOT transfer')
    return allok


if __name__ == '__main__':
    if sys.argv[1:2] == ['--controls']:
        sys.exit(0 if run_controls() else 1)
    # litcmp.py <our_rel_path> <peer_repo> <peer_sha> <peer_path>
    ours, prepo, psha, ppath = sys.argv[1:5]
    a = read_local(f'{CORE}/{ours}')
    b = read_peer(prepo, psha, ppath)
    n, j, shared, na, nb = compare(a, b)
    print(f'ours={ours} (|A|={na})')
    print(f'peer={ppath}@{psha} (|B|={nb})')
    print(f'shared={n} jaccard={j}')
    for s in shared[:60]:
        print(f'   | {s!r}')
