#!/usr/bin/env node
/**
 * f24-c3-clauses-selftest.mjs — prove the clause driver's graders can FAIL
 * before trusting anything they say.
 *
 * LANE-BRIEF §3.2: "Before you trust any gate you write or run, ask whether it
 * could fail." §6b-ii: every instrument repair gets THREE assertions — a
 * known-positive passes, a known-negative fails, **and the old broken matcher
 * would have missed it**. That third assertion is the only one that proves the
 * repair does anything; without it the self-test passes on the broken instrument
 * too.
 *
 * Exit 0 = all passed, 1 = any failed.
 */

import { classify, matches, instrumentFault, legacyMatches } from './f24-correlate.mjs';
import fs from 'node:fs';
import path from 'node:path';

let passed = 0;
let failed = 0;

/**
 * Assertion helper.
 *
 * # Why any thenable hard-fails
 *
 * A sibling self-test in this phase (`f24-discord-selftest.mjs`) had ONE test
 * written `async`. An async assertion failure REJECTS rather than throws, so the
 * checker saw no exception and incremented `passed`. Measured on node v22: a
 * deliberately false assertion printed `ok`, printed `passed=1 failed=0`, and
 * exited 0 — a self-passing gate inside the file whose entire job is to prove
 * nothing else self-passes. Repaired STRUCTURALLY there and structurally here.
 */
function check(name, fn) {
  let result;
  try {
    result = fn();
  } catch (e) {
    failed += 1;
    process.stdout.write(`FAIL ${name}: threw ${e && e.message ? e.message : e}\n`);
    return;
  }
  if (result && typeof result.then === 'function') {
    failed += 1;
    process.stdout.write(
      `FAIL ${name}: returned a thenable. An async assertion rejects rather than throws, ` +
        `so this checker would score a false assertion as a pass. Make the test synchronous.\n`,
    );
    return;
  }
  passed += 1;
  process.stdout.write(`ok   ${name}\n`);
}

function assert(cond, msg) {
  if (!cond) throw new Error(msg);
}

// ─────────────────────────────────────────────────────────────────────────────
// 1. The correlation matcher, three ways (§6b-ii)
// ─────────────────────────────────────────────────────────────────────────────

const TOKEN = 'f24c3fin-f24finone-a1b2c3d4';

check('known-positive: a raw token is an exact arrival and is countable', () => {
  const text = `F24C3-REPLY ${TOKEN} done`;
  assert(classify(text, TOKEN) === 'exact', `want exact, got ${classify(text, TOKEN)}`);
  assert(matches(text, TOKEN), 'must count as an arrival');
  assert(!instrumentFault(text, TOKEN), 'must not be a fault');
});

check('known-negative: an unrelated reply is absent and is NOT excused as a fault', () => {
  const text = 'F24C3-REPLY some other message entirely';
  assert(classify(text, TOKEN) === 'absent', `want absent, got ${classify(text, TOKEN)}`);
  assert(!matches(text, TOKEN), 'must not count as an arrival');
  // This is the load-bearing half of the negative. If a genuine absence were
  // graded as an instrument fault, every real LOSS would be laundered into
  // INCOMPLETE and the driver could never report a product defect.
  assert(!instrumentFault(text, TOKEN), 'a genuine absence must NOT be excused as INCOMPLETE');
});

check('THE THIRD ASSERTION: the old matcher misses what the repaired one catches', () => {
  // MarkdownV2 escaping, exactly as telegram emits it. This is the real shape
  // that made a lane report replied=0 against eight arrivals that had landed.
  const escaped = `F24C3\\-REPLY ${TOKEN.replace(/-/g, '\\-')}`;
  assert(
    legacyMatches(escaped, TOKEN) === false,
    'precondition: the OLD matcher must miss this, else the case proves nothing',
  );
  assert(
    matches(escaped, TOKEN),
    'the repaired matcher must catch it — otherwise the repair changes no outcome',
  );
  assert(classify(escaped, TOKEN) === 'normalized', `want normalized, got ${classify(escaped, TOKEN)}`);
});

check('a present-but-undecodable token grades FAULT, never absence', () => {
  // Percent-encoding inserts ALPHANUMERICS, so a skeleton-substring test alone
  // does not catch it — the bounded subsequence window is what does.
  const mangled = `F24C3-REPLY ${TOKEN.replace(/-/g, '%2D')}`;
  assert(!matches(mangled, TOKEN), 'must not be counted as a decoded arrival');
  assert(
    instrumentFault(mangled, TOKEN),
    'must be flagged as an instrument fault so the run grades INCOMPLETE rather than LOSS',
  );
});

// ─────────────────────────────────────────────────────────────────────────────
// 2. The leg grader — can it fail, and does the positive control bite?
// ─────────────────────────────────────────────────────────────────────────────
//
// Reimplemented here rather than imported, because the driver's `record` lives
// on a class that spawns processes. The LOGIC is what is under test and it is
// three lines; a divergence between these three lines and the driver's is
// caught by the source-scan below.

function grade(ok, positiveControl) {
  const controlOk = positiveControl === undefined ? null : positiveControl > 0;
  return ok && (controlOk === null || controlOk);
}

check('grader: a true assertion with a live control PASSES', () => {
  assert(grade(true, 3) === true, 'should pass');
});

check('grader: a false assertion FAILS even with a live control', () => {
  assert(grade(false, 3) === false, 'should fail');
});

