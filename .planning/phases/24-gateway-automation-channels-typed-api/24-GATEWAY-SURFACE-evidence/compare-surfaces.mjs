#!/usr/bin/env node
// Cell-by-cell comparison of two f24-inbound result.json files taken at the SAME
// binary commit and differing ONLY in `--runtime`.
//
// Why cell-by-cell and not aggregate counts: two runs can both report
// "36 ran, failed=0" while disagreeing about WHICH six were not measured. An
// aggregate comparison would call that identical. This keys on adapter/leg.
//
// Self-test (`--self-test`) carries the third assertion LANE-BRIEF 6b-ii
// demands: a known-positive (a true zero stays zero), and TWO known-negatives
// (a flipped cell and a DROPPED cell must both be caught). A comparator that
// always prints 0 divergences would produce exactly the result this lane
// reports, so the known-negatives are the only thing that makes the zero mean
// anything.
//
// Usage:
//   node compare-surfaces.mjs <gateway-result.json> <json-stream-result.json>
//   node compare-surfaces.mjs --self-test <a.json> <b.json>

import fs from 'node:fs';

/// adapter/leg -> PASS | FAIL | NOT_MEASURED
export function cells(r) {
  const m = new Map();
  for (const x of r.results) m.set(`${x.adapter}/${x.leg}`, x.ok ? 'PASS' : 'FAIL');
  for (const x of r.not_measured ?? []) m.set(`${x.adapter}/${x.leg}`, 'NOT_MEASURED');
  return m;
}

/// MISSING is a distinct state, not an absence. A cell that vanished from one
/// side is a divergence — silently dropping it is how a lost leg hides.
export function compare(ca, cb) {
  const keys = new Set([...ca.keys(), ...cb.keys()]);
  const divergent = [];
  for (const k of [...keys].sort()) {
    const a = ca.get(k) ?? 'MISSING';
    const b = cb.get(k) ?? 'MISSING';
    if (a !== b) divergent.push({ cell: k, a, b });
  }
  return { total: keys.size, divergent };
}

function tally(m) {
  const t = {};
  for (const v of m.values()) t[v] = (t[v] ?? 0) + 1;
  return t;
}

const argv = process.argv.slice(2);
const selfTest = argv[0] === '--self-test';
const [pa, pb] = selfTest ? argv.slice(1) : argv;
if (!pa || !pb) {
  process.stderr.write('usage: compare-surfaces.mjs [--self-test] <a.json> <b.json>\n');
  process.exit(2);
}
const A = JSON.parse(fs.readFileSync(pa, 'utf8'));
const B = JSON.parse(fs.readFileSync(pb, 'utf8'));

if (selfTest) {
  let failed = 0;
  const check = (name, cond, extra) => {
    process.stdout.write(`${cond ? 'PASS' : 'FAIL'} ${name}${extra ? ` ${extra}` : ''}\n`);
    if (!cond) failed += 1;
  };

  const clean = compare(cells(A), cells(B));
  check('M3 KNOWN-POSITIVE: the unmodified pair reports a true zero', clean.divergent.length === 0,
    `divergences=${clean.divergent.length}`);

  const flipped = JSON.parse(JSON.stringify(A));
  const victim = flipped.results.find((x) => x.ok);
  victim.ok = false;
  const d1 = compare(cells(flipped), cells(B));
  check('M1 KNOWN-NEGATIVE: one flipped cell is detected',
    d1.divergent.length === 1 && d1.divergent[0].cell === `${victim.adapter}/${victim.leg}`,
    JSON.stringify(d1.divergent));

  const dropped = JSON.parse(JSON.stringify(A));
  const gone = dropped.results.pop();
  const d2 = compare(cells(dropped), cells(B));
  check('M2 KNOWN-NEGATIVE: a DROPPED cell cannot hide (reports MISSING)',
    d2.divergent.length === 1 && d2.divergent[0].a === 'MISSING',
    JSON.stringify(d2.divergent));
  check('M2b the dropped cell is the one we removed',
    d2.divergent.length === 1 && d2.divergent[0].cell === `${gone.adapter}/${gone.leg}`);

  process.stdout.write(`\nCOMPARATOR SELFTEST ${failed === 0 ? 'GREEN' : 'RED'} failed=${failed}\n`);
  process.exit(failed === 0 ? 0 : 1);
}

const ca = cells(A);
const cb = cells(B);
const { total, divergent } = compare(ca, cb);
process.stdout.write(`A runtime=${A.runtime} cells=${ca.size} tally=${JSON.stringify(tally(ca))}\n`);
process.stdout.write(`B runtime=${B.runtime} cells=${cb.size} tally=${JSON.stringify(tally(cb))}\n`);
process.stdout.write(`union=${total} identical=${total - divergent.length}/${total} DIVERGENT=${divergent.length}\n`);
for (const d of divergent) process.stdout.write(`  ${d.cell}: ${A.runtime}=${d.a} ${B.runtime}=${d.b}\n`);
process.exit(divergent.length === 0 ? 0 : 1);