check('THE UNIVERSAL-DENIAL TRAP: a true assertion with a ZERO control FAILS', () => {
  // This is the exact shape that let 24-C3's `access` leg pass on all three
  // adapters at a broken binary BECAUSE EVERYTHING WAS DENIED, and the shape
  // that let the Discord mutation run satisfy `stranger_replies=0` under total
  // inbound loss. The control is part of the pass condition, not a decoration.
  assert(
    grade(true, 0) === false,
    'a leg whose assertion holds while nothing arrived MUST fail — this is the manufactured green',
  );
});

check('grader: a control of exactly 1 is live and does not fail the leg', () => {
  assert(grade(true, 1) === true, 'off-by-one: 1 arrival is a live control');
});

// ─────────────────────────────────────────────────────────────────────────────
// 3. Journal reading — an empty file and an absent file must not read alike
// ─────────────────────────────────────────────────────────────────────────────

function readJournal(file) {
  if (!fs.existsSync(file)) return { records: [], bytes: 0, existed: false };
  const raw = fs.readFileSync(file, 'utf8');
  const records = raw
    .split('\n')
    .filter((l) => l.trim())
    .map((l) => {
      try {
        return JSON.parse(l);
      } catch {
        return null;
      }
    })
    .filter(Boolean);
  return { records, bytes: Buffer.byteLength(raw, 'utf8'), existed: true };
}

check('journal: an ABSENT file and an EMPTY file are distinguishable', () => {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || '/tmp', 'f24selftest-'));
  const empty = path.join(dir, 'empty.jsonl');
  const missing = path.join(dir, 'missing.jsonl');
  fs.writeFileSync(empty, '');

  const e = readJournal(empty);
  const m = readJournal(missing);
  // Both have zero records. That is precisely why byte count and existence are
  // recorded: "0 arrivals" from a fixture that never started and "0 arrivals"
  // from a product that dropped everything are opposite diagnoses.
  assert(e.records.length === 0 && m.records.length === 0, 'both should parse to zero records');
  assert(e.existed === true && m.existed === false, 'existence must distinguish them');
  fs.rmSync(dir, { recursive: true, force: true });
});

check('journal: byte count is non-zero for a populated journal', () => {
  const dir = fs.mkdtempSync(path.join(process.env.TMPDIR || '/tmp', 'f24selftest-'));
  const f = path.join(dir, 'j.jsonl');
  fs.writeFileSync(f, `${JSON.stringify({ text: 'hello' })}\n`);
  const j = readJournal(f);
  assert(j.records.length === 1, `want 1 record, got ${j.records.length}`);
  assert(j.bytes > 0, 'byte count must be non-zero');
  fs.rmSync(dir, { recursive: true, force: true });
});

// ─────────────────────────────────────────────────────────────────────────────
// 4. Source scan — the driver must not have reintroduced a bare includes()
// ─────────────────────────────────────────────────────────────────────────────
//
// A sibling lane's repair was PARTIAL: `arrivalsFor` was moved onto the shared
// module and `runMatrix`'s route check was NOT, so one call site stayed broken
// and reported `carries_correlation=false` about a reply that had arrived. A
// comment would not have caught it. This scan does.

check('source scan: the driver delegates correlation and never re-implements it', () => {
  const driverPath = path.join(path.dirname(new URL(import.meta.url).pathname), 'f24-c3-clauses.mjs');
  const src = fs.readFileSync(driverPath, 'utf8');

  // Guard against a VACUOUS pass. If the file were missing or truncated the
  // regexes below would all find nothing and this test would go green having
  // proven nothing at all.
  assert(src.length > 10_000, `driver source implausibly small (${src.length} bytes) — scan would be vacuous`);

  assert(
    /import \{[^}]*classify[^}]*\} from '\.\/f24-correlate\.mjs'/.test(src),
    'driver must import the shared matcher',
  );
  // The import must NOT be inside a try/catch with a local fallback: a silent
  // degradation to a hand-rolled matcher fails in the direction that blames the
  // product.
  assert(
    !/try\s*\{[^}]*await import\('\.\/f24-correlate/.test(src),
    'the correlate import must not be wrapped in a silent try/catch fallback',
  );
  // No bare `.includes(token)` anywhere in the driver.
  const bareIncludes = src.match(/\.includes\(\s*token\s*\)/g) || [];
  assert(
    bareIncludes.length === 0,
    `driver contains ${bareIncludes.length} bare .includes(token) call(s) — use the shared matcher`,
  );
});

check('source scan: every recorded leg passes a positive control', () => {
  const driverPath = path.join(path.dirname(new URL(import.meta.url).pathname), 'f24-c3-clauses.mjs');
  const src = fs.readFileSync(driverPath, 'utf8');
  assert(src.length > 10_000, 'driver source implausibly small — scan would be vacuous');
  // The grader must fold the control into the returned pass value. If someone
  // "simplifies" record() to `pass = ok`, the universal-denial trap reopens.
  assert(
    /const pass = ok && \(controlOk === null \|\| controlOk\)/.test(src),
    'record() must fold the positive control into the pass condition',
  );
  assert(
    /zeroArrivals && bound/.test(src),
    'the driver must force FAIL on a green with zero arrivals',
  );
});

// ─────────────────────────────────────────────────────────────────────────────

process.stdout.write(`\nselftest: passed=${passed} failed=${failed}\n`);
process.exit(failed === 0 ? 0 : 1);
